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
