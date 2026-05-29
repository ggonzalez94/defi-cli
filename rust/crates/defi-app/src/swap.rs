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
/// map at `runner.go` ~L1238). The Go key is a `map[string]any` hashed via
/// `cacheKey`'s `json.Marshal`, and `encoding/json` emits map keys in
/// **alphabetical** order — so the field declaration order here is ALPHABETICAL
/// (`amount, chain, from, provider, rpc_url, slippage_mode, slippage_pct,
/// swapper, to, trade_type`) to produce a byte-identical canonical JSON payload
/// and therefore a cross-binary-stable cache key. Built only AFTER the request
/// has been resolved so every field is the canonical normalized form.
#[derive(serde::Serialize)]
struct SwapQuoteCacheKey<'a> {
    amount: &'a str,
    chain: &'a str,
    from: &'a str,
    provider: &'a str,
    rpc_url: &'a str,
    slippage_mode: &'a str,
    slippage_pct: Option<f64>,
    /// Lowercased swapper (Go `strings.ToLower(reqStruct.Swapper)`).
    swapper: String,
    to: &'a str,
    trade_type: &'a str,
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
    use crate::execflags::{decode_f64_field, decode_string_field};

    // null is rejected for any recognized key (Go: cannot be null).
    if raw.is_null() {
        return Err(Error::new(
            Code::Usage,
            format!("structured input field {key:?} cannot be null"),
        ));
    }
    let canonical = key.replace('_', "-");
    match canonical.as_str() {
        "provider" => values.provider = decode_string_field(key, raw)?,
        "chain" => values.chain = decode_string_field(key, raw)?,
        "from-asset" => values.from_asset = decode_string_field(key, raw)?,
        "to-asset" => values.to_asset = decode_string_field(key, raw)?,
        "type" => values.trade_type = decode_string_field(key, raw)?,
        "amount" => values.amount = decode_string_field(key, raw)?,
        "amount-decimal" => values.amount_decimal = decode_string_field(key, raw)?,
        "amount-out" => values.amount_out = decode_string_field(key, raw)?,
        "amount-out-decimal" => values.amount_out_decimal = decode_string_field(key, raw)?,
        "from-address" => values.from_address = decode_string_field(key, raw)?,
        "rpc-url" => values.rpc_url = decode_string_field(key, raw)?,
        "slippage-pct" => {
            values.slippage_pct = decode_f64_field(key, raw)?;
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
        amount: &req.amount_base_units,
        chain: &req.chain.caip2,
        from: &req.from_asset.asset_id,
        provider: &plan.provider,
        rpc_url: &req.rpc_url,
        slippage_mode: &plan.slippage_mode,
        slippage_pct: req.slippage_pct,
        swapper: req.swapper.to_lowercase(),
        to: &req.to_asset.asset_id,
        trade_type: req.trade_type.as_str(),
    };
    crate::protocols::cache_key(command_path, &payload)
}

