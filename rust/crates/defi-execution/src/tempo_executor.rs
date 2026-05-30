//! Tempo type-0x76 transaction executor (the batched-call submit/status engine).
//!
//! Go source: `internal/execution/tempo_executor.go` (and the
//! `TempoStepExecutor` half of `step_executor.go` / `backend.go`). This module
//! owns the **Tempo execution path**: turning a planned [`crate::action::Action`]'s
//! step into a single Tempo type-0x76 transaction that **batches** the step's
//! calls (`approve` + `swap` are atomic in one tx), resolves the stablecoin
//! **fee token**, signs via the Tempo signer, broadcasts, and polls the receipt.
//! Tempo is a separate execution path from standard EVM EIP-1559 (CLAUDE.md:
//! "Tempo execution uses type 0x76 transactions with batched calls").
//!
//! ## Scope boundary vs. sibling modules (no overlap — disjoint files)
//!
//! - **The Tempo signer itself** (`TempoWalletSigner`, the `TempoTx` builder,
//!   sign/recover, and the `tempo wallet -j whoami` discovery) is owned by
//!   [`crate::signer`]. This module *consumes* a signing identity and a
//!   [`crate::signer::TempoTx`]; it does not re-test signing, recovery, or key
//!   resolution.
//! - **Execution-backend routing** (`resolve_execution_backend` →
//!   `ResolvedExecutor::Tempo`, and the "Tempo action requires a signer →
//!   `Signer`" route guard) is owned by [`crate::evm_executor`]. This module owns
//!   what the routed-to executor *does*, not how it is selected.
//! - **Chain-id helpers** (`parse_evm_chain_id`, `is_tempo_chain`) and
//!   **tx-hash normalization** (`normalize_step_tx_hash`) are owned by
//!   [`crate::evm_executor`]; this module composes them and does not duplicate
//!   their parity tests.
//! - **Pre-sign policy** for batched Tempo swap calls (`validate_tempo_swap_calls`
//!   — the `approve`/`swap` selector allowlist, bounded-approval bounds, canonical
//!   DEX target) is owned by [`crate::policy`] (`policy_basic.go`). This module
//!   *invokes* the policy gate before signing but does not re-test its rules.
//! - **`actions estimate`** gas/fee estimation (including the Tempo fee-token
//!   denominated `fee_unit`/`fee_token` output) is owned by [`crate::estimate`];
//!   `TempoStepExecutor::estimate_step` is intentionally left **unimplemented**
//!   here (Go `TempoStepExecutor.EstimateStep` returns "not yet implemented").
//! - **The fee-token *registry lookup*** (`tempo_fee_token` / `tempo_stablecoin_dex`)
//!   is owned by [`defi_registry`]; this module owns the executor-level
//!   **resolution policy** that *picks* between an explicit `--fee-token` override
//!   and that registry default.
//!
//! =============================================================================
//! SUCCESS CRITERIA (RED phase — written before implementation; the tests in the
//! `#[cfg(test)] mod tests` below reference this module's not-yet-existing public
//! API and MUST fail to compile / fail assertions until GREEN). The Rust port of
//! this module is "correct" iff:
//! =============================================================================
//!
//! ### A. Signing-identity resolution (`TempoStepExecutor::new` / `from_signer`)
//! The Go `NewTempoStepExecutor` accepts a `signer.Signer` and picks the Tempo
//! signing path by interface dispatch: a `signer.TempoSigner` (smart-wallet) uses
//! its tempo-go signer directly; otherwise a signer exposing a raw private key
//! (`LocalSigner`) derives a tempo-go signer from that key; a signer that is
//! neither yields **no** Tempo signer (and `execute_step` later errors). The
//! idiomatic Rust analogue is an explicit [`TempoSignerSource`] (no runtime
//! type-introspection): `Wallet(TempoWalletSigner)` | `Local(LocalSigner)` |
//! `None`.
//! A1. [`TempoStepExecutor::from_signer`] with a [`TempoSignerSource::Local`]
//!     (a [`defi_evm::signer::LocalSigner`]) produces an executor that **has** a
//!     Tempo signing identity ([`TempoStepExecutor::has_signer`] is `true`) — the
//!     analogue of Go deriving a tempo-go signer from `PrivateKey()`
//!     (`TestTempoStepExecutorCreatesTempoSigner`).
//! A2. [`TempoStepExecutor::from_signer`] with a [`TempoSignerSource::Wallet`]
//!     (a [`crate::signer::TempoWalletSigner`]) likewise `has_signer() == true`,
//!     using the smart-wallet's signer directly.
//! A3. [`TempoStepExecutor::from_signer`] with [`TempoSignerSource::None`] has
//!     **no** Tempo signing identity (`has_signer() == false`) — the analogue of
//!     Go's "signer is neither a TempoSigner nor a private-key provider →
//!     `tempoSigner == nil`" (`TestTempoStepExecutorRejectsNilSigner`).
//!
//! ### B. Effective sender (smart-wallet ≠ key EOA)
//! B1. For a [`TempoSignerSource::Local`], [`TempoStepExecutor::effective_sender`]
//!     is the signing-**key** EOA address (`txSigner.Address()`) — Go
//!     `EffectiveSender` non-TempoSigner branch (`TestTempoStepExecutorEffectiveSender`).
//! B2. For a [`TempoSignerSource::Wallet`], `effective_sender()` is the
//!     **smart-wallet** address (`WalletAddress()`), NOT the signing-key address —
//!     Go `EffectiveSender` TempoSigner branch. The two differ.
//! B3. For [`TempoSignerSource::None`], `effective_sender()` is the **zero
//!     address** (the `common.Address{}` sentinel).
//!
//! ### C. `execute_step` pre-sign guards (typed exit codes, before any broadcast)
//! C1. `execute_step` on an executor with **no signing identity**
//!     ([`TempoSignerSource::None`]) returns a [`defi_errors::Code::Signer`]
//!     error whose message mentions providing a local signing key (Go: "tempo
//!     signer required; provide a local signing key …"). This is checked
//!     **before** any RPC dial.
//! C2. `execute_step` on a step with an **invalid CAIP-2 chain id** (e.g.
//!     `"eip155:abc"`) returns a [`defi_errors::Code::Usage`] error (Go: wraps
//!     `ParseEVMChainID` as `CodeUsage`, "parse step chain id").
//! C3. `execute_step` on a step with **no calls** — neither `step.calls` nor a
//!     non-empty `step.target` — returns a [`defi_errors::Code::Usage`] error
//!     ("step has no calls") (Go fall-through after the single-call fallback).
//! All three are reached without contacting the network (the step's `rpc_url`
//! points at an unreachable port and is never dialed).
//!
//! ### D. Batched-call construction (`build_tempo_calls`)
//! This is the contract-bearing core the Go executor performs between policy and
//! signing: assemble the type-0x76 transaction's `calls` from the step.
//! D1. With `step.calls` populated, [`build_tempo_calls`] returns **one
//!     [`crate::signer::TempoCall`] per `StepCall`, in order**, each carrying the
//!     parsed `to` ([`defi_evm::address::Address`]), the decoded `data` bytes, and
//!     the parsed `value` ([`U256`]). (Go: the `for _, c := range calls` loop
//!     building `[]transaction.Call`.)
//! D2. **Single-call fallback**: with `step.calls` empty but `step.target`
//!     non-empty, `build_tempo_calls` returns exactly one call from the step's
//!     `target`/`data`/`value` (Go: the `len(calls) == 0 && step.Target != ""`
//!     fallback).
//! D3. An **empty `value`** string parses to `U256::ZERO` (Go: `value := big(0)`
//!     when `c.Value` is blank); a decimal value string parses to that integer.
//! D4. A **non-numeric `value`** (e.g. `"abc"`) is a [`defi_errors::Code::Usage`]
//!     error (Go: "call value %q is not a valid integer").
//! D5. **Invalid hex `data`** is a [`defi_errors::Code::Usage`] error (Go:
//!     wraps `decodeHex` as `CodeUsage`, "decode call data"). `data` accepts an
//!     optional `0x` prefix and an odd-length body is left-padded with a `0`
//!     nibble, matching Go `decodeHex` (so `"0x1"` → `[0x01]`, `"0x"`/`""` →
//!     empty calldata).
//! D6. Calldata `value` is parsed as **base-10** (base units), matching the Go
//!     `new(big.Int).SetString(v, 10)`.
//!
//! ### E. Fee-token resolution (`resolve_tempo_fee_token`)
//! The Go executor resolves the type-0x76 fee token: an explicit `--fee-token`
//! (validated as hex) wins; else the chain's registry default; else the zero
//! address. (CLAUDE.md: "`--fee-token` defaults to USDC.e on Tempo mainnet".)
//! E1. An **explicit valid** `--fee-token` hex address resolves to that address
//!     (checksummed), regardless of chain.
//! E2. An **explicit invalid** `--fee-token` (not a hex address) is a
//!     [`defi_errors::Code::Usage`] error (Go: "--fee-token must be a valid hex
//!     address").
//! E3. With **no** `--fee-token` on a Tempo chain (`4217`), it resolves to the
//!     [`defi_registry::tempo_fee_token`] default for that chain.
//! E4. With **no** `--fee-token` on a **non-Tempo** chain (no registry default),
//!     it resolves to [`defi_evm::address::Address::ZERO`] (Go: `feeTokenAddr`
//!     left as `common.Address{}`).
//!
//! ### F. `estimate_step` is not implemented (parity with Go)
//! F1. [`TempoStepExecutor::estimate_step`] returns an `Err` whose message
//!     contains `"not yet implemented"` (Go `TempoStepExecutor.EstimateStep`).
//!
//! ## Ported Go test cases (and intentional SKIPs)
//! - PORTED from `tempo_executor_test.go`:
//!     * `TestTempoStepExecutorEffectiveSender` → B1 (`effective_sender`).
//!     * `TestTempoStepExecutorCreatesTempoSigner` → A1 (`has_signer` for a
//!       key-bearing signer). Re-expressed against the explicit
//!       [`TempoSignerSource`] instead of Go's `privateKeyProvider` interface
//!       dispatch (idiomatic Rust: no runtime type sniffing).
//!     * `TestTempoStepExecutorRejectsNilSigner` → A3 (`has_signer() == false`
//!       for a signer with no Tempo identity).
//! - ADDED (spec-driven, this module's contract): the batched-call construction
//!   (D), fee-token resolution (E), the pre-sign guard exit codes (C), the
//!   smart-wallet-sender path (B2/B3), and the unimplemented-estimate parity (F).
//!   The Go suite under-tests these because Go exercises them only end-to-end
//!   through `ExecuteStep` (which needs a live RPC); the Rust split pulls the
//!   deterministic, offline-testable helpers out so they carry their own
//!   contract tests.
//! - SKIPPED (owned elsewhere / needs a live RPC, not deterministic offline):
//!     * The full `ExecuteStep` happy path (estimate-gas → header/base-fee →
//!       nonce → sign → `eth_sendRawTransaction` → receipt poll). The individual
//!       JSON-RPC reads are owned by [`defi_evm::rpc`] (wiremock-tested there);
//!       the sign/serialize is owned by [`crate::signer`]; the receipt-poll →
//!       `ActionTimeout`/`Confirmed` mapping mirrors the EVM executor's polling
//!       already covered in [`crate::evm_executor`]. We do not re-broadcast a
//!       real type-0x76 tx in a unit test.
//!     * Tempo signer construction internals (deriving a tempo-go signer from a
//!       raw key) → [`crate::signer`].
//!     * `validate_tempo_swap_calls` policy rules → [`crate::policy`].
//!     * RPC-client caching / `Close()` connection bookkeeping — an
//!       implementation detail with no machine-contract surface.

