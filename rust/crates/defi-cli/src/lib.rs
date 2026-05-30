//! Library surface for the thin `defi` binary.
//!
//! `cmd/defi/main.go` is a twelve-line `os.Exit(runner.Run(...))` shim. Its only
//! contract is to translate the `i32` the runner returns into the **OS process
//! exit status** unmangled. That cast is the one piece of logic the L6 crate
//! owns, so it lives here as a small, pure, unit-testable helper instead of being
//! buried inside `main`. The per-command output contract (envelope shape, JSON
//! declaration order, plain key-sort, projection, golden parity) is owned and
//! exhaustively tested by the `defi-app` (L5) crate.

/// Map a runner exit code to the `u8` process status the OS observes.
///
/// `main` does `ExitCode::from(code as u8)`; this helper makes that exact cast
/// explicit and testable so a regression (clamping, swallowing, or mapping the
/// wrong status) is caught at the OS boundary.
///
/// Every code in the stable contract map
/// (`defi_errors::Code::ALL` = {0,1,2,10,11,12,13,14,15,16,20,21,22,23,24},
/// spec §2.2) is `<= 255`, so the cast is lossless and each code reaches the OS
/// as its own value. Distinct stable codes stay distinct, letting automation
/// branch on them.
#[must_use]
pub fn process_exit_code(code: i32) -> u8 {
    code as u8
}

#[cfg(test)]
mod tests {
    use super::process_exit_code;
    use defi_errors::Code;

    #[test]
    fn every_stable_code_round_trips() {
        for code in Code::ALL {
            let i = code.as_i32();
            assert_eq!(i32::from(process_exit_code(i)), i);
        }
    }
}
