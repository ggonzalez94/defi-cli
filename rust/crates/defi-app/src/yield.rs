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
use defi_id::{Asset, Chain};
use defi_model::{ProviderStatus, YieldHistorySeries, YieldOpportunity, YieldPosition};
use defi_providers::{
    YieldHistoryInterval, YieldHistoryMetric, YieldHistoryProvider, YieldHistoryRequest,
    YieldPositionsProvider, YieldPositionsRequest, YieldProvider, YieldRequest,
};

use crate::protocols::status_from_result;
use crate::runner::FetchOutcome;

/// The registered yield providers (Go `s.yieldProviders` map keys).
const YIELD_PROVIDERS: [&str; 4] = ["aave", "morpho", "kamino", "moonwell"];

/// Cache TTL for `yield opportunities` (Go: `60 * time.Second`).
pub const YIELD_OPPORTUNITIES_TTL_SECS: u64 = 60;
/// Cache TTL for `yield positions` (Go: `30 * time.Second`).
pub const YIELD_POSITIONS_TTL_SECS: u64 = 30;
/// Cache TTL for `yield history` (Go: `5 * time.Minute`).
pub const YIELD_HISTORY_TTL_SECS: u64 = 5 * 60;

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

// ---------------------------------------------------------------------------
// provider construction (capability-aware boxed trait objects).
// ---------------------------------------------------------------------------

/// Construct a [`YieldProvider`] for a registered provider name, applying the
/// `--rpc-url` override to the on-chain reader (Moonwell). Mirrors Go
/// `s.yieldProviders[name]` + `applyRPCOverride(provider, rpcURL)`.
fn yield_provider(
    ctx: &crate::ctx::AppCtx,
    provider_name: &str,
    rpc_url: &str,
) -> Result<Box<dyn YieldProvider>, Error> {
    let http = ctx.http_client();
    let provider: Box<dyn YieldProvider> = match provider_name {
        "aave" => Box::new(defi_providers::aave::Client::new(http)),
        "morpho" => Box::new(defi_providers::morpho::Client::new(http)),
        "kamino" => Box::new(defi_providers::kamino::Client::new(http)),
        "moonwell" => {
            let mut client = defi_providers::moonwell::Client::new();
            let trimmed = rpc_url.trim();
            if !trimmed.is_empty() {
                client.set_rpc_override(trimmed);
            }
            Box::new(client)
        }
        _ => {
            return Err(Error::new(
                Code::Unsupported,
                format!("unsupported yield provider: {provider_name}"),
            ))
        }
    };
    Ok(provider)
}

/// Construct a [`YieldPositionsProvider`] for a registered name, or `None` when
/// the provider does not implement positions (Kamino). Mirrors the Go
/// `provider.(providers.YieldPositionsProvider)` interface assertion. The
/// on-chain reader (Moonwell) reads its RPC override from the per-request
/// `rpc_url` field, so no client-side override is applied here.
fn yield_positions_provider(
    ctx: &crate::ctx::AppCtx,
    provider_name: &str,
) -> Result<Option<Box<dyn YieldPositionsProvider>>, Error> {
    let http = ctx.http_client();
    let provider: Option<Box<dyn YieldPositionsProvider>> = match provider_name {
        "aave" => Some(Box::new(defi_providers::aave::Client::new(http))),
        "morpho" => Some(Box::new(defi_providers::morpho::Client::new(http))),
        // Kamino implements YieldProvider but NOT positions (Go capability gate).
        "kamino" => None,
        "moonwell" => Some(Box::new(defi_providers::moonwell::Client::new())),
        _ => {
            return Err(Error::new(
                Code::Unsupported,
                format!("unsupported yield provider: {provider_name}"),
            ))
        }
    };
    Ok(provider)
}

/// Construct a [`YieldHistoryProvider`] for a registered name, or `None` when
/// the provider does not implement history (Moonwell). Mirrors the Go
/// `provider.(providers.YieldHistoryProvider)` interface assertion.
fn yield_history_provider(
    ctx: &crate::ctx::AppCtx,
    provider_name: &str,
) -> Result<Option<Box<dyn YieldHistoryProvider>>, Error> {
    let http = ctx.http_client();
    let provider: Option<Box<dyn YieldHistoryProvider>> = match provider_name {
        "aave" => Some(Box::new(defi_providers::aave::Client::new(http))),
        "morpho" => Some(Box::new(defi_providers::morpho::Client::new(http))),
        "kamino" => Some(Box::new(defi_providers::kamino::Client::new(http))),
        // Moonwell implements YieldProvider + positions but NOT history.
        "moonwell" => None,
        _ => {
            return Err(Error::new(
                Code::Unsupported,
                format!("unsupported yield provider: {provider_name}"),
            ))
        }
    };
    Ok(provider)
}

/// The canonical [`ProviderInfo::name`] for a yield provider (used for status
/// rows when the boxed trait object is `None`, mirroring `provider.Info().Name`).
fn yield_provider_label(ctx: &crate::ctx::AppCtx, provider_name: &str) -> String {
    yield_provider(ctx, provider_name, "")
        .map(|p| {
            use defi_providers::Provider;
            p.info().name
        })
        .unwrap_or_else(|_| provider_name.to_string())
}

/// A provider error captured as a typed [`Error`] for `firstErr` parity (Go
/// keeps the first provider error and falls back to a `CodeUnavailable`
/// "no ... returned by selected providers" if every provider yielded zero rows
/// without erroring).
type FetchErr = (Vec<ProviderStatus>, Vec<String>, bool, Error);

// ---------------------------------------------------------------------------
// read-command orchestration (multi-provider aggregation loops).
// ---------------------------------------------------------------------------

/// Run `yield opportunities`: select providers, fetch from each, aggregate,
/// dedupe, sort, and truncate (Go `opportunitiesCmd` fetch closure).
async fn run_opportunities(
    ctx: &crate::ctx::AppCtx,
    req: &YieldRequest,
    chain: &Chain,
    rpc_url: &str,
) -> Result<FetchOutcome, FetchErr> {
    let selected =
        match crate::runner::select_yield_providers(&YIELD_PROVIDERS, &req.providers, chain) {
            Ok(s) => s,
            Err(err) => return Err((Vec::new(), Vec::new(), false, err)),
        };

    let mut statuses: Vec<ProviderStatus> = Vec::with_capacity(selected.len());
    let mut warnings: Vec<String> = Vec::new();
    let mut combined: Vec<YieldOpportunity> = Vec::new();
    let mut partial = false;
    let mut first_err: Option<Error> = None;

    for provider_name in &selected {
        let provider = match yield_provider(ctx, provider_name, rpc_url) {
            Ok(p) => p,
            Err(err) => return Err((statuses, warnings, partial, err)),
        };
        let name = {
            use defi_providers::Provider;
            provider.info().name
        };
        // Per-provider request: clear the providers filter (Go `reqCopy.Providers
        // = nil`) so the adapter does not re-filter.
        let mut req_copy = req.clone();
        req_copy.providers = Vec::new();
        let res = provider.yield_opportunities(req_copy).await;
        statuses.push(ProviderStatus {
            name: name.clone(),
            status: status_from_result(&res),
            latency_ms: 0,
        });
        match res {
            Ok(items) => combined.extend(items),
            Err(err) => {
                partial = true;
                warnings.push(format!("provider {name} failed: {err}"));
                if first_err.is_none() {
                    first_err = Some(err);
                }
            }
        }
    }

    if req.include_incomplete {
        warnings.push(
            "include_incomplete enabled: opportunities with missing APY/TVL may be present"
                .to_string(),
        );
    }

    if combined.is_empty() {
        let err = first_err.unwrap_or_else(|| {
            Error::new(
                Code::Unavailable,
                "no yield opportunities returned by selected providers",
            )
        });
        return Err((statuses, warnings, partial, err));
    }

    combined = dedupe_yield_by_opportunity_id(combined);
    sort_yield_opportunities(&mut combined, &req.sort_by);
    combined = apply_yield_opportunity_limit(combined, req.limit);
    if req.include_incomplete {
        warnings.push(format!(
            "returned {} combined opportunities across {} provider(s)",
            combined.len(),
            selected.len()
        ));
    }

    let data = serde_json::to_value(&combined).map_err(|e| {
        (
            Vec::new(),
            Vec::new(),
            false,
            Error::wrap(Code::Internal, "serialize yield opportunities", e),
        )
    })?;
    Ok(FetchOutcome {
        data,
        providers: statuses,
        warnings,
        partial,
    })
}

/// Run `yield positions`: select providers, gate each on the positions
/// capability, fetch, aggregate, sort, truncate (Go `positionsCmd` closure).
#[allow(clippy::too_many_arguments)]
async fn run_positions(
    ctx: &crate::ctx::AppCtx,
    chain: &Chain,
    account: &str,
    asset: &Asset,
    provider_filter: &[String],
    limit: i64,
    rpc_url: &str,
) -> Result<FetchOutcome, FetchErr> {
    let selected =
        match crate::runner::select_yield_providers(&YIELD_PROVIDERS, provider_filter, chain) {
            Ok(s) => s,
            Err(err) => return Err((Vec::new(), Vec::new(), false, err)),
        };

    let mut statuses: Vec<ProviderStatus> = Vec::with_capacity(selected.len());
    let mut warnings: Vec<String> = Vec::new();
    let mut combined: Vec<YieldPosition> = Vec::new();
    let mut partial = false;
    let mut first_err: Option<Error> = None;

    for provider_name in &selected {
        let label = yield_provider_label(ctx, provider_name);
        let provider = match yield_positions_provider(ctx, provider_name) {
            Ok(p) => p,
            Err(err) => return Err((statuses, warnings, partial, err)),
        };
        let req = YieldPositionsRequest {
            chain: chain.clone(),
            account: account.to_string(),
            asset: asset.clone(),
            limit,
            rpc_url: rpc_url.trim().to_string(),
        };
        let res = fetch_yield_positions(provider_name, provider.as_deref(), req).await;
        statuses.push(ProviderStatus {
            name: label.clone(),
            status: status_from_result(&res),
            latency_ms: 0,
        });
        match res {
            Ok(items) => combined.extend(items),
            Err(err) => {
                partial = true;
                if matches!(err.code, Code::Unsupported) {
                    warnings.push(format!("provider {label} does not support yield positions"));
                } else {
                    warnings.push(format!("provider {label} failed: {err}"));
                }
                if first_err.is_none() {
                    first_err = Some(err);
                }
            }
        }
    }

    if combined.is_empty() {
        let err = first_err.unwrap_or_else(|| {
            Error::new(
                Code::Unavailable,
                "no yield positions returned by selected providers",
            )
        });
        return Err((statuses, warnings, partial, err));
    }

    sort_yield_positions(&mut combined);
    combined = apply_yield_position_limit(combined, limit);

    let data = serde_json::to_value(&combined).map_err(|e| {
        (
            Vec::new(),
            Vec::new(),
            false,
            Error::wrap(Code::Internal, "serialize yield positions", e),
        )
    })?;
    Ok(FetchOutcome {
        data,
        providers: statuses,
        warnings,
        partial,
    })
}

