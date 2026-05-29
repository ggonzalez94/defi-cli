//! LiFi bridge provider adapter.
//!
//! Go source: `internal/providers/lifi/client.go` (+ `client_test.go`).
//!
//! Implements the [`BridgeProvider`] (quote) + [`BridgeActionBuilder`]
//! (executable action) trait surfaces, plus [`Provider`] metadata. Both the
//! quote and the executable action are built from the single LiFi `/quote`
//! endpoint (GET). Numeric amounts are kept as base-unit + decimal strings (the
//! machine contract); transaction values are normalized to canonical decimal
//! big-int strings.
//!
//! Unlike Across (which returns its approval transactions inline), LiFi only
//! reports an `approvalAddress`; the executable-action build performs an on-chain
//! `allowance(owner, spender)` read via the source-chain RPC and prepends an
//! `approve` step only when the current allowance is below the input amount.

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_evm::abi::Function;
use defi_evm::address;
use defi_evm::rpc::{CallRequest, RpcClient};
use defi_execution::{Action, ActionStep, Constraints, StepStatus, StepType};
use defi_execution::{BridgeActionBuilder, BridgeExecutionOptions, BridgeQuoteRequest};
use defi_httpx::Client as HttpClient;
use defi_id::format_decimal;
use defi_model as model;
use defi_registry::{resolve_rpc_url, ERC20_MINIMAL_ABI, LIFI_BASE_URL, LIFI_SETTLEMENT_URL};
use num_bigint::BigInt;
use reqwest::{Method, Request, Url};
use serde::Deserialize;

use crate::traits::{BridgeExecutionProvider, BridgeProvider, Provider};

/// Default LiFi quote/execution API base (`https://li.quest/v1`).
const DEFAULT_BASE: &str = LIFI_BASE_URL;
/// Deterministic placeholder sender used for quote-only mode (matches Go).
const QUOTE_PLACEHOLDER_SENDER: &str = "0x0000000000000000000000000000000000000001";
/// The zero address sentinel.
const ZERO_ADDRESS: &str = "0x0000000000000000000000000000000000000000";
/// The conventional native-token marker address.
const NATIVE_MARKER_ADDRESS: &str = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

/// LiFi bridge adapter (mirrors Go `lifi.Client`).
pub struct Client {
    http: HttpClient,
    base_url: String,
}

impl Client {
    /// Build a client with the default LiFi API base (mirrors Go `New`).
    pub fn new(http: HttpClient) -> Self {
        Client {
            http,
            base_url: DEFAULT_BASE.to_string(),
        }
    }

    /// Override the API base URL (test seam for Go `baseURL`).
    pub fn set_base_url(&mut self, base: &str) {
        self.base_url = base.to_string();
    }

    /// Build a GET request to `url`, mapping a parse failure onto an internal
    /// error with `ctx`.
    fn build_get(&self, url: &str, ctx: &'static str) -> Result<Request, Error> {
        let parsed = Url::parse(url).map_err(|e| Error::wrap(Code::Internal, ctx, e))?;
        Ok(Request::new(Method::GET, parsed))
    }

    /// The current RFC3339 UTC timestamp (seconds precision, trailing `Z`),
    /// matching Go `time.Now().UTC().Format(time.RFC3339)`.
    fn now_rfc3339() -> String {
        Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
    }
}

impl Provider for Client {
    fn info(&self) -> model::ProviderInfo {
        model::ProviderInfo {
            name: "lifi".to_string(),
            provider_type: "bridge".to_string(),
            requires_key: false,
            capabilities: vec![
                "bridge.quote".to_string(),
                "bridge.plan".to_string(),
                "bridge.execute".to_string(),
            ],
            key_env_var_name: String::new(),
            capability_auth: Vec::new(),
        }
    }
}

// =============================================================================
// LiFi `/quote` response shape (mirrors Go `quoteResponse` / `quoteStep`).
// =============================================================================

