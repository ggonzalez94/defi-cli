//! `bridge` command group handler (Go: `internal/app` — `newBridgeCommand` in
//! `runner.go` plus `addBridgeExecutionSubcommands` in
//! `bridge_execution_commands.go`).
//!
//! This module owns the **bridge-command-specific** glue that sits between the
//! runner's cache-flow core ([`crate::runner`]), the bridge quote providers
//! ([`defi_providers::BridgeProvider`]), the bridge analytics providers
//! ([`defi_providers::BridgeDataProvider`]), and the action-build registry
//! ([`defi_execution::builder::Registry`]). Specifically it owns:
//!
//! * the bridge quote/plan request builder (`build_bridge_request`) — source +
//!   destination chain parsing, source asset parsing, the `--to-asset`
//!   inference rule (default to the source asset's symbol; fail usage when the
//!   source asset has no symbol to infer from), and amount normalization
//!   against the source asset's decimals (defaulting non-positive decimals to
//!   18);
//! * the `bridge quote` pre-provider guard order (provider required → usage;
//!   provider not in the registered set → unsupported);
//! * the `bridge list` / `bridge details` data-provider gate
//!   (`ensure_bridge_data_provider`: a missing DefiLlama data provider is
//!   unsupported, not usage);
//! * the `bridge plan` schema identity input constraints
//!   (`bridge_plan_identity_constraints`: the standard
//!   `exactly_one_of {wallet, from_address}`);
//! * the persisted-intent gate (`bridge submit` / `bridge status` reject a
//!   non-`bridge` action with a usage error).
//!
//! The bridge request/option types (`BridgeQuoteRequest`,
//! `BridgeExecutionOptions`), the action-build registry routing
//! (`build_bridge_action`, including the unknown / quote-only provider error
//! semantics), the shared execution-identity resolver
//! (`resolve_execution_identity`), and the cache-flow core are owned elsewhere
//! (`defi_execution::builder`, the execution-identity module, [`crate::runner`])
//! and are NOT re-owned here; this module consumes them.

#![allow(dead_code, unused_variables)]

use defi_errors::{Code, Error};
use defi_execution::BridgeQuoteRequest;
use defi_id::{normalize_amount, parse_asset, parse_chain};
use defi_schema::InputConstraint;

/// Build a [`BridgeQuoteRequest`] from the raw bridge flags.
///
/// Parity with the Go `buildRequest` closure shared by `bridge quote` and
/// `bridge plan`:
/// 1. parse `from` then `to` chains (delegates to `defi_id::parse_chain`);
/// 2. parse `asset` on the source chain (delegates to `defi_id::parse_asset`);
/// 3. resolve the destination asset: if `--to-asset` is empty, default to the
///    source asset's `symbol`; if the source asset has no symbol to infer from,
///    fail with a [`defi_errors::Code::Usage`] error
///    (`destination asset cannot be inferred, provide --to-asset`). Parse the
///    resolved destination asset on the destination chain — a parse failure is
///    wrapped as [`defi_errors::Code::Usage`];
/// 4. normalize the amount against the source asset's `decimals` (a
///    non-positive `decimals` defaults to 18), carrying both base + decimal
///    forms (spec §2.4);
/// 5. carry the trimmed `from_amount_for_gas` verbatim.
///
/// All validation failures surface as typed [`Error`]s (usage for the inferred
/// / destination-asset paths).
pub fn build_bridge_request(
    from_arg: &str,
    to_arg: &str,
    asset_arg: &str,
    to_asset_arg: &str,
    amount_base: &str,
    amount_decimal: &str,
    from_amount_for_gas: &str,
) -> Result<BridgeQuoteRequest, Error> {
    // 1. parse source then destination chain (delegates to `defi_id`).
    let from_chain = parse_chain(from_arg)?;
    let to_chain = parse_chain(to_arg)?;

    // 2. parse the source asset on the source chain.
    let from_asset = parse_asset(asset_arg, &from_chain)?;

    // 3. resolve the destination asset: default to the source asset's symbol;
    //    a source asset with no symbol to infer from is a usage error.
    let to_asset_input = to_asset_arg.trim();
    let to_asset_input = if to_asset_input.is_empty() {
        if from_asset.symbol.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "destination asset cannot be inferred, provide --to-asset",
            ));
        }
        from_asset.symbol.clone()
    } else {
        to_asset_input.to_string()
    };
    let to_asset = parse_asset(&to_asset_input, &to_chain)
        .map_err(|err| Error::wrap(Code::Usage, "resolve destination asset", err))?;

    // 4. normalize the amount against the source asset's decimals (non-positive
    //    decimals default to 18), carrying both base + decimal forms.
    let mut decimals = from_asset.decimals;
    if decimals <= 0 {
        decimals = 18;
    }
    let (amount_base_units, amount_decimal) =
        normalize_amount(amount_base, amount_decimal, decimals)?;

    Ok(BridgeQuoteRequest {
        from_chain,
        to_chain,
        from_asset,
        to_asset,
        amount_base_units,
        amount_decimal,
        // 5. carry the trimmed `from_amount_for_gas` verbatim.
        from_amount_for_gas: from_amount_for_gas.trim().to_string(),
    })
}

