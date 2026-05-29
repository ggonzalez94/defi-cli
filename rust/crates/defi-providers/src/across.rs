//! Across bridge provider adapter.
//!
//! Go source: `internal/providers/across/client.go` (+ `client_test.go`).
//!
//! Implements the [`BridgeProvider`] (quote) + [`BridgeActionBuilder`]
//! (executable action) trait surfaces, plus [`Provider`] metadata. Numeric
//! amounts are kept as base-unit + decimal strings (machine contract);
//! transaction values are normalized to canonical decimal big-int strings.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_evm::address;
use defi_execution::{Action, ActionStep, Constraints, StepStatus, StepType};
use defi_execution::{BridgeActionBuilder, BridgeExecutionOptions, BridgeQuoteRequest};
use defi_httpx::Client as HttpClient;
use defi_id::format_decimal;
use defi_model as model;
use defi_registry::{resolve_rpc_url, ACROSS_BASE_URL, ACROSS_SETTLEMENT_URL};
use num_bigint::BigInt;
use reqwest::{Method, Request, Url};
use serde::Deserialize;
use serde_json::Value;

use crate::traits::{BridgeExecutionProvider, BridgeProvider, Provider};

/// Default Across API base (`https://app.across.to/api`).
const DEFAULT_BASE: &str = ACROSS_BASE_URL;

/// A free JSON object from the Across API (limits / suggested-fees).
type JsonMap = HashMap<String, Value>;

/// Across bridge adapter (mirrors Go `across.Client`).
pub struct Client {
    http: HttpClient,
    base_url: String,
}

impl Client {
    /// Build a client with the default Across API base (mirrors Go `New`).
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
            name: "across".to_string(),
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

#[async_trait]
impl BridgeProvider for Client {
    async fn quote_bridge(&self, req: BridgeQuoteRequest) -> Result<model::BridgeQuote, Error> {
        if !req.from_chain.is_evm() || !req.to_chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "across bridge quotes support only EVM chains",
            ));
        }
        let chain_from = req.from_chain.evm_chain_id.to_string();
        let chain_to = req.to_chain.evm_chain_id.to_string();

        let limits_url = self.endpoint_url("limits", &chain_from, &chain_to, &req)?;
        let limits_req = self.build_get(&limits_url, "build across limits request")?;
        let limits = self.http.do_json::<JsonMap>(limits_req).await?.value;

        if !check_amount_within_limits(&req.amount_base_units, &limits) {
            return Err(Error::new(
                Code::Usage,
                "amount is outside across bridge limits",
            ));
        }

        let fees_url = self.endpoint_url("suggested-fees", &chain_from, &chain_to, &req)?;
        let fees_req = self.build_get(&fees_url, "build across fees request")?;
        let fees = self.http.do_json::<JsonMap>(fees_req).await?.value;

        let fee_base_abs = pick_number_string(&fees, &["totalRelayFee", "relayFeeTotal"]);
        let has_absolute_fee = !fee_base_abs.trim().is_empty();
        let fee_base = if has_absolute_fee {
            fee_base_abs.clone()
        } else {
            "0".to_string()
        };

        let mut est_out = pick_number_string(&fees, &["outputAmount"]);
        let has_provider_output_amount = !est_out.trim().is_empty();
        if !has_provider_output_amount && has_absolute_fee {
            est_out = subtract_base_units(&req.amount_base_units, &fee_base);
        }
        if est_out.trim().is_empty() {
            est_out = req.amount_base_units.clone();
        }

        let mut fee_usd = pick_float(&fees, &["totalRelayFeeUsd", "feeUsd"]);
        if fee_usd == 0.0 && has_absolute_fee {
            fee_usd =
                approximate_stable_usd(&req.from_asset.symbol, &fee_base, req.from_asset.decimals);
        }
        let mut est_time = pick_float(&fees, &["estimatedFillTimeSec", "estimatedFillTime"]) as i64;
        if est_time == 0 {
            est_time = 120;
        }

        let fee_breakdown = build_across_fee_breakdown(
            &req,
            &fees,
            &fee_base_abs,
            &est_out,
            fee_usd,
            has_provider_output_amount,
        );

        Ok(model::BridgeQuote {
            provider: "across".to_string(),
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
                amount_base_units: est_out.clone(),
                amount_decimal: format_decimal(&est_out, req.to_asset.decimals),
                decimals: req.to_asset.decimals as i64,
            },
            estimated_fee_usd: fee_usd,
            fee_breakdown,
            estimated_time_s: est_time,
            route: format!("{}->{}", req.from_chain.slug, req.to_chain.slug),
            source_url: "https://app.across.to".to_string(),
            fetched_at: Self::now_rfc3339(),
        })
    }
}

