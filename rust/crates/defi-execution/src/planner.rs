//! Deterministic contract-call planners (lend / yield / rewards / approvals /
//! transfer).
//!
//! Go source: `internal/execution/planner/*.go`
//! (`approvals.go`, `transfer.go`, `aave.go`, `morpho.go`, `morpho_vault.go`,
//! `moonwell.go`).
//!
//! This module owns the *deterministic* half of action construction (spec §3 /
//! AGENTS.md "Execution builder architecture"): `lend`/`yield`/`rewards`/
//! `approvals`/`transfer` actions are composed here from canonical contract
//! calls (in contrast to `swap`/`bridge`, which are provider-capability based and
//! live behind the builder traits). Each `build_*_action` function validates its
//! inputs, optionally reads chain state (allowance / pool address / mToken /
//! market metadata) over RPC + Morpho GraphQL, and emits an
//! [`crate::action::Action`] whose `steps[]` carry ABI-encoded calldata.
//!
//! The emitted `steps[].target` / `steps[].data` / step ordering and the
//! `intent_type` / `provider` strings are observable through the JSON contract,
//! so they must match the Go planner exactly. Idiomatic-Rust divergences from Go:
//!   * `(Action, error)` returns become `Result<Action, defi_errors::Error>`.
//!   * Go's mutable package var `morphoGraphQLEndpoint` (rebound in tests) is
//!     replaced by an explicit, optional `graphql_endpoint` field on the Morpho
//!     request types (empty => `registry::MORPHO_GRAPHQL_ENDPOINT`); no global
//!     mutable state.
//!   * RPC + HTTP are `async` (tokio), so the network-touching builders are
//!     `async fn`; the offline approval/transfer builders stay synchronous.
//!
//! ============================================================================
//! SUCCESS CRITERIA (RED phase — these tests are written before the code).
//!
//! The Rust planner is "correct" iff, for the same inputs, it produces the same
//! observable [`Action`] as the Go planner:
//!
//! APPROVAL (`build_approval_action`, offline) — Go `BuildApprovalAction`:
//!   A1. `intent_type == "approve"`, `provider == "native"`, exactly ONE step of
//!       type `approval` (`StepType::Approval`).
//!   A2. The step `target` is the **checksummed** ERC-20 token address; `value`
//!       is `"0"`; `data` is `approve(spender, amount)` calldata
//!       (`0x` + the ERC-20 `approve` selector + ABI args).
//!   A3. Validation (all `Code::Usage`): empty/invalid sender, empty/invalid
//!       spender, non-hex asset address, and a non-positive / non-integer amount
//!       are each rejected (amount `"0"` rejected).
//!   A4. `action.from_address` == checksummed sender; `action.to_address` ==
//!       checksummed spender; `action.input_amount` == decimal amount string;
//!       `constraints.simulate` reflects the request.
//!
//! TRANSFER (`build_transfer_action`, offline) — Go `BuildTransferAction`:
//!   T1. `intent_type == "transfer"`, `provider == "native"`, exactly ONE step of
//!       type `transfer` (`StepType::Transfer`); `data` is `transfer(to, amount)`
//!       calldata; `target` is the checksummed token address; `value == "0"`.
//!   T2. Non-EVM chain rejected with `Code::Unsupported`.
//!   T3. Validation (`Code::Usage`): empty/invalid sender, empty/invalid
//!       recipient, **zero** recipient address, non-hex asset address, and a
//!       non-positive amount are each rejected.
//!
//! AAVE LEND (`build_aave_lend_action`, async + RPC) — Go `BuildAaveLendAction`:
//!   L1. `provider == "aave"`, `intent_type == "lend_" + verb`.
//!   L2. SUPPLY with a zero current allowance emits TWO steps:
//!       `[approval, lend_call]`; the `lend_call` target is the resolved pool
//!       address (here the explicit `--pool-address`, checksum-insensitive match).
//!   L3. WITHDRAW / BORROW emit a single `lend_call` (no approval); REPAY emits
//!       `[approval, lend_call]` when allowance is insufficient.
//!   L4. BORROW/REPAY default `interest_rate_mode` 0 → 2 (variable); an
//!       out-of-range mode (not 1 or 2) is `Code::Usage`.
//!   L5. Missing/invalid sender is `Code::Usage` (validated before any RPC dial).
//!   L6. `metadata` carries `protocol="aave"`, `pool`, `on_behalf_of`,
//!       `recipient`, `rate_mode`, `lending_action`, `asset_id`.
//!   L7. When the current allowance already covers the amount, the approval step
//!       is SKIPPED (single `lend_call`).
//!
//! AAVE REWARDS (`build_aave_rewards_*_action`, async) — Go
//! `BuildAaveRewardsClaimAction` / `BuildAaveRewardsCompoundAction`:
//!   R1. CLAIM: `intent_type == "claim_rewards"`, one `claim` step
//!       (`StepType::Claim`) targeting the incentives controller; `--assets`
//!       parsed/deduped to checksummed addresses; empty assets → `Code::Usage`.
//!   R2. COMPOUND: `intent_type == "compound_rewards"`, THREE steps in order
//!       `[claim, approval, lend_call]` (claim → approve reward token → supply).
//!   R3. COMPOUND rejects a `recipient` that does not equal `from_address`
//!       (`Code::Usage`); rejects amount `"max"` (`Code::Usage`); rejects an
//!       invalid `on_behalf_of` with a message containing
//!       `"invalid on-behalf-of address"`.
//!
//! MORPHO LEND (`build_morpho_lend_action`, async + RPC + GraphQL) — Go
//! `BuildMorphoLendAction`:
//!   M1. `provider == "morpho"`, `intent_type == "lend_" + verb`.
//!   M2. Requires a valid `market_id`: missing → `Code::Usage`; a non-`0x` /
//!       non-32-byte / non-hex market id → `Code::Usage`.
//!   M3. SUPPLY (zero allowance) emits `[approval, lend_call]`; the `lend_call`
//!       target is the market's `morphoBlue.address` from GraphQL (exact
//!       checksum, here `0xBBBB…FFCb`).
//!   M4. The market's loan token must match `--asset` (else `Code::Usage`).
//!   M5. The Morpho GraphQL endpoint is taken from the request's
//!       `graphql_endpoint` override (so tests point it at a `wiremock` server);
//!       empty => registry default.
//!
//! MORPHO VAULT YIELD (`build_morpho_vault_yield_action`, async + RPC + GraphQL)
//! — Go `BuildMorphoVaultYieldAction`:
//!   V1. `provider == "morpho"`, `intent_type == "yield_" + verb`.
//!   V2. Verb must be `deposit` or `withdraw` (else `Code::Usage`); non-EVM chain
//!       → `Code::Unsupported`.
//!   V3. DEPOSIT (zero allowance) emits `[approval, lend_call]` whose `lend_call`
//!       target is the vault address; `metadata["vault_kind"] == "vault"`.
//!   V4. WITHDRAW emits a single `lend_call`; requires `--vault-address`
//!       (missing/invalid → `Code::Usage`).
//!   V5. The vault asset must match `--asset` (else `Code::Usage`).
//!
//! MOONWELL LEND (`build_moonwell_lend_action`, async + RPC) — Go
//! `BuildMoonwellLendAction`:
//!   W1. `provider == "moonwell"`, `intent_type == "lend_" + verb`.
//!   W2. SUPPLY with explicit mToken (`pool_address`), zero allowance and not yet
//!       a market member emits THREE steps:
//!       `[approval, moonwell-enter-market, moonwell-supply]` (step ids checked).
//!   W3. SUPPLY skips BOTH approval and enter-market when allowance is sufficient
//!       AND already a member (single `moonwell-supply`).
//!   W4. WITHDRAW / BORROW emit a single step (no approval); REPAY emits
//!       `[approval, moonwell-repay]`.
//!   W5. An alternate recipient (recipient != sender) is rejected with
//!       `Code::Unsupported` and a message containing `"alternate recipients"`.
//!   W6. Missing sender / non-positive amount / unsupported verb → `Code::Usage`.
//!   W7. `resolve_moonwell_mtoken` with an explicit address returns it verbatim;
//!       a non-hex explicit address → `Code::Usage`; an unsupported chain (no
//!       comptroller) with no explicit mToken → `Code::Unsupported` (message
//!       contains `"not supported"`).
//!   W8. Auto-resolution: with `pool_address` empty, the planner calls
//!       `Comptroller.getAllMarkets()` then batch-resolves `underlying()` via
//!       Multicall3 and selects the mToken whose underlying matches `--asset`.
//!
//! Go `httptest` servers are mapped to `wiremock`; the JSON-RPC `eth_call` mock
//! dispatches by 4-byte selector and returns ABI-encoded results, mirroring
//! `newPlannerRPCServer` / `newMoonwellPlannerRPCServer`. Tests that assert step
//! COUNT + ORDER + step ids + targets are the contract oracle here.
//! ============================================================================

#![allow(clippy::too_many_arguments)]

use alloy::dyn_abi::DynSolValue;
use alloy::primitives::U256;
use defi_errors::{Code, Error};
use defi_evm::abi::Function;
use defi_evm::address::{self, Address};
use defi_evm::rpc::{CallRequest, RpcClient};
use defi_id::{Asset, Chain};
use defi_registry::{
    aave_pool_address_provider, moonwell_comptroller, resolve_rpc_url, AAVE_POOL_ABI,
    AAVE_POOL_ADDRESS_PROVIDER_ABI, AAVE_REWARDS_ABI, ERC20_MINIMAL_ABI, ERC4626_VAULT_ABI,
    MOONWELL_COMPTROLLER_ABI, MOONWELL_MTOKEN_ABI, MORPHO_BLUE_ABI, MORPHO_GRAPHQL_ENDPOINT,
    MULTICALL3_ABI,
};

use crate::action::{Action, ActionStep, Constraints, StepStatus, StepType};

/// The canonical Multicall3 address (`0xcA11…CA11`).
const MULTICALL3_ADDR: &str = "0xcA11bde05977b3631167028862bE2a173976CA11";

// =============================================================================
// Request types.
// =============================================================================

/// Aave lend verb (`supply|withdraw|borrow|repay`). Parity with Go
/// `AaveLendVerb` (a free-form string in Go; an unknown verb is
/// [`AaveLendVerb::Unsupported`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AaveLendVerb {
    Supply,
    Withdraw,
    Borrow,
    Repay,
    Unsupported(String),
}

impl AaveLendVerb {
    fn as_str(&self) -> &str {
        match self {
            AaveLendVerb::Supply => "supply",
            AaveLendVerb::Withdraw => "withdraw",
            AaveLendVerb::Borrow => "borrow",
            AaveLendVerb::Repay => "repay",
            AaveLendVerb::Unsupported(s) => s,
        }
    }
}

/// Morpho ERC-4626 vault yield verb. Parity with Go `MorphoVaultYieldVerb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MorphoVaultYieldVerb {
    Deposit,
    Withdraw,
}

impl MorphoVaultYieldVerb {
    fn as_str(self) -> &'static str {
        match self {
            MorphoVaultYieldVerb::Deposit => "deposit",
            MorphoVaultYieldVerb::Withdraw => "withdraw",
        }
    }
}

/// An offline ERC-20 approval request. Parity with Go `ApprovalRequest`.
#[derive(Debug, Clone, Default)]
pub struct ApprovalRequest {
    pub chain: Chain,
    pub asset: Asset,
    pub amount_base_units: String,
    pub sender: String,
    pub spender: String,
    pub simulate: bool,
    pub rpc_url: String,
}

/// An offline ERC-20 transfer request. Parity with Go `TransferRequest`.
#[derive(Debug, Clone, Default)]
pub struct TransferRequest {
    pub chain: Chain,
    pub asset: Asset,
    pub amount_base_units: String,
    pub sender: String,
    pub recipient: String,
    pub simulate: bool,
    pub rpc_url: String,
}

/// An Aave lend request. Parity with Go `AaveLendRequest`.
#[derive(Debug, Clone)]
pub struct AaveLendRequest {
    pub verb: AaveLendVerb,
    pub chain: Chain,
    pub asset: Asset,
    pub amount_base_units: String,
    pub sender: String,
    pub recipient: String,
    pub on_behalf_of: String,
    pub interest_rate_mode: i64,
    pub simulate: bool,
    pub rpc_url: String,
    pub pool_address: String,
    pub pool_addresses_provider: String,
}

/// An Aave rewards-claim request. Parity with Go `AaveRewardsClaimRequest`.
#[derive(Debug, Clone)]
pub struct AaveRewardsClaimRequest {
    pub chain: Chain,
    pub sender: String,
    pub recipient: String,
    pub assets: Vec<String>,
    pub reward_token: String,
    pub amount_base_units: String,
    pub simulate: bool,
    pub rpc_url: String,
    pub controller_address: String,
    pub pool_addresses_provider: String,
}

