//! `chains` command group handler.
//!
//! Mirrors the `chains` subtree of `internal/app/runner.go::newChainsCommand`
//! plus the `fetchGasPrice`/`weiToGwei` helpers it composes. This module owns
//! the **command-layer composition** for the chains group; the lower-level
//! pieces are owned elsewhere and reused:
//!
//! * RPC reads + wei→gwei formatting parity: [`defi_evm::rpc`] (`RpcClient`,
//!   `wei_to_gwei`) — already contract-tested there;
//! * chain registry + parsing: [`defi_id`] (`list_chains`, `parse_chain`);
//! * default-RPC resolution + precedence: [`defi_registry::resolve_rpc_url`];
//! * cache-bypass routing (`chains list` / `chains gas` bypass): the runner
//!   (`defi_app::runner::should_open_cache`).
//!
//! The two contract-bearing surfaces this module composes:
//!
//! 1. **`chains list`** — offline, no keys, deterministic: maps the chain
//!    registry to `model::SupportedChain` in CAIP-2 order (golden parity with
//!    the Go binary).
//! 2. **`chains gas`** — live EVM gas, no keys, bypasses cache, returns an
//!    *array* of `model::GasPrice` even for a single chain. Single-chain may use
//!    a `--rpc-url` override; multi-chain forbids it, validates every chain as
//!    EVM up front, fetches in parallel preserving input order, drops failures
//!    into `warnings` (partial), and fails only if *all* chains fail (or, in
//!    strict mode, if any chain fails).
//!
//! Idiomatic-Rust shape note: the Go command closures write to injected
//! `io.Writer`s and return `error`. The Rust port exposes pure/async builder
//! functions returning values (`Vec<SupportedChain>`, `Result<GasPrice, Error>`,
//! `GasOutcome`) so they can be unit-tested without a `cobra.Command`; the
//! envelope construction + rendering is layered on top by the runner.

#![allow(dead_code, unused_variables)]

use chrono::{DateTime, SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_evm::rpc::{wei_to_gwei, RpcClient};
use defi_id::{parse_chain, Chain};
use defi_model::{GasPrice, ProviderStatus, SupportedChain};
use defi_providers::MarketDataProvider;
use serde_json::Value;

/// The cache TTL for `chains top` / `chains assets` (Go: `5 * time.Minute`).
pub const CHAINS_TTL_SECS: u64 = 300;

/// The default `--limit` for `chains top` / `chains assets` (Go default 20).
pub const CHAINS_DEFAULT_LIMIT: i64 = 20;

/// Request payload for `chains top` (Go `map[string]any{"limit":N}`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChainsTopRequest {
    /// `--limit` (number of rows; `<= 0` = all).
    pub limit: i64,
}

/// Request payload for `chains assets`.
///
/// Mirrors the Go request `map[string]any{"chain","asset","limit"}`, whose
/// `encoding/json` emits keys ALPHABETICALLY → `{"asset","chain","limit"}`.
/// Field declaration order is chosen to reproduce that JSON exactly so cache
/// keys stay byte-stable against the Go binary.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChainsAssetsRequest {
    /// The cache-stable asset filter value (Go `chainAssetFilterCacheValue`).
    pub asset: String,
    /// The chain CAIP-2.
    pub chain: String,
    /// `--limit` (number of rows; `<= 0` = all).
    pub limit: i64,
}

/// A resolved `chains top` / `chains assets` fetch.
///
/// Carries the JSON `data` payload (the serialized provider list) and the single
/// captured market-provider [`ProviderStatus`]. The runner layers envelope
/// construction + rendering on top.
#[derive(Debug, Clone)]
pub struct ChainsOutcome {
    /// The fetched list, serialized verbatim as a JSON array for `data`.
    pub data: Value,
    /// The single market-provider status captured for this fetch.
    pub provider: ProviderStatus,
}

/// Capture the single market-provider [`ProviderStatus`] from a fetch result
/// (Go `model.ProviderStatus{Name, Status: statusFromErr(err)}`). Latency timing
/// is owned by the runner's cache-flow state machine, so `latency_ms` is left at
/// zero here (matching the `protocols`/`dexes` command-layer composition).
fn provider_status<T>(provider: &dyn MarketDataProvider, res: &Result<T, Error>) -> ProviderStatus {
    ProviderStatus {
        name: provider.info().name,
        status: crate::protocols::status_from_result(res),
        latency_ms: 0,
    }
}

/// Serialize a fetched row list into a JSON array `data` payload, preserving
/// element struct field declaration order (serde default for structs).
fn rows_to_data<T: serde::Serialize>(rows: &[T]) -> Result<Value, Error> {
    serde_json::to_value(rows).map_err(|e| Error::wrap(Code::Internal, "serialize chains rows", e))
}

/// Run `chains top`: top chains by TVL (Go `newChainsCommand` `top` closure).
///
/// Calls [`MarketDataProvider::chains_top`] with the supplied `--limit`,
/// serializes the resulting `Vec<ChainTvl>` verbatim into `data` (element keys
/// `rank, chain, chain_id, tvl_usd` in struct declaration order), and captures
/// exactly one market-provider status. A provider error propagates with its
/// original code (the runner turns it into the full error envelope).
pub async fn run_top(
    provider: &dyn MarketDataProvider,
    limit: i64,
) -> Result<ChainsOutcome, Error> {
    let res = provider.chains_top(limit).await;
    let status = provider_status(provider, &res);
    let rows = res?;
    Ok(ChainsOutcome {
        data: rows_to_data(&rows)?,
        provider: status,
    })
}

/// Run `chains assets`: TVL by asset for a chain (Go `newChainsCommand`
/// `assets` closure).
///
/// Parses the required `--chain` (CAIP-2; an empty/unknown value surfaces the
/// [`parse_chain`] error → [`Code::Usage`]) and the OPTIONAL `--asset` filter via
/// [`parse_chain_asset_filter`] (which — unlike the looser `lend`/`positions`
/// optional-asset filter — rejects an address/CAIP that resolves to no known
/// token symbol on the chain with [`Code::Usage`]). It then calls
/// [`MarketDataProvider::chains_assets`] with the parsed `Chain` + `Asset` +
/// `--limit`, serializes the resulting `Vec<ChainAssetTvl>` verbatim into `data`
/// (element keys `rank, chain, chain_id, asset, asset_id, tvl_usd`), and captures
/// one market-provider status. Both guards run BEFORE any provider call.
pub async fn run_assets(
    provider: &dyn MarketDataProvider,
    chain_arg: &str,
    asset_arg: &str,
    limit: i64,
) -> Result<ChainsOutcome, Error> {
    let chain = parse_chain(chain_arg)?;
    let asset = parse_chain_asset_filter(&chain, asset_arg)?;
    let res = provider.chains_assets(chain, asset, limit).await;
    let status = provider_status(provider, &res);
    let rows = res?;
    Ok(ChainsOutcome {
        data: rows_to_data(&rows)?,
        provider: status,
    })
}

/// Parse the optional `chains assets` `--asset` filter (Go
/// `parseChainAssetFilter`).
///
/// This is intentionally STRICTER than [`crate::lend::parse_optional_chain_asset`]:
/// when the input parses as an address/CAIP but resolves to NO known token symbol
/// on the chain, it is rejected with [`Code::Usage`] ("asset filter by
/// address/CAIP requires a known token symbol on the selected chain") rather than
/// being forwarded as an unfiltered request. An empty input yields a default
/// (unfiltered) [`Asset`]; a bare symbol filter falls back to a symbol-only asset.
pub fn parse_chain_asset_filter(
    chain: &defi_id::Chain,
    asset_arg: &str,
) -> Result<defi_id::Asset, Error> {
    let asset_arg = asset_arg.trim();
    if asset_arg.is_empty() {
        return Ok(defi_id::Asset::default());
    }

    match defi_id::parse_asset(asset_arg, chain) {
        Ok(asset) => {
            if asset.symbol.trim().is_empty() {
                return Err(Error::new(
                    Code::Usage,
                    "asset filter by address/CAIP requires a known token symbol on the selected chain",
                ));
            }
            Ok(asset)
        }
        Err(err) => {
            if crate::lend::looks_like_address_or_caip(asset_arg)
                || !crate::lend::looks_like_symbol_filter(asset_arg)
            {
                return Err(err);
            }
            Ok(defi_id::Asset {
                chain_id: chain.caip2.clone(),
                symbol: asset_arg.to_ascii_uppercase(),
                ..defi_id::Asset::default()
            })
        }
    }
}

/// Build the `chains list` data payload.
///
/// Maps every entry from [`defi_id::list_chains`] (already deduped + sorted by
/// CAIP-2) to a [`SupportedChain`], preserving `name`/`slug`/`caip2`/`namespace`
/// /`evm_chain_id`/`aliases`. Pure + offline (Go `newChainsCommand` `list`).
pub fn list_chains_data() -> Vec<SupportedChain> {
    defi_id::list_chains()
        .into_iter()
        .map(|entry| SupportedChain {
            name: entry.chain.name.clone(),
            slug: entry.chain.slug.clone(),
            caip2: entry.chain.caip2.clone(),
            namespace: entry.chain.namespace(),
            evm_chain_id: entry.chain.evm_chain_id,
            aliases: entry.aliases,
        })
        .collect()
}

/// A resolved gas-fetch target: a validated EVM chain plus the RPC URL to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GasChainTarget {
    /// The validated (EVM) chain.
    pub chain: Chain,
    /// The resolved RPC URL (override or registry default).
    pub rpc_url: String,
}

