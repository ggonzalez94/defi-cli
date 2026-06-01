//! Shared execution `submit` plumbing (Go `internal/app/execution_helpers.go`
//! + the runner submit helpers in `internal/app/runner.go`).
//!
//! Standard-EVM execution `submit` commands (`approvals`/`transfer`/`bridge`/
//! `lend`/`yield`/`rewards`, plus TaikoSwap `swap submit`) load a persisted
//! action, resolve the signing/execution backend from the action's persisted
//! `execution_backend` + the submit signer flags, validate the resolved sender
//! against `--from-address` and the planned sender, parse the execute options,
//! run the pre-sign guardrails, and broadcast through the engine. This module
//! owns that group-independent glue:
//!
//! * [`resolve_action_execution_backend`] — Go `resolveActionExecutionBackend`:
//!   route on the persisted backend (legacy-local / OWS / Tempo), enforcing the
//!   legacy-vs-non-local-signer guard, the OWS `wallet_id` guard, and the OWS
//!   legacy-signer-flags guard. Legacy-local resolves a [`LocalSigner`] from the
//!   key inputs; OWS resolves the persisted wallet's per-chain sender.
//! * [`validate_execution_sender`] — Go `validateExecutionSender`: reject a
//!   resolved sender that does not match `--from-address` or the planned sender
//!   ([`Code::Signer`]).
//! * [`parse_execute_options`] — Go `parseExecuteOptions`: durations,
//!   `--gas-multiplier > 1`, fee flags, and the approval/provider-tx guard flags.
//! * [`presign_validate_action`] — the bounded-approval pre-sign guardrail run
//!   with the action context (the engine's per-step policy runs without it, so an
//!   inflated approval must be caught here to surface the documented
//!   `--allow-max-approval` hint).
//! * [`execute_resolved`] — Go `executeActionWithTimeout` → `ExecuteAction`:
//!   broadcast through the engine, persisting each transition.

use std::sync::Arc;
use std::time::Duration;

use defi_errors::{Code, Error};
use defi_evm::address;
use defi_evm::signer::LocalSigner;
use defi_execution::action::{Action, ExecutionBackend};
use defi_execution::evm_executor::{
    execute_action, parse_evm_chain_id, LocalSubmitBackend, OwsSubmitBackend,
};
use defi_execution::policy::{validate_step_policy, PolicyOptions};
use defi_execution::signer::{local_signer_from_inputs, KeySource};
use defi_execution::store::Store as ActionStore;
use defi_execution::ExecuteOptions;

/// The signer-related submit flags consumed by backend resolution.
///
/// Parity with Go `submitExecutionInputs`.
pub struct SubmitExecutionInputs<'a> {
    /// `--signer` backend (local|tempo).
    pub signer: &'a str,
    /// `--key-source` (auto|env|file|keystore).
    pub key_source: &'a str,
    /// `--private-key` hex override (less safe).
    pub private_key: &'a str,
    /// `--from-address` expected sender.
    pub from_address: &'a str,
}

/// The resolved submit execution: the effective sender plus the broadcast
/// backend (a local-key backend or an OWS wallet-backed backend).
///
/// Parity with Go `resolvedSubmitExecution` (minus the Tempo branch, which is a
/// separate execution path — Tempo `submit` is `--signer tempo` / WS4a).
pub struct ResolvedSubmitExecution {
    /// The on-chain sender address (EIP-55 checksum hex).
    pub sender: String,
    /// The resolved EVM broadcast backend.
    pub backend: ResolvedBackend,
}

/// The resolved EVM broadcast backend (local-key vs OWS wallet-backed).
pub enum ResolvedBackend {
    /// Local-key signer + broadcast backend.
    Local(LocalSigner),
    /// OWS wallet-backed submit backend (bound to a persisted `wallet_id`).
    Ows(OwsSubmitBackend),
}

