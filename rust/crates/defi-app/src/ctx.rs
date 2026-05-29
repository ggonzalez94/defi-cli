//! Shared application context + handler contract (WS0 foundation).
//!
//! This module defines the plumbing that every command-group handler is written
//! against. It is the Rust analogue of the per-command `*Runner` state the Go
//! `internal/app/runner.go` threads into each `cobra.Command` closure: the
//! resolved [`Settings`], the lazily-constructed provider clients (each exposing
//! the base-URL / `--rpc-url` seam already present on every adapter), an optional
//! [`defi_cache::store::Store`], an optional [`defi_execution::store::Store`],
//! and a `now()`/request-id determinism seam.
//!
//! ## Locked contracts
//!
//! These names are the shared source of truth for every later workstream — they
//! MUST NOT be renamed without updating all group handlers:
//!
//! * [`AppCtx`] — the per-invocation context.
//! * The handler signature: an async fn `(ctx: &AppCtx, args: <GroupArgs>) ->
//!   Result<Envelope, Error>`. Read-command handlers route their fetch through
//!   `ctx.run_cached_command(...)`; metadata + execution handlers build the
//!   [`Envelope`] directly (cache bypassed, spec §2.5).
//! * [`AppCtx::now`] — the injectable clock (UTC).
//! * [`AppCtx::request_id`] — the per-process request-id generator (32 hex).
//!
//! ## Cache routing
//!
//! [`AppCtx::open_cache_for`] opens the sqlite cache iff the command path is a
//! data command ([`crate::runner::should_open_cache`]) AND caching is enabled.
//! Metadata (`version`/`schema`/`providers`/`chains list`/`chains gas`) and
//! execution (`*plan|submit|status`, `actions *`) paths return `None`, matching
//! the Go runner's `shouldOpenCache` bypass.

#![allow(dead_code)]

use std::time::Duration;

use chrono::{DateTime, Utc};
use defi_cache::store::Store as CacheStore;
use defi_config::Settings;
use defi_errors::{Code, Error};
use defi_execution::store::Store as ActionStore;
use defi_httpx::Client as HttpClient;
use defi_model::{CacheStatus, Envelope, ProviderStatus};
use defi_providers::defillama;

use crate::runner::{should_open_cache, FetchOutcome, Runtime};

/// Per-invocation application context shared by every command-group handler.
///
/// Construction is cheap: the provider clients and cache/action stores are
/// produced on demand through the accessor methods (each of which honors the
/// `--rpc-url` / base-URL seams), so metadata commands never pay for a cache or
/// a network client they do not use.
pub struct AppCtx {
    /// Resolved settings (`flags > env > file > defaults`).
    pub settings: Settings,
    /// Injectable clock; defaults to [`Utc::now`].
    pub clock: fn() -> DateTime<Utc>,
    /// Test/offline seam: override the DefiLlama free-endpoint base URL
    /// (`api_base`) so app-level wiremock tests can point the wired handlers at a
    /// mock server. `None` (the production default) uses the public endpoint.
    ///
    /// Applied by [`AppCtx::defillama`] via [`defillama::Client::set_api_base`]
    /// (and `set_bridge_base_url`) when set, so app-level wiremock tests for
    /// `protocols`/`stablecoins`/`dexes` reach the mock rather than the public API.
    pub defillama_api_base: Option<String>,
    /// Test/offline seam: override the DefiLlama stablecoins base URL
    /// (`stablecoins_api_url`). See [`AppCtx::defillama_api_base`]. `None` uses
    /// the public endpoint.
    pub defillama_stablecoins_base: Option<String>,
    /// Test/offline seam: override the HTTP-API swap-quote provider base URL
    /// (applied via each adapter's `set_base_url`, e.g.
    /// [`defi_providers::oneinch::Client::set_base_url`]) so app-level wiremock
    /// tests for `swap quote` reach a mock server. `None` (production) uses the
    /// public endpoints. Analogous to [`AppCtx::defillama_api_base`].
    pub swap_quote_base: Option<String>,
    /// Test/offline seam: override the HTTP-API bridge-quote provider base URL
    /// (applied via each adapter's `set_base_url`, e.g.
    /// [`defi_providers::across::Client::set_base_url`]) so app-level wiremock
    /// tests for `bridge quote` reach a mock server. `None` (production) uses the
    /// public endpoints. Analogous to [`AppCtx::swap_quote_base`].
    ///
    /// Note: the `bridge list` / `bridge details` analytics providers are
    /// DefiLlama-backed and reuse [`AppCtx::defillama_api_base`] (which already
    /// applies `set_bridge_base_url`); this seam only retargets the cross-chain
    /// quote providers (Across / LiFi / Bungee).
    pub bridge_quote_base: Option<String>,
}

