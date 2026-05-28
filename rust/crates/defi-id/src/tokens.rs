//! Bootstrap token symbol/address registry + asset resolution.
//!
//! Go source: `internal/id/id.go` — the *token registry* surface
//! (`Token`, `tokenRegistry`, `findTokensBySymbol`, `findTokenByAddress`,
//! `KnownToken`, `LookupByAddress`) plus the asset-resolution orchestration
//! `ParseAsset` (re-exposed at the crate root as [`crate::parse_asset`]).
//!
//! This module owns the *bootstrap token registry* (spec §2.4: "Symbol parsing
//! uses the local bootstrap token registry; unresolved symbols fall through to
//! symbol filters / require address or CAIP-19") and the routing of a raw asset
//! input through CAIP-19 / address / symbol resolution.
//!
//! Layering: it composes on top of `caip.rs` (CAIP-19 parse/validate, address
//! canonicalization, canonical asset id) and `chain.rs` (the resolved `Chain`
//! and its `is_evm()`/`is_solana()` predicates). It does NOT re-implement CAIP
//! string parsing or chain alias resolution — those are owned by their own
//! modules and tested there. `caip.rs` tests cover the PURE CAIP-19 sub-parse
//! (`parse_caip19`) in isolation; THIS module tests the registry lookups and the
//! end-to-end `parse_asset` routing that ties registry + CAIP-19 + address +
//! symbol together (the contract a caller actually observes).

// =============================================================================
// SUCCESS CRITERIA (RED phase — tests written before implementation)
//
// This module (Go source: internal/id, token-registry + ParseAsset surface)
// owns the asset-identity contract (spec §2.4: CAIP ids, bootstrap token
// registry, "require address or CAIP-19"). The Rust port is "correct" iff:
//
//   1. TOKEN TYPE + FIELD ORDER (Go `id.Token`).
//      `Token { symbol, address, decimals }` — field DECLARATION order mirrors
//      Go (`Symbol, Address, Decimals`) so any future serde projection keeps
//      contract field order. `decimals` is an integer count (e.g. 6 for USDC,
//      18 for WETH, 8 for WBTC).
//
//   2. ASSET TYPE + FIELD ORDER (Go `id.Asset`).
//      `Asset { chain_id, asset_id, address, symbol, decimals }` — field
//      DECLARATION order mirrors Go (`ChainID, AssetID, Address, Symbol,
//      Decimals`). `chain_id` is the resolved chain's canonical CAIP-2;
//      `asset_id` is the canonical CAIP-19 (`<chain>/<ns>:<addr>`); `address` is
//      canonicalized (lowercased for eip155); `symbol` is UPPERCASED for
//      symbol-resolved assets and is whatever the registry has (possibly empty)
//      for address/CAIP-19-resolved assets.
//
//   3. SYMBOL LOOKUP (Go `findTokensBySymbol`, case-insensitive).
//      `find_tokens_by_symbol(chain_id, symbol)` returns every registry token on
//      that chain whose symbol equals `symbol` case-INSENSITIVELY. Each returned
//      Token has its symbol UPPERCASED and its address CANONICALIZED (lowercased
//      for eip155). No match -> empty Vec. An unknown chain id -> empty Vec
//      (no registry entry). (Backs Go ParseAsset symbol branch.)
//
//   4. ADDRESS LOOKUP (Go `findTokenByAddress` / `LookupByAddress`).
//      `find_token_by_address(chain_id, address)` canonicalizes the input
//      address (lowercased for eip155, case-preserved for solana) and returns
//      the registry token whose canonical address matches, else None. The
//      returned Token has its symbol UPPERCASED and canonical address. Address
//      matching is therefore case-insensitive for eip155 (checksum vs lowercase
//      both match) and case-sensitive for solana mints.
//
//   5. KNOWN_TOKEN (Go `KnownToken`).
//      `known_token(chain_id, symbol)` returns Some(token) IFF exactly one
//      registry token matches the symbol (case-insensitive); zero or MORE THAN
//      ONE match -> None (ambiguity is not resolved here).
//
//   6. LOOKUP_BY_ADDRESS (Go `LookupByAddress`).
//      `lookup_by_address(chain_id, address)` == `find_token_by_address` after
//      canonicalization (it is the public alias). Some(token) on hit, None on
//      miss.
//
//   7. parse_asset — EMPTY INPUT (Go ParseAsset guard).
//      Empty or whitespace-only input -> Err(Usage "asset is required").
//
//   8. parse_asset — CAIP-19 BRANCH (Go ParseAsset, the `/`+inner-`:` branch).
//      When the input is CAIP-19 (splits on FIRST "/" into two parts and the 2nd
//      part contains ":"), routing delegates to the CAIP-19 parse/validate
//      (caip.rs) and then enriches with the registry:
//        - SUCCESS: Asset.chain_id = chain.caip2; Asset.address = canonical
//          address; Asset.asset_id = canonical CAIP-19; Asset.symbol/decimals
//          come from the registry (find_token_by_address) when the address is
//          known, else symbol="" and decimals=0 (Go uses the zero Token when the
//          address is not in the registry — symbol/decimals NOT uppercased/typed
//          beyond the registry result). EVM mixed-case input canonicalizes to
//          lowercase (asset_id "eip155:1/erc20:0xa0b8…eb48").
//        - HyperEVM CAIP-19 "eip155:999/erc20:0x5555…5555" on chain hyperevm ->
//          symbol "WHYPE" (Go TestParseAssetHyperEVMAddressAndCAIP19).
//        - Solana CAIP-19 (token: mint) resolves the registry mint -> e.g. SOL
//          for So111…112 (Go TestParseAssetSolanaSymbolAndMint asset3/asset4).
//        - chain mismatch / invalid format / unsupported namespace propagate the
//          caip.rs errors unchanged (Usage / Unsupported). (Those exact error
//          paths are unit-tested in caip.rs; here we assert the ROUTING surfaces
//          them via parse_asset.)
//
//   9. parse_asset — EVM RAW ADDRESS BRANCH (Go ParseAsset, evmAddressPattern).
//      On an EVM chain, a bare `^0x[0-9a-fA-F]{40}$` input is canonicalized
//      (lowercased) and resolved via the registry:
//        - known address -> Asset with registry symbol/decimals (e.g.
//          0xA0B8…EB48 on ethereum -> symbol "USDC", decimals 6; checksum casing
//          accepted). asset_id = "eip155:1/erc20:0xa0b8…eb48".
//        - unknown-but-well-formed address -> Asset with symbol "" / decimals 0
//          but a VALID canonical asset_id (Go returns the zero Token's fields).
//
//  10. parse_asset — SOLANA RAW MINT BRANCH (Go ParseAsset,
//      solanaTokenMintPattern). On a solana chain, a bare base58 mint
//      (^[1-9A-HJ-NP-Za-km-z]{32,44}$) is resolved via the registry preserving
//      case: EPjFW…Dt1v on solana -> symbol "USDC" (Go
//      TestParseAssetSolanaSymbolAndMint asset2). asset_id uses the `token:` ns.
//
//  11. parse_asset — SYMBOL BRANCH (Go ParseAsset, the registry fall-through).
//      Anything that is not CAIP-19 / raw address / raw mint is treated as a
//      SYMBOL filtered through the registry (case-insensitive):
//        - exactly one match -> Asset with symbol UPPERCASED, canonical address,
//          registry decimals, canonical asset_id, chain_id = chain.caip2.
//          "USDC" on ethereum -> decimals 6, non-empty asset_id (Go
//          TestParseAssetSymbolAndAddress); "USDC" on solana -> asset_id
//          "solana:5eykt4Us…/token:EPjFW…Dt1v" (Go
//          TestParseAssetSolanaSymbolAndMint asset1).
//        - ZERO matches -> Err(Usage "symbol <input> not found in registry for
//          chain <caip2>"). The error embeds the ORIGINAL input (Go uses `input`)
//          and the chain CAIP-2. This is the load-bearing "slash-without-CAIP"
//          case too: "USDC/ETH" is NOT CAIP-19 (2nd part has no ":") so it falls
//          through to a symbol lookup and yields "symbol USDC/ETH not found …"
//          (Go TestParseAssetSlashWithoutCAIPNamespaceIsSymbolLookup).
//        - MORE THAN ONE match -> Err(Usage "symbol <input> is ambiguous on
//          chain <caip2>, use address or CAIP-19 (<addr1>, <addr2>, …)") where
//          the addresses are SORTED ascending and comma-space joined (Go sorts
//          via sort.Strings). (The bootstrap registry currently has no ambiguous
//          symbol, so this is asserted via the lookup helpers / construction
//          rather than a live registry collision — see test note.)
//
//  12. REGISTRY COVERAGE (Go tokenRegistry, ported expectations).
//      The bootstrap registry resolves the specific (chain, symbol)->address and
//      (chain, symbol)->decimals expectations ported from the Go tests:
//        - Per-chain USDC/USDT presence on the expanded chain set
//          (TestParseAssetExpandedChainRegistry).
//        - Top-token coverage on tier-1 chains (TestParseAssetExpandedTop20AndTaikoSymbols).
//        - fraxtal FRAX -> 0xfc000…0001, decimals 18 (TestParseAssetFraxtalFraxAddress).
//        - megaETH MEGA/USDT/WETH addresses, lowercased (TestParseAssetMegaETHBootstrapAddresses).
//        - tempo / tempo-testnet / tempo-devnet bootstrap addresses
//          (TestParseAssetTempoBootstrapAddresses).
//        - hyperevm / monad / citrea native+wrapped+USDC addresses
//          (TestParseAssetFibrousChainBootstrapAddresses).
//        - A symbol absent on a chain (blast USDC) -> not-found error
//          (TestParseAssetRequiresAddressWhenSymbolMissingOnChain).
//      These pin the *contract data* (the bootstrap map) the CLI ships with;
//      they are a subset chosen for contract relevance (stablecoins, natives,
//      the chains explicitly enumerated in Go tests), not an exhaustive copy of
//      every registry row (that would calcify the data, which is expected to
//      grow). The decimals/address values asserted ARE part of the machine
//      contract (amounts depend on decimals; ids depend on address).
//
//  13. ERROR CODES are the stable contract codes (spec §2.2): asset required /
//      symbol not found / ambiguous symbol -> Code::Usage (2). CAIP-19
//      chain-mismatch / invalid-format -> Code::Usage (2); unsupported chain
//      namespace -> Code::Unsupported (13) (propagated from caip.rs).
//
// Ported Go tests (meaningful, contract-relevant) re-expressed below:
//   TestParseAssetSymbolAndAddress, TestParseAssetSolanaSymbolAndMint,
//   TestParseAssetSlashWithoutCAIPNamespaceIsSymbolLookup,
//   TestParseAssetHyperEVMAddressAndCAIP19,
//   TestParseAssetExpandedChainRegistry,
//   TestParseAssetExpandedTop20AndTaikoSymbols,
//   TestParseAssetFraxtalFraxAddress,
//   TestParseAssetRequiresAddressWhenSymbolMissingOnChain,
//   TestParseAssetMegaETHBootstrapAddresses,
//   TestParseAssetTempoBootstrapAddresses,
//   TestParseAssetFibrousChainBootstrapAddresses.
// Re-homed (CAIP-19 SUB-PARSE asserted in caip.rs; here only the ROUTING):
//   TestParseAssetCAIP19MixedCaseEVM, TestParseAssetChainMismatch,
//   TestParseAssetSolanaChainMismatch — the pure parse/validate is owned by
//   caip.rs; this module asserts parse_asset routes inputs to it and enriches
//   from the registry.
// Skipped: asserting the EXACT number of registry rows / chains (internal data
//   detail — the registry grows; the contract is the resolution behavior + the
//   specific tier-1 values the Go tests pin, not a magic count).
// =============================================================================

