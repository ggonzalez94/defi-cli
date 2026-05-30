//! Runner: provider routing + cache flow.
//!
//! Mirrors the cache-flow core of `internal/app/runner.go` — the part of the
//! runner that owns the **machine contract** rather than a specific command
//! group. Concretely, this module owns:
//!
//! * the cache policy state machine (`run_cached_command`): fresh-hit short
//!   circuit, TTL-expiry re-fetch, stale fallback within `max_stale`, stale
//!   budget / `no_stale` rejection, and strict-partial handling;
//! * success/error envelope construction (`emit_success` / `render_error`)
//!   including diagnostics (warnings / provider statuses / partial) propagation;
//! * the runner's pure helpers: cache-bypass routing (`should_open_cache`),
//!   foreign-error classification (`normalize_run_error`), stale-budget math
//!   (`stale_exceeds_budget` / `stale_fallback_allowed`), and the small string
//!   helpers (`trim_root_path` / `split_csv`);
//! * provider-selection helpers exercised by the `app` package tests
//!   (`normalize_lending_provider`, `parse_lend_position_type`,
//!   `select_yield_providers`).
//!
//! Idiomatic-Rust shape note: the Go runner writes rendered output to injected
//! `io.Writer`s and returns `error`. The Rust port returns values instead — the
//! cache flow yields the resolved success [`Envelope`] (plus its rendered
//! string) on success and a typed [`defi_errors::Error`] on failure, from which
//! the caller derives the exit code and (for errors) the full error envelope.

#![allow(dead_code)]

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use defi_cache::store::Store;
use defi_config::Settings;
use defi_errors::{Code, Error};
use defi_id::Chain;
use defi_model::{CacheStatus, Envelope, ErrorBody, ProviderStatus};
use defi_providers::LendPositionType;
use serde_json::Value;

/// What a cache-backed fetch closure returns.
///
/// Mirrors the Go `fetchFn` tuple
/// `(data any, providerStatus []ProviderStatus, warnings []string, partial bool, err error)`.
/// `error` is carried in-band (`Result` is the closure's return type) so that
/// provider-failure-with-diagnostics can drive stale fallback while still
/// reporting the attempted provider statuses.
pub struct FetchOutcome {
    /// The successfully fetched payload (the value placed into `data`).
    pub data: Value,
    /// Per-provider statuses observed during the fetch.
    pub providers: Vec<ProviderStatus>,
    /// Non-fatal warnings produced during the fetch.
    pub warnings: Vec<String>,
    /// Whether the result is partial (some providers failed).
    pub partial: bool,
}

/// A resolved success render.
#[derive(Debug)]
pub struct RunOutput {
    /// The fully-built success envelope (before rendering).
    pub envelope: Envelope,
    /// The rendered output string (per `settings`).
    pub rendered: String,
}

/// Runner runtime state for the cache-flow core.
///
/// Holds resolved [`Settings`], an injectable clock (for deterministic
/// timestamps in tests), an optional cache [`Store`], and the captured
/// last-command diagnostics (warnings / providers / partial) used when an error
/// envelope is rendered after the fact.
pub struct Runtime {
    /// Resolved settings (output mode, cache budget, strict, etc.).
    pub settings: Settings,
    /// Injectable clock for deterministic envelope timestamps.
    pub clock: fn() -> DateTime<Utc>,
    /// Optional sqlite cache store; `None` disables caching.
    pub cache: Option<Store>,
    /// Diagnostics captured by the most recent command (for error rendering).
    pub last_warnings: Vec<String>,
    /// Provider statuses captured by the most recent command.
    pub last_providers: Vec<ProviderStatus>,
    /// Partial flag captured by the most recent command.
    pub last_partial: bool,
}

