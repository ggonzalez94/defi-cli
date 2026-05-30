//! Moonwell provider adapter — lending markets/rates/positions + yield
//! opportunities/positions, backed by on-chain RPC reads (Compound v2 style).
//!
//! Go source: `internal/providers/moonwell/client.go` (+ `client_test.go`).
//!
//! Implements the `LendingProvider` (markets/rates), `LendingPositionsProvider`,
//! `YieldProvider`, and `YieldPositionsProvider` trait surfaces, plus `Provider`
//! metadata. Moonwell is the only fully on-chain read adapter: it talks to the
//! chain's Comptroller (Unitroller), mToken contracts, and price oracle via
//! `eth_call`, batching per-market reads through Multicall3 (`aggregate3`). No
//! API key is required; supported on Base (`8453`) and Optimism (`10`).
//!
//! All outputs are deterministic (stable multi-key sorts). Every APY field is a
//! PERCENTAGE POINT, not a ratio (spec §2.5): the linear rate-per-timestamp is
//! annualized and scaled ×100. Amounts carry both base units and decimal forms.

use std::collections::HashSet;

use alloy::dyn_abi::DynSolValue;
use alloy::primitives::{Address as AlloyAddress, U256};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_evm::abi::Function;
use defi_evm::address;
use defi_evm::rpc::{CallRequest, RpcClient};
use defi_id::{format_decimal, Asset, Chain};
use defi_model as model;
use num_bigint::{BigInt, Sign};
use sha1::{Digest, Sha1};

use crate::traits::{
    LendPositionType, LendPositionsRequest, LendingPositionsProvider, LendingProvider, Provider,
    YieldPositionsProvider, YieldPositionsRequest, YieldProvider, YieldRequest,
};
use crate::yieldutil;

const SECONDS_PER_YEAR: f64 = 365.25 * 24.0 * 3600.0;
const SOURCE_URL: &str = "https://moonwell.fi";

/// Multicall3 is deployed at this standard address on all major EVM chains.
const MULTICALL3_ADDR: &str = "0xca11bde05977b3631167028862be2a173976ca11";

/// Number of multicall sub-calls per mToken in the markets phase 1 read.
/// Order: underlying, supplyRate, borrowRate, totalSupply, exchangeRate,
/// totalBorrows, getCash, price.
const CALLS_PER_MARKET_PHASE1: usize = 8;
/// Number of multicall sub-calls per mToken in the positions phase 1 read.
/// Order: snapshot, underlying, supplyRate, borrowRate, price.
const POS_CALLS_PER_MARKET: usize = 5;

/// Moonwell lending + yield adapter (mirrors Go `moonwell.Client`).
pub struct Client {
    /// Injected fixed clock for deterministic `fetched_at`; `None` uses the wall
    /// clock.
    now: Option<DateTime<Utc>>,
    /// Test seam: point on-chain reads at a mock RPC server. Empty falls back to
    /// the registry default for the chain.
    rpc_override: String,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Build a client using the default chain RPC map (mirrors Go `New()`).
    pub fn new() -> Self {
        Client {
            now: None,
            rpc_override: String::new(),
        }
    }

    /// Override the RPC URL used for on-chain reads (test seam for Go
    /// `SetRPCOverride`). Pass `""` to revert to the default.
    pub fn set_rpc_override(&mut self, url: &str) {
        self.rpc_override = url.to_string();
    }

    /// Pin the clock (test seam for Go `client.now`).
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

    /// Resolve the RPC URL + comptroller address for a chain, then connect.
    fn resolve(&self, chain: &Chain, rpc_override: &str) -> Result<(RpcClient, String), Error> {
        if !chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "moonwell supports only EVM chains",
            ));
        }
        let rpc_url = defi_registry::resolve_rpc_url(rpc_override, chain.evm_chain_id)
            .map_err(|e| Error::wrap(Code::Unsupported, "resolve rpc url", e))?;
        let comptroller =
            defi_registry::moonwell_comptroller(chain.evm_chain_id).ok_or_else(|| {
                Error::new(Code::Unsupported, "moonwell is not supported on this chain")
            })?;
        let client = RpcClient::connect(&rpc_url)
            .map_err(|e| Error::wrap(Code::Unavailable, "connect rpc", e))?;
        Ok((client, comptroller.to_string()))
    }

    /// Fetch the full market list for a chain: `(markets, comptroller_address)`.
    async fn fetch_markets(
        &self,
        chain: &Chain,
        rpc_override: &str,
    ) -> Result<(Vec<MoonwellMarket>, String), Error> {
        let (client, comptroller_addr) = self.resolve(chain, rpc_override)?;
        let comptroller = parse_addr(&comptroller_addr)?;

        let comptroller_fns = ComptrollerFns::build()?;
        let mtoken_fns = MTokenFns::build()?;
        let oracle_fns = OracleFns::build()?;
        let erc20_fns = Erc20Fns::build()?;
        let agg = aggregate3_fn()?;

        let m_tokens =
            call_get_all_markets(&client, &comptroller_fns.get_all_markets, comptroller).await?;
        if m_tokens.is_empty() {
            return Ok((Vec::new(), comptroller_addr));
        }
        let oracle = call_oracle(&client, &comptroller_fns.oracle, comptroller).await?;

        // Phase 1 multicall: per-mToken data.
        let underlying_cd = encode_call(&mtoken_fns.underlying, &[])?;
        let supply_rate_cd = encode_call(&mtoken_fns.supply_rate, &[])?;
        let borrow_rate_cd = encode_call(&mtoken_fns.borrow_rate, &[])?;
        let total_supply_cd = encode_call(&mtoken_fns.total_supply, &[])?;
        let exchange_rate_cd = encode_call(&mtoken_fns.exchange_rate, &[])?;
        let total_borrows_cd = encode_call(&mtoken_fns.total_borrows, &[])?;
        let get_cash_cd = encode_call(&mtoken_fns.get_cash, &[])?;

        let mut phase1_calls: Vec<Mc3Call> =
            Vec::with_capacity(m_tokens.len() * CALLS_PER_MARKET_PHASE1);
        for mt in &m_tokens {
            let price_cd = encode_call(
                &oracle_fns.get_underlying_price,
                &[DynSolValue::Address(*mt)],
            )?;
            phase1_calls.push(Mc3Call::new(*mt, underlying_cd.clone()));
            phase1_calls.push(Mc3Call::new(*mt, supply_rate_cd.clone()));
            phase1_calls.push(Mc3Call::new(*mt, borrow_rate_cd.clone()));
            phase1_calls.push(Mc3Call::new(*mt, total_supply_cd.clone()));
            phase1_calls.push(Mc3Call::new(*mt, exchange_rate_cd.clone()));
            phase1_calls.push(Mc3Call::new(*mt, total_borrows_cd.clone()));
            phase1_calls.push(Mc3Call::new(*mt, get_cash_cd.clone()));
            phase1_calls.push(Mc3Call::new(oracle, price_cd));
        }

        let phase1_results = exec_multicall3(&client, &agg, phase1_calls)
            .await
            .map_err(|e| Error::wrap(Code::Unavailable, "multicall market data", e))?;

        struct Phase1Data {
            underlying: AlloyAddress,
            supply_rate: BigInt,
            borrow_rate: BigInt,
            total_supply: BigInt,
            exchange_rate: BigInt,
            total_borrows: BigInt,
            cash: BigInt,
            price_mantissa: BigInt,
        }

        let mut p1_parsed: Vec<Phase1Data> = Vec::with_capacity(m_tokens.len());
        for (i, mt) in m_tokens.iter().enumerate() {
            let base = i * CALLS_PER_MARKET_PHASE1;
            let r = &phase1_results[base..base + CALLS_PER_MARKET_PHASE1];

            let underlying = match decode_address_result(&r[0], &mtoken_fns.underlying) {
                Some(a) => a,
                None => continue,
            };

            p1_parsed.push(Phase1Data {
                underlying,
                supply_rate: decode_uint256_result(&r[1], &mtoken_fns.supply_rate),
                borrow_rate: decode_uint256_result(&r[2], &mtoken_fns.borrow_rate),
                total_supply: decode_uint256_result(&r[3], &mtoken_fns.total_supply),
                exchange_rate: decode_uint256_result(&r[4], &mtoken_fns.exchange_rate),
                total_borrows: decode_uint256_result(&r[5], &mtoken_fns.total_borrows),
                cash: decode_uint256_result(&r[6], &mtoken_fns.get_cash),
                price_mantissa: decode_uint256_result(&r[7], &oracle_fns.get_underlying_price),
            });
        }

        if p1_parsed.is_empty() {
            return Ok((Vec::new(), comptroller_addr));
        }

        // Phase 2 multicall: symbol() + decimals() on each underlying.
        let symbol_cd = encode_call(&erc20_fns.symbol, &[])?;
        let decimals_cd = encode_call(&erc20_fns.decimals, &[])?;
        let mut phase2_calls: Vec<Mc3Call> = Vec::with_capacity(p1_parsed.len() * 2);
        for p in &p1_parsed {
            phase2_calls.push(Mc3Call::new(p.underlying, symbol_cd.clone()));
            phase2_calls.push(Mc3Call::new(p.underlying, decimals_cd.clone()));
        }

        let phase2_results = exec_multicall3(&client, &agg, phase2_calls)
            .await
            .map_err(|e| Error::wrap(Code::Unavailable, "multicall token metadata", e))?;

        let mut markets: Vec<MoonwellMarket> = Vec::with_capacity(p1_parsed.len());
        for (i, p) in p1_parsed.iter().enumerate() {
            let base = i * 2;
            let symbol = decode_string_result(&phase2_results[base], &erc20_fns.symbol);
            let decimals = decode_decimals_result(&phase2_results[base + 1], &erc20_fns.decimals);
            if symbol.is_empty() || decimals == 0 {
                continue;
            }

            let price_usd = mantissa_to_usd(&p.price_mantissa, decimals);

            // TVL = totalSupply(mTokens) * exchangeRate / 1e18 -> underlying
            // units, then * priceUSD.
            let underlying_total = scaled_div_1e18(&p.total_supply, &p.exchange_rate);
            let tvl_usd = bigint_to_float(&underlying_total, decimals) * price_usd;
            let total_borrows_usd = bigint_to_float(&p.total_borrows, decimals) * price_usd;
            let liquidity_usd = bigint_to_float(&p.cash, decimals) * price_usd;
            let utilization = if tvl_usd > 0.0 {
                total_borrows_usd / tvl_usd
            } else {
                0.0
            };

            markets.push(MoonwellMarket {
                underlying_address: lower_hex(&p.underlying),
                underlying_symbol: symbol,
                supply_apy: rate_to_apy(&p.supply_rate),
                borrow_apy: rate_to_apy(&p.borrow_rate),
                tvl_usd,
                liquidity_usd,
                utilization,
            });
        }

        Ok((markets, comptroller_addr))
    }
}

