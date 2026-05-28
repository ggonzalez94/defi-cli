//! Pre-sign policy checks (bounded approvals, canonical targets).
//!
//! Go source: `internal/execution/policy_basic.go` (+ `policy_basic_test.go`).
//! This is the **pre-sign guardrail** the executor calls *before* a step is
//! signed/broadcast: it re-decodes the step's calldata and asserts the
//! transaction the user is about to sign matches the action they planned, using
//! only the canonical, offline metadata in [`defi_registry`] (no network).
//!
//! ## Scope boundary (no overlap with sibling modules)
//! - **Calldata ABI decode** (selectors, `approve(spender,amount)` /
//!   `transfer(to,amount)` re-read) is performed via [`defi_evm::abi::Function`]
//!   against the registry ABI fragments; this module owns the *policy rules*, not
//!   the ABI engine.
//! - **Canonical address/endpoint allowlists** (Uniswap V3 router, Tempo DEX,
//!   bridge execution targets, bridge settlement URLs) live in [`defi_registry`];
//!   this module *consults* them.
//! - **Post-confirmation** allowance-readiness / head-ordering / settlement
//!   polling is the executor's job ([`crate::evm_executor`]), NOT a pre-sign
//!   policy rule.
//!
//! =============================================================================
//! SUCCESS CRITERIA (RED phase — these tests reference the not-yet-implemented
//! public API below and MUST fail to compile / fail assertions until GREEN).
//!
//! The Rust port of this module is "correct" iff `validate_step_policy` (and the
//! directly-tested `validate_swap_policy`) reproduce the Go pre-sign gate exactly.
//! Every rejection is a typed [`defi_errors::Error`]; the dominant rejection code
//! is [`Code::ActionPlan`] (Go `clierr.CodeActionPlan`), with a missing-step
//! [`Code::Internal`] and invalid-target [`Code::Usage`]. No `unwrap`/`expect`/
//! `panic` in library code.
//!
//! ### Options surface
//! O1. The policy gate only ever reads two flags from the executor's options:
//!     "allow max approval" and "unsafe provider tx". This module exposes a
//!     focused [`PolicyOptions`] (both default `false`) so the gate stays a leaf
//!     and the executor maps its richer `ExecuteOptions` down to it. (Idiomatic
//!     Rust divergence from Go's monolithic `ExecuteOptions`: the policy reads a
//!     strict subset, so it takes a strict subset — observable behavior is
//!     identical.)
//!
//! ### Dispatch + target sanity (`validate_step_policy`)
//! D1. A `None` step is impossible in Rust (the signature takes `&ActionStep`), so
//!     the Go "missing action step" → `CodeInternal` branch is folded into the
//!     type system; we still expose [`Code::Internal`] usage for the executor's
//!     own nil guard and assert the *invalid-target* path instead.
//! D2. A step with **no batched calls** and an **invalid `target`** address →
//!     [`Code::Usage`] ("invalid step target address") — Go
//!     `validateStepPolicy` single-target guard.
//! D3. A step **with** batched `calls` (even if `target` is empty) **skips** the
//!     single-target address check (per-call targets are validated by the
//!     provider-specific handler) — Go comment + `Calls` length guard.
//! D4. An unrecognized [`StepType`] (e.g. `Lend`, `Claim`) is a no-op `Ok(())`
//!     (Go `default:` branch).
//!
//! ### Approval policy (`StepType::Approval`)  — Go `validateApprovalPolicy`
//! A1. Bounded approval passes: `approve(spender, amount)` with
//!     `amount <= action.input_amount` and a non-zero spender → `Ok` (Go
//!     `TestValidateApprovalPolicyBounded`: amount 100, input 100).
//! A2. An approval whose `amount > input_amount` is REJECTED by default, and the
//!     error message contains the override hint `"allow-max-approval"` (Go
//!     `TestValidateApprovalPolicyRejectsUnlimitedByDefault`: 101 > 100).
//! A3. `PolicyOptions { allow_max_approval: true, .. }` bypasses the bound (Go
//!     `TestValidateApprovalPolicyAllowsOverride`: 101 passes).
//! A4. Calldata whose leading selector is not ERC-20 `approve` → `ActionPlan`
//!     ("must use ERC20 approve(spender,amount)").
//! A5. A zero spender or a non-positive amount → `ActionPlan` ("invalid spender"
//!     / "invalid approval amount").
//! A6. With bound-checking enabled but a non-numeric `input_amount`, the error
//!     mentions `"--allow-max-approval to override"` (Go: parse-positive failure).
//! A7. Bound-checking with a `None` action context → `ActionPlan` ("cannot
//!     validate approval bounds without action context").
//!
//! ### Transfer policy (`StepType::Transfer`)  — Go `validateTransferPolicy`
//! T1. A `transfer(to, amount)` whose recipient == `action.to_address`,
//!     amount == `action.input_amount`, and whose step `target` ==
//!     `action.metadata["asset_address"]` → `Ok` (Go
//!     `TestValidateTransferPolicyMatchesAction`).
//! T2. Calldata not starting with the ERC-20 `transfer` selector → `ActionPlan`
//!     ("must use ERC20 transfer(to,amount)").
//! T3. Recipient ≠ `to_address` → `ActionPlan` mentioning `"to_address"` (Go
//!     `TestValidateTransferPolicyRejectsRecipientMismatch`).
//! T4. Amount ≠ `input_amount` → `ActionPlan` mentioning `"does not match"` (Go
//!     `TestValidateTransferPolicyRejectsAmountMismatch`).
//! T5. Missing `asset_address` metadata → `ActionPlan` mentioning
//!     `"asset_address"` (Go `TestValidateTransferPolicyRequiresAssetAddressMetadata`).
//!
//! ### Swap policy (`StepType::Swap`)  — Go `validateSwapPolicy`
//! S1. `provider == "taikoswap"`: calldata must start with the Uniswap V3
//!     `exactInputSingle` selector AND the step `target` must equal the canonical
//!     router for the chain; a mismatched target on a supported chain (167000) →
//!     `ActionPlan` (Go `TestValidateSwapPolicyTaikoRouter`).
//! S2. `provider == "tempo"` (legacy single-target): calldata must start with
//!     `swapExactAmountIn`/`swapExactAmountOut` AND target == canonical Tempo DEX;
//!     a mismatched target on Tempo chain (4217) → `ActionPlan` (Go
//!     `TestValidateSwapPolicyTempoDEX`).
//! S3. A `None` action context is a no-op `Ok` (Go: `if action == nil { return nil }`).
//!
//! ### Batched Tempo swap calls — Go `validateTempoSwapCalls`
//! B1. A valid `[approve(dex, n), swapExactAmountIn(...)]` batch on chain 4217
//!     with `metadata["token_in"]` set passes (Go
//!     `TestValidateTempoSwapBatchedCallsPass`).
//! B2. A swap call whose `target` ≠ canonical DEX → `ActionPlan` mentioning
//!     `"canonical stablecoin dex"` (Go `...RejectsWrongDEX`).
//! B3. An unrecognized selector among the calls → `ActionPlan` mentioning
//!     `"unrecognized selector"` (Go `...RejectsUnknownSelector`).
//! B4. A batch with NO swap call (approve only) → `ActionPlan` mentioning
//!     `"at least one swap call"` (Go `...RejectsApproveOnly`).
//! B5. An approve call whose `target` ≠ `action.metadata["token_in"]` →
//!     `ActionPlan` mentioning `"input token"` (Go `...RejectsApproveOnWrongToken`).
//! B6. More than one approve call → `ActionPlan` mentioning
//!     `"more than one approve"` (Go `...RejectsExtraApproval`).
//! B7. An approve call carrying non-zero `value` → `ActionPlan` mentioning
//!     `"zero value"` (Go `...RejectsApproveWithValue`).
//! B8. Missing `token_in` metadata when an approve call is present → `ActionPlan`
//!     mentioning `"token_in metadata"` (Go `...RejectsMissingTokenInMetadata`).
//! B9. (Boundary) An approve spender ≠ canonical DEX → `ActionPlan` mentioning
//!     `"canonical stablecoin dex"`. Fresh spec-driven (Go covers spender via the
//!     `expectedDEX` compare; we assert it explicitly).
//!
//! ### Bridge policy (`StepType::Bridge`) — Go `validateBridgePolicy`
//! G1. `unsafe_provider_tx: true` bypasses ALL bridge checks → `Ok` (Go: first
//!     branch).
//! G2. An untrusted settlement-status endpoint (host not in the provider
//!     allowlist) is REJECTED by default and `unsafe_provider_tx` overrides it
//!     (Go `TestValidateBridgePolicyEndpointGuard`).
//! G3. A canonical settlement endpoint but a non-canonical execution `target` on
//!     a covered provider/chain → `ActionPlan` mentioning `"execution contract"`;
//!     `unsafe_provider_tx` overrides it (Go `TestValidateBridgePolicyTargetGuard`).
//! G4. A canonical Across target on Base (8453) passes (Go
//!     `TestValidateBridgePolicyAllowsCanonicalTarget`).
//! G5. A canonical LiFi target on Ethereum (1) passes (Go
//!     `TestValidateBridgePolicyAllowsCanonicalLiFiTarget`).
//! G6. On a chain with NO target policy for the provider (Across on 43114), the
//!     target check is skipped and any target passes (Go
//!     `...SkipsTargetCheckOnUncoveredChain`).
//! G7. An unknown settlement provider (neither lifi nor across) → `ActionPlan`
//!     mentioning `"settlement provider"`. Fresh spec-driven from the Go branch.
//! =============================================================================

