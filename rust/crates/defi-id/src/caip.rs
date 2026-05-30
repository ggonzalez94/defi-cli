//! CAIP-2 / CAIP-19 parsing, validation, and canonical formatting.
//!
//! Go source: `internal/id/id.go` (the CAIP-owned helpers: `chainNamespace`,
//! `parseCAIP2`, `lookupKnownCAIP2`'s split half, `caip2MatchesChain`,
//! `canonicalizeAddress`, `canonicalAssetID`, and the CAIP-19 parse/validate
//! branch of `ParseAsset`).
//!
//! This module owns the *pure, string-only* CAIP primitives. It is intentionally
//! independent of `chain.rs` (alias/chain resolution) and `tokens.rs` (registry
//! lookup): every function here operates on the canonical CAIP-2 chain-id string
//! (`eip155:1`, `solana:5eykt4Us…`) rather than a resolved `Chain`, exactly as
//! the Go helpers take `chainID string`. Symbol/address registry resolution and
//! `Chain` alias parsing live in their own modules and compose on top of this.

// =============================================================================
// SUCCESS CRITERIA (RED phase — tests written before implementation)
//
// This module (Go source: internal/id, CAIP helpers) owns the CAIP-2 / CAIP-19
// identifier contract (spec §2.4: "CAIP ids", "Amounts carry … CAIP-19",
// "require address or CAIP-19"). The Rust port is "correct" iff:
//
//   1. NAMESPACE EXTRACTION (Go `chainNamespace`).
//      `namespace("eip155:1") == "eip155"`, `namespace("solana:5eykt4Us…")
//      == "solana"`. The namespace is lowercased and surrounding whitespace on
//      the whole input is trimmed. A string with no ":" separator (or fewer than
//      two ":"-delimited parts) yields the empty namespace "".
//
//   2. CAIP-2 SPLIT (Go `parseCAIP2`).
//      `parse_caip2("EIP155:1") == Some(("eip155","1"))` — namespace is
//      lowercased, reference is trimmed but case-preserved. Whitespace around the
//      whole input and around each part is trimmed. An empty namespace or empty
//      reference, or a missing ":" separator, yields None. SplitN(_,":",2)
//      semantics: only the FIRST ":" splits, so the reference may itself contain
//      ":" (e.g. the eip155 ref never does, but solana refs are opaque).
//
//   3. ADDRESS CANONICALIZATION (Go `canonicalizeAddress`).
//      For an `eip155:*` chain id the address is trimmed and LOWERCASED
//      (checksum casing is discarded — canonical form is all-lowercase hex).
//      For any non-eip155 chain id (e.g. `solana:*`) the address is only trimmed,
//      preserving case (Solana base58 mints are case-sensitive).
//
//   4. CANONICAL ASSET ID (Go `canonicalAssetID`).
//      The asset id is `"<chainID>/<assetns>:<canonical-address>"` where the
//      asset namespace is chosen by the CHAIN namespace:
//        eip155 -> "erc20", solana -> "token", anything else -> "asset".
//      The embedded address is canonicalized per criterion 3 (lowercased for
//      eip155). Round-trip: parsing the produced asset id back must recover the
//      same chain id + canonical address.
//
//   5. CAIP-2 ↔ CHAIN MATCH (Go `caip2MatchesChain`).
//      Given a target chain's canonical CAIP-2 id:
//        - EVM (eip155) chain: input matches iff it equals the chain's CAIP-2
//          case-INSENSITIVELY ("EIP155:1" matches "eip155:1"). Whitespace
//          trimmed.
//        - Solana chain: input matches iff input parses as CAIP-2 with namespace
//          "solana" AND its reference equals the chain's reference EXACTLY
//          (case-sensitive on the reference; namespace case-insensitive).
//        - other namespace: exact (trimmed) string equality with the chain's
//          CAIP-2.
//
//   6. CAIP-19 PARSE + VALIDATE (Go `ParseAsset`, the `chainID/ns:addr` branch).
//      An input is treated as CAIP-19 iff it splits on the FIRST "/" into two
//      parts AND the second part contains ":". `parse_caip19` then:
//        a. NON-CAIP-19 input (no "/", or the part after "/" has no ":") ->
//           Ok(None): the caller falls through to symbol/address lookup. This is
//           load-bearing: "USDC/ETH" must NOT be a chain-mismatch error, it must
//           fall through (see Go TestParseAssetSlashWithoutCAIPNamespaceIsSymbol).
//        b. CHAIN MISMATCH: the chain-id part (before "/") must match the target
//           chain per criterion 5, else Err(Usage "asset chain does not match
//           --chain"). NOTE: mismatch is checked BEFORE inner-format validation.
//        c. INNER FORMAT by chain namespace:
//             eip155 chain: inner namespace MUST be "erc20" (case-insensitive)
//               AND address MUST match ^0x[0-9a-fA-F]{40}$, else
//               Err(Usage "invalid CAIP-19 asset format: <original input>").
//             solana chain: inner namespace MUST be "token" (case-insensitive)
//               AND address MUST match the base58 mint pattern
//               ^[1-9A-HJ-NP-Za-km-z]{32,44}$, else the same invalid-format Usage
//               error.
//             other chain namespace: Err(Unsupported "unsupported chain
//               namespace: <ns>").
//        d. SUCCESS -> Ok(Some(parts)) with: chain_id = the TARGET chain's
//           canonical CAIP-2 (NOT the raw input chain-id text — Go uses
//           chain.CAIP2); asset_namespace lowercased; address canonicalized per
//           criterion 3; asset_id = canonical_asset_id(chain.CAIP2, address).
//           Round-trip example: input
//           "EIP155:1/ERC20:0xA0B86991C6218B36C1D19D4A2E9EB0CE3606EB48" on chain
//           eip155:1 -> address 0xa0b8…eb48 (lowercased), asset_id
//           "eip155:1/erc20:0xa0b8…eb48".
//
//   7. ERROR CODES are the stable contract codes (spec §2.2): chain mismatch and
//      invalid CAIP-19 format -> Code::Usage (2); unsupported chain namespace ->
//      Code::Unsupported (13).
//
// Ported Go tests (meaningful, contract-relevant) re-expressed below:
//   TestParseAssetCAIP19MixedCaseEVM, TestParseAssetSolanaSymbolAndMint (CAIP-19
//   parts), TestParseAssetSlashWithoutCAIPNamespaceIsSymbolLookup,
//   TestParseAssetChainMismatch, TestParseAssetSolanaChainMismatch.
// Skipped: tests that exercise the token REGISTRY (symbol→address, decimals) —
//   those belong to tokens.rs / the crate-root parse_asset, not the pure CAIP
//   layer. Skipped: Go-internal helpers with no observable contract surface.
// =============================================================================

