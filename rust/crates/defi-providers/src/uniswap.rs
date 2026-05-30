//! Uniswap provider adapter — Uniswap Trading API swap quotes.
//!
//! Go source: `internal/providers/uniswap/client.go` (+ `client_test.go`).
//!
//! Implements the [`SwapProvider`] (quote) surface plus [`Provider`] metadata.
//! Uniswap is a quote-only provider here: it does NOT build executable actions
//! (no `SwapActionBuilder`), matching the Go adapter whose only capability is
//! `swap.quote`.
//!
//! Quotes are fetched from the hosted Uniswap Trading API
//! (`https://trade-api.gateway.uniswap.org/v1/quote`) via an HTTP POST with the
//! `x-api-key` header (the route is key-gated: `DEFI_UNISWAP_API_KEY`). EVM
//! chains only. A real `swapper` address is required (the API rejects quotes
//! without one). The trade direction defaults to exact-input; exact-output reads
//! the resolved input amount back from the response. Amounts carry both base
//! units and decimal forms. The `fetched_at` clock is injectable for
//! deterministic output.

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
use serde_json::{json, Map, Value};

use crate::traits::{Provider, SwapProvider};

/// Default Uniswap Trading API base.
const DEFAULT_BASE: &str = "https://trade-api.gateway.uniswap.org";
/// Environment variable that supplies the Uniswap API key.
const KEY_ENV_VAR: &str = "DEFI_UNISWAP_API_KEY";
/// Fallback decimals used when an exact-output input asset reports `0` decimals
/// (mirrors Go's `if inputAmountDecimals <= 0 { inputAmountDecimals = 18 }`).
const DEFAULT_INPUT_DECIMALS: i32 = 18;

/// Uniswap swap-quote adapter (mirrors Go `uniswap.Client`).
pub struct Client {
    http: HttpClient,
    base_url: String,
    api_key: String,
    /// Injected fixed clock for deterministic `fetched_at`; `None` uses the wall
    /// clock.
    now: Option<DateTime<Utc>>,
}

