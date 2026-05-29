//! `yield` command group handler (Go: `internal/app` — `newYieldCommand` in
//! `runner.go` + `yield_execution_commands.go`).
//!
//! This module owns the **yield-command-specific** glue that sits between the
//! runner's cache-flow core ([`crate::runner`]) and the provider/execution
//! layers:
//!
//! * the yield read commands' ranking/aggregation primitives
//!   (`opportunities` dedup + sort, `positions` sort, `history` sort,
//!   opportunity-id filtering, per-command limit truncation);
//! * the `history` argument parsing (`metrics`, `interval` incl. aliases, and
//!   the `from/to/window` → `[start,end]` range resolution);
//! * the `positions` input validation + provider-capability gate, and the
//!   `history` provider-capability gate;
//! * the yield execution verb → persisted-intent mapping (`yield_<verb>`) used
//!   by `deposit|withdraw {plan,submit,status}`.
//!
//! Provider SELECTION (`select_yield_providers`, the chain-family default
//! filter) and the shared `split_csv`/`normalize_lending_provider` helpers live
//! in [`crate::runner`] and are NOT re-owned here; this module consumes them.
//! Action-construction routing (`build_yield_action`) lives in
//! `defi_execution::builder` and is NOT re-owned here either.

#![allow(dead_code, unused_variables)]

use chrono::{DateTime, Utc};
use defi_errors::{Code, Error};
use defi_execution::builder::YieldVerb;
use defi_id::Chain;
use defi_model::{YieldHistorySeries, YieldOpportunity, YieldPosition};
use defi_providers::{
    YieldHistoryInterval, YieldHistoryMetric, YieldHistoryProvider, YieldPositionsProvider,
    YieldPositionsRequest,
};

/// The persisted action intent type for a yield execution verb.
///
/// Parity with Go `expectedIntent := "yield_" + string(verb)` in
/// `yield_execution_commands.go`. `plan` writes this onto the action; `submit` /
/// `status` reject an action whose `intent_type` does not match.
pub fn yield_verb_intent(verb: YieldVerb) -> String {
    let suffix = match verb {
        YieldVerb::Deposit => "deposit",
        YieldVerb::Withdraw => "withdraw",
    };
    format!("yield_{suffix}")
}

/// Truncate a list of yield opportunities to `limit`.
///
/// Parity with the inline `combined[:req.Limit]` guard in `newYieldCommand`: a
/// non-positive `limit`, or a list already at/under the limit, is returned
/// unchanged; otherwise the first `limit` items are kept (order preserved). The
/// same shape applies to positions truncation.
pub fn apply_yield_opportunity_limit(
    mut items: Vec<YieldOpportunity>,
    limit: i64,
) -> Vec<YieldOpportunity> {
    if limit <= 0 || (items.len() as i64) <= limit {
        return items;
    }
    items.truncate(limit as usize);
    items
}

/// Truncate a list of yield positions to `limit` (same semantics as
/// [`apply_yield_opportunity_limit`]).
pub fn apply_yield_position_limit(mut items: Vec<YieldPosition>, limit: i64) -> Vec<YieldPosition> {
    if limit <= 0 || (items.len() as i64) <= limit {
        return items;
    }
    items.truncate(limit as usize);
    items
}

/// Total ordering predicate for ranking yield opportunities (Go
/// `compareYieldOpportunities`): returns `true` iff `a` should sort BEFORE `b`.
///
/// Primary key is `sort_by` (`tvl_usd`|`liquidity_usd`|else `apy_total`),
/// always descending; ties break by `apy_total` desc, then `tvl_usd` desc,
/// then `liquidity_usd` desc, then `opportunity_id` ascending (lexicographic).
pub fn compare_yield_opportunities(
    a: &YieldOpportunity,
    b: &YieldOpportunity,
    sort_by: &str,
) -> bool {
    match sort_by {
        "tvl_usd" => {
            if a.tvl_usd != b.tvl_usd {
                return a.tvl_usd > b.tvl_usd;
            }
        }
        "liquidity_usd" => {
            if a.liquidity_usd != b.liquidity_usd {
                return a.liquidity_usd > b.liquidity_usd;
            }
        }
        _ => {
            if a.apy_total != b.apy_total {
                return a.apy_total > b.apy_total;
            }
        }
    }
    if a.apy_total != b.apy_total {
        return a.apy_total > b.apy_total;
    }
    if a.tvl_usd != b.tvl_usd {
        return a.tvl_usd > b.tvl_usd;
    }
    if a.liquidity_usd != b.liquidity_usd {
        return a.liquidity_usd > b.liquidity_usd;
    }
    a.opportunity_id < b.opportunity_id
}