use defi_errors::{Code, Error};

/// EVM (eip155) address pattern: `0x` followed by exactly 40 hex digits.
///
/// Mirrors Go `evmAddressPattern` (`^0x[0-9a-fA-F]{40}$`).
fn is_evm_address(s: &str) -> bool {
    let Some(hex) = s.strip_prefix("0x") else {
        return false;
    };
    hex.len() == 40 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Solana base58 token-mint pattern: 32–44 base58 characters.
///
/// Mirrors Go `solanaTokenMintPattern` (`^[1-9A-HJ-NP-Za-km-z]{32,44}$`):
/// the base58 alphabet excludes `0`, `O`, `I`, and `l`.
fn is_solana_mint(s: &str) -> bool {
    const BASE58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let len = s.len();
    (32..=44).contains(&len) && s.bytes().all(|b| BASE58.contains(&b))
}

/// The lowercased CAIP-2 namespace of a chain id (Go `chainNamespace`).
///
/// Splits the trimmed input on the first `:`. A string with no `:` separator
/// (fewer than two `:`-delimited parts) yields the empty namespace `""`.
/// The namespace is lowercased; surrounding whitespace on the whole input is
/// trimmed.
pub fn namespace(caip2: &str) -> String {
    match caip2.trim().split_once(':') {
        Some((ns, _)) => ns.to_lowercase(),
        None => String::new(),
    }
}

/// Split a CAIP-2 chain id into `(namespace, reference)` (Go `parseCAIP2`).
///
/// Returns `None` when there is no `:` separator, or when either the namespace
/// or the reference is empty after trimming. The namespace is lowercased; the
/// reference is trimmed but keeps its original case (Solana references are
/// case-sensitive). `SplitN(_, ":", 2)` semantics: only the FIRST `:` splits,
/// so the reference may itself contain `:`.
pub fn parse_caip2(input: &str) -> Option<(String, String)> {
    let (ns_part, ref_part) = input.trim().split_once(':')?;
    let namespace = ns_part.trim().to_lowercase();
    let reference = ref_part.trim().to_string();
    if namespace.is_empty() || reference.is_empty() {
        return None;
    }
    Some((namespace, reference))
}

/// Canonicalize an address for a given CAIP-2 chain id (Go `canonicalizeAddress`).
///
/// For an `eip155:*` chain the address is trimmed and LOWERCASED (checksum
/// casing is discarded). For any non-eip155 chain (e.g. `solana:*`) the address
/// is only trimmed, preserving case.
pub fn canonicalize_address(chain_id: &str, address: &str) -> String {
    let addr = address.trim();
    if namespace(chain_id) == "eip155" {
        addr.to_lowercase()
    } else {
        addr.to_string()
    }
}

/// The canonical asset id for an address on a chain (Go `canonicalAssetID`).
///
/// Produces `"<chainID>/<assetns>:<canonical-address>"` where the asset
/// namespace is chosen by the chain namespace: `eip155 -> erc20`,
/// `solana -> token`, anything else -> `asset`. The embedded address is
/// canonicalized via [`canonicalize_address`].
pub fn canonical_asset_id(chain_id: &str, address: &str) -> String {
    let addr = canonicalize_address(chain_id, address);
    let asset_ns = match namespace(chain_id).as_str() {
        "eip155" => "erc20",
        "solana" => "token",
        _ => "asset",
    };
    format!("{chain_id}/{asset_ns}:{addr}")
}

/// Whether a CAIP-2 input refers to the same chain as `chain_caip2`
/// (Go `caip2MatchesChain`).
///
/// - EVM (`eip155`) target: input matches iff it equals the chain's CAIP-2
///   case-INSENSITIVELY (whitespace trimmed).
/// - Solana target: input matches iff it parses as CAIP-2 with namespace
///   `solana` AND its reference equals the chain's reference EXACTLY
///   (case-sensitive on the reference; namespace case-insensitive).
/// - other namespace: exact (trimmed) string equality with the chain's CAIP-2.
pub fn caip2_matches_chain(input: &str, chain_caip2: &str) -> bool {
    match namespace(chain_caip2).as_str() {
        "eip155" => input.trim().eq_ignore_ascii_case(chain_caip2),
        "solana" => {
            let Some((input_ns, input_ref)) = parse_caip2(input) else {
                return false;
            };
            if input_ns != "solana" {
                return false;
            }
            match parse_caip2(chain_caip2) {
                Some((chain_ns, chain_ref)) if chain_ns == "solana" => input_ref == chain_ref,
                _ => false,
            }
        }
        _ => input.trim() == chain_caip2,
    }
}

/// The canonical parts of a parsed CAIP-19 asset identifier.
///
/// Returned by [`parse_caip19`] on a successful parse. `chain_id` is the
/// TARGET chain's canonical CAIP-2 (not the raw input chain-id text);
/// `asset_namespace` is lowercased; `address` is canonicalized via
/// [`canonicalize_address`]; `asset_id` is [`canonical_asset_id`] of the pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caip19Parts {
    pub chain_id: String,
    pub asset_namespace: String,
    pub address: String,
    pub asset_id: String,
}

