//! Configuration: defaults + file/env/flags precedence.
//!
//! Mirrors `internal/config`. Precedence is `flags > env > config file >
//! defaults` (behavioral invariant — spec §2.5).
#![allow(dead_code, unused)]
