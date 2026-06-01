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
            BridgeCmd::Plan(args) => handle_plan(ctx, args).await,
            BridgeCmd::Submit(args) => handle_submit(ctx, args).await,
            BridgeCmd::Status(args) => handle_status(ctx, args).await,
        }
    }

    /// Handle `bridge submit` (Go `submitCmd.RunE`,
    /// `bridge_execution_commands.go` ~L163-215).
    ///
    /// `bridge submit` is the **standard-EVM** execution submit: an Across / LiFi
    /// bridge action is an EVM `legacy_local` / `ows` action (there is NO Tempo
    /// bridge path, unlike `swap submit`). Flow parity with the Go runner:
    /// 1. resolve + validate `--action-id` ([`crate::actions::resolve_action_id`]);
    /// 2. load the persisted action (not-found → usage `load action`);
    /// 3. gate the intent (`bridge`-only — [`super::ensure_bridge_intent`]);
    /// 4. short-circuit an already-`completed` action (success + warning, no
    ///    re-broadcast);
    /// 5. resolve the execution backend + signer
    ///    ([`crate::execsubmit::resolve_action_execution_backend`]: legacy-local /
    ///    OWS guards);
    /// 6. validate the resolved signer vs `--from-address` + the planned sender;
    /// 7. parse the execute options (`--gas-multiplier > 1`, durations, fee flags,
    ///    the `--allow-max-approval` / `--unsafe-provider-tx` guardrail opt-ins —
    ///    bridge submit carries these, like `approvals submit`);
    /// 8. run the bounded-approval pre-sign guardrail with the action context;
    /// 9. broadcast through the engine ([`crate::execsubmit::execute_resolved`]) —
    ///    which, for a `bridge_send` step, waits for destination settlement (Across
    ///    `/deposit/status`, LiFi `/status`) before marking the step confirmed —
    ///    persisting each transition, then emit the terminal-state envelope (cache
    ///    bypassed for execution paths, spec §2.5).
    ///
    /// On every guard/build error the typed [`Error`] is returned (the runner
    /// renders the full error envelope to stderr) and the persisted action is left
    /// in its pre-submit state.
    async fn handle_submit(ctx: &AppCtx, args: SubmitArgs) -> Result<Envelope, Error> {
        use defi_errors::Code;
        use defi_model::ProviderStatus;

        // 1. Resolve + validate the action id.
        let action_id =
            crate::actions::resolve_action_id(args.action_id.as_deref().unwrap_or_default())?;

        // 2. Load the persisted action (not-found → usage `load action`).
        let store = ctx.open_action_store()?;
        let mut action = store
            .get(&action_id)
            .map_err(|e| Error::wrap(Code::Usage, "load action", e))?;

        // 3. Intent gate (bridge-only).
        super::ensure_bridge_intent(&action.intent_type)?;

        // 4. Already-completed short-circuit (no re-broadcast).
        if action.status == defi_execution::action::ActionStatus::Completed {
            let data = serde_json::to_value(&action)
                .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
            let mut env =
                ctx.metadata_envelope("bridge submit", data, Vec::<ProviderStatus>::new());
            env.warnings = vec!["action already completed".to_string()];
            return Ok(env);
        }

        // 5. Resolve the execution backend + signer (legacy-local / OWS guards).
        //    There is NO Tempo bridge branch (bridge planning is OWS-first
        //    standard-EVM only).
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

        // 7. Parse the execute options (durations, gas multiplier, fee flags,
        //    approval/provider-tx guardrail opt-ins).
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

        // 8. Bounded-approval pre-sign guardrail (with action context).
        crate::execsubmit::presign_validate_action(&action, &opts)?;

        // 9. Broadcast through the engine (persisting each transition, incl. the
        //    bridge destination-settlement wait), then emit the terminal-state
        //    envelope (cache bypassed for execution paths).
        crate::execsubmit::execute_resolved(&store, &mut action, resolved, opts).await?;

        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
        Ok(ctx.metadata_envelope("bridge submit", data, Vec::<ProviderStatus>::new()))
    }

    /// Handle `bridge status` (Go `statusCmd.RunE`,
    /// `bridge_execution_commands.go` ~L233-254).
    ///
    /// A pure read over the persisted action store: resolve + validate the
    /// `--action-id`, load the action (not-found → usage `load action`), gate the
    /// intent (`bridge`-only — [`super::ensure_bridge_intent`]), and emit the
    /// action verbatim (cache bypassed, spec §2.5). Backend-agnostic — `bridge
    /// status` never signs.
    async fn handle_status(ctx: &AppCtx, args: StatusArgs) -> Result<Envelope, Error> {
        use defi_errors::Code;
        use defi_model::ProviderStatus;

        let action_id =
            crate::actions::resolve_action_id(args.action_id.as_deref().unwrap_or_default())?;
        let store = ctx.open_action_store()?;
        let action = store
            .get(&action_id)
            .map_err(|e| Error::wrap(Code::Usage, "load action", e))?;
        super::ensure_bridge_intent(&action.intent_type)?;
        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
        Ok(ctx.metadata_envelope("bridge status", data, Vec::<ProviderStatus>::new()))
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

    /// Resolved `bridge plan` flag values after merging structured input.
    struct PlanValues {
        provider: String,
        from: String,
        to: String,
        asset: String,
        to_asset: String,
        amount: String,
        amount_decimal: String,
        from_amount_for_gas: String,
        wallet: String,
        from_address: String,
        recipient: String,
        slippage_bps: i64,
        simulate: bool,
        rpc_url: String,
    }

    /// Handle `bridge plan` (Go `planCmd.RunE`, `bridge_execution_commands.go`
    /// ~L97-138).
    ///
    /// Capability-based bridge planning (Across / LiFi). Flow parity with the Go
    /// runner:
    /// 1. merge structured input (`--input-json` / `--input-file`; explicit flags
    ///    win, Go `applyStructuredFlagInput`);
    /// 2. `--provider` required (empty → usage, BEFORE anything else);
    /// 3. resolve the execution identity ([`resolve_execution_identity`]:
    ///    `exactly_one_of {wallet, from_address}`; the chain arg is the SOURCE
    ///    chain — Go passes `plan.FromArg`). Errors return before any build/persist;
    /// 4. build the canonical [`BridgeQuoteRequest`] ([`super::build_bridge_request`]:
    ///    chain/asset parse, `--to-asset` inference, amount normalization,
    ///    `from_amount_for_gas` carry);
    /// 5. route the build through the populated action-build registry
    ///    ([`Registry::build_bridge_action`] → the `across`/`lifi`
    ///    [`BridgeActionBuilder`]; an unknown provider errors `unsupported bridge
    ///    provider`, a quote-only provider (bungee) errors `quote-only`), capturing
    ///    a single [`ProviderStatus`] keyed on the builder display name (Go
    ///    `provider.Info().Name`), falling back to the normalized provider name;
    /// 6. stamp the identity onto the action ([`apply_execution_identity_to_action`]),
    ///    persist to the action [`Store`], and emit the success envelope (cache
    ///    bypassed for execution paths, spec §2.5) carrying the identity warnings.
    ///
    /// On every guard/build error the typed [`Error`] is returned (the runner
    /// renders the full error envelope to stderr) and NOTHING is persisted.
    ///
    /// [`Registry`]: defi_execution::builder::Registry
    /// [`Store`]: defi_execution::store::Store
    /// [`BridgeQuoteRequest`]: defi_execution::BridgeQuoteRequest
    async fn handle_plan(ctx: &AppCtx, args: PlanArgs) -> Result<Envelope, Error> {
        use crate::execident::{apply_execution_identity_to_action, resolve_execution_identity};
        use defi_errors::Code;
        use defi_execution::BridgeExecutionOptions;
        use defi_model::ProviderStatus;

        // 1. Resolve flag values, merging any structured input (Go PreRunE
        //    `applyStructuredFlagInput`). Explicitly-set flags are never
        //    overridden; unknown JSON keys / null values are usage errors.
        let mut values = PlanValues {
            provider: args.provider.clone().unwrap_or_default(),
            from: args.from.clone().unwrap_or_default(),
            to: args.to.clone().unwrap_or_default(),
            asset: args.asset.clone().unwrap_or_default(),
            to_asset: args.to_asset.clone().unwrap_or_default(),
            amount: args.amount.clone().unwrap_or_default(),
            amount_decimal: args.amount_decimal.clone().unwrap_or_default(),
            from_amount_for_gas: args.from_amount_for_gas.clone().unwrap_or_default(),
            wallet: args.identity.wallet.clone().unwrap_or_default(),
            from_address: args.identity.from_address.clone().unwrap_or_default(),
            recipient: args.recipient.clone().unwrap_or_default(),
            slippage_bps: args.slippage_bps,
            simulate: args.simulate,
            rpc_url: args.rpc_url.clone().unwrap_or_default(),
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
            if args.identity.wallet.is_some() {
                s.insert("wallet");
            }
            if args.identity.from_address.is_some() {
                s.insert("from-address");
            }
            if args.recipient.is_some() {
                s.insert("recipient");
            }
            s
        };
        apply_plan_structured_input(&args.input, &explicit, &mut values)?;

        // 2. `--provider` required (normalized first, like the Go runner: empty →
        //    usage, BEFORE the identity resolve / request build).
        let provider_name = values.provider.trim().to_ascii_lowercase();
        if provider_name.is_empty() {
            return Err(Error::new(Code::Usage, "--provider is required"));
        }

        // 3. Resolve the execution identity (OWS-first `--wallet` / legacy
        //    `--from-address`). Go passes the SOURCE chain (`plan.FromArg`) as the
        //    chain arg. Errors return before any build/persist.
        let identity =
            resolve_execution_identity(&values.wallet, &values.from_address, &values.from)?;

        // 4. Build the canonical request (chain/asset parse, `--to-asset`
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

        // 5. Route the build through the populated registry; capture the status.
        let opts = BridgeExecutionOptions {
            sender: identity.from_address.clone(),
            recipient: values.recipient.clone(),
            slippage_bps: values.slippage_bps,
            simulate: values.simulate,
            rpc_url: values.rpc_url.clone(),
            from_amount_for_gas: values.from_amount_for_gas.clone(),
        };
        let built = ctx
            .bridge_action_registry()
            .build_bridge_action(&provider_name, req, opts)
            .await;
        // The captured provider status is keyed on the builder display name (Go
        // `provider.Info().Name`), falling back to the normalized provider name.
        let status_name = match &built {
            Ok((_, display)) if !display.trim().is_empty() => display.clone(),
            _ => provider_name.clone(),
        };
        let status = ProviderStatus {
            name: status_name,
            status: super::status_from_result(&built),
            latency_ms: 0,
        };
        let (mut action, _display) = built?;

        // 6. Stamp the identity, persist, and emit the success envelope.
        apply_execution_identity_to_action(&mut action, &identity);

        let store = ctx.open_action_store()?;
        store
            .save(&action)
            .map_err(|e| Error::wrap(Code::Internal, "persist planned action", e))?;

        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize planned action", e))?;
        let mut env = ctx.metadata_envelope("bridge plan", data, vec![status]);
        env.warnings = identity.warnings;
        Ok(env)
    }

    /// Merge structured input (`--input-json` / `--input-file`) onto the resolved
    /// `bridge plan` flag values (Go `applyStructuredFlagInput`).
    ///
    /// Reads the payload (mutually-exclusive `--input-json` / `--input-file`;
    /// `-` reads stdin), parses it as a JSON object, and applies each entry
    /// unless the flag was explicitly set on the command line. A non-object
    /// payload, unknown key, or `null` value is a usage error. Recognizes the
    /// full `bridge plan` structured-input surface (Go `bridgePlanArgs` json
    /// tags): the quote fields plus `wallet` / `from_address` / `recipient` /
    /// `slippage_bps` / `simulate` / `rpc_url`.
    fn apply_plan_structured_input(
        input: &crate::execflags::InputFlags,
        explicit: &std::collections::HashSet<&str>,
        values: &mut PlanValues,
    ) -> Result<(), Error> {
        use crate::execflags::{
            apply_structured_input, decode_bool_field, decode_i64_field, decode_string_field,
        };

        apply_structured_input(input, explicit, "bridge plan", |key, canonical, raw| {
            match canonical {
                "provider" => values.provider = decode_string_field(key, raw)?,
                "from" => values.from = decode_string_field(key, raw)?,
                "to" => values.to = decode_string_field(key, raw)?,
                "asset" => values.asset = decode_string_field(key, raw)?,
                "to-asset" => values.to_asset = decode_string_field(key, raw)?,
                "amount" => values.amount = decode_string_field(key, raw)?,
                "amount-decimal" => values.amount_decimal = decode_string_field(key, raw)?,
                "from-amount-for-gas" => {
                    values.from_amount_for_gas = decode_string_field(key, raw)?
                }
                "wallet" => values.wallet = decode_string_field(key, raw)?,
                "from-address" => values.from_address = decode_string_field(key, raw)?,
                "recipient" => values.recipient = decode_string_field(key, raw)?,
                "slippage-bps" => values.slippage_bps = decode_i64_field(key, raw)?,
                "simulate" => values.simulate = decode_bool_field(key, raw)?,
                "rpc-url" => values.rpc_url = decode_string_field(key, raw)?,
                _ => return Ok(false),
            }
            Ok(true)
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
        use crate::execflags::{apply_structured_input, decode_string_field};

        apply_structured_input(input, explicit, "bridge quote", |key, canonical, raw| {
            match canonical {
                "provider" => values.provider = decode_string_field(key, raw)?,
                "from" => values.from = decode_string_field(key, raw)?,
                "to" => values.to = decode_string_field(key, raw)?,
                "asset" => values.asset = decode_string_field(key, raw)?,
                "to-asset" => values.to_asset = decode_string_field(key, raw)?,
                "amount" => values.amount = decode_string_field(key, raw)?,
                "amount-decimal" => values.amount_decimal = decode_string_field(key, raw)?,
                "from-amount-for-gas" => {
                    values.from_amount_for_gas = decode_string_field(key, raw)?
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
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
    use serde_json::{json, Value};
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

#[cfg(test)]
mod plan_tests {
    //! # Success criteria — app-level `bridge plan` (WS3, exec-plan)
    //!
    //! These tests exercise the **wired `bridge plan` handler**
    //! ([`cli::handle`] → `bridge plan`) end-to-end. `bridge plan` is a
    //! capability-based execution-plan command: it builds an executable bridge
    //! [`Action`] via the action-build registry's `BuildBridgeAction`
    //! (Across / LiFi are `BridgeExecutionProvider`s; Bungee is quote-only),
    //! stamps the resolved execution identity, persists to the action
    //! [`Store`], and emits the action envelope with the cache bypassed
    //! (spec §2.5). Flow parity with Go `addBridgeExecutionSubcommands`'
    //! `planCmd.RunE` (`bridge_execution_commands.go` ~L93-161):
    //!
    //!   1. `--provider` required (empty → usage, BEFORE anything else);
    //!   2. resolve the execution identity via the shared OWS-first resolver
    //!      ([`resolve_execution_identity`] over `--wallet` / `--from-address`
    //!      on `--from` as the chain), returning before any build/persist on a
    //!      constraint error;
    //!   3. build the canonical request ([`super::build_bridge_request`]:
    //!      chain/asset parse, `--to-asset` inference, amount normalization,
    //!      `from_amount_for_gas` carry);
    //!   4. route the build through the populated bridge action registry
    //!      ([`Registry::build_bridge_action`]) with [`BridgeExecutionOptions`]
    //!      carrying `sender`/`recipient`/`slippage_bps`/`simulate`/`rpc_url`/
    //!      `from_amount_for_gas`, capturing a single [`ProviderStatus`] keyed
    //!      on the builder display name (falling back to the provider name);
    //!   5. stamp the identity onto the action
    //!      ([`apply_execution_identity_to_action`]), persist to the [`Store`],
    //!      emit the success envelope carrying the identity warnings.
    //!
    //! Because Across/LiFi `BuildBridgeAction` performs an HTTP GET to the
    //! provider (`/swap/approval` for Across, `/quote` for LiFi), these are LIVE
    //! commands in Go and are NOT byte-diffed against the Go binary (spec §4.1):
    //! the handler is driven offline against a `wiremock` server through the
    //! `bridge_quote_base` seam ([`AppCtx::with_bridge_base`]) the GREEN handler
    //! MUST honor when constructing its bridge builders (analogous to how
    //! `swap plan` honors `swap_action_registry` + `swap_quote_base`). The
    //! per-field calldata/fee math inside `build_bridge_action` is owned by
    //! `defi-providers` (wiremock-tested there); here we assert only that the
    //! handler surfaces the builder's action/steps into the envelope and pins
    //! the cross-cutting machine contract.
    //!
    //! Criteria:
    //!
    //!  P1. **Success envelope shape (Across, legacy `--from-address`).** With a
    //!      mock Across `/swap/approval` and `bridge plan --provider across
    //!      --from 1 --to 10 --asset USDC --amount 1000000 --from-address
    //!      <addr>`, the resolved [`Envelope`] has `version="v1"`,
    //!      `success=true`, `error=None`, `meta.command="bridge plan"`,
    //!      `meta.partial=false`, and the execution-path cache bypass
    //!      (`meta.cache.status="bypass"`, `age_ms=0`, `stale=false`). Exactly
    //!      one provider status keyed on the builder display name (`"across"`)
    //!      with status `"ok"`. The mock MUST be contacted (proves the
    //!      `bridge_quote_base` seam is honored — offline + deterministic).
    //!  P2. **Planned action data shape.** `data` is the persisted [`Action`]:
    //!      `action_id` = `act_` + 32 lowercase hex; `intent_type="bridge"`;
    //!      `provider="across"`; `status="planned"`; `chain_id="eip155:1"`;
    //!      `from_address` = checksummed sender; `input_amount="1000000"`. Steps:
    //!      `[approval, bridge_send]` (the mock returns one approval txn + the
    //!      swap/bridge txn), with the bridge step typed `bridge_send` on
    //!      `eip155:1`.
    //!  P3. **Bridge-step calldata + settlement guardrail metadata.** The
    //!      approval step echoes the provider approval calldata (ERC-20
    //!      `approve` selector `0x095ea7b3`); the bridge step echoes the
    //!      provider swap-tx calldata (`0xad5425c6`) and targets the checksummed
    //!      provider settlement contract. The bridge step's `expected_outputs`
    //!      carry the settlement guardrail metadata the submit-time pre-sign
    //!      checks consume (`settlement_provider="across"`, a non-empty
    //!      `settlement_status_endpoint`, `settlement_origin_chain="1"`,
    //!      `settlement_destination_chain="10"`). (Bridge calldata is provider-
    //!      supplied, not planner-ABI-encoded, so there is no `defi-evm` ABI
    //!      golden for the bridge tx itself — only the ERC-20 approve selector.)
    //!  P4. **Plan persists the action to the Store.** After a successful Across
    //!      plan, the action is retrievable from the [`Store`] by its
    //!      `action_id` with `intent_type="bridge"`, `provider="across"`,
    //!      `input_amount="1000000"`.
    //!  P5. **Legacy `--from-address` warning + backend stamping.** The
    //!      `--from-address` path stamps `execution_backend="legacy_local"` and
    //!      surfaces the OWS-recommended legacy warning
    //!      ([`LEGACY_IDENTITY_WARNING`]).
    //!  P6. **LiFi `--from-amount-for-gas` carried into the build.** With a mock
    //!      LiFi `/quote` that matches ONLY when `fromAmountForGas=100000` is
    //!      forwarded, `bridge plan --provider lifi --from-amount-for-gas 100000`
    //!      succeeds, the captured status is keyed `"lifi"`, the action
    //!      `provider="lifi"`, and the reserve amount is reflected in the action
    //!      metadata (`from_amount_for_gas="100000"`). A handler that drops the
    //!      reserve never reaches a 200 (the test fails), pinning the carry.
    //!  P7. **Decimal-amount parity.** `--amount-decimal 1` (USDC, 6 decimals)
    //!      normalizes to `input_amount="1000000"` (base+decimal consistency,
    //!      spec §2.4).
    //!  P8. **`--input-json` precedence.** An explicit `--provider across` flag
    //!      OVERRIDES a JSON `"provider":"bogus"` (Go `applyStructuredFlagInput`
    //!      fills only flags the user did not set): the request reaches the
    //!      Across mock and succeeds rather than failing unsupported.
    //!  P9. **`--provider` required (spec §2.5), persists nothing.** A missing
    //!      `--provider` is a [`Code::Usage`] error (exit 2) BEFORE any build/
    //!      persist; nothing is persisted. (Go `planCmd`: empty provider →
    //!      `CodeUsage` `--provider is required`.)
    //!  P10. **Quote-only provider → unsupported, persists nothing.** Bungee is a
    //!      registered bridge *quote* provider with no execution builder; Go
    //!      `BuildBridgeAction` → `CodeUnsupported` (exit 13) with the quote-only
    //!      message. Nothing is persisted.
    //!  P11. **Unknown provider → unsupported, persists nothing.** A provider not
    //!      in the registered set is [`Code::Unsupported`] (exit 13) with the
    //!      `unsupported bridge provider` message. Nothing is persisted.
    //!  P12. **Identity-constraint errors, persist nothing.** Both identity
    //!      inputs / neither input / malformed `--from-address` each fail with
    //!      [`Code::Usage`] (exit 2) BEFORE any build/persist (Go
    //!      `resolveExecutionIdentity`). Nothing is persisted.
    //!  P13. **`--to-asset` inference failure → usage, persists nothing.** A bare
    //!      contract-address source asset with an empty `--to-asset` is a
    //!      [`Code::Usage`] error (exit 2) from `build_bridge_request`. Nothing
    //!      is persisted.
    //!  P14. **Full-binary exit codes.** Via `run_with_args`: missing `--provider`
    //!      → exit 2; missing identity input → exit 2; quote-only provider →
    //!      exit 13. (Drives the clap parse + guard ordering end-to-end; the
    //!      live build path is not reached on these error paths so no network is
    //!      needed.)
    //!  P15. **Flag parsing.** `bridge plan` parses the full flag surface and
    //!      applies the Go defaults (`--slippage-bps 50`, `--simulate true`).
    //!
    //! SKIPPED (owned elsewhere / wrong layer): the OWS `--wallet` happy-path
    //! resolve + wallet-id persistence (needs a vault fixture / CLI — WS4b e2e);
    //! the per-field BridgeQuote/approval/swap-tx math + `set_base_url` seam
    //! semantics inside the adapters (defi-providers, wiremock-tested there); the
    //! registry routing error semantics for `build_bridge_action`
    //! (`defi-execution::builder`, covered by its own RED suite); submit-time
    //! signer/backend/guardrail enforcement + receipt/settlement polling (WS4);
    //! and JSON field-declaration-order rendering (defi-out golden tests).

    use super::cli::{handle, BridgeCmd, PlanArgs};
    use crate::cli::run_with_args;
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags};
    use crate::execident::LEGACY_IDENTITY_WARNING;
    use defi_config::{MapEnv, Settings};
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::{json, Value};
    use std::path::Path;
    use std::time::Duration;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // --- constants ---------------------------------------------------------

    /// A canonical lowercase EVM sender used as the legacy `--from-address`.
    const SENDER: &str = "0x00000000000000000000000000000000000000aa";

    // --- harness -----------------------------------------------------------

    /// Execution settings with a real action store under `dir`, caching
    /// disabled (execution paths bypass the cache anyway, spec §2.5), and no
    /// provider keys (bridge plan needs none for Across/LiFi).
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

    /// An Across `bridge plan` `PlanArgs` (USDC 1→10, legacy `--from-address`).
    /// Mutate per test.
    fn across_plan_args() -> PlanArgs {
        PlanArgs {
            from: Some("1".to_string()),
            to: Some("10".to_string()),
            asset: Some("USDC".to_string()),
            to_asset: None,
            provider: Some("across".to_string()),
            amount: Some("1000000".to_string()),
            amount_decimal: None,
            from_amount_for_gas: None,
            recipient: None,
            slippage_bps: 50,
            rpc_url: None,
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(SENDER.to_string()),
            },
            input: InputFlags::default(),
        }
    }

    /// A LiFi `bridge plan` `PlanArgs` (USDC 1→10, legacy `--from-address`).
    fn lifi_plan_args() -> PlanArgs {
        let mut args = across_plan_args();
        args.provider = Some("lifi".to_string());
        args
    }

    async fn run_plan(dir: &Path, args: PlanArgs) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(&ctx, BridgeCmd::Plan(args)).await
    }

    /// Run the plan with the bridge-quote provider base URL retargeted at
    /// `base` (the offline/wiremock seam the GREEN handler MUST honor).
    async fn run_plan_with_base(dir: &Path, base: &str, args: PlanArgs) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir)).with_bridge_base(base);
        handle(&ctx, BridgeCmd::Plan(args)).await
    }

    fn usage_exit(err: &Error) -> i32 {
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

    fn env_with_home() -> (MapEnv, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let env = MapEnv::with_home(tmp.path().to_path_buf());
        (env, tmp)
    }

    // --- across mocks ------------------------------------------------------

    /// Mount the Across `/swap/approval` execution route (one approval txn +
    /// the swap/bridge txn) on a fresh `MockServer`. Mirrors the body the
    /// `defi-providers` Across builder suite uses.
    async fn across_swap_approval_mock() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/swap/approval"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "approvalTxns": [{
                        "chainId": 1,
                        "to": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
                        "data": "0x095ea7b3",
                        "value": "0"
                    }],
                    "swapTx": {
                        "chainId": 1,
                        "to": "0x5c7BCd6E7De5423a257D81B442095A1a6ced35C5",
                        "data": "0xad5425c6",
                        "value": "0x0"
                    },
                    "minOutputAmount": "990000",
                    "expectedOutputAmount": "995000",
                    "expectedFillTime": 5
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        server
    }

    // --- P1: success envelope (Across, legacy --from-address) --------------

    #[tokio::test(flavor = "multi_thread")]
    async fn across_plan_emits_success_envelope() {
        let server = across_swap_approval_mock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let env = run_plan_with_base(dir.path(), &server.uri(), across_plan_args())
            .await
            .expect("across bridge plan should succeed against the mock /swap/approval");

        // The wired handler MUST have contacted the mock (proves the
        // bridge_quote_base seam is honored) — offline + deterministic.
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
        assert_eq!(env.meta.command, "bridge plan");
        assert!(!env.meta.partial);

        // Execution paths bypass the cache (spec §2.5).
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        // One provider status row keyed on the builder display name, ok.
        assert_eq!(env.meta.providers.len(), 1, "exactly one provider status");
        assert_eq!(env.meta.providers[0].name, "across");
        assert_eq!(env.meta.providers[0].status, "ok");
    }

    // --- P2: planned action data shape -------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn across_plan_action_shape() {
        let server = across_swap_approval_mock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let env = run_plan_with_base(dir.path(), &server.uri(), across_plan_args())
            .await
            .expect("plan");
        let data = action_data(&env);

        let action_id = data["action_id"].as_str().expect("action_id");
        assert!(
            action_id.starts_with("act_") && action_id.len() == 36,
            "action_id must be act_ + 32 hex: {action_id}"
        );
        assert!(
            action_id[4..].chars().all(|c| c.is_ascii_hexdigit()),
            "action_id suffix must be hex: {action_id}"
        );
        assert_eq!(data["intent_type"], json!("bridge"));
        assert_eq!(data["provider"], json!("across"));
        assert_eq!(data["status"], json!("planned"));
        assert_eq!(data["chain_id"], json!("eip155:1"));
        assert_eq!(
            data["from_address"],
            json!(defi_evm::address::checksum(SENDER).unwrap())
        );
        assert_eq!(data["input_amount"], json!("1000000"));

        let steps = data["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 2, "approval + bridge_send steps");
        assert_eq!(steps[0]["type"], json!("approval"));
        // StepType::Bridge renders as `bridge_send`.
        assert_eq!(steps[1]["type"], json!("bridge_send"));
        assert_eq!(steps[1]["chain_id"], json!("eip155:1"));
    }

    // --- P3: bridge-step calldata + settlement guardrail metadata ----------

    #[tokio::test(flavor = "multi_thread")]
    async fn across_plan_step_calldata_and_settlement_metadata() {
        let server = across_swap_approval_mock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let env = run_plan_with_base(dir.path(), &server.uri(), across_plan_args())
            .await
            .expect("plan");
        let data = action_data(&env);
        let steps = data["steps"].as_array().expect("steps");

        // Approval step echoes the provider approval calldata (ERC-20 approve).
        assert!(
            steps[0]["data"].as_str().unwrap().starts_with("0x095ea7b3"),
            "approval step must be an ERC-20 approve: {}",
            steps[0]["data"]
        );

        // Bridge step echoes the provider swap-tx calldata and targets the
        // checksummed settlement contract from the mock.
        assert_eq!(steps[1]["data"], json!("0xad5425c6"));
        assert_eq!(
            steps[1]["target"].as_str().unwrap().to_lowercase(),
            "0x5c7bcd6e7de5423a257d81b442095a1a6ced35c5",
            "bridge step must target the provider settlement contract"
        );

        // Settlement guardrail metadata the submit-time pre-sign checks consume.
        let outs = steps[1]["expected_outputs"]
            .as_object()
            .expect("bridge step expected_outputs");
        assert_eq!(outs["settlement_provider"], json!("across"));
        assert!(
            outs["settlement_status_endpoint"]
                .as_str()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "settlement_status_endpoint must be present: {outs:?}"
        );
        assert_eq!(outs["settlement_origin_chain"], json!("1"));
        assert_eq!(outs["settlement_destination_chain"], json!("10"));
    }

    // --- P4: persists action to the store ----------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn across_plan_persists_action() {
        let server = across_swap_approval_mock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let env = run_plan_with_base(dir.path(), &server.uri(), across_plan_args())
            .await
            .expect("plan");
        let id = action_data(&env)["action_id"].as_str().unwrap().to_string();

        let store = ActionStore::open(
            dir.path().join("actions.db"),
            dir.path().join("actions.lock"),
        )
        .expect("open store");
        let persisted = store.get(&id).expect("persisted action retrievable");
        assert_eq!(persisted.intent_type, "bridge");
        assert_eq!(persisted.provider, "across");
        assert_eq!(persisted.input_amount, "1000000");
    }

    // --- P5: legacy warning + backend stamping -----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn across_plan_legacy_warning_and_backend() {
        let server = across_swap_approval_mock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let env = run_plan_with_base(dir.path(), &server.uri(), across_plan_args())
            .await
            .expect("plan");
        let data = action_data(&env);
        assert_eq!(
            data["execution_backend"],
            json!("legacy_local"),
            "--from-address path stamps the legacy backend"
        );
        assert!(
            env.warnings.iter().any(|w| w == LEGACY_IDENTITY_WARNING),
            "the OWS-recommended legacy warning must surface: {:?}",
            env.warnings
        );
    }

    // --- P6: LiFi --from-amount-for-gas carried ----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn lifi_plan_carries_from_amount_for_gas() {
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
        // fails), pinning the carry-through into the build options.
        Mock::given(method("GET"))
            .and(path("/quote"))
            .and(query_param("fromAmountForGas", "100000"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        // LiFi `build_bridge_action` reads the ERC-20 allowance via an `eth_call`
        // on the source-chain RPC (USDC has a non-empty approval address, so
        // `should_add_approval` is true). This is a LIVE command path: we point the
        // source-chain RPC at the same mock server (via `--rpc-url`) and serve a
        // zero allowance so the approval step is added — keeping the test offline
        // and deterministic (parity with the `defi-providers` LiFi builder suite).
        let zero_allowance = format!(
            "0x{}",
            hex::encode(alloy::primitives::U256::ZERO.to_be_bytes::<32>())
        );
        Mock::given(method("POST"))
            .and(body_partial_json(json!({ "method": "eth_call" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": zero_allowance,
            })))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = lifi_plan_args();
        args.from_amount_for_gas = Some("100000".to_string());
        // Route the source-chain allowance RPC at the offline mock server.
        args.rpc_url = Some(server.uri());

        let env = run_plan_with_base(dir.path(), &server.uri(), args)
            .await
            .expect("lifi bridge plan with from-amount-for-gas should succeed");

        assert_eq!(env.meta.command, "bridge plan");
        assert!(env.success);
        assert_eq!(env.meta.providers.len(), 1);
        assert_eq!(env.meta.providers[0].name, "lifi");

        let data = action_data(&env);
        assert_eq!(data["provider"], json!("lifi"));
        // The reserve amount is reflected on the action metadata.
        assert_eq!(
            data["metadata"]["from_amount_for_gas"],
            json!("100000"),
            "the reserve amount must be carried into the bridge action: {data:?}"
        );
    }

    // --- P7: decimal-amount parity -----------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn across_plan_decimal_amount_parity() {
        let server = across_swap_approval_mock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = across_plan_args();
        args.amount = None;
        args.amount_decimal = Some("1".to_string()); // USDC has 6 decimals
        let env = run_plan_with_base(dir.path(), &server.uri(), args)
            .await
            .expect("plan");
        let data = action_data(&env);
        assert_eq!(data["input_amount"], json!("1000000"));
    }

    // --- P8: --input-json precedence ---------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn across_plan_explicit_provider_overrides_input_json() {
        // The JSON sets provider="bogus" (which would be exit 13), but the
        // explicit --provider across flag must win (Go applyStructuredFlagInput
        // fills only flags the user did not set). With the mock base, the
        // request reaches the Across mock and succeeds, proving the override.
        let server = across_swap_approval_mock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = across_plan_args();
        args.provider = Some("across".to_string());
        args.input = InputFlags {
            input_json: Some(
                r#"{"provider":"bogus","from":"1","to":"10","asset":"USDC","amount":"1000000"}"#
                    .to_string(),
            ),
            input_file: None,
        };

        let env = run_plan_with_base(dir.path(), &server.uri(), args)
            .await
            .expect("explicit --provider across must override the JSON provider");
        assert!(env.success);
        assert_eq!(action_data(&env)["provider"], json!("across"));
    }

    /// A JSON number supplied for a string flag (`amount`) is a usage decode
    /// error, matching Go `decodeRawFlagValue` (`json.Unmarshal(number → string)`
    /// fails). Locks the strict-decode parity (no silent number→string coercion).
    #[tokio::test(flavor = "multi_thread")]
    async fn plan_input_json_number_for_string_flag_is_usage_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = PlanArgs {
            input: InputFlags {
                input_json: Some(format!(
                    r#"{{"provider":"across","from":"1","to":"10","asset":"USDC","amount":1000000,"from_address":"{SENDER}"}}"#
                )),
                input_file: None,
            },
            ..PlanArgs::default()
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("a JSON number for the string amount flag must be a usage error");
        assert_eq!(err.code, Code::Usage);
        assert!(
            err.message
                .starts_with("decode structured input field \"amount\""),
            "got {:?}",
            err.message
        );
        assert!(no_actions_persisted(dir.path()));
    }

    /// An unrecognized structured-input key is a usage error keyed on the
    /// command path; persists nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn plan_input_json_unknown_field_is_usage_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = PlanArgs {
            input: InputFlags {
                input_json: Some(r#"{"provider":"across","bogus":"x"}"#.to_string()),
                input_file: None,
            },
            ..PlanArgs::default()
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("unknown structured-input field must be a usage error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert_eq!(
            err.message,
            "structured input field \"bogus\" is not supported by bridge plan"
        );
        assert!(no_actions_persisted(dir.path()));
    }

    // --- P9: --provider required (spec §2.5), persists nothing -------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_requires_provider() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = across_plan_args();
        args.provider = None;
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("missing --provider must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(dir.path()));
    }

    // --- P10: quote-only provider (bungee) -> unsupported ------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_quote_only_provider_unsupported() {
        // Bungee is a registered bridge *quote* provider but has no execution
        // builder; Go BuildBridgeAction -> "bridge provider \"bungee\" is
        // quote-only; ...".
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = across_plan_args();
        args.provider = Some("bungee".to_string());
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("quote-only provider must fail planning");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(usage_exit(&err), 13);
        // Message asserts the SPECIFIC Go quote-only guard (not the
        // unimplemented stub, which also returns Unsupported).
        assert!(
            err.to_string().contains("quote-only"),
            "expected the Go quote-only bridge message, got: {err}"
        );
        assert!(no_actions_persisted(dir.path()));
    }

    // --- P11: unknown provider -> unsupported ------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_unknown_provider_unsupported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = across_plan_args();
        args.provider = Some("bogus".to_string());
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("unknown provider must fail");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(usage_exit(&err), 13);
        assert!(
            err.to_string().contains("unsupported bridge provider"),
            "expected the Go unknown-provider message, got: {err}"
        );
        assert!(no_actions_persisted(dir.path()));
    }

    // --- P12: identity-constraint errors, persist nothing ------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_both_identity_inputs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = across_plan_args();
        args.identity = PlanIdentityFlags {
            wallet: Some("alice".to_string()),
            from_address: Some(SENDER.to_string()),
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("both identity inputs must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(dir.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_missing_identity_inputs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = across_plan_args();
        args.identity = PlanIdentityFlags {
            wallet: None,
            from_address: None,
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("missing identity must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(dir.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_rejects_malformed_from_address() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = across_plan_args();
        args.identity = PlanIdentityFlags {
            wallet: None,
            from_address: Some("0xnot-an-address".to_string()),
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("malformed --from-address must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(dir.path()));
    }

    // --- P13: --to-asset inference failure ---------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_to_asset_inference_failure_is_usage() {
        // A bare contract-address source asset has no symbol to infer a
        // destination asset from; with an empty --to-asset this is a usage
        // error from build_bridge_request, BEFORE any build/persist.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = across_plan_args();
        args.asset = Some("0x1111111111111111111111111111111111111111".to_string());
        args.to_asset = None;
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("uninferable destination asset must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("destination asset cannot be inferred"),
            "expected the Go inference-failure message, got: {err}"
        );
        assert!(no_actions_persisted(dir.path()));
    }

    // --- P14: full-binary exit codes ---------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_missing_provider_full_binary_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "bridge",
                "plan",
                "--from",
                "1",
                "--to",
                "10",
                "--asset",
                "USDC",
                "--amount",
                "1000000",
                "--from-address",
                SENDER,
            ],
            &env,
        )
        .await;
        assert_eq!(code, 2, "missing --provider must be a usage error (exit 2)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_missing_identity_full_binary_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "bridge",
                "plan",
                "--provider",
                "across",
                "--from",
                "1",
                "--to",
                "10",
                "--asset",
                "USDC",
                "--amount",
                "1000000",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "missing identity input on bridge plan must be a usage error (exit 2)"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_quote_only_provider_full_binary_exit_13() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "bridge",
                "plan",
                "--provider",
                "bungee",
                "--from",
                "1",
                "--to",
                "10",
                "--asset",
                "USDC",
                "--amount",
                "1000000",
                "--from-address",
                SENDER,
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 13,
            "a quote-only bridge provider plan must be unsupported (exit 13)"
        );
    }

    // --- P15: flag parsing -------------------------------------------------

    #[test]
    fn bridge_plan_flags_parse_with_defaults() {
        use clap::Parser;
        // Defaults: --slippage-bps 50, --simulate true.
        let cli = crate::cli::Cli::try_parse_from([
            "defi",
            "bridge",
            "plan",
            "--provider",
            "across",
            "--from",
            "1",
            "--to",
            "10",
            "--asset",
            "USDC",
            "--amount",
            "1000000",
            "--from-address",
            SENDER,
        ])
        .expect("bridge plan flags parse");
        if let crate::cli::TopCommand::Bridge {
            cmd: BridgeCmd::Plan(args),
        } = cli.command
        {
            assert_eq!(args.provider.as_deref(), Some("across"));
            assert_eq!(args.from.as_deref(), Some("1"));
            assert_eq!(args.to.as_deref(), Some("10"));
            assert_eq!(args.asset.as_deref(), Some("USDC"));
            assert_eq!(args.amount.as_deref(), Some("1000000"));
            assert_eq!(args.identity.from_address.as_deref(), Some(SENDER));
            assert_eq!(args.slippage_bps, 50, "default slippage-bps");
            assert!(args.simulate, "default simulate true");
        } else {
            panic!("expected bridge plan");
        }
    }

    #[test]
    fn bridge_plan_flags_parse_overrides() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "defi",
            "bridge",
            "plan",
            "--provider",
            "lifi",
            "--from",
            "1",
            "--to",
            "10",
            "--asset",
            "USDC",
            "--to-asset",
            "USDC",
            "--amount",
            "1000000",
            "--from-amount-for-gas",
            "100000",
            "--recipient",
            SENDER,
            "--slippage-bps",
            "100",
            "--simulate",
            "false",
            "--rpc-url",
            "http://127.0.0.1:8545",
            "--wallet",
            "alice",
        ])
        .expect("bridge plan overrides parse");
        if let crate::cli::TopCommand::Bridge {
            cmd: BridgeCmd::Plan(args),
        } = cli.command
        {
            assert_eq!(args.provider.as_deref(), Some("lifi"));
            assert_eq!(args.to_asset.as_deref(), Some("USDC"));
            assert_eq!(args.from_amount_for_gas.as_deref(), Some("100000"));
            assert_eq!(args.recipient.as_deref(), Some(SENDER));
            assert_eq!(args.slippage_bps, 100);
            assert!(!args.simulate);
            assert_eq!(args.rpc_url.as_deref(), Some("http://127.0.0.1:8545"));
            assert_eq!(args.identity.wallet.as_deref(), Some("alice"));
        } else {
            panic!("expected bridge plan");
        }
    }
}

