//! Execution engine: action persistence, planners, signing, executors.
//!
//! Mirrors `internal/execution`. Also defines the [`SwapActionBuilder`] and
//! [`BridgeActionBuilder`] traits (and their request/option types) here so that
//! the `defi-providers` crate can implement them without creating a dependency
//! cycle (spec §3 — the Go provider↔execution coupling is broken via traits).
#![allow(dead_code, unused)]
// The `defi-providers` RED test docs (in the `builder`/`policy` test modules)
// use a list-continuation indent clippy now flags; the test prose is owned by
// those modules' authors, so allow it crate-wide rather than rewriting fixtures.
#![allow(clippy::doc_overindented_list_items)]

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub mod action;
pub mod builder;
pub mod estimate;
pub mod evm_executor;
pub mod planner;
pub mod policy;
pub mod signer;
pub mod store;
pub mod tempo_executor;

pub use action::{
    new_action_id, Action, ActionStatus, ActionStep, Constraints, ExecutionBackend, StepCall,
    StepStatus, StepType,
};
pub use builder::{
    BridgeActionBuilder, BridgeExecutionOptions, BridgeQuoteRequest, SwapActionBuilder,
    SwapExecutionOptions, SwapQuoteRequest, SwapTradeType,
};

// =============================================================================
// Crate-level execution-option types (single source of truth, the Rust analogue
// of Go's package-scope `ExecuteOptions` / `EstimateOptions` / `StepGasEstimate`
// in `internal/execution/{executor.go,estimate.go,step_executor.go}`). They live
// at the crate root so both [`evm_executor`] and [`tempo_executor`] (and
// [`estimate`]) share one definition.
// =============================================================================

/// Options that drive a single action execution (`execute_action` / per-step
/// `execute_step`). Parity with Go `ExecuteOptions`.
#[derive(Debug, Clone)]
pub struct ExecuteOptions {
    /// When `true`, simulate each step via `eth_call` before submitting.
    pub simulate: bool,
    /// Receipt / settlement poll interval.
    pub poll_interval: Duration,
    /// Per-step timeout for confirmation / settlement.
    pub step_timeout: Duration,
    /// Multiplier applied to the estimated gas (must be `> 1`).
    pub gas_multiplier: f64,
    /// Optional `--max-fee-gwei` override.
    pub max_fee_gwei: String,
    /// Optional `--max-priority-fee-gwei` override.
    pub max_priority_fee_gwei: String,
    /// Opt into larger-than-bounded ERC-20 approvals.
    pub allow_max_approval: bool,
    /// Bypass bridge provider-tx guardrails.
    pub unsafe_provider_tx: bool,
    /// Optional Tempo fee token (Tempo execution only).
    pub fee_token: String,
}

impl Default for ExecuteOptions {
    fn default() -> Self {
        default_execute_options()
    }
}

/// The default execution options, parity with Go `DefaultExecuteOptions`:
/// `simulate = true`, 2s poll, 2min step timeout, `gas_multiplier = 1.2`.
pub fn default_execute_options() -> ExecuteOptions {
    ExecuteOptions {
        simulate: true,
        poll_interval: Duration::from_secs(2),
        step_timeout: Duration::from_secs(120),
        gas_multiplier: 1.2,
        max_fee_gwei: String::new(),
        max_priority_fee_gwei: String::new(),
        allow_max_approval: false,
        unsafe_provider_tx: false,
        fee_token: String::new(),
    }
}

/// Which block tag gas estimation reads against. Parity with Go
/// `EstimateBlockTag`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimateBlockTag {
    /// `latest`.
    Latest,
    /// `pending` (the default).
    Pending,
}

impl EstimateBlockTag {
    /// The RPC string form (`"latest"` / `"pending"`).
    pub fn as_str(self) -> &'static str {
        match self {
            EstimateBlockTag::Latest => "latest",
            EstimateBlockTag::Pending => "pending",
        }
    }

    /// Parse a `--block-tag` flag value, parity with Go
    /// `normalizeEstimateBlockTag`: empty → pending; `pending`/`latest`
    /// (case-insensitive, trimmed) pass through; anything else is a usage error.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(input: &str) -> Result<EstimateBlockTag, defi_errors::Error> {
        match input.trim().to_ascii_lowercase().as_str() {
            "" | "pending" => Ok(EstimateBlockTag::Pending),
            "latest" => Ok(EstimateBlockTag::Latest),
            _ => Err(defi_errors::Error::new(
                defi_errors::Code::Usage,
                "--block-tag must be one of: pending,latest",
            )),
        }
    }
}

/// Options that drive `actions estimate`. Parity with Go `EstimateOptions`.
#[derive(Debug, Clone)]
pub struct EstimateOptions {
    /// Optional step-id filter (case-insensitive, trimmed). Empty = all steps.
    pub step_ids: Vec<String>,
    /// Gas multiplier (must be `> 1`).
    pub gas_multiplier: f64,
    /// Optional `--max-fee-gwei` override.
    pub max_fee_gwei: String,
    /// Optional `--max-priority-fee-gwei` override.
    pub max_priority_fee_gwei: String,
    /// Block tag the estimate reads against.
    pub block_tag: EstimateBlockTag,
}

impl Default for EstimateOptions {
    fn default() -> Self {
        default_estimate_options()
    }
}

/// The default estimate options, parity with Go `DefaultEstimateOptions`:
/// `gas_multiplier = 1.2`, `block_tag = pending`.
pub fn default_estimate_options() -> EstimateOptions {
    EstimateOptions {
        step_ids: Vec::new(),
        gas_multiplier: 1.2,
        max_fee_gwei: String::new(),
        max_priority_fee_gwei: String::new(),
        block_tag: EstimateBlockTag::Pending,
    }
}

/// Gas/fee estimates for a single action step. Parity with Go
/// `StepGasEstimate`; field declaration order + `omitempty` mirror the Go struct
/// (machine contract).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepGasEstimate {
    pub gas_estimate_raw: String,
    pub gas_limit: String,
    pub base_fee_per_gas_wei: String,
    pub max_priority_fee_per_gas_wei: String,
    pub max_fee_per_gas_wei: String,
    pub effective_gas_price_wei: String,
    pub likely_fee_wei: String,
    pub worst_case_fee_wei: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub fee_unit: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub fee_token: String,
}
