//! `swap` command group handler (Go: `internal/app` — `newSwapCommand` in
//! `runner.go`).
//!
//! This module owns the **swap-command-specific** glue that sits between the
//! runner's cache-flow core ([`crate::runner`]), the swap quote providers
//! ([`defi_providers::SwapProvider`]), and the action-build registry
//! ([`defi_execution::builder::Registry`]). Specifically it owns:
//!
//! * `--type` parsing + the per-provider exact-output capability gate;
//! * the swap quote/plan request builder (`parse_swap_request`) — chain/asset
//!   parsing, amount normalization, and the exact-input/exact-output flag
//!   cross-validation;
//! * the `swap quote` pre-provider guard order (provider required, exact-output
//!   gate, `--slippage-pct` gate, `--from-address` requirements);
//! * the `swap plan` identity resolution (Tempo `--from-address` only vs the
//!   standard `--wallet`/`--from-address` path) and the schema input
//!   constraints it advertises;
//! * the persisted-intent gate (`swap submit`/`status` reject a non-`swap`
//!   action).
//!
//! The provider-name normalization (`normalize_swap_provider`), the
//! `SwapTradeType`/`SwapQuoteRequest`/`SwapExecutionOptions` types, the action
//! build registry routing (`build_swap_action`), and the cache-flow core are
//! owned elsewhere (`defi_providers::normalize`, `defi_execution::builder`,
//! [`crate::runner`]) and are NOT re-owned here; this module consumes them.

#![allow(dead_code, unused_variables)]

use defi_errors::{Code, Error};
use defi_execution::{SwapQuoteRequest, SwapTradeType};
use defi_id::{normalize_amount, parse_asset, parse_chain};
use defi_providers::normalize::normalize_swap_provider;
use defi_schema::InputConstraint;
use indexmap::IndexMap;

/// Normalize a raw `--type` flag value into a [`SwapTradeType`].
///
/// Parity with the Go `normalizeTradeType` closure: trim + lowercase, empty or
/// `exact-input` → [`SwapTradeType::ExactInput`], `exact-output` →
/// [`SwapTradeType::ExactOutput`], anything else → a [`defi_errors::Code::Usage`]
/// error whose message is `--type must be exact-input or exact-output`.
pub fn normalize_trade_type(raw: &str) -> Result<SwapTradeType, Error> {
    SwapTradeType::parse(raw)
        .ok_or_else(|| Error::new(Code::Usage, "--type must be exact-input or exact-output"))
}

/// Whether a (normalized) swap provider supports exact-output trades.
///
/// Parity with the Go `swapProviderSupportsExactOutput` closure: only `uniswap`
/// and `tempo` (the input is normalized via the providers helper first). All
/// other providers return `false`.
pub fn swap_provider_supports_exact_output(provider: &str) -> bool {
    matches!(
        normalize_swap_provider(provider).as_str(),
        "uniswap" | "tempo"
    )
}

/// Build a [`SwapQuoteRequest`] from the raw chain/asset/amount/type flags.
///
/// Parity with the Go `parseSwapRequest` closure:
/// 1. parse `chain` (delegates to `defi_id::parse_chain`);
/// 2. parse `from_asset` then `to_asset` on that chain;
/// 3. for **exact-input**: reject any `amount_out*`, normalize the input amount
///    against `from_asset.decimals` (defaulting non-positive decimals to 18);
/// 4. for **exact-output**: reject any `amount*`, require an `amount_out*`,
///    normalize against `to_asset.decimals` (default 18).
///
/// The returned request carries the canonical `amount_base_units` +
/// `amount_decimal`, the trimmed `rpc_url`, and the `trade_type`. `slippage_pct`
/// / `swapper` are NOT set here (the caller layers those on). All validation
/// failures are [`defi_errors::Code::Usage`] errors.
#[allow(clippy::too_many_arguments)]
pub fn parse_swap_request(
    chain_arg: &str,
    from_asset_arg: &str,
    to_asset_arg: &str,
    trade_type: SwapTradeType,
    amount_base: &str,
    amount_decimal: &str,
    amount_out_base: &str,
    amount_out_decimal: &str,
    rpc_url: &str,
) -> Result<SwapQuoteRequest, Error> {
    let chain = parse_chain(chain_arg)?;
    let from_asset = parse_asset(from_asset_arg, &chain)?;
    let to_asset = parse_asset(to_asset_arg, &chain)?;

    let (base, decimal) = match trade_type {
        SwapTradeType::ExactInput => {
            if !amount_out_base.is_empty() || !amount_out_decimal.is_empty() {
                return Err(Error::new(
                    Code::Usage,
                    "--amount-out/--amount-out-decimal are only valid with --type exact-output",
                ));
            }
            let decimals = if from_asset.decimals <= 0 {
                18
            } else {
                from_asset.decimals
            };
            normalize_amount(amount_base, amount_decimal, decimals)?
        }
        SwapTradeType::ExactOutput => {
            if !amount_base.is_empty() || !amount_decimal.is_empty() {
                return Err(Error::new(
                    Code::Usage,
                    "--amount/--amount-decimal are only valid with --type exact-input",
                ));
            }
            if amount_out_base.is_empty() && amount_out_decimal.is_empty() {
                return Err(Error::new(
                    Code::Usage,
                    "exact-output requires --amount-out or --amount-out-decimal",
                ));
            }
            let decimals = if to_asset.decimals <= 0 {
                18
            } else {
                to_asset.decimals
            };
            normalize_amount(amount_out_base, amount_out_decimal, decimals)?
        }
    };

    Ok(SwapQuoteRequest {
        chain,
        from_asset,
        to_asset,
        amount_base_units: base,
        amount_decimal: decimal,
        rpc_url: rpc_url.trim().to_string(),
        trade_type,
        slippage_pct: None,
        swapper: String::new(),
    })
}

