//! `assets` command group handler.
//!
//! Go source: `internal/app/runner.go::newAssetsCommand` (the `assets resolve`
//! subcommand). `assets resolve` is a deterministic, offline, **metadata-only**
//! command: it resolves an asset symbol / token address / CAIP-19 against the
//! bootstrap token registry on a given chain, and emits an
//! [`defi_model::AssetResolution`] as the `data` of a success envelope
//! (`cache.status == "bypass"`).
//!
//! It is one of the deterministic offline commands covered by the Go golden
//! fixtures `rust/tests/golden/assets-resolve-usdc*.json` plus the two
//! `error-usage-*` fixtures that pin the usage-error messages.
//!
//! This module owns the **command-layer composition**: input precedence,
//! the exact usage-error ordering/messages, and the `AssetResolution` field
//! values. The lower-level chain/asset parsing is owned + tested in
//! [`defi_id`] (`parse_chain` / `parse_asset`).

use defi_errors::{Code, Error};
use defi_id::{parse_asset, parse_chain};
use defi_model::{AssetResolution, CacheStatus, Envelope};

/// Resolve `--symbol`/`--asset` on `--chain` to a canonical [`AssetResolution`].
///
/// Mirrors the Go `newAssetsCommand` `resolve` `RunE`, preserving the exact
/// validation ORDER and messages (which the `error-usage-*` golden fixtures
/// pin):
///
/// 1. `--chain` is required → [`Code::Usage`] `"--chain is required"`.
/// 2. The asset value is `input` (the `--asset` flag) when set, else `symbol`
///    (the `--symbol` flag); if both are empty →
///    [`Code::Usage`] `"--asset or --symbol is required"`.
/// 3. The chain is parsed (`defi_id::parse_chain`); an unsupported chain input
///    surfaces that parser's usage error
///    (`"unsupported chain input: <arg>"`).
/// 4. The asset is parsed (`defi_id::parse_asset`); unknown/ambiguous symbols
///    and malformed CAIP-19 surface that parser's error.
///
/// On success the result's `resolved_by` is the constant `"registry"` and
/// `unambiguous` is `true` (mirroring the Go construction site). `Input` is the
/// raw resolved value (the `--asset` value when set, else the `--symbol`
/// value), NOT the canonical symbol.
pub fn resolve(chain_arg: &str, symbol: &str, asset: &str) -> Result<AssetResolution, Error> {
    if chain_arg.is_empty() {
        return Err(Error::new(Code::Usage, "--chain is required"));
    }
    // `--asset` (CAIP-19/address) takes precedence over `--symbol` (Go uses
    // `value := input; if value == "" { value = symbol }`).
    let value = if !asset.is_empty() { asset } else { symbol };
    if value.is_empty() {
        return Err(Error::new(Code::Usage, "--asset or --symbol is required"));
    }

    let chain = parse_chain(chain_arg)?;
    let resolved = parse_asset(value, &chain)?;

    Ok(AssetResolution {
        input: value.to_string(),
        chain_id: chain.caip2.clone(),
        symbol: resolved.symbol,
        asset_id: resolved.asset_id,
        address: resolved.address,
        decimals: resolved.decimals as i64,
        resolved_by: "registry".to_string(),
        unambiguous: true,
    })
}

/// Build the `assets resolve` success envelope (cache bypassed).
///
/// Mirrors the Go handler tail: `emitSuccess("assets resolve", result, nil,
/// cacheMetaBypass(), nil, false)` — `meta.command == "assets resolve"`,
/// `cache.status == "bypass"`, no providers/warnings, `partial == false`, and
/// `data` is the serialized [`AssetResolution`].
pub fn run(chain_arg: &str, symbol: &str, asset: &str) -> Result<Envelope, Error> {
    let resolution = resolve(chain_arg, symbol, asset)?;
    let data = serde_json::to_value(&resolution)
        .map_err(|e| Error::wrap(Code::Internal, "serialize asset resolution", e))?;
    Ok(Envelope::success(
        "assets resolve",
        data,
        Vec::new(),
        CacheStatus::bypass(),
        Vec::new(),
        false,
    ))
}

