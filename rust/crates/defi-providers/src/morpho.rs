//! Morpho provider adapter — lending markets/rates/positions + yield
//! opportunities/positions/history backed by the Morpho GraphQL API.
//!
//! Go source: `internal/providers/morpho/client.go` (+ `client_test.go`).
//!
//! Implements the `LendingProvider` (markets/rates), `LendingPositionsProvider`,
//! `YieldProvider`, `YieldPositionsProvider`, and `YieldHistoryProvider` trait
//! surfaces, plus `Provider` metadata. Talks to the Morpho GraphQL endpoint
//! (`registry::MORPHO_GRAPHQL_ENDPOINT`). All outputs are deterministic (stable
//! multi-key sorts); every APY field is a PERCENTAGE POINT, not a ratio (spec
//! §2.5) — the GraphQL ratio values (`0.05`) are scaled ×100 to `5.0`.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use defi_errors::{Code, Error};
use defi_httpx::{do_body_json, Client as HttpClient};
use defi_id::{format_decimal, parse_chain, Asset, Chain};
use defi_model as model;
use num_bigint::BigInt;
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

/// Default Morpho GraphQL endpoint (mirrors `registry.MorphoGraphQLEndpoint`).
const DEFAULT_ENDPOINT: &str = defi_registry::MORPHO_GRAPHQL_ENDPOINT;
const SOURCE_URL: &str = "https://app.morpho.org";

const MARKETS_QUERY: &str = r#"query Markets($first:Int,$where:MarketFilters,$orderBy:MarketOrderBy,$orderDirection:OrderDirection){
  markets(first:$first, where:$where, orderBy:$orderBy, orderDirection:$orderDirection){
    items{
      id
      uniqueKey
      irmAddress
      loanAsset{ address symbol decimals chain{ id network } }
      collateralAsset{ address symbol }
      state{ supplyApy borrowApy utilization supplyAssetsUsd liquidityAssetsUsd totalLiquidityUsd }
    }
  }
}"#;

const POSITIONS_QUERY: &str = r#"query Positions($first:Int,$where:MarketPositionFilters,$orderBy:MarketPositionOrderBy,$orderDirection:OrderDirection){
  marketPositions(first:$first, where:$where, orderBy:$orderBy, orderDirection:$orderDirection){
    items{
      id
      market{
        uniqueKey
        loanAsset{ address symbol decimals chain{ id network } }
        collateralAsset{ address symbol decimals }
        state{ supplyApy borrowApy }
      }
      state{
        supplyAssets
        supplyAssetsUsd
        borrowAssets
        borrowAssetsUsd
        collateral
        collateralUsd
      }
    }
  }
}"#;

const VAULT_POSITIONS_QUERY: &str = r#"query VaultPositions($first:Int,$where:VaultPositionFilters,$orderBy:VaultPositionOrderBy,$orderDirection:OrderDirection){
  vaultPositions(first:$first, where:$where, orderBy:$orderBy, orderDirection:$orderDirection){
    items{
      id
      user{ address }
      vault{
        address
        asset{ address symbol decimals chain{ id network } }
        state{ netApy }
      }
      state{
        shares
        assets
        assetsUsd
      }
    }
  }
}"#;

const VAULTS_YIELD_QUERY: &str = r#"query Vaults($first:Int,$skip:Int,$where:VaultFilters,$orderBy:VaultOrderBy,$orderDirection:OrderDirection){
  vaults(first:$first, skip:$skip, where:$where, orderBy:$orderBy, orderDirection:$orderDirection){
    items{
      address
      name
      symbol
      asset{ address symbol }
      state{
        netApy
        totalAssetsUsd
        allocation{
          supplyAssetsUsd
          market{
            loanAsset{ address symbol }
            collateralAsset{ address symbol }
          }
        }
      }
      liquidity{ usd }
    }
  }
}"#;

const VAULT_V2S_YIELD_QUERY: &str = r#"query VaultV2s($first:Int,$skip:Int,$where:VaultV2sFilters,$orderBy:VaultV2OrderBy,$orderDirection:OrderDirection){
  vaultV2s(first:$first, skip:$skip, where:$where, orderBy:$orderBy, orderDirection:$orderDirection){
    items{
      address
      name
      symbol
      asset{ address symbol }
      netApy
      totalAssetsUsd
      liquidityUsd
      liquidityData{
        __typename
        ... on MarketV1LiquidityData {
          market{
            collateralAsset{ address symbol }
          }
        }
        ... on MetaMorphoLiquidityData {
          metaMorpho{
            state{
              allocation{
                supplyAssetsUsd
                market{
                  loanAsset{ address symbol }
                  collateralAsset{ address symbol }
                }
              }
            }
          }
        }
      }
    }
  }
}"#;

const VAULT_HISTORY_QUERY: &str = r#"query VaultHistory($address:String!,$chainId:Int!,$start:Int!,$end:Int!,$interval:TimeseriesInterval!){
  vaultByAddress(address:$address, chainId:$chainId){
    address
    historicalState{
      netApy(options:{startTimestamp:$start, endTimestamp:$end, interval:$interval}){ x y }
      totalAssetsUsd(options:{startTimestamp:$start, endTimestamp:$end, interval:$interval}){ x y }
    }
  }
}"#;

const VAULT_V2_HISTORY_QUERY: &str = r#"query VaultV2History($address:String!,$chainId:Int!,$start:Int!,$end:Int!,$interval:TimeseriesInterval!){
  vaultV2ByAddress(address:$address, chainId:$chainId){
    address
    historicalState{
      avgNetApy(options:{startTimestamp:$start, endTimestamp:$end, interval:$interval}){ x y }
      totalAssetsUsd(options:{startTimestamp:$start, endTimestamp:$end, interval:$interval}){ x y }
    }
  }
}"#;

const YIELD_VAULT_PAGE_SIZE: i64 = 200;
const YIELD_VAULT_MAX_PAGES: i64 = 20;

/// Morpho lending + yield adapter (mirrors Go `morpho.Client`).
pub struct Client {
    http: HttpClient,
    endpoint: String,
    /// Injected fixed clock for deterministic `fetched_at`; `None` uses the wall
    /// clock.
    now: Option<DateTime<Utc>>,
}

impl Client {
    /// Build a client targeting the default Morpho GraphQL endpoint (mirrors Go
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