/// Resolve the (normalized) `bridge quote` provider name.
///
/// Parity with the Go `quoteCmd` `RunE` head guard order (spec §2.5: no
/// implicit provider default):
/// 1. an empty `--provider` → [`defi_errors::Code::Usage`]
///    (`--provider is required (across|lifi)`);
/// 2. a provider not present in `known` (the set of registered bridge quote
///    provider names, already lowercased) → [`defi_errors::Code::Unsupported`]
///    (`unsupported bridge provider`).
///
/// On success returns the trimmed + lowercased provider name. `known` is the
/// set of registered quote provider names so this is testable without a live
/// provider map.
pub fn resolve_bridge_quote_provider(provider: &str, known: &[&str]) -> Result<String, Error> {
    let name = provider.trim().to_ascii_lowercase();
    if name.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "--provider is required (across|lifi)",
        ));
    }
    if !known.contains(&name.as_str()) {
        return Err(Error::new(Code::Unsupported, "unsupported bridge provider"));
    }
    Ok(name)
}

/// Gate the `bridge list` / `bridge details` DefiLlama data provider.
///
/// Parity with the Go `listCmd` / `detailsCmd` `RunE` head guard: when the
/// (key-gated) DefiLlama bridge data provider is NOT configured, fail with
/// [`defi_errors::Code::Unsupported`] (`bridge data provider is not
/// configured`) — NOT a usage error. `configured` models the presence of the
/// `bridgeDataProviders["defillama"]` entry.
pub fn ensure_bridge_data_provider(configured: bool) -> Result<(), Error> {
    if !configured {
        return Err(Error::new(
            Code::Unsupported,
            "bridge data provider is not configured",
        ));
    }
    Ok(())
}

/// The `bridge plan` schema identity input constraints.
///
/// Parity with Go `standardExecutionIdentityInputConstraints`: a single
/// `exactly_one_of` entry over `[wallet, from_address]` (no `when` clause —
/// bridge planning is OWS-first / standard EVM, with no per-provider identity
/// branching like swap's Tempo/TaikoSwap split).
pub fn bridge_plan_identity_constraints() -> Vec<InputConstraint> {
    vec![InputConstraint {
        kind: "exactly_one_of".to_string(),
        fields: vec!["wallet".to_string(), "from_address".to_string()],
        when: Default::default(),
        description: "Provide exactly one execution identity input: `wallet` (OWS, recommended) or `from_address` (local signer).".to_string(),
    }]
}