/// Run `yield history`: select providers, gate each on the history capability,
/// discover opportunities, fetch per-opportunity series, aggregate, sort (Go
/// `historyCmd` closure).
#[allow(clippy::too_many_arguments)]
async fn run_history(
    ctx: &crate::ctx::AppCtx,
    chain: &Chain,
    asset: &Asset,
    provider_filter: &[String],
    metrics: &[YieldHistoryMetric],
    interval: YieldHistoryInterval,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    opportunity_ids: &[String],
    limit: i64,
) -> Result<FetchOutcome, FetchErr> {
    let selected =
        match crate::runner::select_yield_providers(&YIELD_PROVIDERS, provider_filter, chain) {
            Ok(s) => s,
            Err(err) => return Err((Vec::new(), Vec::new(), false, err)),
        };

    let mut statuses: Vec<ProviderStatus> = Vec::with_capacity(selected.len());
    let mut warnings: Vec<String> = Vec::new();
    let mut combined: Vec<YieldHistorySeries> = Vec::new();
    let mut partial = false;
    let mut first_err: Option<Error> = None;

    let has_id_filter = !opportunity_ids.is_empty();

    for provider_name in &selected {
        let label = yield_provider_label(ctx, provider_name);
        let history_provider = match yield_history_provider(ctx, provider_name) {
            Ok(p) => p,
            Err(err) => return Err((statuses, warnings, partial, err)),
        };
        let Some(history_provider) = history_provider else {
            let err = Error::new(
                Code::Unsupported,
                format!("yield provider {provider_name} does not support history"),
            );
            statuses.push(ProviderStatus {
                name: label.clone(),
                status: status_from_result::<()>(&Err(Error::new(err.code, ""))),
                latency_ms: 0,
            });
            warnings.push(format!("provider {label} does not support yield history"));
            partial = true;
            if first_err.is_none() {
                first_err = Some(err);
            }
            continue;
        };

        // Discover opportunities (the history provider is also a YieldProvider).
        let discovery_provider = match yield_provider(ctx, provider_name, "") {
            Ok(p) => p,
            Err(err) => return Err((statuses, warnings, partial, err)),
        };
        let mut discovery_req = YieldRequest {
            chain: chain.clone(),
            asset: asset.clone(),
            limit,
            min_tvl_usd: 0.0,
            min_apy: 0.0,
            providers: Vec::new(),
            sort_by: "apy_total".to_string(),
            include_incomplete: true,
        };
        if has_id_filter {
            discovery_req.limit = 0;
        }
        let discovery = discovery_provider.yield_opportunities(discovery_req).await;
        let mut opportunities = match discovery {
            Ok(o) => o,
            Err(err) => {
                statuses.push(ProviderStatus {
                    name: label.clone(),
                    status: status_from_result::<()>(&Err(Error::new(err.code, ""))),
                    latency_ms: 0,
                });
                warnings.push(format!(
                    "provider {label} failed during opportunity lookup: {err}"
                ));
                partial = true;
                if first_err.is_none() {
                    first_err = Some(err);
                }
                continue;
            }
        };

        if has_id_filter {
            opportunities = filter_yield_opportunities_by_id(opportunities, opportunity_ids);
        }
        if limit > 0 && (opportunities.len() as i64) > limit {
            opportunities.truncate(limit as usize);
        }
        if opportunities.is_empty() {
            let err = Error::new(
                Code::Unavailable,
                format!("provider {provider_name} returned no matching opportunities"),
            );
            statuses.push(ProviderStatus {
                name: label.clone(),
                status: status_from_result::<()>(&Err(Error::new(err.code, ""))),
                latency_ms: 0,
            });
            warnings.push(format!(
                "provider {label} returned no matching opportunities"
            ));
            partial = true;
            if first_err.is_none() {
                first_err = Some(err);
            }
            continue;
        }

        let mut provider_series: Vec<YieldHistorySeries> = Vec::new();
        let mut provider_history_err: Option<Error> = None;
        for opportunity in opportunities {
            let series_res = history_provider
                .yield_history(YieldHistoryRequest {
                    opportunity: opportunity.clone(),
                    start_time,
                    end_time,
                    interval,
                    metrics: metrics.to_vec(),
                })
                .await;
            match series_res {
                Ok(series) => provider_series.extend(series),
                Err(err) => {
                    partial = true;
                    warnings.push(format!(
                        "provider {label} failed history for opportunity {}: {err}",
                        opportunity.opportunity_id
                    ));
                    if provider_history_err.is_none() {
                        provider_history_err = Some(err);
                    }
                }
            }
        }

        let status_err = if let Some(err) = provider_history_err {
            Some(err)
        } else if provider_series.is_empty() {
            Some(Error::new(
                Code::Unavailable,
                format!("provider {provider_name} returned no historical points"),
            ))
        } else {
            None
        };
        let status_str = match &status_err {
            Some(err) => status_from_result::<()>(&Err(Error::new(err.code, ""))),
            None => "ok".to_string(),
        };
        statuses.push(ProviderStatus {
            name: label.clone(),
            status: status_str,
            latency_ms: 0,
        });
        if let Some(err) = status_err {
            if first_err.is_none() {
                first_err = Some(err);
            }
        }
        combined.extend(provider_series);
    }

    if combined.is_empty() {
        let err = first_err.unwrap_or_else(|| {
            Error::new(
                Code::Unavailable,
                "no yield history returned by selected providers",
            )
        });
        return Err((statuses, warnings, partial, err));
    }

    sort_yield_history_series(&mut combined);

    let data = serde_json::to_value(&combined).map_err(|e| {
        (
            Vec::new(),
            Vec::new(),
            false,
            Error::wrap(Code::Internal, "serialize yield history", e),
        )
    })?;
    Ok(FetchOutcome {
        data,
        providers: statuses,
        warnings,
        partial,
    })
}

