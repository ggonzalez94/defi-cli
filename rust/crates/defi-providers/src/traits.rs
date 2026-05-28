//! Provider traits — one per Go provider interface (`internal/providers/types.go`).
//!
//! Async via `async-trait` (locked interface §"Interface contracts locked at
//! scaffold"). Swap/bridge request + option types are re-used from
//! `defi-execution` to avoid duplication and the provider↔execution cycle.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use defi_errors::Error;
use defi_execution::{
    BridgeActionBuilder, BridgeQuoteRequest, SwapActionBuilder, SwapQuoteRequest,
};
use defi_id::{Asset, Chain};
use defi_model as model;

/// Base provider: metadata only (mirrors Go `Provider`).
pub trait Provider {
    fn info(&self) -> model::ProviderInfo;
}

/// Market/TVL/stablecoin/fee/revenue/volume data (mirrors `MarketDataProvider`).
#[async_trait]
pub trait MarketDataProvider: Provider + Send + Sync {
    async fn chains_top(&self, limit: i64) -> Result<Vec<model::ChainTvl>, Error>;
    async fn chains_assets(
        &self,
        chain: Chain,
        asset: Asset,
        limit: i64,
    ) -> Result<Vec<model::ChainAssetTvl>, Error>;
    async fn protocols_top(
        &self,
        category: &str,
        chain: &str,
        limit: i64,
    ) -> Result<Vec<model::ProtocolTvl>, Error>;
    async fn protocols_categories(&self) -> Result<Vec<model::ProtocolCategory>, Error>;
    async fn stablecoins_top(
        &self,
        peg_type: &str,
        limit: i64,
    ) -> Result<Vec<model::Stablecoin>, Error>;
    async fn stablecoin_chains(&self, limit: i64) -> Result<Vec<model::StablecoinChain>, Error>;
    async fn protocols_fees(
        &self,
        category: &str,
        chain: &str,
        limit: i64,
    ) -> Result<Vec<model::ProtocolFees>, Error>;
    async fn protocols_revenue(
        &self,
        category: &str,
        chain: &str,
        limit: i64,
    ) -> Result<Vec<model::ProtocolRevenue>, Error>;
    async fn dexes_volume(&self, chain: &str, limit: i64) -> Result<Vec<model::DexVolume>, Error>;
}

/// Lending market/rate reads (mirrors `LendingProvider`).
#[async_trait]
pub trait LendingProvider: Provider + Send + Sync {
    async fn lend_markets(
        &self,
        provider: &str,
        chain: Chain,
        asset: Asset,
    ) -> Result<Vec<model::LendMarket>, Error>;
    async fn lend_rates(
        &self,
        provider: &str,
        chain: Chain,
        asset: Asset,
    ) -> Result<Vec<model::LendRate>, Error>;
}

/// Lending position type filter (mirrors Go `LendPositionType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LendPositionType {
    All,
    Supply,
    Borrow,
    Collateral,
}

/// Lending positions request (mirrors Go `LendPositionsRequest`).
#[derive(Debug, Clone)]
pub struct LendPositionsRequest {
    pub chain: Chain,
    pub account: String,
    pub asset: Asset,
    pub position_type: LendPositionType,
    pub limit: i64,
    /// Optional RPC URL override (on-chain providers like Moonwell).
    pub rpc_url: String,
}

/// Lending positions reads (mirrors `LendingPositionsProvider`).
#[async_trait]
pub trait LendingPositionsProvider: Provider + Send + Sync {
    async fn lend_positions(
        &self,
        req: LendPositionsRequest,
    ) -> Result<Vec<model::LendPosition>, Error>;
}

/// Yield opportunities request (mirrors Go `YieldRequest`).
#[derive(Debug, Clone)]
pub struct YieldRequest {
    pub chain: Chain,
    pub asset: Asset,
    pub limit: i64,
    pub min_tvl_usd: f64,
    pub min_apy: f64,
    pub providers: Vec<String>,
    pub sort_by: String,
    pub include_incomplete: bool,
}

/// Yield opportunity reads (mirrors `YieldProvider`).
#[async_trait]
pub trait YieldProvider: Provider + Send + Sync {
    async fn yield_opportunities(
        &self,
        req: YieldRequest,
    ) -> Result<Vec<model::YieldOpportunity>, Error>;
}

/// Yield positions request (mirrors Go `YieldPositionsRequest`).
#[derive(Debug, Clone)]
pub struct YieldPositionsRequest {
    pub chain: Chain,
    pub account: String,
    pub asset: Asset,
    pub limit: i64,
    pub rpc_url: String,
}

/// Yield positions reads (mirrors `YieldPositionsProvider`).
#[async_trait]
pub trait YieldPositionsProvider: Provider + Send + Sync {
    async fn yield_positions(
        &self,
        req: YieldPositionsRequest,
    ) -> Result<Vec<model::YieldPosition>, Error>;
}

/// Yield history metric (mirrors Go `YieldHistoryMetric`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YieldHistoryMetric {
    ApyTotal,
    TvlUsd,
}

/// Yield history interval (mirrors Go `YieldHistoryInterval`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YieldHistoryInterval {
    Hour,
    Day,
}

/// Yield history request (mirrors Go `YieldHistoryRequest`).
#[derive(Debug, Clone)]
pub struct YieldHistoryRequest {
    pub opportunity: model::YieldOpportunity,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub interval: YieldHistoryInterval,
    pub metrics: Vec<YieldHistoryMetric>,
}

/// Yield history reads (mirrors `YieldHistoryProvider`).
#[async_trait]
pub trait YieldHistoryProvider: Provider + Send + Sync {
    async fn yield_history(
        &self,
        req: YieldHistoryRequest,
    ) -> Result<Vec<model::YieldHistorySeries>, Error>;
}

/// Bridge quote (mirrors `BridgeProvider`).
#[async_trait]
pub trait BridgeProvider: Provider + Send + Sync {
    async fn quote_bridge(&self, req: BridgeQuoteRequest) -> Result<model::BridgeQuote, Error>;
}

/// Bridge quote + executable action build (mirrors `BridgeExecutionProvider`).
#[async_trait]
pub trait BridgeExecutionProvider: BridgeProvider + BridgeActionBuilder {}

/// Bridge analytics list request (mirrors Go `BridgeListRequest`).
#[derive(Debug, Clone)]
pub struct BridgeListRequest {
    pub limit: i64,
    pub include_chains: bool,
}

/// Bridge analytics details request (mirrors Go `BridgeDetailsRequest`).
#[derive(Debug, Clone)]
pub struct BridgeDetailsRequest {
    pub bridge: String,
    pub include_chain_breakdown: bool,
}

/// Bridge analytics reads (mirrors `BridgeDataProvider`).
#[async_trait]
pub trait BridgeDataProvider: Provider + Send + Sync {
    async fn list_bridges(
        &self,
        req: BridgeListRequest,
    ) -> Result<Vec<model::BridgeSummary>, Error>;
    async fn bridge_details(
        &self,
        req: BridgeDetailsRequest,
    ) -> Result<model::BridgeDetails, Error>;
}

/// Swap quote (mirrors `SwapProvider`).
#[async_trait]
pub trait SwapProvider: Provider + Send + Sync {
    async fn quote_swap(&self, req: SwapQuoteRequest) -> Result<model::SwapQuote, Error>;
}

/// Swap quote + executable action build (mirrors `SwapExecutionProvider`).
#[async_trait]
pub trait SwapExecutionProvider: SwapProvider + SwapActionBuilder {}
