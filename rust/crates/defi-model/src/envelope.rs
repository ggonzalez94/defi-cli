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
