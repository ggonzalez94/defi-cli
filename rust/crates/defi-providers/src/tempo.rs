//! Tempo provider adapter — Stablecoin DEX swap quotes + executable swap action
//! building, backed by on-chain RPC reads.
//!
//! Go source: `internal/providers/tempo/client.go` (+ `client_test.go`).
//!
//! Implements the `SwapProvider` (quote) + `SwapActionBuilder` (action build)
//! surfaces, plus `Provider` metadata, and the marker `SwapExecutionProvider`.
//!
//! Tempo is an on-chain swap path: it talks to the chain's Tempo Stablecoin DEX
//! contract and TIP-20 token metadata via `eth_call`. The DEX only routes
//! USD-denominated TIP-20 pairs, so both legs are validated for a `currency()`
//! of `USD` before quoting. Quotes are uint128-bounded; execution batches an
//! optional ERC-20 approve plus the swap into a single Tempo step (`calls`),
//! settling back to the sender only. No API key is required; supported on Tempo
//! mainnet (`4217`), moderato testnet (`42431`), and devnet (`31318`).
//!
//! Amounts carry both base units and decimal forms. The `fetched_at` clock is
//! injectable for deterministic output.

use alloy::dyn_abi::DynSolValue;
use alloy::primitives::{Address as AlloyAddress, U256};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_evm::abi::Function;
use defi_evm::address;
use defi_evm::rpc::{CallRequest, RpcClient};
use defi_execution::{Action, Constraints, SwapActionBuilder, SwapQuoteRequest, SwapTradeType};
use defi_execution::{ActionStep, StepCall, StepStatus, StepType, SwapExecutionOptions};
use defi_id::{format_decimal, Asset, Chain};
use defi_model as model;
use num_bigint::{BigInt, Sign};

use crate::traits::{Provider, SwapExecutionProvider, SwapProvider};

const SOURCE_URL: &str = "https://tempo.xyz";
const ROUTE: &str = "tempo-dex";
/// Default slippage in basis points when the caller does not specify one
/// (mirrors Go's `slippage <= 0 -> 50`).
const DEFAULT_SLIPPAGE_BPS: i64 = 50;

/// Tempo Stablecoin DEX swap adapter (mirrors Go `tempo.Client`).
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

impl Client {
    /// Build a tempo swap client (mirrors Go `New()`).
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
            name: "tempo".to_string(),
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

    /// Resolve the RPC URL + Tempo DEX address for a chain, then connect.
    /// Mirrors Go `chainConfig`.
    fn chain_config(
        &self,
        chain: &Chain,
        rpc_override: &str,
    ) -> Result<(RpcClient, AlloyAddress), Error> {
        let dex_raw = defi_registry::tempo_stablecoin_dex(chain.evm_chain_id).ok_or_else(|| {
            Error::new(
                Code::Unsupported,
                "tempo swap provider supports only tempo mainnet, moderato testnet, and devnet",
            )
        })?;
        let rpc_url = defi_registry::resolve_rpc_url(rpc_override, chain.evm_chain_id)
            .map_err(|e| Error::wrap(Code::Usage, "resolve rpc url", e))?;
        let client = RpcClient::connect(&rpc_url)
            .map_err(|e| Error::wrap(Code::Unavailable, "connect tempo rpc", e))?;
        let dex = address::parse(dex_raw)
            .map_err(|e| Error::wrap(Code::Internal, "parse tempo dex address", e))?
            .into_inner();
        Ok((client, dex))
    }

    /// Quote the output for an exact-input swap (mirrors Go `quoteExactAmountIn`).
    async fn quote_exact_amount_in(
        &self,
        client: &RpcClient,
        leg: &SwapLeg<'_>,
        amount_in: &BigInt,
    ) -> Result<BigInt, Error> {
        self.call_uint128_method(client, leg, "quoteSwapExactAmountIn", amount_in)
            .await
    }

    /// Quote the input required for an exact-output swap (mirrors Go
    /// `quoteExactAmountOut`).
    async fn quote_exact_amount_out(
        &self,
        client: &RpcClient,
        leg: &SwapLeg<'_>,
        amount_out: &BigInt,
    ) -> Result<BigInt, Error> {
        self.call_uint128_method(client, leg, "quoteSwapExactAmountOut", amount_out)
            .await
    }

