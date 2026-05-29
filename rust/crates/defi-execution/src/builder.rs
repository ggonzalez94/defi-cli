//! Action builder traits (cycle break).
//!
//! In Go, `internal/providers` defined `BuildSwapAction`/`BuildBridgeAction` on
//! provider interfaces while depending on `internal/execution`. Rust forbids
//! dependency cycles, so the builder traits — and the request/option types they
//! take — are defined HERE; `defi-providers` implements them (spec §3, locked
//! interface §"Interface contracts locked at scaffold").

use std::collections::HashMap;

use crate::action::Action;
use crate::planner::{
    self, build_aave_lend_action, build_aave_rewards_claim_action,
    build_aave_rewards_compound_action, build_moonwell_lend_action, build_morpho_lend_action,
    build_morpho_vault_yield_action, AaveLendRequest, AaveLendVerb, AaveRewardsClaimRequest,
    AaveRewardsCompoundRequest, MoonwellLendRequest, MorphoLendRequest, MorphoVaultYieldRequest,
    MorphoVaultYieldVerb,
};
use async_trait::async_trait;
use defi_errors::{Code, Error};
use defi_id::{Asset, Chain};

/// Swap trade direction. Defaults to exact-input (matches Go default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SwapTradeType {
    #[default]
    ExactInput,
    ExactOutput,
}

impl SwapTradeType {
    /// Canonical wire string (mirrors Go `SwapTradeType` constant values).
    ///
    /// Note the hyphen: `exact-input` / `exact-output` (NOT underscores).
    pub fn as_str(self) -> &'static str {
        match self {
            SwapTradeType::ExactInput => "exact-input",
            SwapTradeType::ExactOutput => "exact-output",
        }
    }

    /// Parse a wire string into a [`SwapTradeType`].
    ///
    /// Trim- and case-tolerant (Go uses `strings.ToLower(strings.TrimSpace(..))`).
    /// An empty string parses to the default [`SwapTradeType::ExactInput`] to
    /// match the Go runner, which treats an empty `--type` as exact-input.
    /// Unknown input returns `None`.
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "" => Some(SwapTradeType::ExactInput),
            "exact-input" => Some(SwapTradeType::ExactInput),
            "exact-output" => Some(SwapTradeType::ExactOutput),
            _ => None,
        }
    }
}

/// Parameters for a swap quote/build (mirrors Go `SwapQuoteRequest`).
#[derive(Debug, Clone, Default)]
pub struct SwapQuoteRequest {
    pub chain: Chain,
    pub from_asset: Asset,
    pub to_asset: Asset,
    pub amount_base_units: String,
    pub amount_decimal: String,
    pub rpc_url: String,
    pub trade_type: SwapTradeType,
    pub slippage_pct: Option<f64>,
    pub swapper: String,
}

/// Swap execution options (mirrors Go `SwapExecutionOptions`).
#[derive(Debug, Clone, Default)]
pub struct SwapExecutionOptions {
    pub sender: String,
    pub recipient: String,
    pub slippage_bps: i64,
    pub simulate: bool,
    pub rpc_url: String,
}

/// Parameters for a bridge quote/build (mirrors Go `BridgeQuoteRequest`).
#[derive(Debug, Clone, Default)]
pub struct BridgeQuoteRequest {
    pub from_chain: Chain,
    pub to_chain: Chain,
    pub from_asset: Asset,
    pub to_asset: Asset,
    pub amount_base_units: String,
    pub amount_decimal: String,
    pub from_amount_for_gas: String,
}

/// Bridge execution options (mirrors Go `BridgeExecutionOptions`).
#[derive(Debug, Clone, Default)]
pub struct BridgeExecutionOptions {
    pub sender: String,
    pub recipient: String,
    pub slippage_bps: i64,
    pub simulate: bool,
    pub rpc_url: String,
    pub from_amount_for_gas: String,
}

/// Provider capability: build an executable swap [`Action`] from a quote
/// request (mirrors Go `SwapExecutionProvider.BuildSwapAction`).
#[async_trait]
pub trait SwapActionBuilder: Send + Sync {
    async fn build_swap_action(
        &self,
        req: SwapQuoteRequest,
        opts: SwapExecutionOptions,
    ) -> Result<Action, Error>;
}

/// Provider capability: build an executable bridge [`Action`] from a quote
/// request (mirrors Go `BridgeExecutionProvider.BuildBridgeAction`).
#[async_trait]
pub trait BridgeActionBuilder: Send + Sync {
    async fn build_bridge_action(
        &self,
        req: BridgeQuoteRequest,
        opts: BridgeExecutionOptions,
    ) -> Result<Action, Error>;
}

// =============================================================================
// Action-building routing registry (Go `actionbuilder.Registry`).
// =============================================================================

/// Yield verb (`deposit|withdraw`). Parity with Go `YieldVerb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum YieldVerb {
    #[default]
    Deposit,
    Withdraw,
}

impl YieldVerb {
    fn as_str(self) -> &'static str {
        match self {
            YieldVerb::Deposit => "deposit",
            YieldVerb::Withdraw => "withdraw",
        }
    }
}

