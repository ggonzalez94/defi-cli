//! Configuration: defaults + file/env/flags precedence.
//!
//! Mirrors `internal/config`. Precedence is `flags > env > config file >
//! defaults` (behavioral invariant — spec §2.5).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use defi_errors::{Code, Error};
use serde::Deserialize;

// =============================================================================
// LOCKED INTERFACE.
//
// These signatures are the contract the tests below lock in. They intentionally
// diverge from the Go file layout where idiomatic Rust differs:
//
//   * `Settings::load` takes an injected [`Env`] (env-var + home/XDG resolution)
//     instead of reading process-global `std::env`. Go isolates env per-test with
//     `t.Setenv`; Rust tests run in parallel within one process, so a global env
//     would be racy. An injected `Env` makes the precedence contract
//     (flags > env > file > defaults) deterministic AND parallel-safe — that is
//     the real behavior this module owns, not "reads getenv".
//   * Durations are `std::time::Duration` and parse Go-style strings ("10s",
//     "5m", "0s") so the file/env/flag duration contract is preserved.
// =============================================================================

/// Raw global CLI flags (the highest-precedence layer).
///
/// Field names + declaration order mirror `config.GlobalFlags`. Optional inputs
/// are `Option<_>` so "unset" is distinguishable from "set to the zero value"
/// (Go used the zero value / a sentinel; in Rust `None` means "flag absent").
#[derive(Debug, Clone, Default)]
pub struct GlobalFlags {
    pub config_path: Option<String>,
    pub json: bool,
    pub plain: bool,
    pub select: Option<String>,
    pub results_only: bool,
    pub enable_commands: Option<String>,
    pub strict: bool,
    pub timeout: Option<String>,
    pub retries: Option<i64>,
    pub max_stale: Option<String>,
    pub no_stale: bool,
    pub no_cache: bool,
}

/// Resolved configuration. Field names + declaration order mirror
/// `config.Settings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub output_mode: String,
    pub select_fields: Vec<String>,
    pub results_only: bool,
    pub enable_commands: Vec<String>,
    pub strict: bool,
    pub timeout: Duration,
    pub retries: i64,
    pub max_stale: Duration,
    pub no_stale: bool,
    pub cache_enabled: bool,
    pub cache_path: PathBuf,
    pub cache_lock_path: PathBuf,
    pub action_store_path: PathBuf,
    pub action_lock_path: PathBuf,
    pub defillama_api_key: String,
    pub uniswap_api_key: String,
    pub oneinch_api_key: String,
    pub jupiter_api_key: String,
    pub bungee_api_key: String,
    pub bungee_affiliate: String,
}

/// Environment abstraction consumed by [`Settings::load`].
///
/// Provides process environment variables plus the user home directory (used to
/// derive default cache/config paths). Injected so precedence tests are
/// deterministic and parallel-safe without touching process-global state.
pub trait Env {
    /// Look up an environment variable; `None` when unset or empty.
    fn var(&self, key: &str) -> Option<String>;
    /// The user home directory (`os.UserHomeDir` equivalent).
    fn home_dir(&self) -> Option<PathBuf>;
}

/// In-memory [`Env`] for tests and callers that want full control.
#[derive(Debug, Clone, Default)]
pub struct MapEnv {
    pub vars: HashMap<String, String>,
    pub home: Option<PathBuf>,
}

impl MapEnv {
    /// A `MapEnv` with the given home directory and no variables set.
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        MapEnv {
            vars: HashMap::new(),
            home: Some(home.into()),
        }
    }

    /// Set a variable (builder style).
    pub fn set(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.vars.insert(key.into(), value.into());
        self
    }
}

impl Env for MapEnv {
    fn var(&self, key: &str) -> Option<String> {
        match self.vars.get(key) {
            Some(v) if !v.is_empty() => Some(v.clone()),
            _ => None,
        }
    }
    fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }
}

/// Process-backed [`Env`]: reads `std::env` and the OS home directory.
///
/// This is the production [`Env`] used by the CLI; tests use [`MapEnv`] so the
/// precedence contract stays parallel-safe and deterministic.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemEnv;

impl Env for SystemEnv {
    fn var(&self, key: &str) -> Option<String> {
        match std::env::var(key) {
            Ok(v) if !v.is_empty() => Some(v),
            _ => None,
        }
    }
    fn home_dir(&self) -> Option<PathBuf> {
        // Mirrors Go `os.UserHomeDir`, which reads `$HOME` on unix and
        // `%USERPROFILE%` on Windows.
        #[cfg(windows)]
        {
            std::env::var_os("USERPROFILE").map(PathBuf::from)
        }
        #[cfg(not(windows))]
        {
            std::env::var_os("HOME").map(PathBuf::from)
        }
    }
}

impl Settings {
    /// Load settings applying `flags > env > config file > defaults`.
    ///
    /// Mirrors `config.Load`. Reads the config file (if present) through the
    /// path resolved from `flags.config_path` / `XDG_CONFIG_HOME` / `~/.config`,
    /// overlays environment variables from `env`, then flags. Returns a typed
    /// [`Error`] (usage code) on conflicting flags or unparseable durations.
    pub fn load(flags: &GlobalFlags, env: &dyn Env) -> Result<Settings, Error> {
        let mut settings = default_settings(env)?;

        let cfg_path = resolve_config_path(flags.config_path.as_deref(), env)?;
        apply_file_config(&cfg_path, env, &mut settings)?;

        apply_env(env, &mut settings);

        apply_flags(flags, &mut settings)?;

        // Duration / value floors (mirrors the tail of `config.Load`). An
        // explicit zero from a flag is preserved by `apply_flags`; these only
        // guard against an empty/negative value falling through from
        // file/env/defaults.
        if settings.output_mode.is_empty() {
            settings.output_mode = "json".to_string();
        }
        if settings.timeout.is_zero() {
            settings.timeout = Duration::from_secs(10);
        }
        if settings.retries < 0 {
            settings.retries = 0;
        }

        Ok(settings)
    }
}

