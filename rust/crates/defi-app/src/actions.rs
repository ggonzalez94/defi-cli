//! `actions` command group handler (Go: `internal/app/runner.go` —
//! `newActionsCommand` + the action-store/estimate helpers).
//!
//! This module owns the **actions-command-specific** glue around the persisted
//! execution-action store ([`defi_execution::store::Store`]) and the gas
//! estimate options ([`defi_execution::EstimateOptions`]). The `actions` group
//! is a read-only inspection surface over actions the execution commands
//! persisted (`actions list|show|estimate`); it does no provider routing and
//! bypasses the cache. Concretely it owns:
//!
//! * the action-id resolver (`resolve_action_id`): the Go `resolveActionID` —
//!   trim, reject empty (`--action-id` is required), and validate the
//!   `act_<32 hex chars>` shape;
//! * the `actions estimate` options parser (`parse_action_estimate_options`):
//!   the Go `parseActionEstimateOptions` — split the optional `--step-ids` CSV,
//!   enforce `--gas-multiplier > 1`, carry the EIP-1559 fee overrides verbatim,
//!   and normalize `--block-tag` (empty → pending; `pending`/`latest`;
//!   otherwise a usage error);
//! * the action-store routing predicate (`should_open_action_store`) and its
//!   shared command-path helpers (`normalize_command_path` /
//!   `is_execution_command_path`): which command paths open the persisted action
//!   store (the Go `shouldOpenActionStore` / `isExecutionCommandPath`);
//! * the `actions` subcommand surface (`actions_subcommand_names`): exactly
//!   `list` / `show` / `estimate`, with NO deprecated `status` alias (the Go
//!   `newActionsCommand` structure);
//! * the unknown-subcommand usage error (`unknown_actions_subcommand_error`):
//!   the Go `RunE` fallback for `defi actions <unknown>`.
//!
//! NOT re-owned here (consumed from elsewhere):
//! * the actual gas/fee estimation (`EstimateActionGas`: the EVM/Tempo step
//!   estimate, the `action has no executable steps` rejection) — owned by
//!   `defi_execution::estimate` and covered by its own RED suite;
//! * the action-store persistence (`Store::open` / `save` / `get` / `list`) —
//!   owned by `defi_execution::store`;
//! * the cache-bypass predicate for non-execution paths (`should_open_cache`) —
//!   owned by [`crate::runner`] (it shares [`is_execution_command_path`]);
//! * the success-envelope rendering of list/show/estimate results — runner /
//!   `defi-out` concern.

#![allow(dead_code, unused_variables)]

use defi_errors::{Code, Error};
use defi_execution::{default_estimate_options, EstimateBlockTag, EstimateOptions};

/// Whether `value` matches the action-id shape `^act_[0-9a-f]{32}$`
/// (case-insensitive over hex), parity with the Go `actionIDPattern` regex.
///
/// Implemented byte-wise (no regex dependency): exactly `act_` followed by 32
/// ASCII hex digits and nothing else.
fn is_action_id_shape(value: &str) -> bool {
    let rest = match value.strip_prefix("act_") {
        Some(rest) => rest,
        None => return false,
    };
    rest.len() == 32 && rest.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Resolve and validate an `--action-id` value.
///
/// Parity with Go `resolveActionID`:
/// 1. trim surrounding whitespace;
/// 2. an empty value is a [`defi_errors::Code::Usage`] error
///    (`action id is required (--action-id)`);
/// 3. a value that does not match `^act_[0-9a-f]{32}$` (case-insensitive) is a
///    [`defi_errors::Code::Usage`] error (`action id must match act_<32 hex chars>`);
/// 4. otherwise the trimmed value is returned unchanged.
pub fn resolve_action_id(action_id: &str) -> Result<String, Error> {
    let trimmed = action_id.trim();
    if trimmed.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "action id is required (--action-id)",
        ));
    }
    if !is_action_id_shape(trimmed) {
        return Err(Error::new(
            Code::Usage,
            "action id must match act_<32 hex chars>",
        ));
    }
    Ok(trimmed.to_string())
}

