//! CLI argument parsing (clap derive) + top-level dispatch.
//!
//! This is the contract-bearing "glue" the Go `internal/app/runner.go` owns at
//! the `cobra` layer. The clap [`Cli`] tree here is the **single source of
//! truth** for the whole command surface — every leaf command, its flags,
//! enums, and input modes — and the schema command (WS6) will derive from the
//! same tree. Dispatch resolves [`defi_config::Settings`] (precedence
//! `flags > env > file > defaults`), builds an [`AppCtx`], routes to the owning
//! command-group handler, then renders the result: **success to stdout, errors
//! as a full envelope on stderr**, returning the process exit code.
//!
//! Handlers that are not yet ported return a typed [`defi_errors::Code::Unsupported`]
//! "not yet implemented in Rust port" error — NOT an "unknown command" usage
//! error. Every real Go command therefore routes to a handler.

use std::ffi::OsString;

use clap::{Args, Parser, Subcommand};
use defi_config::{Env, GlobalFlags};
use defi_errors::{exit_code, Code, Error};
use defi_model::Envelope;

use crate::ctx::AppCtx;

// ---------------------------------------------------------------------------
// Global persistent flags (cobra "Global Flags").
// ---------------------------------------------------------------------------

/// The persistent flags available on every command (cobra "Global Flags").
///
/// Field order + names mirror the Go root command's persistent flag set so the
/// schema tree (WS6) and `--help` stay aligned. `--json`/`--plain` conflict is
/// enforced by `Settings::load` (matching `config.Load`).
#[derive(Args, Debug, Clone, Default)]
pub struct GlobalArgs {
    /// Path to config file.
    #[arg(long, global = true)]
    pub config: Option<String>,
    /// Output JSON (default).
    #[arg(long, global = true)]
    pub json: bool,
    /// Output plain text.
    #[arg(long, global = true)]
    pub plain: bool,
    /// Select fields from data (comma-separated).
    #[arg(long, global = true)]
    pub select: Option<String>,
    /// Output only data payload.
    #[arg(long = "results-only", global = true)]
    pub results_only: bool,
    /// Allowlist command paths (comma-separated).
    #[arg(long = "enable-commands", global = true)]
    pub enable_commands: Option<String>,
    /// Fail on partial results.
    #[arg(long, global = true)]
    pub strict: bool,
    /// Provider request timeout.
    #[arg(long, global = true)]
    pub timeout: Option<String>,
    /// Retries per provider request.
    #[arg(long, global = true)]
    pub retries: Option<i64>,
    /// Maximum stale fallback window after TTL expiry.
    #[arg(long = "max-stale", global = true)]
    pub max_stale: Option<String>,
    /// Reject stale cache entries.
    #[arg(long = "no-stale", global = true)]
    pub no_stale: bool,
    /// Disable cache reads and writes.
    #[arg(long = "no-cache", global = true)]
    pub no_cache: bool,
}

impl GlobalArgs {
    /// Map the parsed global flags into the config-layer [`GlobalFlags`].
    fn to_global_flags(&self) -> GlobalFlags {
        GlobalFlags {
            config_path: self.config.clone(),
            json: self.json,
            plain: self.plain,
            select: self.select.clone(),
            results_only: self.results_only,
            enable_commands: self.enable_commands.clone(),
            strict: self.strict,
            timeout: self.timeout.clone(),
            retries: self.retries,
            max_stale: self.max_stale.clone(),
            no_stale: self.no_stale,
            no_cache: self.no_cache,
        }
    }
}

// ---------------------------------------------------------------------------
// Root command tree.
// ---------------------------------------------------------------------------

/// The `defi` CLI: an agent-first DeFi retrieval CLI.
#[derive(Parser, Debug)]
#[command(
    name = "defi",
    about = "Agent-first DeFi retrieval CLI",
    disable_help_subcommand = false,
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: TopCommand,
}