/// The raw `swap quote` flags relevant to pre-provider validation.
#[derive(Debug, Clone, Default)]
pub struct SwapQuoteInputs {
    /// Raw `--provider` value (un-normalized).
    pub provider: String,
    /// Raw `--type` value.
    pub trade_type: String,
    /// Raw `--from-address` value.
    pub from_address: String,
    /// Whether `--slippage-pct` was explicitly set on the command line (Go
    /// `cmd.Flags().Changed("slippage-pct")`).
    pub slippage_changed: bool,
    /// The `--slippage-pct` value (only meaningful when `slippage_changed`).
    pub slippage_pct: f64,
}

/// A validated `swap quote` query: the resolved provider/type plus the slippage
/// + swapper to layer onto the [`SwapQuoteRequest`].
#[derive(Debug, Clone, PartialEq)]
pub struct SwapQuotePlan {
    /// Canonical (normalized) swap provider name.
    pub provider: String,
    /// Parsed trade type.
    pub trade_type: SwapTradeType,
    /// Slippage override (`Some` only when `--slippage-pct` was set).
    pub slippage_pct: Option<f64>,
    /// `"auto"` unless an override was supplied (`"manual"`) — feeds the cache
    /// key.
    pub slippage_mode: String,
    /// Trimmed swapper / sender address (verbatim casing).
    pub swapper: String,
}

/// Validate the pre-provider inputs of `swap quote`.
///
/// Parity with the Go `quoteCmd` `RunE` guard order (each failure
/// [`defi_errors::Code::Usage`] unless noted):
/// 1. empty `--provider` → usage (`--provider is required (...)`);
/// 2. provider not in `known` (the supplied set of registered swap providers) →
///    [`defi_errors::Code::Unsupported`] (`unsupported swap provider`);
/// 3. `--type` parses (usage on unknown);
/// 4. exact-output requested for a provider that does not support it →
///    [`defi_errors::Code::Unsupported`];
/// 5. `--slippage-pct` set for a non-`uniswap` provider → usage; out of
///    `(0,100]` → usage; otherwise `slippage_mode = "manual"`;
/// 6. a non-empty `--from-address` that is not a valid EVM hex address → usage;
/// 7. `uniswap` with an empty `--from-address` → usage.
///
/// `known` is the set of registered swap provider names (already normalized) so
/// this is testable without a live provider map. On success returns the
/// [`SwapQuotePlan`].
pub fn validate_swap_quote_inputs(
    inputs: &SwapQuoteInputs,
    known: &[&str],
) -> Result<SwapQuotePlan, Error> {
    // 1. provider required (normalized first, like the Go runner).
    let provider = normalize_swap_provider(&inputs.provider);
    if provider.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "--provider is required (1inch|uniswap|tempo|taikoswap|jupiter|fibrous|bungee)",
        ));
    }
    // 2. provider must be registered.
    if !known.contains(&provider.as_str()) {
        return Err(Error::new(Code::Unsupported, "unsupported swap provider"));
    }
    // 3. --type parses.
    let trade_type = normalize_trade_type(&inputs.trade_type)?;
    // 4. exact-output capability gate.
    if trade_type == SwapTradeType::ExactOutput && !swap_provider_supports_exact_output(&provider) {
        return Err(Error::new(
            Code::Unsupported,
            "exact-output swap quotes currently support only --provider uniswap or --provider tempo",
        ));
    }

    // 5. slippage override gate.
    let mut slippage_pct = None;
    let mut slippage_mode = "auto".to_string();
    if inputs.slippage_changed {
        if provider != "uniswap" {
            return Err(Error::new(
                Code::Usage,
                "--slippage-pct is supported only with --provider uniswap",
            ));
        }
        if inputs.slippage_pct <= 0.0 || inputs.slippage_pct > 100.0 {
            return Err(Error::new(
                Code::Usage,
                "--slippage-pct must be > 0 and <= 100",
            ));
        }
        slippage_mode = "manual".to_string();
        slippage_pct = Some(inputs.slippage_pct);
    }

    // 6. from-address validity.
    let swapper = inputs.from_address.trim().to_string();
    if !swapper.is_empty() && !defi_evm::address::is_hex_address(&swapper) {
        return Err(Error::new(
            Code::Usage,
            "--from-address must be a valid EVM hex address",
        ));
    }
    // 7. uniswap requires a from-address.
    if provider == "uniswap" && swapper.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "--from-address is required for --provider uniswap",
        ));
    }

    Ok(SwapQuotePlan {
        provider,
        trade_type,
        slippage_pct,
        slippage_mode,
        swapper,
    })
}

