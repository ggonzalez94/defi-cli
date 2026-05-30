//! `transfer` command group handler (Go: `internal/app/transfer_command.go` —
//! `newTransferCommand`).
//!
//! This module owns the **transfer-command-specific** glue that sits between
//! the runner's cache-flow core ([`crate::runner`]), the shared
//! execution-identity resolver, and the action-build registry
//! ([`defi_execution::builder::Registry`]). The `transfer` group is the simplest
//! standard-EVM execution command (an ERC-20 `transfer(recipient, amount)`):
//! there is no provider routing (`provider == "native"`). Specifically it owns:
//!
//! * the `transfer plan` request builder (`build_transfer_request`) — the Go
//!   `buildAction` closure: parse `--chain` + `--asset`, default a non-positive
//!   asset `decimals` to `18`, normalize the amount against those decimals
//!   (carrying base + decimal forms consistently, spec §2.4), and assemble a
//!   [`defi_execution::planner::TransferRequest`] carrying sender / recipient /
//!   simulate / rpc-url verbatim;
//! * the `transfer plan` schema identity input constraints
//!   (`transfer_plan_identity_constraints`: the standard
//!   `exactly_one_of {wallet, from_address}`, with no per-provider `when`
//!   branching — transfer planning is OWS-first / standard EVM, like bridge);
//! * the persisted-intent gate (`ensure_transfer_intent`: `transfer submit` /
//!   `transfer status` reject a non-`transfer` action with a usage error).
//!
//! NOT re-owned here (consumed from elsewhere):
//! * the transfer **action construction + validation** (recipient/sender hex
//!   validation, zero-recipient rejection, positive-amount rejection, calldata
//!   packing) — owned by `defi_execution::planner::build_transfer_action` and
//!   covered by its own RED suite (ported from `planner/transfer_test.go`);
//! * the action-build registry routing (`Registry::build_transfer_action`) —
//!   owned by `defi_execution::builder` (B8);
//! * the shared execution-identity resolver (`resolve_execution_identity`) and
//!   its OWS/legacy backend stamping — owned by the shared execution-identity
//!   module / [`crate::runner`];
//! * the submit signer/backend plumbing, pre-sign guardrails, and receipt
//!   polling — `defi-execution` concern;
//! * the cache-key construction + cache bypass for execution paths — runner
//!   concern, owned by [`crate::runner`].

#![allow(dead_code, unused_variables)]

use defi_errors::{Code, Error};
use defi_execution::planner::TransferRequest;
use defi_id::{normalize_amount, parse_asset, parse_chain};
use defi_schema::InputConstraint;

/// Build a [`TransferRequest`] from the raw `transfer plan` flags.
///
/// Parity with the Go `buildAction` closure in `transfer_command.go`:
/// 1. parse `--chain` then `--asset` on that chain (delegates to
///    `defi_id::parse_chain` / `defi_id::parse_asset`); an empty `--chain` /
///    `--asset`, or a parse failure, surfaces as the typed error from those
///    helpers (usage for the empty/invalid cases);
/// 2. default the asset `decimals` to `18` when the parsed value is
///    non-positive (`decimals <= 0`) — distinct from the planner, which does no
///    decimals defaulting;
/// 3. normalize the amount against those (defaulted) decimals via
///    `defi_id::normalize_amount`, carrying both base + decimal forms (spec
///    §2.4) — supplying both `--amount` and `--amount-decimal` is a usage error,
///    supplying neither is a usage error;
/// 4. assemble the [`TransferRequest`] carrying the resolved sender
///    (`from_address`), recipient, simulate flag, and rpc-url verbatim.
///
/// The recipient / sender hex validation, zero-recipient rejection, and
/// positive-amount enforcement are NOT performed here — they belong to
/// `defi_execution::planner::build_transfer_action`, which consumes this
/// request.
// The flag-derived inputs map 1:1 onto the Go `transferArgs` fields; this is
// the locked public signature the RED suite + callers depend on, so the
// argument count is intentional rather than a struct-grouping opportunity.
#[allow(clippy::too_many_arguments)]
pub fn build_transfer_request(
    chain_arg: &str,
    asset_arg: &str,
    amount_base: &str,
    amount_decimal: &str,
    from_address: &str,
    recipient: &str,
    simulate: bool,
    rpc_url: &str,
) -> Result<TransferRequest, Error> {
    // Parity with Go `parseChainAsset`: an empty `--chain` / `--asset` is a
    // usage error (with the matching message); otherwise delegate to the typed
    // parsers, which surface their own typed errors on parse failure.
    if chain_arg.trim().is_empty() {
        return Err(Error::new(Code::Usage, "--chain is required"));
    }
    if asset_arg.trim().is_empty() {
        return Err(Error::new(Code::Usage, "--asset is required"));
    }
    let chain = parse_chain(chain_arg)?;
    let asset = parse_asset(asset_arg, &chain)?;

    // Default a non-positive asset `decimals` to 18 (Go `buildAction`:
    // `if decimals <= 0 { decimals = 18 }`) — the planner does no defaulting.
    let mut decimals = asset.decimals;
    if decimals <= 0 {
        decimals = 18;
    }

    // Normalize against the (defaulted) decimals, carrying base + decimal forms
    // consistently (spec §2.4). Supplying both / neither amount form is a usage
    // error, surfaced by `normalize_amount`.
    let (base, _) = normalize_amount(amount_base, amount_decimal, decimals)?;

    Ok(TransferRequest {
        chain,
        asset,
        amount_base_units: base,
        sender: from_address.to_string(),
        recipient: recipient.to_string(),
        simulate,
        rpc_url: rpc_url.to_string(),
    })
}

/// The `transfer plan` schema identity input constraints.
///
/// Parity with Go `standardExecutionIdentityInputConstraints` (advertised by
/// `transfer plan` via `configureStructuredInput`): a single `exactly_one_of`
/// entry over `[wallet, from_address]` with no `when` clause — transfer
/// planning is OWS-first / standard EVM, with no per-provider identity branching
/// (unlike swap's Tempo/TaikoSwap split).
pub fn transfer_plan_identity_constraints() -> Vec<InputConstraint> {
    vec![InputConstraint {
        kind: "exactly_one_of".to_string(),
        fields: vec!["wallet".to_string(), "from_address".to_string()],
        when: Default::default(),
        description: "Provide exactly one execution identity input: `wallet` \
                      (OWS, recommended) or `from_address` (local signer)."
            .to_string(),
    }]
}

/// Validate that a persisted action is a `transfer` intent.
///
/// Parity with the `submit` / `status` guard `action.IntentType != "transfer"`
/// in `transfer_command.go`: a non-`transfer` intent yields a
/// [`defi_errors::Code::Usage`] error whose message is
/// `action is not a transfer intent`.
pub fn ensure_transfer_intent(intent_type: &str) -> Result<(), Error> {
    if intent_type != "transfer" {
        return Err(Error::new(Code::Usage, "action is not a transfer intent"));
    }
    Ok(())
}

