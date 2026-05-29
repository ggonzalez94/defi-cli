//! Aave provider adapter — the canonical lending + yield adapter.
//!
//! Go source: `internal/providers/aave/client.go` (+ `client_test.go`).
//!
//! Implements the `LendingProvider` (markets/rates), `LendingPositionsProvider`,
//! `YieldProvider`, `YieldPositionsProvider`, and `YieldHistoryProvider` trait
//! surfaces, plus `Provider` metadata. Talks to the Aave GraphQL endpoint
//! (`https://api.v3.aave.com/graphql`). All outputs are deterministic (stable
//! multi-key sorts); every APY field is a PERCENTAGE POINT, not a ratio (spec
//! §2.5) — the GraphQL ratio values (`0.03`) are scaled ×100 to `3.0`.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_httpx::{do_body_json, Client as HttpClient};
use defi_id::{format_decimal, parse_chain, Asset, Chain};
use defi_model as model;
use reqwest::Method;
use serde::Deserialize;
use serde_json::json;
use sha1::{Digest, Sha1};

use crate::traits::{
    LendPositionType, LendPositionsRequest, LendingPositionsProvider, LendingProvider, Provider,
    YieldHistoryInterval, YieldHistoryMetric, YieldHistoryProvider, YieldHistoryRequest,
    YieldPositionsProvider, YieldPositionsRequest, YieldProvider, YieldRequest,
};
use crate::yieldutil;

/// Default Aave GraphQL endpoint.
const DEFAULT_ENDPOINT: &str = "https://api.v3.aave.com/graphql";
const SOURCE_URL: &str = "https://app.aave.com";

const MARKETS_QUERY: &str = r#"query Markets($request: MarketsRequest!) {
  markets(request: $request) {
    name
    address
    chain { chainId name }
    reserves {
      underlyingToken { address symbol decimals }
      aToken { address }
      size { usd }
      supplyInfo { apy { value } total { value } }
      borrowInfo { apy { value } total { usd } utilizationRate { value } availableLiquidity { usd } }
    }
  }
}"#;

const MARKET_ADDRESSES_QUERY: &str = r#"query MarketAddresses($request: MarketsRequest!) {
  markets(request: $request) {
    address
  }
}"#;

const POSITIONS_QUERY: &str = r#"query Positions($suppliesRequest: UserSuppliesRequest!, $borrowsRequest: UserBorrowsRequest!) {
  userSupplies(request: $suppliesRequest) {
    market { address }
    currency { address symbol decimals }
    balance { amount { raw decimals value } usd }
    apy { value }
    isCollateral
    canBeCollateral
  }
  userBorrows(request: $borrowsRequest) {
    market { address }
    currency { address symbol decimals }
    debt { amount { raw decimals value } usd }
    apy { value }
  }
}"#;

const SUPPLY_APY_HISTORY_QUERY: &str = r#"query SupplyAPYHistory($request: SupplyAPYHistoryRequest!) {
  supplyAPYHistory(request: $request) {
    date
    avgRate { value }
  }
}"#;

/// Aave lending + yield adapter (mirrors Go `aave.Client`).
pub struct Client {
    http: HttpClient,
    endpoint: String,
    /// Injected fixed clock for deterministic `fetched_at` / history-window
    /// selection / time-range filtering; `None` uses the wall clock.
    now: Option<DateTime<Utc>>,
}

impl Client {
    /// Build a client targeting the default Aave GraphQL endpoint (mirrors Go
    /// `New(httpClient)`).
    pub fn new(http: HttpClient) -> Self {
        Client {
            http,
            endpoint: DEFAULT_ENDPOINT.to_string(),
            now: None,
        }
    }

    /// Override the GraphQL endpoint (test seam for Go `client.endpoint`).
    pub fn set_endpoint(&mut self, url: &str) {
        self.endpoint = url.to_string();
    }

    /// Pin the clock (test seam for Go `client.now`).
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

    /// POST a GraphQL `body` to the endpoint and decode the JSON response.
    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        body: serde_json::Value,
        ctx: &'static str,
    ) -> Result<T, Error> {
        let bytes = serde_json::to_vec(&body).map_err(|e| Error::wrap(Code::Internal, ctx, e))?;
        let headers: HashMap<String, String> = HashMap::new();
        let resp = do_body_json::<T>(
            &self.http,
            Method::POST,
            &self.endpoint,
            Some(bytes),
            &headers,
        )
        .await?;
        Ok(resp.value)
    }

    async fn fetch_markets(&self, chain: &Chain) -> Result<Vec<AaveMarket>, Error> {
        if !chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "aave supports only EVM chains",
            ));
        }
        let body = json!({
            "query": MARKETS_QUERY,
            "variables": { "request": { "chainIds": [chain.evm_chain_id] } },
        });
        let resp: MarketsResponse = self.post(body, "marshal aave query").await?;
        if let Some(msg) = first_error(&resp.errors) {
            return Err(Error::new(
                Code::Unavailable,
                format!("aave graphql error: {msg}"),
            ));
        }
        if resp.data.markets.is_empty() {
            return Err(Error::new(
                Code::Unsupported,
                "aave has no market for requested chain",
            ));
        }
        Ok(resp.data.markets)
    }

    async fn fetch_market_addresses(&self, chain: &Chain) -> Result<Vec<String>, Error> {
        if !chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "aave supports only EVM chains",
            ));
        }
        let body = json!({
            "query": MARKET_ADDRESSES_QUERY,
            "variables": { "request": { "chainIds": [chain.evm_chain_id] } },
        });
        let resp: MarketAddressesResponse =
            self.post(body, "marshal aave market-address query").await?;
        if let Some(msg) = first_error(&resp.errors) {
            return Err(Error::new(
                Code::Unavailable,
                format!("aave graphql error: {msg}"),
            ));
        }
        if resp.data.markets.is_empty() {
            return Err(Error::new(
                Code::Unsupported,
                "aave has no market for requested chain",
            ));
        }
        let out: Vec<String> = resp
            .data
            .markets
            .iter()
            .filter_map(|m| {
                let addr = normalize_evm_address(&m.address);
                if addr.is_empty() {
                    None
                } else {
                    Some(addr)
                }
            })
            .collect();
        if out.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "aave market list returned no valid addresses",
            ));
        }
        Ok(out)
    }
}