#[derive(Debug, Default, Deserialize)]
struct QuoteResponse {
    #[serde(default)]
    id: String,
    #[serde(default)]
    estimate: QuoteEstimate,
    #[serde(rename = "toolDetails", default)]
    tool_details: ToolDetails,
    #[serde(default)]
    tool: String,
    #[serde(rename = "includedSteps", default)]
    included_steps: Vec<QuoteStep>,
    #[serde(rename = "transactionRequest", default)]
    transaction_request: TransactionRequest,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteEstimate {
    #[serde(rename = "toAmount", default)]
    to_amount: String,
    #[serde(rename = "toAmountMin", default)]
    to_amount_min: String,
    #[serde(rename = "approvalAddress", default)]
    approval_address: String,
    #[serde(rename = "feeCosts", default)]
    fee_costs: Vec<AmountUsd>,
    #[serde(rename = "gasCosts", default)]
    gas_costs: Vec<AmountUsd>,
    #[serde(rename = "executionDuration", default)]
    execution_duration: i64,
}

#[derive(Debug, Default, Deserialize)]
struct AmountUsd {
    #[serde(rename = "amountUSD", default)]
    amount_usd: String,
}

#[derive(Debug, Default, Deserialize)]
struct ToolDetails {
    #[serde(default)]
    key: String,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteStep {
    #[serde(default)]
    action: QuoteStepAction,
    #[serde(default)]
    estimate: QuoteStepEstimate,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteStepAction {
    #[serde(rename = "toChainId", default)]
    to_chain_id: i64,
    #[serde(rename = "toToken", default)]
    to_token: QuoteStepToken,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteStepToken {
    #[serde(default)]
    address: String,
    #[serde(default)]
    decimals: i32,
}

#[derive(Debug, Default, Deserialize)]
struct QuoteStepEstimate {
    #[serde(rename = "toAmount", default)]
    to_amount: String,
}

#[derive(Debug, Default, Deserialize)]
struct TransactionRequest {
    #[serde(default)]
    to: String,
    #[serde(default)]
    data: String,
    #[serde(default)]
    value: String,
    #[serde(rename = "chainId", default)]
    chain_id: i64,
}

#[async_trait]
impl BridgeProvider for Client {
    async fn quote_bridge(&self, req: BridgeQuoteRequest) -> Result<model::BridgeQuote, Error> {
        if !req.from_chain.is_evm() || !req.to_chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "lifi bridge quotes support only EVM chains",
            ));
        }

        let from_amount_for_gas = normalize_optional_base_units(&req.from_amount_for_gas)
            .map_err(|e| Error::wrap(Code::Usage, "parse bridge gas reserve amount", e))?;

        let url = self.quote_url(&req, QUOTE_PLACEHOLDER_SENDER, "", &from_amount_for_gas)?;
        let h_req = self.build_get(&url, "build lifi quote request")?;
        let resp = self.http.do_json::<QuoteResponse>(h_req).await?.value;

        if resp.estimate.to_amount.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "lifi quote missing output amount",
            ));
        }

        let mut protocol_fee_usd = 0.0;
        for item in &resp.estimate.fee_costs {
            protocol_fee_usd += parse_usd(&item.amount_usd);
        }
        let mut gas_fee_usd = 0.0;
        for item in &resp.estimate.gas_costs {
            gas_fee_usd += parse_usd(&item.amount_usd);
        }
        let fee_usd = protocol_fee_usd + gas_fee_usd;

        let route = if resp.tool_details.name.is_empty() {
            format!("{}->{}", req.from_chain.slug, req.to_chain.slug)
        } else {
            resp.tool_details.name.clone()
        };

        let native_estimate =
            destination_native_estimate(&resp.included_steps, req.to_chain.evm_chain_id);

        let fee_breakdown = build_fee_breakdown(protocol_fee_usd, gas_fee_usd, fee_usd);

        Ok(model::BridgeQuote {
            provider: "lifi".to_string(),
            from_chain_id: req.from_chain.caip2.clone(),
            to_chain_id: req.to_chain.caip2.clone(),
            from_asset_id: req.from_asset.asset_id.clone(),
            to_asset_id: req.to_asset.asset_id.clone(),
            input_amount: model::AmountInfo {
                amount_base_units: req.amount_base_units.clone(),
                amount_decimal: req.amount_decimal.clone(),
                decimals: req.from_asset.decimals as i64,
            },
            from_amount_for_gas,
            estimated_destination_native: native_estimate,
            estimated_out: model::AmountInfo {
                amount_base_units: resp.estimate.to_amount.clone(),
                amount_decimal: format_decimal(&resp.estimate.to_amount, req.to_asset.decimals),
                decimals: req.to_asset.decimals as i64,
            },
            estimated_fee_usd: fee_usd,
            fee_breakdown,
            estimated_time_s: resp.estimate.execution_duration,
            route,
            source_url: "https://li.quest".to_string(),
            fetched_at: Self::now_rfc3339(),
        })
    }
}

impl Client {
    /// Build the LiFi `/quote` endpoint URL with the shared query parameters
    /// (mirrors the Go `url.Values` construction). When `to_address` is empty the
    /// `toAddress` param is omitted (quote-only mode); `slippage` is the caller's
    /// already-formatted fractional string.
    fn quote_url(
        &self,
        req: &BridgeQuoteRequest,
        from_address: &str,
        to_address: &str,
        from_amount_for_gas: &str,
    ) -> Result<String, Error> {
        self.quote_url_with_slippage(req, from_address, to_address, "0.005", from_amount_for_gas)
    }

    /// Build the LiFi `/quote` URL with an explicit slippage string and
    /// optionally lower-cased token addresses (the execution path lower-cases
    /// `fromToken`/`toToken`, matching Go).
    fn quote_url_with_slippage(
        &self,
        req: &BridgeQuoteRequest,
        from_address: &str,
        to_address: &str,
        slippage: &str,
        from_amount_for_gas: &str,
    ) -> Result<String, Error> {
        let lowercase_tokens = !to_address.is_empty();
        let from_token = if lowercase_tokens {
            req.from_asset.address.to_lowercase()
        } else {
            req.from_asset.address.clone()
        };
        let to_token = if lowercase_tokens {
            req.to_asset.address.to_lowercase()
        } else {
            req.to_asset.address.clone()
        };

        let mut url = Url::parse(&format!("{}/quote", self.base_url.trim_end_matches('/')))
            .map_err(|e| Error::wrap(Code::Internal, "build lifi endpoint url", e))?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("fromChain", &req.from_chain.evm_chain_id.to_string())
                .append_pair("toChain", &req.to_chain.evm_chain_id.to_string())
                .append_pair("fromToken", &from_token)
                .append_pair("toToken", &to_token)
                .append_pair("fromAmount", &req.amount_base_units)
                .append_pair("slippage", slippage)
                .append_pair("fromAddress", from_address);
            if !to_address.is_empty() {
                pairs.append_pair("toAddress", to_address);
            }
            if !from_amount_for_gas.is_empty() {
                pairs.append_pair("fromAmountForGas", from_amount_for_gas);
            }
        }
        Ok(url.to_string())
    }
}

