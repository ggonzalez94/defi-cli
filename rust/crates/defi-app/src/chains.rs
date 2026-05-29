//! `chains` command group handler.
//!
//! Mirrors the `chains` subtree of `internal/app/runner.go::newChainsCommand`
//! plus the `fetchGasPrice`/`weiToGwei` helpers it composes. This module owns
//! the **command-layer composition** for the chains group; the lower-level
//! pieces are owned elsewhere and reused:
//!
//! * RPC reads + wei→gwei formatting parity: [`defi_evm::rpc`] (`RpcClient`,
//!   `wei_to_gwei`) — already contract-tested there;
//! * chain registry + parsing: [`defi_id`] (`list_chains`, `parse_chain`);
//! * default-RPC resolution + precedence: [`defi_registry::resolve_rpc_url`];
//! * cache-bypass routing (`chains list` / `chains gas` bypass): the runner
//!   (`defi_app::runner::should_open_cache`).
//!
//! The two contract-bearing surfaces this module composes:
//!
//! 1. **`chains list`** — offline, no keys, deterministic: maps the chain
//!    registry to `model::SupportedChain` in CAIP-2 order (golden parity with
//!    the Go binary).
//! 2. **`chains gas`** — live EVM gas, no keys, bypasses cache, returns an
//!    *array* of `model::GasPrice` even for a single chain. Single-chain may use
//!    a `--rpc-url` override; multi-chain forbids it, validates every chain as
//!    EVM up front, fetches in parallel preserving input order, drops failures
//!    into `warnings` (partial), and fails only if *all* chains fail (or, in
//!    strict mode, if any chain fails).
//!
//! Idiomatic-Rust shape note: the Go command closures write to injected
//! `io.Writer`s and return `error`. The Rust port exposes pure/async builder
//! functions returning values (`Vec<SupportedChain>`, `Result<GasPrice, Error>`,
//! `GasOutcome`) so they can be unit-tested without a `cobra.Command`; the
//! envelope construction + rendering is layered on top by the runner.

#![allow(dead_code, unused_variables)]

use chrono::{DateTime, SecondsFormat, Utc};
use defi_errors::{Code, Error};
use defi_evm::rpc::{wei_to_gwei, RpcClient};
use defi_id::{parse_chain, Chain};
use defi_model::{GasPrice, SupportedChain};

/// Build the `chains list` data payload.
///
/// Maps every entry from [`defi_id::list_chains`] (already deduped + sorted by
/// CAIP-2) to a [`SupportedChain`], preserving `name`/`slug`/`caip2`/`namespace`
/// /`evm_chain_id`/`aliases`. Pure + offline (Go `newChainsCommand` `list`).
pub fn list_chains_data() -> Vec<SupportedChain> {
    defi_id::list_chains()
        .into_iter()
        .map(|entry| SupportedChain {
            name: entry.chain.name.clone(),
            slug: entry.chain.slug.clone(),
            caip2: entry.chain.caip2.clone(),
            namespace: entry.chain.namespace(),
            evm_chain_id: entry.chain.evm_chain_id,
            aliases: entry.aliases,
        })
        .collect()
}

/// A resolved gas-fetch target: a validated EVM chain plus the RPC URL to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GasChainTarget {
    /// The validated (EVM) chain.
    pub chain: Chain,
    /// The resolved RPC URL (override or registry default).
    pub rpc_url: String,
}

/// Parse and validate the `chains gas` `--chain` / `--rpc-url` flags into the
/// ordered list of fetch targets (Go `newChainsCommand` `gas` pre-flight).
///
/// Behavior (preserved from Go):
/// * splits `chain_arg` on `,`, trimming whitespace and dropping empties;
/// * at least one chain is required → [`defi_errors::Code::Usage`];
/// * `--rpc-url` with more than one chain → [`defi_errors::Code::Usage`];
/// * each chain is parsed (`defi_id::parse_chain`); non-EVM (`namespace !=
///   "eip155"`) → [`defi_errors::Code::Unsupported`];
/// * the RPC URL is resolved per chain via `defi_registry::resolve_rpc_url`
///   (override wins for the single-chain case; a missing default surfaces as the
///   resolver's error). Input order is preserved.
pub fn resolve_gas_targets(chain_arg: &str, rpc_url: &str) -> Result<Vec<GasChainTarget>, Error> {
    let chain_args: Vec<&str> = chain_arg
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect();

    if chain_args.is_empty() {
        return Err(Error::new(Code::Usage, "at least one chain is required"));
    }

    if chain_args.len() > 1 && !rpc_url.trim().is_empty() {
        return Err(Error::new(
            Code::Usage,
            "--rpc-url cannot be used with multiple chains",
        ));
    }

    let mut targets = Vec::with_capacity(chain_args.len());
    for raw in chain_args {
        let chain = parse_chain(raw)?;
        if chain.namespace() != "eip155" {
            return Err(Error::new(
                Code::Unsupported,
                format!("chains gas is only supported for EVM chains: {raw}"),
            ));
        }
        let resolved = defi_registry::resolve_rpc_url(rpc_url, chain.evm_chain_id)
            .map_err(|e| Error::wrap(Code::Unavailable, format!("resolve rpc for {raw}"), e))?;
        targets.push(GasChainTarget {
            chain,
            rpc_url: resolved,
        });
    }
    Ok(targets)
}

