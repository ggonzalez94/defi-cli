//! End-to-end golden CLI parity tests (the **primary success oracle**).
//!
//! These run the assembled `defi` binary (built on demand from the sibling
//! `defi-cli` package, then invoked as a subprocess) for every
//! deterministic, offline command that has a captured Go golden fixture under
//! `rust/tests/golden/`, and assert the produced output matches the Go capture
//! **byte-for-byte after the documented volatile-field normalization**
//! (`rust/tests/golden/README.md`): `meta.request_id`, `meta.timestamp`, and
//! `meta.cache.age_ms` are blanked to fixed sentinels on BOTH sides before
//! comparison; `*fetched_at*` and `meta.providers[].latency_ms` are normalized
//! too (none appear in these offline fixtures, but the normalizer is complete).
//!
//! Stream + exit-code contract asserted:
//!   * success output → **stdout**, exit 0;
//!   * error envelopes → **stderr**, exit 2 (usage), ALWAYS the full envelope
//!     even under `--results-only` (the two `error-usage-missing-asset*`
//!     fixtures are byte-identical, encoding that invariant).
//!
//! The `schema` command's whole-document byte parity (WS6) IS asserted here:
//! `defi schema` stdout must equal the full `schema.json` golden byte-for-byte
//! after normalizing only the two volatile envelope fields (`request_id`,
//! `timestamp`) at the string level. Per-node + scoped-subtree parity is also
//! covered by the `defi-app::schema` unit tests.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

/// Path to the captured golden fixtures.
fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("golden")
}

fn read_golden(slug: &str) -> String {
    let path = golden_dir().join(format!("{slug}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read golden {}: {e}", path.display()))
}

/// Resolve the path to the assembled `defi` binary, (re)building it first.
///
/// The `defi` binary is produced by the *sibling* `defi-cli` package, so
/// `CARGO_BIN_EXE_defi` is NOT set for this `defi-app` integration test, and
/// `cargo test -p defi-app` does not build reverse-dependencies. We derive the
/// target profile directory from this test executable's own path
/// (`.../target/<profile>/deps/golden_cli-<hash>`) and **always** run
/// `cargo build -p defi-cli` (matching profile) exactly once per test process
/// before locating the binary.
///
/// The rebuild is mandatory, not best-effort: a stale `defi` binary left over
/// from a previous build would let these parity tests pass against old code,
/// giving false confidence. Cargo's incremental build makes the no-op case
/// cheap. This works under both `debug` and `release`.
fn defi_bin() -> PathBuf {
    static BUILD: std::sync::Once = std::sync::Once::new();

    let test_exe = std::env::current_exe().expect("current_exe");
    // `.../<profile>/deps/<name>` → profile dir is the parent of `deps`.
    let profile_dir = test_exe
        .parent()
        .and_then(|deps| deps.parent())
        .expect("profile dir")
        .to_path_buf();
    let release = profile_dir
        .file_name()
        .map(|n| n == "release")
        .unwrap_or(false);

    BUILD.call_once(|| {
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args(["build", "-p", "defi-cli"]);
        if release {
            cmd.arg("--release");
        }
        let status = cmd.status().expect("spawn cargo build -p defi-cli");
        assert!(status.success(), "failed to build the `defi` binary");
    });

    let exe = if cfg!(windows) { "defi.exe" } else { "defi" };
    let bin = profile_dir.join(exe);
    assert!(
        bin.exists(),
        "the `defi` binary was not found at {} after building",
        bin.display()
    );
    bin
}

/// Run the `defi` binary with `args`, returning its captured output.
fn run(args: &[&str]) -> Output {
    let mut cmd = Command::new(defi_bin());
    cmd.args(args);
    // Keep the environment minimal + deterministic: no provider keys, a fixed
    // HOME so cache-path resolution never touches the real user config.
    cmd.env_clear();
    cmd.env("HOME", std::env::temp_dir());
    cmd.output().expect("run defi binary")
}