impl Runtime {
    /// Run a cache-backed command.
    ///
    /// Implements the Go `runCachedCommand` policy: serve a fresh cache hit
    /// without calling the provider; on TTL expiry re-fetch; on provider
    /// failure fall back to stale data only when the error is retryable
    /// (`Unavailable`/`RateLimited`), stale fallback is enabled, and the entry
    /// is within the stale budget; in strict mode a partial fetch is an error.
    ///
    /// Returns the resolved success [`RunOutput`] or a typed [`Error`].
    pub fn run_cached_command<F>(
        &mut self,
        command_path: &str,
        key: &str,
        ttl: Duration,
        fetch: F,
    ) -> Result<RunOutput, Error>
    where
        F: FnOnce() -> Result<FetchOutcome, (Vec<ProviderStatus>, Vec<String>, bool, Error)>,
    {
        self.reset_command_diagnostics();

        let mut cache_status = cache_meta_miss();
        let mut warnings: Vec<String> = Vec::new();

        // Stale fallback bookkeeping (only populated when a stale hit exists).
        let mut stale: Option<StaleEntry> = None;

        if self.settings.cache_enabled {
            if let Some(store) = &self.cache {
                if let Ok(cached) = store.get(key, self.settings.max_stale) {
                    if cached.hit {
                        let entry_status = CacheStatus {
                            status: "hit".to_string(),
                            age_ms: duration_millis(cached.age),
                            stale: cached.stale,
                        };
                        match serde_json::from_slice::<Value>(&cached.value) {
                            Ok(data) if !cached.stale => {
                                // Fresh hit short-circuit: do NOT call the provider.
                                self.capture_command_diagnostics(
                                    warnings.clone(),
                                    Vec::new(),
                                    false,
                                );
                                return self.emit_success(
                                    command_path,
                                    data,
                                    warnings,
                                    entry_status,
                                    Vec::new(),
                                    false,
                                );
                            }
                            Ok(data) => {
                                // Stale hit: remember it as a possible fallback.
                                stale = Some(StaleEntry {
                                    data,
                                    observed_age: cached.age,
                                    observed_at: Instant::now(),
                                    cache_status: entry_status,
                                });
                            }
                            // A corrupt cached payload is ignored (treated as a
                            // miss), matching the Go `json.Unmarshal` err branch.
                            Err(_) => {}
                        }
                    }
                }
            }
        }

        // TTL expired (or miss / stale): call the provider exactly once.
        let outcome = fetch();
        match outcome {
            Err((provider_status, provider_warnings, partial, err)) => {
                warnings.extend(provider_warnings);
                self.capture_command_diagnostics(
                    warnings.clone(),
                    provider_status.clone(),
                    partial,
                );

                if let Some(stale) = stale {
                    if !stale_fallback_allowed(&err) {
                        return Err(err);
                    }
                    let mut stale_cache_status = stale.cache_status;
                    let current_stale_age = stale
                        .observed_age
                        .saturating_add(stale.observed_at.elapsed());
                    stale_cache_status.age_ms = duration_millis(current_stale_age);

                    if self.settings.no_stale {
                        return Err(Error::wrap(
                            Code::Stale,
                            "fresh provider fetch failed and stale fallback is disabled (--no-stale)",
                            err,
                        ));
                    }
                    if stale_exceeds_budget(current_stale_age, ttl, self.settings.max_stale) {
                        return Err(Error::wrap(
                            Code::Stale,
                            "fresh provider fetch failed and cached data exceeded stale budget",
                            err,
                        ));
                    }
                    warnings.push(
                        "provider fetch failed; serving stale data within max-stale budget"
                            .to_string(),
                    );
                    self.capture_command_diagnostics(
                        warnings.clone(),
                        provider_status.clone(),
                        false,
                    );
                    return self.emit_success(
                        command_path,
                        stale.data,
                        warnings,
                        stale_cache_status,
                        provider_status,
                        false,
                    );
                }
                Err(err)
            }
            Ok(outcome) => {
                let FetchOutcome {
                    data,
                    providers: provider_status,
                    warnings: provider_warnings,
                    partial,
                } = outcome;
                warnings.extend(provider_warnings);
                self.capture_command_diagnostics(
                    warnings.clone(),
                    provider_status.clone(),
                    partial,
                );

                if partial && self.settings.strict {
                    self.capture_command_diagnostics(
                        warnings.clone(),
                        provider_status.clone(),
                        true,
                    );
                    return Err(Error::new(
                        Code::PartialStrict,
                        "partial results returned in strict mode",
                    ));
                }

                if self.settings.cache_enabled {
                    if let Some(store) = &self.cache {
                        if let Ok(payload) = serde_json::to_vec(&data) {
                            // Best-effort write; a failure must not fail the command.
                            let _ = store.set(key, &payload, ttl);
                            cache_status = CacheStatus {
                                status: "write".to_string(),
                                age_ms: 0,
                                stale: false,
                            };
                        }
                    }
                }

                self.capture_command_diagnostics(
                    warnings.clone(),
                    provider_status.clone(),
                    partial,
                );
                self.emit_success(
                    command_path,
                    data,
                    warnings,
                    cache_status,
                    provider_status,
                    partial,
                )
            }
        }
    }

    /// Build the full error envelope (mirrors Go `renderError`).
    ///
    /// Error output ALWAYS carries the full envelope: `success=false`,
    /// `data=[]`, an [`defi_model::ErrorBody`] whose `type` is derived from the
    /// error code, `cache.status="bypass"`, and the supplied diagnostics. The
    /// `results_only`/`select` projection is intentionally ignored here.
    pub fn render_error(
        &self,
        command_path: &str,
        err: &Error,
        warnings: Vec<String>,
        providers: Vec<ProviderStatus>,
        partial: bool,
    ) -> Envelope {
        let command = if command_path.trim().is_empty() {
            // Mirrors the Go fallback to the last command / CLI name. The pure
            // module port has no `last_command`; an empty path falls back to the
            // root CLI name, matching `version.CLIName`.
            "defi".to_string()
        } else {
            command_path.to_string()
        };

        let code = err.code.as_i32() as i64;
        let error_type = error_type_for_code(err.code).to_string();
        let message = err.to_string();

        let mut env = Envelope::error(
            command,
            ErrorBody {
                code,
                error_type,
                message,
            },
            warnings,
            providers,
            partial,
        );
        env.meta.timestamp = (self.clock)();
        env
    }

    /// Reset the captured last-command diagnostics (mirrors Go
    /// `resetCommandDiagnostics`).
    fn reset_command_diagnostics(&mut self) {
        self.last_warnings.clear();
        self.last_providers.clear();
        self.last_partial = false;
    }

    /// Capture the latest command diagnostics so a subsequent error render can
    /// surface them (mirrors Go `captureCommandDiagnostics`).
    fn capture_command_diagnostics(
        &mut self,
        warnings: Vec<String>,
        providers: Vec<ProviderStatus>,
        partial: bool,
    ) {
        self.last_warnings = warnings;
        self.last_providers = providers;
        self.last_partial = partial;
    }

