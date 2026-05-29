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

/// The cache-key payload for `swap quote` (mirrors the Go `quoteCmd` cache-key
/// map at `runner.go` ~L1238). Field declaration/serialization order matches the
/// Go `map[string]any` rendered to canonical JSON; identical inputs MUST yield an
/// identical key (the runner hashes the canonical JSON). Built only AFTER the
/// request has been resolved so every field is the canonical normalized form.
#[derive(serde::Serialize)]
struct SwapQuoteCacheKey<'a> {
    provider: &'a str,
    chain: &'a str,
    from: &'a str,
    to: &'a str,
    trade_type: &'a str,
    amount: &'a str,
    slippage_mode: &'a str,
    slippage_pct: Option<f64>,
    /// Lowercased swapper (Go `strings.ToLower(reqStruct.Swapper)`).
    swapper: String,
    rpc_url: &'a str,
}

/// `swap quote` time-to-live (Go `runCachedCommand(..., 15*time.Second, ...)`).
const SWAP_QUOTE_TTL_SECS: u64 = 15;

/// Apply a parsed structured-input JSON map onto the raw `swap quote` flag
/// values, mirroring the Go `applyStructuredFlagInput` merge order:
/// * an explicitly-set flag (already `Some`/non-default) is never overridden;
/// * an unknown JSON key is a [`defi_errors::Code::Usage`] error
///   (`structured input field "<k>" is not supported by swap quote`);
/// * a `null` JSON value is a usage error (`... cannot be null`);
/// * otherwise the JSON value fills the unset flag.
///
/// `slippage_changed` reports whether `slippage-pct` was set (explicitly OR via
/// JSON), feeding the runner's `cmd.Flags().Changed("slippage-pct")` guard.
struct QuoteFlagValues {
    provider: String,
    chain: String,
    from_asset: String,
    to_asset: String,
    trade_type: String,
    amount: String,
    amount_decimal: String,
    amount_out: String,
    amount_out_decimal: String,
    from_address: String,
    slippage_pct: f64,
    slippage_changed: bool,
    rpc_url: String,
}

/// JSON keys the `swap quote` command accepts (flag-name `_`→`-` already
/// resolved; the field on the right is the canonical flag name). Mirrors the Go
/// local-flag set the `applyStructuredFlagInput` PreRunE merges into.
fn quote_set_flag(
    values: &mut QuoteFlagValues,
    key: &str,
    raw: &serde_json::Value,
) -> Result<(), Error> {
    // null is rejected for any recognized key (Go: cannot be null).
    if raw.is_null() {
        return Err(Error::new(
            Code::Usage,
            format!("structured input field {key:?} cannot be null"),
        ));
    }
    // Decode a scalar to its flag-string form (Go decodeRawFlagValue).
    let as_string = |v: &serde_json::Value| -> Option<String> {
        match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    };
    let canonical = key.replace('_', "-");
    match canonical.as_str() {
        "provider" => values.provider = as_string(raw).unwrap_or_default(),
        "chain" => values.chain = as_string(raw).unwrap_or_default(),
        "from-asset" => values.from_asset = as_string(raw).unwrap_or_default(),
        "to-asset" => values.to_asset = as_string(raw).unwrap_or_default(),
        "type" => values.trade_type = as_string(raw).unwrap_or_default(),
        "amount" => values.amount = as_string(raw).unwrap_or_default(),
        "amount-decimal" => values.amount_decimal = as_string(raw).unwrap_or_default(),
        "amount-out" => values.amount_out = as_string(raw).unwrap_or_default(),
        "amount-out-decimal" => values.amount_out_decimal = as_string(raw).unwrap_or_default(),
        "from-address" => values.from_address = as_string(raw).unwrap_or_default(),
        "rpc-url" => values.rpc_url = as_string(raw).unwrap_or_default(),
        "slippage-pct" => {
            let f = raw.as_f64().ok_or_else(|| {
                Error::new(
                    Code::Usage,
                    format!("decode structured input field {key:?}"),
                )
            })?;
            values.slippage_pct = f;
            values.slippage_changed = true;
        }
        _ => {
            return Err(Error::new(
                Code::Usage,
                format!("structured input field {key:?} is not supported by swap quote"),
            ));
        }
    }
    Ok(())
}

