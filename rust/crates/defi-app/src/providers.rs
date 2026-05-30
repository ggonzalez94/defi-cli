//! `providers` command group handler.
//!
//! Go source: `internal/app/runner.go` — `newProvidersCommand` (the `providers
//! list` subcommand) plus the provider-catalog assembly site
//! (`s.providerInfos = []model.ProviderInfo{ ... }`, runner.go ~L193-209) and
//! `runner_test.go` (`TestRunnerProvidersList`,
//! `TestRunnerProvidersListBypassesCacheOpen`).
//!
//! `providers list` is a **metadata-only** command: it requires no provider API
//! keys, bypasses cache initialization, and renders a fixed catalog of
//! [`defi_model::ProviderInfo`] entries (one per provider/mode the CLI wires
//! up). It is one of the deterministic offline commands covered by the Go
//! golden fixture `rust/tests/golden/providers-list.json`.
//!
//! This module owns two pieces of the machine contract:
//!   1. [`provider_catalog`] — the canonical, declaration-ordered list of
//!      `ProviderInfo` the CLI advertises. Order, `requires_key`,
//!      `capabilities`, `key_env_var`, and `capability_auth` are all part of the
//!      contract and must match the Go reference byte-for-byte.
//!   2. [`list`] — the `providers list` command handler: it builds a full
//!      success [`defi_model::Envelope`] whose `data` is the catalog and whose
//!      `meta.cache.status` is `"bypass"` (this command never touches the
//!      cache), with `command == "providers list"`.

use defi_model::{CacheStatus, Envelope, ProviderCapabilityAuth, ProviderInfo};

/// Build a [`ProviderInfo`] entry from its contract parts.
///
/// Helper keeping [`provider_catalog`] declarative and free of repeated struct
/// literals. `key_env_var` is the top-level provider key hint (empty string =>
/// omitted via `omitempty`), and `capability_auth` is the per-capability auth
/// list (empty => omitted).
fn info(
    name: &str,
    provider_type: &str,
    requires_key: bool,
    capabilities: &[&str],
    key_env_var: &str,
    capability_auth: Vec<ProviderCapabilityAuth>,
) -> ProviderInfo {
    ProviderInfo {
        name: name.to_string(),
        provider_type: provider_type.to_string(),
        requires_key,
        capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
        key_env_var_name: key_env_var.to_string(),
        capability_auth,
    }
}

/// Build a [`ProviderCapabilityAuth`] entry (empty description => omitted).
fn auth(capability: &str, key_env_var: &str, description: &str) -> ProviderCapabilityAuth {
    ProviderCapabilityAuth {
        capability: capability.to_string(),
        key_env_var: key_env_var.to_string(),
        description: description.to_string(),
    }
}

