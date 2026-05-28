//! Execution engine: action persistence, planners, signing, executors.
//!
//! Mirrors `internal/execution`. Also defines the [`SwapActionBuilder`] and
//! [`BridgeActionBuilder`] traits (and their request/option types) here so that
//! the `defi-providers` crate can implement them without creating a dependency
//! cycle (spec §3 — the Go provider↔execution coupling is broken via traits).
#![allow(dead_code, unused)]

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
    Action, ActionStatus, ActionStep, Constraints, ExecutionBackend, StepCall, StepStatus, StepType,
};
pub use builder::{
    BridgeActionBuilder, BridgeExecutionOptions, BridgeQuoteRequest, SwapActionBuilder,
    SwapExecutionOptions, SwapQuoteRequest, SwapTradeType,
};
