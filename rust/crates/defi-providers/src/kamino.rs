//! Kamino provider adapter — lending markets/rates + yield
//! opportunities/history, backed by the Kamino Finance REST API.
//!
//! Go source: `internal/providers/kamino/client.go` (+ `client_test.go`).
//!
//! Implements the `LendingProvider` (markets/rates), `YieldProvider`, and
//! `YieldHistoryProvider` trait surfaces, plus `Provider` metadata. Kamino is a
//! Solana-only protocol: every read first validates the chain is Solana mainnet
//! (`solana:5eykt4Us…`). The market list comes from `/v2/kamino-market`; each
//! market's reserve metrics are fetched from
//! `/kamino-market/{market}/reserves/metrics?env=mainnet-beta`, and historical
//! series from `/kamino-market/{market}/reserves/{reserve}/metrics/history`.
//!
//! All outputs are deterministic (stable multi-key sorts). Every APY field is a
//! PERCENTAGE POINT, not a ratio (spec §2.5): the API's ratio values (`0.032`)
//! are scaled ×100 to `3.2`. No API key is required.
//!
//! Concurrency note: the Go client fetches per-market reserves through a bounded
//! worker pool purely for latency. The fetch order has NO effect on the wire
//! contract (markets are pre-sorted before fetching, and the collected reserves
//! are re-sorted by the deterministic output comparators), so the Rust port
//! fetches sequentially — observationally identical and free of `tokio::spawn`
//! in library code.

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_httpx::{do_body_json, Client as HttpClient};
use defi_id::{parse_chain, Asset, Chain};
use defi_model as model;
use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;
use sha1::{Digest, Sha1};

use crate::traits::{
    LendingProvider, Provider, YieldHistoryInterval, YieldHistoryMetric, YieldHistoryProvider,
    YieldHistoryRequest, YieldProvider, YieldRequest,
};
use crate::yieldutil;

/// Default Kamino REST base URL (mirrors Go `defaultBase`).
const DEFAULT_BASE: &str = "https://api.kamino.finance";
/// Solana mainnet CAIP-2 chain id (mirrors Go `solanaMainnetCAIP2`).
const SOLANA_MAINNET_CAIP2: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";

/// Kamino lending + yield adapter (mirrors Go `kamino.Client`).
pub struct Client {
    http: HttpClient,
    base_url: String,
    /// Injected fixed clock for deterministic `fetched_at`; `None` uses the wall
    /// clock.
    now: Option<DateTime<Utc>>,
}

impl Client {
    /// Build a client pointed at the default Kamino base URL (mirrors Go `New`).
    pub fn new(http: HttpClient) -> Self {
        Client {
            http,
            base_url: DEFAULT_BASE.to_string(),
            now: None,
        }
    }

    /// Override the REST base URL (test seam for Go `c.baseURL = srv.URL`).
    pub fn set_base_url(&mut self, url: &str) {
        self.base_url = url.to_string();
    }

    /// Pin the clock (test seam for Go `c.now`).
    pub fn set_now(&mut self, now: DateTime<Utc>) {
        self.now = Some(now);
    }

    /// Current UTC time: the injected clock if set, else the wall clock.
    fn now(&self) -> DateTime<Utc> {
        self.now.unwrap_or_else(Utc::now)
    }

