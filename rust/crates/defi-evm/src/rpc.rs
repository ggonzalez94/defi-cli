//! JSON-RPC client wrapper + gas/fee math. Scaffold stub — Phase 2 (RED).
//!
//! This module owns the EVM JSON-RPC half of the machine contract that the Go
//! tree reached for via go-ethereum's `ethclient`/`rpc` packages. Every on-chain
//! read and broadcast the CLI performs funnels through `ethclient.Client`:
//!
//! - `chains gas` (`internal/app/runner.go::fetchGasPrice`) reads the latest
//!   header (block number + EIP-1559 base fee), `SuggestGasPrice`, and
//!   `SuggestGasTipCap`, then formats wei → gwei with [`wei_to_gwei`].
//! - the EVM executor (`internal/execution/evm_executor.go`) reads `ChainID`,
//!   simulates with `CallContract` (`eth_call`), `EstimateGas`, reads the latest
//!   header base fee, `PendingNonceAt`, broadcasts via `SendTransaction`, and
//!   polls `TransactionReceipt`.
//! - gas-fee math (`internal/execution/executor.go::{resolveTipCap,resolveFeeCap,
//!   parseGwei}`) turns RPC-suggested values + `--max-fee-gwei` /
//!   `--max-priority-fee-gwei` overrides into the EIP-1559 `GasTipCap`/`GasFeeCap`
//!   that go straight into the signed `DynamicFeeTx`.
//!
//! The idiomatic Rust port wraps `alloy`'s provider stack behind a small
//! [`RpcClient`] type so the same JSON-RPC calls reach the same endpoints. The
//! default-RPC-URL map (`internal/registry/rpc.go`) lives in `defi-registry`
//! (L2), NOT here; this module owns the *client* + the wei/gwei + fee-cap math.
//!
//! Output of `chains gas` is part of the JSON contract (`model::GasPrice`,
//! 6-decimal gwei strings); the broadcast `DynamicFeeTx` fee fields are
//! consumed by `defi-execution`'s signer. Both must stay byte-stable, so the
//! math below is golden-tested against go-ethereum's `big.Float.Text('f', 6)`
//! and `big.Rat`-based `parseGwei` semantics.
//!
//! # Success criteria (contract this module must preserve)
//!
//! 1. **`wei_to_gwei` formatting parity with Go `weiToGwei`** — [`wei_to_gwei`]:
//!    divides the wei amount by `1e9` and renders with **exactly 6 fractional
//!    digits** (`big.Float.Text('f', 6)`). Verified vectors (from
//!    `runner_gas_test.go::TestWeiToGwei`):
//!      - `None` (Go `nil`) → `"0"` (NOT `"0.000000"`);
//!      - `0` → `"0.000000"`;
//!      - `1_000_000_000` (1 gwei) → `"1.000000"`;
//!      - `30_500_000_000` → `"30.500000"`;
//!      - `500_000` (sub-gwei) → `"0.000500"`.
//!
//!    Large values beyond `u128`/`u64` (real base fees are small, but the type is
//!    `U256`) must still render exactly, with no scientific notation and no loss.
//!
//! 2. **`parse_gwei` parity with Go `parseGwei`** — [`parse_gwei`]: parses a
//!    decimal gwei string and returns the equivalent **wei** as an integer.
//!      - `"1"` → `1_000_000_000`; `"2"` → `2_000_000_000`;
//!      - `"0.000000001"` → `1` (1 wei, the smallest representable);
//!      - `"1.5"` → `1_500_000_000`;
//!      - empty/whitespace → `Err`;
//!      - non-numeric (`"abc"`) → `Err`;
//!      - negative (`"-1"`) → `Err` ("value must be non-negative");
//!      - a value that does NOT resolve to an integer wei amount
//!        (`"0.0000000001"`, i.e. 0.1 wei) → `Err` ("must resolve to an integer
//!        wei amount"). go-ethereum uses `big.Rat`; the port must not silently
//!        truncate sub-wei precision.
//!
//! 3. **`resolve_tip_cap` override + RPC-suggested fallback parity** —
//!    [`resolve_tip_cap`]:
//!      - when an override gwei string is given, returns `parse_gwei(override)`
//!        (and surfaces a typed usage error if it is malformed);
//!      - with no override, returns the client's `eth_maxPriorityFeePerGas`
//!        (`SuggestGasTipCap`) value;
//!      - if that RPC call errors, falls back to **2 gwei**
//!        (`2_000_000_000` wei) and does NOT error (matches Go's silent
//!        fallback).
//!
//! 4. **`resolve_fee_cap` parity with Go `resolveFeeCap`** — [`resolve_fee_cap`]:
//!      - no override → `baseFee*2 + tipCap`;
//!      - override → `parse_gwei(override)`, but a typed usage error if the
//!        override resolves below `tipCap` ("--max-fee-gwei must be >=
//!        --max-priority-fee-gwei");
//!      - a malformed override → typed usage error.
//!
//! 5. **JSON-RPC client reads parity with `ethclient`** — [`RpcClient`] built
//!    from an HTTP URL ([`RpcClient::connect`]) performs the same JSON-RPC
//!    method calls go-ethereum did, decoded identically (wiremock-mocked, the
//!    Rust analogue of `runner_gas_test.go::newMockRPCServer`):
//!      - [`RpcClient::chain_id`] ← `eth_chainId`;
//!      - [`RpcClient::block_number`] ← latest-header `number`
//!        (`HeaderByNumber(nil)`), e.g. `0x10` → `16`;
//!      - [`RpcClient::base_fee`] ← latest-header `baseFeePerGas`; **`None`** when
//!        the header omits it (legacy chains — the `eip1559=false` signal);
//!      - [`RpcClient::gas_price`] ← `eth_gasPrice` (`SuggestGasPrice`);
//!      - [`RpcClient::max_priority_fee`] ← `eth_maxPriorityFeePerGas`
//!        (`SuggestGasTipCap`); a JSON-RPC error result surfaces as `Err`
//!        (the caller decides the fallback, per criteria 3 and the
//!        `chains gas` warning path).
//!
//! 6. **JSON-RPC execution primitives parity** — [`RpcClient`] also exposes the
//!    write/estimate path the executor used:
//!      - [`RpcClient::pending_nonce`] ← `eth_getTransactionCount(addr,
//!        "pending")` (`PendingNonceAt`);
//!      - [`RpcClient::estimate_gas`] ← `eth_estimateGas` (`EstimateGas`);
//!      - [`RpcClient::call`] ← `eth_call` (`CallContract`), returning the raw
//!        return bytes;
//!      - [`RpcClient::send_raw_transaction`] ← `eth_sendRawTransaction`
//!        (`SendTransaction`), returning the 32-byte tx hash;
//!      - [`RpcClient::transaction_receipt`] ← `eth_getTransactionReceipt`,
//!        returning `None` when the receipt is not yet available (go-ethereum's
//!        `ethereum.NotFound`, the executor's poll-until-mined signal).
//!
//! 7. **Typed, no-panic error surface** — connecting to an unreachable endpoint
//!    or a transport failure yields a `defi_errors`-typed [`crate::Error`] with
//!    [`defi_errors::Code::Unavailable`] (Go wrapped these as
//!    `clierr.Wrap(CodeUnavailable, "connect rpc"/"read chain id"/...)`); an
//!    invalid HTTP URL is rejected without panic. No `unwrap`/`expect`/`panic`
//!    in non-test library code.
//!
//! The receipt-polling loop, nonce-locking, simulate-via-`eth_simulateV1`
//! batching, and revert-reason decoding are orchestration that lives in
//! `defi-execution` (L3) built on these primitives; they are intentionally NOT
//! re-tested here — this module owns only the single-call RPC wrapper + the
//! deterministic wei/gwei + fee-cap math.

