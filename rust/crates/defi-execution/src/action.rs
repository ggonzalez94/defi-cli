//! Action / step types.
//!
//! Field declaration order, `rename`s, and `skip_serializing_if` mirror
//! `internal/execution/types.go` exactly (machine contract).

use serde::{Deserialize, Serialize};

/// Lifecycle status of an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionStatus {
    Planned,
    Running,
    Completed,
    Failed,
}

/// Lifecycle status of a single step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Pending,
    Simulated,
    Submitted,
    Confirmed,
    Failed,
}

/// The kind of on-chain step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepType {
    #[serde(rename = "approval")]
    Approval,
    #[serde(rename = "transfer")]
    Transfer,
    #[serde(rename = "swap")]
    Swap,
    #[serde(rename = "bridge_send")]
    Bridge,
    #[serde(rename = "lend_call")]
    Lend,
    #[serde(rename = "claim")]
    Claim,
}

/// Which signing/execution backend an action targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionBackend {
    #[serde(rename = "ows")]
    Ows,
    #[serde(rename = "legacy_local")]
    LegacyLocal,
    #[serde(rename = "tempo")]
    Tempo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Constraints {
    #[serde(skip_serializing_if = "is_zero_i64", default)]
    pub slippage_bps: i64,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub deadline: String,
    pub simulate: bool,
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

/// A single call within a batched action step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepCall {
    pub target: String,
    pub data: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionStep {
    pub step_id: String,
    #[serde(rename = "type")]
    pub step_type: StepType,
    pub status: StepStatus,
    pub chain_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub rpc_url: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    pub target: String,
    pub data: String,
    pub value: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub calls: Vec<StepCall>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub expected_outputs: Option<StringMap>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub tx_hash: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub error: String,
}

// `expected_outputs` is a `map[string]string` in Go; modeled as a JSON object
// to preserve insertion order via `serde_json`'s `preserve_order`.
type StringMap = serde_json::Map<String, serde_json::Value>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub action_id: String,
    pub intent_type: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider: String,
    pub status: ActionStatus,
    pub chain_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub from_address: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub wallet_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub wallet_name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub execution_backend: Option<ExecutionBackend>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub to_address: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub input_amount: String,
    pub created_at: String,
    pub updated_at: String,
    pub constraints: Constraints,
    pub steps: Vec<ActionStep>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provider_data: Option<serde_json::Map<String, serde_json::Value>>,
}