/// Map a swap-quote fetch result to the Go `statusFromErr` provider-status
/// string: `Ok` → `"ok"`; `Auth` → `"auth_error"`; `RateLimited` →
/// `"rate_limited"`; `Unavailable` → `"unavailable"`; anything else → `"error"`.
pub fn status_from_quote_result<T>(res: &Result<T, Error>) -> String {
    match res {
        Ok(_) => "ok",
        Err(err) => match err.code {
            Code::Auth => "auth_error",
            Code::RateLimited => "rate_limited",
            Code::Unavailable => "unavailable",
            _ => "error",
        },
    }
    .to_string()
}

/// Compute the deterministic `swap quote` cache key from the resolved plan +
/// request (Go `cacheKey(path, map{...})`):
/// `hex(sha256(command_path | CACHE_PAYLOAD_SCHEMA_VERSION | canonical_json(map)))`.
pub fn cache_key_for_quote(
    command_path: &str,
    plan: &SwapQuotePlan,
    req: &SwapQuoteRequest,
) -> String {
    let payload = SwapQuoteCacheKey {
        provider: &plan.provider,
        chain: &req.chain.caip2,
        from: &req.from_asset.asset_id,
        to: &req.to_asset.asset_id,
        trade_type: req.trade_type.as_str(),
        amount: &req.amount_base_units,
        slippage_mode: &plan.slippage_mode,
        slippage_pct: req.slippage_pct,
        swapper: req.swapper.to_lowercase(),
        rpc_url: &req.rpc_url,
    };
    crate::protocols::cache_key(command_path, &payload)
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
    pub async fn handle(ctx: &AppCtx, cmd: SwapCmd) -> Result<Envelope, Error> {
        match cmd {
            SwapCmd::Quote(args) => handle_quote(ctx, args).await,
            SwapCmd::Plan(_) => Err(AppCtx::unimplemented("swap plan", "WS3")),
            SwapCmd::Submit(_) => Err(AppCtx::unimplemented("swap submit", "WS4")),
            SwapCmd::Status(_) => Err(AppCtx::unimplemented("swap status", "WS4")),
        }
    }

    /// Handle `swap quote`: validate inputs, build the request, route through the
    /// selected [`defi_providers::SwapProvider`] adapter via the cache flow.
    ///
    /// Parity with the Go `quoteCmd.RunE` (`runner.go` ~L1184-1256): structured
    /// input is merged first (explicit flags win), then the pre-provider guard
    /// order ([`super::validate_swap_quote_inputs`]) runs, the request is built
    /// ([`super::parse_swap_request`]), and the provider's `QuoteSwap` is invoked
    /// inside [`crate::runner::run_cached_command`] (15s TTL) so a fresh cache
    /// hit short-circuits the provider.
    async fn handle_quote(ctx: &AppCtx, args: QuoteArgs) -> Result<Envelope, Error> {
        use defi_model::ProviderStatus;

        // 1. Resolve flag values, merging any structured input (Go PreRunE
        //    `applyStructuredFlagInput`). Explicitly-set flags are never
        //    overridden; unknown JSON keys / null values are usage errors.
        let mut values = super::QuoteFlagValues {
            provider: args.provider.clone().unwrap_or_default(),
            chain: args.chain.clone().unwrap_or_default(),
            from_asset: args.from_asset.clone().unwrap_or_default(),
            to_asset: args.to_asset.clone().unwrap_or_default(),
            trade_type: args.r#type.clone(),
            amount: args.amount.clone().unwrap_or_default(),
            amount_decimal: args.amount_decimal.clone().unwrap_or_default(),
            amount_out: args.amount_out.clone().unwrap_or_default(),
            amount_out_decimal: args.amount_out_decimal.clone().unwrap_or_default(),
            from_address: args.from_address.clone().unwrap_or_default(),
            slippage_pct: args.slippage_pct.unwrap_or(0.0),
            slippage_changed: args.slippage_pct.is_some(),
            rpc_url: args.rpc_url.clone().unwrap_or_default(),
        };
        // Track which flags the user set explicitly so the JSON never overrides
        // them (Go `changedFlagNames`). `type` defaults to "exact-input"; treat a
        // non-default value as explicit.
        let explicit: std::collections::HashSet<&str> = {
            let mut s = std::collections::HashSet::new();
            if args.provider.is_some() {
                s.insert("provider");
            }
            if args.chain.is_some() {
                s.insert("chain");
            }
            if args.from_asset.is_some() {
                s.insert("from-asset");
            }
            if args.to_asset.is_some() {
                s.insert("to-asset");
            }
            if args.r#type != "exact-input" {
                s.insert("type");
            }
            if args.amount.is_some() {
                s.insert("amount");
            }
            if args.amount_decimal.is_some() {
                s.insert("amount-decimal");
            }
            if args.amount_out.is_some() {
                s.insert("amount-out");
            }
            if args.amount_out_decimal.is_some() {
                s.insert("amount-out-decimal");
            }
            if args.from_address.is_some() {
                s.insert("from-address");
            }
            if args.slippage_pct.is_some() {
                s.insert("slippage-pct");
            }
            if args.rpc_url.is_some() {
                s.insert("rpc-url");
            }
            s
        };
        apply_quote_structured_input(&args.input, &explicit, &mut values)?;

        // 2. Pre-provider guard order (provider required -> unsupported -> type ->
        //    exact-output gate -> slippage gate -> from-address validity).
        let inputs = super::SwapQuoteInputs {
            provider: values.provider.clone(),
            trade_type: values.trade_type.clone(),
            from_address: values.from_address.clone(),
            slippage_changed: values.slippage_changed,
            slippage_pct: values.slippage_pct,
        };
        let plan = super::validate_swap_quote_inputs(&inputs, ctx.swap_provider_names())?;

        // 3. Build the canonical request, then layer slippage + swapper.
        let mut req = super::parse_swap_request(
            &values.chain,
            &values.from_asset,
            &values.to_asset,
            plan.trade_type,
            &values.amount,
            &values.amount_decimal,
            &values.amount_out,
            &values.amount_out_decimal,
            &values.rpc_url,
        )?;
        req.slippage_pct = plan.slippage_pct;
        req.swapper = plan.swapper.clone();

        // 4. Resolve the provider adapter (registered above -> always Some).
        let provider = ctx.swap_provider(&plan.provider).ok_or_else(|| {
            defi_errors::Error::new(defi_errors::Code::Unsupported, "unsupported swap provider")
        })?;

        // 5. Compose the cache key (Go cacheKey map) + fetch closure.
        let path = "swap quote";
        let key = super::cache_key_for_quote(path, &plan, &req);
        let ttl = std::time::Duration::from_secs(super::SWAP_QUOTE_TTL_SECS);
        let provider_name = provider.info().name;
        let req_for_fetch = req.clone();

        ctx.run_cached_command(path, &key, ttl, || {
            let res = crate::ctx::block_on_fetch(provider.quote_swap(req_for_fetch));
            let status = ProviderStatus {
                name: provider_name.clone(),
                status: super::status_from_quote_result(&res),
                latency_ms: 0,
            };
            match res {
                Ok(quote) => match serde_json::to_value(&quote) {
                    Ok(data) => Ok(crate::runner::FetchOutcome {
                        data,
                        providers: vec![status],
                        warnings: Vec::new(),
                        partial: false,
                    }),
                    Err(e) => {
                        let err = defi_errors::Error::wrap(
                            defi_errors::Code::Internal,
                            "serialize swap quote",
                            e,
                        );
                        let st = ProviderStatus {
                            name: provider_name.clone(),
                            status: "error".to_string(),
                            latency_ms: 0,
                        };
                        Err((vec![st], Vec::new(), false, err))
                    }
                },
                Err(err) => Err((vec![status], Vec::new(), false, err)),
            }
        })
    }

    /// Merge structured input (`--input-json` / `--input-file`) onto the resolved
    /// `swap quote` flag values (Go `applyStructuredFlagInput`).
    ///
    /// Reads the payload (mutually-exclusive `--input-json` / `--input-file`;
    /// `-` reads stdin), parses it as a JSON object, and applies each entry via
    /// [`super::quote_set_flag`] unless the flag was explicitly set on the command
    /// line. A non-object payload, unknown key, or `null` value is a usage error.
    fn apply_quote_structured_input(
        input: &crate::execflags::InputFlags,
        explicit: &std::collections::HashSet<&str>,
        values: &mut super::QuoteFlagValues,
    ) -> Result<(), Error> {
        use defi_errors::Code;

        let payload = read_structured_input(input)?;
        let payload = match payload {
            Some(p) if !p.trim().is_empty() => p,
            _ => return Ok(()),
        };

        let parsed: serde_json::Value = serde_json::from_str(&payload)
            .map_err(|e| Error::wrap(Code::Usage, "parse structured input", e))?;
        let obj = parsed
            .as_object()
            .ok_or_else(|| Error::new(Code::Usage, "structured input must be a JSON object"))?;

        for (key, raw) in obj {
            let canonical = key.replace('_', "-");
            if explicit.contains(canonical.as_str()) {
                continue;
            }
            super::quote_set_flag(values, key, raw)?;
        }
        Ok(())
    }

    /// Resolve the structured-input payload string from `--input-json` /
    /// `--input-file` (`-` = stdin), enforcing mutual exclusivity (Go
    /// `readStructuredInput`).
    fn read_structured_input(
        input: &crate::execflags::InputFlags,
    ) -> Result<Option<String>, Error> {
        use defi_errors::Code;

        let json = input.input_json.as_deref().unwrap_or("").trim().to_string();
        let file = input.input_file.as_deref().unwrap_or("").trim().to_string();
        if !json.is_empty() && !file.is_empty() {
            return Err(Error::new(
                Code::Usage,
                "use only one of --input-json or --input-file",
            ));
        }
        if !json.is_empty() {
            return Ok(Some(json));
        }
        if file.is_empty() {
            return Ok(None);
        }
        if file == "-" {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| Error::wrap(Code::Usage, "read structured input from stdin", e))?;
            return Ok(Some(buf));
        }
        let buf = std::fs::read_to_string(&file)
            .map_err(|e| Error::wrap(Code::Usage, "read structured input file", e))?;
        Ok(Some(buf))
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