    /// RFC3339 (`...Z`) timestamp for `fetched_at`, matching Go's
    /// `time.Now().UTC().Format(time.RFC3339)`.
    fn fetched_at(&self) -> String {
        self.now().to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    /// Provider metadata (mirrors Go `Info`).
    pub fn info(&self) -> model::ProviderInfo {
        model::ProviderInfo {
            name: "kamino".to_string(),
            provider_type: "lending+yield".to_string(),
            requires_key: false,
            capabilities: vec![
                "lend.markets".to_string(),
                "lend.rates".to_string(),
                "yield.opportunities".to_string(),
                "yield.history".to_string(),
            ],
            key_env_var_name: String::new(),
            capability_auth: Vec::new(),
        }
    }

    /// Base URL with any trailing slash trimmed (mirrors Go
    /// `strings.TrimRight(c.baseURL, "/")`).
    fn base(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }

    /// Fetch every Solana mainnet reserve paired with its owning market.
    ///
    /// Validates the chain is Solana mainnet, pulls the market list, sorts it
    /// deterministically (primary, then curated, then pubkey), and fetches each
    /// market's reserve metrics. Any single market fetch failure aborts the whole
    /// read with `Unavailable` (mirrors Go's `firstErr` propagation), matching
    /// the "fail the command if any market reserve fetch fails" contract.
    async fn fetch_reserves(&self, chain: &Chain) -> Result<Vec<ReserveWithMarket>, Error> {
        if !chain.is_solana() {
            return Err(Error::new(
                Code::Unsupported,
                "kamino supports only Solana chains",
            ));
        }
        if chain.caip2 != SOLANA_MAINNET_CAIP2 {
            return Err(Error::new(
                Code::Unsupported,
                "kamino supports only Solana mainnet",
            ));
        }

        let markets_url = format!("{}/v2/kamino-market", self.base());
        let mut markets: Vec<MarketInfo> = do_body_json(
            &self.http,
            Method::GET,
            &markets_url,
            None,
            &Default::default(),
        )
        .await?
        .value;
        if markets.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "kamino returned no lending markets",
            ));
        }

        // Deterministic market ordering: primary first, then curated, then
        // lexicographic pubkey (mirrors Go's `sort.Slice`).
        markets.sort_by(|a, b| {
            b.is_primary
                .cmp(&a.is_primary)
                .then_with(|| b.is_curated.cmp(&a.is_curated))
                .then_with(|| a.lending_market.cmp(&b.lending_market))
        });

        let mut collected: Vec<ReserveWithMarket> = Vec::new();
        for market in &markets {
            let reserves = self.fetch_market_reserves(&market.lending_market).await?;
            for reserve in reserves {
                collected.push(ReserveWithMarket {
                    market: market.clone(),
                    reserve,
                });
            }
        }
        if collected.is_empty() {
            return Err(Error::new(Code::Unavailable, "kamino returned no reserves"));
        }
        Ok(collected)
    }

    /// Fetch the reserve metrics for a single market pubkey.
    async fn fetch_market_reserves(
        &self,
        market_pubkey: &str,
    ) -> Result<Vec<ReserveMetric>, Error> {
        let endpoint = format!(
            "{}/kamino-market/{}/reserves/metrics?env=mainnet-beta",
            self.base(),
            market_pubkey.trim()
        );
        let reserves: Vec<ReserveMetric> = do_body_json(
            &self.http,
            Method::GET,
            &endpoint,
            None,
            &Default::default(),
        )
        .await?
        .value;
        Ok(reserves)
    }

    /// Fetch the metrics history for a single reserve within `[start, end]`.
    async fn fetch_reserve_metrics_history(
        &self,
        market_pubkey: &str,
        reserve: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        frequency: &str,
    ) -> Result<ReserveMetricsHistoryResponse, Error> {
        let endpoint = format!(
            "{}/kamino-market/{}/reserves/{}/metrics/history?env=mainnet-beta&start={}&end={}&frequency={}",
            self.base(),
            market_pubkey.trim(),
            reserve.trim(),
            urlencode(&start.to_rfc3339_opts(SecondsFormat::Secs, true)),
            urlencode(&end.to_rfc3339_opts(SecondsFormat::Secs, true)),
            urlencode(frequency.trim()),
        );
        let resp: ReserveMetricsHistoryResponse = do_body_json(
            &self.http,
            Method::GET,
            &endpoint,
            None,
            &Default::default(),
        )
        .await?
        .value;
        Ok(resp)
    }

    /// Resolve the owning market pubkey for a reserve by scanning every reserve
    /// (mirrors Go `resolveMarketForReserve`).
    async fn resolve_market_for_reserve(
        &self,
        chain: &Chain,
        reserve: &str,
    ) -> Result<String, Error> {
        let reserve = reserve.trim();
        if reserve.is_empty() {
            return Err(Error::new(Code::Usage, "reserve id is required"));
        }
        let reserves = self.fetch_reserves(chain).await?;
        for item in &reserves {
            if item.reserve.reserve.trim().eq_ignore_ascii_case(reserve) {
                return Ok(item.market.lending_market.trim().to_string());
            }
        }
        Err(Error::new(
            Code::Unavailable,
            "kamino market not found for reserve",
        ))
    }
}

impl Provider for Client {
    fn info(&self) -> model::ProviderInfo {
        Client::info(self)
    }
}

#[async_trait]
impl LendingProvider for Client {
    async fn lend_markets(
        &self,
        provider: &str,
        chain: Chain,
        asset: Asset,
    ) -> Result<Vec<model::LendMarket>, Error> {
        if !provider.trim().eq_ignore_ascii_case("kamino") {
            return Err(Error::new(
                Code::Unsupported,
                "kamino adapter supports only provider=kamino",
            ));
        }
        let reserves = self.fetch_reserves(&chain).await?;
        let fetched_at = self.fetched_at();

        let mut out: Vec<model::LendMarket> = Vec::with_capacity(reserves.len());
        for item in &reserves {
            if !matches_reserve_asset(&item.reserve, &asset) {
                continue;
            }
            let supply_usd = parse_non_negative(&item.reserve.total_supply_usd);
            let borrow_usd = parse_non_negative(&item.reserve.total_borrow_usd);
            let tvl = yieldutil::positive_first(&[supply_usd, borrow_usd]);
            if tvl <= 0.0 {
                continue;
            }
            let mut liquidity_usd = supply_usd - borrow_usd;
            if liquidity_usd <= 0.0 {
                liquidity_usd = tvl;
            }
            let asset_id = reserve_asset_id(
                &chain.caip2,
                &asset.asset_id,
                &item.reserve.liquidity_token_mint,
            );
            out.push(model::LendMarket {
                protocol: "kamino".to_string(),
                provider: "kamino".to_string(),
                chain_id: chain.caip2.clone(),
                asset_id,
                provider_native_id: item.reserve.reserve.trim().to_string(),
                provider_native_id_kind: model::NATIVE_ID_KIND_POOL_ID.to_string(),
                supply_apy: ratio_to_percent(&item.reserve.supply_apy),
                borrow_apy: ratio_to_percent(&item.reserve.borrow_apy),
                tvl_usd: tvl,
                liquidity_usd,
                source_url: market_url(&item.market.lending_market),
                fetched_at: fetched_at.clone(),
            });
        }

        out.sort_by(|a, b| desc(a.tvl_usd, b.tvl_usd).then_with(|| a.asset_id.cmp(&b.asset_id)));
        if out.is_empty() {
            return Err(Error::new(
                Code::Unsupported,
                "no kamino lending market for requested chain/asset",
            ));
        }
        Ok(out)
    }