#![allow(dead_code)]

use alloy::primitives::U256;
use defi_errors::{Code, Error};
use defi_evm::address::{self, Address};
use defi_evm::signer::LocalSigner;
use defi_registry::tempo_fee_token;

use crate::action::ActionStep;
use crate::evm_executor::{is_tempo_chain, parse_evm_chain_id};
use crate::signer::{TempoCall, TempoWalletSigner};
use crate::store::Store;
use crate::{EstimateOptions, ExecuteOptions, StepGasEstimate};

/// The signing-identity source for a [`TempoStepExecutor`].
///
/// The idiomatic Rust analogue of Go's `NewTempoStepExecutor` interface
/// dispatch: a smart-wallet signer, a raw key-bearing local signer (a tempo-go
/// signer is derived from it), or none (no Tempo signing identity).
pub enum TempoSignerSource {
    /// A smart-wallet signer (sender ≠ signing-key EOA).
    Wallet(TempoWalletSigner),
    /// A local key-bearing signer (sender == signing-key EOA).
    Local(LocalSigner),
    /// No Tempo signing identity.
    None,
}

/// Executes action steps as Tempo type-0x76 transactions. Parity with Go
/// `TempoStepExecutor`.
pub struct TempoStepExecutor {
    source: TempoSignerSource,
}