/// clap parsing + handler for the `yield` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::{Code, Error};
    use defi_execution::builder::{Registry, YieldRequest, YieldVerb};
    use defi_id::normalize_amount;
    use defi_model::{Envelope, ProviderStatus};

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, SubmitArgs};
    use crate::execident::{apply_execution_identity_to_action, resolve_execution_identity};

    /// `yield` subcommands: read data + the two execution verbs.
    #[derive(Subcommand, Debug)]
    pub enum YieldCmd {
        /// Rank yield opportunities.
        Opportunities(OpportunitiesArgs),
        /// List yield positions for an account address.
        Positions(PositionsArgs),
        /// Get yield history for provider opportunities.
        History(HistoryArgs),
        /// Deposit assets into a yield product.
        #[command(subcommand)]
        Deposit(YieldVerbCmd),
        /// Withdraw assets from a yield product.
        #[command(subcommand)]
        Withdraw(YieldVerbCmd),
    }

    impl YieldCmd {
        /// The full path tail (e.g. `opportunities`, `deposit plan`).
        pub fn path(&self) -> String {
            match self {
                YieldCmd::Opportunities(_) => "opportunities".to_string(),
                YieldCmd::Positions(_) => "positions".to_string(),
                YieldCmd::History(_) => "history".to_string(),
                YieldCmd::Deposit(v) => format!("deposit {}", v.path()),
                YieldCmd::Withdraw(v) => format!("withdraw {}", v.path()),
            }
        }
    }

    /// `yield opportunities` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct OpportunitiesArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Asset symbol/address/CAIP-19.
        #[arg(long)]
        pub asset: Option<String>,
        /// Filter by provider names (aave,morpho,kamino,moonwell).
        #[arg(long)]
        pub providers: Option<String>,
        /// Sort key (apy_total|tvl_usd|liquidity_usd).
        #[arg(long, default_value = "apy_total")]
        pub sort: String,
        /// Minimum total APY percent.
        #[arg(long = "min-apy")]
        pub min_apy: Option<f64>,
        /// Minimum TVL in USD.
        #[arg(long = "min-tvl-usd")]
        pub min_tvl_usd: Option<f64>,
        /// Include opportunities missing APY/TVL.
        #[arg(long = "include-incomplete")]
        pub include_incomplete: bool,
        /// Maximum opportunities to return.
        #[arg(long, default_value_t = 20)]
        pub limit: i64,
        /// Optional RPC URL override for on-chain providers.
        #[arg(long = "rpc-url")]
        pub rpc_url: Option<String>,
    }

    /// `yield positions` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct PositionsArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Position owner address.
        #[arg(long)]
        pub address: Option<String>,
        /// Optional asset filter (symbol/address/CAIP-19).
        #[arg(long)]
        pub asset: Option<String>,
        /// Filter by provider names (aave,morpho,kamino,moonwell).
        #[arg(long)]
        pub providers: Option<String>,
        /// Maximum positions to return.
        #[arg(long, default_value_t = 20)]
        pub limit: i64,
        /// Optional RPC URL override used by providers that need on-chain valuation.
        #[arg(long = "rpc-url")]
        pub rpc_url: Option<String>,
    }

    /// `yield history` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct HistoryArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Asset symbol/address/CAIP-19.
        #[arg(long)]
        pub asset: Option<String>,
        /// Filter by provider names (aave,morpho,kamino).
        #[arg(long)]
        pub providers: Option<String>,
        /// Optional comma-separated opportunity IDs from yield opportunities.
        #[arg(long = "opportunity-ids")]
        pub opportunity_ids: Option<String>,
        /// History metrics (apy_total,tvl_usd).
        #[arg(long, default_value = "apy_total")]
        pub metrics: String,
        /// Lookback window (for example 24h,7d,30d).
        #[arg(long, default_value = "7d")]
        pub window: String,
        /// Point interval (hour|day).
        #[arg(long, default_value = "day")]
        pub interval: String,
        /// Start time (RFC3339). Overrides --window when set.
        #[arg(long)]
        pub from: Option<String>,
        /// End time (RFC3339). Defaults to now.
        #[arg(long)]
        pub to: Option<String>,
        /// Maximum opportunities per provider to fetch history for.
        #[arg(long, default_value_t = 20)]
        pub limit: i64,
    }

    /// The `plan` / `submit` / `status` sub-subcommands shared by both yield verbs.
    #[derive(Subcommand, Debug)]
    pub enum YieldVerbCmd {
        /// Create and persist a yield action plan.
        Plan(YieldPlanArgs),
        /// Execute an existing yield action.
        Submit(SubmitArgs),
        /// Get yield action status.
        Status(StatusArgs),
    }

    impl YieldVerbCmd {
        /// The leaf path token (`plan`/`submit`/`status`).
        pub fn path(&self) -> &'static str {
            match self {
                YieldVerbCmd::Plan(_) => "plan",
                YieldVerbCmd::Submit(_) => "submit",
                YieldVerbCmd::Status(_) => "status",
            }
        }
    }

    /// `yield <verb> plan` flags (shared across deposit/withdraw).
    #[derive(Args, Debug, Clone, Default)]
    pub struct YieldPlanArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Asset symbol/address/CAIP-19.
        #[arg(long)]
        pub asset: Option<String>,
        /// Amount in base units.
        #[arg(long)]
        pub amount: Option<String>,
        /// Amount in decimal units.
        #[arg(long = "amount-decimal")]
        pub amount_decimal: Option<String>,
        /// Yield provider (aave|morpho|moonwell).
        #[arg(long)]
        pub provider: Option<String>,
        /// Recipient address (defaults to the resolved sender address).
        #[arg(long)]
        pub recipient: Option<String>,
        /// Position owner address (defaults to the resolved sender address).
        #[arg(long = "on-behalf-of")]
        pub on_behalf_of: Option<String>,
        /// Morpho vault address (required for --provider morpho).
        #[arg(long = "vault-address")]
        pub vault_address: Option<String>,
        /// Aave pool address override.
        #[arg(long = "pool-address")]
        pub pool_address: Option<String>,
        /// Aave pool address provider override.
        #[arg(long = "pool-address-provider")]
        pub pool_address_provider: Option<String>,
        /// RPC URL override for the selected chain.
        #[arg(long = "rpc-url")]
        pub rpc_url: Option<String>,
        /// Include simulation checks during execution.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        pub simulate: bool,
        #[command(flatten)]
        pub identity: PlanIdentityFlags,
        #[command(flatten)]
        pub input: crate::execflags::InputFlags,
    }

    /// Handle `yield <sub>`.
    ///
    /// Reads (`opportunities`/`positions`/`history`) are WS2 (wired here);
    /// execution verbs are WS3 (`plan`) / WS4 (`submit`/`status`). All route
    /// here; unimplemented leaves return a typed `Unsupported` error (never
    /// `unknown command`).
    pub async fn handle(ctx: &AppCtx, cmd: YieldCmd) -> Result<Envelope, Error> {
        match cmd {
            YieldCmd::Opportunities(args) => handle_opportunities(ctx, args).await,
            YieldCmd::Positions(args) => handle_positions(ctx, args).await,
            YieldCmd::History(args) => handle_history(ctx, args).await,
            YieldCmd::Deposit(YieldVerbCmd::Plan(args)) => {
                handle_plan(ctx, YieldVerb::Deposit, args).await
            }
            YieldCmd::Withdraw(YieldVerbCmd::Plan(args)) => {
                handle_plan(ctx, YieldVerb::Withdraw, args).await
            }
            other => {
                let path = format!("yield {}", other.path());
                let ws = if path.ends_with("plan") { "WS3" } else { "WS4" };
                Err(AppCtx::unimplemented(&path, ws))
            }
        }
    }

    /// Handle `yield <verb> plan` (Go `planCmd.RunE` in
    /// `yield_execution_commands.go`), shared across deposit/withdraw.
    ///
    /// Flow parity with the Go runner (identical in shape to the lend handler,
    /// differing only in the routing request fields + the status-name fallback):
    /// 1. resolve the execution identity (OWS `--wallet` first / legacy
    ///    `--from-address`) on the requested chain; an identity error returns the
    ///    typed [`Error`] before anything is persisted;
    /// 2. parse `--chain` + `--asset`, default a non-positive asset `decimals` to
    ///    18, and normalize the amount against those decimals (carrying base +
    ///    decimal forms consistently, spec §2.4);
    /// 3. route the build by `--provider` through the action-build registry
    ///    ([`Registry::build_yield_action`] → the Aave/Morpho/Moonwell planner),
    ///    capturing one provider status keyed on the normalized lending provider
    ///    name (fallback `"yield"` when empty; Go `statusFromErr`);
    /// 4. stamp the resolved identity (wallet id/name, from-address, execution
    ///    backend) onto the action and persist it to the action [`Store`];
    /// 5. emit the success envelope with the identity warnings, the cache
    ///    bypassed (execution paths skip the cache, spec §2.5), and the yield
    ///    provider status.
    ///
    /// [`Store`]: defi_execution::store::Store
    async fn handle_plan(
        ctx: &AppCtx,
        verb: YieldVerb,
        args: YieldPlanArgs,
    ) -> Result<Envelope, Error> {
        let chain_arg = args.chain.as_deref().unwrap_or_default();
        let wallet_ref = args.identity.wallet.as_deref().unwrap_or_default();
        let from_flag = args.identity.from_address.as_deref().unwrap_or_default();

        // 1. Resolve the execution identity (returns before any persistence on
        //    error — both / neither input, malformed address, Tempo/non-EVM
        //    --wallet, OWS resolve failures).
        let identity = resolve_execution_identity(wallet_ref, from_flag, chain_arg)?;

        // The provider status name is keyed on the normalized lending provider
        // (Go `normalizeLendingProvider(plan.Provider)`); fall back to "yield"
        // when empty so a missing/unknown provider still reports one status row.
        let provider_name =
            crate::runner::normalize_lending_provider(args.provider.as_deref().unwrap_or_default());
        let status_name = if provider_name.is_empty() {
            "yield".to_string()
        } else {
            provider_name
        };

        // 2 & 3. Build + route the yield action; capture the provider status.
        let action = build_plan_action(verb, &args, &identity.from_address).await;
        let status = ProviderStatus {
            name: status_name,
            status: super::status_from_result(&action),
            latency_ms: 0,
        };
        let mut action = action?;

        // 4. Stamp the identity + persist (status already captured ok above).
        apply_execution_identity_to_action(&mut action, &identity);
        let store = ctx.open_action_store()?;
        store
            .save(&action)
            .map_err(|e| Error::wrap(Code::Internal, "persist planned action", e))?;

        // 5. Emit the success envelope (cache bypassed for execution paths).
        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize planned action", e))?;
        let path = format!("yield {} plan", verb_path(verb));
        let mut env = ctx.metadata_envelope(&path, data, vec![status]);
        env.warnings = identity.warnings;
        Ok(env)
    }

    /// Build the yield [`Action`] for a `plan` request (Go `buildAction`
    /// closure): parse chain/asset, default decimals to 18, normalize the amount,
    /// then route the [`YieldRequest`] by provider through the registry.
    ///
    /// [`Action`]: defi_execution::action::Action
    async fn build_plan_action(
        verb: YieldVerb,
        args: &YieldPlanArgs,
        sender: &str,
    ) -> Result<defi_execution::action::Action, Error> {
        let chain_arg = args.chain.as_deref().unwrap_or_default();
        let asset_arg = args.asset.as_deref().unwrap_or_default();
        let (chain, asset) = crate::lend::parse_chain_asset(chain_arg, asset_arg)?;

        // Default a non-positive asset `decimals` to 18 (Go `buildAction`).
        let mut decimals = asset.decimals;
        if decimals <= 0 {
            decimals = 18;
        }
        let (base, _) = normalize_amount(
            args.amount.as_deref().unwrap_or_default(),
            args.amount_decimal.as_deref().unwrap_or_default(),
            decimals,
        )?;

        Registry::new()
            .build_yield_action(YieldRequest {
                provider: args.provider.clone().unwrap_or_default(),
                verb,
                chain,
                asset,
                vault_address: args.vault_address.clone().unwrap_or_default(),
                amount_base_units: base,
                sender: sender.to_string(),
                recipient: args.recipient.clone().unwrap_or_default(),
                on_behalf_of: args.on_behalf_of.clone().unwrap_or_default(),
                simulate: args.simulate,
                rpc_url: args.rpc_url.clone().unwrap_or_default(),
                pool_address: args.pool_address.clone().unwrap_or_default(),
                pool_address_provider: args.pool_address_provider.clone().unwrap_or_default(),
            })
            .await
    }

    /// The leaf verb token for `meta.command` (`deposit`/`withdraw`).
    fn verb_path(verb: YieldVerb) -> &'static str {
        match verb {
            YieldVerb::Deposit => "deposit",
            YieldVerb::Withdraw => "withdraw",
        }
    }

    /// Cache-key request payload for `yield opportunities`.
    ///
    /// Field declaration order is ALPHABETICAL so the serde JSON matches the Go
    /// `map[string]any` payload (Go `json.Marshal` of a map sorts keys), keeping
    /// cache keys cross-binary stable.
    #[derive(serde::Serialize)]
    struct OpportunitiesCacheReq {
        asset: String,
        chain: String,
        include_incomplete: bool,
        limit: i64,
        min_apy: f64,
        min_tvl_usd: f64,
        providers: Vec<String>,
        rpc_url: String,
        sort: String,
    }

    /// Cache-key request payload for `yield positions` (alphabetical order).
    #[derive(serde::Serialize)]
    struct PositionsCacheReq {
        address: String,
        asset: String,
        chain: String,
        limit: i64,
        providers: Vec<String>,
        rpc_url: String,
    }

    /// Cache-key request payload for `yield history` (alphabetical order).
    #[derive(serde::Serialize)]
    struct HistoryCacheReq {
        asset: String,
        chain: String,
        end_time: String,
        interval: String,
        metrics: Vec<String>,
        opportunity_ids: Vec<String>,
        opportunity_limit: i64,
        providers: Vec<String>,
        start_time: String,
    }

    /// Handle `yield opportunities`: required `--chain`/`--asset` → cache flow.
    async fn handle_opportunities(
        ctx: &AppCtx,
        args: OpportunitiesArgs,
    ) -> Result<Envelope, Error> {
        let path = "yield opportunities";
        let chain_arg = args.chain.clone().unwrap_or_default();
        let asset_arg = args.asset.clone().unwrap_or_default();
        let (chain, asset) = crate::lend::parse_chain_asset(&chain_arg, &asset_arg)?;
        let rpc_url = args.rpc_url.clone().unwrap_or_default();
        let provider_filter = crate::runner::split_csv(&args.providers.clone().unwrap_or_default());

        let req = super::YieldRequest {
            chain: chain.clone(),
            asset: asset.clone(),
            limit: args.limit,
            min_tvl_usd: args.min_tvl_usd.unwrap_or(0.0),
            min_apy: args.min_apy.unwrap_or(0.0),
            providers: provider_filter.clone(),
            sort_by: args.sort.clone(),
            include_incomplete: args.include_incomplete,
        };
        let cache_req = OpportunitiesCacheReq {
            asset: asset.asset_id.clone(),
            chain: chain.caip2.clone(),
            include_incomplete: args.include_incomplete,
            limit: args.limit,
            min_apy: req.min_apy,
            min_tvl_usd: req.min_tvl_usd,
            providers: provider_filter.clone(),
            rpc_url: rpc_url.trim().to_string(),
            sort: args.sort.clone(),
        };
        let key = crate::protocols::cache_key(path, &cache_req);
        let ttl = std::time::Duration::from_secs(super::YIELD_OPPORTUNITIES_TTL_SECS);
        ctx.run_cached_command(path, &key, ttl, || {
            crate::ctx::block_on_fetch(super::run_opportunities(ctx, &req, &chain, &rpc_url))
        })
    }

    /// Handle `yield positions`: input validation (chain/address) → cache flow.
    async fn handle_positions(ctx: &AppCtx, args: PositionsArgs) -> Result<Envelope, Error> {
        let path = "yield positions";
        let chain_arg = args.chain.clone().unwrap_or_default();
        let address = args.address.clone().unwrap_or_default();
        let asset_arg = args.asset.clone().unwrap_or_default();

        let validated = super::validate_yield_positions_input(&chain_arg, &address)?;
        let chain = validated.chain;
        let account = validated.account;

        let asset = crate::lend::parse_optional_chain_asset(&chain, &asset_arg)?;
        let rpc_url = args.rpc_url.clone().unwrap_or_default();
        let provider_filter = crate::runner::split_csv(&args.providers.clone().unwrap_or_default());

        let cache_account = if chain.is_evm() {
            account.to_ascii_lowercase()
        } else {
            account.clone()
        };
        let cache_req = PositionsCacheReq {
            address: cache_account,
            asset: crate::lend::chain_asset_filter_cache_value(&asset, &asset_arg),
            chain: chain.caip2.clone(),
            limit: args.limit,
            providers: provider_filter.clone(),
            rpc_url: rpc_url.trim().to_string(),
        };
        let key = crate::protocols::cache_key(path, &cache_req);
        let ttl = std::time::Duration::from_secs(super::YIELD_POSITIONS_TTL_SECS);
        ctx.run_cached_command(path, &key, ttl, || {
            crate::ctx::block_on_fetch(super::run_positions(
                ctx,
                &chain,
                &account,
                &asset,
                &provider_filter,
                args.limit,
                &rpc_url,
            ))
        })
    }

    /// Handle `yield history`: required `--chain`/`--asset`, metric/interval/
    /// range parsing → cache flow.
    async fn handle_history(ctx: &AppCtx, args: HistoryArgs) -> Result<Envelope, Error> {
        let path = "yield history";
        let chain_arg = args.chain.clone().unwrap_or_default();
        let asset_arg = args.asset.clone().unwrap_or_default();
        let (chain, asset) = crate::lend::parse_chain_asset(&chain_arg, &asset_arg)?;

        let metrics = super::parse_yield_history_metrics(&args.metrics)?;
        let interval = super::parse_yield_history_interval(&args.interval)?;
        let (start_time, end_time) = super::resolve_yield_history_range(
            args.from.as_deref().unwrap_or_default(),
            args.to.as_deref().unwrap_or_default(),
            &args.window,
            ctx.now(),
        )?;
        let opportunity_ids =
            crate::runner::split_csv(&args.opportunity_ids.clone().unwrap_or_default());
        let provider_filter = crate::runner::split_csv(&args.providers.clone().unwrap_or_default());

        let cache_req = HistoryCacheReq {
            asset: asset.asset_id.clone(),
            chain: chain.caip2.clone(),
            end_time: end_time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            interval: interval.as_str().to_string(),
            metrics: metrics.iter().map(|m| m.as_str().to_string()).collect(),
            opportunity_ids: opportunity_ids.clone(),
            opportunity_limit: args.limit,
            providers: provider_filter.clone(),
            start_time: start_time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        };
        let key = crate::protocols::cache_key(path, &cache_req);
        let ttl = std::time::Duration::from_secs(super::YIELD_HISTORY_TTL_SECS);
        ctx.run_cached_command(path, &key, ttl, || {
            crate::ctx::block_on_fetch(super::run_history(
                ctx,
                &chain,
                &asset,
                &provider_filter,
                &metrics,
                interval,
                start_time,
                end_time,
                &opportunity_ids,
                args.limit,
            ))
        })
    }
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

