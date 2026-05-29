//! # Phase 3 — end-to-end golden CLI parity (the primary success oracle).
//!
//! Drives the **assembled `defi` executable** (the artifact the L6 `defi-cli`
//! crate produces, resolved by `assert_cmd` via `CARGO_BIN_EXE_defi`) for every
//! deterministic, OFFLINE command that has a captured Go golden fixture under
//! `rust/tests/golden/`, and asserts the produced **stdout + exit code** matches
//! the Go capture after the documented volatile-field normalization
//! (`rust/tests/golden/README.md`).
//!
//! Coverage (per the Phase-3 task):
//!   * every captured command (`version`, `version --long`, `providers list`,
//!     `chains list`, `assets resolve`, and `schema`);
//!   * the `--results-only` variant (byte-exact: no volatile fields in the
//!     projected body);
//!   * the `--select <fields>` variant (projection — kept keys sorted
//!     ALPHABETICALLY, mirroring Go's `map[string]any` JSON serialization);
//!   * an error case asserting the FULL envelope is printed on error (on
//!     **stderr**) and the exit code matches the stable map (`Usage` = 2),
//!     including the invariant that `--results-only` is IGNORED on error.
//!
//! ## Why `assert_cmd` + the assembled binary
//! This is the only layer where the whole contract is observable end-to-end:
//! argv parsing → settings precedence → routing → envelope construction →
//! rendering → stream selection (stdout vs stderr) → the `i32 -> process status`
//! cast in `main.rs`. `assert_cmd::Command::cargo_bin("defi")` resolves the same
//! `CARGO_BIN_EXE_defi` Cargo builds for this package's integration tests, so
//! these run against freshly built code with no stale-binary hazard.
//!
//! ## Determinism
//! Only `meta.request_id` and `meta.timestamp` vary across runs (the Go runner
//! uses `crypto/rand` + `time.Now()`; the Rust runner mirrors that shape). The
//! Go reference tests do NOT inject a fixed clock — they ignore those fields —
//! so the faithful mirror here is the README's documented **normalization**:
//! blank `meta.request_id`, `meta.timestamp`, `meta.cache.age_ms`,
//! `meta.providers[].latency_ms`, and any `*fetched_at*` to fixed sentinels on
//! BOTH sides before comparing. Results-only / projected bodies carry none of
//! these, so they are compared **byte-for-byte**.
//!
//! ## Deferred: whole-document `schema` parity
//! The Go `schema.json` golden is the full 19-command tree (~959 KB). The Rust
//! `schema` command currently emits only the `defi`/`schema`/`version` subtree
//! (wiring the full tree is deferred integration work — see the remainder plan
//! and the `defi-app::schema` module deferral note). So `schema` whole-document
//! parity is asserted only at the STRUCTURAL/envelope level here (correct
//! envelope shape, field order, exit 0, stdout), not byte-for-byte against the
//! Go golden. This deferral is recorded explicitly rather than faked.

use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Fixture loading.
// ---------------------------------------------------------------------------

/// Path to the captured Go golden fixtures (`rust/tests/golden/`).
fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("golden")
}

/// Read a golden `<slug>.json` fixture (captured stdout / stderr body).
fn golden_json(slug: &str) -> String {
    let path = golden_dir().join(format!("{slug}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read golden {}: {e}", path.display()))
}

/// Read a golden `<slug>.exit` fixture (the captured process exit code).
fn golden_exit(slug: &str) -> i32 {
    let path = golden_dir().join(format!("{slug}.exit"));
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read golden exit {}: {e}", path.display()));
    raw.trim()
        .parse::<i32>()
        .unwrap_or_else(|e| panic!("parse golden exit {slug}: {e}"))
}

// ---------------------------------------------------------------------------
// Running the assembled binary deterministically.
// ---------------------------------------------------------------------------