/// Parse and validate the `chains gas` `--chain` / `--rpc-url` flags into the
/// ordered list of fetch targets (Go `newChainsCommand` `gas` pre-flight).
///
/// Behavior (preserved from Go):
/// * splits `chain_arg` on `,`, trimming whitespace and dropping empties;
/// * at least one chain is required → [`defi_errors::Code::Usage`];
/// * `--rpc-url` with more than one chain → [`defi_errors::Code::Usage`];
/// * each chain is parsed (`defi_id::parse_chain`); non-EVM (`namespace !=
///   "eip155"`) → [`defi_errors::Code::Unsupported`];
/// * the RPC URL is resolved per chain via `defi_registry::resolve_rpc_url`
///   (override wins for the single-chain case; a missing default surfaces as the
///   resolver's error). Input order is preserved.
pub fn resolve_gas_targets(chain_arg: &str, rpc_url: &str) -> Result<Vec<GasChainTarget>, Error> {
    let chain_args: Vec<&str> = chain_arg
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect();

    if chain_args.is_empty() {
        return Err(Error::new(Code::Usage, "at least one chain is required"));
    }

    if chain_args.len() > 1 && !rpc_url.trim().is_empty() {
        return Err(Error::new(
            Code::Usage,
            "--rpc-url cannot be used with multiple chains",
        ));
    }

    let mut targets = Vec::with_capacity(chain_args.len());
    for raw in chain_args {
        let chain = parse_chain(raw)?;
        if chain.namespace() != "eip155" {
            return Err(Error::new(
                Code::Unsupported,
                format!("chains gas is only supported for EVM chains: {raw}"),
            ));
        }
        let resolved = defi_registry::resolve_rpc_url(rpc_url, chain.evm_chain_id)
            .map_err(|e| Error::wrap(Code::Unavailable, format!("resolve rpc for {raw}"), e))?;
        targets.push(GasChainTarget {
            chain,
            rpc_url: resolved,
        });
    }
    Ok(targets)
}

/// Fetch the current gas price for one chain over an established RPC client
/// (Go `fetchGasPrice`).
///
/// Reads the latest header (block number + optional `baseFeePerGas`) and the
/// suggested gas price. EIP-1559 is detected by the presence of a base fee; when
/// present the suggested priority fee (tip cap) is read too — and if that read
/// fails, the tip is recorded as zero (`"0.000000"`) with a
/// `"priority fee unavailable: …"` warning rather than failing the call. All
/// gwei fields use the [`defi_evm::rpc::wei_to_gwei`] formatter; `fetched_at` is
/// `now` rendered as RFC 3339 (UTC, `Z`). Legacy chains omit the base/priority
/// fee fields (empty → omitted from JSON).
pub async fn fetch_gas_price(
    client: &RpcClient,
    chain: &Chain,
    now: DateTime<Utc>,
) -> Result<GasPrice, Error> {
    let block_number = client.block_number().await?;
    let base_fee = client.base_fee().await?;
    let gas_price = client.gas_price().await?;

    let eip1559 = base_fee.is_some();
    let mut warnings = Vec::new();
    let mut base_fee_gwei = String::new();
    let mut priority_fee_gwei = String::new();

    if let Some(base) = base_fee {
        base_fee_gwei = wei_to_gwei(Some(base));
        let priority_fee = match client.max_priority_fee().await {
            Ok(tip) => tip,
            Err(e) => {
                warnings.push(format!("priority fee unavailable: {e}"));
                alloy::primitives::U256::ZERO
            }
        };
        priority_fee_gwei = wei_to_gwei(Some(priority_fee));
    }

    Ok(GasPrice {
        chain_id: chain.caip2.clone(),
        chain_name: chain.name.clone(),
        block_number: block_number as i64,
        eip1559,
        base_fee_gwei,
        priority_fee_gwei,
        gas_price_gwei: wei_to_gwei(Some(gas_price)),
        warnings,
        fetched_at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
    })
}

/// The resolved result of a `chains gas` invocation over one or more targets.
///
/// Mirrors the Go command's success path: an ordered list of [`GasPrice`] for
/// the chains that succeeded, the per-chain failure `warnings`, and the
/// `partial` flag (true iff at least one chain failed). A single-chain request
/// still yields a one-element `prices` vector (array-always contract).
#[derive(Debug, Clone)]
pub struct GasOutcome {
    /// Successful per-chain gas prices, in input order.
    pub prices: Vec<GasPrice>,
    /// `"chain <caip2>: <error>"` for every chain that failed.
    pub warnings: Vec<String>,
    /// Whether any chain failed.
    pub partial: bool,
}

/// Run `chains gas` across already-resolved targets, fetching in parallel and
/// preserving input order (Go `newChainsCommand` `gas` success path).
///
/// Behavior (preserved from Go):
/// * each target is fetched via [`fetch_gas_price`]; failures become
///   `"chain <caip2>: <error>"` warnings and are dropped from `prices`;
/// * if *every* chain failed → [`defi_errors::Code::Unavailable`] with an
///   `"all chains failed; …"` message;
/// * otherwise the surviving prices are returned with `partial = !warnings`.
///
/// Strict-mode partial rejection is layered by the runner, not here.
pub async fn run_gas(targets: &[GasChainTarget], now: DateTime<Utc>) -> Result<GasOutcome, Error> {
    // Fetch every chain concurrently, then reassemble in input order. Each task
    // owns a cloned chain + RPC URL so the borrow of `targets` does not escape.
    let handles: Vec<_> = targets
        .iter()
        .map(|target| {
            let chain = target.chain.clone();
            let rpc_url = target.rpc_url.clone();
            tokio::spawn(async move {
                let client = RpcClient::connect(&rpc_url)
                    .map_err(|e| Error::wrap(Code::Unavailable, "connect rpc", e))?;
                fetch_gas_price(&client, &chain, now).await
            })
        })
        .collect();

    let mut prices = Vec::with_capacity(targets.len());
    let mut warnings = Vec::new();
    for (target, handle) in targets.iter().zip(handles) {
        let result = match handle.await {
            Ok(res) => res,
            Err(join_err) => Err(Error::new(
                Code::Unavailable,
                format!("gas fetch task failed: {join_err}"),
            )),
        };
        match result {
            Ok(price) => prices.push(price),
            Err(err) => warnings.push(format!("chain {}: {}", target.chain.caip2, err)),
        }
    }

    if prices.is_empty() {
        return Err(Error::new(
            Code::Unavailable,
            format!("all chains failed; {}", warnings.join("; ")),
        ));
    }

    let partial = !warnings.is_empty();
    Ok(GasOutcome {
        prices,
        warnings,
        partial,
    })
}

