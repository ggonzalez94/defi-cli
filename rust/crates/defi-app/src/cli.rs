//! CLI argument parsing + top-level dispatch.
//!
//! This is the contract-bearing "glue" the Go `internal/app/runner.go` owns at
//! the `cobra` layer: parse global flags + a subcommand path, resolve
//! [`defi_config::Settings`] (precedence `flags > env > file > defaults`),
//! dispatch to a command-group handler, then render the result — **success to
//! stdout, errors to a full envelope on stderr** — and return the process exit
//! code.
//!
//! Only the deterministic, offline command surface with golden-parity coverage
//! is wired today (`version`, `schema`, `providers list`, `chains list`,
//! `assets resolve`); unwired/unknown paths produce a [`defi_errors::Code::Usage`]
//! error envelope (exit 2), matching the Go behavior for unknown commands.

use std::ffi::OsString;

use chrono::Utc;
use defi_config::{Env, GlobalFlags, Settings};
use defi_errors::{exit_code, Code, Error};
use defi_model::{CacheStatus, Envelope};

/// The outcome of a successful command: a fully-rendered output body printed to
/// stdout (the `version` plain line, or an envelope already rendered per
/// `settings`). The trailing newline is added by [`emit_success`].
struct Success(String);

/// Parse `args`, dispatch, render, and return the process exit code.
///
/// Splits `args` into the global flags + the subcommand path, resolves
/// [`Settings`] from `env`, runs the matching handler, and prints the result.
/// On any error a full error envelope is printed to **stderr** and the mapped
/// exit code is returned.
pub async fn run_with_args<I, T>(args: I, env: &dyn Env) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let argv: Vec<String> = args
        .into_iter()
        .map(|a| a.into().to_string_lossy().into_owned())
        .collect();

    // argv[0] is the program name; the rest are user tokens.
    let tokens: Vec<String> = argv.into_iter().skip(1).collect();

    match dispatch(&tokens, env).await {
        Ok(success) => emit_success(success),
        Err((command_path, err)) => emit_error(&command_path, &err),
    }
}

/// Parse global flags + subcommand path and route to a handler.
///
/// Returns the [`Success`] on success, or `(command_path, Error)` on failure so
/// the error envelope can carry the resolved command path.
async fn dispatch(tokens: &[String], env: &dyn Env) -> Result<Success, (String, Error)> {
    let parsed = match Parsed::from_tokens(tokens) {
        Ok(p) => p,
        // Conflicting/invalid global flags surface with no resolved command.
        Err(err) => return Err((String::new(), err)),
    };

    let command_path = parsed.command.join(" ");

    // `version` bypasses Settings/envelope entirely (plain text, exit 0).
    if parsed.command.first().map(String::as_str) == Some("version") {
        let long = parsed.bool_flag("long");
        return Ok(Success(crate::version::render(long)));
    }

    let settings = match Settings::load(&parsed.global, env) {
        Ok(s) => s,
        Err(err) => return Err((command_path, err)),
    };

    let envelope = route(&parsed).map_err(|e| (command_path.clone(), e))?;

    // Attach a request id + timestamp the way the Go runner does in
    // `emitSuccess` (the golden tests normalize both to sentinels).
    let mut envelope = envelope;
    envelope.meta.request_id = new_request_id();
    envelope.meta.timestamp = Utc::now();

    let rendered = match defi_out::render(&envelope, &settings) {
        Ok(s) => s,
        Err(err) => {
            return Err((
                command_path,
                Error::wrap(Code::Internal, "render output", err),
            ))
        }
    };
    Ok(Success(rendered.trim_end_matches('\n').to_string()))
}

/// Route a parsed command to its handler, returning the success [`Envelope`].
fn route(parsed: &Parsed) -> Result<Envelope, Error> {
    let cmd: Vec<&str> = parsed.command.iter().map(String::as_str).collect();
    match cmd.as_slice() {
        ["providers", "list"] => Ok(crate::providers::list()),
        ["chains", "list"] => Ok(chains_list_envelope()),
        ["assets", "resolve"] => crate::assets::run(
            &parsed.string_flag("chain"),
            &parsed.string_flag("symbol"),
            &parsed.string_flag("asset"),
        ),
        ["schema", rest @ ..] => {
            let root = schema_root();
            let path = rest.join(" ");
            crate::schema::run(&root, &path, &crate::schema::root_persistent_flags())
        }
        // Unknown / not-yet-wired command path → usage error (exit 2), matching
        // the Go "unknown command" behavior.
        [] => Err(Error::new(Code::Usage, "a command is required")),
        other => Err(Error::new(
            Code::Usage,
            format!("unknown command: {}", other.join(" ")),
        )),
    }
}