use alloy::primitives::U256;
use defi_errors::{Code, Error};
use defi_evm::abi::{function_selector, Function};
use defi_evm::address;
use defi_registry::{
    has_bridge_execution_target_policy, is_allowed_bridge_execution_target,
    is_allowed_bridge_settlement_url, tempo_stablecoin_dex, uniswap_v3_contracts,
    ERC20_MINIMAL_ABI,
};

use crate::action::{Action, ActionStep, StepCall, StepType};

/// The focused subset of executor options the pre-sign policy gate reads.
/// Parity with the two `ExecuteOptions` fields the Go policy consults.
#[derive(Debug, Clone, Default)]
pub struct PolicyOptions {
    /// Opt into approvals larger than the planned input amount.
    pub allow_max_approval: bool,
    /// Bypass bridge provider-tx guardrails.
    pub unsafe_provider_tx: bool,
}

/// The 4-byte ERC-20 `approve` selector.
fn approve_selector() -> [u8; 4] {
    function_selector("approve(address,uint256)")
}

/// The 4-byte ERC-20 `transfer` selector.
fn transfer_selector() -> [u8; 4] {
    function_selector("transfer(address,uint256)")
}

/// The 4-byte Uniswap V3 `exactInputSingle` selector.
fn uniswap_v3_swap_selector() -> [u8; 4] {
    Function::from_abi_json(defi_registry::UNISWAP_V3_ROUTER_ABI, "exactInputSingle")
        .map(|f| f.selector())
        .unwrap_or([0u8; 4])
}

/// The 4-byte Tempo DEX `swapExactAmountIn` selector.
fn tempo_swap_exact_in_selector() -> [u8; 4] {
    Function::from_abi_json(defi_registry::TEMPO_STABLECOIN_DEX_ABI, "swapExactAmountIn")
        .map(|f| f.selector())
        .unwrap_or([0u8; 4])
}

/// The 4-byte Tempo DEX `swapExactAmountOut` selector.
fn tempo_swap_exact_out_selector() -> [u8; 4] {
    Function::from_abi_json(
        defi_registry::TEMPO_STABLECOIN_DEX_ABI,
        "swapExactAmountOut",
    )
    .map(|f| f.selector())
    .unwrap_or([0u8; 4])
}

/// Validate a step against the pre-sign policy gate, parity with Go
/// `validateStepPolicy`.
///
/// A step with no batched calls and an invalid single `target` is [`Code::Usage`];
/// otherwise dispatch by [`StepType`]. Approval/transfer/swap/bridge are policed;
/// other step types are a no-op `Ok`.
pub fn validate_step_policy(
    action: Option<&Action>,
    step: &ActionStep,
    chain_id: i64,
    data: &[u8],
    opts: &PolicyOptions,
) -> Result<(), Error> {
    if step.calls.is_empty() && !address::is_hex_address(step.target.trim()) {
        return Err(Error::new(Code::Usage, "invalid step target address"));
    }
    match step.step_type {
        StepType::Approval => validate_approval_policy(action, data, opts),
        StepType::Transfer => validate_transfer_policy(action, step, data),
        StepType::Swap => validate_swap_policy(action, step, chain_id, data, opts),
        StepType::Bridge => validate_bridge_policy(action, step, chain_id, opts),
        _ => Ok(()),
    }
}