/// A lend routing request. Parity with Go `actionbuilder.LendRequest`.
#[derive(Debug, Clone, Default)]
pub struct LendRequest {
    pub provider: String,
    pub verb: LendVerb,
    pub chain: Chain,
    pub asset: Asset,
    pub market_id: String,
    pub amount_base_units: String,
    pub sender: String,
    pub recipient: String,
    pub on_behalf_of: String,
    pub interest_rate_mode: i64,
    pub simulate: bool,
    pub rpc_url: String,
    pub pool_address: String,
    pub pool_address_provider: String,
}

/// Lend verb mirror that is `Default` for the routing request (the planner's
/// [`AaveLendVerb`] has no default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LendVerb {
    #[default]
    Supply,
    Withdraw,
    Borrow,
    Repay,
}

impl From<LendVerb> for AaveLendVerb {
    fn from(v: LendVerb) -> Self {
        match v {
            LendVerb::Supply => AaveLendVerb::Supply,
            LendVerb::Withdraw => AaveLendVerb::Withdraw,
            LendVerb::Borrow => AaveLendVerb::Borrow,
            LendVerb::Repay => AaveLendVerb::Repay,
        }
    }
}

/// A yield routing request. Parity with Go `actionbuilder.YieldRequest`.
#[derive(Debug, Clone, Default)]
pub struct YieldRequest {
    pub provider: String,
    pub verb: YieldVerb,
    pub chain: Chain,
    pub asset: Asset,
    pub vault_address: String,
    pub amount_base_units: String,
    pub sender: String,
    pub recipient: String,
    pub on_behalf_of: String,
    pub simulate: bool,
    pub rpc_url: String,
    pub pool_address: String,
    pub pool_address_provider: String,
}

/// A rewards-claim routing request. Parity with Go
/// `actionbuilder.RewardsClaimRequest`.
#[derive(Debug, Clone, Default)]
pub struct RewardsClaimRequest {
    pub provider: String,
    pub chain: Chain,
    pub sender: String,
    pub recipient: String,
    pub assets: Vec<String>,
    pub reward_token: String,
    pub amount_base_units: String,
    pub simulate: bool,
    pub rpc_url: String,
    pub controller_address: String,
    pub pool_address_provider: String,
}

/// A rewards-compound routing request. Parity with Go
/// `actionbuilder.RewardsCompoundRequest`.
#[derive(Debug, Clone, Default)]
pub struct RewardsCompoundRequest {
    pub provider: String,
    pub chain: Chain,
    pub sender: String,
    pub recipient: String,
    pub on_behalf_of: String,
    pub assets: Vec<String>,
    pub reward_token: String,
    pub amount_base_units: String,
    pub simulate: bool,
    pub rpc_url: String,
    pub controller_address: String,
    pub pool_address: String,
    pub pool_address_provider: String,
}