/// clap parsing + handler for the `chains` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::Error;
    use defi_model::{CacheStatus, Envelope};

    use super::{ChainsAssetsRequest, ChainsTopRequest, CHAINS_DEFAULT_LIMIT, CHAINS_TTL_SECS};
    use crate::ctx::AppCtx;

    /// `chains` subcommands (Go `newChainsCommand`).
    #[derive(Subcommand, Debug)]
    pub enum ChainsCmd {
        /// List all supported chains with aliases (no keys required).
        List,
        /// Current gas prices for one or more EVM chains (no keys required).
        Gas(GasArgs),
        /// Top chains by TVL.
        Top(TopArgs),
        /// TVL by asset for a chain (DefiLlama key required).
        Assets(AssetsArgs),
    }

    impl ChainsCmd {
        /// The leaf path token (for `meta.command`).
        pub fn path(&self) -> &'static str {
            match self {
                ChainsCmd::List => "list",
                ChainsCmd::Gas(_) => "gas",
                ChainsCmd::Top(_) => "top",
                ChainsCmd::Assets(_) => "assets",
            }
        }
    }

    /// `chains gas` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct GasArgs {
        /// Chain id/name/CAIP-2 (comma-separated for multiple).
        #[arg(long)]
        pub chain: Option<String>,
        /// RPC URL override (single chain only).
        #[arg(long = "rpc-url")]
        pub rpc_url: Option<String>,
    }

    /// `chains top` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct TopArgs {
        /// Number of chains to return.
        #[arg(long, default_value_t = CHAINS_DEFAULT_LIMIT)]
        pub limit: i64,
    }

    /// `chains assets` flags.
    ///
    /// `--chain` is REQUIRED (Go cobra `MarkFlagRequired("chain")`): omitting it
    /// is a clap parse error (exit 2) before any handler runs.
    #[derive(Args, Debug, Clone, Default)]
    pub struct AssetsArgs {
        /// Chain id/name/CAIP-2.
        #[arg(long, required = true)]
        pub chain: Option<String>,
        /// Asset filter (symbol/address/CAIP-19).
        #[arg(long)]
        pub asset: Option<String>,
        /// Number of assets to return.
        #[arg(long, default_value_t = CHAINS_DEFAULT_LIMIT)]
        pub limit: i64,
    }

    /// Handle `chains <sub>`.
    ///
    /// `list`/`gas` are metadata routes (cache bypassed); `top`/`assets` are
    /// DefiLlama-backed data routes driven through the runner's cache flow. The
    /// async provider fetch is deferred into the cache-flow closure (run via
    /// [`crate::ctx::block_on_fetch`]) so a fresh cache hit short-circuits WITHOUT
    /// issuing a network call (spec §2.5). `chains assets` is key-gated: the
    /// DefiLlama adapter rejects a missing `DEFI_DEFILLAMA_API_KEY` before any
    /// network call.
    pub async fn handle(ctx: &AppCtx, cmd: ChainsCmd) -> Result<Envelope, Error> {
        match cmd {
            ChainsCmd::List => Ok(list_envelope(ctx)),
            ChainsCmd::Gas(args) => gas(ctx, args).await,
            ChainsCmd::Top(args) => top(ctx, args),
            ChainsCmd::Assets(args) => assets(ctx, args),
        }
    }

    /// Run `chains top`: top chains by TVL (DefiLlama, no key, cached).
    fn top(ctx: &AppCtx, args: TopArgs) -> Result<Envelope, Error> {
        let ttl = std::time::Duration::from_secs(CHAINS_TTL_SECS);
        let provider = ctx.defillama();
        let path = "chains top";
        let req = ChainsTopRequest { limit: args.limit };
        let key = crate::protocols::cache_key(path, &req);
        ctx.run_cached_command(path, &key, ttl, || {
            finalize(crate::ctx::block_on_fetch(super::run_top(
                &provider, args.limit,
            )))
        })
    }

    /// Run `chains assets`: TVL by asset for a chain (DefiLlama, key-gated,
    /// cached).
    ///
    /// The `--chain` (CAIP-2) + optional `--asset` filter are parsed up front so
    /// the cache key matches Go (`{"asset","chain","limit"}` → alphabetical
    /// `{"asset","chain","limit"}` JSON), and a usage error (bad chain / address
    /// filter without a known symbol) short-circuits before the cache flow.
    fn assets(ctx: &AppCtx, args: AssetsArgs) -> Result<Envelope, Error> {
        let ttl = std::time::Duration::from_secs(CHAINS_TTL_SECS);
        let provider = ctx.defillama();
        let path = "chains assets";

        let chain_arg = args.chain.clone().unwrap_or_default();
        let asset_arg = args.asset.clone().unwrap_or_default();
        // Parse the same way the fetch will, so the cache key uses the
        // cache-stable filter value and usage errors surface before any I/O.
        let chain = super::parse_chain(&chain_arg)?;
        let asset = super::parse_chain_asset_filter(&chain, &asset_arg)?;
        let req = ChainsAssetsRequest {
            asset: crate::lend::chain_asset_filter_cache_value(&asset, &asset_arg),
            chain: chain.caip2.clone(),
            limit: args.limit,
        };
        let key = crate::protocols::cache_key(path, &req);
        ctx.run_cached_command(path, &key, ttl, || {
            finalize(crate::ctx::block_on_fetch(super::run_assets(
                &provider, &chain_arg, &asset_arg, args.limit,
            )))
        })
    }

    /// Convert a [`super::ChainsOutcome`] result into the cache-flow fetch outcome
    /// tuple expected by `run_cached_command` (mirrors the `protocols` finalize).
    #[allow(clippy::type_complexity)]
    fn finalize(
        outcome: Result<super::ChainsOutcome, Error>,
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
                    name: "defillama".to_string(),
                    status: crate::protocols::status_from_result::<()>(&Err(Error::new(
                        err.code, "",
                    ))),
                    latency_ms: 0,
                };
                Err((vec![status], Vec::new(), false, err))
            }
        }
    }

    /// Build the `chains list` success envelope (metadata, cache bypassed).
    fn list_envelope(ctx: &AppCtx) -> Envelope {
        let data =
            serde_json::to_value(super::list_chains_data()).unwrap_or(serde_json::Value::Null);
        ctx.metadata_envelope("chains list", data, Vec::new())
    }

    /// Run `chains gas`: live EVM gas prices (no keys, cache bypassed). Returns
    /// an array of [`defi_model::GasPrice`] even for a single chain.
    async fn gas(ctx: &AppCtx, args: GasArgs) -> Result<Envelope, Error> {
        let targets = super::resolve_gas_targets(
            args.chain.as_deref().unwrap_or_default(),
            args.rpc_url.as_deref().unwrap_or_default(),
        )?;
        let outcome = super::run_gas(&targets, ctx.now()).await?;

        if outcome.partial && ctx.settings.strict {
            return Err(Error::new(
                defi_errors::Code::PartialStrict,
                "partial results returned in strict mode",
            ));
        }

        let data = serde_json::to_value(&outcome.prices)
            .map_err(|e| Error::wrap(defi_errors::Code::Internal, "serialize gas prices", e))?;
        let mut env = Envelope::success(
            "chains gas",
            data,
            outcome.warnings,
            CacheStatus::bypass(),
            Vec::new(),
            outcome.partial,
        );
        env.meta.timestamp = ctx.now();
        Ok(env)
    }
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::chains_cmd` (Go: `internal/app` chains)
    //!
    //! This module owns the **command-layer composition** for the `chains`
    //! group. "Correct" means it preserves the stable machine contract (design
    //! spec §2.1 envelope, §2.2 exit codes, §2.3 rendering, §2.4 ids/amounts)
    //! and the chains-specific behaviors of `internal/app/runner.go`. The
    //! criteria asserted below (NOT Go internals — the RPC plumbing + gwei
    //! formatting parity already live in `defi-evm::rpc`):
    //!
    //! 1. **`chains list` shape + golden parity.** [`list_chains_data`] yields a
    //!    `SupportedChain` per registry entry in CAIP-2 order; rendered as a
    //!    success envelope (`status="bypass"`) and with `--results-only` it is
    //!    byte-for-byte the Go binary's `chains list` golden fixtures.
    //!    `evm_chain_id`/`aliases` are omitted when zero/empty (omitempty
    //!    contract). (Spec §2.1, §2.3.)
    //! 2. **`chains list` bypasses the cache** (metadata route — spec §2.5).
    //!    Asserted via `runner::should_open_cache("chains list") == false`.
    //! 3. **`chains gas` EIP-1559 composition.** [`fetch_gas_price`] over a
    //!    base-fee chain sets `eip1559=true`, fills `base_fee_gwei` /
    //!    `priority_fee_gwei` / `gas_price_gwei` via the wei→gwei formatter
    //!    (`"1.000000"`, `"2.000000"`, `"3.000000"`), copies `chain_id`/
    //!    `chain_name`, reads `block_number` from the latest header, and renders
    //!    `fetched_at` as RFC 3339 UTC. (Go `TestFetchGasPriceEIP1559`.)
    //! 4. **`chains gas` legacy composition + omitempty.** A chain whose latest
    //!    header has no base fee → `eip1559=false`, `base_fee_gwei`/
    //!    `priority_fee_gwei` empty (and therefore omitted from JSON),
    //!    `gas_price_gwei` still set. (Go `TestFetchGasPriceLegacy`.)
    //! 5. **`chains gas` tip-cap failure is non-fatal.** An EIP-1559 chain whose
    //!    `eth_maxPriorityFeePerGas` errors still succeeds with `eip1559=true`,
    //!    `priority_fee_gwei="0.000000"`, and a `"priority fee unavailable: …"`
    //!    warning. (Go `TestFetchGasPriceTipCapFailureAddsWarning`.)
    //! 6. **`chains gas` requires a chain** → [`Code::Usage`]; empty/blank
    //!    `--chain` is rejected. (Go `TestChainsGasRequiresChainFlag`.)
    //! 7. **`chains gas` is EVM-only** → a non-EVM chain (`solana`) is rejected
    //!    with [`Code::Unsupported`], in both single and multi-chain lists. (Go
    //!    `TestChainsGasRejectsNonEVM`, `TestChainsGasRejectsNonEVMInMulti`.)
    //! 8. **`chains gas` `--rpc-url` is single-chain only** → a multi-chain
    //!    `--chain` with `--rpc-url` is [`Code::Usage`]. (Go
    //!    `TestChainsGasMultipleChainsRejectsRPCURL`.)
    //! 9. **`chains gas` array-always.** Both single- and multi-chain requests
    //!    produce a `Vec<GasPrice>` (one element for a single chain), in input
    //!    order. (Go `TestChainsGasSingleChainReturnsArray`,
    //!    `TestChainsGasMultipleChainsWithMockRPC`.)
    //! 10. **`chains gas` partial tolerance.** With multiple chains where some
    //!     fail, surviving prices are returned, failures become
    //!     `"chain <caip2>: …"` warnings, and `partial=true`. All chains failing
    //!     → [`Code::Unavailable`] (`"all chains failed; …"`). Strict-mode
    //!     partial rejection (exit 15) is the runner's responsibility, not this
    //!     module's. (Spec §2.5 partial; Go gas command success path.)
    //! 11. **`chains gas` bypasses the cache** (metadata route — spec §2.5).
    //!     Asserted via `runner::should_open_cache("chains gas") == false`.
    //!
    //! Ported from `runner_gas_test.go` (the meaningful command-composition
    //! cases). Skipped here (covered elsewhere or internal detail):
    //! * `TestWeiToGwei` + the byte-level RPC reads — owned/tested by
    //!   `defi-evm::rpc` (`wei_to_gwei`, `RpcClient`), not re-asserted here;
    //! * `TestChainsGasBypassesCache` (`shouldOpenCache`) — the routing predicate
    //!   lives in `defi-app::runner` and is asserted by its tests; we add one
    //!   confirmation here for the chains paths;
    //! * the Go `httptest` batch-vs-single request plumbing — an
    //!   ethclient/alloy transport detail, not part of the contract.

    use super::*;
    use chrono::TimeZone;
    use defi_config::Settings;
    use defi_errors::Code;
    use defi_model::Envelope;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::time::Duration;
    use wiremock::matchers::{body_partial_json, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // --- fixtures ----------------------------------------------------------

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 9, 12, 0, 0).unwrap()
    }

    fn evm_chain(name: &str, slug: &str, id: i64) -> Chain {
        Chain {
            name: name.to_string(),
            slug: slug.to_string(),
            caip2: format!("eip155:{id}"),
            evm_chain_id: id,
        }
    }

    /// Minimal results-only JSON settings for golden-parity rendering.
    fn results_only_settings() -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: true,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_secs(2),
            retries: 0,
            max_stale: Duration::from_secs(0),
            no_stale: false,
            cache_enabled: false,
            cache_path: PathBuf::new(),
            cache_lock_path: PathBuf::new(),
            action_store_path: PathBuf::new(),
            action_lock_path: PathBuf::new(),
            defillama_api_key: String::new(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    fn full_envelope_settings() -> Settings {
        let mut s = results_only_settings();
        s.results_only = false;
        s
    }

    /// Register a JSON-RPC method responder returning `result`. Mirrors
    /// `runner_gas_test.go::newMockRPCServer` (one responder per method).
    async fn mock_method(server: &MockServer, rpc_method: &str, result: Value) {
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

    async fn mock_method_error(server: &MockServer, rpc_method: &str) {
        Mock::given(method("POST"))
            .and(body_partial_json(json!({ "method": rpc_method })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": { "code": -32601, "message": "method not found" },
            })))
            .mount(server)
            .await;
    }

    fn block_result(number_hex: &str, base_fee_hex: Option<&str>) -> Value {
        let mut obj = json!({
            "number": number_hex,
            "hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "gasLimit": "0x0",
            "gasUsed": "0x0",
            "timestamp": "0x0",
        });
        match base_fee_hex {
            Some(b) => obj["baseFeePerGas"] = json!(b),
            None => obj["baseFeePerGas"] = Value::Null,
        }
        obj
    }

    /// A mock RPC server primed for a gas fetch with the given hex fee values.
    /// `base_fee_hex == None` simulates a legacy chain; `priority_ok == false`
    /// makes `eth_maxPriorityFeePerGas` return a JSON-RPC error.
    async fn gas_server(
        block_hex: &str,
        base_fee_hex: Option<&str>,
        gas_price_hex: &str,
        priority_fee_hex: &str,
        priority_ok: bool,
    ) -> MockServer {
        let server = MockServer::start().await;
        mock_method(
            &server,
            "eth_getBlockByNumber",
            block_result(block_hex, base_fee_hex),
        )
        .await;
        mock_method(&server, "eth_gasPrice", json!(gas_price_hex)).await;
        if priority_ok {
            mock_method(&server, "eth_maxPriorityFeePerGas", json!(priority_fee_hex)).await;
        } else {
            mock_method_error(&server, "eth_maxPriorityFeePerGas").await;
        }
        server
    }

    // --- 1. chains list shape + golden parity -----------------------------

    #[test]
    fn list_chains_data_includes_ethereum_in_caip2_order() {
        let chains = list_chains_data();
        assert!(!chains.is_empty());
        // First entry is Ethereum (lowest CAIP-2, eip155:1), with its alias.
        let first = &chains[0];
        assert_eq!(first.name, "Ethereum");
        assert_eq!(first.slug, "ethereum");
        assert_eq!(first.caip2, "eip155:1");
        assert_eq!(first.namespace, "eip155");
        assert_eq!(first.evm_chain_id, 1);
        assert_eq!(first.aliases, vec!["mainnet".to_string()]);
    }

    #[test]
    fn list_chains_data_results_only_matches_go_golden() {
        let data = serde_json::to_value(list_chains_data()).expect("serialize chains");
        let env = Envelope::success(
            "chains list",
            data,
            Vec::new(),
            defi_model::CacheStatus::bypass(),
            Vec::new(),
            false,
        );
        let rendered =
            defi_out::render(&env, &results_only_settings()).expect("render results-only");
        let golden = include_str!("../../../tests/golden/chains-list-results-only.json");
        assert_eq!(
            rendered.trim_end(),
            golden.trim_end(),
            "chains list --results-only must match the Go golden fixture byte-for-byte"
        );
    }

    #[test]
    fn list_chains_data_full_envelope_data_matches_go_golden_data() {
        // The full-envelope golden carries nondeterministic request_id/timestamp,
        // so compare only the `data` array (the part this module owns).
        let env_data = serde_json::to_value(list_chains_data()).expect("serialize chains");
        let golden: Value =
            serde_json::from_str(include_str!("../../../tests/golden/chains-list.json"))
                .expect("parse golden envelope");
        assert_eq!(
            &env_data,
            golden.get("data").expect("golden data array"),
            "chains list `data` must match the Go golden envelope"
        );
        assert_eq!(golden["version"], json!("v1"));
        assert_eq!(golden["success"], json!(true));
    }

    #[test]
    fn list_chains_data_omits_zero_evm_id_and_empty_aliases() {
        let chains = list_chains_data();
        // Polygon has no aliases in the golden → `aliases` omitted from JSON.
        let polygon = chains
            .iter()
            .find(|c| c.caip2 == "eip155:137")
            .expect("polygon present");
        let v = serde_json::to_value(polygon).expect("serialize polygon");
        assert!(
            v.get("aliases").is_none(),
            "empty aliases must be omitted (omitempty), got: {v}"
        );
        // A Solana (non-EVM) chain, if present, omits evm_chain_id (zero).
        if let Some(solana) = chains.iter().find(|c| c.namespace == "solana") {
            let sv = serde_json::to_value(solana).expect("serialize solana");
            assert!(
                sv.get("evm_chain_id").is_none(),
                "zero evm_chain_id must be omitted (omitempty), got: {sv}"
            );
        }
    }

    // --- 2 & 11. cache-bypass routing -------------------------------------

    #[test]
    fn chains_list_and_gas_bypass_cache() {
        assert!(
            !crate::runner::should_open_cache("chains list"),
            "chains list must bypass cache"
        );
        assert!(
            !crate::runner::should_open_cache("chains gas"),
            "chains gas must bypass cache"
        );
        // A data command in the chains group still opens the cache.
        assert!(
            crate::runner::should_open_cache("chains assets"),
            "chains assets must open cache"
        );
    }

    // --- 3. EIP-1559 composition ------------------------------------------

    #[tokio::test]
    async fn fetch_gas_price_eip1559_fills_all_fee_fields() {
        // base 1 gwei, tip 2 gwei, gas price 3 gwei, block 16.
        let server = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "0x77359400", true).await;
        let client = RpcClient::connect(&server.uri()).expect("connect");
        let chain = evm_chain("Ethereum", "ethereum", 1);

        let result = fetch_gas_price(&client, &chain, fixed_now())
            .await
            .expect("fetch gas price");

        assert_eq!(result.chain_id, "eip155:1");
        assert_eq!(result.chain_name, "Ethereum");
        assert_eq!(result.block_number, 16);
        assert!(result.eip1559);
        assert_eq!(result.base_fee_gwei, "1.000000");
        assert_eq!(result.priority_fee_gwei, "2.000000");
        assert_eq!(result.gas_price_gwei, "3.000000");
        assert_eq!(result.fetched_at, "2026-03-09T12:00:00Z");
        assert!(result.warnings.is_empty());
    }

    // --- 4. legacy composition + omitempty --------------------------------

    #[tokio::test]
    async fn fetch_gas_price_legacy_has_no_eip1559_fee_fields() {
        // No base fee => legacy chain; gas price 5 gwei.
        let server = gas_server("0x5", None, "0x12A05F200", "", false).await;
        let client = RpcClient::connect(&server.uri()).expect("connect");
        let chain = evm_chain("TestLegacy", "legacy", 999);

        let result = fetch_gas_price(&client, &chain, fixed_now())
            .await
            .expect("fetch gas price");

        assert!(!result.eip1559);
        assert_eq!(result.base_fee_gwei, "");
        assert_eq!(result.priority_fee_gwei, "");
        assert_eq!(result.gas_price_gwei, "5.000000");

        // omitempty: empty base/priority fees must be absent from JSON.
        let v = serde_json::to_value(&result).expect("serialize gas price");
        assert!(
            v.get("base_fee_gwei").is_none(),
            "legacy chain must omit base_fee_gwei, got: {v}"
        );
        assert!(
            v.get("priority_fee_gwei").is_none(),
            "legacy chain must omit priority_fee_gwei, got: {v}"
        );
    }

    // --- 5. tip-cap failure is non-fatal ----------------------------------

    #[tokio::test]
    async fn fetch_gas_price_tip_cap_failure_adds_warning_and_zero_tip() {
        // EIP-1559 chain (has base fee) but eth_maxPriorityFeePerGas errors.
        let server = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "", false).await;
        let client = RpcClient::connect(&server.uri()).expect("connect");
        let chain = evm_chain("Ethereum", "ethereum", 1);

        let result = fetch_gas_price(&client, &chain, fixed_now())
            .await
            .expect("fetch gas price should not fail on tip-cap error");

        assert!(result.eip1559);
        assert_eq!(result.priority_fee_gwei, "0.000000");
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("priority fee unavailable")),
            "expected a priority-fee-unavailable warning, got: {:?}",
            result.warnings
        );
    }

    // --- 6, 7, 8. flag/chain validation (resolve_gas_targets) -------------

    #[test]
    fn resolve_gas_targets_requires_at_least_one_chain() {
        let err = resolve_gas_targets("", "").expect_err("empty chain rejected");
        assert_eq!(err.code, Code::Usage);
        let err = resolve_gas_targets("  ,  , ", "").expect_err("blank chain rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn resolve_gas_targets_rejects_non_evm_chain() {
        let err = resolve_gas_targets("solana", "").expect_err("non-EVM rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert!(
            err.to_string().to_uppercase().contains("EVM"),
            "expected EVM-only message, got: {err}"
        );
    }

    #[test]
    fn resolve_gas_targets_rejects_non_evm_in_multi_list() {
        let err = resolve_gas_targets("1,solana", "").expect_err("non-EVM in multi rejected");
        assert_eq!(err.code, Code::Unsupported);
    }

    #[test]
    fn resolve_gas_targets_rejects_rpc_url_with_multiple_chains() {
        let err = resolve_gas_targets("1,10", "https://example.com")
            .expect_err("multi-chain + rpc-url rejected");
        assert_eq!(err.code, Code::Usage);
        assert!(
            err.to_string().contains("rpc-url"),
            "expected rpc-url message, got: {err}"
        );
    }

    #[test]
    fn resolve_gas_targets_single_chain_uses_rpc_url_override() {
        let targets = resolve_gas_targets("1", "https://override.example.test")
            .expect("single chain + override resolves");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].chain.caip2, "eip155:1");
        assert_eq!(targets[0].rpc_url, "https://override.example.test");
    }

    #[test]
    fn resolve_gas_targets_preserves_input_order_for_multi() {
        // No --rpc-url, so registry defaults are used for both; order preserved.
        let targets = resolve_gas_targets("10,1", "").expect("multi resolves with defaults");
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].chain.caip2, "eip155:10");
        assert_eq!(targets[1].chain.caip2, "eip155:1");
    }

    // --- 9. array-always (single + multi) ---------------------------------

    #[tokio::test]
    async fn run_gas_single_chain_returns_one_element_array() {
        let server = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "0x77359400", true).await;
        let targets = vec![GasChainTarget {
            chain: evm_chain("Ethereum", "ethereum", 1),
            rpc_url: server.uri(),
        }];

        let outcome = run_gas(&targets, fixed_now()).await.expect("run gas");
        assert_eq!(
            outcome.prices.len(),
            1,
            "single chain still yields an array"
        );
        assert_eq!(outcome.prices[0].chain_id, "eip155:1");
        assert!(!outcome.partial);
        assert!(outcome.warnings.is_empty());
    }

    #[tokio::test]
    async fn run_gas_multi_chain_preserves_input_order() {
        // chain1: gas 3 gwei, block 16; chain2: gas 4 gwei, block 32.
        let srv1 = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "0x77359400", true).await;
        let srv2 = gas_server("0x20", Some("0x77359400"), "0xEE6B2800", "0x3B9ACA00", true).await;
        let targets = vec![
            GasChainTarget {
                chain: evm_chain("Ethereum", "ethereum", 1),
                rpc_url: srv1.uri(),
            },
            GasChainTarget {
                chain: evm_chain("Optimism", "optimism", 10),
                rpc_url: srv2.uri(),
            },
        ];

        let outcome = run_gas(&targets, fixed_now()).await.expect("run gas");
        assert_eq!(outcome.prices.len(), 2);
        assert_eq!(outcome.prices[0].chain_id, "eip155:1");
        assert_eq!(outcome.prices[1].chain_id, "eip155:10");
        assert_eq!(outcome.prices[0].gas_price_gwei, "3.000000");
        assert_eq!(outcome.prices[1].gas_price_gwei, "4.000000");
        assert_eq!(outcome.prices[1].block_number, 32);
        assert!(!outcome.partial);
    }

    // --- 10. partial tolerance --------------------------------------------

    #[tokio::test]
    async fn run_gas_partial_drops_failures_into_warnings() {
        // chain1 succeeds; chain2 points at a dead URL (connection refused).
        let srv1 = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "0x77359400", true).await;
        let targets = vec![
            GasChainTarget {
                chain: evm_chain("Ethereum", "ethereum", 1),
                rpc_url: srv1.uri(),
            },
            GasChainTarget {
                chain: evm_chain("Optimism", "optimism", 10),
                // Unroutable port → fetch fails.
                rpc_url: "http://127.0.0.1:1".to_string(),
            },
        ];

        let outcome = run_gas(&targets, fixed_now())
            .await
            .expect("partial still succeeds");
        assert_eq!(outcome.prices.len(), 1, "only the healthy chain survives");
        assert_eq!(outcome.prices[0].chain_id, "eip155:1");
        assert!(outcome.partial);
        assert!(
            outcome.warnings.iter().any(|w| w.contains("eip155:10")),
            "failed chain must be named in warnings, got: {:?}",
            outcome.warnings
        );
    }

    #[tokio::test]
    async fn run_gas_all_chains_failed_is_unavailable() {
        let targets = vec![GasChainTarget {
            chain: evm_chain("Ethereum", "ethereum", 1),
            rpc_url: "http://127.0.0.1:1".to_string(),
        }];

        let err = run_gas(&targets, fixed_now())
            .await
            .expect_err("all-failed is an error");
        assert_eq!(err.code, Code::Unavailable);
        assert!(
            err.to_string().contains("all chains failed"),
            "expected all-chains-failed message, got: {err}"
        );
    }

    // =====================================================================
    // App-level `chains gas` (WS1, wiremock RPC end-to-end through handle /
    // run_with_args). These exercise the wired handler's full envelope + exit
    // codes via the existing `--rpc-url` seam (no DefiLlama base-URL override
    // needed). Additional success criteria over the unit cases above:
    //
    //  A1. **Single-chain handler envelope.** `chains gas --chain 1 --rpc-url
    //      <mock>` resolves a success [`Envelope`]: `version="v1"`,
    //      `success=true`, `error=None`, `data` = a ONE-element array of
    //      `GasPrice`, `meta.command="chains gas"`, `meta.cache.status="bypass"`
    //      (metadata route), `meta.providers` EMPTY (the Go gas command passes
    //      `nil` providers), `partial=false`. (Go gas command success path.)
    //  A2. **Multi-chain array + input order.** Two chains with two mock RPCs
    //      (no `--rpc-url`, registry defaults) → a two-element array in input
    //      order. (Driven via `cli::handle` with explicit targets is the unit
    //      path; here we assert the array-always contract end-to-end with one
    //      mock + a single chain, and the multi-chain `--rpc-url` rejection.)
    //  A3. **`--rpc-url` rejected with multiple chains → exit 2 (usage)**,
    //      through the full `run_with_args` path: a usage error renders the FULL
    //      envelope on stderr and returns exit code 2. (Go
    //      `TestChainsGasMultipleChainsRejectsRPCURL`.)
    //  A4. **Missing `--chain` → exit 2 (usage)** through `run_with_args`.
    //  A5. **Non-EVM chain → exit 13 (unsupported)** through `run_with_args`.
    //  A6. **Single-chain success → exit 0** through `run_with_args` with a mock
    //      RPC.
    // =====================================================================

    use crate::cli::run_with_args;
    use crate::ctx::AppCtx;
    use defi_config::MapEnv;

    /// App settings: JSON, cache bypassed (gas always bypasses anyway).
    fn app_settings() -> Settings {
        let mut s = results_only_settings();
        s.results_only = false;
        s.timeout = Duration::from_secs(5);
        s
    }

    /// A `MapEnv` whose HOME points at a temp dir so `Settings::load` can resolve
    /// cache/config paths without touching the real home directory. Returns the
    /// `TempDir` guard so the caller keeps it alive for the test's duration.
    fn env_with_home() -> (MapEnv, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let env = MapEnv::with_home(tmp.path().to_path_buf());
        (env, tmp)
    }

    fn gas_args(chain: &str, rpc_url: Option<&str>) -> super::cli::GasArgs {
        super::cli::GasArgs {
            chain: Some(chain.to_string()),
            rpc_url: rpc_url.map(str::to_string),
        }
    }

    // --- A1. single-chain handler envelope --------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_gas_handler_single_chain_full_envelope() {
        let server = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "0x77359400", true).await;
        let ctx = AppCtx::new(app_settings());

        let env = super::cli::handle(
            &ctx,
            super::cli::ChainsCmd::Gas(gas_args("1", Some(&server.uri()))),
        )
        .await
        .expect("chains gas single chain should succeed");

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert_eq!(env.meta.command, "chains gas");
        assert!(!env.meta.partial);

        // Metadata route: cache bypassed, no provider statuses.
        assert_eq!(env.meta.cache.status, "bypass");
        assert!(
            env.meta.providers.is_empty(),
            "chains gas passes nil providers (Go parity), got: {:?}",
            env.meta.providers
        );

        // Array-always: a single chain still yields a one-element array.
        let rows = env
            .data
            .as_ref()
            .and_then(Value::as_array)
            .expect("data is an array");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["chain_id"], json!("eip155:1"));
        assert_eq!(rows[0]["eip1559"], json!(true));
        assert_eq!(rows[0]["gas_price_gwei"], json!("3.000000"));
    }

    // --- A3. multi-chain + --rpc-url rejected (usage) via run_with_args ----

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_gas_multi_chain_with_rpc_url_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "chains",
                "gas",
                "--chain",
                "1,10",
                "--rpc-url",
                "https://example.test",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "multi-chain + --rpc-url must be a usage error (exit 2)"
        );
    }

    // --- A4. missing --chain (usage) via run_with_args --------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_gas_missing_chain_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        // --chain is optional at the parser; the handler rejects an empty chain
        // with CodeUsage (Go parity). Either way → exit 2.
        let code = run_with_args(["defi", "chains", "gas"], &env).await;
        assert_eq!(code, 2, "missing --chain must be a usage error (exit 2)");
    }

    // --- A5. non-EVM chain (unsupported) via run_with_args ----------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_gas_non_evm_is_unsupported_exit_13() {
        let (env, _home) = env_with_home();
        let code = run_with_args(["defi", "chains", "gas", "--chain", "solana"], &env).await;
        assert_eq!(
            code, 13,
            "a non-EVM chain must be unsupported (exit 13), got {code}"
        );
    }

    // --- A6. single-chain success → exit 0 via run_with_args --------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_gas_single_chain_success_exit_0() {
        let server = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "0x77359400", true).await;
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "chains",
                "gas",
                "--chain",
                "1",
                "--rpc-url",
                &server.uri(),
            ],
            &env,
        )
        .await;
        assert_eq!(code, 0, "a healthy single-chain gas query must exit 0");
    }
}