#[cfg(test)]
mod submit_app_tests {
    //! # Success criteria — app-level `bridge submit` (WS4, exec-submit)
    //!
    //! Go oracle: `internal/app/bridge_execution_commands.go`
    //! `addBridgeExecutionSubcommands` `submitCmd.RunE` (lines ~163-215). `bridge
    //! submit` is the **standard-EVM** execution submit (Across / LiFi bridge
    //! actions are EVM `legacy_local` / `ows` actions — there is NO Tempo bridge
    //! path, unlike `swap submit`). It loads a persisted bridge action, resolves
    //! the signing/execution backend from the action's persisted
    //! `execution_backend` + the submit signer flags, validates the resolved
    //! sender against `--from-address` and the planned sender, parses the execute
    //! options (including the `--allow-max-approval` / `--unsafe-provider-tx`
    //! guardrail opt-ins that bridge submit carries), runs the bounded-approval
    //! pre-sign guardrail, and broadcasts through the engine — which, for a
    //! `bridge_send` step, waits for **destination settlement** (Across
    //! `/deposit/status`, LiFi `/status`) before marking the step confirmed
    //! (owned by `defi_execution::evm_executor`; the settlement-wait semantics are
    //! pinned by the sibling `settlement_tests` module). The terminal-state
    //! envelope is emitted with the cache bypassed (spec §2.5).
    //!
    //! Flow parity with the Go `submitCmd.RunE`:
    //!   1. resolve + validate `--action-id`
    //!      ([`crate::actions::resolve_action_id`]: empty / malformed → usage);
    //!   2. load the persisted action (not-found → usage `load action`);
    //!   3. gate the intent (`bridge`-only — [`super::ensure_bridge_intent`]);
    //!   4. short-circuit an already-`completed` action (success + warning, no
    //!      re-broadcast);
    //!   5. resolve the execution backend + signer
    //!      ([`crate::execsubmit::resolve_action_execution_backend`]: legacy-local
    //!      only accepts `--signer local`; OWS requires a persisted `wallet_id`
    //!      and rejects legacy signer flags). There is NO Tempo bridge branch;
    //!   6. validate the resolved signer vs `--from-address` + the planned sender
    //!      ([`crate::execsubmit::validate_execution_sender`]);
    //!   7. parse the execute options ([`crate::execsubmit::parse_execute_options`]:
    //!      durations, `--gas-multiplier > 1`, fee flags, the
    //!      `--allow-max-approval` / `--unsafe-provider-tx` opt-ins);
    //!   8. run the bounded-approval pre-sign guardrail with the action context
    //!      ([`crate::execsubmit::presign_validate_action`]);
    //!   9. broadcast through the engine ([`crate::execsubmit::execute_resolved`]),
    //!      persisting each transition, and emit the terminal-state envelope.
    //!
    //! On every guard/build error the typed [`Error`] is returned (the runner
    //! renders the full error envelope to stderr) and the persisted action is left
    //! in its pre-submit state.
    //!
    //! Because the Across / LiFi `bridge plan` build path performs an HTTP GET to
    //! the provider, these fixtures plan offline against a `wiremock` server (the
    //! `bridge_quote_base` seam, [`AppCtx::with_bridge_base`]); the offline-policed
    //! engine then confirms the persisted steps WITHOUT dialing a live RPC
    //! (parity with the `swap`/`approvals` submit suites — the full RPC-backed
    //! sign/broadcast is exercised by `defi-execution` integration tests). The
    //! bridge plan stamps the canonical Across settlement endpoint + execution
    //! target, so a default (bounded) submit passes the bridge pre-sign policy.
    //!
    //! Criteria (each maps to a Go `submitCmd.RunE` behavior):
    //!
    //!  S1. **Submit success envelope + completion (Across, legacy `--from-address`).**
    //!      A planned Across bridge action submitted with the deterministic local
    //!      key completes offline: `version="v1"`, `success=true`, `error=None`,
    //!      `meta.command="bridge submit"`, `meta.partial=false`, execution-path
    //!      cache bypass (`status="bypass"`, `age_ms=0`, `stale=false`); `data` is
    //!      the [`Action`] with `status="completed"` and every step `confirmed`,
    //!      including the `bridge_send` step.
    //!  S2. **Submit persists the terminal state.** After a successful submit the
    //!      action reloads from the [`Store`] with `status="completed"`.
    //!  S3. **action-id validation.** An empty / malformed `--action-id` is a
    //!      [`Code::Usage`] error (exit 2) BEFORE any load.
    //!  S4. **Unknown action → usage load error.** A well-formed but unknown
    //!      `--action-id` surfaces a [`Code::Usage`] `load action` error (exit 2).
    //!  S5. **Intent gate (bridge-only).** A persisted NON-`bridge` action (e.g. a
    //!      `swap` action) submitted through `bridge submit` is a [`Code::Usage`]
    //!      error (exit 2) `action is not a bridge intent`; the action status is
    //!      untouched.
    //!  S6. **Already-completed short-circuit.** A completed action returns success
    //!      with the `action already completed` warning and no re-broadcast.
    //!  S7. **Legacy backend rejects a non-local signer.** A `legacy_local` bridge
    //!      action submitted with `--signer tempo` is a [`Code::Usage`] error
    //!      (exit 2) `legacy actions only support --signer local`; status untouched.
    //!  S8. **OWS backend missing `wallet_id` → usage.** An `ows`-backed bridge
    //!      action with an empty `wallet_id` (and no legacy signer flags) is a
    //!      [`Code::Usage`] error (exit 2) `wallet-backed action is missing
    //!      persisted wallet_id`.
    //!  S9. **OWS backend rejects legacy signer flags.** An `ows`-backed bridge
    //!      action submitted with an explicit `--private-key` is a [`Code::Usage`]
    //!      error (exit 2) `wallet-backed actions do not accept legacy signer
    //!      flags`.
    //!  S10. **`--from-address` mismatch → signer error.** A resolved signer whose
    //!      address differs from `--from-address` is a [`Code::Signer`] error
    //!      (exit 24); status untouched.
    //!  S11. **Planned-sender / signer mismatch → signer error.** A planned action
    //!      sender that differs from the resolved signer is a [`Code::Signer`]
    //!      error (exit 24); status untouched.
    //!  S12. **execute-option validation.** `--gas-multiplier <= 1`, a non-positive
    //!      `--poll-interval`, and an unparseable `--step-timeout` are each
    //!      [`Code::Usage`] errors (exit 2).
    //!  S13. **Signer init failure (no key) → signer error.** A `legacy_local`
    //!      action submitted with `--key-source env` and no `--private-key`
    //!      override is a [`Code::Signer`] error (exit 24); status untouched.
    //!  S14. **Inflated-approval pre-sign gate + `--allow-max-approval` opt-in.** A
    //!      bridge action whose leading approval step exceeds `input_amount`
    //!      (an inflated / max approval — common for Across max approvals) is a
    //!      [`Code::ActionPlan`] error by default with the documented
    //!      `--allow-max-approval` hint; the opt-in lets it complete.
    //!  S15. **Full-binary exit codes.** Via `run_with_args`: malformed
    //!      `--action-id` → exit 2; well-formed unknown `--action-id` → exit 2.
    //!  S16. **Bridge provider-tx pre-sign gate + `--unsafe-provider-tx` opt-in.**
    //!      A bridge action whose `bridge_send` step carries a valid Across
    //!      settlement provider + canonical settlement endpoint but a
    //!      NON-canonical execution target (not an allowed Across spoke-pool /
    //!      execution contract) is rejected by default with a [`Code::ActionPlan`]
    //!      error surfacing the documented `--unsafe-provider-tx` hint; the
    //!      persisted status is untouched. With `--unsafe-provider-tx` the same
    //!      action completes offline. This is the bridge-distinguishing pre-sign
    //!      guardrail (Go `parseExecuteOptions` `UnsafeProviderTx` →
    //!      `validateBridgePolicy`'s target/endpoint allowlist; the policy-layer
    //!      unit matrix lives in `defi_execution::policy`, but the wiring through
    //!      `bridge submit` is asserted HERE, mirroring S14's approval-guardrail
    //!      end-to-end coverage).
    //!
    //! SKIPPED (owned elsewhere / wrong layer): the destination-settlement
    //! wait semantics (Across `/deposit/status`, LiFi `/status`) — owned by
    //! `defi_execution::evm_executor::verify_bridge_settlement`, pinned by the
    //! sibling `settlement_tests` module; the backend-resolution / sender-validation
    //! / execute-option / pre-sign internals — `crate::execsubmit` +
    //! `defi-execution`; the full RPC-backed sign/broadcast byte layout —
    //! `defi-evm` / `defi-execution` integration tests; JSON field-declaration-order
    //! rendering — `defi-out` golden tests.