/// Built-in defaults (lowest precedence layer). Mirrors `defaultSettings`.
fn default_settings(env: &dyn Env) -> Result<Settings, Error> {
    let (cache_path, cache_lock_path) = default_cache_paths(env)?;
    let cache_dir = cache_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    Ok(Settings {
        output_mode: "json".to_string(),
        select_fields: Vec::new(),
        results_only: false,
        enable_commands: Vec::new(),
        strict: false,
        timeout: Duration::from_secs(10),
        retries: 2,
        max_stale: Duration::from_secs(5 * 60),
        no_stale: false,
        cache_enabled: true,
        cache_path,
        cache_lock_path,
        action_store_path: cache_dir.join("actions.db"),
        action_lock_path: cache_dir.join("actions.lock"),
        defillama_api_key: String::new(),
        uniswap_api_key: String::new(),
        oneinch_api_key: String::new(),
        jupiter_api_key: String::new(),
        bungee_api_key: String::new(),
        bungee_affiliate: String::new(),
    })
}

/// Resolve the config file path. Mirrors `resolveConfigPath`.
///
/// An explicit (non-blank) input is normalized (trim, control-char reject,
/// `~`/`~/` expansion, `Clean`, absolutize). Otherwise it derives
/// `<XDG_CONFIG_HOME | ~/.config>/defi/config.yaml`.
fn resolve_config_path(input: Option<&str>, env: &dyn Env) -> Result<PathBuf, Error> {
    if let Some(raw) = input {
        if !raw.trim().is_empty() {
            return normalize_path(raw, env);
        }
    }
    let base = match env.var("XDG_CONFIG_HOME") {
        Some(v) => PathBuf::from(v),
        None => home_dir(env)?.join(".config"),
    };
    Ok(base.join("defi").join("config.yaml"))
}

/// Default sqlite cache + lock paths. Mirrors `defaultCachePaths`.
fn default_cache_paths(env: &dyn Env) -> Result<(PathBuf, PathBuf), Error> {
    let base = match env.var("XDG_CACHE_HOME") {
        Some(v) => PathBuf::from(v),
        None => home_dir(env)?.join(".cache"),
    };
    let dir = base.join("defi");
    Ok((dir.join("cache.db"), dir.join("cache.lock")))
}

/// The user home directory, or a typed usage error if it cannot be resolved.
fn home_dir(env: &dyn Env) -> Result<PathBuf, Error> {
    env.home_dir()
        .ok_or_else(|| Error::new(Code::Usage, "resolve home directory"))
}

/// Normalize an explicit filesystem path. Mirrors `fsutil.NormalizePath`:
/// trim, reject control chars, expand `~`/`~/`, `filepath.Clean`, absolutize.
fn normalize_path(input: &str, env: &dyn Env) -> Result<PathBuf, Error> {
    let value = input.trim();
    if value.is_empty() {
        return Ok(PathBuf::new());
    }
    if value.chars().any(|c| (c as u32) < 0x20) {
        return Err(Error::new(Code::Usage, "path contains control characters"));
    }

    let expanded: PathBuf = if value == "~" {
        home_dir(env)?
    } else if let Some(rest) = value.strip_prefix("~/") {
        home_dir(env)?.join(rest)
    } else {
        PathBuf::from(value)
    };

    let cleaned = clean_path(&expanded);
    absolutize(&cleaned)
}

/// Lexically clean a path the way Go's `filepath.Clean` does: collapse `.`
/// and redundant separators, resolve `..` against prior non-`..` components,
/// and keep a leading `/` for absolute paths.
fn clean_path(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut stack: Vec<Component> = Vec::new();
    let mut has_root = false;
    let mut prefix: Option<Component> = None;

    for comp in path.components() {
        match comp {
            Component::Prefix(_) => prefix = Some(comp),
            Component::RootDir => has_root = true,
            Component::CurDir => {}
            Component::ParentDir => match stack.last() {
                Some(Component::Normal(_)) => {
                    stack.pop();
                }
                Some(Component::ParentDir) | None if !has_root => stack.push(comp),
                None => {} // `..` at filesystem root is the root itself
                _ => {}
            },
            Component::Normal(_) => stack.push(comp),
        }
    }

    let mut out = PathBuf::new();
    if let Some(p) = prefix {
        out.push(p.as_os_str());
    }
    if has_root {
        out.push(std::path::MAIN_SEPARATOR.to_string());
    }
    for comp in &stack {
        out.push(comp.as_os_str());
    }

    if out.as_os_str().is_empty() {
        // Clean of "" / "." is ".".
        out.push(".");
    }
    out
}

/// Make a path absolute against the current working directory, like Go's
/// `filepath.Abs`. Already-absolute paths pass through unchanged.
fn absolutize(path: &Path) -> Result<PathBuf, Error> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir()
        .map_err(|e| Error::wrap(Code::Usage, "resolve absolute path", e))?;
    Ok(clean_path(&cwd.join(path)))
}

// =============================================================================
// File config (YAML). Mirrors the `fileConfig` struct in `config.go`. Optional
// scalar fields use `Option` so "absent in file" is distinguished from "set to
// the zero value" (Go used pointer fields for the same reason).
// =============================================================================

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    strict: Option<bool>,
    #[serde(default)]
    timeout: Option<String>,
    #[serde(default)]
    retries: Option<i64>,
    #[serde(default)]
    cache: CacheConfig,
    #[serde(default)]
    execution: ExecutionConfig,
    #[serde(default)]
    providers: ProvidersConfig,
}

