//! Stable, machine-readable error codes mapped to process exit codes.
//!
//! Mirrors `internal/errors/errors.go`. The numeric values are part of the
//! machine contract (spec §2.2) and MUST NOT change.

use thiserror::Error;

/// Stable, machine-readable error code mapped to a process exit code.
///
/// The discriminant values are part of the machine contract (spec §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum Code {
    Success = 0,
    Internal = 1,
    Usage = 2,
    Auth = 10,
    RateLimited = 11,
    Unavailable = 12,
    Unsupported = 13,
    Stale = 14,
    PartialStrict = 15,
    Blocked = 16,
    ActionPlan = 20,
    ActionSim = 21,
    ActionPolicy = 22,
    ActionTimeout = 23,
    Signer = 24,
}

impl Code {
    /// The canonical, ordered list of every error code in the contract.
    ///
    /// Lets callers enumerate the stable code set (spec §2.2) without
    /// hand-maintaining a copy. Order matches the spec table.
    pub const ALL: [Code; 15] = [
        Code::Success,
        Code::Internal,
        Code::Usage,
        Code::Auth,
        Code::RateLimited,
        Code::Unavailable,
        Code::Unsupported,
        Code::Stale,
        Code::PartialStrict,
        Code::Blocked,
        Code::ActionPlan,
        Code::ActionSim,
        Code::ActionPolicy,
        Code::ActionTimeout,
        Code::Signer,
    ];

    /// The stable integer value of this code.
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

/// A typed CLI error carrying a stable [`Code`].
#[derive(Debug, Error)]
pub struct Error {
    pub code: Code,
    pub message: String,
    #[source]
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.cause {
            None => write!(f, "{}", self.message),
            Some(cause) => write!(f, "{}: {}", self.message, cause),
        }
    }
}

impl Error {
    /// Create a new typed error without a cause.
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Error {
            code,
            message: message.into(),
            cause: None,
        }
    }

    /// Create a new typed error wrapping a cause.
    pub fn wrap(
        code: Code,
        message: impl Into<String>,
        cause: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Error {
            code,
            message: message.into(),
            cause: Some(Box::new(cause)),
        }
    }

    /// Discover a typed [`Error`] through an arbitrary error-wrapping chain.
    ///
    /// Mirrors Go `errors.As(err, &target)` for `*clierr.Error`: starting at
    /// `err`, walk the [`std::error::Error::source`] chain and return the first
    /// node that downcasts to a typed [`Error`]. Returns [`None`] for a foreign
    /// error that neither is nor wraps a typed [`Error`].
    pub fn find<'a>(err: &'a (dyn std::error::Error + 'static)) -> Option<&'a Error> {
        let mut current: Option<&'a (dyn std::error::Error + 'static)> = Some(err);
        while let Some(e) = current {
            if let Some(typed) = e.downcast_ref::<Error>() {
                return Some(typed);
            }
            current = e.source();
        }
        None
    }
}

/// The process exit code for a result.
///
/// `Ok(())` → 0 (success). A typed [`Error`] → its [`Code`] value.
pub fn exit_code(result: &Result<(), Error>) -> i32 {
    match result {
        Ok(()) => Code::Success.as_i32(),
        Err(err) => err.code.as_i32(),
    }
}