    async fn lend_rates(
        &self,
        provider: &str,
        chain: Chain,
        asset: Asset,
    ) -> Result<Vec<model::LendRate>, Error> {
        if !provider.trim().eq_ignore_ascii_case("kamino") {
            return Err(Error::new(
                Code::Unsupported,
                "kamino adapter supports only provider=kamino",
            ));
        }
        let reserves = self.fetch_reserves(&chain).await?;
        let fetched_at = self.fetched_at();

        let mut out: Vec<model::LendRate> = Vec::with_capacity(reserves.len());
        for item in &reserves {
            if !matches_reserve_asset(&item.reserve, &asset) {
                continue;
            }
            let supply_usd = parse_non_negative(&item.reserve.total_supply_usd);
            let borrow_usd = parse_non_negative(&item.reserve.total_borrow_usd);
            let utilization = if supply_usd > 0.0 {
                borrow_usd / supply_usd
            } else {
                0.0
            };
            let asset_id = reserve_asset_id(
                &chain.caip2,
                &asset.asset_id,
                &item.reserve.liquidity_token_mint,
            );
            out.push(model::LendRate {
                protocol: "kamino".to_string(),
                provider: "kamino".to_string(),
                chain_id: chain.caip2.clone(),
                asset_id,
                provider_native_id: item.reserve.reserve.trim().to_string(),
                provider_native_id_kind: model::NATIVE_ID_KIND_POOL_ID.to_string(),
                supply_apy: ratio_to_percent(&item.reserve.supply_apy),
                borrow_apy: ratio_to_percent(&item.reserve.borrow_apy),
                utilization: utilization.clamp(0.0, 1.0),
                source_url: market_url(&item.market.lending_market),
                fetched_at: fetched_at.clone(),
            });
        }

        out.sort_by(|a, b| {
            desc(a.supply_apy, b.supply_apy).then_with(|| a.asset_id.cmp(&b.asset_id))
        });
        if out.is_empty() {
            return Err(Error::new(
                Code::Unsupported,
                "no kamino lending rates for requested chain/asset",
            ));
        }
        Ok(out)
    }
}

#[async_trait]
impl YieldProvider for Client {
    async fn yield_opportunities(
        &self,
        req: YieldRequest,
    ) -> Result<Vec<model::YieldOpportunity>, Error> {
        let reserves = self.fetch_reserves(&req.chain).await?;
        let fetched_at = self.fetched_at();

        let mut out: Vec<model::YieldOpportunity> = Vec::with_capacity(reserves.len());
        for item in &reserves {
            if !matches_reserve_asset(&item.reserve, &req.asset) {
                continue;
            }
            let apy = ratio_to_percent(&item.reserve.supply_apy);
            let tvl = parse_non_negative(&item.reserve.total_supply_usd);
            if (apy == 0.0 || tvl == 0.0) && !req.include_incomplete {
                continue;
            }
            if apy < req.min_apy {
                continue;
            }
            if tvl < req.min_tvl_usd {
                continue;
            }

            let borrow_usd = parse_non_negative(&item.reserve.total_borrow_usd);
            let liquidity_usd = (tvl - borrow_usd).max(0.0);

            let asset_id = reserve_asset_id(
                &req.chain.caip2,
                &req.asset.asset_id,
                &item.reserve.liquidity_token_mint,
            );
            let seed = [
                "kamino",
                req.chain.caip2.as_str(),
                item.market.lending_market.as_str(),
                item.reserve.reserve.as_str(),
                asset_id.as_str(),
            ]
            .join("|");
            out.push(model::YieldOpportunity {
                opportunity_id: hash_opportunity(&seed),
                provider: "kamino".to_string(),
                protocol: "kamino".to_string(),
                chain_id: req.chain.caip2.clone(),
                asset_id: asset_id.clone(),
                provider_native_id: item.reserve.reserve.trim().to_string(),
                provider_native_id_kind: model::NATIVE_ID_KIND_POOL_ID.to_string(),
                opportunity_type: "lend".to_string(),
                apy_base: apy,
                apy_reward: 0.0,
                apy_total: apy,
                tvl_usd: tvl,
                liquidity_usd,
                lockup_days: 0.0,
                withdrawal_terms: "variable".to_string(),
                backing_assets: vec![model::YieldBackingAsset {
                    asset_id,
                    symbol: item.reserve.liquidity_token.trim().to_string(),
                    share_pct: 100.0,
                }],
                source_url: market_url(&item.market.lending_market),
                fetched_at: fetched_at.clone(),
            });
        }

        if out.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "no kamino yield opportunities for requested chain/asset",
            ));
        }
        yieldutil::sort_opportunities(&mut out, &req.sort_by);
        let limit = if req.limit <= 0 || (req.limit as usize) > out.len() {
            out.len()
        } else {
            req.limit as usize
        };
        out.truncate(limit);
        Ok(out)
    }
}

