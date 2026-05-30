//! Provider adapters + provider traits + normalization.
//!
//! Mirrors `internal/providers`. Each adapter is a module; the provider traits
//! (one per Go provider interface) live in [`traits`]. Execution-capable
//! providers implement the builder traits from `defi-execution`.
#![allow(dead_code, unused)]

pub mod normalize;
pub(crate) mod serde_util;
pub mod traits;

// One module per provider adapter.
pub mod aave;
pub mod across;
pub mod bungee;
pub mod defillama;
pub mod fibrous;
pub mod jupiter;
pub mod kamino;
pub mod lifi;
pub mod moonwell;
pub mod morpho;
pub mod oneinch;
pub mod taikoswap;
pub mod tempo;
pub mod uniswap;
pub mod yieldutil;

pub use traits::*;