    /// `eth_call` a DEX method returning a single `uint128`, classifying revert
    /// errors into pair-support guidance (mirrors Go `callUint128Method`).
    async fn call_uint128_method(
        &self,
        client: &RpcClient,
        leg: &SwapLeg<'_>,
        method: &str,
        amount: &BigInt,
    ) -> Result<BigInt, Error> {
        let func = dex_function(method)?;
        let call_data = func.encode(&[
            DynSolValue::Address(leg.token_in),
            DynSolValue::Address(leg.token_out),
            bigint_to_uint128(amount),
        ])?;
        let request = CallRequest::new(None, Some(leg.dex.into()), U256::ZERO, call_data);
        let out = match client.call(&request).await {
            Ok(out) => out,
            Err(e) => {
                return Err(classify_tempo_swap_call_error(
                    &e,
                    &tempo_asset_label(leg.from_asset),
                    &tempo_asset_label(leg.to_asset),
                ))
            }
        };
        let values = func
            .decode_output(&out)
            .map_err(|_| Error::new(Code::Unavailable, "decode tempo dex response"))?;
        let amount = values
            .first()
            .and_then(dyn_uint_to_bigint)
            .filter(|n| n.sign() == Sign::Plus)
            .ok_or_else(|| Error::new(Code::Unavailable, "tempo quote returned invalid amount"))?;
        Ok(amount)
    }
}

/// The resolved swap leg context: DEX target, both assets (for error labels),
/// and both token addresses. Bundling these keeps the quote helpers' arity low.
struct SwapLeg<'a> {
    dex: AlloyAddress,
    from_asset: &'a Asset,
    to_asset: &'a Asset,
    token_in: AlloyAddress,
    token_out: AlloyAddress,
}

impl Provider for Client {
    fn info(&self) -> model::ProviderInfo {
        Client::info(self)
    }
}

