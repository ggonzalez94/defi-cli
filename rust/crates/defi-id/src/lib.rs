//! Canonical IDs and amount normalization.
//!
//! Mirrors `internal/id`: CAIP-2/19 parsing, chain aliases, amount
//! normalization (base units + decimal), and the bootstrap token registry.

pub mod amount;
pub mod caip;
pub mod chain;
pub mod tokens;

pub use amount::{format_decimal, normalize_amount, MAX_UINT256};
pub use chain::{list_chains, parse_chain, Chain, ChainEntry};
pub use tokens::{
    find_token_by_address, find_tokens_by_symbol, known_token, lookup_by_address, parse_asset,
    Token,
};

/// A resolved asset reference (token symbol/address/CAIP-19) on a chain.
///
/// Field declaration order mirrors Go `id.Asset` (`ChainID, AssetID, Address,
/// Symbol, Decimals`) so any future serde projection keeps contract field
/// order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Asset {
    pub chain_id: String,
    pub asset_id: String,
    pub address: String,
    pub symbol: String,
    pub decimals: i32,
}