/// Build the `chains list` success envelope (metadata, cache bypassed).
fn chains_list_envelope() -> Envelope {
    let data =
        serde_json::to_value(crate::chains::list_chains_data()).unwrap_or(serde_json::Value::Null);
    Envelope::success(
        "chains list",
        data,
        Vec::new(),
        CacheStatus::bypass(),
        Vec::new(),
        false,
    )
}

/// The (partial) schema command tree used by `schema`.
///
/// NOTE: only the `version` and `schema` subtrees are populated today; the full
/// 19-command tree (required for whole-document golden parity with the Go
/// `schema.json`) is deferred integration work tracked in the remainder plan.
fn schema_root() -> crate::schema::CommandNode {
    crate::schema::CommandNode {
        name: "defi".to_string(),
        r#use: "defi".to_string(),
        short: "DeFi CLI".to_string(),
        persistent_flags: crate::schema::root_persistent_flags(),
        subcommands: vec![crate::schema::schema_node(), crate::schema::version_node()],
        ..crate::schema::CommandNode::leaf("defi", "defi", "DeFi CLI")
    }
}

/// Print a successful command result to stdout and return exit code 0.
fn emit_success(success: Success) -> i32 {
    println!("{}", success.0);
    0
}

/// Print the full error envelope to **stderr** and return the mapped exit code.
///
/// Mirrors the Go `renderError`: error output is ALWAYS the full envelope (even
/// under `--results-only`/`--select`), with `data=[]`, `cache.status="bypass"`,
/// and the code-derived `error.type`.
fn emit_error(command_path: &str, err: &Error) -> i32 {
    let command = if command_path.trim().is_empty() {
        "defi".to_string()
    } else {
        command_path.to_string()
    };
    let body = defi_model::ErrorBody {
        code: err.code.as_i32() as i64,
        error_type: error_type_for_code(err.code).to_string(),
        message: err.to_string(),
    };
    let mut env = Envelope::error(command, body, Vec::new(), Vec::new(), false);
    env.meta.request_id = new_request_id();
    env.meta.timestamp = Utc::now();

    // Error output is the full envelope regardless of results-only/select.
    match env.to_pretty_json() {
        Ok(s) => eprintln!("{s}"),
        Err(_) => eprintln!("{{\"version\":\"v1\",\"success\":false}}"),
    }
    exit_code(&Err(Error::new(err.code, "")))
}

/// The stable `error.type` string for a [`Code`] (mirrors the runner's mapping).
fn error_type_for_code(code: Code) -> &'static str {
    match code {
        Code::Usage => "usage_error",
        Code::Auth => "auth_error",
        Code::RateLimited => "rate_limited",
        Code::Unavailable => "provider_unavailable",
        Code::Unsupported => "unsupported",
        Code::Stale => "stale_data",
        Code::PartialStrict => "partial_results",
        Code::Blocked => "command_blocked",
        Code::ActionPlan => "action_plan_error",
        Code::ActionSim => "action_simulation_error",
        Code::ActionPolicy => "action_policy_error",
        Code::ActionTimeout => "action_timeout",
        Code::Signer => "signer_error",
        Code::Success | Code::Internal => "internal_error",
    }
}