/// The top-level command groups (mirrors the Go root `AddCommand` set).
///
/// Each group whose payload is itself a [`Subcommand`] enum is flattened with
/// `#[command(subcommand)]`; `version` / `schema` are leaf args structs.
#[derive(Subcommand, Debug)]
pub enum TopCommand {
    /// Print CLI version.
    Version(crate::version::cli::VersionArgs),
    /// Print machine-readable command schema.
    Schema(crate::schema::cli::SchemaArgs),
    /// Provider commands.
    Providers {
        #[command(subcommand)]
        cmd: crate::providers::cli::ProvidersCmd,
    },
    /// Asset helpers.
    Assets {
        #[command(subcommand)]
        cmd: crate::assets::cli::AssetsCmd,
    },
    /// Wallet helpers.
    Wallet {
        #[command(subcommand)]
        cmd: crate::wallet::cli::WalletCmd,
    },
    /// Chain market data.
    Chains {
        #[command(subcommand)]
        cmd: crate::chains::cli::ChainsCmd,
    },
    /// Protocol market data.
    Protocols {
        #[command(subcommand)]
        cmd: crate::protocols::cli::ProtocolsCmd,
    },
    /// Stablecoin market data.
    Stablecoins {
        #[command(subcommand)]
        cmd: crate::stablecoins::cli::StablecoinsCmd,
    },
    /// DEX market data.
    Dexes {
        #[command(subcommand)]
        cmd: crate::dexes::cli::DexesCmd,
    },
    /// Lending data.
    Lend {
        #[command(subcommand)]
        cmd: crate::lend::cli::LendCmd,
    },
    /// Yield opportunities, positions, history, and execution.
    Yield {
        #[command(subcommand)]
        cmd: crate::r#yield::cli::YieldCmd,
    },
    /// Swap quote and execution commands.
    Swap {
        #[command(subcommand)]
        cmd: crate::swap::cli::SwapCmd,
    },
    /// Bridge quote and analytics commands.
    Bridge {
        #[command(subcommand)]
        cmd: crate::bridge::cli::BridgeCmd,
    },
    /// Approval execution commands.
    Approvals {
        #[command(subcommand)]
        cmd: crate::approvals::cli::ApprovalsCmd,
    },
    /// ERC-20 transfer execution commands.
    Transfer {
        #[command(subcommand)]
        cmd: crate::transfer::cli::TransferCmd,
    },
    /// Rewards claim and compound execution commands.
    Rewards {
        #[command(subcommand)]
        cmd: crate::rewards::cli::RewardsCmd,
    },
    /// Execution action inspection commands.
    Actions {
        #[command(subcommand)]
        cmd: crate::actions::cli::ActionsCmd,
    },
}

impl TopCommand {
    /// The space-joined command path for envelope `meta.command` / schema keys.
    fn command_path(&self) -> String {
        match self {
            TopCommand::Version(_) => "version".to_string(),
            TopCommand::Schema(_) => "schema".to_string(),
            TopCommand::Providers { cmd } => format!("providers {}", cmd.path()),
            TopCommand::Assets { cmd } => format!("assets {}", cmd.path()),
            TopCommand::Wallet { cmd } => format!("wallet {}", cmd.path()),
            TopCommand::Chains { cmd } => format!("chains {}", cmd.path()),
            TopCommand::Protocols { cmd } => format!("protocols {}", cmd.path()),
            TopCommand::Stablecoins { cmd } => format!("stablecoins {}", cmd.path()),
            TopCommand::Dexes { cmd } => format!("dexes {}", cmd.path()),
            TopCommand::Lend { cmd } => format!("lend {}", cmd.path()),
            TopCommand::Yield { cmd } => format!("yield {}", cmd.path()),
            TopCommand::Swap { cmd } => format!("swap {}", cmd.path()),
            TopCommand::Bridge { cmd } => format!("bridge {}", cmd.path()),
            TopCommand::Approvals { cmd } => format!("approvals {}", cmd.path()),
            TopCommand::Transfer { cmd } => format!("transfer {}", cmd.path()),
            TopCommand::Rewards { cmd } => format!("rewards {}", cmd.path()),
            TopCommand::Actions { cmd } => format!("actions {}", cmd.path()),
        }
        .trim()
        .to_string()
    }
}

