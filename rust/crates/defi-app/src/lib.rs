//! Command wiring (clap), provider routing, cache flow.
//!
//! Mirrors `internal/app/runner.go` plus the per-command-group handlers. The
//! binary crate (`defi-cli`) is a thin shim over [`run`].
#![allow(dead_code, unused)]
// Stylistic rustdoc list-formatting lints (stabilized in clippy 1.94) trip on
// the deeply-nested `(a)/(b)/(c)` enumerations in several command modules'
// success-criteria doc comments. They are pure prose formatting with no bearing
// on the machine contract; allow them crate-wide so the always-green clippy gate
// stays clean without rewording the criteria.
#![allow(clippy::doc_overindented_list_items, clippy::doc_lazy_continuation)]

pub mod runner;

// Shared application plumbing.
pub mod ctx;
pub mod execflags;
pub mod execident;
pub mod execsubmit;

// One module per command group.
pub mod actions;
pub mod approvals;
pub mod assets;
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

mod cli;

/// CLI entrypoint. Parses args from the process, routes to a command group,
/// renders the envelope (success → stdout, error → stderr), and returns the
/// process exit code.
///
/// Currently wires the deterministic, offline command surface that has golden
/// parity coverage (`version`, `schema`, `providers list`, `chains list`,
/// `assets resolve`). Live/cache-backed command groups are dispatched by their
/// own modules and wired here incrementally.
pub async fn run() -> i32 {
    cli::run_with_args(std::env::args_os(), &defi_config::SystemEnv).await
}