    use super::cli::{handle, BridgeCmd, PlanArgs};
    use crate::cli::run_with_args;
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags, SubmitArgs};
    use defi_config::{MapEnv, Settings};
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::action::{Action, ActionStatus, ExecutionBackend, StepStatus};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::{json, Value};
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // --- contract constants ------------------------------------------------

    /// The deterministic secp256k1 test key (`internal/execution/signer`
    /// `testPrivateKey`); shared with the `defi-evm` / `defi-execution` suites.
    const TEST_KEY: &str = "59c6995e998f97a5a0044976f0945388cf9b7e5e5f4f9d2d9d8f1f5b7f6d11d1";
    /// The EIP-55 address `defi-evm` derives for [`TEST_KEY`] (pinned against the
    /// go-ethereum oracle). A planned action's `from_address` must equal this for
    /// the local-signer submit to pass the sender-match guard.
    const SIGNER_ADDR: &str = "0x14DDBd1fe5026E58A12eE8691cAEbFD24bb10eef";
    /// A DIFFERENT canonical address — used to force the sender-mismatch guards.
    const OTHER_ADDR: &str = "0x1111111111111111111111111111111111111111";
    const EXPECTED_TX_HASH: &str =
        "0x1111111111111111111111111111111111111111111111111111111111111111";

    // --- harness -----------------------------------------------------------

    /// Execution settings with a real action store under `dir`, cache disabled
    /// (execution paths bypass the cache anyway, spec §2.5).
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

    /// A `BridgeCmd::Submit` `SubmitArgs` carrying the clap flag DEFAULTS (the
    /// `#[derive(Default)]` zero values would NOT match the parsed defaults, so
    /// they are stamped here): `signer=local`, `key_source=auto`,
    /// `gas_multiplier=1.2`, `poll_interval=2s`, `step_timeout=2m`,
    /// `simulate=true`, both guardrail opt-ins `false`. The `--private-key` is
    /// pre-set to the deterministic test key so the offline local-signer path
    /// resolves. Callers mutate the returned value per test.
    pub(super) fn base_submit_args(action_id: &str) -> SubmitArgs {
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

    /// Mount the Across `/swap/approval` execution route (one approval txn + the
    /// swap/bridge txn) on a fresh `MockServer`. The approval txn carries a REAL
    /// bounded `approve(spender, 1000000)` calldata (selector `0x095ea7b3` +
    /// 32-byte spender + 32-byte amount == the planned `input_amount`), so the
    /// default (no `--allow-max-approval`) bounded-approval pre-sign guardrail
    /// passes and the submit-completion tests exercise the full broadcast path.
    /// Mirrors the body the `defi-providers` Across builder suite uses and stamps
    /// the canonical Across execution target so the default submit also passes the
    /// bridge provider-tx pre-sign policy.
    async fn across_swap_approval_mock() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/swap/approval"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "approvalTxns": [{
                        "chainId": 1,
                        "to": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
                        "data": "0x095ea7b30000000000000000000000005c7bcd6e7de5423a257d81b442095a1a6ced35c500000000000000000000000000000000000000000000000000000000000f4240",
                        "value": "0"
                    }],
                    "swapTx": {
                        "chainId": 1,
                        "to": "0x5c7BCd6E7De5423a257D81B442095A1a6ced35C5",
                        "data": "0xad5425c6",
                        "value": "0x0"
                    },
                    "minOutputAmount": "990000",
                    "expectedOutputAmount": "995000",
                    "expectedFillTime": 5
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        mount_standard_submit_rpc(&server).await;
        mount_across_settlement(&server).await;
        server
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

    async fn mount_standard_submit_rpc(server: &MockServer) {
        mock_rpc_method(server, "eth_chainId", json!("0x1")).await;
        mock_rpc_method(server, "eth_call", json!("0x")).await;
        mock_rpc_method(server, "eth_estimateGas", json!("0x5208")).await;
        mock_rpc_method(
            server,
            "eth_getBlockByNumber",
            json!({
                "number": "0x10",
                "baseFeePerGas": "0x3b9aca00"
            }),
        )
        .await;
        mock_rpc_method(server, "eth_maxPriorityFeePerGas", json!("0x3b9aca00")).await;
        mock_rpc_method(server, "eth_getTransactionCount", json!("0x7")).await;
        mock_rpc_method(server, "eth_sendRawTransaction", json!(EXPECTED_TX_HASH)).await;
        mock_rpc_method(
            server,
            "eth_getTransactionReceipt",
            json!({
                "status": "0x1",
                "blockNumber": "0x11",
                "gasUsed": "0x5208"
            }),
        )
        .await;
    }

    async fn mount_across_settlement(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/deposit/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "filled",
                "fillTx": "0x2222222222222222222222222222222222222222222222222222222222222222"
            })))
            .mount(server)
            .await;
    }

    /// An Across `bridge plan` `PlanArgs` (USDC 1→10, legacy `--from-address`).
    fn across_plan_args(from_addr: &str, rpc_url: &str) -> PlanArgs {
        PlanArgs {
            from: Some("1".to_string()),
            to: Some("10".to_string()),
            asset: Some("USDC".to_string()),
            to_asset: None,
            provider: Some("across".to_string()),
            amount: Some("1000000".to_string()),
            amount_decimal: None,
            from_amount_for_gas: None,
            recipient: None,
            slippage_bps: 50,
            rpc_url: Some(rpc_url.to_string()),
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(from_addr.to_string()),
            },
            input: InputFlags::default(),
        }
    }

    /// Plan + persist a canonical Across `bridge` action against `dir`, returning
    /// its `action_id`. `from_addr` becomes the action's `from_address`. Plans
    /// through the real `cli::handle` plan path (offline, via the bridge-quote
    /// base seam) so the persisted shape is identical to production.
    pub(super) async fn plan_across(dir: &Path, from_addr: &str) -> String {
        let server = across_swap_approval_mock().await;
        plan_across_with_server(dir, from_addr, &server).await
    }

    async fn plan_across_with_server(dir: &Path, from_addr: &str, server: &MockServer) -> String {
        let ctx = AppCtx::new(exec_settings(dir)).with_bridge_base(&server.uri());
        let env = handle(
            &ctx,
            BridgeCmd::Plan(across_plan_args(from_addr, &server.uri())),
        )
        .await
        .expect("plan an across bridge action for the submit fixture");
        let action_id = env.data.expect("plan data")["action_id"]
            .as_str()
            .expect("action_id")
            .to_string();
        point_bridge_settlement_to_mock(dir, &action_id, &server.uri());
        action_id
    }

    fn point_bridge_settlement_to_mock(dir: &Path, action_id: &str, server_uri: &str) {
        let store = ActionStore::open(dir.join("actions.db"), dir.join("actions.lock"))
            .expect("open action store");
        let mut action = store.get(action_id).expect("load planned bridge");
        let endpoint = format!("{server_uri}/deposit/status");
        for step in &mut action.steps {
            if let Some(outputs) = step.expected_outputs.as_mut() {
                if outputs
                    .get("settlement_provider")
                    .and_then(|v| v.as_str())
                    .map(|v| v.eq_ignore_ascii_case("across"))
                    .unwrap_or(false)
                {
                    outputs.insert("settlement_status_endpoint".into(), endpoint.clone().into());
                }
            }
        }
        store
            .save(&action)
            .expect("persist settlement mock endpoint");
    }

    /// Persist `action` directly (used for fixtures the plan path cannot build,
    /// e.g. a `swap`-intent or an OWS-backed action).
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
        handle(&ctx, BridgeCmd::Submit(args)).await
    }

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn data_of(env: &Envelope) -> Value {
        env.data.clone().expect("submit envelope carries `data`")
    }

    fn env_with_home() -> (MapEnv, TempDir) {
        let tmp = TempDir::new().expect("tempdir");
        let env = MapEnv::with_home(tmp.path().to_path_buf());
        (env, tmp)
    }

    // --- S1, S2: submit success + completion + persistence -----------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_across_legacy_local_completes_and_emits_envelope() {
        let tmp = TempDir::new().expect("tempdir");
        let server = across_swap_approval_mock().await;
        let action_id = plan_across_with_server(tmp.path(), SIGNER_ADDR, &server).await;

        let env = run_submit(tmp.path(), base_submit_args(&action_id))
            .await
            .expect("legacy-local across bridge submit should complete offline");

        // Envelope contract.
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "bridge submit");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        // Completed action in data; the bridge_send step is confirmed.
        let data = data_of(&env);
        assert_eq!(data["status"], Value::from("completed"));
        let steps = data["steps"].as_array().expect("steps array");
        assert!(
            steps
                .iter()
                .any(|s| s["type"].as_str() == Some("bridge_send")),
            "the action must carry a bridge_send step: {steps:?}"
        );
        for step in steps {
            assert_eq!(
                step["status"],
                Value::from("confirmed"),
                "every step must be confirmed after a successful submit: {step:?}"
            );
        }

        // Persisted terminal state (criterion S2).
        assert_eq!(persisted_status(tmp.path(), &action_id), "completed");
    }

    // --- S3: action-id validation ------------------------------------------

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

    // --- S4: load failure for an unknown action ----------------------------

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

    // --- S5: intent gate (bridge-only) -------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_non_bridge_intent() {
        let tmp = TempDir::new().expect("tempdir");
        // A persisted SWAP-intent action submitted through bridge submit.
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "swap",
            "eip155:1",
            Default::default(),
        );
        action.from_address = SIGNER_ADDR.to_string();
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        save_action(tmp.path(), &action);

        let args = base_submit_args(&action.action_id);
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("non-bridge intent rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("action is not a bridge intent"),
            "got: {err}"
        );
        // Status untouched.
        assert_eq!(persisted_status(tmp.path(), &action.action_id), "planned");
    }

    // --- S6: already-completed short-circuit -------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_already_completed_short_circuits_with_warning() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_across(tmp.path(), SIGNER_ADDR).await;
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
        assert_eq!(env.meta.command, "bridge submit");
        assert!(
            env.warnings.iter().any(|w| w == "action already completed"),
            "expected `action already completed` warning, got {:?}",
            env.warnings
        );
        let data = data_of(&env);
        assert_eq!(data["status"], Value::from("completed"));
    }

    // --- S7: legacy backend rejects a non-local signer ---------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_legacy_action_rejects_non_local_signer() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_across(tmp.path(), SIGNER_ADDR).await;
        let mut args = base_submit_args(&action_id);
        args.signer = "tempo".to_string();
        args.private_key = None; // a non-local signer + private key is a different error
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

    // --- S8, S9: OWS backend offline guards --------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_ows_action_missing_wallet_id_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "bridge",
            "eip155:1",
            Default::default(),
        );
        action.provider = "across".to_string();
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
            .expect_err("OWS bridge action without wallet_id rejected");
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
            "bridge",
            "eip155:1",
            Default::default(),
        );
        action.provider = "across".to_string();
        action.execution_backend = Some(ExecutionBackend::Ows);
        action.wallet_id = "wallet-123".to_string();
        action.from_address = SIGNER_ADDR.to_string();
        save_action(tmp.path(), &action);

        let mut args = base_submit_args(&action.action_id);
        args.private_key = Some(TEST_KEY.to_string()); // explicit legacy flag
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("OWS bridge action with legacy signer flags rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("wallet-backed actions do not accept legacy signer flags"),
            "got: {err}"
        );
    }

    // --- S10, S11: sender mismatch -----------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_from_address_mismatch() {
        let tmp = TempDir::new().expect("tempdir");
        // Action sender matches the signer, but --from-address is a DIFFERENT addr.
        let action_id = plan_across(tmp.path(), SIGNER_ADDR).await;
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
        let action_id = plan_across(tmp.path(), OTHER_ADDR).await;
        let args = base_submit_args(&action_id);
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("planned-sender/signer mismatch rejected");
        assert_eq!(err.code, Code::Signer);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 24);
        assert_eq!(persisted_status(tmp.path(), &action_id), "planned");
    }

    // --- S12: execute-option validation ------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_rejects_gas_multiplier_not_greater_than_one() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_across(tmp.path(), SIGNER_ADDR).await;
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
        let action_id = plan_across(tmp.path(), SIGNER_ADDR).await;
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
        let action_id = plan_across(tmp.path(), SIGNER_ADDR).await;
        let mut args = base_submit_args(&action_id);
        args.step_timeout = "nope".to_string();
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("unparseable step-timeout rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- S13: signer init failure (no key) ---------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_signer_init_failure_is_signer_error() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = plan_across(tmp.path(), SIGNER_ADDR).await;
        let mut args = base_submit_args(&action_id);
        // Force an unresolvable key: source=env with no --private-key override.
        args.private_key = None;
        args.key_source = "env".to_string();
        let err = run_submit(tmp.path(), args)
            .await
            .expect_err("signer init with no key must fail");
        assert_eq!(err.code, Code::Signer);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 24);
        assert_eq!(persisted_status(tmp.path(), &action_id), "planned");
    }

    // --- S14: inflated-approval pre-sign gate + --allow-max-approval -------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_inflated_approval_requires_allow_max_approval() {
        // A bridge action whose leading approval step approves MORE than the
        // action's input_amount (an inflated / "max" approval — common for Across
        // routes) must be rejected by the bounded-approval pre-sign guardrail
        // unless `--allow-max-approval` is set (Go `parseExecuteOptions` +
        // `presign_validate_action`). Built directly so the approval calldata
        // encodes an over-bound amount.
        let tmp = TempDir::new().expect("tempdir");
        let rpc = MockServer::start().await;
        mount_standard_submit_rpc(&rpc).await;
        let action = inflated_approval_bridge_action(&rpc.uri());
        save_action(tmp.path(), &action);

        // Default submit (no opt-in) → ActionPlan rejection with the hint.
        let err = run_submit(tmp.path(), base_submit_args(&action.action_id))
            .await
            .expect_err("an inflated approval must be rejected without --allow-max-approval");
        assert_eq!(err.code, Code::ActionPlan);
        assert!(
            err.to_string().contains("allow-max-approval"),
            "the rejection must surface the --allow-max-approval hint: {err}"
        );
        // Nothing broadcast → status untouched.
        assert_eq!(persisted_status(tmp.path(), &action.action_id), "planned");

        // With the opt-in the same action completes offline.
        let mut args = base_submit_args(&action.action_id);
        args.allow_max_approval = true;
        let env = run_submit(tmp.path(), args)
            .await
            .expect("--allow-max-approval lets the inflated approval through");
        assert_eq!(data_of(&env)["status"], Value::from("completed"));
    }

    /// Build a `bridge` action with a single inflated `approval` step: the ERC-20
    /// `approve` calldata grants `u128::MAX` while the action `input_amount` is
    /// `1000000`, so the bounded-approval pre-sign guardrail trips by default. The
    /// step targets an arbitrary token and carries a (fake but well-formed)
    /// `rpc_url` so the offline-policed engine can confirm it once the bound check
    /// passes. No `bridge_send` step is needed (the approval gate runs first).
    fn inflated_approval_bridge_action(rpc_url: &str) -> Action {
        use defi_execution::action::{ActionStep, StepType};

        // approve(spender, u128::MAX) — selector 0x095ea7b3.
        let spender = "0000000000000000000000005c7bcd6e7de5423a257d81b442095a1a6ced35c5";
        let max = "00000000000000000000000000000000ffffffffffffffffffffffffffffffff";
        let approve_data = format!("0x095ea7b3{spender}{max}");

        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "bridge",
            "eip155:1",
            Default::default(),
        );
        action.provider = "across".to_string();
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        action.from_address = SIGNER_ADDR.to_string();
        action.input_amount = "1000000".to_string();
        action.steps = vec![ActionStep {
            step_id: "step-1".to_string(),
            step_type: StepType::Approval,
            status: StepStatus::Pending,
            chain_id: "eip155:1".to_string(),
            rpc_url: rpc_url.to_string(),
            description: String::new(),
            target: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string(),
            data: approve_data,
            value: "0".to_string(),
            calls: Vec::new(),
            expected_outputs: None,
            tx_hash: String::new(),
            error: String::new(),
        }];
        action
    }

    // --- S16: bridge provider-tx pre-sign gate + --unsafe-provider-tx ------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_non_canonical_bridge_target_requires_unsafe_provider_tx() {
        // A bridge action whose `bridge_send` step carries a valid Across
        // settlement provider + canonical settlement endpoint but a NON-canonical
        // execution target must be rejected by the bridge provider-tx pre-sign
        // guardrail unless `--unsafe-provider-tx` is set (Go `parseExecuteOptions`
        // `UnsafeProviderTx` → `validateBridgePolicy` target allowlist). This is
        // the bridge-distinguishing pre-sign check; S14 covers the (shared)
        // bounded-approval gate, this covers the provider-tx gate end-to-end.
        let tmp = TempDir::new().expect("tempdir");
        let rpc = MockServer::start().await;
        mount_standard_submit_rpc(&rpc).await;
        mount_across_settlement(&rpc).await;
        let action = non_canonical_target_bridge_action(
            &rpc.uri(),
            &format!("{}/deposit/status", rpc.uri()),
        );
        save_action(tmp.path(), &action);

        // Default submit (no opt-in) → ActionPlan rejection with the hint.
        let err = run_submit(tmp.path(), base_submit_args(&action.action_id))
            .await
            .expect_err(
                "a non-canonical bridge target must be rejected without --unsafe-provider-tx",
            );
        assert_eq!(err.code, Code::ActionPlan);
        assert!(
            err.to_string().contains("unsafe-provider-tx"),
            "the rejection must surface the --unsafe-provider-tx hint: {err}"
        );
        // Nothing broadcast → status untouched.
        assert_eq!(persisted_status(tmp.path(), &action.action_id), "planned");

        // With the opt-in the same action completes offline.
        let mut args = base_submit_args(&action.action_id);
        args.unsafe_provider_tx = true;
        let env = run_submit(tmp.path(), args)
            .await
            .expect("--unsafe-provider-tx lets the non-canonical bridge target through");
        assert_eq!(data_of(&env)["status"], Value::from("completed"));
    }

    /// Build a `bridge` action with a single `bridge_send` step that carries a
    /// VALID Across settlement provider + canonical settlement endpoint (so the
    /// provider + settlement-endpoint guards pass) but a NON-canonical execution
    /// `target` on chain 1 (an arbitrary address, NOT an allowed Across spoke-pool
    /// / execution contract), so ONLY the bridge target guard trips by default.
    /// The step has empty `expected_outputs.settlement_provider` defaulting NOT
    /// used — the provider is stamped explicitly. `from_address` matches the
    /// signer so the sender guard passes first. No `data`/`calls` are needed; the
    /// pre-sign target guard runs before any broadcast.
    fn non_canonical_target_bridge_action(rpc_url: &str, settlement_endpoint: &str) -> Action {
        use defi_execution::action::{ActionStep, StepType};

        let mut outs = serde_json::Map::new();
        outs.insert("settlement_provider".into(), "across".into());
        outs.insert(
            "settlement_status_endpoint".into(),
            settlement_endpoint.into(),
        );

        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "bridge",
            "eip155:1",
            Default::default(),
        );
        action.provider = "across".to_string();
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        action.from_address = SIGNER_ADDR.to_string();
        action.input_amount = "1000000".to_string();
        action.steps = vec![ActionStep {
            step_id: "step-bridge".to_string(),
            step_type: StepType::Bridge,
            status: StepStatus::Pending,
            chain_id: "eip155:1".to_string(),
            rpc_url: rpc_url.to_string(),
            description: String::new(),
            // A non-canonical target: NOT the Across spoke-pool / execution
            // contract for chain 1 (`0x5c7BCd6E7De5423a257D81B442095A1a6ced35C5`).
            target: "0x000000000000000000000000000000000000dEaD".to_string(),
            data: "0xad5425c6".to_string(),
            value: "0".to_string(),
            calls: Vec::new(),
            expected_outputs: Some(outs),
            tx_hash: String::new(),
            error: String::new(),
        }];
        action
    }

    // --- S15: full-binary exit codes ---------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_full_binary_malformed_action_id_exit_2() {
        let (env, _home) = env_with_home();
        let code =
            run_with_args(["defi", "bridge", "submit", "--action-id", "act_xyz"], &env).await;
        assert_eq!(
            code, 2,
            "malformed --action-id must be a usage error (exit 2)"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_full_binary_unknown_action_id_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "bridge",
                "submit",
                "--action-id",
                "act_0123456789abcdef0123456789abcdef",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "well-formed unknown --action-id must be a usage (load) error (exit 2)"
        );
    }

    // --- flag parsing: submit defaults + forwarding ------------------------

    #[test]
    fn bridge_submit_flags_parse_with_defaults() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "defi",
            "bridge",
            "submit",
            "--action-id",
            "act_0123456789abcdef0123456789abcdef",
        ])
        .expect("bridge submit flags parse");
        if let crate::cli::TopCommand::Bridge {
            cmd: BridgeCmd::Submit(args),
        } = cli.command
        {
            assert_eq!(
                args.action_id.as_deref(),
                Some("act_0123456789abcdef0123456789abcdef")
            );
            // Go defaults: --signer local, --key-source auto, --gas-multiplier 1.2,
            // --poll-interval 2s, --step-timeout 2m, --simulate true, both
            // guardrail opt-ins false.
            assert_eq!(args.signer, "local");
            assert_eq!(args.key_source, "auto");
            assert_eq!(args.gas_multiplier, 1.2);
            assert_eq!(args.poll_interval, "2s");
            assert_eq!(args.step_timeout, "2m");
            assert!(args.simulate);
            assert!(!args.allow_max_approval);
            assert!(!args.unsafe_provider_tx);
        } else {
            panic!("expected bridge submit");
        }
    }

    #[test]
    fn bridge_submit_flags_parse_guardrail_opt_ins() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "defi",
            "bridge",
            "submit",
            "--action-id",
            "act_0123456789abcdef0123456789abcdef",
            "--allow-max-approval",
            "--unsafe-provider-tx",
            "--step-timeout",
            "5m",
        ])
        .expect("bridge submit guardrail flags parse");
        if let crate::cli::TopCommand::Bridge {
            cmd: BridgeCmd::Submit(args),
        } = cli.command
        {
            assert!(args.allow_max_approval);
            assert!(args.unsafe_provider_tx);
            assert_eq!(args.step_timeout, "5m");
        } else {
            panic!("expected bridge submit");
        }
    }
}

