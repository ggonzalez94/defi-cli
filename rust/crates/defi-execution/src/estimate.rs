//! `actions estimate` — gas/fee estimation for a planned action.
//!
//! Go source: `internal/execution/estimate.go` (+ the chain-id/Tempo helpers in
//! `step_executor.go` and the fee-cap math in `executor.go` that this module
//! composes). This module owns the **read-only gas/fee estimate** path: turning a
//! persisted [`crate::action::Action`]'s steps into per-step + per-chain
//! EIP-1559 (or Tempo fee-token) cost projections, WITHOUT signing or
//! broadcasting anything.
//!
//! ## Scope boundary vs. sibling modules (no overlap)
//!
//! - **Single JSON-RPC reads** (`eth_chainId`, latest-header base fee,
//!   `eth_estimateGas`, `eth_maxPriorityFeePerGas`) and the wei/gwei + fee-cap
//!   math (`wei_to_gwei`, `parse_gwei`, `resolve_tip_cap`, `resolve_fee_cap`) are
//!   owned by [`defi_evm::rpc`] (L1, already wiremock-tested there). This module
//!   *composes* those primitives plus the batched-simulation (`eth_simulateV1`)
//!   optimization and the per-step/per-chain aggregation; it does NOT re-test the
//!   single-call reads.
//! - **Chain-id helpers** (`parse_evm_chain_id`, `is_tempo_chain`) and the
//!   **Tempo fee-token registry lookup** (`defi_registry::tempo_fee_token`) are
//!   owned elsewhere; this module consumes them and adds the *fee-token symbol
//!   labeling* + the *18→6 decimal Tempo fee conversion* on top.
//! - **Action shape / persistence** is owned by [`crate::action`] /
//!   [`crate::store`]; the estimate types here are an output projection, not a
//!   persisted shape.
//! - **Actual execution** (sign + broadcast + receipt poll) is owned by
//!   [`crate::evm_executor`] / [`crate::tempo_executor`]; this module never signs.
//!
//! =============================================================================
//! SUCCESS CRITERIA (RED phase — written before implementation; the tests in the
//! `#[cfg(test)] mod tests` below reference this module's not-yet-existing public
//! API and MUST fail to compile / fail assertions until GREEN). The Rust port of
//! this module is "correct" iff:
//! =============================================================================
//!
//! ### A. Output shape + JSON contract (machine contract — byte stable)
//! A1. [`ActionGasEstimate`] serializes its fields in Go struct **declaration
//!     order**: `action_id, estimated_at, block_tag, steps, totals_by_chain`.
//! A2. [`ActionGasEstimateStep`] serializes in declaration order: `step_id, type,
//!     status, chain_id, gas_estimate_raw, gas_limit, base_fee_per_gas_wei,
//!     max_priority_fee_per_gas_wei, max_fee_per_gas_wei, effective_gas_price_wei,
//!     likely_fee_wei, worst_case_fee_wei, fee_unit, fee_token`. `fee_unit` and
//!     `fee_token` are `omitempty` (omitted when empty — EVM/non-Tempo steps).
//! A3. [`ActionGasEstimateChainTotal`] serializes in declaration order: `chain_id,
//!     likely_fee_wei, worst_case_fee_wei, fee_unit, fee_token` (last two
//!     `omitempty`).
//! A4. All numeric gas/fee fields are decimal **integer strings** (base units /
//!     wei), never JSON numbers (parity with Go's `big.Int.String()` /
//!     `strconvUint64`). The `type`/`status` enum wire values match the
//!     [`crate::action`] contract (`swap`, `pending`, ...).
//!
//! ### B. Single-step EVM estimate (the core arithmetic — Go
//!        `TestEstimateActionGasSingleStep`)
//! B1. With raw gas `21000`, the default multiplier `1.2` yields `gas_limit ==
//!     25200` (`floor(21000 * 1.2)`); the multiplier truncates toward zero (Go
//!     `uint64(float64(rawGas) * mult)`).
//! B2. Base fee `1 gwei` + suggested tip `2 gwei` ⇒ `base_fee_per_gas_wei ==
//!     "1000000000"`, `max_priority_fee_per_gas_wei == "2000000000"`.
//! B3. Fee cap (no override) `= base*2 + tip = 4 gwei` ⇒ `max_fee_per_gas_wei ==
//!     "4000000000"`.
//! B4. Effective gas price `= min(base + tip, fee_cap) = 3 gwei` ⇒
//!     `effective_gas_price_wei == "3000000000"`.
//! B5. `likely_fee_wei = gas_limit * effective = 25200 * 3e9 == "75600000000000"`;
//!     `worst_case_fee_wei = gas_limit * fee_cap = 25200 * 4e9 == "100800000000000"`.
//! B6. The single chain total mirrors the single step's likely/worst fees, with
//!     `chain_id == "eip155:1"`.
//! B7. `block_tag == "pending"` by default; `estimated_at` is a non-empty RFC3339
//!     UTC timestamp (Go `time.Now().UTC().Format(time.RFC3339)`); `action_id`
//!     passes through unchanged.
//!
//! ### C. Chain-id canonicalization (Go
//!        `TestEstimateActionGasCanonicalizesStepChainID`)
//! C1. A step with an **empty** `chain_id` has its estimate `chain_id` filled from
//!     the RPC `eth_chainId` as `eip155:<n>` (here `eip155:1`); the chain total
//!     carries the same canonical id.
//! C2. A step whose declared `chain_id` does NOT match the RPC chain id is
//!     rejected as [`defi_errors::Code::ActionPlan`] ("step chain mismatch")
//!     (Go `clierr.New(CodeActionPlan, ...)`). Match is case-insensitive.
//!
//! ### D. Step filtering (Go `TestEstimateActionGasFiltersSteps` +
//!        `TestEstimateActionGasFilterNoMatches`)
//! D1. `opts.step_ids = ["second-step"]` estimates ONLY that step (1 step out of
//!     2); the surviving step is the requested one. Filter match is
//!     case-insensitive + whitespace-trimmed (`build_step_filter` /
//!     `matches_step_filter`).
//! D2. A filter that matches NO step is [`defi_errors::Code::Usage`] ("no action
//!     steps matched the requested --step-ids filter").
//! D3. An empty / whitespace-only `step_ids` list is treated as "no filter" (all
//!     steps estimated).
//!
//! ### E. Sequential `eth_simulateV1` optimization + fallback (Go
//!        `TestEstimateActionGasUsesSequentialSimulationWhenAvailable` +
//!        `TestEstimateActionGasFallsBackWhenSequentialSimulationUnavailable`)
//! E1. With ≥2 non-Tempo steps on the same RPC, the estimator calls
//!     `eth_simulateV1` ONCE and uses the per-call `gasUsed` (e.g. `0x5208 →
//!     21000`, `0x1d4c0 → 120000`) as each step's `gas_estimate_raw`, WITHOUT any
//!     legacy `eth_estimateGas` call.
//! E2. When `eth_simulateV1` is unsupported (JSON-RPC `-32601` / "does not
//!     exist"), the estimator falls back to per-step `eth_estimateGas` and still
//!     produces both step estimates (here both `21000`).
//! E3. A single step (or <2 steps on an RPC) never invokes `eth_simulateV1` — it
//!     goes straight to `eth_estimateGas` (Go `len(prepared) < 2` short-circuit).
//!
//! ### F. Tempo fee-token conversion + labeling (Go
//!        `TestEstimateActionGasTempoFeeToken` + `TestEstimateActionGasTempoBatchedCalls`)
//! F1. On a Tempo chain (`4217`), the step carries `fee_unit == "USDC.e"` and a
//!     non-empty `fee_token` (the registry fee-token address); the chain total
//!     carries the same `fee_unit`/`fee_token`.
//! F2. The likely/worst fees are converted from 18-decimal gas pricing to the
//!     6-decimal fee-token base units by dividing by `10^12`: base fee `1e12`,
//!     tip `0` ⇒ effective `1e12`, `gas_limit 25200` ⇒ `likely_fee_wei = 25200 *
//!     1e12 / 1e12 == "25200"`.
//! F3. A Tempo step expressed as **batched `calls`** (empty `target`, ≥2 calls)
//!     estimates EACH call via `eth_estimateGas` and SUMS them: two calls of
//!     `21000` ⇒ `gas_estimate_raw == "42000"`; it still labels `fee_unit ==
//!     "USDC.e"`. Batched Tempo steps are excluded from `eth_simulateV1`.
//! F4. `tempo_fee_token_symbol`: maps the known mainnet USDC.e address to
//!     `"USDC.e"` and the testnet/devnet AlphaUSD address to `"AlphaUSD"`;
//!     an unknown address truncates to `0x<6>...<4>`.
//!
//! ### G. Input validation (Go `EstimateActionGas` guards)
//! G1. A blank `action_id` is [`defi_errors::Code::Usage`] ("missing action id").
//! G2. An action with no steps is [`defi_errors::Code::Usage`] ("action has no
//!     executable steps").
//! G3. `gas_multiplier <= 1.0` is [`defi_errors::Code::Usage`]
//!     ("--gas-multiplier must be > 1").
//! G4. An invalid `from_address` (non-hex) is [`defi_errors::Code::Usage`]
//!     ("action has invalid from_address"); a blank `from_address` is allowed
//!     (uses the zero address as the call sender).
//! G5. A step missing `rpc_url` is [`defi_errors::Code::Usage`] ("missing rpc_url").
//! G6. A non-batched step with an invalid `target` address is
//!     [`defi_errors::Code::Usage`] ("invalid target address").
//! G7. An unknown `block_tag` (not `pending`/`latest`/empty) is
//!     [`defi_errors::Code::Usage`]; empty normalizes to `pending`, and `latest`
//!     passes through.
//!
//! ### H. Defaults (Go `DefaultEstimateOptions`)
//! H1. [`EstimateOptions::default`] sets `gas_multiplier == 1.2` and
//!     `block_tag == Pending`, with empty fee overrides and no step filter.
//!
//! ## Ported Go test cases (and intentional SKIPs)
//! - PORTED: every test in `estimate_test.go`
//!   (`TestEstimateActionGasSingleStep`, `...CanonicalizesStepChainID`,
//!   `...FiltersSteps`, `...FilterNoMatches`,
//!   `...UsesSequentialSimulationWhenAvailable`,
//!   `...FallsBackWhenSequentialSimulationUnavailable`, `...TempoFeeToken`,
//!   `...TempoBatchedCalls`) is re-expressed above (criteria B–F), with the Go
//!   `httptest` JSON-RPC handlers → `wiremock` body-`method` responders.
//! - SKIPPED (owned elsewhere / non-idiomatic to re-test here):
//!     * the exact `eth_estimateGas`/`eth_getBlockByNumber`/`eth_chainId` single
//!       reads + the wei/gwei + `resolveFeeCap`/`resolveTipCap` math →
//!       [`defi_evm::rpc`] (already wiremock-tested there); this module asserts
//!       the *composed* result, not the wire shape of each call.
//!     * the internal `callArgFromCallMsg`/`decodeSimulateBlocks`/
//!       `isSimulateMethodUnsupported` helper plumbing — implementation details;
//!       the observable contract (E1/E2) is asserted through the public estimate.
//!     * `parseNonNegativeBaseUnits` / `decodeHex` — re-implemented privately;
//!       covered indirectly by the value/calldata paths.

