//! `stablecoins` command group handler.
//!
//! Mirrors the `stablecoins` subtree of
//! `internal/app/runner.go::newStablecoinsCommand` (the `top` / `chains`
//! subcommands). This module owns the **command-layer composition** for the
//! stablecoins group; the lower-level pieces are owned elsewhere and reused:
//!
//! * the data fetch + peg filter / circulating-total / sort / rank parity
//!   (descending-by-circulating ordering, peg-type filtering, dominant-peg
//!   selection, zero-supply skipping, `rank` assignment, `limit` capping): the
//!   `MarketDataProvider` impl in [`defi_providers::defillama`] — already
//!   contract-tested there (`TestStablecoinsTop*`, `TestStablecoinChains*`);
//! * the success/error envelope + cache-flow state machine: the runner
//!   (`defi_app::runner`);
//! * cache-bypass routing: the runner (`defi_app::runner::should_open_cache`);
//! * the deterministic cache-key formula: [`crate::protocols::cache_key`]
//!   (`hex(sha256(path | schema-version | json(req)))`).
//!
//! What this module owns (the contract-bearing command composition):
//!
//! 1. **Request shaping per subcommand.** `top` takes `--peg-type` + `--limit`
//!    (default 20); `chains` takes `--limit` (default 20). The request struct
//!    serialized into the cache key must mirror the Go `map[string]any` payloads
//!    (`{"peg_type","limit"}` and `{"limit"}`), which `encoding/json` emits with
//!    **alphabetically sorted** keys — so `top` keys as `{"limit","peg_type"}`.
//!    Field declaration order here is chosen to reproduce that JSON exactly so
//!    cache keys stay byte-stable against the Go binary.
//! 2. **Provider-status capture.** Each fetch yields exactly one
//!    `model::ProviderStatus` for the market provider, whose `status` string is
//!    derived from the fetch result via the Go `statusFromErr` mapping
//!    (ok / auth_error / rate_limited / unavailable / error).
//! 3. **Success-payload shape.** The fetched list is serialized verbatim into
//!    `data` (a JSON array), the command path is `stablecoins <sub>`, and the
//!    5-minute TTL is used.
//! 4. **Cache routing.** Both `stablecoins *` paths open the cache (they are NOT
//!    metadata/execution routes).
//!
//! Idiomatic-Rust shape note: the Go command closures write to injected
//! `io.Writer`s and return `error`. The Rust port exposes async builder functions
//! returning values (a `StablecoinsOutcome` carrying the JSON `data` payload + the
//! captured `ProviderStatus`) so they can be unit-tested without a `cobra.Command`;
//! the envelope construction + rendering is layered on top by the runner.

#![allow(dead_code)]

use defi_errors::{Code, Error};
use defi_model::ProviderStatus;
use defi_providers::{MarketDataProvider, Provider};
use serde::Serialize;
use serde_json::Value;

/// The cache TTL for every `stablecoins *` subcommand (Go: `5 * time.Minute`).
pub const STABLECOINS_TTL_SECS: u64 = 300;

/// The default `--limit` for `stablecoins top`/`chains` (Go default 20).
pub const DEFAULT_LIMIT: i64 = 20;

/// Request payload for `stablecoins top`.
///
/// Mirrors the Go request `map[string]any{"peg_type", "limit"}`. Go's
/// `encoding/json` serializes map keys **alphabetically**, so the on-the-wire
/// JSON is `{"limit":N,"peg_type":"..."}`. Field declaration order here matches
/// that ordering so the canonical-JSON fed into the cache key is byte-identical
/// to the Go binary's.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StablecoinsTopRequest {
    /// `--limit` (number of rows; `<= 0` = all).
    pub limit: i64,
    /// `--peg-type` (e.g. `peggedUSD`, `peggedEUR`; empty = no filter).
    pub peg_type: String,
}