/// Validate that a persisted action is a `bridge` intent.
///
/// Parity with the `submit` / `status` guard `action.IntentType != "bridge"`: a
/// non-`bridge` intent yields a [`defi_errors::Code::Usage`] error whose message
/// is `action is not a bridge intent`.
pub fn ensure_bridge_intent(intent_type: &str) -> Result<(), Error> {
    if intent_type != "bridge" {
        return Err(Error::new(Code::Usage, "action is not a bridge intent"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::bridge` (Go: `internal/app` bridge command
    //! group: `newBridgeCommand` in `runner.go` + `addBridgeExecutionSubcommands`
    //! in `bridge_execution_commands.go`)
    //!
    //! This module owns the **bridge-command glue**. "Correct" means it preserves
    //! the runner-owned bridge behaviors AND the stable machine contract (design
    //! spec §2.2 exit codes, §2.4 ids/amounts kept consistent, §2.5 multi-provider
    //! paths require an explicit `--provider`). The bridge request/option types,
    //! the action-build registry routing (`build_bridge_action`, with its
    //! unknown / quote-only provider error semantics — already covered by the
    //! `defi-execution::builder` RED suite), the shared execution-identity
    //! resolver, and the cache-flow core are owned elsewhere and are NOT
    //! re-asserted here. Criteria:
    //!
    //! 1. **Request building + `--to-asset` inference + amount normalization.**
    //!    `build_bridge_request` mirrors Go `buildRequest`.
    //!    (a) Source + destination chains parse to their CAIP-2 ids; the source
    //!        asset parses on the source chain.
    //!    (b) An empty `--to-asset` defaults to the SOURCE asset's symbol and
    //!        parses that on the DESTINATION chain (so a USDC bridge keeps the
    //!        USDC destination asset id on the target chain).
    //!    (c) An explicit `--to-asset` overrides the inference.
    //!    (d) The amount is normalized against the source asset's decimals (USDC
    //!        = 6): base `1000000` ⇔ decimal `1` stay consistent (spec §2.4).
    //!    (e) `from_amount_for_gas` is carried verbatim (trimmed).
    //!    (Ported indirectly from `TestBridgePlanAcceptsStructuredWalletInput`,
    //!    which bridges USDC 1→10 with an inferred/explicit USDC destination.)
    //!
    //! 2. **`--to-asset` inference failure is a usage error.** When the source
    //!    asset cannot be parsed to a symbol AND `--to-asset` is empty,
    //!    `build_bridge_request` fails with [`Code::Usage`] (exit 2) and a message
    //!    containing `destination asset cannot be inferred`. (Go `buildRequest`
    //!    `--to-asset` branch.)
    //!
    //! 3. **`bridge quote` pre-provider guard order + exit codes.**
    //!    `resolve_bridge_quote_provider` mirrors the Go `quoteCmd` head.
    //!    (a) An empty `--provider` → [`Code::Usage`] (exit 2) BEFORE anything
    //!        else (spec §2.5: no implicit provider default), message contains
    //!        `--provider is required`.
    //!    (b) A provider not in the registered set → [`Code::Unsupported`]
    //!        (exit 13), message contains `unsupported bridge provider`.
    //!    (c) A registered provider resolves to its trimmed + lowercased name
    //!        (e.g. `ACROSS` → `across`).
    //!
    //! 4. **`bridge list` / `bridge details` data-provider gate.**
    //!    `ensure_bridge_data_provider(false)` → [`Code::Unsupported`] (exit 13,
    //!    NOT usage) with message `bridge data provider is not configured`;
    //!    `ensure_bridge_data_provider(true)` → `Ok`. (Go `listCmd` / `detailsCmd`
    //!    head; ported from `TestRunnerBridgeListRejectsProviderFlag` /
    //!    `TestRunnerBridgeDetailsRequiresBridgeFlag` are cobra-flag-wiring
    //!    concerns and are SKIPPED — see below — but the provider-gate semantics
    //!    they sit behind ARE asserted here.)
    //!
    //! 5. **`bridge plan` schema identity constraints.**
    //!    `bridge_plan_identity_constraints` returns EXACTLY one `exactly_one_of`
    //!    entry over `[wallet, from_address]` with no `when` clause — the standard
    //!    OWS-first execution identity (no per-provider branching, unlike swap).
    //!    (Mirrors Go `standardExecutionIdentityInputConstraints`, advertised by
    //!    `bridge plan` via `configureStructuredInput`.)
    //!
    //! 6. **Persisted-intent gate.** `ensure_bridge_intent` accepts `"bridge"`
    //!    and rejects any other intent with [`Code::Usage`] (exit 2) +
    //!    `action is not a bridge intent`. (Ported from the `submit` / `status`
    //!    `IntentType != "bridge"` guards in `bridge_execution_commands.go`.)
    //!
    //! SKIPPED (Go internal-detail / wrong-module):
    //!   * cobra flag wiring + flag defaults (`--slippage-bps 50`, `--simulate
    //!     true`, required-flag marking) — harness concern, asserted by the
    //!     integration golden-CLI suite, not this unit;
    //!   * the unknown / quote-only bridge-provider routing error semantics for
    //!     `bridge plan` — owned by `defi_execution::builder::Registry`
    //!     (`build_bridge_action`) and covered by its own RED suite (B2/B3);
    //!   * the full `submit` signer/backend plumbing, pre-sign guardrails, and
    //!     receipt/settlement polling — `defi-execution` concern;
    //!   * the DefiLlama / Across / LiFi adapter response bodies — per-provider
    //!     (`defi-providers`) concern, covered there via wiremock;
    //!   * the cache-key construction + TTL selection — runner concern, owned by
    //!     [`crate::runner`].

    use super::*;
    use defi_errors::{exit_code, Code};

    // --- helpers -----------------------------------------------------------

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    // --- 1. request building + --to-asset inference + amount normalization ---

    #[test]
    fn build_request_infers_destination_asset_from_source_symbol() {
        // USDC bridged 1 → 10 with no --to-asset: the destination asset is
        // inferred from the source symbol (USDC) and parsed on chain 10.
        let req = build_bridge_request("1", "10", "USDC", "", "1000000", "", "")
            .expect("inferred destination asset");
        assert_eq!(req.from_chain.caip2, "eip155:1");
        assert_eq!(req.to_chain.caip2, "eip155:10");
        assert_eq!(req.from_asset.symbol, "USDC");
        // Destination asset resolved to USDC on the destination chain.
        assert_eq!(req.to_asset.symbol, "USDC");
        assert_eq!(req.to_asset.chain_id, "eip155:10");
        // Amount normalized against USDC decimals (6): base ⇔ decimal consistent.
        assert_eq!(req.amount_base_units, "1000000");
        assert_eq!(req.amount_decimal, "1");
    }

    #[test]
    fn build_request_honors_explicit_to_asset_override() {
        let req = build_bridge_request("1", "10", "USDC", "USDC", "1000000", "", "")
            .expect("explicit destination asset");
        assert_eq!(req.to_asset.symbol, "USDC");
        assert_eq!(req.to_asset.chain_id, "eip155:10");
        assert_eq!(req.amount_base_units, "1000000");
    }

    #[test]
    fn build_request_normalizes_from_decimal_amount() {
        // The decimal form normalizes to base units against USDC decimals (6).
        let req = build_bridge_request("1", "10", "USDC", "", "", "1", "")
            .expect("decimal amount normalizes");
        assert_eq!(req.amount_base_units, "1000000");
        assert_eq!(req.amount_decimal, "1");
    }

    #[test]
    fn build_request_carries_from_amount_for_gas_trimmed() {
        let req = build_bridge_request("1", "10", "USDC", "", "1000000", "", "  500000  ")
            .expect("from-amount-for-gas carried");
        assert_eq!(req.from_amount_for_gas, "500000");
    }

    #[test]
    fn build_request_rejects_both_amount_forms() {
        // Amount normalization rejects supplying both base + decimal (spec §2.4).
        let err = build_bridge_request("1", "10", "USDC", "", "1000000", "1", "")
            .expect_err("both amount forms rejected");
        assert_eq!(err.code, Code::Usage);
    }

    // --- 2. --to-asset inference failure -----------------------------------

    #[test]
    fn build_request_inference_failure_is_usage() {
        // A bare contract address has no symbol to infer a destination asset
        // from; with an empty --to-asset this is a usage error.
        let err = build_bridge_request(
            "1",
            "10",
            "0x1111111111111111111111111111111111111111",
            "",
            "1000000",
            "",
            "",
        )
        .expect_err("uninferable destination asset rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("destination asset cannot be inferred"),
            "got: {err}"
        );
    }

    // --- 3. bridge quote pre-provider guard order --------------------------

    const KNOWN: &[&str] = &["across", "lifi", "bungee"];

    #[test]
    fn quote_requires_provider_first() {
        // Spec §2.5: empty --provider is a usage error before anything else.
        let err = resolve_bridge_quote_provider("", KNOWN).expect_err("empty provider rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("--provider is required"),
            "got: {err}"
        );
    }

    #[test]
    fn quote_rejects_unknown_provider_as_unsupported() {
        let err =
            resolve_bridge_quote_provider("bogus", KNOWN).expect_err("unknown provider rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string().contains("unsupported bridge provider"),
            "got: {err}"
        );
    }

    #[test]
    fn quote_resolves_registered_provider_normalized() {
        let name = resolve_bridge_quote_provider("  ACROSS ", KNOWN)
            .expect("registered provider resolves");
        assert_eq!(name, "across");
        let name = resolve_bridge_quote_provider("lifi", KNOWN).expect("lifi resolves");
        assert_eq!(name, "lifi");
    }

    // --- 4. bridge list / details data-provider gate -----------------------

    #[test]
    fn data_provider_gate_unconfigured_is_unsupported() {
        let err = ensure_bridge_data_provider(false).expect_err("missing data provider rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .contains("bridge data provider is not configured"),
            "got: {err}"
        );
    }

    #[test]
    fn data_provider_gate_configured_is_ok() {
        ensure_bridge_data_provider(true).expect("configured data provider accepted");
    }

    // --- 5. bridge plan schema identity constraints ------------------------

    #[test]
    fn plan_identity_constraints_are_standard_exactly_one_of() {
        let constraints = bridge_plan_identity_constraints();
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].kind, "exactly_one_of");
        assert_eq!(
            constraints[0].fields,
            vec!["wallet".to_string(), "from_address".to_string()]
        );
        // No per-provider `when` clause — bridge planning is provider-agnostic
        // for identity (unlike swap's Tempo/TaikoSwap split).
        assert!(
            constraints[0].when.is_empty(),
            "standard identity constraint has no `when` clause"
        );
    }

    // --- 6. persisted-intent gate ------------------------------------------

    #[test]
    fn ensure_bridge_intent_accepts_bridge() {
        ensure_bridge_intent("bridge").expect("bridge intent accepted");
    }

    #[test]
    fn ensure_bridge_intent_rejects_non_bridge() {
        // A swap action submitted/queried through `bridge submit|status` fails.
        let err = ensure_bridge_intent("swap").expect_err("non-bridge intent rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("action is not a bridge intent"),
            "got: {err}"
        );
    }
}