impl Client {
    /// Build the `limits` / `suggested-fees` endpoint URL with the shared query
    /// parameters (mirrors the Go `url.Values` construction).
    fn endpoint_url(
        &self,
        path: &str,
        chain_from: &str,
        chain_to: &str,
        req: &BridgeQuoteRequest,
    ) -> Result<String, Error> {
        let mut url = Url::parse(&format!("{}/{}", self.base_url.trim_end_matches('/'), path))
            .map_err(|e| Error::wrap(Code::Internal, "build across endpoint url", e))?;
        url.query_pairs_mut()
            .append_pair("originChainId", chain_from)
            .append_pair("destinationChainId", chain_to)
            .append_pair("token", &req.from_asset.address)
            .append_pair("amount", &req.amount_base_units);
        Ok(url.to_string())
    }
}

/// The Across `/swap/approval` execution response (mirrors Go
/// `swapApprovalResponse`).
#[derive(Debug, Default, Deserialize)]
struct SwapApprovalResponse {
    #[serde(rename = "approvalTxns", default)]
    approval_txns: Vec<TxPayload>,
    #[serde(rename = "swapTx", default)]
    swap_tx: TxPayload,
    #[serde(rename = "minOutputAmount", default)]
    min_output_amount: String,
    #[serde(rename = "expectedOutputAmount", default)]
    expected_output_amount: String,
    #[serde(default)]
    steps: SwapSteps,
}

#[derive(Debug, Default, Deserialize)]
struct TxPayload {
    #[serde(rename = "chainId", default)]
    chain_id: i64,
    #[serde(default)]
    to: String,
    #[serde(default)]
    data: String,
    #[serde(default)]
    value: String,
}

#[derive(Debug, Default, Deserialize)]
struct SwapSteps {
    #[serde(default)]
    bridge: SwapBridgeStep,
}

#[derive(Debug, Default, Deserialize)]
struct SwapBridgeStep {
    #[serde(rename = "outputAmount", default)]
    output_amount: String,
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

        let mut url = Url::parse(&format!(
            "{}/swap/approval",
            self.base_url.trim_end_matches('/')
        ))
        .map_err(|e| Error::wrap(Code::Internal, "build across execution request", e))?;
        url.query_pairs_mut()
            .append_pair("amount", &req.amount_base_units)
            .append_pair("inputToken", &req.from_asset.address)
            .append_pair("outputToken", &req.to_asset.address)
            .append_pair("originChainId", &req.from_chain.evm_chain_id.to_string())
            .append_pair("destinationChainId", &req.to_chain.evm_chain_id.to_string())
            .append_pair("depositor", &sender)
            .append_pair("recipient", &recipient)
            .append_pair("slippage", &format_slippage(slippage_bps));

        let h_req = self.build_get(url.as_str(), "build across execution request")?;
        let resp = self
            .http
            .do_json::<SwapApprovalResponse>(h_req)
            .await?
            .value;