    async fn fetch_markets(
        &self,
        chain: &Chain,
        asset: &Asset,
    ) -> Result<Vec<MorphoMarket>, Error> {
        if !chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "morpho supports only EVM chains",
            ));
        }
        let mut where_clause = json!({
            "chainId_in": [chain.evm_chain_id],
            "listed": true,
        });
        let addr = asset.address.trim();
        if !addr.is_empty() {
            where_clause["loanAssetAddress_in"] = json!([addr.to_ascii_lowercase()]);
        }
        let body = json!({
            "query": MARKETS_QUERY,
            "variables": {
                "first": 100,
                "orderBy": "SupplyAssetsUsd",
                "orderDirection": "Desc",
                "where": where_clause,
            },
        });

        let resp: MarketsResponse = self.post(body, "marshal morpho query").await?;
        if let Some(msg) = first_error(&resp.errors) {
            return Err(Error::new(
                Code::Unavailable,
                format!("morpho graphql error: {msg}"),
            ));
        }
        if resp.data.markets.items.is_empty() {
            return Err(Error::new(
                Code::Unsupported,
                "morpho has no market for requested chain/asset",
            ));
        }
        Ok(resp.data.markets.items)
    }

    async fn fetch_vaults(&self, chain: &Chain, asset: &Asset) -> Result<Vec<MorphoVault>, Error> {
        let mut where_clause = json!({
            "chainId_in": [chain.evm_chain_id],
            "listed": true,
        });
        let addr = normalize_evm_address(&asset.address);
        if !addr.is_empty() {
            where_clause["assetAddress_in"] = json!([addr]);
        } else {
            let symbol = asset.symbol.trim();
            if !symbol.is_empty() {
                where_clause["assetSymbol_in"] = json!([symbol]);
            }
        }

        let mut out: Vec<MorphoVault> = Vec::with_capacity(YIELD_VAULT_PAGE_SIZE as usize);
        for page in 0..YIELD_VAULT_MAX_PAGES {
            let body = json!({
                "query": VAULTS_YIELD_QUERY,
                "variables": {
                    "first": YIELD_VAULT_PAGE_SIZE,
                    "skip": page * YIELD_VAULT_PAGE_SIZE,
                    "where": where_clause,
                },
            });
            let resp: VaultsResponse = self.post(body, "marshal morpho vault query").await?;
            if let Some(msg) = first_error(&resp.errors) {
                return Err(Error::new(
                    Code::Unavailable,
                    format!("morpho graphql error: {msg}"),
                ));
            }
            let count = resp.data.vaults.items.len() as i64;
            out.extend(resp.data.vaults.items);
            if count < YIELD_VAULT_PAGE_SIZE {
                break;
            }
        }
        Ok(out)
    }

    async fn fetch_vault_v2s(&self, chain: &Chain) -> Result<Vec<MorphoVaultV2>, Error> {
        let where_clause = json!({
            "chainId_in": [chain.evm_chain_id],
            "listed": true,
        });

        let mut out: Vec<MorphoVaultV2> = Vec::with_capacity(YIELD_VAULT_PAGE_SIZE as usize);
        for page in 0..YIELD_VAULT_MAX_PAGES {
            let body = json!({
                "query": VAULT_V2S_YIELD_QUERY,
                "variables": {
                    "first": YIELD_VAULT_PAGE_SIZE,
                    "skip": page * YIELD_VAULT_PAGE_SIZE,
                    "where": where_clause,
                },
            });
            let resp: VaultV2sResponse = self.post(body, "marshal morpho vault-v2 query").await?;
            if let Some(msg) = first_error(&resp.errors) {
                return Err(Error::new(
                    Code::Unavailable,
                    format!("morpho graphql error: {msg}"),
                ));
            }
            let count = resp.data.vault_v2s.items.len() as i64;
            out.extend(resp.data.vault_v2s.items);
            if count < YIELD_VAULT_PAGE_SIZE {
                break;
            }
        }
        Ok(out)
    }

    async fn fetch_yield_vault_candidates(
        &self,
        chain: &Chain,
        asset: &Asset,
    ) -> Result<Vec<VaultYieldCandidate>, Error> {
        if !chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "morpho supports only EVM chains",
            ));
        }

        let vaults = self.fetch_vaults(chain, asset).await?;
        let vault_v2s = self.fetch_vault_v2s(chain).await?;

        let mut out: Vec<VaultYieldCandidate> = Vec::with_capacity(vaults.len() + vault_v2s.len());
        for vault in &vaults {
            let (asset_address, asset_symbol) = match &vault.asset {
                Some(a) => (a.address.clone(), a.symbol.clone()),
                None => (String::new(), String::new()),
            };
            if !matches_vault_asset(&asset_address, &asset_symbol, asset) {
                continue;
            }
            let (net_apy, tvl) = match &vault.state {
                Some(s) => (s.net_apy * 100.0, s.total_assets_usd),
                None => (0.0, 0.0),
            };
            let liquidity = vault.liquidity.as_ref().map(|l| l.usd).unwrap_or(0.0);
            let allocation = vault
                .state
                .as_ref()
                .map(|s| s.allocation.as_slice())
                .unwrap_or(&[]);
            out.push(VaultYieldCandidate {
                address: vault.address.clone(),
                asset_address: asset_address.clone(),
                asset_symbol: asset_symbol.clone(),
                net_apy_percent: net_apy,
                total_assets_usd: tvl,
                liquidity_usd: liquidity,
                backing_shares: collateral_shares_from_allocation(
                    0.0,
                    allocation,
                    &asset_address,
                    &asset_symbol,
                ),
            });
        }
        for vault in &vault_v2s {
            let (asset_address, asset_symbol) = match &vault.asset {
                Some(a) => (a.address.clone(), a.symbol.clone()),
                None => (String::new(), String::new()),
            };
            if !matches_vault_asset(&asset_address, &asset_symbol, asset) {
                continue;
            }
            out.push(VaultYieldCandidate {
                address: vault.address.clone(),
                asset_address: asset_address.clone(),
                asset_symbol: asset_symbol.clone(),
                net_apy_percent: vault.net_apy * 100.0,
                total_assets_usd: vault.total_assets_usd,
                liquidity_usd: vault.liquidity_usd,
                backing_shares: collateral_shares_from_vault_v2(
                    vault,
                    &asset_address,
                    &asset_symbol,
                ),
            });
        }
        if out.is_empty() {
            return Err(Error::new(
                Code::Unsupported,
                "morpho has no yield vault for requested chain/asset",
            ));
        }
        Ok(out)
    }

    /// Fetch raw vault history (v1 first, falling back to v2 on "no results").
    /// Returns `(apy_points, tvl_points, source_url)`.
    async fn fetch_vault_history(
        &self,
        address: &str,
        chain_id: i64,
        start: i64,
        end: i64,
        interval: &str,
    ) -> Result<(Vec<MorphoFloatDataPoint>, Vec<MorphoFloatDataPoint>, String), Error> {
        let body = json!({
            "query": VAULT_HISTORY_QUERY,
            "variables": {
                "address": address,
                "chainId": chain_id,
                "start": start,
                "end": end,
                "interval": interval,
            },
        });
        let resp: VaultHistoryResponse = self
            .post(body, "marshal morpho vault history query")
            .await?;
        if let Some(msg) = first_error(&resp.errors) {
            if !is_morpho_no_results_error(msg) {
                return Err(Error::new(
                    Code::Unavailable,
                    format!("morpho graphql error: {msg}"),
                ));
            }
        }
        if let Some(vault) = &resp.data.vault_by_address {
            if let Some(state) = &vault.historical_state {
                return Ok((
                    state.net_apy.clone(),
                    state.tvl_usd.clone(),
                    source_url_for_vault(address),
                ));
            }
        }

        let body = json!({
            "query": VAULT_V2_HISTORY_QUERY,
            "variables": {
                "address": address,
                "chainId": chain_id,
                "start": start,
                "end": end,
                "interval": interval,
            },
        });
        let resp_v2: VaultV2HistoryResponse = self
            .post(body, "marshal morpho vault-v2 history query")
            .await?;
        if let Some(msg) = first_error(&resp_v2.errors) {
            return Err(Error::new(
                Code::Unavailable,
                format!("morpho graphql error: {msg}"),
            ));
        }
        let vault = match &resp_v2.data.vault_v2_by_address {
            Some(v) => v,
            None => {
                return Err(Error::new(
                    Code::Unavailable,
                    "morpho returned no vault history for requested opportunity",
                ));
            }
        };
        let state = match &vault.historical_state {
            Some(s) => s,
            None => {
                return Err(Error::new(
                    Code::Unavailable,
                    "morpho returned no vault history for requested opportunity",
                ));
            }
        };
        Ok((
            state.avg_net_apy.clone(),
            state.tvl_usd.clone(),
            source_url_for_vault(address),
        ))
    }
}

