//! Action / step types.
//!
//! Field declaration order, `rename`s, and `skip_serializing_if` mirror
//! `internal/execution/types.go` exactly (machine contract).

use rand::RngCore;
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

/// RFC3339 timestamp at the current instant in UTC with seconds precision.
///
/// Matches Go's `time.Now().UTC().Format(time.RFC3339)` (no sub-second
/// fraction, trailing `Z`).
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Generates a fresh action id: `act_` + 32 lowercase hex chars (16 random
/// bytes). Mirrors Go `execution.NewActionID`.
pub fn new_action_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    format!("act_{}", hex::encode(bytes))
}

impl Action {
    /// Constructs a freshly planned action. Mirrors Go `execution.NewAction`:
    /// status is [`ActionStatus::Planned`], `steps` is empty, and
    /// `created_at == updated_at` is the current RFC3339 UTC timestamp.
    pub fn new(
        action_id: impl Into<String>,
        intent_type: impl Into<String>,
        chain_id: impl Into<String>,
        constraints: Constraints,
    ) -> Self {
        let now = now_rfc3339();
        Action {
            action_id: action_id.into(),
            intent_type: intent_type.into(),
            provider: String::new(),
            status: ActionStatus::Planned,
            chain_id: chain_id.into(),
            from_address: String::new(),
            wallet_id: String::new(),
            wallet_name: String::new(),
            execution_backend: None,
            to_address: String::new(),
            input_amount: String::new(),
            created_at: now.clone(),
            updated_at: now,
            constraints,
            steps: Vec::new(),
            metadata: None,
            provider_data: None,
        }
    }