/// Parse a possible CAIP-19 asset id against a target chain
/// (Go `ParseAsset`, the `chainID/ns:addr` branch).
///
/// An input is treated as CAIP-19 iff it splits on the FIRST `/` into two parts
/// AND the second part contains `:`. Otherwise this returns `Ok(None)` so the
/// caller can fall through to symbol/address lookup (load-bearing: `USDC/ETH`
/// must NOT be a chain-mismatch error).
///
/// On a CAIP-19-shaped input:
/// - the chain-id part (before `/`) must match `chain_caip2` per
///   [`caip2_matches_chain`], else `Err(Usage "asset chain does not match
///   --chain")` (checked BEFORE inner-format validation);
/// - the inner format is validated by the chain namespace: `eip155` requires
///   inner ns `erc20` + a `0x…40hex` address; `solana` requires inner ns
///   `token` + a base58 mint; any other chain namespace yields
///   `Err(Unsupported "unsupported chain namespace: <ns>")`;
/// - an invalid inner format yields `Err(Usage "invalid CAIP-19 asset format:
///   <original input>")`.
pub fn parse_caip19(input: &str, chain_caip2: &str) -> Result<Option<Caip19Parts>, Error> {
    let trimmed = input.trim();

    // CAIP-19 detection: split on the FIRST '/' into two parts, the second of
    // which must contain ':'. Anything else falls through (Ok(None)).
    let Some((chain_part, asset_part)) = trimmed.split_once('/') else {
        return Ok(None);
    };
    if !asset_part.contains(':') {
        return Ok(None);
    }

    // Chain mismatch is checked BEFORE inner-format validation.
    let chain_id_part = chain_part.trim();
    if !caip2_matches_chain(chain_id_part, chain_caip2) {
        return Err(Error::new(
            Code::Usage,
            "asset chain does not match --chain",
        ));
    }

    // Inner asset format: SplitN(_, ":", 2) — only the first ':' splits.
    let invalid_format = || {
        Error::new(
            Code::Usage,
            format!("invalid CAIP-19 asset format: {input}"),
        )
    };
    let (inner_ns_part, address_part) = asset_part.split_once(':').ok_or_else(invalid_format)?;
    let asset_namespace = inner_ns_part.trim().to_lowercase();
    let address_raw = address_part.trim();

    let chain_ns = namespace(chain_caip2);
    match chain_ns.as_str() {
        "eip155" => {
            if asset_namespace != "erc20" || !is_evm_address(address_raw) {
                return Err(invalid_format());
            }
        }
        "solana" => {
            if asset_namespace != "token" || !is_solana_mint(address_raw) {
                return Err(invalid_format());
            }
        }
        _ => {
            return Err(Error::new(
                Code::Unsupported,
                format!("unsupported chain namespace: {chain_ns}"),
            ));
        }
    }

    let address = canonicalize_address(chain_caip2, address_raw);
    Ok(Some(Caip19Parts {
        chain_id: chain_caip2.to_string(),
        asset_namespace,
        asset_id: canonical_asset_id(chain_caip2, address_raw),
        address,
    }))
}

