//! Shared clap flag groups for execution commands.
//!
//! The execution `submit` / `status` flag sets are (nearly) uniform across the
//! `swap` / `bridge` / `lend` / `yield` / `rewards` / `approvals` / `transfer`
//! groups, so they are defined once here and flattened into each group's
//! subcommand structs. Keeping a single definition guarantees the schema tree
//! (WS6) and the runtime parser stay aligned, matching the Go execution flag
//! surface (`internal/app/runner.go` execution command builders).

use clap::Args;

/// Structured-input flags shared by every `plan` / `submit` command
/// (`--input-json` / `--input-file`; explicit flags override these values).
#[derive(Args, Debug, Clone, Default)]
pub struct InputFlags {
    /// Structured request JSON.
    #[arg(long = "input-json")]
    pub input_json: Option<String>,
    /// Path to structured request JSON file ('-' for stdin).
    #[arg(long = "input-file")]
    pub input_file: Option<String>,
}

/// Identity flags shared by every `plan` command: OWS-first `--wallet`, local
/// signer `--from-address`. (`bridge`/`swap`/`lend`/`yield`/`rewards` plans.)
#[derive(Args, Debug, Clone, Default)]
pub struct PlanIdentityFlags {
    /// Wallet identifier or name (OWS-first identity input).
    #[arg(long)]
    pub wallet: Option<String>,
    /// Sender EOA address (local signer identity input).
    #[arg(long = "from-address")]
    pub from_address: Option<String>,
}

/// The full `submit` flag surface shared by `swap`/`bridge`/`lend`/`yield`/
/// `rewards`/`approvals` submit (transfer submit omits the approval/provider-tx
/// guardrail flags — see [`TransferSubmitArgs`]).
#[derive(Args, Debug, Clone, Default)]
pub struct SubmitArgs {
    /// Action identifier returned by the corresponding plan command.
    #[arg(long = "action-id")]
    pub action_id: Option<String>,
    /// Expected sender EOA address.
    #[arg(long = "from-address")]
    pub from_address: Option<String>,
    /// Allow approval amounts greater than planned input amount.
    #[arg(long = "allow-max-approval")]
    pub allow_max_approval: bool,
    /// Bypass provider transaction guardrails for bridge/aggregator payloads.
    #[arg(long = "unsafe-provider-tx")]
    pub unsafe_provider_tx: bool,
    /// Signer backend (local|tempo).
    #[arg(long, default_value = "local")]
    pub signer: String,
    /// Key source (auto|env|file|keystore).
    #[arg(long = "key-source", default_value = "auto")]
    pub key_source: String,
    /// Private key hex override for local signer (less safe).
    #[arg(long = "private-key")]
    pub private_key: Option<String>,
    /// Fee token address for Tempo chains (defaults to chain USDC.e).
    #[arg(long = "fee-token")]
    pub fee_token: Option<String>,
    /// Gas estimate safety multiplier.
    #[arg(long = "gas-multiplier", default_value_t = 1.2)]
    pub gas_multiplier: f64,
    /// Optional EIP-1559 max fee (gwei).
    #[arg(long = "max-fee-gwei")]
    pub max_fee_gwei: Option<String>,
    /// Optional EIP-1559 max priority fee (gwei).
    #[arg(long = "max-priority-fee-gwei")]
    pub max_priority_fee_gwei: Option<String>,
    /// Run preflight simulation before submission.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub simulate: bool,
    /// Receipt polling interval.
    #[arg(long = "poll-interval", default_value = "2s")]
    pub poll_interval: String,
    /// Per-step receipt timeout.
    #[arg(long = "step-timeout", default_value = "2m")]
    pub step_timeout: String,
    #[command(flatten)]
    pub input: InputFlags,
}

/// The `transfer submit` flag surface (no approval/provider-tx guardrails).
#[derive(Args, Debug, Clone, Default)]
pub struct TransferSubmitArgs {
    /// Action identifier returned by transfer plan.
    #[arg(long = "action-id")]
    pub action_id: Option<String>,
    /// Expected sender EOA address.
    #[arg(long = "from-address")]
    pub from_address: Option<String>,
    /// Signer backend (local|tempo).
    #[arg(long, default_value = "local")]
    pub signer: String,
    /// Key source (auto|env|file|keystore).
    #[arg(long = "key-source", default_value = "auto")]
    pub key_source: String,
    /// Private key hex override for local signer (less safe).
    #[arg(long = "private-key")]
    pub private_key: Option<String>,
    /// Fee token address for Tempo chains (defaults to chain USDC.e).
    #[arg(long = "fee-token")]
    pub fee_token: Option<String>,
    /// Gas estimate safety multiplier.
    #[arg(long = "gas-multiplier", default_value_t = 1.2)]
    pub gas_multiplier: f64,
    /// Optional EIP-1559 max fee (gwei).
    #[arg(long = "max-fee-gwei")]
    pub max_fee_gwei: Option<String>,
    /// Optional EIP-1559 max priority fee (gwei).
    #[arg(long = "max-priority-fee-gwei")]
    pub max_priority_fee_gwei: Option<String>,
    /// Run preflight simulation before submission.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub simulate: bool,
    /// Receipt polling interval.
    #[arg(long = "poll-interval", default_value = "2s")]
    pub poll_interval: String,
    /// Per-step receipt timeout.
    #[arg(long = "step-timeout", default_value = "2m")]
    pub step_timeout: String,
    #[command(flatten)]
    pub input: InputFlags,
}

/// The `status` flag surface shared by every execution group (`--action-id`).
#[derive(Args, Debug, Clone, Default)]
pub struct StatusArgs {
    /// Action identifier returned by the corresponding plan command.
    #[arg(long = "action-id")]
    pub action_id: Option<String>,
}