/// The resolved `swap plan` identity (the sender address to build the action
/// with, plus any warnings the standard identity resolver surfaced).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapPlanSender {
    /// The sender EOA used to build the action.
    pub sender: String,
    /// `true` when the sender came from the Tempo `--from-address`-only path
    /// (the caller then stamps `execution_backend = tempo`).
    pub is_tempo: bool,
    /// Warnings surfaced by the standard identity resolver (empty for Tempo).
    pub warnings: Vec<String>,
}

/// Resolve the `swap plan` sender for the (normalized) provider.
///
/// Parity with the Go `planCmd` identity branch:
/// * **tempo**: reject supplying both `--wallet` and `--from-address` (usage);
///   reject `--wallet` entirely ([`defi_errors::Code::Unsupported`],
///   `--wallet planning is not supported on Tempo chains yet`); require a
///   non-empty `--from-address` (usage); the `--from-address` must be a valid
///   EVM hex address (usage); the resolved sender is its EIP-55 checksum form
///   (parity with go-ethereum `common.HexToAddress(..).Hex()`); `is_tempo`
///   is `true`, no warnings.
/// * **standard** (taikoswap / everything else): delegate to the shared
///   execution-identity resolver, returning its `from_address` + warnings;
///   `is_tempo` is `false`.
///
/// `resolve_standard` models the runner's `resolveExecutionIdentity` for the
/// non-Tempo path (returns the resolved `(from_address, warnings)` or a typed
/// error), kept injectable so this guard order is testable in isolation.
pub fn resolve_swap_plan_sender<F>(
    provider: &str,
    wallet_ref: &str,
    from_address: &str,
    resolve_standard: F,
) -> Result<SwapPlanSender, Error>
where
    F: FnOnce() -> Result<(String, Vec<String>), Error>,
{
    if normalize_swap_provider(provider) == "tempo" {
        let wallet = wallet_ref.trim();
        let addr = from_address.trim();
        if !wallet.is_empty() && !addr.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "use only one identity input: --wallet or --from-address",
            ));
        }
        if !wallet.is_empty() {
            return Err(Error::new(
                Code::Unsupported,
                "--wallet planning is not supported on Tempo chains yet; use --from-address",
            ));
        }
        if addr.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "--from-address is required for --provider tempo",
            ));
        }
        if !defi_evm::address::is_hex_address(addr) {
            return Err(Error::new(
                Code::Usage,
                "--from-address must be a valid EVM hex address",
            ));
        }
        let sender = defi_evm::address::checksum(addr)?;
        return Ok(SwapPlanSender {
            sender,
            is_tempo: true,
            warnings: Vec::new(),
        });
    }

    let (sender, warnings) = resolve_standard()?;
    Ok(SwapPlanSender {
        sender,
        is_tempo: false,
        warnings,
    })
}

/// The provider-specific `swap plan` schema input constraints.
///
/// Parity with Go `swapPlanIdentityInputConstraints`: three entries in this
/// exact order —
/// 1. `required` on `from_address` when `provider == tempo`;
/// 2. `forbidden` on `wallet` when `provider == tempo`;
/// 3. `exactly_one_of` on `[wallet, from_address]` when `provider == taikoswap`.
pub fn swap_plan_identity_constraints() -> Vec<InputConstraint> {
    fn when_provider(value: &str) -> IndexMap<String, Vec<String>> {
        let mut when = IndexMap::new();
        when.insert("provider".to_string(), vec![value.to_string()]);
        when
    }

    vec![
        InputConstraint {
            kind: "required".to_string(),
            fields: vec!["from_address".to_string()],
            when: when_provider("tempo"),
            description:
                "Tempo planning requires `from_address` and does not support `wallet` yet."
                    .to_string(),
        },
        InputConstraint {
            kind: "forbidden".to_string(),
            fields: vec!["wallet".to_string()],
            when: when_provider("tempo"),
            description: "Tempo planning rejects `wallet`; use `from_address`.".to_string(),
        },
        InputConstraint {
            kind: "exactly_one_of".to_string(),
            fields: vec!["wallet".to_string(), "from_address".to_string()],
            when: when_provider("taikoswap"),
            description: "TaikoSwap planning requires exactly one execution identity input: \
                `wallet` (OWS, recommended) or `from_address` (local signer)."
                .to_string(),
        },
    ]
}

