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

impl LendPositionType {
    /// Canonical wire string (matches the Go constant values exactly).
    pub fn as_str(self) -> &'static str {
        match self {
            LendPositionType::All => "all",
            LendPositionType::Supply => "supply",
            LendPositionType::Borrow => "borrow",
            LendPositionType::Collateral => "collateral",
        }
    }

    /// Parse a wire string into a [`LendPositionType`].
    ///
    /// Trim- and case-tolerant; unknown input (including empty) returns `None`.
    /// Empty-to-`All` defaulting is the runner's responsibility, not this type's.
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "all" => Some(LendPositionType::All),
            "supply" => Some(LendPositionType::Supply),
            "borrow" => Some(LendPositionType::Borrow),
            "collateral" => Some(LendPositionType::Collateral),
            _ => None,
        }
    }
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

impl YieldHistoryMetric {
    /// Canonical wire string (matches the Go constant values exactly).
    pub fn as_str(self) -> &'static str {
        match self {
            YieldHistoryMetric::ApyTotal => "apy_total",
            YieldHistoryMetric::TvlUsd => "tvl_usd",
        }
    }

    /// Parse a canonical wire string. CSV/dedup/alias handling lives in the
    /// runner; this only round-trips the canonical forms. Unknown returns `None`.
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "apy_total" => Some(YieldHistoryMetric::ApyTotal),
            "tvl_usd" => Some(YieldHistoryMetric::TvlUsd),
            _ => None,
        }
    }
}

/// Yield history interval (mirrors Go `YieldHistoryInterval`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YieldHistoryInterval {
    Hour,
    Day,
}

