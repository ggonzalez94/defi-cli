//! Machine-readable command schema.
//!
//! Mirrors the data model and clap-independent helpers of Go's `internal/schema`.
//!
//! Scope note (idiomatic split): the cobra-coupled `Build`/`serialize`/`collectFlags`
//! tree walk in the Go `schema.go` is reproduced in `defi-app` (where the clap command
//! tree lives and the `schema` command is rendered — covered by the `schema.json` golden
//! fixture). This L0 crate owns the **serde schema data model** (exact JSON field names,
//! declaration order, and `omitempty` semantics) plus the **clap-free string helpers**
//! that the schema builder depends on (enum inference, default parsing, enum splitting).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Full per-command schema node. Field declaration order is the JSON output order.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CommandSchema {
    pub path: String,
    #[serde(rename = "use")]
    pub r#use: String,
    pub short: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mutation: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_modes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_constraints: Vec<InputConstraint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth: Vec<AuthRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<TypeSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<TypeSchema>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<FlagSchema>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<CommandSchema>,
}

/// Per-flag schema node.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FlagSchema {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shorthand: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub usage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(rename = "enum", default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub format: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
}

/// Command-level metadata attached out-of-band (mutation, auth, request/response, etc.).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CommandMetadata {
    #[serde(default, skip_serializing_if = "is_false")]
    pub mutation: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_modes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_constraints: Vec<InputConstraint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth: Vec<AuthRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<TypeSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<TypeSchema>,
}

/// Auth requirement descriptor.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AuthRequirement {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_vars: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub when: IndexMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// Input constraint descriptor (exactly_one_of / required / forbidden ...).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InputConstraint {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub when: IndexMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// Flag metadata (required / enum / format) carried alongside a flag.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FlagMetadata {
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(rename = "enum", default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub format: String,
}

/// Structural type schema for request/response shapes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TypeSchema {
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub format: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(rename = "enum", default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<SchemaField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<TypeSchema>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_properties: Option<Box<TypeSchema>>,
}

/// One field within a `TypeSchema` object.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SchemaField {
    pub name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub schema: TypeSchema,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[cfg(test)]
mod tests;

/// Infer an enum value list from a flag usage string's parenthetical, e.g.
/// `"Provider (aave|morpho)"` -> `["aave","morpho"]`, or a `k=v,k2=v2` body's keys.
/// Returns `None` when no enum can be inferred.
///
/// Port of Go `inferEnumValues`.
pub fn infer_enum_values(usage: &str) -> Option<Vec<String>> {
    let start = usage.find('(')?;
    let end = usage.rfind(')')?;
    // Go: start < 0 || end <= start -> nil. (`find`/`rfind` already handle absence.)
    if end <= start {
        return None;
    }
    let body = usage[start + 1..end].trim();
    if body.is_empty() {
        return None;
    }

    // Pipe form: `a|b|c`.
    if body.contains('|') {
        let out: Vec<String> = body
            .split('|')
            .map(sanitize_enum_value)
            .filter(|p| !p.is_empty())
            .collect();
        if !out.is_empty() {
            return Some(out);
        }
    }

    // Key=value comma form: `k1=v1, k2=v2` -> keys.
    if body.contains('=') && body.contains(',') {
        let mut out: Vec<String> = Vec::new();
        for part in body.split(',') {
            let trimmed = part.trim();
            // Go uses strings.Cut: split on the first '='; `ok` is true only when
            // a '=' is present.
            if let Some((left, _)) = trimmed.split_once('=') {
                let left = sanitize_enum_value(left);
                if !left.is_empty() {
                    out.push(left);
                }
            }
        }
        if !out.is_empty() {
            return Some(out);
        }
    }

    None
}

/// Trim a raw enum token down to its first whitespace-delimited word, stripping
/// trailing punctuation (`,;.)]`). Returns empty string when nothing usable.
///
/// Port of Go `sanitizeEnumValue`.
pub fn sanitize_enum_value(raw: &str) -> String {
    let value = raw.trim();
    if value.is_empty() {
        return String::new();
    }
    // Go: strings.Fields splits on runs of whitespace; take the first field.
    match value.split_whitespace().next() {
        Some(first) => first
            .trim_end_matches([',', ';', '.', ')', ']'])
            .to_string(),
        None => String::new(),
    }
}

/// Split a comma-separated enum tag into trimmed, non-empty values.
///
/// Port of Go `splitSchemaEnum`.
pub fn split_schema_enum(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse a cobra `stringSlice` default value (`"[a,b]"` / `"a,b"`) into a vector,
/// dropping empties; `""`/`"[]"` yield an empty vector.
///
/// Port of Go `parseStringSliceDefault`.
pub fn parse_string_slice_default(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "[]" {
        return Vec::new();
    }
    // Strip a single leading `[` and trailing `]` (matches Go TrimPrefix/TrimSuffix).
    let raw = raw.strip_prefix('[').unwrap_or(raw);
    let raw = raw.strip_suffix(']').unwrap_or(raw);
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}
