//! Golden-parity tests for the `envelope` module against the Go reference oracle.
//!
//! # Success criteria
//!
//! The Rust `Envelope` renderer must reproduce the Go binary's full-envelope
//! JSON **byte-for-byte** after volatile-field normalization (design spec §2.1 /
//! §2.3; golden README under `rust/tests/golden/`). These tests load the actual
//! Go-captured fixtures from `rust/tests/golden/` and assert:
//!
//! - The error envelope (`error-usage-bad-chain.json`) re-renders identically
//!   from a Rust `Envelope` constructed with the fixture's stable fields, after
//!   blanking the documented volatile paths (`meta.request_id`,
//!   `meta.timestamp`, `meta.cache.age_ms`) on both sides.
//! - Field declaration order is preserved (NOT alphabetical) — `serde_json` with
//!   `preserve_order` keeps struct declaration order. The error body uses the
//!   JSON key `type`.
//! - The error envelope carries `success=false`, `data=[]`, `error` set, and
//!   `cache.status="bypass"` (full envelope on error, regardless of flags).

use serde_json::Value;

const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden");

/// Blank the documented volatile JSON paths so the comparison is deterministic.
fn normalize(v: &mut Value) {
    if let Some(meta) = v.get_mut("meta").and_then(Value::as_object_mut) {
        if meta.contains_key("request_id") {
            meta.insert("request_id".into(), Value::String("<request_id>".into()));
        }
        if meta.contains_key("timestamp") {
            meta.insert("timestamp".into(), Value::String("<timestamp>".into()));
        }
        if let Some(cache) = meta.get_mut("cache").and_then(Value::as_object_mut) {
            if cache.contains_key("age_ms") {
                cache.insert("age_ms".into(), Value::from(0));
            }
        }
        if let Some(providers) = meta.get_mut("providers").and_then(Value::as_array_mut) {
            for p in providers.iter_mut() {
                if let Some(obj) = p.as_object_mut() {
                    if obj.contains_key("latency_ms") {
                        obj.insert("latency_ms".into(), Value::from(0));
                    }
                }
            }
        }
    }
}

fn load_golden(slug: &str) -> String {
    let path = format!("{GOLDEN_DIR}/{slug}.json");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read golden {path}: {e}"))
}

#[test]
fn error_envelope_matches_go_golden_after_normalization() {
    use defi_model::{CacheStatus, Envelope, ErrorBody};

    // Build the Rust envelope from the stable fields of the Go fixture.
    let mut env = Envelope::error(
        "assets resolve",
        ErrorBody {
            code: 2,
            error_type: "usage_error".into(),
            message: "unsupported chain input: notarealchain".into(),
        },
        vec![],
        vec![],
        false,
    );
    env.meta.request_id = "rust-side".into();
    env.meta.timestamp = chrono::Utc::now();
    // Sanity: error envelopes bypass cache (matches the Go fixture).
    assert_eq!(env.meta.cache.status, "bypass");
    let _ = CacheStatus {
        status: "bypass".into(),
        age_ms: 0,
        stale: false,
    };

    let rust_rendered = env.to_pretty_json().expect("render");

    let mut rust_value: Value = serde_json::from_str(&rust_rendered).expect("rust json");
    let mut go_value: Value =
        serde_json::from_str(&load_golden("error-usage-bad-chain")).expect("go json");
    normalize(&mut rust_value);
    normalize(&mut go_value);

    assert_eq!(
        rust_value, go_value,
        "structural parity with Go error envelope"
    );

    // Declaration order is part of the contract: keys must NOT be alphabetical.
    let keys: Vec<&str> = rust_value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec!["version", "success", "data", "error", "warnings", "meta"]
            .into_iter()
            .filter(|k| rust_value.as_object().unwrap().contains_key(*k))
            .collect::<Vec<_>>(),
    );
}

#[test]
fn go_golden_error_envelope_has_full_envelope_shape() {
    // Independent of the Rust code: assert the contract the fixture encodes, so
    // the Rust constructor is held to the same shape.
    let go: Value = serde_json::from_str(&load_golden("error-usage-bad-chain")).expect("go json");
    assert_eq!(go["version"], "v1");
    assert_eq!(go["success"], false);
    assert_eq!(go["data"], serde_json::json!([]));
    assert_eq!(go["error"]["code"], 2);
    assert_eq!(go["error"]["type"], "usage_error");
    assert_eq!(go["meta"]["cache"]["status"], "bypass");

    // Error body field order: code, type, message.
    let err_keys: Vec<&str> = go["error"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(err_keys, vec!["code", "type", "message"]);
}