/// Validate that a persisted action is a `swap` intent.
///
/// Parity with the `submit` / `status` guard `action.IntentType != "swap"`: a
/// non-`swap` intent yields a [`defi_errors::Code::Usage`] error whose message
/// is `action is not a swap intent`.
pub fn ensure_swap_intent(intent_type: &str) -> Result<(), Error> {
    if intent_type != "swap" {
        return Err(Error::new(Code::Usage, "action is not a swap intent"));
    }
    Ok(())
}

/// clap parsing + handler for the `swap` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::Error;
    use defi_model::Envelope;

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, SubmitArgs};

    /// `swap` subcommands (Go `newSwapCommand`).
    #[derive(Subcommand, Debug)]
    pub enum SwapCmd {
        /// Get swap quote.
        Quote(QuoteArgs),
        /// Create and persist a swap action plan.
        Plan(PlanArgs),
        /// Execute a previously planned swap action.
        Submit(SubmitArgs),
        /// Get swap action status.
        Status(StatusArgs),
    }

    impl SwapCmd {
        /// The leaf path token (for `meta.command`).
        pub fn path(&self) -> &'static str {
            match self {
                SwapCmd::Quote(_) => "quote",
                SwapCmd::Plan(_) => "plan",
                SwapCmd::Submit(_) => "submit",
                SwapCmd::Status(_) => "status",
            }
        }
    }

    /// `swap quote` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct QuoteArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Input asset.
        #[arg(long = "from-asset")]
        pub from_asset: Option<String>,
        /// Output asset.
        #[arg(long = "to-asset")]
        pub to_asset: Option<String>,
        /// Swap provider (1inch|uniswap|tempo|taikoswap|jupiter|fibrous|bungee).
        #[arg(long)]
        pub provider: Option<String>,
        /// Swap type (exact-input|exact-output).
        #[arg(long, default_value = "exact-input")]
        pub r#type: String,
        /// Exact-input amount in base units.
        #[arg(long)]
        pub amount: Option<String>,
        /// Exact-input amount in decimal units.
        #[arg(long = "amount-decimal")]
        pub amount_decimal: Option<String>,
        /// Exact-output amount in base units.
        #[arg(long = "amount-out")]
        pub amount_out: Option<String>,
        /// Exact-output amount in decimal units.
        #[arg(long = "amount-out-decimal")]
        pub amount_out_decimal: Option<String>,
        /// Swapper/sender EOA address (required for --provider uniswap).
        #[arg(long = "from-address")]
        pub from_address: Option<String>,
        /// Manual max slippage percent override (Uniswap only).
        #[arg(long = "slippage-pct")]
        pub slippage_pct: Option<f64>,
        /// RPC URL override for on-chain quote providers.
        #[arg(long = "rpc-url")]
        pub rpc_url: Option<String>,
        #[command(flatten)]
        pub input: crate::execflags::InputFlags,
    }

    /// `swap plan` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct PlanArgs {
        /// Chain identifier.
        #[arg(long)]
        pub chain: Option<String>,
        /// Input asset.
        #[arg(long = "from-asset")]
        pub from_asset: Option<String>,
        /// Output asset.
        #[arg(long = "to-asset")]
        pub to_asset: Option<String>,
        /// Swap execution provider (taikoswap|tempo).
        #[arg(long)]
        pub provider: Option<String>,
        /// Swap type (exact-input|exact-output).
        #[arg(long, default_value = "exact-input")]
        pub r#type: String,
        /// Exact-input amount in base units.
        #[arg(long)]
        pub amount: Option<String>,
        /// Exact-input amount in decimal units.
        #[arg(long = "amount-decimal")]
        pub amount_decimal: Option<String>,
        /// Exact-output amount in base units.
        #[arg(long = "amount-out")]
        pub amount_out: Option<String>,
        /// Exact-output amount in decimal units.
        #[arg(long = "amount-out-decimal")]
        pub amount_out_decimal: Option<String>,
        /// Recipient address (defaults to the resolved sender address).
        #[arg(long)]
        pub recipient: Option<String>,
        /// Max slippage in basis points.
        #[arg(long = "slippage-bps", default_value_t = 50)]
        pub slippage_bps: i64,
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

    /// Handle `swap <sub>`.
    pub async fn handle(_ctx: &AppCtx, cmd: SwapCmd) -> Result<Envelope, Error> {
        let path = format!("swap {}", cmd.path());
        let ws = match cmd {
            SwapCmd::Quote(_) => "WS2",
            SwapCmd::Plan(_) => "WS3",
            SwapCmd::Submit(_) | SwapCmd::Status(_) => "WS4",
        };
        Err(AppCtx::unimplemented(&path, ws))
    }
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::swap` (Go: `internal/app` swap command
    //! group: `newSwapCommand` in `runner.go`)
    //!
    //! This module owns the **swap-command glue**. "Correct" means it preserves
    //! the runner-owned swap behaviors AND the stable machine contract (design
    //! spec §2.2 exit codes, §2.4 ids/amounts, §2.5 multi-provider paths require
    //! an explicit `--provider`). The provider-name normalization
    //! (`normalize_swap_provider`), the request/option/trade-type types, the
    //! action-build registry routing (`build_swap_action`), and the cache-flow
    //! core are owned elsewhere and are NOT re-asserted here. Criteria:
    //!
    //! 1. **`--type` parsing.** `normalize_trade_type`: empty / `exact-input`
    //!    (any casing/whitespace) → `ExactInput`; `exact-output` →
    //!    `ExactOutput`; anything else → [`Code::Usage`] (exit 2) with message
    //!    `--type must be exact-input or exact-output`. (Go `normalizeTradeType`
    //!    + `TestSwapTypeValidation`.)
    //! 2. **Exact-output capability gate.** `swap_provider_supports_exact_output`
    //!    is `true` only for `uniswap` / `tempo` (input normalized first, so
    //!    aliases like `tempo-dex` resolve), `false` otherwise. (Go
    //!    `swapProviderSupportsExactOutput`.)
    //! 3. **Request building + amount/flag cross-validation.**
    //!    `parse_swap_request` mirrors Go `parseSwapRequest`. (a) exact-input
    //!    rejects `--amount-out*` (usage), normalizes the input amount against
    //!    `from_asset.decimals`. (b) exact-output rejects `--amount*` (usage),
    //!    REQUIRES `--amount-out*` (usage), normalizes against
    //!    `to_asset.decimals`. (c) base/decimal forms stay consistent (spec
    //!    §2.4) — exact-output of `1` ETH (18 decimals) yields base
    //!    `1000000000000000000` + decimal `1`; the `rpc_url` is trimmed and the
    //!    `trade_type` is carried. (Ported from
    //!    `TestSwapExactOutputPassedToProvider`,
    //!    `TestSwapExactOutputTempoPassedToProvider`,
    //!    `TestSwapExactOutputRequiresOutputAmount`.)
    //! 4. **`swap quote` pre-provider guard order + exit codes.**
    //!    `validate_swap_quote_inputs` mirrors the Go `quoteCmd` guards. (a)
    //!    empty `--provider` → usage BEFORE anything else (spec §2.5). (b) an
    //!    unknown provider → [`Code::Unsupported`] (exit 13). (c) exact-output
    //!    for a non-capable provider → unsupported. (d) `--slippage-pct` set for
    //!    non-`uniswap` → usage; out of `(0,100]` → usage; valid →
    //!    `slippage_mode = "manual"` + `Some(pct)`. (e) a non-hex
    //!    `--from-address` → usage; `uniswap` with empty `--from-address` →
    //!    usage. (f) happy path returns the normalized provider, parsed type,
    //!    swapper verbatim, and `slippage_mode = "auto"` when no override.
    //!    (Ported from `TestSwapQuoteWithJupiterForSolana`,
    //!    `TestSwapQuoteWithOneInchForEVM`, `TestSwapSlippageOverridePassedToProvider`,
    //!    `TestSwapSlippageOverrideValidation`,
    //!    `TestSwapSlippageOverrideRejectedForNonUniswap`,
    //!    `TestSwapExactOutputRequiresExplicitProvider`,
    //!    `TestSwapExactOutputWithoutProviderRejectedOnSolana`.)
    //! 5. **`swap plan` identity resolution.** `resolve_swap_plan_sender`. (a)
    //!    tempo + empty `--from-address` → usage (Go
    //!    `TestRunnerSwapPlanRequiresFromAddress`, which exits 2). (b) tempo +
    //!    `--wallet` → [`Code::Unsupported`] with
    //!    `--wallet planning is not supported on Tempo chains yet`
    //!    (`TestRunnerSwapPlanTempoRejectsWallet`). (c) tempo + both `--wallet`
    //!    and `--from-address` → usage. (d) tempo + valid `--from-address` →
    //!    checksummed sender, `is_tempo` true (the caller then stamps
    //!    `execution_backend = tempo`, per
    //!    `TestRunnerSwapPlanTempoSetsTempoExecutionBackend`). (e) standard
    //!    provider → delegates to the injected resolver, carrying its sender +
    //!    warnings, `is_tempo` false.
    //! 6. **`swap plan` schema constraints.** `swap_plan_identity_constraints`
    //!    returns exactly the tempo-`required`, tempo-`forbidden`, and
    //!    taikoswap-`exactly_one_of` entries in that order. (Ported from
    //!    `TestSwapPlanSchemaIncludesProviderSpecificIdentityConstraints`.)
    //! 7. **Persisted-intent gate.** `ensure_swap_intent` accepts `"swap"` and
    //!    rejects any other intent with [`Code::Usage`] +
    //!    `action is not a swap intent`. (Ported from
    //!    `TestRunnerSwapStatusRejectsNonSwapIntent`.)
    //!
    //! SKIPPED (Go internal-detail / wrong-module): cobra flag wiring + flag
    //! defaults, cache-key construction (runner concern), the full submit
    //! signer/backend plumbing and receipt polling (execution-crate concern),
    //! and provider adapter response bodies (per-provider concern).

    use super::*;
    use defi_errors::{exit_code, Code};

    // --- helpers -----------------------------------------------------------

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    // The dEaD checksum address (EIP-55 mixed case), matching go-ethereum's
    // `common.HexToAddress("0x...dead").Hex()`.
    const DEAD: &str = "0x000000000000000000000000000000000000dEaD";

    // --- 1. --type parsing -------------------------------------------------

    #[test]
    fn normalize_trade_type_defaults_and_parses() {
        assert_eq!(
            normalize_trade_type("").expect("empty"),
            SwapTradeType::ExactInput
        );
        assert_eq!(
            normalize_trade_type("exact-input").expect("exact-input"),
            SwapTradeType::ExactInput
        );
        assert_eq!(
            normalize_trade_type("  EXACT-INPUT ").expect("trim+case"),
            SwapTradeType::ExactInput
        );
        assert_eq!(
            normalize_trade_type("exact-output").expect("exact-output"),
            SwapTradeType::ExactOutput
        );
    }

    #[test]
    fn normalize_trade_type_rejects_unknown() {
        // Parity with TestSwapTypeValidation ("limit-order").
        let err = normalize_trade_type("limit-order").expect_err("unknown type rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("exact-input or exact-output"),
            "got: {err}"
        );
    }

    // --- 2. exact-output capability gate -----------------------------------

    #[test]
    fn exact_output_capability_gate() {
        assert!(swap_provider_supports_exact_output("uniswap"));
        assert!(swap_provider_supports_exact_output("tempo"));
        // alias resolves via normalize_swap_provider first.
        assert!(swap_provider_supports_exact_output("tempo-dex"));
        assert!(!swap_provider_supports_exact_output("1inch"));
        assert!(!swap_provider_supports_exact_output("jupiter"));
        assert!(!swap_provider_supports_exact_output("taikoswap"));
        assert!(!swap_provider_supports_exact_output(""));
    }

    // --- 3. request building + amount/flag cross-validation ----------------

    #[test]
    fn parse_request_exact_input_normalizes_and_carries_fields() {
        let req = parse_swap_request(
            "1",
            "USDC",
            "DAI",
            SwapTradeType::ExactInput,
            "1000000",
            "",
            "",
            "",
            "  https://rpc.example  ",
        )
        .expect("exact-input request");
        assert_eq!(req.chain.caip2, "eip155:1");
        assert_eq!(req.amount_base_units, "1000000");
        assert_eq!(req.trade_type, SwapTradeType::ExactInput);
        // rpc_url is trimmed.
        assert_eq!(req.rpc_url, "https://rpc.example");
    }

    #[test]
    fn parse_request_exact_input_rejects_amount_out() {
        let err = parse_swap_request(
            "1",
            "USDC",
            "DAI",
            SwapTradeType::ExactInput,
            "1000000",
            "",
            "1000000000000000000",
            "",
            "",
        )
        .expect_err("amount-out with exact-input rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn parse_request_exact_output_normalizes_against_to_asset_decimals() {
        // Parity with TestSwapExactOutputPassedToProvider: 1 ETH (18 decimals)
        // → base 1000000000000000000, decimal "1".
        let req = parse_swap_request(
            "1",
            "USDC",
            "WETH",
            SwapTradeType::ExactOutput,
            "",
            "",
            "1000000000000000000",
            "",
            "",
        )
        .expect("exact-output request");
        assert_eq!(req.trade_type, SwapTradeType::ExactOutput);
        assert_eq!(req.amount_base_units, "1000000000000000000");
        assert_eq!(req.amount_decimal, "1");
    }

    #[test]
    fn parse_request_exact_output_rejects_input_amount() {
        let err = parse_swap_request(
            "1",
            "USDC",
            "WETH",
            SwapTradeType::ExactOutput,
            "1000000",
            "",
            "",
            "",
            "",
        )
        .expect_err("amount with exact-output rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn parse_request_exact_output_requires_output_amount() {
        // Parity with TestSwapExactOutputRequiresOutputAmount.
        let err = parse_swap_request(
            "1",
            "USDC",
            "WETH",
            SwapTradeType::ExactOutput,
            "",
            "",
            "",
            "",
            "",
        )
        .expect_err("missing output amount rejected");
        assert_eq!(err.code, Code::Usage);
    }

    // --- 4. swap quote pre-provider guard order ----------------------------

    fn quote_inputs(provider: &str) -> SwapQuoteInputs {
        SwapQuoteInputs {
            provider: provider.to_string(),
            trade_type: "exact-input".to_string(),
            from_address: String::new(),
            slippage_changed: false,
            slippage_pct: 0.0,
        }
    }

    const KNOWN: &[&str] = &["1inch", "uniswap", "tempo", "jupiter", "taikoswap"];

    #[test]
    fn quote_requires_provider_first() {
        // Parity with TestSwapExactOutputRequiresExplicitProvider /
        // TestSwapExactOutputWithoutProviderRejectedOnSolana (spec §2.5: no
        // implicit provider default).
        let err = validate_swap_quote_inputs(&quote_inputs(""), KNOWN)
            .expect_err("empty provider rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    #[test]
    fn quote_rejects_unknown_provider() {
        let err = validate_swap_quote_inputs(&quote_inputs("bogus"), KNOWN)
            .expect_err("unknown provider rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
    }

    #[test]
    fn quote_routes_known_evm_and_solana_providers() {
        // Parity with TestSwapQuoteWithOneInchForEVM + TestSwapQuoteWithJupiterForSolana:
        // the explicitly named provider is resolved, no implicit fallback.
        let plan =
            validate_swap_quote_inputs(&quote_inputs("1inch"), KNOWN).expect("1inch resolves");
        assert_eq!(plan.provider, "1inch");
        let plan =
            validate_swap_quote_inputs(&quote_inputs("jupiter"), KNOWN).expect("jupiter resolves");
        assert_eq!(plan.provider, "jupiter");
        // default (no override) => auto slippage, no swapper.
        assert_eq!(plan.slippage_mode, "auto");
        assert_eq!(plan.slippage_pct, None);
        assert!(plan.swapper.is_empty());
    }

    #[test]
    fn quote_exact_output_gate_blocks_non_capable_provider() {
        let mut inputs = quote_inputs("1inch");
        inputs.trade_type = "exact-output".to_string();
        let err =
            validate_swap_quote_inputs(&inputs, KNOWN).expect_err("exact-output on 1inch rejected");
        assert_eq!(err.code, Code::Unsupported);
    }

    #[test]
    fn quote_exact_output_gate_allows_uniswap() {
        let mut inputs = quote_inputs("uniswap");
        inputs.trade_type = "exact-output".to_string();
        inputs.from_address = DEAD.to_string();
        let plan =
            validate_swap_quote_inputs(&inputs, KNOWN).expect("exact-output uniswap allowed");
        assert_eq!(plan.trade_type, SwapTradeType::ExactOutput);
    }

    #[test]
    fn quote_slippage_override_rejected_for_non_uniswap() {
        // Parity with TestSwapSlippageOverrideRejectedForNonUniswap.
        let mut inputs = quote_inputs("1inch");
        inputs.slippage_changed = true;
        inputs.slippage_pct = 1.0;
        let err = validate_swap_quote_inputs(&inputs, KNOWN)
            .expect_err("slippage override on 1inch rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn quote_slippage_override_out_of_range_rejected() {
        // Parity with TestSwapSlippageOverrideValidation (--slippage-pct 0).
        let mut inputs = quote_inputs("uniswap");
        inputs.from_address = DEAD.to_string();
        inputs.slippage_changed = true;
        inputs.slippage_pct = 0.0;
        let err = validate_swap_quote_inputs(&inputs, KNOWN).expect_err("zero slippage rejected");
        assert_eq!(err.code, Code::Usage);

        inputs.slippage_pct = 100.5;
        let err =
            validate_swap_quote_inputs(&inputs, KNOWN).expect_err("over-100 slippage rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn quote_slippage_override_valid_sets_manual_mode() {
        // Parity with TestSwapSlippageOverridePassedToProvider.
        let mut inputs = quote_inputs("uniswap");
        inputs.from_address = DEAD.to_string();
        inputs.slippage_changed = true;
        inputs.slippage_pct = 1.25;
        let plan = validate_swap_quote_inputs(&inputs, KNOWN).expect("valid slippage override");
        assert_eq!(plan.slippage_mode, "manual");
        assert_eq!(plan.slippage_pct, Some(1.25));
        // swapper carried verbatim (casing preserved).
        assert_eq!(plan.swapper, DEAD);
    }

    #[test]
    fn quote_rejects_non_hex_from_address() {
        let mut inputs = quote_inputs("uniswap");
        inputs.from_address = "not-an-address".to_string();
        let err =
            validate_swap_quote_inputs(&inputs, KNOWN).expect_err("non-hex from-address rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn quote_uniswap_requires_from_address() {
        // uniswap with no --from-address is a usage error.
        let err = validate_swap_quote_inputs(&quote_inputs("uniswap"), KNOWN)
            .expect_err("uniswap requires from-address");
        assert_eq!(err.code, Code::Usage);
    }

    // --- 5. swap plan identity resolution ----------------------------------

    fn deny_standard() -> Result<(String, Vec<String>), Error> {
        panic!("standard resolver must not be called on the tempo path")
    }

    #[test]
    fn plan_tempo_requires_from_address() {
        // Parity with TestRunnerSwapPlanRequiresFromAddress (exit 2).
        let err = resolve_swap_plan_sender("tempo", "", "", deny_standard)
            .expect_err("tempo requires from-address");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
    }

    #[test]
    fn plan_tempo_rejects_wallet() {
        // Parity with TestRunnerSwapPlanTempoRejectsWallet.
        let err = resolve_swap_plan_sender("tempo", "wallet-123", "", deny_standard)
            .expect_err("tempo wallet rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert!(
            err.to_string()
                .contains("--wallet planning is not supported on Tempo chains yet"),
            "got: {err}"
        );
    }

    #[test]
    fn plan_tempo_rejects_both_identity_inputs() {
        let err = resolve_swap_plan_sender("tempo", "wallet-123", DEAD, deny_standard)
            .expect_err("both identity inputs rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn plan_tempo_rejects_non_hex_from_address() {
        let err = resolve_swap_plan_sender("tempo", "", "not-an-address", deny_standard)
            .expect_err("non-hex from-address rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn plan_tempo_checksums_sender() {
        // Parity with TestRunnerSwapPlanTempoSetsTempoExecutionBackend: the
        // sender is the EIP-55 checksum (go-ethereum HexToAddress(..).Hex()).
        let resolved = resolve_swap_plan_sender(
            "tempo",
            "",
            "0x00000000000000000000000000000000000000aa",
            deny_standard,
        )
        .expect("tempo from-address accepted");
        assert!(resolved.is_tempo);
        assert!(resolved.warnings.is_empty());
        // lowercase in, EIP-55 checksum out. The trailing `aa` checksums to
        // `AA` (verified against go-ethereum
        // `common.HexToAddress("0x..aa").Hex()` == "0x..AA").
        assert_eq!(
            resolved.sender,
            "0x00000000000000000000000000000000000000AA"
        );
    }

    #[test]
    fn plan_standard_delegates_to_resolver() {
        let resolved = resolve_swap_plan_sender("taikoswap", "wallet-x", "", || {
            Ok((DEAD.to_string(), vec!["heads up".to_string()]))
        })
        .expect("standard identity resolved");
        assert!(!resolved.is_tempo);
        assert_eq!(resolved.sender, DEAD);
        assert_eq!(resolved.warnings, vec!["heads up".to_string()]);
    }

    #[test]
    fn plan_standard_propagates_resolver_error() {
        let err = resolve_swap_plan_sender("taikoswap", "", "", || {
            Err(Error::new(Code::Usage, "no identity"))
        })
        .expect_err("resolver error propagated");
        assert_eq!(err.code, Code::Usage);
    }

    // --- 6. swap plan schema constraints -----------------------------------

    #[test]
    fn plan_identity_constraints_match_go() {
        // Parity with TestSwapPlanSchemaIncludesProviderSpecificIdentityConstraints.
        let constraints = swap_plan_identity_constraints();
        assert_eq!(constraints.len(), 3);

        // 1. tempo required from_address.
        assert_eq!(constraints[0].kind, "required");
        assert_eq!(constraints[0].fields, vec!["from_address".to_string()]);
        assert_eq!(
            constraints[0].when.get("provider"),
            Some(&vec!["tempo".to_string()])
        );

        // 2. tempo forbidden wallet.
        assert_eq!(constraints[1].kind, "forbidden");
        assert_eq!(constraints[1].fields, vec!["wallet".to_string()]);
        assert_eq!(
            constraints[1].when.get("provider"),
            Some(&vec!["tempo".to_string()])
        );

        // 3. taikoswap exactly_one_of.
        assert_eq!(constraints[2].kind, "exactly_one_of");
        assert_eq!(
            constraints[2].fields,
            vec!["wallet".to_string(), "from_address".to_string()]
        );
        assert_eq!(
            constraints[2].when.get("provider"),
            Some(&vec!["taikoswap".to_string()])
        );
    }

    // --- 7. persisted-intent gate ------------------------------------------

    #[test]
    fn ensure_swap_intent_accepts_swap() {
        ensure_swap_intent("swap").expect("swap intent accepted");
    }

    #[test]
    fn ensure_swap_intent_rejects_non_swap() {
        // Parity with TestRunnerSwapStatusRejectsNonSwapIntent (bridge action).
        let err = ensure_swap_intent("bridge").expect_err("non-swap intent rejected");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string().contains("action is not a swap intent"),
            "got: {err}"
        );
    }
}