impl Provider for Client {
    fn info(&self) -> model::ProviderInfo {
        model::ProviderInfo {
            name: "moonwell".to_string(),
            provider_type: "lending+yield".to_string(),
            requires_key: false,
            capabilities: vec![
                "lend.markets".to_string(),
                "lend.rates".to_string(),
                "lend.positions".to_string(),
                "yield.opportunities".to_string(),
                "yield.positions".to_string(),
                "lend.plan".to_string(),
                "lend.execute".to_string(),
                "yield.plan".to_string(),
                "yield.execute".to_string(),
            ],
            key_env_var_name: String::new(),
            capability_auth: Vec::new(),
        }
    }
}

#[async_trait]
impl LendingProvider for Client {
    async fn lend_markets(
        &self,
        _provider: &str,
        chain: Chain,
        asset: Asset,
    ) -> Result<Vec<model::LendMarket>, Error> {
        if !chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "moonwell supports only EVM chains",
            ));
        }
        let (markets, comptroller) = self.fetch_markets(&chain, &self.rpc_override).await?;

        let mut out: Vec<model::LendMarket> = Vec::with_capacity(markets.len());
        for m in &markets {
            if !matches_asset(&m.underlying_address, &m.underlying_symbol, &asset) {
                continue;
            }
            let asset_id = canonical_asset_id_for_chain(&chain.caip2, &m.underlying_address);
            if asset_id.is_empty() {
                continue;
            }
            let native_id = provider_native_id(
                "moonwell",
                &chain.caip2,
                &comptroller,
                &m.underlying_address,
            );
            out.push(model::LendMarket {
                protocol: "moonwell".to_string(),
                provider: "moonwell".to_string(),
                chain_id: chain.caip2.clone(),
                asset_id,
                provider_native_id: native_id,
                provider_native_id_kind: model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET.to_string(),
                supply_apy: m.supply_apy,
                borrow_apy: m.borrow_apy,
                tvl_usd: m.tvl_usd,
                liquidity_usd: m.liquidity_usd,
                source_url: SOURCE_URL.to_string(),
                fetched_at: self.fetched_at(),
            });
        }

        out.sort_by(|a, b| {
            desc_f64(a.tvl_usd, b.tvl_usd).then_with(|| a.asset_id.cmp(&b.asset_id))
        });
        Ok(out)
    }

    async fn lend_rates(
        &self,
        _provider: &str,
        chain: Chain,
        asset: Asset,
    ) -> Result<Vec<model::LendRate>, Error> {
        if !chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "moonwell supports only EVM chains",
            ));
        }
        let (markets, comptroller) = self.fetch_markets(&chain, &self.rpc_override).await?;

        let mut out: Vec<model::LendRate> = Vec::with_capacity(markets.len());
        for m in &markets {
            if !matches_asset(&m.underlying_address, &m.underlying_symbol, &asset) {
                continue;
            }
            let asset_id = canonical_asset_id_for_chain(&chain.caip2, &m.underlying_address);
            if asset_id.is_empty() {
                continue;
            }
            let native_id = provider_native_id(
                "moonwell",
                &chain.caip2,
                &comptroller,
                &m.underlying_address,
            );
            out.push(model::LendRate {
                protocol: "moonwell".to_string(),
                provider: "moonwell".to_string(),
                chain_id: chain.caip2.clone(),
                asset_id,
                provider_native_id: native_id,
                provider_native_id_kind: model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET.to_string(),
                supply_apy: m.supply_apy,
                borrow_apy: m.borrow_apy,
                utilization: m.utilization,
                source_url: SOURCE_URL.to_string(),
                fetched_at: self.fetched_at(),
            });
        }

        out.sort_by(|a, b| {
            desc_f64(a.supply_apy, b.supply_apy).then_with(|| a.asset_id.cmp(&b.asset_id))
        });
        Ok(out)
    }
}