#[async_trait]
impl BridgeActionBuilder for Client {
    async fn build_bridge_action(
        &self,
        req: BridgeQuoteRequest,
        opts: BridgeExecutionOptions,
    ) -> Result<Action, Error> {
        let sender = opts.sender.trim().to_string();
        if sender.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "bridge execution requires sender address",
            ));
        }
        if !address::is_hex_address(&sender) {
            return Err(Error::new(
                Code::Usage,
                "bridge execution sender must be a valid EVM address",
            ));
        }
        let mut recipient = opts.recipient.trim().to_string();
        if recipient.is_empty() {
            recipient = sender.clone();
        }
        if !address::is_hex_address(&recipient) {
            return Err(Error::new(
                Code::Usage,
                "bridge execution recipient must be a valid EVM address",
            ));
        }
        if !address::is_hex_address(&req.from_asset.address)
            || !address::is_hex_address(&req.to_asset.address)
        {
            return Err(Error::new(
                Code::Usage,
                "bridge execution requires ERC20 token addresses for from/to assets",
            ));
        }
        let mut slippage_bps = opts.slippage_bps;
        if slippage_bps <= 0 {
            slippage_bps = 50;
        }
        if slippage_bps >= 10_000 {
            return Err(Error::new(
                Code::Usage,
                "slippage bps must be less than 10000",
            ));
        }

        let from_amount_for_gas = normalize_optional_base_units(&first_non_empty(&[
            &opts.from_amount_for_gas,
            &req.from_amount_for_gas,
        ]))
        .map_err(|e| Error::wrap(Code::Usage, "parse bridge gas reserve amount", e))?;

        let url = self.quote_url_with_slippage(
            &req,
            &sender,
            &recipient,
            &format_slippage(slippage_bps),
            &from_amount_for_gas,
        )?;
        let h_req = self.build_get(&url, "build lifi execution quote request")?;
        let resp = self.http.do_json::<QuoteResponse>(h_req).await?.value;

        if resp.transaction_request.to.trim().is_empty()
            || resp.transaction_request.data.trim().is_empty()
        {
            return Err(Error::new(
                Code::Unavailable,
                "lifi quote missing executable transaction payload",
            ));
        }
        if !address::is_hex_address(resp.transaction_request.to.trim()) {
            return Err(Error::new(
                Code::ActionPlan,
                "lifi transaction target is not a valid EVM address",
            ));
        }
        if resp.transaction_request.chain_id != 0
            && resp.transaction_request.chain_id != req.from_chain.evm_chain_id
        {
            return Err(Error::new(
                Code::ActionPlan,
                "lifi transaction chain does not match source chain",
            ));
        }
        let target = address::checksum(resp.transaction_request.to.trim())
            .map_err(|e| Error::wrap(Code::ActionPlan, "checksum lifi transaction target", e))?;

        let rpc_url = resolve_rpc_url(&opts.rpc_url, req.from_chain.evm_chain_id)
            .map_err(|e| Error::wrap(Code::Usage, "resolve rpc url", e))?;
        let native_estimate =
            destination_native_estimate(&resp.included_steps, req.to_chain.evm_chain_id);

        let mut action = Action::new(
            defi_execution::new_action_id(),
            "bridge",
            req.from_chain.caip2.clone(),
            Constraints {
                slippage_bps,
                deadline: String::new(),
                simulate: opts.simulate,
            },
        );
        action.provider = "lifi".to_string();
        action.from_address = sender.clone();
        action.to_address = recipient.clone();
        action.input_amount = req.amount_base_units.clone();

        let mut metadata = serde_json::Map::new();
        metadata.insert("to_chain_id".into(), req.to_chain.caip2.clone().into());
        metadata.insert(
            "from_asset_id".into(),
            req.from_asset.asset_id.clone().into(),
        );
        metadata.insert("to_asset_id".into(), req.to_asset.asset_id.clone().into());
        metadata.insert(
            "route".into(),
            first_non_empty(&[&resp.tool_details.name, &resp.tool]).into(),
        );
        metadata.insert(
            "approval_spender".into(),
            resp.estimate.approval_address.trim().into(),
        );
        if !from_amount_for_gas.is_empty() {
            metadata.insert(
                "from_amount_for_gas".into(),
                from_amount_for_gas.clone().into(),
            );
        }
        if let Some(native) = &native_estimate {
            metadata.insert(
                "estimated_destination_native_base_units".into(),
                native.amount_base_units.clone().into(),
            );
        }
        action.metadata = Some(metadata);

        if should_add_approval(&req.from_asset.address, &resp.estimate.approval_address) {
            if !address::is_hex_address(&resp.estimate.approval_address) {
                return Err(Error::new(
                    Code::ActionPlan,
                    "lifi quote returned invalid approval address",
                ));
            }
            let approve_data = self
                .resolve_approval(&req, &sender, &resp.estimate.approval_address, &rpc_url)
                .await?;
            if let Some(data) = approve_data {
                let token_target = address::checksum(&req.from_asset.address)
                    .map_err(|e| Error::wrap(Code::Usage, "checksum source token", e))?;
                action.steps.push(ActionStep {
                    step_id: "approve-bridge-token".to_string(),
                    step_type: StepType::Approval,
                    status: StepStatus::Pending,
                    chain_id: req.from_chain.caip2.clone(),
                    rpc_url: rpc_url.clone(),
                    description: "Approve bridge spender for source token".to_string(),
                    target: token_target,
                    data: ensure_hex_prefix(&data),
                    value: "0".to_string(),
                    calls: Vec::new(),
                    expected_outputs: None,
                    tx_hash: String::new(),
                    error: String::new(),
                });
            }
        }

        let bridge_value = hex_to_decimal(&resp.transaction_request.value)
            .map_err(|e| Error::wrap(Code::ActionPlan, "parse bridge transaction value", e))?;

        let mut expected_outputs = serde_json::Map::new();
        expected_outputs.insert(
            "to_amount_min".into(),
            first_non_empty(&[&resp.estimate.to_amount_min, &resp.estimate.to_amount]).into(),
        );
        expected_outputs.insert("settlement_provider".into(), "lifi".into());
        expected_outputs.insert(
            "settlement_status_endpoint".into(),
            LIFI_SETTLEMENT_URL.into(),
        );
        expected_outputs.insert(
            "settlement_bridge".into(),
            first_non_empty(&[&resp.tool_details.key, &resp.tool]).into(),
        );
        expected_outputs.insert(
            "settlement_from_chain".into(),
            req.from_chain.evm_chain_id.to_string().into(),
        );
        expected_outputs.insert(
            "settlement_to_chain".into(),
            req.to_chain.evm_chain_id.to_string().into(),
        );
        expected_outputs.insert(
            "settlement_quote_response_id".into(),
            resp.id.clone().into(),
        );
        if let Some(native) = &native_estimate {
            expected_outputs.insert(
                "destination_native_estimated".into(),
                native.amount_base_units.clone().into(),
            );
        }

        action.steps.push(ActionStep {
            step_id: "bridge-transfer".to_string(),
            step_type: StepType::Bridge,
            status: StepStatus::Pending,
            chain_id: req.from_chain.caip2.clone(),
            rpc_url,
            description: "Bridge transfer via LiFi route".to_string(),
            target,
            data: ensure_hex_prefix(&resp.transaction_request.data),
            value: bridge_value,
            calls: Vec::new(),
            expected_outputs: Some(expected_outputs),
            tx_hash: String::new(),
            error: String::new(),
        });

        Ok(action)
    }
}