/// Parse the `actions estimate` options from the raw flags.
///
/// Parity with Go `parseActionEstimateOptions`, starting from
/// [`defi_execution::default_estimate_options`]:
/// 1. `--step-ids` is split as a CSV (lowercased, trimmed, non-empty parts);
/// 2. `--gas-multiplier` MUST be `> 1` — `<= 1` is a [`defi_errors::Code::Usage`]
///    error (`--gas-multiplier must be > 1`);
/// 3. `--max-fee-gwei` / `--max-priority-fee-gwei` are carried verbatim
///    (trimmed);
/// 4. `--block-tag` is normalized via [`defi_execution::EstimateBlockTag::from_str`]
///    (empty → pending; `pending`/`latest` case-insensitive; otherwise a usage
///    error: `--block-tag must be one of: pending,latest`).
pub fn parse_action_estimate_options(
    step_ids_csv: &str,
    gas_multiplier: f64,
    max_fee_gwei: &str,
    max_priority_fee_gwei: &str,
    block_tag: &str,
) -> Result<EstimateOptions, Error> {
    let mut opts = default_estimate_options();
    opts.step_ids = split_csv(step_ids_csv);
    if gas_multiplier <= 1.0 {
        return Err(Error::new(Code::Usage, "--gas-multiplier must be > 1"));
    }
    opts.gas_multiplier = gas_multiplier;
    opts.max_fee_gwei = max_fee_gwei.trim().to_string();
    opts.max_priority_fee_gwei = max_priority_fee_gwei.trim().to_string();
    opts.block_tag = EstimateBlockTag::from_str(block_tag)?;
    Ok(opts)
}

/// Split a comma-separated value into lowercased, trimmed, non-empty parts.
///
/// Parity with Go `splitCSV`: a blank input (after trimming) yields an empty
/// list; otherwise split on commas, lowercase + trim each part, and drop empty
/// segments.
fn split_csv(value: &str) -> Vec<String> {
    if value.trim().is_empty() {
        return Vec::new();
    }
    value
        .split(',')
        .map(|part| part.trim().to_lowercase())
        .filter(|part| !part.is_empty())
        .collect()
}

/// Normalize a cobra-style command path for routing comparisons.
///
/// Parity with Go `normalizeCommandPath`: trim, lowercase, and collapse runs of
/// whitespace into single spaces (`"  Actions   List "` → `"actions list"`).
pub fn normalize_command_path(command_path: &str) -> String {
    command_path
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a (already-normalized) command path is an execution command path.
///
/// Parity with Go `isExecutionCommandPath`:
/// * the bare `actions`, `actions list`, `actions show`, `actions estimate`
///   paths are execution paths;
/// * a `swap`/`bridge`/`approvals`/`transfer`/`lend`/`rewards`/`yield` path
///   whose LAST segment is `plan`/`submit`/`status` is an execution path
///   (e.g. `lend supply status`, `yield deposit plan`);
/// * everything else (incl. `swap quote`, `lend markets`, single-segment paths)
///   is NOT.
pub fn is_execution_command_path(path: &str) -> bool {
    match path {
        "actions" | "actions list" | "actions show" | "actions estimate" => return true,
        _ => {}
    }
    let parts: Vec<&str> = path.split_whitespace().collect();
    if parts.len() < 2 {
        return false;
    }
    match parts[0] {
        "swap" | "bridge" | "approvals" | "transfer" | "lend" | "rewards" | "yield" => {
            let last = parts[parts.len() - 1];
            last == "plan" || last == "submit" || last == "status"
        }
        _ => false,
    }
}

/// Whether the persisted action store should be opened for a command path.
///
/// Parity with Go `shouldOpenActionStore`: exactly the execution command paths
/// (normalize then [`is_execution_command_path`]).
pub fn should_open_action_store(command_path: &str) -> bool {
    is_execution_command_path(&normalize_command_path(command_path))
}

/// The `actions` subcommand names, in declaration order.
///
/// Parity with the Go `newActionsCommand` structure: exactly `list`, `show`,
/// `estimate` — and crucially NO deprecated `status` alias.
pub fn actions_subcommand_names() -> Vec<&'static str> {
    vec!["list", "show", "estimate"]
}

/// The usage error for an unknown `actions` subcommand.
///
/// Parity with the Go `newActionsCommand` `RunE` fallback: a
/// [`defi_errors::Code::Usage`] error whose message is
/// `unknown actions subcommand "<arg>"`.
pub fn unknown_actions_subcommand_error(arg: &str) -> Error {
    Error::new(
        Code::Usage,
        format!("unknown actions subcommand {}", go_quote(arg)),
    )
}