/// Generate a 128-bit hex request id (mirrors the SHAPE of Go `newRequestID`:
/// `hex.EncodeToString(16 bytes)` → 32 lowercase hex chars).
///
/// The Go runner uses `crypto/rand`; the golden tests normalize `request_id` to
/// a sentinel so only the SHAPE (32 hex chars) is contract-relevant. We derive
/// 16 bytes from a SHA-256 over a high-resolution timestamp plus a
/// process-monotonic counter — unique per invocation without pulling in an RNG
/// dependency. (`sha2` is already a `defi-app` dependency.)
fn new_request_id() -> String {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(seq.to_le_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

/// Parsed CLI input: resolved global flags plus the subcommand path and the
/// per-command flag values (kept generic so this stays clap-free and easy to
/// drive from tests).
struct Parsed {
    global: GlobalFlags,
    command: Vec<String>,
    /// Command-level flag values by long name (without the `--`).
    flags: std::collections::HashMap<String, FlagValue>,
}

#[derive(Clone)]
enum FlagValue {
    Bool(bool),
    Str(String),
}

impl Parsed {
    /// Parse `tokens` (everything after argv[0]) into [`Parsed`].
    ///
    /// Recognizes the global persistent flags (consumed wherever they appear),
    /// treats the first non-flag tokens as the command path, and collects the
    /// remaining `--flag value` / `--flag=value` / `--bool` pairs as command
    /// flags. Conflicting `--json`/`--plain` is a usage error (matches the Go
    /// `config.Load` conflict).
    fn from_tokens(tokens: &[String]) -> Result<Parsed, Error> {
        let mut global = GlobalFlags::default();
        let mut command: Vec<String> = Vec::new();
        let mut flags: std::collections::HashMap<String, FlagValue> =
            std::collections::HashMap::new();

        let mut i = 0;
        while i < tokens.len() {
            let tok = &tokens[i];
            if let Some(rest) = tok.strip_prefix("--") {
                // Split `--name=value`.
                let (name, inline_value) = match rest.split_once('=') {
                    Some((n, v)) => (n.to_string(), Some(v.to_string())),
                    None => (rest.to_string(), None),
                };

                // Global boolean flags.
                match name.as_str() {
                    "json" => {
                        global.json = true;
                        i += 1;
                        continue;
                    }
                    "plain" => {
                        global.plain = true;
                        i += 1;
                        continue;
                    }
                    "results-only" => {
                        global.results_only = true;
                        i += 1;
                        continue;
                    }
                    "strict" => {
                        global.strict = true;
                        i += 1;
                        continue;
                    }
                    "no-stale" => {
                        global.no_stale = true;
                        i += 1;
                        continue;
                    }
                    "no-cache" => {
                        global.no_cache = true;
                        i += 1;
                        continue;
                    }
                    _ => {}
                }

                // Value-bearing flags: take the inline value or the next token.
                let value = match inline_value {
                    Some(v) => v,
                    None => {
                        let next = tokens.get(i + 1).cloned();
                        match next {
                            Some(v) if !v.starts_with("--") => {
                                i += 1;
                                v
                            }
                            _ => String::new(),
                        }
                    }
                };

                match name.as_str() {
                    "select" => global.select = Some(value),
                    "enable-commands" => global.enable_commands = Some(value),
                    "timeout" => global.timeout = Some(value),
                    "max-stale" => global.max_stale = Some(value),
                    "config" => global.config_path = Some(value),
                    "retries" => {
                        global.retries = value.parse::<i64>().ok();
                    }
                    other => {
                        // Command-level flag.
                        flags.insert(other.to_string(), FlagValue::Str(value));
                    }
                }
                i += 1;
            } else {
                // A non-flag token is part of the (space-separated) command
                // path (e.g. `chains list`, `schema yield plan`).
                command.push(tok.clone());
                i += 1;
            }
        }

        if global.json && global.plain {
            return Err(Error::new(
                Code::Usage,
                "cannot use both --json and --plain",
            ));
        }

        Ok(Parsed {
            global,
            command,
            flags,
        })
    }

    /// A command-level string flag value (empty when absent).
    fn string_flag(&self, name: &str) -> String {
        match self.flags.get(name) {
            Some(FlagValue::Str(v)) => v.clone(),
            _ => String::new(),
        }
    }

    /// A command-level boolean flag (`--long` style). Present-as-string also
    /// counts as set (clap-free leniency).
    fn bool_flag(&self, name: &str) -> bool {
        // The bare `--long` form is captured as a command string flag with an
        // empty value (no following value token), or as an explicit `=true`.
        match self.flags.get(name) {
            Some(FlagValue::Bool(b)) => *b,
            Some(FlagValue::Str(v)) => v.is_empty() || v == "true",
            None => false,
        }
    }
}