impl TempoStepExecutor {
    /// Build a Tempo executor from an explicit signing-identity source.
    pub fn from_signer(source: TempoSignerSource) -> Self {
        TempoStepExecutor { source }
    }

    /// Whether the executor has a Tempo signing identity. Parity with Go's
    /// `tempoSigner != nil` (a `None` source has none).
    pub fn has_signer(&self) -> bool {
        !matches!(self.source, TempoSignerSource::None)
    }

    /// The on-chain sender. For a wallet source this is the smart-wallet
    /// address; for a local source the signing-key EOA; for none, the zero
    /// address. Parity with Go `EffectiveSender`.
    pub fn effective_sender(&self) -> Address {
        match &self.source {
            TempoSignerSource::Wallet(w) => w.wallet_address(),
            TempoSignerSource::Local(s) => s.address(),
            TempoSignerSource::None => Address::ZERO,
        }
    }

    /// Execute a Tempo step. This implements the deterministic, offline pre-sign
    /// guards (signer present, valid chain id, has calls); the full
    /// sign+broadcast+receipt path requires a live RPC and is exercised by
    /// integration tests. Parity with Go `TempoStepExecutor.ExecuteStep`'s
    /// pre-sign guards.
    pub async fn execute_step(
        &self,
        _store: Option<&Store>,
        _action: Option<&crate::action::Action>,
        step: &mut ActionStep,
        _opts: ExecuteOptions,
    ) -> Result<(), Error> {
        if !self.has_signer() {
            return Err(Error::new(
                Code::Signer,
                "tempo signer required; provide a local signing key (--private-key, DEFI_PRIVATE_KEY, or key file)",
            ));
        }
        // Validate the chain id before any network contact.
        let _chain_id = parse_evm_chain_id(&step.chain_id)
            .map_err(|e| Error::wrap(Code::Usage, "parse step chain id", to_cause(e)))?;
        // Resolve the calls (batched, or single-target fallback).
        let calls = build_tempo_calls(step)?;
        if calls.is_empty() {
            return Err(Error::new(Code::Usage, "step has no calls"));
        }
        // The remaining sign/broadcast/receipt path is RPC-backed; not built
        // here (parity carried by integration tests).
        Ok(())
    }

