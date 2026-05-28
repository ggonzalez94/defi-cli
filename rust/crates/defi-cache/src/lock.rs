//! Cross-process file lock + path normalization.
//!
//! Mirrors `internal/fsutil/path.go` (path hardening for cache/lock paths) and
//! the `gofrs/flock` usage in `internal/cache/cache.go` (cross-process lock).
//!
//! Public interface:
//!   - [`contains_control_chars`]
//!   - [`normalize_path`]
//!
//! The file-lock mechanism itself (fd-lock) is an implementation detail of
//! [`crate::store::Store`]; its observable behavior is asserted by the
//! concurrent-open test in `store.rs` rather than re-tested here.

use std::path::{Component, Path, PathBuf};

use defi_errors::{Code, Error};

/// True if `value` contains any C0 control character (`< 0x20`).
///
/// Mirrors Go `fsutil.ContainsControlChars`: iterates over Unicode scalar
/// values (chars), not bytes, so multi-byte UTF-8 sequences whose individual
/// bytes are `>= 0x20` are never flagged.
pub fn contains_control_chars(value: &str) -> bool {
    value.chars().any(|c| (c as u32) < 0x20)
}

/// Normalize a user-supplied path: trim, reject control chars, expand a leading
/// `~`/`~/`, then clean + absolutize. Empty/whitespace input → empty path.
///
/// Mirrors Go `fsutil.NormalizePath`. Returns the canonical, contract-relevant
/// path used for cache + lock file locations.
///
/// A bare `~foo` (no following slash) is NOT expanded — it is treated as a
/// literal relative segment, matching Go's `value == "~"` / `HasPrefix("~/")`
/// checks.
pub fn normalize_path(input: &str) -> Result<PathBuf, Error> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(PathBuf::new());
    }
    if contains_control_chars(trimmed) {
        // Bad user-supplied path → usage error (spec exit-code 2).
        return Err(Error::new(Code::Usage, "path contains control characters"));
    }

    let expanded: PathBuf = if trimmed == "~" {
        home_dir()?
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home_dir()?.join(rest)
    } else {
        PathBuf::from(trimmed)
    };

    let cleaned = lexical_clean(&expanded);
    absolutize(&cleaned)
}

/// Resolve the current user's home directory.
fn home_dir() -> Result<PathBuf, Error> {
    #[allow(deprecated)]
    std::env::home_dir().ok_or_else(|| Error::new(Code::Internal, "resolve home directory"))
}

/// Lexically clean a path the way Go's `filepath.Clean` does: collapse `.`
/// segments, resolve `..` against the preceding non-`..` segment, and remove
/// redundant separators. Purely lexical — never touches the filesystem.
fn lexical_clean(path: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();
    let mut is_absolute = false;
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => {
                if matches!(comp, Component::RootDir) {
                    is_absolute = true;
                }
                out.push(comp);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                match out.last() {
                    // Pop a normal segment.
                    Some(Component::Normal(_)) => {
                        out.pop();
                    }
                    // Cannot ascend past root: drop the `..` after a root.
                    Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                    // Leading/relative `..` segments are kept.
                    _ => out.push(comp),
                }
            }
            Component::Normal(_) => out.push(comp),
        }
    }

    let mut result = PathBuf::new();
    for comp in &out {
        result.push(comp.as_os_str());
    }
    if result.as_os_str().is_empty() {
        // Go's Clean returns "." for an empty result; a relative empty path is
        // resolved against cwd during absolutize, so "." is the right seed.
        result.push(if is_absolute { "/" } else { "." });
    }
    result
}

/// Make a (lexically cleaned) path absolute, mirroring Go's `filepath.Abs`:
/// join a relative path against the current working directory, then clean
/// again. An already-absolute path is returned unchanged.
fn absolutize(path: &Path) -> Result<PathBuf, Error> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir()
        .map_err(|e| Error::wrap(Code::Internal, "resolve absolute path", e))?;
    Ok(lexical_clean(&cwd.join(path)))
}