#[derive(Debug, Default, Deserialize)]
struct CacheConfig {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    max_stale: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    lock_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ExecutionConfig {
    #[serde(default)]
    actions_path: Option<String>,
    #[serde(default)]
    actions_lock_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ProvidersConfig {
    #[serde(default)]
    defillama: ProviderKeyConfig,
    #[serde(default)]
    uniswap: ProviderKeyConfig,
    #[serde(default)]
    oneinch: ProviderKeyConfig,
    #[serde(default)]
    jupiter: ProviderKeyConfig,
    #[serde(default)]
    bungee: BungeeConfig,
}

#[derive(Debug, Default, Deserialize)]
struct ProviderKeyConfig {
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BungeeConfig {
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    affiliate: Option<String>,
    #[serde(default)]
    affiliate_env: Option<String>,
}

/// Returns a non-empty `Option<String>` (treats `Some("")` as `None`).
fn non_empty(value: Option<&String>) -> Option<String> {
    value
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Overlay file-config values onto `settings`. A missing file is NOT an error
/// (defaults stand); a malformed file or unparseable duration IS. Mirrors
/// `applyFileConfig`.
fn apply_file_config(path: &Path, env: &dyn Env, settings: &mut Settings) -> Result<(), Error> {
    let buf = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Error::wrap(Code::Usage, "read config", e)),
    };

    let cfg: FileConfig =
        serde_yaml::from_str(&buf).map_err(|e| Error::wrap(Code::Usage, "parse config yaml", e))?;

    if let Some(output) = non_empty(cfg.output.as_ref()) {
        settings.output_mode = output.to_lowercase();
    }
    if let Some(strict) = cfg.strict {
        settings.strict = strict;
    }
    if let Some(timeout) = non_empty(cfg.timeout.as_ref()) {
        let nanos = parse_go_duration(&timeout)
            .map_err(|e| Error::new(Code::Usage, format!("config timeout: {e}")))?;
        settings.timeout = timeout_from_nanos(nanos);
    }
    if let Some(retries) = cfg.retries {
        settings.retries = retries;
    }
    if let Some(enabled) = cfg.cache.enabled {
        settings.cache_enabled = enabled;
    }
    if let Some(max_stale) = non_empty(cfg.cache.max_stale.as_ref()) {
        let nanos = parse_go_duration(&max_stale)
            .map_err(|e| Error::new(Code::Usage, format!("config cache.max_stale: {e}")))?;
        settings.max_stale = max_stale_from_nanos(nanos);
    }
    if let Some(p) = non_empty(cfg.cache.path.as_ref()) {
        settings.cache_path = PathBuf::from(p);
    }
    if let Some(p) = non_empty(cfg.cache.lock_path.as_ref()) {
        settings.cache_lock_path = PathBuf::from(p);
    }
    if let Some(p) = non_empty(cfg.execution.actions_path.as_ref()) {
        settings.action_store_path = PathBuf::from(p);
    }
    if let Some(p) = non_empty(cfg.execution.actions_lock_path.as_ref()) {
        settings.action_lock_path = PathBuf::from(p);
    }

    // Provider keys. The file may carry a literal `api_key` or an indirection
    // `api_key_env: NAME` (read the value of env var NAME). Order mirrors Go:
    // for each provider the literal applies first, then the env-name
    // indirection overrides it (matching `applyFileConfig`).
    apply_provider_key(
        &cfg.providers.defillama,
        env,
        &mut settings.defillama_api_key,
    );
    apply_provider_key(&cfg.providers.uniswap, env, &mut settings.uniswap_api_key);
    apply_provider_key(&cfg.providers.oneinch, env, &mut settings.oneinch_api_key);
    apply_provider_key(&cfg.providers.jupiter, env, &mut settings.jupiter_api_key);

    if let Some(v) = non_empty(cfg.providers.bungee.api_key.as_ref()) {
        settings.bungee_api_key = v;
    }
    if let Some(name) = non_empty(cfg.providers.bungee.api_key_env.as_ref()) {
        settings.bungee_api_key = env.var(&name).unwrap_or_default();
    }
    if let Some(v) = non_empty(cfg.providers.bungee.affiliate.as_ref()) {
        settings.bungee_affiliate = v;
    }
    if let Some(name) = non_empty(cfg.providers.bungee.affiliate_env.as_ref()) {
        settings.bungee_affiliate = env.var(&name).unwrap_or_default();
    }

