//! ABI encoding/decoding for contract calls.
//!
//! This module owns the contract-ABI half of the machine contract that the Go
//! tree reached for via go-ethereum's `accounts/abi` package
//! (`abi.JSON(...).Pack(...)`, `.Unpack(...)`, `Method.ID` selectors,
//! `Method.Inputs.Unpack`, `Method.Outputs.Pack`, and `abi.UnpackRevert`). Every
//! execution planner (`internal/execution/planner/*`), the executor's
//! approval/allowance policy checks (`internal/execution/executor.go`,
//! `policy_basic.go`), and the on-chain read providers (Moonwell, Tempo,
//! TaikoSwap, LiFi) funnel their calldata construction and return-data decoding
//! through this one encoding engine. The on-chain *bytes* it emits go straight
//! into broadcast transactions and into the JSON contract's
//! `steps[].data` field, so they must be **byte-for-byte identical** to what
//! go-ethereum produced — there is no room for drift.
//!
//! Go used a *runtime* JSON ABI (`abi.JSON(strings.NewReader(raw))`); the
//! idiomatic Rust port wraps `alloy-dyn-abi` (`JsonAbi`/`DynSolValue`) so the
//! same JSON fragments stored in `defi-registry` round-trip to the same bytes.
//! The ABI JSON strings themselves live in the registry crate (L2); this module
//! owns the *engine* that turns "(fragment, args)" into selectors + calldata and
//! "(fragment, return data)" back into typed values.
//!
//! # Success criteria (contract this module preserves)
//!
//! All golden hex byte-strings in the tests were probed directly from
//! go-ethereum `accounts/abi` against the exact ABI fragments in
//! `internal/registry/abis.go` (ERC20, Aave Pool, Aave Rewards, Morpho Blue,
//! ERC4626 vault, Moonwell mToken, Moonwell Comptroller, Aave
//! PoolAddressesProvider). They are the ground-truth oracle this engine
//! reproduces.
//!
//! 1. **Function selectors == go-ethereum `Method.ID`** — [`function_selector`]
//!    and [`Function::selector`]: the first 4 bytes of `keccak256` over the
//!    canonical signature.
//! 2. **Function-call encoding == go-ethereum `ABI.Pack`** — [`Function::encode`]:
//!    selector ++ head/tail ABI-encoded args.
//! 3. **Return-data decoding == go-ethereum `ABI.Unpack`** —
//!    [`Function::decode_output`]; truncated/short data is an `Err`, never a
//!    panic.
//! 4. **Encode/decode round-trip** — [`Function::decode_input`] re-reads a call's
//!    inputs (the `Method.Inputs.Unpack(data[4:])` path the policy/executor use
//!    for allowance-bound checks).
//! 5. **Revert-reason decoding == go-ethereum `abi.UnpackRevert`** —
//!    [`decode_revert_reason`]: `Error(string)` selector `0x08c379a0` + an
//!    ABI-encoded string decodes to that string; anything else yields `None`.
//! 6. **Strict, no-panic library surface** — invalid fragments, wrong
//!    arity/types, and malformed return data all return a `thiserror`-typed,
//!    displayable error; no `unwrap`/`expect`/`panic` in non-test code.

use alloy::dyn_abi::{DynSolType, DynSolValue, FunctionExt, JsonAbiExt};
use alloy::json_abi::{Function as JsonFunction, JsonAbi};
use alloy::primitives::keccak256;
use defi_errors::{Code, Error};

/// The `Error(string)` selector (`keccak256("Error(string)")[..4]`) prefixed to
/// Solidity's standard revert payload, matching go-ethereum `abi.UnpackRevert`.
const ERROR_STRING_SELECTOR: [u8; 4] = [0x08, 0xc3, 0x79, 0xa0];