impl Client {
    /// Read the on-chain ERC-20 allowance the bridge spender currently holds and,
    /// when it is below the input amount, return the `approve(spender, amount)`
    /// calldata to prepend as an approval step. Returns `Ok(None)` when the
    /// current allowance already covers the amount.
    ///
    /// Mirrors the Go allowance-read branch of `BuildBridgeAction`.
    async fn resolve_approval(
        &self,
        req: &BridgeQuoteRequest,
        sender: &str,
        spender: &str,
        rpc_url: &str,
    ) -> Result<Option<String>, Error> {
        let client = RpcClient::connect(rpc_url)
            .map_err(|e| Error::wrap(Code::Unavailable, "connect source chain rpc", e))?;

        let amount_in = BigInt::parse_bytes(req.amount_base_units.as_bytes(), 10)
            .ok_or_else(|| Error::new(Code::Usage, "invalid amount base units"))?;

        let token_addr = address::parse(&req.from_asset.address)?;
        let owner_addr = address::parse(sender)?;
        let spender_addr = address::parse(spender)?;

        let erc20 = Function::from_abi_json(ERC20_MINIMAL_ABI, "allowance")?;
        let allowance_data = erc20
            .encode(&[
                alloy::dyn_abi::DynSolValue::Address(owner_addr.into_inner()),
                alloy::dyn_abi::DynSolValue::Address(spender_addr.into_inner()),
            ])
            .map_err(|e| Error::wrap(Code::Internal, "pack allowance call", e))?;

        let request = CallRequest::new(
            Some(owner_addr),
            Some(token_addr),
            alloy::primitives::U256::ZERO,
            allowance_data,
        );
        let allowance_raw = client
            .call(&request)
            .await
            .map_err(|e| Error::wrap(Code::Unavailable, "read allowance", e))?;
        let decoded = erc20
            .decode_output(&allowance_raw)
            .map_err(|e| Error::wrap(Code::Unavailable, "decode allowance", e))?;
        let current = decoded
            .first()
            .and_then(|v| v.as_uint())
            .map(|(v, _)| v)
            .ok_or_else(|| Error::new(Code::Unavailable, "invalid allowance response type"))?;
        let current_allowance =
            BigInt::from_bytes_be(num_bigint::Sign::Plus, &current.to_be_bytes::<32>());

        if current_allowance >= amount_in {
            return Ok(None);
        }

        let approve = Function::from_abi_json(ERC20_MINIMAL_ABI, "approve")?;
        let amount_u256 = alloy::primitives::U256::from_str_radix(&req.amount_base_units, 10)
            .map_err(|e| Error::wrap(Code::Usage, "parse approve amount", to_std_err(e)))?;
        let approve_data = approve
            .encode(&[
                alloy::dyn_abi::DynSolValue::Address(spender_addr.into_inner()),
                alloy::dyn_abi::DynSolValue::Uint(amount_u256, 256),
            ])
            .map_err(|e| Error::wrap(Code::Internal, "pack approve calldata", e))?;
        Ok(Some(format!("0x{}", hex::encode(approve_data))))
    }
}