/// Recursively blank the volatile JSON fields described in the golden README so
/// two captures of the same command compare equal.
fn normalize(value: &mut Value) {
    if let Some(Value::Object(meta_map)) = value.get_mut("meta") {
        if meta_map.contains_key("request_id") {
            meta_map.insert("request_id".into(), Value::from("<request_id>"));
        }
        if meta_map.contains_key("timestamp") {
            meta_map.insert("timestamp".into(), Value::from("<timestamp>"));
        }
        if let Some(Value::Object(cache)) = meta_map.get_mut("cache") {
            if cache.contains_key("age_ms") {
                cache.insert("age_ms".into(), Value::from(0));
            }
        }
        if let Some(Value::Array(providers)) = meta_map.get_mut("providers") {
            for p in providers {
                if let Value::Object(pm) = p {
                    if pm.contains_key("latency_ms") {
                        pm.insert("latency_ms".into(), Value::from(0));
                    }
                }
            }
        }
    }
    normalize_fetched_at(value);
}

/// Recursively blank any object key matching `*fetched_at*` to a sentinel.
fn normalize_fetched_at(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if k.contains("fetched_at") {
                    *v = Value::from("<fetched_at>");
                } else {
                    normalize_fetched_at(v);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_fetched_at(item);
            }
        }
        _ => {}
    }
}

/// Parse, normalize, and compare two JSON documents for equality, panicking
/// with a readable diff on mismatch.
fn assert_json_parity(got: &str, golden: &str, ctx: &str) {
    let mut got_v: Value = serde_json::from_str(got)
        .unwrap_or_else(|e| panic!("{ctx}: parse produced JSON: {e}\n{got}"));
    let mut want_v: Value =
        serde_json::from_str(golden).unwrap_or_else(|e| panic!("{ctx}: parse golden JSON: {e}"));
    normalize(&mut got_v);
    normalize(&mut want_v);
    assert_eq!(
        got_v,
        want_v,
        "{ctx}: normalized JSON must match the Go golden\n--- got ---\n{}\n--- want ---\n{}",
        serde_json::to_string_pretty(&got_v).unwrap(),
        serde_json::to_string_pretty(&want_v).unwrap(),
    );
}

// ---------------------------------------------------------------------------
// version (raw string, NOT an envelope)
// ---------------------------------------------------------------------------