use crate::caip::{canonical_asset_id, canonicalize_address, parse_caip19};
use crate::chain::Chain;
use crate::Asset;
use defi_errors::{Code, Error};

/// A registry token entry (Go `id.Token`).
///
/// Field declaration order mirrors Go (`Symbol, Address, Decimals`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Token {
    pub symbol: String,
    pub address: String,
    pub decimals: i32,
}

/// EVM address pattern: `0x` followed by exactly 40 hex digits (Go
/// `evmAddressPattern`).
fn is_evm_address(s: &str) -> bool {
    let Some(hex) = s.strip_prefix("0x") else {
        return false;
    };
    hex.len() == 40 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Solana base58 token-mint pattern: 32–44 base58 characters (Go
/// `solanaTokenMintPattern`).
fn is_solana_mint(s: &str) -> bool {
    const BASE58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let len = s.len();
    (32..=44).contains(&len) && s.bytes().all(|b| BASE58.contains(&b))
}

/// A raw registry row: `(symbol, address, decimals)`.
type Row = (&'static str, &'static str, i32);

/// The bootstrap token registry (Go `tokenRegistry`), keyed by CAIP-2 chain id.
///
/// This is the deterministic, offline token data the CLI ships with for tier-1
/// chains. It is intentionally a subset (stablecoins, natives, top tokens), not
/// an exhaustive token list.
fn registry_rows(chain_id: &str) -> &'static [Row] {
    match chain_id {
        "eip155:1" => &[
            ("AAVE", "0x7fc66500c84a76ad7e9c93437bfc5ac33e2ddae9", 18),
            ("BNB", "0xb8c77482e45f1f44de1745f52c74426c631bdd52", 18),
            ("CAKE", "0x152649ea73beab28c5b49b26eb48f7ead6d4c898", 18),
            ("CBBTC", "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf", 8),
            ("CRV", "0xd533a949740bb3306d119cc777fa900ba034cd52", 18),
            ("CRVUSD", "0xf939e0a03fb07f59a73314e73794be0e57ac1b4e", 18),
            ("DAI", "0x6b175474e89094c44da98b954eedeac495271d0f", 18),
            ("ENA", "0x57e114b691db790c35207b2e685d4a43181e6061", 18),
            ("ETHFI", "0xfe0c30065b384f05761f15d0cc899d4f9f9cc0eb", 18),
            ("EURC", "0x1abaea1f7c830bd89acc67ec4af516284b1bc33c", 6),
            ("FRAX", "0x853d955acef822db058eb8505911ed77f175b99e", 18),
            ("GHO", "0x40d16fc0246ad3160ccc09b8d0d3a2cd28ae6c2f", 18),
            ("LDO", "0x5a98fcbea516cf06857215779fd812ca3bef1b32", 18),
            ("LINK", "0x514910771af9ca656af840dff83e8264ecf986ca", 18),
            ("MORPHO", "0x58d97b57bb95320f9a05dc918aef65434969c2b2", 18),
            ("PAXG", "0x45804880de22913dafe09f4980848ece6ecbaf78", 18),
            ("PENDLE", "0x808507121b80c02388fad14726482e061b8da827", 18),
            ("PEPE", "0x6982508145454ce325ddbe47a25d4ec3d2311933", 18),
            ("SHIB", "0x95ad61b0a150d79219dcf64e1e6cc01f0b64c4ce", 18),
            ("TAIKO", "0x10dea67478c5f8c5e2d90e5e9b26dbe60c54d800", 18),
            ("TUSD", "0x0000000000085d4780b73119b644ae5ecd22b376", 18),
            ("UNI", "0x1f9840a85d5af5bf1d1762f925bdaddc4201f984", 18),
            ("USDC", "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", 6),
            ("USDE", "0x4c9edd5852cd905f086c759e8383e09bff1e68b3", 18),
            ("USDS", "0xdc035d45d973e3ec169d2276ddab16f1e407384f", 18),
            ("USDT", "0xdac17f958d2ee523a2206206994597c13d831ec7", 6),
            ("USD1", "0x8d0d000ee44948fc98c9b98a4fa4921476f08b0d", 18),
            ("WBTC", "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599", 8),
            ("WETH", "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2", 18),
            ("WLFI", "0xda5e1988097297dcdc1f90d4dfe7909e847cbef6", 18),
            ("XAUT", "0x68749665ff8d2d112fa859aa293f07a622782f38", 6),
            ("ZRO", "0x6985884c4392d348587b19cb9eaaf157f13271cd", 18),
        ],
        "eip155:10" => &[
            ("AAVE", "0x76fb31fb4af56892a25e32cfc43de717950c9278", 18),
            ("CRV", "0x0994206dfe8de6ec6920ff4d779b0d950605fb53", 18),
            ("CRVUSD", "0xc52d7f23a2e460248db6ee192cb23dd12bddcbf6", 18),
            ("DAI", "0xda10009cbd5d07dd0cecc66161fc93d7c9000da1", 18),
            ("ENA", "0x58538e6a46e07434d7e7375bc268d3cb839c0133", 18),
            ("FRAX", "0x2e3d870790dc77a83dd1d18184acc7439a53f475", 18),
            ("LDO", "0xfdb794692724153d1488ccdbe0c56c252596735f", 18),
            ("LINK", "0x350a791bfc2c21f9ed5d10980dad2e2638ffa7f6", 18),
            ("OP", "0x4200000000000000000000000000000000000042", 18),
            ("PENDLE", "0xbc7b1ff1c6989f006a1185318ed4e7b5796e66e1", 18),
            ("TUSD", "0xcb59a0a753fdb7491d5f3d794316f1ade197b21e", 18),
            ("UNI", "0x6fd9d7ad17242c41f7131d257212c54a0e816691", 18),
            ("USDC", "0x0b2c639c533813f4aa9d7837caf62653d097ff85", 6),
            ("USDC.e", "0x7f5c764cbc14f9669b88837ca1490cca17c31607", 6),
            ("USDE", "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", 18),
            ("USDT", "0x94b008aa00579c1307b0ef2c499ad98a8ce58e58", 6),
            ("USDT0", "0x01bff41798a0bcf287b996046ca68b395dbc1071", 6),
            ("WBTC", "0x68f180fcce6836688e9084f035309e29bf0a2095", 8),
            ("WETH", "0x4200000000000000000000000000000000000006", 18),
            ("ZRO", "0x6985884c4392d348587b19cb9eaaf157f13271cd", 18),
        ],
        "eip155:56" => &[
            ("AAVE", "0xfb6115445bff7b52feb98650c87f44907e58f802", 18),
            ("BTCB", "0x7130d2a12b9bcbfae4f2634d864a1ee1ce3ead9c", 18),
            ("CAKE", "0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82", 18),
            ("CRVUSD", "0xe2fb3f127f5450dee44afe054385d74c392bdef4", 18),
            ("DAI", "0x1af3f329e8be154074d8769d1ffa4ee058b1dbc3", 18),
            ("ENA", "0x58538e6a46e07434d7e7375bc268d3cb839c0133", 18),
            ("FRAX", "0x90c97f71e18723b0cf0dfa30ee176ab653e89f40", 18),
            ("LINK", "0xf8a0bf9cf54bb92f17374d9e9a321e6a111a51bd", 18),
            ("PENDLE", "0xb3ed0a426155b79b898849803e3b36552f7ed507", 18),
            ("PEPE", "0x25d887ce7a35172c62febfd67a1856f20faebb00", 18),
            ("TUSD", "0x40af3827f39d0eacbf4a168f8d4ee67c121d11c9", 18),
            ("UNI", "0xbf5140a22578168fd562dccf235e5d43a02ce9b1", 18),
            ("USDC", "0x8ac76a51cc950d9822d68b83fe1ad97b32cd580d", 18),
            ("USDE", "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", 18),
            ("USDT", "0x55d398326f99059ff775485246999027b3197955", 18),
            ("USD1", "0x8d0d000ee44948fc98c9b98a4fa4921476f08b0d", 18),
            ("WBNB", "0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c", 18),
            ("WBTC", "0x0555e30da8f98308edb960aa94c0db47230d2b9c", 8),
            ("WETH", "0x2170ed0880ac9a755fd29b2688956bd959f933f8", 18),
            ("ZRO", "0x6985884c4392d348587b19cb9eaaf157f13271cd", 18),
        ],
        "eip155:100" => &[
            ("AAVE", "0xdf613af6b44a31299e48131e9347f034347e2f00", 18),
            ("CRV", "0x712b3d230f3c1c19db860d80619288b1f0bdd0bd", 18),
            ("CRVUSD", "0xabef652195f98a91e490f047a5006b71c85f058d", 18),
            ("FRAX", "0xca5d82e40081f220d59f7ed9e2e1428deaf55355", 18),
            ("GHO", "0xfc421ad3c883bf9e7c4f42de845c4e4405799e73", 18),
            ("LDO", "0x96e334926454cd4b7b4efb8a8fcb650a738ad244", 18),
            ("LINK", "0xe2e73a1c69ecf83f464efce6a5be353a37ca09b2", 18),
            ("TUSD", "0xb714654e905edad1ca1940b7790a8239ece5a9ff", 18),
            ("UNI", "0x4537e328bf7e4efa29d05caea260d7fe26af9d74", 18),
            ("USDC", "0xddafbb505ad214d7b80b1f830fccc89b60fb7a83", 6),
            ("USDT", "0x4ecaba5870353805a9f068101a40e0f32ed605c6", 6),
            ("WETH", "0x6a023ccd1ff6f2045c3309768ead9e68f978f6e1", 18),
        ],
        "eip155:137" => &[
            ("AAVE", "0xd6df932a45c0f255f85145f286ea0b292b21c90b", 18),
            ("CRV", "0x172370d5cd63279efa6d502dab29171933a610af", 18),
            ("CRVUSD", "0xc4ce1d6f5d98d65ee25cf85e9f2e9dcfee6cb5d6", 18),
            ("DAI", "0x8f3cf7ad23cd3cadbd9735aff958023239c6a063", 18),
            ("FRAX", "0x45c32fa6df82ead1e2ef74d17b76547eddfaff89", 18),
            ("LDO", "0xc3c7d422809852031b44ab29eec9f1eff2a58756", 18),
            ("LINK", "0x53e0bca35ec356bd5dddfebbd1fc0fd03fabad39", 18),
            ("TUSD", "0x2e1ad108ff1d8c782fcbbb89aad783ac49586756", 18),
            ("UNI", "0xb33eaad8d922b1083446dc23f610c2567fb5180f", 18),
            ("USDC", "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359", 6),
            ("USDT", "0xc2132d05d31c914a87c6611c10748aeb04b58e8f", 6),
            ("WETH", "0x7ceb23fd6bc0add59e62ac25578270cff1b9f619", 18),
            ("ZRO", "0x6985884c4392d348587b19cb9eaaf157f13271cd", 18),
        ],
        "eip155:146" => &[
            ("CRVUSD", "0x7fff4c4a827c84e32c5e175052834111b2ccd270", 18),
            ("LINK", "0x71052bae71c25c78e37fd12e5ff1101a71d9018f", 18),
            ("PENDLE", "0xf1ef7d2d4c0c881cd634481e0586ed5d2871a74b", 18),
            ("USDC", "0x29219dd400f2bf60e5a23d13be72b486d4038894", 6),
            ("USDT", "0x6047828dc181963ba44974801ff68e538da5eaf9", 6),
            ("WETH", "0x50c42deacd8fc9773493ed674b675be577f2634b", 18),
        ],
        "eip155:252" => &[
            ("CRV", "0x331b9182088e2a7d6d3fe4742aba1fb231aecc56", 18),
            ("CRVUSD", "0xb102f7efa0d5de071a8d37b3548e1c7cb148caf3", 18),
            ("ENA", "0x58538e6a46e07434d7e7375bc268d3cb839c0133", 18),
            ("FRAX", "0xfc00000000000000000000000000000000000001", 18),
            ("LINK", "0xd6a6ba37faac229b9665e86739ca501401f5a940", 18),
            ("USDC", "0xdcc0f2d8f90fde85b10ac1c8ab57dc0ae946a543", 6),
            ("USDE", "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", 18),
            ("USDT", "0x4d15ea9c2573addaed814e48c148b5262694646a", 6),
        ],
        "eip155:324" => &[
            ("CAKE", "0x3a287a06c66f9e95a56327185ca2bdf5f031cecd", 18),
            ("CRVUSD", "0x43cd37cc4b9ec54833c8ac362dd55e58bfd62b86", 18),
            ("ENA", "0x686b311f82b407f0be842652a98e5619f64cc25f", 18),
            ("FRAX", "0xb4c1544cb4163f4c2eca1ae9ce999f63892d912a", 18),
            ("LINK", "0x52869bae3e091e36b0915941577f2d47d8d8b534", 18),
            ("USDC", "0x1d17cbcf0d6d143135ae902365d2e5e2a16538d4", 6),
            ("USDE", "0x39fe7a0dacce31bd90418e3e659fb0b5f0b3db0d", 18),
            ("USDT", "0x493257fd37edb34451f62edf8d2a0c418852ba4c", 6),
            ("WETH", "0x5aea5775959fbc2557cc8789bc1bf90a239d9a91", 18),
        ],
        "eip155:4217" => &[
            ("pathUSD", "0x20c0000000000000000000000000000000000000", 6),
            ("USDC.e", "0x20c000000000000000000000b9537d11c60e8b50", 6),
            ("EURC.e", "0x20c0000000000000000000001621e21f71cf12fb", 6),
            ("USDT0", "0x20c00000000000000000000014f22ca97301eb73", 6),
            ("frxUSD", "0x20c0000000000000000000003554d28269e0f3c2", 6),
            ("cUSD", "0x20c0000000000000000000000520792dcccccccc", 6),
            ("stcUSD", "0x20c00000000000000000000031f228af88888888", 6),
        ],
        "eip155:480" => &[
            ("EURC", "0x1c60ba0a0ed1019e8eb035e6daf4155a5ce2380b", 6),
            ("LINK", "0x915b648e994d5f31059b38223b9fbe98ae185473", 18),
            ("USDC", "0x79a02482a880bce3f13e09da970dc34db4cd24d1", 6),
        ],
        "eip155:5000" => &[
            ("ENA", "0x58538e6a46e07434d7e7375bc268d3cb839c0133", 18),
            ("GHO", "0xfc421ad3c883bf9e7c4f42de845c4e4405799e73", 18),
            ("LINK", "0xfe36cf0b43aae49fbc5cfc5c0af22a623114e043", 18),
            ("USDC", "0x09bc4e0d864854c6afb6eb9a9cdf58ac190d0df9", 6),
            ("USDE", "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", 18),
            ("USDT", "0x201eba5cc46d216ce6dc03f6a759e8e766e956ae", 6),
            ("WETH", "0xdeaddeaddeaddeaddeaddeaddeaddeaddead1111", 18),
        ],
        "eip155:8453" => &[
            ("AAVE", "0x63706e401c06ac8513145b7687a14804d17f814b", 18),
            ("CAKE", "0x3055913c90fcc1a6ce9a358911721eeb942013a1", 18),
            ("CBBTC", "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf", 8),
            ("CRV", "0x8ee73c484a26e0a5df2ee2a4960b789967dd0415", 18),
            ("CRVUSD", "0x417ac0e078398c154edfadd9ef675d30be60af93", 18),
            ("DAI", "0x50c5725949a6f0c72e6c4a641f24049a917db0cb", 18),
            ("ENA", "0x58538e6a46e07434d7e7375bc268d3cb839c0133", 18),
            ("ETHFI", "0x6c240dda6b5c336df09a4d011139beaaa1ea2aa2", 18),
            ("EURC", "0x60a3e35cc302bfa44cb288bc5a4f316fdb1adb42", 6),
            ("FRAX", "0x909dbde1ebe906af95660033e478d59efe831fed", 18),
            ("GHO", "0x6bb7a212910682dcfdbd5bcbb3e28fb4e8da10ee", 18),
            ("LINK", "0x88fb150bdc53a65fe94dea0c9ba0a6daf8c6e196", 18),
            ("MORPHO", "0xbaa5cc21fd487b8fcc2f632f3f4e8d37262a0842", 18),
            ("PENDLE", "0xa99f6e6785da0f5d6fb42495fe424bce029eeb3e", 18),
            ("SNX", "0x22e6966b799c4d5b13be962e1d117b56327fda66", 18),
            ("UNI", "0xc3de830ea07524a0761646a6a4e4be0e114a3c83", 18),
            ("USDC", "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913", 6),
            ("USDE", "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", 18),
            ("USDS", "0x820c137fa70c8691f0e44dc420a5e53c168921dc", 18),
            ("USDT", "0xfde4c96c8593536e31f229ea8f37b2ada2699bb2", 6),
            ("WBTC", "0x1cea84203673764244e05693e42e6ace62be9ba5", 8),
            ("WETH", "0x4200000000000000000000000000000000000006", 18),
            ("ZRO", "0x6985884c4392d348587b19cb9eaaf157f13271cd", 18),
        ],
        "eip155:42161" => &[
            ("AAVE", "0xba5ddd1f9d7f570dc94a51479a000e3bce967196", 18),
            ("ARB", "0x912ce59144191c1204e64559fe8253a0e49e6548", 18),
            ("CAKE", "0x1b896893dfc86bb67cf57767298b9073d2c1ba2c", 18),
            ("CBBTC", "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf", 8),
            ("CRV", "0x11cdb42b0eb46d95f990bedd4695a6e3fa034978", 18),
            ("CRVUSD", "0x498bf2b1e120fed3ad3d42ea2165e9b73f99c1e5", 18),
            ("DAI", "0xda10009cbd5d07dd0cecc66161fc93d7c9000da1", 18),
            ("ENA", "0x58538e6a46e07434d7e7375bc268d3cb839c0133", 18),
            ("ETHFI", "0x7189fb5b6504bbff6a852b13b7b82a3c118fdc27", 18),
            ("FRAX", "0x17fc002b466eec40dae837fc4be5c67993ddbd6f", 18),
            ("GHO", "0x7dff72693f6a4149b17e7c6314655f6a9f7c8b33", 18),
            ("LDO", "0x13ad51ed4f1b7e9dc168d8a00cb3f4ddd85efa60", 18),
            ("LINK", "0xf97f4df75117a78c1a5a0dbb814af92458539fb4", 18),
            ("MORPHO", "0x40bd670a58238e6e230c430bbb5ce6ec0d40df48", 18),
            ("PENDLE", "0x0c880f6761f1af8d9aa9c466984b80dab9a8c9e8", 18),
            ("PEPE", "0x25d887ce7a35172c62febfd67a1856f20faebb00", 18),
            ("PYUSD", "0x46850ad61c2b7d64d08c9c754f45254596696984", 6),
            ("TUSD", "0x4d15a3a2286d883af0aa1b3f21367843fac63e07", 18),
            ("UNI", "0xfa7f8980b0f1e64a2062791cc3b0871572f1f7f0", 18),
            ("USDC", "0xaf88d065e77c8cc2239327c5edb3a432268e5831", 6),
            ("USDE", "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", 18),
            ("USDS", "0x6491c05a82219b8d1479057361ff1654749b876b", 18),
            ("USDT", "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9", 6),
            ("WBTC", "0x2f2a2543b76a4166549f7aab2e75bef0aefc5b0f", 8),
            ("WETH", "0x82af49447d8a07e3bd95bd0d56f35241523fbab1", 18),
            ("ZRO", "0x6985884c4392d348587b19cb9eaaf157f13271cd", 18),
        ],
        "eip155:4326" => &[
            ("MEGA", "0x28B7E77f82B25B95953825F1E3eA0E36c1c29861", 18),
            ("USDT", "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb", 6),
            ("WETH", "0x4200000000000000000000000000000000000006", 18),
        ],
        "eip155:42431" => &[
            ("pathUSD", "0x20c0000000000000000000000000000000000000", 6),
            ("alphaUSD", "0x20c0000000000000000000000000000000000001", 6),
            ("betaUSD", "0x20c0000000000000000000000000000000000002", 6),
            ("thetaUSD", "0x20c0000000000000000000000000000000000003", 6),
            ("USDC.e", "0x20c0000000000000000000009e8d7eb59b783726", 6),
            ("EURC.e", "0x20c000000000000000000000d72572838bbee59c", 6),
        ],
        "eip155:42220" => &[
            ("LINK", "0xd07294e6e917e07dfdcee882dd1e2565085c2ae0", 18),
            ("USDC", "0xceba9300f2b948710d2653dd7b07f33a8b32118c", 6),
            ("USDT", "0x48065fbbe25f71c9282ddf5e1cd6d6a887483d5e", 6),
            ("WETH", "0xd221812de1bd094f35587ee8e174b07b6167d9af", 18),
        ],
        "eip155:43114" => &[
            ("AAVE", "0x63a72806098bd3d9520cc43356dd78afe5d386d9", 18),
            ("DAI", "0xd586e7f844cea2f87f50152665bcbc2c279d8d70", 18),
            ("ENA", "0x58538e6a46e07434d7e7375bc268d3cb839c0133", 18),
            ("EURC", "0xc891eb4cbdeff6e073e859e987815ed1505c2acd", 6),
            ("FRAX", "0xd24c2ad096400b6fbcd2ad8b24e7acbc21a1da64", 18),
            ("GHO", "0xfc421ad3c883bf9e7c4f42de845c4e4405799e73", 18),
            ("LINK", "0x5947bb275c521040051d82396192181b413227a3", 18),
            ("PENDLE", "0xfb98b335551a418cd0737375a2ea0ded62ea213b", 18),
            ("PEPE", "0xa659d083b677d6bffe1cb704e1473b896727be6d", 18),
            ("TUSD", "0x1c20e891bab6b1727d14da358fae2984ed9b59eb", 18),
            ("UNI", "0x8ebaf22b6f053dffeaf46f4dd9efa95d89ba8580", 18),
            ("USDC", "0xb97ef9ef8734c71904d8002f8b6bc66dd9c48a6e", 6),
            ("USDE", "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", 18),
            ("USDT", "0x9702230a8ea53601f5cd2dc00fdbc13d4df4a8c7", 6),
            ("WAVAX", "0xb31f66aa3c1e785363f0875a1b74e27b85fd66c7", 18),
            ("WBTC", "0x0555e30da8f98308edb960aa94c0db47230d2b9c", 8),
            ("WETH", "0x49d5c2bdffac6ce2bfdb6640f4f80f226bc10bab", 18),
            ("ZRO", "0x6985884c4392d348587b19cb9eaaf157f13271cd", 18),
        ],
        "eip155:57073" => &[
            ("GHO", "0xfc421ad3c883bf9e7c4f42de845c4e4405799e73", 18),
            ("LINK", "0x71052bae71c25c78e37fd12e5ff1101a71d9018f", 18),
            ("USDC", "0x2d270e6886d130d724215a266106e6832161eaed", 6),
            ("USDE", "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", 18),
            ("WETH", "0x4200000000000000000000000000000000000006", 18),
        ],
        "eip155:59144" => &[
            ("CAKE", "0x0d1e753a25ebda689453309112904807625befbe", 18),
            ("ENA", "0x58538e6a46e07434d7e7375bc268d3cb839c0133", 18),
            ("LINK", "0xa18152629128738a5c081eb226335fed4b9c95e9", 18),
            ("USDC", "0x176211869ca2b568f2a7d4ee941e073a821ee1ff", 6),
            ("USDE", "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", 18),
            ("USDT", "0xa219439258ca9da29e9cc4ce5596924745e12b93", 6),
            ("WETH", "0xe5d7c2a44ffddf6b295a15c148167daaaf5cf34f", 18),
        ],
        "eip155:80094" => &[
            ("LINK", "0x71052bae71c25c78e37fd12e5ff1101a71d9018f", 18),
            ("PENDLE", "0xff9c599d51c407a45d631c6e89cb047efb88aef6", 18),
            ("USDE", "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", 18),
        ],
        "eip155:81457" => &[
            ("ENA", "0x58538e6a46e07434d7e7375bc268d3cb839c0133", 18),
            ("FRAX", "0x909dbde1ebe906af95660033e478d59efe831fed", 18),
            ("LINK", "0x93202ec683288a9ea75bb829c6bacfb2bfea9013", 18),
            ("USDE", "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", 18),
        ],
        "eip155:167000" => &[
            ("CRVUSD", "0xc8f4518ed4bab9a972808a493107926ce8237068", 18),
            ("LINK", "0x917a3964c37993e99a47c779beb5db1e9d13804d", 18),
            ("TAIKO", "0xa9d23408b9ba935c230493c40c73824df71a0975", 18),
            ("USDC", "0x07d83526730c7438048d55a4fc0b850e2aab6f0b", 6),
            ("USDT", "0x2def195713cf4a606b49d07e520e22c17899a736", 6),
            ("WETH", "0xa51894664a773981c6c112c43ce576f315d5b1b6", 18),
        ],
        "eip155:167013" => &[
            ("USDC", "0x18d5bb147f3d05d5f6c5e60caf1daeedbf5155b6", 6),
            ("USDT", "0xeb4e8eb83d6ffba2ce0d8f62ace60648d1ece116", 6),
            ("WETH", "0x3b39685b5495359c892ddd1057b5712f49976835", 18),
        ],
        "eip155:31318" => &[
            ("pathUSD", "0x20c0000000000000000000000000000000000000", 6),
            ("alphaUSD", "0x20c0000000000000000000000000000000000001", 6),
            ("betaUSD", "0x20c0000000000000000000000000000000000002", 6),
            ("thetaUSD", "0x20c0000000000000000000000000000000000003", 6),
        ],
        "eip155:534352" => &[
            ("CAKE", "0x1b896893dfc86bb67cf57767298b9073d2c1ba2c", 18),
            ("ENA", "0x58538e6a46e07434d7e7375bc268d3cb839c0133", 18),
            ("ETHFI", "0x056a5fa5da84ceb7f93d36e545c5905607d8bd81", 18),
            ("LINK", "0x548c6944cba02b9d1c0570102c89de64d258d3ac", 18),
            ("USDC", "0x06efdbff2a14a7c8e15944d1f4a48f9f95f663a4", 6),
            ("USDE", "0x5d3a1ff2b6bab83b63cd9ad0787074081a52ef34", 18),
            ("USDT", "0xf55bec9cafdbe8730f096aa55dad6d22d44099df", 6),
            ("WETH", "0x5300000000000000000000000000000000000004", 18),
        ],
        "eip155:999" => &[
            ("HYPE", "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE", 18),
            ("WHYPE", "0x5555555555555555555555555555555555555555", 18),
            ("USDC", "0xb88339cb7199b77e23db6e890353e22632ba630f", 6),
        ],
        "eip155:143" => &[
            ("MON", "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE", 18),
            ("WMON", "0x3bd359C1119dA7Da1D913D1C4D2B7c461115433A", 18),
            ("USDC", "0x754704Bc059F8C67012fEd69BC8A327a5aafb603", 6),
        ],
        "eip155:4114" => &[
            ("CBTC", "0x0000000000000000000000000000000000000000", 18),
            ("WCBTC", "0x3100000000000000000000000000000000000006", 18),
            ("USDC", "0xE045e6c36cF77FAA2CfB54466D71A3aEF7bBE839", 6),
        ],
        "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp" => &[
            ("USDC", "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", 6),
            ("USDT", "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", 6),
            ("SOL", "So11111111111111111111111111111111111111112", 9),
            ("JUP", "JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN", 6),
            ("JTO", "jtojtomepa8beP8AuQc6eXt5FriJwfFMwGQx2v2f9mCL", 9),
        ],
        _ => &[],
    }
}

/// Find every registry token on a chain whose symbol matches case-insensitively
/// (Go `findTokensBySymbol`).
///
/// Returned tokens have their symbol UPPERCASED and address CANONICALIZED.
pub fn find_tokens_by_symbol(chain_id: &str, symbol: &str) -> Vec<Token> {
    registry_rows(chain_id)
        .iter()
        .filter(|(sym, _, _)| sym.eq_ignore_ascii_case(symbol))
        .map(|(sym, addr, decimals)| Token {
            symbol: sym.to_uppercase(),
            address: canonicalize_address(chain_id, addr),
            decimals: *decimals,
        })
        .collect()
}

/// Find the registry token whose canonical address matches the input
/// (Go `findTokenByAddress`).
///
/// The input address is canonicalized (lowercased for eip155, case-preserved
/// for solana) before matching; the returned token has its symbol UPPERCASED.
pub fn find_token_by_address(chain_id: &str, address: &str) -> Option<Token> {
    let target = canonicalize_address(chain_id, address);
    registry_rows(chain_id)
        .iter()
        .find_map(|(sym, addr, decimals)| {
            let candidate = canonicalize_address(chain_id, addr);
            if candidate == target {
                Some(Token {
                    symbol: sym.to_uppercase(),
                    address: candidate,
                    decimals: *decimals,
                })
            } else {
                None
            }
        })
}

/// Resolve a symbol to a token IFF exactly one registry token matches
/// (Go `KnownToken`). Zero or more than one match -> None.
pub fn known_token(chain_id: &str, symbol: &str) -> Option<Token> {
    let matches = find_tokens_by_symbol(chain_id, symbol);
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

/// Public alias for [`find_token_by_address`] (Go `LookupByAddress`).
pub fn lookup_by_address(chain_id: &str, address: &str) -> Option<Token> {
    find_token_by_address(chain_id, address)
}

/// Build the ambiguous-symbol usage error for a chain (Go `ParseAsset`,
/// `len(matches) > 1` branch).
///
/// The matching tokens' addresses are SORTED ascending and comma-space joined,
/// mirroring Go's `sort.Strings` + `strings.Join(addresses, ", ")`. The message
/// embeds the ORIGINAL input and the chain CAIP-2. Pulled out as a named helper
/// so the exact contract message/shape is unit-testable without depending on a
/// live registry symbol collision (the bootstrap registry currently has none).
fn ambiguous_symbol_error(input: &str, chain_caip2: &str, matches: &[Token]) -> Error {
    let mut addresses: Vec<String> = matches.iter().map(|m| m.address.clone()).collect();
    addresses.sort();
    Error::new(
        Code::Usage,
        format!(
            "symbol {input} is ambiguous on chain {chain_caip2}, use address or CAIP-19 ({})",
            addresses.join(", ")
        ),
    )
}

/// Build a resolved [`Asset`] from a canonical address on a chain, enriching
/// symbol/decimals from the registry (zero token when the address is unknown).
fn asset_from_address(chain: &Chain, canonical_addr: &str) -> Asset {
    let token = find_token_by_address(&chain.caip2, canonical_addr).unwrap_or_default();
    Asset {
        chain_id: chain.caip2.clone(),
        asset_id: canonical_asset_id(&chain.caip2, canonical_addr),
        address: canonical_addr.to_string(),
        symbol: token.symbol,
        decimals: token.decimals,
    }
}

/// Resolve a raw asset input to a canonical [`Asset`] on a chain
/// (Go `ParseAsset`).
///
/// Routes the input through CAIP-19 / raw-address / raw-mint / symbol
/// resolution, enriching from the bootstrap token registry. Empty input, an
/// unknown symbol, an ambiguous symbol, a chain mismatch, or an invalid CAIP-19
/// format are usage errors; an unsupported chain namespace is unsupported.
pub fn parse_asset(input: &str, chain: &Chain) -> Result<Asset, Error> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(Error::new(Code::Usage, "asset is required"));
    }

    // CAIP-19 branch: the pure parse/validate lives in caip.rs; here we route
    // and enrich from the registry.
    if let Some(parts) = parse_caip19(raw, &chain.caip2)? {
        return Ok(asset_from_address(chain, &parts.address));
    }

    // EVM raw address.
    if chain.is_evm() && is_evm_address(raw) {
        let addr = canonicalize_address(&chain.caip2, raw);
        return Ok(asset_from_address(chain, &addr));
    }

    // Solana raw mint.
    if chain.is_solana() && is_solana_mint(raw) {
        let addr = canonicalize_address(&chain.caip2, raw);
        return Ok(asset_from_address(chain, &addr));
    }

    // Symbol fall-through.
    let matches = find_tokens_by_symbol(&chain.caip2, raw);
    if matches.is_empty() {
        return Err(Error::new(
            Code::Usage,
            format!(
                "symbol {input} not found in registry for chain {}",
                chain.caip2
            ),
        ));
    }
    if matches.len() > 1 {
        return Err(ambiguous_symbol_error(input, &chain.caip2, &matches));
    }

    let t = &matches[0];
    let addr = canonicalize_address(&chain.caip2, &t.address);
    Ok(Asset {
        chain_id: chain.caip2.clone(),
        asset_id: canonical_asset_id(&chain.caip2, &addr),
        address: addr,
        symbol: t.symbol.to_uppercase(),
        decimals: t.decimals,
    })
}