#[async_trait]
impl LendingPositionsProvider for Client {
    async fn lend_positions(
        &self,
        req: LendPositionsRequest,
    ) -> Result<Vec<model::LendPosition>, Error> {
        if !req.chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "moonwell supports only EVM chains",
            ));
        }
        let account = normalize_evm_address(&req.account);
        if account.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "lend positions requires a valid EVM address",
            ));
        }

        let rpc_override = if req.rpc_url.is_empty() {
            self.rpc_override.clone()
        } else {
            req.rpc_url.clone()
        };
        let (client, comptroller_addr) = self.resolve(&req.chain, &rpc_override)?;
        let comptroller = parse_addr(&comptroller_addr)?;
        let account_addr = parse_addr(&account)?;

        let comptroller_fns = ComptrollerFns::build()?;
        let mtoken_fns = MTokenFns::build()?;
        let oracle_fns = OracleFns::build()?;
        let erc20_fns = Erc20Fns::build()?;
        let agg = aggregate3_fn()?;

        // Three sequential reads: markets, collateral set, oracle.
        let all_markets =
            call_get_all_markets(&client, &comptroller_fns.get_all_markets, comptroller).await?;
        let collateral_set = call_get_assets_in(
            &client,
            &comptroller_fns.get_assets_in,
            comptroller,
            account_addr,
        )
        .await?;
        let oracle = call_oracle(&client, &comptroller_fns.oracle, comptroller).await?;

        // Phase 1 multicall, per mToken: snapshot, underlying, supplyRate,
        // borrowRate, price.
        let underlying_cd = encode_call(&mtoken_fns.underlying, &[])?;
        let supply_rate_cd = encode_call(&mtoken_fns.supply_rate, &[])?;
        let borrow_rate_cd = encode_call(&mtoken_fns.borrow_rate, &[])?;
        let snapshot_cd = encode_call(
            &mtoken_fns.get_account_snapshot,
            &[DynSolValue::Address(account_addr)],
        )?;

        let mut snapshot_calls: Vec<Mc3Call> =
            Vec::with_capacity(all_markets.len() * POS_CALLS_PER_MARKET);
        for mt in &all_markets {
            let price_cd = encode_call(
                &oracle_fns.get_underlying_price,
                &[DynSolValue::Address(*mt)],
            )?;
            snapshot_calls.push(Mc3Call::new(*mt, snapshot_cd.clone()));
            snapshot_calls.push(Mc3Call::new(*mt, underlying_cd.clone()));
            snapshot_calls.push(Mc3Call::new(*mt, supply_rate_cd.clone()));
            snapshot_calls.push(Mc3Call::new(*mt, borrow_rate_cd.clone()));
            snapshot_calls.push(Mc3Call::new(oracle, price_cd));
        }

        let phase1_results = exec_multicall3(&client, &agg, snapshot_calls)
            .await
            .map_err(|e| Error::wrap(Code::Unavailable, "multicall positions", e))?;

        struct PosMarket {
            m_token: AlloyAddress,
            underlying: AlloyAddress,
            m_token_bal: BigInt,
            borrow_bal: BigInt,
            exchange_rate: BigInt,
            supply_rate: BigInt,
            borrow_rate: BigInt,
            price_mantissa: BigInt,
        }

        let mut pos_markets: Vec<PosMarket> = Vec::new();
        for (i, mt) in all_markets.iter().enumerate() {
            let base = i * POS_CALLS_PER_MARKET;
            let r = &phase1_results[base..base + POS_CALLS_PER_MARKET];

            // getAccountSnapshot -> (errCode, mTokenBal, borrowBal, exchangeRate).
            let snap = match decode_result(&r[0], &mtoken_fns.get_account_snapshot) {
                Some(values) if values.len() >= 4 => values,
                _ => continue,
            };
            let err_code = dyn_uint_to_bigint(&snap[0]);
            let m_token_bal = dyn_uint_to_bigint(&snap[1]);
            let borrow_bal = dyn_uint_to_bigint(&snap[2]);
            let exchange_rate = dyn_uint_to_bigint(&snap[3]);

            if err_code.sign() != Sign::NoSign
                || (m_token_bal.sign() == Sign::NoSign && borrow_bal.sign() == Sign::NoSign)
            {
                continue;
            }

            let underlying = match decode_address_result(&r[1], &mtoken_fns.underlying) {
                Some(a) => a,
                None => continue,
            };

            pos_markets.push(PosMarket {
                m_token: *mt,
                underlying,
                m_token_bal,
                borrow_bal,
                exchange_rate,
                supply_rate: decode_uint256_result(&r[2], &mtoken_fns.supply_rate),
                borrow_rate: decode_uint256_result(&r[3], &mtoken_fns.borrow_rate),
                price_mantissa: decode_uint256_result(&r[4], &oracle_fns.get_underlying_price),
            });
        }

        if pos_markets.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 2: symbol + decimals for each underlying.
        let symbol_cd = encode_call(&erc20_fns.symbol, &[])?;
        let decimals_cd = encode_call(&erc20_fns.decimals, &[])?;
        let mut phase2_calls: Vec<Mc3Call> = Vec::with_capacity(pos_markets.len() * 2);
        for pm in &pos_markets {
            phase2_calls.push(Mc3Call::new(pm.underlying, symbol_cd.clone()));
            phase2_calls.push(Mc3Call::new(pm.underlying, decimals_cd.clone()));
        }

        let phase2_results = exec_multicall3(&client, &agg, phase2_calls)
            .await
            .map_err(|e| Error::wrap(Code::Unavailable, "multicall position metadata", e))?;

        let filter_type = req.position_type;
        let mut out: Vec<model::LendPosition> = Vec::new();

        for (i, pm) in pos_markets.iter().enumerate() {
            let base = i * 2;
            let symbol = decode_string_result(&phase2_results[base], &erc20_fns.symbol);
            let decimals = decode_decimals_result(&phase2_results[base + 1], &erc20_fns.decimals);
            if symbol.is_empty() || decimals == 0 {
                continue;
            }

            let ul_addr = lower_hex(&pm.underlying);
            if !matches_asset(&ul_addr, &symbol, &req.asset) {
                continue;
            }
            let asset_id = canonical_asset_id_for_chain(&req.chain.caip2, &ul_addr);
            if asset_id.is_empty() {
                continue;
            }
            let native_id =
                provider_native_id("moonwell", &req.chain.caip2, &comptroller_addr, &ul_addr);
            let price_usd = mantissa_to_usd(&pm.price_mantissa, decimals);

            // Supply position.
            if pm.m_token_bal.sign() == Sign::Plus {
                let underlying_bal = scaled_div_1e18(&pm.m_token_bal, &pm.exchange_rate);
                let pos_type = if collateral_set.contains(&pm.m_token) {
                    LendPositionType::Collateral
                } else {
                    LendPositionType::Supply
                };
                if matches_position_type(filter_type, pos_type) {
                    let amount_usd = bigint_to_float(&underlying_bal, decimals) * price_usd;
                    out.push(model::LendPosition {
                        protocol: "moonwell".to_string(),
                        provider: "moonwell".to_string(),
                        chain_id: req.chain.caip2.clone(),
                        account_address: account.clone(),
                        position_type: pos_type.as_str().to_string(),
                        asset_id: asset_id.clone(),
                        provider_native_id: native_id.clone(),
                        provider_native_id_kind: model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET
                            .to_string(),
                        amount: amount_info_from_bigint(&underlying_bal, decimals),
                        amount_usd,
                        apy: rate_to_apy(&pm.supply_rate),
                        source_url: SOURCE_URL.to_string(),
                        fetched_at: self.fetched_at(),
                    });
                }
            }

            // Borrow position.
            if pm.borrow_bal.sign() == Sign::Plus
                && matches_position_type(filter_type, LendPositionType::Borrow)
            {
                let amount_usd = bigint_to_float(&pm.borrow_bal, decimals) * price_usd;
                out.push(model::LendPosition {
                    protocol: "moonwell".to_string(),
                    provider: "moonwell".to_string(),
                    chain_id: req.chain.caip2.clone(),
                    account_address: account.clone(),
                    position_type: LendPositionType::Borrow.as_str().to_string(),
                    asset_id: asset_id.clone(),
                    provider_native_id: native_id.clone(),
                    provider_native_id_kind: model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET
                        .to_string(),
                    amount: amount_info_from_bigint(&pm.borrow_bal, decimals),
                    amount_usd,
                    apy: rate_to_apy(&pm.borrow_rate),
                    source_url: SOURCE_URL.to_string(),
                    fetched_at: self.fetched_at(),
                });
            }
        }

        sort_lend_positions(&mut out);
        if req.limit > 0 && (out.len() as i64) > req.limit {
            out.truncate(req.limit as usize);
        }
        Ok(out)
    }
}

