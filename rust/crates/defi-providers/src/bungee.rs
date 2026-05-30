//! Bungee provider adapter (swap + bridge quotes).
//!
//! Go source: `internal/providers/bungee/client.go` (+ `client_test.go`).
//!
//! A single Bungee `/bungee/quote` endpoint (GET) backs both a [`SwapProvider`]
//! (same-chain, exact-input only) and a [`BridgeProvider`] (cross-chain) quote.
//! The mode is fixed at construction (`new_swap` / `new_bridge`), mirroring the
//! Go `NewSwap` / `NewBridge` constructors. Numeric amounts are kept as
//! base-unit + decimal strings (the machine contract).
//!
//! Bungee runs two backends: a public backend and a "dedicated" backend that
//! requires both an API key and an affiliate. When (and only when) both are
//! provided, requests go to the dedicated base URL with `x-api-key` +
//! `affiliate` headers; otherwise the public backend is used with no auth
//! headers.

use std::cmp::Ordering;

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_execution::{BridgeQuoteRequest, SwapQuoteRequest, SwapTradeType};
use defi_httpx::Client as HttpClient;
use defi_id::{format_decimal, Chain};
use defi_model as model;
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Method, Request, Url};
use serde::Deserialize;
use serde_json::Value;

use crate::traits::{BridgeProvider, Provider, SwapProvider};

/// Default public backend base URL (mirrors Go `defaultBase`).
const DEFAULT_BASE: &str = "https://public-backend.bungee.exchange/api/v1";
/// Default dedicated backend base URL (mirrors Go `defaultDedicatedBase`).
const DEFAULT_DEDICATED_BASE: &str = "https://dedicated-backend.bungee.exchange/api/v1";
/// Deterministic placeholder EVM user/receiver address used for quote-only
/// requests (mirrors Go `defaultEVMUserAddress`).
const DEFAULT_EVM_USER_ADDRESS: &str = "0x0000000000000000000000000000000000000001";
/// Public source URL surfaced on every quote (mirrors Go literal).
const SOURCE_URL: &str = "https://www.bungee.exchange";

/// Client mode: bridge (cross-chain) or swap (same-chain). Mirrors Go `mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Bridge,
    Swap,
}

/// Bungee quote adapter (mirrors Go `bungee.Client`).
pub struct Client {
    http: HttpClient,
    base_url: String,
    dedicated_base_url: String,
    api_key: String,
    affiliate: String,
    mode: Mode,
}

impl Client {
    /// Build a bridge-mode client (mirrors Go `NewBridge`).
    pub fn new_bridge(http: HttpClient, api_key: &str, affiliate: &str) -> Self {
        Self::new(http, api_key, affiliate, Mode::Bridge)
    }

    /// Build a swap-mode client (mirrors Go `NewSwap`).
    pub fn new_swap(http: HttpClient, api_key: &str, affiliate: &str) -> Self {
        Self::new(http, api_key, affiliate, Mode::Swap)
    }

    fn new(http: HttpClient, api_key: &str, affiliate: &str, mode: Mode) -> Self {
        Client {
            http,
            base_url: DEFAULT_BASE.to_string(),
            dedicated_base_url: DEFAULT_DEDICATED_BASE.to_string(),
            api_key: api_key.to_string(),
            affiliate: affiliate.to_string(),
            mode,
        }
    }

    /// Override the public backend base URL (test seam for Go `baseURL`).
    pub fn set_base_url(&mut self, base: &str) {
        self.base_url = base.to_string();
    }

    /// Override the dedicated backend base URL (test seam for Go
    /// `dedicatedBaseURL`).
    pub fn set_dedicated_base_url(&mut self, base: &str) {
        self.dedicated_base_url = base.to_string();
    }