use alloy::eips::eip2718::Encodable2718;
use alloy::primitives::{Bytes, TxKind, B256, U256};
use alloy::rpc::client::RpcClient as AlloyRpcClient;
use alloy::transports::http::reqwest::Url;
use defi_errors::{Code, Error};
use num_bigint::BigUint;
use serde_json::{json, Value};

use crate::address::Address;
use crate::signer::SignedTx;

/// One gwei expressed in wei (`10^9`).
const WEI_PER_GWEI: u64 = 1_000_000_000;
/// The number of fractional digits `weiToGwei` renders (`big.Float.Text('f', 6)`).
const GWEI_DECIMALS: u32 = 6;
/// Minimum mantissa precision (in bits) of the `big.Float` quotient Go computes.
///
/// Go's `weiToGwei` does `new(big.Float).SetInt(wei)` (precision
/// `max(wei.BitLen(), 64)`) then `Quo(.., big.NewFloat(1e9))` (precision 53);
/// the quotient inherits `max(operand precisions)`, i.e. `max(wei.BitLen(), 64)`.
/// For every realistic wei value (`< 2^64`) that floor of **64** applies, so the
/// gwei string is the decimal rendering of a 64-bit-mantissa binary float — *not*
/// of the exact rational. This is why exact-half decimal ties (e.g. `500` wei =
/// `0.0000005` gwei) can round either way: the binary float that approximates the
/// rational decides. Larger `U256` inputs use their full bit length as the
/// precision, matching `SetInt` exactly.
const GWEI_FLOAT_PREC_BITS: u32 = 64;
/// Default EIP-1559 priority-fee tip when the node lacks
/// `eth_maxPriorityFeePerGas` (Go: `big.NewInt(2_000_000_000)`).
const DEFAULT_TIP_CAP_WEI: u64 = 2_000_000_000;

// =============================================================================
// Pure math (criteria 1–4): wei/gwei conversion + EIP-1559 fee-cap resolution.
// =============================================================================

/// Format a wei amount as a gwei string, parity with go-ethereum `weiToGwei`.
///
/// `None` (the Go `nil` base-fee / priority-fee sentinel) renders as the bare
/// `"0"`. Any concrete amount is divided by `10^9` and rendered with **exactly
/// 6** fractional digits, never scientific notation, byte-for-byte identical to
/// go-ethereum's `new(big.Float).SetInt(wei).Quo(.., big.NewFloat(1e9)).Text('f',
/// 6)`.
///
/// The Go code rounds through a binary float whose mantissa precision is
/// `max(wei.BitLen(), 64)` bits (`SetInt`'s precision, inherited by the quotient
/// since it exceeds the divisor's 53), so the output is *not* a clean decimal
/// round-half-even of `wei/1e9`: at exact-half decimal ties (e.g. `500` wei →
/// `0.000001`, `1500` wei → `0.000001`, `13500` wei → `0.000014`) the tie is
/// broken by where the binary approximation falls. This implementation
/// reproduces both rounding steps exactly with arbitrary-precision integer
/// arithmetic (no native float), so it matches Go on every value across the full
/// `U256` range, including those binary ties — see the regression-oracle tests.
pub fn wei_to_gwei(wei: Option<U256>) -> String {
    let Some(wei) = wei else {
        return "0".to_string();
    };
    let wei = u256_to_biguint(wei);

    // Step 1: round wei / 1e9 to a binary float with the same mantissa precision
    // Go's big.Float quotient uses: max(SetInt precision, 53) = max(wei bits, 64).
    let prec = wei.bits().max(u64::from(GWEI_FLOAT_PREC_BITS)) as u32;
    let (mantissa, exp2) = round_ratio_to_binary_float(&wei, &BigUint::from(WEI_PER_GWEI), prec);

    // Step 2: render that binary float (mantissa * 2^exp2) to GWEI_DECIMALS
    // fractional digits, exactly as big.Float.Text('f', 6) does.
    let scaled = scale_binary_float_to_decimal(&mantissa, exp2, GWEI_DECIMALS);
    let denom = BigUint::from(10u64).pow(GWEI_DECIMALS);
    let whole = &scaled / &denom;
    let frac = &scaled % &denom;
    format!("{whole}.{:0>width$}", frac, width = GWEI_DECIMALS as usize)
}

/// Convert an alloy `U256` to a `num-bigint` `BigUint` via its big-endian bytes.
fn u256_to_biguint(v: U256) -> BigUint {
    BigUint::from_bytes_be(&v.to_be_bytes::<32>())
}