    Ok(())
}

/// Apply a provider's `api_key` / `api_key_env` indirection onto `target`.
fn apply_provider_key(cfg: &ProviderKeyConfig, env: &dyn Env, target: &mut String) {
    if let Some(v) = non_empty(cfg.api_key.as_ref()) {
        *target = v;
    }
    if let Some(name) = non_empty(cfg.api_key_env.as_ref()) {
        // Go reads `os.Getenv(name)`, which yields "" for an unset var.
        *target = env.var(&name).unwrap_or_default();
    }
}

/// Overlay environment variables onto `settings`. Mirrors `applyEnv`. Empty
/// values are treated as unset (matching Go's `if v := os.Getenv(...); v != ""`,
/// which the injected [`Env::var`] enforces by returning `None` for empties).
fn apply_env(env: &dyn Env, settings: &mut Settings) {
    if let Some(v) = env.var("DEFI_OUTPUT") {
        settings.output_mode = v.to_lowercase();
    }
    if let Some(v) = env.var("DEFI_STRICT") {
        if let Some(b) = parse_go_bool(&v) {
            settings.strict = b;
        }
    }
    if let Some(v) = env.var("DEFI_TIMEOUT") {
        if let Ok(nanos) = parse_go_duration(&v) {
            settings.timeout = timeout_from_nanos(nanos);
        }
    }
    if let Some(v) = env.var("DEFI_RETRIES") {
        if let Ok(n) = v.parse::<i64>() {
            settings.retries = n;
        }
    }
    if let Some(v) = env.var("DEFI_MAX_STALE") {
        if let Ok(nanos) = parse_go_duration(&v) {
            settings.max_stale = max_stale_from_nanos(nanos);
        }
    }
    if let Some(v) = env.var("DEFI_NO_STALE") {
        if let Some(b) = parse_go_bool(&v) {
            settings.no_stale = b;
        }
    }
    if let Some(v) = env.var("DEFI_NO_CACHE") {
        if let Some(b) = parse_go_bool(&v) {
            settings.cache_enabled = !b;
        }
    }
    if let Some(v) = env.var("DEFI_CACHE_PATH") {
        settings.cache_path = PathBuf::from(v);
    }
    if let Some(v) = env.var("DEFI_CACHE_LOCK_PATH") {
        settings.cache_lock_path = PathBuf::from(v);
    }
    if let Some(v) = env.var("DEFI_ACTIONS_PATH") {
        settings.action_store_path = PathBuf::from(v);
    }
    if let Some(v) = env.var("DEFI_ACTIONS_LOCK_PATH") {
        settings.action_lock_path = PathBuf::from(v);
    }
    if let Some(v) = env.var("DEFI_UNISWAP_API_KEY") {
        settings.uniswap_api_key = v;
    }
    if let Some(v) = env.var("DEFI_DEFILLAMA_API_KEY") {
        settings.defillama_api_key = v;
    }
    if let Some(v) = env.var("DEFI_1INCH_API_KEY") {
        settings.oneinch_api_key = v;
    }
    if let Some(v) = env.var("DEFI_JUPITER_API_KEY") {
        settings.jupiter_api_key = v;
    }
    if let Some(v) = env.var("DEFI_BUNGEE_API_KEY") {
        settings.bungee_api_key = v;
    }
    if let Some(v) = env.var("DEFI_BUNGEE_AFFILIATE") {
        settings.bungee_affiliate = v;
    }
}

/// Overlay CLI flags onto `settings` (highest precedence). Mirrors `applyFlags`,
/// including its validations. Returns a typed usage [`Error`] on conflicting
/// output flags, unparseable durations, or a non-`json|plain` output mode.
fn apply_flags(flags: &GlobalFlags, settings: &mut Settings) -> Result<(), Error> {
    if flags.json && flags.plain {
        return Err(Error::new(
            Code::Usage,
            "cannot use --json and --plain together",
        ));
    }
    if flags.json {
        settings.output_mode = "json".to_string();
    }
    if flags.plain {
        settings.output_mode = "plain".to_string();
    }

    if let Some(select) = &flags.select {
        if !select.trim().is_empty() {
            settings.select_fields = split_csv(select);
        }
    }
    settings.results_only = flags.results_only;

    if let Some(enable) = &flags.enable_commands {
        if !enable.trim().is_empty() {
            settings.enable_commands = split_csv(enable);
        }
    }

    if flags.strict {
        settings.strict = true;
    }
    if let Some(timeout) = &flags.timeout {
        if !timeout.is_empty() {
            let nanos = parse_go_duration(timeout)
                .map_err(|e| Error::new(Code::Usage, format!("parse --timeout: {e}")))?;
            settings.timeout = timeout_from_nanos(nanos);
        }
    }
    // Go: `if flags.Retries >= 0`. A negative flag is treated as "unset".
    if let Some(retries) = flags.retries {
        if retries >= 0 {
            settings.retries = retries;
        }
    }
    if let Some(max_stale) = &flags.max_stale {
        if !max_stale.is_empty() {
            let nanos = parse_go_duration(max_stale)
                .map_err(|e| Error::new(Code::Usage, format!("parse --max-stale: {e}")))?;
            settings.max_stale = max_stale_from_nanos(nanos);
        }
    }
    if flags.no_stale {
        settings.no_stale = true;
    }
    if flags.no_cache {
        settings.cache_enabled = false;
    }

    if settings.output_mode != "json" && settings.output_mode != "plain" {
        return Err(Error::new(Code::Usage, "output must be json or plain"));
    }

    Ok(())
}

/// Split a comma-separated list, trimming each item and dropping empties.
/// Mirrors the `strings.Split` + trim + skip-empty loops in `applyFlags`.
fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Parse a Go-style boolean (`strconv.ParseBool`): accepts
/// `1 t T TRUE true True 0 f F FALSE false False`. Anything else → `None`
/// (the value is left unchanged, matching Go's "ignore on parse error").
fn parse_go_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

/// Parse a Go `time.ParseDuration` string into **signed** nanoseconds.
///
/// Supports a signed sequence of `<number><unit>` components (e.g. `"300ms"`,
/// `"-1.5h"`, `"2h45m"`, `"0s"`), with units `ns us µs μs ms s m h`. Fractional
/// values are permitted. Negative durations parse successfully and yield a
/// negative result, exactly like Go: callers then apply Go's `Load` floors
/// (`Timeout <= 0 → 10s`, `MaxStale < 0 → 5m`) at the point of assignment. This
/// matters for contract parity — Go silently floors a negative `--timeout` /
/// `--max-stale` (or a file value) rather than erroring.
fn parse_go_duration(input: &str) -> Result<i128, String> {
    // Mirrors the relevant subset of Go's stdlib parser.
    let original = input;
    let mut s = input;

    let mut neg = false;
    if let Some(rest) = s.strip_prefix('-') {
        neg = true;
        s = rest;
    } else if let Some(rest) = s.strip_prefix('+') {
        s = rest;
    }

    // Special case: "0" with no unit is valid in Go.
    if s == "0" {
        return Ok(0);
    }
    if s.is_empty() {
        return Err(format!("invalid duration {original:?}"));
    }

    // Total in nanoseconds, accumulated as f64 then rounded — matches Go's
    // fractional handling closely enough for the units the CLI uses.
    let mut total_nanos: f64 = 0.0;
    let mut saw_component = false;

    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Parse the numeric portion (digits + optional single '.').
        let num_start = i;
        let mut saw_digit = false;
        let mut saw_dot = false;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c.is_ascii_digit() {
                saw_digit = true;
                i += 1;
            } else if c == '.' && !saw_dot {
                saw_dot = true;
                i += 1;
            } else {
                break;
            }
        }
        if !saw_digit {
            return Err(format!("invalid duration {original:?}"));
        }
        let num: f64 = s[num_start..i]
            .parse()
            .map_err(|_| format!("invalid duration {original:?}"))?;