        if resp.swap_tx.to.trim().is_empty() || resp.swap_tx.data.trim().is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "across execution response missing swap transaction payload",
            ));
        }
        if !address::is_hex_address(resp.swap_tx.to.trim()) {
            return Err(Error::new(
                Code::ActionPlan,
                "across swap transaction target is not a valid EVM address",
            ));
        }
        if resp.swap_tx.chain_id != 0 && resp.swap_tx.chain_id != req.from_chain.evm_chain_id {
            return Err(Error::new(
                Code::ActionPlan,
                "across swap transaction chain does not match source chain",
            ));
        }

        let rpc_url = resolve_rpc_url(&opts.rpc_url, req.from_chain.evm_chain_id)
            .map_err(|e| Error::wrap(Code::Usage, "resolve rpc url", e))?;

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
        action.provider = "across".to_string();
        action.from_address = address::checksum(&sender)
            .map_err(|e| Error::wrap(Code::Usage, "checksum sender", e))?;
        action.to_address = address::checksum(&recipient)
            .map_err(|e| Error::wrap(Code::Usage, "checksum recipient", e))?;
        action.input_amount = req.amount_base_units.clone();

        let mut metadata = serde_json::Map::new();
        metadata.insert("to_chain_id".into(), req.to_chain.caip2.clone().into());
        metadata.insert(
            "from_asset_id".into(),
            req.from_asset.asset_id.clone().into(),
        );
        metadata.insert("to_asset_id".into(), req.to_asset.asset_id.clone().into());
        metadata.insert("route".into(), "across".into());
        action.metadata = Some(metadata);

        for (i, approval) in resp.approval_txns.iter().enumerate() {
            if approval.to.trim().is_empty() || approval.data.trim().is_empty() {
                continue;
            }
            if !address::is_hex_address(approval.to.trim()) {
                return Err(Error::new(
                    Code::ActionPlan,
                    "across approval transaction target is not a valid EVM address",
                ));
            }
            if approval.chain_id != 0 && approval.chain_id != req.from_chain.evm_chain_id {
                continue;
            }
            let target = address::checksum(approval.to.trim())
                .map_err(|e| Error::wrap(Code::ActionPlan, "checksum approval target", e))?;
            action.steps.push(ActionStep {
                step_id: format!("approve-bridge-token-{}", i + 1),
                step_type: StepType::Approval,
                status: StepStatus::Pending,
                chain_id: req.from_chain.caip2.clone(),
                rpc_url: rpc_url.clone(),
                description: "Approve across bridge contract for source token".to_string(),
                target,
                data: ensure_hex_prefix(&approval.data),
                value: normalize_transaction_value(&approval.value),
                calls: Vec::new(),
                expected_outputs: None,
                tx_hash: String::new(),
                error: String::new(),
            });
        }

        let swap_value = normalize_transaction_value(&resp.swap_tx.value);
        let swap_target = address::checksum(resp.swap_tx.to.trim())
            .map_err(|e| Error::wrap(Code::ActionPlan, "checksum swap target", e))?;
        let recipient_checksum = address::checksum(&recipient)
            .map_err(|e| Error::wrap(Code::Usage, "checksum recipient", e))?;

        let mut expected_outputs = serde_json::Map::new();
        expected_outputs.insert(
            "to_amount_min".into(),
            first_non_empty(&[
                &resp.min_output_amount,
                &resp.expected_output_amount,
                &resp.steps.bridge.output_amount,
            ])
            .into(),
        );
        expected_outputs.insert("settlement_provider".into(), "across".into());
        expected_outputs.insert(
            "settlement_status_endpoint".into(),
            ACROSS_SETTLEMENT_URL.into(),
        );
        expected_outputs.insert(
            "settlement_origin_chain".into(),
            req.from_chain.evm_chain_id.to_string().into(),
        );
        expected_outputs.insert("settlement_recipient".into(), recipient_checksum.into());
        expected_outputs.insert(
            "settlement_destination_chain".into(),
            req.to_chain.evm_chain_id.to_string().into(),
        );

        action.steps.push(ActionStep {
            step_id: "bridge-transfer".to_string(),
            step_type: StepType::Bridge,
            status: StepStatus::Pending,
            chain_id: req.from_chain.caip2.clone(),
            rpc_url,
            description: "Bridge transfer via Across".to_string(),
            target: swap_target,
            data: ensure_hex_prefix(&resp.swap_tx.data),
            value: swap_value,
            calls: Vec::new(),
            expected_outputs: Some(expected_outputs),
            tx_hash: String::new(),
            error: String::new(),
        });

        Ok(action)
    }
}