// ---------------------------------------------------------------------------
// Entry point + dispatch.
// ---------------------------------------------------------------------------

/// Parse `args`, dispatch, render, and return the process exit code.
pub async fn run_with_args<I, T>(args: I, env: &dyn Env) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => return emit_clap_error(err),
    };

    let command_path = cli.command.command_path();

    // `version` bypasses Settings/envelope entirely (plain text, exit 0).
    if let TopCommand::Version(args) = &cli.command {
        println!("{}", crate::version::render(args.long));
        return 0;
    }

    let flags = cli.global.to_global_flags();
    let settings = match defi_config::Settings::load(&flags, env) {
        Ok(s) => s,
        Err(err) => return emit_error(&command_path, &err),
    };

    let ctx = AppCtx::new(settings);

    match dispatch(&ctx, cli.command).await {
        Ok(envelope) => emit_success(&ctx, envelope),
        Err(err) => emit_error(&command_path, &err),
    }
}

/// Route a parsed command to its owning group handler, returning the success
/// [`Envelope`].
///
/// Every command path resolves to exactly one handler. Handlers that are not
/// yet ported return a typed [`defi_errors::Code::Unsupported`] error from
/// inside their own group module (never `unknown command`).
async fn dispatch(ctx: &AppCtx, command: TopCommand) -> Result<Envelope, Error> {
    match command {
        // version is handled before dispatch (plain text).
        TopCommand::Version(_) => unreachable!("version handled before dispatch"),
        TopCommand::Schema(args) => crate::schema::cli::handle(ctx, args),
        TopCommand::Providers { cmd } => crate::providers::cli::handle(ctx, cmd).await,
        TopCommand::Assets { cmd } => crate::assets::cli::handle(ctx, cmd).await,
        TopCommand::Wallet { cmd } => crate::wallet::cli::handle(ctx, cmd).await,
        TopCommand::Chains { cmd } => crate::chains::cli::handle(ctx, cmd).await,
        TopCommand::Protocols { cmd } => crate::protocols::cli::handle(ctx, cmd).await,
        TopCommand::Stablecoins { cmd } => crate::stablecoins::cli::handle(ctx, cmd).await,
        TopCommand::Dexes { cmd } => crate::dexes::cli::handle(ctx, cmd).await,
        TopCommand::Lend { cmd } => crate::lend::cli::handle(ctx, cmd).await,
        TopCommand::Yield { cmd } => crate::r#yield::cli::handle(ctx, cmd).await,
        TopCommand::Swap { cmd } => crate::swap::cli::handle(ctx, cmd).await,
        TopCommand::Bridge { cmd } => crate::bridge::cli::handle(ctx, cmd).await,
        TopCommand::Approvals { cmd } => crate::approvals::cli::handle(ctx, cmd).await,
        TopCommand::Transfer { cmd } => crate::transfer::cli::handle(ctx, cmd).await,
        TopCommand::Rewards { cmd } => crate::rewards::cli::handle(ctx, cmd).await,
        TopCommand::Actions { cmd } => crate::actions::cli::handle(ctx, cmd).await,
    }
}

// ---------------------------------------------------------------------------
// Output emission.
// ---------------------------------------------------------------------------

/// Print a successful command result to stdout (rendered per settings) and
/// return exit code 0. Attaches a request id + timestamp the way the Go runner
/// does in `emitSuccess` (golden tests normalize both).
fn emit_success(ctx: &AppCtx, mut envelope: Envelope) -> i32 {
    envelope.meta.request_id = ctx.request_id();
    if envelope.meta.timestamp.timestamp() == 0 {
        envelope.meta.timestamp = ctx.now();
    }
    match defi_out::render(&envelope, &ctx.settings) {
        Ok(rendered) => {
            println!("{}", rendered.trim_end_matches('\n'));
            0
        }
        Err(err) => emit_error(
            &envelope.meta.command,
            &Error::wrap(Code::Internal, "render output", err),
        ),
    }
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
    env.meta.timestamp = chrono::Utc::now();

    match env.to_pretty_json() {
        Ok(s) => eprintln!("{s}"),
        Err(_) => eprintln!("{{\"version\":\"v1\",\"success\":false}}"),
    }
    exit_code(&Err(Error::new(err.code, "")))
}