#[cfg(test)]
mod app_tests {
    //! # Success criteria — app-level `yield {opportunities,positions,history}`
    //! (WS2, read; Go: `internal/app` `newYieldCommand` in `runner.go`)
    //!
    //! These tests exercise the **wired command-group handler**
    //! ([`cli::handle`]) end-to-end, asserting the full machine contract the
    //! handler is responsible for — NOT the provider's ranking/normalization
    //! (owned/tested by `defi-providers::{aave,morpho,moonwell}`) nor the
    //! cache-flow state machine internals (owned/tested by `defi-app::runner`),
    //! nor the pure ranking/parsing helpers (asserted in this module's sibling
    //! `tests`). These tests are RED until WS2 wires the `yield` handler:
    //! `cli::handle` currently returns the typed `unimplemented` stub
    //! ([`Code::Unsupported`] with `"not yet implemented"`), so every assertion
    //! that expects a real envelope / a Go-semantic error fails.
    //!
    //! ## Provider seam (offline determinism)
    //!
    //! `yield`'s read providers are the SAME set as `lend` (aave/morpho via
    //! GraphQL, moonwell via on-chain RPC). The only one injectable through the
    //! already-present `--rpc-url` flag with no `AppCtx` change is **Moonwell**
    //! (on-chain reads on Base `eip155:8453`, the chain `yield_provider_supports_chain`
    //! whitelists). Success-path envelopes are therefore asserted via Moonwell on
    //! Base, reusing the same JSON-RPC multicall mock the provider crate + the
    //! `lend` app tests use. Aave/Morpho GraphQL success envelopes have no
    //! app-level base-URL seam yet (deferred to a later GREEN seam + the WS5
    //! sweep), exactly as documented for `lend`.
    //!
    //! ## Success path (wiremock, Moonwell via `--rpc-url`)
    //!
    //! Y-A1. **`yield opportunities` success envelope.** `yield opportunities
    //!       --chain base --asset USDC --providers moonwell --rpc-url <mock>`
    //!       resolves a success [`Envelope`]: `version="v1"`, `success=true`,
    //!       `error=None`, `meta.command="yield opportunities"`, `data` is a
    //!       non-empty array of `YieldOpportunity` whose `provider == protocol ==
    //!       "moonwell"`, `apy_total` is a percentage point (spec §2.5: positive,
    //!       not a sub-1 ratio), and `partial=false`. (Go `opportunitiesCmd`
    //!       success path.)
    //! Y-A2. **`yield opportunities` reports the provider status.**
    //!       `meta.providers` contains exactly one entry `{name:"moonwell",
    //!       status:"ok"}` (Go appends one `ProviderStatus` per selected provider
    //!       with `statusFromErr(nil)=="ok"`).
    //! Y-A3. **`yield opportunities` cache transition.** With caching ENABLED the
    //!       first invocation writes the cache (`meta.cache.status=="write"`,
    //!       `stale=false`); a SECOND identical invocation serves the cache
    //!       WITHOUT a second provider call (`meta.cache.status=="hit"`,
    //!       `stale=false`, `meta.providers` empty). With caching DISABLED the
    //!       status is `"miss"`. (`yield opportunities` is a data route, NOT
    //!       bypassed — `should_open_cache` is true; TTL is 60s in Go.)
    //! Y-A4. **`yield opportunities --limit` truncates the envelope payload.** The
    //!       `data` array length is `min(combined_rows, limit)` (Go
    //!       `combined[:req.Limit]`); `--limit 1` keeps at most one row.
    //! Y-A5. **`yield opportunities --min-tvl-usd` is threaded to the provider.**
    //!       An impossibly high `--min-tvl-usd` filters out every Moonwell market,
    //!       so the provider returns nothing and the command surfaces the
    //!       Go-semantic `Code::Unavailable` (no opportunities) rather than a
    //!       success envelope — proving the flag reaches the provider request.
    //! Y-A6. **`yield positions` success envelope.** `yield positions --chain base
    //!       --address <dead> --providers moonwell --rpc-url <mock>` → success
    //!       envelope, `meta.command="yield positions"`, a non-empty
    //!       `YieldPosition` array (`provider=="moonwell"`), one
    //!       `{name:"moonwell",status:"ok"}` provider status. (Go `positionsCmd`
    //!       success path; TTL 30s.)
    //!
    //! ## Error paths (Go-semantic)
    //!
    //! Y-E1. **`yield positions` requires `--address`.** `yield positions --chain
    //!       1` (no address) → exit 2 (usage). (Go `MarkFlagRequired("address")`
    //!       / in-handler `--address is required`.)
    //! Y-E2. **`yield positions` invalid EVM address** → exit 2 (usage). (Go
    //!       `--address must be a valid EVM hex address`.)
    //! Y-E3. **`yield opportunities` requires `--chain`/`--asset`.** Omitting the
    //!       required `--asset` → exit 2 (usage). (Go `MarkFlagRequired`.)
    //! Y-E4. **`yield history` requires `--chain`/`--asset`.** Omitting `--asset`
    //!       → exit 2 (usage). (Go `MarkFlagRequired`.)
    //! Y-E5. **Unknown `--providers` is a usage error.** `yield opportunities
    //!       --chain 1 --asset USDC --providers bogus` → exit 2 (usage), matching
    //!       Go `selectYieldProviders` (`unsupported yield provider`).
    //! Y-E6. **`yield positions --providers kamino` (EVM chain) is unsupported.**
    //!       Kamino is not selected on an EVM chain, BUT an explicit
    //!       `--providers kamino` is validated against the registered set and then
    //!       gated: Kamino implements `YieldProvider` but NOT
    //!       `YieldPositionsProvider`, so the command surfaces the capability gate
    //!       (`Code::Unsupported`, `"does not support positions"`), NOT the WS2
    //!       placeholder stub. (Go `provider.(providers.YieldPositionsProvider)`
    //!       assertion.)
    //! Y-E7. **`yield history --providers moonwell` is unsupported.** Moonwell
    //!       implements `YieldProvider` + positions but NOT
    //!       `YieldHistoryProvider`, so `yield history --chain base --asset USDC
    //!       --providers moonwell` surfaces `Code::Unsupported` (exit 13) with
    //!       `"does not support history"`. (Go
    //!       `provider.(providers.YieldHistoryProvider)` assertion; ported from
    //!       `TestYieldHistoryCommandFailsWhenProviderHasNoHistorySupport`.)
    //! Y-E8. **`yield history` rejects invalid `--metrics`/`--interval`.** A bogus
    //!       `--metrics`/`--interval` value is a usage error (exit 2) BEFORE any
    //!       provider call. (Go `parseYieldHistoryMetrics` /
    //!       `parseYieldHistoryInterval`.)
    //! Y-E9. **`yield history` rejects a future `--to`.** A `--to` more than 5m in
    //!       the future is a usage error (exit 2). (Go `resolveYieldHistoryRange`.)
    //!
    //! ## Flag parsing
    //!
    //! Y-F1. **Defaults parse.** `yield opportunities --chain 1 --asset USDC`
    //!       parses with `limit==20`, `sort=="apy_total"`,
    //!       `include_incomplete==false`. `yield history` defaults
    //!       `metrics=="apy_total"`, `interval=="day"`, `window=="7d"`,
    //!       `limit==20`. `yield positions` defaults `limit==20`.
    //! Y-F2. **`--providers` (multi) + `--min-tvl-usd` + `--rpc-url` parse and are
    //!       forwarded.** `yield opportunities ... --providers aave,morpho
    //!       --min-tvl-usd 1000000 --rpc-url http://x` parses into the typed args.
    //!
    //! SKIPPED here (covered elsewhere): per-row field/format byte parity
    //! (provider goldens + WS5 sweep), Aave/Morpho GraphQL success envelopes (no
    //! app-level base-URL seam yet), and the exact cobra-vs-clap required-flag
    //! phrasing (asserted at the exit-code level only).

    use super::cli::{handle, HistoryArgs, OpportunitiesArgs, PositionsArgs, YieldCmd};
    use crate::cli::run_with_args;
    use crate::ctx::AppCtx;
    use defi_config::{MapEnv, Settings};
    use defi_errors::Code;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use alloy::dyn_abi::DynSolValue;
    use alloy::json_abi::JsonAbi;
    use alloy::primitives::{Address as AlloyAddress, U256};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    // ---- canonical Moonwell-on-Base test addresses (mirror the provider mock) -
    const TEST_COMPTROLLER: &str = "0xfBb21d0380beE3312B33c4353c8936a0F13EF26C";
    const TEST_ORACLE: &str = "0xEC942bE8A8114bFD0396A5052c36027f2cA6a9d0";
    const TEST_MTOKEN_USDC: &str = "0xEdc817A28E8B93B03976FBd4a3dDBc9f7D176c22";
    const TEST_USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    const DEAD: &str = "0x000000000000000000000000000000000000dEaD";
    const MULTICALL3_ADDR: &str = "0xca11bde05977b3631167028862be2a173976ca11";

    // ---- settings + env helpers ------------------------------------------