/// An Aave rewards-compound request. Parity with Go `AaveRewardsCompoundRequest`.
#[derive(Debug, Clone)]
pub struct AaveRewardsCompoundRequest {
    pub chain: Chain,
    pub sender: String,
    pub recipient: String,
    pub assets: Vec<String>,
    pub reward_token: String,
    pub amount_base_units: String,
    pub simulate: bool,
    pub rpc_url: String,
    pub controller_address: String,
    pub pool_address: String,
    pub pool_addresses_provider: String,
    pub on_behalf_of: String,
}

/// A Morpho Blue lend request. Parity with Go `MorphoLendRequest` (plus an
/// explicit `graphql_endpoint` override replacing Go's package var).
#[derive(Debug, Clone)]
pub struct MorphoLendRequest {
    pub verb: AaveLendVerb,
    pub chain: Chain,
    pub asset: Asset,
    pub amount_base_units: String,
    pub sender: String,
    pub recipient: String,
    pub on_behalf_of: String,
    pub simulate: bool,
    pub rpc_url: String,
    pub market_id: String,
    pub graphql_endpoint: String,
}

/// A Morpho ERC-4626 vault yield request. Parity with Go
/// `MorphoVaultYieldRequest`.
#[derive(Debug, Clone)]
pub struct MorphoVaultYieldRequest {
    pub verb: MorphoVaultYieldVerb,
    pub chain: Chain,
    pub asset: Asset,
    pub vault_address: String,
    pub amount_base_units: String,
    pub sender: String,
    pub recipient: String,
    pub on_behalf_of: String,
    pub simulate: bool,
    pub rpc_url: String,
    pub graphql_endpoint: String,
}

/// A Moonwell lend request. Parity with Go `MoonwellLendRequest`.
#[derive(Debug, Clone)]
pub struct MoonwellLendRequest {
    pub verb: AaveLendVerb,
    pub chain: Chain,
    pub asset: Asset,
    pub amount_base_units: String,
    pub sender: String,
    pub recipient: String,
    pub simulate: bool,
    pub rpc_url: String,
    pub mtoken_address: String,
}

// =============================================================================
// APPROVAL + TRANSFER (offline).
// =============================================================================

/// Build a single-step ERC-20 approval action. Parity with Go
/// `BuildApprovalAction`.
pub fn build_approval_action(req: ApprovalRequest) -> Result<Action, Error> {
    let sender = req.sender.trim();
    if sender.is_empty() {
        return Err(Error::new(Code::Usage, "approval requires sender address"));
    }
    if !address::is_hex_address(sender) {
        return Err(Error::new(
            Code::Usage,
            "approval sender must be a valid EVM address",
        ));
    }
    let spender = req.spender.trim();
    if spender.is_empty() {
        return Err(Error::new(Code::Usage, "approval requires spender address"));
    }
    if !address::is_hex_address(spender) {
        return Err(Error::new(
            Code::Usage,
            "approval spender must be a valid EVM address",
        ));
    }
    if !address::is_hex_address(req.asset.address.trim()) {
        return Err(Error::new(
            Code::Usage,
            "approval requires ERC20 token address",
        ));
    }
    let amount = parse_positive_amount(&req.amount_base_units).ok_or_else(|| {
        Error::new(
            Code::Usage,
            "approval amount must be a positive integer in base units",
        )
    })?;
    let rpc_url = resolve_rpc(&req.rpc_url, req.chain.evm_chain_id)?;

    let sender = address::parse(sender)?;
    let spender = address::parse(spender)?;
    let token = address::parse(req.asset.address.trim())?;
    let approve_data = encode_erc20("approve", spender, amount)?;

    let mut action = Action::new(
        crate::action::new_action_id(),
        "approve",
        &req.chain.caip2,
        Constraints {
            simulate: req.simulate,
            ..Default::default()
        },
    );
    action.provider = "native".into();
    action.from_address = sender.to_hex();
    action.to_address = spender.to_hex();
    action.input_amount = amount.to_string();
    action.metadata = Some(obj(&[
        ("asset_id", &req.asset.asset_id),
        ("spender", &spender.to_hex()),
    ]));
    action.steps.push(step(
        "approve-token",
        StepType::Approval,
        &req.chain.caip2,
        &rpc_url,
        &format!("Approve {} for spender", req.asset.symbol.to_uppercase()),
        &token.to_hex(),
        &approve_data,
    ));
    Ok(action)
}

/// Build a single-step ERC-20 transfer action. Parity with Go
/// `BuildTransferAction`.
pub fn build_transfer_action(req: TransferRequest) -> Result<Action, Error> {
    if !req.chain.is_evm() {
        return Err(Error::new(
            Code::Unsupported,
            "transfer currently supports EVM chains only",
        ));
    }
    let sender = req.sender.trim();
    if sender.is_empty() {
        return Err(Error::new(Code::Usage, "transfer requires sender address"));
    }
    if !address::is_hex_address(sender) {
        return Err(Error::new(
            Code::Usage,
            "transfer sender must be a valid EVM address",
        ));
    }
    let recipient = req.recipient.trim();
    if recipient.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "transfer requires recipient address",
        ));
    }
    if !address::is_hex_address(recipient) {
        return Err(Error::new(
            Code::Usage,
            "transfer recipient must be a valid EVM address",
        ));
    }
    let recipient_addr = address::parse(recipient)?;
    if recipient_addr.is_zero() {
        return Err(Error::new(
            Code::Usage,
            "transfer recipient cannot be zero address",
        ));
    }
    if !address::is_hex_address(req.asset.address.trim()) {
        return Err(Error::new(
            Code::Usage,
            "transfer requires ERC20 token address",
        ));
    }
    let amount = parse_positive_amount(&req.amount_base_units).ok_or_else(|| {
        Error::new(
            Code::Usage,
            "transfer amount must be a positive integer in base units",
        )
    })?;
    let rpc_url = resolve_rpc(&req.rpc_url, req.chain.evm_chain_id)?;

    let sender = address::parse(sender)?;
    let token = address::parse(req.asset.address.trim())?;
    let transfer_data = encode_erc20("transfer", recipient_addr, amount)?;

    let mut action = Action::new(
        crate::action::new_action_id(),
        "transfer",
        &req.chain.caip2,
        Constraints {
            simulate: req.simulate,
            ..Default::default()
        },
    );
    action.provider = "native".into();
    action.from_address = sender.to_hex();
    action.to_address = recipient_addr.to_hex();
    action.input_amount = amount.to_string();
    action.metadata = Some(obj(&[
        ("asset_id", &req.asset.asset_id),
        ("asset_address", &token.to_hex()),
        ("recipient", &recipient_addr.to_hex()),
    ]));
    action.steps.push(step(
        "transfer-token",
        StepType::Transfer,
        &req.chain.caip2,
        &rpc_url,
        &format!("Transfer {} to recipient", req.asset.symbol.to_uppercase()),
        &token.to_hex(),
        &transfer_data,
    ));
    Ok(action)
}

// =============================================================================
// AAVE LEND + REWARDS (RPC).
// =============================================================================

/// Build an Aave lend action. Parity with Go `BuildAaveLendAction`.
pub async fn build_aave_lend_action(req: AaveLendRequest) -> Result<Action, Error> {
    let verb = req.verb.as_str().to_string();
    let inputs = normalize_lend_inputs(
        &req.sender,
        &req.recipient,
        &req.on_behalf_of,
        &req.asset.address,
        &req.amount_base_units,
        &req.rpc_url,
        req.chain.evm_chain_id,
    )?;
    let client = RpcClient::connect(&inputs.rpc_url)?;
    let pool = resolve_aave_pool_address(
        &client,
        req.chain.evm_chain_id,
        &req.pool_address,
        &req.pool_addresses_provider,
    )
    .await?;

    let mut action = Action::new(
        crate::action::new_action_id(),
        format!("lend_{verb}"),
        &req.chain.caip2,
        Constraints {
            simulate: req.simulate,
            ..Default::default()
        },
    );
    action.provider = "aave".into();
    action.from_address = inputs.sender.to_hex();
    action.to_address = inputs.recipient.to_hex();
    action.input_amount = inputs.amount.to_string();
    let mut meta = obj(&[
        ("protocol", "aave"),
        ("asset_id", &req.asset.asset_id),
        ("pool", &pool.to_hex()),
        ("on_behalf_of", &inputs.on_behalf_of.to_hex()),
        ("recipient", &inputs.recipient.to_hex()),
        ("lending_action", &verb),
    ]);
    meta.insert(
        "rate_mode".into(),
        serde_json::Value::Number(req.interest_rate_mode.into()),
    );
    action.metadata = Some(meta);

    match req.verb {
        AaveLendVerb::Supply => {
            append_approval_if_needed(
                &client,
                &mut action,
                &req.chain.caip2,
                &inputs.rpc_url,
                inputs.token,
                inputs.sender,
                pool,
                inputs.amount,
                "Approve token for Aave supply",
            )
            .await?;
            let data = encode_aave(
                "supply",
                &[
                    DynSolValue::Address(inputs.token.into_inner()),
                    uint256(inputs.amount),
                    DynSolValue::Address(inputs.on_behalf_of.into_inner()),
                    DynSolValue::Uint(U256::ZERO, 16),
                ],
            )?;
            action.steps.push(step(
                "aave-supply",
                StepType::Lend,
                &req.chain.caip2,
                &inputs.rpc_url,
                "Supply asset to Aave",
                &pool.to_hex(),
                &data,
            ));
        }
        AaveLendVerb::Withdraw => {
            let data = encode_aave(
                "withdraw",
                &[
                    DynSolValue::Address(inputs.token.into_inner()),
                    uint256(inputs.amount),
                    DynSolValue::Address(inputs.recipient.into_inner()),
                ],
            )?;
            action.steps.push(step(
                "aave-withdraw",
                StepType::Lend,
                &req.chain.caip2,
                &inputs.rpc_url,
                "Withdraw asset from Aave",
                &pool.to_hex(),
                &data,
            ));
        }
        AaveLendVerb::Borrow => {
            let rate_mode = resolve_rate_mode(req.interest_rate_mode)?;
            let data = encode_aave(
                "borrow",
                &[
                    DynSolValue::Address(inputs.token.into_inner()),
                    uint256(inputs.amount),
                    DynSolValue::Uint(U256::from(rate_mode), 256),
                    DynSolValue::Uint(U256::ZERO, 16),
                    DynSolValue::Address(inputs.on_behalf_of.into_inner()),
                ],
            )?;
            action.steps.push(step(
                "aave-borrow",
                StepType::Lend,
                &req.chain.caip2,
                &inputs.rpc_url,
                "Borrow asset from Aave",
                &pool.to_hex(),
                &data,
            ));
        }
        AaveLendVerb::Repay => {
            let rate_mode = resolve_rate_mode(req.interest_rate_mode)?;
            append_approval_if_needed(
                &client,
                &mut action,
                &req.chain.caip2,
                &inputs.rpc_url,
                inputs.token,
                inputs.sender,
                pool,
                inputs.amount,
                "Approve token for Aave repay",
            )
            .await?;
            let data = encode_aave(
                "repay",
                &[
                    DynSolValue::Address(inputs.token.into_inner()),
                    uint256(inputs.amount),
                    DynSolValue::Uint(U256::from(rate_mode), 256),
                    DynSolValue::Address(inputs.on_behalf_of.into_inner()),
                ],
            )?;
            action.steps.push(step(
                "aave-repay",
                StepType::Lend,
                &req.chain.caip2,
                &inputs.rpc_url,
                "Repay borrowed asset on Aave",
                &pool.to_hex(),
                &data,
            ));
        }
        AaveLendVerb::Unsupported(_) => {
            return Err(Error::new(Code::Usage, "unsupported lend action verb"));
        }
    }
    Ok(action)
}