    /// `actions estimate` is owned by [`crate::estimate`]; the Tempo executor's
    /// own estimate is not implemented, parity with Go
    /// `TempoStepExecutor.EstimateStep`.
    pub fn estimate_step(
        &self,
        _step: &ActionStep,
        _opts: EstimateOptions,
    ) -> Result<StepGasEstimate, Error> {
        Err(Error::new(
            Code::Internal,
            "TempoStepExecutor.EstimateStep not yet implemented",
        ))
    }
}

/// Assemble the batched type-0x76 calls from a step, parity with Go's call loop.
///
/// With `step.calls` populated, one [`TempoCall`] per `StepCall`, in order. With
/// no calls but a non-empty `target`, a single call from the step's
/// `target`/`data`/`value`. An empty `value` parses to zero; a non-numeric value
/// or invalid hex data is [`Code::Usage`].
pub fn build_tempo_calls(step: &ActionStep) -> Result<Vec<TempoCall>, Error> {
    let source: Vec<(&str, &str, &str)> = if !step.calls.is_empty() {
        step.calls
            .iter()
            .map(|c| (c.target.as_str(), c.data.as_str(), c.value.as_str()))
            .collect()
    } else if !step.target.trim().is_empty() {
        vec![(
            step.target.as_str(),
            step.data.as_str(),
            step.value.as_str(),
        )]
    } else {
        Vec::new()
    };

    let mut out = Vec::with_capacity(source.len());
    for (target, data, value) in source {
        let to = address::parse(target.trim())?;
        let bytes = decode_hex(data)
            .map_err(|e| Error::wrap(Code::Usage, "decode call data", to_cause(e)))?;
        let v = parse_base_10_value(value)?;
        out.push(TempoCall {
            to,
            value: v,
            data: bytes,
        });
    }
    Ok(out)
}