/// Round the exact rational `num/den` to a binary float with `prec` significant
/// mantissa bits, round-half-to-even — the rounding Go's `big.Float` applies.
///
/// Returns `(mantissa, exp2)` such that the rounded value equals
/// `mantissa * 2^exp2` and `mantissa` has at most `prec` bits. Zero maps to
/// `(0, 0)`.
fn round_ratio_to_binary_float(num: &BigUint, den: &BigUint, prec: u32) -> (BigUint, i32) {
    if num == &BigUint::ZERO {
        return (BigUint::ZERO, 0);
    }

    // Find the binary exponent so the leading mantissa bit aligns: we want
    // `q = round(num * 2^shift / den)` to occupy exactly `prec` bits.
    //
    // Start by computing the integer quotient's bit length, then choose a shift
    // that yields a `prec`-bit result.
    let num_bits = num.bits() as i64;
    let den_bits = den.bits() as i64;
    // Rough binary exponent of num/den (may be off by one; the carry loop below
    // corrects it). We want the quotient to have `prec` bits, so scale num up by
    // `prec - approx_value_bits` before dividing.
    let approx_value_bits = num_bits - den_bits;
    let mut shift = prec as i64 - approx_value_bits;

    let round_div = |shift: i64| -> (BigUint, i64) {
        // value = num * 2^shift / den, rounded half-to-even.
        let (scaled_num, eff_shift) = if shift >= 0 {
            (num << (shift as u32), shift)
        } else {
            (num.clone(), shift)
        };
        // If shift is negative we instead divide num by 2^(-shift) * den.
        let (n, d) = if shift >= 0 {
            (scaled_num, den.clone())
        } else {
            (num.clone(), den << ((-shift) as u32))
        };
        let q = &n / &d;
        let r = &n % &d;
        let twice_r = &r * BigUint::from(2u64);
        let rounded = match twice_r.cmp(&d) {
            std::cmp::Ordering::Greater => q + BigUint::from(1u64),
            std::cmp::Ordering::Less => q,
            std::cmp::Ordering::Equal => {
                if (&q % BigUint::from(2u64)) == BigUint::ZERO {
                    q
                } else {
                    q + BigUint::from(1u64)
                }
            }
        };
        (rounded, eff_shift)
    };

    let (mut mantissa, _) = round_div(shift);
    // Rounding up can carry into an extra bit (e.g. 0b111..1 -> 0b1000..0); if
    // the mantissa now exceeds `prec` bits, divide by two more and re-round.
    while mantissa.bits() as u32 > prec {
        shift -= 1;
        let (m, _) = round_div(shift);
        mantissa = m;
    }
    // Drop trailing zero bits so the (mantissa, exp2) pair is normalized; this
    // does not change the represented value.
    let exp2 = -shift as i32;
    (mantissa, exp2)
}

/// Render the binary float `mantissa * 2^exp2` to a fixed-point integer with
/// `decimals` fractional decimal digits, round-half-to-even — exactly as
/// `big.Float.Text('f', decimals)` does.
///
/// Returns `round(mantissa * 2^exp2 * 10^decimals)` as a `BigUint`.
fn scale_binary_float_to_decimal(mantissa: &BigUint, exp2: i32, decimals: u32) -> BigUint {
    let pow10 = BigUint::from(10u64).pow(decimals);
    // target = mantissa * 2^exp2 * 10^decimals, exact rational num/den.
    let base = mantissa * &pow10;
    let (num, den) = if exp2 >= 0 {
        (base << (exp2 as u32), BigUint::from(1u64))
    } else {
        (base, BigUint::from(1u64) << ((-exp2) as u32))
    };
    let q = &num / &den;
    let r = &num % &den;
    let twice_r = &r * BigUint::from(2u64);
    match twice_r.cmp(&den) {
        std::cmp::Ordering::Greater => q + BigUint::from(1u64),
        std::cmp::Ordering::Less => q,
        std::cmp::Ordering::Equal => {
            if (&q % BigUint::from(2u64)) == BigUint::ZERO {
                q
            } else {
                q + BigUint::from(1u64)
            }
        }
    }
}

/// Parse a decimal gwei string into the equivalent wei amount, parity with
/// go-ethereum `parseGwei` (which uses `big.Rat`).
///
/// Trims surrounding whitespace, then requires a non-negative decimal value
/// whose `value * 10^9` is an exact integer number of wei. Rejects empty /
/// whitespace-only, non-numeric, negative, and sub-wei-precision inputs with a
/// typed [`Error`] rather than truncating.
pub fn parse_gwei(v: &str) -> Result<U256, Error> {
    let clean = v.trim();
    if clean.is_empty() {
        return Err(Error::new(Code::Usage, "empty gwei value"));
    }

    // Reject a leading sign: negatives are invalid, and a `+` is not part of the
    // decimal grammar the Go contract accepts.
    if let Some(first) = clean.chars().next() {
        if first == '-' {
            return Err(Error::new(Code::Usage, "value must be non-negative"));
        }
        if first == '+' {
            return Err(Error::new(
                Code::Usage,
                format!("invalid numeric value {v:?}"),
            ));
        }
    }

    let (int_part, frac_part) = match clean.split_once('.') {
        Some((i, f)) => (i, f),
        None => (clean, ""),
    };

    // A bare "." or interior "1.2.3" / non-digit characters are invalid.
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(Error::new(
            Code::Usage,
            format!("invalid numeric value {v:?}"),
        ));
    }
    let digits_ok = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !digits_ok(int_part) || !digits_ok(frac_part) {
        return Err(Error::new(
            Code::Usage,
            format!("invalid numeric value {v:?}"),
        ));
    }

    // The fractional part may carry at most 9 digits (1 gwei == 10^9 wei); any
    // 10th-or-deeper non-zero digit is sub-wei precision the Go path rejects.
    if frac_part.len() > 9 {
        let (kept, dropped) = frac_part.split_at(9);
        if dropped.bytes().any(|b| b != b'0') {
            return Err(Error::new(
                Code::Usage,
                "value must resolve to an integer wei amount",
            ));
        }
        // Trailing zeros beyond 9 places are harmless; keep the first 9.
        return wei_from_parts(int_part, kept);
    }

    wei_from_parts(int_part, frac_part)
}

/// Compose a wei `U256` from already-validated integer + fractional digit
/// strings, where the fractional part is at most 9 digits.
fn wei_from_parts(int_part: &str, frac_part: &str) -> Result<U256, Error> {
    let parse_u256 = |s: &str| -> Result<U256, Error> {
        if s.is_empty() {
            return Ok(U256::ZERO);
        }
        U256::from_str_radix(s, 10)
            .map_err(|e| Error::wrap(Code::Usage, "parse gwei value", to_std_err(e)))
    };

    let whole = parse_u256(int_part)?;
    let whole_wei = whole
        .checked_mul(U256::from(WEI_PER_GWEI))
        .ok_or_else(|| Error::new(Code::Usage, "gwei value overflows"))?;

    if frac_part.is_empty() {
        return Ok(whole_wei);
    }

    // Right-pad the fractional digits to exactly 9 places: each fractional digit
    // position d (1-based) contributes digit * 10^(9-d) wei.
    let frac = parse_u256(frac_part)?;
    let pad = 9u32 - frac_part.len() as u32;
    let frac_wei = frac
        .checked_mul(U256::from(10u64).pow(U256::from(pad)))
        .ok_or_else(|| Error::new(Code::Usage, "gwei value overflows"))?;

    whole_wei
        .checked_add(frac_wei)
        .ok_or_else(|| Error::new(Code::Usage, "gwei value overflows"))
}