#[cfg(test)]
mod quote_handler_tests {
    //! # Success criteria — `defi-app::swap` `swap quote` HANDLER (WS2 read)
    //!
    //! Go source: `internal/app/runner.go` `newSwapCommand` `quoteCmd.RunE`
    //! (lines ~1181-1256) + the cache-flow core `runCachedCommand` + the swap
    //! provider adapters (`internal/providers/{oneinch,jupiter,...}`). The pure
    //! pre-provider helpers (`normalize_trade_type`,
    //! `swap_provider_supports_exact_output`, `parse_swap_request`,
    //! `validate_swap_quote_inputs`, `resolve_swap_plan_sender`,
    //! `ensure_swap_intent`) are already covered by the sibling `tests` module
    //! and are NOT re-asserted here. THIS module asserts the WIRED HANDLER
    //! (`cli::handle` → `swap quote`): full envelope + meta, cache transitions,
    //! exit codes, flag parsing, provider routing, key-gating, and the
    //! Go-semantic error paths. The provider adapter response BODIES (per-field
    //! quote math) are owned by `defi-providers` and are NOT re-asserted here —
    //! only that the handler surfaces the adapter result into the envelope.
    //!
    //! These are LIVE commands in Go (1inch/jupiter hit real APIs), so per the
    //! migration spec §4.1 / completion plan WS2 they are NOT byte-diffed
    //! against the Go binary; instead the handler is driven offline against a
    //! `wiremock` `MockServer` through the swap-provider base-URL seam
    //! ([`AppCtx::with_swap_base`], analogous to the existing
    //! [`AppCtx::with_defillama_base`]) that the GREEN handler must honor. The
    //! 1inch base-URL `set_base_url` seam already exists on the provider client.
    //!
    //! Criteria (each maps to a Go behavior in `quoteCmd.RunE`):
    //!
    //!  Q1. **Success envelope shape (1inch / EVM).** With a valid 1inch key + a
    //!      mock 1inch Swap API, `swap quote --provider 1inch --chain 1
    //!      --from-asset USDC --to-asset DAI --amount 1000000` returns
    //!      `version="v1"`, `success=true`, `error=None`,
    //!      `meta.command="swap quote"`, `meta.partial=false`, and `data` is the
    //!      SwapQuote object with `provider="1inch"`, `chain_id="eip155:1"`,
    //!      `trade_type="exact-input"`, and `input_amount.amount_base_units` echo
    //!      of `1000000`. (Go: `provider.QuoteSwap(reqStruct)` → envelope.)
    //!
    //!  Q2. **`meta.providers[]` status row.** On success the handler records
    //!      exactly one provider status row keyed on the adapter's
    //!      `Info().Name` (`"1inch"`) with status `"ok"` (Go:
    //!      `statusFromErr(nil) == "ok"`, `provider.Info().Name`).
    //!
    //!  Q3. **Cache transition write → fresh hit.** With caching enabled, the
    //!      first identical call is `meta.cache.status="write"` (not stale); the
    //!      second identical call is a fresh `"hit"` that short-circuits the
    //!      provider (so `meta.providers` is empty). `swap quote` is a cached
    //!      read path (15s TTL — Go `runCachedCommand(..., 15*time.Second, ...)`).
    //!      With caching disabled the status stays `"miss"`.
    //!
    //!  Q4. **Provider error → full envelope + provider status (auth_error).**
    //!      `swap quote --provider 1inch` with NO key surfaces the adapter's
    //!      [`Code::Auth`] error: the handler returns the typed error (exit 10),
    //!      and the captured provider status row is `"auth_error"` (Go
    //!      `statusFromErr(CodeAuth)`). Asserted via the handler error + the
    //!      full-binary `run_with_args` exit code.
    //!
    //!  Q5. **`--provider` required (multi-provider, spec §2.5).** Missing
    //!      `--provider` is a usage error (exit 2) BEFORE any chain/asset parse
    //!      (Go: empty `NormalizeSwapProvider` → CodeUsage). Asserted via
    //!      `run_with_args` (full envelope to stderr, exit 2).
    //!
    //!  Q6. **Unknown provider → unsupported (exit 13).** `--provider bogus` is a
    //!      [`Code::Unsupported`] error (Go: not in `s.swapProviders`).
    //!
    //!  Q7. **`--type` enum + exact-output capability gate.** An invalid
    //!      `--type limit-order` is usage (exit 2). `--type exact-output
    //!      --provider 1inch` is unsupported (exit 13) — only uniswap/tempo
    //!      support exact-output. (Go `normalizeTradeType` +
    //!      `swapProviderSupportsExactOutput`.)
    //!
    //!  Q8. **uniswap key-gating + identity.** `--provider uniswap` requires a
    //!      `--from-address` (usage exit 2 when absent) AND a Uniswap API key
    //!      (auth exit 10 when the key env var is unset but a from-address is
    //!      supplied). (Go: `--from-address is required for --provider uniswap`
    //!      guard, then the adapter's key check.)
    //!
    //!  Q9. **`--input-json` precedence.** `swap quote --input-json
    //!      '{"provider":"bogus","chain":"1",...}' --provider 1inch` — the
    //!      explicit `--provider 1inch` flag OVERRIDES the JSON's
    //!      `"provider":"bogus"` (Go `applyStructuredFlagInput` only fills
    //!      flags the user did not set). Verified by NOT getting the
    //!      unsupported-provider (exit 13) the JSON value would cause; instead
    //!      the explicit 1inch flag drives the request (reaching the mock).
    //!
    //!  Q10. **`--slippage-pct` gate.** `--slippage-pct` on a non-uniswap
    //!       provider is usage (exit 2). (Go: only uniswap honors the override.)
    //!
    //! SKIPPED (owned elsewhere / wrong layer): per-field SwapQuote math
    //! (defi-providers), cache-key byte composition (runner), the
    //! `swap plan|submit|status` paths (WS3/WS4), and the JSON
    //! field-declaration-order rendering (defi-out golden tests).

