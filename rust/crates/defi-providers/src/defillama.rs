//! DefiLlama provider adapter. Market + bridge-data adapter.
//!
//! Go source: `internal/providers/defillama/client.go` (+ `client_test.go`).
//!
//! Implements the `MarketDataProvider` (chains/protocols/stablecoins/fees/
//! revenue/dexes) and `BridgeDataProvider` (bridge list/details) trait surfaces,
//! plus `Provider` metadata. All outputs are deterministic (stable sort +
//! sequential ranks); numeric fields carry raw USD/percentage values (APY/
//! percent are points, not ratios — spec §2.5).

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use defi_errors::{Code, Error};
use defi_httpx::Client as HttpClient;
use defi_id::{known_token, parse_chain, Asset, Chain};
use defi_model as model;
use reqwest::{Method, Request, Url};
use serde::Deserialize;

use crate::traits::{
    BridgeDataProvider, BridgeDetailsRequest, BridgeListRequest, MarketDataProvider, Provider,
};

/// Free-endpoint API base (`https://api.llama.fi`).
const DEFAULT_API_BASE: &str = "https://api.llama.fi";
/// Key-gated bridge + chainAssets API base (`https://pro-api.llama.fi`).
const DEFAULT_BRIDGE_API_URL: &str = "https://pro-api.llama.fi";
/// Stablecoins API base (`https://stablecoins.llama.fi`).
const DEFAULT_STABLECOINS_API_URL: &str = "https://stablecoins.llama.fi";

/// DefiLlama market + bridge-data adapter (mirrors Go `defillama.Client`).
pub struct Client {
    http: HttpClient,
    api_base: String,
    bridge_base_url: String,
    stablecoins_api_url: String,
    api_key: String,
    /// Injected fixed clock (UNIX seconds) for deterministic `fetched_at` /
    /// `last_updated_unix` stamps in tests; `None` uses the wall clock.
    now_unix: Option<i64>,
}

impl Client {
    /// Build a client with default DefiLlama base URLs (mirrors Go `New`).
    ///
    /// The API key is trimmed; an empty key leaves key-gated routes callable
    /// only as metadata (`Provider::info`).
    pub fn new(http: HttpClient, api_key: &str) -> Self {
        Client {
            http,
            api_base: DEFAULT_API_BASE.to_string(),
            bridge_base_url: DEFAULT_BRIDGE_API_URL.to_string(),
            stablecoins_api_url: DEFAULT_STABLECOINS_API_URL.to_string(),
            api_key: api_key.trim().to_string(),
            now_unix: None,
        }
    }

    /// Override the free-endpoint API base (test seam for Go `apiBase`).
    pub fn set_api_base(&mut self, base: &str) {
        self.api_base = base.to_string();
    }

    /// Override the bridge/chainAssets API base (test seam for Go `bridgeBaseURL`).
    pub fn set_bridge_base_url(&mut self, base: &str) {
        self.bridge_base_url = base.to_string();
    }

    /// Override the stablecoins API base (test seam for Go `stablecoinsAPIURL`).
    pub fn set_stablecoins_api_url(&mut self, base: &str) {
        self.stablecoins_api_url = base.to_string();
    }

    /// Inject a fixed clock (UNIX seconds) so `fetched_at` / `last_updated_unix`
    /// are deterministic (test seam for Go `now`).
    pub fn set_now_unix(&mut self, unix: i64) {
        self.now_unix = Some(unix);
    }

    /// The current UNIX seconds: the injected clock if set, else the wall clock.
    fn now_unix(&self) -> i64 {
        self.now_unix.unwrap_or_else(|| Utc::now().timestamp())
    }

    /// Build a GET request to `url`, mapping a parse failure onto an internal
    /// error with `ctx`.
    fn build_get(&self, url: &str, ctx: &'static str) -> Result<Request, Error> {
        let parsed = Url::parse(url).map_err(|e| Error::wrap(Code::Internal, ctx, e))?;
        Ok(Request::new(Method::GET, parsed))
    }

    fn require_chain_assets_api_key(&self) -> Result<(), Error> {
        if self.api_key.trim().is_empty() {
            return Err(Error::new(
                Code::Auth,
                "defillama chain asset tvl requires DEFI_DEFILLAMA_API_KEY",
            ));
        }
        Ok(())
    }

    fn require_bridge_api_key(&self) -> Result<(), Error> {
        if self.api_key.trim().is_empty() {
            return Err(Error::new(
                Code::Auth,
                "defillama bridge data requires DEFI_DEFILLAMA_API_KEY",
            ));
        }
        Ok(())
    }

    /// The `/<key>/api/chainAssets` endpoint (mirrors Go `chainAssetsURL`).
    fn chain_assets_url(&self) -> String {
        let base = self.bridge_base_url.trim_end_matches('/');
        format!("{base}/{}/api/chainAssets", self.api_key)
    }

    /// The `/<key>/bridges/<path>` endpoint (mirrors Go `bridgeURL`).
    fn bridge_url(&self, path: &str, query: &[(&str, &str)]) -> String {
        let clean_path = path.trim().trim_start_matches('/');
        let base = self.bridge_base_url.trim_end_matches('/');
        let endpoint = format!("{base}/{}/bridges/{clean_path}", self.api_key);
        if query.is_empty() {
            endpoint
        } else {
            let qs: Vec<String> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!("{endpoint}?{}", qs.join("&"))
        }
    }
}

// ----- wire response shapes ------------------------------------------------

#[derive(Debug, Deserialize)]
struct ChainResp {
    #[serde(default)]
    name: String,
    #[serde(default, deserialize_with = "crate::serde_util::de_f64_null_default")]
    tvl: f64,
}

#[derive(Debug, Deserialize)]
struct ChainAssetsCategory {
    #[serde(default)]
    breakdown: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ProtocolResp {
    #[serde(default)]
    name: String,
    #[serde(default)]
    category: String,
    #[serde(default, deserialize_with = "crate::serde_util::de_f64_null_default")]
    tvl: f64,
    #[serde(default)]
    chains: Vec<String>,
    #[serde(
        rename = "chainTvls",
        default,
        deserialize_with = "crate::serde_util::de_f64_map_null_default"
    )]
    chain_tvls: HashMap<String, f64>,
}

#[derive(Debug, Deserialize)]
struct FeesProtocolResp {
    #[serde(default)]
    name: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    total24h: Option<f64>,
    #[serde(default)]
    total7d: Option<f64>,
    #[serde(default)]
    total30d: Option<f64>,
    #[serde(default)]
    change_1d: Option<f64>,
    #[serde(default)]
    change_7d: Option<f64>,
    #[serde(default)]
    change_1m: Option<f64>,
    #[serde(default)]
    chains: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FeesOverviewResp {
    #[serde(default)]
    protocols: Vec<FeesProtocolResp>,
}

#[derive(Debug, Deserialize)]
struct StablecoinResp {
    #[serde(default)]
    name: String,
    #[serde(default)]
    symbol: String,
    #[serde(rename = "pegType", default)]
    peg_type: String,
    #[serde(rename = "pegMechanism", default)]
    peg_mechanism: String,
    #[serde(
        default,
        deserialize_with = "crate::serde_util::de_f64_map_null_default"
    )]
    circulating: HashMap<String, f64>,
    #[serde(
        rename = "circulatingPrevDay",
        default,
        deserialize_with = "crate::serde_util::de_f64_map_null_default"
    )]
    circulating_prev_day: HashMap<String, f64>,
    #[serde(
        rename = "circulatingPrevWeek",
        default,
        deserialize_with = "crate::serde_util::de_f64_map_null_default"
    )]
    circulating_prev_week: HashMap<String, f64>,
    #[serde(
        rename = "circulatingPrevMonth",
        default,
        deserialize_with = "crate::serde_util::de_f64_map_null_default"
    )]
    circulating_prev_month: HashMap<String, f64>,
    #[serde(default)]
    chains: Vec<String>,
    #[serde(default)]
    price: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct StablecoinsEnvelope {
    #[serde(rename = "peggedAssets", default)]
    pegged_assets: Vec<StablecoinResp>,
}

#[derive(Debug, Deserialize)]
struct StablecoinChainResp {
    #[serde(
        rename = "totalCirculatingUSD",
        default,
        deserialize_with = "crate::serde_util::de_f64_map_null_default"
    )]
    total_circulating_usd: HashMap<String, f64>,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct BridgeTxCountsResp {
    #[serde(default, deserialize_with = "crate::serde_util::de_f64_null_default")]
    deposits: f64,
    #[serde(default, deserialize_with = "crate::serde_util::de_f64_null_default")]
    withdrawals: f64,
}

#[derive(Debug, Deserialize)]
struct BridgeListItem {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(rename = "displayName", default)]
    display_name: String,
    #[serde(default)]
    slug: String,
    #[serde(rename = "destinationChain", default)]
    destination_chain: serde_json::Value,
    #[serde(default)]
    url: String,
    #[serde(default)]
    chains: Vec<String>,
    #[serde(rename = "lastHourlyVolume", default)]
    last_hourly_volume: Option<f64>,
    #[serde(rename = "last24hVolume", default)]
    last_24h_volume: Option<f64>,
    #[serde(rename = "lastDailyVolume", default)]
    last_daily_volume: Option<f64>,
    #[serde(rename = "volumePrevDay", default)]
    volume_prev_day: Option<f64>,
    #[serde(rename = "dayBeforeLastVolume", default)]
    day_before_last_volume: Option<f64>,
    #[serde(rename = "volumePrev2Day", default)]
    volume_prev_2day: Option<f64>,
    #[serde(rename = "weeklyVolume", default)]
    weekly_volume: Option<f64>,
    #[serde(rename = "monthlyVolume", default)]
    monthly_volume: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct BridgeListEnvelope {
    #[serde(default)]
    bridges: Vec<BridgeListItem>,
}

#[derive(Debug, Default, Deserialize)]
struct BridgeChainMetrics {
    #[serde(rename = "lastHourlyVolume", default)]
    last_hourly_volume: Option<f64>,
    #[serde(rename = "last24hVolume", default)]
    last_24h_volume: Option<f64>,
    #[serde(rename = "lastDailyVolume", default)]
    last_daily_volume: Option<f64>,
    #[serde(rename = "volumePrevDay", default)]
    volume_prev_day: Option<f64>,
    #[serde(rename = "dayBeforeLastVolume", default)]
    day_before_last_volume: Option<f64>,
    #[serde(rename = "volumePrev2Day", default)]
    volume_prev_2day: Option<f64>,
    #[serde(rename = "weeklyVolume", default)]
    weekly_volume: Option<f64>,
    #[serde(rename = "monthlyVolume", default)]
    monthly_volume: Option<f64>,
    #[serde(rename = "lastHourlyTxs", default)]
    last_hourly_txs: BridgeTxCountsResp,
    #[serde(rename = "currentDayTxs", default)]
    current_day_txs: BridgeTxCountsResp,
    #[serde(rename = "prevDayTxs", default)]
    prev_day_txs: BridgeTxCountsResp,
    #[serde(rename = "dayBeforeLastTxs", default)]
    day_before_last_txs: BridgeTxCountsResp,
    #[serde(rename = "weeklyTxs", default)]
    weekly_txs: BridgeTxCountsResp,
    #[serde(rename = "monthlyTxs", default)]
    monthly_txs: BridgeTxCountsResp,
}