/// Resolve the EIP-1559 fee cap (`maxFeePerGas`), parity with go-ethereum
/// `resolveFeeCap`.
///
/// With no override (`override_gwei` empty/whitespace) the cap is
/// `base_fee*2 + tip_cap`. With an override, the override gwei value is used
/// directly, but a value resolving below `tip_cap` is a usage error
/// (`--max-fee-gwei must be >= --max-priority-fee-gwei`), and a malformed
/// override is a usage error.
pub fn resolve_fee_cap(base_fee: U256, tip_cap: U256, override_gwei: &str) -> Result<U256, Error> {
    if !override_gwei.trim().is_empty() {
        let v = parse_gwei(override_gwei)
            .map_err(|e| Error::wrap(Code::Usage, "parse --max-fee-gwei", to_std_err(e)))?;
        if v < tip_cap {
            return Err(Error::new(
                Code::Usage,
                "--max-fee-gwei must be >= --max-priority-fee-gwei",
            ));
        }
        return Ok(v);
    }
    let fee_cap = base_fee
        .checked_mul(U256::from(2u64))
        .and_then(|v| v.checked_add(tip_cap))
        .ok_or_else(|| Error::new(Code::Usage, "fee cap overflows"))?;
    Ok(fee_cap)
}

/// Resolve the EIP-1559 tip cap (`maxPriorityFeePerGas`), parity with
/// go-ethereum `resolveTipCap`.
///
/// An explicit override gwei value wins (and surfaces a usage error if
/// malformed). Otherwise the node's `eth_maxPriorityFeePerGas` suggestion is
/// used; if that RPC call fails, the tip silently falls back to **2 gwei**
/// (matching Go's behavior for nodes lacking the method).
pub async fn resolve_tip_cap(client: &RpcClient, override_gwei: &str) -> Result<U256, Error> {
    if !override_gwei.trim().is_empty() {
        return parse_gwei(override_gwei)
            .map_err(|e| Error::wrap(Code::Usage, "parse --max-priority-fee-gwei", to_std_err(e)));
    }
    match client.max_priority_fee().await {
        Ok(tip) => Ok(tip),
        Err(_) => Ok(U256::from(DEFAULT_TIP_CAP_WEI)),
    }
}

// =============================================================================
// JSON-RPC client (criteria 5–7).
// =============================================================================

/// An `eth_call` / `eth_estimateGas` request payload.
///
/// The Rust analogue of go-ethereum's `ethereum.CallMsg`: the optional
/// `from`/`to` addresses, a `value`, and the calldata `input`. Serialized to the
/// JSON-RPC object shape both `eth_call` and `eth_estimateGas` accept.
#[derive(Debug, Clone)]
pub struct CallRequest {
    from: Option<Address>,
    to: Option<Address>,
    value: U256,
    data: Vec<u8>,
}

impl CallRequest {
    /// Build a call request from optional sender/target, a value, and calldata.
    pub fn new(from: Option<Address>, to: Option<Address>, value: U256, data: Vec<u8>) -> Self {
        CallRequest {
            from,
            to,
            value,
            data,
        }
    }

    /// Render the JSON-RPC call object (omitting empty optional fields).
    fn to_json(&self) -> Value {
        let mut obj = serde_json::Map::new();
        if let Some(from) = self.from {
            obj.insert("from".to_string(), json!(from.to_hex()));
        }
        if let Some(to) = self.to {
            obj.insert("to".to_string(), json!(to.to_hex()));
        }
        // Always include value + data: go-ethereum's CallMsg encodes them, and a
        // zero value/empty data round-trips cleanly as "0x0"/"0x".
        obj.insert("value".to_string(), json!(format!("0x{:x}", self.value)));
        obj.insert(
            "data".to_string(),
            json!(format!("0x{}", hex_encode(&self.data))),
        );
        Value::Object(obj)
    }
}

/// A decoded transaction receipt (the subset the executor's poll loop needs).
#[derive(Debug, Clone)]
pub struct TransactionReceipt {
    status: bool,
    block_number: Option<u64>,
    gas_used: Option<u64>,
}

impl TransactionReceipt {
    /// Whether the transaction succeeded (`status == 0x1`).
    pub fn success(&self) -> bool {
        self.status
    }

    /// The block the receipt was included in, if present.
    pub fn block_number(&self) -> Option<u64> {
        self.block_number
    }

    /// The gas the transaction consumed, if present.
    pub fn gas_used(&self) -> Option<u64> {
        self.gas_used
    }
}

/// A single-call JSON-RPC client over HTTP, the Rust analogue of the go-ethereum
/// `ethclient.Client` reads the CLI funnels through.
///
/// Each method maps to exactly one JSON-RPC call against the configured
/// endpoint and decodes the result identically to go-ethereum. Transport
/// failures and JSON-RPC error responses surface as typed [`Error`]s with
/// [`Code::Unavailable`]; there is no panic/unwrap in this module.
#[derive(Debug, Clone)]
pub struct RpcClient {
    inner: AlloyRpcClient,
}

impl RpcClient {
    /// Connect (lazily) to an HTTP JSON-RPC endpoint.
    ///
    /// The URL is validated up front; an invalid URL is rejected with a usage
    /// error rather than panicking. No network I/O happens here — the transport
    /// dials on the first request, matching the way `chains gas`/the executor
    /// surface connection failures at read time.
    pub fn connect(url: &str) -> Result<Self, Error> {
        let parsed: Url = url
            .parse()
            .map_err(|e| Error::wrap(Code::Usage, "invalid rpc url", to_std_err(e)))?;
        Ok(RpcClient {
            inner: AlloyRpcClient::new_http(parsed),
        })
    }

    /// `eth_chainId` → the numeric chain id (go-ethereum `ChainID`).
    pub async fn chain_id(&self) -> Result<u64, Error> {
        let raw: U256 = self
            .request_no_params("eth_chainId", "read chain id")
            .await?;
        u256_to_u64(raw, "chain id")
    }