    /// Build + render a success envelope (mirrors Go `emitSuccess`).
    fn emit_success(
        &self,
        command_path: &str,
        data: Value,
        warnings: Vec<String>,
        cache: CacheStatus,
        providers: Vec<ProviderStatus>,
        partial: bool,
    ) -> Result<RunOutput, Error> {
        let mut envelope =
            Envelope::success(command_path, data, warnings, cache, providers, partial);
        envelope.meta.timestamp = (self.clock)();

        let rendered = defi_out::render(&envelope, &self.settings)
            .map_err(|e| Error::wrap(Code::Internal, "render output", e))?;
        Ok(RunOutput { envelope, rendered })
    }
}

/// A stale cache entry retained as a potential fallback during a failed fetch.
struct StaleEntry {
    data: Value,
    observed_age: Duration,
    observed_at: Instant,
    cache_status: CacheStatus,
}

/// The `miss` cache status used before any provider call (mirrors Go
/// `cacheMetaMiss`).
fn cache_meta_miss() -> CacheStatus {
    CacheStatus {
        status: "miss".to_string(),
        age_ms: 0,
        stale: false,
    }
}

/// Whole-millisecond duration (clamped to `i64`), matching Go's
/// `time.Duration.Milliseconds()`.
fn duration_millis(d: Duration) -> i64 {
    d.as_millis().min(i64::MAX as u128) as i64
}

/// The stable `error.type` string for a [`Code`] (mirrors the Go `renderError`
/// switch). Codes without an explicit case map to `internal_error`.
fn error_type_for_code(code: Code) -> &'static str {
    match code {
        Code::Usage => "usage_error",
        Code::Auth => "auth_error",
        Code::RateLimited => "rate_limited",
        Code::Unavailable => "provider_unavailable",
        Code::Unsupported => "unsupported",
        Code::Stale => "stale_data",
        Code::PartialStrict => "partial_results",
        Code::Blocked => "command_blocked",
        Code::ActionPlan => "action_plan_error",
        Code::ActionSim => "action_simulation_error",
        Code::ActionPolicy => "action_policy_error",
        Code::ActionTimeout => "action_timeout",
        Code::Signer => "signer_error",
        Code::Success | Code::Internal => "internal_error",
    }
}

/// Strip the leading root-command token from a command path
/// (`"defi yield opportunities"` → `"yield opportunities"`).
pub fn trim_root_path(path: &str) -> String {
    let parts: Vec<&str> = path.split_whitespace().collect();
    if parts.len() <= 1 {
        return path.to_string();
    }
    parts[1..].join(" ")
}

/// Split a comma-separated value into lowercased, trimmed, non-empty parts.
pub fn split_csv(value: &str) -> Vec<String> {
    if value.trim().is_empty() {
        return Vec::new();
    }
    value
        .split(',')
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|norm| !norm.is_empty())
        .collect()
}

/// Classify a foreign (non-typed) error as a usage error or an internal error
/// (mirrors Go `normalizeRunError` + `isLikelyUsageError`). A message that
/// looks like a clap/cobra usage failure becomes [`defi_errors::Code::Usage`];
/// anything else becomes [`defi_errors::Code::Internal`].
pub fn normalize_run_error(message: &str) -> Error {
    if is_likely_usage_error(message) {
        Error::new(Code::Usage, "invalid command input")
    } else {
        Error::new(Code::Internal, "execute command")
    }
}

/// Whether a foreign error message looks like a clap/cobra usage failure
/// (mirrors Go `isLikelyUsageError`). Matching is case-insensitive on the
/// trimmed message and uses the same substring patterns as Go.
fn is_likely_usage_error(message: &str) -> bool {
    let msg = message.trim().to_ascii_lowercase();
    const PATTERNS: [&str; 9] = [
        "unknown command",
        "unknown flag",
        "required flag(s)",
        "flag needs an argument",
        "requires at least",
        "requires exactly",
        "accepts ",
        "invalid argument",
        "invalid args",
    ];
    PATTERNS.iter().any(|p| msg.contains(p))
}

/// Whether a stale cache entry is now beyond the stale budget
/// (`age > ttl + max_stale`). A negative `max_stale` means unbounded (never
/// exceeds). An entry still within `ttl` never exceeds.
pub fn stale_exceeds_budget(age: Duration, ttl: Duration, max_stale: Duration) -> bool {
    // `Duration` is unsigned, so the Go `maxStale < 0` (unbounded) guard is
    // never triggered here; the within-ttl short-circuit + budget comparison
    // reproduce the rest of `staleExceedsBudget`.
    if age <= ttl {
        return false;
    }
    age > ttl.saturating_add(max_stale)
}

/// Whether a provider error permits serving stale cached data
/// (`Unavailable` or `RateLimited` only).
pub fn stale_fallback_allowed(err: &Error) -> bool {
    matches!(err.code, Code::Unavailable | Code::RateLimited)
}

/// Whether the cache should be opened for a command path. Metadata and
/// execution command paths bypass cache initialization (mirrors Go
/// `shouldOpenCache`).
pub fn should_open_cache(command_path: &str) -> bool {
    let path = normalize_command_path(command_path);
    match path.as_str() {
        "" | "version" | "schema" | "providers" | "providers list" | "chains list"
        | "chains gas" => return false,
        _ => {}
    }
    !is_execution_command_path(&path)
}