/// Sort yield opportunities in place (Go `sortYieldOpportunities`). An empty /
/// blank `sort_by` defaults to `apy_total`.
pub fn sort_yield_opportunities(items: &mut [YieldOpportunity], sort_by: &str) {
    let key = sort_by.trim().to_ascii_lowercase();
    let key = if key.is_empty() { "apy_total" } else { &key };
    items.sort_by(|a, b| {
        if compare_yield_opportunities(a, b, key) {
            std::cmp::Ordering::Less
        } else if compare_yield_opportunities(b, a, key) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
}

/// De-duplicate opportunities by `opportunity_id`, keeping the
/// best-by-`apy_total` row for each id (Go `dedupeYieldByOpportunityID`).
///
/// Inputs of length <= 1 are returned unchanged. The returned set is NOT
/// ordered (the caller sorts afterwards), so assertions over it must be
/// order-independent.
pub fn dedupe_yield_by_opportunity_id(items: Vec<YieldOpportunity>) -> Vec<YieldOpportunity> {
    if items.len() <= 1 {
        return items;
    }
    let mut by_id: std::collections::HashMap<String, YieldOpportunity> =
        std::collections::HashMap::with_capacity(items.len());
    for item in items {
        match by_id.get(&item.opportunity_id) {
            Some(existing) if !compare_yield_opportunities(&item, existing, "apy_total") => {}
            _ => {
                by_id.insert(item.opportunity_id.clone(), item);
            }
        }
    }
    by_id.into_values().collect()
}

/// Keep only opportunities whose (trimmed, lowercased) `opportunity_id` is in
/// `ids` (Go `filterYieldOpportunitiesByID`). An empty `ids` set returns the
/// input unchanged.
pub fn filter_yield_opportunities_by_id(
    items: Vec<YieldOpportunity>,
    ids: &[String],
) -> Vec<YieldOpportunity> {
    if ids.is_empty() {
        return items;
    }
    let wanted: std::collections::HashSet<String> = ids
        .iter()
        .map(|id| id.trim().to_ascii_lowercase())
        .collect();
    items
        .into_iter()
        .filter(|item| wanted.contains(&item.opportunity_id.trim().to_ascii_lowercase()))
        .collect()
}

/// Sort yield positions in place (Go `sortYieldPositions`): `amount_usd` desc,
/// then `apy_total` desc, then `provider` asc, then `asset_id` asc, then
/// `provider_native_id` asc.
pub fn sort_yield_positions(items: &mut [YieldPosition]) {
    items.sort_by(|a, b| {
        b.amount_usd
            .partial_cmp(&a.amount_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                b.apy_total
                    .partial_cmp(&a.apy_total)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then_with(|| a.provider.cmp(&b.provider))
            .then_with(|| a.asset_id.cmp(&b.asset_id))
            .then_with(|| a.provider_native_id.cmp(&b.provider_native_id))
    });
}

/// Sort yield history series in place (Go `sortYieldHistorySeries`): each
/// series' points are first sorted by `timestamp` asc, then series are ordered
/// by `provider`, `opportunity_id`, `metric`, `interval`, `start_time` (all
/// ascending lexicographic).
pub fn sort_yield_history_series(items: &mut [YieldHistorySeries]) {
    for series in items.iter_mut() {
        series.points.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    }
    items.sort_by(|a, b| {
        a.provider
            .cmp(&b.provider)
            .then_with(|| a.opportunity_id.cmp(&b.opportunity_id))
            .then_with(|| a.metric.cmp(&b.metric))
            .then_with(|| a.interval.cmp(&b.interval))
            .then_with(|| a.start_time.cmp(&b.start_time))
    });
}

/// Parse and de-duplicate the `--metrics` CSV (Go `parseYieldHistoryMetrics`).
///
/// Empty input defaults to `[ApyTotal]`. Order of first occurrence is
/// preserved; duplicates are dropped. An unknown metric is a usage error.
pub fn parse_yield_history_metrics(input: &str) -> Result<Vec<YieldHistoryMetric>, Error> {
    let parts: Vec<String> = input
        .split(',')
        .map(|p| p.trim().to_ascii_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return Ok(vec![YieldHistoryMetric::ApyTotal]);
    }
    let mut out: Vec<YieldHistoryMetric> = Vec::with_capacity(parts.len());
    for part in parts {
        let Some(metric) = YieldHistoryMetric::parse(&part) else {
            return Err(Error::new(
                Code::Usage,
                "--metrics must be one or more of: apy_total,tvl_usd",
            ));
        };
        if !out.contains(&metric) {
            out.push(metric);
        }
    }
    Ok(out)
}

/// Parse the `--interval` selector (Go `parseYieldHistoryInterval`), INCLUDING
/// aliases: ``/`day`/`daily`/`1d` → Day; `hour`/`hourly`/`1h` → Hour. An
/// unknown value is a usage error. (Alias handling is the runner's job here, NOT
/// the provider enum's `parse`.)
pub fn parse_yield_history_interval(input: &str) -> Result<YieldHistoryInterval, Error> {
    match input.trim().to_ascii_lowercase().as_str() {
        "" | "day" | "daily" | "1d" => Ok(YieldHistoryInterval::Day),
        "hour" | "hourly" | "1h" => Ok(YieldHistoryInterval::Hour),
        _ => Err(Error::new(
            Code::Usage,
            "--interval must be one of: hour,day",
        )),
    }
}

/// Resolve the `[start,end]` history range from `--from`/`--to`/`--window`
/// against a caller-supplied `now` (Go `resolveYieldHistoryRange`).
///
/// * `to` defaults to `now`; an explicit `to` must parse (RFC3339) and may not
///   be more than 5m in the future (usage error otherwise);
/// * `from` defaults to `end - window` (default window `7d`); an explicit
///   `from` must parse;
/// * the range must be non-empty (`from < to`) and at most `366d`.
///
/// All inputs are interpreted/returned in UTC.
pub fn resolve_yield_history_range(
    from_arg: &str,
    to_arg: &str,
    window_arg: &str,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), Error> {
    let mut end_time = now;
    if !to_arg.trim().is_empty() {
        end_time = parse_rfc3339(to_arg)
            .map_err(|e| Error::new(Code::Usage, format!("parse --to: {e}")))?;
    }
    if end_time > now + chrono::Duration::minutes(5) {
        return Err(Error::new(Code::Usage, "--to cannot be in the future"));
    }

    let start_time = if !from_arg.trim().is_empty() {
        parse_rfc3339(from_arg)
            .map_err(|e| Error::new(Code::Usage, format!("parse --from: {e}")))?
    } else {
        let window = parse_lookback_window(window_arg)
            .map_err(|e| Error::new(Code::Usage, format!("parse --window: {e}")))?;
        end_time - window
    };

    if start_time >= end_time {
        return Err(Error::new(
            Code::Usage,
            "history range must have --from before --to",
        ));
    }
    if end_time - start_time > chrono::Duration::days(366) {
        return Err(Error::new(Code::Usage, "history range cannot exceed 366d"));
    }
    Ok((start_time, end_time))
}

/// Parse an RFC3339(-nano) timestamp into UTC (Go `parseRFC3339`).
fn parse_rfc3339(raw: &str) -> Result<DateTime<Utc>, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("empty timestamp".to_string());
    }
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| "expected RFC3339 timestamp".to_string())
}

