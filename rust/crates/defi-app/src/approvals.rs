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
    use defi_errors::Error;
    use defi_model::Envelope;

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, SubmitArgs};

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
    pub async fn handle(_ctx: &AppCtx, cmd: ApprovalsCmd) -> Result<Envelope, Error> {
        let path = format!("approvals {}", cmd.path());
        let ws = match cmd {
            ApprovalsCmd::Plan(_) => "WS3",
            ApprovalsCmd::Submit(_) | ApprovalsCmd::Status(_) => "WS4",
        };
        Err(AppCtx::unimplemented(&path, ws))
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