impl Provider for Client {
    fn info(&self) -> model::ProviderInfo {
        model::ProviderInfo {
            name: "aave".to_string(),
            provider_type: "lending+yield".to_string(),
            requires_key: false,
            capabilities: vec![
                "lend.markets".to_string(),
                "lend.rates".to_string(),
                "lend.positions".to_string(),
                "yield.opportunities".to_string(),
                "yield.positions".to_string(),
                "yield.history".to_string(),
                "lend.plan".to_string(),
                "lend.execute".to_string(),
                "yield.plan".to_string(),
                "yield.execute".to_string(),
                "rewards.plan".to_string(),
                "rewards.execute".to_string(),
            ],
            key_env_var_name: String::new(),
            capability_auth: Vec::new(),
        }
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
        if !provider.eq_ignore_ascii_case("aave") {
            return Err(Error::new(
                Code::Unsupported,
                "aave adapter supports only provider=aave",
            ));
        }
        let markets = self.fetch_markets(&chain).await?;

        let mut out: Vec<model::LendMarket> = Vec::new();
        for m in &markets {
            for r in &m.reserves {
                if !matches_reserve_asset(r, &asset) {
                    continue;
                }
                let supply_apy = parse_float(&r.supply_info.apy.value) * 100.0;
                let borrow_apy = r
                    .borrow_info
                    .as_ref()
                    .map(|b| parse_float(&b.apy.value) * 100.0)
                    .unwrap_or(0.0);
                let tvl_usd = parse_float(&r.size.usd);
                if tvl_usd <= 0.0 {
                    continue;
                }
                out.push(model::LendMarket {
                    protocol: "aave".to_string(),
                    provider: "aave".to_string(),
                    chain_id: chain.caip2.clone(),
                    asset_id: canonical_asset_id(&asset, &r.underlying_token.address),
                    provider_native_id: provider_native_id(
                        "aave",
                        &chain.caip2,
                        &m.address,
                        &r.underlying_token.address,
                    ),
                    provider_native_id_kind: model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET
                        .to_string(),
                    supply_apy,
                    borrow_apy,
                    tvl_usd,
                    liquidity_usd: tvl_usd,
                    source_url: SOURCE_URL.to_string(),
                    fetched_at: self.fetched_at(),
                });
            }
        }

        out.sort_by(|a, b| {
            desc_f64(a.tvl_usd, b.tvl_usd).then_with(|| a.asset_id.cmp(&b.asset_id))
        });
        if out.is_empty() {
            return Err(Error::new(
                Code::Unsupported,
                "no aave lending market for requested chain/asset",
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
        if !provider.eq_ignore_ascii_case("aave") {
            return Err(Error::new(
                Code::Unsupported,
                "aave adapter supports only provider=aave",
            ));
        }
        let markets = self.fetch_markets(&chain).await?;

        let mut out: Vec<model::LendRate> = Vec::new();
        for m in &markets {
            for r in &m.reserves {
                if !matches_reserve_asset(r, &asset) {
                    continue;
                }
                let supply_apy = parse_float(&r.supply_info.apy.value) * 100.0;
                let (borrow_apy, utilization) = match &r.borrow_info {
                    Some(b) => (
                        parse_float(&b.apy.value) * 100.0,
                        parse_float(&b.utilization_rate.value),
                    ),
                    None => (0.0, 0.0),
                };
                out.push(model::LendRate {
                    protocol: "aave".to_string(),
                    provider: "aave".to_string(),
                    chain_id: chain.caip2.clone(),
                    asset_id: canonical_asset_id(&asset, &r.underlying_token.address),
                    provider_native_id: provider_native_id(
                        "aave",
                        &chain.caip2,
                        &m.address,
                        &r.underlying_token.address,
                    ),
                    provider_native_id_kind: model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET
                        .to_string(),
                    supply_apy,
                    borrow_apy,
                    utilization,
                    source_url: SOURCE_URL.to_string(),
                    fetched_at: self.fetched_at(),
                });
            }
        }

        out.sort_by(|a, b| {
            desc_f64(a.supply_apy, b.supply_apy).then_with(|| a.asset_id.cmp(&b.asset_id))
        });
        if out.is_empty() {
            return Err(Error::new(
                Code::Unsupported,
                "no aave lending rates for requested chain/asset",
            ));
        }
        Ok(out)
    }
}

#[async_trait]
impl LendingPositionsProvider for Client {
    async fn lend_positions(
        &self,
        req: LendPositionsRequest,
    ) -> Result<Vec<model::LendPosition>, Error> {
        if !req.chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "aave supports only EVM chains",
            ));
        }
        let account = normalize_evm_address(&req.account);
        if account.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "aave positions requires a valid EVM account address",
            ));
        }

        let market_addresses = self.fetch_market_addresses(&req.chain).await?;
        let markets: Vec<serde_json::Value> = market_addresses
            .iter()
            .map(|address| json!({ "address": address, "chainId": req.chain.evm_chain_id }))
            .collect();

        let body = json!({
            "query": POSITIONS_QUERY,
            "variables": {
                "suppliesRequest": {
                    "markets": markets,
                    "user": account,
                    "collateralsOnly": false,
                    "orderBy": { "balance": "DESC" },
                },
                "borrowsRequest": {
                    "markets": markets,
                    "user": account,
                    "orderBy": { "debt": "DESC" },
                },
            },
        });

        let resp: PositionsResponse = self.post(body, "marshal aave positions query").await?;
        if let Some(msg) = first_error(&resp.errors) {
            return Err(Error::new(
                Code::Unavailable,
                format!("aave graphql error: {msg}"),
            ));
        }

        let filter = req.position_type;
        let mut out: Vec<model::LendPosition> = Vec::new();
        for supply in &resp.data.user_supplies {
            let position_type = if supply.is_collateral {
                LendPositionType::Collateral
            } else {
                LendPositionType::Supply
            };
            if !matches_position_type(filter, position_type) {
                continue;
            }
            if !matches_position_asset(
                &supply.currency.address,
                &supply.currency.symbol,
                &req.asset,
            ) {
                continue;
            }
            let asset_id = canonical_asset_id_for_chain(&req.chain.caip2, &supply.currency.address);
            if asset_id.is_empty() {
                continue;
            }
            let amount = amount_info_from_raw(&supply.balance.amount.raw, supply.currency.decimals);
            out.push(model::LendPosition {
                protocol: "aave".to_string(),
                provider: "aave".to_string(),
                chain_id: req.chain.caip2.clone(),
                account_address: account.clone(),
                position_type: position_type.as_str().to_string(),
                asset_id,
                provider_native_id: provider_native_id(
                    "aave",
                    &req.chain.caip2,
                    &supply.market.address,
                    &supply.currency.address,
                ),
                provider_native_id_kind: model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET.to_string(),
                amount,
                amount_usd: parse_float(&supply.balance.usd),
                apy: parse_float(&supply.apy.value) * 100.0,
                source_url: SOURCE_URL.to_string(),
                fetched_at: self.fetched_at(),
            });
        }

        for borrow in &resp.data.user_borrows {
            if !matches_position_type(filter, LendPositionType::Borrow) {
                continue;
            }
            if !matches_position_asset(
                &borrow.currency.address,
                &borrow.currency.symbol,
                &req.asset,
            ) {
                continue;
            }
            let asset_id = canonical_asset_id_for_chain(&req.chain.caip2, &borrow.currency.address);
            if asset_id.is_empty() {
                continue;
            }
            let amount = amount_info_from_raw(&borrow.debt.amount.raw, borrow.currency.decimals);
            out.push(model::LendPosition {
                protocol: "aave".to_string(),
                provider: "aave".to_string(),
                chain_id: req.chain.caip2.clone(),
                account_address: account.clone(),
                position_type: LendPositionType::Borrow.as_str().to_string(),
                asset_id,
                provider_native_id: provider_native_id(
                    "aave",
                    &req.chain.caip2,
                    &borrow.market.address,
                    &borrow.currency.address,
                ),
                provider_native_id_kind: model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET.to_string(),
                amount,
                amount_usd: parse_float(&borrow.debt.usd),
                apy: parse_float(&borrow.apy.value) * 100.0,
                source_url: SOURCE_URL.to_string(),
                fetched_at: self.fetched_at(),
            });
        }

        sort_lend_positions(&mut out);
        if req.limit > 0 && (out.len() as i64) > req.limit {
            out.truncate(req.limit as usize);
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
        let markets = self.fetch_markets(&req.chain).await?;

        let mut out: Vec<model::YieldOpportunity> = Vec::new();
        for m in &markets {
            for r in &m.reserves {
                if !matches_reserve_asset(r, &req.asset) {
                    continue;
                }
                let apy = parse_float(&r.supply_info.apy.value) * 100.0;
                let tvl = parse_float(&r.size.usd);
                if (apy == 0.0 || tvl == 0.0) && !req.include_incomplete {
                    continue;
                }
                if apy < req.min_apy {
                    continue;
                }
                if tvl < req.min_tvl_usd {
                    continue;
                }

                let asset_id = canonical_asset_id(&req.asset, &r.underlying_token.address);
                let liquidity_usd = match &r.borrow_info {
                    Some(b) => parse_float(&b.available_liquidity.usd),
                    None => tvl,
                };
                let normalized_market = normalize_evm_address(&m.address);
                let normalized_underlying = normalize_evm_address(&r.underlying_token.address);
                let native_id = provider_native_id(
                    "aave",
                    &req.chain.caip2,
                    &normalized_market,
                    &normalized_underlying,
                );
                let opportunity_id =
                    hash_opportunity("aave", &req.chain.caip2, &native_id, &asset_id);
                out.push(model::YieldOpportunity {
                    opportunity_id,
                    provider: "aave".to_string(),
                    protocol: "aave".to_string(),
                    chain_id: req.chain.caip2.clone(),
                    asset_id: asset_id.clone(),
                    provider_native_id: native_id,
                    provider_native_id_kind: model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET
                        .to_string(),
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
                        symbol: r.underlying_token.symbol.trim().to_string(),
                        share_pct: 100.0,
                    }],
                    source_url: SOURCE_URL.to_string(),
                    fetched_at: self.fetched_at(),
                });
            }
        }

        if out.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "no aave yield opportunities for requested chain/asset",
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
impl YieldPositionsProvider for Client {
    async fn yield_positions(
        &self,
        req: YieldPositionsRequest,
    ) -> Result<Vec<model::YieldPosition>, Error> {
        let lend_rows = self
            .lend_positions(LendPositionsRequest {
                chain: req.chain.clone(),
                account: req.account.clone(),
                asset: req.asset.clone(),
                position_type: LendPositionType::All,
                limit: req.limit,
                rpc_url: String::new(),
            })
            .await?;

        let mut out: Vec<model::YieldPosition> = Vec::new();
        for row in &lend_rows {
            match row.position_type.as_str() {
                "supply" | "collateral" => {}
                _ => continue,
            }
            let opportunity_id = if row.provider_native_id.trim().is_empty() {
                String::new()
            } else {
                hash_opportunity(
                    "aave",
                    &row.chain_id,
                    &row.provider_native_id,
                    &row.asset_id,
                )
            };
            out.push(model::YieldPosition {
                protocol: "aave".to_string(),
                provider: "aave".to_string(),
                chain_id: row.chain_id.clone(),
                account_address: row.account_address.clone(),
                position_type: "deposit".to_string(),
                opportunity_id,
                asset_id: row.asset_id.clone(),
                provider_native_id: row.provider_native_id.clone(),
                provider_native_id_kind: row.provider_native_id_kind.clone(),
                amount: row.amount.clone(),
                shares: None,
                amount_usd: row.amount_usd,
                apy_total: row.apy,
                source_url: row.source_url.clone(),
                fetched_at: row.fetched_at.clone(),
            });
        }

        sort_yield_positions(&mut out);
        if req.limit > 0 && (out.len() as i64) > req.limit {
            out.truncate(req.limit as usize);
        }
        Ok(out)
    }
}