#[cfg(test)]
mod tests {
    use defi_errors::Code;

    // The Solana mainnet CAIP-2 reference + id, mirrored from internal/id/id.go.
    const SOL_REF: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";
    const SOL_CAIP2: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";
    // USDC on Ethereum mainnet, in checksum casing (mixed case) to exercise
    // canonicalization (lowercasing).
    const USDC_CHECKSUM: &str = "0xA0B86991C6218B36C1D19D4A2E9EB0CE3606EB48";
    const USDC_LOWER: &str = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
    const SOL_MINT: &str = "So11111111111111111111111111111111111111112";

    // ---- Criterion 1: namespace extraction (Go chainNamespace) -----------

    #[test]
    fn namespace_extracts_and_lowercases() {
        assert_eq!(crate::caip::namespace("eip155:1"), "eip155");
        assert_eq!(crate::caip::namespace(SOL_CAIP2), "solana");
    }

    #[test]
    fn namespace_lowercases_uppercase_input() {
        assert_eq!(crate::caip::namespace("EIP155:1"), "eip155");
        assert_eq!(crate::caip::namespace("SOLANA:abc"), "solana");
    }

    #[test]
    fn namespace_trims_surrounding_whitespace() {
        assert_eq!(crate::caip::namespace("  eip155:1  "), "eip155");
    }