    /// Advances `updated_at` to the current RFC3339 UTC timestamp without
    /// touching any other field. Mirrors Go `(*Action).Touch`.
    pub fn touch(&mut self) {
        self.updated_at = now_rfc3339();
    }
}

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for the `action` module (machine contract — must hold byte-for-byte).
    //!
    //! This module owns the persisted [`Action`] shape and its constructors, mirroring
    //! Go `internal/execution/{action.go,types.go}`. The Rust port is "correct" iff:
    //!
    //! 1. JSON field order == Go struct **declaration order** for `Action`, `ActionStep`,
    //!    `StepCall`, `Constraints` (serde emits declaration order; `serde_json` is built
    //!    with `preserve_order` so `metadata`/`provider_data`/`expected_outputs` keep
    //!    insertion order, not alphabetical).
    //! 2. JSON is rendered with **2-space indentation** by the shared renderer (verified here
    //!    against `serde_json::to_string_pretty`, which is 2-space).
    //! 3. `omitempty` parity:
    //!    - `Constraints.simulate` is ALWAYS present (no skip); `slippage_bps` omitted when 0;
    //!      `deadline` omitted when empty.
    //!    - `ActionStep`: `rpc_url`, `description`, `tx_hash`, `error` omitted when empty;
    //!      `expected_outputs` omitted when `None`; `calls` omitted when empty/None. (Idiomatic
    //!      Rust diverges from Go here: Go's `omitempty` keeps a non-nil empty slice, but the
    //!      persisted shape never relies on that — an empty `calls` is semantically "no calls",
    //!      so `Vec::is_empty` omission is the contract we lock for Rust.)
    //!    - `Action`: `provider`, `from_address`, `wallet_id`, `wallet_name`, `to_address`,
    //!      `input_amount` omitted when empty; `execution_backend`, `metadata`, `provider_data`
    //!      omitted when `None`; `steps` ALWAYS present (even when empty -> `[]`).
    //! 4. Enum wire values: `ActionStatus`/`StepStatus` are lowercase; `StepType` renders
    //!    `approval|transfer|swap|bridge_send|lend_call|claim`; `ExecutionBackend` renders
    //!    `ows|legacy_local|tempo`.
    //! 5. `new_action_id()` returns `act_` + exactly 32 lowercase hex chars (16 random bytes),
    //!    and is unique across calls.
    //! 6. `Action::new(action_id, intent_type, chain_id, constraints)` initializes:
    //!    status = `Planned`, `steps == []`, `created_at == updated_at`, both an RFC3339 UTC
    //!    timestamp (`...Z`, seconds precision, matching Go `time.RFC3339`), and copies through
    //!    the provided id/intent/chain/constraints.
    //! 7. `Action::touch()` advances `updated_at` to "now" (RFC3339 UTC) without changing
    //!    `created_at` or any other field.
    //! 8. Full round-trip: serialize -> deserialize preserves all fields, including wallet
    //!    metadata (`wallet_id`, `wallet_name`, `from_address`, `execution_backend`) and batched
    //!    `calls`.

    use super::*;
    use serde_json::json;

    fn sample_step_with_calls() -> ActionStep {
        ActionStep {
            step_id: "step-1".into(),
            step_type: StepType::Swap,
            status: StepStatus::Pending,
            chain_id: "eip155:4217".into(),
            rpc_url: String::new(),
            description: String::new(),
            target: "0x00000000000000000000000000000000000000aa".into(),
            data: "0x".into(),
            value: "0".into(),
            calls: vec![
                StepCall {
                    target: "0x00000000000000000000000000000000000000bb".into(),
                    data: "0xabcdef".into(),
                    value: "1000".into(),
                },
                StepCall {
                    target: "0x00000000000000000000000000000000000000cc".into(),
                    data: "0x123456".into(),
                    value: "0".into(),
                },
            ],
            expected_outputs: None,
            tx_hash: String::new(),
            error: String::new(),
        }
    }

    // --- Ported from Go: TestActionStepCallsRoundTrip ---
    #[test]
    fn action_step_calls_round_trip() {
        let step = sample_step_with_calls();
        let data = serde_json::to_string(&step).expect("marshal step");
        let decoded: ActionStep = serde_json::from_str(&data).expect("unmarshal step");

        assert_eq!(decoded.calls.len(), 2);
        assert_eq!(decoded.calls[0].target, step.calls[0].target);
        assert_eq!(decoded.calls[0].data, step.calls[0].data);
        assert_eq!(decoded.calls[0].value, step.calls[0].value);
        assert_eq!(decoded.calls[1].target, step.calls[1].target);
    }

    // --- Ported from Go: TestActionStepCallsOmittedWhenEmpty + TestActionStepCallsNilOmitted ---
    // In idiomatic Rust there is no nil-vs-empty distinction: an empty `Vec` is omitted.
    #[test]
    fn action_step_calls_omitted_when_empty() {
        let mut step = sample_step_with_calls();
        step.calls = Vec::new();

        let data = serde_json::to_string(&step).expect("marshal step");
        assert!(
            !data.contains("\"calls\""),
            "expected calls to be omitted from JSON when empty, got: {data}"
        );

        // Round-trips back to an empty Vec.
        let decoded: ActionStep = serde_json::from_str(&data).expect("unmarshal step");
        assert_eq!(decoded.calls.len(), 0);
    }

    // --- Ported from Go: TestActionRoundTripIncludesWalletMetadata ---
    #[test]
    fn action_round_trip_includes_wallet_metadata() {
        let mut action = Action::new(
            "action-wallet-roundtrip",
            "swap",
            "eip155:1",
            Constraints::default(),
        );
        action.from_address = "0x00000000000000000000000000000000000000aa".into();
        action.wallet_id = "wallet-123".into();
        action.wallet_name = "Agent Wallet".into();
        action.execution_backend = Some(ExecutionBackend::Ows);

        let body = serde_json::to_string(&action).expect("marshal action");
        assert!(body.contains("\"wallet_id\":\"wallet-123\""), "got: {body}");
        assert!(
            body.contains("\"wallet_name\":\"Agent Wallet\""),
            "got: {body}"
        );
        assert!(
            body.contains("\"from_address\":\"0x00000000000000000000000000000000000000aa\""),
            "got: {body}"
        );
        assert!(
            body.contains("\"execution_backend\":\"ows\""),
            "got: {body}"
        );

        let decoded: Action = serde_json::from_str(&body).expect("unmarshal action");
        assert_eq!(decoded.wallet_id, action.wallet_id);
        assert_eq!(decoded.wallet_name, action.wallet_name);
        assert_eq!(decoded.execution_backend, action.execution_backend);
        assert_eq!(decoded.from_address, action.from_address);
    }

    // --- Spec-driven: new_action_id format + uniqueness ---
    #[test]
    fn new_action_id_has_act_prefix_and_32_hex_chars() {
        let id = new_action_id();
        assert!(id.starts_with("act_"), "id missing act_ prefix: {id}");
        let hexpart = &id["act_".len()..];
        assert_eq!(
            hexpart.len(),
            32,
            "expected 32 hex chars, got {}",
            hexpart.len()
        );
        assert!(
            hexpart
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "expected lowercase hex chars, got: {hexpart}"
        );
    }

    #[test]
    fn new_action_id_is_unique() {
        let a = new_action_id();
        let b = new_action_id();
        assert_ne!(a, b, "action ids must be unique across calls");
    }

    // --- Spec-driven: NewAction defaults ---
    #[test]
    fn new_action_sets_planned_status_and_empty_steps() {
        let action = Action::new(
            "act_x",
            "lend_supply",
            "eip155:8453",
            Constraints::default(),
        );
        assert_eq!(action.action_id, "act_x");
        assert_eq!(action.intent_type, "lend_supply");
        assert_eq!(action.chain_id, "eip155:8453");
        assert_eq!(action.status, ActionStatus::Planned);
        assert!(action.steps.is_empty());
        assert_eq!(action.created_at, action.updated_at);
        assert!(!action.created_at.is_empty(), "created_at must be set");
    }

    #[test]
    fn new_action_timestamps_are_rfc3339_utc_seconds() {
        let action = Action::new("act_x", "swap", "eip155:1", Constraints::default());
        // Go time.RFC3339 in UTC: seconds precision, trailing `Z`.
        assert!(
            action.created_at.ends_with('Z'),
            "expected UTC `Z` suffix, got: {}",
            action.created_at
        );
        let parsed = chrono::DateTime::parse_from_rfc3339(&action.created_at);
        assert!(
            parsed.is_ok(),
            "created_at not valid RFC3339: {}",
            action.created_at
        );
        // No sub-second fraction (matches Go time.RFC3339 default formatting).
        assert!(
            !action.created_at.contains('.'),
            "expected seconds precision (no fraction), got: {}",
            action.created_at
        );
    }

    // --- Spec-driven: empty steps serialize as `[]`, always present ---
    #[test]
    fn empty_steps_serialize_as_present_empty_array() {
        let action = Action::new("act_x", "swap", "eip155:1", Constraints::default());
        let body = serde_json::to_value(&action).expect("to_value");
        assert_eq!(body["steps"], json!([]), "steps must be present as []");
    }

    // --- Spec-driven: Touch advances updated_at only ---
    #[test]
    fn touch_advances_updated_at_only() {
        let mut action = Action::new("act_x", "swap", "eip155:1", Constraints::default());
        let created = action.created_at.clone();
        // Force a distinct timestamp by injecting an older created/updated then touching.
        action.created_at = "2000-01-01T00:00:00Z".into();
        action.updated_at = "2000-01-01T00:00:00Z".into();
        let original_created = action.created_at.clone();

        action.touch();

        assert_eq!(
            action.created_at, original_created,
            "touch must not change created_at"
        );
        assert_ne!(
            action.updated_at, "2000-01-01T00:00:00Z",
            "touch must advance updated_at"
        );
        assert!(
            chrono::DateTime::parse_from_rfc3339(&action.updated_at).is_ok(),
            "updated_at not RFC3339: {}",
            action.updated_at
        );
        // sanity: original constructor timestamp existed
        assert!(!created.is_empty());
    }

    // --- Spec-driven: Constraints omitempty parity ---
    #[test]
    fn constraints_simulate_always_present_others_omitted_when_zero() {
        let c = Constraints::default();
        let body = serde_json::to_string(&c).expect("marshal constraints");
        assert_eq!(
            body, "{\"simulate\":false}",
            "default Constraints must emit only `simulate`, got: {body}"
        );

        let c2 = Constraints {
            slippage_bps: 50,
            deadline: "2030-01-01T00:00:00Z".into(),
            simulate: true,
        };
        let v = serde_json::to_value(&c2).expect("to_value");
        assert_eq!(v["slippage_bps"], json!(50));
        assert_eq!(v["deadline"], json!("2030-01-01T00:00:00Z"));
        assert_eq!(v["simulate"], json!(true));
    }

    // --- Spec-driven: enum wire values ---
    #[test]
    fn enum_wire_values_match_contract() {
        assert_eq!(
            serde_json::to_value(ActionStatus::Planned).unwrap(),
            json!("planned")
        );
        assert_eq!(
            serde_json::to_value(ActionStatus::Running).unwrap(),
            json!("running")
        );
        assert_eq!(
            serde_json::to_value(ActionStatus::Completed).unwrap(),
            json!("completed")
        );
        assert_eq!(
            serde_json::to_value(ActionStatus::Failed).unwrap(),
            json!("failed")
        );

        assert_eq!(
            serde_json::to_value(StepStatus::Pending).unwrap(),
            json!("pending")
        );
        assert_eq!(
            serde_json::to_value(StepStatus::Simulated).unwrap(),
            json!("simulated")
        );
        assert_eq!(
            serde_json::to_value(StepStatus::Submitted).unwrap(),
            json!("submitted")
        );
        assert_eq!(
            serde_json::to_value(StepStatus::Confirmed).unwrap(),
            json!("confirmed")
        );
        assert_eq!(
            serde_json::to_value(StepStatus::Failed).unwrap(),
            json!("failed")
        );

        assert_eq!(
            serde_json::to_value(StepType::Approval).unwrap(),
            json!("approval")
        );
        assert_eq!(
            serde_json::to_value(StepType::Transfer).unwrap(),
            json!("transfer")
        );
        assert_eq!(serde_json::to_value(StepType::Swap).unwrap(), json!("swap"));
        assert_eq!(
            serde_json::to_value(StepType::Bridge).unwrap(),
            json!("bridge_send")
        );
        assert_eq!(
            serde_json::to_value(StepType::Lend).unwrap(),
            json!("lend_call")
        );
        assert_eq!(
            serde_json::to_value(StepType::Claim).unwrap(),
            json!("claim")
        );

        assert_eq!(
            serde_json::to_value(ExecutionBackend::Ows).unwrap(),
            json!("ows")
        );
        assert_eq!(
            serde_json::to_value(ExecutionBackend::LegacyLocal).unwrap(),
            json!("legacy_local")
        );
        assert_eq!(
            serde_json::to_value(ExecutionBackend::Tempo).unwrap(),
            json!("tempo")
        );
    }

    // --- Spec-driven: Action JSON field DECLARATION order (not alphabetical) ---
    #[test]
    fn action_json_preserves_declaration_order() {
        let mut action = Action::new("act_x", "swap", "eip155:1", Constraints::default());
        // populate every optional field so all keys appear, in declaration order
        action.provider = "aave".into();
        action.from_address = "0xfrom".into();
        action.wallet_id = "w1".into();
        action.wallet_name = "Wallet".into();
        action.execution_backend = Some(ExecutionBackend::Ows);
        action.to_address = "0xto".into();
        action.input_amount = "100".into();
        let mut meta = serde_json::Map::new();
        meta.insert("k".into(), json!("v"));
        action.metadata = Some(meta.clone());
        action.provider_data = Some(meta);

        let body = serde_json::to_string(&action).expect("marshal");
        // Keys in the exact Go struct declaration order.
        let expected_order = [
            "action_id",
            "intent_type",
            "provider",
            "status",
            "chain_id",
            "from_address",
            "wallet_id",
            "wallet_name",
            "execution_backend",
            "to_address",
            "input_amount",
            "created_at",
            "updated_at",
            "constraints",
            "steps",
            "metadata",
            "provider_data",
        ];
        let mut last = 0usize;
        for key in expected_order {
            let needle = format!("\"{key}\":");
            let pos = body.find(&needle).unwrap_or_else(|| {
                panic!("missing key {key} in serialized action: {body}");
            });
            assert!(
                pos >= last,
                "key `{key}` out of declaration order in: {body}"
            );
            last = pos;
        }
    }

    // --- Spec-driven: ActionStep JSON field declaration order ---
    #[test]
    fn action_step_json_preserves_declaration_order() {
        let mut step = sample_step_with_calls();
        step.rpc_url = "https://rpc.example".into();
        step.description = "desc".into();
        let mut outs = serde_json::Map::new();
        outs.insert("amount_out".into(), json!("999"));
        step.expected_outputs = Some(outs);
        step.tx_hash = "0xhash".into();
        step.error = "".into();

        let body = serde_json::to_string(&step).expect("marshal step");
        let expected_order = [
            "step_id",
            "type",
            "status",
            "chain_id",
            "rpc_url",
            "description",
            "target",
            "data",
            "value",
            "calls",
            "expected_outputs",
            "tx_hash",
        ];
        let mut last = 0usize;
        for key in expected_order {
            let needle = format!("\"{key}\":");
            let pos = body
                .find(&needle)
                .unwrap_or_else(|| panic!("missing key {key} in: {body}"));
            assert!(
                pos >= last,
                "key `{key}` out of declaration order in: {body}"
            );
            last = pos;
        }
        // `type` renamed (not `step_type`)
        assert!(body.contains("\"type\":\"swap\""), "got: {body}");
        assert!(
            !body.contains("step_type"),
            "should rename to `type`: {body}"
        );
    }

    // --- Spec-driven: pretty JSON uses 2-space indent ---
    #[test]
    fn pretty_json_uses_two_space_indent() {
        let action = Action::new("act_x", "swap", "eip155:1", Constraints::default());
        let pretty = serde_json::to_string_pretty(&action).expect("pretty");
        // The second line should be indented by exactly two spaces.
        let second_line = pretty.lines().nth(1).expect("at least two lines");
        assert!(
            second_line.starts_with("  ") && !second_line.starts_with("   "),
            "expected 2-space indent, got line: {second_line:?}"
        );
    }

    // --- Spec-driven: metadata / provider_data omitted when None ---
    #[test]
    fn metadata_and_provider_data_omitted_when_none() {
        let action = Action::new("act_x", "swap", "eip155:1", Constraints::default());
        let body = serde_json::to_string(&action).expect("marshal");
        assert!(
            !body.contains("\"metadata\""),
            "metadata should be omitted: {body}"
        );
        assert!(
            !body.contains("\"provider_data\""),
            "provider_data should be omitted: {body}"
        );
        // optional string fields omitted too
        assert!(
            !body.contains("\"provider\""),
            "provider should be omitted: {body}"
        );
        assert!(
            !body.contains("\"from_address\""),
            "from_address omitted: {body}"
        );
        assert!(
            !body.contains("\"execution_backend\""),
            "execution_backend omitted: {body}"
        );
    }
}