#[cfg(test)]
mod status_app_tests {
    //! # Success criteria — app-level `bridge status` (WS4, exec-status)
    //!
    //! Go oracle: `internal/app/bridge_execution_commands.go`
    //! `addBridgeExecutionSubcommands` `statusCmd.RunE` (lines ~233-254). `bridge
    //! status` is a pure READ over the persisted action store: resolve + validate
    //! the `--action-id`, load the action (not-found → usage `load action`), gate
    //! the intent (`bridge`-only — [`super::ensure_bridge_intent`]), and emit the
    //! action verbatim (cache bypassed, spec §2.5). There is NO broadcast and NO
    //! signer — `bridge status` never signs and is backend-agnostic.
    //!
    //! Criteria:
    //!
    //!  T1. **Status success envelope + verbatim action.** A planned Across bridge
    //!      action returns `version="v1"`, `success=true`, `error=None`,
    //!      `meta.command="bridge status"`, `meta.partial=false`, execution-path
    //!      cache bypass, and no provider routing (`meta.providers` empty); `data`
    //!      echoes the [`Action`] (`action_id`, `intent_type="bridge"`,
    //!      `provider="across"`, `status="planned"`).
    //!  T2. **Status reflects a completed action.** After a successful `bridge
    //!      submit`, `bridge status` reports `status="completed"` with the
    //!      `bridge_send` step `confirmed`.
    //!  T3. **action-id validation.** An empty / malformed `--action-id` is a
    //!      [`Code::Usage`] error (exit 2).
    //!  T4. **Unknown action → usage load error.** A well-formed but unknown
    //!      `--action-id` surfaces a [`Code::Usage`] `load action` error (exit 2).
    //!  T5. **Intent gate (bridge-only).** A persisted NON-`bridge` action queried
    //!      through `bridge status` is a [`Code::Usage`] error (exit 2) `action is
    //!      not a bridge intent`.
    //!  T6. **Full-binary exit codes.** Via `run_with_args`: malformed
    //!      `--action-id` → exit 2; well-formed unknown `--action-id` → exit 2.
    //!
    //! SKIPPED (owned elsewhere / wrong layer): the destination-settlement wait
    //! (Go `bridge status` does NOT poll settlement — settlement is owned by the
    //! submit-time engine path, pinned by `settlement_tests`); JSON
    //! field-declaration-order rendering — `defi-out` golden tests.