#[derive(Debug, Deserialize)]
struct BridgeDetailResponse {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(rename = "displayName", default)]
    display_name: String,
    #[serde(rename = "destinationChain", default)]
    destination_chain: serde_json::Value,
    #[serde(rename = "lastHourlyVolume", default)]
    last_hourly_volume: Option<f64>,
    #[serde(rename = "last24hVolume", default)]
    last_24h_volume: Option<f64>,
    #[serde(rename = "lastDailyVolume", default)]
    last_daily_volume: Option<f64>,
    #[serde(rename = "volumePrevDay", default)]
    volume_prev_day: Option<f64>,
    #[serde(rename = "dayBeforeLastVolume", default)]
    day_before_last_volume: Option<f64>,
    #[serde(rename = "volumePrev2Day", default)]
    volume_prev_2day: Option<f64>,
    #[serde(rename = "weeklyVolume", default)]
    weekly_volume: Option<f64>,
    #[serde(rename = "monthlyVolume", default)]
    monthly_volume: Option<f64>,
    #[serde(rename = "lastHourlyTxs", default)]
    last_hourly_txs: BridgeTxCountsResp,
    #[serde(rename = "currentDayTxs", default)]
    current_day_txs: BridgeTxCountsResp,
    #[serde(rename = "prevDayTxs", default)]
    prev_day_txs: BridgeTxCountsResp,
    #[serde(rename = "dayBeforeLastTxs", default)]
    day_before_last_txs: BridgeTxCountsResp,
    #[serde(rename = "weeklyTxs", default)]
    weekly_txs: BridgeTxCountsResp,
    #[serde(rename = "monthlyTxs", default)]
    monthly_txs: BridgeTxCountsResp,
    #[serde(rename = "chainBreakdown", default)]
    chain_breakdown: HashMap<String, BridgeChainMetrics>,
}

// ----- Provider metadata ---------------------------------------------------

impl Provider for Client {
    fn info(&self) -> model::ProviderInfo {
        model::ProviderInfo {
            name: "defillama".to_string(),
            provider_type: "market+bridge-data".to_string(),
            requires_key: false,
            capabilities: vec![
                "chains.top".to_string(),
                "chains.assets".to_string(),
                "protocols.top".to_string(),
                "protocols.categories".to_string(),
                "protocols.fees".to_string(),
                "protocols.revenue".to_string(),
                "dexes.volume".to_string(),
                "stablecoins.top".to_string(),
                "stablecoins.chains".to_string(),
                "bridge.list".to_string(),
                "bridge.details".to_string(),
            ],
            key_env_var_name: "DEFI_DEFILLAMA_API_KEY".to_string(),
            capability_auth: vec![
                model::ProviderCapabilityAuth {
                    capability: "chains.assets".to_string(),
                    key_env_var: "DEFI_DEFILLAMA_API_KEY".to_string(),
                    description: "Required for chain-level TVL by asset endpoint".to_string(),
                },
                model::ProviderCapabilityAuth {
                    capability: "bridge.details".to_string(),
                    key_env_var: "DEFI_DEFILLAMA_API_KEY".to_string(),
                    description: "Required for bridge analytics details endpoint".to_string(),
                },
                model::ProviderCapabilityAuth {
                    capability: "bridge.list".to_string(),
                    key_env_var: "DEFI_DEFILLAMA_API_KEY".to_string(),
                    description: "Required for bridge analytics list endpoint".to_string(),
                },
            ],
        }
    }
}

// ----- MarketDataProvider ---------------------------------------------------

#[async_trait]
impl MarketDataProvider for Client {
    async fn chains_top(&self, limit: i64) -> Result<Vec<model::ChainTvl>, Error> {
        self.chains_top(limit).await
    }

    async fn chains_assets(
        &self,
        chain: Chain,
        asset: Asset,
        limit: i64,
    ) -> Result<Vec<model::ChainAssetTvl>, Error> {
        self.chains_assets(chain, asset, limit).await
    }

    async fn protocols_top(
        &self,
        category: &str,
        chain: &str,
        limit: i64,
    ) -> Result<Vec<model::ProtocolTvl>, Error> {
        self.protocols_top(category, chain, limit).await
    }

    async fn protocols_categories(&self) -> Result<Vec<model::ProtocolCategory>, Error> {
        self.protocols_categories().await
    }

    async fn stablecoins_top(
        &self,
        peg_type: &str,
        limit: i64,
    ) -> Result<Vec<model::Stablecoin>, Error> {
        self.stablecoins_top(peg_type, limit).await
    }

    async fn stablecoin_chains(&self, limit: i64) -> Result<Vec<model::StablecoinChain>, Error> {
        self.stablecoin_chains(limit).await
    }

    async fn protocols_fees(
        &self,
        category: &str,
        chain: &str,
        limit: i64,
    ) -> Result<Vec<model::ProtocolFees>, Error> {
        self.protocols_fees(category, chain, limit).await
    }

    async fn protocols_revenue(
        &self,
        category: &str,
        chain: &str,
        limit: i64,
    ) -> Result<Vec<model::ProtocolRevenue>, Error> {
        self.protocols_revenue(category, chain, limit).await
    }

    async fn dexes_volume(&self, chain: &str, limit: i64) -> Result<Vec<model::DexVolume>, Error> {
        self.dexes_volume(chain, limit).await
    }
}

// ----- BridgeDataProvider ---------------------------------------------------

#[async_trait]
impl BridgeDataProvider for Client {
    async fn list_bridges(
        &self,
        req: BridgeListRequest,
    ) -> Result<Vec<model::BridgeSummary>, Error> {
        self.list_bridges(req).await
    }

    async fn bridge_details(
        &self,
        req: BridgeDetailsRequest,
    ) -> Result<model::BridgeDetails, Error> {
        self.bridge_details(req).await
    }
}

// ----- inherent implementations (the trait methods delegate to these) -------

impl Client {
    /// `GET /v2/chains`: sort descending by TVL, sequential ranks from 1,
    /// resolvable chain names get a CAIP-2 `chain_id`.
    async fn chains_top(&self, limit: i64) -> Result<Vec<model::ChainTvl>, Error> {
        let url = format!("{}/v2/chains", self.api_base);
        let req = self.build_get(&url, "build chains request")?;
        let mut resp = self.http.do_json::<Vec<ChainResp>>(req).await?.value;

        resp.sort_by(|a, b| b.tvl.total_cmp(&a.tvl));
        let n = effective_limit(limit, resp.len());
        let mut out = Vec::with_capacity(n);
        for (i, item) in resp.into_iter().take(n).enumerate() {
            let chain_id = parse_chain(&item.name).map(|c| c.caip2).unwrap_or_default();
            out.push(model::ChainTvl {
                rank: (i + 1) as i64,
                chain: item.name,
                chain_id,
                tvl_usd: item.tvl,
            });
        }
        Ok(out)
    }

    /// `GET /<key>/api/chainAssets`: aggregate per-symbol across categories,
    /// drop non-positive totals, optional symbol filter, sort by TVL desc then
    /// symbol asc, limit, sequential ranks. Requires the API key.
    async fn chains_assets(
        &self,
        chain: Chain,
        asset: Asset,
        limit: i64,
    ) -> Result<Vec<model::ChainAssetTvl>, Error> {
        self.require_chain_assets_api_key()?;

        let url = self.chain_assets_url();
        let req = self.build_get(&url, "build chain assets request")?;
        let raw = self
            .http
            .do_json::<HashMap<String, serde_json::Value>>(req)
            .await?
            .value;

        let (assets_by_symbol, chain_name) = select_chain_asset_breakdown(&raw, &chain)?;

        let filter_symbol = asset.symbol.trim().to_uppercase();
        let mut out: Vec<model::ChainAssetTvl> = Vec::with_capacity(assets_by_symbol.len());
        for (symbol, tvl) in &assets_by_symbol {
            if !filter_symbol.is_empty() && symbol != &filter_symbol {
                continue;
            }
            if *tvl <= 0.0 {
                continue;
            }
            out.push(model::ChainAssetTvl {
                rank: 0,
                chain: chain_name.clone(),
                chain_id: chain.caip2.clone(),
                asset: symbol.clone(),
                asset_id: known_asset_id(&chain, symbol),
                tvl_usd: *tvl,
            });
        }

        if out.is_empty() {
            if !filter_symbol.is_empty() {
                return Err(Error::new(
                    Code::Unavailable,
                    "no chain asset tvl found for requested chain/asset",
                ));
            }
            return Err(Error::new(
                Code::Unavailable,
                "no chain asset tvl found for requested chain",
            ));
        }

        out.sort_by(|a, b| {
            if a.tvl_usd != b.tvl_usd {
                b.tvl_usd.total_cmp(&a.tvl_usd)
            } else {
                a.asset.cmp(&b.asset)
            }
        });
        if limit > 0 && out.len() > limit as usize {
            out.truncate(limit as usize);
        }
        for (i, item) in out.iter_mut().enumerate() {
            item.rank = (i + 1) as i64;
        }
        Ok(out)
    }

    /// `GET /protocols`: sort descending by TVL (chain-specific when filtered),
    /// sequential ranks, `chains` is the COUNT of the protocol's chains.
    async fn protocols_top(
        &self,
        category: &str,
        chain: &str,
        limit: i64,
    ) -> Result<Vec<model::ProtocolTvl>, Error> {
        let resp = self.fetch_protocols().await?;

        let norm_category = category.trim().to_lowercase();
        let norm_chain = chain.trim().to_lowercase();

        let mut filtered: Vec<(ProtocolResp, f64)> = Vec::with_capacity(resp.len());
        for p in resp {
            if !norm_category.is_empty() && p.category.to_lowercase() != norm_category {
                continue;
            }
            if !norm_chain.is_empty() && !contains_chain(&p.chains, &norm_chain) {
                continue;
            }
            let mut tvl = p.tvl;
            if !norm_chain.is_empty() {
                match chain_tvl(&p.chain_tvls, &norm_chain) {
                    // Protocol lists the chain but has no chainTvls entry —
                    // skip rather than falling back to global TVL.
                    None => continue,
                    Some(c_tvl) => tvl = c_tvl,
                }
            }
            filtered.push((p, tvl));
        }

        filtered.sort_by(|a, b| b.1.total_cmp(&a.1));
        let n = effective_limit(limit, filtered.len());
        let mut out = Vec::with_capacity(n);
        for (i, (item, tvl)) in filtered.into_iter().take(n).enumerate() {
            out.push(model::ProtocolTvl {
                rank: (i + 1) as i64,
                protocol: item.name,
                category: item.category,
                tvl_usd: tvl,
                chains: item.chains.len() as i64,
            });
        }
        Ok(out)
    }

    /// `GET /protocols`: aggregate by category (count + summed TVL), skip
    /// blank/whitespace categories, sort TVL desc, then protocol count desc,
    /// then case-insensitive name asc.
    async fn protocols_categories(&self) -> Result<Vec<model::ProtocolCategory>, Error> {
        let resp = self.fetch_protocols().await?;

        struct CatAgg {
            name: String,
            protocols: i64,
            tvl: f64,
        }
        // BTreeMap keyed by lowercase category for deterministic iteration
        // before the explicit sort (matches Go's keyed aggregation map).
        let mut agg: std::collections::BTreeMap<String, CatAgg> = std::collections::BTreeMap::new();
        for p in resp {
            let cat = p.category.trim().to_string();
            if cat.is_empty() {
                continue;
            }
            let key = cat.to_lowercase();
            let entry = agg.entry(key).or_insert_with(|| CatAgg {
                name: cat.clone(),
                protocols: 0,
                tvl: 0.0,
            });
            entry.protocols += 1;
            entry.tvl += p.tvl;
        }

        let mut out: Vec<model::ProtocolCategory> = agg
            .into_values()
            .map(|e| model::ProtocolCategory {
                name: e.name,
                protocols: e.protocols,
                tvl_usd: e.tvl,
            })
            .collect();
        out.sort_by(|a, b| {
            if a.tvl_usd != b.tvl_usd {
                return b.tvl_usd.total_cmp(&a.tvl_usd);
            }
            if a.protocols != b.protocols {
                return b.protocols.cmp(&a.protocols);
            }
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        });
        Ok(out)
    }