/// Quote a string the way Go's `fmt`/`%q` does for a typical CLI argument.
///
/// Parity with Go `fmt.Sprintf("%q", arg)`: wrap in double quotes and escape
/// the backslash, double-quote, and the common ASCII control characters Go
/// renders with short escapes. This keeps the error message byte-stable with
/// the Go runner for the arguments the CLI realistically sees.
fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// clap parsing + handler for the `actions` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::{Code, Error};
    use defi_model::{Envelope, ProviderStatus};

    use super::{parse_action_estimate_options, resolve_action_id};
    use crate::ctx::AppCtx;

    /// `actions` subcommands (Go `newActionsCommand`).
    #[derive(Subcommand, Debug)]
    pub enum ActionsCmd {
        /// List persisted actions.
        List(ListArgs),
        /// Show action details by action id.
        Show(ShowArgs),
        /// Estimate gas and EIP-1559 fees for a planned action.
        Estimate(EstimateArgs),
    }

    impl ActionsCmd {
        /// The leaf path token (for `meta.command`).
        pub fn path(&self) -> &'static str {
            match self {
                ActionsCmd::List(_) => "list",
                ActionsCmd::Show(_) => "show",
                ActionsCmd::Estimate(_) => "estimate",
            }
        }
    }

    /// `actions list` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct ListArgs {
        /// Optional action status filter.
        #[arg(long)]
        pub status: Option<String>,
        /// Maximum actions to return.
        #[arg(long, default_value_t = 20)]
        pub limit: i64,
    }

    /// `actions show` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct ShowArgs {
        /// Action identifier.
        #[arg(long = "action-id")]
        pub action_id: Option<String>,
    }

    /// `actions estimate` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct EstimateArgs {
        /// Action identifier.
        #[arg(long = "action-id")]
        pub action_id: Option<String>,
        /// Optional comma-separated step_id filter.
        #[arg(long = "step-ids")]
        pub step_ids: Option<String>,
        /// Block tag used for estimation (pending|latest).
        #[arg(long = "block-tag", default_value = "pending")]
        pub block_tag: String,
        /// Gas estimate safety multiplier.
        #[arg(long = "gas-multiplier", default_value_t = 1.2)]
        pub gas_multiplier: f64,
        /// Optional EIP-1559 max fee (gwei).
        #[arg(long = "max-fee-gwei")]
        pub max_fee_gwei: Option<String>,
        /// Optional EIP-1559 max priority fee (gwei).
        #[arg(long = "max-priority-fee-gwei")]
        pub max_priority_fee_gwei: Option<String>,
    }

    /// Handle `actions <sub>`.
    ///
    /// The `actions` group is a read-only inspection surface over the persisted
    /// execution-action [`Store`]; it does no provider routing and bypasses the
    /// cache (spec §2.5, execution command paths). Each handler builds the
    /// success [`Envelope`] directly via [`AppCtx::metadata_envelope`] with
    /// `cache.status == "bypass"` and no provider statuses.
    ///
    /// [`Store`]: defi_execution::store::Store
    pub async fn handle(ctx: &AppCtx, cmd: ActionsCmd) -> Result<Envelope, Error> {
        match cmd {
            ActionsCmd::List(args) => handle_list(ctx, args).await,
            ActionsCmd::Show(args) => handle_show(ctx, args).await,
            ActionsCmd::Estimate(args) => handle_estimate(ctx, args).await,
        }
    }

    /// Handle `actions list` (Go `listCmd.RunE` in `newActionsCommand`).
    ///
    /// Flow parity with the Go runner: open the action store, list the persisted
    /// actions (`--status` filter trimmed, `--limit` cap), and emit the resulting
    /// array as the envelope `data` (empty → `[]`). A list error is wrapped as a
    /// [`Code::Internal`] `list actions` error.
    async fn handle_list(ctx: &AppCtx, args: ListArgs) -> Result<Envelope, Error> {
        let store = ctx.open_action_store()?;
        let items = store
            .list(
                args.status.as_deref().unwrap_or_default().trim(),
                args.limit,
            )
            .map_err(|e| Error::wrap(Code::Internal, "list actions", e))?;
        let data = serde_json::to_value(&items)
            .map_err(|e| Error::wrap(Code::Internal, "serialize actions", e))?;
        Ok(ctx.metadata_envelope("actions list", data, Vec::<ProviderStatus>::new()))
    }

    /// Handle `actions show` (Go `showCmd.RunE` → `lookupAction`).
    ///
    /// Flow parity with the Go runner: resolve + validate the `--action-id`
    /// (required, `act_<32 hex chars>`), open the store, load the action, and emit
    /// it as the envelope `data`. A load failure (not found / decode) is wrapped as
    /// a [`Code::Usage`] `load action` error (matching Go `lookupAction`).
    async fn handle_show(ctx: &AppCtx, args: ShowArgs) -> Result<Envelope, Error> {
        let action_id = resolve_action_id(args.action_id.as_deref().unwrap_or_default())?;
        let store = ctx.open_action_store()?;
        let action = store
            .get(&action_id)
            .map_err(|e| Error::wrap(Code::Usage, "load action", e))?;
        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize action", e))?;
        Ok(ctx.metadata_envelope("actions show", data, Vec::<ProviderStatus>::new()))
    }

    /// Handle `actions estimate` (Go `estimateCmd.RunE` in `newActionsCommand`).
    ///
    /// Flow parity with the Go runner:
    /// 1. resolve + validate the `--action-id` (required, `act_<32 hex chars>`);
    /// 2. open the store and load the action (load failure → [`Code::Usage`]
    ///    `load action`);
    /// 3. parse the estimate options ([`parse_action_estimate_options`]:
    ///    `--step-ids` CSV, `--gas-multiplier > 1`, EIP-1559 fee overrides,
    ///    `--block-tag` normalization);
    /// 4. run the gas/fee estimate ([`estimate_action_gas`]) — EIP-1559 native gas
    ///    for EVM actions, fee-token (`fee_unit`/`fee_token`) for Tempo actions;
    ///    a no-steps action surfaces the `action has no executable steps` error;
    /// 5. emit the estimate as the envelope `data`.
    ///
    /// [`estimate_action_gas`]: defi_execution::estimate::estimate_action_gas
    async fn handle_estimate(ctx: &AppCtx, args: EstimateArgs) -> Result<Envelope, Error> {
        let action_id = resolve_action_id(args.action_id.as_deref().unwrap_or_default())?;
        let store = ctx.open_action_store()?;
        let action = store
            .get(&action_id)
            .map_err(|e| Error::wrap(Code::Usage, "load action", e))?;
        let opts = parse_action_estimate_options(
            args.step_ids.as_deref().unwrap_or_default(),
            args.gas_multiplier,
            args.max_fee_gwei.as_deref().unwrap_or_default(),
            args.max_priority_fee_gwei.as_deref().unwrap_or_default(),
            &args.block_tag,
        )?;
        let estimate = defi_execution::estimate::estimate_action_gas(&action, opts).await?;
        let data = serde_json::to_value(&estimate)
            .map_err(|e| Error::wrap(Code::Internal, "serialize estimate", e))?;
        Ok(ctx.metadata_envelope("actions estimate", data, Vec::<ProviderStatus>::new()))
    }
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::actions` (Go: `internal/app` actions
    //! command group: `newActionsCommand` + `resolveActionID` /
    //! `parseActionEstimateOptions` / `shouldOpenActionStore` /
    //! `isExecutionCommandPath` / `normalizeCommandPath` in `runner.go`)
    //!
    //! This module owns the **actions-command glue**: action-id validation, the
    //! `estimate` options parser, the action-store routing predicate, and the
    //! `actions` subcommand surface. "Correct" means it preserves the
    //! runner-owned actions behaviors AND the stable machine contract (design
    //! spec §2.2 exit codes — usage failures map to exit 2). The actual gas
    //! estimation, the action-store persistence, the cache-bypass predicate for
    //! non-execution paths, and the success-envelope rendering are owned
    //! elsewhere and are NOT re-asserted here. Criteria:
    //!
    //! 1. **Action-id resolution.** `resolve_action_id` accepts a well-formed
    //!    `act_<32 hex chars>` id (returned trimmed, unchanged), and rejects an
    //!    empty id and a malformed id (`act_invalid`) with [`Code::Usage`]
    //!    (exit 2). The match is case-insensitive over hex and trims surrounding
    //!    whitespace. (Ported from `TestResolveActionID`.)
    //!
    //! 2. **`actions estimate` options parsing.** `parse_action_estimate_options`
    //!    rejects `--gas-multiplier <= 1` ([`Code::Usage`], exit 2) and an
    //!    unknown `--block-tag` (e.g. `safe`) ([`Code::Usage`], exit 2). A valid
    //!    call (`gas_multiplier = 1.2`, blank tag) succeeds with the multiplier
    //!    carried, `block_tag = pending`, and the `--step-ids` CSV split into
    //!    parts; `latest` and `pending` (any case) normalize correctly. (Ported
    //!    from `TestParseActionEstimateOptionsRejectsGasMultiplierLTEOne`,
    //!    `TestParseActionEstimateOptionsRejectsUnknownBlockTag`, plus
    //!    spec-driven valid-path coverage.)
    //!
    //! 3. **Action-store routing.** `should_open_action_store` returns `true` for
    //!    every execution command path (`swap plan`, `bridge plan`,
    //!    `approvals submit`, `transfer plan`, `lend supply status`,
    //!    `yield deposit plan`, `rewards claim plan`, and the `actions
    //!    list|show|estimate` paths) and `false` for non-execution paths
    //!    (`swap quote`, `lend markets`). The predicate normalizes the path
    //!    first (case / whitespace insensitive). (Ported from
    //!    `TestShouldOpenActionStore`, plus the `isExecutionCommandPath` cases
    //!    asserted via the public predicate.)
    //!
    //! 4. **`actions` subcommand surface.** `actions_subcommand_names` is exactly
    //!    `[list, show, estimate]` with NO deprecated `status` alias. (Ported
    //!    from `TestActionsCommandHasNoStatusAlias` — re-expressed as a pure
    //!    structural assertion instead of constructing a cobra tree.)
    //!
    //! 5. **Unknown-subcommand usage error.** `unknown_actions_subcommand_error`
    //!    yields a [`Code::Usage`] error (exit 2) whose message contains
    //!    `unknown actions subcommand` and the quoted argument. (Ported from
    //!    `TestRunnerActionsStatusRejected`, which drives `actions status` and
    //!    asserts the error-envelope message.)
    //!
    //! SKIPPED (Go internal-detail / wrong-module):
    //!   * the cobra command construction itself (flag wiring, `--limit 20`
    //!     default, `--gas-multiplier 1.2` default, `--block-tag pending`
    //!     default) — harness concern, asserted by the integration golden-CLI /
    //!     schema suites, not this unit;
    //!   * `parseExecuteOptions` (`TestParseExecuteOptions*`) — owned by the
    //!     submit/execute path, not the read-only `actions` group;
    //!   * `shouldOpenCache` (`TestShouldOpenCacheBypassesExecutionCommands`) —
    //!     owned by [`crate::runner`] (it consumes the shared
    //!     [`is_execution_command_path`] this module exports);
    //!   * the actual gas estimation + `action has no executable steps`
    //!     rejection (`TestRunnerActionsEstimateTempoActionsNoSteps`) — owned by
    //!     `defi_execution::estimate`;
    //!   * the action-store open/save/get/list + the
    //!     `actions list` cache-bypass success-envelope render
    //!     (`TestRunnerActionsListBypassesCacheOpen`,
    //!     `TestRunnerExecutionStatusBypassesCacheOpen`) — action-store /
    //!     runner / `defi-out` concern, asserted by the integration suite;
    //!   * the swap/transfer-intent persisted-action gates
    //!     (`TestRunnerSwapStatusRejectsNonSwapIntent`) — owned by each
    //!     command group's intent-gate (e.g. `transfer::ensure_transfer_intent`).

    use super::*;
    use defi_errors::{exit_code, Code};
    use defi_execution::EstimateBlockTag;

    // --- helpers -----------------------------------------------------------

    /// Derive the process exit code a typed error would produce (spec §2.2).
    fn err_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, String::new())))
    }

    const VALID_ID: &str = "act_0123456789abcdef0123456789abcdef";

    // --- 1. action-id resolution -------------------------------------------

    #[test]
    fn resolve_action_id_accepts_well_formed_id() {
        let id = resolve_action_id(VALID_ID).expect("well-formed action id accepted");
        assert_eq!(id, VALID_ID);
    }

    #[test]
    fn resolve_action_id_trims_surrounding_whitespace() {
        let id = resolve_action_id("  act_0123456789abcdef0123456789abcdef  ")
            .expect("whitespace-padded id accepted");
        assert_eq!(id, VALID_ID, "id returned trimmed");
    }

    #[test]
    fn resolve_action_id_is_case_insensitive_over_hex() {
        let upper = "act_0123456789ABCDEF0123456789ABCDEF";
        let id = resolve_action_id(upper).expect("uppercase hex accepted");
        assert_eq!(id, upper, "value returned unchanged (case preserved)");
    }

    #[test]
    fn resolve_action_id_rejects_empty() {
        let err = resolve_action_id("").expect_err("empty action id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(err_exit(&err), 2);
    }

    #[test]
    fn resolve_action_id_rejects_whitespace_only() {
        let err = resolve_action_id("   ").expect_err("blank action id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(err_exit(&err), 2);
    }

    #[test]
    fn resolve_action_id_rejects_malformed() {
        // Too short / wrong shape — Go `TestResolveActionID` uses `act_invalid`.
        let err = resolve_action_id("act_invalid").expect_err("malformed action id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(err_exit(&err), 2);
    }

    #[test]
    fn resolve_action_id_rejects_missing_prefix() {
        let err = resolve_action_id("0123456789abcdef0123456789abcdef")
            .expect_err("missing act_ prefix rejected");
        assert_eq!(err.code, Code::Usage);
    }

    // --- 2. actions estimate options parsing -------------------------------

    #[test]
    fn parse_estimate_options_rejects_gas_multiplier_lte_one() {
        // Go `TestParseActionEstimateOptionsRejectsGasMultiplierLTEOne`.
        let err = parse_action_estimate_options("", 1.0, "", "", "pending")
            .expect_err("gas multiplier == 1 rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(err_exit(&err), 2);
    }

    #[test]
    fn parse_estimate_options_rejects_unknown_block_tag() {
        // Go `TestParseActionEstimateOptionsRejectsUnknownBlockTag` (tag `safe`).
        let err = parse_action_estimate_options("", 1.2, "", "", "safe")
            .expect_err("unknown block tag rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(err_exit(&err), 2);
    }

    #[test]
    fn parse_estimate_options_accepts_valid_defaults() {
        let opts = parse_action_estimate_options("", 1.2, "", "", "")
            .expect("valid estimate options parsed");
        assert_eq!(opts.gas_multiplier, 1.2);
        // Empty block tag defaults to pending (spec parity with Go).
        assert_eq!(opts.block_tag, EstimateBlockTag::Pending);
        assert!(opts.step_ids.is_empty());
        assert_eq!(opts.max_fee_gwei, "");
        assert_eq!(opts.max_priority_fee_gwei, "");
    }

    #[test]
    fn parse_estimate_options_normalizes_latest_block_tag() {
        let opts = parse_action_estimate_options("", 1.5, "", "", "LATEST")
            .expect("latest block tag accepted (case-insensitive)");
        assert_eq!(opts.block_tag, EstimateBlockTag::Latest);
    }

    #[test]
    fn parse_estimate_options_splits_step_ids_csv() {
        let opts = parse_action_estimate_options(" Step-1 , step-2 ,", 1.2, "", "", "pending")
            .expect("step ids parsed");
        // CSV split lowercases + trims + drops empty segments (Go splitCSV).
        assert_eq!(
            opts.step_ids,
            vec!["step-1".to_string(), "step-2".to_string()]
        );
    }

    #[test]
    fn parse_estimate_options_carries_fee_overrides_trimmed() {
        let opts = parse_action_estimate_options("", 2.0, " 30 ", " 2 ", "pending")
            .expect("fee overrides carried");
        assert_eq!(opts.max_fee_gwei, "30");
        assert_eq!(opts.max_priority_fee_gwei, "2");
        assert_eq!(opts.gas_multiplier, 2.0);
    }

    // --- 3. action-store routing -------------------------------------------

    #[test]
    fn should_open_action_store_for_execution_paths() {
        // Ported verbatim from Go `TestShouldOpenActionStore`.
        for path in [
            "swap plan",
            "bridge plan",
            "approvals submit",
            "transfer plan",
            "lend supply status",
            "yield deposit plan",
            "rewards claim plan",
            "actions list",
            "actions show",
            "actions estimate",
        ] {
            assert!(
                should_open_action_store(path),
                "expected {path:?} to open the action store"
            );
        }
    }

    #[test]
    fn should_not_open_action_store_for_read_paths() {
        for path in ["swap quote", "lend markets"] {
            assert!(
                !should_open_action_store(path),
                "did not expect {path:?} to open the action store"
            );
        }
    }

    #[test]
    fn should_open_action_store_normalizes_case_and_whitespace() {
        assert!(should_open_action_store("  SWAP   Plan "));
        assert!(should_open_action_store("Actions List"));
        assert!(!should_open_action_store("  Lend   Markets "));
    }

    #[test]
    fn is_execution_command_path_covers_bare_actions_and_last_segment_verbs() {
        // Bare `actions` and its read subcommands are execution paths.
        assert!(is_execution_command_path("actions"));
        assert!(is_execution_command_path("actions list"));
        assert!(is_execution_command_path("actions show"));
        assert!(is_execution_command_path("actions estimate"));
        // Last-segment plan/submit/status across the execution command groups.
        assert!(is_execution_command_path("lend repay submit"));
        assert!(is_execution_command_path("rewards compound status"));
        assert!(is_execution_command_path("yield withdraw plan"));
        // Read paths and single-segment paths are not execution paths.
        assert!(!is_execution_command_path("swap quote"));
        assert!(!is_execution_command_path("lend markets"));
        assert!(!is_execution_command_path("providers"));
        assert!(!is_execution_command_path("version"));
        assert!(!is_execution_command_path(""));
    }

    // --- 4. actions subcommand surface -------------------------------------

    #[test]
    fn actions_subcommands_are_list_show_estimate_only() {
        // Go `TestActionsCommandHasNoStatusAlias`.
        let names = actions_subcommand_names();
        assert!(names.contains(&"list"), "expected `list` subcommand");
        assert!(names.contains(&"show"), "expected `show` subcommand");
        assert!(
            names.contains(&"estimate"),
            "expected `estimate` subcommand"
        );
        assert!(
            !names.contains(&"status"),
            "did not expect deprecated `status` alias"
        );
        assert_eq!(
            names,
            vec!["list", "show", "estimate"],
            "subcommands in declaration order, no extras"
        );
    }

    // --- 5. unknown-subcommand usage error ---------------------------------

    #[test]
    fn unknown_subcommand_is_usage_error_with_message() {
        // Go `TestRunnerActionsStatusRejected` drives `actions status`.
        let err = unknown_actions_subcommand_error("status");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(err_exit(&err), 2);
        let msg = err.to_string();
        assert!(msg.contains("unknown actions subcommand"), "got: {msg}");
        assert!(msg.contains("status"), "message quotes the arg: {msg}");
    }
}

#[cfg(test)]
mod handler_tests {
    //! # Success criteria — `defi-app::actions::cli::handle` (Go: `internal/app`
    //! `newActionsCommand` `RunE` closures: `list` / `show` (`lookupAction`) /
    //! `estimate`)
    //!
    //! These exercise the WIRED `actions list|show|estimate` handlers end-to-end
    //! over a real persisted [`defi_execution::store::Store`] (the action-id
    //! resolver / estimate-options parser / store routing are unit-asserted in the
    //! parent module). "Correct" means each handler preserves the runner-owned
    //! actions flow AND the stable machine contract (design spec §2.1 envelope,
    //! §2.2 exit codes, §2.5 execution paths bypass the cache). Criteria:
    //!
    //! 1. **`actions list` over the store.** With a persisted action present,
    //!    `actions list` emits a success envelope whose `data` is an ARRAY
    //!    containing the action; with an EMPTY store it emits `[]` (Go
    //!    `TestRunnerActionsListBypassesCacheOpen`). The cache is bypassed
    //!    (`cache.status == "bypass"`).
    //!
    //! 2. **`actions show` over the store.** `actions show --action-id <id>` loads
    //!    the persisted action and emits it as a single OBJECT `data`. A missing
    //!    `--action-id` is a [`Code::Usage`] error (exit 2); a well-formed but
    //!    absent id surfaces a [`Code::Usage`] `load action` error (Go
    //!    `lookupAction`).
    //!
    //! 3. **`actions estimate` over the store.** A zero-step action surfaces the
    //!    `action has no executable steps` error (Go
    //!    `TestRunnerActionsEstimateTempoActionsNoSteps`); `--gas-multiplier <= 1`
    //!    is rejected before any RPC.
    //!
    //! SKIPPED (owned elsewhere): the actual gas/fee estimation numbers + the
    //! EVM/Tempo fee_unit/fee_token shape (owned by `defi_execution::estimate`),
    //! the action-store persistence (owned by `defi_execution::store`), and the
    //! cache-bypass routing predicate (owned by `crate::runner`).

    use super::cli::{handle, ActionsCmd, EstimateArgs, ListArgs, ShowArgs};
    use crate::ctx::AppCtx;
    use defi_config::Settings;
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::action::{Action, Constraints};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;

    const VALID_ID: &str = "act_0123456789abcdef0123456789abcdef";

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, String::new())))
    }

    /// Execution settings with a real action store under `dir`, cache disabled
    /// (execution paths bypass the cache, spec §2.5).
    fn exec_settings(dir: &Path) -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: false,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_millis(750),
            retries: 0,
            max_stale: Duration::from_secs(0),
            no_stale: false,
            cache_enabled: false,
            cache_path: dir.join("cache.db"),
            cache_lock_path: dir.join("cache.lock"),
            action_store_path: dir.join("actions.db"),
            action_lock_path: dir.join("actions.lock"),
            defillama_api_key: String::new(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    /// Persist a zero-step `swap` action with the canonical fixed id (mirrors the
    /// Go `TestRunnerActionsEstimateTempoActionsNoSteps` fixture).
    fn save_fixture_action(settings: &Settings) -> Action {
        let store = ActionStore::open(&settings.action_store_path, &settings.action_lock_path)
            .expect("open action store");
        let action = Action::new(
            VALID_ID,
            "swap",
            "eip155:4217",
            Constraints {
                simulate: true,
                ..Constraints::default()
            },
        );
        store.save(&action).expect("save fixture action");
        action
    }

    fn data(env: &Envelope) -> Value {
        env.data.clone().expect("success envelope carries `data`")
    }

    // --- 1. actions list ---------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn list_empty_store_emits_empty_array() {
        let tmp = TempDir::new().expect("tempdir");
        let ctx = AppCtx::new(exec_settings(tmp.path()));
        let env = handle(&ctx, ActionsCmd::List(ListArgs::default()))
            .await
            .expect("actions list should succeed on an empty store");
        assert!(env.success);
        let d = data(&env);
        assert!(d.is_array(), "data should be an array, got {d}");
        assert_eq!(d.as_array().expect("array").len(), 0);
        assert_eq!(env.meta.cache.status, "bypass");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_returns_persisted_action() {
        let tmp = TempDir::new().expect("tempdir");
        let settings = exec_settings(tmp.path());
        save_fixture_action(&settings);
        let ctx = AppCtx::new(settings);
        let env = handle(&ctx, ActionsCmd::List(ListArgs::default()))
            .await
            .expect("actions list should succeed");
        let d = data(&env);
        let arr = d.as_array().expect("array");
        assert_eq!(arr.len(), 1, "one persisted action listed");
        assert_eq!(arr[0]["action_id"], Value::from(VALID_ID));
        assert_eq!(arr[0]["intent_type"], Value::from("swap"));
    }

    // --- 2. actions show ---------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn show_returns_persisted_action_object() {
        let tmp = TempDir::new().expect("tempdir");
        let settings = exec_settings(tmp.path());
        save_fixture_action(&settings);
        let ctx = AppCtx::new(settings);
        let env = handle(
            &ctx,
            ActionsCmd::Show(ShowArgs {
                action_id: Some(VALID_ID.to_string()),
            }),
        )
        .await
        .expect("actions show should succeed");
        let d = data(&env);
        assert!(d.is_object(), "data should be a single object, got {d}");
        assert_eq!(d["action_id"], Value::from(VALID_ID));
        assert_eq!(d["intent_type"], Value::from("swap"));
        assert_eq!(env.meta.cache.status, "bypass");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn show_missing_action_id_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        let ctx = AppCtx::new(exec_settings(tmp.path()));
        let err = handle(&ctx, ActionsCmd::Show(ShowArgs { action_id: None }))
            .await
            .expect_err("missing --action-id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn show_absent_action_is_usage_load_error() {
        let tmp = TempDir::new().expect("tempdir");
        let ctx = AppCtx::new(exec_settings(tmp.path()));
        let err = handle(
            &ctx,
            ActionsCmd::Show(ShowArgs {
                action_id: Some(VALID_ID.to_string()),
            }),
        )
        .await
        .expect_err("absent action should fail to load");
        // Go `lookupAction` wraps the store not-found as a Usage `load action`.
        assert_eq!(err.code, Code::Usage);
        assert!(err.to_string().contains("load action"), "got: {err}");
    }

    // --- 3. actions estimate -----------------------------------------------

    fn estimate_args(action_id: &str) -> EstimateArgs {
        EstimateArgs {
            action_id: Some(action_id.to_string()),
            step_ids: None,
            block_tag: "pending".to_string(),
            gas_multiplier: 1.2,
            max_fee_gwei: None,
            max_priority_fee_gwei: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn estimate_zero_step_action_has_no_executable_steps() {
        // Go `TestRunnerActionsEstimateTempoActionsNoSteps`.
        let tmp = TempDir::new().expect("tempdir");
        let settings = exec_settings(tmp.path());
        save_fixture_action(&settings);
        let ctx = AppCtx::new(settings);
        let err = handle(&ctx, ActionsCmd::Estimate(estimate_args(VALID_ID)))
            .await
            .expect_err("zero-step action should fail to estimate");
        assert!(
            err.to_string().contains("no executable steps"),
            "expected no-steps error, got: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn estimate_rejects_gas_multiplier_lte_one() {
        let tmp = TempDir::new().expect("tempdir");
        let settings = exec_settings(tmp.path());
        save_fixture_action(&settings);
        let ctx = AppCtx::new(settings);
        let mut args = estimate_args(VALID_ID);
        args.gas_multiplier = 1.0;
        let err = handle(&ctx, ActionsCmd::Estimate(args))
            .await
            .expect_err("gas multiplier == 1 rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn estimate_missing_action_id_is_usage_error() {
        let tmp = TempDir::new().expect("tempdir");
        let ctx = AppCtx::new(exec_settings(tmp.path()));
        let mut args = estimate_args(VALID_ID);
        args.action_id = None;
        let err = handle(&ctx, ActionsCmd::Estimate(args))
            .await
            .expect_err("missing --action-id rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }
}
