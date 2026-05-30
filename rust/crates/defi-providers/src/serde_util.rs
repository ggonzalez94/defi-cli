//! Null-tolerant serde deserializers mirroring Go `encoding/json` value-type
//! semantics.
//!
//! Go's `encoding/json` unmarshals a JSON `null` into a **non-pointer** numeric
//! field by leaving it at its zero value (it does NOT error). The same holds for
//! `null` map values: `{"a":null}` into `map[string]float64` yields `{"a": 0}`.
//! Several provider wire DTOs (notably DefiLlama `/protocols`, where ~10% of
//! rows carry `"tvl": null`, and the Morpho GraphQL float fields) rely on this
//! leniency. serde's `#[serde(default)]` only covers a **missing** field — a
//! field present as `null` still errors with `invalid type: null, expected f64`.
//!
//! These helpers restore Go parity:
//! * [`de_f64_null_default`] — a scalar `f64`: `null` → `0.0` (also handles the
//!   missing case when paired with `#[serde(default)]`).
//! * [`de_f64_map_null_default`] — a `HashMap<String, f64>`: each `null` value →
//!   `0.0`, key retained (matching Go's map-value coercion).

use std::collections::HashMap;

use serde::{Deserialize, Deserializer};

/// Deserialize a scalar `f64`, mapping a JSON `null` to `0.0` (Go value-type
/// `float64` semantics). Pair with `#[serde(default)]` so a *missing* field also
/// yields `0.0`.
pub fn de_f64_null_default<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<f64>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// Deserialize a `HashMap<String, f64>`, mapping each `null` value to `0.0`
/// (Go map-value `float64` semantics). A missing/`null` map itself yields an
/// empty map. Pair with `#[serde(default)]` for the missing-field case.
pub fn de_f64_map_null_default<'de, D>(deserializer: D) -> Result<HashMap<String, f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<HashMap<String, Option<f64>>>::deserialize(deserializer)?;
    Ok(opt
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, v.unwrap_or_default()))
        .collect())
}

#[cfg(test)]
mod tests {
    //! # Success criteria — null-tolerant deserializers
    //!
    //! Go `encoding/json` coerces a JSON `null` into a non-pointer numeric field
    //! (scalar or map value) as the zero value, never an error. These helpers
    //! restore that leniency on top of serde, whose `#[serde(default)]` only
    //! covers a *missing* field. This is the fix for the DefiLlama
    //! `/protocols` decode failure (`invalid type: null, expected f64`), where
    //! ~10% of protocol rows carry `"tvl": null`.

    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Scalar {
        #[serde(default, deserialize_with = "de_f64_null_default")]
        tvl: f64,
    }

    #[derive(Debug, Deserialize)]
    struct Mapped {
        #[serde(default, deserialize_with = "de_f64_map_null_default")]
        m: std::collections::HashMap<String, f64>,
    }

    #[test]
    fn scalar_null_becomes_zero() {
        let s: Scalar = serde_json::from_str(r#"{"tvl":null}"#).expect("null tvl decodes");
        assert_eq!(s.tvl, 0.0);
    }

    #[test]
    fn scalar_missing_becomes_zero() {
        let s: Scalar = serde_json::from_str(r#"{}"#).expect("missing tvl decodes");
        assert_eq!(s.tvl, 0.0);
    }

    #[test]
    fn scalar_value_is_preserved() {
        let s: Scalar =
            serde_json::from_str(r#"{"tvl":150296157328.0473}"#).expect("value tvl decodes");
        assert_eq!(s.tvl, 150296157328.0473);
    }

    #[test]
    fn map_null_values_become_zero_keys_retained() {
        // Mirrors Go: `{"a":null,"b":1.5}` -> `{"a":0,"b":1.5}` (key retained).
        let m: Mapped =
            serde_json::from_str(r#"{"m":{"a":null,"b":1.5}}"#).expect("null map value decodes");
        assert_eq!(m.m.get("a"), Some(&0.0));
        assert_eq!(m.m.get("b"), Some(&1.5));
    }

    #[test]
    fn map_null_or_missing_is_empty() {
        let m: Mapped = serde_json::from_str(r#"{"m":null}"#).expect("null map decodes");
        assert!(m.m.is_empty());
        let m: Mapped = serde_json::from_str(r#"{}"#).expect("missing map decodes");
        assert!(m.m.is_empty());
    }
}