/// The canonical, declaration-ordered provider catalog advertised by
/// `providers list`.
///
/// Order mirrors the Go runner's `s.providerInfos` assembly
/// (`internal/app/runner.go` ~L193-209): each provider's `Info()` in sequence
/// `defillama, aave, morpho, kamino, moonwell, across, lifi, bungee (bridge),
/// 1inch, uniswap, tempo, taikoswap, jupiter, bungee (swap), fibrous`. The
/// field values are the machine contract pinned by the Go golden fixture
/// `rust/tests/golden/providers-list.json`. This is a pure, offline,
/// key-free metadata function — it performs no I/O and reads no env vars.
pub fn provider_catalog() -> Vec<ProviderInfo> {
    vec![
        info(
            "defillama",
            "market+bridge-data",
            false,
            &[
                "chains.top",
                "chains.assets",
                "protocols.top",
                "protocols.categories",
                "protocols.fees",
                "protocols.revenue",
                "dexes.volume",
                "stablecoins.top",
                "stablecoins.chains",
                "bridge.list",
                "bridge.details",
            ],
            "DEFI_DEFILLAMA_API_KEY",
            vec![
                auth(
                    "chains.assets",
                    "DEFI_DEFILLAMA_API_KEY",
                    "Required for chain-level TVL by asset endpoint",
                ),
                auth(
                    "bridge.details",
                    "DEFI_DEFILLAMA_API_KEY",
                    "Required for bridge analytics details endpoint",
                ),
                auth(
                    "bridge.list",
                    "DEFI_DEFILLAMA_API_KEY",
                    "Required for bridge analytics list endpoint",
                ),
            ],
        ),
        info(
            "aave",
            "lending+yield",
            false,
            &[
                "lend.markets",
                "lend.rates",
                "lend.positions",
                "yield.opportunities",
                "yield.positions",
                "yield.history",
                "lend.plan",
                "lend.execute",
                "yield.plan",
                "yield.execute",
                "rewards.plan",
                "rewards.execute",
            ],
            "",
            vec![],
        ),
        info(
            "morpho",
            "lending+yield",
            false,
            &[
                "lend.markets",
                "lend.rates",
                "lend.positions",
                "yield.opportunities",
                "yield.positions",
                "yield.history",
                "lend.plan",
                "lend.execute",
                "yield.plan",
                "yield.execute",
            ],
            "",
            vec![],
        ),
        info(
            "kamino",
            "lending+yield",
            false,
            &[
                "lend.markets",
                "lend.rates",
                "yield.opportunities",
                "yield.history",
            ],
            "",
            vec![],
        ),
        info(
            "moonwell",
            "lending+yield",
            false,
            &[
                "lend.markets",
                "lend.rates",
                "lend.positions",
                "yield.opportunities",
                "yield.positions",
                "lend.plan",
                "lend.execute",
                "yield.plan",
                "yield.execute",
            ],
            "",
            vec![],
        ),
        info(
            "across",
            "bridge",
            false,
            &["bridge.quote", "bridge.plan", "bridge.execute"],
            "",
            vec![],
        ),
        info(
            "lifi",
            "bridge",
            false,
            &["bridge.quote", "bridge.plan", "bridge.execute"],
            "",
            vec![],
        ),
        info(
            "bungee",
            "bridge",
            false,
            &["bridge.quote"],
            "",
            vec![
                auth(
                    "bridge.quote",
                    "DEFI_BUNGEE_API_KEY",
                    "Optional dedicated backend mode (requires both API key and affiliate)",
                ),
                auth(
                    "bridge.quote",
                    "DEFI_BUNGEE_AFFILIATE",
                    "Optional dedicated backend mode (requires both API key and affiliate)",
                ),
            ],
        ),
        info(
            "1inch",
            "swap",
            true,
            &["swap.quote"],
            "DEFI_1INCH_API_KEY",
            vec![auth("swap.quote", "DEFI_1INCH_API_KEY", "")],
        ),
        info(
            "uniswap",
            "swap",
            true,
            &["swap.quote"],
            "DEFI_UNISWAP_API_KEY",
            vec![auth("swap.quote", "DEFI_UNISWAP_API_KEY", "")],
        ),
        info(
            "tempo",
            "swap",
            false,
            &["swap.quote", "swap.plan", "swap.execute"],
            "",
            vec![],
        ),
        info(
            "taikoswap",
            "swap",
            false,
            &["swap.quote", "swap.plan", "swap.execute"],
            "",
            vec![],
        ),
        info(
            "jupiter",
            "swap",
            false,
            &["swap.quote"],
            "DEFI_JUPITER_API_KEY",
            vec![auth(
                "swap.quote",
                "DEFI_JUPITER_API_KEY",
                "Optional API key for higher Jupiter API limits",
            )],
        ),
        info(
            "bungee",
            "swap",
            false,
            &["swap.quote"],
            "",
            vec![
                auth(
                    "swap.quote",
                    "DEFI_BUNGEE_API_KEY",
                    "Optional dedicated backend mode (requires both API key and affiliate)",
                ),
                auth(
                    "swap.quote",
                    "DEFI_BUNGEE_AFFILIATE",
                    "Optional dedicated backend mode (requires both API key and affiliate)",
                ),
            ],
        ),
        info("fibrous", "swap", false, &["swap.quote"], "", vec![]),
    ]
}