    /// The latest block number from the latest header (`HeaderByNumber(nil)`).
    pub async fn block_number(&self) -> Result<u64, Error> {
        let block = self.latest_block().await?;
        let number = block
            .get("number")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::new(Code::Unavailable, "latest block missing number"))?;
        hex_to_u64(number, "block number")
    }

    /// The latest header's `baseFeePerGas`, or `None` for a legacy chain that
    /// omits it (the `eip1559=false` signal).
    pub async fn base_fee(&self) -> Result<Option<U256>, Error> {
        let block = self.latest_block().await?;
        match block.get("baseFeePerGas") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(hex_to_u256(s, "base fee")?)),
            Some(_) => Err(Error::new(
                Code::Unavailable,
                "base fee has unexpected type",
            )),
        }
    }

    /// `eth_gasPrice` → the suggested gas price (`SuggestGasPrice`).
    pub async fn gas_price(&self) -> Result<U256, Error> {
        self.request_no_params("eth_gasPrice", "fetch gas price")
            .await
    }

    /// `eth_maxPriorityFeePerGas` → the suggested tip (`SuggestGasTipCap`).
    ///
    /// A JSON-RPC error result surfaces as `Err`; the caller decides the
    /// fallback (see [`resolve_tip_cap`]).
    pub async fn max_priority_fee(&self) -> Result<U256, Error> {
        self.request_no_params("eth_maxPriorityFeePerGas", "fetch priority fee")
            .await
    }

    /// `eth_getTransactionCount(addr, "pending")` → the pending nonce
    /// (`PendingNonceAt`).
    pub async fn pending_nonce(&self, addr: &Address) -> Result<u64, Error> {
        let raw: U256 = self
            .request(
                "eth_getTransactionCount",
                json!([addr.to_hex(), "pending"]),
                "fetch pending nonce",
            )
            .await?;
        u256_to_u64(raw, "pending nonce")
    }

    /// `eth_estimateGas` → the estimated gas limit (`EstimateGas`).
    pub async fn estimate_gas(&self, call: &CallRequest) -> Result<u64, Error> {
        let raw: U256 = self
            .request("eth_estimateGas", json!([call.to_json()]), "estimate gas")
            .await?;
        u256_to_u64(raw, "estimate gas")
    }

    /// `eth_call` → the raw return bytes (`CallContract`).
    pub async fn call(&self, call: &CallRequest) -> Result<Vec<u8>, Error> {
        let raw: Bytes = self
            .request("eth_call", json!([call.to_json(), "latest"]), "eth_call")
            .await?;
        Ok(raw.to_vec())
    }

    /// `eth_sendRawTransaction` → the 32-byte tx hash (`SendTransaction`).
    pub async fn send_raw_transaction(&self, raw: &[u8]) -> Result<[u8; 32], Error> {
        let payload = format!("0x{}", hex_encode(raw));
        let hash: B256 = self
            .request(
                "eth_sendRawTransaction",
                json!([payload]),
                "send raw transaction",
            )
            .await?;
        Ok(hash.0)
    }

    /// Broadcast an already-signed EIP-1559 transaction and return its tx hash.
    ///
    /// Convenience over [`send_raw_transaction`](Self::send_raw_transaction):
    /// encodes the signed tx's EIP-2718 envelope and submits it, matching the
    /// executor's `client.SendTransaction(signed)` step.
    pub async fn send_transaction(&self, signed: &SignedTx) -> Result<[u8; 32], Error> {
        self.send_raw_transaction(&signed.raw()).await
    }

    /// `eth_getTransactionReceipt` → the receipt, or `None` when not yet mined
    /// (go-ethereum's `ethereum.NotFound`).
    pub async fn transaction_receipt(
        &self,
        hash: &[u8; 32],
    ) -> Result<Option<TransactionReceipt>, Error> {
        let payload = format!("0x{}", hex_encode(hash));
        let raw: Value = self
            .request(
                "eth_getTransactionReceipt",
                json!([payload]),
                "fetch transaction receipt",
            )
            .await?;
        if raw.is_null() {
            return Ok(None);
        }
        let status = raw
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| hex_to_u64(s, "receipt status"))
            .transpose()?
            .map(|n| n == 1)
            .unwrap_or(false);
        let block_number = raw
            .get("blockNumber")
            .and_then(|v| v.as_str())
            .map(|s| hex_to_u64(s, "receipt block number"))
            .transpose()?;
        let gas_used = raw
            .get("gasUsed")
            .and_then(|v| v.as_str())
            .map(|s| hex_to_u64(s, "receipt gas used"))
            .transpose()?;
        Ok(Some(TransactionReceipt {
            status,
            block_number,
            gas_used,
        }))
    }

    /// Fetch the latest block via `eth_getBlockByNumber("latest", false)`
    /// (go-ethereum `HeaderByNumber(nil)`).
    async fn latest_block(&self) -> Result<Value, Error> {
        self.request(
            "eth_getBlockByNumber",
            json!(["latest", false]),
            "fetch block header",
        )
        .await
    }

    /// Issue a JSON-RPC request with no params, mapping transport / error-result
    /// failures to a typed [`Code::Unavailable`] error.
    async fn request_no_params<T>(&self, method: &'static str, context: &str) -> Result<T, Error>
    where
        T: serde::de::DeserializeOwned + std::fmt::Debug + Send + Sync + Unpin + 'static,
    {
        self.inner
            .request_noparams::<T>(method)
            .await
            .map_err(|e| Error::wrap(Code::Unavailable, context.to_string(), to_std_err(e)))
    }

    /// Issue a JSON-RPC request with params, mapping transport / error-result
    /// failures to a typed [`Code::Unavailable`] error.
    async fn request<T>(
        &self,
        method: &'static str,
        params: Value,
        context: &str,
    ) -> Result<T, Error>
    where
        T: serde::de::DeserializeOwned + std::fmt::Debug + Send + Sync + Unpin + 'static,
    {
        self.inner
            .request::<Value, T>(method, params)
            .await
            .map_err(|e| Error::wrap(Code::Unavailable, context.to_string(), to_std_err(e)))
    }
}

/// Build the unsigned EIP-1559 transaction body the executor signs + broadcasts.
///
/// A thin re-export bridge so callers in `defi-execution` can compose the same
/// `to`/`value`/`input` + fee fields that flow into [`crate::signer::LocalSigner`].
pub fn build_eip1559(
    to: Option<Address>,
    value: U256,
    input: Vec<u8>,
) -> (Option<TxKind>, U256, Bytes) {
    let kind = to.map(|a| TxKind::Call(a.into_inner()));
    (kind, value, Bytes::from(input))
}

// ---- helpers ---------------------------------------------------------------

/// Lowercase hex-encode bytes without an `0x` prefix.
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap_or('0'));
    }
    s
}

/// Parse a `0x`-prefixed (or bare) hex quantity into a `U256`.
fn hex_to_u256(s: &str, what: &str) -> Result<U256, Error> {
    let body = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if body.is_empty() {
        return Ok(U256::ZERO);
    }
    U256::from_str_radix(body, 16)
        .map_err(|e| Error::wrap(Code::Unavailable, format!("decode {what}"), to_std_err(e)))
}