#[cfg(test)]
mod tests {
    use crate::tokens::{
        find_token_by_address, find_tokens_by_symbol, known_token, lookup_by_address,
    };
    use crate::{parse_asset, parse_chain, Asset, Token};
    use defi_errors::Code;

    const SOL_CAIP2: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";
    const USDC_ETH_CHECKSUM: &str = "0xA0B86991C6218B36C1D19D4A2E9EB0CE3606EB48";
    const USDC_ETH_LOWER: &str = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
    const USDC_SOL_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const SOL_MINT: &str = "So11111111111111111111111111111111111111112";

    // ---- helpers ---------------------------------------------------------

    fn assert_usage(result: Result<Asset, defi_errors::Error>, msg: &str, ctx: &str) {
        let err = result.expect_err(&format!("{ctx}: expected Err, got Ok"));
        assert_eq!(err.code, Code::Usage, "{ctx}: wrong code");
        assert_eq!(err.message, msg, "{ctx}: wrong message");
    }

    // ---- Criterion 1: Token type + field declaration order ----------------

    #[test]
    fn token_field_declaration_order_is_symbol_address_decimals() {
        let t = Token {
            symbol: "USDC".into(),
            address: USDC_ETH_LOWER.into(),
            decimals: 6,
        };
        assert_eq!(t.symbol, "USDC");
        assert_eq!(t.address, USDC_ETH_LOWER);
        assert_eq!(t.decimals, 6);
    }

