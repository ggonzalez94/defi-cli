//! `approvals` command group handler (Go: `internal/app/approvals_command.go` —
//! `newApprovalsCommand`).
//!
//! This module owns the **approvals-command-specific** glue that sits between
//! the runner's cache-flow core ([`crate::runner`]), the shared
//! execution-identity resolver, and the action-build registry
//! ([`defi_execution::builder::Registry`]). The `approvals` group is a
//! standard-EVM execution command (an ERC-20 `approve(spender, amount)`): there
//! is no provider routing (`provider == "native"`). Specifically it owns:
//!
//! * the `approvals plan` request builder (`build_approval_request`) — the Go
//!   `buildAction` closure: parse `--chain` + `--asset`, default a non-positive
//!   asset `decimals` to `18`, normalize the amount against those decimals
//!   (carrying base + decimal forms consistently, spec §2.4), and assemble a
//!   [`defi_execution::planner::ApprovalRequest`] carrying sender / spender /
//!   simulate / rpc-url verbatim;
//! * the `approvals plan` schema identity input constraints
//!   (`approvals_plan_identity_constraints`: the standard
//!   `exactly_one_of {wallet, from_address}`, with no per-provider `when`
//!   branching — approval planning is OWS-first / standard EVM, like transfer);
//! * the persisted-intent gate (`ensure_approve_intent`: `approvals submit` /
//!   `approvals status` reject a non-`approve` action with a usage error).
//!
//! NOT re-owned here (consumed from elsewhere):
//! * the approval **action construction + validation** (sender/spender/token hex
//!   validation, positive-amount enforcement, calldata packing) — owned by
//!   `defi_execution::planner::build_approval_action` and covered by its own RED
//!   suite (ported from `planner/approvals_test.go`);
//! * the action-build registry routing (`Registry::build_approval_action`) —
//!   owned by `defi_execution::builder` (B8);
//! * the shared execution-identity resolver (`resolve_execution_identity`) and
//!   its OWS/legacy backend stamping — owned by the shared execution-identity
//!   module / [`crate::runner`];
//! * the submit signer/backend plumbing, bounded-approval pre-sign guardrails,
//!   and receipt polling — `defi-execution` concern;
//! * the cache-key construction + cache bypass for execution paths — runner
//!   concern, owned by [`crate::runner`].

#![allow(dead_code, unused_variables)]

use defi_errors::{Code, Error};
use defi_execution::planner::ApprovalRequest;
use defi_id::{normalize_amount, parse_asset, parse_chain};
use defi_schema::InputConstraint;

/// Build an [`ApprovalRequest`] from the raw `approvals plan` flags.
///
/// Parity with the Go `buildAction` closure in `approvals_command.go`:
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
/// 4. assemble the [`ApprovalRequest`] carrying the resolved sender
///    (`from_address`), spender, simulate flag, and rpc-url verbatim.
///
/// The sender / spender / token hex validation and positive-amount enforcement
/// are NOT performed here — they belong to
/// `defi_execution::planner::build_approval_action`, which consumes this
/// request.
// The flag-derived inputs map 1:1 onto the Go approval `buildAction` args; this
// is the locked public signature the RED suite + callers depend on, so the
// argument count is intentional rather than a struct-grouping opportunity.
#[allow(clippy::too_many_arguments)]
pub fn build_approval_request(
    chain_arg: &str,
    asset_arg: &str,
    spender: &str,
    amount_base: &str,
    amount_decimal: &str,
    from_address: &str,
    simulate: bool,
    rpc_url: &str,
) -> Result<ApprovalRequest, Error> {
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

    Ok(ApprovalRequest {
        chain,
        asset,
        amount_base_units: base,
        sender: from_address.to_string(),
        spender: spender.to_string(),
        simulate,
        rpc_url: rpc_url.to_string(),
    })
}

/// The `approvals plan` schema identity input constraints.
///
/// Parity with Go `standardExecutionIdentityInputConstraints` (advertised by
/// `approvals plan` via `configureStructuredInput`): a single `exactly_one_of`
/// entry over `[wallet, from_address]` with no `when` clause — approval planning
/// is OWS-first / standard EVM, with no per-provider identity branching (unlike
/// swap's Tempo/TaikoSwap split).
pub fn approvals_plan_identity_constraints() -> Vec<InputConstraint> {
    vec![InputConstraint {
        kind: "exactly_one_of".to_string(),
        fields: vec!["wallet".to_string(), "from_address".to_string()],
        when: Default::default(),
        description: "Provide exactly one execution identity input: `wallet` \
                      (OWS, recommended) or `from_address` (local signer)."
            .to_string(),
    }]
}

/// Validate that a persisted action is an `approve` intent.
///
/// Parity with the `submit` / `status` guard `action.IntentType != "approve"`
/// in `approvals_command.go`: a non-`approve` intent yields a
/// [`defi_errors::Code::Usage`] error whose message is
/// `action is not an approval intent`.
pub fn ensure_approve_intent(intent_type: &str) -> Result<(), Error> {
    if intent_type != "approve" {
        return Err(Error::new(Code::Usage, "action is not an approval intent"));
    }
    Ok(())
}