#![allow(dead_code)]

use std::collections::HashMap;

use alloy::primitives::U256;
use alloy::rpc::client::RpcClient as AlloyRpcClient;
use alloy::transports::http::reqwest::Url;
use defi_errors::{Code, Error};
use defi_evm::address::{self, Address};
use defi_evm::rpc::{parse_gwei, resolve_fee_cap};
use defi_registry::tempo_fee_token;
use num_bigint::BigUint;
use serde::Serialize;
use serde_json::{json, Value};

use crate::action::{Action, ActionStep, StepCall, StepStatus, StepType};
use crate::evm_executor::{is_tempo_chain, parse_evm_chain_id};
use crate::{EstimateBlockTag, EstimateOptions};

/// The gas/fee estimate for a whole action. Parity with Go `ActionGasEstimate`.
#[derive(Debug, Clone, Serialize)]
pub struct ActionGasEstimate {
    pub action_id: String,
    pub estimated_at: String,
    pub block_tag: String,
    pub steps: Vec<ActionGasEstimateStep>,
    pub totals_by_chain: Vec<ActionGasEstimateChainTotal>,
}

/// The per-step gas/fee estimate. Parity with Go `ActionGasEstimateStep`;
/// field declaration order + `omitempty` mirror the Go struct.
#[derive(Debug, Clone, Serialize)]
pub struct ActionGasEstimateStep {
    pub step_id: String,
    #[serde(rename = "type")]
    pub step_type: StepType,
    pub status: StepStatus,
    pub chain_id: String,
    pub gas_estimate_raw: String,
    pub gas_limit: String,
    pub base_fee_per_gas_wei: String,
    pub max_priority_fee_per_gas_wei: String,
    pub max_fee_per_gas_wei: String,
    pub effective_gas_price_wei: String,
    pub likely_fee_wei: String,
    pub worst_case_fee_wei: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fee_unit: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fee_token: String,
}

/// The per-chain fee totals. Parity with Go `ActionGasEstimateChainTotal`.
#[derive(Debug, Clone, Serialize)]
pub struct ActionGasEstimateChainTotal {
    pub chain_id: String,
    pub likely_fee_wei: String,
    pub worst_case_fee_wei: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fee_unit: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fee_token: String,
}

/// A prepared estimate step: the resolved client + call messages + canonical
/// chain key. Parity with Go `preparedEstimateStep`.
struct PreparedEstimateStep {
    step: ActionStep,
    msgs: Vec<EstimateCall>,
    chain_key: String,
    rpc_url: String,
}

/// An `eth_call`/`eth_estimateGas`/`eth_simulateV1` call message.
#[derive(Debug, Clone)]
struct EstimateCall {
    from: Address,
    to: Option<Address>,
    value: U256,
    data: Vec<u8>,
}

