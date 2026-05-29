//! Jupiter provider adapter — Solana swap quotes over the Jupiter swap HTTP API.
//!
//! Go source: `internal/providers/jupiter/client.go` (+ `client_test.go`).
//!
//! Implements the `SwapProvider` (quote) surface plus `Provider` metadata.
//! Jupiter is quote-only here (no executable action building): the Go adapter
//! does not implement `SwapExecutionProvider`.
//!
//! Jupiter is Solana-only and mainnet-only. Quotes hit `/quote` with the input/
//! output mint addresses, the base-unit amount, and a fixed `slippageBps=50`.
//! An optional API key (`DEFI_JUPITER_API_KEY`) selects the higher-limit "pro"
//! base URL and is sent as the `x-api-key` header; without a key the public
//! "lite" base is used. Amounts carry both base-unit and decimal forms. Only
//! `--type exact-input` is supported (Go is exact-input only). The `fetched_at`
//! clock is injectable for deterministic output.

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_execution::{SwapQuoteRequest, SwapTradeType};
use defi_httpx::Client as HttpClient;
use defi_id::format_decimal;
use defi_model as model;
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Method, Request, Url};
use serde::Deserialize;

use crate::traits::{Provider, SwapProvider};

/// Public "lite" API base used when no API key is configured (mirrors Go
/// `defaultLiteBase`).
const DEFAULT_LITE_BASE: &str = "https://lite-api.jup.ag/swap/v1";
/// Key-gated "pro" API base used when an API key is configured (mirrors Go
/// `defaultProBase`).
const DEFAULT_PRO_BASE: &str = "https://api.jup.ag/swap/v1";
/// Canonical Solana mainnet CAIP-2 id; the only chain Jupiter quotes support
/// (mirrors Go `solanaMainnetCAIP2`).
const SOLANA_MAINNET_CAIP2: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";
/// Public source URL surfaced on every quote (mirrors Go literal).
const SOURCE_URL: &str = "https://jup.ag";
/// Fixed slippage in basis points sent on every quote request (mirrors Go's
/// hard-coded `slippageBps=50`).
const SLIPPAGE_BPS: &str = "50";

/// Jupiter swap-quote adapter (mirrors Go `jupiter.Client`).
pub struct Client {
    http: HttpClient,
    base_url: String,
    api_key: String,
    /// Injected fixed clock for deterministic `fetched_at`; `None` uses the wall
    /// clock.
    now: Option<DateTime<Utc>>,
}

impl Client {
    /// Build a Jupiter client (mirrors Go `New`).
    ///
    /// The API key is trimmed; a non-empty key selects the "pro" base URL and is
    /// sent as `x-api-key`, while an empty key keeps the public "lite" base.
    pub fn new(http: HttpClient, api_key: &str) -> Self {
        let api_key = api_key.trim().to_string();
        let base_url = if api_key.is_empty() {
            DEFAULT_LITE_BASE
        } else {
            DEFAULT_PRO_BASE
        }
        .to_string();
        Client {
            http,
            base_url,
            api_key,
            now: None,
        }
    }

    /// Override the API base URL (test seam for Go's mutable `baseURL`).
    pub fn set_base_url(&mut self, base: &str) {
        self.base_url = base.to_string();
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
    /// `c.now().UTC().Format(time.RFC3339)`.
    fn fetched_at(&self) -> String {
        self.now().to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    /// Provider metadata (mirrors Go `Info`).
    pub fn info(&self) -> model::ProviderInfo {
        model::ProviderInfo {
            name: "jupiter".to_string(),
            provider_type: "swap".to_string(),
            requires_key: false,
            capabilities: vec!["swap.quote".to_string()],
            key_env_var_name: "DEFI_JUPITER_API_KEY".to_string(),
            capability_auth: vec![model::ProviderCapabilityAuth {
                capability: "swap.quote".to_string(),
                key_env_var: "DEFI_JUPITER_API_KEY".to_string(),
                description: "Optional API key for higher Jupiter API limits".to_string(),
            }],
        }
    }
}

impl Provider for Client {
    fn info(&self) -> model::ProviderInfo {
        Client::info(self)
    }
}

#[async_trait]
impl SwapProvider for Client {
    async fn quote_swap(&self, req: SwapQuoteRequest) -> Result<model::SwapQuote, Error> {
        if req.trade_type != SwapTradeType::ExactInput {
            return Err(Error::new(
                Code::Unsupported,
                "jupiter supports only --type exact-input",
            ));
        }
        if !req.chain.is_solana() {
            return Err(Error::new(
                Code::Unsupported,
                "jupiter swap quotes support only Solana chains",
            ));
        }
        if req.chain.caip2 != SOLANA_MAINNET_CAIP2 {
            return Err(Error::new(
                Code::Unsupported,
                "jupiter swap quotes support only Solana mainnet",
            ));
        }

        let mut url = Url::parse(&format!("{}/quote", self.base_url.trim_end_matches('/')))
            .map_err(|e| Error::wrap(Code::Internal, "build jupiter quote request", e))?;
        url.query_pairs_mut()
            .append_pair("inputMint", &req.from_asset.address)
            .append_pair("outputMint", &req.to_asset.address)
            .append_pair("amount", &req.amount_base_units)
            .append_pair("slippageBps", SLIPPAGE_BPS);

        let mut h_req = Request::new(Method::GET, url);
        if !self.api_key.is_empty() {
            set_header(&mut h_req, "x-api-key", &self.api_key)?;
        }

        let resp = self.http.do_json::<QuoteResponse>(h_req).await?.value;
        if resp.out_amount.trim().is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "jupiter quote missing output amount",
            ));
        }