/// clap parsing + handler for the `approvals` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::{Code, Error};
    use defi_execution::builder::Registry;
    use defi_model::{Envelope, ProviderStatus};

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, SubmitArgs};
    use crate::execident::{apply_execution_identity_to_action, resolve_execution_identity};

    /// `approvals` subcommands (Go `newApprovalsCommand`).
    #[derive(Subcommand, Debug)]
    pub enum ApprovalsCmd {
        /// Create and persist an approval action plan.
        Plan(PlanArgs),
        /// Execute an existing approval action.
        Submit(SubmitArgs),
        /// Get approval action status.
        Status(StatusArgs),
    }

    impl ApprovalsCmd {
        /// The leaf path token (for `meta.command`).
        pub fn path(&self) -> &'static str {
            match self {
                ApprovalsCmd::Plan(_) => "plan",
                ApprovalsCmd::Submit(_) => "submit",
                ApprovalsCmd::Status(_) => "status",
            }
        }
    }

    /// `approvals plan` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct PlanArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Asset symbol/address/CAIP-19.
        #[arg(long)]
        pub asset: Option<String>,
        /// Spender address.
        #[arg(long)]
        pub spender: Option<String>,
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

    /// Handle `approvals <sub>`.
    pub async fn handle(ctx: &AppCtx, cmd: ApprovalsCmd) -> Result<Envelope, Error> {
        match cmd {
            ApprovalsCmd::Plan(args) => handle_plan(ctx, args).await,
            ApprovalsCmd::Submit(args) => handle_submit(ctx, args).await,
            ApprovalsCmd::Status(args) => handle_status(ctx, args).await,
        }
    }

    /// Handle `approvals plan` (Go `planCmd.RunE` in `approvals_command.go`).
    ///
    /// Flow parity with the Go runner:
    /// 1. resolve the execution identity (OWS `--wallet` first / legacy
    ///    `--from-address`) on the requested chain; an identity error returns the
    ///    typed [`Error`] before anything is persisted;
    /// 2. build the [`ApprovalRequest`] from the flags + the resolved sender
    ///    ([`super::build_approval_request`]: chain/asset parse, decimals
    ///    defaulting to 18, amount normalization carrying base + decimal forms);
    /// 3. compose the single-step `approve` action via the action-build registry
    ///    ([`Registry::build_approval_action`] → `planner::build_approval_action`),
    ///    capturing a synthetic `native` provider status (Go `statusFromErr`);
    /// 4. stamp the resolved identity (wallet id/name, from-address, execution
    ///    backend) onto the action and persist it to the action [`Store`];
    /// 5. emit the success envelope with the identity warnings, the cache
    ///    bypassed (execution paths skip the cache, spec §2.5), and the `native`
    ///    provider status.
    ///
    /// [`Store`]: defi_execution::store::Store
    /// [`ApprovalRequest`]: defi_execution::planner::ApprovalRequest
    async fn handle_plan(ctx: &AppCtx, args: PlanArgs) -> Result<Envelope, Error> {
        // 0. Merge structured input (`--input-json` / `--input-file`) onto the
        //    parsed flags before any guard (Go PreRunE `applyStructuredFlagInput`
        //    over `approvalArgs`). Explicit flags win; unknown key / null → usage.
        let mut args = args;
        merge_plan_input(&mut args)?;

        let chain_arg = args.chain.as_deref().unwrap_or_default();
        let wallet_ref = args.identity.wallet.as_deref().unwrap_or_default();
        let from_flag = args.identity.from_address.as_deref().unwrap_or_default();

        // 1. Resolve the execution identity (returns before any persistence on
        //    error — both / neither input, malformed address, Tempo/non-EVM
        //    --wallet, OWS resolve failures).
        let identity = resolve_execution_identity(wallet_ref, from_flag, chain_arg)?;

        // 2. Build the approval request against the resolved sender.
        let request = super::build_approval_request(
            chain_arg,
            args.asset.as_deref().unwrap_or_default(),
            args.spender.as_deref().unwrap_or_default(),
            args.amount.as_deref().unwrap_or_default(),
            args.amount_decimal.as_deref().unwrap_or_default(),
            &identity.from_address,
            args.simulate,
            args.rpc_url.as_deref().unwrap_or_default(),
        )?;

        // 3. Compose the action via the registry (approval routes straight to the
        //    planner; no provider routing — `provider == "native"`). A build error
        //    is returned (the runner renders the full error envelope to stderr).
        let mut action = Registry::new().build_approval_action(request)?;

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
        let mut env = ctx.metadata_envelope("approvals plan", data, providers);
        env.warnings = identity.warnings;
        Ok(env)
    }

    /// Handle `approvals submit` (Go `submitCmd.RunE` in `approvals_command.go`).
    ///
    /// Flow parity with the Go runner:
    /// 1. resolve + validate the `--action-id` ([`crate::actions::resolve_action_id`]);
    /// 2. load the persisted action from the action [`Store`]; a not-found load
    ///    surfaces as a [`Code::Usage`] `load action` error (Go
    ///    `clierr.Wrap(CodeUsage, "load action", err)`);
    /// 3. gate the intent (`approve`-only — [`super::ensure_approve_intent`]);
    /// 4. short-circuit an already-`completed` action (success + warning, no
    ///    re-broadcast);
    /// 5. resolve the execution backend from the persisted
    ///    `execution_backend` (legacy-local / OWS) and the submit signer flags,
    ///    rejecting unsupported combinations (legacy + non-local signer, OWS
    ///    without `wallet_id`, OWS + legacy signer flags);
    /// 6. validate the resolved signer against `--from-address` + the persisted
    ///    planned sender ([`Code::Signer`] on mismatch);
    /// 7. parse the execute options (`--gas-multiplier > 1`, durations, fee
    ///    flags);
    /// 8. run the bounded-approval pre-sign guardrail with the action context
    ///    (inflated approval without `--allow-max-approval` → [`Code::ActionPlan`]);
    /// 9. broadcast through the engine ([`defi_execution::evm_executor::execute_action`]),
    ///    persisting each transition; and emit the terminal-state envelope.
    ///
    /// [`Store`]: defi_execution::store::Store
    async fn handle_submit(ctx: &AppCtx, args: SubmitArgs) -> Result<Envelope, Error> {
        // 1. Resolve + validate the action id.
        let action_id =
            crate::actions::resolve_action_id(args.action_id.as_deref().unwrap_or_default())?;

        // 2. Load the persisted action (not-found → usage `load action`).
        let store = ctx.open_action_store()?;
        let mut action = store
            .get(&action_id)
            .map_err(|e| Error::wrap(Code::Usage, "load action", e))?;

        // 3. Intent gate (approve-only).
        super::ensure_approve_intent(&action.intent_type)?;

        // 4. Already-completed short-circuit (no re-broadcast).
        if action.status == defi_execution::action::ActionStatus::Completed {
            let data = serde_json::to_value(&action)
                .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
            let mut env =
                ctx.metadata_envelope("approvals submit", data, Vec::<ProviderStatus>::new());
            env.warnings = vec!["action already completed".to_string()];
            return Ok(env);
        }

        // 5. Resolve the execution backend + signer (legacy-local / OWS guards).
        let resolved = crate::execsubmit::resolve_action_execution_backend(
            &action,
            crate::execsubmit::SubmitExecutionInputs {
                signer: &args.signer,
                key_source: &args.key_source,
                private_key: args.private_key.as_deref().unwrap_or_default(),
                from_address: args.from_address.as_deref().unwrap_or_default(),
            },
        )?;

        // 6. Validate the resolved sender vs --from-address + planned sender.
        crate::execsubmit::validate_execution_sender(
            &action,
            args.from_address.as_deref().unwrap_or_default(),
            &resolved.sender,
        )?;

        // 7. Parse the execute options (durations, gas multiplier, fee flags).
        let opts =
            crate::execsubmit::parse_execute_options(&crate::execsubmit::ExecuteOptionInputs {
                simulate: args.simulate,
                poll_interval: &args.poll_interval,
                step_timeout: &args.step_timeout,
                gas_multiplier: args.gas_multiplier,
                max_fee_gwei: args.max_fee_gwei.as_deref().unwrap_or_default(),
                max_priority_fee_gwei: args.max_priority_fee_gwei.as_deref().unwrap_or_default(),
                allow_max_approval: args.allow_max_approval,
                unsafe_provider_tx: args.unsafe_provider_tx,
                fee_token: args.fee_token.as_deref().unwrap_or_default(),
            })?;

        // 8. Bounded-approval pre-sign guardrail (run with action context so an
        //    inflated approval yields the documented `allow-max-approval` hint;
        //    the engine's per-step policy runs without action context).
        crate::execsubmit::presign_validate_action(&action, &opts)?;

        // 9. Broadcast through the engine (persisting each transition), then emit
        //    the terminal-state envelope (cache bypassed for execution paths).
        crate::execsubmit::execute_resolved(&store, &mut action, resolved, opts).await?;

        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
        Ok(ctx.metadata_envelope("approvals submit", data, Vec::<ProviderStatus>::new()))
    }

    /// Handle `approvals status` (Go `statusCmd.RunE` in `approvals_command.go`).
    ///
    /// A pure read over the persisted action store: resolve + validate the
    /// `--action-id`, load the action (not-found → usage `load action`), gate the
    /// intent (`approve`-only), and emit the action verbatim (cache bypassed).
    async fn handle_status(ctx: &AppCtx, args: StatusArgs) -> Result<Envelope, Error> {
        let action_id =
            crate::actions::resolve_action_id(args.action_id.as_deref().unwrap_or_default())?;
        let store = ctx.open_action_store()?;
        let action = store
            .get(&action_id)
            .map_err(|e| Error::wrap(Code::Usage, "load action", e))?;
        super::ensure_approve_intent(&action.intent_type)?;
        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
        Ok(ctx.metadata_envelope("approvals status", data, Vec::<ProviderStatus>::new()))
    }

    /// Merge structured input (`--input-json` / `--input-file`) onto the parsed
    /// `approvals plan` flags (Go PreRunE `applyStructuredFlagInput` over
    /// `approvalArgs`). Explicitly-set flags are never overridden; an unknown key
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
        if args.spender.is_some() {
            explicit.insert("spender");
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
            "approvals plan",
            |key, canonical, raw| {
                match canonical {
                    "chain" => args.chain = Some(decode_string_field(key, raw)?),
                    "asset" => args.asset = Some(decode_string_field(key, raw)?),
                    "spender" => args.spender = Some(decode_string_field(key, raw)?),
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
    //! # Success criteria — `defi-app::approvals` (Go: `internal/app` approvals
    //! command group: `newApprovalsCommand` in `approvals_command.go`)
    //!
    //! This module owns the **approvals-command glue**. "Correct" means it
    //! preserves the runner-owned approval behaviors AND the stable machine
    //! contract (design spec §2.2 exit codes, §2.4 ids/amounts kept consistent,
    //! §2.5 OWS-first standard-EVM execution identity). The approval action
    //! construction + validation (`build_approval_action`, with sender/spender/
    //! token hex + positive-amount validation — covered by the
    //! `defi-execution::planner` RED suite ported from `planner/approvals_test.go`),
    //! the registry routing (`Registry::build_approval_action`, B8), the shared
    //! execution-identity resolver, the submit signer/backend plumbing
    //! (incl. the bounded-approval / `--allow-max-approval` pre-sign guardrail),
    //! and the cache-flow core are owned elsewhere and are NOT re-asserted here.
    //! Criteria:
    //!
    //! 1. **Request building + amount normalization.** `build_approval_request`
    //!    mirrors the Go `buildAction` closure.
    //!    (a) `--chain` + `--asset` parse to the chain CAIP-2 id and the asset on
    //!        that chain (USDC on Ethereum mainnet → 6 decimals, asset_id
    //!        `eip155:1/erc20:0xa0b8...eb48`).
    //!    (b) The amount is normalized against the asset's decimals: base
    //!        `1000000` (USDC, 6 decimals) ⇔ decimal `1` stay consistent (spec
    //!        §2.4); the decimal form `1` normalizes back to base `1000000`.
    //!    (c) The resolved sender (`from_address`), spender, simulate flag, and
    //!        rpc-url are carried verbatim onto the [`ApprovalRequest`].
    //!    (Mirrors the request-build half of the Go `approvals plan` path, whose
    //!    persisted action is exercised by the Go oracle:
    //!    `approvals plan --chain 1 --asset USDC --amount 1000000` →
    //!    `intent_type: "approve"`, `input_amount: "1000000"`.)
    //!
    //! 2. **Decimals defaulting to 18.** When the parsed asset's `decimals` is
    //!    non-positive (e.g. a bare token address with no registry entry, parsed
    //!    on an EVM chain), `build_approval_request` normalizes the amount as if
    //!    `decimals == 18` — distinct from the planner, which performs no
    //!    defaulting. A decimal amount of `1` therefore yields base
    //!    `1000000000000000000`. (Go `buildAction`: `if decimals <= 0 { decimals
    //!    = 18 }`.)
    //!
    //! 3. **Amount cross-validation is a usage error.** Supplying BOTH `--amount`
    //!    and `--amount-decimal` → [`Code::Usage`] (exit 2); supplying NEITHER →
    //!    [`Code::Usage`] (exit 2). (Delegated to `defi_id::normalize_amount`,
    //!    spec §2.4, asserted here because the approval builder owns the call. The
    //!    Go oracle returns `use either --amount or --amount-decimal, not both`
    //!    with `code: 2` for the both-forms case.)
    //!
    //! 4. **`approvals plan` schema identity constraints.**
    //!    `approvals_plan_identity_constraints` returns EXACTLY one
    //!    `exactly_one_of` entry over `[wallet, from_address]` with no `when`
    //!    clause — the standard OWS-first execution identity (no per-provider
    //!    branching, unlike swap). (Parity with `approvals_command.go` wiring
    //!    `InputConstraints: standardExecutionIdentityInputConstraints()`.)
    //!
    //! 5. **Persisted-intent gate.** `ensure_approve_intent` accepts `"approve"`
    //!    and rejects any other intent with [`Code::Usage`] (exit 2) + `action is
    //!    not an approval intent`. (Ported from the `submit` / `status`
    //!    `IntentType != "approve"` guards in `approvals_command.go`; the runner
    //!    test `TestRunnerExecutionStatusBypassesCacheOpen` exercises the
    //!    `approvals status` usage-exit path.)
    //!
    //! SKIPPED (Go internal-detail / wrong-module):
    //!   * cobra flag wiring + flag defaults (`--simulate true`, `--signer
    //!     local`, `--key-source auto`, `--gas-multiplier 1.2`, `--poll-interval
    //!     2s`, `--step-timeout 2m`, required-flag marking for
    //!     `--chain`/`--asset`/`--spender`) — harness concern, asserted by the
    //!     integration golden-CLI / schema suites
    //!     (`TestRunnerExecutionCommandsInSchema` covers `approvals plan` /
    //!     `approvals submit` schema presence), not this unit;
    //!   * the approval sender/spender/token hex validation, positive-amount
    //!     enforcement, and calldata packing (`0x095ea7b3…` approve selector) —
    //!     owned by `defi_execution::planner::build_approval_action` (ported from
    //!     `planner/approvals_test.go`: `TestBuildApprovalAction`,
    //!     `TestBuildApprovalActionRejectsInvalidAmount`);
    //!   * the registry routing for the `approve` intent — owned by
    //!     `defi_execution::builder` (B8);
    //!   * the bounded-ERC20-approval pre-sign guardrail +
    //!     `--allow-max-approval` opt-in (`runner_actions_test.go`
    //!     `AllowMaxApproval` parse) — `defi-execution` submit/options concern;
    //!   * `shouldOpenActionStore("approvals submit")` /
    //!     `shouldOpenCache("approvals status")` routing
    //!     (`TestShouldOpenActionStore`,
    //!     `TestShouldOpenCacheBypassesExecutionCommands`) — runner cache-flow
    //!     concern, owned by [`crate::runner`];
    //!   * the OWS-vs-legacy execution-backend stamping + wallet-id persistence
    //!     and submit auth metadata (OWS-token first, legacy signer compat) —
    //!     shared execution-identity / schema-auth concern;
    //!   * the structured `--input-json` parsing + already-completed
    //!     short-circuit — structured-input / action-store concern.

    use super::*;
    use defi_errors::{exit_code, Code};

    // --- helpers -----------------------------------------------------------

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    // A canonical-but-arbitrary EVM sender/spender pair (not validated by the
    // request builder — that's the planner's job — but carried verbatim).
    const SENDER: &str = "0x00000000000000000000000000000000000000aa";
    const SPENDER: &str = "0x1111111111111111111111111111111111111111";

    // --- 1. request building + amount normalization ------------------------

    #[test]
    fn build_request_parses_chain_asset_and_normalizes_base_amount() {
        // USDC (6 decimals) approval on Ethereum mainnet with a base-unit amount.
        let req = build_approval_request(
            "1",
            "USDC",
            SPENDER,
            "1000000",
            "",
            SENDER,
            true,
            "http://127.0.0.1:8545",
        )
        .expect("approval request built");
        assert_eq!(req.chain.caip2, "eip155:1");
        assert_eq!(req.asset.symbol, "USDC");
        assert_eq!(req.asset.decimals, 6);
        assert_eq!(
            req.asset.asset_id,
            "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
        );
        // base ⇔ decimal stay consistent (spec §2.4).
        assert_eq!(req.amount_base_units, "1000000");
        // sender / spender / simulate / rpc carried verbatim.
        assert_eq!(req.sender, SENDER);
        assert_eq!(req.spender, SPENDER);
        assert!(req.simulate);
        assert_eq!(req.rpc_url, "http://127.0.0.1:8545");
    }

    #[test]
    fn build_request_normalizes_decimal_amount_against_asset_decimals() {
        // The decimal form normalizes to base units against USDC decimals (6).
        let req = build_approval_request("1", "USDC", SPENDER, "", "1", SENDER, true, "")
            .expect("decimal amount normalizes");
        assert_eq!(req.amount_base_units, "1000000");
        assert_eq!(req.asset.decimals, 6);
    }

    #[test]
    fn build_request_carries_simulate_false() {
        let req = build_approval_request("1", "USDC", SPENDER, "1000000", "", SENDER, false, "")
            .expect("simulate=false carried");
        assert!(!req.simulate);
    }

    // --- 2. decimals defaulting to 18 --------------------------------------

    #[test]
    fn build_request_defaults_decimals_to_18_for_unknown_token() {
        // A bare contract address with no registry symbol parses on an EVM chain
        // but carries non-positive decimals; the approval builder defaults to 18
        // (Go `buildAction`), so a decimal amount of 1 yields 1e18 base units.
        let token = "0x2222222222222222222222222222222222222222";
        let req = build_approval_request("1", token, SPENDER, "", "1", SENDER, true, "")
            .expect("decimals default to 18");
        assert_eq!(
            req.amount_base_units, "1000000000000000000",
            "decimal 1 against defaulted 18 decimals => 1e18 base units"
        );
    }

    // --- 3. amount cross-validation ----------------------------------------

    #[test]
    fn build_request_rejects_both_amount_forms() {
        let err = build_approval_request("1", "USDC", SPENDER, "1000000", "1", SENDER, true, "")
            .expect_err("both amount forms rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    #[test]
    fn build_request_rejects_missing_amount() {
        let err = build_approval_request("1", "USDC", SPENDER, "", "", SENDER, true, "")
            .expect_err("missing amount rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- 4. approvals plan schema identity constraints ---------------------

    #[test]
    fn plan_identity_constraints_are_standard_exactly_one_of() {
        let constraints = approvals_plan_identity_constraints();
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].kind, "exactly_one_of");
        assert_eq!(
            constraints[0].fields,
            vec!["wallet".to_string(), "from_address".to_string()]
        );
        // No per-provider `when` clause — approval planning is OWS-first /
        // standard EVM (no Tempo/TaikoSwap-style branching like swap).
        assert!(
            constraints[0].when.is_empty(),
            "standard identity constraint has no `when` clause"
        );
    }

    // --- 5. persisted-intent gate ------------------------------------------

    #[test]
    fn ensure_approve_intent_accepts_approve() {
        ensure_approve_intent("approve").expect("approve intent accepted");
    }

    #[test]
    fn ensure_approve_intent_rejects_non_approve() {
        // A swap action submitted/queried through `approvals submit|status` fails.
        let err = ensure_approve_intent("swap").expect_err("non-approve intent rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("action is not an approval intent"),
            "got: {err}"
        );
    }

    #[test]
    fn ensure_approve_intent_rejects_transfer() {
        // Guard is intent-specific: a `transfer` action is not an approval.
        let err = ensure_approve_intent("transfer").expect_err("transfer intent rejected");
        assert_eq!(err.code, Code::Usage);
    }
}

#[cfg(test)]
mod app_tests {
    //! # Success criteria — `approvals plan` app-level handler (WS3, exec-plan)
    //!
    //! Go oracle: `internal/app/approvals_command.go` `planCmd.RunE`. These tests
    //! drive [`cli::handle`] (the real dispatch entry point the binary calls)
    //! end-to-end for `approvals plan` ONLY, asserting the full machine contract
    //! the Go runner emits via `emitSuccess(...)` / `renderError(...)`. They are
    //! offline + deterministic: an ERC-20 `approve(spender, amount)` action is
    //! built entirely from calldata (the planner does NOT connect to RPC for
    //! approvals — `--rpc-url` / the registry default RPC is only carried onto the
    //! step), and persistence uses a real [`defi_execution::store::Store`] over a
    //! `tempfile` directory. No wiremock network is required for the approve build
    //! itself; the base-URL / `--rpc-url` seams exist but no provider HTTP call is
    //! made on this path. Identity is exercised through the OFFLINE `--from-address`
    //! (legacy_local) path so no OWS vault / network is touched; the `--wallet`
    //! happy path (OWS resolve) is WS4b e2e territory and is asserted here only via
    //! its offline guard rejections.
    //!
    //! Criteria (each a failing test until `cli::handle` is implemented):
    //!
    //! 1. **Plan success envelope (legacy `--from-address`).** A valid
    //!    `approvals plan --chain 1 --asset USDC --spender 0x..BB --amount 1000000
    //!    --from-address 0x..aa` returns an `Ok(Envelope)` (exit 0) with:
    //!    `version == "v1"`, `success == true`, `error == None`, `meta.partial ==
    //!    false`, `meta.command == "approvals plan"`,
    //!    `meta.cache == {status:"bypass", age_ms:0, stale:false}` (execution paths
    //!    bypass the cache, spec §2.5), and `meta.providers == [{name:"native",
    //!    status:"ok"}]` (Go `statusFromErr(nil) == "ok"`; approval has no provider
    //!    routing — `provider == "native"`).
    //!
    //! 2. **Planned action `data` shape.** `env.data` is the serialized [`Action`]:
    //!    `action_id` matches `^act_[0-9a-f]{32}$`; `intent_type == "approve"`;
    //!    `provider == "native"`; `status == "planned"`; `chain_id == "eip155:1"`;
    //!    `from_address` == the EIP-55 checksum of the sender; `to_address` ==
    //!    the spender address; `input_amount == "1000000"`; exactly ONE step with
    //!    `type == "approval"`, `value == "0"`, `target` == the USDC token address,
    //!    and `chain_id == "eip155:1"`. (Mirrors the Go oracle: `approvals plan
    //!    --chain 1 --asset USDC --amount 1000000` → `intent_type:"approve"`,
    //!    `input_amount:"1000000"`.)
    //!
    //! 3. **Step calldata reuses the `defi-evm` ABI golden.** With spender
    //!    `0x00000000000000000000000000000000000000BB` and amount `1000000`, the
    //!    step `data` equals the pinned ERC-20 `approve` calldata golden
    //!    (`defi-evm` `encode_erc20_approve_matches_golden`):
    //!    `0x095ea7b3` + spender(32) + `0xf4240`(=1000000, 32). This proves the
    //!    handler routes through `build_approval_action` (no re-encoding).
    //!
    //! 4. **Bounded-approval plan invariant.** The planned `input_amount` and the
    //!    step calldata amount equal EXACTLY the requested amount (`1000000`), with
    //!    no max/unbounded substitution at plan time. (The `--allow-max-approval`
    //!    opt-in is a SUBMIT-time pre-sign guardrail, WS4 — plan never inflates the
    //!    bound; the plan side of the bounded-approval contract is "persist exactly
    //!    what was requested".)
    //!
    //! 5. **Legacy-identity warning surfaces in the envelope.** The
    //!    `--from-address` path stamps `execution_backend == "legacy_local"` on the
    //!    action AND surfaces the Go warning
    //!    `--wallet (OWS) is recommended over --from-address for planning; see docs
    //!    for details` in `env.warnings`. (Go `resolveExecutionIdentity` legacy
    //!    branch + `emitSuccess(..., identity.Warnings, ...)`.)
    //!
    //! 6. **Plan persists the action to the Store.** After a successful plan the
    //!    action is retrievable by its `action_id` from a freshly opened
    //!    [`defi_execution::store::Store`] over the same path, with matching
    //!    `intent_type == "approve"` and `input_amount`. (Go `s.actionStore.Save`.)
    //!
    //! 7. **Decimal amount parity.** `--amount-decimal 1` (no `--amount`) on USDC
    //!    (6 decimals) yields the same `input_amount == "1000000"` and the same
    //!    calldata golden — base ⇔ decimal stay consistent (spec §2.4).
    //!
    //! 8. **Identity-constraint errors (offline).**
    //!    (a) BOTH `--wallet` and `--from-address` → [`Code::Usage`] (exit 2);
    //!    (b) NEITHER `--wallet` nor `--from-address` → [`Code::Usage`] (exit 2);
    //!    (c) a malformed `--from-address` → [`Code::Usage`] (exit 2);
    //!    (d) `--wallet` on a Tempo chain → [`Code::Unsupported`] (exit 13)
    //!        (`--wallet planning is not supported on Tempo chains yet`).
    //!    (Go `resolveExecutionIdentity`.) On every error the handler returns the
    //!    typed `Err(Error)` (the runner renders the full error envelope to stderr,
    //!    spec §2.1) and persists NOTHING to the Store.
    //!
    //! 9. **Amount cross-validation through the handler.** BOTH `--amount` +
    //!    `--amount-decimal` → [`Code::Usage`] (exit 2); NEITHER → [`Code::Usage`]
    //!    (exit 2). (Delegated to `defi_id::normalize_amount` via
    //!    `build_approval_request`; asserted at the handler boundary.)
    //!
    //! 10. **Planner validation surfaces through the handler.**
    //!     (a) a malformed `--spender` → [`Code::Usage`] (exit 2)
    //!         (`build_approval_action` spender hex validation);
    //!     (b) a non-positive `--amount` (`0`) → [`Code::Usage`] (exit 2)
    //!         (`approval amount must be a positive integer in base units`).
    //!     On both, nothing is persisted.
    //!
    //! SKIPPED (covered elsewhere / wrong unit):
    //!   * the `approve` calldata ABI encoding itself — `defi-evm::abi` golden;
    //!   * `build_approval_action` sender/spender/token hex + positive-amount
    //!     internals — `defi-execution::planner` RED suite;
    //!   * the `--allow-max-approval` / bounded-ERC20 pre-sign guardrail at
    //!     SUBMIT time — WS4 (`approvals submit`), a `defi-execution` concern;
    //!   * the OWS `--wallet` happy-path resolve + wallet-id persistence — WS4b
    //!     e2e (here only its offline guard rejections are asserted);
    //!   * `--input-json`/`--input-file` precedence — structured-input unit;
    //!   * cobra/clap flag defaults + required-flag marking — schema/CLI suites.

    use super::cli::{handle, ApprovalsCmd, PlanArgs};
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
    /// Spender matching the `defi-evm` `encode_erc20_approve_matches_golden`
    /// fixture (`SPENDER = 0x..BB`), so the planned step `data` reuses that golden.
    const SPENDER: &str = "0x00000000000000000000000000000000000000BB";
    /// USDC contract on Ethereum mainnet (6 decimals) — resolved by `parse_asset`.
    const USDC_MAINNET: &str = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
    /// The pinned ERC-20 `approve(0x..BB, 1000000)` calldata (defi-evm golden).
    const APPROVE_CALLDATA_GOLDEN: &str = "0x095ea7b300000000000000000000000000000000000000000000000000000000000000bb00000000000000000000000000000000000000000000000000000000000f4240";
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
            spender: Some(SPENDER.to_string()),
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
        handle(&ctx, ApprovalsCmd::Plan(args)).await
    }

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn action_data(env: &Envelope) -> Value {
        env.data.clone().expect("plan envelope carries `data`")
    }

    // --- 1, 2, 4, 5. plan success envelope + action shape ------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_legacy_from_address_emits_success_envelope() {
        let tmp = TempDir::new().expect("tempdir");
        let env = run_plan(tmp.path(), base_plan_args())
            .await
            .expect("approvals plan should succeed on the legacy path");

        // Envelope contract (Go `emitSuccess`).
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "approvals plan");

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
        assert_eq!(data["intent_type"], Value::from("approve"));
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
            SPENDER.to_lowercase(),
            "to_address is the spender"
        );
        // Bounded-approval plan invariant: persist EXACTLY the requested amount.
        assert_eq!(data["input_amount"], Value::from("1000000"));

        // Exactly one approval step, value 0, target = token, chain carried.
        let steps = data["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1, "approval is a single-step action");
        assert_eq!(steps[0]["type"], Value::from("approval"));
        assert_eq!(steps[0]["value"], Value::from("0"));
        assert_eq!(steps[0]["chain_id"], Value::from("eip155:1"));
        assert_eq!(
            steps[0]["target"].as_str().unwrap().to_lowercase(),
            USDC_MAINNET,
            "approval step targets the USDC token contract"
        );

        // Legacy backend stamping + warning (criterion 5).
        assert_eq!(data["execution_backend"], Value::from("legacy_local"));
        assert!(
            env.warnings.iter().any(|w| w == LEGACY_WARNING),
            "legacy --from-address plan surfaces the OWS-recommended warning; got {:?}",
            env.warnings
        );
    }

    // --- structured input (`--input-json` / `--input-file`) ----------------
    //
    // Go: `configureStructuredInput[approvalArgs]` wires the PreRunE merge onto
    // `approvals plan`. JSON fills flags; explicit flags override JSON; unknown
    // keys / null values are usage errors that persist nothing.

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_resolves_all_flags_from_input_json() {
        let tmp = TempDir::new().expect("tempdir");
        let args = PlanArgs {
            input: InputFlags {
                input_json: Some(format!(
                    r#"{{"chain":"1","asset":"USDC","spender":"{SPENDER}","amount":"1000000","from_address":"{SENDER}"}}"#
                )),
                input_file: None,
            },
            ..PlanArgs::default()
        };
        let env = run_plan(tmp.path(), args)
            .await
            .expect("input-json should fill all flags and the plan should succeed");
        assert!(env.success);
        assert_eq!(env.meta.command, "approvals plan");
        let data = action_data(&env);
        assert_eq!(data["intent_type"], Value::from("approve"));
        // The approval step calldata still matches the pinned golden, proving the
        // spender/amount were taken from the JSON.
        assert_eq!(
            data["steps"][0]["data"].as_str().expect("step data"),
            APPROVE_CALLDATA_GOLDEN
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_explicit_flag_overrides_input_json() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_plan_args();
        // Explicit asset stays USDC; JSON tries to flip it to a bogus symbol — the
        // explicit flag must win, so the plan still succeeds on USDC.
        args.input = InputFlags {
            input_json: Some(r#"{"asset":"NOT_A_REAL_TOKEN"}"#.to_string()),
            input_file: None,
        };
        let env = run_plan(tmp.path(), args)
            .await
            .expect("explicit --asset must win over the JSON asset");
        assert!(env.success);
        let data = action_data(&env);
        assert_eq!(
            data["steps"][0]["data"].as_str().expect("step data"),
            APPROVE_CALLDATA_GOLDEN
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_input_json_unknown_field_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        let args = PlanArgs {
            input: InputFlags {
                input_json: Some(r#"{"chain":"1","token":"USDC"}"#.to_string()),
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
            "structured input field \"token\" is not supported by approvals plan"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_input_json_null_field_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        let args = PlanArgs {
            input: InputFlags {
                input_json: Some(r#"{"chain":null}"#.to_string()),
                input_file: None,
            },
            ..PlanArgs::default()
        };
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("null structured-input field must be a usage error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(
            err.message,
            "structured input field \"chain\" cannot be null"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 3, 4. step calldata reuses the defi-evm ABI golden ----------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_step_calldata_matches_defi_evm_approve_golden() {
        let tmp = TempDir::new().expect("tempdir");
        let env = run_plan(tmp.path(), base_plan_args())
            .await
            .expect("approvals plan should succeed");
        let data = action_data(&env);
        let calldata = data["steps"][0]["data"].as_str().expect("step data string");
        assert_eq!(
            calldata, APPROVE_CALLDATA_GOLDEN,
            "approval step calldata must equal the pinned defi-evm ERC-20 approve golden"
        );
    }

    // --- 6. plan persists the action to the Store --------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_persists_action_to_store() {
        let tmp = TempDir::new().expect("tempdir");
        let settings = exec_settings(tmp.path());
        let ctx = AppCtx::new(settings.clone());
        let env = handle(&ctx, ApprovalsCmd::Plan(base_plan_args()))
            .await
            .expect("approvals plan should succeed");
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
        assert_eq!(persisted.intent_type, "approve");
        assert_eq!(persisted.input_amount, "1000000");
        assert_eq!(persisted.provider, "native");
    }

    // --- 7. decimal amount parity ------------------------------------------

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
            APPROVE_CALLDATA_GOLDEN,
            "decimal 1 USDC normalizes to the same calldata as base 1000000"
        );
    }

    // --- 8. identity-constraint errors (offline) ---------------------------

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
        // Nothing persisted on the error path.
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

    // --- 9. amount cross-validation through the handler --------------------

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

    // --- 10. planner validation surfaces through the handler ---------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_malformed_spender() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = base_plan_args();
        args.spender = Some("0xdeadbeef".to_string()); // too short -> invalid hex addr
        let err = run_plan(tmp.path(), args)
            .await
            .expect_err("malformed --spender must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
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

    /// True iff no `approve`-intent action is persisted under `dir` (error paths
    /// must persist nothing). Opens the store leniently; a never-created store
    /// (no actions persisted yet) counts as empty.
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
    //! # Success criteria — `approvals submit` app-level handler (WS4, exec-submit)
    //!
    //! Go oracle: `internal/app/approvals_command.go` `submitCmd.RunE` +
    //! `internal/app/execution_helpers.go`
    //! (`resolveActionExecutionBackend` / `validateExecutionSender` /
    //! `executeActionWithTimeout`) + `internal/app/runner.go`
    //! (`resolveActionID` / `newExecutionSigner` / `parseExecuteOptions`). These
    //! tests drive [`cli::handle`] (the real binary dispatch entry point) for
    //! `approvals submit` ONLY, asserting the full machine contract the Go runner
    //! emits via `emitSuccess(...)` / `renderError(...)`.
    //!
    //! ## Determinism / offline strategy (no live chains)
    //!
    //! The reused [`defi_execution`] engine ([`defi_execution::evm_executor::execute_action`])
    //! is the contract source of truth, and the tests reuse it exactly as its own
    //! suite does:
    //!
    //! * **Pre-broadcast guards** (action-id, store load, intent gate,
    //!   already-completed short-circuit, backend selection, sender match,
    //!   execute-option validation) all fire BEFORE any network and are fully
    //!   deterministic.
    //! * **Local-signer broadcast/completion** is exercised through the
    //!   `--private-key` override (a deterministic in-args secp256k1 key whose
    //!   address is pinned in `defi-evm`) and `--allow-max-approval` against a
    //!   `wiremock` JSON-RPC server. The success path validates simulation,
    //!   gas/fee/nonce reads, `eth_sendRawTransaction`, receipt polling, terminal
    //!   persistence, and the recorded `tx_hash`.
    //! * **Bounded-approval pre-sign guardrail** (the documented submit-time check,
    //!   AGENTS.md "Execution pre-sign checks enforce bounded ERC-20 approvals")
    //!   IS asserted offline: an inflated approval without `--allow-max-approval`
    //!   is rejected; `--allow-max-approval` opts in.
    //! * **OWS `--wallet` backend** resolves through the OWS vault/CLI (WS4b e2e),
    //!   so only its OFFLINE guard rejections are asserted (missing persisted
    //!   `wallet_id`; legacy signer flags on a wallet-backed action). The OWS
    //!   happy-path broadcast (the `OwsSubmitBackend` send-hook seam) is a WS4b
    //!   deferral.
    //! * **Bridge destination-settlement waits** do NOT apply to `approvals`
    //!   (approval actions never carry a `bridge_send` step); that transition is
    //!   owned by the `bridge submit/status` unit + the `defi-execution`
    //!   `verify_bridge_settlement` suite, and is intentionally NOT re-asserted
    //!   here.
    //!
    //! Each criterion below is a FAILING test until `cli::handle` implements
    //! `approvals submit` (today it returns the `AppCtx::unimplemented` stub).
    //!
    //! Criteria:
    //!
    //! 1. **Submit success envelope (legacy local key) + completion.** Given a
    //!    persisted `approve` action whose `from_address` matches the deterministic
    //!    `--private-key` signer, a submit with `--allow-max-approval` returns
    //!    `Ok(Envelope)` (exit 0) with: `version == "v1"`, `success == true`,
    //!    `error == None`, `meta.partial == false`, `meta.command ==
    //!    "approvals submit"`, and `meta.cache == {status:"bypass", age_ms:0,
    //!    stale:false}` (execution paths bypass the cache, spec §2.5). The
    //!    serialized `data` Action has `status == "completed"` and its single step
    //!    has `status == "confirmed"`. (Go `emitSuccess(..., action, nil,
    //!    cacheMetaBypass(), nil, false)` after `executeActionWithTimeout`.)
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
    //! 5. **Intent gate.** Submitting a persisted NON-`approve` action (e.g. a
    //!    `transfer` intent) through `approvals submit` → [`Code::Usage`] (exit 2)
    //!    with `action is not an approval intent`. (Go `submitCmd` IntentType
    //!    guard; mirrors `super::ensure_approve_intent`.)
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
    //!    (`--private-key` / `--signer` / `--key-source`) → [`Code::Usage`]
    //!    (exit 2) (`wallet-backed actions do not accept legacy signer flags`).
    //!    (Go `usesLegacySignerFlags` guard — asserted via the `--private-key`
    //!    flag, which is unambiguously "explicitly set".)
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
    //!     a [`Code::Signer`] (exit 24) error surfaces from the
    //!     persisted-sender validation. (Go `validateExecutionSender` /
    //!     `validate_persisted_action_sender`: backend sender ≠ planned sender.)
    //!
    //! 12. **Bounded-approval guardrail (pre-sign).** A persisted approval whose
    //!     step calldata approves MORE than the planned `input_amount`, submitted
    //!     WITHOUT `--allow-max-approval`, → [`Code::ActionPlan`] (exit 20) with an
    //!     error mentioning `allow-max-approval`. The same action with
    //!     `--allow-max-approval` is accepted (exit 0, completed). (AGENTS.md
    //!     bounded-approval pre-sign check; `defi_execution::policy`
    //!     `validate_approval_policy`.)
    //!
    //! 13. **Execute-option validation.** `--gas-multiplier 1.0` → [`Code::Usage`]
    //!     (exit 2) (`--gas-multiplier must be > 1`); `--poll-interval "0s"` →
    //!     [`Code::Usage`] (exit 2); `--step-timeout "nope"` → [`Code::Usage`]
    //!     (exit 2). (Go `parseExecuteOptions`.)
    //!
    //! 14. **Signer init failure (no key).** A `legacy_local` action submitted with
    //!     `--signer local` and NO resolvable key (`--key-source env` with the env
    //!     unset, no `--private-key`) → [`Code::Signer`] (exit 24). (Go
    //!     `newExecutionSigner` → `initialize local signer`.)
    //!
    //! 15. **Error paths do not mutate terminal status.** On every rejected submit
    //!     (criteria 3–14, error cases) the persisted action — when one exists —
    //!     remains in its pre-submit `status == "planned"` (the handler returns the
    //!     typed `Err(Error)`; the runner renders the full error envelope to
    //!     stderr, spec §2.1).
    //!
    //! SKIPPED (covered elsewhere / wrong unit / deferred):
    //!   * the OWS happy-path resolve + send-hook broadcast — WS4b e2e deferral;
    //!   * Tempo (type 0x76) submit — Tempo is a separate execution path
    //!     (`--signer tempo` / `execution_backend == "tempo"`), byte-parity is
    //!     WS4a, and `approvals` planning is OWS-first standard-EVM (no Tempo
    //!     identity branch);
    //!   * bridge destination-settlement waits — `bridge submit/status` unit +
    //!     `defi-execution::verify_bridge_settlement`;
    //!   * the EIP-1559 signing byte layout — `defi-evm` signer goldens;
    //!   * the bounded-approval ABI decode internals — `defi-execution::policy`
    //!     RED suite;
    //!   * `--input-json`/`--input-file` precedence on submit — structured-input
    //!     unit (the plan-side merge is already covered in `app_tests`);
    //!   * cobra/clap flag defaults + schema auth metadata — schema/CLI suites.

    use super::cli::{handle, ApprovalsCmd, PlanArgs};
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags, SubmitArgs};
    use defi_config::Settings;
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::action::{Action, ActionStatus, ExecutionBackend};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::{json, Value};
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;
    use wiremock::matchers::{body_partial_json, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
    /// Spender for planned approvals.
    const SPENDER: &str = "0x00000000000000000000000000000000000000BB";
    const EXPECTED_TX_HASH: &str =
        "0x1111111111111111111111111111111111111111111111111111111111111111";

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

    /// A `SubmitArgs` carrying the clap flag DEFAULTS (the `#[derive(Default)]`
    /// `String`/`f64`/`bool` zero values would NOT match the parsed defaults, so
    /// they are stamped here): `signer=local`, `key_source=auto`,
    /// `gas_multiplier=1.2`, `poll_interval=2s`, `step_timeout=2m`,
    /// `simulate=true`. Callers mutate the returned value per test.
    fn base_submit_args(action_id: &str) -> SubmitArgs {
        SubmitArgs {
            action_id: Some(action_id.to_string()),
            from_address: None,
            allow_max_approval: false,
            unsafe_provider_tx: false,
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

    /// Plan + persist a canonical `approve` action against `dir`, returning its
    /// `action_id`. `from_addr` becomes the action's `from_address`; `amount` is
    /// the approved base-unit amount (which is also the planned `input_amount`).
    /// Plans through the real `cli::handle` plan path so the persisted shape is
    /// identical to production.
    async fn plan_approval_with_rpc(
        dir: &Path,
        from_addr: &str,
        amount: &str,
        rpc_url: &str,
    ) -> String {
        let ctx = AppCtx::new(exec_settings(dir));
        let args = PlanArgs {
            chain: Some("1".to_string()),
            asset: Some("USDC".to_string()),
            spender: Some(SPENDER.to_string()),
            amount: Some(amount.to_string()),
            amount_decimal: None,
            rpc_url: Some(rpc_url.to_string()),
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(from_addr.to_string()),
            },
            input: InputFlags::default(),
        };
        let env = handle(&ctx, ApprovalsCmd::Plan(args))
            .await
            .expect("plan an approve action for the submit fixture");
        env.data.expect("plan data")["action_id"]
            .as_str()
            .expect("action_id")
            .to_string()
    }

    async fn plan_approval(dir: &Path, from_addr: &str, amount: &str) -> String {
        plan_approval_with_rpc(dir, from_addr, amount, DEAD_RPC).await
    }

    /// A non-dialed RPC sentinel for the step (the policed EVM step path does not
    /// reach the network in this build; this keeps the action well-formed).
    const DEAD_RPC: &str = "http://127.0.0.1:0";

    /// Persist `action` directly (used for fixtures the plan path cannot build,
    /// e.g. a `transfer`-intent or an OWS-backed action).
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

    async fn run_submit(dir: &Path, args: SubmitArgs) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(&ctx, ApprovalsCmd::Submit(args)).await
    }

    async fn mock_rpc_method(server: &MockServer, rpc_method: &'static str, result: Value) {
        Mock::given(method("POST"))
            .and(body_partial_json(json!({ "method": rpc_method })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": result,
            })))
            .mount(server)
            .await;
    }

    async fn standard_submit_rpc() -> MockServer {
        let server = MockServer::start().await;
        mock_rpc_method(&server, "eth_chainId", json!("0x1")).await;
        mock_rpc_method(&server, "eth_call", json!("0x")).await;
        mock_rpc_method(&server, "eth_estimateGas", json!("0x5208")).await;
        mock_rpc_method(
            &server,
            "eth_getBlockByNumber",
            json!({
                "number": "0x10",
                "baseFeePerGas": "0x3b9aca00"
            }),
        )
        .await;
        mock_rpc_method(&server, "eth_maxPriorityFeePerGas", json!("0x3b9aca00")).await;
        mock_rpc_method(&server, "eth_getTransactionCount", json!("0x7")).await;
        mock_rpc_method(&server, "eth_sendRawTransaction", json!(EXPECTED_TX_HASH)).await;
        mock_rpc_method(
            &server,
            "eth_getTransactionReceipt",
            json!({
                "status": "0x1",
                "blockNumber": "0x11",
                "gasUsed": "0x5208"
            }),
        )
        .await;
        server
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
        let rpc = standard_submit_rpc().await;
        // Plan an approval whose sender matches the deterministic local signer.
        let action_id =
            plan_approval_with_rpc(tmp.path(), SIGNER_ADDR, "1000000", &rpc.uri()).await;

        let mut args = base_submit_args(&action_id);
        // Opt into the bounded-approval bypass so the offline pre-sign policy path
        // does not require action context for the bound check.
        args.allow_max_approval = true;
        let env = run_submit(tmp.path(), args)
            .await
            .expect("legacy-local approval submit should complete offline");

        // Envelope contract.
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "approvals submit");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        // Completed action in data, single confirmed step.
        let data = data_of(&env);
        assert_eq!(data["status"], Value::from("completed"));
        let steps = data["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0]["status"], Value::from("confirmed"));
        assert_eq!(steps[0]["tx_hash"], Value::from(EXPECTED_TX_HASH));

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
    async fn submit_rejects_non_approve_intent() {
        let tmp = TempDir::new().expect("tempdir");
        // A persisted TRANSFER-intent action submitted through approvals submit.
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "transfer",
            "eip155:1",
            Default::default(),
        );
        action.from_address = SIGNER_ADDR.to_string();
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        save_action(tmp.path(), &action);

        let args = base_submit_args(&action.action_id);
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("non-approve intent rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("action is not an approval intent"),
            "got: {err}"
        );
        // Status untouched.
        assert_eq!(persisted_status(tmp.path(), &action.action_id), "planned");
    }

    // --- 6. already-completed short-circuit --------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_already_completed_short_circuits_with_warning() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_approval(tmp.path(), SIGNER_ADDR, "1000000").await;
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
        assert_eq!(env.meta.command, "approvals submit");
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
        let action_id = plan_approval(tmp.path(), SIGNER_ADDR, "1000000").await;
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
            "approve",
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
            "approve",
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
        let action_id = plan_approval(tmp.path(), SIGNER_ADDR, "1000000").await;
        let mut args = base_submit_args(&action_id);
        args.allow_max_approval = true;
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
        let action_id = plan_approval(tmp.path(), OTHER_ADDR, "1000000").await;
        let mut args = base_submit_args(&action_id);
        args.allow_max_approval = true;
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("planned-sender/signer mismatch rejected");
        assert_eq!(err.code, Code::Signer);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 24);
        assert_eq!(persisted_status(tmp.path(), &action_id), "planned");
    }

    // --- 12. bounded-approval pre-sign guardrail ---------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_inflated_approval_without_allow_max() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_approval(tmp.path(), SIGNER_ADDR, "1000000").await;
        // Inflate the persisted step's approve amount ABOVE the planned
        // input_amount, simulating an over-approval that the bounded check must
        // reject without --allow-max-approval.
        {
            let store = ActionStore::open(
                tmp.path().join("actions.db"),
                tmp.path().join("actions.lock"),
            )
            .expect("open store");
            let mut action = store.get(&action_id).expect("load");
            // approve(spender, 0xffffffff...) — max uint256, > input_amount.
            action.steps[0].data = format!(
                "0x095ea7b3000000000000000000000000{}{}",
                SPENDER.trim_start_matches("0x").to_lowercase(),
                "f".repeat(64)
            );
            store.save(&action).expect("persist inflated approval");
        }

        let err = run_submit(tmp.path(), base_submit_args(&action_id))
            .await
            .expect_err("inflated approval rejected without --allow-max-approval");
        assert_eq!(err.code, Code::ActionPlan);
        // ActionPlan maps to exit 20 (spec §2.2).
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 20);
        assert!(
            err.to_string().contains("allow-max-approval"),
            "expected the bounded-approval override hint, got: {err}"
        );
        assert_eq!(persisted_status(tmp.path(), &action_id), "planned");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_inflated_approval_accepted_with_allow_max() {
        let tmp = TempDir::new().expect("tempdir");
        let rpc = standard_submit_rpc().await;
        let action_id =
            plan_approval_with_rpc(tmp.path(), SIGNER_ADDR, "1000000", &rpc.uri()).await;
        {
            let store = ActionStore::open(
                tmp.path().join("actions.db"),
                tmp.path().join("actions.lock"),
            )
            .expect("open store");
            let mut action = store.get(&action_id).expect("load");
            action.steps[0].data = format!(
                "0x095ea7b3000000000000000000000000{}{}",
                SPENDER.trim_start_matches("0x").to_lowercase(),
                "f".repeat(64)
            );
            store.save(&action).expect("persist inflated approval");
        }

        let mut args = base_submit_args(&action_id);
        args.allow_max_approval = true;
        let env = run_submit(tmp.path(), args)
            .await
            .expect("inflated approval accepted with --allow-max-approval");
        assert!(env.success);
        assert_eq!(data_of(&env)["status"], Value::from("completed"));
    }

    // --- 13. execute-option validation -------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_gas_multiplier_not_greater_than_one() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_approval(tmp.path(), SIGNER_ADDR, "1000000").await;
        let mut args = base_submit_args(&action_id);
        args.allow_max_approval = true;
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
        let action_id = plan_approval(tmp.path(), SIGNER_ADDR, "1000000").await;
        let mut args = base_submit_args(&action_id);
        args.allow_max_approval = true;
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
        let action_id = plan_approval(tmp.path(), SIGNER_ADDR, "1000000").await;
        let mut args = base_submit_args(&action_id);
        args.allow_max_approval = true;
        args.step_timeout = "nope".to_string();
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("unparseable step-timeout rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- 14. signer init failure (no key) ----------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_signer_init_failure_is_signer_error() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_approval(tmp.path(), SIGNER_ADDR, "1000000").await;
        let mut args = base_submit_args(&action_id);
        args.allow_max_approval = true;
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
    //! # Success criteria — `approvals status` app-level handler (WS4, exec-status)
    //!
    //! Go oracle: `internal/app/approvals_command.go` `statusCmd.RunE`. These
    //! tests drive [`cli::handle`] for `approvals status` ONLY. `approvals status`
    //! is a pure READ over the persisted action store (no signing, no network),
    //! so it is fully offline + deterministic. (Bridge destination-settlement
    //! polling — the only network-backed status transition — does NOT apply to
    //! `approvals`: approval actions never carry a `bridge_send` step. That wait
    //! is owned by `bridge status` + `defi-execution::verify_bridge_settlement`
    //! and is NOT re-asserted here.)
    //!
    //! Criteria (each FAILING until `cli::handle` implements `approvals status`):
    //!
    //! 1. **Status success envelope reflects the persisted action.** Given a
    //!    persisted `approve` action in `status == "planned"`, `approvals status
    //!    --action-id <id>` returns `Ok(Envelope)` (exit 0) with `version ==
    //!    "v1"`, `success == true`, `error == None`, `meta.command ==
    //!    "approvals status"`, `meta.cache == {status:"bypass", age_ms:0,
    //!    stale:false}` (execution paths bypass the cache, spec §2.5), and `data`
    //!    is the serialized Action with `action_id` == the requested id,
    //!    `intent_type == "approve"`, and `status == "planned"`. (Go
    //!    `emitSuccess(..., action, nil, cacheMetaBypass(), nil, false)`.)
    //!
    //! 2. **Status reflects a `completed` transition.** After the persisted action
    //!    is advanced to `completed`, `approvals status` returns `data.status ==
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
    //!    not-found as `clierr.Wrap(CodeUsage, "load action", err)`). Mirrors the
    //!    Go runner test `TestRunnerExecutionStatusBypassesCacheOpen`, which runs
    //!    `approvals status --action-id act_<32hex>` against an empty store and
    //!    asserts exit code 2.
    //!
    //! 6. **Intent gate.** `approvals status` on a persisted NON-`approve` action
    //!    (e.g. a `bridge` intent) → [`Code::Usage`] (exit 2) with `action is not
    //!    an approval intent`. (Go `statusCmd` IntentType guard; parity with the Go
    //!    runner test `TestRunnerSwapStatusRejectsNonSwapIntent` for the
    //!    cross-group case.)
    //!
    //! SKIPPED (covered elsewhere / wrong unit):
    //!   * bridge destination-settlement polling — `bridge status` unit;
    //!   * the action JSON shape internals — `defi-execution::action` golden;
    //!   * cache-bypass routing for `approvals status` — runner cache-flow concern
    //!     (`should_open_cache`), asserted here only via `meta.cache.status`.

    use super::cli::{handle, ApprovalsCmd, PlanArgs};
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
    const SPENDER: &str = "0x00000000000000000000000000000000000000BB";

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

    /// Plan + persist a canonical `approve` action, returning its `action_id`.
    async fn plan_approval(dir: &Path) -> String {
        let ctx = AppCtx::new(exec_settings(dir));
        let args = PlanArgs {
            chain: Some("1".to_string()),
            asset: Some("USDC".to_string()),
            spender: Some(SPENDER.to_string()),
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
        let env = handle(&ctx, ApprovalsCmd::Plan(args))
            .await
            .expect("plan an approve action for the status fixture");
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
            ApprovalsCmd::Status(StatusArgs {
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
        let action_id = plan_approval(tmp.path()).await;
        let env = run_status(tmp.path(), &action_id)
            .await
            .expect("status on a planned approval should succeed");

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "approvals status");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        let data = data_of(&env);
        assert_eq!(data["action_id"], Value::from(action_id.as_str()));
        assert_eq!(data["intent_type"], Value::from("approve"));
        assert_eq!(data["status"], Value::from("planned"));
    }

    // --- 2, 3. status reflects lifecycle transitions -----------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn status_reflects_completed_transition() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_approval(tmp.path()).await;
        set_status(tmp.path(), &action_id, ActionStatus::Completed);
        let env = run_status(tmp.path(), &action_id).await.expect("status ok");
        assert_eq!(data_of(&env)["status"], Value::from("completed"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn status_reflects_running_transition() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_approval(tmp.path()).await;
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

    // --- 5. load failure for an unknown action (matches the Go runner test) -

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
    async fn status_rejects_non_approve_intent() {
        let tmp = TempDir::new().expect("tempdir");
        // A persisted BRIDGE-intent action queried through approvals status.
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
            .expect_err("non-approve intent rejected by approvals status");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("action is not an approval intent"),
            "got: {err}"
        );
    }
}
