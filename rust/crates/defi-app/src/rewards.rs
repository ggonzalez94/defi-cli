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
    use defi_errors::Error;
    use defi_model::Envelope;

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, SubmitArgs};

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
    pub async fn handle(_ctx: &AppCtx, cmd: RewardsCmd) -> Result<Envelope, Error> {
        let path = format!("rewards {}", cmd.path());
        let ws = if path.ends_with("plan") { "WS3" } else { "WS4" };
        Err(AppCtx::unimplemented(&path, ws))
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
