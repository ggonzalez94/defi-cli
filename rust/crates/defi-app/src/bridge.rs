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

/// Map a bridge fetch result to the Go `statusFromErr` provider-status string:
/// `Ok` → `"ok"`; `Auth` → `"auth_error"`; `RateLimited` → `"rate_limited"`;
/// `Unavailable` → `"unavailable"`; anything else → `"error"`.
fn status_from_result<T>(res: &Result<T, Error>) -> String {
    match res {
        Ok(_) => "ok",
        Err(err) => match err.code {
            Code::Auth => "auth_error",
            Code::RateLimited => "rate_limited",
            Code::Unavailable => "unavailable",
            _ => "error",
        },
    }
    .to_string()
}

/// The `bridge quote` cache-key payload (mirrors the Go `quoteCmd` cache-key
/// `map[string]any` at `runner.go` ~L964). The Go `cacheKey` hashes the map's
/// canonical JSON, whose keys serialize in ALPHABETICAL order, so the struct
/// fields are declared alphabetically. Identical inputs MUST yield an identical
/// key (the runner hashes the canonical JSON).
#[derive(serde::Serialize)]
struct BridgeQuoteCacheKey<'a> {
    amount: &'a str,
    from: &'a str,
    from_amount_for_gas: &'a str,
    from_asset: &'a str,
    provider: &'a str,
    to: &'a str,
    to_asset: &'a str,
}

/// The `bridge list` cache-key payload (Go `listCmd` map at `runner.go` ~L1018;
/// alphabetical key order).
#[derive(serde::Serialize)]
struct BridgeListCacheKey<'a> {
    include_chains: bool,
    limit: i64,
    provider: &'a str,
}

/// The `bridge details` cache-key payload (Go `detailsCmd` map at `runner.go`
/// ~L1058; alphabetical key order). `bridge` is the lowercased + trimmed ref.
#[derive(serde::Serialize)]
struct BridgeDetailsCacheKey<'a> {
    bridge: &'a str,
    include_chain_breakdown: bool,
    provider: &'a str,
}

/// `bridge quote` time-to-live (Go `runCachedCommand(..., 15*time.Second, ...)`).
const BRIDGE_QUOTE_TTL_SECS: u64 = 15;
/// `bridge list` / `bridge details` time-to-live (Go `60*time.Second`).
const BRIDGE_DATA_TTL_SECS: u64 = 60;

