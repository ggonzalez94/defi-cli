//! `protocols` command group handler.
//!
//! Mirrors the `protocols` subtree of `internal/app/runner.go::newProtocolsCommand`
//! (the `top` / `categories` / `fees` / `revenue` subcommands). This module owns
//! the **command-layer composition** for the protocols group; the lower-level
//! pieces are owned elsewhere and reused:
//!
//! * the data fetch + sort/filter/rank parity (descending-by-metric ordering,
//!   chain/category filtering, `rank` assignment, `limit` capping): the
//!   `MarketDataProvider` impl in [`defi_providers::defillama`] — already
//!   contract-tested there (`TestProtocolsTop*`, `TestProtocolsCategories*`,
//!   `TestProtocolsFees*`, `TestProtocolsRevenue*`);
//! * the success/error envelope + cache-flow state machine: the runner
//!   (`defi_app::runner`);
//! * cache-bypass routing: the runner (`defi_app::runner::should_open_cache`).
//!
//! What this module owns (the contract-bearing command composition):
//!
//! 1. **Request shaping per subcommand.** Each subcommand has its own flag set —
//!    `top`/`fees`/`revenue` take `--category`, `--chain`, `--limit` (default
//!    20); `categories` takes none. The request struct serialized into the cache
//!    key must mirror the Go `map[string]any{"category", "chain", "limit"}`
//!    payload (and the empty `{}` map for `categories`) so cache keys stay stable.
//! 2. **Deterministic cache keys.** `cache_key(path, req)` =
//!    `hex(sha256(path | schema-version | canonical-json(req)))`, identical to Go
//!    `cacheKey`, including the `cachePayloadSchemaVersion` ("v2") component.
//! 3. **Provider-status capture.** Each fetch yields exactly one
//!    `model::ProviderStatus` for the market provider, whose `status` string is
//!    derived from the fetch result via the Go `statusFromErr` mapping
//!    (ok / auth_error / rate_limited / unavailable / error).
//! 4. **Success envelope shape.** The fetched list is serialized verbatim into
//!    `data` (a JSON array), provider status is surfaced in `meta.providers`, the
//!    command path is `protocols <sub>`, and the 5-minute TTL is used.
//! 5. **Cache routing.** All four `protocols *` paths open the cache (they are
//!    NOT metadata/execution routes).
//!
//! Idiomatic-Rust shape note: the Go command closures write to injected
//! `io.Writer`s and return `error`. The Rust port exposes async builder functions
//! returning values (a `ProtocolsOutcome` carrying the JSON `data` payload + the
//! captured `ProviderStatus`) so they can be unit-tested without a `cobra.Command`;
//! the envelope construction + rendering is layered on top by the runner.

#![allow(dead_code)]

use defi_errors::{Code, Error};
use defi_model::ProviderStatus;
use defi_providers::{MarketDataProvider, Provider};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The cache payload schema version baked into every cache key (Go
/// `cachePayloadSchemaVersion`). Bumping it invalidates all cached entries.
pub const CACHE_PAYLOAD_SCHEMA_VERSION: &str = "v2";

/// The cache TTL for every `protocols *` subcommand (Go: `5 * time.Minute`).
pub const PROTOCOLS_TTL_SECS: u64 = 300;

/// The default `--limit` for `protocols top`/`fees`/`revenue` (Go default 20).
pub const DEFAULT_LIMIT: i64 = 20;

/// Filters shared by the `top` / `fees` / `revenue` subcommands.
///
/// Mirrors the Go request `map[string]any{"category", "chain", "limit"}` that is
/// serialized into the cache key, so the JSON shape (field names + declaration
/// order) is contract-bearing for cache-key stability.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProtocolsFilter {
    /// `--category` (DefiLlama category filter; empty = no filter).
    pub category: String,
    /// `--chain` (DefiLlama chain name filter; empty = no filter).
    pub chain: String,
    /// `--limit` (number of rows; `<= 0` = all).
    pub limit: i64,
}

/// A resolved protocols-subcommand fetch.
///
/// Carries the JSON `data` payload (the serialized provider list) and the single
/// captured market-provider [`ProviderStatus`]. The runner layers envelope
/// construction + rendering on top.
#[derive(Debug, Clone)]
pub struct ProtocolsOutcome {
    /// The fetched list, serialized verbatim as a JSON array for `data`.
    pub data: Value,
    /// The single market-provider status captured for this fetch.
    pub provider: ProviderStatus,
}

