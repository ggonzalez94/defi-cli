//! JSON/plain rendering and field selection.
//!
//! Mirrors `internal/out/render.go`. JSON uses 2-space indent with struct field
//! declaration order; plain output sorts map keys alphabetically; `--select`
//! projects named top-level fields (machine contract — spec §2.3).

use defi_config::Settings;
use defi_model::Envelope;
use serde_json::Value;

// =============================================================================
// LOCKED INTERFACE (signatures the tests lock in).
//
// Go's `out.Render(w io.Writer, env model.Envelope, settings config.Settings)
// error` writes to an `io.Writer`. The idiomatic Rust port returns the rendered
// bytes as a `String` (the caller — the runner — writes them to stdout/stderr),
// which keeps `defi-out` pure, easy to test, and free of borrowed-writer
// plumbing. Every record is `\n`-terminated exactly as Go's
// `json.Encoder.Encode` / `fmt.Fprintln` produce.
//
// Render is settings-driven and has NO error-awareness: the "errors always
// print the full envelope (even with --results-only/--select)" invariant is
// owned by the CALLER (the Go runner resets `ResultsOnly=false` /
// `SelectFields=nil` before calling Render). `defi-out` faithfully renders
// whatever (envelope, settings) pair it is handed.
// =============================================================================

/// Errors produced while rendering an envelope.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// JSON serialization of the envelope or its `data` failed.
    #[error("render json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Render an envelope to a string per `settings` (mirrors `out.Render`).
///
/// - `settings.output_mode == "json"` → 2-space-indent JSON, struct field
///   declaration order preserved (map keys keep declaration order via
///   `serde_json` `preserve_order`).
/// - `settings.output_mode == "plain"` → for an array, one line per element; for
///   a map, keys sorted ALPHABETICALLY and joined as `k=v` space-separated;
///   scalars print their JSON form. An empty array prints `[]`.
/// - `settings.results_only` → render `data` only (after projection); otherwise
///   render the whole envelope (json) or a `{success,data,warnings,meta,error?}`
///   plain map.
/// - `settings.select_fields` (non-empty) → project the named top-level fields
///   over an object or array-of-objects (kept keys sorted alphabetically, like
///   Go's `map[string]any` JSON serialization — see [`project`]).
///
/// Every record ends with a trailing `\n` (matching Go).
pub fn render(env: &Envelope, settings: &Settings) -> Result<String, RenderError> {
    // Go reads `env.Data` (the raw payload). In the Rust model `data` is
    // `Option<Value>`; the Go nil/absent payload renders as `null`.
    let mut data = env.data.clone().unwrap_or(Value::Null);
    if !settings.select_fields.is_empty() {
        data = project(&data, &settings.select_fields);
    }

    if settings.results_only {
        if settings.output_mode == "json" {
            return Ok(encode_json(&data)?);
        }
        return Ok(render_plain(&data));
    }

    if settings.output_mode == "json" {
        // Re-attach the (possibly projected) data and render the full envelope.
        let mut env = env.clone();
        env.data = Some(data);
        return Ok(encode_json(&env)?);
    }

    // Full-envelope plain rendering. Go builds a `map[string]any` of
    // {success, data, warnings, meta} (+ error when non-nil) and renders it as a
    // single sorted `k=v` line. This path is not part of the stable machine
    // contract (the contract plain path is `--results-only`); we faithfully
    // reproduce the Go map-construction shape via serde.
    let mut plain = serde_json::Map::new();
    plain.insert("success".to_string(), Value::Bool(env.success));
    plain.insert("data".to_string(), data);
    plain.insert("warnings".to_string(), serde_json::to_value(&env.warnings)?);
    plain.insert("meta".to_string(), serde_json::to_value(&env.meta)?);
    if let Some(err) = &env.error {
        plain.insert("error".to_string(), serde_json::to_value(err)?);
    }
    Ok(render_plain(&Value::Object(plain)))
}

/// JSON-encode a value the way Go's `json.Encoder` with `SetIndent("", "  ")`
/// does: 2-space pretty indent plus a single trailing newline.
fn encode_json<T: serde::Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut s = serde_json::to_string_pretty(value)?;
    s.push('\n');
    Ok(s)
}

/// Render a value as plain text (mirrors `renderPlain`).
///
/// - Array: one `\n`-terminated line per element; an EMPTY array prints `"[]\n"`.
/// - Anything else (object/scalar/null): one `\n`-terminated `to_line` line.
fn render_plain(data: &Value) -> String {
    match data {
        Value::Array(items) => {
            if items.is_empty() {
                return "[]\n".to_string();
            }
            let mut out = String::new();
            for item in items {
                out.push_str(&to_line(item));
                out.push('\n');
            }
            out
        }
        other => {
            let mut out = to_line(other);
            out.push('\n');
            out
        }
    }
}