    // ---- Criterion 2: Asset type + field declaration order ----------------

    #[test]
    fn asset_field_declaration_order_is_chain_asset_address_symbol_decimals() {
        let a = Asset {
            chain_id: "eip155:1".into(),
            asset_id: format!("eip155:1/erc20:{USDC_ETH_LOWER}"),
            address: USDC_ETH_LOWER.into(),
            symbol: "USDC".into(),
            decimals: 6,
        };
        assert_eq!(a.chain_id, "eip155:1");
        assert_eq!(a.asset_id, format!("eip155:1/erc20:{USDC_ETH_LOWER}"));
        assert_eq!(a.address, USDC_ETH_LOWER);
        assert_eq!(a.symbol, "USDC");
        assert_eq!(a.decimals, 6);
    }

    // ---- Criterion 3: symbol lookup (case-insensitive) --------------------

    #[test]
    fn find_tokens_by_symbol_is_case_insensitive_and_uppercases_and_canonicalizes() {
        for sym in ["USDC", "usdc", "Usdc"] {
            let matches = find_tokens_by_symbol("eip155:1", sym);
            assert_eq!(matches.len(), 1, "{sym}: exactly one USDC on ethereum");
            let t = &matches[0];
            assert_eq!(t.symbol, "USDC", "{sym}: symbol uppercased");
            assert_eq!(t.address, USDC_ETH_LOWER, "{sym}: address canonicalized");
            assert_eq!(t.decimals, 6, "{sym}: decimals");
        }
    }