#[async_trait]
impl YieldProvider for Client {
    async fn yield_opportunities(
        &self,
        req: YieldRequest,
    ) -> Result<Vec<model::YieldOpportunity>, Error> {
        let (markets, comptroller) = self.fetch_markets(&req.chain, &self.rpc_override).await?;

        let mut out: Vec<model::YieldOpportunity> = Vec::with_capacity(markets.len());
        for m in &markets {
            if !matches_asset(&m.underlying_address, &m.underlying_symbol, &req.asset) {
                continue;
            }
            if (m.supply_apy == 0.0 || m.tvl_usd == 0.0) && !req.include_incomplete {
                continue;
            }
            if m.supply_apy < req.min_apy {
                continue;
            }
            if m.tvl_usd < req.min_tvl_usd {
                continue;
            }

            let asset_id = canonical_asset_id_for_chain(&req.chain.caip2, &m.underlying_address);
            if asset_id.is_empty() {
                continue;
            }
            let native_id = provider_native_id(
                "moonwell",
                &req.chain.caip2,
                &comptroller,
                &m.underlying_address,
            );
            let opportunity_id =
                hash_opportunity("moonwell", &req.chain.caip2, &native_id, &asset_id);

            out.push(model::YieldOpportunity {
                opportunity_id,
                provider: "moonwell".to_string(),
                protocol: "moonwell".to_string(),
                chain_id: req.chain.caip2.clone(),
                asset_id: asset_id.clone(),
                provider_native_id: native_id,
                provider_native_id_kind: model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET.to_string(),
                opportunity_type: "lend".to_string(),
                apy_base: m.supply_apy,
                apy_reward: 0.0,
                apy_total: m.supply_apy,
                tvl_usd: m.tvl_usd,
                liquidity_usd: m.liquidity_usd,
                lockup_days: 0.0,
                withdrawal_terms: "variable".to_string(),
                backing_assets: vec![model::YieldBackingAsset {
                    asset_id,
                    symbol: m.underlying_symbol.clone(),
                    share_pct: 100.0,
                }],
                source_url: SOURCE_URL.to_string(),
                fetched_at: self.fetched_at(),
            });
        }

        if out.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "no moonwell yield opportunities for requested chain/asset",
            ));
        }
        yieldutil::sort_opportunities(&mut out, &req.sort_by);
        let limit = if req.limit <= 0 || req.limit > out.len() as i64 {
            out.len()
        } else {
            req.limit as usize
        };
        out.truncate(limit);
        Ok(out)
    }
}

#[async_trait]
impl YieldPositionsProvider for Client {
    async fn yield_positions(
        &self,
        req: YieldPositionsRequest,
    ) -> Result<Vec<model::YieldPosition>, Error> {
        let lend_rows = self
            .lend_positions(LendPositionsRequest {
                chain: req.chain.clone(),
                account: req.account.clone(),
                asset: req.asset.clone(),
                position_type: LendPositionType::All,
                limit: req.limit,
                rpc_url: req.rpc_url.clone(),
            })
            .await?;

        let mut out: Vec<model::YieldPosition> = Vec::with_capacity(lend_rows.len());
        for row in &lend_rows {
            match row.position_type.as_str() {
                "supply" | "collateral" => {}
                _ => continue,
            }
            let opportunity_id = if row.provider_native_id.trim().is_empty() {
                String::new()
            } else {
                hash_opportunity(
                    "moonwell",
                    &row.chain_id,
                    &row.provider_native_id,
                    &row.asset_id,
                )
            };
            out.push(model::YieldPosition {
                protocol: "moonwell".to_string(),
                provider: "moonwell".to_string(),
                chain_id: row.chain_id.clone(),
                account_address: row.account_address.clone(),
                position_type: "deposit".to_string(),
                opportunity_id,
                asset_id: row.asset_id.clone(),
                provider_native_id: row.provider_native_id.clone(),
                provider_native_id_kind: row.provider_native_id_kind.clone(),
                amount: row.amount.clone(),
                shares: None,
                amount_usd: row.amount_usd,
                apy_total: row.apy,
                source_url: row.source_url.clone(),
                fetched_at: row.fetched_at.clone(),
            });
        }

        sort_yield_positions(&mut out);
        if req.limit > 0 && (out.len() as i64) > req.limit {
            out.truncate(req.limit as usize);
        }
        Ok(out)
    }
}

// ── internal market struct ──────────────────────────────────────────────

struct MoonwellMarket {
    underlying_address: String,
    underlying_symbol: String,
    supply_apy: f64,
    borrow_apy: f64,
    tvl_usd: f64,
    liquidity_usd: f64,
    utilization: f64,
}

// ── Multicall + RPC call plumbing ───────────────────────────────────────

/// A single Multicall3 sub-call (`allowFailure` is always `true`, matching the
/// Go fixtures).
struct Mc3Call {
    target: AlloyAddress,
    calldata: Vec<u8>,
}

impl Mc3Call {
    fn new(target: AlloyAddress, calldata: Vec<u8>) -> Self {
        Mc3Call { target, calldata }
    }
}

/// One decoded Multicall3 result (`success`, `returnData`).
struct Mc3Result {
    success: bool,
    return_data: Vec<u8>,
}

/// Batch multiple contract calls into a single `Multicall3.aggregate3` call.
async fn exec_multicall3(
    client: &RpcClient,
    agg: &Function,
    calls: Vec<Mc3Call>,
) -> Result<Vec<Mc3Result>, Error> {
    if calls.is_empty() {
        return Ok(Vec::new());
    }

    let tuples: Vec<DynSolValue> = calls
        .iter()
        .map(|c| {
            DynSolValue::Tuple(vec![
                DynSolValue::Address(c.target),
                DynSolValue::Bool(true),
                DynSolValue::Bytes(c.calldata.clone()),
            ])
        })
        .collect();
    let data = agg
        .encode(&[DynSolValue::Array(tuples)])
        .map_err(|e| Error::wrap(Code::Internal, "pack aggregate3", e))?;

    let mc3 = parse_addr(MULTICALL3_ADDR)?;
    let request = CallRequest::new(None, Some(mc3.into()), U256::ZERO, data);
    let out = client
        .call(&request)
        .await
        .map_err(|e| Error::wrap(Code::Unavailable, "call aggregate3", e))?;

    let decoded = agg
        .decode_output(&out)
        .map_err(|e| Error::wrap(Code::Unavailable, "decode aggregate3", e))?;
    let arr = decoded
        .first()
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::new(Code::Unavailable, "empty aggregate3 response"))?;

    let mut results = Vec::with_capacity(arr.len());
    for item in arr {
        let tuple = item
            .as_tuple()
            .ok_or_else(|| Error::new(Code::Unavailable, "invalid aggregate3 result tuple"))?;
        let success = tuple.first().and_then(|v| v.as_bool()).unwrap_or(false);
        let return_data = tuple
            .get(1)
            .and_then(|v| v.as_bytes())
            .map(|b| b.to_vec())
            .unwrap_or_default();
        results.push(Mc3Result {
            success,
            return_data,
        });
    }
    Ok(results)
}

async fn call_get_all_markets(
    client: &RpcClient,
    func: &Function,
    comptroller: AlloyAddress,
) -> Result<Vec<AlloyAddress>, Error> {
    let data =
        encode_call(func, &[]).map_err(|e| Error::wrap(Code::Internal, "pack getAllMarkets", e))?;
    let out = single_call(client, comptroller, data, "getAllMarkets").await?;
    decode_address_array(&out, func, "getAllMarkets")
}

async fn call_get_assets_in(
    client: &RpcClient,
    func: &Function,
    comptroller: AlloyAddress,
    account: AlloyAddress,
) -> Result<HashSet<AlloyAddress>, Error> {
    let data = encode_call(func, &[DynSolValue::Address(account)])
        .map_err(|e| Error::wrap(Code::Internal, "pack getAssetsIn", e))?;
    let out = single_call(client, comptroller, data, "getAssetsIn").await?;
    let addrs = decode_address_array(&out, func, "getAssetsIn")?;
    Ok(addrs.into_iter().collect())
}

async fn call_oracle(
    client: &RpcClient,
    func: &Function,
    comptroller: AlloyAddress,
) -> Result<AlloyAddress, Error> {
    let data = encode_call(func, &[]).map_err(|e| Error::wrap(Code::Internal, "pack oracle", e))?;
    let out = single_call(client, comptroller, data, "oracle").await?;
    let decoded = func
        .decode_output(&out)
        .map_err(|_| Error::new(Code::Unavailable, "decode oracle"))?;
    decoded
        .first()
        .and_then(|v| v.as_address())
        .ok_or_else(|| Error::new(Code::Unavailable, "invalid oracle response"))
}

/// Perform a single `eth_call` against `target` with `data`.
async fn single_call(
    client: &RpcClient,
    target: AlloyAddress,
    data: Vec<u8>,
    ctx: &'static str,
) -> Result<Vec<u8>, Error> {
    let request = CallRequest::new(None, Some(target.into()), U256::ZERO, data);
    client
        .call(&request)
        .await
        .map_err(|e| Error::wrap(Code::Unavailable, ctx, e))
}

fn decode_address_array(
    out: &[u8],
    func: &Function,
    ctx: &'static str,
) -> Result<Vec<AlloyAddress>, Error> {
    let decoded = func
        .decode_output(out)
        .map_err(|_| Error::new(Code::Unavailable, ctx))?;
    let arr = decoded
        .first()
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::new(Code::Unavailable, ctx))?;
    let mut addrs = Vec::with_capacity(arr.len());
    for item in arr {
        if let Some(a) = item.as_address() {
            addrs.push(a);
        }
    }
    Ok(addrs)
}

