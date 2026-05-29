//! TaikoSwap provider adapter — Uniswap V3-style swap quotes + executable swap
//! action building, backed by on-chain RPC reads.
//!
//! Go source: `internal/providers/taikoswap/client.go` (+ `client_test.go`).
//!
//! Implements the `SwapProvider` (quote) + `SwapActionBuilder` (action build)
//! surfaces, plus `Provider` metadata, and the marker `SwapExecutionProvider`.
//!
//! TaikoSwap is a Uniswap V3 fork on Taiko. Quotes probe each canonical fee
//! tier (100/500/3000/10000) via the QuoterV2 `quoteExactInputSingle` and pick
//! the route returning the most output (ties broken by lower gas estimate).
//! Execution builds a standard EVM action: an optional ERC-20 `approve` step
//! when the router's allowance is short, followed by an `exactInputSingle` swap
//! step. No API key is required; supported on Taiko mainnet (`167000`) and
//! Taiko Hoodi testnet (`167013`).
//!
//! Amounts carry both base units and decimal forms. The `fetched_at` clock is
//! injectable for deterministic output. Exact-output is not supported (Go is
//! exact-input only).

use alloy::dyn_abi::DynSolValue;
use alloy::primitives::{Address as AlloyAddress, U256};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_evm::abi::Function;
use defi_evm::address;
use defi_evm::rpc::{CallRequest, RpcClient};
use defi_execution::{
    Action, ActionStep, Constraints, StepStatus, StepType, SwapActionBuilder, SwapExecutionOptions,
    SwapQuoteRequest,
};
use defi_id::{format_decimal, Chain};
use defi_model as model;
use num_bigint::{BigInt, Sign};

use crate::traits::{Provider, SwapExecutionProvider, SwapProvider};

const SOURCE_URL: &str = "https://swap.taiko.xyz";
/// Default slippage in basis points when the caller does not specify one
/// (mirrors Go's `slippage <= 0 -> 50`).
const DEFAULT_SLIPPAGE_BPS: i64 = 50;
/// Canonical Uniswap V3 fee tiers probed for the best route (mirrors Go
/// `feeTiers`).
const FEE_TIERS: [u32; 4] = [100, 500, 3000, 10000];

/// TaikoSwap (Uniswap V3-style) swap adapter (mirrors Go `taikoswap.Client`).
pub struct Client {
    /// Injected fixed clock for deterministic `fetched_at`; `None` uses the wall
    /// clock.
    now: Option<DateTime<Utc>>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolved chain configuration: connected RPC client, the resolved RPC URL
/// (stored on action steps), the QuoterV2 address, and the SwapRouter address.
struct ChainConfig {
    client: RpcClient,
    rpc_url: String,
    quoter: AlloyAddress,
    router: AlloyAddress,
}

impl Client {
    /// Build a TaikoSwap client (mirrors Go `New()`).
    pub fn new() -> Self {
        Client { now: None }
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
            name: "taikoswap".to_string(),
            provider_type: "swap".to_string(),
            requires_key: false,
            capabilities: vec![
                "swap.quote".to_string(),
                "swap.plan".to_string(),
                "swap.execute".to_string(),
            ],
            key_env_var_name: String::new(),
            capability_auth: Vec::new(),
        }
    }