#[async_trait]
impl YieldHistoryProvider for Client {
    async fn yield_history(
        &self,
        req: YieldHistoryRequest,
    ) -> Result<Vec<model::YieldHistorySeries>, Error> {
        if !req
            .opportunity
            .provider
            .trim()
            .eq_ignore_ascii_case("kamino")
        {
            return Err(Error::new(
                Code::Unsupported,
                "kamino history supports only kamino opportunities",
            ));
        }
        if req.start_time >= req.end_time {
            return Err(Error::new(
                Code::Usage,
                "history start time must be before end time",
            ));
        }

        let chain = parse_chain(&req.opportunity.chain_id)
            .map_err(|e| Error::wrap(Code::Usage, "parse kamino opportunity chain", e))?;
        if !chain.is_solana() || chain.caip2 != SOLANA_MAINNET_CAIP2 {
            return Err(Error::new(
                Code::Unsupported,
                "kamino history supports only Solana mainnet",
            ));
        }

        let reserve = req.opportunity.provider_native_id.trim().to_string();
        if reserve.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "kamino opportunity requires provider_native_id reserve",
            ));
        }

        let mut market = market_from_source_url(&req.opportunity.source_url);
        if market.is_empty() {
            market = self.resolve_market_for_reserve(&chain, &reserve).await?;
        }
        let frequency = kamino_history_frequency(req.interval)?;

        let history = self
            .fetch_reserve_metrics_history(
                &market,
                &reserve,
                req.start_time,
                req.end_time,
                frequency,
            )
            .await?;
        if history.history.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "no kamino historical points for requested range",
            ));
        }

        // Dedup requested metrics. Both variants are supported, so the
        // exhaustive `match` in the Go validation loop reduces to two flags here;
        // any future unsupported metric would surface as a new enum variant and
        // force this code to be revisited (the `match` below makes that explicit).
        let mut want_apy = false;
        let mut want_tvl = false;
        for metric in &req.metrics {
            match metric {
                YieldHistoryMetric::ApyTotal => want_apy = true,
                YieldHistoryMetric::TvlUsd => want_tvl = true,
            }
        }

        let mut series: Vec<model::YieldHistorySeries> = Vec::new();

        if want_apy {
            let mut points: Vec<model::YieldHistoryPoint> =
                Vec::with_capacity(history.history.len());
            for sample in &history.history {
                let Some(ts) = parse_rfc3339(sample.timestamp.trim()) else {
                    continue;
                };
                let Some(value) = parse_history_metric(&sample.metrics, "supplyInterestAPY") else {
                    continue;
                };
                points.push(model::YieldHistoryPoint {
                    timestamp: ts.to_rfc3339_opts(SecondsFormat::Secs, true),
                    value: value * 100.0,
                });
            }
            sort_history_points(&mut points);
            if !points.is_empty() {
                series.push(self.history_series(&req, YieldHistoryMetric::ApyTotal, points));
            }
        }

        if want_tvl {
            let mut points: Vec<model::YieldHistoryPoint> =
                Vec::with_capacity(history.history.len());
            for sample in &history.history {
                let Some(ts) = parse_rfc3339(sample.timestamp.trim()) else {
                    continue;
                };
                let Some(value) = parse_history_metric(&sample.metrics, "depositTvl") else {
                    continue;
                };
                points.push(model::YieldHistoryPoint {
                    timestamp: ts.to_rfc3339_opts(SecondsFormat::Secs, true),
                    value,
                });
            }
            sort_history_points(&mut points);
            if !points.is_empty() {
                series.push(self.history_series(&req, YieldHistoryMetric::TvlUsd, points));
            }
        }

        if series.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "no kamino historical points for requested range",
            ));
        }
        Ok(series)
    }
}

impl Client {
    /// Assemble a [`model::YieldHistorySeries`] for one metric from the
    /// opportunity metadata + already-built points (mirrors the duplicated Go
    /// series construction).
    fn history_series(
        &self,
        req: &YieldHistoryRequest,
        metric: YieldHistoryMetric,
        points: Vec<model::YieldHistoryPoint>,
    ) -> model::YieldHistorySeries {
        model::YieldHistorySeries {
            opportunity_id: req.opportunity.opportunity_id.clone(),
            provider: "kamino".to_string(),
            protocol: req.opportunity.protocol.clone(),
            chain_id: req.opportunity.chain_id.clone(),
            asset_id: req.opportunity.asset_id.clone(),
            provider_native_id: req.opportunity.provider_native_id.clone(),
            provider_native_id_kind: req.opportunity.provider_native_id_kind.clone(),
            metric: metric.as_str().to_string(),
            interval: req.interval.as_str().to_string(),
            start_time: req.start_time.to_rfc3339_opts(SecondsFormat::Secs, true),
            end_time: req.end_time.to_rfc3339_opts(SecondsFormat::Secs, true),
            points,
            source_url: req.opportunity.source_url.clone(),
            fetched_at: self.fetched_at(),
        }
    }
}