        // Parse the unit (letters / µ).
        let unit_start = i;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c.is_ascii_digit() || c == '.' {
                break;
            }
            i += 1;
        }
        if unit_start == i {
            return Err(format!("missing unit in duration {original:?}"));
        }
        let unit = &s[unit_start..i];
        let unit_nanos: f64 = match unit {
            "ns" => 1.0,
            "us" | "µs" | "μs" => 1_000.0,
            "ms" => 1_000_000.0,
            "s" => 1_000_000_000.0,
            "m" => 60.0 * 1_000_000_000.0,
            "h" => 3_600.0 * 1_000_000_000.0,
            other => return Err(format!("unknown unit {other:?} in duration {original:?}")),
        };

        total_nanos += num * unit_nanos;
        saw_component = true;
    }

    if !saw_component {
        return Err(format!("invalid duration {original:?}"));
    }

    let magnitude = total_nanos.round() as i128;
    Ok(if neg { -magnitude } else { magnitude })
}

/// Convert parsed signed nanoseconds into a timeout [`Duration`], applying Go's
/// `Load` floor `if Timeout <= 0 { 10s }`. A non-positive value (negative OR
/// zero) becomes the 10s default, matching Go exactly.
fn timeout_from_nanos(nanos: i128) -> Duration {
    if nanos <= 0 {
        Duration::from_secs(10)
    } else {
        Duration::from_nanos(nanos as u64)
    }
}

/// Convert parsed signed nanoseconds into a max-stale [`Duration`], applying
/// Go's `Load` floor `if MaxStale < 0 { 5m }`. Zero is preserved (the explicit
/// `--max-stale 0s` contract); only a strictly negative value resets to 5m.
fn max_stale_from_nanos(nanos: i128) -> Duration {
    if nanos < 0 {
        Duration::from_secs(5 * 60)
    } else {
        Duration::from_nanos(nanos as u64)
    }
}