/// The process exit code for an arbitrary error.
///
/// Mirrors Go `ExitCode(err error)` for the non-nil case: if a typed [`Error`]
/// is discoverable through the wrapping chain ([`Error::find`]), surface its
/// [`Code`]; otherwise a foreign/untyped error maps to [`Code::Internal`].
/// Success is never produced for a non-nil error.
pub fn exit_code_for(err: &(dyn std::error::Error + 'static)) -> i32 {
    match Error::find(err) {
        Some(typed) => typed.code.as_i32(),
        None => Code::Internal.as_i32(),
    }
}

// =============================================================================
// SUCCESS CRITERIA (RED phase — tests written before implementation)
//
// This module (Go source: internal/errors) owns the stable error-code contract
// (spec §2.2). The Rust port is "correct" iff:
//
//   1. EXIT CODE MAP is byte-stable. Every Code discriminant equals the exact
//      integer from spec §2.2:
//        Success=0 Internal=1 Usage=2 Auth=10 RateLimited=11 Unavailable=12
//        Unsupported=13 Stale=14 PartialStrict=15 Blocked=16 ActionPlan=20
//        ActionSim=21 ActionPolicy=22 ActionTimeout=23 Signer=24
//      No other values; the enum is exactly these 15 variants.
//
//   2. exit_code(Ok(())) == 0; exit_code(Err(e)) == e.code as i32. (mirrors
//      Go ExitCode for the success + typed-error cases.)
//
//   3. An UNTYPED / unknown error maps to Internal (1). Go's
//      `ExitCode(err error)` accepts ANY error and returns CodeInternal when
//      the error is not (and does not wrap) a *clierr.Error. The Rust analogue
//      `exit_code_for(&dyn Error)` must reproduce this: Success is never
//      produced for a non-nil error; a foreign error → 1.
//
//   4. `As`-equivalence: a typed Error must be discoverable through an arbitrary
//      error-wrapping chain (Go `errors.As(wrapped, &typed)`), and
//      exit_code_for must surface the wrapped typed Error's code (not Internal).
//      See internal/execution/executor_error_test.go: wrapEVMExecutionError wraps
//      a typed CodeActionSim error inside another error and errors.As recovers it.
//
//   5. Display formatting matches Go `(*Error).Error()`:
//        - no cause      → exactly `message`
//        - with cause    → exactly `message: <cause display>`
//      (Go uses fmt.Sprintf("%s: %v", Message, Cause).)
//
//   6. Constructors: `new` sets code+message, no cause; `wrap` sets
//      code+message+cause and the cause is reachable via `source()`.
//
// These are fresh spec-driven tests (the Go internal/errors package ships with
// NO *_test.go file); the wrapping/As behavior is ported from the meaningful
// assertions in internal/execution/executor_error_test.go.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as StdError;

    // ---- Criterion 1: stable exit-code map -------------------------------

    #[test]
    fn code_discriminants_match_spec_2_2() {
        assert_eq!(Code::Success.as_i32(), 0);
        assert_eq!(Code::Internal.as_i32(), 1);
        assert_eq!(Code::Usage.as_i32(), 2);
        assert_eq!(Code::Auth.as_i32(), 10);
        assert_eq!(Code::RateLimited.as_i32(), 11);
        assert_eq!(Code::Unavailable.as_i32(), 12);
        assert_eq!(Code::Unsupported.as_i32(), 13);
        assert_eq!(Code::Stale.as_i32(), 14);
        assert_eq!(Code::PartialStrict.as_i32(), 15);
        assert_eq!(Code::Blocked.as_i32(), 16);
        assert_eq!(Code::ActionPlan.as_i32(), 20);
        assert_eq!(Code::ActionSim.as_i32(), 21);
        assert_eq!(Code::ActionPolicy.as_i32(), 22);
        assert_eq!(Code::ActionTimeout.as_i32(), 23);
        assert_eq!(Code::Signer.as_i32(), 24);
    }

    #[test]
    fn code_all_lists_exactly_the_spec_set_in_order() {
        // `Code::ALL` is the canonical, ordered list of every code; lets callers
        // (and this test) enumerate the contract without hand-maintaining a copy.
        let expected: &[(Code, i32)] = &[
            (Code::Success, 0),
            (Code::Internal, 1),
            (Code::Usage, 2),
            (Code::Auth, 10),
            (Code::RateLimited, 11),
            (Code::Unavailable, 12),
            (Code::Unsupported, 13),
            (Code::Stale, 14),
            (Code::PartialStrict, 15),
            (Code::Blocked, 16),
            (Code::ActionPlan, 20),
            (Code::ActionSim, 21),
            (Code::ActionPolicy, 22),
            (Code::ActionTimeout, 23),
            (Code::Signer, 24),
        ];
        assert_eq!(Code::ALL.len(), expected.len(), "exactly 15 codes");
        for ((code, want), got) in expected.iter().zip(Code::ALL.iter()) {
            assert_eq!(*code, *got);
            assert_eq!(got.as_i32(), *want);
        }
    }

    #[test]
    fn exit_codes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for c in Code::ALL {
            assert!(
                seen.insert(c.as_i32()),
                "duplicate exit code {}",
                c.as_i32()
            );
        }
    }

    // ---- Criterion 2: exit_code for Result -------------------------------

    #[test]
    fn exit_code_ok_is_zero() {
        let ok: Result<(), Error> = Ok(());
        assert_eq!(exit_code(&ok), 0);
    }

    #[test]
    fn exit_code_typed_err_is_its_code() {
        let err: Result<(), Error> = Err(Error::new(Code::Auth, "no key"));
        assert_eq!(exit_code(&err), 10);
        let err2: Result<(), Error> = Err(Error::new(Code::Usage, "bad flag"));
        assert_eq!(exit_code(&err2), 2);
    }

    // ---- Criterion 3: untyped error → Internal (1) -----------------------

    #[test]
    fn exit_code_for_untyped_error_is_internal() {
        // A foreign std error that is not (and does not wrap) a typed Error.
        let foreign = std::io::Error::other("boom");
        assert_eq!(exit_code_for(&foreign), Code::Internal.as_i32());
    }

    #[test]
    fn exit_code_for_typed_error_is_its_code() {
        let typed = Error::new(Code::RateLimited, "slow down");
        assert_eq!(exit_code_for(&typed), 11);
    }

    // ---- Criterion 4: As-equivalence through a wrapping chain ------------

    /// A FOREIGN error type (not a typed [`Error`]) that wraps a `source`.
    ///
    /// This is the crux of the `errors.As` contract: a typed CLI error nested
    /// inside a foreign wrapper must be recoverable only by WALKING the
    /// `source()` chain. A typed-wraps-typed case does NOT exercise that walk
    /// (the first downcast succeeds immediately), so this foreign wrapper is
    /// required to actually test the traversal in [`Error::find`].
    #[derive(Debug)]
    struct ForeignWrapper {
        msg: &'static str,
        source: Box<dyn StdError + Send + Sync + 'static>,
    }

    impl std::fmt::Display for ForeignWrapper {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.msg)
        }
    }

    impl StdError for ForeignWrapper {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            Some(self.source.as_ref())
        }
    }

    #[test]
    fn find_recovers_typed_error_nested_in_foreign_wrapper() {
        // Mirrors Go errors.As(wrapped, &typed) where the wrapper is FOREIGN:
        // find must walk source() to recover the inner typed error. This is the
        // only test that exercises the chain-walking branch of Error::find; a
        // typed-wraps-typed case returns on the first downcast and never walks.
        let typed = Error::new(Code::ActionSim, "simulate step (eth_call)");
        let foreign = ForeignWrapper {
            msg: "execution reverted",
            source: Box::new(typed),
        };

        let found = Error::find(&foreign).expect("typed error must be discoverable via source()");
        assert_eq!(found.code, Code::ActionSim);
        assert_eq!(found.message, "simulate step (eth_call)");
    }

    #[test]
    fn find_recovers_typed_error_two_foreign_layers_deep() {
        // Two foreign layers above the typed error: proves find keeps walking,
        // not just one hop. If the loop stopped advancing (current = e.source()
        // removed), this would regress to None.
        let typed = Error::new(Code::Signer, "sender mismatch");
        let inner = ForeignWrapper {
            msg: "submit step",
            source: Box::new(typed),
        };
        let outer = ForeignWrapper {
            msg: "execute action",
            source: Box::new(inner),
        };

        let found = Error::find(&outer).expect("typed error must survive two foreign layers");
        assert_eq!(found.code, Code::Signer);
    }

    #[test]
    fn find_returns_first_typed_error_when_outer_is_typed() {
        // When the OUTER node is itself typed, find returns it immediately
        // (matches Go errors.As taking the first *Error in the chain), even if a
        // different typed code is nested below.
        let inner = Error::new(Code::Usage, "bad flag");
        let outer = Error::wrap(Code::Internal, "persist action state", inner);
        let found = Error::find(&outer).expect("outer typed error must be found");
        assert_eq!(found.code, Code::Internal);
    }

    #[test]
    fn find_returns_none_for_foreign_error() {
        let foreign = std::io::Error::other("boom");
        assert!(Error::find(&foreign).is_none());
    }

    #[test]
    fn find_returns_none_for_foreign_wrapping_foreign() {
        // A foreign error wrapping another foreign error (no typed node anywhere)
        // must return None even though find walks the whole chain.
        let root = std::io::Error::other("root cause");
        let foreign = ForeignWrapper {
            msg: "outer",
            source: Box::new(root),
        };
        assert!(Error::find(&foreign).is_none());
    }

    #[test]
    fn exit_code_for_surfaces_typed_code_nested_in_foreign_wrapper() {
        // The real "surfaces wrapped code" contract: a typed Usage error nested
        // inside a FOREIGN wrapper must surface code 2 (Usage), NOT Internal (1).
        // This proves exit_code_for does not fall back to Internal whenever the
        // outermost error happens to be foreign.
        let typed = Error::new(Code::Usage, "bad flag");
        let foreign = ForeignWrapper {
            msg: "execution reverted",
            source: Box::new(typed),
        };
        assert_eq!(exit_code_for(&foreign), Code::Usage.as_i32());
    }

    #[test]
    fn exit_code_for_surfaces_outermost_typed_code() {
        // Wrap a typed Usage error as the cause of an Internal-coded typed
        // wrapper; the OUTERMOST typed error's code is what surfaces (matches Go:
        // the first typed *Error found by errors.As, i.e. the outer one).
        let inner = Error::new(Code::Usage, "bad flag");
        let outer = Error::wrap(Code::Internal, "persist action state", inner);
        assert_eq!(exit_code_for(&outer), Code::Internal.as_i32());
    }

    // ---- Criterion 5: Display formatting ---------------------------------

    #[test]
    fn display_without_cause_is_message_only() {
        let e = Error::new(Code::Usage, "exactly one identity input is required");
        assert_eq!(e.to_string(), "exactly one identity input is required");
    }

    #[test]
    fn display_with_cause_is_message_colon_cause() {
        let cause = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let e = Error::wrap(Code::Internal, "load action", cause);
        assert_eq!(e.to_string(), "load action: missing");
    }

    // ---- Criterion 6: constructors + source ------------------------------

    #[test]
    fn new_sets_code_and_message_no_cause() {
        let e = Error::new(Code::Blocked, "blocked");
        assert_eq!(e.code, Code::Blocked);
        assert_eq!(e.message, "blocked");
        assert!(e.source().is_none());
    }

    #[test]
    fn wrap_exposes_cause_via_source() {
        let cause = std::io::Error::other("root");
        let e = Error::wrap(Code::ActionTimeout, "submit", cause);
        assert_eq!(e.code, Code::ActionTimeout);
        let src = e.source().expect("cause must be reachable via source()");
        assert_eq!(src.to_string(), "root");
    }
}
