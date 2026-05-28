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
    pub tvl_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainAssetTvl {
    pub rank: i64,
    pub chain: String,
    pub chain_id: String,
    pub asset: String,
    pub asset_id: String,
    pub tvl_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolTvl {
    pub rank: i64,
    pub protocol: String,
    pub category: String,
    pub tvl_usd: f64,
    pub chains: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolCategory {
    pub name: String,
    pub protocols: i64,
    pub tvl_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolFees {
    pub rank: i64,
    pub protocol: String,
    pub category: String,
    pub fees_24h_usd: f64,
    pub fees_7d_usd: f64,
    pub fees_30d_usd: f64,
    pub change_1d_pct: f64,
    pub change_7d_pct: f64,
    pub change_1m_pct: f64,
    pub chains: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolRevenue {
    pub rank: i64,
    pub protocol: String,
    pub category: String,
    pub revenue_24h_usd: f64,
    pub revenue_7d_usd: f64,
    pub revenue_30d_usd: f64,
    pub change_1d_pct: f64,
    pub change_7d_pct: f64,
    pub change_1m_pct: f64,
    pub chains: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DexVolume {
    pub rank: i64,
    pub protocol: String,
    pub volume_24h_usd: f64,
    pub volume_7d_usd: f64,
    pub volume_30d_usd: f64,
    pub change_1d_pct: f64,
    pub change_7d_pct: f64,
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
    pub circulating_usd: f64,
    pub price: f64,
    pub chains: i64,
    pub day_change_usd: f64,
    pub week_change_usd: f64,
    pub month_change_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StablecoinChain {
    pub rank: i64,
    pub chain: String,
    pub chain_id: String,
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
    pub supply_apy: f64,
    pub borrow_apy: f64,
    pub tvl_usd: f64,
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
    pub supply_apy: f64,
    pub borrow_apy: f64,
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
    pub amount_usd: f64,
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
    #[serde(skip_serializing_if = "is_zero_f64", default)]
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
    #[serde(skip_serializing_if = "is_zero_f64", default)]
    pub total_fee_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub consistent_with_amount_delta: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeVolumes {
    pub last_hourly_usd: f64,
    pub last_24h_usd: f64,
    pub last_daily_usd: f64,
    pub prev_day_usd: f64,
    pub prev_2d_usd: f64,
    pub weekly_usd: f64,
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
    pub estimated_gas_usd: f64,
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
    pub apy_base: f64,
    pub apy_reward: f64,
    pub apy_total: f64,
    pub tvl_usd: f64,
    pub liquidity_usd: f64,
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
    pub amount_usd: f64,
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
