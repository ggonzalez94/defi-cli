//! Command allowlist policy.
//!
//! Mirrors `internal/policy`. The crate owns the `--enable-commands` allowlist
//! gate: a single public function, [`check_command_allowed`], decides whether a
//! command path may run given the configured allowlist.

use defi_errors::{Code, Error};

/// Check whether `command_path` is permitted by the `--enable-commands`
/// allowlist.
///
/// Behavior mirrors Go `policy.CheckCommandAllowed`:
///
/// - An empty allowlist is a no-op: every command is allowed (`Ok(())`). The
///   gate is only active once the user opts in with at least one entry.
/// - Otherwise the command is allowed iff its normalized form equals the
///   normalized form of any allowlist entry.
/// - A non-matching command is blocked with a typed [`Error`] carrying
///   [`Code::Blocked`] (exit code 16) and the byte-stable message
///   `"command blocked by --enable-commands policy"`.
///
/// Normalization (applied to both sides before comparison) lowercases, trims
/// surrounding whitespace, and collapses any run of internal whitespace to a
/// single space, mirroring Go's
/// `strings.Join(strings.Fields(strings.ToLower(strings.TrimSpace(v))), " ")`.
///
/// The allowlist is generic over any item that borrows as `str`, so callers can
/// pass `&[&str]`, `&[String]`, or `&Vec<String>` without conversion.
pub fn check_command_allowed<S: AsRef<str>>(
    allowlist: &[S],
    command_path: &str,
) -> Result<(), Error> {
    if allowlist.is_empty() {
        return Ok(());
    }
    let norm_path = normalize(command_path);
    for allowed in allowlist {
        if normalize(allowed.as_ref()) == norm_path {
            return Ok(());
        }
    }
    Err(Error::new(
        Code::Blocked,
        "command blocked by --enable-commands policy",
    ))
}

