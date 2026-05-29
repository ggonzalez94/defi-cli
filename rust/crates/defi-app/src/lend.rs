//! `lend` command group handler (Go: `internal/app` — `newLendCommand` in
//! `runner.go` + `lend_execution_commands.go`).
//!
//! This module owns the **lend-command-specific** glue that sits between the
//! runner's cache-flow core ([`crate::runner`]) and the provider/execution
//! layers:
//!
//! * the lend read commands (`markets` / `rates` / `positions`) — provider
//!   routing, per-command limit truncation, and the `positions` input
//!   validation + capability gate;
//! * the lend execution verb → persisted-intent mapping (`lend_<verb>`) used by
//!   `supply|withdraw|borrow|repay {plan,submit,status}`.
//!
//! The provider-selection helpers shared with `yield`
//! (`normalize_lending_provider`, `parse_lend_position_type`) live in
//! [`crate::runner`]; the action-construction routing (`build_lend_action`)
//! lives in `defi_execution::builder`. This module deliberately does NOT
//! re-own those; it consumes them.

#![allow(dead_code, unused_variables)]

use defi_errors::{Code, Error};
use defi_execution::builder::LendVerb;
use defi_id::{parse_asset, parse_chain, Asset, Chain};
use defi_model::{LendMarket, LendPosition, LendRate, ProviderStatus};
use defi_providers::{
    LendPositionType, LendPositionsRequest, LendingPositionsProvider, LendingProvider,
};
use serde::Serialize;

use crate::protocols::status_from_result;

/// Cache TTL for `lend markets` (Go: `60 * time.Second`).
pub const LEND_MARKETS_TTL_SECS: u64 = 60;
/// Cache TTL for `lend rates` (Go: `30 * time.Second`).
pub const LEND_RATES_TTL_SECS: u64 = 30;
/// Cache TTL for `lend positions` (Go: `30 * time.Second`).
pub const LEND_POSITIONS_TTL_SECS: u64 = 30;

/// The default `--limit` for the lend read commands (Go default 20).
pub const DEFAULT_LIMIT: i64 = 20;

/// The lending providers that expose market/rate reads (Go `lendingProviders`
/// map keys).
const LENDING_PROVIDERS: [&str; 4] = ["aave", "morpho", "kamino", "moonwell"];

/// Cache-key request payload for `lend markets` / `lend rates`.
///
/// Field declaration order is ALPHABETICAL so the serde JSON matches the Go
/// `map[string]any{"provider","chain","asset","limit","rpc_url"}` payload (Go
/// `json.Marshal` of a map sorts keys), keeping cache keys cross-binary stable.
#[derive(Debug, Clone, Serialize)]
struct LendReadCacheReq {
    /// Parsed asset CAIP-19 id (`asset.AssetID`).
    asset: String,
    /// Parsed chain CAIP-2 id.
    chain: String,
    /// `--limit`.
    limit: i64,
    /// Canonical (normalized) provider name.
    provider: String,
    /// Trimmed `--rpc-url`.
    rpc_url: String,
}

/// Cache-key request payload for `lend positions`.
///
/// Alphabetical field order matches the Go map JSON (`address, asset, chain,
/// limit, provider, rpc_url, type`).
#[derive(Debug, Clone, Serialize)]
struct LendPositionsCacheReq {
    /// Cache account (lowercased on EVM chains, verbatim otherwise).
    address: String,
    /// Asset filter cache value (see [`chain_asset_filter_cache_value`]).
    asset: String,
    /// Parsed chain CAIP-2 id.
    chain: String,
    /// `--limit`.
    limit: i64,
    /// Canonical (normalized) provider name.
    provider: String,
    /// Trimmed `--rpc-url`.
    rpc_url: String,
    /// Position-type filter wire string.
    r#type: String,
}

/// Truncate a list of lend markets to `limit`.
///
/// Parity with Go `applyLendMarketLimit`: a non-positive `limit`, or a list
/// already at/under the limit, is returned unchanged; otherwise the first
/// `limit` items are kept (order preserved).
pub fn apply_lend_market_limit(mut items: Vec<LendMarket>, limit: i64) -> Vec<LendMarket> {
    if limit <= 0 || (items.len() as i64) <= limit {
        return items;
    }
    items.truncate(limit as usize);
    items
}

/// Truncate a list of lend rates to `limit`.
///
/// Parity with Go `applyLendRateLimit` (same semantics as
/// [`apply_lend_market_limit`]).
pub fn apply_lend_rate_limit(mut items: Vec<LendRate>, limit: i64) -> Vec<LendRate> {
    if limit <= 0 || (items.len() as i64) <= limit {
        return items;
    }
    items.truncate(limit as usize);
    items
}

/// The persisted action intent type for a lend execution verb.
///
/// Parity with Go `expectedIntent := "lend_" + string(verb)` in
/// `lend_execution_commands.go`. `plan` writes this onto the action; `submit` /
/// `status` reject an action whose `intent_type` does not match.
pub fn lend_verb_intent(verb: LendVerb) -> String {
    let suffix = match verb {
        LendVerb::Supply => "supply",
        LendVerb::Withdraw => "withdraw",
        LendVerb::Borrow => "borrow",
        LendVerb::Repay => "repay",
    };
    format!("lend_{suffix}")
}

/// A validated `lend positions` query (the inputs needed to build a
/// [`LendPositionsRequest`] for the selected provider).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LendPositionsQuery {
    /// Canonical (normalized) lending provider name.
    pub provider: String,
    /// Parsed chain.
    pub chain: Chain,
    /// The position-owner account (verbatim, un-lowercased — caller lowercases
    /// for the cache key on EVM chains).
    pub account: String,
    /// Parsed position-type filter (empty input defaults to
    /// [`LendPositionType::All`]).
    pub position_type: LendPositionType,
}

/// Validate the pre-provider inputs of `lend positions`.
///
/// Parity with the `positionsCmd` `RunE` guard order in `runner.go`:
/// 1. `--provider` is required (usage) — normalized via the runner helper;
/// 2. `--chain` parses (delegates to `defi_id::parse_chain`);
/// 3. `--address` is required (usage);
/// 4. on EVM chains, `--address` must be a valid hex address (usage);
/// 5. `--type` parses (usage on an unknown value), empty → `All`.
///
/// On success returns the [`LendPositionsQuery`]; the provider is NOT yet
/// consulted (matching the Go ordering where validation precedes provider
/// selection / the cached fetch closure).
pub fn validate_lend_positions_input(
    provider: &str,
    chain_arg: &str,
    address: &str,
    type_arg: &str,
) -> Result<LendPositionsQuery, Error> {
    // 1. `--provider` is required (normalized via the runner helper).
    let provider_name = crate::runner::normalize_lending_provider(provider);
    if provider_name.is_empty() {
        return Err(Error::new(Code::Usage, "--provider is required"));
    }

    // 2. `--chain` parses (surfaces the id parse error verbatim).
    let chain = defi_id::parse_chain(chain_arg)?;

    // 3. `--address` is required.
    let account = address.trim().to_string();
    if account.is_empty() {
        return Err(Error::new(Code::Usage, "--address is required"));
    }

    // 4. On EVM chains, `--address` must be a valid hex address (parity with
    //    go-ethereum `common.IsHexAddress`).
    if chain.is_evm() && !defi_evm::address::is_hex_address(&account) {
        return Err(Error::new(
            Code::Usage,
            "--address must be a valid EVM hex address",
        ));
    }

    // 5. `--type` parses (empty → `All`).
    let position_type = crate::runner::parse_lend_position_type(type_arg)?;

    Ok(LendPositionsQuery {
        provider: provider_name,
        chain,
        account,
        position_type,
    })
}

/// Fetch lend positions, enforcing the provider-capability gate.
///
/// Parity with the Go interface assertion
/// `provider.(providers.LendingPositionsProvider)`: a selected lending provider
/// that does not implement positions yields a [`defi_errors::Code::Unsupported`]
/// error whose message contains `"does not support positions"` (modeled here as
/// `positions == None`). Otherwise the request is forwarded to the provider.
pub async fn fetch_lend_positions(
    provider_name: &str,
    positions: Option<&dyn LendingPositionsProvider>,
    req: LendPositionsRequest,
) -> Result<Vec<LendPosition>, Error> {
    match positions {
        None => Err(Error::new(
            Code::Unsupported,
            format!("lending provider {provider_name} does not support positions"),
        )),
        Some(provider) => provider.lend_positions(req).await,
    }
}

// ---------------------------------------------------------------------------
// chain/asset parsing helpers (mirror the Go runner free functions).
// ---------------------------------------------------------------------------