fn validate_approval_policy(
    action: Option<&Action>,
    data: &[u8],
    opts: &PolicyOptions,
) -> Result<(), Error> {
    if data.len() < 4 || data[..4] != approve_selector() {
        return Err(Error::new(
            Code::ActionPlan,
            "approval step must use ERC20 approve(spender,amount)",
        ));
    }
    let (spender, amount) = decode_address_amount(data)
        .ok_or_else(|| Error::new(Code::ActionPlan, "approval step calldata is invalid"))?;
    if spender.is_zero() {
        return Err(Error::new(
            Code::ActionPlan,
            "approval step has invalid spender",
        ));
    }
    if amount.is_zero() {
        return Err(Error::new(
            Code::ActionPlan,
            "approval step has invalid approval amount",
        ));
    }
    if opts.allow_max_approval {
        return Ok(());
    }
    let action = action.ok_or_else(|| {
        Error::new(
            Code::ActionPlan,
            "cannot validate approval bounds without action context",
        )
    })?;
    let requested = parse_positive_base_units(&action.input_amount).ok_or_else(|| {
        Error::new(
            Code::ActionPlan,
            "cannot validate approval bounds for non-numeric input amount; use --allow-max-approval to override",
        )
    })?;
    if amount > requested {
        return Err(Error::new(
            Code::ActionPlan,
            format!(
                "approval amount {amount} exceeds requested input amount {requested}; use --allow-max-approval to override"
            ),
        ));
    }
    Ok(())
}

fn validate_transfer_policy(
    action: Option<&Action>,
    step: &ActionStep,
    data: &[u8],
) -> Result<(), Error> {
    if data.len() < 4 || data[..4] != transfer_selector() {
        return Err(Error::new(
            Code::ActionPlan,
            "transfer step must use ERC20 transfer(to,amount)",
        ));
    }
    let (recipient, amount) = decode_address_amount(data)
        .ok_or_else(|| Error::new(Code::ActionPlan, "transfer step calldata is invalid"))?;
    if recipient.is_zero() {
        return Err(Error::new(
            Code::ActionPlan,
            "transfer step has invalid recipient",
        ));
    }
    if amount.is_zero() {
        return Err(Error::new(
            Code::ActionPlan,
            "transfer step has invalid transfer amount",
        ));
    }
    let Some(action) = action else {
        return Ok(());
    };
    let requested = parse_positive_base_units(&action.input_amount).ok_or_else(|| {
        Error::new(
            Code::ActionPlan,
            "cannot validate transfer amount for non-numeric input amount",
        )
    })?;
    if amount != requested {
        return Err(Error::new(
            Code::ActionPlan,
            format!("transfer amount {amount} does not match requested input amount {requested}"),
        ));
    }
    if !action.to_address.trim().is_empty()
        && !address::eq_fold(action.to_address.trim(), &recipient.to_hex())
    {
        return Err(Error::new(
            Code::ActionPlan,
            "transfer recipient does not match action to_address",
        ));
    }
    if !step.target.trim().is_empty() && !address::is_hex_address(step.target.trim()) {
        return Err(Error::new(
            Code::ActionPlan,
            "transfer step has invalid token target",
        ));
    }
    let asset_address = metadata_string(action, "asset_address");
    if asset_address.is_empty() {
        return Err(Error::new(
            Code::ActionPlan,
            "transfer action missing asset_address metadata",
        ));
    }
    if !address::is_hex_address(&asset_address) {
        return Err(Error::new(
            Code::ActionPlan,
            "transfer action metadata has invalid asset_address",
        ));
    }
    if !address::eq_fold(step.target.trim(), &asset_address) {
        return Err(Error::new(
            Code::ActionPlan,
            "transfer step target does not match action asset_address",
        ));
    }
    Ok(())
}

