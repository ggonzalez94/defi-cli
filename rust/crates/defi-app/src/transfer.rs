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
    use defi_errors::Error;
    use defi_model::Envelope;

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, TransferSubmitArgs};

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
    pub async fn handle(_ctx: &AppCtx, cmd: TransferCmd) -> Result<Envelope, Error> {
        let path = format!("transfer {}", cmd.path());
        let ws = match cmd {
            TransferCmd::Plan(_) => "WS3",
            TransferCmd::Submit(_) | TransferCmd::Status(_) => "WS4",
        };
        Err(AppCtx::unimplemented(&path, ws))
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