    /// JSON-output settings with caching toggled per `cache_enabled`. Cache /
    /// action store paths live in the supplied temp dir so a cache-enabled
    /// variant can open sqlite without touching the real home.
    fn settings_in(tmp: &std::path::Path, cache_enabled: bool) -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: false,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_secs(5),
            retries: 0,
            max_stale: Duration::from_secs(0),
            no_stale: false,
            cache_enabled,
            cache_path: tmp.join("cache.sqlite"),
            cache_lock_path: tmp.join("cache.lock"),
            action_store_path: tmp.join("actions.sqlite"),
            action_lock_path: tmp.join("actions.lock"),
            defillama_api_key: String::new(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    /// A `MapEnv` whose HOME points at a temp dir so `Settings::load` resolves
    /// cache/config paths without touching the real home.
    fn env_with_home() -> (MapEnv, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let env = MapEnv::with_home(tmp.path().to_path_buf());
        (env, tmp)
    }

    fn opportunities_args(rpc: &str) -> OpportunitiesArgs {
        OpportunitiesArgs {
            chain: Some("base".to_string()),
            asset: Some("USDC".to_string()),
            providers: Some("moonwell".to_string()),
            sort: "apy_total".to_string(),
            min_apy: None,
            min_tvl_usd: None,
            include_incomplete: false,
            limit: 20,
            rpc_url: Some(rpc.to_string()),
        }
    }

    fn positions_args(rpc: &str) -> PositionsArgs {
        PositionsArgs {
            chain: Some("base".to_string()),
            address: Some(DEAD.to_string()),
            asset: None,
            providers: Some("moonwell".to_string()),
            limit: 20,
            rpc_url: Some(rpc.to_string()),
        }
    }

    fn history_args() -> HistoryArgs {
        HistoryArgs {
            chain: Some("base".to_string()),
            asset: Some("USDC".to_string()),
            providers: Some("moonwell".to_string()),
            opportunity_ids: None,
            metrics: "apy_total".to_string(),
            window: "7d".to_string(),
            interval: "day".to_string(),
            from: None,
            to: None,
            limit: 20,
        }
    }

    fn data_array(env: &defi_model::Envelope) -> Vec<Value> {
        env.data
            .as_ref()
            .and_then(Value::as_array)
            .cloned()
            .expect("data is an array")
    }

    // ---- Moonwell JSON-RPC multicall mock (ported from the lend app tests) -

    fn addr(s: &str) -> AlloyAddress {
        s.parse().expect("valid test address")
    }

    fn selector_for(abi_json: &str, name: &str) -> String {
        let abi: JsonAbi = serde_json::from_str(abi_json).expect("parse abi");
        let f = abi
            .function(name)
            .and_then(|o| o.first())
            .cloned()
            .expect("function present");
        hex::encode(f.selector().0)
    }

    fn encode_output(values: &[DynSolValue]) -> Vec<u8> {
        DynSolValue::Tuple(values.to_vec()).abi_encode_params()
    }

    fn aggregate3_json() -> alloy::json_abi::Function {
        let abi: JsonAbi = serde_json::from_str(defi_registry::MULTICALL3_ABI).expect("parse mc3");
        abi.function("aggregate3")
            .and_then(|o| o.first())
            .cloned()
            .expect("aggregate3 present")
    }

    fn lower_hex(a: &AlloyAddress) -> String {
        format!("0x{}", hex::encode(a.as_slice()))
    }

    /// Per-call dispatcher resolving `(target, selector)` to an ABI return blob,
    /// mirroring the provider-crate + lend-app Moonwell mock fixtures one-to-one.
    struct Dispatcher {
        get_all_markets_sel: String,
        oracle_sel: String,
        get_assets_in_sel: String,
        m_underlying_sel: String,
        m_supply_rate_sel: String,
        m_borrow_rate_sel: String,
        m_total_supply_sel: String,
        m_exchange_rate_sel: String,
        m_total_borrows_sel: String,
        m_get_cash_sel: String,
        m_snapshot_sel: String,
        e_symbol_sel: String,
        e_decimals_sel: String,
        o_price_sel: String,
        supply_rate: U256,
        borrow_rate: U256,
        total_supply: U256,
        exchange_rate: U256,
        total_borrows: U256,
        cash: U256,
        price: U256,
        m_token_bal: U256,
        borrow_bal: U256,
    }

    impl Dispatcher {
        fn new() -> Self {
            let pow = |base: u128, exp: u32| U256::from(base).pow(U256::from(exp));
            let comptroller_abi = defi_registry::MOONWELL_COMPTROLLER_ABI;
            let mtoken_abi = defi_registry::MOONWELL_MTOKEN_ABI;
            let erc20_abi = defi_registry::MOONWELL_ERC20_MINIMAL_ABI;
            let oracle_abi = defi_registry::MOONWELL_ORACLE_ABI;
            Dispatcher {
                get_all_markets_sel: selector_for(comptroller_abi, "getAllMarkets"),
                oracle_sel: selector_for(comptroller_abi, "oracle"),
                get_assets_in_sel: selector_for(comptroller_abi, "getAssetsIn"),
                m_underlying_sel: selector_for(mtoken_abi, "underlying"),
                m_supply_rate_sel: selector_for(mtoken_abi, "supplyRatePerTimestamp"),
                m_borrow_rate_sel: selector_for(mtoken_abi, "borrowRatePerTimestamp"),
                m_total_supply_sel: selector_for(mtoken_abi, "totalSupply"),
                m_exchange_rate_sel: selector_for(mtoken_abi, "exchangeRateCurrent"),
                m_total_borrows_sel: selector_for(mtoken_abi, "totalBorrowsCurrent"),
                m_get_cash_sel: selector_for(mtoken_abi, "getCash"),
                m_snapshot_sel: selector_for(mtoken_abi, "getAccountSnapshot"),
                e_symbol_sel: selector_for(erc20_abi, "symbol"),
                e_decimals_sel: selector_for(erc20_abi, "decimals"),
                o_price_sel: selector_for(oracle_abi, "getUnderlyingPrice"),
                supply_rate: U256::from(951293759u64),
                borrow_rate: U256::from(1585489599u64),
                total_supply: U256::from(100_000_000u128) * pow(10, 8),
                exchange_rate: U256::from(2u128) * pow(10, 14),
                total_borrows: U256::from(500_000u128) * pow(10, 6),
                cash: U256::from(500_000u128) * pow(10, 6),
                price: pow(10, 30),
                m_token_bal: U256::from(10_000u128) * pow(10, 8),
                borrow_bal: U256::from(1_000u128) * pow(10, 6),
            }
        }

        fn dispatch(&self, to: &str, data_hex: &str) -> Option<Vec<u8>> {
            let selector = data_hex.get(..8).unwrap_or("");
            let to = to.to_ascii_lowercase();

            if to == TEST_COMPTROLLER.to_ascii_lowercase() {
                if selector == self.get_all_markets_sel {
                    return Some(encode_output(&[DynSolValue::Array(vec![
                        DynSolValue::Address(addr(TEST_MTOKEN_USDC)),
                    ])]));
                }
                if selector == self.oracle_sel {
                    return Some(encode_output(&[DynSolValue::Address(addr(TEST_ORACLE))]));
                }
                if selector == self.get_assets_in_sel {
                    return Some(encode_output(&[DynSolValue::Array(vec![
                        DynSolValue::Address(addr(TEST_MTOKEN_USDC)),
                    ])]));
                }
            } else if to == TEST_ORACLE.to_ascii_lowercase() {
                if selector == self.o_price_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.price, 256)]));
                }
            } else if to == TEST_MTOKEN_USDC.to_ascii_lowercase() {
                if selector == self.m_underlying_sel {
                    return Some(encode_output(&[DynSolValue::Address(addr(TEST_USDC))]));
                }
                if selector == self.m_supply_rate_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.supply_rate, 256)]));
                }
                if selector == self.m_borrow_rate_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.borrow_rate, 256)]));
                }
                if selector == self.m_total_supply_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.total_supply, 256)]));
                }
                if selector == self.m_exchange_rate_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.exchange_rate, 256)]));
                }
                if selector == self.m_total_borrows_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.total_borrows, 256)]));
                }
                if selector == self.m_get_cash_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.cash, 256)]));
                }
                if selector == self.m_snapshot_sel {
                    return Some(encode_output(&[
                        DynSolValue::Uint(U256::ZERO, 256),
                        DynSolValue::Uint(self.m_token_bal, 256),
                        DynSolValue::Uint(self.borrow_bal, 256),
                        DynSolValue::Uint(self.exchange_rate, 256),
                    ]));
                }
            } else if to == TEST_USDC.to_ascii_lowercase() {
                if selector == self.e_symbol_sel {
                    return Some(encode_output(&[DynSolValue::String("USDC".to_string())]));
                }
                if selector == self.e_decimals_sel {
                    return Some(encode_output(&[DynSolValue::Uint(U256::from(6u8), 8)]));
                }
            }
            None
        }
    }

    struct RpcResponder {
        dispatcher: Arc<Dispatcher>,
    }

    impl Respond for RpcResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = match serde_json::from_slice(&request.body) {
                Ok(v) => v,
                Err(_) => return ResponseTemplate::new(400),
            };
            let id = body.get("id").cloned().unwrap_or(json!(1));
            let method_name = body.get("method").and_then(Value::as_str).unwrap_or("");
            if method_name != "eth_call" {
                return ok_response(&id, "0x");
            }
            let params = match body.get("params").and_then(|p| p.get(0)) {
                Some(p) => p,
                None => return ok_response(&id, "0x"),
            };
            let to = params
                .get("to")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let data_hex = params
                .get("data")
                .or_else(|| params.get("input"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim_start_matches("0x")
                .to_string();
            let selector = data_hex.get(..8).unwrap_or("");

            let mc3_sel = selector_for(defi_registry::MULTICALL3_ABI, "aggregate3");
            if to.to_ascii_lowercase() == MULTICALL3_ADDR && selector == mc3_sel {
                let result = self.handle_aggregate3(&data_hex);
                return ok_response(&id, &result);
            }

            let result = match self.dispatcher.dispatch(&to, &data_hex) {
                Some(bytes) => format!("0x{}", hex::encode(bytes)),
                None => "0x".to_string(),
            };
            ok_response(&id, &result)
        }
    }

    impl RpcResponder {
        fn handle_aggregate3(&self, data_hex: &str) -> String {
            use alloy::dyn_abi::{FunctionExt, JsonAbiExt};
            let raw = match hex::decode(data_hex) {
                Ok(b) => b,
                Err(_) => return "0x".to_string(),
            };
            if raw.len() < 4 {
                return "0x".to_string();
            }
            let agg = aggregate3_json();
            let decoded = match agg.abi_decode_input(&raw[4..]) {
                Ok(v) => v,
                Err(_) => return "0x".to_string(),
            };
            let calls = match decoded.first().and_then(|v| v.as_array()) {
                Some(c) => c,
                None => return "0x".to_string(),
            };

            let mut results: Vec<DynSolValue> = Vec::with_capacity(calls.len());
            for call in calls {
                let tuple = match call.as_tuple() {
                    Some(t) if t.len() == 3 => t,
                    _ => {
                        results.push(failed_result());
                        continue;
                    }
                };
                let target = tuple[0]
                    .as_address()
                    .map(|a| lower_hex(&a))
                    .unwrap_or_default();
                let sub_data = tuple[2].as_bytes().map(hex::encode).unwrap_or_default();
                match self.dispatcher.dispatch(&target, &sub_data) {
                    Some(bytes) => results.push(DynSolValue::Tuple(vec![
                        DynSolValue::Bool(true),
                        DynSolValue::Bytes(bytes),
                    ])),
                    None => results.push(failed_result()),
                }
            }

            match agg.abi_encode_output(&[DynSolValue::Array(results)]) {
                Ok(bytes) => format!("0x{}", hex::encode(bytes)),
                Err(_) => "0x".to_string(),
            }
        }
    }

    fn failed_result() -> DynSolValue {
        DynSolValue::Tuple(vec![
            DynSolValue::Bool(false),
            DynSolValue::Bytes(Vec::new()),
        ])
    }

    fn ok_response(id: &Value, result: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    async fn moonwell_rpc_server() -> MockServer {
        let server = MockServer::start().await;
        let responder = RpcResponder {
            dispatcher: Arc::new(Dispatcher::new()),
        };
        Mock::given(method("POST"))
            .respond_with(responder)
            .mount(&server)
            .await;
        server
    }

    // ---- Y-A1 / Y-A2: opportunities success envelope + provider status ----

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_opportunities_success_envelope_and_provider_status() {
        let server = moonwell_rpc_server().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        let env = handle(
            &ctx,
            YieldCmd::Opportunities(opportunities_args(&server.uri())),
        )
        .await
        .expect("yield opportunities should succeed against the mock RPC");

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert_eq!(env.meta.command, "yield opportunities");
        assert!(!env.meta.partial);

        let rows = data_array(&env);
        assert!(!rows.is_empty(), "expected at least one opportunity");
        assert_eq!(rows[0]["provider"], json!("moonwell"));
        assert_eq!(rows[0]["protocol"], json!("moonwell"));
        // APY = percentage points (spec §2.5): positive, not a sub-1 ratio.
        let apy = rows[0]["apy_total"].as_f64().expect("apy_total f64");
        assert!(apy > 0.0, "apy_total should be positive: {apy}");

        // Y-A2: one provider status, status "ok".
        assert_eq!(env.meta.providers.len(), 1, "exactly one provider status");
        assert_eq!(env.meta.providers[0].name, "moonwell");
        assert_eq!(env.meta.providers[0].status, "ok");
    }

    // ---- Y-A3: cache transition write -> hit ------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_opportunities_cache_write_then_hit() {
        let server = moonwell_rpc_server().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), true));

        let first = handle(
            &ctx,
            YieldCmd::Opportunities(opportunities_args(&server.uri())),
        )
        .await
        .expect("first yield opportunities");
        assert_eq!(
            first.meta.cache.status, "write",
            "first cache-enabled fetch should write the cache"
        );
        assert!(!first.meta.cache.stale);

        let second = handle(
            &ctx,
            YieldCmd::Opportunities(opportunities_args(&server.uri())),
        )
        .await
        .expect("second yield opportunities");
        assert_eq!(
            second.meta.cache.status, "hit",
            "second identical fetch should hit the cache"
        );
        assert!(!second.meta.cache.stale);
        assert!(
            second.meta.providers.is_empty(),
            "fresh hit must not call the provider"
        );
    }

    // ---- Y-A3 (disabled cache): status "miss" -----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_opportunities_cache_disabled_status_miss() {
        let server = moonwell_rpc_server().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        let env = handle(
            &ctx,
            YieldCmd::Opportunities(opportunities_args(&server.uri())),
        )
        .await
        .expect("yield opportunities");
        assert_eq!(
            env.meta.cache.status, "miss",
            "cache-disabled fetch keeps the initial miss status"
        );
    }

    // ---- Y-A4: --limit threads into the handler ---------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_opportunities_limit_caps_payload() {
        let server = moonwell_rpc_server().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        let mut args = opportunities_args(&server.uri());
        args.limit = 1;
        let env = handle(&ctx, YieldCmd::Opportunities(args))
            .await
            .expect("yield opportunities --limit 1");
        let rows = data_array(&env);
        assert!(
            rows.len() <= 1,
            "--limit 1 must cap rows to 1, got {}",
            rows.len()
        );
    }

    // ---- Y-A5: --min-tvl-usd is forwarded to the provider request ---------

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_opportunities_min_tvl_filters_everything_to_unavailable() {
        let server = moonwell_rpc_server().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        let mut args = opportunities_args(&server.uri());
        // Impossibly large TVL floor: the single mock market is filtered out, so
        // the provider returns nothing -> Go-semantic Unavailable (NOT success).
        args.min_tvl_usd = Some(1e30);
        let err = handle(&ctx, YieldCmd::Opportunities(args))
            .await
            .expect_err("an impossible --min-tvl-usd must filter out all rows");
        assert_eq!(
            err.code,
            Code::Unavailable,
            "no opportunities after filtering must be Unavailable, got {:?}",
            err.code
        );
        // Must NOT be the WS2 placeholder stub error.
        assert!(
            !err.to_string()
                .to_lowercase()
                .contains("not yet implemented"),
            "must route to the real handler, got: {err}"
        );
    }

    // ---- Y-A6: positions success envelope ---------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_positions_success_envelope_and_provider_status() {
        let server = moonwell_rpc_server().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        let env = handle(&ctx, YieldCmd::Positions(positions_args(&server.uri())))
            .await
            .expect("yield positions should succeed against the mock RPC");

        assert_eq!(env.meta.command, "yield positions");
        assert!(env.success);
        let rows = data_array(&env);
        assert!(!rows.is_empty(), "expected at least one position");
        assert_eq!(rows[0]["provider"], json!("moonwell"));

        assert_eq!(env.meta.providers.len(), 1);
        assert_eq!(env.meta.providers[0].name, "moonwell");
        assert_eq!(env.meta.providers[0].status, "ok");
    }

    // ---- Y-E6: kamino yield positions is unsupported (via handle) ---------

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_positions_kamino_is_unsupported_typed_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        let mut args = positions_args("");
        args.providers = Some("kamino".to_string());
        // Kamino is Solana-only; use a Solana address + chain so selection passes.
        args.chain = Some("solana".to_string());
        args.address = Some("6dM4QgP1VnRfx6TVV1t5hBf3ytA5Qn2ATqNnSboP8qz5".to_string());

        let err = handle(&ctx, YieldCmd::Positions(args))
            .await
            .expect_err("kamino yield positions must be unsupported");
        assert_eq!(err.code, Code::Unsupported);
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("does not support positions"),
            "expected capability-gate message, got: {msg}"
        );
        assert!(
            !msg.contains("not yet implemented"),
            "kamino positions must route to the real capability gate, got: {msg}"
        );
    }

    // ---- Y-E7: moonwell yield history is unsupported (via handle) ----------

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_history_moonwell_is_unsupported_typed_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false));

        // Moonwell implements YieldProvider + positions but NOT history.
        let env_err = handle(&ctx, YieldCmd::History(history_args()))
            .await
            .expect_err("moonwell yield history must be unsupported");
        assert_eq!(env_err.code, Code::Unsupported);
        let msg = env_err.to_string().to_lowercase();
        assert!(
            msg.contains("does not support history"),
            "expected history capability-gate message, got: {msg}"
        );
        assert!(
            !msg.contains("not yet implemented"),
            "must route to the real capability gate, got: {msg}"
        );
    }

    // ---- Y-E1..E5, E8, E9: usage error paths via run_with_args ------------

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_positions_missing_address_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(["defi", "yield", "positions", "--chain", "1"], &env).await;
        assert_eq!(code, 2, "missing --address must be a usage error (exit 2)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_positions_invalid_evm_address_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "yield",
                "positions",
                "--chain",
                "1",
                "--address",
                "notanaddress",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "invalid EVM address must be a usage error (exit 2)"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_opportunities_missing_asset_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(["defi", "yield", "opportunities", "--chain", "1"], &env).await;
        assert_eq!(code, 2, "missing --asset must be a usage error (exit 2)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_history_missing_asset_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(["defi", "yield", "history", "--chain", "1"], &env).await;
        assert_eq!(code, 2, "missing --asset must be a usage error (exit 2)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_opportunities_unknown_provider_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "yield",
                "opportunities",
                "--chain",
                "1",
                "--asset",
                "USDC",
                "--providers",
                "bogusprovider",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "unknown --providers must be a usage error (exit 2), matching selectYieldProviders"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_history_invalid_metrics_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "yield",
                "history",
                "--chain",
                "1",
                "--asset",
                "USDC",
                "--metrics",
                "bogus_metric",
            ],
            &env,
        )
        .await;
        assert_eq!(code, 2, "invalid --metrics must be a usage error (exit 2)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_history_invalid_interval_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "yield",
                "history",
                "--chain",
                "1",
                "--asset",
                "USDC",
                "--interval",
                "fortnight",
            ],
            &env,
        )
        .await;
        assert_eq!(code, 2, "invalid --interval must be a usage error (exit 2)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn yield_history_future_to_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "yield",
                "history",
                "--chain",
                "1",
                "--asset",
                "USDC",
                "--to",
                "2999-01-01T00:00:00Z",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "a --to far in the future must be a usage error (exit 2)"
        );
    }

    // (Y-E7's full-binary exit-13 variant is intentionally omitted: the WS2
    // `unimplemented` stub ALSO returns exit 13 (Code::Unsupported), so an
    // exit-code-only assertion through `run_with_args` cannot distinguish the
    // real capability gate from the stub. Y-E7 is asserted strongly above via
    // `yield_history_moonwell_is_unsupported_typed_error`, which checks the
    // gate's `"does not support history"` message and that it is NOT the
    // `"not yet implemented"` placeholder.)

    // ---- Y-F1 / Y-F2: flag parsing ---------------------------------------

    #[test]
    fn yield_opportunities_flag_defaults_and_forwarding_parse() {
        use clap::Parser;
        // Defaults.
        let cli = crate::cli::Cli::try_parse_from([
            "defi",
            "yield",
            "opportunities",
            "--chain",
            "1",
            "--asset",
            "USDC",
        ])
        .expect("yield opportunities parses");
        if let crate::cli::TopCommand::Yield {
            cmd: YieldCmd::Opportunities(args),
        } = cli.command
        {
            assert_eq!(args.limit, 20, "default --limit is 20");
            assert_eq!(args.sort, "apy_total", "default --sort is apy_total");
            assert!(
                !args.include_incomplete,
                "default --include-incomplete false"
            );
        } else {
            panic!("expected yield opportunities command");
        }

        // Multi --providers + --min-tvl-usd + --rpc-url forwarding.
        let cli2 = crate::cli::Cli::try_parse_from([
            "defi",
            "yield",
            "opportunities",
            "--chain",
            "1",
            "--asset",
            "USDC",
            "--providers",
            "aave,morpho",
            "--min-tvl-usd",
            "1000000",
            "--rpc-url",
            "http://x",
        ])
        .expect("yield opportunities with filters parses");
        if let crate::cli::TopCommand::Yield {
            cmd: YieldCmd::Opportunities(args),
        } = cli2.command
        {
            assert_eq!(args.providers.as_deref(), Some("aave,morpho"));
            assert_eq!(args.min_tvl_usd, Some(1_000_000.0));
            assert_eq!(args.rpc_url.as_deref(), Some("http://x"));
        } else {
            panic!("expected yield opportunities command");
        }
    }

    #[test]
    fn yield_history_flag_defaults_parse() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "defi", "yield", "history", "--chain", "1", "--asset", "USDC",
        ])
        .expect("yield history parses");
        if let crate::cli::TopCommand::Yield {
            cmd: YieldCmd::History(args),
        } = cli.command
        {
            assert_eq!(args.metrics, "apy_total");
            assert_eq!(args.interval, "day");
            assert_eq!(args.window, "7d");
            assert_eq!(args.limit, 20);
        } else {
            panic!("expected yield history command");
        }
    }

    #[test]
    fn yield_positions_flag_defaults_parse() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "defi",
            "yield",
            "positions",
            "--chain",
            "1",
            "--address",
            DEAD,
        ])
        .expect("yield positions parses");
        if let crate::cli::TopCommand::Yield {
            cmd: YieldCmd::Positions(args),
        } = cli.command
        {
            assert_eq!(args.limit, 20, "default --limit is 20");
        } else {
            panic!("expected yield positions command");
        }
    }

    // ---- silence unused-import lint on PathBuf in some build configs ------
    #[allow(dead_code)]
    fn _assert_pathbuf_used(_p: PathBuf) {}
}