impl YieldHistoryInterval {
    /// Canonical wire string (matches the Go constant values exactly).
    pub fn as_str(self) -> &'static str {
        match self {
            YieldHistoryInterval::Hour => "hour",
            YieldHistoryInterval::Day => "day",
        }
    }

    /// Parse a canonical wire string. Alias handling (`daily|1d|hourly|1h`)
    /// lives in the runner; this only round-trips the canonical forms.
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "hour" => Some(YieldHistoryInterval::Hour),
            "day" => Some(YieldHistoryInterval::Day),
            _ => None,
        }
    }
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

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for the `defi-providers::traits` module.
    //!
    //! Go source: `internal/providers/types.go` (the provider interfaces +
    //! their shared request/option types + the string-valued enum constants).
    //! That package has NO `*_test.go` files of its own; the contract-relevant
    //! behavior it owns is exercised indirectly by `internal/app/runner.go`
    //! (and `runner_test.go`), which uses the enum constants' STRING forms as
    //! CLI flag defaults, schema enum values, and parse targets:
    //!   * `--type all|supply|borrow|collateral`  (lend positions)
    //!   * `--type exact-input|exact-output`       (swap)
    //!   * `--metrics apy_total,tvl_usd`           (yield history)
    //!   * `--interval hour|day`                   (yield history)
    //!
    //! In Go these are `type X string` newtypes whose constant VALUES are the
    //! wire strings (e.g. `LendPositionTypeAll LendPositionType = "all"`). The
    //! idiomatic Rust port models them as plain enums, so the wire contract is
    //! only preserved if each enum exposes a canonical string form (`as_str`)
    //! and a parser (`parse`) that round-trips those exact byte sequences.
    //! THIS module owns those mappings (alias/normalization logic such as
    //! `daily|1d -> day` lives in `defi-app`, not here, so it is NOT asserted).
    //!
    //! The Rust port of `traits` is "correct" iff:
    //!
    //!  T1. `LendPositionType::as_str` returns EXACTLY the Go constant values
    //!      `all|supply|borrow|collateral` (declaration order: All, Supply,
    //!      Borrow, Collateral). `parse` round-trips each, is case-insensitive
    //!      and trim-tolerant for parsing, and rejects unknown input.
    //!
    //!  T2. `SwapTradeType::as_str` returns EXACTLY `exact-input|exact-output`
    //!      (note the hyphen — NOT `exact_input`). `SwapTradeType::default()`
    //!      is `ExactInput` (Go swap default). `parse` round-trips and an empty
    //!      string parses to the default `ExactInput` (Go runner treats the
    //!      empty `--type` as exact-input).
    //!
    //!  T3. `YieldHistoryMetric::as_str` returns EXACTLY `apy_total|tvl_usd`.
    //!      `parse` round-trips each and rejects unknown input.
    //!
    //!  T4. `YieldHistoryInterval::as_str` returns EXACTLY `hour|day`. `parse`
    //!      round-trips each canonical form.
    //!
    //!  T5. The shared request/option types preserve the Go FIELD set + types
    //!      and have ergonomic construction: `YieldRequest`,
    //!      `LendPositionsRequest`, `YieldPositionsRequest`,
    //!      `BridgeListRequest`, `BridgeDetailsRequest`, `YieldHistoryRequest`
    //!      are constructible and round-trip their scalar fields. (Field
    //!      declaration order mirrors `types.go` so any future serde projection
    //!      keeps contract field order.)
    //!
    //! Go tests intentionally SKIPPED as internal-detail / owned elsewhere:
    //!   * Provider-name alias normalization (`NormalizeLendingProvider` /
    //!     `NormalizeSwapProvider`) -> owned by `defi-providers::normalize`.
    //!   * Interval ALIAS parsing (`daily|1d|hourly|1h`) -> owned by the
    //!     `defi-app` runner (`parseYieldHistoryInterval`), not this module.
    //!   * Metric CSV parsing / dedup -> owned by the `defi-app` runner
    //!     (`parseYieldHistoryMetrics`).
    //!   * The trait METHOD bodies (adapter behavior) -> covered per-provider
    //!     via wiremock in each provider module's own RED suite.

    use super::*;
    // `SwapTradeType` is re-used from `defi-execution` (cycle break); bring it
    // into the test scope explicitly so T2 fails on the missing `as_str`/`parse`
    // contract methods, not on an unresolved type.
    use defi_execution::SwapTradeType;

    // ----- T1: LendPositionType wire strings ------------------------------
    #[test]
    fn lend_position_type_wire_strings_match_go_constants() {
        assert_eq!(LendPositionType::All.as_str(), "all");
        assert_eq!(LendPositionType::Supply.as_str(), "supply");
        assert_eq!(LendPositionType::Borrow.as_str(), "borrow");
        assert_eq!(LendPositionType::Collateral.as_str(), "collateral");
    }

    #[test]
    fn lend_position_type_round_trips() {
        for v in [
            LendPositionType::All,
            LendPositionType::Supply,
            LendPositionType::Borrow,
            LendPositionType::Collateral,
        ] {
            assert_eq!(LendPositionType::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn lend_position_type_parse_is_trim_and_case_insensitive() {
        assert_eq!(
            LendPositionType::parse("  SUPPLY "),
            Some(LendPositionType::Supply)
        );
        assert_eq!(LendPositionType::parse("nonsense"), None);
    }

    // ----- T2: SwapTradeType wire strings + default -----------------------
    #[test]
    fn swap_trade_type_wire_strings_use_hyphen() {
        assert_eq!(SwapTradeType::ExactInput.as_str(), "exact-input");
        assert_eq!(SwapTradeType::ExactOutput.as_str(), "exact-output");
    }

    #[test]
    fn swap_trade_type_default_is_exact_input() {
        assert_eq!(SwapTradeType::default(), SwapTradeType::ExactInput);
    }

    #[test]
    fn swap_trade_type_empty_parses_to_default() {
        // Go runner treats an empty `--type` as exact-input.
        assert_eq!(SwapTradeType::parse(""), Some(SwapTradeType::ExactInput));
        assert_eq!(
            SwapTradeType::parse("exact-output"),
            Some(SwapTradeType::ExactOutput)
        );
        assert_eq!(SwapTradeType::parse("bogus"), None);
    }

    // ----- T3: YieldHistoryMetric wire strings ----------------------------
    #[test]
    fn yield_history_metric_wire_strings_match_go_constants() {
        assert_eq!(YieldHistoryMetric::ApyTotal.as_str(), "apy_total");
        assert_eq!(YieldHistoryMetric::TvlUsd.as_str(), "tvl_usd");
    }

    #[test]
    fn yield_history_metric_round_trips() {
        for v in [YieldHistoryMetric::ApyTotal, YieldHistoryMetric::TvlUsd] {
            assert_eq!(YieldHistoryMetric::parse(v.as_str()), Some(v));
        }
        assert_eq!(YieldHistoryMetric::parse("unknown"), None);
    }

    // ----- T4: YieldHistoryInterval wire strings --------------------------
    #[test]
    fn yield_history_interval_wire_strings_match_go_constants() {
        assert_eq!(YieldHistoryInterval::Hour.as_str(), "hour");
        assert_eq!(YieldHistoryInterval::Day.as_str(), "day");
    }

    #[test]
    fn yield_history_interval_round_trips() {
        for v in [YieldHistoryInterval::Hour, YieldHistoryInterval::Day] {
            assert_eq!(YieldHistoryInterval::parse(v.as_str()), Some(v));
        }
    }

    // ----- T5: shared request/option type shape ---------------------------
    #[test]
    fn yield_request_preserves_scalar_fields() {
        let req = YieldRequest {
            chain: Chain::default(),
            asset: Asset::default(),
            limit: 5,
            min_tvl_usd: 1_000.0,
            min_apy: 2.5,
            providers: vec!["aave".to_string(), "morpho".to_string()],
            sort_by: "apy".to_string(),
            include_incomplete: true,
        };
        assert_eq!(req.limit, 5);
        assert_eq!(req.min_tvl_usd, 1_000.0);
        assert_eq!(req.min_apy, 2.5);
        assert_eq!(req.providers, vec!["aave", "morpho"]);
        assert_eq!(req.sort_by, "apy");
        assert!(req.include_incomplete);
    }

    #[test]
    fn lend_positions_request_carries_type_and_rpc_override() {
        let req = LendPositionsRequest {
            chain: Chain::default(),
            account: "0xabc".to_string(),
            asset: Asset::default(),
            position_type: LendPositionType::Supply,
            limit: 3,
            rpc_url: "https://rpc.example".to_string(),
        };
        assert_eq!(req.position_type, LendPositionType::Supply);
        assert_eq!(req.account, "0xabc");
        assert_eq!(req.limit, 3);
        assert_eq!(req.rpc_url, "https://rpc.example");
    }

    #[test]
    fn yield_positions_request_carries_rpc_override() {
        let req = YieldPositionsRequest {
            chain: Chain::default(),
            account: "0xdef".to_string(),
            asset: Asset::default(),
            limit: 7,
            rpc_url: "https://rpc2.example".to_string(),
        };
        assert_eq!(req.account, "0xdef");
        assert_eq!(req.limit, 7);
        assert_eq!(req.rpc_url, "https://rpc2.example");
    }

    #[test]
    fn bridge_list_and_details_requests_shape() {
        let list = BridgeListRequest {
            limit: 10,
            include_chains: true,
        };
        assert_eq!(list.limit, 10);
        assert!(list.include_chains);

        let details = BridgeDetailsRequest {
            bridge: "across".to_string(),
            include_chain_breakdown: false,
        };
        assert_eq!(details.bridge, "across");
        assert!(!details.include_chain_breakdown);
    }

    // A minimal `YieldOpportunity` so the history-request test stays focused on
    // `interval` + `metrics` (the fields this module owns) without coupling to
    // the model crate's full field set.
    fn sample_opportunity() -> model::YieldOpportunity {
        model::YieldOpportunity {
            opportunity_id: "op_1".to_string(),
            provider: "aave".to_string(),
            protocol: "aave-v3".to_string(),
            chain_id: "eip155:1".to_string(),
            asset_id: "eip155:1/erc20:0x0".to_string(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            opportunity_type: "lending".to_string(),
            apy_base: 0.0,
            apy_reward: 0.0,
            apy_total: 0.0,
            tvl_usd: 0.0,
            liquidity_usd: 0.0,
            lockup_days: 0.0,
            withdrawal_terms: String::new(),
            backing_assets: Vec::new(),
            source_url: String::new(),
            fetched_at: String::new(),
        }
    }

    #[test]
    fn yield_history_request_carries_interval_and_metrics() {
        let req = YieldHistoryRequest {
            opportunity: sample_opportunity(),
            start_time: Utc::now(),
            end_time: Utc::now(),
            interval: YieldHistoryInterval::Hour,
            metrics: vec![YieldHistoryMetric::ApyTotal, YieldHistoryMetric::TvlUsd],
        };
        assert_eq!(req.interval, YieldHistoryInterval::Hour);
        assert_eq!(
            req.metrics,
            vec![YieldHistoryMetric::ApyTotal, YieldHistoryMetric::TvlUsd]
        );
    }
}
