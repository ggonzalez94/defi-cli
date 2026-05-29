//! `rewards` command group handler (Go: `internal/app/rewards_command.go` —
//! `newRewardsCommand` / `newRewardsClaimCommand` / `newRewardsCompoundCommand`).
//!
//! This module owns the **rewards-command-specific** glue that sits between the
//! runner's cache-flow core ([`crate::runner`]), the shared execution-identity
//! resolver, and the action-build registry ([`defi_execution::builder::Registry`]).
//! The `rewards` group has two subcommands (`claim`, `compound`), each with
//! `plan` / `submit` / `status`. Both route only to `provider=aave`. Specifically
//! this module owns:
//!
//! * the `rewards claim plan` request builder (`build_rewards_claim_request`) —
//!   the Go `buildAction` closure inside `newRewardsClaimCommand`: parse
//!   `--chain`, normalize `--assets` (drop blanks), require at least one asset
//!   (usage), and DEFAULT an empty `--amount` to the sentinel `"max"`; assemble a
//!   [`defi_execution::builder::RewardsClaimRequest`] carrying provider / sender /
//!   recipient / reward-token / simulate / rpc-url / controller / pool-address
//!   provider verbatim;
//! * the `rewards compound plan` request builder
//!   (`build_rewards_compound_request`) — the Go `buildAction` closure inside
//!   `newRewardsCompoundCommand`: same chain parse + asset normalization +
//!   at-least-one-asset gate, but `--amount` is REQUIRED (an empty amount is a
//!   usage error — compound has no `"max"` default); assemble a
//!   [`defi_execution::builder::RewardsCompoundRequest`] carrying the additional
//!   `on_behalf_of` / `pool_address` fields verbatim;
//! * the `rewards {claim,compound} plan` schema identity input constraints
//!   (`rewards_plan_identity_constraints`: the standard
//!   `exactly_one_of {wallet, from_address}`, no per-provider `when` branching —
//!   rewards planning is OWS-first / standard EVM, like transfer/bridge);
//! * the persisted-intent gates (`ensure_rewards_claim_intent` /
//!   `ensure_rewards_compound_intent`: `rewards claim {submit,status}` reject a
//!   non-`claim_rewards` action, and `rewards compound {submit,status}` reject a
//!   non-`compound_rewards` action, both with a usage error).
//!
//! NOT re-owned here (consumed from elsewhere):
//! * the rewards **action construction** (claim → `claimRewards` calldata, the
//!   3-step compound `[claim, approval, lend_call]`, address/amount validation) —
//!   owned by `defi_execution::planner::{build_aave_rewards_claim_action,
//!   build_aave_rewards_compound_action}` and covered by its own RED suite;
//! * the action-build registry routing (`Registry::build_rewards_claim_action` /
//!   `build_rewards_compound_action`, with the `provider != aave` unsupported
//!   gate) — owned by `defi_execution::builder` (B6);
//! * the provider canonicalization (`normalize_lending_provider`) — owned by
//!   [`crate::runner`] / `defi_providers::normalize`;
//! * the shared execution-identity resolver (`resolve_execution_identity`) and
//!   its OWS/legacy backend stamping — shared execution-identity module / runner;
//! * the submit signer/backend plumbing, pre-sign guardrails, receipt polling,
//!   already-completed short-circuit — `defi-execution` / runner concern;
//! * the cache-key construction + cache bypass for execution paths — runner
//!   concern, owned by [`crate::runner`].

#![allow(dead_code, unused_variables)]

use defi_errors::{Code, Error};
use defi_execution::builder::{RewardsClaimRequest, RewardsCompoundRequest};
use defi_id::parse_chain;
use defi_schema::InputConstraint;

/// Normalize a string slice the way Go `normalizeStringSlice` does: trim each
/// entry and drop the ones that are empty after trimming, preserving the order
/// of survivors.
fn normalize_string_slice(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .collect()
}

/// Build a [`RewardsClaimRequest`] from the raw `rewards claim plan` flags.
///
/// Parity with the Go `buildAction` closure in `newRewardsClaimCommand`:
/// 1. parse `--chain` (delegates to `defi_id::parse_chain`); an empty / invalid
///    `--chain` surfaces the typed error from that helper;
/// 2. normalize `--assets` via the Go `normalizeStringSlice` rule: trim each
///    entry and drop blanks;
/// 3. if the normalized asset list is empty → [`defi_errors::Code::Usage`]
///    (`--assets is required`);
/// 4. DEFAULT an empty (trimmed) `--amount` to the sentinel `"max"` — claim has a
///    "claim everything" default (distinct from compound, which requires it);
/// 5. assemble the [`RewardsClaimRequest`] carrying provider, the resolved sender
///    (`from_address`), recipient, the normalized assets, reward-token, the
///    resolved amount, simulate, rpc-url, controller-address, and
///    pool-address-provider verbatim.
///
/// The reward-token / sender / recipient hex validation, the per-asset address
/// validation, and the amount parsing are NOT performed here — they belong to
/// `defi_execution::planner::build_aave_rewards_claim_action`, which consumes the
/// routed request.
#[allow(clippy::too_many_arguments)]
pub fn build_rewards_claim_request(
    provider: &str,
    chain_arg: &str,
    from_address: &str,
    recipient: &str,
    assets: &[String],
    reward_token: &str,
    amount_base: &str,
    simulate: bool,
    rpc_url: &str,
    controller_address: &str,
    pool_address_provider: &str,
) -> Result<RewardsClaimRequest, Error> {
    let chain = parse_chain(chain_arg)?;
    let assets = normalize_string_slice(assets);
    if assets.is_empty() {
        return Err(Error::new(Code::Usage, "--assets is required"));
    }
    // Claim "claim everything": an empty (trimmed) amount defaults to "max".
    let mut amount = amount_base.trim().to_string();
    if amount.is_empty() {
        amount = "max".to_string();
    }
    Ok(RewardsClaimRequest {
        provider: provider.to_string(),
        chain,
        sender: from_address.to_string(),
        recipient: recipient.to_string(),
        assets,
        reward_token: reward_token.to_string(),
        amount_base_units: amount,
        simulate,
        rpc_url: rpc_url.to_string(),
        controller_address: controller_address.to_string(),
        pool_address_provider: pool_address_provider.to_string(),
    })
}

/// Build a [`RewardsCompoundRequest`] from the raw `rewards compound plan` flags.
///
/// Parity with the Go `buildAction` closure in `newRewardsCompoundCommand`:
/// 1. parse `--chain` (delegates to `defi_id::parse_chain`);
/// 2. normalize `--assets` (trim + drop blanks, Go `normalizeStringSlice`);
/// 3. if the normalized asset list is empty → [`defi_errors::Code::Usage`]
///    (`--assets is required`);
/// 4. `--amount` is REQUIRED: an empty (trimmed) amount → [`Code::Usage`]
///    (`--amount is required`) — compound has NO `"max"` default, unlike claim;
/// 5. assemble the [`RewardsCompoundRequest`] carrying provider, sender, recipient,
///    `on_behalf_of`, the normalized assets, reward-token, the amount, simulate,
///    rpc-url, controller-address, `pool_address`, and pool-address-provider
///    verbatim.
#[allow(clippy::too_many_arguments)]
pub fn build_rewards_compound_request(
    provider: &str,
    chain_arg: &str,
    from_address: &str,
    recipient: &str,
    on_behalf_of: &str,
    assets: &[String],
    reward_token: &str,
    amount_base: &str,
    simulate: bool,
    rpc_url: &str,
    controller_address: &str,
    pool_address: &str,
    pool_address_provider: &str,
) -> Result<RewardsCompoundRequest, Error> {
    let chain = parse_chain(chain_arg)?;
    let assets = normalize_string_slice(assets);
    if assets.is_empty() {
        return Err(Error::new(Code::Usage, "--assets is required"));
    }
    // Compound has NO "max" default: an empty (trimmed) amount is a usage error.
    let amount = amount_base.trim().to_string();
    if amount.is_empty() {
        return Err(Error::new(Code::Usage, "--amount is required"));
    }
    Ok(RewardsCompoundRequest {
        provider: provider.to_string(),
        chain,
        sender: from_address.to_string(),
        recipient: recipient.to_string(),
        on_behalf_of: on_behalf_of.to_string(),
        assets,
        reward_token: reward_token.to_string(),
        amount_base_units: amount,
        simulate,
        rpc_url: rpc_url.to_string(),
        controller_address: controller_address.to_string(),
        pool_address: pool_address.to_string(),
        pool_address_provider: pool_address_provider.to_string(),
    })
}

/// The `rewards {claim,compound} plan` schema identity input constraints.
///
/// Parity with Go `standardExecutionIdentityInputConstraints` (advertised by
/// both rewards plan commands via `configureStructuredInput`): a single
/// `exactly_one_of` entry over `[wallet, from_address]` with no `when` clause —
/// rewards planning is OWS-first / standard EVM, with no per-provider identity
/// branching (unlike swap's Tempo/TaikoSwap split).
pub fn rewards_plan_identity_constraints() -> Vec<InputConstraint> {
    vec![InputConstraint {
        kind: "exactly_one_of".to_string(),
        fields: vec!["wallet".to_string(), "from_address".to_string()],
        when: Default::default(),
        description: "Provide exactly one execution identity input: `wallet` \
                      (OWS, recommended) or `from_address` (local signer)."
            .to_string(),
    }]
}

/// Validate that a persisted action is a `claim_rewards` intent.
///
/// Parity with the `claim submit` / `claim status` guard
/// `action.IntentType != "claim_rewards"` in `rewards_command.go`: a mismatched
/// intent yields a [`defi_errors::Code::Usage`] error whose message is
/// `action is not a rewards claim intent`.
pub fn ensure_rewards_claim_intent(intent_type: &str) -> Result<(), Error> {
    if intent_type != "claim_rewards" {
        return Err(Error::new(
            Code::Usage,
            "action is not a rewards claim intent",
        ));
    }
    Ok(())
}

/// Validate that a persisted action is a `compound_rewards` intent.
///
/// Parity with the `compound submit` / `compound status` guard
/// `action.IntentType != "compound_rewards"` in `rewards_command.go`: a
/// mismatched intent yields a [`defi_errors::Code::Usage`] error whose message is
/// `action is not a rewards compound intent`.
pub fn ensure_rewards_compound_intent(intent_type: &str) -> Result<(), Error> {
    if intent_type != "compound_rewards" {
        return Err(Error::new(
            Code::Usage,
            "action is not a rewards compound intent",
        ));
    }
    Ok(())
}