/// Validate a swap step against the pre-sign policy gate, parity with Go
/// `validateSwapPolicy`. A `None` action is a no-op `Ok`.
pub fn validate_swap_policy(
    action: Option<&Action>,
    step: &ActionStep,
    chain_id: i64,
    data: &[u8],
    opts: &PolicyOptions,
) -> Result<(), Error> {
    let Some(action) = action else {
        return Ok(());
    };
    match action.provider.trim().to_lowercase().as_str() {
        "taikoswap" => {
            if data.len() < 4 || data[..4] != uniswap_v3_swap_selector() {
                return Err(Error::new(
                    Code::ActionPlan,
                    "taikoswap swap step must call exactInputSingle",
                ));
            }
            let router = uniswap_v3_contracts(chain_id)
                .map(|(_, r)| r)
                .ok_or_else(|| {
                    Error::new(
                        Code::ActionPlan,
                        "taikoswap swap step has unsupported chain",
                    )
                })?;
            if !address::eq_fold(step.target.trim(), router) {
                return Err(Error::new(
                    Code::ActionPlan,
                    "taikoswap swap step target does not match canonical router",
                ));
            }
            Ok(())
        }
        "tempo" => {
            if !step.calls.is_empty() {
                return validate_tempo_swap_calls(chain_id, &step.calls, Some(action), opts);
            }
            if data.len() < 4
                || (data[..4] != tempo_swap_exact_in_selector()
                    && data[..4] != tempo_swap_exact_out_selector())
            {
                return Err(Error::new(
                    Code::ActionPlan,
                    "tempo swap step must call swapExactAmountIn or swapExactAmountOut",
                ));
            }
            let dex = tempo_stablecoin_dex(chain_id).ok_or_else(|| {
                Error::new(Code::ActionPlan, "tempo swap step has unsupported chain")
            })?;
            if !address::eq_fold(step.target.trim(), dex) {
                return Err(Error::new(
                    Code::ActionPlan,
                    "tempo swap step target does not match canonical stablecoin dex",
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_tempo_swap_calls(
    chain_id: i64,
    calls: &[StepCall],
    action: Option<&Action>,
    opts: &PolicyOptions,
) -> Result<(), Error> {
    let dex = tempo_stablecoin_dex(chain_id)
        .ok_or_else(|| Error::new(Code::ActionPlan, "tempo swap step has unsupported chain"))?;

    let mut has_swap_call = false;
    let mut approve_count = 0usize;
    for (i, call) in calls.iter().enumerate() {
        let data = decode_hex(&call.data).map_err(|e| {
            Error::wrap(
                Code::ActionPlan,
                format!("tempo swap call {i} has invalid data"),
                e,
            )
        })?;
        if data.len() < 4 {
            return Err(Error::new(
                Code::ActionPlan,
                format!("tempo swap call {i} has insufficient calldata"),
            ));
        }
        let selector = &data[..4];
        if selector == approve_selector() {
            approve_count += 1;
            if approve_count > 1 {
                return Err(Error::new(
                    Code::ActionPlan,
                    "tempo swap step contains more than one approve call",
                ));
            }
            let value = call.value.trim();
            if !value.is_empty() && value != "0" {
                return Err(Error::new(
                    Code::ActionPlan,
                    format!("tempo swap call {i} approve must have zero value"),
                ));
            }
            if let Some(action) = action {
                let expected_token = metadata_string(action, "token_in");
                if expected_token.is_empty() {
                    return Err(Error::new(
                        Code::ActionPlan,
                        format!(
                            "tempo swap call {i} cannot validate approve target: action missing token_in metadata"
                        ),
                    ));
                }
                if !address::eq_fold(call.target.trim(), &expected_token) {
                    return Err(Error::new(
                        Code::ActionPlan,
                        format!(
                            "tempo swap call {i} approve target does not match action input token"
                        ),
                    ));
                }
            }
            let (spender, amount) = decode_address_amount(&data).ok_or_else(|| {
                Error::new(
                    Code::ActionPlan,
                    format!("tempo swap call {i} has invalid approve calldata"),
                )
            })?;
            if spender.is_zero() {
                return Err(Error::new(
                    Code::ActionPlan,
                    format!("tempo swap call {i} has invalid approve spender"),
                ));
            }
            if !address::eq_fold(&spender.to_hex(), dex) {
                return Err(Error::new(
                    Code::ActionPlan,
                    format!(
                        "tempo swap call {i} approve spender does not match canonical stablecoin dex"
                    ),
                ));
            }
            if !opts.allow_max_approval {
                if amount.is_zero() {
                    return Err(Error::new(
                        Code::ActionPlan,
                        format!("tempo swap call {i} has invalid approve amount"),
                    ));
                }
                if let Some(action) = action {
                    let requested = parse_positive_base_units(&action.input_amount).ok_or_else(|| {
                        Error::new(
                            Code::ActionPlan,
                            "cannot validate approval bounds for non-numeric input amount; use --allow-max-approval to override",
                        )
                    })?;
                    if amount > requested {
                        return Err(Error::new(
                            Code::ActionPlan,
                            format!(
                                "tempo swap call {i} approval amount {amount} exceeds requested input amount {requested}; use --allow-max-approval to override"
                            ),
                        ));
                    }
                }
            }
        } else if selector == tempo_swap_exact_in_selector()
            || selector == tempo_swap_exact_out_selector()
        {
            if !address::eq_fold(call.target.trim(), dex) {
                return Err(Error::new(
                    Code::ActionPlan,
                    "tempo swap call target does not match canonical stablecoin dex",
                ));
            }
            has_swap_call = true;
        } else {
            return Err(Error::new(
                Code::ActionPlan,
                format!(
                    "tempo swap call {i} has unrecognized selector 0x{}",
                    hex::encode(selector)
                ),
            ));
        }
    }
    if !has_swap_call {
        return Err(Error::new(
            Code::ActionPlan,
            "tempo swap step must contain at least one swap call (swapExactAmountIn or swapExactAmountOut)",
        ));
    }
    Ok(())
}

fn validate_bridge_policy(
    action: Option<&Action>,
    step: &ActionStep,
    chain_id: i64,
    opts: &PolicyOptions,
) -> Result<(), Error> {
    if opts.unsafe_provider_tx {
        return Ok(());
    }
    let mut provider = step
        .expected_outputs
        .as_ref()
        .and_then(|o| o.get("settlement_provider"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if provider.is_empty() {
        if let Some(action) = action {
            provider = action.provider.trim().to_lowercase();
        }
    }
    if provider != "lifi" && provider != "across" {
        return Err(Error::new(
            Code::ActionPlan,
            "bridge step has unknown settlement provider; use --unsafe-provider-tx to override",
        ));
    }
    if let Some(action) = action {
        let action_provider = action.provider.trim();
        if !action_provider.is_empty() && !action_provider.eq_ignore_ascii_case(&provider) {
            return Err(Error::new(
                Code::ActionPlan,
                "bridge step provider does not match action provider",
            ));
        }
    }
    let status_endpoint = step
        .expected_outputs
        .as_ref()
        .and_then(|o| o.get("settlement_status_endpoint"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !is_allowed_bridge_settlement_url(&provider, status_endpoint) {
        return Err(Error::new(
            Code::ActionPlan,
            "bridge step settlement endpoint is not allowed; use --unsafe-provider-tx to override",
        ));
    }
    if has_bridge_execution_target_policy(&provider, chain_id)
        && !is_allowed_bridge_execution_target(&provider, chain_id, step.target.trim())
    {
        return Err(Error::new(
            Code::ActionPlan,
            "bridge step target is not an allowed provider execution contract; use --unsafe-provider-tx to override",
        ));
    }
    Ok(())
}

/// Decode `(address, uint256)` from ABI-encoded calldata (selector ++ args).
fn decode_address_amount(data: &[u8]) -> Option<(address::Address, U256)> {
    if data.len() < 4 {
        return None;
    }
    let func = Function::from_abi_json(ERC20_MINIMAL_ABI, "approve").ok()?;
    let args = func.decode_input(&data[4..]).ok()?;
    if args.len() != 2 {
        return None;
    }
    let addr = address::Address::from(args[0].as_address()?);
    let (amount, _) = args[1].as_uint()?;
    Some((addr, amount))
}

/// Parse a positive base-units integer string; `None` for empty, non-numeric, or
/// non-positive. Parity with Go `parsePositiveBaseUnits`.
fn parse_positive_base_units(value: &str) -> Option<U256> {
    let v = value.trim();
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let parsed = U256::from_str_radix(v, 10).ok()?;
    if parsed.is_zero() {
        return None;
    }
    Some(parsed)
}

/// Read a string value from an action's `metadata` map for `key`.
fn metadata_string(action: &Action, key: &str) -> String {
    action
        .metadata
        .as_ref()
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
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
    hex::decode(body)
        .map_err(|e| Error::wrap(Code::ActionPlan, "invalid hex", HexCause(e.to_string())))
}

/// A concrete cause carrying an error's display text.
#[derive(Debug)]
struct HexCause(String);

impl std::fmt::Display for HexCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HexCause {}

#[cfg(test)]
mod tests {
    // RED phase. These reference the not-yet-implemented public API of this
    // module (`PolicyOptions`, `validate_step_policy`, `validate_swap_policy`) and
    // MUST fail to compile / fail assertions until the GREEN implementation lands.
    //
    // All vectors are deterministic and offline. Calldata is built with the real
    // ABI engine (`defi_evm::abi::Function`) against the registry ABI fragments,
    // so the selectors/encodings the policy decodes are exactly what the Go
    // `policyERC20ABI.Pack(...)` / `policyTempoDEXABI.Pack(...)` produced.

    use super::*;
    use crate::action::{Action, ActionStep, Constraints, StepCall, StepStatus, StepType};
    use defi_evm::abi::Function;
    use defi_registry::{ERC20_MINIMAL_ABI, TEMPO_STABLECOIN_DEX_ABI, UNISWAP_V3_ROUTER_ABI};

    // ---- canonical test addresses (mirror policy_basic_test.go) ----
    const SPENDER: &str = "0x00000000000000000000000000000000000000ab";
    const STEP_TARGET: &str = "0x00000000000000000000000000000000000000cd";
    const RECIPIENT: &str = "0x00000000000000000000000000000000000000ab";
    const TEMPO_DEX: &str = "0xdec0000000000000000000000000000000000000";
    const TEMPO_TOKEN_IN: &str = "0x20c0000000000000000000000000000000000000";
    const TEMPO_TOKEN_OUT: &str = "0x20c000000000000000000000b9537d11c60e8b50";

    // ---- ABI helpers (the policy's own decode path is exercised indirectly) ----

    fn av_addr(s: &str) -> alloy::dyn_abi::DynSolValue {
        alloy::dyn_abi::DynSolValue::Address(s.parse().expect("valid test address"))
    }
    fn av_u256(n: u128) -> alloy::dyn_abi::DynSolValue {
        alloy::dyn_abi::DynSolValue::Uint(alloy::primitives::U256::from(n), 256)
    }
    fn av_u128(n: u128) -> alloy::dyn_abi::DynSolValue {
        alloy::dyn_abi::DynSolValue::Uint(alloy::primitives::U256::from(n), 128)
    }

    fn encode(abi_json: &str, name: &str, args: &[alloy::dyn_abi::DynSolValue]) -> Vec<u8> {
        Function::from_abi_json(abi_json, name)
            .expect("fragment parses")
            .encode(args)
            .expect("encode succeeds")
    }
    fn hex0x(bytes: &[u8]) -> String {
        format!("0x{}", hex::encode(bytes))
    }

    fn approve_calldata(spender: &str, amount: u128) -> Vec<u8> {
        encode(
            ERC20_MINIMAL_ABI,
            "approve",
            &[av_addr(spender), av_u256(amount)],
        )
    }
    fn transfer_calldata(to: &str, amount: u128) -> Vec<u8> {
        encode(
            ERC20_MINIMAL_ABI,
            "transfer",
            &[av_addr(to), av_u256(amount)],
        )
    }
    fn uniswap_exact_input_selector() -> Vec<u8> {
        Function::from_abi_json(UNISWAP_V3_ROUTER_ABI, "exactInputSingle")
            .expect("fragment parses")
            .selector()
            .to_vec()
    }
    fn tempo_swap_exact_in_selector() -> Vec<u8> {
        Function::from_abi_json(TEMPO_STABLECOIN_DEX_ABI, "swapExactAmountIn")
            .expect("fragment parses")
            .selector()
            .to_vec()
    }
    fn tempo_swap_exact_in_calldata() -> Vec<u8> {
        encode(
            TEMPO_STABLECOIN_DEX_ABI,
            "swapExactAmountIn",
            &[
                av_addr(TEMPO_TOKEN_IN),
                av_addr(TEMPO_TOKEN_OUT),
                av_u128(1000),
                av_u128(990),
            ],
        )
    }

    // ---- step/action builders ----

    fn step(step_type: StepType, target: &str) -> ActionStep {
        ActionStep {
            step_id: "step-1".into(),
            step_type,
            status: StepStatus::Pending,
            chain_id: String::new(),
            rpc_url: String::new(),
            description: String::new(),
            target: target.into(),
            data: String::new(),
            value: String::new(),
            calls: Vec::new(),
            expected_outputs: None,
            tx_hash: String::new(),
            error: String::new(),
        }
    }

    fn action(input_amount: &str) -> Action {
        let mut a = Action::new("act_x", "test", "eip155:1", Constraints::default());
        a.input_amount = input_amount.into();
        a
    }

    fn call(target: &str, data: &[u8], value: &str) -> StepCall {
        StepCall {
            target: target.into(),
            data: hex0x(data),
            value: value.into(),
        }
    }

    fn outputs(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        for (k, v) in pairs {
            m.insert((*k).into(), serde_json::Value::String((*v).into()));
        }
        m
    }

    // =====================================================================
    // O1: PolicyOptions surface
    // =====================================================================

    #[test]
    fn policy_options_default_is_all_false() {
        let o = PolicyOptions::default();
        assert!(!o.allow_max_approval);
        assert!(!o.unsafe_provider_tx);
    }

    // =====================================================================
    // D: dispatch + target sanity
    // =====================================================================

    #[test]
    fn rejects_invalid_target_when_no_calls() {
        // D2: a non-hex target with no batched calls is a usage error.
        let s = step(StepType::Approval, "not-an-address");
        let a = action("100");
        let err = validate_step_policy(
            Some(&a),
            &s,
            1,
            &approve_calldata(SPENDER, 100),
            &PolicyOptions::default(),
        )
        .expect_err("invalid target must fail");
        assert_eq!(err.code, Code::Usage);
        assert!(
            err.to_string().contains("invalid step target address"),
            "got: {err}"
        );
    }

    #[test]
    fn skips_single_target_check_when_calls_present() {
        // D3: with batched calls, an empty/invalid single `target` is allowed;
        // per-call targets are validated by the swap handler instead.
        let mut s = step(StepType::Swap, "");
        s.calls = vec![
            call(TEMPO_TOKEN_IN, &approve_calldata(TEMPO_DEX, 1000), "0"),
            call(TEMPO_DEX, &tempo_swap_exact_in_calldata(), "0"),
        ];
        let mut a = action("1000");
        a.provider = "tempo".into();
        a.metadata = Some(outputs(&[("token_in", TEMPO_TOKEN_IN)]));
        validate_step_policy(Some(&a), &s, 4217, &[], &PolicyOptions::default())
            .expect("batched calls skip single-target check and pass");
    }

    #[test]
    fn unrecognized_step_type_is_noop() {
        // D4: Lend/Claim steps are not policed here.
        let s = step(StepType::Lend, STEP_TARGET);
        let a = action("100");
        validate_step_policy(Some(&a), &s, 1, &[0x01], &PolicyOptions::default())
            .expect("non-policed step type is a no-op Ok");
    }

    // =====================================================================
    // A: approval policy
    // =====================================================================

    #[test]
    fn approval_bounded_passes() {
        // A1: Go TestValidateApprovalPolicyBounded.
        let s = step(StepType::Approval, STEP_TARGET);
        let a = action("100");
        validate_step_policy(
            Some(&a),
            &s,
            1,
            &approve_calldata(SPENDER, 100),
            &PolicyOptions::default(),
        )
        .expect("bounded approval (100 <= 100) passes");
    }

    #[test]
    fn approval_rejects_unlimited_by_default() {
        // A2: Go TestValidateApprovalPolicyRejectsUnlimitedByDefault.
        let s = step(StepType::Approval, STEP_TARGET);
        let a = action("100");
        let err = validate_step_policy(
            Some(&a),
            &s,
            1,
            &approve_calldata(SPENDER, 101),
            &PolicyOptions::default(),
        )
        .expect_err("101 > 100 must be rejected");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(
            err.to_string().contains("allow-max-approval"),
            "expected override hint, got: {err}"
        );
    }

    #[test]
    fn approval_allows_override() {
        // A3: Go TestValidateApprovalPolicyAllowsOverride.
        let s = step(StepType::Approval, STEP_TARGET);
        let a = action("100");
        validate_step_policy(
            Some(&a),
            &s,
            1,
            &approve_calldata(SPENDER, 101),
            &PolicyOptions {
                allow_max_approval: true,
                unsafe_provider_tx: false,
            },
        )
        .expect("allow_max_approval bypasses the bound");
    }

    #[test]
    fn approval_rejects_non_approve_selector() {
        // A4: calldata that is not approve(...) is rejected.
        let s = step(StepType::Approval, STEP_TARGET);
        let a = action("100");
        let err = validate_step_policy(
            Some(&a),
            &s,
            1,
            &transfer_calldata(SPENDER, 100), // transfer, not approve
            &PolicyOptions::default(),
        )
        .expect_err("non-approve selector must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(err.to_string().contains("ERC20 approve"), "got: {err}");
    }

    #[test]
    fn approval_rejects_zero_spender() {
        // A5: spender == zero address.
        let s = step(StepType::Approval, STEP_TARGET);
        let a = action("100");
        let err = validate_step_policy(
            Some(&a),
            &s,
            1,
            &approve_calldata("0x0000000000000000000000000000000000000000", 100),
            &PolicyOptions::default(),
        )
        .expect_err("zero spender must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(err.to_string().contains("spender"), "got: {err}");
    }

    #[test]
    fn approval_rejects_zero_amount() {
        // A5: amount <= 0.
        let s = step(StepType::Approval, STEP_TARGET);
        let a = action("100");
        let err = validate_step_policy(
            Some(&a),
            &s,
            1,
            &approve_calldata(SPENDER, 0),
            &PolicyOptions::default(),
        )
        .expect_err("zero amount must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(err.to_string().contains("approval amount"), "got: {err}");
    }

    #[test]
    fn approval_non_numeric_input_amount_requires_override() {
        // A6: input_amount is not base-units numeric → instructs to override.
        let s = step(StepType::Approval, STEP_TARGET);
        let a = action("1.5"); // decimal, not base units → not a positive integer
        let err = validate_step_policy(
            Some(&a),
            &s,
            1,
            &approve_calldata(SPENDER, 100),
            &PolicyOptions::default(),
        )
        .expect_err("non-numeric input amount cannot be bound-checked");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(
            err.to_string().contains("--allow-max-approval to override"),
            "got: {err}"
        );
    }

    #[test]
    fn approval_without_action_context_is_rejected() {
        // A7: bound-checking without an action cannot validate the bound.
        let s = step(StepType::Approval, STEP_TARGET);
        let err = validate_step_policy(
            None,
            &s,
            1,
            &approve_calldata(SPENDER, 100),
            &PolicyOptions::default(),
        )
        .expect_err("missing action context must fail when bound-checking");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(
            err.to_string().contains("without action context"),
            "got: {err}"
        );
    }

    // =====================================================================
    // T: transfer policy
    // =====================================================================

    #[test]
    fn transfer_matches_action() {
        // T1: Go TestValidateTransferPolicyMatchesAction.
        let s = step(StepType::Transfer, STEP_TARGET);
        let mut a = action("100");
        a.to_address = RECIPIENT.into();
        a.metadata = Some(outputs(&[("asset_address", STEP_TARGET)]));
        validate_step_policy(
            Some(&a),
            &s,
            1,
            &transfer_calldata(RECIPIENT, 100),
            &PolicyOptions::default(),
        )
        .expect("matching transfer passes");
    }

    #[test]
    fn transfer_rejects_non_transfer_selector() {
        // T2: calldata not starting with transfer selector.
        let s = step(StepType::Transfer, STEP_TARGET);
        let mut a = action("100");
        a.to_address = RECIPIENT.into();
        a.metadata = Some(outputs(&[("asset_address", STEP_TARGET)]));
        let err = validate_step_policy(
            Some(&a),
            &s,
            1,
            &approve_calldata(RECIPIENT, 100), // approve, not transfer
            &PolicyOptions::default(),
        )
        .expect_err("non-transfer selector must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(err.to_string().contains("ERC20 transfer"), "got: {err}");
    }

    #[test]
    fn transfer_rejects_recipient_mismatch() {
        // T3: Go TestValidateTransferPolicyRejectsRecipientMismatch.
        let s = step(StepType::Transfer, STEP_TARGET);
        let mut a = action("100");
        a.to_address = "0x00000000000000000000000000000000000000ff".into();
        let err = validate_step_policy(
            Some(&a),
            &s,
            1,
            &transfer_calldata(RECIPIENT, 100),
            &PolicyOptions::default(),
        )
        .expect_err("recipient mismatch must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(err.to_string().contains("to_address"), "got: {err}");
    }

    #[test]
    fn transfer_rejects_amount_mismatch() {
        // T4: Go TestValidateTransferPolicyRejectsAmountMismatch.
        let s = step(StepType::Transfer, STEP_TARGET);
        let mut a = action("100");
        a.to_address = RECIPIENT.into();
        let err = validate_step_policy(
            Some(&a),
            &s,
            1,
            &transfer_calldata(RECIPIENT, 101),
            &PolicyOptions::default(),
        )
        .expect_err("amount mismatch must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(err.to_string().contains("does not match"), "got: {err}");
    }

    #[test]
    fn transfer_requires_asset_address_metadata() {
        // T5: Go TestValidateTransferPolicyRequiresAssetAddressMetadata.
        let s = step(StepType::Transfer, STEP_TARGET);
        let mut a = action("100");
        a.to_address = RECIPIENT.into();
        // no asset_address metadata
        let err = validate_step_policy(
            Some(&a),
            &s,
            1,
            &transfer_calldata(RECIPIENT, 100),
            &PolicyOptions::default(),
        )
        .expect_err("missing asset_address metadata must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(err.to_string().contains("asset_address"), "got: {err}");
    }

    // =====================================================================
    // S: swap policy (single-target dispatch)
    // =====================================================================

    #[test]
    fn swap_taikoswap_router_mismatch_fails() {
        // S1: Go TestValidateSwapPolicyTaikoRouter — correct selector but the
        // step target is not the canonical router for chain 167000.
        let s = step(StepType::Swap, STEP_TARGET);
        let mut a = action("100");
        a.provider = "taikoswap".into();
        let err = validate_step_policy(
            Some(&a),
            &s,
            167000,
            &uniswap_exact_input_selector(),
            &PolicyOptions::default(),
        )
        .expect_err("taikoswap router mismatch must fail");
        assert_eq!(err.code, Code::ActionPlan);
    }

    #[test]
    fn swap_tempo_dex_mismatch_fails() {
        // S2: Go TestValidateSwapPolicyTempoDEX — correct selector but the step
        // target is not the canonical Tempo DEX for chain 4217.
        let s = step(StepType::Swap, STEP_TARGET);
        let mut a = action("100");
        a.provider = "tempo".into();
        let err = validate_step_policy(
            Some(&a),
            &s,
            4217,
            &tempo_swap_exact_in_selector(),
            &PolicyOptions::default(),
        )
        .expect_err("tempo dex mismatch must fail");
        assert_eq!(err.code, Code::ActionPlan);
    }

    #[test]
    fn swap_without_action_is_noop() {
        // S3: validate_swap_policy with no action returns Ok.
        let s = step(StepType::Swap, STEP_TARGET);
        validate_swap_policy(None, &s, 4217, &[], &PolicyOptions::default())
            .expect("nil action makes swap policy a no-op");
    }

    // =====================================================================
    // B: batched Tempo swap calls (validate_swap_policy, exercised directly)
    // =====================================================================

    fn tempo_action() -> Action {
        let mut a = action("1000");
        a.provider = "tempo".into();
        a.metadata = Some(outputs(&[("token_in", TEMPO_TOKEN_IN)]));
        a
    }

    fn tempo_batched_step(calls: Vec<StepCall>) -> ActionStep {
        let mut s = step(StepType::Swap, "");
        s.calls = calls;
        s
    }

    #[test]
    fn tempo_batched_calls_pass() {
        // B1: Go TestValidateTempoSwapBatchedCallsPass.
        let s = tempo_batched_step(vec![
            call(TEMPO_TOKEN_IN, &approve_calldata(TEMPO_DEX, 1000), "0"),
            call(TEMPO_DEX, &tempo_swap_exact_in_calldata(), "0"),
        ]);
        validate_swap_policy(
            Some(&tempo_action()),
            &s,
            4217,
            &[],
            &PolicyOptions::default(),
        )
        .expect("valid batched tempo swap passes");
    }

    #[test]
    fn tempo_batched_calls_reject_wrong_dex() {
        // B2: Go TestValidateTempoSwapBatchedCallsRejectsWrongDEX.
        let wrong_dex = "0x00000000000000000000000000000000000000ff";
        let s = tempo_batched_step(vec![call(wrong_dex, &tempo_swap_exact_in_calldata(), "0")]);
        let mut a = action("1000");
        a.provider = "tempo".into();
        let err = validate_swap_policy(Some(&a), &s, 4217, &[], &PolicyOptions::default())
            .expect_err("swap call to wrong dex must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(
            err.to_string().contains("canonical stablecoin dex"),
            "got: {err}"
        );
    }

    #[test]
    fn tempo_batched_calls_reject_unknown_selector() {
        // B3: Go TestValidateTempoSwapBatchedCallsRejectsUnknownSelector.
        let s = tempo_batched_step(vec![call(TEMPO_DEX, &[0xde, 0xad, 0xbe, 0xef], "0")]);
        let mut a = action("1000");
        a.provider = "tempo".into();
        let err = validate_swap_policy(Some(&a), &s, 4217, &[], &PolicyOptions::default())
            .expect_err("unknown selector must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(
            err.to_string().contains("unrecognized selector"),
            "got: {err}"
        );
    }

    #[test]
    fn tempo_batched_calls_reject_approve_only() {
        // B4: Go TestValidateTempoSwapBatchedCallsRejectsApproveOnly.
        let s = tempo_batched_step(vec![call(
            TEMPO_TOKEN_IN,
            &approve_calldata(TEMPO_DEX, 1000),
            "0",
        )]);
        let err = validate_swap_policy(
            Some(&tempo_action()),
            &s,
            4217,
            &[],
            &PolicyOptions::default(),
        )
        .expect_err("approve-only batch must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(
            err.to_string().contains("at least one swap call"),
            "got: {err}"
        );
    }

    #[test]
    fn tempo_batched_calls_reject_approve_on_wrong_token() {
        // B5: Go TestValidateTempoSwapBatchedCallsRejectsApproveOnWrongToken.
        let wrong_token = "0xba00000000000000000000000000000000000000";
        let s = tempo_batched_step(vec![
            call(wrong_token, &approve_calldata(TEMPO_DEX, 1000), "0"),
            call(TEMPO_DEX, &tempo_swap_exact_in_calldata(), "0"),
        ]);
        let err = validate_swap_policy(
            Some(&tempo_action()),
            &s,
            4217,
            &[],
            &PolicyOptions::default(),
        )
        .expect_err("approve on wrong token must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(err.to_string().contains("input token"), "got: {err}");
    }

    #[test]
    fn tempo_batched_calls_reject_extra_approval() {
        // B6: Go TestValidateTempoSwapBatchedCallsRejectsExtraApproval.
        let s = tempo_batched_step(vec![
            call(TEMPO_TOKEN_IN, &approve_calldata(TEMPO_DEX, 500), "0"),
            call(TEMPO_TOKEN_IN, &approve_calldata(TEMPO_DEX, 500), "0"),
            call(TEMPO_DEX, &tempo_swap_exact_in_calldata(), "0"),
        ]);
        let err = validate_swap_policy(
            Some(&tempo_action()),
            &s,
            4217,
            &[],
            &PolicyOptions::default(),
        )
        .expect_err("two approve calls must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(
            err.to_string().contains("more than one approve"),
            "got: {err}"
        );
    }

    #[test]
    fn tempo_batched_calls_reject_approve_with_value() {
        // B7: Go TestValidateTempoSwapBatchedCallsRejectsApproveWithValue.
        let s = tempo_batched_step(vec![
            call(TEMPO_TOKEN_IN, &approve_calldata(TEMPO_DEX, 1000), "100"),
            call(TEMPO_DEX, &tempo_swap_exact_in_calldata(), "0"),
        ]);
        let err = validate_swap_policy(
            Some(&tempo_action()),
            &s,
            4217,
            &[],
            &PolicyOptions::default(),
        )
        .expect_err("approve with non-zero value must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(err.to_string().contains("zero value"), "got: {err}");
    }

    #[test]
    fn tempo_batched_calls_reject_missing_token_in_metadata() {
        // B8: Go TestValidateTempoSwapBatchedCallsRejectsMissingTokenInMetadata.
        let s = tempo_batched_step(vec![
            call(TEMPO_TOKEN_IN, &approve_calldata(TEMPO_DEX, 1000), "0"),
            call(TEMPO_DEX, &tempo_swap_exact_in_calldata(), "0"),
        ]);
        let mut a = action("1000");
        a.provider = "tempo".into(); // no token_in metadata
        let err = validate_swap_policy(Some(&a), &s, 4217, &[], &PolicyOptions::default())
            .expect_err("missing token_in metadata must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(err.to_string().contains("token_in metadata"), "got: {err}");
    }

    #[test]
    fn tempo_batched_calls_reject_approve_spender_not_dex() {
        // B9 (fresh): approve spender is some non-DEX address; rejected as a
        // non-canonical spender.
        let other_spender = "0x00000000000000000000000000000000000000ee";
        let s = tempo_batched_step(vec![
            call(TEMPO_TOKEN_IN, &approve_calldata(other_spender, 1000), "0"),
            call(TEMPO_DEX, &tempo_swap_exact_in_calldata(), "0"),
        ]);
        let err = validate_swap_policy(
            Some(&tempo_action()),
            &s,
            4217,
            &[],
            &PolicyOptions::default(),
        )
        .expect_err("approve spender != canonical DEX must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(
            err.to_string().contains("canonical stablecoin dex"),
            "got: {err}"
        );
    }

    #[test]
    fn tempo_batched_calls_reject_unsupported_chain() {
        // Fresh: a non-Tempo chain has no canonical DEX → unsupported chain.
        let s = tempo_batched_step(vec![
            call(TEMPO_TOKEN_IN, &approve_calldata(TEMPO_DEX, 1000), "0"),
            call(TEMPO_DEX, &tempo_swap_exact_in_calldata(), "0"),
        ]);
        let err =
            validate_swap_policy(Some(&tempo_action()), &s, 1, &[], &PolicyOptions::default())
                .expect_err("non-tempo chain must be unsupported");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(err.to_string().contains("unsupported chain"), "got: {err}");
    }

    // =====================================================================
    // G: bridge policy
    // =====================================================================

    fn bridge_step(target: &str, provider: &str, endpoint: &str) -> ActionStep {
        let mut s = step(StepType::Bridge, target);
        s.expected_outputs = Some(outputs(&[
            ("settlement_provider", provider),
            ("settlement_status_endpoint", endpoint),
        ]));
        s
    }

    #[test]
    fn bridge_endpoint_guard_rejects_untrusted_and_unsafe_overrides() {
        // G2: Go TestValidateBridgePolicyEndpointGuard.
        let mut a = action("0");
        a.provider = "lifi".into();
        let s = bridge_step(STEP_TARGET, "lifi", "https://evil.example/status");

        let err = validate_step_policy(Some(&a), &s, 1, &[0x01], &PolicyOptions::default())
            .expect_err("untrusted settlement endpoint must fail");
        assert_eq!(err.code, Code::ActionPlan);

        validate_step_policy(
            Some(&a),
            &s,
            1,
            &[0x01],
            &PolicyOptions {
                allow_max_approval: false,
                unsafe_provider_tx: true,
            },
        )
        .expect("unsafe_provider_tx overrides the endpoint guard");
    }

    #[test]
    fn bridge_target_guard_rejects_non_canonical_and_unsafe_overrides() {
        // G3: Go TestValidateBridgePolicyTargetGuard.
        let mut a = action("0");
        a.provider = "lifi".into();
        let s = bridge_step(
            "0x1111111111111111111111111111111111111111",
            "lifi",
            "https://li.quest/v1/status",
        );

        let err = validate_step_policy(Some(&a), &s, 1, &[0x01], &PolicyOptions::default())
            .expect_err("non-canonical bridge target must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(err.to_string().contains("execution contract"), "got: {err}");

        validate_step_policy(
            Some(&a),
            &s,
            1,
            &[0x01],
            &PolicyOptions {
                allow_max_approval: false,
                unsafe_provider_tx: true,
            },
        )
        .expect("unsafe_provider_tx overrides the target guard");
    }

    #[test]
    fn bridge_allows_canonical_across_target() {
        // G4: Go TestValidateBridgePolicyAllowsCanonicalTarget (Across on Base).
        let mut a = action("0");
        a.provider = "across".into();
        let s = bridge_step(
            "0x767e4c20F521a829dE4Ffc40C25176676878147f",
            "across",
            "https://app.across.to/api/deposit/status",
        );
        validate_step_policy(Some(&a), &s, 8453, &[0x01], &PolicyOptions::default())
            .expect("canonical across target on Base passes");
    }

    #[test]
    fn bridge_allows_canonical_lifi_target() {
        // G5: Go TestValidateBridgePolicyAllowsCanonicalLiFiTarget (LiFi on L1).
        let mut a = action("0");
        a.provider = "lifi".into();
        let s = bridge_step(
            "0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE",
            "lifi",
            "https://li.quest/v1/status",
        );
        validate_step_policy(Some(&a), &s, 1, &[0x01], &PolicyOptions::default())
            .expect("canonical lifi target on Ethereum passes");
    }

    #[test]
    fn bridge_skips_target_check_on_uncovered_chain() {
        // G6: Go TestValidateBridgePolicySkipsTargetCheckOnUncoveredChain
        // (Across on Avalanche 43114 has no target policy).
        let mut a = action("0");
        a.provider = "across".into();
        let s = bridge_step(
            "0x1111111111111111111111111111111111111111",
            "across",
            "https://app.across.to/api/deposit/status",
        );
        validate_step_policy(Some(&a), &s, 43114, &[0x01], &PolicyOptions::default())
            .expect("uncovered chain skips target check");
    }

    #[test]
    fn bridge_rejects_unknown_settlement_provider() {
        // G7 (fresh): provider neither lifi nor across.
        let mut a = action("0");
        a.provider = "wormhole".into();
        let s = bridge_step(
            STEP_TARGET,
            "wormhole",
            "https://app.across.to/api/deposit/status",
        );
        let err = validate_step_policy(Some(&a), &s, 1, &[0x01], &PolicyOptions::default())
            .expect_err("unknown settlement provider must fail");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(
            err.to_string().contains("settlement provider"),
            "got: {err}"
        );
    }

    #[test]
    fn bridge_unsafe_provider_tx_bypasses_all_checks() {
        // G1: with unsafe_provider_tx every bridge guard is skipped, even with a
        // bogus provider + target + endpoint.
        let mut a = action("0");
        a.provider = "wormhole".into();
        let s = bridge_step(
            "0x1111111111111111111111111111111111111111",
            "wormhole",
            "https://evil.example/status",
        );
        validate_step_policy(
            Some(&a),
            &s,
            1,
            &[0x01],
            &PolicyOptions {
                allow_max_approval: false,
                unsafe_provider_tx: true,
            },
        )
        .expect("unsafe_provider_tx bypasses all bridge checks");
    }
}