// ── decode helpers ──────────────────────────────────────────────────────

/// Decode a multicall result into typed values, or `None` when the sub-call
/// failed / returned too little data.
fn decode_result(r: &Mc3Result, func: &Function) -> Option<Vec<DynSolValue>> {
    if !r.success || r.return_data.len() < 32 {
        return None;
    }
    func.decode_output(&r.return_data).ok()
}

/// Decode a single `uint256` from a multicall result; `0` on any failure.
fn decode_uint256_result(r: &Mc3Result, func: &Function) -> BigInt {
    match decode_result(r, func) {
        Some(values) => values.first().map(dyn_uint_to_bigint).unwrap_or_default(),
        None => BigInt::default(),
    }
}

/// Decode a single `address` from a multicall result.
fn decode_address_result(r: &Mc3Result, func: &Function) -> Option<AlloyAddress> {
    decode_result(r, func).and_then(|values| values.first().and_then(|v| v.as_address()))
}

/// Decode a single `string` from a multicall result; empty on any failure.
fn decode_string_result(r: &Mc3Result, func: &Function) -> String {
    decode_result(r, func)
        .and_then(|values| values.first().and_then(|v| v.as_str().map(str::to_string)))
        .unwrap_or_default()
}

/// Decode a single `uint8` decimals value from a multicall result; `0` on
/// failure.
fn decode_decimals_result(r: &Mc3Result, func: &Function) -> i32 {
    decode_result(r, func)
        .and_then(|values| values.first().and_then(|v| v.as_uint()))
        .map(|(n, _)| u256_to_i32(n))
        .unwrap_or(0)
}

/// Convert a `DynSolValue::Uint` into a `BigInt` (`0` for any other variant).
fn dyn_uint_to_bigint(v: &DynSolValue) -> BigInt {
    match v.as_uint() {
        Some((n, _)) => u256_to_bigint(n),
        None => BigInt::default(),
    }
}

fn u256_to_bigint(n: U256) -> BigInt {
    BigInt::from_bytes_be(Sign::Plus, &n.to_be_bytes::<32>())
}

fn u256_to_i32(n: U256) -> i32 {
    // decimals are tiny (≤ 255); clamp to i32 for the rare overflow.
    i32::try_from(n).unwrap_or(0)
}

// ── numeric helpers (mirror the Go big.Float math) ──────────────────────

/// APY ≈ ratePerSecond * secondsPerYear / 1e18 * 100 (linear approximation).
fn rate_to_apy(rate_per_timestamp: &BigInt) -> f64 {
    if rate_per_timestamp.sign() == Sign::NoSign {
        return 0.0;
    }
    let rate = bigint_to_f64(rate_per_timestamp);
    let result = rate * SECONDS_PER_YEAR / 1e18 * 100.0;
    if result.is_nan() || result.is_infinite() {
        return 0.0;
    }
    result
}

/// Convert a base-unit `BigInt` to a decimal float by dividing by `10^decimals`.
fn bigint_to_float(v: &BigInt, decimals: i32) -> f64 {
    if v.sign() == Sign::NoSign {
        return 0.0;
    }
    bigint_to_f64(v) / 10f64.powi(decimals)
}

/// Moonwell oracle price mantissa -> USD float. The oracle returns price scaled
/// by `10^(36 - underlyingDecimals)`.
fn mantissa_to_usd(price_mantissa: &BigInt, underlying_decimals: i32) -> f64 {
    if price_mantissa.sign() == Sign::NoSign {
        return 0.0;
    }
    let scale_pow = (36 - underlying_decimals).max(0);
    bigint_to_f64(price_mantissa) / 10f64.powi(scale_pow)
}

/// `a * b / 1e18` in integer arithmetic (matches Go's `big.Int` exchange-rate
/// scaling, truncating toward zero).
fn scaled_div_1e18(a: &BigInt, b: &BigInt) -> BigInt {
    let scale = BigInt::from(10u64).pow(18);
    (a * b) / scale
}

/// Convert a `BigInt` to `f64` via its decimal string. Matches Go's
/// `big.Float.SetInt(...).Float64()` for the magnitudes seen here.
fn bigint_to_f64(v: &BigInt) -> f64 {
    v.to_string().parse::<f64>().unwrap_or(0.0)
}

// ── id / formatting helpers ─────────────────────────────────────────────

fn lower_hex(addr: &AlloyAddress) -> String {
    format!("0x{:x}", addr)
}

fn parse_addr(addr: &str) -> Result<AlloyAddress, Error> {
    address::parse(addr).map(|a| a.into_inner())
}

fn amount_info_from_bigint(v: &BigInt, decimals: i32) -> model::AmountInfo {
    let base = v.to_string();
    model::AmountInfo {
        amount_decimal: format_decimal(&base, decimals),
        amount_base_units: base,
        decimals: decimals as i64,
    }
}

fn normalize_evm_address(address: &str) -> String {
    let addr = address.trim().to_ascii_lowercase();
    if addr.len() != 42 || !addr.starts_with("0x") {
        return String::new();
    }
    addr
}

fn canonical_asset_id_for_chain(chain_id: &str, address: &str) -> String {
    let addr = normalize_evm_address(address);
    if chain_id.is_empty() || addr.is_empty() {
        return String::new();
    }
    format!("{chain_id}/erc20:{addr}")
}

fn provider_native_id(
    provider: &str,
    chain_id: &str,
    comptroller_address: &str,
    underlying_address: &str,
) -> String {
    format!(
        "{provider}:{chain_id}:{}:{}",
        normalize_evm_address(comptroller_address),
        normalize_evm_address(underlying_address)
    )
}

fn hash_opportunity(provider: &str, chain_id: &str, market_id: &str, asset_id: &str) -> String {
    let seed = [provider, chain_id, market_id, asset_id].join("|");
    let mut hasher = Sha1::new();
    hasher.update(seed.as_bytes());
    hex::encode(hasher.finalize())
}

fn matches_asset(address: &str, symbol: &str, asset: &Asset) -> bool {
    let asset_address = asset.address.trim();
    if !asset_address.is_empty() {
        return address.trim().eq_ignore_ascii_case(asset_address);
    }
    let asset_symbol = asset.symbol.trim();
    if !asset_symbol.is_empty() {
        return symbol.trim().eq_ignore_ascii_case(asset_symbol);
    }
    true
}

fn matches_position_type(filter: LendPositionType, position: LendPositionType) -> bool {
    if filter == LendPositionType::All {
        return true;
    }
    filter == position
}

fn sort_lend_positions(items: &mut [model::LendPosition]) {
    items.sort_by(|a, b| {
        desc_f64(a.amount_usd, b.amount_usd)
            .then_with(|| a.position_type.cmp(&b.position_type))
            .then_with(|| a.asset_id.cmp(&b.asset_id))
            .then_with(|| a.provider_native_id.cmp(&b.provider_native_id))
    });
}

fn sort_yield_positions(items: &mut [model::YieldPosition]) {
    items.sort_by(|a, b| {
        desc_f64(a.amount_usd, b.amount_usd)
            .then_with(|| desc_f64(a.apy_total, b.apy_total))
            .then_with(|| a.asset_id.cmp(&b.asset_id))
            .then_with(|| a.provider_native_id.cmp(&b.provider_native_id))
    });
}

/// Compare two `f64` values for a DESCENDING, total-order-safe sort.
fn desc_f64(a: f64, b: f64) -> std::cmp::Ordering {
    b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
}

// ── ABI fragment sets ───────────────────────────────────────────────────
//
// The registry ABI fragments are static and known-good (the `defi-registry`
// ABI-parse test guarantees they parse), but parsing is still fallible, so —
// matching the execution planner's pattern — we build the `Function` fragments
// once per call and propagate any (impossible) parse error with `?` rather than
// panicking or carrying a lazy singleton.

struct ComptrollerFns {
    get_all_markets: Function,
    get_assets_in: Function,
    oracle: Function,
}

