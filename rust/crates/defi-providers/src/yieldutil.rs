//! yieldutil — shared yield-opportunity ranking + numeric selection helpers.
//!
//! Ports `internal/providers/yieldutil/yieldutil.go`. This module owns two
//! deterministic, offline helpers shared by every yield-capable provider
//! adapter (aave, morpho, moonwell, kamino, defillama):
//!
//! * [`positive_first`] — pick the first usable USD/APY figure from a list of
//!   candidate readings, skipping non-finite and non-positive values.
//! * [`sort_opportunities`] — rank a slice of [`defi_model::YieldOpportunity`]
//!   for stable, automation-friendly output.
//!
//! Phase 2 RED: tests are written first and MUST fail until the real
//! implementation lands.

use std::cmp::Ordering;

use defi_model::YieldOpportunity;

/// Return the first strictly-positive, finite value scanning left to right.
///
/// Skips zero, negative, `NaN`, and `±Inf` values; returns `0.0` when nothing
/// qualifies (mirrors Go `yieldutil.PositiveFirst`).
pub fn positive_first(values: &[f64]) -> f64 {
    for &value in values {
        if value > 0.0 && value.is_finite() {
            return value;
        }
    }
    0.0
}

/// Sort yield opportunities in place, descending by the chosen primary key,
/// with a deterministic total-order tie-break chain (mirrors Go
/// `yieldutil.Sort`).
///
/// `sort_by` is trimmed + lowercased; an empty/whitespace or unrecognized key
/// falls back to `apy_total`. The tie-break chain after the primary key is
/// `apy_total` desc -> `tvl_usd` desc -> `liquidity_usd` desc ->
/// `opportunity_id` ascending lexicographic, which guarantees a stable,
/// reproducible order across runs.
pub fn sort_opportunities(items: &mut [YieldOpportunity], sort_by: &str) {
    let key = sort_by.trim().to_ascii_lowercase();
    let key = if key.is_empty() { "apy_total" } else { &key };

    items.sort_by(|a, b| {
        // Primary key (descending). An unknown key falls through to the shared
        // chain below, which leads with `apy_total` — matching the Go default.
        let primary = match key {
            "tvl_usd" => desc(a.tvl_usd, b.tvl_usd),
            "liquidity_usd" => desc(a.liquidity_usd, b.liquidity_usd),
            // "apy_total" and any unrecognized key.
            _ => desc(a.apy_total, b.apy_total),
        };
        if primary != Ordering::Equal {
            return primary;
        }
        // Shared deterministic tie-break chain.
        desc(a.apy_total, b.apy_total)
            .then_with(|| desc(a.tvl_usd, b.tvl_usd))
            .then_with(|| desc(a.liquidity_usd, b.liquidity_usd))
            .then_with(|| a.opportunity_id.cmp(&b.opportunity_id))
    });
}

/// Compare two `f64` values for a DESCENDING sort with a deterministic,
/// panic-free total order. Non-finite values (`NaN`, `±Inf`) are treated as the
/// non-qualifying low end so finite values rank ahead of them.
fn desc(a: f64, b: f64) -> Ordering {
    // Larger finite value should come first (Ordering::Less). Use a
    // total-order rank where higher rank => earlier. Non-finite/NaN sink low.
    fn rank(v: f64) -> f64 {
        if v.is_finite() {
            v
        } else if v == f64::INFINITY {
            // Treat +Inf as non-qualifying (low end) to match the "finite ranks
            // ahead of non-finite" contract while staying deterministic.
            f64::NEG_INFINITY
        } else {
            // NaN and -Inf both sink to the very bottom.
            f64::NEG_INFINITY
        }
    }
    // Descending: b vs a, with a total_cmp fallback for absolute determinism.
    rank(b).partial_cmp(&rank(a)).unwrap_or(Ordering::Equal)
}