impl Provider for Client {
    fn info(&self) -> model::ProviderInfo {
        model::ProviderInfo {
            name: "morpho".to_string(),
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
        if !provider.eq_ignore_ascii_case("morpho") {
            return Err(Error::new(
                Code::Unsupported,
                "morpho adapter supports only provider=morpho",
            ));
        }
        let markets = self.fetch_markets(&chain, &asset).await?;

        let mut out: Vec<model::LendMarket> = Vec::with_capacity(markets.len());
        for m in &markets {
            let tvl = yieldutil::positive_first(&[
                m.state.supply_assets_usd,
                m.state.total_liquidity_usd,
                m.state.liquidity_assets_usd,
            ]);
            if tvl <= 0.0 {
                continue;
            }
            let supply_apy = m.state.supply_apy * 100.0;
            let borrow_apy = m.state.borrow_apy * 100.0;
            out.push(model::LendMarket {
                protocol: "morpho".to_string(),
                provider: "morpho".to_string(),
                chain_id: chain.caip2.clone(),
                asset_id: canonical_asset_id(&asset, &m.loan_asset.address),
                provider_native_id: m.unique_key.trim().to_string(),
                provider_native_id_kind: model::NATIVE_ID_KIND_MARKET_ID.to_string(),
                supply_apy,
                borrow_apy,
                tvl_usd: tvl,
                liquidity_usd: yieldutil::positive_first(&[
                    m.state.liquidity_assets_usd,
                    m.state.total_liquidity_usd,
                    tvl,
                ]),
                source_url: SOURCE_URL.to_string(),
                fetched_at: self.fetched_at(),
            });
        }

        out.sort_by(|a, b| {
            desc_f64(a.tvl_usd, b.tvl_usd).then_with(|| a.asset_id.cmp(&b.asset_id))
        });
        if out.is_empty() {
            return Err(Error::new(
                Code::Unsupported,
                "no morpho lending market for requested chain/asset",
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
        if !provider.eq_ignore_ascii_case("morpho") {
            return Err(Error::new(
                Code::Unsupported,
                "morpho adapter supports only provider=morpho",
            ));
        }
        let markets = self.fetch_markets(&chain, &asset).await?;

        let mut out: Vec<model::LendRate> = Vec::with_capacity(markets.len());
        for m in &markets {
            out.push(model::LendRate {
                protocol: "morpho".to_string(),
                provider: "morpho".to_string(),
                chain_id: chain.caip2.clone(),
                asset_id: canonical_asset_id(&asset, &m.loan_asset.address),
                provider_native_id: m.unique_key.trim().to_string(),
                provider_native_id_kind: model::NATIVE_ID_KIND_MARKET_ID.to_string(),
                supply_apy: m.state.supply_apy * 100.0,
                borrow_apy: m.state.borrow_apy * 100.0,
                utilization: m.state.utilization,
                source_url: SOURCE_URL.to_string(),
                fetched_at: self.fetched_at(),
            });
        }

        out.sort_by(|a, b| {
            desc_f64(a.supply_apy, b.supply_apy).then_with(|| a.asset_id.cmp(&b.asset_id))
        });
        if out.is_empty() {
            return Err(Error::new(
                Code::Unsupported,
                "no morpho lending rates for requested chain/asset",
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
                "morpho supports only EVM chains",
            ));
        }
        let account = normalize_evm_address(&req.account);
        if account.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "morpho positions requires a valid EVM account address",
            ));
        }
        let filter_type = req.position_type;

        let first = clamp_first(req.limit);
        let body = json!({
            "query": POSITIONS_QUERY,
            "variables": {
                "first": first,
                "orderBy": "SupplyShares",
                "orderDirection": "Desc",
                "where": {
                    "userAddress_in": [account],
                    "chainId_in": [req.chain.evm_chain_id],
                    "marketListed": true,
                },
            },
        });

        let resp: PositionsResponse = self.post(body, "marshal morpho positions query").await?;
        if let Some(msg) = first_error(&resp.errors) {
            return Err(Error::new(
                Code::Unavailable,
                format!("morpho graphql error: {msg}"),
            ));
        }

        let chain_caip2 = req.chain.caip2.clone();
        let mut out: Vec<model::LendPosition> = Vec::new();
        for item in &resp.data.market_positions.items {
            let state = match &item.state {
                Some(s) => s,
                None => continue,
            };

            let loan_asset_id =
                canonical_asset_id_for_chain(&chain_caip2, &item.market.loan_asset.address);
            if !loan_asset_id.is_empty() {
                if matches_position_type(filter_type, LendPositionType::Supply)
                    && matches_position_asset(
                        &item.market.loan_asset.address,
                        &item.market.loan_asset.symbol,
                        &req.asset,
                    )
                {
                    let base = normalized_bigint(&state.supply_assets);
                    if base != "0" {
                        let supply_apy = item
                            .market
                            .state
                            .as_ref()
                            .map(|s| s.supply_apy * 100.0)
                            .unwrap_or(0.0);
                        out.push(model::LendPosition {
                            protocol: "morpho".to_string(),
                            provider: "morpho".to_string(),
                            chain_id: chain_caip2.clone(),
                            account_address: account.clone(),
                            position_type: LendPositionType::Supply.as_str().to_string(),
                            asset_id: loan_asset_id.clone(),
                            provider_native_id: item.market.unique_key.trim().to_string(),
                            provider_native_id_kind: model::NATIVE_ID_KIND_MARKET_ID.to_string(),
                            amount: amount_info_from_base(&base, item.market.loan_asset.decimals),
                            amount_usd: state.supply_assets_usd,
                            apy: supply_apy,
                            source_url: SOURCE_URL.to_string(),
                            fetched_at: self.fetched_at(),
                        });
                    }
                }

                if matches_position_type(filter_type, LendPositionType::Borrow)
                    && matches_position_asset(
                        &item.market.loan_asset.address,
                        &item.market.loan_asset.symbol,
                        &req.asset,
                    )
                {
                    let base = normalized_bigint(&state.borrow_assets);
                    if base != "0" {
                        let borrow_apy = item
                            .market
                            .state
                            .as_ref()
                            .map(|s| s.borrow_apy * 100.0)
                            .unwrap_or(0.0);
                        out.push(model::LendPosition {
                            protocol: "morpho".to_string(),
                            provider: "morpho".to_string(),
                            chain_id: chain_caip2.clone(),
                            account_address: account.clone(),
                            position_type: LendPositionType::Borrow.as_str().to_string(),
                            asset_id: loan_asset_id.clone(),
                            provider_native_id: item.market.unique_key.trim().to_string(),
                            provider_native_id_kind: model::NATIVE_ID_KIND_MARKET_ID.to_string(),
                            amount: amount_info_from_base(&base, item.market.loan_asset.decimals),
                            amount_usd: state.borrow_assets_usd,
                            apy: borrow_apy,
                            source_url: SOURCE_URL.to_string(),
                            fetched_at: self.fetched_at(),
                        });
                    }
                }
            }

            if let Some(collateral_asset) = &item.market.collateral_asset {
                if matches_position_type(filter_type, LendPositionType::Collateral)
                    && matches_position_asset(
                        &collateral_asset.address,
                        &collateral_asset.symbol,
                        &req.asset,
                    )
                {
                    let base = normalized_bigint(&state.collateral);
                    let collateral_asset_id =
                        canonical_asset_id_for_chain(&chain_caip2, &collateral_asset.address);
                    if base != "0" && !collateral_asset_id.is_empty() {
                        out.push(model::LendPosition {
                            protocol: "morpho".to_string(),
                            provider: "morpho".to_string(),
                            chain_id: chain_caip2.clone(),
                            account_address: account.clone(),
                            position_type: LendPositionType::Collateral.as_str().to_string(),
                            asset_id: collateral_asset_id,
                            provider_native_id: item.market.unique_key.trim().to_string(),
                            provider_native_id_kind: model::NATIVE_ID_KIND_MARKET_ID.to_string(),
                            amount: amount_info_from_base(&base, collateral_asset.decimals),
                            amount_usd: state.collateral_usd,
                            apy: 0.0,
                            source_url: SOURCE_URL.to_string(),
                            fetched_at: self.fetched_at(),
                        });
                    }
                }
            }
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
        let vaults = self
            .fetch_yield_vault_candidates(&req.chain, &req.asset)
            .await?;

        let mut out: Vec<model::YieldOpportunity> = Vec::with_capacity(vaults.len());
        for vault in &vaults {
            let apy = vault.net_apy_percent;
            let tvl = vault.total_assets_usd;
            if (apy == 0.0 || tvl == 0.0) && !req.include_incomplete {
                continue;
            }
            if apy < req.min_apy || tvl < req.min_tvl_usd {
                continue;
            }
            let backing_assets = backing_assets_from_shares(
                &vault.backing_shares,
                &req.chain.caip2,
                &vault.asset_address,
                &vault.asset_symbol,
                &req.asset.asset_id,
            );
            let liq = vault.liquidity_usd;
            let asset_id = canonical_asset_id(&req.asset, &vault.asset_address);
            let vault_address = normalize_evm_address(&vault.address);
            if vault_address.is_empty() {
                continue;
            }
            out.push(model::YieldOpportunity {
                opportunity_id: hash_opportunity(
                    "morpho",
                    &req.chain.caip2,
                    &vault_address,
                    &asset_id,
                ),
                provider: "morpho".to_string(),
                protocol: "morpho".to_string(),
                chain_id: req.chain.caip2.clone(),
                asset_id,
                provider_native_id: vault_address.clone(),
                provider_native_id_kind: model::NATIVE_ID_KIND_VAULT_ADDRESS.to_string(),
                opportunity_type: "lend".to_string(),
                apy_base: apy,
                apy_reward: 0.0,
                apy_total: apy,
                tvl_usd: tvl,
                liquidity_usd: liq,
                lockup_days: 0.0,
                withdrawal_terms: "variable".to_string(),
                backing_assets,
                source_url: source_url_for_vault(&vault_address),
                fetched_at: self.fetched_at(),
            });
        }

        if out.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "no morpho yield opportunities for requested chain/asset",
            ));
        }
        yieldutil::sort_opportunities(&mut out, &req.sort_by);
        let limit = if req.limit <= 0 || req.limit > out.len() as i64 {
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
        if !req.chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "morpho supports only EVM chains",
            ));
        }
        let account = normalize_evm_address(&req.account);
        if account.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "morpho positions requires a valid EVM account address",
            ));
        }

        let first = clamp_first(req.limit);
        let body = json!({
            "query": VAULT_POSITIONS_QUERY,
            "variables": {
                "first": first,
                "orderBy": "Shares",
                "orderDirection": "Desc",
                "where": {
                    "userAddress_in": [account],
                    "chainId_in": [req.chain.evm_chain_id],
                    "vaultListed": true,
                    "shares_gte": "1",
                },
            },
        });

        let resp: VaultPositionsResponse = self
            .post(body, "marshal morpho vault positions query")
            .await?;
        if let Some(msg) = first_error(&resp.errors) {
            return Err(Error::new(
                Code::Unavailable,
                format!("morpho graphql error: {msg}"),
            ));
        }

        let chain_caip2 = req.chain.caip2.clone();
        let mut out: Vec<model::YieldPosition> =
            Vec::with_capacity(resp.data.vault_positions.items.len());
        for item in &resp.data.vault_positions.items {
            let state = match &item.state {
                Some(s) => s,
                None => continue,
            };
            let vault_asset = match &item.vault.asset {
                Some(a) => a,
                None => continue,
            };
            if !matches_position_asset(&vault_asset.address, &vault_asset.symbol, &req.asset) {
                continue;
            }

            let shares_base = normalized_bigint(&state.shares);
            if shares_base == "0" {
                continue;
            }
            let assets_base = normalized_bigint(&state.assets);
            if assets_base == "0" {
                continue;
            }
            let vault_address = normalize_evm_address(&item.vault.address);
            if vault_address.is_empty() {
                continue;
            }
            let asset_id = canonical_asset_id_for_chain(&chain_caip2, &vault_asset.address);
            if asset_id.is_empty() {
                continue;
            }
            let apy_total = item
                .vault
                .state
                .as_ref()
                .map(|s| s.net_apy * 100.0)
                .unwrap_or(0.0);
            out.push(model::YieldPosition {
                protocol: "morpho".to_string(),
                provider: "morpho".to_string(),
                chain_id: chain_caip2.clone(),
                account_address: account.clone(),
                position_type: "deposit".to_string(),
                opportunity_id: hash_opportunity("morpho", &chain_caip2, &vault_address, &asset_id),
                asset_id,
                provider_native_id: vault_address.clone(),
                provider_native_id_kind: model::NATIVE_ID_KIND_VAULT_ADDRESS.to_string(),
                amount: amount_info_from_base(&assets_base, vault_asset.decimals),
                shares: Some(amount_info_from_base(&shares_base, 18)),
                amount_usd: state.assets_usd,
                apy_total,
                source_url: source_url_for_vault(&vault_address),
                fetched_at: self.fetched_at(),
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
        if !req
            .opportunity
            .provider
            .trim()
            .eq_ignore_ascii_case("morpho")
        {
            return Err(Error::new(
                Code::Unsupported,
                "morpho history supports only morpho opportunities",
            ));
        }
        if req.start_time >= req.end_time {
            return Err(Error::new(
                Code::Usage,
                "history start time must be before end time",
            ));
        }

        let chain = parse_chain(&req.opportunity.chain_id)
            .map_err(|e| Error::wrap(Code::Usage, "parse morpho opportunity chain", e))?;
        if !chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "morpho supports only EVM chains",
            ));
        }
        let vault_address = normalize_evm_address(&req.opportunity.provider_native_id);
        if vault_address.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "morpho opportunity requires a vault address provider_native_id",
            ));
        }

        let interval = morpho_timeseries_interval(req.interval)?;
        let start = req.start_time.timestamp();
        let end = req.end_time.timestamp();

        // Distinct requested metrics (dedup, matching the Go map-set), validated
        // against the supported set.
        let mut want_apy = false;
        let mut want_tvl = false;
        for metric in &req.metrics {
            match metric {
                YieldHistoryMetric::ApyTotal => want_apy = true,
                YieldHistoryMetric::TvlUsd => want_tvl = true,
            }
        }

        let (apys, tvl, source_url) = self
            .fetch_vault_history(&vault_address, chain.evm_chain_id, start, end, interval)
            .await?;

        let mut series: Vec<model::YieldHistorySeries> = Vec::new();
        if want_apy {
            let points = convert_morpho_points(&apys, true);
            if !points.is_empty() {
                series.push(self.history_series(
                    &req,
                    YieldHistoryMetric::ApyTotal.as_str(),
                    points,
                    &source_url,
                ));
            }
        }
        if want_tvl {
            let points = convert_morpho_points(&tvl, false);
            if !points.is_empty() {
                series.push(self.history_series(
                    &req,
                    YieldHistoryMetric::TvlUsd.as_str(),
                    points,
                    &source_url,
                ));
            }
        }
        if series.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "no morpho historical points for requested range",
            ));
        }
        Ok(series)
    }
}