/// Estimate gas/fees for an action, parity with Go `EstimateActionGas`.
pub async fn estimate_action_gas(
    action: &Action,
    opts: EstimateOptions,
) -> Result<ActionGasEstimate, Error> {
    if action.action_id.trim().is_empty() {
        return Err(Error::new(Code::Usage, "missing action id"));
    }
    if action.steps.is_empty() {
        return Err(Error::new(Code::Usage, "action has no executable steps"));
    }
    if opts.gas_multiplier <= 1.0 {
        return Err(Error::new(Code::Usage, "--gas-multiplier must be > 1"));
    }
    let block_tag = opts.block_tag;

    let from_address = if action.from_address.trim().is_empty() {
        Address::ZERO
    } else if !address::is_hex_address(action.from_address.trim()) {
        return Err(Error::new(Code::Usage, "action has invalid from_address"));
    } else {
        address::parse(action.from_address.trim())?
    };

    let filter = build_step_filter(&opts.step_ids);
    let selected: Vec<&ActionStep> = action
        .steps
        .iter()
        .filter(|s| matches_step_filter(&filter, &s.step_id))
        .collect();
    if selected.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "no action steps matched the requested --step-ids filter",
        ));
    }

    let mut clients: HashMap<String, AlloyRpcClient> = HashMap::new();
    let mut prepared: Vec<PreparedEstimateStep> = Vec::with_capacity(selected.len());

    for step in &selected {
        let rpc_url = step.rpc_url.trim().to_string();
        if rpc_url.is_empty() {
            return Err(Error::new(
                Code::Usage,
                format!("step {} is missing rpc_url", step.step_id),
            ));
        }
        let has_calls = !step.calls.is_empty();
        if !has_calls
            && (step.target.trim().is_empty() || !address::is_hex_address(step.target.trim()))
        {
            return Err(Error::new(
                Code::Usage,
                format!("step {} has invalid target address", step.step_id),
            ));
        }

        let client = match clients.get(&rpc_url) {
            Some(c) => c.clone(),
            None => {
                let c = connect_rpc(&rpc_url)?;
                clients.insert(rpc_url.clone(), c.clone());
                c
            }
        };

        let chain_id = read_chain_id(&client).await?;
        let chain_key = format!("eip155:{chain_id}");
        if !step.chain_id.trim().is_empty()
            && !step.chain_id.trim().eq_ignore_ascii_case(&chain_key)
        {
            return Err(Error::new(
                Code::ActionPlan,
                format!(
                    "step chain mismatch: expected {chain_key}, got {}",
                    step.chain_id
                ),
            ));
        }

        let msgs: Vec<EstimateCall> = if has_calls {
            let mut out = Vec::with_capacity(step.calls.len());
            for c in &step.calls {
                out.push(step_call_to_call_msg(c, from_address)?);
            }
            out
        } else {
            vec![action_step_call_msg(step, from_address)?]
        };

        prepared.push(PreparedEstimateStep {
            step: (*step).clone(),
            msgs,
            chain_key,
            rpc_url,
        });
    }

    // Sequential eth_simulateV1 where supported (non-Tempo single-call steps).
    let non_tempo: Vec<&PreparedEstimateStep> = prepared
        .iter()
        .filter(|ps| {
            let cid = parse_evm_chain_id(&ps.chain_key).unwrap_or(0);
            !is_tempo_chain(cid) && ps.msgs.len() <= 1
        })
        .collect();
    let raw_from_simulation =
        estimate_gas_sequential_where_supported(&clients, &non_tempo, block_tag).await?;

    let mut by_chain_likely: HashMap<String, BigUint> = HashMap::new();
    let mut by_chain_worst: HashMap<String, BigUint> = HashMap::new();
    let mut by_chain_fee_unit: HashMap<String, String> = HashMap::new();
    let mut by_chain_fee_token: HashMap<String, String> = HashMap::new();
    let mut estimated_steps: Vec<ActionGasEstimateStep> = Vec::with_capacity(prepared.len());

    for ps in &prepared {
        let client = clients
            .get(&ps.rpc_url)
            .ok_or_else(|| Error::new(Code::Internal, "missing rpc client"))?;
        let numeric_chain_id = parse_evm_chain_id(&ps.chain_key).unwrap_or(0);
        let is_tempo = is_tempo_chain(numeric_chain_id);

        let raw_gas: u64 = if is_tempo && ps.msgs.len() > 1 {
            let mut total = 0u64;
            for m in &ps.msgs {
                total =
                    total.saturating_add(estimate_gas_with_block_tag(client, m, block_tag).await?);
            }
            total
        } else {
            let key = ps.step.step_id.trim().to_lowercase();
            match raw_from_simulation.get(&key) {
                Some(&g) if g != 0 => g,
                _ => estimate_gas_with_block_tag(client, &ps.msgs[0], block_tag).await?,
            }
        };

        let gas_limit = (raw_gas as f64 * opts.gas_multiplier) as u64;
        if gas_limit == 0 {
            return Err(Error::new(Code::ActionSim, "estimate gas returned zero"));
        }

        let tip_cap = resolve_tip_cap(client, &opts.max_priority_fee_gwei).await?;
        let base_fee = base_fee_at_block_tag(client, block_tag).await?;
        let fee_cap = resolve_fee_cap(base_fee, tip_cap, &opts.max_fee_gwei)?;

        let mut effective_gas_price = base_fee.saturating_add(tip_cap);
        if effective_gas_price > fee_cap {
            effective_gas_price = fee_cap;
        }

        let gas_limit_bi = BigUint::from(gas_limit);
        let mut likely_fee = &gas_limit_bi * u256_to_biguint(effective_gas_price);
        let mut worst_fee = &gas_limit_bi * u256_to_biguint(fee_cap);

        let mut fee_unit = String::new();
        let mut fee_token = String::new();
        if is_tempo {
            if let Some(ft) = tempo_fee_token(numeric_chain_id) {
                fee_token = ft.to_string();
                fee_unit = tempo_fee_token_symbol(ft);
            }
            if !fee_unit.is_empty() {
                let divisor = BigUint::from(10u64).pow(12);
                likely_fee /= &divisor;
                worst_fee /= &divisor;
            }
        }

        estimated_steps.push(ActionGasEstimateStep {
            step_id: ps.step.step_id.clone(),
            step_type: ps.step.step_type,
            status: ps.step.status,
            chain_id: ps.chain_key.clone(),
            gas_estimate_raw: raw_gas.to_string(),
            gas_limit: gas_limit.to_string(),
            base_fee_per_gas_wei: u256_dec(base_fee),
            max_priority_fee_per_gas_wei: u256_dec(tip_cap),
            max_fee_per_gas_wei: u256_dec(fee_cap),
            effective_gas_price_wei: u256_dec(effective_gas_price),
            likely_fee_wei: likely_fee.to_string(),
            worst_case_fee_wei: worst_fee.to_string(),
            fee_unit: fee_unit.clone(),
            fee_token: fee_token.clone(),
        });

        *by_chain_likely.entry(ps.chain_key.clone()).or_default() += &likely_fee;
        *by_chain_worst.entry(ps.chain_key.clone()).or_default() += &worst_fee;
        if !fee_unit.is_empty() {
            by_chain_fee_unit.insert(ps.chain_key.clone(), fee_unit);
        }
        if !fee_token.is_empty() {
            by_chain_fee_token.insert(ps.chain_key.clone(), fee_token);
        }
    }

    let mut chain_ids: Vec<String> = by_chain_likely.keys().cloned().collect();
    chain_ids.sort();
    let totals: Vec<ActionGasEstimateChainTotal> = chain_ids
        .iter()
        .map(|chain_id| ActionGasEstimateChainTotal {
            chain_id: chain_id.clone(),
            likely_fee_wei: by_chain_likely[chain_id].to_string(),
            worst_case_fee_wei: by_chain_worst[chain_id].to_string(),
            fee_unit: by_chain_fee_unit.get(chain_id).cloned().unwrap_or_default(),
            fee_token: by_chain_fee_token
                .get(chain_id)
                .cloned()
                .unwrap_or_default(),
        })
        .collect();

    Ok(ActionGasEstimate {
        action_id: action.action_id.clone(),
        estimated_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        block_tag: block_tag.as_str().to_string(),
        steps: estimated_steps,
        totals_by_chain: totals,
    })
}

/// Map a known Tempo fee-token address to a human-readable symbol, parity with
/// Go `tempoFeeTokenSymbol`. Unknown addresses truncate to `0xXXXX...XXXX`.
pub fn tempo_fee_token_symbol(addr: &str) -> String {
    let normalized = addr.trim().to_lowercase();
    match normalized.as_str() {
        "0x20c000000000000000000000b9537d11c60e8b50" => "USDC.e".to_string(),
        "0x20c0000000000000000000000000000000000001" => "AlphaUSD".to_string(),
        _ => {
            if normalized.len() >= 10 {
                format!(
                    "{}...{}",
                    &normalized[..6],
                    &normalized[normalized.len() - 4..]
                )
            } else {
                normalized
            }
        }
    }
}

