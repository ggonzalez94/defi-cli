//! # Success criteria — `defi-cli` assembled-binary stream/parity smoke
//!
//! Companion to `exit_codes.rs`. Where that file pins the i32 -> process-status
//! cast, this file proves the **assembled `defi` executable** (the thing the L6
//! crate actually produces, located via `CARGO_BIN_EXE_defi`) wires the runner
//! to the real stdio + exit-status boundary.
//!
//! The exhaustive per-command golden parity lives in `defi-app`'s
//! `tests/golden_cli.rs`; here we assert only the binary-level invariants the
//! thin shim is responsible for, sanity-checked against the same Go golden
//! fixtures so a regression in assembly (wrong stream, swallowed error, dropped
//! body) is caught at L6:
//!
//!  B1. **Success → stdout, nothing on stderr, exit 0.** A deterministic offline
//!      command prints its body to stdout, leaves stderr empty, exits 0.
//!  B2. **`--results-only` body is byte-exact** to the Go golden (no volatile
//!      fields in the projected array/object), proving the assembled binary does
//!      not re-wrap or reorder the data.
//!  B3. **Error → full envelope on STDERR, exit 2, stdout empty.** Errors are
//!      written to stderr (never stdout) and are ALWAYS the full envelope even
//!      under `--results-only` (spec §2.1 / §2.3). The error body is valid JSON
//!      with `success=false`, `error.code=2`, `error.type="usage_error"`,
//!      `version="v1"`, `data=[]`, `meta.cache.status="bypass"`.
//!  B4. **`--results-only` is ignored on error.** The error envelope under
//!      `--results-only` is structurally identical to the non-results-only error
//!      (the stable invariant the two `error-usage-missing-asset*` fixtures
//!      encode).
//!  B5. **`version` is a bare line, not JSON, exit 0, stdout only.** The
//!      `version` command bypasses the envelope entirely.
//!  B6. **`completion <shell>` is a bare completion script.** Completion
//!      generation is clap-native output, not the JSON envelope, and must stay
//!      wired because the machine-readable schema advertises those leaves.

use std::process::Command;

use serde_json::Value;

fn defi_bin() -> &'static str {
    env!("CARGO_BIN_EXE_defi")
}