impl Client {
    fn history_series(
        &self,
        req: &YieldHistoryRequest,
        metric: &str,
        points: Vec<model::YieldHistoryPoint>,
        source_url: &str,
    ) -> model::YieldHistorySeries {
        model::YieldHistorySeries {
            opportunity_id: req.opportunity.opportunity_id.clone(),
            provider: "morpho".to_string(),
            protocol: req.opportunity.protocol.clone(),
            chain_id: req.opportunity.chain_id.clone(),
            asset_id: req.opportunity.asset_id.clone(),
            provider_native_id: req.opportunity.provider_native_id.clone(),
            provider_native_id_kind: req.opportunity.provider_native_id_kind.clone(),
            metric: metric.to_string(),
            interval: req.interval.as_str().to_string(),
            start_time: req.start_time.to_rfc3339_opts(SecondsFormat::Secs, true),
            end_time: req.end_time.to_rfc3339_opts(SecondsFormat::Secs, true),
            points,
            source_url: source_url.to_string(),
            fetched_at: self.fetched_at(),
        }
    }
}

// --- intermediate candidate types (mirror the Go private structs) ---

struct VaultYieldCandidate {
    address: String,
    asset_address: String,
    asset_symbol: String,
    net_apy_percent: f64,
    total_assets_usd: f64,
    liquidity_usd: f64,
    backing_shares: Vec<CollateralShare>,
}

struct CollateralShare {
    address: String,
    symbol: String,
    usd: f64,
}

// --- GraphQL response shapes (deserialize-only) ---

#[derive(Debug, Deserialize)]
struct GraphqlError {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Default, Clone, Deserialize)]
struct MorphoFloatDataPoint {
    #[serde(default, deserialize_with = "crate::serde_util::de_f64_null_default")]
    x: f64,
    y: Option<f64>,
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
    markets: ItemList<MorphoMarket>,
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
    #[serde(rename = "marketPositions", default)]
    market_positions: ItemList<MorphoMarketPosition>,
}

#[derive(Debug, Deserialize)]
struct VaultPositionsResponse {
    #[serde(default)]
    data: VaultPositionsData,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Default, Deserialize)]
struct VaultPositionsData {
    #[serde(rename = "vaultPositions", default)]
    vault_positions: ItemList<MorphoVaultPosition>,
}

#[derive(Debug, Deserialize)]
struct VaultsResponse {
    #[serde(default)]
    data: VaultsData,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Default, Deserialize)]
struct VaultsData {
    #[serde(default)]
    vaults: ItemList<MorphoVault>,
}

#[derive(Debug, Deserialize)]
struct VaultV2sResponse {
    #[serde(default)]
    data: VaultV2sData,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Default, Deserialize)]
struct VaultV2sData {
    #[serde(rename = "vaultV2s", default)]
    vault_v2s: ItemList<MorphoVaultV2>,
}

#[derive(Debug, Deserialize)]
struct ItemList<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
}

// Manual `Default` so the wrapping `*Data` structs can derive `Default` without
// requiring the inner item type `T` to be `Default` (the items are pointer-like
// nullable structs in the Go source).
impl<T> Default for ItemList<T> {
    fn default() -> Self {
        ItemList { items: Vec::new() }
    }
}

#[derive(Debug, Deserialize)]
struct VaultHistoryResponse {
    #[serde(default)]
    data: VaultHistoryData,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Default, Deserialize)]
struct VaultHistoryData {
    #[serde(rename = "vaultByAddress")]
    vault_by_address: Option<VaultByAddress>,
}

#[derive(Debug, Deserialize)]
struct VaultByAddress {
    #[serde(rename = "historicalState")]
    historical_state: Option<VaultHistoricalState>,
}

#[derive(Debug, Deserialize)]
struct VaultHistoricalState {
    #[serde(rename = "netApy", default)]
    net_apy: Vec<MorphoFloatDataPoint>,
    #[serde(rename = "totalAssetsUsd", default)]
    tvl_usd: Vec<MorphoFloatDataPoint>,
}

#[derive(Debug, Deserialize)]
struct VaultV2HistoryResponse {
    #[serde(default)]
    data: VaultV2HistoryData,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Default, Deserialize)]
struct VaultV2HistoryData {
    #[serde(rename = "vaultV2ByAddress")]
    vault_v2_by_address: Option<VaultV2ByAddress>,
}

#[derive(Debug, Deserialize)]
struct VaultV2ByAddress {
    #[serde(rename = "historicalState")]
    historical_state: Option<VaultV2HistoricalState>,
}

#[derive(Debug, Deserialize)]
struct VaultV2HistoricalState {
    #[serde(rename = "avgNetApy", default)]
    avg_net_apy: Vec<MorphoFloatDataPoint>,
    #[serde(rename = "totalAssetsUsd", default)]
    tvl_usd: Vec<MorphoFloatDataPoint>,
}

#[derive(Debug, Deserialize)]
struct MorphoMarket {
    #[serde(rename = "uniqueKey", default)]
    unique_key: String,
    #[serde(rename = "loanAsset", default)]
    loan_asset: LoanAsset,
    state: MarketState,
}

#[derive(Debug, Default, Deserialize)]
struct LoanAsset {
    #[serde(default)]
    address: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    decimals: i64,
}

#[derive(Debug, Default, Deserialize)]
struct MarketState {
    #[serde(
        rename = "supplyApy",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    supply_apy: f64,
    #[serde(
        rename = "borrowApy",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    borrow_apy: f64,
    #[serde(default, deserialize_with = "crate::serde_util::de_f64_null_default")]
    utilization: f64,
    #[serde(
        rename = "supplyAssetsUsd",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    supply_assets_usd: f64,
    #[serde(
        rename = "liquidityAssetsUsd",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    liquidity_assets_usd: f64,
    #[serde(
        rename = "totalLiquidityUsd",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    total_liquidity_usd: f64,
}

#[derive(Debug, Deserialize)]
struct MorphoMarketPosition {
    market: PositionMarket,
    state: Option<MarketPositionState>,
}

#[derive(Debug, Deserialize)]
struct PositionMarket {
    #[serde(rename = "uniqueKey", default)]
    unique_key: String,
    #[serde(rename = "loanAsset", default)]
    loan_asset: PositionAsset,
    #[serde(rename = "collateralAsset")]
    collateral_asset: Option<PositionAsset>,
    state: Option<PositionMarketRates>,
}

#[derive(Debug, Default, Deserialize)]
struct PositionAsset {
    #[serde(default)]
    address: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    decimals: i64,
}

#[derive(Debug, Deserialize)]
struct PositionMarketRates {
    #[serde(
        rename = "supplyApy",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    supply_apy: f64,
    #[serde(
        rename = "borrowApy",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    borrow_apy: f64,
}

#[derive(Debug, Deserialize)]
struct MarketPositionState {
    #[serde(rename = "supplyAssets", default)]
    supply_assets: serde_json::Value,
    #[serde(
        rename = "supplyAssetsUsd",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    supply_assets_usd: f64,
    #[serde(rename = "borrowAssets", default)]
    borrow_assets: serde_json::Value,
    #[serde(
        rename = "borrowAssetsUsd",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    borrow_assets_usd: f64,
    #[serde(default)]
    collateral: serde_json::Value,
    #[serde(
        rename = "collateralUsd",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    collateral_usd: f64,
}

#[derive(Debug, Deserialize)]
struct MorphoVaultPosition {
    vault: PositionVault,
    state: Option<VaultPositionState>,
}

#[derive(Debug, Deserialize)]
struct PositionVault {
    #[serde(default)]
    address: String,
    asset: Option<VaultPositionAsset>,
    state: Option<VaultNetApy>,
}

#[derive(Debug, Deserialize)]
struct VaultPositionAsset {
    #[serde(default)]
    address: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    decimals: i64,
}

#[derive(Debug, Deserialize)]
struct VaultNetApy {
    #[serde(
        rename = "netApy",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    net_apy: f64,
}

#[derive(Debug, Deserialize)]
struct VaultPositionState {
    #[serde(default)]
    shares: serde_json::Value,
    #[serde(default)]
    assets: serde_json::Value,
    #[serde(
        rename = "assetsUsd",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    assets_usd: f64,
}

#[derive(Debug, Deserialize)]
struct MorphoVault {
    #[serde(default)]
    address: String,
    asset: Option<SimpleAsset>,
    state: Option<VaultStateFull>,
    liquidity: Option<LiquidityUsd>,
}

#[derive(Debug, Default, Deserialize)]
struct SimpleAsset {
    #[serde(default)]
    address: String,
    #[serde(default)]
    symbol: String,
}

#[derive(Debug, Deserialize)]
struct VaultStateFull {
    #[serde(
        rename = "netApy",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    net_apy: f64,
    #[serde(
        rename = "totalAssetsUsd",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    total_assets_usd: f64,
    #[serde(default)]
    allocation: Vec<MarketAllocation>,
}

#[derive(Debug, Deserialize)]
struct LiquidityUsd {
    #[serde(default, deserialize_with = "crate::serde_util::de_f64_null_default")]
    usd: f64,
}

#[derive(Debug, Deserialize)]
struct MorphoVaultV2 {
    #[serde(default)]
    address: String,
    #[serde(
        rename = "netApy",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    net_apy: f64,
    #[serde(
        rename = "totalAssetsUsd",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    total_assets_usd: f64,
    #[serde(
        rename = "liquidityUsd",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    liquidity_usd: f64,
    asset: Option<SimpleAsset>,
    #[serde(rename = "liquidityData")]
    liquidity_data: Option<LiquidityData>,
}

#[derive(Debug, Deserialize)]
struct LiquidityData {
    #[serde(rename = "__typename", default)]
    typename: String,
    market: Option<LiquidityDataMarket>,
    #[serde(rename = "metaMorpho")]
    meta_morpho: Option<MetaMorpho>,
}