#[async_trait]
impl YieldHistoryProvider for Client {
    async fn yield_history(
        &self,
        req: YieldHistoryRequest,
    ) -> Result<Vec<model::YieldHistorySeries>, Error> {
        if !req.opportunity.provider.trim().eq_ignore_ascii_case("aave") {
            return Err(Error::new(
                Code::Unsupported,
                "aave history supports only aave opportunities",
            ));
        }
        if req.start_time >= req.end_time {
            return Err(Error::new(
                Code::Usage,
                "history start time must be before end time",
            ));
        }
        for metric in &req.metrics {
            if *metric != YieldHistoryMetric::ApyTotal {
                return Err(Error::new(
                    Code::Unsupported,
                    "aave history supports only metric=apy_total",
                ));
            }
        }

        let chain = parse_chain(&req.opportunity.chain_id)
            .map_err(|e| Error::wrap(Code::Usage, "parse aave opportunity chain", e))?;
        if !chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "aave supports only EVM chains",
            ));
        }

        let (market_address, underlying_address) = parse_opportunity_native_id(&req.opportunity)?;
        let window = history_window(req.start_time, req.end_time, self.now())?;

        let body = json!({
            "query": SUPPLY_APY_HISTORY_QUERY,
            "variables": {
                "request": {
                    "market": market_address,
                    "underlyingToken": underlying_address,
                    "window": window,
                    "chainId": chain.evm_chain_id,
                },
            },
        });

        let resp: SupplyApyHistoryResponse = self.post(body, "marshal aave history query").await?;
        if let Some(msg) = first_error(&resp.errors) {
            return Err(Error::new(
                Code::Unavailable,
                format!("aave graphql error: {msg}"),
            ));
        }

        let mut points: Vec<model::YieldHistoryPoint> = Vec::new();
        for sample in &resp.data.supply_apy_history {
            let ts = match parse_api_time(&sample.date) {
                Some(ts) => ts,
                None => continue,
            };
            if ts < req.start_time || ts > req.end_time {
                continue;
            }
            points.push(model::YieldHistoryPoint {
                timestamp: ts.to_rfc3339_opts(SecondsFormat::Secs, true),
                value: parse_float(&sample.avg_rate.value) * 100.0,
            });
        }
        if req.interval == YieldHistoryInterval::Day {
            points = average_points_by_day(points);
        } else {
            sort_history_points(&mut points);
        }
        if points.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "no aave historical points for requested range",
            ));
        }

        Ok(vec![model::YieldHistorySeries {
            opportunity_id: req.opportunity.opportunity_id.clone(),
            provider: "aave".to_string(),
            protocol: req.opportunity.protocol.clone(),
            chain_id: req.opportunity.chain_id.clone(),
            asset_id: req.opportunity.asset_id.clone(),
            provider_native_id: req.opportunity.provider_native_id.clone(),
            provider_native_id_kind: req.opportunity.provider_native_id_kind.clone(),
            metric: YieldHistoryMetric::ApyTotal.as_str().to_string(),
            interval: req.interval.as_str().to_string(),
            start_time: req.start_time.to_rfc3339_opts(SecondsFormat::Secs, true),
            end_time: req.end_time.to_rfc3339_opts(SecondsFormat::Secs, true),
            points,
            source_url: req.opportunity.source_url.clone(),
            fetched_at: self.fetched_at(),
        }])
    }
}

// --- GraphQL response shapes (deserialize-only) ---

#[derive(Debug, Deserialize)]
struct GraphqlError {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Default, Deserialize)]
struct ValueField {
    #[serde(default)]
    value: String,
}

#[derive(Debug, Default, Deserialize)]
struct UsdField {
    #[serde(default)]
    usd: String,
}

#[derive(Debug, Deserialize)]
struct MarketsResponse {
    #[serde(default)]
    data: MarketsData,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Default, Deserialize)]
struct MarketsData {
    #[serde(default)]
    markets: Vec<AaveMarket>,
}

#[derive(Debug, Deserialize)]
struct MarketAddressesResponse {
    #[serde(default)]
    data: MarketAddressesData,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Default, Deserialize)]
struct MarketAddressesData {
    #[serde(default)]
    markets: Vec<MarketAddressOnly>,
}

#[derive(Debug, Deserialize)]
struct MarketAddressOnly {
    #[serde(default)]
    address: String,
}