#[cfg(test)]
#[allow(clippy::doc_overindented_list_items)]
mod tests {
    //! # Success criteria for `yieldutil`
    //!
    //! The Rust port MUST preserve the exact ranking + selection semantics of
    //! the Go original (`internal/providers/yieldutil`), because the output is
    //! part of the stable machine contract (deterministic ordering of the
    //! `yield opportunities` array) and feeds USD/APY fields consumed by
    //! automation.
    //!
    //! ## `positive_first(values) -> f64`
    //! 1. Returns the FIRST value that is strictly `> 0` AND finite (not `NaN`,
    //!    not `±Inf`), scanning left to right.
    //! 2. Skips zero, negative, `NaN`, and infinite values.
    //! 3. Returns `0.0` when no candidate qualifies (including the empty slice).
    //! 4. Pure / order-sensitive: earlier qualifying values win over later ones.
    //!
    //! ## `sort_opportunities(items, sort_by)`
    //! 5. Sorts IN PLACE, DESCENDING by the chosen primary key.
    //! 6. Recognized `sort_by` keys (case-insensitive, surrounding whitespace
    //!    trimmed): `apy_total`, `tvl_usd`, `liquidity_usd`.
    //! 7. Empty/whitespace `sort_by` defaults to `apy_total`. Any unknown key
    //!    also falls back to `apy_total` ordering.
    //! 8. Deterministic tie-break chain applied after the primary key (and as
    //!    the full ordering once the primary key ties):
    //!       `apy_total` desc -> `tvl_usd` desc -> `liquidity_usd` desc
    //!       -> `opportunity_id` ASCENDING lexicographic (byte order).
    //!    The lexicographic id tie-break guarantees a TOTAL, stable,
    //!    reproducible order across runs.
    //! 9. Non-finite primary metric values must not panic the comparator.
    //!
    //! These criteria are derived from the contract (deterministic ordering),
    //! the Go source, and the two Go tests (`TestPositiveFirst`, `TestSort`)
    //! plus the cross-module determinism test in
    //! `internal/providers/defillama/client_test.go::TestYieldSortDeterministic`.

    use defi_model::YieldOpportunity;

    use super::{positive_first, sort_opportunities};

    /// Build a `YieldOpportunity` with only the ranking-relevant fields set;
    /// everything else gets contract-valid placeholder values.
    fn opp(id: &str, apy_total: f64, tvl_usd: f64, liquidity_usd: f64) -> YieldOpportunity {
        YieldOpportunity {
            opportunity_id: id.to_string(),
            provider: "test".to_string(),
            protocol: "test".to_string(),
            chain_id: "eip155:1".to_string(),
            asset_id: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".to_string(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            opportunity_type: "lending".to_string(),
            apy_base: 0.0,
            apy_reward: 0.0,
            apy_total,
            tvl_usd,
            liquidity_usd,
            lockup_days: 0.0,
            withdrawal_terms: "instant".to_string(),
            backing_assets: Vec::new(),
            source_url: String::new(),
            fetched_at: "2026-05-28T00:00:00Z".to_string(),
        }
    }

    fn ids(items: &[YieldOpportunity]) -> Vec<&str> {
        items.iter().map(|o| o.opportunity_id.as_str()).collect()
    }

    // ---- positive_first -------------------------------------------------

    /// Ported from Go `TestPositiveFirst`:
    /// PositiveFirst(NaN, -1, 0, 4, 5) == 4 — first positive finite value.
    #[test]
    fn positive_first_picks_first_positive_finite() {
        let got = positive_first(&[f64::NAN, -1.0, 0.0, 4.0, 5.0]);
        assert_eq!(got, 4.0, "expected first positive finite value");
    }