/// clap parsing + handler for the `rewards` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::{Code, Error};
    use defi_execution::builder::Registry;
    use defi_model::{Envelope, ProviderStatus};
    use defi_providers::normalize::normalize_lending_provider;

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, SubmitArgs};
    use crate::execident::{apply_execution_identity_to_action, resolve_execution_identity};
    use crate::execsubmit::{
        execute_resolved, parse_execute_options, presign_validate_action,
        resolve_action_execution_backend, validate_execution_sender, ExecuteOptionInputs,
        SubmitExecutionInputs,
    };

    /// `rewards` subcommands: the two execution verbs.
    #[derive(Subcommand, Debug)]
    pub enum RewardsCmd {
        /// Claim rewards.
        #[command(subcommand)]
        Claim(ClaimVerbCmd),
        /// Compound rewards by claim + resupply.
        #[command(subcommand)]
        Compound(CompoundVerbCmd),
    }

    impl RewardsCmd {
        /// The full path tail (e.g. `claim plan`).
        pub fn path(&self) -> String {
            match self {
                RewardsCmd::Claim(v) => format!("claim {}", v.path()),
                RewardsCmd::Compound(v) => format!("compound {}", v.path()),
            }
        }
    }

    /// `rewards claim` sub-subcommands.
    #[derive(Subcommand, Debug)]
    pub enum ClaimVerbCmd {
        /// Create and persist a rewards-claim action plan.
        Plan(ClaimPlanArgs),
        /// Execute an existing rewards-claim action.
        Submit(SubmitArgs),
        /// Get rewards-claim action status.
        Status(StatusArgs),
    }

    impl ClaimVerbCmd {
        /// The leaf path token.
        pub fn path(&self) -> &'static str {
            match self {
                ClaimVerbCmd::Plan(_) => "plan",
                ClaimVerbCmd::Submit(_) => "submit",
                ClaimVerbCmd::Status(_) => "status",
            }
        }
    }

    /// `rewards compound` sub-subcommands.
    #[derive(Subcommand, Debug)]
    pub enum CompoundVerbCmd {
        /// Create and persist a rewards-compound action plan.
        Plan(CompoundPlanArgs),
        /// Execute an existing rewards-compound action.
        Submit(SubmitArgs),
        /// Get rewards-compound action status.
        Status(StatusArgs),
    }

    impl CompoundVerbCmd {
        /// The leaf path token.
        pub fn path(&self) -> &'static str {
            match self {
                CompoundVerbCmd::Plan(_) => "plan",
                CompoundVerbCmd::Submit(_) => "submit",
                CompoundVerbCmd::Status(_) => "status",
            }
        }
    }

    /// `rewards claim plan` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct ClaimPlanArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Comma-separated rewards source asset addresses.
        #[arg(long, value_delimiter = ',')]
        pub assets: Vec<String>,
        /// Reward token address.
        #[arg(long = "reward-token")]
        pub reward_token: Option<String>,
        /// Claim amount in base units (defaults to max).
        #[arg(long)]
        pub amount: Option<String>,
        /// Recipient address (defaults to the resolved sender address).
        #[arg(long)]
        pub recipient: Option<String>,
        /// Aave incentives controller address override.
        #[arg(long = "controller-address")]
        pub controller_address: Option<String>,
        /// Aave pool address provider override.
        #[arg(long = "pool-address-provider")]
        pub pool_address_provider: Option<String>,
        /// Rewards provider (aave).
        #[arg(long)]
        pub provider: Option<String>,
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

    /// `rewards compound plan` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct CompoundPlanArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Comma-separated rewards source asset addresses.
        #[arg(long, value_delimiter = ',')]
        pub assets: Vec<String>,
        /// Reward token address.
        #[arg(long = "reward-token")]
        pub reward_token: Option<String>,
        /// Compound amount in base units.
        #[arg(long)]
        pub amount: Option<String>,
        /// Recipient address (defaults to the resolved sender address).
        #[arg(long)]
        pub recipient: Option<String>,
        /// Aave onBehalfOf address for compounding supply.
        #[arg(long = "on-behalf-of")]
        pub on_behalf_of: Option<String>,
        /// Aave incentives controller address override.
        #[arg(long = "controller-address")]
        pub controller_address: Option<String>,
        /// Aave pool address override.
        #[arg(long = "pool-address")]
        pub pool_address: Option<String>,
        /// Aave pool address provider override.
        #[arg(long = "pool-address-provider")]
        pub pool_address_provider: Option<String>,
        /// Rewards provider (aave).
        #[arg(long)]
        pub provider: Option<String>,
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

    /// Handle `rewards <sub>`.
    pub async fn handle(ctx: &AppCtx, cmd: RewardsCmd) -> Result<Envelope, Error> {
        match cmd {
            RewardsCmd::Claim(ClaimVerbCmd::Plan(args)) => handle_claim_plan(ctx, args).await,
            RewardsCmd::Claim(ClaimVerbCmd::Submit(args)) => handle_claim_submit(ctx, args).await,
            RewardsCmd::Claim(ClaimVerbCmd::Status(args)) => handle_claim_status(ctx, args).await,
            RewardsCmd::Compound(CompoundVerbCmd::Plan(args)) => {
                handle_compound_plan(ctx, args).await
            }
            RewardsCmd::Compound(CompoundVerbCmd::Submit(args)) => {
                handle_compound_submit(ctx, args).await
            }
            RewardsCmd::Compound(CompoundVerbCmd::Status(args)) => {
                handle_compound_status(ctx, args).await
            }
        }
    }

    /// Compute the rewards-plan provider-status name the way the Go runner does
    /// (`normalizeLendingProvider(provider)` → trimmed `--provider` → `"unknown"`).
    fn provider_status_name(provider: &str) -> String {
        let normalized = normalize_lending_provider(provider);
        if !normalized.is_empty() {
            return normalized;
        }
        let trimmed = provider.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        "unknown".to_string()
    }

    /// Handle `rewards claim plan` (Go `planCmd.RunE` in `newRewardsClaimCommand`).
    ///
    /// Flow parity with the Go runner:
    /// 1. resolve the execution identity (OWS `--wallet` first / legacy
    ///    `--from-address`) on the requested chain; an identity error returns the
    ///    typed [`Error`] before anything is persisted;
    /// 2. build the [`RewardsClaimRequest`] from the flags + the resolved sender
    ///    ([`super::build_rewards_claim_request`]: chain parse, `--assets`
    ///    normalization with the at-least-one gate, and the empty-amount → `"max"`
    ///    default);
    /// 3. compose the claim action via the action-build registry
    ///    ([`Registry::build_rewards_claim_action`] → the Aave rewards planner,
    ///    which gates `--provider`, auto-resolves the incentives controller, and
    ///    encodes the `claimRewards` calldata); a build error returns the typed
    ///    [`Error`] (nothing persisted);
    /// 4. stamp the resolved identity onto the action and persist it to the action
    ///    [`Store`];
    /// 5. emit the success envelope with the identity warnings, the cache bypassed
    ///    (execution paths skip the cache, spec §2.5), and the provider status
    ///    keyed on the normalized lending provider.
    ///
    /// [`Store`]: defi_execution::store::Store
    async fn handle_claim_plan(ctx: &AppCtx, args: ClaimPlanArgs) -> Result<Envelope, Error> {
        // 0. Merge structured input (`--input-json` / `--input-file`) onto the
        //    parsed flags before any guard (Go PreRunE `applyStructuredFlagInput`
        //    over `claimArgs`). Explicit flags win; unknown key / null → usage.
        let mut args = args;
        merge_claim_plan_input(&mut args)?;

        let chain_arg = args.chain.as_deref().unwrap_or_default();
        let wallet_ref = args.identity.wallet.as_deref().unwrap_or_default();
        let from_flag = args.identity.from_address.as_deref().unwrap_or_default();
        let provider = args.provider.as_deref().unwrap_or_default();

        // 1. Resolve the execution identity (returns before any persistence on
        //    error — both / neither input, malformed address, Tempo/non-EVM
        //    --wallet, OWS resolve failures).
        let identity = resolve_execution_identity(wallet_ref, from_flag, chain_arg)?;

        // 2. Build the claim request against the resolved sender (assets
        //    normalization + at-least-one gate + empty-amount → "max").
        let request = super::build_rewards_claim_request(
            provider,
            chain_arg,
            &identity.from_address,
            args.recipient.as_deref().unwrap_or_default(),
            &args.assets,
            args.reward_token.as_deref().unwrap_or_default(),
            args.amount.as_deref().unwrap_or_default(),
            args.simulate,
            args.rpc_url.as_deref().unwrap_or_default(),
            args.controller_address.as_deref().unwrap_or_default(),
            args.pool_address_provider.as_deref().unwrap_or_default(),
        )?;

        // 3. Compose the action via the registry (provider gating + on-chain
        //    controller resolution + calldata encoding live in the planner). A
        //    build error is returned (the runner renders the full error envelope).
        let mut action = Registry::new().build_rewards_claim_action(request).await?;

        // 4. Stamp the identity + persist.
        apply_execution_identity_to_action(&mut action, &identity);
        let store = ctx.open_action_store()?;
        store
            .save(&action)
            .map_err(|e| Error::wrap(Code::Internal, "persist planned action", e))?;

        // 5. Emit the success envelope (cache bypassed for execution paths). The
        //    provider status is `ok` because the build succeeded
        //    (Go `statusFromErr(nil)`).
        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize planned action", e))?;
        let providers = vec![ProviderStatus {
            name: provider_status_name(provider),
            status: "ok".to_string(),
            latency_ms: 0,
        }];
        let mut env = ctx.metadata_envelope("rewards claim plan", data, providers);
        env.warnings = identity.warnings;
        Ok(env)
    }

    /// Handle `rewards compound plan` (Go `planCmd.RunE` in
    /// `newRewardsCompoundCommand`).
    ///
    /// Same flow as [`handle_claim_plan`] with the compound divergences carried by
    /// [`super::build_rewards_compound_request`] (the `--amount` is REQUIRED, no
    /// `"max"` default) and the Aave rewards-compound planner
    /// ([`Registry::build_rewards_compound_action`]): the `"max"` sentinel +
    /// recipient-mismatch rejections, the pool resolution + allowance-gated
    /// `[claim, approval, supply]` step assembly, and the `on_behalf_of` default.
    async fn handle_compound_plan(ctx: &AppCtx, args: CompoundPlanArgs) -> Result<Envelope, Error> {
        // 0. Merge structured input (`--input-json` / `--input-file`) onto the
        //    parsed flags before any guard (Go PreRunE `applyStructuredFlagInput`
        //    over `compoundArgs`). Explicit flags win; unknown key / null → usage.
        let mut args = args;
        merge_compound_plan_input(&mut args)?;

        let chain_arg = args.chain.as_deref().unwrap_or_default();
        let wallet_ref = args.identity.wallet.as_deref().unwrap_or_default();
        let from_flag = args.identity.from_address.as_deref().unwrap_or_default();
        let provider = args.provider.as_deref().unwrap_or_default();

        // 1. Resolve the execution identity (returns before any persistence on error).
        let identity = resolve_execution_identity(wallet_ref, from_flag, chain_arg)?;

        // 2. Build the compound request against the resolved sender (assets
        //    normalization + at-least-one gate + REQUIRED non-empty amount).
        let request = super::build_rewards_compound_request(
            provider,
            chain_arg,
            &identity.from_address,
            args.recipient.as_deref().unwrap_or_default(),
            args.on_behalf_of.as_deref().unwrap_or_default(),
            &args.assets,
            args.reward_token.as_deref().unwrap_or_default(),
            args.amount.as_deref().unwrap_or_default(),
            args.simulate,
            args.rpc_url.as_deref().unwrap_or_default(),
            args.controller_address.as_deref().unwrap_or_default(),
            args.pool_address.as_deref().unwrap_or_default(),
            args.pool_address_provider.as_deref().unwrap_or_default(),
        )?;

        // 3. Compose the 3-step compound action via the registry (provider gating
        //    + max-sentinel/recipient-mismatch rejections + on-chain pool/allowance
        //    resolution + step assembly live in the planner).
        let mut action = Registry::new()
            .build_rewards_compound_action(request)
            .await?;

        // 4. Stamp the identity + persist.
        apply_execution_identity_to_action(&mut action, &identity);
        let store = ctx.open_action_store()?;
        store
            .save(&action)
            .map_err(|e| Error::wrap(Code::Internal, "persist planned action", e))?;

        // 5. Emit the success envelope (cache bypassed for execution paths).
        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize planned action", e))?;
        let providers = vec![ProviderStatus {
            name: provider_status_name(provider),
            status: "ok".to_string(),
            latency_ms: 0,
        }];
        let mut env = ctx.metadata_envelope("rewards compound plan", data, providers);
        env.warnings = identity.warnings;
        Ok(env)
    }

    /// Handle `rewards claim submit` (Go `submitCmd.RunE` in
    /// `newRewardsClaimCommand`).
    ///
    /// Structurally identical to `approvals submit` (the same shared `execsubmit`
    /// plumbing: action-id resolve → store load → intent gate → already-completed
    /// short-circuit → backend/signer resolve → sender match → execute-option
    /// parse → bounded-approval pre-sign guardrail → broadcast), with the
    /// `claim_rewards`-only intent gate ([`super::ensure_rewards_claim_intent`]).
    /// A `claim` step is never an `approval`, so the bounded-approval guardrail is
    /// a no-op here, but the call mirrors the shared path and the engine's per-step
    /// policy contract.
    async fn handle_claim_submit(ctx: &AppCtx, args: SubmitArgs) -> Result<Envelope, Error> {
        submit_rewards_action(ctx, args, "rewards claim submit", RewardsKind::Claim).await
    }

    /// Handle `rewards compound submit` (Go `submitCmd.RunE` in
    /// `newRewardsCompoundCommand`).
    ///
    /// Same shared `execsubmit` plumbing as [`handle_claim_submit`] with the
    /// `compound_rewards`-only intent gate
    /// ([`super::ensure_rewards_compound_intent`]). Compound is the only multi-step
    /// rewards action (`[claim, approval, lend_call]`), so the `approval` step IS
    /// subject to the bounded-approval pre-sign guardrail
    /// ([`crate::execsubmit::presign_validate_action`]): an inflated approval
    /// without `--allow-max-approval` surfaces the documented override hint.
    async fn handle_compound_submit(ctx: &AppCtx, args: SubmitArgs) -> Result<Envelope, Error> {
        submit_rewards_action(ctx, args, "rewards compound submit", RewardsKind::Compound).await
    }

    /// Handle `rewards claim status` (Go `statusCmd.RunE` in
    /// `newRewardsClaimCommand`): a pure read over the persisted action store.
    async fn handle_claim_status(ctx: &AppCtx, args: StatusArgs) -> Result<Envelope, Error> {
        status_rewards_action(ctx, args, "rewards claim status", RewardsKind::Claim).await
    }

    /// Handle `rewards compound status` (Go `statusCmd.RunE` in
    /// `newRewardsCompoundCommand`): a pure read over the persisted action store.
    async fn handle_compound_status(ctx: &AppCtx, args: StatusArgs) -> Result<Envelope, Error> {
        status_rewards_action(ctx, args, "rewards compound status", RewardsKind::Compound).await
    }

    /// Which rewards verb a submit/status invocation targets (selects the
    /// persisted-intent gate).
    #[derive(Clone, Copy)]
    enum RewardsKind {
        Claim,
        Compound,
    }

    impl RewardsKind {
        /// Gate the persisted action's intent for this verb (claim → only
        /// `claim_rewards`; compound → only `compound_rewards`).
        fn ensure_intent(self, intent_type: &str) -> Result<(), Error> {
            match self {
                RewardsKind::Claim => super::ensure_rewards_claim_intent(intent_type),
                RewardsKind::Compound => super::ensure_rewards_compound_intent(intent_type),
            }
        }
    }

    /// Shared `rewards {claim,compound} submit` flow (Go `submitCmd.RunE`).
    ///
    /// Flow parity with the Go runner:
    /// 1. resolve + validate the `--action-id`;
    /// 2. load the persisted action (not-found → usage `load action`);
    /// 3. gate the intent (claim → `claim_rewards`; compound → `compound_rewards`);
    /// 4. short-circuit an already-`completed` action (success + warning, no
    ///    re-broadcast);
    /// 5. resolve the execution backend from the persisted `execution_backend` +
    ///    the submit signer flags (legacy-local / OWS guards);
    /// 6. validate the resolved signer against `--from-address` + the planned
    ///    sender ([`Code::Signer`] on mismatch);
    /// 7. parse the execute options (`--gas-multiplier > 1`, durations, fee flags,
    ///    the approval/provider-tx guard flags);
    /// 8. run the bounded-approval pre-sign guardrail WITH the action context (an
    ///    inflated `approval` step without `--allow-max-approval` →
    ///    [`Code::ActionPlan`]; a no-op for a single `claim` step);
    /// 9. broadcast through the engine (persisting each transition) and emit the
    ///    terminal-state envelope (cache bypassed for execution paths).
    async fn submit_rewards_action(
        ctx: &AppCtx,
        args: SubmitArgs,
        command: &str,
        kind: RewardsKind,
    ) -> Result<Envelope, Error> {
        // 1. Resolve + validate the action id.
        let action_id =
            crate::actions::resolve_action_id(args.action_id.as_deref().unwrap_or_default())?;

        // 2. Load the persisted action (not-found → usage `load action`).
        let store = ctx.open_action_store()?;
        let mut action = store
            .get(&action_id)
            .map_err(|e| Error::wrap(Code::Usage, "load action", e))?;

        // 3. Intent gate (claim-only / compound-only).
        kind.ensure_intent(&action.intent_type)?;

        // 4. Already-completed short-circuit (no re-broadcast).
        if action.status == defi_execution::action::ActionStatus::Completed {
            let data = serde_json::to_value(&action)
                .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
            let mut env = ctx.metadata_envelope(command, data, Vec::<ProviderStatus>::new());
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

        // 7. Parse the execute options (durations, gas multiplier, fee flags, the
        //    approval/provider-tx guard flags).
        let opts = parse_execute_options(&ExecuteOptionInputs {
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
        //    inflated compound `approval` step yields the documented
        //    `allow-max-approval` hint; a no-op for a single `claim` step).
        presign_validate_action(&action, &opts)?;

        // 9. Broadcast through the engine (persisting each transition), then emit
        //    the terminal-state envelope (cache bypassed for execution paths).
        execute_resolved(&store, &mut action, resolved, opts).await?;

        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
        Ok(ctx.metadata_envelope(command, data, Vec::<ProviderStatus>::new()))
    }

    /// Shared `rewards {claim,compound} status` flow (Go `statusCmd.RunE`).
    ///
    /// A pure read over the persisted action store: resolve + validate the
    /// `--action-id`, load the action (not-found → usage `load action`), gate the
    /// intent (claim-only / compound-only), and emit the action verbatim (cache
    /// bypassed for execution paths, spec §2.5).
    async fn status_rewards_action(
        ctx: &AppCtx,
        args: StatusArgs,
        command: &str,
        kind: RewardsKind,
    ) -> Result<Envelope, Error> {
        let action_id =
            crate::actions::resolve_action_id(args.action_id.as_deref().unwrap_or_default())?;
        let store = ctx.open_action_store()?;
        let action = store
            .get(&action_id)
            .map_err(|e| Error::wrap(Code::Usage, "load action", e))?;
        kind.ensure_intent(&action.intent_type)?;
        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
        Ok(ctx.metadata_envelope(command, data, Vec::<ProviderStatus>::new()))
    }

    /// Merge structured input (`--input-json` / `--input-file`) onto the parsed
    /// `rewards claim plan` flags (Go PreRunE `applyStructuredFlagInput` over
    /// `claimArgs`). Explicitly-set flags are never overridden; an unknown key /
    /// null value is a usage error keyed on the full command path.
    fn merge_claim_plan_input(args: &mut ClaimPlanArgs) -> Result<(), Error> {
        use crate::execflags::{
            apply_structured_input, decode_bool_field, decode_string_field,
            decode_string_slice_field,
        };

        let mut explicit: std::collections::HashSet<&str> = std::collections::HashSet::new();
        if args.provider.is_some() {
            explicit.insert("provider");
        }
        if args.chain.is_some() {
            explicit.insert("chain");
        }
        if args.identity.wallet.is_some() {
            explicit.insert("wallet");
        }
        if args.identity.from_address.is_some() {
            explicit.insert("from-address");
        }
        if args.recipient.is_some() {
            explicit.insert("recipient");
        }
        if !args.assets.is_empty() {
            explicit.insert("assets");
        }
        if args.reward_token.is_some() {
            explicit.insert("reward-token");
        }
        if args.amount.is_some() {
            explicit.insert("amount");
        }
        if args.controller_address.is_some() {
            explicit.insert("controller-address");
        }
        if args.pool_address_provider.is_some() {
            explicit.insert("pool-address-provider");
        }
        if !args.simulate {
            explicit.insert("simulate");
        }

        apply_structured_input(
            &args.input,
            &explicit,
            "rewards claim plan",
            |key, canonical, raw| {
                match canonical {
                    "provider" => args.provider = Some(decode_string_field(key, raw)?),
                    "chain" => args.chain = Some(decode_string_field(key, raw)?),
                    "wallet" => args.identity.wallet = Some(decode_string_field(key, raw)?),
                    "from-address" => {
                        args.identity.from_address = Some(decode_string_field(key, raw)?)
                    }
                    "recipient" => args.recipient = Some(decode_string_field(key, raw)?),
                    "assets" => args.assets = decode_string_slice_field(key, raw)?,
                    "reward-token" => args.reward_token = Some(decode_string_field(key, raw)?),
                    "amount" => args.amount = Some(decode_string_field(key, raw)?),
                    "controller-address" => {
                        args.controller_address = Some(decode_string_field(key, raw)?)
                    }
                    "pool-address-provider" => {
                        args.pool_address_provider = Some(decode_string_field(key, raw)?)
                    }
                    "simulate" => args.simulate = decode_bool_field(key, raw)?,
                    "rpc-url" => args.rpc_url = Some(decode_string_field(key, raw)?),
                    _ => return Ok(false),
                }
                Ok(true)
            },
        )
    }

    /// Merge structured input (`--input-json` / `--input-file`) onto the parsed
    /// `rewards compound plan` flags (Go PreRunE `applyStructuredFlagInput` over
    /// `compoundArgs`). Explicitly-set flags are never overridden; an unknown key
    /// / null value is a usage error keyed on the full command path.
    fn merge_compound_plan_input(args: &mut CompoundPlanArgs) -> Result<(), Error> {
        use crate::execflags::{
            apply_structured_input, decode_bool_field, decode_string_field,
            decode_string_slice_field,
        };

        let mut explicit: std::collections::HashSet<&str> = std::collections::HashSet::new();
        if args.provider.is_some() {
            explicit.insert("provider");
        }
        if args.chain.is_some() {
            explicit.insert("chain");
        }
        if args.identity.wallet.is_some() {
            explicit.insert("wallet");
        }
        if args.identity.from_address.is_some() {
            explicit.insert("from-address");
        }
        if args.recipient.is_some() {
            explicit.insert("recipient");
        }
        if args.on_behalf_of.is_some() {
            explicit.insert("on-behalf-of");
        }
        if !args.assets.is_empty() {
            explicit.insert("assets");
        }
        if args.reward_token.is_some() {
            explicit.insert("reward-token");
        }
        if args.amount.is_some() {
            explicit.insert("amount");
        }
        if args.controller_address.is_some() {
            explicit.insert("controller-address");
        }
        if args.pool_address.is_some() {
            explicit.insert("pool-address");
        }
        if args.pool_address_provider.is_some() {
            explicit.insert("pool-address-provider");
        }
        if !args.simulate {
            explicit.insert("simulate");
        }

        apply_structured_input(
            &args.input,
            &explicit,
            "rewards compound plan",
            |key, canonical, raw| {
                match canonical {
                    "provider" => args.provider = Some(decode_string_field(key, raw)?),
                    "chain" => args.chain = Some(decode_string_field(key, raw)?),
                    "wallet" => args.identity.wallet = Some(decode_string_field(key, raw)?),
                    "from-address" => {
                        args.identity.from_address = Some(decode_string_field(key, raw)?)
                    }
                    "recipient" => args.recipient = Some(decode_string_field(key, raw)?),
                    "on-behalf-of" => args.on_behalf_of = Some(decode_string_field(key, raw)?),
                    "assets" => args.assets = decode_string_slice_field(key, raw)?,
                    "reward-token" => args.reward_token = Some(decode_string_field(key, raw)?),
                    "amount" => args.amount = Some(decode_string_field(key, raw)?),
                    "controller-address" => {
                        args.controller_address = Some(decode_string_field(key, raw)?)
                    }
                    "pool-address" => args.pool_address = Some(decode_string_field(key, raw)?),
                    "pool-address-provider" => {
                        args.pool_address_provider = Some(decode_string_field(key, raw)?)
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
    //! # Success criteria — `defi-app::rewards` (Go: `internal/app` rewards
    //! command group: `newRewardsCommand` /  `newRewardsClaimCommand` /
    //! `newRewardsCompoundCommand` in `rewards_command.go`)
    //!
    //! This module owns the **rewards-command glue**. "Correct" means it
    //! preserves the runner-owned rewards behaviors AND the stable machine
    //! contract (design spec §2.2 exit codes, §2.4 ids/amounts kept consistent,
    //! §2.5 OWS-first standard-EVM execution identity). The rewards ACTION
    //! construction (claim calldata, the 3-step compound `[claim, approval,
    //! lend_call]`, address/amount validation — covered by the
    //! `defi-execution::planner` RED suite), the registry routing
    //! (`Registry::build_rewards_{claim,compound}_action` with the `provider !=
    //! aave` unsupported gate — `defi-execution::builder` B6), the provider
    //! canonicalization (`normalize_lending_provider` — runner / providers), the
    //! shared execution-identity resolver, the submit signer/backend plumbing,
    //! and the cache-flow core are owned elsewhere and are NOT re-asserted here.
    //! Criteria:
    //!
    //! 1. **Claim request building + asset normalization + amount default.**
    //!    `build_rewards_claim_request` mirrors the Go `buildAction` closure.
    //!    (a) `--chain` parses to the chain CAIP-2 id (`1` → `eip155:1`).
    //!    (b) `--assets` is normalized by trimming each entry and dropping blanks
    //!        (Go `normalizeStringSlice`); the surviving order is preserved.
    //!    (c) An empty (or whitespace-only) `--amount` DEFAULTS to the sentinel
    //!        `"max"` (claim "claim everything").
    //!    (d) provider / sender (`from_address`) / recipient / reward-token /
    //!        simulate / rpc-url / controller-address / pool-address-provider are
    //!        carried verbatim onto the [`RewardsClaimRequest`].
    //!
    //! 2. **Claim requires at least one asset.** A `--assets` list that
    //!    normalizes to empty (nil, all-blank, or whitespace-only entries) →
    //!    [`Code::Usage`] (exit 2) with `--assets is required`. (Go `buildAction`:
    //!    `if len(assets) == 0 { return ... "--assets is required" }`.)
    //!
    //! 3. **Claim explicit amount is preserved (not overridden by the default).**
    //!    A non-empty `--amount` is carried verbatim (the `"max"` default applies
    //!    only to an empty amount).
    //!
    //! 4. **Compound request building + on_behalf_of/pool_address carry.**
    //!    `build_rewards_compound_request` mirrors the compound `buildAction`:
    //!    same chain parse + asset normalization, and the extra `on_behalf_of` /
    //!    `pool_address` fields are carried verbatim onto the
    //!    [`RewardsCompoundRequest`].
    //!
    //! 5. **Compound requires a non-empty amount (NO `"max"` default).** An empty
    //!    (or whitespace-only) `--amount` → [`Code::Usage`] (exit 2) with
    //!    `--amount is required`. This is the key claim-vs-compound divergence:
    //!    claim defaults to `"max"`, compound rejects an empty amount. (Go
    //!    compound `buildAction`: `if amount == "" { return ... "--amount is
    //!    required" }`.)
    //!
    //! 6. **Compound requires at least one asset.** Same empty-asset gate as
    //!    claim → [`Code::Usage`] (exit 2) with `--assets is required`.
    //!
    //! 7. **`rewards plan` schema identity constraints.**
    //!    `rewards_plan_identity_constraints` returns EXACTLY one `exactly_one_of`
    //!    entry over `[wallet, from_address]` with no `when` clause — the standard
    //!    OWS-first execution identity (no per-provider branching, unlike swap).
    //!    Shared by both `rewards claim plan` and `rewards compound plan`.
    //!    (Mirrors `transfer`/`bridge` `standardExecutionIdentityInputConstraints`.)
    //!
    //! 8. **Persisted-intent gates.** `ensure_rewards_claim_intent` accepts
    //!    `"claim_rewards"` and rejects any other intent (incl. the sibling
    //!    `"compound_rewards"`) with [`Code::Usage`] (exit 2) + `action is not a
    //!    rewards claim intent`. `ensure_rewards_compound_intent` accepts
    //!    `"compound_rewards"` and rejects any other intent (incl. `"claim_rewards"`)
    //!    with [`Code::Usage`] (exit 2) + `action is not a rewards compound
    //!    intent`. (Ported from the `submit` / `status` `IntentType` guards in
    //!    `rewards_command.go`.)
    //!
    //! SKIPPED (Go internal-detail / wrong-module):
    //!   * cobra flag wiring + flag defaults (`--simulate true`, `--signer local`,
    //!     `--key-source auto`, `--gas-multiplier 1.2`, `--poll-interval 2s`,
    //!     `--step-timeout 2m`, required-flag marking for
    //!     `--provider`/`--chain`/`--assets`/`--reward-token`[/`--amount`]) —
    //!     harness concern, asserted by the integration golden-CLI / schema suites
    //!     (`TestRunnerExecutionCommandsInSchema` covers `rewards claim plan` /
    //!     `rewards compound status` schema presence), not this unit;
    //!   * the rewards calldata packing + the 3-step compound assembly + the
    //!     address/amount validation — owned by
    //!     `defi_execution::planner::{build_aave_rewards_claim_action,
    //!     build_aave_rewards_compound_action}` (its own RED suite);
    //!   * the registry routing + the `provider != aave` unsupported gate
    //!     (`rewards execution currently supports only provider=aave`) — owned by
    //!     `defi_execution::builder` (B6: `rewards_claim_routing_rejects_*`);
    //!   * the provider canonicalization (`normalize_lending_provider`) — runner /
    //!     `defi_providers::normalize` concern;
    //!   * the OWS-vs-legacy execution-backend stamping + wallet-id persistence —
    //!     shared execution-identity / action-store concern;
    //!   * the submit signer/backend plumbing, pre-sign guardrails, receipt
    //!     polling, and the already-completed short-circuit — `defi-execution` /
    //!     runner concern;
    //!   * the cache bypass for execution paths (`TestShouldOpenCacheBypasses
    //!     ExecutionCommands` / `TestShouldOpenActionStore` covering `rewards
    //!     claim plan` / `rewards compound status`) — runner concern.

    use super::*;
    use defi_errors::{exit_code, Code};

    // --- helpers -----------------------------------------------------------

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    // Canonical-but-arbitrary EVM identities (NOT validated by the request
    // builder — that's the planner's job — but carried verbatim).
    const SENDER: &str = "0x00000000000000000000000000000000000000aa";
    const RECIPIENT: &str = "0x00000000000000000000000000000000000000bb";
    const ON_BEHALF: &str = "0x00000000000000000000000000000000000000cc";
    // An Aave incentives "asset" (aToken/debtToken) source + the reward token.
    const ASSET_A: &str = "0x1111111111111111111111111111111111111111";
    const ASSET_B: &str = "0x2222222222222222222222222222222222222222";
    const REWARD: &str = "0x3333333333333333333333333333333333333333";

    fn assets(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    // --- 1. claim request building -----------------------------------------

    #[test]
    fn build_claim_request_parses_chain_and_carries_fields() {
        let req = build_rewards_claim_request(
            "aave",
            "1",
            SENDER,
            RECIPIENT,
            &assets(&[ASSET_A]),
            REWARD,
            "1000000",
            true,
            "http://127.0.0.1:8545",
            "0x4444444444444444444444444444444444444444",
            "0x5555555555555555555555555555555555555555",
        )
        .expect("claim request built");

        assert_eq!(req.provider, "aave");
        assert_eq!(req.chain.caip2, "eip155:1");
        assert_eq!(req.sender, SENDER);
        assert_eq!(req.recipient, RECIPIENT);
        assert_eq!(req.assets, assets(&[ASSET_A]));
        assert_eq!(req.reward_token, REWARD);
        assert_eq!(req.amount_base_units, "1000000");
        assert!(req.simulate);
        assert_eq!(req.rpc_url, "http://127.0.0.1:8545");
        assert_eq!(
            req.controller_address,
            "0x4444444444444444444444444444444444444444"
        );
        assert_eq!(
            req.pool_address_provider,
            "0x5555555555555555555555555555555555555555"
        );
    }

    #[test]
    fn build_claim_request_normalizes_assets_and_preserves_order() {
        // Blanks / whitespace-only entries are dropped; surviving order kept.
        let req = build_rewards_claim_request(
            "aave",
            "1",
            SENDER,
            RECIPIENT,
            &assets(&["  ", ASSET_A, "", &format!("  {ASSET_B}  ")]),
            REWARD,
            "100",
            true,
            "",
            "",
            "",
        )
        .expect("assets normalized");
        // Whitespace trimmed, blanks dropped, order preserved.
        assert_eq!(req.assets, assets(&[ASSET_A, ASSET_B]));
    }

    #[test]
    fn build_claim_request_defaults_empty_amount_to_max() {
        // Claim "claim everything": an empty (or whitespace-only) amount defaults
        // to the sentinel "max".
        let req = build_rewards_claim_request(
            "aave",
            "1",
            SENDER,
            RECIPIENT,
            &assets(&[ASSET_A]),
            REWARD,
            "   ",
            false,
            "",
            "",
            "",
        )
        .expect("empty amount defaults to max");
        assert_eq!(req.amount_base_units, "max");
        assert!(!req.simulate);
    }

    #[test]
    fn build_claim_request_preserves_explicit_amount() {
        // The "max" default applies ONLY to an empty amount.
        let req = build_rewards_claim_request(
            "aave",
            "1",
            SENDER,
            RECIPIENT,
            &assets(&[ASSET_A]),
            REWARD,
            "250000",
            true,
            "",
            "",
            "",
        )
        .expect("explicit amount preserved");
        assert_eq!(req.amount_base_units, "250000");
    }

    // --- 2. claim requires at least one asset ------------------------------

    #[test]
    fn build_claim_request_rejects_empty_assets() {
        let err = build_rewards_claim_request(
            "aave",
            "1",
            SENDER,
            RECIPIENT,
            &[],
            REWARD,
            "max",
            true,
            "",
            "",
            "",
        )
        .expect_err("empty assets rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("--assets is required"),
            "got: {err}"
        );
    }

    #[test]
    fn build_claim_request_rejects_all_blank_assets() {
        // A non-empty list that normalizes to empty is still "no assets".
        let err = build_rewards_claim_request(
            "aave",
            "1",
            SENDER,
            RECIPIENT,
            &assets(&["", "   ", "\t"]),
            REWARD,
            "max",
            true,
            "",
            "",
            "",
        )
        .expect_err("all-blank assets rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("--assets is required"),
            "got: {err}"
        );
    }

    // --- 3. compound request building --------------------------------------

    #[test]
    fn build_compound_request_carries_on_behalf_and_pool_address() {
        let req = build_rewards_compound_request(
            "aave",
            "137",
            SENDER,
            RECIPIENT,
            ON_BEHALF,
            &assets(&[ASSET_A, ASSET_B]),
            REWARD,
            "500000",
            true,
            "http://127.0.0.1:8545",
            "0x4444444444444444444444444444444444444444",
            "0x6666666666666666666666666666666666666666",
            "0x5555555555555555555555555555555555555555",
        )
        .expect("compound request built");

        assert_eq!(req.provider, "aave");
        assert_eq!(req.chain.caip2, "eip155:137");
        assert_eq!(req.sender, SENDER);
        assert_eq!(req.recipient, RECIPIENT);
        assert_eq!(req.on_behalf_of, ON_BEHALF);
        assert_eq!(req.assets, assets(&[ASSET_A, ASSET_B]));
        assert_eq!(req.reward_token, REWARD);
        assert_eq!(req.amount_base_units, "500000");
        assert!(req.simulate);
        assert_eq!(req.rpc_url, "http://127.0.0.1:8545");
        assert_eq!(
            req.controller_address,
            "0x4444444444444444444444444444444444444444"
        );
        assert_eq!(
            req.pool_address,
            "0x6666666666666666666666666666666666666666"
        );
        assert_eq!(
            req.pool_address_provider,
            "0x5555555555555555555555555555555555555555"
        );
    }

    #[test]
    fn build_compound_request_normalizes_assets() {
        let req = build_rewards_compound_request(
            "aave",
            "1",
            SENDER,
            RECIPIENT,
            "",
            &assets(&[&format!(" {ASSET_A} "), "", ASSET_B]),
            REWARD,
            "1",
            true,
            "",
            "",
            "",
            "",
        )
        .expect("compound assets normalized");
        assert_eq!(req.assets, assets(&[ASSET_A, ASSET_B]));
    }

    // --- 4. compound requires a non-empty amount (no "max" default) --------

    #[test]
    fn build_compound_request_rejects_empty_amount() {
        // Key claim-vs-compound divergence: compound has NO "max" default.
        let err = build_rewards_compound_request(
            "aave",
            "1",
            SENDER,
            RECIPIENT,
            "",
            &assets(&[ASSET_A]),
            REWARD,
            "  ",
            true,
            "",
            "",
            "",
            "",
        )
        .expect_err("empty compound amount rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("--amount is required"),
            "got: {err}"
        );
    }

    // --- 5. compound requires at least one asset ---------------------------

    #[test]
    fn build_compound_request_rejects_empty_assets() {
        let err = build_rewards_compound_request(
            "aave",
            "1",
            SENDER,
            RECIPIENT,
            "",
            &[],
            REWARD,
            "1",
            true,
            "",
            "",
            "",
            "",
        )
        .expect_err("empty compound assets rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("--assets is required"),
            "got: {err}"
        );
    }

    // --- 6. rewards plan schema identity constraints -----------------------

    #[test]
    fn plan_identity_constraints_are_standard_exactly_one_of() {
        let constraints = rewards_plan_identity_constraints();
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].kind, "exactly_one_of");
        assert_eq!(
            constraints[0].fields,
            vec!["wallet".to_string(), "from_address".to_string()]
        );
        // No per-provider `when` clause — rewards planning is OWS-first /
        // standard EVM (no Tempo/TaikoSwap-style branching like swap).
        assert!(
            constraints[0].when.is_empty(),
            "standard identity constraint has no `when` clause"
        );
    }

    // --- 7. persisted-intent gates -----------------------------------------

    #[test]
    fn ensure_claim_intent_accepts_claim_rewards() {
        ensure_rewards_claim_intent("claim_rewards").expect("claim_rewards accepted");
    }

    #[test]
    fn ensure_claim_intent_rejects_non_claim() {
        // The sibling compound intent must NOT pass the claim gate.
        let err = ensure_rewards_claim_intent("compound_rewards")
            .expect_err("compound_rewards rejected by claim gate");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("action is not a rewards claim intent"),
            "got: {err}"
        );
    }

    #[test]
    fn ensure_compound_intent_accepts_compound_rewards() {
        ensure_rewards_compound_intent("compound_rewards").expect("compound_rewards accepted");
    }

    #[test]
    fn ensure_compound_intent_rejects_non_compound() {
        // The sibling claim intent must NOT pass the compound gate.
        let err = ensure_rewards_compound_intent("claim_rewards")
            .expect_err("claim_rewards rejected by compound gate");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("action is not a rewards compound intent"),
            "got: {err}"
        );
    }
}

#[cfg(test)]
mod app_tests {
    //! # Success criteria — `rewards {claim,compound} plan` app-level handlers
    //! (WS3, exec-plan)
    //!
    //! Go oracle: `internal/app/rewards_command.go` — the `planCmd.RunE` closures
    //! inside `newRewardsClaimCommand` / `newRewardsCompoundCommand`. These tests
    //! drive [`cli::handle`] (the real dispatch entry point the binary calls)
    //! end-to-end for `rewards claim plan` and `rewards compound plan` ONLY,
    //! asserting the full machine contract the Go runner emits via
    //! `emitSuccess(...)` / `renderError(...)`.
    //!
    //! Unlike `transfer`/`approvals` (whose internal planners build calldata with
    //! no network), the Aave rewards planner reads on-chain (the incentives
    //! controller via the pool-address-provider, the Aave pool via `getPool()`,
    //! and an ERC-20 `allowance` for the compound supply approval). The tests stay
    //! offline + deterministic by exercising BOTH the no-network short-circuits
    //! (`--controller-address` / `--pool-address` provided) AND the on-chain
    //! resolution path through a `wiremock` JSON-RPC mock injected via the
    //! already-present `--rpc-url` seam. Persistence uses a real
    //! [`defi_execution::store::Store`] over a `tempfile` directory. Identity is
    //! exercised through the OFFLINE `--from-address` (legacy_local) path so no OWS
    //! vault / network is touched; the `--wallet` happy path (OWS resolve) is WS4b
    //! e2e territory and is asserted here only via its offline guard rejections.
    //!
    //! Rewards is the only execution group that routes EXCLUSIVELY to
    //! `provider=aave` (no `native`, no Morpho/Moonwell), so the provider-status
    //! row + the `provider != aave` unsupported gate are asserted at the handler
    //! boundary (the routing internals live in `defi-execution::builder` B6). The
    //! claim-vs-compound divergences (claim defaults the amount to `"max"`,
    //! compound rejects an empty amount AND the `"max"` sentinel; compound is a
    //! 3-step `[claim, approval, supply]` plan) are asserted here as the unique
    //! rewards-plan behaviors.
    //!
    //! Criteria (each a failing test until `cli::handle` routes `*Plan` to a real
    //! handler — the stub currently returns the `AppCtx::unimplemented` error):
    //!
    //! 1. **Claim plan success envelope (legacy `--from-address`).** A valid
    //!    `rewards claim plan --provider aave --chain 1 --assets 0x11.. --reward-token
    //!    0x33.. --amount 1000000 --controller-address 0x44.. --from-address 0x..aa`
    //!    returns an `Ok(Envelope)` (exit 0) with: `version == "v1"`, `success ==
    //!    true`, `error == None`, `meta.partial == false`, `meta.command ==
    //!    "rewards claim plan"`, `meta.cache == {status:"bypass", age_ms:0,
    //!    stale:false}` (execution paths bypass the cache, spec §2.5), and
    //!    `meta.providers == [{name:"aave", status:"ok"}]` (Go
    //!    `statusFromErr(nil) == "ok"`; the provider status is keyed on the
    //!    normalized lending provider, NOT `native`).
    //!
    //! 2. **Claim planned action `data` shape.** `env.data` is the serialized
    //!    [`Action`]: `action_id` matches `^act_[0-9a-f]{32}$`; `intent_type ==
    //!    "claim_rewards"`; `provider == "aave"`; `status == "planned"`; `chain_id
    //!    == "eip155:1"`; `from_address` == the EIP-55 checksum of the sender;
    //!    `to_address` == the recipient (defaults to the sender when `--recipient`
    //!    is empty); `input_amount == "1000000"`; exactly ONE step with `type ==
    //!    "claim"`, `value == "0"`, `target` == the controller address, and
    //!    `chain_id == "eip155:1"`; `metadata.protocol == "aave"`,
    //!    `metadata.controller` == the controller, `metadata.reward_token` == the
    //!    reward token, and `metadata.assets` == the normalized asset list.
    //!
    //! 3. **Claim step calldata reuses the Aave rewards ABI golden.** With assets
    //!    `[0x11..]`, amount `1000000`, recipient (default sender), and reward
    //!    `0x33..`, the step `data` equals the alloy `AAVE_REWARDS_ABI`
    //!    `claimRewards(assets, amount, to, reward)` encoding (computed in-test from
    //!    `defi_registry::AAVE_REWARDS_ABI`, the same source the planner uses). This
    //!    proves the handler routes through `build_aave_rewards_claim_action` (no
    //!    re-encoding).
    //!
    //! 4. **Claim legacy-identity warning + backend.** The `--from-address` path
    //!    stamps `execution_backend == "legacy_local"` on the action AND surfaces
    //!    the Go warning `--wallet (OWS) is recommended over --from-address for
    //!    planning; see docs for details` in `env.warnings`.
    //!
    //! 5. **Claim plan persists the action to the Store.** After a successful plan
    //!    the action is retrievable by its `action_id` from a freshly opened Store
    //!    over the same path, with matching `intent_type == "claim_rewards"`,
    //!    `input_amount`, and `provider == "aave"`.
    //!
    //! 6. **Claim defaults an empty `--amount` to `"max"` through the handler.**
    //!    Omitting `--amount` yields a `claim` action whose calldata encodes the
    //!    `max` sentinel amount (`U256::MAX`) — the "claim everything" default
    //!    (Go `buildAction`: empty amount → `"max"`). The planner parses `"max"`
    //!    to `U256::MAX`, so `input_amount` is the decimal `U256::MAX` string.
    //!
    //! 7. **Claim auto-resolves the incentives controller via RPC.** Omitting
    //!    `--controller-address` routes through the pool-address-provider
    //!    `getAddress(INCENTIVES_CONTROLLER)` on-chain lookup; pointed at a
    //!    `wiremock` JSON-RPC mock that returns the controller address word, the
    //!    plan succeeds and the `claim` step targets the resolved controller. This
    //!    proves the `--rpc-url` seam reaches the planner.
    //!
    //! 8. **Claim provider gating.**
    //!    (a) `--provider morpho` → [`Code::Unsupported`] (exit 13) with `rewards
    //!        execution currently supports only provider=aave`;
    //!    (b) a missing/empty `--provider` → [`Code::Usage`] (exit 2) with
    //!        `--provider is required`.
    //!    On each, nothing is persisted.
    //!
    //! 9. **Claim identity-constraint errors (offline).**
    //!    (a) BOTH `--wallet` and `--from-address` → [`Code::Usage`] (exit 2);
    //!    (b) NEITHER → [`Code::Usage`] (exit 2);
    //!    (c) a malformed `--from-address` → [`Code::Usage`] (exit 2);
    //!    (d) `--wallet` on a Tempo chain → [`Code::Unsupported`] (exit 13)
    //!        (`--wallet planning is not supported on Tempo chains yet`).
    //!    On every error the handler returns the typed `Err(Error)` (the runner
    //!    renders the full error envelope to stderr, spec §2.1) and persists
    //!    NOTHING.
    //!
    //! 10. **Claim requires at least one asset (through the handler).** An empty /
    //!     all-blank `--assets` → [`Code::Usage`] (exit 2) with `--assets is
    //!     required`. Nothing persisted.
    //!
    //! SKIPPED (covered elsewhere / wrong unit):
    //!   * the `claimRewards` calldata ABI encoding itself — `defi-evm::abi` golden
    //!     (`encode_claim_rewards_with_address_array_matches_golden`);
    //!   * the planner's sender/recipient/reward/asset hex validation + amount
    //!     parsing internals — `defi-execution::planner` RED suite;
    //!   * the registry routing + the `provider != aave` unsupported message —
    //!     `defi-execution::builder` B6 (asserted here only at the handler
    //!     boundary for the contract exit code);
    //!   * the OWS `--wallet` happy-path resolve + wallet-id persistence — WS4b
    //!     e2e (here only its offline guard rejections are asserted);
    //!   * `--input-json`/`--input-file` precedence — structured-input unit;
    //!   * clap flag defaults + required-flag marking — schema/CLI suites;
    //!   * `rewards claim submit`/`status` — WS4.

    use super::cli::{handle, ClaimPlanArgs, ClaimVerbCmd, RewardsCmd};
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags};
    use alloy::dyn_abi::JsonAbiExt;
    use alloy::json_abi::JsonAbi;
    use alloy::primitives::U256;
    use defi_config::Settings;
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    // --- contract constants ------------------------------------------------

    /// Sender EOA (legacy `--from-address` identity); its EIP-55 checksum lands on
    /// the action.
    const SENDER: &str = "0x00000000000000000000000000000000000000aa";
    /// An Aave incentives "asset" (aToken/debtToken source).
    const ASSET_A: &str = "0x1111111111111111111111111111111111111111";
    /// The reward token claimed from the incentives controller.
    const REWARD: &str = "0x3333333333333333333333333333333333333333";
    /// The incentives controller (`--controller-address` override) — short-circuits
    /// the on-chain `getAddress(INCENTIVES_CONTROLLER)` lookup.
    const CONTROLLER: &str = "0x4444444444444444444444444444444444444444";
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
            timeout: Duration::from_secs(5),
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

    /// A `rewards claim plan` `ClaimPlanArgs` with the canonical happy-path values;
    /// mutate per test. `--controller-address` is set so no on-chain controller
    /// lookup is needed (claim build does no eth_call on this path).
    fn claim_args(rpc: &str) -> ClaimPlanArgs {
        ClaimPlanArgs {
            chain: Some("1".to_string()),
            assets: vec![ASSET_A.to_string()],
            reward_token: Some(REWARD.to_string()),
            amount: Some("1000000".to_string()),
            recipient: None,
            controller_address: Some(CONTROLLER.to_string()),
            pool_address_provider: None,
            provider: Some("aave".to_string()),
            rpc_url: Some(rpc.to_string()),
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(SENDER.to_string()),
            },
            input: InputFlags::default(),
        }
    }

    async fn run_claim(dir: &Path, args: ClaimPlanArgs) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(&ctx, RewardsCmd::Claim(ClaimVerbCmd::Plan(args))).await
    }

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn unsupported_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn action_data(env: &Envelope) -> Value {
        env.data.clone().expect("plan envelope carries `data`")
    }

    /// True iff no action is persisted under `dir` (error paths must persist
    /// nothing). A never-created store counts as empty.
    fn no_actions_persisted(dir: &Path) -> bool {
        let store = match ActionStore::open(dir.join("actions.db"), dir.join("actions.lock")) {
            Ok(store) => store,
            Err(_) => return true,
        };
        store
            .list("", 1000)
            .map(|actions| actions.is_empty())
            .unwrap_or(true)
    }

    // --- wiremock JSON-RPC: every eth_call returns `result` ----------------

    /// A `wiremock` responder that wraps a fixed hex `result` in a JSON-RPC
    /// success envelope, echoing the incoming request `id` (mirrors the
    /// `defi-execution` planner `EchoIdResponder`).
    struct EchoIdResponder {
        result: String,
    }

    impl Respond for EchoIdResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let id = serde_json::from_slice::<Value>(&request.body)
                .ok()
                .and_then(|body| body.get("id").cloned())
                .unwrap_or_else(|| Value::from(1));
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": self.result,
            }))
        }
    }

    /// A mock JSON-RPC endpoint answering every `eth_call` with the ABI word for
    /// `addr` (12 zero bytes + 20 address bytes). Used by the controller
    /// auto-resolution test (no `--controller-address`).
    async fn address_word_rpc(addr: &str) -> MockServer {
        let server = MockServer::start().await;
        let word = format!("0x000000000000000000000000{}", &addr[2..]);
        Mock::given(method("POST"))
            .respond_with(EchoIdResponder { result: word })
            .mount(&server)
            .await;
        server
    }

    // --- in-test alloy/ABI golden (reuses AAVE_REWARDS_ABI) ----------------

    /// The expected `claimRewards(assets, amount, to, reward)` calldata, computed
    /// from `defi_registry::AAVE_REWARDS_ABI` (the same source the planner uses).
    fn claim_calldata(assets: &[&str], amount: U256, to: &str, reward: &str) -> String {
        use alloy::dyn_abi::DynSolValue;
        let abi: JsonAbi =
            serde_json::from_str(defi_registry::AAVE_REWARDS_ABI).expect("parse rewards abi");
        let f = abi
            .function("claimRewards")
            .and_then(|o| o.first())
            .cloned()
            .expect("claimRewards present");
        let asset_vals: Vec<DynSolValue> = assets
            .iter()
            .map(|a| DynSolValue::Address(a.parse().expect("valid asset address")))
            .collect();
        let data = f
            .abi_encode_input(&[
                DynSolValue::Array(asset_vals),
                DynSolValue::Uint(amount, 256),
                DynSolValue::Address(to.parse().expect("valid to address")),
                DynSolValue::Address(reward.parse().expect("valid reward address")),
            ])
            .expect("encode claimRewards");
        format!("0x{}", hex::encode(data))
    }

    // --- 1, 2, 4. claim happy path -----------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_emits_success_envelope_and_action_shape() {
        // No eth_call is made when --controller-address is provided, but connect
        // must succeed against a parseable URL; a wiremock URI is harmless here.
        let rpc = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let env = run_claim(tmp.path(), claim_args(&rpc.uri()))
            .await
            .expect("aave rewards claim plan should succeed");

        // Envelope contract (Go `emitSuccess`).
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "rewards claim plan");

        // Execution paths bypass the cache (spec §2.5).
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        // One provider status keyed on the normalized lending provider, ok.
        assert_eq!(env.meta.providers.len(), 1, "exactly one provider status");
        assert_eq!(env.meta.providers[0].name, "aave");
        assert_eq!(env.meta.providers[0].status, "ok");

        // Action `data` shape (Go persisted action).
        let data = action_data(&env);
        let action_id = data["action_id"].as_str().expect("action_id string");
        assert!(
            action_id.strip_prefix("act_").is_some_and(|rest| rest.len() == 32
                && rest.bytes().all(|b| b.is_ascii_hexdigit())),
            "action_id must match act_<32 hex>: got {action_id}"
        );
        assert_eq!(data["intent_type"], Value::from("claim_rewards"));
        assert_eq!(data["provider"], Value::from("aave"));
        assert_eq!(data["status"], Value::from("planned"));
        assert_eq!(data["chain_id"], Value::from("eip155:1"));
        assert_eq!(
            data["from_address"].as_str().unwrap().to_lowercase(),
            SENDER.to_lowercase(),
            "from_address is the (checksummed) sender"
        );
        // recipient defaults to the sender when --recipient is empty.
        assert_eq!(
            data["to_address"].as_str().unwrap().to_lowercase(),
            SENDER.to_lowercase(),
            "to_address defaults to the sender"
        );
        assert_eq!(data["input_amount"], Value::from("1000000"));

        // Exactly one claim step, value 0, target = controller, chain carried.
        let steps = data["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1, "claim is a single-step action");
        assert_eq!(steps[0]["type"], Value::from("claim"));
        assert_eq!(steps[0]["value"], Value::from("0"));
        assert_eq!(steps[0]["chain_id"], Value::from("eip155:1"));
        assert_eq!(
            steps[0]["target"].as_str().unwrap().to_lowercase(),
            CONTROLLER.to_lowercase(),
            "claim step targets the incentives controller"
        );

        // metadata carries the Aave rewards context.
        let meta = data["metadata"].as_object().expect("metadata object");
        assert_eq!(meta.get("protocol"), Some(&Value::from("aave")));
        assert_eq!(
            meta.get("controller")
                .map(|v| v.as_str().unwrap().to_lowercase()),
            Some(CONTROLLER.to_lowercase())
        );
        assert_eq!(
            meta.get("reward_token")
                .map(|v| v.as_str().unwrap().to_lowercase()),
            Some(REWARD.to_lowercase())
        );
        let assets = meta
            .get("assets")
            .and_then(|v| v.as_array())
            .expect("assets array");
        assert_eq!(assets.len(), 1);
        assert_eq!(
            assets[0].as_str().unwrap().to_lowercase(),
            ASSET_A.to_lowercase()
        );

        // Legacy backend stamping + warning (criterion 4).
        assert_eq!(data["execution_backend"], Value::from("legacy_local"));
        assert!(
            env.warnings.iter().any(|w| w == LEGACY_WARNING),
            "legacy --from-address plan surfaces the OWS-recommended warning; got {:?}",
            env.warnings
        );
    }

    // --- 3. claim step calldata reuses the Aave rewards ABI golden ----------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_step_calldata_matches_aave_rewards_golden() {
        let rpc = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let env = run_claim(tmp.path(), claim_args(&rpc.uri()))
            .await
            .expect("aave rewards claim plan should succeed");
        let data = action_data(&env);
        let calldata = data["steps"][0]["data"].as_str().expect("step data string");
        // recipient defaults to the sender; amount 1_000_000; one asset, reward.
        assert_eq!(
            calldata.to_lowercase(),
            claim_calldata(&[ASSET_A], U256::from(1_000_000u64), SENDER, REWARD).to_lowercase(),
            "claim step calldata must equal the alloy AAVE_REWARDS_ABI claimRewards golden"
        );
    }

    // --- 5. claim plan persists the action to the Store --------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_persists_action_to_store() {
        let rpc = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let settings = exec_settings(tmp.path());
        let ctx = AppCtx::new(settings.clone());
        let env = handle(
            &ctx,
            RewardsCmd::Claim(ClaimVerbCmd::Plan(claim_args(&rpc.uri()))),
        )
        .await
        .expect("aave rewards claim plan should succeed");
        let action_id = action_data(&env)["action_id"]
            .as_str()
            .expect("action_id")
            .to_string();

        let store = ActionStore::open(&settings.action_store_path, &settings.action_lock_path)
            .expect("reopen action store");
        let persisted = store
            .get(&action_id)
            .expect("planned action retrievable by id");
        assert_eq!(persisted.intent_type, "claim_rewards");
        assert_eq!(persisted.input_amount, "1000000");
        assert_eq!(persisted.provider, "aave");
    }

    // --- 6. claim defaults an empty --amount to "max" ----------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_defaults_empty_amount_to_max() {
        let rpc = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = claim_args(&rpc.uri());
        args.amount = None; // "claim everything" -> "max" -> U256::MAX.
        let env = run_claim(tmp.path(), args)
            .await
            .expect("empty-amount claim plan should default to max");
        let data = action_data(&env);
        // The planner parses "max" to U256::MAX; input_amount is its decimal form.
        assert_eq!(
            data["input_amount"],
            Value::from(U256::MAX.to_string()),
            "empty --amount defaults to the max sentinel (U256::MAX)"
        );
        // The claim step calldata encodes U256::MAX as the amount.
        let calldata = data["steps"][0]["data"].as_str().expect("step data string");
        assert_eq!(
            calldata.to_lowercase(),
            claim_calldata(&[ASSET_A], U256::MAX, SENDER, REWARD).to_lowercase(),
            "max-amount claim encodes U256::MAX"
        );
    }

    // --- 7. claim auto-resolves the incentives controller via RPC ----------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_auto_resolves_controller_via_rpc() {
        // No --controller-address: the planner must read the controller on-chain
        // via the chain-default pool-address-provider. The mock answers the
        // getAddress(INCENTIVES_CONTROLLER) eth_call with the controller word.
        let rpc = address_word_rpc(CONTROLLER).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = claim_args(&rpc.uri());
        args.controller_address = None; // force the on-chain lookup.
        let env = run_claim(tmp.path(), args)
            .await
            .expect("controller auto-resolution should succeed against the mock RPC");
        let data = action_data(&env);
        assert_eq!(
            data["steps"][0]["target"].as_str().unwrap().to_lowercase(),
            CONTROLLER.to_lowercase(),
            "claim step targets the RPC-resolved incentives controller"
        );
    }

    // --- 8. claim provider gating ------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_rejects_non_aave_provider() {
        let rpc = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = claim_args(&rpc.uri());
        args.provider = Some("morpho".to_string());
        let err = run_claim(tmp.path(), args)
            .await
            .expect_err("rewards plan rejects non-aave providers");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(unsupported_exit(&err), 13);
        assert!(
            err.to_string()
                .contains("rewards execution currently supports only provider=aave"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_rejects_missing_provider() {
        let rpc = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = claim_args(&rpc.uri());
        args.provider = None;
        let err = run_claim(tmp.path(), args)
            .await
            .expect_err("rewards plan requires a provider");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- structured input (`--input-json` / `--input-file`) ----------------
    //
    // Go: `configureStructuredInput[claimArgs]` wires the PreRunE merge onto
    // `rewards claim plan`. JSON fills flags (incl. the `assets` string array);
    // explicit flags override JSON; unknown keys / null are usage errors that
    // persist nothing.

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_resolves_all_flags_from_input_json() {
        let rpc = MockServer::start().await; // controller provided -> no eth_call.
        let tmp = TempDir::new().expect("tempdir");
        let args = ClaimPlanArgs {
            input: InputFlags {
                input_json: Some(format!(
                    r#"{{"provider":"aave","chain":"1","assets":["{ASSET_A}"],"reward_token":"{REWARD}","amount":"1000000","from_address":"{SENDER}","controller_address":"{CONTROLLER}","rpc_url":"{rpc}"}}"#,
                    rpc = rpc.uri()
                )),
                input_file: None,
            },
            ..ClaimPlanArgs::default()
        };
        let env = run_claim(tmp.path(), args)
            .await
            .expect("input-json should fill all flags (incl. the assets array)");
        assert!(env.success);
        assert_eq!(env.meta.command, "rewards claim plan");
        let data = action_data(&env);
        assert_eq!(data["intent_type"], Value::from("claim_rewards"));
        assert_eq!(data["provider"], Value::from("aave"));
        // The claim step calldata reuses the assets/amount/reward from the JSON.
        let calldata = data["steps"][0]["data"].as_str().expect("claim step data");
        assert_eq!(
            calldata.to_lowercase(),
            claim_calldata(&[ASSET_A], U256::from(1_000_000u64), SENDER, REWARD).to_lowercase()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_input_json_unknown_field_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        let args = ClaimPlanArgs {
            input: InputFlags {
                input_json: Some(r#"{"provider":"aave","bogus":"x"}"#.to_string()),
                input_file: None,
            },
            ..ClaimPlanArgs::default()
        };
        let err = run_claim(tmp.path(), args)
            .await
            .expect_err("unknown structured-input field must be a usage error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert_eq!(
            err.message,
            "structured input field \"bogus\" is not supported by rewards claim plan"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 9. claim identity-constraint errors (offline) ---------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_rejects_both_identity_inputs() {
        let rpc = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = claim_args(&rpc.uri());
        args.identity.wallet = Some("alice".to_string());
        // from_address already set in base.
        let err = run_claim(tmp.path(), args)
            .await
            .expect_err("both identity inputs must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_rejects_missing_identity_inputs() {
        let rpc = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = claim_args(&rpc.uri());
        args.identity.wallet = None;
        args.identity.from_address = None;
        let err = run_claim(tmp.path(), args)
            .await
            .expect_err("missing identity inputs must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_rejects_malformed_from_address() {
        let rpc = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = claim_args(&rpc.uri());
        args.identity.from_address = Some("0xnot-an-address".to_string());
        let err = run_claim(tmp.path(), args)
            .await
            .expect_err("malformed --from-address must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_rejects_wallet_on_tempo_chain() {
        let rpc = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = claim_args(&rpc.uri());
        args.chain = Some("tempo".to_string()); // eip155:4217 (Tempo mainnet)
        args.identity.from_address = None;
        args.identity.wallet = Some("alice".to_string());
        let err = run_claim(tmp.path(), args)
            .await
            .expect_err("--wallet on Tempo must be rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .contains("--wallet planning is not supported on Tempo chains yet"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 10. claim requires at least one asset (through the handler) -------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_rejects_empty_assets() {
        let rpc = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = claim_args(&rpc.uri());
        args.assets = Vec::new();
        let err = run_claim(tmp.path(), args)
            .await
            .expect_err("empty --assets must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("--assets is required"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_plan_rejects_all_blank_assets() {
        let rpc = MockServer::start().await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = claim_args(&rpc.uri());
        args.assets = vec!["   ".to_string(), "".to_string()];
        let err = run_claim(tmp.path(), args)
            .await
            .expect_err("all-blank --assets must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }
}

#[cfg(test)]
mod compound_app_tests {
    //! # Success criteria — `rewards compound plan` app-level handler (WS3,
    //! exec-plan)
    //!
    //! Go oracle: `internal/app/rewards_command.go` `planCmd.RunE` inside
    //! `newRewardsCompoundCommand`. These tests drive [`cli::handle`] end-to-end
    //! for `rewards compound plan` ONLY. Compound is the only THREE-step rewards
    //! plan: it claims the reward (`claim` step), approves the reward token for the
    //! Aave pool (`approval` step, only when allowance is insufficient), then
    //! supplies the claimed reward back into Aave (`lend_call` step). It therefore
    //! reads on-chain (controller + pool resolution + allowance), all injected via
    //! the `--rpc-url` `wiremock` seam; `--controller-address` + `--pool-address`
    //! short-circuit the discovery lookups so only the allowance `eth_call` hits
    //! the mock. Persistence uses a real [`defi_execution::store::Store`].
    //!
    //! Criteria (each failing until `cli::handle` routes `Compound(Plan)` to a real
    //! handler):
    //!
    //! 1. **Compound plan success envelope.** A valid `rewards compound plan
    //!    --provider aave --chain 1 --assets 0x11.. --reward-token 0x33.. --amount
    //!    1000000 --controller-address 0x44.. --pool-address 0x..cc --rpc-url
    //!    <mock> --from-address 0x..aa` returns `Ok(Envelope)` (exit 0) with
    //!    `version == "v1"`, `success == true`, `error == None`, `meta.partial ==
    //!    false`, `meta.command == "rewards compound plan"`, `meta.cache ==
    //!    {status:"bypass", age_ms:0, stale:false}`, and `meta.providers ==
    //!    [{name:"aave", status:"ok"}]`.
    //!
    //! 2. **Compound 3-step action shape (insufficient allowance).** With the
    //!    allowance mock returning `0` (< amount), `env.data` is the serialized
    //!    [`Action`] with `intent_type == "compound_rewards"`, `provider ==
    //!    "aave"`, `status == "planned"`, and EXACTLY the steps `["claim",
    //!    "approval", "lend_call"]` in order: the `claim` step targets the
    //!    controller, the `approval` step targets the reward token, the `lend_call`
    //!    step targets the pool (`value == "0"`, `chain_id == "eip155:1"`).
    //!    `metadata.compound == true`, `metadata.pool` == the pool, and
    //!    `metadata.on_behalf_of` == the sender (default).
    //!
    //! 3. **Compound skips the approval when allowance is sufficient.** With the
    //!    allowance mock returning a value `>= amount`, the steps collapse to
    //!    `["claim", "lend_call"]` (no `approval` step).
    //!
    //! 4. **Compound supply step calldata reuses the Aave pool ABI golden.** The
    //!    `lend_call` step `data` equals the alloy `AAVE_POOL_ABI`
    //!    `supply(reward, amount, onBehalfOf, referralCode=0)` encoding (computed
    //!    in-test from `defi_registry::AAVE_POOL_ABI`), proving the handler routes
    //!    through `build_aave_rewards_compound_action`.
    //!
    //! 5. **Compound persists the action to the Store.** Retrievable by
    //!    `action_id` with `intent_type == "compound_rewards"`, `input_amount`,
    //!    `provider == "aave"`.
    //!
    //! 6. **Compound requires a non-empty `--amount` (NO `"max"` default).** An
    //!    empty `--amount` → [`Code::Usage`] (exit 2) with `--amount is required`
    //!    (the request-builder gate, distinct from claim). Nothing persisted.
    //!
    //! 7. **Compound rejects the `"max"` sentinel.** An explicit `--amount max` →
    //!    [`Code::Usage`] (exit 2) (`compound requires an explicit --amount in base
    //!    units (max is unsupported)` — the planner gate). Nothing persisted.
    //!
    //! 8. **Compound rejects a recipient that mismatches the sender.** A
    //!    `--recipient` that differs from the resolved sender → [`Code::Usage`]
    //!    (exit 2) (`compound requires --recipient to match --from-address`).
    //!    Nothing persisted.
    //!
    //! 9. **Compound provider gating.** `--provider morpho` → [`Code::Unsupported`]
    //!    (exit 13); a missing `--provider` → [`Code::Usage`] (exit 2). Nothing
    //!    persisted.
    //!
    //! 10. **Compound legacy-identity warning + backend.** `execution_backend ==
    //!     "legacy_local"` + the OWS-recommended warning in `env.warnings`.
    //!
    //! 11. **Compound requires at least one asset (through the handler).** An empty
    //!     `--assets` → [`Code::Usage`] (exit 2) with `--assets is required`.
    //!     Nothing persisted.
    //!
    //! SKIPPED (covered elsewhere): the planner's compound assembly + validation
    //!   internals (`defi-execution::planner` RED suite); the `supply`/`approve`
    //!   ABI encodings (`defi-evm::abi` goldens); the registry routing
    //!   (`defi-execution::builder` B6); the OWS `--wallet` happy path (WS4b);
    //!   `--input-json` precedence; clap flag defaults; `compound submit`/`status`
    //!   (WS4).

    use super::cli::{handle, CompoundPlanArgs, CompoundVerbCmd, RewardsCmd};
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags};
    use alloy::dyn_abi::{DynSolValue, JsonAbiExt};
    use alloy::json_abi::JsonAbi;
    use alloy::primitives::U256;
    use defi_config::Settings;
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    // --- contract constants ------------------------------------------------

    const SENDER: &str = "0x00000000000000000000000000000000000000aa";
    const OTHER: &str = "0x00000000000000000000000000000000000000bb";
    const ASSET_A: &str = "0x1111111111111111111111111111111111111111";
    const REWARD: &str = "0x3333333333333333333333333333333333333333";
    const CONTROLLER: &str = "0x4444444444444444444444444444444444444444";
    /// Aave pool (`--pool-address` override) — short-circuits the `getPool()` lookup.
    const POOL: &str = "0x00000000000000000000000000000000000000cc";
    const LEGACY_WARNING: &str =
        "--wallet (OWS) is recommended over --from-address for planning; see docs for details";

    // --- harness -----------------------------------------------------------

    fn exec_settings(dir: &Path) -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: false,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_secs(5),
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

    /// A `rewards compound plan` `CompoundPlanArgs` with the canonical happy-path
    /// values; mutate per test. `--controller-address` + `--pool-address` are set
    /// so only the allowance `eth_call` hits the mock RPC.
    fn compound_args(rpc: &str) -> CompoundPlanArgs {
        CompoundPlanArgs {
            chain: Some("1".to_string()),
            assets: vec![ASSET_A.to_string()],
            reward_token: Some(REWARD.to_string()),
            amount: Some("1000000".to_string()),
            recipient: None,
            on_behalf_of: None,
            controller_address: Some(CONTROLLER.to_string()),
            pool_address: Some(POOL.to_string()),
            pool_address_provider: None,
            provider: Some("aave".to_string()),
            rpc_url: Some(rpc.to_string()),
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(SENDER.to_string()),
            },
            input: InputFlags::default(),
        }
    }

    async fn run_compound(dir: &Path, args: CompoundPlanArgs) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(&ctx, RewardsCmd::Compound(CompoundVerbCmd::Plan(args))).await
    }

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn action_data(env: &Envelope) -> Value {
        env.data.clone().expect("plan envelope carries `data`")
    }

    fn no_actions_persisted(dir: &Path) -> bool {
        let store = match ActionStore::open(dir.join("actions.db"), dir.join("actions.lock")) {
            Ok(store) => store,
            Err(_) => return true,
        };
        store
            .list("", 1000)
            .map(|actions| actions.is_empty())
            .unwrap_or(true)
    }

    fn step_types(data: &Value) -> Vec<String> {
        data["steps"]
            .as_array()
            .expect("steps array")
            .iter()
            .map(|s| s["type"].as_str().unwrap_or("").to_string())
            .collect()
    }

    fn step_of_type<'a>(data: &'a Value, kind: &str) -> &'a Value {
        data["steps"]
            .as_array()
            .expect("steps array")
            .iter()
            .find(|s| s["type"].as_str() == Some(kind))
            .unwrap_or_else(|| panic!("a {kind} step is present"))
    }

    // --- wiremock JSON-RPC: every eth_call returns a uint word -------------

    struct EchoIdResponder {
        result: String,
    }

    impl Respond for EchoIdResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let id = serde_json::from_slice::<Value>(&request.body)
                .ok()
                .and_then(|body| body.get("id").cloned())
                .unwrap_or_else(|| Value::from(1));
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": self.result,
            }))
        }
    }

    fn uint_word(v: u128) -> String {
        format!("0x{}", hex::encode(U256::from(v).to_be_bytes::<32>()))
    }

    /// A mock JSON-RPC endpoint answering every `eth_call` with a single
    /// ABI-encoded `uint256` word == `allowance` (the compound supply approval
    /// allowance check). `--controller-address` + `--pool-address` short-circuit
    /// the address-returning lookups, so every reaching eth_call is the allowance.
    async fn allowance_rpc(allowance: u128) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(EchoIdResponder {
                result: uint_word(allowance),
            })
            .mount(&server)
            .await;
        server
    }

    /// The expected `supply(asset, amount, onBehalfOf, referralCode=0)` calldata,
    /// computed from `defi_registry::AAVE_POOL_ABI`.
    fn supply_calldata(asset: &str, amount: u128, on_behalf_of: &str) -> String {
        let abi: JsonAbi =
            serde_json::from_str(defi_registry::AAVE_POOL_ABI).expect("parse pool abi");
        let f = abi
            .function("supply")
            .and_then(|o| o.first())
            .cloned()
            .expect("supply present");
        let data = f
            .abi_encode_input(&[
                DynSolValue::Address(asset.parse().expect("valid asset")),
                DynSolValue::Uint(U256::from(amount), 256),
                DynSolValue::Address(on_behalf_of.parse().expect("valid on-behalf")),
                DynSolValue::Uint(U256::ZERO, 16),
            ])
            .expect("encode supply");
        format!("0x{}", hex::encode(data))
    }

    // --- 1, 2, 10. compound happy path (insufficient allowance) ------------

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_plan_emits_success_envelope_and_three_step_shape() {
        let rpc = allowance_rpc(0).await; // insufficient -> approval needed.
        let tmp = TempDir::new().expect("tempdir");
        let env = run_compound(tmp.path(), compound_args(&rpc.uri()))
            .await
            .expect("aave rewards compound plan should succeed against the mock RPC");

        // Envelope contract (Go `emitSuccess`).
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "rewards compound plan");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);
        assert_eq!(env.meta.providers.len(), 1, "exactly one provider status");
        assert_eq!(env.meta.providers[0].name, "aave");
        assert_eq!(env.meta.providers[0].status, "ok");

        // Action `data` shape.
        let data = action_data(&env);
        let action_id = data["action_id"].as_str().expect("action_id string");
        assert!(
            action_id.strip_prefix("act_").is_some_and(|rest| rest.len() == 32
                && rest.bytes().all(|b| b.is_ascii_hexdigit())),
            "action_id must match act_<32 hex>: got {action_id}"
        );
        assert_eq!(data["intent_type"], Value::from("compound_rewards"));
        assert_eq!(data["provider"], Value::from("aave"));
        assert_eq!(data["status"], Value::from("planned"));
        assert_eq!(data["chain_id"], Value::from("eip155:1"));
        assert_eq!(data["input_amount"], Value::from("1000000"));

        // Insufficient allowance -> [claim, approval, lend_call] in order.
        assert_eq!(
            step_types(&data),
            vec![
                "claim".to_string(),
                "approval".to_string(),
                "lend_call".to_string()
            ],
            "compound (insufficient allowance) => claim, approval, supply"
        );
        // claim targets the controller.
        assert_eq!(
            step_of_type(&data, "claim")["target"]
                .as_str()
                .unwrap()
                .to_lowercase(),
            CONTROLLER.to_lowercase()
        );
        // approval targets the reward token.
        assert_eq!(
            step_of_type(&data, "approval")["target"]
                .as_str()
                .unwrap()
                .to_lowercase(),
            REWARD.to_lowercase()
        );
        // supply (lend_call) targets the pool.
        let supply = step_of_type(&data, "lend_call");
        assert_eq!(supply["value"], Value::from("0"));
        assert_eq!(supply["chain_id"], Value::from("eip155:1"));
        assert_eq!(
            supply["target"].as_str().unwrap().to_lowercase(),
            POOL.to_lowercase()
        );

        // metadata carries the compound + pool + on_behalf_of context.
        let meta = data["metadata"].as_object().expect("metadata object");
        assert_eq!(meta.get("compound"), Some(&Value::Bool(true)));
        assert_eq!(
            meta.get("pool").map(|v| v.as_str().unwrap().to_lowercase()),
            Some(POOL.to_lowercase())
        );
        assert_eq!(
            meta.get("on_behalf_of")
                .map(|v| v.as_str().unwrap().to_lowercase()),
            Some(SENDER.to_lowercase()),
            "on_behalf_of defaults to the sender"
        );

        // Legacy backend stamping + warning (criterion 10).
        assert_eq!(data["execution_backend"], Value::from("legacy_local"));
        assert!(
            env.warnings.iter().any(|w| w == LEGACY_WARNING),
            "legacy plan surfaces the OWS-recommended warning; got {:?}",
            env.warnings
        );
    }

    // --- 3. compound skips approval when allowance sufficient --------------

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_plan_skips_approval_when_allowance_sufficient() {
        let rpc = allowance_rpc(10_000_000).await; // >= requested.
        let tmp = TempDir::new().expect("tempdir");
        let env = run_compound(tmp.path(), compound_args(&rpc.uri()))
            .await
            .expect("aave rewards compound plan should succeed");
        let data = action_data(&env);
        assert_eq!(
            step_types(&data),
            vec!["claim".to_string(), "lend_call".to_string()],
            "sufficient allowance => claim then supply (no approval)"
        );
    }

    // --- 4. compound supply step calldata reuses the Aave pool ABI golden ---

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_plan_supply_calldata_matches_aave_pool_golden() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let env = run_compound(tmp.path(), compound_args(&rpc.uri()))
            .await
            .expect("aave rewards compound plan should succeed");
        let data = action_data(&env);
        let supply = step_of_type(&data, "lend_call");
        // on_behalf_of defaults to the sender; supplies the reward token.
        assert_eq!(
            supply["data"].as_str().unwrap().to_lowercase(),
            supply_calldata(REWARD, 1_000_000, SENDER).to_lowercase(),
            "compound supply calldata must equal the alloy AAVE_POOL_ABI golden"
        );
    }

    // --- 5. compound persists the action to the Store ----------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_plan_persists_action_to_store() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let settings = exec_settings(tmp.path());
        let ctx = AppCtx::new(settings.clone());
        let env = handle(
            &ctx,
            RewardsCmd::Compound(CompoundVerbCmd::Plan(compound_args(&rpc.uri()))),
        )
        .await
        .expect("aave rewards compound plan should succeed");
        let action_id = action_data(&env)["action_id"]
            .as_str()
            .expect("action_id")
            .to_string();

        let store = ActionStore::open(&settings.action_store_path, &settings.action_lock_path)
            .expect("reopen action store");
        let persisted = store
            .get(&action_id)
            .expect("planned action retrievable by id");
        assert_eq!(persisted.intent_type, "compound_rewards");
        assert_eq!(persisted.input_amount, "1000000");
        assert_eq!(persisted.provider, "aave");
    }

    // --- 6. compound requires a non-empty amount (no "max" default) --------

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_plan_rejects_empty_amount() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = compound_args(&rpc.uri());
        args.amount = None;
        let err = run_compound(tmp.path(), args)
            .await
            .expect_err("empty compound --amount must be rejected (no max default)");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("--amount is required"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 7. compound rejects the "max" sentinel ----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_plan_rejects_max_amount() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = compound_args(&rpc.uri());
        args.amount = Some("max".to_string());
        let err = run_compound(tmp.path(), args)
            .await
            .expect_err("compound rejects the max sentinel");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(err.to_string().contains("max is unsupported"), "got: {err}");
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 8. compound rejects a recipient mismatch --------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_plan_rejects_recipient_mismatch() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = compound_args(&rpc.uri());
        args.recipient = Some(OTHER.to_string()); // differs from the sender.
        let err = run_compound(tmp.path(), args)
            .await
            .expect_err("compound requires recipient == sender");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("compound requires --recipient to match --from-address"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 9. compound provider gating ---------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_plan_rejects_non_aave_provider() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = compound_args(&rpc.uri());
        args.provider = Some("morpho".to_string());
        let err = run_compound(tmp.path(), args)
            .await
            .expect_err("compound plan rejects non-aave providers");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .contains("rewards execution currently supports only provider=aave"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_plan_rejects_missing_provider() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = compound_args(&rpc.uri());
        args.provider = None;
        let err = run_compound(tmp.path(), args)
            .await
            .expect_err("compound plan requires a provider");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- structured input (`--input-json` / `--input-file`) ----------------
    //
    // Go: `configureStructuredInput[compoundArgs]` wires the PreRunE merge onto
    // `rewards compound plan`. An unknown key is a usage error keyed on the full
    // command path; persists nothing.

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_plan_input_json_unknown_field_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        let args = CompoundPlanArgs {
            input: InputFlags {
                input_json: Some(r#"{"provider":"aave","bogus":"x"}"#.to_string()),
                input_file: None,
            },
            ..CompoundPlanArgs::default()
        };
        let err = run_compound(tmp.path(), args)
            .await
            .expect_err("unknown structured-input field must be a usage error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert_eq!(
            err.message,
            "structured input field \"bogus\" is not supported by rewards compound plan"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_plan_resolves_all_flags_from_input_json() {
        let rpc = allowance_rpc(0).await; // insufficient -> approval needed.
        let tmp = TempDir::new().expect("tempdir");
        let args = CompoundPlanArgs {
            input: InputFlags {
                input_json: Some(format!(
                    r#"{{"provider":"aave","chain":"1","assets":["{ASSET_A}"],"reward_token":"{REWARD}","amount":"1000000","from_address":"{SENDER}","controller_address":"{CONTROLLER}","pool_address":"{POOL}","rpc_url":"{rpc}"}}"#,
                    rpc = rpc.uri()
                )),
                input_file: None,
            },
            ..CompoundPlanArgs::default()
        };
        let env = run_compound(tmp.path(), args)
            .await
            .expect("input-json should fill all flags and the plan should succeed");
        assert!(env.success);
        assert_eq!(env.meta.command, "rewards compound plan");
        let data = action_data(&env);
        assert_eq!(data["intent_type"], Value::from("compound_rewards"));
        assert_eq!(data["provider"], Value::from("aave"));
    }

    // --- 11. compound requires at least one asset --------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_plan_rejects_empty_assets() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = compound_args(&rpc.uri());
        args.assets = Vec::new();
        let err = run_compound(tmp.path(), args)
            .await
            .expect_err("empty --assets must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("--assets is required"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }
}

#[cfg(test)]
mod claim_submit_app_tests {
    //! # Success criteria — `rewards claim submit` app-level handler (WS4,
    //! exec-submit)
    //!
    //! Go oracle: `internal/app/rewards_command.go` `submitCmd.RunE` inside
    //! `newRewardsClaimCommand` + `internal/app/execution_helpers.go`
    //! (`resolveActionExecutionBackend` / `validateExecutionSender` /
    //! `executeActionWithTimeout`) + `internal/app/runner.go`
    //! (`resolveActionID` / `newExecutionSigner` / `parseExecuteOptions`). These
    //! tests drive [`cli::handle`] (the real binary dispatch entry point) for
    //! `rewards claim submit` ONLY, asserting the full machine contract the Go
    //! runner emits via `emitSuccess(...)` / `renderError(...)`.
    //!
    //! ## Determinism / offline strategy (no live chains)
    //!
    //! The reused [`defi_execution`] engine
    //! ([`defi_execution::evm_executor::execute_action`]) is the contract source
    //! of truth, and these tests reuse it exactly as its own suite does:
    //!
    //! * **Pre-broadcast guards** (action-id, store load, intent gate,
    //!   already-completed short-circuit, backend selection, sender match,
    //!   execute-option validation) all fire BEFORE any network and are fully
    //!   deterministic.
    //! * **Local-signer broadcast/completion** is exercised OFFLINE through the
    //!   `--private-key` override (the deterministic in-args secp256k1 key whose
    //!   address is pinned in `defi-evm`): in this build the policed EVM step path
    //!   runs the pre-sign policy then marks the step `confirmed` and the action
    //!   `completed` WITHOUT a network call (matching the engine's own
    //!   `execute_action` tests). The full RPC-backed sign+broadcast
    //!   (chain-id/gas/nonce/`sendRawTransaction`/receipt) is `wiremock`-RPC
    //!   integration territory (WS5) and is recorded as a deferral — NOT asserted
    //!   here.
    //! * **The single `claim` step is NOT an `approval`/`bridge` step**, so the
    //!   bounded-approval pre-sign guardrail and the bridge canonical-target
    //!   guardrail do NOT apply to `rewards claim` (they are owned by
    //!   `approvals`/`bridge` submit + the `defi-execution::policy` /
    //!   `verify_bridge_settlement` suites and are intentionally NOT re-asserted
    //!   here). A claim submit therefore completes offline WITHOUT
    //!   `--allow-max-approval`.
    //! * **OWS `--wallet` backend** resolves through the OWS vault/CLI (WS4b e2e),
    //!   so only its OFFLINE guard rejections are asserted (missing persisted
    //!   `wallet_id`; legacy signer flags on a wallet-backed action). The OWS
    //!   happy-path broadcast is a WS4b deferral.
    //! * **Bridge destination-settlement waits do NOT apply to `rewards`**: a
    //!   `claim_rewards` action never carries a `bridge_send` step, so no
    //!   settlement poll is reachable. (That transition is owned by `bridge
    //!   submit/status` + `defi-execution::verify_bridge_settlement` and is NOT
    //!   re-asserted here.)
    //!
    //! Each criterion below is a FAILING test until `cli::handle` routes
    //! `Claim(Submit)` to a real handler (today it returns the
    //! `AppCtx::unimplemented` stub).
    //!
    //! Criteria:
    //!
    //! 1. **Submit success envelope (legacy local key) + completion.** Given a
    //!    persisted `claim_rewards` action whose `from_address` matches the
    //!    deterministic `--private-key` signer, `rewards claim submit` returns
    //!    `Ok(Envelope)` (exit 0) with: `version == "v1"`, `success == true`,
    //!    `error == None`, `meta.partial == false`, `meta.command == "rewards
    //!    claim submit"`, and `meta.cache == {status:"bypass", age_ms:0,
    //!    stale:false}` (execution paths bypass the cache, spec §2.5). The
    //!    serialized `data` Action has `status == "completed"` and its single
    //!    `claim` step has `status == "confirmed"`. (Go `emitSuccess(..., action,
    //!    nil, cacheMetaBypass(), nil, false)` after `executeActionWithTimeout`.)
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
    //! 5. **Intent gate.** Submitting a persisted NON-`claim_rewards` action
    //!    (e.g. a `compound_rewards` action) through `rewards claim submit` →
    //!    [`Code::Usage`] (exit 2) with `action is not a rewards claim intent`.
    //!    (Go `submitCmd` IntentType guard; mirrors
    //!    [`super::ensure_rewards_claim_intent`].)
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
    //!    (`legacy actions only support --signer local`). (Go
    //!    `resolveActionExecutionBackend` legacy branch.)
    //!
    //! 8. **OWS action missing persisted wallet_id.** A wallet-backed
    //!    (`execution_backend == "ows"`) action with an empty `wallet_id` →
    //!    [`Code::Usage`] (exit 2) (`wallet-backed action is missing persisted
    //!    wallet_id`). (Go OWS branch guard — reachable OFFLINE because the guard
    //!    precedes any OWS resolve.)
    //!
    //! 9. **OWS action rejects legacy signer flags.** A wallet-backed action with
    //!    a persisted `wallet_id` submitted with an explicit legacy signer flag
    //!    (`--private-key`) → [`Code::Usage`] (exit 2) (`wallet-backed actions do
    //!    not accept legacy signer flags`). (Go `usesLegacySignerFlags` guard.)
    //!
    //! 10. **Sender mismatch (`--from-address`).** A `legacy_local` action whose
    //!     persisted `from_address` matches the signer, submitted with
    //!     `--from-address` == a DIFFERENT addr → [`Code::Signer`] (exit 24). (Go
    //!     `validateExecutionSender`: `signer address does not match
    //!     --from-address`.)
    //!
    //! 11. **Sender mismatch (planned action sender vs signer).** A `legacy_local`
    //!     action whose persisted `from_address` does NOT match the
    //!     `--private-key` signer (and no `--from-address`) → [`Code::Signer`]
    //!     (exit 24). (Go `validateExecutionSender` /
    //!     `validate_persisted_action_sender`.)
    //!
    //! 12. **Execute-option validation.** `--gas-multiplier 1.0` → [`Code::Usage`]
    //!     (exit 2) (`--gas-multiplier must be > 1`); `--poll-interval "0s"` →
    //!     [`Code::Usage`] (exit 2); `--step-timeout "nope"` → [`Code::Usage`]
    //!     (exit 2). (Go `parseExecuteOptions`.)
    //!
    //! 13. **Signer init failure (no key).** A `legacy_local` action submitted
    //!     with `--signer local` and NO resolvable key (`--key-source env` with
    //!     the env unset, no `--private-key`) → [`Code::Signer`] (exit 24). (Go
    //!     `newExecutionSigner` → `initialize local signer`.)
    //!
    //! 14. **Error paths do not mutate terminal status.** On every rejected submit
    //!     (criteria 3–13, error cases) the persisted action — when one exists —
    //!     remains in its pre-submit `status == "planned"` (the handler returns
    //!     the typed `Err(Error)`; the runner renders the full error envelope to
    //!     stderr, spec §2.1).
    //!
    //! SKIPPED (covered elsewhere / wrong unit / deferred):
    //!   * the full RPC-backed sign+broadcast — WS5 `wiremock`-RPC integration
    //!     deferral;
    //!   * the OWS happy-path resolve + send-hook broadcast — WS4b e2e deferral;
    //!   * Tempo (type 0x76) submit — Tempo is a separate execution path
    //!     (`--signer tempo`), byte-parity is WS4a, and `rewards` planning is
    //!     OWS-first standard-EVM (no Tempo identity branch);
    //!   * bridge destination-settlement waits — `bridge submit/status` unit +
    //!     `defi-execution::verify_bridge_settlement` (not reachable for
    //!     `rewards`);
    //!   * the bounded-approval ABI decode internals — `defi-execution::policy`
    //!     RED suite (and not reachable from a single `claim` step);
    //!   * the EIP-1559 signing byte layout — `defi-evm` signer goldens;
    //!   * `--input-json`/`--input-file` precedence on submit — structured-input
    //!     unit (the plan-side merge is already covered in `app_tests`);
    //!   * clap/cobra flag defaults + schema auth metadata — schema/CLI suites.

    use super::cli::{handle, ClaimPlanArgs, ClaimVerbCmd, RewardsCmd};
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags, SubmitArgs};
    use defi_config::Settings;
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::action::{Action, ActionStatus, ExecutionBackend};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;
    use wiremock::MockServer;

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
    /// An Aave incentives "asset" (aToken/debtToken source) for planned claims.
    const ASSET_A: &str = "0x1111111111111111111111111111111111111111";
    /// The reward token claimed from the incentives controller.
    const REWARD: &str = "0x3333333333333333333333333333333333333333";
    /// The incentives controller (`--controller-address` override) —
    /// short-circuits the on-chain `getAddress(INCENTIVES_CONTROLLER)` lookup.
    const CONTROLLER: &str = "0x4444444444444444444444444444444444444444";

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
    /// zero values would NOT match the parsed defaults, so they are stamped
    /// here): `signer=local`, `key_source=auto`, `gas_multiplier=1.2`,
    /// `poll_interval=2s`, `step_timeout=2m`, `simulate=true`, plus the
    /// deterministic `--private-key`. Callers mutate per test.
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

    /// A non-dialed RPC sentinel for the planned step (the policed EVM step path
    /// does not reach the network in this build; this keeps the action
    /// well-formed). The controller override avoids any plan-time eth_call.
    const DEAD_RPC: &str = "http://127.0.0.1:0";

    /// Plan + persist a canonical `claim_rewards` action against `dir`, returning
    /// its `action_id`. `from_addr` becomes the action's `from_address`. Plans
    /// through the real `cli::handle` plan path so the persisted shape is
    /// identical to production. `--controller-address` is set so no plan-time
    /// eth_call is needed (a parseable wiremock URI is still required by connect).
    async fn plan_claim(dir: &Path, from_addr: &str) -> String {
        // A wiremock server only to provide a parseable, connectable URI for the
        // plan path (no eth_call is made with the controller override).
        let rpc = MockServer::start().await;
        let ctx = AppCtx::new(exec_settings(dir));
        let args = ClaimPlanArgs {
            chain: Some("1".to_string()),
            assets: vec![ASSET_A.to_string()],
            reward_token: Some(REWARD.to_string()),
            amount: Some("1000000".to_string()),
            recipient: None,
            controller_address: Some(CONTROLLER.to_string()),
            pool_address_provider: None,
            provider: Some("aave".to_string()),
            rpc_url: Some(rpc.uri()),
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(from_addr.to_string()),
            },
            input: InputFlags::default(),
        };
        let env = handle(&ctx, RewardsCmd::Claim(ClaimVerbCmd::Plan(args)))
            .await
            .expect("plan a claim_rewards action for the submit fixture");
        let action_id = env.data.expect("plan data")["action_id"]
            .as_str()
            .expect("action_id")
            .to_string();
        // Re-point the persisted step rpc_url at a non-dialed sentinel so the
        // offline policed-EVM submit path is robust to the wiremock server
        // shutting down.
        let store = ActionStore::open(dir.join("actions.db"), dir.join("actions.lock"))
            .expect("open store");
        let mut action = store.get(&action_id).expect("load");
        for step in &mut action.steps {
            step.rpc_url = DEAD_RPC.to_string();
        }
        store.save(&action).expect("persist sentinel rpc_url");
        action_id
    }

    /// Persist `action` directly (used for fixtures the plan path cannot build,
    /// e.g. a `compound_rewards`-intent or an OWS-backed action).
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
        handle(&ctx, RewardsCmd::Claim(ClaimVerbCmd::Submit(args))).await
    }

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn signer_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn data_of(env: &Envelope) -> Value {
        env.data.clone().expect("submit envelope carries `data`")
    }

    // --- 1, 2. submit success + completion + persistence -------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_legacy_local_completes_and_emits_envelope() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_claim(tmp.path(), SIGNER_ADDR).await;

        // No --allow-max-approval needed: the single `claim` step is not an
        // approval step, so the bounded-approval guardrail does not apply.
        let env = run_submit(tmp.path(), base_submit_args(&action_id))
            .await
            .expect("legacy-local claim submit should complete offline");

        // Envelope contract.
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "rewards claim submit");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        // Completed action in data, single confirmed claim step.
        let data = data_of(&env);
        assert_eq!(data["status"], Value::from("completed"));
        let steps = data["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1, "claim is a single-step action");
        assert_eq!(steps[0]["type"], Value::from("claim"));
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
        let args = base_submit_args("act_0123456789abcdef0123456789abcdef");
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("unknown action must surface a load (usage) error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- 5. intent gate ----------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_non_claim_intent() {
        let tmp = TempDir::new().expect("tempdir");
        // A persisted COMPOUND-intent action submitted through claim submit.
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "compound_rewards",
            "eip155:1",
            Default::default(),
        );
        action.from_address = SIGNER_ADDR.to_string();
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        save_action(tmp.path(), &action);

        let args = base_submit_args(&action.action_id);
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("non-claim intent rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("action is not a rewards claim intent"),
            "got: {err}"
        );
        assert_eq!(persisted_status(tmp.path(), &action.action_id), "planned");
    }

    // --- 6. already-completed short-circuit --------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_already_completed_short_circuits_with_warning() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_claim(tmp.path(), SIGNER_ADDR).await;
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
        assert_eq!(env.meta.command, "rewards claim submit");
        assert!(
            env.warnings.iter().any(|w| w == "action already completed"),
            "expected `action already completed` warning, got {:?}",
            env.warnings
        );
        assert_eq!(data_of(&env)["status"], Value::from("completed"));
    }

    // --- 7. legacy backend rejects a non-local signer ----------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_legacy_action_rejects_tempo_signer() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_claim(tmp.path(), SIGNER_ADDR).await;
        let mut args = base_submit_args(&action_id);
        args.signer = "tempo".to_string();
        args.private_key = None;
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
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "claim_rewards",
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
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "claim_rewards",
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
        let action_id = plan_claim(tmp.path(), SIGNER_ADDR).await;
        let mut args = base_submit_args(&action_id);
        args.from_address = Some(OTHER_ADDR.to_string());
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("--from-address mismatch rejected");
        assert_eq!(err.code, Code::Signer);
        // Signer maps to exit 24 (spec §2.2).
        assert_eq!(signer_exit(&err), 24);
        assert_eq!(persisted_status(tmp.path(), &action_id), "planned");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_planned_sender_signer_mismatch() {
        let tmp = TempDir::new().expect("tempdir");
        // Planned action sender is OTHER_ADDR but the local signer is SIGNER_ADDR;
        // no --from-address supplied.
        let action_id = plan_claim(tmp.path(), OTHER_ADDR).await;
        let args = base_submit_args(&action_id);
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("planned-sender/signer mismatch rejected");
        assert_eq!(err.code, Code::Signer);
        assert_eq!(signer_exit(&err), 24);
        assert_eq!(persisted_status(tmp.path(), &action_id), "planned");
    }

    // --- 12. execute-option validation -------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_gas_multiplier_not_greater_than_one() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_claim(tmp.path(), SIGNER_ADDR).await;
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
        let action_id = plan_claim(tmp.path(), SIGNER_ADDR).await;
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
        let action_id = plan_claim(tmp.path(), SIGNER_ADDR).await;
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
        let action_id = plan_claim(tmp.path(), SIGNER_ADDR).await;
        let mut args = base_submit_args(&action_id);
        // Force an unresolvable key: source=env (isolates the env hex var) with no
        // --private-key override. DEFI_PRIVATE_KEY is not set in this test.
        args.private_key = None;
        args.key_source = "env".to_string();
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("signer init with no key must fail");
        assert_eq!(err.code, Code::Signer);
        assert_eq!(signer_exit(&err), 24);
        assert_eq!(persisted_status(tmp.path(), &action_id), "planned");
    }
}

