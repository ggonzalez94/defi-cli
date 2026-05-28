//! Action builder traits (cycle break).
//!
//! In Go, `internal/providers` defined `BuildSwapAction`/`BuildBridgeAction` on
//! provider interfaces while depending on `internal/execution`. Rust forbids
//! dependency cycles, so the builder traits — and the request/option types they
//! take — are defined HERE; `defi-providers` implements them (spec §3, locked
//! interface §"Interface contracts locked at scaffold").

use crate::action::Action;
use async_trait::async_trait;
use defi_errors::Error;
use defi_id::{Asset, Chain};

/// Swap trade direction. Defaults to exact-input (matches Go default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SwapTradeType {
    #[default]
    ExactInput,
    ExactOutput,
}

/// Parameters for a swap quote/build (mirrors Go `SwapQuoteRequest`).
#[derive(Debug, Clone, Default)]
pub struct SwapQuoteRequest {
    pub chain: Chain,
    pub from_asset: Asset,
    pub to_asset: Asset,
    pub amount_base_units: String,
    pub amount_decimal: String,
    pub rpc_url: String,
    pub trade_type: SwapTradeType,
    pub slippage_pct: Option<f64>,
    pub swapper: String,
}

/// Swap execution options (mirrors Go `SwapExecutionOptions`).
#[derive(Debug, Clone, Default)]
pub struct SwapExecutionOptions {
    pub sender: String,
    pub recipient: String,
    pub slippage_bps: i64,
    pub simulate: bool,
    pub rpc_url: String,
}

/// Parameters for a bridge quote/build (mirrors Go `BridgeQuoteRequest`).
#[derive(Debug, Clone, Default)]
pub struct BridgeQuoteRequest {
    pub from_chain: Chain,
    pub to_chain: Chain,
    pub from_asset: Asset,
    pub to_asset: Asset,
    pub amount_base_units: String,
    pub amount_decimal: String,
    pub from_amount_for_gas: String,
}

/// Bridge execution options (mirrors Go `BridgeExecutionOptions`).
#[derive(Debug, Clone, Default)]
pub struct BridgeExecutionOptions {
    pub sender: String,
    pub recipient: String,
    pub slippage_bps: i64,
    pub simulate: bool,
    pub rpc_url: String,
    pub from_amount_for_gas: String,
}

/// Provider capability: build an executable swap [`Action`] from a quote
/// request (mirrors Go `SwapExecutionProvider.BuildSwapAction`).
#[async_trait]
pub trait SwapActionBuilder: Send + Sync {
    async fn build_swap_action(
        &self,
        req: SwapQuoteRequest,
        opts: SwapExecutionOptions,
    ) -> Result<Action, Error>;
}

/// Provider capability: build an executable bridge [`Action`] from a quote
/// request (mirrors Go `BridgeExecutionProvider.BuildBridgeAction`).
#[async_trait]
pub trait BridgeActionBuilder: Send + Sync {
    async fn build_bridge_action(
        &self,
        req: BridgeQuoteRequest,
        opts: BridgeExecutionOptions,
    ) -> Result<Action, Error>;
}