    /// Criterion 3: no qualifying value -> 0.0.
    #[test]
    fn positive_first_returns_zero_when_none_qualify() {
        assert_eq!(positive_first(&[]), 0.0, "empty slice -> 0.0");
        assert_eq!(
            positive_first(&[0.0, -1.0, -2.5]),
            0.0,
            "no positives -> 0.0"
        );
        assert_eq!(
            positive_first(&[f64::NAN, f64::INFINITY, f64::NEG_INFINITY]),
            0.0,
            "only non-finite -> 0.0"
        );
    }

    /// Criterion 2: skip infinities and NaN even when followed by a real value.
    #[test]
    fn positive_first_skips_non_finite_then_returns_finite() {
        assert_eq!(positive_first(&[f64::INFINITY, 7.5]), 7.5);
        assert_eq!(
            positive_first(&[f64::NAN, f64::NEG_INFINITY, 0.0, 3.25]),
            3.25
        );
    }

    /// Criterion 4: earlier qualifying value wins; later positives ignored.
    #[test]
    fn positive_first_is_order_sensitive() {
        assert_eq!(positive_first(&[2.0, 9.0]), 2.0);
        // This mirrors real usage e.g. morpho: PositiveFirst(totalAssets, liquidityUSD).
        assert_eq!(positive_first(&[225.0, 100.0]), 225.0);
    }

    // ---- sort_opportunities --------------------------------------------

    /// Ported from Go `TestSort`: sort by apy_total. Equal apy_total + equal
    /// tvl_usd resolves on liquidity_usd desc, then the lower-apy item last.
    /// Input order [b,a,c] (b.liq=40 > a.liq=30) must yield [b, a, c].
    #[test]
    fn sort_by_apy_total_with_liquidity_tie_break() {
        let mut items = vec![
            opp("b", 8.0, 100.0, 40.0),
            opp("a", 8.0, 100.0, 30.0),
            opp("c", 4.0, 90.0, 20.0),
        ];
        sort_opportunities(&mut items, "apy_total");
        assert_eq!(ids(&items), vec!["b", "a", "c"], "unexpected sort order");
    }

    /// Ported from defillama `TestYieldSortDeterministic`: when every ranking
    /// metric ties, fall back to lexicographic `opportunity_id` ASCENDING.
    /// Input [b, a] (identical metrics) must yield [a, b].
    #[test]
    fn sort_lexicographic_tie_break_when_all_metrics_equal() {
        let mut items = vec![opp("b", 10.0, 100.0, 50.0), opp("a", 10.0, 100.0, 50.0)];
        sort_opportunities(&mut items, "apy_total");
        assert_eq!(
            ids(&items),
            vec!["a", "b"],
            "expected lexicographic tie-break"
        );
    }

    /// Criterion 7: empty / whitespace sort_by defaults to apy_total ordering.
    #[test]
    fn empty_sort_by_defaults_to_apy_total() {
        let mut items = vec![opp("low", 1.0, 999.0, 999.0), opp("high", 50.0, 1.0, 1.0)];
        sort_opportunities(&mut items, "   ");
        assert_eq!(
            ids(&items),
            vec!["high", "low"],
            "blank sort_by must default to apy_total desc"
        );
    }

    /// Criterion 6: sort_by is case-insensitive and trimmed.
    #[test]
    fn sort_by_is_case_insensitive_and_trimmed() {
        let mut items = vec![opp("small", 5.0, 10.0, 0.0), opp("big", 5.0, 9000.0, 0.0)];
        sort_opportunities(&mut items, "  TVL_USD  ");
        assert_eq!(
            ids(&items),
            vec!["big", "small"],
            "tvl_usd ranking should apply regardless of case/whitespace"
        );
    }

    /// Criterion 6: primary key tvl_usd ranks by TVL descending.
    #[test]
    fn sort_by_tvl_usd_ranks_by_tvl_descending() {
        let mut items = vec![
            opp("mid", 1.0, 500.0, 0.0),
            opp("top", 1.0, 1000.0, 0.0),
            opp("bot", 1.0, 100.0, 0.0),
        ];
        sort_opportunities(&mut items, "tvl_usd");
        assert_eq!(ids(&items), vec!["top", "mid", "bot"]);
    }