/// Normalize a command path for comparison: lowercase, trim, and collapse
/// internal whitespace runs to single spaces.
///
/// Mirrors Go's
/// `strings.Join(strings.Fields(strings.ToLower(strings.TrimSpace(v))), " ")`.
/// `split_whitespace` reproduces `strings.Fields` (any Unicode whitespace,
/// including tabs and newlines, splits tokens and is discarded). A
/// whitespace-only input normalizes to the empty string.
fn normalize(v: &str) -> String {
    v.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    //! Success criteria for `defi-policy` (mirrors Go `internal/policy`).
    //!
    //! The crate owns the `--enable-commands` allowlist gate. Its single public
    //! behavior is `check_command_allowed(allowlist, command_path)`:
    //!
    //! 1. EMPTY ALLOWLIST IS A NO-OP. An empty (or absent) allowlist allows
    //!    every command: the gate is only active when the user opts in by
    //!    supplying at least one allowed command path. Returns `Ok(())`.
    //! 2. EXACT (NORMALIZED) MATCH ALLOWS. If the (normalized) command path
    //!    equals any (normalized) allowlist entry, the command is allowed:
    //!    `Ok(())`.
    //! 3. NO MATCH BLOCKS. Otherwise the command is blocked, returning a typed
    //!    `defi_errors::Error` whose `code` is `Code::Blocked` (exit code 16,
    //!    part of the machine contract) and whose `message` is exactly
    //!    `"command blocked by --enable-commands policy"`.
    //! 4. NORMALIZATION (applied to BOTH allowlist entries and the command
    //!    path before comparison): lowercase, trim surrounding whitespace, and
    //!    collapse any run of internal whitespace (spaces, tabs, newlines) to a
    //!    single space. This makes matching case-insensitive and
    //!    whitespace-insensitive, mirroring Go's
    //!    `strings.Join(strings.Fields(strings.ToLower(strings.TrimSpace(v))), " ")`.
    //!    A whitespace-only string normalizes to the empty string.
    //!
    //! Contract notes asserted here: the blocked error maps to the stable exit
    //! code 16 (`Code::Blocked`), and the blocked message is byte-stable.

    use crate::check_command_allowed;
    use defi_errors::Code;

    // --- Ported Go cases (internal/policy/policy_test.go: TestCheckCommandAllowed) ---

    #[test]
    fn empty_allowlist_allows_any_command() {
        // Go: CheckCommandAllowed(nil, "yield opportunities") == nil
        let allowlist: &[&str] = &[];
        assert!(check_command_allowed(allowlist, "yield opportunities").is_ok());
    }

    #[test]
    fn exact_match_is_allowed() {
        // Go: CheckCommandAllowed([]string{"yield opportunities"}, "yield opportunities") == nil
        assert!(check_command_allowed(&["yield opportunities"], "yield opportunities").is_ok());
    }

    #[test]
    fn non_matching_command_is_blocked() {
        // Go: CheckCommandAllowed([]string{"chains top"}, "yield opportunities") != nil
        let result = check_command_allowed(&["chains top"], "yield opportunities");
        assert!(result.is_err());
    }

    // --- Fresh spec-driven contract tests ---

    #[test]
    fn blocked_error_carries_stable_code_and_message() {
        // The blocked error MUST map to the stable contract exit code (16) and
        // use the exact, byte-stable message string.
        let err = check_command_allowed(&["chains top"], "yield opportunities")
            .expect_err("non-matching command must be blocked");
        assert_eq!(err.code, Code::Blocked);
        assert_eq!(err.code.as_i32(), 16);
        assert_eq!(err.message, "command blocked by --enable-commands policy");
    }

    #[test]
    fn match_is_case_insensitive() {
        // Normalization lowercases both sides before comparing.
        assert!(check_command_allowed(&["Yield Opportunities"], "yield opportunities").is_ok());
        assert!(check_command_allowed(&["yield opportunities"], "YIELD OPPORTUNITIES").is_ok());
    }

    #[test]
    fn match_collapses_and_trims_whitespace() {
        // Surrounding whitespace is trimmed and internal runs collapse to one
        // space; tabs/newlines count as whitespace.
        assert!(
            check_command_allowed(&["  yield   opportunities  "], "yield opportunities").is_ok()
        );
        assert!(
            check_command_allowed(&["yield opportunities"], "  yield   opportunities  ").is_ok()
        );
        assert!(check_command_allowed(&["yield\topportunities"], "yield opportunities").is_ok());
        assert!(check_command_allowed(&["yield\n\nopportunities"], "yield opportunities").is_ok());
    }

    #[test]
    fn multi_entry_allowlist_matches_any_entry() {
        let allowlist = &["chains top", "yield opportunities", "lend markets"];
        assert!(check_command_allowed(allowlist, "yield opportunities").is_ok());
        assert!(check_command_allowed(allowlist, "lend markets").is_ok());
        assert!(check_command_allowed(allowlist, "chains top").is_ok());
        assert!(check_command_allowed(allowlist, "swap quote").is_err());
    }

    #[test]
    fn partial_or_prefix_paths_do_not_match() {
        // Matching is on the full normalized path, not a prefix/substring.
        assert!(check_command_allowed(&["yield opportunities"], "yield").is_err());
        assert!(check_command_allowed(&["yield"], "yield opportunities").is_err());
        assert!(check_command_allowed(&["yield opportunities"], "opportunities").is_err());
    }

    #[test]
    fn whitespace_only_allowlist_entry_matches_empty_normalized_path() {
        // A whitespace-only entry normalizes to "" and so matches a command
        // path that also normalizes to "" (e.g. an empty/whitespace path).
        assert!(check_command_allowed(&["   "], "").is_ok());
        assert!(check_command_allowed(&["   "], "   ").is_ok());
        // ...but does not match a real command path.
        assert!(check_command_allowed(&["   "], "yield opportunities").is_err());
    }

    #[test]
    fn accepts_owned_string_allowlist() {
        // Config produces owned `Vec<String>`; the API must accept it without
        // forcing callers to convert to `&[&str]`.
        let allowlist: Vec<String> = vec!["yield opportunities".to_string()];
        assert!(check_command_allowed(&allowlist, "yield opportunities").is_ok());
        assert!(check_command_allowed(&allowlist, "chains top").is_err());
    }
}
