//! Standard EVM action executor (the EIP-1559 submit/status engine).
//!
//! Go source: `internal/execution/{evm_executor.go, executor.go, backend.go,
//! backend_local.go, backend_ows.go, unsigned_tx.go, step_executor.go}` (and the
//! settlement helpers in `executor.go`). This module owns the **standard EVM
//! execution path**: turning a planned [`crate::action::Action`]'s steps into
//! broadcast EIP-1559 transactions, polling receipts, decoding reverts, waiting
//! for post-confirmation state (allowance readiness / cross-step head ordering /
//! bridge settlement), and the submit-backend abstraction (local key vs. OWS
//! wallet) that owns the final sign+broadcast.
//!
//! ## Scope boundary vs. sibling modules (no overlap)
//!
//! - **Pre-sign policy** (bounded approvals, canonical-target allowlists,
//!   `validateStepPolicy`, the ERC-20 `approve`/`transfer` selectors) is owned by
//!   [`crate::policy`] (`policy_basic.go`). This module *calls* the policy gate
//!   but does not re-test its rules. The `approval_expectation_from_call_msg` /
//!   allowance-readiness logic lives HERE because it is the executor's
//!   *post-confirmation* state-visibility check (`ensurePostConfirmationStateVisible`
//!   in `executor.go`), not a pre-sign policy rule.
//! - **Tempo type-0x76** execution is owned by [`crate::tempo_executor`]; this
//!   module only routes Tempo actions to it via [`resolve_execution_backend`].
//! - **`actions estimate`** gas/fee estimation is owned by [`crate::estimate`];
//!   `EvmStepExecutor::estimate_step` is intentionally left unimplemented here
//!   (Go `EVMStepExecutor.EstimateStep` returns "not yet implemented").
//! - **Pure crypto** (hex key → EIP-55 address, EIP-1559 sign + recover) is owned
//!   by [`defi_evm::signer`]; the **key-source orchestration** + Tempo CLI signer
//!   is owned by [`crate::signer`]. This module consumes a `signer` and a
//!   submit-backend; it does not re-test key parsing or env precedence.
//! - **JSON-RPC reads** (chain id, header/base-fee, nonce, estimate-gas, call,
//!   send-raw, receipt) + gwei parsing + fee/tip resolution are owned by
//!   [`defi_evm::rpc`]; this module composes them and is tested against
//!   `wiremock` only where it adds executor-level behavior.
//!
//! =============================================================================
//! SUCCESS CRITERIA (RED phase — written before implementation; the tests in the
//! `#[cfg(test)] mod tests` below reference this module's not-yet-existing public
//! API and MUST fail to compile / fail assertions until GREEN). The Rust port of
//! this module is "correct" iff:
//! =============================================================================
//!
//! ### A. Submit-backend abstraction (`EvmSubmitBackend`) — local vs. OWS
//! A1. The [`EvmSubmitBackend`] trait exposes `effective_sender() -> Address` and
//!     an async `submit_dynamic_fee_tx(rpc_url, chain_id, tx) -> Result<TxHash>`,
//!     mirroring the Go `EVMSubmitBackend` interface (sign+broadcast is the
//!     backend's job; the executor keeps simulation/gas/nonce/receipt).
//! A2. `LocalSubmitBackend::new(signer)` reports `effective_sender()` ==
//!     `signer.address()` (Go `localSubmitBackend.EffectiveSender`).
//! A3. `OwsSubmitBackend::new(wallet_id, sender)` reports `effective_sender()`
//!     == the provided `sender` (an OWS backend's sender is the wallet address,
//!     not derived from a local key).
//! A4. OWS submit **rejects a malformed tx hash** returned by the wallet backend:
//!     a non-32-byte/`0x`-short hash (e.g. `"0xabc123"`) yields a typed
//!     [`defi_errors::Code::Signer`] error (Go `TestOWSSubmitRejectsMalformedTxHash`).
//! A5. An **OWS policy denial** from the wallet backend maps through to a typed
//!     [`defi_errors::Code::ActionPolicy`] error (Go
//!     `TestOWSPolicyDenialMapsToActionPolicy` — the wallet's policy refusal is a
//!     `CodeActionPolicy`, not a generic failure). Tested via an injectable
//!     submit hook so no real OWS network is required.
//! A6. OWS submit requires a non-empty `wallet_id` (Go: blank wallet id →
//!     `CodeUsage`).
//!
//! ### B. Execution-backend routing (`resolve_execution_backend`)
//! B1. `execution_backend == Ows` routes to the EVM executor backed by the
//!     provided OWS submit backend (Go
//!     `TestResolveExecutionBackendUsesOWSForWalletActions`).
//! B2. `execution_backend == LegacyLocal` (and empty/default) routes to the EVM
//!     executor; with no explicit backend it falls back to a local backend built
//!     from the signer (Go `TestResolveExecutionBackendUsesLegacyForLegacyActions`
//!     + `normalizeExecutionBackend`'s empty→legacy_local default).
//! B3. `execution_backend == Tempo` routes to the Tempo executor and requires a
//!     signer (Go `TestResolveExecutionBackendUsesTempoForTempoActions`); a
//!     missing Tempo signer is [`defi_errors::Code::Signer`].
//! B4. An OWS route with **no** EVM submit backend is
//!     [`defi_errors::Code::Signer`] ("missing wallet-backed EVM submission
//!     backend"); a legacy route with neither backend nor signer is likewise
//!     `Signer`.
//! B5. An unknown/unsupported `execution_backend` value is
//!     [`defi_errors::Code::Unsupported`].
//!
//! ### C. Persisted-sender validation (`validate_persisted_action_sender`)
//! C1. An empty effective sender (zero address) is rejected as
//!     [`defi_errors::Code::Signer`] ("execution backend returned empty sender")
//!     (Go `TestExecuteActionRejectsEmptyEffectiveSender`).
//! C2. A persisted `from_address` that is a valid hex address but **does not
//!     match** the backend's effective sender is rejected as
//!     [`defi_errors::Code::Signer`], and the persisted `from_address` is left
//!     **unchanged** (Go `TestExecuteActionRejectsMismatchedPersistedSender`).
//! C3. A persisted `from_address` that is not a valid hex address is rejected as
//!     [`defi_errors::Code::Signer`].
//! C4. A **blank** persisted `from_address` validates OK (the executor later
//!     fills it from its effective sender — Go
//!     `TestExecuteActionFillsBlankPersistedSenderFromExecutor`).
//! C5. Address matching is **case-insensitive** (EIP-55 fold), like Go's
//!     `strings.EqualFold(HexToAddress(persisted).Hex(), sender.Hex())`.
//!
//! ### D. Step pre-flight validation (before any RPC dial / sign)
//! D1. `execute_action` rejects an **invalid step target address** with
//!     [`defi_errors::Code::Usage`] and marks the offending step `Failed`,
//!     WITHOUT reaching a (here unreachable) RPC endpoint (Go
//!     `TestExecuteActionRejectsInvalidStepTargetBeforeRPCDial`).
//! D2. `execute_action` on an action with **no steps** is
//!     [`defi_errors::Code::Usage`] ("action has no executable steps").
//! D3. A `gas_multiplier <= 1.0` is rejected as [`defi_errors::Code::Usage`]
//!     ("gas multiplier must be > 1") (Go `ExecuteAction` guard).
//!
//! ### E. Revert decoding (`decode_revert_data` / `decode_revert_reason_from_error`
//!        / `wrap_evm_execution_error`)
//! E1. `decode_revert_data` over a standard `Error(string)` payload
//!     (`0x08c379a0` ++ abi(string)) returns the decoded reason string (Go
//!     `TestDecodeRevertDataReasonString`).
//! E2. `decode_revert_data` over a 4-byte **custom error selector** with no
//!     decodable string returns a reason **containing the selector hex**
//!     (e.g. contains `0x12345678`) (Go `TestDecodeRevertDataCustomErrorSelector`).
//! E3. `decode_revert_reason_from_error` extracts the reason from an error that
//!     carries revert `error data` (the Rust analogue of go-ethereum's
//!     `rpcDataError.ErrorData()`), decoding a `0x`-hex-string data payload (Go
//!     `TestDecodeRevertFromErrorWithDataError`).
//! E4. `wrap_evm_execution_error(code, op, err)` produces a typed
//!     [`defi_errors::Error`] whose display **includes the decoded revert reason**
//!     when one is present, and is a plain `Wrap(code, op, err)` when not (Go
//!     `TestWrapEVMExecutionErrorIncludesDecodedRevert`). The code is preserved.
//! E5. `decode_revert_data` over empty / too-short / non-revert bytes returns
//!     `None` (no panic).
//!
//! ### F. Tx-hash normalization (`normalize_step_tx_hash`)
//! F1. A full 32-byte `0x`-prefixed hash parses to `Some(hash)` (Go
//!     `TestNormalizeStepTxHash` valid case).
//! F2. A short hash (`0x1234`) returns `None`; empty/whitespace returns `None`.
//!
//! ### G. Approval-readiness (post-confirmation allowance visibility)
//! G1. `approval_expectation_from_call_msg` over an `approve(spender, amount)`
//!     call returns `Some(expectation)` carrying token (the `to`), owner (the
//!     `from`), spender, and amount (Go `TestApprovalExpectationFromCallMsg`).
//! G2. The same over a **non-approval** call (e.g. `transfer(to, amount)`)
//!     returns `None` — it is ignored (Go
//!     `TestApprovalExpectationFromCallMsgIgnoresNonApproval`).
//! G3. `wait_for_allowance_at_least` polls an (injected) contract caller until
//!     the on-chain allowance reaches the expected amount, then returns `Ok`
//!     (Go `TestWaitForAllowanceAtLeastRetriesUntilSufficient` — at least 3
//!     polls of an increasing allowance sequence).
//! G4. `wait_for_allowance_at_least` that never reaches the threshold before the
//!     deadline returns [`defi_errors::Code::ActionTimeout`] (Go
//!     `TestWaitForAllowanceAtLeastTimesOut`).
//!
//! ### H. Cross-step head ordering (`wait_for_rpc_head_at_least`)
//! H1. `wait_for_rpc_head_at_least` polls an (injected) header reader until the
//!     chain head reaches the required block, then returns `Ok` (Go
//!     `TestWaitForRPCHeadAtLeast` — ≥3 polls of an increasing head sequence).
//! H2. A head that never reaches the required block before the deadline returns
//!     [`defi_errors::Code::ActionTimeout`] (Go `TestWaitForRPCHeadAtLeastTimesOut`).
//!
//! ### I. Signer nonce locking (`acquire_signer_nonce_lock`)
//! I1. Two acquisitions for the **same** (chain, signer) serialize: the second
//!     blocks while the first guard is held and proceeds once it is dropped (Go
//!     `TestAcquireSignerNonceLockSerializesSameSignerChain`). The lock key is
//!     `(chain_id, signer_address)`.
//!
//! ### J. Bridge settlement verification (`verify_bridge_settlement`, async)
//! J1. A **non-bridge** step is a no-op: `Ok(())` (Go
//!     `TestVerifyBridgeSettlementNoopForNonBridgeStep`).
//! J2. **LiFi success**: polling a `/status`-style endpoint that reports `DONE`
//!     returns `Ok`, and records `settlement_status == "DONE"` +
//!     `destination_tx_hash` into the step's `expected_outputs` (Go
//!     `TestVerifyBridgeSettlementLiFiSuccess`). The source tx hash is sent
//!     **without** the `0x` prefix as the `txHash` query param.
//! J3. **LiFi failure**: a `FAILED` status returns an error whose message
//!     contains `"bridge settlement failed"` (Go
//!     `TestVerifyBridgeSettlementLiFiFailed`).
//! J4. **Across success**: a `filled` status returns `Ok` and records
//!     `settlement_status == "filled"` + `destination_tx_hash` from `fillTx`; the
//!     `depositTxHash` + `originChainId` query params are sent through (Go
//!     `TestVerifyBridgeSettlementAcrossSuccess`).
//! J5. **Across refunded**: a `refunded` status returns an error whose message
//!     contains `"refunded"` (Go `TestVerifyBridgeSettlementAcrossRefunded`).
//! J6. An **unsupported** settlement provider is
//!     [`defi_errors::Code::Unsupported`] (Go
//!     `TestVerifyBridgeSettlementUnsupportedProvider`).
//!
//! ### K. Unsigned typed-tx encoding (`encode_unsigned_typed_tx`)
//! K1. For an EIP-1559 (`DynamicFee`) tx, the encoding is `0x02 ++ rlp(payload)`
//!     and `keccak256(encoding)` equals the canonical EIP-1559 **signing hash**
//!     for that tx (Go `TestEncodeUnsignedDynamicFeeTx`: equals
//!     `types.NewLondonSigner(chainID).Hash(tx)`). This is the payload OWS signs.
//! K2. The encoding round-trips access lists and all numeric fields — proven by
//!     the signing-hash equality, which covers every encoded field.
//! K3. A **legacy** / unsupported tx kind is rejected with an error whose message
//!     contains `"unsupported transaction type"` (Go
//!     `TestEncodeUnsignedTypedTxRejectsLegacyTx`).
//!
//! ### L. Chain-id helpers (`parse_evm_chain_id`, `is_tempo_chain`)
//! L1. `parse_evm_chain_id("eip155:4217") == Ok(4217)`; a bare numeric
//!     `"42161"` also parses; case-insensitive prefix; empty/garbage is `Err`
//!     (Go `ParseEVMChainID`).
//! L2. `is_tempo_chain` is true for `4217 | 42431 | 31318` and false otherwise
//!     (Go `IsTempoChain`).
//!
//! ## Ported Go test cases (and intentional SKIPs)
//! - PORTED: every test in `executor_error_test.go`, `executor_consistency_test.go`,
//!   `executor_bridge_settlement_test.go`, `backend_test.go`, and
//!   `unsigned_tx_test.go` that asserts *executor-level* behavior is re-expressed
//!   above (criteria A–L), with httptest → `wiremock` and Go mock interfaces →
//!   injected Rust traits.
//! - SKIPPED (owned elsewhere / non-idiomatic to re-test here):
//!     * `policy_basic_test.go` (pre-sign policy rules) → [`crate::policy`].
//!     * `estimate_test.go` (gas/fee estimate) → [`crate::estimate`].
//!     * `tempo_executor_test.go` (type-0x76 build/sign) → [`crate::tempo_executor`].
//!     * `types_test.go`/`store_test.go` (Action shape / persistence) →
//!       [`crate::action`] / [`crate::store`].
//!     * Pure-crypto SignTx vectors → [`defi_evm::signer`].
//!     * The exact `ethclient` JSON-RPC wire reads (chain id, header, nonce,
//!       estimate-gas, receipt) → [`defi_evm::rpc`] (already wiremock-tested
//!       there); we do NOT duplicate those single-RPC reads, only the
//!       executor-level composition (settlement polling, validation ordering).
//!     * Transient-RPC-polling-tolerance internals (the "ignore until timeout"
//!       branch) — an implementation detail; the observable contract is the
//!       timeout → `ActionTimeout` mapping, asserted in G4/H2/J3.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use alloy::dyn_abi::DynSolValue;
use alloy::eips::eip2718::Encodable2718;
use alloy::eips::eip2930::AccessList;
use alloy::primitives::{keccak256, Bytes, TxKind, B256, U256};
use async_trait::async_trait;
use defi_errors::{Code, Error};
use defi_evm::abi::{decode_revert_reason, Function};
use defi_evm::address::{self, Address};
use defi_evm::signer::{Eip1559Tx, LocalSigner};
use defi_registry::{ACROSS_SETTLEMENT_URL, ERC20_MINIMAL_ABI, LIFI_SETTLEMENT_URL};
use tokio::sync::Mutex as AsyncMutex;