    #[test]
    fn namespace_of_non_caip_is_empty() {
        assert_eq!(crate::caip::namespace("notacaip"), "");
        assert_eq!(crate::caip::namespace(""), "");
    }

    // ---- Criterion 2: CAIP-2 split (Go parseCAIP2) -----------------------

    #[test]
    fn parse_caip2_lowercases_namespace_preserves_reference_case() {
        let (ns, reference) =
            crate::caip::parse_caip2("EIP155:1").expect("valid CAIP-2 must parse");
        assert_eq!(ns, "eip155");
        assert_eq!(reference, "1");

        let (ns, reference) =
            crate::caip::parse_caip2(SOL_CAIP2).expect("solana CAIP-2 must parse");
        assert_eq!(ns, "solana");
        // Reference keeps its original (case-sensitive) casing.
        assert_eq!(reference, SOL_REF);
    }

    #[test]
    fn parse_caip2_trims_whitespace_around_parts() {
        let (ns, reference) =
            crate::caip::parse_caip2("  eip155 : 1  ").expect("must parse with spaces");
        assert_eq!(ns, "eip155");
        assert_eq!(reference, "1");
    }

    #[test]
    fn parse_caip2_rejects_missing_separator_or_empty_parts() {
        assert!(crate::caip::parse_caip2("eip155").is_none());
        assert!(crate::caip::parse_caip2("eip155:").is_none());
        assert!(crate::caip::parse_caip2(":1").is_none());
        assert!(crate::caip::parse_caip2("").is_none());
    }

    #[test]
    fn parse_caip2_splits_only_on_first_colon() {
        // SplitN(_, ":", 2): the reference may itself contain a ":".
        let (ns, reference) =
            crate::caip::parse_caip2("eip155:1:extra").expect("first-colon split");
        assert_eq!(ns, "eip155");
        assert_eq!(reference, "1:extra");
    }

    // ---- Criterion 3: address canonicalization (Go canonicalizeAddress) --

    #[test]
    fn canonicalize_address_lowercases_for_eip155() {
        assert_eq!(
            crate::caip::canonicalize_address("eip155:1", USDC_CHECKSUM),
            USDC_LOWER
        );
    }

    #[test]
    fn canonicalize_address_preserves_case_for_solana() {
        assert_eq!(
            crate::caip::canonicalize_address(SOL_CAIP2, SOL_MINT),
            SOL_MINT
        );
    }

    #[test]
    fn canonicalize_address_trims_whitespace() {
        assert_eq!(
            crate::caip::canonicalize_address(
                "eip155:1",
                "  0xABCDEF0123456789abcdef0123456789ABCDEF01  "
            ),
            "0xabcdef0123456789abcdef0123456789abcdef01"
        );
        // Solana: trims but preserves case.
        assert_eq!(
            crate::caip::canonicalize_address(SOL_CAIP2, "  SoMixedCase  "),
            "SoMixedCase"
        );
    }

    // ---- Criterion 4: canonical asset id (Go canonicalAssetID) -----------

    #[test]
    fn canonical_asset_id_eip155_uses_erc20_and_lowercases() {
        assert_eq!(
            crate::caip::canonical_asset_id("eip155:1", USDC_CHECKSUM),
            format!("eip155:1/erc20:{USDC_LOWER}")
        );
    }

    #[test]
    fn canonical_asset_id_solana_uses_token_and_preserves_case() {
        assert_eq!(
            crate::caip::canonical_asset_id(SOL_CAIP2, SOL_MINT),
            format!("{SOL_CAIP2}/token:{SOL_MINT}")
        );
    }

