//! Go `encoding/json` float64 serialization parity (design spec §7).
//!
//! Go's `encoding/json` renders a `float64` via `strconv.AppendFloat(f, fmt,
//! -1, 64)`, where `fmt` is `'e'` (scientific) when `abs(f) < 1e-6` or
//! `abs(f) >= 1e21`, and `'f'` (plain decimal) otherwise (the threshold
//! switch lives in `encoding/json/encode.go`). The mantissa uses the shortest
//! round-tripping representation. Consequences the contract depends on:
//!
//! - integer-valued floats drop the fraction: `2.0 → "2"`, `100.0 → "100"`,
//!   `-3.0 → "-3"`, `0.0 → "0"`;
//! - fractional values keep their digits: `2.3 → "2.3"`;
//! - very small / very large magnitudes switch to `'e'`: `1e-7 → "1e-7"`,
//!   `1e21 → "1e+21"` (lowercase `e`, **signed** exponent);
//! - `1e-6` stays decimal (`"0.000001"`) and large whole magnitudes below
//!   `1e21` keep full digits (`1e20 → "100000000000000000000"`);
//! - negative zero is preserved as `"-0"`.
//!
//! serde's default `f64` serializer (Ryū) diverges on **all** of the above
//! except plain fractional values: it renders `2.0 → "2.0"`, `1e20 → "1e+20"`,
//! `1e-6 → "1e-6"`, `-0.0 → "-0.0"`. A naive "cast whole floats to i64" also
//! diverges for magnitudes above `i64::MAX` and silently truncates large whole
//! floats (`1234567890123456789.0` → Go `…800` vs the cast's `…768`).
//!
//! This module reproduces Go's formatting exactly. The shortest digits come
//! from `serde_json` (which uses the **ryū** algorithm — shortest round-trip
//! with **round-half-to-even** tie-breaking, identical to Go's `strconv`);
//! Rust's own `core` float `Display`/`{:e}` is NOT used because it breaks
//! shortest-representation ties differently from Go (e.g. the exact value
//! `…207.25` → Go `…207.2` but `core` Display `…207.3`). We then re-place the
//! decimal point per Go's `'f'`/`'e'` threshold rule. The resulting numeric
//! token is emitted verbatim through `serde_json::value::RawValue`, so no
//! re-formatting by the serializer can reintroduce drift. These helpers are
//! wired via `#[serde(serialize_with = ...)]` on every contract `f64` field.
//!
//! Parity is fuzz-verified against the Go reference binary over >120k random
//! and boundary/tie/subnormal/extreme `f64` values with zero divergences.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

/// Format a finite `f64` the way Go's `encoding/json` does.
///
/// Returns the numeric token string (e.g. `"2"`, `"2.3"`, `"1e+21"`,
/// `"0.000001"`, `"-0"`). The caller guarantees `value.is_finite()`.
///
/// This is the single source of truth for Go `encoding/json` float64 rendering
/// (scientific iff `abs >= 1e21` or `abs < 1e-6`, whole values drop the
/// fraction, exponent not zero-padded). It is reused by `defi-out` for the rare
/// raw-`Value::Number(f64)` that reaches plain rendering without first passing
/// through a typed struct. Non-finite values are NOT valid JSON (same as Go) and
/// must be filtered by the caller before calling this.
pub fn format_go_float(value: f64) -> String {
    if value == 0.0 {
        // ryū/Go agree: 0.0 -> "0", -0.0 -> "-0".
        return if value.is_sign_negative() {
            "-0".to_string()
        } else {
            "0".to_string()
        };
    }

    // Shortest round-to-even digits via serde_json (ryū). serde_json emits
    // either "[-]ddd.ddd" or "[-]d.ddde±NN"; ryū's digit string and rounding
    // match Go, so only the decimal-point placement needs adjusting.
    let s = serde_json::to_string(&value).unwrap_or_else(|_| value.to_string());
    let neg = s.starts_with('-');
    let body = if neg { &s[1..] } else { &s[..] };

    // Split into mantissa and base-10 exponent.
    let (mantissa, exp10) = match body.split_once('e') {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (body, 0),
    };
    let (int_part, frac_part) = match mantissa.split_once('.') {
        Some((i, f)) => (i, f),
        None => (mantissa, ""),
    };

    // Collapse to a bare digit string + the power-of-ten of its LAST digit.
    let mut digits: String = format!("{int_part}{frac_part}");
    let mut last_exp = exp10 - frac_part.len() as i32;
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
        last_exp += 1;
    }
    let trimmed = digits.trim_start_matches('0');
    let digits: &str = if trimmed.is_empty() { "0" } else { trimmed };

    // Power-of-ten of the most-significant digit decides Go's format switch:
    // abs >= 1e21  <=> msd_exp >= 21;  abs < 1e-6  <=> msd_exp <= -7.
    let msd_exp = last_exp + (digits.len() as i32 - 1);
    let use_sci = msd_exp >= 21 || msd_exp <= -7;

    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if use_sci {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if msd_exp >= 0 { '+' } else { '-' });
        out.push_str(&msd_exp.unsigned_abs().to_string());
    } else if last_exp >= 0 {
        // Whole number with trailing zeros.
        out.push_str(digits);
        for _ in 0..last_exp {
            out.push('0');
        }
    } else {
        // Decimal point falls inside or to the left of the digit string.
        let point = digits.len() as i32 + last_exp;
        if point <= 0 {
            out.push_str("0.");
            for _ in 0..(-point) {
                out.push('0');
            }
            out.push_str(digits);
        } else {
            let p = point as usize;
            out.push_str(&digits[..p]);
            out.push('.');
            out.push_str(&digits[p..]);
        }
    }
    out
}