    /// The current RFC3339 UTC timestamp (seconds precision, trailing `Z`),
    /// matching Go `time.Now().UTC().Format(time.RFC3339)`.
    fn now_rfc3339() -> String {
        Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    /// Resolve dedicated-backend auth: returns `(api_key, affiliate, true)` only
    /// when BOTH are non-empty after trimming (mirrors Go `dedicatedAuth`).
    fn dedicated_auth(&self) -> (String, String, bool) {
        let api_key = self.api_key.trim().to_string();
        let affiliate = self.affiliate.trim().to_string();
        let ok = !api_key.is_empty() && !affiliate.is_empty();
        (api_key, affiliate, ok)
    }

    /// Perform a `/bungee/quote` GET and decode the envelope (mirrors Go
    /// `(*Client).quote`).
    async fn quote(
        &self,
        from_chain: &Chain,
        to_chain: &Chain,
        from_token: &str,
        to_token: &str,
        amount_base: &str,
    ) -> Result<QuoteResponse, Error> {
        let (api_key, affiliate, use_dedicated) = self.dedicated_auth();
        let base = if use_dedicated {
            &self.dedicated_base_url
        } else {
            &self.base_url
        };

        let mut url = Url::parse(&format!("{}/bungee/quote", base.trim_end_matches('/')))
            .map_err(|e| Error::wrap(Code::Internal, "build bungee quote request", e))?;
        url.query_pairs_mut()
            .append_pair("originChainId", &from_chain.evm_chain_id.to_string())
            .append_pair("destinationChainId", &to_chain.evm_chain_id.to_string())
            .append_pair("inputToken", from_token)
            .append_pair("outputToken", to_token)
            .append_pair("inputAmount", amount_base)
            .append_pair("userAddress", default_address_for_chain(from_chain))
            .append_pair("receiverAddress", default_address_for_chain(to_chain));

        let mut req = Request::new(Method::GET, url);
        if use_dedicated {
            set_header(&mut req, "x-api-key", &api_key)?;
            set_header(&mut req, "affiliate", &affiliate)?;
        }

        let resp = self.http.do_json::<QuoteResponse>(req).await?.value;
        if !resp.success {
            return Err(Error::new(Code::Unavailable, bungee_error(&resp.error)));
        }
        Ok(resp)
    }
}

/// Set a request header, mapping invalid header bytes onto an internal error.
fn set_header(req: &mut Request, name: &str, value: &str) -> Result<(), Error> {
    let header_name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|e| Error::wrap(Code::Internal, "build bungee quote header", e))?;
    let header_value = HeaderValue::from_str(value)
        .map_err(|e| Error::wrap(Code::Internal, "build bungee quote header", e))?;
    req.headers_mut().insert(header_name, header_value);
    Ok(())
}

impl Provider for Client {
    fn info(&self) -> model::ProviderInfo {
        let (provider_type, capability) = match self.mode {
            Mode::Swap => ("swap", "swap.quote"),
            Mode::Bridge => ("bridge", "bridge.quote"),
        };
        model::ProviderInfo {
            name: "bungee".to_string(),
            provider_type: provider_type.to_string(),
            requires_key: false,
            capabilities: vec![capability.to_string()],
            key_env_var_name: String::new(),
            capability_auth: vec![
                model::ProviderCapabilityAuth {
                    capability: capability.to_string(),
                    key_env_var: "DEFI_BUNGEE_API_KEY".to_string(),
                    description:
                        "Optional dedicated backend mode (requires both API key and affiliate)"
                            .to_string(),
                },
                model::ProviderCapabilityAuth {
                    capability: capability.to_string(),
                    key_env_var: "DEFI_BUNGEE_AFFILIATE".to_string(),
                    description:
                        "Optional dedicated backend mode (requires both API key and affiliate)"
                            .to_string(),
                },
            ],
        }
    }
}

#[async_trait]
impl BridgeProvider for Client {
    async fn quote_bridge(&self, req: BridgeQuoteRequest) -> Result<model::BridgeQuote, Error> {
        let resp = self
            .quote(
                &req.from_chain,
                &req.to_chain,
                &req.from_asset.address,
                &req.to_asset.address,
                &req.amount_base_units,
            )
            .await?;
        let summary = summarize_quote(&resp, req.to_asset.decimals)?;

        let fee_breakdown = if summary.fee_usd > 0.0 {
            Some(model::BridgeFeeBreakdown {
                gas_fee: Some(model::FeeAmount {
                    amount_usd: summary.fee_usd,
                    ..Default::default()
                }),
                total_fee_usd: summary.fee_usd,
                ..Default::default()
            })
        } else {
            None
        };

        Ok(model::BridgeQuote {
            provider: "bungee".to_string(),
            from_chain_id: req.from_chain.caip2.clone(),
            to_chain_id: req.to_chain.caip2.clone(),
            from_asset_id: req.from_asset.asset_id.clone(),
            to_asset_id: req.to_asset.asset_id.clone(),
            input_amount: model::AmountInfo {
                amount_base_units: req.amount_base_units.clone(),
                amount_decimal: req.amount_decimal.clone(),
                decimals: req.from_asset.decimals as i64,
            },
            from_amount_for_gas: String::new(),
            estimated_destination_native: None,
            estimated_out: model::AmountInfo {
                amount_base_units: summary.amount_base.clone(),
                amount_decimal: format_decimal(&summary.amount_base, summary.decimals),
                decimals: summary.decimals as i64,
            },
            estimated_fee_usd: summary.fee_usd,
            fee_breakdown,
            estimated_time_s: summary.service_time,
            route: summary.route,
            source_url: SOURCE_URL.to_string(),
            fetched_at: Self::now_rfc3339(),
        })
    }
}