impl ComptrollerFns {
    fn build() -> Result<Self, Error> {
        let abi = defi_registry::MOONWELL_COMPTROLLER_ABI;
        Ok(ComptrollerFns {
            get_all_markets: Function::from_abi_json(abi, "getAllMarkets")?,
            get_assets_in: Function::from_abi_json(abi, "getAssetsIn")?,
            oracle: Function::from_abi_json(abi, "oracle")?,
        })
    }
}

struct MTokenFns {
    underlying: Function,
    supply_rate: Function,
    borrow_rate: Function,
    total_supply: Function,
    exchange_rate: Function,
    total_borrows: Function,
    get_cash: Function,
    get_account_snapshot: Function,
}

impl MTokenFns {
    fn build() -> Result<Self, Error> {
        let abi = defi_registry::MOONWELL_MTOKEN_ABI;
        Ok(MTokenFns {
            underlying: Function::from_abi_json(abi, "underlying")?,
            supply_rate: Function::from_abi_json(abi, "supplyRatePerTimestamp")?,
            borrow_rate: Function::from_abi_json(abi, "borrowRatePerTimestamp")?,
            total_supply: Function::from_abi_json(abi, "totalSupply")?,
            exchange_rate: Function::from_abi_json(abi, "exchangeRateCurrent")?,
            total_borrows: Function::from_abi_json(abi, "totalBorrowsCurrent")?,
            get_cash: Function::from_abi_json(abi, "getCash")?,
            get_account_snapshot: Function::from_abi_json(abi, "getAccountSnapshot")?,
        })
    }
}

struct OracleFns {
    get_underlying_price: Function,
}

impl OracleFns {
    fn build() -> Result<Self, Error> {
        Ok(OracleFns {
            get_underlying_price: Function::from_abi_json(
                defi_registry::MOONWELL_ORACLE_ABI,
                "getUnderlyingPrice",
            )?,
        })
    }
}

struct Erc20Fns {
    symbol: Function,
    decimals: Function,
}

impl Erc20Fns {
    fn build() -> Result<Self, Error> {
        let abi = defi_registry::MOONWELL_ERC20_MINIMAL_ABI;
        Ok(Erc20Fns {
            symbol: Function::from_abi_json(abi, "symbol")?,
            decimals: Function::from_abi_json(abi, "decimals")?,
        })
    }
}

fn aggregate3_fn() -> Result<Function, Error> {
    Function::from_abi_json(defi_registry::MULTICALL3_ABI, "aggregate3")
}

/// Encode a call to a parsed ABI function fragment.
fn encode_call(func: &Function, args: &[DynSolValue]) -> Result<Vec<u8>, Error> {
    func.encode(args)
}

#[cfg(test)]
#[allow(clippy::doc_lazy_continuation)]
mod tests {
    //! # Success criteria for the `moonwell` provider adapter
    //!
    //! Go source: `internal/providers/moonwell/client.go`; ported behavioral
    //! cases from `internal/providers/moonwell/client_test.go`. Moonwell is the
    //! only fully on-chain read adapter: every read funnels through `eth_call`
    //! (single calls for `getAllMarkets`/`getAssetsIn`/`oracle`, batched reads
    //! through `Multicall3.aggregate3`). The Go test stands up an `httptest`
    //! JSON-RPC server that decodes `aggregate3`, dispatches each sub-call by
    //! `(target, selector)`, and re-encodes the `Result[]`. The Rust port
    //! reproduces that server with `wiremock` + a custom `Respond` impl, decoding
    //! and re-encoding via the same `alloy` ABI engine the adapter uses.
    //!
    //! The `Client` exposes two test seams mirroring the package-private fields
    //! the Go tests poke:
    //!   * `set_rpc_override(&url)` — point on-chain reads at the mock RPC
    //!     server (Go `client.rpcOverride = srv.URL`).
    //!   * `set_now(DateTime<Utc>)` — pin the clock (Go `client.now`).
    //!
    //! ## Criteria
    //!
    //!  W0. **Provider metadata** (`Provider::info`). `name == "moonwell"`,
    //!      `provider_type == "lending+yield"`, `requires_key == false`, the read
    //!      + execution capabilities present. Callable as metadata WITHOUT a key.
    //!
    //!  W1. **LendMarkets + LendRates + YieldOpportunities** (Go
    //!      `TestLendMarketsAndYield`). For the single USDC market on Base
    //!      (`eip155:8453`): exactly one `LendMarket` with `provider ==
    //!      protocol == "moonwell"`, non-empty `provider_native_id` +
    //!      `provider_native_id_kind == composite_market_asset`, positive
    //!      supply/borrow APY and TVL. `LendRates` returns one rate with positive
    //!      utilization. `YieldOpportunities` returns one `lend` opportunity with
    //!      `withdrawal_terms == "variable"` and a single full-share `USDC`
    //!      backing asset.
    //!
    //!  W2. **LendPositions type split** (Go `TestLendPositions`). For the dead
    //!      account holding both an mToken (collateral, since the market is in the
    //!      account's `getAssetsIn` set) and a borrow balance, `type=all` returns
    //!      TWO rows (`collateral` + `borrow`), each with positive `amount_usd`.
    //!
    //!  W3. **LendPositions filtering** (Go `TestLendPositionsFiltering`).
    //!      `type=collateral` returns ONLY the collateral row; `type=borrow`
    //!      returns ONLY the borrow row.
    //!
    //!  W4. **YieldPositions** (Go `TestYieldPositions`). Derived from
    //!      `LendPositions(type=all)`: only `supply`/`collateral` rows become
    //!      yield rows. One collateral input -> exactly one `deposit` yield row
    //!      with `provider == "moonwell"`.
    //!
    //!  W5. **Unsupported chain** (Go `TestUnsupportedChain`). A chain Moonwell
    //!      does not cover (`eip155:999`) -> typed error (no network).
    //!
    //!  W6. **rate_to_apy + bigint_to_float** (Go `TestRateToAPY`,
    //!      `TestBigIntToFloat`). `rate_to_apy(951293759)` ≈ 3% (within
    //!      `[2.9, 3.1]`); `rate_to_apy(0) == 0`; `bigint_to_float(1_000_000, 6)
    //!      == 1.0`.

    use super::*;

    use alloy::dyn_abi::DynSolValue;
    use alloy::json_abi::JsonAbi;
    use alloy::primitives::{Address as AlloyAddress, U256};
    use defi_errors::Code;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    // ---- canonical test addresses (mirror the Go fixtures) ----
    const TEST_COMPTROLLER: &str = "0xfBb21d0380beE3312B33c4353c8936a0F13EF26C";
    const TEST_ORACLE: &str = "0xEC942bE8A8114bFD0396A5052c36027f2cA6a9d0";
    const TEST_MTOKEN_USDC: &str = "0xEdc817A28E8B93B03976FBd4a3dDBc9f7D176c22";
    const TEST_USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    const TEST_ACCOUNT: &str = "0x000000000000000000000000000000000000dEaD";

    fn addr(s: &str) -> AlloyAddress {
        s.parse().expect("valid test address")
    }

    /// 4-byte selector (hex) for a function in a registry ABI document, the
    /// analogue of the Go `selectorHex`.
    fn selector_for(abi_json: &str, name: &str) -> String {
        let abi: JsonAbi = serde_json::from_str(abi_json).expect("parse abi");
        let f = abi
            .function(name)
            .and_then(|o| o.first())
            .cloned()
            .expect("function present");
        hex::encode(f.selector().0)
    }

    /// Encode output values as an ABI return blob (the analogue of the Go
    /// `packOutput` helper).
    fn encode_output(values: &[DynSolValue]) -> Vec<u8> {
        DynSolValue::Tuple(values.to_vec()).abi_encode_params()
    }

    /// The parsed `Multicall3.aggregate3` fragment used by the mock server to
    /// decode the request input and re-encode the `Result[]` output.
    fn aggregate3_json() -> alloy::json_abi::Function {
        let abi: JsonAbi = serde_json::from_str(defi_registry::MULTICALL3_ABI).expect("parse mc3");
        abi.function("aggregate3")
            .and_then(|o| o.first())
            .cloned()
            .expect("aggregate3 present")
    }

    fn chain_base() -> Chain {
        Chain {
            caip2: "eip155:8453".to_string(),
            evm_chain_id: 8453,
            ..Default::default()
        }
    }

