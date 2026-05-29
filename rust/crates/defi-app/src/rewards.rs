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
