//! Cross-provider normalization helpers.
//!
//! Mirrors `internal/providers/normalize.go`. These canonicalize provider
//! NAME aliases used for routing (lending + swap). The traits module delegates
//! ownership of `NormalizeLendingProvider` / `NormalizeSwapProvider` here, and
//! the `defi-app` runner routes on the canonical names these return.
//!
//! Contract (must match Go byte-for-byte):
//!   * input is trimmed and lowercased FIRST;
//!   * known aliases collapse to a canonical name;
//!   * any unknown input falls through as its trimmed-lowercased form (NOT an
//!     error) — the runner decides whether the canonical name is supported.

/// Canonicalize a supported lending provider alias.
///
/// Parity with Go `NormalizeLendingProvider`:
///   * `aave`, `aave-v2`, `aave-v3`        → `aave`
///   * `morpho`, `morpho-blue`             → `morpho`
///   * `kamino`, `kamino-lend`, `kamino-finance` → `kamino`
///   * `moonwell`, `moonwell-v2`           → `moonwell`
///   * anything else                        → trimmed-lowercased input
pub fn normalize_lending_provider(input: &str) -> String {
    let key = input.trim().to_ascii_lowercase();
    match key.as_str() {
        "aave" | "aave-v2" | "aave-v3" => "aave".to_string(),
        "morpho" | "morpho-blue" => "morpho".to_string(),
        "kamino" | "kamino-lend" | "kamino-finance" => "kamino".to_string(),
        "moonwell" | "moonwell-v2" => "moonwell".to_string(),
        _ => key,
    }
}

/// Canonicalize a supported swap provider alias.
///
/// Parity with Go `NormalizeSwapProvider`:
///   * `tempo`, `tempo-dex`, `tempodex`    → `tempo`
///   * anything else                        → trimmed-lowercased input
pub fn normalize_swap_provider(input: &str) -> String {
    let key = input.trim().to_ascii_lowercase();
    match key.as_str() {
        "tempo" | "tempo-dex" | "tempodex" => "tempo".to_string(),
        _ => key,
    }
}

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for `defi-providers::normalize`.
    //!
    //! Go source: `internal/providers/normalize.go` plus the alias cases
    //! exercised by `internal/app/provider_selection_test.go::TestNormalizeLendingProvider`
    //! and `internal/execution/actionbuilder/registry_test.go::TestNormalizeLendingProviderAliases`.
    //!
    //! Correct iff, for BOTH functions:
    //!   N1. Every canonical name maps to itself (idempotent).
    //!   N2. Every documented alias collapses to its canonical name.
    //!   N3. Matching is case-insensitive and whitespace-trimmed (the Go switch
    //!       lowercases + trims BEFORE matching), so `"  AAVE-V3 "` → `"aave"`.
    //!   N4. Unknown input is NOT canonicalized but IS still trimmed+lowercased
    //!       (Go `default:` returns `strings.ToLower(strings.TrimSpace(input))`).
    //!   N5. Lending and swap namespaces are independent: a swap alias is inert
    //!       in the lending function and vice-versa.

    use super::*;

    // ----- N1/N2: lending canonical + alias collapse ----------------------
    #[test]
    fn lending_aliases_collapse_to_canonical() {
        for input in ["aave", "aave-v2", "aave-v3"] {
            assert_eq!(normalize_lending_provider(input), "aave", "input={input}");
        }
        for input in ["morpho", "morpho-blue"] {
            assert_eq!(normalize_lending_provider(input), "morpho", "input={input}");
        }
        for input in ["kamino", "kamino-lend", "kamino-finance"] {
            assert_eq!(normalize_lending_provider(input), "kamino", "input={input}");
        }
        for input in ["moonwell", "moonwell-v2"] {
            assert_eq!(
                normalize_lending_provider(input),
                "moonwell",
                "input={input}"
            );
        }
    }

    // ----- N3: lending trim + case insensitivity --------------------------
    #[test]
    fn lending_is_trim_and_case_insensitive() {
        assert_eq!(normalize_lending_provider("AAVE-V3"), "aave");
        assert_eq!(normalize_lending_provider("  Morpho-Blue  "), "morpho");
        assert_eq!(normalize_lending_provider("\tKAMINO-FINANCE\n"), "kamino");
    }

    // ----- N4: lending unknown falls through, still normalized ------------
    #[test]
    fn lending_unknown_falls_through_trimmed_lowercased() {
        // Go `default:` returns the trimmed+lowercased input, not the raw input.
        assert_eq!(normalize_lending_provider("  Compound "), "compound");
        assert_eq!(normalize_lending_provider("SPARK"), "spark");
        assert_eq!(normalize_lending_provider(""), "");
        // A swap alias must NOT be treated as a lending alias.
        assert_eq!(normalize_lending_provider("tempo-dex"), "tempo-dex");
    }

    // ----- N1/N2: swap canonical + alias collapse -------------------------
    #[test]
    fn swap_aliases_collapse_to_canonical() {
        for input in ["tempo", "tempo-dex", "tempodex"] {
            assert_eq!(normalize_swap_provider(input), "tempo", "input={input}");
        }
    }

    // ----- N3: swap trim + case insensitivity -----------------------------
    #[test]
    fn swap_is_trim_and_case_insensitive() {
        assert_eq!(normalize_swap_provider("  Tempo-DEX  "), "tempo");
        assert_eq!(normalize_swap_provider("TEMPODEX"), "tempo");
    }

    // ----- N4/N5: swap unknown falls through; lending alias inert ---------
    #[test]
    fn swap_unknown_falls_through_trimmed_lowercased() {
        assert_eq!(normalize_swap_provider("  Uniswap "), "uniswap");
        assert_eq!(normalize_swap_provider("1INCH"), "1inch");
        assert_eq!(normalize_swap_provider(""), "");
        // A lending alias must NOT be treated as a swap alias.
        assert_eq!(normalize_swap_provider("aave-v3"), "aave-v3");
    }
}