impl AppCtx {
    /// Build a context from resolved [`Settings`] using the real wall clock.
    pub fn new(settings: Settings) -> AppCtx {
        AppCtx {
            settings,
            clock: Utc::now,
            defillama_api_base: None,
            defillama_stablecoins_base: None,
            swap_quote_base: None,
            bridge_quote_base: None,
        }
    }

    /// Retarget the HTTP-API swap-quote provider clients at a single mock-server
    /// origin (the offline/wiremock seam used by app-level `swap quote` tests).
    ///
    /// The `swap quote` handler (WS2) MUST honor this when constructing its swap
    /// providers (applying it via each adapter's `set_base_url`), analogous to
    /// how [`AppCtx::defillama`] honors [`AppCtx::defillama_api_base`].
    pub fn with_swap_base(mut self, base: &str) -> AppCtx {
        self.swap_quote_base = Some(base.to_string());
        self
    }

    /// Retarget the HTTP-API bridge-quote provider clients (Across / LiFi /
    /// Bungee) at a single mock-server origin (the offline/wiremock seam used by
    /// app-level `bridge quote` tests).
    ///
    /// The `bridge quote` handler (WS2) MUST honor this when constructing its
    /// bridge quote providers (applying it via each adapter's `set_base_url`),
    /// analogous to how [`AppCtx::with_swap_base`] feeds the swap providers. The
    /// `bridge list`/`bridge details` analytics path reuses
    /// [`AppCtx::with_defillama_base`] instead.
    pub fn with_bridge_base(mut self, base: &str) -> AppCtx {
        self.bridge_quote_base = Some(base.to_string());
        self
    }

    /// Retarget the DefiLlama base URLs (free `api_base` + stablecoins) at a
    /// single mock-server origin (the offline/wiremock seam used by app-level
    /// market-data tests). Both overrides are set to `base`.
    pub fn with_defillama_base(mut self, base: &str) -> AppCtx {
        self.defillama_api_base = Some(base.to_string());
        self.defillama_stablecoins_base = Some(base.to_string());
        self
    }

    /// The current time from the injected clock.
    pub fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    /// Generate a 128-bit hex request id (mirrors the SHAPE of Go
    /// `newRequestID`: 32 lowercase hex chars). The golden tests normalize this
    /// to a sentinel, so only the shape is contract-relevant.
    pub fn request_id(&self) -> String {
        use sha2::{Digest, Sha256};
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

        let mut hasher = Sha256::new();
        hasher.update(nanos.to_le_bytes());
        hasher.update(seq.to_le_bytes());
        let digest = hasher.finalize();
        hex::encode(&digest[..16])
    }

    /// A shared HTTP client honoring the resolved request timeout + retries.
    ///
    /// Mirrors the Go runner's `httpx.NewClient(timeout, retries)` construction
    /// used for every provider adapter.
    pub fn http_client(&self) -> HttpClient {
        let retries = self.settings.retries.max(0) as u32;
        HttpClient::new(self.settings.timeout, retries)
    }

    /// Construct a DefiLlama market-data client (base URLs default to the public
    /// endpoints; the API key comes from settings — empty when unset).
    ///
    /// When the test/offline base-URL seams ([`AppCtx::defillama_api_base`] /
    /// [`AppCtx::defillama_stablecoins_base`]) are set (via
    /// [`AppCtx::with_defillama_base`]), the corresponding `set_api_base` /
    /// `set_stablecoins_api_url` override is applied so app-level wiremock tests
    /// point the wired handlers at a mock server. In production both are `None`
    /// and the public endpoints are used.
    pub fn defillama(&self) -> defillama::Client {
        let mut client =
            defillama::Client::new(self.http_client(), &self.settings.defillama_api_key);
        if let Some(base) = &self.defillama_api_base {
            client.set_api_base(base);
            client.set_bridge_base_url(base);
        }
        if let Some(base) = &self.defillama_stablecoins_base {
            client.set_stablecoins_api_url(base);
        }
        client
    }

