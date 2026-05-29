//! Fibrous provider adapter — EVM swap quotes over the Fibrous Finance HTTP API.
//!
//! Go source: `internal/providers/fibrous/client.go` (+ `client_test.go`).
//!
//! Implements the `SwapProvider` (quote) surface plus `Provider` metadata.
//! Fibrous is quote-only here (no executable action building): the Go adapter
//! does not implement `SwapExecutionProvider`.
//!
//! Fibrous quotes hit `/{chainSlug}/route` with the base-unit amount and the
//! input/output token addresses. Only a fixed set of chains is supported
//! (keyed by EVM chain ID → Fibrous chain slug): HyperEVM (`999`), Citrea
//! (`4114`), and Base (`8453`). Amounts carry both base-unit and decimal forms.
//! Only `--type exact-input` is supported (Go is exact-input only). No API key
//! is required. The `fetched_at` clock is injectable for deterministic output.

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_execution::{SwapQuoteRequest, SwapTradeType};
use defi_httpx::Client as HttpClient;
use defi_id::format_decimal;
use defi_model as model;
use reqwest::{Method, Request, Url};
use serde::Deserialize;

use crate::traits::{Provider, SwapProvider};

/// Default Fibrous API base URL (mirrors Go `defaultBase`).
const DEFAULT_BASE: &str = "https://api.fibrous.finance";
/// Public source URL surfaced on every quote (mirrors Go literal).
const SOURCE_URL: &str = "https://fibrous.finance";

/// Map an EVM chain ID to its Fibrous API chain slug (mirrors Go `chainSlugs`).
///
/// Returns `None` for any chain Fibrous does not support.
fn chain_slug(evm_chain_id: i64) -> Option<&'static str> {
    match evm_chain_id {
        999 => Some("hyperevm"),
        4114 => Some("citrea"),
        8453 => Some("base"),
        _ => None,
    }
}

/// The full sorted list of supported Fibrous chain slugs, used in the
/// unsupported-chain error message (mirrors Go's `sort.Strings(supported)`).
const SUPPORTED_SLUGS: [&str; 3] = ["base", "citrea", "hyperevm"];

/// Fibrous swap-quote adapter (mirrors Go `fibrous.Client`).
pub struct Client {
    http: HttpClient,
    base_url: String,
    /// Injected fixed clock for deterministic `fetched_at`; `None` uses the wall
    /// clock.
    now: Option<DateTime<Utc>>,
}

impl Client {
    /// Build a Fibrous client (mirrors Go `New`).
    pub fn new(http: HttpClient) -> Self {
        Client {
            http,
            base_url: DEFAULT_BASE.to_string(),
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
            name: "fibrous".to_string(),
            provider_type: "swap".to_string(),
            requires_key: false,
            capabilities: vec!["swap.quote".to_string()],
            key_env_var_name: String::new(),
            capability_auth: Vec::new(),
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
                "fibrous supports only --type exact-input",
            ));
        }

        let chain_slug = chain_slug(req.chain.evm_chain_id).ok_or_else(|| {
            Error::new(
                Code::Unsupported,
                format!(
                    "fibrous does not support chain {} (supported: {})",
                    req.chain.slug,
                    SUPPORTED_SLUGS.join(", ")
                ),
            )
        })?;

        let mut url = Url::parse(&format!(
            "{}/{}/route",
            self.base_url.trim_end_matches('/'),
            chain_slug
        ))
        .map_err(|e| Error::wrap(Code::Internal, "build fibrous route request", e))?;
        url.query_pairs_mut()
            .append_pair("amount", &req.amount_base_units)
            .append_pair("tokenInAddress", &req.from_asset.address)
            .append_pair("tokenOutAddress", &req.to_asset.address);

        let h_req = Request::new(Method::GET, url);
        let resp = self.http.do_json::<RouteResponse>(h_req).await?.value;

        if !resp.success {
            return Err(Error::new(
                Code::Unavailable,
                "fibrous route returned success=false",
            ));
        }
        if resp.output_amount.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "fibrous route missing output amount",
            ));
        }

        let trade_type = SwapTradeType::ExactInput;
        let out_decimals = req.to_asset.decimals;
        Ok(model::SwapQuote {
            provider: "fibrous".to_string(),
            chain_id: req.chain.caip2.clone(),
            from_asset_id: req.from_asset.asset_id.clone(),
            to_asset_id: req.to_asset.asset_id.clone(),
            trade_type: trade_type.as_str().to_string(),
            input_amount: model::AmountInfo {
                amount_base_units: req.amount_base_units.clone(),
                amount_decimal: req.amount_decimal.clone(),
                decimals: req.from_asset.decimals as i64,
            },
            estimated_out: model::AmountInfo {
                amount_base_units: resp.output_amount.clone(),
                amount_decimal: format_decimal(&resp.output_amount, out_decimals),
                decimals: out_decimals as i64,
            },
            // Go reads `estimatedGasUsedInUsd`, defaulting a null/absent field
            // to 0.0.
            estimated_gas_usd: resp.estimated_gas_used_in_usd.unwrap_or(0.0),
            price_impact_pct: 0.0,
            route: "fibrous".to_string(),
            source_url: SOURCE_URL.to_string(),
            fetched_at: self.fetched_at(),
        })
    }
}

