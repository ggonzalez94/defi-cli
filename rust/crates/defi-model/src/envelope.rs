//! Output envelope and metadata.
//!
//! Field declaration order and `rename`/`skip_serializing_if` mirror
//! `internal/model/types.go` exactly (machine contract — spec §2.1).

use serde::{Deserialize, Serialize};

/// The top-level output envelope.
///
/// `data` is omitted when empty; `error` is always present (null on success);
/// `warnings`/`providers` are omitted when empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub version: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    pub error: Option<ErrorBody>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub warnings: Vec<String>,
    pub meta: EnvelopeMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: i64,
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeMeta {
    pub request_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub command: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub providers: Vec<ProviderStatus>,
    pub cache: CacheStatus,
    pub partial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub name: String,
    pub status: String,
    pub latency_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStatus {
    pub status: String,
    pub age_ms: i64,
    pub stale: bool,
}

impl CacheStatus {
    /// The cache status used by metadata/execution/error envelopes that bypass
    /// the cache entirely (`status="bypass"`, `age_ms=0`, `stale=false`).
    pub fn bypass() -> Self {
        CacheStatus {
            status: "bypass".to_string(),
            age_ms: 0,
            stale: false,
        }
    }
}

impl Envelope {
    /// Build a success envelope (`success=true`, `error=null`).
    ///
    /// Mirrors the Go runner's `emitSuccess` construction site: `data` is the
    /// payload placed verbatim into `data`, and `meta` carries the resolved
    /// `cache`/`providers`/`partial` state for the request.
    pub fn success(
        command: impl Into<String>,
        data: serde_json::Value,
        warnings: Vec<String>,
        cache: CacheStatus,
        providers: Vec<ProviderStatus>,
        partial: bool,
    ) -> Self {
        Envelope {
            version: crate::ENVELOPE_VERSION.to_string(),
            success: true,
            data: Some(data),
            error: None,
            warnings,
            meta: EnvelopeMeta {
                request_id: String::new(),
                timestamp: chrono::Utc::now(),
                command: command.into(),
                providers,
                cache,
                partial,
            },
        }
    }

    /// Build an error envelope (`success=false`, `data=[]`, `error` set,
    /// `cache.status="bypass"`).
    ///
    /// Mirrors the Go runner's `renderError` construction site: error output
    /// always carries the full envelope (even with `--results-only`/`--select`),
    /// with `data` set to an empty array and the cache bypassed.
    pub fn error(
        command: impl Into<String>,
        error: ErrorBody,
        warnings: Vec<String>,
        providers: Vec<ProviderStatus>,
        partial: bool,
    ) -> Self {
        Envelope {
            version: crate::ENVELOPE_VERSION.to_string(),
            success: false,
            data: Some(serde_json::Value::Array(Vec::new())),
            error: Some(error),
            warnings,
            meta: EnvelopeMeta {
                request_id: String::new(),
                timestamp: chrono::Utc::now(),
                command: command.into(),
                providers,
                cache: CacheStatus::bypass(),
                partial,
            },
        }
    }

    /// Render the envelope as canonical pretty JSON: 2-space indent with struct
    /// field declaration order preserved (machine contract — spec §2.3).
    pub fn to_pretty_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-model::envelope` (Go: `internal/model/types.go`)
    //!
    //! This module owns the top-level machine **envelope** and its `meta` block.
    //! The Rust port is "correct" iff it preserves the stable machine contract
    //! (design spec §2.1 / §2.3). These tests assert the contract, not Go
    //! internals:
    //!
    //! 1. **Envelope shape & field DECLARATION order.** Serialized JSON keys
    //!    appear in struct declaration order: `version, success, data?, error,
    //!    warnings?, meta`. `meta` keys: `request_id, timestamp, command,
    //!    providers?, cache, partial`. `cache` keys: `status, age_ms, stale`.
    //!    `error` body keys: `code, type, message` (note JSON key `type`).
    //! 2. **Omit-empty semantics (Go `omitempty`).** `data` is omitted when
    //!    empty/absent; `warnings` omitted when empty; `meta.providers` omitted
    //!    when empty. `error` is ALWAYS present (serialized as `null` on success).
    //!    `meta.cache` and `meta.partial` are ALWAYS present (no omitempty).
    //! 3. **`EnvelopeVersion == "v1"`** and the four `NATIVE_ID_KIND_*` constants
    //!    have their exact contract string values.
    //! 4. **Timestamp format.** `meta.timestamp` serializes as RFC3339 UTC with a
    //!    `Z` suffix (matching Go `time.Time` JSON), and round-trips.
    //! 5. **Ergonomic constructors** mirror the two Go runner construction sites
    //!    (`emitSuccess` / `renderError`): `Envelope::success(...)` builds a
    //!    success envelope (`success=true`, `error=null`); `Envelope::error(...)`
    //!    builds an error envelope (`success=false`, `data=[]`, `error` set,
    //!    `cache.status="bypass"`). These are the public surface this module owns.
    //! 6. **2-space-indent canonical JSON.** `Envelope::to_pretty_json` renders
    //!    with serde_json 2-space indent + preserved declaration order; the bytes
    //!    of an error envelope match the Go golden fixture after volatile-field
    //!    normalization (`meta.request_id`, `meta.timestamp`).
    //! 7. **Round-trip.** An envelope deserialized from canonical JSON and
    //!    re-serialized is byte-identical (declaration order is stable both ways).

    use super::*;
    use serde_json::{json, Value};

    // --- helpers ------------------------------------------------------------

    fn fixed_ts() -> chrono::DateTime<chrono::Utc> {
        // 2026-05-28T18:48:18.949627Z (matches the Go golden fixture instant).
        chrono::DateTime::parse_from_rfc3339("2026-05-28T18:48:18.949627Z")
            .expect("valid rfc3339")
            .with_timezone(&chrono::Utc)
    }

    /// Ordered list of top-level JSON keys in serialization order.
    fn ordered_keys(v: &Value) -> Vec<String> {
        v.as_object()
            .expect("expected JSON object")
            .keys()
            .cloned()
            .collect()
    }

    // --- 3. constants -------------------------------------------------------

    #[test]
    fn envelope_version_is_v1() {
        assert_eq!(crate::ENVELOPE_VERSION, "v1");
    }

    #[test]
    fn native_id_kind_constants_match_contract() {
        assert_eq!(
            crate::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET,
            "composite_market_asset"
        );
        assert_eq!(crate::NATIVE_ID_KIND_MARKET_ID, "market_id");
        assert_eq!(crate::NATIVE_ID_KIND_VAULT_ADDRESS, "vault_address");
        assert_eq!(crate::NATIVE_ID_KIND_POOL_ID, "pool_id");
    }

    // --- 5. constructors ----------------------------------------------------

    #[test]
    fn success_constructor_sets_invariants() {
        let env = Envelope::success(
            "chains list",
            json!([{"name": "Ethereum"}]),
            vec![],
            CacheStatus {
                status: "bypass".into(),
                age_ms: 0,
                stale: false,
            },
            vec![],
            false,
        );
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none(), "success envelope has null error");
        assert_eq!(env.meta.command, "chains list");
        assert!(env.data.is_some());
    }

    #[test]
    fn error_constructor_sets_invariants() {
        let env = Envelope::error(
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
        assert!(!env.success);
        let err = env.error.as_ref().expect("error envelope has error body");
        assert_eq!(err.code, 2);
        assert_eq!(err.error_type, "usage_error");
        // Error envelope must carry data = [] (empty array), and bypass cache.
        let v = serde_json::to_value(&env).expect("serialize");
        assert_eq!(v["data"], json!([]), "error envelope data is []");
        assert_eq!(v["meta"]["cache"]["status"], "bypass");
    }

    // --- 1. field declaration order -----------------------------------------

    #[test]
    fn envelope_top_level_field_order() {
        let env = Envelope::success(
            "chains list",
            json!([{"name": "Ethereum"}]),
            vec!["w1".into()], // non-empty so `warnings` is present
            CacheStatus {
                status: "bypass".into(),
                age_ms: 0,
                stale: false,
            },
            vec![ProviderStatus {
                name: "p".into(),
                status: "ok".into(),
                latency_ms: 0,
            }],
            false,
        );
        let v = serde_json::to_value(&env).expect("serialize");
        assert_eq!(
            ordered_keys(&v),
            vec!["version", "success", "data", "error", "warnings", "meta"],
        );
    }

    #[test]
    fn meta_field_order() {
        let meta = EnvelopeMeta {
            request_id: "r".into(),
            timestamp: fixed_ts(),
            command: "chains list".into(),
            providers: vec![ProviderStatus {
                name: "p".into(),
                status: "ok".into(),
                latency_ms: 0,
            }],
            cache: CacheStatus {
                status: "bypass".into(),
                age_ms: 0,
                stale: false,
            },
            partial: false,
        };
        let v = serde_json::to_value(&meta).expect("serialize");
        assert_eq!(
            ordered_keys(&v),
            vec![
                "request_id",
                "timestamp",
                "command",
                "providers",
                "cache",
                "partial"
            ],
        );
    }

    #[test]
    fn cache_status_field_order_and_keys() {
        let c = CacheStatus {
            status: "hit".into(),
            age_ms: 1234,
            stale: true,
        };
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(ordered_keys(&v), vec!["status", "age_ms", "stale"]);
    }

    #[test]
    fn provider_status_field_order_and_keys() {
        let p = ProviderStatus {
            name: "aave".into(),
            status: "ok".into(),
            latency_ms: 42,
        };
        let v = serde_json::to_value(&p).expect("serialize");
        assert_eq!(ordered_keys(&v), vec!["name", "status", "latency_ms"]);
    }

    #[test]
    fn error_body_uses_json_key_type() {
        let e = ErrorBody {
            code: 10,
            error_type: "auth_error".into(),
            message: "missing key".into(),
        };
        let v = serde_json::to_value(&e).expect("serialize");
        assert_eq!(ordered_keys(&v), vec!["code", "type", "message"]);
        assert_eq!(v["type"], "auth_error");
    }

    // --- 2. omit-empty semantics --------------------------------------------

    #[test]
    fn empty_warnings_and_providers_are_omitted() {
        let env = Envelope::success(
            "chains list",
            json!([{"name": "Ethereum"}]),
            vec![], // empty warnings -> omitted
            CacheStatus {
                status: "bypass".into(),
                age_ms: 0,
                stale: false,
            },
            vec![], // empty providers -> omitted
            false,
        );
        let v = serde_json::to_value(&env).expect("serialize");
        let obj = v.as_object().expect("object");
        assert!(!obj.contains_key("warnings"), "empty warnings omitted");
        assert!(
            !v["meta"].as_object().unwrap().contains_key("providers"),
            "empty providers omitted"
        );
    }

    #[test]
    fn error_is_always_present_as_null_on_success() {
        let env = Envelope::success(
            "chains list",
            json!([]),
            vec![],
            CacheStatus {
                status: "bypass".into(),
                age_ms: 0,
                stale: false,
            },
            vec![],
            false,
        );
        let v = serde_json::to_value(&env).expect("serialize");
        let obj = v.as_object().expect("object");
        assert!(obj.contains_key("error"), "error key always present");
        assert_eq!(v["error"], Value::Null, "error is null on success");
    }

    #[test]
    fn cache_and_partial_always_present_in_meta() {
        let env = Envelope::success(
            "chains list",
            json!([]),
            vec![],
            CacheStatus {
                status: "miss".into(),
                age_ms: 0,
                stale: false,
            },
            vec![],
            false,
        );
        let v = serde_json::to_value(&env).expect("serialize");
        let meta = v["meta"].as_object().expect("meta object");
        assert!(meta.contains_key("cache"), "cache always present");
        assert!(meta.contains_key("partial"), "partial always present");
    }

    // --- 4. timestamp format ------------------------------------------------

    #[test]
    fn timestamp_serializes_as_rfc3339_z() {
        let env = Envelope::success(
            "chains list",
            json!([]),
            vec![],
            CacheStatus {
                status: "bypass".into(),
                age_ms: 0,
                stale: false,
            },
            vec![],
            false,
        );
        // Force the deterministic instant for assertion.
        let mut env = env;
        env.meta.timestamp = fixed_ts();
        let v = serde_json::to_value(&env).expect("serialize");
        let ts = v["meta"]["timestamp"].as_str().expect("timestamp string");
        assert!(ts.ends_with('Z'), "timestamp ends with Z, got {ts}");
        assert!(
            ts.starts_with("2026-05-28T18:48:18"),
            "timestamp preserved, got {ts}"
        );
        // Round-trips back to the same instant.
        let parsed = chrono::DateTime::parse_from_rfc3339(ts)
            .expect("rfc3339 round-trip")
            .with_timezone(&chrono::Utc);
        assert_eq!(parsed, fixed_ts());
    }

    // --- 6 & 7. canonical 2-space JSON + round-trip -------------------------

    #[test]
    fn to_pretty_json_uses_two_space_indent_declaration_order() {
        let env = Envelope::error(
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
        let mut env = env;
        env.meta.request_id = "968d4eba20cf5a05f90de5a0d4008d85".into();
        env.meta.timestamp = fixed_ts();

        let rendered = env.to_pretty_json().expect("render");

        // 2-space indent: top-level keys are indented exactly two spaces.
        assert!(
            rendered.contains("\n  \"version\": \"v1\""),
            "2-space indent for top-level keys:\n{rendered}"
        );
        // Declaration order: "version" precedes "success" precedes "data" ...
        let iv = rendered.find("\"version\"").unwrap();
        let is = rendered.find("\"success\"").unwrap();
        let id = rendered.find("\"data\"").unwrap();
        let ie = rendered.find("\"error\"").unwrap();
        let im = rendered.find("\"meta\"").unwrap();
        assert!(
            iv < is && is < id && id < ie && ie < im,
            "declaration order"
        );
    }

    #[test]
    fn pretty_json_matches_go_golden_error_envelope() {
        // Go golden fixture (rust/tests/golden/error-usage-bad-chain.json),
        // normalized per the documented volatile-field rules
        // (meta.request_id / meta.timestamp set to the fixed sentinels used
        // when constructing the envelope below). Declaration order + 2-space
        // indent are part of the contract and MUST match byte-for-byte.
        let expected = r#"{
  "version": "v1",
  "success": false,
  "data": [],
  "error": {
    "code": 2,
    "type": "usage_error",
    "message": "unsupported chain input: notarealchain"
  },
  "meta": {
    "request_id": "968d4eba20cf5a05f90de5a0d4008d85",
    "timestamp": "2026-05-28T18:48:18.949627Z",
    "command": "assets resolve",
    "cache": {
      "status": "bypass",
      "age_ms": 0,
      "stale": false
    },
    "partial": false
  }
}"#;

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
        env.meta.request_id = "968d4eba20cf5a05f90de5a0d4008d85".into();
        env.meta.timestamp = fixed_ts();

        assert_eq!(env.to_pretty_json().expect("render"), expected);
    }

    #[test]
    fn canonical_json_round_trips_byte_identical() {
        let canonical = r#"{
  "version": "v1",
  "success": true,
  "data": [
    {
      "name": "Ethereum"
    }
  ],
  "error": null,
  "meta": {
    "request_id": "abc",
    "timestamp": "2026-05-28T18:48:18.949627Z",
    "command": "chains list",
    "cache": {
      "status": "bypass",
      "age_ms": 0,
      "stale": false
    },
    "partial": false
  }
}"#;
        let env: Envelope = serde_json::from_str(canonical).expect("deserialize");
        assert_eq!(env.to_pretty_json().expect("render"), canonical);
    }
}