impl BridgeExecutionProvider for Client {}

// =============================================================================
// JSON dynamic-value helpers (mirror Go `numberString` / `floatValue` etc.).
// =============================================================================

/// Whether `amount` falls within the Across deposit `min`/`max` limits.
fn check_amount_within_limits(amount: &str, limits: &JsonMap) -> bool {
    let min = pick_number_string(limits, &["minDeposit", "minLimit"]);
    let max = pick_number_string(limits, &["maxDeposit", "maxLimit"]);
    if !min.is_empty() && compare_base_units(amount, &min) < 0 {
        return false;
    }
    if !max.is_empty() && compare_base_units(amount, &max) > 0 {
        return false;
    }
    true
}

/// First non-empty number string for any of `keys` (mirrors Go
/// `pickNumberString`).
fn pick_number_string(m: &JsonMap, keys: &[&str]) -> String {
    for key in keys {
        if let Some(v) = m.get(*key) {
            let out = number_string(v);
            if !out.is_empty() {
                return out;
            }
        }
    }
    String::new()
}

/// First parseable float for any of `keys` (mirrors Go `pickFloat`).
fn pick_float(m: &JsonMap, keys: &[&str]) -> f64 {
    for key in keys {
        if let Some(v) = m.get(*key) {
            if let Some(out) = float_value(v) {
                return out;
            }
        }
    }
    0.0
}

/// Normalize a dynamic JSON value into a canonical integer-string (mirrors Go
/// `numberString`): trims strings + leading zeros, formats numbers as integers,
/// and descends into `total` / `amount` for nested objects.
fn number_string(v: &Value) -> String {
    match v {
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                String::new()
            } else {
                trim_leading_zeros(s)
            }
        }
        Value::Number(n) => match n.as_f64() {
            Some(f) => trim_leading_zeros(&format!("{}", f.trunc() as i128)),
            None => String::new(),
        },
        Value::Object(map) => {
            let total = map.get("total").map(number_string).unwrap_or_default();
            if !total.is_empty() {
                return total;
            }
            map.get("amount").map(number_string).unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// Normalize a dynamic JSON value into a float (mirrors Go `floatValue`):
/// numbers pass through, numeric strings are parsed, and objects descend into
/// `usd` / `value`.
fn float_value(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                s.parse::<f64>().ok()
            }
        }
        Value::Object(map) => {
            if let Some(f) = map.get("usd").and_then(float_value) {
                return Some(f);
            }
            map.get("value").and_then(float_value)
        }
        _ => None,
    }
}