/// Fetch the current gas price for one chain over an established RPC client
/// (Go `fetchGasPrice`).
///
/// Reads the latest header (block number + optional `baseFeePerGas`) and the
/// suggested gas price. EIP-1559 is detected by the presence of a base fee; when
/// present the suggested priority fee (tip cap) is read too — and if that read
/// fails, the tip is recorded as zero (`"0.000000"`) with a
/// `"priority fee unavailable: …"` warning rather than failing the call. All
/// gwei fields use the [`defi_evm::rpc::wei_to_gwei`] formatter; `fetched_at` is
/// `now` rendered as RFC 3339 (UTC, `Z`). Legacy chains omit the base/priority
/// fee fields (empty → omitted from JSON).
pub async fn fetch_gas_price(
    client: &RpcClient,
    chain: &Chain,
    now: DateTime<Utc>,
) -> Result<GasPrice, Error> {
    let block_number = client.block_number().await?;
    let base_fee = client.base_fee().await?;
    let gas_price = client.gas_price().await?;

    let eip1559 = base_fee.is_some();
    let mut warnings = Vec::new();
    let mut base_fee_gwei = String::new();
    let mut priority_fee_gwei = String::new();

    if let Some(base) = base_fee {
        base_fee_gwei = wei_to_gwei(Some(base));
        let priority_fee = match client.max_priority_fee().await {
            Ok(tip) => tip,
            Err(e) => {
                warnings.push(format!("priority fee unavailable: {e}"));
                alloy::primitives::U256::ZERO
            }
        };
        priority_fee_gwei = wei_to_gwei(Some(priority_fee));
    }

    Ok(GasPrice {
        chain_id: chain.caip2.clone(),
        chain_name: chain.name.clone(),
        block_number: block_number as i64,
        eip1559,
        base_fee_gwei,
        priority_fee_gwei,
        gas_price_gwei: wei_to_gwei(Some(gas_price)),
        warnings,
        fetched_at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
    })
}

/// The resolved result of a `chains gas` invocation over one or more targets.
///
/// Mirrors the Go command's success path: an ordered list of [`GasPrice`] for
/// the chains that succeeded, the per-chain failure `warnings`, and the
/// `partial` flag (true iff at least one chain failed). A single-chain request
/// still yields a one-element `prices` vector (array-always contract).
#[derive(Debug, Clone)]
pub struct GasOutcome {
    /// Successful per-chain gas prices, in input order.
    pub prices: Vec<GasPrice>,
    /// `"chain <caip2>: <error>"` for every chain that failed.
    pub warnings: Vec<String>,
    /// Whether any chain failed.
    pub partial: bool,
}

/// Run `chains gas` across already-resolved targets, fetching in parallel and
/// preserving input order (Go `newChainsCommand` `gas` success path).
///
/// Behavior (preserved from Go):
/// * each target is fetched via [`fetch_gas_price`]; failures become
///   `"chain <caip2>: <error>"` warnings and are dropped from `prices`;
/// * if *every* chain failed → [`defi_errors::Code::Unavailable`] with an
///   `"all chains failed; …"` message;
/// * otherwise the surviving prices are returned with `partial = !warnings`.
///
/// Strict-mode partial rejection is layered by the runner, not here.
pub async fn run_gas(targets: &[GasChainTarget], now: DateTime<Utc>) -> Result<GasOutcome, Error> {
    // Fetch every chain concurrently, then reassemble in input order. Each task
    // owns a cloned chain + RPC URL so the borrow of `targets` does not escape.
    let handles: Vec<_> = targets
        .iter()
        .map(|target| {
            let chain = target.chain.clone();
            let rpc_url = target.rpc_url.clone();
            tokio::spawn(async move {
                let client = RpcClient::connect(&rpc_url)
                    .map_err(|e| Error::wrap(Code::Unavailable, "connect rpc", e))?;
                fetch_gas_price(&client, &chain, now).await
            })
        })
        .collect();

    let mut prices = Vec::with_capacity(targets.len());
    let mut warnings = Vec::new();
    for (target, handle) in targets.iter().zip(handles) {
        let result = match handle.await {
            Ok(res) => res,
            Err(join_err) => Err(Error::new(
                Code::Unavailable,
                format!("gas fetch task failed: {join_err}"),
            )),
        };
        match result {
            Ok(price) => prices.push(price),
            Err(err) => warnings.push(format!("chain {}: {}", target.chain.caip2, err)),
        }
    }

    if prices.is_empty() {
        return Err(Error::new(
            Code::Unavailable,
            format!("all chains failed; {}", warnings.join("; ")),
        ));
    }

    let partial = !warnings.is_empty();
    Ok(GasOutcome {
        prices,
        warnings,
        partial,
    })
}