    use super::cli::{handle, BridgeCmd};
    use super::submit_app_tests;
    use crate::cli::run_with_args;
    use crate::ctx::AppCtx;
    use crate::execflags::StatusArgs;
    use defi_config::{MapEnv, Settings};
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::action::{Action, ExecutionBackend};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;

    const SIGNER_ADDR: &str = "0x14DDBd1fe5026E58A12eE8691cAEbFD24bb10eef";

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

    fn status_args(action_id: &str) -> StatusArgs {
        StatusArgs {
            action_id: Some(action_id.to_string()),
        }
    }

    async fn run_status(dir: &Path, args: StatusArgs) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(&ctx, BridgeCmd::Status(args)).await
    }

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn data_of(env: &Envelope) -> Value {
        env.data.clone().expect("status envelope carries `data`")
    }

    fn save_action(dir: &Path, action: &Action) {
        let store = ActionStore::open(dir.join("actions.db"), dir.join("actions.lock"))
            .expect("open action store");
        store.save(action).expect("persist fixture action");
    }

    fn env_with_home() -> (MapEnv, TempDir) {
        let tmp = TempDir::new().expect("tempdir");
        let env = MapEnv::with_home(tmp.path().to_path_buf());
        (env, tmp)
    }

    // --- T1: status success envelope + verbatim action ---------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn status_emits_success_envelope() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = submit_app_tests::plan_across(tmp.path(), SIGNER_ADDR).await;

        let env = run_status(tmp.path(), status_args(&action_id))
            .await
            .expect("bridge status should succeed for a planned bridge action");

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "bridge status");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);
        assert!(
            env.meta.providers.is_empty(),
            "status does no provider routing"
        );

        let data = data_of(&env);
        assert_eq!(data["action_id"], Value::from(action_id.as_str()));
        assert_eq!(data["intent_type"], Value::from("bridge"));
        assert_eq!(data["provider"], Value::from("across"));
        assert_eq!(data["status"], Value::from("planned"));
    }

    // --- T2: status reflects a completed action ----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn status_reflects_completed_action() {
        let tmp = TempDir::new().expect("tempdir");
        let action_id = submit_app_tests::plan_across(tmp.path(), SIGNER_ADDR).await;

        // Submit through the real handler so status reads the post-broadcast state.
        let ctx = AppCtx::new(exec_settings(tmp.path()));
        let submit_args = submit_app_tests::base_submit_args(&action_id);
        handle(&ctx, BridgeCmd::Submit(submit_args))
            .await
            .expect("bridge submit should complete offline");

        let env = run_status(tmp.path(), status_args(&action_id))
            .await
            .expect("status after submit");
        let data = data_of(&env);
        assert_eq!(data["status"], Value::from("completed"));
        let steps = data["steps"].as_array().expect("steps array");
        assert!(
            steps
                .iter()
                .any(|s| s["type"].as_str() == Some("bridge_send")
                    && s["status"].as_str() == Some("confirmed")),
            "the bridge_send step must be confirmed after submit: {steps:?}"
        );
    }

    // --- T3: action-id validation ------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn status_rejects_empty_action_id() {
        let tmp = TempDir::new().expect("tempdir");
        let err = run_status(tmp.path(), status_args(""))
            .await
            .expect_err("empty action id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn status_rejects_malformed_action_id() {
        let tmp = TempDir::new().expect("tempdir");
        let err = run_status(tmp.path(), status_args("act_xyz"))
            .await
            .expect_err("malformed action id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- T4: load failure for an unknown action ----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn status_unknown_action_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        let err = run_status(
            tmp.path(),
            status_args("act_0123456789abcdef0123456789abcdef"),
        )
        .await
        .expect_err("unknown action must surface a load (usage) error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    // --- T5: intent gate (bridge-only) -------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn status_rejects_non_bridge_intent() {
        let tmp = TempDir::new().expect("tempdir");
        let mut action = Action::new(
            "act_0123456789abcdef0123456789abcdef",
            "swap",
            "eip155:1",
            Default::default(),
        );
        action.from_address = SIGNER_ADDR.to_string();
        action.execution_backend = Some(ExecutionBackend::LegacyLocal);
        save_action(tmp.path(), &action);

        let err = run_status(tmp.path(), status_args(&action.action_id))
            .await
            .expect_err("non-bridge intent rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("action is not a bridge intent"),
            "got: {err}"
        );
    }

    // --- T6: full-binary exit codes ----------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn status_full_binary_malformed_action_id_exit_2() {
        let (env, _home) = env_with_home();
        let code =
            run_with_args(["defi", "bridge", "status", "--action-id", "act_xyz"], &env).await;
        assert_eq!(
            code, 2,
            "malformed --action-id must be a usage error (exit 2)"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn status_full_binary_unknown_action_id_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "bridge",
                "status",
                "--action-id",
                "act_0123456789abcdef0123456789abcdef",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "well-formed unknown --action-id must be a usage (load) error (exit 2)"
        );
    }
}