#[derive(Debug, Deserialize)]
struct AaveMarket {
    #[serde(default)]
    address: String,
    #[serde(default)]
    reserves: Vec<AaveReserve>,
}

#[derive(Debug, Deserialize)]
struct AaveReserve {
    #[serde(rename = "underlyingToken", default)]
    underlying_token: TokenInfo,
    #[serde(default)]
    size: UsdField,
    #[serde(rename = "supplyInfo", default)]
    supply_info: SupplyInfo,
    #[serde(rename = "borrowInfo")]
    borrow_info: Option<BorrowInfo>,
}

#[derive(Debug, Default, Deserialize)]
struct TokenInfo {
    #[serde(default)]
    address: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    decimals: i64,
}

#[derive(Debug, Default, Deserialize)]
struct SupplyInfo {
    #[serde(default)]
    apy: ValueField,
}

#[derive(Debug, Default, Deserialize)]
struct BorrowInfo {
    #[serde(default)]
    apy: ValueField,
    #[serde(rename = "utilizationRate", default)]
    utilization_rate: ValueField,
    #[serde(rename = "availableLiquidity", default)]
    available_liquidity: UsdField,
}

#[derive(Debug, Deserialize)]
struct PositionsResponse {
    #[serde(default)]
    data: PositionsData,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Default, Deserialize)]
struct PositionsData {
    #[serde(rename = "userSupplies", default)]
    user_supplies: Vec<UserSupply>,
    #[serde(rename = "userBorrows", default)]
    user_borrows: Vec<UserBorrow>,
}

#[derive(Debug, Deserialize)]
struct MarketRef {
    #[serde(default)]
    address: String,
}

#[derive(Debug, Default, Deserialize)]
struct AmountRaw {
    #[serde(default)]
    raw: String,
}

#[derive(Debug, Default, Deserialize)]
struct BalanceField {
    #[serde(default)]
    amount: AmountRaw,
    #[serde(default)]
    usd: String,
}

#[derive(Debug, Deserialize)]
struct UserSupply {
    market: MarketRef,
    currency: TokenInfo,
    balance: BalanceField,
    #[serde(default)]
    apy: ValueField,
    #[serde(rename = "isCollateral", default)]
    is_collateral: bool,
}

#[derive(Debug, Deserialize)]
struct UserBorrow {
    market: MarketRef,
    currency: TokenInfo,
    debt: BalanceField,
    #[serde(default)]
    apy: ValueField,
}

#[derive(Debug, Deserialize)]
struct SupplyApyHistoryResponse {
    #[serde(default)]
    data: SupplyApyHistoryData,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Default, Deserialize)]
struct SupplyApyHistoryData {
    #[serde(rename = "supplyAPYHistory", default)]
    supply_apy_history: Vec<HistorySample>,
}

#[derive(Debug, Deserialize)]
struct HistorySample {
    #[serde(default)]
    date: String,
    #[serde(rename = "avgRate", default)]
    avg_rate: ValueField,
}

// --- helpers (mirror the package-private Go helpers) ---

fn first_error(errors: &[GraphqlError]) -> Option<&str> {
    errors.first().map(|e| e.message.as_str())
}

fn matches_reserve_asset(r: &AaveReserve, asset: &Asset) -> bool {
    let asset_address = asset.address.trim();
    if !asset_address.is_empty() {
        return r
            .underlying_token
            .address
            .trim()
            .eq_ignore_ascii_case(asset_address);
    }
    r.underlying_token
        .symbol
        .trim()
        .eq_ignore_ascii_case(asset.symbol.trim())
}

fn canonical_asset_id(asset: &Asset, address: &str) -> String {
    let addr = address.trim().to_ascii_lowercase();
    if addr.is_empty() {
        return asset.asset_id.clone();
    }
    format!("{}/erc20:{addr}", asset.chain_id)
}

fn canonical_asset_id_for_chain(chain_id: &str, address: &str) -> String {
    let addr = normalize_evm_address(address);
    if chain_id.is_empty() || addr.is_empty() {
        return String::new();
    }
    format!("{chain_id}/erc20:{addr}")
}

fn normalize_evm_address(address: &str) -> String {
    let addr = address.trim().to_ascii_lowercase();
    if addr.len() != 42 || !addr.starts_with("0x") {
        return String::new();
    }
    addr
}

fn provider_native_id(
    provider: &str,
    chain_id: &str,
    market_address: &str,
    underlying_address: &str,
) -> String {
    format!(
        "{provider}:{chain_id}:{}:{}",
        normalize_evm_address(market_address),
        normalize_evm_address(underlying_address)
    )
}

fn parse_opportunity_native_id(op: &model::YieldOpportunity) -> Result<(String, String), Error> {
    let native_id = op.provider_native_id.trim();
    if native_id.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "aave opportunity missing provider_native_id",
        ));
    }
    let prefix = format!("aave:{}:", op.chain_id.trim());
    if !native_id
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
    {
        return Err(Error::new(
            Code::Usage,
            "invalid aave provider_native_id format",
        ));
    }
    let suffix = &native_id[prefix.len()..];
    let parts: Vec<&str> = suffix.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(Error::new(
            Code::Usage,
            "invalid aave provider_native_id format",
        ));
    }
    let market_address = normalize_evm_address(parts[0]);
    let underlying_address = normalize_evm_address(parts[1]);
    if market_address.is_empty() || underlying_address.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "invalid aave provider_native_id addresses",
        ));
    }
    Ok((market_address, underlying_address))
}

fn history_window(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<&'static str, Error> {
    if end < now - chrono::Duration::hours(2) {
        return Err(Error::new(
            Code::Unsupported,
            "aave history supports lookback windows ending near now",
        ));
    }
    let span = end - start;
    let day = chrono::Duration::hours(24);
    if span <= day {
        Ok("LAST_DAY")
    } else if span <= day * 7 {
        Ok("LAST_WEEK")
    } else if span <= day * 31 {
        Ok("LAST_MONTH")
    } else if span <= day * 183 {
        Ok("LAST_SIX_MONTHS")
    } else if span <= day * 366 {
        Ok("LAST_YEAR")
    } else {
        Err(Error::new(
            Code::Unsupported,
            "aave history supports windows up to 1 year",
        ))
    }
}

