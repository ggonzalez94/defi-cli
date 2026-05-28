//! Stable, machine-readable error codes mapped to process exit codes.
//!
//! Mirrors `internal/errors/errors.go`. The numeric values are part of the
//! machine contract (spec §2.2) and MUST NOT change.
#![allow(dead_code, unused)]

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