/// Resolve the execution backend from a persisted action + the submit signer
/// flags, parity with Go `resolveActionExecutionBackend`.
///
/// - Legacy-local (or empty/default): only `--signer local` is allowed; any
///   other signer is a [`Code::Usage`] error. The local signer is initialized
///   from the key inputs (env/file/keystore + `--private-key`); an unresolvable
///   key is a [`Code::Signer`] error.
/// - OWS: requires a persisted `wallet_id` ([`Code::Usage`] otherwise) and
///   rejects explicit legacy signer flags ([`Code::Usage`]). The wallet's
///   per-chain sender is resolved through the OWS vault.
/// - Tempo: a separate execution path (`--signer tempo`); not supported by this
///   standard-EVM submit helper.
pub fn resolve_action_execution_backend(
    action: &Action,
    input: SubmitExecutionInputs<'_>,
) -> Result<ResolvedSubmitExecution, Error> {
    match action.execution_backend {
        None | Some(ExecutionBackend::LegacyLocal) => {
            let mut signer_backend = input.signer.trim().to_ascii_lowercase();
            if signer_backend.is_empty() {
                signer_backend = "local".to_string();
            }
            if signer_backend != "local" {
                return Err(Error::new(
                    Code::Usage,
                    "legacy actions only support --signer local; tempo submit requires execution_backend=tempo",
                ));
            }
            let signer = new_local_signer(input.key_source, input.private_key)?;
            let sender = signer.address().to_hex();
            Ok(ResolvedSubmitExecution {
                sender,
                backend: ResolvedBackend::Local(signer),
            })
        }
        Some(ExecutionBackend::Ows) => {
            if action.wallet_id.trim().is_empty() {
                return Err(Error::new(
                    Code::Usage,
                    "wallet-backed action is missing persisted wallet_id",
                ));
            }
            if uses_legacy_signer_flags(&input) {
                return Err(Error::new(
                    Code::Usage,
                    "wallet-backed actions do not accept legacy signer flags (--signer, --key-source, --private-key)",
                ));
            }
            let sender = resolve_persisted_ows_sender(action)?;
            let sender_addr = address::parse(&sender)?;
            let send_hook = Arc::new(
                |wallet_id: &str, chain_id: &str, tx_bytes: &[u8], rpc_url: &str| {
                    let runner = defi_ows::SystemCommandRunner;
                    let token = std::env::var(defi_ows::ENV_OWS_TOKEN).ok();
                    let result = defi_ows::send_unsigned_tx(
                        &runner,
                        token.as_deref(),
                        wallet_id,
                        chain_id,
                        tx_bytes,
                        rpc_url,
                    )?;
                    Ok(result.tx_hash)
                },
            );
            Ok(ResolvedSubmitExecution {
                backend: ResolvedBackend::Ows(
                    OwsSubmitBackend::new(action.wallet_id.clone(), sender_addr)
                        .with_send_hook(send_hook),
                ),
                sender,
            })
        }
        Some(ExecutionBackend::Tempo) => Err(Error::new(
            Code::Unsupported,
            "tempo execution backend submit is a separate execution path (use --signer tempo)",
        )),
    }
}

/// Build a local signer from the key inputs, parity with Go `newExecutionSigner`
/// (`local` branch): resolve the hex key via the env/file/keystore precedence +
/// `--private-key` override and parse it. A missing/unparseable key surfaces as a
/// [`Code::Signer`] error (Go wraps with `initialize local signer`).
fn new_local_signer(key_source: &str, private_key: &str) -> Result<LocalSigner, Error> {
    let source = KeySource::parse(key_source)?;
    local_signer_from_inputs(source, private_key, &defi_config::SystemEnv)
        .map_err(|err| Error::wrap(Code::Signer, "initialize local signer", err))
}

/// Whether the submit invocation set any explicit legacy signer flag.
///
/// Parity with Go `usesLegacySignerFlags` (`flag.Changed` on `signer`/
/// `key-source`/`private-key`). With clap-parsed structs there is no per-flag
/// "changed" bit, so a non-default value is treated as explicitly set: a
/// non-empty `--private-key`, a `--signer` other than `local`, or a
/// `--key-source` other than `auto`.
fn uses_legacy_signer_flags(input: &SubmitExecutionInputs<'_>) -> bool {
    if !input.private_key.trim().is_empty() {
        return true;
    }
    let signer = input.signer.trim().to_ascii_lowercase();
    if !signer.is_empty() && signer != "local" {
        return true;
    }
    let key_source = input.key_source.trim().to_ascii_lowercase();
    if !key_source.is_empty() && key_source != "auto" {
        return true;
    }
    false
}

/// Resolve a wallet-backed action's on-chain sender, parity with Go
/// `resolvePersistedOWSSender`.
fn resolve_persisted_ows_sender(action: &Action) -> Result<String, Error> {
    let mut chain_id = action.chain_id.trim().to_string();
    if chain_id.is_empty() {
        for step in &action.steps {
            if !step.chain_id.trim().is_empty() {
                chain_id = step.chain_id.trim().to_string();
                break;
            }
        }
    }
    if chain_id.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "wallet-backed action is missing chain id for sender resolution",
        ));
    }

    let wallet = defi_ows::resolve_wallet_ref("", &action.wallet_id)
        .map_err(|err| Error::wrap(err.code, "resolve persisted wallet_id", err))?;
    let sender = defi_ows::sender_address_for_chain(&wallet, &chain_id)
        .map_err(|err| Error::wrap(err.code, "resolve wallet sender for action chain", err))?;
    if !address::is_hex_address(&sender) {
        return Err(Error::new(
            Code::Unavailable,
            "resolved wallet sender must be a valid EVM hex address",
        ));
    }
    let canonical = address::checksum(&sender)?;
    let persisted = action.from_address.trim();
    if !persisted.is_empty() && !address::eq_fold(persisted, &canonical) {
        return Err(Error::new(
            Code::Signer,
            "planned action sender does not match resolved wallet sender",
        ));
    }
    Ok(canonical)
}