    fn usdc_asset() -> Asset {
        Asset {
            symbol: "USDC".to_string(),
            chain_id: "eip155:8453".to_string(),
            ..Default::default()
        }
    }

    /// The mock JSON-RPC server's per-call dispatcher. Resolves a single
    /// `(target, selector)` to its ABI-encoded return blob, mirroring the Go
    /// `dispatchSingleCall`.
    struct Dispatcher {
        get_all_markets_sel: String,
        oracle_sel: String,
        get_assets_in_sel: String,
        m_underlying_sel: String,
        m_supply_rate_sel: String,
        m_borrow_rate_sel: String,
        m_total_supply_sel: String,
        m_exchange_rate_sel: String,
        m_total_borrows_sel: String,
        m_get_cash_sel: String,
        m_snapshot_sel: String,
        e_symbol_sel: String,
        e_decimals_sel: String,
        o_price_sel: String,
        // sample values (mirror the Go fixtures)
        supply_rate: U256,
        borrow_rate: U256,
        total_supply: U256,
        exchange_rate: U256,
        total_borrows: U256,
        cash: U256,
        price: U256,
        m_token_bal: U256,
        borrow_bal: U256,
    }

    impl Dispatcher {
        fn new() -> Self {
            let pow = |base: u128, exp: u32| U256::from(base).pow(U256::from(exp));
            let comptroller_abi = defi_registry::MOONWELL_COMPTROLLER_ABI;
            let mtoken_abi = defi_registry::MOONWELL_MTOKEN_ABI;
            let erc20_abi = defi_registry::MOONWELL_ERC20_MINIMAL_ABI;
            let oracle_abi = defi_registry::MOONWELL_ORACLE_ABI;
            Dispatcher {
                get_all_markets_sel: selector_for(comptroller_abi, "getAllMarkets"),
                oracle_sel: selector_for(comptroller_abi, "oracle"),
                get_assets_in_sel: selector_for(comptroller_abi, "getAssetsIn"),
                m_underlying_sel: selector_for(mtoken_abi, "underlying"),
                m_supply_rate_sel: selector_for(mtoken_abi, "supplyRatePerTimestamp"),
                m_borrow_rate_sel: selector_for(mtoken_abi, "borrowRatePerTimestamp"),
                m_total_supply_sel: selector_for(mtoken_abi, "totalSupply"),
                m_exchange_rate_sel: selector_for(mtoken_abi, "exchangeRateCurrent"),
                m_total_borrows_sel: selector_for(mtoken_abi, "totalBorrowsCurrent"),
                m_get_cash_sel: selector_for(mtoken_abi, "getCash"),
                m_snapshot_sel: selector_for(mtoken_abi, "getAccountSnapshot"),
                e_symbol_sel: selector_for(erc20_abi, "symbol"),
                e_decimals_sel: selector_for(erc20_abi, "decimals"),
                o_price_sel: selector_for(oracle_abi, "getUnderlyingPrice"),
                supply_rate: U256::from(951293759u64),
                borrow_rate: U256::from(1585489599u64),
                total_supply: U256::from(100_000_000u128) * pow(10, 8),
                exchange_rate: U256::from(2u128) * pow(10, 14),
                total_borrows: U256::from(500_000u128) * pow(10, 6),
                cash: U256::from(500_000u128) * pow(10, 6),
                price: pow(10, 30),
                m_token_bal: U256::from(10_000u128) * pow(10, 8),
                borrow_bal: U256::from(1_000u128) * pow(10, 6),
            }
        }