    /// The set of registered swap quote provider names (canonical/normalized),
    /// in the Go registration order (`runner.go` `s.swapProviders`).
    ///
    /// Mirrors the Go runner's `s.swapProviders` keys (`1inch`, `uniswap`,
    /// `tempo`, `taikoswap`, `jupiter`, `bungee`, `fibrous`). Used by the
    /// `swap quote` guard to reject unknown providers with
    /// [`defi_errors::Code::Unsupported`] before any chain/asset parse.
    pub fn swap_provider_names(&self) -> &'static [&'static str] {
        &[
            "1inch",
            "uniswap",
            "tempo",
            "taikoswap",
            "jupiter",
            "bungee",
            "fibrous",
        ]
    }

    /// Construct the swap quote provider adapter for a (normalized) provider
    /// name, applying the offline/wiremock base-URL seam ([`AppCtx::swap_quote_base`])
    /// to the HTTP-API providers (1inch/uniswap/jupiter/bungee/fibrous). Returns
    /// `None` for an unregistered provider name (the caller maps that to a typed
    /// [`defi_errors::Code::Unsupported`] error).
    ///
    /// Mirrors the Go runner's `s.swapProviders[providerName]` lookup; the
    /// adapters are constructed lazily here (per invocation) the same way the Go
    /// runner builds them in `withRuntimeState`. Tempo/TaikoSwap are RPC-only and
    /// carry no HTTP base-URL seam.
    pub fn swap_provider(&self, name: &str) -> Option<Box<dyn defi_providers::SwapProvider>> {
        use defi_providers::{bungee, fibrous, jupiter, oneinch, taikoswap, tempo, uniswap};

        let base = self.swap_quote_base.as_deref();
        match name {
            "1inch" => {
                let mut c =
                    oneinch::Client::new(self.http_client(), &self.settings.oneinch_api_key);
                if let Some(b) = base {
                    c.set_base_url(b);
                }
                Some(Box::new(c))
            }
            "uniswap" => {
                let mut c =
                    uniswap::Client::new(self.http_client(), &self.settings.uniswap_api_key);
                if let Some(b) = base {
                    c.set_base_url(b);
                }
                Some(Box::new(c))
            }
            "jupiter" => {
                let mut c =
                    jupiter::Client::new(self.http_client(), &self.settings.jupiter_api_key);
                if let Some(b) = base {
                    c.set_base_url(b);
                }
                Some(Box::new(c))
            }
            "bungee" => {
                let mut c = bungee::Client::new_swap(
                    self.http_client(),
                    &self.settings.bungee_api_key,
                    &self.settings.bungee_affiliate,
                );
                if let Some(b) = base {
                    c.set_base_url(b);
                }
                Some(Box::new(c))
            }
            "fibrous" => {
                let mut c = fibrous::Client::new(self.http_client());
                if let Some(b) = base {
                    c.set_base_url(b);
                }
                Some(Box::new(c))
            }
            "tempo" => Some(Box::new(tempo::Client::new())),
            "taikoswap" => Some(Box::new(taikoswap::Client::new())),
            _ => None,
        }
    }

    /// The set of registered bridge quote provider names (canonical/normalized),
    /// in the Go registration order (`runner.go` `s.bridgeProviders`).
    ///
    /// Mirrors the Go runner's `s.bridgeProviders` keys (`across`, `lifi`,
    /// `bungee`). Used by the `bridge quote` guard to reject unknown providers
    /// with [`defi_errors::Code::Unsupported`] (after the empty-`--provider`
    /// usage guard).
    pub fn bridge_provider_names(&self) -> &'static [&'static str] {
        &["across", "lifi", "bungee"]
    }

    /// Construct the bridge quote provider adapter for a (normalized) provider
    /// name, applying the offline/wiremock base-URL seam
    /// ([`AppCtx::bridge_quote_base`]) via each adapter's `set_base_url`. Returns
    /// `None` for an unregistered provider name (the caller maps that to a typed
    /// [`defi_errors::Code::Unsupported`] error).
    ///
    /// Mirrors the Go runner's `s.bridgeProviders[providerName]` lookup; the
    /// adapters are constructed lazily here (per invocation) the same way the Go
    /// runner builds them in `withRuntimeState`.
    pub fn bridge_provider(&self, name: &str) -> Option<Box<dyn defi_providers::BridgeProvider>> {
        use defi_providers::{across, bungee, lifi};

        let base = self.bridge_quote_base.as_deref();
        match name {
            "across" => {
                let mut c = across::Client::new(self.http_client());
                if let Some(b) = base {
                    c.set_base_url(b);
                }
                Some(Box::new(c))
            }
            "lifi" => {
                let mut c = lifi::Client::new(self.http_client());
                if let Some(b) = base {
                    c.set_base_url(b);
                }
                Some(Box::new(c))
            }
            "bungee" => {
                let mut c = bungee::Client::new_bridge(
                    self.http_client(),
                    &self.settings.bungee_api_key,
                    &self.settings.bungee_affiliate,
                );
                if let Some(b) = base {
                    c.set_base_url(b);
                }
                Some(Box::new(c))
            }
            _ => None,
        }
    }

    /// Build the action-build routing [`Registry`] populated with the swap
    /// execution providers (Go `actionbuilder.New(s.swapProviders, ...)`).
    ///
    /// The execution-capable swap providers — `taikoswap` and `tempo` — are
    /// registered as builders keyed on their `Info().Name` (so a captured
    /// `ProviderStatus` matches Go, lowercase). The remaining registered swap
    /// quote providers (`1inch`, `uniswap`, `jupiter`, `bungee`, `fibrous`) are
    /// marked known-but-quote-only so `build_swap_action` routes them to the Go
    /// `provider X does not support swap planning` error (rather than the
    /// unknown-provider error). The bridge builders are not populated here (the
    /// swap-plan path does not need them).
    ///
    /// [`Registry`]: defi_execution::builder::Registry
    pub fn swap_action_registry(&self) -> defi_execution::builder::Registry {
        use defi_providers::{taikoswap, tempo};

        let mut reg = defi_execution::builder::Registry::new();
        reg.register_swap_builder_named(
            "taikoswap",
            &taikoswap::Client::new().info().name,
            Box::new(taikoswap::Client::new()),
        );
        reg.register_swap_builder_named(
            "tempo",
            &tempo::Client::new().info().name,
            Box::new(tempo::Client::new()),
        );
        // Known-but-quote-only swap providers (no execution builder) — Go marks
        // these as registered swap providers without a `SwapExecutionProvider`
        // implementation, so planning them is "does not support swap planning".
        for name in ["1inch", "uniswap", "jupiter", "bungee", "fibrous"] {
            reg.register_known_swap_provider(name);
        }
        reg
    }

    /// Build the action-build routing [`Registry`] populated with the bridge
    /// execution providers (Go `actionbuilder.New(..., s.bridgeProviders)`).
    ///
    /// The execution-capable bridge providers — `across` and `lifi` — are
    /// registered as builders keyed on their `Info().Name` (lowercase, matching
    /// the captured Go `ProviderStatus`). `bungee` is a registered bridge *quote*
    /// provider with no execution builder (Go: it does not implement the
    /// `BridgeExecutionProvider` interface), so it is marked known-but-quote-only
    /// — planning it routes to the Go quote-only error rather than the
    /// unknown-provider error. The offline/wiremock base-URL seam
    /// ([`AppCtx::bridge_quote_base`]) is applied to the Across/LiFi adapters via
    /// each adapter's `set_base_url` so app-level wiremock tests reach a mock
    /// server.
    ///
    /// [`Registry`]: defi_execution::builder::Registry
    pub fn bridge_action_registry(&self) -> defi_execution::builder::Registry {
        use defi_providers::{across, lifi, Provider};

        let base = self.bridge_quote_base.as_deref();
        let mut reg = defi_execution::builder::Registry::new();

        let mut across_client = across::Client::new(self.http_client());
        if let Some(b) = base {
            across_client.set_base_url(b);
        }
        let across_name = across_client.info().name;
        reg.register_bridge_builder("across", &across_name, Box::new(across_client));

        let mut lifi_client = lifi::Client::new(self.http_client());
        if let Some(b) = base {
            lifi_client.set_base_url(b);
        }
        let lifi_name = lifi_client.info().name;
        reg.register_bridge_builder("lifi", &lifi_name, Box::new(lifi_client));

        // Known-but-quote-only bridge provider (registered quote provider with no
        // `BridgeExecutionProvider` implementation in Go).
        reg.register_known_bridge_provider("bungee");
        reg
    }

    /// Open the sqlite cache store for `command_path`, or `None` when the path
    /// bypasses the cache (metadata/execution) or caching is disabled.
    ///
    /// Mirrors the Go runner's `shouldOpenCache` gate (spec §2.5): metadata and
    /// execution commands never initialize the cache.
    pub fn open_cache_for(&self, command_path: &str) -> Option<CacheStore> {
        if !self.settings.cache_enabled {
            return None;
        }
        if !should_open_cache(command_path) {
            return None;
        }
        CacheStore::open(
            &self.settings.cache_path,
            &self.settings.cache_lock_path,
            self.settings.max_stale,
        )
        .ok()
    }

    /// Open the persisted execution action store (used by `plan`/`submit`/
    /// `status` + `actions *`). Errors surface as a typed [`Error`] because the
    /// store is mandatory for execution commands.
    pub fn open_action_store(&self) -> Result<ActionStore, Error> {
        ActionStore::open(
            &self.settings.action_store_path,
            &self.settings.action_lock_path,
        )
    }

    /// Run a cache-backed read command through the runner's cache-flow state
    /// machine and return the resolved success [`Envelope`].
    ///
    /// This is the single entry point read-command handlers use: it opens the
    /// cache for `command_path` (bypassing for metadata/execution paths), then
    /// drives [`Runtime::run_cached_command`] with `fetch`. On error the typed
    /// [`Error`] is returned for the caller to render as a full error envelope.
    pub fn run_cached_command<F>(
        &self,
        command_path: &str,
        key: &str,
        ttl: Duration,
        fetch: F,
    ) -> Result<Envelope, Error>
    where
        F: FnOnce() -> Result<FetchOutcome, (Vec<ProviderStatus>, Vec<String>, bool, Error)>,
    {
        let cache = self.open_cache_for(command_path);
        let mut runtime = Runtime {
            settings: self.settings.clone(),
            clock: self.clock,
            cache,
            last_warnings: Vec::new(),
            last_providers: Vec::new(),
            last_partial: false,
        };
        let out = runtime.run_cached_command(command_path, key, ttl, fetch)?;
        Ok(out.envelope)
    }

    /// Build a metadata (cache-bypassed) success envelope for `command_path`
    /// from an already-resolved `data` value + provider statuses.
    pub fn metadata_envelope(
        &self,
        command_path: &str,
        data: serde_json::Value,
        providers: Vec<ProviderStatus>,
    ) -> Envelope {
        let mut env = Envelope::success(
            command_path,
            data,
            Vec::new(),
            CacheStatus::bypass(),
            providers,
            false,
        );
        env.meta.timestamp = self.now();
        env
    }

    // (intentionally left blank — see free function `block_on_fetch`.)

    /// A typed `not yet implemented` error for command groups whose handlers are
    /// scaffolded but not yet ported. The message names the completion-plan
    /// workstream so the gap is traceable. This is NOT an `unknown command`
    /// usage error — the command routes correctly, it is merely unimplemented.
    pub fn unimplemented(command_path: &str, workstream: &str) -> Error {
        Error::new(
            Code::Unsupported,
            format!(
                "{command_path}: not yet implemented in Rust port (see completion plan {workstream})"
            ),
        )
    }
}

/// Run an async provider-fetch future to completion from inside a synchronous
/// cache-flow closure.
///
/// The runner's `run_cached_command` takes a **synchronous** `FnOnce` fetch
/// closure (mirroring the Go `fetchFn`, whose HTTP calls block). Because the
/// Rust provider adapters are async, the closure bridges back to the async world
/// here — but only when the closure actually runs (i.e. on a cache miss / TTL
/// expiry), so a fresh hit still short-circuits WITHOUT a network call.
///
/// Uses `block_in_place` + the current runtime handle, which requires the
/// multi-threaded Tokio runtime the `defi` binary (`#[tokio::main]`) and the
/// app-level tests (`#[tokio::test(flavor = "multi_thread")]`) use.
pub fn block_on_fetch<F>(fut: F) -> F::Output
where
    F: std::future::Future,
{
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}