// =============================================================================
// Sequential simulation (eth_simulateV1) + per-step gas estimation.
// =============================================================================

async fn estimate_gas_sequential_where_supported(
    clients: &HashMap<String, AlloyRpcClient>,
    prepared: &[&PreparedEstimateStep],
    block_tag: EstimateBlockTag,
) -> Result<HashMap<String, u64>, Error> {
    if prepared.len() < 2 {
        return Ok(HashMap::new());
    }
    let mut by_rpc: HashMap<String, Vec<&PreparedEstimateStep>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for ps in prepared {
        if !by_rpc.contains_key(&ps.rpc_url) {
            order.push(ps.rpc_url.clone());
        }
        by_rpc.entry(ps.rpc_url.clone()).or_default().push(ps);
    }

    let mut out: HashMap<String, u64> = HashMap::new();
    for rpc_url in order {
        let group = &by_rpc[&rpc_url];
        if group.len() < 2 {
            continue;
        }
        let client = clients
            .get(&rpc_url)
            .ok_or_else(|| Error::new(Code::Internal, "missing rpc client for simulation"))?;
        let (estimates, supported) =
            estimate_gas_sequential_group(client, group, block_tag).await?;
        if !supported {
            continue;
        }
        for (step_id, gas) in estimates {
            out.insert(step_id.trim().to_lowercase(), gas);
        }
    }
    Ok(out)
}

async fn estimate_gas_sequential_group(
    client: &AlloyRpcClient,
    group: &[&PreparedEstimateStep],
    block_tag: EstimateBlockTag,
) -> Result<(HashMap<String, u64>, bool), Error> {
    let calls: Vec<Value> = group
        .iter()
        .map(|ps| call_arg_from_call_msg(&ps.msgs[0]))
        .collect();
    let opts = json!({
        "blockStateCalls": [ { "calls": calls } ],
    });
    let params = json!([opts, block_tag.as_str()]);

    let raw: Result<Value, _> = client
        .request::<Value, Value>("eth_simulateV1", params)
        .await;
    let raw = match raw {
        Ok(v) => v,
        Err(e) => {
            if is_simulate_method_unsupported(&e.to_string()) {
                return Ok((HashMap::new(), false));
            }
            return Err(Error::wrap(
                Code::ActionSim,
                "simulate action (eth_simulateV1)",
                to_cause(e),
            ));
        }
    };

    let blocks = decode_simulate_blocks(&raw)?;
    if blocks.is_empty() {
        return Err(Error::new(
            Code::ActionSim,
            "eth_simulateV1 returned no blocks",
        ));
    }
    let first = &blocks[0];
    if first.len() != group.len() {
        return Err(Error::new(
            Code::ActionSim,
            format!(
                "eth_simulateV1 returned {} calls for {} requested steps",
                first.len(),
                group.len()
            ),
        ));
    }
    let mut out = HashMap::with_capacity(group.len());
    for (i, call) in first.iter().enumerate() {
        let step_id = &group[i].step.step_id;
        if let Some(status) = call.status {
            if status == 0 {
                return Err(Error::new(
                    Code::ActionSim,
                    format!("simulate step {step_id} reverted"),
                ));
            }
        }
        let gas = call.gas_used.ok_or_else(|| {
            Error::new(
                Code::ActionSim,
                format!("simulate step {step_id} did not return gasUsed"),
            )
        })?;
        if gas == 0 {
            return Err(Error::new(
                Code::ActionSim,
                format!("simulate step {step_id} returned zero gas"),
            ));
        }
        out.insert(step_id.clone(), gas);
    }
    Ok((out, true))
}

/// A decoded `eth_simulateV1` per-call result (the subset we read).
struct SimulateCallResult {
    gas_used: Option<u64>,
    status: Option<u64>,
}

fn decode_simulate_blocks(raw: &Value) -> Result<Vec<Vec<SimulateCallResult>>, Error> {
    if raw.is_null() {
        return Err(Error::new(Code::ActionSim, "empty eth_simulateV1 response"));
    }
    let arr = if raw.is_array() {
        raw.as_array().cloned().unwrap_or_default()
    } else {
        vec![raw.clone()]
    };
    let mut blocks = Vec::with_capacity(arr.len());
    for block in arr {
        let calls = block
            .get("calls")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();
        let mut decoded = Vec::with_capacity(calls.len());
        for call in calls {
            decoded.push(SimulateCallResult {
                gas_used: call
                    .get("gasUsed")
                    .and_then(|v| v.as_str())
                    .and_then(hex_to_u64),
                status: call
                    .get("status")
                    .and_then(|v| v.as_str())
                    .and_then(hex_to_u64),
            });
        }
        blocks.push(decoded);
    }
    Ok(blocks)
}

fn is_simulate_method_unsupported(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    if lower.contains("-32601") || lower.contains("-32602") {
        return true;
    }
    if lower.contains("eth_simulatev1") && lower.contains("not") {
        return true;
    }
    lower.contains("method not found")
        || lower.contains("does not exist")
        || lower.contains("unknown method")
        || lower.contains("not available")
}

async fn estimate_gas_with_block_tag(
    client: &AlloyRpcClient,
    msg: &EstimateCall,
    block_tag: EstimateBlockTag,
) -> Result<u64, Error> {
    let arg = estimate_gas_arg(msg);
    let params = json!([arg, block_tag.as_str()]);
    match client
        .request::<Value, Value>("eth_estimateGas", params)
        .await
    {
        Ok(v) => hex_value_to_u64(&v, "estimate gas"),
        Err(_) => {
            // Retry against latest if we were on pending, then fall back.
            if block_tag == EstimateBlockTag::Pending {
                let retry_params = json!([estimate_gas_arg(msg), "latest"]);
                if let Ok(v) = client
                    .request::<Value, Value>("eth_estimateGas", retry_params)
                    .await
                {
                    return hex_value_to_u64(&v, "estimate gas");
                }
            }
            // Final fallback: plain eth_estimateGas with no block tag.
            let plain = json!([estimate_gas_arg(msg)]);
            let v: Value = client
                .request::<Value, Value>("eth_estimateGas", plain)
                .await
                .map_err(|e| Error::wrap(Code::ActionSim, "estimate gas", to_cause(e)))?;
            hex_value_to_u64(&v, "estimate gas")
        }
    }
}

async fn base_fee_at_block_tag(
    client: &AlloyRpcClient,
    block_tag: EstimateBlockTag,
) -> Result<U256, Error> {
    let read = |tag: &'static str| async move {
        client
            .request::<Value, Value>("eth_getBlockByNumber", json!([tag, false]))
            .await
    };
    let block = match read(block_tag.as_str()).await {
        Ok(b) => b,
        Err(_) => {
            if block_tag == EstimateBlockTag::Pending {
                read("latest").await.map_err(|e| {
                    Error::wrap(Code::Unavailable, "fetch latest header", to_cause(e))
                })?
            } else {
                return Err(Error::new(Code::Unavailable, "fetch latest header"));
            }
        }
    };
    match block.get("baseFeePerGas").and_then(|v| v.as_str()) {
        Some(s) => Ok(hex_to_u256(s).unwrap_or_else(|| U256::from(1_000_000_000u64))),
        None => Ok(U256::from(1_000_000_000u64)),
    }
}

async fn resolve_tip_cap(client: &AlloyRpcClient, override_gwei: &str) -> Result<U256, Error> {
    if !override_gwei.trim().is_empty() {
        return parse_gwei(override_gwei)
            .map_err(|e| Error::wrap(Code::Usage, "parse --max-priority-fee-gwei", to_cause(e)));
    }
    match client
        .request_noparams::<Value>("eth_maxPriorityFeePerGas")
        .await
    {
        Ok(v) => Ok(v
            .as_str()
            .and_then(hex_to_u256)
            .unwrap_or_else(|| U256::from(2_000_000_000u64))),
        Err(_) => Ok(U256::from(2_000_000_000u64)),
    }
}