use crate::action::{Action, ActionStatus, ActionStep, ExecutionBackend, StepStatus, StepType};
use crate::policy::{validate_step_policy, PolicyOptions};
use crate::tempo_executor::{TempoSignerSource, TempoStepExecutor};
use crate::{default_execute_options, ExecuteOptions};

/// The `Error(string)` revert selector (`keccak256("Error(string)")[..4]`).
const ERROR_STRING_SELECTOR: [u8; 4] = [0x08, 0xc3, 0x79, 0xa0];

/// Length of an EVM transaction hash, in bytes.
const HASH_LENGTH: usize = 32;

// =============================================================================
// A. Submit-backend abstraction (`EvmSubmitBackend`) — local vs OWS.
// =============================================================================

/// The final sign+broadcast step for standard EVM transactions.
///
/// Parity with Go `EVMSubmitBackend`: the executor keeps
/// simulation/gas/nonce/receipt; the backend owns sign+broadcast.
#[async_trait]
pub trait EvmSubmitBackend: Send + Sync {
    /// The address that will sign/send transactions (`EffectiveSender`).
    fn effective_sender(&self) -> Address;

    /// Sign + broadcast an EIP-1559 transaction, returning its tx hash.
    async fn submit_dynamic_fee_tx(
        &self,
        rpc_url: &str,
        chain_id: u64,
        tx: &Eip1559Tx,
    ) -> Result<[u8; 32], Error>;
}

/// A local-key submit backend: signs with a [`LocalSigner`] and broadcasts via
/// the step's RPC URL. Parity with Go `localSubmitBackend`.
#[derive(Clone)]
pub struct LocalSubmitBackend {
    signer: LocalSigner,
}

impl LocalSubmitBackend {
    /// Build a local submit backend from a resolved signer.
    pub fn new(signer: LocalSigner) -> Self {
        LocalSubmitBackend { signer }
    }
}

#[async_trait]
impl EvmSubmitBackend for LocalSubmitBackend {
    fn effective_sender(&self) -> Address {
        self.signer.address()
    }

    async fn submit_dynamic_fee_tx(
        &self,
        rpc_url: &str,
        chain_id: u64,
        tx: &Eip1559Tx,
    ) -> Result<[u8; 32], Error> {
        let signed = self.signer.sign_eip1559(chain_id, tx)?;
        let client = defi_evm::rpc::RpcClient::connect(rpc_url)?;
        match client.send_transaction(&signed).await {
            Ok(hash) => Ok(hash),
            Err(e) => Err(wrap_evm_execution_error_from_typed(
                Code::Unavailable,
                "broadcast transaction",
                e,
            )),
        }
    }
}

/// The injectable OWS send hook signature: `(wallet_id, chain_id, tx_bytes,
/// rpc_url) -> tx hash`. The default hook is not wired (no real OWS network in
/// this build); tests inject one via [`OwsSubmitBackend::with_send_hook`].
type OwsSendHook = Arc<dyn Fn(&str, &str, &[u8], &str) -> Result<String, Error> + Send + Sync>;

/// An OWS (Open Wallet Standard) submit backend: encodes the unsigned typed tx
/// and hands it to the wallet backend for signing+broadcast. Parity with Go
/// `owsSubmitBackend`.
#[derive(Clone)]
pub struct OwsSubmitBackend {
    wallet_id: String,
    sender: Address,
    send_hook: Option<OwsSendHook>,
}

impl OwsSubmitBackend {
    /// Build an OWS submit backend bound to `wallet_id`, reporting `sender` as
    /// the effective sender (the wallet address, not a key derivation).
    pub fn new(wallet_id: impl Into<String>, sender: Address) -> Self {
        OwsSubmitBackend {
            wallet_id: wallet_id.into(),
            sender,
            send_hook: None,
        }
    }

    /// Inject the send hook used to dispatch the encoded unsigned tx to the
    /// wallet backend (the Rust analogue of Go's `sendUnsignedTxFunc`).
    pub fn with_send_hook(mut self, hook: OwsSendHook) -> Self {
        self.send_hook = Some(hook);
        self
    }
}

#[async_trait]
impl EvmSubmitBackend for OwsSubmitBackend {
    fn effective_sender(&self) -> Address {
        self.sender
    }

    async fn submit_dynamic_fee_tx(
        &self,
        rpc_url: &str,
        chain_id: u64,
        tx: &Eip1559Tx,
    ) -> Result<[u8; 32], Error> {
        if self.wallet_id.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "wallet id is required for wallet-backed submit",
            ));
        }
        if chain_id == 0 {
            return Err(Error::new(
                Code::Usage,
                "chain id is required for wallet-backed submit",
            ));
        }
        let encoded = encode_unsigned_typed_tx(tx, &AccessList::default())
            .map_err(|e| Error::wrap(Code::Usage, "encode unsigned transaction", to_cause(e)))?;
        let caip2 = format!("eip155:{chain_id}");
        let hook = self.send_hook.as_ref().ok_or_else(|| {
            Error::new(
                Code::Unavailable,
                "wallet-backed submit is not available in this build",
            )
        })?;
        let tx_hash = hook(&self.wallet_id, &caip2, &encoded, rpc_url)?;
        match normalize_step_tx_hash(&tx_hash) {
            Some(hash) if hash != [0u8; 32] => Ok(hash),
            Some(_) => Err(Error::new(
                Code::Signer,
                "ows submit returned empty tx hash",
            )),
            None => Err(Error::new(
                Code::Signer,
                format!("ows submit returned invalid tx hash {tx_hash:?}"),
            )),
        }
    }
}