#[cfg(test)]
mod compound_submit_app_tests {
    //! # Success criteria — `rewards compound submit` app-level handler (WS4,
    //! exec-submit)
    //!
    //! Go oracle: `internal/app/rewards_command.go` `submitCmd.RunE` inside
    //! `newRewardsCompoundCommand`. These tests drive [`cli::handle`] for
    //! `rewards compound submit` ONLY. Compound is the only MULTI-step rewards
    //! action: `[claim, approval, lend_call]` (the `approval` step is dropped when
    //! the on-chain allowance already covers the supply). Unlike `claim`, the
    //! `approval` step IS subject to the bounded-approval pre-sign guardrail, so
    //! the inflated-approval rejection + `--allow-max-approval` opt-in ARE
    //! asserted here.
    //!
    //! Same offline determinism strategy as
    //! [`super::claim_submit_app_tests`]: the policed EVM step path runs the
    //! pre-sign policy then marks each step `confirmed` and the action `completed`
    //! WITHOUT a network call. The full RPC-backed broadcast is a WS5 deferral;
    //! the OWS happy path is a WS4b deferral; bridge settlement waits do NOT apply
    //! (a `compound_rewards` action carries no `bridge_send` step).
    //!
    //! Criteria (each FAILING until `cli::handle` routes `Compound(Submit)` to a
    //! real handler — today the stub returns `AppCtx::unimplemented`):
    //!
    //! 1. **Submit success envelope (legacy local key) + completion.** A persisted
    //!    `compound_rewards` action (allowance sufficient → `[claim, lend_call]`)
    //!    whose `from_address` matches the deterministic signer returns
    //!    `Ok(Envelope)` (exit 0) with `meta.command == "rewards compound
    //!    submit"`, `meta.cache == {status:"bypass", age_ms:0, stale:false}`,
    //!    `data.status == "completed"`, and EVERY step `status == "confirmed"`.
    //!
    //! 2. **Submit persists the terminal state.** The re-loaded action has
    //!    `status == "completed"`.
    //!
    //! 3. **Bounded-approval guardrail (pre-sign).** A persisted compound whose
    //!    `approval` step calldata approves MORE than the planned `input_amount`,
    //!    submitted WITHOUT `--allow-max-approval`, → [`Code::ActionPlan`]
    //!    (exit 20) with an error mentioning `allow-max-approval`. The same action
    //!    with `--allow-max-approval` is accepted (exit 0, completed). (AGENTS.md
    //!    bounded-approval pre-sign check; `defi_execution::policy`
    //!    `validate_approval_policy`.)
    //!
    //! 4. **Intent gate.** Submitting a persisted NON-`compound_rewards` action
    //!    (e.g. a `claim_rewards` action) through `rewards compound submit` →
    //!    [`Code::Usage`] (exit 2) with `action is not a rewards compound intent`.
    //!    (Mirrors [`super::ensure_rewards_compound_intent`].)
    //!
    //! 5. **Action-id validation + unknown-action load failure.** `--action-id ""`
    //!    / a malformed id / a well-formed unknown id → [`Code::Usage`] (exit 2).
    //!
    //! 6. **Already-completed short-circuit.** An action already `completed`
    //!    returns success WITHOUT re-broadcast + the `action already completed`
    //!    warning.
    //!
    //! 7. **Backend / sender / option guards** (parity with claim submit, asserted
    //!    on the compound path): legacy `--signer tempo` rejection;
    //!    `--from-address` mismatch → [`Code::Signer`] (exit 24); `--gas-multiplier
    //!    1.0` → [`Code::Usage`] (exit 2).
    //!
    //! 8. **Error paths do not mutate terminal status.** Every rejected submit
    //!    leaves a persisted action in `status == "planned"`.
    //!
    //! SKIPPED: identical deferrals to [`super::claim_submit_app_tests`] (full
    //!   RPC broadcast WS5; OWS happy path WS4b; Tempo WS4a; bridge settlement;
    //!   EIP-1559 byte layout; structured-input precedence; flag defaults). The
    //!   OWS/wallet offline guards are already asserted on the claim path (the
    //!   `resolve_action_execution_backend` helper is group-independent) and are
    //!   not duplicated here.