#[async_trait]
impl SwapProvider for Client {
    async fn quote_swap(&self, req: SwapQuoteRequest) -> Result<model::SwapQuote, Error> {
        let (client, dex) = self.chain_config(&req.chain, &req.rpc_url)?;
        let trade_type = req.trade_type;

        let amount = parse_uint128(&req.amount_base_units)?;
        let token_in = parse_token_address(&req.from_asset)?;
        let token_out = parse_token_address(&req.to_asset)?;
        validate_usd_pair(&client, &req.from_asset, &req.to_asset, token_in, token_out).await?;

        let leg = SwapLeg {
            dex,
            from_asset: &req.from_asset,
            to_asset: &req.to_asset,
            token_in,
            token_out,
        };

        let (input_amount, estimated_out) = match trade_type {
            SwapTradeType::ExactInput => {
                let out = self.quote_exact_amount_in(&client, &leg, &amount).await?;
                (amount.clone(), out)
            }
            SwapTradeType::ExactOutput => {
                let input = self.quote_exact_amount_out(&client, &leg, &amount).await?;
                (input, amount.clone())
            }
        };

        let input_decimals = asset_decimals(&req.from_asset);
        let output_decimals = asset_decimals(&req.to_asset);

        Ok(model::SwapQuote {
            provider: "tempo".to_string(),
            chain_id: req.chain.caip2.clone(),
            from_asset_id: req.from_asset.asset_id.clone(),
            to_asset_id: req.to_asset.asset_id.clone(),
            trade_type: trade_type.as_str().to_string(),
            input_amount: model::AmountInfo {
                amount_base_units: input_amount.to_string(),
                amount_decimal: format_decimal(&input_amount.to_string(), input_decimals),
                decimals: input_decimals as i64,
            },
            estimated_out: model::AmountInfo {
                amount_base_units: estimated_out.to_string(),
                amount_decimal: format_decimal(&estimated_out.to_string(), output_decimals),
                decimals: output_decimals as i64,
            },
            estimated_gas_usd: 0.0,
            price_impact_pct: 0.0,
            route: ROUTE.to_string(),
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
        if !address::eq_fold(recipient, sender) {
            return Err(Error::new(
                Code::Unsupported,
                "tempo swap execution currently settles to the sender only; omit --recipient or set it equal to --from-address",
            ));
        }

        let (client, dex) = self.chain_config(&req.chain, &opts.rpc_url)?;
        let trade_type = req.trade_type;

        let amount = parse_uint128(&req.amount_base_units)?;
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

        let token_in = parse_token_address(&req.from_asset)?;
        let token_out = parse_token_address(&req.to_asset)?;
        let sender_addr = address::parse(sender)
            .map_err(|e| Error::wrap(Code::Usage, "parse sender address", e))?
            .into_inner();
        validate_usd_pair(&client, &req.from_asset, &req.to_asset, token_in, token_out).await?;

        let leg = SwapLeg {
            dex,
            from_asset: &req.from_asset,
            to_asset: &req.to_asset,
            token_in,
            token_out,
        };

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
        action.provider = "tempo".to_string();
        action.from_address = lower_hex(&sender_addr);
        action.to_address = lower_hex(&sender_addr);

        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "trade_type".to_string(),
            serde_json::Value::String(trade_type.as_str().to_string()),
        );
        metadata.insert(
            "token_in".to_string(),
            serde_json::Value::String(lower_hex(&token_in)),
        );
        metadata.insert(
            "token_out".to_string(),
            serde_json::Value::String(lower_hex(&token_out)),
        );
        metadata.insert(
            "route".to_string(),
            serde_json::Value::String(ROUTE.to_string()),
        );

        let approval_amount: BigInt;
        let swap_data: Vec<u8>;
        let step_id: &str;
        let description: &str;
        let mut expected = serde_json::Map::new();

        match trade_type {
            SwapTradeType::ExactInput => {
                let quoted_out = self.quote_exact_amount_in(&client, &leg, &amount).await?;
                let min_amount_out = apply_slippage_floor(&quoted_out, slippage);
                swap_data = dex_function("swapExactAmountIn")?.encode(&[
                    DynSolValue::Address(token_in),
                    DynSolValue::Address(token_out),
                    bigint_to_uint128(&amount),
                    bigint_to_uint128(&min_amount_out),
                ])?;
                action.input_amount = amount.to_string();
                metadata.insert(
                    "quoted_amount_out".to_string(),
                    serde_json::Value::String(quoted_out.to_string()),
                );
                metadata.insert(
                    "amount_out_min".to_string(),
                    serde_json::Value::String(min_amount_out.to_string()),
                );
                approval_amount = amount.clone();
                step_id = "tempo-swap-exact-input";
                description = "Swap exact input via Tempo Stablecoin DEX";
                expected.insert(
                    "amount_out_min".to_string(),
                    serde_json::Value::String(min_amount_out.to_string()),
                );
            }
            SwapTradeType::ExactOutput => {
                let quoted_in = self.quote_exact_amount_out(&client, &leg, &amount).await?;
                let max_amount_in = apply_slippage_ceil(&quoted_in, slippage);
                swap_data = dex_function("swapExactAmountOut")?.encode(&[
                    DynSolValue::Address(token_in),
                    DynSolValue::Address(token_out),
                    bigint_to_uint128(&amount),
                    bigint_to_uint128(&max_amount_in),
                ])?;
                action.input_amount = max_amount_in.to_string();
                metadata.insert(
                    "desired_amount_out".to_string(),
                    serde_json::Value::String(amount.to_string()),
                );
                metadata.insert(
                    "quoted_amount_in".to_string(),
                    serde_json::Value::String(quoted_in.to_string()),
                );
                metadata.insert(
                    "amount_in_max".to_string(),
                    serde_json::Value::String(max_amount_in.to_string()),
                );
                approval_amount = max_amount_in.clone();
                step_id = "tempo-swap-exact-output";
                description = "Swap exact output via Tempo Stablecoin DEX";
                expected.insert(
                    "amount_in_max".to_string(),
                    serde_json::Value::String(max_amount_in.to_string()),
                );
                expected.insert(
                    "amount_out".to_string(),
                    serde_json::Value::String(amount.to_string()),
                );
            }
        }

        action.metadata = Some(metadata);

        // Build a single batched step with Calls. If approval is needed, the
        // approve call precedes the swap call in the same Tempo transaction.
        let mut calls: Vec<StepCall> = Vec::new();

        let allowance = read_allowance(&client, token_in, sender_addr, dex).await?;
        if allowance < approval_amount {
            let approve_data = erc20_function("approve")?.encode(&[
                DynSolValue::Address(dex),
                bigint_to_uint256(&approval_amount),
            ])?;
            calls.push(StepCall {
                target: lower_hex(&token_in),
                data: format!("0x{}", hex::encode(approve_data)),
                value: "0".to_string(),
            });
        }

        calls.push(StepCall {
            target: lower_hex(&dex),
            data: format!("0x{}", hex::encode(swap_data)),
            value: "0".to_string(),
        });

        action.steps.push(ActionStep {
            step_id: step_id.to_string(),
            step_type: StepType::Swap,
            status: StepStatus::Pending,
            chain_id: req.chain.caip2.clone(),
            rpc_url: rpc_url_for_step(&opts.rpc_url, req.chain.evm_chain_id),
            description: description.to_string(),
            target: String::new(),
            data: String::new(),
            value: "0".to_string(),
            calls,
            expected_outputs: Some(expected),
            tx_hash: String::new(),
            error: String::new(),
        });
        Ok(action)
    }
}

impl SwapExecutionProvider for Client {}

// ----- on-chain reads ------------------------------------------------------

/// Read the ERC-20 allowance of `spender` over `owner`'s `token` balance
/// (mirrors Go `readAllowance`).
async fn read_allowance(
    client: &RpcClient,
    token: AlloyAddress,
    owner: AlloyAddress,
    spender: AlloyAddress,
) -> Result<BigInt, Error> {
    let func = erc20_function("allowance")?;
    let call_data = func.encode(&[DynSolValue::Address(owner), DynSolValue::Address(spender)])?;
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
        .map_err(|_| Error::new(Code::Unavailable, "decode allowance"))?;
    values
        .first()
        .and_then(dyn_uint_to_bigint)
        .ok_or_else(|| Error::new(Code::Unavailable, "invalid allowance response"))
}

/// Validate that both legs of a swap are USD-denominated TIP-20 tokens
/// (mirrors Go `validateUSDPair`).
async fn validate_usd_pair(
    client: &RpcClient,
    from_asset: &Asset,
    to_asset: &Asset,
    token_in: AlloyAddress,
    token_out: AlloyAddress,
) -> Result<(), Error> {
    let from_currency = read_tip20_currency(client, token_in, from_asset).await?;
    let to_currency = read_tip20_currency(client, token_out, to_asset).await?;
    if !from_currency.eq_ignore_ascii_case("USD") || !to_currency.eq_ignore_ascii_case("USD") {
        return Err(Error::new(
            Code::Unsupported,
            format!(
                "tempo stablecoin dex supports only USD-denominated TIP-20s; got {} ({}) -> {} ({})",
                tempo_asset_label(from_asset),
                from_currency,
                tempo_asset_label(to_asset),
                to_currency
            ),
        ));
    }
    Ok(())
}

/// Read a TIP-20 token's `currency()` metadata (mirrors Go `readTIP20Currency`).
async fn read_tip20_currency(
    client: &RpcClient,
    token: AlloyAddress,
    asset: &Asset,
) -> Result<String, Error> {
    let func = tip20_function("currency")?;
    let call_data = func.encode(&[])?;
    let request = CallRequest::new(None, Some(token.into()), U256::ZERO, call_data);
    let out = match client.call(&request).await {
        Ok(out) => out,
        Err(e) => {
            if is_tempo_revert_error(&e) {
                return Err(Error::new(
                    Code::Unsupported,
                    format!(
                        "tempo swap asset {} is not a TIP-20 token with currency metadata",
                        tempo_asset_label(asset)
                    ),
                ));
            }
            return Err(Error::wrap(Code::Unavailable, "read token currency", e));
        }
    };
    let values = func
        .decode_output(&out)
        .map_err(|_| Error::new(Code::Unavailable, "decode token currency"))?;
    let currency = values
        .first()
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| Error::new(Code::Unavailable, "invalid token currency response"))?;
    Ok(currency.trim().to_ascii_uppercase())
}