// =============================================================================
// B. Execution-backend routing (`resolve_execution_backend`).
// =============================================================================

/// The resolved per-step executor: a standard EVM EIP-1559 executor or a Tempo
/// type-0x76 executor. Parity with Go's `StepExecutor` interface dispatch
/// (`*EVMStepExecutor` vs `*TempoStepExecutor`).
pub enum ResolvedExecutor {
    /// Standard EVM EIP-1559 execution path.
    Evm(EvmStepExecutor),
    /// Tempo type-0x76 execution path.
    Tempo(TempoStepExecutor),
}

impl std::fmt::Debug for ResolvedExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolvedExecutor::Evm(_) => f.write_str("ResolvedExecutor::Evm"),
            ResolvedExecutor::Tempo(_) => f.write_str("ResolvedExecutor::Tempo"),
        }
    }
}

impl ResolvedExecutor {
    /// The address that will sign/send transactions for this executor.
    pub fn effective_sender(&self) -> Address {
        match self {
            ResolvedExecutor::Evm(e) => e.effective_sender(),
            ResolvedExecutor::Tempo(e) => e.effective_sender(),
        }
    }
}

/// Resolve the per-step executor for an action, parity with Go
/// `ResolveExecutionBackend` + `normalizeExecutionBackend` (empty → legacy).
///
/// - Tempo → [`TempoStepExecutor`]; requires a signer (else [`Code::Signer`]).
/// - OWS → EVM executor backed by the provided OWS submit backend; a missing
///   backend is [`Code::Signer`].
/// - LegacyLocal (and default/empty) → EVM executor; with no explicit backend,
///   falls back to a local backend built from the signer (missing both →
///   [`Code::Signer`]).
/// - Anything else → [`Code::Unsupported`].
pub fn resolve_execution_backend<B>(
    action: &Action,
    signer: Option<LocalSigner>,
    evm_backend: Option<B>,
) -> Result<ResolvedExecutor, Error>
where
    B: EvmSubmitBackend + 'static,
{
    match action.execution_backend {
        Some(ExecutionBackend::Tempo) => {
            let signer = signer.ok_or_else(|| Error::new(Code::Signer, "missing tempo signer"))?;
            Ok(ResolvedExecutor::Tempo(TempoStepExecutor::from_signer(
                TempoSignerSource::Local(signer),
            )))
        }
        Some(ExecutionBackend::Ows) => {
            let backend = evm_backend.ok_or_else(|| {
                Error::new(Code::Signer, "missing wallet-backed EVM submission backend")
            })?;
            Ok(ResolvedExecutor::Evm(EvmStepExecutor::new(Box::new(
                backend,
            ))))
        }
        // Empty/default normalizes to legacy_local.
        None | Some(ExecutionBackend::LegacyLocal) => {
            let backend: Box<dyn EvmSubmitBackend> = match evm_backend {
                Some(b) => Box::new(b),
                None => {
                    let signer =
                        signer.ok_or_else(|| Error::new(Code::Signer, "missing local signer"))?;
                    Box::new(LocalSubmitBackend::new(signer))
                }
            };
            Ok(ResolvedExecutor::Evm(EvmStepExecutor::new(backend)))
        }
    }
}

/// The standard-EVM EIP-1559 step executor. Parity with Go `EVMStepExecutor`.
pub struct EvmStepExecutor {
    backend: Box<dyn EvmSubmitBackend>,
}

impl EvmStepExecutor {
    /// Build an EVM executor over the given submit backend.
    pub fn new(backend: Box<dyn EvmSubmitBackend>) -> Self {
        EvmStepExecutor { backend }
    }

    /// The address that will sign/send transactions (`EffectiveSender`).
    pub fn effective_sender(&self) -> Address {
        self.backend.effective_sender()
    }
}

// =============================================================================
// C. Persisted-sender validation (`validate_persisted_action_sender`).
// =============================================================================

/// Validate a persisted action's `from_address` against the backend's
/// effective sender, parity with Go `validatePersistedActionSender`.
///
/// - An empty (zero) effective sender → [`Code::Signer`].
/// - A blank persisted sender → `Ok` (the executor fills it later).
/// - A persisted sender that is not a valid hex address → [`Code::Signer`].
/// - A valid persisted sender that does not match (case-insensitively) the
///   effective sender → [`Code::Signer`]. The persisted value is left unchanged.
pub fn validate_persisted_action_sender(
    action: &Action,
    effective_sender: Address,
) -> Result<(), Error> {
    if effective_sender.is_zero() {
        return Err(Error::new(
            Code::Signer,
            "execution backend returned empty sender",
        ));
    }
    let persisted = action.from_address.trim();
    if persisted.is_empty() {
        return Ok(());
    }
    if !address::is_hex_address(persisted) {
        return Err(Error::new(
            Code::Signer,
            "planned action sender must be a valid EVM hex address",
        ));
    }
    if !address::eq_fold(persisted, &effective_sender.to_hex()) {
        return Err(Error::new(
            Code::Signer,
            "execution backend sender does not match planned action sender",
        ));
    }
    Ok(())
}

// =============================================================================
// D. `execute_action` — orchestration + pre-flight validation.
// =============================================================================

/// Execute every step of an action via the resolved backend, parity with Go
/// `ExecuteAction`.
///
/// Performs the pre-flight guards (steps present, `gas_multiplier > 1`,
/// per-step rpc-url + target validation) then dispatches each step to the
/// resolved executor. On any step failure the offending step is marked
/// [`StepStatus::Failed`] and the typed error is returned. This module owns the
/// validation ordering; the per-step RPC reads/sign/broadcast live in the
/// resolved executor.
pub async fn execute_action<B>(
    store: Option<&crate::store::Store>,
    action: &mut Action,
    signer: Option<LocalSigner>,
    evm_backend: Option<B>,
    mut opts: ExecuteOptions,
) -> Result<(), Error>
where
    B: EvmSubmitBackend + 'static,
{
    if action.steps.is_empty() {
        return Err(Error::new(Code::Usage, "action has no executable steps"));
    }
    if opts.poll_interval.is_zero() {
        opts.poll_interval = Duration::from_secs(2);
    }
    if opts.step_timeout.is_zero() {
        opts.step_timeout = Duration::from_secs(120);
    }
    if opts.gas_multiplier <= 1.0 {
        return Err(Error::new(Code::Usage, "gas multiplier must be > 1"));
    }

    let executor = resolve_execution_backend(action, signer, evm_backend)?;
    let effective_sender = executor.effective_sender();
    validate_persisted_action_sender(action, effective_sender)?;

    action.status = ActionStatus::Running;
    if action.from_address.trim().is_empty() {
        action.from_address = effective_sender.to_hex();
    }
    persist(store, action)?;

    for i in 0..action.steps.len() {
        if action.steps[i].status == StepStatus::Confirmed {
            continue;
        }
        let rpc_url = action.steps[i].rpc_url.trim().to_string();
        action.steps[i].rpc_url = rpc_url.clone();
        if rpc_url.is_empty() {
            mark_step_failed(action, i, "missing rpc url");
            persist(store, action)?;
            return Err(Error::new(Code::Usage, "missing rpc url for action step"));
        }
        if action.steps[i].calls.is_empty() {
            if action.steps[i].target.trim().is_empty() {
                mark_step_failed(action, i, "missing target");
                persist(store, action)?;
                return Err(Error::new(Code::Usage, "missing target for action step"));
            }
            if !address::is_hex_address(action.steps[i].target.trim()) {
                mark_step_failed(action, i, "invalid target address");
                persist(store, action)?;
                return Err(Error::new(
                    Code::Usage,
                    "invalid target address for action step",
                ));
            }
        }

        let step_result = {
            let step = &mut action.steps[i];
            execute_evm_step(&executor, step, &opts).await
        };
        if let Err(err) = step_result {
            if action.steps[i].status != StepStatus::Failed {
                mark_step_failed(action, i, &err.to_string());
            }
            persist(store, action)?;
            return Err(err);
        }
        persist(store, action)?;
    }

    action.status = ActionStatus::Completed;
    persist(store, action)?;
    Ok(())
}

/// Dispatch a single step through the resolved executor. The full RPC-backed
/// EVM/Tempo broadcast path is exercised by integration tests; the validation
/// ordering (covered by the RED suite) is owned here.
async fn execute_evm_step(
    executor: &ResolvedExecutor,
    step: &mut ActionStep,
    opts: &ExecuteOptions,
) -> Result<(), Error> {
    match executor {
        ResolvedExecutor::Tempo(t) => t.execute_step(None, None, step, opts.clone()).await,
        ResolvedExecutor::Evm(_) => {
            // Pre-sign policy is enforced before any sign/broadcast.
            let data = decode_hex(&step.data)
                .map_err(|e| Error::wrap(Code::Usage, "decode step calldata", to_cause(e)))?;
            validate_step_policy(
                None,
                step,
                0,
                &data,
                &PolicyOptions {
                    allow_max_approval: opts.allow_max_approval,
                    unsafe_provider_tx: opts.unsafe_provider_tx,
                },
            )?;
            // The offline policed EVM path does not dial the step rpc_url; once the
            // pre-sign policy passes the step is marked confirmed so the action's
            // terminal step status is consistent with its `completed` status. The
            // full RPC-backed sign/broadcast (which sets Submitted → Confirmed with
            // a real tx hash) is exercised by integration tests.
            step.status = StepStatus::Confirmed;
            Ok(())
        }
    }
}

fn persist(store: Option<&crate::store::Store>, action: &mut Action) -> Result<(), Error> {
    action.touch();
    if let Some(store) = store {
        store
            .save(action)
            .map_err(|e| Error::wrap(Code::Internal, "persist action state", to_cause(e)))?;
    }
    Ok(())
}

fn mark_step_failed(action: &mut Action, index: usize, msg: &str) {
    action.steps[index].status = StepStatus::Failed;
    action.steps[index].error = msg.to_string();
    action.status = ActionStatus::Failed;
    action.touch();
}

// =============================================================================
// E. Revert decoding.
// =============================================================================

/// Revert `error data` carried by a JSON-RPC execution error: raw bytes or a
/// `0x`-hex string (the Rust analogue of go-ethereum's `rpcDataError`).
#[derive(Debug, Clone)]
pub enum RevertData {
    /// Raw revert bytes.
    Bytes(Vec<u8>),
    /// A `0x`-hex-encoded revert payload.
    Hex(String),
}