#[cfg(test)]
mod settlement_tests {
    //! # Success criteria — `bridge submit` destination-settlement wait (WS4)
    //!
    //! Go oracle: `internal/execution/executor.go` `verifyBridgeSettlement` /
    //! `waitForAcrossSettlement` / `waitForLiFiSettlement` (lines ~500-636), which
    //! the executor invokes after a `bridge_send` step's source-chain receipt
    //! confirms (`waitForStepConfirmation` line ~248) so a bridge submit only
    //! reports `completed` once the DESTINATION has settled. The Rust analogue is
    //! [`defi_execution::evm_executor::verify_bridge_settlement`] — the engine seam
    //! the `bridge submit` handler relies on. This module pins the settlement-wait
    //! contract the handler depends on, driven offline against `wiremock` so no
    //! live chain/provider is contacted.
    //!
    //! These tests build a REAL Across `bridge plan` (so the persisted
    //! `bridge_send` step carries the exact `settlement_provider` /
    //! `settlement_status_endpoint` / `settlement_origin_chain` /
    //! `settlement_destination_chain` metadata the production planner stamps),
    //! retarget the step's `settlement_status_endpoint` at a `wiremock` server
    //! (the offline seam), and assert the wait:
    //!
    //!  X1. **Across settlement completes on `filled`.** With a mock Across
    //!      `/deposit/status` returning `{"status":"filled","fillTx":"0x..."}`,
    //!      [`verify_bridge_settlement`] resolves `Ok(())` and records
    //!      `settlement_status="filled"` + `destination_tx_hash` on the step (Go
    //!      `waitForAcrossSettlement` success path). Proves the handler's
    //!      submit waits for Across destination settlement before `completed`.
    //!  X2. **Across settlement fails on `refunded`.** A `{"status":"refunded"}`
    //!      response is a [`Code::Unavailable`] `bridge settlement refunded` error
    //!      — the submit must NOT report success on a refund.
    //!  X3. **LiFi settlement completes on `DONE`.** A planned LiFi-provider bridge
    //!      step whose `settlement_status_endpoint` points at a mock `/status`
    //!      returning `{"status":"DONE",...}` resolves `Ok(())` and records the
    //!      destination tx hash (Go `waitForLiFiSettlement` success path).
    //!  X4. **LiFi settlement fails on `FAILED`.** A `{"status":"FAILED"}` response
    //!      is a [`Code::Unavailable`] `bridge settlement failed` error.
    //!
    //! The provider-specific query-param shaping + response parsing is owned by
    //! `defi-execution`; here we assert the END-TO-END settlement gate the bridge
    //! submit relies on, using the metadata a real `bridge plan` produces.