/// Handle `providers list`: build the full success [`Envelope`] (metadata
/// command, cache bypassed) whose `data` is the [`provider_catalog`].
///
/// Mirrors the Go runner's `newProvidersCommand` `list` handler, which emits a
/// success envelope via `emitSuccess(... s.providerInfos, nil,
/// cacheMetaBypass(), nil, false)`: command `"providers list"`, no warnings,
/// `cache.status == "bypass"`, no provider statuses, `partial == false`.
pub fn list() -> Envelope {
    let catalog = provider_catalog();
    let data = serde_json::to_value(&catalog).unwrap_or(serde_json::Value::Null);
    Envelope::success(
        "providers list",
        data,
        Vec::new(),
        CacheStatus::bypass(),
        Vec::new(),
        false,
    )
}

/// clap parsing + handler for the `providers` command group.
pub mod cli {
    use clap::Subcommand;
    use defi_errors::Error;
    use defi_model::Envelope;

    use crate::ctx::AppCtx;

    /// `providers` subcommands (Go `newProvidersCommand`).
    #[derive(Subcommand, Debug)]
    pub enum ProvidersCmd {
        /// List supported providers and API key metadata (no keys required).
        List,
    }

    impl ProvidersCmd {
        /// The leaf path token (for `meta.command`).
        pub fn path(&self) -> &'static str {
            match self {
                ProvidersCmd::List => "list",
            }
        }
    }

    /// Handle `providers <sub>`.
    pub async fn handle(_ctx: &AppCtx, cmd: ProvidersCmd) -> Result<Envelope, Error> {
        match cmd {
            ProvidersCmd::List => Ok(super::list()),
        }
    }
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::providers` (`providers list`)
    //!
    //! Go sources: `internal/app/runner.go` (`newProvidersCommand` +
    //! `s.providerInfos` assembly) and `internal/app/runner_test.go`
    //! (`TestRunnerProvidersList`, `TestRunnerProvidersListBypassesCacheOpen`).
    //!
    //! `providers list` is a deterministic, offline, **metadata-only** command.
    //! Its output is the primary success oracle captured in
    //! `rust/tests/golden/providers-list.json` (the `--results-only` form: a bare
    //! `ProviderInfo` array, exit 0). The Rust port is "correct" iff:
    //!
    //!  P1. **Catalog parity (golden).** [`provider_catalog`], serialized as the
    //!      `data` payload, is byte-for-byte identical to the Go golden fixture
    //!      `providers-list.json` — same entries, same DECLARATION ORDER, same
    //!      field declaration order within each `ProviderInfo`, same 2-space
    //!      indent. This single assertion pins the whole contract (order +
    //!      `requires_key` + `capabilities` + `key_env_var` + `capability_auth`).
    //!
    //!  P2. **Catalog ordering.** The catalog names appear in exactly the Go
    //!      runner's assembly order:
    //!      `defillama, aave, morpho, kamino, moonwell, across, lifi, bungee,
    //!       1inch, uniswap, tempo, taikoswap, jupiter, bungee, fibrous`
    //!      (note: `bungee` appears twice — bridge mode then swap mode).
    //!
    //!  P3. **Key requirements per provider.** `requires_key` is `true` ONLY for
    //!      the key-gated swap providers `1inch` and `uniswap`; every other
    //!      entry (incl. `tempo`, `fibrous`, `jupiter`, both `bungee` modes,
    //!      `defillama`) is `requires_key == false`. (Mirrors the Go
    //!      `TestRunnerProvidersList` assertions on tempo/fibrous and the
    //!      key-gated route caveats.)
    //!
    //!  P4. **Exactly one jupiter entry.** `jupiter` appears exactly once
    //!      (Go `TestRunnerProvidersList` asserts `jupiterCount == 1`).
    //!
    //!  P5. **Two bungee entries, one per mode.** `bungee` appears exactly twice:
    //!      once with `type == "bridge"` (capability `bridge.quote`) and once
    //!      with `type == "swap"` (capability `swap.quote`). Both carry the
    //!      optional dedicated-backend `capability_auth` (API key + affiliate)
    //!      and remain `requires_key == false`.
    //!
    //!  P6. **No provider keys / network required.** Building the catalog and the
    //!      envelope must NOT require any `DEFI_*` env var or any I/O — this is a
    //!      pure metadata command (`providers list` is callable without keys).
    //!
    //!  P7. **`list` envelope shape.** [`list`] returns a SUCCESS envelope
    //!      (`success == true`, `error == None`) with:
    //!        * `meta.command == "providers list"`,
    //!        * `meta.cache.status == "bypass"`, `meta.cache.age_ms == 0`,
    //!          `meta.cache.stale == false` (metadata command bypasses cache —
    //!          `TestRunnerProvidersListBypassesCacheOpen`),
    //!        * `data` equal to the serialized [`provider_catalog`],
    //!        * `version == "v1"`,
    //!        * no provider statuses and `partial == false`.
    //!
    //!  P8. **Envelope JSON field order.** Rendering the `list` envelope as
    //!      canonical pretty JSON preserves top-level field DECLARATION order
    //!      (`version, success, data, error, warnings, meta`), NOT alphabetical —
    //!      with `warnings` omitted when empty.
    //!
    //! Go tests intentionally SKIPPED as owned elsewhere / internal-detail:
    //!   * Cache-open bypass *mechanics* (`setUnopenableCacheEnv`) — the runner's
    //!     `should_open_cache` routing is owned + tested in `defi-app::runner`.
    //!     Here we only assert the *observable* contract (`cache.status ==
    //!     "bypass"`), which is what the Go bypass test ultimately proves.
    //!   * `findProviderInfo` helper plumbing — a Go test fixture detail.
    //!   * `--results-only` / `--select` projection — owned by `defi-out`; the
    //!     golden `--results-only` fixture is reused here only as the catalog
    //!     oracle (the `data` array), not to test projection.

    use super::*;
    use serde_json::Value;

    const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden");

    fn load_golden(slug: &str) -> String {
        let path = format!("{GOLDEN_DIR}/{slug}.json");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read golden {path}: {e}"))
    }

    /// Serialize the catalog to a `serde_json::Value` (the form placed into the
    /// envelope `data`), for structural + ordering comparisons.
    fn catalog_value() -> Value {
        serde_json::to_value(provider_catalog()).expect("serialize catalog")
    }

    fn names() -> Vec<String> {
        provider_catalog().into_iter().map(|p| p.name).collect()
    }

    fn entries_named<'a>(catalog: &'a [ProviderInfo], name: &str) -> Vec<&'a ProviderInfo> {
        catalog.iter().filter(|p| p.name == name).collect()
    }

    // ----- P1: catalog parity with the Go golden fixture ------------------
    #[test]
    fn catalog_matches_go_golden_structurally() {
        let go: Value = serde_json::from_str(&load_golden("providers-list")).expect("go json");
        assert_eq!(
            catalog_value(),
            go,
            "provider_catalog must equal the Go golden providers-list array"
        );
    }

    #[test]
    fn catalog_renders_byte_identical_to_go_golden() {
        // The golden file is exactly the Go binary's `--results-only` stdout: a
        // 2-space-indent JSON array. Rendering the catalog with the same settings
        // (`to_string_pretty`) must reproduce it byte-for-byte (trailing newline
        // is the CLI's print, not the JSON body — compare trimmed bodies).
        let rust = serde_json::to_string_pretty(&provider_catalog()).expect("render catalog");
        let go = load_golden("providers-list");
        assert_eq!(
            rust.trim_end(),
            go.trim_end(),
            "catalog pretty-JSON must match Go golden byte-for-byte"
        );
    }

    // ----- P2: declaration order ------------------------------------------
    #[test]
    fn catalog_order_matches_go_runner_assembly() {
        assert_eq!(
            names(),
            vec![
                "defillama",
                "aave",
                "morpho",
                "kamino",
                "moonwell",
                "across",
                "lifi",
                "bungee",
                "1inch",
                "uniswap",
                "tempo",
                "taikoswap",
                "jupiter",
                "bungee",
                "fibrous",
            ],
        );
    }

    // ----- P3: key requirements -------------------------------------------
    #[test]
    fn only_oneinch_and_uniswap_require_keys() {
        let catalog = provider_catalog();
        for p in &catalog {
            let expected = p.name == "1inch" || p.name == "uniswap";
            assert_eq!(
                p.requires_key, expected,
                "provider {} requires_key should be {expected}",
                p.name
            );
        }
    }

    #[test]
    fn tempo_and_fibrous_do_not_require_keys() {
        let catalog = provider_catalog();
        for name in ["tempo", "fibrous"] {
            let info = entries_named(&catalog, name);
            assert_eq!(info.len(), 1, "expected exactly one {name} entry");
            assert!(!info[0].requires_key, "{name} requires_key should be false",);
        }
    }

    // ----- P4: exactly one jupiter ----------------------------------------
    #[test]
    fn exactly_one_jupiter_entry() {
        let catalog = provider_catalog();
        assert_eq!(
            entries_named(&catalog, "jupiter").len(),
            1,
            "expected exactly one jupiter provider entry",
        );
    }

    // ----- P5: two bungee entries (bridge + swap) -------------------------
    #[test]
    fn two_bungee_entries_one_per_mode() {
        let catalog = provider_catalog();
        let bungee = entries_named(&catalog, "bungee");
        assert_eq!(bungee.len(), 2, "expected exactly two bungee entries");

        let bridge = bungee
            .iter()
            .find(|p| p.provider_type == "bridge")
            .expect("bungee bridge-mode entry");
        let swap = bungee
            .iter()
            .find(|p| p.provider_type == "swap")
            .expect("bungee swap-mode entry");

        assert_eq!(bridge.capabilities, vec!["bridge.quote".to_string()]);
        assert_eq!(swap.capabilities, vec!["swap.quote".to_string()]);
        assert!(!bridge.requires_key);
        assert!(!swap.requires_key);

        // Both modes advertise the optional dedicated-backend auth pair
        // (API key + affiliate).
        for entry in [bridge, swap] {
            assert_eq!(
                entry.capability_auth.len(),
                2,
                "bungee {} should carry 2 capability_auth entries",
                entry.provider_type
            );
            let env_vars: Vec<&str> = entry
                .capability_auth
                .iter()
                .map(|a| a.key_env_var.as_str())
                .collect();
            assert!(env_vars.contains(&"DEFI_BUNGEE_API_KEY"));
            assert!(env_vars.contains(&"DEFI_BUNGEE_AFFILIATE"));
        }
    }

    // ----- P6: no keys / no network ---------------------------------------
    #[test]
    fn catalog_builds_without_provider_keys() {
        // Clear every gated key env var: the catalog (metadata) must still build
        // and still report the same key requirements.
        for var in [
            "DEFI_DEFILLAMA_API_KEY",
            "DEFI_1INCH_API_KEY",
            "DEFI_UNISWAP_API_KEY",
            "DEFI_JUPITER_API_KEY",
            "DEFI_BUNGEE_API_KEY",
            "DEFI_BUNGEE_AFFILIATE",
        ] {
            std::env::remove_var(var);
        }
        let catalog = provider_catalog();
        assert!(
            !catalog.is_empty(),
            "catalog must be non-empty without keys"
        );
        // 1inch/uniswap still advertise requires_key even with no key present:
        // `providers list` is metadata, not a liveness check.
        let oneinch = entries_named(&catalog, "1inch");
        assert_eq!(oneinch.len(), 1);
        assert!(oneinch[0].requires_key);
    }

    // ----- P7: list envelope shape ----------------------------------------
    #[test]
    fn list_returns_bypass_success_envelope() {
        let env = list();
        assert!(env.success, "providers list is a success envelope");
        assert!(env.error.is_none(), "success envelope has no error");
        assert_eq!(env.version, "v1");
        assert_eq!(env.meta.command, "providers list");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);
        assert!(env.meta.providers.is_empty());
        assert!(!env.meta.partial);
        assert!(env.warnings.is_empty());

        // data equals the serialized catalog.
        let data = env.data.as_ref().expect("data present");
        assert_eq!(data, &catalog_value());
    }

    // ----- P8: envelope JSON field declaration order ----------------------
    #[test]
    fn list_envelope_preserves_top_level_field_order() {
        let env = list();
        let rendered = env.to_pretty_json().expect("render envelope");
        let value: Value = serde_json::from_str(&rendered).expect("parse rendered");
        let keys: Vec<&str> = value
            .as_object()
            .expect("envelope is an object")
            .keys()
            .map(String::as_str)
            .collect();
        // Declaration order (NOT alphabetical); `warnings` omitted when empty.
        assert_eq!(keys, vec!["version", "success", "data", "error", "meta"]);
    }
}