    use super::cli::{handle, CompoundPlanArgs, CompoundVerbCmd, RewardsCmd};
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags, SubmitArgs};
    use defi_config::Settings;
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::action::{Action, ActionStatus, ExecutionBackend};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    // --- contract constants ------------------------------------------------

    const TEST_KEY: &str = "59c6995e998f97a5a0044976f0945388cf9b7e5e5f4f9d2d9d8f1f5b7f6d11d1";
    const SIGNER_ADDR: &str = "0x14DDBd1fe5026E58A12eE8691cAEbFD24bb10eef";
    const OTHER_ADDR: &str = "0x2222222222222222222222222222222222222222";
    const ASSET_A: &str = "0x1111111111111111111111111111111111111111";
    const REWARD: &str = "0x3333333333333333333333333333333333333333";
    const CONTROLLER: &str = "0x4444444444444444444444444444444444444444";
    /// Aave pool (`--pool-address` override) — short-circuits the `getPool()`
    /// lookup, so the only plan-time eth_call is the allowance check.
    const POOL: &str = "0x00000000000000000000000000000000000000cc";
    const DEAD_RPC: &str = "http://127.0.0.1:0";

    // --- harness -----------------------------------------------------------

    fn exec_settings(dir: &Path) -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: false,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_secs(5),
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

    // --- wiremock JSON-RPC: every eth_call returns a fixed uint word --------

    struct EchoIdResponder {
        result: String,
    }

    impl Respond for EchoIdResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let id = serde_json::from_slice::<Value>(&request.body)
                .ok()
                .and_then(|body| body.get("id").cloned())
                .unwrap_or_else(|| Value::from(1));
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": self.result,
            }))
        }
    }

    fn uint_word(v: u128) -> String {
        use alloy::primitives::U256;
        format!("0x{}", hex::encode(U256::from(v).to_be_bytes::<32>()))
    }

    /// A mock JSON-RPC endpoint answering every `eth_call` with `allowance` (the
    /// compound supply approval check). `--controller-address` + `--pool-address`
    /// short-circuit the address-returning lookups.
    async fn allowance_rpc(allowance: u128) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(EchoIdResponder {
                result: uint_word(allowance),
            })
            .mount(&server)
            .await;
        server
    }

    /// Plan + persist a canonical `compound_rewards` action against `dir`,
    /// returning its `action_id`. `allowance` controls whether the persisted plan
    /// carries an `approval` step (insufficient → yes). After planning, the
    /// persisted step `rpc_url`s are re-pointed at a non-dialed sentinel so the
    /// offline policed-EVM submit path is robust to the wiremock shutdown.
    async fn plan_compound(dir: &Path, from_addr: &str, allowance: u128) -> String {
        let rpc = allowance_rpc(allowance).await;
        let ctx = AppCtx::new(exec_settings(dir));
        let args = CompoundPlanArgs {
            chain: Some("1".to_string()),
            assets: vec![ASSET_A.to_string()],
            reward_token: Some(REWARD.to_string()),
            amount: Some("1000000".to_string()),
            recipient: None,
            on_behalf_of: None,
            controller_address: Some(CONTROLLER.to_string()),
            pool_address: Some(POOL.to_string()),
            pool_address_provider: None,
            provider: Some("aave".to_string()),
            rpc_url: Some(rpc.uri()),
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(from_addr.to_string()),
            },
            input: InputFlags::default(),
        };
        let env = handle(&ctx, RewardsCmd::Compound(CompoundVerbCmd::Plan(args)))
            .await
            .expect("plan a compound_rewards action for the submit fixture");
        let action_id = env.data.expect("plan data")["action_id"]
            .as_str()
            .expect("action_id")
            .to_string();
        let store = ActionStore::open(dir.join("actions.db"), dir.join("actions.lock"))
            .expect("open store");
        let mut action = store.get(&action_id).expect("load");
        for step in &mut action.steps {
            step.rpc_url = DEAD_RPC.to_string();
        }
        store.save(&action).expect("persist sentinel rpc_url");
        action_id
    }

    fn save_action(dir: &Path, action: &Action) {
        let store = ActionStore::open(dir.join("actions.db"), dir.join("actions.lock"))
            .expect("open action store");
        store.save(action).expect("persist fixture action");
    }

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
        handle(&ctx, RewardsCmd::Compound(CompoundVerbCmd::Submit(args))).await
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
        // Sufficient allowance => [claim, lend_call] (no approval step), so no
        // bounded-approval opt-in is needed for the happy path.
        let action_id = plan_compound(tmp.path(), SIGNER_ADDR, 10_000_000).await;

        let env = run_submit(tmp.path(), base_submit_args(&action_id))
            .await
            .expect("legacy-local compound submit should complete offline");

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "rewards compound submit");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        let data = data_of(&env);
        assert_eq!(data["status"], Value::from("completed"));
        let steps = data["steps"].as_array().expect("steps array");
        assert!(!steps.is_empty(), "compound has at least claim + supply");
        for step in steps {
            assert_eq!(
                step["status"],
                Value::from("confirmed"),
                "every compound step confirmed offline"
            );
        }
        assert_eq!(persisted_status(tmp.path(), &action_id), "completed");
    }

    // --- 3. bounded-approval pre-sign guardrail ----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_inflated_approval_without_allow_max() {
        let tmp = TempDir::new().expect("tempdir");
        // Insufficient allowance => the plan carries an `approval` step.
        let action_id = plan_compound(tmp.path(), SIGNER_ADDR, 0).await;
        // Inflate the persisted approval step's amount ABOVE the planned
        // input_amount (max uint256), simulating an over-approval the bounded
        // check must reject without --allow-max-approval.
        {
            let store = ActionStore::open(
                tmp.path().join("actions.db"),
                tmp.path().join("actions.lock"),
            )
            .expect("open store");
            let mut action = store.get(&action_id).expect("load");
            let approval = action
                .steps
                .iter_mut()
                .find(|s| {
                    serde_json::to_value(s.step_type)
                        .ok()
                        .and_then(|v| v.as_str().map(|x| x.to_string()))
                        .as_deref()
                        == Some("approval")
                })
                .expect("plan carries an approval step with insufficient allowance");
            // approve(reward, 0xffff...ffff) — max uint256, > input_amount.
            approval.data = format!(
                "0x095ea7b3000000000000000000000000{}{}",
                REWARD.trim_start_matches("0x").to_lowercase(),
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
        let action_id = plan_compound(tmp.path(), SIGNER_ADDR, 0).await;
        {
            let store = ActionStore::open(
                tmp.path().join("actions.db"),
                tmp.path().join("actions.lock"),
            )
            .expect("open store");
            let mut action = store.get(&action_id).expect("load");
            let approval = action
                .steps
                .iter_mut()
                .find(|s| {
                    serde_json::to_value(s.step_type)
                        .ok()
                        .and_then(|v| v.as_str().map(|x| x.to_string()))
                        .as_deref()
                        == Some("approval")
                })
                .expect("plan carries an approval step");
            approval.data = format!(
                "0x095ea7b3000000000000000000000000{}{}",
                REWARD.trim_start_matches("0x").to_lowercase(),
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

    // --- 4. intent gate ----------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_non_compound_intent() {
        let tmp = TempDir::new().expect("tempdir");
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "claim_rewards",
            "eip155:1",
            Default::default(),
        );
        action.from_address = SIGNER_ADDR.to_string();
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        save_action(tmp.path(), &action);

        let args = base_submit_args(&action.action_id);
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("non-compound intent rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("action is not a rewards compound intent"),
            "got: {err}"
        );
        assert_eq!(persisted_status(tmp.path(), &action.action_id), "planned");
    }

    // --- 5. action-id validation + unknown-action load failure -------------

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
        let args = base_submit_args("act_nope");
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("malformed action id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_unknown_action_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        let args = base_submit_args("act_0123456789abcdef0123456789abcdef");
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("unknown action must surface a load (usage) error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- 6. already-completed short-circuit --------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_already_completed_short_circuits_with_warning() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_compound(tmp.path(), SIGNER_ADDR, 10_000_000).await;
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
        assert_eq!(env.meta.command, "rewards compound submit");
        assert!(
            env.warnings.iter().any(|w| w == "action already completed"),
            "expected `action already completed` warning, got {:?}",
            env.warnings
        );
        assert_eq!(data_of(&env)["status"], Value::from("completed"));
    }

    // --- 7. backend / sender / option guards (compound path) ---------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_legacy_action_rejects_tempo_signer() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_compound(tmp.path(), SIGNER_ADDR, 10_000_000).await;
        let mut args = base_submit_args(&action_id);
        args.signer = "tempo".to_string();
        args.private_key = None;
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

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_from_address_mismatch() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_compound(tmp.path(), SIGNER_ADDR, 10_000_000).await;
        let mut args = base_submit_args(&action_id);
        args.from_address = Some(OTHER_ADDR.to_string());
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("--from-address mismatch rejected");
        assert_eq!(err.code, Code::Signer);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 24);
        assert_eq!(persisted_status(tmp.path(), &action_id), "planned");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_gas_multiplier_not_greater_than_one() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_compound(tmp.path(), SIGNER_ADDR, 10_000_000).await;
        let mut args = base_submit_args(&action_id);
        args.gas_multiplier = 1.0;
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("gas-multiplier <= 1 rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(err.to_string().contains("gas-multiplier"), "got: {err}");
    }
}

#[cfg(test)]
mod status_app_tests {
    //! # Success criteria — `rewards {claim,compound} status` app-level handlers
    //! (WS4, exec-status)
    //!
    //! Go oracle: `internal/app/rewards_command.go` `statusCmd.RunE` inside
    //! `newRewardsClaimCommand` / `newRewardsCompoundCommand`. These tests drive
    //! [`cli::handle`] for `rewards claim status` and `rewards compound status`.
    //! Both are pure READS over the persisted action store (no signing, no
    //! network), so they are fully offline + deterministic. (Bridge
    //! destination-settlement polling — the only network-backed status transition
    //! — does NOT apply to `rewards`: claim/compound actions never carry a
    //! `bridge_send` step. That wait is owned by `bridge status` +
    //! `defi-execution::verify_bridge_settlement` and is NOT re-asserted here.)
    //!
    //! Criteria (each FAILING until `cli::handle` implements rewards status):
    //!
    //! 1. **Claim status success envelope reflects the persisted action.** Given a
    //!    persisted `claim_rewards` action in `status == "planned"`, `rewards claim
    //!    status --action-id <id>` returns `Ok(Envelope)` (exit 0) with `version
    //!    == "v1"`, `success == true`, `error == None`, `meta.command == "rewards
    //!    claim status"`, `meta.cache == {status:"bypass", age_ms:0, stale:false}`
    //!    (execution paths bypass the cache, spec §2.5), and `data` is the
    //!    serialized Action with `action_id` == the requested id, `intent_type ==
    //!    "claim_rewards"`, and `status == "planned"`.
    //!
    //! 2. **Claim status reflects lifecycle transitions.** After the persisted
    //!    action is advanced to `completed` / `running`, `rewards claim status`
    //!    returns `data.status == "completed"` / `"running"` verbatim (status is a
    //!    read of the persisted lifecycle, not a re-execution).
    //!
    //! 3. **Compound status success envelope.** Given a persisted
    //!    `compound_rewards` action, `rewards compound status` returns `Ok` with
    //!    `meta.command == "rewards compound status"`, `data.intent_type ==
    //!    "compound_rewards"`, and the persisted `status`.
    //!
    //! 4. **Action-id validation.** `--action-id ""` / a malformed id → for BOTH
    //!    claim and compound status → [`Code::Usage`] (exit 2). (Go
    //!    `resolveActionID`.)
    //!
    //! 5. **Load failure for an unknown action.** A well-formed but unknown
    //!    `--action-id` → [`Code::Usage`] (exit 2) (Go wraps the store `Get`
    //!    not-found as `clierr.Wrap(CodeUsage, "load action", err)`).
    //!
    //! 6. **Intent gate (cross-sibling).** `rewards claim status` on a persisted
    //!    `compound_rewards` action → [`Code::Usage`] (exit 2) with `action is not
    //!    a rewards claim intent`; `rewards compound status` on a `claim_rewards`
    //!    action → [`Code::Usage`] (exit 2) with `action is not a rewards compound
    //!    intent`. (Go `statusCmd` IntentType guards.)
    //!
    //! NON-APPLICABLE boundaries (documented, not tested here — by design):
    //!   * **Estimate fields** (EIP-1559 native gas for EVM / fee-token for Tempo)
    //!     are emitted by the `actions estimate` command, NOT by any `rewards`
    //!     handler. A `claim_rewards` / `compound_rewards` action is estimable as
    //!     ordinary native-gas (no Tempo branch — rewards is Aave-only standard
    //!     EVM), but that surface + arithmetic is owned by the `actions` unit and
    //!     `defi-execution::estimate` (its `single_step_estimate_arithmetic_parity`
    //!     and `estimate_json_preserves_declaration_order_and_omits_evm_fee_meta`
    //!     tests). It is intentionally NOT re-asserted through a `rewards` handler.
    //!   * **Bridge destination-settlement waits** are the only network-backed
    //!     status transition, and they do NOT apply to `rewards`: claim/compound
    //!     actions never carry a `bridge_send` step, so no settlement poll is
    //!     reachable. That wait is owned by `bridge submit/status` +
    //!     `defi-execution::verify_bridge_settlement`.
    //!
    //! SKIPPED (covered elsewhere / wrong unit):
    //!   * the action JSON shape internals — `defi-execution::action` golden;
    //!   * cache-bypass routing for rewards status — runner cache-flow concern,
    //!     asserted here only via `meta.cache.status`.

    use super::cli::{handle, ClaimPlanArgs, ClaimVerbCmd, CompoundVerbCmd, RewardsCmd};
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
    use wiremock::MockServer;

    const SENDER: &str = "0x00000000000000000000000000000000000000aa";
    const ASSET_A: &str = "0x1111111111111111111111111111111111111111";
    const REWARD: &str = "0x3333333333333333333333333333333333333333";
    const CONTROLLER: &str = "0x4444444444444444444444444444444444444444";

    fn exec_settings(dir: &Path) -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: false,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_secs(5),
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

    /// Plan + persist a canonical `claim_rewards` action, returning its
    /// `action_id`. The persisted step `rpc_url` is left pointing at `step_rpc`
    /// (used by the estimate test to dial the wiremock RPC); pass an unused
    /// wiremock URI for status-only tests.
    async fn plan_claim(dir: &Path, step_rpc: &str) -> String {
        let rpc = MockServer::start().await;
        let ctx = AppCtx::new(exec_settings(dir));
        let args = ClaimPlanArgs {
            chain: Some("1".to_string()),
            assets: vec![ASSET_A.to_string()],
            reward_token: Some(REWARD.to_string()),
            amount: Some("1000000".to_string()),
            recipient: None,
            controller_address: Some(CONTROLLER.to_string()),
            pool_address_provider: None,
            provider: Some("aave".to_string()),
            rpc_url: Some(rpc.uri()),
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(SENDER.to_string()),
            },
            input: InputFlags::default(),
        };
        let env = handle(&ctx, RewardsCmd::Claim(ClaimVerbCmd::Plan(args)))
            .await
            .expect("plan a claim_rewards action for the status fixture");
        let action_id = env.data.expect("plan data")["action_id"]
            .as_str()
            .expect("action_id")
            .to_string();
        // Re-point the persisted step rpc_url at the requested endpoint.
        let store = ActionStore::open(dir.join("actions.db"), dir.join("actions.lock"))
            .expect("open store");
        let mut action = store.get(&action_id).expect("load");
        for step in &mut action.steps {
            step.rpc_url = step_rpc.to_string();
        }
        store.save(&action).expect("persist step rpc_url");
        action_id
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

    async fn run_claim_status(dir: &Path, action_id: &str) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(
            &ctx,
            RewardsCmd::Claim(ClaimVerbCmd::Status(StatusArgs {
                action_id: Some(action_id.to_string()),
            })),
        )
        .await
    }

    async fn run_compound_status(dir: &Path, action_id: &str) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(
            &ctx,
            RewardsCmd::Compound(CompoundVerbCmd::Status(StatusArgs {
                action_id: Some(action_id.to_string()),
            })),
        )
        .await
    }

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn data_of(env: &Envelope) -> Value {
        env.data.clone().expect("status envelope carries `data`")
    }

    // --- 1. claim status success envelope ----------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_status_planned_emits_success_envelope() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_claim(tmp.path(), "http://127.0.0.1:0").await;
        let env = run_claim_status(tmp.path(), &action_id)
            .await
            .expect("status on a planned claim should succeed");

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "rewards claim status");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        let data = data_of(&env);
        assert_eq!(data["action_id"], Value::from(action_id.as_str()));
        assert_eq!(data["intent_type"], Value::from("claim_rewards"));
        assert_eq!(data["status"], Value::from("planned"));
    }

    // --- 2. claim status reflects lifecycle transitions --------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_status_reflects_completed_transition() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_claim(tmp.path(), "http://127.0.0.1:0").await;
        set_status(tmp.path(), &action_id, ActionStatus::Completed);
        let env = run_claim_status(tmp.path(), &action_id)
            .await
            .expect("status ok");
        assert_eq!(data_of(&env)["status"], Value::from("completed"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_status_reflects_running_transition() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_claim(tmp.path(), "http://127.0.0.1:0").await;
        set_status(tmp.path(), &action_id, ActionStatus::Running);
        let env = run_claim_status(tmp.path(), &action_id)
            .await
            .expect("status ok");
        assert_eq!(data_of(&env)["status"], Value::from("running"));
    }

    // --- 3. compound status success envelope -------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_status_emits_success_envelope() {
        let tmp = TempDir::new().expect("tempdir");
        // A directly-persisted compound action (status read needs no build).
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "compound_rewards",
            "eip155:1",
            Default::default(),
        );
        action.provider = "aave".to_string();
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        save_action(tmp.path(), &action);

        let env = run_compound_status(tmp.path(), &action.action_id)
            .await
            .expect("status on a compound action should succeed");
        assert!(env.success);
        assert_eq!(env.meta.command, "rewards compound status");
        assert_eq!(env.meta.cache.status, "bypass");
        let data = data_of(&env);
        assert_eq!(data["intent_type"], Value::from("compound_rewards"));
        assert_eq!(data["status"], Value::from("planned"));
    }

    // --- 4. action-id validation -------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_status_rejects_empty_action_id() {
        let tmp = TempDir::new().expect("tempdir");
        let err = run_claim_status(tmp.path(), "")
            .await
            .expect_err("empty action id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_status_rejects_malformed_action_id() {
        let tmp = TempDir::new().expect("tempdir");
        let err = run_compound_status(tmp.path(), "act_not_hex")
            .await
            .expect_err("malformed action id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- 5. load failure for an unknown action -----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_status_unknown_action_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        let err = run_claim_status(tmp.path(), "act_0123456789abcdef0123456789abcdef")
            .await
            .expect_err("unknown action surfaces a load (usage) error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- 6. intent gates (cross-sibling) -----------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn claim_status_rejects_compound_intent() {
        let tmp = TempDir::new().expect("tempdir");
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "compound_rewards",
            "eip155:1",
            Default::default(),
        );
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        save_action(tmp.path(), &action);

        let err = run_claim_status(tmp.path(), &action.action_id)
            .await
            .expect_err("compound action rejected by claim status");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("action is not a rewards claim intent"),
            "got: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compound_status_rejects_claim_intent() {
        let tmp = TempDir::new().expect("tempdir");
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "claim_rewards",
            "eip155:1",
            Default::default(),
        );
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        save_action(tmp.path(), &action);

        let err = run_compound_status(tmp.path(), &action.action_id)
            .await
            .expect_err("claim action rejected by compound status");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("action is not a rewards compound intent"),
            "got: {err}"
        );
    }
}