    /// Resolve the RPC URL + QuoterV2/Router addresses for a chain, then connect.
    /// Mirrors Go `chainConfig`.
    fn chain_config(&self, chain: &Chain, rpc_override: &str) -> Result<ChainConfig, Error> {
        let (quoter_raw, router_raw) = defi_registry::uniswap_v3_contracts(chain.evm_chain_id)
            .ok_or_else(|| {
                Error::new(
                    Code::Unsupported,
                    "taikoswap only supports taiko mainnet/hoodi chains",
                )
            })?;
        let rpc_url = defi_registry::resolve_rpc_url(rpc_override, chain.evm_chain_id)
            .map_err(|e| Error::wrap(Code::Usage, "resolve rpc url", e))?;
        let client = RpcClient::connect(&rpc_url)
            .map_err(|e| Error::wrap(Code::Unavailable, "connect taiko rpc", e))?;
        let quoter = address::parse(quoter_raw)
            .map_err(|e| Error::wrap(Code::Internal, "parse taikoswap quoter address", e))?
            .into_inner();
        let router = address::parse(router_raw)
            .map_err(|e| Error::wrap(Code::Internal, "parse taikoswap router address", e))?
            .into_inner();
        Ok(ChainConfig {
            client,
            rpc_url,
            quoter,
            router,
        })
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
        let cfg = self.chain_config(&req.chain, &req.rpc_url)?;

        let amount_in = parse_amount(&req.amount_base_units)?;
        let from = parse_token_address(&req.from_asset.address)?;
        let to = parse_token_address(&req.to_asset.address)?;

        let best = quote_best_fee(&cfg.client, cfg.quoter, from, to, &amount_in).await?;

        let output_decimals = req.to_asset.decimals;
        Ok(model::SwapQuote {
            provider: "taikoswap".to_string(),
            chain_id: req.chain.caip2.clone(),
            from_asset_id: req.from_asset.asset_id.clone(),
            to_asset_id: req.to_asset.asset_id.clone(),
            // Go leaves TradeType empty for this provider's quote literal.
            trade_type: String::new(),
            input_amount: model::AmountInfo {
                amount_base_units: req.amount_base_units.clone(),
                amount_decimal: req.amount_decimal.clone(),
                decimals: req.from_asset.decimals as i64,
            },
            estimated_out: model::AmountInfo {
                amount_base_units: best.amount_out.to_string(),
                amount_decimal: format_decimal(&best.amount_out.to_string(), output_decimals),
                decimals: output_decimals as i64,
            },
            estimated_gas_usd: 0.0,
            price_impact_pct: 0.0,
            route: format!("taikoswap-v3-fee-{}", best.fee),
            source_url: SOURCE_URL.to_string(),
            fetched_at: self.fetched_at(),
        })
    }
}