/// Build the optional [`model::BridgeFeeBreakdown`] from the suggested-fees
/// payload (mirrors Go `buildAcrossFeeBreakdown`).
fn build_across_fee_breakdown(
    req: &BridgeQuoteRequest,
    fees: &JsonMap,
    total_fee_base: &str,
    estimated_out: &str,
    total_fee_usd: f64,
    has_provider_output_amount: bool,
) -> Option<model::BridgeFeeBreakdown> {
    let lp_fee_base = pick_number_string(fees, &["lpFee", "lpFeeTotal"]);
    let relayer_fee_base = pick_number_string(fees, &["relayerCapitalFee", "capitalFeeTotal"]);
    let gas_fee_base = pick_number_string(fees, &["relayerGasFee", "relayGasFeeTotal"]);

    let mut breakdown = model::BridgeFeeBreakdown {
        lp_fee: fee_amount_from_base(&lp_fee_base, req.from_asset.decimals),
        relayer_fee: fee_amount_from_base(&relayer_fee_base, req.from_asset.decimals),
        gas_fee: fee_amount_from_base(&gas_fee_base, req.from_asset.decimals),
        total_fee_base_units: String::new(),
        total_fee_decimal: String::new(),
        total_fee_usd,
        consistent_with_amount_delta: None,
    };

    if !total_fee_base.trim().is_empty() {
        breakdown.total_fee_base_units = trim_leading_zeros(total_fee_base);
        breakdown.total_fee_decimal =
            format_decimal(&breakdown.total_fee_base_units, req.from_asset.decimals);
    }
    if has_provider_output_amount
        && !breakdown.total_fee_base_units.is_empty()
        && !estimated_out.trim().is_empty()
    {
        let delta = subtract_base_units(&req.amount_base_units, estimated_out);
        let consistent = compare_base_units(&delta, &breakdown.total_fee_base_units) == 0;
        breakdown.consistent_with_amount_delta = Some(consistent);
    }

    if breakdown.lp_fee.is_none()
        && breakdown.relayer_fee.is_none()
        && breakdown.gas_fee.is_none()
        && breakdown.total_fee_usd == 0.0
        && breakdown.total_fee_base_units.is_empty()
        && breakdown.consistent_with_amount_delta.is_none()
    {
        return None;
    }
    Some(breakdown)
}

/// Build an optional [`model::FeeAmount`] from a base-unit string (mirrors Go
/// `feeAmountFromBase`): empty or zero amounts yield `None`.
fn fee_amount_from_base(amount_base: &str, decimals: i32) -> Option<model::FeeAmount> {
    let amount_base = trim_leading_zeros(amount_base);
    if amount_base.is_empty() || amount_base == "0" {
        return None;
    }
    Some(model::FeeAmount {
        amount_base_units: amount_base.clone(),
        amount_decimal: format_decimal(&amount_base, decimals),
        amount_usd: 0.0,
    })
}

/// Approximate the USD fee for a USD-pegged stable asset (mirrors Go
/// `approximateStableUSD`): non-stable symbols and unparseable amounts → `0`.
fn approximate_stable_usd(symbol: &str, amount_base: &str, decimals: i32) -> f64 {
    if !is_likely_stable_symbol(symbol) {
        return 0.0;
    }
    let amount_decimal = format_decimal(amount_base, decimals);
    if amount_decimal.trim().is_empty() {
        return 0.0;
    }
    amount_decimal.trim().parse::<f64>().unwrap_or(0.0)
}

/// Whether `symbol` is a known USD-pegged stablecoin (mirrors Go
/// `isLikelyStableSymbol`).
fn is_likely_stable_symbol(symbol: &str) -> bool {
    matches!(
        symbol.trim().to_ascii_uppercase().as_str(),
        "USDC"
            | "USDT"
            | "USDT0"
            | "DAI"
            | "USDE"
            | "USDS"
            | "USD1"
            | "FRAX"
            | "GHO"
            | "TUSD"
            | "LUSD"
            | "PYUSD"
    )
}

// =============================================================================
// Base-unit big-integer string math (mirror Go helpers).
// =============================================================================

/// Compare two non-negative decimal base-unit strings (mirrors Go
/// `compareBaseUnits`): `-1`, `0`, or `1`.
fn compare_base_units(a: &str, b: &str) -> i32 {
    let a = trim_leading_zeros(a);
    let b = trim_leading_zeros(b);
    if a.len() != b.len() {
        return if a.len() < b.len() { -1 } else { 1 };
    }
    match a.cmp(&b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Equal => 0,
    }
}