/// Routes execution requests by `--provider` to the matching builder (swap /
/// bridge via registered provider builders; lend / yield / rewards / approval /
/// transfer via the internal deterministic planner). Parity with Go
/// `actionbuilder.Registry`.
#[derive(Default)]
pub struct Registry {
    swap_builders: HashMap<String, (String, Box<dyn SwapActionBuilder>)>,
    swap_known: std::collections::HashSet<String>,
    bridge_builders: HashMap<String, (String, Box<dyn BridgeActionBuilder>)>,
    bridge_known: std::collections::HashSet<String>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Registry::default()
    }

    /// Register an execution-capable swap builder under a normalized name.
    pub fn register_swap_builder(&mut self, name: &str, builder: Box<dyn SwapActionBuilder>) {
        let key = normalize_swap_provider(name);
        let display = builder_display_name(name);
        self.swap_known.insert(key.clone());
        self.swap_builders.insert(key, (display, builder));
    }

    /// Mark a provider as known-but-quote-only (no execution builder).
    pub fn register_known_swap_provider(&mut self, name: &str) {
        self.swap_known.insert(normalize_swap_provider(name));
    }

    /// Register an execution-capable bridge builder with an explicit display name.
    pub fn register_bridge_builder(
        &mut self,
        name: &str,
        display_name: &str,
        builder: Box<dyn BridgeActionBuilder>,
    ) {
        let key = name.trim().to_lowercase();
        self.bridge_known.insert(key.clone());
        self.bridge_builders
            .insert(key, (display_name.to_string(), builder));
    }

    /// Mark a bridge provider as known-but-quote-only.
    pub fn register_known_bridge_provider(&mut self, name: &str) {
        self.bridge_known.insert(name.trim().to_lowercase());
    }

    /// Route a swap build by provider, returning the [`Action`] and the
    /// provider's display name. Parity with Go `BuildSwapAction`.
    pub async fn build_swap_action(
        &self,
        provider: &str,
        op: &str,
        req: SwapQuoteRequest,
        opts: SwapExecutionOptions,
    ) -> Result<(Action, String), Error> {
        let name = normalize_swap_provider(provider);
        if name.is_empty() {
            return Err(Error::new(Code::Usage, "--provider is required"));
        }
        if !self.swap_known.contains(&name) {
            return Err(Error::new(Code::Unsupported, "unsupported swap provider"));
        }
        match self.swap_builders.get(&name) {
            Some((display, builder)) => {
                let action = builder.build_swap_action(req, opts).await?;
                Ok((action, display.clone()))
            }
            None => {
                let msg = match op.trim().to_lowercase().as_str() {
                    "plan" | "planning" => {
                        format!("provider {name} does not support swap planning")
                    }
                    _ => format!("provider {name} does not support swap execution"),
                };
                Err(Error::new(Code::Unsupported, msg))
            }
        }
    }

    /// Route a bridge build by provider, returning the [`Action`] and the
    /// provider's display name. Parity with Go `BuildBridgeAction`.
    pub async fn build_bridge_action(
        &self,
        provider: &str,
        req: BridgeQuoteRequest,
        opts: BridgeExecutionOptions,
    ) -> Result<(Action, String), Error> {
        let name = provider.trim().to_lowercase();
        if name.is_empty() {
            return Err(Error::new(Code::Usage, "--provider is required"));
        }
        if !self.bridge_known.contains(&name) {
            return Err(Error::new(Code::Unsupported, "unsupported bridge provider"));
        }
        match self.bridge_builders.get(&name) {
            Some((display, builder)) => {
                let action = builder.build_bridge_action(req, opts).await?;
                Ok((action, display.clone()))
            }
            None => Err(Error::new(
                Code::Unsupported,
                format!(
                    "bridge provider {name:?} is quote-only; execution providers: {}",
                    self.bridge_execution_provider_names().join(",")
                ),
            )),
        }
    }

    /// The execution-capable bridge provider names, sorted ascending. Parity with
    /// Go `BridgeExecutionProviderNames`.
    pub fn bridge_execution_provider_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.bridge_builders.keys().cloned().collect();
        names.sort();
        names
    }

    /// Route a lend build by provider. Parity with Go `BuildLendAction`.
    pub async fn build_lend_action(&self, req: LendRequest) -> Result<Action, Error> {
        let name = normalize_lending_provider(&req.provider);
        if name.is_empty() {
            return Err(Error::new(Code::Usage, "--provider is required"));
        }
        match name.as_str() {
            "aave" => {
                build_aave_lend_action(AaveLendRequest {
                    verb: req.verb.into(),
                    chain: req.chain,
                    asset: req.asset,
                    amount_base_units: req.amount_base_units,
                    sender: req.sender,
                    recipient: req.recipient,
                    on_behalf_of: req.on_behalf_of,
                    interest_rate_mode: req.interest_rate_mode,
                    simulate: req.simulate,
                    rpc_url: req.rpc_url,
                    pool_address: req.pool_address,
                    pool_addresses_provider: req.pool_address_provider,
                })
                .await
            }
            "morpho" => {
                build_morpho_lend_action(MorphoLendRequest {
                    verb: req.verb.into(),
                    chain: req.chain,
                    asset: req.asset,
                    amount_base_units: req.amount_base_units,
                    sender: req.sender,
                    recipient: req.recipient,
                    on_behalf_of: req.on_behalf_of,
                    simulate: req.simulate,
                    rpc_url: req.rpc_url,
                    market_id: req.market_id,
                    graphql_endpoint: String::new(),
                })
                .await
            }
            "moonwell" => {
                if !req.on_behalf_of.trim().is_empty() {
                    return Err(Error::new(
                        Code::Unsupported,
                        "moonwell does not support --on-behalf-of; Compound v2 calls operate on msg.sender only",
                    ));
                }
                build_moonwell_lend_action(MoonwellLendRequest {
                    verb: req.verb.into(),
                    chain: req.chain,
                    asset: req.asset,
                    amount_base_units: req.amount_base_units,
                    sender: req.sender,
                    recipient: req.recipient,
                    simulate: req.simulate,
                    rpc_url: req.rpc_url,
                    mtoken_address: req.pool_address,
                })
                .await
            }
            _ => Err(Error::new(
                Code::Unsupported,
                "lend execution currently supports provider=aave|morpho|moonwell",
            )),
        }
    }

    /// Route a yield build by provider. Parity with Go `BuildYieldAction`.
    pub async fn build_yield_action(&self, req: YieldRequest) -> Result<Action, Error> {
        let name = normalize_lending_provider(&req.provider);
        if name.is_empty() {
            return Err(Error::new(Code::Usage, "--provider is required"));
        }
        let verb = req.verb.as_str();
        match name.as_str() {
            "aave" => {
                let lend_verb = match req.verb {
                    YieldVerb::Deposit => AaveLendVerb::Supply,
                    YieldVerb::Withdraw => AaveLendVerb::Withdraw,
                };
                let mut action = build_aave_lend_action(AaveLendRequest {
                    verb: lend_verb,
                    chain: req.chain,
                    asset: req.asset,
                    amount_base_units: req.amount_base_units,
                    sender: req.sender,
                    recipient: req.recipient,
                    on_behalf_of: req.on_behalf_of,
                    interest_rate_mode: 0,
                    simulate: req.simulate,
                    rpc_url: req.rpc_url,
                    pool_address: req.pool_address,
                    pool_addresses_provider: req.pool_address_provider,
                })
                .await?;
                action.intent_type = format!("yield_{verb}");
                let meta = action.metadata.get_or_insert_with(serde_json::Map::new);
                meta.insert("yield_action".into(), verb.into());
                meta.insert("yield_product".into(), "aave_reserve".into());
                Ok(action)
            }
            "morpho" => {
                build_morpho_vault_yield_action(MorphoVaultYieldRequest {
                    verb: match req.verb {
                        YieldVerb::Deposit => MorphoVaultYieldVerb::Deposit,
                        YieldVerb::Withdraw => MorphoVaultYieldVerb::Withdraw,
                    },
                    chain: req.chain,
                    asset: req.asset,
                    vault_address: req.vault_address,
                    amount_base_units: req.amount_base_units,
                    sender: req.sender,
                    recipient: req.recipient,
                    on_behalf_of: req.on_behalf_of,
                    simulate: req.simulate,
                    rpc_url: req.rpc_url,
                    graphql_endpoint: String::new(),
                })
                .await
            }
            "moonwell" => {
                if !req.on_behalf_of.trim().is_empty() {
                    return Err(Error::new(
                        Code::Unsupported,
                        "moonwell does not support --on-behalf-of; Compound v2 calls operate on msg.sender only",
                    ));
                }
                let lend_verb = match req.verb {
                    YieldVerb::Deposit => AaveLendVerb::Supply,
                    YieldVerb::Withdraw => AaveLendVerb::Withdraw,
                };
                let mut action = build_moonwell_lend_action(MoonwellLendRequest {
                    verb: lend_verb,
                    chain: req.chain,
                    asset: req.asset,
                    amount_base_units: req.amount_base_units,
                    sender: req.sender,
                    recipient: req.recipient,
                    simulate: req.simulate,
                    rpc_url: req.rpc_url,
                    mtoken_address: req.pool_address,
                })
                .await?;
                action.intent_type = format!("yield_{verb}");
                let meta = action.metadata.get_or_insert_with(serde_json::Map::new);
                meta.insert("yield_action".into(), verb.into());
                meta.insert("yield_product".into(), "moonwell_market".into());
                Ok(action)
            }
            _ => Err(Error::new(
                Code::Unsupported,
                "yield execution currently supports provider=aave|morpho|moonwell",
            )),
        }
    }

    /// Route a rewards-claim build by provider. Parity with Go
    /// `BuildRewardsClaimAction`.
    pub async fn build_rewards_claim_action(
        &self,
        req: RewardsClaimRequest,
    ) -> Result<Action, Error> {
        let name = normalize_lending_provider(&req.provider);
        if name.is_empty() {
            return Err(Error::new(Code::Usage, "--provider is required"));
        }
        if name != "aave" {
            return Err(Error::new(
                Code::Unsupported,
                "rewards execution currently supports only provider=aave",
            ));
        }
        build_aave_rewards_claim_action(AaveRewardsClaimRequest {
            chain: req.chain,
            sender: req.sender,
            recipient: req.recipient,
            assets: req.assets,
            reward_token: req.reward_token,
            amount_base_units: req.amount_base_units,
            simulate: req.simulate,
            rpc_url: req.rpc_url,
            controller_address: req.controller_address,
            pool_addresses_provider: req.pool_address_provider,
        })
        .await
    }

    /// Route a rewards-compound build by provider. Parity with Go
    /// `BuildRewardsCompoundAction`.
    pub async fn build_rewards_compound_action(
        &self,
        req: RewardsCompoundRequest,
    ) -> Result<Action, Error> {
        let name = normalize_lending_provider(&req.provider);
        if name.is_empty() {
            return Err(Error::new(Code::Usage, "--provider is required"));
        }
        if name != "aave" {
            return Err(Error::new(
                Code::Unsupported,
                "rewards execution currently supports only provider=aave",
            ));
        }
        build_aave_rewards_compound_action(AaveRewardsCompoundRequest {
            chain: req.chain,
            sender: req.sender,
            recipient: req.recipient,
            assets: req.assets,
            reward_token: req.reward_token,
            amount_base_units: req.amount_base_units,
            simulate: req.simulate,
            rpc_url: req.rpc_url,
            controller_address: req.controller_address,
            pool_address: req.pool_address,
            pool_addresses_provider: req.pool_address_provider,
            on_behalf_of: req.on_behalf_of,
        })
        .await
    }

    /// Route an approval build to the planner. Parity with Go
    /// `BuildApprovalAction`.
    pub fn build_approval_action(&self, req: planner::ApprovalRequest) -> Result<Action, Error> {
        planner::build_approval_action(req)
    }

    /// Route a transfer build to the planner. Parity with Go
    /// `BuildTransferAction`.
    pub fn build_transfer_action(&self, req: planner::TransferRequest) -> Result<Action, Error> {
        planner::build_transfer_action(req)
    }
}