/// clap parsing + handler for the `chains` command group.
pub mod cli {
    use clap::{Args, Subcommand};
    use defi_errors::Error;
    use defi_model::{CacheStatus, Envelope};

    use crate::ctx::AppCtx;

    /// `chains` subcommands (Go `newChainsCommand`).
    #[derive(Subcommand, Debug)]
    pub enum ChainsCmd {
        /// List all supported chains with aliases (no keys required).
        List,
        /// Current gas prices for one or more EVM chains (no keys required).
        Gas(GasArgs),
        /// Top chains by TVL.
        Top(TopArgs),
        /// TVL by asset for a chain (DefiLlama key required).
        Assets(AssetsArgs),
    }

    impl ChainsCmd {
        /// The leaf path token (for `meta.command`).
        pub fn path(&self) -> &'static str {
            match self {
                ChainsCmd::List => "list",
                ChainsCmd::Gas(_) => "gas",
                ChainsCmd::Top(_) => "top",
                ChainsCmd::Assets(_) => "assets",
            }
        }
    }

    /// `chains gas` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct GasArgs {
        /// Chain id/name/CAIP-2 (comma-separated for multiple).
        #[arg(long)]
        pub chain: Option<String>,
        /// RPC URL override (single chain only).
        #[arg(long = "rpc-url")]
        pub rpc_url: Option<String>,
    }

    /// `chains top` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct TopArgs {
        /// Number of chains to return.
        #[arg(long, default_value_t = 20)]
        pub limit: i64,
    }

    /// `chains assets` flags.
    #[derive(Args, Debug, Clone, Default)]
    pub struct AssetsArgs {
        /// Chain id/name/CAIP-2.
        #[arg(long)]
        pub chain: Option<String>,
        /// Asset filter (symbol/address/CAIP-19).
        #[arg(long)]
        pub asset: Option<String>,
        /// Number of assets to return.
        #[arg(long, default_value_t = 20)]
        pub limit: i64,
    }

    /// Handle `chains <sub>`.
    pub async fn handle(ctx: &AppCtx, cmd: ChainsCmd) -> Result<Envelope, Error> {
        match cmd {
            ChainsCmd::List => Ok(list_envelope(ctx)),
            ChainsCmd::Gas(args) => gas(ctx, args).await,
            ChainsCmd::Top(_) => Err(AppCtx::unimplemented("chains top", "WS2")),
            ChainsCmd::Assets(_) => Err(AppCtx::unimplemented("chains assets", "WS2")),
        }
    }

    /// Build the `chains list` success envelope (metadata, cache bypassed).
    fn list_envelope(ctx: &AppCtx) -> Envelope {
        let data =
            serde_json::to_value(super::list_chains_data()).unwrap_or(serde_json::Value::Null);
        ctx.metadata_envelope("chains list", data, Vec::new())
    }

    /// Run `chains gas`: live EVM gas prices (no keys, cache bypassed). Returns
    /// an array of [`defi_model::GasPrice`] even for a single chain.
    async fn gas(ctx: &AppCtx, args: GasArgs) -> Result<Envelope, Error> {
        let targets = super::resolve_gas_targets(
            args.chain.as_deref().unwrap_or_default(),
            args.rpc_url.as_deref().unwrap_or_default(),
        )?;
        let outcome = super::run_gas(&targets, ctx.now()).await?;

        if outcome.partial && ctx.settings.strict {
            return Err(Error::new(
                defi_errors::Code::PartialStrict,
                "partial results returned in strict mode",
            ));
        }

        let data = serde_json::to_value(&outcome.prices)
            .map_err(|e| Error::wrap(defi_errors::Code::Internal, "serialize gas prices", e))?;
        let mut env = Envelope::success(
            "chains gas",
            data,
            outcome.warnings,
            CacheStatus::bypass(),
            Vec::new(),
            outcome.partial,
        );
        env.meta.timestamp = ctx.now();
        Ok(env)
    }
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::chains_cmd` (Go: `internal/app` chains)
    //!
    //! This module owns the **command-layer composition** for the `chains`
    //! group. "Correct" means it preserves the stable machine contract (design
    //! spec §2.1 envelope, §2.2 exit codes, §2.3 rendering, §2.4 ids/amounts)
    //! and the chains-specific behaviors of `internal/app/runner.go`. The
    //! criteria asserted below (NOT Go internals — the RPC plumbing + gwei
    //! formatting parity already live in `defi-evm::rpc`):
    //!
    //! 1. **`chains list` shape + golden parity.** [`list_chains_data`] yields a
    //!    `SupportedChain` per registry entry in CAIP-2 order; rendered as a
    //!    success envelope (`status="bypass"`) and with `--results-only` it is
    //!    byte-for-byte the Go binary's `chains list` golden fixtures.
    //!    `evm_chain_id`/`aliases` are omitted when zero/empty (omitempty
    //!    contract). (Spec §2.1, §2.3.)
    //! 2. **`chains list` bypasses the cache** (metadata route — spec §2.5).
    //!    Asserted via `runner::should_open_cache("chains list") == false`.
    //! 3. **`chains gas` EIP-1559 composition.** [`fetch_gas_price`] over a
    //!    base-fee chain sets `eip1559=true`, fills `base_fee_gwei` /
    //!    `priority_fee_gwei` / `gas_price_gwei` via the wei→gwei formatter
    //!    (`"1.000000"`, `"2.000000"`, `"3.000000"`), copies `chain_id`/
    //!    `chain_name`, reads `block_number` from the latest header, and renders
    //!    `fetched_at` as RFC 3339 UTC. (Go `TestFetchGasPriceEIP1559`.)
    //! 4. **`chains gas` legacy composition + omitempty.** A chain whose latest
    //!    header has no base fee → `eip1559=false`, `base_fee_gwei`/
    //!    `priority_fee_gwei` empty (and therefore omitted from JSON),
    //!    `gas_price_gwei` still set. (Go `TestFetchGasPriceLegacy`.)
    //! 5. **`chains gas` tip-cap failure is non-fatal.** An EIP-1559 chain whose
    //!    `eth_maxPriorityFeePerGas` errors still succeeds with `eip1559=true`,
    //!    `priority_fee_gwei="0.000000"`, and a `"priority fee unavailable: …"`
    //!    warning. (Go `TestFetchGasPriceTipCapFailureAddsWarning`.)
    //! 6. **`chains gas` requires a chain** → [`Code::Usage`]; empty/blank
    //!    `--chain` is rejected. (Go `TestChainsGasRequiresChainFlag`.)
    //! 7. **`chains gas` is EVM-only** → a non-EVM chain (`solana`) is rejected
    //!    with [`Code::Unsupported`], in both single and multi-chain lists. (Go
    //!    `TestChainsGasRejectsNonEVM`, `TestChainsGasRejectsNonEVMInMulti`.)
    //! 8. **`chains gas` `--rpc-url` is single-chain only** → a multi-chain
    //!    `--chain` with `--rpc-url` is [`Code::Usage`]. (Go
    //!    `TestChainsGasMultipleChainsRejectsRPCURL`.)
    //! 9. **`chains gas` array-always.** Both single- and multi-chain requests
    //!    produce a `Vec<GasPrice>` (one element for a single chain), in input
    //!    order. (Go `TestChainsGasSingleChainReturnsArray`,
    //!    `TestChainsGasMultipleChainsWithMockRPC`.)
    //! 10. **`chains gas` partial tolerance.** With multiple chains where some
    //!     fail, surviving prices are returned, failures become
    //!     `"chain <caip2>: …"` warnings, and `partial=true`. All chains failing
    //!     → [`Code::Unavailable`] (`"all chains failed; …"`). Strict-mode
    //!     partial rejection (exit 15) is the runner's responsibility, not this
    //!     module's. (Spec §2.5 partial; Go gas command success path.)
    //! 11. **`chains gas` bypasses the cache** (metadata route — spec §2.5).
    //!     Asserted via `runner::should_open_cache("chains gas") == false`.
    //!
    //! Ported from `runner_gas_test.go` (the meaningful command-composition
    //! cases). Skipped here (covered elsewhere or internal detail):
    //! * `TestWeiToGwei` + the byte-level RPC reads — owned/tested by
    //!   `defi-evm::rpc` (`wei_to_gwei`, `RpcClient`), not re-asserted here;
    //! * `TestChainsGasBypassesCache` (`shouldOpenCache`) — the routing predicate
    //!   lives in `defi-app::runner` and is asserted by its tests; we add one
    //!   confirmation here for the chains paths;
    //! * the Go `httptest` batch-vs-single request plumbing — an
    //!   ethclient/alloy transport detail, not part of the contract.

    use super::*;
    use chrono::TimeZone;
    use defi_config::Settings;
    use defi_errors::Code;
    use defi_model::Envelope;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::time::Duration;
    use wiremock::matchers::{body_partial_json, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // --- fixtures ----------------------------------------------------------

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 9, 12, 0, 0).unwrap()
    }

    fn evm_chain(name: &str, slug: &str, id: i64) -> Chain {
        Chain {
            name: name.to_string(),
            slug: slug.to_string(),
            caip2: format!("eip155:{id}"),
            evm_chain_id: id,
        }
    }

    /// Minimal results-only JSON settings for golden-parity rendering.
    fn results_only_settings() -> Settings {
        Settings {
            output_mode: "json".to_string(),
            select_fields: Vec::new(),
            results_only: true,
            enable_commands: Vec::new(),
            strict: false,
            timeout: Duration::from_secs(2),
            retries: 0,
            max_stale: Duration::from_secs(0),
            no_stale: false,
            cache_enabled: false,
            cache_path: PathBuf::new(),
            cache_lock_path: PathBuf::new(),
            action_store_path: PathBuf::new(),
            action_lock_path: PathBuf::new(),
            defillama_api_key: String::new(),
            uniswap_api_key: String::new(),
            oneinch_api_key: String::new(),
            jupiter_api_key: String::new(),
            bungee_api_key: String::new(),
            bungee_affiliate: String::new(),
        }
    }

    fn full_envelope_settings() -> Settings {
        let mut s = results_only_settings();
        s.results_only = false;
        s
    }

    /// Register a JSON-RPC method responder returning `result`. Mirrors
    /// `runner_gas_test.go::newMockRPCServer` (one responder per method).
    async fn mock_method(server: &MockServer, rpc_method: &str, result: Value) {
        Mock::given(method("POST"))
            .and(body_partial_json(json!({ "method": rpc_method })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": result,
            })))
            .mount(server)
            .await;
    }

    async fn mock_method_error(server: &MockServer, rpc_method: &str) {
        Mock::given(method("POST"))
            .and(body_partial_json(json!({ "method": rpc_method })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": { "code": -32601, "message": "method not found" },
            })))
            .mount(server)
            .await;
    }

    fn block_result(number_hex: &str, base_fee_hex: Option<&str>) -> Value {
        let mut obj = json!({
            "number": number_hex,
            "hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "gasLimit": "0x0",
            "gasUsed": "0x0",
            "timestamp": "0x0",
        });
        match base_fee_hex {
            Some(b) => obj["baseFeePerGas"] = json!(b),
            None => obj["baseFeePerGas"] = Value::Null,
        }
        obj
    }

    /// A mock RPC server primed for a gas fetch with the given hex fee values.
    /// `base_fee_hex == None` simulates a legacy chain; `priority_ok == false`
    /// makes `eth_maxPriorityFeePerGas` return a JSON-RPC error.
    async fn gas_server(
        block_hex: &str,
        base_fee_hex: Option<&str>,
        gas_price_hex: &str,
        priority_fee_hex: &str,
        priority_ok: bool,
    ) -> MockServer {
        let server = MockServer::start().await;
        mock_method(
            &server,
            "eth_getBlockByNumber",
            block_result(block_hex, base_fee_hex),
        )
        .await;
        mock_method(&server, "eth_gasPrice", json!(gas_price_hex)).await;
        if priority_ok {
            mock_method(&server, "eth_maxPriorityFeePerGas", json!(priority_fee_hex)).await;
        } else {
            mock_method_error(&server, "eth_maxPriorityFeePerGas").await;
        }
        server
    }

    // --- 1. chains list shape + golden parity -----------------------------

    #[test]
    fn list_chains_data_includes_ethereum_in_caip2_order() {
        let chains = list_chains_data();
        assert!(!chains.is_empty());
        // First entry is Ethereum (lowest CAIP-2, eip155:1), with its alias.
        let first = &chains[0];
        assert_eq!(first.name, "Ethereum");
        assert_eq!(first.slug, "ethereum");
        assert_eq!(first.caip2, "eip155:1");
        assert_eq!(first.namespace, "eip155");
        assert_eq!(first.evm_chain_id, 1);
        assert_eq!(first.aliases, vec!["mainnet".to_string()]);
    }

    #[test]
    fn list_chains_data_results_only_matches_go_golden() {
        let data = serde_json::to_value(list_chains_data()).expect("serialize chains");
        let env = Envelope::success(
            "chains list",
            data,
            Vec::new(),
            defi_model::CacheStatus::bypass(),
            Vec::new(),
            false,
        );
        let rendered =
            defi_out::render(&env, &results_only_settings()).expect("render results-only");
        let golden = include_str!("../../../tests/golden/chains-list-results-only.json");
        assert_eq!(
            rendered.trim_end(),
            golden.trim_end(),
            "chains list --results-only must match the Go golden fixture byte-for-byte"
        );
    }

    #[test]
    fn list_chains_data_full_envelope_data_matches_go_golden_data() {
        // The full-envelope golden carries nondeterministic request_id/timestamp,
        // so compare only the `data` array (the part this module owns).
        let env_data = serde_json::to_value(list_chains_data()).expect("serialize chains");
        let golden: Value =
            serde_json::from_str(include_str!("../../../tests/golden/chains-list.json"))
                .expect("parse golden envelope");
        assert_eq!(
            &env_data,
            golden.get("data").expect("golden data array"),
            "chains list `data` must match the Go golden envelope"
        );
        assert_eq!(golden["version"], json!("v1"));
        assert_eq!(golden["success"], json!(true));
    }

    #[test]
    fn list_chains_data_omits_zero_evm_id_and_empty_aliases() {
        let chains = list_chains_data();
        // Polygon has no aliases in the golden → `aliases` omitted from JSON.
        let polygon = chains
            .iter()
            .find(|c| c.caip2 == "eip155:137")
            .expect("polygon present");
        let v = serde_json::to_value(polygon).expect("serialize polygon");
        assert!(
            v.get("aliases").is_none(),
            "empty aliases must be omitted (omitempty), got: {v}"
        );
        // A Solana (non-EVM) chain, if present, omits evm_chain_id (zero).
        if let Some(solana) = chains.iter().find(|c| c.namespace == "solana") {
            let sv = serde_json::to_value(solana).expect("serialize solana");
            assert!(
                sv.get("evm_chain_id").is_none(),
                "zero evm_chain_id must be omitted (omitempty), got: {sv}"
            );
        }
    }

    // --- 2 & 11. cache-bypass routing -------------------------------------

    #[test]
    fn chains_list_and_gas_bypass_cache() {
        assert!(
            !crate::runner::should_open_cache("chains list"),
            "chains list must bypass cache"
        );
        assert!(
            !crate::runner::should_open_cache("chains gas"),
            "chains gas must bypass cache"
        );
        // A data command in the chains group still opens the cache.
        assert!(
            crate::runner::should_open_cache("chains assets"),
            "chains assets must open cache"
        );
    }

    // --- 3. EIP-1559 composition ------------------------------------------

    #[tokio::test]
    async fn fetch_gas_price_eip1559_fills_all_fee_fields() {
        // base 1 gwei, tip 2 gwei, gas price 3 gwei, block 16.
        let server = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "0x77359400", true).await;
        let client = RpcClient::connect(&server.uri()).expect("connect");
        let chain = evm_chain("Ethereum", "ethereum", 1);

        let result = fetch_gas_price(&client, &chain, fixed_now())
            .await
            .expect("fetch gas price");

        assert_eq!(result.chain_id, "eip155:1");
        assert_eq!(result.chain_name, "Ethereum");
        assert_eq!(result.block_number, 16);
        assert!(result.eip1559);
        assert_eq!(result.base_fee_gwei, "1.000000");
        assert_eq!(result.priority_fee_gwei, "2.000000");
        assert_eq!(result.gas_price_gwei, "3.000000");
        assert_eq!(result.fetched_at, "2026-03-09T12:00:00Z");
        assert!(result.warnings.is_empty());
    }

    // --- 4. legacy composition + omitempty --------------------------------

    #[tokio::test]
    async fn fetch_gas_price_legacy_has_no_eip1559_fee_fields() {
        // No base fee => legacy chain; gas price 5 gwei.
        let server = gas_server("0x5", None, "0x12A05F200", "", false).await;
        let client = RpcClient::connect(&server.uri()).expect("connect");
        let chain = evm_chain("TestLegacy", "legacy", 999);

        let result = fetch_gas_price(&client, &chain, fixed_now())
            .await
            .expect("fetch gas price");

        assert!(!result.eip1559);
        assert_eq!(result.base_fee_gwei, "");
        assert_eq!(result.priority_fee_gwei, "");
        assert_eq!(result.gas_price_gwei, "5.000000");

        // omitempty: empty base/priority fees must be absent from JSON.
        let v = serde_json::to_value(&result).expect("serialize gas price");
        assert!(
            v.get("base_fee_gwei").is_none(),
            "legacy chain must omit base_fee_gwei, got: {v}"
        );
        assert!(
            v.get("priority_fee_gwei").is_none(),
            "legacy chain must omit priority_fee_gwei, got: {v}"
        );
    }

    // --- 5. tip-cap failure is non-fatal ----------------------------------

    #[tokio::test]
    async fn fetch_gas_price_tip_cap_failure_adds_warning_and_zero_tip() {
        // EIP-1559 chain (has base fee) but eth_maxPriorityFeePerGas errors.
        let server = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "", false).await;
        let client = RpcClient::connect(&server.uri()).expect("connect");
        let chain = evm_chain("Ethereum", "ethereum", 1);

        let result = fetch_gas_price(&client, &chain, fixed_now())
            .await
            .expect("fetch gas price should not fail on tip-cap error");

        assert!(result.eip1559);
        assert_eq!(result.priority_fee_gwei, "0.000000");
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("priority fee unavailable")),
            "expected a priority-fee-unavailable warning, got: {:?}",
            result.warnings
        );
    }

    // --- 6, 7, 8. flag/chain validation (resolve_gas_targets) -------------

    #[test]
    fn resolve_gas_targets_requires_at_least_one_chain() {
        let err = resolve_gas_targets("", "").expect_err("empty chain rejected");
        assert_eq!(err.code, Code::Usage);
        let err = resolve_gas_targets("  ,  , ", "").expect_err("blank chain rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn resolve_gas_targets_rejects_non_evm_chain() {
        let err = resolve_gas_targets("solana", "").expect_err("non-EVM rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert!(
            err.to_string().to_uppercase().contains("EVM"),
            "expected EVM-only message, got: {err}"
        );
    }

    #[test]
    fn resolve_gas_targets_rejects_non_evm_in_multi_list() {
        let err = resolve_gas_targets("1,solana", "").expect_err("non-EVM in multi rejected");
        assert_eq!(err.code, Code::Unsupported);
    }

    #[test]
    fn resolve_gas_targets_rejects_rpc_url_with_multiple_chains() {
        let err = resolve_gas_targets("1,10", "https://example.com")
            .expect_err("multi-chain + rpc-url rejected");
        assert_eq!(err.code, Code::Usage);
        assert!(
            err.to_string().contains("rpc-url"),
            "expected rpc-url message, got: {err}"
        );
    }

    #[test]
    fn resolve_gas_targets_single_chain_uses_rpc_url_override() {
        let targets = resolve_gas_targets("1", "https://override.example.test")
            .expect("single chain + override resolves");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].chain.caip2, "eip155:1");
        assert_eq!(targets[0].rpc_url, "https://override.example.test");
    }

    #[test]
    fn resolve_gas_targets_preserves_input_order_for_multi() {
        // No --rpc-url, so registry defaults are used for both; order preserved.
        let targets = resolve_gas_targets("10,1", "").expect("multi resolves with defaults");
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].chain.caip2, "eip155:10");
        assert_eq!(targets[1].chain.caip2, "eip155:1");
    }

    // --- 9. array-always (single + multi) ---------------------------------

    #[tokio::test]
    async fn run_gas_single_chain_returns_one_element_array() {
        let server = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "0x77359400", true).await;
        let targets = vec![GasChainTarget {
            chain: evm_chain("Ethereum", "ethereum", 1),
            rpc_url: server.uri(),
        }];

        let outcome = run_gas(&targets, fixed_now()).await.expect("run gas");
        assert_eq!(
            outcome.prices.len(),
            1,
            "single chain still yields an array"
        );
        assert_eq!(outcome.prices[0].chain_id, "eip155:1");
        assert!(!outcome.partial);
        assert!(outcome.warnings.is_empty());
    }

    #[tokio::test]
    async fn run_gas_multi_chain_preserves_input_order() {
        // chain1: gas 3 gwei, block 16; chain2: gas 4 gwei, block 32.
        let srv1 = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "0x77359400", true).await;
        let srv2 = gas_server("0x20", Some("0x77359400"), "0xEE6B2800", "0x3B9ACA00", true).await;
        let targets = vec![
            GasChainTarget {
                chain: evm_chain("Ethereum", "ethereum", 1),
                rpc_url: srv1.uri(),
            },
            GasChainTarget {
                chain: evm_chain("Optimism", "optimism", 10),
                rpc_url: srv2.uri(),
            },
        ];

        let outcome = run_gas(&targets, fixed_now()).await.expect("run gas");
        assert_eq!(outcome.prices.len(), 2);
        assert_eq!(outcome.prices[0].chain_id, "eip155:1");
        assert_eq!(outcome.prices[1].chain_id, "eip155:10");
        assert_eq!(outcome.prices[0].gas_price_gwei, "3.000000");
        assert_eq!(outcome.prices[1].gas_price_gwei, "4.000000");
        assert_eq!(outcome.prices[1].block_number, 32);
        assert!(!outcome.partial);
    }

    // --- 10. partial tolerance --------------------------------------------

    #[tokio::test]
    async fn run_gas_partial_drops_failures_into_warnings() {
        // chain1 succeeds; chain2 points at a dead URL (connection refused).
        let srv1 = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "0x77359400", true).await;
        let targets = vec![
            GasChainTarget {
                chain: evm_chain("Ethereum", "ethereum", 1),
                rpc_url: srv1.uri(),
            },
            GasChainTarget {
                chain: evm_chain("Optimism", "optimism", 10),
                // Unroutable port → fetch fails.
                rpc_url: "http://127.0.0.1:1".to_string(),
            },
        ];

        let outcome = run_gas(&targets, fixed_now())
            .await
            .expect("partial still succeeds");
        assert_eq!(outcome.prices.len(), 1, "only the healthy chain survives");
        assert_eq!(outcome.prices[0].chain_id, "eip155:1");
        assert!(outcome.partial);
        assert!(
            outcome.warnings.iter().any(|w| w.contains("eip155:10")),
            "failed chain must be named in warnings, got: {:?}",
            outcome.warnings
        );
    }

    #[tokio::test]
    async fn run_gas_all_chains_failed_is_unavailable() {
        let targets = vec![GasChainTarget {
            chain: evm_chain("Ethereum", "ethereum", 1),
            rpc_url: "http://127.0.0.1:1".to_string(),
        }];

        let err = run_gas(&targets, fixed_now())
            .await
            .expect_err("all-failed is an error");
        assert_eq!(err.code, Code::Unavailable);
        assert!(
            err.to_string().contains("all chains failed"),
            "expected all-chains-failed message, got: {err}"
        );
    }

    // =====================================================================
    // App-level `chains gas` (WS1, wiremock RPC end-to-end through handle /
    // run_with_args). These exercise the wired handler's full envelope + exit
    // codes via the existing `--rpc-url` seam (no DefiLlama base-URL override
    // needed). Additional success criteria over the unit cases above:
    //
    //  A1. **Single-chain handler envelope.** `chains gas --chain 1 --rpc-url
    //      <mock>` resolves a success [`Envelope`]: `version="v1"`,
    //      `success=true`, `error=None`, `data` = a ONE-element array of
    //      `GasPrice`, `meta.command="chains gas"`, `meta.cache.status="bypass"`
    //      (metadata route), `meta.providers` EMPTY (the Go gas command passes
    //      `nil` providers), `partial=false`. (Go gas command success path.)
    //  A2. **Multi-chain array + input order.** Two chains with two mock RPCs
    //      (no `--rpc-url`, registry defaults) → a two-element array in input
    //      order. (Driven via `cli::handle` with explicit targets is the unit
    //      path; here we assert the array-always contract end-to-end with one
    //      mock + a single chain, and the multi-chain `--rpc-url` rejection.)
    //  A3. **`--rpc-url` rejected with multiple chains → exit 2 (usage)**,
    //      through the full `run_with_args` path: a usage error renders the FULL
    //      envelope on stderr and returns exit code 2. (Go
    //      `TestChainsGasMultipleChainsRejectsRPCURL`.)
    //  A4. **Missing `--chain` → exit 2 (usage)** through `run_with_args`.
    //  A5. **Non-EVM chain → exit 13 (unsupported)** through `run_with_args`.
    //  A6. **Single-chain success → exit 0** through `run_with_args` with a mock
    //      RPC.
    // =====================================================================

    use crate::cli::run_with_args;
    use crate::ctx::AppCtx;
    use defi_config::MapEnv;

    /// App settings: JSON, cache bypassed (gas always bypasses anyway).
    fn app_settings() -> Settings {
        let mut s = results_only_settings();
        s.results_only = false;
        s.timeout = Duration::from_secs(5);
        s
    }

    /// A `MapEnv` whose HOME points at a temp dir so `Settings::load` can resolve
    /// cache/config paths without touching the real home directory. Returns the
    /// `TempDir` guard so the caller keeps it alive for the test's duration.
    fn env_with_home() -> (MapEnv, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let env = MapEnv::with_home(tmp.path().to_path_buf());
        (env, tmp)
    }

    fn gas_args(chain: &str, rpc_url: Option<&str>) -> super::cli::GasArgs {
        super::cli::GasArgs {
            chain: Some(chain.to_string()),
            rpc_url: rpc_url.map(str::to_string),
        }
    }

    // --- A1. single-chain handler envelope --------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_gas_handler_single_chain_full_envelope() {
        let server = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "0x77359400", true).await;
        let ctx = AppCtx::new(app_settings());

        let env = super::cli::handle(
            &ctx,
            super::cli::ChainsCmd::Gas(gas_args("1", Some(&server.uri()))),
        )
        .await
        .expect("chains gas single chain should succeed");

        assert_eq!(env.version, "v1");
        assert!(env.success);
        assert!(env.error.is_none());
        assert_eq!(env.meta.command, "chains gas");
        assert!(!env.meta.partial);

        // Metadata route: cache bypassed, no provider statuses.
        assert_eq!(env.meta.cache.status, "bypass");
        assert!(
            env.meta.providers.is_empty(),
            "chains gas passes nil providers (Go parity), got: {:?}",
            env.meta.providers
        );

        // Array-always: a single chain still yields a one-element array.
        let rows = env
            .data
            .as_ref()
            .and_then(Value::as_array)
            .expect("data is an array");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["chain_id"], json!("eip155:1"));
        assert_eq!(rows[0]["eip1559"], json!(true));
        assert_eq!(rows[0]["gas_price_gwei"], json!("3.000000"));
    }

    // --- A3. multi-chain + --rpc-url rejected (usage) via run_with_args ----

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_gas_multi_chain_with_rpc_url_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "chains",
                "gas",
                "--chain",
                "1,10",
                "--rpc-url",
                "https://example.test",
            ],
            &env,
        )
        .await;
        assert_eq!(
            code, 2,
            "multi-chain + --rpc-url must be a usage error (exit 2)"
        );
    }

    // --- A4. missing --chain (usage) via run_with_args --------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_gas_missing_chain_is_usage_exit_2() {
        let (env, _home) = env_with_home();
        // --chain is optional at the parser; the handler rejects an empty chain
        // with CodeUsage (Go parity). Either way → exit 2.
        let code = run_with_args(["defi", "chains", "gas"], &env).await;
        assert_eq!(code, 2, "missing --chain must be a usage error (exit 2)");
    }

    // --- A5. non-EVM chain (unsupported) via run_with_args ----------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_gas_non_evm_is_unsupported_exit_13() {
        let (env, _home) = env_with_home();
        let code = run_with_args(["defi", "chains", "gas", "--chain", "solana"], &env).await;
        assert_eq!(
            code, 13,
            "a non-EVM chain must be unsupported (exit 13), got {code}"
        );
    }

    // --- A6. single-chain success → exit 0 via run_with_args --------------

    #[tokio::test(flavor = "multi_thread")]
    async fn chains_gas_single_chain_success_exit_0() {
        let server = gas_server("0x10", Some("0x3B9ACA00"), "0xB2D05E00", "0x77359400", true).await;
        let (env, _home) = env_with_home();
        let code = run_with_args(
            [
                "defi",
                "chains",
                "gas",
                "--chain",
                "1",
                "--rpc-url",
                &server.uri(),
            ],
            &env,
        )
        .await;
        assert_eq!(code, 0, "a healthy single-chain gas query must exit 0");
    }
}