// =============================================================================
// SUCCESS CRITERIA (RED phase — tests written before implementation)
//
// This module (Go source: internal/config) owns the config-precedence
// behavioral invariant (spec §2.5: flags > env > config file > defaults) plus
// the default values that feed cache freshness/staleness and output rendering.
// The Rust port is "correct" iff:
//
//   1. PRECEDENCE — flags beat env beat file beat defaults.
//        a. A flag value wins over both an env value and a file value for the
//           same setting (output mode, retries). [ports
//           TestLoadPrecedenceFlagsOverEnvOverFile]
//        b. An env value wins over a file value when no flag is set.
//        c. A file value wins over the built-in default when neither env nor
//           flag is set.
//        d. With no flags/env/file, the built-in defaults apply.
//
//   2. DEFAULTS — exact values the rest of the contract depends on:
//        output_mode="json", timeout=10s, retries=2, max_stale=5m,
//        cache_enabled=true. Cache/action paths derive from XDG_CACHE_HOME (or
//        ~/.cache) → <base>/defi/{cache.db,cache.lock,actions.db,actions.lock}.
//
//   3. OUTPUT MODE — `--json` and `--plain` together is a usage error; `--json`
//        forces json, `--plain` forces plain; an output mode other than
//        json|plain (e.g. from file/env) is a usage error. [ports
//        TestLoadMutuallyExclusiveOutputFlags]
//
//   4. DURATION FLOORS — Load tolerates an explicit zero max-stale ("0s" ⇒
//        Duration::ZERO, NOT reset to the 5m default). Negative-style guards:
//        a negative retries flag is ignored (treated as "unset"); a
//        timeout/max-stale that resolves to <= 0 from FILE/ENV falls back to the
//        default, but an explicit "0s" max-stale FLAG stays zero. [ports
//        TestLoadAllowsZeroMaxStale]
//
//   5. PROVIDER KEYS — env wins for each provider key:
//        DEFI_DEFILLAMA_API_KEY, DEFI_JUPITER_API_KEY, DEFI_1INCH_API_KEY,
//        DEFI_UNISWAP_API_KEY, DEFI_BUNGEE_API_KEY, DEFI_BUNGEE_AFFILIATE.
//        [ports TestLoadDefiLlamaAPIKeyFromEnv, TestLoadJupiterAPIKeyFromEnv,
//        TestLoadBungeeDedicatedSettingsFromEnv]
//        File `api_key`/`affiliate` populate the same fields when env is unset.
//        [ports TestLoadBungeeDedicatedSettingsFromFile]
//        A file `api_key_env: NAME` indirection reads the value of env var NAME.
//
//   6. EXECUTION PATHS — DEFI_ACTIONS_PATH / DEFI_ACTIONS_LOCK_PATH env override
//        the derived action store/lock paths. [ports TestLoadExecutionPathsFromEnv]
//        File `execution.actions_path`/`actions_lock_path` do the same when env
//        is unset.
//
//   7. LIST PARSING — `--select a, b ,,c` ⇒ ["a","b","c"] (trim, drop empties);
//        `--enable-commands` parses the same way; `--results-only` maps straight
//        through.
//
//   8. NO PROCESS-GLOBAL ENV — Load reads only the injected `Env`; two loads
//        with different `MapEnv`s never interfere (parallel-safe). This is the
//        idiomatic-Rust replacement for Go's `t.Setenv` isolation.
//
//   9. FILE PARSING — a missing config file is NOT an error (defaults stand);
//        a malformed YAML config file is a typed error; an unparseable duration
//        in the file is a typed error.
//
// Ported Go tests (internal/config/config_test.go): all six map to criteria
// 1/3/4/5/6 above. Skipped: none meaningful — every Go assertion is preserved,
// just re-expressed against the injected `Env` instead of `t.Setenv`.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;

    /// A `MapEnv` whose home points at a fresh temp dir (so default cache/config
    /// paths resolve under an isolated location, never the real `$HOME`).
    fn env_with_temp_home(home: &Path) -> MapEnv {
        MapEnv::with_home(home)
    }

    fn write_config(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("config.yaml");
        fs::write(&path, body).expect("write config");
        path
    }

    // ---- Criterion 1a: flags beat env beat file -------------------------------
    // Ports TestLoadPrecedenceFlagsOverEnvOverFile.

    #[test]
    fn flags_win_over_env_and_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(tmp.path(), "output: plain\nretries: 1\n");

        // file says output=plain,retries=1 ; env says output=json ;
        // flags say plain + retries=5 → flags win.
        let env = env_with_temp_home(tmp.path()).set("DEFI_OUTPUT", "json");
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            plain: true,
            retries: Some(5),
            ..Default::default()
        };

        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(
            s.output_mode, "plain",
            "flag --plain must win over env json"
        );
        assert_eq!(s.retries, 5, "retries must come from the flag");
    }

    // ---- Criterion 1b: env beats file (no flag) -------------------------------

    #[test]
    fn env_wins_over_file_when_no_flag() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(tmp.path(), "output: plain\nretries: 1\n");

        let env = env_with_temp_home(tmp.path())
            .set("DEFI_OUTPUT", "json")
            .set("DEFI_RETRIES", "7");
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            ..Default::default()
        };

        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(
            s.output_mode, "json",
            "env DEFI_OUTPUT must beat file output"
        );
        assert_eq!(s.retries, 7, "env DEFI_RETRIES must beat file retries");
    }

    // ---- Criterion 1c: file beats defaults ------------------------------------

    #[test]
    fn file_wins_over_defaults_when_no_env_or_flag() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(tmp.path(), "output: plain\nretries: 1\ntimeout: 30s\n");

        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            ..Default::default()
        };

        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(s.output_mode, "plain");
        assert_eq!(s.retries, 1);
        assert_eq!(s.timeout, Duration::from_secs(30));
    }

    // ---- Criterion 1d + 2: defaults -------------------------------------------

    #[test]
    fn defaults_apply_with_no_inputs() {
        let tmp = TempDir::new().unwrap();
        // No config file at the default location; empty env; no flags.
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags::default();

        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(s.output_mode, "json");
        assert_eq!(s.timeout, Duration::from_secs(10));
        assert_eq!(s.retries, 2);
        assert_eq!(s.max_stale, Duration::from_secs(5 * 60));
        assert!(s.cache_enabled);
        assert!(s.select_fields.is_empty());
        assert!(s.enable_commands.is_empty());
        assert!(!s.results_only);
        assert!(!s.strict);
        assert!(!s.no_stale);
        // All provider keys default empty.
        assert_eq!(s.defillama_api_key, "");
        assert_eq!(s.uniswap_api_key, "");
        assert_eq!(s.oneinch_api_key, "");
        assert_eq!(s.jupiter_api_key, "");
        assert_eq!(s.bungee_api_key, "");
        assert_eq!(s.bungee_affiliate, "");
    }

    // ---- Criterion 2: derived cache/action paths under XDG_CACHE_HOME ---------

    #[test]
    fn cache_and_action_paths_derive_from_xdg_cache_home() {
        let tmp = TempDir::new().unwrap();
        let cache_base = tmp.path().join("xdg-cache");
        let env = env_with_temp_home(tmp.path())
            .set("XDG_CACHE_HOME", cache_base.to_string_lossy().into_owned());
        let flags = GlobalFlags::default();

        let s = Settings::load(&flags, &env).expect("load");
        let dir = cache_base.join("defi");
        assert_eq!(s.cache_path, dir.join("cache.db"));
        assert_eq!(s.cache_lock_path, dir.join("cache.lock"));
        assert_eq!(s.action_store_path, dir.join("actions.db"));
        assert_eq!(s.action_lock_path, dir.join("actions.lock"));
    }

    #[test]
    fn cache_paths_fall_back_to_home_dot_cache() {
        let tmp = TempDir::new().unwrap();
        // No XDG_CACHE_HOME → ~/.cache/defi.
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags::default();

        let s = Settings::load(&flags, &env).expect("load");
        let dir = tmp.path().join(".cache").join("defi");
        assert_eq!(s.cache_path, dir.join("cache.db"));
        assert_eq!(s.cache_lock_path, dir.join("cache.lock"));
    }

    // ---- Criterion 3: output-mode flag rules ----------------------------------
    // Ports TestLoadMutuallyExclusiveOutputFlags.

    #[test]
    fn json_and_plain_together_is_usage_error() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            json: true,
            plain: true,
            ..Default::default()
        };
        let err = Settings::load(&flags, &env).expect_err("conflicting output flags must error");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    #[test]
    fn json_flag_forces_json_and_plain_flag_forces_plain() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(tmp.path(), "output: plain\n");

        let env = env_with_temp_home(tmp.path());
        let json_flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            json: true,
            ..Default::default()
        };
        let s = Settings::load(&json_flags, &env).expect("load");
        assert_eq!(s.output_mode, "json");

        let cfg2 = write_config(tmp.path(), "output: json\n");
        let plain_flags = GlobalFlags {
            config_path: Some(cfg2.to_string_lossy().into_owned()),
            plain: true,
            ..Default::default()
        };
        let s2 = Settings::load(&plain_flags, &env).expect("load");
        assert_eq!(s2.output_mode, "plain");
    }

    #[test]
    fn invalid_output_mode_from_file_is_usage_error() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(tmp.path(), "output: yaml\n");
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let err = Settings::load(&flags, &env).expect_err("non json|plain output must error");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // ---- Criterion 4: duration floors -----------------------------------------
    // Ports TestLoadAllowsZeroMaxStale.

    #[test]
    fn explicit_zero_max_stale_flag_stays_zero() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            max_stale: Some("0s".to_string()),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(
            s.max_stale,
            Duration::ZERO,
            "explicit 0s flag must NOT reset to 5m default"
        );
    }

    #[test]
    fn negative_retries_flag_is_ignored() {
        // Go: `if flags.Retries >= 0`. A negative retries flag is treated as
        // "unset" and the default (2) stands.
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            retries: Some(-1),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(s.retries, 2);
    }

    // Negative durations parse OK in Go and are silently floored by `Load`
    // (Timeout <= 0 -> 10s, MaxStale < 0 -> 5m). A negative duration must NOT
    // be a usage error — that would diverge from the Go contract (exit 0 +
    // stdout success vs exit 2 + stderr error envelope).

    #[test]
    fn negative_timeout_flag_floors_to_default_not_error() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            timeout: Some("-5s".to_string()),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("negative --timeout must floor, not error");
        assert_eq!(
            s.timeout,
            Duration::from_secs(10),
            "Go floors a non-positive timeout to 10s"
        );
    }

    #[test]
    fn negative_max_stale_flag_floors_to_default_not_error() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            max_stale: Some("-5m".to_string()),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("negative --max-stale must floor, not error");
        assert_eq!(
            s.max_stale,
            Duration::from_secs(5 * 60),
            "Go floors a negative max_stale to 5m"
        );
    }

    #[test]
    fn negative_timeout_in_file_floors_to_default_not_error() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(tmp.path(), "timeout: -5s\n");
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("negative file timeout must floor, not error");
        assert_eq!(s.timeout, Duration::from_secs(10));
    }

    #[test]
    fn negative_max_stale_in_file_floors_to_default_not_error() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(tmp.path(), "cache:\n  max_stale: -5m\n");
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("negative file max_stale must floor");
        assert_eq!(s.max_stale, Duration::from_secs(5 * 60));
    }

    #[test]
    fn negative_max_stale_env_floors_to_default() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path()).set("DEFI_MAX_STALE", "-1ns");
        let s = Settings::load(&GlobalFlags::default(), &env).expect("load");
        assert_eq!(s.max_stale, Duration::from_secs(5 * 60));
    }

    // A later positive layer must still win over a floored negative from an
    // earlier layer (mirrors Go applying the floor only once, at the end).
    #[test]
    fn positive_flag_timeout_wins_over_negative_file_timeout() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(tmp.path(), "timeout: -5s\n");
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            timeout: Some("30s".to_string()),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(s.timeout, Duration::from_secs(30));
    }

    #[test]
    fn unparseable_timeout_flag_is_usage_error() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            timeout: Some("not-a-duration".to_string()),
            ..Default::default()
        };
        let err = Settings::load(&flags, &env).expect_err("bad --timeout must error");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // ---- Criterion 5: provider keys -------------------------------------------
    // Ports TestLoadDefiLlamaAPIKeyFromEnv, TestLoadJupiterAPIKeyFromEnv,
    // TestLoadBungeeDedicatedSettingsFromEnv, TestLoadBungeeDedicatedSettingsFromFile.

    #[test]
    fn defillama_api_key_from_env() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path()).set("DEFI_DEFILLAMA_API_KEY", "key-123");
        let s = Settings::load(&GlobalFlags::default(), &env).expect("load");
        assert_eq!(s.defillama_api_key, "key-123");
    }

    #[test]
    fn jupiter_api_key_from_env() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path()).set("DEFI_JUPITER_API_KEY", "jup-key");
        let s = Settings::load(&GlobalFlags::default(), &env).expect("load");
        assert_eq!(s.jupiter_api_key, "jup-key");
    }

    #[test]
    fn oneinch_and_uniswap_api_keys_from_env() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path())
            .set("DEFI_1INCH_API_KEY", "oneinch-key")
            .set("DEFI_UNISWAP_API_KEY", "uni-key");
        let s = Settings::load(&GlobalFlags::default(), &env).expect("load");
        assert_eq!(s.oneinch_api_key, "oneinch-key");
        assert_eq!(s.uniswap_api_key, "uni-key");
    }

    #[test]
    fn bungee_settings_from_env() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path())
            .set("DEFI_BUNGEE_API_KEY", "bungee-key")
            .set("DEFI_BUNGEE_AFFILIATE", "affiliate-id");
        let s = Settings::load(&GlobalFlags::default(), &env).expect("load");
        assert_eq!(s.bungee_api_key, "bungee-key");
        assert_eq!(s.bungee_affiliate, "affiliate-id");
    }

    #[test]
    fn bungee_settings_from_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(
            tmp.path(),
            "providers:\n  bungee:\n    api_key: file-key\n    affiliate: file-affiliate\n",
        );
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(s.bungee_api_key, "file-key");
        assert_eq!(s.bungee_affiliate, "file-affiliate");
    }

    #[test]
    fn env_provider_key_wins_over_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(
            tmp.path(),
            "providers:\n  defillama:\n    api_key: file-key\n",
        );
        let env = env_with_temp_home(tmp.path()).set("DEFI_DEFILLAMA_API_KEY", "env-key");
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(
            s.defillama_api_key, "env-key",
            "env must beat file for provider keys"
        );
    }

    #[test]
    fn file_api_key_env_indirection_reads_named_var() {
        // file `api_key_env: NAME` ⇒ read value of env var NAME (Go behavior).
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(
            tmp.path(),
            "providers:\n  defillama:\n    api_key_env: MY_DEFILLAMA_VAR\n",
        );
        let env = env_with_temp_home(tmp.path()).set("MY_DEFILLAMA_VAR", "resolved-key");
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(s.defillama_api_key, "resolved-key");
    }

    // ---- Criterion 6: execution paths -----------------------------------------
    // Ports TestLoadExecutionPathsFromEnv.

    #[test]
    fn execution_paths_from_env() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path())
            .set("DEFI_ACTIONS_PATH", "/tmp/defi-actions.db")
            .set("DEFI_ACTIONS_LOCK_PATH", "/tmp/defi-actions.lock");
        let s = Settings::load(&GlobalFlags::default(), &env).expect("load");
        assert_eq!(s.action_store_path, PathBuf::from("/tmp/defi-actions.db"));
        assert_eq!(s.action_lock_path, PathBuf::from("/tmp/defi-actions.lock"));
    }

    #[test]
    fn execution_paths_from_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(
            tmp.path(),
            "execution:\n  actions_path: /var/defi/actions.db\n  actions_lock_path: /var/defi/actions.lock\n",
        );
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(s.action_store_path, PathBuf::from("/var/defi/actions.db"));
        assert_eq!(s.action_lock_path, PathBuf::from("/var/defi/actions.lock"));
    }

    // ---- Criterion 7: list parsing --------------------------------------------

    #[test]
    fn select_parses_trimmed_nonempty_fields() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            select: Some("a, b ,,c".to_string()),
            results_only: true,
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(s.select_fields, vec!["a", "b", "c"]);
        assert!(s.results_only);
    }

    #[test]
    fn enable_commands_parses_trimmed_nonempty() {
        let tmp = TempDir::new().unwrap();
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            enable_commands: Some(" lend , swap ,".to_string()),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("load");
        assert_eq!(s.enable_commands, vec!["lend", "swap"]);
    }

    // ---- Criterion 8: no process-global env (parallel safety) -----------------

    #[test]
    fn two_loads_with_distinct_envs_do_not_interfere() {
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        let env_a = env_with_temp_home(tmp_a.path()).set("DEFI_OUTPUT", "plain");
        let env_b = env_with_temp_home(tmp_b.path()); // no DEFI_OUTPUT

        let a = Settings::load(&GlobalFlags::default(), &env_a).expect("load a");
        let b = Settings::load(&GlobalFlags::default(), &env_b).expect("load b");

        assert_eq!(a.output_mode, "plain");
        assert_eq!(b.output_mode, "json", "env_b must be unaffected by env_a");
    }

    // ---- Criterion 9: file parsing edge cases ---------------------------------

    #[test]
    fn missing_config_file_is_not_an_error() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist.yaml");
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            config_path: Some(missing.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let s = Settings::load(&flags, &env).expect("missing file must fall back to defaults");
        assert_eq!(s.output_mode, "json");
        assert_eq!(s.retries, 2);
    }

    #[test]
    fn malformed_yaml_config_is_error() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(tmp.path(), "output: : : not yaml\n  - broken\n");
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert!(
            Settings::load(&flags, &env).is_err(),
            "malformed yaml must error"
        );
    }

    // ---- parse_go_duration: direct Go-parity edge cases -----------------------

    #[test]
    fn duration_parser_matches_go_edge_cases() {
        const S: i128 = 1_000_000_000;
        const MS: i128 = 1_000_000;
        // (input, expected signed nanos) — values verified against Go time.ParseDuration.
        assert_eq!(parse_go_duration("0"), Ok(0));
        assert_eq!(parse_go_duration("-0"), Ok(0));
        assert_eq!(parse_go_duration("0s"), Ok(0));
        assert_eq!(parse_go_duration("-0s"), Ok(0));
        assert_eq!(parse_go_duration("5s"), Ok(5 * S));
        assert_eq!(parse_go_duration("+3s"), Ok(3 * S));
        assert_eq!(parse_go_duration("-5m"), Ok(-5 * 60 * S));
        assert_eq!(parse_go_duration(".5s"), Ok(500 * MS));
        assert_eq!(parse_go_duration("1.s"), Ok(S)); // trailing dot, no frac digits
        assert_eq!(parse_go_duration("2h45m"), Ok((2 * 3600 + 45 * 60) * S));
        assert_eq!(parse_go_duration("300ms"), Ok(300 * MS));
        // Unicode micro variants.
        assert_eq!(parse_go_duration("1µs"), Ok(1_000));
        assert_eq!(parse_go_duration("1μs"), Ok(1_000));
        assert_eq!(parse_go_duration("1us"), Ok(1_000));
    }

    #[test]
    fn duration_parser_rejects_go_invalid_inputs() {
        // Bare number with no unit (Go: "missing unit").
        assert!(parse_go_duration("100").is_err());
        assert!(parse_go_duration("5").is_err());
        // Unknown unit (Go: `unknown unit "d"`).
        assert!(parse_go_duration("1d").is_err());
        // Empty / non-numeric.
        assert!(parse_go_duration("").is_err());
        assert!(parse_go_duration("abc").is_err());
    }

    #[test]
    fn unparseable_duration_in_file_is_error() {
        let tmp = TempDir::new().unwrap();
        let cfg = write_config(tmp.path(), "timeout: not-a-duration\n");
        let env = env_with_temp_home(tmp.path());
        let flags = GlobalFlags {
            config_path: Some(cfg.to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert!(
            Settings::load(&flags, &env).is_err(),
            "bad file timeout must error"
        );
    }
}
