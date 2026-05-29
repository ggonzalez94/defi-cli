//! `dexes` command group handler.
//!
//! Mirrors the `dexes` subtree of `internal/app/runner.go::newDexesCommand` (the
//! single `volume` subcommand). This module owns the **command-layer
//! composition** for the dexes group; the lower-level pieces are owned elsewhere
//! and reused:
//!
//! * the data fetch + filter / sort / rank parity (positive-24h filtering,
//!   optional chain-presence filtering, descending-by-24h ordering, `rank`
//!   assignment, `limit` capping, null/zero/negative skipping): the
//!   `MarketDataProvider::dexes_volume` impl in [`defi_providers::defillama`] —
//!   already contract-tested there (`TestDexesVolumeSortsAndLimits`,
//!   `TestDexesVolumeFiltersByChain`, `TestDexesVolumeSkipsNullAndZero`);
//! * the success/error envelope + cache-flow state machine: the runner
//!   (`defi_app::runner`);
//! * cache-bypass routing: the runner (`defi_app::runner::should_open_cache`);
//! * the deterministic cache-key formula: [`crate::protocols::cache_key`]
//!   (`hex(sha256(path | schema-version | json(req)))`).
//!
//! What this module owns (the contract-bearing command composition):
//!
//! 1. **Request shaping.** `volume` takes `--chain` (DefiLlama chain name filter;
//!    empty = all) + `--limit` (default 20). The request struct serialized into
//!    the cache key must mirror the Go `map[string]any{"chain", "limit"}` payload,
//!    which `encoding/json` emits with **alphabetically sorted** keys — so the
//!    JSON is `{"chain":"...","limit":N}`. Field declaration order here is chosen
//!    to reproduce that JSON exactly so cache keys stay byte-stable against the Go
//!    binary.
//! 2. **Provider-status capture.** The fetch yields exactly one
//!    `model::ProviderStatus` for the market provider, whose `status` string is
//!    derived from the fetch result via the Go `statusFromErr` mapping
//!    (ok / auth_error / rate_limited / unavailable / error).
//! 3. **Success-payload shape.** The fetched list is serialized verbatim into
//!    `data` (a JSON array), the command path is `dexes volume`, and the 5-minute
//!    TTL is used.
//! 4. **Cache routing.** The `dexes volume` path opens the cache (it is NOT a
//!    metadata/execution route).
//!
//! Idiomatic-Rust shape note: the Go command closure writes to injected
//! `io.Writer`s and returns `error`. The Rust port exposes an async builder
//! function returning a value (a `DexesOutcome` carrying the JSON `data` payload +
//! the captured `ProviderStatus`) so it can be unit-tested without a
//! `cobra.Command`; the envelope construction + rendering is layered on top by the
//! runner.

#![allow(dead_code)]

use defi_errors::{Code, Error};
use defi_model::ProviderStatus;
use defi_providers::{MarketDataProvider, Provider};
use serde_json::Value;

/// The cache TTL for the `dexes volume` subcommand (Go: `5 * time.Minute`).
pub const DEXES_TTL_SECS: u64 = 300;

/// The default `--limit` for `dexes volume` (Go default 20).
pub const DEFAULT_LIMIT: i64 = 20;

/// Request payload for `dexes volume`.
///
/// Mirrors the Go request `map[string]any{"chain", "limit"}`. Go's
/// `encoding/json` serializes map keys **alphabetically**, so the on-the-wire
/// JSON is `{"chain":"...","limit":N}`. Field declaration order here matches that
/// ordering so the canonical-JSON fed into the cache key is byte-identical to the
/// Go binary's.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DexesVolumeRequest {
    /// `--chain` (DefiLlama chain name filter, e.g. `Ethereum`; empty = all).
    pub chain: String,
    /// `--limit` (number of rows; `<= 0` = all).
    pub limit: i64,
}

/// A resolved dexes-subcommand fetch.
///
/// Carries the JSON `data` payload (the serialized provider list) and the single
/// captured market-provider [`ProviderStatus`]. The runner layers envelope
/// construction + rendering on top.
#[derive(Debug, Clone)]
pub struct DexesOutcome {
    /// The fetched list, serialized verbatim as a JSON array for `data`.
    pub data: Value,
    /// The single market-provider status captured for this fetch.
    pub provider: ProviderStatus,
}

/// Map a fetch result to the Go `statusFromErr` provider-status string:
/// `Ok` → `"ok"`; `Auth` → `"auth_error"`; `RateLimited` → `"rate_limited"`;
/// `Unavailable` → `"unavailable"`; anything else → `"error"`.
///
/// Shared with the rest of the command layer (Go `statusFromErr`); delegates to
/// the single implementation in [`crate::protocols::status_from_result`] so the
/// mapping stays in one place.
pub fn status_from_result<T>(res: &Result<T, Error>) -> String {
    crate::protocols::status_from_result(res)
}