#[derive(Debug, Deserialize)]
struct LiquidityDataMarket {
    #[serde(rename = "loanAsset")]
    loan_asset: Option<SimpleAsset>,
    #[serde(rename = "collateralAsset")]
    collateral_asset: Option<SimpleAsset>,
}

#[derive(Debug, Deserialize)]
struct MetaMorpho {
    state: Option<MetaMorphoState>,
}

#[derive(Debug, Deserialize)]
struct MetaMorphoState {
    #[serde(default)]
    allocation: Vec<MarketAllocation>,
}

#[derive(Debug, Deserialize)]
struct MarketAllocation {
    #[serde(
        rename = "supplyAssetsUsd",
        default,
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    supply_assets_usd: f64,
    market: Option<AllocationMarket>,
}

#[derive(Debug, Deserialize)]
struct AllocationMarket {
    #[serde(rename = "loanAsset")]
    loan_asset: Option<SimpleAsset>,
    #[serde(rename = "collateralAsset")]
    collateral_asset: Option<SimpleAsset>,
}

// --- helpers (mirror the package-private Go helpers) ---

fn first_error(errors: &[GraphqlError]) -> Option<&str> {
    errors.first().map(|e| e.message.as_str())
}

fn is_morpho_no_results_error(message: &str) -> bool {
    message
        .trim()
        .to_ascii_lowercase()
        .contains("no results matching given parameters")
}

fn morpho_timeseries_interval(interval: YieldHistoryInterval) -> Result<&'static str, Error> {
    match interval {
        YieldHistoryInterval::Hour => Ok("HOUR"),
        YieldHistoryInterval::Day => Ok("DAY"),
    }
}

/// First-non-zero limit clamp mirroring the Go positions queries: `<=0` -> 200,
/// `<50` -> 50, otherwise the requested value.
fn clamp_first(limit: i64) -> i64 {
    if limit <= 0 {
        200
    } else if limit < 50 {
        50
    } else {
        limit
    }
}

fn convert_morpho_points(
    points: &[MorphoFloatDataPoint],
    percent: bool,
) -> Vec<model::YieldHistoryPoint> {
    let mut out: Vec<model::YieldHistoryPoint> = Vec::with_capacity(points.len());
    for point in points {
        let y = match point.y {
            Some(v) => v,
            None => continue,
        };
        let ts = Utc
            .timestamp_opt(point.x as i64, 0)
            .single()
            .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_default());
        let value = if percent { y * 100.0 } else { y };
        out.push(model::YieldHistoryPoint {
            timestamp: ts.to_rfc3339_opts(SecondsFormat::Secs, true),
            value,
        });
    }
    out.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    out
}

fn matches_vault_asset(vault_asset_address: &str, vault_asset_symbol: &str, asset: &Asset) -> bool {
    let addr = normalize_evm_address(&asset.address);
    if !addr.is_empty() {
        return normalize_evm_address(vault_asset_address).eq_ignore_ascii_case(&addr);
    }
    let symbol = asset.symbol.trim();
    if !symbol.is_empty() {
        return vault_asset_symbol.trim().eq_ignore_ascii_case(symbol);
    }
    true
}

fn collateral_shares_from_vault_v2(
    vault: &MorphoVaultV2,
    fallback_address: &str,
    fallback_symbol: &str,
) -> Vec<CollateralShare> {
    let liquidity_data = match &vault.liquidity_data {
        Some(d) => d,
        None => {
            let usd = yieldutil::positive_first(&[vault.total_assets_usd, vault.liquidity_usd]);
            if usd > 0.0 {
                return vec![CollateralShare {
                    address: fallback_address.to_string(),
                    symbol: fallback_symbol.to_string(),
                    usd,
                }];
            }
            return Vec::new();
        }
    };

    match liquidity_data.typename.as_str() {
        "MarketV1LiquidityData" => {
            let mut address = fallback_address.to_string();
            let mut symbol = String::new();
            if let Some(market) = &liquidity_data.market {
                if let Some(collateral) = &market.collateral_asset {
                    address = collateral.address.clone();
                    symbol = collateral.symbol.clone();
                } else if let Some(loan) = &market.loan_asset {
                    address = loan.address.clone();
                    symbol = loan.symbol.clone();
                }
            }
            if symbol.trim().is_empty() {
                symbol = fallback_symbol.to_string();
            }
            let usd = yieldutil::positive_first(&[vault.total_assets_usd, vault.liquidity_usd]);
            if usd <= 0.0 {
                return Vec::new();
            }
            vec![CollateralShare {
                address,
                symbol,
                usd,
            }]
        }
        "MetaMorphoLiquidityData" => {
            if let Some(meta) = &liquidity_data.meta_morpho {
                if let Some(state) = &meta.state {
                    let shares = collateral_shares_from_allocation(
                        vault.total_assets_usd,
                        &state.allocation,
                        fallback_address,
                        fallback_symbol,
                    );
                    if !shares.is_empty() {
                        return shares;
                    }
                }
            }
            fallback_collateral_share(vault, fallback_address, fallback_symbol)
        }
        _ => fallback_collateral_share(vault, fallback_address, fallback_symbol),
    }
}

fn fallback_collateral_share(
    vault: &MorphoVaultV2,
    fallback_address: &str,
    fallback_symbol: &str,
) -> Vec<CollateralShare> {
    let usd = yieldutil::positive_first(&[vault.total_assets_usd, vault.liquidity_usd]);
    if usd > 0.0 {
        return vec![CollateralShare {
            address: fallback_address.to_string(),
            symbol: fallback_symbol.to_string(),
            usd,
        }];
    }
    Vec::new()
}

fn collateral_shares_from_allocation(
    total_override: f64,
    allocation: &[MarketAllocation],
    fallback_address: &str,
    fallback_symbol: &str,
) -> Vec<CollateralShare> {
    let mut shares: Vec<CollateralShare> = Vec::with_capacity(allocation.len());
    let mut total = 0.0;
    for item in allocation {
        if item.supply_assets_usd > 0.0 {
            total += item.supply_assets_usd;
        }
    }
    for item in allocation {
        if item.supply_assets_usd <= 0.0 {
            continue;
        }
        let mut usd = item.supply_assets_usd;
        if total_override > 0.0 && total > 0.0 {
            usd = total_override * item.supply_assets_usd / total;
        }
        let mut address = fallback_address.to_string();
        let mut symbol = fallback_symbol.to_string();
        if let Some(market) = &item.market {
            if let Some(collateral) = &market.collateral_asset {
                address = collateral.address.clone();
                symbol = collateral.symbol.clone();
            } else if let Some(loan) = &market.loan_asset {
                address = loan.address.clone();
                symbol = loan.symbol.clone();
            }
        }
        if address.trim().is_empty() {
            address = fallback_address.to_string();
        }
        if symbol.trim().is_empty() {
            symbol = fallback_symbol.to_string();
        }
        shares.push(CollateralShare {
            address,
            symbol,
            usd,
        });
    }
    shares
}

struct BackingAggregate {
    symbol: String,
    usd: f64,
}

fn backing_assets_from_shares(
    shares: &[CollateralShare],
    chain_id: &str,
    fallback_address: &str,
    fallback_symbol: &str,
    fallback_asset_id: &str,
) -> Vec<model::YieldBackingAsset> {
    // Insertion-ordered aggregate map by asset_id (mirrors the Go map; final
    // output is sorted deterministically so insertion order is not contractual).
    let mut order: Vec<String> = Vec::new();
    let mut by_asset: HashMap<String, BackingAggregate> = HashMap::new();
    let mut total = 0.0;
    for share in shares {
        if share.usd <= 0.0 {
            continue;
        }
        let mut asset_id = canonical_asset_id_for_chain(chain_id, &share.address);
        let symbol = share.symbol.trim().to_string();
        if asset_id.is_empty() {
            asset_id = canonical_asset_id_for_chain(chain_id, fallback_address);
        }
        if asset_id.is_empty() {
            asset_id = fallback_asset_id.trim().to_string();
        }
        if asset_id.is_empty() {
            continue;
        }
        let symbol = if symbol.is_empty() {
            fallback_symbol.trim().to_string()
        } else {
            symbol
        };
        let entry = by_asset.entry(asset_id.clone()).or_insert_with(|| {
            order.push(asset_id.clone());
            BackingAggregate {
                symbol: String::new(),
                usd: 0.0,
            }
        });
        if entry.symbol.is_empty() {
            entry.symbol = symbol;
        }
        entry.usd += share.usd;
        total += share.usd;
    }

    if by_asset.is_empty() {
        let mut asset_id = canonical_asset_id_for_chain(chain_id, fallback_address);
        if asset_id.is_empty() {
            asset_id = fallback_asset_id.trim().to_string();
        }
        if asset_id.is_empty() {
            return Vec::new();
        }
        return vec![model::YieldBackingAsset {
            asset_id,
            symbol: fallback_symbol.trim().to_string(),
            share_pct: 100.0,
        }];
    }

    let mut out: Vec<model::YieldBackingAsset> = Vec::with_capacity(by_asset.len());
    for asset_id in &order {
        let item = &by_asset[asset_id];
        let share_pct = if total > 0.0 {
            (item.usd / total) * 100.0
        } else {
            0.0
        };
        out.push(model::YieldBackingAsset {
            asset_id: asset_id.clone(),
            symbol: item.symbol.trim().to_string(),
            share_pct,
        });
    }
    out.sort_by(|a, b| {
        desc_f64(a.share_pct, b.share_pct).then_with(|| a.asset_id.cmp(&b.asset_id))
    });
    out
}