    /// `GET /stablecoins`: sum peg-keyed circulating maps, optional peg_type
    /// filter, sort by total circulating desc, rank, limit.
    async fn stablecoins_top(
        &self,
        peg_type: &str,
        limit: i64,
    ) -> Result<Vec<model::Stablecoin>, Error> {
        let url = format!(
            "{}/stablecoins?includePrices=true",
            self.stablecoins_api_url
        );
        let req = self.build_get(&url, "build stablecoins request")?;
        let resp = self.http.do_json::<StablecoinsEnvelope>(req).await?.value;

        let norm_peg = peg_type.trim().to_lowercase();
        let mut filtered: Vec<StablecoinResp> = Vec::with_capacity(resp.pegged_assets.len());
        for s in resp.pegged_assets {
            if !norm_peg.is_empty() && s.peg_type.to_lowercase() != norm_peg {
                continue;
            }
            filtered.push(s);
        }

        filtered.sort_by(|a, b| map_total(&b.circulating).total_cmp(&map_total(&a.circulating)));
        let n = effective_limit(limit, filtered.len());
        let mut out = Vec::with_capacity(n);
        for (i, item) in filtered.into_iter().take(n).enumerate() {
            let circulating = map_total(&item.circulating);
            let price = item.price.unwrap_or(0.0);
            out.push(model::Stablecoin {
                rank: (i + 1) as i64,
                name: item.name,
                symbol: item.symbol,
                peg_type: item.peg_type,
                peg_mechanism: item.peg_mechanism,
                circulating_usd: circulating,
                price,
                chains: item.chains.len() as i64,
                day_change_usd: circulating - map_total(&item.circulating_prev_day),
                week_change_usd: circulating - map_total(&item.circulating_prev_week),
                month_change_usd: circulating - map_total(&item.circulating_prev_month),
            });
        }
        Ok(out)
    }

    /// `GET /stablecoinchains`: aggregate `totalCirculatingUSD` per chain, pick
    /// the dominant peg (largest value), skip chains with total <= 0, sort desc,
    /// rank, limit (limit 0 = all).
    async fn stablecoin_chains(&self, limit: i64) -> Result<Vec<model::StablecoinChain>, Error> {
        let url = format!("{}/stablecoinchains", self.stablecoins_api_url);
        let req = self.build_get(&url, "build stablecoin chains request")?;
        let resp = self
            .http
            .do_json::<Vec<StablecoinChainResp>>(req)
            .await?
            .value;

        let mut out: Vec<model::StablecoinChain> = Vec::with_capacity(resp.len());
        for item in resp {
            let mut total = 0.0;
            let mut dominant_peg = String::new();
            let mut dominant_amount = 0.0;
            // Iterate sorted by peg key so ties on amount resolve deterministically.
            let mut entries: Vec<(&String, &f64)> = item.total_circulating_usd.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (peg_type, amount) in entries {
                total += *amount;
                if *amount > dominant_amount {
                    dominant_amount = *amount;
                    dominant_peg = peg_type.clone();
                }
            }
            if total <= 0.0 {
                continue;
            }
            let chain_id = parse_chain(&item.name).map(|c| c.caip2).unwrap_or_default();
            out.push(model::StablecoinChain {
                rank: 0,
                chain: item.name,
                chain_id,
                circulating_usd: total,
                dominant_peg_type: dominant_peg,
            });
        }

        out.sort_by(|a, b| b.circulating_usd.total_cmp(&a.circulating_usd));
        if limit > 0 && out.len() > limit as usize {
            out.truncate(limit as usize);
        }
        for (i, item) in out.iter_mut().enumerate() {
            item.rank = (i + 1) as i64;
        }
        Ok(out)
    }

    /// `GET /overview/fees`: positive-24h filter + optional category/chain,
    /// sort by 24h desc, rank, limit; null `total*`/`change_*` -> 0.
    async fn protocols_fees(
        &self,
        category: &str,
        chain: &str,
        limit: i64,
    ) -> Result<Vec<model::ProtocolFees>, Error> {
        let url = format!(
            "{}/overview/fees?excludeTotalDataChart=true&excludeTotalDataChartBreakdown=true",
            self.api_base
        );
        let req = self.build_get(&url, "build fees request")?;
        let resp = self.http.do_json::<FeesOverviewResp>(req).await?.value;

        let filtered = filter_fees_protocols(resp.protocols, category, chain);
        let n = effective_limit(limit, filtered.len());
        let mut out = Vec::with_capacity(n);
        for (i, item) in filtered.into_iter().take(n).enumerate() {
            out.push(model::ProtocolFees {
                rank: (i + 1) as i64,
                protocol: item.name,
                category: item.category,
                fees_24h_usd: val_or_zero(item.total24h),
                fees_7d_usd: val_or_zero(item.total7d),
                fees_30d_usd: val_or_zero(item.total30d),
                change_1d_pct: val_or_zero(item.change_1d),
                change_7d_pct: val_or_zero(item.change_7d),
                change_1m_pct: val_or_zero(item.change_1m),
                chains: item.chains.len() as i64,
            });
        }
        Ok(out)
    }

    /// Same `/overview/fees` endpoint with `dataType=dailyRevenue`, mapped onto
    /// revenue fields.
    async fn protocols_revenue(
        &self,
        category: &str,
        chain: &str,
        limit: i64,
    ) -> Result<Vec<model::ProtocolRevenue>, Error> {
        let url = format!(
            "{}/overview/fees?excludeTotalDataChart=true&excludeTotalDataChartBreakdown=true&dataType=dailyRevenue",
            self.api_base
        );
        let req = self.build_get(&url, "build revenue request")?;
        let resp = self.http.do_json::<FeesOverviewResp>(req).await?.value;

        let filtered = filter_fees_protocols(resp.protocols, category, chain);
        let n = effective_limit(limit, filtered.len());
        let mut out = Vec::with_capacity(n);
        for (i, item) in filtered.into_iter().take(n).enumerate() {
            out.push(model::ProtocolRevenue {
                rank: (i + 1) as i64,
                protocol: item.name,
                category: item.category,
                revenue_24h_usd: val_or_zero(item.total24h),
                revenue_7d_usd: val_or_zero(item.total7d),
                revenue_30d_usd: val_or_zero(item.total30d),
                change_1d_pct: val_or_zero(item.change_1d),
                change_7d_pct: val_or_zero(item.change_7d),
                change_1m_pct: val_or_zero(item.change_1m),
                chains: item.chains.len() as i64,
            });
        }
        Ok(out)
    }

    /// `GET /overview/dexs`: positive-24h filter (no category) onto volume fields.
    async fn dexes_volume(&self, chain: &str, limit: i64) -> Result<Vec<model::DexVolume>, Error> {
        let url = format!(
            "{}/overview/dexs?excludeTotalDataChart=true&excludeTotalDataChartBreakdown=true",
            self.api_base
        );
        let req = self.build_get(&url, "build dex volume request")?;
        let resp = self.http.do_json::<FeesOverviewResp>(req).await?.value;

        let filtered = filter_fees_protocols(resp.protocols, "", chain);
        let n = effective_limit(limit, filtered.len());
        let mut out = Vec::with_capacity(n);
        for (i, item) in filtered.into_iter().take(n).enumerate() {
            out.push(model::DexVolume {
                rank: (i + 1) as i64,
                protocol: item.name,
                volume_24h_usd: val_or_zero(item.total24h),
                volume_7d_usd: val_or_zero(item.total7d),
                volume_30d_usd: val_or_zero(item.total30d),
                change_1d_pct: val_or_zero(item.change_1d),
                change_7d_pct: val_or_zero(item.change_7d),
                change_1m_pct: val_or_zero(item.change_1m),
                chains: item.chains.len() as i64,
            });
        }
        Ok(out)
    }