impl BridgeExecutionProvider for Client {}

// =============================================================================
// Helpers (mirror Go free functions).
// =============================================================================

/// Build the optional [`model::BridgeFeeBreakdown`] from LiFi's split protocol /
/// gas USD costs (mirrors the Go `feeBreakdown` assembly): a relayer-fee entry is
/// emitted when the protocol fee is positive, a gas-fee entry when the gas fee is
/// positive, and `None` is returned when both are zero.
fn build_fee_breakdown(
    protocol_fee_usd: f64,
    gas_fee_usd: f64,
    total_fee_usd: f64,
) -> Option<model::BridgeFeeBreakdown> {
    let relayer_fee = if protocol_fee_usd > 0.0 {
        Some(model::FeeAmount {
            amount_base_units: String::new(),
            amount_decimal: String::new(),
            amount_usd: protocol_fee_usd,
        })
    } else {
        None
    };
    let gas_fee = if gas_fee_usd > 0.0 {
        Some(model::FeeAmount {
            amount_base_units: String::new(),
            amount_decimal: String::new(),
            amount_usd: gas_fee_usd,
        })
    } else {
        None
    };
    if relayer_fee.is_none() && gas_fee.is_none() {
        return None;
    }
    Some(model::BridgeFeeBreakdown {
        lp_fee: None,
        relayer_fee,
        gas_fee,
        total_fee_base_units: String::new(),
        total_fee_decimal: String::new(),
        total_fee_usd,
        consistent_with_amount_delta: None,
    })
}

/// Whether an approval step should be considered for `token`/`spender` (mirrors
/// Go `shouldAddApproval`): both must be valid, non-empty addresses and the
/// token must not be the zero address.
fn should_add_approval(token_addr: &str, spender: &str) -> bool {
    let token = token_addr.trim();
    let spender = spender.trim();
    if token.is_empty() || spender.is_empty() {
        return false;
    }
    if !address::is_hex_address(token) || !address::is_hex_address(spender) {
        return false;
    }
    !address::eq_fold(token, ZERO_ADDRESS)
}

/// Pull the destination native-token estimate out of the LiFi `includedSteps`
/// (mirrors Go `destinationNativeEstimate`): the first step targeting the
/// destination chain whose `toToken` is a native marker with a non-empty amount.
fn destination_native_estimate(
    steps: &[QuoteStep],
    destination_chain_id: i64,
) -> Option<model::AmountInfo> {
    for step in steps {
        if step.action.to_chain_id != destination_chain_id {
            continue;
        }
        let addr = step.action.to_token.address.trim();
        if !is_native_token_address(addr) {
            continue;
        }
        let amount = step.estimate.to_amount.trim();
        if amount.is_empty() {
            continue;
        }
        let mut decimals = step.action.to_token.decimals;
        if decimals <= 0 {
            decimals = 18;
        }
        return Some(model::AmountInfo {
            amount_base_units: amount.to_string(),
            amount_decimal: format_decimal(amount, decimals),
            decimals: decimals as i64,
        });
    }
    None
}

/// Whether `addr` is one of the conventional native-token marker addresses
/// (mirrors Go `isNativeTokenAddress`).
fn is_native_token_address(addr: &str) -> bool {
    addr.eq_ignore_ascii_case(ZERO_ADDRESS) || addr.eq_ignore_ascii_case(NATIVE_MARKER_ADDRESS)
}

/// Parse a USD-amount string, treating non-numeric/empty values as `0` (mirrors
/// Go's `strconv.ParseFloat(item.AmountUSD, 64)` with the ignored error).
fn parse_usd(v: &str) -> f64 {
    v.trim().parse::<f64>().unwrap_or(0.0)
}

/// Normalize an optional base-units string (mirrors Go `normalizeOptionalBaseUnits`):
/// empty trims to empty; otherwise the value must be a positive integer or an
/// error is returned.
fn normalize_optional_base_units(v: &str) -> Result<String, Error> {
    let clean = v.trim();
    if clean.is_empty() {
        return Ok(String::new());
    }
    let amount = BigInt::parse_bytes(clean.as_bytes(), 10)
        .ok_or_else(|| Error::new(Code::Usage, "amount must be an integer base-unit value"))?;
    if amount.sign() != num_bigint::Sign::Plus {
        return Err(Error::new(Code::Usage, "amount must be greater than zero"));
    }
    Ok(amount.to_string())
}

/// Render a basis-points slippage as a fractional string with 6 decimals
/// (mirrors Go `formatSlippage`).
fn format_slippage(bps: i64) -> String {
    format!("{:.6}", bps as f64 / 10_000.0)
}

/// First trimmed non-empty value, returning the ORIGINAL (untrimmed) value
/// (mirrors Go `firstNonEmpty`, which returns the original slice element).
fn first_non_empty(values: &[&str]) -> String {
    for v in values {
        if !v.trim().is_empty() {
            return (*v).to_string();
        }
    }
    String::new()
}

/// Ensure a hex string carries a `0x` prefix (mirrors Go `ensureHexPrefix`).
fn ensure_hex_prefix(v: &str) -> String {
    let clean = v.trim();
    if clean.starts_with("0x") || clean.starts_with("0X") {
        clean.to_string()
    } else {
        format!("0x{clean}")
    }
}