/// clap parsing + handler for the `transfer` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::{Code, Error};
    use defi_execution::builder::Registry;
    use defi_model::{Envelope, ProviderStatus};

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, TransferSubmitArgs};
    use crate::execident::{apply_execution_identity_to_action, resolve_execution_identity};
    use crate::execsubmit::{
        execute_resolved, parse_execute_options, presign_validate_action,
        resolve_action_execution_backend, validate_execution_sender, ExecuteOptionInputs,
        SubmitExecutionInputs,
    };

    /// `transfer` subcommands (Go `newTransferCommand`).
    #[derive(Subcommand, Debug)]
    pub enum TransferCmd {
        /// Create and persist an ERC-20 transfer action plan.
        Plan(PlanArgs),
        /// Execute an existing ERC-20 transfer action.
        Submit(TransferSubmitArgs),
        /// Get transfer action status.
        Status(StatusArgs),
    }

    impl TransferCmd {
        /// The leaf path token (for `meta.command`).
        pub fn path(&self) -> &'static str {
            match self {
                TransferCmd::Plan(_) => "plan",
                TransferCmd::Submit(_) => "submit",
                TransferCmd::Status(_) => "status",
            }
        }
    }

    /// `transfer plan` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct PlanArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Asset symbol/address/CAIP-19.
        #[arg(long)]
        pub asset: Option<String>,
        /// Recipient EOA address.
        #[arg(long)]
        pub recipient: Option<String>,
        /// Amount in base units.
        #[arg(long)]
        pub amount: Option<String>,
        /// Amount in decimal units.
        #[arg(long = "amount-decimal")]
        pub amount_decimal: Option<String>,
        /// RPC URL override for the selected chain.
        #[arg(long = "rpc-url")]
        pub rpc_url: Option<String>,
        /// Include simulation checks during execution.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        pub simulate: bool,
        #[command(flatten)]
        pub identity: PlanIdentityFlags,
        #[command(flatten)]
        pub input: crate::execflags::InputFlags,
    }

    /// Handle `transfer <sub>`.
    pub async fn handle(ctx: &AppCtx, cmd: TransferCmd) -> Result<Envelope, Error> {
        match cmd {
            TransferCmd::Plan(args) => handle_plan(ctx, args).await,
            TransferCmd::Submit(args) => handle_submit(ctx, args).await,
            TransferCmd::Status(args) => handle_status(ctx, args).await,
        }
    }

    /// Handle `transfer plan` (Go `planCmd.RunE` in `transfer_command.go`).
    ///
    /// Flow parity with the Go runner:
    /// 1. resolve the execution identity (OWS `--wallet` first / legacy
    ///    `--from-address`) on the requested chain; an identity error returns the
    ///    typed [`Error`] before anything is persisted;
    /// 2. build the [`TransferRequest`] from the flags + the resolved sender
    ///    ([`super::build_transfer_request`]: chain/asset parse, decimals
    ///    defaulting to 18, amount normalization carrying base + decimal forms);
    /// 3. compose the single-step `transfer` action via the action-build registry
    ///    ([`Registry::build_transfer_action`] → `planner::build_transfer_action`),
    ///    capturing a synthetic `native` provider status (Go `statusFromErr`);
    /// 4. stamp the resolved identity (wallet id/name, from-address, execution
    ///    backend) onto the action and persist it to the action [`Store`];
    /// 5. emit the success envelope with the identity warnings, the cache
    ///    bypassed (execution paths skip the cache, spec §2.5), and the `native`
    ///    provider status.
    ///
    /// [`Store`]: defi_execution::store::Store
    /// [`TransferRequest`]: defi_execution::planner::TransferRequest
    async fn handle_plan(ctx: &AppCtx, args: PlanArgs) -> Result<Envelope, Error> {
        // 0. Merge structured input (`--input-json` / `--input-file`) onto the
        //    parsed flags before any guard (Go PreRunE `applyStructuredFlagInput`
        //    over `transferArgs`). Explicit flags win; unknown key / null → usage.
        let mut args = args;
        merge_plan_input(&mut args)?;

        let chain_arg = args.chain.as_deref().unwrap_or_default();
        let wallet_ref = args.identity.wallet.as_deref().unwrap_or_default();
        let from_flag = args.identity.from_address.as_deref().unwrap_or_default();

        // 1. Resolve the execution identity (returns before any persistence on
        //    error — both / neither input, malformed address, Tempo/non-EVM
        //    --wallet, OWS resolve failures).
        let identity = resolve_execution_identity(wallet_ref, from_flag, chain_arg)?;

        // 2. Build the transfer request against the resolved sender.
        let request = super::build_transfer_request(
            chain_arg,
            args.asset.as_deref().unwrap_or_default(),
            args.amount.as_deref().unwrap_or_default(),
            args.amount_decimal.as_deref().unwrap_or_default(),
            &identity.from_address,
            args.recipient.as_deref().unwrap_or_default(),
            args.simulate,
            args.rpc_url.as_deref().unwrap_or_default(),
        )?;

        // 3. Compose the action via the registry (transfer routes straight to the
        //    planner; no provider routing — `provider == "native"`). A build error
        //    is returned (the runner renders the full error envelope to stderr).
        let mut action = Registry::new().build_transfer_action(request)?;

        // 4. Stamp the identity + persist. The synthetic `native` provider status
        //    is `ok` because the build succeeded (Go `statusFromErr(nil)`).
        apply_execution_identity_to_action(&mut action, &identity);
        let store = ctx.open_action_store()?;
        store
            .save(&action)
            .map_err(|e| Error::wrap(Code::Internal, "persist planned action", e))?;

        // 5. Emit the success envelope (cache bypassed for execution paths).
        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize planned action", e))?;
        let providers = vec![ProviderStatus {
            name: "native".to_string(),
            status: "ok".to_string(),
            latency_ms: 0,
        }];
        let mut env = ctx.metadata_envelope("transfer plan", data, providers);
        env.warnings = identity.warnings;
        Ok(env)
    }

    /// Handle `transfer submit` (Go `submitCmd.RunE` in `transfer_command.go`).
    ///
    /// Structurally identical to `approvals submit` (the same shared `execsubmit`
    /// plumbing: action-id resolve → store load → intent gate → already-completed
    /// short-circuit → backend/signer resolve → sender match → execute-option
    /// parse → broadcast), with the `transfer`-only intent gate
    /// ([`super::ensure_transfer_intent`]). `transfer submit` carries NO
    /// `--allow-max-approval` / `--unsafe-provider-tx` flags
    /// ([`TransferSubmitArgs`]); the Go handler hardcodes both `false` for
    /// `parseExecuteOptions`, so the bounded-approval pre-sign guardrail is
    /// irrelevant here (a `transfer` step is never an `approve`).
    async fn handle_submit(ctx: &AppCtx, args: TransferSubmitArgs) -> Result<Envelope, Error> {
        // 1. Resolve + validate the action id.
        let action_id =
            crate::actions::resolve_action_id(args.action_id.as_deref().unwrap_or_default())?;

        // 2. Load the persisted action (not-found → usage `load action`).
        let store = ctx.open_action_store()?;
        let mut action = store
            .get(&action_id)
            .map_err(|e| Error::wrap(Code::Usage, "load action", e))?;

        // 3. Intent gate (transfer-only).
        super::ensure_transfer_intent(&action.intent_type)?;

        // 4. Already-completed short-circuit (no re-broadcast).
        if action.status == defi_execution::action::ActionStatus::Completed {
            let data = serde_json::to_value(&action)
                .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
            let mut env =
                ctx.metadata_envelope("transfer submit", data, Vec::<ProviderStatus>::new());
            env.warnings = vec!["action already completed".to_string()];
            return Ok(env);
        }

        // 5. Resolve the execution backend + signer (legacy-local / OWS guards).
        let resolved = resolve_action_execution_backend(
            &action,
            SubmitExecutionInputs {
                signer: &args.signer,
                key_source: &args.key_source,
                private_key: args.private_key.as_deref().unwrap_or_default(),
                from_address: args.from_address.as_deref().unwrap_or_default(),
            },
        )?;

        // 6. Validate the resolved sender vs --from-address + planned sender.
        validate_execution_sender(
            &action,
            args.from_address.as_deref().unwrap_or_default(),
            &resolved.sender,
        )?;

        // 7. Parse the execute options (durations, gas multiplier, fee flags). A
        //    transfer carries no approval/provider-tx guardrails, so the Go handler
        //    hardcodes `allow_max_approval` / `unsafe_provider_tx` to `false`.
        let opts = parse_execute_options(&ExecuteOptionInputs {
            simulate: args.simulate,
            poll_interval: &args.poll_interval,
            step_timeout: &args.step_timeout,
            gas_multiplier: args.gas_multiplier,
            max_fee_gwei: args.max_fee_gwei.as_deref().unwrap_or_default(),
            max_priority_fee_gwei: args.max_priority_fee_gwei.as_deref().unwrap_or_default(),
            allow_max_approval: false,
            unsafe_provider_tx: false,
            fee_token: args.fee_token.as_deref().unwrap_or_default(),
        })?;

        // 8. Pre-sign guardrail (a transfer step is never an approval, so this is a
        //    no-op for the bounded-approval bound, but keeping the call mirrors the
        //    approvals path and the engine's per-step policy contract).
        presign_validate_action(&action, &opts)?;

        // 9. Broadcast through the engine (persisting each transition), then emit
        //    the terminal-state envelope (cache bypassed for execution paths).
        execute_resolved(&store, &mut action, resolved, opts).await?;

        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
        Ok(ctx.metadata_envelope("transfer submit", data, Vec::<ProviderStatus>::new()))
    }

    /// Handle `transfer status` (Go `statusCmd.RunE` in `transfer_command.go`).
    ///
    /// A pure read over the persisted action store: resolve + validate the
    /// `--action-id`, load the action (not-found → usage `load action`), gate the
    /// intent (`transfer`-only), and emit the action verbatim (cache bypassed).
    async fn handle_status(ctx: &AppCtx, args: StatusArgs) -> Result<Envelope, Error> {
        let action_id =
            crate::actions::resolve_action_id(args.action_id.as_deref().unwrap_or_default())?;
        let store = ctx.open_action_store()?;
        let action = store
            .get(&action_id)
            .map_err(|e| Error::wrap(Code::Usage, "load action", e))?;
        super::ensure_transfer_intent(&action.intent_type)?;
        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
        Ok(ctx.metadata_envelope("transfer status", data, Vec::<ProviderStatus>::new()))
    }

    /// Merge structured input (`--input-json` / `--input-file`) onto the parsed
    /// `transfer plan` flags (Go PreRunE `applyStructuredFlagInput` over
    /// `transferArgs`). Explicitly-set flags are never overridden; an unknown key
    /// / null value is a usage error keyed on the full command path.
    fn merge_plan_input(args: &mut PlanArgs) -> Result<(), Error> {
        use crate::execflags::{apply_structured_input, decode_bool_field, decode_string_field};

        let mut explicit: std::collections::HashSet<&str> = std::collections::HashSet::new();
        if args.chain.is_some() {
            explicit.insert("chain");
        }
        if args.asset.is_some() {
            explicit.insert("asset");
        }
        if args.recipient.is_some() {
            explicit.insert("recipient");
        }
        if args.amount.is_some() {
            explicit.insert("amount");
        }
        if args.amount_decimal.is_some() {
            explicit.insert("amount-decimal");
        }
        if args.identity.wallet.is_some() {
            explicit.insert("wallet");
        }
        if args.identity.from_address.is_some() {
            explicit.insert("from-address");
        }
        if !args.simulate {
            explicit.insert("simulate");
        }

        apply_structured_input(
            &args.input,
            &explicit,
            "transfer plan",
            |key, canonical, raw| {
                match canonical {
                    "chain" => args.chain = Some(decode_string_field(key, raw)?),
                    "asset" => args.asset = Some(decode_string_field(key, raw)?),
                    "recipient" => args.recipient = Some(decode_string_field(key, raw)?),
                    "amount" => args.amount = Some(decode_string_field(key, raw)?),
                    "amount-decimal" => args.amount_decimal = Some(decode_string_field(key, raw)?),
                    "wallet" => args.identity.wallet = Some(decode_string_field(key, raw)?),
                    "from-address" => {
                        args.identity.from_address = Some(decode_string_field(key, raw)?)
                    }
                    "simulate" => args.simulate = decode_bool_field(key, raw)?,
                    "rpc-url" => args.rpc_url = Some(decode_string_field(key, raw)?),
                    _ => return Ok(false),
                }
                Ok(true)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::transfer` (Go: `internal/app` transfer
    //! command group: `newTransferCommand` in `transfer_command.go`)
    //!
    //! This module owns the **transfer-command glue**. "Correct" means it
    //! preserves the runner-owned transfer behaviors AND the stable machine
    //! contract (design spec §2.2 exit codes, §2.4 ids/amounts kept consistent,
    //! §2.5 OWS-first standard-EVM execution identity). The transfer action
    //! construction + validation (`build_transfer_action`, with recipient/sender
    //! hex + zero-recipient + positive-amount validation — covered by the
    //! `defi-execution::planner` RED suite), the registry routing
    //! (`Registry::build_transfer_action`, B8), the shared execution-identity
    //! resolver, the submit signer/backend plumbing, and the cache-flow core are
    //! owned elsewhere and are NOT re-asserted here. Criteria:
    //!
    //! 1. **Request building + decimals defaulting + amount normalization.**
    //!    `build_transfer_request` mirrors the Go `buildAction` closure.
    //!    (a) `--chain` + `--asset` parse to the chain CAIP-2 id and the asset on
    //!        that chain (USDC on taiko → 6 decimals).
    //!    (b) The amount is normalized against the asset's decimals: base
    //!        `1000000` (USDC, 6 decimals) ⇔ decimal `1` stay consistent (spec
    //!        §2.4). The decimal form `1` normalizes back to base `1000000`.
    //!    (c) The resolved sender (`from_address`), recipient, simulate flag, and
    //!        rpc-url are carried verbatim onto the [`TransferRequest`].
    //!    (Ported from the request-build half of `TestBuildTransferAction` /
    //!    `TestRunnerTransferPlanAcceptsStructuredInputJSON` /
    //!    `TestLegacyFromAddressPlanMarksLegacyBackend`, which all transfer USDC
    //!    1000000 base units on taiko/167000.)
    //!
    //! 2. **Decimals defaulting to 18.** When the parsed asset's `decimals` is
    //!    non-positive (e.g. a bare token address with no registry entry, parsed
    //!    on an EVM chain), `build_transfer_request` normalizes the amount as if
    //!    `decimals == 18` — distinct from the planner, which performs no
    //!    defaulting. A decimal amount of `1` therefore yields base
    //!    `1000000000000000000`. (Go `buildAction`: `if decimals <= 0 { decimals
    //!    = 18 }`.)
    //!
    //! 3. **Amount cross-validation is a usage error.** Supplying BOTH `--amount`
    //!    and `--amount-decimal` → [`Code::Usage`] (exit 2); supplying NEITHER →
    //!    [`Code::Usage`] (exit 2). (Delegated to `defi_id::normalize_amount`,
    //!    spec §2.4, asserted here because the transfer builder owns the call.)
    //!
    //! 4. **`transfer plan` schema identity constraints.**
    //!    `transfer_plan_identity_constraints` returns EXACTLY one
    //!    `exactly_one_of` entry over `[wallet, from_address]` with no `when`
    //!    clause — the standard OWS-first execution identity (no per-provider
    //!    branching, unlike swap). (Ported from
    //!    `TestTransferPlanSchemaIncludesIdentityInputConstraint`,
    //!    `TestTransferPlanSchemaIncludesWallet`.)
    //!
    //! 5. **Persisted-intent gate.** `ensure_transfer_intent` accepts
    //!    `"transfer"` and rejects any other intent with [`Code::Usage`] (exit 2)
    //!    + `action is not a transfer intent`. (Ported from the `submit` /
    //!    `status` `IntentType != "transfer"` guards in `transfer_command.go`.)
    //!
    //! SKIPPED (Go internal-detail / wrong-module):
    //!   * cobra flag wiring + flag defaults (`--simulate true`, `--signer
    //!     local`, required-flag marking for `--chain`/`--asset`/`--recipient`,
    //!     `--gas-multiplier 1.2`, `--poll-interval 2s`) — harness concern,
    //!     asserted by the integration golden-CLI / schema suites, not this unit
    //!     (`TestRunnerTransferPlanRequiresRecipient`,
    //!     `TestRunnerTransferPlanSchemaIncludesStructuredInputMetadata`,
    //!     `TestRunnerTransferSubmitSchemaIncludesStructuredInputMetadata`,
    //!     `TestRunnerTransferPlanRejectsInheritedStructuredInputFields`);
    //!   * the transfer recipient/sender hex validation, zero-recipient
    //!     rejection, positive-amount enforcement, and calldata packing — owned
    //!     by `defi_execution::planner::build_transfer_action` (ported from
    //!     `planner/transfer_test.go`: `TestBuildTransferAction`,
    //!     `TestBuildTransferActionRejectsInvalidAmount`,
    //!     `TestBuildTransferActionRejectsZeroRecipient`);
    //!   * the registry routing for the `transfer` intent — owned by
    //!     `defi_execution::builder` (B8);
    //!   * the OWS-vs-legacy execution-backend stamping + wallet-id persistence
    //!     (`TestLegacyFromAddressPlanMarksLegacyBackend`,
    //!     `TestWalletPlanPersistsWalletIDAndFromAddress`) — shared
    //!     execution-identity / action-store concern;
    //!   * the submit auth metadata (OWS-token first, legacy signer compat) +
    //!     signer enum (`TestTransferSubmitAuthMetadataPrefersOWSAndKeepsLegacy
    //!     Compatibility`) — schema/auth-metadata concern;
    //!   * the structured `--input-json` parsing + already-completed short-circuit
    //!     (`TestRunnerTransferSubmitAcceptsStructuredInputJSON`) — structured-input
    //!     / action-store concern.

    use super::*;
    use defi_errors::{exit_code, Code};

    // --- helpers -----------------------------------------------------------

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    // A canonical-but-arbitrary EVM sender/recipient pair (not validated by the
    // request builder — that's the planner's job — but carried verbatim).
    const SENDER: &str = "0x00000000000000000000000000000000000000aa";
    const RECIPIENT: &str = "0x00000000000000000000000000000000000000bb";

    // --- 1. request building + amount normalization ------------------------

    #[test]
    fn build_request_parses_chain_asset_and_normalizes_base_amount() {
        // USDC (6 decimals) transferred on taiko with a base-unit amount.
        let req = build_transfer_request(
            "taiko",
            "USDC",
            "1000000",
            "",
            SENDER,
            RECIPIENT,
            true,
            "http://127.0.0.1:8545",
        )
        .expect("transfer request built");
        assert_eq!(req.chain.caip2, "eip155:167000");
        assert_eq!(req.asset.symbol, "USDC");
        assert_eq!(req.asset.decimals, 6);
        // base ⇔ decimal stay consistent (spec §2.4).
        assert_eq!(req.amount_base_units, "1000000");
        // sender / recipient / simulate / rpc carried verbatim.
        assert_eq!(req.sender, SENDER);
        assert_eq!(req.recipient, RECIPIENT);
        assert!(req.simulate);
        assert_eq!(req.rpc_url, "http://127.0.0.1:8545");
    }

    #[test]
    fn build_request_normalizes_decimal_amount_against_asset_decimals() {
        // The decimal form normalizes to base units against USDC decimals (6).
        let req = build_transfer_request("taiko", "USDC", "", "1", SENDER, RECIPIENT, true, "")
            .expect("decimal amount normalizes");
        assert_eq!(req.amount_base_units, "1000000");
        assert_eq!(req.asset.decimals, 6);
    }

    #[test]
    fn build_request_carries_simulate_false() {
        let req =
            build_transfer_request("taiko", "USDC", "1000000", "", SENDER, RECIPIENT, false, "")
                .expect("simulate=false carried");
        assert!(!req.simulate);
    }

    // --- 2. decimals defaulting to 18 --------------------------------------

    #[test]
    fn build_request_defaults_decimals_to_18_for_unknown_token() {
        // A bare contract address with no registry symbol parses on an EVM chain
        // but carries non-positive decimals; the transfer builder defaults to 18
        // (Go `buildAction`), so a decimal amount of 1 yields 1e18 base units.
        let token = "0x1111111111111111111111111111111111111111";
        let req = build_transfer_request("1", token, "", "1", SENDER, RECIPIENT, true, "")
            .expect("decimals default to 18");
        assert_eq!(
            req.amount_base_units, "1000000000000000000",
            "decimal 1 against defaulted 18 decimals => 1e18 base units"
        );
    }

    // --- 3. amount cross-validation ----------------------------------------

    #[test]
    fn build_request_rejects_both_amount_forms() {
        let err =
            build_transfer_request("taiko", "USDC", "1000000", "1", SENDER, RECIPIENT, true, "")
                .expect_err("both amount forms rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    #[test]
    fn build_request_rejects_missing_amount() {
        let err = build_transfer_request("taiko", "USDC", "", "", SENDER, RECIPIENT, true, "")
            .expect_err("missing amount rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- 4. transfer plan schema identity constraints ----------------------

    #[test]
    fn plan_identity_constraints_are_standard_exactly_one_of() {
        let constraints = transfer_plan_identity_constraints();
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].kind, "exactly_one_of");
        assert_eq!(
            constraints[0].fields,
            vec!["wallet".to_string(), "from_address".to_string()]
        );
        // No per-provider `when` clause — transfer planning is OWS-first /
        // standard EVM (no Tempo/TaikoSwap-style branching like swap).
        assert!(
            constraints[0].when.is_empty(),
            "standard identity constraint has no `when` clause"
        );
    }

    // --- 5. persisted-intent gate ------------------------------------------

    #[test]
    fn ensure_transfer_intent_accepts_transfer() {
        ensure_transfer_intent("transfer").expect("transfer intent accepted");
    }

    #[test]
    fn ensure_transfer_intent_rejects_non_transfer() {
        // A swap action submitted/queried through `transfer submit|status` fails.
        let err = ensure_transfer_intent("swap").expect_err("non-transfer intent rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("action is not a transfer intent"),
            "got: {err}"
        );
    }
}

#[cfg(test)]
mod app_tests {
    //! # Success criteria — `transfer plan` app-level handler (WS3, exec-plan)
    //!
    //! Go oracle: `internal/app/transfer_command.go` `planCmd.RunE`. These tests
    //! drive [`cli::handle`] (the real dispatch entry point the binary calls)
    //! end-to-end for `transfer plan` ONLY, asserting the full machine contract
    //! the Go runner emits via `emitSuccess(...)` / `renderError(...)`. They are
    //! offline + deterministic: an ERC-20 `transfer(recipient, amount)` action is
    //! built entirely from calldata (the planner does NOT connect to RPC for
    //! transfers — `--rpc-url` / the registry default RPC is only carried onto the
    //! step), and persistence uses a real [`defi_execution::store::Store`] over a
    //! `tempfile` directory. No wiremock network is required for the transfer
    //! build itself; the base-URL / `--rpc-url` seams exist but no provider HTTP
    //! call is made on this path (`provider == "native"`). Identity is exercised
    //! through the OFFLINE `--from-address` (legacy_local) path so no OWS vault /
    //! network is touched; the `--wallet` happy path (OWS resolve) is WS4b e2e
    //! territory and is asserted here only via its offline guard rejections.
    //!
    //! Transfer is the simplest standard-EVM execution command and is structurally
    //! identical to `approvals plan` (no provider routing, internal planner,
    //! OWS-first identity) — these criteria are the transfer analogue of the
    //! `approvals plan` app suite, with `--recipient` in place of `--spender`, the
    //! `transfer` intent, and the `defi-evm` ERC-20 `transfer` calldata golden.
    //!
    //! Criteria (each a failing test until `cli::handle` routes `Plan` to a real
    //! handler — the stub currently returns the `AppCtx::unimplemented` error):
    //!
    //! 1. **Plan success envelope (legacy `--from-address`).** A valid
    //!    `transfer plan --chain 1 --asset USDC --recipient 0x..CC --amount
    //!    1000000 --from-address 0x..aa` returns an `Ok(Envelope)` (exit 0) with:
    //!    `version == "v1"`, `success == true`, `error == None`, `meta.partial ==
    //!    false`, `meta.command == "transfer plan"`,
    //!    `meta.cache == {status:"bypass", age_ms:0, stale:false}` (execution paths
    //!    bypass the cache, spec §2.5), and `meta.providers == [{name:"native",
    //!    status:"ok"}]` (Go `statusFromErr(nil) == "ok"`; transfer has no provider
    //!    routing — `provider == "native"`).
    //!
    //! 2. **Planned action `data` shape.** `env.data` is the serialized [`Action`]:
    //!    `action_id` matches `^act_[0-9a-f]{32}$`; `intent_type == "transfer"`;
    //!    `provider == "native"`; `status == "planned"`; `chain_id == "eip155:1"`;
    //!    `from_address` == the EIP-55 checksum of the sender; `to_address` == the
    //!    recipient address; `input_amount == "1000000"`; exactly ONE step with
    //!    `type == "transfer"`, `value == "0"`, `target` == the USDC token address,
    //!    and `chain_id == "eip155:1"`. (Mirrors the Go oracle persisted action:
    //!    `transfer plan ... --asset USDC --amount 1000000` → `intent_type:
    //!    "transfer"`, `input_amount: "1000000"`, step `type: "transfer"`.)
    //!
    //! 3. **Step calldata reuses the `defi-evm` ABI golden.** With recipient
    //!    `0x00000000000000000000000000000000000000CC` and amount `1000000`, the
    //!    step `data` equals the pinned ERC-20 `transfer` calldata golden
    //!    (`defi-evm` `encode_erc20_transfer_matches_golden`):
    //!    `0xa9059cbb` + recipient(32) + `0xf4240`(=1000000, 32). This proves the
    //!    handler routes through `build_transfer_action` (no re-encoding).
    //!
    //! 4. **Legacy-identity warning surfaces in the envelope.** The
    //!    `--from-address` path stamps `execution_backend == "legacy_local"` on the
    //!    action AND surfaces the Go warning
    //!    `--wallet (OWS) is recommended over --from-address for planning; see docs
    //!    for details` in `env.warnings`. (Go `resolveExecutionIdentity` legacy
    //!    branch + `emitSuccess(..., identity.Warnings, ...)`.)
    //!
    //! 5. **Plan persists the action to the Store.** After a successful plan the
    //!    action is retrievable by its `action_id` from a freshly opened
    //!    [`defi_execution::store::Store`] over the same path, with matching
    //!    `intent_type == "transfer"`, `input_amount`, and `provider == "native"`.
    //!    (Go `s.actionStore.Save`.)
    //!
    //! 6. **Decimal amount parity.** `--amount-decimal 1` (no `--amount`) on USDC
    //!    (6 decimals) yields the same `input_amount == "1000000"` and the same
    //!    calldata golden — base ⇔ decimal stay consistent (spec §2.4).
    //!
    //! 7. **Identity-constraint errors (offline).**
    //!    (a) BOTH `--wallet` and `--from-address` → [`Code::Usage`] (exit 2);
    //!    (b) NEITHER `--wallet` nor `--from-address` → [`Code::Usage`] (exit 2);
    //!    (c) a malformed `--from-address` → [`Code::Usage`] (exit 2);
    //!    (d) `--wallet` on a Tempo chain → [`Code::Unsupported`] (exit 13)
    //!        (`--wallet planning is not supported on Tempo chains yet`).
    //!    (Go `resolveExecutionIdentity`.) On every error the handler returns the
    //!    typed `Err(Error)` (the runner renders the full error envelope to stderr,
    //!    spec §2.1) and persists NOTHING to the Store.
    //!
    //! 8. **Amount cross-validation through the handler.** BOTH `--amount` +
    //!    `--amount-decimal` → [`Code::Usage`] (exit 2); NEITHER → [`Code::Usage`]
    //!    (exit 2). (Delegated to `defi_id::normalize_amount` via
    //!    `build_transfer_request`; asserted at the handler boundary.)
    //!
    //! 9. **Planner validation surfaces through the handler.**
    //!    (a) a malformed `--recipient` → [`Code::Usage`] (exit 2)
    //!        (`build_transfer_action` recipient hex validation);
    //!    (b) a zero `--recipient` (the zero address) → [`Code::Usage`] (exit 2)
    //!        (`transfer recipient cannot be zero address`);
    //!    (c) a non-positive `--amount` (`0`) → [`Code::Usage`] (exit 2)
    //!        (`transfer amount must be a positive integer in base units`).
    //!    On each, nothing is persisted.
    //!
    //! SKIPPED (covered elsewhere / wrong unit):
    //!   * the `transfer` calldata ABI encoding itself — `defi-evm::abi` golden
    //!     (`encode_erc20_transfer_matches_golden`);
    //!   * `build_transfer_action` sender/recipient/token hex + zero-recipient +
    //!     positive-amount internals — `defi-execution::planner` RED suite (ported
    //!     from `planner/transfer_test.go`);
    //!   * the registry routing for the `transfer` intent — `defi-execution::builder`;
    //!   * the OWS `--wallet` happy-path resolve + wallet-id persistence — WS4b
    //!     e2e (here only its offline guard rejections are asserted);
    //!   * `--input-json`/`--input-file` precedence — structured-input unit;
    //!   * cobra/clap flag defaults + required-flag marking — schema/CLI suites;
    //!   * `transfer submit`/`status` — WS4 (`defi-execution` submit/signer concern).

    use super::cli::{handle, PlanArgs, TransferCmd};
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags};
    use defi_config::Settings;
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;

    // --- contract constants ------------------------------------------------

    /// Sender EOA (legacy `--from-address` identity); not validated for casing by
    /// the handler — its EIP-55 checksum is what lands on the action.
    const SENDER: &str = "0x00000000000000000000000000000000000000aa";
    /// Recipient matching the `defi-evm` `encode_erc20_transfer_matches_golden`
    /// fixture (`RECIPIENT = 0x..CC`), so the planned step `data` reuses that
    /// golden.
    const RECIPIENT: &str = "0x00000000000000000000000000000000000000CC";
    /// USDC contract on Ethereum mainnet (6 decimals) — resolved by `parse_asset`.
    const USDC_MAINNET: &str = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
    /// The pinned ERC-20 `transfer(0x..CC, 1000000)` calldata (defi-evm golden).
    const TRANSFER_CALLDATA_GOLDEN: &str = "0xa9059cbb00000000000000000000000000000000000000000000000000000000000000cc00000000000000000000000000000000000000000000000000000000000f4240";
    /// The Go legacy-identity warning surfaced when planning with `--from-address`.
    const LEGACY_WARNING: &str =
        "--wallet (OWS) is recommended over --from-address for planning; see docs for details";

    // --- harness -----------------------------------------------------------

    /// Execution settings with a real action store under `dir` and the cache
    /// disabled (execution paths bypass the cache anyway, spec §2.5).
    fn exec_settings(dir: &Path) -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: false,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_millis(750),
            retries: 0,
            max_stale: Duration::from_secs(0),
            no_stale: false,
            cache_enabled: false,
            cache_path: dir.join("cache.db"),
            cache_lock_path: dir.join("cache.lock"),
            action_store_path: dir.join("actions.db"),
            action_lock_path: dir.join("actions.lock"),
            defillama_api_key: String::new(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    /// A `PlanArgs` with the canonical happy-path values; mutate the result per
    /// test (e.g. clear `amount`, set `wallet`).
    fn base_plan_args() -> PlanArgs {
        PlanArgs {
            chain: Some("1".to_string()),
            asset: Some("USDC".to_string()),
            recipient: Some(RECIPIENT.to_string()),
            amount: Some("1000000".to_string()),
            amount_decimal: None,
            rpc_url: None,
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(SENDER.to_string()),
            },
            input: InputFlags::default(),
        }
    }

    async fn run_plan(dir: &Path, args: PlanArgs) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(&ctx, TransferCmd::Plan(args)).await
    }

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn action_data(env: &Envelope) -> Value {
        env.data.clone().expect("plan envelope carries `data`")
    }

    // --- 1, 2. plan success envelope + action shape ------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_legacy_from_address_emits_success_envelope() {
        let tmp = TempDir::new().expect("tempdir");
        let env = run_plan(tmp.path(), base_plan_args())
            .await
            .expect("transfer plan should succeed on the legacy path");

        // Envelope contract (Go `emitSuccess`).
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "transfer plan");

        // Execution paths bypass the cache (spec §2.5).
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        // No provider routing: a single synthetic `native` status, ok.
        assert_eq!(env.meta.providers.len(), 1, "exactly one provider status");
        assert_eq!(env.meta.providers[0].name, "native");
        assert_eq!(env.meta.providers[0].status, "ok");

        // Action `data` shape (Go persisted action).
        let data = action_data(&env);
        let action_id = data["action_id"].as_str().expect("action_id string");
        assert!(
            action_id.strip_prefix("act_").is_some_and(|rest| rest.len() == 32
                && rest.bytes().all(|b| b.is_ascii_hexdigit())),
            "action_id must match act_<32 hex>: got {action_id}"
        );
        assert_eq!(data["intent_type"], Value::from("transfer"));
        assert_eq!(data["provider"], Value::from("native"));
        assert_eq!(data["status"], Value::from("planned"));
        assert_eq!(data["chain_id"], Value::from("eip155:1"));
        assert_eq!(
            data["from_address"].as_str().unwrap().to_lowercase(),
            SENDER.to_lowercase(),
            "from_address is the (checksummed) sender"
        );
        assert_eq!(
            data["to_address"].as_str().unwrap().to_lowercase(),
            RECIPIENT.to_lowercase(),
            "to_address is the recipient"
        );
        assert_eq!(data["input_amount"], Value::from("1000000"));

        // Exactly one transfer step, value 0, target = token, chain carried.
        let steps = data["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1, "transfer is a single-step action");
        assert_eq!(steps[0]["type"], Value::from("transfer"));
        assert_eq!(steps[0]["value"], Value::from("0"));
        assert_eq!(steps[0]["chain_id"], Value::from("eip155:1"));
        assert_eq!(
            steps[0]["target"].as_str().unwrap().to_lowercase(),
            USDC_MAINNET,
            "transfer step targets the USDC token contract"
        );

        // Legacy backend stamping + warning (criterion 4).
        assert_eq!(data["execution_backend"], Value::from("legacy_local"));
        assert!(
            env.warnings.iter().any(|w| w == LEGACY_WARNING),
            "legacy --from-address plan surfaces the OWS-recommended warning; got {:?}",
            env.warnings
        );
    }

    // --- 3. step calldata reuses the defi-evm ABI golden -------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_step_calldata_matches_defi_evm_transfer_golden() {
        let tmp = TempDir::new().expect("tempdir");
        let env = run_plan(tmp.path(), base_plan_args())
            .await
            .expect("transfer plan should succeed");
        let data = action_data(&env);
        let calldata = data["steps"][0]["data"].as_str().expect("step data string");
        assert_eq!(
            calldata, TRANSFER_CALLDATA_GOLDEN,
            "transfer step calldata must equal the pinned defi-evm ERC-20 transfer golden"
        );
    }

    // --- structured input (`--input-json` / `--input-file`) ----------------
    //
    // Go: `configureStructuredInput[transferArgs]` wires the PreRunE merge onto
    // `transfer plan`. JSON fills flags; explicit flags override JSON; unknown
    // keys / null values are usage errors that persist nothing.

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_resolves_all_flags_from_input_json() {
        let tmp = TempDir::new().expect("tempdir");
        let args = PlanArgs {
            input: InputFlags {
                input_json: Some(format!(
                    r#"{{"chain":"1","asset":"USDC","recipient":"{RECIPIENT}","amount":"1000000","from_address":"{SENDER}"}}"#
                )),
                input_file: None,
            },
            ..PlanArgs::default()
        };
        let env = run_plan(tmp.path(), args)
            .await
            .expect("input-json should fill all flags and the plan should succeed");
        assert!(env.success);
        assert_eq!(env.meta.command, "transfer plan");
        let data = action_data(&env);
        assert_eq!(data["intent_type"], Value::from("transfer"));
        assert_eq!(
            data["steps"][0]["data"].as_str().expect("step data"),
            TRANSFER_CALLDATA_GOLDEN,
            "recipient/amount taken from the JSON must reproduce the pinned golden"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_input_json_unknown_field_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        // `to` is not a transfer-plan field (the flag is `recipient`).
        let args = PlanArgs {
            input: InputFlags {
                input_json: Some(r#"{"chain":"1","to":"0x00"}"#.to_string()),
                input_file: None,
            },
            ..PlanArgs::default()
        };
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("unknown structured-input field must be a usage error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert_eq!(
            err.message,
            "structured input field \"to\" is not supported by transfer plan"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_input_json_number_for_string_flag_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        let args = PlanArgs {
            input: InputFlags {
                input_json: Some(format!(
                    r#"{{"chain":"1","asset":"USDC","recipient":"{RECIPIENT}","amount":1000000,"from_address":"{SENDER}"}}"#
                )),
                input_file: None,
            },
            ..PlanArgs::default()
        };
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("a JSON number for a string flag must be a usage decode error");
        assert_eq!(err.code, Code::Usage);
        assert!(
            err.message
                .starts_with("decode structured input field \"amount\""),
            "got {:?}",
            err.message
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 5. plan persists the action to the Store --------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_persists_action_to_store() {
        let tmp = TempDir::new().expect("tempdir");
        let settings = exec_settings(tmp.path());
        let ctx = AppCtx::new(settings.clone());
        let env = handle(&ctx, TransferCmd::Plan(base_plan_args()))
            .await
            .expect("transfer plan should succeed");
        let action_id = action_data(&env)["action_id"]
            .as_str()
            .expect("action_id")
            .to_string();

        // Re-open the store independently and confirm the action persisted.
        let store = ActionStore::open(&settings.action_store_path, &settings.action_lock_path)
            .expect("reopen action store");
        let persisted = store
            .get(&action_id)
            .expect("planned action retrievable by id");
        assert_eq!(persisted.intent_type, "transfer");
        assert_eq!(persisted.input_amount, "1000000");
        assert_eq!(persisted.provider, "native");
    }

    // --- 6. decimal amount parity ------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_decimal_amount_yields_same_base_and_calldata() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_plan_args();
        args.amount = None;
        args.amount_decimal = Some("1".to_string()); // 1 USDC (6 decimals)
        let env = run_plan(tmp.path(), args)
            .await
            .expect("decimal-amount plan should succeed");
        let data = action_data(&env);
        assert_eq!(data["input_amount"], Value::from("1000000"));
        assert_eq!(
            data["steps"][0]["data"].as_str().unwrap(),
            TRANSFER_CALLDATA_GOLDEN,
            "decimal 1 USDC normalizes to the same calldata as base 1000000"
        );
    }

    // --- 7. identity-constraint errors (offline) ---------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_both_identity_inputs() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_plan_args();
        args.identity.wallet = Some("alice".to_string());
        // from_address already set in base.
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("both identity inputs must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_missing_identity_inputs() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_plan_args();
        args.identity.wallet = None;
        args.identity.from_address = None;
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("missing identity inputs must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_malformed_from_address() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_plan_args();
        args.identity.from_address = Some("0xnot-an-address".to_string());
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("malformed --from-address must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_wallet_on_tempo_chain() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_plan_args();
        args.chain = Some("tempo".to_string()); // eip155:4217 (Tempo mainnet)
        args.identity.from_address = None;
        args.identity.wallet = Some("alice".to_string());
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("--wallet on Tempo must be rejected");
        assert_eq!(err.code, Code::Unsupported);
        // Unsupported maps to exit 13 (spec §2.2).
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        // Go message (distinguishes the real guard from the unimplemented stub,
        // which is also Unsupported but with a different message).
        assert!(
            err.to_string()
                .contains("--wallet planning is not supported on Tempo chains yet"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 8. amount cross-validation through the handler --------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_both_amount_forms() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_plan_args();
        args.amount = Some("1000000".to_string());
        args.amount_decimal = Some("1".to_string());
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("both amount forms must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_missing_amount() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_plan_args();
        args.amount = None;
        args.amount_decimal = None;
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("missing amount must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 9. planner validation surfaces through the handler ----------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_malformed_recipient() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_plan_args();
        args.recipient = Some("0xdeadbeef".to_string()); // too short -> invalid hex addr
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("malformed --recipient must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_zero_recipient() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_plan_args();
        args.recipient = Some("0x0000000000000000000000000000000000000000".to_string());
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("zero recipient must be rejected by the planner");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("transfer recipient cannot be zero address"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_non_positive_amount() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_plan_args();
        args.amount = Some("0".to_string());
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("zero amount must be rejected by the planner");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- helpers depending on the store ------------------------------------

    /// True iff no action is persisted under `dir` (error paths must persist
    /// nothing). Opens the store leniently; a never-created store (no actions
    /// persisted yet) counts as empty.
    fn no_actions_persisted(dir: &Path) -> bool {
        let store = match ActionStore::open(dir.join("actions.db"), dir.join("actions.lock")) {
            Ok(store) => store,
            // If the store was never opened by the handler, nothing persisted.
            Err(_) => return true,
        };
        store
            .list("", 1000)
            .map(|actions| actions.is_empty())
            .unwrap_or(true)
    }
}

#[cfg(test)]
mod submit_app_tests {
    //! # Success criteria — `transfer submit` app-level handler (WS4, exec-submit)
    //!
    //! Go oracle: `internal/app/transfer_command.go` `submitCmd.RunE` +
    //! `internal/app/execution_helpers.go`
    //! (`resolveActionExecutionBackend` / `validateExecutionSender` /
    //! `executeActionWithTimeout`) + `internal/app/runner.go`
    //! (`resolveActionID` / `newExecutionSigner` / `parseExecuteOptions`). These
    //! tests drive [`cli::handle`] (the real binary dispatch entry point) for
    //! `transfer submit` ONLY, asserting the full machine contract the Go runner
    //! emits via `emitSuccess(...)` / `renderError(...)`.
    //!
    //! Transfer submit is structurally identical to `approvals submit` (the same
    //! shared `execsubmit` plumbing: action-id resolve → store load → intent gate →
    //! already-completed short-circuit → backend/signer resolve → sender match →
    //! execute-option parse → broadcast), with three differences:
    //!   * the intent gate is `transfer`-only (`action is not a transfer intent`),
    //!     not `approve`-only ([`super::ensure_transfer_intent`]);
    //!   * `transfer submit` carries NO `--allow-max-approval` / `--unsafe-provider-tx`
    //!     flags ([`crate::execflags::TransferSubmitArgs`]) — the Go handler hardcodes
    //!     `false`/`false` for those `parseExecuteOptions` args, so the bounded-approval
    //!     pre-sign guardrail is irrelevant here (a `transfer` step is never an
    //!     `approve`, so no over-approval bound exists);
    //!   * the persisted step is a `transfer` (`StepType::Transfer`), not an
    //!     `approval`.
    //!
    //! ## Determinism / offline strategy (no live chains)
    //!
    //! The reused [`defi_execution`] engine ([`defi_execution::evm_executor::execute_action`])
    //! is the contract source of truth, and the tests reuse it exactly as its own
    //! suite (and the `approvals submit` app suite) does:
    //!
    //! * **Pre-broadcast guards** (action-id, store load, intent gate,
    //!   already-completed short-circuit, backend selection, sender match,
    //!   execute-option validation) all fire BEFORE any network and are fully
    //!   deterministic.
    //! * **Local-signer broadcast/completion** is exercised OFFLINE through the
    //!   `--private-key` override (a deterministic in-args secp256k1 key whose
    //!   address is pinned in `defi-evm`): in this build the EVM step path enforces
    //!   the pre-sign policy and (matching the engine's own `execute_action` tests,
    //!   which never dial the step `rpc_url` for a policed EVM step) transitions the
    //!   action to `completed` without a network call. Unlike `approvals submit`, a
    //!   transfer step needs NO `--allow-max-approval` to pass the policy (there is
    //!   no approval bound to inflate). The full RPC-backed sign+broadcast
    //!   (chain-id/gas/nonce/`sendRawTransaction`/receipt) is integration/`wiremock`-RPC
    //!   territory (WS5) and is recorded as a deferral — it is NOT asserted here.
    //! * **OWS `--wallet` backend** resolves through the OWS vault/CLI (WS4b e2e),
    //!   so only its OFFLINE guard rejections are asserted (missing persisted
    //!   `wallet_id`; legacy signer flags on a wallet-backed action). The OWS
    //!   happy-path broadcast (the `OwsSubmitBackend` send-hook seam) is a WS4b
    //!   deferral.
    //! * **Bridge destination-settlement waits** do NOT apply to `transfer`
    //!   (transfer actions never carry a `bridge_send` step); that transition is
    //!   owned by the `bridge submit/status` unit + the `defi-execution`
    //!   `verify_bridge_settlement` suite, and is intentionally NOT re-asserted here.
    //!
    //! Each criterion below is a FAILING test until `cli::handle` implements
    //! `transfer submit` (today it returns the `AppCtx::unimplemented` stub).
    //!
    //! Criteria:
    //!
    //! 1. **Submit success envelope (legacy local key) + completion.** Given a
    //!    persisted `transfer` action whose `from_address` matches the deterministic
    //!    `--private-key` signer, a submit returns `Ok(Envelope)` (exit 0) with:
    //!    `version == "v1"`, `success == true`, `error == None`, `meta.partial ==
    //!    false`, `meta.command == "transfer submit"`, and `meta.cache ==
    //!    {status:"bypass", age_ms:0, stale:false}` (execution paths bypass the
    //!    cache, spec §2.5). The serialized `data` Action has `status ==
    //!    "completed"` and its single step has `status == "confirmed"`. (Go
    //!    `emitSuccess(..., action, nil, cacheMetaBypass(), nil, false)` after
    //!    `executeActionWithTimeout`.) NB: no `--allow-max-approval` is needed (the
    //!    flag does not exist on `transfer submit`).
    //!
    //! 2. **Submit persists the terminal state.** After a successful submit, the
    //!    action re-loaded from a freshly opened [`defi_execution::store::Store`]
    //!    has `status == "completed"`. (Go `ExecuteAction` persists each
    //!    transition through `s.actionStore`.)
    //!
    //! 3. **Action-id validation.** `--action-id ""` → [`Code::Usage`] (exit 2)
    //!    (`action id is required (--action-id)`); a malformed id (`"act_xyz"`) →
    //!    [`Code::Usage`] (exit 2) (`action id must match act_<32 hex chars>`).
    //!    (Go `resolveActionID`.)
    //!
    //! 4. **Load failure for a non-existent action.** A well-formed but unknown
    //!    `--action-id` → [`Code::Usage`] (exit 2) (Go wraps the store `Get`
    //!    not-found as `clierr.Wrap(CodeUsage, "load action", err)`).
    //!
    //! 5. **Intent gate.** Submitting a persisted NON-`transfer` action (e.g. an
    //!    `approve` intent) through `transfer submit` → [`Code::Usage`] (exit 2)
    //!    with `action is not a transfer intent`. (Go `submitCmd` IntentType
    //!    guard; mirrors [`super::ensure_transfer_intent`].)
    //!
    //! 6. **Already-completed short-circuit.** Submitting an action already in
    //!    `status == "completed"` returns `Ok(Envelope)` (exit 0) WITHOUT
    //!    re-broadcast, carrying the warning `action already completed` and the
    //!    unchanged completed action in `data`. (Go `if action.Status ==
    //!    ActionStatusCompleted { return s.emitSuccess(..., []string{"action
    //!    already completed"}, ...) }`.)
    //!
    //! 7. **Legacy backend rejects a non-local signer.** A `legacy_local` action
    //!    submitted with `--signer tempo` → [`Code::Usage`] (exit 2)
    //!    (`legacy actions only support --signer local; tempo submit requires
    //!    execution_backend=tempo`). (Go `resolveActionExecutionBackend` legacy
    //!    branch.)
    //!
    //! 8. **OWS action missing persisted wallet_id.** A wallet-backed
    //!    (`execution_backend == "ows"`) action with an empty `wallet_id` → submit
    //!    is rejected with [`Code::Usage`] (exit 2)
    //!    (`wallet-backed action is missing persisted wallet_id`). (Go OWS branch
    //!    guard — reachable OFFLINE because the guard precedes any OWS resolve.)
    //!
    //! 9. **OWS action rejects legacy signer flags.** A wallet-backed action with a
    //!    persisted `wallet_id` submitted with an explicit legacy signer flag
    //!    (`--private-key`) → [`Code::Usage`] (exit 2)
    //!    (`wallet-backed actions do not accept legacy signer flags`). (Go
    //!    `usesLegacySignerFlags` guard — asserted via the `--private-key` flag,
    //!    which is unambiguously "explicitly set".)
    //!
    //! 10. **Sender mismatch (`--from-address`).** A `legacy_local` action whose
    //!     persisted `from_address` is address A, submitted with `--from-address`
    //!     == address B (≠ the resolved signer) → [`Code::Signer`] (exit 24).
    //!     (Go `validateExecutionSender`: `signer address does not match
    //!     --from-address`.)
    //!
    //! 11. **Sender mismatch (planned action sender vs signer).** A `legacy_local`
    //!     action whose persisted `from_address` does NOT match the
    //!     `--private-key` signer address (and no `--from-address` is supplied) →
    //!     a [`Code::Signer`] (exit 24) error surfaces from the persisted-sender
    //!     validation. (Go `validateExecutionSender` /
    //!     `validate_persisted_action_sender`: backend sender ≠ planned sender.)
    //!
    //! 12. **Execute-option validation.** `--gas-multiplier 1.0` → [`Code::Usage`]
    //!     (exit 2) (`--gas-multiplier must be > 1`); `--poll-interval "0s"` →
    //!     [`Code::Usage`] (exit 2); `--step-timeout "nope"` → [`Code::Usage`]
    //!     (exit 2). (Go `parseExecuteOptions`.)
    //!
    //! 13. **Signer init failure (no key).** A `legacy_local` action submitted with
    //!     `--signer local` and NO resolvable key (`--key-source env` with the env
    //!     unset, no `--private-key`) → [`Code::Signer`] (exit 24). (Go
    //!     `newExecutionSigner` → `initialize local signer`.)
    //!
    //! 14. **Error paths do not mutate terminal status.** On every rejected submit
    //!     (criteria 3–13, error cases) the persisted action — when one exists —
    //!     remains in its pre-submit `status == "planned"` (the handler returns the
    //!     typed `Err(Error)`; the runner renders the full error envelope to
    //!     stderr, spec §2.1).
    //!
    //! SKIPPED (covered elsewhere / wrong unit / deferred):
    //!   * the full RPC-backed sign+broadcast (chain-id/gas/fee/nonce/
    //!     `sendRawTransaction`/receipt) — WS5 `wiremock`-RPC integration deferral;
    //!   * the OWS happy-path resolve + send-hook broadcast — WS4b e2e deferral;
    //!   * Tempo (type 0x76) submit — Tempo is a separate execution path
    //!     (`--signer tempo` / `execution_backend == "tempo"`), byte-parity is
    //!     WS4a, and `transfer` planning is OWS-first standard-EVM (no Tempo
    //!     identity branch);
    //!   * the bounded-approval pre-sign guardrail / `--allow-max-approval` —
    //!     N/A to transfer (no such flag, no approval bound); owned + asserted by
    //!     the `approvals submit` app suite + `defi-execution::policy`;
    //!   * bridge destination-settlement waits — `bridge submit/status` unit +
    //!     `defi-execution::verify_bridge_settlement`;
    //!   * the EIP-1559 signing byte layout — `defi-evm` signer goldens;
    //!   * `--input-json`/`--input-file` precedence on submit — structured-input
    //!     unit (the plan-side merge is already covered in `app_tests`);
    //!   * cobra/clap flag defaults + schema auth metadata — schema/CLI suites.

    use super::cli::{handle, PlanArgs, TransferCmd};
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags, TransferSubmitArgs};
    use defi_config::Settings;
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::action::{Action, ActionStatus, ExecutionBackend};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;

    // --- contract constants ------------------------------------------------

    /// The deterministic secp256k1 test key (`internal/execution/signer`
    /// `testPrivateKey`); shared with the `defi-evm` / `defi-execution` suites.
    const TEST_KEY: &str = "59c6995e998f97a5a0044976f0945388cf9b7e5e5f4f9d2d9d8f1f5b7f6d11d1";
    /// The EIP-55 address `defi-evm` derives for [`TEST_KEY`] (pinned in
    /// `defi-evm::signer` against the go-ethereum oracle). The persisted action's
    /// `from_address` must equal this for the local-signer submit to pass the
    /// sender-match guard.
    const SIGNER_ADDR: &str = "0x14DDBd1fe5026E58A12eE8691cAEbFD24bb10eef";
    /// A DIFFERENT canonical address — used to force the sender-mismatch guards.
    const OTHER_ADDR: &str = "0x1111111111111111111111111111111111111111";
    /// Recipient for planned transfers (matches the `defi-evm` transfer golden
    /// recipient `0x..CC` so the planned step shape is identical to production).
    const RECIPIENT: &str = "0x00000000000000000000000000000000000000CC";

    /// A non-dialed RPC sentinel for the step (the policed EVM step path does not
    /// reach the network in this build; this keeps the action well-formed).
    const DEAD_RPC: &str = "http://127.0.0.1:0";

    // --- harness -----------------------------------------------------------

    /// Execution settings with a real action store under `dir`, cache disabled.
    fn exec_settings(dir: &Path) -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: false,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_millis(750),
            retries: 0,
            max_stale: Duration::from_secs(0),
            no_stale: false,
            cache_enabled: false,
            cache_path: dir.join("cache.db"),
            cache_lock_path: dir.join("cache.lock"),
            action_store_path: dir.join("actions.db"),
            action_lock_path: dir.join("actions.lock"),
            defillama_api_key: String::new(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    /// A `TransferSubmitArgs` carrying the clap flag DEFAULTS (the
    /// `#[derive(Default)]` zero values would NOT match the parsed defaults, so
    /// they are stamped here): `signer=local`, `key_source=auto`,
    /// `gas_multiplier=1.2`, `poll_interval=2s`, `step_timeout=2m`,
    /// `simulate=true`. The `--private-key` is pre-set to the deterministic test
    /// key so the offline local-signer path resolves. Callers mutate the returned
    /// value per test. NB: there is NO `allow_max_approval`/`unsafe_provider_tx`
    /// field on transfer submit (Go hardcodes them to false).
    fn base_submit_args(action_id: &str) -> TransferSubmitArgs {
        TransferSubmitArgs {
            action_id: Some(action_id.to_string()),
            from_address: None,
            signer: "local".to_string(),
            key_source: "auto".to_string(),
            private_key: Some(TEST_KEY.to_string()),
            fee_token: None,
            gas_multiplier: 1.2,
            max_fee_gwei: None,
            max_priority_fee_gwei: None,
            simulate: true,
            poll_interval: "2s".to_string(),
            step_timeout: "2m".to_string(),
            input: InputFlags::default(),
        }
    }

    /// Plan + persist a canonical `transfer` action against `dir`, returning its
    /// `action_id`. `from_addr` becomes the action's `from_address`; `amount` is
    /// the transferred base-unit amount (which is also the planned `input_amount`).
    /// Plans through the real `cli::handle` plan path so the persisted shape is
    /// identical to production.
    async fn plan_transfer(dir: &Path, from_addr: &str, amount: &str) -> String {
        let ctx = AppCtx::new(exec_settings(dir));
        let args = PlanArgs {
            chain: Some("1".to_string()),
            asset: Some("USDC".to_string()),
            recipient: Some(RECIPIENT.to_string()),
            amount: Some(amount.to_string()),
            amount_decimal: None,
            rpc_url: Some(DEAD_RPC.to_string()),
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(from_addr.to_string()),
            },
            input: InputFlags::default(),
        };
        let env = handle(&ctx, TransferCmd::Plan(args))
            .await
            .expect("plan a transfer action for the submit fixture");
        env.data.expect("plan data")["action_id"]
            .as_str()
            .expect("action_id")
            .to_string()
    }

    /// Persist `action` directly (used for fixtures the plan path cannot build,
    /// e.g. an `approve`-intent or an OWS-backed action).
    fn save_action(dir: &Path, action: &Action) {
        let store = ActionStore::open(dir.join("actions.db"), dir.join("actions.lock"))
            .expect("open action store");
        store.save(action).expect("persist fixture action");
    }

    /// Re-load a persisted action's `status` string from a freshly opened store.
    fn persisted_status(dir: &Path, action_id: &str) -> String {
        let store = ActionStore::open(dir.join("actions.db"), dir.join("actions.lock"))
            .expect("open action store");
        let action = store.get(action_id).expect("action retrievable");
        serde_json::to_value(action.status)
            .expect("status serializes")
            .as_str()
            .expect("status is a string")
            .to_string()
    }

    async fn run_submit(dir: &Path, args: TransferSubmitArgs) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(&ctx, TransferCmd::Submit(args)).await
    }

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn data_of(env: &Envelope) -> Value {
        env.data.clone().expect("submit envelope carries `data`")
    }

    // --- 1, 2. submit success + completion + persistence -------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_legacy_local_completes_and_emits_envelope() {
        let tmp = TempDir::new().expect("tempdir");
        // Plan a transfer whose sender matches the deterministic local signer.
        let action_id = plan_transfer(tmp.path(), SIGNER_ADDR, "1000000").await;

        // No --allow-max-approval needed: a transfer step is never an approval, so
        // the bounded-approval pre-sign policy does not apply.
        let env = run_submit(tmp.path(), base_submit_args(&action_id))
            .await
            .expect("legacy-local transfer submit should complete offline");

        // Envelope contract.
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "transfer submit");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        // Completed action in data, single confirmed step.
        let data = data_of(&env);
        assert_eq!(data["status"], Value::from("completed"));
        let steps = data["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0]["status"], Value::from("confirmed"));

        // Persisted terminal state (criterion 2).
        assert_eq!(persisted_status(tmp.path(), &action_id), "completed");
    }

    // --- 3. action-id validation -------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_empty_action_id() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_submit_args("");
        args.action_id = Some(String::new());
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("empty action id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_malformed_action_id() {
        let tmp = TempDir::new().expect("tempdir");
        let args = base_submit_args("act_xyz");
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("malformed action id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- 4. load failure for an unknown action -----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_unknown_action_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        // Well-formed id that was never persisted.
        let args = base_submit_args("act_0123456789abcdef0123456789abcdef");
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("unknown action must surface a load (usage) error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- 5. intent gate ----------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_non_transfer_intent() {
        let tmp = TempDir::new().expect("tempdir");
        // A persisted APPROVE-intent action submitted through transfer submit.
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "approve",
            "eip155:1",
            Default::default(),
        );
        action.from_address = SIGNER_ADDR.to_string();
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        save_action(tmp.path(), &action);

        let args = base_submit_args(&action.action_id);
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("non-transfer intent rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("action is not a transfer intent"),
            "got: {err}"
        );
        // Status untouched.
        assert_eq!(persisted_status(tmp.path(), &action.action_id), "planned");
    }

    // --- 6. already-completed short-circuit --------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_already_completed_short_circuits_with_warning() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_transfer(tmp.path(), SIGNER_ADDR, "1000000").await;
        // Force the persisted action to completed without re-broadcasting.
        {
            let store = ActionStore::open(
                tmp.path().join("actions.db"),
                tmp.path().join("actions.lock"),
            )
            .expect("open store");
            let mut action = store.get(&action_id).expect("load");
            action.status = ActionStatus::Completed;
            store.save(&action).expect("persist completed");
        }

        let env = run_submit(tmp.path(), base_submit_args(&action_id))
            .await
            .expect("already-completed submit returns success without re-broadcast");
        assert!(env.success);
        assert_eq!(env.meta.command, "transfer submit");
        assert!(
            env.warnings.iter().any(|w| w == "action already completed"),
            "expected `action already completed` warning, got {:?}",
            env.warnings
        );
        let data = data_of(&env);
        assert_eq!(data["status"], Value::from("completed"));
    }

    // --- 7. legacy backend rejects a non-local signer ----------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_legacy_action_rejects_tempo_signer() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_transfer(tmp.path(), SIGNER_ADDR, "1000000").await;
        let mut args = base_submit_args(&action_id);
        args.signer = "tempo".to_string();
        args.private_key = None; // tempo signer + private key would be a different error
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("legacy action with --signer tempo rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("legacy actions only support --signer local"),
            "got: {err}"
        );
        assert_eq!(persisted_status(tmp.path(), &action_id), "planned");
    }

    // --- 8, 9. OWS backend offline guards ----------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_ows_action_missing_wallet_id_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        // A wallet-backed action with an EMPTY wallet_id (the guard precedes any
        // OWS resolve, so this is fully offline).
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "transfer",
            "eip155:1",
            Default::default(),
        );
        action.execution_backend = Some(ExecutionBackend::Ows);
        action.wallet_id = String::new();
        action.from_address = SIGNER_ADDR.to_string();
        save_action(tmp.path(), &action);

        let mut args = base_submit_args(&action.action_id);
        // No legacy signer flags (those would trip a different guard first).
        args.private_key = None;
        args.signer = "local".to_string();
        args.key_source = "auto".to_string();
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("OWS action without wallet_id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("wallet-backed action is missing persisted wallet_id"),
            "got: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_ows_action_rejects_legacy_signer_flags() {
        let tmp = TempDir::new().expect("tempdir");
        // A wallet-backed action WITH a persisted wallet_id, submitted with an
        // explicit legacy signer flag (--private-key).
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "transfer",
            "eip155:1",
            Default::default(),
        );
        action.execution_backend = Some(ExecutionBackend::Ows);
        action.wallet_id = "wallet-123".to_string();
        action.from_address = SIGNER_ADDR.to_string();
        save_action(tmp.path(), &action);

        let mut args = base_submit_args(&action.action_id);
        args.private_key = Some(TEST_KEY.to_string()); // explicit legacy flag
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("OWS action with legacy signer flags rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("wallet-backed actions do not accept legacy signer flags"),
            "got: {err}"
        );
    }

    // --- 10, 11. sender mismatch -------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_from_address_mismatch() {
        let tmp = TempDir::new().expect("tempdir");
        // Action sender matches the signer, but --from-address is a DIFFERENT addr.
        let action_id = plan_transfer(tmp.path(), SIGNER_ADDR, "1000000").await;
        let mut args = base_submit_args(&action_id);
        args.from_address = Some(OTHER_ADDR.to_string());
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("--from-address mismatch rejected");
        assert_eq!(err.code, Code::Signer);
        // Signer maps to exit 24 (spec §2.2).
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 24);
        assert_eq!(persisted_status(tmp.path(), &action_id), "planned");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_planned_sender_signer_mismatch() {
        let tmp = TempDir::new().expect("tempdir");
        // Planned action sender is OTHER_ADDR but the local signer is SIGNER_ADDR;
        // no --from-address supplied.
        let action_id = plan_transfer(tmp.path(), OTHER_ADDR, "1000000").await;
        let args = base_submit_args(&action_id);
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("planned-sender/signer mismatch rejected");
        assert_eq!(err.code, Code::Signer);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 24);
        assert_eq!(persisted_status(tmp.path(), &action_id), "planned");
    }

    // --- 12. execute-option validation -------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_gas_multiplier_not_greater_than_one() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_transfer(tmp.path(), SIGNER_ADDR, "1000000").await;
        let mut args = base_submit_args(&action_id);
        args.gas_multiplier = 1.0;
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("gas-multiplier <= 1 rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(err.to_string().contains("gas-multiplier"), "got: {err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_non_positive_poll_interval() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_transfer(tmp.path(), SIGNER_ADDR, "1000000").await;
        let mut args = base_submit_args(&action_id);
        args.poll_interval = "0s".to_string();
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("non-positive poll-interval rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_unparseable_step_timeout() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_transfer(tmp.path(), SIGNER_ADDR, "1000000").await;
        let mut args = base_submit_args(&action_id);
        args.step_timeout = "nope".to_string();
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("unparseable step-timeout rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- 13. signer init failure (no key) ----------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_signer_init_failure_is_signer_error() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_transfer(tmp.path(), SIGNER_ADDR, "1000000").await;
        let mut args = base_submit_args(&action_id);
        // Force an unresolvable key: source=env (isolates the env hex var) with no
        // --private-key override. The DEFI_PRIVATE_KEY env var is not set in this
        // test, so local-signer init must fail with a signer error.
        args.private_key = None;
        args.key_source = "env".to_string();
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("signer init with no key must fail");
        assert_eq!(err.code, Code::Signer);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 24);
        assert_eq!(persisted_status(tmp.path(), &action_id), "planned");
    }
}

#[cfg(test)]
mod status_app_tests {
    //! # Success criteria — `transfer status` app-level handler (WS4, exec-status)
    //!
    //! Go oracle: `internal/app/transfer_command.go` `statusCmd.RunE`. These tests
    //! drive [`cli::handle`] for `transfer status` ONLY. `transfer status` is a
    //! pure READ over the persisted action store (no signing, no network), so it is
    //! fully offline + deterministic. (Bridge destination-settlement polling — the
    //! only network-backed status transition — does NOT apply to `transfer`:
    //! transfer actions never carry a `bridge_send` step. That wait is owned by
    //! `bridge status` + `defi-execution::verify_bridge_settlement` and is NOT
    //! re-asserted here.) Structurally identical to `approvals status`, with the
    //! `transfer` intent gate (`action is not a transfer intent`).
    //!
    //! Criteria (each FAILING until `cli::handle` implements `transfer status`):
    //!
    //! 1. **Status success envelope reflects the persisted action.** Given a
    //!    persisted `transfer` action in `status == "planned"`, `transfer status
    //!    --action-id <id>` returns `Ok(Envelope)` (exit 0) with `version ==
    //!    "v1"`, `success == true`, `error == None`, `meta.command ==
    //!    "transfer status"`, `meta.cache == {status:"bypass", age_ms:0,
    //!    stale:false}` (execution paths bypass the cache, spec §2.5), and `data`
    //!    is the serialized Action with `action_id` == the requested id,
    //!    `intent_type == "transfer"`, and `status == "planned"`. (Go
    //!    `emitSuccess(..., action, nil, cacheMetaBypass(), nil, false)`.)
    //!
    //! 2. **Status reflects a `completed` transition.** After the persisted action
    //!    is advanced to `completed`, `transfer status` returns `data.status ==
    //!    "completed"` (status is a read of the persisted lifecycle, not a
    //!    re-execution).
    //!
    //! 3. **Status reflects a `running` transition.** A persisted action in
    //!    `running` is reported verbatim as `data.status == "running"`.
    //!
    //! 4. **Action-id validation.** `--action-id ""` → [`Code::Usage`] (exit 2);
    //!    a malformed id → [`Code::Usage`] (exit 2). (Go `resolveActionID`.)
    //!
    //! 5. **Load failure for an unknown action.** A well-formed but unknown
    //!    `--action-id` → [`Code::Usage`] (exit 2) (Go wraps the store `Get`
    //!    not-found as `clierr.Wrap(CodeUsage, "load action", err)`).
    //!
    //! 6. **Intent gate.** `transfer status` on a persisted NON-`transfer` action
    //!    (e.g. a `bridge` intent) → [`Code::Usage`] (exit 2) with `action is not
    //!    a transfer intent`. (Go `statusCmd` IntentType guard.)
    //!
    //! SKIPPED (covered elsewhere / wrong unit):
    //!   * bridge destination-settlement polling — `bridge status` unit;
    //!   * the action JSON shape internals — `defi-execution::action` golden;
    //!   * cache-bypass routing for `transfer status` — runner cache-flow concern
    //!     (`should_open_cache`), asserted here only via `meta.cache.status`.

    use super::cli::{handle, PlanArgs, TransferCmd};
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags, StatusArgs};
    use defi_config::Settings;
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::action::{Action, ActionStatus, ExecutionBackend};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;

    const SENDER: &str = "0x00000000000000000000000000000000000000aa";
    const RECIPIENT: &str = "0x00000000000000000000000000000000000000CC";

    fn exec_settings(dir: &Path) -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: false,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_millis(750),
            retries: 0,
            max_stale: Duration::from_secs(0),
            no_stale: false,
            cache_enabled: false,
            cache_path: dir.join("cache.db"),
            cache_lock_path: dir.join("cache.lock"),
            action_store_path: dir.join("actions.db"),
            action_lock_path: dir.join("actions.lock"),
            defillama_api_key: String::new(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    /// Plan + persist a canonical `transfer` action, returning its `action_id`.
    async fn plan_transfer(dir: &Path) -> String {
        let ctx = AppCtx::new(exec_settings(dir));
        let args = PlanArgs {
            chain: Some("1".to_string()),
            asset: Some("USDC".to_string()),
            recipient: Some(RECIPIENT.to_string()),
            amount: Some("1000000".to_string()),
            amount_decimal: None,
            rpc_url: None,
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(SENDER.to_string()),
            },
            input: InputFlags::default(),
        };
        let env = handle(&ctx, TransferCmd::Plan(args))
            .await
            .expect("plan a transfer action for the status fixture");
        env.data.expect("plan data")["action_id"]
            .as_str()
            .expect("action_id")
            .to_string()
    }

    fn save_action(dir: &Path, action: &Action) {
        let store = ActionStore::open(dir.join("actions.db"), dir.join("actions.lock"))
            .expect("open action store");
        store.save(action).expect("persist fixture action");
    }

    fn set_status(dir: &Path, action_id: &str, status: ActionStatus) {
        let store = ActionStore::open(dir.join("actions.db"), dir.join("actions.lock"))
            .expect("open store");
        let mut action = store.get(action_id).expect("load");
        action.status = status;
        store.save(&action).expect("persist status");
    }

    async fn run_status(dir: &Path, action_id: &str) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(
            &ctx,
            TransferCmd::Status(StatusArgs {
                action_id: Some(action_id.to_string()),
            }),
        )
        .await
    }

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn data_of(env: &Envelope) -> Value {
        env.data.clone().expect("status envelope carries `data`")
    }

    // --- 1. status success envelope ----------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn status_planned_emits_success_envelope() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_transfer(tmp.path()).await;
        let env = run_status(tmp.path(), &action_id)
            .await
            .expect("status on a planned transfer should succeed");

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "transfer status");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        let data = data_of(&env);
        assert_eq!(data["action_id"], Value::from(action_id.as_str()));
        assert_eq!(data["intent_type"], Value::from("transfer"));
        assert_eq!(data["status"], Value::from("planned"));
    }

    // --- 2, 3. status reflects lifecycle transitions -----------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn status_reflects_completed_transition() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_transfer(tmp.path()).await;
        set_status(tmp.path(), &action_id, ActionStatus::Completed);
        let env = run_status(tmp.path(), &action_id).await.expect("status ok");
        assert_eq!(data_of(&env)["status"], Value::from("completed"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn status_reflects_running_transition() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_transfer(tmp.path()).await;
        set_status(tmp.path(), &action_id, ActionStatus::Running);
        let env = run_status(tmp.path(), &action_id).await.expect("status ok");
        assert_eq!(data_of(&env)["status"], Value::from("running"));
    }

    // --- 4. action-id validation -------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn status_rejects_empty_action_id() {
        let tmp = TempDir::new().expect("tempdir");
        let err = run_status(tmp.path(), "")
            .await
            .expect_err("empty action id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn status_rejects_malformed_action_id() {
        let tmp = TempDir::new().expect("tempdir");
        let err = run_status(tmp.path(), "act_not_hex")
            .await
            .expect_err("malformed action id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- 5. load failure for an unknown action -----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn status_unknown_action_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        let err = run_status(tmp.path(), "act_0123456789abcdef0123456789abcdef")
            .await
            .expect_err("unknown action surfaces a load (usage) error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- 6. intent gate ----------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn status_rejects_non_transfer_intent() {
        let tmp = TempDir::new().expect("tempdir");
        // A persisted BRIDGE-intent action queried through transfer status.
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "bridge",
            "eip155:1",
            Default::default(),
        );
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        save_action(tmp.path(), &action);

        let err = run_status(tmp.path(), &action.action_id)
            .await
            .expect_err("non-transfer intent rejected by transfer status");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("action is not a transfer intent"),
            "got: {err}"
        );
    }
}
