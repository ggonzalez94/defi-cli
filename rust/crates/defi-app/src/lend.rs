//! `lend` command group handler (Go: `internal/app` — `newLendCommand` in
//! `runner.go` + `lend_execution_commands.go`).
//!
//! This module owns the **lend-command-specific** glue that sits between the
//! runner's cache-flow core ([`crate::runner`]) and the provider/execution
//! layers:
//!
//! * the lend read commands (`markets` / `rates` / `positions`) — provider
//!   routing, per-command limit truncation, and the `positions` input
//!   validation + capability gate;
//! * the lend execution verb → persisted-intent mapping (`lend_<verb>`) used by
//!   `supply|withdraw|borrow|repay {plan,submit,status}`.
//!
//! The provider-selection helpers shared with `yield`
//! (`normalize_lending_provider`, `parse_lend_position_type`) live in
//! [`crate::runner`]; the action-construction routing (`build_lend_action`)
//! lives in `defi_execution::builder`. This module deliberately does NOT
//! re-own those; it consumes them.

#![allow(dead_code, unused_variables)]

use defi_errors::{Code, Error};
use defi_execution::builder::LendVerb;
use defi_id::Chain;
use defi_model::{LendMarket, LendPosition, LendRate};
use defi_providers::{LendPositionType, LendPositionsRequest, LendingPositionsProvider};

/// Truncate a list of lend markets to `limit`.
///
/// Parity with Go `applyLendMarketLimit`: a non-positive `limit`, or a list
/// already at/under the limit, is returned unchanged; otherwise the first
/// `limit` items are kept (order preserved).
pub fn apply_lend_market_limit(mut items: Vec<LendMarket>, limit: i64) -> Vec<LendMarket> {
    if limit <= 0 || (items.len() as i64) <= limit {
        return items;
    }
    items.truncate(limit as usize);
    items
}

/// Truncate a list of lend rates to `limit`.
///
/// Parity with Go `applyLendRateLimit` (same semantics as
/// [`apply_lend_market_limit`]).
pub fn apply_lend_rate_limit(mut items: Vec<LendRate>, limit: i64) -> Vec<LendRate> {
    if limit <= 0 || (items.len() as i64) <= limit {
        return items;
    }
    items.truncate(limit as usize);
    items
}

/// The persisted action intent type for a lend execution verb.
///
/// Parity with Go `expectedIntent := "lend_" + string(verb)` in
/// `lend_execution_commands.go`. `plan` writes this onto the action; `submit` /
/// `status` reject an action whose `intent_type` does not match.
pub fn lend_verb_intent(verb: LendVerb) -> String {
    let suffix = match verb {
        LendVerb::Supply => "supply",
        LendVerb::Withdraw => "withdraw",
        LendVerb::Borrow => "borrow",
        LendVerb::Repay => "repay",
    };
    format!("lend_{suffix}")
}

/// A validated `lend positions` query (the inputs needed to build a
/// [`LendPositionsRequest`] for the selected provider).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LendPositionsQuery {
    /// Canonical (normalized) lending provider name.
    pub provider: String,
    /// Parsed chain.
    pub chain: Chain,
    /// The position-owner account (verbatim, un-lowercased — caller lowercases
    /// for the cache key on EVM chains).
    pub account: String,
    /// Parsed position-type filter (empty input defaults to
    /// [`LendPositionType::All`]).
    pub position_type: LendPositionType,
}

