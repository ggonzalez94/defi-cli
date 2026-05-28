//! Amount normalization: base units <-> decimal with `decimals`.
//!
//! Go source: `internal/id/amount.go` — `NormalizeAmount`, `formatDecimal`
//! (exported as `FormatDecimalCompat`), `decimalToBaseUnits`, `normalizeDecimal`,
//! and the `MaxUint256` constant.
//!
//! This module owns the *amount* leg of spec §2.4: amounts carry both a
//! base-unit integer string AND a normalized decimal string, kept consistent for
//! a given token `decimals`. It is pure string/bigint math; it depends on
//! `defi_errors` only for the stable usage-error code. It does NOT touch CAIP
//! parsing (`caip.rs`), chain resolution (`chain.rs`), or the token registry
//! (`tokens.rs`).

// =============================================================================
// SUCCESS CRITERIA (RED phase — tests written before implementation)
//
// This module (Go source: internal/id/amount.go) owns the amount-normalization
// contract (spec §2.4: "Amounts carry both `amount_base_units` and
// `amount_decimal` + `decimals`, kept consistent"). The Rust port is "correct"
// iff:
//
//   1. NORMALIZE_AMOUNT SIGNATURE + DUAL FORM (Go NormalizeAmount).
//      `normalize_amount(base_units, decimal, decimals) -> Result<(String,
//      String), Error>` returns `(base_units, decimal)` kept consistent for the
//      given `decimals`. Exactly ONE of `base_units` / `decimal` is provided:
//        - base-units input -> returns (base_units verbatim, formatted decimal).
//          NormalizeAmount("1000000","",6) == ("1000000","1").
//        - decimal input -> returns (computed base units, normalized decimal).
//          NormalizeAmount("","1.25",6) == ("1250000","1.25").
//      (Ported from Go TestNormalizeAmountBaseUnits / TestNormalizeAmountDecimal.)
//
//   2. MUTUAL EXCLUSIVITY + REQUIREDNESS (Go usage guards).
//      - Both provided (non-empty) -> Err(Usage "use either --amount or
//        --amount-decimal, not both"). (Go TestNormalizeAmountValidation.)
//      - Neither provided -> Err(Usage "amount is required").
//      - decimals < 0 -> Err(Usage "decimals must be >= 0"). NOTE: this guard
//        runs BEFORE the "max" shortcut and before any parsing, so it fires even
//        when an otherwise-valid amount is supplied.
//
//   3. "max" SHORTCUT (Go strings.EqualFold + TrimSpace on baseUnits).
//      When the BASE-UNITS argument equals "max" case-insensitively (after
//      trimming surrounding whitespace) -> Ok((MaxUint256, "max")). The decimal
//      string is literally "max" (NOT a formatted number). Case-insensitive:
//      "max","MAX","Max","  mAx  " all resolve. (Ported from
//      TestNormalizeAmountMax — both the lower-case and "MAX" cases.) The "max"
//      shortcut is checked ONLY on the base-units arg; "max" given as the decimal
//      arg is not special-cased (it fails the decimal pattern instead).
//
//   4. BASE-UNITS VALIDATION (Go big.Int parse + sign check).
//      A non-empty base-units string must be a non-negative integer:
//        - Not a valid base-10 integer -> Err(Usage "--amount must be a positive
//          integer string"). (e.g. "12.5", "abc", "0x10", "1_000".)
//        - A leading "-" (negative) -> Err(Usage "--amount must be
//          non-negative"). NOTE ordering: big.Int parses "-5" successfully, so the
//          sign check is a SEPARATE guard that fires after a successful parse;
//          "-abc" fails the integer parse first ("must be a positive integer
//          string"), while "-5" fails the sign guard ("must be non-negative").
//        - Valid -> returns the base-units string VERBATIM (no normalization of
//          leading zeros on the base-units side) plus its formatted decimal.
//          NormalizeAmount("007","",0) -> base "007" (verbatim), decimal "7".
//
//   5. DECIMAL VALIDATION + CONVERSION (Go decimalPattern + decimalToBaseUnits).
//      A non-empty decimal string:
//        - Must match ^[0-9]+(\.[0-9]+)?$ (digits, optional single fractional
//          part; no sign, no exponent, no bare "." or ".5" or "5.") else
//          Err(Usage "--amount-decimal must be in decimal form like 1.23").
//        - Fractional digit count must be <= decimals, else Err(Usage "decimal
//          precision exceeds token decimals (<decimals>)") with the actual
//          decimals interpolated. (Ported from TestNormalizeAmountValidation:
//          "1.1234567" with decimals=6 -> precision error.)
//        - Conversion: shift the decimal point right by `decimals`, drop the dot,
//          strip leading zeros; an all-zero result yields base units "0".
//          NormalizeAmount("","1.25",6) -> "1250000"; ("","0",6) -> "0";
//          ("","0.000001",6) -> "1"; ("","12",0) -> "12".
//
//   6. FORMAT_DECIMAL (Go formatDecimal / FormatDecimalCompat).
//      `format_decimal(base_units, decimals) -> String` renders a base-units
//      integer string as its decimal form:
//        - decimals == 0 -> the integer's canonical big.Int string (so "007" ->
//          "7"; "0" -> "0").
//        - Otherwise: left-pad so there are > decimals digits, split int/frac,
//          and RIGHT-TRIM trailing zeros from the fraction; if the fraction is all
//          zeros, return just the integer part (no trailing ".").
//          format_decimal("1000000",6) == "1";  format_decimal("1250000",6) ==
//          "1.25";  format_decimal("1",6) == "0.000001";
//          format_decimal("123456",6) == "0.123456";  format_decimal("0",6) ==
//          "0" (Go TestNormalizeAmountValidation asserts FormatDecimalCompat("0",
//          6) == "0").
//      format_decimal operates on the big.Int VALUE, so leading zeros in the
//      input are normalized away by the int round-trip.
//
//   7. NORMALIZE_DECIMAL (Go normalizeDecimal, the decimal-input echo).
//      The returned decimal for a decimal input is the input with leading zeros
//      on the integer part and trailing zeros on the fraction stripped:
//        - "1.250" -> "1.25";  "01.5" -> "1.5";  "1.000" -> "1";  "0.50" ->
//          "0.5";  "000" -> "0";  "00.00" -> "0"  (note: an all-zero integer with
//          no dot collapses to "0", and an all-zero fraction drops the dot).
//      This is the ECHO of the user's decimal (distinct from format_decimal,
//      which is computed from base units). The base-units field for a decimal
//      input is still computed via criterion 5.
//
//   8. MAX_UINT256 CONSTANT (Go MaxUint256).
//      The exported constant equals the decimal string of 2^256 - 1:
//      "115792089237316195423570985008687907853269984665640564039457584007913129639935".
//
//   9. ERROR CODES are the stable contract codes (spec §2.2): every validation
//      failure in this module is Code::Usage (2) — there are no other codes here.
//      (Ported assertions verify via defi_errors::Code, mirroring Go
//      clierr.New(clierr.CodeUsage, ...).)
//
//  10. BIG-INT RANGE (no silent overflow). Base-unit and converted values may be
//      arbitrarily large (up to / beyond uint256); normalization must not
//      truncate or overflow. A 60+ digit base-units string round-trips verbatim
//      through normalize_amount, and a high-precision decimal converts exactly.
//
// Ported Go tests (meaningful, contract-relevant) re-expressed below:
//   TestNormalizeAmountBaseUnits, TestNormalizeAmountDecimal,
//   TestNormalizeAmountMax (lower + MAX cases), TestNormalizeAmountValidation
//   (mutual exclusivity, precision overflow, FormatDecimalCompat("0",6)=="0").
// Added fresh spec-driven tests for the consistency invariant (base<->decimal
// round-trip), the full formatDecimal / normalizeDecimal trimming behavior, the
// requiredness/decimals<0/sign/integer-parse guards, the MaxUint256 value, and
// big-int range — all derived from the §2.4 contract, not Go internals.
// Skipped: nothing — amount.go has no internal-detail-only helper worth omitting;
//   formatDecimal is part of the public contract (exported as FormatDecimalCompat
//   and consumed across commands), so it is tested directly.
// =============================================================================