// ----- API DTOs ------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct MarketInfo {
    #[serde(rename = "lendingMarket", default)]
    lending_market: String,
    #[serde(default)]
    #[allow(dead_code)]
    name: String,
    #[serde(rename = "isPrimary", default)]
    is_primary: bool,
    #[serde(rename = "isCurated", default)]
    is_curated: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ReserveMetric {
    #[serde(default)]
    reserve: String,
    #[serde(rename = "liquidityToken", default)]
    liquidity_token: String,
    #[serde(rename = "liquidityTokenMint", default)]
    liquidity_token_mint: String,
    #[serde(rename = "borrowApy", default)]
    borrow_apy: String,
    #[serde(rename = "supplyApy", default)]
    supply_apy: String,
    #[serde(rename = "totalSupplyUsd", default)]
    total_supply_usd: String,
    #[serde(rename = "totalBorrowUsd", default)]
    total_borrow_usd: String,
}

#[derive(Debug, Clone)]
struct ReserveWithMarket {
    market: MarketInfo,
    reserve: ReserveMetric,
}

#[derive(Debug, Clone, Deserialize)]
struct ReserveMetricsHistoryResponse {
    #[serde(default)]
    #[allow(dead_code)]
    reserve: String,
    #[serde(default)]
    history: Vec<ReserveMetricsHistoryItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReserveMetricsHistoryItem {
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    metrics: std::collections::HashMap<String, Value>,
}

// ----- pure helpers --------------------------------------------------------

/// Whether a reserve matches the requested asset: by mint address when one is
/// resolved, else case-insensitive symbol match (mirrors Go
/// `matchesReserveAsset`).
fn matches_reserve_asset(reserve: &ReserveMetric, asset: &Asset) -> bool {
    if !asset.address.trim().is_empty() {
        return reserve.liquidity_token_mint.trim() == asset.address.trim();
    }
    reserve
        .liquidity_token
        .trim()
        .eq_ignore_ascii_case(asset.symbol.trim())
}

/// Map a Kamino history `interval` to the API `frequency` query param.
fn kamino_history_frequency(interval: YieldHistoryInterval) -> Result<&'static str, Error> {
    match interval {
        YieldHistoryInterval::Hour => Ok("hour"),
        YieldHistoryInterval::Day => Ok("day"),
    }
}