/// Validate the pre-provider inputs of `lend positions`.
///
/// Parity with the `positionsCmd` `RunE` guard order in `runner.go`:
/// 1. `--provider` is required (usage) — normalized via the runner helper;
/// 2. `--chain` parses (delegates to `defi_id::parse_chain`);
/// 3. `--address` is required (usage);
/// 4. on EVM chains, `--address` must be a valid hex address (usage);
/// 5. `--type` parses (usage on an unknown value), empty → `All`.
///
/// On success returns the [`LendPositionsQuery`]; the provider is NOT yet
/// consulted (matching the Go ordering where validation precedes provider
/// selection / the cached fetch closure).
pub fn validate_lend_positions_input(
    provider: &str,
    chain_arg: &str,
    address: &str,
    type_arg: &str,
) -> Result<LendPositionsQuery, Error> {
    // 1. `--provider` is required (normalized via the runner helper).
    let provider_name = crate::runner::normalize_lending_provider(provider);
    if provider_name.is_empty() {
        return Err(Error::new(Code::Usage, "--provider is required"));
    }

    // 2. `--chain` parses (surfaces the id parse error verbatim).
    let chain = defi_id::parse_chain(chain_arg)?;

    // 3. `--address` is required.
    let account = address.trim().to_string();
    if account.is_empty() {
        return Err(Error::new(Code::Usage, "--address is required"));
    }

    // 4. On EVM chains, `--address` must be a valid hex address (parity with
    //    go-ethereum `common.IsHexAddress`).
    if chain.is_evm() && !defi_evm::address::is_hex_address(&account) {
        return Err(Error::new(
            Code::Usage,
            "--address must be a valid EVM hex address",
        ));
    }

    // 5. `--type` parses (empty → `All`).
    let position_type = crate::runner::parse_lend_position_type(type_arg)?;

    Ok(LendPositionsQuery {
        provider: provider_name,
        chain,
        account,
        position_type,
    })
}

/// Fetch lend positions, enforcing the provider-capability gate.
///
/// Parity with the Go interface assertion
/// `provider.(providers.LendingPositionsProvider)`: a selected lending provider
/// that does not implement positions yields a [`defi_errors::Code::Unsupported`]
/// error whose message contains `"does not support positions"` (modeled here as
/// `positions == None`). Otherwise the request is forwarded to the provider.
pub async fn fetch_lend_positions(
    provider_name: &str,
    positions: Option<&dyn LendingPositionsProvider>,
    req: LendPositionsRequest,
) -> Result<Vec<LendPosition>, Error> {
    match positions {
        None => Err(Error::new(
            Code::Unsupported,
            format!("lending provider {provider_name} does not support positions"),
        )),
        Some(provider) => provider.lend_positions(req).await,
    }
}