/// clap parsing + handler for the `swap` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::{Code, Error};
    use defi_model::{Envelope, ProviderStatus};

    use crate::ctx::AppCtx;
    use crate::execflags::{PlanIdentityFlags, StatusArgs, SubmitArgs};
    use crate::execident::{apply_execution_identity_to_action, resolve_execution_identity};

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
            SwapCmd::Plan(args) => handle_plan(ctx, args).await,
            SwapCmd::Submit(_) => Err(AppCtx::unimplemented("swap submit", "WS4")),
            SwapCmd::Status(_) => Err(AppCtx::unimplemented("swap status", "WS4")),
        }
    }

    /// Handle `swap plan` (Go `planCmd.RunE`, `runner.go` ~L1343-1431).
    ///
    /// Capability-based swap planning. Flow parity with the Go runner:
    /// 1. `--provider` required (empty → usage, BEFORE anything else);
    /// 2. `--type` parses (usage on unknown);
    /// 3. exact-output capability gate: a non-tempo provider with
    ///    `--type exact-output` → unsupported, BEFORE any build/persist;
    /// 4. build the canonical [`SwapQuoteRequest`] ([`super::parse_swap_request`]:
    ///    chain/asset parse, amount/flag cross-validation, base+decimal carry);
    /// 5. resolve the sender identity — Tempo uses the `--from-address`-only path
    ///    ([`super::resolve_swap_plan_sender`]); every other provider uses the
    ///    shared OWS-first [`resolve_execution_identity`];
    /// 6. route the build through the populated action-build registry
    ///    ([`Registry::build_swap_action`] → the `taikoswap`/`tempo`
    ///    `SwapActionBuilder`; unknown/quote-only providers error here), capturing
    ///    a single [`ProviderStatus`] keyed on the builder display name;
    /// 7. stamp the identity onto the action (Tempo: `from_address = checksummed
    ///    sender` + `execution_backend = tempo`; standard:
    ///    [`apply_execution_identity_to_action`]), persist to the action [`Store`],
    ///    and emit the success envelope (cache bypassed for execution paths,
    ///    spec §2.5) carrying the identity warnings.
    ///
    /// On every guard/build error the typed [`Error`] is returned (the runner
    /// renders the full error envelope to stderr) and NOTHING is persisted.
    ///
    /// [`Registry`]: defi_execution::builder::Registry
    /// [`Store`]: defi_execution::store::Store
    /// [`SwapQuoteRequest`]: defi_execution::SwapQuoteRequest
    async fn handle_plan(ctx: &AppCtx, args: PlanArgs) -> Result<Envelope, Error> {
        use defi_execution::action::ExecutionBackend;
        use defi_execution::SwapExecutionOptions;
        use defi_providers::normalize::normalize_swap_provider;

        // 0. Merge structured input (`--input-json` / `--input-file`) onto the
        //    resolved flag values before any guard (Go PreRunE
        //    `applyStructuredFlagInput`). Explicitly-set flags are never
        //    overridden; an unknown key / null value is a usage error.
        let values = resolve_plan_values(&args)?;

        // 1. `--provider` required (normalized first, like the Go runner).
        let provider_name = normalize_swap_provider(&values.provider);
        if provider_name.is_empty() {
            return Err(Error::new(Code::Usage, "--provider is required"));
        }

        // 2. `--type` parses (usage on unknown).
        let trade_type = super::normalize_trade_type(&values.trade_type)?;

        // 3. exact-output capability gate (BEFORE any build/persist).
        if trade_type == defi_execution::SwapTradeType::ExactOutput
            && !super::swap_provider_supports_exact_output(&provider_name)
        {
            return Err(Error::new(
                Code::Unsupported,
                "exact-output swap planning currently supports only --provider tempo",
            ));
        }

        // 4. Build the canonical request (chain/asset parse, amount cross-validation).
        let req = super::parse_swap_request(
            &values.chain,
            &values.from_asset,
            &values.to_asset,
            trade_type,
            &values.amount,
            &values.amount_decimal,
            &values.amount_out,
            &values.amount_out_decimal,
            &values.rpc_url,
        )?;

        // 5. Resolve the sender identity (Tempo = `--from-address` only; standard =
        //    OWS-first shared resolver). Errors return before any build/persist.
        let chain_arg = values.chain.as_str();
        let wallet_ref = values.wallet.as_str();
        let from_flag = values.from_address.as_str();

        let mut identity = None;
        let sender = if normalize_swap_provider(&provider_name) == "tempo" {
            super::resolve_swap_plan_sender(&provider_name, wallet_ref, from_flag, || {
                unreachable!("tempo path does not call the standard resolver")
            })?
            .sender
        } else {
            let resolved = resolve_execution_identity(wallet_ref, from_flag, chain_arg)?;
            let sender = resolved.from_address.clone();
            identity = Some(resolved);
            sender
        };

        // 6. Route the build through the populated registry; capture the status.
        let opts = SwapExecutionOptions {
            sender: sender.clone(),
            recipient: values.recipient.clone(),
            slippage_bps: values.slippage_bps,
            simulate: values.simulate,
            rpc_url: values.rpc_url.clone(),
        };
        let built = ctx
            .swap_action_registry()
            .build_swap_action(&provider_name, "plan", req, opts)
            .await;
        // The captured provider status is keyed on the builder display name (Go
        // `provider.Info().Name`), falling back to the normalized provider name.
        let status_name = match &built {
            Ok((_, display)) if !display.trim().is_empty() => display.clone(),
            _ => provider_name.clone(),
        };
        let status = ProviderStatus {
            name: status_name,
            status: super::status_from_quote_result(&built),
            latency_ms: 0,
        };
        let (mut action, _display) = built?;

        // 7. Stamp the identity, persist, and emit the success envelope.
        if let Some(identity) = &identity {
            apply_execution_identity_to_action(&mut action, identity);
        } else {
            // Tempo path: stamp the checksummed sender + the Tempo backend.
            action.from_address = sender;
            action.execution_backend = Some(ExecutionBackend::Tempo);
        }

        let store = ctx.open_action_store()?;
        store
            .save(&action)
            .map_err(|e| Error::wrap(Code::Internal, "persist planned action", e))?;

        let data = serde_json::to_value(&action)
            .map_err(|e| Error::wrap(Code::Internal, "serialize planned action", e))?;
        let mut env = ctx.metadata_envelope("swap plan", data, vec![status]);
        env.warnings = identity.map(|i| i.warnings).unwrap_or_default();
        Ok(env)
    }

    /// The resolved `swap plan` flag values (after structured-input merge).
    struct PlanValues {
        provider: String,
        chain: String,
        from_asset: String,
        to_asset: String,
        trade_type: String,
        amount: String,
        amount_decimal: String,
        amount_out: String,
        amount_out_decimal: String,
        wallet: String,
        from_address: String,
        recipient: String,
        slippage_bps: i64,
        simulate: bool,
        rpc_url: String,
    }

    /// Resolve the `swap plan` flag values, merging any structured input
    /// (`--input-json` / `--input-file`) onto the parsed flags (Go PreRunE
    /// `applyStructuredFlagInput` over `swapPlanArgs`). Explicitly-set flags are
    /// never overridden; an unknown key / null value is a usage error.
    fn resolve_plan_values(args: &PlanArgs) -> Result<PlanValues, Error> {
        use crate::execflags::{
            apply_structured_input, decode_bool_field, decode_i64_field, decode_string_field,
        };

        let mut values = PlanValues {
            provider: args.provider.clone().unwrap_or_default(),
            chain: args.chain.clone().unwrap_or_default(),
            from_asset: args.from_asset.clone().unwrap_or_default(),
            to_asset: args.to_asset.clone().unwrap_or_default(),
            trade_type: args.r#type.clone(),
            amount: args.amount.clone().unwrap_or_default(),
            amount_decimal: args.amount_decimal.clone().unwrap_or_default(),
            amount_out: args.amount_out.clone().unwrap_or_default(),
            amount_out_decimal: args.amount_out_decimal.clone().unwrap_or_default(),
            wallet: args.identity.wallet.clone().unwrap_or_default(),
            from_address: args.identity.from_address.clone().unwrap_or_default(),
            recipient: args.recipient.clone().unwrap_or_default(),
            slippage_bps: args.slippage_bps,
            simulate: args.simulate,
            rpc_url: args.rpc_url.clone().unwrap_or_default(),
        };

        let mut explicit: std::collections::HashSet<&str> = std::collections::HashSet::new();
        if args.provider.is_some() {
            explicit.insert("provider");
        }
        if args.chain.is_some() {
            explicit.insert("chain");
        }
        if args.from_asset.is_some() {
            explicit.insert("from-asset");
        }
        if args.to_asset.is_some() {
            explicit.insert("to-asset");
        }
        if args.r#type != "exact-input" {
            explicit.insert("type");
        }
        if args.amount.is_some() {
            explicit.insert("amount");
        }
        if args.amount_decimal.is_some() {
            explicit.insert("amount-decimal");
        }
        if args.amount_out.is_some() {
            explicit.insert("amount-out");
        }
        if args.amount_out_decimal.is_some() {
            explicit.insert("amount-out-decimal");
        }
        if args.identity.wallet.is_some() {
            explicit.insert("wallet");
        }
        if args.identity.from_address.is_some() {
            explicit.insert("from-address");
        }
        if args.recipient.is_some() {
            explicit.insert("recipient");
        }

        apply_structured_input(
            &args.input,
            &explicit,
            "swap plan",
            |key, canonical, raw| {
                match canonical {
                    "provider" => values.provider = decode_string_field(key, raw)?,
                    "chain" => values.chain = decode_string_field(key, raw)?,
                    "from-asset" => values.from_asset = decode_string_field(key, raw)?,
                    "to-asset" => values.to_asset = decode_string_field(key, raw)?,
                    "type" => values.trade_type = decode_string_field(key, raw)?,
                    "amount" => values.amount = decode_string_field(key, raw)?,
                    "amount-decimal" => values.amount_decimal = decode_string_field(key, raw)?,
                    "amount-out" => values.amount_out = decode_string_field(key, raw)?,
                    "amount-out-decimal" => {
                        values.amount_out_decimal = decode_string_field(key, raw)?
                    }
                    "wallet" => values.wallet = decode_string_field(key, raw)?,
                    "from-address" => values.from_address = decode_string_field(key, raw)?,
                    "recipient" => values.recipient = decode_string_field(key, raw)?,
                    "slippage-bps" => values.slippage_bps = decode_i64_field(key, raw)?,
                    "simulate" => values.simulate = decode_bool_field(key, raw)?,
                    "rpc-url" => values.rpc_url = decode_string_field(key, raw)?,
                    _ => return Ok(false),
                }
                Ok(true)
            },
        )?;

        Ok(values)
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

        let payload = crate::execflags::read_structured_input(input)?;
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

    // --- 8. cache-key cross-binary parity ----------------------------------

    #[test]
    fn cache_key_for_quote_matches_go_alphabetical_map_json() {
        // The Go swap-quote cache key hashes a `map[string]any` via `cacheKey`'s
        // `json.Marshal`, and `encoding/json` emits map keys in ALPHABETICAL
        // order. The Rust `SwapQuoteCacheKey` struct must therefore serialize its
        // fields alphabetically so the resulting key is byte-identical to Go's
        // (cross-binary cache stability). This pins that ordering: if the struct
        // field order drifts away from alphabetical, the canonical JSON — and
        // thus the key — diverges and this assertion fails.
        use defi_id::{Asset, Chain};

        let plan = SwapQuotePlan {
            provider: "1inch".to_string(),
            trade_type: SwapTradeType::ExactInput,
            slippage_pct: None,
            slippage_mode: "auto".to_string(),
            swapper: String::new(),
        };
        let req = SwapQuoteRequest {
            chain: Chain {
                caip2: "eip155:1".to_string(),
                ..Chain::default()
            },
            from_asset: Asset {
                asset_id: "eip155:1/erc20:0xfrom".to_string(),
                ..Asset::default()
            },
            to_asset: Asset {
                asset_id: "eip155:1/erc20:0xto".to_string(),
                ..Asset::default()
            },
            amount_base_units: "1000000".to_string(),
            amount_decimal: "1".to_string(),
            rpc_url: "https://rpc.example".to_string(),
            trade_type: SwapTradeType::ExactInput,
            slippage_pct: None,
            swapper: String::new(),
        };

        let got = cache_key_for_quote("swap quote", &plan, &req);

        // Independent reference: an alphabetically-keyed JSON object (serde
        // serializes a `json!` object in insertion order, so the keys are listed
        // alphabetically here on purpose) run through the documented
        // `hex(sha256(path | "v2" | json))` formula. This mirrors Go's
        // `json.Marshal(map[string]any{...})` (sorted keys).
        let payload = serde_json::json!({
            "amount": "1000000",
            "chain": "eip155:1",
            "from": "eip155:1/erc20:0xfrom",
            "provider": "1inch",
            "rpc_url": "https://rpc.example",
            "slippage_mode": "auto",
            "slippage_pct": serde_json::Value::Null,
            "swapper": "",
            "to": "eip155:1/erc20:0xto",
            "trade_type": "exact-input",
        });
        let canonical = serde_json::to_string(&payload).expect("serialize payload");
        // Sanity-check the reference really is alphabetical (guards the test
        // itself from a mis-ordered literal above).
        assert!(
            canonical.starts_with(r#"{"amount":"#),
            "reference payload must be alphabetical, got: {canonical}"
        );
        let expected = crate::protocols::cache_key("swap quote", &payload);

        assert_eq!(
            got, expected,
            "swap-quote cache key must equal hex(sha256(path | v2 | alphabetical-map-json))"
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

#[cfg(test)]
mod plan_app_tests {
    //! # Success criteria — `defi-app::swap` `swap plan` HANDLER (WS3 exec-plan)
    //!
    //! Go source: `internal/app/runner.go` `newSwapCommand` `planCmd.RunE`
    //! (lines ~1343-1431). These tests drive [`cli::handle`] (the real dispatch
    //! entry the `defi` binary calls) end-to-end for `swap plan` ONLY, asserting
    //! the full machine contract the Go runner emits via `emitSuccess(...)` (a
    //! built+persisted [`Action`] envelope, cache bypassed per spec §2.5) and the
    //! typed-error → full-envelope `renderError(...)` path on every guard.
    //!
    //! The handler is **capability-based** for swap (Go
    //! `s.actionBuilderRegistry().BuildSwapAction(...)`): it routes by `--provider`
    //! to a registered [`defi_execution::builder::SwapActionBuilder`]
    //! (`taikoswap` / `tempo`), persists, then stamps identity. The TWO execution
    //! providers exercise the TWO identity paths:
    //!   * **taikoswap** — standard EVM identity (`--wallet` OWS-first OR
    //!     `--from-address` legacy), via the shared
    //!     [`crate::execident::resolve_execution_identity`] +
    //!     `apply_execution_identity_to_action` (so `execution_backend ==
    //!     legacy_local` and the OWS-recommended warning surface on the
    //!     `--from-address` path); exact-input only.
    //!   * **tempo** — Tempo-only `--from-address` identity (NO shared resolver;
    //!     [`super::resolve_swap_plan_sender`]), where the handler stamps
    //!     `action.from_address = sender` (checksummed) +
    //!     `execution_backend == tempo`; supports exact-output.
    //!
    //! ## Determinism / offline seams
    //!
    //! Both builders connect to RPC through the already-present `--rpc-url` flag
    //! (`PlanArgs.rpc_url`, resolved by `defi_registry::resolve_rpc_url(override,
    //! chain_id)` where the override always wins). TaikoSwap's
    //! `build_swap_action` issues four `quoteExactInputSingle` probes (one per
    //! canonical fee tier) then one `allowance(owner,spender)` read; the mock
    //! reproduces the provider-suite `RpcResponder` (probes return `1000, 2000,
    //! 1500, 500` so the best tier is the 2nd, fee 500, and the 5th `eth_call`
    //! returns the allowance). Tempo's `build_swap_action` issues `currency()`
    //! TIP-20 reads for the USD-pair guard then a `quoteSwapExactAmountIn` /
    //! `quoteSwapExactAmountOut`; the mock reproduces the Tempo provider-suite
    //! `RpcResponder` (selector-routed). All RPC is offline + deterministic.
    //! Identity is exercised through the OFFLINE `--from-address` (legacy / Tempo)
    //! path so no OWS vault / network is touched; the `--wallet` happy path is
    //! WS4b e2e territory and is asserted here only via its offline rejections.
    //!
    //! ## Criteria (each a failing test until `cli::handle` wires `swap plan`)
    //!
    //!  P1. **Plan success envelope (TaikoSwap, legacy `--from-address`).** A valid
    //!      `swap plan --provider taikoswap --chain taiko --from-asset USDC
    //!      --to-asset WETH --amount 1000000 --from-address 0x..aa --rpc-url
    //!      <mock>` (allowance insufficient) returns `Ok(Envelope)` (exit 0) with:
    //!      `version=="v1"`, `success==true`, `error==None`, `meta.partial==false`,
    //!      `meta.command=="swap plan"`,
    //!      `meta.cache=={status:"bypass", age_ms:0, stale:false}` (execution paths
    //!      bypass the cache, spec §2.5), and `meta.providers==[{name:"taikoswap",
    //!      status:"ok"}]` (Go captures one `ProviderStatus` keyed on the builder's
    //!      returned display name with `statusFromErr(nil)=="ok"`).
    //!
    //!  P2. **Planned action `data` shape (TaikoSwap supply).** `env.data` is the
    //!      serialized [`Action`]: `action_id` matches `^act_[0-9a-f]{32}$`;
    //!      `intent_type=="swap"`; `provider=="taikoswap"`; `status=="planned"`;
    //!      `chain_id=="eip155:167000"`; `from_address` == the EIP-55 checksum of
    //!      the sender; `input_amount=="1000000"`. With an INSUFFICIENT allowance
    //!      the action has TWO steps — `[approval, swap]` — where step 0
    //!      `type=="approval"` and step 1 `type=="swap"`, `value=="0"`,
    //!      `chain_id=="eip155:167000"`. (Go `BuildSwapAction` →
    //!      `taikoswap.build_swap_action` + `emitSuccess`.)
    //!
    //!  P3. **TaikoSwap swap-step calldata reuses the alloy/ABI golden.** The swap
    //!      step `target` == the TaikoSwap router (`UNISWAP_V3_ROUTER` for chain
    //!      167000) and `data` starts with the `exactInputSingle` selector
    //!      (computed in-test from the canonical `UNISWAP_V3_ROUTER_ABI`, NOT
    //!      re-encoded by the handler); the approval step `data` starts with the
    //!      ERC-20 `approve` selector `0x095ea7b3`. This proves the handler routes
    //!      through the builder (no re-encoding) and base⇔decimal amounts stay
    //!      consistent (spec §2.4).
    //!
    //!  P4. **TaikoSwap skips the approval step when allowance is sufficient.** The
    //!      same plan against a mock whose `allowance` >= the requested amount
    //!      yields a SINGLE `swap` step (no leading `approval`). (Go
    //!      inline allowance read → no approval.)
    //!
    //!  P5. **Plan persists the action to the Store.** After a successful TaikoSwap
    //!      plan the action is retrievable by its `action_id` from a freshly
    //!      opened [`defi_execution::store::Store`] over the same path, with
    //!      `intent_type=="swap"`, `input_amount=="1000000"`, and
    //!      `provider=="taikoswap"`. (Go `s.actionStore.Save`.)
    //!
    //!  P6. **Legacy-identity warning + backend stamping (TaikoSwap).** The
    //!      `--from-address` path stamps `execution_backend=="legacy_local"` on the
    //!      action AND surfaces the Go warning `--wallet (OWS) is recommended over
    //!      --from-address for planning; see docs for details` in `env.warnings`.
    //!      (Go `resolveExecutionIdentity` legacy branch +
    //!      `emitSuccess(..., identity.Warnings, ...)`.)
    //!
    //!  P7. **Decimal amount parity (TaikoSwap).** `--amount-decimal 1` (no
    //!      `--amount`) on USDC (6 decimals) yields the same `input_amount==
    //!      "1000000"` and the same two-step shape — base⇔decimal stay consistent
    //!      (spec §2.4).
    //!
    //!  P8. **Tempo plan stamps the Tempo backend (exact-input).** A valid `swap
    //!      plan --provider tempo --chain tempo --from-asset pathUSD --to-asset
    //!      USDC.e --amount 1000000 --from-address 0x..aa --rpc-url <mock>` returns
    //!      `Ok(Envelope)` (exit 0) whose action has
    //!      `execution_backend=="tempo"`, `provider=="tempo"`,
    //!      `intent_type=="swap"`, `from_address` == the EIP-55 checksum of the
    //!      sender (Go stamps `action.FromAddress = sender`), and a SINGLE Tempo
    //!      swap step (`tempo-swap-exact-input`). `meta.providers==[{name:"tempo",
    //!      status:"ok"}]`, NO legacy warning (Tempo path surfaces none). (Go
    //!      `planCmd.RunE` tempo branch +
    //!      `action.ExecutionBackend = ExecutionBackendTempo`.)
    //!
    //!  P9. **Tempo exact-output plan.** `swap plan --provider tempo --type
    //!      exact-output --amount-out 1000000 ...` builds a single Tempo
    //!      exact-output swap step (`tempo-swap-exact-output`), still
    //!      `execution_backend=="tempo"`. (Go: exact-output planning supports only
    //!      tempo — `swapProviderSupportsExactOutput`.)
    //!
    //! P10. **`--provider` is required.** `swap plan` with an empty/missing
    //!      `--provider` → [`Code::Usage`] (exit 2) and persists NOTHING. (Go
    //!      `NormalizeSwapProvider("")=="" → --provider is required`.)
    //!
    //! P11. **Unknown / quote-only swap provider → unsupported.** `--provider
    //!      bogus` and a markets/quote-only provider like `--provider 1inch` (no
    //!      execution builder registered) → [`Code::Unsupported`] (exit 13);
    //!      persists NOTHING. (Go `BuildSwapAction`: unknown → unsupported,
    //!      quote-only → `provider X does not support swap planning`.)
    //!
    //! P12. **Exact-output capability gate (TaikoSwap).** `swap plan --provider
    //!      taikoswap --type exact-output --amount-out 1000000 ...` →
    //!      [`Code::Unsupported`] (exit 13) with the Go message `exact-output swap
    //!      planning currently supports only --provider tempo`, BEFORE any
    //!      build/persist. Persists NOTHING. (Go gate
    //!      `swapProviderSupportsExactOutput`.)
    //!
    //! P13. **`--type` enum validation.** An invalid `--type limit-order` →
    //!      [`Code::Usage`] (exit 2) (Go `normalizeTradeType`). Persists NOTHING.
    //!
    //! P14. **TaikoSwap identity-constraint errors (offline).**
    //!      (a) BOTH `--wallet` and `--from-address` → [`Code::Usage`] (exit 2);
    //!      (b) NEITHER `--wallet` nor `--from-address` → [`Code::Usage`] (exit 2);
    //!      (c) a malformed `--from-address` → [`Code::Usage`] (exit 2).
    //!      (Go `resolveExecutionIdentity`.) On every error the handler returns the
    //!      typed `Err(Error)` (the runner renders the full error envelope to
    //!      stderr, spec §2.1) and persists NOTHING.
    //!
    //! P15. **Tempo identity-constraint errors (offline).**
    //!      (a) `--wallet` on a Tempo plan → [`Code::Unsupported`] (exit 13) with
    //!          `--wallet planning is not supported on Tempo chains yet`;
    //!      (b) BOTH `--wallet` and `--from-address` → [`Code::Usage`] (exit 2);
    //!      (c) NEITHER → [`Code::Usage`] (exit 2)
    //!          (`--from-address is required for --provider tempo`);
    //!      (d) a malformed `--from-address` → [`Code::Usage`] (exit 2).
    //!      (Go `planCmd.RunE` tempo branch.) Persists NOTHING.
    //!
    //! P16. **Full-binary exit codes.** Through `run_with_args` (the real binary
    //!      path with no env): `swap plan` with no `--provider` → exit 2; a missing
    //!      identity input on `--provider taikoswap` → exit 2. (Confirms the wired
    //!      dispatch + clap surface, not just the in-process handler.)
    //!
    //! SKIPPED (covered elsewhere / wrong unit):
    //!   * the TaikoSwap/Tempo best-fee selection, slippage math, USD-pair gating,
    //!     and exact ABI tuple encoding internals — owned by the
    //!     `defi-providers::{taikoswap,tempo}` RED suites (ported from
    //!     `client_test.go`);
    //!   * the `build_swap_action` registry routing itself — `defi-execution::
    //!     builder` suite;
    //!   * the pure pre-provider helpers (`normalize_trade_type`,
    //!     `swap_provider_supports_exact_output`, `parse_swap_request`,
    //!     `resolve_swap_plan_sender`, `swap_plan_identity_constraints`) — the
    //!     sibling `tests` module;
    //!   * the OWS `--wallet` happy-path resolve + wallet-id persistence — WS4b
    //!     e2e (here only its offline guard rejections are asserted);
    //!   * `--input-json`/`--input-file` precedence — structured-input unit;
    //!   * `swap submit|status` — WS4;
    //!   * the JSON field-declaration-order rendering — `defi-out` golden tests.

    use super::cli::{handle, PlanArgs, SwapCmd};
    use crate::cli::run_with_args;
    use crate::ctx::AppCtx;
    use crate::execflags::{InputFlags, PlanIdentityFlags};
    use defi_config::{MapEnv, Settings};
    use defi_errors::{exit_code, Code, Error};
    use defi_execution::store::Store as ActionStore;
    use defi_model::Envelope;
    use serde_json::{json, Value};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use alloy::dyn_abi::{FunctionExt, JsonAbiExt};
    use alloy::json_abi::{Function as JsonFunction, JsonAbi};
    use alloy::primitives::U256;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    // --- contract constants -------------------------------------------------

    /// Sender EOA (legacy / Tempo `--from-address` identity); its EIP-55 checksum
    /// lands on the action.
    const SENDER: &str = "0x00000000000000000000000000000000000000aa";
    /// A second address used only for the both-identity-inputs rejection.
    const OTHER: &str = "0x00000000000000000000000000000000000000bb";
    /// TaikoSwap V3 router for chain 167000 (from `defi_registry::uniswap_v3_contracts`).
    /// The swap step must target this address.
    const TAIKO_ROUTER: &str = "0x";
    /// The Go legacy-identity warning surfaced when planning with `--from-address`.
    const LEGACY_WARNING: &str =
        "--wallet (OWS) is recommended over --from-address for planning; see docs for details";

    // --- harness ------------------------------------------------------------

    /// Execution settings with a real action store under `dir` and the cache
    /// disabled (execution paths bypass the cache anyway, spec §2.5).
    fn exec_settings(dir: &Path) -> Settings {
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
            cache_enabled: false,
            cache_path: dir.join("cache.db"),
            cache_lock_path: dir.join("cache.lock"),
            action_store_path: dir.join("actions.db"),
            action_lock_path: dir.join("actions.lock"),
            defillama_api_key: String::new(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    /// A TaikoSwap `PlanArgs` with the canonical happy-path values; mutate per
    /// test. `--from-address` (legacy) identity; exact-input USDC→WETH on taiko.
    fn taikoswap_args(rpc: &str) -> PlanArgs {
        PlanArgs {
            chain: Some("taiko".to_string()),
            from_asset: Some("USDC".to_string()),
            to_asset: Some("WETH".to_string()),
            provider: Some("taikoswap".to_string()),
            r#type: "exact-input".to_string(),
            amount: Some("1000000".to_string()),
            amount_decimal: None,
            amount_out: None,
            amount_out_decimal: None,
            recipient: None,
            slippage_bps: 50,
            rpc_url: Some(rpc.to_string()),
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(SENDER.to_string()),
            },
            input: InputFlags::default(),
        }
    }

    /// A Tempo `PlanArgs` (exact-input pathUSD→USDC.e on the tempo chain) with the
    /// Tempo-only `--from-address` identity.
    fn tempo_args(rpc: &str) -> PlanArgs {
        PlanArgs {
            chain: Some("tempo".to_string()),
            from_asset: Some("pathUSD".to_string()),
            to_asset: Some("USDC.e".to_string()),
            provider: Some("tempo".to_string()),
            r#type: "exact-input".to_string(),
            amount: Some("1000000".to_string()),
            amount_decimal: None,
            amount_out: None,
            amount_out_decimal: None,
            recipient: None,
            slippage_bps: 50,
            rpc_url: Some(rpc.to_string()),
            simulate: true,
            identity: PlanIdentityFlags {
                wallet: None,
                from_address: Some(SENDER.to_string()),
            },
            input: InputFlags::default(),
        }
    }

    async fn run_plan(dir: &Path, args: PlanArgs) -> Result<Envelope, Error> {
        let ctx = AppCtx::new(exec_settings(dir));
        handle(&ctx, SwapCmd::Plan(args)).await
    }

    fn usage_exit(err: &Error) -> i32 {
        exit_code(&Err(Error::new(err.code, "")))
    }

    fn action_data(env: &Envelope) -> Value {
        env.data.clone().expect("plan envelope carries `data`")
    }

    /// True iff no action is persisted under `dir` (error paths must persist
    /// nothing). A never-created store counts as empty.
    fn no_actions_persisted(dir: &Path) -> bool {
        let store = match ActionStore::open(dir.join("actions.db"), dir.join("actions.lock")) {
            Ok(store) => store,
            Err(_) => return true,
        };
        store
            .list("", 1000)
            .map(|actions| actions.is_empty())
            .unwrap_or(true)
    }

    fn env_with_home() -> (MapEnv, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let env = MapEnv::with_home(tmp.path().to_path_buf());
        (env, tmp)
    }

    // --- abi helpers (in-test goldens) -------------------------------------

    fn json_function(abi_json: &str, name: &str) -> JsonFunction {
        let abi: JsonAbi = serde_json::from_str(abi_json).expect("parse abi");
        abi.function(name)
            .and_then(|o| o.first())
            .cloned()
            .expect("function present")
    }

    fn selector_hex(abi_json: &str, name: &str) -> String {
        format!(
            "0x{}",
            hex::encode(json_function(abi_json, name).selector().0)
        )
    }

    /// The TaikoSwap V3 router address for chain 167000, EIP-55 checksummed
    /// (the swap step target). Read from the canonical registry so the test does
    /// not hardcode a possibly-stale literal.
    fn taiko_router_checksum() -> String {
        let (_quoter, router) =
            defi_registry::uniswap_v3_contracts(167000).expect("taiko v3 contracts");
        defi_evm::address::checksum(router).expect("checksum router")
    }

    // --- wiremock JSON-RPC: TaikoSwap quoter probes + allowance ------------

    /// A `wiremock` responder reproducing the TaikoSwap provider-suite mock:
    /// counts `eth_call`s, returns quoter outputs `1000, 2000, 1500, 500` for the
    /// four fee-tier probes (best = 2nd, fee 500), and on the 5th call (the
    /// allowance read) returns `allowance`.
    struct TaikoRpcResponder {
        allowance: u128,
        call_count: AtomicUsize,
        quoter_fn: JsonFunction,
        allowance_fn: JsonFunction,
    }

    impl TaikoRpcResponder {
        fn new(allowance: u128) -> Self {
            TaikoRpcResponder {
                allowance,
                call_count: AtomicUsize::new(0),
                quoter_fn: json_function(
                    defi_registry::UNISWAP_V3_QUOTER_V2_ABI,
                    "quoteExactInputSingle",
                ),
                allowance_fn: json_function(defi_registry::ERC20_MINIMAL_ABI, "allowance"),
            }
        }

        fn pack_output(func: &JsonFunction, values: &[alloy::dyn_abi::DynSolValue]) -> String {
            let bytes = func.abi_encode_output(values).expect("pack output");
            format!("0x{}", hex::encode(bytes))
        }
    }

    impl Respond for TaikoRpcResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            use alloy::dyn_abi::DynSolValue;
            let body: Value = match serde_json::from_slice(&request.body) {
                Ok(v) => v,
                Err(_) => return ResponseTemplate::new(400),
            };
            let id = body.get("id").cloned().unwrap_or(json!(1));
            let method_name = body.get("method").and_then(Value::as_str).unwrap_or("");
            if method_name != "eth_call" {
                return rpc_error(&id, -32601, "method not supported in test");
            }
            let index = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
            if index == 5 {
                return rpc_result(
                    &id,
                    &Self::pack_output(
                        &self.allowance_fn,
                        &[DynSolValue::Uint(U256::from(self.allowance), 256)],
                    ),
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
                        DynSolValue::Uint(U256::from(amount_out), 256),
                        DynSolValue::Uint(U256::ZERO, 160),
                        DynSolValue::Uint(U256::ZERO, 32),
                        DynSolValue::Uint(U256::from(70_000u64), 256),
                    ],
                ),
            )
        }
    }

    async fn taiko_rpc(allowance: u128) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(TaikoRpcResponder::new(allowance))
            .mount(&server)
            .await;
        server
    }

    // --- wiremock JSON-RPC: Tempo currency + quote + allowance -------------

    /// A `wiremock` responder reproducing the Tempo provider-suite mock:
    /// selector-routed `currency()` (USD for the canonical tokens),
    /// `quoteSwapExactAmountIn` / `quoteSwapExactAmountOut`, and `allowance`.
    struct TempoRpcResponder {
        allowance: u128,
        quote_in: u128,
        quote_out: u128,
        currency_sel: String,
        quote_in_sel: String,
        quote_out_sel: String,
        allowance_sel: String,
        currency_fn: JsonFunction,
        quote_in_fn: JsonFunction,
        quote_out_fn: JsonFunction,
        allowance_fn: JsonFunction,
    }

    impl TempoRpcResponder {
        fn new(allowance: u128) -> Self {
            let dex_abi = defi_registry::TEMPO_STABLECOIN_DEX_ABI;
            let erc20_abi = defi_registry::ERC20_MINIMAL_ABI;
            let tip20_abi = defi_registry::TEMPO_TIP20_METADATA_ABI;
            TempoRpcResponder {
                allowance,
                quote_in: 980_000,
                quote_out: 1_010_100,
                currency_sel: raw_selector(tip20_abi, "currency"),
                quote_in_sel: raw_selector(dex_abi, "quoteSwapExactAmountIn"),
                quote_out_sel: raw_selector(dex_abi, "quoteSwapExactAmountOut"),
                allowance_sel: raw_selector(erc20_abi, "allowance"),
                currency_fn: json_function(tip20_abi, "currency"),
                quote_in_fn: json_function(dex_abi, "quoteSwapExactAmountIn"),
                quote_out_fn: json_function(dex_abi, "quoteSwapExactAmountOut"),
                allowance_fn: json_function(erc20_abi, "allowance"),
            }
        }

        fn pack_output(func: &JsonFunction, values: &[alloy::dyn_abi::DynSolValue]) -> String {
            let bytes = func.abi_encode_output(values).expect("pack output");
            format!("0x{}", hex::encode(bytes))
        }
    }

    fn raw_selector(abi_json: &str, name: &str) -> String {
        hex::encode(json_function(abi_json, name).selector().0)
    }

    /// Tempo USD token currency lookup (subset of the provider-suite mock).
    fn token_currency(token: &str) -> Option<&'static str> {
        match token.to_ascii_lowercase().as_str() {
            "0x20c0000000000000000000000000000000000000" => Some("USD"), // pathUSD
            "0x20c000000000000000000000b9537d11c60e8b50" => Some("USD"), // USDC.e
            "0x20c00000000000000000000014f22ca97301eb73" => Some("USD"), // USDT0
            _ => None,
        }
    }

    impl Respond for TempoRpcResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            use alloy::dyn_abi::DynSolValue;
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
                return rpc_result(
                    &id,
                    &Self::pack_output(
                        &self.quote_in_fn,
                        &[DynSolValue::Uint(U256::from(self.quote_in), 128)],
                    ),
                );
            }
            if selector == self.quote_out_sel {
                return rpc_result(
                    &id,
                    &Self::pack_output(
                        &self.quote_out_fn,
                        &[DynSolValue::Uint(U256::from(self.quote_out), 128)],
                    ),
                );
            }
            if selector == self.allowance_sel {
                return rpc_result(
                    &id,
                    &Self::pack_output(
                        &self.allowance_fn,
                        &[DynSolValue::Uint(U256::from(self.allowance), 256)],
                    ),
                );
            }
            rpc_error(&id, -32601, "unsupported eth_call data")
        }
    }

    async fn tempo_rpc(allowance: u128) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(TempoRpcResponder::new(allowance))
            .mount(&server)
            .await;
        server
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

    // ---- P1: success envelope (TaikoSwap, legacy --from-address) ----------

    #[tokio::test(flavor = "multi_thread")]
    async fn taikoswap_plan_emits_success_envelope() {
        let server = taiko_rpc(0).await; // insufficient allowance -> approval added
        let dir = tempfile::tempdir().expect("tempdir");
        let env = run_plan(dir.path(), taikoswap_args(&server.uri()))
            .await
            .expect("taikoswap plan should succeed against the mock RPC");

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert_eq!(env.meta.command, "swap plan");
        assert!(!env.meta.partial);

        // Execution paths bypass the cache (spec §2.5).
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);

        // One provider status row keyed on the builder display name, status ok.
        assert_eq!(env.meta.providers.len(), 1, "exactly one provider status");
        assert_eq!(env.meta.providers[0].name, "taikoswap");
        assert_eq!(env.meta.providers[0].status, "ok");
    }

    // ---- P2: planned action data shape ------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn taikoswap_plan_action_shape() {
        let server = taiko_rpc(0).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let env = run_plan(dir.path(), taikoswap_args(&server.uri()))
            .await
            .expect("plan");
        let data = action_data(&env);

        let action_id = data["action_id"].as_str().expect("action_id");
        assert!(
            action_id.starts_with("act_") && action_id.len() == 36,
            "action_id must be act_ + 32 hex: {action_id}"
        );
        assert!(
            action_id[4..].chars().all(|c| c.is_ascii_hexdigit()),
            "action_id suffix must be hex: {action_id}"
        );
        assert_eq!(data["intent_type"], json!("swap"));
        assert_eq!(data["provider"], json!("taikoswap"));
        assert_eq!(data["status"], json!("planned"));
        assert_eq!(data["chain_id"], json!("eip155:167000"));
        assert_eq!(
            data["from_address"],
            json!(defi_evm::address::checksum(SENDER).unwrap())
        );
        assert_eq!(data["input_amount"], json!("1000000"));

        let steps = data["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 2, "insufficient allowance -> [approval, swap]");
        assert_eq!(steps[0]["type"], json!("approval"));
        assert_eq!(steps[1]["type"], json!("swap"));
        assert_eq!(steps[1]["value"], json!("0"));
        assert_eq!(steps[1]["chain_id"], json!("eip155:167000"));
    }

    // ---- P3: swap-step calldata reuses the alloy/ABI golden ---------------

    #[tokio::test(flavor = "multi_thread")]
    async fn taikoswap_plan_swap_step_calldata_golden() {
        let server = taiko_rpc(0).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let env = run_plan(dir.path(), taikoswap_args(&server.uri()))
            .await
            .expect("plan");
        let data = action_data(&env);
        let steps = data["steps"].as_array().expect("steps");

        // Approval step: ERC-20 approve selector.
        assert!(
            steps[0]["data"].as_str().unwrap().starts_with("0x095ea7b3"),
            "approval step must be an ERC-20 approve: {}",
            steps[0]["data"]
        );

        // Swap step targets the canonical TaikoSwap router and encodes
        // exactInputSingle (selector from the canonical router ABI golden).
        assert_eq!(
            steps[1]["target"].as_str().unwrap().to_lowercase(),
            taiko_router_checksum().to_lowercase(),
            "swap step must target the TaikoSwap router"
        );
        let want_sel = selector_hex(defi_registry::UNISWAP_V3_ROUTER_ABI, "exactInputSingle");
        assert!(
            steps[1]["data"].as_str().unwrap().starts_with(&want_sel),
            "swap step calldata must be exactInputSingle ({want_sel}): {}",
            steps[1]["data"]
        );
        // Keep TAIKO_ROUTER referenced so the placeholder const documents intent.
        let _ = TAIKO_ROUTER;
    }

    // ---- P4: approval skipped when allowance sufficient -------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn taikoswap_plan_skips_approval_when_allowance_sufficient() {
        let server = taiko_rpc(u128::MAX).await; // allowance >= amount
        let dir = tempfile::tempdir().expect("tempdir");
        let env = run_plan(dir.path(), taikoswap_args(&server.uri()))
            .await
            .expect("plan");
        let data = action_data(&env);
        let steps = data["steps"].as_array().expect("steps");
        assert_eq!(steps.len(), 1, "sufficient allowance -> single swap step");
        assert_eq!(steps[0]["type"], json!("swap"));
    }

    // ---- P5: persists action to the store ---------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn taikoswap_plan_persists_action() {
        let server = taiko_rpc(0).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let env = run_plan(dir.path(), taikoswap_args(&server.uri()))
            .await
            .expect("plan");
        let id = action_data(&env)["action_id"].as_str().unwrap().to_string();

        let store = ActionStore::open(
            dir.path().join("actions.db"),
            dir.path().join("actions.lock"),
        )
        .expect("open store");
        let persisted = store.get(&id).expect("persisted action retrievable");
        assert_eq!(persisted.intent_type, "swap");
        assert_eq!(persisted.input_amount, "1000000");
        assert_eq!(persisted.provider, "taikoswap");
    }

    // ---- P6: legacy warning + backend stamping (TaikoSwap) ----------------

    #[tokio::test(flavor = "multi_thread")]
    async fn taikoswap_plan_legacy_warning_and_backend() {
        let server = taiko_rpc(0).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let env = run_plan(dir.path(), taikoswap_args(&server.uri()))
            .await
            .expect("plan");
        let data = action_data(&env);
        assert_eq!(
            data["execution_backend"],
            json!("legacy_local"),
            "--from-address path stamps the legacy backend"
        );
        assert!(
            env.warnings.iter().any(|w| w == LEGACY_WARNING),
            "the OWS-recommended legacy warning must surface: {:?}",
            env.warnings
        );
    }

    // ---- P7: decimal amount parity ----------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn taikoswap_plan_decimal_amount_parity() {
        let server = taiko_rpc(0).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = taikoswap_args(&server.uri());
        args.amount = None;
        args.amount_decimal = Some("1".to_string()); // USDC has 6 decimals
        let env = run_plan(dir.path(), args).await.expect("plan");
        let data = action_data(&env);
        assert_eq!(data["input_amount"], json!("1000000"));
        assert_eq!(data["steps"].as_array().unwrap().len(), 2);
    }

    // ---- P8: Tempo plan stamps the tempo backend (exact-input) ------------

    #[tokio::test(flavor = "multi_thread")]
    async fn tempo_plan_stamps_tempo_backend() {
        let server = tempo_rpc(0).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let env = run_plan(dir.path(), tempo_args(&server.uri()))
            .await
            .expect("tempo plan should succeed against the mock RPC");

        assert_eq!(env.meta.command, "swap plan");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.providers.len(), 1);
        assert_eq!(env.meta.providers[0].name, "tempo");
        assert_eq!(env.meta.providers[0].status, "ok");
        // Tempo path surfaces no legacy warning.
        assert!(
            !env.warnings.iter().any(|w| w == LEGACY_WARNING),
            "tempo plan must not surface the legacy warning: {:?}",
            env.warnings
        );

        let data = action_data(&env);
        assert_eq!(data["intent_type"], json!("swap"));
        assert_eq!(data["provider"], json!("tempo"));
        assert_eq!(
            data["execution_backend"],
            json!("tempo"),
            "tempo plan stamps execution_backend = tempo"
        );
        assert_eq!(
            data["from_address"],
            json!(defi_evm::address::checksum(SENDER).unwrap()),
            "handler stamps the checksummed sender on the tempo action"
        );
        let steps = data["steps"].as_array().expect("steps");
        assert_eq!(steps.len(), 1, "tempo emits a single swap step");
        assert_eq!(steps[0]["type"], json!("swap"));
    }

    // ---- P9: Tempo exact-output plan --------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn tempo_plan_exact_output() {
        let server = tempo_rpc(0).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = tempo_args(&server.uri());
        args.r#type = "exact-output".to_string();
        args.amount = None;
        args.amount_out = Some("1000000".to_string());
        let env = run_plan(dir.path(), args)
            .await
            .expect("tempo exact-output plan");
        let data = action_data(&env);
        assert_eq!(data["execution_backend"], json!("tempo"));
        let steps = data["steps"].as_array().expect("steps");
        assert_eq!(steps.len(), 1, "tempo exact-output is a single swap step");
        assert_eq!(steps[0]["step_id"], json!("tempo-swap-exact-output"));
    }

    // ---- P10: --provider required -----------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_requires_provider() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = taikoswap_args("http://127.0.0.1:1");
        args.provider = None;
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("missing --provider must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(dir.path()));
    }

    // ---- P11: unknown / quote-only provider -> unsupported ----------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_unknown_provider_unsupported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = taikoswap_args("http://127.0.0.1:1");
        args.provider = Some("bogus".to_string());
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("unknown provider must fail");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(usage_exit(&err), 13);
        // Message asserts the SPECIFIC Go BuildSwapAction guard (not the
        // unimplemented stub, which also returns Unsupported).
        assert!(
            err.to_string().contains("unsupported swap provider"),
            "expected the Go unknown-provider message, got: {err}"
        );
        assert!(no_actions_persisted(dir.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_quote_only_provider_unsupported() {
        // 1inch is a registered swap *quote* provider but has no execution
        // builder; Go BuildSwapAction -> "provider 1inch does not support swap
        // planning".
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = taikoswap_args("http://127.0.0.1:1");
        args.provider = Some("1inch".to_string());
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("quote-only provider must fail planning");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(usage_exit(&err), 13);
        // Message asserts the SPECIFIC Go quote-only guard (not the unimplemented
        // stub, which also returns Unsupported).
        assert!(
            err.to_string().contains("does not support swap planning"),
            "expected the Go quote-only planning message, got: {err}"
        );
        assert!(no_actions_persisted(dir.path()));
    }

    // ---- P12: exact-output capability gate (TaikoSwap) --------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_exact_output_on_taikoswap_unsupported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = taikoswap_args("http://127.0.0.1:1");
        args.r#type = "exact-output".to_string();
        args.amount = None;
        args.amount_out = Some("1000000".to_string());
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("exact-output on taikoswap must fail");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(usage_exit(&err), 13);
        assert!(
            err.to_string()
                .contains("exact-output swap planning currently supports only --provider tempo"),
            "expected the Go exact-output gate message, got: {err}"
        );
        assert!(no_actions_persisted(dir.path()));
    }

    // ---- P13: --type enum validation --------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_invalid_type_is_usage() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = taikoswap_args("http://127.0.0.1:1");
        args.r#type = "limit-order".to_string();
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("invalid --type must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(dir.path()));
    }

    // ---- P14: TaikoSwap identity-constraint errors ------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn taikoswap_plan_rejects_both_identity_inputs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = taikoswap_args("http://127.0.0.1:1");
        args.identity = PlanIdentityFlags {
            wallet: Some("alice".to_string()),
            from_address: Some(SENDER.to_string()),
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("both identity inputs must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(dir.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn taikoswap_plan_rejects_missing_identity_inputs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = taikoswap_args("http://127.0.0.1:1");
        args.identity = PlanIdentityFlags {
            wallet: None,
            from_address: None,
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("missing identity must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(dir.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn taikoswap_plan_rejects_malformed_from_address() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = taikoswap_args("http://127.0.0.1:1");
        args.identity = PlanIdentityFlags {
            wallet: None,
            from_address: Some("0xnot-an-address".to_string()),
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("malformed --from-address must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(dir.path()));
    }

    // ---- P15: Tempo identity-constraint errors ----------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn tempo_plan_rejects_wallet() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = tempo_args("http://127.0.0.1:1");
        args.identity = PlanIdentityFlags {
            wallet: Some("alice".to_string()),
            from_address: None,
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("--wallet on tempo must fail");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(usage_exit(&err), 13);
        assert!(
            err.to_string()
                .contains("--wallet planning is not supported on Tempo chains yet"),
            "expected the Go tempo-wallet rejection, got: {err}"
        );
        assert!(no_actions_persisted(dir.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tempo_plan_rejects_both_identity_inputs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = tempo_args("http://127.0.0.1:1");
        args.identity = PlanIdentityFlags {
            wallet: Some("alice".to_string()),
            from_address: Some(OTHER.to_string()),
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("both identity inputs on tempo must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(dir.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tempo_plan_rejects_missing_from_address() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = tempo_args("http://127.0.0.1:1");
        args.identity = PlanIdentityFlags {
            wallet: None,
            from_address: None,
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("missing --from-address on tempo must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(
            err.to_string()
                .contains("--from-address is required for --provider tempo"),
            "expected the Go tempo from-address requirement, got: {err}"
        );
        assert!(no_actions_persisted(dir.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tempo_plan_rejects_malformed_from_address() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut args = tempo_args("http://127.0.0.1:1");
        args.identity = PlanIdentityFlags {
            wallet: None,
            from_address: Some("0xnope".to_string()),
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("malformed --from-address on tempo must fail");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert!(no_actions_persisted(dir.path()));
    }

    // ---- structured input (`--input-json` / `--input-file`) ---------------
    //
    // Go: `configureStructuredInput[swapPlanArgs]` wires the PreRunE merge onto
    // `swap plan`. JSON fills flags; explicit flags override JSON; unknown keys /
    // null values are usage errors that persist nothing.

    #[tokio::test(flavor = "multi_thread")]
    async fn taikoswap_plan_resolves_all_flags_from_input_json() {
        let server = taiko_rpc(0).await;
        let dir = tempfile::tempdir().expect("tempdir");
        // No explicit flags: everything arrives via structured input.
        let args = PlanArgs {
            input: InputFlags {
                input_json: Some(format!(
                    r#"{{"provider":"taikoswap","chain":"taiko","from_asset":"USDC","to_asset":"WETH","amount":"1000000","from_address":"{SENDER}","rpc_url":"{rpc}"}}"#,
                    rpc = server.uri()
                )),
                input_file: None,
            },
            ..PlanArgs::default()
        };
        let env = run_plan(dir.path(), args)
            .await
            .expect("input-json should fill all flags and the plan should succeed");
        assert!(env.success);
        assert_eq!(env.meta.command, "swap plan");
        assert_eq!(env.meta.providers[0].name, "taikoswap");
        let data = action_data(&env);
        assert_eq!(data["intent_type"], json!("swap"));
        assert_eq!(data["provider"], json!("taikoswap"));
        assert_eq!(data["chain_id"], json!("eip155:167000"));
        assert_eq!(data["input_amount"], json!("1000000"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn swap_plan_input_json_unknown_field_is_usage_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = PlanArgs {
            input: InputFlags {
                input_json: Some(r#"{"provider":"taikoswap","bogus":"x"}"#.to_string()),
                input_file: None,
            },
            ..PlanArgs::default()
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("unknown structured-input field must be a usage error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(usage_exit(&err), 2);
        assert_eq!(
            err.message,
            "structured input field \"bogus\" is not supported by swap plan"
        );
        assert!(no_actions_persisted(dir.path()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn swap_plan_input_json_number_for_string_flag_is_usage_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = PlanArgs {
            input: InputFlags {
                input_json: Some(format!(
                    r#"{{"provider":"taikoswap","chain":"taiko","from_asset":"USDC","to_asset":"WETH","amount":1000000,"from_address":"{SENDER}"}}"#
                )),
                input_file: None,
            },
            ..PlanArgs::default()
        };
        let err = run_plan(dir.path(), args)
            .await
            .expect_err("a JSON number for a string flag must be a usage error");
        assert_eq!(err.code, Code::Usage);
        assert!(
            err.message
                .starts_with("decode structured input field \"amount\""),
            "got {:?}",
            err.message
        );
        assert!(no_actions_persisted(dir.path()));
    }

    // ---- P16: full-binary exit codes --------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_missing_provider_full_binary_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "swap",
                "plan",
                "--chain",
                "taiko",
                "--from-asset",
                "USDC",
                "--to-asset",
                "WETH",
                "--amount",
                "1000000",
                "--from-address",
                SENDER,
            ],
            &env,
        )
        .await;
        assert_eq!(code, 2, "missing --provider must be a usage error (exit 2)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plan_missing_identity_full_binary_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "swap",
                "plan",
                "--provider",
                "taikoswap",
                "--chain",
                "taiko",
                "--from-asset",
                "USDC",
                "--to-asset",
                "WETH",
                "--amount",
                "1000000",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "missing identity input on taikoswap plan must be a usage error (exit 2)"
        );
    }
}