/// Build an Aave rewards-claim action. Parity with Go
/// `BuildAaveRewardsClaimAction`.
pub async fn build_aave_rewards_claim_action(
    req: AaveRewardsClaimRequest,
) -> Result<Action, Error> {
    let sender = req.sender.trim();
    if !address::is_hex_address(sender) {
        return Err(Error::new(
            Code::Usage,
            "rewards claim requires sender address",
        ));
    }
    let recipient_raw = if req.recipient.trim().is_empty() {
        sender
    } else {
        req.recipient.trim()
    };
    if !address::is_hex_address(recipient_raw) {
        return Err(Error::new(Code::Usage, "invalid rewards recipient address"));
    }
    if !address::is_hex_address(req.reward_token.trim()) {
        return Err(Error::new(Code::Usage, "reward token must be an address"));
    }
    let assets = normalize_address_list(&req.assets)?;
    if assets.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "rewards claim requires at least one asset in --assets",
        ));
    }
    let rpc_url = resolve_rpc(&req.rpc_url, req.chain.evm_chain_id)?;
    let client = RpcClient::connect(&rpc_url)?;
    let controller = resolve_incentives_controller(
        &client,
        req.chain.evm_chain_id,
        &req.controller_address,
        &req.pool_addresses_provider,
    )
    .await?;
    let amount = parse_reward_amount(&req.amount_base_units)?;

    let recipient = address::parse(recipient_raw)?;
    let sender_addr = address::parse(sender)?;
    let reward = address::parse(req.reward_token.trim())?;
    let asset_values: Vec<DynSolValue> = assets
        .iter()
        .map(|a| address::parse(a).map(|x| DynSolValue::Address(x.into_inner())))
        .collect::<Result<_, _>>()?;
    let data = encode_fn(
        AAVE_REWARDS_ABI,
        "claimRewards",
        &[
            DynSolValue::Array(asset_values),
            uint256(amount),
            DynSolValue::Address(recipient.into_inner()),
            DynSolValue::Address(reward.into_inner()),
        ],
    )?;

    let mut action = Action::new(
        crate::action::new_action_id(),
        "claim_rewards",
        &req.chain.caip2,
        Constraints {
            simulate: req.simulate,
            ..Default::default()
        },
    );
    action.provider = "aave".into();
    action.from_address = sender_addr.to_hex();
    action.to_address = recipient.to_hex();
    action.input_amount = amount.to_string();
    let mut meta = obj(&[
        ("protocol", "aave"),
        ("controller", &controller.to_hex()),
        ("reward_token", &reward.to_hex()),
        ("amount_base_units", &amount.to_string()),
    ]);
    meta.insert(
        "assets".into(),
        serde_json::Value::Array(
            assets
                .iter()
                .map(|a| serde_json::Value::String(a.clone()))
                .collect(),
        ),
    );
    action.metadata = Some(meta);
    action.steps.push(step(
        "aave-claim-rewards",
        StepType::Claim,
        &req.chain.caip2,
        &rpc_url,
        "Claim rewards from Aave incentives controller",
        &controller.to_hex(),
        &data,
    ));
    Ok(action)
}

/// Build an Aave rewards-compound action (claim → approve → supply). Parity with
/// Go `BuildAaveRewardsCompoundAction`.
pub async fn build_aave_rewards_compound_action(
    req: AaveRewardsCompoundRequest,
) -> Result<Action, Error> {
    if req.amount_base_units.trim().eq_ignore_ascii_case("max") {
        return Err(Error::new(
            Code::Usage,
            "compound requires an explicit --amount in base units (max is unsupported)",
        ));
    }
    let sender_input = req.sender.trim();
    let recipient_input = req.recipient.trim();
    if !recipient_input.is_empty() && !recipient_input.eq_ignore_ascii_case(sender_input) {
        return Err(Error::new(
            Code::Usage,
            "compound requires --recipient to match --from-address",
        ));
    }
    let mut action = build_aave_rewards_claim_action(AaveRewardsClaimRequest {
        chain: req.chain.clone(),
        sender: sender_input.to_string(),
        recipient: sender_input.to_string(),
        assets: req.assets.clone(),
        reward_token: req.reward_token.clone(),
        amount_base_units: req.amount_base_units.clone(),
        simulate: req.simulate,
        rpc_url: req.rpc_url.clone(),
        controller_address: req.controller_address.clone(),
        pool_addresses_provider: req.pool_addresses_provider.clone(),
    })
    .await?;
    action.action_id = crate::action::new_action_id();
    action.intent_type = "compound_rewards".into();
    if let Some(meta) = action.metadata.as_mut() {
        meta.insert("compound".into(), serde_json::Value::Bool(true));
    }

    let rpc_url = resolve_rpc(&req.rpc_url, req.chain.evm_chain_id)?;
    let client = RpcClient::connect(&rpc_url)?;
    let pool = resolve_aave_pool_address(
        &client,
        req.chain.evm_chain_id,
        &req.pool_address,
        &req.pool_addresses_provider,
    )
    .await?;
    let amount = parse_positive_amount(&req.amount_base_units).ok_or_else(|| {
        Error::new(
            Code::Usage,
            "compound amount must be a positive integer in base units",
        )
    })?;
    let sender = address::parse(req.sender.trim())?;
    let on_behalf_of = if req.on_behalf_of.trim().is_empty() {
        sender
    } else {
        if !address::is_hex_address(req.on_behalf_of.trim()) {
            return Err(Error::new(Code::Usage, "invalid on-behalf-of address"));
        }
        address::parse(req.on_behalf_of.trim())?
    };
    let reward = address::parse(req.reward_token.trim())?;
    append_approval_if_needed(
        &client,
        &mut action,
        &req.chain.caip2,
        &rpc_url,
        reward,
        sender,
        pool,
        amount,
        "Approve reward token for Aave supply",
    )
    .await?;
    let supply_data = encode_aave(
        "supply",
        &[
            DynSolValue::Address(reward.into_inner()),
            uint256(amount),
            DynSolValue::Address(on_behalf_of.into_inner()),
            DynSolValue::Uint(U256::ZERO, 16),
        ],
    )?;
    action.steps.push(step(
        "aave-compound-supply",
        StepType::Lend,
        &req.chain.caip2,
        &rpc_url,
        "Supply claimed reward token to Aave",
        &pool.to_hex(),
        &supply_data,
    ));
    if let Some(meta) = action.metadata.as_mut() {
        meta.insert("pool".into(), serde_json::Value::String(pool.to_hex()));
        meta.insert(
            "on_behalf_of".into(),
            serde_json::Value::String(on_behalf_of.to_hex()),
        );
    }
    Ok(action)
}

// =============================================================================
// MORPHO LEND + VAULT (RPC + GraphQL).
// =============================================================================

/// Build a Morpho Blue lend action. Parity with Go `BuildMorphoLendAction`.
pub async fn build_morpho_lend_action(req: MorphoLendRequest) -> Result<Action, Error> {
    let verb = req.verb.as_str().to_string();
    let inputs = normalize_lend_inputs(
        &req.sender,
        &req.recipient,
        &req.on_behalf_of,
        &req.asset.address,
        &req.amount_base_units,
        &req.rpc_url,
        req.chain.evm_chain_id,
    )?;
    let market_id = normalize_morpho_market_id(&req.market_id)?;
    let endpoint = if req.graphql_endpoint.trim().is_empty() {
        MORPHO_GRAPHQL_ENDPOINT.to_string()
    } else {
        req.graphql_endpoint.trim().to_string()
    };
    let market = fetch_morpho_market_by_id(req.chain.evm_chain_id, &market_id, &endpoint).await?;

    if !market
        .loan_asset_address
        .eq_ignore_ascii_case(&inputs.token.to_hex())
    {
        return Err(Error::new(
            Code::Usage,
            "selected morpho market loan token does not match --asset",
        ));
    }
    if !address::is_hex_address(&market.morpho_address) {
        return Err(Error::new(
            Code::Unavailable,
            "morpho market missing executable morpho contract address",
        ));
    }
    if !address::is_hex_address(&market.oracle_address) {
        return Err(Error::new(
            Code::Unavailable,
            "morpho market missing oracle address",
        ));
    }
    if !address::is_hex_address(&market.irm) {
        return Err(Error::new(
            Code::Unavailable,
            "morpho market missing irm address",
        ));
    }
    if !address::is_hex_address(&market.collateral_address) {
        return Err(Error::new(
            Code::Unavailable,
            "morpho market missing collateral token address",
        ));
    }
    let lltv = parse_positive_amount(&market.lltv)
        .ok_or_else(|| Error::new(Code::Unavailable, "morpho market returned invalid lltv"))?;

    let morpho = address::parse(&market.morpho_address)?;
    let loan_token = address::parse(&market.loan_asset_address)?;
    let market_params = DynSolValue::Tuple(vec![
        DynSolValue::Address(loan_token.into_inner()),
        DynSolValue::Address(address::parse(&market.collateral_address)?.into_inner()),
        DynSolValue::Address(address::parse(&market.oracle_address)?.into_inner()),
        DynSolValue::Address(address::parse(&market.irm)?.into_inner()),
        uint256(lltv),
    ]);

    let client = RpcClient::connect(&inputs.rpc_url)?;

    let mut action = Action::new(
        crate::action::new_action_id(),
        format!("lend_{verb}"),
        &req.chain.caip2,
        Constraints {
            simulate: req.simulate,
            ..Default::default()
        },
    );
    action.provider = "morpho".into();
    action.from_address = inputs.sender.to_hex();
    action.to_address = inputs.recipient.to_hex();
    action.input_amount = inputs.amount.to_string();
    action.metadata = Some(obj(&[
        ("protocol", "morpho"),
        ("asset_id", &req.asset.asset_id),
        ("market_id", &market_id),
        ("loan_token", &loan_token.to_hex()),
        ("morpho_address", &morpho.to_hex()),
        ("on_behalf_of", &inputs.on_behalf_of.to_hex()),
        ("recipient", &inputs.recipient.to_hex()),
        ("lending_action", &verb),
    ]));

    let zero = DynSolValue::Uint(U256::ZERO, 256);
    match req.verb {
        AaveLendVerb::Supply => {
            append_approval_if_needed(
                &client,
                &mut action,
                &req.chain.caip2,
                &inputs.rpc_url,
                loan_token,
                inputs.sender,
                morpho,
                inputs.amount,
                "Approve token for Morpho supply",
            )
            .await?;
            let data = encode_fn(
                MORPHO_BLUE_ABI,
                "supply",
                &[
                    market_params,
                    uint256(inputs.amount),
                    zero,
                    DynSolValue::Address(inputs.on_behalf_of.into_inner()),
                    DynSolValue::Bytes(vec![]),
                ],
            )?;
            action.steps.push(step(
                "morpho-supply",
                StepType::Lend,
                &req.chain.caip2,
                &inputs.rpc_url,
                "Supply asset to Morpho market",
                &morpho.to_hex(),
                &data,
            ));
        }
        AaveLendVerb::Withdraw => {
            let data = encode_fn(
                MORPHO_BLUE_ABI,
                "withdraw",
                &[
                    market_params,
                    uint256(inputs.amount),
                    zero,
                    DynSolValue::Address(inputs.on_behalf_of.into_inner()),
                    DynSolValue::Address(inputs.recipient.into_inner()),
                ],
            )?;
            action.steps.push(step(
                "morpho-withdraw",
                StepType::Lend,
                &req.chain.caip2,
                &inputs.rpc_url,
                "Withdraw supplied assets from Morpho market",
                &morpho.to_hex(),
                &data,
            ));
        }
        AaveLendVerb::Borrow => {
            let data = encode_fn(
                MORPHO_BLUE_ABI,
                "borrow",
                &[
                    market_params,
                    uint256(inputs.amount),
                    zero,
                    DynSolValue::Address(inputs.on_behalf_of.into_inner()),
                    DynSolValue::Address(inputs.recipient.into_inner()),
                ],
            )?;
            action.steps.push(step(
                "morpho-borrow",
                StepType::Lend,
                &req.chain.caip2,
                &inputs.rpc_url,
                "Borrow asset from Morpho market",
                &morpho.to_hex(),
                &data,
            ));
        }
        AaveLendVerb::Repay => {
            append_approval_if_needed(
                &client,
                &mut action,
                &req.chain.caip2,
                &inputs.rpc_url,
                loan_token,
                inputs.sender,
                morpho,
                inputs.amount,
                "Approve token for Morpho repay",
            )
            .await?;
            let data = encode_fn(
                MORPHO_BLUE_ABI,
                "repay",
                &[
                    market_params,
                    uint256(inputs.amount),
                    zero,
                    DynSolValue::Address(inputs.on_behalf_of.into_inner()),
                    DynSolValue::Bytes(vec![]),
                ],
            )?;
            action.steps.push(step(
                "morpho-repay",
                StepType::Lend,
                &req.chain.caip2,
                &inputs.rpc_url,
                "Repay borrowed assets in Morpho market",
                &morpho.to_hex(),
                &data,
            ));
        }
        AaveLendVerb::Unsupported(_) => {
            return Err(Error::new(Code::Usage, "unsupported lend action verb"));
        }
    }
    Ok(action)
}