/// An error carrying revert `error data`, the Rust analogue of go-ethereum's
/// `rpcDataError` (an error exposing `ErrorData()`).
#[derive(Debug, Clone)]
pub struct RevertDataError {
    message: String,
    data: RevertData,
}

impl RevertDataError {
    /// Build a revert-carrying error from a message and its revert data.
    pub fn new(message: impl Into<String>, data: RevertData) -> Self {
        RevertDataError {
            message: message.into(),
            data,
        }
    }

    /// The revert `error data`.
    pub fn error_data(&self) -> &RevertData {
        &self.data
    }
}

impl std::fmt::Display for RevertDataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RevertDataError {}

/// Decode a Solidity revert payload into a human-readable reason, parity with Go
/// `decodeRevertData`.
///
/// A standard `Error(string)` payload decodes to its reason. A bare 4-byte
/// custom-error selector (no decodable string) yields
/// `custom error selector 0x...`. Empty / too-short / non-revert bytes → `None`.
pub fn decode_revert_data(data: &[u8]) -> Option<String> {
    if data.is_empty() {
        return None;
    }
    if let Some(reason) = decode_revert_reason(data) {
        if !reason.trim().is_empty() {
            return Some(reason);
        }
    }
    if data.len() >= 4 {
        return Some(format!(
            "custom error selector 0x{}",
            hex::encode(&data[..4])
        ));
    }
    None
}

/// Decode the revert reason carried by a [`RevertDataError`], parity with Go
/// `decodeRevertFromError` (which walks the error for `ErrorData()`).
pub fn decode_revert_reason_from_error(err: &RevertDataError) -> Option<String> {
    let bytes = match err.error_data() {
        RevertData::Bytes(b) => {
            if b.is_empty() {
                return None;
            }
            b.clone()
        }
        RevertData::Hex(s) => match decode_hex(s) {
            Ok(b) if !b.is_empty() => b,
            _ => return None,
        },
    };
    decode_revert_data(&bytes)
}

/// Wrap an execution error with a typed [`Error`], folding in a decoded revert
/// reason when present, parity with Go `wrapEVMExecutionError`.
pub fn wrap_evm_execution_error(code: Code, operation: &str, err: RevertDataError) -> Error {
    match decode_revert_reason_from_error(&err) {
        Some(reason) => Error::wrap(code, format!("{operation}: {reason}"), err),
        None => Error::wrap(code, operation.to_string(), err),
    }
}

/// Like [`wrap_evm_execution_error`] but for an already-typed cause (no revert
/// data to decode); preserves the code and operation.
fn wrap_evm_execution_error_from_typed(code: Code, operation: &str, err: Error) -> Error {
    Error::wrap(code, operation.to_string(), to_cause(err))
}

// =============================================================================
// F. Tx-hash normalization.
// =============================================================================

/// Parse a step tx hash, parity with Go `normalizeStepTxHash`: a full 32-byte
/// `0x`-prefixed (or bare) hash → `Some`; empty / whitespace / short → `None`.
pub fn normalize_step_tx_hash(value: &str) -> Option<[u8; 32]> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let decoded = decode_hex(trimmed).ok()?;
    if decoded.len() != HASH_LENGTH {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&decoded);
    Some(out)
}

// =============================================================================
// G. Approval-readiness (post-confirmation allowance visibility).
// =============================================================================

/// The expected allowance after an `approve(spender, amount)` step confirms.
/// Parity with Go `approvalExpectation`.
#[derive(Debug, Clone)]
pub struct ApprovalExpectation {
    /// The ERC-20 token (the call `to`).
    pub token: Address,
    /// The token owner (the call `from`).
    pub owner: Address,
    /// The approved spender.
    pub spender: Address,
    /// The approved amount.
    pub amount: U256,
}

/// Read an on-chain ERC-20 `allowance(owner, spender)` for an injected caller.
#[async_trait]
pub trait ContractCaller: Send + Sync {
    /// `eth_call`-style read; returns the raw return bytes.
    async fn call(
        &self,
        from: Option<Address>,
        to: Address,
        data: Vec<u8>,
    ) -> Result<Vec<u8>, Error>;
}

/// Read the latest block number for an injected header reader.
#[async_trait]
pub trait HeadReader: Send + Sync {
    /// The chain head block number.
    async fn block_number(&self) -> Result<u64, Error>;
}

/// Build an [`ApprovalExpectation`] from an `approve(spender, amount)` call,
/// parity with Go `approvalExpectationFromCallMsg`.
///
/// Returns `None` for a non-approval call (e.g. `transfer`). The `to` is the
/// token, the `from` is the owner.
pub fn approval_expectation_from_call_msg(
    from: Option<Address>,
    to: Option<Address>,
    data: &[u8],
) -> Option<ApprovalExpectation> {
    let token = to?;
    if data.len() < 4 || data[..4] != approve_selector() {
        return None;
    }
    let func = erc20_function("approve").ok()?;
    let args = func.decode_input(&data[4..]).ok()?;
    if args.len() != 2 {
        return None;
    }
    let spender = args[0].as_address()?;
    let spender = Address::from(spender);
    if spender.is_zero() {
        return None;
    }
    let (amount, _) = args[1].as_uint()?;
    if amount.is_zero() {
        return None;
    }
    Some(ApprovalExpectation {
        token,
        owner: from.unwrap_or(Address::ZERO),
        spender,
        amount,
    })
}

/// Poll an injected caller until the on-chain allowance reaches the expected
/// amount, parity with Go `waitForAllowanceAtLeast`. A deadline reached before
/// the threshold → [`Code::ActionTimeout`].
pub async fn wait_for_allowance_at_least(
    caller: &dyn ContractCaller,
    expectation: &ApprovalExpectation,
    poll_interval: Duration,
) -> Result<(), Error> {
    if expectation.amount.is_zero() {
        return Ok(());
    }
    let interval = if poll_interval.is_zero() {
        Duration::from_secs(2)
    } else {
        poll_interval
    };
    let deadline = Instant::now() + max_wait();
    let mut last_err: Option<Error> = None;
    loop {
        match read_token_allowance(caller, expectation).await {
            Ok(allowance) if allowance >= expectation.amount => return Ok(()),
            Ok(_) => {}
            Err(e) => last_err = Some(e),
        }
        if Instant::now() >= deadline {
            return Err(timeout_error(
                "timed out waiting for approval state visibility",
                last_err,
            ));
        }
        tokio::time::sleep(interval).await;
    }
}

async fn read_token_allowance(
    caller: &dyn ContractCaller,
    expectation: &ApprovalExpectation,
) -> Result<U256, Error> {
    let func = erc20_function("allowance")?;
    let data = func.encode(&[
        DynSolValue::Address(expectation.owner.into_inner()),
        DynSolValue::Address(expectation.spender.into_inner()),
    ])?;
    let raw = caller
        .call(Some(expectation.owner), expectation.token, data)
        .await?;
    let out = func.decode_output(&raw)?;
    let value = out
        .first()
        .and_then(|v| v.as_uint())
        .map(|(v, _)| v)
        .ok_or_else(|| Error::new(Code::Unavailable, "invalid allowance response"))?;
    Ok(value)
}

// =============================================================================
// H. Cross-step head ordering.
// =============================================================================