/// Parse a required `--chain` + `--asset` pair (Go `parseChainAsset`).
///
/// Both flags are required (empty input is a usage error reported BEFORE
/// parsing); the chain parses first, then the asset is resolved against it.
pub fn parse_chain_asset(chain_arg: &str, asset_arg: &str) -> Result<(Chain, Asset), Error> {
    if chain_arg.trim().is_empty() {
        return Err(Error::new(Code::Usage, "--chain is required"));
    }
    if asset_arg.trim().is_empty() {
        return Err(Error::new(Code::Usage, "--asset is required"));
    }
    let chain = parse_chain(chain_arg)?;
    let asset = parse_asset(asset_arg, &chain)?;
    Ok((chain, asset))
}

/// Parse an OPTIONAL `--asset` filter against an already-parsed chain (Go
/// `parseOptionalChainAsset`).
///
/// An empty input yields a default (unfiltered) [`Asset`]. A value that parses
/// is returned as-is. Otherwise, if it looks like a bare symbol filter (not an
/// address/CAIP), it falls back to a symbol-only asset; an address/CAIP that
/// fails to parse surfaces the parse error.
pub fn parse_optional_chain_asset(chain: &Chain, asset_arg: &str) -> Result<Asset, Error> {
    let asset_arg = asset_arg.trim();
    if asset_arg.is_empty() {
        return Ok(Asset::default());
    }
    match parse_asset(asset_arg, chain) {
        Ok(asset) => Ok(asset),
        Err(err) => {
            if looks_like_address_or_caip(asset_arg) || !looks_like_symbol_filter(asset_arg) {
                return Err(err);
            }
            Ok(Asset {
                chain_id: chain.caip2.clone(),
                symbol: asset_arg.to_ascii_uppercase(),
                ..Asset::default()
            })
        }
    }
}

/// Whether the input looks like an EVM address or a CAIP id (Go
/// `looksLikeAddressOrCAIP`).
pub(crate) fn looks_like_address_or_caip(input: &str) -> bool {
    let norm = input.trim().to_ascii_lowercase();
    norm.starts_with("eip155:") || (norm.starts_with("0x") && norm.len() == 42)
}

/// Whether the input looks like a bare token-symbol filter (Go
/// `looksLikeSymbolFilter`): non-empty, <= 64 chars, no whitespace/`:`/`/`.
pub(crate) fn looks_like_symbol_filter(input: &str) -> bool {
    let norm = input.trim();
    if norm.is_empty() || norm.len() > 64 {
        return false;
    }
    !norm.contains([' ', '\t', '\r', '\n', ':', '/'])
}

/// The cache-stable string for an optional asset filter (Go
/// `chainAssetFilterCacheValue`): empty raw input → `""`; a resolved asset id →
/// the CAIP-19 id; a symbol-only asset → `"symbol:<UPPER>"`; otherwise
/// `"raw:<UPPER>"`.
pub fn chain_asset_filter_cache_value(asset: &Asset, raw_input: &str) -> String {
    if raw_input.trim().is_empty() {
        return String::new();
    }
    if !asset.asset_id.trim().is_empty() {
        return asset.asset_id.clone();
    }
    if !asset.symbol.trim().is_empty() {
        return format!("symbol:{}", asset.symbol.trim().to_ascii_uppercase());
    }
    format!("raw:{}", raw_input.trim().to_ascii_uppercase())
}

// ---------------------------------------------------------------------------
// provider routing + cache-key construction.
// ---------------------------------------------------------------------------

/// Compute the cache key for a lend read command (Go `cacheKey`):
/// `hex(sha256(path | CACHE_PAYLOAD_SCHEMA_VERSION | json(req)))`.
fn cache_key<T: Serialize>(command_path: &str, req: &T) -> String {
    crate::protocols::cache_key(command_path, req)
}

/// Select the markets/rates provider for a normalized provider name.
///
/// Mirrors Go `selectLendingProvider`: an unknown name is an
/// [`Code::Unsupported`] error. The selected provider is returned as a boxed
/// trait object; the Moonwell adapter (the only on-chain reader) has the
/// `--rpc-url` override applied (Go `applyRPCOverride`, interface-checked).
fn select_lending_provider(
    ctx: &crate::ctx::AppCtx,
    provider_name: &str,
    rpc_url: &str,
) -> Result<Box<dyn LendingProvider>, Error> {
    let http = ctx.http_client();
    let provider: Box<dyn LendingProvider> = match provider_name {
        "aave" => Box::new(defi_providers::aave::Client::new(http)),
        "morpho" => Box::new(defi_providers::morpho::Client::new(http)),
        "kamino" => Box::new(defi_providers::kamino::Client::new(http)),
        "moonwell" => {
            let mut client = defi_providers::moonwell::Client::new();
            let trimmed = rpc_url.trim();
            if !trimmed.is_empty() {
                client.set_rpc_override(trimmed);
            }
            Box::new(client)
        }
        _ => {
            return Err(Error::new(
                Code::Unsupported,
                format!("unsupported lending provider: {provider_name}"),
            ))
        }
    };
    Ok(provider)
}

/// Select the positions provider for a normalized provider name.
///
/// Mirrors Go `selectLendingProvider` + the `LendingPositionsProvider`
/// interface assertion: an unknown name is [`Code::Unsupported`]; a known name
/// that does not implement positions (kamino) returns `Ok(None)` so the
/// capability gate ([`fetch_lend_positions`]) can surface the canonical
/// "does not support positions" error.
fn select_lending_positions_provider(
    ctx: &crate::ctx::AppCtx,
    provider_name: &str,
    rpc_url: &str,
) -> Result<Option<Box<dyn LendingPositionsProvider>>, Error> {
    let http = ctx.http_client();
    let provider: Option<Box<dyn LendingPositionsProvider>> = match provider_name {
        "aave" => Some(Box::new(defi_providers::aave::Client::new(http))),
        "morpho" => Some(Box::new(defi_providers::morpho::Client::new(http))),
        // Kamino implements LendingProvider but NOT positions (Go capability gate).
        "kamino" => None,
        "moonwell" => {
            let mut client = defi_providers::moonwell::Client::new();
            let trimmed = rpc_url.trim();
            if !trimmed.is_empty() {
                client.set_rpc_override(trimmed);
            }
            Some(Box::new(client))
        }
        _ => {
            return Err(Error::new(
                Code::Unsupported,
                format!("unsupported lending provider: {provider_name}"),
            ))
        }
    };
    Ok(provider)
}

// ---------------------------------------------------------------------------
// read-command builders (data + captured provider status).
// ---------------------------------------------------------------------------

/// A resolved lend read fetch: the JSON `data` payload + the single captured
/// provider [`ProviderStatus`].
pub struct LendOutcome {
    /// The fetched list, serialized verbatim as a JSON array for `data`.
    pub data: serde_json::Value,
    /// The single lending-provider status captured for this fetch.
    pub provider: ProviderStatus,
}

/// Build a `lend markets` outcome: select the provider, fetch, apply the limit,
/// capture status.
async fn run_markets(
    ctx: &crate::ctx::AppCtx,
    provider_name: &str,
    chain: &Chain,
    asset: &Asset,
    limit: i64,
    rpc_url: &str,
) -> Result<LendOutcome, Error> {
    let provider = select_lending_provider(ctx, provider_name, rpc_url)?;
    let res = provider
        .lend_markets(provider_name, chain.clone(), asset.clone())
        .await;
    let status = ProviderStatus {
        name: provider.info().name,
        status: status_from_result(&res),
        latency_ms: 0,
    };
    let rows = res?;
    let rows = apply_lend_market_limit(rows, limit);
    let data = serde_json::to_value(&rows)
        .map_err(|e| Error::wrap(Code::Internal, "serialize lend markets", e))?;
    Ok(LendOutcome {
        data,
        provider: status,
    })
}

/// Build a `lend rates` outcome.
async fn run_rates(
    ctx: &crate::ctx::AppCtx,
    provider_name: &str,
    chain: &Chain,
    asset: &Asset,
    limit: i64,
    rpc_url: &str,
) -> Result<LendOutcome, Error> {
    let provider = select_lending_provider(ctx, provider_name, rpc_url)?;
    let res = provider
        .lend_rates(provider_name, chain.clone(), asset.clone())
        .await;
    let status = ProviderStatus {
        name: provider.info().name,
        status: status_from_result(&res),
        latency_ms: 0,
    };
    let rows = res?;
    let rows = apply_lend_rate_limit(rows, limit);
    let data = serde_json::to_value(&rows)
        .map_err(|e| Error::wrap(Code::Internal, "serialize lend rates", e))?;
    Ok(LendOutcome {
        data,
        provider: status,
    })
}