/// Project the named top-level `fields` over `data` (object or array-of-objects),
/// mirroring `project`/`projectMap`.
///
/// CONTRACT NOTE — key ordering: Go's `projectMap` builds a plain
/// `map[string]any`; `encoding/json` then serializes that map with its keys
/// **sorted ALPHABETICALLY**. So the projected JSON's key order is alphabetical,
/// NOT the requested `--select` order. (`--select symbol,asset_id` and
/// `--select asset_id,symbol` both emit `asset_id` before `symbol`.) The set of
/// kept fields is the requested set; only their *order* is alphabetical. A
/// scalar or any non-object/non-array value passes through unchanged.
pub fn project(data: &Value, fields: &[String]) -> Value {
    match data {
        Value::Array(items) => {
            // Go drops non-object elements (only objects are projected).
            let projected: Vec<Value> = items
                .iter()
                .filter_map(|item| {
                    item.as_object()
                        .map(|m| Value::Object(project_map(m, fields)))
                })
                .collect();
            Value::Array(projected)
        }
        Value::Object(map) => Value::Object(project_map(map, fields)),
        other => other.clone(),
    }
}

/// Project the requested `fields` out of `map`, silently skipping any field that
/// is absent (mirrors `projectMap`).
///
/// The kept fields are emitted with their keys **sorted alphabetically** to
/// match Go: `projectMap` returns a `map[string]any`, and `encoding/json` sorts
/// map keys on output. A duplicate field in `fields` is kept once.
fn project_map(
    map: &serde_json::Map<String, Value>,
    fields: &[String],
) -> serde_json::Map<String, Value> {
    // Collect the present requested keys, then sort alphabetically so the
    // rendered object's key order matches Go's `encoding/json` map serialization
    // (independent of the requested `--select` order).
    let mut keys: Vec<&String> = fields.iter().filter(|f| map.contains_key(*f)).collect();
    keys.sort();
    keys.dedup();

    let mut out = serde_json::Map::new();
    for f in keys {
        if let Some(v) = map.get(f) {
            out.insert(f.clone(), v.clone());
        }
    }
    out
}

/// Render a JSON value as a single plain line (mirrors `toLine`).
///
/// For an object: keys sorted alphabetically, joined as `k=v` (space-separated),
/// with each value rendered Go-`%v`-style (strings unquoted, numbers via Go
/// float formatting, arrays as `[a b c]`). For any other value: its JSON form.
pub fn to_line(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            keys.iter()
                .map(|k| format!("{}={}", k, go_v(&map[*k])))
                .collect::<Vec<_>>()
                .join(" ")
        }
        // Go's `toLine` default branch json.Marshals the value. For a float64
        // that means `encoding/json` formatting, which drops the fraction on
        // whole values (`2.0 → "2"`) and only goes scientific at |exp| >= 21 —
        // DIFFERENT from the `%v` map-value path (`format_go_g`, scientific at
        // |exp| >= 6). serde_json's default f64 emits a trailing `.0`
        // (`2.0 → "2.0"`), so route non-integer numbers through the Go-json
        // float formatter; everything else (strings quoted, ints, bools) matches
        // serde_json verbatim.
        Value::Number(n) if n.as_i64().is_none() && n.as_u64().is_none() => match n.as_f64() {
            Some(f) => format_go_json_float(f),
            None => serde_json::to_string(value).unwrap_or_default(),
        },
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Format an `f64` the way Go's `encoding/json.Marshal` does (the `toLine`
/// scalar branch), as opposed to the `%v` map-value path.
///
/// Go's `encoding/json` goes scientific only at `abs >= 1e21` or `abs < 1e-6`
/// (NOT the `fmt`/`%v` rule of `|exp| >= 6`): whole values drop the fraction
/// (`2.0 → "2"`, `1000000.0 → "1000000"`), `0.00001 → "0.00001"`, and the
/// exponent is not zero-padded (`1e-09 → 1e-09`? no: Go strips to `1e-09`'s
/// shortest form). This delegates to `defi-model::go_float::format_go_float`,
/// the single source of truth for that rule (also used for the typed-struct JSON
/// path), so the two paths can never drift. Non-finite values are not valid JSON
/// (same as Go), so they fall back to serde's token here.
fn format_go_json_float(value: f64) -> String {
    if value.is_finite() {
        defi_model::go_float::format_go_float(value)
    } else {
        serde_json::to_string(&value).unwrap_or_default()
    }
}

/// Render a JSON value the way Go's `fmt.Sprintf("%v")` renders the
/// `normalizeValue` (JSON-decoded) representation used in plain `k=v` pairs:
///
/// - string → unquoted text (`name=x`);
/// - number → Go float/`%g` formatting (`score=42`, `apy=2.3`);
/// - bool → `true`/`false`;
/// - null → `<nil>` (Go's `%v` of a nil interface);
/// - array → space-joined elements wrapped in brackets (`tags=[a b]`);
/// - object → Go map form (`map[k:v ...]`, keys sorted, Go-`%v` semantics).
fn go_v(value: &Value) -> String {
    match value {
        Value::Null => "<nil>".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::String(s) => s.clone(),
        Value::Number(n) => format_go_number(n),
        Value::Array(items) => {
            let inner = items.iter().map(go_v).collect::<Vec<_>>().join(" ");
            format!("[{inner}]")
        }
        Value::Object(map) => {
            // Go's `%v` of a `map[string]any` sorts keys and joins as
            // `map[k1:v1 k2:v2]`.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner = keys
                .iter()
                .map(|k| format!("{}:{}", k, go_v(&map[*k])))
                .collect::<Vec<_>>()
                .join(" ");
            format!("map[{inner}]")
        }
    }
}