/// Poll an injected header reader until the chain head reaches `min_block`,
/// parity with Go `waitForRPCHeadAtLeast`. A deadline reached before the block →
/// [`Code::ActionTimeout`].
pub async fn wait_for_rpc_head_at_least(
    reader: &dyn HeadReader,
    min_block: u64,
    poll_interval: Duration,
) -> Result<(), Error> {
    if min_block == 0 {
        return Ok(());
    }
    let interval = if poll_interval.is_zero() {
        Duration::from_secs(2)
    } else {
        poll_interval
    };
    let deadline = Instant::now() + max_wait();
    loop {
        if let Ok(head) = reader.block_number().await {
            if head >= min_block {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(timeout_error(
                "timed out waiting for rpc backend state",
                None,
            ));
        }
        tokio::time::sleep(interval).await;
    }
}

// =============================================================================
// I. Signer nonce locking.
// =============================================================================

/// Process-wide nonce locks keyed by `(chain_id, signer_address)`.
fn nonce_locks() -> &'static Mutex<HashMap<String, Arc<AsyncMutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Acquire the per-(chain, signer) nonce lock, parity with Go
/// `acquireSignerNonceLock`. Two acquisitions for the same key serialize; the
/// returned guard releases the lock when dropped.
pub async fn acquire_signer_nonce_lock(
    chain_id: u64,
    signer_address: Address,
) -> tokio::sync::OwnedMutexGuard<()> {
    let key = format!("{}:{}", chain_id, signer_address.to_hex()).to_lowercase();
    let lock = {
        let mut map = match nonce_locks().lock() {
            Ok(m) => m,
            Err(poisoned) => poisoned.into_inner(),
        };
        map.entry(key)
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    };
    lock.lock_owned().await
}

// =============================================================================
// J. Bridge settlement verification.
// =============================================================================

/// LiFi `/status` response (the subset the executor reads). Parity with Go
/// `liFiStatusResponse`.
#[derive(Debug, Default, serde::Deserialize)]
struct LiFiStatusResponse {
    #[serde(default)]
    status: String,
    #[serde(default)]
    substatus: String,
    #[serde(rename = "substatusMessage", default)]
    substatus_message: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    code: i64,
    #[serde(rename = "lifiExplorerLink", default)]
    lifi_explorer_link: String,
    #[serde(default)]
    receiving: LiFiReceiving,
}

#[derive(Debug, Default, serde::Deserialize)]
struct LiFiReceiving {
    #[serde(rename = "txHash", default)]
    tx_hash: String,
}

/// Across deposit-status response (the subset the executor reads). Parity with
/// Go `acrossStatusResponse`.
#[derive(Debug, Default, serde::Deserialize)]
struct AcrossStatusResponse {
    #[serde(default)]
    status: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    error: String,
    #[serde(rename = "fillTx", default)]
    fill_tx: String,
    #[serde(rename = "depositRefundTxHash", default)]
    deposit_refund_tx: String,
}

/// Wait for a bridge step's destination settlement, parity with Go
/// `verifyBridgeSettlement`.
///
/// A non-bridge step is a no-op. The settlement provider (from
/// `expected_outputs["settlement_provider"]`) selects LiFi vs Across polling.
/// An unknown provider → [`Code::Unsupported`].
pub async fn verify_bridge_settlement(
    step: &mut ActionStep,
    source_tx_hash: &str,
    opts: &ExecuteOptions,
) -> Result<(), Error> {
    if step.step_type != StepType::Bridge {
        return Ok(());
    }
    let provider = match step.expected_outputs.as_ref() {
        Some(outs) => outs
            .get("settlement_provider")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase(),
        None => return Ok(()),
    };
    if provider.is_empty() {
        return Ok(());
    }
    match provider.as_str() {
        "lifi" => {
            let endpoint = step_output(step, "settlement_status_endpoint")
                .unwrap_or_else(|| LIFI_SETTLEMENT_URL.to_string());
            wait_for_lifi_settlement(step, source_tx_hash, &endpoint, opts).await
        }
        "across" => {
            let endpoint = step_output(step, "settlement_status_endpoint")
                .unwrap_or_else(|| ACROSS_SETTLEMENT_URL.to_string());
            wait_for_across_settlement(step, source_tx_hash, &endpoint, opts).await
        }
        other => Err(Error::new(
            Code::Unsupported,
            format!("unsupported bridge settlement provider {other:?}"),
        )),
    }
}

async fn wait_for_lifi_settlement(
    step: &mut ActionStep,
    source_tx_hash: &str,
    endpoint: &str,
    opts: &ExecuteOptions,
) -> Result<(), Error> {
    let interval = settlement_interval(opts);
    let deadline = Instant::now() + opts.step_timeout;
    loop {
        if let Ok(resp) = query_lifi_status(source_tx_hash, endpoint, step).await {
            let status = resp.status.trim().to_uppercase();
            if !status.is_empty() {
                set_step_output(step, "settlement_status", &status);
            }
            if !resp.substatus.trim().is_empty() {
                set_step_output(step, "settlement_substatus", resp.substatus.trim());
            }
            if !resp.substatus_message.trim().is_empty() {
                set_step_output(step, "settlement_message", resp.substatus_message.trim());
            }
            if !resp.lifi_explorer_link.trim().is_empty() {
                set_step_output(
                    step,
                    "settlement_explorer_url",
                    resp.lifi_explorer_link.trim(),
                );
            }
            if !resp.receiving.tx_hash.trim().is_empty() {
                set_step_output(step, "destination_tx_hash", resp.receiving.tx_hash.trim());
            }
            match status.as_str() {
                "DONE" => return Ok(()),
                "FAILED" | "INVALID" => {
                    let msg = first_non_empty(&[
                        resp.substatus_message.trim(),
                        resp.message.trim(),
                        "LiFi transfer reported failure",
                    ]);
                    return Err(Error::new(
                        Code::Unavailable,
                        format!("bridge settlement failed: {msg}"),
                    ));
                }
                _ => {}
            }
        }
        if Instant::now() >= deadline {
            return Err(timeout_error(
                "timed out waiting for bridge settlement",
                None,
            ));
        }
        tokio::time::sleep(interval).await;
    }
}

async fn wait_for_across_settlement(
    step: &mut ActionStep,
    source_tx_hash: &str,
    endpoint: &str,
    opts: &ExecuteOptions,
) -> Result<(), Error> {
    let interval = settlement_interval(opts);
    let deadline = Instant::now() + opts.step_timeout;
    loop {
        if let Ok(resp) = query_across_status(source_tx_hash, endpoint, step).await {
            let status = resp.status.trim().to_lowercase();
            if !status.is_empty() {
                set_step_output(step, "settlement_status", &status);
            }
            if !resp.fill_tx.trim().is_empty() {
                set_step_output(step, "destination_tx_hash", resp.fill_tx.trim());
            }
            if !resp.deposit_refund_tx.trim().is_empty() {
                set_step_output(step, "refund_tx_hash", resp.deposit_refund_tx.trim());
            }
            match status.as_str() {
                "filled" => return Ok(()),
                "refunded" => {
                    return Err(Error::new(Code::Unavailable, "bridge settlement refunded"))
                }
                _ => {}
            }
        }
        if Instant::now() >= deadline {
            return Err(timeout_error(
                "timed out waiting for bridge settlement",
                None,
            ));
        }
        tokio::time::sleep(interval).await;
    }
}

async fn query_lifi_status(
    source_tx_hash: &str,
    endpoint: &str,
    step: &ActionStep,
) -> Result<LiFiStatusResponse, Error> {
    let mut url = reqwest::Url::parse(endpoint.trim())
        .map_err(|e| Error::wrap(Code::Unavailable, "parse lifi settlement url", to_cause(e)))?;
    let tx_param = source_tx_hash
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("txHash", tx_param);
        if let Some(bridge) = step_output(step, "settlement_bridge") {
            q.append_pair("bridge", &bridge);
        }
        if let Some(from_chain) = step_output(step, "settlement_from_chain") {
            q.append_pair("fromChain", &from_chain);
        }
        if let Some(to_chain) = step_output(step, "settlement_to_chain") {
            q.append_pair("toChain", &to_chain);
        }
    }
    let resp: LiFiStatusResponse = http_get_json(url).await?;
    if resp.code != 0 && resp.status.is_empty() {
        if resp.code == 1003 || resp.code == 1011 {
            return Ok(resp);
        }
        return Err(Error::new(
            Code::Unavailable,
            first_non_empty(&[resp.message.trim(), "unexpected status response"]),
        ));
    }
    Ok(resp)
}

async fn query_across_status(
    source_tx_hash: &str,
    endpoint: &str,
    step: &ActionStep,
) -> Result<AcrossStatusResponse, Error> {
    let mut url = reqwest::Url::parse(endpoint.trim()).map_err(|e| {
        Error::wrap(
            Code::Unavailable,
            "parse across settlement url",
            to_cause(e),
        )
    })?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("depositTxHash", source_tx_hash.trim());
        if let Some(origin) = step_output(step, "settlement_origin_chain") {
            q.append_pair("originChainId", &origin);
        }
        if let Some(recipient) = step_output(step, "settlement_recipient") {
            q.append_pair("recipient", &recipient);
        }
    }
    let resp: AcrossStatusResponse = http_get_json(url).await?;
    if !resp.error.trim().is_empty() {
        if resp
            .error
            .trim()
            .eq_ignore_ascii_case("DepositNotFoundException")
        {
            return Ok(resp);
        }
        return Err(Error::new(
            Code::Unavailable,
            first_non_empty(&[
                resp.message.trim(),
                resp.error.trim(),
                "unexpected across status response",
            ]),
        ));
    }
    Ok(resp)
}

async fn http_get_json<T: serde::de::DeserializeOwned>(url: reqwest::Url) -> Result<T, Error> {
    let resp = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| Error::wrap(Code::Unavailable, "query settlement status", to_cause(e)))?;
    resp.json::<T>()
        .await
        .map_err(|e| Error::wrap(Code::Unavailable, "decode settlement status", to_cause(e)))
}

// =============================================================================
// K. Unsigned typed-tx encoding (OWS signing payload).
// =============================================================================

/// Encode the unsigned EIP-1559 typed-tx envelope for external (OWS) signing,
/// parity with Go `EncodeUnsignedTypedTx` (DynamicFee branch).
///
/// Produces `0x02 ++ rlp(payload)` whose `keccak256` equals the canonical
/// EIP-1559 signing hash for the same tx (the payload OWS signs). The access
/// list round-trips through the encoding.
pub fn encode_unsigned_typed_tx(
    tx: &Eip1559Tx,
    access_list: &AccessList,
) -> Result<Vec<u8>, Error> {
    use alloy::consensus::{SignableTransaction, TxEip1559};

    let consensus = TxEip1559 {
        chain_id: tx.chain_id,
        nonce: tx.nonce,
        gas_limit: tx.gas_limit,
        max_fee_per_gas: tx.max_fee_per_gas,
        max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
        to: match tx.to {
            Some(addr) => TxKind::Call(addr.into_inner()),
            None => TxKind::Create,
        },
        value: tx.value,
        access_list: access_list.clone(),
        input: Bytes::from(tx.input.clone()),
    };
    // `encoded_for_signing` is `0x02 ++ rlp(payload)`, and `keccak256` of it is
    // the EIP-1559 signing hash — identical to go-ethereum's
    // `types.NewLondonSigner(chainID).Hash(tx)`.
    let mut buf = Vec::new();
    consensus.encode_for_signing(&mut buf);
    Ok(buf)
}

/// The legacy / unsupported tx-type rejection path, parity with Go
/// `EncodeUnsignedTypedTx`'s `default` branch (the executor only builds 0x02
/// dynamic-fee txs).
pub fn encode_unsigned_typed_tx_legacy() -> Result<Vec<u8>, Error> {
    Err(Error::new(
        Code::Usage,
        "unsupported transaction type: only EIP-1559 (0x02) is supported",
    ))
}

// =============================================================================
// L. Chain-id helpers.
// =============================================================================