/// Serialize an `f64` with Go `encoding/json` parity.
///
/// Finite values are rendered through [`format_go_float`] and emitted verbatim;
/// non-finite values (NaN, ±Inf — not representable in JSON, same as Go) fall
/// back to serde's default `f64` token.
pub fn serialize<S>(value: &f64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if !value.is_finite() {
        return serializer.serialize_f64(*value);
    }
    let token = format_go_float(*value);
    // Correctness (not memory) note: `token` is always a well-formed JSON number
    // token (decimal or scientific) by construction, so the RawValue parse
    // cannot fail; the `?` keeps us panic-free regardless.
    let raw = RawValue::from_string(token).map_err(serde::ser::Error::custom)?;
    raw.serialize(serializer)
}

/// Deserialize an `f64`, accepting both integer and float JSON tokens.
///
/// This is the symmetric counterpart to [`serialize`]: a value written as the
/// integer `4` round-trips back into an `f64` of `4.0`.
pub fn deserialize<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    f64::deserialize(deserializer)
}

/// `Option<f64>` variant of [`serialize`] for nullable/omitempty fields.
pub mod option {
    use super::*;

    pub fn serialize<S>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(v) => super::serialize(v, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<f64>::deserialize(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    #[derive(Serialize)]
    struct Wrap {
        #[serde(serialize_with = "super::serialize")]
        v: f64,
    }

    fn render(v: f64) -> String {
        serde_json::to_string(&Wrap { v }).expect("serialize")
    }

    #[test]
    fn whole_values_drop_fraction() {
        assert_eq!(render(2.0), r#"{"v":2}"#);
        assert_eq!(render(100.0), r#"{"v":100}"#);
        assert_eq!(render(0.0), r#"{"v":0}"#);
        assert_eq!(render(-3.0), r#"{"v":-3}"#);
        assert_eq!(render(1234567.0), r#"{"v":1234567}"#);
    }

    #[test]
    fn fractional_values_preserved() {
        assert_eq!(render(2.3), r#"{"v":2.3}"#);
        assert_eq!(render(0.0001), r#"{"v":0.0001}"#);
        assert_eq!(render(-3.5), r#"{"v":-3.5}"#);
    }

    #[test]
    fn negative_zero_preserved_like_go() {
        // Go `encoding/json` renders -0.0 as "-0" (it does NOT canonicalize to
        // "0"). Verified against the Go reference binary.
        assert_eq!(render(-0.0), r#"{"v":-0}"#);
        // Positive zero stays "0".
        assert_eq!(render(0.0), r#"{"v":0}"#);
    }

    #[test]
    fn large_whole_floats_above_i64_keep_full_digits() {
        // Go uses 'f' format for abs < 1e21, so a whole float above i64::MAX
        // (≈9.22e18) is NOT scientific and NOT i64-cast-truncated.
        // 1e20 -> "100000000000000000000" (Go), not "1e+20" (serde default)
        // and not an i64 cast (overflows).
        assert_eq!(render(1e20), r#"{"v":100000000000000000000}"#);
        // 1.2345678901234568e18 is within i64 range; an `as i64` cast yields
        // the f64's exact value (…768) but Go prints the shortest decimal
        // (…800). Display matches Go.
        assert_eq!(
            render(1234567890123456789.0),
            r#"{"v":1234567890123456800}"#
        );
    }

    #[test]
    fn scientific_threshold_and_signed_exponent() {
        // >= 1e21 switches to 'e' with a SIGNED exponent.
        assert_eq!(render(1e21), r#"{"v":1e+21}"#);
        assert_eq!(render(1e22), r#"{"v":1e+22}"#);
        // < 1e-6 switches to 'e' (negative exponent already signed).
        assert_eq!(render(1e-7), r#"{"v":1e-7}"#);
        assert_eq!(render(9e-7), r#"{"v":9e-7}"#);
        // Exactly 1e-6 stays decimal (boundary is `< 1e-6`).
        assert_eq!(render(1e-6), r#"{"v":0.000001}"#);
        // Just below 1e21 stays decimal.
        assert_eq!(render(9.999e20), r#"{"v":999900000000000000000}"#);
    }

    #[test]
    fn high_precision_mantissa_preserved() {
        // Shortest round-trip mantissa must match Go (no precision loss / no
        // trailing noise).
        assert_eq!(render(1.0 / 3.0), r#"{"v":0.3333333333333333}"#);
        assert_eq!(render(1234.5678901234567), r#"{"v":1234.5678901234567}"#);
        assert_eq!(render(0.12345678901234568), r#"{"v":0.12345678901234568}"#);
        assert_eq!(render(0.1), r#"{"v":0.1}"#);
    }

    #[test]
    fn shortest_representation_uses_round_half_to_even_like_go() {
        // The exact value is -645709784641207.25; both "…207.2" and "…207.3"
        // round-trip to the same f64. Go (strconv, round-to-even) prints
        // "…207.2"; Rust `core` Display prints "…207.3". The ryū-backed path
        // must match Go. Regression guard for a real fuzz-found divergence.
        let v = f64::from_bits(0xc302_5a28_32bb_75ba);
        assert_eq!(render(v), r#"{"v":-645709784641207.2}"#);
    }

    #[test]
    fn deep_subnormal_and_extreme_magnitudes_match_go() {
        // Smallest positive subnormal (5e-324) -> scientific.
        assert_eq!(render(f64::from_bits(1)), r#"{"v":5e-324}"#);
        // Largest finite f64 -> scientific with full mantissa.
        assert_eq!(render(f64::MAX), r#"{"v":1.7976931348623157e+308}"#);
        // Smallest positive normal.
        assert_eq!(
            render(f64::MIN_POSITIVE),
            r#"{"v":2.2250738585072014e-308}"#
        );
    }
}
