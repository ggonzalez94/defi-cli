//! 1inch provider adapter — 1inch Swap API (v6.0) swap quotes.
//!
//! Go source: `internal/providers/oneinch/client.go` (+ `client_test.go`).
//!
//! Implements the [`SwapProvider`] (quote) surface plus [`Provider`] metadata.
//! 1inch is a quote-only provider here: it does NOT build executable actions
//! (no `SwapActionBuilder`), matching the Go adapter whose only capability is
//! `swap.quote`.
//!
//! Quotes are fetched from the hosted 1inch API
//! (`https://api.1inch.dev/swap/v6.0/{chainId}/quote`) via an HTTP GET with the
//! `Authorization: Bearer <key>` header (the route is key-gated:
//! `DEFI_1INCH_API_KEY`). EVM chains only. Exact-input only (exact-output is
//! rejected as unsupported). The destination amount is read from `dstAmount`;
//! the input amount echoes the request inputs. Gas is requested
//! (`includeGas=true`) but, matching the Go adapter, `estimated_gas_usd` stays
//! `0` (the response gas figure is not a USD value). The `fetched_at` clock is
//! injectable for deterministic output.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_execution::{SwapQuoteRequest, SwapTradeType};
use defi_httpx::{do_body_json, Client as HttpClient};
use defi_id::format_decimal;
use defi_model as model;
use reqwest::Method;
use serde::Deserialize;

use crate::traits::{Provider, SwapProvider};

/// Default 1inch API base.
const DEFAULT_BASE: &str = "https://api.1inch.dev";
/// Environment variable that supplies the 1inch API key.
const KEY_ENV_VAR: &str = "DEFI_1INCH_API_KEY";

/// 1inch swap-quote adapter (mirrors Go `oneinch.Client`).
pub struct Client {
    http: HttpClient,
    base_url: String,
    api_key: String,
    /// Injected fixed clock for deterministic `fetched_at`; `None` uses the wall
    /// clock.
    now: Option<DateTime<Utc>>,
}

impl Client {
    /// Build a client with the default 1inch API base (mirrors Go `New`).
    pub fn new(http: HttpClient, api_key: impl Into<String>) -> Self {
        Client {
            http,
            base_url: DEFAULT_BASE.to_string(),
            api_key: api_key.into(),
            now: None,
        }
    }

    /// Override the API base URL (test seam for Go `baseURL`).
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
    /// `time.Now().UTC().Format(time.RFC3339)`.
    fn fetched_at(&self) -> String {
        self.now().to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    /// Provider metadata (mirrors Go `Info`).
    pub fn info(&self) -> model::ProviderInfo {
        model::ProviderInfo {
            name: "1inch".to_string(),
            provider_type: "swap".to_string(),
            requires_key: true,
            capabilities: vec!["swap.quote".to_string()],
            key_env_var_name: KEY_ENV_VAR.to_string(),
            capability_auth: vec![model::ProviderCapabilityAuth {
                capability: "swap.quote".to_string(),
                key_env_var: KEY_ENV_VAR.to_string(),
                description: String::new(),
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
        // Trade type defaults to exact-input; only exact-input is supported.
        match req.trade_type {
            SwapTradeType::ExactInput => {}
            SwapTradeType::ExactOutput => {
                return Err(Error::new(
                    Code::Unsupported,
                    "1inch supports only --type exact-input",
                ));
            }
        }

        if !req.chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "1inch swap quotes support only EVM chains",
            ));
        }
        if self.api_key.is_empty() {
            return Err(Error::new(
                Code::Auth,
                "missing required API key for 1inch (DEFI_1INCH_API_KEY)",
            ));
        }

        // Build `{base}/swap/v6.0/{chainId}/quote?...`. `query_pairs_mut`
        // URL-encodes values just like Go's `url.Values.Encode`.
        let chain_id = req.chain.evm_chain_id;
        let mut url = reqwest::Url::parse(&format!(
            "{}/swap/v6.0/{}/quote",
            self.base_url.trim_end_matches('/'),
            chain_id
        ))
        .map_err(|e| Error::wrap(Code::Internal, "build 1inch quote request", e))?;
        url.query_pairs_mut()
            .append_pair("src", &req.from_asset.address)
            .append_pair("dst", &req.to_asset.address)
            .append_pair("amount", &req.amount_base_units)
            .append_pair("includeGas", "true");

        let mut headers = HashMap::new();
        headers.insert(
            "Authorization".to_string(),
            format!("Bearer {}", self.api_key),
        );

        let resp: QuoteResponse =
            do_body_json(&self.http, Method::GET, url.as_str(), None, &headers)
                .await?
                .value;

        if resp.dst_amount.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "1inch quote missing destination amount",
            ));
        }

        Ok(model::SwapQuote {
            provider: "1inch".to_string(),
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
                amount_base_units: resp.dst_amount.clone(),
                amount_decimal: format_decimal(&resp.dst_amount, req.to_asset.decimals),
                decimals: req.to_asset.decimals as i64,
            },
            // Go hardcodes EstimatedGasUSD to 0 (the 1inch `gas` figure is a gas
            // unit estimate, not a USD value).
            estimated_gas_usd: 0.0,
            price_impact_pct: 0.0,
            route: "1inch".to_string(),
            source_url: "https://app.1inch.io".to_string(),
            fetched_at: self.fetched_at(),
        })
    }
}