    #[test]
    fn find_tokens_by_symbol_unknown_symbol_is_empty() {
        assert!(find_tokens_by_symbol("eip155:1", "NOTATOKEN").is_empty());
    }

    #[test]
    fn find_tokens_by_symbol_unknown_chain_is_empty() {
        // No registry entry for this synthetic chain id.
        assert!(find_tokens_by_symbol("eip155:999999", "USDC").is_empty());
    }

    // ---- Criterion 4: address lookup (eip155 case-insensitive) ------------

    #[test]
    fn find_token_by_address_matches_checksum_or_lowercase_for_eip155() {
        for addr in [USDC_ETH_CHECKSUM, USDC_ETH_LOWER] {
            let t = find_token_by_address("eip155:1", addr)
                .unwrap_or_else(|| panic!("{addr}: USDC must be found"));
            assert_eq!(t.symbol, "USDC", "{addr}");
            assert_eq!(t.address, USDC_ETH_LOWER, "{addr}: canonical address");
            assert_eq!(t.decimals, 6, "{addr}");
        }
    }

    #[test]
    fn find_token_by_address_unknown_is_none() {
        assert!(
            find_token_by_address("eip155:1", "0x0000000000000000000000000000000000000000")
                .is_none()
        );
    }

    #[test]
    fn find_token_by_address_solana_is_case_sensitive() {
        // The exact mint resolves; a lowercased mint does NOT (solana base58 is
        // case-sensitive).
        let t = find_token_by_address(SOL_CAIP2, USDC_SOL_MINT)
            .expect("solana USDC mint must be found");
        assert_eq!(t.symbol, "USDC");
        assert_eq!(t.address, USDC_SOL_MINT, "solana address preserves case");
        assert!(
            find_token_by_address(SOL_CAIP2, &USDC_SOL_MINT.to_lowercase()).is_none(),
            "lowercased solana mint must not match"
        );
    }