/// Parse a `0x`-prefixed (or bare) hex quantity into a `u64`.
fn hex_to_u64(s: &str, what: &str) -> Result<u64, Error> {
    u256_to_u64(hex_to_u256(s, what)?, what)
}

/// Narrow a `U256` to `u64`, erroring (no panic) on overflow.
fn u256_to_u64(v: U256, what: &str) -> Result<u64, Error> {
    if v > U256::from(u64::MAX) {
        return Err(Error::new(
            Code::Unavailable,
            format!("{what} exceeds u64 range"),
        ));
    }
    Ok(v.to::<u64>())
}

/// A concrete, `Send + Sync` std error carrying a display message.
///
/// Lets us record the underlying alloy/transport error text as the `cause` of a
/// typed [`Error`] without depending on each foreign error type implementing the
/// exact `Error + Send + Sync + 'static` bound [`Error::wrap`] requires.
#[derive(Debug)]
struct MsgError(String);

impl std::fmt::Display for MsgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MsgError {}

/// Capture an arbitrary error's display text as a concrete [`MsgError`] cause.
fn to_std_err<E: std::fmt::Display>(e: E) -> MsgError {
    MsgError(e.to_string())
}

#[cfg(test)]
mod tests {
    //! RED phase: these reference the not-yet-implemented public API of this
    //! module. They MUST fail to compile / fail assertions until GREEN.
    //!
    //! Pure math (criteria 1–4) is asserted against the exact go-ethereum
    //! `weiToGwei`/`parseGwei`/`resolveFeeCap` vectors from the Go tests. The
    //! JSON-RPC client (criteria 5–7) is exercised with `wiremock` — the Rust
    //! analogue of `runner_gas_test.go::newMockRPCServer` — so the tests stay
    //! deterministic and offline.
    use super::*;

    use alloy::primitives::U256;

    // ---- canonical test addresses ----
    const SENDER: &str = "0x00000000000000000000000000000000000000aa";

    // =====================================================================
    // 1. wei_to_gwei formatting parity with Go weiToGwei
    // =====================================================================

    #[test]
    fn wei_to_gwei_none_is_bare_zero() {
        // Go: weiToGwei(nil) == "0" (NOT "0.000000").
        assert_eq!(wei_to_gwei(None), "0");
    }

    #[test]
    fn wei_to_gwei_zero_has_six_decimals() {
        assert_eq!(wei_to_gwei(Some(U256::ZERO)), "0.000000");
    }

    #[test]
    fn wei_to_gwei_one_gwei() {
        assert_eq!(wei_to_gwei(Some(U256::from(1_000_000_000u64))), "1.000000");
    }

    #[test]
    fn wei_to_gwei_thirty_point_five_gwei() {
        assert_eq!(
            wei_to_gwei(Some(U256::from(30_500_000_000u64))),
            "30.500000"
        );
    }

    #[test]
    fn wei_to_gwei_sub_gwei() {
        // 500_000 wei = 0.000500 gwei.
        assert_eq!(wei_to_gwei(Some(U256::from(500_000u64))), "0.000500");
    }

    #[test]
    fn wei_to_gwei_large_value_no_scientific_notation_or_loss() {
        // 250 gwei expressed in wei — beyond what naive f64 division renders
        // exactly. Must produce a plain fixed-point string with 6 decimals.
        let got = wei_to_gwei(Some(U256::from(250_000_000_000u64)));
        assert_eq!(got, "250.000000");
        assert!(!got.contains('e') && !got.contains('E'), "no sci-notation");
    }

    #[test]
    fn wei_to_gwei_three_gwei_matches_chains_gas_golden() {
        // 0xB2D05E00 == 3 gwei, the gas_price the chains-gas end-to-end test
        // asserts renders as "3.000000".
        assert_eq!(wei_to_gwei(Some(U256::from(3_000_000_000u64))), "3.000000");
    }

    #[test]
    fn wei_to_gwei_matches_go_big_float_binary_tie_oracle() {
        // GROUND TRUTH: each (wei, gwei) pair was captured directly from the Go
        // reference `weiToGwei` (`new(big.Float).SetInt(wei).Quo(.., 1e9).Text('f',
        // 6)`, go-ethereum's big.Float). These are the exact-half decimal ties
        // where the binary-float rounding (NOT clean decimal round-half-even)
        // decides the last digit. A naive decimal-half-even or f64 implementation
        // produces DIFFERENT strings for `500`, `1500`, `13500`, ... so this test
        // is the regression guard for big.Float parity.
        let oracle: &[(u64, &str)] = &[
            (0, "0.000000"),
            (1, "0.000000"),
            (499, "0.000000"),
            (500, "0.000001"),
            (501, "0.000001"),
            (999, "0.000001"),
            (1000, "0.000001"),
            (1001, "0.000001"),
            (1499, "0.000001"),
            (1500, "0.000001"),
            (1501, "0.000002"),
            (2500, "0.000002"),
            (3500, "0.000004"),
            (4500, "0.000004"),
            (5500, "0.000006"),
            (6500, "0.000006"),
            (7500, "0.000008"),
            (8500, "0.000008"),
            (9500, "0.000010"),
            (10500, "0.000010"),
            (11500, "0.000011"),
            (12500, "0.000013"),
            (13500, "0.000014"),
            (500_000, "0.000500"),
            (123_456_789, "0.123457"),
            (999_999_999, "1.000000"),
            (1_234_567_890_123, "1234.567890"),
            (12_345_678_901_234_567, "12345678.901235"),
            (30_500_000_000, "30.500000"),
            (3_000_000_000, "3.000000"),
            (250_000_000_000, "250.000000"),
        ];
        for (wei, want) in oracle {
            assert_eq!(
                wei_to_gwei(Some(U256::from(*wei))),
                *want,
                "wei_to_gwei({wei}) must match Go big.Float output"
            );
        }
    }

    #[test]
    fn wei_to_gwei_full_u256_max_matches_go_big_float() {
        // Real base fees are small, but the type is U256; the extreme value must
        // still render exactly as Go's big.Float does (no panic, no scientific
        // notation, no precision loss). Captured from the Go reference weiToGwei
        // for 2^256-1.
        let got = wei_to_gwei(Some(U256::MAX));
        assert_eq!(
            got,
            "115792089237316195423570985008687907853269984665640564039457584007913.129640"
        );
        assert!(!got.contains('e') && !got.contains('E'), "no sci-notation");
    }

    // =====================================================================
    // 2. parse_gwei parity with Go parseGwei
    // =====================================================================