fn parse_api_time(v: &str) -> Option<DateTime<Utc>> {
    let raw = v.trim();
    if raw.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn sort_history_points(points: &mut [model::YieldHistoryPoint]) {
    points.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
}

fn average_points_by_day(
    mut points: Vec<model::YieldHistoryPoint>,
) -> Vec<model::YieldHistoryPoint> {
    if points.is_empty() {
        return Vec::new();
    }
    sort_history_points(&mut points);
    let mut by_day: HashMap<String, (f64, i64)> = HashMap::new();
    for point in &points {
        let ts = match DateTime::parse_from_rfc3339(&point.timestamp) {
            Ok(ts) => ts.with_timezone(&Utc),
            Err(_) => continue,
        };
        let day = ts.format("%Y-%m-%d").to_string();
        let entry = by_day.entry(day).or_insert((0.0, 0));
        entry.0 += point.value;
        entry.1 += 1;
    }
    let mut days: Vec<String> = by_day.keys().cloned().collect();
    days.sort();
    let mut out = Vec::with_capacity(days.len());
    for day in days {
        let (sum, count) = by_day[&day];
        if count == 0 {
            continue;
        }
        out.push(model::YieldHistoryPoint {
            timestamp: format!("{day}T00:00:00Z"),
            value: sum / count as f64,
        });
    }
    out
}

fn matches_position_type(filter: LendPositionType, position: LendPositionType) -> bool {
    if filter == LendPositionType::All {
        return true;
    }
    filter == position
}

fn matches_position_asset(address: &str, symbol: &str, asset: &Asset) -> bool {
    if !asset.address.trim().is_empty() {
        return address.trim().eq_ignore_ascii_case(asset.address.trim());
    }
    if !asset.symbol.trim().is_empty() {
        return symbol.trim().eq_ignore_ascii_case(asset.symbol.trim());
    }
    true
}

fn amount_info_from_raw(raw: &str, decimals: i64) -> model::AmountInfo {
    let decimals = decimals.max(0);
    let base = normalize_base_units(raw);
    let amount_decimal = format_decimal(&base, decimals as i32);
    model::AmountInfo {
        amount_base_units: base,
        amount_decimal,
        decimals,
    }
}

fn normalize_base_units(v: &str) -> String {
    let clean = v.trim();
    if clean.is_empty() {
        return "0".to_string();
    }
    if clean.chars().all(|c| c.is_ascii_digit()) {
        clean.to_string()
    } else {
        "0".to_string()
    }
}

fn sort_lend_positions(items: &mut [model::LendPosition]) {
    items.sort_by(|a, b| {
        desc_f64(a.amount_usd, b.amount_usd)
            .then_with(|| a.position_type.cmp(&b.position_type))
            .then_with(|| a.asset_id.cmp(&b.asset_id))
            .then_with(|| a.provider_native_id.cmp(&b.provider_native_id))
    });
}

fn sort_yield_positions(items: &mut [model::YieldPosition]) {
    items.sort_by(|a, b| {
        desc_f64(a.amount_usd, b.amount_usd)
            .then_with(|| desc_f64(a.apy_total, b.apy_total))
            .then_with(|| a.asset_id.cmp(&b.asset_id))
            .then_with(|| a.provider_native_id.cmp(&b.provider_native_id))
    });
}

/// Compare two finite-ish `f64` values for a DESCENDING sort, total-order safe.
fn desc_f64(a: f64, b: f64) -> std::cmp::Ordering {
    b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
}

fn parse_float(v: &str) -> f64 {
    match v.trim().parse::<f64>() {
        Ok(f) if f.is_finite() => f,
        _ => 0.0,
    }
}

fn hash_opportunity(provider: &str, chain_id: &str, market_id: &str, asset_id: &str) -> String {
    let seed = [provider, chain_id, market_id, asset_id].join("|");
    let mut hasher = Sha1::new();
    hasher.update(seed.as_bytes());
    let digest = hasher.finalize();
    hex::encode(digest)
}

#[cfg(test)]
#[allow(clippy::doc_lazy_continuation)]
mod tests {
    //! # Success criteria for the `aave` provider adapter
    //!
    //! Go source: `internal/providers/aave/client.go`; ported behavioral cases
    //! from `internal/providers/aave/client_test.go`. External HTTP (Aave's
    //! GraphQL endpoint, `https://api.v3.aave.com/graphql`) is mocked with
    //! `wiremock` (the Rust analogue of Go's `httptest.Server`).
    //!
    //! Aave is the canonical lending + yield adapter. It implements the
    //! `LendingProvider` (markets/rates), `LendingPositionsProvider`,
    //! `YieldProvider`, `YieldPositionsProvider`, and `YieldHistoryProvider`
    //! trait surfaces, plus `Provider` metadata. All outputs are deterministic
    //! (stable multi-key sorts) and every numeric APY field is a PERCENTAGE
    //! POINT, not a ratio (spec §2.5): the adapter multiplies the GraphQL ratio
    //! values (`0.03`) by 100 to get the contract value (`3.0`).
    //!
    //! The `Client` exposes two test seams mirroring the package-private fields
    //! the Go tests poke:
    //!   * `set_endpoint(&url)` — overrides the GraphQL endpoint to point at a
    //!     `wiremock::MockServer` (Go `client.endpoint = srv.URL`).
    //!   * `set_now(DateTime<Utc>)` — pins the clock for `fetched_at`,
    //!     history-window selection, and time-range filtering (Go
    //!     `client.now = func() time.Time { ... }`).
    //! The constructor mirrors Go `New(httpClient)` (single arg; the endpoint
    //! defaults to the real Aave GraphQL URL).
    //!
    //! ## Criteria
    //!
    //!  A0. **Provider metadata** (`Provider::info`). `name == "aave"`,
    //!      `provider_type == "lending+yield"`, `requires_key == false`,
    //!      capabilities include `lend.markets`, `lend.positions`,
    //!      `yield.opportunities`, `yield.positions`, `yield.history`. Callable
    //!      as metadata WITHOUT any key (spec §2.5).
    //!
    //!  A1. **LendMarkets** (Go `TestLendMarketsAndYield`). POSTs the markets
    //!      GraphQL query; for each matching reserve emits a `LendMarket` with
    //!      `protocol == provider == "aave"`, `chain_id` = chain CAIP-2,
    //!      `provider_native_id` non-empty + `provider_native_id_kind ==
    //!      composite_market_asset`. APY ratios are scaled ×100
    //!      (`supplyInfo.apy 0.03 -> supply_apy 3.0`). Reserves with non-positive
    //!      `size.usd` are dropped. Sorted by TVL desc, then asset_id asc. Empty
    //!      result -> typed `Unsupported` error.
    //!
    //!  A2. **LendMarkets prefers address match over symbol** (Go
    //!      `TestLendMarketsPrefersAddressMatchOverSymbol`). When the resolved
    //!      asset carries an address (e.g. `USDC` resolves to its canonical
    //!      ethereum address via `parse_asset`), a reserve whose underlying token
    //!      address differs is NOT matched even when the SYMBOL matches -> the
    //!      call returns a typed `Unsupported` error (no market).
    //!
    //!  A3. **LendMarkets rejects a foreign provider name.** `lend_markets` is
    //!      called with the routed provider string; any value other than `aave`
    //!      (case-insensitive) returns a typed `Unsupported` error and does NOT
    //!      hit the network. (Go guard at the top of `LendMarkets`.)
    //!
    //!  A4. **LendRates** sorts by supply APY desc then asset_id asc, scales APY
    //!      ×100, and carries `utilization` from `borrowInfo.utilizationRate`
    //!      (NOT ×100 — utilization is passed through verbatim). Empty -> typed
    //!      `Unsupported`. (Go `LendRates`, same routing guard as A3.)
    //!
    //!  A5. **YieldOpportunities** (Go `TestLendMarketsAndYield`). Emits a single
    //!      `lend`-type opportunity per matching reserve with
    //!      `provider == protocol == "aave"`, a deterministic `opportunity_id`
    //!      (sha1 hex of `provider|chain|native_id|asset_id`), `apy_total ==
    //!      apy_base == supply_apy` and `apy_reward == 0`, `liquidity_usd` taken
    //!      from `borrowInfo.availableLiquidity.usd` (`600000`) when present, and
    //!      exactly one backing asset at `share_pct == 100`. Sorted via the
    //!      shared yield sort; honors `limit`. Empty -> typed `Unavailable`.
    //!
    //!  A6. **LendPositions type split** (Go `TestLendPositionsTypeSplit`). First
    //!      POSTs the market-addresses query, then the positions query. A
    //!      non-collateral supply -> `supply`; a collateral supply ->
    //!      `collateral`; a borrow -> `borrow`. With `type=all`, all three are
    //!      returned (non-overlapping intents). `type=supply` returns ONLY the
    //!      non-collateral supply; `type=collateral` returns ONLY the collateral
    //!      row. Each carries `provider_native_id_kind == composite_market_asset`
    //!      and an `amount` whose `amount_base_units` is the raw balance and
    //!      `amount_decimal` is the decimal-scaled form.
    //!
    //!  A7. **LendPositions rejects non-EVM chains and missing account.** A
    //!      non-EVM chain -> typed `Unsupported`; an empty / non-hex account ->
    //!      typed `Usage`. (Go guards at the top of `LendPositions`.)
    //!
    //!  A8. **YieldPositions** (Go `TestLendPositionsTypeSplit`). Derived from
    //!      `LendPositions(type=all)`: only `supply`/`collateral` rows become
    //!      yield rows; borrows are dropped. Each yield row has
    //!      `position_type == "deposit"` and `provider_native_id_kind ==
    //!      composite_market_asset`. With one supply + one collateral + one
    //!      borrow input, exactly TWO yield rows are produced.
    //!
    //!  A9. **YieldHistory APY** (Go `TestYieldHistoryAPY`). POSTs the
    //!      supplyAPYHistory query whose body embeds the correct window
    //!      (`"window":"LAST_DAY"` for a sub-24h span). Returns one series with
    //!      `metric == "apy_total"`, points scaled ×100 (`avgRate 0.02 -> 2.0`),
    //!      filtered to `[start, end]`, preserving the series metadata
    //!      (`opportunity_id`, `chain_id`, `provider_native_id`, etc.) from the
    //!      request opportunity.
    //!
    //!  A10. **YieldHistory rejects unsupported metric** (Go
    //!       `TestYieldHistoryRejectsUnsupportedMetric`). A metric other than
    //!       `apy_total` (e.g. `tvl_usd`) -> typed error, and the call does NOT
    //!       hit the network.
    //!
    //! ## Go tests intentionally SKIPPED here (owned elsewhere / not this module)
    //!   * `yieldutil.Sort` determinism — owned by the `yieldutil` module's own
    //!     RED suite, not the aave adapter (A5 only asserts that the sort is
    //!     APPLIED + `limit` honored, not its tie-break internals).
    //!   * `New`/struct-field plumbing details (Go pokes package-private
    //!     `endpoint`/`now`) — re-expressed as the idiomatic `set_endpoint` /
    //!     `set_now` test seams above, not a 1:1 field-poke.
    //!   * Low-level helper internals (`parseFloat`, `normalizeBaseUnits`,
    //!     `hashOpportunity`, `historyWindow` switch arms) — exercised indirectly
    //!     through the public method assertions, not as private-fn unit tests.

    use std::time::Duration;

    use chrono::{TimeZone, Utc};
    use defi_errors::Code;
    use defi_httpx::Client as HttpClient;
    use defi_id::{parse_asset, parse_chain};
    use defi_model as model;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::aave::Client;
    use crate::traits::{
        LendPositionType, LendPositionsRequest, LendingPositionsProvider, LendingProvider,
        Provider, YieldHistoryInterval, YieldHistoryMetric, YieldHistoryProvider,
        YieldHistoryRequest, YieldPositionsProvider, YieldPositionsRequest, YieldProvider,
        YieldRequest,
    };

    fn http() -> HttpClient {
        HttpClient::new(Duration::from_secs(2), 0)
    }

    /// The canonical ethereum USDC address (matches `parse_asset("USDC", eth)`).
    const USDC_ETH: &str = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";

    /// Build a `YieldRequest` carrying only the fields the aave path reads.
    fn yield_req(chain: defi_id::Chain, asset: defi_id::Asset, limit: i64) -> YieldRequest {
        YieldRequest {
            chain,
            asset,
            limit,
            min_tvl_usd: 0.0,
            min_apy: 0.0,
            providers: vec!["aave".to_string()],
            sort_by: String::new(),
            include_incomplete: false,
        }
    }

    // ----- A0: provider metadata (callable without a key) ------------------

    #[test]
    fn info_is_metadata_only_no_key_required() {
        let client = Client::new(http());
        let info = client.info();
        assert_eq!(info.name, "aave");
        assert_eq!(info.provider_type, "lending+yield");
        assert!(!info.requires_key);
        for cap in [
            "lend.markets",
            "lend.rates",
            "lend.positions",
            "yield.opportunities",
            "yield.positions",
            "yield.history",
        ] {
            assert!(
                info.capabilities.iter().any(|c| c == cap),
                "expected capability {cap}, got {:?}",
                info.capabilities
            );
        }
    }

    // ----- A1: LendMarkets -------------------------------------------------

    /// The markets GraphQL response used by A1 + A5 (USDC reserve with full
    /// supply/borrow info). Mirrors the Go `TestLendMarketsAndYield` fixture.
    fn markets_body() -> String {
        format!(
            r#"{{
                "data": {{
                    "markets": [
                        {{
                            "name": "AaveV3Ethereum",
                            "address": "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2",
                            "chain": {{"chainId": 1, "name": "Ethereum"}},
                            "reserves": [
                                {{
                                    "underlyingToken": {{"address": "{USDC_ETH}", "symbol": "USDC", "decimals": 6}},
                                    "aToken": {{"address": "0x71Aef7b30728b9BB371578f36c5A1f1502a5723e"}},
                                    "size": {{"usd": "1000000"}},
                                    "supplyInfo": {{"apy": {{"value": "0.03"}}, "total": {{"value": "1000000"}}}},
                                    "borrowInfo": {{"apy": {{"value": "0.05"}}, "total": {{"usd": "500000"}}, "utilizationRate": {{"value": "0.4"}}, "availableLiquidity": {{"usd": "600000"}}}}
                                }}
                            ]
                        }}
                    ]
                }}
            }}"#
        )
    }

    #[tokio::test]
    async fn lend_markets_scales_apy_and_carries_native_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(markets_body(), "application/json"),
            )
            .mount(&server)
            .await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let markets = client
            .lend_markets("aave", chain.clone(), asset)
            .await
            .expect("lend_markets");
        assert_eq!(markets.len(), 1);
        let m = &markets[0];
        assert_eq!(m.protocol, "aave");
        assert_eq!(m.provider, "aave");
        assert_eq!(m.chain_id, chain.caip2);
        // 0.03 ratio -> 3.0 percentage points (spec §2.5).
        assert_eq!(m.supply_apy, 3.0);
        assert_eq!(m.borrow_apy, 5.0);
        assert_eq!(m.tvl_usd, 1_000_000.0);
        assert!(!m.provider_native_id.is_empty(), "native id present");
        assert_eq!(
            m.provider_native_id_kind,
            model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET
        );
    }

    #[tokio::test]
    async fn lend_markets_drops_non_positive_tvl_and_errors_empty() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{
                        "data": {{
                            "markets": [
                                {{
                                    "name": "AaveV3Ethereum",
                                    "address": "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2",
                                    "chain": {{"chainId": 1, "name": "Ethereum"}},
                                    "reserves": [
                                        {{
                                            "underlyingToken": {{"address": "{USDC_ETH}", "symbol": "USDC", "decimals": 6}},
                                            "size": {{"usd": "0"}},
                                            "supplyInfo": {{"apy": {{"value": "0.03"}}, "total": {{"value": "0"}}}}
                                        }}
                                    ]
                                }}
                            ]
                        }}
                    }}"#
                ),
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let err = client
            .lend_markets("aave", chain, asset)
            .await
            .expect_err("zero-tvl reserve must yield no market");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- A2: address match preferred over symbol -------------------------

    #[tokio::test]
    async fn lend_markets_prefers_address_match_over_symbol() {
        let server = MockServer::start().await;
        // Same symbol (USDC) but a DIFFERENT underlying address than the
        // resolved asset's canonical ethereum address.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "data": {
                        "markets": [
                            {
                                "name": "AaveV3Ethereum",
                                "chain": {"chainId": 1, "name": "Ethereum"},
                                "reserves": [
                                    {
                                        "underlyingToken": {"address": "0x0000000000000000000000000000000000000001", "symbol": "USDC", "decimals": 6},
                                        "size": {"usd": "1000000"},
                                        "supplyInfo": {"apy": {"value": "0.03"}, "total": {"value": "1000000"}},
                                        "borrowInfo": {"apy": {"value": "0.05"}, "total": {"usd": "500000"}, "utilizationRate": {"value": "0.4"}}
                                    }
                                ]
                            }
                        ]
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        // Sanity: the resolved asset DOES carry an address (so address-match wins).
        assert!(!asset.address.is_empty(), "USDC must resolve to an address");

        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let err = client
            .lend_markets("aave", chain, asset)
            .await
            .expect_err("address mismatch must yield no market");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- A3: routing guard rejects a foreign provider --------------------

    #[tokio::test]
    async fn lend_markets_rejects_foreign_provider_without_network() {
        // No mock mounted: if the adapter hit the network it would error on the
        // connection, not on the routing guard. The guard must fire first.
        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let mut client = Client::new(http());
        client.set_endpoint("http://127.0.0.1:0"); // unroutable on purpose

        let err = client
            .lend_markets("morpho", chain, asset)
            .await
            .expect_err("foreign provider must be rejected");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- A4: LendRates ---------------------------------------------------

    #[tokio::test]
    async fn lend_rates_scales_apy_and_passes_utilization_through() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(markets_body(), "application/json"),
            )
            .mount(&server)
            .await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let rates = client
            .lend_rates("aave", chain, asset)
            .await
            .expect("lend_rates");
        assert_eq!(rates.len(), 1);
        let r = &rates[0];
        assert_eq!(r.protocol, "aave");
        assert_eq!(r.supply_apy, 3.0); // 0.03 * 100
        assert_eq!(r.borrow_apy, 5.0); // 0.05 * 100
                                       // utilizationRate 0.4 is passed through verbatim (NOT * 100).
        assert_eq!(r.utilization, 0.4);
        assert_eq!(
            r.provider_native_id_kind,
            model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET
        );
    }

    // ----- A5: YieldOpportunities ------------------------------------------

    #[tokio::test]
    async fn yield_opportunities_emits_lend_opportunity_with_liquidity_and_backing() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(markets_body(), "application/json"),
            )
            .mount(&server)
            .await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let opps = client
            .yield_opportunities(yield_req(chain, asset, 10))
            .await
            .expect("yield_opportunities");
        assert_eq!(opps.len(), 1);
        let o = &opps[0];
        assert_eq!(o.provider, "aave");
        assert_eq!(o.protocol, "aave");
        assert_eq!(o.opportunity_type, "lend");
        assert!(!o.opportunity_id.is_empty(), "deterministic id present");
        assert_eq!(o.apy_base, 3.0);
        assert_eq!(o.apy_reward, 0.0);
        assert_eq!(o.apy_total, 3.0);
        assert_eq!(o.tvl_usd, 1_000_000.0);
        // liquidity_usd comes from borrowInfo.availableLiquidity.usd.
        assert_eq!(o.liquidity_usd, 600_000.0);
        assert!(!o.provider_native_id.is_empty());
        assert_eq!(
            o.provider_native_id_kind,
            model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET
        );
        assert_eq!(o.backing_assets.len(), 1);
        assert_eq!(o.backing_assets[0].share_pct, 100.0);
        assert_eq!(o.backing_assets[0].symbol, "USDC");
    }

    #[tokio::test]
    async fn yield_opportunities_empty_is_unavailable() {
        let server = MockServer::start().await;
        // A market with no reserves matching the requested asset.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "data": {
                        "markets": [
                            {
                                "name": "AaveV3Ethereum",
                                "address": "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2",
                                "chain": {"chainId": 1, "name": "Ethereum"},
                                "reserves": [
                                    {
                                        "underlyingToken": {"address": "0x0000000000000000000000000000000000000099", "symbol": "WBTC", "decimals": 8},
                                        "size": {"usd": "1000000"},
                                        "supplyInfo": {"apy": {"value": "0.03"}, "total": {"value": "1000000"}}
                                    }
                                ]
                            }
                        ]
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let err = client
            .yield_opportunities(yield_req(chain, asset, 10))
            .await
            .expect_err("no matching reserve must be unavailable");
        assert_eq!(err.code, Code::Unavailable);
    }

    // ----- A6 + A8: LendPositions type split + YieldPositions --------------

    /// Mount the two-query position fixture (market-addresses, then positions)
    /// onto a fresh `MockServer`. Routes by GraphQL operation name embedded in
    /// the POST body (mirrors the Go `strings.Contains(body, "...")` switch).
    async fn mount_positions(server: &MockServer) {
        Mock::given(method("POST"))
            .and(body_string_contains("MarketAddresses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "data": {
                        "markets": [
                            {"address": "0x1111111111111111111111111111111111111111"}
                        ]
                    }
                }"#,
                "application/json",
            ))
            .mount(server)
            .await;

        Mock::given(method("POST"))
            .and(body_string_contains("Positions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{
                        "data": {{
                            "userSupplies": [
                                {{
                                    "market": {{"address": "0x1111111111111111111111111111111111111111"}},
                                    "currency": {{"address": "{USDC_ETH}", "symbol": "USDC", "decimals": 6}},
                                    "balance": {{"amount": {{"raw": "1000000", "decimals": 6, "value": "1"}}, "usd": "1"}},
                                    "apy": {{"value": "0.03"}},
                                    "isCollateral": false,
                                    "canBeCollateral": true
                                }},
                                {{
                                    "market": {{"address": "0x1111111111111111111111111111111111111111"}},
                                    "currency": {{"address": "{USDC_ETH}", "symbol": "USDC", "decimals": 6}},
                                    "balance": {{"amount": {{"raw": "2000000", "decimals": 6, "value": "2"}}, "usd": "2"}},
                                    "apy": {{"value": "0.03"}},
                                    "isCollateral": true,
                                    "canBeCollateral": true
                                }}
                            ],
                            "userBorrows": [
                                {{
                                    "market": {{"address": "0x1111111111111111111111111111111111111111"}},
                                    "currency": {{"address": "{USDC_ETH}", "symbol": "USDC", "decimals": 6}},
                                    "debt": {{"amount": {{"raw": "500000", "decimals": 6, "value": "0.5"}}, "usd": "0.5"}},
                                    "apy": {{"value": "0.05"}}
                                }}
                            ]
                        }}
                    }}"#
                ),
                "application/json",
            ))
            .mount(server)
            .await;
    }

    fn positions_req(
        chain: defi_id::Chain,
        account: &str,
        position_type: LendPositionType,
    ) -> LendPositionsRequest {
        LendPositionsRequest {
            chain,
            account: account.to_string(),
            asset: defi_id::Asset::default(),
            position_type,
            limit: 0,
            rpc_url: String::new(),
        }
    }

    const DEAD_ACCOUNT: &str = "0x000000000000000000000000000000000000dEaD";

    #[tokio::test]
    async fn lend_positions_type_all_returns_supply_collateral_borrow() {
        let server = MockServer::start().await;
        mount_positions(&server).await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let all = client
            .lend_positions(positions_req(chain, DEAD_ACCOUNT, LendPositionType::All))
            .await
            .expect("lend_positions(all)");
        assert_eq!(all.len(), 3, "supply + collateral + borrow");

        let mut counts = std::collections::HashMap::new();
        for item in &all {
            *counts.entry(item.position_type.clone()).or_insert(0) += 1;
            assert_eq!(
                item.provider_native_id_kind,
                model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET
            );
        }
        assert_eq!(counts.get("supply"), Some(&1));
        assert_eq!(counts.get("collateral"), Some(&1));
        assert_eq!(counts.get("borrow"), Some(&1));
    }

    #[tokio::test]
    async fn lend_positions_filters_supply_only() {
        let server = MockServer::start().await;
        mount_positions(&server).await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let supply_only = client
            .lend_positions(positions_req(chain, DEAD_ACCOUNT, LendPositionType::Supply))
            .await
            .expect("lend_positions(supply)");
        assert_eq!(supply_only.len(), 1);
        assert_eq!(supply_only[0].position_type, "supply");
        // The non-collateral supply has raw balance 1000000 (1 USDC, 6 decimals).
        assert_eq!(supply_only[0].amount.amount_base_units, "1000000");
        assert_eq!(supply_only[0].amount.amount_decimal, "1");
        assert_eq!(supply_only[0].amount.decimals, 6);
    }

    #[tokio::test]
    async fn lend_positions_filters_collateral_only() {
        let server = MockServer::start().await;
        mount_positions(&server).await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let collateral_only = client
            .lend_positions(positions_req(
                chain,
                DEAD_ACCOUNT,
                LendPositionType::Collateral,
            ))
            .await
            .expect("lend_positions(collateral)");
        assert_eq!(collateral_only.len(), 1);
        assert_eq!(collateral_only[0].position_type, "collateral");
    }

    #[tokio::test]
    async fn yield_positions_keeps_supply_and_collateral_as_deposits() {
        let server = MockServer::start().await;
        mount_positions(&server).await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let rows = client
            .yield_positions(YieldPositionsRequest {
                chain,
                account: DEAD_ACCOUNT.to_string(),
                asset: defi_id::Asset::default(),
                limit: 0,
                rpc_url: String::new(),
            })
            .await
            .expect("yield_positions");
        // supply + collateral become deposits; the borrow is dropped.
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row.position_type, "deposit");
            assert_eq!(
                row.provider_native_id_kind,
                model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET
            );
        }
    }

    // ----- A7: LendPositions input guards ----------------------------------

    #[tokio::test]
    async fn lend_positions_rejects_non_evm_chain() {
        let chain = parse_chain("solana").expect("parse solana");
        let mut client = Client::new(http());
        client.set_endpoint("http://127.0.0.1:0");

        let err = client
            .lend_positions(positions_req(chain, DEAD_ACCOUNT, LendPositionType::All))
            .await
            .expect_err("non-EVM chain must be unsupported");
        assert_eq!(err.code, Code::Unsupported);
    }

    #[tokio::test]
    async fn lend_positions_rejects_invalid_account() {
        let chain = parse_chain("ethereum").expect("parse ethereum");
        let mut client = Client::new(http());
        client.set_endpoint("http://127.0.0.1:0");

        let err = client
            .lend_positions(positions_req(
                chain,
                "not-an-address",
                LendPositionType::All,
            ))
            .await
            .expect_err("invalid account must be a usage error");
        assert_eq!(err.code, Code::Usage);
    }

    // ----- A9: YieldHistory APY --------------------------------------------

    #[tokio::test]
    async fn yield_history_returns_scaled_points_with_last_day_window() {
        let fixed_now = Utc.with_ymd_and_hms(2026, 2, 26, 20, 0, 0).unwrap();
        let start = fixed_now - chrono::Duration::hours(6);
        let market = "0x1111111111111111111111111111111111111111";
        let underlying = USDC_ETH;

        // Sample timestamps inside [start, end].
        let t1 = (fixed_now - chrono::Duration::hours(5)).to_rfc3339();
        let t2 = (fixed_now - chrono::Duration::hours(3)).to_rfc3339();

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("SupplyAPYHistory"))
            // Sub-24h span must select the LAST_DAY window in the request body.
            .and(body_string_contains("LAST_DAY"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{
                        "data": {{
                            "supplyAPYHistory": [
                                {{"date": "{t1}", "avgRate": {{"value": "0.02"}}}},
                                {{"date": "{t2}", "avgRate": {{"value": "0.018"}}}}
                            ]
                        }}
                    }}"#
                ),
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());
        client.set_now(fixed_now);

        let opportunity = model::YieldOpportunity {
            opportunity_id: "opp-1".into(),
            provider: "aave".into(),
            protocol: "aave".into(),
            chain_id: "eip155:1".into(),
            asset_id: format!("eip155:1/erc20:{USDC_ETH}"),
            provider_native_id: format!("aave:eip155:1:{market}:{underlying}"),
            provider_native_id_kind: model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET.into(),
            opportunity_type: "lend".into(),
            apy_base: 0.0,
            apy_reward: 0.0,
            apy_total: 0.0,
            tvl_usd: 0.0,
            liquidity_usd: 0.0,
            lockup_days: 0.0,
            withdrawal_terms: String::new(),
            backing_assets: Vec::new(),
            source_url: "https://app.aave.com".into(),
            fetched_at: String::new(),
        };

        let series = client
            .yield_history(YieldHistoryRequest {
                opportunity,
                start_time: start,
                end_time: fixed_now,
                interval: YieldHistoryInterval::Hour,
                metrics: vec![YieldHistoryMetric::ApyTotal],
            })
            .await
            .expect("yield_history");
        assert_eq!(series.len(), 1);
        let s = &series[0];
        assert_eq!(s.metric, "apy_total");
        assert_eq!(s.opportunity_id, "opp-1");
        assert_eq!(s.chain_id, "eip155:1");
        assert_eq!(s.points.len(), 2);
        // 0.02 ratio -> 2.0 percentage points.
        assert_eq!(s.points[0].value, 2.0);
    }

    // ----- A10: YieldHistory rejects unsupported metric --------------------

    #[tokio::test]
    async fn yield_history_rejects_unsupported_metric_without_network() {
        let fixed_now = Utc.with_ymd_and_hms(2026, 2, 26, 20, 0, 0).unwrap();
        let mut client = Client::new(http());
        client.set_endpoint("http://127.0.0.1:0"); // unroutable: guard must fire first
        client.set_now(fixed_now);

        let opportunity = model::YieldOpportunity {
            opportunity_id: String::new(),
            provider: "aave".into(),
            protocol: "aave".into(),
            chain_id: "eip155:1".into(),
            asset_id: String::new(),
            provider_native_id: "aave:eip155:1:0x1111111111111111111111111111111111111111:"
                .to_string()
                + USDC_ETH,
            provider_native_id_kind: String::new(),
            opportunity_type: "lend".into(),
            apy_base: 0.0,
            apy_reward: 0.0,
            apy_total: 0.0,
            tvl_usd: 0.0,
            liquidity_usd: 0.0,
            lockup_days: 0.0,
            withdrawal_terms: String::new(),
            backing_assets: Vec::new(),
            source_url: String::new(),
            fetched_at: String::new(),
        };

        let err = client
            .yield_history(YieldHistoryRequest {
                opportunity,
                start_time: fixed_now - chrono::Duration::hours(1),
                end_time: fixed_now,
                interval: YieldHistoryInterval::Hour,
                metrics: vec![YieldHistoryMetric::TvlUsd],
            })
            .await
            .expect_err("tvl_usd metric must be rejected");
        // Go maps this to CodeUnsupported.
        assert_eq!(err.code, Code::Unsupported);
    }
}