/// Subtract `fee` from `amount` over non-negative decimal base-unit strings
/// (mirrors Go `subtractBaseUnits`): underflow clamps to `"0"`.
fn subtract_base_units(amount: &str, fee: &str) -> String {
    if compare_base_units(amount, fee) <= 0 {
        return "0".to_string();
    }
    let ai = to_digits(amount);
    let bi = to_digits(fee);
    let ai = ai.as_bytes();
    let bi = bi.as_bytes();
    let mut carry = 0i32;
    let mut res: Vec<u8> = Vec::with_capacity(ai.len());
    let mut i = ai.len() as isize - 1;
    let mut j = bi.len() as isize - 1;
    while i >= 0 {
        let mut a = (ai[i as usize] - b'0') as i32 - carry;
        let b = if j >= 0 {
            (bi[j as usize] - b'0') as i32
        } else {
            0
        };
        if a < b {
            a += 10;
            carry = 1;
        } else {
            carry = 0;
        }
        res.push((a - b) as u8 + b'0');
        i -= 1;
        j -= 1;
    }
    res.reverse();
    // `res` is ASCII digits by construction.
    trim_leading_zeros(&String::from_utf8(res).unwrap_or_else(|_| "0".to_string()))
}

/// Strip leading zeros (mirrors Go `trimLeadingZeros`): an all-zero or empty
/// input collapses to `"0"`.
fn trim_leading_zeros(v: &str) -> String {
    let trimmed = v.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Coerce an arbitrary string into a non-negative decimal-digit string (mirrors
/// Go `toDigits`): non-numeric input → `"0"`.
fn to_digits(v: &str) -> String {
    let v = v.trim();
    if v.is_empty() {
        return "0".to_string();
    }
    if !v.chars().all(|c| c.is_ascii_digit()) {
        return "0".to_string();
    }
    trim_leading_zeros(v)
}

/// Render a basis-points slippage as a fractional string with 6 decimals
/// (mirrors Go `formatSlippage`).
fn format_slippage(bps: i64) -> String {
    format!("{:.6}", bps as f64 / 10_000.0)
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

/// Normalize a transaction value (hex or decimal) into a canonical decimal
/// big-int string (mirrors Go `normalizeTransactionValue`): empty/invalid → `0`.
fn normalize_transaction_value(v: &str) -> String {
    let clean = v.trim();
    if clean.is_empty() {
        return "0".to_string();
    }
    if let Some(hex) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        return match BigInt::parse_bytes(hex.as_bytes(), 16) {
            Some(n) => n.to_string(),
            None => "0".to_string(),
        };
    }
    match BigInt::parse_bytes(clean.as_bytes(), 10) {
        Some(n) => n.to_string(),
        None => "0".to_string(),
    }
}

/// First trimmed non-empty value (mirrors Go `firstNonEmpty`).
fn first_non_empty(values: &[&str]) -> String {
    for v in values {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for the `defi-providers::across` module.
    //!
    //! Go source: `internal/providers/across/{client.go,client_test.go}`. The
    //! adapter implements bridge QUOTE (`/limits` + `/suggested-fees`) and the
    //! executable bridge ACTION build (`/swap/approval`). The Rust port is
    //! "correct" iff it preserves the machine-contract-relevant behavior the Go
    //! tests assert (all ported here via `wiremock`, offline + deterministic):
    //!
    //!  A1. Base-unit string math (`compare_base_units` / `subtract_base_units`):
    //!      `100 > 99`; `1000 - 1 == 999`; underflow `1 - 2 == 0`.
    //!      (Ports Go `TestBaseUnitMathHelpers`.)
    //!
    //!  A2. QUOTE with absolute fees + provider output amount: the estimated out
    //!      equals the provider `outputAmount`; the USD fee falls back to the
    //!      stable-asset approximation when no USD field is present; the fee
    //!      breakdown carries `total_fee_base_units` + per-component gas/relayer
    //!      fees, and `consistent_with_amount_delta == true` when the input minus
    //!      output equals the total fee. (Ports Go
    //!      `TestQuoteBridgeAcrossFeeBreakdownAndConsistency`.)
    //!
    //!  A3. QUOTE with only a percentage relay fee: the estimated out stays the
    //!      input amount (a percentage must NOT be treated as base units); the
    //!      provider USD fee is used verbatim; no canonical total fee base units /
    //!      decimal are emitted, and the consistency flag is omitted. (Ports Go
    //!      `TestQuoteBridgeDoesNotTreatRelayFeePctAsBaseUnits`.)
    //!
    //!  A4. QUOTE rejects non-EVM chains with an error. (Ports Go
    //!      `TestQuoteBridgeRejectsNonEVMChains`.)
    //!
    //!  A5. ACTION build produces an approval step + a bridge-transfer step; the
    //!      bridge step's expected outputs mark Across as the settlement provider.
    //!      (Ports Go `TestBuildBridgeAction`.)
    //!
    //!  A6. ACTION build rejects an invalid swap-transaction target address.
    //!      (Ports Go `TestBuildBridgeActionRejectsInvalidSwapTarget`.)
    //!
    //!  A7. `approximate_stable_usd` / `is_likely_stable_symbol` exclude non-USD
    //!      pegs such as `EURS`. (Ports Go `TestApproximateStableUSDExcludesEURS`.)
    //!
    //! Go tests intentionally SKIPPED as covered elsewhere: none — every Go test
    //! case in `client_test.go` is ported above.

    use super::*;
    use std::time::Duration;

    use defi_execution::{BridgeExecutionOptions, BridgeQuoteRequest};
    use defi_id::{parse_asset, parse_chain};
    use wiremock::matchers::{method, path};
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

    // ----- A1: base-unit math helpers --------------------------------------
    #[test]
    fn base_unit_math_helpers() {
        assert!(compare_base_units("100", "99") > 0);
        assert_eq!(subtract_base_units("1000", "1"), "999");
        assert_eq!(subtract_base_units("1", "2"), "0");
    }

    // ----- A2: quote fee breakdown + consistency ---------------------------
    #[tokio::test]
    async fn quote_bridge_fee_breakdown_and_consistency() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/limits"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"minDeposit":"500007","maxDeposit":"1954894537806"}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/suggested-fees"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "relayFeeTotal":"2633",
                    "relayGasFeeTotal":"2533",
                    "capitalFeeTotal":"100",
                    "lpFee":{"total":"0"},
                    "outputAmount":"997367",
                    "estimatedFillTimeSec":5
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http());
        client.set_base_url(&server.uri());

        let got = client
            .quote_bridge(quote_req("ethereum", "base"))
            .await
            .expect("quote_bridge");

        assert_eq!(got.estimated_out.amount_base_units, "997367");
        assert!(
            got.estimated_fee_usd > 0.0,
            "expected non-zero fee usd fallback for stable asset, got {}",
            got.estimated_fee_usd
        );
        let fb = got.fee_breakdown.expect("expected fee breakdown");
        assert_eq!(fb.total_fee_base_units, "2633");
        let gas = fb.gas_fee.expect("expected gas fee");
        assert_eq!(gas.amount_base_units, "2533");
        let relayer = fb.relayer_fee.expect("expected relayer fee");
        assert_eq!(relayer.amount_base_units, "100");
        assert_eq!(
            fb.consistent_with_amount_delta,
            Some(true),
            "expected consistency check true"
        );
    }

    // ----- A3: percentage relay fee must not be treated as base units ------
    #[tokio::test]
    async fn quote_bridge_does_not_treat_relay_fee_pct_as_base_units() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/limits"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"minDeposit":"1","maxDeposit":"1954894537806"}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/suggested-fees"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"relayFeePct":"0.003","feeUsd":1.23,"estimatedFillTimeSec":5}"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http());
        client.set_base_url(&server.uri());

        let got = client
            .quote_bridge(quote_req("ethereum", "base"))
            .await
            .expect("quote_bridge");

        assert_eq!(
            got.estimated_out.amount_base_units, "1000000",
            "estimated out should remain input amount when only relayFeePct is present"
        );
        assert_eq!(got.estimated_fee_usd, 1.23);
        let fb = got
            .fee_breakdown
            .expect("expected fee breakdown when fee usd is present");
        assert_eq!(
            fb.total_fee_base_units, "",
            "expected no canonical total fee base units when absolute fee is unavailable"
        );
        assert_eq!(
            fb.total_fee_decimal, "",
            "expected no total fee decimal without canonical base units"
        );
        assert_eq!(
            fb.consistent_with_amount_delta, None,
            "expected consistency check omitted when output amount is not provider-reported"
        );
    }

    // ----- A4: non-EVM chains rejected -------------------------------------
    #[tokio::test]
    async fn quote_bridge_rejects_non_evm_chains() {
        let client = Client::new(http());
        let err = client
            .quote_bridge(quote_req("solana", "base"))
            .await
            .expect_err("expected unsupported chain error");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- A5: build bridge action ----------------------------------------
    #[tokio::test]
    async fn build_bridge_action() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/swap/approval"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "approvalTxns": [{
                        "chainId": 1,
                        "to": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
                        "data": "0x095ea7b3",
                        "value": "0"
                    }],
                    "swapTx": {
                        "chainId": 1,
                        "to": "0x5c7BCd6E7De5423a257D81B442095A1a6ced35C5",
                        "data": "0xad5425c6",
                        "value": "0x0"
                    },
                    "minOutputAmount": "990000",
                    "expectedOutputAmount": "995000",
                    "expectedFillTime": 5
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

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
                    rpc_url: String::new(),
                    from_amount_for_gas: String::new(),
                },
            )
            .await
            .expect("build_bridge_action");

        assert_eq!(action.provider, "across");
        assert_eq!(
            action.steps.len(),
            2,
            "expected approval + bridge steps, got {}",
            action.steps.len()
        );
        let outs = action.steps[1]
            .expected_outputs
            .as_ref()
            .expect("bridge step expected outputs");
        assert_eq!(
            outs.get("settlement_provider").and_then(|v| v.as_str()),
            Some("across")
        );
    }

    // ----- A6: invalid swap target rejected --------------------------------
    #[tokio::test]
    async fn build_bridge_action_rejects_invalid_swap_target() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/swap/approval"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "approvalTxns": [],
                    "swapTx": {
                        "chainId": 1,
                        "to": "not-an-address",
                        "data": "0xad5425c6",
                        "value": "0x0"
                    }
                }"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let mut client = Client::new(http());
        client.set_base_url(&server.uri());

        let err = client
            .build_bridge_action(
                quote_req("ethereum", "base"),
                BridgeExecutionOptions {
                    sender: "0x00000000000000000000000000000000000000AA".to_string(),
                    recipient: "0x00000000000000000000000000000000000000BB".to_string(),
                    slippage_bps: 50,
                    simulate: true,
                    rpc_url: String::new(),
                    from_amount_for_gas: String::new(),
                },
            )
            .await
            .expect_err("expected invalid swap target error");
        assert_eq!(err.code, Code::ActionPlan);
    }

    // ----- A7: EURS is not treated as USD-pegged ---------------------------
    #[test]
    fn approximate_stable_usd_excludes_eurs() {
        assert!(
            !is_likely_stable_symbol("EURS"),
            "EURS should not be treated as USD-pegged"
        );
        assert_eq!(
            approximate_stable_usd("EURS", "1000000", 6),
            0.0,
            "expected EURS USD approximation to be disabled"
        );
    }

    // ----- metadata: callable without a key --------------------------------
    #[test]
    fn info_is_bridge_metadata() {
        let client = Client::new(http());
        let info = client.info();
        assert_eq!(info.name, "across");
        assert_eq!(info.provider_type, "bridge");
        assert!(!info.requires_key);
        assert!(info.capabilities.iter().any(|c| c == "bridge.quote"));
        assert!(info.capabilities.iter().any(|c| c == "bridge.plan"));
        assert!(info.capabilities.iter().any(|c| c == "bridge.execute"));
    }
}
