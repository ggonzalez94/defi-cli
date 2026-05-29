//! Shared clap flag groups for execution commands.
//!
//! The execution `submit` / `status` flag sets are (nearly) uniform across the
//! `swap` / `bridge` / `lend` / `yield` / `rewards` / `approvals` / `transfer`
//! groups, so they are defined once here and flattened into each group's
//! subcommand structs. Keeping a single definition guarantees the schema tree
//! (WS6) and the runtime parser stay aligned, matching the Go execution flag
//! surface (`internal/app/runner.go` execution command builders).

use clap::Args;
use defi_errors::{Code, Error};

/// Structured-input flags shared by every `plan` / `submit` command
/// (`--input-json` / `--input-file`; explicit flags override these values).
#[derive(Args, Debug, Clone, Default)]
pub struct InputFlags {
    /// Structured request JSON.
    #[arg(long = "input-json")]
    pub input_json: Option<String>,
    /// Path to structured request JSON file ('-' for stdin).
    #[arg(long = "input-file")]
    pub input_file: Option<String>,
}

/// Resolve the structured-input payload string from `--input-json` /
/// `--input-file` (`-` = stdin), enforcing mutual exclusivity (Go
/// `readStructuredInput`).
///
/// Returns `Ok(None)` when neither input is provided. A populated `--input-json`
/// and `--input-file` together is a usage error.
pub fn read_structured_input(input: &InputFlags) -> Result<Option<String>, Error> {
    let json = input.input_json.as_deref().unwrap_or("").trim().to_string();
    let file = input.input_file.as_deref().unwrap_or("").trim().to_string();
    if !json.is_empty() && !file.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "use only one of --input-json or --input-file",
        ));
    }
    if !json.is_empty() {
        return Ok(Some(json));
    }
    if file.is_empty() {
        return Ok(None);
    }
    if file == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| Error::wrap(Code::Usage, "read structured input from stdin", e))?;
        return Ok(Some(buf));
    }
    let buf = std::fs::read_to_string(&file)
        .map_err(|e| Error::wrap(Code::Usage, "read structured input file", e))?;
    Ok(Some(buf))
}

/// Merge structured input (`--input-json` / `--input-file`) onto a command's
/// resolved flag values (Go `applyStructuredFlagInput`).
///
/// Parity with the Go runner's `PreRunE` merge:
/// - reads the payload (mutually-exclusive `--input-json` / `--input-file`;
///   `-` reads stdin) and skips merge when empty;
/// - the payload must be a JSON object (non-object → usage error);
/// - each key is canonicalized (`_` → `-`, trimmed) before lookup;
/// - explicitly-set flags are never overridden (`explicit` carries canonical
///   flag names);
/// - a `null` value is a usage error (matching Go, BEFORE the explicit check is
///   irrelevant — Go checks explicit first, then null);
/// - the `set` callback applies a recognized key and returns `Ok(true)`; an
///   unrecognized key returns `Ok(false)` → usage error
///   `structured input field "<key>" is not supported by <command>`.
///
/// `command` is the trimmed command path (e.g. `"swap plan"`) used verbatim in
/// the unsupported-field error, matching the Go message format. The `set`
/// callback receives the ORIGINAL key (for typed-decode error messages) plus the
/// canonical key and the raw JSON value.
pub fn apply_structured_input<F>(
    input: &InputFlags,
    explicit: &std::collections::HashSet<&str>,
    command: &str,
    mut set: F,
) -> Result<(), Error>
where
    F: FnMut(&str, &str, &serde_json::Value) -> Result<bool, Error>,
{
    let payload = match read_structured_input(input)? {
        Some(p) if !p.trim().is_empty() => p,
        _ => return Ok(()),
    };

    let parsed: serde_json::Value = serde_json::from_str(&payload)
        .map_err(|e| Error::wrap(Code::Usage, "parse structured input", e))?;
    let obj = parsed
        .as_object()
        .ok_or_else(|| Error::new(Code::Usage, "structured input must be a JSON object"))?;

    for (key, raw) in obj {
        let canonical = key.trim().replace('_', "-");
        // Go checks the explicit (changed) flags BEFORE the null guard, so an
        // explicitly-set flag with a null JSON value is silently skipped.
        if explicit.contains(canonical.as_str()) {
            continue;
        }
        if raw.is_null() {
            return Err(Error::new(
                Code::Usage,
                format!("structured input field {key:?} cannot be null"),
            ));
        }
        if !set(key, &canonical, raw)? {
            return Err(Error::new(
                Code::Usage,
                format!("structured input field {key:?} is not supported by {command}"),
            ));
        }
    }
    Ok(())
}

