//! Chain parsing: CAIP-2, numeric chain IDs, and the alias set.
//!
//! Go source: `internal/id/id.go` — the chain-resolution surface: the `Chain`
//! type (+ `Namespace`/`IsEVM`/`IsSolana`), the `chainBySlug` / `chainByID` /
//! `chainByCAIP2` registries, `ParseChain`, and `ListChains` / `ChainEntry`.
//!
//! This module owns *chain identity resolution* (alias/numeric/CAIP-2 → `Chain`)
//! and the deduped, sorted chain listing. It composes on top of the pure CAIP
//! string primitives in `caip.rs` (namespace extraction, CAIP-2 split) but does
//! NOT touch the token registry (`tokens.rs`) or amount math (`amount.rs`).

use crate::caip::{namespace, parse_caip2};
use defi_errors::{Code, Error};

/// The Solana mainnet CAIP-2 reference (Go `solanaMainnetRef`).
const SOLANA_MAINNET_REF: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";
/// The Solana mainnet CAIP-2 chain id (Go `solanaMainnetCAIP2`).
const SOLANA_MAINNET_CAIP2: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";

/// A canonical chain reference.
///
/// Field declaration order mirrors Go `id.Chain` (`Name, Slug, CAIP2,
/// EVMChainID`) so any future serde projection keeps contract field order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chain {
    pub name: String,
    pub slug: String,
    pub caip2: String,
    pub evm_chain_id: i64,
}

impl Chain {
    /// The lowercased CAIP-2 namespace of this chain (Go `Chain.Namespace`).
    pub fn namespace(&self) -> String {
        namespace(&self.caip2)
    }

    /// Whether this chain is an EVM (`eip155`) chain (Go `Chain.IsEVM`).
    pub fn is_evm(&self) -> bool {
        self.namespace() == "eip155"
    }

    /// Whether this chain is a Solana chain (Go `Chain.IsSolana`).
    pub fn is_solana(&self) -> bool {
        self.namespace() == "solana"
    }
}

/// A chain with its accepted aliases (Go `ChainEntry`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainEntry {
    pub chain: Chain,
    pub aliases: Vec<String>,
}

/// Construct an EVM chain entry tersely. The CAIP-2 is `eip155:<id>`.
fn evm(name: &str, slug: &str, id: i64) -> Chain {
    Chain {
        name: name.to_string(),
        slug: slug.to_string(),
        caip2: format!("eip155:{id}"),
        evm_chain_id: id,
    }
}

/// The solana mainnet chain (no EVM chain id).
fn solana_chain() -> Chain {
    Chain {
        name: "Solana".to_string(),
        slug: "solana".to_string(),
        caip2: SOLANA_MAINNET_CAIP2.to_string(),
        evm_chain_id: 0,
    }
}