/// Validate the resolved sender against `--from-address` and the planned action
/// sender, parity with Go `validateExecutionSender`.
///
/// A non-empty `expected_sender` (`--from-address`) that does not match the
/// resolved sender is a [`Code::Signer`] error; likewise a non-empty persisted
/// `action.from_address` that does not match.
pub fn validate_execution_sender(
    action: &Action,
    expected_sender: &str,
    actual_sender: &str,
) -> Result<(), Error> {
    let expected = expected_sender.trim();
    if !expected.is_empty() && !address::eq_fold(expected, actual_sender) {
        return Err(Error::new(
            Code::Signer,
            "signer address does not match --from-address",
        ));
    }
    let persisted = action.from_address.trim();
    if !persisted.is_empty() && !address::eq_fold(persisted, actual_sender) {
        return Err(Error::new(
            Code::Signer,
            "signer address does not match planned action sender",
        ));
    }
    Ok(())
}

/// The flag-derived inputs to [`parse_execute_options`] (Go `parseExecuteOptions`
/// args).
pub struct ExecuteOptionInputs<'a> {
    /// `--simulate`.
    pub simulate: bool,
    /// `--poll-interval` (Go duration string).
    pub poll_interval: &'a str,
    /// `--step-timeout` (Go duration string).
    pub step_timeout: &'a str,
    /// `--gas-multiplier` (must be `> 1`).
    pub gas_multiplier: f64,
    /// `--max-fee-gwei`.
    pub max_fee_gwei: &'a str,
    /// `--max-priority-fee-gwei`.
    pub max_priority_fee_gwei: &'a str,
    /// `--allow-max-approval`.
    pub allow_max_approval: bool,
    /// `--unsafe-provider-tx`.
    pub unsafe_provider_tx: bool,
    /// `--fee-token` (Tempo only).
    pub fee_token: &'a str,
}

/// Parse the execute options, parity with Go `parseExecuteOptions`.
///
/// Durations use the Go `time.ParseDuration` grammar; a non-positive
/// poll-interval / step-timeout is a [`Code::Usage`] error, as is a
/// `--gas-multiplier <= 1`.
pub fn parse_execute_options(input: &ExecuteOptionInputs<'_>) -> Result<ExecuteOptions, Error> {
    let defaults = ExecuteOptions::default();

    // Resolve the durations first (defaulting when the flag is empty) so the
    // final options can be built in a single initializer. The Go grammar +
    // non-positive guard ordering (poll, then step, then gas) is preserved.
    let poll_interval = if input.poll_interval.trim().is_empty() {
        defaults.poll_interval
    } else {
        let d = parse_go_duration(input.poll_interval)
            .map_err(|e| Error::new(Code::Usage, format!("parse --poll-interval: {e}")))?;
        if d.is_zero() {
            return Err(Error::new(Code::Usage, "--poll-interval must be > 0"));
        }
        d
    };
    let step_timeout = if input.step_timeout.trim().is_empty() {
        defaults.step_timeout
    } else {
        let d = parse_go_duration(input.step_timeout)
            .map_err(|e| Error::new(Code::Usage, format!("parse --step-timeout: {e}")))?;
        if d.is_zero() {
            return Err(Error::new(Code::Usage, "--step-timeout must be > 0"));
        }
        d
    };
    if input.gas_multiplier <= 1.0 {
        return Err(Error::new(Code::Usage, "--gas-multiplier must be > 1"));
    }

    Ok(ExecuteOptions {
        simulate: input.simulate,
        poll_interval,
        step_timeout,
        gas_multiplier: input.gas_multiplier,
        max_fee_gwei: input.max_fee_gwei.trim().to_string(),
        max_priority_fee_gwei: input.max_priority_fee_gwei.trim().to_string(),
        allow_max_approval: input.allow_max_approval,
        unsafe_provider_tx: input.unsafe_provider_tx,
        fee_token: input.fee_token.trim().to_string(),
    })
}

/// Run the pre-sign policy guardrails over each pending step WITH the action
/// context.
///
/// The engine's per-step policy (`execute_evm_step`) runs without the action
/// context, so the bounded-approval bound check (which needs the action's
/// `input_amount`) must run here to surface the documented `--allow-max-approval`
/// override hint (an inflated approval without the opt-in → [`Code::ActionPlan`]).
/// Confirmed steps are skipped (they already broadcast).
pub fn presign_validate_action(action: &Action, opts: &ExecuteOptions) -> Result<(), Error> {
    let policy_opts = PolicyOptions {
        allow_max_approval: opts.allow_max_approval,
        unsafe_provider_tx: opts.unsafe_provider_tx,
    };
    for step in &action.steps {
        if step.status == defi_execution::action::StepStatus::Confirmed {
            continue;
        }
        let chain_id = parse_evm_chain_id(step.chain_id.trim()).unwrap_or(0);
        let data = decode_step_data(&step.data)?;
        validate_step_policy(Some(action), step, chain_id, &data, &policy_opts)?;
    }
    Ok(())
}