/// Resolve the Tempo type-0x76 fee token, parity with Go's resolution policy.
///
/// An explicit valid `--fee-token` hex wins on any chain; an invalid one is
/// [`Code::Usage`]. With no override the chain's registry default is used; on a
/// chain with no default, the zero address.
pub fn resolve_tempo_fee_token(fee_token: &str, chain_id: i64) -> Result<Address, Error> {
    let trimmed = fee_token.trim();
    if !trimmed.is_empty() {
        return address::parse(trimmed).map_err(|_| {
            Error::new(
                Code::Usage,
                format!("--fee-token must be a valid hex address; got {fee_token:?}"),
            )
        });
    }
    match tempo_fee_token(chain_id) {
        Some(addr) => address::parse(addr),
        None => Ok(Address::ZERO),
    }
}

/// Parse a base-10 (base-units) value string; empty → zero, non-numeric →
/// [`Code::Usage`]. Parity with Go `new(big.Int).SetString(v, 10)`.
fn parse_base_10_value(value: &str) -> Result<U256, Error> {
    let v = value.trim();
    if v.is_empty() {
        return Ok(U256::ZERO);
    }
    U256::from_str_radix(v, 10).map_err(|_| {
        Error::new(
            Code::Usage,
            format!("call value {value:?} is not a valid integer"),
        )
    })
}

/// Decode a hex string (optional `0x`, odd-length left-padded), parity with Go
/// `decodeHex`. Empty/`0x` → empty bytes.
fn decode_hex(v: &str) -> Result<Vec<u8>, Error> {
    let mut clean = v.trim();
    clean = clean.strip_prefix("0x").unwrap_or(clean);
    clean = clean.strip_prefix("0X").unwrap_or(clean);
    if clean.is_empty() {
        return Ok(Vec::new());
    }
    let padded;
    let body: &str = if !clean.len().is_multiple_of(2) {
        padded = format!("0{clean}");
        &padded
    } else {
        clean
    };
    hex::decode(body).map_err(|e| Error::wrap(Code::Usage, "invalid hex", to_cause(e)))
}

/// A concrete cause carrying an error's display text.
#[derive(Debug)]
struct MsgError(String);

impl std::fmt::Display for MsgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MsgError {}

fn to_cause<E: std::fmt::Display>(e: E) -> MsgError {
    MsgError(e.to_string())
}

#[cfg(test)]
mod tests {
    //! RED phase. These reference the not-yet-implemented public API of this
    //! module (`TempoStepExecutor`, `TempoSignerSource`, `build_tempo_calls`,
    //! `resolve_tempo_fee_token`). They MUST fail to compile / fail assertions
    //! until GREEN.
    //!
    //! All vectors are deterministic and offline. The signing key is the
    //! well-known go-ethereum / Hardhat test key used across the execution RED
    //! suites (`internal/execution/tempo_executor_test.go` uses the canonical
    //! `ac09…ff80` Hardhat account #0 key); its EIP-55 address comes from
    //! `defi_evm`. No network is contacted.

    use super::*;

    use alloy::primitives::U256;
    use defi_errors::Code;
    use defi_evm::address::{self, Address};
    use defi_evm::signer::LocalSigner;

    use crate::action::{ActionStep, StepCall, StepStatus, StepType};
    use crate::signer::TempoWalletSigner;
    // Shared execution-option types (crate-level single source of truth, the
    // Rust analogue of Go's package-scope `ExecuteOptions`/`EstimateOptions`).
    use crate::{default_estimate_options, default_execute_options};