/// Pull a numeric metric value out of the loosely-typed metrics map. Accepts
/// JSON numbers and numeric strings; rejects non-finite values and non-numeric
/// types (mirrors Go `parseHistoryMetric`).
fn parse_history_metric(
    metrics: &std::collections::HashMap<String, Value>,
    key: &str,
) -> Option<f64> {
    let value = metrics.get(key.trim())?;
    match value {
        Value::Number(n) => {
            let f = n.as_f64()?;
            if f.is_finite() {
                Some(f)
            } else {
                None
            }
        }
        Value::String(s) => {
            let f: f64 = s.trim().parse().ok()?;
            if f.is_finite() {
                Some(f)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Sort history points ascending by their RFC3339 timestamp string (mirrors Go
/// `sortHistoryPoints`).
fn sort_history_points(points: &mut [model::YieldHistoryPoint]) {
    points.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
}

/// Compose the canonical asset id from a mint when present, else fall back to
/// the requested asset id (mirrors Go `reserveAssetID`).
fn reserve_asset_id(chain_id: &str, fallback_asset_id: &str, mint: &str) -> String {
    let mint = mint.trim();
    if mint.is_empty() {
        return fallback_asset_id.to_string();
    }
    format!("{chain_id}/token:{mint}")
}

/// Build the Kamino app market URL (mirrors Go `marketURL`).
fn market_url(pubkey: &str) -> String {
    let pubkey = pubkey.trim();
    if pubkey.is_empty() {
        return "https://app.kamino.finance".to_string();
    }
    format!("https://app.kamino.finance/lending/{pubkey}")
}

/// Extract the market pubkey from a `…/lending/{market}` source URL path
/// (mirrors Go `marketFromSourceURL`). Returns empty when the path doesn't have
/// the expected `lending/{market}` shape.
fn market_from_source_url(source: &str) -> String {
    let raw = source.trim();
    if raw.is_empty() {
        return String::new();
    }
    let Ok(parsed) = reqwest::Url::parse(raw) else {
        return String::new();
    };
    let parts: Vec<&str> = parsed
        .path()
        .trim()
        .trim_matches('/')
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() < 2 || !parts[0].eq_ignore_ascii_case("lending") {
        return String::new();
    }
    parts[1].trim().to_string()
}

/// Parse a ratio string and scale it to a percentage point (×100). Mirrors Go
/// `ratioToPercent`; non-numeric/negative/non-finite inputs collapse to `0`.
fn ratio_to_percent(v: &str) -> f64 {
    parse_non_negative(v) * 100.0
}

/// Parse a non-negative finite float, returning `0.0` for any invalid, negative,
/// or non-finite input (mirrors Go `parseNonNegative`).
fn parse_non_negative(v: &str) -> f64 {
    match v.trim().parse::<f64>() {
        Ok(f) if f.is_finite() && f >= 0.0 => f,
        _ => 0.0,
    }
}

/// SHA-1 hex digest of the opportunity seed (mirrors Go `hashOpportunity`).
fn hash_opportunity(seed: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(seed.as_bytes());
    hex::encode(hasher.finalize())
}

/// Parse an RFC3339 timestamp into UTC, returning `None` on failure.
fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Percent-encode a query-param value (mirrors Go `url.QueryEscape`). The
/// timestamps and `hour`/`day` frequency are the only values escaped here; the
/// RFC3339 colon characters must be percent-encoded to match Go's behavior.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            // Unreserved per Go's `url.QueryEscape` (RFC 3986 unreserved set).
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            other => {
                out.push('%');
                out.push_str(&format!("{other:02X}"));
            }
        }
    }
    out
}

/// Compare two `f64` values for a DESCENDING sort with a deterministic,
/// panic-free total order (matches the Go `out[i] > out[j]` comparators, which
/// never see NaN in practice).
fn desc(a: f64, b: f64) -> std::cmp::Ordering {
    b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-providers::kamino`
    //!
    //! Go source: `internal/providers/kamino/client.go` (+ `client_test.go`).
    //! The Kamino REST API is mocked with `wiremock` (the Rust analogue of Go's
    //! `httptest`). Tests are deterministic and offline; the clock is pinned via
    //! `set_now`. Each test re-expresses one Go `client_test.go` case:
    //!
    //!   * `TestLendMarketsRejectsNonSolanaChain`
    //!   * `TestLendMarketsAndRatesFromKaminoAPI`
    //!   * `TestYieldOpportunitiesFiltersByAPYAndTVL`
    //!   * `TestLendMarketsPrefersMintMatchOverSymbol`
    //!   * `TestLendMarketsFailsWhenAnyMarketReserveFetchFails`
    //!   * `TestYieldHistoryFromSourceMarket`
    //!   * `TestYieldHistoryResolvesMarketFromReserve`
    //!
    //! Contract invariants asserted: Solana-only gating, APY in percentage
    //! points (×100), mint-over-symbol matching, deterministic TVL/APY ordering,
    //! single-100%-backing-asset yield shape, APY/TVL filtering, abort-on-any-
    //! market-fetch-failure, and history series construction from both an
    //! explicit `source_url` market and a reserve→market resolution.

    use std::time::Duration;

    use chrono::{TimeZone, Utc};
    use defi_httpx::Client as HttpClient;
    use defi_id::{parse_asset, parse_chain, Asset};
    use defi_model as model;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::kamino::Client;
    use crate::traits::{
        LendingProvider, Provider, YieldHistoryInterval, YieldHistoryMetric, YieldHistoryProvider,
        YieldHistoryRequest, YieldProvider, YieldRequest,
    };

    fn http() -> HttpClient {
        HttpClient::new(Duration::from_secs(2), 0)
    }

    fn client_at(base: &str) -> Client {
        let mut c = Client::new(http());
        c.set_base_url(base);
        c.set_now(Utc.with_ymd_and_hms(2026, 2, 26, 20, 0, 0).unwrap());
        c
    }

    fn yield_req(chain: defi_id::Chain, asset: Asset, limit: i64) -> YieldRequest {
        YieldRequest {
            chain,
            asset,
            limit,
            min_tvl_usd: 0.0,
            min_apy: 0.0,
            providers: vec!["kamino".to_string()],
            sort_by: "apy_total".to_string(),
            include_incomplete: false,
        }
    }

    fn opportunity(source_url: &str) -> model::YieldOpportunity {
        model::YieldOpportunity {
            opportunity_id: "opp-1".to_string(),
            provider: "kamino".to_string(),
            protocol: "kamino".to_string(),
            chain_id: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            asset_id:
                "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp/token:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
                    .to_string(),
            provider_native_id: "reserve-1".to_string(),
            provider_native_id_kind: model::NATIVE_ID_KIND_POOL_ID.to_string(),
            opportunity_type: String::new(),
            apy_base: 0.0,
            apy_reward: 0.0,
            apy_total: 0.0,
            tvl_usd: 0.0,
            liquidity_usd: 0.0,
            lockup_days: 0.0,
            withdrawal_terms: String::new(),
            backing_assets: Vec::new(),
            source_url: source_url.to_string(),
            fetched_at: String::new(),
        }
    }

    // ----- metadata -------------------------------------------------------

    #[test]
    fn info_is_metadata_only_no_key_required() {
        let client = Client::new(http());
        let info = Provider::info(&client);
        assert_eq!(info.name, "kamino");
        assert_eq!(info.provider_type, "lending+yield");
        assert!(!info.requires_key);
        for cap in [
            "lend.markets",
            "lend.rates",
            "yield.opportunities",
            "yield.history",
        ] {
            assert!(
                info.capabilities.iter().any(|c| c == cap),
                "missing capability {cap}"
            );
        }
    }

    // ----- TestLendMarketsRejectsNonSolanaChain ---------------------------

    #[tokio::test]
    async fn lend_markets_rejects_non_solana_chain() {
        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let client = Client::new(http());
        let err = client
            .lend_markets("kamino", chain, asset)
            .await
            .expect_err("expected unsupported chain error");
        assert_eq!(err.code, defi_errors::Code::Unsupported);
    }

    // ----- TestLendMarketsAndRatesFromKaminoAPI ---------------------------

    async fn mount_two_markets(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/v2/kamino-market"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"lendingMarket":"market-primary","name":"Main Market","isPrimary":true,"isCurated":false},
                    {"lendingMarket":"market-jup","name":"JUP Market","isPrimary":false,"isCurated":false}
                ]"#,
                "application/json",
            ))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/kamino-market/market-primary/reserves/metrics"))
            .and(query_param("env", "mainnet-beta"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"reserve":"reserve-usdc-main","liquidityToken":"USDC","liquidityTokenMint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","borrowApy":"0.045","supplyApy":"0.032","totalSupplyUsd":"1000000","totalBorrowUsd":"500000"}
                ]"#,
                "application/json",
            ))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/kamino-market/market-jup/reserves/metrics"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"reserve":"reserve-usdc-jup","liquidityToken":"USDC","liquidityTokenMint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","borrowApy":"0.025","supplyApy":"0.020","totalSupplyUsd":"2000000","totalBorrowUsd":"1000000"},
                    {"reserve":"reserve-sol-jup","liquidityToken":"SOL","liquidityTokenMint":"So11111111111111111111111111111111111111112","borrowApy":"0.01","supplyApy":"0.005","totalSupplyUsd":"100","totalBorrowUsd":"1"}
                ]"#,
                "application/json",
            ))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn lend_markets_and_rates_from_kamino_api() {
        let server = MockServer::start().await;
        mount_two_markets(&server).await;
        let client = client_at(&server.uri());

        let chain = parse_chain("solana").expect("parse solana");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");

        let markets = client
            .lend_markets("kamino", chain.clone(), asset.clone())
            .await
            .expect("lend markets");
        assert_eq!(markets.len(), 2, "expected 2 usdc markets");
        // Highest TVL first: the JUP USDC reserve at 2_000_000.
        assert_eq!(markets[0].tvl_usd, 2_000_000.0);
        // APY in percentage points: 0.020 -> 2.0.
        assert_eq!(markets[0].supply_apy, 2.0);
        assert_eq!(markets[0].provider, "kamino");
        assert_eq!(
            markets[0].provider_native_id_kind,
            model::NATIVE_ID_KIND_POOL_ID
        );
        assert!(!markets[0].provider_native_id.is_empty());

        let rates = client
            .lend_rates("kamino", chain, asset)
            .await
            .expect("lend rates");
        assert_eq!(rates.len(), 2, "expected 2 usdc rates");
        // Sorted by supply APY desc: main reserve 0.032 -> 3.2 first,
        // utilization = 500000/1000000 = 0.5.
        assert_eq!(rates[0].utilization, 0.5);
        assert_eq!(rates[0].provider, "kamino");
        assert_eq!(
            rates[0].provider_native_id_kind,
            model::NATIVE_ID_KIND_POOL_ID
        );
        assert!(!rates[0].provider_native_id.is_empty());
    }

    // ----- TestYieldOpportunitiesFiltersByAPYAndTVL -----------------------

    #[tokio::test]
    async fn yield_opportunities_filters_by_apy_and_tvl() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/kamino-market"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[{"lendingMarket":"market-primary","name":"Main Market","isPrimary":true,"isCurated":false}]"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/kamino-market/market-primary/reserves/metrics"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"reserve":"reserve-1","liquidityToken":"USDC","liquidityTokenMint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","borrowApy":"0.03","supplyApy":"0.04","totalSupplyUsd":"1000000","totalBorrowUsd":"400000"},
                    {"reserve":"reserve-2","liquidityToken":"USDC","liquidityTokenMint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","borrowApy":"0.02","supplyApy":"0.005","totalSupplyUsd":"1000","totalBorrowUsd":"200"}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let client = client_at(&server.uri());

        let chain = parse_chain("solana").expect("parse solana");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let mut req = yield_req(chain, asset, 10);
        req.min_tvl_usd = 50_000.0;
        req.min_apy = 1.0;

        let opps = client.yield_opportunities(req).await.expect("yield opps");
        assert_eq!(opps.len(), 1, "expected 1 filtered opportunity");
        assert_eq!(opps[0].provider, "kamino");
        assert_eq!(opps[0].protocol, "kamino");
        assert_eq!(
            opps[0].provider_native_id_kind,
            model::NATIVE_ID_KIND_POOL_ID
        );
        assert_eq!(opps[0].provider_native_id, "reserve-1");
        // APY total in percentage points: 0.04 -> 4.0.
        assert_eq!(opps[0].apy_total, 4.0);
        // liquidity = totalSupply - totalBorrow = 1_000_000 - 400_000.
        assert_eq!(opps[0].liquidity_usd, 600_000.0);
        assert_eq!(opps[0].backing_assets.len(), 1);
        assert_eq!(opps[0].backing_assets[0].share_pct, 100.0);
    }

    // ----- TestLendMarketsPrefersMintMatchOverSymbol ----------------------

    #[tokio::test]
    async fn lend_markets_prefers_mint_match_over_symbol() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/kamino-market"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[{"lendingMarket":"market-primary","name":"Main Market","isPrimary":true,"isCurated":false}]"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/kamino-market/market-primary/reserves/metrics"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"reserve":"reserve-usdc-other","liquidityToken":"USDC","liquidityTokenMint":"USDCwNeWRongMint111111111111111111111111111","borrowApy":"0.045","supplyApy":"0.032","totalSupplyUsd":"1000000","totalBorrowUsd":"500000"}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let client = client_at(&server.uri());

        let chain = parse_chain("solana").expect("parse solana");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let err = client
            .lend_markets("kamino", chain, asset)
            .await
            .expect_err("expected no market match due to mint mismatch");
        assert_eq!(err.code, defi_errors::Code::Unsupported);
    }

    // ----- TestLendMarketsFailsWhenAnyMarketReserveFetchFails -------------

    #[tokio::test]
    async fn lend_markets_fails_when_any_market_reserve_fetch_fails() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/kamino-market"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"lendingMarket":"market-good","name":"Good Market","isPrimary":true,"isCurated":false},
                    {"lendingMarket":"market-fail","name":"Fail Market","isPrimary":false,"isCurated":false}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/kamino-market/market-good/reserves/metrics"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"reserve":"reserve-usdc-good","liquidityToken":"USDC","liquidityTokenMint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","borrowApy":"0.03","supplyApy":"0.02","totalSupplyUsd":"1000000","totalBorrowUsd":"500000"}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/kamino-market/market-fail/reserves/metrics"))
            .respond_with(
                ResponseTemplate::new(503)
                    .set_body_raw(r#"{"error":"temporary failure"}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let client = client_at(&server.uri());

        let chain = parse_chain("solana").expect("parse solana");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let err = client
            .lend_markets("kamino", chain, asset)
            .await
            .expect_err("expected reserve fetch failure to fail command");
        // 503 from any market aborts the read.
        assert_eq!(err.code, defi_errors::Code::Unavailable);
    }

    // ----- TestYieldHistoryFromSourceMarket -------------------------------

    #[tokio::test]
    async fn yield_history_from_source_market() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/kamino-market/market-primary/reserves/reserve-1/metrics/history",
            ))
            .and(query_param("frequency", "hour"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "reserve":"reserve-1",
                    "history":[
                        {"timestamp":"2026-02-25T00:00:00Z","metrics":{"supplyInterestAPY":0.03,"depositTvl":"1000000"}},
                        {"timestamp":"2026-02-25T01:00:00Z","metrics":{"supplyInterestAPY":0.031,"depositTvl":"1100000"}}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let client = client_at(&server.uri());

        let req = YieldHistoryRequest {
            opportunity: opportunity("https://app.kamino.finance/lending/market-primary"),
            start_time: Utc.with_ymd_and_hms(2026, 2, 25, 0, 0, 0).unwrap(),
            end_time: Utc.with_ymd_and_hms(2026, 2, 25, 2, 0, 0).unwrap(),
            interval: YieldHistoryInterval::Hour,
            metrics: vec![YieldHistoryMetric::ApyTotal, YieldHistoryMetric::TvlUsd],
        };
        let series = client.yield_history(req).await.expect("yield history");
        assert_eq!(series.len(), 2, "expected two series");

        let apy = series
            .iter()
            .find(|s| s.metric == YieldHistoryMetric::ApyTotal.as_str())
            .expect("apy series");
        assert_eq!(apy.points.len(), 2);
        // 0.03 * 100 = 3.
        assert_eq!(apy.points[0].value, 3.0);

        let tvl = series
            .iter()
            .find(|s| s.metric == YieldHistoryMetric::TvlUsd.as_str())
            .expect("tvl series");
        assert_eq!(tvl.points.len(), 2);
        assert_eq!(tvl.points[1].value, 1_100_000.0);
    }

    // ----- TestYieldHistoryResolvesMarketFromReserve ----------------------

    #[tokio::test]
    async fn yield_history_resolves_market_from_reserve() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/kamino-market"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[{"lendingMarket":"market-primary","name":"Main Market","isPrimary":true,"isCurated":false}]"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/kamino-market/market-primary/reserves/metrics"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"reserve":"reserve-1","liquidityToken":"USDC","liquidityTokenMint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","borrowApy":"0.03","supplyApy":"0.04","totalSupplyUsd":"1000000","totalBorrowUsd":"400000"}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/kamino-market/market-primary/reserves/reserve-1/metrics/history",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"reserve":"reserve-1","history":[{"timestamp":"2026-02-25T00:00:00Z","metrics":{"supplyInterestAPY":0.03,"depositTvl":"1000000"}}]}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let client = client_at(&server.uri());

        // No source_url -> the client resolves the market by scanning reserves.
        let req = YieldHistoryRequest {
            opportunity: opportunity(""),
            start_time: Utc.with_ymd_and_hms(2026, 2, 25, 0, 0, 0).unwrap(),
            end_time: Utc.with_ymd_and_hms(2026, 2, 25, 2, 0, 0).unwrap(),
            interval: YieldHistoryInterval::Day,
            metrics: vec![YieldHistoryMetric::ApyTotal],
        };
        let series = client.yield_history(req).await.expect("yield history");
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].points.len(), 1);
    }

    // ----- pure-helper coverage -------------------------------------------

    #[test]
    fn ratio_to_percent_scales_and_guards() {
        assert_eq!(super::ratio_to_percent("0.032"), 3.2);
        assert_eq!(super::ratio_to_percent("-1"), 0.0);
        assert_eq!(super::ratio_to_percent("nope"), 0.0);
    }

    #[test]
    fn market_from_source_url_extracts_pubkey() {
        assert_eq!(
            super::market_from_source_url("https://app.kamino.finance/lending/market-primary"),
            "market-primary"
        );
        assert_eq!(super::market_from_source_url(""), "");
        assert_eq!(
            super::market_from_source_url("https://app.kamino.finance/other/market-primary"),
            ""
        );
    }
}