fn golden(slug: &str) -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden");
    std::fs::read_to_string(format!("{path}/{slug}.json"))
        .unwrap_or_else(|e| panic!("read golden {slug}: {e}"))
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str]) -> Run {
    let out = Command::new(defi_bin())
        .args(args)
        .env_clear()
        .env("HOME", std::env::temp_dir())
        .output()
        .expect("run assembled `defi` binary");
    Run {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Blank the documented volatile envelope fields (golden/README.md) so two
/// captures of the same command compare equal.
fn normalize(v: &mut Value) {
    if let Some(Value::Object(meta)) = v.get_mut("meta") {
        for k in ["request_id", "timestamp"] {
            if meta.contains_key(k) {
                meta.insert(k.into(), Value::from(format!("<{k}>")));
            }
        }
        if let Some(Value::Object(cache)) = meta.get_mut("cache") {
            if cache.contains_key("age_ms") {
                cache.insert("age_ms".into(), Value::from(0));
            }
        }
    }
}

// ----- B1 + B2 -------------------------------------------------------------

#[test]
fn success_goes_to_stdout_exit_zero() {
    let r = run(&["providers", "list", "--results-only"]);
    assert_eq!(r.code, Some(0));
    assert!(r.stderr.is_empty(), "success must write nothing to stderr");
    assert!(!r.stdout.is_empty(), "success body must be on stdout");
}

#[test]
fn results_only_body_is_byte_exact_to_go_golden() {
    // B2: no volatile fields in a results-only array → byte-for-byte parity,
    // proving the assembled binary streams the data body unmodified.
    let r = run(&["providers", "list", "--results-only"]);
    assert_eq!(r.code, Some(0));
    assert_eq!(
        r.stdout,
        golden("providers-list"),
        "providers list --results-only must match the Go golden byte-for-byte"
    );
}

#[test]
fn results_only_object_is_byte_exact_to_go_golden() {
    let r = run(&[
        "assets",
        "resolve",
        "--symbol",
        "USDC",
        "--chain",
        "1",
        "--results-only",
    ]);
    assert_eq!(r.code, Some(0));
    assert_eq!(
        r.stdout,
        golden("assets-resolve-usdc-results-only"),
        "assets resolve --results-only must match the Go golden byte-for-byte"
    );
}

// ----- B3 ------------------------------------------------------------------

#[test]
fn error_full_envelope_on_stderr_exit_two() {
    let r = run(&["assets", "resolve", "--chain", "1"]);
    assert_eq!(r.code, Some(2));
    assert!(
        r.stdout.is_empty(),
        "error output must NOT go to stdout, got: {:?}",
        r.stdout
    );
    let v: Value = serde_json::from_str(&r.stderr).expect("error envelope JSON on stderr");
    assert_eq!(v["version"], Value::from("v1"));
    assert_eq!(v["success"], Value::Bool(false));
    assert_eq!(v["data"], Value::Array(vec![]));
    assert_eq!(v["error"]["code"], Value::from(2));
    assert_eq!(v["error"]["type"], Value::from("usage_error"));
    assert_eq!(v["meta"]["cache"]["status"], Value::from("bypass"));
    // The error body uses the JSON key `type`, never `error_type`.
    assert!(!r.stderr.contains("error_type"));
}

// ----- B4 ------------------------------------------------------------------

#[test]
fn results_only_is_ignored_on_error() {
    let with = run(&["assets", "resolve", "--chain", "1", "--results-only"]);
    let without = run(&["assets", "resolve", "--chain", "1"]);
    assert_eq!(with.code, Some(2));
    assert!(with.stdout.is_empty());

    let mut a: Value = serde_json::from_str(&with.stderr).expect("results-only error JSON");
    let mut b: Value = serde_json::from_str(&without.stderr).expect("error JSON");
    normalize(&mut a);
    normalize(&mut b);
    assert_eq!(
        a, b,
        "--results-only must be ignored on error: the error envelope is identical to the \
         non-results-only case (full envelope always)"
    );
}

// ----- B5 ------------------------------------------------------------------

#[test]
fn version_is_bare_line_not_json() {
    let r = run(&["version"]);
    assert_eq!(r.code, Some(0));
    assert!(r.stderr.is_empty());
    // Shape: `"<version>\n"`; the version tracks the crate version (README rule 1).
    assert_eq!(r.stdout, format!("{}\n", env!("CARGO_PKG_VERSION")));
    assert!(
        serde_json::from_str::<Value>(r.stdout.trim_end()).is_err(),
        "version output must NOT be JSON"
    );
}

#[test]
fn version_long_is_bare_line() {
    let r = run(&["version", "--long"]);
    assert_eq!(r.code, Some(0));
    // Default (un-instrumented) build → commit/built are "unknown" like Go.
    assert_eq!(
        r.stdout,
        format!(
            "{} (commit: unknown, built: unknown)\n",
            env!("CARGO_PKG_VERSION")
        )
    );
}

// ----- B6 ------------------------------------------------------------------

#[test]
fn completion_bash_is_bare_script() {
    let r = run(&["completion", "bash"]);
    assert_eq!(r.code, Some(0));
    assert!(r.stderr.is_empty());
    assert!(
        r.stdout.contains("_defi") && r.stdout.contains("complete -F"),
        "bash completion should be a shell script on stdout, got: {:?}",
        r.stdout
    );
    assert!(
        serde_json::from_str::<Value>(r.stdout.trim_end()).is_err(),
        "completion output must NOT be JSON"
    );
}

#[test]
fn completion_bash_tolerates_broken_pipe() {
    let script = format!(
        "set -o pipefail\n\"{}\" completion bash | head -n 1 >/dev/null",
        defi_bin()
    );
    let out = Command::new("/bin/bash")
        .arg("-lc")
        .arg(script)
        .env_clear()
        .env("HOME", std::env::temp_dir())
        .output()
        .expect("run completion pipeline");
    assert!(
        out.status.success(),
        "completion should exit cleanly when the reader closes early; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