/// Parse a lookback window (Go `parseLookbackWindow`).
///
/// Empty defaults to `7d`. Supports `Nd` (days), `Nw` (weeks), and Go-style
/// duration suffixes (`h`/`m`/`s`). The result must be strictly positive.
fn parse_lookback_window(raw: &str) -> Result<chrono::Duration, String> {
    let mut value = raw.trim().to_ascii_lowercase();
    if value.is_empty() {
        value = "7d".to_string();
    }
    if let Some(days) = value.strip_suffix('d') {
        let n: i64 = days.parse().map_err(|_| "invalid day window".to_string())?;
        if n <= 0 {
            return Err("invalid day window".to_string());
        }
        return Ok(chrono::Duration::days(n));
    }
    if let Some(weeks) = value.strip_suffix('w') {
        let n: i64 = weeks
            .parse()
            .map_err(|_| "invalid week window".to_string())?;
        if n <= 0 {
            return Err("invalid week window".to_string());
        }
        return Ok(chrono::Duration::weeks(n));
    }
    let d = parse_go_duration(&value).ok_or_else(|| "invalid duration window".to_string())?;
    if d <= chrono::Duration::zero() {
        return Err("invalid duration window".to_string());
    }
    Ok(d)
}

/// Parse a Go-style duration string (e.g. `24h`, `90m`, `1h30m`).
///
/// Mirrors the subset of `time.ParseDuration` reachable from the `--window`
/// default branch: composed `h`/`m`/`s`/`ms`/`us`/`ns` unit segments with
/// integer or fractional magnitudes. Returns `None` on any malformed input.
fn parse_go_duration(input: &str) -> Option<chrono::Duration> {
    let s = input.trim();
    if s.is_empty() || s == "0" {
        return None;
    }
    let bytes = s.as_bytes();
    let mut idx = 0usize;
    let mut total_ns: i128 = 0;
    let mut saw_segment = false;
    while idx < bytes.len() {
        // magnitude (integer + optional fraction)
        let num_start = idx;
        while idx < bytes.len() && (bytes[idx].is_ascii_digit() || bytes[idx] == b'.') {
            idx += 1;
        }
        if idx == num_start {
            return None;
        }
        let magnitude: f64 = s[num_start..idx].parse().ok()?;
        // unit
        let unit_start = idx;
        while idx < bytes.len() && !bytes[idx].is_ascii_digit() && bytes[idx] != b'.' {
            idx += 1;
        }
        if idx == unit_start {
            return None;
        }
        let unit = &s[unit_start..idx];
        let ns_per_unit: f64 = match unit {
            "ns" => 1.0,
            "us" | "\u{00b5}s" => 1_000.0,
            "ms" => 1_000_000.0,
            "s" => 1_000_000_000.0,
            "m" => 60.0 * 1_000_000_000.0,
            "h" => 3_600.0 * 1_000_000_000.0,
            _ => return None,
        };
        total_ns += (magnitude * ns_per_unit) as i128;
        saw_segment = true;
    }
    if !saw_segment {
        return None;
    }
    Some(chrono::Duration::nanoseconds(total_ns as i64))
}

/// A validated `yield positions` query (the inputs needed to build a
/// [`YieldPositionsRequest`] for the selected provider).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YieldPositionsQuery {
    /// Parsed chain.
    pub chain: Chain,
    /// The position-owner account (verbatim, un-lowercased — caller lowercases
    /// for the cache key on EVM chains).
    pub account: String,
}

/// Validate the pre-provider inputs of `yield positions`.
///
/// Parity with the `positionsCmd` `RunE` guard order in `runner.go`:
/// 1. `--chain` parses (delegates to `defi_id::parse_chain`);
/// 2. `--address` is required (usage);
/// 3. on an EVM chain, `--address` must be a valid hex address (usage).
///
/// On success returns the [`YieldPositionsQuery`]; the provider is NOT yet
/// consulted (matching the Go ordering where validation precedes provider
/// selection / the cached fetch closure).
pub fn validate_yield_positions_input(
    chain_arg: &str,
    address: &str,
) -> Result<YieldPositionsQuery, Error> {
    let chain = defi_id::parse_chain(chain_arg)?;
    let account = address.trim();
    if account.is_empty() {
        return Err(Error::new(Code::Usage, "--address is required"));
    }
    if chain.is_evm() && !defi_evm::address::is_hex_address(account) {
        return Err(Error::new(
            Code::Usage,
            "--address must be a valid EVM hex address",
        ));
    }
    Ok(YieldPositionsQuery {
        chain,
        account: account.to_string(),
    })
}