/// clap parsing + handler for the `assets` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::Error;
    use defi_model::Envelope;

    use crate::ctx::AppCtx;

    /// `assets` subcommands (Go `newAssetsCommand`).
    #[derive(Subcommand, Debug)]
    pub enum AssetsCmd {
        /// Resolve an asset symbol/address/CAIP-19 to canonical asset ID.
        Resolve(ResolveArgs),
    }

    impl AssetsCmd {
        /// The leaf path token (for `meta.command`).
        pub fn path(&self) -> &'static str {
            match self {
                AssetsCmd::Resolve(_) => "resolve",
            }
        }
    }

    /// `assets resolve` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct ResolveArgs {
        /// Chain identifier (CAIP-2, chain ID, or slug).
        #[arg(long)]
        pub chain: Option<String>,
        /// Asset symbol (e.g., USDC).
        #[arg(long)]
        pub symbol: Option<String>,
        /// Asset as CAIP-19 or token address.
        #[arg(long)]
        pub asset: Option<String>,
    }

    /// Handle `assets <sub>`.
    pub async fn handle(_ctx: &AppCtx, cmd: AssetsCmd) -> Result<Envelope, Error> {
        match cmd {
            AssetsCmd::Resolve(args) => super::run(
                args.chain.as_deref().unwrap_or_default(),
                args.symbol.as_deref().unwrap_or_default(),
                args.asset.as_deref().unwrap_or_default(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::assets` (Go: `newAssetsCommand`)
    //!
    //! `assets resolve` is deterministic, offline, key-free. The Rust port is
    //! "correct" iff it preserves the resolution contract + the exact
    //! usage-error ordering pinned by the Go golden fixtures:
    //!
    //!  A1. **Symbol resolution (golden).** `assets resolve --symbol USDC
    //!      --chain 1` yields the canonical `AssetResolution` in
    //!      `assets-resolve-usdc.json` (`chain_id=eip155:1`, `symbol=USDC`,
    //!      `asset_id=eip155:1/erc20:0x…eb48`, `address=0x…eb48`, `decimals=6`,
    //!      `resolved_by=registry`, `unambiguous=true`, `input=USDC`).
    //!  A2. **`--asset` precedence over `--symbol`.** When both are set the
    //!      `--asset` value is used as `input` (Go `value := input; if value ==
    //!      "" { value = symbol }`).
    //!  A3. **Chain-required first.** Missing `--chain` → usage error
    //!      `"--chain is required"` even when neither asset form is set
    //!      (validation order matches Go).
    //!  A4. **Asset-required second.** With a chain but no asset form → usage
    //!      error `"--asset or --symbol is required"` (pins
    //!      `error-usage-missing-asset.json`).
    //!  A5. **Bad chain → parser usage error.** `--chain notarealchain`
    //!      surfaces `"unsupported chain input: notarealchain"` (pins
    //!      `error-usage-bad-chain.json`).
    //!  A6. **Envelope shape.** [`run`] returns a success envelope with
    //!      `meta.command == "assets resolve"`, `cache.status == "bypass"`,
    //!      `version == "v1"`, no providers/warnings, `partial == false`, and
    //!      `data` equal to the serialized resolution — and the serialized
    //!      `data` matches the golden fixture's `data` byte-for-byte (field
    //!      declaration order).
    //!  A7. **Cache bypass** (metadata route): `assets resolve` bypasses the
    //!      cache (`runner::should_open_cache("assets resolve") == true` is the
    //!      data-command default, but the handler itself always bypasses via
    //!      `CacheStatus::bypass()`, matching the Go `cacheMetaBypass()` site).

    use super::*;
    use serde_json::Value;

    const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden");

    fn golden(slug: &str) -> Value {
        let path = format!("{GOLDEN_DIR}/{slug}.json");
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read golden {path}: {e}"));
        serde_json::from_str(&raw).expect("parse golden json")
    }

    // ----- A1 + A6: symbol resolution matches the golden data -------------
    #[test]
    fn resolve_usdc_matches_go_golden_data() {
        let env = run("1", "USDC", "").expect("resolve USDC");
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert_eq!(env.meta.command, "assets resolve");
        assert_eq!(env.meta.cache.status, "bypass");
        assert!(env.meta.providers.is_empty());
        assert!(env.warnings.is_empty());
        assert!(!env.meta.partial);

        let full = golden("assets-resolve-usdc");
        let want_data = full.get("data").expect("golden data");
        let got_data = env.data.as_ref().expect("data present");
        assert_eq!(
            got_data, want_data,
            "assets resolve `data` must match the Go golden envelope byte-for-byte"
        );
        // Also matches the results-only fixture (data object only).
        let want_results_only = golden("assets-resolve-usdc-results-only");
        assert_eq!(got_data, &want_results_only);
    }

    // ----- A2: --asset precedence ----------------------------------------
    #[test]
    fn asset_flag_takes_precedence_over_symbol() {
        // Pass the canonical USDC address via --asset plus a bogus --symbol; the
        // --asset value wins and `input` echoes it verbatim.
        let res = resolve(
            "1",
            "WRONGSYMBOL",
            "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
        )
        .expect("resolve by address");
        assert_eq!(res.input, "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");
        assert_eq!(res.symbol, "USDC");
        assert_eq!(
            res.asset_id,
            "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
        );
    }

    // ----- A3: chain required first --------------------------------------
    #[test]
    fn missing_chain_is_usage_error_before_asset_check() {
        // Neither chain nor asset set: chain check fires FIRST.
        let err = resolve("", "", "").expect_err("missing chain");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(err.message, "--chain is required");
    }

    // ----- A4: asset required second (golden message) --------------------
    #[test]
    fn missing_asset_is_usage_error_matching_golden() {
        let err = resolve("1", "", "").expect_err("missing asset");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(err.message, "--asset or --symbol is required");

        let full = golden("error-usage-missing-asset");
        assert_eq!(full["error"]["code"], Value::from(Code::Usage.as_i32()));
        assert_eq!(full["error"]["type"], Value::from("usage_error"));
        assert_eq!(full["error"]["message"], Value::from(err.message.as_str()));
    }

    // ----- A5: bad chain → parser usage error (golden message) -----------
    #[test]
    fn bad_chain_surfaces_parser_usage_error_matching_golden() {
        let err = resolve("notarealchain", "USDC", "").expect_err("bad chain");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(err.message, "unsupported chain input: notarealchain");

        let full = golden("error-usage-bad-chain");
        assert_eq!(full["error"]["message"], Value::from(err.message.as_str()));
    }
}