    #[test]
    fn canonical_asset_id_other_namespace_uses_asset() {
        // A namespace that is neither eip155 nor solana falls to the default
        // "asset" branch (Go switch default).
        assert_eq!(
            crate::caip::canonical_asset_id("cosmos:cosmoshub-4", "uatom"),
            "cosmos:cosmoshub-4/asset:uatom"
        );
    }

    // ---- Criterion 5: CAIP-2 ↔ chain match (Go caip2MatchesChain) --------

    #[test]
    fn caip2_matches_evm_chain_case_insensitively() {
        assert!(crate::caip::caip2_matches_chain("EIP155:1", "eip155:1"));
        assert!(crate::caip::caip2_matches_chain("  eip155:1  ", "eip155:1"));
        assert!(!crate::caip::caip2_matches_chain("eip155:8453", "eip155:1"));
    }

    #[test]
    fn caip2_matches_solana_chain_by_reference_case_sensitive() {
        // Namespace case-insensitive, reference case-sensitive.
        assert!(crate::caip::caip2_matches_chain(
            &format!("SOLANA:{SOL_REF}"),
            SOL_CAIP2
        ));
        // Different (lowercased) reference must NOT match.
        let lower_ref = SOL_REF.to_lowercase();
        assert!(!crate::caip::caip2_matches_chain(
            &format!("solana:{lower_ref}"),
            SOL_CAIP2
        ));
        // Wrong namespace must not match a solana chain.
        assert!(!crate::caip::caip2_matches_chain("eip155:1", SOL_CAIP2));
    }

    // ---- Criterion 6a: non-CAIP-19 input falls through (Ok(None)) --------

    #[test]
    fn parse_caip19_returns_none_when_no_slash() {
        // Plain symbol / bare address: not CAIP-19, caller falls through.
        let got =
            crate::caip::parse_caip19("USDC", "eip155:1").expect("non-caip19 input must not error");
        assert!(got.is_none(), "plain symbol must fall through to None");
    }

    #[test]
    fn parse_caip19_slash_without_inner_colon_is_none() {
        // Ports Go TestParseAssetSlashWithoutCAIPNamespaceIsSymbolLookup:
        // "USDC/ETH" has a "/" but the second part has no ":", so it is NOT
        // CAIP-19 and must fall through (Ok(None)) — NOT a chain-mismatch error.
        let got = crate::caip::parse_caip19("USDC/ETH", "eip155:1")
            .expect("slash-without-colon must not error, must fall through");
        assert!(got.is_none(), "USDC/ETH must fall through to symbol lookup");
    }

    // ---- Criterion 6b: chain mismatch -> Usage error ---------------------