    /// Criterion 6: primary key liquidity_usd ranks by liquidity descending.
    #[test]
    fn sort_by_liquidity_usd_ranks_by_liquidity_descending() {
        let mut items = vec![
            opp("a", 1.0, 1.0, 10.0),
            opp("b", 1.0, 1.0, 90.0),
            opp("c", 1.0, 1.0, 50.0),
        ];
        sort_opportunities(&mut items, "liquidity_usd");
        assert_eq!(ids(&items), vec!["b", "c", "a"]);
    }

    /// Criterion 7: an unrecognized key falls back to apy_total ordering
    /// (NOT a panic, NOT input order).
    #[test]
    fn unknown_sort_by_falls_back_to_apy_total() {
        let mut items = vec![opp("x", 2.0, 5.0, 5.0), opp("y", 9.0, 1.0, 1.0)];
        sort_opportunities(&mut items, "nonsense");
        assert_eq!(ids(&items), vec!["y", "x"], "unknown key -> apy_total desc");
    }

    /// Criterion 8 (full chain): tvl_usd primary, ties resolved through the
    /// shared chain (apy_total desc -> liquidity_usd desc -> id asc).
    #[test]
    fn sort_by_tvl_then_apy_then_liquidity_then_id() {
        let mut items = vec![
            // same tvl(100); apy ties at 5 -> liquidity 10 vs 20 -> "n" before? no:
            opp("n", 5.0, 100.0, 10.0),
            opp("m", 5.0, 100.0, 20.0), // higher liquidity -> ranks first among the tvl=100,apy=5 group
            opp("k", 7.0, 100.0, 0.0),  // higher apy within tvl=100 -> ranks first overall in group
            opp("z", 5.0, 50.0, 999.0), // lower tvl -> ranks last regardless of liquidity
        ];
        sort_opportunities(&mut items, "tvl_usd");
        assert_eq!(ids(&items), vec!["k", "m", "n", "z"]);
    }

    /// Criterion 9: non-finite metric values must not panic the comparator and
    /// must produce a deterministic total order (NaN/Inf treated as the
    /// non-qualifying low end; finite positive values rank ahead of NaN).
    #[test]
    fn sort_does_not_panic_on_non_finite_metrics() {
        let mut items = vec![
            opp("nan", f64::NAN, 1.0, 1.0),
            opp("real", 3.0, 1.0, 1.0),
            opp("inf", f64::INFINITY, 1.0, 1.0),
        ];
        // Must not panic.
        sort_opportunities(&mut items, "apy_total");
        // Determinism: the same input always yields the same order.
        let first_pass = ids(&items)
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let mut again = vec![
            opp("nan", f64::NAN, 1.0, 1.0),
            opp("real", 3.0, 1.0, 1.0),
            opp("inf", f64::INFINITY, 1.0, 1.0),
        ];
        sort_opportunities(&mut again, "apy_total");
        let second_pass = ids(&again)
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        assert_eq!(first_pass, second_pass, "sort must be deterministic");
        // The finite real value must outrank NaN (NaN is non-qualifying).
        let real_pos = first_pass.iter().position(|s| s == "real").unwrap();
        let nan_pos = first_pass.iter().position(|s| s == "nan").unwrap();
        assert!(real_pos < nan_pos, "finite apy must rank ahead of NaN apy");
    }

    /// Empty slice and single-element slice are no-ops (must not panic).
    #[test]
    fn sort_handles_empty_and_single() {
        let mut empty: Vec<YieldOpportunity> = Vec::new();
        sort_opportunities(&mut empty, "apy_total");
        assert!(empty.is_empty());

        let mut one = vec![opp("solo", 1.0, 1.0, 1.0)];
        sort_opportunities(&mut one, "tvl_usd");
        assert_eq!(ids(&one), vec!["solo"]);
    }
}