async fn read_chain_id(client: &AlloyRpcClient) -> Result<i64, Error> {
    let v: Value = client
        .request_noparams::<Value>("eth_chainId")
        .await
        .map_err(|e| Error::wrap(Code::Unavailable, "read chain id", to_cause(e)))?;
    v.as_str()
        .and_then(hex_to_u64)
        .map(|n| n as i64)
        .ok_or_else(|| Error::new(Code::Unavailable, "invalid chain id response"))
}

fn connect_rpc(url: &str) -> Result<AlloyRpcClient, Error> {
    let parsed: Url = url
        .parse()
        .map_err(|e| Error::wrap(Code::Unavailable, "connect rpc", to_cause(e)))?;
    Ok(AlloyRpcClient::new_http(parsed))
}

// =============================================================================
// Call-message construction + JSON-RPC arg shaping.
// =============================================================================

fn estimate_gas_arg(msg: &EstimateCall) -> Value {
    let mut arg = serde_json::Map::new();
    arg.insert("from".into(), json!(msg.from.to_hex()));
    if let Some(to) = msg.to {
        arg.insert("to".into(), json!(to.to_hex()));
    }
    if !msg.data.is_empty() {
        arg.insert(
            "data".into(),
            json!(format!("0x{}", hex::encode(&msg.data))),
        );
    }
    arg.insert("value".into(), json!(format!("0x{:x}", msg.value)));
    Value::Object(arg)
}

fn call_arg_from_call_msg(msg: &EstimateCall) -> Value {
    let mut arg = serde_json::Map::new();
    arg.insert("from".into(), json!(msg.from.to_hex()));
    if let Some(to) = msg.to {
        arg.insert("to".into(), json!(to.to_hex()));
    }
    if !msg.data.is_empty() {
        arg.insert(
            "input".into(),
            json!(format!("0x{}", hex::encode(&msg.data))),
        );
    }
    if !msg.value.is_zero() {
        arg.insert("value".into(), json!(format!("0x{:x}", msg.value)));
    }
    Value::Object(arg)
}

fn action_step_call_msg(step: &ActionStep, from: Address) -> Result<EstimateCall, Error> {
    let target = address::parse(step.target.trim())?;
    let data =
        decode_hex(&step.data).map_err(|e| Error::wrap(Code::Usage, "decode step calldata", e))?;
    let value = parse_non_negative_base_units(&step.value)
        .map_err(|e| Error::wrap(Code::Usage, "parse step value", e))?;
    Ok(EstimateCall {
        from,
        to: Some(target),
        value,
        data,
    })
}

fn step_call_to_call_msg(c: &StepCall, from: Address) -> Result<EstimateCall, Error> {
    if c.target.trim().is_empty() || !address::is_hex_address(c.target.trim()) {
        return Err(Error::new(
            Code::Usage,
            "batched call has invalid target address",
        ));
    }
    let target = address::parse(c.target.trim())?;
    let data = decode_hex(&c.data).map_err(|e| Error::wrap(Code::Usage, "decode call data", e))?;
    let value = parse_non_negative_base_units(&c.value)
        .map_err(|e| Error::wrap(Code::Usage, "parse call value", e))?;
    Ok(EstimateCall {
        from,
        to: Some(target),
        value,
        data,
    })
}

// =============================================================================
// Helpers.
// =============================================================================

fn build_step_filter(step_ids: &[String]) -> Option<std::collections::HashSet<String>> {
    if step_ids.is_empty() {
        return None;
    }
    let set: std::collections::HashSet<String> = step_ids
        .iter()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if set.is_empty() {
        None
    } else {
        Some(set)
    }
}

fn matches_step_filter(filter: &Option<std::collections::HashSet<String>>, step_id: &str) -> bool {
    match filter {
        None => true,
        Some(set) => set.contains(&step_id.trim().to_lowercase()),
    }
}

fn parse_non_negative_base_units(raw: &str) -> Result<U256, HexCause> {
    let clean = raw.trim();
    if clean.is_empty() {
        return Ok(U256::ZERO);
    }
    if !clean.bytes().all(|b| b.is_ascii_digit()) {
        return Err(HexCause("invalid base-units integer".to_string()));
    }
    U256::from_str_radix(clean, 10).map_err(|e| HexCause(e.to_string()))
}

fn decode_hex(v: &str) -> Result<Vec<u8>, HexCause> {
    let mut clean = v.trim();
    clean = clean.strip_prefix("0x").unwrap_or(clean);
    clean = clean.strip_prefix("0X").unwrap_or(clean);
    if clean.is_empty() {
        return Ok(Vec::new());
    }
    let padded;
    let body: &str = if !clean.len().is_multiple_of(2) {
        padded = format!("0{clean}");
        &padded
    } else {
        clean
    };
    hex::decode(body).map_err(|e| HexCause(e.to_string()))
}

fn hex_to_u64(s: &str) -> Option<u64> {
    let body = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if body.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(body, 16).ok()
}

fn hex_to_u256(s: &str) -> Option<U256> {
    let body = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if body.is_empty() {
        return Some(U256::ZERO);
    }
    U256::from_str_radix(body, 16).ok()
}

fn hex_value_to_u64(v: &Value, what: &str) -> Result<u64, Error> {
    v.as_str()
        .and_then(hex_to_u64)
        .ok_or_else(|| Error::new(Code::ActionSim, format!("invalid {what} response")))
}

fn u256_to_biguint(v: U256) -> BigUint {
    BigUint::from_bytes_be(&v.to_be_bytes::<32>())
}

fn u256_dec(v: U256) -> String {
    v.to_string()
}

/// A concrete cause carrying an error's display text.
#[derive(Debug)]
struct HexCause(String);

impl std::fmt::Display for HexCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HexCause {}

fn to_cause<E: std::fmt::Display>(e: E) -> HexCause {
    HexCause(e.to_string())
}

#[cfg(test)]
mod tests {
    //! RED phase. These reference the not-yet-implemented public API of this
    //! module. They MUST fail to compile / fail assertions until GREEN.
    //!
    //! All vectors are deterministic and offline. The EVM JSON-RPC endpoint is
    //! mocked with `wiremock` (the Rust analogue of Go's
    //! `estimate_test.go::newEstimateRPCServer`): a single POST responder keyed
    //! off the request body's `method` field, returning `{jsonrpc,id,result}` or
    //! `{jsonrpc,id,error}` exactly like the Go handlers.

    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use defi_errors::Code;
    use serde_json::{json, Value};
    use wiremock::matchers::{body_partial_json, method};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    use crate::action::{
        Action, ActionStatus, ActionStep, Constraints, StepCall, StepStatus, StepType,
    };

    // ---- canonical test addresses ---------------------------------------
    const FROM: &str = "0x00000000000000000000000000000000000000aa";
    const TARGET_BB: &str = "0x00000000000000000000000000000000000000bb";
    const TARGET_CC: &str = "0x00000000000000000000000000000000000000cc";

    // ---- action / step builders (struct literals; no dependency on the
    //      sibling `action` module's constructor, matching the convention in
    //      `evm_executor.rs`/`store.rs`) -----------------------------------

    fn make_action(action_id: &str, steps: Vec<ActionStep>) -> Action {
        Action {
            action_id: action_id.to_string(),
            intent_type: "swap".to_string(),
            provider: String::new(),
            status: ActionStatus::Planned,
            chain_id: "eip155:1".to_string(),
            from_address: FROM.to_string(),
            wallet_id: String::new(),
            wallet_name: String::new(),
            execution_backend: None,
            to_address: String::new(),
            input_amount: String::new(),
            created_at: "2026-05-28T00:00:00Z".to_string(),
            updated_at: "2026-05-28T00:00:00Z".to_string(),
            constraints: Constraints::default(),
            steps,
            metadata: None,
            provider_data: None,
        }
    }