    // ---- Criterion 5: known_token (exactly-one-match semantics) -----------

    #[test]
    fn known_token_returns_some_for_unique_symbol() {
        let t = known_token("eip155:1", "weth").expect("WETH must resolve uniquely");
        assert_eq!(t.symbol, "WETH");
        assert_eq!(t.decimals, 18);
    }

    #[test]
    fn known_token_returns_none_for_missing_symbol() {
        assert!(known_token("eip155:1", "NOTATOKEN").is_none());
    }

    // ---- Criterion 6: lookup_by_address (public alias) --------------------

    #[test]
    fn lookup_by_address_equals_find_token_by_address() {
        let a = lookup_by_address("eip155:1", USDC_ETH_CHECKSUM);
        let b = find_token_by_address("eip155:1", USDC_ETH_CHECKSUM);
        assert_eq!(a, b);
        assert_eq!(a.expect("USDC").symbol, "USDC");
    }

    // ---- Criterion 7: parse_asset empty input -----------------------------

    #[test]
    fn parse_asset_empty_input_is_usage_asset_required() {
        let eth = parse_chain("ethereum").expect("ethereum must parse");
        for input in ["", "   ", "\t"] {
            assert_usage(
                parse_asset(input, &eth),
                "asset is required",
                &format!("{input:?}"),
            );
        }
    }

    // ---- Criterion 8: parse_asset CAIP-19 routing -------------------------

    #[test]
    fn parse_asset_evm_caip19_mixed_case_canonicalizes_and_enriches() {
        // Ports the routing of Go TestParseAssetCAIP19MixedCaseEVM +
        // TestParseAssetSymbolAndAddress(address leg).
        let eth = parse_chain("ethereum").expect("ethereum must parse");
        let asset = parse_asset(&format!("EIP155:1/ERC20:{USDC_ETH_CHECKSUM}"), &eth)
            .expect("valid CAIP-19 must resolve");
        assert_eq!(asset.chain_id, "eip155:1");
        assert_eq!(asset.address, USDC_ETH_LOWER);
        assert_eq!(asset.asset_id, format!("eip155:1/erc20:{USDC_ETH_LOWER}"));
        // Enriched from the registry (known address).
        assert_eq!(asset.symbol, "USDC");
        assert_eq!(asset.decimals, 6);
    }

    #[test]
    fn parse_asset_hyperevm_caip19_resolves_registry_symbol() {
        // Ports Go TestParseAssetHyperEVMAddressAndCAIP19 (CAIP-19 leg).
        let chain = parse_chain("hyperevm").expect("hyperevm must parse");
        let asset = parse_asset(
            "eip155:999/erc20:0x5555555555555555555555555555555555555555",
            &chain,
        )
        .expect("hyperevm CAIP-19 must resolve");
        assert_eq!(asset.symbol, "WHYPE");
        assert_eq!(asset.chain_id, "eip155:999");
    }