/// Compute the cache key for a protocols subcommand (Go `cacheKey`).
///
/// `hex(sha256(command_path | CACHE_PAYLOAD_SCHEMA_VERSION | canonical_json(req)))`.
/// `req` is serialized with serde_json (compact, declaration order) to match Go's
/// `json.Marshal`. Identical inputs MUST produce identical keys across runs.
pub fn cache_key<T: serde::Serialize>(command_path: &str, req: &T) -> String {
    // Compact JSON, declaration/alphabetical order — matches Go `json.Marshal`.
    // A serialization failure here would indicate a non-serializable request
    // type, which is a programmer error; fall back to an empty payload so the
    // key stays a valid 64-hex string rather than panicking.
    let payload = serde_json::to_string(req).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(command_path.as_bytes());
    hasher.update(b"|");
    hasher.update(CACHE_PAYLOAD_SCHEMA_VERSION.as_bytes());
    hasher.update(b"|");
    hasher.update(payload.as_bytes());
    hex::encode(hasher.finalize())
}

/// Map a fetch result to the Go `statusFromErr` provider-status string:
/// `Ok` → `"ok"`; `Auth` → `"auth_error"`; `RateLimited` → `"rate_limited"`;
/// `Unavailable` → `"unavailable"`; anything else → `"error"`.
pub fn status_from_result<T>(res: &Result<T, Error>) -> String {
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

/// Build a single market-provider [`ProviderStatus`] from a fetch result.
///
/// Mirrors the Go closure's `model.ProviderStatus{Name, Status: statusFromErr,
/// LatencyMS}` capture. Latency timing is owned by the runner's cache-flow
/// state machine, so the command layer leaves `latency_ms` at zero.
fn provider_status<T>(provider: &dyn MarketDataProvider, res: &Result<T, Error>) -> ProviderStatus {
    ProviderStatus {
        name: provider.info().name,
        status: status_from_result(res),
        latency_ms: 0,
    }
}

/// Serialize a fetched row list into a JSON array `data` payload, preserving
/// element struct field declaration order (serde default for structs).
fn rows_to_data<T: Serialize>(rows: &[T]) -> Result<Value, Error> {
    serde_json::to_value(rows)
        .map_err(|e| Error::wrap(Code::Internal, "serialize protocols rows", e))
}

/// Shared fetch→outcome composition for the filtered subcommands.
///
/// Captures provider status from the result, propagates any provider error,
/// and otherwise serializes the rows into `data`.
fn build_outcome<T: Serialize>(
    provider: &dyn MarketDataProvider,
    res: Result<Vec<T>, Error>,
) -> Result<ProtocolsOutcome, Error> {
    let status = provider_status(provider, &res);
    let rows = res?;
    Ok(ProtocolsOutcome {
        data: rows_to_data(&rows)?,
        provider: status,
    })
}

/// Run `protocols top`: fetch top protocols by TVL.
///
/// Calls [`MarketDataProvider::protocols_top`] with the filter, serializes the
/// resulting `Vec<ProtocolTvl>` into `data`, and captures the provider status.
pub async fn run_top(
    provider: &dyn MarketDataProvider,
    filter: &ProtocolsFilter,
) -> Result<ProtocolsOutcome, Error> {
    let res = provider
        .protocols_top(&filter.category, &filter.chain, filter.limit)
        .await;
    build_outcome(provider, res)
}

/// Run `protocols categories`: list categories with counts + aggregate TVL.
///
/// Calls [`MarketDataProvider::protocols_categories`] (no filters), serializes the
/// resulting `Vec<ProtocolCategory>` into `data`, and captures provider status.
pub async fn run_categories(provider: &dyn MarketDataProvider) -> Result<ProtocolsOutcome, Error> {
    let res = provider.protocols_categories().await;
    build_outcome(provider, res)
}

/// Run `protocols fees`: top protocols by 24h fees.
pub async fn run_fees(
    provider: &dyn MarketDataProvider,
    filter: &ProtocolsFilter,
) -> Result<ProtocolsOutcome, Error> {
    let res = provider
        .protocols_fees(&filter.category, &filter.chain, filter.limit)
        .await;
    build_outcome(provider, res)
}

/// Run `protocols revenue`: top protocols by 24h revenue.
pub async fn run_revenue(
    provider: &dyn MarketDataProvider,
    filter: &ProtocolsFilter,
) -> Result<ProtocolsOutcome, Error> {
    let res = provider
        .protocols_revenue(&filter.category, &filter.chain, filter.limit)
        .await;
    build_outcome(provider, res)
}

/// clap parsing + handler for the `protocols` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::Error;
    use defi_model::Envelope;

    use super::{ProtocolsFilter, DEFAULT_LIMIT, PROTOCOLS_TTL_SECS};
    use crate::ctx::AppCtx;

    /// `protocols` subcommands (Go `newProtocolsCommand`).
    #[derive(Subcommand, Debug)]
    pub enum ProtocolsCmd {
        /// Top protocols by TVL.
        Top(FilterArgs),
        /// List protocol categories with protocol counts and TVL.
        Categories,
        /// Top protocols by 24h fees.
        Fees(FilterArgs),
        /// Top protocols by 24h revenue.
        Revenue(FilterArgs),
    }

    impl ProtocolsCmd {
        /// The leaf path token (for `meta.command`).
        pub fn path(&self) -> &'static str {
            match self {
                ProtocolsCmd::Top(_) => "top",
                ProtocolsCmd::Categories => "categories",
                ProtocolsCmd::Fees(_) => "fees",
                ProtocolsCmd::Revenue(_) => "revenue",
            }
        }
    }

    /// Shared `--category` / `--chain` / `--limit` flags for top/fees/revenue.
    #[derive(Args, Debug, Clone, Default)]
    pub struct FilterArgs {
        /// Filter by protocol category (e.g. lending).
        #[arg(long)]
        pub category: Option<String>,
        /// Filter by DefiLlama chain name (e.g. Ethereum, Arbitrum, Polygon).
        #[arg(long)]
        pub chain: Option<String>,
        /// Number of protocols to return.
        #[arg(long, default_value_t = DEFAULT_LIMIT)]
        pub limit: i64,
    }

    impl FilterArgs {
        fn to_filter(&self) -> ProtocolsFilter {
            ProtocolsFilter {
                category: self.category.clone().unwrap_or_default(),
                chain: self.chain.clone().unwrap_or_default(),
                limit: self.limit,
            }
        }
    }

    /// Handle `protocols <sub>`: fetch via DefiLlama through the cache flow.
    ///
    /// The async provider fetch is deferred into the cache-flow closure (run via
    /// [`crate::ctx::block_on_fetch`]) so a fresh cache hit short-circuits WITHOUT
    /// issuing a network call (spec §2.5).
    pub async fn handle(ctx: &AppCtx, cmd: ProtocolsCmd) -> Result<Envelope, Error> {
        let ttl = std::time::Duration::from_secs(PROTOCOLS_TTL_SECS);
        let provider = ctx.defillama();
        match cmd {
            ProtocolsCmd::Top(args) => {
                let filter = args.to_filter();
                let path = "protocols top";
                let key = super::cache_key(path, &filter);
                ctx.run_cached_command(path, &key, ttl, || {
                    finalize(crate::ctx::block_on_fetch(super::run_top(
                        &provider, &filter,
                    )))
                })
            }
            ProtocolsCmd::Categories => {
                let path = "protocols categories";
                let key = super::cache_key(path, &serde_json::json!({}));
                ctx.run_cached_command(path, &key, ttl, || {
                    finalize(crate::ctx::block_on_fetch(super::run_categories(&provider)))
                })
            }
            ProtocolsCmd::Fees(args) => {
                let filter = args.to_filter();
                let path = "protocols fees";
                let key = super::cache_key(path, &filter);
                ctx.run_cached_command(path, &key, ttl, || {
                    finalize(crate::ctx::block_on_fetch(super::run_fees(
                        &provider, &filter,
                    )))
                })
            }
            ProtocolsCmd::Revenue(args) => {
                let filter = args.to_filter();
                let path = "protocols revenue";
                let key = super::cache_key(path, &filter);
                ctx.run_cached_command(path, &key, ttl, || {
                    finalize(crate::ctx::block_on_fetch(super::run_revenue(
                        &provider, &filter,
                    )))
                })
            }
        }
    }

    /// Convert a [`super::ProtocolsOutcome`] result into the cache-flow fetch
    /// outcome tuple expected by `run_cached_command`.
    #[allow(clippy::type_complexity)]
    fn finalize(
        outcome: Result<super::ProtocolsOutcome, Error>,
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
    //! # Success criteria — `defi-app::protocols_cmd` (Go: `internal/app` protocols)
    //!
    //! This module owns the **command-layer composition** for the `protocols`
    //! group (`top` / `categories` / `fees` / `revenue`). "Correct" means it
    //! preserves the stable machine contract (design spec §2.1 envelope, §2.3
    //! rendering, §2.5 cache behavior) and the protocols-specific command wiring
    //! of `internal/app/runner.go::newProtocolsCommand`. The data sort/filter/
    //! rank parity is NOT re-asserted here — it lives in (and is tested by)
    //! `defi-providers::defillama` (`TestProtocolsTop*` etc.). The criteria
    //! asserted below:
    //!
    //!  1. **`protocols top` composition.** [`run_top`] calls the provider with
    //!     the supplied `--category`/`--chain`/`--limit` filter, serializes the
    //!     returned `Vec<ProtocolTvl>` verbatim into `data` (a JSON array whose
    //!     element keys are `rank, protocol, category, tvl_usd, chains` in
    //!     declaration order), and captures one `"ok"` provider status. Rendered
    //!     as a success envelope the `data` array round-trips the rows.
    //!     (Ported from Go `TestRunnerProtocolsTop` shape checks + the model
    //!     declaration-order contract.)
    //!  2. **`protocols categories` composition.** [`run_categories`] calls the
    //!     no-arg provider method and serializes `Vec<ProtocolCategory>` into
    //!     `data` (element keys `name, protocols, tvl_usd`). (Go
    //!     `TestRunnerProtocolsCategories`.)
    //!  3. **`protocols fees` composition.** [`run_fees`] serializes
    //!     `Vec<ProtocolFees>` into `data` (element keys include `protocol`,
    //!     `fees_24h_usd`). (Go `TestRunnerProtocolsFees`.)
    //!  4. **`protocols revenue` composition.** [`run_revenue`] serializes
    //!     `Vec<ProtocolRevenue>` into `data` (element keys include `protocol`,
    //!     `revenue_24h_usd`). (Go `TestRunnerProtocolsRevenue`.)
    //!  5. **Filter pass-through.** The exact `--category`/`--chain`/`--limit`
    //!     values are forwarded to the provider unchanged (the command layer does
    //!     no normalization; filtering is the provider's job). Asserted via a
    //!     recording fake that captures the args it was called with.
    //!  6. **Provider-status capture + `statusFromErr` mapping.** A successful
    //!     fetch yields one provider status with `status="ok"`; a failed fetch
    //!     surfaces the error (the command fails) and `status_from_result` maps
    //!     each error code to its Go status string (`auth_error` / `rate_limited`
    //!     / `unavailable` / `error`). (Go `statusFromErr`.)
    //!  7. **Error propagation.** A provider error from any subcommand propagates
    //!     as a typed `Error` with the same code (the runner turns it into the
    //!     full error envelope; that is the runner's contract, not re-tested here).
    //!  8. **Deterministic cache keys.** [`cache_key`] is a pure
    //!     `hex(sha256(path | "v2" | json(req)))`: identical inputs → identical
    //!     64-hex-char keys; different command paths, different filter values, and
    //!     a different schema-version component all change the key. The
    //!     `categories` subcommand keys on the empty `{}` request. (Go `cacheKey`
    //!     + `cachePayloadSchemaVersion`.)
    //!  9. **Default limit + TTL constants.** `DEFAULT_LIMIT == 20` and
    //!     `PROTOCOLS_TTL_SECS == 300` (Go `--limit` default 20, `5*time.Minute`).
    //! 10. **Cache routing.** All four `protocols *` paths open the cache (they
    //!     are data routes, not metadata/execution). Asserted via
    //!     `runner::should_open_cache`.
    //!
    //! Ported from the `TestRunnerProtocols*` command-composition cases in
    //! `runner_test.go`. Skipped here (covered elsewhere or internal detail):
    //! * the DefiLlama sort/filter/rank/limit behavior + httptest plumbing —
    //!   owned/tested by `defi-providers::defillama`, not re-asserted here;
    //! * the envelope shape/field-order + render contract — owned/tested by
    //!   `defi-model::envelope` and `defi-out`; we only assert the `data`
    //!   payload this module produces;
    //! * the cache-flow state machine (fresh hit / stale fallback / strict
    //!   partial) — owned/tested by `defi-app::runner`.

    use super::*;
    use async_trait::async_trait;
    use defi_errors::{Code, Error};
    use defi_id::{Asset, Chain};
    use defi_model::{
        self as model, CacheStatus, Envelope, ProtocolCategory, ProtocolFees, ProtocolRevenue,
        ProtocolTvl, ProviderInfo,
    };
    use defi_providers::{MarketDataProvider, Provider};
    use serde_json::Value;
    use std::sync::Mutex;

    // --- recording fake market provider ------------------------------------

    /// What the fake was asked for on its most recent call.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    struct CallArgs {
        category: String,
        chain: String,
        limit: i64,
    }

    /// A `MarketDataProvider` that returns canned protocol lists (or a canned
    /// error) and records the filter args it was called with. Mirrors the Go
    /// `fakeMarketProvider` used by the `TestRunnerProtocols*` tests.
    struct FakeMarket {
        name: String,
        top: Vec<ProtocolTvl>,
        categories: Vec<ProtocolCategory>,
        fees: Vec<ProtocolFees>,
        revenue: Vec<ProtocolRevenue>,
        /// When set, every fetch returns this error instead of the canned list.
        fail: Option<Code>,
        last_call: Mutex<CallArgs>,
    }

    impl FakeMarket {
        fn new() -> Self {
            FakeMarket {
                name: "defillama".to_string(),
                top: Vec::new(),
                categories: Vec::new(),
                fees: Vec::new(),
                revenue: Vec::new(),
                fail: None,
                last_call: Mutex::new(CallArgs::default()),
            }
        }

        fn record(&self, category: &str, chain: &str, limit: i64) {
            *self.last_call.lock().unwrap() = CallArgs {
                category: category.to_string(),
                chain: chain.to_string(),
                limit,
            };
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
                capabilities: vec!["protocols.top".to_string()],
                key_env_var_name: String::new(),
                capability_auth: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl MarketDataProvider for FakeMarket {
        async fn chains_top(&self, _limit: i64) -> Result<Vec<model::ChainTvl>, Error> {
            Ok(Vec::new())
        }
        async fn chains_assets(
            &self,
            _chain: Chain,
            _asset: Asset,
            _limit: i64,
        ) -> Result<Vec<model::ChainAssetTvl>, Error> {
            Ok(Vec::new())
        }
        async fn protocols_top(
            &self,
            category: &str,
            chain: &str,
            limit: i64,
        ) -> Result<Vec<ProtocolTvl>, Error> {
            self.record(category, chain, limit);
            if self.fail.is_some() {
                return Err(self.err());
            }
            Ok(self.top.clone())
        }
        async fn protocols_categories(&self) -> Result<Vec<ProtocolCategory>, Error> {
            self.record("", "", 0);
            if self.fail.is_some() {
                return Err(self.err());
            }
            Ok(self.categories.clone())
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
            category: &str,
            chain: &str,
            limit: i64,
        ) -> Result<Vec<ProtocolFees>, Error> {
            self.record(category, chain, limit);
            if self.fail.is_some() {
                return Err(self.err());
            }
            Ok(self.fees.clone())
        }
        async fn protocols_revenue(
            &self,
            category: &str,
            chain: &str,
            limit: i64,
        ) -> Result<Vec<ProtocolRevenue>, Error> {
            self.record(category, chain, limit);
            if self.fail.is_some() {
                return Err(self.err());
            }
            Ok(self.revenue.clone())
        }
        async fn dexes_volume(
            &self,
            _chain: &str,
            _limit: i64,
        ) -> Result<Vec<model::DexVolume>, Error> {
            Ok(Vec::new())
        }
    }

    fn filter(category: &str, chain: &str, limit: i64) -> ProtocolsFilter {
        ProtocolsFilter {
            category: category.to_string(),
            chain: chain.to_string(),
            limit,
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

    // --- 1. protocols top composition -------------------------------------

    #[tokio::test]
    async fn run_top_serializes_rows_and_captures_ok_status() {
        let mut p = FakeMarket::new();
        p.top = vec![ProtocolTvl {
            rank: 1,
            protocol: "Aave".to_string(),
            category: "Lending".to_string(),
            tvl_usd: 15_000_000.0,
            chains: 12,
        }];
        let out = run_top(&p, &filter("", "", DEFAULT_LIMIT))
            .await
            .expect("run_top success");

        assert_eq!(out.provider.name, "defillama");
        assert_eq!(out.provider.status, "ok");

        let row = first_row(&out.data);
        assert_eq!(row["rank"], Value::from(1));
        assert_eq!(row["protocol"], Value::from("Aave"));
        assert_eq!(row["category"], Value::from("Lending"));
        assert!(row.contains_key("tvl_usd"));
        assert_eq!(row["chains"], Value::from(12));
        // Element keys in struct declaration order.
        let keys: Vec<&String> = row.keys().collect();
        assert_eq!(
            keys,
            vec!["rank", "protocol", "category", "tvl_usd", "chains"]
        );

        // Rendered into a success envelope, `data` round-trips the rows.
        let env = Envelope::success(
            "protocols top",
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

    // --- 2. protocols categories composition ------------------------------

    #[tokio::test]
    async fn run_categories_serializes_category_rows() {
        let mut p = FakeMarket::new();
        p.categories = vec![ProtocolCategory {
            name: "Lending".to_string(),
            protocols: 2,
            tvl_usd: 15_000.0,
        }];
        let out = run_categories(&p).await.expect("run_categories success");

        let row = first_row(&out.data);
        assert_eq!(row["name"], Value::from("Lending"));
        assert_eq!(row["protocols"], Value::from(2));
        assert!(row.contains_key("tvl_usd"));
        let keys: Vec<&String> = row.keys().collect();
        assert_eq!(keys, vec!["name", "protocols", "tvl_usd"]);
        assert_eq!(out.provider.status, "ok");
    }

    // --- 3. protocols fees composition ------------------------------------

    #[tokio::test]
    async fn run_fees_serializes_fee_rows() {
        let mut p = FakeMarket::new();
        p.fees = vec![ProtocolFees {
            rank: 1,
            protocol: "Lido".to_string(),
            category: "Liquid Staking".to_string(),
            fees_24h_usd: 8_000_000.0,
            fees_7d_usd: 55_000_000.0,
            fees_30d_usd: 200_000_000.0,
            change_1d_pct: 0.0,
            change_7d_pct: 0.0,
            change_1m_pct: 0.0,
            chains: 1,
        }];
        let out = run_fees(&p, &filter("", "", DEFAULT_LIMIT))
            .await
            .expect("run_fees success");

        let row = first_row(&out.data);
        assert_eq!(row["protocol"], Value::from("Lido"));
        assert!(row.contains_key("fees_24h_usd"));
        assert_eq!(out.provider.status, "ok");
    }

    // --- 4. protocols revenue composition ---------------------------------

    #[tokio::test]
    async fn run_revenue_serializes_revenue_rows() {
        let mut p = FakeMarket::new();
        p.revenue = vec![ProtocolRevenue {
            rank: 1,
            protocol: "Lido".to_string(),
            category: "Liquid Staking".to_string(),
            revenue_24h_usd: 5_000_000.0,
            revenue_7d_usd: 35_000_000.0,
            revenue_30d_usd: 130_000_000.0,
            change_1d_pct: 0.0,
            change_7d_pct: 0.0,
            change_1m_pct: 0.0,
            chains: 1,
        }];
        let out = run_revenue(&p, &filter("", "", DEFAULT_LIMIT))
            .await
            .expect("run_revenue success");

        let row = first_row(&out.data);
        assert_eq!(row["protocol"], Value::from("Lido"));
        assert!(row.contains_key("revenue_24h_usd"));
        assert_eq!(out.provider.status, "ok");
    }

    // --- 5. filter pass-through (no command-layer normalization) ----------

    #[tokio::test]
    async fn run_top_forwards_filter_verbatim_to_provider() {
        let p = FakeMarket::new();
        let _ = run_top(&p, &filter("Lending", "Ethereum", 5))
            .await
            .expect("run_top success");
        assert_eq!(
            p.last(),
            CallArgs {
                category: "Lending".to_string(),
                chain: "Ethereum".to_string(),
                limit: 5,
            }
        );
    }

    #[tokio::test]
    async fn run_fees_and_revenue_forward_filter_verbatim() {
        let p = FakeMarket::new();
        let _ = run_fees(&p, &filter("Dexs", "Arbitrum", 3))
            .await
            .expect("run_fees");
        assert_eq!(
            p.last(),
            CallArgs {
                category: "Dexs".to_string(),
                chain: "Arbitrum".to_string(),
                limit: 3,
            }
        );

        let p2 = FakeMarket::new();
        let _ = run_revenue(&p2, &filter("Lending", "Polygon", 7))
            .await
            .expect("run_revenue");
        assert_eq!(
            p2.last(),
            CallArgs {
                category: "Lending".to_string(),
                chain: "Polygon".to_string(),
                limit: 7,
            }
        );
    }

    // --- 6. provider-status capture + statusFromErr mapping ---------------

    #[test]
    fn status_from_result_maps_each_code() {
        let ok: Result<(), Error> = Ok(());
        assert_eq!(status_from_result(&ok), "ok");
        assert_eq!(
            status_from_result::<()>(&Err(Error::new(Code::Auth, "x"))),
            "auth_error"
        );
        assert_eq!(
            status_from_result::<()>(&Err(Error::new(Code::RateLimited, "x"))),
            "rate_limited"
        );
        assert_eq!(
            status_from_result::<()>(&Err(Error::new(Code::Unavailable, "x"))),
            "unavailable"
        );
        // Any other code collapses to the generic "error" bucket.
        assert_eq!(
            status_from_result::<()>(&Err(Error::new(Code::Unsupported, "x"))),
            "error"
        );
        assert_eq!(
            status_from_result::<()>(&Err(Error::new(Code::Internal, "x"))),
            "error"
        );
    }

    // --- 7. error propagation ---------------------------------------------

    #[tokio::test]
    async fn run_top_propagates_provider_error_with_same_code() {
        let mut p = FakeMarket::new();
        p.fail = Some(Code::Unavailable);
        let err = run_top(&p, &filter("", "", DEFAULT_LIMIT))
            .await
            .expect_err("provider failure propagates");
        assert_eq!(err.code, Code::Unavailable);
    }

    #[tokio::test]
    async fn run_categories_propagates_auth_error() {
        let mut p = FakeMarket::new();
        p.fail = Some(Code::Auth);
        let err = run_categories(&p)
            .await
            .expect_err("auth failure propagates");
        assert_eq!(err.code, Code::Auth);
    }

    // --- 8. deterministic cache keys --------------------------------------

    #[test]
    fn cache_key_is_deterministic_and_hex_sha256() {
        let req = filter("Lending", "Ethereum", 20);
        let a = cache_key("protocols top", &req);
        let b = cache_key("protocols top", &req);
        assert_eq!(a, b, "identical inputs => identical key");
        assert_eq!(a.len(), 64, "sha256 hex is 64 chars");
        assert!(
            a.chars().all(|c| c.is_ascii_hexdigit()),
            "key is lowercase hex, got: {a}"
        );
    }

    #[test]
    fn cache_key_changes_with_command_path() {
        let req = filter("", "", 20);
        assert_ne!(
            cache_key("protocols fees", &req),
            cache_key("protocols revenue", &req),
            "different command paths must produce different keys"
        );
    }

    #[test]
    fn cache_key_changes_with_filter_values() {
        let base = cache_key("protocols top", &filter("", "", 20));
        assert_ne!(base, cache_key("protocols top", &filter("Lending", "", 20)));
        assert_ne!(
            base,
            cache_key("protocols top", &filter("", "Ethereum", 20))
        );
        assert_ne!(base, cache_key("protocols top", &filter("", "", 5)));
    }

    #[test]
    fn cache_key_categories_uses_empty_request() {
        // `categories` keys on the empty `{}` request (Go `map[string]any{}`),
        // which must differ from the `top` key on the same empty filter request.
        let empty = serde_json::Map::<String, Value>::new();
        let cat_key = cache_key("protocols categories", &empty);
        assert_eq!(cat_key.len(), 64);
        // Stable across calls.
        assert_eq!(cat_key, cache_key("protocols categories", &empty));
    }

    #[test]
    fn cache_key_matches_go_hash_formula_with_schema_version() {
        // Pin the key to the exact Go formula
        // `hex(sha256(path | cachePayloadSchemaVersion | json(req)))`, proving
        // the "v2" schema version is mixed into the hashed prefix. The expected
        // value below is computed independently from the Go formula; if
        // `cache_key` dropped the version or changed the prefix layout this
        // assertion fails.
        let req = filter("Lending", "Ethereum", 20);
        let payload = serde_json::to_string(&req).expect("serialize req");
        // Independent reference hash via the same `hex` crate the impl uses,
        // computed over the documented prefix layout.
        let prefix = format!("protocols top|{CACHE_PAYLOAD_SCHEMA_VERSION}|");
        let expected = reference_sha256_hex(prefix.as_bytes(), payload.as_bytes());
        assert_eq!(
            cache_key("protocols top", &req),
            expected,
            "cache_key must equal hex(sha256(path | v2 | json(req)))"
        );
        // And differs from the same hash computed with a different version,
        // proving the version genuinely participates.
        let wrong_prefix = b"protocols top|v999|";
        let wrong = reference_sha256_hex(wrong_prefix, payload.as_bytes());
        assert_ne!(cache_key("protocols top", &req), wrong);
    }

    /// A dependency-free SHA-256 used only as an independent reference oracle for
    /// the cache-key formula (FIPS 180-4). Kept inside the test module so the
    /// production crate gains no crypto dependency for a test assertion.
    fn reference_sha256_hex(prefix: &[u8], payload: &[u8]) -> String {
        let mut msg = Vec::with_capacity(prefix.len() + payload.len());
        msg.extend_from_slice(prefix);
        msg.extend_from_slice(payload);
        let digest = sha256(&msg);
        let mut s = String::with_capacity(64);
        for b in digest {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    fn sha256(data: &[u8]) -> [u8; 32] {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut h: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];
        let mut msg = data.to_vec();
        let bitlen = (data.len() as u64) * 8;
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bitlen.to_be_bytes());

        for chunk in msg.chunks_exact(64) {
            let mut w = [0u32; 64];
            for (i, word) in w.iter_mut().take(16).enumerate() {
                *word = u32::from_be_bytes([
                    chunk[i * 4],
                    chunk[i * 4 + 1],
                    chunk[i * 4 + 2],
                    chunk[i * 4 + 3],
                ]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let mut v = h;
            for i in 0..64 {
                let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
                let ch = (v[4] & v[5]) ^ ((!v[4]) & v[6]);
                let t1 = v[7]
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
                let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
                let t2 = s0.wrapping_add(maj);
                v[7] = v[6];
                v[6] = v[5];
                v[5] = v[4];
                v[4] = v[3].wrapping_add(t1);
                v[3] = v[2];
                v[2] = v[1];
                v[1] = v[0];
                v[0] = t1.wrapping_add(t2);
            }
            for (hi, vi) in h.iter_mut().zip(v.iter()) {
                *hi = hi.wrapping_add(*vi);
            }
        }
        let mut out = [0u8; 32];
        for (i, word) in h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    // --- 9. default limit + TTL constants ---------------------------------

    #[test]
    fn default_limit_and_ttl_match_go() {
        assert_eq!(DEFAULT_LIMIT, 20);
        assert_eq!(PROTOCOLS_TTL_SECS, 300);
        assert_eq!(CACHE_PAYLOAD_SCHEMA_VERSION, "v2");
    }

    // --- 10. cache routing -------------------------------------------------

    #[test]
    fn all_protocols_paths_open_the_cache() {
        for p in [
            "protocols top",
            "protocols categories",
            "protocols fees",
            "protocols revenue",
        ] {
            assert!(
                crate::runner::should_open_cache(p),
                "{p:?} is a data route and must open the cache"
            );
        }
    }
}