    fn make_step(step_id: &str, chain_id: &str, rpc_url: &str, target: &str) -> ActionStep {
        ActionStep {
            step_id: step_id.to_string(),
            step_type: StepType::Swap,
            status: StepStatus::Pending,
            chain_id: chain_id.to_string(),
            rpc_url: rpc_url.to_string(),
            description: String::new(),
            target: target.to_string(),
            data: "0x".to_string(),
            value: "0".to_string(),
            calls: Vec::new(),
            expected_outputs: None,
            tx_hash: String::new(),
            error: String::new(),
        }
    }

    // ---- wiremock JSON-RPC helpers --------------------------------------
    //
    // The Rust analogue of `estimate_test.go::newEstimateRPCServer`: one POST
    // responder per JSON-RPC method, matched on the request body's `method`.

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

    async fn mock_method_error(server: &MockServer, rpc_method: &str, code: i64, message: &str) {
        Mock::given(method("POST"))
            .and(body_partial_json(json!({ "method": rpc_method })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": { "code": code, "message": message },
            })))
            .mount(server)
            .await;
    }

    /// A latest-header result carrying `baseFeePerGas`.
    fn block_with_base_fee(base_fee_hex: &str) -> Value {
        json!({
            "number": "0x10",
            "baseFeePerGas": base_fee_hex,
        })
    }

    /// Mount the standard single-step EVM responders (chain 1, base fee 1 gwei,
    /// tip 2 gwei, gas 21000), mirroring Go `newEstimateRPCServer`.
    async fn mount_standard_evm(server: &MockServer) {
        mock_method(server, "eth_chainId", json!("0x1")).await;
        mock_method(server, "eth_estimateGas", json!("0x5208")).await; // 21000
        mock_method(server, "eth_maxPriorityFeePerGas", json!("0x77359400")).await; // 2 gwei
        mock_method(
            server,
            "eth_getBlockByNumber",
            block_with_base_fee("0x3b9aca00"), // 1 gwei
        )
        .await;
    }

    // =====================================================================
    // H. Defaults
    // =====================================================================

    #[test]
    fn default_options_match_go_defaults() {
        // H1.
        let opts = EstimateOptions::default();
        assert_eq!(opts.gas_multiplier, 1.2);
        assert_eq!(opts.block_tag, EstimateBlockTag::Pending);
        assert!(opts.step_ids.is_empty());
        assert!(opts.max_fee_gwei.is_empty());
        assert!(opts.max_priority_fee_gwei.is_empty());
    }

    // =====================================================================
    // B. Single-step EVM estimate (the core arithmetic)
    // =====================================================================

    #[tokio::test]
    async fn single_step_estimate_arithmetic_parity() {
        // B1–B7 + A4: ported from Go TestEstimateActionGasSingleStep.
        let server = MockServer::start().await;
        mount_standard_evm(&server).await;

        let action = make_action(
            "act_test",
            vec![make_step("swap-step", "eip155:1", &server.uri(), TARGET_BB)],
        );

        let estimate = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .expect("estimate");

        assert_eq!(estimate.action_id, "act_test");
        assert_eq!(estimate.block_tag, "pending");
        assert!(
            !estimate.estimated_at.is_empty(),
            "estimated_at must be set (RFC3339 UTC)"
        );
        assert!(
            chrono::DateTime::parse_from_rfc3339(&estimate.estimated_at).is_ok(),
            "estimated_at not RFC3339: {}",
            estimate.estimated_at
        );

        assert_eq!(estimate.steps.len(), 1);
        let step = &estimate.steps[0];
        assert_eq!(step.step_id, "swap-step");
        assert_eq!(step.gas_estimate_raw, "21000"); // B (raw)
        assert_eq!(step.gas_limit, "25200"); // B1: floor(21000 * 1.2)
        assert_eq!(step.base_fee_per_gas_wei, "1000000000"); // B2
        assert_eq!(step.max_priority_fee_per_gas_wei, "2000000000"); // B2
        assert_eq!(step.max_fee_per_gas_wei, "4000000000"); // B3
        assert_eq!(step.effective_gas_price_wei, "3000000000"); // B4
        assert_eq!(step.likely_fee_wei, "75600000000000"); // B5
        assert_eq!(step.worst_case_fee_wei, "100800000000000"); // B5

        assert_eq!(estimate.totals_by_chain.len(), 1); // B6
        let total = &estimate.totals_by_chain[0];
        assert_eq!(total.chain_id, "eip155:1");
        assert_eq!(total.likely_fee_wei, step.likely_fee_wei);
        assert_eq!(total.worst_case_fee_wei, step.worst_case_fee_wei);
    }

    // =====================================================================
    // A. Output shape + JSON contract
    // =====================================================================

    #[tokio::test]
    async fn estimate_json_preserves_declaration_order_and_omits_evm_fee_meta() {
        // A1–A3: field declaration order; fee_unit/fee_token omitted on EVM steps.
        let server = MockServer::start().await;
        mount_standard_evm(&server).await;
        let action = make_action(
            "act_json",
            vec![make_step("swap-step", "eip155:1", &server.uri(), TARGET_BB)],
        );
        let estimate = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .expect("estimate");

        let body = serde_json::to_string(&estimate).expect("marshal");
        let top_order = [
            "action_id",
            "estimated_at",
            "block_tag",
            "steps",
            "totals_by_chain",
        ];
        assert_in_order(&body, &top_order);

        let step_body = serde_json::to_string(&estimate.steps[0]).expect("marshal step");
        let step_order = [
            "step_id",
            "type",
            "status",
            "chain_id",
            "gas_estimate_raw",
            "gas_limit",
            "base_fee_per_gas_wei",
            "max_priority_fee_per_gas_wei",
            "max_fee_per_gas_wei",
            "effective_gas_price_wei",
            "likely_fee_wei",
            "worst_case_fee_wei",
        ];
        assert_in_order(&step_body, &step_order);
        // A2: fee_unit/fee_token are omitempty -> absent on an EVM step.
        assert!(
            !step_body.contains("fee_unit"),
            "fee_unit must be omitted on EVM step: {step_body}"
        );
        assert!(
            !step_body.contains("fee_token"),
            "fee_token must be omitted on EVM step: {step_body}"
        );
        // A4: enum wire values + integer-string numerics.
        assert!(step_body.contains("\"type\":\"swap\""), "got: {step_body}");
        assert!(
            step_body.contains("\"status\":\"pending\""),
            "got: {step_body}"
        );
        assert!(
            step_body.contains("\"gas_estimate_raw\":\"21000\""),
            "numerics must be strings: {step_body}"
        );

        let total_body =
            serde_json::to_string(&estimate.totals_by_chain[0]).expect("marshal total");
        assert_in_order(
            &total_body,
            &["chain_id", "likely_fee_wei", "worst_case_fee_wei"],
        );
    }

    /// Assert `keys` appear in the given relative order within `body`.
    fn assert_in_order(body: &str, keys: &[&str]) {
        let mut last = 0usize;
        for key in keys {
            let needle = format!("\"{key}\":");
            let pos = body
                .find(&needle)
                .unwrap_or_else(|| panic!("missing key {key} in: {body}"));
            assert!(
                pos >= last,
                "key `{key}` out of declaration order in: {body}"
            );
            last = pos;
        }
    }

    // =====================================================================
    // C. Chain-id canonicalization
    // =====================================================================

    #[tokio::test]
    async fn empty_step_chain_id_is_canonicalized_from_rpc() {
        // C1: ported from Go TestEstimateActionGasCanonicalizesStepChainID.
        let server = MockServer::start().await;
        mount_standard_evm(&server).await;
        // Step declares an empty chain id; the RPC reports chain 1.
        let action = make_action(
            "act_chain",
            vec![make_step("swap-step", "", &server.uri(), TARGET_BB)],
        );
        let estimate = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .expect("estimate");
        assert_eq!(estimate.steps[0].chain_id, "eip155:1");
        assert_eq!(estimate.totals_by_chain[0].chain_id, "eip155:1");
    }

    #[tokio::test]
    async fn mismatched_step_chain_id_is_action_plan_error() {
        // C2: declared chain id disagrees with the RPC chain id.
        let server = MockServer::start().await;
        mount_standard_evm(&server).await; // RPC reports chain 1
        let action = make_action(
            "act_chain_mismatch",
            vec![make_step(
                "swap-step",
                "eip155:137",
                &server.uri(),
                TARGET_BB,
            )],
        );
        let err = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::ActionPlan);
    }

    // =====================================================================
    // D. Step filtering
    // =====================================================================

    #[tokio::test]
    async fn step_filter_estimates_only_requested_step() {
        // D1: ported from Go TestEstimateActionGasFiltersSteps.
        let server = MockServer::start().await;
        mount_standard_evm(&server).await;
        let action = make_action(
            "act_filter",
            vec![
                make_step("first-step", "eip155:1", &server.uri(), TARGET_BB),
                make_step("second-step", "eip155:1", &server.uri(), TARGET_CC),
            ],
        );
        let opts = EstimateOptions {
            step_ids: vec!["second-step".to_string()],
            ..EstimateOptions::default()
        };
        let estimate = estimate_action_gas(&action, opts).await.expect("estimate");
        assert_eq!(estimate.steps.len(), 1);
        assert_eq!(estimate.steps[0].step_id, "second-step");
    }

    #[tokio::test]
    async fn step_filter_is_case_insensitive_and_trimmed() {
        // D1 (matching semantics): "  SECOND-STEP " still selects "second-step".
        let server = MockServer::start().await;
        mount_standard_evm(&server).await;
        let action = make_action(
            "act_filter_ci",
            vec![
                make_step("first-step", "eip155:1", &server.uri(), TARGET_BB),
                make_step("second-step", "eip155:1", &server.uri(), TARGET_CC),
            ],
        );
        let opts = EstimateOptions {
            step_ids: vec!["  SECOND-STEP ".to_string()],
            ..EstimateOptions::default()
        };
        let estimate = estimate_action_gas(&action, opts).await.expect("estimate");
        assert_eq!(estimate.steps.len(), 1);
        assert_eq!(estimate.steps[0].step_id, "second-step");
    }

    #[tokio::test]
    async fn step_filter_no_match_is_usage_error() {
        // D2: ported from Go TestEstimateActionGasFilterNoMatches.
        let server = MockServer::start().await;
        mount_standard_evm(&server).await;
        let action = make_action(
            "act_filter_none",
            vec![make_step("only-step", "eip155:1", &server.uri(), TARGET_BB)],
        );
        let opts = EstimateOptions {
            step_ids: vec!["missing-step".to_string()],
            ..EstimateOptions::default()
        };
        let err = estimate_action_gas(&action, opts).await.unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    #[tokio::test]
    async fn empty_step_ids_is_no_filter() {
        // D3: a whitespace-only step id list does not filter anything out.
        let server = MockServer::start().await;
        mount_standard_evm(&server).await;
        let action = make_action(
            "act_filter_blank",
            vec![make_step("only-step", "eip155:1", &server.uri(), TARGET_BB)],
        );
        let opts = EstimateOptions {
            step_ids: vec!["   ".to_string()],
            ..EstimateOptions::default()
        };
        let estimate = estimate_action_gas(&action, opts).await.expect("estimate");
        assert_eq!(estimate.steps.len(), 1);
    }

    // =====================================================================
    // E. Sequential eth_simulateV1 optimization + fallback
    // =====================================================================

    /// A `method`-routing responder that scripts `eth_simulateV1` and counts
    /// legacy `eth_estimateGas` calls — the Rust analogue of Go's inline handler.
    struct SimRouter {
        legacy_estimate_calls: Arc<AtomicUsize>,
        simulate_supported: bool,
    }

    impl Respond for SimRouter {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let m = body.get("method").and_then(|v| v.as_str()).unwrap_or("");
            let ok = |result: Value| {
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0", "id": 1, "result": result,
                }))
            };
            let err = |code: i64, message: &str| {
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0", "id": 1,
                    "error": { "code": code, "message": message },
                }))
            };
            match m {
                "eth_chainId" => ok(json!("0x1")),
                "eth_simulateV1" => {
                    if self.simulate_supported {
                        ok(json!([{
                            "calls": [
                                { "gasUsed": "0x5208", "status": "0x1" },   // 21000
                                { "gasUsed": "0x1d4c0", "status": "0x1" },  // 120000
                            ]
                        }]))
                    } else {
                        err(
                            -32601,
                            "the method eth_simulateV1 does not exist/is not available",
                        )
                    }
                }
                "eth_estimateGas" => {
                    self.legacy_estimate_calls.fetch_add(1, Ordering::SeqCst);
                    ok(json!("0x5208")) // 21000
                }
                "eth_maxPriorityFeePerGas" => ok(json!("0x77359400")),
                "eth_getBlockByNumber" => ok(block_with_base_fee("0x3b9aca00")),
                _ => err(-32601, "method not supported in test"),
            }
        }
    }

    #[tokio::test]
    async fn uses_sequential_simulation_when_available() {
        // E1: ported from Go TestEstimateActionGasUsesSequentialSimulationWhenAvailable.
        let server = MockServer::start().await;
        let legacy = Arc::new(AtomicUsize::new(0));
        Mock::given(method("POST"))
            .respond_with(SimRouter {
                legacy_estimate_calls: legacy.clone(),
                simulate_supported: true,
            })
            .mount(&server)
            .await;

        let action = make_action(
            "act_seq_sim",
            vec![
                {
                    let mut s = make_step("approve-step", "eip155:1", &server.uri(), TARGET_BB);
                    s.step_type = StepType::Approval;
                    s
                },
                {
                    let mut s = make_step("deposit-step", "eip155:1", &server.uri(), TARGET_CC);
                    s.step_type = StepType::Lend;
                    s
                },
            ],
        );
        let estimate = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .expect("estimate");

        assert_eq!(estimate.steps.len(), 2);
        assert_eq!(estimate.steps[0].gas_estimate_raw, "21000");
        assert_eq!(estimate.steps[1].gas_estimate_raw, "120000");
        assert_eq!(
            legacy.load(Ordering::SeqCst),
            0,
            "no legacy eth_estimateGas calls when eth_simulateV1 is available"
        );
    }

    #[tokio::test]
    async fn falls_back_to_legacy_estimate_when_simulation_unavailable() {
        // E2: ported from Go TestEstimateActionGasFallsBackWhenSequentialSimulationUnavailable.
        let server = MockServer::start().await;
        let legacy = Arc::new(AtomicUsize::new(0));
        Mock::given(method("POST"))
            .respond_with(SimRouter {
                legacy_estimate_calls: legacy.clone(),
                simulate_supported: false,
            })
            .mount(&server)
            .await;

        let action = make_action(
            "act_seq_fallback",
            vec![
                {
                    let mut s = make_step("approve-step", "eip155:1", &server.uri(), TARGET_BB);
                    s.step_type = StepType::Approval;
                    s
                },
                {
                    let mut s = make_step("deposit-step", "eip155:1", &server.uri(), TARGET_CC);
                    s.step_type = StepType::Lend;
                    s
                },
            ],
        );
        let estimate = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .expect("estimate");

        assert_eq!(estimate.steps.len(), 2);
        assert_eq!(estimate.steps[0].gas_estimate_raw, "21000");
        assert_eq!(estimate.steps[1].gas_estimate_raw, "21000");
        assert!(
            legacy.load(Ordering::SeqCst) >= 2,
            "both steps must fall back to legacy eth_estimateGas"
        );
    }

    // =====================================================================
    // F. Tempo fee-token conversion + labeling
    // =====================================================================

    /// Mount the Tempo single-step responders (chain 4217, base fee 1e12 in
    /// 18-decimal USD pricing, zero tip, gas 21000), mirroring Go's Tempo handler.
    async fn mount_tempo_evm(server: &MockServer) {
        mock_method(server, "eth_chainId", json!("0x1079")).await; // 4217
        mock_method(server, "eth_estimateGas", json!("0x5208")).await; // 21000
        mock_method(server, "eth_maxPriorityFeePerGas", json!("0x0")).await; // 0 tip
        mock_method(
            server,
            "eth_getBlockByNumber",
            block_with_base_fee("0xe8d4a51000"), // 1e12
        )
        .await;
    }

    #[tokio::test]
    async fn tempo_fee_token_conversion_and_labeling() {
        // F1 + F2: ported from Go TestEstimateActionGasTempoFeeToken.
        let server = MockServer::start().await;
        mount_tempo_evm(&server).await;
        let action = make_action(
            "act_tempo_fee",
            vec![make_step(
                "swap-step",
                "eip155:4217",
                &server.uri(),
                TARGET_BB,
            )],
        );
        let estimate = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .expect("estimate");

        assert_eq!(estimate.steps.len(), 1);
        let step = &estimate.steps[0];
        assert_eq!(step.fee_unit, "USDC.e"); // F1
        assert!(!step.fee_token.is_empty(), "fee_token must be set on Tempo");
        // F2: 25200 * 1e12 / 1e12 == 25200 base units.
        assert_eq!(step.likely_fee_wei, "25200");

        assert_eq!(estimate.totals_by_chain.len(), 1);
        let total = &estimate.totals_by_chain[0];
        assert_eq!(total.fee_unit, "USDC.e"); // F1 (chain total)
        assert!(!total.fee_token.is_empty());
    }

    #[tokio::test]
    async fn tempo_batched_calls_sum_per_call_gas() {
        // F3: ported from Go TestEstimateActionGasTempoBatchedCalls.
        let server = MockServer::start().await;
        mount_tempo_evm(&server).await;
        let mut step = make_step("batch-step", "eip155:4217", &server.uri(), "");
        step.target = String::new();
        step.data = String::new();
        step.calls = vec![
            StepCall {
                target: TARGET_BB.to_string(),
                data: "0x".to_string(),
                value: "0".to_string(),
            },
            StepCall {
                target: TARGET_CC.to_string(),
                data: "0x".to_string(),
                value: "0".to_string(),
            },
        ];
        let action = make_action("act_tempo_batch", vec![step]);
        let estimate = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .expect("estimate");

        assert_eq!(estimate.steps.len(), 1);
        // Two calls of 21000 each => raw gas 42000.
        assert_eq!(estimate.steps[0].gas_estimate_raw, "42000");
        assert_eq!(estimate.steps[0].fee_unit, "USDC.e");
    }

    #[test]
    fn tempo_fee_token_symbol_labels_known_addresses() {
        // F4: known mainnet/testnet labels + truncated unknown.
        assert_eq!(
            tempo_fee_token_symbol("0x20c000000000000000000000b9537d11c60e8b50"),
            "USDC.e"
        );
        assert_eq!(
            tempo_fee_token_symbol("0x20C000000000000000000000B9537D11C60E8B50"),
            "USDC.e",
            "address labeling must be case-insensitive"
        );
        assert_eq!(
            tempo_fee_token_symbol("0x20c0000000000000000000000000000000000001"),
            "AlphaUSD"
        );
        let unknown = tempo_fee_token_symbol("0x1234567890abcdef1234567890abcdef12345678");
        assert_eq!(unknown, "0x1234...5678", "unknown address truncates");
    }

    // =====================================================================
    // G. Input validation (no RPC needed)
    // =====================================================================

    #[tokio::test]
    async fn blank_action_id_is_usage_error() {
        // G1: a whitespace-only action id is rejected (Go trims then checks empty).
        let action = make_action(
            "   ",
            vec![make_step("s", "eip155:1", "http://unused", TARGET_BB)],
        );
        let err = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    #[tokio::test]
    async fn action_with_no_steps_is_usage_error() {
        // G2.
        let action = make_action("act_empty", vec![]);
        let err = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    #[tokio::test]
    async fn gas_multiplier_not_greater_than_one_is_usage_error() {
        // G3.
        let action = make_action(
            "act_mult",
            vec![make_step("s", "eip155:1", "http://unused", TARGET_BB)],
        );
        let opts = EstimateOptions {
            gas_multiplier: 1.0,
            ..EstimateOptions::default()
        };
        let err = estimate_action_gas(&action, opts).await.unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    #[tokio::test]
    async fn invalid_from_address_is_usage_error() {
        // G4.
        let mut action = make_action(
            "act_from",
            vec![make_step("s", "eip155:1", "http://unused", TARGET_BB)],
        );
        action.from_address = "not-an-address".to_string();
        let err = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    #[tokio::test]
    async fn blank_from_address_is_allowed() {
        // G4 (allowed half): a blank from_address uses the zero address sender.
        let server = MockServer::start().await;
        mount_standard_evm(&server).await;
        let mut action = make_action(
            "act_blank_from",
            vec![make_step("swap-step", "eip155:1", &server.uri(), TARGET_BB)],
        );
        action.from_address = String::new();
        let estimate = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .expect("blank from_address is allowed");
        assert_eq!(estimate.steps.len(), 1);
    }

    #[tokio::test]
    async fn step_missing_rpc_url_is_usage_error() {
        // G5.
        let action = make_action(
            "act_no_rpc",
            vec![make_step("s", "eip155:1", "", TARGET_BB)],
        );
        let err = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    #[tokio::test]
    async fn step_with_invalid_target_is_usage_error() {
        // G6: a non-batched step with a bad target address (no RPC dial needed —
        // validated before estimation).
        let action = make_action(
            "act_bad_target",
            vec![make_step(
                "s",
                "eip155:1",
                "http://unused",
                "not-an-address",
            )],
        );
        let err = estimate_action_gas(&action, EstimateOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn block_tag_normalization_parity() {
        // G7: the enum makes invalid block-tag states unrepresentable, so the
        // unknown-tag rejection lives at parse time (the CLI parses the
        // `--block-tag` flag through this entry point before building options):
        // empty -> pending; pending/latest pass through; unknown -> Err.
        assert_eq!(
            EstimateBlockTag::from_str("").unwrap(),
            EstimateBlockTag::Pending
        );
        assert_eq!(
            EstimateBlockTag::from_str("pending").unwrap(),
            EstimateBlockTag::Pending
        );
        assert_eq!(
            EstimateBlockTag::from_str("latest").unwrap(),
            EstimateBlockTag::Latest
        );
        assert_eq!(
            EstimateBlockTag::from_str("LATEST").unwrap(),
            EstimateBlockTag::Latest,
            "block tag parsing must be case-insensitive"
        );
        assert!(EstimateBlockTag::from_str("finalized").is_err());
    }

    #[test]
    fn block_tag_wire_value_parity() {
        // The block tag serializes to its lowercase wire string for the output
        // `block_tag` field.
        assert_eq!(EstimateBlockTag::Pending.as_str(), "pending");
        assert_eq!(EstimateBlockTag::Latest.as_str(), "latest");
    }
}