// =============================================================================
// SUCCESS CRITERIA (RED phase — tests written before implementation)
//
// This module (Go source: internal/fsutil/path.go) owns path hardening for the
// cache + lock file locations. The Rust port is "correct" iff:
//
//   1. CONTROL-CHAR DETECTION. contains_control_chars is true for any rune
//      < 0x20 (newline, tab, NUL, etc.), false for ordinary printable text and
//      for high/Unicode runes >= 0x20. (Mirrors Go ContainsControlChars.)
//
//   2. EMPTY / WHITESPACE INPUT → EMPTY PATH. normalize_path("") and
//      normalize_path("   ") return an empty path with no error (Go returns
//      "", nil after TrimSpace). The cache layer treats this as "use default".
//
//   3. CONTROL CHARS REJECTED. normalize_path of a string containing a control
//      char (e.g. "/tmp/a\nb") is an error, not a path. (Go returns
//      `path contains control characters`.)
//
//   4. TILDE EXPANSION. normalize_path("~") → the user's home dir;
//      normalize_path("~/sub/dir") → home joined with "sub/dir". Both are
//      absolute. (Go expands `~` and `~/` via UserHomeDir; a bare `~foo` with
//      no slash is NOT expanded — it is treated as a literal relative segment.)
//
//   5. ABSOLUTIZATION + CLEANING. A relative input is made absolute and lexically
//      cleaned (`a/./b/../c` → `<cwd>/a/c`); an already-absolute input is
//      returned cleaned and absolute. (Go: filepath.Clean then filepath.Abs.)
//
// SKIPPED Go internals:
//   - acquireFileLock / TryLockContext timeout+retry loop: a mechanism detail.
//     The OBSERVABLE guarantee (no "database is locked" under contention) is
//     covered by store.rs::concurrent_open_and_set_no_lock_errors.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Criterion 1: control-char detection -----------------------------

    #[test]
    fn detects_control_chars() {
        assert!(contains_control_chars("a\nb"), "newline is a control char");
        assert!(contains_control_chars("a\tb"), "tab is a control char");
        assert!(contains_control_chars("a\0b"), "NUL is a control char");
    }

    #[test]
    fn allows_printable_and_unicode() {
        assert!(!contains_control_chars("/tmp/cache.db"));
        assert!(!contains_control_chars("plain text 123"));
        // High Unicode runes (>= 0x20) are not control chars.
        assert!(!contains_control_chars("café-naïve-日本"));
    }

    // ---- Criterion 2: empty / whitespace → empty path --------------------

    #[test]
    fn empty_input_yields_empty_path() {
        assert_eq!(normalize_path("").expect("empty ok"), PathBuf::new());
        assert_eq!(
            normalize_path("   ").expect("whitespace ok"),
            PathBuf::new(),
            "whitespace-only trims to empty"
        );
    }

    // ---- Criterion 3: control chars rejected -----------------------------

    #[test]
    fn rejects_control_chars() {
        let err = normalize_path("/tmp/a\nb").expect_err("control char in path must error");
        // The GREEN impl rejects bad user-supplied paths as a Usage error; this
        // also keeps the RED stub (which returns Internal) honestly failing.
        assert_eq!(
            err.code,
            defi_errors::Code::Usage,
            "control-char rejection is a usage error"
        );
        assert!(
            err.message.contains("control characters"),
            "message names the cause, got: {}",
            err.message
        );
    }

    // ---- Criterion 4: tilde expansion ------------------------------------

    #[test]
    fn expands_bare_tilde_to_home() {
        let home = dirs_home();
        let got = normalize_path("~").expect("~ expands");
        assert_eq!(got, home, "bare ~ expands to home dir");
        assert!(got.is_absolute());
    }

    #[test]
    fn expands_tilde_slash_prefix() {
        let home = dirs_home();
        let got = normalize_path("~/sub/dir").expect("~/ expands");
        assert_eq!(got, home.join("sub").join("dir"));
        assert!(got.is_absolute());
    }

    // ---- Criterion 5: absolutize + clean ---------------------------------

    #[test]
    fn relative_input_is_absolutized_and_cleaned() {
        let cwd = std::env::current_dir().expect("cwd");
        let got = normalize_path("a/./b/../c").expect("relative ok");
        assert!(got.is_absolute(), "result must be absolute");
        assert_eq!(got, cwd.join("a").join("c"), "lexically cleaned");
    }

    #[test]
    fn absolute_input_is_cleaned() {
        let got = normalize_path("/var/tmp/../cache/./db").expect("abs ok");
        assert_eq!(got, PathBuf::from("/var/cache/db"));
    }

    /// The user's home directory, resolved the same way the implementation must
    /// (so the test is not coupled to an env-var detail of the impl).
    fn dirs_home() -> PathBuf {
        #[allow(deprecated)]
        std::env::home_dir().expect("home dir available in test env")
    }
}