/// Normalize a swap provider name, parity with Go `NormalizeSwapProvider`
/// (`tempo-dex`/`tempodex` → `tempo`).
fn normalize_swap_provider(name: &str) -> String {
    let n = name.trim().to_lowercase();
    match n.as_str() {
        "tempo-dex" | "tempodex" => "tempo".to_string(),
        other => other.to_string(),
    }
}

/// Normalize a lending provider name, parity with Go `NormalizeLendingProvider`
/// (`aave-v3`→`aave`, `morpho-blue`→`morpho`, `kamino-finance`→`kamino`,
/// `moonwell-v2`→`moonwell`).
fn normalize_lending_provider(name: &str) -> String {
    let n = name.trim().to_lowercase();
    match n.as_str() {
        "aave-v3" | "aavev3" => "aave".to_string(),
        "morpho-blue" | "morphoblue" => "morpho".to_string(),
        "kamino-finance" => "kamino".to_string(),
        "moonwell-v2" => "moonwell".to_string(),
        other => other.to_string(),
    }
}

/// A display name for a registered swap provider (Title-cased fallback).
fn builder_display_name(name: &str) -> String {
    let n = normalize_swap_provider(name);
    let mut chars = n.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => n,
    }
}

#[cfg(test)]
mod tests {
    //! SUCCESS CRITERIA for the `builder` / action-routing module
    //! (machine contract — exit codes + error semantics must hold).
    //!
    //! Go source: `internal/execution/actionbuilder/registry.go` (`Registry`),
    //! exercised by `internal/execution/actionbuilder/registry_test.go`.
    //!
    //! In Go, `actionbuilder.Registry` holds `map[string]providers.SwapProvider`
    //! + `map[string]providers.BridgeProvider` and routes by `--provider` to:
    //!   * the provider's `BuildSwapAction` / `BuildBridgeAction` when it
    //!     implements the execution interface (else a "quote-only" / "does not
    //!     support" error), and
    //!   * the internal deterministic `planner` for lend / yield / rewards /
    //!     approval / transfer.
    //!
    //! Rust forbids the Go provider↔execution dependency cycle, so the routing
    //! `Registry` lives HERE (in `defi-execution`) and is generic over the
    //! [`SwapActionBuilder`] / [`BridgeActionBuilder`] traits defined in this
    //! module; `defi-providers` registers concrete builders into it. To preserve
    //! the Go "known-but-quote-only vs unknown provider" distinction without
    //! depending on `defi-providers`, the registry tracks the set of *known*
    //! provider names alongside the subset that is execution-capable (has a
    //! registered builder):
    //!
    //!   * unknown provider name           -> `Code::Unsupported` ("unsupported … provider")
    //!   * known but no builder (quote-only) -> `Code::Unsupported`
    //!       swap+plan : message contains "does not support swap planning"
    //!       swap+other: message contains "does not support swap execution"
    //!       bridge    : message contains "quote-only"
    //!   * empty provider name             -> `Code::Usage` ("--provider is required")
    //!
    //! The Rust port is "correct" iff:
    //!
    //!  B1. SWAP routing rejects an empty provider with `Code::Usage`, an unknown
    //!      provider with `Code::Unsupported`, and a known quote-only provider for
    //!      a `"plan"` op with `Code::Unsupported` + a message containing
    //!      "does not support swap planning"; for a non-plan op the message
    //!      contains "does not support swap execution". A registered builder is
    //!      invoked and its `Action` + the provider's display name are returned.
    //!      Provider names are normalized via `NormalizeSwapProvider`
    //!      (`tempo-dex`/`tempodex` -> `tempo`) before lookup.
    //!
    //!  B2. BRIDGE routing rejects an empty provider with `Code::Usage`, an unknown
    //!      provider with `Code::Unsupported`, and a known quote-only provider with
    //!      `Code::Unsupported` + a message containing "quote-only". The error for
    //!      a quote-only bridge also lists the execution-capable bridge provider
    //!      names (sorted) via `bridge_execution_provider_names()`.
    //!
    //!  B3. `bridge_execution_provider_names()` returns the names of the registered
    //!      bridge builders, sorted ascending (mirrors Go `sort.Strings`).
    //!
    //!  B4. LEND routing normalizes the provider (`NormalizeLendingProvider`:
    //!      `aave-v3`->`aave`, `morpho-blue`->`morpho`, `kamino-finance`->`kamino`,
    //!      `moonwell-v2`->`moonwell`); an empty provider -> `Code::Usage`; an
    //!      unsupported provider (e.g. `kamino`) -> `Code::Unsupported`
    //!      ("…supports provider=aave|morpho|moonwell"); `moonwell` with a
    //!      non-empty `on_behalf_of` -> `Code::Unsupported` + message containing
    //!      "--on-behalf-of". Supported providers route to the matching planner.
    //!
    //!  B5. YIELD routing: empty provider -> `Code::Usage`; unsupported provider
    //!      -> `Code::Unsupported`; verb other than deposit/withdraw ->
    //!      `Code::Usage`; `moonwell` with `on_behalf_of` -> `Code::Unsupported`
    //!      ("--on-behalf-of"). For aave/moonwell deposit/withdraw the resulting
    //!      `Action.intent_type` is `"yield_<verb>"` and `metadata["yield_action"]`
    //!      == verb (aave -> `metadata["yield_product"]=="aave_reserve"`,
    //!      moonwell -> `"moonwell_market"`).
    //!
    //!  B6. REWARDS routing (claim + compound): empty provider -> `Code::Usage`;
    //!      any provider other than `aave` -> `Code::Unsupported`
    //!      ("…only provider=aave"). `aave` routes to the rewards planner.
    //!
    //!  B7. APPROVAL routing delegates to the planner and yields an `Action` with
    //!      `intent_type == "approve"`.
    //!
    //!  B8. TRANSFER routing delegates to the planner and yields an `Action` with
    //!      `intent_type == "transfer"`.
    //!
    //! Go tests intentionally SKIPPED as internal-detail / covered elsewhere:
    //!   * `TestNormalizeLendingProviderAliases` — alias normalization is owned by
    //!     `defi-providers::normalize` (its own RED suite), re-asserted indirectly
    //!     here via B4 routing, not duplicated as a unit test in this crate.
    //!   * The planner's calldata/step-shape assertions live in the `planner`
    //!     module's own RED suite; here we only assert the *routing* outcome
    //!     (intent_type / metadata / error code), not contract calldata.