/// clap parsing + handler for the `lend` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::Error;
    use defi_model::Envelope;

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, SubmitArgs};

    /// `lend` subcommands: read data + the four execution verbs.
    #[derive(Subcommand, Debug)]
    pub enum LendCmd {
        /// List lending markets.
        Markets(MarketsArgs),
        /// List lending rates.
        Rates(MarketsArgs),
        /// List lending positions for an account address.
        Positions(PositionsArgs),
        /// Supply assets to a lending protocol.
        #[command(subcommand)]
        Supply(LendVerbCmd),
        /// Withdraw assets from a lending protocol.
        #[command(subcommand)]
        Withdraw(LendVerbCmd),
        /// Borrow assets from a lending protocol.
        #[command(subcommand)]
        Borrow(LendVerbCmd),
        /// Repay borrowed assets on a lending protocol.
        #[command(subcommand)]
        Repay(LendVerbCmd),
    }

    impl LendCmd {
        /// The full path tail (e.g. `markets`, `supply plan`) for `meta.command`.
        pub fn path(&self) -> String {
            match self {
                LendCmd::Markets(_) => "markets".to_string(),
                LendCmd::Rates(_) => "rates".to_string(),
                LendCmd::Positions(_) => "positions".to_string(),
                LendCmd::Supply(v) => format!("supply {}", v.path()),
                LendCmd::Withdraw(v) => format!("withdraw {}", v.path()),
                LendCmd::Borrow(v) => format!("borrow {}", v.path()),
                LendCmd::Repay(v) => format!("repay {}", v.path()),
            }
        }
    }

    /// `lend markets` / `lend rates` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct MarketsArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Asset (symbol/address/CAIP-19).
        #[arg(long)]
        pub asset: Option<String>,
        /// Lending provider (aave, morpho, kamino, moonwell).
        #[arg(long)]
        pub provider: Option<String>,
        /// Maximum rows to return.
        #[arg(long, default_value_t = 20)]
        pub limit: i64,
        /// Optional RPC URL override for on-chain providers.
        #[arg(long = "rpc-url")]
        pub rpc_url: Option<String>,
    }

    /// `lend positions` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct PositionsArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Position owner address.
        #[arg(long)]
        pub address: Option<String>,
        /// Optional asset filter (symbol/address/CAIP-19).
        #[arg(long)]
        pub asset: Option<String>,
        /// Lending provider (aave, morpho, moonwell).
        #[arg(long)]
        pub provider: Option<String>,
        /// Position type filter (all|supply|borrow|collateral).
        #[arg(long, default_value = "all")]
        pub r#type: String,
        /// Maximum positions to return.
        #[arg(long, default_value_t = 20)]
        pub limit: i64,
        /// Optional RPC URL override used by providers that need on-chain reads.
        #[arg(long = "rpc-url")]
        pub rpc_url: Option<String>,
    }

    /// The `plan` / `submit` / `status` sub-subcommands shared by every lend verb.
    #[derive(Subcommand, Debug)]
    pub enum LendVerbCmd {
        /// Create and persist a lend action plan.
        Plan(LendPlanArgs),
        /// Execute an existing lend action.
        Submit(SubmitArgs),
        /// Get lend action status.
        Status(StatusArgs),
    }

    impl LendVerbCmd {
        /// The leaf path token (`plan`/`submit`/`status`).
        pub fn path(&self) -> &'static str {
            match self {
                LendVerbCmd::Plan(_) => "plan",
                LendVerbCmd::Submit(_) => "submit",
                LendVerbCmd::Status(_) => "status",
            }
        }
    }

    /// `lend <verb> plan` flags (shared across supply/withdraw/borrow/repay).
    #[derive(Args, Debug, Clone, Default)]
    pub struct LendPlanArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Asset symbol/address/CAIP-19.
        #[arg(long)]
        pub asset: Option<String>,
        /// Amount in base units.
        #[arg(long)]
        pub amount: Option<String>,
        /// Amount in decimal units.
        #[arg(long = "amount-decimal")]
        pub amount_decimal: Option<String>,
        /// Lending provider (aave|morpho|moonwell).
        #[arg(long)]
        pub provider: Option<String>,
        /// Recipient address (defaults to the resolved sender address).
        #[arg(long)]
        pub recipient: Option<String>,
        /// Position owner address (defaults to the resolved sender address).
        #[arg(long = "on-behalf-of")]
        pub on_behalf_of: Option<String>,
        /// Aave borrow/repay mode (1=stable,2=variable).
        #[arg(long = "interest-rate-mode", default_value_t = 2)]
        pub interest_rate_mode: i64,
        /// Morpho market unique key (required for --provider morpho).
        #[arg(long = "market-id")]
        pub market_id: Option<String>,
        /// Aave pool address override.
        #[arg(long = "pool-address")]
        pub pool_address: Option<String>,
        /// Aave pool address provider override.
        #[arg(long = "pool-address-provider")]
        pub pool_address_provider: Option<String>,
        /// RPC URL override for the selected chain.
        #[arg(long = "rpc-url")]
        pub rpc_url: Option<String>,
        /// Include simulation checks during execution.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        pub simulate: bool,
        #[command(flatten)]
        pub identity: PlanIdentityFlags,
        #[command(flatten)]
        pub input: crate::execflags::InputFlags,
    }

    /// Handle `lend <sub>`.
    ///
    /// Reads (`markets`/`rates`/`positions`) are WS2; execution verbs are
    /// WS3 (`plan`) / WS4 (`submit`/`status`). All route here; unimplemented
    /// leaves return a typed `Unsupported` error (never `unknown command`).
    pub async fn handle(_ctx: &AppCtx, cmd: LendCmd) -> Result<Envelope, Error> {
        let path = format!("lend {}", cmd.path());
        let ws = if matches!(
            cmd,
            LendCmd::Markets(_) | LendCmd::Rates(_) | LendCmd::Positions(_)
        ) {
            "WS2"
        } else if path.ends_with("plan") {
            "WS3"
        } else {
            "WS4"
        };
        Err(AppCtx::unimplemented(&path, ws))
    }
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::lend` (Go: `internal/app` lend command
    //! group: `newLendCommand` in `runner.go` + `lend_execution_commands.go`)
    //!
    //! This module owns the **lend-command glue**. "Correct" means it preserves
    //! the runner-owned lend behaviors AND the stable machine contract (design
    //! spec §2.2 exit codes, §2.4 ids/amounts). The provider-selection helpers
    //! (`normalize_lending_provider`, `parse_lend_position_type`) and the
    //! cache-flow core are owned by [`crate::runner`] and are NOT re-asserted
    //! here; the action-construction routing is owned by
    //! `defi_execution::builder` and is NOT re-asserted here. Criteria:
    //!
    //! 1. **Per-command limit truncation.** `apply_lend_market_limit` /
    //!    `apply_lend_rate_limit`: a non-positive limit, or a list already
    //!    at/under the limit, is returned UNCHANGED (no realloc/drop); a list
    //!    longer than the limit keeps exactly the first `limit` items in order.
    //!    (Go `applyLendMarketLimit` / `applyLendRateLimit`.)
    //! 2. **Execution intent mapping.** `lend_verb_intent(verb)` is exactly
    //!    `"lend_<verb>"` (`lend_supply`/`lend_withdraw`/`lend_borrow`/
    //!    `lend_repay`) — the persisted `Action.intent_type` that `plan` writes
    //!    and that `submit`/`status` match against. (Go
    //!    `expectedIntent := "lend_" + string(verb)`.)
    //! 3. **`positions` input validation order + exit codes.**
    //!    `validate_lend_positions_input` mirrors the Go `positionsCmd` guard
    //!    order, every failure carrying [`Code::Usage`] (exit 2):
    //!    a. empty `--provider` → usage error BEFORE chain parsing;
    //!    b. an unparseable `--chain` surfaces the id error;
    //!    c. empty `--address` → usage error;
    //!    d. on an EVM chain, a non-hex `--address` → usage error (parity with
    //!    go-ethereum `common.IsHexAddress`);
    //!    e. an unknown `--type` → usage error;
    //!    and on success it returns the normalized provider, parsed chain, the
    //!    verbatim account, and the parsed position type (empty → `All`).
    //!    (Ported from `TestRunnerLendPositionsRejectsInvalidType`,
    //!    `TestRunnerLendPositionsRejectsInvalidEVMAddress`, and the happy-path
    //!    setup in `TestRunnerLendPositionsCallsProvider`.)
    //! 4. **Provider-capability gate.** `fetch_lend_positions` with
    //!    `positions == None` (the selected lending provider does not implement
    //!    positions) fails with [`Code::Unsupported`] (exit 13) and a message
    //!    containing `"does not support positions"`, WITHOUT touching the
    //!    provider. (Ported from
    //!    `TestRunnerLendPositionsRequiresProviderCapability`.)
    //! 5. **Provider request forwarding.** `fetch_lend_positions` with a capable
    //!    provider forwards the request verbatim (account, asset filter, type,
    //!    limit) exactly once and returns the provider's rows. (Ported from
    //!    `TestRunnerLendPositionsCallsProvider`.)
    //!
    //! SKIPPED (Go internal-detail / wrong-module): cobra flag wiring,
    //! cache-key construction (runner concern), and the full `plan/submit/status`
    //! signer/backend plumbing (execution-crate concern, covered there).

    use super::*;
    use async_trait::async_trait;
    use defi_errors::{exit_code, Code};
    use defi_id::{parse_chain, Asset};
    use defi_model::{AmountInfo, ProviderInfo};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // --- fixtures ----------------------------------------------------------

    fn market(provider: &str) -> LendMarket {
        LendMarket {
            protocol: provider.to_string(),
            provider: provider.to_string(),
            chain_id: "eip155:1".to_string(),
            asset_id: "eip155:1/erc20:0xa0b8".to_string(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            supply_apy: 1.0,
            borrow_apy: 2.0,
            tvl_usd: 3.0,
            liquidity_usd: 4.0,
            source_url: String::new(),
            fetched_at: "2026-05-28T00:00:00Z".to_string(),
        }
    }

    fn rate(provider: &str) -> LendRate {
        LendRate {
            protocol: provider.to_string(),
            provider: provider.to_string(),
            chain_id: "eip155:1".to_string(),
            asset_id: "eip155:1/erc20:0xa0b8".to_string(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            supply_apy: 1.0,
            borrow_apy: 2.0,
            utilization: 0.5,
            source_url: String::new(),
            fetched_at: "2026-05-28T00:00:00Z".to_string(),
        }
    }

    fn position(provider: &str) -> LendPosition {
        LendPosition {
            protocol: provider.to_string(),
            provider: provider.to_string(),
            chain_id: "eip155:1".to_string(),
            account_address: "0x000000000000000000000000000000000000dead".to_string(),
            position_type: "collateral".to_string(),
            asset_id: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".to_string(),
            provider_native_id: String::new(),
            provider_native_id_kind: String::new(),
            amount: AmountInfo::default(),
            amount_usd: 0.0,
            apy: 0.0,
            source_url: String::new(),
            fetched_at: "2026-05-28T00:00:00Z".to_string(),
        }
    }

    /// A fake positions-capable provider that records the request it received.
    struct FakeLendingPositionsProvider {
        name: String,
        rows: Vec<LendPosition>,
        calls: AtomicUsize,
        last_req: std::sync::Mutex<Option<LendPositionsRequest>>,
    }

    impl FakeLendingPositionsProvider {
        fn new(name: &str, rows: Vec<LendPosition>) -> Self {
            Self {
                name: name.to_string(),
                rows,
                calls: AtomicUsize::new(0),
                last_req: std::sync::Mutex::new(None),
            }
        }
    }

    impl defi_providers::Provider for FakeLendingPositionsProvider {
        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                name: self.name.clone(),
                provider_type: "lending".to_string(),
                requires_key: false,
                capabilities: vec![
                    "lend.markets".to_string(),
                    "lend.rates".to_string(),
                    "lend.positions".to_string(),
                ],
                key_env_var_name: String::new(),
                capability_auth: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl LendingPositionsProvider for FakeLendingPositionsProvider {
        async fn lend_positions(
            &self,
            req: LendPositionsRequest,
        ) -> Result<Vec<LendPosition>, Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_req.lock().unwrap() = Some(req);
            Ok(self.rows.clone())
        }
    }

    // --- 1. limit truncation ----------------------------------------------

    #[test]
    fn apply_lend_market_limit_truncates_and_passes_through() {
        let items = vec![market("aave"), market("morpho"), market("moonwell")];
        // non-positive limit => unchanged.
        assert_eq!(apply_lend_market_limit(items.clone(), 0).len(), 3);
        assert_eq!(apply_lend_market_limit(items.clone(), -1).len(), 3);
        // limit >= len => unchanged.
        assert_eq!(apply_lend_market_limit(items.clone(), 3).len(), 3);
        assert_eq!(apply_lend_market_limit(items.clone(), 10).len(), 3);
        // limit < len => first `limit` items, order preserved.
        let truncated = apply_lend_market_limit(items.clone(), 2);
        assert_eq!(truncated.len(), 2);
        assert_eq!(truncated[0].provider, "aave");
        assert_eq!(truncated[1].provider, "morpho");
    }

    #[test]
    fn apply_lend_rate_limit_truncates_and_passes_through() {
        let items = vec![rate("aave"), rate("morpho"), rate("moonwell")];
        assert_eq!(apply_lend_rate_limit(items.clone(), 0).len(), 3);
        assert_eq!(apply_lend_rate_limit(items.clone(), 5).len(), 3);
        let truncated = apply_lend_rate_limit(items.clone(), 1);
        assert_eq!(truncated.len(), 1);
        assert_eq!(truncated[0].provider, "aave");
    }

    // --- 2. execution intent mapping --------------------------------------

    #[test]
    fn lend_verb_intent_is_lend_prefixed_verb() {
        assert_eq!(lend_verb_intent(LendVerb::Supply), "lend_supply");
        assert_eq!(lend_verb_intent(LendVerb::Withdraw), "lend_withdraw");
        assert_eq!(lend_verb_intent(LendVerb::Borrow), "lend_borrow");
        assert_eq!(lend_verb_intent(LendVerb::Repay), "lend_repay");
    }

    // --- 3. positions input validation ------------------------------------

    #[test]
    fn positions_input_requires_provider_before_chain() {
        // empty provider => usage, even with an otherwise-bogus chain (the
        // provider guard fires first).
        let err = validate_lend_positions_input("", "not-a-chain", "0xabc", "all")
            .expect_err("empty provider rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 2);
    }

    #[test]
    fn positions_input_rejects_unparseable_chain() {
        let err = validate_lend_positions_input("aave", "definitely-not-a-chain", "0xabc", "all")
            .expect_err("bad chain rejected");
        // id parse errors are usage-coded.
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn positions_input_requires_address() {
        let err = validate_lend_positions_input("aave", "1", "", "all")
            .expect_err("empty address rejected");
        assert_eq!(err.code, Code::Usage);
        assert!(
            err.to_string().to_lowercase().contains("address"),
            "got: {err}"
        );
    }

    #[test]
    fn positions_input_rejects_invalid_evm_address() {
        // Parity with TestRunnerLendPositionsRejectsInvalidEVMAddress.
        let err = validate_lend_positions_input("aave", "1", "not-an-address", "all")
            .expect_err("invalid evm address rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn positions_input_rejects_invalid_type() {
        // Parity with TestRunnerLendPositionsRejectsInvalidType ("debt").
        let err = validate_lend_positions_input(
            "aave",
            "1",
            "0x000000000000000000000000000000000000dEaD",
            "debt",
        )
        .expect_err("invalid type rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn positions_input_accepts_valid_inputs_and_normalizes() {
        // Parity with the happy path of TestRunnerLendPositionsCallsProvider:
        // provider alias normalized, EVM address accepted verbatim, type parsed.
        let q = validate_lend_positions_input(
            "AAVE-V3",
            "1",
            "0x000000000000000000000000000000000000dEaD",
            "collateral",
        )
        .expect("valid positions input");
        assert_eq!(q.provider, "aave");
        assert_eq!(q.chain.caip2, "eip155:1");
        // account preserved verbatim (caller lowercases only for the cache key).
        assert_eq!(q.account, "0x000000000000000000000000000000000000dEaD");
        assert_eq!(q.position_type, LendPositionType::Collateral);
    }

    #[test]
    fn positions_input_empty_type_defaults_to_all() {
        let q = validate_lend_positions_input(
            "aave",
            "1",
            "0x000000000000000000000000000000000000dEaD",
            "",
        )
        .expect("valid positions input");
        assert_eq!(q.position_type, LendPositionType::All);
    }

    // --- 4. provider-capability gate --------------------------------------

    #[tokio::test]
    async fn fetch_positions_without_capability_is_unsupported() {
        // Parity with TestRunnerLendPositionsRequiresProviderCapability.
        let req = LendPositionsRequest {
            chain: parse_chain("solana").expect("solana"),
            account: "6dM4QgP1VnRfx6TVV1t5hBf3ytA5Qn2ATqNnSboP8qz5".to_string(),
            asset: Asset::default(),
            position_type: LendPositionType::All,
            limit: 20,
            rpc_url: String::new(),
        };
        let err = fetch_lend_positions("kamino", None, req)
            .await
            .expect_err("missing positions capability rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .to_lowercase()
                .contains("does not support positions"),
            "got: {err}"
        );
    }

    // --- 5. provider request forwarding -----------------------------------

    #[tokio::test]
    async fn fetch_positions_forwards_request_and_returns_rows() {
        // Parity with TestRunnerLendPositionsCallsProvider.
        let provider = FakeLendingPositionsProvider::new("aave", vec![position("aave")]);
        let req = LendPositionsRequest {
            chain: parse_chain("1").expect("mainnet"),
            account: "0x000000000000000000000000000000000000dead".to_string(),
            asset: Asset {
                chain_id: "eip155:1".to_string(),
                symbol: "USDC".to_string(),
                ..Asset::default()
            },
            position_type: LendPositionType::Collateral,
            limit: 5,
            rpc_url: String::new(),
        };

        let rows = fetch_lend_positions("aave", Some(&provider), req)
            .await
            .expect("positions fetched");

        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider, "aave");

        let last = provider.last_req.lock().unwrap();
        let last = last.as_ref().expect("request recorded");
        assert_eq!(last.position_type, LendPositionType::Collateral);
        assert!(last
            .account
            .eq_ignore_ascii_case("0x000000000000000000000000000000000000dead"));
        assert!(last.asset.symbol.eq_ignore_ascii_case("USDC"));
        assert_eq!(last.limit, 5);
    }
}