/// Request payload for `stablecoins chains`.
///
/// Mirrors the Go request `map[string]any{"limit"}`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StablecoinChainsRequest {
    /// `--limit` (number of rows; `<= 0` = all).
    pub limit: i64,
}

/// A resolved stablecoins-subcommand fetch.
///
/// Carries the JSON `data` payload (the serialized provider list) and the single
/// captured market-provider [`ProviderStatus`]. The runner layers envelope
/// construction + rendering on top.
#[derive(Debug, Clone)]
pub struct StablecoinsOutcome {
    /// The fetched list, serialized verbatim as a JSON array for `data`.
    pub data: Value,
    /// The single market-provider status captured for this fetch.
    pub provider: ProviderStatus,
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
        .map_err(|e| Error::wrap(Code::Internal, "serialize stablecoins rows", e))
}

/// Shared fetch→outcome composition for both subcommands.
///
/// Captures provider status from the result, propagates any provider error,
/// and otherwise serializes the rows into `data`.
fn build_outcome<T: Serialize>(
    provider: &dyn MarketDataProvider,
    res: Result<Vec<T>, Error>,
) -> Result<StablecoinsOutcome, Error> {
    let status = provider_status(provider, &res);
    let rows = res?;
    Ok(StablecoinsOutcome {
        data: rows_to_data(&rows)?,
        provider: status,
    })
}

/// Run `stablecoins top`: top stablecoins by circulating market cap.
///
/// Calls [`MarketDataProvider::stablecoins_top`] with the `--peg-type`/`--limit`
/// request, serializes the resulting `Vec<Stablecoin>` into `data`, and captures
/// the provider status.
pub async fn run_top(
    provider: &dyn MarketDataProvider,
    req: &StablecoinsTopRequest,
) -> Result<StablecoinsOutcome, Error> {
    let res = provider.stablecoins_top(&req.peg_type, req.limit).await;
    build_outcome(provider, res)
}