// ----- error classification ------------------------------------------------

/// Map an `eth_call` failure into typed pair-support guidance (mirrors Go
/// `classifyTempoSwapCallError`).
fn classify_tempo_swap_call_error(
    err: &Error,
    token_in_label: &str,
    token_out_label: &str,
) -> Error {
    if is_tempo_revert_error(err) {
        let text = err.to_string();
        if text.contains("PairDoesNotExist") {
            return Error::new(
                Code::Unsupported,
                format!("tempo dex does not support {token_in_label} -> {token_out_label}"),
            );
        }
        if text.contains("InsufficientLiquidity") {
            return Error::new(
                Code::Unsupported,
                format!(
                    "tempo dex has insufficient liquidity for {token_in_label} -> {token_out_label}"
                ),
            );
        }
        return Error::new(
            Code::Unsupported,
            format!("tempo dex rejected {token_in_label} -> {token_out_label} swap request: {err}"),
        );
    }
    Error::new(Code::Unavailable, format!("query tempo dex: {err}"))
}

/// Whether an error's message indicates an EVM revert (mirrors Go
/// `isTempoRevertError`).
fn is_tempo_revert_error(err: &Error) -> bool {
    err.to_string()
        .to_ascii_lowercase()
        .contains("execution reverted")
}

// ----- ABI helpers ---------------------------------------------------------

fn dex_function(name: &str) -> Result<Function, Error> {
    Function::from_abi_json(defi_registry::TEMPO_STABLECOIN_DEX_ABI, name)
}

fn erc20_function(name: &str) -> Result<Function, Error> {
    Function::from_abi_json(defi_registry::ERC20_MINIMAL_ABI, name)
}

fn tip20_function(name: &str) -> Result<Function, Error> {
    Function::from_abi_json(defi_registry::TEMPO_TIP20_METADATA_ABI, name)
}

// ----- pure helpers --------------------------------------------------------

/// A short human label for an asset: its symbol when set, else its address
/// (mirrors Go `tempoAssetLabel`).
fn tempo_asset_label(asset: &Asset) -> String {
    if !asset.symbol.trim().is_empty() {
        asset.symbol.clone()
    } else {
        asset.address.clone()
    }
}

/// Effective decimals for an asset; non-positive falls back to 18 (mirrors the
/// Go `decimals <= 0 -> 18` defaulting).
fn asset_decimals(asset: &Asset) -> i32 {
    if asset.decimals <= 0 {
        18
    } else {
        asset.decimals
    }
}