#[test]
fn version_short_matches_golden_shape() {
    let out = run(&["version"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stderr.is_empty(), "version writes nothing to stderr");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    // The golden documents the SHAPE (`"<version>\n"`); the version is
    // release-dependent, so assert against the crate version (per README rule 1).
    assert_eq!(stdout, format!("{}\n", env!("CARGO_PKG_VERSION")));
    // It is NOT JSON.
    assert!(serde_json::from_str::<Value>(stdout.trim_end()).is_err());
}

#[test]
fn version_long_matches_golden_shape() {
    let out = run(&["version", "--long"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    // Default (un-instrumented) build → commit/built are "unknown" like the Go
    // golden capture.
    assert_eq!(
        stdout,
        format!(
            "{} (commit: unknown, built: unknown)\n",
            env!("CARGO_PKG_VERSION")
        )
    );
}

// ---------------------------------------------------------------------------
// providers list --results-only (bare array, byte-exact, no volatile fields)
// ---------------------------------------------------------------------------

#[test]
fn providers_list_results_only_matches_golden_byte_for_byte() {
    let out = run(&["providers", "list", "--results-only"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stderr.is_empty());
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    // No volatile fields in the results-only array → byte-for-byte.
    assert_eq!(
        stdout,
        read_golden("providers-list"),
        "providers list --results-only must match the Go golden byte-for-byte"
    );
}

// ---------------------------------------------------------------------------
// chains list (+ --results-only)
// ---------------------------------------------------------------------------

#[test]
fn chains_list_results_only_matches_golden_byte_for_byte() {
    let out = run(&["chains", "list", "--results-only"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert_eq!(
        stdout,
        read_golden("chains-list-results-only"),
        "chains list --results-only must match the Go golden byte-for-byte"
    );
}

#[test]
fn chains_list_full_envelope_matches_golden_after_normalization() {
    let out = run(&["chains", "list"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert_json_parity(&stdout, &read_golden("chains-list"), "chains list");
}

// ---------------------------------------------------------------------------
// assets resolve (success + results-only)
// ---------------------------------------------------------------------------

#[test]
fn assets_resolve_full_envelope_matches_golden_after_normalization() {
    let out = run(&["assets", "resolve", "--symbol", "USDC", "--chain", "1"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stderr.is_empty(), "success goes to stdout, not stderr");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert_json_parity(
        &stdout,
        &read_golden("assets-resolve-usdc"),
        "assets resolve",
    );
}

#[test]
fn assets_resolve_results_only_matches_golden_byte_for_byte() {
    let out = run(&[
        "assets",
        "resolve",
        "--symbol",
        "USDC",
        "--chain",
        "1",
        "--results-only",
    ]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    // The data object carries no volatile fields → byte-for-byte.
    assert_eq!(
        stdout,
        read_golden("assets-resolve-usdc-results-only"),
        "assets resolve --results-only must match the Go golden byte-for-byte"
    );
}

// ---------------------------------------------------------------------------
// Error cases: full envelope on STDERR, exit 2, --results-only ignored.
// ---------------------------------------------------------------------------

#[test]
fn error_missing_asset_is_full_envelope_on_stderr_exit_2() {
    let out = run(&["assets", "resolve", "--chain", "1"]);
    assert_eq!(out.status.code(), Some(2), "usage error exits 2");
    assert!(
        out.stdout.is_empty(),
        "error output must NOT go to stdout (got: {:?})",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    assert_json_parity(
        &stderr,
        &read_golden("error-usage-missing-asset"),
        "error missing asset",
    );
}

#[test]
fn error_results_only_is_ignored_full_envelope_on_stderr() {
    // `--results-only` must be IGNORED on error: still the full envelope on
    // stderr, byte-identical (after normalization) to the non-results-only case.
    let out = run(&["assets", "resolve", "--chain", "1", "--results-only"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    assert_json_parity(
        &stderr,
        &read_golden("error-usage-missing-asset-results-only"),
        "error missing asset (results-only)",
    );
    // And the two error fixtures are byte-identical after normalization — proves
    // results-only is dropped on error.
    assert_json_parity(
        &stderr,
        &read_golden("error-usage-missing-asset"),
        "error results-only == error non-results-only",
    );
}

#[test]
fn error_bad_chain_is_usage_error_on_stderr_exit_2() {
    let out = run(&[
        "assets",
        "resolve",
        "--symbol",
        "USDC",
        "--chain",
        "notarealchain",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    assert_json_parity(
        &stderr,
        &read_golden("error-usage-bad-chain"),
        "error bad chain",
    );
}

// ---------------------------------------------------------------------------
// Contract invariants directly on rendered bytes.
// ---------------------------------------------------------------------------

#[test]
fn json_uses_two_space_indent_and_declaration_field_order() {
    let out = run(&["assets", "resolve", "--symbol", "USDC", "--chain", "1"]);
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    // 2-space indent: the `"success"` key sits at column 2.
    assert!(
        stdout.contains("\n  \"success\": true,"),
        "expected 2-space-indented `success` key, got:\n{stdout}"
    );
    // Declaration field order (NOT alphabetical): version < success < data <
    // error < meta, and within data: input < chain_id < symbol < asset_id.
    let pos = |needle: &str| {
        stdout
            .find(needle)
            .unwrap_or_else(|| panic!("missing {needle}"))
    };
    assert!(pos("\"version\"") < pos("\"success\""));
    assert!(pos("\"success\"") < pos("\"data\""));
    assert!(pos("\"data\"") < pos("\"error\""));
    assert!(pos("\"error\"") < pos("\"meta\""));
    assert!(pos("\"input\"") < pos("\"chain_id\""));
    assert!(pos("\"chain_id\"") < pos("\"symbol\""));
    assert!(pos("\"symbol\"") < pos("\"asset_id\""));
    // The `error` body uses the JSON key `type` (not `error_type`).
    let err_out = run(&["assets", "resolve", "--chain", "1"]);
    let stderr = String::from_utf8(err_out.stderr).expect("utf8");
    assert!(stderr.contains("\"type\": \"usage_error\""));
    assert!(!stderr.contains("error_type"));
}

// ---------------------------------------------------------------------------
// schema — whole-document byte parity (WS6).
// ---------------------------------------------------------------------------

/// String-level normalize the two volatile envelope fields so two captures of
/// the same envelope compare byte-for-byte. Operates on the raw rendered text
/// (NOT a parsed `Value`) so formatting/ordering differences are NOT masked.
fn normalize_volatile_lines(text: &str) -> String {
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("\"request_id\":") {
                let indent = &line[..line.len() - trimmed.len()];
                format!("{indent}\"request_id\": \"<request_id>\",")
            } else if trimmed.starts_with("\"timestamp\":") {
                let indent = &line[..line.len() - trimmed.len()];
                format!("{indent}\"timestamp\": \"<timestamp>\",")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn schema_whole_document_matches_golden_byte_for_byte() {
    let out = run(&["schema"]);
    assert_eq!(out.status.code(), Some(0), "schema exits 0");
    assert!(out.stderr.is_empty(), "schema writes nothing to stderr");
    let stdout = String::from_utf8(out.stdout).expect("utf8");

    let got = normalize_volatile_lines(stdout.trim_end_matches('\n'));
    let golden = read_golden("schema");
    let want = normalize_volatile_lines(golden.trim_end_matches('\n'));

    assert_eq!(
        got, want,
        "`defi schema` must match the full Go golden schema.json byte-for-byte \
         (after request_id/timestamp normalization)"
    );
}

#[test]
fn schema_scoped_path_matches_golden_subtree() {
    // A scoped path returns exactly that node's subtree as the envelope `data`.
    let out = run(&["schema", "lend", "supply", "plan"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    let v: Value = serde_json::from_str(&stdout).expect("schema envelope JSON");
    let data = &v["data"];
    assert_eq!(data["path"], Value::from("defi lend supply plan"));
    assert_eq!(data["use"], Value::from("plan"));
    assert_eq!(data["mutation"], Value::Bool(true));
    // Bypass cache (metadata command).
    assert_eq!(v["meta"]["cache"]["status"], Value::from("bypass"));
}

#[test]
fn schema_unknown_path_is_wrapped_usage_error_on_stderr() {
    let out = run(&["schema", "nope"]);
    assert_eq!(out.status.code(), Some(2), "unknown schema path exits 2");
    assert!(out.stdout.is_empty(), "error goes to stderr, not stdout");
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    let v: Value = serde_json::from_str(&stderr).expect("error envelope JSON");
    assert_eq!(v["success"], Value::Bool(false));
    assert_eq!(v["error"]["code"], Value::from(2));
    assert_eq!(v["error"]["type"], Value::from("usage_error"));
    assert_eq!(
        v["error"]["message"],
        Value::from("build schema: command not found: nope")
    );
}

#[test]
fn unknown_command_is_usage_error_exit_2() {
    let out = run(&["frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8(out.stderr).expect("utf8");
    let v: Value = serde_json::from_str(&stderr).expect("error envelope JSON");
    assert_eq!(v["success"], Value::Bool(false));
    assert_eq!(v["error"]["code"], Value::from(2));
    assert_eq!(v["error"]["type"], Value::from("usage_error"));
    // Full envelope shape on error.
    assert_eq!(v["version"], Value::from("v1"));
    assert_eq!(v["data"], Value::Array(vec![]));
    assert_eq!(v["meta"]["cache"]["status"], Value::from("bypass"));
}