/// Run `stablecoins chains`: chains ranked by total stablecoin market cap.
///
/// Calls [`MarketDataProvider::stablecoin_chains`] with the `--limit` request,
/// serializes the resulting `Vec<StablecoinChain>` into `data`, and captures the
/// provider status.
pub async fn run_chains(
    provider: &dyn MarketDataProvider,
    req: &StablecoinChainsRequest,
) -> Result<StablecoinsOutcome, Error> {
    let res = provider.stablecoin_chains(req.limit).await;
    build_outcome(provider, res)
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::stablecoins_cmd` (Go: `internal/app` stablecoins)
    //!
    //! This module owns the **command-layer composition** for the `stablecoins`
    //! group (`top` / `chains`), i.e. the wiring in
    //! `internal/app/runner.go::newStablecoinsCommand`. "Correct" means it
    //! preserves the stable machine contract (design spec §2.1 envelope, §2.3
    //! rendering, §2.5 cache behavior) and the stablecoins-specific command
    //! wiring. The data peg-filter / circulating-total / sort / rank parity is
    //! NOT re-asserted here — it lives in (and is tested by)
    //! `defi-providers::defillama` (`TestStablecoinsTop*`, `TestStablecoinChains*`).
    //! The criteria asserted below:
    //!
    //!  1. **`stablecoins top` composition.** [`run_top`] calls the provider with
    //!     the supplied `--peg-type`/`--limit` request, serializes the returned
    //!     `Vec<Stablecoin>` verbatim into `data` (a JSON array whose element keys
    //!     are `rank, name, symbol, peg_type, peg_mechanism, circulating_usd,
    //!     price, chains, day_change_usd, week_change_usd, month_change_usd` in
    //!     struct DECLARATION order), and captures one `"ok"` provider status.
    //!     Rendered as a success envelope the `data` array round-trips the rows.
    //!     (Spec §2.3 declaration-order contract; Go `model.Stablecoin`.)
    //!  2. **`stablecoins chains` composition.** [`run_chains`] calls the
    //!     `--limit` provider method and serializes `Vec<StablecoinChain>` into
    //!     `data` (element keys `rank, chain, chain_id, circulating_usd,
    //!     dominant_peg_type` in declaration order). (Go `model.StablecoinChain`.)
    //!  3. **Request pass-through.** The exact `--peg-type`/`--limit` values are
    //!     forwarded to the provider unchanged (the command layer does no
    //!     normalization; filtering/sorting is the provider's job). Asserted via a
    //!     recording fake that captures the args it was called with.
    //!  4. **Provider-status capture + `statusFromErr` mapping.** A successful
    //!     fetch yields one provider status with `status="ok"`; a failed fetch
    //!     surfaces the error (the command fails) and `status_from_result` maps
    //!     each error code to its Go status string (`auth_error` / `rate_limited`
    //!     / `unavailable` / `error`). (Go `statusFromErr`.)
    //!  5. **Error propagation.** A provider error from either subcommand
    //!     propagates as a typed `Error` with the same code (the runner turns it
    //!     into the full error envelope; that is the runner's contract, not
    //!     re-tested here).
    //!  6. **Deterministic, Go-parity cache keys.** The cache key (shared
    //!     [`crate::protocols::cache_key`]) is a pure
    //!     `hex(sha256(path | "v2" | json(req)))`. Because Go keys on a
    //!     `map[string]any` whose JSON has **alphabetically sorted** keys, the
    //!     `top` request must serialize as `{"limit":N,"peg_type":"..."}` and the
    //!     `chains` request as `{"limit":N}`. Identical inputs → identical
    //!     64-hex-char keys; different command paths and different request values
    //!     all change the key; an independent SHA-256 reference oracle pins the
    //!     exact formula (incl. the `v2` schema-version component) for both
    //!     subcommands' request shapes.
    //!  7. **Empty-result payload.** When the provider returns an empty list, the
    //!     `data` payload is an empty JSON array `[]` (not null), still with an
    //!     `"ok"` provider status. (Contract: lists serialize as arrays.)
    //!  8. **Default limit + TTL constants.** `DEFAULT_LIMIT == 20` and
    //!     `STABLECOINS_TTL_SECS == 300` (Go `--limit` default 20, `5*time.Minute`).
    //!  9. **Cache routing.** Both `stablecoins *` paths open the cache (they are
    //!     data routes, not metadata/execution). Asserted via
    //!     `runner::should_open_cache`.
    //!
    //! Skipped here (covered elsewhere or internal detail):
    //! * the DefiLlama peg-filter / sort / rank / limit / dominant-peg behavior +
    //!   httptest plumbing — owned/tested by `defi-providers::defillama`, not
    //!   re-asserted here;
    //! * the envelope shape/field-order + render contract — owned/tested by
    //!   `defi-model::envelope` and `defi-out`; we only assert the `data` payload
    //!   this module produces;
    //! * the cache-flow state machine (fresh hit / stale fallback / strict
    //!   partial) — owned/tested by `defi-app::runner`.

    use super::*;
    use crate::protocols::{cache_key, CACHE_PAYLOAD_SCHEMA_VERSION};
    use async_trait::async_trait;
    use defi_errors::{Code, Error};
    use defi_id::{Asset, Chain};
    use defi_model::{
        self as model, CacheStatus, Envelope, ProviderInfo, Stablecoin, StablecoinChain,
    };
    use defi_providers::{MarketDataProvider, Provider};
    use serde_json::Value;
    use std::sync::Mutex;

    // --- recording fake market provider ------------------------------------

    /// What the fake was asked for on its most recent call.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    struct CallArgs {
        peg_type: String,
        limit: i64,
    }

    /// A `MarketDataProvider` that returns canned stablecoin lists (or a canned
    /// error) and records the request args it was called with. Mirrors the Go
    /// `fakeMarketProvider` used by the runner tests.
    struct FakeMarket {
        name: String,
        top: Vec<Stablecoin>,
        chains: Vec<StablecoinChain>,
        /// When set, every fetch returns this error instead of the canned list.
        fail: Option<Code>,
        last_call: Mutex<CallArgs>,
    }

    impl FakeMarket {
        fn new() -> Self {
            FakeMarket {
                name: "defillama".to_string(),
                top: Vec::new(),
                chains: Vec::new(),
                fail: None,
                last_call: Mutex::new(CallArgs::default()),
            }
        }

        fn record(&self, peg_type: &str, limit: i64) {
            *self.last_call.lock().unwrap() = CallArgs {
                peg_type: peg_type.to_string(),
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
                capabilities: vec!["stablecoins.top".to_string()],
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
            peg_type: &str,
            limit: i64,
        ) -> Result<Vec<Stablecoin>, Error> {
            self.record(peg_type, limit);
            if self.fail.is_some() {
                return Err(self.err());
            }
            Ok(self.top.clone())
        }
        async fn stablecoin_chains(&self, limit: i64) -> Result<Vec<StablecoinChain>, Error> {
            self.record("", limit);
            if self.fail.is_some() {
                return Err(self.err());
            }
            Ok(self.chains.clone())
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

    fn top_req(peg_type: &str, limit: i64) -> StablecoinsTopRequest {
        StablecoinsTopRequest {
            limit,
            peg_type: peg_type.to_string(),
        }
    }

    fn chains_req(limit: i64) -> StablecoinChainsRequest {
        StablecoinChainsRequest { limit }
    }

    fn sample_stablecoin() -> Stablecoin {
        Stablecoin {
            rank: 1,
            name: "Tether".to_string(),
            symbol: "USDT".to_string(),
            peg_type: "peggedUSD".to_string(),
            peg_mechanism: "fiat-backed".to_string(),
            circulating_usd: 100_000_000_000.0,
            price: 1.0,
            chains: 14,
            day_change_usd: 1_000_000.0,
            week_change_usd: 5_000_000.0,
            month_change_usd: 20_000_000.0,
        }
    }

    fn sample_chain() -> StablecoinChain {
        StablecoinChain {
            rank: 1,
            chain: "Ethereum".to_string(),
            chain_id: "eip155:1".to_string(),
            circulating_usd: 80_000_000_000.0,
            dominant_peg_type: "peggedUSD".to_string(),
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

    // --- 1. stablecoins top composition -----------------------------------

    #[tokio::test]
    async fn run_top_serializes_rows_in_declaration_order_and_captures_ok_status() {
        let mut p = FakeMarket::new();
        p.top = vec![sample_stablecoin()];
        let out = run_top(&p, &top_req("", DEFAULT_LIMIT))
            .await
            .expect("run_top success");

        assert_eq!(out.provider.name, "defillama");
        assert_eq!(out.provider.status, "ok");

        let row = first_row(&out.data);
        assert_eq!(row["rank"], Value::from(1));
        assert_eq!(row["name"], Value::from("Tether"));
        assert_eq!(row["symbol"], Value::from("USDT"));
        assert_eq!(row["peg_type"], Value::from("peggedUSD"));
        assert_eq!(row["peg_mechanism"], Value::from("fiat-backed"));
        assert!(row.contains_key("circulating_usd"));
        assert!(row.contains_key("price"));
        assert_eq!(row["chains"], Value::from(14));
        assert!(row.contains_key("day_change_usd"));
        assert!(row.contains_key("week_change_usd"));
        assert!(row.contains_key("month_change_usd"));
        // Element keys in struct DECLARATION order (machine contract §2.3).
        let keys: Vec<&String> = row.keys().collect();
        assert_eq!(
            keys,
            vec![
                "rank",
                "name",
                "symbol",
                "peg_type",
                "peg_mechanism",
                "circulating_usd",
                "price",
                "chains",
                "day_change_usd",
                "week_change_usd",
                "month_change_usd",
            ]
        );

        // Rendered into a success envelope, `data` round-trips the rows.
        let env = Envelope::success(
            "stablecoins top",
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

    // --- 2. stablecoins chains composition --------------------------------

    #[tokio::test]
    async fn run_chains_serializes_chain_rows_in_declaration_order() {
        let mut p = FakeMarket::new();
        p.chains = vec![sample_chain()];
        let out = run_chains(&p, &chains_req(DEFAULT_LIMIT))
            .await
            .expect("run_chains success");

        let row = first_row(&out.data);
        assert_eq!(row["rank"], Value::from(1));
        assert_eq!(row["chain"], Value::from("Ethereum"));
        assert_eq!(row["chain_id"], Value::from("eip155:1"));
        assert!(row.contains_key("circulating_usd"));
        assert_eq!(row["dominant_peg_type"], Value::from("peggedUSD"));
        let keys: Vec<&String> = row.keys().collect();
        assert_eq!(
            keys,
            vec![
                "rank",
                "chain",
                "chain_id",
                "circulating_usd",
                "dominant_peg_type",
            ]
        );
        assert_eq!(out.provider.status, "ok");
    }

    // --- 3. request pass-through (no command-layer normalization) ---------

    #[tokio::test]
    async fn run_top_forwards_peg_type_and_limit_verbatim() {
        let p = FakeMarket::new();
        let _ = run_top(&p, &top_req("peggedEUR", 5))
            .await
            .expect("run_top success");
        assert_eq!(
            p.last(),
            CallArgs {
                peg_type: "peggedEUR".to_string(),
                limit: 5,
            }
        );
    }

    #[tokio::test]
    async fn run_chains_forwards_limit_verbatim() {
        let p = FakeMarket::new();
        let _ = run_chains(&p, &chains_req(3))
            .await
            .expect("run_chains success");
        assert_eq!(
            p.last(),
            CallArgs {
                peg_type: String::new(),
                limit: 3,
            }
        );
    }

    // --- 4. provider-status capture + statusFromErr mapping ---------------

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

    // --- 5. error propagation ---------------------------------------------

    #[tokio::test]
    async fn run_top_propagates_provider_error_with_same_code() {
        let mut p = FakeMarket::new();
        p.fail = Some(Code::Unavailable);
        let err = run_top(&p, &top_req("", DEFAULT_LIMIT))
            .await
            .expect_err("provider failure propagates");
        assert_eq!(err.code, Code::Unavailable);
    }

    #[tokio::test]
    async fn run_chains_propagates_rate_limited_error() {
        let mut p = FakeMarket::new();
        p.fail = Some(Code::RateLimited);
        let err = run_chains(&p, &chains_req(DEFAULT_LIMIT))
            .await
            .expect_err("rate-limit failure propagates");
        assert_eq!(err.code, Code::RateLimited);
    }

    // --- 6. deterministic, Go-parity cache keys ---------------------------

    #[test]
    fn cache_key_is_deterministic_and_hex_sha256() {
        let req = top_req("peggedUSD", 20);
        let a = cache_key("stablecoins top", &req);
        let b = cache_key("stablecoins top", &req);
        assert_eq!(a, b, "identical inputs => identical key");
        assert_eq!(a.len(), 64, "sha256 hex is 64 chars");
        assert!(
            a.chars().all(|c| c.is_ascii_hexdigit()),
            "key is lowercase hex, got: {a}"
        );
    }

    #[test]
    fn cache_key_changes_with_command_path_and_request_values() {
        let top = cache_key("stablecoins top", &top_req("", 20));
        let chains = cache_key("stablecoins chains", &chains_req(20));
        assert_ne!(top, chains, "different command paths => different keys");

        let base = cache_key("stablecoins top", &top_req("", 20));
        assert_ne!(
            base,
            cache_key("stablecoins top", &top_req("peggedEUR", 20)),
            "peg-type participates in the key"
        );
        assert_ne!(
            base,
            cache_key("stablecoins top", &top_req("", 5)),
            "limit participates in the key"
        );

        let chains_base = cache_key("stablecoins chains", &chains_req(20));
        assert_ne!(
            chains_base,
            cache_key("stablecoins chains", &chains_req(5)),
            "limit participates in the chains key"
        );
    }

    #[test]
    fn top_request_serializes_with_go_alphabetical_map_key_order() {
        // Go keys on `map[string]any{"peg_type","limit"}`, whose `json.Marshal`
        // emits keys ALPHABETICALLY: `{"limit":N,"peg_type":"..."}`. For
        // byte-stable cache parity the Rust request must serialize identically.
        let json = serde_json::to_string(&top_req("peggedUSD", 20)).expect("serialize");
        assert_eq!(json, r#"{"limit":20,"peg_type":"peggedUSD"}"#);
    }

    #[test]
    fn chains_request_serializes_as_single_limit_key() {
        let json = serde_json::to_string(&chains_req(15)).expect("serialize");
        assert_eq!(json, r#"{"limit":15}"#);
    }

    #[test]
    fn cache_key_matches_go_hash_formula_for_top_and_chains() {
        // Pin both subcommand keys to the exact Go formula
        // `hex(sha256(path | cachePayloadSchemaVersion | json(req)))`, where
        // `json(req)` is the alphabetical-map JSON the Go binary produces.
        let top_payload = r#"{"limit":20,"peg_type":"peggedUSD"}"#;
        let top_prefix = format!("stablecoins top|{CACHE_PAYLOAD_SCHEMA_VERSION}|");
        let expected_top = reference_sha256_hex(top_prefix.as_bytes(), top_payload.as_bytes());
        assert_eq!(
            cache_key("stablecoins top", &top_req("peggedUSD", 20)),
            expected_top,
            "top cache_key must equal hex(sha256(path | v2 | json(req)))"
        );

        let chains_payload = r#"{"limit":15}"#;
        let chains_prefix = format!("stablecoins chains|{CACHE_PAYLOAD_SCHEMA_VERSION}|");
        let expected_chains =
            reference_sha256_hex(chains_prefix.as_bytes(), chains_payload.as_bytes());
        assert_eq!(
            cache_key("stablecoins chains", &chains_req(15)),
            expected_chains,
            "chains cache_key must equal hex(sha256(path | v2 | json(req)))"
        );

        // Proving the schema version genuinely participates: a different version
        // yields a different key.
        let wrong = reference_sha256_hex(b"stablecoins top|v999|", top_payload.as_bytes());
        assert_ne!(
            cache_key("stablecoins top", &top_req("peggedUSD", 20)),
            wrong
        );
    }

    // --- 7. empty-result payload ------------------------------------------

    #[tokio::test]
    async fn run_top_empty_result_serializes_as_empty_array() {
        let p = FakeMarket::new(); // no rows
        let out = run_top(&p, &top_req("", DEFAULT_LIMIT))
            .await
            .expect("run_top success");
        assert_eq!(out.data, Value::Array(Vec::new()));
        assert_eq!(out.provider.status, "ok");
    }

    #[tokio::test]
    async fn run_chains_empty_result_serializes_as_empty_array() {
        let p = FakeMarket::new(); // no rows
        let out = run_chains(&p, &chains_req(DEFAULT_LIMIT))
            .await
            .expect("run_chains success");
        assert_eq!(out.data, Value::Array(Vec::new()));
        assert_eq!(out.provider.status, "ok");
    }

    // --- 8. default limit + TTL constants ---------------------------------

    #[test]
    fn default_limit_and_ttl_match_go() {
        assert_eq!(DEFAULT_LIMIT, 20);
        assert_eq!(STABLECOINS_TTL_SECS, 300);
    }

    // --- 9. cache routing -------------------------------------------------

    #[test]
    fn both_stablecoins_paths_open_the_cache() {
        for p in ["stablecoins top", "stablecoins chains"] {
            assert!(
                crate::runner::should_open_cache(p),
                "{p:?} is a data route and must open the cache"
            );
        }
    }

    // --- independent SHA-256 reference oracle (test-only) ------------------

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
}