/// Run `dexes volume`: top DEXes by 24h trading volume.
///
/// Calls [`MarketDataProvider::dexes_volume`] with the `--chain`/`--limit`
/// request, serializes the resulting `Vec<DexVolume>` into `data`, and captures
/// the provider status. The command layer does no normalization — the
/// chain/limit filtering, sorting, and ranking are the provider's job.
pub async fn run_volume(
    provider: &dyn MarketDataProvider,
    req: &DexesVolumeRequest,
) -> Result<DexesOutcome, Error> {
    let res = provider.dexes_volume(&req.chain, req.limit).await;

    // Capture provider status from the result before `?` consumes it (Go
    // closure captures `model.ProviderStatus{Name, Status: statusFromErr}`).
    // Latency timing is owned by the runner's cache-flow state machine, so the
    // command layer leaves `latency_ms` at zero.
    let status = ProviderStatus {
        name: provider.info().name,
        status: status_from_result(&res),
        latency_ms: 0,
    };

    let rows = res?;
    let data = serde_json::to_value(&rows)
        .map_err(|e| Error::wrap(Code::Internal, "serialize dexes rows", e))?;

    Ok(DexesOutcome {
        data,
        provider: status,
    })
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::dexes_cmd` (Go: `internal/app` dexes)
    //!
    //! This module owns the **command-layer composition** for the `dexes` group
    //! (the single `volume` subcommand), i.e. the wiring in
    //! `internal/app/runner.go::newDexesCommand`. "Correct" means it preserves the
    //! stable machine contract (design spec §2.1 envelope, §2.3 rendering, §2.5
    //! cache behavior) and the dexes-specific command wiring. The data
    //! filter/sort/rank/limit parity is NOT re-asserted here — it lives in (and is
    //! tested by) `defi-providers::defillama` (`TestDexesVolumeSortsAndLimits`,
    //! `TestDexesVolumeFiltersByChain`, `TestDexesVolumeSkipsNullAndZero`). The
    //! criteria asserted below:
    //!
    //!  1. **`dexes volume` composition.** [`run_volume`] calls the provider with
    //!     the supplied `--chain`/`--limit` request, serializes the returned
    //!     `Vec<DexVolume>` verbatim into `data` (a JSON array whose element keys
    //!     are `rank, protocol, volume_24h_usd, volume_7d_usd, volume_30d_usd,
    //!     change_1d_pct, change_7d_pct, change_1m_pct, chains` in struct
    //!     DECLARATION order — machine contract §2.3), and captures one `"ok"`
    //!     provider status. Rendered as a success envelope the `data` array
    //!     round-trips the rows.
    //!  2. **Request pass-through.** The exact `--chain`/`--limit` values are
    //!     forwarded to the provider unchanged (the command layer does no
    //!     normalization; the filtering/sorting is the provider's job). Asserted
    //!     via a recording fake that captures the args it was called with.
    //!  3. **Provider-status capture + `statusFromErr` mapping.** A successful
    //!     fetch yields one provider status named after the market provider with
    //!     `status="ok"`; a failed fetch surfaces the error (the command fails) and
    //!     `status_from_result` maps each error code to its Go status string
    //!     (`auth_error` / `rate_limited` / `unavailable` / `error`). (Go
    //!     `statusFromErr`.)
    //!  4. **Error propagation.** A provider error propagates as a typed `Error`
    //!     with the same code (the runner turns it into the full error envelope;
    //!     that is the runner's contract, not re-tested here).
    //!  5. **Deterministic, Go-parity cache keys.** The cache key (shared
    //!     [`crate::protocols::cache_key`]) is a pure
    //!     `hex(sha256(path | "v2" | json(req)))`. Because Go keys on a
    //!     `map[string]any{"chain","limit"}` whose JSON has **alphabetically
    //!     sorted** keys, the request must serialize as `{"chain":"...","limit":N}`.
    //!     Identical inputs → identical 64-hex-char keys; the `--chain` and
    //!     `--limit` values each participate in the key; an independent SHA-256
    //!     reference oracle pins the exact formula (incl. the `v2` schema-version
    //!     component).
    //!  6. **Empty-result payload.** When the provider returns an empty list, the
    //!     `data` payload is an empty JSON array `[]` (not null), still with an
    //!     `"ok"` provider status. (Contract: lists serialize as arrays.)
    //!  7. **Default limit + TTL constants.** `DEFAULT_LIMIT == 20` and
    //!     `DEXES_TTL_SECS == 300` (Go `--limit` default 20, `5*time.Minute`).
    //!  8. **Cache routing.** The `dexes volume` path opens the cache (it is a data
    //!     route, not metadata/execution). Asserted via
    //!     `runner::should_open_cache`.
    //!
    //! Ported from the `dexes volume` wiring in `runner.go::newDexesCommand` (no
    //! dedicated app-level Go test exists beyond the `fakeMarketProvider` stub; the
    //! provider-level behavior is covered by the DefiLlama `TestDexesVolume*`
    //! cases). Skipped here (covered elsewhere or internal detail):
    //! * the DefiLlama filter / sort / rank / limit / null-skip behavior + httptest
    //!   plumbing — owned/tested by `defi-providers::defillama`, not re-asserted
    //!   here;
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
    use defi_model::{self as model, CacheStatus, DexVolume, Envelope, ProviderInfo};
    use defi_providers::{MarketDataProvider, Provider};
    use serde_json::Value;
    use std::sync::Mutex;

    // --- recording fake market provider ------------------------------------

    /// What the fake was asked for on its most recent call.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    struct CallArgs {
        chain: String,
        limit: i64,
    }

    /// A `MarketDataProvider` that returns a canned dex-volume list (or a canned
    /// error) and records the request args it was called with. Mirrors the Go
    /// `fakeMarketProvider` used by the runner tests.
    struct FakeMarket {
        name: String,
        volume: Vec<DexVolume>,
        /// When set, every fetch returns this error instead of the canned list.
        fail: Option<Code>,
        last_call: Mutex<CallArgs>,
    }

    impl FakeMarket {
        fn new() -> Self {
            FakeMarket {
                name: "defillama".to_string(),
                volume: Vec::new(),
                fail: None,
                last_call: Mutex::new(CallArgs::default()),
            }
        }

        fn record(&self, chain: &str, limit: i64) {
            *self.last_call.lock().unwrap() = CallArgs {
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
                capabilities: vec!["dexes.volume".to_string()],
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
        async fn dexes_volume(&self, chain: &str, limit: i64) -> Result<Vec<DexVolume>, Error> {
            self.record(chain, limit);
            if self.fail.is_some() {
                return Err(self.err());
            }
            Ok(self.volume.clone())
        }
    }

    fn req(chain: &str, limit: i64) -> DexesVolumeRequest {
        DexesVolumeRequest {
            chain: chain.to_string(),
            limit,
        }
    }

    fn sample_dex() -> DexVolume {
        DexVolume {
            rank: 1,
            protocol: "Uniswap".to_string(),
            volume_24h_usd: 5_000_000.0,
            volume_7d_usd: 30_000_000.0,
            volume_30d_usd: 120_000_000.0,
            change_1d_pct: 5.2,
            change_7d_pct: -2.1,
            change_1m_pct: 10.5,
            chains: 3,
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

    // --- 1. dexes volume composition --------------------------------------

    #[tokio::test]
    async fn run_volume_serializes_rows_in_declaration_order_and_captures_ok_status() {
        let mut p = FakeMarket::new();
        p.volume = vec![sample_dex()];
        let out = run_volume(&p, &req("", DEFAULT_LIMIT))
            .await
            .expect("run_volume success");

        assert_eq!(out.provider.name, "defillama");
        assert_eq!(out.provider.status, "ok");

        let row = first_row(&out.data);
        assert_eq!(row["rank"], Value::from(1));
        assert_eq!(row["protocol"], Value::from("Uniswap"));
        assert!(row.contains_key("volume_24h_usd"));
        assert!(row.contains_key("volume_7d_usd"));
        assert!(row.contains_key("volume_30d_usd"));
        assert!(row.contains_key("change_1d_pct"));
        assert!(row.contains_key("change_7d_pct"));
        assert!(row.contains_key("change_1m_pct"));
        assert_eq!(row["chains"], Value::from(3));
        // Element keys in struct DECLARATION order (machine contract §2.3).
        let keys: Vec<&String> = row.keys().collect();
        assert_eq!(
            keys,
            vec![
                "rank",
                "protocol",
                "volume_24h_usd",
                "volume_7d_usd",
                "volume_30d_usd",
                "change_1d_pct",
                "change_7d_pct",
                "change_1m_pct",
                "chains",
            ]
        );

        // Rendered into a success envelope, `data` round-trips the rows.
        let env = Envelope::success(
            "dexes volume",
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

    // --- 2. request pass-through (no command-layer normalization) ---------

    #[tokio::test]
    async fn run_volume_forwards_chain_and_limit_verbatim() {
        let p = FakeMarket::new();
        let _ = run_volume(&p, &req("Ethereum", 5))
            .await
            .expect("run_volume success");
        assert_eq!(
            p.last(),
            CallArgs {
                chain: "Ethereum".to_string(),
                limit: 5,
            }
        );
    }

    #[tokio::test]
    async fn run_volume_forwards_empty_chain_and_default_limit() {
        let p = FakeMarket::new();
        let _ = run_volume(&p, &req("", DEFAULT_LIMIT))
            .await
            .expect("run_volume success");
        assert_eq!(
            p.last(),
            CallArgs {
                chain: String::new(),
                limit: DEFAULT_LIMIT,
            }
        );
    }

    // --- 3. provider-status capture + statusFromErr mapping ---------------

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

    // --- 4. error propagation ---------------------------------------------

    #[tokio::test]
    async fn run_volume_propagates_provider_error_with_same_code() {
        let mut p = FakeMarket::new();
        p.fail = Some(Code::Unavailable);
        let err = run_volume(&p, &req("", DEFAULT_LIMIT))
            .await
            .expect_err("provider failure propagates");
        assert_eq!(err.code, Code::Unavailable);
    }

    #[tokio::test]
    async fn run_volume_propagates_rate_limited_error() {
        let mut p = FakeMarket::new();
        p.fail = Some(Code::RateLimited);
        let err = run_volume(&p, &req("Ethereum", DEFAULT_LIMIT))
            .await
            .expect_err("rate-limit failure propagates");
        assert_eq!(err.code, Code::RateLimited);
    }

    // --- 5. deterministic, Go-parity cache keys ---------------------------

    #[test]
    fn cache_key_is_deterministic_and_hex_sha256() {
        let r = req("Ethereum", 20);
        let a = cache_key("dexes volume", &r);
        let b = cache_key("dexes volume", &r);
        assert_eq!(a, b, "identical inputs => identical key");
        assert_eq!(a.len(), 64, "sha256 hex is 64 chars");
        assert!(
            a.chars().all(|c| c.is_ascii_hexdigit()),
            "key is lowercase hex, got: {a}"
        );
    }

    #[test]
    fn cache_key_changes_with_request_values() {
        let base = cache_key("dexes volume", &req("", 20));
        assert_ne!(
            base,
            cache_key("dexes volume", &req("Ethereum", 20)),
            "chain participates in the key"
        );
        assert_ne!(
            base,
            cache_key("dexes volume", &req("", 5)),
            "limit participates in the key"
        );
    }

    #[test]
    fn request_serializes_with_go_alphabetical_map_key_order() {
        // Go keys on `map[string]any{"chain","limit"}`, whose `json.Marshal` emits
        // keys ALPHABETICALLY: `{"chain":"...","limit":N}`. For byte-stable cache
        // parity the Rust request must serialize identically.
        let json = serde_json::to_string(&req("Ethereum", 20)).expect("serialize");
        assert_eq!(json, r#"{"chain":"Ethereum","limit":20}"#);
    }

    #[test]
    fn cache_key_matches_go_hash_formula_with_schema_version() {
        // Pin the key to the exact Go formula
        // `hex(sha256(path | cachePayloadSchemaVersion | json(req)))`, where
        // `json(req)` is the alphabetical-map JSON the Go binary produces.
        let payload = r#"{"chain":"Ethereum","limit":20}"#;
        let prefix = format!("dexes volume|{CACHE_PAYLOAD_SCHEMA_VERSION}|");
        let expected = reference_sha256_hex(prefix.as_bytes(), payload.as_bytes());
        assert_eq!(
            cache_key("dexes volume", &req("Ethereum", 20)),
            expected,
            "cache_key must equal hex(sha256(path | v2 | json(req)))"
        );

        // Proving the schema version genuinely participates: a different version
        // yields a different key.
        let wrong = reference_sha256_hex(b"dexes volume|v999|", payload.as_bytes());
        assert_ne!(cache_key("dexes volume", &req("Ethereum", 20)), wrong);
    }

    // --- 6. empty-result payload ------------------------------------------

    #[tokio::test]
    async fn run_volume_empty_result_serializes_as_empty_array() {
        let p = FakeMarket::new(); // no rows
        let out = run_volume(&p, &req("", DEFAULT_LIMIT))
            .await
            .expect("run_volume success");
        assert_eq!(out.data, Value::Array(Vec::new()));
        assert_eq!(out.provider.status, "ok");
    }

    // --- 7. default limit + TTL constants ---------------------------------

    #[test]
    fn default_limit_and_ttl_match_go() {
        assert_eq!(DEFAULT_LIMIT, 20);
        assert_eq!(DEXES_TTL_SECS, 300);
    }

    // --- 8. cache routing -------------------------------------------------

    #[test]
    fn dexes_volume_path_opens_the_cache() {
        assert!(
            crate::runner::should_open_cache("dexes volume"),
            "\"dexes volume\" is a data route and must open the cache"
        );
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