    /// Bridge analytics list (requires the API key): sort by 24h volume desc,
    /// then weekly desc, then name asc; dedup + sort chains; stamp the clock.
    async fn list_bridges(
        &self,
        req: BridgeListRequest,
    ) -> Result<Vec<model::BridgeSummary>, Error> {
        let items = self.fetch_bridge_list(req.include_chains).await?;
        if items.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "defillama bridges returned no data",
            ));
        }

        let unix = self.now_unix();
        let fetched_at = format_rfc3339(unix);
        let mut out: Vec<model::BridgeSummary> = Vec::with_capacity(items.len());
        for item in items {
            out.push(model::BridgeSummary {
                bridge_id: item.id,
                name: item.name,
                display_name: item.display_name,
                slug: item.slug,
                destination_chain: normalize_destination_chain(&item.destination_chain),
                url: item.url.trim().to_string(),
                chains: normalize_string_slice(&item.chains),
                volumes: bridge_volumes_from_parts(
                    item.last_hourly_volume,
                    item.last_24h_volume,
                    item.last_daily_volume,
                    item.volume_prev_day,
                    item.day_before_last_volume,
                    item.volume_prev_2day,
                    item.weekly_volume,
                    item.monthly_volume,
                ),
                last_updated_unix: unix,
                fetched_at: fetched_at.clone(),
            });
        }

        out.sort_by(|a, b| {
            if a.volumes.last_24h_usd != b.volumes.last_24h_usd {
                return b.volumes.last_24h_usd.total_cmp(&a.volumes.last_24h_usd);
            }
            if a.volumes.weekly_usd != b.volumes.weekly_usd {
                return b.volumes.weekly_usd.total_cmp(&a.volumes.weekly_usd);
            }
            a.name.cmp(&b.name)
        });

        if req.limit > 0 && out.len() > req.limit as usize {
            out.truncate(req.limit as usize);
        }
        Ok(out)
    }

    /// Bridge analytics details: resolve a bridge reference to its id, fetch
    /// details, and (when requested) attach a chain breakdown sorted by 24h
    /// volume desc then chain name asc.
    async fn bridge_details(
        &self,
        req: BridgeDetailsRequest,
    ) -> Result<model::BridgeDetails, Error> {
        let bridge_ref = req.bridge.trim().to_string();
        if bridge_ref.is_empty() {
            return Err(Error::new(Code::Usage, "bridge identifier is required"));
        }
        let bridge_id = self.resolve_bridge_id(&bridge_ref).await?;
        self.require_bridge_api_key()?;

        let url = self.bridge_url(&format!("/bridge/{bridge_id}"), &[]);
        let h_req = self.build_get(&url, "build bridge details request")?;
        let resp = self
            .http
            .do_json::<BridgeDetailResponse>(h_req)
            .await?
            .value;

        let unix = self.now_unix();
        let mut details = model::BridgeDetails {
            bridge_id: resp.id,
            name: resp.name,
            display_name: resp.display_name,
            destination_chain: normalize_destination_chain(&resp.destination_chain),
            volumes: bridge_volumes_from_parts(
                resp.last_hourly_volume,
                resp.last_24h_volume,
                resp.last_daily_volume,
                resp.volume_prev_day,
                resp.day_before_last_volume,
                resp.volume_prev_2day,
                resp.weekly_volume,
                resp.monthly_volume,
            ),
            transactions: model::BridgeTransactions {
                last_hourly: tx_counts_from(&resp.last_hourly_txs),
                current_day: tx_counts_from(&resp.current_day_txs),
                prev_day: tx_counts_from(&resp.prev_day_txs),
                prev_2d: tx_counts_from(&resp.day_before_last_txs),
                weekly: tx_counts_from(&resp.weekly_txs),
                monthly: tx_counts_from(&resp.monthly_txs),
            },
            chain_breakdown: Vec::new(),
            last_updated_unix: unix,
            fetched_at: format_rfc3339(unix),
        };

        if !req.include_chain_breakdown {
            return Ok(details);
        }

        let mut breakdown: Vec<model::BridgeChainDetails> =
            Vec::with_capacity(resp.chain_breakdown.len());
        for (chain_name, chain) in &resp.chain_breakdown {
            let chain_id = parse_chain(chain_name).map(|c| c.caip2).unwrap_or_default();
            breakdown.push(model::BridgeChainDetails {
                chain: chain_name.clone(),
                chain_id,
                volumes: bridge_volumes_from_parts(
                    chain.last_hourly_volume,
                    chain.last_24h_volume,
                    chain.last_daily_volume,
                    chain.volume_prev_day,
                    chain.day_before_last_volume,
                    chain.volume_prev_2day,
                    chain.weekly_volume,
                    chain.monthly_volume,
                ),
                transactions: model::BridgeTransactions {
                    last_hourly: tx_counts_from(&chain.last_hourly_txs),
                    current_day: tx_counts_from(&chain.current_day_txs),
                    prev_day: tx_counts_from(&chain.prev_day_txs),
                    prev_2d: tx_counts_from(&chain.day_before_last_txs),
                    weekly: tx_counts_from(&chain.weekly_txs),
                    monthly: tx_counts_from(&chain.monthly_txs),
                },
            });
        }
        breakdown.sort_by(|a, b| {
            if a.volumes.last_24h_usd != b.volumes.last_24h_usd {
                return b.volumes.last_24h_usd.total_cmp(&a.volumes.last_24h_usd);
            }
            a.chain.cmp(&b.chain)
        });
        details.chain_breakdown = breakdown;
        Ok(details)
    }

    /// Fetch the raw `/protocols` array (shared by `protocols_top` /
    /// `protocols_categories`).
    async fn fetch_protocols(&self) -> Result<Vec<ProtocolResp>, Error> {
        let url = format!("{}/protocols", self.api_base);
        let req = self.build_get(&url, "build protocols request")?;
        Ok(self.http.do_json::<Vec<ProtocolResp>>(req).await?.value)
    }

    /// Fetch the bridge list (requires the API key).
    async fn fetch_bridge_list(&self, include_chains: bool) -> Result<Vec<BridgeListItem>, Error> {
        self.require_bridge_api_key()?;
        let query: Vec<(&str, &str)> = if include_chains {
            vec![("includeChains", "true")]
        } else {
            vec![]
        };
        let url = self.bridge_url("/bridges", &query);
        let h_req = self.build_get(&url, "build bridges request")?;
        let resp = self.http.do_json::<BridgeListEnvelope>(h_req).await?.value;
        Ok(resp.bridges)
    }

    /// Resolve a bridge reference (numeric id, or name/displayName/slug) to its
    /// numeric id.
    async fn resolve_bridge_id(&self, reference: &str) -> Result<i64, Error> {
        let trimmed = reference.trim();
        if let Ok(id_num) = trimmed.parse::<i64>() {
            if id_num <= 0 {
                return Err(Error::new(Code::Usage, "bridge id must be > 0"));
            }
            return Ok(id_num);
        }

        let items = self.fetch_bridge_list(false).await?;
        let norm_ref = trimmed.to_lowercase();

        let exact: Vec<&BridgeListItem> = items
            .iter()
            .filter(|item| bridge_matches_exact(item, &norm_ref))
            .collect();
        if exact.len() == 1 {
            return Ok(exact[0].id);
        }
        if exact.len() > 1 {
            return Err(Error::new(
                Code::Usage,
                "bridge reference is ambiguous; use bridge id",
            ));
        }

        let partial: Vec<&BridgeListItem> = items
            .iter()
            .filter(|item| bridge_matches_partial(item, &norm_ref))
            .collect();
        if partial.len() == 1 {
            return Ok(partial[0].id);
        }
        if partial.len() > 1 {
            return Err(Error::new(
                Code::Usage,
                "bridge reference matched multiple bridges; use bridge id",
            ));
        }
        Err(Error::new(
            Code::Usage,
            format!("bridge not found: {reference}"),
        ))
    }
}

// ----- free helpers ---------------------------------------------------------

/// The effective slice length: `limit` clamped into `0..=total`; `<= 0` or
/// over-large limits mean "all".
fn effective_limit(limit: i64, total: usize) -> usize {
    if limit <= 0 || limit as usize > total {
        total
    } else {
        limit as usize
    }
}

/// Sum all peg-keyed values in a circulating map (Go `peggedAmount.total`).
fn map_total(m: &HashMap<String, f64>) -> f64 {
    m.values().sum()
}

/// `None` -> 0 (Go `valOrZero`).
fn val_or_zero(v: Option<f64>) -> f64 {
    v.unwrap_or(0.0)
}

/// First non-`None` value in order, else 0 (Go `firstNonNilFloat`).
fn first_non_nil_float(values: &[Option<f64>]) -> f64 {
    values.iter().flatten().next().copied().unwrap_or(0.0)
}

/// Whether `chains` contains `target` (already lowercased), case-insensitively
/// and trim-tolerantly (Go `containsChain`).
fn contains_chain(chains: &[String], target: &str) -> bool {
    chains.iter().any(|c| c.trim().to_lowercase() == target)
}

/// The TVL for a specific chain from the `chainTvls` map. Suffixed keys (e.g.
/// `Ethereum-staking`) are ignored. `None` means "chain not in map"; `Some(0.0)`
/// means an explicit zero TVL (Go `chainTVL`).
fn chain_tvl(chain_tvls: &HashMap<String, f64>, norm_chain: &str) -> Option<f64> {
    for (k, v) in chain_tvls {
        if k.contains('-') {
            continue;
        }
        if k.trim().to_lowercase() == norm_chain {
            return Some(*v);
        }
    }
    None
}

/// Filter protocols by positive 24h value, optional category, optional chain
/// presence, then sort descending by 24h total (Go `filterFeesProtocols`).
fn filter_fees_protocols(
    protocols: Vec<FeesProtocolResp>,
    category: &str,
    chain: &str,
) -> Vec<FeesProtocolResp> {
    let norm_category = category.trim().to_lowercase();
    let norm_chain = chain.trim().to_lowercase();
    let mut filtered: Vec<FeesProtocolResp> = Vec::with_capacity(protocols.len());
    for p in protocols {
        match p.total24h {
            Some(t) if t > 0.0 => {}
            _ => continue,
        }
        if !norm_category.is_empty() && p.category.to_lowercase() != norm_category {
            continue;
        }
        if !norm_chain.is_empty() && !contains_chain(&p.chains, &norm_chain) {
            continue;
        }
        filtered.push(p);
    }
    filtered.sort_by(|a, b| val_or_zero(b.total24h).total_cmp(&val_or_zero(a.total24h)));
    filtered
}

/// Build bridge volumes from the raw nullable parts (Go `bridgeVolumesFromParts`).
#[allow(clippy::too_many_arguments)]
fn bridge_volumes_from_parts(
    last_hourly: Option<f64>,
    last_24h: Option<f64>,
    last_daily: Option<f64>,
    prev_day: Option<f64>,
    day_before_last: Option<f64>,
    prev_2day: Option<f64>,
    weekly: Option<f64>,
    monthly: Option<f64>,
) -> model::BridgeVolumes {
    model::BridgeVolumes {
        last_hourly_usd: val_or_zero(last_hourly),
        last_24h_usd: first_non_nil_float(&[last_24h, last_daily, prev_day]),
        last_daily_usd: first_non_nil_float(&[last_daily, prev_day]),
        prev_day_usd: first_non_nil_float(&[prev_day, last_daily]),
        prev_2d_usd: first_non_nil_float(&[prev_2day, day_before_last]),
        weekly_usd: val_or_zero(weekly),
        monthly_usd: val_or_zero(monthly),
    }
}

/// Convert wire tx counts (floats) to the model's integer counts (Go `txCountsFrom`).
fn tx_counts_from(v: &BridgeTxCountsResp) -> model::BridgeTxCounts {
    model::BridgeTxCounts {
        deposits: v.deposits as i64,
        withdrawals: v.withdrawals as i64,
    }
}

/// Trim, drop blanks, dedup case-insensitively (keeping first cased form), sort
/// ascending; empty input -> empty (Go `normalizeStringSlice`).
fn normalize_string_slice(items: &[String]) -> Vec<String> {
    if items.is_empty() {
        return Vec::new();
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let clean = item.trim();
        if clean.is_empty() {
            continue;
        }
        let key = clean.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);
        out.push(clean.to_string());
    }
    out.sort();
    out
}