    #[test]
    fn parse_caip19_chain_mismatch_is_usage_error() {
        // Ports Go TestParseAssetChainMismatch: asset is on eip155:1 but target
        // chain is eip155:8453.
        let err = crate::caip::parse_caip19(&format!("eip155:1/erc20:{USDC_LOWER}"), "eip155:8453")
            .expect_err("chain mismatch must be an error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(err.message, "asset chain does not match --chain");
    }

    #[test]
    fn parse_caip19_solana_chain_mismatch_is_error() {
        // Ports Go TestParseAssetSolanaChainMismatch: the CAIP-19 chain reference
        // is a different solana reference than the target chain.
        let err = crate::caip::parse_caip19(
            &format!("solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1/token:{SOL_MINT}"),
            SOL_CAIP2,
        )
        .expect_err("solana chain mismatch must be an error");
        assert_eq!(err.code, Code::Usage);
    }

    // ---- Criterion 6c: inner-format validation ---------------------------

    #[test]
    fn parse_caip19_evm_wrong_inner_namespace_is_invalid_format() {
        // eip155 chain requires inner ns "erc20"; "token" is invalid.
        let input = format!("eip155:1/token:{USDC_LOWER}");
        let err =
            crate::caip::parse_caip19(&input, "eip155:1").expect_err("wrong inner ns must error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(
            err.message,
            format!("invalid CAIP-19 asset format: {input}")
        );
    }

    #[test]
    fn parse_caip19_evm_bad_address_is_invalid_format() {
        let input = "eip155:1/erc20:0xnothex";
        let err =
            crate::caip::parse_caip19(input, "eip155:1").expect_err("bad evm address must error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(
            err.message,
            format!("invalid CAIP-19 asset format: {input}")
        );
    }

    #[test]
    fn parse_caip19_solana_wrong_inner_namespace_is_invalid_format() {
        // solana chain requires inner ns "token"; "erc20" is invalid.
        let input = format!("{SOL_CAIP2}/erc20:{SOL_MINT}");
        let err = crate::caip::parse_caip19(&input, SOL_CAIP2)
            .expect_err("wrong inner ns on solana must error");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn parse_caip19_unsupported_chain_namespace_is_unsupported() {
        // A target chain whose namespace is neither eip155 nor solana hits the
        // "unsupported chain namespace" branch (Code::Unsupported).
        let input = "cosmos:cosmoshub-4/asset:uatom";
        let err = crate::caip::parse_caip19(input, "cosmos:cosmoshub-4")
            .expect_err("non-evm/non-solana chain must be unsupported");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(err.message, "unsupported chain namespace: cosmos");
    }

    // ---- Criterion 6d: success -> canonical parts + round-trip -----------

    #[test]
    fn parse_caip19_evm_mixed_case_canonicalizes() {
        // Ports Go TestParseAssetCAIP19MixedCaseEVM: mixed-case namespace AND
        // address must canonicalize to all-lowercase.
        let input = format!("EIP155:1/ERC20:{USDC_CHECKSUM}");
        let parts = crate::caip::parse_caip19(&input, "eip155:1")
            .expect("valid CAIP-19 must not error")
            .expect("valid CAIP-19 must yield Some");

        // chain_id is the TARGET chain's canonical CAIP-2.
        assert_eq!(parts.chain_id, "eip155:1");
        // inner namespace lowercased.
        assert_eq!(parts.asset_namespace, "erc20");
        // address canonicalized (lowercased).
        assert_eq!(parts.address, USDC_LOWER);
        // asset_id round-trips through canonical_asset_id.
        assert_eq!(parts.asset_id, format!("eip155:1/erc20:{USDC_LOWER}"));
        assert_eq!(
            parts.asset_id,
            crate::caip::canonical_asset_id("eip155:1", USDC_CHECKSUM)
        );
    }

    #[test]
    fn parse_caip19_solana_preserves_mint_case() {
        // Ports the CAIP-19 leg of Go TestParseAssetSolanaSymbolAndMint: solana
        // mint address is case-sensitive (NOT lowercased).
        let input = format!("{SOL_CAIP2}/token:{SOL_MINT}");
        let parts = crate::caip::parse_caip19(&input, SOL_CAIP2)
            .expect("valid solana CAIP-19 must not error")
            .expect("valid solana CAIP-19 must yield Some");

        assert_eq!(parts.chain_id, SOL_CAIP2);
        assert_eq!(parts.asset_namespace, "token");
        assert_eq!(parts.address, SOL_MINT); // case preserved
        assert_eq!(parts.asset_id, format!("{SOL_CAIP2}/token:{SOL_MINT}"));
    }

    #[test]
    fn parse_caip19_solana_uppercase_namespaces_canonicalize() {
        // Ports the uppercase-CAIP-19 case from Go TestParseAssetSolanaSymbolAndMint
        // (asset4): "SOLANA:…/TOKEN:…" must parse, lowercasing namespaces while
        // keeping the mint case-sensitive.
        let input = format!("SOLANA:{SOL_REF}/TOKEN:{SOL_MINT}");
        let parts = crate::caip::parse_caip19(&input, SOL_CAIP2)
            .expect("uppercase solana CAIP-19 must not error")
            .expect("uppercase solana CAIP-19 must yield Some");
        assert_eq!(parts.chain_id, SOL_CAIP2);
        assert_eq!(parts.asset_namespace, "token");
        assert_eq!(parts.address, SOL_MINT);
    }
}