#[async_trait]
impl SwapProvider for Client {
    async fn quote_swap(&self, req: SwapQuoteRequest) -> Result<model::SwapQuote, Error> {
        if req.trade_type != SwapTradeType::ExactInput {
            return Err(Error::new(
                Code::Unsupported,
                "bungee supports only --type exact-input",
            ));
        }

        let resp = self
            .quote(
                &req.chain,
                &req.chain,
                &req.from_asset.address,
                &req.to_asset.address,
                &req.amount_base_units,
            )
            .await?;
        let summary = summarize_quote(&resp, req.to_asset.decimals)?;

        Ok(model::SwapQuote {
            provider: "bungee".to_string(),
            chain_id: req.chain.caip2.clone(),
            from_asset_id: req.from_asset.asset_id.clone(),
            to_asset_id: req.to_asset.asset_id.clone(),
            trade_type: SwapTradeType::ExactInput.as_str().to_string(),
            input_amount: model::AmountInfo {
                amount_base_units: req.amount_base_units.clone(),
                amount_decimal: req.amount_decimal.clone(),
                decimals: req.from_asset.decimals as i64,
            },
            estimated_out: model::AmountInfo {
                amount_base_units: summary.amount_base.clone(),
                amount_decimal: format_decimal(&summary.amount_base, summary.decimals),
                decimals: summary.decimals as i64,
            },
            estimated_gas_usd: summary.fee_usd,
            price_impact_pct: 0.0,
            route: summary.route,
            source_url: SOURCE_URL.to_string(),
            fetched_at: Self::now_rfc3339(),
        })
    }
}