use defi_errors::{Code, Error};
use num_bigint::BigInt;

/// The decimal string representation of `2^256 - 1` (Go `MaxUint256`).
pub const MAX_UINT256: &str =
    "115792089237316195423570985008687907853269984665640564039457584007913129639935";

/// Parse a base-10 integer string the way Go's `big.Int.SetString(_, 10)` does:
/// an optional leading sign followed by ASCII digits, no whitespace, no
/// separators (Go only allows `_` separators when the base is 0), no radix
/// prefix.
///
/// `num_bigint::parse_bytes` is more permissive (it accepts `_` separators), so
/// we validate the strict Go shape first and only then delegate.
fn parse_big_int(s: &str) -> Option<BigInt> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    BigInt::parse_bytes(s.as_bytes(), 10)
}

/// Whether a string matches Go's `decimalPattern` (`^[0-9]+(\.[0-9]+)?$`):
/// one or more digits, optionally followed by a single `.`-delimited fractional
/// digit group. No sign, no exponent, no bare/trailing dot.
fn matches_decimal_pattern(s: &str) -> bool {
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let all_digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(int_part) {
        return false;
    }
    match frac_part {
        // A '.' with no fractional digits, or a second '.', is invalid.
        Some(f) => all_digits(f) && !f.contains('.'),
        None => true,
    }
}

