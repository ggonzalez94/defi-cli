//! sqlite cache + file lock.
//!
//! Mirrors `internal/cache` (+ fsutil). Fresh hit (`age <= ttl`) skips provider
//! calls; expired re-fetches; stale served only within `max_stale` on temporary
//! provider failure (behavioral invariant — spec §2.5).
#![allow(dead_code, unused)]

pub mod lock;
pub mod store;