/// Parse and validate a positive base-unit amount that fits in uint128
/// (mirrors Go `parseUint128`).
fn parse_uint128(raw: &str) -> Result<BigInt, Error> {
    let amount: BigInt = raw.trim().parse().map_err(|_| {
        Error::new(
            Code::Usage,
            "swap amount must be a positive integer in base units",
        )
    })?;
    if amount.sign() != Sign::Plus {
        return Err(Error::new(
            Code::Usage,
            "swap amount must be a positive integer in base units",
        ));
    }
    if amount.bits() > 128 {
        return Err(Error::new(
            Code::Usage,
            "swap amount exceeds uint128 bounds",
        ));
    }
    Ok(amount)
}

/// Parse the asset's address into an alloy `Address` (mirrors Go
/// `common.HexToAddress(req.FromAsset.Address)`).
fn parse_token_address(asset: &Asset) -> Result<AlloyAddress, Error> {
    address::parse(asset.address.trim())
        .map(|a| a.into_inner())
        .map_err(|e| Error::wrap(Code::Usage, "parse swap token address", e))
}

/// Apply a slippage floor: `amount * (10000 - bps) / 10000` (mirrors Go
/// `applySlippageFloor`).
fn apply_slippage_floor(amount: &BigInt, bps: i64) -> BigInt {
    (amount * BigInt::from(10_000 - bps)) / BigInt::from(10_000)
}

/// Apply a slippage ceiling: `ceil(amount * (10000 + bps) / 10000)` (mirrors Go
/// `applySlippageCeil`).
fn apply_slippage_ceil(amount: &BigInt, bps: i64) -> BigInt {
    let numerator = (amount * BigInt::from(10_000 + bps)) + BigInt::from(9_999);
    numerator / BigInt::from(10_000)
}

/// Convert a non-negative `BigInt` into a `DynSolValue::Uint` width 128.
fn bigint_to_uint128(v: &BigInt) -> DynSolValue {
    DynSolValue::Uint(bigint_to_u256(v), 128)
}

/// Convert a non-negative `BigInt` into a `DynSolValue::Uint` width 256.
fn bigint_to_uint256(v: &BigInt) -> DynSolValue {
    DynSolValue::Uint(bigint_to_u256(v), 256)
}

/// Convert a non-negative `BigInt` into a `U256` (clamps negatives to `0`,
/// matching the Go path which only ever passes non-negative amounts).
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

/// Lowercase `0x` hex of an alloy address (mirrors Go `addr.Hex()` lowercased
/// for the persisted action shape).
fn lower_hex(addr: &AlloyAddress) -> String {
    format!("0x{}", hex::encode(addr.as_slice()))
}