/// The `(alias, Chain)` registry (Go `chainBySlug`).
///
/// Each entry maps a lowercase alias to the canonical [`Chain`] it resolves to;
/// the resolved chain carries its canonical `slug` (not the alias).
fn chain_by_slug() -> Vec<(&'static str, Chain)> {
    vec![
        ("ethereum", evm("Ethereum", "ethereum", 1)),
        ("mainnet", evm("Ethereum", "ethereum", 1)),
        ("optimism", evm("Optimism", "optimism", 10)),
        ("op mainnet", evm("Optimism", "optimism", 10)),
        ("op-mainnet", evm("Optimism", "optimism", 10)),
        ("bsc", evm("BSC", "bsc", 56)),
        ("gnosis", evm("Gnosis", "gnosis", 100)),
        ("xdai", evm("Gnosis", "gnosis", 100)),
        ("polygon", evm("Polygon", "polygon", 137)),
        ("monad", evm("Monad", "monad", 143)),
        ("sonic", evm("Sonic", "sonic", 146)),
        ("fraxtal", evm("Fraxtal", "fraxtal", 252)),
        ("zksync", evm("zkSync Era", "zksync", 324)),
        ("zksync era", evm("zkSync Era", "zksync", 324)),
        ("zksync-era", evm("zkSync Era", "zksync", 324)),
        ("tempo", evm("Tempo", "tempo", 4217)),
        ("tempo mainnet", evm("Tempo", "tempo", 4217)),
        ("tempo-mainnet", evm("Tempo", "tempo", 4217)),
        ("presto", evm("Tempo", "tempo", 4217)),
        ("worldchain", evm("World Chain", "world-chain", 480)),
        ("world chain", evm("World Chain", "world-chain", 480)),
        ("world-chain", evm("World Chain", "world-chain", 480)),
        ("hyperevm", evm("HyperEVM", "hyperevm", 999)),
        ("hyper evm", evm("HyperEVM", "hyperevm", 999)),
        ("hyper-evm", evm("HyperEVM", "hyperevm", 999)),
        ("citrea", evm("Citrea", "citrea", 4114)),
        ("mantle", evm("Mantle", "mantle", 5000)),
        ("megaeth", evm("MegaETH", "megaeth", 4326)),
        ("mega eth", evm("MegaETH", "megaeth", 4326)),
        ("mega-eth", evm("MegaETH", "megaeth", 4326)),
        (
            "tempo testnet",
            evm("Tempo Moderato", "tempo-moderato", 42431),
        ),
        (
            "tempo-testnet",
            evm("Tempo Moderato", "tempo-moderato", 42431),
        ),
        ("moderato", evm("Tempo Moderato", "tempo-moderato", 42431)),
        ("base", evm("Base", "base", 8453)),
        ("blast", evm("Blast", "blast", 81457)),
        ("berachain", evm("Berachain", "berachain", 80094)),
        ("arbitrum", evm("Arbitrum", "arbitrum", 42161)),
        ("avalanche", evm("Avalanche", "avalanche", 43114)),
        ("tempo devnet", evm("Tempo Devnet", "tempo-devnet", 31318)),
        ("tempo-devnet", evm("Tempo Devnet", "tempo-devnet", 31318)),
        ("linea", evm("Linea", "linea", 59144)),
        ("ink", evm("Ink", "ink", 57073)),
        ("scroll", evm("Scroll", "scroll", 534352)),
        ("celo", evm("Celo", "celo", 42220)),
        ("taiko", evm("Taiko", "taiko", 167000)),
        ("taiko alethia", evm("Taiko", "taiko", 167000)),
        ("taiko-alethia", evm("Taiko", "taiko", 167000)),
        ("taiko hoodi", evm("Taiko Hoodi", "taiko-hoodi", 167013)),
        ("taiko-hoodi", evm("Taiko Hoodi", "taiko-hoodi", 167013)),
        ("hoodi", evm("Taiko Hoodi", "taiko-hoodi", 167013)),
        ("solana", solana_chain()),
        ("solana-mainnet", solana_chain()),
        ("mainnet-beta", solana_chain()),
    ]
}

/// Look up a chain by its registered EVM chain id (Go `chainByID`).
///
/// Only the IDs that have an explicit `chainByID` row in Go resolve here; any
/// other numeric id falls through to a synthesized `EVM-<id>` chain.
fn chain_by_id(id: i64) -> Option<Chain> {
    let slug = match id {
        1 => "ethereum",
        10 => "optimism",
        56 => "bsc",
        100 => "gnosis",
        137 => "polygon",
        143 => "monad",
        999 => "hyperevm",
        4114 => "citrea",
        146 => "sonic",
        252 => "fraxtal",
        324 => "zksync",
        4217 => "tempo",
        480 => "world-chain",
        5000 => "mantle",
        4326 => "megaeth",
        8453 => "base",
        42220 => "celo",
        42161 => "arbitrum",
        42431 => "moderato",
        43114 => "avalanche",
        57073 => "ink",
        59144 => "linea",
        80094 => "berachain",
        81457 => "blast",
        167000 => "taiko",
        167013 => "taiko-hoodi",
        31318 => "tempo-devnet",
        534352 => "scroll",
        _ => return None,
    };
    chain_by_slug()
        .into_iter()
        .find(|(alias, _)| *alias == slug)
        .map(|(_, chain)| chain)
}

/// Look up a chain by its canonical CAIP-2 id (Go `chainByCAIP2`).
fn chain_by_caip2(caip2: &str) -> Option<Chain> {
    chain_by_slug()
        .into_iter()
        .find(|(_, chain)| chain.caip2 == caip2)
        .map(|(_, chain)| chain)
}

/// Whether a string is a base58 Solana token-mint pattern (Go
/// `solanaTokenMintPattern`, `^[1-9A-HJ-NP-Za-km-z]{32,44}$`).
fn is_solana_mint(s: &str) -> bool {
    const BASE58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let len = s.len();
    (32..=44).contains(&len) && s.bytes().all(|b| BASE58.contains(&b))
}