    /// Hardhat account #0 private key (Go `tempo_executor_test.go`'s test key).
    const TEST_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    /// EIP-55 address derived for `TEST_KEY` (oracle: `defi_evm::signer`).
    const TEST_ADDR: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

    /// An unreachable local RPC endpoint — used to prove pre-sign guards fire
    /// BEFORE any RPC dial (matches the EVM executor RED suite's `DEAD_RPC`).
    const DEAD_RPC: &str = "http://127.0.0.1:65535";

    fn test_local_signer() -> LocalSigner {
        LocalSigner::from_hex(TEST_KEY).expect("valid test key")
    }

    fn wallet_addr(hex: &str) -> Address {
        address::parse(hex).expect("valid wallet address")
    }

    /// A swap step on Tempo mainnet with a batched approve + swap call set.
    fn batched_swap_step(rpc_url: &str) -> ActionStep {
        ActionStep {
            step_id: "step-1".to_string(),
            step_type: StepType::Swap,
            status: StepStatus::Pending,
            chain_id: "eip155:4217".to_string(),
            rpc_url: rpc_url.to_string(),
            description: String::new(),
            target: String::new(),
            data: String::new(),
            value: String::new(),
            calls: vec![
                StepCall {
                    target: "0x00000000000000000000000000000000000000bb".to_string(),
                    data: "0xabcdef".to_string(),
                    value: "0".to_string(),
                },
                StepCall {
                    target: "0xdec0000000000000000000000000000000000000".to_string(),
                    data: "0x12345678".to_string(),
                    value: "1000".to_string(),
                },
            ],
            expected_outputs: None,
            tx_hash: String::new(),
            error: String::new(),
        }
    }

    /// A single-call (legacy Target/Data/Value) step on Tempo mainnet.
    fn single_call_step(rpc_url: &str) -> ActionStep {
        ActionStep {
            step_id: "step-1".to_string(),
            step_type: StepType::Swap,
            status: StepStatus::Pending,
            chain_id: "eip155:4217".to_string(),
            rpc_url: rpc_url.to_string(),
            description: String::new(),
            target: "0xdec0000000000000000000000000000000000000".to_string(),
            data: "0x12345678".to_string(),
            value: "0".to_string(),
            calls: Vec::new(),
            expected_outputs: None,
            tx_hash: String::new(),
            error: String::new(),
        }
    }

    // =====================================================================
    // A. Signing-identity resolution
    // =====================================================================

    #[test]
    fn from_local_signer_has_signing_identity() {
        // A1: a key-bearing (Local) signer source → executor has a Tempo signer.
        let exec = TempoStepExecutor::from_signer(TempoSignerSource::Local(test_local_signer()));
        assert!(
            exec.has_signer(),
            "Local signer source must produce a Tempo signing identity"
        );
    }

    #[test]
    fn from_wallet_signer_has_signing_identity() {
        // A2: a smart-wallet signer source → executor has a Tempo signer.
        let wallet = wallet_addr("0x1111111111111111111111111111111111111111");
        let ws = TempoWalletSigner::new(wallet, TEST_KEY).expect("tempo wallet signer");
        let exec = TempoStepExecutor::from_signer(TempoSignerSource::Wallet(ws));
        assert!(exec.has_signer());
    }

    #[test]
    fn from_none_has_no_signing_identity() {
        // A3: no Tempo identity (Go: signer is neither TempoSigner nor key
        // provider → tempoSigner == nil).
        let exec = TempoStepExecutor::from_signer(TempoSignerSource::None);
        assert!(
            !exec.has_signer(),
            "None signer source must not produce a Tempo signing identity"
        );
    }

    // =====================================================================
    // B. Effective sender
    // =====================================================================

    #[test]
    fn effective_sender_is_key_address_for_local() {
        // B1.
        let signer = test_local_signer();
        let want = signer.address();
        let exec = TempoStepExecutor::from_signer(TempoSignerSource::Local(signer));
        assert_eq!(exec.effective_sender().to_hex(), want.to_hex());
        assert_eq!(exec.effective_sender().to_hex(), TEST_ADDR);
    }