    use super::cli::{handle, QuoteArgs, SwapCmd};
    use crate::cli::run_with_args;
    use crate::ctx::AppCtx;
    use defi_config::{MapEnv, Settings};
    use defi_errors::exit_code;
    use defi_errors::{Code, Error};
    use serde_json::Value;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const DEAD: &str = "0x000000000000000000000000000000000000dEaD";

    // ---- settings + env helpers ------------------------------------------

    /// JSON-output settings with caching toggled by `cache_enabled` and the
    /// 1inch / uniswap keys threaded explicitly (so the key-gated success path
    /// can pass an adapter key check). Cache/action paths point at `tmp`.
    fn settings_in(
        tmp: &std::path::Path,
        cache_enabled: bool,
        oneinch_key: &str,
        uniswap_key: &str,
    ) -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: false,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_secs(5),
            retries: 0,
            max_stale: Duration::from_secs(0),
            no_stale: false,
            cache_enabled,
            cache_path: tmp.join("cache.sqlite"),
            cache_lock_path: tmp.join("cache.lock"),
            action_store_path: tmp.join("actions.sqlite"),
            action_lock_path: tmp.join("actions.lock"),
            defillama_api_key: String::new(),
            uniswap_api_key: uniswap_key.to_string(),
            oneinch_api_key: oneinch_key.to_string(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    /// A `MapEnv` whose HOME points at a temp dir (so `Settings::load` resolves
    /// cache/config paths without touching the real home). Keeps the `TempDir`
    /// guard alive for the test's duration.
    fn env_with_home() -> (MapEnv, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let env = MapEnv::with_home(tmp.path().to_path_buf());
        (env, tmp)
    }

    /// `swap quote --provider 1inch --chain 1 --from-asset USDC --to-asset DAI
    /// --amount 1000000` flag set (the canonical EVM happy path).
    fn oneinch_quote_args() -> QuoteArgs {
        QuoteArgs {
            chain: Some("1".to_string()),
            from_asset: Some("USDC".to_string()),
            to_asset: Some("DAI".to_string()),
            provider: Some("1inch".to_string()),
            r#type: "exact-input".to_string(),
            amount: Some("1000000".to_string()),
            amount_decimal: None,
            amount_out: None,
            amount_out_decimal: None,
            from_address: None,
            slippage_pct: None,
            rpc_url: None,
            input: crate::execflags::InputFlags::default(),
        }
    }

    /// Mount a 1inch Swap API quote response on a fresh `MockServer`.
    /// Mirrors the real `{base}/swap/v6.0/{chainId}/quote` route shape the
    /// adapter targets (chain 1 → `/swap/v6.0/1/quote`).
    async fn oneinch_mock() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/swap/v6.0/1/quote"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/json")
                    .set_body_string(r#"{"dstAmount":"999847836538317147","gas":120000}"#),
            )
            .mount(&server)
            .await;
        server
    }

    // ---- Q1: success envelope shape (1inch / EVM) -------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_success_envelope_1inch() {
        let server = oneinch_mock().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "test-key", ""))
            .with_swap_base(&server.uri());

        let env = handle(&ctx, SwapCmd::Quote(oneinch_quote_args()))
            .await
            .expect("swap quote should succeed against the mock 1inch API");

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert_eq!(env.meta.command, "swap quote");
        assert!(!env.meta.partial);

        let data = env.data.as_ref().expect("data present on success");
        assert_eq!(data["provider"], Value::from("1inch"));
        assert_eq!(data["chain_id"], Value::from("eip155:1"));
        assert_eq!(data["trade_type"], Value::from("exact-input"));
        // Input amount echoed (base+decimal consistency, spec §2.4).
        assert_eq!(
            data["input_amount"]["amount_base_units"],
            Value::from("1000000")
        );
        // Adapter result is surfaced into the envelope (estimated_out present).
        assert!(
            data["estimated_out"]["amount_base_units"]
                .as_str()
                .is_some(),
            "estimated_out must be surfaced from the adapter: {data}"
        );
    }

    // ---- Q2: meta.providers[] status row ----------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_success_provider_status_ok() {
        let server = oneinch_mock().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "test-key", ""))
            .with_swap_base(&server.uri());

        let env = handle(&ctx, SwapCmd::Quote(oneinch_quote_args()))
            .await
            .expect("swap quote success");

        assert_eq!(
            env.meta.providers.len(),
            1,
            "exactly one provider status row"
        );
        assert_eq!(env.meta.providers[0].name, "1inch");
        assert_eq!(env.meta.providers[0].status, "ok");
    }

    // ---- Q3: cache transitions --------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_cache_write_then_hit() {
        let server = oneinch_mock().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), true, "test-key", ""))
            .with_swap_base(&server.uri());

        // First call: miss -> provider fetch -> cache write.
        let first = handle(&ctx, SwapCmd::Quote(oneinch_quote_args()))
            .await
            .expect("first swap quote");
        assert_eq!(
            first.meta.cache.status, "write",
            "first cache-enabled fetch should write the cache"
        );
        assert!(!first.meta.cache.stale);

        // Second identical call: fresh hit -> no provider call.
        let second = handle(&ctx, SwapCmd::Quote(oneinch_quote_args()))
            .await
            .expect("second swap quote");
        assert_eq!(
            second.meta.cache.status, "hit",
            "second identical fetch should hit the cache"
        );
        assert!(!second.meta.cache.stale);
        assert!(
            second.meta.providers.is_empty(),
            "a fresh hit must short-circuit the provider"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_cache_disabled_status_miss() {
        let server = oneinch_mock().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "test-key", ""))
            .with_swap_base(&server.uri());

        let env = handle(&ctx, SwapCmd::Quote(oneinch_quote_args()))
            .await
            .expect("swap quote");
        assert_eq!(
            env.meta.cache.status, "miss",
            "cache-disabled fetch keeps the initial miss status"
        );
    }

    // ---- Q4: provider error -> auth_error status + exit 10 ----------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_missing_1inch_key_is_auth_error() {
        // No 1inch key: the adapter's key check fails with Code::Auth. The
        // handler surfaces it as a typed error (the cache-flow records the
        // provider status as "auth_error", Go statusFromErr(CodeAuth)).
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "", ""));

        let err = handle(&ctx, SwapCmd::Quote(oneinch_quote_args()))
            .await
            .expect_err("missing 1inch key must fail");
        assert_eq!(err.code, Code::Auth);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 10);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_missing_1inch_key_full_binary_exit_10() {
        // Full-binary path: no DEFI_1INCH_API_KEY env -> exit 10 (auth).
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "swap",
                "quote",
                "--provider",
                "1inch",
                "--chain",
                "1",
                "--from-asset",
                "USDC",
                "--to-asset",
                "DAI",
                "--amount",
                "1000000",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 10,
            "missing 1inch API key must be an auth error (exit 10)"
        );
    }

    // ---- Q5: --provider required (spec §2.5) ------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_missing_provider_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "swap",
                "quote",
                "--chain",
                "1",
                "--from-asset",
                "USDC",
                "--to-asset",
                "DAI",
                "--amount",
                "1000000",
            ],
            &env,
        )
        .await;
        assert_eq!(code, 2, "missing --provider must be a usage error (exit 2)");
    }

    // ---- Q6: unknown provider -> unsupported (exit 13) --------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_unknown_provider_is_unsupported_exit_13() {
        // Asserted via `handle` so the SPECIFIC Go message is checked (the stub
        // also returns exit 13, so the message guards against a false pass).
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "test-key", ""));
        let mut args = oneinch_quote_args();
        args.provider = Some("bogus".to_string());

        let err = handle(&ctx, SwapCmd::Quote(args))
            .await
            .expect_err("unknown provider must fail");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string().contains("unsupported swap provider"),
            "expected the Go-semantic 'unsupported swap provider' message, got: {err}"
        );
    }

    // ---- Q7: --type enum + exact-output capability gate -------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_invalid_type_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "swap",
                "quote",
                "--provider",
                "1inch",
                "--chain",
                "1",
                "--from-asset",
                "USDC",
                "--to-asset",
                "DAI",
                "--amount",
                "1000000",
                "--type",
                "limit-order",
            ],
            &env,
        )
        .await;
        assert_eq!(code, 2, "invalid --type must be a usage error (exit 2)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_exact_output_on_1inch_is_unsupported_exit_13() {
        // Asserted via `handle` so the SPECIFIC capability-gate message is
        // checked (the stub also returns exit 13; the message guards against a
        // false pass).
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "test-key", ""));
        let mut args = oneinch_quote_args();
        args.r#type = "exact-output".to_string();
        args.amount = None;
        args.amount_out = Some("1000000000000000000".to_string());

        let err = handle(&ctx, SwapCmd::Quote(args))
            .await
            .expect_err("exact-output on 1inch must fail");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(exit_code(&Err(Error::new(err.code, ""))), 13);
        assert!(
            err.to_string()
                .contains("exact-output swap quotes currently support only"),
            "expected the Go-semantic exact-output capability message, got: {err}"
        );
    }

    // ---- Q8: uniswap key-gating + identity --------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_uniswap_requires_from_address_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "swap",
                "quote",
                "--provider",
                "uniswap",
                "--chain",
                "1",
                "--from-asset",
                "USDC",
                "--to-asset",
                "DAI",
                "--amount",
                "1000000",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "uniswap without --from-address must be a usage error (exit 2)"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_uniswap_missing_key_is_auth_exit_10() {
        // With a valid --from-address but NO DEFI_UNISWAP_API_KEY, the request
        // passes the identity guard and reaches the adapter key check -> auth.
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "swap",
                "quote",
                "--provider",
                "uniswap",
                "--chain",
                "1",
                "--from-asset",
                "USDC",
                "--to-asset",
                "DAI",
                "--amount",
                "1000000",
                "--from-address",
                DEAD,
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 10,
            "uniswap without an API key must be an auth error (exit 10)"
        );
    }

    // ---- Q9: --input-json precedence (explicit flag overrides JSON) -------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_explicit_provider_overrides_input_json() {
        // The JSON sets provider="bogus" (which would be exit 13), but the
        // explicit --provider 1inch flag must win (Go applyStructuredFlagInput
        // only fills flags the user did not set). With a 1inch key + the mock
        // base, the request reaches the mock and succeeds (exit 0), proving the
        // explicit flag overrode the JSON value.
        let server = oneinch_mock().await;
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = AppCtx::new(settings_in(tmp.path(), false, "test-key", ""))
            .with_swap_base(&server.uri());

        let mut args = oneinch_quote_args();
        // provider explicitly set to 1inch via the flag.
        args.provider = Some("1inch".to_string());
        args.input = crate::execflags::InputFlags {
            input_json: Some(
                r#"{"provider":"bogus","chain":"1","from_asset":"USDC","to_asset":"DAI","amount":"1000000"}"#
                    .to_string(),
            ),
            input_file: None,
        };

        let env = handle(&ctx, SwapCmd::Quote(args))
            .await
            .expect("explicit --provider 1inch must override the JSON provider");
        assert!(env.success);
        assert_eq!(
            env.data.as_ref().expect("data")["provider"],
            Value::from("1inch")
        );
    }

    // ---- Q10: --slippage-pct gate -----------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn quote_slippage_pct_on_non_uniswap_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "swap",
                "quote",
                "--provider",
                "1inch",
                "--chain",
                "1",
                "--from-asset",
                "USDC",
                "--to-asset",
                "DAI",
                "--amount",
                "1000000",
                "--slippage-pct",
                "1.0",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "--slippage-pct on a non-uniswap provider must be a usage error (exit 2)"
        );
    }
}
