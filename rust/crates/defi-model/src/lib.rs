//! Output envelope + domain models.
//!
//! Mirrors `internal/model/types.go`. Field names and declaration order are
//! part of the machine contract (spec §2.1, §2.3) and MUST be preserved —
//! serde serializes struct fields in declaration order with these `rename`s.
#![allow(dead_code, unused)]

pub mod domain;
pub mod envelope;

pub use domain::*;
pub use envelope::*;

/// Envelope schema version (`"v1"`).
pub const ENVELOPE_VERSION: &str = "v1";

/// Provider-native ID kinds.
pub const NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET: &str = "composite_market_asset";
pub const NATIVE_ID_KIND_MARKET_ID: &str = "market_id";
pub const NATIVE_ID_KIND_VAULT_ADDRESS: &str = "vault_address";
pub const NATIVE_ID_KIND_POOL_ID: &str = "pool_id";