/// Broadcast a resolved action through the engine, persisting each transition.
///
/// Parity with Go `executeActionWithTimeout` → `execution.ExecuteAction`: the
/// resolved backend (local-key or OWS) drives sign+broadcast; the engine owns
/// simulation/gas/nonce/receipt and persists each step transition to the store.
pub async fn execute_resolved(
    store: &ActionStore,
    action: &mut Action,
    resolved: ResolvedSubmitExecution,
    opts: ExecuteOptions,
) -> Result<(), Error> {
    match resolved.backend {
        ResolvedBackend::Local(signer) => {
            // Pass the explicit local backend so the engine broadcasts via the
            // resolved key (no implicit re-derivation).
            let backend = LocalSubmitBackend::new(signer);
            execute_action(Some(store), action, None, Some(backend), opts).await
        }
        ResolvedBackend::Ows(backend) => {
            execute_action(Some(store), action, None, Some(backend), opts).await
        }
    }
}

/// Decode a `0x`-prefixed (or bare) hex step calldata string into bytes.
fn decode_step_data(value: &str) -> Result<Vec<u8>, Error> {
    let trimmed = value.trim();
    let body = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    if body.is_empty() {
        return Ok(Vec::new());
    }
    hex::decode(body).map_err(|e| Error::wrap(Code::Usage, "decode step calldata", e))
}

/// Parse a Go-style duration string (`time.ParseDuration` grammar) into a
/// [`Duration`].
///
/// Supports the common units the execution flags use (`ns`/`us`/`µs`/`ms`/`s`/
/// `m`/`h`), signed/fractional components, and multi-unit concatenation
/// (e.g. `1m30s`). A bare number or an unknown unit is an error. Only
/// non-negative durations are returned (the submit guards reject `<= 0`).
fn parse_go_duration(input: &str) -> Result<Duration, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("empty duration".to_string());
    }
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    if neg {
        // Go accepts negative durations, but every submit guard rejects `<= 0`,
        // so a negative duration collapses to zero (which the caller rejects).
        return Ok(Duration::ZERO);
    }

    let mut total_nanos: f64 = 0.0;
    let mut chars = rest.char_indices().peekable();
    let mut consumed_any = false;
    while let Some(&(start, c)) = chars.peek() {
        if !(c.is_ascii_digit() || c == '.') {
            return Err(format!("invalid duration {input:?}"));
        }
        // Consume the numeric component.
        let mut end = start;
        while let Some(&(i, ch)) = chars.peek() {
            if ch.is_ascii_digit() || ch == '.' {
                end = i + ch.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let num: f64 = rest[start..end]
            .parse()
            .map_err(|_| format!("invalid duration number in {input:?}"))?;
        // Consume the unit component.
        let unit_start = end;
        let mut unit_end = end;
        while let Some(&(i, ch)) = chars.peek() {
            if ch.is_ascii_alphabetic() || ch == 'µ' {
                unit_end = i + ch.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let unit = &rest[unit_start..unit_end];
        if unit.is_empty() {
            return Err(format!("missing unit in duration {input:?}"));
        }
        let mult = match unit {
            "ns" => 1.0,
            "us" | "µs" | "μs" => 1_000.0,
            "ms" => 1_000_000.0,
            "s" => 1_000_000_000.0,
            "m" => 60.0 * 1_000_000_000.0,
            "h" => 3_600.0 * 1_000_000_000.0,
            other => return Err(format!("unknown unit {other:?} in duration {input:?}")),
        };
        total_nanos += num * mult;
        consumed_any = true;
    }
    if !consumed_any {
        return Err(format!("invalid duration {input:?}"));
    }
    Ok(Duration::from_nanos(total_nanos as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_durations() {
        assert_eq!(parse_go_duration("2s").unwrap(), Duration::from_secs(2));
        assert_eq!(parse_go_duration("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(
            parse_go_duration("500ms").unwrap(),
            Duration::from_millis(500)
        );
        assert_eq!(parse_go_duration("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_go_duration("0s").unwrap(), Duration::ZERO);
        assert_eq!(parse_go_duration("0").unwrap(), Duration::ZERO);
    }

    #[test]
    fn rejects_unparseable_durations() {
        assert!(parse_go_duration("nope").is_err());
        assert!(parse_go_duration("10").is_err()); // bare number, no unit
        assert!(parse_go_duration("").is_err());
    }
}