        let out_decimals = req.to_asset.decimals;
        Ok(model::SwapQuote {
            provider: "jupiter".to_string(),
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
                amount_base_units: resp.out_amount.clone(),
                amount_decimal: format_decimal(&resp.out_amount, out_decimals),
                decimals: out_decimals as i64,
            },
            estimated_gas_usd: 0.0,
            price_impact_pct: parse_price_impact_pct(&resp.price_impact_pct),
            route: route_from_plan(&resp.route_plan),
            source_url: SOURCE_URL.to_string(),
            fetched_at: self.fetched_at(),
        })
    }
}

/// Set a request header, mapping invalid header bytes onto an internal error.
fn set_header(req: &mut Request, name: &str, value: &str) -> Result<(), Error> {
    let header_name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|e| Error::wrap(Code::Internal, "build jupiter quote header", e))?;
    let header_value = HeaderValue::from_str(value)
        .map_err(|e| Error::wrap(Code::Internal, "build jupiter quote header", e))?;
    req.headers_mut().insert(header_name, header_value);
    Ok(())
}

/// The Jupiter `/quote` response projection (mirrors Go `quoteResponse`).
#[derive(Debug, Default, Deserialize)]
struct QuoteResponse {
    #[serde(rename = "outAmount", default)]
    out_amount: String,
    #[serde(rename = "priceImpactPct", default)]
    price_impact_pct: String,
    #[serde(rename = "routePlan", default)]
    route_plan: Vec<RoutePlanHop>,
}

/// A single hop of the Jupiter route plan (mirrors the anonymous Go struct).
#[derive(Debug, Default, Deserialize)]
struct RoutePlanHop {
    #[serde(rename = "swapInfo", default)]
    swap_info: SwapInfo,
}

#[derive(Debug, Default, Deserialize)]
struct SwapInfo {
    #[serde(default)]
    label: String,
}

/// Parse the `priceImpactPct` string, clamping unparseable and negative values
/// to `0` (mirrors Go `parsePriceImpactPct`).
fn parse_price_impact_pct(v: &str) -> f64 {
    match v.trim().parse::<f64>() {
        Ok(f) if f >= 0.0 => f,
        _ => 0.0,
    }
}