    #[test]
    fn parse_asset_solana_caip19_resolves_mint_symbol() {
        // Ports Go TestParseAssetSolanaSymbolAndMint asset3/asset4.
        let sol = parse_chain("solana").expect("solana must parse");
        for input in [
            format!("{SOL_CAIP2}/token:{SOL_MINT}"),
            format!("SOLANA:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp/TOKEN:{SOL_MINT}"),
        ] {
            let asset =
                parse_asset(&input, &sol).unwrap_or_else(|_| panic!("{input} must resolve"));
            assert_eq!(asset.symbol, "SOL", "{input}");
            assert_eq!(asset.address, SOL_MINT, "{input}: mint case preserved");
        }
    }

    #[test]
    fn parse_asset_caip19_unknown_address_yields_empty_symbol_but_valid_id() {
        // A well-formed EVM CAIP-19 whose address is NOT in the registry resolves
        // to a valid canonical asset_id with empty symbol / decimals 0 (Go zero
        // Token).
        let eth = parse_chain("ethereum").expect("ethereum must parse");
        let unknown = "0x1111111111111111111111111111111111111111";
        let asset = parse_asset(&format!("eip155:1/erc20:{unknown}"), &eth)
            .expect("unknown but well-formed CAIP-19 must resolve");
        assert_eq!(asset.chain_id, "eip155:1");
        assert_eq!(asset.address, unknown);
        assert_eq!(asset.asset_id, format!("eip155:1/erc20:{unknown}"));
        assert_eq!(asset.symbol, "", "unknown address -> empty symbol");
        assert_eq!(asset.decimals, 0, "unknown address -> decimals 0");
    }

    #[test]
    fn parse_asset_caip19_chain_mismatch_propagates_usage_error() {
        // Ports the routing of Go TestParseAssetChainMismatch: the pure error is
        // unit-tested in caip.rs; here we assert parse_asset surfaces it.
        let base = parse_chain("base").expect("base must parse");
        assert_usage(
            parse_asset(&format!("eip155:1/erc20:{USDC_ETH_LOWER}"), &base),
            "asset chain does not match --chain",
            "evm chain mismatch",
        );
    }

    #[test]
    fn parse_asset_solana_caip19_chain_mismatch_is_error() {
        // Ports Go TestParseAssetSolanaChainMismatch (routing).
        let sol = parse_chain("solana").expect("solana must parse");
        let err = parse_asset(
            &format!("solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1/token:{USDC_SOL_MINT}"),
            &sol,
        )
        .expect_err("solana chain mismatch must error");
        assert_eq!(err.code, Code::Usage);
    }

    // ---- Criterion 9: parse_asset EVM raw address -------------------------

    #[test]
    fn parse_asset_evm_raw_address_resolves_registry_symbol() {
        // Ports Go TestParseAssetSymbolAndAddress (address leg).
        let eth = parse_chain("ethereum").expect("ethereum must parse");
        let asset = parse_asset(USDC_ETH_CHECKSUM, &eth).expect("address must resolve");
        assert_eq!(asset.symbol, "USDC");
        assert_eq!(asset.address, USDC_ETH_LOWER);
        assert_eq!(asset.decimals, 6);
        assert_eq!(asset.asset_id, format!("eip155:1/erc20:{USDC_ETH_LOWER}"));
    }

    #[test]
    fn parse_asset_hyperevm_raw_address_resolves_registry_symbol() {
        // Ports Go TestParseAssetHyperEVMAddressAndCAIP19 (raw address leg).
        let chain = parse_chain("hyperevm").expect("hyperevm must parse");
        let asset = parse_asset("0xb88339cb7199b77e23db6e890353e22632ba630f", &chain)
            .expect("hyperevm USDC address must resolve");
        assert_eq!(asset.symbol, "USDC");
    }

    #[test]
    fn parse_asset_evm_raw_address_unknown_yields_empty_symbol_valid_id() {
        let eth = parse_chain("ethereum").expect("ethereum must parse");
        let unknown = "0x2222222222222222222222222222222222222222";
        let asset = parse_asset(unknown, &eth).expect("well-formed unknown address must resolve");
        assert_eq!(asset.address, unknown);
        assert_eq!(asset.symbol, "");
        assert_eq!(asset.decimals, 0);
        assert_eq!(asset.asset_id, format!("eip155:1/erc20:{unknown}"));
    }

    // ---- Criterion 10: parse_asset solana raw mint ------------------------

    #[test]
    fn parse_asset_solana_raw_mint_resolves_registry_symbol() {
        // Ports Go TestParseAssetSolanaSymbolAndMint asset2.
        let sol = parse_chain("solana").expect("solana must parse");
        let asset = parse_asset(USDC_SOL_MINT, &sol).expect("solana mint must resolve");
        assert_eq!(asset.symbol, "USDC");
        assert_eq!(asset.address, USDC_SOL_MINT, "mint case preserved");
    }

    // ---- Criterion 11: parse_asset symbol branch --------------------------

    #[test]
    fn parse_asset_symbol_resolves_uppercased_with_registry_decimals() {
        // Ports Go TestParseAssetSymbolAndAddress (symbol leg).
        let eth = parse_chain("ethereum").expect("ethereum must parse");
        let asset = parse_asset("usdc", &eth).expect("USDC symbol must resolve");
        assert_eq!(asset.symbol, "USDC", "symbol uppercased");
        assert_eq!(asset.decimals, 6);
        assert!(!asset.asset_id.is_empty(), "asset_id must be populated");
        assert_eq!(asset.chain_id, "eip155:1");
        assert_eq!(asset.address, USDC_ETH_LOWER);
    }

    #[test]
    fn parse_asset_solana_symbol_resolves_canonical_asset_id() {
        // Ports Go TestParseAssetSolanaSymbolAndMint asset1.
        let sol = parse_chain("solana").expect("solana must parse");
        let asset = parse_asset("USDC", &sol).expect("solana USDC symbol must resolve");
        assert_eq!(asset.asset_id, format!("{SOL_CAIP2}/token:{USDC_SOL_MINT}"));
        assert_eq!(asset.symbol, "USDC");
    }

    #[test]
    fn parse_asset_unknown_symbol_is_not_found_error_with_original_input() {
        // The error embeds the ORIGINAL input + the chain CAIP-2.
        let eth = parse_chain("ethereum").expect("ethereum must parse");
        assert_usage(
            parse_asset("NOTATOKEN", &eth),
            "symbol NOTATOKEN not found in registry for chain eip155:1",
            "unknown symbol",
        );
    }

    #[test]
    fn parse_asset_slash_without_caip_namespace_falls_through_to_symbol_lookup() {
        // Ports Go TestParseAssetSlashWithoutCAIPNamespaceIsSymbolLookup:
        // "USDC/ETH" has a "/" but the 2nd part has no ":", so it is NOT CAIP-19
        // and must fall through to a symbol lookup -> not-found (NOT a
        // chain-mismatch error). The error embeds the FULL original input.
        let eth = parse_chain("ethereum").expect("ethereum must parse");
        assert_usage(
            parse_asset("USDC/ETH", &eth),
            "symbol USDC/ETH not found in registry for chain eip155:1",
            "slash without caip namespace",
        );
    }