#[cfg(test)]
mod chains_extra_tests {
    //! # Success criteria — `chains top` / `chains assets` command composition
    //! (unit "chains-extra", WS2; Go: `internal/app/runner.go::newChainsCommand`
    //! `top` + `assets`).
    //!
    //! This module owns the **command-layer composition** for the two remaining
    //! `chains` data subcommands. "Correct" means it preserves the stable machine
    //! contract (design spec §2.1 envelope, §2.3 rendering, §2.4 ids, §2.5 cache
    //! behavior) and the chains-specific wiring of the Go runner. The DefiLlama
    //! data fetch (sort/aggregate/rank/limit/filter + key-gating) is NOT
    //! re-asserted here — it lives in (and is tested by) `defi-providers::defillama`
    //! (`chains_top_sorts_descending`, `chains_assets_requires_api_key`,
    //! `chains_assets_aggregates_sorts_and_limits`, `chains_assets_filters_by_asset`).
    //! The criteria asserted by THIS module's unit tests:
    //!
    //!  1. **`chains top` composition.** [`super::run_top`] calls
    //!     [`defi_providers::MarketDataProvider::chains_top`] with the supplied
    //!     `--limit`, serializes the returned `Vec<ChainTvl>` verbatim into `data`
    //!     (a JSON array whose element keys are `rank, chain, chain_id, tvl_usd` in
    //!     struct DECLARATION order — machine contract §2.3), and captures exactly
    //!     one provider status named after the market provider (`"defillama"`) with
    //!     `status="ok"`. (Go `top` closure.)
    //!  2. **`chains top` limit pass-through.** The `--limit` value is forwarded to
    //!     the provider unchanged (the command layer does no capping — that is the
    //!     provider's job). Asserted via a recording fake.
    //!  3. **`chains assets` composition.** [`super::run_assets`] parses the
    //!     required `--chain` (CAIP-2), parses the OPTIONAL `--asset` filter, calls
    //!     [`defi_providers::MarketDataProvider::chains_assets`] with the parsed
    //!     `Chain` + `Asset` + `--limit`, and serializes the returned
    //!     `Vec<ChainAssetTvl>` verbatim into `data` (element keys
    //!     `rank, chain, chain_id, asset, asset_id, tvl_usd` in declaration order).
    //!     One `"ok"` provider status is captured. (Go `assets` closure.)
    //!  4. **`chains assets` chain + asset pass-through.** The parsed `Chain`
    //!     (CAIP-2) and the parsed/filter `Asset` (symbol uppercased) plus the
    //!     `--limit` reach the provider unchanged. A bare symbol filter (`usdc`)
    //!     resolves to an `Asset` whose `symbol == "USDC"` on the selected chain.
    //!  5. **`chains assets` required `--chain` (usage).** An empty `--chain`
    //!     argument is a [`Code::Usage`] error reported BEFORE any provider call
    //!     (Go cobra `MarkFlagRequired("chain")` + the `ParseChain` guard). A
    //!     non-EVM / unknown chain string surfaces the `ParseChain` error
    //!     ([`Code::Usage`]).
    //!  6. **`chains assets` empty-asset filter is unfiltered.** With no `--asset`
    //!     the provider is called with a default (empty-symbol) [`Asset`], i.e. an
    //!     unfiltered request. (Go `parseChainAssetFilter("")` → zero `id.Asset`.)
    //!  7. **`chains assets` address/CAIP without a known symbol is a usage
    //!     error.** A `--asset` that parses to an address/CAIP but resolves to NO
    //!     known token symbol on the chain is rejected with [`Code::Usage`]
    //!     ("asset filter by address/CAIP requires a known token symbol"),
    //!     matching Go `parseChainAssetFilter`. (This is the behavior that
    //!     distinguishes the `chains assets` filter from the looser
    //!     `lend`/`positions` optional-asset filter.)
    //!  8. **Provider-status capture + `statusFromErr` mapping.** A successful
    //!     fetch yields one provider status with `status="ok"`; a failed fetch
    //!     surfaces the error (the command fails) and propagates the SAME error
    //!     code (`auth_error` for the missing-key case, `unavailable` otherwise).
    //!  9. **Deterministic, Go-parity cache keys.** Each subcommand keys on the Go
    //!     request map (`top` → `{"limit":N}`; `assets` → `{"asset","chain",
    //!     "limit"}` with `encoding/json` ALPHABETICAL key order →
    //!     `{"asset":"...","chain":"...","limit":N}`), through the shared
    //!     [`crate::protocols::cache_key`] formula
    //!     `hex(sha256(path | "v2" | json(req)))`. The `assets` request's `asset`
    //!     component is the cache-stable [`crate::lend::chain_asset_filter_cache_value`]
    //!     (CAIP-19 / `symbol:<UPPER>` / `raw:<UPPER>` / empty), so two different
    //!     asset filters produce different keys and the same filter is stable.
    //! 10. **Default limit + TTL.** Both subcommands default `--limit` to 20 and
    //!     use the 5-minute (`300`s) TTL (Go `--limit` default 20, `5*time.Minute`).
    //! 11. **Cache routing.** Both `chains top` and `chains assets` open the cache
    //!     (they are data routes, not metadata/execution). Asserted via
    //!     `runner::should_open_cache`.
    //!
    //! Skipped here (covered elsewhere or internal detail):
    //! * the DefiLlama sort/aggregate/rank/limit/filter + key-gating + httptest
    //!   plumbing — owned/tested by `defi-providers::defillama`;
    //! * the envelope shape/field-order + render contract — owned/tested by
    //!   `defi-model::envelope` and `defi-out`; we assert only the `data` payload
    //!   and the provider/cache `meta` this module produces;
    //! * the cache-flow state machine (fresh hit / stale fallback / strict
    //!   partial) — owned/tested by `defi-app::runner`.