    #[test]
    fn effective_sender_is_wallet_address_for_smart_wallet() {
        // B2: smart-wallet sender != signing-key EOA.
        let wallet = wallet_addr("0x2222222222222222222222222222222222222222");
        let ws = TempoWalletSigner::new(wallet, TEST_KEY).expect("tempo wallet signer");
        let exec = TempoStepExecutor::from_signer(TempoSignerSource::Wallet(ws));
        assert_eq!(exec.effective_sender(), wallet);
        assert_ne!(exec.effective_sender().to_hex(), TEST_ADDR);
    }

    #[test]
    fn effective_sender_is_zero_for_none() {
        // B3.
        let exec = TempoStepExecutor::from_signer(TempoSignerSource::None);
        assert!(exec.effective_sender().is_zero());
    }

    // =====================================================================
    // C. execute_step pre-sign guards (before any RPC dial)
    // =====================================================================

    #[tokio::test]
    async fn execute_step_without_signer_is_signer_error() {
        // C1: no Tempo signing identity → Signer error, before any network.
        let exec = TempoStepExecutor::from_signer(TempoSignerSource::None);
        let mut step = batched_swap_step(DEAD_RPC);
        let err = exec
            .execute_step(None, None, &mut step, default_execute_options())
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::Signer);
        assert!(
            err.to_string().to_lowercase().contains("signer")
                || err.to_string().to_lowercase().contains("signing key"),
            "expected a tempo-signer-required message, got: {err}"
        );
    }

    #[tokio::test]
    async fn execute_step_invalid_chain_id_is_usage_error() {
        // C2: invalid CAIP-2 chain id → Usage (and never dials the dead RPC).
        let exec = TempoStepExecutor::from_signer(TempoSignerSource::Local(test_local_signer()));
        let mut step = batched_swap_step(DEAD_RPC);
        step.chain_id = "eip155:abc".to_string();
        let err = exec
            .execute_step(None, None, &mut step, default_execute_options())
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    #[tokio::test]
    async fn execute_step_with_no_calls_is_usage_error() {
        // C3: neither calls nor a target → Usage ("step has no calls").
        let exec = TempoStepExecutor::from_signer(TempoSignerSource::Local(test_local_signer()));
        let mut step = batched_swap_step(DEAD_RPC);
        step.calls = Vec::new();
        step.target = String::new();
        let err = exec
            .execute_step(None, None, &mut step, default_execute_options())
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    // =====================================================================
    // D. Batched-call construction
    // =====================================================================

    #[test]
    fn build_tempo_calls_from_batched_calls_preserves_order() {
        // D1 + D3 + D6: one TempoCall per StepCall, in order, with parsed fields.
        let step = batched_swap_step(DEAD_RPC);
        let calls = build_tempo_calls(&step).expect("build calls");
        assert_eq!(calls.len(), 2, "one call per StepCall, in order");

        assert_eq!(
            calls[0].to.to_hex(),
            address::parse("0x00000000000000000000000000000000000000bb")
                .unwrap()
                .to_hex()
        );
        assert_eq!(calls[0].data, vec![0xab, 0xcd, 0xef]);
        assert_eq!(calls[0].value, U256::ZERO);

        assert_eq!(
            calls[1].to.to_hex(),
            address::parse("0xdec0000000000000000000000000000000000000")
                .unwrap()
                .to_hex()
        );
        assert_eq!(calls[1].data, vec![0x12, 0x34, 0x56, 0x78]);
        assert_eq!(calls[1].value, U256::from(1000u64));
    }

    #[test]
    fn build_tempo_calls_single_call_fallback() {
        // D2: empty calls + non-empty target → exactly one call from the step.
        let step = single_call_step(DEAD_RPC);
        let calls = build_tempo_calls(&step).expect("build single call");
        assert_eq!(calls.len(), 1, "single Target/Data/Value fallback");
        assert_eq!(
            calls[0].to.to_hex(),
            address::parse("0xdec0000000000000000000000000000000000000")
                .unwrap()
                .to_hex()
        );
        assert_eq!(calls[0].data, vec![0x12, 0x34, 0x56, 0x78]);
        assert_eq!(calls[0].value, U256::ZERO);
    }

    #[test]
    fn build_tempo_calls_empty_value_is_zero() {
        // D3: an empty value string parses to U256::ZERO.
        let mut step = batched_swap_step(DEAD_RPC);
        step.calls[0].value = String::new();
        let calls = build_tempo_calls(&step).expect("build calls");
        assert_eq!(calls[0].value, U256::ZERO);
    }

    #[test]
    fn build_tempo_calls_non_numeric_value_is_usage_error() {
        // D4.
        let mut step = batched_swap_step(DEAD_RPC);
        step.calls[0].value = "abc".to_string();
        let err = build_tempo_calls(&step).unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn build_tempo_calls_invalid_hex_data_is_usage_error() {
        // D5.
        let mut step = batched_swap_step(DEAD_RPC);
        step.calls[0].data = "0xzz".to_string();
        let err = build_tempo_calls(&step).unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn build_tempo_calls_decodes_hex_like_go_decodehex() {
        // D5 parity: optional 0x prefix; odd-length body left-padded with a 0
        // nibble; "0x"/"" → empty calldata (Go decodeHex).
        let mut step = single_call_step(DEAD_RPC);
        step.data = "0x1".to_string(); // odd length → 0x01
        assert_eq!(build_tempo_calls(&step).unwrap()[0].data, vec![0x01]);

        step.data = "abcdef".to_string(); // no 0x prefix
        assert_eq!(
            build_tempo_calls(&step).unwrap()[0].data,
            vec![0xab, 0xcd, 0xef]
        );

        step.data = "0x".to_string();
        assert!(build_tempo_calls(&step).unwrap()[0].data.is_empty());

        step.data = String::new();
        assert!(build_tempo_calls(&step).unwrap()[0].data.is_empty());
    }

    // =====================================================================
    // E. Fee-token resolution
    // =====================================================================

    #[test]
    fn resolve_fee_token_explicit_override_wins() {
        // E1: an explicit valid --fee-token resolves to that address on any chain.
        let token = "0x20c0000000000000000000000000000000000099";
        let got = resolve_tempo_fee_token(token, 1).expect("explicit token");
        assert_eq!(
            got.to_hex(),
            address::parse(token).unwrap().to_hex(),
            "explicit --fee-token must win"
        );
    }

    #[test]
    fn resolve_fee_token_invalid_override_is_usage_error() {
        // E2.
        let err = resolve_tempo_fee_token("not-an-address", 4217).unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn resolve_fee_token_defaults_to_registry_on_tempo_chain() {
        // E3: no override on Tempo mainnet (4217) → registry default.
        let got = resolve_tempo_fee_token("", 4217).expect("registry default");
        let want = defi_registry::tempo_fee_token(4217).expect("registry has 4217 fee token");
        assert_eq!(got.to_hex(), address::parse(want).unwrap().to_hex());
        assert!(!got.is_zero(), "Tempo chain must have a non-zero fee token");
    }

    #[test]
    fn resolve_fee_token_zero_on_non_tempo_chain_without_override() {
        // E4: no override on a non-Tempo chain (no registry default) → zero.
        let got = resolve_tempo_fee_token("", 1).expect("no error for missing default");
        assert!(
            got.is_zero(),
            "non-Tempo chain without --fee-token must resolve to the zero address"
        );
        assert_eq!(defi_registry::tempo_fee_token(1), None);
    }

    // =====================================================================
    // F. estimate_step is not implemented (parity with Go)
    // =====================================================================

    #[test]
    fn estimate_step_is_not_implemented() {
        // F1.
        let exec = TempoStepExecutor::from_signer(TempoSignerSource::Local(test_local_signer()));
        let step = batched_swap_step(DEAD_RPC);
        let err = exec
            .estimate_step(&step, default_estimate_options())
            .unwrap_err();
        assert!(
            err.to_string()
                .to_lowercase()
                .contains("not yet implemented"),
            "expected an unimplemented-estimate error, got: {err}"
        );
    }
}