/// Build a `lend positions` outcome: select the positions-capable provider
/// (capability gate), fetch, capture status.
async fn run_positions(
    ctx: &crate::ctx::AppCtx,
    provider_name: &str,
    req: LendPositionsRequest,
) -> Result<LendOutcome, Error> {
    let rpc_url = req.rpc_url.clone();
    let provider = select_lending_positions_provider(ctx, provider_name, &rpc_url)?;
    // Capture the provider name for the status row (the boxed provider may be
    // None when the selected provider lacks positions; the gate surfaces the
    // canonical Unsupported error in that case).
    let provider_label = provider
        .as_ref()
        .map(|p| p.info().name)
        .unwrap_or_else(|| provider_name.to_string());

    let res = fetch_lend_positions(provider_name, provider.as_deref(), req).await;
    let status = ProviderStatus {
        name: provider_label,
        status: status_from_result(&res),
        latency_ms: 0,
    };
    let rows = res?;
    let data = serde_json::to_value(&rows)
        .map_err(|e| Error::wrap(Code::Internal, "serialize lend positions", e))?;
    Ok(LendOutcome {
        data,
        provider: status,
    })
}

/// clap parsing + handler for the `lend` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::{Code, Error};
    use defi_model::Envelope;

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, SubmitArgs};

    /// `lend` subcommands: read data + the four execution verbs.
    #[derive(Subcommand, Debug)]
    pub enum LendCmd {
        /// List lending markets.
        Markets(MarketsArgs),
        /// List lending rates.
        Rates(MarketsArgs),
        /// List lending positions for an account address.
        Positions(PositionsArgs),
        /// Supply assets to a lending protocol.
        #[command(subcommand)]
        Supply(LendVerbCmd),
        /// Withdraw assets from a lending protocol.
        #[command(subcommand)]
        Withdraw(LendVerbCmd),
        /// Borrow assets from a lending protocol.
        #[command(subcommand)]
        Borrow(LendVerbCmd),
        /// Repay borrowed assets on a lending protocol.
        #[command(subcommand)]
        Repay(LendVerbCmd),
    }

    impl LendCmd {
        /// The full path tail (e.g. `markets`, `supply plan`) for `meta.command`.
        pub fn path(&self) -> String {
            match self {
                LendCmd::Markets(_) => "markets".to_string(),
                LendCmd::Rates(_) => "rates".to_string(),
                LendCmd::Positions(_) => "positions".to_string(),
                LendCmd::Supply(v) => format!("supply {}", v.path()),
                LendCmd::Withdraw(v) => format!("withdraw {}", v.path()),
                LendCmd::Borrow(v) => format!("borrow {}", v.path()),
                LendCmd::Repay(v) => format!("repay {}", v.path()),
            }
        }
    }

    /// `lend markets` / `lend rates` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct MarketsArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Asset (symbol/address/CAIP-19).
        #[arg(long)]
        pub asset: Option<String>,
        /// Lending provider (aave, morpho, kamino, moonwell).
        #[arg(long)]
        pub provider: Option<String>,
        /// Maximum rows to return.
        #[arg(long, default_value_t = 20)]
        pub limit: i64,
        /// Optional RPC URL override for on-chain providers.
        #[arg(long = "rpc-url")]
        pub rpc_url: Option<String>,
    }

    /// `lend positions` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct PositionsArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Position owner address.
        #[arg(long)]
        pub address: Option<String>,
        /// Optional asset filter (symbol/address/CAIP-19).
        #[arg(long)]
        pub asset: Option<String>,
        /// Lending provider (aave, morpho, moonwell).
        #[arg(long)]
        pub provider: Option<String>,
        /// Position type filter (all|supply|borrow|collateral).
        #[arg(long, default_value = "all")]
        pub r#type: String,
        /// Maximum positions to return.
        #[arg(long, default_value_t = 20)]
        pub limit: i64,
        /// Optional RPC URL override used by providers that need on-chain reads.
        #[arg(long = "rpc-url")]
        pub rpc_url: Option<String>,
    }

    /// The `plan` / `submit` / `status` sub-subcommands shared by every lend verb.
    #[derive(Subcommand, Debug)]
    pub enum LendVerbCmd {
        /// Create and persist a lend action plan.
        Plan(LendPlanArgs),
        /// Execute an existing lend action.
        Submit(SubmitArgs),
        /// Get lend action status.
        Status(StatusArgs),
    }

    impl LendVerbCmd {
        /// The leaf path token (`plan`/`submit`/`status`).
        pub fn path(&self) -> &'static str {
            match self {
                LendVerbCmd::Plan(_) => "plan",
                LendVerbCmd::Submit(_) => "submit",
                LendVerbCmd::Status(_) => "status",
            }
        }
    }

    /// `lend <verb> plan` flags (shared across supply/withdraw/borrow/repay).
    #[derive(Args, Debug, Clone, Default)]
    pub struct LendPlanArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Asset symbol/address/CAIP-19.
        #[arg(long)]
        pub asset: Option<String>,
        /// Amount in base units.
        #[arg(long)]
        pub amount: Option<String>,
        /// Amount in decimal units.
        #[arg(long = "amount-decimal")]
        pub amount_decimal: Option<String>,
        /// Lending provider (aave|morpho|moonwell).
        #[arg(long)]
        pub provider: Option<String>,
        /// Recipient address (defaults to the resolved sender address).
        #[arg(long)]
        pub recipient: Option<String>,
        /// Position owner address (defaults to the resolved sender address).
        #[arg(long = "on-behalf-of")]
        pub on_behalf_of: Option<String>,
        /// Aave borrow/repay mode (1=stable,2=variable).
        #[arg(long = "interest-rate-mode", default_value_t = 2)]
        pub interest_rate_mode: i64,
        /// Morpho market unique key (required for --provider morpho).
        #[arg(long = "market-id")]
        pub market_id: Option<String>,
        /// Aave pool address override.
        #[arg(long = "pool-address")]
        pub pool_address: Option<String>,
        /// Aave pool address provider override.
        #[arg(long = "pool-address-provider")]
        pub pool_address_provider: Option<String>,
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

    /// Handle `lend <sub>`.
    ///
    /// Reads (`markets`/`rates`/`positions`) are WS2 (wired here); execution
    /// verbs are WS3 (`plan`) / WS4 (`submit`/`status`). All route here;
    /// unimplemented leaves return a typed `Unsupported` error (never
    /// `unknown command`).
    pub async fn handle(ctx: &AppCtx, cmd: LendCmd) -> Result<Envelope, Error> {
        match cmd {
            LendCmd::Markets(args) => handle_markets(ctx, args).await,
            LendCmd::Rates(args) => handle_rates(ctx, args).await,
            LendCmd::Positions(args) => handle_positions(ctx, args).await,
            other => {
                let path = format!("lend {}", other.path());
                let ws = if path.ends_with("plan") { "WS3" } else { "WS4" };
                Err(AppCtx::unimplemented(&path, ws))
            }
        }
    }

    /// Handle `lend markets`: provider-required validation → cache flow.
    async fn handle_markets(ctx: &AppCtx, args: MarketsArgs) -> Result<Envelope, Error> {
        let path = "lend markets";
        let provider_name = require_provider(args.provider.as_deref())?;
        let chain_arg = args.chain.clone().unwrap_or_default();
        let asset_arg = args.asset.clone().unwrap_or_default();
        let (chain, asset) = super::parse_chain_asset(&chain_arg, &asset_arg)?;
        let rpc_url = args.rpc_url.clone().unwrap_or_default();

        let req = super::LendReadCacheReq {
            asset: asset.asset_id.clone(),
            chain: chain.caip2.clone(),
            limit: args.limit,
            provider: provider_name.clone(),
            rpc_url: rpc_url.trim().to_string(),
        };
        let key = super::cache_key(path, &req);
        let ttl = std::time::Duration::from_secs(super::LEND_MARKETS_TTL_SECS);
        ctx.run_cached_command(path, &key, ttl, || {
            finalize(
                &provider_name,
                crate::ctx::block_on_fetch(super::run_markets(
                    ctx,
                    &provider_name,
                    &chain,
                    &asset,
                    args.limit,
                    &rpc_url,
                )),
            )
        })
    }

    /// Handle `lend rates`.
    async fn handle_rates(ctx: &AppCtx, args: MarketsArgs) -> Result<Envelope, Error> {
        let path = "lend rates";
        let provider_name = require_provider(args.provider.as_deref())?;
        let chain_arg = args.chain.clone().unwrap_or_default();
        let asset_arg = args.asset.clone().unwrap_or_default();
        let (chain, asset) = super::parse_chain_asset(&chain_arg, &asset_arg)?;
        let rpc_url = args.rpc_url.clone().unwrap_or_default();

        let req = super::LendReadCacheReq {
            asset: asset.asset_id.clone(),
            chain: chain.caip2.clone(),
            limit: args.limit,
            provider: provider_name.clone(),
            rpc_url: rpc_url.trim().to_string(),
        };
        let key = super::cache_key(path, &req);
        let ttl = std::time::Duration::from_secs(super::LEND_RATES_TTL_SECS);
        ctx.run_cached_command(path, &key, ttl, || {
            finalize(
                &provider_name,
                crate::ctx::block_on_fetch(super::run_rates(
                    ctx,
                    &provider_name,
                    &chain,
                    &asset,
                    args.limit,
                    &rpc_url,
                )),
            )
        })
    }

    /// Handle `lend positions`: input validation (provider/chain/address/type)
    /// → capability gate → cache flow.
    async fn handle_positions(ctx: &AppCtx, args: PositionsArgs) -> Result<Envelope, Error> {
        let path = "lend positions";
        let provider_name = require_provider(args.provider.as_deref())?;
        let chain_arg = args.chain.clone().unwrap_or_default();
        let address = args.address.clone().unwrap_or_default();
        let asset_arg = args.asset.clone().unwrap_or_default();

        // Validate provider/chain/address/type ordering (matches the Go guard
        // order); this also normalizes the provider + parses the position type.
        let validated = super::validate_lend_positions_input(
            &provider_name,
            &chain_arg,
            &address,
            &args.r#type,
        )?;
        let chain = validated.chain;
        let account = validated.account;
        let position_type = validated.position_type;

        let asset = super::parse_optional_chain_asset(&chain, &asset_arg)?;
        let rpc_url = args.rpc_url.clone().unwrap_or_default();

        // Cache account is lowercased on EVM chains (Go cacheAccount).
        let cache_account = if chain.is_evm() {
            account.to_ascii_lowercase()
        } else {
            account.clone()
        };
        let req = super::LendPositionsCacheReq {
            address: cache_account,
            asset: super::chain_asset_filter_cache_value(&asset, &asset_arg),
            chain: chain.caip2.clone(),
            limit: args.limit,
            provider: provider_name.clone(),
            rpc_url: rpc_url.trim().to_string(),
            r#type: position_type.as_str().to_string(),
        };
        let key = super::cache_key(path, &req);
        let ttl = std::time::Duration::from_secs(super::LEND_POSITIONS_TTL_SECS);

        let positions_req = defi_providers::LendPositionsRequest {
            chain,
            account,
            asset,
            position_type,
            limit: args.limit,
            rpc_url: rpc_url.trim().to_string(),
        };
        ctx.run_cached_command(path, &key, ttl, || {
            finalize(
                &provider_name,
                crate::ctx::block_on_fetch(super::run_positions(
                    ctx,
                    &provider_name,
                    positions_req,
                )),
            )
        })
    }

    /// Require a non-empty, normalized `--provider` (Go
    /// `--provider is required`). Returns the canonical provider name.
    fn require_provider(provider: Option<&str>) -> Result<String, Error> {
        let normalized = crate::runner::normalize_lending_provider(provider.unwrap_or_default());
        if normalized.is_empty() {
            return Err(Error::new(Code::Usage, "--provider is required"));
        }
        Ok(normalized)
    }

    /// Convert a [`super::LendOutcome`] result into the cache-flow fetch outcome
    /// tuple expected by `run_cached_command`. On error, surface one provider
    /// status row keyed on the normalized provider name (Go statuses capture).
    #[allow(clippy::type_complexity)]
    fn finalize(
        provider_name: &str,
        outcome: Result<super::LendOutcome, Error>,
    ) -> Result<
        crate::runner::FetchOutcome,
        (Vec<defi_model::ProviderStatus>, Vec<String>, bool, Error),
    > {
        match outcome {
            Ok(o) => Ok(crate::runner::FetchOutcome {
                data: o.data,
                providers: vec![o.provider],
                warnings: Vec::new(),
                partial: false,
            }),
            Err(err) => {
                let status = defi_model::ProviderStatus {
                    name: provider_name.to_string(),
                    status: super::status_from_result::<()>(&Err(Error::new(err.code, ""))),
                    latency_ms: 0,
                };
                Err((vec![status], Vec::new(), false, err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::lend` (Go: `internal/app` lend command
    //! group: `newLendCommand` in `runner.go` + `lend_execution_commands.go`)
    //!
    //! This module owns the **lend-command glue**. "Correct" means it preserves
    //! the runner-owned lend behaviors AND the stable machine contract (design
    //! spec §2.2 exit codes, §2.4 ids/amounts). The provider-selection helpers
    //! (`normalize_lending_provider`, `parse_lend_position_type`) and the
    //! cache-flow core are owned by [`crate::runner`] and are NOT re-asserted
    //! here; the action-construction routing is owned by
    //! `defi_execution::builder` and is NOT re-asserted here. Criteria:
    //!
    //! 1. **Per-command limit truncation.** `apply_lend_market_limit` /
    //!    `apply_lend_rate_limit`: a non-positive limit, or a list already
    //!    at/under the limit, is returned UNCHANGED (no realloc/drop); a list
    //!    longer than the limit keeps exactly the first `limit` items in order.
    //!    (Go `applyLendMarketLimit` / `applyLendRateLimit`.)
    //! 2. **Execution intent mapping.** `lend_verb_intent(verb)` is exactly
    //!    `"lend_<verb>"` (`lend_supply`/`lend_withdraw`/`lend_borrow`/
    //!    `lend_repay`) — the persisted `Action.intent_type` that `plan` writes
    //!    and that `submit`/`status` match against. (Go
    //!    `expectedIntent := "lend_" + string(verb)`.)
    //! 3. **`positions` input validation order + exit codes.**
    //!    `validate_lend_positions_input` mirrors the Go `positionsCmd` guard
    //!    order, every failure carrying [`Code::Usage`] (exit 2):
    //!    a. empty `--provider` → usage error BEFORE chain parsing;
    //!    b. an unparseable `--chain` surfaces the id error;
    //!    c. empty `--address` → usage error;
    //!    d. on an EVM chain, a non-hex `--address` → usage error (parity with
    //!    go-ethereum `common.IsHexAddress`);
    //!    e. an unknown `--type` → usage error;
    //!    and on success it returns the normalized provider, parsed chain, the
    //!    verbatim account, and the parsed position type (empty → `All`).
    //!    (Ported from `TestRunnerLendPositionsRejectsInvalidType`,
    //!    `TestRunnerLendPositionsRejectsInvalidEVMAddress`, and the happy-path
    //!    setup in `TestRunnerLendPositionsCallsProvider`.)
    //! 4. **Provider-capability gate.** `fetch_lend_positions` with
    //!    `positions == None` (the selected lending provider does not implement
    //!    positions) fails with [`Code::Unsupported`] (exit 13) and a message
    //!    containing `"does not support positions"`, WITHOUT touching the
    //!    provider. (Ported from
    //!    `TestRunnerLendPositionsRequiresProviderCapability`.)
    //! 5. **Provider request forwarding.** `fetch_lend_positions` with a capable
    //!    provider forwards the request verbatim (account, asset filter, type,
    //!    limit) exactly once and returns the provider's rows. (Ported from
    //!    `TestRunnerLendPositionsCallsProvider`.)
    //!
    //! SKIPPED (Go internal-detail / wrong-module): cobra flag wiring,
    //! cache-key construction (runner concern), and the full `plan/submit/status`
    //! signer/backend plumbing (execution-crate concern, covered there).

    use super::*;
    use async_trait::async_trait;
    use defi_errors::{exit_code, Code};
    use defi_id::{parse_chain, Asset};
    use defi_model::{AmountInfo, ProviderInfo};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // --- fixtures ----------------------------------------------------------

    fn market(provider: &str) -> LendMarket {
        LendMarket {
            protocol: provider.to_string(),
            provider: provider.to_string(),
            chain_id: "eip155:1".to_string(),
            asset_id: "eip155:1/erc20:0xa0b8".to_string(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            supply_apy: 1.0,
            borrow_apy: 2.0,
            tvl_usd: 3.0,
            liquidity_usd: 4.0,
            source_url: String::new(),
            fetched_at: "2026-05-28T00:00:00Z".to_string(),
        }
    }

    fn rate(provider: &str) -> LendRate {
        LendRate {
            protocol: provider.to_string(),
            provider: provider.to_string(),
            chain_id: "eip155:1".to_string(),
            asset_id: "eip155:1/erc20:0xa0b8".to_string(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            supply_apy: 1.0,
            borrow_apy: 2.0,
            utilization: 0.5,
            source_url: String::new(),
            fetched_at: "2026-05-28T00:00:00Z".to_string(),
        }
    }

    fn position(provider: &str) -> LendPosition {
        LendPosition {
            protocol: provider.to_string(),
            provider: provider.to_string(),
            chain_id: "eip155:1".to_string(),
            account_address: "0x000000000000000000000000000000000000dead".to_string(),
            position_type: "collateral".to_string(),
            asset_id: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".to_string(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            amount: AmountInfo::default(),
            amount_usd: 0.0,
            apy: 0.0,
            source_url: String::new(),
            fetched_at: "2026-05-28T00:00:00Z".to_string(),
        }
    }

    /// A fake positions-capable provider that records the request it received.
    struct FakeLendingPositionsProvider {
        name: String,
        rows: Vec<LendPosition>,
        calls: AtomicUsize,
        last_req: std::sync::Mutex<Option<LendPositionsRequest>>,
    }

    impl FakeLendingPositionsProvider {
        fn new(name: &str, rows: Vec<LendPosition>) -> Self {
            Self {
                name: name.to_string(),
                rows,
                calls: AtomicUsize::new(0),
                last_req: std::sync::Mutex::new(None),
            }
        }
    }

    impl defi_providers::Provider for FakeLendingPositionsProvider {
        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                name: self.name.clone(),
                provider_type: "lending".to_string(),
                requires_key: false,
                capabilities: vec![
                    "lend.markets".to_string(),
                    "lend.rates".to_string(),
                    "lend.positions".to_string(),
                ],
                key_env_var_name: String::new(),
                capability_auth: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl LendingPositionsProvider for FakeLendingPositionsProvider {
        async fn lend_positions(
            &self,
            req: LendPositionsRequest,
        ) -> Result<Vec<LendPosition>, Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_req.lock().unwrap() = Some(req);
            Ok(self.rows.clone())
        }
    }

    // --- 1. limit truncation ----------------------------------------------

    #[test]
    fn apply_lend_market_limit_truncates_and_passes_through() {
        let items = vec![market("aave"), market("morpho"), market("moonwell")];
        // non-positive limit => unchanged.
        assert_eq!(apply_lend_market_limit(items.clone(), 0).len(), 3);
        assert_eq!(apply_lend_market_limit(items.clone(), -1).len(), 3);
        // limit >= len => unchanged.
        assert_eq!(apply_lend_market_limit(items.clone(), 3).len(), 3);
        assert_eq!(apply_lend_market_limit(items.clone(), 10).len(), 3);
        // limit < len => first `limit` items, order preserved.
        let truncated = apply_lend_market_limit(items.clone(), 2);
        assert_eq!(truncated.len(), 2);
        assert_eq!(truncated[0].provider, "aave");
        assert_eq!(truncated[1].provider, "morpho");
    }

    #[test]
    fn apply_lend_rate_limit_truncates_and_passes_through() {
        let items = vec![rate("aave"), rate("morpho"), rate("moonwell")];
        assert_eq!(apply_lend_rate_limit(items.clone(), 0).len(), 3);
        assert_eq!(apply_lend_rate_limit(items.clone(), 5).len(), 3);
        let truncated = apply_lend_rate_limit(items.clone(), 1);
        assert_eq!(truncated.len(), 1);
        assert_eq!(truncated[0].provider, "aave");
    }

    // --- 2. execution intent mapping --------------------------------------

    #[test]
    fn lend_verb_intent_is_lend_prefixed_verb() {
        assert_eq!(lend_verb_intent(LendVerb::Supply), "lend_supply");
        assert_eq!(lend_verb_intent(LendVerb::Withdraw), "lend_withdraw");
        assert_eq!(lend_verb_intent(LendVerb::Borrow), "lend_borrow");
        assert_eq!(lend_verb_intent(LendVerb::Repay), "lend_repay");
    }

    // --- 3. positions input validation ------------------------------------

    #[test]
    fn positions_input_requires_provider_before_chain() {
        // empty provider => usage, even with an otherwise-bogus chain (the
        // provider guard fires first).
        let err = validate_lend_positions_input("", "not-a-chain", "0xabc", "all")
            .expect_err("empty provider rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 2);
    }

    #[test]
    fn positions_input_rejects_unparseable_chain() {
        let err = validate_lend_positions_input("aave", "definitely-not-a-chain", "0xabc", "all")
            .expect_err("bad chain rejected");
        // id parse errors are usage-coded.
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn positions_input_requires_address() {
        let err = validate_lend_positions_input("aave", "1", "", "all")
            .expect_err("empty address rejected");
        assert_eq!(err.code, Code::Usage);
        assert!(
            err.to_string().to_lowercase().contains("address"),
            "got: {err}"
        );
    }

    #[test]
    fn positions_input_rejects_invalid_evm_address() {
        // Parity with TestRunnerLendPositionsRejectsInvalidEVMAddress.
        let err = validate_lend_positions_input("aave", "1", "not-an-address", "all")
            .expect_err("invalid evm address rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn positions_input_rejects_invalid_type() {
        // Parity with TestRunnerLendPositionsRejectsInvalidType ("debt").
        let err = validate_lend_positions_input(
            "aave",
            "1",
            "0x000000000000000000000000000000000000dEaD",
            "debt",
        )
        .expect_err("invalid type rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn positions_input_accepts_valid_inputs_and_normalizes() {
        // Parity with the happy path of TestRunnerLendPositionsCallsProvider:
        // provider alias normalized, EVM address accepted verbatim, type parsed.
        let q = validate_lend_positions_input(
            "AAVE-V3",
            "1",
            "0x000000000000000000000000000000000000dEaD",
            "collateral",
        )
        .expect("valid positions input");
        assert_eq!(q.provider, "aave");
        assert_eq!(q.chain.caip2, "eip155:1");
        // account preserved verbatim (caller lowercases only for the cache key).
        assert_eq!(q.account, "0x000000000000000000000000000000000000dEaD");
        assert_eq!(q.position_type, LendPositionType::Collateral);
    }

    #[test]
    fn positions_input_empty_type_defaults_to_all() {
        let q = validate_lend_positions_input(
            "aave",
            "1",
            "0x000000000000000000000000000000000000dEaD",
            "",
        )
        .expect("valid positions input");
        assert_eq!(q.position_type, LendPositionType::All);
    }

    // --- 4. provider-capability gate --------------------------------------

    #[tokio::test]
    async fn fetch_positions_without_capability_is_unsupported() {
        // Parity with TestRunnerLendPositionsRequiresProviderCapability.
        let req = LendPositionsRequest {
            chain: parse_chain("solana").expect("solana"),
            account: "6dM4QgP1VnRfx6TVV1t5hBf3ytA5Qn2ATqNnSboP8qz5".to_string(),
            asset: Asset::default(),
            position_type: LendPositionType::All,
            limit: 20,
            rpc_url: String::new(),
        };
        let err = fetch_lend_positions("kamino", None, req)
            .await
            .expect_err("missing positions capability rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .to_lowercase()
                .contains("does not support positions"),
            "got: {err}"
        );
    }

    // --- 5. provider request forwarding -----------------------------------

    #[tokio::test]
    async fn fetch_positions_forwards_request_and_returns_rows() {
        // Parity with TestRunnerLendPositionsCallsProvider.
        let provider = FakeLendingPositionsProvider::new("aave", vec![position("aave")]);
        let req = LendPositionsRequest {
            chain: parse_chain("1").expect("mainnet"),
            account: "0x000000000000000000000000000000000000dead".to_string(),
            asset: Asset {
                chain_id: "eip155:1".to_string(),
                symbol: "USDC".to_string(),
                ..Asset::default()
            },
            position_type: LendPositionType::Collateral,
            limit: 5,
            rpc_url: String::new(),
        };

        let rows = fetch_lend_positions("aave", Some(&provider), req)
            .await
            .expect("positions fetched");

        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider, "aave");

        let last = provider.last_req.lock().unwrap();
        let last = last.as_ref().expect("request recorded");
        assert_eq!(last.position_type, LendPositionType::Collateral);
        assert!(last
            .account
            .eq_ignore_ascii_case("0x000000000000000000000000000000000000dead"));
        assert!(last.asset.symbol.eq_ignore_ascii_case("USDC"));
        assert_eq!(last.limit, 5);
    }
}

#[cfg(test)]
mod app_tests {
    //! # Success criteria — `defi-app::lend` app-level handler (WS2, read)
    //!
    //! These are the **command-layer** (handler / `run_with_args`) criteria for
    //! `lend markets` / `lend rates` / `lend positions`, driving the wired
    //! `cli::handle` path end-to-end. They complement the unit-level helper
    //! criteria above (limit truncation, intent mapping, positions input
    //! validation, capability gate, request forwarding) — those test the glue
    //! functions in isolation; these test the composed envelope + exit codes.
    //!
    //! The Go oracle (`internal/app/runner.go` `newLendCommand`,
    //! verified against the `./defi` binary) anchors every assertion:
    //!
    //! ## Success path (wiremock, via the existing `--rpc-url` seam)
    //!
    //! The only lending provider whose read path is injectable through the
    //! already-present `--rpc-url` flag (no `AppCtx` change required) is
    //! **Moonwell** (on-chain RPC reads on Base). Aave/Morpho read from a
    //! GraphQL endpoint with no app-level override seam yet, so the success-path
    //! envelope contract is asserted via Moonwell on `eip155:8453`, reusing the
    //! same JSON-RPC multicall mock the provider crate uses.
    //!
    //! L-A1. **`lend markets` success envelope.** `lend markets --provider
    //!       moonwell --chain base --asset USDC --rpc-url <mock>` resolves a
    //!       success [`Envelope`]: `version="v1"`, `success=true`, `error=None`,
    //!       `meta.command="lend markets"`, `data` is a non-empty array of
    //!       `LendMarket` whose `provider == protocol == "moonwell"`, APY values
    //!       are percentage points (spec §2.5: positive, not a ratio),
    //!       `partial=false`. (Go markets command success path.)
    //! L-A2. **`lend markets` reports the provider status.** `meta.providers`
    //!       contains exactly one entry `{name:"moonwell", status:"ok"}` (Go
    //!       `statuses := []ProviderStatus{{Name: provider.Info().Name,
    //!       Status: statusFromErr(nil)=="ok", ...}}`).
    //! L-A3. **`lend markets` cache transition.** With caching ENABLED, the first
    //!       invocation is a provider fetch that writes the cache
    //!       (`meta.cache.status=="write"`, `stale=false`); a SECOND invocation
    //!       with the same args serves the cache WITHOUT a second provider call
    //!       (`meta.cache.status=="hit"`, `stale=false`). With caching DISABLED
    //!       the status is `"miss"`. (Spec §2.5 cache flow; `lend markets` is a
    //!       data route, NOT bypassed — `should_open_cache("lend markets")`.)
    //! L-A4. **`lend markets --limit` truncates the envelope payload.** The
    //!       `data` array length is `min(provider_rows, limit)` (Go
    //!       `applyLendMarketLimit`). (Asserted with `--limit 0`/large is
    //!       pass-through; here the single-market fixture means `--limit 1`
    //!       keeps the row and a smaller dataset is unaffected — the truncation
    //!       wiring is covered by the unit test; this asserts the limit flag is
    //!       threaded into the handler at all.)
    //! L-A5. **`lend rates` success envelope.** `lend rates --provider moonwell
    //!       --chain base --asset USDC --rpc-url <mock>` → success envelope with
    //!       `meta.command="lend rates"`, a non-empty `LendRate` array with
    //!       positive `utilization`, and one `{name:"moonwell",status:"ok"}`
    //!       provider status. (Go rates command success path.)
    //! L-A6. **`lend positions` success envelope.** `lend positions --provider
    //!       moonwell --chain base --address <dead> --rpc-url <mock>` → success
    //!       envelope with `meta.command="lend positions"`, a non-empty
    //!       `LendPosition` array (`provider=="moonwell"`), and one
    //!       `{name:"moonwell",status:"ok"}` provider status. (Go positions
    //!       command success path.)
    //!
    //! ## Error paths (Go-semantic, via `run_with_args` full-binary path)
    //!
    //! L-E1. **`--provider` required.** `lend markets --chain 1 --asset USDC`
    //!       (no provider) → exit 2 (usage). (Go cobra `MarkFlagRequired
    //!       ("provider")` / in-handler `--provider is required`.)
    //! L-E2. **`lend rates` requires provider too** → exit 2. (Same Go guard.)
    //! L-E3. **`lend positions` requires `--address`.** `lend positions
    //!       --provider aave --chain 1` (no address) → exit 2 (usage). (Go
    //!       `MarkFlagRequired("address")` / `--address is required`.)
    //! L-E4. **`lend positions` invalid EVM address** → exit 2 (usage). (Go
    //!       `--address must be a valid EVM hex address`.)
    //! L-E5. **`lend positions` invalid `--type`** → exit 2 (usage). (Go
    //!       `--type must be one of: all,supply,borrow,collateral`.)
    //! L-E6. **`lend positions --provider kamino` is unsupported.** Kamino
    //!       implements `LendingProvider` (markets/rates) but NOT
    //!       `LendingPositionsProvider`, so positions → exit 13 (unsupported)
    //!       with message `"lending provider kamino does not support positions"`,
    //!       and the FULL error envelope is rendered (success=false, data=[],
    //!       error.code=13, meta.command="lend positions",
    //!       cache.status="bypass"). (Go capability gate; verified against the
    //!       `./defi` binary.)
    //! L-E7. **Error envelope is full + on the contract.** Driving the kamino
    //!       unsupported case through `cli::handle` returns a typed
    //!       [`Code::Unsupported`] error (exit 13) — NOT the WS2 "not yet
    //!       implemented" stub error — confirming the handler routes to the real
    //!       capability gate rather than the placeholder.
    //!
    //! SKIPPED here (covered elsewhere): per-row field/format byte parity
    //! (provider-crate goldens + WS5 sweep), Aave/Morpho GraphQL success
    //! envelopes (no app-level base-URL seam yet — deferred to the GREEN seam +
    //! WS5), and cobra-vs-clap exact required-flag phrasing (asserted at the
    //! exit-code + `usage_error` level only, robust to either enforcement site).

    use super::cli::{handle, LendCmd, MarketsArgs, PositionsArgs};
    use crate::cli::run_with_args;
    use crate::ctx::AppCtx;
    use defi_config::{MapEnv, Settings};
    use defi_errors::Code;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use alloy::dyn_abi::DynSolValue;
    use alloy::json_abi::JsonAbi;
    use alloy::primitives::{Address as AlloyAddress, U256};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    // ---- canonical Moonwell-on-Base test addresses (mirror the provider mock) -
    const TEST_COMPTROLLER: &str = "0xfBb21d0380beE3312B33c4353c8936a0F13EF26C";
    const TEST_ORACLE: &str = "0xEC942bE8A8114bFD0396A5052c36027f2cA6a9d0";
    const TEST_MTOKEN_USDC: &str = "0xEdc817A28E8B93B03976FBd4a3dDBc9f7D176c22";
    const TEST_USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    const DEAD: &str = "0x000000000000000000000000000000000000dEaD";
    const MULTICALL3_ADDR: &str = "0xca11bde05977b3631167028862be2a173976ca11";

    // ---- settings + env helpers ------------------------------------------

    /// JSON-output settings with caching toggled off by default. The cache /
    /// action store paths point at the supplied temp dir so a cache-enabled
    /// variant can open sqlite without touching the real home.
    fn settings_in(tmp: &std::path::Path, cache_enabled: bool) -> Settings {
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
            defillama_api_key: String::new(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    /// A `MapEnv` whose HOME points at a temp dir so `Settings::load` resolves
    /// cache/config paths without touching the real home. Keeps the `TempDir`
    /// guard alive for the test's duration.
    fn env_with_home() -> (MapEnv, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let env = MapEnv::with_home(tmp.path().to_path_buf());
        (env, tmp)
    }

    fn markets_args(rpc: &str) -> MarketsArgs {
        MarketsArgs {
            chain: Some("base".to_string()),
            asset: Some("USDC".to_string()),
            provider: Some("moonwell".to_string()),
            limit: 20,
            rpc_url: Some(rpc.to_string()),
        }
    }

    fn positions_args(rpc: &str) -> PositionsArgs {
        PositionsArgs {
            chain: Some("base".to_string()),
            address: Some(DEAD.to_string()),
            asset: None,
            provider: Some("moonwell".to_string()),
            r#type: "all".to_string(),
            limit: 20,
            rpc_url: Some(rpc.to_string()),
        }
    }

    // ---- Moonwell JSON-RPC multicall mock (ported from the provider crate) -

    fn addr(s: &str) -> AlloyAddress {
        s.parse().expect("valid test address")
    }

    fn selector_for(abi_json: &str, name: &str) -> String {
        let abi: JsonAbi = serde_json::from_str(abi_json).expect("parse abi");
        let f = abi
            .function(name)
            .and_then(|o| o.first())
            .cloned()
            .expect("function present");
        hex::encode(f.selector().0)
    }

    fn encode_output(values: &[DynSolValue]) -> Vec<u8> {
        DynSolValue::Tuple(values.to_vec()).abi_encode_params()
    }

    fn aggregate3_json() -> alloy::json_abi::Function {
        let abi: JsonAbi = serde_json::from_str(defi_registry::MULTICALL3_ABI).expect("parse mc3");
        abi.function("aggregate3")
            .and_then(|o| o.first())
            .cloned()
            .expect("aggregate3 present")
    }

    fn lower_hex(a: &AlloyAddress) -> String {
        format!("0x{}", hex::encode(a.as_slice()))
    }

    /// Per-call dispatcher resolving `(target, selector)` to an ABI return blob,
    /// mirroring the provider-crate Moonwell mock fixtures one-to-one.
    struct Dispatcher {
        get_all_markets_sel: String,
        oracle_sel: String,
        get_assets_in_sel: String,
        m_underlying_sel: String,
        m_supply_rate_sel: String,
        m_borrow_rate_sel: String,
        m_total_supply_sel: String,
        m_exchange_rate_sel: String,
        m_total_borrows_sel: String,
        m_get_cash_sel: String,
        m_snapshot_sel: String,
        e_symbol_sel: String,
        e_decimals_sel: String,
        o_price_sel: String,
        supply_rate: U256,
        borrow_rate: U256,
        total_supply: U256,
        exchange_rate: U256,
        total_borrows: U256,
        cash: U256,
        price: U256,
        m_token_bal: U256,
        borrow_bal: U256,
    }

    impl Dispatcher {
        fn new() -> Self {
            let pow = |base: u128, exp: u32| U256::from(base).pow(U256::from(exp));
            let comptroller_abi = defi_registry::MOONWELL_COMPTROLLER_ABI;
            let mtoken_abi = defi_registry::MOONWELL_MTOKEN_ABI;
            let erc20_abi = defi_registry::MOONWELL_ERC20_MINIMAL_ABI;
            let oracle_abi = defi_registry::MOONWELL_ORACLE_ABI;
            Dispatcher {
                get_all_markets_sel: selector_for(comptroller_abi, "getAllMarkets"),
                oracle_sel: selector_for(comptroller_abi, "oracle"),
                get_assets_in_sel: selector_for(comptroller_abi, "getAssetsIn"),
                m_underlying_sel: selector_for(mtoken_abi, "underlying"),
                m_supply_rate_sel: selector_for(mtoken_abi, "supplyRatePerTimestamp"),
                m_borrow_rate_sel: selector_for(mtoken_abi, "borrowRatePerTimestamp"),
                m_total_supply_sel: selector_for(mtoken_abi, "totalSupply"),
                m_exchange_rate_sel: selector_for(mtoken_abi, "exchangeRateCurrent"),
                m_total_borrows_sel: selector_for(mtoken_abi, "totalBorrowsCurrent"),
                m_get_cash_sel: selector_for(mtoken_abi, "getCash"),
                m_snapshot_sel: selector_for(mtoken_abi, "getAccountSnapshot"),
                e_symbol_sel: selector_for(erc20_abi, "symbol"),
                e_decimals_sel: selector_for(erc20_abi, "decimals"),
                o_price_sel: selector_for(oracle_abi, "getUnderlyingPrice"),
                supply_rate: U256::from(951293759u64),
                borrow_rate: U256::from(1585489599u64),
                total_supply: U256::from(100_000_000u128) * pow(10, 8),
                exchange_rate: U256::from(2u128) * pow(10, 14),
                total_borrows: U256::from(500_000u128) * pow(10, 6),
                cash: U256::from(500_000u128) * pow(10, 6),
                price: pow(10, 30),
                m_token_bal: U256::from(10_000u128) * pow(10, 8),
                borrow_bal: U256::from(1_000u128) * pow(10, 6),
            }
        }

        fn dispatch(&self, to: &str, data_hex: &str) -> Option<Vec<u8>> {
            let selector = data_hex.get(..8).unwrap_or("");
            let to = to.to_ascii_lowercase();

            if to == TEST_COMPTROLLER.to_ascii_lowercase() {
                if selector == self.get_all_markets_sel {
                    return Some(encode_output(&[DynSolValue::Array(vec![
                        DynSolValue::Address(addr(TEST_MTOKEN_USDC)),
                    ])]));
                }
                if selector == self.oracle_sel {
                    return Some(encode_output(&[DynSolValue::Address(addr(TEST_ORACLE))]));
                }
                if selector == self.get_assets_in_sel {
                    return Some(encode_output(&[DynSolValue::Array(vec![
                        DynSolValue::Address(addr(TEST_MTOKEN_USDC)),
                    ])]));
                }
            } else if to == TEST_ORACLE.to_ascii_lowercase() {
                if selector == self.o_price_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.price, 256)]));
                }
            } else if to == TEST_MTOKEN_USDC.to_ascii_lowercase() {
                if selector == self.m_underlying_sel {
                    return Some(encode_output(&[DynSolValue::Address(addr(TEST_USDC))]));
                }
                if selector == self.m_supply_rate_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.supply_rate, 256)]));
                }
                if selector == self.m_borrow_rate_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.borrow_rate, 256)]));
                }
                if selector == self.m_total_supply_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.total_supply, 256)]));
                }
                if selector == self.m_exchange_rate_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.exchange_rate, 256)]));
                }
                if selector == self.m_total_borrows_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.total_borrows, 256)]));
                }
                if selector == self.m_get_cash_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.cash, 256)]));
                }
                if selector == self.m_snapshot_sel {
                    return Some(encode_output(&[
                        DynSolValue::Uint(U256::ZERO, 256),
                        DynSolValue::Uint(self.m_token_bal, 256),
                        DynSolValue::Uint(self.borrow_bal, 256),
                        DynSolValue::Uint(self.exchange_rate, 256),
                    ]));
                }
            } else if to == TEST_USDC.to_ascii_lowercase() {
                if selector == self.e_symbol_sel {
                    return Some(encode_output(&[DynSolValue::String("USDC".to_string())]));
                }
                if selector == self.e_decimals_sel {
                    return Some(encode_output(&[DynSolValue::Uint(U256::from(6u8), 8)]));
                }
            }
            None
        }
    }

    struct RpcResponder {
        dispatcher: Arc<Dispatcher>,
    }

    impl Respond for RpcResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = match serde_json::from_slice(&request.body) {
                Ok(v) => v,
                Err(_) => return ResponseTemplate::new(400),
            };
            let id = body.get("id").cloned().unwrap_or(json!(1));
            let method_name = body.get("method").and_then(Value::as_str).unwrap_or("");
            if method_name != "eth_call" {
                return ok_response(&id, "0x");
            }
            let params = match body.get("params").and_then(|p| p.get(0)) {
                Some(p) => p,
                None => return ok_response(&id, "0x"),
            };
            let to = params
                .get("to")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let data_hex = params
                .get("data")
                .or_else(|| params.get("input"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim_start_matches("0x")
                .to_string();
            let selector = data_hex.get(..8).unwrap_or("");

            let mc3_sel = selector_for(defi_registry::MULTICALL3_ABI, "aggregate3");
            if to.to_ascii_lowercase() == MULTICALL3_ADDR && selector == mc3_sel {
                let result = self.handle_aggregate3(&data_hex);
                return ok_response(&id, &result);
            }

            let result = match self.dispatcher.dispatch(&to, &data_hex) {
                Some(bytes) => format!("0x{}", hex::encode(bytes)),
                None => "0x".to_string(),
            };
            ok_response(&id, &result)
        }
    }

    impl RpcResponder {
        fn handle_aggregate3(&self, data_hex: &str) -> String {
            use alloy::dyn_abi::{FunctionExt, JsonAbiExt};
            let raw = match hex::decode(data_hex) {
                Ok(b) => b,
                Err(_) => return "0x".to_string(),
            };
            if raw.len() < 4 {
                return "0x".to_string();
            }
            let agg = aggregate3_json();
            let decoded = match agg.abi_decode_input(&raw[4..]) {
                Ok(v) => v,
                Err(_) => return "0x".to_string(),
            };
            let calls = match decoded.first().and_then(|v| v.as_array()) {
                Some(c) => c,
                None => return "0x".to_string(),
            };

            let mut results: Vec<DynSolValue> = Vec::with_capacity(calls.len());
            for call in calls {
                let tuple = match call.as_tuple() {
                    Some(t) if t.len() == 3 => t,
                    _ => {
                        results.push(failed_result());
                        continue;
                    }
                };
                let target = tuple[0]
                    .as_address()
                    .map(|a| lower_hex(&a))
                    .unwrap_or_default();
                let sub_data = tuple[2].as_bytes().map(hex::encode).unwrap_or_default();
                match self.dispatcher.dispatch(&target, &sub_data) {
                    Some(bytes) => results.push(DynSolValue::Tuple(vec![
                        DynSolValue::Bool(true),
                        DynSolValue::Bytes(bytes),
                    ])),
                    None => results.push(failed_result()),
                }
            }

            match agg.abi_encode_output(&[DynSolValue::Array(results)]) {
                Ok(bytes) => format!("0x{}", hex::encode(bytes)),
                Err(_) => "0x".to_string(),
            }
        }
    }

    fn failed_result() -> DynSolValue {
        DynSolValue::Tuple(vec![
            DynSolValue::Bool(false),
            DynSolValue::Bytes(Vec::new()),
        ])
    }

    fn ok_response(id: &Value, result: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    async fn moonwell_rpc_server() -> MockServer {
        let server = MockServer::start().await;
        let responder = RpcResponder {
            dispatcher: Arc::new(Dispatcher::new()),
        };
        Mock::given(method("POST"))
            .respond_with(responder)
            .mount(&server)
            .await;
        server
    }

    // ---- L-A1 / L-A2: markets success envelope + provider status ----------

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_markets_success_envelope_and_provider_status() {
        let server = moonwell_rpc_server().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        let env = handle(&ctx, LendCmd::Markets(markets_args(&server.uri())))
            .await
            .expect("lend markets should succeed against the mock RPC");

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert_eq!(env.meta.command, "lend markets");
        assert!(!env.meta.partial);

        let rows = env
            .data
            .as_ref()
            .and_then(Value::as_array)
            .expect("data is an array");
        assert!(!rows.is_empty(), "expected at least one market");
        assert_eq!(rows[0]["provider"], json!("moonwell"));
        assert_eq!(rows[0]["protocol"], json!("moonwell"));
        // APY = percentage points (spec §2.5): positive, not a sub-1 ratio.
        let supply_apy = rows[0]["supply_apy"].as_f64().expect("supply_apy f64");
        assert!(
            supply_apy > 0.0,
            "supply_apy should be positive: {supply_apy}"
        );

        // L-A2: one provider status, status "ok".
        assert_eq!(env.meta.providers.len(), 1, "exactly one provider status");
        assert_eq!(env.meta.providers[0].name, "moonwell");
        assert_eq!(env.meta.providers[0].status, "ok");
    }

    // ---- L-A3: cache transition write -> hit ------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_markets_cache_write_then_hit() {
        let server = moonwell_rpc_server().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), true));

        // First call: miss -> provider fetch -> cache write.
        let first = handle(&ctx, LendCmd::Markets(markets_args(&server.uri())))
            .await
            .expect("first lend markets");
        assert_eq!(
            first.meta.cache.status, "write",
            "first cache-enabled fetch should write the cache"
        );
        assert!(!first.meta.cache.stale);

        // Second call with identical args: fresh hit -> no provider call.
        let second = handle(&ctx, LendCmd::Markets(markets_args(&server.uri())))
            .await
            .expect("second lend markets");
        assert_eq!(
            second.meta.cache.status, "hit",
            "second identical fetch should hit the cache"
        );
        assert!(!second.meta.cache.stale);
        // A fresh hit short-circuits the provider, so no provider status row.
        assert!(
            second.meta.providers.is_empty(),
            "fresh hit must not call the provider"
        );
    }

    // ---- L-A3 (disabled cache): status "miss" -----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_markets_cache_disabled_status_miss() {
        let server = moonwell_rpc_server().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        let env = handle(&ctx, LendCmd::Markets(markets_args(&server.uri())))
            .await
            .expect("lend markets");
        assert_eq!(
            env.meta.cache.status, "miss",
            "cache-disabled fetch keeps the initial miss status"
        );
    }

    // ---- L-A4: --limit threads into the handler ---------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_markets_limit_caps_payload() {
        let server = moonwell_rpc_server().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        let mut args = markets_args(&server.uri());
        args.limit = 1;
        let env = handle(&ctx, LendCmd::Markets(args))
            .await
            .expect("lend markets --limit 1");
        let rows = env
            .data
            .as_ref()
            .and_then(Value::as_array)
            .expect("data is an array");
        assert!(
            rows.len() <= 1,
            "--limit 1 must cap rows to 1, got {}",
            rows.len()
        );
    }

    // ---- L-A5: rates success envelope -------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_rates_success_envelope_and_provider_status() {
        let server = moonwell_rpc_server().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        // `lend rates` reuses MarketsArgs (clap alias); same flags.
        let env = handle(&ctx, LendCmd::Rates(markets_args(&server.uri())))
            .await
            .expect("lend rates should succeed against the mock RPC");

        assert_eq!(env.meta.command, "lend rates");
        assert!(env.success);
        let rows = env
            .data
            .as_ref()
            .and_then(Value::as_array)
            .expect("data is an array");
        assert!(!rows.is_empty(), "expected at least one rate");
        let util = rows[0]["utilization"].as_f64().expect("utilization f64");
        assert!(util > 0.0, "utilization should be positive: {util}");

        assert_eq!(env.meta.providers.len(), 1);
        assert_eq!(env.meta.providers[0].name, "moonwell");
        assert_eq!(env.meta.providers[0].status, "ok");
    }

    // ---- L-A6: positions success envelope ---------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_positions_success_envelope_and_provider_status() {
        let server = moonwell_rpc_server().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        let env = handle(&ctx, LendCmd::Positions(positions_args(&server.uri())))
            .await
            .expect("lend positions should succeed against the mock RPC");

        assert_eq!(env.meta.command, "lend positions");
        assert!(env.success);
        let rows = env
            .data
            .as_ref()
            .and_then(Value::as_array)
            .expect("data is an array");
        assert!(!rows.is_empty(), "expected at least one position");
        assert_eq!(rows[0]["provider"], json!("moonwell"));

        assert_eq!(env.meta.providers.len(), 1);
        assert_eq!(env.meta.providers[0].name, "moonwell");
        assert_eq!(env.meta.providers[0].status, "ok");
    }

    // ---- L-E6 / L-E7: kamino positions is unsupported (via handle) --------

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_positions_kamino_is_unsupported_typed_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        let mut args = positions_args("");
        args.provider = Some("kamino".to_string());
        args.chain = Some("1".to_string());

        let err = handle(&ctx, LendCmd::Positions(args))
            .await
            .expect_err("kamino positions must be unsupported");
        assert_eq!(err.code, Code::Unsupported);
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("does not support positions"),
            "expected capability-gate message, got: {msg}"
        );
        // L-E7: must NOT be the WS2 placeholder stub error.
        assert!(
            !msg.contains("not yet implemented"),
            "kamino positions must route to the real capability gate, got: {msg}"
        );
    }

    // ---- L-E1..L-E5: usage error paths via run_with_args (full binary) ----

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_markets_missing_provider_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            ["defi", "lend", "markets", "--chain", "1", "--asset", "USDC"],
            &env,
        )
        .await;
        assert_eq!(code, 2, "missing --provider must be a usage error (exit 2)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_rates_missing_provider_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            ["defi", "lend", "rates", "--chain", "1", "--asset", "USDC"],
            &env,
        )
        .await;
        assert_eq!(code, 2, "missing --provider must be a usage error (exit 2)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_positions_missing_address_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "lend",
                "positions",
                "--provider",
                "aave",
                "--chain",
                "1",
            ],
            &env,
        )
        .await;
        assert_eq!(code, 2, "missing --address must be a usage error (exit 2)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_positions_invalid_evm_address_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "lend",
                "positions",
                "--provider",
                "aave",
                "--chain",
                "1",
                "--address",
                "notanaddress",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "invalid EVM address must be a usage error (exit 2)"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_positions_invalid_type_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "lend",
                "positions",
                "--provider",
                "aave",
                "--chain",
                "1",
                "--address",
                DEAD,
                "--type",
                "debt",
            ],
            &env,
        )
        .await;
        assert_eq!(code, 2, "invalid --type must be a usage error (exit 2)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lend_positions_kamino_is_unsupported_exit_13() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "lend",
                "positions",
                "--provider",
                "kamino",
                "--chain",
                "1",
                "--address",
                DEAD,
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 13,
            "kamino positions must be unsupported (exit 13), matching the Go oracle"
        );
    }

    // ---- silence unused-import lint on PathBuf in some build configs ------
    #[allow(dead_code)]
    fn _assert_pathbuf_used(_p: PathBuf) {}
}