/// Parse a `0x`-prefixed (or bare) hex value into a canonical decimal big-int
/// string (mirrors Go `hexToDecimal`): empty → `"0"`, invalid → error.
fn hex_to_decimal(v: &str) -> Result<String, Error> {
    let clean = v.trim();
    if clean.is_empty() {
        return Ok("0".to_string());
    }
    let body = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
        .unwrap_or(clean);
    match BigInt::parse_bytes(body.as_bytes(), 16) {
        Some(n) => Ok(n.to_string()),
        None => Err(Error::new(
            Code::ActionPlan,
            format!("invalid hex value {v:?}"),
        )),
    }
}

/// A concrete, `Send + Sync` std error carrying a display message, so foreign
/// error display text can be attached as a typed [`Error`] cause.
#[derive(Debug)]
struct MsgError(String);

impl std::fmt::Display for MsgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MsgError {}

fn to_std_err<E: std::fmt::Display>(e: E) -> MsgError {
    MsgError(e.to_string())
}

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for the `defi-providers::lifi` module.
    //!
    //! Go source: `internal/providers/lifi/{client.go,client_test.go}`. The
    //! adapter implements bridge QUOTE (`/quote`) and the executable bridge
    //! ACTION build (`/quote` + an on-chain `allowance` read). The Rust port is
    //! "correct" iff it preserves the machine-contract-relevant behavior the Go
    //! tests assert (all ported here via `wiremock`, offline + deterministic):
    //!
    //!  L1. QUOTE returns the provider `toAmount` as the estimated out and a
    //!      positive aggregated USD fee. (Ports Go `TestQuoteBridge`.)
    //!
    //!  L2. QUOTE rejects non-EVM chains with an error. (Ports Go
    //!      `TestQuoteBridgeRejectsNonEVMChains`.)
    //!
    //!  L3. QUOTE forwards `fromAmountForGas`, surfaces it on the quote, and
    //!      populates the destination native estimate from `includedSteps`.
    //!      (Ports Go `TestQuoteBridgeWithFromAmountForGas`.)
    //!
    //!  L4. ACTION build adds an approval step (when on-chain allowance is below
    //!      the amount) + a bridge step; the bridge step marks LiFi as the
    //!      settlement provider and carries a settlement status endpoint. (Ports
    //!      Go `TestBuildBridgeActionAddsApprovalStep`.)
    //!
    //!  L5. ACTION build skips the approval step when the spender is missing.
    //!      (Ports Go `TestBuildBridgeActionSkipsApprovalWhenSpenderMissing`.)
    //!
    //!  L6. ACTION build accepts a non-canonical (but valid) transaction target
    //!      at plan time (canonical-target validation is deferred to pre-sign).
    //!      (Ports Go `TestBuildBridgeActionAllowsNonCanonicalTransactionTargetAtPlanTime`.)
    //!
    //!  L7. ACTION build rejects an invalid transaction target address. (Ports
    //!      Go `TestBuildBridgeActionRejectsInvalidTransactionTarget`.)
    //!
    //! Go tests intentionally SKIPPED: none — every Go test case in
    //! `client_test.go` is ported above.

    use super::*;
    use std::time::Duration;

    use alloy::dyn_abi::DynSolValue;
    use alloy::primitives::U256;
    use defi_id::{parse_asset, parse_chain};
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::traits::Provider;

    fn http() -> HttpClient {
        HttpClient::new(Duration::from_secs(2), 0)
    }

    fn quote_req(from: &str, to: &str) -> BridgeQuoteRequest {
        let from_chain = parse_chain(from).expect("parse from chain");
        let to_chain = parse_chain(to).expect("parse to chain");
        let from_asset = parse_asset("USDC", &from_chain).expect("parse from asset");
        let to_asset = parse_asset("USDC", &to_chain).expect("parse to asset");
        BridgeQuoteRequest {
            from_chain,
            to_chain,
            from_asset,
            to_asset,
            amount_base_units: "1000000".to_string(),
            amount_decimal: "1".to_string(),
            from_amount_for_gas: String::new(),
        }
    }

    /// A canonical LiFi `/quote` body with the given approval address + tx `to`.
    fn quote_body(approval_address: &str, tx_to: &str) -> String {
        format!(
            r#"{{
                "id": "quote-id:0",
                "estimate": {{
                    "toAmount": "950000",
                    "toAmountMin": "940000",
                    "approvalAddress": "{approval_address}",
                    "feeCosts": [{{"amountUSD":"0.40"}}],
                    "gasCosts": [{{"amountUSD":"0.60"}}],
                    "executionDuration": 120
                }},
                "toolDetails": {{"key":"across","name":"across"}},
                "tool": "across",
                "includedSteps": [],
                "transactionRequest": {{
                    "to": "{tx_to}",
                    "from": "0x00000000000000000000000000000000000000AA",
                    "data": "0x1234",
                    "value": "0x0",
                    "chainId": 1
                }}
            }}"#
        )
    }

    /// Mount a LiFi `/quote` responder returning the canonical body.
    async fn mount_quote(server: &MockServer, approval_address: &str, tx_to: &str) {
        Mock::given(method("GET"))
            .and(path("/quote"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(quote_body(approval_address, tx_to), "application/json"),
            )
            .mount(server)
            .await;
    }

    /// Mount an `eth_call` responder returning the ABI-encoded `allowance` value.
    async fn mount_allowance(server: &MockServer, allowance: u128) {
        let func = Function::from_abi_json(ERC20_MINIMAL_ABI, "allowance").expect("allowance fn");
        // Build the 32-byte uint256 return word.
        let word = U256::from(allowance).to_be_bytes::<32>();
        let result = format!("0x{}", hex::encode(word));
        let _ = func; // function only needed to mirror the decode shape.
        Mock::given(method("POST"))
            .and(body_partial_json(json!({ "method": "eth_call" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": result,
            })))
            .mount(server)
            .await;
    }

    // ----- L1: quote returns provider output + positive fee ----------------
    #[tokio::test]
    async fn quote_bridge() {
        let server = MockServer::start().await;
        mount_quote(
            &server,
            "0x0000000000000000000000000000000000000ABC",
            "0x1231DeB6f5749EF6Ce6943a275A1D3E7486F4EaE",
        )
        .await;

        let mut client = Client::new(http());
        client.set_base_url(&server.uri());

        let quote = client
            .quote_bridge(quote_req("ethereum", "base"))
            .await
            .expect("quote_bridge");

        assert_eq!(quote.provider, "lifi");
        assert_eq!(quote.estimated_out.amount_base_units, "950000");
        assert!(
            quote.estimated_fee_usd > 0.0,
            "expected positive fee estimate, got {}",
            quote.estimated_fee_usd
        );
    }

    // ----- L2: non-EVM chains rejected -------------------------------------
    #[tokio::test]
    async fn quote_bridge_rejects_non_evm_chains() {
        let client = Client::new(http());
        let err = client
            .quote_bridge(quote_req("solana", "base"))
            .await
            .expect_err("expected unsupported chain error");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- L3: fromAmountForGas forwarded + destination native estimate -----
    #[tokio::test]
    async fn quote_bridge_with_from_amount_for_gas() {
        let server = MockServer::start().await;
        let body = r#"{
            "estimate": {
                "toAmount": "900000",
                "toAmountMin": "890000",
                "approvalAddress": "0x0000000000000000000000000000000000000ABC",
                "feeCosts": [{"amountUSD":"0.40"}],
                "gasCosts": [{"amountUSD":"0.60"}],
                "executionDuration": 45
            },
            "toolDetails": {"key":"across","name":"across"},
            "tool": "across",
            "includedSteps": [{
                "action": {
                    "toChainId": 8453,
                    "toToken": {"address":"0x0000000000000000000000000000000000000000","decimals":18}
                },
                "estimate": {"toAmount":"500000000000000"}
            }],
            "transactionRequest": {
                "to": "0x1231DeB6f5749EF6Ce6943a275A1D3E7486F4EaE",
                "from": "0x00000000000000000000000000000000000000AA",
                "data": "0x1234",
                "value": "0x0",
                "chainId": 1
            }
        }"#;
        Mock::given(method("GET"))
            .and(path("/quote"))
            .and(query_param("fromAmountForGas", "100000"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;

        let mut client = Client::new(http());
        client.set_base_url(&server.uri());

        let mut req = quote_req("ethereum", "base");
        req.from_amount_for_gas = "100000".to_string();
        let quote = client.quote_bridge(req).await.expect("quote_bridge");

        assert_eq!(quote.from_amount_for_gas, "100000");
        let native = quote
            .estimated_destination_native
            .expect("expected destination native estimate to be populated");
        assert_eq!(native.amount_base_units, "500000000000000");
    }

    // ----- L4: build bridge action adds approval step ----------------------
    #[tokio::test]
    async fn build_bridge_action_adds_approval_step() {
        let server = MockServer::start().await;
        mount_quote(
            &server,
            "0x0000000000000000000000000000000000000ABC",
            "0x1231DeB6f5749EF6Ce6943a275A1D3E7486F4EaE",
        )
        .await;
        // Allowance == 0 < amount => approval step is added.
        mount_allowance(&server, 0).await;

        let mut client = Client::new(http());
        client.set_base_url(&server.uri());

        let action = client
            .build_bridge_action(
                quote_req("ethereum", "base"),
                BridgeExecutionOptions {
                    sender: "0x00000000000000000000000000000000000000AA".to_string(),
                    recipient: "0x00000000000000000000000000000000000000BB".to_string(),
                    slippage_bps: 50,
                    simulate: true,
                    rpc_url: server.uri(),
                    from_amount_for_gas: String::new(),
                },
            )
            .await
            .expect("build_bridge_action");

        assert_eq!(action.intent_type, "bridge");
        assert_eq!(
            action.steps.len(),
            2,
            "expected approval + bridge steps, got {}",
            action.steps.len()
        );
        assert_eq!(action.steps[0].step_type, StepType::Approval);
        assert_eq!(action.steps[1].step_type, StepType::Bridge);
        let outs = action.steps[1]
            .expected_outputs
            .as_ref()
            .expect("bridge step expected outputs");
        assert_eq!(
            outs.get("settlement_provider").and_then(|v| v.as_str()),
            Some("lifi")
        );
        assert_eq!(
            outs.get("settlement_status_endpoint")
                .and_then(|v| v.as_str()),
            Some(LIFI_SETTLEMENT_URL)
        );
    }

    // ----- L5: skip approval when spender missing --------------------------
    #[tokio::test]
    async fn build_bridge_action_skips_approval_when_spender_missing() {
        let server = MockServer::start().await;
        // Empty approval address => no allowance read, no approval step.
        mount_quote(&server, "", "0x1231DeB6f5749EF6Ce6943a275A1D3E7486F4EaE").await;

        let mut client = Client::new(http());
        client.set_base_url(&server.uri());

        let action = client
            .build_bridge_action(
                quote_req("ethereum", "base"),
                BridgeExecutionOptions {
                    sender: "0x00000000000000000000000000000000000000AA".to_string(),
                    recipient: "0x00000000000000000000000000000000000000AA".to_string(),
                    slippage_bps: 0,
                    simulate: true,
                    rpc_url: "http://127.0.0.1:1".to_string(),
                    from_amount_for_gas: String::new(),
                },
            )
            .await
            .expect("build_bridge_action");

        assert_eq!(
            action.steps.len(),
            1,
            "expected bridge-only step, got {}",
            action.steps.len()
        );
        assert_eq!(action.steps[0].step_type, StepType::Bridge);
    }

    // ----- L6: non-canonical (valid) target accepted at plan time ----------
    #[tokio::test]
    async fn build_bridge_action_allows_non_canonical_transaction_target_at_plan_time() {
        let server = MockServer::start().await;
        // Empty approval address (skips allowance read) but a non-canonical,
        // still-valid target.
        mount_quote(&server, "", "0x1111111111111111111111111111111111111111").await;

        let mut client = Client::new(http());
        client.set_base_url(&server.uri());

        let action = client
            .build_bridge_action(
                quote_req("ethereum", "base"),
                BridgeExecutionOptions {
                    sender: "0x00000000000000000000000000000000000000AA".to_string(),
                    recipient: "0x00000000000000000000000000000000000000AA".to_string(),
                    slippage_bps: 0,
                    simulate: true,
                    rpc_url: "http://127.0.0.1:1".to_string(),
                    from_amount_for_gas: String::new(),
                },
            )
            .await
            .expect("expected plan-time target validation to be deferred");

        assert_eq!(action.steps.len(), 1);
        assert_eq!(
            action.steps[0].target,
            "0x1111111111111111111111111111111111111111"
        );
    }

    // ----- L7: invalid transaction target rejected -------------------------
    #[tokio::test]
    async fn build_bridge_action_rejects_invalid_transaction_target() {
        let server = MockServer::start().await;
        mount_quote(
            &server,
            "0x0000000000000000000000000000000000000ABC",
            "not-an-address",
        )
        .await;

        let mut client = Client::new(http());
        client.set_base_url(&server.uri());

        let err = client
            .build_bridge_action(
                quote_req("ethereum", "base"),
                BridgeExecutionOptions {
                    sender: "0x00000000000000000000000000000000000000AA".to_string(),
                    recipient: "0x00000000000000000000000000000000000000AA".to_string(),
                    slippage_bps: 0,
                    simulate: true,
                    rpc_url: "http://127.0.0.1:1".to_string(),
                    from_amount_for_gas: String::new(),
                },
            )
            .await
            .expect_err("expected invalid transaction target error");
        assert_eq!(err.code, Code::ActionPlan);
    }

    // ----- metadata: callable without a key --------------------------------
    #[test]
    fn info_is_bridge_metadata() {
        let client = Client::new(http());
        let info = client.info();
        assert_eq!(info.name, "lifi");
        assert_eq!(info.provider_type, "bridge");
        assert!(!info.requires_key);
        assert!(info.capabilities.iter().any(|c| c == "bridge.quote"));
        assert!(info.capabilities.iter().any(|c| c == "bridge.plan"));
        assert!(info.capabilities.iter().any(|c| c == "bridge.execute"));
    }

    // ----- helper unit checks ---------------------------------------------
    #[test]
    fn helpers_match_go_semantics() {
        assert_eq!(format_slippage(50), "0.005000");
        assert_eq!(hex_to_decimal("0x0").unwrap(), "0");
        assert_eq!(hex_to_decimal("0x10").unwrap(), "16");
        assert!(hex_to_decimal("0xzz").is_err());
        assert_eq!(normalize_optional_base_units("  ").unwrap(), "");
        assert_eq!(normalize_optional_base_units("100").unwrap(), "100");
        assert!(normalize_optional_base_units("0").is_err());
        assert!(normalize_optional_base_units("-1").is_err());
        assert!(should_add_approval(
            "0x000000000000000000000000000000000000DEAD",
            "0x0000000000000000000000000000000000000ABC"
        ));
        assert!(!should_add_approval(
            ZERO_ADDRESS,
            "0x0000000000000000000000000000000000000ABC"
        ));
        assert!(!should_add_approval(
            "0x000000000000000000000000000000000000DEAD",
            ""
        ));
        assert_eq!(first_non_empty(&["", "  ", "x"]), "x");
        // approve calldata referencing the function (kept reachable for parity).
        let approve = Function::from_abi_json(ERC20_MINIMAL_ABI, "approve").expect("approve fn");
        let data = approve
            .encode(&[
                DynSolValue::Address(
                    "0x0000000000000000000000000000000000000ABC"
                        .parse()
                        .unwrap(),
                ),
                DynSolValue::Uint(U256::from(1_000_000u64), 256),
            ])
            .expect("encode approve");
        assert_eq!(&data[..4], &[0x09, 0x5e, 0xa7, 0xb3]);
    }
}