/// Normalize an amount into consistent `(base_units, decimal)` strings for a
/// token's `decimals` (Go `NormalizeAmount`).
///
/// Exactly one of `base_units` / `decimal` must be provided. The special
/// base-units keyword `"max"` (case-insensitive, trimmed) resolves to
/// [`MAX_UINT256`] with the literal decimal `"max"`. All validation failures are
/// [`Code::Usage`] errors.
pub fn normalize_amount(
    base_units: &str,
    decimal: &str,
    decimals: i32,
) -> Result<(String, String), Error> {
    if !base_units.is_empty() && !decimal.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "use either --amount or --amount-decimal, not both",
        ));
    }
    if base_units.is_empty() && decimal.is_empty() {
        return Err(Error::new(Code::Usage, "amount is required"));
    }
    if decimals < 0 {
        return Err(Error::new(Code::Usage, "decimals must be >= 0"));
    }

    // "max" resolves to uint256.max (close-full-balance semantics).
    if base_units.trim().eq_ignore_ascii_case("max") {
        return Ok((MAX_UINT256.to_string(), "max".to_string()));
    }

    if !base_units.is_empty() {
        if parse_big_int(base_units).is_none() {
            return Err(Error::new(
                Code::Usage,
                "--amount must be a positive integer string",
            ));
        }
        if base_units.starts_with('-') {
            return Err(Error::new(Code::Usage, "--amount must be non-negative"));
        }
        return Ok((base_units.to_string(), format_decimal(base_units, decimals)));
    }

    if !matches_decimal_pattern(decimal) {
        return Err(Error::new(
            Code::Usage,
            "--amount-decimal must be in decimal form like 1.23",
        ));
    }
    let base = decimal_to_base_units(decimal, decimals)?;
    Ok((base, normalize_decimal(decimal)))
}

/// Render a base-units integer string as its decimal form (Go `formatDecimal`,
/// exported as `FormatDecimalCompat`).
///
/// `decimals == 0` returns the canonical big-int string (normalizing leading
/// zeros). Otherwise the value is split into integer/fraction parts and the
/// fraction is right-trimmed of trailing zeros (dropping a now-empty fraction
/// and its dot).
pub fn format_decimal(base_units: &str, decimals: i32) -> String {
    let n = parse_big_int(base_units).unwrap_or_default();
    if decimals == 0 {
        return n.to_string();
    }
    let decimals = decimals as usize;

    let mut s = n.to_string();
    if s.len() <= decimals {
        let pad = "0".repeat(decimals - s.len() + 1);
        s = format!("{pad}{s}");
    }
    let split_at = s.len() - decimals;
    let int_part = &s[..split_at];
    let frac_part = s[split_at..].trim_end_matches('0');
    if frac_part.is_empty() {
        int_part.to_string()
    } else {
        format!("{int_part}.{frac_part}")
    }
}

/// Convert a validated decimal string into a base-units integer string for the
/// given `decimals` (Go `decimalToBaseUnits`).
///
/// Errors when the fractional precision exceeds `decimals`.
fn decimal_to_base_units(decimal: &str, decimals: i32) -> Result<String, Error> {
    let decimals = decimals as usize;
    let (int_part, frac_part) = match decimal.split_once('.') {
        Some((i, f)) => (i, f),
        None => (decimal, ""),
    };
    if frac_part.len() > decimals {
        return Err(Error::new(
            Code::Usage,
            format!("decimal precision exceeds token decimals ({decimals})"),
        ));
    }

    let padded_frac = format!("{frac_part}{}", "0".repeat(decimals - frac_part.len()));
    let combined = format!("{int_part}{padded_frac}");
    let combined = combined.trim_start_matches('0');
    if combined.is_empty() {
        return Ok("0".to_string());
    }
    if parse_big_int(combined).is_none() {
        return Err(Error::new(Code::Usage, "invalid decimal amount"));
    }
    Ok(combined.to_string())
}