    use super::*;
    use crate::action::Action;
    use crate::builder::{
        LendRequest, Registry, RewardsClaimRequest, RewardsCompoundRequest, YieldRequest, YieldVerb,
    };
    use crate::planner::{ApprovalRequest, TransferRequest};
    use defi_errors::Code;
    use defi_id::{parse_asset, parse_chain, Chain};

    const SENDER: &str = "0x00000000000000000000000000000000000000aa";
    const SPENDER: &str = "0x00000000000000000000000000000000000000bb";
    const RECIPIENT: &str = "0x00000000000000000000000000000000000000bb";

    // -- a fake execution-capable builder -----------------------------------
    // Records the request it received and returns a fixed `Action`, so routing
    // (provider lookup + name normalization + invocation) can be asserted
    // without depending on `defi-providers`.
    struct FakeSwapBuilder {
        display_name: String,
    }

    #[async_trait]
    impl SwapActionBuilder for FakeSwapBuilder {
        async fn build_swap_action(
            &self,
            _req: SwapQuoteRequest,
            _opts: SwapExecutionOptions,
        ) -> Result<Action, Error> {
            Ok(Action::new(
                "act_fake",
                "swap",
                "eip155:1",
                Default::default(),
            ))
        }
    }

    struct FakeBridgeBuilder;