impl Client {
    /// Build a client with the default Uniswap API base (mirrors Go `New`).
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
            name: "uniswap".to_string(),
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
        if !req.chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "uniswap swap quotes support only EVM chains",
            ));
        }
        if self.api_key.is_empty() {
            return Err(Error::new(
                Code::Auth,
                "missing required API key for uniswap (DEFI_UNISWAP_API_KEY)",
            ));
        }

        // Trade type defaults to exact-input; only exact-input/exact-output are
        // accepted (mirrors Go's switch over the trade-type constants).
        let trade_type = req.trade_type;
        match trade_type {
            SwapTradeType::ExactInput | SwapTradeType::ExactOutput => {}
        }

        let swapper = req.swapper.trim();
        if swapper.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "uniswap swap quotes require a swapper address",
            ));
        }

        let mut payload = Map::new();
        payload.insert("tokenInChainId".to_string(), json!(req.chain.evm_chain_id));
        payload.insert("tokenOutChainId".to_string(), json!(req.chain.evm_chain_id));
        payload.insert(
            "tokenIn".to_string(),
            Value::String(req.from_asset.address.clone()),
        );
        payload.insert(
            "tokenOut".to_string(),
            Value::String(req.to_asset.address.clone()),
        );
        payload.insert(
            "amount".to_string(),
            Value::String(req.amount_base_units.clone()),
        );
        payload.insert(
            "type".to_string(),
            Value::String(uniswap_trade_type(trade_type).to_string()),
        );
        payload.insert("swapper".to_string(), Value::String(swapper.to_string()));
        match req.slippage_pct {
            Some(pct) => {
                payload.insert("slippageTolerance".to_string(), json!(pct));
            }
            None => {
                payload.insert(
                    "autoSlippage".to_string(),
                    Value::String("DEFAULT".to_string()),
                );
            }
        }

        let body = serde_json::to_vec(&Value::Object(payload))
            .map_err(|e| Error::wrap(Code::Internal, "marshal uniswap request", e))?;

        let mut headers = HashMap::new();
        headers.insert("x-api-key".to_string(), self.api_key.clone());

        let url = format!("{}/v1/quote", self.base_url.trim_end_matches('/'));
        let resp: QuoteResponse =
            do_body_json(&self.http, Method::POST, &url, Some(body), &headers)
                .await?
                .value;

        // Output amount: top-level `amountOut` wins, else nested
        // `quote.output.amount`.
        let mut amount_out = resp.amount_out.clone();
        if amount_out.is_empty() {
            amount_out = resp.quote.output.amount.clone();
        }
        if amount_out.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "uniswap quote missing output amount",
            ));
        }

        // Input amount: for exact-output the API resolves the input; otherwise
        // echo the request inputs.
        let mut input_amount_base = req.amount_base_units.clone();
        let mut input_amount_decimal = req.amount_decimal.clone();
        let mut input_amount_decimals = req.from_asset.decimals;
        if trade_type == SwapTradeType::ExactOutput {
            input_amount_base = resp.amount_in.clone();
            if input_amount_base.is_empty() {
                input_amount_base = resp.quote.input.amount.clone();
            }
            if input_amount_base.is_empty() {
                return Err(Error::new(
                    Code::Unavailable,
                    "uniswap exact-output quote missing input amount",
                ));
            }
            if input_amount_decimals <= 0 {
                input_amount_decimals = DEFAULT_INPUT_DECIMALS;
            }
            input_amount_decimal = format_decimal(&input_amount_base, input_amount_decimals);
        }

        // Gas estimate: top-level `gasUSD` wins; if absent/zero fall back to the
        // nested `quote.gasFeeUSD`. Both may be numeric or string-encoded.
        let mut gas_usd = parse_json_float(&resp.gas_usd)
            .map_err(|e| Error::wrap(Code::Unavailable, "decode uniswap gasUSD", e))?;
        if gas_usd == 0.0 {
            gas_usd = parse_json_float(&resp.quote.gas_fee_usd)
                .map_err(|e| Error::wrap(Code::Unavailable, "decode uniswap quote.gasFeeUSD", e))?;
        }

        Ok(model::SwapQuote {
            provider: "uniswap".to_string(),
            chain_id: req.chain.caip2.clone(),
            from_asset_id: req.from_asset.asset_id.clone(),
            to_asset_id: req.to_asset.asset_id.clone(),
            trade_type: trade_type.as_str().to_string(),
            input_amount: model::AmountInfo {
                amount_base_units: input_amount_base,
                amount_decimal: input_amount_decimal,
                decimals: input_amount_decimals as i64,
            },
            estimated_out: model::AmountInfo {
                amount_base_units: amount_out.clone(),
                amount_decimal: format_decimal(&amount_out, req.to_asset.decimals),
                decimals: req.to_asset.decimals as i64,
            },
            estimated_gas_usd: gas_usd,
            price_impact_pct: 0.0,
            route: "uniswap".to_string(),
            source_url: "https://app.uniswap.org".to_string(),
            fetched_at: self.fetched_at(),
        })
    }
}