    use async_trait::async_trait;
    use defi_errors::{Code, Error};
    use defi_id::{parse_chain, Asset, Chain};
    use defi_model::{self as model, CacheStatus, ChainAssetTvl, ChainTvl, Envelope, ProviderInfo};
    use defi_providers::{MarketDataProvider, Provider};
    use serde_json::Value;
    use std::sync::Mutex;

    // --- recording fake market provider ------------------------------------

    /// What the fake was asked for on its most recent `chains_*` call.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    struct CallArgs {
        /// CAIP-2 of the chain passed to `chains_assets` (empty for `chains_top`).
        chain_caip2: String,
        /// Uppercased symbol of the asset filter passed to `chains_assets`.
        asset_symbol: String,
        /// CAIP-19 asset id passed to `chains_assets` (when resolved).
        asset_id: String,
        limit: i64,
    }

    /// A `MarketDataProvider` returning canned `chains_top` / `chains_assets`
    /// lists (or a canned error) and recording the args it was called with.
    /// Mirrors the Go `fakeMarketProvider` used by the runner tests + the
    /// `FakeMarket` already used by the `protocols`/`dexes` command-layer tests.
    struct FakeMarket {
        name: String,
        top: Vec<ChainTvl>,
        assets: Vec<ChainAssetTvl>,
        /// When set, every fetch returns this error instead of the canned list.
        fail: Option<Code>,
        last_call: Mutex<CallArgs>,
    }

    impl FakeMarket {
        fn new() -> Self {
            FakeMarket {
                name: "defillama".to_string(),
                top: Vec::new(),
                assets: Vec::new(),
                fail: None,
                last_call: Mutex::new(CallArgs::default()),
            }
        }

        fn last(&self) -> CallArgs {
            self.last_call.lock().unwrap().clone()
        }

        fn err(&self) -> Error {
            Error::new(self.fail.unwrap(), "provider failed")
        }
    }

    impl Provider for FakeMarket {
        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                name: self.name.clone(),
                provider_type: "market_data".to_string(),
                requires_key: false,
                capabilities: vec!["chains.top".to_string(), "chains.assets".to_string()],
                key_env_var_name: String::new(),
                capability_auth: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl MarketDataProvider for FakeMarket {
        async fn chains_top(&self, limit: i64) -> Result<Vec<ChainTvl>, Error> {
            *self.last_call.lock().unwrap() = CallArgs {
                limit,
                ..CallArgs::default()
            };
            if self.fail.is_some() {
                return Err(self.err());
            }
            Ok(self.top.clone())
        }
        async fn chains_assets(
            &self,
            chain: Chain,
            asset: Asset,
            limit: i64,
        ) -> Result<Vec<ChainAssetTvl>, Error> {
            *self.last_call.lock().unwrap() = CallArgs {
                chain_caip2: chain.caip2.clone(),
                asset_symbol: asset.symbol.to_ascii_uppercase(),
                asset_id: asset.asset_id.clone(),
                limit,
            };
            if self.fail.is_some() {
                return Err(self.err());
            }
            Ok(self.assets.clone())
        }
        async fn protocols_top(
            &self,
            _category: &str,
            _chain: &str,
            _limit: i64,
        ) -> Result<Vec<model::ProtocolTvl>, Error> {
            Ok(Vec::new())
        }
        async fn protocols_categories(&self) -> Result<Vec<model::ProtocolCategory>, Error> {
            Ok(Vec::new())
        }
        async fn stablecoins_top(
            &self,
            _peg_type: &str,
            _limit: i64,
        ) -> Result<Vec<model::Stablecoin>, Error> {
            Ok(Vec::new())
        }
        async fn stablecoin_chains(
            &self,
            _limit: i64,
        ) -> Result<Vec<model::StablecoinChain>, Error> {
            Ok(Vec::new())
        }
        async fn protocols_fees(
            &self,
            _category: &str,
            _chain: &str,
            _limit: i64,
        ) -> Result<Vec<model::ProtocolFees>, Error> {
            Ok(Vec::new())
        }
        async fn protocols_revenue(
            &self,
            _category: &str,
            _chain: &str,
            _limit: i64,
        ) -> Result<Vec<model::ProtocolRevenue>, Error> {
            Ok(Vec::new())
        }
        async fn dexes_volume(
            &self,
            _chain: &str,
            _limit: i64,
        ) -> Result<Vec<model::DexVolume>, Error> {
            Ok(Vec::new())
        }
    }

    fn sample_chain_tvl() -> ChainTvl {
        ChainTvl {
            rank: 1,
            chain: "Ethereum".to_string(),
            chain_id: "eip155:1".to_string(),
            tvl_usd: 50_000_000.0,
        }
    }

    fn sample_chain_asset_tvl() -> ChainAssetTvl {
        ChainAssetTvl {
            rank: 1,
            chain: "Ethereum".to_string(),
            chain_id: "eip155:1".to_string(),
            asset: "USDC".to_string(),
            asset_id: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".to_string(),
            tvl_usd: 225.0,
        }
    }

    /// First element of the `data` array as an object.
    fn first_row(data: &Value) -> &serde_json::Map<String, Value> {
        data.as_array()
            .expect("data is an array")
            .first()
            .expect("at least one row")
            .as_object()
            .expect("row is an object")
    }

    // --- 1. chains top composition ----------------------------------------

    #[tokio::test]
    async fn run_top_serializes_rows_in_declaration_order_and_captures_ok_status() {
        let mut p = FakeMarket::new();
        p.top = vec![sample_chain_tvl()];

        let out = super::run_top(&p, 20).await.expect("run_top success");

        assert_eq!(out.provider.name, "defillama");
        assert_eq!(out.provider.status, "ok");

        let row = first_row(&out.data);
        assert_eq!(row["rank"], Value::from(1));
        assert_eq!(row["chain"], Value::from("Ethereum"));
        assert_eq!(row["chain_id"], Value::from("eip155:1"));
        assert!(row.contains_key("tvl_usd"));
        let keys: Vec<&String> = row.keys().collect();
        assert_eq!(keys, vec!["rank", "chain", "chain_id", "tvl_usd"]);

        // Rendered into a success envelope, `data` round-trips the rows.
        let env = Envelope::success(
            "chains top",
            out.data.clone(),
            Vec::new(),
            CacheStatus::bypass(),
            vec![out.provider.clone()],
            false,
        );
        assert!(env.success);
        assert_eq!(env.meta.providers.len(), 1);
        assert_eq!(
            env.data.as_ref().and_then(Value::as_array).map(Vec::len),
            Some(1)
        );
    }

    // --- 2. chains top limit pass-through ---------------------------------

    #[tokio::test]
    async fn run_top_forwards_limit_verbatim() {
        let p = FakeMarket::new();
        let _ = super::run_top(&p, 7).await.expect("run_top success");
        assert_eq!(p.last().limit, 7);
    }

    #[tokio::test]
    async fn run_top_empty_result_serializes_as_empty_array() {
        let p = FakeMarket::new(); // no rows
        let out = super::run_top(&p, 20).await.expect("run_top success");
        assert_eq!(out.data, Value::Array(Vec::new()));
        assert_eq!(out.provider.status, "ok");
    }

    // --- 3. chains assets composition -------------------------------------

    #[tokio::test]
    async fn run_assets_serializes_rows_in_declaration_order_and_captures_ok_status() {
        let mut p = FakeMarket::new();
        p.assets = vec![sample_chain_asset_tvl()];

        let out = super::run_assets(&p, "1", "USDC", 20)
            .await
            .expect("run_assets success");

        assert_eq!(out.provider.name, "defillama");
        assert_eq!(out.provider.status, "ok");

        let row = first_row(&out.data);
        assert_eq!(row["rank"], Value::from(1));
        assert_eq!(row["chain"], Value::from("Ethereum"));
        assert_eq!(row["chain_id"], Value::from("eip155:1"));
        assert_eq!(row["asset"], Value::from("USDC"));
        assert!(row.contains_key("asset_id"));
        assert!(row.contains_key("tvl_usd"));
        let keys: Vec<&String> = row.keys().collect();
        assert_eq!(
            keys,
            vec!["rank", "chain", "chain_id", "asset", "asset_id", "tvl_usd"]
        );
    }

    // --- 4. chains assets chain + asset pass-through ----------------------

    #[tokio::test]
    async fn run_assets_forwards_parsed_chain_asset_and_limit() {
        let p = FakeMarket::new();
        // `1` parses to eip155:1; a bare `usdc` symbol filter uppercases to USDC
        // and resolves to the canonical USDC asset on Ethereum.
        let _ = super::run_assets(&p, "1", "usdc", 5)
            .await
            .expect("run_assets success");
        let call = p.last();
        assert_eq!(call.chain_caip2, "eip155:1");
        assert_eq!(call.asset_symbol, "USDC");
        assert_eq!(call.limit, 5);
    }

    // --- 5. chains assets required --chain (usage) ------------------------

    #[tokio::test]
    async fn run_assets_empty_chain_is_usage_before_provider_call() {
        let p = FakeMarket::new();
        let err = super::run_assets(&p, "", "USDC", 20)
            .await
            .expect_err("empty --chain is rejected");
        assert_eq!(err.code, Code::Usage);
        // No provider call happened (the chain guard short-circuits).
        assert_eq!(p.last(), CallArgs::default());
    }

    #[tokio::test]
    async fn run_assets_unknown_chain_surfaces_parse_error() {
        let p = FakeMarket::new();
        let err = super::run_assets(&p, "boguschainxyz", "", 20)
            .await
            .expect_err("unknown chain is rejected");
        // ParseChain failure is a usage error (Go `ParseChain` → CodeUsage).
        assert_eq!(err.code, Code::Usage);
    }

    // --- 6. chains assets empty-asset filter is unfiltered ----------------

    #[tokio::test]
    async fn run_assets_empty_asset_filter_is_unfiltered() {
        let p = FakeMarket::new();
        let _ = super::run_assets(&p, "1", "", 20)
            .await
            .expect("run_assets success");
        let call = p.last();
        assert_eq!(call.chain_caip2, "eip155:1");
        // Default (empty) asset symbol => unfiltered request.
        assert_eq!(call.asset_symbol, "");
        assert_eq!(call.asset_id, "");
    }

    // --- 7. chains assets address/CAIP without known symbol is usage ------

    #[tokio::test]
    async fn run_assets_address_without_known_symbol_is_usage() {
        let p = FakeMarket::new();
        // An address that resolves to no known token symbol on Ethereum must be
        // rejected (Go `parseChainAssetFilter` requires a known symbol for
        // address/CAIP filters). A clearly-unregistered address is used.
        let err = super::run_assets(&p, "1", "0x000000000000000000000000000000000000dead", 20)
            .await
            .expect_err("address without a known symbol must be a usage error");
        assert_eq!(err.code, Code::Usage);
        // The guard rejects before any provider call.
        assert_eq!(p.last(), CallArgs::default());
    }

    // --- 8. provider-status capture + error propagation -------------------

    #[tokio::test]
    async fn run_top_propagates_provider_error_with_same_code() {
        let mut p = FakeMarket::new();
        p.fail = Some(Code::Unavailable);
        let err = super::run_top(&p, 20)
            .await
            .expect_err("provider failure propagates");
        assert_eq!(err.code, Code::Unavailable);
    }

    #[tokio::test]
    async fn run_assets_propagates_auth_error_with_same_code() {
        let mut p = FakeMarket::new();
        p.fail = Some(Code::Auth);
        let err = super::run_assets(&p, "1", "USDC", 20)
            .await
            .expect_err("provider auth failure propagates");
        assert_eq!(err.code, Code::Auth);
    }

    // --- 9. deterministic, Go-parity cache keys ---------------------------

    #[test]
    fn chains_top_cache_request_serializes_as_single_limit_key() {
        // Go keys `chains top` on `map[string]any{"limit":N}` →
        // `{"limit":N}`. The Rust request must serialize identically.
        let req = super::ChainsTopRequest { limit: 20 };
        let json = serde_json::to_string(&req).expect("serialize");
        assert_eq!(json, r#"{"limit":20}"#);
    }

    #[test]
    fn chains_assets_cache_request_serializes_with_alphabetical_keys() {
        // Go keys `chains assets` on `map[string]any{"chain","asset","limit"}`,
        // whose `json.Marshal` emits keys ALPHABETICALLY:
        // `{"asset":"...","chain":"...","limit":N}`.
        let req = super::ChainsAssetsRequest {
            asset: "symbol:USDC".to_string(),
            chain: "eip155:1".to_string(),
            limit: 20,
        };
        let json = serde_json::to_string(&req).expect("serialize");
        assert_eq!(
            json,
            r#"{"asset":"symbol:USDC","chain":"eip155:1","limit":20}"#
        );
    }

    #[test]
    fn chains_top_cache_key_is_deterministic_hex_and_limit_sensitive() {
        let a = crate::protocols::cache_key("chains top", &super::ChainsTopRequest { limit: 20 });
        let b = crate::protocols::cache_key("chains top", &super::ChainsTopRequest { limit: 20 });
        assert_eq!(a, b, "identical inputs => identical key");
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(
            a,
            crate::protocols::cache_key("chains top", &super::ChainsTopRequest { limit: 5 }),
            "limit participates in the key"
        );
    }

    #[test]
    fn chains_assets_cache_key_changes_with_asset_filter() {
        let base = crate::protocols::cache_key(
            "chains assets",
            &super::ChainsAssetsRequest {
                asset: String::new(),
                chain: "eip155:1".to_string(),
                limit: 20,
            },
        );
        let filtered = crate::protocols::cache_key(
            "chains assets",
            &super::ChainsAssetsRequest {
                asset: "symbol:USDC".to_string(),
                chain: "eip155:1".to_string(),
                limit: 20,
            },
        );
        assert_ne!(base, filtered, "asset filter participates in the key");
        // Different chains differ too.
        let other_chain = crate::protocols::cache_key(
            "chains assets",
            &super::ChainsAssetsRequest {
                asset: String::new(),
                chain: "eip155:10".to_string(),
                limit: 20,
            },
        );
        assert_ne!(base, other_chain, "chain participates in the key");
    }

    #[test]
    fn chains_assets_request_asset_is_cache_stable_filter_value() {
        // The `asset` field of the request is the cache-stable filter value
        // (Go `chainAssetFilterCacheValue`): a bare symbol → `symbol:<UPPER>`.
        let chain = parse_chain("1").expect("parse chain");
        let asset = crate::lend::parse_optional_chain_asset(&chain, "usdc").expect("parse usdc");
        let cache_value = crate::lend::chain_asset_filter_cache_value(&asset, "usdc");
        // For a resolved symbol with a known asset id, the cache value is the
        // CAIP-19 id; for a symbol-only filter it is `symbol:USDC`. Either way it
        // is non-empty and uppercase-stable.
        assert!(!cache_value.is_empty());
        assert_eq!(cache_value, cache_value.trim());
    }

    // --- 10. default limit + TTL ------------------------------------------

    #[test]
    fn chains_extra_default_limit_and_ttl_match_go() {
        assert_eq!(super::CHAINS_DEFAULT_LIMIT, 20);
        assert_eq!(super::CHAINS_TTL_SECS, 300);
    }

    // --- 11. cache routing ------------------------------------------------

    #[test]
    fn chains_top_and_assets_open_the_cache() {
        assert!(
            crate::runner::should_open_cache("chains top"),
            "\"chains top\" is a data route and must open the cache"
        );
        assert!(
            crate::runner::should_open_cache("chains assets"),
            "\"chains assets\" is a data route and must open the cache"
        );
    }
}

#[cfg(test)]
mod chains_extra_app_tests {
    //! # Success criteria — app-level `chains top` / `chains assets` (WS2,
    //! wiremock + `run_with_args` end-to-end).
    //!
    //! These tests exercise the **wired command-group handler**
    //! ([`super::cli::handle`]) and the full `run_with_args` path. `chains top` is
    //! a no-key DefiLlama read driven against a `wiremock` server via the
    //! [`AppCtx`] base-URL seam ([`AppCtx::with_defillama_base`]); `chains assets`
    //! is **key-gated** (DefiLlama), so the offline-deterministic assertions cover
    //! both the gated success path (with a key + mock) and the no-key auth gate +
    //! usage gates (which fail BEFORE any network call, so they are safe to drive
    //! through `run_with_args` without a live API). Asserted:
    //!
    //!  A1. **`chains top` wiremock reachability + full envelope.** With the
    //!      DefiLlama `api_base` retargeted at the mock and `--no-cache`,
    //!      `chains top` MUST issue `GET /v2/chains` to the mock (RED gap: the
    //!      handler is `unimplemented!`/stubbed and never contacts it). The
    //!      resolved [`Envelope`] has `version="v1"`, `success=true`,
    //!      `error=None`, `data` = the JSON `ChainTvl` array (element keys
    //!      `rank, chain, chain_id, tvl_usd` in declaration order, sorted
    //!      descending by TVL by the provider), `meta.command="chains top"`,
    //!      `partial=false`, one `defillama` provider status `status="ok"`,
    //!      `meta.cache.status="miss"` (cache disabled).
    //!  A2. **`chains top` cache write → hit.** With a real temp cache the first
    //!      call writes (`status="write"`) and a second identical call is a fresh
    //!      `"hit"` with NO second provider request (mock `expect(1)`).
    //!  A3. **`chains top` provider error → non-zero exit.** A 503 from DefiLlama
    //!      surfaces as a typed `Error` whose code maps to a non-zero exit code,
    //!      originating from the injected mock (deterministic/offline).
    //!  A4. **`chains assets` key-gated success.** With a DefiLlama API key set and
    //!      the bridge/chainAssets base retargeted at a mock, `chains assets
    //!      --chain 1 --asset USDC` MUST issue `GET /<key>/api/chainAssets`,
    //!      build a success envelope (`meta.command="chains assets"`, one `"ok"`
    //!      provider status, `data` a `ChainAssetTvl` array with element keys
    //!      `rank, chain, chain_id, asset, asset_id, tvl_usd`).
    //!  A5. **`chains assets` key-gating (no key) → exit 10 (auth)** through the
    //!      full `run_with_args` path. The provider's
    //!      `require_chain_assets_api_key` rejects BEFORE any network call, so this
    //!      is offline + deterministic; the FULL error envelope is rendered on
    //!      stderr and the exit code is 10. (Go oracle: exit 10, message
    //!      "defillama chain asset tvl requires DEFI_DEFILLAMA_API_KEY".)
    //!  A6. **`chains assets` required `--chain` → exit 2 (usage)** through
    //!      `run_with_args` (clap-level required flag or the handler's chain
    //!      guard). (Go oracle: exit 2, "required flag(s) \"chain\" not set".)
    //!  A7. **`chains assets` unknown chain → exit 2 (usage)** through
    //!      `run_with_args` (`ParseChain` failure). (Go oracle: exit 2,
    //!      "unsupported chain input: ...".)
    //!  A8. **Flag parsing.** `chains top --limit 7` parses to `limit=7`;
    //!      `chains top` defaults to `limit=20`. `chains assets --chain 1 --asset
    //!      USDC --limit 5` parses chain/asset/limit; `chains assets` defaults
    //!      `--limit` to 20.

    use super::cli::{handle, AssetsArgs, ChainsCmd, TopArgs};
    use crate::cli::run_with_args;
    use crate::ctx::AppCtx;
    use defi_config::{MapEnv, Settings};
    use defi_model::Envelope;
    use serde_json::Value;
    use std::path::PathBuf;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// JSON settings, caching DISABLED, no provider key (default for app tests).
    fn no_cache_settings() -> Settings {
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
            cache_path: PathBuf::new(),
            cache_lock_path: PathBuf::new(),
            action_store_path: PathBuf::new(),
            action_lock_path: PathBuf::new(),
            defillama_api_key: String::new(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    /// Settings backed by a real temp sqlite cache (for write/hit tests).
    fn cache_settings(dir: &std::path::Path) -> Settings {
        let mut s = no_cache_settings();
        s.cache_enabled = true;
        s.cache_path = dir.join("cache.db");
        s.cache_lock_path = dir.join("cache.lock");
        s
    }

    /// Settings carrying a DefiLlama API key (for the key-gated assets path).
    fn keyed_settings() -> Settings {
        let mut s = no_cache_settings();
        s.defillama_api_key = "test-key".to_string();
        s
    }

    fn env_with_home() -> (MapEnv, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let env = MapEnv::with_home(tmp.path().to_path_buf());
        (env, tmp)
    }

    fn chains_top_body() -> &'static str {
        r#"[ {"name":"Arbitrum","tvl":2000}, {"name":"Ethereum","tvl":50000} ]"#
    }

    fn chain_assets_body() -> &'static str {
        r#"{
            "Ethereum":{
                "canonical":{"total":"250.5","breakdown":{"USDC":"100","USDT":"150.5"}},
                "thirdParty":{"total":"125","breakdown":{"USDC":"125"}}
            },
            "timestamp":1752843956
        }"#
    }

    fn data_array(env: &Envelope) -> Vec<Value> {
        env.data
            .as_ref()
            .and_then(Value::as_array)
            .cloned()
            .expect("data is an array")
    }

    fn top_args(limit: i64) -> TopArgs {
        TopArgs { limit }
    }

    fn assets_args(chain: &str, asset: Option<&str>, limit: i64) -> AssetsArgs {
        AssetsArgs {
            chain: Some(chain.to_string()),
            asset: asset.map(str::to_string),
            limit,
        }
    }

    // --- A1. chains top wiremock + full envelope --------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_top_handler_hits_wiremock_and_builds_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/chains"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(chains_top_body(), "application/json"),
            )
            .mount(&server)
            .await;

        let ctx = AppCtx::new(no_cache_settings()).with_defillama_base(&server.uri());
        let env = handle(&ctx, ChainsCmd::Top(top_args(20)))
            .await
            .expect("chains top should succeed against the mock");

        assert_eq!(
            server.received_requests().await.unwrap_or_default().len(),
            1,
            "handler must issue exactly one GET /v2/chains to the injected mock"
        );

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert_eq!(env.meta.command, "chains top");
        assert!(!env.meta.partial);

        let rows = data_array(&env);
        assert_eq!(rows.len(), 2);
        // Sorted descending by TVL by the provider: Ethereum first.
        assert_eq!(rows[0]["chain"], Value::from("Ethereum"));
        let keys: Vec<&String> = rows[0].as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["rank", "chain", "chain_id", "tvl_usd"]);

        assert_eq!(env.meta.providers.len(), 1);
        assert_eq!(env.meta.providers[0].name, "defillama");
        assert_eq!(env.meta.providers[0].status, "ok");
        assert_eq!(env.meta.cache.status, "miss");
    }

    // --- A2. chains top cache write then hit ------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_top_caches_write_then_hit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/chains"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(chains_top_body(), "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(cache_settings(tmp.path())).with_defillama_base(&server.uri());

        let first = handle(&ctx, ChainsCmd::Top(top_args(20)))
            .await
            .expect("first chains top");
        assert_eq!(first.meta.cache.status, "write");

        let second = handle(&ctx, ChainsCmd::Top(top_args(20)))
            .await
            .expect("second chains top");
        assert_eq!(second.meta.cache.status, "hit");
        assert!(!second.meta.cache.stale);

        drop(server);
    }

    // --- A3. chains top provider error → non-zero exit --------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_top_provider_error_propagates() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/chains"))
            .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
            .mount(&server)
            .await;

        let ctx = AppCtx::new(no_cache_settings()).with_defillama_base(&server.uri());
        let err = handle(&ctx, ChainsCmd::Top(top_args(20)))
            .await
            .expect_err("a 503 from DefiLlama must surface as a typed error");

        assert!(
            !server
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty(),
            "the 503 error must originate from the injected mock, not the live API"
        );
        assert_ne!(
            defi_errors::exit_code(&Err(defi_errors::Error::new(err.code, ""))),
            0,
            "provider error must map to a non-zero exit code, got code {:?}",
            err.code
        );
    }

    // --- A4. chains assets key-gated success ------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_assets_handler_key_gated_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/test-key/api/chainAssets"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(chain_assets_body(), "application/json"),
            )
            .mount(&server)
            .await;

        // The chainAssets endpoint is served off the bridge/pro base, which
        // `with_defillama_base` retargets via `set_bridge_base_url`.
        let ctx = AppCtx::new(keyed_settings()).with_defillama_base(&server.uri());
        let env = handle(&ctx, ChainsCmd::Assets(assets_args("1", Some("USDC"), 20)))
            .await
            .expect("chains assets should succeed with a key against the mock");

        assert_eq!(
            server.received_requests().await.unwrap_or_default().len(),
            1,
            "handler must issue exactly one GET /<key>/api/chainAssets to the mock"
        );

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert_eq!(env.meta.command, "chains assets");

        let rows = data_array(&env);
        assert_eq!(rows.len(), 1, "USDC filter yields a single aggregated row");
        assert_eq!(rows[0]["asset"], Value::from("USDC"));
        // 100 (canonical) + 125 (thirdParty) aggregated. The `tvl_usd` field uses
        // the Go `encoding/json` float serializer (`go_float`), so a whole-valued
        // float drops its fraction → JSON integer `225` (Go parity), not `225.0`.
        assert_eq!(rows[0]["tvl_usd"], Value::from(225));
        let keys: Vec<&String> = rows[0].as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            vec!["rank", "chain", "chain_id", "asset", "asset_id", "tvl_usd"]
        );

        assert_eq!(env.meta.providers.len(), 1);
        assert_eq!(env.meta.providers[0].name, "defillama");
        assert_eq!(env.meta.providers[0].status, "ok");
    }

    // --- A5. chains assets no key → exit 10 (auth) via run_with_args ------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_assets_no_key_is_auth_exit_10() {
        // No DEFI_DEFILLAMA_API_KEY in the env: the provider rejects BEFORE any
        // network call, so this is deterministic + offline. Go oracle: exit 10.
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi", "chains", "assets", "--chain", "1", "--asset", "USDC",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 10,
            "chains assets without a DefiLlama key must be an auth error (exit 10)"
        );
    }

    // --- A6. chains assets required --chain → exit 2 (usage) --------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_assets_missing_chain_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(["defi", "chains", "assets", "--asset", "USDC"], &env).await;
        assert_eq!(
            code, 2,
            "chains assets without --chain must be a usage error (exit 2)"
        );
    }

    // --- A7. chains assets unknown chain → exit 2 (usage) -----------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_assets_unknown_chain_is_usage_exit_2() {
        // Provide a key so the chain guard (not the key gate) is what fails.
        let tmp = tempfile::tempdir().expect("tempdir");
        let env =
            MapEnv::with_home(tmp.path().to_path_buf()).set("DEFI_DEFILLAMA_API_KEY", "test-key");
        let code = run_with_args(
            ["defi", "chains", "assets", "--chain", "boguschainxyz"],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "chains assets with an unknown chain must be a usage error (exit 2)"
        );
    }

    // --- A8. flag parsing -------------------------------------------------

    #[test]
    fn chains_top_flags_parse_with_defaults() {
        use clap::Parser;
        let cli =
            crate::cli::Cli::try_parse_from(["defi", "chains", "top"]).expect("chains top parses");
        if let crate::cli::TopCommand::Chains {
            cmd: ChainsCmd::Top(args),
        } = cli.command
        {
            assert_eq!(args.limit, 20, "chains top --limit defaults to 20");
        } else {
            panic!("expected chains top");
        }

        let cli = crate::cli::Cli::try_parse_from(["defi", "chains", "top", "--limit", "7"])
            .expect("chains top --limit parses");
        if let crate::cli::TopCommand::Chains {
            cmd: ChainsCmd::Top(args),
        } = cli.command
        {
            assert_eq!(args.limit, 7);
        } else {
            panic!("expected chains top");
        }
    }

    #[test]
    fn chains_assets_flags_parse_with_defaults() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "defi", "chains", "assets", "--chain", "1", "--asset", "USDC", "--limit", "5",
        ])
        .expect("chains assets flags parse");
        if let crate::cli::TopCommand::Chains {
            cmd: ChainsCmd::Assets(args),
        } = cli.command
        {
            assert_eq!(args.chain.as_deref(), Some("1"));
            assert_eq!(args.asset.as_deref(), Some("USDC"));
            assert_eq!(args.limit, 5);
        } else {
            panic!("expected chains assets");
        }

        // --limit defaults to 20 when omitted (chain still supplied).
        let cli = crate::cli::Cli::try_parse_from(["defi", "chains", "assets", "--chain", "1"])
            .expect("chains assets default limit parses");
        if let crate::cli::TopCommand::Chains {
            cmd: ChainsCmd::Assets(args),
        } = cli.command
        {
            assert_eq!(args.limit, 20, "chains assets --limit defaults to 20");
        } else {
            panic!("expected chains assets");
        }

        // Missing required --chain is a parse error (Go MarkFlagRequired("chain")).
        assert!(
            crate::cli::Cli::try_parse_from(["defi", "chains", "assets", "--asset", "USDC"])
                .is_err(),
            "chains assets without --chain must be a clap parse error"
        );
    }
}