/// Convert a clap parse failure into the machine contract: `--help`/`--version`
/// requests print to stdout (exit 0); a genuine parse error becomes a full
/// usage error envelope on stderr (exit 2), matching the Go runner's
/// `normalizeRunError` classification of cobra usage failures.
fn emit_clap_error(err: clap::Error) -> i32 {
    use clap::error::ErrorKind;
    match err.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            // clap renders help/version to the appropriate stream itself.
            print!("{err}");
            0
        }
        _ => {
            // A genuine usage failure (unknown command/flag, missing value,
            // bad enum, etc.) → full usage-error envelope on stderr, exit 2.
            let message = first_line(&err.to_string());
            emit_error("defi", &Error::new(Code::Usage, message))
        }
    }
}

/// The first non-empty line of a (possibly multi-line) clap error message,
/// stripped of the leading `error: ` prefix clap adds.
fn first_line(message: &str) -> String {
    let line = message
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("invalid command input")
        .trim();
    line.strip_prefix("error: ").unwrap_or(line).to_string()
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

/// Generate a 128-bit hex request id for error envelopes (no `AppCtx` in scope).
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

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::cli` (Go: `internal/app/runner.go` cobra
    //! root + dispatch)
    //!
    //! This module owns the **clap command tree + dispatch skeleton** (WS0): the
    //! single source of truth for the whole command surface. "Correct" for WS0
    //! means:
    //!
    //! 1. **Every real Go command routes to a handler.** Each of the 65 real
    //!    leaf command paths (the full 19-group tree captured in
    //!    `rust/tests/golden/schema.json`, excluding the cobra-native `help` /
    //!    `completion` leaves deferred to WS7) parses through the clap [`Cli`]
    //!    tree AND its [`TopCommand::command_path`] equals the expected
    //!    space-joined path. No real command falls through to an "unknown
    //!    command" usage error (design spec §2.5; completion-plan WS0
    //!    acceptance).
    //! 2. **Unimplemented leaves return a typed [`Code::Unsupported`] "not yet
    //!    implemented" error — NOT "unknown command".** Dispatching any leaf
    //!    whose handler is still a stub yields `Code::Unsupported` whose message
    //!    names the owning workstream, so the gap is traceable and never looks
    //!    like a routing failure.
    //! 3. **A genuinely unknown command IS a clap usage failure** (exit 2),
    //!    distinct from (2). (Mirrors cobra's `unknown command` → usage error.)
    //! 4. **Parser flag surface.** Representative per-group flag cases parse: the
    //!    shared `--input-json` / `--input-file` structured-input modes, the
    //!    string-passthrough enum flags (`--type`, `--signer`), execution
    //!    identity flags (`--wallet` / `--from-address`), and the
    //!    `--json`/`--plain` output conflict (a `Settings::load` usage error,
    //!    matching cobra letting both flags through and the runner rejecting).
    //!
    //! SKIPPED (owned elsewhere / later workstreams): per-handler request
    //! validation and provider/cache I/O (the group modules' own unit tests +
    //! WS1–WS4 wiremock tests), full `schema` tree parity (WS6), and clap-native
    //! `help`/`completion` generation (WS7).

    use super::*;
    use defi_config::Settings;
    use std::path::PathBuf;
    use std::time::Duration;

    /// A no-cache, no-network [`Settings`] for routing/dispatch tests. Stub
    /// handlers return immediately (no cache/network), so dispatch is safe and
    /// fast; cache is disabled so wired-but-not-exercised paths never touch disk.
    fn test_settings() -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: false,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_secs(2),
            retries: 0,
            max_stale: Duration::from_secs(0),
            no_stale: false,
            cache_enabled: false,
            cache_path: PathBuf::new(),
            cache_lock_path: PathBuf::new(),
            action_store_path: PathBuf::new(),
            action_lock_path: PathBuf::new(),
            defillama_api_key: String::new(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    /// The full set of **real** leaf command paths (65) with a minimal valid
    /// argv that parses through the clap tree. Mirrors the leaves enumerated in
    /// `rust/tests/golden/schema.json` minus the cobra-native `help` and four
    /// `completion <shell>` leaves (deferred to WS7). Each entry is
    /// `(expected_command_path, &[argv_after_program_name])`.
    fn all_real_commands() -> Vec<(&'static str, Vec<&'static str>)> {
        vec![
            // --- metadata / read groups ------------------------------------
            ("version", vec!["version"]),
            ("schema", vec!["schema"]),
            ("providers list", vec!["providers", "list"]),
            ("assets resolve", vec!["assets", "resolve"]),
            ("wallet balance", vec!["wallet", "balance"]),
            ("chains list", vec!["chains", "list"]),
            ("chains gas", vec!["chains", "gas"]),
            ("chains top", vec!["chains", "top"]),
            // `chains assets` requires `--chain` at the clap level (Go cobra
            // `MarkFlagRequired("chain")`), so the routing argv supplies it.
            ("chains assets", vec!["chains", "assets", "--chain", "1"]),
            ("protocols top", vec!["protocols", "top"]),
            ("protocols categories", vec!["protocols", "categories"]),
            ("protocols fees", vec!["protocols", "fees"]),
            ("protocols revenue", vec!["protocols", "revenue"]),
            ("stablecoins top", vec!["stablecoins", "top"]),
            ("stablecoins chains", vec!["stablecoins", "chains"]),
            ("dexes volume", vec!["dexes", "volume"]),
            ("lend markets", vec!["lend", "markets"]),
            ("lend rates", vec!["lend", "rates"]),
            ("lend positions", vec!["lend", "positions"]),
            ("yield opportunities", vec!["yield", "opportunities"]),
            ("yield positions", vec!["yield", "positions"]),
            ("yield history", vec!["yield", "history"]),
            ("swap quote", vec!["swap", "quote"]),
            ("bridge quote", vec!["bridge", "quote"]),
            ("bridge list", vec!["bridge", "list"]),
            ("bridge details", vec!["bridge", "details"]),
            // --- execution: swap / bridge / transfer / approvals -----------
            ("swap plan", vec!["swap", "plan"]),
            ("swap submit", vec!["swap", "submit"]),
            ("swap status", vec!["swap", "status"]),
            ("bridge plan", vec!["bridge", "plan"]),
            ("bridge submit", vec!["bridge", "submit"]),
            ("bridge status", vec!["bridge", "status"]),
            ("transfer plan", vec!["transfer", "plan"]),
            ("transfer submit", vec!["transfer", "submit"]),
            ("transfer status", vec!["transfer", "status"]),
            ("approvals plan", vec!["approvals", "plan"]),
            ("approvals submit", vec!["approvals", "submit"]),
            ("approvals status", vec!["approvals", "status"]),
            // --- execution: lend verbs × plan/submit/status ----------------
            ("lend supply plan", vec!["lend", "supply", "plan"]),
            ("lend supply submit", vec!["lend", "supply", "submit"]),
            ("lend supply status", vec!["lend", "supply", "status"]),
            ("lend withdraw plan", vec!["lend", "withdraw", "plan"]),
            ("lend withdraw submit", vec!["lend", "withdraw", "submit"]),
            ("lend withdraw status", vec!["lend", "withdraw", "status"]),
            ("lend borrow plan", vec!["lend", "borrow", "plan"]),
            ("lend borrow submit", vec!["lend", "borrow", "submit"]),
            ("lend borrow status", vec!["lend", "borrow", "status"]),
            ("lend repay plan", vec!["lend", "repay", "plan"]),
            ("lend repay submit", vec!["lend", "repay", "submit"]),
            ("lend repay status", vec!["lend", "repay", "status"]),
            // --- execution: yield verbs × plan/submit/status ---------------
            ("yield deposit plan", vec!["yield", "deposit", "plan"]),
            ("yield deposit submit", vec!["yield", "deposit", "submit"]),
            ("yield deposit status", vec!["yield", "deposit", "status"]),
            ("yield withdraw plan", vec!["yield", "withdraw", "plan"]),
            ("yield withdraw submit", vec!["yield", "withdraw", "submit"]),
            ("yield withdraw status", vec!["yield", "withdraw", "status"]),
            // --- execution: rewards verbs × plan/submit/status -------------
            ("rewards claim plan", vec!["rewards", "claim", "plan"]),
            ("rewards claim submit", vec!["rewards", "claim", "submit"]),
            ("rewards claim status", vec!["rewards", "claim", "status"]),
            ("rewards compound plan", vec!["rewards", "compound", "plan"]),
            (
                "rewards compound submit",
                vec!["rewards", "compound", "submit"],
            ),
            (
                "rewards compound status",
                vec!["rewards", "compound", "status"],
            ),
            // --- actions inspection ----------------------------------------
            ("actions list", vec!["actions", "list"]),
            ("actions show", vec!["actions", "show"]),
            ("actions estimate", vec!["actions", "estimate"]),
        ]
    }

    /// The leaves whose handlers are still WS1–WS4 stubs (return the typed
    /// `Unsupported` "not yet implemented" error). The complement of these
    /// within `all_real_commands` is the already-wired surface (`version`,
    /// `schema`, `providers list`, `assets resolve`, `chains list`,
    /// `chains gas`, the `protocols`/`stablecoins`/`dexes` market data, the
    /// `lend markets`/`lend rates`/`lend positions` reads, the
    /// `yield opportunities`/`yield positions`/`yield history` reads,
    /// `swap quote`, and the `bridge quote`/`bridge list`/`bridge details`
    /// reads), which we route-check by parse + `command_path` only (dispatching
    /// them would do real provider/cache I/O, or — for the lend reads,
    /// `swap quote`, and `bridge quote` — require `--provider`).
    fn is_stub(path: &str) -> bool {
        // `chains top` / `chains assets` are wired (WS2 unit "chains-extra");
        // they are now route-verified by parse + command_path above and exercised
        // end-to-end by their own module tests, so they are no longer stubs.
        matches!(path, "wallet balance")
            || path.ends_with(" plan")
            || path.ends_with(" submit")
            || path.ends_with(" status")
            || path.starts_with("actions ")
    }

    // --- 1 & 2. routing: every real command resolves to a handler ----------

    #[tokio::test(flavor = "multi_thread")]
    async fn every_real_command_routes_to_a_handler() {
        let ctx = AppCtx::new(test_settings());
        for (expected_path, argv) in all_real_commands() {
            // Prepend the program name (clap expects argv[0]).
            let mut full = vec!["defi"];
            full.extend(argv.iter().copied());

            let cli = Cli::try_parse_from(&full)
                .unwrap_or_else(|e| panic!("`{}` should parse: {e}", full.join(" ")));
            assert_eq!(
                cli.command.command_path(),
                expected_path,
                "command_path mismatch for `{}`",
                full.join(" ")
            );

            // version is handled before dispatch (plain text); skip dispatch.
            if expected_path == "version" {
                continue;
            }

            // Only dispatch the stub leaves: they return immediately with the
            // typed Unsupported error and never touch provider/cache I/O. Wired
            // leaves are route-verified by parse + command_path above (their own
            // module tests + WS1–WS4 cover dispatch).
            if !is_stub(expected_path) {
                continue;
            }

            let result = dispatch(&ctx, cli.command).await;
            let err = result.expect_err(&format!(
                "stub `{expected_path}` should return a typed Unsupported error"
            ));
            assert_eq!(
                err.code,
                Code::Unsupported,
                "stub `{expected_path}` should be Code::Unsupported, got {:?}",
                err.code
            );
            let msg = err.to_string();
            assert!(
                msg.contains("not yet implemented in Rust port"),
                "stub `{expected_path}` message should name the not-yet-implemented gap, got: {msg}"
            );
            assert!(
                !msg.contains("unknown command"),
                "stub `{expected_path}` must NOT look like an unknown-command error, got: {msg}"
            );
        }
    }

    #[test]
    fn all_real_command_paths_are_unique_and_cover_the_tree() {
        let cmds = all_real_commands();
        // The Go `schema.json` golden has 70 leaves; subtracting the cobra-native
        // `help` leaf and the four `completion <shell>` leaves (all deferred to
        // WS7) leaves exactly 65 real commands the Rust port must route.
        assert_eq!(cmds.len(), 65, "expected the 65 real Go leaf commands");
        let mut seen = std::collections::BTreeSet::new();
        for (path, _) in &cmds {
            assert!(seen.insert(*path), "duplicate command path: {path}");
        }
    }

    // --- 3. a genuinely unknown command is a usage failure -----------------

    #[test]
    fn unknown_command_is_a_clap_usage_failure() {
        let err = Cli::try_parse_from(["defi", "frobnicate"])
            .expect_err("an unknown command must fail to parse");
        // clap classifies this as a usage error (not help/version display).
        assert!(
            !matches!(
                err.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ),
            "unknown command should be a genuine usage failure, got {:?}",
            err.kind()
        );
        // The emitter maps this to exit code 2 (usage).
        assert_eq!(emit_clap_error(err), 2);
    }

    #[test]
    fn unknown_subcommand_under_known_group_is_a_usage_failure() {
        let err = Cli::try_parse_from(["defi", "lend", "frobnicate"])
            .expect_err("unknown lend subcommand must fail to parse");
        assert_eq!(emit_clap_error(err), 2);
    }

    // --- 4. parser flag surface --------------------------------------------

    #[test]
    fn structured_input_modes_parse() {
        // --input-json on a plan command.
        let cli = Cli::try_parse_from(["defi", "swap", "plan", "--input-json", r#"{"chain":"1"}"#])
            .expect("--input-json should parse");
        if let TopCommand::Swap {
            cmd: crate::swap::cli::SwapCmd::Plan(args),
        } = cli.command
        {
            assert_eq!(args.input.input_json.as_deref(), Some(r#"{"chain":"1"}"#));
            assert!(args.input.input_file.is_none());
        } else {
            panic!("expected swap plan");
        }

        // --input-file '-' (stdin) on a submit command.
        let cli = Cli::try_parse_from(["defi", "bridge", "submit", "--input-file", "-"])
            .expect("--input-file should parse");
        if let TopCommand::Bridge {
            cmd: crate::bridge::cli::BridgeCmd::Submit(args),
        } = cli.command
        {
            assert_eq!(args.input.input_file.as_deref(), Some("-"));
        } else {
            panic!("expected bridge submit");
        }
    }

    #[test]
    fn enum_like_flags_pass_through_as_strings() {
        // `--type` is validated in-handler (cobra parity: not a cobra enum), so
        // the parser accepts any string; the handler rejects unknown values.
        let cli = Cli::try_parse_from([
            "defi",
            "swap",
            "quote",
            "--type",
            "exact-output",
            "--provider",
            "uniswap",
        ])
        .expect("swap quote --type should parse");
        if let TopCommand::Swap {
            cmd: crate::swap::cli::SwapCmd::Quote(args),
        } = cli.command
        {
            assert_eq!(args.r#type, "exact-output");
            assert_eq!(args.provider.as_deref(), Some("uniswap"));
        } else {
            panic!("expected swap quote");
        }

        // `--signer` defaults to "local" and accepts overrides.
        let cli = Cli::try_parse_from(["defi", "swap", "submit", "--signer", "tempo"])
            .expect("swap submit --signer should parse");
        if let TopCommand::Swap {
            cmd: crate::swap::cli::SwapCmd::Submit(args),
        } = cli.command
        {
            assert_eq!(args.signer, "tempo");
        } else {
            panic!("expected swap submit");
        }
    }

    #[test]
    fn submit_signer_defaults_to_local() {
        let cli = Cli::try_parse_from(["defi", "lend", "supply", "submit"])
            .expect("lend supply submit should parse with defaults");
        if let TopCommand::Lend {
            cmd: crate::lend::cli::LendCmd::Supply(crate::lend::cli::LendVerbCmd::Submit(args)),
        } = cli.command
        {
            assert_eq!(args.signer, "local");
            assert_eq!(args.key_source, "auto");
            assert!(args.simulate, "simulate defaults to true");
        } else {
            panic!("expected lend supply submit");
        }
    }

    #[test]
    fn plan_identity_flags_parse() {
        // OWS-first --wallet.
        let cli = Cli::try_parse_from([
            "defi",
            "lend",
            "supply",
            "plan",
            "--wallet",
            "my-wallet",
            "--provider",
            "aave",
        ])
        .expect("lend supply plan --wallet should parse");
        if let TopCommand::Lend {
            cmd: crate::lend::cli::LendCmd::Supply(crate::lend::cli::LendVerbCmd::Plan(args)),
        } = cli.command
        {
            assert_eq!(args.identity.wallet.as_deref(), Some("my-wallet"));
            assert!(args.identity.from_address.is_none());
        } else {
            panic!("expected lend supply plan");
        }

        // Local signer --from-address.
        let cli = Cli::try_parse_from([
            "defi",
            "transfer",
            "plan",
            "--from-address",
            "0x000000000000000000000000000000000000dEaD",
        ])
        .expect("transfer plan --from-address should parse");
        if let TopCommand::Transfer {
            cmd: crate::transfer::cli::TransferCmd::Plan(args),
        } = cli.command
        {
            assert_eq!(
                args.identity.from_address.as_deref(),
                Some("0x000000000000000000000000000000000000dEaD")
            );
        } else {
            panic!("expected transfer plan");
        }
    }

    #[test]
    fn rpc_url_override_parses_on_read_and_plan() {
        let cli = Cli::try_parse_from([
            "defi",
            "lend",
            "markets",
            "--rpc-url",
            "https://rpc.example",
        ])
        .expect("lend markets --rpc-url should parse");
        if let TopCommand::Lend {
            cmd: crate::lend::cli::LendCmd::Markets(args),
        } = cli.command
        {
            assert_eq!(args.rpc_url.as_deref(), Some("https://rpc.example"));
        } else {
            panic!("expected lend markets");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn json_and_plain_together_is_a_usage_error() {
        // cobra lets both flags through; the runner (Settings::load) rejects.
        let env = defi_config::MapEnv::default();
        let code = run_with_args(["defi", "--json", "--plain", "providers", "list"], &env).await;
        assert_eq!(code, 2, "--json --plain together should be a usage error");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn version_prints_plain_text_exit_0() {
        let env = defi_config::MapEnv::default();
        let code = run_with_args(["defi", "version"], &env).await;
        assert_eq!(code, 0);
    }

    #[test]
    fn global_flags_are_accepted_before_and_after_subcommand() {
        // Global persistent flags must work in both positions (cobra parity).
        Cli::try_parse_from(["defi", "--results-only", "providers", "list"])
            .expect("global flag before subcommand");
        Cli::try_parse_from(["defi", "providers", "list", "--results-only"])
            .expect("global flag after subcommand");
        let cli = Cli::try_parse_from([
            "defi",
            "providers",
            "list",
            "--select",
            "name,requires_key",
            "--no-cache",
        ])
        .expect("--select + --no-cache parse");
        assert_eq!(cli.global.select.as_deref(), Some("name,requires_key"));
        assert!(cli.global.no_cache);
    }
}