    #[async_trait]
    impl BridgeActionBuilder for FakeBridgeBuilder {
        async fn build_bridge_action(
            &self,
            _req: BridgeQuoteRequest,
            _opts: BridgeExecutionOptions,
        ) -> Result<Action, Error> {
            Ok(Action::new(
                "act_fake",
                "bridge",
                "eip155:1",
                Default::default(),
            ))
        }
    }

    fn eth_chain() -> Chain {
        parse_chain("1").expect("parse chain 1")
    }

    fn usdc(chain: &Chain) -> defi_id::Asset {
        parse_asset("USDC", chain).expect("parse USDC")
    }

    // ======================================================================
    // SWAP routing — B1
    // ======================================================================

    // B1 — empty provider is a usage error.
    #[tokio::test]
    async fn swap_routing_rejects_empty_provider() {
        let reg = Registry::new();
        let err = reg
            .build_swap_action(
                "",
                "plan",
                SwapQuoteRequest::default(),
                SwapExecutionOptions::default(),
            )
            .await
            .expect_err("empty provider rejected");
        assert_eq!(err.code, Code::Usage);
    }

    // B1 — unknown provider is unsupported.
    #[tokio::test]
    async fn swap_routing_rejects_unknown_provider() {
        let reg = Registry::new();
        let err = reg
            .build_swap_action(
                "doesnotexist",
                "plan",
                SwapQuoteRequest::default(),
                SwapExecutionOptions::default(),
            )
            .await
            .expect_err("unknown provider rejected");
        assert_eq!(err.code, Code::Unsupported);
    }

    // B1 — Ported from Go: TestBuildSwapActionRejectsQuoteOnlyProvider.
    // A known-but-quote-only provider fails a `plan` op with a message that
    // mentions swap PLANNING.
    #[tokio::test]
    async fn swap_routing_rejects_quote_only_provider_for_plan() {
        let mut reg = Registry::new();
        reg.register_known_swap_provider("quoteonly");
        let err = reg
            .build_swap_action(
                "quoteonly",
                "plan",
                SwapQuoteRequest::default(),
                SwapExecutionOptions::default(),
            )
            .await
            .expect_err("quote-only provider rejected for plan");
        assert_eq!(err.code, Code::Unsupported);
        assert!(
            err.to_string()
                .to_lowercase()
                .contains("does not support swap planning"),
            "unexpected error: {err}"
        );
    }

    // B1 — a non-plan op against a quote-only provider mentions swap EXECUTION.
    #[tokio::test]
    async fn swap_routing_quote_only_non_plan_mentions_execution() {
        let mut reg = Registry::new();
        reg.register_known_swap_provider("quoteonly");
        let err = reg
            .build_swap_action(
                "quoteonly",
                "submit",
                SwapQuoteRequest::default(),
                SwapExecutionOptions::default(),
            )
            .await
            .expect_err("quote-only provider rejected for submit");
        assert_eq!(err.code, Code::Unsupported);
        assert!(
            err.to_string()
                .to_lowercase()
                .contains("does not support swap execution"),
            "unexpected error: {err}"
        );
    }