/// Decode a JSON value destined for a `string` flag (Go `decodeRawFlagValue`
/// `case "string"`): only a JSON string is accepted; a number/bool/etc. is a
/// decode error, matching Go's `json.Unmarshal` into a `string`.
pub fn decode_string_field(key: &str, raw: &serde_json::Value) -> Result<String, Error> {
    match raw {
        serde_json::Value::String(s) => Ok(s.clone()),
        _ => Err(Error::new(
            Code::Usage,
            format!("decode structured input field {key:?}: expected a JSON string"),
        )),
    }
}

/// Decode a JSON value destined for a `bool` flag (Go `decodeRawFlagValue`
/// `case "bool"`): only a JSON boolean is accepted.
pub fn decode_bool_field(key: &str, raw: &serde_json::Value) -> Result<bool, Error> {
    match raw {
        serde_json::Value::Bool(b) => Ok(*b),
        _ => Err(Error::new(
            Code::Usage,
            format!("decode structured input field {key:?}: expected a JSON boolean"),
        )),
    }
}

/// Decode a JSON value destined for an `int64` flag (Go `decodeRawFlagValue`
/// `case "int64"`): only a JSON integer is accepted.
pub fn decode_i64_field(key: &str, raw: &serde_json::Value) -> Result<i64, Error> {
    match raw {
        serde_json::Value::Number(n) => n.as_i64().ok_or_else(|| {
            Error::new(
                Code::Usage,
                format!("decode structured input field {key:?}: expected a JSON integer"),
            )
        }),
        _ => Err(Error::new(
            Code::Usage,
            format!("decode structured input field {key:?}: expected a JSON integer"),
        )),
    }
}

/// Decode a JSON value destined for a `f64` flag (Go `decodeRawFlagValue`
/// `case "float64"`): only a JSON number is accepted.
pub fn decode_f64_field(key: &str, raw: &serde_json::Value) -> Result<f64, Error> {
    match raw {
        serde_json::Value::Number(n) => n.as_f64().ok_or_else(|| {
            Error::new(
                Code::Usage,
                format!("decode structured input field {key:?}: expected a JSON number"),
            )
        }),
        _ => Err(Error::new(
            Code::Usage,
            format!("decode structured input field {key:?}: expected a JSON number"),
        )),
    }
}

/// Decode a JSON value destined for a `stringSlice`/`stringArray` flag (Go
/// `decodeRawFlagValue` `case "stringSlice"`): a JSON array of strings, or a
/// single JSON string (kept as one element). The Go code joins on `,`; we return
/// the element vector so callers can store it directly.
pub fn decode_string_slice_field(key: &str, raw: &serde_json::Value) -> Result<Vec<String>, Error> {
    match raw {
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    serde_json::Value::String(s) => out.push(s.clone()),
                    _ => {
                        return Err(Error::new(
                            Code::Usage,
                            format!(
                            "decode structured input field {key:?}: expected a JSON string array"
                        ),
                        ))
                    }
                }
            }
            Ok(out)
        }
        serde_json::Value::String(s) => Ok(vec![s.clone()]),
        _ => Err(Error::new(
            Code::Usage,
            format!(
                "decode structured input field {key:?}: expected a JSON string or string array"
            ),
        )),
    }
}

/// Identity flags shared by every `plan` command: OWS-first `--wallet`, local
/// signer `--from-address`. (`bridge`/`swap`/`lend`/`yield`/`rewards` plans.)
#[derive(Args, Debug, Clone, Default)]
pub struct PlanIdentityFlags {
    /// Wallet identifier or name (OWS-first identity input).
    #[arg(long)]
    pub wallet: Option<String>,
    /// Sender EOA address (local signer identity input).
    #[arg(long = "from-address")]
    pub from_address: Option<String>,
}

