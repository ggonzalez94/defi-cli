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
    use defi_execution::builder::{LendRequest, LendVerb, Registry};
    use defi_id::normalize_amount;
    use defi_model::{Envelope, ProviderStatus};

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, SubmitArgs};
    use crate::execident::{apply_execution_identity_to_action, resolve_execution_identity};

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
            LendCmd::Supply(LendVerbCmd::Plan(args)) => {
                handle_plan(ctx, LendVerb::Supply, args).await
            }
            LendCmd::Withdraw(LendVerbCmd::Plan(args)) => {
                handle_plan(ctx, LendVerb::Withdraw, args).await
            }
            LendCmd::Borrow(LendVerbCmd::Plan(args)) => {
                handle_plan(ctx, LendVerb::Borrow, args).await
            }
            LendCmd::Repay(LendVerbCmd::Plan(args)) => {
                handle_plan(ctx, LendVerb::Repay, args).await
            }
            other => {
                let path = format!("lend {}", other.path());
                let ws = if path.ends_with("plan") { "WS3" } else { "WS4" };
                Err(AppCtx::unimplemented(&path, ws))
            }
        }
    }

    /// Handle `lend <verb> plan` (Go `planCmd.RunE` in
    /// `lend_execution_commands.go`), shared across supply/withdraw/borrow/repay.
    ///
    /// Flow parity with the Go runner:
    /// 1. resolve the execution identity (OWS `--wallet` first / legacy
    ///    `--from-address`) on the requested chain; an identity error returns the
    ///    typed [`Error`] before anything is persisted;
    /// 2. parse `--chain` + `--asset`, default a non-positive asset `decimals` to
    ///    18, and normalize the amount against those decimals (carrying base +
    ///    decimal forms consistently, spec §2.4);
    /// 3. route the build by `--provider` through the action-build registry
    ///    ([`Registry::build_lend_action`] → the Aave/Morpho/Moonwell planner),
    ///    capturing one provider status keyed on the normalized lending provider
    ///    name (fallback `"lend"` when empty; Go `statusFromErr`);
    /// 4. stamp the resolved identity (wallet id/name, from-address, execution
    ///    backend) onto the action and persist it to the action [`Store`];
    /// 5. emit the success envelope with the identity warnings, the cache
    ///    bypassed (execution paths skip the cache, spec §2.5), and the lending
    ///    provider status.
    ///
    /// [`Store`]: defi_execution::store::Store
    async fn handle_plan(
        ctx: &AppCtx,
        verb: LendVerb,
        args: LendPlanArgs,
    ) -> Result<Envelope, Error> {
        let chain_arg = args.chain.as_deref().unwrap_or_default();
        let wallet_ref = args.identity.wallet.as_deref().unwrap_or_default();
        let from_flag = args.identity.from_address.as_deref().unwrap_or_default();

        // 1. Resolve the execution identity (returns before any persistence on
        //    error — both / neither input, malformed address, Tempo/non-EVM
        //    --wallet, OWS resolve failures).
        let identity = resolve_execution_identity(wallet_ref, from_flag, chain_arg)?;

        // The provider status name is keyed on the normalized lending provider
        // (Go `normalizeLendingProvider(plan.Provider)`); fall back to "lend"
        // when empty so a missing/unknown provider still reports one status row.
        let provider_name =
            crate::runner::normalize_lending_provider(args.provider.as_deref().unwrap_or_default());
        let status_name = if provider_name.is_empty() {
            "lend".to_string()
        } else {
            provider_name
        };

        // 2 & 3. Build + route the lend action; capture the provider status.
        let action = build_plan_action(verb, &args, &identity.from_address).await;
        let status = ProviderStatus {
            name: status_name,
            status: super::status_from_result(&action),
            latency_ms: 0,
        };
        let mut action = action?;

        // 4. Stamp the identity + persist (status already captured ok above).
        apply_execution_identity_to_action(&mut action, &identity);
        let store = ctx.open_action_store()?;
        store
            .save(&action)
            .map_err(|e| Error::wrap(Code::Internal, "persist planned action", e))?;

        // 5. Emit the success envelope (cache bypassed for execution paths).
        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize planned action", e))?;
        let path = format!("lend {} plan", verb_path(verb));
        let mut env = ctx.metadata_envelope(&path, data, vec![status]);
        env.warnings = identity.warnings;
        Ok(env)
    }

    /// Build the lend [`Action`] for a `plan` request (Go `buildAction` closure):
    /// parse chain/asset, default decimals to 18, normalize the amount, then route
    /// the [`LendRequest`] by provider through the registry.
    ///
    /// [`Action`]: defi_execution::action::Action
    async fn build_plan_action(
        verb: LendVerb,
        args: &LendPlanArgs,
        sender: &str,
    ) -> Result<defi_execution::action::Action, Error> {
        let chain_arg = args.chain.as_deref().unwrap_or_default();
        let asset_arg = args.asset.as_deref().unwrap_or_default();
        let (chain, asset) = super::parse_chain_asset(chain_arg, asset_arg)?;

        // Default a non-positive asset `decimals` to 18 (Go `buildAction`).
        let mut decimals = asset.decimals;
        if decimals <= 0 {
            decimals = 18;
        }
        let (base, _) = normalize_amount(
            args.amount.as_deref().unwrap_or_default(),
            args.amount_decimal.as_deref().unwrap_or_default(),
            decimals,
        )?;

        Registry::new()
            .build_lend_action(LendRequest {
                provider: args.provider.clone().unwrap_or_default(),
                verb,
                chain,
                asset,
                market_id: args.market_id.clone().unwrap_or_default(),
                amount_base_units: base,
                sender: sender.to_string(),
                recipient: args.recipient.clone().unwrap_or_default(),
                on_behalf_of: args.on_behalf_of.clone().unwrap_or_default(),
                interest_rate_mode: args.interest_rate_mode,
                simulate: args.simulate,
                rpc_url: args.rpc_url.clone().unwrap_or_default(),
                pool_address: args.pool_address.clone().unwrap_or_default(),
                pool_address_provider: args.pool_address_provider.clone().unwrap_or_default(),
            })
            .await
    }

    /// The leaf verb token for `meta.command` (`supply`/`withdraw`/`borrow`/
    /// `repay`).
    fn verb_path(verb: LendVerb) -> &'static str {
        match verb {
            LendVerb::Supply => "supply",
            LendVerb::Withdraw => "withdraw",
            LendVerb::Borrow => "borrow",
            LendVerb::Repay => "repay",
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

#[cfg(test)]
mod plan_app_tests {
    //! # Success criteria — `lend <verb> plan` app-level handler (WS3, exec-plan)
    //!
    //! Go oracle: `internal/app/lend_execution_commands.go` `planCmd.RunE` (the
    //! `buildAction` closure → `s.actionBuilderRegistry().BuildLendAction(...)` →
    //! `applyExecutionIdentityToAction` → `s.actionStore.Save` → `emitSuccess`).
    //! These tests drive [`cli::handle`] (the real dispatch entry the binary
    //! calls) end-to-end for the FOUR lend plan verbs (`supply`/`withdraw`/
    //! `borrow`/`repay` `plan`) ONLY, asserting the full machine contract the Go
    //! runner emits via `emitSuccess(...)` / the typed error → full-envelope
    //! `renderError(...)` path.
    //!
    //! ## Determinism / offline seams
    //!
    //! The lend builders (`build_aave_lend_action` etc.) connect to RPC
    //! (`RpcClient::connect`) and, for `supply`/`repay`, issue exactly one
    //! `eth_call` (`allowance(owner,spender)`) to decide whether an approval step
    //! is needed; `withdraw`/`borrow` issue NO `eth_call` when `--pool-address` is
    //! supplied (the pool is not RPC-resolved). All RPC is injected through the
    //! already-present `--rpc-url` flag pointed at a `wiremock` JSON-RPC mock that
    //! answers every `eth_call` with an ABI-encoded `allowance` word (the same
    //! `EchoIdResponder` shape the `defi-execution` planner suite uses), so the
    //! tests are fully offline + deterministic. Identity is exercised through the
    //! OFFLINE `--from-address` (legacy_local) path so no OWS vault / network is
    //! touched; the `--wallet` happy path (OWS resolve) is WS4b e2e territory and
    //! is asserted here only via its offline guard rejections.
    //!
    //! Aave pool resolution: passing `--pool-address` short-circuits the on-chain
    //! `getPool()` lookup, so the Aave verbs build deterministically without a
    //! pool-provider mock. A separate test asserts the chain-default
    //! pool-address-provider coverage (chains `1/10/137/8453/42161/43114`) by
    //! mocking the `getPool()` response on a covered chain WITHOUT `--pool-address`.
    //!
    //! Morpho/Moonwell: a full Morpho happy path needs the Morpho GraphQL endpoint
    //! (no app-level base-URL seam — the builder uses the production endpoint), so
    //! Morpho is asserted via its OFFLINE guards (`--market-id` required; malformed
    //! `--market-id`), which the planner checks before any GraphQL fetch. Moonwell
    //! is asserted via its OFFLINE `--on-behalf-of` rejection (Compound v2 calls
    //! operate on `msg.sender` only), checked before any RPC.
    //!
    //! ## Criteria (each a failing test until `cli::handle` wires `*_plan`)
    //!
    //! 1. **Plan success envelope (Aave supply, legacy `--from-address`).** A
    //!    valid `lend supply plan --provider aave --chain 1 --asset USDC --amount
    //!    1000000 --from-address 0x..aa --pool-address 0x..CC --rpc-url <mock>`
    //!    (allowance insufficient) returns `Ok(Envelope)` (exit 0) with:
    //!    `version=="v1"`, `success==true`, `error==None`, `meta.partial==false`,
    //!    `meta.command=="lend supply plan"`,
    //!    `meta.cache=={status:"bypass", age_ms:0, stale:false}` (execution paths
    //!    bypass the cache, spec §2.5), and `meta.providers==[{name:"aave",
    //!    status:"ok"}]` (Go captures one `ProviderStatus` keyed on the normalized
    //!    lending provider name with `statusFromErr(nil)=="ok"`).
    //!
    //! 2. **Planned action `data` shape (Aave supply).** `env.data` is the
    //!    serialized [`Action`]: `action_id` matches `^act_[0-9a-f]{32}$`;
    //!    `intent_type=="lend_supply"`; `provider=="aave"`; `status=="planned"`;
    //!    `chain_id=="eip155:1"`; `from_address` == the EIP-55 checksum of the
    //!    sender; `input_amount=="1000000"`. With an INSUFFICIENT allowance the
    //!    action has TWO steps — `[approval, lend_call]` — where the lend step
    //!    `type=="lend_call"`, `value=="0"`, `chain_id=="eip155:1"`, and `target` ==
    //!    the pool address (`0x..CC`). The action `metadata` carries the Aave context
    //!    (`protocol=="aave"`, `lending_action=="supply"`, plus `pool`,
    //!    `on_behalf_of`, `recipient`, `rate_mode`). (Go `BuildLendAction`→
    //!    `build_aave_lend_action` + `emitSuccess`.)
    //!
    //! 3. **Aave supply lend-step calldata reuses the alloy/ABI golden.** The lend
    //!    step `data` equals `supply(asset, amount, onBehalfOf, 0)` encoded with
    //!    the canonical `AAVE_POOL_ABI` via the same alloy `Function` machinery the
    //!    planner uses (computed in-test, NOT re-encoded by the handler). With the
    //!    default `--on-behalf-of` empty, `onBehalfOf` defaults to the resolved
    //!    sender. This proves the handler routes through `build_lend_action` (no
    //!    re-encoding) and that base⇔decimal amounts stay consistent (spec §2.4).
    //!
    //! 4. **Aave supply skips the approval step when allowance is sufficient.**
    //!    The same plan against a mock whose `allowance` >= the requested amount
    //!    yields a SINGLE `lend` step (no leading `approval` step). (Go
    //!    `appendApprovalIfNeeded`: `current >= amount` → no approval.)
    //!
    //! 5. **Aave withdraw is a single lend step (no RPC `eth_call`).** `lend
    //!    withdraw plan ... --pool-address 0x..CC --rpc-url <mock>` yields a single
    //!    `lend` step with `intent_type=="lend_withdraw"`, target == pool, and
    //!    calldata == `withdraw(asset, amount, to=recipient)` (recipient defaults
    //!    to the sender). No `approval` step. (Go withdraw verb.)
    //!
    //! 6. **Aave borrow is a single lend step with the default rate mode.** `lend
    //!    borrow plan ...` (default `--interest-rate-mode 2`) yields a single
    //!    `lend` step with `intent_type=="lend_borrow"` and calldata ==
    //!    `borrow(asset, amount, rateMode=2, 0, onBehalfOf=sender)`. The action
    //!    `metadata.rate_mode == 2`. (Go borrow verb + `resolveRateMode`.)
    //!
    //! 7. **Aave repay emits an approval then a lend step (allowance
    //!    insufficient).** `lend repay plan ...` yields `[approval, lend]` with
    //!    `intent_type=="lend_repay"` and the lend-step calldata ==
    //!    `repay(asset, amount, rateMode=2, onBehalfOf=sender)`. (Go repay verb.)
    //!
    //! 8. **Aave chain-default pool-address-provider.** WITHOUT `--pool-address`,
    //!    on a covered chain (e.g. `--chain 1`), the handler resolves the pool via
    //!    the chain-default pool-address-provider with an on-chain `getPool()` call
    //!    (mocked to return `0x..CC`), and the lend step targets that resolved
    //!    pool. (Go `resolveAavePoolAddress` default coverage for `1/10/137/8453/
    //!    42161/43114`.) An UNCOVERED chain without `--pool-address` /
    //!    `--pool-address-provider` → [`Code::Unsupported`] (exit 13).
    //!
    //! 9. **Plan persists the action to the Store.** After a successful Aave
    //!    supply plan the action is retrievable by its `action_id` from a freshly
    //!    opened [`defi_execution::store::Store`] over the same path, with matching
    //!    `intent_type=="lend_supply"`, `input_amount=="1000000"`, and
    //!    `provider=="aave"`. (Go `s.actionStore.Save`.)
    //!
    //! 10. **Legacy-identity warning + backend stamping.** The `--from-address`
    //!     path stamps `execution_backend=="legacy_local"` on the action AND
    //!     surfaces the Go warning `--wallet (OWS) is recommended over
    //!     --from-address for planning; see docs for details` in `env.warnings`.
    //!     (Go `resolveExecutionIdentity` legacy branch + `emitSuccess(...,
    //!     identity.Warnings, ...)`.)
    //!
    //! 11. **Decimal amount parity.** `--amount-decimal 1` (no `--amount`) on USDC
    //!     (6 decimals) yields the same `input_amount=="1000000"` and the same
    //!     supply calldata golden — base⇔decimal stay consistent (spec §2.4).
    //!
    //! 12. **`--provider` is required.** `lend supply plan` with an empty/missing
    //!     `--provider` → [`Code::Usage`] (exit 2) and persists NOTHING. (Go
    //!     `BuildLendAction`: `--provider is required`.)
    //!
    //! 13. **Unsupported lending provider.** `--provider kamino` (markets-only,
    //!     no execution builder) → [`Code::Unsupported`] (exit 13) with the Go
    //!     message `lend execution currently supports provider=aave|morpho|
    //!     moonwell`; persists NOTHING. (Go builder routing.)
    //!
    //! 14. **Identity-constraint errors (offline).**
    //!     (a) BOTH `--wallet` and `--from-address` → [`Code::Usage`] (exit 2);
    //!     (b) NEITHER `--wallet` nor `--from-address` → [`Code::Usage`] (exit 2);
    //!     (c) a malformed `--from-address` → [`Code::Usage`] (exit 2);
    //!     (d) `--wallet` on a Tempo chain → [`Code::Unsupported`] (exit 13)
    //!         (`--wallet planning is not supported on Tempo chains yet`).
    //!     (Go `resolveExecutionIdentity`.) On every error the handler returns the
    //!     typed `Err(Error)` (the runner renders the full error envelope to
    //!     stderr, spec §2.1) and persists NOTHING.
    //!
    //! 15. **Amount cross-validation through the handler.** BOTH `--amount` +
    //!     `--amount-decimal` → [`Code::Usage`] (exit 2); NEITHER → [`Code::Usage`]
    //!     (exit 2). A non-positive `--amount` (`0`) → [`Code::Usage`] (exit 2)
    //!     (`lend amount must be a positive integer in base units`). Nothing
    //!     persisted. (Delegated to `defi_id::normalize_amount` /
    //!     `normalize_lend_inputs` via `build_lend_action`.)
    //!
    //! 16. **Morpho requires `--market-id` (offline).** `lend supply plan
    //!     --provider morpho --chain 1 --asset USDC --amount 1000000
    //!     --from-address 0x..aa --rpc-url <mock>` with NO `--market-id` →
    //!     [`Code::Usage`] (exit 2) (the planner's `normalize_morpho_market_id`
    //!     guard, checked before any GraphQL fetch); a malformed (non-32-byte)
    //!     `--market-id` is likewise [`Code::Usage`] (exit 2). Nothing persisted.
    //!     (Go `BuildLendAction` morpho path → `normalizeMorphoMarketID`.)
    //!
    //! 17. **Moonwell rejects `--on-behalf-of` (offline).** `lend supply plan
    //!     --provider moonwell --chain base --asset USDC --amount 1000000
    //!     --on-behalf-of 0x..bb --from-address 0x..aa` → [`Code::Unsupported`]
    //!     (exit 13) with `moonwell does not support --on-behalf-of` (checked
    //!     before any RPC). Nothing persisted. (Go builder Moonwell guard.)
    //!
    //! SKIPPED (covered elsewhere / wrong unit):
    //!   * the Aave/Morpho/Moonwell ABI calldata encoding internals + the
    //!     sender/recipient/asset hex + positive-amount validation — owned by the
    //!     `defi-execution::planner` RED suite (ported from `planner/*_test.go`);
    //!   * the `build_lend_action` provider routing itself — `defi-execution::
    //!     builder` (B8);
    //!   * the OWS `--wallet` happy-path resolve + wallet-id persistence — WS4b
    //!     e2e (here only its offline guard rejections are asserted);
    //!   * `--input-json`/`--input-file` precedence — structured-input unit;
    //!   * cobra/clap flag defaults + required-flag marking — schema/CLI suites;
    //!   * a full Morpho/Moonwell happy-path action build (GraphQL/RPC heavy) —
    //!     `defi-execution::planner` suite + WS5 sweep.

    use super::cli::{handle, LendCmd, LendPlanArgs, LendVerbCmd};
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

    use alloy::dyn_abi::{DynSolValue, FunctionExt, JsonAbiExt};
    use alloy::json_abi::JsonAbi;
    use alloy::primitives::U256;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    // --- contract constants -------------------------------------------------

    /// Sender EOA (legacy `--from-address` identity); its EIP-55 checksum lands on
    /// the action.
    const SENDER: &str = "0x00000000000000000000000000000000000000aa";
    /// An on-behalf-of / recipient address used only in the Moonwell-rejection test.
    const OTHER: &str = "0x00000000000000000000000000000000000000bb";
    /// Aave Pool override (`--pool-address`) — short-circuits the on-chain
    /// `getPool()` lookup. The chain-default test mocks `getPool()` to return this.
    const POOL: &str = "0x00000000000000000000000000000000000000cc";
    /// USDC contract on Ethereum mainnet (6 decimals) — resolved by `parse_asset`.
    const USDC_MAINNET: &str = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
    /// A syntactically valid but non-32-byte Morpho market id (malformed).
    const SHORT_MARKET_ID: &str = "0x1234";
    /// The Go legacy-identity warning surfaced when planning with `--from-address`.
    const LEGACY_WARNING: &str =
        "--wallet (OWS) is recommended over --from-address for planning; see docs for details";

    // --- harness ------------------------------------------------------------

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

    /// An Aave supply `LendPlanArgs` with the canonical happy-path values; mutate
    /// per test. `--pool-address` is set so no on-chain `getPool()` is needed.
    fn aave_supply_args(rpc: &str) -> LendPlanArgs {
        LendPlanArgs {
            chain: Some("1".to_string()),
            asset: Some("USDC".to_string()),
            amount: Some("1000000".to_string()),
            amount_decimal: None,
            provider: Some("aave".to_string()),
            recipient: None,
            on_behalf_of: None,
            interest_rate_mode: 2,
            market_id: None,
            pool_address: Some(POOL.to_string()),
            pool_address_provider: None,
            rpc_url: Some(rpc.to_string()),
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(SENDER.to_string()),
            },
            input: InputFlags::default(),
        }
    }

    async fn run_plan(dir: &Path, cmd: LendCmd) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(&ctx, cmd).await
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

    // --- wiremock JSON-RPC: every eth_call returns `result` --------------------

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

    fn uint_word(v: u128) -> String {
        format!("0x{}", hex::encode(U256::from(v).to_be_bytes::<32>()))
    }

    /// A mock JSON-RPC endpoint answering every `eth_call` with a single
    /// ABI-encoded `uint256` word == `allowance`. Used for the
    /// allowance-check path (supply/repay) and for `getPool()` (returns an
    /// address right-padded in a 32-byte word; an allowance word whose value is
    /// the pool address is indistinguishable at the ABI level, so the pool-default
    /// test uses a dedicated address-word mock — see [`pool_getpool_rpc`]).
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

    /// A mock JSON-RPC endpoint answering every `eth_call` with the ABI word for
    /// the pool address (`getPool()` returns `address`). Used by the chain-default
    /// pool-address-provider test, which does NOT pass `--pool-address`.
    async fn pool_getpool_rpc() -> MockServer {
        let server = MockServer::start().await;
        // address word = 12 zero bytes + 20 address bytes.
        let word = format!("0x000000000000000000000000{}", &POOL[2..]);
        Mock::given(method("POST"))
            .respond_with(EchoIdResponder { result: word })
            .mount(&server)
            .await;
        server
    }

    // --- in-test alloy/ABI golden (reuses AAVE_POOL_ABI) -----------------------

    fn aave_fn(name: &str) -> alloy::json_abi::Function {
        let abi: JsonAbi = serde_json::from_str(defi_registry::AAVE_POOL_ABI).expect("parse abi");
        abi.function(name)
            .and_then(|o| o.first())
            .cloned()
            .expect("aave fn present")
    }

    fn aave_calldata(name: &str, args: &[DynSolValue]) -> String {
        let data = aave_fn(name)
            .abi_encode_input(args)
            .expect("encode aave fn");
        format!("0x{}", hex::encode(data))
    }

    fn addr_val(hexaddr: &str) -> DynSolValue {
        DynSolValue::Address(hexaddr.parse().expect("valid address"))
    }

    /// Expected `supply(asset, amount, onBehalfOf, referralCode=0)` calldata.
    fn supply_calldata(amount: u128, on_behalf_of: &str) -> String {
        aave_calldata(
            "supply",
            &[
                addr_val(USDC_MAINNET),
                DynSolValue::Uint(U256::from(amount), 256),
                addr_val(on_behalf_of),
                DynSolValue::Uint(U256::ZERO, 16),
            ],
        )
    }

    /// Expected `withdraw(asset, amount, to)` calldata.
    fn withdraw_calldata(amount: u128, to: &str) -> String {
        aave_calldata(
            "withdraw",
            &[
                addr_val(USDC_MAINNET),
                DynSolValue::Uint(U256::from(amount), 256),
                addr_val(to),
            ],
        )
    }

    /// Expected `borrow(asset, amount, interestRateMode, referralCode=0, onBehalfOf)`
    /// calldata.
    fn borrow_calldata(amount: u128, rate_mode: u64, on_behalf_of: &str) -> String {
        aave_calldata(
            "borrow",
            &[
                addr_val(USDC_MAINNET),
                DynSolValue::Uint(U256::from(amount), 256),
                DynSolValue::Uint(U256::from(rate_mode), 256),
                DynSolValue::Uint(U256::ZERO, 16),
                addr_val(on_behalf_of),
            ],
        )
    }

    /// Expected `repay(asset, amount, interestRateMode, onBehalfOf)` calldata.
    fn repay_calldata(amount: u128, rate_mode: u64, on_behalf_of: &str) -> String {
        aave_calldata(
            "repay",
            &[
                addr_val(USDC_MAINNET),
                DynSolValue::Uint(U256::from(amount), 256),
                DynSolValue::Uint(U256::from(rate_mode), 256),
                addr_val(on_behalf_of),
            ],
        )
    }

    fn step_types(data: &Value) -> Vec<String> {
        data["steps"]
            .as_array()
            .expect("steps array")
            .iter()
            .map(|s| s["type"].as_str().unwrap_or("").to_string())
            .collect()
    }

    /// The first step whose `type == "lend_call"` (the canonical machine-contract
    /// step type for a lend protocol call; Go `StepTypeLend == "lend_call"`).
    fn lend_step(data: &Value) -> Value {
        data["steps"]
            .as_array()
            .expect("steps array")
            .iter()
            .find(|s| s["type"].as_str() == Some("lend_call"))
            .cloned()
            .expect("a lend step is present")
    }

    // --- 1, 2, 3, 10. Aave supply happy path -------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_emits_success_envelope_and_action_shape() {
        let rpc = allowance_rpc(0).await; // insufficient -> approval needed.
        let tmp = TempDir::new().expect("tempdir");
        let env = run_plan(
            tmp.path(),
            LendCmd::Supply(LendVerbCmd::Plan(aave_supply_args(&rpc.uri()))),
        )
        .await
        .expect("aave supply plan should succeed against the mock RPC");

        // Envelope contract (Go `emitSuccess`).
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "lend supply plan");

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
        assert_eq!(data["intent_type"], Value::from("lend_supply"));
        assert_eq!(data["provider"], Value::from("aave"));
        assert_eq!(data["status"], Value::from("planned"));
        assert_eq!(data["chain_id"], Value::from("eip155:1"));
        assert_eq!(
            data["from_address"].as_str().unwrap().to_lowercase(),
            SENDER.to_lowercase(),
            "from_address is the (checksummed) sender"
        );
        assert_eq!(data["input_amount"], Value::from("1000000"));

        // Insufficient allowance -> [approval, lend_call].
        assert_eq!(
            step_types(&data),
            vec!["approval".to_string(), "lend_call".to_string()],
            "insufficient allowance => approval then lend_call"
        );
        let lend = lend_step(&data);
        assert_eq!(lend["value"], Value::from("0"));
        assert_eq!(lend["chain_id"], Value::from("eip155:1"));
        assert_eq!(
            lend["target"].as_str().unwrap().to_lowercase(),
            POOL.to_lowercase(),
            "lend step targets the resolved pool"
        );

        // metadata carries the Aave context.
        let meta = data["metadata"].as_object().expect("metadata object");
        assert_eq!(meta.get("protocol"), Some(&Value::from("aave")));
        assert_eq!(meta.get("lending_action"), Some(&Value::from("supply")));
        assert!(meta.contains_key("pool"));
        assert!(meta.contains_key("on_behalf_of"));
        assert!(meta.contains_key("recipient"));
        assert!(meta.contains_key("rate_mode"));

        // Legacy backend stamping + warning (criterion 10).
        assert_eq!(data["execution_backend"], Value::from("legacy_local"));
        assert!(
            env.warnings.iter().any(|w| w == LEGACY_WARNING),
            "legacy --from-address plan surfaces the OWS-recommended warning; got {:?}",
            env.warnings
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_lend_step_calldata_matches_aave_abi_golden() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let env = run_plan(
            tmp.path(),
            LendCmd::Supply(LendVerbCmd::Plan(aave_supply_args(&rpc.uri()))),
        )
        .await
        .expect("aave supply plan should succeed");
        let data = action_data(&env);
        let lend = lend_step(&data);
        let calldata = lend["data"].as_str().expect("lend step data");
        // on_behalf_of defaults to the sender when the flag is empty.
        assert_eq!(
            calldata.to_lowercase(),
            supply_calldata(1_000_000, SENDER).to_lowercase(),
            "supply lend-step calldata must equal the alloy AAVE_POOL_ABI golden"
        );
    }

    // --- 4. allowance sufficient -> single lend step ----------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_skips_approval_when_allowance_sufficient() {
        let rpc = allowance_rpc(10_000_000).await; // >= requested.
        let tmp = TempDir::new().expect("tempdir");
        let env = run_plan(
            tmp.path(),
            LendCmd::Supply(LendVerbCmd::Plan(aave_supply_args(&rpc.uri()))),
        )
        .await
        .expect("aave supply plan should succeed");
        let data = action_data(&env);
        assert_eq!(
            step_types(&data),
            vec!["lend_call".to_string()],
            "sufficient allowance => single lend step"
        );
    }

    // --- 5. Aave withdraw --------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn withdraw_plan_is_single_lend_step_with_golden_calldata() {
        let rpc = allowance_rpc(0).await; // withdraw makes no eth_call, but connect succeeds.
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args(&rpc.uri());
        args.amount = Some("500000".to_string());
        let env = run_plan(tmp.path(), LendCmd::Withdraw(LendVerbCmd::Plan(args)))
            .await
            .expect("aave withdraw plan should succeed");
        let data = action_data(&env);
        assert_eq!(data["intent_type"], Value::from("lend_withdraw"));
        assert_eq!(env.meta.command, "lend withdraw plan");
        assert_eq!(step_types(&data), vec!["lend_call".to_string()]);
        let lend = lend_step(&data);
        assert_eq!(
            lend["target"].as_str().unwrap().to_lowercase(),
            POOL.to_lowercase()
        );
        // recipient defaults to the sender.
        assert_eq!(
            lend["data"].as_str().unwrap().to_lowercase(),
            withdraw_calldata(500_000, SENDER).to_lowercase(),
            "withdraw calldata must equal the alloy AAVE_POOL_ABI golden"
        );
    }

    // --- 6. Aave borrow ----------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn borrow_plan_is_single_lend_step_with_default_rate_mode() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let args = aave_supply_args(&rpc.uri());
        let env = run_plan(tmp.path(), LendCmd::Borrow(LendVerbCmd::Plan(args)))
            .await
            .expect("aave borrow plan should succeed");
        let data = action_data(&env);
        assert_eq!(data["intent_type"], Value::from("lend_borrow"));
        assert_eq!(env.meta.command, "lend borrow plan");
        assert_eq!(step_types(&data), vec!["lend_call".to_string()]);
        let lend = lend_step(&data);
        assert_eq!(
            lend["data"].as_str().unwrap().to_lowercase(),
            borrow_calldata(1_000_000, 2, SENDER).to_lowercase(),
            "borrow calldata must equal the alloy golden (default rate mode 2)"
        );
        // metadata.rate_mode carries the requested mode verbatim.
        let meta = data["metadata"].as_object().expect("metadata object");
        assert_eq!(meta.get("rate_mode"), Some(&Value::from(2)));
    }

    // --- 7. Aave repay -----------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn repay_plan_emits_approval_then_lend_step() {
        let rpc = allowance_rpc(0).await; // insufficient -> approval needed.
        let tmp = TempDir::new().expect("tempdir");
        let args = aave_supply_args(&rpc.uri());
        let env = run_plan(tmp.path(), LendCmd::Repay(LendVerbCmd::Plan(args)))
            .await
            .expect("aave repay plan should succeed");
        let data = action_data(&env);
        assert_eq!(data["intent_type"], Value::from("lend_repay"));
        assert_eq!(env.meta.command, "lend repay plan");
        assert_eq!(
            step_types(&data),
            vec!["approval".to_string(), "lend_call".to_string()]
        );
        let lend = lend_step(&data);
        assert_eq!(
            lend["data"].as_str().unwrap().to_lowercase(),
            repay_calldata(1_000_000, 2, SENDER).to_lowercase(),
            "repay calldata must equal the alloy golden (default rate mode 2)"
        );
    }

    // --- 8. Aave chain-default pool-address-provider -----------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_resolves_chain_default_pool_provider() {
        // WITHOUT --pool-address on a covered chain (1): the pool is resolved via
        // the chain-default pool-address-provider (getPool() mocked to return POOL).
        let rpc = pool_getpool_rpc().await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args(&rpc.uri());
        args.pool_address = None;
        let env = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect("aave supply plan should resolve the default pool provider");
        let data = action_data(&env);
        let lend = lend_step(&data);
        assert_eq!(
            lend["target"].as_str().unwrap().to_lowercase(),
            POOL.to_lowercase(),
            "lend step targets the RPC-resolved default pool"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_uncovered_chain_without_pool_is_unsupported() {
        // An EVM chain with no Aave pool-address-provider default and no
        // --pool-address / --pool-address-provider -> Unsupported (exit 13).
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args(&rpc.uri());
        args.pool_address = None;
        args.chain = Some("56".to_string()); // BNB chain: not in the default map.
                                             // BSC has no bootstrap USDC registry entry; use a bare token address so
                                             // the asset resolves on an EVM chain (the pool guard fires regardless).
        args.asset = Some(USDC_MAINNET.to_string());
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect_err("uncovered chain without a pool must be unsupported");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        // Distinguish the real pool-resolution guard from the unimplemented stub
        // (both are Unsupported; only the real guard names the pool provider).
        let msg = err.to_string();
        assert!(
            msg.contains("aave pool address provider is unavailable"),
            "expected the pool-resolution guard, got: {msg}"
        );
        assert!(
            !msg.contains("not yet implemented"),
            "must route to the real planner, not the stub: {msg}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 9. plan persists the action to the Store --------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_persists_action_to_store() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let settings = exec_settings(tmp.path());
        let ctx = AppCtx::new(settings.clone());
        let env = handle(
            &ctx,
            LendCmd::Supply(LendVerbCmd::Plan(aave_supply_args(&rpc.uri()))),
        )
        .await
        .expect("aave supply plan should succeed");
        let action_id = action_data(&env)["action_id"]
            .as_str()
            .expect("action_id")
            .to_string();

        let store = ActionStore::open(&settings.action_store_path, &settings.action_lock_path)
            .expect("reopen action store");
        let persisted = store
            .get(&action_id)
            .expect("planned action retrievable by id");
        assert_eq!(persisted.intent_type, "lend_supply");
        assert_eq!(persisted.input_amount, "1000000");
        assert_eq!(persisted.provider, "aave");
    }

    // --- 11. decimal amount parity -----------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_decimal_amount_yields_same_base_and_calldata() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args(&rpc.uri());
        args.amount = None;
        args.amount_decimal = Some("1".to_string()); // 1 USDC (6 decimals).
        let env = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect("decimal-amount plan should succeed");
        let data = action_data(&env);
        assert_eq!(data["input_amount"], Value::from("1000000"));
        assert_eq!(
            lend_step(&data)["data"].as_str().unwrap().to_lowercase(),
            supply_calldata(1_000_000, SENDER).to_lowercase(),
            "decimal 1 USDC normalizes to the same calldata as base 1000000"
        );
    }

    // --- 12. --provider required -------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_requires_provider() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args(&rpc.uri());
        args.provider = None;
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect_err("missing --provider must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 13. unsupported provider ------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_rejects_kamino_provider() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args(&rpc.uri());
        args.provider = Some("kamino".to_string());
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect_err("kamino lend execution must be unsupported");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .contains("lend execution currently supports provider=aave|morpho|moonwell"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 14. identity-constraint errors (offline) --------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_rejects_both_identity_inputs() {
        let tmp = TempDir::new().expect("tempdir");
        // No RPC needed: identity resolution happens before any build.
        let mut args = aave_supply_args("http://127.0.0.1:1");
        args.identity.wallet = Some("alice".to_string());
        // from_address already set in base.
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect_err("both identity inputs must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_rejects_missing_identity_inputs() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args("http://127.0.0.1:1");
        args.identity.wallet = None;
        args.identity.from_address = None;
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect_err("missing identity inputs must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_rejects_malformed_from_address() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args("http://127.0.0.1:1");
        args.identity.from_address = Some("0xnot-an-address".to_string());
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect_err("malformed --from-address must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_rejects_wallet_on_tempo_chain() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args("http://127.0.0.1:1");
        args.chain = Some("tempo".to_string()); // eip155:4217 (Tempo mainnet).
        args.identity.from_address = None;
        args.identity.wallet = Some("alice".to_string());
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
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

    // --- 15. amount cross-validation through the handler -------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_rejects_both_amount_forms() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args("http://127.0.0.1:1");
        args.amount = Some("1000000".to_string());
        args.amount_decimal = Some("1".to_string());
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect_err("both amount forms must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_rejects_missing_amount() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args("http://127.0.0.1:1");
        args.amount = None;
        args.amount_decimal = None;
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect_err("missing amount must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn supply_plan_rejects_non_positive_amount() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args(&rpc.uri());
        args.amount = Some("0".to_string());
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect_err("zero amount must be rejected by the planner");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 16. Morpho requires --market-id (offline) -------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn morpho_supply_plan_requires_market_id() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args(&rpc.uri());
        args.provider = Some("morpho".to_string());
        args.pool_address = None; // morpho ignores --pool-address.
        args.market_id = None;
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect_err("morpho without --market-id must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn morpho_supply_plan_rejects_malformed_market_id() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_supply_args(&rpc.uri());
        args.provider = Some("morpho".to_string());
        args.pool_address = None;
        args.market_id = Some(SHORT_MARKET_ID.to_string());
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect_err("morpho with a short --market-id must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 17. Moonwell rejects --on-behalf-of (offline) ---------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn moonwell_supply_plan_rejects_on_behalf_of() {
        let tmp = TempDir::new().expect("tempdir");
        // No RPC needed: the on-behalf-of guard fires before any RPC call.
        let mut args = aave_supply_args("http://127.0.0.1:1");
        args.provider = Some("moonwell".to_string());
        args.chain = Some("base".to_string());
        args.pool_address = None;
        args.on_behalf_of = Some(OTHER.to_string());
        let err = run_plan(tmp.path(), LendCmd::Supply(LendVerbCmd::Plan(args)))
            .await
            .expect_err("moonwell --on-behalf-of must be unsupported");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .contains("moonwell does not support --on-behalf-of"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }
}
