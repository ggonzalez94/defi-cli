//! EVM address parsing/validation/checksumming. Scaffold stub — Phase 2 (RED).
//!
//! This module owns the EVM-address half of the machine contract that the Go tree
//! reached for via go-ethereum's `common` package (`IsHexAddress`, `HexToAddress`,
//! `Address.Hex()`, the zero-address comparison, and the `strings.EqualFold` on
//! `.Hex()` outputs). It is the single canonical place address strings get
//! validated and rendered, so the JSON contract (canonical EIP-55 checksum in
//! `from_address`/`to_address`/step targets) and the usage-error contract
//! (exit code 2 "must be a valid EVM hex address") stay byte-stable across the port.
//!
//! # Success criteria (contract this module must preserve)
//!
//! 1. **Validation parity with go-ethereum `common.IsHexAddress`** — [`is_hex_address`]:
//!    - accepts an optional `0x` **or** `0X` prefix, then **exactly 40** hex digits;
//!    - case-insensitive on the hex body (no EIP-55 checksum enforced here);
//!    - rejects empty, `0x`, too-short (39), too-long (41/42), non-hex chars, and
//!      any input with surrounding/internal whitespace (go-ethereum does **not**
//!      trim — `"  0x..  "` is invalid). Returns a `bool`, never errors.
//!
//! 2. **Canonical EIP-55 checksum rendering parity with `Address.Hex()`** —
//!    [`checksum`] / [`Address::to_hex`]: a valid 40-hex input (any case, with or
//!    without prefix) renders to the exact mixed-case EIP-55 string go-ethereum
//!    produces (verified against the canonical EIP-55 reference vectors and the
//!    `0x..dEaD` vector the Go runner/identity tests assert). Always `0x`-prefixed,
//!    always 42 chars.
//!
//! 3. **Parsing is strict (idiomatic Rust), not go-ethereum-lenient** — [`parse`]:
//!    returns a typed [`Address`] for any input `is_hex_address` accepts, and
//!    `Err` otherwise. (go-ethereum's `HexToAddress` silently right-aligns/truncates
//!    bad input; the Go *contract* only ever feeds it strings that already passed
//!    `IsHexAddress`, so the observable behavior we must keep is "valid in → checksum
//!    out, invalid in → usage error". We surface the error instead of corrupting.)
//!    On error it yields a `thiserror`-typed error (no panic/unwrap in lib code).
//!
//! 4. **Zero address** — [`Address::ZERO`] renders to
//!    `0x0000000000000000000000000000000000000000`; [`Address::is_zero`] is true
//!    only for it. This is the `common.Address{}` sentinel the executor/backends
//!    compare against.
//!
//! 5. **Case-insensitive equality parity with `strings.EqualFold(a.Hex(), b.Hex())`**
//!    — [`eq_fold`]: two address strings are equal iff they denote the same 20-byte
//!    address regardless of input casing/prefix; invalid inputs are never equal.
//!    This is the comparison the executor (`validatePersistedActionSender`) and the
//!    policy checks (`policy_basic`) rely on.
//!
//! These five points are exactly the address behaviors `internal/execution` and
//! `internal/app` depend on; lower-level signing lives in `signer.rs`, ABI in
//! `abi.rs`.

use alloy::primitives::Address as AlloyAddress;
use defi_errors::{Code, Error};

/// A validated 20-byte EVM address.
///
/// Construct via [`parse`] (strict) so the only way to hold an [`Address`] is
/// from an input go-ethereum's `common.IsHexAddress` would accept. Renders to
/// the canonical EIP-55 checksum via [`Address::to_hex`], matching go-ethereum's
/// `common.Address.Hex()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Address(AlloyAddress);

impl Address {
    /// The zero address (`common.Address{}` sentinel).
    ///
    /// Renders to `0x0000000000000000000000000000000000000000`.
    pub const ZERO: Address = Address(AlloyAddress::ZERO);

    /// The canonical EIP-55 checksum rendering, always `0x`-prefixed and 42
    /// characters long. Parity with go-ethereum `common.Address.Hex()`.
    pub fn to_hex(&self) -> String {
        self.0.to_checksum(None)
    }