/// Format a JSON number the way Go's `fmt` `%v` verb formats the `float64` /
/// integer value produced by `normalizeValue` (a JSON round-trip).
///
/// `normalizeValue` decodes every JSON number into a Go `float64`, so `%v` runs
/// through `strconv.FormatFloat(f, 'g', -1, 64)`:
/// - integer-valued floats drop the fraction (`42`, `2`, `-3`);
/// - fractional values keep their shortest digits (`2.3`);
/// - very small/large magnitudes switch to scientific (`'g'` threshold).
///
/// For the values the CLI emits this coincides with the JSON decimal form for
/// whole and modest fractional numbers, which is all the contract relies on.
fn format_go_number(n: &serde_json::Number) -> String {
    if let Some(f) = n.as_f64() {
        format_go_g(f)
    } else {
        // Integers outside f64 range: print the raw token (already integral).
        n.to_string()
    }
}

/// Reproduce Go's `strconv.FormatFloat(f, 'g', -1, 64)` (the `%v` verb for
/// `float64`) used in the plain `k=v` MAP-VALUE path.
///
/// Go's shortest-`'g'` uses scientific notation when the decimal exponent of the
/// most-significant digit is `< -4` OR `>= 6`. The upper bound is `6`, NOT 21:
/// Go's `internal/strconv/ftoa.go` `formatDigits` sets `eprec = 6` for shortest
/// precision (`if shortest { eprec = 6 }`), and the switch is
/// `exp < -4 || exp >= eprec`. So e.g. `1_000_000` (exp = 6) renders `1e+06`,
/// `999_999` (exp = 5) renders `999999`, and `2_500_000.55` renders
/// `2.50000055e+06` — exactly the USD/TVL-scale magnitudes the CLI emits for
/// `tvl_usd`, `circulating_usd`, `volume_24h_usd`, etc.
///
/// Scientific is used when the most-significant-digit exponent is `< -4` OR
/// `>= 6`; otherwise the shortest decimal is printed (whole values drop the
/// fraction). Shortest round-tripping digits come from `serde_json` (ryū,
/// round-half-to-even — identical tie-breaking to Go's `strconv`), so only the
/// decimal-point placement and scientific switch are reconstructed here.
///
/// NOTE: the JSON-form scalar path uses a DIFFERENT rule (Go `encoding/json`,
/// scientific at `abs >= 1e21` / `< 1e-6`); see [`format_go_json_float`].
fn format_go_g(value: f64) -> String {
    const SCI_THRESHOLD: i32 = 6;
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0".to_string()
        } else {
            "0".to_string()
        };
    }
    if !value.is_finite() {
        // Go `%v` of non-finite floats: +Inf / -Inf / NaN.
        if value.is_nan() {
            return "NaN".to_string();
        }
        return if value.is_sign_positive() {
            "+Inf".to_string()
        } else {
            "-Inf".to_string()
        };
    }

    // Shortest round-to-even digits via serde_json (ryū). serde_json emits
    // either "[-]ddd.ddd" or "[-]d.ddde±NN"; ryū's digit string and rounding
    // match Go's strconv, so only the decimal-point placement needs adjusting.
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

    // Power-of-ten of the most-significant digit decides Go's `'g'` switch:
    // scientific when msd_exp >= SCI_THRESHOLD or msd_exp < -4.
    let msd_exp = last_exp + (digits.len() as i32 - 1);
    let use_sci = !(-4..SCI_THRESHOLD).contains(&msd_exp);

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
        // Go's 'g' pads the exponent to at least two digits.
        let exp = msd_exp.unsigned_abs();
        if exp < 10 {
            out.push('0');
        }
        out.push_str(&exp.to_string());
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

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-out` (Go source: `internal/out/render.go`)
    //!
    //! This crate owns the **rendering** half of the machine contract (design
    //! spec §2.3). The Go `out.Render(w, env, settings)` is faithfully ported as
    //! a pure `render(&Envelope, &Settings) -> Result<String, _>` that returns
    //! the bytes the runner writes. The port is "correct" iff:
    //!
    //! 1. **JSON, full envelope.** `output_mode="json"`, not results-only →
    //!    canonical 2-space-indent JSON of the whole envelope, struct field
    //!    DECLARATION order preserved, one trailing `\n` (Go `json.Encoder`).
    //!
    //! 2. **JSON, results-only.** `results_only=true` → render `data` only (after
    //!    projection), 2-space-indent, trailing `\n`. Scalars render their JSON
    //!    form: a string `"0.5.0"` → `"\"0.5.0\"\n"`; a number `42` → `"42\n"`;
    //!    an empty array → `"[]\n"`; `null`/absent data → `"null\n"`.
    //!    (Probed against the Go binary + `json.Encoder` with `SetIndent("","  ")`.)
    //!
    //! 3. **`--select` projection (json & plain).** With non-empty
    //!    `select_fields`, project the named TOP-LEVEL fields over an object or an
    //!    array-of-objects; keep exactly the requested set; SORT the kept keys
    //!    ALPHABETICALLY (Go's `projectMap` returns a `map[string]any` and
    //!    `encoding/json` sorts map keys on output, so projected order is
    //!    alphabetical, NOT the requested order); drop the rest; silently skip a
    //!    requested field that is absent; pass a scalar through unchanged.
    //!    (Ports `TestRenderJSONSelectResultsOnly`.)
    //!
    //! 4. **Plain, results-only — the contract path.** For an ARRAY: one line per
    //!    element, each line being its object rendered as `k=v` pairs with keys
    //!    sorted ALPHABETICALLY and space-joined; an EMPTY array prints exactly
    //!    `"[]\n"`. For a single OBJECT: one `k=v` line. For a SCALAR: its JSON
    //!    form on one line (string → quoted `"0.5.0"`, number → `42`, bool →
    //!    `true`). Each record `\n`-terminated. (Ports `TestRenderPlain`; values
    //!    probed against Go `fmt.Sprintf("%s=%v")` on `normalizeValue` output.)
    //!
    //! 5. **Plain value formatting (Go `%v` parity for realistic data).** Inside
    //!    `k=v`: strings are UNQUOTED (`name=x`, not `name="x"`); whole-valued
    //!    numbers drop the fraction (`score=42`, `count=2`); fractional numbers
    //!    keep digits (`apy=2.3`); booleans are `true`/`false`; an array of
    //!    scalars renders `tags=[a b]` (space-joined, no quotes/commas).
    //!
    //! 6. **Alphabetical key sort is independent of input order.** Two objects
    //!    with the same keys in different insertion orders produce the SAME plain
    //!    line.
    //!
    //! 7. **Render is settings-driven, NOT error-aware.** `render` honors
    //!    `results_only`/`select_fields` regardless of `success`; the
    //!    "full-envelope-on-error" invariant is the runner's job (it resets those
    //!    settings before calling render). A success envelope rendered with
    //!    `results_only=true` yields data only; an error envelope rendered with
    //!    `results_only=false` (as the runner does) yields the full envelope.
    //!
    //! 8. **No panics.** `render`/`project`/`to_line` never panic on the value
    //!    shapes the CLI emits; serialization failure surfaces as
    //!    `RenderError::Json`.
    //!
    //! ## Ported Go tests
    //! - `TestRenderJSONSelectResultsOnly` → `json_select_results_only_projects_named_fields`
    //! - `TestRenderPlain` → `plain_results_only_renders_object_as_sorted_kv`
    //!
    //! ## Deliberately NOT ported (non-contract Go-`fmt` artifacts)
    //! The Go full-envelope PLAIN path renders `meta`/`error` as Go's
    //! `fmt.Sprintf("%v")` of nested maps (`meta=map[cache:map[...]]`) and nil
    //! warnings as `<nil>`. No machine consumes this; it is a Go implementation
    //! artifact, not part of the stable contract. The contract-stable plain path
    //! is `--results-only` (data only), which IS exhaustively tested here. The
    //! full-envelope-plain rendering of nested structs is intentionally left as a
    //! VERIFY/remainder concern rather than calcifying Go `fmt` output.

    use super::*;
    use defi_config::Settings;
    use defi_model::{CacheStatus, Envelope, ErrorBody};
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::time::Duration;

    // --- helpers ------------------------------------------------------------

    fn fixed_ts() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-05-28T18:48:18.949627Z")
            .expect("valid rfc3339")
            .with_timezone(&chrono::Utc)
    }

    /// A minimal resolved [`Settings`] for rendering tests. Only the rendering
    /// fields matter here; the rest take harmless placeholder values.
    fn settings(output_mode: &str, results_only: bool, select_fields: &[&str]) -> Settings {
        Settings {
            output_mode: output_mode.to_string(),
            select_fields: select_fields.iter().map(|s| s.to_string()).collect(),
            results_only,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_secs(10),
            retries: 2,
            max_stale: Duration::from_secs(300),
            no_stale: false,
            cache_enabled: true,
            cache_path: PathBuf::from("/tmp/cache.db"),
            cache_lock_path: PathBuf::from("/tmp/cache.lock"),
            action_store_path: PathBuf::from("/tmp/actions.db"),
            action_lock_path: PathBuf::from("/tmp/actions.lock"),
            defillama_api_key: String::new(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    /// A success envelope wrapping `data`, with a deterministic meta block
    /// (matches the Go runner's `emitSuccess` construction site).
    fn success_env(command: &str, data: Value) -> Envelope {
        let mut env = Envelope::success(
            command,
            data,
            Vec::new(),
            CacheStatus::bypass(),
            Vec::new(),
            false,
        );
        env.meta.request_id = "fixedreqid".into();
        env.meta.timestamp = fixed_ts();
        env
    }

    /// An error envelope (as the Go runner builds in `renderError`):
    /// `success=false`, `data=[]`, `error` set, `cache.status="bypass"`.
    fn error_env(command: &str, code: i64, typ: &str, message: &str) -> Envelope {
        let mut env = Envelope::error(
            command,
            ErrorBody {
                code,
                error_type: typ.into(),
                message: message.into(),
            },
            Vec::new(),
            Vec::new(),
            false,
        );
        env.meta.request_id = "fixedreqid".into();
        env.meta.timestamp = fixed_ts();
        env
    }

    // =========================================================================
    // Criterion 3 — `--select` projection (ports TestRenderJSONSelectResultsOnly)
    // =========================================================================

    #[test]
    fn json_select_results_only_projects_named_fields() {
        // Go TestRenderJSONSelectResultsOnly: data=[{a:1,b:2}], select=["a"],
        // results-only json → [{"a":1}], no "b".
        let env = success_env("x", json!([{"a": 1, "b": 2}]));
        let out = render(&env, &settings("json", true, &["a"])).expect("render");

        let parsed: Vec<serde_json::Map<String, Value>> =
            serde_json::from_str(&out).expect("results-only json is an array of objects");
        assert_eq!(parsed.len(), 1, "one element survives, got: {out}");
        assert_eq!(parsed[0].get("a"), Some(&json!(1)), "projected field kept");
        assert!(
            !parsed[0].contains_key("b"),
            "non-selected field dropped, got: {out}"
        );
        assert!(out.ends_with('\n'), "json record is newline-terminated");
    }

    #[test]
    fn project_sorts_kept_keys_alphabetically_not_requested_order() {
        // CONTRACT: Go's `projectMap` returns a `map[string]any`, which
        // `encoding/json` serializes with keys sorted ALPHABETICALLY. So a
        // requested order of [b, a] still emits keys as [a, b]; only the *set* of
        // kept fields follows the request, not their order. (Probed against the
        // Go binary: `--select b,a` and `--select a,b` produce identical output.)
        let data = json!({"a": 1, "b": 2, "c": 3});
        let out = project(&data, &["b".into(), "a".into()]);
        let keys: Vec<&String> = out.as_object().expect("object projection").keys().collect();
        assert_eq!(
            keys,
            vec!["a", "b"],
            "projection keys are sorted alphabetically"
        );
        assert!(
            out.as_object().unwrap().get("c").is_none(),
            "unrequested field dropped"
        );
        // Order-independence: reversing the request changes nothing.
        let out_rev = project(&data, &["a".into(), "b".into()]);
        assert_eq!(
            out, out_rev,
            "projected key order is independent of --select order"
        );
    }

    #[test]
    fn project_over_array_of_objects_projects_each_element() {
        let data = json!([{"a": 1, "b": 2}, {"a": 3, "b": 4}]);
        let out = project(&data, &["a".into()]);
        assert_eq!(out, json!([{"a": 1}, {"a": 3}]), "each element projected");
    }

    #[test]
    fn project_skips_absent_requested_field() {
        let data = json!({"a": 1});
        let out = project(&data, &["a".into(), "missing".into()]);
        assert_eq!(
            out,
            json!({"a": 1}),
            "absent requested field is silently skipped, not null"
        );
    }

    #[test]
    fn project_passes_scalar_through_unchanged() {
        let data = json!("0.5.0");
        let out = project(&data, &["a".into()]);
        assert_eq!(out, json!("0.5.0"), "scalar passes through projection");
    }

    // =========================================================================
    // Criterion 2 — JSON results-only scalar/array/null parity
    // (probed against Go json.Encoder with SetIndent("","  "))
    // =========================================================================

    #[test]
    fn json_results_only_string_scalar_is_quoted_with_newline() {
        // Go `version` data is the scalar string "0.5.0"; json.Encoder →
        // "\"0.5.0\"\n".
        let env = success_env("version", json!("0.5.0"));
        let out = render(&env, &settings("json", true, &[])).expect("render");
        assert_eq!(out, "\"0.5.0\"\n");
    }

    #[test]
    fn json_results_only_number_scalar() {
        let env = success_env("x", json!(42));
        let out = render(&env, &settings("json", true, &[])).expect("render");
        assert_eq!(out, "42\n");
    }

    #[test]
    fn json_results_only_empty_array() {
        let env = success_env("x", json!([]));
        let out = render(&env, &settings("json", true, &[])).expect("render");
        assert_eq!(out, "[]\n");
    }

    #[test]
    fn json_results_only_array_of_objects_is_two_space_pretty() {
        // Go json.Encoder SetIndent("","  ") of [{a:1,b:2}].
        let env = success_env("x", json!([{"a": 1, "b": 2}]));
        let out = render(&env, &settings("json", true, &[])).expect("render");
        assert_eq!(out, "[\n  {\n    \"a\": 1,\n    \"b\": 2\n  }\n]\n");
    }

    #[test]
    fn json_results_only_null_data() {
        // Go: omitempty drops `data`; results-only renders the (absent) data as
        // `null\n` via json.Encoder.Encode(nil).
        let env = success_env("x", Value::Null);
        let out = render(&env, &settings("json", true, &[])).expect("render");
        assert_eq!(out, "null\n");
    }

    // =========================================================================
    // Criterion 1 — JSON full-envelope rendering (2-space, declaration order)
    // =========================================================================

    #[test]
    fn json_full_envelope_is_two_space_declaration_order() {
        let env = error_env(
            "assets resolve",
            2,
            "usage_error",
            "unsupported chain input: notarealchain",
        );
        let out = render(&env, &settings("json", false, &[])).expect("render");
        let expected = "{\n  \"version\": \"v1\",\n  \"success\": false,\n  \"data\": [],\n  \"error\": {\n    \"code\": 2,\n    \"type\": \"usage_error\",\n    \"message\": \"unsupported chain input: notarealchain\"\n  },\n  \"meta\": {\n    \"request_id\": \"fixedreqid\",\n    \"timestamp\": \"2026-05-28T18:48:18.949627Z\",\n    \"command\": \"assets resolve\",\n    \"cache\": {\n      \"status\": \"bypass\",\n      \"age_ms\": 0,\n      \"stale\": false\n    },\n    \"partial\": false\n  }\n}\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn json_full_envelope_select_projects_data_but_keeps_envelope() {
        // --select with full envelope (not results-only) projects `data` in place
        // while keeping the envelope wrapper. Go applies `project` to env.Data
        // before re-attaching it.
        let env = success_env("x", json!([{"a": 1, "b": 2}]));
        let out = render(&env, &settings("json", false, &["a"])).expect("render");
        let v: Value = serde_json::from_str(&out).expect("envelope json");
        assert_eq!(v["data"], json!([{"a": 1}]), "data projected in envelope");
        assert_eq!(v["version"], json!("v1"), "envelope wrapper preserved");
        assert!(out.starts_with("{\n  \"version\""), "2-space envelope");
    }

    // =========================================================================
    // Criterion 4 — Plain results-only (the contract path).
    // Ports TestRenderPlain.
    // =========================================================================

    #[test]
    fn plain_results_only_renders_object_as_sorted_kv() {
        // Go TestRenderPlain: data=[{name:"x",score:42}], plain results-only →
        // a line containing "name=x". Keys sorted alphabetically.
        let env = success_env("x", json!([{"name": "x", "score": 42}]));
        let out = render(&env, &settings("plain", true, &[])).expect("render");
        assert_eq!(out, "name=x score=42\n");
    }

    #[test]
    fn plain_results_only_one_line_per_array_element() {
        let env = success_env("x", json!([{"name": "a", "v": 1}, {"name": "b", "v": 2}]));
        let out = render(&env, &settings("plain", true, &[])).expect("render");
        assert_eq!(out, "name=a v=1\nname=b v=2\n");
    }

    #[test]
    fn plain_results_only_empty_array_prints_brackets() {
        let env = success_env("x", json!([]));
        let out = render(&env, &settings("plain", true, &[])).expect("render");
        assert_eq!(out, "[]\n", "empty slice prints [] (Go renderPlain)");
    }

    #[test]
    fn plain_results_only_single_object_one_line() {
        let env = success_env("x", json!({"b": 2, "a": 1}));
        let out = render(&env, &settings("plain", true, &[])).expect("render");
        assert_eq!(out, "a=1 b=2\n", "single map → one sorted kv line");
    }

    #[test]
    fn plain_results_only_scalar_string_is_json_quoted() {
        // Go toLine default branch json.Marshals a scalar → "0.5.0" (quoted).
        let env = success_env("version", json!("0.5.0"));
        let out = render(&env, &settings("plain", true, &[])).expect("render");
        assert_eq!(out, "\"0.5.0\"\n");
    }

    #[test]
    fn plain_results_only_scalar_number_and_bool() {
        let n = success_env("x", json!(42));
        assert_eq!(
            render(&n, &settings("plain", true, &[])).expect("render"),
            "42\n"
        );
        let b = success_env("x", json!(true));
        assert_eq!(
            render(&b, &settings("plain", true, &[])).expect("render"),
            "true\n"
        );
    }

    // =========================================================================
    // Criterion 5 — Plain value formatting (Go `%v` parity)
    // =========================================================================

    #[test]
    fn to_line_strings_unquoted_numbers_bools_arrays() {
        // Go: fmt.Sprintf("%s=%v") over normalizeValue ⇒
        //   apy=2.3 name=x score=42 tags=[a b]
        let line = to_line(&json!({
            "name": "x",
            "score": 42,
            "apy": 2.3,
            "tags": ["a", "b"],
        }));
        assert_eq!(line, "apy=2.3 name=x score=42 tags=[a b]");
    }

    #[test]
    fn to_line_whole_floats_drop_fraction_like_go() {
        // Go %v of float64 2.0 → "2"; 2.3 → "2.3".
        assert_eq!(to_line(&json!({"x": 2.0})), "x=2");
        assert_eq!(to_line(&json!({"x": 2.3})), "x=2.3");
    }

    #[test]
    fn go_g_float_formatting_matches_strconv_reference_table() {
        // Reference table captured from the Go binary's exact rendering path:
        //   normalizeValue(f) -> float64; fmt.Sprintf("%v", f)
        //     == strconv.FormatFloat(f, 'g', -1, 64)
        // The Go shortest-'g' switches to scientific when the decimal exponent
        // of the most-significant digit is < -4 OR >= 6 (Go sets eprec=6 for
        // shortest precision). These USD/TVL-scale magnitudes (>= 1e6) are
        // EXACTLY what the CLI emits for `tvl_usd`, `circulating_usd`,
        // `volume_24h_usd`, etc., so getting this boundary right is contract.
        let cases: &[(f64, &str)] = &[
            (0_f64, "0"),
            (1_f64, "1"),
            (2_f64, "2"),
            (2.3_f64, "2.3"),
            (42_f64, "42"),
            (-3_f64, "-3"),
            (0.5_f64, "0.5"),
            (0.1_f64, "0.1"),
            (100000_f64, "100000"),
            (999999_f64, "999999"),
            (1_000_000_f64, "1e+06"),
            (1_234_567_f64, "1.234567e+06"),
            (2_500_000.55_f64, "2.50000055e+06"),
            (12_300_000_f64, "1.23e+07"),
            (1e15_f64, "1e+15"),
            (1e20_f64, "1e+20"),
            (1e21_f64, "1e+21"),
            (1e22_f64, "1e+22"),
            (0.0001_f64, "0.0001"),
            (0.00001_f64, "1e-05"),
            (0.000012345_f64, "1.2345e-05"),
            (12345.6789_f64, "12345.6789"),
            (99.99_f64, "99.99"),
            (0.3333333333333333_f64, "0.3333333333333333"),
            (9.87654321_f64, "9.87654321"),
            (123450_f64, "123450"),
            (100001_f64, "100001"),
            (-1_000_000_f64, "-1e+06"),
            (-0.00001_f64, "-1e-05"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                format_go_g(*input),
                *expected,
                "format_go_g({input}) should match Go strconv.FormatFloat('g',-1,64)"
            );
            // And via the public k=v path used by plain rendering.
            assert_eq!(
                to_line(&json!({ "v": input })),
                format!("v={expected}"),
                "to_line k=v float parity for {input}"
            );
        }
    }

    #[test]
    fn to_line_sorts_keys_alphabetically() {
        assert_eq!(to_line(&json!({"z": 1, "a": 2, "m": 3})), "a=2 m=3 z=1");
    }

    #[test]
    fn to_line_scalar_uses_json_form() {
        // Non-map values render their JSON form (Go toLine default branch).
        assert_eq!(to_line(&json!("hi")), "\"hi\"");
        assert_eq!(to_line(&json!(7)), "7");
        assert_eq!(to_line(&json!(false)), "false");
    }

    #[test]
    fn to_line_scalar_float_uses_json_form_not_go_v_scientific() {
        // CONTRACT NUANCE: the two plain paths diverge for big/small floats.
        //   * Map VALUE (`k=v`): Go uses fmt.Sprintf("%v") -> shortest 'g' ->
        //     scientific for |exp|>=6 (e.g. 1e+06).
        //   * Top-level SCALAR: Go's toLine default branch uses json.Marshal,
        //     which is the JSON decimal form and NEVER scientific at these
        //     magnitudes (1000000, 0.00001).
        // Probed against the Go binary (encoding/json.Marshal of a float64):
        //   json.Marshal(1000000.0) == "1000000"; json.Marshal(0.00001) == "0.00001".
        assert_eq!(
            to_line(&json!(1_000_000.0)),
            "1000000",
            "scalar float = JSON form"
        );
        assert_eq!(
            to_line(&json!(0.00001_f64)),
            "0.00001",
            "scalar float = JSON form"
        );
        assert_eq!(to_line(&json!(2.0_f64)), "2", "whole scalar float = '2'");
        assert_eq!(to_line(&json!(2.3_f64)), "2.3");
        // Same magnitudes INSIDE a map go through %v (format_go_g) and DO use
        // scientific — proving the two paths really differ.
        assert_eq!(to_line(&json!({"v": 1_000_000.0})), "v=1e+06");
        assert_eq!(to_line(&json!({"v": 0.00001_f64})), "v=1e-05");
    }

    // =========================================================================
    // Criterion 6 — key sort independent of input order
    // =========================================================================

    #[test]
    fn plain_key_sort_is_independent_of_insertion_order() {
        let a = success_env("x", json!([{"name": "x", "score": 42}]));
        let b = success_env("x", json!([{"score": 42, "name": "x"}]));
        let out_a = render(&a, &settings("plain", true, &[])).expect("render");
        let out_b = render(&b, &settings("plain", true, &[])).expect("render");
        assert_eq!(
            out_a, out_b,
            "plain output is independent of insertion order"
        );
        assert_eq!(out_a, "name=x score=42\n");
    }

    // =========================================================================
    // Criterion 7 — Render is settings-driven, not error-aware
    // =========================================================================

    #[test]
    fn results_only_applies_regardless_of_success_flag() {
        // An error envelope rendered with results_only=true would yield ONLY the
        // data ([]). (The runner avoids this by resetting results_only=false for
        // errors — but `render` itself must faithfully honor the settings it is
        // handed; it has no error special-casing.)
        let env = error_env("x", 12, "provider_unavailable", "boom");
        let out = render(&env, &settings("json", true, &[])).expect("render");
        assert_eq!(
            out, "[]\n",
            "results-only renders data only, even for errors"
        );
    }

    #[test]
    fn error_envelope_full_render_carries_error_and_bypass_cache() {
        // The runner-shaped call (results_only=false) renders the full envelope.
        let env = error_env("x", 10, "auth_error", "missing key");
        let out = render(&env, &settings("json", false, &[])).expect("render");
        let v: Value = serde_json::from_str(&out).expect("envelope json");
        assert_eq!(v["success"], json!(false));
        assert_eq!(v["data"], json!([]));
        assert_eq!(v["error"]["code"], json!(10));
        assert_eq!(v["error"]["type"], json!("auth_error"));
        assert_eq!(v["meta"]["cache"]["status"], json!("bypass"));
    }

    // =========================================================================
    // Criterion 1/4 (integration) — typed domain struct → to_value → render,
    // proving Go `encoding/json` float parity survives the runner pipeline.
    //
    // The runner builds `env.data` via `serde_json::to_value(domain_struct)`.
    // `defi-model`'s `go_float` serializer must keep whole-valued f64 as the
    // INTEGER JSON token (`1000000`, NOT serde's default `1000000.0`) all the
    // way through the Value tree into `defi-out`'s rendered bytes. Captured from
    // the Go binary (`json.Encoder` SetIndent("","  ") of []ChainTVL).
    // =========================================================================

    #[test]
    fn json_results_only_whole_float_field_has_no_trailing_dot_zero() {
        use defi_model::ChainTvl;
        let rows = vec![
            ChainTvl {
                rank: 1,
                chain: "Ethereum".into(),
                chain_id: "eip155:1".into(),
                tvl_usd: 1_000_000.0,
            },
            ChainTvl {
                rank: 2,
                chain: "Base".into(),
                chain_id: "eip155:8453".into(),
                tvl_usd: 2_500_000.55,
            },
        ];
        let data = serde_json::to_value(&rows).expect("to_value");
        let env = success_env("chains tvl", data);
        let out = render(&env, &settings("json", true, &[])).expect("render");
        let expected = "[\n  {\n    \"rank\": 1,\n    \"chain\": \"Ethereum\",\n    \"chain_id\": \"eip155:1\",\n    \"tvl_usd\": 1000000\n  },\n  {\n    \"rank\": 2,\n    \"chain\": \"Base\",\n    \"chain_id\": \"eip155:8453\",\n    \"tvl_usd\": 2500000.55\n  }\n]\n";
        assert_eq!(
            out, expected,
            "whole-valued f64 must render as integer JSON token (Go parity)"
        );
    }

    #[test]
    fn plain_results_only_whole_float_field_uses_go_v_scientific() {
        // Same data, PLAIN results-only. The k=v path runs Go `%v`
        // (format_go_g), so the whole 1e6 TVL becomes scientific `1e+06` while
        // the fractional one becomes `2.50000055e+06`. Keys sorted alpha.
        use defi_model::ChainTvl;
        let rows = vec![ChainTvl {
            rank: 1,
            chain: "Ethereum".into(),
            chain_id: "eip155:1".into(),
            tvl_usd: 1_000_000.0,
        }];
        let data = serde_json::to_value(&rows).expect("to_value");
        let env = success_env("chains tvl", data);
        let out = render(&env, &settings("plain", true, &[])).expect("render");
        assert_eq!(
            out,
            "chain=Ethereum chain_id=eip155:1 rank=1 tvl_usd=1e+06\n"
        );
    }

    // =========================================================================
    // Criterion 8 — no panics on the shapes the CLI emits
    // =========================================================================

    #[test]
    fn render_does_not_panic_on_nested_and_empty_shapes() {
        for data in [
            json!(null),
            json!([]),
            json!({}),
            json!([{"a": {"nested": [1, 2, 3]}}]),
            json!("scalar"),
        ] {
            let env = success_env("x", data.clone());
            // Both modes, results-only and full, must produce Ok(_).
            for mode in ["json", "plain"] {
                for ro in [true, false] {
                    let _ = render(&env, &settings(mode, ro, &[]))
                        .unwrap_or_else(|e| panic!("render({mode},{ro}) on {data:?}: {e}"));
                }
            }
        }
    }
}