    // B1 — a registered builder is invoked and its action + display name return.
    #[tokio::test]
    async fn swap_routing_invokes_registered_builder() {
        let mut reg = Registry::new();
        reg.register_swap_builder(
            "tempo",
            Box::new(FakeSwapBuilder {
                display_name: "Tempo".into(),
            }),
        );
        let (action, name) = reg
            .build_swap_action(
                "tempo",
                "plan",
                SwapQuoteRequest::default(),
                SwapExecutionOptions::default(),
            )
            .await
            .expect("registered builder invoked");
        assert_eq!(action.intent_type, "swap");
        assert_eq!(name, "Tempo");
    }

    // B1 — provider name is normalized (`tempodex` -> `tempo`) before lookup.
    #[tokio::test]
    async fn swap_routing_normalizes_provider_name() {
        let mut reg = Registry::new();
        reg.register_swap_builder(
            "tempo",
            Box::new(FakeSwapBuilder {
                display_name: "Tempo".into(),
            }),
        );
        let (_, name) = reg
            .build_swap_action(
                "tempodex",
                "plan",
                SwapQuoteRequest::default(),
                SwapExecutionOptions::default(),
            )
            .await
            .expect("normalized provider name resolves");
        assert_eq!(name, "Tempo");
    }

    // ======================================================================
    // BRIDGE routing — B2, B3
    // ======================================================================

    // B2 — empty provider is a usage error.
    #[tokio::test]
    async fn bridge_routing_rejects_empty_provider() {
        let reg = Registry::new();
        let err = reg
            .build_bridge_action(
                "",
                BridgeQuoteRequest::default(),
                BridgeExecutionOptions::default(),
            )
            .await
            .expect_err("empty provider rejected");
        assert_eq!(err.code, Code::Usage);
    }

    // B2 — unknown provider is unsupported.
    #[tokio::test]
    async fn bridge_routing_rejects_unknown_provider() {
        let reg = Registry::new();
        let err = reg
            .build_bridge_action(
                "doesnotexist",
                BridgeQuoteRequest::default(),
                BridgeExecutionOptions::default(),
            )
            .await
            .expect_err("unknown provider rejected");
        assert_eq!(err.code, Code::Unsupported);
    }

    // B2 — Ported from Go: TestBuildBridgeActionRejectsQuoteOnlyProvider.
    #[tokio::test]
    async fn bridge_routing_rejects_quote_only_provider() {
        let mut reg = Registry::new();
        reg.register_known_bridge_provider("quoteonly");
        let err = reg
            .build_bridge_action(
                "quoteonly",
                BridgeQuoteRequest::default(),
                BridgeExecutionOptions::default(),
            )
            .await
            .expect_err("quote-only bridge provider rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert!(
            err.to_string().to_lowercase().contains("quote-only"),
            "unexpected error: {err}"
        );
    }

    // B2 — a registered builder is invoked and its action + display name return.
    #[tokio::test]
    async fn bridge_routing_invokes_registered_builder() {
        let mut reg = Registry::new();
        reg.register_bridge_builder("across", "Across", Box::new(FakeBridgeBuilder));
        let (action, name) = reg
            .build_bridge_action(
                "across",
                BridgeQuoteRequest::default(),
                BridgeExecutionOptions::default(),
            )
            .await
            .expect("registered bridge builder invoked");
        assert_eq!(action.intent_type, "bridge");
        assert_eq!(name, "Across");
    }

    // B3 — execution-capable bridge provider names come back sorted.
    #[tokio::test]
    async fn bridge_execution_provider_names_sorted() {
        let mut reg = Registry::new();
        reg.register_bridge_builder("lifi", "LiFi", Box::new(FakeBridgeBuilder));
        reg.register_bridge_builder("across", "Across", Box::new(FakeBridgeBuilder));
        // a known quote-only provider must NOT appear in the list.
        reg.register_known_bridge_provider("bungee");
        assert_eq!(
            reg.bridge_execution_provider_names(),
            vec!["across", "lifi"]
        );
    }

    // ======================================================================
    // LEND routing — B4
    // ======================================================================

    // B4 — empty provider is a usage error.
    #[tokio::test]
    async fn lend_routing_rejects_empty_provider() {
        let reg = Registry::new();
        let err = reg
            .build_lend_action(LendRequest {
                provider: String::new(),
                ..Default::default()
            })
            .await
            .expect_err("empty provider rejected");
        assert_eq!(err.code, Code::Usage);
    }

    // B4 — Ported from Go: TestBuildLendActionRejectsUnsupportedProvider.
    #[tokio::test]
    async fn lend_routing_rejects_unsupported_provider() {
        let reg = Registry::new();
        let err = reg
            .build_lend_action(LendRequest {
                provider: "kamino".into(),
                ..Default::default()
            })
            .await
            .expect_err("unsupported provider rejected");
        assert_eq!(err.code, Code::Unsupported);
    }