/// Whether a string matches the `eip155:<digits>` pattern (Go
/// `eip155ChainPattern`, `^eip155:[0-9]+$`).
fn is_eip155_pattern(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("eip155:") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
}

/// Resolve a `--chain` input to a canonical [`Chain`] (Go `ParseChain`).
///
/// Accepts a known alias (case-insensitive), a bare numeric chain id, an
/// `eip155:N` id, or a `solana:<ref>` CAIP-2 id. Empty/whitespace input is a
/// usage error; unknown input is a usage error; solana devnet/testnet and
/// non-mainnet solana references are unsupported.
pub fn parse_chain(input: &str) -> Result<Chain, Error> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(Error::new(Code::Usage, "chain is required"));
    }
    let norm = raw.to_lowercase();

    if norm == "solana-devnet" || norm == "solana-testnet" {
        return Err(Error::new(
            Code::Unsupported,
            "solana devnet/testnet are not supported; only solana mainnet is supported",
        ));
    }

    if let Some((_, chain)) = chain_by_slug()
        .into_iter()
        .find(|(alias, _)| *alias == norm)
    {
        return Ok(chain);
    }

    if is_eip155_pattern(&norm) {
        // The pattern guarantees the part after the colon is all digits.
        let id: i64 = norm
            .split_once(':')
            .and_then(|(_, n)| n.parse().ok())
            .unwrap_or(0);
        if let Some(known) = chain_by_id(id) {
            return Ok(known);
        }
        return Ok(Chain {
            name: format!("EVM-{id}"),
            slug: format!("evm-{id}"),
            caip2: norm,
            evm_chain_id: id,
        });
    }

    if let Some((ns, reference)) = parse_caip2(raw) {
        if ns == "solana" {
            if reference == SOLANA_MAINNET_REF {
                if let Some(known) = chain_by_caip2(SOLANA_MAINNET_CAIP2) {
                    return Ok(known);
                }
                return Ok(solana_chain());
            }
            if is_solana_mint(&reference) {
                return Err(Error::new(
                    Code::Unsupported,
                    "solana non-mainnet references are not supported; only solana mainnet is supported",
                ));
            }
            return Err(Error::new(
                Code::Usage,
                format!("unsupported chain input: {input}"),
            ));
        }
    }

    if let Some(chain) = chain_by_caip2(raw) {
        return Ok(chain);
    }

    if let Ok(id) = norm.parse::<i64>() {
        if let Some(chain) = chain_by_id(id) {
            return Ok(chain);
        }
        return Ok(Chain {
            name: format!("EVM-{id}"),
            slug: format!("evm-{id}"),
            caip2: format!("eip155:{id}"),
            evm_chain_id: id,
        });
    }

    Err(Error::new(
        Code::Usage,
        format!("unsupported chain input: {input}"),
    ))
}

/// List all unique supported chains sorted by CAIP-2 id (Go `ListChains`).
///
/// Entries are deduped by CAIP-2 and sorted ascending by CAIP-2. Each entry's
/// `aliases` are the slugs that map to that chain EXCLUDING the primary slug,
/// sorted ascending.
pub fn list_chains() -> Vec<ChainEntry> {
    use std::collections::HashMap;

    let mut seen: HashMap<String, ChainEntry> = HashMap::new();
    for (slug, chain) in chain_by_slug() {
        let entry = seen
            .entry(chain.caip2.clone())
            .or_insert_with(|| ChainEntry {
                chain: chain.clone(),
                aliases: Vec::new(),
            });
        if slug != entry.chain.slug {
            entry.aliases.push(slug.to_string());
        }
    }

    let mut entries: Vec<ChainEntry> = seen.into_values().collect();
    for e in &mut entries {
        e.aliases.sort();
    }
    entries.sort_by(|a, b| a.chain.caip2.cmp(&b.chain.caip2));
    entries
}