/// Decoded 1inch quote response (mirrors Go `quoteResponse`).
#[derive(Debug, Default, Deserialize)]
struct QuoteResponse {
    #[serde(rename = "dstAmount", default)]
    dst_amount: String,
    /// Gas-unit estimate; decoded for completeness but not surfaced (Go reads it
    /// into `Gas` but never emits it).
    #[serde(default)]
    #[allow(dead_code)]
    gas: f64,
}

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for the `defi-providers::oneinch` module.
    //!
    //! Go source: `internal/providers/oneinch/client.go` (+ `client_test.go`).
    //! The 1inch Swap API is mocked with `wiremock` (the Rust analogue of Go's
    //! `httptest`). Tests are deterministic and offline.
    //!
    //! Ports of the Go `client_test.go` cases:
    //!   * `TestQuoteSwapRequiresAPIKey`     -> [`quote_swap_requires_api_key`]
    //!   * `TestQuoteSwapRejectsNonEVMChain` -> [`quote_swap_rejects_non_evm_chain`]
    //!   * `TestQuoteSwapRejectsExactOutput` -> [`quote_swap_rejects_exact_output`]
    //!
    //! Plus contract-invariant coverage the Go suite leaves implicit (exercised
    //! indirectly by the runner/schema in Go), made explicit here:
    //!   * provider metadata: key-gated, single `swap.quote` capability, name
    //!     `1inch`, env var `DEFI_1INCH_API_KEY`;
    //!   * happy-path quote shape: GET `/swap/v6.0/{chainId}/quote` with the
    //!     `src|dst|amount|includeGas` query params and the
    //!     `Authorization: Bearer` header; `dstAmount` -> `estimated_out`;
    //!     echoed input amount; `exact-input` trade-type echo; `estimated_gas_usd`
    //!     stays `0`; deterministic `fetched_at`;
    //!   * missing `dstAmount` -> `Unavailable`.

    use super::*;

    use chrono::TimeZone;
    use defi_id::{parse_asset, parse_chain, Asset, Chain};
    use std::time::Duration;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn http() -> HttpClient {
        HttpClient::new(Duration::from_secs(1), 0)
    }

    fn client(api_key: &str) -> Client {
        let mut c = Client::new(http(), api_key);
        c.set_now(Utc.with_ymd_and_hms(2026, 2, 25, 17, 30, 0).unwrap());
        c
    }

    fn eth_assets() -> (Chain, Asset, Asset) {
        let chain = parse_chain("ethereum").expect("parse ethereum");
        let from = parse_asset("USDC", &chain).expect("parse USDC");
        let to = parse_asset("DAI", &chain).expect("parse DAI");
        (chain, from, to)
    }

    fn base_req(chain: Chain, from: Asset, to: Asset) -> SwapQuoteRequest {
        SwapQuoteRequest {
            chain,
            from_asset: from,
            to_asset: to,
            amount_base_units: "1000000".to_string(),
            amount_decimal: "1".to_string(),
            rpc_url: String::new(),
            trade_type: SwapTradeType::ExactInput,
            slippage_pct: None,
            swapper: String::new(),
        }
    }

    // ----- metadata -------------------------------------------------------

    #[test]
    fn info_is_key_gated_quote_only() {
        let c = Client::new(http(), "");
        let info = Provider::info(&c);
        assert_eq!(info.name, "1inch");
        assert_eq!(info.provider_type, "swap");
        assert!(info.requires_key);
        assert_eq!(info.key_env_var_name, "DEFI_1INCH_API_KEY");
        assert_eq!(info.capabilities, vec!["swap.quote".to_string()]);
        assert_eq!(info.capability_auth.len(), 1);
        assert_eq!(info.capability_auth[0].capability, "swap.quote");
        assert_eq!(info.capability_auth[0].key_env_var, "DEFI_1INCH_API_KEY");
    }

    // ----- happy path -----------------------------------------------------

    #[tokio::test]
    async fn quote_swap_builds_quote_and_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/swap/v6.0/1/quote"))
            .and(query_param("amount", "1000000"))
            .and(query_param("includeGas", "true"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/json")
                    .set_body_string(r#"{"dstAmount":"999847836538317147","gas":120000}"#),
            )
            .mount(&server)
            .await;

        let (chain, from, to) = eth_assets();
        let from_id = from.asset_id.clone();
        let to_id = to.asset_id.clone();
        let to_decimals = to.decimals as i64;

        let mut c = client("test-key");
        c.set_base_url(&server.uri());
        let quote = c
            .quote_swap(base_req(chain, from, to))
            .await
            .expect("quote");

        assert_eq!(quote.provider, "1inch");
        assert_eq!(quote.chain_id, "eip155:1");
        assert_eq!(quote.from_asset_id, from_id);
        assert_eq!(quote.to_asset_id, to_id);
        assert_eq!(quote.trade_type, "exact-input");
        assert_eq!(quote.input_amount.amount_base_units, "1000000");
        assert_eq!(quote.input_amount.amount_decimal, "1");
        assert_eq!(quote.estimated_out.amount_base_units, "999847836538317147");
        // DAI has 18 decimals: 999847836538317147 base -> 0.999847836538317147.
        assert_eq!(quote.estimated_out.amount_decimal, "0.999847836538317147");
        assert_eq!(quote.estimated_out.decimals, to_decimals);
        // Go hardcodes gas USD to 0.
        assert_eq!(quote.estimated_gas_usd, 0.0);
        assert_eq!(quote.route, "1inch");
        assert_eq!(quote.source_url, "https://app.1inch.io");
        assert_eq!(quote.fetched_at, "2026-02-25T17:30:00Z");
    }

    #[tokio::test]
    async fn quote_swap_errors_on_missing_dst_amount() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/swap/v6.0/1/quote"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/json")
                    .set_body_string(r#"{"gas":120000}"#),
            )
            .mount(&server)
            .await;

        let (chain, from, to) = eth_assets();
        let mut c = client("test-key");
        c.set_base_url(&server.uri());
        let err = c
            .quote_swap(base_req(chain, from, to))
            .await
            .expect_err("missing dstAmount must fail");
        assert_eq!(err.code, Code::Unavailable);
    }

    // ----- TestQuoteSwapRequiresAPIKey ------------------------------------

    #[tokio::test]
    async fn quote_swap_requires_api_key() {
        let (chain, from, to) = eth_assets();
        let c = client("");
        let err = c
            .quote_swap(base_req(chain, from, to))
            .await
            .expect_err("missing API key must fail");
        assert_eq!(err.code, Code::Auth);
    }

    // ----- TestQuoteSwapRejectsNonEVMChain --------------------------------

    #[tokio::test]
    async fn quote_swap_rejects_non_evm_chain() {
        let chain = parse_chain("solana").expect("parse solana");
        let from = parse_asset("USDC", &chain).expect("parse USDC");
        let to = parse_asset("USDT", &chain).expect("parse USDT");
        // No API key set, but the EVM check runs before the key check.
        let c = client("");
        let err = c
            .quote_swap(base_req(chain, from, to))
            .await
            .expect_err("non-EVM chain must fail");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- TestQuoteSwapRejectsExactOutput --------------------------------

    #[tokio::test]
    async fn quote_swap_rejects_exact_output() {
        let (chain, from, to) = eth_assets();
        let c = client("test-key");
        let mut req = base_req(chain, from, to);
        req.amount_base_units = "1000000000000000000".to_string();
        req.trade_type = SwapTradeType::ExactOutput;
        let err = c
            .quote_swap(req)
            .await
            .expect_err("exact-output must be unsupported");
        assert_eq!(err.code, Code::Unsupported);
    }
}