/// Compute the 4-byte function selector for a canonical signature string.
///
/// Parity with go-ethereum `Method.ID`: the first 4 bytes of
/// `keccak256(signature)`, where `signature` is the canonical form
/// `name(type1,type2,...)` with normalized types (e.g. `uint`→`uint256`, tuples
/// rendered as `(...)`, arrays as `type[]`). The caller supplies the already
/// canonicalized signature; this function does no normalization of its own.
pub fn function_selector(signature: &str) -> [u8; 4] {
    let hash = keccak256(signature.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

/// A single parsed ABI function fragment.
///
/// Construct via [`Function::from_abi_json`] from one of the runtime JSON ABI
/// fragments stored in `defi-registry`. Wraps `alloy`'s `json_abi::Function`,
/// which canonicalizes types so selectors and calldata match go-ethereum
/// byte-for-byte.
#[derive(Debug, Clone)]
pub struct Function {
    inner: JsonFunction,
}

impl Function {
    /// Parse a JSON ABI document and pick out the named function fragment.
    ///
    /// `abi_json` is the same runtime JSON string go-ethereum fed to
    /// `abi.JSON(strings.NewReader(raw))` — either a bare ABI array
    /// (`[{...},{...}]`) or a contract object with an `abi` field. Returns the
    /// fragment named `name`.
    ///
    /// # Errors
    ///
    /// - [`Code::Internal`] if `abi_json` is not valid JSON ABI (mirrors the Go
    ///   tree's `mustPlannerABI` invariant on static, known-good fragments,
    ///   surfaced here as a typed error rather than a panic).
    /// - [`Code::Internal`] if no function named `name` exists in the document.
    pub fn from_abi_json(abi_json: &str, name: &str) -> Result<Function, Error> {
        let abi: JsonAbi = serde_json::from_str(abi_json)
            .map_err(|e| Error::wrap(Code::Internal, "parse ABI fragment", e))?;
        let overloads = abi.function(name).ok_or_else(|| {
            Error::new(
                Code::Internal,
                format!("ABI has no function named {name:?}"),
            )
        })?;
        let inner = overloads
            .first()
            .ok_or_else(|| {
                Error::new(
                    Code::Internal,
                    format!("ABI has no function named {name:?}"),
                )
            })?
            .clone();
        Ok(Function { inner })
    }

    /// The 4-byte function selector, parity with go-ethereum `Method.ID`.
    ///
    /// The first 4 bytes of `keccak256` over the function's canonical signature
    /// (`alloy` canonicalizes types, so this matches go-ethereum exactly).
    pub fn selector(&self) -> [u8; 4] {
        self.inner.selector().0
    }

    /// The function's canonical signature (`name(type1,type2,...)`), the
    /// keccak preimage of the selector.
    pub fn signature(&self) -> String {
        self.inner.signature()
    }

    /// ABI-encode a call to this function: selector ++ ABI-encoded inputs.
    ///
    /// Parity with go-ethereum `ABI.Pack(name, args...)`. The produced bytes go
    /// straight into broadcast transactions and the JSON contract's
    /// `steps[].data`, so they must match go-ethereum byte-for-byte.
    ///
    /// # Errors
    ///
    /// [`Code::Internal`] if `args` does not match the function's input arity or
    /// types (a typed error, never a panic).
    pub fn encode(&self, args: &[DynSolValue]) -> Result<Vec<u8>, Error> {
        self.inner
            .abi_encode_input(args)
            .map_err(|e| Error::wrap(Code::Internal, "ABI-encode function inputs", e))
    }

    /// Decode a function call's input arguments from its calldata body.
    ///
    /// Parity with go-ethereum `Method.Inputs.Unpack(data[4:])`: `data` must be
    /// the calldata **without** the leading 4-byte selector. This is the path
    /// `policy_basic`/executor use to re-read an `approve(spender, amount)` call
    /// for allowance-bound checks.
    ///
    /// # Errors
    ///
    /// [`Code::Unavailable`] if `data` is malformed or truncated for this
    /// function's input types (decode failure is a typed error, never a panic).
    pub fn decode_input(&self, data: &[u8]) -> Result<Vec<DynSolValue>, Error> {
        self.inner
            .abi_decode_input(data)
            .map_err(|e| Error::wrap(Code::Unavailable, "ABI-decode function inputs", e))
    }

    /// Decode a function call's return data into typed values.
    ///
    /// Parity with go-ethereum `ABI.Unpack(name, data)`.
    ///
    /// # Errors
    ///
    /// [`Code::Unavailable`] if `data` is malformed or truncated for this
    /// function's output types (the Go path treats decode failure as a typed
    /// `Unavailable`/internal error, never a crash).
    pub fn decode_output(&self, data: &[u8]) -> Result<Vec<DynSolValue>, Error> {
        self.inner
            .abi_decode_output(data)
            .map_err(|e| Error::wrap(Code::Unavailable, "ABI-decode function outputs", e))
    }
}

/// Decode a Solidity standard-revert payload into its human-readable reason.
///
/// Parity with go-ethereum `abi.UnpackRevert`: a `data` slice prefixed with the
/// `Error(string)` selector `0x08c379a0` followed by a well-formed ABI-encoded
/// string yields `Some(reason)`. Anything that is not a well-formed
/// `Error(string)` payload (empty, wrong selector, truncated body, non-string
/// body) yields `None`. Never panics. This feeds the executor's human-readable
/// revert surfacing.
pub fn decode_revert_reason(data: &[u8]) -> Option<String> {
    let body = data.strip_prefix(&ERROR_STRING_SELECTOR[..])?;
    match DynSolType::String.abi_decode(body) {
        Ok(DynSolValue::String(s)) => Some(s),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! Golden vectors are byte-for-byte outputs of go-ethereum `accounts/abi`
    //! over the registry ABI fragments, captured with the canonical test args
    //! (spender `0x..BB`, recipient `0x..CC`, onBehalf/owner `0x..AA`, token
    //! `0x..DEAD`, amount `1_000_000`).
    use super::*;

    use alloy::dyn_abi::DynSolValue;
    use alloy::primitives::{Address as AlloyAddress, U256};

    // ---- canonical test addresses (right-aligned 20-byte values) ----
    const SPENDER: &str = "0x00000000000000000000000000000000000000BB";
    const RECIPIENT: &str = "0x00000000000000000000000000000000000000CC";
    const ON_BEHALF: &str = "0x00000000000000000000000000000000000000AA";
    const OWNER: &str = "0x00000000000000000000000000000000000000AA";
    const TOKEN: &str = "0x000000000000000000000000000000000000DEAD";

    // ---- registry ABI fragments (mirrors internal/registry/abis.go) ----
    const ERC20_ABI: &str = r#"[
        {"name":"allowance","type":"function","stateMutability":"view","inputs":[{"name":"owner","type":"address"},{"name":"spender","type":"address"}],"outputs":[{"name":"","type":"uint256"}]},
        {"name":"approve","type":"function","stateMutability":"nonpayable","inputs":[{"name":"spender","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"","type":"bool"}]},
        {"name":"transfer","type":"function","stateMutability":"nonpayable","inputs":[{"name":"to","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"","type":"bool"}]}
    ]"#;

    const AAVE_POOL_ABI: &str = r#"[
        {"name":"supply","type":"function","stateMutability":"nonpayable","inputs":[{"name":"asset","type":"address"},{"name":"amount","type":"uint256"},{"name":"onBehalfOf","type":"address"},{"name":"referralCode","type":"uint16"}],"outputs":[]},
        {"name":"withdraw","type":"function","stateMutability":"nonpayable","inputs":[{"name":"asset","type":"address"},{"name":"amount","type":"uint256"},{"name":"to","type":"address"}],"outputs":[{"name":"","type":"uint256"}]},
        {"name":"borrow","type":"function","stateMutability":"nonpayable","inputs":[{"name":"asset","type":"address"},{"name":"amount","type":"uint256"},{"name":"interestRateMode","type":"uint256"},{"name":"referralCode","type":"uint16"},{"name":"onBehalfOf","type":"address"}],"outputs":[]},
        {"name":"repay","type":"function","stateMutability":"nonpayable","inputs":[{"name":"asset","type":"address"},{"name":"amount","type":"uint256"},{"name":"interestRateMode","type":"uint256"},{"name":"onBehalfOf","type":"address"}],"outputs":[{"name":"","type":"uint256"}]}
    ]"#;

    const AAVE_REWARDS_ABI: &str = r#"[
        {"name":"claimRewards","type":"function","stateMutability":"nonpayable","inputs":[{"name":"assets","type":"address[]"},{"name":"amount","type":"uint256"},{"name":"to","type":"address"},{"name":"reward","type":"address"}],"outputs":[{"name":"","type":"uint256"}]}
    ]"#;

    const ERC4626_VAULT_ABI: &str = r#"[
        {"name":"asset","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]},
        {"name":"deposit","type":"function","stateMutability":"nonpayable","inputs":[{"name":"assets","type":"uint256"},{"name":"receiver","type":"address"}],"outputs":[{"name":"shares","type":"uint256"}]},
        {"name":"withdraw","type":"function","stateMutability":"nonpayable","inputs":[{"name":"assets","type":"uint256"},{"name":"receiver","type":"address"},{"name":"owner","type":"address"}],"outputs":[{"name":"shares","type":"uint256"}]}
    ]"#;

    const MTOKEN_ABI: &str = r#"[
        {"name":"underlying","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]},
        {"name":"mint","type":"function","stateMutability":"nonpayable","inputs":[{"name":"mintAmount","type":"uint256"}],"outputs":[{"name":"","type":"uint256"}]}
    ]"#;

    const COMPTROLLER_ABI: &str = r#"[
        {"name":"getAllMarkets","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address[]"}]},
        {"name":"enterMarkets","type":"function","stateMutability":"nonpayable","inputs":[{"name":"mTokens","type":"address[]"}],"outputs":[{"name":"","type":"uint256[]"}]}
    ]"#;

    const POOL_PROVIDER_ABI: &str = r#"[
        {"name":"getPool","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]},
        {"name":"getAddress","type":"function","stateMutability":"view","inputs":[{"name":"id","type":"bytes32"}],"outputs":[{"name":"","type":"address"}]}
    ]"#;

    const MORPHO_BLUE_ABI: &str = r#"[
        {"name":"supply","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"data","type":"bytes"}],"outputs":[{"name":"assetsSupplied","type":"uint256"},{"name":"sharesSupplied","type":"uint256"}]}
    ]"#;

    // -------- small helpers (test-only) --------

    fn addr(s: &str) -> AlloyAddress {
        s.parse().expect("valid test address")
    }
    fn av_addr(s: &str) -> DynSolValue {
        DynSolValue::Address(addr(s))
    }
    fn av_u256(n: u128) -> DynSolValue {
        DynSolValue::Uint(U256::from(n), 256)
    }
    fn hex_to_bytes(s: &str) -> Vec<u8> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
    fn func(abi_json: &str, name: &str) -> Function {
        Function::from_abi_json(abi_json, name).expect("function fragment must parse")
    }
    fn encode_hex(abi_json: &str, name: &str, args: &[DynSolValue]) -> String {
        let bytes = func(abi_json, name)
            .encode(args)
            .expect("encode must succeed for valid args");
        format!("0x{}", hex::encode(&bytes))
    }

    // ===================================================================
    // 1. Function selectors == go-ethereum Method.ID
    // ===================================================================

    #[test]
    fn selector_from_canonical_signature() {
        assert_eq!(
            function_selector("approve(address,uint256)"),
            [0x09, 0x5e, 0xa7, 0xb3]
        );
        assert_eq!(
            function_selector("transfer(address,uint256)"),
            [0xa9, 0x05, 0x9c, 0xbb]
        );
        assert_eq!(
            function_selector("allowance(address,address)"),
            [0xdd, 0x62, 0xed, 0x3e]
        );
    }

    #[test]
    fn function_selector_matches_go_ethereum_method_id() {
        let cases: &[(&str, &str, [u8; 4])] = &[
            (ERC20_ABI, "approve", [0x09, 0x5e, 0xa7, 0xb3]),
            (ERC20_ABI, "transfer", [0xa9, 0x05, 0x9c, 0xbb]),
            (ERC20_ABI, "allowance", [0xdd, 0x62, 0xed, 0x3e]),
            (AAVE_POOL_ABI, "supply", [0x61, 0x7b, 0xa0, 0x37]),
            (AAVE_POOL_ABI, "withdraw", [0x69, 0x32, 0x8d, 0xec]),
            (AAVE_POOL_ABI, "borrow", [0xa4, 0x15, 0xbc, 0xad]),
            (AAVE_POOL_ABI, "repay", [0x57, 0x3a, 0xde, 0x81]),
            (AAVE_REWARDS_ABI, "claimRewards", [0x23, 0x63, 0x00, 0xdc]),
            (ERC4626_VAULT_ABI, "deposit", [0x6e, 0x55, 0x3f, 0x65]),
            (ERC4626_VAULT_ABI, "withdraw", [0xb4, 0x60, 0xaf, 0x94]),
            (MTOKEN_ABI, "mint", [0xa0, 0x71, 0x2d, 0x68]),
            (COMPTROLLER_ABI, "enterMarkets", [0xc2, 0x99, 0x82, 0x38]),
            (POOL_PROVIDER_ABI, "getPool", [0x02, 0x6b, 0x1d, 0x5f]),
            (POOL_PROVIDER_ABI, "getAddress", [0x21, 0xf8, 0xa7, 0x21]),
            (MORPHO_BLUE_ABI, "supply", [0xa9, 0x9a, 0xad, 0x89]),
        ];
        for (abi_json, name, want) in cases {
            assert_eq!(
                func(abi_json, name).selector(),
                *want,
                "selector mismatch for {name}"
            );
        }
    }

    #[test]
    fn morpho_tuple_selector_uses_parenthesized_components() {
        // Morpho's first arg is a tuple; the canonical signature is
        // supply((address,address,address,address,uint256),uint256,uint256,address,bytes)
        assert_eq!(
            function_selector(
                "supply((address,address,address,address,uint256),uint256,uint256,address,bytes)"
            ),
            [0xa9, 0x9a, 0xad, 0x89]
        );
    }

    // ===================================================================
    // 2. Function-call encoding == go-ethereum ABI.Pack
    // ===================================================================

    #[test]
    fn encode_erc20_approve_matches_golden() {
        let got = encode_hex(
            ERC20_ABI,
            "approve",
            &[av_addr(SPENDER), av_u256(1_000_000)],
        );
        assert_eq!(
            got,
            "0x095ea7b300000000000000000000000000000000000000000000000000000000000000bb00000000000000000000000000000000000000000000000000000000000f4240"
        );
    }

    #[test]
    fn encode_erc20_transfer_matches_golden() {
        let got = encode_hex(
            ERC20_ABI,
            "transfer",
            &[av_addr(RECIPIENT), av_u256(1_000_000)],
        );
        assert_eq!(
            got,
            "0xa9059cbb00000000000000000000000000000000000000000000000000000000000000cc00000000000000000000000000000000000000000000000000000000000f4240"
        );
    }

    #[test]
    fn encode_erc20_allowance_matches_golden() {
        let got = encode_hex(ERC20_ABI, "allowance", &[av_addr(OWNER), av_addr(SPENDER)]);
        assert_eq!(
            got,
            "0xdd62ed3e00000000000000000000000000000000000000000000000000000000000000aa00000000000000000000000000000000000000000000000000000000000000bb"
        );
    }

    #[test]
    fn encode_aave_supply_with_uint16_referral_matches_golden() {
        // The trailing uint16(0) must be left-zero-padded into a full word.
        let got = encode_hex(
            AAVE_POOL_ABI,
            "supply",
            &[
                av_addr(TOKEN),
                av_u256(1_000_000),
                av_addr(ON_BEHALF),
                DynSolValue::Uint(U256::ZERO, 16),
            ],
        );
        assert_eq!(
            got,
            "0x617ba037000000000000000000000000000000000000000000000000000000000000dead00000000000000000000000000000000000000000000000000000000000f424000000000000000000000000000000000000000000000000000000000000000aa0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn encode_aave_withdraw_matches_golden() {
        let got = encode_hex(
            AAVE_POOL_ABI,
            "withdraw",
            &[av_addr(TOKEN), av_u256(1_000_000), av_addr(RECIPIENT)],
        );
        assert_eq!(
            got,
            "0x69328dec000000000000000000000000000000000000000000000000000000000000dead00000000000000000000000000000000000000000000000000000000000f424000000000000000000000000000000000000000000000000000000000000000cc"
        );
    }

    #[test]
    fn encode_aave_borrow_with_rate_mode_and_uint16_matches_golden() {
        let got = encode_hex(
            AAVE_POOL_ABI,
            "borrow",
            &[
                av_addr(TOKEN),
                av_u256(1_000_000),
                av_u256(2), // interestRateMode uint256
                DynSolValue::Uint(U256::ZERO, 16),
                av_addr(ON_BEHALF),
            ],
        );
        assert_eq!(
            got,
            "0xa415bcad000000000000000000000000000000000000000000000000000000000000dead00000000000000000000000000000000000000000000000000000000000f42400000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000aa"
        );
    }

    #[test]
    fn encode_aave_repay_matches_golden() {
        let got = encode_hex(
            AAVE_POOL_ABI,
            "repay",
            &[
                av_addr(TOKEN),
                av_u256(1_000_000),
                av_u256(2),
                av_addr(ON_BEHALF),
            ],
        );
        assert_eq!(
            got,
            "0x573ade81000000000000000000000000000000000000000000000000000000000000dead00000000000000000000000000000000000000000000000000000000000f4240000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000aa"
        );
    }

    #[test]
    fn encode_vault_deposit_matches_golden() {
        let got = encode_hex(
            ERC4626_VAULT_ABI,
            "deposit",
            &[av_u256(1_000_000), av_addr(RECIPIENT)],
        );
        assert_eq!(
            got,
            "0x6e553f6500000000000000000000000000000000000000000000000000000000000f424000000000000000000000000000000000000000000000000000000000000000cc"
        );
    }

    #[test]
    fn encode_vault_withdraw_matches_golden() {
        let got = encode_hex(
            ERC4626_VAULT_ABI,
            "withdraw",
            &[av_u256(1_000_000), av_addr(RECIPIENT), av_addr(OWNER)],
        );
        assert_eq!(
            got,
            "0xb460af9400000000000000000000000000000000000000000000000000000000000f424000000000000000000000000000000000000000000000000000000000000000cc00000000000000000000000000000000000000000000000000000000000000aa"
        );
    }

    #[test]
    fn encode_mtoken_mint_matches_golden() {
        let got = encode_hex(MTOKEN_ABI, "mint", &[av_u256(1_000_000)]);
        assert_eq!(
            got,
            "0xa0712d6800000000000000000000000000000000000000000000000000000000000f4240"
        );
    }

    #[test]
    fn encode_no_arg_getpool_is_just_selector() {
        let got = encode_hex(POOL_PROVIDER_ABI, "getPool", &[]);
        assert_eq!(got, "0x026b1d5f");
    }

    #[test]
    fn encode_getaddress_bytes32_matches_golden() {
        // bytes32 slot = keccak256("INCENTIVES_CONTROLLER")
        let slot = hex_to_bytes("703c2c8634bed68d98c029c18f310e7f7ec0e5d6342c590190b3cb8b3ba54532");
        let mut word = [0u8; 32];
        word.copy_from_slice(&slot);
        let got = encode_hex(
            POOL_PROVIDER_ABI,
            "getAddress",
            &[DynSolValue::FixedBytes(word.into(), 32)],
        );
        assert_eq!(
            got,
            "0x21f8a721703c2c8634bed68d98c029c18f310e7f7ec0e5d6342c590190b3cb8b3ba54532"
        );
    }

    #[test]
    fn encode_dynamic_address_array_enter_markets_matches_golden() {
        // enterMarkets([TOKEN]) — offset(0x20) + len(1) + element.
        let got = encode_hex(
            COMPTROLLER_ABI,
            "enterMarkets",
            &[DynSolValue::Array(vec![av_addr(TOKEN)])],
        );
        assert_eq!(
            got,
            "0xc299823800000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000dead"
        );
    }

    #[test]
    fn encode_claim_rewards_with_address_array_matches_golden() {
        // claimRewards([TOKEN, SPENDER], 1_000_000, RECIPIENT, TOKEN).
        let got = encode_hex(
            AAVE_REWARDS_ABI,
            "claimRewards",
            &[
                DynSolValue::Array(vec![av_addr(TOKEN), av_addr(SPENDER)]),
                av_u256(1_000_000),
                av_addr(RECIPIENT),
                av_addr(TOKEN),
            ],
        );
        assert_eq!(
            got,
            "0x236300dc000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000f424000000000000000000000000000000000000000000000000000000000000000cc000000000000000000000000000000000000000000000000000000000000dead0000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000dead00000000000000000000000000000000000000000000000000000000000000bb"
        );
    }

    #[test]
    fn encode_morpho_supply_tuple_and_empty_bytes_matches_golden() {
        // supply(MarketParams{loan,collat,oracle,irm,lltv}, assets, shares=0, onBehalf, data=0x)
        // lltv = 860000000000000000 (0.86e18).
        let lltv = U256::from(860_000_000_000_000_000u128);
        let market_params = DynSolValue::Tuple(vec![
            av_addr(TOKEN),     // loanToken
            av_addr(SPENDER),   // collateralToken
            av_addr(RECIPIENT), // oracle
            av_addr(ON_BEHALF), // irm
            DynSolValue::Uint(lltv, 256),
        ]);
        let got = encode_hex(
            MORPHO_BLUE_ABI,
            "supply",
            &[
                market_params,
                av_u256(1_000_000),                 // assets
                DynSolValue::Uint(U256::ZERO, 256), // shares
                av_addr(ON_BEHALF),                 // onBehalf
                DynSolValue::Bytes(vec![]),         // data
            ],
        );
        assert_eq!(
            got,
            "0xa99aad89000000000000000000000000000000000000000000000000000000000000dead00000000000000000000000000000000000000000000000000000000000000bb00000000000000000000000000000000000000000000000000000000000000cc00000000000000000000000000000000000000000000000000000000000000aa0000000000000000000000000000000000000000000000000bef55718ad6000000000000000000000000000000000000000000000000000000000000000f4240000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000aa00000000000000000000000000000000000000000000000000000000000001200000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn encode_rejects_wrong_arity() {
        // approve takes 2 args; passing 1 must be a typed Err, not a panic.
        let res = func(ERC20_ABI, "approve").encode(&[av_addr(SPENDER)]);
        assert!(res.is_err(), "wrong arity must error");
    }

    // ===================================================================
    // 3. Return-data decoding == go-ethereum ABI.Unpack
    // ===================================================================

    #[test]
    fn decode_getpool_output_address() {
        // go-ethereum-encoded getPool() return for TOKEN (0x..DEAD).
        let data = hex_to_bytes("000000000000000000000000000000000000000000000000000000000000dead");
        let out = func(POOL_PROVIDER_ABI, "getPool")
            .decode_output(&data)
            .expect("decode address output");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].as_address(), Some(addr(TOKEN)));
    }

    #[test]
    fn decode_allowance_output_uint256() {
        // go-ethereum-encoded allowance() return for 123_456_789.
        let data = hex_to_bytes("00000000000000000000000000000000000000000000000000000000075bcd15");
        let out = func(ERC20_ABI, "allowance")
            .decode_output(&data)
            .expect("decode uint256 output");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].as_uint(), Some((U256::from(123_456_789u64), 256)));
    }

    #[test]
    fn decode_output_rejects_truncated_data() {
        // Short / malformed return data must be a typed Err, never a panic.
        let res = func(POOL_PROVIDER_ABI, "getPool").decode_output(&[0x00, 0x01, 0x02]);
        assert!(res.is_err(), "truncated return data must error");
    }

    // ===================================================================
    // 4. Encode/decode round-trip (policy/executor re-read path)
    // ===================================================================

    #[test]
    fn approve_input_round_trips_and_selector_is_leading() {
        let f = func(ERC20_ABI, "approve");
        let args = [av_addr(SPENDER), av_u256(1_000_000)];
        let calldata = f.encode(&args).expect("encode approve");

        // selector is the first 4 bytes
        assert_eq!(&calldata[..4], &f.selector());

        // decode inputs back from the body (Go: Method.Inputs.Unpack(data[4:]))
        let decoded = f
            .decode_input(&calldata[4..])
            .expect("decode approve inputs");
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].as_address(), Some(addr(SPENDER)));
        assert_eq!(decoded[1].as_uint(), Some((U256::from(1_000_000u64), 256)));
    }

    // ===================================================================
    // 5. Revert-reason decoding == go-ethereum abi.UnpackRevert
    // ===================================================================

    #[test]
    fn decode_revert_reason_error_string() {
        // 0x08c379a0 ++ abi.encode("insufficient balance")
        let data = hex_to_bytes(
            "08c379a000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000014696e73756666696369656e742062616c616e6365000000000000000000000000",
        );
        assert_eq!(
            decode_revert_reason(&data),
            Some("insufficient balance".to_string())
        );
    }

    #[test]
    fn decode_revert_reason_none_for_non_error_payloads() {
        assert_eq!(decode_revert_reason(&[]), None);
        // wrong selector
        assert_eq!(decode_revert_reason(&[0xde, 0xad, 0xbe, 0xef]), None);
        // right selector but truncated body
        let truncated = hex_to_bytes("08c379a000");
        assert_eq!(decode_revert_reason(&truncated), None);
    }

    // ===================================================================
    // 6. Strict, no-panic library surface
    // ===================================================================

    #[test]
    fn parse_invalid_abi_fragment_is_err() {
        assert!(Function::from_abi_json("not json", "approve").is_err());
        assert!(Function::from_abi_json("[]", "approve").is_err());
    }

    #[test]
    fn missing_function_name_is_err() {
        // valid ABI, but the requested function is absent.
        assert!(Function::from_abi_json(ERC20_ABI, "nonexistent").is_err());
    }

    #[test]
    fn function_error_is_typed_and_displayable() {
        let err = Function::from_abi_json("[]", "approve").unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}