/// Join the route-plan hop labels into a `" > "`-separated route, collapsing
/// consecutive duplicate labels and skipping empty labels. An empty plan (or one
/// with no usable labels) falls back to `"jupiter"` (mirrors Go `routeFromPlan`).
fn route_from_plan(plan: &[RoutePlanHop]) -> String {
    if plan.is_empty() {
        return "jupiter".to_string();
    }
    let mut parts: Vec<&str> = Vec::with_capacity(plan.len());
    for hop in plan {
        let label = hop.swap_info.label.trim();
        if label.is_empty() {
            continue;
        }
        if parts.last() != Some(&label) {
            parts.push(label);
        }
    }
    if parts.is_empty() {
        return "jupiter".to_string();
    }
    parts.join(" > ")
}

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for the `defi-providers::jupiter` module.
    //!
    //! Go source: `internal/providers/jupiter/client.go` + `client_test.go`.
    //! These ports re-express the Go `httptest` suite with `wiremock`
    //! (deterministic, offline). Every Go test case is covered:
    //!
    //!   * `TestQuoteSwapRejectsNonSolanaChains`        -> J1
    //!   * `TestQuoteSwapRejectsNonMainnetSolanaChain`  -> J2
    //!   * `TestQuoteSwapParsesJupiterResponse`         -> J3
    //!   * `TestQuoteSwapRejectsExactOutput`            -> J4
    //!
    //! The Rust port is "correct" iff:
    //!
    //!  J1. A quote on a non-Solana chain (ethereum) is rejected as
    //!      `Unsupported` WITHOUT a network call.
    //!
    //!  J2. A quote on a Solana chain whose CAIP-2 is NOT the mainnet reference
    //!      is rejected as `Unsupported` WITHOUT a network call.
    //!
    //!  J3. A successful `/quote` response is parsed: provider `jupiter`, trade
    //!      type `exact-input`, output base units from `outAmount`, price impact
    //!      from `priceImpactPct`, and a `" > "`-joined route from the route plan
    //!      hop labels. The `x-api-key` header carries the configured key.
    //!
    //!  J4. A quote with `--type exact-output` is rejected as `Unsupported`
    //!      WITHOUT a network call.
    //!
    //! Additional spec-driven coverage (not 1:1 with a Go test but contract-
    //! relevant): `Provider::info` metadata, base-URL selection by key presence,
    //! the `parse_price_impact_pct` / `route_from_plan` pure helpers, and the
    //! missing-`outAmount` -> `Unavailable` path.

    use super::*;
    use std::time::Duration;

    use chrono::TimeZone;
    use defi_id::{parse_asset, parse_chain, Asset, Chain};
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::traits::Provider as _;

    fn http() -> HttpClient {
        HttpClient::new(Duration::from_secs(2), 0)
    }

    fn client(api_key: &str) -> Client {
        let mut c = Client::new(http(), api_key);
        c.set_now(Utc.with_ymd_and_hms(2026, 5, 28, 12, 0, 0).unwrap());
        c
    }

    fn asset(symbol: &str, chain: &Chain) -> Asset {
        parse_asset(symbol, chain).unwrap_or_else(|_| panic!("parse asset {symbol}"))
    }

    fn req(chain: Chain, from: Asset, to: Asset, trade_type: SwapTradeType) -> SwapQuoteRequest {
        SwapQuoteRequest {
            chain,
            from_asset: from,
            to_asset: to,
            amount_base_units: "2000000".to_string(),
            amount_decimal: "2".to_string(),
            trade_type,
            ..Default::default()
        }
    }

    // ----- J1: non-Solana chain rejected (no network call) ----------------
    #[tokio::test]
    async fn quote_swap_rejects_non_solana_chains() {
        let chain = parse_chain("ethereum").expect("ethereum");
        let from = asset("USDC", &chain);
        let to = asset("DAI", &chain);
        // No mock server: must fail before any HTTP I/O.
        let err = client("")
            .quote_swap(req(chain, from, to, SwapTradeType::ExactInput))
            .await
            .expect_err("expected non-solana chain error");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- J2: non-mainnet Solana chain rejected (no network call) --------
    #[tokio::test]
    async fn quote_swap_rejects_non_mainnet_solana_chain() {
        let chain = Chain {
            name: "Solana Devnet".to_string(),
            slug: "solana-devnet".to_string(),
            caip2: "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1".to_string(),
            evm_chain_id: 0,
        };
        assert!(chain.is_solana(), "devnet chain must still be solana");
        let err = client("")
            .quote_swap(SwapQuoteRequest {
                chain,
                ..Default::default()
            })
            .await
            .expect_err("expected non-mainnet solana chain error");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- J3: parse a successful Jupiter response ------------------------
    #[tokio::test]
    async fn quote_swap_parses_jupiter_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/quote"))
            .and(header("x-api-key", "test-key"))
            .and(query_param("inputMint", &asset("USDC", &solana()).address))
            .and(query_param("slippageBps", "50"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "outAmount":"1995000",
                    "priceImpactPct":"0.13",
                    "routePlan":[
                        {"swapInfo":{"label":"Meteora"}},
                        {"swapInfo":{"label":"Orca"}}
                    ]
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = solana();
        let from = asset("USDC", &chain);
        let to = asset("USDT", &chain);

        let mut c = client("test-key");
        c.set_base_url(&server.uri());
        let quote = c
            .quote_swap(req(chain, from, to, SwapTradeType::ExactInput))
            .await
            .expect("quote_swap");

        assert_eq!(quote.provider, "jupiter");
        assert_eq!(quote.trade_type, "exact-input");
        assert_eq!(quote.estimated_out.amount_base_units, "1995000");
        assert_eq!(quote.input_amount.amount_base_units, "2000000");
        assert_eq!(quote.price_impact_pct, 0.13);
        assert_eq!(quote.route, "Meteora > Orca");
        assert_eq!(quote.source_url, "https://jup.ag");
    }

    // ----- J4: exact-output rejected (no network call) --------------------
    #[tokio::test]
    async fn quote_swap_rejects_exact_output() {
        let chain = solana();
        let from = asset("USDC", &chain);
        let to = asset("USDT", &chain);
        let err = client("")
            .quote_swap(req(chain, from, to, SwapTradeType::ExactOutput))
            .await
            .expect_err("expected unsupported exact-output error");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- missing outAmount -> Unavailable -------------------------------
    #[tokio::test]
    async fn quote_swap_rejects_missing_out_amount() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/quote"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"priceImpactPct":"0.1"}"#, "application/json"),
            )
            .mount(&server)
            .await;

        let chain = solana();
        let from = asset("USDC", &chain);
        let to = asset("USDT", &chain);
        let mut c = client("");
        c.set_base_url(&server.uri());
        let err = c
            .quote_swap(req(chain, from, to, SwapTradeType::ExactInput))
            .await
            .expect_err("expected missing-out-amount error");
        assert_eq!(err.code, Code::Unavailable);
    }

    // ----- metadata -------------------------------------------------------
    #[test]
    fn info_is_metadata_only_no_key_required() {
        let info = Provider::info(&Client::new(http(), ""));
        assert_eq!(info.name, "jupiter");
        assert_eq!(info.provider_type, "swap");
        assert!(!info.requires_key);
        assert_eq!(info.capabilities, vec!["swap.quote".to_string()]);
        assert_eq!(info.key_env_var_name, "DEFI_JUPITER_API_KEY");
        assert_eq!(info.capability_auth.len(), 1);
        assert_eq!(info.capability_auth[0].capability, "swap.quote");
        assert_eq!(info.capability_auth[0].key_env_var, "DEFI_JUPITER_API_KEY");
    }

    // ----- base-URL selection by key presence -----------------------------
    #[test]
    fn new_selects_base_url_by_key_presence() {
        assert_eq!(Client::new(http(), "").base_url, DEFAULT_LITE_BASE);
        assert_eq!(Client::new(http(), "  ").base_url, DEFAULT_LITE_BASE);
        assert_eq!(Client::new(http(), "key").base_url, DEFAULT_PRO_BASE);
        // Key is trimmed.
        assert_eq!(Client::new(http(), "  key  ").api_key, "key");
    }

    // ----- pure helpers ---------------------------------------------------
    #[test]
    fn parse_price_impact_pct_clamps_and_parses() {
        assert_eq!(parse_price_impact_pct("0.13"), 0.13);
        assert_eq!(parse_price_impact_pct("  0.5 "), 0.5);
        assert_eq!(parse_price_impact_pct("-0.2"), 0.0);
        assert_eq!(parse_price_impact_pct("nan-text"), 0.0);
        assert_eq!(parse_price_impact_pct(""), 0.0);
    }

    #[test]
    fn route_from_plan_joins_dedups_and_falls_back() {
        assert_eq!(route_from_plan(&[]), "jupiter");
        assert_eq!(route_from_plan(&[hop("")]), "jupiter");
        assert_eq!(
            route_from_plan(&[hop("Meteora"), hop("Orca")]),
            "Meteora > Orca"
        );
        // Consecutive duplicates collapse; empties skipped.
        assert_eq!(
            route_from_plan(&[hop("Orca"), hop("Orca"), hop(""), hop("Raydium")]),
            "Orca > Raydium"
        );
    }

    fn hop(label: &str) -> RoutePlanHop {
        RoutePlanHop {
            swap_info: SwapInfo {
                label: label.to_string(),
            },
        }
    }

    fn solana() -> Chain {
        parse_chain("solana").expect("parse solana chain")
    }
}