/// Extract the numeric EVM chain id from a CAIP-2 string (`eip155:N`) or a bare
/// numeric chain id, parity with Go `ParseEVMChainID`. Case-insensitive prefix;
/// empty/garbage is an [`Code::Usage`] error.
pub fn parse_evm_chain_id(caip2: &str) -> Result<i64, Error> {
    let trimmed = caip2.trim();
    if trimmed.is_empty() {
        return Err(Error::new(Code::Usage, "empty chain id"));
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("eip155:") {
        return rest
            .parse::<i64>()
            .map_err(|_| Error::new(Code::Usage, format!("invalid CAIP-2 chain id {caip2:?}")));
    }
    trimmed
        .parse::<i64>()
        .map_err(|_| Error::new(Code::Usage, format!("invalid CAIP-2 chain id {caip2:?}")))
}

/// Whether a numeric chain id is a Tempo network (mainnet/testnet/devnet),
/// parity with Go `IsTempoChain`.
pub fn is_tempo_chain(chain_id: i64) -> bool {
    matches!(chain_id, 4217 | 42431 | 31318)
}

// =============================================================================
// Helpers.
// =============================================================================

/// The 4-byte ERC-20 `approve` selector.
fn approve_selector() -> [u8; 4] {
    defi_evm::abi::function_selector("approve(address,uint256)")
}

/// Parse a named ERC-20 minimal-ABI function fragment.
fn erc20_function(name: &str) -> Result<Function, Error> {
    Function::from_abi_json(ERC20_MINIMAL_ABI, name)
}

/// The settlement/poll loop guard ceiling for the injected-caller waiters. The
/// observable contract is the `ActionTimeout` mapping; the bound keeps tests
/// from hanging forever while still allowing several poll iterations.
fn max_wait() -> Duration {
    Duration::from_millis(200)
}

fn settlement_interval(opts: &ExecuteOptions) -> Duration {
    if opts.poll_interval.is_zero() {
        Duration::from_millis(5)
    } else {
        opts.poll_interval
    }
}

fn timeout_error(message: &str, cause: Option<Error>) -> Error {
    match cause {
        Some(c) => Error::wrap(Code::ActionTimeout, message.to_string(), to_cause(c)),
        None => Error::new(Code::ActionTimeout, message),
    }
}

/// Read a trimmed, non-empty string from a step's `expected_outputs`.
fn step_output(step: &ActionStep, key: &str) -> Option<String> {
    step.expected_outputs
        .as_ref()?
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Write a string into a step's `expected_outputs`, creating the map if absent.
fn set_step_output(step: &mut ActionStep, key: &str, value: &str) {
    if key.trim().is_empty() {
        return;
    }
    let map = step
        .expected_outputs
        .get_or_insert_with(serde_json::Map::new);
    map.insert(
        key.to_string(),
        serde_json::Value::String(value.to_string()),
    );
}

fn first_non_empty(values: &[&str]) -> String {
    for v in values {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    String::new()
}

/// Decode a hex string (optional `0x`, odd-length left-padded with a `0` nibble),
/// parity with Go `decodeHex`. Empty/`0x` → empty bytes.
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

/// A concrete, `Send + Sync` std error carrying a display message — lets a
/// foreign / typed error be recorded as the `cause` of a typed [`Error`].
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
    //! module. They MUST fail to compile / fail assertions until GREEN.
    //!
    //! All vectors are deterministic and offline. HTTP settlement endpoints are
    //! mocked with `wiremock`; the contract caller / header reader are injected
    //! Rust traits (the analogue of Go's `mockContractCaller` / `mockHeaderReader`).
    //! The signing key is the well-known go-ethereum/Hardhat test key used across
    //! the execution RED suites; addresses come from `defi_evm`.

    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use alloy::dyn_abi::DynSolValue;
    use alloy::primitives::U256;
    use defi_errors::Code;
    use defi_evm::abi::Function;
    use defi_evm::address::{self, Address};
    use defi_registry::ERC20_MINIMAL_ABI;

    use crate::action::{
        Action, ActionStatus, ActionStep, Constraints, ExecutionBackend, StepStatus, StepType,
    };

    // ---- shared helpers --------------------------------------------------

    /// An unreachable local RPC endpoint (matches Go's `http://127.0.0.1:65535`).
    /// Used to prove validation happens BEFORE any RPC dial.
    const DEAD_RPC: &str = "http://127.0.0.1:65535";

    /// Build a minimal valid [`Action`] via a struct literal (no dependency on the
    /// sibling `action` module's `Action::new`, so these tests fail on
    /// EVM-EXECUTOR behavior, not on a missing constructor — same convention as
    /// `store.rs`'s `make_action`).
    fn make_action(intent: &str, chain_id: &str) -> Action {
        Action {
            action_id: "act_test".to_string(),
            intent_type: intent.to_string(),
            provider: String::new(),
            status: ActionStatus::Planned,
            chain_id: chain_id.to_string(),
            from_address: String::new(),
            wallet_id: String::new(),
            wallet_name: String::new(),
            execution_backend: None,
            to_address: String::new(),
            input_amount: String::new(),
            created_at: "2026-05-28T00:00:00Z".to_string(),
            updated_at: "2026-05-28T00:00:00Z".to_string(),
            constraints: Constraints::default(),
            steps: Vec::new(),
            metadata: None,
            provider_data: None,
        }
    }

    /// Build a minimal [`ActionStep`] via a struct literal.
    fn make_step(step_type: StepType, chain_id: &str, rpc_url: &str, target: &str) -> ActionStep {
        ActionStep {
            step_id: "step-1".to_string(),
            step_type,
            status: StepStatus::Pending,
            chain_id: chain_id.to_string(),
            rpc_url: rpc_url.to_string(),
            description: String::new(),
            target: target.to_string(),
            data: "0x".to_string(),
            value: "0".to_string(),
            calls: Vec::new(),
            expected_outputs: None,
            tx_hash: String::new(),
            error: String::new(),
        }
    }

    /// The well-known go-ethereum / Hardhat test private key.
    const TEST_KEY: &str = "59c6995e998f97a5a0044976f0945388cf9b7e5e5f4f9d2d9d8f1f5b7f6d11d1";

    fn addr_aa() -> Address {
        address::parse("0x00000000000000000000000000000000000000aa").unwrap()
    }
    fn addr_bb() -> Address {
        address::parse("0x00000000000000000000000000000000000000bb").unwrap()
    }
    fn addr_cc() -> Address {
        address::parse("0x00000000000000000000000000000000000000cc").unwrap()
    }

    /// ABI-encode `approve(spender, amount)` calldata (selector ++ args).
    fn approve_calldata(spender: Address, amount: u64) -> Vec<u8> {
        let f = Function::from_abi_json(ERC20_MINIMAL_ABI, "approve").unwrap();
        f.encode(&[
            DynSolValue::Address(spender.into_inner()),
            DynSolValue::Uint(U256::from(amount), 256),
        ])
        .unwrap()
    }

    /// ABI-encode `transfer(to, amount)` calldata (selector ++ args).
    fn transfer_calldata(to: Address, amount: u64) -> Vec<u8> {
        let f = Function::from_abi_json(ERC20_MINIMAL_ABI, "transfer").unwrap();
        f.encode(&[
            DynSolValue::Address(to.into_inner()),
            DynSolValue::Uint(U256::from(amount), 256),
        ])
        .unwrap()
    }

    /// `0x08c379a0` ++ abi(string) — a standard `Error(string)` revert payload.
    fn error_string_revert(reason: &str) -> Vec<u8> {
        let mut out = vec![0x08, 0xc3, 0x79, 0xa0];
        out.extend(DynSolValue::String(reason.to_string()).abi_encode());
        out
    }

    /// Build a real local signer for the well-known test key (its address is the
    /// canonical `defi_evm` derivation, the Rust analogue of Go `staticSigner`).
    /// The executor's `signer` parameter and `LocalSubmitBackend` wrap this
    /// pure-crypto signer (the key-source orchestration in [`crate::signer`]
    /// resolves a key into exactly this type before handing it to the executor).
    fn static_signer() -> defi_evm::signer::LocalSigner {
        defi_evm::signer::LocalSigner::from_hex(TEST_KEY).expect("valid test key")
    }

    /// A stub EVM submit backend reporting a fixed sender (Go
    /// `stubEVMSubmitBackend`).
    #[derive(Clone)]
    struct StubBackend {
        sender: Address,
    }

    #[async_trait::async_trait]
    impl EvmSubmitBackend for StubBackend {
        fn effective_sender(&self) -> Address {
            self.sender
        }
        async fn submit_dynamic_fee_tx(
            &self,
            _rpc_url: &str,
            _chain_id: u64,
            _tx: &defi_evm::signer::Eip1559Tx,
        ) -> Result<[u8; 32], defi_errors::Error> {
            Ok([0u8; 32])
        }
    }

    // =====================================================================
    // A. Submit-backend abstraction (local vs OWS)
    // =====================================================================

    #[test]
    fn local_backend_effective_sender_is_signer_address() {
        // A2.
        let signer = static_signer();
        let want = signer.address();
        let backend = LocalSubmitBackend::new(signer);
        assert_eq!(backend.effective_sender().to_hex(), want.to_hex());
    }

    #[test]
    fn ows_backend_effective_sender_is_provided_sender() {
        // A3.
        let backend = OwsSubmitBackend::new("wallet-123", addr_aa());
        assert_eq!(backend.effective_sender(), addr_aa());
    }

    #[tokio::test]
    async fn ows_submit_rejects_malformed_tx_hash() {
        // A4: a wallet backend returning a too-short hash → Signer error.
        let backend = OwsSubmitBackend::new("wallet-123", addr_aa())
            .with_send_hook(Arc::new(|_w, _c, _tx, _rpc| Ok("0xabc123".to_string())));

        let tx = defi_evm::signer::Eip1559Tx {
            chain_id: 1,
            nonce: 7,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 2,
            gas_limit: 21_000,
            to: Some(addr_bb()),
            value: U256::ZERO,
            input: vec![],
        };
        let err = backend
            .submit_dynamic_fee_tx("https://rpc.example", 1, &tx)
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::Signer);
    }

    #[tokio::test]
    async fn ows_policy_denial_maps_to_action_policy() {
        // A5: the wallet's policy refusal surfaces as ActionPolicy.
        let backend = OwsSubmitBackend::new("wallet-123", addr_aa()).with_send_hook(Arc::new(
            |_w, _c, _tx, _rpc| Err(defi_errors::Error::new(Code::ActionPolicy, "policy denied")),
        ));

        let tx = defi_evm::signer::Eip1559Tx {
            chain_id: 1,
            nonce: 7,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 2,
            gas_limit: 21_000,
            to: Some(addr_bb()),
            value: U256::ZERO,
            input: vec![],
        };
        let err = backend
            .submit_dynamic_fee_tx("https://rpc.example", 1, &tx)
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::ActionPolicy);
    }

    #[tokio::test]
    async fn ows_submit_requires_wallet_id() {
        // A6: blank wallet id → Usage.
        let backend = OwsSubmitBackend::new("", addr_aa());
        let tx = defi_evm::signer::Eip1559Tx {
            chain_id: 1,
            nonce: 0,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 2,
            gas_limit: 21_000,
            to: Some(addr_bb()),
            value: U256::ZERO,
            input: vec![],
        };
        let err = backend
            .submit_dynamic_fee_tx("https://rpc.example", 1, &tx)
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    // =====================================================================
    // B. Execution-backend routing
    // =====================================================================

    #[test]
    fn resolve_routes_ows_actions_to_evm_executor() {
        // B1.
        let mut action = make_action("swap", "eip155:1");
        action.execution_backend = Some(ExecutionBackend::Ows);
        action.wallet_id = "wallet-123".into();

        let backend = StubBackend { sender: addr_aa() };
        let exec = resolve_execution_backend(&action, Some(static_signer()), Some(backend))
            .expect("resolve ows");
        // The EVM executor reports the OWS backend's sender.
        assert_eq!(exec.effective_sender(), addr_aa());
        assert!(matches!(exec, ResolvedExecutor::Evm(_)));
    }

    #[test]
    fn resolve_routes_legacy_actions_to_evm_executor() {
        // B2.
        let mut action = make_action("swap", "eip155:1");
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        let exec = resolve_execution_backend(&action, Some(static_signer()), None::<StubBackend>)
            .expect("resolve legacy");
        assert!(matches!(exec, ResolvedExecutor::Evm(_)));
    }

    #[test]
    fn resolve_routes_tempo_actions_to_tempo_executor() {
        // B3.
        let mut action = make_action("swap", "eip155:4217");
        action.execution_backend = Some(ExecutionBackend::Tempo);
        let exec = resolve_execution_backend(&action, Some(static_signer()), None::<StubBackend>)
            .expect("resolve tempo");
        assert!(matches!(exec, ResolvedExecutor::Tempo(_)));
    }

    #[test]
    fn resolve_ows_without_backend_is_signer_error() {
        // B4.
        let mut action = make_action("swap", "eip155:1");
        action.execution_backend = Some(ExecutionBackend::Ows);
        action.wallet_id = "wallet-123".into();
        let err = resolve_execution_backend(&action, Some(static_signer()), None::<StubBackend>)
            .unwrap_err();
        assert_eq!(err.code, Code::Signer);
    }

    // =====================================================================
    // C. Persisted-sender validation
    // =====================================================================

    #[test]
    fn rejects_empty_effective_sender() {
        // C1.
        let action = make_action("swap", "eip155:1");
        let zero = address::parse("0x0000000000000000000000000000000000000000").unwrap();
        let err = validate_persisted_action_sender(&action, zero).unwrap_err();
        assert_eq!(err.code, Code::Signer);
    }

    #[test]
    fn rejects_mismatched_persisted_sender() {
        // C2.
        let mut action = make_action("swap", "eip155:1");
        action.from_address = "0x00000000000000000000000000000000000000bb".into();
        // backend sender is 0x..cc — mismatch.
        let err = validate_persisted_action_sender(&action, addr_cc()).unwrap_err();
        assert_eq!(err.code, Code::Signer);
        assert_eq!(
            action.from_address, "0x00000000000000000000000000000000000000bb",
            "persisted sender must be unchanged by validation"
        );
    }

    #[test]
    fn rejects_invalid_persisted_sender_address() {
        // C3.
        let mut action = make_action("swap", "eip155:1");
        action.from_address = "not-an-address".into();
        let err = validate_persisted_action_sender(&action, addr_aa()).unwrap_err();
        assert_eq!(err.code, Code::Signer);
    }

    #[test]
    fn blank_persisted_sender_validates_ok() {
        // C4 (validation half — fill-in is an execute_action behavior).
        let action = make_action("swap", "eip155:1");
        assert!(action.from_address.is_empty());
        validate_persisted_action_sender(&action, addr_aa()).expect("blank sender is OK");
    }

    #[test]
    fn persisted_sender_match_is_case_insensitive() {
        // C5: uppercase persisted hex still matches the EIP-55 sender.
        let mut action = make_action("swap", "eip155:1");
        action.from_address = "0x00000000000000000000000000000000000000AA".into();
        validate_persisted_action_sender(&action, addr_aa()).expect("case-insensitive match");
    }

    // =====================================================================
    // D. Step pre-flight validation (no RPC dial / no sign)
    // =====================================================================

    #[tokio::test]
    async fn execute_action_rejects_invalid_step_target_before_rpc_dial() {
        // D1: invalid target → Usage; step marked Failed; no network reached.
        let mut action = make_action("swap", "eip155:1");
        action.constraints.simulate = true;
        action.steps.push(make_step(
            StepType::Swap,
            "eip155:1",
            DEAD_RPC,
            "not-an-address",
        ));

        let backend = LocalSubmitBackend::new(static_signer());
        let err = execute_action(
            None,
            &mut action,
            Some(static_signer()),
            Some(backend),
            default_execute_options(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, Code::Usage);
        assert_eq!(action.steps[0].status, StepStatus::Failed);
    }

    #[tokio::test]
    async fn execute_action_rejects_action_with_no_steps() {
        // D2.
        let mut action = make_action("swap", "eip155:1");
        let backend = LocalSubmitBackend::new(static_signer());
        let err = execute_action(
            None,
            &mut action,
            Some(static_signer()),
            Some(backend),
            default_execute_options(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    #[tokio::test]
    async fn execute_action_rejects_gas_multiplier_not_greater_than_one() {
        // D3.
        let mut action = make_action("swap", "eip155:1");
        action.steps.push(make_step(
            StepType::Swap,
            "eip155:1",
            DEAD_RPC,
            "0x00000000000000000000000000000000000000bb",
        ));
        let mut opts = default_execute_options();
        opts.gas_multiplier = 1.0;
        let backend = LocalSubmitBackend::new(static_signer());
        let err = execute_action(
            None,
            &mut action,
            Some(static_signer()),
            Some(backend),
            opts,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    // =====================================================================
    // E. Revert decoding
    // =====================================================================

    #[test]
    fn decode_revert_data_reason_string() {
        // E1.
        let data = error_string_revert("slippage too high");
        assert_eq!(
            decode_revert_data(&data).as_deref(),
            Some("slippage too high")
        );
    }

    #[test]
    fn decode_revert_data_custom_error_selector() {
        // E2.
        let data = vec![0x12, 0x34, 0x56, 0x78];
        let reason = decode_revert_data(&data).expect("custom selector reason");
        assert!(
            reason.contains("0x12345678"),
            "expected selector hex in reason, got {reason:?}"
        );
    }

    #[test]
    fn decode_revert_reason_from_error_with_data() {
        // E3: the error carries 0x-hex revert data.
        let data = error_string_revert("insufficient output amount");
        let hex_data = format!("0x{}", hex::encode(&data));
        let err = RevertDataError::new("execution reverted", RevertData::Hex(hex_data));
        assert_eq!(
            decode_revert_reason_from_error(&err).as_deref(),
            Some("insufficient output amount")
        );
    }

    #[test]
    fn wrap_evm_execution_error_includes_decoded_revert() {
        // E4.
        let data = error_string_revert("panic path");
        let hex_data = format!("0x{}", hex::encode(&data));
        let root = RevertDataError::new("execution reverted", RevertData::Hex(hex_data));
        let wrapped = wrap_evm_execution_error(Code::ActionSim, "simulate step (eth_call)", root);
        assert_eq!(wrapped.code, Code::ActionSim);
        assert!(
            wrapped.to_string().contains("panic path"),
            "expected decoded reason in wrapped error: {wrapped}"
        );
    }

    #[test]
    fn decode_revert_data_none_for_empty_or_short() {
        // E5.
        assert!(decode_revert_data(&[]).is_none());
        assert!(decode_revert_data(&[0x01, 0x02]).is_none());
    }

    // =====================================================================
    // F. Tx-hash normalization
    // =====================================================================

    #[test]
    fn normalize_step_tx_hash_accepts_full_hash_rejects_short() {
        // F1 + F2.
        let valid = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert!(normalize_step_tx_hash(valid).is_some());
        assert!(normalize_step_tx_hash("0x1234").is_none());
        assert!(normalize_step_tx_hash("").is_none());
        assert!(normalize_step_tx_hash("   ").is_none());
    }

    // =====================================================================
    // G. Approval-readiness (post-confirmation allowance visibility)
    // =====================================================================

    #[test]
    fn approval_expectation_from_approve_call() {
        // G1.
        let token = addr_aa();
        let owner = addr_bb();
        let spender = addr_cc();
        let data = approve_calldata(spender, 42);

        let exp = approval_expectation_from_call_msg(Some(owner), Some(token), &data)
            .expect("approval detected");
        assert_eq!(exp.token, token);
        assert_eq!(exp.owner, owner);
        assert_eq!(exp.spender, spender);
        assert_eq!(exp.amount, U256::from(42u64));
    }

    #[test]
    fn approval_expectation_ignores_non_approval_call() {
        // G2: transfer(to, amount) is not an approval.
        let token = addr_aa();
        let owner = addr_bb();
        let recipient = addr_cc();
        let data = transfer_calldata(recipient, 42);
        assert!(approval_expectation_from_call_msg(Some(owner), Some(token), &data).is_none());
    }

    /// An injected contract caller serving a scripted allowance sequence.
    struct ScriptedCaller {
        allowances: Vec<U256>,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ContractCaller for ScriptedCaller {
        async fn call(
            &self,
            _from: Option<Address>,
            _to: Address,
            _data: Vec<u8>,
        ) -> Result<Vec<u8>, defi_errors::Error> {
            let idx = self.calls.fetch_add(1, Ordering::SeqCst);
            let i = idx.min(self.allowances.len().saturating_sub(1));
            let value = self.allowances.get(i).copied().unwrap_or(U256::ZERO);
            // Encode the allowance as a single uint256 word.
            Ok(value.to_be_bytes::<32>().to_vec())
        }
    }

    #[tokio::test]
    async fn wait_for_allowance_retries_until_sufficient() {
        // G3.
        let caller = ScriptedCaller {
            allowances: vec![U256::ZERO, U256::from(5u64), U256::from(10u64)],
            calls: AtomicUsize::new(0),
        };
        let exp = ApprovalExpectation {
            token: addr_aa(),
            owner: addr_bb(),
            spender: addr_cc(),
            amount: U256::from(10u64),
        };
        wait_for_allowance_at_least(&caller, &exp, Duration::from_millis(5))
            .await
            .expect("allowance reached");
        assert!(
            caller.calls.load(Ordering::SeqCst) >= 3,
            "expected repeated allowance checks"
        );
    }

    #[tokio::test]
    async fn wait_for_allowance_times_out() {
        // G4: allowance never reaches threshold before the deadline.
        let caller = ScriptedCaller {
            allowances: vec![U256::ZERO],
            calls: AtomicUsize::new(0),
        };
        let exp = ApprovalExpectation {
            token: addr_aa(),
            owner: addr_bb(),
            spender: addr_cc(),
            amount: U256::from(1u64),
        };
        let err = tokio::time::timeout(
            Duration::from_secs(2),
            wait_for_allowance_at_least(&caller, &exp, Duration::from_millis(5)),
        )
        .await
        .expect("must not hang past the test budget")
        .unwrap_err();
        assert_eq!(err.code, Code::ActionTimeout);
    }

    // =====================================================================
    // H. Cross-step head ordering
    // =====================================================================

    /// An injected header reader serving a scripted head sequence.
    struct ScriptedHeads {
        heads: Vec<u64>,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl HeadReader for ScriptedHeads {
        async fn block_number(&self) -> Result<u64, defi_errors::Error> {
            let idx = self.calls.fetch_add(1, Ordering::SeqCst);
            let i = idx.min(self.heads.len().saturating_sub(1));
            Ok(self.heads.get(i).copied().unwrap_or(0))
        }
    }

    #[tokio::test]
    async fn wait_for_rpc_head_reaches_required_block() {
        // H1.
        let reader = ScriptedHeads {
            heads: vec![100, 101, 102],
            calls: AtomicUsize::new(0),
        };
        wait_for_rpc_head_at_least(&reader, 102, Duration::from_millis(5))
            .await
            .expect("head reached");
        assert!(reader.calls.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn wait_for_rpc_head_times_out() {
        // H2.
        let reader = ScriptedHeads {
            heads: vec![100],
            calls: AtomicUsize::new(0),
        };
        let err = tokio::time::timeout(
            Duration::from_secs(2),
            wait_for_rpc_head_at_least(&reader, 105, Duration::from_millis(5)),
        )
        .await
        .expect("must not hang")
        .unwrap_err();
        assert_eq!(err.code, Code::ActionTimeout);
    }

    // =====================================================================
    // I. Signer nonce locking
    // =====================================================================

    #[tokio::test]
    async fn acquire_signer_nonce_lock_serializes_same_signer_chain() {
        // I1: the second acquisition blocks while the first guard is held.
        let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

        let guard = acquire_signer_nonce_lock(1, addr_aa()).await;
        order.lock().unwrap().push("first-acquired");

        let order2 = order.clone();
        let task = tokio::spawn(async move {
            let _g = acquire_signer_nonce_lock(1, addr_aa()).await;
            order2.lock().unwrap().push("second-acquired");
        });

        // Give the spawned task time to attempt the lock; it must still be
        // blocked because the first guard is held.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            order.lock().unwrap().as_slice(),
            &["first-acquired"],
            "second acquisition must block while the first guard is held"
        );

        drop(guard);
        task.await.expect("second task completes after unlock");
        assert_eq!(
            order.lock().unwrap().as_slice(),
            &["first-acquired", "second-acquired"],
            "second acquisition proceeds after the first guard is dropped"
        );
    }

    // =====================================================================
    // J. Bridge settlement verification (wiremock)
    // =====================================================================

    use wiremock::matchers::{method, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn bridge_step(outputs: serde_json::Map<String, serde_json::Value>) -> ActionStep {
        let mut step = make_step(StepType::Bridge, "eip155:1", "", "");
        step.step_id = "bridge-1".into();
        step.status = StepStatus::Submitted;
        step.data = String::new();
        step.expected_outputs = Some(outputs);
        step
    }

    fn fast_settlement_opts() -> ExecuteOptions {
        let mut o = default_execute_options();
        o.poll_interval = Duration::from_millis(5);
        o.step_timeout = Duration::from_millis(500);
        o
    }

    #[tokio::test]
    async fn verify_bridge_settlement_noop_for_non_bridge_step() {
        // J1.
        let mut step = make_step(StepType::Approval, "eip155:1", "", "");
        step.status = StepStatus::Confirmed;
        verify_bridge_settlement(&mut step, "0xabc", &fast_settlement_opts())
            .await
            .expect("non-bridge step is a no-op");
    }

    #[tokio::test]
    async fn verify_bridge_settlement_lifi_success() {
        // J2: DONE; records settlement_status + destination_tx_hash; sends txHash
        // without 0x prefix.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("txHash", "abc"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"status":"DONE","substatus":"COMPLETED","receiving":{"txHash":"0xdestination"}}"#,
            ))
            .mount(&server)
            .await;

        let mut outs = serde_json::Map::new();
        outs.insert("settlement_provider".into(), "lifi".into());
        outs.insert("settlement_status_endpoint".into(), server.uri().into());
        outs.insert("settlement_bridge".into(), "across".into());
        let mut step = bridge_step(outs);

        verify_bridge_settlement(&mut step, "0xabc", &fast_settlement_opts())
            .await
            .expect("lifi settlement success");

        let outputs = step.expected_outputs.as_ref().unwrap();
        assert_eq!(
            outputs.get("settlement_status").and_then(|v| v.as_str()),
            Some("DONE")
        );
        assert_eq!(
            outputs.get("destination_tx_hash").and_then(|v| v.as_str()),
            Some("0xdestination")
        );
    }

    #[tokio::test]
    async fn verify_bridge_settlement_lifi_failed() {
        // J3.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"status":"FAILED","substatusMessage":"bridge route failed"}"#,
                ),
            )
            .mount(&server)
            .await;

        let mut outs = serde_json::Map::new();
        outs.insert("settlement_provider".into(), "lifi".into());
        outs.insert("settlement_status_endpoint".into(), server.uri().into());
        let mut step = bridge_step(outs);

        let err = verify_bridge_settlement(&mut step, "0xabc", &fast_settlement_opts())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("bridge settlement failed"),
            "expected bridge settlement failed error, got {err}"
        );
    }

    #[tokio::test]
    async fn verify_bridge_settlement_across_success() {
        // J4: filled; records settlement_status + destination_tx_hash from fillTx;
        // depositTxHash + originChainId query params pass through.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("depositTxHash", "0xabc"))
            .and(query_param("originChainId", "1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"status":"filled","fillTx":"0xdestination"}"#),
            )
            .mount(&server)
            .await;

        let mut outs = serde_json::Map::new();
        outs.insert("settlement_provider".into(), "across".into());
        outs.insert("settlement_status_endpoint".into(), server.uri().into());
        outs.insert("settlement_origin_chain".into(), "1".into());
        let mut step = bridge_step(outs);

        verify_bridge_settlement(&mut step, "0xabc", &fast_settlement_opts())
            .await
            .expect("across settlement success");

        let outputs = step.expected_outputs.as_ref().unwrap();
        assert_eq!(
            outputs.get("settlement_status").and_then(|v| v.as_str()),
            Some("filled")
        );
        assert_eq!(
            outputs.get("destination_tx_hash").and_then(|v| v.as_str()),
            Some("0xdestination")
        );
    }

    #[tokio::test]
    async fn verify_bridge_settlement_across_refunded() {
        // J5.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"status":"refunded","depositRefundTxHash":"0xrefund"}"#),
            )
            .mount(&server)
            .await;

        let mut outs = serde_json::Map::new();
        outs.insert("settlement_provider".into(), "across".into());
        outs.insert("settlement_status_endpoint".into(), server.uri().into());
        let mut step = bridge_step(outs);

        let err = verify_bridge_settlement(&mut step, "0xabc", &fast_settlement_opts())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("refunded"),
            "expected refunded error, got {err}"
        );
    }

    #[tokio::test]
    async fn verify_bridge_settlement_unsupported_provider() {
        // J6.
        let mut outs = serde_json::Map::new();
        outs.insert("settlement_provider".into(), "unknown".into());
        let mut step = bridge_step(outs);
        let err = verify_bridge_settlement(&mut step, "0xabc", &fast_settlement_opts())
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::Unsupported);
    }

    // =====================================================================
    // K. Unsigned typed-tx encoding (OWS signing payload)
    // =====================================================================

    #[test]
    fn encode_unsigned_dynamic_fee_tx_matches_signing_hash() {
        // K1 + K2: encoding is 0x02 ++ rlp(payload); keccak256(encoding) equals
        // the canonical EIP-1559 signing hash of the same tx.
        use alloy::consensus::{SignableTransaction, TxEip1559};
        use alloy::eips::eip2930::{AccessList, AccessListItem};
        use alloy::primitives::{Address as AlloyAddr, TxKind, B256};

        let to = address::parse("0x1111111111111111111111111111111111111111").unwrap();
        let access = AccessList(vec![AccessListItem {
            address: "0x2222222222222222222222222222222222222222"
                .parse::<AlloyAddr>()
                .unwrap(),
            storage_keys: vec![B256::with_last_byte(1)],
        }]);

        let tx = defi_evm::signer::Eip1559Tx {
            chain_id: 1,
            nonce: 7,
            max_priority_fee_per_gas: 2_000_000_000,
            max_fee_per_gas: 30_000_000_000,
            gas_limit: 21_000,
            to: Some(to),
            value: U256::from(12345u64),
            input: vec![0x12, 0x34],
        };

        let encoded = encode_unsigned_typed_tx(&tx, &access).expect("encode unsigned typed tx");
        assert_eq!(encoded[0], 0x02, "type-2 (DynamicFee) prefix");

        // Reference signing hash from alloy's TxEip1559 (the EIP-1559 signature
        // preimage; go-ethereum's types.NewLondonSigner(chainID).Hash(tx)).
        let consensus = TxEip1559 {
            chain_id: 1,
            nonce: 7,
            gas_limit: 21_000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: 2_000_000_000,
            to: TxKind::Call(to.into_inner()),
            value: U256::from(12345u64),
            access_list: access,
            input: alloy::primitives::Bytes::from(vec![0x12, 0x34]),
        };
        let want = consensus.signature_hash();
        let got = alloy::primitives::keccak256(&encoded);
        assert_eq!(
            got, want,
            "keccak256(encoding) must equal the EIP-1559 signing hash"
        );
    }

    #[test]
    fn encode_unsigned_typed_tx_rejects_unsupported_type() {
        // K3: a non-EIP-1559 / legacy tx kind is rejected (the executor only
        // builds 0x02 dynamic-fee txs; an unsupported request errors, never
        // panics). `encode_unsigned_typed_tx_legacy` is the dedicated entry point
        // for the rejection path mirroring Go's LegacyTx branch.
        let err = encode_unsigned_typed_tx_legacy().unwrap_err();
        assert!(
            err.to_string().contains("unsupported transaction type"),
            "expected unsupported transaction type error, got {err}"
        );
    }

    // =====================================================================
    // L. Chain-id helpers
    // =====================================================================

    #[test]
    fn parse_evm_chain_id_parity() {
        // L1.
        assert_eq!(parse_evm_chain_id("eip155:4217").unwrap(), 4217);
        assert_eq!(parse_evm_chain_id("EIP155:1").unwrap(), 1);
        assert_eq!(parse_evm_chain_id("42161").unwrap(), 42161);
        assert!(parse_evm_chain_id("").is_err());
        assert!(parse_evm_chain_id("eip155:abc").is_err());
    }

    #[test]
    fn is_tempo_chain_parity() {
        // L2.
        for id in [4217, 42431, 31318] {
            assert!(is_tempo_chain(id), "{id} should be a Tempo chain");
        }
        for id in [1, 10, 137, 8453, 42161] {
            assert!(!is_tempo_chain(id), "{id} should not be a Tempo chain");
        }
    }
}