/// Build a Morpho ERC-4626 vault yield action. Parity with Go
/// `BuildMorphoVaultYieldAction`.
pub async fn build_morpho_vault_yield_action(
    req: MorphoVaultYieldRequest,
) -> Result<Action, Error> {
    if !req.chain.is_evm() {
        return Err(Error::new(
            Code::Unsupported,
            "morpho vault execution supports only EVM chains",
        ));
    }
    let verb = req.verb.as_str();
    let inputs = normalize_lend_inputs(
        &req.sender,
        &req.recipient,
        &req.on_behalf_of,
        &req.asset.address,
        &req.amount_base_units,
        &req.rpc_url,
        req.chain.evm_chain_id,
    )?;
    if req.verb == MorphoVaultYieldVerb::Withdraw && inputs.sender != inputs.on_behalf_of {
        return Err(Error::new(
            Code::Usage,
            "morpho vault withdraw currently requires --on-behalf-of to match sender",
        ));
    }
    if !address::is_hex_address(req.vault_address.trim()) {
        return Err(Error::new(
            Code::Usage,
            "morpho vault yield execution requires a valid --vault-address",
        ));
    }
    let vault = address::parse(req.vault_address.trim())?;
    let endpoint = if req.graphql_endpoint.trim().is_empty() {
        MORPHO_GRAPHQL_ENDPOINT.to_string()
    } else {
        req.graphql_endpoint.trim().to_string()
    };
    let vault_meta =
        fetch_morpho_vault_by_address(req.chain.evm_chain_id, &vault.to_hex(), &endpoint).await?;
    if !vault_meta
        .asset_address
        .eq_ignore_ascii_case(&inputs.token.to_hex())
    {
        return Err(Error::new(
            Code::Usage,
            "selected morpho vault asset does not match --asset",
        ));
    }
    let client = RpcClient::connect(&inputs.rpc_url)?;

    let mut action = Action::new(
        crate::action::new_action_id(),
        format!("yield_{verb}"),
        &req.chain.caip2,
        Constraints {
            simulate: req.simulate,
            ..Default::default()
        },
    );
    action.provider = "morpho".into();
    action.from_address = inputs.sender.to_hex();
    action.to_address = inputs.recipient.to_hex();
    action.input_amount = inputs.amount.to_string();
    action.metadata = Some(obj(&[
        ("protocol", "morpho"),
        ("asset_id", &req.asset.asset_id),
        ("vault_address", &vault.to_hex()),
        ("vault_kind", &vault_meta.kind),
        ("yield_action", verb),
        ("yield_product", "vault"),
        ("recipient", &inputs.recipient.to_hex()),
        ("on_behalf_of", &inputs.on_behalf_of.to_hex()),
    ]));

    match req.verb {
        MorphoVaultYieldVerb::Deposit => {
            append_approval_if_needed(
                &client,
                &mut action,
                &req.chain.caip2,
                &inputs.rpc_url,
                inputs.token,
                inputs.sender,
                vault,
                inputs.amount,
                "Approve token for Morpho vault deposit",
            )
            .await?;
            let data = encode_fn(
                ERC4626_VAULT_ABI,
                "deposit",
                &[
                    uint256(inputs.amount),
                    DynSolValue::Address(inputs.recipient.into_inner()),
                ],
            )?;
            action.steps.push(step(
                "morpho-vault-deposit",
                StepType::Lend,
                &req.chain.caip2,
                &inputs.rpc_url,
                "Deposit asset into Morpho vault",
                &vault.to_hex(),
                &data,
            ));
        }
        MorphoVaultYieldVerb::Withdraw => {
            let data = encode_fn(
                ERC4626_VAULT_ABI,
                "withdraw",
                &[
                    uint256(inputs.amount),
                    DynSolValue::Address(inputs.recipient.into_inner()),
                    DynSolValue::Address(inputs.on_behalf_of.into_inner()),
                ],
            )?;
            action.steps.push(step(
                "morpho-vault-withdraw",
                StepType::Lend,
                &req.chain.caip2,
                &inputs.rpc_url,
                "Withdraw asset from Morpho vault",
                &vault.to_hex(),
                &data,
            ));
        }
    }
    Ok(action)
}

// =============================================================================
// MOONWELL LEND (RPC + Multicall3).
// =============================================================================

/// Build a Moonwell lend action. Parity with Go `BuildMoonwellLendAction`.
pub async fn build_moonwell_lend_action(req: MoonwellLendRequest) -> Result<Action, Error> {
    let verb = req.verb.as_str().to_string();
    let sender = req.sender.trim();
    if !address::is_hex_address(sender) {
        return Err(Error::new(
            Code::Usage,
            "lend action requires sender address",
        ));
    }
    let recipient = if req.recipient.trim().is_empty() {
        sender
    } else {
        req.recipient.trim()
    };
    if !address::is_hex_address(recipient) {
        return Err(Error::new(Code::Usage, "invalid recipient address"));
    }
    if !recipient.eq_ignore_ascii_case(sender) {
        return Err(Error::new(
            Code::Unsupported,
            "moonwell does not support alternate recipients; Compound v2 calls operate on msg.sender only",
        ));
    }
    if !address::is_hex_address(req.asset.address.trim()) {
        return Err(Error::new(
            Code::Usage,
            "moonwell lend asset must resolve to an ERC20 address",
        ));
    }
    let amount = parse_positive_amount(&req.amount_base_units).ok_or_else(|| {
        Error::new(
            Code::Usage,
            "lend amount must be a positive integer in base units",
        )
    })?;
    let rpc_url = resolve_rpc(&req.rpc_url, req.chain.evm_chain_id)?;
    let client = RpcClient::connect(&rpc_url)?;

    let sender_addr = address::parse(sender)?;
    let recipient_addr = address::parse(recipient)?;
    let token = address::parse(req.asset.address.trim())?;

    let mtoken =
        resolve_moonwell_mtoken(Some(&client), &req.chain, &req.mtoken_address, &token).await?;

    let mut action = Action::new(
        crate::action::new_action_id(),
        format!("lend_{verb}"),
        &req.chain.caip2,
        Constraints {
            simulate: req.simulate,
            ..Default::default()
        },
    );
    action.provider = "moonwell".into();
    action.from_address = sender_addr.to_hex();
    action.to_address = recipient_addr.to_hex();
    action.input_amount = amount.to_string();
    action.metadata = Some(obj(&[
        ("protocol", "moonwell"),
        ("asset_id", &req.asset.asset_id),
        ("mtoken", &mtoken.to_hex()),
        ("lending_action", &verb),
    ]));

    match req.verb {
        AaveLendVerb::Supply => {
            append_approval_if_needed(
                &client,
                &mut action,
                &req.chain.caip2,
                &rpc_url,
                token,
                sender_addr,
                mtoken,
                amount,
                "Approve token for Moonwell supply",
            )
            .await?;
            append_enter_markets_if_needed(
                &client,
                &mut action,
                req.chain.evm_chain_id,
                &req.chain.caip2,
                &rpc_url,
                sender_addr,
                mtoken,
            )
            .await?;
            let data = encode_fn(MOONWELL_MTOKEN_ABI, "mint", &[uint256(amount)])?;
            action.steps.push(step(
                "moonwell-supply",
                StepType::Lend,
                &req.chain.caip2,
                &rpc_url,
                "Supply asset to Moonwell",
                &mtoken.to_hex(),
                &data,
            ));
        }
        AaveLendVerb::Withdraw => {
            let data = encode_fn(MOONWELL_MTOKEN_ABI, "redeemUnderlying", &[uint256(amount)])?;
            action.steps.push(step(
                "moonwell-withdraw",
                StepType::Lend,
                &req.chain.caip2,
                &rpc_url,
                "Withdraw asset from Moonwell",
                &mtoken.to_hex(),
                &data,
            ));
        }
        AaveLendVerb::Borrow => {
            let data = encode_fn(MOONWELL_MTOKEN_ABI, "borrow", &[uint256(amount)])?;
            action.steps.push(step(
                "moonwell-borrow",
                StepType::Lend,
                &req.chain.caip2,
                &rpc_url,
                "Borrow asset from Moonwell",
                &mtoken.to_hex(),
                &data,
            ));
        }
        AaveLendVerb::Repay => {
            append_approval_if_needed(
                &client,
                &mut action,
                &req.chain.caip2,
                &rpc_url,
                token,
                sender_addr,
                mtoken,
                amount,
                "Approve token for Moonwell repay",
            )
            .await?;
            let data = encode_fn(MOONWELL_MTOKEN_ABI, "repayBorrow", &[uint256(amount)])?;
            action.steps.push(step(
                "moonwell-repay",
                StepType::Lend,
                &req.chain.caip2,
                &rpc_url,
                "Repay borrowed asset on Moonwell",
                &mtoken.to_hex(),
                &data,
            ));
        }
        AaveLendVerb::Unsupported(_) => {
            return Err(Error::new(
                Code::Usage,
                "unsupported moonwell lend action verb",
            ));
        }
    }
    Ok(action)
}

/// Resolve the Moonwell mToken for an underlying asset, parity with Go
/// `resolveMoonwellMToken`. An explicit address is returned verbatim (validated);
/// otherwise `Comptroller.getAllMarkets()` + Multicall3 `underlying()` selects
/// the matching mToken. The `client` may be `None` when an explicit mToken is
/// given or the chain has no comptroller (those paths never dial).
pub async fn resolve_moonwell_mtoken(
    client: Option<&RpcClient>,
    chain: &Chain,
    mtoken_address: &str,
    underlying: &Address,
) -> Result<Address, Error> {
    let chain_id = chain.evm_chain_id;
    if !mtoken_address.trim().is_empty() {
        if !address::is_hex_address(mtoken_address.trim()) {
            return Err(Error::new(
                Code::Usage,
                "invalid --pool-address (mToken address)",
            ));
        }
        return address::parse(mtoken_address.trim());
    }
    let comptroller = moonwell_comptroller(chain_id).ok_or_else(|| {
        Error::new(
            Code::Unsupported,
            "moonwell is not supported on this chain; pass --pool-address with the mToken address",
        )
    })?;
    let client = client.ok_or_else(|| {
        Error::new(
            Code::Unavailable,
            "moonwell mToken resolution requires an rpc client",
        )
    })?;
    let comptroller = address::parse(comptroller)?;

    let get_all = Function::from_abi_json(MOONWELL_COMPTROLLER_ABI, "getAllMarkets")?;
    let data = get_all.encode(&[])?;
    let out = client
        .call(&CallRequest::new(None, Some(comptroller), U256::ZERO, data))
        .await?;
    let decoded = get_all.decode_output(&out)?;
    let markets: Vec<Address> = decoded
        .first()
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_address().map(Address::from))
                .collect()
        })
        .ok_or_else(|| Error::new(Code::Unavailable, "invalid getAllMarkets response"))?;

    let underlying_cd = Function::from_abi_json(MOONWELL_MTOKEN_ABI, "underlying")?.encode(&[])?;
    let mc3 = address::parse(MULTICALL3_ADDR)?;
    let agg = Function::from_abi_json(MULTICALL3_ABI, "aggregate3")?;
    let calls: Vec<DynSolValue> = markets
        .iter()
        .map(|mt| {
            DynSolValue::Tuple(vec![
                DynSolValue::Address(mt.into_inner()),
                DynSolValue::Bool(true),
                DynSolValue::Bytes(underlying_cd.clone()),
            ])
        })
        .collect();
    let agg_data = agg.encode(&[DynSolValue::Array(calls)])?;
    let agg_out = client
        .call(&CallRequest::new(None, Some(mc3), U256::ZERO, agg_data))
        .await?;
    let agg_decoded = agg.decode_output(&agg_out)?;
    let results = agg_decoded
        .first()
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::new(Code::Unavailable, "empty aggregate3 response"))?;

    for (i, r) in results.iter().enumerate() {
        let tuple = match r.as_tuple() {
            Some(t) => t,
            None => continue,
        };
        let success = tuple.first().and_then(|v| v.as_bool()).unwrap_or(false);
        let return_data = tuple
            .get(1)
            .and_then(|v| v.as_bytes())
            .map(|b| b.to_vec())
            .unwrap_or_default();
        if !success || return_data.len() < 32 {
            continue;
        }
        let addr = Address::from(alloy::primitives::Address::from_slice(&return_data[12..32]));
        if addr.to_hex().eq_ignore_ascii_case(&underlying.to_hex()) {
            return Ok(markets[i]);
        }
    }
    Err(Error::new(
        Code::Unsupported,
        format!(
            "no moonwell mToken found for underlying {} on chain {chain_id}; pass --pool-address with the mToken address",
            underlying.to_hex()
        ),
    ))
}

// =============================================================================
// Shared helpers.
// =============================================================================

/// Normalized lend inputs (parity with Go `normalizeLendInputs`).
struct LendInputs {
    sender: Address,
    recipient: Address,
    on_behalf_of: Address,
    amount: U256,
    rpc_url: String,
    token: Address,
}

fn normalize_lend_inputs(
    sender: &str,
    recipient: &str,
    on_behalf_of: &str,
    asset_address: &str,
    amount_base_units: &str,
    rpc_url_override: &str,
    chain_id: i64,
) -> Result<LendInputs, Error> {
    let sender = sender.trim();
    if !address::is_hex_address(sender) {
        return Err(Error::new(
            Code::Usage,
            "lend action requires sender address",
        ));
    }
    let recipient_raw = if recipient.trim().is_empty() {
        sender
    } else {
        recipient.trim()
    };
    if !address::is_hex_address(recipient_raw) {
        return Err(Error::new(Code::Usage, "invalid recipient address"));
    }
    let on_behalf_raw = if on_behalf_of.trim().is_empty() {
        sender
    } else {
        on_behalf_of.trim()
    };
    if !address::is_hex_address(on_behalf_raw) {
        return Err(Error::new(Code::Usage, "invalid on-behalf-of address"));
    }
    if !address::is_hex_address(asset_address.trim()) {
        return Err(Error::new(
            Code::Usage,
            "lend asset must resolve to an ERC20 address",
        ));
    }
    let amount = parse_positive_amount(amount_base_units).ok_or_else(|| {
        Error::new(
            Code::Usage,
            "lend amount must be a positive integer in base units",
        )
    })?;
    let rpc_url = resolve_rpc(rpc_url_override, chain_id)?;
    Ok(LendInputs {
        sender: address::parse(sender)?,
        recipient: address::parse(recipient_raw)?,
        on_behalf_of: address::parse(on_behalf_raw)?,
        amount,
        rpc_url,
        token: address::parse(asset_address.trim())?,
    })
}