#[async_trait]
impl SwapActionBuilder for Client {
    async fn build_swap_action(
        &self,
        req: SwapQuoteRequest,
        opts: SwapExecutionOptions,
    ) -> Result<Action, Error> {
        let sender = opts.sender.trim();
        if sender.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "swap execution requires sender address",
            ));
        }
        if !address::is_hex_address(sender) {
            return Err(Error::new(
                Code::Usage,
                "swap execution sender must be a valid EVM address",
            ));
        }
        let cfg = self.chain_config(&req.chain, &opts.rpc_url)?;

        let amount_in = parse_amount(&req.amount_base_units)?;
        let from_token = parse_token_address(&req.from_asset.address)?;
        let to_token = parse_token_address(&req.to_asset.address)?;

        let recipient_raw = opts.recipient.trim();
        let recipient = if recipient_raw.is_empty() {
            sender
        } else {
            recipient_raw
        };
        if !address::is_hex_address(recipient) {
            return Err(Error::new(
                Code::Usage,
                "swap execution recipient must be a valid EVM address",
            ));
        }
        let recipient_addr = address::parse(recipient)
            .map_err(|e| Error::wrap(Code::Usage, "parse recipient address", e))?
            .into_inner();
        let sender_addr = address::parse(sender)
            .map_err(|e| Error::wrap(Code::Usage, "parse sender address", e))?
            .into_inner();

        let best =
            quote_best_fee(&cfg.client, cfg.quoter, from_token, to_token, &amount_in).await?;

        let mut slippage = opts.slippage_bps;
        if slippage <= 0 {
            slippage = DEFAULT_SLIPPAGE_BPS;
        }
        if slippage >= 10_000 {
            return Err(Error::new(
                Code::Usage,
                "slippage bps must be less than 10000",
            ));
        }
        let amount_out_min = apply_slippage_floor(&best.amount_out, slippage);

        let mut action = Action::new(
            defi_execution::new_action_id(),
            "swap",
            req.chain.caip2.clone(),
            Constraints {
                slippage_bps: slippage,
                deadline: String::new(),
                simulate: opts.simulate,
            },
        );
        action.provider = "taikoswap".to_string();
        action.from_address = checksum_hex(&sender_addr);
        action.to_address = checksum_hex(&recipient_addr);
        action.input_amount = req.amount_base_units.clone();

        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "token_in".to_string(),
            serde_json::Value::String(checksum_hex(&from_token)),
        );
        metadata.insert(
            "token_out".to_string(),
            serde_json::Value::String(checksum_hex(&to_token)),
        );
        metadata.insert(
            "fee".to_string(),
            serde_json::Value::Number(serde_json::Number::from(best.fee)),
        );
        metadata.insert(
            "quoted_amount".to_string(),
            serde_json::Value::String(best.amount_out.to_string()),
        );
        metadata.insert(
            "amount_out_min".to_string(),
            serde_json::Value::String(amount_out_min.to_string()),
        );
        action.metadata = Some(metadata);

        let allowance = read_allowance(&cfg.client, from_token, sender_addr, cfg.router).await?;
        if allowance < amount_in {
            let approve_data = erc20_function("approve")?.encode(&[
                DynSolValue::Address(cfg.router),
                bigint_to_uint256(&amount_in),
            ])?;
            action.steps.push(ActionStep {
                step_id: "approve-token-in".to_string(),
                step_type: StepType::Approval,
                status: StepStatus::Pending,
                chain_id: req.chain.caip2.clone(),
                rpc_url: cfg.rpc_url.clone(),
                description: "Approve token spending for swap router".to_string(),
                target: checksum_hex(&from_token),
                data: format!("0x{}", hex::encode(approve_data)),
                value: "0".to_string(),
                calls: Vec::new(),
                expected_outputs: None,
                tx_hash: String::new(),
                error: String::new(),
            });
        }

        let swap_params = DynSolValue::Tuple(vec![
            DynSolValue::Address(from_token),
            DynSolValue::Address(to_token),
            DynSolValue::Uint(U256::from(best.fee), 24),
            DynSolValue::Address(recipient_addr),
            bigint_to_uint256(&amount_in),
            bigint_to_uint256(&amount_out_min),
            DynSolValue::Uint(U256::ZERO, 160),
        ]);
        let swap_data = router_function("exactInputSingle")?.encode(&[swap_params])?;

        let mut expected = serde_json::Map::new();
        expected.insert(
            "amount_out_min".to_string(),
            serde_json::Value::String(amount_out_min.to_string()),
        );
        action.steps.push(ActionStep {
            step_id: "swap-exact-input-single".to_string(),
            step_type: StepType::Swap,
            status: StepStatus::Pending,
            chain_id: req.chain.caip2.clone(),
            rpc_url: cfg.rpc_url.clone(),
            description: "Swap exact input via TaikoSwap router".to_string(),
            target: checksum_hex(&cfg.router),
            data: format!("0x{}", hex::encode(swap_data)),
            value: "0".to_string(),
            calls: Vec::new(),
            expected_outputs: Some(expected),
            tx_hash: String::new(),
            error: String::new(),
        });
        Ok(action)
    }
}

impl SwapExecutionProvider for Client {}

// ----- on-chain reads ------------------------------------------------------

/// The winning route from probing each fee tier (Go `quoteBestFee` return).
struct BestFee {
    amount_out: BigInt,
    fee: u32,
}