#[cfg(test)]
mod plan_app_tests {
    //! # Success criteria — `yield <verb> plan` app-level handler (WS3, exec-plan)
    //!
    //! Go oracle: `internal/app/yield_execution_commands.go` `planCmd.RunE` (the
    //! `buildAction` closure → `s.actionBuilderRegistry().BuildYieldAction(...)` →
    //! `applyExecutionIdentityToAction` → `s.actionStore.Save` → `emitSuccess`).
    //! These tests drive [`cli::handle`] (the real dispatch entry the binary
    //! calls) end-to-end for the TWO yield plan verbs (`deposit`/`withdraw`
    //! `plan`) ONLY, asserting the full machine contract the Go runner emits via
    //! `emitSuccess(...)` / the typed error → full-envelope `renderError(...)`
    //! path. RED until WS3 wires the yield `plan` handler: [`cli::handle`]
    //! currently returns the typed `unimplemented` stub ([`Code::Unsupported`]
    //! with `"not yet implemented"`) for both verb commands, so every assertion
    //! that expects a real action envelope / a Go-semantic guard error fails.
    //!
    //! ## Determinism / offline seams
    //!
    //! `BuildYieldAction` routes by `--provider`:
    //!   * `aave` → `build_aave_lend_action` (supply/withdraw), then stamps
    //!     `intent_type = "yield_<verb>"` and adds `metadata.yield_action` +
    //!     `metadata.yield_product == "aave_reserve"` over the Aave lend context;
    //!   * `morpho` → `build_morpho_vault_yield_action` (ERC-4626 vault; needs a
    //!     valid `--vault-address` + a Morpho GraphQL lookup);
    //!   * `moonwell` → rejects `--on-behalf-of`, else `build_moonwell_lend_action`,
    //!     then stamps `yield_<verb>` + `metadata.yield_product == "moonwell_market"`.
    //!
    //! The Aave path connects to RPC (`RpcClient::connect`) and, for
    //! `deposit`/`withdraw`, the underlying Aave supply path issues exactly one
    //! `eth_call` (`allowance(owner,spender)`) to decide whether an approval step
    //! is needed when `--pool-address` is supplied (the pool is not RPC-resolved).
    //! All RPC is injected through the already-present `--rpc-url` flag pointed at
    //! a `wiremock` JSON-RPC mock that answers every `eth_call` with an
    //! ABI-encoded `allowance` word (the same `EchoIdResponder` shape the
    //! `defi-execution` planner suite + the `lend` plan app tests use), so the
    //! Aave tests are fully offline + deterministic. Identity is exercised through
    //! the OFFLINE `--from-address` (legacy_local) path so no OWS vault / network
    //! is touched; the `--wallet` happy path (OWS resolve) is WS4b e2e territory
    //! and is asserted here only via its offline guard rejections.
    //!
    //! Aave yield uses `interest_rate_mode == 0` internally (it is a supply/
    //! withdraw, not a borrow), and `--pool-address` short-circuits the on-chain
    //! `getPool()` lookup, so the Aave verbs build deterministically without a
    //! pool-provider mock.
    //!
    //! Morpho: a full Morpho vault happy path needs the Morpho GraphQL endpoint
    //! (no app-level base-URL seam — the builder uses the production endpoint), so
    //! Morpho is asserted via its OFFLINE guard (`--vault-address` required;
    //! malformed `--vault-address`), which the planner checks before any GraphQL
    //! fetch. Moonwell is asserted via its OFFLINE `--on-behalf-of` rejection
    //! (Compound v2 calls operate on `msg.sender` only), checked before any RPC.
    //!
    //! ## Criteria (each a failing test until `cli::handle` wires `*_plan`)
    //!
    //! 1. **Plan success envelope (Aave deposit, legacy `--from-address`).** A
    //!    valid `yield deposit plan --provider aave --chain 1 --asset USDC --amount
    //!    1000000 --from-address 0x..aa --pool-address 0x..CC --rpc-url <mock>`
    //!    (allowance insufficient) returns `Ok(Envelope)` (exit 0) with:
    //!    `version=="v1"`, `success==true`, `error==None`, `meta.partial==false`,
    //!    `meta.command=="yield deposit plan"`,
    //!    `meta.cache=={status:"bypass", age_ms:0, stale:false}` (execution paths
    //!    bypass the cache, spec §2.5), and `meta.providers==[{name:"aave",
    //!    status:"ok"}]` (Go captures one `ProviderStatus` keyed on the normalized
    //!    lending provider name with `statusFromErr(nil)=="ok"`).
    //!
    //! 2. **Planned action `data` shape (Aave deposit).** `env.data` is the
    //!    serialized [`Action`]: `action_id` matches `^act_[0-9a-f]{32}$`;
    //!    `intent_type=="yield_deposit"`; `provider=="aave"`; `status=="planned"`;
    //!    `chain_id=="eip155:1"`; `from_address` == the EIP-55 checksum of the
    //!    sender; `input_amount=="1000000"`. With an INSUFFICIENT allowance the
    //!    action has TWO steps — `[approval, lend_call]` — where the lend step
    //!    `type=="lend_call"`, `value=="0"`, `chain_id=="eip155:1"`, and `target` ==
    //!    the pool address (`0x..CC`). The action `metadata` carries the Aave
    //!    context (`protocol=="aave"`) PLUS the yield-routing additions
    //!    `yield_action=="deposit"` and `yield_product=="aave_reserve"`. (Go
    //!    `BuildYieldAction` aave branch → `BuildAaveLendAction` + the
    //!    `yield_<verb>`/`yield_action`/`yield_product` overwrite + `emitSuccess`.)
    //!
    //! 3. **Aave deposit lend-step calldata reuses the alloy/ABI golden.** The lend
    //!    step `data` equals `supply(asset, amount, onBehalfOf, 0)` encoded with the
    //!    canonical `AAVE_POOL_ABI` via the same alloy `Function` machinery the
    //!    planner uses (computed in-test, NOT re-encoded by the handler). With the
    //!    default `--on-behalf-of` empty, `onBehalfOf` defaults to the resolved
    //!    sender. Proves the handler routes through `build_yield_action`→Aave (no
    //!    re-encoding) and that base⇔decimal amounts stay consistent (spec §2.4).
    //!
    //! 4. **Aave deposit skips the approval step when allowance is sufficient.**
    //!    The same plan against a mock whose `allowance` >= the requested amount
    //!    yields a SINGLE `lend_call` step (no leading `approval` step). (Go
    //!    `appendApprovalIfNeeded`: `current >= amount` → no approval.)
    //!
    //! 5. **Aave withdraw is a single lend step (no RPC `eth_call`).** `yield
    //!    withdraw plan ... --pool-address 0x..CC --rpc-url <mock>` yields a single
    //!    `lend_call` step with `intent_type=="yield_withdraw"`,
    //!    `meta.command=="yield withdraw plan"`, target == pool, calldata ==
    //!    `withdraw(asset, amount, to=recipient)` (recipient defaults to the
    //!    sender), and `metadata.yield_action=="withdraw"`. No `approval` step.
    //!    (Go withdraw verb via the Aave `AaveVerbWithdraw` path.)
    //!
    //! 6. **Plan persists the action to the Store.** After a successful Aave
    //!    deposit plan the action is retrievable by its `action_id` from a freshly
    //!    opened [`defi_execution::store::Store`] over the same path, with matching
    //!    `intent_type=="yield_deposit"`, `input_amount=="1000000"`, and
    //!    `provider=="aave"`. (Go `s.actionStore.Save`.)
    //!
    //! 7. **Legacy-identity warning + backend stamping.** The `--from-address`
    //!    path stamps `execution_backend=="legacy_local"` on the action AND
    //!    surfaces the Go warning `--wallet (OWS) is recommended over
    //!    --from-address for planning; see docs for details` in `env.warnings`.
    //!    (Go `resolveExecutionIdentity` legacy branch + `emitSuccess(...,
    //!    identity.Warnings, ...)`.)
    //!
    //! 8. **Decimal amount parity.** `--amount-decimal 1` (no `--amount`) on USDC
    //!    (6 decimals) yields the same `input_amount=="1000000"` and the same
    //!    deposit calldata golden — base⇔decimal stay consistent (spec §2.4).
    //!
    //! 9. **`--provider` is required.** `yield deposit plan` with an empty/missing
    //!    `--provider` → [`Code::Usage`] (exit 2) and persists NOTHING. (Go
    //!    `BuildYieldAction`: `--provider is required`.)
    //!
    //! 10. **Unsupported yield provider.** `--provider kamino` (no yield-execution
    //!     builder) → [`Code::Unsupported`] (exit 13) with the Go message `yield
    //!     execution currently supports provider=aave|morpho|moonwell`; persists
    //!     NOTHING. (Go `BuildYieldAction` default branch.)
    //!
    //! 11. **Identity-constraint errors (offline).**
    //!     (a) BOTH `--wallet` and `--from-address` → [`Code::Usage`] (exit 2);
    //!     (b) NEITHER `--wallet` nor `--from-address` → [`Code::Usage`] (exit 2);
    //!     (c) a malformed `--from-address` → [`Code::Usage`] (exit 2);
    //!     (d) `--wallet` on a Tempo chain → [`Code::Unsupported`] (exit 13)
    //!         (`--wallet planning is not supported on Tempo chains yet`).
    //!     (Go `resolveExecutionIdentity`.) On every error the handler returns the
    //!     typed `Err(Error)` (the runner renders the full error envelope to
    //!     stderr, spec §2.1) and persists NOTHING.
    //!
    //! 12. **Amount cross-validation through the handler.** BOTH `--amount` +
    //!     `--amount-decimal` → [`Code::Usage`] (exit 2); NEITHER → [`Code::Usage`]
    //!     (exit 2); a non-positive `--amount` (`0`) → [`Code::Usage`] (exit 2).
    //!     Nothing persisted. (Delegated to `defi_id::normalize_amount` /
    //!     `normalize_lend_inputs` via `build_yield_action`.)
    //!
    //! 13. **Morpho requires a valid `--vault-address` (offline).** `yield deposit
    //!     plan --provider morpho --chain 1 --asset USDC --amount 1000000
    //!     --from-address 0x..aa --rpc-url <mock>` with NO `--vault-address` →
    //!     [`Code::Usage`] (exit 2) with `morpho vault yield execution requires a
    //!     valid --vault-address` (the planner's offline guard, checked before any
    //!     GraphQL fetch); a malformed (non-hex) `--vault-address` is likewise
    //!     [`Code::Usage`] (exit 2). Nothing persisted. (Go `BuildYieldAction`
    //!     morpho path → `BuildMorphoVaultYieldAction` vault-address guard.)
    //!
    //! 14. **Moonwell rejects `--on-behalf-of` (offline).** `yield deposit plan
    //!     --provider moonwell --chain base --asset USDC --amount 1000000
    //!     --on-behalf-of 0x..bb --from-address 0x..aa` → [`Code::Unsupported`]
    //!     (exit 13) with `moonwell does not support --on-behalf-of` (checked
    //!     before any RPC). Nothing persisted. (Go `BuildYieldAction` Moonwell
    //!     guard.)
    //!
    //! 15. **Provider-status fallback name is `"yield"`.** When the build fails
    //!     because of an UNSUPPORTED provider (so a status row is still captured
    //!     with the normalized provider name), the Go runner keys the row on the
    //!     normalized lending provider, falling back to `"yield"` (NOT `"lend"`)
    //!     when empty — asserted indirectly via the success path (`aave`) here and
    //!     the unsupported-provider path's error code. (Go `providerName =
    //!     "yield"` fallback in `yield_execution_commands.go`.)
    //!
    //! SKIPPED (covered elsewhere / wrong unit):
    //!   * the Aave/Morpho/Moonwell ABI calldata encoding internals + the
    //!     sender/recipient/asset hex + positive-amount validation — owned by the
    //!     `defi-execution::planner` suite (ported from `planner/*_test.go`);
    //!   * the `build_yield_action` provider routing itself — `defi-execution::
    //!     builder` (its own suite);
    //!   * the OWS `--wallet` happy-path resolve + wallet-id persistence — WS4b
    //!     e2e (here only its offline guard rejections are asserted);
    //!   * `--input-json`/`--input-file` precedence — structured-input unit;
    //!   * cobra/clap flag defaults + required-flag marking — schema/CLI suites;
    //!   * a full Morpho/Moonwell happy-path action build (GraphQL/RPC heavy) —
    //!     `defi-execution::planner` suite + WS5 sweep.