    use super::cli::{handle, BridgeCmd, PlanArgs};
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags};
    use defi_config::Settings;
    use defi_errors::Code;
    use defi_execution::action::{ActionStep, StepType};
    use defi_execution::evm_executor::verify_bridge_settlement;
    use defi_execution::ExecuteOptions;
    use std::path::Path;
    use std::time::Duration;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SENDER: &str = "0x00000000000000000000000000000000000000aa";

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

    /// Fast settlement-poll options so the offline loop polls immediately and
    /// times out quickly instead of waiting the default 2s/2m.
    fn fast_settlement_opts() -> ExecuteOptions {
        ExecuteOptions {
            poll_interval: Duration::from_millis(5),
            step_timeout: Duration::from_millis(200),
            gas_multiplier: 1.2,
            ..ExecuteOptions::default()
        }
    }

    /// Across `/swap/approval` execution route (one approval + the swap/bridge
    /// txn) — drives the offline `bridge plan` build.
    async fn across_swap_approval_mock() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "approvalTxns": [{
                        "chainId": 1,
                        "to": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
                        "data": "0x095ea7b3",
                        "value": "0"
                    }],
                    "swapTx": {
                        "chainId": 1,
                        "to": "0x5c7BCd6E7De5423a257D81B442095A1a6ced35C5",
                        "data": "0xad5425c6",
                        "value": "0x0"
                    },
                    "minOutputAmount": "990000",
                    "expectedOutputAmount": "995000",
                    "expectedFillTime": 5
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        server
    }

    fn plan_args(provider: &str) -> PlanArgs {
        PlanArgs {
            from: Some("1".to_string()),
            to: Some("10".to_string()),
            asset: Some("USDC".to_string()),
            to_asset: None,
            provider: Some(provider.to_string()),
            amount: Some("1000000".to_string()),
            amount_decimal: None,
            from_amount_for_gas: None,
            recipient: None,
            slippage_bps: 50,
            rpc_url: None,
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(SENDER.to_string()),
            },
            input: InputFlags::default(),
        }
    }

    /// Plan a real Across bridge action offline and return its persisted
    /// `bridge_send` step (carrying the production settlement metadata).
    async fn across_bridge_step(dir: &Path) -> ActionStep {
        let server = across_swap_approval_mock().await;
        let ctx = AppCtx::new(exec_settings(dir)).with_bridge_base(&server.uri());
        let env = handle(&ctx, BridgeCmd::Plan(plan_args("across")))
            .await
            .expect("across bridge plan for settlement fixture");
        let action: defi_execution::action::Action =
            serde_json::from_value(env.data.expect("plan data")).expect("deserialize action");
        action
            .steps
            .into_iter()
            .find(|s| s.step_type == StepType::Bridge)
            .expect("planned action carries a bridge_send step")
    }

    // --- X1: Across settlement completes on `filled` -----------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn across_settlement_completes_on_filled() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut step = across_bridge_step(tmp.path()).await;

        // Retarget the settlement status endpoint at the offline mock.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"status":"filled","fillTx":"0xdestination"}"#),
            )
            .mount(&server)
            .await;
        set_settlement_endpoint(&mut step, &server.uri());

        verify_bridge_settlement(&mut step, "0xsourcehash", &fast_settlement_opts())
            .await
            .expect("across destination settlement should complete on `filled`");

        let outs = step.expected_outputs.as_ref().expect("settlement outputs");
        assert_eq!(
            outs.get("settlement_status").and_then(|v| v.as_str()),
            Some("filled")
        );
        assert_eq!(
            outs.get("destination_tx_hash").and_then(|v| v.as_str()),
            Some("0xdestination")
        );
    }

    // --- X2: Across settlement fails on `refunded` -------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn across_settlement_fails_on_refunded() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut step = across_bridge_step(tmp.path()).await;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"status":"refunded","depositRefundTxHash":"0xrefund"}"#),
            )
            .mount(&server)
            .await;
        set_settlement_endpoint(&mut step, &server.uri());

        let err = verify_bridge_settlement(&mut step, "0xsourcehash", &fast_settlement_opts())
            .await
            .expect_err("a refunded Across settlement must fail the submit");
        assert_eq!(err.code, Code::Unavailable);
        assert!(
            err.to_string().contains("refunded"),
            "expected a refunded settlement error, got: {err}"
        );
    }

    // --- X3: LiFi settlement completes on `DONE` ---------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn lifi_settlement_completes_on_done() {
        // Build a LiFi-flavored bridge step directly (the LiFi plan build needs a
        // source-chain allowance RPC; here only the settlement metadata matters).
        let mut step = lifi_bridge_step();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"status":"DONE","substatus":"COMPLETED","receiving":{"txHash":"0xdestination"}}"#,
            ))
            .mount(&server)
            .await;
        set_settlement_endpoint(&mut step, &server.uri());

        verify_bridge_settlement(&mut step, "0xsourcehash", &fast_settlement_opts())
            .await
            .expect("lifi destination settlement should complete on `DONE`");

        let outs = step.expected_outputs.as_ref().expect("settlement outputs");
        assert_eq!(
            outs.get("settlement_status").and_then(|v| v.as_str()),
            Some("DONE")
        );
        assert_eq!(
            outs.get("destination_tx_hash").and_then(|v| v.as_str()),
            Some("0xdestination")
        );
    }

    // --- X4: LiFi settlement fails on `FAILED` -----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn lifi_settlement_fails_on_failed() {
        let mut step = lifi_bridge_step();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"status":"FAILED","substatusMessage":"bridge route failed"}"#,
                ),
            )
            .mount(&server)
            .await;
        set_settlement_endpoint(&mut step, &server.uri());

        let err = verify_bridge_settlement(&mut step, "0xsourcehash", &fast_settlement_opts())
            .await
            .expect_err("a FAILED LiFi settlement must fail the submit");
        assert_eq!(err.code, Code::Unavailable);
        assert!(
            err.to_string().contains("bridge settlement failed"),
            "expected a failed settlement error, got: {err}"
        );
    }

    // --- helpers -----------------------------------------------------------

    /// Overwrite the step's `settlement_status_endpoint` so the offline poll hits
    /// the wiremock server instead of the canonical live settlement URL.
    fn set_settlement_endpoint(step: &mut ActionStep, endpoint: &str) {
        let outs = step.expected_outputs.get_or_insert_with(Default::default);
        outs.insert(
            "settlement_status_endpoint".to_string(),
            serde_json::Value::String(endpoint.to_string()),
        );
    }

    /// A minimal LiFi `bridge_send` step carrying the LiFi settlement metadata a
    /// LiFi `bridge plan` stamps (`settlement_provider="lifi"`).
    fn lifi_bridge_step() -> ActionStep {
        use defi_execution::action::StepStatus;
        let mut outs = serde_json::Map::new();
        outs.insert("settlement_provider".into(), "lifi".into());
        ActionStep {
            step_id: "step-bridge".to_string(),
            step_type: StepType::Bridge,
            status: StepStatus::Submitted,
            chain_id: "eip155:1".to_string(),
            rpc_url: String::new(),
            description: String::new(),
            target: "0x1231DeB6f5749EF6Ce6943a275A1D3E7486F4EaE".to_string(),
            data: "0x1234".to_string(),
            value: "0x0".to_string(),
            calls: Vec::new(),
            expected_outputs: Some(outs),
            tx_hash: String::new(),
            error: String::new(),
        }
    }
}