/// Resolve the step's RPC URL: the override if set, else the registry default
/// for the chain (mirrors Go's `rpcURL` from `chainConfig`, which is stored on
/// the step).
fn rpc_url_for_step(rpc_override: &str, chain_id: i64) -> String {
    defi_registry::resolve_rpc_url(rpc_override, chain_id).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for the `defi-providers::tempo` module.
    //!
    //! Go source: `internal/providers/tempo/client.go` (+ `client_test.go`).
    //! The Tempo JSON-RPC server is mocked with `wiremock` (the Rust analogue of
    //! Go's `httptest`). Tests are deterministic and offline. Each test
    //! re-expresses one Go `client_test.go` case:
    //!
    //!   * `TestQuoteSwapExactInput`
    //!   * `TestQuoteSwapExactOutput`
    //!   * `TestBuildSwapActionBatchesApproveAndSwapForExactInput`
    //!   * `TestBuildSwapActionSingleCallWhenApproved`
    //!   * `TestBuildSwapActionExactOutputUsesMaxInput`
    //!   * `TestBuildSwapActionRejectsRecipientMismatch`
    //!   * `TestQuoteSwapRejectsNonUSDCurrency`
    //!   * `TestQuoteSwapClassifiesPairDoesNotExistAsUnsupported`
    //!
    //! Contract invariants asserted: provider metadata (no key); exact-input vs
    //! exact-output quote amounts; USD-only TIP-20 gating; revert classification
    //! to `Unsupported`; batched approve+swap step shape (`calls`) with the
    //! ERC-20 approve selector `0x095ea7b3`; single-call step when already
    //! approved; exact-output max-input slippage ceiling; recipient-must-equal-
    //! sender guard.

    use super::*;

    use alloy::dyn_abi::{FunctionExt, JsonAbiExt};
    use alloy::json_abi::{Function as JsonFunction, JsonAbi};
    use alloy::primitives::U256;
    use chrono::{TimeZone, Utc};
    use defi_id::{parse_asset, parse_chain};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    // ---- token currency map (mirror the Go `tempoTokenCurrency`) ----
    fn token_currency(token: &str) -> Option<&'static str> {
        match token.to_ascii_lowercase().as_str() {
            "0x20c0000000000000000000000000000000000000" => Some("USD"), // pathUSD
            "0x20c000000000000000000000b9537d11c60e8b50" => Some("USD"), // USDC.e
            "0x20c0000000000000000000001621e21f71cf12fb" => Some("EUR"), // EURC.e
            "0x20c00000000000000000000014f22ca97301eb73" => Some("USD"), // USDT0
            _ => None,
        }
    }

    fn json_function(abi_json: &str, name: &str) -> JsonFunction {
        let abi: JsonAbi = serde_json::from_str(abi_json).expect("parse abi");
        abi.function(name)
            .and_then(|o| o.first())
            .cloned()
            .expect("function present")
    }

    fn selector_hex(abi_json: &str, name: &str) -> String {
        hex::encode(json_function(abi_json, name).selector().0)
    }

    #[derive(Clone, Default)]
    struct MockConfig {
        allowance: u128,
        quote_exact_in: Option<u128>,
        quote_exact_out: Option<u128>,
        quote_exact_in_err: Option<String>,
        quote_exact_out_err: Option<String>,
    }

    struct RpcResponder {
        cfg: MockConfig,
        currency_sel: String,
        quote_in_sel: String,
        quote_out_sel: String,
        allowance_sel: String,
        currency_fn: JsonFunction,
        quote_in_fn: JsonFunction,
        quote_out_fn: JsonFunction,
        allowance_fn: JsonFunction,
    }

    impl RpcResponder {
        fn new(cfg: MockConfig) -> Self {
            let dex_abi = defi_registry::TEMPO_STABLECOIN_DEX_ABI;
            let erc20_abi = defi_registry::ERC20_MINIMAL_ABI;
            let tip20_abi = defi_registry::TEMPO_TIP20_METADATA_ABI;
            RpcResponder {
                currency_sel: selector_hex(tip20_abi, "currency"),
                quote_in_sel: selector_hex(dex_abi, "quoteSwapExactAmountIn"),
                quote_out_sel: selector_hex(dex_abi, "quoteSwapExactAmountOut"),
                allowance_sel: selector_hex(erc20_abi, "allowance"),
                currency_fn: json_function(tip20_abi, "currency"),
                quote_in_fn: json_function(dex_abi, "quoteSwapExactAmountIn"),
                quote_out_fn: json_function(dex_abi, "quoteSwapExactAmountOut"),
                allowance_fn: json_function(erc20_abi, "allowance"),
                cfg,
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
                return rpc_error(&id, -32601, "unsupported method");
            }
            let params = match body.get("params").and_then(|p| p.get(0)) {
                Some(p) => p,
                None => return rpc_error(&id, -32602, "missing params"),
            };
            let to = params
                .get("to")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            let data_hex = params
                .get("data")
                .or_else(|| params.get("input"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim_start_matches("0x")
                .to_string();
            let selector = data_hex.get(..8).unwrap_or("");

            if selector == self.currency_sel {
                return match token_currency(&to) {
                    Some(c) => rpc_result(
                        &id,
                        &Self::pack_output(
                            &self.currency_fn,
                            &[DynSolValue::String(c.to_string())],
                        ),
                    ),
                    None => rpc_error(&id, -32000, "execution reverted: UnknownToken"),
                };
            }
            if selector == self.quote_in_sel {
                if let Some(msg) = &self.cfg.quote_exact_in_err {
                    return rpc_error(&id, -32000, msg);
                }
                let v = self.cfg.quote_exact_in.unwrap_or(980_000);
                return rpc_result(
                    &id,
                    &Self::pack_output(&self.quote_in_fn, &[DynSolValue::Uint(U256::from(v), 128)]),
                );
            }
            if selector == self.quote_out_sel {
                if let Some(msg) = &self.cfg.quote_exact_out_err {
                    return rpc_error(&id, -32000, msg);
                }
                let v = self.cfg.quote_exact_out.unwrap_or(1_010_100);
                return rpc_result(
                    &id,
                    &Self::pack_output(
                        &self.quote_out_fn,
                        &[DynSolValue::Uint(U256::from(v), 128)],
                    ),
                );
            }
            if selector == self.allowance_sel {
                return rpc_result(
                    &id,
                    &Self::pack_output(
                        &self.allowance_fn,
                        &[DynSolValue::Uint(U256::from(self.cfg.allowance), 256)],
                    ),
                );
            }
            rpc_error(&id, -32601, "unsupported eth_call data")
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

    async fn mock_server(cfg: MockConfig) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(RpcResponder::new(cfg))
            .mount(&server)
            .await;
        server
    }

    fn client() -> Client {
        let mut c = Client::new();
        c.set_now(Utc.with_ymd_and_hms(2026, 5, 28, 12, 0, 0).unwrap());
        c
    }

    fn assets(from: &str, to: &str) -> (Chain, Asset, Asset) {
        let chain = parse_chain("tempo").expect("parse tempo chain");
        let from_asset = parse_asset(from, &chain).unwrap_or_else(|_| panic!("parse {from}"));
        let to_asset = parse_asset(to, &chain).unwrap_or_else(|_| panic!("parse {to}"));
        (chain, from_asset, to_asset)
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
        assert_eq!(info.name, "tempo");
        assert_eq!(info.provider_type, "swap");
        assert!(!info.requires_key);
        for cap in ["swap.quote", "swap.plan", "swap.execute"] {
            assert!(
                info.capabilities.iter().any(|c| c == cap),
                "missing capability {cap}"
            );
        }
    }

    // ----- TestQuoteSwapExactInput ----------------------------------------

    #[tokio::test]
    async fn quote_swap_exact_input() {
        let server = mock_server(MockConfig::default()).await;
        let (chain, from, to) = assets("pathUSD", "USDC.e");
        let quote = client()
            .quote_swap(quote_req(chain, from, to, &server.uri()))
            .await
            .expect("quote");
        assert_eq!(quote.provider, "tempo");
        assert_eq!(quote.trade_type, "exact-input");
        assert_eq!(quote.input_amount.amount_base_units, "1000000");
        assert_eq!(quote.estimated_out.amount_base_units, "980000");
        assert_eq!(quote.route, ROUTE);
    }

    // ----- TestQuoteSwapExactOutput ---------------------------------------

    #[tokio::test]
    async fn quote_swap_exact_output() {
        let server = mock_server(MockConfig::default()).await;
        let (chain, from, to) = assets("pathUSD", "USDC.e");
        let mut req = quote_req(chain, from, to, &server.uri());
        req.trade_type = SwapTradeType::ExactOutput;
        let quote = client().quote_swap(req).await.expect("quote");
        assert_eq!(quote.trade_type, "exact-output");
        assert_eq!(quote.input_amount.amount_base_units, "1010100");
        assert_eq!(quote.estimated_out.amount_base_units, "1000000");
    }

    // ----- TestBuildSwapActionBatchesApproveAndSwapForExactInput ----------

    #[tokio::test]
    async fn build_swap_action_batches_approve_and_swap_for_exact_input() {
        let server = mock_server(MockConfig::default()).await;
        let (chain, from, to) = assets("pathUSD", "USDC.e");
        let mut req = quote_req(chain, from, to, "");
        req.rpc_url = String::new();
        let opts = SwapExecutionOptions {
            sender: "0x00000000000000000000000000000000000000AA".to_string(),
            recipient: String::new(),
            slippage_bps: 100,
            simulate: true,
            rpc_url: server.uri(),
        };
        let action = client().build_swap_action(req, opts).await.expect("build");
        assert_eq!(action.provider, "tempo");
        assert_eq!(action.steps.len(), 1, "expected 1 batched step");
        let step = &action.steps[0];
        assert_eq!(step.step_id, "tempo-swap-exact-input");
        assert_eq!(step.step_type, StepType::Swap);
        assert_eq!(step.calls.len(), 2, "expected approve + swap");
        // First call is the ERC-20 approve.
        assert!(
            step.calls[0].data.starts_with("0x095ea7b3"),
            "expected approve selector, got {}",
            &step.calls[0].data[..10.min(step.calls[0].data.len())]
        );
        // Second call is the swap, with a non-empty target.
        assert!(!step.calls[1].target.is_empty());
    }

    // ----- TestBuildSwapActionSingleCallWhenApproved ----------------------

    #[tokio::test]
    async fn build_swap_action_single_call_when_approved() {
        let server = mock_server(MockConfig {
            allowance: 9_999_999,
            ..Default::default()
        })
        .await;
        let (chain, from, to) = assets("pathUSD", "USDC.e");
        let req = quote_req(chain, from, to, "");
        let opts = SwapExecutionOptions {
            sender: "0x00000000000000000000000000000000000000AA".to_string(),
            recipient: String::new(),
            slippage_bps: 100,
            simulate: true,
            rpc_url: server.uri(),
        };
        let action = client().build_swap_action(req, opts).await.expect("build");
        assert_eq!(action.steps.len(), 1);
        let step = &action.steps[0];
        assert_eq!(step.calls.len(), 1, "expected swap only");
        assert!(step.target.is_empty(), "batched step target must be empty");
        assert!(step.data.is_empty(), "batched step data must be empty");
    }

    // ----- TestBuildSwapActionExactOutputUsesMaxInput ---------------------

    #[tokio::test]
    async fn build_swap_action_exact_output_uses_max_input() {
        let server = mock_server(MockConfig::default()).await;
        let (chain, from, to) = assets("pathUSD", "USDC.e");
        let mut req = quote_req(chain, from, to, "");
        req.trade_type = SwapTradeType::ExactOutput;
        let opts = SwapExecutionOptions {
            sender: "0x00000000000000000000000000000000000000AA".to_string(),
            recipient: String::new(),
            slippage_bps: 100,
            simulate: true,
            rpc_url: server.uri(),
        };
        let action = client().build_swap_action(req, opts).await.expect("build");
        // quoted_in = 1_010_100; ceil(1_010_100 * 10100 / 10000) = 1_020_201.
        assert_eq!(action.input_amount, "1020201");
        assert_eq!(action.steps.len(), 1);
        let step = &action.steps[0];
        assert_eq!(step.step_id, "tempo-swap-exact-output");
        // With zero allowance, approve + swap.
        assert_eq!(step.calls.len(), 2);
    }

    // ----- TestBuildSwapActionRejectsRecipientMismatch --------------------

    #[tokio::test]
    async fn build_swap_action_rejects_recipient_mismatch() {
        let (chain, from, to) = assets("pathUSD", "USDC.e");
        let req = quote_req(chain, from, to, "");
        let opts = SwapExecutionOptions {
            sender: "0x00000000000000000000000000000000000000AA".to_string(),
            recipient: "0x00000000000000000000000000000000000000BB".to_string(),
            slippage_bps: 0,
            simulate: false,
            rpc_url: String::new(),
        };
        let err = client()
            .build_swap_action(req, opts)
            .await
            .expect_err("recipient mismatch must fail");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- TestQuoteSwapRejectsNonUSDCurrency -----------------------------

    #[tokio::test]
    async fn quote_swap_rejects_non_usd_currency() {
        let server = mock_server(MockConfig::default()).await;
        let (chain, from, to) = assets("USDC.e", "EURC.e");
        let err = client()
            .quote_swap(quote_req(chain, from, to, &server.uri()))
            .await
            .expect_err("non-USD pair must fail");
        assert_eq!(err.code, Code::Unsupported);
        assert!(
            err.to_string().contains("USD-denominated TIP-20s"),
            "expected USD-only guidance, got: {err}"
        );
    }

    // ----- TestQuoteSwapClassifiesPairDoesNotExistAsUnsupported -----------

    #[tokio::test]
    async fn quote_swap_classifies_pair_does_not_exist_as_unsupported() {
        let server = mock_server(MockConfig {
            quote_exact_in_err: Some("execution reverted: PairDoesNotExist".to_string()),
            ..Default::default()
        })
        .await;
        let (chain, from, to) = assets("pathUSD", "USDC.e");
        let err = client()
            .quote_swap(quote_req(chain, from, to, &server.uri()))
            .await
            .expect_err("PairDoesNotExist must fail");
        assert_eq!(err.code, Code::Unsupported);
        assert!(
            err.to_string().contains("does not support"),
            "expected pair support guidance, got: {err}"
        );
    }

    // ----- pure-helper coverage -------------------------------------------

    #[test]
    fn slippage_floor_and_ceil_match_go() {
        // floor: 1_000_000 * 9900 / 10000 = 990_000.
        assert_eq!(
            apply_slippage_floor(&BigInt::from(1_000_000u64), 100),
            BigInt::from(990_000u64)
        );
        // ceil: ceil(1_010_100 * 10100 / 10000) = 1_020_201.
        assert_eq!(
            apply_slippage_ceil(&BigInt::from(1_010_100u64), 100),
            BigInt::from(1_020_201u64)
        );
    }

    #[test]
    fn parse_uint128_rejects_non_positive_and_overflow() {
        assert!(parse_uint128("1000000").is_ok());
        assert_eq!(parse_uint128("0").unwrap_err().code, Code::Usage);
        assert_eq!(parse_uint128("-5").unwrap_err().code, Code::Usage);
        assert_eq!(parse_uint128("nope").unwrap_err().code, Code::Usage);
        // 2^128 exceeds uint128 bounds.
        let too_big = (BigInt::from(1) << 128u32).to_string();
        assert_eq!(parse_uint128(&too_big).unwrap_err().code, Code::Usage);
        // 2^128 - 1 is the max uint128 and is accepted.
        let max = ((BigInt::from(1) << 128u32) - BigInt::from(1)).to_string();
        assert!(parse_uint128(&max).is_ok());
    }

    // keep the FunctionExt/JsonAbiExt imports referenced even if the compiler
    // folds the trait methods.
    #[test]
    fn responder_constructs() {
        let _r = RpcResponder::new(MockConfig::default());
        let _ = Arc::new(1u8);
    }
}