/// clap parsing + handler for the `bridge` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::Error;
    use defi_model::Envelope;

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, SubmitArgs};

    /// `bridge` subcommands (Go `newBridgeCommand`).
    #[derive(Subcommand, Debug)]
    pub enum BridgeCmd {
        /// Get bridge quote.
        Quote(QuoteArgs),
        /// List bridge volumes and coverage (DefiLlama key required).
        List(ListArgs),
        /// Get bridge volume details and chain breakdown (DefiLlama key required).
        Details(DetailsArgs),
        /// Create and persist a bridge action plan.
        Plan(PlanArgs),
        /// Execute an existing bridge action.
        Submit(SubmitArgs),
        /// Get bridge action status.
        Status(StatusArgs),
    }

    impl BridgeCmd {
        /// The leaf path token (for `meta.command`).
        pub fn path(&self) -> &'static str {
            match self {
                BridgeCmd::Quote(_) => "quote",
                BridgeCmd::List(_) => "list",
                BridgeCmd::Details(_) => "details",
                BridgeCmd::Plan(_) => "plan",
                BridgeCmd::Submit(_) => "submit",
                BridgeCmd::Status(_) => "status",
            }
        }
    }

    /// `bridge quote` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct QuoteArgs {
        /// Source chain.
        #[arg(long)]
        pub from: Option<String>,
        /// Destination chain.
        #[arg(long)]
        pub to: Option<String>,
        /// Asset (symbol/address/CAIP-19) on source chain.
        #[arg(long)]
        pub asset: Option<String>,
        /// Destination asset override (symbol/address/CAIP-19).
        #[arg(long = "to-asset")]
        pub to_asset: Option<String>,
        /// Bridge provider (across|lifi|bungee; no API key required).
        #[arg(long)]
        pub provider: Option<String>,
        /// Amount in base units.
        #[arg(long)]
        pub amount: Option<String>,
        /// Amount in decimal units.
        #[arg(long = "amount-decimal")]
        pub amount_decimal: Option<String>,
        /// Optional source token base units reserved for destination native gas (LiFi).
        #[arg(long = "from-amount-for-gas")]
        pub from_amount_for_gas: Option<String>,
        #[command(flatten)]
        pub input: crate::execflags::InputFlags,
    }

    /// `bridge list` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct ListArgs {
        /// Include chain coverage for each bridge.
        #[arg(long = "include-chains", default_value_t = true, action = clap::ArgAction::Set)]
        pub include_chains: bool,
        /// Maximum bridges to return.
        #[arg(long, default_value_t = 20)]
        pub limit: i64,
    }

    /// `bridge details` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct DetailsArgs {
        /// Bridge identifier (id, slug, or name).
        #[arg(long)]
        pub bridge: Option<String>,
        /// Include per-chain bridge stats.
        #[arg(long = "include-chain-breakdown", default_value_t = true, action = clap::ArgAction::Set)]
        pub include_chain_breakdown: bool,
    }

    /// `bridge plan` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct PlanArgs {
        /// Source chain.
        #[arg(long)]
        pub from: Option<String>,
        /// Destination chain.
        #[arg(long)]
        pub to: Option<String>,
        /// Asset on source chain.
        #[arg(long)]
        pub asset: Option<String>,
        /// Destination asset override.
        #[arg(long = "to-asset")]
        pub to_asset: Option<String>,
        /// Bridge provider (across|lifi).
        #[arg(long)]
        pub provider: Option<String>,
        /// Amount in base units.
        #[arg(long)]
        pub amount: Option<String>,
        /// Amount in decimal units.
        #[arg(long = "amount-decimal")]
        pub amount_decimal: Option<String>,
        /// Optional source token base units reserved for destination native gas (LiFi).
        #[arg(long = "from-amount-for-gas")]
        pub from_amount_for_gas: Option<String>,
        /// Recipient address (defaults to the resolved sender address).
        #[arg(long)]
        pub recipient: Option<String>,
        /// Max slippage in basis points.
        #[arg(long = "slippage-bps", default_value_t = 50)]
        pub slippage_bps: i64,
        /// RPC URL override for source chain.
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

    /// Handle `bridge <sub>`.
    pub async fn handle(ctx: &AppCtx, cmd: BridgeCmd) -> Result<Envelope, Error> {
        match cmd {
            BridgeCmd::Quote(args) => handle_quote(ctx, args).await,
            BridgeCmd::List(args) => handle_list(ctx, args).await,
            BridgeCmd::Details(args) => handle_details(ctx, args).await,
            BridgeCmd::Plan(_) => Err(AppCtx::unimplemented("bridge plan", "WS3")),
            BridgeCmd::Submit(_) => Err(AppCtx::unimplemented("bridge submit", "WS4")),
            BridgeCmd::Status(_) => Err(AppCtx::unimplemented("bridge status", "WS4")),
        }
    }

    /// Resolved `bridge quote` flag values after merging structured input.
    struct QuoteValues {
        provider: String,
        from: String,
        to: String,
        asset: String,
        to_asset: String,
        amount: String,
        amount_decimal: String,
        from_amount_for_gas: String,
    }

    /// Handle `bridge quote`: merge structured input, run the pre-provider guard
    /// order, build the request, then route through the selected
    /// [`defi_providers::BridgeProvider`] adapter via the cache flow.
    ///
    /// Parity with the Go `quoteCmd.RunE` (`runner.go` ~L909-979): the empty /
    /// unsupported provider guards run BEFORE any chain/asset parse (spec §2.5),
    /// the request is built ([`super::build_bridge_request`]), and the provider's
    /// `QuoteBridge` is invoked inside [`crate::runner::run_cached_command`]
    /// (15s TTL) so a fresh cache hit short-circuits the provider.
    async fn handle_quote(ctx: &AppCtx, args: QuoteArgs) -> Result<Envelope, Error> {
        use defi_model::ProviderStatus;

        // 1. Resolve flag values, merging any structured input (Go PreRunE
        //    `applyStructuredFlagInput`). Explicitly-set flags are never
        //    overridden; unknown JSON keys / null values are usage errors.
        let mut values = QuoteValues {
            provider: args.provider.clone().unwrap_or_default(),
            from: args.from.clone().unwrap_or_default(),
            to: args.to.clone().unwrap_or_default(),
            asset: args.asset.clone().unwrap_or_default(),
            to_asset: args.to_asset.clone().unwrap_or_default(),
            amount: args.amount.clone().unwrap_or_default(),
            amount_decimal: args.amount_decimal.clone().unwrap_or_default(),
            from_amount_for_gas: args.from_amount_for_gas.clone().unwrap_or_default(),
        };
        let explicit: std::collections::HashSet<&str> = {
            let mut s = std::collections::HashSet::new();
            if args.provider.is_some() {
                s.insert("provider");
            }
            if args.from.is_some() {
                s.insert("from");
            }
            if args.to.is_some() {
                s.insert("to");
            }
            if args.asset.is_some() {
                s.insert("asset");
            }
            if args.to_asset.is_some() {
                s.insert("to-asset");
            }
            if args.amount.is_some() {
                s.insert("amount");
            }
            if args.amount_decimal.is_some() {
                s.insert("amount-decimal");
            }
            if args.from_amount_for_gas.is_some() {
                s.insert("from-amount-for-gas");
            }
            s
        };
        apply_quote_structured_input(&args.input, &explicit, &mut values)?;

        // 2. Pre-provider guard order: empty `--provider` → usage; an unknown
        //    provider → unsupported (BEFORE any chain/asset parse).
        let provider_name =
            super::resolve_bridge_quote_provider(&values.provider, ctx.bridge_provider_names())?;

        // 3. Build the canonical request (chain/asset parse, `--to-asset`
        //    inference, amount normalization, `from_amount_for_gas` carry).
        let req = super::build_bridge_request(
            &values.from,
            &values.to,
            &values.asset,
            &values.to_asset,
            &values.amount,
            &values.amount_decimal,
            &values.from_amount_for_gas,
        )?;

        // 4. Resolve the provider adapter (registered above -> always Some).
        let provider = ctx.bridge_provider(&provider_name).ok_or_else(|| {
            Error::new(
                defi_errors::Code::Unsupported,
                "unsupported bridge provider",
            )
        })?;

        // 5. Compose the cache key (Go cacheKey map; alphabetical key order) +
        //    fetch closure.
        let path = "bridge quote";
        let key = crate::protocols::cache_key(
            path,
            &super::BridgeQuoteCacheKey {
                amount: &req.amount_base_units,
                from: &req.from_chain.caip2,
                from_amount_for_gas: &req.from_amount_for_gas,
                from_asset: &req.from_asset.asset_id,
                provider: &provider_name,
                to: &req.to_chain.caip2,
                to_asset: &req.to_asset.asset_id,
            },
        );
        let ttl = std::time::Duration::from_secs(super::BRIDGE_QUOTE_TTL_SECS);
        let adapter_name = provider.info().name;
        let req_for_fetch = req.clone();

        ctx.run_cached_command(path, &key, ttl, || {
            let res = crate::ctx::block_on_fetch(provider.quote_bridge(req_for_fetch));
            let status = ProviderStatus {
                name: adapter_name.clone(),
                status: super::status_from_result(&res),
                latency_ms: 0,
            };
            match res {
                Ok(quote) => match serde_json::to_value(&quote) {
                    Ok(data) => Ok(crate::runner::FetchOutcome {
                        data,
                        providers: vec![status],
                        warnings: Vec::new(),
                        partial: false,
                    }),
                    Err(e) => {
                        let err =
                            Error::wrap(defi_errors::Code::Internal, "serialize bridge quote", e);
                        let st = ProviderStatus {
                            name: adapter_name.clone(),
                            status: "error".to_string(),
                            latency_ms: 0,
                        };
                        Err((vec![st], Vec::new(), false, err))
                    }
                },
                Err(err) => Err((vec![status], Vec::new(), false, err)),
            }
        })
    }

    /// Handle `bridge list`: the DefiLlama-backed bridge analytics list.
    ///
    /// Parity with the Go `listCmd.RunE` (`runner.go` ~L1008-1029): the bridge
    /// data provider is the always-configured DefiLlama client (the
    /// `not configured` branch is dead in production); the adapter's
    /// `require_bridge_api_key` enforces the [`defi_errors::Code::Auth`] key
    /// gate, and the provider's `ListBridges` is invoked inside
    /// [`crate::runner::run_cached_command`] (60s TTL).
    async fn handle_list(ctx: &AppCtx, args: ListArgs) -> Result<Envelope, Error> {
        use defi_model::ProviderStatus;
        use defi_providers::{BridgeDataProvider, BridgeListRequest};

        const PROVIDER_NAME: &str = "defillama";
        // The DefiLlama bridge data provider is always configured (Go keeps the
        // `llama` client in `bridgeDataProviders` unconditionally); the gate is
        // retained for parity but never trips.
        super::ensure_bridge_data_provider(true)?;

        let req = BridgeListRequest {
            limit: args.limit,
            include_chains: args.include_chains,
        };
        let path = "bridge list";
        let key = crate::protocols::cache_key(
            path,
            &super::BridgeListCacheKey {
                include_chains: req.include_chains,
                limit: req.limit,
                provider: PROVIDER_NAME,
            },
        );
        let ttl = std::time::Duration::from_secs(super::BRIDGE_DATA_TTL_SECS);
        let provider = ctx.defillama();
        let req_for_fetch = req.clone();

        ctx.run_cached_command(path, &key, ttl, || {
            let res = crate::ctx::block_on_fetch(provider.list_bridges(req_for_fetch));
            let status = ProviderStatus {
                name: PROVIDER_NAME.to_string(),
                status: super::status_from_result(&res),
                latency_ms: 0,
            };
            match res {
                Ok(rows) => match serde_json::to_value(&rows) {
                    Ok(data) => Ok(crate::runner::FetchOutcome {
                        data,
                        providers: vec![status],
                        warnings: Vec::new(),
                        partial: false,
                    }),
                    Err(e) => {
                        let err =
                            Error::wrap(defi_errors::Code::Internal, "serialize bridge list", e);
                        let st = ProviderStatus {
                            name: PROVIDER_NAME.to_string(),
                            status: "error".to_string(),
                            latency_ms: 0,
                        };
                        Err((vec![st], Vec::new(), false, err))
                    }
                },
                Err(err) => Err((vec![status], Vec::new(), false, err)),
            }
        })
    }

    /// Handle `bridge details`: the DefiLlama-backed bridge volume detail view.
    ///
    /// Parity with the Go `detailsCmd.RunE` (`runner.go` ~L1048-1068): `--bridge`
    /// is required (cobra `MarkFlagRequired`, enforced BEFORE the provider call),
    /// the data provider is the always-configured DefiLlama client, and the
    /// provider's `BridgeDetails` (which itself runs the auth gate) is invoked
    /// inside [`crate::runner::run_cached_command`] (60s TTL).
    async fn handle_details(ctx: &AppCtx, args: DetailsArgs) -> Result<Envelope, Error> {
        use defi_model::ProviderStatus;
        use defi_providers::{BridgeDataProvider, BridgeDetailsRequest};

        const PROVIDER_NAME: &str = "defillama";

        // `--bridge` required (cobra MarkFlagRequired). Enforced before the
        // provider/auth check so a missing flag is a usage error (exit 2).
        let bridge = args.bridge.clone().unwrap_or_default();
        if bridge.trim().is_empty() {
            return Err(Error::new(defi_errors::Code::Usage, "--bridge is required"));
        }

        super::ensure_bridge_data_provider(true)?;

        let req = BridgeDetailsRequest {
            bridge,
            include_chain_breakdown: args.include_chain_breakdown,
        };
        let path = "bridge details";
        let key = crate::protocols::cache_key(
            path,
            &super::BridgeDetailsCacheKey {
                bridge: &req.bridge.trim().to_ascii_lowercase(),
                include_chain_breakdown: req.include_chain_breakdown,
                provider: PROVIDER_NAME,
            },
        );
        let ttl = std::time::Duration::from_secs(super::BRIDGE_DATA_TTL_SECS);
        let provider = ctx.defillama();
        let req_for_fetch = req.clone();

        ctx.run_cached_command(path, &key, ttl, || {
            let res = crate::ctx::block_on_fetch(provider.bridge_details(req_for_fetch));
            let status = ProviderStatus {
                name: PROVIDER_NAME.to_string(),
                status: super::status_from_result(&res),
                latency_ms: 0,
            };
            match res {
                Ok(details) => match serde_json::to_value(&details) {
                    Ok(data) => Ok(crate::runner::FetchOutcome {
                        data,
                        providers: vec![status],
                        warnings: Vec::new(),
                        partial: false,
                    }),
                    Err(e) => {
                        let err =
                            Error::wrap(defi_errors::Code::Internal, "serialize bridge details", e);
                        let st = ProviderStatus {
                            name: PROVIDER_NAME.to_string(),
                            status: "error".to_string(),
                            latency_ms: 0,
                        };
                        Err((vec![st], Vec::new(), false, err))
                    }
                },
                Err(err) => Err((vec![status], Vec::new(), false, err)),
            }
        })
    }

    /// Merge structured input (`--input-json` / `--input-file`) onto the resolved
    /// `bridge quote` flag values (Go `applyStructuredFlagInput`).
    ///
    /// Reads the payload (mutually-exclusive `--input-json` / `--input-file`;
    /// `-` reads stdin), parses it as a JSON object, and applies each entry
    /// unless the flag was explicitly set on the command line. A non-object
    /// payload, unknown key, or `null` value is a usage error.
    fn apply_quote_structured_input(
        input: &crate::execflags::InputFlags,
        explicit: &std::collections::HashSet<&str>,
        values: &mut QuoteValues,
    ) -> Result<(), Error> {
        use defi_errors::Code;

        let payload = read_structured_input(input)?;
        let payload = match payload {
            Some(p) if !p.trim().is_empty() => p,
            _ => return Ok(()),
        };

        let parsed: serde_json::Value = serde_json::from_str(&payload)
            .map_err(|e| Error::wrap(Code::Usage, "parse structured input", e))?;
        let obj = parsed
            .as_object()
            .ok_or_else(|| Error::new(Code::Usage, "structured input must be a JSON object"))?;

        let as_string = |v: &serde_json::Value| -> Option<String> {
            match v {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                serde_json::Value::Bool(b) => Some(b.to_string()),
                _ => None,
            }
        };

        for (key, raw) in obj {
            let canonical = key.replace('_', "-");
            if explicit.contains(canonical.as_str()) {
                continue;
            }
            if raw.is_null() {
                return Err(Error::new(
                    Code::Usage,
                    format!("structured input field {key:?} cannot be null"),
                ));
            }
            match canonical.as_str() {
                "provider" => values.provider = as_string(raw).unwrap_or_default(),
                "from" => values.from = as_string(raw).unwrap_or_default(),
                "to" => values.to = as_string(raw).unwrap_or_default(),
                "asset" => values.asset = as_string(raw).unwrap_or_default(),
                "to-asset" => values.to_asset = as_string(raw).unwrap_or_default(),
                "amount" => values.amount = as_string(raw).unwrap_or_default(),
                "amount-decimal" => values.amount_decimal = as_string(raw).unwrap_or_default(),
                "from-amount-for-gas" => {
                    values.from_amount_for_gas = as_string(raw).unwrap_or_default()
                }
                _ => {
                    return Err(Error::new(
                        Code::Usage,
                        format!("structured input field {key:?} is not supported by bridge quote"),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Resolve the structured-input payload string from `--input-json` /
    /// `--input-file` (`-` = stdin), enforcing mutual exclusivity (Go
    /// `readStructuredInput`).
    fn read_structured_input(
        input: &crate::execflags::InputFlags,
    ) -> Result<Option<String>, Error> {
        use defi_errors::Code;

        let json = input.input_json.as_deref().unwrap_or("").trim().to_string();
        let file = input.input_file.as_deref().unwrap_or("").trim().to_string();
        if !json.is_empty() && !file.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "use only one of --input-json or --input-file",
            ));
        }
        if !json.is_empty() {
            return Ok(Some(json));
        }
        if file.is_empty() {
            return Ok(None);
        }
        if file == "-" {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| Error::wrap(Code::Usage, "read structured input from stdin", e))?;
            return Ok(Some(buf));
        }
        let buf = std::fs::read_to_string(&file)
            .map_err(|e| Error::wrap(Code::Usage, "read structured input file", e))?;
        Ok(Some(buf))
    }
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

#[cfg(test)]
mod app_tests {
    //! # Success criteria — app-level `bridge quote|list|details` (WS2, wiremock)
    //!
    //! These tests exercise the **wired bridge-reads command-group handler**
    //! ([`cli::handle`] → `bridge quote` / `bridge list` / `bridge details`)
    //! end-to-end against `wiremock` servers, via the [`AppCtx`] base-URL seams:
    //!
    //!  * `bridge quote` reaches the cross-chain quote providers (Across / LiFi /
    //!    Bungee) through [`AppCtx::with_bridge_base`] (analogous to
    //!    [`AppCtx::with_swap_base`]); each adapter exposes a `set_base_url` seam.
    //!  * `bridge list` / `bridge details` reach the DefiLlama bridge-analytics
    //!    provider through the EXISTING [`AppCtx::with_defillama_base`] seam (which
    //!    already applies `set_bridge_base_url`), and are key-gated on a non-empty
    //!    `DEFI_DEFILLAMA_API_KEY`.
    //!
    //! The sibling `tests` module already covers the pure helpers
    //! (`build_bridge_request`, `resolve_bridge_quote_provider`,
    //! `ensure_bridge_data_provider`, `bridge_plan_identity_constraints`,
    //! `ensure_bridge_intent`); THIS module asserts the WIRED HANDLER's full
    //! machine contract. These are LIVE commands in Go (Across/LiFi/DefiLlama hit
    //! real APIs), so per spec §4.1 / completion plan WS2 they are NOT byte-diffed
    //! against the Go binary; instead the handler is driven offline against a
    //! mock through the base-URL seams the GREEN handler MUST honor. Provider
    //! adapter response BODIES (per-field quote/volume math) are owned by
    //! `defi-providers` and are NOT re-asserted here — only that the handler
    //! surfaces the adapter result into the envelope.
    //!
    //! Each criterion maps to a behavior in the Go `newBridgeCommand` closures
    //! (`runner.go` ~L904-1088):
    //!
    //!  ## bridge quote
    //!  Q1. **Success envelope shape (Across / EVM).** With a mock Across API and
    //!      `bridge quote --provider across --from 1 --to 10 --asset USDC --amount
    //!      1000000`, the resolved [`Envelope`] has `version="v1"`, `success=true`,
    //!      `error=None`, `meta.command="bridge quote"`, `meta.partial=false`, and
    //!      `data` is the BridgeQuote with `provider="across"`,
    //!      `from_chain_id="eip155:1"`, `to_chain_id="eip155:10"`, and
    //!      `input_amount.amount_base_units="1000000"` (base+decimal consistency,
    //!      spec §2.4). The mock MUST be contacted (proving the seam is honored).
    //!      (Go: `provider.QuoteBridge(reqStruct)` → envelope.)
    //!  Q2. **`meta.providers[]` status row.** Exactly one provider status keyed on
    //!      the adapter's `Info().Name` (`"across"`) with status `"ok"`.
    //!      (Go: `statusFromErr(nil) == "ok"`.)
    //!  Q3. **Cache transition write → fresh hit; disabled → miss.** `bridge quote`
    //!      is a cached read path (Go `runCachedCommand(..., 15*time.Second, ...)`).
    //!      With caching enabled the first call writes (`status="write"`) and a
    //!      second identical call is a fresh `"hit"` that short-circuits the
    //!      provider (`meta.providers` empty, mock received exactly one request).
    //!      With caching disabled the status stays `"miss"`.
    //!  Q4. **`--provider` required (multi-provider, spec §2.5).** A missing
    //!      `--provider` is a usage error (exit 2) BEFORE any chain/asset parse
    //!      (Go: empty provider → `CodeUsage` `--provider is required`). Asserted
    //!      via the full binary (`run_with_args`) so the parse/guard ordering is
    //!      exercised end-to-end.
    //!  Q5. **Unknown provider → unsupported (exit 13).** `--provider bogus` is a
    //!      [`Code::Unsupported`] error with the Go-semantic message
    //!      `unsupported bridge provider` (Go: not in `s.bridgeProviders`).
    //!  Q6. **LiFi `--from-amount-for-gas` carried.** With a mock LiFi API and
    //!      `--provider lifi --from-amount-for-gas 100000`, the handler forwards
    //!      the reserve amount to the adapter (the mock matches
    //!      `fromAmountForGas=100000`) and the BridgeQuote `from_amount_for_gas`
    //!      echoes `100000`. (Go: `FromAmountForGas` carried into the request.)
    //!  Q7. **`--input-json` precedence.** An explicit `--provider across` flag
    //!      OVERRIDES a JSON `"provider":"bogus"` (Go `applyStructuredFlagInput`
    //!      only fills flags the user did not set): the request reaches the Across
    //!      mock and succeeds rather than failing unsupported (exit 13).
    //!
    //!  ## bridge list
    //!  L1. **Success envelope + provider status.** With `DEFI_DEFILLAMA_API_KEY`
    //!      set and a mock DefiLlama bridges endpoint, `bridge list` returns
    //!      `success=true`, `meta.command="bridge list"`, `data` is the JSON array
    //!      of BridgeSummary rows, and one `"ok"` `"defillama"` provider status.
    //!  L2. **Key-gating (auth, exit 10).** With NO DefiLlama key the DefiLlama
    //!      adapter's `require_bridge_api_key` fails with [`Code::Auth`] (exit 10).
    //!      Asserted via the full binary (`run_with_args`, no key env).
    //!  L3. **`--provider` rejected (unknown flag, exit 2).** `bridge list` has no
    //!      `--provider` flag (it hardcodes DefiLlama); `--provider x` is a clap
    //!      unknown-argument usage error (exit 2). (Go
    //!      `TestRunnerBridgeListRejectsProviderFlag`.)
    //!  L4. **Cache transition.** `bridge list` is a cached read path (Go
    //!      `60*time.Second` TTL): write then fresh hit with exactly one provider
    //!      request.
    //!
    //!  ## bridge details
    //!  D1. **Success envelope with chain breakdown.** With a key + mock DefiLlama
    //!      (bridges resolve + `/bridge/{id}`), `bridge details --bridge layerzero`
    //!      returns `success=true`, `meta.command="bridge details"`, and `data` is
    //!      the BridgeDetails object (`name`, `chain_breakdown`).
    //!  D2. **Key-gating (auth, exit 10).** No key → [`Code::Auth`] (exit 10).
    //!  D3. **`--bridge` required (usage, exit 2).** `bridge details` with no
    //!      `--bridge` is a usage error (exit 2). (Go cobra `MarkFlagRequired` /
    //!      `TestRunnerBridgeDetailsRequiresBridgeFlag`.)
    //!
    //! SKIPPED (owned elsewhere / wrong layer): per-field BridgeQuote/BridgeSummary/
    //! BridgeDetails math (defi-providers, wiremock-tested there), the cache-key
    //! byte composition + cache-flow state machine internals (runner), the
    //! `bridge plan|submit|status` paths (WS3/WS4), and JSON field-declaration-order
    //! rendering (defi-out golden tests).

    use super::cli::{handle, BridgeCmd, DetailsArgs, ListArgs, QuoteArgs};
    use crate::cli::run_with_args;
    use crate::ctx::AppCtx;
    use defi_config::{MapEnv, Settings};
    use defi_errors::{exit_code, Code, Error};
    use defi_model::Envelope;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ---- settings + env helpers ------------------------------------------

    /// JSON-output settings with caching toggled by `cache_enabled` and the
    /// DefiLlama key threaded explicitly (so the key-gated list/details success
    /// path can pass the adapter key check). Cache/action paths point at `tmp`.
    fn settings_in(tmp: &Path, cache_enabled: bool, defillama_key: &str) -> Settings {
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
            cache_enabled,
            cache_path: tmp.join("cache.sqlite"),
            cache_lock_path: tmp.join("cache.lock"),
            action_store_path: tmp.join("actions.sqlite"),
            action_lock_path: tmp.join("actions.lock"),
            defillama_api_key: defillama_key.to_string(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    /// A `MapEnv` whose HOME points at a temp dir (so `Settings::load` resolves
    /// cache/config paths without touching the real home). Keeps the `TempDir`
    /// guard alive for the test's duration.
    fn env_with_home() -> (MapEnv, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let env = MapEnv::with_home(tmp.path().to_path_buf());
        (env, tmp)
    }

    fn data_obj(env: &Envelope) -> serde_json::Map<String, Value> {
        env.data
            .as_ref()
            .and_then(Value::as_object)
            .cloned()
            .expect("data is an object")
    }

    fn data_array(env: &Envelope) -> Vec<Value> {
        env.data
            .as_ref()
            .and_then(Value::as_array)
            .cloned()
            .expect("data is an array")
    }

    // ---- bridge quote mocks ----------------------------------------------

    /// `bridge quote --provider across --from 1 --to 10 --asset USDC --amount
    /// 1000000` flag set (the canonical EVM happy path).
    fn across_quote_args() -> QuoteArgs {
        QuoteArgs {
            from: Some("1".to_string()),
            to: Some("10".to_string()),
            asset: Some("USDC".to_string()),
            to_asset: None,
            provider: Some("across".to_string()),
            amount: Some("1000000".to_string()),
            amount_decimal: None,
            from_amount_for_gas: None,
            input: crate::execflags::InputFlags::default(),
        }
    }

    /// Mount the Across `limits` + `suggested-fees` quote routes on a fresh
    /// `MockServer` (the adapter targets `{base}/limits` and
    /// `{base}/suggested-fees`). `.expect(n)` on each verifies the request count.
    async fn across_mock() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/limits"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"minDeposit":"1","maxDeposit":"1954894537806"}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/suggested-fees"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "relayFeeTotal":"2633",
                    "relayGasFeeTotal":"2533",
                    "capitalFeeTotal":"100",
                    "lpFee":{"total":"0"},
                    "outputAmount":"997367",
                    "estimatedFillTimeSec":5
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        server
    }

    // ---- Q1: bridge quote success envelope (Across / EVM) -----------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_success_envelope_across() {
        let server = across_mock().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "")).with_bridge_base(&server.uri());

        let env = handle(&ctx, BridgeCmd::Quote(across_quote_args()))
            .await
            .expect("bridge quote should succeed against the mock Across API");

        // The wired handler MUST have contacted the mock (proves the seam is
        // honored) — keeps the test offline + deterministic.
        assert!(
            !server
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty(),
            "handler must reach the injected Across mock, not the live API"
        );

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert_eq!(env.meta.command, "bridge quote");
        assert!(!env.meta.partial);

        let data = data_obj(&env);
        assert_eq!(data["provider"], Value::from("across"));
        assert_eq!(data["from_chain_id"], Value::from("eip155:1"));
        assert_eq!(data["to_chain_id"], Value::from("eip155:10"));
        // Input amount echoed (base+decimal consistency, spec §2.4).
        assert_eq!(
            data["input_amount"]["amount_base_units"],
            Value::from("1000000")
        );
        // Adapter result is surfaced into the envelope (estimated_out present).
        assert!(
            data["estimated_out"]["amount_base_units"]
                .as_str()
                .is_some(),
            "estimated_out must be surfaced from the adapter: {data:?}"
        );
    }

    // ---- Q2: meta.providers[] status row ----------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_success_provider_status_ok() {
        let server = across_mock().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "")).with_bridge_base(&server.uri());

        let env = handle(&ctx, BridgeCmd::Quote(across_quote_args()))
            .await
            .expect("bridge quote success");

        assert_eq!(
            env.meta.providers.len(),
            1,
            "exactly one provider status row"
        );
        assert_eq!(env.meta.providers[0].name, "across");
        assert_eq!(env.meta.providers[0].status, "ok");
    }

    // ---- Q3: cache transitions --------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_cache_write_then_hit() {
        let server = MockServer::start().await;
        // Across issues two GETs per quote (limits + suggested-fees); across two
        // invocations a fresh hit must short-circuit the second, so each route is
        // expected EXACTLY once.
        Mock::given(method("GET"))
            .and(path("/limits"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"minDeposit":"1","maxDeposit":"1954894537806"}"#,
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/suggested-fees"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"relayFeeTotal":"2633","outputAmount":"997367","estimatedFillTimeSec":5}"#,
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), true, "")).with_bridge_base(&server.uri());

        let first = handle(&ctx, BridgeCmd::Quote(across_quote_args()))
            .await
            .expect("first bridge quote");
        assert_eq!(
            first.meta.cache.status, "write",
            "first cache-enabled fetch should write the cache"
        );
        assert!(!first.meta.cache.stale);

        let second = handle(&ctx, BridgeCmd::Quote(across_quote_args()))
            .await
            .expect("second bridge quote");
        assert_eq!(
            second.meta.cache.status, "hit",
            "second identical fetch should hit the cache"
        );
        assert!(!second.meta.cache.stale);
        assert!(
            second.meta.providers.is_empty(),
            "a fresh hit must short-circuit the provider"
        );

        // Mock's expect(1) per route verifies exactly one provider fetch on drop.
        drop(server);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_cache_disabled_status_miss() {
        let server = across_mock().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "")).with_bridge_base(&server.uri());

        let env = handle(&ctx, BridgeCmd::Quote(across_quote_args()))
            .await
            .expect("bridge quote");
        assert_eq!(
            env.meta.cache.status, "miss",
            "cache-disabled fetch keeps the initial miss status"
        );
    }

    // ---- Q4: --provider required (spec §2.5), full binary -----------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_missing_provider_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi", "bridge", "quote", "--from", "1", "--to", "10", "--asset", "USDC",
                "--amount", "1000000",
            ],
            &env,
        )
        .await;
        assert_eq!(code, 2, "missing --provider must be a usage error (exit 2)");
    }

    // ---- Q5: unknown provider -> unsupported (exit 13) --------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_unknown_provider_is_unsupported_exit_13() {
        // Asserted via `handle` so the SPECIFIC Go message is checked (the stub
        // also returns exit 13, so the message guards against a false pass).
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, ""));
        let mut args = across_quote_args();
        args.provider = Some("bogus".to_string());

        let err = handle(&ctx, BridgeCmd::Quote(args))
            .await
            .expect_err("unknown provider must fail");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string().contains("unsupported bridge provider"),
            "expected the Go-semantic 'unsupported bridge provider' message, got: {err}"
        );
    }

    // ---- Q6: LiFi --from-amount-for-gas carried ---------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_lifi_carries_from_amount_for_gas() {
        let server = MockServer::start().await;
        let body = r#"{
            "estimate": {
                "toAmount": "900000",
                "toAmountMin": "890000",
                "approvalAddress": "0x0000000000000000000000000000000000000ABC",
                "feeCosts": [{"amountUSD":"0.40"}],
                "gasCosts": [{"amountUSD":"0.60"}],
                "executionDuration": 45
            },
            "toolDetails": {"key":"across","name":"across"},
            "tool": "across",
            "includedSteps": [{
                "action": {
                    "toChainId": 10,
                    "toToken": {"address":"0x0000000000000000000000000000000000000000","decimals":18}
                },
                "estimate": {"toAmount":"500000000000000"}
            }],
            "transactionRequest": {
                "to": "0x1231DeB6f5749EF6Ce6943a275A1D3E7486F4EaE",
                "from": "0x00000000000000000000000000000000000000AA",
                "data": "0x1234",
                "value": "0x0",
                "chainId": 1
            }
        }"#;
        // The mock ONLY matches when fromAmountForGas=100000 is forwarded — so a
        // handler that drops the reserve amount never reaches a 200 (the test
        // fails), pinning the carry-through.
        Mock::given(method("GET"))
            .and(path("/quote"))
            .and(query_param("fromAmountForGas", "100000"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "")).with_bridge_base(&server.uri());

        let mut args = across_quote_args();
        args.provider = Some("lifi".to_string());
        args.from_amount_for_gas = Some("100000".to_string());

        let env = handle(&ctx, BridgeCmd::Quote(args))
            .await
            .expect("lifi bridge quote with from-amount-for-gas should succeed");

        assert_eq!(env.meta.command, "bridge quote");
        assert!(env.success);
        let data = data_obj(&env);
        assert_eq!(data["provider"], Value::from("lifi"));
        assert_eq!(
            data["from_amount_for_gas"],
            Value::from("100000"),
            "the reserve amount must be carried into the BridgeQuote"
        );
    }

    // ---- Q7: --input-json precedence (explicit flag overrides JSON) -------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_explicit_provider_overrides_input_json() {
        // The JSON sets provider="bogus" (which would be exit 13), but the
        // explicit --provider across flag must win (Go applyStructuredFlagInput
        // only fills flags the user did not set). With the mock base, the request
        // reaches the Across mock and succeeds, proving the override.
        let server = across_mock().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "")).with_bridge_base(&server.uri());

        let mut args = across_quote_args();
        args.provider = Some("across".to_string());
        args.input = crate::execflags::InputFlags {
            input_json: Some(
                r#"{"provider":"bogus","from":"1","to":"10","asset":"USDC","amount":"1000000"}"#
                    .to_string(),
            ),
            input_file: None,
        };

        let env = handle(&ctx, BridgeCmd::Quote(args))
            .await
            .expect("explicit --provider across must override the JSON provider");
        assert!(env.success);
        assert_eq!(data_obj(&env)["provider"], Value::from("across"));
    }

    // ---- bridge list mocks ------------------------------------------------

    fn list_args() -> ListArgs {
        ListArgs {
            include_chains: true,
            limit: 20,
        }
    }

    /// Mount the DefiLlama bridges list route. With api_key="test-key" the
    /// adapter targets `{base}/test-key/bridges/bridges`.
    async fn defillama_bridges_mock(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/test-key/bridges/bridges"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "bridges":[
                        {"id":1,"name":"b","displayName":"Bridge B","slug":"bridge-b","last24hVolume":150,"weeklyVolume":1000,"monthlyVolume":5000,"chains":["Base","Ethereum"]},
                        {"id":2,"name":"a","displayName":"Bridge A","slug":"bridge-a","last24hVolume":250,"weeklyVolume":900,"monthlyVolume":6000,"chains":["Ethereum","Base"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(server)
            .await;
    }

    // ---- L1: bridge list success envelope + provider status ---------------

    #[tokio::test(flavor = "multi_thread")]
    async fn list_success_envelope() {
        let server = MockServer::start().await;
        defillama_bridges_mock(&server).await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "test-key"))
            .with_defillama_base(&server.uri());

        let env = handle(&ctx, BridgeCmd::List(list_args()))
            .await
            .expect("bridge list should succeed against the mock DefiLlama API");

        assert!(
            !server
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty(),
            "handler must reach the injected DefiLlama mock"
        );
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert_eq!(env.meta.command, "bridge list");

        let rows = data_array(&env);
        assert_eq!(rows.len(), 2, "both mock bridges surface in data");
        // Sorted by 24h volume desc: id 2 (250) before id 1 (150).
        assert_eq!(rows[0]["bridge_id"], Value::from(2));

        assert_eq!(env.meta.providers.len(), 1);
        assert_eq!(env.meta.providers[0].name, "defillama");
        assert_eq!(env.meta.providers[0].status, "ok");
    }

    // ---- L2: key-gating (auth, exit 10) -----------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn list_missing_key_is_auth_exit_10_handle() {
        // No DefiLlama key -> the adapter's require_bridge_api_key fails (Auth).
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, ""));

        let err = handle(&ctx, BridgeCmd::List(list_args()))
            .await
            .expect_err("missing DefiLlama key must fail bridge list");
        assert_eq!(err.code, Code::Auth);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 10);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_missing_key_full_binary_exit_10() {
        // Full-binary: no DEFI_DEFILLAMA_API_KEY env -> auth (exit 10).
        let (env, _home) = env_with_home();
        let code = run_with_args(["defi", "bridge", "list"], &env).await;
        assert_eq!(
            code, 10,
            "bridge list without a DefiLlama key must be an auth error (exit 10)"
        );
    }

    // ---- L3: --provider rejected (unknown flag, exit 2) -------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn list_rejects_provider_flag_exit_2() {
        // `bridge list` has no --provider flag (it hardcodes DefiLlama); clap
        // rejects the unknown argument as a usage error (Go
        // TestRunnerBridgeListRejectsProviderFlag).
        let (env, _home) = env_with_home();
        let code = run_with_args(["defi", "bridge", "list", "--provider", "unknown"], &env).await;
        assert_eq!(
            code, 2,
            "an unknown --provider flag on bridge list must be a usage error (exit 2)"
        );
    }

    // ---- L4: cache write then hit -----------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn list_cache_write_then_hit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/test-key/bridges/bridges"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"bridges":[{"id":1,"name":"b","displayName":"B","slug":"b","last24hVolume":150}]}"#,
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), true, "test-key"))
            .with_defillama_base(&server.uri());

        let first = handle(&ctx, BridgeCmd::List(list_args()))
            .await
            .expect("first bridge list");
        assert_eq!(first.meta.cache.status, "write");

        let second = handle(&ctx, BridgeCmd::List(list_args()))
            .await
            .expect("second bridge list");
        assert_eq!(second.meta.cache.status, "hit");
        assert!(second.meta.providers.is_empty());
        drop(server);
    }

    // ---- bridge details mocks ---------------------------------------------

    fn details_args(bridge: &str) -> DetailsArgs {
        DetailsArgs {
            bridge: Some(bridge.to_string()),
            include_chain_breakdown: true,
        }
    }

    /// Mount the DefiLlama bridges resolve route + the `/bridge/{id}` detail
    /// route (api_key="test-key"; "layerzero" resolves to id 84).
    async fn defillama_details_mock(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/test-key/bridges/bridges"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"bridges":[{"id":84,"name":"layerzero","displayName":"LayerZero","slug":"layerzero"}]}"#,
                "application/json",
            ))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/test-key/bridges/bridge/84"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "id":84,
                    "name":"layerzero",
                    "displayName":"LayerZero",
                    "last24hVolume":123.45,
                    "chainBreakdown":{
                        "Base":{"last24hVolume":80},
                        "Arbitrum":{"last24hVolume":40}
                    }
                }"#,
                "application/json",
            ))
            .mount(server)
            .await;
    }

    // ---- D1: bridge details success envelope with chain breakdown ---------

    #[tokio::test(flavor = "multi_thread")]
    async fn details_success_envelope() {
        let server = MockServer::start().await;
        defillama_details_mock(&server).await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "test-key"))
            .with_defillama_base(&server.uri());

        let env = handle(&ctx, BridgeCmd::Details(details_args("layerzero")))
            .await
            .expect("bridge details should succeed against the mock DefiLlama API");

        assert!(!server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert_eq!(env.meta.command, "bridge details");

        let data = data_obj(&env);
        assert_eq!(data["bridge_id"], Value::from(84));
        assert_eq!(data["name"], Value::from("layerzero"));
        let breakdown = data["chain_breakdown"]
            .as_array()
            .expect("chain_breakdown array");
        assert_eq!(breakdown.len(), 2);
        // Highest-volume chain first: Base (80) > Arbitrum (40).
        assert_eq!(breakdown[0]["chain"], Value::from("Base"));

        assert_eq!(env.meta.providers.len(), 1);
        assert_eq!(env.meta.providers[0].name, "defillama");
        assert_eq!(env.meta.providers[0].status, "ok");
    }

    // ---- D2: key-gating (auth, exit 10) -----------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn details_missing_key_is_auth_exit_10_handle() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, ""));

        let err = handle(&ctx, BridgeCmd::Details(details_args("layerzero")))
            .await
            .expect_err("missing DefiLlama key must fail bridge details");
        assert_eq!(err.code, Code::Auth);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 10);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn details_missing_key_full_binary_exit_10() {
        let (env, _home) = env_with_home();
        let code =
            run_with_args(["defi", "bridge", "details", "--bridge", "layerzero"], &env).await;
        assert_eq!(
            code, 10,
            "bridge details without a DefiLlama key must be an auth error (exit 10)"
        );
    }

    // ---- D3: --bridge required (usage, exit 2) ----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn details_requires_bridge_flag_exit_2() {
        // `bridge details` with no --bridge is a usage error (Go cobra
        // MarkFlagRequired / TestRunnerBridgeDetailsRequiresBridgeFlag). The
        // GREEN handler must enforce this BEFORE the auth/key check.
        let (env, _home) = env_with_home();
        let code = run_with_args(["defi", "bridge", "details"], &env).await;
        assert_eq!(
            code, 2,
            "bridge details without --bridge must be a usage error (exit 2)"
        );
    }

    // ---- flag parsing: defaults + forwarding ------------------------------

    #[test]
    fn bridge_quote_flags_parse() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "defi",
            "bridge",
            "quote",
            "--provider",
            "lifi",
            "--from",
            "1",
            "--to",
            "10",
            "--asset",
            "USDC",
            "--amount",
            "1000000",
            "--from-amount-for-gas",
            "100000",
        ])
        .expect("bridge quote flags parse");
        if let crate::cli::TopCommand::Bridge {
            cmd: BridgeCmd::Quote(args),
        } = cli.command
        {
            assert_eq!(args.provider.as_deref(), Some("lifi"));
            assert_eq!(args.from.as_deref(), Some("1"));
            assert_eq!(args.to.as_deref(), Some("10"));
            assert_eq!(args.asset.as_deref(), Some("USDC"));
            assert_eq!(args.amount.as_deref(), Some("1000000"));
            assert_eq!(args.from_amount_for_gas.as_deref(), Some("100000"));
        } else {
            panic!("expected bridge quote");
        }
    }

    #[test]
    fn bridge_list_flags_default_and_parse() {
        use clap::Parser;
        // Defaults: --limit 20, --include-chains true.
        let cli = crate::cli::Cli::try_parse_from(["defi", "bridge", "list"])
            .expect("bridge list parses");
        if let crate::cli::TopCommand::Bridge {
            cmd: BridgeCmd::List(args),
        } = cli.command
        {
            assert_eq!(args.limit, 20);
            assert!(args.include_chains);
        } else {
            panic!("expected bridge list");
        }

        // Explicit overrides parse.
        let cli = crate::cli::Cli::try_parse_from([
            "defi",
            "bridge",
            "list",
            "--limit",
            "5",
            "--include-chains",
            "false",
        ])
        .expect("bridge list overrides parse");
        if let crate::cli::TopCommand::Bridge {
            cmd: BridgeCmd::List(args),
        } = cli.command
        {
            assert_eq!(args.limit, 5);
            assert!(!args.include_chains);
        } else {
            panic!("expected bridge list");
        }
    }

    #[test]
    fn bridge_details_flags_parse() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "defi",
            "bridge",
            "details",
            "--bridge",
            "layerzero",
            "--include-chain-breakdown",
            "false",
        ])
        .expect("bridge details flags parse");
        if let crate::cli::TopCommand::Bridge {
            cmd: BridgeCmd::Details(args),
        } = cli.command
        {
            assert_eq!(args.bridge.as_deref(), Some("layerzero"));
            assert!(!args.include_chain_breakdown);
        } else {
            panic!("expected bridge details");
        }
    }
}