    #[test]
    fn parse_gwei_whole_numbers() {
        assert_eq!(parse_gwei("1").unwrap(), U256::from(1_000_000_000u64));
        assert_eq!(parse_gwei("2").unwrap(), U256::from(2_000_000_000u64));
    }

    #[test]
    fn parse_gwei_fractional() {
        assert_eq!(parse_gwei("1.5").unwrap(), U256::from(1_500_000_000u64));
    }

    #[test]
    fn parse_gwei_one_wei_is_smallest_integer() {
        // 0.000000001 gwei == 1 wei (exact integer).
        assert_eq!(parse_gwei("0.000000001").unwrap(), U256::from(1u64));
    }

    #[test]
    fn parse_gwei_trims_whitespace() {
        assert_eq!(parse_gwei("  3  ").unwrap(), U256::from(3_000_000_000u64));
    }

    #[test]
    fn parse_gwei_rejects_empty() {
        assert!(parse_gwei("").is_err());
        assert!(parse_gwei("   ").is_err());
    }

    #[test]
    fn parse_gwei_rejects_non_numeric() {
        assert!(parse_gwei("abc").is_err());
        assert!(parse_gwei("1.2.3").is_err());
    }

    #[test]
    fn parse_gwei_rejects_negative() {
        assert!(parse_gwei("-1").is_err());
    }

    #[test]
    fn parse_gwei_rejects_sub_wei_precision() {
        // 0.0000000001 gwei == 0.1 wei — Go errors rather than truncate.
        assert!(parse_gwei("0.0000000001").is_err());
    }

    // =====================================================================
    // 4. resolve_fee_cap parity with Go resolveFeeCap (pure, no client)
    // =====================================================================

    #[test]
    fn resolve_fee_cap_no_override_is_base_times_two_plus_tip() {
        let base = U256::from(1_000_000_000u64); // 1 gwei
        let tip = U256::from(2_000_000_000u64); // 2 gwei
                                                // base*2 + tip = 4 gwei.
        assert_eq!(
            resolve_fee_cap(base, tip, "").unwrap(),
            U256::from(4_000_000_000u64)
        );
    }

    #[test]
    fn resolve_fee_cap_override_above_tip_is_accepted() {
        let base = U256::from(1_000_000_000u64);
        let tip = U256::from(2_000_000_000u64);
        // override 10 gwei >= tip 2 gwei.
        assert_eq!(
            resolve_fee_cap(base, tip, "10").unwrap(),
            U256::from(10_000_000_000u64)
        );
    }