/// Normalize the `destinationChain` field, which may be a string or a bool
/// (Go `normalizeDestinationChain`).
fn normalize_destination_chain(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => {
            let clean = s.trim();
            if clean.eq_ignore_ascii_case("false") {
                String::new()
            } else {
                clean.to_string()
            }
        }
        serde_json::Value::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

/// Whether `item` matches `reference` exactly (case-insensitive) on name,
/// displayName, or slug (Go `bridgeMatchesExact`).
fn bridge_matches_exact(item: &BridgeListItem, reference: &str) -> bool {
    item.name.eq_ignore_ascii_case(reference)
        || item.display_name.eq_ignore_ascii_case(reference)
        || item.slug.eq_ignore_ascii_case(reference)
}

/// Whether `item` partially matches `reference` (substring, lowercase) on name,
/// displayName, or slug (Go `bridgeMatchesPartial`).
fn bridge_matches_partial(item: &BridgeListItem, reference: &str) -> bool {
    item.name.to_lowercase().contains(reference)
        || item.display_name.to_lowercase().contains(reference)
        || item.slug.to_lowercase().contains(reference)
}

/// Whether `input` refers to `chain` by name or slug, with space->hyphen
/// normalization (Go `matchesChain`).
fn matches_chain(input: &str, chain: &Chain) -> bool {
    let mut norm_input = input.trim().to_lowercase();
    if norm_input.is_empty() {
        return false;
    }
    if norm_input.eq_ignore_ascii_case(&chain.name) {
        return true;
    }
    if norm_input.eq_ignore_ascii_case(&chain.slug) {
        return true;
    }
    if norm_input.contains(' ') {
        norm_input = norm_input.replace(' ', "-");
    }
    norm_input == chain.slug
}

/// Select the best-matching chain's aggregated per-symbol breakdown from the raw
/// chainAssets payload (Go `selectChainAssetBreakdown`). Returns the per-symbol
/// totals and the matched chain key. Skips the `timestamp` key.
fn select_chain_asset_breakdown(
    raw: &HashMap<String, serde_json::Value>,
    chain: &Chain,
) -> Result<(HashMap<String, f64>, String), Error> {
    struct Candidate {
        name: String,
        rank: i32,
        assets: HashMap<String, f64>,
    }
    let mut matches: Vec<Candidate> = Vec::with_capacity(2);
    for (name, body) in raw {
        if name.trim().eq_ignore_ascii_case("timestamp") {
            continue;
        }
        if !matches_chain(name, chain) {
            continue;
        }
        let assets = parse_chain_asset_breakdown(body)?;
        if assets.is_empty() {
            continue;
        }
        let rank = if name.trim().eq_ignore_ascii_case(&chain.name) {
            1
        } else if name.trim().eq_ignore_ascii_case(&chain.slug) {
            2
        } else {
            3
        };
        matches.push(Candidate {
            name: name.clone(),
            rank,
            assets,
        });
    }

    if matches.is_empty() {
        return Err(Error::new(
            Code::Unsupported,
            "defillama has no chain asset data for requested chain",
        ));
    }
    matches.sort_by(|a, b| {
        if a.rank != b.rank {
            a.rank.cmp(&b.rank)
        } else {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        }
    });
    let best = matches.into_iter().next().unwrap_or_else(|| Candidate {
        name: String::new(),
        rank: 0,
        assets: HashMap::new(),
    });
    Ok((best.assets, best.name))
}

/// Parse one chain's `{category: {breakdown: {symbol: value}}}` block into
/// per-UPPERCASE-symbol totals, dropping non-positive amounts (Go
/// `parseChainAssetBreakdown`).
fn parse_chain_asset_breakdown(raw: &serde_json::Value) -> Result<HashMap<String, f64>, Error> {
    let categories: HashMap<String, ChainAssetsCategory> = serde_json::from_value(raw.clone())
        .map_err(|e| Error::wrap(Code::Internal, "parse defillama chain asset payload", e))?;

    let mut out: HashMap<String, f64> = HashMap::new();
    for category in categories.values() {
        for (symbol, value) in &category.breakdown {
            let norm_symbol = symbol.trim().to_uppercase();
            if norm_symbol.is_empty() {
                continue;
            }
            match parse_loose_float(value) {
                Some(amount) if amount > 0.0 => {
                    *out.entry(norm_symbol).or_insert(0.0) += amount;
                }
                _ => continue,
            }
        }
    }
    Ok(out)
}

/// Parse a loosely-typed JSON value (number or numeric string) into a finite
/// float (Go `parseLooseFloat`). Non-numeric / non-finite values -> `None`.
fn parse_loose_float(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => {
            let f = n.as_f64()?;
            if f.is_finite() {
                Some(f)
            } else {
                None
            }
        }
        serde_json::Value::String(s) => {
            let value = s.trim();
            if value.is_empty() {
                return None;
            }
            match value.parse::<f64>() {
                Ok(f) if f.is_finite() => Some(f),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The canonical asset id for a known symbol on a chain, or empty when the
/// symbol is not in the registry (Go `knownAssetID`).
fn known_asset_id(chain: &Chain, symbol: &str) -> String {
    match known_token(&chain.caip2, symbol) {
        Some(token) => format!("{}/erc20:{}", chain.caip2, token.address.to_lowercase()),
        None => String::new(),
    }
}

/// Format a UNIX-second timestamp as RFC3339 UTC (Go `time.RFC3339`).
fn format_rfc3339(unix: i64) -> String {
    Utc.timestamp_opt(unix, 0)
        .single()
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::doc_lazy_continuation)]
mod tests {
    //! # Success criteria for the `defillama` provider adapter
    //!
    //! Go source: `internal/providers/defillama/client.go`, ported behavioral
    //! cases from `internal/providers/defillama/client_test.go`. External HTTP is
    //! mocked with `wiremock` (the Rust analogue of Go's `httptest.Server`).
    //!
    //! DefiLlama is the market + bridge-data adapter. It implements the
    //! `MarketDataProvider` (chains/protocols/stablecoins/fees/revenue/dexes) and
    //! `BridgeDataProvider` (bridge list/details) trait surfaces, plus `Provider`
    //! metadata. The adapter is "correct" iff it preserves the contract behaviors
    //! below. All outputs are deterministic (stable sort + sequential ranks), and
    //! numeric fields carry raw USD/percentage values (APY/percent are points,
    //! not ratios — spec §2.5).
    //!
    //! The `Client` exposes test seams for the three base URLs DefiLlama uses
    //! (matching the Go package-private fields the Go tests poke):
    //!   * `api_base`            — `https://api.llama.fi`        (free endpoints)
    //!   * `bridge_base_url`     — `https://pro-api.llama.fi`    (key-gated bridge + chainAssets)
    //!   * `stablecoins_api_url` — `https://stablecoins.llama.fi`
    //! Tests build a `Client` pointed at a `wiremock::MockServer` for the relevant
    //! base. The constructor mirrors Go `New(httpClient, apiKey)`.
    //!
    //! ## Criteria
    //!
    //!  D0. **Provider metadata** (`Provider::info`). `name == "defillama"`,
    //!      `provider_type == "market+bridge-data"`, `requires_key == false`,
    //!      `key_env_var_name == "DEFI_DEFILLAMA_API_KEY"`, capabilities include
    //!      `bridge.list`/`bridge.details`/`chains.assets`, and `capability_auth`
    //!      carries the three key-gated capability descriptions. `providers list`
    //!      must stay callable as metadata WITHOUT an API key (spec §2.5).
    //!
    //!  D1. **ChainsTop sorts descending by TVL** and assigns sequential ranks
    //!      starting at 1; resolvable chain names get a CAIP-2 `chain_id`
    //!      (`GET /v2/chains`). (Go `TestChainsTopSortsDescending`.)
    //!
    //!  D2. **ChainsAssets requires the API key** — with no key it returns a typed
    //!      error whose exit code is `Auth` (10), and it does NOT hit the network.
    //!      (Go `TestChainsAssetsRequiresAPIKey`.)
    //!
    //!  D3. **ChainsAssets aggregates per-symbol across categories, sorts, ranks,
    //!      and limits.** Breakdown values across `canonical|native|thirdParty`
    //!      are summed per UPPERCASE symbol; non-positive totals dropped; sorted
    //!      by TVL desc then symbol asc; limited; sequential ranks; chain name +
    //!      CAIP-2 normalized; known symbols carry an `asset_id` of the form
    //!      `<caip2>/erc20:<lowercase-address>`. The request path embeds the API
    //!      key (`/<key>/api/chainAssets`). (Go `TestChainsAssetsSortsAggregatesAndLimits`.)
    //!
    //!  D4. **ChainsAssets filters by requested asset symbol** and emits that
    //!      symbol's canonical `asset_id` (matching `parse_asset`). (Go
    //!      `TestChainsAssetsFiltersByAsset`.)
    //!
    //!  D5. **ProtocolsTop sorts descending by TVL**, ranks sequentially, and
    //!      reports `chains` as the COUNT of the protocol's chains
    //!      (`GET /protocols`). (Go `TestProtocolsTopSortsDescending`.)
    //!
    //!  D6. **ProtocolsTop chain filter** uses the chain-specific TVL from
    //!      `chainTvls` (plain chain key only — suffixed keys like
    //!      `Ethereum-staking` ignored), case-insensitive chain match, and ranks
    //!      by that chain TVL. (Go `TestProtocolsTopFiltersByChain`,
    //!      `...ChainFilterCaseInsensitive`.)
    //!
    //!  D7. **ProtocolsTop combined category + chain filter.** (Go
    //!      `TestProtocolsTopChainAndCategoryFilter`.)
    //!
    //!  D8. **ProtocolsTop chain filter: missing `chainTvls` entry is skipped, but
    //!      an explicit zero TVL is preserved.** A protocol that lists the chain in
    //!      `chains` but has no matching `chainTvls` key is dropped (NOT a global
    //!      TVL fallback); an explicit `0` chain TVL keeps the protocol with
    //!      `tvl_usd == 0`. (Go `...ChainMissingChainTvlsSkipped`,
    //!      `...ChainZeroTVLPreserved`.)
    //!
    //!  D9. **ProtocolsCategories aggregates by category** (count + summed TVL),
    //!      skips blank/whitespace categories, and sorts TVL desc, then protocol
    //!      count desc, then case-insensitive name asc. Empty input → empty out.
    //!      (Go `TestProtocolsCategoriesAggregation`, `...Empty`,
    //!      `...DeterministicTieBreak`.)
    //!
    //!  D10. **StablecoinsTop** sums peg-keyed `circulating*` maps, optionally
    //!       filters by `peg_type` (case-insensitive), sorts by total circulating
    //!       desc, ranks, limits; `price` defaults to 0 when null; day/week/month
    //!       change = current total − prior total; non-USD pegs are summed from
    //!       their own peg key. (Go `TestStablecoinsTopSortsAndLimits`,
    //!       `...FiltersByPegType`, `...NonUSDPegCirculating`, `...NullPrice`.)
    //!
    //!  D11. **StablecoinChains** aggregates `totalCirculatingUSD` per chain, picks
    //!       the dominant peg type (largest value), skips chains with total ≤ 0 or
    //!       empty maps, sorts desc, ranks, limits (limit 0 = all); resolvable
    //!       chain names get a CAIP-2 id. (Go `TestStablecoinChainsSortsAndLimits`,
    //!       `...SkipsZeroSupply`, `...NoLimit`.)
    //!
    //!  D12. **ProtocolsFees** (`GET /overview/fees`) keeps only protocols with a
    //!       positive `total24h`, optional category/chain filters, sorts by 24h
    //!       desc, ranks, limits; null `total*`/`change_*` → 0. (Go
    //!       `TestProtocolsFees*`.)
    //!
    //!  D13. **ProtocolsRevenue** = same overview endpoint with
    //!       `dataType=dailyRevenue` query, mapped onto revenue fields. (Go
    //!       `TestProtocolsRevenue*`.)
    //!
    //!  D14. **DexesVolume** (`GET /overview/dexs`) reuses the positive-24h filter
    //!       (no category) onto volume fields. (Go `TestDexesVolume*`.)
    //!
    //!  D15. **ListBridges requires the API key**; with a key it sorts by 24h
    //!       volume desc (then weekly desc, then name asc), limits, dedups +
    //!       sorts the `chains` slice, and stamps `last_updated_unix` /`fetched_at`
    //!       from an injectable clock. (Go `TestListBridgesRequiresAPIKey`,
    //!       `TestListBridgesSortsAndLimits`.)
    //!
    //!  D16. **BridgeDetails** resolves a bridge reference (numeric id or
    //!       name/displayName/slug) to its id, fetches details, and — when
    //!       requested — returns a chain breakdown sorted by 24h volume desc
    //!       (then chain name asc) with CAIP-2 chain ids and tx-count rollups.
    //!       (Go `TestBridgeDetailsBySlugIncludesBreakdown`.)
    //!
    //! ## Go tests intentionally SKIPPED here (owned elsewhere / not this module)
    //!   * `TestYieldSortDeterministic` — exercises `yieldutil.Sort`, owned by the
    //!     `yieldutil` module's own RED suite, not the defillama adapter.
    //!   * `New`/struct-field plumbing details (Go pokes package-private fields) —
    //!     re-expressed as the idiomatic base-URL test seams above, not as a
    //!     1:1 field-poke.

    use std::time::Duration;

    use defi_errors::Code;
    use defi_httpx::Client as HttpClient;
    use defi_id::{parse_asset, parse_chain};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::defillama::Client;
    use crate::traits::{
        BridgeDataProvider, BridgeDetailsRequest, BridgeListRequest, MarketDataProvider, Provider,
    };

    fn http() -> HttpClient {
        HttpClient::new(Duration::from_secs(2), 0)
    }

    // A fixed clock so `fetched_at` / `last_updated_unix` are deterministic.
    const FIXED_UNIX: i64 = 1_700_000_000;

    // ----- D0: provider metadata (callable without a key) ------------------

    #[test]
    fn info_is_metadata_only_no_key_required() {
        let client = Client::new(http(), "");
        let info = client.info();
        assert_eq!(info.name, "defillama");
        assert_eq!(info.provider_type, "market+bridge-data");
        assert!(!info.requires_key);
        assert_eq!(info.key_env_var_name, "DEFI_DEFILLAMA_API_KEY");
        assert!(info.capabilities.iter().any(|c| c == "bridge.list"));
        assert!(info.capabilities.iter().any(|c| c == "bridge.details"));
        assert!(info.capabilities.iter().any(|c| c == "chains.assets"));
        // Three key-gated capabilities are documented in capability_auth.
        let gated: Vec<&str> = info
            .capability_auth
            .iter()
            .map(|a| a.capability.as_str())
            .collect();
        assert!(gated.contains(&"chains.assets"));
        assert!(gated.contains(&"bridge.details"));
        assert!(gated.contains(&"bridge.list"));
        for a in &info.capability_auth {
            assert_eq!(a.key_env_var, "DEFI_DEFILLAMA_API_KEY");
        }
    }

    // ----- D1: ChainsTop sorts descending ----------------------------------

    #[tokio::test]
    async fn chains_top_sorts_descending() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/chains"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[ {"name":"B","tvl":2}, {"name":"A","tvl":3} ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client.chains_top(2).await.expect("chains_top");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].chain, "A");
        assert_eq!(items[0].rank, 1);
        assert_eq!(items[0].tvl_usd, 3.0);
        assert_eq!(items[1].chain, "B");
        assert_eq!(items[1].rank, 2);
    }

    // ----- D2: ChainsAssets requires API key -------------------------------

    #[tokio::test]
    async fn chains_assets_requires_api_key() {
        let chain = parse_chain("ethereum").expect("parse ethereum");
        let client = Client::new(http(), "");
        let err = client
            .chains_assets(chain, defi_id::Asset::default(), 20)
            .await
            .expect_err("expected api key error");
        assert_eq!(err.code, Code::Auth);
    }

    // ----- D3: ChainsAssets aggregates, sorts, ranks, limits ---------------

    #[tokio::test]
    async fn chains_assets_aggregates_sorts_and_limits() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/test-key/api/chainAssets"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "Ethereum":{
                        "canonical":{"total":"250.5","breakdown":{"USDC":"100","USDT":"150.5"}},
                        "native":{"total":"50","breakdown":{"ETH":"50"}},
                        "thirdParty":{"total":"205","breakdown":{"WBTC":"80","USDC":"125"}}
                    },
                    "Arbitrum":{"canonical":{"total":"10","breakdown":{"USDC":"10"}}},
                    "timestamp":1752843956
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let mut client = Client::new(http(), "test-key");
        client.set_bridge_base_url(&server.uri());

        let items = client
            .chains_assets(chain, defi_id::Asset::default(), 3)
            .await
            .expect("chains_assets");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].asset, "USDC");
        assert_eq!(items[0].tvl_usd, 225.0); // 100 + 125
        assert_eq!(items[1].asset, "USDT");
        assert_eq!(items[1].tvl_usd, 150.5);
        assert_eq!(items[2].asset, "WBTC");
        assert_eq!(items[2].tvl_usd, 80.0);
        assert_eq!(items[0].rank, 1);
        assert_eq!(items[1].rank, 2);
        assert_eq!(items[2].rank, 3);
        assert_eq!(items[0].chain, "Ethereum");
        assert_eq!(items[0].chain_id, "eip155:1");
        assert!(items[0].asset_id.starts_with("eip155:1/erc20:"));
    }

    // ----- D4: ChainsAssets filters by requested asset ---------------------

    #[tokio::test]
    async fn chains_assets_filters_by_asset() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/test-key/api/chainAssets"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "Ethereum":{
                        "canonical":{"total":"250.5","breakdown":{"USDC":"100","USDT":"150.5"}},
                        "native":{"total":"50","breakdown":{"ETH":"50"}},
                        "thirdParty":{"total":"205","breakdown":{"WBTC":"80","USDC":"125"}}
                    },
                    "timestamp":1752843956
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = parse_chain("ethereum").expect("parse ethereum");
        let asset = parse_asset("USDC", &chain).expect("parse USDC");
        let mut client = Client::new(http(), "test-key");
        client.set_bridge_base_url(&server.uri());

        let items = client
            .chains_assets(chain, asset.clone(), 20)
            .await
            .expect("chains_assets");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].asset, "USDC");
        assert_eq!(items[0].tvl_usd, 225.0);
        assert_eq!(items[0].asset_id, asset.asset_id);
    }

    // ----- D5: ProtocolsTop sorts descending -------------------------------

    #[tokio::test]
    async fn protocols_top_sorts_descending() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protocols"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"name":"Aave","category":"Lending","tvl":10000,"chains":["Ethereum","Polygon"],"chainTvls":{"Ethereum":7000,"Polygon":3000}},
                    {"name":"Lido","category":"Liquid Staking","tvl":30000,"chains":["Ethereum"],"chainTvls":{"Ethereum":30000}},
                    {"name":"Uniswap","category":"Dexes","tvl":20000,"chains":["Ethereum","Arbitrum","Base"],"chainTvls":{"Ethereum":12000,"Arbitrum":5000,"Base":3000}}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_top("", "", 0)
            .await
            .expect("protocols_top");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].protocol, "Lido");
        assert_eq!(items[0].rank, 1);
        assert_eq!(items[0].tvl_usd, 30000.0);
        assert_eq!(items[0].chains, 1);
        assert_eq!(items[1].protocol, "Uniswap");
        assert_eq!(items[1].chains, 3);
    }

    /// Regression: the live `/protocols` response carries `"tvl": null` for
    /// ~10% of rows (and may carry null `chainTvls` values). Go coerces these to
    /// `0.0`; the Rust port must too (was: `invalid type: null, expected f64`).
    #[tokio::test]
    async fn protocols_top_tolerates_null_tvl() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protocols"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"name":"Fantom","category":"Chain","tvl":null,"chains":[],"chainTvls":{}},
                    {"name":"Lido","category":"Liquid Staking","tvl":30000,"chains":["Ethereum"],"chainTvls":{"Ethereum":30000,"Ethereum-staking":null}},
                    {"name":"Aave","category":"Lending","tvl":10000,"chains":["Ethereum"],"chainTvls":{"Ethereum":10000}}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_top("", "", 0)
            .await
            .expect("protocols_top tolerates null tvl");
        // The null-tvl row decodes (tvl -> 0.0) and sorts last; nothing errors.
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].protocol, "Lido");
        assert_eq!(items[0].tvl_usd, 30000.0);
        let fantom = items
            .iter()
            .find(|p| p.protocol == "Fantom")
            .expect("null-tvl row present");
        assert_eq!(fantom.tvl_usd, 0.0);
    }

    // ----- D6: ProtocolsTop chain filter uses chain-specific TVL -----------

    #[tokio::test]
    async fn protocols_top_filters_by_chain() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protocols"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"name":"Aave","category":"Lending","tvl":10000,"chains":["Ethereum","Polygon"],"chainTvls":{"Ethereum":7000,"Polygon":3000,"Ethereum-staking":500}},
                    {"name":"Lido","category":"Liquid Staking","tvl":30000,"chains":["Ethereum"],"chainTvls":{"Ethereum":30000}},
                    {"name":"PancakeSwap","category":"Dexes","tvl":8000,"chains":["BSC"],"chainTvls":{"BSC":8000}},
                    {"name":"Uniswap","category":"Dexes","tvl":20000,"chains":["Ethereum","Arbitrum","Base"],"chainTvls":{"Ethereum":12000,"Arbitrum":5000,"Base":3000}}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_top("", "Ethereum", 0)
            .await
            .expect("protocols_top");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].protocol, "Lido");
        assert_eq!(items[0].tvl_usd, 30000.0);
        assert_eq!(items[1].protocol, "Uniswap");
        assert_eq!(items[1].tvl_usd, 12000.0);
        assert_eq!(items[2].protocol, "Aave");
        assert_eq!(items[2].tvl_usd, 7000.0);
    }

    #[tokio::test]
    async fn protocols_top_chain_filter_case_insensitive() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protocols"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"name":"Aave","category":"Lending","tvl":10000,"chains":["Ethereum"],"chainTvls":{"Ethereum":10000}},
                    {"name":"PancakeSwap","category":"Dexes","tvl":8000,"chains":["BSC"],"chainTvls":{"BSC":8000}}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_top("", "ethereum", 0)
            .await
            .expect("protocols_top");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].protocol, "Aave");
    }

    // ----- D7: ProtocolsTop combined category + chain filter ---------------

    #[tokio::test]
    async fn protocols_top_chain_and_category_filter() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protocols"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"name":"Aave","category":"Lending","tvl":10000,"chains":["Ethereum","Polygon"],"chainTvls":{"Ethereum":7000,"Polygon":3000}},
                    {"name":"Lido","category":"Liquid Staking","tvl":30000,"chains":["Ethereum"],"chainTvls":{"Ethereum":30000}},
                    {"name":"Morpho","category":"Lending","tvl":5000,"chains":["Ethereum","Base"],"chainTvls":{"Ethereum":4000,"Base":1000}}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_top("Lending", "Ethereum", 0)
            .await
            .expect("protocols_top");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].protocol, "Aave");
        assert_eq!(items[0].tvl_usd, 7000.0);
        assert_eq!(items[1].protocol, "Morpho");
        assert_eq!(items[1].tvl_usd, 4000.0);
    }

    // ----- D8: ProtocolsTop missing chainTvls skipped / zero preserved -----

    #[tokio::test]
    async fn protocols_top_chain_missing_chain_tvls_skipped() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protocols"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"name":"OldProtocol","category":"Lending","tvl":5000,"chains":["Ethereum"],"chainTvls":{}}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_top("", "Ethereum", 0)
            .await
            .expect("protocols_top");
        assert_eq!(items.len(), 0);
    }

    #[tokio::test]
    async fn protocols_top_chain_zero_tvl_preserved() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protocols"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"name":"ZeroTVLProtocol","category":"Lending","tvl":5000,"chains":["Ethereum"],"chainTvls":{"Ethereum":0}}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_top("", "Ethereum", 0)
            .await
            .expect("protocols_top");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].tvl_usd, 0.0);
    }

    // ----- D9: ProtocolsCategories aggregation -----------------------------

    #[tokio::test]
    async fn protocols_categories_aggregation() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protocols"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"name":"Aave V3","category":"Lending","tvl":10000},
                    {"name":"Morpho","category":"Lending","tvl":5000},
                    {"name":"Uniswap","category":"Dexes","tvl":20000},
                    {"name":"Curve","category":"Dexes","tvl":8000},
                    {"name":"Lido","category":"Liquid Staking","tvl":30000},
                    {"name":"Empty","category":"","tvl":100},
                    {"name":"Spaces","category":"  ","tvl":50}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let cats = client
            .protocols_categories()
            .await
            .expect("protocols_categories");
        assert_eq!(cats.len(), 3);
        assert_eq!(cats[0].name, "Liquid Staking");
        assert_eq!(cats[0].protocols, 1);
        assert_eq!(cats[0].tvl_usd, 30000.0);
        assert_eq!(cats[1].name, "Dexes");
        assert_eq!(cats[1].protocols, 2);
        assert_eq!(cats[1].tvl_usd, 28000.0);
        assert_eq!(cats[2].name, "Lending");
        assert_eq!(cats[2].protocols, 2);
        assert_eq!(cats[2].tvl_usd, 15000.0);
    }

    #[tokio::test]
    async fn protocols_categories_empty() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protocols"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("[]", "application/json"))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let cats = client
            .protocols_categories()
            .await
            .expect("protocols_categories");
        assert_eq!(cats.len(), 0);
    }

    #[tokio::test]
    async fn protocols_categories_deterministic_tie_break() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protocols"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"name":"P1","category":"zeta","tvl":1000},
                    {"name":"P2","category":"Alpha","tvl":1000},
                    {"name":"P3","category":"alpha","tvl":1000}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let cats = client
            .protocols_categories()
            .await
            .expect("protocols_categories");
        // "Alpha"/"alpha" aggregate to one case-insensitive category (2 protocols);
        // tie on TVL → more protocols first.
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].name, "Alpha");
        assert_eq!(cats[0].protocols, 2);
        assert_eq!(cats[1].name, "zeta");
        assert_eq!(cats[1].protocols, 1);
    }

    // ----- D10: StablecoinsTop ---------------------------------------------

    #[tokio::test]
    async fn stablecoins_top_sorts_and_limits() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stablecoins"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "peggedAssets":[
                        {"name":"Tether","symbol":"USDT","pegType":"peggedUSD","pegMechanism":"fiat-backed",
                         "circulating":{"peggedUSD":120000000000},"circulatingPrevDay":{"peggedUSD":119500000000},
                         "circulatingPrevWeek":{"peggedUSD":118000000000},"circulatingPrevMonth":{"peggedUSD":115000000000},
                         "chains":["Ethereum","Tron","BSC","Arbitrum","Solana"],"price":1.0001},
                        {"name":"USD Coin","symbol":"USDC","pegType":"peggedUSD","pegMechanism":"fiat-backed",
                         "circulating":{"peggedUSD":55000000000},"circulatingPrevDay":{"peggedUSD":54800000000},
                         "circulatingPrevWeek":{"peggedUSD":54000000000},"circulatingPrevMonth":{"peggedUSD":52000000000},
                         "chains":["Ethereum","Base","Solana"],"price":0.9999},
                        {"name":"Dai","symbol":"DAI","pegType":"peggedUSD","pegMechanism":"crypto-backed",
                         "circulating":{"peggedUSD":5000000000},"circulatingPrevDay":{"peggedUSD":4990000000},
                         "circulatingPrevWeek":{"peggedUSD":4900000000},"circulatingPrevMonth":{"peggedUSD":4800000000},
                         "chains":["Ethereum","Polygon"],"price":1.0}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_stablecoins_api_url(&server.uri());

        let items = client
            .stablecoins_top("", 2)
            .await
            .expect("stablecoins_top");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].symbol, "USDT");
        assert_eq!(items[0].rank, 1);
        assert_eq!(items[0].circulating_usd, 120000000000.0);
        assert_eq!(items[0].chains, 5);
        assert_eq!(items[0].price, 1.0001);
        assert_eq!(items[0].day_change_usd, 120000000000.0 - 119500000000.0);
        assert_eq!(items[1].symbol, "USDC");
        assert_eq!(items[1].rank, 2);
    }

    #[tokio::test]
    async fn stablecoins_top_filters_by_peg_type() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stablecoins"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "peggedAssets":[
                        {"name":"Tether","symbol":"USDT","pegType":"peggedUSD","pegMechanism":"fiat-backed",
                         "circulating":{"peggedUSD":120000000000},"circulatingPrevDay":{"peggedUSD":119500000000},
                         "circulatingPrevWeek":{"peggedUSD":118000000000},"circulatingPrevMonth":{"peggedUSD":115000000000},
                         "chains":["Ethereum"],"price":1.0},
                        {"name":"STASIS EURO","symbol":"EURS","pegType":"peggedEUR","pegMechanism":"fiat-backed",
                         "circulating":{"peggedUSD":100000000},"circulatingPrevDay":{"peggedUSD":99000000},
                         "circulatingPrevWeek":{"peggedUSD":98000000},"circulatingPrevMonth":{"peggedUSD":95000000},
                         "chains":["Ethereum"],"price":1.1}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_stablecoins_api_url(&server.uri());

        let items = client
            .stablecoins_top("peggedEUR", 20)
            .await
            .expect("stablecoins_top");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].symbol, "EURS");
        assert_eq!(items[0].peg_type, "peggedEUR");
    }

    #[tokio::test]
    async fn stablecoins_top_non_usd_peg_circulating() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stablecoins"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "peggedAssets":[
                        {"name":"STASIS EURO","symbol":"EURS","pegType":"peggedEUR","pegMechanism":"fiat-backed",
                         "circulating":{"peggedEUR":100000000},"circulatingPrevDay":{"peggedEUR":99000000},
                         "circulatingPrevWeek":{"peggedEUR":98000000},"circulatingPrevMonth":{"peggedEUR":95000000},
                         "chains":["Ethereum"],"price":1.1},
                        {"name":"Tether","symbol":"USDT","pegType":"peggedUSD","pegMechanism":"fiat-backed",
                         "circulating":{"peggedUSD":50000000},"circulatingPrevDay":{"peggedUSD":49000000},
                         "circulatingPrevWeek":{"peggedUSD":48000000},"circulatingPrevMonth":{"peggedUSD":47000000},
                         "chains":["Ethereum"],"price":1.0}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_stablecoins_api_url(&server.uri());

        let items = client
            .stablecoins_top("", 0)
            .await
            .expect("stablecoins_top");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].symbol, "EURS");
        assert_eq!(items[0].circulating_usd, 100000000.0);
        assert_eq!(items[0].day_change_usd, 100000000.0 - 99000000.0);
    }

    #[tokio::test]
    async fn stablecoins_top_null_price() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stablecoins"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "peggedAssets":[
                        {"name":"NoPrice","symbol":"NP","pegType":"peggedUSD","pegMechanism":"algo",
                         "circulating":{"peggedUSD":1000},"circulatingPrevDay":{"peggedUSD":1000},
                         "circulatingPrevWeek":{"peggedUSD":1000},"circulatingPrevMonth":{"peggedUSD":1000},
                         "chains":["Ethereum"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_stablecoins_api_url(&server.uri());

        let items = client
            .stablecoins_top("", 20)
            .await
            .expect("stablecoins_top");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].price, 0.0);
    }

    // ----- D11: StablecoinChains -------------------------------------------

    #[tokio::test]
    async fn stablecoin_chains_sorts_and_limits() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stablecoinchains"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"gecko_id":"ethereum","totalCirculatingUSD":{"peggedUSD":90000000000,"peggedEUR":500000000},"tokenSymbol":"ETH","name":"Ethereum"},
                    {"gecko_id":"tron","totalCirculatingUSD":{"peggedUSD":60000000000},"tokenSymbol":"TRX","name":"Tron"},
                    {"gecko_id":"binancecoin","totalCirculatingUSD":{"peggedUSD":8000000000,"peggedEUR":200000000},"tokenSymbol":"BNB","name":"BSC"},
                    {"gecko_id":"solana","totalCirculatingUSD":{"peggedUSD":12000000000},"tokenSymbol":"SOL","name":"Solana"}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_stablecoins_api_url(&server.uri());

        let items = client
            .stablecoin_chains(3)
            .await
            .expect("stablecoin_chains");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].chain, "Ethereum");
        assert_eq!(items[0].rank, 1);
        assert_eq!(items[0].circulating_usd, 90500000000.0); // USD + EUR
        assert_eq!(items[0].dominant_peg_type, "peggedUSD");
        assert_eq!(items[1].chain, "Tron");
        assert_eq!(items[1].rank, 2);
        assert_eq!(items[2].chain, "Solana");
        assert_eq!(items[2].rank, 3);
    }

    #[tokio::test]
    async fn stablecoin_chains_skips_zero_supply() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stablecoinchains"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"gecko_id":"ethereum","totalCirculatingUSD":{"peggedUSD":90000000000},"tokenSymbol":"ETH","name":"Ethereum"},
                    {"gecko_id":"dead","totalCirculatingUSD":{"peggedUSD":0},"tokenSymbol":"DEAD","name":"DeadChain"},
                    {"gecko_id":"empty","totalCirculatingUSD":{},"tokenSymbol":null,"name":"EmptyChain"}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_stablecoins_api_url(&server.uri());

        let items = client
            .stablecoin_chains(0)
            .await
            .expect("stablecoin_chains");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].chain, "Ethereum");
    }

    #[tokio::test]
    async fn stablecoin_chains_no_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stablecoinchains"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[
                    {"gecko_id":"ethereum","totalCirculatingUSD":{"peggedUSD":90000000000},"tokenSymbol":"ETH","name":"Ethereum"},
                    {"gecko_id":"tron","totalCirculatingUSD":{"peggedUSD":60000000000},"tokenSymbol":"TRX","name":"Tron"}
                ]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_stablecoins_api_url(&server.uri());

        let items = client
            .stablecoin_chains(0)
            .await
            .expect("stablecoin_chains");
        assert_eq!(items.len(), 2);
    }

    // ----- D12: ProtocolsFees ----------------------------------------------

    #[tokio::test]
    async fn protocols_fees_sorts_and_limits() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/overview/fees"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "protocols":[
                        {"name":"Uniswap","category":"Dexs","total24h":5000000,"total7d":30000000,"total30d":120000000,"change_1d":5.2,"change_7d":-2.1,"change_1m":10.5,"chains":["Ethereum","Arbitrum","Base"]},
                        {"name":"Aave","category":"Lending","total24h":2000000,"total7d":12000000,"total30d":50000000,"change_1d":1.5,"change_7d":3.0,"change_1m":-5.0,"chains":["Ethereum","Polygon"]},
                        {"name":"Lido","category":"Liquid Staking","total24h":8000000,"total7d":55000000,"total30d":200000000,"change_1d":-1.0,"change_7d":0.5,"change_1m":15.0,"chains":["Ethereum"]},
                        {"name":"Dead","category":"Dexs","total24h":null,"chains":[]},
                        {"name":"Tiny","category":"Dexs","total24h":0,"chains":["BSC"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_fees("", "", 2)
            .await
            .expect("protocols_fees");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].protocol, "Lido");
        assert_eq!(items[0].rank, 1);
        assert_eq!(items[0].fees_24h_usd, 8000000.0);
        assert_eq!(items[0].chains, 1);
        assert_eq!(items[1].protocol, "Uniswap");
        assert_eq!(items[1].rank, 2);
    }

    #[tokio::test]
    async fn protocols_fees_filters_by_category() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/overview/fees"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "protocols":[
                        {"name":"Uniswap","category":"Dexs","total24h":5000000,"chains":["Ethereum"]},
                        {"name":"Aave","category":"Lending","total24h":2000000,"chains":["Ethereum"]},
                        {"name":"Curve","category":"Dexs","total24h":1000000,"chains":["Ethereum"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_fees("Dexs", "", 0)
            .await
            .expect("protocols_fees");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].protocol, "Uniswap");
        assert_eq!(items[1].protocol, "Curve");
    }

    #[tokio::test]
    async fn protocols_fees_filters_by_chain() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/overview/fees"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "protocols":[
                        {"name":"Uniswap","category":"Dexs","total24h":5000000,"chains":["Ethereum","Arbitrum","Base"]},
                        {"name":"PancakeSwap","category":"Dexs","total24h":8000000,"chains":["BSC"]},
                        {"name":"Aave","category":"Lending","total24h":2000000,"chains":["Ethereum","Polygon"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_fees("", "Ethereum", 0)
            .await
            .expect("protocols_fees");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].protocol, "Uniswap");
        assert_eq!(items[1].protocol, "Aave");
    }

    #[tokio::test]
    async fn protocols_fees_filters_by_category_and_chain() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/overview/fees"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "protocols":[
                        {"name":"Uniswap","category":"Dexs","total24h":5000000,"chains":["Ethereum","Arbitrum"]},
                        {"name":"Aave","category":"Lending","total24h":2000000,"chains":["Ethereum","Polygon"]},
                        {"name":"Curve","category":"Dexs","total24h":1000000,"chains":["Ethereum"]},
                        {"name":"PancakeSwap","category":"Dexs","total24h":8000000,"chains":["BSC"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_fees("Dexs", "Ethereum", 0)
            .await
            .expect("protocols_fees");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].protocol, "Uniswap");
        assert_eq!(items[1].protocol, "Curve");
    }

    #[tokio::test]
    async fn protocols_fees_skips_null_and_zero() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/overview/fees"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "protocols":[
                        {"name":"NullFees","category":"Dexs","total24h":null,"chains":[]},
                        {"name":"ZeroFees","category":"Dexs","total24h":0,"chains":["Ethereum"]},
                        {"name":"NegativeFees","category":"Dexs","total24h":-100,"chains":["Ethereum"]},
                        {"name":"ValidFees","category":"Dexs","total24h":500,"chains":["Ethereum"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_fees("", "", 0)
            .await
            .expect("protocols_fees");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].protocol, "ValidFees");
    }

    // ----- D13: ProtocolsRevenue (dataType=dailyRevenue) -------------------

    #[tokio::test]
    async fn protocols_revenue_sorts_and_limits_with_revenue_query() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/overview/fees"))
            .and(query_param("dataType", "dailyRevenue"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "protocols":[
                        {"name":"Uniswap","category":"Dexs","total24h":3000000,"total7d":18000000,"total30d":70000000,"change_1d":4.2,"change_7d":-1.1,"change_1m":8.5,"chains":["Ethereum","Arbitrum","Base"]},
                        {"name":"Aave","category":"Lending","total24h":1000000,"total7d":6000000,"total30d":25000000,"change_1d":2.5,"change_7d":4.0,"change_1m":-3.0,"chains":["Ethereum","Polygon"]},
                        {"name":"Lido","category":"Liquid Staking","total24h":5000000,"total7d":35000000,"total30d":130000000,"change_1d":-0.5,"change_7d":1.5,"change_1m":12.0,"chains":["Ethereum"]},
                        {"name":"Dead","category":"Dexs","total24h":null,"chains":[]},
                        {"name":"Tiny","category":"Dexs","total24h":0,"chains":["BSC"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_revenue("", "", 2)
            .await
            .expect("protocols_revenue");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].protocol, "Lido");
        assert_eq!(items[0].rank, 1);
        assert_eq!(items[0].revenue_24h_usd, 5000000.0);
        assert_eq!(items[0].chains, 1);
        assert_eq!(items[1].protocol, "Uniswap");
        assert_eq!(items[1].rank, 2);
    }

    #[tokio::test]
    async fn protocols_revenue_filters_by_category() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/overview/fees"))
            .and(query_param("dataType", "dailyRevenue"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "protocols":[
                        {"name":"Uniswap","category":"Dexs","total24h":3000000,"chains":["Ethereum"]},
                        {"name":"Aave","category":"Lending","total24h":1000000,"chains":["Ethereum"]},
                        {"name":"Curve","category":"Dexs","total24h":500000,"chains":["Ethereum"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_revenue("Dexs", "", 0)
            .await
            .expect("protocols_revenue");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].protocol, "Uniswap");
        assert_eq!(items[1].protocol, "Curve");
    }

    #[tokio::test]
    async fn protocols_revenue_skips_null_and_zero() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/overview/fees"))
            .and(query_param("dataType", "dailyRevenue"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "protocols":[
                        {"name":"NullRev","category":"Dexs","total24h":null,"chains":[]},
                        {"name":"ZeroRev","category":"Dexs","total24h":0,"chains":["Ethereum"]},
                        {"name":"NegRev","category":"Dexs","total24h":-100,"chains":["Ethereum"]},
                        {"name":"ValidRev","category":"Dexs","total24h":500,"chains":["Ethereum"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .protocols_revenue("", "", 0)
            .await
            .expect("protocols_revenue");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].protocol, "ValidRev");
    }

    // ----- D14: DexesVolume -------------------------------------------------

    #[tokio::test]
    async fn dexes_volume_sorts_and_limits() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/overview/dexs"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "protocols":[
                        {"name":"Uniswap","total24h":5000000,"total7d":30000000,"total30d":120000000,"change_1d":5.2,"change_7d":-2.1,"change_1m":10.5,"chains":["Ethereum","Arbitrum","Base"]},
                        {"name":"Curve","total24h":2000000,"total7d":12000000,"total30d":50000000,"change_1d":1.5,"change_7d":3.0,"change_1m":-5.0,"chains":["Ethereum","Polygon"]},
                        {"name":"PancakeSwap","total24h":8000000,"total7d":55000000,"total30d":200000000,"change_1d":-1.0,"change_7d":0.5,"change_1m":15.0,"chains":["BSC"]},
                        {"name":"Dead","total24h":null,"chains":[]},
                        {"name":"Tiny","total24h":0,"chains":["BSC"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client.dexes_volume("", 2).await.expect("dexes_volume");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].protocol, "PancakeSwap");
        assert_eq!(items[0].rank, 1);
        assert_eq!(items[0].volume_24h_usd, 8000000.0);
        assert_eq!(items[0].chains, 1);
        assert_eq!(items[1].protocol, "Uniswap");
        assert_eq!(items[1].rank, 2);
    }

    #[tokio::test]
    async fn dexes_volume_filters_by_chain() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/overview/dexs"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "protocols":[
                        {"name":"Uniswap","total24h":5000000,"chains":["Ethereum","Arbitrum","Base"]},
                        {"name":"PancakeSwap","total24h":8000000,"chains":["BSC"]},
                        {"name":"SushiSwap","total24h":1000000,"chains":["Ethereum","Polygon"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client
            .dexes_volume("Ethereum", 0)
            .await
            .expect("dexes_volume");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].protocol, "Uniswap");
        assert_eq!(items[1].protocol, "SushiSwap");
    }

    #[tokio::test]
    async fn dexes_volume_skips_null_and_zero() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/overview/dexs"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "protocols":[
                        {"name":"NullVol","total24h":null,"chains":[]},
                        {"name":"ZeroVol","total24h":0,"chains":["Ethereum"]},
                        {"name":"NegVol","total24h":-100,"chains":["Ethereum"]},
                        {"name":"ValidVol","total24h":500,"chains":["Ethereum"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "");
        client.set_api_base(&server.uri());

        let items = client.dexes_volume("", 0).await.expect("dexes_volume");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].protocol, "ValidVol");
    }

    // ----- D15: ListBridges -------------------------------------------------

    #[tokio::test]
    async fn list_bridges_requires_api_key() {
        let client = Client::new(http(), "");
        let err = client
            .list_bridges(BridgeListRequest {
                limit: 5,
                include_chains: true,
            })
            .await
            .expect_err("expected api key error");
        assert_eq!(err.code, Code::Auth);
    }

    #[tokio::test]
    async fn list_bridges_sorts_and_limits() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/test-key/bridges/bridges"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "bridges":[
                        {"id":1,"name":"b","displayName":"Bridge B","slug":"bridge-b","last24hVolume":150,"weeklyVolume":1000,"monthlyVolume":5000,"chains":["Base","Ethereum"]},
                        {"id":2,"name":"a","displayName":"Bridge A","slug":"bridge-a","last24hVolume":250,"weeklyVolume":900,"monthlyVolume":6000,"chains":["Ethereum","Base"]},
                        {"id":3,"name":"c","displayName":"Bridge C","slug":"bridge-c","last24hVolume":90,"weeklyVolume":700,"monthlyVolume":2000,"chains":["Arbitrum"]}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "test-key");
        client.set_bridge_base_url(&server.uri());
        client.set_now_unix(FIXED_UNIX);

        let got = client
            .list_bridges(BridgeListRequest {
                limit: 2,
                include_chains: true,
            })
            .await
            .expect("list_bridges");
        assert_eq!(got.len(), 2);
        // Sorted by 24h volume desc: id 2 (250) then id 1 (150).
        assert_eq!(got[0].bridge_id, 2);
        assert_eq!(got[1].bridge_id, 1);
        // chains deduped + sorted ascending.
        assert_eq!(
            got[0].chains,
            vec!["Base".to_string(), "Ethereum".to_string()]
        );
        // injected clock drives the fetched-at stamp.
        assert_eq!(got[0].last_updated_unix, FIXED_UNIX);
    }

    // ----- D16: BridgeDetails (by slug, with chain breakdown) --------------

    #[tokio::test]
    async fn bridge_details_by_slug_includes_breakdown() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/test-key/bridges/bridges"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "bridges":[
                        {"id":84,"name":"layerzero","displayName":"LayerZero","slug":"layerzero"}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/test-key/bridges/bridge/84"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "id":84,
                    "name":"layerzero",
                    "displayName":"LayerZero",
                    "last24hVolume":123.45,
                    "weeklyVolume":999.1,
                    "monthlyVolume":4200.7,
                    "lastHourlyTxs":{"deposits":1,"withdrawals":2},
                    "currentDayTxs":{"deposits":0,"withdrawals":0},
                    "prevDayTxs":{"deposits":10,"withdrawals":20},
                    "dayBeforeLastTxs":{"deposits":7,"withdrawals":8},
                    "weeklyTxs":{"deposits":100,"withdrawals":200},
                    "monthlyTxs":{"deposits":300,"withdrawals":400},
                    "chainBreakdown":{
                        "Base":{
                            "last24hVolume":80,
                            "weeklyVolume":600,
                            "monthlyVolume":2000,
                            "lastHourlyTxs":{"deposits":1,"withdrawals":1},
                            "currentDayTxs":{"deposits":0,"withdrawals":0},
                            "prevDayTxs":{"deposits":5,"withdrawals":6},
                            "dayBeforeLastTxs":{"deposits":2,"withdrawals":3},
                            "weeklyTxs":{"deposits":50,"withdrawals":60},
                            "monthlyTxs":{"deposits":100,"withdrawals":110}
                        },
                        "Arbitrum":{
                            "last24hVolume":40,
                            "weeklyVolume":300,
                            "monthlyVolume":1500,
                            "lastHourlyTxs":{"deposits":0,"withdrawals":1},
                            "currentDayTxs":{"deposits":0,"withdrawals":0},
                            "prevDayTxs":{"deposits":2,"withdrawals":1},
                            "dayBeforeLastTxs":{"deposits":2,"withdrawals":1},
                            "weeklyTxs":{"deposits":20,"withdrawals":10},
                            "monthlyTxs":{"deposits":30,"withdrawals":20}
                        }
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http(), "test-key");
        client.set_bridge_base_url(&server.uri());
        client.set_now_unix(FIXED_UNIX);

        let got = client
            .bridge_details(BridgeDetailsRequest {
                bridge: "layerzero".to_string(),
                include_chain_breakdown: true,
            })
            .await
            .expect("bridge_details");
        assert_eq!(got.bridge_id, 84);
        assert_eq!(got.name, "layerzero");
        assert_eq!(got.chain_breakdown.len(), 2);
        // Highest-volume chain first: Base (80) > Arbitrum (40).
        assert_eq!(got.chain_breakdown[0].chain, "Base");
        assert_eq!(got.chain_breakdown[0].chain_id, "eip155:8453");
        assert_eq!(got.transactions.weekly.deposits, 100);
        assert_eq!(got.transactions.weekly.withdrawals, 200);
    }
}