// ---------------------------------------------------------------------------
// Wire envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct QuoteResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    result: QuoteResult,
    #[serde(default)]
    error: Value,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteResult {
    #[serde(default)]
    output: QuoteOutput,
    #[serde(default)]
    #[serde(rename = "autoRoute")]
    auto_route: Option<QuoteAutoRoute>,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteOutput {
    #[serde(default)]
    amount: String,
    #[serde(default)]
    decimals: i32,
    #[serde(default)]
    token: QuoteOutputToken,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteOutputToken {
    #[serde(default)]
    decimals: i32,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteAutoRoute {
    #[serde(default)]
    output: QuoteOutput,
    #[serde(default)]
    #[serde(rename = "outputAmount")]
    output_amount: String,
    #[serde(default)]
    #[serde(rename = "estimatedTime")]
    estimated_time: i64,
    #[serde(default)]
    #[serde(rename = "gasFee")]
    gas_fee: Option<QuoteGasFee>,
    #[serde(default)]
    #[serde(rename = "routeDetails")]
    route_details: QuoteDetails,
    #[serde(default)]
    #[serde(rename = "userTxs")]
    user_txs: Vec<QuoteUserTx>,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteGasFee {
    #[serde(
        default,
        rename = "feeInUsd",
        deserialize_with = "crate::serde_util::de_f64_null_default"
    )]
    fee_in_usd: f64,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteUserTx {
    #[serde(default)]
    #[serde(rename = "stepType")]
    step_type: String,
    #[serde(default)]
    #[serde(rename = "routeDetails")]
    route_details: QuoteDetails,
    #[serde(default)]
    #[serde(rename = "swapRoutes")]
    swap_routes: Vec<QuoteSwapRoute>,
    #[serde(default)]
    #[serde(rename = "bridgeRoutes")]
    bridge_routes: Vec<QuoteBridgeRoute>,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteDetails {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteSwapRoute {
    #[serde(default)]
    #[serde(rename = "usedDexName")]
    used_dex_name: String,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteBridgeRoute {
    #[serde(default)]
    #[serde(rename = "usedBridgeNames")]
    used_bridge_names: Vec<String>,
}

// ---------------------------------------------------------------------------
// Summarization (mirrors Go free functions)
// ---------------------------------------------------------------------------

/// The normalized fields extracted from a quote response.
struct QuoteSummary {
    amount_base: String,
    decimals: i32,
    fee_usd: f64,
    service_time: i64,
    route: String,
}

/// Extract output amount/decimals/fee/time/route from a quote envelope,
/// preferring the `autoRoute` projection when present (mirrors Go
/// `summarizeQuote`).
fn summarize_quote(resp: &QuoteResponse, fallback_decimals: i32) -> Result<QuoteSummary, Error> {
    let mut amount_base = resp.result.output.amount.trim().to_string();
    let mut decimals = positive_or_fallback(
        resp.result.output.token.decimals,
        positive_or_fallback(resp.result.output.decimals, fallback_decimals),
    );
    let mut fee_usd = 0.0;
    let mut service_time = 0;
    let mut route = String::new();

    if let Some(auto) = resp.result.auto_route.as_ref() {
        let v = auto.output.amount.trim();
        if !v.is_empty() {
            amount_base = v.to_string();
        }
        let v = auto.output_amount.trim();
        if !v.is_empty() {
            amount_base = v.to_string();
        }
        decimals = positive_or_fallback(
            auto.output.token.decimals,
            positive_or_fallback(auto.output.decimals, decimals),
        );
        if let Some(gas) = auto.gas_fee.as_ref() {
            fee_usd = gas.fee_in_usd;
        }
        service_time = auto.estimated_time;
        let details = auto_route_details(&auto.user_txs, &auto.route_details.name);
        if !details.is_empty() {
            route = format!("bungee:auto:{details}");
        }
    }

    if amount_base.is_empty() {
        return Err(Error::new(
            Code::Unavailable,
            "bungee quote missing output amount",
        ));
    }
    if decimals <= 0 {
        decimals = fallback_decimals;
    }
    if decimals < 0 {
        decimals = 0;
    }

    Ok(QuoteSummary {
        amount_base,
        decimals,
        fee_usd,
        service_time,
        route,
    })
}

/// Compose a lowercased route summary string from the auto-route step list,
/// falling back to a named route when present (mirrors Go `autoRouteDetails`).
fn auto_route_details(user_txs: &[QuoteUserTx], route_name: &str) -> String {
    let route_name = route_name.trim();
    if !route_name.is_empty() {
        return route_name.to_ascii_lowercase();
    }

    let mut steps: Vec<String> = Vec::with_capacity(user_txs.len());
    for tx in user_txs {
        let step = tx.step_type.trim().to_ascii_lowercase();
        match step.as_str() {
            "swap" => {
                let mut names: Vec<String> = tx
                    .swap_routes
                    .iter()
                    .filter_map(|r| {
                        let n = r.used_dex_name.trim().to_ascii_lowercase();
                        if n.is_empty() {
                            None
                        } else {
                            Some(n)
                        }
                    })
                    .collect();
                names.sort();
                if names.is_empty() {
                    steps.push("swap".to_string());
                } else {
                    steps.push(format!("swap({})", unique_strings(names).join("+")));
                }
            }
            "bridge" => {
                let mut names: Vec<String> = Vec::new();
                for r in &tx.bridge_routes {
                    for bridge in &r.used_bridge_names {
                        let n = bridge.trim().to_ascii_lowercase();
                        if !n.is_empty() {
                            names.push(n);
                        }
                    }
                }
                names.sort();
                if names.is_empty() {
                    steps.push("bridge".to_string());
                } else {
                    steps.push(format!("bridge({})", unique_strings(names).join("+")));
                }
            }
            _ => {
                let name = tx.route_details.name.trim().to_ascii_lowercase();
                if !name.is_empty() {
                    steps.push(name);
                } else if !step.is_empty() {
                    steps.push(step);
                }
            }
        }
    }
    steps.join("->")
}

/// Deduplicate adjacent duplicates in a sorted slice (mirrors Go
/// `uniqueStrings`, which assumes its input is already sorted).
fn unique_strings(items: Vec<String>) -> Vec<String> {
    if items.len() <= 1 {
        return items;
    }
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        if i == 0 || Some(item) != out.last() {
            out.push(item.clone());
        }
    }
    out
}

/// Quote-only requests always use the deterministic placeholder address; the
/// chain is accepted for parity with Go but does not change the result.
fn default_address_for_chain(_chain: &Chain) -> &'static str {
    DEFAULT_EVM_USER_ADDRESS
}

/// Return `v` when positive, otherwise `fallback` (mirrors Go
/// `positiveOrFallback`).
fn positive_or_fallback(v: i32, fallback: i32) -> i32 {
    if v > 0 {
        v
    } else {
        fallback
    }
}

/// Best-effort error message extraction from the polymorphic Bungee `error`
/// field (mirrors Go `bungeeError`).
fn bungee_error(v: &Value) -> String {
    const DEFAULT: &str = "bungee quote failed";
    match v {
        Value::Null => DEFAULT.to_string(),
        Value::String(s) => {
            let msg = s.trim();
            if msg.is_empty() {
                DEFAULT.to_string()
            } else {
                msg.to_string()
            }
        }
        Value::Object(map) => {
            if let Some(Value::String(msg)) = map.get("message") {
                let msg = msg.trim();
                if !msg.is_empty() {
                    return msg.to_string();
                }
            }
            DEFAULT.to_string()
        }
        _ => DEFAULT.to_string(),
    }
}

/// Compare two big-int base-unit decimal strings (helper kept generic; unused
/// outside tests but mirrors the numeric semantics other adapters rely on).
#[allow(dead_code)]
fn compare_base_units(a: &str, b: &str) -> Ordering {
    use num_bigint::BigInt;
    let av: BigInt = a.trim().parse().unwrap_or_default();
    let bv: BigInt = b.trim().parse().unwrap_or_default();
    av.cmp(&bv)
}

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for the `defi-providers::bungee` module.
    //!
    //! Go source: `internal/providers/bungee/client.go` + `client_test.go`.
    //! These ports re-express the Go `httptest` suite with `wiremock`
    //! (deterministic, offline). Every Go test case is covered.
    //!
    //! The Rust port is "correct" iff:
    //!
    //!  B1. Bridge quote with an `autoRoute` projection prefers `outputAmount`
    //!      over `output.amount`, surfaces the gas fee USD, estimated time, and
    //!      a `bungee:auto:<lowercased route name>` route, and pins the request
    //!      query params (origin/destination chain ids, input amount).
    //!      (Ports Go `TestQuoteBridgeAutoRoute`.)
    //!
    //!  B2. Swap quote on a non-mainnet EVM chain (hyperevm) uses the
    //!      deterministic placeholder user/receiver address, returns the
    //!      autoRoute `outputAmount` + token decimals, gas USD, and a
    //!      `bungee:auto:swap(<dex>)` route. Trade type echoes `exact-input`.
    //!      (Ports Go `TestQuoteSwapHyperEVM`.)
    //!
    //!  B3. A null `gasFee` yields zero gas USD without erroring; the
    //!      `output.amount` is used when no `outputAmount` is present.
    //!      (Ports Go `TestQuoteSwapHandlesNullGasFee`.)
    //!
    //!  B4. A successful quote with no `autoRoute` returns an empty route.
    //!      (Ports Go `TestQuoteBridgeNoAutoRouteReturnsEmptyRoute`.)
    //!
    //!  B5. An unsuccessful envelope (`success:false`) is surfaced as an error.
    //!      (Ports Go `TestQuoteHandlesUnsuccessfulEnvelope`.)
    //!
    //!  B6. A swap quote with `--type exact-output` is rejected as unsupported
    //!      WITHOUT a network call. (Ports Go `TestQuoteSwapRejectsExactOutput`.)
    //!
    //!  B7. When BOTH api key and affiliate are set, requests go to the
    //!      dedicated base URL with `x-api-key` + `affiliate` headers.
    //!      (Ports Go
    //!      `TestQuoteUsesDedicatedBackendAndHeadersWhenAPIKeyAndAffiliateProvided`.)
    //!
    //!  B8. When the dedicated config is incomplete (key but no affiliate),
    //!      requests fall back to the public base URL with no auth headers.
    //!      (Ports Go `TestQuoteUsesPublicBackendWhenDedicatedConfigIsIncomplete`.)
    //!
    //!  B9. `Provider::info` reflects swap vs bridge mode (type + capability)
    //!      and advertises the optional dedicated-backend auth env vars.

    use super::*;
    use std::time::Duration;

    use defi_id::{parse_asset, parse_chain, Asset};
    use wiremock::matchers::{
        header, header_exists, method, path, query_param, query_param_is_missing,
    };
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::traits::Provider as _;

    fn http() -> HttpClient {
        HttpClient::new(Duration::from_secs(2), 0)
    }

    fn asset(symbol: &str, chain: &Chain) -> Asset {
        parse_asset(symbol, chain).unwrap_or_else(|_| panic!("parse asset {symbol}"))
    }

    fn bridge_req(
        from: &Chain,
        to: &Chain,
        from_asset: Asset,
        to_asset: Asset,
    ) -> BridgeQuoteRequest {
        BridgeQuoteRequest {
            from_chain: from.clone(),
            to_chain: to.clone(),
            from_asset,
            to_asset,
            amount_base_units: "1000000".to_string(),
            amount_decimal: "1".to_string(),
            from_amount_for_gas: String::new(),
        }
    }

    // ----- B1: bridge autoRoute --------------------------------------------
    #[tokio::test]
    async fn quote_bridge_auto_route() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bungee/quote"))
            .and(query_param("originChainId", "1"))
            .and(query_param("destinationChainId", "8453"))
            .and(query_param("inputAmount", "1000000"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "success": true,
                    "result": {
                        "originChainId": 1,
                        "destinationChainId": 8453,
                        "autoRoute": {
                            "estimatedTime": 10,
                            "gasFee": {"feeInUsd": 0.00563382},
                            "routeDetails": {"name": "Bungee Protocol"},
                            "output": {"amount": "995000", "token": {"decimals": 6}},
                            "outputAmount": "999735"
                        }
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let from = parse_chain("ethereum").expect("ethereum");
        let to = parse_chain("base").expect("base");
        let from_asset = asset("USDC", &from);
        let to_asset = asset("USDC", &to);

        let mut client = Client::new_bridge(http(), "", "");
        client.set_base_url(&format!("{}/api/v1", server.uri()));
        let got = client
            .quote_bridge(bridge_req(&from, &to, from_asset, to_asset))
            .await
            .expect("quote_bridge");

        assert_eq!(got.provider, "bungee");
        assert_eq!(got.estimated_out.amount_base_units, "999735");
        assert_eq!(got.estimated_fee_usd, 0.00563382);
        assert_eq!(got.estimated_time_s, 10);
        assert_eq!(got.route, "bungee:auto:bungee protocol");
        let fb = got.fee_breakdown.expect("fee breakdown");
        assert_eq!(fb.total_fee_usd, 0.00563382);
        assert_eq!(fb.gas_fee.expect("gas fee").amount_usd, 0.00563382);
    }

    // ----- B2: swap on hyperevm --------------------------------------------
    #[tokio::test]
    async fn quote_swap_hyperevm() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bungee/quote"))
            .and(query_param("originChainId", "999"))
            .and(query_param("destinationChainId", "999"))
            .and(query_param("userAddress", DEFAULT_EVM_USER_ADDRESS))
            .and(query_param("receiverAddress", DEFAULT_EVM_USER_ADDRESS))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "success": true,
                    "result": {
                        "originChainId": 999,
                        "destinationChainId": 999,
                        "autoRoute": {
                            "gasFee": {"feeInUsd": 0.04},
                            "estimatedTime": 7,
                            "userTxs": [{"stepType": "swap", "swapRoutes": [{"usedDexName": "HyperSwap"}]}],
                            "output": {"amount": "1000000000000000000", "token": {"decimals": 18}},
                            "outputAmount": "1000000000000000001"
                        }
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = parse_chain("hyperevm").expect("hyperevm");
        let from_asset = asset("USDC", &chain);
        let to_asset = asset("WHYPE", &chain);

        let mut client = Client::new_swap(http(), "", "");
        client.set_base_url(&format!("{}/api/v1", server.uri()));
        let got = client
            .quote_swap(SwapQuoteRequest {
                chain: chain.clone(),
                from_asset,
                to_asset,
                amount_base_units: "1000000".to_string(),
                amount_decimal: "1".to_string(),
                ..Default::default()
            })
            .await
            .expect("quote_swap");

        assert_eq!(got.provider, "bungee");
        assert_eq!(got.trade_type, "exact-input");
        assert_eq!(got.chain_id, chain.caip2);
        assert_eq!(got.estimated_out.amount_base_units, "1000000000000000001");
        assert_eq!(got.estimated_out.decimals, 18);
        assert_eq!(got.estimated_gas_usd, 0.04);
        assert_eq!(got.route, "bungee:auto:swap(hyperswap)");
    }

    // ----- B3: null gasFee --------------------------------------------------
    #[tokio::test]
    async fn quote_swap_handles_null_gas_fee() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bungee/quote"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "success": true,
                    "result": {
                        "originChainId": 1,
                        "destinationChainId": 1,
                        "autoRoute": {
                            "estimatedTime": 10,
                            "gasFee": null,
                            "routeDetails": {"name": "Bungee Protocol"},
                            "output": {"amount": "1999735", "token": {"decimals": 6}}
                        }
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = parse_chain("ethereum").expect("ethereum");
        let from_asset = asset("USDC", &chain);
        let to_asset = asset("USDT", &chain);

        let mut client = Client::new_swap(http(), "", "");
        client.set_base_url(&format!("{}/api/v1", server.uri()));
        let got = client
            .quote_swap(SwapQuoteRequest {
                chain: chain.clone(),
                from_asset,
                to_asset,
                amount_base_units: "2000000".to_string(),
                amount_decimal: "2".to_string(),
                ..Default::default()
            })
            .await
            .expect("quote_swap");

        assert_eq!(got.estimated_gas_usd, 0.0);
        assert_eq!(got.estimated_out.amount_base_units, "1999735");
    }

    // ----- B4: no autoRoute -> empty route ---------------------------------
    #[tokio::test]
    async fn quote_bridge_no_auto_route_returns_empty_route() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bungee/quote"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "success": true,
                    "result": {
                        "originChainId": 1,
                        "destinationChainId": 8453,
                        "output": {"amount": "999735", "token": {"decimals": 6}}
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let from = parse_chain("ethereum").expect("ethereum");
        let to = parse_chain("base").expect("base");
        let from_asset = asset("USDC", &from);
        let to_asset = asset("USDC", &to);

        let mut client = Client::new_bridge(http(), "", "");
        client.set_base_url(&format!("{}/api/v1", server.uri()));
        let got = client
            .quote_bridge(bridge_req(&from, &to, from_asset, to_asset))
            .await
            .expect("quote_bridge");

        assert_eq!(got.route, "");
        assert_eq!(got.estimated_out.amount_base_units, "999735");
    }

    // ----- B5: unsuccessful envelope ---------------------------------------
    #[tokio::test]
    async fn quote_handles_unsuccessful_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bungee/quote"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"success": false, "error": {"message":"no routes found"}}"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = parse_chain("ethereum").expect("ethereum");
        let from_asset = asset("USDC", &chain);
        let to_asset = asset("USDT", &chain);

        let mut client = Client::new_swap(http(), "", "");
        client.set_base_url(&format!("{}/api/v1", server.uri()));
        let err = client
            .quote_swap(SwapQuoteRequest {
                chain,
                from_asset,
                to_asset,
                amount_base_units: "1000000".to_string(),
                amount_decimal: "1".to_string(),
                ..Default::default()
            })
            .await
            .expect_err("expected quote error");
        assert_eq!(err.to_string(), "no routes found");
    }

    // ----- B6: exact-output rejected (no network call) ---------------------
    #[tokio::test]
    async fn quote_swap_rejects_exact_output() {
        let chain = parse_chain("ethereum").expect("ethereum");
        let from_asset = asset("USDC", &chain);
        let to_asset = asset("USDT", &chain);

        // No mock server: an exact-output request must fail before any HTTP I/O.
        let client = Client::new_swap(http(), "", "");
        let err = client
            .quote_swap(SwapQuoteRequest {
                chain,
                from_asset,
                to_asset,
                amount_base_units: "1000000".to_string(),
                amount_decimal: "1".to_string(),
                trade_type: SwapTradeType::ExactOutput,
                ..Default::default()
            })
            .await
            .expect_err("expected unsupported exact-output error");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- B7: dedicated backend + headers ---------------------------------
    #[tokio::test]
    async fn quote_uses_dedicated_backend_and_headers_when_key_and_affiliate_provided() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bungee/quote"))
            .and(header("x-api-key", "test-key"))
            .and(header("affiliate", "test-affiliate"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "success": true,
                    "result": {
                        "autoRoute": {
                            "outputAmount": "999735",
                            "output": {"token": {"decimals": 6}}
                        }
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let from = parse_chain("ethereum").expect("ethereum");
        let to = parse_chain("base").expect("base");
        let from_asset = asset("USDC", &from);
        let to_asset = asset("USDC", &to);

        let mut client = Client::new_bridge(http(), "test-key", "test-affiliate");
        client.set_base_url(&format!("{}/unused-public", server.uri()));
        client.set_dedicated_base_url(&format!("{}/api/v1", server.uri()));
        client
            .quote_bridge(bridge_req(&from, &to, from_asset, to_asset))
            .await
            .expect("quote_bridge");
    }

    // ----- B8: public backend fallback (incomplete dedicated config) -------
    #[tokio::test]
    async fn quote_uses_public_backend_when_dedicated_config_incomplete() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bungee/quote"))
            .and(query_param_is_missing("__never__"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "success": true,
                    "result": {
                        "autoRoute": {
                            "outputAmount": "999735",
                            "output": {"token": {"decimals": 6}}
                        }
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        // No request may carry auth headers on the public backend.
        Mock::given(header_exists("x-api-key"))
            .respond_with(ResponseTemplate::new(599))
            .mount(&server)
            .await;
        Mock::given(header_exists("affiliate"))
            .respond_with(ResponseTemplate::new(599))
            .mount(&server)
            .await;

        let from = parse_chain("ethereum").expect("ethereum");
        let to = parse_chain("base").expect("base");
        let from_asset = asset("USDC", &from);
        let to_asset = asset("USDC", &to);

        // Key present but affiliate empty -> dedicated config incomplete.
        let mut client = Client::new_bridge(http(), "test-key", "");
        client.set_base_url(&format!("{}/api/v1", server.uri()));
        client.set_dedicated_base_url(&format!("{}/unused-dedicated", server.uri()));
        client
            .quote_bridge(bridge_req(&from, &to, from_asset, to_asset))
            .await
            .expect("quote_bridge");
    }

    // ----- B9: provider info ------------------------------------------------
    #[test]
    fn provider_info_reflects_mode() {
        let swap = Client::new_swap(http(), "", "").info();
        assert_eq!(swap.name, "bungee");
        assert_eq!(swap.provider_type, "swap");
        assert!(!swap.requires_key);
        assert_eq!(swap.capabilities, vec!["swap.quote".to_string()]);
        assert_eq!(swap.capability_auth.len(), 2);
        assert_eq!(swap.capability_auth[0].key_env_var, "DEFI_BUNGEE_API_KEY");
        assert_eq!(swap.capability_auth[1].key_env_var, "DEFI_BUNGEE_AFFILIATE");
        assert_eq!(swap.capability_auth[0].capability, "swap.quote");

        let bridge = Client::new_bridge(http(), "", "").info();
        assert_eq!(bridge.provider_type, "bridge");
        assert_eq!(bridge.capabilities, vec!["bridge.quote".to_string()]);
        assert_eq!(bridge.capability_auth[0].capability, "bridge.quote");
    }

    // ----- helper unit coverage --------------------------------------------
    #[test]
    fn auto_route_details_sorts_and_dedups_bridge_names() {
        let txs = vec![QuoteUserTx {
            step_type: "bridge".to_string(),
            bridge_routes: vec![QuoteBridgeRoute {
                used_bridge_names: vec![
                    "Stargate".to_string(),
                    "across".to_string(),
                    "across".to_string(),
                ],
            }],
            ..Default::default()
        }];
        assert_eq!(auto_route_details(&txs, ""), "bridge(across+stargate)");
    }

    #[test]
    fn bungee_error_extracts_message_or_default() {
        assert_eq!(
            bungee_error(&serde_json::json!({"message": "boom"})),
            "boom"
        );
        assert_eq!(bungee_error(&serde_json::json!("raw")), "raw");
        assert_eq!(bungee_error(&Value::Null), "bungee quote failed");
        assert_eq!(bungee_error(&serde_json::json!({})), "bungee quote failed");
    }

    #[test]
    fn compare_base_units_orders_numerically() {
        assert_eq!(compare_base_units("100", "99"), Ordering::Greater);
        assert_eq!(compare_base_units("5", "5"), Ordering::Equal);
    }
}