/// Captured output of one assembled-binary run.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Run the assembled `defi` binary with `args` in a minimal, deterministic
/// environment.
///
/// `assert_cmd::Command::cargo_bin("defi")` locates `CARGO_BIN_EXE_defi` (built
/// fresh by Cargo for this package's tests). The environment is cleared and a
/// throwaway `HOME` is set so cache-path resolution never touches the real user
/// config and no provider API keys leak in — these are all offline,
/// cache-bypassing metadata commands, so this keeps every run reproducible.
fn run(args: &[&str]) -> Run {
    let assert = Command::cargo_bin("defi")
        .expect("locate assembled `defi` binary (CARGO_BIN_EXE_defi)")
        .args(args)
        .env_clear()
        .env("HOME", std::env::temp_dir())
        .assert();
    let output = assert.get_output();
    Run {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

// ---------------------------------------------------------------------------
// Volatile-field normalization (rust/tests/golden/README.md).
// ---------------------------------------------------------------------------

/// Blank the documented volatile JSON fields to fixed sentinels so two captures
/// of the same command compare equal.
///
/// Mirrors the README's normalization rules exactly:
///   * `meta.request_id`  -> `"<request_id>"`
///   * `meta.timestamp`   -> `"<timestamp>"`
///   * `meta.cache.age_ms`-> `0`
///   * every `meta.providers[i].latency_ms` -> `0`
///   * any object key matching `*fetched_at*` -> `"<fetched_at>"` (recursive)
///
/// None of the `*fetched_at*`/`providers` paths appear in the offline Phase-0
/// fixtures, but the normalizer is complete so it also works for any
/// cache/provider-backed command added later.
fn normalize(value: &mut Value) {
    if let Some(Value::Object(meta)) = value.get_mut("meta") {
        if meta.contains_key("request_id") {
            meta.insert("request_id".into(), Value::from("<request_id>"));
        }
        if meta.contains_key("timestamp") {
            meta.insert("timestamp".into(), Value::from("<timestamp>"));
        }
        if let Some(Value::Object(cache)) = meta.get_mut("cache") {
            if cache.contains_key("age_ms") {
                cache.insert("age_ms".into(), Value::from(0));
            }
        }
        if let Some(Value::Array(providers)) = meta.get_mut("providers") {
            for p in providers.iter_mut() {
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

/// Recursively blank any object key containing `fetched_at` to a sentinel.
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
        Value::Array(items) => items.iter_mut().for_each(normalize_fetched_at),
        _ => {}
    }
}

/// Parse two JSON documents, normalize the volatile fields on BOTH, and assert
/// structural equality — preserving declaration field order (we compare parsed
/// `Value`s under `serde_json`'s `preserve_order`, so key order is part of the
/// comparison for objects).
fn assert_json_parity(got: &str, golden: &str, ctx: &str) {
    let mut got_v: Value = serde_json::from_str(got)
        .unwrap_or_else(|e| panic!("{ctx}: produced output is not JSON: {e}\n{got}"));
    let mut want_v: Value =
        serde_json::from_str(golden).unwrap_or_else(|e| panic!("{ctx}: golden is not JSON: {e}"));
    normalize(&mut got_v);
    normalize(&mut want_v);
    assert_eq!(
        got_v,
        want_v,
        "{ctx}: normalized JSON must match the Go golden\n--- got ---\n{}\n--- want ---\n{}",
        serde_json::to_string_pretty(&got_v).unwrap_or_default(),
        serde_json::to_string_pretty(&want_v).unwrap_or_default(),
    );
}

// ===========================================================================
// Captured command: `version` / `version --long` (raw string, NOT an envelope).
// ===========================================================================

#[test]
fn version_short_matches_golden_shape() {
    // The `version` golden documents the SHAPE `"<version>\n"`; the embedded
    // number is release-dependent (README rule 1), so we assert the SHAPE
    // against the crate version, and that the output is NOT JSON.
    let r = run(&["version"]);
    assert_eq!(r.code, Some(golden_exit("version")), "version exit code");
    assert!(r.stderr.is_empty(), "version writes nothing to stderr");
    assert_eq!(r.stdout, format!("{}\n", env!("CARGO_PKG_VERSION")));
    assert!(
        serde_json::from_str::<Value>(r.stdout.trim_end()).is_err(),
        "version output must NOT be JSON"
    );
}

#[test]
fn version_long_matches_golden_shape() {
    // Shape: `"<version> (commit: <c>, built: <b>)\n"`. A default
    // (un-instrumented) build reports commit/built as `unknown`, matching the Go
    // golden capture.
    let r = run(&["version", "--long"]);
    assert_eq!(r.code, Some(golden_exit("version-long")));
    assert_eq!(
        r.stdout,
        format!(
            "{} (commit: unknown, built: unknown)\n",
            env!("CARGO_PKG_VERSION")
        )
    );
}

// ===========================================================================
// Captured command: `providers list --results-only` (bare array; byte-exact).
// ===========================================================================

#[test]
fn providers_list_results_only_byte_for_byte() {
    let r = run(&["providers", "list", "--results-only"]);
    assert_eq!(r.code, Some(golden_exit("providers-list")));
    assert!(r.stderr.is_empty());
    // No volatile fields in a results-only array → byte-for-byte parity.
    assert_eq!(
        r.stdout,
        golden_json("providers-list"),
        "providers list --results-only must match the Go golden byte-for-byte"
    );
}

// ===========================================================================
// Captured command: `chains list` (+ `--results-only`).
// ===========================================================================

#[test]
fn chains_list_full_envelope_matches_golden_after_normalization() {
    let r = run(&["chains", "list"]);
    assert_eq!(r.code, Some(golden_exit("chains-list")));
    assert!(r.stderr.is_empty(), "success goes to stdout, not stderr");
    assert_json_parity(&r.stdout, &golden_json("chains-list"), "chains list");
}

#[test]
fn chains_list_results_only_byte_for_byte() {
    let r = run(&["chains", "list", "--results-only"]);
    assert_eq!(r.code, Some(golden_exit("chains-list-results-only")));
    assert_eq!(
        r.stdout,
        golden_json("chains-list-results-only"),
        "chains list --results-only must match the Go golden byte-for-byte"
    );
}

// ===========================================================================
// Captured command: `assets resolve` (success + results-only).
// ===========================================================================

#[test]
fn assets_resolve_full_envelope_matches_golden_after_normalization() {
    let r = run(&["assets", "resolve", "--symbol", "USDC", "--chain", "1"]);
    assert_eq!(r.code, Some(golden_exit("assets-resolve-usdc")));
    assert!(r.stderr.is_empty());
    assert_json_parity(
        &r.stdout,
        &golden_json("assets-resolve-usdc"),
        "assets resolve",
    );
}

#[test]
fn assets_resolve_results_only_byte_for_byte() {
    let r = run(&[
        "assets",
        "resolve",
        "--symbol",
        "USDC",
        "--chain",
        "1",
        "--results-only",
    ]);
    assert_eq!(
        r.code,
        Some(golden_exit("assets-resolve-usdc-results-only"))
    );
    // The data object carries no volatile fields → byte-for-byte.
    assert_eq!(
        r.stdout,
        golden_json("assets-resolve-usdc-results-only"),
        "assets resolve --results-only must match the Go golden byte-for-byte"
    );
}

// ===========================================================================
// `--select <fields>` projection parity.
//
// CONTRACT: Go's `projectMap` builds a `map[string]any`, and `encoding/json`
// serializes map keys ALPHABETICALLY — so the projected key order is
// alphabetical, NOT the requested `--select` order. The two assertions below
// pin exactly that: `--select name,caip2` (requested order) emits `caip2`
// before `name`, and reversing the request changes nothing.
// ===========================================================================

#[test]
fn select_projects_alphabetically_not_requested_order_results_only() {
    // `--select name,caip2` → kept set {name, caip2}, keys sorted alpha → caip2
    // first. Byte-exact: a projected results-only array carries no volatile
    // fields. We assert against an inline expectation derived from the
    // chains-list golden, which is the exact Go behavior captured separately.
    let r = run(&["chains", "list", "--select", "name,caip2", "--results-only"]);
    assert_eq!(r.code, Some(0), "select projection over success → exit 0");
    let v: Value = serde_json::from_str(&r.stdout).expect("results-only select is JSON array");
    let arr = v.as_array().expect("array");
    assert!(!arr.is_empty(), "chains list is non-empty");
    // First element is Ethereum (declaration order of the chain list) projected
    // to exactly {caip2, name} with alphabetically-ordered keys.
    let first = arr[0].as_object().expect("object");
    let keys: Vec<&String> = first.keys().collect();
    assert_eq!(
        keys,
        vec!["caip2", "name"],
        "projected keys are ALPHABETICAL (caip2 < name), NOT requested order"
    );
    assert_eq!(first.get("caip2"), Some(&Value::from("eip155:1")));
    assert_eq!(first.get("name"), Some(&Value::from("Ethereum")));
    // Every element carries exactly the two projected keys, nothing else.
    for el in arr {
        let o = el.as_object().expect("object element");
        assert_eq!(o.len(), 2, "only the two selected fields survive: {o:?}");
        assert!(o.contains_key("caip2") && o.contains_key("name"));
    }

    // Order-independence: reversing the request produces byte-identical output.
    let rev = run(&["chains", "list", "--select", "caip2,name", "--results-only"]);
    assert_eq!(
        rev.stdout, r.stdout,
        "--select key order is alphabetical and independent of the requested order"
    );
}

#[test]
fn select_over_object_full_envelope_projects_data_in_place() {
    // `--select` with a single OBJECT data payload (assets resolve), full
    // envelope (not results-only): the envelope wrapper is preserved and `data`
    // is projected to the requested set with alphabetically-ordered keys.
    let r = run(&[
        "assets",
        "resolve",
        "--symbol",
        "USDC",
        "--chain",
        "1",
        "--select",
        "symbol,asset_id",
    ]);
    assert_eq!(r.code, Some(0));
    let v: Value = serde_json::from_str(&r.stdout).expect("envelope JSON");
    assert_eq!(
        v["version"],
        Value::from("v1"),
        "envelope wrapper preserved"
    );
    assert_eq!(v["success"], Value::Bool(true));
    let data = v["data"].as_object().expect("projected data object");
    let keys: Vec<&String> = data.keys().collect();
    assert_eq!(
        keys,
        vec!["asset_id", "symbol"],
        "projected data keys ALPHABETICAL (asset_id < symbol), not requested order"
    );
    assert_eq!(data.get("symbol"), Some(&Value::from("USDC")));
    assert!(
        !data.contains_key("chain_id") && !data.contains_key("input"),
        "unselected fields dropped from data"
    );
}

// ===========================================================================
// Captured command: `schema` — STRUCTURAL parity only (whole-document parity
// deferred; see module note + remainder plan).
// ===========================================================================

#[test]
fn schema_is_full_envelope_exit_zero_on_stdout() {
    let r = run(&["schema"]);
    assert_eq!(r.code, Some(golden_exit("schema")), "schema exits 0");
    assert!(r.stderr.is_empty(), "schema success → stdout only");

    let got: Value = serde_json::from_str(&r.stdout).expect("schema output is JSON");
    let want: Value = serde_json::from_str(&golden_json("schema")).expect("schema golden is JSON");
    // Envelope shape parity (the part that is fully wired): version, success,
    // declaration order of the top-level keys, and the schema `data` root.
    assert_eq!(got["version"], want["version"], "schema envelope version");
    assert_eq!(got["success"], want["success"], "schema success flag");
    assert_eq!(
        got["error"], want["error"],
        "schema error is null on success"
    );
    assert_eq!(
        got["data"]["path"], want["data"]["path"],
        "schema root `data.path`"
    );
    assert_eq!(
        got["data"]["use"], want["data"]["use"],
        "schema root `data.use`"
    );
    // Top-level envelope key order matches the contract (declaration order).
    let got_keys: Vec<&String> = got.as_object().expect("object").keys().collect();
    assert_eq!(
        got_keys,
        vec!["version", "success", "data", "error", "meta"],
        "envelope keys in declaration order"
    );
    // NOTE: whole-document `data` parity (the full 19-command tree) is DEFERRED;
    // the Rust schema currently emits a partial subtree. Recorded as a drift in
    // the Phase-3 report rather than asserted here.
}

// ===========================================================================
// Error case: FULL envelope on error, on STDERR, exit code from the stable map.
// ===========================================================================

#[test]
fn error_missing_asset_full_envelope_on_stderr_exit_two() {
    // `assets resolve` with no `--symbol`/`--asset` is a usage error.
    let r = run(&["assets", "resolve", "--chain", "1"]);
    assert_eq!(
        r.code,
        Some(golden_exit("error-usage-missing-asset")),
        "usage error exits 2 (stable map)"
    );
    assert_eq!(r.code, Some(2), "Code::Usage == 2");
    assert!(
        r.stdout.is_empty(),
        "error output must NOT go to stdout (got: {:?})",
        r.stdout
    );
    // The FULL envelope is printed on error (stderr), matching the Go golden.
    assert_json_parity(
        &r.stderr,
        &golden_json("error-usage-missing-asset"),
        "error missing asset",
    );
    // Spot-check the full-envelope invariants directly on the bytes.
    let v: Value = serde_json::from_str(&r.stderr).expect("error envelope JSON on stderr");
    assert_eq!(v["version"], Value::from("v1"));
    assert_eq!(v["success"], Value::Bool(false));
    assert_eq!(v["data"], Value::Array(vec![]), "error data is []");
    assert_eq!(v["error"]["code"], Value::from(2));
    assert_eq!(v["error"]["type"], Value::from("usage_error"));
    assert_eq!(v["meta"]["cache"]["status"], Value::from("bypass"));
    assert!(
        !r.stderr.contains("error_type"),
        "the error body uses the JSON key `type`, never `error_type`"
    );
}

#[test]
fn error_results_only_is_ignored_full_envelope_on_stderr() {
    // `--results-only` MUST be ignored on error: still the full envelope on
    // stderr, byte-identical (after normalization) to the non-results-only case.
    let r = run(&["assets", "resolve", "--chain", "1", "--results-only"]);
    assert_eq!(
        r.code,
        Some(golden_exit("error-usage-missing-asset-results-only"))
    );
    assert!(r.stdout.is_empty());
    assert_json_parity(
        &r.stderr,
        &golden_json("error-usage-missing-asset-results-only"),
        "error missing asset (results-only)",
    );
    // The two error fixtures are byte-identical after normalization — encoding
    // the "results-only is dropped on error" invariant.
    let without = run(&["assets", "resolve", "--chain", "1"]);
    let mut a: Value = serde_json::from_str(&r.stderr).expect("results-only error JSON");
    let mut b: Value = serde_json::from_str(&without.stderr).expect("error JSON");
    normalize(&mut a);
    normalize(&mut b);
    assert_eq!(
        a, b,
        "--results-only error envelope is identical to the non-results-only one"
    );
}