async fn resolve_aave_pool_address(
    client: &RpcClient,
    chain_id: i64,
    pool_address: &str,
    pool_provider: &str,
) -> Result<Address, Error> {
    if !pool_address.trim().is_empty() {
        if !address::is_hex_address(pool_address.trim()) {
            return Err(Error::new(Code::Usage, "invalid --pool-address"));
        }
        return address::parse(pool_address.trim());
    }
    let mut provider_addr = pool_provider.trim().to_string();
    if provider_addr.is_empty() {
        if let Some(discovered) = aave_pool_address_provider(chain_id) {
            provider_addr = discovered.to_string();
        }
    }
    if provider_addr.is_empty() {
        return Err(Error::new(
            Code::Unsupported,
            "aave pool address provider is unavailable for this chain; pass --pool-address or --pool-address-provider",
        ));
    }
    if !address::is_hex_address(&provider_addr) {
        return Err(Error::new(Code::Usage, "invalid --pool-address-provider"));
    }
    let provider = address::parse(&provider_addr)?;
    let get_pool = Function::from_abi_json(AAVE_POOL_ADDRESS_PROVIDER_ABI, "getPool")?;
    let data = get_pool.encode(&[])?;
    let out = client
        .call(&CallRequest::new(None, Some(provider), U256::ZERO, data))
        .await?;
    let decoded = get_pool.decode_output(&out)?;
    let pool = decoded
        .first()
        .and_then(|v| v.as_address())
        .map(Address::from)
        .ok_or_else(|| Error::new(Code::Unavailable, "invalid aave pool response"))?;
    if pool.is_zero() {
        return Err(Error::new(Code::Unavailable, "aave pool address is zero"));
    }
    Ok(pool)
}

async fn resolve_incentives_controller(
    client: &RpcClient,
    chain_id: i64,
    controller_address: &str,
    pool_provider: &str,
) -> Result<Address, Error> {
    if !controller_address.trim().is_empty() {
        if !address::is_hex_address(controller_address.trim()) {
            return Err(Error::new(Code::Usage, "invalid --controller-address"));
        }
        return address::parse(controller_address.trim());
    }
    let mut provider_addr = pool_provider.trim().to_string();
    if provider_addr.is_empty() {
        if let Some(discovered) = aave_pool_address_provider(chain_id) {
            provider_addr = discovered.to_string();
        }
    }
    if provider_addr.is_empty() {
        return Err(Error::new(
            Code::Unsupported,
            "aave incentives controller is unavailable for this chain; pass --controller-address",
        ));
    }
    if !address::is_hex_address(&provider_addr) {
        return Err(Error::new(Code::Usage, "invalid --pool-address-provider"));
    }
    let provider = address::parse(&provider_addr)?;
    let slot = alloy::primitives::keccak256(b"INCENTIVES_CONTROLLER");
    let get_address = Function::from_abi_json(AAVE_POOL_ADDRESS_PROVIDER_ABI, "getAddress")?;
    let data = get_address.encode(&[DynSolValue::FixedBytes(slot, 32)])?;
    let out = client
        .call(&CallRequest::new(None, Some(provider), U256::ZERO, data))
        .await?;
    let decoded = get_address.decode_output(&out)?;
    let controller = decoded
        .first()
        .and_then(|v| v.as_address())
        .map(Address::from)
        .ok_or_else(|| Error::new(Code::Unavailable, "invalid incentives controller response"))?;
    if controller.is_zero() {
        return Err(Error::new(
            Code::Unavailable,
            "incentives controller address is zero",
        ));
    }
    Ok(controller)
}

async fn append_approval_if_needed(
    client: &RpcClient,
    action: &mut Action,
    chain_id: &str,
    rpc_url: &str,
    token: Address,
    owner: Address,
    spender: Address,
    amount: U256,
    description: &str,
) -> Result<(), Error> {
    let allowance_fn = Function::from_abi_json(ERC20_MINIMAL_ABI, "allowance")?;
    let data = allowance_fn.encode(&[
        DynSolValue::Address(owner.into_inner()),
        DynSolValue::Address(spender.into_inner()),
    ])?;
    let raw = client
        .call(&CallRequest::new(
            Some(owner),
            Some(token),
            U256::ZERO,
            data,
        ))
        .await?;
    let decoded = allowance_fn.decode_output(&raw)?;
    let current = decoded
        .first()
        .and_then(|v| v.as_uint())
        .map(|(v, _)| v)
        .ok_or_else(|| Error::new(Code::Unavailable, "invalid allowance response"))?;
    if current >= amount {
        return Ok(());
    }
    let approve_data = encode_erc20("approve", spender, amount)?;
    let step_id = format!(
        "approve-{}",
        token.to_hex().trim_start_matches("0x").to_lowercase()
    );
    action.steps.push(step(
        &step_id,
        StepType::Approval,
        chain_id,
        rpc_url,
        description,
        &token.to_hex(),
        &approve_data,
    ));
    Ok(())
}

async fn append_enter_markets_if_needed(
    client: &RpcClient,
    action: &mut Action,
    chain_id: i64,
    caip2: &str,
    rpc_url: &str,
    sender: Address,
    mtoken: Address,
) -> Result<(), Error> {
    let comptroller = match moonwell_comptroller(chain_id) {
        Some(c) => address::parse(c)?,
        None => return Ok(()),
    };
    let check = Function::from_abi_json(MOONWELL_COMPTROLLER_ABI, "checkMembership")?;
    let check_data = check.encode(&[
        DynSolValue::Address(sender.into_inner()),
        DynSolValue::Address(mtoken.into_inner()),
    ])?;
    let out = client
        .call(&CallRequest::new(
            None,
            Some(comptroller),
            U256::ZERO,
            check_data,
        ))
        .await?;
    let decoded = check.decode_output(&out)?;
    if decoded.first().and_then(|v| v.as_bool()).unwrap_or(false) {
        return Ok(());
    }
    let enter_data = encode_fn(
        MOONWELL_COMPTROLLER_ABI,
        "enterMarkets",
        &[DynSolValue::Array(vec![DynSolValue::Address(
            mtoken.into_inner(),
        )])],
    )?;
    action.steps.push(step(
        "moonwell-enter-market",
        StepType::Lend,
        caip2,
        rpc_url,
        "Enable asset as collateral on Moonwell",
        &comptroller.to_hex(),
        &enter_data,
    ));
    Ok(())
}

fn normalize_address_list(values: &[String]) -> Result<Vec<String>, Error> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for value in values {
        for part in value.split(',') {
            let norm = part.trim();
            if norm.is_empty() {
                continue;
            }
            if !address::is_hex_address(norm) {
                return Err(Error::new(
                    Code::Usage,
                    format!("invalid address in --assets: {norm}"),
                ));
            }
            let canonical = address::parse(norm)?.to_hex();
            if seen.insert(canonical.clone()) {
                out.push(canonical);
            }
        }
    }
    Ok(out)
}

fn parse_reward_amount(v: &str) -> Result<U256, Error> {
    let clean = v.trim();
    if clean.is_empty() || clean.eq_ignore_ascii_case("max") {
        return Ok(U256::MAX);
    }
    parse_positive_amount(clean).ok_or_else(|| {
        Error::new(
            Code::Usage,
            "reward amount must be a positive integer in base units or 'max'",
        )
    })
}

fn normalize_morpho_market_id(market_id: &str) -> Result<String, Error> {
    let clean = market_id.trim();
    if clean.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "morpho lend execution requires --market-id",
        ));
    }
    if !clean.starts_with("0x") && !clean.starts_with("0X") {
        return Err(Error::new(
            Code::Usage,
            "morpho --market-id must be a 0x-prefixed bytes32 value",
        ));
    }
    let raw = &clean[2..];
    if raw.len() != 64 {
        return Err(Error::new(
            Code::Usage,
            "morpho --market-id must be a 32-byte hex value",
        ));
    }
    if hex::decode(raw).is_err() {
        return Err(Error::new(
            Code::Usage,
            "morpho --market-id must be valid hex",
        ));
    }
    Ok(format!("0x{}", raw.to_lowercase()))
}

/// Resolved fields from a Morpho market GraphQL lookup.
struct MorphoMarket {
    morpho_address: String,
    oracle_address: String,
    irm: String,
    lltv: String,
    loan_asset_address: String,
    collateral_address: String,
}

async fn fetch_morpho_market_by_id(
    chain_id: i64,
    market_id: &str,
    endpoint: &str,
) -> Result<MorphoMarket, Error> {
    let query = r#"query Market($chain:Int!,$key:String!){
  markets(first: 1, where:{ chainId_in: [$chain], uniqueKey_in: [$key], listed: true }){
    items{ uniqueKey irmAddress lltv morphoBlue{ address } oracle{ address }
      loanAsset{ address symbol decimals chain{ id } }
      collateralAsset{ address symbol decimals }
      state{ supplyAssetsUsd liquidityAssetsUsd } } } }"#;
    let body = serde_json::json!({
        "query": query,
        "variables": { "chain": chain_id, "key": market_id },
    });
    let resp: serde_json::Value = graphql_post(endpoint, &body).await?;
    if let Some(errors) = resp.get("errors").and_then(|e| e.as_array()) {
        if let Some(first) = errors.first() {
            let msg = first.get("message").and_then(|m| m.as_str()).unwrap_or("");
            return Err(Error::new(
                Code::Unavailable,
                format!("morpho graphql error: {msg}"),
            ));
        }
    }
    let item = resp
        .pointer("/data/markets/items/0")
        .ok_or_else(|| Error::new(Code::Usage, "morpho market-id not found for selected chain"))?;
    Ok(MorphoMarket {
        morpho_address: gql_str(item, "/morphoBlue/address"),
        oracle_address: gql_str(item, "/oracle/address"),
        irm: gql_str(item, "/irmAddress"),
        lltv: gql_str(item, "/lltv"),
        loan_asset_address: gql_str(item, "/loanAsset/address"),
        collateral_address: gql_str(item, "/collateralAsset/address"),
    })
}

/// Resolved fields from a Morpho vault GraphQL lookup.
struct MorphoVault {
    asset_address: String,
    kind: String,
}

async fn fetch_morpho_vault_by_address(
    chain_id: i64,
    address: &str,
    endpoint: &str,
) -> Result<MorphoVault, Error> {
    let query = r#"query VaultByAddress($address:String!,$chainId:Int!){
  vaultByAddress(address:$address, chainId:$chainId){ address listed
    asset{ address symbol decimals chain{ id } } } }"#;
    let body = serde_json::json!({
        "query": query,
        "variables": { "address": address, "chainId": chain_id },
    });
    let resp: serde_json::Value = graphql_post(endpoint, &body).await?;
    if let Some(errors) = resp.get("errors").and_then(|e| e.as_array()) {
        if let Some(first) = errors.first() {
            let msg = first.get("message").and_then(|m| m.as_str()).unwrap_or("");
            if !is_morpho_lookup_not_found(msg) {
                return Err(Error::new(
                    Code::Unavailable,
                    format!("morpho graphql error: {msg}"),
                ));
            }
        }
    }
    if let Some(vault) = resp.pointer("/data/vaultByAddress") {
        if !vault.is_null() {
            if !vault
                .get("listed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return Err(Error::new(Code::Unsupported, "morpho vault is not listed"));
            }
            let asset = gql_str(vault, "/asset/address");
            if !address::is_hex_address(&asset) {
                return Err(Error::new(
                    Code::Unavailable,
                    "morpho vault missing asset metadata",
                ));
            }
            return Ok(MorphoVault {
                asset_address: address::parse(&asset)?.to_hex(),
                kind: "vault".into(),
            });
        }
    }
    Err(Error::new(
        Code::Usage,
        "morpho vault address not found for selected chain",
    ))
}

fn is_morpho_lookup_not_found(message: &str) -> bool {
    message
        .trim()
        .to_lowercase()
        .contains("no results matching given parameters")
}