        /// Resolve a single sub-call to a return blob, or `None` for unknown
        /// (the Go `"0x"`).
        fn dispatch(&self, to: &str, data_hex: &str) -> Option<Vec<u8>> {
            let selector = data_hex.get(..8).unwrap_or("");
            let to = to.to_ascii_lowercase();

            if to == TEST_COMPTROLLER.to_ascii_lowercase() {
                if selector == self.get_all_markets_sel {
                    return Some(encode_output(&[DynSolValue::Array(vec![
                        DynSolValue::Address(addr(TEST_MTOKEN_USDC)),
                    ])]));
                }
                if selector == self.oracle_sel {
                    return Some(encode_output(&[DynSolValue::Address(addr(TEST_ORACLE))]));
                }
                if selector == self.get_assets_in_sel {
                    return Some(encode_output(&[DynSolValue::Array(vec![
                        DynSolValue::Address(addr(TEST_MTOKEN_USDC)),
                    ])]));
                }
            } else if to == TEST_ORACLE.to_ascii_lowercase() {
                if selector == self.o_price_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.price, 256)]));
                }
            } else if to == TEST_MTOKEN_USDC.to_ascii_lowercase() {
                if selector == self.m_underlying_sel {
                    return Some(encode_output(&[DynSolValue::Address(addr(TEST_USDC))]));
                }
                if selector == self.m_supply_rate_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.supply_rate, 256)]));
                }
                if selector == self.m_borrow_rate_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.borrow_rate, 256)]));
                }
                if selector == self.m_total_supply_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.total_supply, 256)]));
                }
                if selector == self.m_exchange_rate_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.exchange_rate, 256)]));
                }
                if selector == self.m_total_borrows_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.total_borrows, 256)]));
                }
                if selector == self.m_get_cash_sel {
                    return Some(encode_output(&[DynSolValue::Uint(self.cash, 256)]));
                }
                if selector == self.m_snapshot_sel {
                    return Some(encode_output(&[
                        DynSolValue::Uint(U256::ZERO, 256),
                        DynSolValue::Uint(self.m_token_bal, 256),
                        DynSolValue::Uint(self.borrow_bal, 256),
                        DynSolValue::Uint(self.exchange_rate, 256),
                    ]));
                }
            } else if to == TEST_USDC.to_ascii_lowercase() {
                if selector == self.e_symbol_sel {
                    return Some(encode_output(&[DynSolValue::String("USDC".to_string())]));
                }
                if selector == self.e_decimals_sel {
                    return Some(encode_output(&[DynSolValue::Uint(U256::from(6u8), 8)]));
                }
            }
            None
        }
    }

    /// A `wiremock` responder that emulates the Moonwell JSON-RPC server: it
    /// decodes `aggregate3`, dispatches each sub-call, and re-encodes `Result[]`.
    struct RpcResponder {
        dispatcher: Arc<Dispatcher>,
    }

    impl Respond for RpcResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = match serde_json::from_slice(&request.body) {
                Ok(v) => v,
                Err(_) => return ResponseTemplate::new(400),
            };
            // Support a single request object (the alloy client batches one
            // call per request).
            let id = body.get("id").cloned().unwrap_or(json!(1));
            let method_name = body.get("method").and_then(Value::as_str).unwrap_or("");
            if method_name != "eth_call" {
                return ok_response(&id, "0x");
            }
            let params = match body.get("params").and_then(|p| p.get(0)) {
                Some(p) => p,
                None => return ok_response(&id, "0x"),
            };
            let to = params
                .get("to")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let data_hex = params
                .get("data")
                .or_else(|| params.get("input"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim_start_matches("0x")
                .to_string();
            let selector = data_hex.get(..8).unwrap_or("");

            let mc3_sel = selector_for(defi_registry::MULTICALL3_ABI, "aggregate3");
            if to.to_ascii_lowercase() == MULTICALL3_ADDR && selector == mc3_sel {
                let result = self.handle_aggregate3(&data_hex);
                return ok_response(&id, &result);
            }

            let result = match self.dispatcher.dispatch(&to, &data_hex) {
                Some(bytes) => format!("0x{}", hex::encode(bytes)),
                None => "0x".to_string(),
            };
            ok_response(&id, &result)
        }
    }

    impl RpcResponder {
        fn handle_aggregate3(&self, data_hex: &str) -> String {
            use alloy::dyn_abi::{FunctionExt, JsonAbiExt};
            let raw = match hex::decode(data_hex) {
                Ok(b) => b,
                Err(_) => return "0x".to_string(),
            };
            if raw.len() < 4 {
                return "0x".to_string();
            }
            let agg = aggregate3_json();
            let decoded = match agg.abi_decode_input(&raw[4..]) {
                Ok(v) => v,
                Err(_) => return "0x".to_string(),
            };
            let calls = match decoded.first().and_then(|v| v.as_array()) {
                Some(c) => c,
                None => return "0x".to_string(),
            };

            let mut results: Vec<DynSolValue> = Vec::with_capacity(calls.len());
            for call in calls {
                let tuple = match call.as_tuple() {
                    Some(t) if t.len() == 3 => t,
                    _ => {
                        results.push(failed_result());
                        continue;
                    }
                };
                let target = tuple[0]
                    .as_address()
                    .map(|a| lower_hex(&a))
                    .unwrap_or_default();
                let sub_data = tuple[2].as_bytes().map(hex::encode).unwrap_or_default();
                match self.dispatcher.dispatch(&target, &sub_data) {
                    Some(bytes) => results.push(DynSolValue::Tuple(vec![
                        DynSolValue::Bool(true),
                        DynSolValue::Bytes(bytes),
                    ])),
                    None => results.push(failed_result()),
                }
            }

            // Encode as the aggregate3 output: tuple[](bool, bytes).
            match agg.abi_encode_output(&[DynSolValue::Array(results)]) {
                Ok(bytes) => format!("0x{}", hex::encode(bytes)),
                Err(_) => "0x".to_string(),
            }
        }
    }

    fn failed_result() -> DynSolValue {
        DynSolValue::Tuple(vec![
            DynSolValue::Bool(false),
            DynSolValue::Bytes(Vec::new()),
        ])
    }

    fn ok_response(id: &Value, result: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    async fn mock_server() -> MockServer {
        let server = MockServer::start().await;
        let responder = RpcResponder {
            dispatcher: Arc::new(Dispatcher::new()),
        };
        Mock::given(method("POST"))
            .respond_with(responder)
            .mount(&server)
            .await;
        server
    }

    fn client_for(server: &MockServer) -> Client {
        let mut client = Client::new();
        client.set_rpc_override(&server.uri());
        client.set_now(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap());
        client
    }

    use chrono::TimeZone;

    // ----- W0: provider metadata -----------------------------------------
    #[test]
    fn info_is_metadata_only_no_key_required() {
        let client = Client::new();
        let info = client.info();
        assert_eq!(info.name, "moonwell");
        assert_eq!(info.provider_type, "lending+yield");
        assert!(!info.requires_key);
        for cap in [
            "lend.markets",
            "lend.rates",
            "lend.positions",
            "yield.opportunities",
            "yield.positions",
        ] {
            assert!(
                info.capabilities.iter().any(|c| c == cap),
                "expected capability {cap}"
            );
        }
    }

    // ----- W1: markets + rates + yield ------------------------------------
    #[tokio::test]
    async fn lend_markets_rates_and_yield() {
        let server = mock_server().await;
        let client = client_for(&server);
        let chain = chain_base();
        let asset = usdc_asset();

        let markets = client
            .lend_markets("moonwell", chain.clone(), asset.clone())
            .await
            .expect("lend_markets");
        assert_eq!(markets.len(), 1, "expected 1 market");
        let m = &markets[0];
        assert_eq!(m.provider, "moonwell");
        assert_eq!(m.protocol, "moonwell");
        assert!(!m.provider_native_id.is_empty());
        assert_eq!(
            m.provider_native_id_kind,
            model::NATIVE_ID_KIND_COMPOSITE_MARKET_ASSET
        );
        assert!(m.supply_apy > 0.0, "supply apy {}", m.supply_apy);
        assert!(m.borrow_apy > 0.0, "borrow apy {}", m.borrow_apy);
        assert!(m.tvl_usd > 0.0, "tvl {}", m.tvl_usd);

        let rates = client
            .lend_rates("moonwell", chain.clone(), asset.clone())
            .await
            .expect("lend_rates");
        assert_eq!(rates.len(), 1);
        assert!(
            rates[0].utilization > 0.0,
            "utilization {}",
            rates[0].utilization
        );

        let opps = client
            .yield_opportunities(YieldRequest {
                chain: chain.clone(),
                asset: asset.clone(),
                limit: 10,
                min_tvl_usd: 0.0,
                min_apy: 0.0,
                providers: vec!["moonwell".to_string()],
                sort_by: String::new(),
                include_incomplete: false,
            })
            .await
            .expect("yield_opportunities");
        assert_eq!(opps.len(), 1);
        assert_eq!(opps[0].provider, "moonwell");
        assert_eq!(opps[0].opportunity_type, "lend");
        assert_eq!(opps[0].withdrawal_terms, "variable");
        assert_eq!(opps[0].backing_assets.len(), 1);
        assert_eq!(opps[0].backing_assets[0].share_pct, 100.0);
        assert_eq!(opps[0].backing_assets[0].symbol, "USDC");
    }

    // ----- W2: positions type split ---------------------------------------
    #[tokio::test]
    async fn lend_positions_collateral_and_borrow() {
        let server = mock_server().await;
        let client = client_for(&server);

        let positions = client
            .lend_positions(LendPositionsRequest {
                chain: chain_base(),
                account: TEST_ACCOUNT.to_string(),
                asset: Asset::default(),
                position_type: LendPositionType::All,
                limit: 0,
                rpc_url: String::new(),
            })
            .await
            .expect("lend_positions");
        assert_eq!(
            positions.len(),
            2,
            "expected collateral + borrow, got {positions:?}"
        );

        let mut has_collateral = false;
        let mut has_borrow = false;
        for p in &positions {
            if p.position_type == "collateral" {
                has_collateral = true;
                assert_eq!(p.provider, "moonwell");
                assert!(p.amount_usd > 0.0);
            }
            if p.position_type == "borrow" {
                has_borrow = true;
                assert!(p.amount_usd > 0.0);
            }
        }
        assert!(has_collateral && has_borrow);
    }

    // ----- W3: positions filtering ----------------------------------------
    #[tokio::test]
    async fn lend_positions_filtering() {
        let server = mock_server().await;
        let client = client_for(&server);

        let collateral = client
            .lend_positions(LendPositionsRequest {
                chain: chain_base(),
                account: TEST_ACCOUNT.to_string(),
                asset: Asset::default(),
                position_type: LendPositionType::Collateral,
                limit: 0,
                rpc_url: String::new(),
            })
            .await
            .expect("collateral");
        assert_eq!(collateral.len(), 1);
        assert_eq!(collateral[0].position_type, "collateral");

        let borrows = client
            .lend_positions(LendPositionsRequest {
                chain: chain_base(),
                account: TEST_ACCOUNT.to_string(),
                asset: Asset::default(),
                position_type: LendPositionType::Borrow,
                limit: 0,
                rpc_url: String::new(),
            })
            .await
            .expect("borrows");
        assert_eq!(borrows.len(), 1);
        assert_eq!(borrows[0].position_type, "borrow");
    }

    // ----- W4: yield positions --------------------------------------------
    #[tokio::test]
    async fn yield_positions_derives_deposit() {
        let server = mock_server().await;
        let client = client_for(&server);

        let positions = client
            .yield_positions(YieldPositionsRequest {
                chain: chain_base(),
                account: TEST_ACCOUNT.to_string(),
                asset: Asset::default(),
                limit: 0,
                rpc_url: String::new(),
            })
            .await
            .expect("yield_positions");
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].position_type, "deposit");
        assert_eq!(positions[0].provider, "moonwell");
    }

    // ----- W5: unsupported chain ------------------------------------------
    #[tokio::test]
    async fn unsupported_chain_errors() {
        let client = Client::new();
        let chain = Chain {
            caip2: "eip155:999".to_string(),
            evm_chain_id: 999,
            ..Default::default()
        };
        let asset = Asset {
            symbol: "USDC".to_string(),
            chain_id: "eip155:999".to_string(),
            ..Default::default()
        };
        let err = client
            .lend_markets("moonwell", chain, asset)
            .await
            .expect_err("expected error for unsupported chain");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ----- W6: numeric helpers --------------------------------------------
    #[test]
    fn rate_to_apy_matches_go() {
        let apy = rate_to_apy(&BigInt::from(951293759u64));
        assert!((2.9..=3.1).contains(&apy), "expected ~3%, got {apy}");
        assert_eq!(rate_to_apy(&BigInt::from(0u64)), 0.0);
    }

    #[test]
    fn bigint_to_float_matches_go() {
        assert_eq!(bigint_to_float(&BigInt::from(1_000_000u64), 6), 1.0);
    }
}
