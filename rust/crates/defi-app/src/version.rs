//! `version` command group handler.
//!
//! Go source: `internal/app/runner.go::newVersionCommand` plus the
//! `internal/version` package (`CLIName`, `CLIVersion`, `Commit`, `BuildDate`,
//! `Long`). This is the simplest command surface: it does **not** emit the JSON
//! envelope at all — it prints a bare line of plain text to stdout and exits 0.
//!
//! Two forms (mirrors the Go `--long` flag):
//!
//!   * `defi version` → `CLIVersion` (e.g. `"0.5.0"`), captured byte-for-byte in
//!     the golden fixture `rust/tests/golden/version.json`;
//!   * `defi version --long` → `"<version> (commit: <commit>, built: <date>)"`,
//!     captured in `rust/tests/golden/version-long.json`.
//!
//! This module owns the contract-bearing surface (the exact output strings +
//! the build-info constants). The CLI version is sourced from the crate's
//! `CARGO_PKG_VERSION` so it stays in lockstep with the workspace version
//! (`0.5.0`) — matching the Go `version.CLIVersion`. Build metadata
//! (`commit`/`built`) defaults to `"unknown"` like the Go reference, and can be
//! injected at compile time via the `DEFI_BUILD_COMMIT` / `DEFI_BUILD_DATE`
//! environment variables (the Rust analogue of Go's `-ldflags` overrides).

/// The CLI binary name (mirrors Go `version.CLIName`).
pub const CLI_NAME: &str = "defi";

/// The CLI semantic version, sourced from the crate version so it tracks the
/// workspace version (`0.5.0`) and the Go `version.CLIVersion`.
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The build commit hash. Defaults to `"unknown"` (matching Go), overridable at
/// compile time via `DEFI_BUILD_COMMIT` (the Rust analogue of Go `-ldflags`).
pub const COMMIT: &str = match option_env!("DEFI_BUILD_COMMIT") {
    Some(c) => c,
    None => "unknown",
};

/// The build date. Defaults to `"unknown"` (matching Go), overridable at compile
/// time via `DEFI_BUILD_DATE`.
pub const BUILD_DATE: &str = match option_env!("DEFI_BUILD_DATE") {
    Some(d) => d,
    None => "unknown",
};

/// The short `version` output: the bare CLI version string (Go
/// `version.CLIVersion`).
///
/// This is the line the `defi version` command prints (the runner appends the
/// trailing newline, matching Go's `fmt.Fprintln`).
pub fn short() -> String {
    CLI_VERSION.to_string()
}

/// The extended `version --long` output (Go `version.Long`):
/// `"<version> (commit: <commit>, built: <build-date>)"`.
pub fn long() -> String {
    format!("{CLI_VERSION} (commit: {COMMIT}, built: {BUILD_DATE})")
}

/// Render the `version` command output for the given `long` flag.
///
/// Returns the bare line (without a trailing newline); the caller prints it with
/// a newline. `long == false` → [`short`]; `long == true` → [`long`].
pub fn render(long: bool) -> String {
    if long {
        self::long()
    } else {
        short()
    }
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::version` (Go: `internal/version` +
    //! `internal/app/runner.go::newVersionCommand`)
    //!
    //! `version` is a deterministic, offline, **metadata-only** command that
    //! prints a single plain-text line (NOT a JSON envelope) and exits 0. Its
    //! output is a primary success oracle captured in the golden fixtures
    //! `rust/tests/golden/version.json` and `version-long.json`. The Rust port is
    //! "correct" iff:
    //!
    //!  V1. **Short form (golden).** `defi version` prints exactly the CLI version
    //!      string. The fixture body (sans trailing newline) is `"0.5.0"`.
    //!  V2. **Long form (golden).** `defi version --long` prints
    //!      `"<version> (commit: <commit>, built: <date>)"`. With the default
    //!      (un-injected) build metadata this is exactly
    //!      `"0.5.0 (commit: unknown, built: unknown)"`, matching the Go binary's
    //!      `version --long` output captured in `version-long.json`.
    //!  V3. **Version tracks the workspace version.** `CLI_VERSION` equals the
    //!      crate's `CARGO_PKG_VERSION` (`0.5.0`), keeping the Rust port in
    //!      lockstep with the Go `version.CLIVersion` without a hand-maintained
    //!      constant.
    //!  V4. **`render` dispatches on the `long` flag.** `render(false) == short()`
    //!      and `render(true) == long()`.
    //!  V5. **No envelope / no I/O / no keys.** The output is bare plain text —
    //!      it is not valid envelope JSON, requires no env vars, and performs no
    //!      I/O. (`version` is in the cache-bypass metadata set.)
    //!
    //! Skipped (owned elsewhere): the cache-bypass routing predicate is owned +
    //! tested in `defi-app::runner` (`should_open_cache("version") == false`); we
    //! add one confirmation here for the `version` path.

    use super::*;

    const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden");

    fn load_golden(slug: &str) -> String {
        let path = format!("{GOLDEN_DIR}/{slug}.json");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read golden {path}: {e}"))
    }

    // ----- V1: short form matches the Go golden ---------------------------
    #[test]
    fn short_matches_go_golden() {
        let golden = load_golden("version");
        assert_eq!(
            short(),
            golden.trim_end(),
            "version short output must match the Go golden byte-for-byte"
        );
        assert_eq!(short(), "0.5.0");
    }

    // ----- V2: long form matches the Go golden ----------------------------
    #[test]
    fn long_matches_go_golden_with_default_build_metadata() {
        // The golden was captured from a `go build` with no `-ldflags`, so commit
        // and build date are the Go defaults (`"unknown"`). The Rust defaults are
        // identical unless DEFI_BUILD_* were injected; only assert byte parity in
        // the (default) un-injected case so an instrumented build does not fail.
        if COMMIT == "unknown" && BUILD_DATE == "unknown" {
            let golden = load_golden("version-long");
            assert_eq!(
                long(),
                golden.trim_end(),
                "version --long output must match the Go golden byte-for-byte"
            );
            assert_eq!(long(), "0.5.0 (commit: unknown, built: unknown)");
        }
        // The long form always embeds the short version and the labelled
        // commit/build metadata, regardless of injection.
        assert!(long().starts_with(CLI_VERSION));
        assert!(long().contains(&format!("commit: {COMMIT}")));
        assert!(long().contains(&format!("built: {BUILD_DATE}")));
    }

    // ----- V3: version tracks the crate/workspace version -----------------
    #[test]
    fn cli_version_tracks_crate_version() {
        assert_eq!(CLI_VERSION, env!("CARGO_PKG_VERSION"));
        assert_eq!(CLI_NAME, "defi");
    }

    // ----- V4: render dispatches on the long flag -------------------------
    #[test]
    fn render_dispatches_on_long_flag() {
        assert_eq!(render(false), short());
        assert_eq!(render(true), long());
        assert_ne!(render(false), render(true));
    }

    // ----- V5: output is bare plain text, not envelope JSON ---------------
    #[test]
    fn output_is_plain_text_not_envelope_json() {
        // Neither form is a JSON object (the version command bypasses the
        // envelope entirely).
        assert!(serde_json::from_str::<serde_json::Value>(&short()).is_err());
        let parsed = serde_json::from_str::<serde_json::Value>(&long());
        assert!(
            parsed.is_err(),
            "long form must not be a JSON value, got: {parsed:?}"
        );
    }

    // ----- cache-bypass confirmation --------------------------------------
    #[test]
    fn version_bypasses_cache() {
        assert!(
            !crate::runner::should_open_cache("version"),
            "version must bypass cache"
        );
    }
}
