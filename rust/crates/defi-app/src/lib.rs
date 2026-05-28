//! Command wiring (clap), provider routing, cache flow.
//!
//! Mirrors `internal/app/runner.go` plus the per-command-group handlers. The
//! binary crate (`defi-cli`) is a thin shim over [`run`].
#![allow(dead_code, unused)]

pub mod runner;

// One module per command group.
pub mod actions;
pub mod approvals;
pub mod bridge;
pub mod chains;
pub mod dexes;
pub mod lend;
pub mod protocols;
pub mod providers;
pub mod rewards;
pub mod schema;
pub mod stablecoins;
pub mod swap;
pub mod transfer;
pub mod version;
pub mod wallet;
pub mod r#yield;

/// CLI entrypoint. Parses args, routes to a command group, renders the
/// envelope, and returns the process exit code.
///
/// Scaffold stub — implemented in Phase 2/3.
pub async fn run() -> i32 {
    todo!("defi-app::run wired in Phase 2/3")
}