async fn graphql_post(
    endpoint: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, Error> {
    let resp = reqwest::Client::new()
        .post(endpoint)
        .json(body)
        .send()
        .await
        .map_err(|e| Error::wrap(Code::Unavailable, "morpho graphql request", to_cause(e)))?;
    resp.json::<serde_json::Value>().await.map_err(|e| {
        Error::wrap(
            Code::Unavailable,
            "decode morpho graphql response",
            to_cause(e),
        )
    })
}

fn gql_str(value: &serde_json::Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn resolve_rate_mode(mode: i64) -> Result<i64, Error> {
    let m = if mode == 0 { 2 } else { mode };
    if m != 1 && m != 2 {
        return Err(Error::new(
            Code::Usage,
            "borrow interest rate mode must be 1 (stable) or 2 (variable)",
        ));
    }
    Ok(m)
}

fn resolve_rpc(override_url: &str, chain_id: i64) -> Result<String, Error> {
    resolve_rpc_url(override_url, chain_id)
        .map_err(|e| Error::wrap(Code::Usage, "resolve rpc url", to_cause(e)))
}

fn parse_positive_amount(value: &str) -> Option<U256> {
    let v = value.trim();
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let parsed = U256::from_str_radix(v, 10).ok()?;
    if parsed.is_zero() {
        return None;
    }
    Some(parsed)
}

fn uint256(v: U256) -> DynSolValue {
    DynSolValue::Uint(v, 256)
}

fn encode_erc20(name: &str, addr: Address, amount: U256) -> Result<String, Error> {
    encode_fn(
        ERC20_MINIMAL_ABI,
        name,
        &[DynSolValue::Address(addr.into_inner()), uint256(amount)],
    )
}

fn encode_aave(name: &str, args: &[DynSolValue]) -> Result<String, Error> {
    encode_fn(AAVE_POOL_ABI, name, args)
}

fn encode_fn(abi_json: &str, name: &str, args: &[DynSolValue]) -> Result<String, Error> {
    let func = Function::from_abi_json(abi_json, name)?;
    let data = func.encode(args)?;
    Ok(format!("0x{}", hex::encode(data)))
}

fn step(
    step_id: &str,
    step_type: StepType,
    chain_id: &str,
    rpc_url: &str,
    description: &str,
    target: &str,
    data: &str,
) -> ActionStep {
    ActionStep {
        step_id: step_id.into(),
        step_type,
        status: StepStatus::Pending,
        chain_id: chain_id.into(),
        rpc_url: rpc_url.into(),
        description: description.into(),
        target: target.into(),
        data: data.into(),
        value: "0".into(),
        calls: Vec::new(),
        expected_outputs: None,
        tx_hash: String::new(),
        error: String::new(),
    }
}

fn obj(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    for (k, v) in pairs {
        m.insert((*k).into(), serde_json::Value::String((*v).into()));
    }
    m
}

/// A concrete cause carrying an error's display text.
#[derive(Debug)]
struct MsgError(String);

impl std::fmt::Display for MsgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MsgError {}

fn to_cause<E: std::fmt::Display>(e: E) -> MsgError {
    MsgError(e.to_string())
}

#[cfg(test)]
mod tests {
    use crate::action::{Action, StepType};
    use crate::planner::{
        build_aave_lend_action, build_aave_rewards_claim_action,
        build_aave_rewards_compound_action, build_approval_action, build_moonwell_lend_action,
        build_morpho_lend_action, build_morpho_vault_yield_action, build_transfer_action,
        resolve_moonwell_mtoken, AaveLendRequest, AaveLendVerb, AaveRewardsClaimRequest,
        AaveRewardsCompoundRequest, ApprovalRequest, MoonwellLendRequest, MorphoLendRequest,
        MorphoVaultYieldRequest, MorphoVaultYieldVerb, TransferRequest,
    };
    use defi_evm::abi::Function;
    use defi_evm::address::{self, Address};
    use defi_id::{parse_asset, parse_chain, Asset, Chain};
    use defi_registry::ERC20_MINIMAL_ABI;

    use alloy::dyn_abi::DynSolValue;
    use alloy::primitives::U256;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ---- canonical test addresses (mirror the Go planner tests) ----------
    const SENDER: &str = "0x00000000000000000000000000000000000000AA";
    const SPENDER: &str = "0x00000000000000000000000000000000000000BB";
    const RECIPIENT: &str = "0x00000000000000000000000000000000000000BB";
    const POOL: &str = "0x00000000000000000000000000000000000000CC";
    const ZERO_ADDR: &str = "0x0000000000000000000000000000000000000000";
    // USDC on Ethereum (lowercase) — matches the Go morpho/vault GraphQL fixtures.
    const USDC_ETH: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
    const MORPHO_BLUE: &str = "0xBBBBBbbBBb9cC5e90e3b3Af64bdAF62C37EEFFCb";
    const MORPHO_MARKET_ID: &str =
        "0x64d65c9a2d91c36d56fbc42d69e979335320169b3df63bf92789e2c8883fcc64";
    const VAULT_ADDR: &str = "0x1111111111111111111111111111111111111111";
    const M_TOKEN: &str = "0x0000000000000000000000000000000000000011";

    // -- helpers -----------------------------------------------------------

    fn eth_chain() -> Chain {
        parse_chain("ethereum").expect("parse ethereum")
    }

    fn base_chain() -> Chain {
        parse_chain("8453").expect("parse base by id")
    }

    fn usdc(chain: &Chain) -> Asset {
        parse_asset("USDC", chain).expect("parse USDC")
    }

    fn step_types(action: &Action) -> Vec<StepType> {
        action.steps.iter().map(|s| s.step_type).collect()
    }

    fn step_ids(action: &Action) -> Vec<String> {
        action.steps.iter().map(|s| s.step_id.clone()).collect()
    }

    /// The `0x`-prefixed approve(spender, amount) calldata, used to assert the
    /// emitted approval step encodes the same bytes as go-ethereum / alloy.
    fn approve_calldata(spender: &str, amount: u128) -> String {
        let func = Function::from_abi_json(ERC20_MINIMAL_ABI, "approve").expect("approve fn");
        let spender = address::parse(spender).expect("spender");
        let data = func
            .encode(&[
                DynSolValue::Address(spender.into_inner()),
                DynSolValue::Uint(U256::from(amount), 256),
            ])
            .expect("encode approve");
        format!("0x{}", hex::encode(data))
    }

    /// A `wiremock` JSON-RPC endpoint that answers every `eth_call` with an
    /// ABI-encoded `allowance` value (mirrors Go `newPlannerRPCServer`). Used by
    /// the lend/yield builders whose only on-chain read is the allowance check.
    async fn allowance_rpc(allowance: u128) -> MockServer {
        let server = MockServer::start().await;
        // allowance() output is a single ABI uint256 word. The responder echoes
        // the request id (alloy correlates responses by id) so it stays correct
        // regardless of how many requests the planner issues.
        let result = format!("0x{}", hex::encode(encode_uint_word(allowance)));
        Mock::given(method("POST"))
            .respond_with(EchoIdResponder { result })
            .mount(&server)
            .await;
        server
    }

    /// A `wiremock` responder that wraps `result` in a JSON-RPC success envelope,
    /// echoing the incoming request `id`.
    struct EchoIdResponder {
        result: String,
    }

    impl wiremock::Respond for EchoIdResponder {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            let id = serde_json::from_slice::<serde_json::Value>(&request.body)
                .ok()
                .and_then(|body| body.get("id").cloned())
                .unwrap_or_else(|| serde_json::Value::from(1));
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": self.result,
            }))
        }
    }

    /// Fallback uint256 word encoder (used if the abi helper is unavailable).
    fn encode_uint_word(v: u128) -> Vec<u8> {
        let u = U256::from(v);
        u.to_be_bytes::<32>().to_vec()
    }

    /// A `wiremock` GraphQL endpoint that returns the Go morpho market fixture.
    async fn morpho_market_graphql() -> MockServer {
        let server = MockServer::start().await;
        let body = format!(
            r#"{{"data":{{"markets":{{"items":[{{
                "uniqueKey":"{MORPHO_MARKET_ID}",
                "irmAddress":"0x870aC11D48B15DB9a138Cf899d20F13F79Ba00BC",
                "lltv":"860000000000000000",
                "morphoBlue":{{"address":"{MORPHO_BLUE}"}},
                "oracle":{{"address":"0xA6D6950c9F177F1De7f7757FB33539e3Ec60182a"}},
                "loanAsset":{{"address":"{USDC_ETH}","symbol":"USDC","decimals":6,"chain":{{"id":1}}}},
                "collateralAsset":{{"address":"0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf","symbol":"cbBTC","decimals":8}}
            }}]}}}}}}"#
        );
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        server
    }

    /// A `wiremock` GraphQL endpoint that returns the Go morpho VAULT fixture.
    async fn morpho_vault_graphql() -> MockServer {
        let server = MockServer::start().await;
        let body = format!(
            r#"{{"data":{{"vaultByAddress":{{
                "address":"{VAULT_ADDR}",
                "listed":true,
                "asset":{{"address":"{USDC_ETH}","symbol":"USDC","decimals":6,"chain":{{"id":1}}}}
            }}}}}}"#
        );
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        server
    }

    // =====================================================================
    // APPROVAL — Go approvals_test.go
    // =====================================================================

    // A1, A2, A4 — Ported from Go: TestBuildApprovalAction (+ calldata/target/
    // amount assertions are fresh spec-driven hardening of the same path).
    #[test]
    fn approval_action_emits_single_approval_step() {
        let chain = parse_chain("taiko").expect("parse taiko");
        let asset = usdc(&chain);
        let action = build_approval_action(ApprovalRequest {
            chain: chain.clone(),
            asset: asset.clone(),
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            spender: SPENDER.into(),
            simulate: true,
            rpc_url: "http://127.0.0.1:8545".into(),
        })
        .expect("build approval");

        assert_eq!(action.intent_type, "approve");
        assert_eq!(action.provider, "native");
        assert_eq!(action.steps.len(), 1);
        assert_eq!(action.steps[0].step_type, StepType::Approval);
        assert_eq!(action.steps[0].value, "0");
        // target == checksummed token address.
        assert!(
            address::eq_fold(&action.steps[0].target, &asset.address),
            "target {} != asset {}",
            action.steps[0].target,
            asset.address
        );
        // data == approve(spender, 1_000_000) calldata.
        assert_eq!(action.steps[0].data, approve_calldata(SPENDER, 1_000_000));
        assert_eq!(action.input_amount, "1000000");
        assert!(action.constraints.simulate);
        assert!(address::eq_fold(&action.from_address, SENDER));
        assert!(address::eq_fold(&action.to_address, SPENDER));
    }

    // A3 — Ported from Go: TestBuildApprovalActionRejectsInvalidAmount.
    #[test]
    fn approval_action_rejects_zero_amount() {
        let chain = parse_chain("taiko").expect("parse taiko");
        let asset = usdc(&chain);
        let err = build_approval_action(ApprovalRequest {
            chain,
            asset,
            amount_base_units: "0".into(),
            sender: SENDER.into(),
            spender: SPENDER.into(),
            simulate: false,
            rpc_url: String::new(),
        })
        .expect_err("zero amount must be rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // A3 — fresh spec-driven: missing sender / spender are usage errors.
    #[test]
    fn approval_action_rejects_empty_sender_and_spender() {
        let chain = parse_chain("taiko").expect("parse taiko");
        let asset = usdc(&chain);
        let no_sender = build_approval_action(ApprovalRequest {
            chain: chain.clone(),
            asset: asset.clone(),
            amount_base_units: "1".into(),
            sender: "".into(),
            spender: SPENDER.into(),
            simulate: false,
            rpc_url: String::new(),
        })
        .expect_err("empty sender rejected");
        assert_eq!(no_sender.code, defi_errors::Code::Usage);

        let no_spender = build_approval_action(ApprovalRequest {
            chain,
            asset,
            amount_base_units: "1".into(),
            sender: SENDER.into(),
            spender: "".into(),
            simulate: false,
            rpc_url: String::new(),
        })
        .expect_err("empty spender rejected");
        assert_eq!(no_spender.code, defi_errors::Code::Usage);
    }

    // =====================================================================
    // TRANSFER — Go transfer_test.go
    // =====================================================================

    // T1 — Ported from Go: TestBuildTransferAction.
    #[test]
    fn transfer_action_emits_single_transfer_step() {
        let chain = parse_chain("taiko").expect("parse taiko");
        let asset = usdc(&chain);
        let action = build_transfer_action(TransferRequest {
            chain,
            asset: asset.clone(),
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: RECIPIENT.into(),
            simulate: true,
            rpc_url: "http://127.0.0.1:8545".into(),
        })
        .expect("build transfer");

        assert_eq!(action.intent_type, "transfer");
        assert_eq!(action.provider, "native");
        assert_eq!(action.steps.len(), 1);
        assert_eq!(action.steps[0].step_type, StepType::Transfer);
        assert_eq!(action.steps[0].value, "0");
        assert!(address::eq_fold(&action.steps[0].target, &asset.address));
    }

    // T3 — Ported from Go: TestBuildTransferActionRejectsInvalidAmount.
    #[test]
    fn transfer_action_rejects_zero_amount() {
        let chain = parse_chain("taiko").expect("parse taiko");
        let asset = usdc(&chain);
        let err = build_transfer_action(TransferRequest {
            chain,
            asset,
            amount_base_units: "0".into(),
            sender: SENDER.into(),
            recipient: RECIPIENT.into(),
            simulate: false,
            rpc_url: String::new(),
        })
        .expect_err("zero amount rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // T3 — Ported from Go: TestBuildTransferActionRejectsZeroRecipient.
    #[test]
    fn transfer_action_rejects_zero_recipient() {
        let chain = parse_chain("taiko").expect("parse taiko");
        let asset = usdc(&chain);
        let err = build_transfer_action(TransferRequest {
            chain,
            asset,
            amount_base_units: "1000".into(),
            sender: SENDER.into(),
            recipient: ZERO_ADDR.into(),
            simulate: false,
            rpc_url: String::new(),
        })
        .expect_err("zero recipient rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // T2 — fresh spec-driven: non-EVM chain is unsupported.
    #[test]
    fn transfer_action_rejects_non_evm_chain() {
        let chain = parse_chain("solana").expect("parse solana");
        // A symbol asset on solana — only the chain check needs to fire.
        let asset = parse_asset("USDC", &chain).expect("parse solana USDC");
        let err = build_transfer_action(TransferRequest {
            chain,
            asset,
            amount_base_units: "1000".into(),
            sender: SENDER.into(),
            recipient: RECIPIENT.into(),
            simulate: false,
            rpc_url: String::new(),
        })
        .expect_err("non-evm chain rejected");
        assert_eq!(err.code, defi_errors::Code::Unsupported);
    }

    // =====================================================================
    // AAVE LEND — Go aave_test.go
    // =====================================================================

    // L1, L2 — Ported from Go: TestBuildAaveLendActionSupply.
    #[tokio::test]
    async fn aave_lend_supply_emits_approval_then_lend() {
        let rpc = allowance_rpc(0).await;
        let chain = eth_chain();
        let asset = usdc(&chain);
        let action = build_aave_lend_action(AaveLendRequest {
            verb: AaveLendVerb::Supply,
            chain,
            asset,
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: RECIPIENT.into(),
            on_behalf_of: String::new(),
            interest_rate_mode: 0,
            simulate: true,
            rpc_url: rpc.uri(),
            pool_address: POOL.into(),
            pool_addresses_provider: String::new(),
        })
        .await
        .expect("build aave supply");

        assert_eq!(action.provider, "aave");
        assert_eq!(action.intent_type, "lend_supply");
        assert_eq!(
            step_types(&action),
            vec![StepType::Approval, StepType::Lend]
        );
        assert!(
            address::eq_fold(&action.steps[1].target, POOL),
            "lend target {} != pool {POOL}",
            action.steps[1].target
        );
        // metadata carries the Aave protocol context.
        let meta = action.metadata.as_ref().expect("metadata present");
        assert_eq!(meta.get("protocol").and_then(|v| v.as_str()), Some("aave"));
        assert_eq!(
            meta.get("lending_action").and_then(|v| v.as_str()),
            Some("supply")
        );
        assert!(meta.contains_key("pool"));
        assert!(meta.contains_key("on_behalf_of"));
        assert!(meta.contains_key("recipient"));
        assert!(meta.contains_key("rate_mode"));
    }

    // L7 — fresh spec-driven: a sufficient allowance skips the approval step.
    #[tokio::test]
    async fn aave_lend_supply_skips_approval_when_allowance_sufficient() {
        let rpc = allowance_rpc(10_000_000).await;
        let chain = eth_chain();
        let asset = usdc(&chain);
        let action = build_aave_lend_action(AaveLendRequest {
            verb: AaveLendVerb::Supply,
            chain,
            asset,
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: String::new(),
            on_behalf_of: String::new(),
            interest_rate_mode: 0,
            simulate: true,
            rpc_url: rpc.uri(),
            pool_address: POOL.into(),
            pool_addresses_provider: String::new(),
        })
        .await
        .expect("build aave supply");
        assert_eq!(step_types(&action), vec![StepType::Lend]);
    }

    // L3 — fresh spec-driven: withdraw is a single lend_call (no approval).
    #[tokio::test]
    async fn aave_lend_withdraw_is_single_lend_call() {
        let rpc = allowance_rpc(0).await;
        let chain = eth_chain();
        let asset = usdc(&chain);
        let action = build_aave_lend_action(AaveLendRequest {
            verb: AaveLendVerb::Withdraw,
            chain,
            asset,
            amount_base_units: "500000".into(),
            sender: SENDER.into(),
            recipient: String::new(),
            on_behalf_of: String::new(),
            interest_rate_mode: 0,
            simulate: true,
            rpc_url: rpc.uri(),
            pool_address: POOL.into(),
            pool_addresses_provider: String::new(),
        })
        .await
        .expect("build aave withdraw");
        assert_eq!(action.intent_type, "lend_withdraw");
        assert_eq!(step_types(&action), vec![StepType::Lend]);
    }

    // L4 — fresh spec-driven: an out-of-range rate mode is a usage error.
    #[tokio::test]
    async fn aave_lend_borrow_rejects_invalid_rate_mode() {
        let rpc = allowance_rpc(0).await;
        let chain = eth_chain();
        let asset = usdc(&chain);
        let err = build_aave_lend_action(AaveLendRequest {
            verb: AaveLendVerb::Borrow,
            chain,
            asset,
            amount_base_units: "1000".into(),
            sender: SENDER.into(),
            recipient: String::new(),
            on_behalf_of: String::new(),
            interest_rate_mode: 3,
            simulate: true,
            rpc_url: rpc.uri(),
            pool_address: POOL.into(),
            pool_addresses_provider: String::new(),
        })
        .await
        .expect_err("invalid rate mode rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // L5 — Ported from Go: TestBuildAaveLendActionRequiresSender (validated
    // before any RPC dial, so no mock server is needed).
    #[tokio::test]
    async fn aave_lend_requires_sender() {
        let chain = eth_chain();
        let asset = usdc(&chain);
        let err = build_aave_lend_action(AaveLendRequest {
            verb: AaveLendVerb::Supply,
            chain,
            asset,
            amount_base_units: "1000000".into(),
            sender: String::new(),
            recipient: String::new(),
            on_behalf_of: String::new(),
            interest_rate_mode: 0,
            simulate: false,
            rpc_url: String::new(),
            pool_address: POOL.into(),
            pool_addresses_provider: String::new(),
        })
        .await
        .expect_err("missing sender rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // =====================================================================
    // AAVE REWARDS — Go aave_test.go
    // =====================================================================

    // R2 — Ported from Go: TestBuildAaveRewardsCompoundAction.
    #[tokio::test]
    async fn aave_rewards_compound_emits_claim_approval_supply() {
        let rpc = allowance_rpc(0).await;
        let chain = eth_chain();
        let action = build_aave_rewards_compound_action(AaveRewardsCompoundRequest {
            chain,
            sender: SENDER.into(),
            recipient: SENDER.into(),
            assets: vec!["0x00000000000000000000000000000000000000D1".into()],
            reward_token: "0x00000000000000000000000000000000000000D2".into(),
            amount_base_units: "1000".into(),
            simulate: true,
            rpc_url: rpc.uri(),
            controller_address: "0x00000000000000000000000000000000000000D3".into(),
            pool_address: "0x00000000000000000000000000000000000000D4".into(),
            pool_addresses_provider: String::new(),
            on_behalf_of: String::new(),
        })
        .await
        .expect("build compound");
        assert_eq!(action.intent_type, "compound_rewards");
        assert_eq!(
            step_types(&action),
            vec![StepType::Claim, StepType::Approval, StepType::Lend]
        );
    }

    // R3 — Ported from Go: TestBuildAaveRewardsCompoundActionRejectsRecipientMismatch.
    #[tokio::test]
    async fn aave_rewards_compound_rejects_recipient_mismatch() {
        let rpc = allowance_rpc(0).await;
        let chain = eth_chain();
        let err = build_aave_rewards_compound_action(AaveRewardsCompoundRequest {
            chain,
            sender: SENDER.into(),
            recipient: "0x00000000000000000000000000000000000000BB".into(),
            assets: vec!["0x00000000000000000000000000000000000000D1".into()],
            reward_token: "0x00000000000000000000000000000000000000D2".into(),
            amount_base_units: "1000".into(),
            simulate: true,
            rpc_url: rpc.uri(),
            controller_address: "0x00000000000000000000000000000000000000D3".into(),
            pool_address: "0x00000000000000000000000000000000000000D4".into(),
            pool_addresses_provider: String::new(),
            on_behalf_of: String::new(),
        })
        .await
        .expect_err("recipient mismatch rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // R3 — Ported from Go: TestBuildAaveRewardsCompoundActionRejectsInvalidOnBehalfOf.
    #[tokio::test]
    async fn aave_rewards_compound_rejects_invalid_on_behalf_of() {
        let rpc = allowance_rpc(0).await;
        let chain = eth_chain();
        let err = build_aave_rewards_compound_action(AaveRewardsCompoundRequest {
            chain,
            sender: SENDER.into(),
            recipient: SENDER.into(),
            assets: vec!["0x00000000000000000000000000000000000000D1".into()],
            reward_token: "0x00000000000000000000000000000000000000D2".into(),
            amount_base_units: "1000".into(),
            simulate: true,
            rpc_url: rpc.uri(),
            controller_address: "0x00000000000000000000000000000000000000D3".into(),
            pool_address: "0x00000000000000000000000000000000000000D4".into(),
            pool_addresses_provider: String::new(),
            on_behalf_of: "invalid".into(),
        })
        .await
        .expect_err("invalid on-behalf-of rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
        assert!(
            err.to_string().contains("invalid on-behalf-of address"),
            "unexpected error message: {err}"
        );
    }

    // R1 — fresh spec-driven: claim requires at least one asset.
    #[tokio::test]
    async fn aave_rewards_claim_requires_assets() {
        let rpc = allowance_rpc(0).await;
        let chain = eth_chain();
        let err = build_aave_rewards_claim_action(AaveRewardsClaimRequest {
            chain,
            sender: SENDER.into(),
            recipient: SENDER.into(),
            assets: vec![],
            reward_token: "0x00000000000000000000000000000000000000D2".into(),
            amount_base_units: "1000".into(),
            simulate: true,
            rpc_url: rpc.uri(),
            controller_address: "0x00000000000000000000000000000000000000D3".into(),
            pool_addresses_provider: String::new(),
        })
        .await
        .expect_err("empty assets rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // =====================================================================
    // MORPHO LEND — Go morpho_test.go
    // =====================================================================

    // M1, M3 — Ported from Go: TestBuildMorphoLendActionSupply.
    #[tokio::test]
    async fn morpho_lend_supply_emits_approval_then_lend() {
        let rpc = allowance_rpc(0).await;
        let graphql = morpho_market_graphql().await;
        let chain = eth_chain();
        let asset = usdc(&chain);
        let action = build_morpho_lend_action(MorphoLendRequest {
            verb: AaveLendVerb::Supply,
            chain,
            asset,
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: RECIPIENT.into(),
            on_behalf_of: String::new(),
            simulate: true,
            rpc_url: rpc.uri(),
            market_id: MORPHO_MARKET_ID.into(),
            graphql_endpoint: graphql.uri(),
        })
        .await
        .expect("build morpho supply");

        assert_eq!(action.intent_type, "lend_supply");
        assert_eq!(action.provider, "morpho");
        assert_eq!(
            step_types(&action),
            vec![StepType::Approval, StepType::Lend]
        );
        assert_eq!(action.steps[1].target, MORPHO_BLUE);
    }

    // M2 — Ported from Go: TestBuildMorphoLendActionRequiresMarketID (validated
    // before RPC/GraphQL, so no mock servers are needed).
    #[tokio::test]
    async fn morpho_lend_requires_market_id() {
        let chain = eth_chain();
        let asset = usdc(&chain);
        let err = build_morpho_lend_action(MorphoLendRequest {
            verb: AaveLendVerb::Supply,
            chain,
            asset,
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: String::new(),
            on_behalf_of: String::new(),
            simulate: false,
            rpc_url: String::new(),
            market_id: String::new(),
            graphql_endpoint: String::new(),
        })
        .await
        .expect_err("missing market id rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // M2 — fresh spec-driven: a malformed (non-32-byte) market id is usage.
    #[tokio::test]
    async fn morpho_lend_rejects_short_market_id() {
        let chain = eth_chain();
        let asset = usdc(&chain);
        let err = build_morpho_lend_action(MorphoLendRequest {
            verb: AaveLendVerb::Supply,
            chain,
            asset,
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: String::new(),
            on_behalf_of: String::new(),
            simulate: false,
            rpc_url: String::new(),
            market_id: "0x1234".into(),
            graphql_endpoint: String::new(),
        })
        .await
        .expect_err("short market id rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // =====================================================================
    // MORPHO VAULT YIELD — Go morpho_vault_test.go
    // =====================================================================

    // V1, V3 — Ported from Go: TestBuildMorphoVaultYieldActionDeposit.
    #[tokio::test]
    async fn morpho_vault_deposit_emits_approval_then_lend() {
        let rpc = allowance_rpc(0).await;
        let graphql = morpho_vault_graphql().await;
        let chain = eth_chain();
        let asset = usdc(&chain);
        let action = build_morpho_vault_yield_action(MorphoVaultYieldRequest {
            verb: MorphoVaultYieldVerb::Deposit,
            chain,
            asset,
            vault_address: VAULT_ADDR.into(),
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: RECIPIENT.into(),
            on_behalf_of: String::new(),
            simulate: true,
            rpc_url: rpc.uri(),
            graphql_endpoint: graphql.uri(),
        })
        .await
        .expect("build vault deposit");

        assert_eq!(action.intent_type, "yield_deposit");
        assert_eq!(action.provider, "morpho");
        assert_eq!(
            step_types(&action),
            vec![StepType::Approval, StepType::Lend]
        );
        assert!(address::eq_fold(&action.steps[1].target, VAULT_ADDR));
        let meta = action.metadata.as_ref().expect("metadata present");
        assert_eq!(
            meta.get("vault_kind").and_then(|v| v.as_str()),
            Some("vault")
        );
    }

    // V4 — Ported from Go: TestBuildMorphoVaultYieldActionWithdraw.
    #[tokio::test]
    async fn morpho_vault_withdraw_is_single_lend_call() {
        let rpc = allowance_rpc(0).await;
        let graphql = morpho_vault_graphql().await;
        let chain = eth_chain();
        let asset = usdc(&chain);
        let action = build_morpho_vault_yield_action(MorphoVaultYieldRequest {
            verb: MorphoVaultYieldVerb::Withdraw,
            chain,
            asset,
            vault_address: VAULT_ADDR.into(),
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: RECIPIENT.into(),
            on_behalf_of: SENDER.into(),
            simulate: true,
            rpc_url: rpc.uri(),
            graphql_endpoint: graphql.uri(),
        })
        .await
        .expect("build vault withdraw");
        assert_eq!(action.intent_type, "yield_withdraw");
        assert_eq!(step_types(&action), vec![StepType::Lend]);
    }

    // V4 — Ported from Go: TestBuildMorphoVaultYieldActionRequiresVaultAddress.
    #[tokio::test]
    async fn morpho_vault_requires_vault_address() {
        let chain = eth_chain();
        let asset = usdc(&chain);
        let err = build_morpho_vault_yield_action(MorphoVaultYieldRequest {
            verb: MorphoVaultYieldVerb::Deposit,
            chain,
            asset,
            vault_address: String::new(),
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: String::new(),
            on_behalf_of: String::new(),
            simulate: false,
            rpc_url: String::new(),
            graphql_endpoint: String::new(),
        })
        .await
        .expect_err("missing vault address rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // =====================================================================
    // MOONWELL LEND — Go moonwell_test.go
    // =====================================================================

    /// A `wiremock` JSON-RPC endpoint that dispatches `eth_call` by selector and
    /// returns ABI-encoded `allowance` / `checkMembership` results (mirrors Go
    /// `newMoonwellPlannerRPCServer`).
    async fn moonwell_rpc(allowance: u128, is_member: bool) -> MockServer {
        let server = MockServer::start().await;
        let allowance_sel = hex::encode(
            Function::from_abi_json(ERC20_MINIMAL_ABI, "allowance")
                .expect("allowance fn")
                .selector(),
        );
        let membership_sel = hex::encode(
            Function::from_abi_json(defi_registry::MOONWELL_COMPTROLLER_ABI, "checkMembership")
                .expect("checkMembership fn")
                .selector(),
        );
        let allowance_word = format!("0x{}", hex::encode(encode_uint_word(allowance)));
        let bool_word = {
            let mut w = vec![0u8; 32];
            if is_member {
                w[31] = 1;
            }
            format!("0x{}", hex::encode(w))
        };
        // wiremock can't branch on body easily; dispatch by matching the JSON-RPC
        // `data` selector and answer with a `Respond`er that echoes the request id
        // (alloy matches responses by id) plus the selector's ABI-encoded result.
        Mock::given(method("POST"))
            .and(SelectorMatcher {
                selector: allowance_sel.clone(),
            })
            .respond_with(SelectorResponder {
                selector: allowance_sel,
                result: allowance_word,
            })
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(SelectorMatcher {
                selector: membership_sel.clone(),
            })
            .respond_with(SelectorResponder {
                selector: membership_sel,
                result: bool_word,
            })
            .mount(&server)
            .await;
        server
    }

    /// Extract the lowercased 4-byte (8-hex-char) selector from a JSON-RPC
    /// `eth_call` request body, if present.
    fn request_selector(request: &wiremock::Request) -> Option<String> {
        let body: serde_json::Value = serde_json::from_slice(&request.body).ok()?;
        let data = body["params"][0]["data"]
            .as_str()
            .or_else(|| body["params"][0]["input"].as_str())?;
        let data = data.trim_start_matches("0x");
        if data.len() < 8 {
            return None;
        }
        Some(data[..8].to_ascii_lowercase())
    }

    /// Custom `wiremock` matcher: matches a JSON-RPC `eth_call` whose calldata
    /// begins with `selector`.
    struct SelectorMatcher {
        selector: String,
    }

    impl wiremock::Match for SelectorMatcher {
        fn matches(&self, request: &wiremock::Request) -> bool {
            request_selector(request)
                .map(|sel| sel.eq_ignore_ascii_case(&self.selector))
                .unwrap_or(false)
        }
    }

    /// Custom `wiremock` responder: returns a JSON-RPC success envelope echoing
    /// the request `id` (alloy correlates responses by id) and carrying the
    /// selector's ABI-encoded `result` word.
    struct SelectorResponder {
        #[allow(dead_code)]
        selector: String,
        result: String,
    }

    impl wiremock::Respond for SelectorResponder {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            let id = serde_json::from_slice::<serde_json::Value>(&request.body)
                .ok()
                .and_then(|body| body.get("id").cloned())
                .unwrap_or_else(|| serde_json::Value::from(1));
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": self.result,
            }))
        }
    }

    // W1, W2 — Ported from Go: TestBuildMoonwellSupplyWithExplicitMToken.
    #[tokio::test]
    async fn moonwell_supply_emits_approval_enter_supply() {
        let rpc = moonwell_rpc(0, false).await;
        let chain = base_chain();
        let asset = usdc(&chain);
        let action = build_moonwell_lend_action(MoonwellLendRequest {
            verb: AaveLendVerb::Supply,
            chain,
            asset,
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: SENDER.into(),
            simulate: true,
            rpc_url: rpc.uri(),
            mtoken_address: M_TOKEN.into(),
        })
        .await
        .expect("build moonwell supply");

        assert_eq!(action.intent_type, "lend_supply");
        assert_eq!(action.provider, "moonwell");
        assert_eq!(action.steps.len(), 3);
        assert_eq!(action.steps[0].step_type, StepType::Approval);
        assert_eq!(action.steps[1].step_id, "moonwell-enter-market");
        assert_eq!(action.steps[2].step_id, "moonwell-supply");
        assert_eq!(action.steps[2].step_type, StepType::Lend);
        assert!(address::eq_fold(&action.steps[2].target, M_TOKEN));
    }

    // W3 — Ported from Go: TestBuildMoonwellSupplySkipsApprovalWhenSufficient.
    #[tokio::test]
    async fn moonwell_supply_skips_approval_and_enter_when_ready() {
        let rpc = moonwell_rpc(10_000_000, true).await;
        let chain = base_chain();
        let asset = usdc(&chain);
        let action = build_moonwell_lend_action(MoonwellLendRequest {
            verb: AaveLendVerb::Supply,
            chain,
            asset,
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: String::new(),
            simulate: true,
            rpc_url: rpc.uri(),
            mtoken_address: M_TOKEN.into(),
        })
        .await
        .expect("build moonwell supply");
        assert_eq!(step_ids(&action), vec!["moonwell-supply".to_string()]);
    }

    // W4 — Ported from Go: TestBuildMoonwellWithdraw.
    #[tokio::test]
    async fn moonwell_withdraw_is_single_step() {
        let rpc = moonwell_rpc(0, false).await;
        let chain = base_chain();
        let asset = usdc(&chain);
        let action = build_moonwell_lend_action(MoonwellLendRequest {
            verb: AaveLendVerb::Withdraw,
            chain,
            asset,
            amount_base_units: "500000".into(),
            sender: SENDER.into(),
            recipient: String::new(),
            simulate: true,
            rpc_url: rpc.uri(),
            mtoken_address: M_TOKEN.into(),
        })
        .await
        .expect("build moonwell withdraw");
        assert_eq!(action.intent_type, "lend_withdraw");
        assert_eq!(step_ids(&action), vec!["moonwell-withdraw".to_string()]);
        assert!(address::eq_fold(&action.steps[0].target, M_TOKEN));
    }

    // W4 — Ported from Go: TestBuildMoonwellRepay.
    #[tokio::test]
    async fn moonwell_repay_emits_approval_then_repay() {
        let rpc = moonwell_rpc(0, false).await;
        let chain = base_chain();
        let asset = usdc(&chain);
        let action = build_moonwell_lend_action(MoonwellLendRequest {
            verb: AaveLendVerb::Repay,
            chain,
            asset,
            amount_base_units: "750000".into(),
            sender: SENDER.into(),
            recipient: SENDER.into(),
            simulate: true,
            rpc_url: rpc.uri(),
            mtoken_address: M_TOKEN.into(),
        })
        .await
        .expect("build moonwell repay");
        assert_eq!(action.intent_type, "lend_repay");
        assert_eq!(action.steps.len(), 2);
        assert_eq!(action.steps[0].step_type, StepType::Approval);
        assert_eq!(action.steps[1].step_id, "moonwell-repay");
    }

    // W5 — Ported from Go: TestBuildMoonwellLendRejectsAlternateRecipient.
    #[tokio::test]
    async fn moonwell_rejects_alternate_recipient() {
        let chain = base_chain();
        let asset = usdc(&chain);
        let err = build_moonwell_lend_action(MoonwellLendRequest {
            verb: AaveLendVerb::Supply,
            chain,
            asset,
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: "0x00000000000000000000000000000000000000BB".into(),
            simulate: true,
            rpc_url: String::new(),
            mtoken_address: M_TOKEN.into(),
        })
        .await
        .expect_err("alternate recipient rejected");
        assert_eq!(err.code, defi_errors::Code::Unsupported);
        assert!(
            err.to_string().contains("alternate recipients"),
            "unexpected error: {err}"
        );
    }

    // W6 — Ported from Go: TestBuildMoonwellRequiresSender.
    #[tokio::test]
    async fn moonwell_requires_sender() {
        let chain = base_chain();
        let asset = usdc(&chain);
        let err = build_moonwell_lend_action(MoonwellLendRequest {
            verb: AaveLendVerb::Supply,
            chain,
            asset,
            amount_base_units: "1000000".into(),
            sender: String::new(),
            recipient: String::new(),
            simulate: false,
            rpc_url: String::new(),
            mtoken_address: M_TOKEN.into(),
        })
        .await
        .expect_err("missing sender rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // W6 — Ported from Go: TestBuildMoonwellRejectsUnsupportedVerb.
    #[tokio::test]
    async fn moonwell_rejects_unsupported_verb() {
        let rpc = moonwell_rpc(0, false).await;
        let chain = base_chain();
        let asset = usdc(&chain);
        let err = build_moonwell_lend_action(MoonwellLendRequest {
            verb: AaveLendVerb::Unsupported("invalid".into()),
            chain,
            asset,
            amount_base_units: "1000000".into(),
            sender: SENDER.into(),
            recipient: String::new(),
            simulate: false,
            rpc_url: rpc.uri(),
            mtoken_address: M_TOKEN.into(),
        })
        .await
        .expect_err("unsupported verb rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // W7 — Ported from Go: TestResolveMoonwellMTokenExplicit.
    #[tokio::test]
    async fn resolve_moonwell_mtoken_explicit() {
        let chain = base_chain();
        let addr = resolve_moonwell_mtoken(None, &chain, M_TOKEN, &Address::ZERO)
            .await
            .expect("explicit mtoken");
        assert!(address::eq_fold(&addr.to_hex(), M_TOKEN));
    }

    // W7 — Ported from Go: TestResolveMoonwellMTokenInvalidExplicit.
    #[tokio::test]
    async fn resolve_moonwell_mtoken_invalid_explicit() {
        let chain = base_chain();
        let err = resolve_moonwell_mtoken(None, &chain, "not-hex", &Address::ZERO)
            .await
            .expect_err("invalid explicit mtoken rejected");
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // W7 — Ported from Go: TestResolveMoonwellMTokenUnsupportedChain.
    #[tokio::test]
    async fn resolve_moonwell_mtoken_unsupported_chain() {
        let chain = parse_chain("999").expect("parse chain 999");
        let err = resolve_moonwell_mtoken(None, &chain, "", &Address::ZERO)
            .await
            .expect_err("unsupported chain rejected");
        assert_eq!(err.code, defi_errors::Code::Unsupported);
        assert!(
            err.to_string().contains("not supported"),
            "unexpected error: {err}"
        );
    }
}