/// Probe each canonical fee tier and return the best route: the highest output
/// amount, ties broken by the lower gas estimate (mirrors Go `quoteBestFee`).
async fn quote_best_fee(
    client: &RpcClient,
    quoter: AlloyAddress,
    token_in: AlloyAddress,
    token_out: AlloyAddress,
    amount_in: &BigInt,
) -> Result<BestFee, Error> {
    let func = quoter_function("quoteExactInputSingle")?;
    let mut best: Option<(BigInt, BigInt, u32)> = None; // (out, gas, fee)

    for fee in FEE_TIERS {
        let params = DynSolValue::Tuple(vec![
            DynSolValue::Address(token_in),
            DynSolValue::Address(token_out),
            bigint_to_uint256(amount_in),
            DynSolValue::Uint(U256::from(fee), 24),
            DynSolValue::Uint(U256::ZERO, 160),
        ]);
        let call_data = func
            .encode(&[params])
            .map_err(|e| Error::wrap(Code::Internal, "pack quoter calldata", e))?;
        let request = CallRequest::new(None, Some(quoter.into()), U256::ZERO, call_data);
        // A reverting fee tier (e.g. no pool) is skipped, matching Go's
        // `continue` on call / decode failure.
        let out = match client.call(&request).await {
            Ok(out) => out,
            Err(_) => continue,
        };
        let decoded = match func.decode_output(&out) {
            Ok(values) if values.len() >= 4 => values,
            _ => continue,
        };
        let amount_out = match decoded.first().and_then(dyn_uint_to_bigint) {
            Some(n) if n.sign() == Sign::Plus => n,
            _ => continue,
        };
        let gas_estimate = decoded
            .get(3)
            .and_then(dyn_uint_to_bigint)
            .unwrap_or_else(|| BigInt::from(0));

        let replace = match &best {
            None => true,
            Some((best_out, best_gas, _)) => {
                amount_out > *best_out || (amount_out == *best_out && gas_estimate < *best_gas)
            }
        };
        if replace {
            best = Some((amount_out, gas_estimate, fee));
        }
    }

    match best {
        Some((amount_out, _, fee)) => Ok(BestFee { amount_out, fee }),
        None => Err(Error::new(
            Code::Unavailable,
            "taikoswap quote unavailable for token pair",
        )),
    }
}

/// Read the ERC-20 allowance of `spender` over `owner`'s `token` balance
/// (mirrors Go's inline `allowance` read in `BuildSwapAction`).
async fn read_allowance(
    client: &RpcClient,
    token: AlloyAddress,
    owner: AlloyAddress,
    spender: AlloyAddress,
) -> Result<BigInt, Error> {
    let func = erc20_function("allowance")?;
    let call_data = func
        .encode(&[DynSolValue::Address(owner), DynSolValue::Address(spender)])
        .map_err(|e| Error::wrap(Code::Internal, "pack allowance call", e))?;
    let request = CallRequest::new(
        Some(owner.into()),
        Some(token.into()),
        U256::ZERO,
        call_data,
    );
    let out = client
        .call(&request)
        .await
        .map_err(|e| Error::wrap(Code::Unavailable, "read allowance", e))?;
    let values = func
        .decode_output(&out)
        .map_err(|e| Error::wrap(Code::Unavailable, "decode allowance", e))?;
    values
        .first()
        .and_then(dyn_uint_to_bigint)
        .ok_or_else(|| Error::new(Code::Unavailable, "invalid allowance response"))
}

// ----- ABI helpers ---------------------------------------------------------

fn quoter_function(name: &str) -> Result<Function, Error> {
    Function::from_abi_json(defi_registry::UNISWAP_V3_QUOTER_V2_ABI, name)
}

fn router_function(name: &str) -> Result<Function, Error> {
    Function::from_abi_json(defi_registry::UNISWAP_V3_ROUTER_ABI, name)
}

fn erc20_function(name: &str) -> Result<Function, Error> {
    Function::from_abi_json(defi_registry::ERC20_MINIMAL_ABI, name)
}

// ----- pure helpers --------------------------------------------------------

/// Parse a base-unit amount as a `BigInt` (mirrors Go `new(big.Int).SetString`
/// with the `CodeUsage` "invalid amount base units" error).
fn parse_amount(raw: &str) -> Result<BigInt, Error> {
    raw.trim()
        .parse::<BigInt>()
        .map_err(|_| Error::new(Code::Usage, "invalid amount base units"))
}

/// Parse a token's address into an alloy `Address` (mirrors Go
/// `common.HexToAddress(...)`).
fn parse_token_address(raw: &str) -> Result<AlloyAddress, Error> {
    address::parse(raw.trim())
        .map(|a| a.into_inner())
        .map_err(|e| Error::wrap(Code::Usage, "parse swap token address", e))
}

/// Apply a slippage floor: `amount * (10000 - bps) / 10000` (mirrors Go's
/// `amountOutMin` computation in `BuildSwapAction`).
fn apply_slippage_floor(amount: &BigInt, bps: i64) -> BigInt {
    (amount * BigInt::from(10_000 - bps)) / BigInt::from(10_000)
}