    /// True only for the zero address — the `common.Address{}` sentinel the
    /// executor/backends compare against.
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    /// The raw 20-byte big-endian representation.
    pub fn as_bytes(&self) -> [u8; 20] {
        self.0.into_array()
    }

    /// The underlying `alloy` address, for handoff to the ABI/RPC/signer layers.
    pub fn into_inner(self) -> AlloyAddress {
        self.0
    }
}

impl From<AlloyAddress> for Address {
    fn from(inner: AlloyAddress) -> Self {
        Address(inner)
    }
}

impl From<Address> for AlloyAddress {
    fn from(addr: Address) -> Self {
        addr.0
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Validation parity with go-ethereum `common.IsHexAddress`.
///
/// Accepts an optional `0x`/`0X` prefix followed by **exactly 40** hex digits,
/// case-insensitive on the body. No EIP-55 checksum is enforced and no
/// whitespace is trimmed: any surrounding/internal whitespace, the bare prefix
/// `0x`, wrong lengths, and non-hex characters all return `false`. Never errors.
pub fn is_hex_address(s: &str) -> bool {
    hex_body(s).is_some()
}

/// The canonical EIP-55 checksum string for a valid address input.
///
/// Accepts any input [`is_hex_address`] accepts (any casing, with or without a
/// `0x`/`0X` prefix) and renders the exact mixed-case string go-ethereum's
/// `common.Address.Hex()` produces. Returns a usage-coded [`Error`] otherwise.
pub fn checksum(s: &str) -> Result<String, Error> {
    Ok(parse(s)?.to_hex())
}

/// Strictly parse an address string into a typed [`Address`].
///
/// Returns `Ok` for any input [`is_hex_address`] accepts and a usage-coded
/// [`Error`] otherwise. Unlike go-ethereum's lenient `HexToAddress` (which
/// silently right-aligns/truncates bad input), this surfaces the error rather
/// than corrupting the value — the observable contract ("valid in → checksum
/// out, invalid in → usage error") is preserved.
pub fn parse(s: &str) -> Result<Address, Error> {
    let body = hex_body(s).ok_or_else(|| {
        Error::new(
            Code::Usage,
            format!("{s:?} must be a valid EVM hex address"),
        )
    })?;
    // `body` is guaranteed to be exactly 40 ASCII hex digits here.
    let mut bytes = [0u8; 20];
    for (i, chunk) in bytes.iter_mut().enumerate() {
        let hi = hex_nibble(body.as_bytes()[i * 2]);
        let lo = hex_nibble(body.as_bytes()[i * 2 + 1]);
        match (hi, lo) {
            (Some(hi), Some(lo)) => *chunk = (hi << 4) | lo,
            _ => {
                // Unreachable: `hex_body` already validated every digit, but we
                // keep lib code panic-free.
                return Err(Error::new(
                    Code::Usage,
                    format!("{s:?} must be a valid EVM hex address"),
                ));
            }
        }
    }
    Ok(Address(AlloyAddress::from(bytes)))
}

/// Case-insensitive address equality, parity with
/// `strings.EqualFold(a.Hex(), b.Hex())`.
///
/// True iff both inputs are valid and denote the same 20-byte address,
/// regardless of casing or prefix. If either side is invalid, returns `false`
/// (an invalid address never equals anything).
pub fn eq_fold(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Returns the 40-char hex body (prefix stripped) iff `s` is a valid address
/// per go-ethereum `common.IsHexAddress` rules; otherwise `None`.
fn hex_body(s: &str) -> Option<&str> {
    let body = match s.as_bytes() {
        [b'0', b'x' | b'X', rest @ ..] => {
            // Re-slice as &str to keep char boundaries valid (ASCII prefix).
            std::str::from_utf8(rest).ok()?
        }
        _ => s,
    };
    if body.len() != 40 {
        return None;
    }
    if body.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(body)
    } else {
        None
    }
}

/// Decode a single ASCII hex digit to its nibble value.
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! RED phase: these reference the not-yet-implemented public API of this
    //! module. They MUST fail to compile / fail assertions until GREEN.
    use super::*;

    // Canonical EIP-55 reference vectors (lowercase input -> expected checksum),
    // the exact set go-ethereum's checksum implementation is verified against.
    // Confirmed byte-for-byte via a go-ethereum probe of common.HexToAddress(..).Hex().
    const EIP55_VECTORS: &[(&str, &str)] = &[
        (
            "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed",
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
        ),
        (
            "0xfb6916095ca1df60bb79ce92ce3ea74c37c5d359",
            "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
        ),
        (
            "0xdbf03b407c01e7cd3cbea99509d93f8dddc8c6fb",
            "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
        ),
        (
            "0xd1220a0cf47c7b9be7a2e6ba89f429762e7b9adb",
            "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
        ),
    ];

    // -------- 1. is_hex_address parity with go-ethereum common.IsHexAddress --------

    #[test]
    fn is_hex_address_accepts_lowercase_with_prefix() {
        assert!(is_hex_address("0xab5801a7d398351b8be11c439e05c5b3259aec9b"));
    }

    #[test]
    fn is_hex_address_accepts_mixed_case_checksum() {
        assert!(is_hex_address("0xAb5801a7D398351b8bE11C439e05C5B3259aeC9B"));
    }

    #[test]
    fn is_hex_address_accepts_without_prefix() {
        assert!(is_hex_address("ab5801a7d398351b8be11c439e05c5b3259aec9b"));
    }

    #[test]
    fn is_hex_address_accepts_uppercase_0x_prefix() {
        // go-ethereum accepts both the "0x" and "0X" prefix.
        assert!(is_hex_address("0Xab5801a7d398351b8be11c439e05c5b3259aec9b"));
    }

    #[test]
    fn is_hex_address_accepts_zero_and_dead() {
        assert!(is_hex_address("0x0000000000000000000000000000000000000000"));
        assert!(is_hex_address("0x000000000000000000000000000000000000dEaD"));
    }

    #[test]
    fn is_hex_address_rejects_empty() {
        assert!(!is_hex_address(""));
    }

    #[test]
    fn is_hex_address_rejects_bare_prefix() {
        assert!(!is_hex_address("0x"));
    }

    #[test]
    fn is_hex_address_rejects_too_short() {
        // 39 hex digits.
        assert!(!is_hex_address("0xab5801a7d398351b8be11c439e05c5b3259aec9"));
    }

    #[test]
    fn is_hex_address_rejects_too_long() {
        // 42 hex digits.
        assert!(!is_hex_address(
            "0xab5801a7d398351b8be11c439e05c5b3259aec9bff"
        ));
    }

    #[test]
    fn is_hex_address_rejects_non_hex_chars() {
        assert!(!is_hex_address(
            "0xZZZ801a7d398351b8be11c439e05c5b3259aec9b"
        ));
    }

    #[test]
    fn is_hex_address_rejects_surrounding_whitespace() {
        // go-ethereum does NOT trim; whitespace makes it invalid.
        assert!(!is_hex_address(
            "  0xab5801a7d398351b8be11c439e05c5b3259aec9b  "
        ));
    }

    // -------- 2. checksum rendering parity with Address.Hex() --------

    #[test]
    fn checksum_matches_eip55_reference_vectors() {
        for (input, want) in EIP55_VECTORS {
            let got = checksum(input).expect("valid address must checksum");
            assert_eq!(&got, want, "checksum mismatch for {input}");
        }
    }

    #[test]
    fn checksum_is_case_insensitive_on_input() {
        // Same address from lowercase, uppercase-hex, and already-checksummed input
        // must all render to the identical canonical EIP-55 string.
        let want = "0xAb5801a7D398351b8bE11C439e05C5B3259aeC9B";
        for input in [
            "0xab5801a7d398351b8be11c439e05c5b3259aec9b",
            "0xAB5801A7D398351B8BE11C439E05C5B3259AEC9B",
            "0xAb5801a7D398351b8bE11C439e05C5B3259aeC9B",
            "ab5801a7d398351b8be11c439e05c5b3259aec9b",
        ] {
            assert_eq!(checksum(input).unwrap(), want, "for input {input}");
        }
    }

    #[test]
    fn checksum_preserves_dead_vector_from_go_runner() {
        // The Go execution-identity + wallet tests assert this exact normalization:
        // lowercase "...dead" -> "...dEaD".
        assert_eq!(
            checksum("0x000000000000000000000000000000000000dead").unwrap(),
            "0x000000000000000000000000000000000000dEaD"
        );
    }

    #[test]
    fn checksum_zero_address() {
        assert_eq!(
            checksum("0x0000000000000000000000000000000000000000").unwrap(),
            "0x0000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn checksum_rejects_invalid_input() {
        assert!(checksum("0x123").is_err());
        assert!(checksum("not-an-address").is_err());
        assert!(checksum("").is_err());
    }

    // -------- 3. parse() strict + Address::to_hex round-trips --------

    #[test]
    fn parse_valid_returns_address_and_round_trips_to_checksum() {
        let addr =
            parse("0xab5801a7d398351b8be11c439e05c5b3259aec9b").expect("valid address must parse");
        assert_eq!(addr.to_hex(), "0xAb5801a7D398351b8bE11C439e05C5B3259aeC9B");
    }

    #[test]
    fn parse_accepts_no_prefix_and_mixed_case() {
        let a = parse("ab5801a7d398351b8be11c439e05c5b3259aec9b").unwrap();
        let b = parse("0xAb5801a7D398351b8bE11C439e05C5B3259aeC9B").unwrap();
        assert_eq!(a.to_hex(), b.to_hex());
    }

    #[test]
    fn parse_rejects_invalid_inputs() {
        for bad in [
            "",
            "0x",
            "0x123",
            "0xZZZ801a7d398351b8be11c439e05c5b3259aec9b",
            "0xab5801a7d398351b8be11c439e05c5b3259aec9", // 39
            "0xab5801a7d398351b8be11c439e05c5b3259aec9bff", // 42
            "  0xab5801a7d398351b8be11c439e05c5b3259aec9b  ",
        ] {
            assert!(parse(bad).is_err(), "expected {bad:?} to be rejected");
        }
    }

    #[test]
    fn parse_error_is_typed_and_displayable() {
        // Lib code must not panic; error surfaces as a typed, displayable error.
        let err = parse("not-an-address").unwrap_err();
        let msg = err.to_string();
        assert!(!msg.is_empty(), "error message must be non-empty");
    }

    // -------- 4. zero address --------

    #[test]
    fn zero_address_constant_and_predicate() {
        assert_eq!(
            Address::ZERO.to_hex(),
            "0x0000000000000000000000000000000000000000"
        );
        assert!(Address::ZERO.is_zero());

        let nonzero = parse("0x0000000000000000000000000000000000000001").unwrap();
        assert!(!nonzero.is_zero());
    }

    // -------- 5. eq_fold parity with strings.EqualFold(a.Hex(), b.Hex()) --------

    #[test]
    fn eq_fold_true_for_same_address_different_casing_and_prefix() {
        assert!(eq_fold(
            "0xab5801a7d398351b8be11c439e05c5b3259aec9b",
            "0xAb5801a7D398351b8bE11C439e05C5B3259aeC9B"
        ));
        assert!(eq_fold(
            "ab5801a7d398351b8be11c439e05c5b3259aec9b",
            "0xAB5801A7D398351B8BE11C439E05C5B3259AEC9B"
        ));
    }

    #[test]
    fn eq_fold_false_for_different_addresses() {
        assert!(!eq_fold(
            "0x0000000000000000000000000000000000000001",
            "0x0000000000000000000000000000000000000002"
        ));
    }

    #[test]
    fn eq_fold_false_when_either_side_invalid() {
        assert!(!eq_fold(
            "0xab5801a7d398351b8be11c439e05c5b3259aec9b",
            "not-an-address"
        ));
        assert!(!eq_fold(
            "garbage",
            "0xab5801a7d398351b8be11c439e05c5b3259aec9b"
        ));
        assert!(!eq_fold("", ""));
    }
}