    #[test]
    fn resolve_fee_cap_override_below_tip_is_usage_error() {
        let base = U256::from(1_000_000_000u64);
        let tip = U256::from(5_000_000_000u64); // 5 gwei
                                                // override 1 gwei < tip 5 gwei -> usage error.
        let err = resolve_fee_cap(base, tip, "1").unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    #[test]
    fn resolve_fee_cap_malformed_override_is_usage_error() {
        let base = U256::from(1_000_000_000u64);
        let tip = U256::from(2_000_000_000u64);
        let err = resolve_fee_cap(base, tip, "not-a-number").unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // =====================================================================
    // 5. JSON-RPC client reads parity with ethclient (wiremock)
    // =====================================================================
    //
    // mock_rpc spins up a wiremock server that answers single JSON-RPC POSTs
    // exactly like runner_gas_test.go::newMockRPCServer: one method per call,
    // keyed off the request body's "method" field.

    use serde_json::{json, Value};
    use wiremock::matchers::{body_partial_json, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Register a JSON-RPC method responder returning `result`.
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

    /// Register a JSON-RPC method responder returning a JSON-RPC error object.
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

    /// A latest-block result with the given number + optional baseFeePerGas.
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
            Some(b) => {
                obj["baseFeePerGas"] = json!(b);
            }
            None => {
                obj["baseFeePerGas"] = Value::Null;
            }
        }
        obj
    }

    #[tokio::test]
    async fn client_reads_chain_id() {
        let server = MockServer::start().await;
        mock_method(&server, "eth_chainId", json!("0x1")).await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        let chain_id = client.chain_id().await.expect("chain id");
        assert_eq!(chain_id, 1);
    }

    #[tokio::test]
    async fn client_reads_block_number_from_latest_header() {
        let server = MockServer::start().await;
        // 0x10 == block 16, matching the chains-gas mock.
        mock_method(
            &server,
            "eth_getBlockByNumber",
            block_result("0x10", Some("0x3B9ACA00")),
        )
        .await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        let n = client.block_number().await.expect("block number");
        assert_eq!(n, 16);
    }

    #[tokio::test]
    async fn client_reads_base_fee_eip1559() {
        let server = MockServer::start().await;
        // 0x3B9ACA00 == 1 gwei base fee.
        mock_method(
            &server,
            "eth_getBlockByNumber",
            block_result("0x10", Some("0x3B9ACA00")),
        )
        .await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        let base = client.base_fee().await.expect("base fee call");
        assert_eq!(base, Some(U256::from(1_000_000_000u64)));
    }

    #[tokio::test]
    async fn client_base_fee_is_none_for_legacy_chain() {
        let server = MockServer::start().await;
        // No baseFeePerGas => legacy chain => eip1559=false signal.
        mock_method(&server, "eth_getBlockByNumber", block_result("0x5", None)).await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        let base = client.base_fee().await.expect("base fee call");
        assert_eq!(base, None);
    }

    #[tokio::test]
    async fn client_reads_gas_price() {
        let server = MockServer::start().await;
        // 0xB2D05E00 == 3 gwei.
        mock_method(&server, "eth_gasPrice", json!("0xB2D05E00")).await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        let price = client.gas_price().await.expect("gas price");
        assert_eq!(price, U256::from(3_000_000_000u64));

        // And it round-trips through the chains-gas formatter.
        assert_eq!(wei_to_gwei(Some(price)), "3.000000");
    }

    #[tokio::test]
    async fn client_reads_max_priority_fee() {
        let server = MockServer::start().await;
        // 0x77359400 == 2 gwei.
        mock_method(&server, "eth_maxPriorityFeePerGas", json!("0x77359400")).await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        let tip = client.max_priority_fee().await.expect("priority fee");
        assert_eq!(tip, U256::from(2_000_000_000u64));
    }

    #[tokio::test]
    async fn client_max_priority_fee_surfaces_rpc_error() {
        let server = MockServer::start().await;
        // The chains-gas legacy/old-node case: method returns an RPC error.
        mock_method_error(&server, "eth_maxPriorityFeePerGas").await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        // The caller decides the fallback (warning + zero, or 2-gwei tip);
        // the client itself must surface the error, not swallow it.
        assert!(client.max_priority_fee().await.is_err());
    }

    // =====================================================================
    // 3. resolve_tip_cap (override + RPC-suggested fallback) — needs a client
    // =====================================================================

    #[tokio::test]
    async fn resolve_tip_cap_override_takes_precedence() {
        let server = MockServer::start().await;
        // The override should win even if the node would suggest something else.
        mock_method(&server, "eth_maxPriorityFeePerGas", json!("0x77359400")).await;
        let client = RpcClient::connect(&server.uri()).expect("connect");

        let tip = resolve_tip_cap(&client, "5").await.expect("override tip");
        assert_eq!(tip, U256::from(5_000_000_000u64));
    }

    #[tokio::test]
    async fn resolve_tip_cap_uses_rpc_suggestion_without_override() {
        let server = MockServer::start().await;
        mock_method(&server, "eth_maxPriorityFeePerGas", json!("0x77359400")).await; // 2 gwei
        let client = RpcClient::connect(&server.uri()).expect("connect");

        let tip = resolve_tip_cap(&client, "").await.expect("suggested tip");
        assert_eq!(tip, U256::from(2_000_000_000u64));
    }

    #[tokio::test]
    async fn resolve_tip_cap_falls_back_to_two_gwei_on_rpc_error() {
        let server = MockServer::start().await;
        mock_method_error(&server, "eth_maxPriorityFeePerGas").await;
        let client = RpcClient::connect(&server.uri()).expect("connect");

        // Go silently falls back to 2 gwei (does NOT error) when the node lacks
        // eth_maxPriorityFeePerGas.
        let tip = resolve_tip_cap(&client, "").await.expect("fallback tip");
        assert_eq!(tip, U256::from(2_000_000_000u64));
    }

    #[tokio::test]
    async fn resolve_tip_cap_malformed_override_is_usage_error() {
        let server = MockServer::start().await;
        let client = RpcClient::connect(&server.uri()).expect("connect");
        let err = resolve_tip_cap(&client, "not-a-number").await.unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // =====================================================================
    // 6. JSON-RPC execution primitives parity (wiremock)
    // =====================================================================

    #[tokio::test]
    async fn client_reads_pending_nonce() {
        let server = MockServer::start().await;
        // 0x7 == nonce 7.
        mock_method(&server, "eth_getTransactionCount", json!("0x7")).await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        let addr = crate::address::parse(SENDER).expect("addr");
        let nonce = client.pending_nonce(&addr).await.expect("nonce");
        assert_eq!(nonce, 7);
    }

    #[tokio::test]
    async fn client_estimates_gas() {
        let server = MockServer::start().await;
        // 0x5208 == 21000.
        mock_method(&server, "eth_estimateGas", json!("0x5208")).await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        let from = crate::address::parse(SENDER).expect("from");
        let to = crate::address::parse("0x00000000000000000000000000000000000000bb").expect("to");
        let call = CallRequest::new(Some(from), Some(to), U256::ZERO, vec![]);
        let gas = client.estimate_gas(&call).await.expect("estimate");
        assert_eq!(gas, 21_000);
    }

    #[tokio::test]
    async fn client_eth_call_returns_raw_bytes() {
        let server = MockServer::start().await;
        // An address right-aligned in a 32-byte word (getPool-style return).
        mock_method(
            &server,
            "eth_call",
            json!("0x000000000000000000000000000000000000000000000000000000000000dead"),
        )
        .await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        let to = crate::address::parse("0x00000000000000000000000000000000000000bb").expect("to");
        let call = CallRequest::new(None, Some(to), U256::ZERO, vec![0x02, 0x6b, 0x1d, 0x5f]);
        let out = client.call(&call).await.expect("eth_call");
        assert_eq!(
            hex::encode(&out),
            "000000000000000000000000000000000000000000000000000000000000dead"
        );
    }

    #[tokio::test]
    async fn client_sends_raw_transaction_returns_hash() {
        let server = MockServer::start().await;
        let tx_hash = "0x1111111111111111111111111111111111111111111111111111111111111111";
        mock_method(&server, "eth_sendRawTransaction", json!(tx_hash)).await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        let raw = vec![0x02u8, 0xf8, 0x6b]; // arbitrary RLP-ish bytes
        let hash = client.send_raw_transaction(&raw).await.expect("broadcast");
        assert_eq!(format!("0x{}", hex::encode(hash)), tx_hash);
    }

    #[tokio::test]
    async fn client_transaction_receipt_some_when_mined() {
        let server = MockServer::start().await;
        let receipt = json!({
            "transactionHash": "0x1111111111111111111111111111111111111111111111111111111111111111",
            "blockNumber": "0x10",
            "status": "0x1",
            "gasUsed": "0x5208",
        });
        mock_method(&server, "eth_getTransactionReceipt", receipt).await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        let hash = [0x11u8; 32];
        let got = client
            .transaction_receipt(&hash)
            .await
            .expect("receipt call");
        let receipt = got.expect("receipt should be present once mined");
        assert!(receipt.success(), "status 0x1 means success");
        assert_eq!(receipt.block_number(), Some(16));
    }

    #[tokio::test]
    async fn client_transaction_receipt_none_when_not_yet_mined() {
        let server = MockServer::start().await;
        // go-ethereum's ethereum.NotFound == JSON-RPC null result.
        mock_method(&server, "eth_getTransactionReceipt", Value::Null).await;

        let client = RpcClient::connect(&server.uri()).expect("connect");
        let hash = [0x11u8; 32];
        let got = client
            .transaction_receipt(&hash)
            .await
            .expect("receipt call");
        assert!(got.is_none(), "null receipt => not yet mined => None");
    }

    // =====================================================================
    // 7. Typed, no-panic error surface
    // =====================================================================

    #[tokio::test]
    async fn unreachable_endpoint_yields_unavailable_error() {
        // Nothing listening on this port: the read must surface a typed
        // Unavailable error (Go: clierr.Wrap(CodeUnavailable, ...)), never panic.
        let client = RpcClient::connect("http://127.0.0.1:1").expect("connect builds lazily");
        let err = client.chain_id().await.unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Unavailable);
    }

    #[test]
    fn connect_rejects_invalid_url_without_panic() {
        // An obviously malformed URL must return Err, not panic.
        assert!(RpcClient::connect("not a url").is_err());
    }

    #[tokio::test]
    async fn rpc_error_result_is_typed_and_displayable() {
        let server = MockServer::start().await;
        mock_method_error(&server, "eth_chainId").await;
        let client = RpcClient::connect(&server.uri()).expect("connect");
        let err = client.chain_id().await.unwrap_err();
        assert!(!err.to_string().is_empty(), "error must be displayable");
    }
}