    use super::cli::{handle, YieldCmd, YieldPlanArgs, YieldVerbCmd};
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags};
    use defi_config::Settings;
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;

    use alloy::dyn_abi::{DynSolValue, FunctionExt, JsonAbiExt};
    use alloy::json_abi::JsonAbi;
    use alloy::primitives::U256;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    // --- contract constants -------------------------------------------------

    /// Sender EOA (legacy `--from-address` identity); its EIP-55 checksum lands on
    /// the action.
    const SENDER: &str = "0x00000000000000000000000000000000000000aa";
    /// An on-behalf-of address used only in the Moonwell-rejection test.
    const OTHER: &str = "0x00000000000000000000000000000000000000bb";
    /// Aave Pool override (`--pool-address`) — short-circuits the on-chain
    /// `getPool()` lookup.
    const POOL: &str = "0x00000000000000000000000000000000000000cc";
    /// USDC contract on Ethereum mainnet (6 decimals) — resolved by `parse_asset`.
    const USDC_MAINNET: &str = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
    /// A syntactically invalid (too-short, non-hex-address) Morpho vault address.
    const SHORT_VAULT: &str = "0x1234";
    /// The Go legacy-identity warning surfaced when planning with `--from-address`.
    const LEGACY_WARNING: &str =
        "--wallet (OWS) is recommended over --from-address for planning; see docs for details";

    // --- harness ------------------------------------------------------------

    /// Execution settings with a real action store under `dir` and the cache
    /// disabled (execution paths bypass the cache anyway, spec §2.5).
    fn exec_settings(dir: &Path) -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: false,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_secs(5),
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

    /// An Aave deposit `YieldPlanArgs` with the canonical happy-path values; mutate
    /// per test. `--pool-address` is set so no on-chain `getPool()` is needed.
    fn aave_deposit_args(rpc: &str) -> YieldPlanArgs {
        YieldPlanArgs {
            chain: Some("1".to_string()),
            asset: Some("USDC".to_string()),
            amount: Some("1000000".to_string()),
            amount_decimal: None,
            provider: Some("aave".to_string()),
            recipient: None,
            on_behalf_of: None,
            vault_address: None,
            pool_address: Some(POOL.to_string()),
            pool_address_provider: None,
            rpc_url: Some(rpc.to_string()),
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(SENDER.to_string()),
            },
            input: InputFlags::default(),
        }
    }

    async fn run_plan(dir: &Path, cmd: YieldCmd) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(&ctx, cmd).await
    }

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn action_data(env: &Envelope) -> Value {
        env.data.clone().expect("plan envelope carries `data`")
    }

    /// True iff no action is persisted under `dir` (error paths must persist
    /// nothing). A never-created store counts as empty.
    fn no_actions_persisted(dir: &Path) -> bool {
        let store = match ActionStore::open(dir.join("actions.db"), dir.join("actions.lock")) {
            Ok(store) => store,
            Err(_) => return true,
        };
        store
            .list("", 1000)
            .map(|actions| actions.is_empty())
            .unwrap_or(true)
    }

    // --- wiremock JSON-RPC: every eth_call returns `result` --------------------

    /// A `wiremock` responder that wraps a fixed hex `result` in a JSON-RPC
    /// success envelope, echoing the incoming request `id` (mirrors the
    /// `defi-execution` planner `EchoIdResponder`).
    struct EchoIdResponder {
        result: String,
    }

    impl Respond for EchoIdResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let id = serde_json::from_slice::<Value>(&request.body)
                .ok()
                .and_then(|body| body.get("id").cloned())
                .unwrap_or_else(|| Value::from(1));
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": self.result,
            }))
        }
    }

    fn uint_word(v: u128) -> String {
        format!("0x{}", hex::encode(U256::from(v).to_be_bytes::<32>()))
    }

    /// A mock JSON-RPC endpoint answering every `eth_call` with a single
    /// ABI-encoded `uint256` word == `allowance`. Used for the allowance-check
    /// path (deposit) and accepted (but unused) by withdraw, which makes no
    /// `eth_call` when `--pool-address` is supplied.
    async fn allowance_rpc(allowance: u128) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(EchoIdResponder {
                result: uint_word(allowance),
            })
            .mount(&server)
            .await;
        server
    }

    // --- in-test alloy/ABI golden (reuses AAVE_POOL_ABI) -----------------------

    fn aave_fn(name: &str) -> alloy::json_abi::Function {
        let abi: JsonAbi = serde_json::from_str(defi_registry::AAVE_POOL_ABI).expect("parse abi");
        abi.function(name)
            .and_then(|o| o.first())
            .cloned()
            .expect("aave fn present")
    }

    fn aave_calldata(name: &str, args: &[DynSolValue]) -> String {
        let data = aave_fn(name)
            .abi_encode_input(args)
            .expect("encode aave fn");
        format!("0x{}", hex::encode(data))
    }

    fn addr_val(hexaddr: &str) -> DynSolValue {
        DynSolValue::Address(hexaddr.parse().expect("valid address"))
    }

    /// Expected `supply(asset, amount, onBehalfOf, referralCode=0)` calldata
    /// (Aave yield deposit reuses the Aave supply path).
    fn supply_calldata(amount: u128, on_behalf_of: &str) -> String {
        aave_calldata(
            "supply",
            &[
                addr_val(USDC_MAINNET),
                DynSolValue::Uint(U256::from(amount), 256),
                addr_val(on_behalf_of),
                DynSolValue::Uint(U256::ZERO, 16),
            ],
        )
    }

    /// Expected `withdraw(asset, amount, to)` calldata (Aave yield withdraw
    /// reuses the Aave withdraw path).
    fn withdraw_calldata(amount: u128, to: &str) -> String {
        aave_calldata(
            "withdraw",
            &[
                addr_val(USDC_MAINNET),
                DynSolValue::Uint(U256::from(amount), 256),
                addr_val(to),
            ],
        )
    }

    fn step_types(data: &Value) -> Vec<String> {
        data["steps"]
            .as_array()
            .expect("steps array")
            .iter()
            .map(|s| s["type"].as_str().unwrap_or("").to_string())
            .collect()
    }

    /// The first step whose `type == "lend_call"` (Go `StepTypeLend ==
    /// "lend_call"`; yield deposits/withdraws reuse the lend step type).
    fn lend_step(data: &Value) -> Value {
        data["steps"]
            .as_array()
            .expect("steps array")
            .iter()
            .find(|s| s["type"].as_str() == Some("lend_call"))
            .cloned()
            .expect("a lend step is present")
    }

    // --- 1, 2, 3, 7, 15. Aave deposit happy path ---------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_emits_success_envelope_and_action_shape() {
        let rpc = allowance_rpc(0).await; // insufficient -> approval needed.
        let tmp = TempDir::new().expect("tempdir");
        let env = run_plan(
            tmp.path(),
            YieldCmd::Deposit(YieldVerbCmd::Plan(aave_deposit_args(&rpc.uri()))),
        )
        .await
        .expect("aave yield deposit plan should succeed against the mock RPC");

        // Envelope contract (Go `emitSuccess`).
        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert!(!env.meta.partial);
        assert_eq!(env.meta.command, "yield deposit plan");

        // Execution paths bypass the cache (spec §2.5).
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        // One provider status keyed on the normalized lending provider, ok.
        assert_eq!(env.meta.providers.len(), 1, "exactly one provider status");
        assert_eq!(env.meta.providers[0].name, "aave");
        assert_eq!(env.meta.providers[0].status, "ok");

        // Action `data` shape (Go persisted action).
        let data = action_data(&env);
        let action_id = data["action_id"].as_str().expect("action_id string");
        assert!(
            action_id.strip_prefix("act_").is_some_and(|rest| rest.len() == 32
                && rest.bytes().all(|b| b.is_ascii_hexdigit())),
            "action_id must match act_<32 hex>: got {action_id}"
        );
        assert_eq!(data["intent_type"], Value::from("yield_deposit"));
        assert_eq!(data["provider"], Value::from("aave"));
        assert_eq!(data["status"], Value::from("planned"));
        assert_eq!(data["chain_id"], Value::from("eip155:1"));
        assert_eq!(
            data["from_address"].as_str().unwrap().to_lowercase(),
            SENDER.to_lowercase(),
            "from_address is the (checksummed) sender"
        );
        assert_eq!(data["input_amount"], Value::from("1000000"));

        // Insufficient allowance -> [approval, lend_call].
        assert_eq!(
            step_types(&data),
            vec!["approval".to_string(), "lend_call".to_string()],
            "insufficient allowance => approval then lend_call"
        );
        let lend = lend_step(&data);
        assert_eq!(lend["value"], Value::from("0"));
        assert_eq!(lend["chain_id"], Value::from("eip155:1"));
        assert_eq!(
            lend["target"].as_str().unwrap().to_lowercase(),
            POOL.to_lowercase(),
            "lend step targets the resolved pool"
        );

        // metadata carries the Aave context PLUS the yield-routing additions.
        let meta = data["metadata"].as_object().expect("metadata object");
        assert_eq!(meta.get("protocol"), Some(&Value::from("aave")));
        assert_eq!(
            meta.get("yield_action"),
            Some(&Value::from("deposit")),
            "yield routing stamps yield_action"
        );
        assert_eq!(
            meta.get("yield_product"),
            Some(&Value::from("aave_reserve")),
            "Aave yield product label"
        );

        // Legacy backend stamping + warning (criterion 7).
        assert_eq!(data["execution_backend"], Value::from("legacy_local"));
        assert!(
            env.warnings.iter().any(|w| w == LEGACY_WARNING),
            "legacy --from-address plan surfaces the OWS-recommended warning; got {:?}",
            env.warnings
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_lend_step_calldata_matches_aave_abi_golden() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let env = run_plan(
            tmp.path(),
            YieldCmd::Deposit(YieldVerbCmd::Plan(aave_deposit_args(&rpc.uri()))),
        )
        .await
        .expect("aave yield deposit plan should succeed");
        let data = action_data(&env);
        let lend = lend_step(&data);
        let calldata = lend["data"].as_str().expect("lend step data");
        // on_behalf_of defaults to the sender when the flag is empty.
        assert_eq!(
            calldata.to_lowercase(),
            supply_calldata(1_000_000, SENDER).to_lowercase(),
            "deposit lend-step calldata must equal the alloy AAVE_POOL_ABI supply golden"
        );
    }

    // --- 4. allowance sufficient -> single lend step ----------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_skips_approval_when_allowance_sufficient() {
        let rpc = allowance_rpc(10_000_000).await; // >= requested.
        let tmp = TempDir::new().expect("tempdir");
        let env = run_plan(
            tmp.path(),
            YieldCmd::Deposit(YieldVerbCmd::Plan(aave_deposit_args(&rpc.uri()))),
        )
        .await
        .expect("aave yield deposit plan should succeed");
        let data = action_data(&env);
        assert_eq!(
            step_types(&data),
            vec!["lend_call".to_string()],
            "sufficient allowance => single lend step"
        );
    }

    // --- 5. Aave withdraw --------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn withdraw_plan_is_single_lend_step_with_golden_calldata() {
        let rpc = allowance_rpc(0).await; // withdraw makes no eth_call, but connect succeeds.
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_deposit_args(&rpc.uri());
        args.amount = Some("500000".to_string());
        let env = run_plan(tmp.path(), YieldCmd::Withdraw(YieldVerbCmd::Plan(args)))
            .await
            .expect("aave yield withdraw plan should succeed");
        let data = action_data(&env);
        assert_eq!(data["intent_type"], Value::from("yield_withdraw"));
        assert_eq!(env.meta.command, "yield withdraw plan");
        assert_eq!(step_types(&data), vec!["lend_call".to_string()]);
        let lend = lend_step(&data);
        assert_eq!(
            lend["target"].as_str().unwrap().to_lowercase(),
            POOL.to_lowercase()
        );
        // recipient defaults to the sender.
        assert_eq!(
            lend["data"].as_str().unwrap().to_lowercase(),
            withdraw_calldata(500_000, SENDER).to_lowercase(),
            "withdraw calldata must equal the alloy AAVE_POOL_ABI golden"
        );
        // yield-routing metadata addition for the withdraw verb.
        let meta = data["metadata"].as_object().expect("metadata object");
        assert_eq!(meta.get("yield_action"), Some(&Value::from("withdraw")));
        assert_eq!(
            meta.get("yield_product"),
            Some(&Value::from("aave_reserve"))
        );
    }

    // --- 6. plan persists the action to the Store --------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_persists_action_to_store() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let settings = exec_settings(tmp.path());
        let ctx = AppCtx::new(settings.clone());
        let env = handle(
            &ctx,
            YieldCmd::Deposit(YieldVerbCmd::Plan(aave_deposit_args(&rpc.uri()))),
        )
        .await
        .expect("aave yield deposit plan should succeed");
        let action_id = action_data(&env)["action_id"]
            .as_str()
            .expect("action_id")
            .to_string();

        let store = ActionStore::open(&settings.action_store_path, &settings.action_lock_path)
            .expect("reopen action store");
        let persisted = store
            .get(&action_id)
            .expect("planned action retrievable by id");
        assert_eq!(persisted.intent_type, "yield_deposit");
        assert_eq!(persisted.input_amount, "1000000");
        assert_eq!(persisted.provider, "aave");
    }

    // --- 8. decimal amount parity ------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_decimal_amount_yields_same_base_and_calldata() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_deposit_args(&rpc.uri());
        args.amount = None;
        args.amount_decimal = Some("1".to_string()); // 1 USDC (6 decimals).
        let env = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect("decimal-amount plan should succeed");
        let data = action_data(&env);
        assert_eq!(data["input_amount"], Value::from("1000000"));
        assert_eq!(
            lend_step(&data)["data"].as_str().unwrap().to_lowercase(),
            supply_calldata(1_000_000, SENDER).to_lowercase(),
            "decimal 1 USDC normalizes to the same calldata as base 1000000"
        );
    }

    // --- 9. --provider required --------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_requires_provider() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_deposit_args(&rpc.uri());
        args.provider = None;
        let err = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect_err("missing --provider must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 10. unsupported yield provider ------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_rejects_kamino_provider() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_deposit_args(&rpc.uri());
        args.provider = Some("kamino".to_string());
        let err = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect_err("kamino yield execution must be unsupported");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .contains("yield execution currently supports provider=aave|morpho|moonwell"),
            "got: {err}"
        );
        assert!(
            !err.to_string().contains("not yet implemented"),
            "must route to the real builder, not the WS3 stub: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 11. identity-constraint errors (offline) --------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_rejects_both_identity_inputs() {
        let tmp = TempDir::new().expect("tempdir");
        // No RPC needed: identity resolution happens before any build.
        let mut args = aave_deposit_args("http://127.0.0.1:1");
        args.identity.wallet = Some("alice".to_string());
        // from_address already set in base.
        let err = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect_err("both identity inputs must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_rejects_missing_identity_inputs() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_deposit_args("http://127.0.0.1:1");
        args.identity.wallet = None;
        args.identity.from_address = None;
        let err = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect_err("missing identity inputs must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_rejects_malformed_from_address() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_deposit_args("http://127.0.0.1:1");
        args.identity.from_address = Some("0xnot-an-address".to_string());
        let err = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect_err("malformed --from-address must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_rejects_wallet_on_tempo_chain() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_deposit_args("http://127.0.0.1:1");
        args.chain = Some("tempo".to_string()); // Tempo mainnet.
        args.identity.from_address = None;
        args.identity.wallet = Some("alice".to_string());
        let err = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect_err("--wallet on Tempo must be rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .contains("--wallet planning is not supported on Tempo chains yet"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 12. amount cross-validation through the handler -------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_rejects_both_amount_forms() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_deposit_args("http://127.0.0.1:1");
        args.amount = Some("1000000".to_string());
        args.amount_decimal = Some("1".to_string());
        let err = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect_err("both amount forms must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_rejects_missing_amount() {
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_deposit_args("http://127.0.0.1:1");
        args.amount = None;
        args.amount_decimal = None;
        let err = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect_err("missing amount must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deposit_plan_rejects_non_positive_amount() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_deposit_args(&rpc.uri());
        args.amount = Some("0".to_string());
        let err = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect_err("zero amount must be rejected by the planner");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 13. Morpho requires a valid --vault-address (offline) -------------

    #[tokio::test(flavor = "multi_thread")]
    async fn morpho_deposit_plan_requires_vault_address() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_deposit_args(&rpc.uri());
        args.provider = Some("morpho".to_string());
        args.pool_address = None; // morpho ignores --pool-address.
        args.vault_address = None;
        let err = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect_err("morpho without --vault-address must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("morpho vault yield execution requires a valid --vault-address"),
            "expected the vault-address guard, got: {err}"
        );
        assert!(
            !err.to_string().contains("not yet implemented"),
            "must route to the real planner, not the WS3 stub: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn morpho_deposit_plan_rejects_malformed_vault_address() {
        let rpc = allowance_rpc(0).await;
        let tmp = TempDir::new().expect("tempdir");
        let mut args = aave_deposit_args(&rpc.uri());
        args.provider = Some("morpho".to_string());
        args.pool_address = None;
        args.vault_address = Some(SHORT_VAULT.to_string());
        let err = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect_err("morpho with a malformed --vault-address must be rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(tmp.path()));
    }

    // --- 14. Moonwell rejects --on-behalf-of (offline) ---------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn moonwell_deposit_plan_rejects_on_behalf_of() {
        let tmp = TempDir::new().expect("tempdir");
        // No RPC needed: the on-behalf-of guard fires before any RPC call.
        let mut args = aave_deposit_args("http://127.0.0.1:1");
        args.provider = Some("moonwell".to_string());
        args.chain = Some("base".to_string());
        args.pool_address = None;
        args.on_behalf_of = Some(OTHER.to_string());
        let err = run_plan(tmp.path(), YieldCmd::Deposit(YieldVerbCmd::Plan(args)))
            .await
            .expect_err("moonwell --on-behalf-of must be unsupported");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .contains("moonwell does not support --on-behalf-of"),
            "got: {err}"
        );
        assert!(no_actions_persisted(tmp.path()));
    }
}