    #[test]
    fn parse_asset_symbol_missing_on_chain_is_not_found_error() {
        // Ports Go TestParseAssetRequiresAddressWhenSymbolMissingOnChain:
        // USDC is not in the blast bootstrap registry.
        let blast = parse_chain("blast").expect("blast must parse");
        let err = parse_asset("USDC", &blast).expect_err("USDC missing on blast must error");
        assert_eq!(err.code, Code::Usage);
        assert!(
            err.message.contains("symbol USDC not found"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn parse_asset_ambiguous_symbol_error_shape() {
        // The bootstrap registry currently has no ambiguous symbol on any single
        // chain, so a live collision cannot be triggered through parse_asset.
        // The CONTRACT for the ambiguous case (Go ParseAsset len(matches)>1) is
        // therefore pinned structurally: when more than one registry token shares
        // a symbol on a chain, find_tokens_by_symbol returns them all (so the
        // caller can detect ambiguity), and known_token returns None (refuses to
        // disambiguate). This guards the helper semantics the ambiguous-symbol
        // error path is built on without depending on a registry collision that
        // may legitimately never exist.
        //
        // Sanity: a uniquely-matched symbol yields exactly one entry, and
        // known_token returns it — the negative-space of the ambiguity branch.
        let eth = parse_chain("ethereum").expect("ethereum must parse");
        let matches = find_tokens_by_symbol("eip155:1", "USDC");
        assert_eq!(
            matches.len(),
            1,
            "USDC is unique on ethereum (not ambiguous)"
        );
        assert!(known_token("eip155:1", "USDC").is_some());
        // And the unambiguous symbol does NOT produce an ambiguity error.
        let asset = parse_asset("USDC", &eth).expect("unique symbol must resolve");
        assert!(
            !asset.asset_id.is_empty(),
            "unique symbol resolves without ambiguity error"
        );
    }

    #[test]
    fn ambiguous_symbol_error_sorts_addresses_and_formats_message() {
        // Directly exercise the ambiguity error CONSTRUCTION (Go ParseAsset
        // len(matches)>1 branch): addresses are SORTED ascending and comma-space
        // joined into the exact contract message. The bootstrap registry has no
        // live collision, so this pins the contract message/shape via the named
        // helper rather than a registry coincidence. Matches are passed in
        // DESCENDING order to prove the helper sorts (a regression that dropped
        // the sort would emit them reversed and fail here).
        use crate::tokens::ambiguous_symbol_error;
        let matches = vec![
            Token {
                symbol: "FOO".into(),
                address: "0xcccccccccccccccccccccccccccccccccccccccc".into(),
                decimals: 18,
            },
            Token {
                symbol: "FOO".into(),
                address: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                decimals: 6,
            },
        ];
        let err = ambiguous_symbol_error("FOO", "eip155:1", &matches);
        assert_eq!(err.code, Code::Usage);
        assert_eq!(
            err.message,
            "symbol FOO is ambiguous on chain eip155:1, use address or CAIP-19 \
             (0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, \
             0xcccccccccccccccccccccccccccccccccccccccc)",
            "addresses must be sorted ascending and comma-space joined"
        );
    }

    // ---- Criterion 12: registry coverage (ported data expectations) -------

    #[test]
    fn registry_expanded_chain_usdc_usdt_coverage() {
        // Ports Go TestParseAssetExpandedChainRegistry.
        let cases: &[(&str, &str)] = &[
            ("mantle", "USDC"),
            ("ink", "USDC"),
            ("scroll", "USDC"),
            ("gnosis", "USDC"),
            ("linea", "USDC"),
            ("sonic", "USDC"),
            ("hyperevm", "USDC"),
            ("monad", "USDC"),
            ("citrea", "USDC"),
            ("megaeth", "USDT"),
            ("tempo", "USDC.E"),
            ("tempo testnet", "USDC.E"),
            ("tempo devnet", "PATHUSD"),
            ("celo", "USDC"),
            ("taiko", "USDC"),
            ("hoodi", "USDC"),
            ("zksync", "USDC"),
        ];
        for (chain_input, symbol) in cases {
            let chain = parse_chain(chain_input)
                .unwrap_or_else(|_| panic!("parse_chain({chain_input}) failed"));
            let asset = parse_asset(symbol, &chain)
                .unwrap_or_else(|_| panic!("parse_asset({symbol}) on {chain_input} failed"));
            assert_eq!(asset.symbol, *symbol, "{chain_input}/{symbol}: symbol");
            assert_eq!(
                asset.chain_id, chain.caip2,
                "{chain_input}/{symbol}: chain id"
            );
        }
    }

    #[test]
    fn registry_top_token_and_taiko_coverage() {
        // Ports Go TestParseAssetExpandedTop20AndTaikoSymbols.
        let cases: &[(&str, &str)] = &[
            ("ethereum", "AAVE"),
            ("ethereum", "WBTC"),
            ("ethereum", "USD1"),
            ("base", "USDE"),
            ("base", "USDS"),
            ("base", "CBBTC"),
            ("base", "SNX"),
            ("arbitrum", "MORPHO"),
            ("arbitrum", "ARB"),
            ("bsc", "CAKE"),
            ("bsc", "WBNB"),
            ("ethereum", "CRVUSD"),
            ("ethereum", "TUSD"),
            ("avalanche", "EURC"),
            ("avalanche", "WAVAX"),
            ("base", "FRAX"),
            ("fraxtal", "FRAX"),
            ("ethereum", "LDO"),
            ("arbitrum", "UNI"),
            ("base", "ZRO"),
            ("scroll", "ETHFI"),
            ("optimism", "OP"),
            ("optimism", "USDT0"),
            ("taiko", "TAIKO"),
        ];
        for (chain_input, symbol) in cases {
            let chain = parse_chain(chain_input)
                .unwrap_or_else(|_| panic!("parse_chain({chain_input}) failed"));
            let asset = parse_asset(symbol, &chain)
                .unwrap_or_else(|_| panic!("parse_asset({symbol}) on {chain_input} failed"));
            assert_eq!(asset.symbol, *symbol, "{chain_input}/{symbol}");
            assert_eq!(asset.chain_id, chain.caip2, "{chain_input}/{symbol}");
        }
    }

    #[test]
    fn registry_fraxtal_frax_address_and_decimals() {
        // Ports Go TestParseAssetFraxtalFraxAddress.
        let chain = parse_chain("fraxtal").expect("fraxtal must parse");
        let asset = parse_asset("FRAX", &chain).expect("FRAX must resolve on fraxtal");
        assert_eq!(asset.address, "0xfc00000000000000000000000000000000000001");
        assert_eq!(asset.decimals, 18);
    }

    #[test]
    fn registry_megaeth_bootstrap_addresses_lowercased() {
        // Ports Go TestParseAssetMegaETHBootstrapAddresses.
        let chain = parse_chain("megaeth").expect("megaeth must parse");
        let cases: &[(&str, &str)] = &[
            ("MEGA", "0x28b7e77f82b25b95953825f1e3ea0e36c1c29861"),
            ("USDT", "0xb8ce59fc3717ada4c02eadf9682a9e934f625ebb"),
            ("WETH", "0x4200000000000000000000000000000000000006"),
        ];
        for (symbol, address) in cases {
            let asset = parse_asset(symbol, &chain)
                .unwrap_or_else(|_| panic!("parse_asset({symbol}) on megaeth failed"));
            assert_eq!(asset.address, *address, "{symbol}");
        }
    }

    #[test]
    fn registry_tempo_bootstrap_addresses() {
        // Ports Go TestParseAssetTempoBootstrapAddresses.
        let cases: &[(&str, &str, &str)] = &[
            (
                "tempo",
                "pathUSD",
                "0x20c0000000000000000000000000000000000000",
            ),
            (
                "tempo",
                "USDC.e",
                "0x20c000000000000000000000b9537d11c60e8b50",
            ),
            (
                "tempo",
                "EURC.e",
                "0x20c0000000000000000000001621e21f71cf12fb",
            ),
            (
                "tempo testnet",
                "alphaUSD",
                "0x20c0000000000000000000000000000000000001",
            ),
            (
                "tempo testnet",
                "USDC.e",
                "0x20c0000000000000000000009e8d7eb59b783726",
            ),
            (
                "tempo devnet",
                "thetaUSD",
                "0x20c0000000000000000000000000000000000003",
            ),
        ];
        for (chain_input, symbol, address) in cases {
            let chain = parse_chain(chain_input)
                .unwrap_or_else(|_| panic!("parse_chain({chain_input}) failed"));
            let asset = parse_asset(symbol, &chain)
                .unwrap_or_else(|_| panic!("parse_asset({symbol}) on {chain_input} failed"));
            assert_eq!(asset.address, *address, "{chain_input}/{symbol}");
        }
    }

    #[test]
    fn registry_fibrous_chain_bootstrap_addresses() {
        // Ports Go TestParseAssetFibrousChainBootstrapAddresses.
        let cases: &[(&str, &str, &str)] = &[
            (
                "hyperevm",
                "WHYPE",
                "0x5555555555555555555555555555555555555555",
            ),
            (
                "hyperevm",
                "HYPE",
                "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            ),
            (
                "monad",
                "WMON",
                "0x3bd359c1119da7da1d913d1c4d2b7c461115433a",
            ),
            (
                "monad",
                "USDC",
                "0x754704bc059f8c67012fed69bc8a327a5aafb603",
            ),
            ("monad", "MON", "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
            (
                "citrea",
                "WCBTC",
                "0x3100000000000000000000000000000000000006",
            ),
            (
                "citrea",
                "CBTC",
                "0x0000000000000000000000000000000000000000",
            ),
        ];
        for (chain_input, symbol, address) in cases {
            let chain = parse_chain(chain_input)
                .unwrap_or_else(|_| panic!("parse_chain({chain_input}) failed"));
            let asset = parse_asset(symbol, &chain)
                .unwrap_or_else(|_| panic!("parse_asset({symbol}) on {chain_input} failed"));
            assert_eq!(asset.address, *address, "{chain_input}/{symbol}");
        }
    }

    // ---- Criterion 13: error codes (covered by assert_usage above) --------
    // Usage for required/not-found/ambiguous/mismatch/invalid; Unsupported for
    // non-evm/non-solana chain namespace is exercised via caip.rs routing.
}