/// Decoded Uniswap Trading API quote response (mirrors Go `quoteResponse`).
#[derive(Debug, Default, Deserialize)]
struct QuoteResponse {
    #[serde(default)]
    quote: QuoteInner,
    #[serde(rename = "amountIn", default)]
    amount_in: String,
    #[serde(rename = "amountOut", default)]
    amount_out: String,
    /// Raw `gasUSD` token — may be a JSON number or a string-encoded number.
    #[serde(rename = "gasUSD", default)]
    gas_usd: Value,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteInner {
    #[serde(default)]
    input: QuoteAmount,
    #[serde(default)]
    output: QuoteAmount,
    /// Raw `gasFeeUSD` token — may be a JSON number or a string-encoded number.
    #[serde(rename = "gasFeeUSD", default)]
    gas_fee_usd: Value,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteAmount {
    #[serde(default)]
    amount: String,
}

/// Parse a JSON value as an `f64`, accepting either a JSON number or a
/// string-encoded number (mirrors Go `parseJSONFloat`).
///
/// Returns `0.0` for an absent/`null`/empty-string token.
fn parse_json_float(raw: &Value) -> Result<f64, Error> {
    match raw {
        Value::Null => Ok(0.0),
        Value::Number(n) => n.as_f64().ok_or_else(|| {
            Error::new(
                Code::Unavailable,
                "expected numeric or string-encoded numeric value",
            )
        }),
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() || trimmed == "null" {
                return Ok(0.0);
            }
            trimmed
                .parse::<f64>()
                .map_err(|e| Error::wrap(Code::Unavailable, "parse numeric string", e))
        }
        _ => Err(Error::new(
            Code::Unavailable,
            "expected numeric or string-encoded numeric value",
        )),
    }
}

/// Map a [`SwapTradeType`] onto the Uniswap API trade-type string (mirrors Go
/// `uniswapTradeType`).
fn uniswap_trade_type(t: SwapTradeType) -> &'static str {
    match t {
        SwapTradeType::ExactOutput => "EXACT_OUTPUT",
        SwapTradeType::ExactInput => "EXACT_INPUT",
    }
}

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for the `defi-providers::uniswap` module.
    //!
    //! Go source: `internal/providers/uniswap/client.go` (+ `client_test.go`).
    //! The Uniswap Trading API is mocked with `wiremock` (the Rust analogue of
    //! Go's `httptest`). Tests are deterministic and offline. Each test
    //! re-expresses one Go `client_test.go` case:
    //!
    //!   * `TestQuoteSwapIncludesRequiredSwapper`
    //!   * `TestQuoteSwapUsesManualSlippageOverride`
    //!   * `TestQuoteSwapSupportsExactOutput`
    //!   * `TestQuoteSwapExactOutputFallsBackInputDecimalsWhenMissing`
    //!   * `TestQuoteSwapRequiresAPIKey`
    //!   * `TestQuoteSwapRequiresSwapper`
    //!   * `TestQuoteSwapRejectsNonEVMChain`
    //!
    //! Contract invariants asserted: provider metadata (key-gated, single
    //! `swap.quote` capability); request payload shape (chain ids, token
    //! addresses, amount, `EXACT_INPUT`/`EXACT_OUTPUT` type, swapper, and the
    //! mutually-exclusive `autoSlippage=DEFAULT` vs `slippageTolerance`);
    //! `x-api-key` header; exact-input vs exact-output amount resolution; the
    //! 18-decimals fallback; string-encoded gas parsing; the canonical
    //! `exact-input`/`exact-output` trade-type echo; and deterministic
    //! `fetched_at`.

    use super::*;

    use chrono::TimeZone;
    use defi_id::{parse_asset, parse_chain, Asset, Chain};
    use serde_json::Value;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    const TEST_SWAPPER: &str = "0x000000000000000000000000000000000000dEaD";

    /// A responder that captures the request body and replies with a fixed JSON
    /// document. The captured body lets tests assert payload shape (the Go tests
    /// decode `r.Body` into a typed struct).
    struct CaptureResponder {
        body: &'static str,
        captured: Arc<Mutex<Option<Value>>>,
        require_key: bool,
    }

    impl Respond for CaptureResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            if self.require_key {
                let key = request
                    .headers
                    .get("x-api-key")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                if key != "test-key" {
                    return ResponseTemplate::new(401);
                }
            }
            match serde_json::from_slice::<Value>(&request.body) {
                Ok(v) => {
                    *self.captured.lock().expect("lock") = Some(v);
                }
                Err(_) => return ResponseTemplate::new(400),
            }
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/json")
                .set_body_string(self.body)
        }
    }

    /// Start a mock server returning `body` for `POST /v1/quote`, capturing the
    /// request payload into the returned handle.
    async fn mock_server(
        body: &'static str,
        require_key: bool,
    ) -> (MockServer, Arc<Mutex<Option<Value>>>) {
        let server = MockServer::start().await;
        let captured = Arc::new(Mutex::new(None));
        Mock::given(method("POST"))
            .and(path("/v1/quote"))
            .respond_with(CaptureResponder {
                body,
                captured: captured.clone(),
                require_key,
            })
            .mount(&server)
            .await;
        (server, captured)
    }

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
            swapper: TEST_SWAPPER.to_string(),
        }
    }

    // ----- metadata -------------------------------------------------------

    #[test]
    fn info_is_key_gated_quote_only() {
        let c = Client::new(http(), "");
        let info = Provider::info(&c);
        assert_eq!(info.name, "uniswap");
        assert_eq!(info.provider_type, "swap");
        assert!(info.requires_key);
        assert_eq!(info.key_env_var_name, "DEFI_UNISWAP_API_KEY");
        assert_eq!(info.capabilities, vec!["swap.quote".to_string()]);
        assert_eq!(info.capability_auth.len(), 1);
        assert_eq!(info.capability_auth[0].capability, "swap.quote");
        assert_eq!(info.capability_auth[0].key_env_var, "DEFI_UNISWAP_API_KEY");
    }

    // ----- TestQuoteSwapIncludesRequiredSwapper ---------------------------

    #[tokio::test]
    async fn quote_swap_includes_required_swapper() {
        let (server, captured) = mock_server(
            r#"{"quote":{"output":{"amount":"999847836538317147"},"gasFeeUSD":"0.1589"}}"#,
            true,
        )
        .await;
        let (chain, from, to) = eth_assets();
        let from_addr = from.address.clone();
        let to_addr = to.address.clone();

        let mut c = client("test-key");
        c.set_base_url(&server.uri());
        let quote = c
            .quote_swap(base_req(chain, from, to))
            .await
            .expect("quote");

        let body = captured.lock().expect("lock").clone().expect("captured");
        assert_eq!(body["tokenInChainId"], json!(1));
        assert_eq!(body["tokenOutChainId"], json!(1));
        assert_eq!(body["tokenIn"], json!(from_addr));
        assert_eq!(body["tokenOut"], json!(to_addr));
        assert_eq!(body["amount"], json!("1000000"));
        assert_eq!(body["type"], json!("EXACT_INPUT"));
        assert_eq!(body["swapper"], json!(TEST_SWAPPER));
        assert_eq!(body["autoSlippage"], json!("DEFAULT"));
        assert!(
            body.get("slippageTolerance").is_none(),
            "slippageTolerance must be omitted when auto-slippage is used"
        );

        assert_eq!(quote.provider, "uniswap");
        assert_eq!(quote.trade_type, "exact-input");
        assert_eq!(quote.estimated_out.amount_base_units, "999847836538317147");
        assert_eq!(quote.estimated_gas_usd, 0.1589);
        assert_eq!(quote.fetched_at, "2026-02-25T17:30:00Z");
    }

    // ----- TestQuoteSwapUsesManualSlippageOverride ------------------------

    #[tokio::test]
    async fn quote_swap_uses_manual_slippage_override() {
        let (server, captured) = mock_server(
            r#"{"quote":{"output":{"amount":"1000000000000000000"},"gasFeeUSD":"0.1"}}"#,
            true,
        )
        .await;
        let (chain, from, to) = eth_assets();

        let mut c = client("test-key");
        c.set_base_url(&server.uri());
        let mut req = base_req(chain, from, to);
        req.slippage_pct = Some(1.25);
        let quote = c.quote_swap(req).await.expect("quote");

        let body = captured.lock().expect("lock").clone().expect("captured");
        assert!(
            body.get("autoSlippage").is_none(),
            "autoSlippage must be omitted when a manual override is given"
        );
        assert_eq!(body["slippageTolerance"], json!(1.25));
        assert_eq!(quote.estimated_gas_usd, 0.1);
        assert_eq!(quote.trade_type, "exact-input");
    }

    // ----- TestQuoteSwapSupportsExactOutput -------------------------------

    #[tokio::test]
    async fn quote_swap_supports_exact_output() {
        let (server, captured) = mock_server(
            r#"{"quote":{"input":{"amount":"1000900"},"output":{"amount":"1000000000000000000"},"gasFeeUSD":"0.12"}}"#,
            true,
        )
        .await;
        let (chain, from, to) = eth_assets();

        let mut c = client("test-key");
        c.set_base_url(&server.uri());
        let mut req = base_req(chain, from, to);
        req.amount_base_units = "1000000000000000000".to_string();
        req.trade_type = SwapTradeType::ExactOutput;
        let quote = c.quote_swap(req).await.expect("quote");

        let body = captured.lock().expect("lock").clone().expect("captured");
        assert_eq!(body["type"], json!("EXACT_OUTPUT"));
        assert_eq!(body["amount"], json!("1000000000000000000"));

        assert_eq!(quote.trade_type, "exact-output");
        // USDC input has 6 decimals: 1000900 base -> 1.0009.
        assert_eq!(quote.input_amount.amount_base_units, "1000900");
        assert_eq!(quote.input_amount.amount_decimal, "1.0009");
        assert_eq!(quote.estimated_out.amount_base_units, "1000000000000000000");
    }

    // ----- TestQuoteSwapExactOutputFallsBackInputDecimalsWhenMissing ------

    #[tokio::test]
    async fn quote_swap_exact_output_falls_back_input_decimals_when_missing() {
        let (server, _captured) = mock_server(
            r#"{"quote":{"input":{"amount":"1000900000000000000"},"output":{"amount":"1000000000000000000"},"gasFeeUSD":"0.12"}}"#,
            true,
        )
        .await;
        let chain = parse_chain("ethereum").expect("parse ethereum");
        let from = Asset {
            chain_id: chain.caip2.clone(),
            asset_id: "eip155:1/erc20:0x1111111111111111111111111111111111111111".to_string(),
            address: "0x1111111111111111111111111111111111111111".to_string(),
            symbol: "UNK".to_string(),
            decimals: 0,
        };
        let to = parse_asset("DAI", &chain).expect("parse DAI");

        let mut c = client("test-key");
        c.set_base_url(&server.uri());
        let mut req = base_req(chain, from, to);
        req.amount_base_units = "1000000000000000000".to_string();
        req.trade_type = SwapTradeType::ExactOutput;
        let quote = c.quote_swap(req).await.expect("quote");

        // Fallback to 18 decimals: 1000900000000000000 base -> 1.0009.
        assert_eq!(quote.input_amount.amount_decimal, "1.0009");
        assert_eq!(quote.input_amount.decimals, 18);
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

    // ----- TestQuoteSwapRequiresSwapper -----------------------------------

    #[tokio::test]
    async fn quote_swap_requires_swapper() {
        let (chain, from, to) = eth_assets();
        let c = client("test-key");
        let mut req = base_req(chain, from, to);
        req.swapper = String::new();
        let err = c
            .quote_swap(req)
            .await
            .expect_err("missing swapper must fail");
        assert_eq!(err.code, Code::Usage);
    }

    // ----- TestQuoteSwapRejectsNonEVMChain --------------------------------

    #[tokio::test]
    async fn quote_swap_rejects_non_evm_chain() {
        let chain = parse_chain("solana").expect("parse solana");
        let from = parse_asset("USDC", &chain).expect("parse USDC");
        let to = parse_asset("USDT", &chain).expect("parse USDT");
        // No API key set, but the EVM check runs first.
        let c = client("");
        let err = c
            .quote_swap(base_req(chain, from, to))
            .await
            .expect_err("non-EVM chain must fail");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- pure-helper coverage -------------------------------------------

    #[test]
    fn parse_json_float_accepts_numeric_and_string() {
        assert_eq!(parse_json_float(&json!(0.1589)).expect("num"), 0.1589);
        assert_eq!(parse_json_float(&json!("0.1589")).expect("str"), 0.1589);
        assert_eq!(parse_json_float(&Value::Null).expect("null"), 0.0);
        assert_eq!(parse_json_float(&json!("")).expect("empty"), 0.0);
        assert_eq!(parse_json_float(&json!("null")).expect("nullstr"), 0.0);
        assert!(parse_json_float(&json!("nope")).is_err());
    }

    #[test]
    fn uniswap_trade_type_strings() {
        assert_eq!(uniswap_trade_type(SwapTradeType::ExactInput), "EXACT_INPUT");
        assert_eq!(
            uniswap_trade_type(SwapTradeType::ExactOutput),
            "EXACT_OUTPUT"
        );
    }
}