/// The full `submit` flag surface shared by `swap`/`bridge`/`lend`/`yield`/
/// `rewards`/`approvals` submit (transfer submit omits the approval/provider-tx
/// guardrail flags — see [`TransferSubmitArgs`]).
#[derive(Args, Debug, Clone, Default)]
pub struct SubmitArgs {
    /// Action identifier returned by the corresponding plan command.
    #[arg(long = "action-id")]
    pub action_id: Option<String>,
    /// Expected sender EOA address.
    #[arg(long = "from-address")]
    pub from_address: Option<String>,
    /// Allow approval amounts greater than planned input amount.
    #[arg(long = "allow-max-approval")]
    pub allow_max_approval: bool,
    /// Bypass provider transaction guardrails for bridge/aggregator payloads.
    #[arg(long = "unsafe-provider-tx")]
    pub unsafe_provider_tx: bool,
    /// Signer backend (local|tempo).
    #[arg(long, default_value = "local")]
    pub signer: String,
    /// Key source (auto|env|file|keystore).
    #[arg(long = "key-source", default_value = "auto")]
    pub key_source: String,
    /// Private key hex override for local signer (less safe).
    #[arg(long = "private-key")]
    pub private_key: Option<String>,
    /// Fee token address for Tempo chains (defaults to chain USDC.e).
    #[arg(long = "fee-token")]
    pub fee_token: Option<String>,
    /// Gas estimate safety multiplier.
    #[arg(long = "gas-multiplier", default_value_t = 1.2)]
    pub gas_multiplier: f64,
    /// Optional EIP-1559 max fee (gwei).
    #[arg(long = "max-fee-gwei")]
    pub max_fee_gwei: Option<String>,
    /// Optional EIP-1559 max priority fee (gwei).
    #[arg(long = "max-priority-fee-gwei")]
    pub max_priority_fee_gwei: Option<String>,
    /// Run preflight simulation before submission.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub simulate: bool,
    /// Receipt polling interval.
    #[arg(long = "poll-interval", default_value = "2s")]
    pub poll_interval: String,
    /// Per-step receipt timeout.
    #[arg(long = "step-timeout", default_value = "2m")]
    pub step_timeout: String,
    #[command(flatten)]
    pub input: InputFlags,
}

/// The `transfer submit` flag surface (no approval/provider-tx guardrails).
#[derive(Args, Debug, Clone, Default)]
pub struct TransferSubmitArgs {
    /// Action identifier returned by transfer plan.
    #[arg(long = "action-id")]
    pub action_id: Option<String>,
    /// Expected sender EOA address.
    #[arg(long = "from-address")]
    pub from_address: Option<String>,
    /// Signer backend (local|tempo).
    #[arg(long, default_value = "local")]
    pub signer: String,
    /// Key source (auto|env|file|keystore).
    #[arg(long = "key-source", default_value = "auto")]
    pub key_source: String,
    /// Private key hex override for local signer (less safe).
    #[arg(long = "private-key")]
    pub private_key: Option<String>,
    /// Fee token address for Tempo chains (defaults to chain USDC.e).
    #[arg(long = "fee-token")]
    pub fee_token: Option<String>,
    /// Gas estimate safety multiplier.
    #[arg(long = "gas-multiplier", default_value_t = 1.2)]
    pub gas_multiplier: f64,
    /// Optional EIP-1559 max fee (gwei).
    #[arg(long = "max-fee-gwei")]
    pub max_fee_gwei: Option<String>,
    /// Optional EIP-1559 max priority fee (gwei).
    #[arg(long = "max-priority-fee-gwei")]
    pub max_priority_fee_gwei: Option<String>,
    /// Run preflight simulation before submission.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub simulate: bool,
    /// Receipt polling interval.
    #[arg(long = "poll-interval", default_value = "2s")]
    pub poll_interval: String,
    /// Per-step receipt timeout.
    #[arg(long = "step-timeout", default_value = "2m")]
    pub step_timeout: String,
    #[command(flatten)]
    pub input: InputFlags,
}

/// The `status` flag surface shared by every execution group (`--action-id`).
#[derive(Args, Debug, Clone, Default)]
pub struct StatusArgs {
    /// Action identifier returned by the corresponding plan command.
    #[arg(long = "action-id")]
    pub action_id: Option<String>,
}