/// Normalize the echo of a decimal input (Go `normalizeDecimal`): strip leading
/// zeros from the integer part and trailing zeros from the fraction, collapsing
/// an all-zero value to `"0"` and dropping an empty fraction's dot.
fn normalize_decimal(v: &str) -> String {
    let Some((int_raw, frac_raw)) = v.split_once('.') else {
        let out = v.trim_start_matches('0');
        return if out.is_empty() {
            "0".to_string()
        } else {
            out.to_string()
        };
    };
    let mut int_part = int_raw.trim_start_matches('0');
    if int_part.is_empty() {
        int_part = "0";
    }
    let frac_part = frac_raw.trim_end_matches('0');
    if frac_part.is_empty() {
        int_part.to_string()
    } else {
        format!("{int_part}.{frac_part}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use defi_errors::Code;

    /// Convenience: assert a `normalize_amount` call is an Err with the given
    /// code + exact message (mirrors Go's clierr.As + Code/message checks).
    fn assert_usage_err(
        result: Result<(String, String), defi_errors::Error>,
        message: &str,
        context: &str,
    ) {
        let err = result.expect_err(&format!("{context}: expected an error, got Ok"));
        assert_eq!(err.code, Code::Usage, "{context}: wrong code");
        assert_eq!(err.message, message, "{context}: wrong message");
    }

    // ---- Criterion 1: dual form (base units / decimal) -------------------

    #[test]
    fn normalize_base_units_returns_verbatim_base_and_formatted_decimal() {
        // Ports Go TestNormalizeAmountBaseUnits.
        let (base, dec) = normalize_amount("1000000", "", 6).expect("valid base units must parse");
        assert_eq!(base, "1000000");
        assert_eq!(dec, "1");
    }

    #[test]
    fn normalize_decimal_returns_computed_base_and_normalized_decimal() {
        // Ports Go TestNormalizeAmountDecimal.
        let (base, dec) = normalize_amount("", "1.25", 6).expect("valid decimal must parse");
        assert_eq!(base, "1250000");
        assert_eq!(dec, "1.25");
    }

    #[test]
    fn base_and_decimal_forms_are_mutually_consistent() {
        // Consistency invariant (spec §2.4): for the same logical amount, the
        // base-units path and decimal path must agree on BOTH fields.
        let decimals = 6;
        let (base_from_units, dec_from_units) =
            normalize_amount("1250000", "", decimals).expect("base-units path");
        let (base_from_decimal, dec_from_decimal) =
            normalize_amount("", "1.25", decimals).expect("decimal path");
        assert_eq!(base_from_units, base_from_decimal, "base units must agree");
        assert_eq!(dec_from_units, dec_from_decimal, "decimal must agree");
        assert_eq!(base_from_units, "1250000");
        assert_eq!(dec_from_units, "1.25");
    }

    // ---- Criterion 2: mutual exclusivity / requiredness / decimals<0 -----

    #[test]
    fn both_amount_forms_provided_is_usage_error() {
        // Ports Go TestNormalizeAmountValidation (mutual exclusivity).
        assert_usage_err(
            normalize_amount("10", "1", 6),
            "use either --amount or --amount-decimal, not both",
            "both forms",
        );
    }

    #[test]
    fn neither_amount_form_provided_is_usage_error() {
        assert_usage_err(
            normalize_amount("", "", 6),
            "amount is required",
            "no amount",
        );
    }

    #[test]
    fn negative_decimals_is_usage_error_before_anything_else() {
        // decimals < 0 guard runs before the "max" shortcut and before parsing,
        // so even an otherwise-valid base-units value still errors.
        assert_usage_err(
            normalize_amount("1000000", "", -1),
            "decimals must be >= 0",
            "negative decimals with base units",
        );
        assert_usage_err(
            normalize_amount("max", "", -1),
            "decimals must be >= 0",
            "negative decimals with max",
        );
    }

    // ---- Criterion 3: "max" shortcut -------------------------------------

    #[test]
    fn max_shortcut_resolves_to_max_uint256_and_literal_max_decimal() {
        // Ports Go TestNormalizeAmountMax.
        let (base, dec) = normalize_amount("max", "", 18).expect("max must resolve");
        assert_eq!(base, MAX_UINT256);
        assert_eq!(dec, "max");
    }

    #[test]
    fn max_shortcut_is_case_insensitive_and_trims_whitespace() {
        // Ports the "MAX" case of Go TestNormalizeAmountMax; adds the trim case.
        for input in ["MAX", "Max", "mAx", "  max  "] {
            let (base, dec) = normalize_amount(input, "", 6)
                .unwrap_or_else(|_| panic!("{input:?} must resolve to max"));
            assert_eq!(base, MAX_UINT256, "{input:?}");
            assert_eq!(dec, "max", "{input:?}");
        }
    }

    #[test]
    fn max_is_only_special_as_base_units_not_as_decimal() {
        // "max" given as the DECIMAL arg is not special-cased; it fails the
        // decimal pattern instead.
        assert_usage_err(
            normalize_amount("", "max", 6),
            "--amount-decimal must be in decimal form like 1.23",
            "max as decimal",
        );
    }

    // ---- Criterion 4: base-units validation ------------------------------

    #[test]
    fn non_integer_base_units_is_usage_error() {
        for bad in ["12.5", "abc", "0x10", "1_000", "1e6", " 5 ", "1,000"] {
            assert_usage_err(
                normalize_amount(bad, "", 6),
                "--amount must be a positive integer string",
                bad,
            );
        }
    }

    #[test]
    fn negative_base_units_is_non_negative_usage_error() {
        // "-5" parses as a valid big.Int, so it trips the SEPARATE sign guard
        // ("must be non-negative"), not the integer-parse guard.
        assert_usage_err(
            normalize_amount("-5", "", 6),
            "--amount must be non-negative",
            "negative base units",
        );
    }

    #[test]
    fn negative_non_integer_base_units_fails_integer_parse_first() {
        // "-abc" is not a valid integer at all, so it fails the integer-parse
        // guard BEFORE the sign guard is reached.
        assert_usage_err(
            normalize_amount("-abc", "", 6),
            "--amount must be a positive integer string",
            "negative non-integer",
        );
    }

    #[test]
    fn base_units_are_returned_verbatim_including_leading_zeros() {
        // The base-units string is returned VERBATIM (no leading-zero stripping),
        // while the decimal is computed from the big.Int VALUE.
        let (base, dec) = normalize_amount("007", "", 0).expect("leading-zero int");
        assert_eq!(base, "007", "base units returned verbatim");
        assert_eq!(dec, "7", "decimal computed from value");
    }

    #[test]
    fn base_units_zero_with_decimals() {
        let (base, dec) = normalize_amount("0", "", 6).expect("zero base units");
        assert_eq!(base, "0");
        assert_eq!(dec, "0");
    }

    // ---- Criterion 5: decimal validation + conversion --------------------

    #[test]
    fn decimal_pattern_rejects_malformed_decimals() {
        for bad in [".5", "5.", "1.2.3", "-1.5", "+1", "1e3", "abc", "", " 1.5 "] {
            // empty string is "neither provided" -> different message; skip it
            // here and let the requiredness test cover it.
            if bad.is_empty() {
                continue;
            }
            assert_usage_err(
                normalize_amount("", bad, 6),
                "--amount-decimal must be in decimal form like 1.23",
                bad,
            );
        }
    }

    #[test]
    fn decimal_precision_exceeding_token_decimals_is_usage_error() {
        // Ports Go TestNormalizeAmountValidation ("1.1234567" with decimals=6).
        assert_usage_err(
            normalize_amount("", "1.1234567", 6),
            "decimal precision exceeds token decimals (6)",
            "precision overflow",
        );
        // Interpolation uses the actual decimals value.
        assert_usage_err(
            normalize_amount("", "1.123", 2),
            "decimal precision exceeds token decimals (2)",
            "precision overflow d=2",
        );
    }

    #[test]
    fn decimal_conversion_examples() {
        let cases: &[(&str, i32, &str, &str)] = &[
            // (decimal input, decimals, expected base, expected decimal echo)
            ("1.25", 6, "1250000", "1.25"),
            ("0", 6, "0", "0"),
            ("0.000001", 6, "1", "0.000001"),
            ("12", 0, "12", "12"),
            ("1", 18, "1000000000000000000", "1"),
            ("0.5", 1, "5", "0.5"),
            ("10.0", 6, "10000000", "10"),
        ];
        for (input, decimals, want_base, want_dec) in cases {
            let (base, dec) = normalize_amount("", input, *decimals)
                .unwrap_or_else(|_| panic!("{input} (d={decimals}) must convert"));
            assert_eq!(base, *want_base, "base for {input} d={decimals}");
            assert_eq!(dec, *want_dec, "decimal for {input} d={decimals}");
        }
    }

    // ---- Criterion 6: format_decimal (exported FormatDecimalCompat) ------

    #[test]
    fn format_decimal_zero_is_zero() {
        // Ports Go TestNormalizeAmountValidation: FormatDecimalCompat("0",6)=="0".
        assert_eq!(format_decimal("0", 6), "0");
    }

    #[test]
    fn format_decimal_examples() {
        let cases: &[(&str, i32, &str)] = &[
            ("1000000", 6, "1"),
            ("1250000", 6, "1.25"),
            ("1", 6, "0.000001"),
            ("123456", 6, "0.123456"),
            ("0", 6, "0"),
            ("100", 2, "1"),
            ("150", 2, "1.5"),
            ("12", 0, "12"),
            ("007", 0, "7"), // decimals==0 path normalizes via big.Int value
            ("1000000000000000000", 18, "1"),
        ];
        for (base, decimals, want) in cases {
            assert_eq!(
                format_decimal(base, *decimals),
                *want,
                "format_decimal({base}, {decimals})"
            );
        }
    }

    // ---- Criterion 7: normalize_decimal echo (via decimal input) ---------

    #[test]
    fn decimal_echo_strips_leading_int_and_trailing_frac_zeros() {
        // The decimal field returned for a DECIMAL input is the normalized echo
        // (distinct from format_decimal). Exercise it through normalize_amount
        // with decimals large enough to avoid the precision guard.
        let cases: &[(&str, i32, &str)] = &[
            ("1.250", 6, "1.25"),
            ("01.5", 6, "1.5"),
            ("1.000", 6, "1"),
            ("0.50", 6, "0.5"),
            ("000", 6, "0"),
            ("00.00", 6, "0"),
            ("007.00", 6, "7"),
        ];
        for (input, decimals, want_dec) in cases {
            let (_base, dec) = normalize_amount("", input, *decimals)
                .unwrap_or_else(|_| panic!("{input} must normalize"));
            assert_eq!(dec, *want_dec, "decimal echo for {input}");
        }
    }

    // ---- Criterion 8: MaxUint256 constant --------------------------------

    #[test]
    fn max_uint256_constant_value() {
        assert_eq!(
            MAX_UINT256,
            "115792089237316195423570985008687907853269984665640564039457584007913129639935"
        );
        // Sanity: 78 decimal digits, no sign, all numeric.
        assert_eq!(MAX_UINT256.len(), 78);
        assert!(MAX_UINT256.bytes().all(|b| b.is_ascii_digit()));
    }

    // ---- Criterion 9: every failure here is Code::Usage ------------------
    // (covered by assert_usage_err across the validation tests above)

    // ---- Criterion 10: big-int range (no overflow / truncation) ----------

    #[test]
    fn large_base_units_round_trip_without_overflow() {
        // A 60-digit base-units value (well beyond u128) must pass through
        // verbatim and format correctly with decimals.
        let big = "123456789012345678901234567890123456789012345678901234567890";
        let (base, dec) = normalize_amount(big, "", 0).expect("huge int must normalize");
        assert_eq!(base, big, "base units returned verbatim");
        assert_eq!(dec, big, "decimal at decimals=0 equals the value");
    }

    #[test]
    fn max_uint256_passed_as_explicit_base_units_round_trips() {
        // Passing the literal MaxUint256 as base units (not the "max" keyword)
        // must parse as a normal big integer and survive without truncation.
        let (base, _dec) = normalize_amount(MAX_UINT256, "", 0).expect("max uint256 as int");
        assert_eq!(base, MAX_UINT256);
    }

    #[test]
    fn high_precision_decimal_converts_exactly() {
        // 18-decimal token, full precision -> exact base units, no rounding.
        let (base, dec) =
            normalize_amount("", "1.234567890123456789", 18).expect("18-decimal must convert");
        assert_eq!(base, "1234567890123456789");
        assert_eq!(dec, "1.234567890123456789");
    }
}