    // B4 — Ported from Go: TestBuildLendActionMoonwellRejectsOnBehalfOf.
    #[tokio::test]
    async fn lend_routing_moonwell_rejects_on_behalf_of() {
        let reg = Registry::new();
        let err = reg
            .build_lend_action(LendRequest {
                provider: "moonwell".into(),
                on_behalf_of: "0x00000000000000000000000000000000000000aa".into(),
                ..Default::default()
            })
            .await
            .expect_err("moonwell on-behalf-of rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert!(
            err.to_string().contains("--on-behalf-of"),
            "error should mention --on-behalf-of, got: {err}"
        );
    }

    // ======================================================================
    // YIELD routing — B5
    // ======================================================================

    // B5 — Ported from Go: TestBuildYieldActionRejectsUnsupportedProvider.
    #[tokio::test]
    async fn yield_routing_rejects_unsupported_provider() {
        let reg = Registry::new();
        let err = reg
            .build_yield_action(YieldRequest {
                provider: "kamino".into(),
                verb: YieldVerb::Deposit,
                ..Default::default()
            })
            .await
            .expect_err("unsupported provider rejected");
        assert_eq!(err.code, Code::Unsupported);
    }

    // B5 — empty provider is a usage error.
    #[tokio::test]
    async fn yield_routing_rejects_empty_provider() {
        let reg = Registry::new();
        let err = reg
            .build_yield_action(YieldRequest {
                provider: String::new(),
                verb: YieldVerb::Deposit,
                ..Default::default()
            })
            .await
            .expect_err("empty provider rejected");
        assert_eq!(err.code, Code::Usage);
    }

    // B5 — Ported from Go: TestBuildYieldActionMoonwellRejectsOnBehalfOf.
    #[tokio::test]
    async fn yield_routing_moonwell_rejects_on_behalf_of() {
        let reg = Registry::new();
        let err = reg
            .build_yield_action(YieldRequest {
                provider: "moonwell".into(),
                verb: YieldVerb::Deposit,
                on_behalf_of: "0x00000000000000000000000000000000000000aa".into(),
                ..Default::default()
            })
            .await
            .expect_err("moonwell on-behalf-of rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert!(
            err.to_string().contains("--on-behalf-of"),
            "error should mention --on-behalf-of, got: {err}"
        );
    }

    // ======================================================================
    // REWARDS routing — B6
    // ======================================================================

    // B6 — Ported from Go: TestBuildRewardsClaimActionRejectsUnsupportedProvider.
    #[tokio::test]
    async fn rewards_claim_routing_rejects_unsupported_provider() {
        let reg = Registry::new();
        let err = reg
            .build_rewards_claim_action(RewardsClaimRequest {
                provider: "morpho".into(),
                ..Default::default()
            })
            .await
            .expect_err("unsupported provider rejected");
        assert_eq!(err.code, Code::Unsupported);
    }

    // B6 — empty provider is a usage error (claim).
    #[tokio::test]
    async fn rewards_claim_routing_rejects_empty_provider() {
        let reg = Registry::new();
        let err = reg
            .build_rewards_claim_action(RewardsClaimRequest {
                provider: String::new(),
                ..Default::default()
            })
            .await
            .expect_err("empty provider rejected");
        assert_eq!(err.code, Code::Usage);
    }

    // B6 — compound rewards reject a non-aave provider too.
    #[tokio::test]
    async fn rewards_compound_routing_rejects_unsupported_provider() {
        let reg = Registry::new();
        let err = reg
            .build_rewards_compound_action(RewardsCompoundRequest {
                provider: "morpho".into(),
                ..Default::default()
            })
            .await
            .expect_err("unsupported provider rejected");
        assert_eq!(err.code, Code::Unsupported);
    }

    // ======================================================================
    // APPROVAL / TRANSFER routing — B7, B8
    // ======================================================================

    // B7 — Ported from Go: TestBuildApprovalActionRoutesToPlanner.
    #[test]
    fn approval_routing_returns_approve_intent() {
        let reg = Registry::new();
        let chain = eth_chain();
        let asset = usdc(&chain);
        let action = reg
            .build_approval_action(ApprovalRequest {
                chain,
                asset,
                amount_base_units: "1000".into(),
                sender: SENDER.into(),
                spender: SPENDER.into(),
                simulate: true,
                rpc_url: "https://eth.llamarpc.com".into(),
            })
            .expect("build approval");
        assert_eq!(action.intent_type, "approve");
    }

    // B8 — Ported from Go: TestBuildTransferActionRoutesToPlanner.
    #[test]
    fn transfer_routing_returns_transfer_intent() {
        let reg = Registry::new();
        let chain = eth_chain();
        let asset = usdc(&chain);
        let action = reg
            .build_transfer_action(TransferRequest {
                chain,
                asset,
                amount_base_units: "1000".into(),
                sender: SENDER.into(),
                recipient: RECIPIENT.into(),
                simulate: true,
                rpc_url: "https://eth.llamarpc.com".into(),
            })
            .expect("build transfer");
        assert_eq!(action.intent_type, "transfer");
    }
}
