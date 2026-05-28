//! Canonical IDs and amount normalization.
//!
//! Mirrors `internal/id`: CAIP-2/19 parsing, chain aliases, amount
//! normalization (base units + decimal), and the bootstrap token registry.
#![allow(dead_code, unused)]

pub mod amount;
pub mod caip;
pub mod chain;
pub mod tokens;

pub use chain::Chain;

/// A resolved asset reference (token symbol/address/CAIP-19) on a chain.
///
/// Scaffold stub — fields are filled in by the `caip`/`tokens` modules in
/// Phase 2.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Asset {
    pub raw: String,
}