/// The Fibrous `/{chain}/route` response projection (mirrors Go `routeResponse`).
#[derive(Debug, Default, Deserialize)]
struct RouteResponse {
    #[serde(default)]
    success: bool,
    #[serde(rename = "outputAmount", default)]
    output_amount: String,
    /// Nullable in the API; absent or `null` is treated as "no estimate" → 0.0.
    #[serde(rename = "estimatedGasUsedInUsd", default)]
    estimated_gas_used_in_usd: Option<f64>,
}

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for the `defi-providers::fibrous` module.
    //!
    //! Go source: `internal/providers/fibrous/client.go` + `client_test.go`.
    //! These ports re-express the Go `httptest` suite with `wiremock`
    //! (deterministic, offline). Every Go test case is covered:
    //!
    //!   * `TestQuoteSwap_Success`              -> F1
    //!   * `TestQuoteSwap_UnsupportedChain`     -> F2
    //!   * `TestQuoteSwap_RejectsExactOutput`   -> F3
    //!   * `TestQuoteSwap_MonadDisabled`        -> F4
    //!   * `TestQuoteSwap_APIError`             -> F5
    //!   * `TestQuoteSwap_HyperEVM`             -> F6
    //!   * `TestQuoteSwap_NullEstimatedGasUSD`  -> F7
    //!   * `TestInfo`                           -> F8
    //!
    //! The Rust port is "correct" iff:
    //!
    //!  F1. A successful `/base/route` response is parsed: provider `fibrous`,
    //!      trade type `exact-input`, chain `eip155:8453`, input base units
    //!      echoed, output base units from `outputAmount`, gas USD from
    //!      `estimatedGasUsedInUsd`, non-empty `fetched_at`. The request carries
    //!      the `amount`, `tokenInAddress`, and `tokenOutAddress` query params.
    //!
    //!  F2. A quote on a chain absent from the slug map (ethereum) is rejected
    //!      as `Unsupported` WITHOUT a network call.
    //!
    //!  F3. A quote with `--type exact-output` is rejected as `Unsupported`
    //!      WITHOUT a network call.
    //!
    //!  F4. A quote on monad (143, absent from the slug map) is rejected as
    //!      `Unsupported` WITHOUT a network call.
    //!
    //!  F5. A `success=false` response is rejected as `Unavailable`.
    //!
    //!  F6. A successful `/hyperevm/route` response resolves chain `eip155:999`
    //!      and parses the output amount.
    //!
    //!  F7. A response with `estimatedGasUsedInUsd: null` yields a zero gas-USD
    //!      estimate.
    //!
    //!  F8. `Provider::info` reports `fibrous`, `swap`, no key required, and at
    //!      least one capability.
    //!
    //! Additional spec-driven coverage (not 1:1 with a Go test but contract-
    //! relevant): the missing-`outputAmount` → `Unavailable` path and the
    //! `chain_slug` pure-helper mapping.

    use super::*;
    use std::time::Duration;

    use chrono::TimeZone;
    use defi_id::{parse_asset, parse_chain, Asset, Chain};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::traits::Provider as _;

    const USDC_BASE: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
    const WETH_BASE: &str = "0x4200000000000000000000000000000000000006";

    fn http() -> HttpClient {
        HttpClient::new(Duration::from_secs(2), 0)
    }

    fn client() -> Client {
        let mut c = Client::new(http());
        c.set_now(Utc.with_ymd_and_hms(2026, 5, 28, 12, 0, 0).unwrap());
        c
    }

    fn asset_by_address(address: &str, chain: &Chain) -> Asset {
        parse_asset(address, chain).unwrap_or_else(|_| panic!("parse asset {address}"))
    }

    fn req(chain: Chain, from: Asset, to: Asset, trade_type: SwapTradeType) -> SwapQuoteRequest {
        SwapQuoteRequest {
            chain,
            from_asset: from,
            to_asset: to,
            amount_base_units: "1000000".to_string(),
            amount_decimal: "1".to_string(),
            trade_type,
            ..Default::default()
        }
    }

    // ----- F1: parse a successful Base response ---------------------------
    #[tokio::test]
    async fn quote_swap_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/base/route"))
            .and(query_param("amount", "1000000"))
            .and(query_param("tokenInAddress", USDC_BASE))
            .and(query_param("tokenOutAddress", WETH_BASE))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "success": true,
                    "outputAmount": "471974940000000000",
                    "estimatedGasUsedInUsd": 0.05,
                    "inputToken": {"address": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913", "decimals": 6},
                    "outputToken": {"address": "0x4200000000000000000000000000000000000006", "decimals": 18}
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = parse_chain("base").expect("parse base");
        let from = asset_by_address(USDC_BASE, &chain);
        let to = asset_by_address(WETH_BASE, &chain);

        let mut c = client();
        c.set_base_url(&server.uri());
        let quote = c
            .quote_swap(req(chain, from, to, SwapTradeType::ExactInput))
            .await
            .expect("quote_swap");

        assert_eq!(quote.provider, "fibrous");
        assert_eq!(quote.trade_type, "exact-input");
        assert_eq!(quote.chain_id, "eip155:8453");
        assert_eq!(quote.input_amount.amount_base_units, "1000000");
        assert_eq!(quote.estimated_out.amount_base_units, "471974940000000000");
        assert_eq!(quote.estimated_gas_usd, 0.05);
        assert!(!quote.fetched_at.is_empty());
    }

    // ----- F2: unsupported chain rejected (no network call) ---------------
    #[tokio::test]
    async fn quote_swap_unsupported_chain() {
        let chain = parse_chain("ethereum").expect("parse ethereum");
        let from = asset_by_address("USDC", &chain);
        let to = asset_by_address("WETH", &chain);
        // No mock server: must fail before any HTTP I/O.
        let err = client()
            .quote_swap(req(chain, from, to, SwapTradeType::ExactInput))
            .await
            .expect_err("expected unsupported chain error");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- F3: exact-output rejected (no network call) --------------------
    #[tokio::test]
    async fn quote_swap_rejects_exact_output() {
        let chain = parse_chain("base").expect("parse base");
        let from = asset_by_address(USDC_BASE, &chain);
        let to = asset_by_address(WETH_BASE, &chain);
        let err = client()
            .quote_swap(req(chain, from, to, SwapTradeType::ExactOutput))
            .await
            .expect_err("expected unsupported exact-output error");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- F4: monad disabled (no network call) ---------------------------
    #[tokio::test]
    async fn quote_swap_monad_disabled() {
        let chain = parse_chain("monad").expect("parse monad");
        let from = asset_by_address("USDC", &chain);
        let to = asset_by_address("WMON", &chain);
        let err = client()
            .quote_swap(req(chain, from, to, SwapTradeType::ExactInput))
            .await
            .expect_err("expected unsupported chain error for monad");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- F5: success=false response rejected ----------------------------
    #[tokio::test]
    async fn quote_swap_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/base/route"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"success": false}"#, "application/json"),
            )
            .mount(&server)
            .await;

        let chain = parse_chain("base").expect("parse base");
        let from = asset_by_address(USDC_BASE, &chain);
        let to = asset_by_address(WETH_BASE, &chain);
        let mut c = client();
        c.set_base_url(&server.uri());
        let err = c
            .quote_swap(req(chain, from, to, SwapTradeType::ExactInput))
            .await
            .expect_err("expected error for success=false response");
        assert_eq!(err.code, Code::Unavailable);
    }

    // ----- F6: HyperEVM route ---------------------------------------------
    #[tokio::test]
    async fn quote_swap_hyperevm() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/hyperevm/route"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "success": true,
                    "outputAmount": "998000000000000000",
                    "estimatedGasUsedInUsd": 0.001
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = parse_chain("hyperevm").expect("parse hyperevm");
        let from = Asset {
            chain_id: "eip155:999".to_string(),
            asset_id: "eip155:999/erc20:0x5555555555555555555555555555555555555555".to_string(),
            address: "0x5555555555555555555555555555555555555555".to_string(),
            symbol: String::new(),
            decimals: 18,
        };
        let to = Asset {
            chain_id: "eip155:999".to_string(),
            asset_id: "eip155:999/erc20:0x6666666666666666666666666666666666666666".to_string(),
            address: "0x6666666666666666666666666666666666666666".to_string(),
            symbol: String::new(),
            decimals: 18,
        };

        let mut c = client();
        c.set_base_url(&server.uri());
        let quote = c
            .quote_swap(SwapQuoteRequest {
                chain,
                from_asset: from,
                to_asset: to,
                amount_base_units: "1000000000000000000".to_string(),
                amount_decimal: "1".to_string(),
                trade_type: SwapTradeType::ExactInput,
                ..Default::default()
            })
            .await
            .expect("quote_swap hyperevm");

        assert_eq!(quote.chain_id, "eip155:999");
        assert_eq!(quote.estimated_out.amount_base_units, "998000000000000000");
    }

    // ----- F7: null estimatedGasUsedInUsd -> 0 ----------------------------
    #[tokio::test]
    async fn quote_swap_null_estimated_gas_usd() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/base/route"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"success": true, "outputAmount": "1234567", "estimatedGasUsedInUsd": null}"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let chain = parse_chain("base").expect("parse base");
        let from = asset_by_address(USDC_BASE, &chain);
        let to = asset_by_address(WETH_BASE, &chain);
        let mut c = client();
        c.set_base_url(&server.uri());
        let quote = c
            .quote_swap(req(chain, from, to, SwapTradeType::ExactInput))
            .await
            .expect("quote_swap");
        assert_eq!(quote.estimated_gas_usd, 0.0);
    }

    // ----- missing outputAmount -> Unavailable ----------------------------
    #[tokio::test]
    async fn quote_swap_rejects_missing_output_amount() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/base/route"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(r#"{"success": true}"#, "application/json"),
            )
            .mount(&server)
            .await;

        let chain = parse_chain("base").expect("parse base");
        let from = asset_by_address(USDC_BASE, &chain);
        let to = asset_by_address(WETH_BASE, &chain);
        let mut c = client();
        c.set_base_url(&server.uri());
        let err = c
            .quote_swap(req(chain, from, to, SwapTradeType::ExactInput))
            .await
            .expect_err("expected missing-output-amount error");
        assert_eq!(err.code, Code::Unavailable);
    }

    // ----- F8: metadata ---------------------------------------------------
    #[test]
    fn info_is_metadata_only_no_key_required() {
        let info = Provider::info(&Client::new(http()));
        assert_eq!(info.name, "fibrous");
        assert_eq!(info.provider_type, "swap");
        assert!(!info.requires_key);
        assert!(!info.capabilities.is_empty());
    }

    // ----- pure helper: chain_slug mapping --------------------------------
    #[test]
    fn chain_slug_maps_supported_chains() {
        assert_eq!(chain_slug(8453), Some("base"));
        assert_eq!(chain_slug(4114), Some("citrea"));
        assert_eq!(chain_slug(999), Some("hyperevm"));
        assert_eq!(chain_slug(1), None);
        assert_eq!(chain_slug(143), None);
    }
}