/// Lowercase + collapse whitespace of a command path (mirrors Go
/// `normalizeCommandPath`).
fn normalize_command_path(command_path: &str) -> String {
    command_path
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a normalized command path is an execution command path (mirrors Go
/// `isExecutionCommandPath`): the `actions` reads, plus any
/// `swap|bridge|approvals|transfer|lend|rewards|yield ... plan|submit|status`.
fn is_execution_command_path(path: &str) -> bool {
    match path {
        "actions" | "actions list" | "actions show" | "actions estimate" => return true,
        _ => {}
    }
    let parts: Vec<&str> = path.split_whitespace().collect();
    if parts.len() < 2 {
        return false;
    }
    match parts[0] {
        "swap" | "bridge" | "approvals" | "transfer" | "lend" | "rewards" | "yield" => {
            let last = parts[parts.len() - 1];
            last == "plan" || last == "submit" || last == "status"
        }
        _ => false,
    }
}

/// Normalize a lending-provider selector to its canonical name (delegates to
/// `defi_providers::normalize_lending_provider`).
pub fn normalize_lending_provider(input: &str) -> String {
    defi_providers::normalize::normalize_lending_provider(input)
}

/// Parse the `--type` lend-positions selector. Empty defaults to
/// [`LendPositionType::All`]; an unknown value is a usage error.
pub fn parse_lend_position_type(input: &str) -> Result<LendPositionType, Error> {
    let key = input.trim().to_ascii_lowercase();
    if key.is_empty() {
        return Ok(LendPositionType::All);
    }
    LendPositionType::parse(&key).ok_or_else(|| {
        Error::new(
            Code::Usage,
            "--type must be one of: all,supply,borrow,collateral",
        )
    })
}

/// Resolve the set of yield providers for a request.
///
/// With an empty `filter`, returns the alphabetically-sorted subset of
/// `available` providers that support the chain family (Solana: `kamino`;
/// EVM: `aave`/`morpho`; Moonwell only on Base/Optimism). With an explicit
/// `filter`, validates each name against `available` (unknown → usage error),
/// de-duplicates, and returns it sorted.
pub fn select_yield_providers(
    available: &[&str],
    filter: &[String],
    chain: &Chain,
) -> Result<Vec<String>, Error> {
    if filter.is_empty() {
        let mut keys: Vec<String> = available
            .iter()
            .filter(|name| yield_provider_supports_chain(name, chain))
            .map(|name| name.to_string())
            .collect();
        keys.sort();
        return Ok(keys);
    }

    let mut selected: Vec<String> = Vec::with_capacity(filter.len());
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in filter {
        let name = item.trim().to_ascii_lowercase();
        if !available.iter().any(|a| *a == name) {
            return Err(Error::new(
                Code::Usage,
                format!("unsupported yield provider: {item}"),
            ));
        }
        if seen.insert(name.clone()) {
            selected.push(name);
        }
    }
    selected.sort();
    Ok(selected)
}

/// Whether a yield provider supports the given chain family (mirrors Go
/// `yieldProviderSupportsChain`): `kamino` on Solana; `aave`/`morpho` on any
/// EVM chain; `moonwell` only on Base (8453) / Optimism (10); anything else
/// supports every chain.
fn yield_provider_supports_chain(name: &str, chain: &Chain) -> bool {
    match name {
        "kamino" => chain.is_solana(),
        "aave" | "morpho" => chain.is_evm(),
        "moonwell" => chain.is_evm() && (chain.evm_chain_id == 8453 || chain.evm_chain_id == 10),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::runner` (Go: `internal/app/runner.go`)
    //!
    //! This module owns the **cache-flow core** of the runner plus its pure
    //! helpers. "Correct" means it preserves the stable machine contract
    //! (design spec §2.5 behavioral invariants, §2.1 envelope, §2.2 exit codes)
    //! and the runner-owned routing/parsing behaviors. The criteria asserted
    //! below (NOT Go internals):
    //!
    //! 1. **Cache fresh-hit short-circuit.** A non-stale hit (`age <= ttl`)
    //!    serves cached data WITHOUT calling the provider; `cache.status="hit"`,
    //!    `stale=false`. (Spec §2.5.)
    //! 2. **TTL-expiry re-fetch.** Once `age > ttl` the provider is called
    //!    exactly once; on success the new data is written
    //!    (`cache.status="write"`, `stale=false`) and provider statuses are
    //!    surfaced in `meta.providers`.
    //! 3. **Stale fallback within budget.** On a retryable provider failure
    //!    (`Unavailable`/`RateLimited`) with a stale entry inside `max_stale`,
    //!    the runner serves the stale cached data (`cache.status="hit"`,
    //!    `stale=true`), surfaces the provider-failure status, and appends the
    //!    warning `"provider fetch failed; serving stale data within max-stale
    //!    budget"`. Exactly one fetch attempt.
    //! 4. **Stale budget rejection.** When the stale entry is beyond
    //!    `ttl + max_stale` (either initially, or because the failed fetch took
    //!    long enough to cross the budget), the command FAILS with exit code
    //!    `14` (`Stale`) and a message containing `"cached data exceeded stale
    //!    budget"`. The fetch is still attempted exactly once.
    //! 5. **No stale fallback on non-retryable errors.** An `Auth` failure is
    //!    NOT eligible for stale fallback; the command fails with exit code `10`
    //!    (`Auth`) even though a stale entry exists.
    //! 6. **Strict partial.** With `strict=true`, a partial fetch FAILS with
    //!    exit code `15` (`PartialStrict`); the error envelope built afterwards
    //!    has `error.type="partial_results"`, `meta.partial=true`, surfaces all
    //!    provider statuses, and preserves the propagated warning.
    //! 7. **Error-envelope shape (`render_error`).** Always a FULL envelope:
    //!    `success=false`, `data=[]`, `cache.status="bypass"`, the correct
    //!    `error.code`/`error.type` for each [`Code`], with diagnostics
    //!    (warnings/providers/partial) carried through. (Spec §2.1, §2.3.)
    //! 8. **Cache-bypass routing (`should_open_cache`).** Metadata paths
    //!    (`version`, `schema`, `providers`/`providers list`, `chains list`,
    //!    `chains gas`, empty) and execution command paths bypass cache init;
    //!    data commands (e.g. `lend markets`, `yield opportunities`) open it.
    //! 9. **Foreign-error classification (`normalize_run_error`).** clap/cobra
    //!    usage-shaped messages → `Usage` (exit 2); other foreign messages →
    //!    `Internal` (exit 1).
    //! 10. **Stale-budget math.** `stale_exceeds_budget`: within `ttl` → false;
    //!     `max_stale < 0` (unbounded) → false; `age > ttl+max_stale` → true.
    //!     `stale_fallback_allowed`: only `Unavailable`/`RateLimited`.
    //! 11. **String helpers.** `trim_root_path` strips the leading root token;
    //!     `split_csv` lowercases/trims/drops empties.
    //! 12. **Provider selection.** `normalize_lending_provider` canonicalizes
    //!     aliases; `parse_lend_position_type` defaults empty→All and rejects
    //!     unknowns as usage errors; `select_yield_providers` filters by chain
    //!     family when unfiltered, validates+dedupes+sorts an explicit filter,
    //!     and rejects unknown providers.
    //!
    //! Ported from `runner_cache_policy_test.go`, `provider_selection_test.go`,
    //! and the runner-helper cases in `runner_test.go`. Skipped: Go tests that
    //! assert command-group wiring (lend/swap/bridge/yield/etc.) — those belong
    //! to their own `defi-app` modules, not the cache-flow runner core.

    use super::*;
    use chrono::TimeZone;
    use defi_config::Settings;
    use defi_errors::{exit_code, Code, Error};
    use defi_model::ProviderStatus;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::time::Duration;

    // --- test fixtures -----------------------------------------------------

    /// Fixed clock so envelope timestamps are deterministic.
    fn fixed_clock() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 28, 0, 0, 0).unwrap()
    }

    /// Build a `Settings` for cache-policy tests with the given stale budget,
    /// no-stale toggle, and strict toggle. Other fields are minimal sane
    /// defaults; the runner cache flow only reads cache_enabled / max_stale /
    /// no_stale / strict / timeout / output_mode.
    fn policy_settings(max_stale: Duration, no_stale: bool, strict: bool) -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: false,
            enable_commands: Vec::new(),
            strict,
            timeout: Duration::from_secs(2),
            retries: 0,
            max_stale,
            no_stale,
            cache_enabled: true,
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

    /// A `Runtime` with a fresh temp-dir sqlite cache opened with `max_stale`.
    fn new_runtime(
        max_stale: Duration,
        no_stale: bool,
        strict: bool,
    ) -> (Runtime, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = Store::open(
            tmp.path().join("cache.db"),
            tmp.path().join("cache.lock"),
            max_stale,
        )
        .expect("open cache");
        let rt = Runtime {
            settings: policy_settings(max_stale, no_stale, strict),
            clock: fixed_clock,
            cache: Some(store),
            last_warnings: Vec::new(),
            last_providers: Vec::new(),
            last_partial: false,
        };
        (rt, tmp)
    }

    fn provider(name: &str, status: &str, latency: i64) -> ProviderStatus {
        ProviderStatus {
            name: name.to_string(),
            status: status.to_string(),
            latency_ms: latency,
        }
    }

    fn cache_status(env: &Envelope) -> &defi_model::CacheStatus {
        &env.meta.cache
    }

    fn data_source(env: &Envelope) -> Option<String> {
        env.data
            .as_ref()
            .and_then(|d| d.get("source"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    // --- 1. fresh hit short-circuit ---------------------------------------

    #[test]
    fn cache_fresh_hit_skips_provider_fetch() {
        let (mut rt, _tmp) = new_runtime(Duration::from_secs(300), false, false);
        let key = "runner-cache-policy-fresh-hit";
        // ttl large => the just-written entry is fresh.
        rt.cache
            .as_ref()
            .unwrap()
            .set(key, br#"{"source":"cache"}"#, Duration::from_secs(60))
            .unwrap();

        let mut fetch_calls = 0;
        let out = rt
            .run_cached_command("test command", key, Duration::from_secs(60), || {
                fetch_calls += 1;
                Ok(FetchOutcome {
                    data: json!({"source": "provider"}),
                    providers: vec![provider("test-provider", "ok", 1)],
                    warnings: Vec::new(),
                    partial: false,
                })
            })
            .expect("fresh hit success");

        assert_eq!(fetch_calls, 0, "fresh hit must NOT call the provider");
        assert!(out.envelope.success);
        assert_eq!(data_source(&out.envelope).as_deref(), Some("cache"));
        assert_eq!(cache_status(&out.envelope).status, "hit");
        assert!(!cache_status(&out.envelope).stale);
    }

    // --- 2. TTL-expiry re-fetch -------------------------------------------

    #[test]
    fn cache_refetches_provider_after_ttl_expiry() {
        let (mut rt, _tmp) = new_runtime(Duration::from_secs(300), false, false);
        let key = "runner-cache-policy-fetch-after-ttl";
        rt.cache
            .as_ref()
            .unwrap()
            .set(key, br#"{"source":"cache"}"#, Duration::from_secs(1))
            .unwrap();
        std::thread::sleep(Duration::from_millis(1200));

        let mut fetch_calls = 0;
        let out = rt
            .run_cached_command("test command", key, Duration::from_secs(1), || {
                fetch_calls += 1;
                Ok(FetchOutcome {
                    data: json!({"source": "provider"}),
                    providers: vec![provider("test-provider", "ok", 1)],
                    warnings: Vec::new(),
                    partial: false,
                })
            })
            .expect("refetch success");

        assert_eq!(
            fetch_calls, 1,
            "expected exactly one provider fetch after ttl"
        );
        assert!(out.envelope.success);
        assert_eq!(data_source(&out.envelope).as_deref(), Some("provider"));
        assert_eq!(cache_status(&out.envelope).status, "write");
        assert!(!cache_status(&out.envelope).stale);
        assert_eq!(out.envelope.meta.providers.len(), 1);
        assert_eq!(out.envelope.meta.providers[0].name, "test-provider");
    }

    // --- 3. stale fallback within budget ----------------------------------

    #[test]
    fn cache_falls_back_to_stale_on_retryable_failure() {
        let (mut rt, _tmp) = new_runtime(Duration::from_secs(5), false, false);
        let key = "runner-cache-policy-fallback-stale";
        rt.cache
            .as_ref()
            .unwrap()
            .set(key, br#"{"source":"cache"}"#, Duration::from_secs(1))
            .unwrap();
        std::thread::sleep(Duration::from_millis(1200));

        let mut fetch_calls = 0;
        let out = rt
            .run_cached_command("test command", key, Duration::from_secs(1), || {
                fetch_calls += 1;
                Err((
                    vec![provider("test-provider", "unavailable", 1)],
                    Vec::new(),
                    false,
                    Error::new(Code::Unavailable, "provider unavailable"),
                ))
            })
            .expect("stale fallback should succeed");

        assert_eq!(fetch_calls, 1);
        assert_eq!(data_source(&out.envelope).as_deref(), Some("cache"));
        assert_eq!(cache_status(&out.envelope).status, "hit");
        assert!(cache_status(&out.envelope).stale);
        assert_eq!(out.envelope.meta.providers.len(), 1);
        assert_eq!(out.envelope.meta.providers[0].status, "unavailable");
        assert!(out
            .envelope
            .warnings
            .iter()
            .any(|w| { w == "provider fetch failed; serving stale data within max-stale budget" }));
    }

    // --- 4a. stale budget rejection (beyond budget initially) -------------

    #[test]
    fn cache_rejects_stale_beyond_max_stale() {
        let (mut rt, _tmp) = new_runtime(Duration::from_millis(10), false, false);
        let key = "runner-cache-policy-too-stale";
        rt.cache
            .as_ref()
            .unwrap()
            .set(key, br#"{"source":"cache"}"#, Duration::from_secs(1))
            .unwrap();
        std::thread::sleep(Duration::from_millis(1300));

        let mut fetch_calls = 0;
        let err = rt
            .run_cached_command("test command", key, Duration::from_secs(1), || {
                fetch_calls += 1;
                Err((
                    vec![provider("test-provider", "unavailable", 1)],
                    Vec::new(),
                    false,
                    Error::new(Code::Unavailable, "provider unavailable"),
                ))
            })
            .expect_err("expected stale rejection");

        assert_eq!(fetch_calls, 1, "fetch attempted before stale rejection");
        assert_eq!(err.code, Code::Stale);
        assert_eq!(
            exit_code(&Err(Error::new(err.code, ""))),
            Code::Stale.as_i32()
        );
        assert!(
            err.to_string()
                .contains("cached data exceeded stale budget"),
            "got: {err}"
        );
    }

    // --- 4b. stale budget rejection (fetch delay crosses budget) ----------

    #[test]
    fn cache_rejects_stale_when_fetch_delay_crosses_budget() {
        let (mut rt, _tmp) = new_runtime(Duration::from_secs(2), false, false);
        let key = "runner-cache-policy-crosses-budget-during-fetch";
        rt.cache
            .as_ref()
            .unwrap()
            .set(key, br#"{"source":"cache"}"#, Duration::from_secs(1))
            .unwrap();
        std::thread::sleep(Duration::from_millis(1200));

        let mut fetch_calls = 0;
        let err = rt
            .run_cached_command("test command", key, Duration::from_secs(1), || {
                fetch_calls += 1;
                std::thread::sleep(Duration::from_secs(2));
                Err((
                    vec![provider("test-provider", "unavailable", 2000)],
                    Vec::new(),
                    false,
                    Error::new(Code::Unavailable, "provider unavailable"),
                ))
            })
            .expect_err("expected stale rejection after delayed fetch");

        assert_eq!(fetch_calls, 1);
        assert_eq!(err.code, Code::Stale);
        assert!(
            err.to_string()
                .contains("cached data exceeded stale budget"),
            "got: {err}"
        );
    }

    // --- 5. no stale fallback on auth failure -----------------------------

    #[test]
    fn cache_does_not_fall_back_on_auth_failure() {
        let (mut rt, _tmp) = new_runtime(Duration::from_secs(5), false, false);
        let key = "runner-cache-policy-no-fallback-auth";
        rt.cache
            .as_ref()
            .unwrap()
            .set(key, br#"{"source":"cache"}"#, Duration::from_secs(1))
            .unwrap();
        std::thread::sleep(Duration::from_millis(1200));

        let err = rt
            .run_cached_command("test command", key, Duration::from_secs(1), || {
                Err((
                    vec![provider("test-provider", "auth_error", 1)],
                    Vec::new(),
                    false,
                    Error::new(Code::Auth, "missing api key"),
                ))
            })
            .expect_err("expected auth error");

        assert_eq!(err.code, Code::Auth);
    }

    // --- 6. strict partial -------------------------------------------------

    #[test]
    fn strict_partial_fails_and_error_envelope_preserves_diagnostics() {
        let (mut rt, _tmp) = new_runtime(Duration::from_secs(5), false, true);
        let key = "runner-cache-policy-strict-partial";

        let err = rt
            .run_cached_command("test command", key, Duration::from_secs(1), || {
                Ok(FetchOutcome {
                    data: json!({"source": "provider"}),
                    providers: vec![
                        provider("aave", "ok", 12),
                        provider("morpho", "unavailable", 34),
                    ],
                    warnings: vec!["provider morpho failed: timeout".to_string()],
                    partial: true,
                })
            })
            .expect_err("expected strict partial error");

        assert_eq!(err.code, Code::PartialStrict);

        // The runner captured diagnostics; the error envelope must surface them.
        let env = rt.render_error(
            "test command",
            &err,
            rt.last_warnings.clone(),
            rt.last_providers.clone(),
            rt.last_partial,
        );
        assert!(!env.success);
        let body = env.error.as_ref().expect("error body present");
        assert_eq!(body.error_type, "partial_results");
        assert!(env.meta.partial);
        assert_eq!(env.meta.providers.len(), 2);
        assert!(env
            .warnings
            .iter()
            .any(|w| w == "provider morpho failed: timeout"));
    }

    // --- 7. error-envelope shape ------------------------------------------

    #[test]
    fn render_error_builds_full_bypass_envelope() {
        let (rt, _tmp) = new_runtime(Duration::from_secs(5), false, false);
        let err = Error::new(Code::Unavailable, "provider unavailable");
        let env = rt.render_error(
            "yield opportunities",
            &err,
            vec!["w1".to_string()],
            vec![provider("aave", "unavailable", 7)],
            false,
        );
        assert_eq!(env.version, "v1");
        assert!(!env.success);
        // data is an empty array (full-envelope-on-error contract).
        assert_eq!(env.data, Some(Value::Array(Vec::new())));
        let body = env.error.as_ref().expect("error body");
        assert_eq!(body.code, Code::Unavailable.as_i32() as i64);
        assert_eq!(body.error_type, "provider_unavailable");
        assert_eq!(body.message, "provider unavailable");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.command, "yield opportunities");
    }

    #[test]
    fn render_error_maps_each_code_to_its_type() {
        let (rt, _tmp) = new_runtime(Duration::from_secs(5), false, false);
        let cases = [
            (Code::Usage, "usage_error"),
            (Code::Auth, "auth_error"),
            (Code::RateLimited, "rate_limited"),
            (Code::Unavailable, "provider_unavailable"),
            (Code::Unsupported, "unsupported"),
            (Code::Stale, "stale_data"),
            (Code::PartialStrict, "partial_results"),
            (Code::Blocked, "command_blocked"),
            (Code::ActionPlan, "action_plan_error"),
            (Code::ActionSim, "action_simulation_error"),
            (Code::ActionPolicy, "action_policy_error"),
            (Code::ActionTimeout, "action_timeout"),
            (Code::Signer, "signer_error"),
            (Code::Internal, "internal_error"),
        ];
        for (code, want_type) in cases {
            let env = rt.render_error("cmd", &Error::new(code, "m"), vec![], vec![], false);
            let body = env.error.as_ref().expect("body");
            assert_eq!(body.error_type, want_type, "code {code:?}");
            assert_eq!(body.code, code.as_i32() as i64, "code {code:?}");
        }
    }

    // --- 8. cache-bypass routing ------------------------------------------

    #[test]
    fn should_open_cache_bypasses_metadata_and_execution_paths() {
        // Metadata + empty paths bypass.
        for p in [
            "",
            "version",
            "schema",
            "providers",
            "providers list",
            "chains list",
            "chains gas",
        ] {
            assert!(!should_open_cache(p), "{p:?} should bypass cache");
        }
        // Execution command paths bypass.
        for p in [
            "swap plan",
            "bridge submit",
            "lend supply plan",
            "yield deposit submit",
            "actions list",
        ] {
            assert!(
                !should_open_cache(p),
                "{p:?} (execution) should bypass cache"
            );
        }
        // Data commands open the cache.
        for p in [
            "lend markets",
            "yield opportunities",
            "chains assets",
            "protocols fees",
        ] {
            assert!(should_open_cache(p), "{p:?} should open cache");
        }
    }

    // --- 9. foreign-error classification ----------------------------------

    #[test]
    fn normalize_run_error_classifies_usage_vs_internal() {
        let usage_msgs = [
            "unknown command \"frobnicate\" for \"defi\"",
            "unknown flag: --nope",
            "required flag(s) \"chain\" not set",
            "flag needs an argument: --chain",
            "requires at least 1 arg(s)",
            "accepts 1 arg(s), received 2",
            "invalid argument \"x\" for \"--limit\"",
        ];
        for m in usage_msgs {
            assert_eq!(normalize_run_error(m).code, Code::Usage, "msg: {m}");
        }
        let internal_msgs = ["sqlite is on fire", "connection reset by peer"];
        for m in internal_msgs {
            assert_eq!(normalize_run_error(m).code, Code::Internal, "msg: {m}");
        }
    }

    // --- 10. stale-budget math --------------------------------------------

    #[test]
    fn stale_exceeds_budget_math() {
        let ttl = Duration::from_secs(10);
        // within ttl => never exceeds.
        assert!(!stale_exceeds_budget(
            Duration::from_secs(5),
            ttl,
            Duration::from_secs(1)
        ));
        // exactly ttl => false (age <= ttl).
        assert!(!stale_exceeds_budget(ttl, ttl, Duration::from_secs(0)));
        // within ttl + max_stale => false.
        assert!(!stale_exceeds_budget(
            Duration::from_secs(15),
            ttl,
            Duration::from_secs(10)
        ));
        // beyond ttl + max_stale => true.
        assert!(stale_exceeds_budget(
            Duration::from_secs(25),
            ttl,
            Duration::from_secs(10)
        ));
    }

    #[test]
    fn stale_fallback_allowed_only_for_retryable() {
        assert!(stale_fallback_allowed(&Error::new(Code::Unavailable, "x")));
        assert!(stale_fallback_allowed(&Error::new(Code::RateLimited, "x")));
        assert!(!stale_fallback_allowed(&Error::new(Code::Auth, "x")));
        assert!(!stale_fallback_allowed(&Error::new(Code::Usage, "x")));
        assert!(!stale_fallback_allowed(&Error::new(Code::Internal, "x")));
    }

    // --- 11. string helpers -----------------------------------------------

    #[test]
    fn trim_root_path_strips_leading_token() {
        assert_eq!(
            trim_root_path("defi yield opportunities"),
            "yield opportunities"
        );
        // single token is returned unchanged.
        assert_eq!(trim_root_path("defi"), "defi");
    }

    #[test]
    fn split_csv_lowercases_trims_and_drops_empties() {
        assert_eq!(split_csv("Aave, morpho ,"), vec!["aave", "morpho"]);
        assert!(split_csv("   ").is_empty());
        assert!(split_csv("").is_empty());
    }

    // --- 12. provider selection -------------------------------------------

    #[test]
    fn normalize_lending_provider_canonicalizes_aliases() {
        assert_eq!(normalize_lending_provider("AAVE-V3"), "aave");
        assert_eq!(normalize_lending_provider("morpho-blue"), "morpho");
        assert_eq!(normalize_lending_provider("kamino-finance"), "kamino");
    }

    #[test]
    fn parse_lend_position_type_defaults_and_rejects() {
        assert_eq!(parse_lend_position_type("").unwrap(), LendPositionType::All);
        assert_eq!(
            parse_lend_position_type("all").unwrap(),
            LendPositionType::All
        );
        assert_eq!(
            parse_lend_position_type("supply").unwrap(),
            LendPositionType::Supply
        );
        assert_eq!(
            parse_lend_position_type("borrow").unwrap(),
            LendPositionType::Borrow
        );
        assert_eq!(
            parse_lend_position_type("collateral").unwrap(),
            LendPositionType::Collateral
        );
        let err = parse_lend_position_type("debt").expect_err("invalid type rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn select_yield_providers_filters_by_chain_family_when_unfiltered() {
        let available = ["aave", "morpho", "kamino"];
        let evm = defi_id::parse_chain("base").expect("base chain");
        assert_eq!(
            select_yield_providers(&available, &[], &evm).unwrap(),
            vec!["aave".to_string(), "morpho".to_string()]
        );
        let solana = defi_id::parse_chain("solana").expect("solana chain");
        assert_eq!(
            select_yield_providers(&available, &[], &solana).unwrap(),
            vec!["kamino".to_string()]
        );
    }

    #[test]
    fn select_yield_providers_explicit_filter_validates_and_sorts() {
        let available = ["aave", "morpho"];
        let chain = defi_id::parse_chain("base").expect("base chain");
        // explicit filter bypasses chain-family defaults; order normalized.
        assert_eq!(
            select_yield_providers(&available, &["aave".to_string()], &chain).unwrap(),
            vec!["aave".to_string()]
        );
        // unknown provider => usage error.
        let err = select_yield_providers(&available, &["unknown".to_string()], &chain)
            .expect_err("unknown provider rejected");
        assert_eq!(err.code, Code::Usage);
    }
}