/// Convert a non-negative `BigInt` into a `DynSolValue::Uint` width 256.
fn bigint_to_uint256(v: &BigInt) -> DynSolValue {
    DynSolValue::Uint(bigint_to_u256(v), 256)
}

/// Convert a non-negative `BigInt` into a `U256` (clamps negatives to `0`; the
/// Go path only ever passes non-negative amounts).
fn bigint_to_u256(v: &BigInt) -> U256 {
    if v.sign() != Sign::Plus {
        return U256::ZERO;
    }
    let (_, bytes) = v.to_bytes_be();
    U256::try_from_be_slice(&bytes).unwrap_or(U256::ZERO)
}

/// Convert a `DynSolValue::Uint` into a `BigInt` (`None` for other variants).
fn dyn_uint_to_bigint(v: &DynSolValue) -> Option<BigInt> {
    let (n, _) = v.as_uint()?;
    Some(BigInt::from_bytes_be(Sign::Plus, &n.to_be_bytes::<32>()))
}

/// EIP-55 checksum `0x` hex of an alloy address (mirrors Go `addr.Hex()`).
fn checksum_hex(addr: &AlloyAddress) -> String {
    address::Address::from(*addr).to_hex()
}

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for the `defi-providers::taikoswap` module.
    //!
    //! Go source: `internal/providers/taikoswap/client.go` (+ `client_test.go`).
    //! The TaikoSwap JSON-RPC server is mocked with `wiremock` (the Rust analogue
    //! of Go's `httptest`). Tests are deterministic and offline. Each test
    //! re-expresses one Go `client_test.go` case:
    //!
    //!   * `TestQuoteSwapChoosesBestFeeRoute`
    //!   * `TestBuildSwapActionAddsApprovalWhenNeeded`
    //!   * `TestBuildSwapActionRequiresSender`
    //!   * `TestBuildSwapActionRejectsInvalidSender`
    //!   * `TestBuildSwapActionRejectsInvalidRecipient`
    //!   * `TestBuildSwapActionUsesRPCOverride`
    //!
    //! Contract invariants asserted: provider metadata (no key); best-fee-tier
    //! selection (highest output, tie-broken by gas); `taikoswap-v3-fee-<n>`
    //! route; approval + swap step ordering with the ERC-20 approve selector
    //! `0x095ea7b3`; sender/recipient validation; and step RPC-URL propagation
    //! from the override.

    use super::*;

    use alloy::dyn_abi::{FunctionExt, JsonAbiExt};
    use alloy::json_abi::{Function as JsonFunction, JsonAbi};
    use chrono::{TimeZone, Utc};
    use defi_execution::SwapTradeType;
    use defi_id::{parse_asset, Asset};
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    fn json_function(abi_json: &str, name: &str) -> JsonFunction {
        let abi: JsonAbi = serde_json::from_str(abi_json).expect("parse abi");
        abi.function(name)
            .and_then(|o| o.first())
            .cloned()
            .expect("function present")
    }

    /// Mock RPC responder reproducing the Go `newMockRPCServer` behavior: it
    /// counts `eth_call` requests and, on the 5th call when `include_allowance`
    /// is set, returns a zero allowance; the four quoter probes return outputs
    /// `1000, 2000, 1500, 500` (best = the 2nd, fee tier 500).
    struct RpcResponder {
        include_allowance: bool,
        call_count: AtomicUsize,
        quoter_fn: JsonFunction,
        allowance_fn: JsonFunction,
    }

    impl RpcResponder {
        fn new(include_allowance: bool) -> Self {
            RpcResponder {
                include_allowance,
                call_count: AtomicUsize::new(0),
                quoter_fn: json_function(
                    defi_registry::UNISWAP_V3_QUOTER_V2_ABI,
                    "quoteExactInputSingle",
                ),
                allowance_fn: json_function(defi_registry::ERC20_MINIMAL_ABI, "allowance"),
            }
        }

        fn pack_output(func: &JsonFunction, values: &[DynSolValue]) -> String {
            let bytes = func.abi_encode_output(values).expect("pack output");
            format!("0x{}", hex::encode(bytes))
        }
    }

    impl Respond for RpcResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = match serde_json::from_slice(&request.body) {
                Ok(v) => v,
                Err(_) => return ResponseTemplate::new(400),
            };
            let id = body.get("id").cloned().unwrap_or(json!(1));
            let method_name = body.get("method").and_then(Value::as_str).unwrap_or("");
            if method_name != "eth_call" {
                return rpc_error(&id, -32601, "method not supported in test");
            }
            // 1-based call index, matching the Go counter.
            let index = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;

            if self.include_allowance && index == 5 {
                return rpc_result(
                    &id,
                    &Self::pack_output(&self.allowance_fn, &[DynSolValue::Uint(U256::ZERO, 256)]),
                );
            }

            let amount_out: u64 = match index {
                1 => 1000,
                2 => 2000,
                3 => 1500,
                _ => 500,
            };
            rpc_result(
                &id,
                &Self::pack_output(
                    &self.quoter_fn,
                    &[
                        DynSolValue::Uint(U256::from(amount_out), 256), // amountOut
                        DynSolValue::Uint(U256::ZERO, 160),             // sqrtPriceX96After
                        DynSolValue::Uint(U256::ZERO, 32),              // initializedTicksCrossed
                        DynSolValue::Uint(U256::from(70_000u64), 256),  // gasEstimate
                    ],
                ),
            )
        }
    }

    fn rpc_result(id: &Value, result: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    fn rpc_error(id: &Value, code: i64, message: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        }))
    }

    async fn mock_server(include_allowance: bool) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(RpcResponder::new(include_allowance))
            .mount(&server)
            .await;
        server
    }

    fn client() -> Client {
        let mut c = Client::new();
        c.set_now(Utc.with_ymd_and_hms(2026, 5, 28, 12, 0, 0).unwrap());
        c
    }

    fn assets() -> (Chain, Asset, Asset) {
        let chain = defi_id::parse_chain("taiko").expect("parse taiko chain");
        let from = parse_asset("USDC", &chain).expect("parse USDC");
        let to = parse_asset("WETH", &chain).expect("parse WETH");
        (chain, from, to)
    }

    fn quote_req(chain: Chain, from: Asset, to: Asset, rpc: &str) -> SwapQuoteRequest {
        SwapQuoteRequest {
            chain,
            from_asset: from,
            to_asset: to,
            amount_base_units: "1000000".to_string(),
            amount_decimal: "1".to_string(),
            rpc_url: rpc.to_string(),
            trade_type: SwapTradeType::ExactInput,
            slippage_pct: None,
            swapper: String::new(),
        }
    }

    // ----- metadata -------------------------------------------------------

    #[test]
    fn info_is_metadata_only_no_key_required() {
        let c = Client::new();
        let info = Provider::info(&c);
        assert_eq!(info.name, "taikoswap");
        assert_eq!(info.provider_type, "swap");
        assert!(!info.requires_key);
        for cap in ["swap.quote", "swap.plan", "swap.execute"] {
            assert!(
                info.capabilities.iter().any(|c| c == cap),
                "missing capability {cap}"
            );
        }
    }

    // ----- TestQuoteSwapChoosesBestFeeRoute -------------------------------

    #[tokio::test]
    async fn quote_swap_chooses_best_fee_route() {
        let server = mock_server(false).await;
        let (chain, from, to) = assets();
        let quote = client()
            .quote_swap(quote_req(chain, from, to, &server.uri()))
            .await
            .expect("quote");
        assert_eq!(quote.provider, "taikoswap");
        assert!(
            quote.route.contains("fee-500"),
            "expected best fee tier 500 in route, got {}",
            quote.route
        );
        assert_eq!(quote.estimated_out.amount_base_units, "2000");
        assert_eq!(quote.input_amount.amount_base_units, "1000000");
    }

    // ----- TestBuildSwapActionAddsApprovalWhenNeeded ----------------------

    #[tokio::test]
    async fn build_swap_action_adds_approval_when_needed() {
        let server = mock_server(true).await;
        let (chain, from, to) = assets();
        let req = quote_req(chain, from, to, "");
        let opts = SwapExecutionOptions {
            sender: "0x00000000000000000000000000000000000000AA".to_string(),
            recipient: "0x00000000000000000000000000000000000000BB".to_string(),
            slippage_bps: 100,
            simulate: true,
            rpc_url: server.uri(),
        };
        let action = client().build_swap_action(req, opts).await.expect("build");
        assert_eq!(action.intent_type, "swap");
        assert_eq!(action.provider, "taikoswap");
        assert_eq!(action.steps.len(), 2, "expected approval + swap steps");
        assert_eq!(action.steps[0].step_type, StepType::Approval);
        assert_eq!(action.steps[1].step_type, StepType::Swap);
        // The first step is an ERC-20 approve.
        assert!(
            action.steps[0].data.starts_with("0x095ea7b3"),
            "expected approve selector, got {}",
            &action.steps[0].data[..10.min(action.steps[0].data.len())]
        );
    }

    // ----- TestBuildSwapActionRequiresSender ------------------------------

    #[tokio::test]
    async fn build_swap_action_requires_sender() {
        let (chain, from, to) = assets();
        let req = quote_req(chain, from, to, "");
        let err = client()
            .build_swap_action(req, SwapExecutionOptions::default())
            .await
            .expect_err("missing sender must fail");
        assert_eq!(err.code, Code::Usage);
    }

    // ----- TestBuildSwapActionRejectsInvalidSender ------------------------

    #[tokio::test]
    async fn build_swap_action_rejects_invalid_sender() {
        let (chain, from, to) = assets();
        let req = quote_req(chain, from, to, "");
        let opts = SwapExecutionOptions {
            sender: "not-an-address".to_string(),
            ..Default::default()
        };
        let err = client()
            .build_swap_action(req, opts)
            .await
            .expect_err("invalid sender must fail");
        assert_eq!(err.code, Code::Usage);
    }

    // ----- TestBuildSwapActionRejectsInvalidRecipient ---------------------

    #[tokio::test]
    async fn build_swap_action_rejects_invalid_recipient() {
        let server = mock_server(true).await;
        let (chain, from, to) = assets();
        let req = quote_req(chain, from, to, "");
        let opts = SwapExecutionOptions {
            sender: "0x00000000000000000000000000000000000000AA".to_string(),
            recipient: "not-an-address".to_string(),
            rpc_url: server.uri(),
            ..Default::default()
        };
        let err = client()
            .build_swap_action(req, opts)
            .await
            .expect_err("invalid recipient must fail");
        assert_eq!(err.code, Code::Usage);
    }

    // ----- TestBuildSwapActionUsesRPCOverride -----------------------------

    #[tokio::test]
    async fn build_swap_action_uses_rpc_override() {
        let server = mock_server(true).await;
        let (chain, from, to) = assets();
        let req = quote_req(chain, from, to, "");
        let opts = SwapExecutionOptions {
            sender: "0x00000000000000000000000000000000000000AA".to_string(),
            recipient: String::new(),
            slippage_bps: 100,
            simulate: true,
            rpc_url: server.uri(),
        };
        let action = client().build_swap_action(req, opts).await.expect("build");
        assert!(!action.steps.is_empty(), "expected non-empty steps");
        for (i, step) in action.steps.iter().enumerate() {
            assert_eq!(
                step.rpc_url,
                server.uri(),
                "expected step {i} rpc override propagated"
            );
        }
    }

    // ----- pure-helper coverage -------------------------------------------

    #[test]
    fn slippage_floor_matches_go() {
        // 2000 * (10000 - 100) / 10000 = 1980.
        assert_eq!(
            apply_slippage_floor(&BigInt::from(2000u64), 100),
            BigInt::from(1980u64)
        );
        // Default 50 bps: 1_000_000 * 9950 / 10000 = 995_000.
        assert_eq!(
            apply_slippage_floor(&BigInt::from(1_000_000u64), 50),
            BigInt::from(995_000u64)
        );
    }

    #[test]
    fn parse_amount_rejects_non_integer() {
        assert!(parse_amount("1000000").is_ok());
        assert_eq!(parse_amount("nope").unwrap_err().code, Code::Usage);
    }

    // keep the FunctionExt/JsonAbiExt imports referenced even if the compiler
    // folds the trait methods.
    #[test]
    fn responder_constructs() {
        let _r = RpcResponder::new(false);
        let _ = Arc::new(1u8);
    }
}