fn source_url_for_vault(address: &str) -> String {
    let addr = normalize_evm_address(address);
    if addr.is_empty() {
        return SOURCE_URL.to_string();
    }
    format!("{SOURCE_URL}/vault/{addr}")
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

fn hash_opportunity(provider: &str, chain_id: &str, market_id: &str, asset_id: &str) -> String {
    let seed = [provider, chain_id, market_id, asset_id].join("|");
    let mut hasher = Sha1::new();
    hasher.update(seed.as_bytes());
    let digest = hasher.finalize();
    hex::encode(digest)
}

/// Normalize a JSON value that holds a big integer (string or number, possibly
/// `null`) into a canonical base-10 string. Non-positive / unparseable values
/// collapse to `"0"` (mirrors Go `bigintString.normalized`).
fn normalized_bigint(value: &serde_json::Value) -> String {
    let raw = match value {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => return "0".to_string(),
        other => other.to_string(),
    };
    if raw.is_empty() {
        return "0".to_string();
    }
    match BigInt::parse_bytes(raw.as_bytes(), 10) {
        Some(n) if n.sign() == num_bigint::Sign::Plus => n.to_string(),
        _ => "0".to_string(),
    }
}

fn normalize_evm_address(address: &str) -> String {
    let addr = address.trim().to_ascii_lowercase();
    if addr.len() != 42 || !addr.starts_with("0x") {
        return String::new();
    }
    addr
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

fn amount_info_from_base(base: &str, decimals: i64) -> model::AmountInfo {
    let decimals = decimals.max(0);
    model::AmountInfo {
        amount_base_units: base.to_string(),
        amount_decimal: format_decimal(base, decimals as i32),
        decimals,
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

/// Compare two `f64` values for a DESCENDING sort, total-order safe.
fn desc_f64(a: f64, b: f64) -> std::cmp::Ordering {
    b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
}

#[cfg(test)]
#[allow(clippy::doc_lazy_continuation)]
mod tests {
    //! # Success criteria for the `morpho` provider adapter
    //!
    //! Go source: `internal/providers/morpho/client.go`; ported behavioral cases
    //! from `internal/providers/morpho/client_test.go`. External HTTP (Morpho's
    //! GraphQL endpoint) is mocked with `wiremock` (the Rust analogue of Go's
    //! `httptest.Server`). The single endpoint is routed by GraphQL operation
    //! name embedded in the POST body (`query Markets(`, `query Vaults(`, …),
    //! exactly as the Go fixtures switch on `strings.Contains(query, ...)`.
    //!
    //! Morpho is a lending + yield adapter (markets/rates/positions + yield
    //! opportunities/positions/history). It implements `LendingProvider`,
    //! `LendingPositionsProvider`, `YieldProvider`, `YieldPositionsProvider`, and
    //! `YieldHistoryProvider`, plus `Provider` metadata. All outputs are
    //! deterministic (stable multi-key sorts) and every APY field is a PERCENTAGE
    //! POINT, not a ratio (spec §2.5): the adapter scales the GraphQL ratio
    //! (`0.02`) by 100 to the contract value (`2.0`).
    //!
    //! The `Client` exposes the same two test seams as `aave`:
    //!   * `set_endpoint(&url)` — point the GraphQL endpoint at a `wiremock`
    //!     server (Go `client.endpoint = srv.URL`).
    //!   * `set_now(DateTime<Utc>)` — pin the clock (Go `client.now`).
    //!
    //! ## Criteria
    //!
    //!  M0. **Provider metadata** (`Provider::info`). `name == "morpho"`,
    //!      `provider_type == "lending+yield"`, `requires_key == false`, and the
    //!      read capabilities are present. Callable as metadata WITHOUT a key.
    //!
    //!  M1. **LendRates** (Go `TestLendRatesAndYield`). POSTs `query Markets(`;
    //!      for the USDC market it emits one `LendRate` with `provider == "morpho"`,
    //!      `provider_native_id == "m1"`, `provider_native_id_kind == market_id`,
    //!      and `supply_apy == 2.0` (ratio `0.02` ×100).
    //!
    //!  M2. **YieldOpportunities vault + vaultV2 normalization** (Go
    //!      `TestLendRatesAndYield`). Fetches `query Vaults(` and `query VaultV2s(`.
    //!      A USDC request yields exactly TWO opportunities (a v1 vault + a USDC
    //!      v2 vault); the USDT v2 vault is filtered out. Each carries
    //!      `provider == "morpho"`, `provider_native_id_kind == vault_address`.
    //!      The v1 vault's `liquidity_usd` comes from `liquidity.usd` (`500000`)
    //!      and exposes a single full-share `WETH` backing asset; the v2 vault's
    //!      `liquidity_usd` comes from `liquidityUsd` (`1500000`) and exposes a
    //!      single full-share `DAI` backing asset (from the MetaMorpho allocation).
    //!
    //!  M3. **YieldOpportunities sort + limit** (Go
    //!      `TestYieldOpportunitiesVaultSortAndLimit`). With `sort_by=tvl_usd` and
    //!      `limit=1`, the single returned opportunity is the highest-TVL vault
    //!      (`0x2222...`, `2_000_000` > `1_000_000`), proving the shared yield
    //!      sort is APPLIED and the limit honored.
    //!
    //!  M4. **LendPositions type split** (Go `TestLendPositionsTypeSplit`). POSTs
    //!      `marketPositions`. A single market position with non-zero
    //!      supply/borrow/collateral yields THREE non-overlapping rows under
    //!      `type=all` (`supply`, `borrow`, `collateral`). `type=supply` returns
    //!      ONLY the supply row. An `asset=USDC` filter keeps the loan-asset rows
    //!      (supply + borrow) and drops the WETH collateral row. Each row carries
    //!      `provider_native_id_kind == market_id` and an `amount` whose
    //!      `amount_base_units` is the raw GraphQL string.
    //!
    //!  M5. **YieldPositions vaults** (Go `TestYieldPositionsVaults`). POSTs
    //!      `vaultPositions`. With an `asset=USDC` filter, the USDT vault row is
    //!      dropped, leaving ONE row with `position_type == "deposit"`,
    //!      `provider_native_id_kind == vault_address`, `amount.amount_base_units
    //!      == "10100000"`, `shares.amount_base_units == "10000000000000000000"`
    //!      (18-decimal shares), and `apy_total == 4.0` (ratio `0.04` ×100).
    //!
    //!  M6. **YieldHistory from vault** (Go `TestYieldHistoryFromVault`). POSTs
    //!      `query VaultHistory(`. With both `apy_total` + `tvl_usd` metrics it
    //!      returns TWO series; the apy points are ratio ×100 (`0.03 -> 3.0`), the
    //!      tvl points are passed through (`1000000`), each filtered/sorted by
    //!      timestamp.
    //!
    //!  M7. **YieldHistory falls back to vaultV2** (Go
    //!      `TestYieldHistoryFallsBackToVaultV2`). When `query VaultHistory(`
    //!      returns the "No results matching given parameters" error + null vault,
    //!      the adapter retries `query VaultV2History(` and uses its `avgNetApy`
    //!      (`0.04 -> 4.0`).
    //!
    //!  M8. **YieldHistory rejects a foreign opportunity provider.** An
    //!      opportunity whose `provider` is not `morpho` (case-insensitive) -> typed
    //!      `Unsupported`, with no network call.
    //!
    //! ## Go tests intentionally SKIPPED here (owned elsewhere / not this module)
    //!   * `yieldutil::sort_opportunities` tie-break internals — owned by the
    //!     `yieldutil` RED suite; M3 only asserts the sort is APPLIED + limited.
    //!   * `format_decimal` / amount-normalization internals — owned by `defi-id`;
    //!     exercised here indirectly through `amount_base_units` assertions.
    //!   * Low-level helper internals (`normalized_bigint`, `hash_opportunity`,
    //!     `canonical_asset_id*`) — exercised through the public method outputs.

    use std::time::Duration;

    use chrono::{TimeZone, Utc};
    use defi_errors::Code;
    use defi_httpx::Client as HttpClient;
    use defi_id::{parse_asset, parse_chain, Asset};
    use defi_model as model;
    use wiremock::matchers::{body_string_contains, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::morpho::Client;
    use crate::traits::{
        LendPositionType, LendPositionsRequest, LendingPositionsProvider, LendingProvider,
        Provider, YieldHistoryInterval, YieldHistoryMetric, YieldHistoryProvider,
        YieldHistoryRequest, YieldPositionsProvider, YieldPositionsRequest, YieldProvider,
        YieldRequest,
    };

    fn http() -> HttpClient {
        HttpClient::new(Duration::from_secs(2), 0)
    }

    const USDC_ETH: &str = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
    const DEAD: &str = "0x000000000000000000000000000000000000dEaD";

    fn yield_req(chain: defi_id::Chain, asset: Asset, limit: i64, sort_by: &str) -> YieldRequest {
        YieldRequest {
            chain,
            asset,
            limit,
            min_tvl_usd: 0.0,
            min_apy: 0.0,
            providers: vec!["morpho".to_string()],
            sort_by: sort_by.to_string(),
            include_incomplete: false,
        }
    }

    fn lend_positions_req(
        chain: defi_id::Chain,
        account: &str,
        position_type: LendPositionType,
        asset: Asset,
    ) -> LendPositionsRequest {
        LendPositionsRequest {
            chain,
            account: account.to_string(),
            asset,
            position_type,
            limit: 0,
            rpc_url: String::new(),
        }
    }

    // ----- M0: provider metadata (callable without a key) ------------------

    #[test]
    fn info_is_metadata_only_no_key_required() {
        let client = Client::new(http());
        let info = client.info();
        assert_eq!(info.name, "morpho");
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

    // ----- shared fixtures for M1/M2/M3 ------------------------------------

    fn markets_body() -> String {
        format!(
            r#"{{
                "data": {{
                    "markets": {{
                        "items": [
                            {{
                                "id": "4f598145-0188-44dc-9e18-38a2817020a1",
                                "uniqueKey": "m1",
                                "irmAddress": "0x870aC11D48B15DB9a138Cf899d20F13F79Ba00BC",
                                "loanAsset": {{"address": "{USDC_ETH}", "symbol": "USDC", "decimals": 6, "chain": {{"id": 1, "network": "ethereum"}}}},
                                "collateralAsset": {{"address": "0x111", "symbol": "WETH"}},
                                "state": {{"supplyApy": 0.02, "borrowApy": 0.03, "utilization": 0.5, "supplyAssetsUsd": 2000000, "liquidityAssetsUsd": 1000000, "totalLiquidityUsd": 1200000}}
                            }}
                        ]
                    }}
                }}
            }}"#
        )
    }

    fn vaults_body() -> String {
        format!(
            r#"{{
                "data": {{
                    "vaults": {{
                        "items": [
                            {{
                                "address": "0x1111111111111111111111111111111111111111",
                                "name": "Morpho USDC Vault",
                                "symbol": "vUSDC",
                                "asset": {{"address": "{USDC_ETH}", "symbol": "USDC"}},
                                "state": {{
                                    "netApy": 0.05,
                                    "totalAssetsUsd": 1000000,
                                    "allocation": [
                                        {{
                                            "supplyAssetsUsd": 1000000,
                                            "market": {{"loanAsset": {{"address": "{USDC_ETH}", "symbol": "USDC"}}, "collateralAsset": {{"address": "0x4200000000000000000000000000000000000006", "symbol": "WETH"}}}}
                                        }}
                                    ]
                                }},
                                "liquidity": {{"usd": 500000}}
                            }}
                        ]
                    }}
                }}
            }}"#
        )
    }

    fn vault_v2s_body() -> String {
        format!(
            r#"{{
                "data": {{
                    "vaultV2s": {{
                        "items": [
                            {{
                                "address": "0x2222222222222222222222222222222222222222",
                                "name": "Morpho USDC V2 Vault",
                                "symbol": "v2USDC",
                                "asset": {{"address": "{USDC_ETH}", "symbol": "USDC"}},
                                "netApy": 0.03,
                                "totalAssetsUsd": 2000000,
                                "liquidityUsd": 1500000,
                                "liquidityData": {{
                                    "__typename": "MetaMorphoLiquidityData",
                                    "metaMorpho": {{
                                        "state": {{
                                            "allocation": [
                                                {{
                                                    "supplyAssetsUsd": 2000000,
                                                    "market": {{"loanAsset": {{"address": "{USDC_ETH}", "symbol": "USDC"}}, "collateralAsset": {{"address": "0x6b175474e89094c44da98b954eedeac495271d0f", "symbol": "DAI"}}}}
                                                }}
                                            ]
                                        }}
                                    }}
                                }}
                            }},
                            {{
                                "address": "0x3333333333333333333333333333333333333333",
                                "name": "Morpho USDT V2 Vault",
                                "symbol": "v2USDT",
                                "asset": {{"address": "0xdac17f958d2ee523a2206206994597c13d831ec7", "symbol": "USDT"}},
                                "netApy": 0.09,
                                "totalAssetsUsd": 3000000,
                                "liquidityUsd": 2500000,
                                "liquidityData": {{"__typename": "MetaMorphoLiquidityData"}}
                            }}
                        ]
                    }}
                }}
            }}"#
        )
    }

    /// Mount the markets/vaults/vaultV2s handlers routed by operation name; any
    /// other query gets an empty-markets payload (mirrors the Go `default` arm).
    async fn mount_markets_and_yield(server: &MockServer) {
        Mock::given(method("POST"))
            .and(body_string_contains("query Markets("))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(markets_body(), "application/json"),
            )
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(body_string_contains("query Vaults("))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(vaults_body(), "application/json"),
            )
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(body_string_contains("query VaultV2s("))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(vault_v2s_body(), "application/json"),
            )
            .mount(server)
            .await;
        // Fallback (markets-with-no-items) for any other operation.
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"data":{"markets":{"items":[]}}}"#, "application/json"),
            )
            .mount(server)
            .await;
    }

    // ----- M1: LendRates ---------------------------------------------------

    #[tokio::test]
    async fn lend_rates_scales_apy_and_carries_market_id() {
        let server = MockServer::start().await;
        mount_markets_and_yield(&server).await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let rates = client
            .lend_rates("morpho", chain, asset)
            .await
            .expect("lend_rates");
        assert_eq!(rates.len(), 1);
        let r = &rates[0];
        assert_eq!(r.supply_apy, 2.0);
        assert_eq!(r.provider, "morpho");
        assert_eq!(r.provider_native_id, "m1");
        assert_eq!(r.provider_native_id_kind, model::NATIVE_ID_KIND_MARKET_ID);
    }

    #[tokio::test]
    async fn lend_rates_rejects_foreign_provider() {
        let server = MockServer::start().await;
        mount_markets_and_yield(&server).await;
        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let err = client
            .lend_rates("aave", chain, asset)
            .await
            .expect_err("foreign provider rejected");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- M2: YieldOpportunities normalization ----------------------------

    #[tokio::test]
    async fn yield_opportunities_normalizes_vault_and_vault_v2() {
        let server = MockServer::start().await;
        mount_markets_and_yield(&server).await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let opps = client
            .yield_opportunities(yield_req(chain, asset, 10, ""))
            .await
            .expect("yield_opportunities");
        assert_eq!(opps.len(), 2, "unexpected opportunities: {opps:?}");

        let mut by_id = std::collections::HashMap::new();
        for opp in &opps {
            assert_eq!(opp.provider, "morpho");
            by_id.insert(opp.provider_native_id.clone(), opp.clone());
        }

        let vault_one = by_id
            .get("0x1111111111111111111111111111111111111111")
            .expect("first vault present");
        assert_eq!(
            vault_one.provider_native_id_kind,
            model::NATIVE_ID_KIND_VAULT_ADDRESS
        );
        assert_eq!(vault_one.liquidity_usd, 500_000.0);
        assert_eq!(vault_one.backing_assets.len(), 1);
        assert_eq!(vault_one.backing_assets[0].symbol, "WETH");
        assert_eq!(vault_one.backing_assets[0].share_pct, 100.0);

        let vault_two = by_id
            .get("0x2222222222222222222222222222222222222222")
            .expect("second vault present");
        assert_eq!(
            vault_two.provider_native_id_kind,
            model::NATIVE_ID_KIND_VAULT_ADDRESS
        );
        assert_eq!(vault_two.liquidity_usd, 1_500_000.0);
        assert_eq!(vault_two.backing_assets.len(), 1);
        assert_eq!(vault_two.backing_assets[0].symbol, "DAI");
        assert_eq!(vault_two.backing_assets[0].share_pct, 100.0);

        assert!(
            !by_id.contains_key("0x3333333333333333333333333333333333333333"),
            "USDT vault must be filtered out for USDC request"
        );
    }

    // ----- M3: YieldOpportunities sort + limit -----------------------------

    #[tokio::test]
    async fn yield_opportunities_sort_and_limit() {
        let server = MockServer::start().await;
        // Distinct fixture: a v1 vault (tvl 1M) + a v2 vault (tvl 2M).
        Mock::given(method("POST"))
            .and(body_string_contains("query Vaults("))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{
                        "data": {{
                            "vaults": {{
                                "items": [
                                    {{
                                        "address": "0x1111111111111111111111111111111111111111",
                                        "name": "Morpho USDC Vault",
                                        "symbol": "vUSDC",
                                        "asset": {{"address": "{USDC_ETH}", "symbol": "USDC"}},
                                        "state": {{
                                            "netApy": 0.06,
                                            "totalAssetsUsd": 1000000,
                                            "allocation": [
                                                {{
                                                    "supplyAssetsUsd": 1000000,
                                                    "market": {{"loanAsset": {{"address": "{USDC_ETH}", "symbol": "USDC"}}, "collateralAsset": {{"address": "0x4200000000000000000000000000000000000006", "symbol": "WETH"}}}}
                                                }}
                                            ]
                                        }},
                                        "liquidity": {{"usd": 700000}}
                                    }}
                                ]
                            }}
                        }}
                    }}"#
                ),
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_string_contains("query VaultV2s("))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{
                        "data": {{
                            "vaultV2s": {{
                                "items": [
                                    {{
                                        "address": "0x2222222222222222222222222222222222222222",
                                        "name": "Morpho USDC V2 Vault",
                                        "symbol": "v2USDC",
                                        "asset": {{"address": "{USDC_ETH}", "symbol": "USDC"}},
                                        "netApy": 0.03,
                                        "totalAssetsUsd": 2000000,
                                        "liquidityUsd": 1800000,
                                        "liquidityData": {{
                                            "__typename": "MetaMorphoLiquidityData",
                                            "metaMorpho": {{
                                                "state": {{
                                                    "allocation": [
                                                        {{
                                                            "supplyAssetsUsd": 2000000,
                                                            "market": {{"loanAsset": {{"address": "{USDC_ETH}", "symbol": "USDC"}}, "collateralAsset": {{"address": "0x6b175474e89094c44da98b954eedeac495271d0f", "symbol": "DAI"}}}}
                                                        }}
                                                    ]
                                                }}
                                            }}
                                        }}
                                    }}
                                ]
                            }}
                        }}
                    }}"#
                ),
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"data":{"markets":{"items":[]}}}"#, "application/json"),
            )
            .mount(&server)
            .await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let opps = client
            .yield_opportunities(yield_req(chain, asset, 1, "tvl_usd"))
            .await
            .expect("yield_opportunities");
        assert_eq!(opps.len(), 1, "limit honored");
        assert_eq!(
            opps[0].provider_native_id, "0x2222222222222222222222222222222222222222",
            "highest-tvl vault first"
        );
    }

    // ----- M4: LendPositions type split ------------------------------------

    fn positions_body() -> String {
        format!(
            r#"{{
                "data": {{
                    "marketPositions": {{
                        "items": [
                            {{
                                "id": "position-1",
                                "market": {{
                                    "uniqueKey": "market-1",
                                    "loanAsset": {{"address": "{USDC_ETH}", "symbol": "USDC", "decimals": 6, "chain": {{"id": 1, "network": "ethereum"}}}},
                                    "collateralAsset": {{"address": "0x4200000000000000000000000000000000000006", "symbol": "WETH", "decimals": 18}},
                                    "state": {{"supplyApy": 0.02, "borrowApy": 0.03}}
                                }},
                                "state": {{
                                    "supplyAssets": "1500000",
                                    "supplyAssetsUsd": 1.5,
                                    "borrowAssets": "500000",
                                    "borrowAssetsUsd": 0.5,
                                    "collateral": "1000000000000000000",
                                    "collateralUsd": 2000
                                }}
                            }}
                        ]
                    }}
                }}
            }}"#
        )
    }

    async fn mount_positions(server: &MockServer) {
        Mock::given(method("POST"))
            .and(body_string_contains("marketPositions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(positions_body(), "application/json"),
            )
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn lend_positions_splits_by_type_and_filters_by_asset() {
        let server = MockServer::start().await;
        mount_positions(&server).await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let all = client
            .lend_positions(lend_positions_req(
                chain.clone(),
                DEAD,
                LendPositionType::All,
                Asset::default(),
            ))
            .await
            .expect("lend_positions all");
        assert_eq!(all.len(), 3, "three distinct positions");
        let mut counts = std::collections::HashMap::new();
        for item in &all {
            *counts.entry(item.position_type.clone()).or_insert(0) += 1;
            assert_eq!(
                item.provider_native_id_kind,
                model::NATIVE_ID_KIND_MARKET_ID
            );
        }
        assert_eq!(counts.get("supply"), Some(&1));
        assert_eq!(counts.get("borrow"), Some(&1));
        assert_eq!(counts.get("collateral"), Some(&1));

        // raw base units preserved.
        let supply = all.iter().find(|p| p.position_type == "supply").unwrap();
        assert_eq!(supply.amount.amount_base_units, "1500000");

        let supply_only = client
            .lend_positions(lend_positions_req(
                chain.clone(),
                DEAD,
                LendPositionType::Supply,
                Asset::default(),
            ))
            .await
            .expect("lend_positions supply");
        assert_eq!(supply_only.len(), 1);
        assert_eq!(supply_only[0].position_type, "supply");

        let usdc_only = client
            .lend_positions(lend_positions_req(
                chain.clone(),
                DEAD,
                LendPositionType::All,
                Asset {
                    chain_id: chain.caip2.clone(),
                    symbol: "USDC".to_string(),
                    ..Asset::default()
                },
            ))
            .await
            .expect("lend_positions usdc");
        assert_eq!(usdc_only.len(), 2, "supply + borrow for USDC filter");
        for item in &usdc_only {
            assert!(item.position_type == "supply" || item.position_type == "borrow");
        }
    }

    #[tokio::test]
    async fn lend_positions_rejects_missing_account() {
        let server = MockServer::start().await;
        mount_positions(&server).await;
        let chain = parse_chain("ethereum").expect("parse ethereum");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let err = client
            .lend_positions(lend_positions_req(
                chain,
                "not-an-address",
                LendPositionType::All,
                Asset::default(),
            ))
            .await
            .expect_err("missing account rejected");
        assert_eq!(err.code, Code::Usage);
    }

    // ----- M5: YieldPositions vaults ---------------------------------------

    #[tokio::test]
    async fn yield_positions_vaults_filtered_by_asset() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("vaultPositions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "data": {
                        "vaultPositions": {
                            "items": [
                                {
                                    "id": "vault-position-1",
                                    "user": {"address": "0x000000000000000000000000000000000000dEaD"},
                                    "vault": {
                                        "address": "0x1111111111111111111111111111111111111111",
                                        "asset": {"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "symbol": "USDC", "decimals": 6, "chain": {"id": 1, "network": "ethereum"}},
                                        "state": {"netApy": 0.04}
                                    },
                                    "state": {"shares": "10000000000000000000", "assets": "10100000", "assetsUsd": 10.1}
                                },
                                {
                                    "id": "vault-position-2",
                                    "user": {"address": "0x000000000000000000000000000000000000dEaD"},
                                    "vault": {
                                        "address": "0x2222222222222222222222222222222222222222",
                                        "asset": {"address": "0xdac17f958d2ee523a2206206994597c13d831ec7", "symbol": "USDT", "decimals": 6, "chain": {"id": 1, "network": "ethereum"}},
                                        "state": {"netApy": 0.06}
                                    },
                                    "state": {"shares": "5000000000000000000", "assets": "5050000", "assetsUsd": 5.05}
                                }
                            ]
                        }
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());

        let rows = client
            .yield_positions(YieldPositionsRequest {
                chain: chain.clone(),
                account: DEAD.to_string(),
                asset: Asset {
                    chain_id: chain.caip2.clone(),
                    symbol: "USDC".to_string(),
                    ..Asset::default()
                },
                limit: 0,
                rpc_url: String::new(),
            })
            .await
            .expect("yield_positions");
        assert_eq!(rows.len(), 1, "one USDC vault row");
        let row = &rows[0];
        assert_eq!(row.position_type, "deposit");
        assert_eq!(
            row.provider_native_id_kind,
            model::NATIVE_ID_KIND_VAULT_ADDRESS
        );
        assert_eq!(row.amount.amount_base_units, "10100000");
        let shares = row.shares.as_ref().expect("shares present");
        assert_eq!(shares.amount_base_units, "10000000000000000000");
        assert_eq!(row.apy_total, 4.0);
    }

    // ----- M6: YieldHistory from vault -------------------------------------

    fn morpho_opportunity(native_id: &str, opp_id: &str) -> model::YieldOpportunity {
        model::YieldOpportunity {
            opportunity_id: opp_id.to_string(),
            provider: "morpho".to_string(),
            protocol: "morpho".to_string(),
            chain_id: "eip155:1".to_string(),
            asset_id: format!("eip155:1/erc20:{USDC_ETH}"),
            provider_native_id: native_id.to_string(),
            provider_native_id_kind: model::NATIVE_ID_KIND_VAULT_ADDRESS.to_string(),
            opportunity_type: "lend".to_string(),
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
        }
    }

    #[tokio::test]
    async fn yield_history_from_vault() {
        let fixed_now = Utc
            .with_ymd_and_hms(2026, 2, 26, 20, 0, 0)
            .single()
            .unwrap();
        let start = fixed_now - chrono::Duration::hours(48);

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("query VaultHistory("))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "data": {
                        "vaultByAddress": {
                            "address": "0x1111111111111111111111111111111111111111",
                            "historicalState": {
                                "netApy": [{"x": 1771981200, "y": 0.03}, {"x": 1772067600, "y": 0.031}],
                                "totalAssetsUsd": [{"x": 1771981200, "y": 1000000}, {"x": 1772067600, "y": 1100000}]
                            }
                        }
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());
        client.set_now(fixed_now);

        let series = client
            .yield_history(YieldHistoryRequest {
                opportunity: morpho_opportunity(
                    "0x1111111111111111111111111111111111111111",
                    "opp-1",
                ),
                start_time: start,
                end_time: fixed_now,
                interval: YieldHistoryInterval::Day,
                metrics: vec![YieldHistoryMetric::ApyTotal, YieldHistoryMetric::TvlUsd],
            })
            .await
            .expect("yield_history");
        assert_eq!(series.len(), 2);
        let mut by_metric = std::collections::HashMap::new();
        for item in &series {
            by_metric.insert(item.metric.clone(), item.clone());
        }
        let apy = by_metric.get("apy_total").expect("apy series");
        assert_eq!(apy.points.len(), 2);
        assert_eq!(apy.points[0].value, 3.0);
        let tvl = by_metric.get("tvl_usd").expect("tvl series");
        assert_eq!(tvl.points.len(), 2);
        assert_eq!(tvl.points[0].value, 1_000_000.0);
    }

    // ----- M7: YieldHistory falls back to vaultV2 --------------------------

    #[tokio::test]
    async fn yield_history_falls_back_to_vault_v2() {
        let fixed_now = Utc
            .with_ymd_and_hms(2026, 2, 26, 20, 0, 0)
            .single()
            .unwrap();
        let start = fixed_now - chrono::Duration::hours(48);

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("query VaultHistory("))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"data":{"vaultByAddress":null},"errors":[{"message":"No results matching given parameters"}]}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_string_contains("query VaultV2History("))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "data": {
                        "vaultV2ByAddress": {
                            "address": "0x2222222222222222222222222222222222222222",
                            "historicalState": {
                                "avgNetApy": [{"x": 1771981200, "y": 0.04}],
                                "totalAssetsUsd": [{"x": 1771981200, "y": 2000000}]
                            }
                        }
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http());
        client.set_endpoint(&server.uri());
        client.set_now(fixed_now);

        let series = client
            .yield_history(YieldHistoryRequest {
                opportunity: morpho_opportunity(
                    "0x2222222222222222222222222222222222222222",
                    "opp-2",
                ),
                start_time: start,
                end_time: fixed_now,
                interval: YieldHistoryInterval::Day,
                metrics: vec![YieldHistoryMetric::ApyTotal],
            })
            .await
            .expect("yield_history");
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].points.len(), 1);
        assert_eq!(series[0].points[0].value, 4.0);
    }

    // ----- M8: YieldHistory rejects foreign opportunity provider -----------

    #[tokio::test]
    async fn yield_history_rejects_foreign_provider() {
        let fixed_now = Utc
            .with_ymd_and_hms(2026, 2, 26, 20, 0, 0)
            .single()
            .unwrap();
        let start = fixed_now - chrono::Duration::hours(48);

        let client = Client::new(http());
        // No endpoint set: a network call would fail to connect, but the guard
        // returns before any HTTP is attempted.
        let mut opp = morpho_opportunity("0x1111111111111111111111111111111111111111", "opp-x");
        opp.provider = "aave".to_string();

        let err = client
            .yield_history(YieldHistoryRequest {
                opportunity: opp,
                start_time: start,
                end_time: fixed_now,
                interval: YieldHistoryInterval::Day,
                metrics: vec![YieldHistoryMetric::ApyTotal],
            })
            .await
            .expect_err("foreign provider rejected");
        assert_eq!(err.code, Code::Unsupported);
    }
}
