//! Domain models.
//!
//! Field declaration order, `rename`s, and `skip_serializing_if` mirror
//! `internal/model/types.go` exactly. Go `omitempty` on numeric/bool fields
//! maps to `skip_serializing_if` helpers below so zero values are omitted to
//! match Go's encoding/json behavior (machine contract — spec §2.1).

use serde::{Deserialize, Serialize};

// --- omitempty helpers (match Go encoding/json zero-value omission) ---

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub requires_key: bool,
    pub capabilities: Vec<String>,
    #[serde(
        rename = "key_env_var",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub key_env_var_name: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub capability_auth: Vec<ProviderCapabilityAuth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCapabilityAuth {
    pub capability: String,
    pub key_env_var: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportedChain {
    pub name: String,
    pub slug: String,
    pub caip2: String,
    pub namespace: String,
    #[serde(skip_serializing_if = "is_zero_i64", default)]
    pub evm_chain_id: i64,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasPrice {
    pub chain_id: String,
    pub chain_name: String,
    pub block_number: i64,
    pub eip1559: bool,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub base_fee_gwei: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub priority_fee_gwei: String,
    pub gas_price_gwei: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub warnings: Vec<String>,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainTvl {
    pub rank: i64,
    pub chain: String,
    pub chain_id: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub tvl_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainAssetTvl {
    pub rank: i64,
    pub chain: String,
    pub chain_id: String,
    pub asset: String,
    pub asset_id: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub tvl_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolTvl {
    pub rank: i64,
    pub protocol: String,
    pub category: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub tvl_usd: f64,
    pub chains: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolCategory {
    pub name: String,
    pub protocols: i64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub tvl_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolFees {
    pub rank: i64,
    pub protocol: String,
    pub category: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub fees_24h_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub fees_7d_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub fees_30d_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub change_1d_pct: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub change_7d_pct: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub change_1m_pct: f64,
    pub chains: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolRevenue {
    pub rank: i64,
    pub protocol: String,
    pub category: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub revenue_24h_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub revenue_7d_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub revenue_30d_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub change_1d_pct: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub change_7d_pct: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub change_1m_pct: f64,
    pub chains: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DexVolume {
    pub rank: i64,
    pub protocol: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub volume_24h_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub volume_7d_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub volume_30d_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub change_1d_pct: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub change_7d_pct: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub change_1m_pct: f64,
    pub chains: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stablecoin {
    pub rank: i64,
    pub name: String,
    pub symbol: String,
    pub peg_type: String,
    pub peg_mechanism: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub circulating_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub price: f64,
    pub chains: i64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub day_change_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub week_change_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub month_change_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StablecoinChain {
    pub rank: i64,
    pub chain: String,
    pub chain_id: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub circulating_usd: f64,
    pub dominant_peg_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetResolution {
    pub input: String,
    pub chain_id: String,
    pub symbol: String,
    pub asset_id: String,
    pub address: String,
    pub decimals: i64,
    pub resolved_by: String,
    pub unambiguous: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LendMarket {
    pub protocol: String,
    pub provider: String,
    pub chain_id: String,
    pub asset_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider_native_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider_native_id_kind: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub supply_apy: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub borrow_apy: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub tvl_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub liquidity_usd: f64,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub source_url: String,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LendRate {
    pub protocol: String,
    pub provider: String,
    pub chain_id: String,
    pub asset_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider_native_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider_native_id_kind: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub supply_apy: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub borrow_apy: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub utilization: f64,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub source_url: String,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LendPosition {
    pub protocol: String,
    pub provider: String,
    pub chain_id: String,
    pub account_address: String,
    pub position_type: String,
    pub asset_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider_native_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider_native_id_kind: String,
    pub amount: AmountInfo,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub amount_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub apy: f64,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub source_url: String,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AmountInfo {
    pub amount_base_units: String,
    pub amount_decimal: String,
    pub decimals: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeeAmount {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub amount_base_units: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub amount_decimal: String,
    #[serde(
        skip_serializing_if = "is_zero_f64",
        serialize_with = "crate::go_float::serialize",
        default
    )]
    pub amount_usd: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeFeeBreakdown {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lp_fee: Option<FeeAmount>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub relayer_fee: Option<FeeAmount>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gas_fee: Option<FeeAmount>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub total_fee_base_units: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub total_fee_decimal: String,
    #[serde(
        skip_serializing_if = "is_zero_f64",
        serialize_with = "crate::go_float::serialize",
        default
    )]
    pub total_fee_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub consistent_with_amount_delta: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeVolumes {
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub last_hourly_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub last_24h_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub last_daily_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub prev_day_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub prev_2d_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub weekly_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub monthly_usd: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeTxCounts {
    pub deposits: i64,
    pub withdrawals: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeTransactions {
    pub last_hourly: BridgeTxCounts,
    pub current_day: BridgeTxCounts,
    pub prev_day: BridgeTxCounts,
    pub prev_2d: BridgeTxCounts,
    pub weekly: BridgeTxCounts,
    pub monthly: BridgeTxCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeSummary {
    pub bridge_id: i64,
    pub name: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub slug: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub destination_chain: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub url: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub chains: Vec<String>,
    pub volumes: BridgeVolumes,
    pub last_updated_unix: i64,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeChainDetails {
    pub chain: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub chain_id: String,
    pub volumes: BridgeVolumes,
    pub transactions: BridgeTransactions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeDetails {
    pub bridge_id: i64,
    pub name: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub destination_chain: String,
    pub volumes: BridgeVolumes,
    pub transactions: BridgeTransactions,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub chain_breakdown: Vec<BridgeChainDetails>,
    pub last_updated_unix: i64,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeQuote {
    pub provider: String,
    pub from_chain_id: String,
    pub to_chain_id: String,
    pub from_asset_id: String,
    pub to_asset_id: String,
    pub input_amount: AmountInfo,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub from_amount_for_gas: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub estimated_destination_native: Option<AmountInfo>,
    pub estimated_out: AmountInfo,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub estimated_fee_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fee_breakdown: Option<BridgeFeeBreakdown>,
    pub estimated_time_s: i64,
    pub route: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub source_url: String,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapQuote {
    pub provider: String,
    pub chain_id: String,
    pub from_asset_id: String,
    pub to_asset_id: String,
    pub trade_type: String,
    pub input_amount: AmountInfo,
    pub estimated_out: AmountInfo,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub estimated_gas_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub price_impact_pct: f64,
    pub route: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub source_url: String,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YieldBackingAsset {
    pub asset_id: String,
    pub symbol: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub share_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YieldOpportunity {
    pub opportunity_id: String,
    pub provider: String,
    pub protocol: String,
    pub chain_id: String,
    pub asset_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider_native_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider_native_id_kind: String,
    #[serde(rename = "type")]
    pub opportunity_type: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub apy_base: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub apy_reward: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub apy_total: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub tvl_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub liquidity_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub lockup_days: f64,
    pub withdrawal_terms: String,
    pub backing_assets: Vec<YieldBackingAsset>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub source_url: String,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YieldPosition {
    pub protocol: String,
    pub provider: String,
    pub chain_id: String,
    pub account_address: String,
    pub position_type: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub opportunity_id: String,
    pub asset_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider_native_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider_native_id_kind: String,
    pub amount: AmountInfo,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shares: Option<AmountInfo>,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub amount_usd: f64,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub apy_total: f64,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub source_url: String,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletBalance {
    pub chain_id: String,
    pub account_address: String,
    pub asset_type: String,
    pub asset_id: String,
    pub symbol: String,
    pub balance: AmountInfo,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YieldHistoryPoint {
    pub timestamp: String,
    #[serde(serialize_with = "crate::go_float::serialize")]
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YieldHistorySeries {
    pub opportunity_id: String,
    pub provider: String,
    pub protocol: String,
    pub chain_id: String,
    pub asset_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider_native_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider_native_id_kind: String,
    pub metric: String,
    pub interval: String,
    pub start_time: String,
    pub end_time: String,
    pub points: Vec<YieldHistoryPoint>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub source_url: String,
    pub fetched_at: String,
}

#[cfg(test)]
#[allow(clippy::doc_lazy_continuation)]
mod tests {
    //! # Success criteria — `defi-model::domain` (Go: `internal/model/types.go`)
    //!
    //! This module owns every domain payload struct that the runner places into
    //! `Envelope.data`. The port is "correct" iff each struct preserves the
    //! stable machine contract (design spec §2.1, §2.3, §2.4, §7) when
    //! serialized with `serde_json` (`preserve_order`). These tests assert the
    //! contract, NOT Go internals:
    //!
    //! 1. **Field DECLARATION order.** Serialized JSON keys appear in struct
    //!    declaration order copied verbatim from `internal/model/types.go` — NOT
    //!    alphabetical. Asserted on representative structs across the shape space:
    //!    a flat scalar struct (`AssetResolution`), a struct with a nested
    //!    `AmountInfo` (`LendPosition`), a struct with a `Vec<_>` field
    //!    (`YieldOpportunity`), and a struct with `Option<_>` fields
    //!    (`BridgeFeeBreakdown`).
    //! 2. **JSON key renames.** Go `json:"type"` maps to JSON key `type` for both
    //!    `ProviderInfo` (Rust field `provider_type`) and `YieldOpportunity`
    //!    (Rust field `opportunity_type`). The Rust field name must NOT leak.
    //! 3. **Go `omitempty` semantics.** Fields tagged `omitempty` in Go are
    //!    omitted at their zero value and present otherwise:
    //!      - `String` → omitted when empty (`source_url`, `provider_native_id`).
    //!      - `Vec<_>` → omitted when empty (`SupportedChain.aliases`,
    //!        `BridgeSummary.chains`).
    //!      - `Option<_>` → omitted when `None` (`YieldPosition.shares`,
    //!        `BridgeFeeBreakdown.*`).
    //!      - numeric/bool `omitempty` → omitted at zero (`FeeAmount.amount_usd`,
    //!        `SupportedChain.evm_chain_id`).
    //!    Fields WITHOUT `omitempty` are ALWAYS present even at zero value
    //!    (`AmountInfo.decimals`, `LendMarket.supply_apy`,
    //!    `AssetResolution.unambiguous`).
    //! 4. **Float formatting parity (spec §7 — load-bearing).** Go `encoding/json`
    //!    renders an integer-valued `float64` WITHOUT a fractional part
    //!    (`2.0 → "2"`, `100.0 → "100"`, `-3.0 → "-3"`, `0.0 → "0"`), while
    //!    fractional values keep their digits (`2.3 → "2.3"`). serde's default
    //!    `f64` renders `2.0 → "2.0"`, which DIVERGES. Every `f64` contract field
    //!    (APYs, USD amounts, price-impact, share-pct, tvl) must serialize the
    //!    Go way. APY values are percentage points, not ratios (e.g. `2.3` == 2.3%).
    //! 5. **CAIP / amount consistency.** `AmountInfo` always carries
    //!    `amount_base_units`, `amount_decimal`, and `decimals` together (spec
    //!    §2.4); `AssetResolution.asset_id` is a CAIP-19 string that round-trips.
    //! 6. **Golden parity.** A standalone `AssetResolution` serialized with
    //!    2-space-indent declaration order matches the Go-captured
    //!    `assets-resolve-usdc-results-only.json` fixture BYTE-FOR-BYTE.
    //! 7. **Round-trip.** Each struct deserialized from canonical JSON and
    //!    re-serialized is value-identical (declaration order stable both ways).

    use super::*;
    use serde_json::{json, Value};

    /// Ordered list of JSON object keys in serialization order.
    fn ordered_keys(v: &Value) -> Vec<String> {
        v.as_object()
            .expect("expected JSON object")
            .keys()
            .cloned()
            .collect()
    }

    // --- 1. field declaration order -----------------------------------------

    #[test]
    fn asset_resolution_field_order() {
        let a = AssetResolution {
            input: "USDC".into(),
            chain_id: "eip155:1".into(),
            symbol: "USDC".into(),
            asset_id: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".into(),
            address: "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".into(),
            decimals: 6,
            resolved_by: "registry".into(),
            unambiguous: true,
        };
        let v = serde_json::to_value(&a).expect("serialize");
        assert_eq!(
            ordered_keys(&v),
            vec![
                "input",
                "chain_id",
                "symbol",
                "asset_id",
                "address",
                "decimals",
                "resolved_by",
                "unambiguous",
            ],
        );
    }

    #[test]
    fn lend_position_field_order_with_nested_amount() {
        let p = LendPosition {
            protocol: "aave-v3".into(),
            provider: "aave".into(),
            chain_id: "eip155:1".into(),
            account_address: "0x000000000000000000000000000000000000dEaD".into(),
            position_type: "supply".into(),
            asset_id: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".into(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            amount: AmountInfo {
                amount_base_units: "1000000".into(),
                amount_decimal: "1".into(),
                decimals: 6,
            },
            amount_usd: 1.0,
            apy: 2.3,
            source_url: String::new(),
            fetched_at: "2026-05-28T18:48:18Z".into(),
        };
        let v = serde_json::to_value(&p).expect("serialize");
        // omitempty: provider_native_id*, source_url omitted (empty).
        assert_eq!(
            ordered_keys(&v),
            vec![
                "protocol",
                "provider",
                "chain_id",
                "account_address",
                "position_type",
                "asset_id",
                "amount",
                "amount_usd",
                "apy",
                "fetched_at",
            ],
        );
        // Nested AmountInfo declaration order.
        assert_eq!(
            ordered_keys(&v["amount"]),
            vec!["amount_base_units", "amount_decimal", "decimals"],
        );
    }

    #[test]
    fn yield_opportunity_field_order_with_vec_and_rename() {
        let o = YieldOpportunity {
            opportunity_id: "opp-1".into(),
            provider: "aave".into(),
            protocol: "aave-v3".into(),
            chain_id: "eip155:1".into(),
            asset_id: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".into(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            opportunity_type: "lending".into(),
            apy_base: 2.0,
            apy_reward: 0.0,
            apy_total: 2.0,
            tvl_usd: 0.0,
            liquidity_usd: 0.0,
            lockup_days: 0.0,
            withdrawal_terms: "instant".into(),
            backing_assets: vec![YieldBackingAsset {
                asset_id: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".into(),
                symbol: "USDC".into(),
                share_pct: 100.0,
            }],
            source_url: String::new(),
            fetched_at: "2026-05-28T18:48:18Z".into(),
        };
        let v = serde_json::to_value(&o).expect("serialize");
        assert_eq!(
            ordered_keys(&v),
            vec![
                "opportunity_id",
                "provider",
                "protocol",
                "chain_id",
                "asset_id",
                "type",
                "apy_base",
                "apy_reward",
                "apy_total",
                "tvl_usd",
                "liquidity_usd",
                "lockup_days",
                "withdrawal_terms",
                "backing_assets",
                "fetched_at",
            ],
        );
        // backing_assets element declaration order.
        assert_eq!(
            ordered_keys(&v["backing_assets"][0]),
            vec!["asset_id", "symbol", "share_pct"],
        );
    }

    // --- 2. JSON key renames (json:"type") ----------------------------------

    #[test]
    fn provider_info_renames_type_key() {
        let p = ProviderInfo {
            name: "aave".into(),
            provider_type: "lending".into(),
            requires_key: false,
            capabilities: vec!["lend_markets".into()],
            key_env_var_name: String::new(),
            capability_auth: vec![],
        };
        let v = serde_json::to_value(&p).expect("serialize");
        let obj = v.as_object().expect("object");
        assert!(obj.contains_key("type"), "json key is `type`");
        assert!(
            !obj.contains_key("provider_type"),
            "rust field name must not leak"
        );
        assert_eq!(v["type"], "lending");
        // omitempty: key_env_var + capability_auth omitted when empty.
        assert!(!obj.contains_key("key_env_var"));
        assert!(!obj.contains_key("capability_auth"));
        // declaration order of present keys.
        assert_eq!(
            ordered_keys(&v),
            vec!["name", "type", "requires_key", "capabilities"],
        );
    }

    #[test]
    fn yield_opportunity_renames_type_key() {
        let o = YieldOpportunity {
            opportunity_id: "opp-1".into(),
            provider: "aave".into(),
            protocol: "aave-v3".into(),
            chain_id: "eip155:1".into(),
            asset_id: "x".into(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            opportunity_type: "lending".into(),
            apy_base: 0.0,
            apy_reward: 0.0,
            apy_total: 0.0,
            tvl_usd: 0.0,
            liquidity_usd: 0.0,
            lockup_days: 0.0,
            withdrawal_terms: String::new(),
            backing_assets: vec![],
            source_url: String::new(),
            fetched_at: String::new(),
        };
        let v = serde_json::to_value(&o).expect("serialize");
        assert!(v.as_object().unwrap().contains_key("type"));
        assert!(!v.as_object().unwrap().contains_key("opportunity_type"));
        assert_eq!(v["type"], "lending");
    }

    // --- 3. omitempty semantics ---------------------------------------------

    #[test]
    fn supported_chain_omits_empty_evm_id_and_aliases() {
        let c = SupportedChain {
            name: "Ethereum".into(),
            slug: "ethereum".into(),
            caip2: "eip155:1".into(),
            namespace: "eip155".into(),
            evm_chain_id: 0, // omitempty -> omitted at zero
            aliases: vec![], // omitempty -> omitted when empty
        };
        let v = serde_json::to_value(&c).expect("serialize");
        let obj = v.as_object().expect("object");
        assert!(
            !obj.contains_key("evm_chain_id"),
            "zero evm_chain_id omitted"
        );
        assert!(!obj.contains_key("aliases"), "empty aliases omitted");
        assert_eq!(ordered_keys(&v), vec!["name", "slug", "caip2", "namespace"],);
    }

    #[test]
    fn supported_chain_keeps_nonzero_evm_id_and_aliases() {
        let c = SupportedChain {
            name: "Ethereum".into(),
            slug: "ethereum".into(),
            caip2: "eip155:1".into(),
            namespace: "eip155".into(),
            evm_chain_id: 1,
            aliases: vec!["eth".into(), "mainnet".into()],
        };
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(v["evm_chain_id"], 1);
        assert_eq!(v["aliases"], json!(["eth", "mainnet"]));
        assert_eq!(
            ordered_keys(&v),
            vec![
                "name",
                "slug",
                "caip2",
                "namespace",
                "evm_chain_id",
                "aliases"
            ],
        );
    }

    #[test]
    fn fee_amount_omits_zero_usd_and_empty_strings() {
        let empty = FeeAmount::default();
        let v = serde_json::to_value(&empty).expect("serialize");
        assert_eq!(v, json!({}), "fully-empty FeeAmount serializes to {{}}");

        let partial = FeeAmount {
            amount_base_units: String::new(),
            amount_decimal: "1.5".into(),
            amount_usd: 0.0, // omitempty -> omitted at zero
        };
        let v = serde_json::to_value(&partial).expect("serialize");
        assert_eq!(v, json!({"amount_decimal": "1.5"}));
    }

    #[test]
    fn bridge_fee_breakdown_omits_none_options() {
        let b = BridgeFeeBreakdown::default();
        let v = serde_json::to_value(&b).expect("serialize");
        assert_eq!(v, json!({}), "all-None/zero BridgeFeeBreakdown is {{}}");

        let with_lp = BridgeFeeBreakdown {
            lp_fee: Some(FeeAmount {
                amount_base_units: String::new(),
                amount_decimal: "0.1".into(),
                amount_usd: 0.0,
            }),
            consistent_with_amount_delta: Some(true),
            ..Default::default()
        };
        let v = serde_json::to_value(&with_lp).expect("serialize");
        let obj = v.as_object().expect("object");
        assert!(obj.contains_key("lp_fee"));
        assert!(!obj.contains_key("relayer_fee"));
        assert!(obj.contains_key("consistent_with_amount_delta"));
        assert_eq!(v["consistent_with_amount_delta"], true);
    }

    #[test]
    fn yield_position_omits_none_shares_keeps_some() {
        let none = YieldPosition {
            protocol: "morpho".into(),
            provider: "morpho".into(),
            chain_id: "eip155:1".into(),
            account_address: "0xdead".into(),
            position_type: "supply".into(),
            opportunity_id: String::new(),
            asset_id: "x".into(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            amount: AmountInfo::default(),
            shares: None,
            amount_usd: 0.0,
            apy_total: 0.0,
            source_url: String::new(),
            fetched_at: String::new(),
        };
        let v = serde_json::to_value(&none).expect("serialize");
        assert!(
            !v.as_object().unwrap().contains_key("shares"),
            "None shares omitted"
        );

        let some = YieldPosition {
            shares: Some(AmountInfo {
                amount_base_units: "5".into(),
                amount_decimal: "5".into(),
                decimals: 0,
            }),
            ..none
        };
        let v = serde_json::to_value(&some).expect("serialize");
        assert!(v.as_object().unwrap().contains_key("shares"));
        assert_eq!(v["shares"]["amount_base_units"], "5");
    }

    #[test]
    fn non_omitempty_zero_fields_always_present() {
        // AmountInfo.decimals (no omitempty) present at 0.
        let amt = AmountInfo {
            amount_base_units: "0".into(),
            amount_decimal: "0".into(),
            decimals: 0,
        };
        let v = serde_json::to_value(&amt).expect("serialize");
        assert!(v.as_object().unwrap().contains_key("decimals"));
        assert_eq!(v["decimals"], 0);

        // AssetResolution.unambiguous (no omitempty) present at false.
        let a = AssetResolution {
            input: "x".into(),
            chain_id: "eip155:1".into(),
            symbol: String::new(),
            asset_id: String::new(),
            address: String::new(),
            decimals: 0,
            resolved_by: String::new(),
            unambiguous: false,
        };
        let v = serde_json::to_value(&a).expect("serialize");
        assert!(v.as_object().unwrap().contains_key("unambiguous"));
        assert_eq!(v["unambiguous"], false);
        assert!(v.as_object().unwrap().contains_key("decimals"));
    }

    // --- 4. float formatting parity (spec §7 — load-bearing) ----------------

    #[test]
    fn integer_valued_float_renders_without_fraction() {
        // Go encoding/json: 2.0 -> "2", 100.0 -> "100", 0.0 -> "0".
        // A struct field carrying an integer-valued APY must match Go.
        let m = LendMarket {
            protocol: "aave-v3".into(),
            provider: "aave".into(),
            chain_id: "eip155:1".into(),
            asset_id: "x".into(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            supply_apy: 2.0,
            borrow_apy: 0.0,
            tvl_usd: 100.0,
            liquidity_usd: 1234567.0,
            source_url: String::new(),
            fetched_at: "t".into(),
        };
        let s = serde_json::to_string(&m).expect("serialize");
        assert!(
            s.contains("\"supply_apy\":2,"),
            "2.0 must render as 2 (Go parity), got: {s}"
        );
        assert!(
            s.contains("\"borrow_apy\":0,"),
            "0.0 must render as 0 (Go parity), got: {s}"
        );
        assert!(
            s.contains("\"tvl_usd\":100,"),
            "100.0 must render as 100 (Go parity), got: {s}"
        );
        assert!(
            s.contains("\"liquidity_usd\":1234567,"),
            "1234567.0 must render as 1234567 (Go parity), got: {s}"
        );
        // It must NOT contain serde's default ".0" rendering.
        assert!(
            !s.contains("2.0") && !s.contains("100.0") && !s.contains("1234567.0"),
            "no serde-default .0 float rendering allowed, got: {s}"
        );
    }

    #[test]
    fn fractional_and_negative_floats_preserved() {
        // Go: 2.3 -> "2.3", -3.0 -> "-3", 0.0001 -> "0.0001".
        let pt = YieldHistoryPoint {
            timestamp: "2026-05-28T00:00:00Z".into(),
            value: 2.3,
        };
        let s = serde_json::to_string(&pt).expect("serialize");
        assert!(s.contains("\"value\":2.3"), "fractional preserved: {s}");

        let neg = YieldHistoryPoint {
            timestamp: "t".into(),
            value: -3.0,
        };
        let s = serde_json::to_string(&neg).expect("serialize");
        assert!(
            s.contains("\"value\":-3"),
            "negative whole renders as -3 (Go parity): {s}"
        );

        let small = YieldHistoryPoint {
            timestamp: "t".into(),
            value: 0.0001,
        };
        let s = serde_json::to_string(&small).expect("serialize");
        assert!(s.contains("\"value\":0.0001"), "small fractional: {s}");
    }

    #[test]
    fn apy_values_are_percentage_points_not_ratios() {
        // Contract: APY 2.3 means 2.3% (not 0.023). The value is stored/rendered
        // verbatim as a percentage point. (Guards against accidental /100.)
        let r = LendRate {
            protocol: "aave-v3".into(),
            provider: "aave".into(),
            chain_id: "eip155:1".into(),
            asset_id: "x".into(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            supply_apy: 2.3,
            borrow_apy: 4.5,
            utilization: 80.0,
            source_url: String::new(),
            fetched_at: "t".into(),
        };
        let v = serde_json::to_value(&r).expect("serialize");
        assert_eq!(v["supply_apy"], json!(2.3));
        assert_eq!(v["borrow_apy"], json!(4.5));
        // utilization 80.0 renders as 80 (integer-valued float parity).
        let s = serde_json::to_string(&r).expect("serialize");
        assert!(
            s.contains("\"utilization\":80,"),
            "utilization 80.0 -> 80 (Go parity): {s}"
        );
    }

    // --- 5. CAIP / amount consistency ---------------------------------------

    #[test]
    fn amount_info_carries_base_decimal_and_decimals_together() {
        let a = AmountInfo {
            amount_base_units: "1000000".into(),
            amount_decimal: "1".into(),
            decimals: 6,
        };
        let v = serde_json::to_value(&a).expect("serialize");
        // All three present and in declaration order (spec §2.4).
        assert_eq!(
            ordered_keys(&v),
            vec!["amount_base_units", "amount_decimal", "decimals"],
        );
        assert_eq!(v["amount_base_units"], "1000000");
        assert_eq!(v["amount_decimal"], "1");
        assert_eq!(v["decimals"], 6);
    }

    #[test]
    fn asset_id_is_caip19_and_round_trips() {
        let caip = "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
        let a = AssetResolution {
            input: "USDC".into(),
            chain_id: "eip155:1".into(),
            symbol: "USDC".into(),
            asset_id: caip.into(),
            address: "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".into(),
            decimals: 6,
            resolved_by: "registry".into(),
            unambiguous: true,
        };
        let s = serde_json::to_string(&a).expect("serialize");
        let back: AssetResolution = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(back.asset_id, caip);
        assert_eq!(back.chain_id, "eip155:1");
    }

    // --- 6. golden parity (byte-for-byte against Go fixture) -----------------

    #[test]
    fn asset_resolution_matches_go_golden_results_only() {
        // Go-captured fixture rust/tests/golden/assets-resolve-usdc-results-only.json
        // (the `data` block of `assets resolve USDC --chain 1 --results-only`).
        // 2-space-indent + declaration order are part of the contract and MUST
        // match byte-for-byte.
        let expected = r#"{
  "input": "USDC",
  "chain_id": "eip155:1",
  "symbol": "USDC",
  "asset_id": "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
  "address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
  "decimals": 6,
  "resolved_by": "registry",
  "unambiguous": true
}"#;
        let a = AssetResolution {
            input: "USDC".into(),
            chain_id: "eip155:1".into(),
            symbol: "USDC".into(),
            asset_id: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".into(),
            address: "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".into(),
            decimals: 6,
            resolved_by: "registry".into(),
            unambiguous: true,
        };
        let rendered = serde_json::to_string_pretty(&a).expect("render");
        assert_eq!(rendered, expected);
    }

    // --- 7. round-trip -------------------------------------------------------

    #[test]
    fn lend_market_round_trips_value_identical() {
        let canonical = r#"{
  "protocol": "aave-v3",
  "provider": "aave",
  "chain_id": "eip155:1",
  "asset_id": "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
  "supply_apy": 2.3,
  "borrow_apy": 4,
  "tvl_usd": 1000000,
  "liquidity_usd": 500000,
  "fetched_at": "2026-05-28T18:48:18Z"
}"#;
        let m: LendMarket = serde_json::from_str(canonical).expect("deserialize");
        // Re-serialize: must be byte-identical (omitempty + float parity + order).
        let rendered = serde_json::to_string_pretty(&m).expect("render");
        assert_eq!(rendered, canonical);
    }

    #[test]
    fn yield_opportunity_round_trips_value_identical() {
        let original = YieldOpportunity {
            opportunity_id: "opp-1".into(),
            provider: "aave".into(),
            protocol: "aave-v3".into(),
            chain_id: "eip155:1".into(),
            asset_id: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".into(),
            provider_native_id: "native".into(),
            provider_native_id_kind: crate::NATIVE_ID_KIND_POOL_ID.into(),
            opportunity_type: "lending".into(),
            apy_base: 2.0,
            apy_reward: 0.5,
            apy_total: 2.5,
            tvl_usd: 1000.0,
            liquidity_usd: 500.0,
            lockup_days: 0.0,
            withdrawal_terms: "instant".into(),
            backing_assets: vec![YieldBackingAsset {
                asset_id: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".into(),
                symbol: "USDC".into(),
                share_pct: 100.0,
            }],
            source_url: "https://example.com".into(),
            fetched_at: "2026-05-28T18:48:18Z".into(),
        };
        let s = serde_json::to_string(&original).expect("serialize");
        let back: YieldOpportunity = serde_json::from_str(&s).expect("deserialize");
        let s2 = serde_json::to_string(&back).expect("re-serialize");
        assert_eq!(s, s2, "round-trip is byte-identical");
        // The renamed `type` key must survive the round-trip.
        let v: Value = serde_json::from_str(&s2).expect("value");
        assert_eq!(v["type"], "lending");
    }
}
