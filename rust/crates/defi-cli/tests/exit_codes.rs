//! # Success criteria — `defi-cli` (L6 thin binary)
//!
//! Its ENTIRE job — and therefore the only contract this crate owns — is to
//! faithfully translate the `int` the runner returns into the **OS process exit
//! status**, unmangled, and to assemble into a real `defi` executable. The
//! per-command output contract (envelope shape, JSON declaration order, plain
//! key-sort, projection, golden parity) is owned and exhaustively tested by the
//! `defi-app` (L5) crate; this crate does NOT re-test that surface. What it
//! adds — and what nothing below L6 can prove — is the **exit-code fidelity
//! across the process boundary**. The Rust port is "correct" iff:
//!
//!  E1. **Every stable exit code survives the cast.** The Rust `main` does
//!      `ExitCode::from(code as u8)`. Every code in the contract map
//!      (`defi_errors::Code::ALL` = {0,1,2,10,11,12,13,14,15,16,20,21,22,23,24},
//!      spec §2.2) is ≤ 255, so the `i32 -> u8` cast is lossless: the helper the
//!      binary uses to compute the process status must round-trip each stable
//!      code to its own value. This catches a regression where someone returns a
//!      code > 255 (silently truncated by `as u8`) or maps the wrong status.
//!  E2. **Success is exit 0.** `process_exit_code(0) == 0`.
//!  E3. **Internal/unknown is exit 1.** `process_exit_code(1) == 1` (the runner
//!      maps untyped errors to `Internal` = 1; the binary must not remap it).
//!  E4. **Usage is exit 2.** `process_exit_code(2) == 2`.
//!  E5. **No clamping / no swallowing.** The binary must NOT collapse non-zero
//!      codes to 0 or to 1 indiscriminately; distinct codes stay distinct
//!      through the cast (so automation can branch on them).
//!  E6. **End-to-end through the assembled binary.** Running the real `defi`
//!      executable for an offline success command exits 0; for a usage error
//!      (missing required flag / unknown command / bad chain) exits 2. This is
//!      the same i32 the runner returns, now observed at the OS level.
//!
//! These criteria assert against the **stable contract** (spec §2.2 exit-code
//! map + the "stable exit codes" non-negotiable), not Go internals. The Go
//! `main` has no `*_test.go` to port (it is the `os.Exit` shim); the meaningful
//! coverage is the OS-boundary fidelity, expressed here.

use std::process::Command;

use defi_errors::Code;

/// Resolve the assembled `defi` binary. `CARGO_BIN_EXE_defi` is set by Cargo for
/// THIS crate's integration tests because the binary (`[[bin]] name = "defi"`)
/// lives in this same package.
fn defi_bin() -> &'static str {
    env!("CARGO_BIN_EXE_defi")
}

/// Run the assembled binary with a minimal, deterministic environment (no
/// provider keys; a throwaway HOME so cache-path resolution never touches the
/// real user config), returning `(exit_code, stdout, stderr)`.
fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(defi_bin())
        .args(args)
        .env_clear()
        .env("HOME", std::env::temp_dir())
        .output()
        .expect("run assembled `defi` binary");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// ---------------------------------------------------------------------------
// E1–E5: the pure i32 -> process-status mapping the thin binary performs.
//
// RED: `defi_cli::process_exit_code` does not exist yet. The GREEN phase must
// expose the cast `main.rs` performs as a small, pure, testable helper (a
// library target / module on the `defi-cli` crate) so the OS-boundary contract
// is unit-tested without spawning a process. Until then this file fails to
// compile — the intended RED signal.
// ---------------------------------------------------------------------------

#[test]
fn every_stable_code_round_trips_through_the_cast() {
    // E1: each contract code is ≤ 255 and maps to its own value unmangled.
    for code in Code::ALL {
        let i = code.as_i32();
        assert!(
            (0..=255).contains(&i),
            "stable exit code {i} must fit in a u8 (process status); a code > 255 \
             would be silently truncated by the `as u8` cast in main.rs"
        );
        assert_eq!(
            i32::from(defi_cli::process_exit_code(i)),
            i,
            "stable code {i} must reach the OS unmangled"
        );
    }
}

#[test]
fn success_is_zero() {
    // E2
    assert_eq!(defi_cli::process_exit_code(Code::Success.as_i32()), 0);
}

#[test]
fn internal_is_one() {
    // E3
    assert_eq!(defi_cli::process_exit_code(Code::Internal.as_i32()), 1);
}

#[test]
fn usage_is_two() {
    // E4
    assert_eq!(defi_cli::process_exit_code(Code::Usage.as_i32()), 2);
}

#[test]
fn distinct_codes_stay_distinct() {
    // E5: no clamping/collapsing — automation must be able to branch on codes.
    let mut seen = std::collections::BTreeSet::new();
    for code in Code::ALL {
        let mapped = defi_cli::process_exit_code(code.as_i32());
        assert!(
            seen.insert(mapped),
            "code {} collided with another after mapping (lost distinctness)",
            code.as_i32()
        );
    }
    // All 15 stable codes remain distinct process statuses.
    assert_eq!(seen.len(), Code::ALL.len());
}

// ---------------------------------------------------------------------------
// E6: end-to-end exit codes observed at the OS level through the real binary.
// ---------------------------------------------------------------------------

#[test]
fn assembled_binary_success_exits_zero() {
    // `providers list` is offline metadata (cache-bypassed) → success, exit 0.
    let (code, _stdout, _stderr) = run(&["providers", "list", "--results-only"]);
    assert_eq!(code, Some(0), "offline success command must exit 0");
}

#[test]
fn assembled_binary_usage_error_exits_two() {
    // Missing required `--asset`/`--symbol` → usage error (Code::Usage = 2).
    let (code, _stdout, _stderr) = run(&["assets", "resolve", "--chain", "1"]);
    assert_eq!(code, Some(2), "usage error must exit 2");
}

#[test]
fn assembled_binary_unknown_command_exits_two() {
    // Unknown command path → usage error (exit 2), matching the Go behavior.
    let (code, _stdout, _stderr) = run(&["frobnicate"]);
    assert_eq!(code, Some(2), "unknown command must exit 2");
}

#[test]
fn assembled_binary_bad_chain_exits_two() {
    let (code, _stdout, _stderr) =
        run(&["assets", "resolve", "--symbol", "USDC", "--chain", "nope"]);
    assert_eq!(code, Some(2), "bad --chain is a usage error → exit 2");
}

#[test]
fn assembled_binary_does_not_swallow_errors() {
    // The shim must propagate the runner's non-zero code; it must never force 0
    // on an error path (the Go `main` returns whatever `Run` returns).
    let (code, _stdout, _stderr) = run(&["assets", "resolve", "--chain", "1"]);
    assert_ne!(code, Some(0), "error path must not exit 0");
}