/// Fetch yield positions, enforcing the provider-capability gate.
///
/// Parity with the Go interface assertion
/// `provider.(providers.YieldPositionsProvider)`: a selected yield provider that
/// does not implement positions yields a [`defi_errors::Code::Unsupported`]
/// error whose message contains `"does not support positions"` (modeled here as
/// `positions == None`). Otherwise the request is forwarded to the provider.
pub async fn fetch_yield_positions(
    provider_name: &str,
    positions: Option<&dyn YieldPositionsProvider>,
    req: YieldPositionsRequest,
) -> Result<Vec<YieldPosition>, Error> {
    let Some(provider) = positions else {
        return Err(Error::new(
            Code::Unsupported,
            format!("yield provider {provider_name} does not support positions"),
        ));
    };
    provider.yield_positions(req).await
}

/// Enforce the `yield history` provider-capability gate.
///
/// Parity with the Go interface assertion
/// `provider.(providers.YieldHistoryProvider)`: a selected yield provider that
/// does not implement history yields a [`defi_errors::Code::Unsupported`] error
/// whose message contains `"does not support history"` (modeled here as
/// `history == None`). Returns the trait object when the provider IS capable.
pub fn require_yield_history_capability<'a>(
    provider_name: &str,
    history: Option<&'a dyn YieldHistoryProvider>,
) -> Result<&'a dyn YieldHistoryProvider, Error> {
    history.ok_or_else(|| {
        Error::new(
            Code::Unsupported,
            format!("yield provider {provider_name} does not support history"),
        )
    })
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::yield` (Go: `internal/app` yield command
    //! group: `newYieldCommand` in `runner.go` + `yield_execution_commands.go`)
    //!
    //! This module owns the **yield-command glue**: ranking/aggregation, history
    //! argument parsing + range resolution, the positions/history capability
    //! gates, and the execution intent mapping. "Correct" means it preserves the
    //! runner-owned yield behaviors AND the stable machine contract (design spec
    //! §2.2 exit codes, §2.4 ids/amounts). Provider SELECTION
    //! (`select_yield_providers`, chain-family defaults) and `split_csv` /
    //! `normalize_lending_provider` are owned by [`crate::runner`] and are NOT
    //! re-asserted here; action-construction routing is owned by
    //! `defi_execution::builder` and is NOT re-asserted here. Criteria:
    //!
    //!  Y1. **Execution intent mapping.** `yield_verb_intent(verb)` is exactly
    //!      `"yield_<verb>"` (`yield_deposit`/`yield_withdraw`) — the persisted
    //!      `Action.intent_type` that `plan` writes and that `submit`/`status`
    //!      match against. (Go `expectedIntent := "yield_" + string(verb)`.)
    //!
    //!  Y2. **Per-command limit truncation.** `apply_yield_opportunity_limit` /
    //!      `apply_yield_position_limit`: a non-positive limit, or a list already
    //!      at/under the limit, is returned UNCHANGED; a longer list keeps
    //!      exactly the first `limit` items in order. (Go `combined[:req.Limit]`
    //!      guard for both opportunities and positions.)
    //!
    //!  Y3. **Opportunity ranking key.** `compare_yield_opportunities` sorts the
    //!      `sort_by` primary key DESC (`tvl_usd`|`liquidity_usd`|default
    //!      `apy_total`), with the deterministic tie-break chain
    //!      `apy_total↓, tvl_usd↓, liquidity_usd↓, opportunity_id↑`.
    //!      `sort_yield_opportunities` applies it stably and treats an empty
    //!      `sort_by` as `apy_total`. (Go `compareYieldOpportunities` /
    //!      `sortYieldOpportunities`.)
    //!
    //!  Y4. **Opportunity de-dup by id.** `dedupe_yield_by_opportunity_id` keeps
    //!      one row per `opportunity_id`, choosing the higher `apy_total`;
    //!      inputs of length <= 1 are returned unchanged. (Go
    //!      `dedupeYieldByOpportunityID`.)
    //!
    //!  Y5. **Opportunity id filter.** `filter_yield_opportunities_by_id` keeps
    //!      only ids in the (trim+lowercase) set; an empty set is a pass-through.
    //!      (Go `filterYieldOpportunitiesByID`.)
    //!
    //!  Y6. **Positions ranking.** `sort_yield_positions` orders by
    //!      `amount_usd↓, apy_total↓, provider↑, asset_id↑, provider_native_id↑`.
    //!      (Go `sortYieldPositions`.)
    //!
    //!  Y7. **History series ordering.** `sort_yield_history_series` sorts each
    //!      series' points by `timestamp↑`, then orders the series by
    //!      `provider↑, opportunity_id↑, metric↑, interval↑, start_time↑`. (Go
    //!      `sortYieldHistorySeries`.)
    //!
    //!  Y8. **History metric parsing.** `parse_yield_history_metrics` defaults
    //!      empty→`[apy_total]`, preserves first-occurrence order, DEDUPES, and
    //!      rejects unknown metrics with [`Code::Usage`]. (Ported from
    //!      `TestParseYieldHistoryMetricsDedupesAndValidates`.)
    //!
    //!  Y9. **History interval ALIASES.** `parse_yield_history_interval` maps
    //!      ``/`day`/`daily`/`1d`→Day and `hour`/`hourly`/`1h`→Hour
    //!      (case/trim-insensitive); unknown → [`Code::Usage`]. (This alias set
    //!      is owned by the runner, NOT the provider enum — Go
    //!      `parseYieldHistoryInterval`.)
    //!
    //! Y10. **History range resolution.** `resolve_yield_history_range` against a
    //!      fixed `now`: default `to`=now and `from`=now-window (default `7d`);
    //!      explicit RFC3339 `from`/`to` honored in UTC; a `to` >5m in the future
    //!      is [`Code::Usage`]; an empty/inverted range (`from >= to`) is
    //!      [`Code::Usage`]; a range exceeding `366d` is [`Code::Usage`]. (Go
    //!      `resolveYieldHistoryRange`; matches the `--window 24h` math asserted
    //!      by `TestYieldHistoryCommandCallsProvider`.)
    //!
    //! Y11. **Positions input validation order + exit codes.**
    //!      `validate_yield_positions_input` mirrors the Go `positionsCmd` guard
    //!      order, each failure carrying [`Code::Usage`] (exit 2):
    //!      a. an unparseable `--chain` surfaces the id error;
    //!      b. empty `--address` → usage error;
    //!      c. on an EVM chain a non-hex `--address` → usage error (parity with
    //!      go-ethereum `common.IsHexAddress`);
    //!      and on success returns the parsed chain + verbatim account.
    //!      (Ported from the setup of `TestYieldPositionsCommandCallsProvider`.)
    //!
    //! Y12. **Positions capability gate.** `fetch_yield_positions` with
    //!      `positions == None` fails with [`Code::Unsupported`] (exit 13) and a
    //!      message containing `"does not support positions"`, WITHOUT touching
    //!      the provider; with a capable provider it forwards the request
    //!      verbatim exactly once and returns its rows. (Ported from
    //!      `TestYieldPositionsCommandCallsProvider`.)
    //!
    //! Y13. **History capability gate.** `require_yield_history_capability` with
    //!      `history == None` fails with [`Code::Unsupported`] (exit 13) and a
    //!      message containing `"does not support history"`. (Ported from
    //!      `TestYieldHistoryCommandFailsWhenProviderHasNoHistorySupport`.)
    //!
    //! SKIPPED (Go internal-detail / owned elsewhere): cobra flag wiring;
    //! cache-key construction (runner concern); `select_yield_providers` +
    //! chain-family default filtering (runner concern, tested there); the full
    //! `plan/submit/status` signer/backend plumbing (execution-crate concern);
    //! adapter HTTP behavior (per-provider wiremock suites).

    use super::*;
    use async_trait::async_trait;
    use chrono::TimeZone;
    use defi_errors::{exit_code, Code};
    use defi_id::{parse_chain, Asset};
    use defi_model::{AmountInfo, ProviderInfo};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // --- fixtures ----------------------------------------------------------

    /// A yield opportunity with tunable ranking fields; everything else fixed.
    fn opp(id: &str, apy_total: f64, tvl_usd: f64, liquidity_usd: f64) -> YieldOpportunity {
        YieldOpportunity {
            opportunity_id: id.to_string(),
            provider: "aave".to_string(),
            protocol: "aave-v3".to_string(),
            chain_id: "eip155:1".to_string(),
            asset_id: "eip155:1/erc20:0xa0b8".to_string(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            opportunity_type: "lending".to_string(),
            apy_base: 0.0,
            apy_reward: 0.0,
            apy_total,
            tvl_usd,
            liquidity_usd,
            lockup_days: 0.0,
            withdrawal_terms: String::new(),
            backing_assets: Vec::new(),
            source_url: String::new(),
            fetched_at: "2026-05-28T00:00:00Z".to_string(),
        }
    }

    fn position(
        provider: &str,
        asset_id: &str,
        native_id: &str,
        amount_usd: f64,
        apy_total: f64,
    ) -> YieldPosition {
        YieldPosition {
            protocol: provider.to_string(),
            provider: provider.to_string(),
            chain_id: "eip155:1".to_string(),
            account_address: "0x000000000000000000000000000000000000dead".to_string(),
            position_type: "deposit".to_string(),
            opportunity_id: "opp-1".to_string(),
            asset_id: asset_id.to_string(),
            provider_native_id: native_id.to_string(),
            provider_native_id_kind: String::new(),
            amount: AmountInfo::default(),
            shares: None,
            amount_usd,
            apy_total,
            source_url: String::new(),
            fetched_at: "2026-05-28T00:00:00Z".to_string(),
        }
    }

    fn series(
        provider: &str,
        opportunity_id: &str,
        metric: &str,
        interval: &str,
        start_time: &str,
        points: Vec<defi_model::YieldHistoryPoint>,
    ) -> YieldHistorySeries {
        YieldHistorySeries {
            opportunity_id: opportunity_id.to_string(),
            provider: provider.to_string(),
            protocol: provider.to_string(),
            chain_id: "eip155:1".to_string(),
            asset_id: "eip155:1/erc20:0xa0b8".to_string(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            metric: metric.to_string(),
            interval: interval.to_string(),
            start_time: start_time.to_string(),
            end_time: "2026-05-28T00:00:00Z".to_string(),
            points,
            source_url: String::new(),
            fetched_at: "2026-05-28T00:00:00Z".to_string(),
        }
    }

    fn point(ts: &str, value: f64) -> defi_model::YieldHistoryPoint {
        defi_model::YieldHistoryPoint {
            timestamp: ts.to_string(),
            value,
        }
    }

    /// A fake positions-capable provider that records the request it received.
    struct FakeYieldPositionsProvider {
        name: String,
        rows: Vec<YieldPosition>,
        calls: AtomicUsize,
        last_req: std::sync::Mutex<Option<YieldPositionsRequest>>,
    }

    impl FakeYieldPositionsProvider {
        fn new(name: &str, rows: Vec<YieldPosition>) -> Self {
            Self {
                name: name.to_string(),
                rows,
                calls: AtomicUsize::new(0),
                last_req: std::sync::Mutex::new(None),
            }
        }
    }

    impl defi_providers::Provider for FakeYieldPositionsProvider {
        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                name: self.name.clone(),
                provider_type: "yield".to_string(),
                requires_key: false,
                capabilities: vec![
                    "yield.opportunities".to_string(),
                    "yield.positions".to_string(),
                ],
                key_env_var_name: String::new(),
                capability_auth: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl YieldPositionsProvider for FakeYieldPositionsProvider {
        async fn yield_positions(
            &self,
            req: YieldPositionsRequest,
        ) -> Result<Vec<YieldPosition>, Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_req.lock().unwrap() = Some(req);
            Ok(self.rows.clone())
        }
    }

    // ===== Y1: execution intent mapping ===================================

    #[test]
    fn yield_verb_intent_is_yield_prefixed_verb() {
        assert_eq!(yield_verb_intent(YieldVerb::Deposit), "yield_deposit");
        assert_eq!(yield_verb_intent(YieldVerb::Withdraw), "yield_withdraw");
    }

    // ===== Y2: limit truncation ===========================================

    #[test]
    fn apply_yield_opportunity_limit_truncates_and_passes_through() {
        let items = vec![
            opp("a", 1.0, 0.0, 0.0),
            opp("b", 2.0, 0.0, 0.0),
            opp("c", 3.0, 0.0, 0.0),
        ];
        // non-positive limit => unchanged.
        assert_eq!(apply_yield_opportunity_limit(items.clone(), 0).len(), 3);
        assert_eq!(apply_yield_opportunity_limit(items.clone(), -1).len(), 3);
        // limit >= len => unchanged.
        assert_eq!(apply_yield_opportunity_limit(items.clone(), 3).len(), 3);
        assert_eq!(apply_yield_opportunity_limit(items.clone(), 9).len(), 3);
        // limit < len => first `limit`, order preserved.
        let truncated = apply_yield_opportunity_limit(items.clone(), 2);
        assert_eq!(truncated.len(), 2);
        assert_eq!(truncated[0].opportunity_id, "a");
        assert_eq!(truncated[1].opportunity_id, "b");
    }

    #[test]
    fn apply_yield_position_limit_truncates_and_passes_through() {
        let items = vec![
            position("aave", "asset-a", "n1", 3.0, 1.0),
            position("morpho", "asset-b", "n2", 2.0, 1.0),
        ];
        assert_eq!(apply_yield_position_limit(items.clone(), 0).len(), 2);
        assert_eq!(apply_yield_position_limit(items.clone(), 5).len(), 2);
        let truncated = apply_yield_position_limit(items.clone(), 1);
        assert_eq!(truncated.len(), 1);
        assert_eq!(truncated[0].provider, "aave");
    }

    // ===== Y3: opportunity ranking ========================================

    #[test]
    fn compare_opportunities_uses_primary_key_descending() {
        // apy_total (default): higher apy sorts first.
        let high = opp("hi", 5.0, 1.0, 1.0);
        let low = opp("lo", 1.0, 9.0, 9.0);
        assert!(compare_yield_opportunities(&high, &low, "apy_total"));
        assert!(!compare_yield_opportunities(&low, &high, "apy_total"));

        // tvl_usd primary key.
        let big_tvl = opp("a", 1.0, 100.0, 1.0);
        let small_tvl = opp("b", 9.0, 10.0, 1.0);
        assert!(compare_yield_opportunities(&big_tvl, &small_tvl, "tvl_usd"));

        // liquidity_usd primary key.
        let big_liq = opp("a", 1.0, 1.0, 100.0);
        let small_liq = opp("b", 9.0, 9.0, 10.0);
        assert!(compare_yield_opportunities(
            &big_liq,
            &small_liq,
            "liquidity_usd"
        ));
    }

    #[test]
    fn compare_opportunities_tie_breaks_deterministically() {
        // Equal on the primary key (apy_total) AND apy_total tie-break AND tvl
        // AND liquidity => fall through to opportunity_id ascending.
        let a = opp("alpha", 2.0, 5.0, 5.0);
        let b = opp("beta", 2.0, 5.0, 5.0);
        assert!(compare_yield_opportunities(&a, &b, "apy_total"));
        assert!(!compare_yield_opportunities(&b, &a, "apy_total"));
    }

    #[test]
    fn sort_opportunities_defaults_empty_sort_to_apy_total() {
        let mut items = vec![
            opp("low", 1.0, 0.0, 0.0),
            opp("high", 9.0, 0.0, 0.0),
            opp("mid", 5.0, 0.0, 0.0),
        ];
        sort_yield_opportunities(&mut items, "");
        let order: Vec<&str> = items.iter().map(|o| o.opportunity_id.as_str()).collect();
        assert_eq!(order, vec!["high", "mid", "low"]);
    }

    // ===== Y4: de-dup by opportunity id ===================================

    #[test]
    fn dedupe_keeps_best_apy_per_id_and_passes_short_inputs() {
        // len <= 1 unchanged.
        let single = vec![opp("x", 1.0, 0.0, 0.0)];
        assert_eq!(dedupe_yield_by_opportunity_id(single).len(), 1);

        let items = vec![
            opp("dup", 1.0, 0.0, 0.0),
            opp("dup", 7.0, 0.0, 0.0), // higher apy wins
            opp("solo", 2.0, 0.0, 0.0),
        ];
        let mut deduped = dedupe_yield_by_opportunity_id(items);
        assert_eq!(deduped.len(), 2);
        // ordering is undefined post-dedup; sort to assert deterministically.
        deduped.sort_by(|a, b| a.opportunity_id.cmp(&b.opportunity_id));
        assert_eq!(deduped[0].opportunity_id, "dup");
        assert_eq!(deduped[0].apy_total, 7.0);
        assert_eq!(deduped[1].opportunity_id, "solo");
    }

    // ===== Y5: opportunity id filter ======================================

    #[test]
    fn filter_by_id_is_trim_lowercase_and_empty_passes_through() {
        let items = vec![opp("Keep-Me", 1.0, 0.0, 0.0), opp("drop-me", 2.0, 0.0, 0.0)];
        // empty filter => unchanged.
        assert_eq!(
            filter_yield_opportunities_by_id(items.clone(), &[]).len(),
            2
        );
        // filter normalizes case/whitespace.
        let kept = filter_yield_opportunities_by_id(items.clone(), &["  keep-me ".to_string()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].opportunity_id, "Keep-Me");
    }

    // ===== Y6: positions ranking ==========================================

    #[test]
    fn sort_positions_orders_by_amount_then_apy_then_strings() {
        let mut items = vec![
            position("zeta", "asset-z", "nz", 1.0, 1.0),
            position("alpha", "asset-a", "na", 5.0, 1.0), // biggest USD => first
            position("beta", "asset-b", "nb", 1.0, 9.0),  // ties USD with zeta but higher apy
        ];
        sort_yield_positions(&mut items);
        let order: Vec<&str> = items.iter().map(|p| p.provider.as_str()).collect();
        assert_eq!(order, vec!["alpha", "beta", "zeta"]);
    }

    // ===== Y7: history series ordering ====================================

    #[test]
    fn sort_history_orders_points_then_series() {
        let mut items = vec![
            series(
                "morpho",
                "opp-2",
                "apy_total",
                "day",
                "2026-05-01T00:00:00Z",
                vec![
                    point("2026-05-02T00:00:00Z", 2.0),
                    point("2026-05-01T00:00:00Z", 1.0),
                ],
            ),
            series(
                "aave",
                "opp-1",
                "apy_total",
                "day",
                "2026-05-01T00:00:00Z",
                vec![],
            ),
        ];
        sort_yield_history_series(&mut items);
        // series ordered by provider asc => aave before morpho.
        assert_eq!(items[0].provider, "aave");
        assert_eq!(items[1].provider, "morpho");
        // points within the morpho series sorted by timestamp asc.
        let pts: Vec<&str> = items[1]
            .points
            .iter()
            .map(|p| p.timestamp.as_str())
            .collect();
        assert_eq!(pts, vec!["2026-05-01T00:00:00Z", "2026-05-02T00:00:00Z"]);
    }

    // ===== Y8: history metric parsing =====================================

    #[test]
    fn parse_metrics_dedupes_and_preserves_order() {
        // Ported from TestParseYieldHistoryMetricsDedupesAndValidates.
        let metrics =
            parse_yield_history_metrics("apy_total,tvl_usd,apy_total").expect("valid metrics");
        assert_eq!(
            metrics,
            vec![YieldHistoryMetric::ApyTotal, YieldHistoryMetric::TvlUsd]
        );
    }

    #[test]
    fn parse_metrics_empty_defaults_to_apy_total() {
        let metrics = parse_yield_history_metrics("").expect("defaulted metrics");
        assert_eq!(metrics, vec![YieldHistoryMetric::ApyTotal]);
    }

    #[test]
    fn parse_metrics_rejects_unknown() {
        let err = parse_yield_history_metrics("foo").expect_err("invalid metric rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 2);
    }

    // ===== Y9: history interval aliases ===================================

    #[test]
    fn parse_interval_maps_aliases() {
        for v in ["", "day", "daily", "1d", " DAY "] {
            assert_eq!(
                parse_yield_history_interval(v).expect("day alias"),
                YieldHistoryInterval::Day,
                "input: {v:?}"
            );
        }
        for v in ["hour", "hourly", "1h", "  HOUR"] {
            assert_eq!(
                parse_yield_history_interval(v).expect("hour alias"),
                YieldHistoryInterval::Hour,
                "input: {v:?}"
            );
        }
    }

    #[test]
    fn parse_interval_rejects_unknown() {
        let err = parse_yield_history_interval("fortnight").expect_err("unknown interval rejected");
        assert_eq!(err.code, Code::Usage);
    }

    // ===== Y10: history range resolution ==================================

    fn ts(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, 0, 0)
            .single()
            .expect("valid ts")
    }

    #[test]
    fn range_defaults_to_window_back_from_now() {
        // Parity with the --window 24h math in TestYieldHistoryCommandCallsProvider.
        let now = ts(2026, 2, 26, 20);
        let (start, end) = resolve_yield_history_range("", "", "24h", now).expect("range");
        assert_eq!(end, now);
        assert_eq!(start, now - chrono::Duration::hours(24));
    }

    #[test]
    fn range_default_window_is_7d() {
        let now = ts(2026, 2, 26, 20);
        let (start, end) = resolve_yield_history_range("", "", "", now).expect("range");
        assert_eq!(end, now);
        assert_eq!(start, now - chrono::Duration::days(7));
    }

    #[test]
    fn range_honors_explicit_rfc3339_from_and_to() {
        let now = ts(2026, 2, 26, 20);
        let (start, end) =
            resolve_yield_history_range("2026-02-20T00:00:00Z", "2026-02-25T00:00:00Z", "7d", now)
                .expect("range");
        assert_eq!(start, ts(2026, 2, 20, 0));
        assert_eq!(end, ts(2026, 2, 25, 0));
    }

    #[test]
    fn range_rejects_future_to() {
        let now = ts(2026, 2, 26, 20);
        let err = resolve_yield_history_range("", "2026-03-01T00:00:00Z", "7d", now)
            .expect_err("future to rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn range_rejects_inverted_range() {
        let now = ts(2026, 2, 26, 20);
        let err =
            resolve_yield_history_range("2026-02-25T00:00:00Z", "2026-02-20T00:00:00Z", "7d", now)
                .expect_err("inverted range rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn range_rejects_window_over_366d() {
        let now = ts(2026, 2, 26, 20);
        let err =
            resolve_yield_history_range("2024-01-01T00:00:00Z", "2026-02-25T00:00:00Z", "7d", now)
                .expect_err("over-366d range rejected");
        assert_eq!(err.code, Code::Usage);
    }

    // ===== Y11: positions input validation ================================

    #[test]
    fn positions_input_rejects_unparseable_chain() {
        let err = validate_yield_positions_input("definitely-not-a-chain", "0xabc")
            .expect_err("bad chain rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn positions_input_requires_address() {
        let err = validate_yield_positions_input("1", "").expect_err("empty address rejected");
        assert_eq!(err.code, Code::Usage);
        assert!(
            err.to_string().to_lowercase().contains("address"),
            "got: {err}"
        );
    }

    #[test]
    fn positions_input_rejects_invalid_evm_address() {
        let err = validate_yield_positions_input("1", "not-an-address")
            .expect_err("invalid evm address rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn positions_input_accepts_valid_inputs_verbatim_account() {
        // Parity with the happy path of TestYieldPositionsCommandCallsProvider.
        let q = validate_yield_positions_input("1", "0x000000000000000000000000000000000000dEaD")
            .expect("valid positions input");
        assert_eq!(q.chain.caip2, "eip155:1");
        // account preserved verbatim (caller lowercases only for the cache key).
        assert_eq!(q.account, "0x000000000000000000000000000000000000dEaD");
    }

    // ===== Y12: positions capability gate =================================

    #[tokio::test]
    async fn fetch_positions_without_capability_is_unsupported() {
        let req = YieldPositionsRequest {
            chain: parse_chain("solana").expect("solana"),
            account: "6dM4QgP1VnRfx6TVV1t5hBf3ytA5Qn2ATqNnSboP8qz5".to_string(),
            asset: Asset::default(),
            limit: 20,
            rpc_url: String::new(),
        };
        let err = fetch_yield_positions("kamino", None, req)
            .await
            .expect_err("missing positions capability rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .to_lowercase()
                .contains("does not support positions"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn fetch_positions_forwards_request_and_returns_rows() {
        // Parity with TestYieldPositionsCommandCallsProvider.
        let provider = FakeYieldPositionsProvider::new(
            "morpho",
            vec![position(
                "morpho",
                "eip155:1/erc20:0xa0b8",
                "0x1111",
                1.0,
                4.2,
            )],
        );
        let req = YieldPositionsRequest {
            chain: parse_chain("1").expect("mainnet"),
            account: "0x000000000000000000000000000000000000dEaD".to_string(),
            asset: Asset::default(),
            limit: 5,
            rpc_url: String::new(),
        };

        let rows = fetch_yield_positions("morpho", Some(&provider), req)
            .await
            .expect("positions fetched");

        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider, "morpho");

        let last = provider.last_req.lock().unwrap();
        let last = last.as_ref().expect("request recorded");
        assert_eq!(last.chain.caip2, "eip155:1");
        assert_eq!(last.account, "0x000000000000000000000000000000000000dEaD");
        assert_eq!(last.limit, 5);
    }

    // ===== Y13: history capability gate ===================================

    #[test]
    fn require_history_capability_rejects_incapable_provider() {
        // Parity with TestYieldHistoryCommandFailsWhenProviderHasNoHistorySupport.
        // `Ok` carries `&dyn YieldHistoryProvider` (not `Debug`), so pattern-match
        // instead of `expect_err`.
        let err = match require_yield_history_capability("aave", None) {
            Ok(_) => panic!("missing history capability should be rejected"),
            Err(e) => e,
        };
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .to_lowercase()
                .contains("does not support history"),
            "got: {err}"
        );
    }
}