// =============================================================================
// SUCCESS CRITERIA (RED phase — tests written before implementation)
//
// This module (Go source: internal/id, chain-resolution surface) owns the
// `--chain` input contract (spec §2.4: "--chain accepts CAIP-2, numeric chain
// IDs, and a fixed alias set"). The Rust port is "correct" iff:
//
//   1. CHAIN TYPE + NAMESPACE PREDICATES (Go Chain.Namespace/IsEVM/IsSolana).
//      `Chain::namespace()` is the lowercased CAIP-2 namespace (`eip155`,
//      `solana`). `is_evm()` == (namespace == "eip155"); `is_solana()` ==
//      (namespace == "solana"). Field declaration order is Name, Slug, CAIP2,
//      EVMChainID (mirrors Go for contract field ordering).
//
//   2. ALIAS RESOLUTION (Go chainBySlug, case-insensitive on the whole input).
//      The fixed alias set resolves to the canonical Chain. Examples that MUST
//      hold (ported from Go TestParseChainExpandedCoverage):
//        "base"->eip155:8453/base, "op mainnet"/"op-mainnet"->eip155:10/optimism,
//        "xdai"/"gnosis"->eip155:100/gnosis, "presto"/"tempo mainnet"/"tempo"->
//        eip155:4217/tempo, "moderato"/"tempo testnet"->eip155:42431/
//        tempo-moderato, "tempo devnet"->eip155:31318/tempo-devnet, "hoodi"/
//        "taiko hoodi"->eip155:167013/taiko-hoodi, "taiko alethia"/"taiko"->
//        eip155:167000/taiko, "zksync era"/"zksync"->eip155:324/zksync,
//        "mega eth"/"megaeth"->eip155:4326/megaeth, "world chain"/"worldchain"/
//        "world-chain"->eip155:480/world-chain, "hyper evm"/"hyperevm"->
//        eip155:999/hyperevm, plus mantle, ink, scroll, berachain, monad, linea,
//        sonic, blast, fraxtal, citrea, celo. Each resolved Chain carries the
//        canonical Slug (NOT the alias) and the matching EVMChainID + CAIP2.
//      Resolution is CASE-INSENSITIVE: "BASE", "Base", "base" all resolve.
//
//   3. NUMERIC CHAIN ID (Go strconv path + chainByID).
//      A bare decimal integer resolves to the known Chain when registered
//      ("8453"->base, "324"->zksync, "143"->monad, …, ported from Go), with the
//      canonical Slug/CAIP2/EVMChainID. An UNKNOWN numeric id (e.g. "999999")
//      synthesizes Chain{ Name:"EVM-999999", Slug:"evm-999999",
//      CAIP2:"eip155:999999", EVMChainID:999999 } — never an error.
//
//   4. eip155:N PASSTHROUGH (Go eip155ChainPattern branch).
//      "eip155:8453" resolves to the known base Chain. "eip155:999999" (unknown)
//      synthesizes Chain{ Name:"EVM-999999", Slug:"evm-999999",
//      CAIP2:"eip155:999999", EVMChainID:999999 } (ported from Go
//      TestParseChainVariants). Matching is case-insensitive on the whole input
//      (norm = ToLower); the synthesized CAIP2 uses the lowercased form.
//
//   5. SOLANA RESOLUTION (Go solana branches).
//      - "solana"/"solana-mainnet"/"mainnet-beta" -> the solana Chain
//        (CAIP2 = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp", is_solana()).
//      - "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp" (CAIP-2, mainnet ref) ->
//        the same solana Chain; NAMESPACE is case-insensitive
//        ("SOLANA:5eykt4…" also resolves — Go
//        TestParseChainSolanaCAIP2NamespaceCaseInsensitive).
//      - A non-mainnet solana CAIP-2 reference that matches the base58 mint
//        pattern -> Err(Unsupported "solana non-mainnet references are not
//        supported; only solana mainnet is supported"). The reference is
//        CASE-SENSITIVE: lowercasing the mainnet ref makes it non-mainnet ->
//        Unsupported (Go TestParseChainSolanaReferenceCaseSensitive).
//      - A solana CAIP-2 whose reference is NOT a valid mint pattern ->
//        Err(Usage "unsupported chain input: <original input>").
//
//   6. SOLANA DEVNET/TESTNET ALIASES REJECTED (Go early-return branch).
//      "solana-devnet" and "solana-testnet" -> Err(Unsupported "solana
//      devnet/testnet are not supported; only solana mainnet is supported")
//      (ported from Go TestParseChainRejectsSolanaDevnetAndTestnetAliases).
//      This is checked BEFORE alias lookup.
//
//   7. EMPTY / UNSUPPORTED INPUT (Go usage errors).
//      Empty or whitespace-only input -> Err(Usage "chain is required").
//      An input that is none of the above (e.g. "notachain", "cosmoshub-4") ->
//      Err(Usage "unsupported chain input: <original input>"). The error message
//      embeds the ORIGINAL (untrimmed? — Go uses the raw `input` arg) input.
//
//   8. ERROR CODES are the stable contract codes (spec §2.2): required/unknown
//      input -> Code::Usage (2); solana non-mainnet/devnet/testnet ->
//      Code::Unsupported (13). (Ported assertions verify via
//      defi_errors::Code, mirroring Go clierr.As + Code checks.)
//
//   9. LIST CHAINS (Go ListChains + ChainEntry).
//      - Entries are DEDUPED by CAIP-2 (one entry per unique chain).
//      - Entries are SORTED ascending by CAIP-2 string.
//      - Each entry's `aliases` are the slugs that map to that chain EXCLUDING
//        the primary slug, sorted ascending. (Go TestListChains*,
//        TestListChainsAliasesExcludePrimarySlug.)
//      - Ethereum is present (slug "ethereum", CAIP2 "eip155:1") with "mainnet"
//        among its aliases and WITHOUT "ethereum" in its aliases. Solana is
//        present (slug "solana", is_solana()).
//
// Ported Go tests (meaningful, contract-relevant) re-expressed below:
//   TestParseChainVariants, TestParseChainSolanaCAIP2NamespaceCaseInsensitive,
//   TestParseChainSolanaReferenceCaseSensitive,
//   TestParseChainRejectsSolanaDevnetAndTestnetAliases,
//   TestParseChainExpandedCoverage, TestListChainsReturnsDedupedSortedEntries,
//   TestListChainsAliasesExcludePrimarySlug.
// Skipped: TestParseAsset* (those exercise the token registry / asset parsing,
//   owned by tokens.rs and the crate-root parse_asset, not chain resolution).
//   Also skipped: asserting the EXACT count of supported chains (an internal
//   implementation detail — the registry grows over time; the contract is the
//   per-chain resolution + dedupe/sort/alias invariants, not a magic number).
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use defi_errors::Code;

    const SOL_REF: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";
    const SOL_CAIP2: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";

    // ---- Criterion 1: Chain type + namespace predicates ------------------

    #[test]
    fn chain_field_declaration_order_is_name_slug_caip2_evmid() {
        // Construct via the field-named literal in declaration order; this
        // documents (and pins) the field set + order that the contract requires.
        let c = Chain {
            name: "Ethereum".into(),
            slug: "ethereum".into(),
            caip2: "eip155:1".into(),
            evm_chain_id: 1,
        };
        assert_eq!(c.name, "Ethereum");
        assert_eq!(c.slug, "ethereum");
        assert_eq!(c.caip2, "eip155:1");
        assert_eq!(c.evm_chain_id, 1);
    }

    #[test]
    fn namespace_and_evm_solana_predicates() {
        let eth = parse_chain("ethereum").expect("ethereum must parse");
        assert_eq!(eth.namespace(), "eip155");
        assert!(eth.is_evm());
        assert!(!eth.is_solana());

        let sol = parse_chain("solana").expect("solana must parse");
        assert_eq!(sol.namespace(), "solana");
        assert!(sol.is_solana());
        assert!(!sol.is_evm());
    }

    // ---- Criterion 2: alias resolution (case-insensitive) ----------------

    /// (input, expected_evm_chain_id, expected_caip2, expected_slug)
    /// Ported verbatim from Go TestParseChainExpandedCoverage.
    fn alias_coverage_cases() -> Vec<(&'static str, i64, &'static str, &'static str)> {
        vec![
            ("mantle", 5000, "eip155:5000", "mantle"),
            ("ink", 57073, "eip155:57073", "ink"),
            ("scroll", 534352, "eip155:534352", "scroll"),
            ("berachain", 80094, "eip155:80094", "berachain"),
            ("gnosis", 100, "eip155:100", "gnosis"),
            ("op mainnet", 10, "eip155:10", "optimism"),
            ("op-mainnet", 10, "eip155:10", "optimism"),
            ("xdai", 100, "eip155:100", "gnosis"),
            ("monad", 143, "eip155:143", "monad"),
            ("linea", 59144, "eip155:59144", "linea"),
            ("sonic", 146, "eip155:146", "sonic"),
            ("blast", 81457, "eip155:81457", "blast"),
            ("fraxtal", 252, "eip155:252", "fraxtal"),
            ("world chain", 480, "eip155:480", "world-chain"),
            ("world-chain", 480, "eip155:480", "world-chain"),
            ("worldchain", 480, "eip155:480", "world-chain"),
            ("hyperevm", 999, "eip155:999", "hyperevm"),
            ("hyper evm", 999, "eip155:999", "hyperevm"),
            ("hyper-evm", 999, "eip155:999", "hyperevm"),
            ("citrea", 4114, "eip155:4114", "citrea"),
            ("megaeth", 4326, "eip155:4326", "megaeth"),
            ("mega eth", 4326, "eip155:4326", "megaeth"),
            ("mega-eth", 4326, "eip155:4326", "megaeth"),
            ("tempo", 4217, "eip155:4217", "tempo"),
            ("tempo mainnet", 4217, "eip155:4217", "tempo"),
            ("tempo-mainnet", 4217, "eip155:4217", "tempo"),
            ("presto", 4217, "eip155:4217", "tempo"),
            ("tempo testnet", 42431, "eip155:42431", "tempo-moderato"),
            ("tempo-testnet", 42431, "eip155:42431", "tempo-moderato"),
            ("moderato", 42431, "eip155:42431", "tempo-moderato"),
            ("tempo devnet", 31318, "eip155:31318", "tempo-devnet"),
            ("tempo-devnet", 31318, "eip155:31318", "tempo-devnet"),
            ("celo", 42220, "eip155:42220", "celo"),
            ("taiko", 167000, "eip155:167000", "taiko"),
            ("taiko alethia", 167000, "eip155:167000", "taiko"),
            ("taiko-alethia", 167000, "eip155:167000", "taiko"),
            ("taiko hoodi", 167013, "eip155:167013", "taiko-hoodi"),
            ("taiko-hoodi", 167013, "eip155:167013", "taiko-hoodi"),
            ("hoodi", 167013, "eip155:167013", "taiko-hoodi"),
            ("zksync", 324, "eip155:324", "zksync"),
            ("zksync era", 324, "eip155:324", "zksync"),
            ("zksync-era", 324, "eip155:324", "zksync"),
        ]
    }

    #[test]
    fn alias_resolution_matches_go_coverage() {
        for (input, chain_id, caip2, slug) in alias_coverage_cases() {
            let chain =
                parse_chain(input).unwrap_or_else(|e| panic!("parse_chain({input}) failed: {e}"));
            assert_eq!(chain.evm_chain_id, chain_id, "{input}: wrong evm_chain_id");
            assert_eq!(chain.caip2, caip2, "{input}: wrong caip2");
            assert_eq!(chain.slug, slug, "{input}: wrong slug");
        }
    }

    #[test]
    fn alias_resolution_is_case_insensitive() {
        for input in ["BASE", "Base", "bAsE", "  base  "] {
            let chain =
                parse_chain(input).unwrap_or_else(|e| panic!("parse_chain({input}) failed: {e}"));
            assert_eq!(chain.caip2, "eip155:8453", "{input}: case-insensitive");
            assert_eq!(chain.slug, "base");
        }
    }

    #[test]
    fn ethereum_and_mainnet_aliases_resolve_identically() {
        // Ports Go TestParseChainVariants("base") + the mainnet alias intent.
        let eth = parse_chain("ethereum").expect("ethereum must parse");
        let mainnet = parse_chain("mainnet").expect("mainnet must parse");
        assert_eq!(eth.caip2, "eip155:1");
        assert_eq!(eth.slug, "ethereum");
        assert_eq!(eth.evm_chain_id, 1);
        // Both aliases resolve to the SAME canonical chain.
        assert_eq!(eth, mainnet);
    }

    // ---- Criterion 3: numeric chain id -----------------------------------

    #[test]
    fn numeric_known_chain_id_resolves_to_canonical_chain() {
        // Ports Go TestParseChainVariants("8453") + the numeric leg of
        // TestParseChainExpandedCoverage.
        let cases: &[(&str, i64, &str, &str)] = &[
            ("8453", 8453, "eip155:8453", "base"),
            ("5000", 5000, "eip155:5000", "mantle"),
            ("324", 324, "eip155:324", "zksync"),
            ("80094", 80094, "eip155:80094", "berachain"),
            ("81457", 81457, "eip155:81457", "blast"),
            ("252", 252, "eip155:252", "fraxtal"),
            ("480", 480, "eip155:480", "world-chain"),
            ("999", 999, "eip155:999", "hyperevm"),
            ("4114", 4114, "eip155:4114", "citrea"),
            ("4326", 4326, "eip155:4326", "megaeth"),
            ("143", 143, "eip155:143", "monad"),
            ("167000", 167000, "eip155:167000", "taiko"),
            ("167013", 167013, "eip155:167013", "taiko-hoodi"),
        ];
        for (input, chain_id, caip2, slug) in cases {
            let chain =
                parse_chain(input).unwrap_or_else(|e| panic!("parse_chain({input}) failed: {e}"));
            assert_eq!(chain.evm_chain_id, *chain_id, "{input}");
            assert_eq!(chain.caip2, *caip2, "{input}");
            assert_eq!(chain.slug, *slug, "{input}");
        }
    }

    #[test]
    fn numeric_unknown_chain_id_synthesizes_evm_chain() {
        let chain = parse_chain("999999").expect("unknown numeric id must not error");
        assert_eq!(chain.name, "EVM-999999");
        assert_eq!(chain.slug, "evm-999999");
        assert_eq!(chain.caip2, "eip155:999999");
        assert_eq!(chain.evm_chain_id, 999999);
        assert!(chain.is_evm());
    }

    // ---- Criterion 4: eip155:N passthrough -------------------------------

    #[test]
    fn eip155_known_resolves_to_canonical_chain() {
        let chain = parse_chain("eip155:8453").expect("eip155:8453 must parse");
        assert_eq!(chain.slug, "base");
        assert_eq!(chain.evm_chain_id, 8453);
        assert_eq!(chain.caip2, "eip155:8453");
    }

    #[test]
    fn eip155_unknown_synthesizes_evm_chain() {
        // Ports Go TestParseChainVariants("eip155:999999").
        let chain = parse_chain("eip155:999999").expect("eip155:999999 must parse");
        assert_eq!(chain.evm_chain_id, 999999);
        assert_eq!(chain.name, "EVM-999999");
        assert_eq!(chain.slug, "evm-999999");
        assert_eq!(chain.caip2, "eip155:999999");
        assert!(chain.is_evm());
    }

    #[test]
    fn eip155_passthrough_is_case_insensitive() {
        // norm = ToLower(raw): "EIP155:999999" must synthesize the lowercased
        // CAIP2 just like the lowercase form.
        let chain = parse_chain("EIP155:999999").expect("uppercase eip155 must parse");
        assert_eq!(chain.caip2, "eip155:999999");
        assert_eq!(chain.evm_chain_id, 999999);
    }

    // ---- Criterion 5: solana resolution ----------------------------------

    #[test]
    fn solana_aliases_resolve_to_solana_chain() {
        for input in ["solana", "solana-mainnet", "mainnet-beta"] {
            let chain =
                parse_chain(input).unwrap_or_else(|e| panic!("parse_chain({input}) failed: {e}"));
            assert_eq!(chain.slug, "solana", "{input}");
            assert_eq!(chain.caip2, SOL_CAIP2, "{input}");
            assert!(chain.is_solana(), "{input}");
        }
    }

    #[test]
    fn solana_caip2_mainnet_reference_resolves() {
        // Ports Go TestParseChainVariants(solana CAIP-2).
        let chain = parse_chain(SOL_CAIP2).expect("solana CAIP-2 must parse");
        assert_eq!(chain.caip2, SOL_CAIP2);
        assert!(chain.is_solana());
    }

    #[test]
    fn solana_caip2_namespace_is_case_insensitive() {
        // Ports Go TestParseChainSolanaCAIP2NamespaceCaseInsensitive.
        let chain =
            parse_chain(&format!("SOLANA:{SOL_REF}")).expect("uppercase solana ns must parse");
        assert_eq!(chain.caip2, SOL_CAIP2);
        assert!(chain.is_solana());
    }

    #[test]
    fn solana_caip2_reference_is_case_sensitive_nonmainnet_unsupported() {
        // Ports Go TestParseChainSolanaReferenceCaseSensitive: lowercasing the
        // mainnet reference yields a non-mainnet (but still base58-mint-shaped)
        // reference -> Unsupported.
        let lower_ref = SOL_REF.to_lowercase();
        let err = parse_chain(&format!("solana:{lower_ref}"))
            .expect_err("lowercased solana ref must be unsupported");
        assert_eq!(err.code, Code::Unsupported);
        assert_eq!(
            err.message,
            "solana non-mainnet references are not supported; only solana mainnet is supported"
        );
    }

    #[test]
    fn solana_caip2_invalid_reference_is_usage_error() {
        // A solana CAIP-2 whose reference is NOT a valid base58 mint pattern
        // (too short) -> Usage "unsupported chain input: <input>".
        let input = "solana:short";
        let err = parse_chain(input).expect_err("invalid solana ref must error");
        assert_eq!(err.code, Code::Usage);
        assert_eq!(err.message, format!("unsupported chain input: {input}"));
    }

    // ---- Criterion 6: solana devnet/testnet aliases rejected -------------

    #[test]
    fn solana_devnet_testnet_are_unsupported_with_message() {
        // Ports Go TestParseChainRejectsSolanaDevnetAndTestnetAliases.
        for input in ["solana-devnet", "solana-testnet"] {
            let err = parse_chain(input).expect_err(&format!("{input} must be unsupported"));
            assert_eq!(err.code, Code::Unsupported, "{input}");
            assert_eq!(
                err.message,
                "solana devnet/testnet are not supported; only solana mainnet is supported",
                "{input}"
            );
        }
    }

    // ---- Criterion 7: empty / unsupported input --------------------------

    #[test]
    fn empty_input_is_usage_chain_required() {
        for input in ["", "   ", "\t"] {
            let err = parse_chain(input).expect_err("empty input must error");
            assert_eq!(err.code, Code::Usage, "{input:?}");
            assert_eq!(err.message, "chain is required", "{input:?}");
        }
    }

    #[test]
    fn unsupported_input_is_usage_with_original_input() {
        for input in ["notachain", "cosmoshub-4"] {
            let err = parse_chain(input).expect_err("unknown input must error");
            assert_eq!(err.code, Code::Usage, "{input}");
            assert_eq!(
                err.message,
                format!("unsupported chain input: {input}"),
                "{input}"
            );
        }
    }

    // ---- Criterion 9: list_chains ----------------------------------------

    #[test]
    fn list_chains_is_nonempty_deduped_and_sorted_by_caip2() {
        // Ports Go TestListChainsReturnsDedupedSortedEntries.
        let entries = list_chains();
        assert!(!entries.is_empty(), "expected at least one chain entry");

        // Deduped by CAIP-2.
        let mut seen = std::collections::HashSet::new();
        for e in &entries {
            assert!(
                seen.insert(e.chain.caip2.clone()),
                "duplicate CAIP-2: {}",
                e.chain.caip2
            );
        }

        // Sorted ascending by CAIP-2.
        for w in entries.windows(2) {
            assert!(
                w[0].chain.caip2 <= w[1].chain.caip2,
                "entries not sorted: {} before {}",
                w[0].chain.caip2,
                w[1].chain.caip2
            );
        }
    }

    #[test]
    fn list_chains_aliases_exclude_primary_slug_and_are_sorted() {
        // Ports Go TestListChainsAliasesExcludePrimarySlug + the alias sort.
        let entries = list_chains();
        for e in &entries {
            for alias in &e.aliases {
                assert_ne!(
                    *alias, e.chain.slug,
                    "chain {} has its primary slug in aliases",
                    e.chain.slug
                );
            }
            // aliases sorted ascending.
            for w in e.aliases.windows(2) {
                assert!(w[0] <= w[1], "aliases not sorted for {}", e.chain.slug);
            }
        }
    }

    #[test]
    fn list_chains_includes_ethereum_with_mainnet_alias() {
        // Ports the Ethereum assertions of Go TestListChainsReturnsDedupedSortedEntries.
        let entries = list_chains();
        let eth = entries
            .iter()
            .find(|e| e.chain.slug == "ethereum")
            .expect("ethereum must be in chain list");
        assert_eq!(eth.chain.caip2, "eip155:1");
        assert!(
            eth.aliases.iter().any(|a| a == "mainnet"),
            "expected 'mainnet' among ethereum aliases"
        );
        assert!(
            !eth.aliases.iter().any(|a| a == "ethereum"),
            "primary slug must not appear in aliases"
        );
    }

    #[test]
    fn list_chains_includes_solana() {
        let entries = list_chains();
        let sol = entries
            .iter()
            .find(|e| e.chain.slug == "solana")
            .expect("solana must be in chain list");
        assert!(sol.chain.is_solana());
    }
}
