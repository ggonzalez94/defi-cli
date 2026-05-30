//! Canonical execution endpoints/contracts/ABIs + default chain RPC map.
//!
//! Mirrors `internal/registry`. This crate is the single source of truth for the
//! canonical, *offline* on-chain metadata the execution engine and on-chain read
//! providers depend on: default EVM RPC URLs (used when no `--rpc-url` override is
//! given), Uniswap-V3-compatible quoter/router contracts, Aave V3
//! PoolAddressesProvider and Moonwell Comptroller addresses, Tempo Stablecoin
//! DEX and fee-token addresses, bridge execution-target allowlists plus
//! settlement-URL allowlists (the `--unsafe-provider-tx` guardrails), and the
//! ABI fragments every planner packs calldata against. None of this touches the
//! network, so it is fully deterministic and golden-testable.
//!
//! The data values themselves are part of the machine contract: the calldata
//! targets, the RPC defaults, and the canonical execution-target/settlement
//! allowlists are observable through the JSON output and through the pre-sign
//! guardrails, so they must stay byte-stable across the port. The lookups are
//! re-expressed in idiomatic Rust: Go's `(value, bool)` returns become
//! [`Option`], and `ResolveRPCURL`'s `(string, error)` becomes a `Result` with a
//! `defi_errors`-typed usage error (no `unwrap`/`expect`/`panic` in lib code).
//!
//! # Success criteria (contract this crate must preserve)
//!
//! 1. **Default RPC map parity (`DefaultRPCURL`)** — [`default_rpc_url`]: returns
//!    `Some(url)` for every chain ID in the canonical default map (e.g. Taiko
//!    mainnet `167000`, Base `8453`, Ethereum `1`, Tempo mainnet `4217`) and
//!    `None` for any chain ID not in the map (e.g. `999999`). URLs are non-empty
//!    and exact-match the Go map values.
//!
//! 2. **RPC resolution + precedence (`ResolveRPCURL`)** — [`resolve_rpc_url`]:
//!    a non-blank override wins and is returned **trimmed** (`" https://x "` →
//!    `"https://x"`); a blank/whitespace override falls back to the default map;
//!    a missing default yields a `Code::Usage` error mentioning the chain id /
//!    `--rpc-url`. No panic.
//!
//! 3. **Uniswap V3 contracts (`UniswapV3Contracts`)** — [`uniswap_v3_contracts`]:
//!    returns `Some((quoter_v2, router))` for supported chains (Taiko mainnet
//!    `167000`, Taiko hoodi `167013`) with non-empty values; `None` for
//!    unsupported chains (e.g. `1`).
//!
//! 4. **Aave PoolAddressesProvider (`AavePoolAddressProvider`)** —
//!    [`aave_pool_address_provider`]: `Some(addr)` for the covered set
//!    `{1,10,137,8453,42161,43114}`; `None` otherwise (e.g. `167000`).
//!
//! 5. **Moonwell Comptroller (`MoonwellComptroller`)** —
//!    [`moonwell_comptroller`]: `Some(addr)` for Base `8453` and Optimism `10`;
//!    `None` otherwise.
//!
//! 6. **Tempo addresses (`TempoStablecoinDEX` / `TempoFeeToken`)** —
//!    [`tempo_stablecoin_dex`] / [`tempo_fee_token`]: `Some(addr)` for Tempo chain
//!    IDs `{4217, 42431, 31318}`; `None` for non-Tempo chains (`1`, `8453`).
//!
//! 7. **Bridge settlement URLs (`BridgeSettlementURL`)** —
//!    [`bridge_settlement_url`]: `Some(LIFI_SETTLEMENT_URL)` for `"lifi"`,
//!    `Some(ACROSS_SETTLEMENT_URL)` for `"across"` (case/space-insensitive on the
//!    provider), `None` otherwise.
//!
//! 8. **Settlement-URL allowlist (`IsAllowedBridgeSettlementURL`)** —
//!    [`is_allowed_bridge_settlement_url`]: empty endpoint allowed; canonical
//!    endpoint allowed (incl. explicit default port `:443`); loopback over
//!    http/https allowed (dev); non-https non-loopback rejected; wrong path /
//!    wrong provider / malformed URL rejected.
//!
//! 9. **Bridge execution-target policy (`HasBridgeExecutionTargetPolicy`)** —
//!    [`has_bridge_execution_target_policy`]: `true` for every covered
//!    (provider, chain) pair (LiFi across all its EVM chains; Across over its
//!    supported chains); `false` for uncovered chains / unknown providers.
//!
//! 10. **Bridge execution-target allowlist (`IsAllowedBridgeExecutionTarget`)** —
//!     [`is_allowed_bridge_execution_target`]: canonical target allowed
//!     **case-insensitively** on its chain; chain-specific (non-standard) diamond
//!     addresses only allowed on their own chain; unknown/empty/malformed targets,
//!     unrelated-provider targets, and targets on uncovered chains all rejected.
//!
//! 11. **ABI fragments parse (`abis.go` consts)** — every public ABI constant is
//!     valid JSON ABI: it round-trips through `defi_evm::abi` and a known function
//!     is extractable. Selectors/calldata bytes are owned by `defi-evm`; this crate
//!     only owns the fragment *strings* and that they parse.

use defi_errors::{Code, Error};
use defi_evm::address;

// ---------------------------------------------------------------------------
// Execution provider endpoints (parity with internal/registry/endpoints.go)
// ---------------------------------------------------------------------------

/// LiFi quote/execution API base URL.
pub const LIFI_BASE_URL: &str = "https://li.quest/v1";
/// LiFi bridge settlement status endpoint.
pub const LIFI_SETTLEMENT_URL: &str = "https://li.quest/v1/status";
/// Across quote/execution API base URL.
pub const ACROSS_BASE_URL: &str = "https://app.across.to/api";
/// Across bridge settlement status endpoint.
pub const ACROSS_SETTLEMENT_URL: &str = "https://app.across.to/api/deposit/status";
/// Shared Morpho GraphQL endpoint (adapter + execution planner).
pub const MORPHO_GRAPHQL_ENDPOINT: &str = "https://api.morpho.org/graphql";

/// Canonical settlement status URL for a bridge provider, if any.
///
/// Provider matching is case- and whitespace-insensitive. Mirrors
/// `registry.BridgeSettlementURL`.
pub fn bridge_settlement_url(provider: &str) -> Option<&'static str> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "lifi" => Some(LIFI_SETTLEMENT_URL),
        "across" => Some(ACROSS_SETTLEMENT_URL),
        _ => None,
    }
}

/// Whether a settlement-status endpoint is allowed for a bridge provider.
///
/// Empty endpoint is allowed; loopback hosts are allowed over http/https (dev);
/// otherwise the endpoint must be the canonical https URL for the provider
/// (scheme + host + normalized port + normalized path). Mirrors
/// `registry.IsAllowedBridgeSettlementURL`.
pub fn is_allowed_bridge_settlement_url(provider: &str, endpoint: &str) -> bool {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return true;
    }
    let parsed = match ParsedUrl::parse(endpoint) {
        Some(parsed) => parsed,
        None => return false,
    };
    if parsed.hostname().trim().is_empty() {
        return false;
    }
    if is_loopback_host(parsed.hostname()) {
        let scheme = parsed.scheme().trim().to_ascii_lowercase();
        return scheme.is_empty() || scheme == "http" || scheme == "https";
    }
    if !parsed.scheme().trim().eq_ignore_ascii_case("https") {
        return false;
    }
    let allowed_raw = match bridge_settlement_url(provider) {
        Some(raw) => raw,
        None => return false,
    };
    let allowed = match ParsedUrl::parse(allowed_raw) {
        Some(allowed) => allowed,
        None => return false,
    };
    if !parsed.scheme().eq_ignore_ascii_case(allowed.scheme()) {
        return false;
    }
    if !parsed.hostname().eq_ignore_ascii_case(allowed.hostname()) {
        return false;
    }
    if parsed.normalized_port() != allowed.normalized_port() {
        return false;
    }
    normalized_url_path(parsed.path()) == normalized_url_path(allowed.path())
}

/// Whether `host` is a loopback host (`localhost` or a loopback IP), mirroring
/// the Go helper `isLoopbackHost`.
fn is_loopback_host(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    if h == "localhost" {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => false,
    }
}

/// Normalize a URL path the way Go's `normalizedURLPath` does: empty (or
/// reduced-to-empty after trimming a trailing slash) becomes `"/"`, otherwise a
/// single trailing slash is stripped.
fn normalized_url_path(path: &str) -> String {
    let p = path.trim();
    if p.is_empty() {
        return "/".to_string();
    }
    let p = p.strip_suffix('/').unwrap_or(p);
    if p.is_empty() {
        return "/".to_string();
    }
    p.to_string()
}

/// A minimal URL parse capturing exactly the fields the settlement-URL
/// allowlist needs (scheme, host, port, path), with semantics matching Go's
/// `net/url.Parse` for the inputs this guardrail sees.
///
/// Go's `url.Parse` is lenient: an input with no `scheme://authority` (e.g.
/// `"not-a-url"`) parses successfully but yields an empty `Hostname()`, which
/// the caller then rejects. We reproduce that observable behavior: only inputs
/// of the form `scheme://host[:port][/path]` populate a hostname; everything
/// else parses with an empty hostname.
struct ParsedUrl {
    scheme: String,
    host: String,
    port: String,
    path: String,
}

impl ParsedUrl {
    fn parse(raw: &str) -> Option<ParsedUrl> {
        let raw = raw.trim();
        // Split off the scheme (`scheme:`), if present.
        let (scheme, after_scheme) = match raw.find(':') {
            Some(idx) if is_valid_scheme(&raw[..idx]) => (raw[..idx].to_string(), &raw[idx + 1..]),
            _ => (String::new(), raw),
        };

        // Only `//authority` form carries a host (matching Go's net/url, where a
        // missing authority leaves Hostname() empty).
        let (host, port, path) = if let Some(rest) = after_scheme.strip_prefix("//") {
            // authority ends at the first '/', '?', or '#'.
            let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let authority = &rest[..auth_end];
            let path = &rest[auth_end..];

            // Drop any userinfo (`user:pass@host`).
            let host_port = match authority.rfind('@') {
                Some(at) => &authority[at + 1..],
                None => authority,
            };
            let (host, port) = split_host_port(host_port);
            let path = path.split(['?', '#']).next().unwrap_or("").to_string();
            (host, port, path)
        } else {
            // Opaque / rootless: no authority, so no host (Go: Hostname() == "").
            let path = after_scheme
                .split(['?', '#'])
                .next()
                .unwrap_or("")
                .to_string();
            (String::new(), String::new(), path)
        };

        Some(ParsedUrl {
            scheme,
            host,
            port,
            path,
        })
    }

    fn scheme(&self) -> &str {
        &self.scheme
    }

    fn hostname(&self) -> &str {
        &self.host
    }

    fn path(&self) -> &str {
        &self.path
    }

    /// The explicit port, or the default port for the scheme (`http`→`80`,
    /// `https`→`443`), or empty for unknown schemes. Mirrors the Go helper
    /// `normalizedURLPort`.
    fn normalized_port(&self) -> String {
        let port = self.port.trim();
        if !port.is_empty() {
            return port.to_string();
        }
        match self.scheme.trim().to_ascii_lowercase().as_str() {
            "http" => "80".to_string(),
            "https" => "443".to_string(),
            _ => String::new(),
        }
    }
}

/// Whether `s` is a valid URL scheme per RFC 3986: ALPHA *( ALPHA / DIGIT / "+"
/// / "-" / "." ). Guards against treating a bare `host:port` (e.g. the `:8080`
/// in a relative reference) as a scheme.
fn is_valid_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Split a `host[:port]` authority into `(host, port)`. Bracketed IPv6 literals
/// keep their brackets stripped from the host (Go's `Hostname()` behavior).
fn split_host_port(host_port: &str) -> (String, String) {
    if let Some(rest) = host_port.strip_prefix('[') {
        // IPv6 literal: `[::1]` or `[::1]:port`.
        if let Some(close) = rest.find(']') {
            let host = rest[..close].to_string();
            let after = &rest[close + 1..];
            let port = after.strip_prefix(':').unwrap_or("").to_string();
            return (host, port);
        }
        return (host_port.to_string(), String::new());
    }
    match host_port.rfind(':') {
        Some(idx) => (
            host_port[..idx].to_string(),
            host_port[idx + 1..].to_string(),
        ),
        None => (host_port.to_string(), String::new()),
    }
}

// ---------------------------------------------------------------------------
// Default RPC map (parity with internal/registry/rpc.go)
// ---------------------------------------------------------------------------

/// The canonical default EVM RPC URL for a chain ID, used when no `--rpc-url`
/// override is given. Mirrors `registry.DefaultRPCURL`.
pub fn default_rpc_url(chain_id: i64) -> Option<&'static str> {
    let url = match chain_id {
        1 => "https://eth.llamarpc.com",
        10 => "https://mainnet.optimism.io",
        56 => "https://bsc-dataseed.binance.org",
        100 => "https://rpc.gnosischain.com",
        137 => "https://polygon-rpc.com",
        146 => "https://rpc.soniclabs.com",
        252 => "https://rpc.frax.com",
        324 => "https://mainnet.era.zksync.io",
        4217 => "https://rpc.tempo.xyz",
        480 => "https://worldchain-mainnet.g.alchemy.com/public",
        5000 => "https://rpc.mantle.xyz",
        8453 => "https://mainnet.base.org",
        42220 => "https://forno.celo.org",
        42161 => "https://arb1.arbitrum.io/rpc",
        43114 => "https://api.avax.network/ext/bc/C/rpc",
        42431 => "https://rpc.moderato.tempo.xyz",
        57073 => "https://rpc-gel.inkonchain.com",
        59144 => "https://rpc.linea.build",
        80094 => "https://rpc.berachain.com",
        81457 => "https://rpc.blast.io",
        167000 => "https://rpc.mainnet.taiko.xyz",
        167013 => "https://rpc.hoodi.taiko.xyz",
        31318 => "https://rpc.devnet.tempoxyz.dev",
        534352 => "https://rpc.scroll.io",
        _ => return None,
    };
    Some(url)
}

/// Resolve the RPC URL to use: a non-blank `override` wins (trimmed), otherwise
/// the default map, otherwise a `Code::Usage` error. Mirrors
/// `registry.ResolveRPCURL`.
pub fn resolve_rpc_url(override_url: &str, chain_id: i64) -> Result<String, Error> {
    let trimmed = override_url.trim();
    if !trimmed.is_empty() {
        return Ok(trimmed.to_string());
    }
    if let Some(value) = default_rpc_url(chain_id) {
        return Ok(value.to_string());
    }
    Err(Error::new(
        Code::Usage,
        format!("no default rpc configured for chain id {chain_id}; provide --rpc-url"),
    ))
}

// ---------------------------------------------------------------------------
// Contracts (parity with internal/registry/contracts.go)
// ---------------------------------------------------------------------------

/// Uniswap V3-compatible `(QuoterV2, Router)` contracts for a chain, if covered.
/// Mirrors `registry.UniswapV3Contracts`.
pub fn uniswap_v3_contracts(chain_id: i64) -> Option<(&'static str, &'static str)> {
    match chain_id {
        167000 => Some((
            "0xcBa70D57be34aA26557B8E80135a9B7754680aDb",
            "0x1A0c3a0Cfd1791FAC7798FA2b05208B66aaadfeD",
        )),
        167013 => Some((
            "0xAC8D93657DCc5C0dE9d9AF2772aF9eA3A032a1C6",
            "0x482233e4DBD56853530fA1918157CE59B60dF230",
        )),
        _ => None,
    }
}

/// Aave V3 PoolAddressesProvider for a chain, if covered. Mirrors
/// `registry.AavePoolAddressProvider`.
pub fn aave_pool_address_provider(chain_id: i64) -> Option<&'static str> {
    let addr = match chain_id {
        1 => "0x2f39d218133AFaB8F2B819B1066c7E434Ad94E9e",
        10 => "0xa97684ead0e402dC232d5A977953DF7ECBaB3CDb",
        137 => "0xa97684ead0e402dC232d5A977953DF7ECBaB3CDb",
        8453 => "0xe20fCBdBfFC4Dd138cE8b2E6FBb6CB49777ad64D",
        42161 => "0xa97684ead0e402dC232d5A977953DF7ECBaB3CDb",
        43114 => "0xa97684ead0e402dC232d5A977953DF7ECBaB3CDb",
        _ => return None,
    };
    Some(addr)
}

/// Moonwell Comptroller (Unitroller) for a chain, if covered. Mirrors
/// `registry.MoonwellComptroller`.
pub fn moonwell_comptroller(chain_id: i64) -> Option<&'static str> {
    let addr = match chain_id {
        8453 => "0xfBb21d0380beE3312B33c4353c8936a0F13EF26C",
        10 => "0xCa889f40aae37FFf165BccF69aeF1E82b5C511B9",
        _ => return None,
    };
    Some(addr)
}

/// Canonical Tempo Stablecoin DEX contract address (shared across Tempo chains).
const TEMPO_STABLECOIN_DEX_ADDRESS: &str = "0xdec0000000000000000000000000000000000000";

/// Whether `chain_id` is one of the recognized Tempo chains.
fn is_tempo_chain(chain_id: i64) -> bool {
    matches!(chain_id, 31318 | 4217 | 42431)
}

/// Tempo Stablecoin DEX address for a Tempo chain, if covered. Mirrors
/// `registry.TempoStablecoinDEX`.
pub fn tempo_stablecoin_dex(chain_id: i64) -> Option<&'static str> {
    if is_tempo_chain(chain_id) {
        Some(TEMPO_STABLECOIN_DEX_ADDRESS)
    } else {
        None
    }
}

/// Tempo fee-token address for a Tempo chain, if covered. Mirrors
/// `registry.TempoFeeToken`.
pub fn tempo_fee_token(chain_id: i64) -> Option<&'static str> {
    let addr = match chain_id {
        4217 => "0x20c000000000000000000000b9537d11c60e8b50",
        42431 => "0x20c0000000000000000000000000000000000001",
        31318 => "0x20c0000000000000000000000000000000000001",
        _ => return None,
    };
    Some(addr)
}

// ---------------------------------------------------------------------------
// Bridge execution-target allowlists (parity with internal/registry/bridge_targets.go)
// ---------------------------------------------------------------------------

/// Canonical bridge execution targets, sourced from provider deployment
/// artifacts. Returns the allowlisted targets for a `(provider, chain)` pair, or
/// `None` if the pair is not covered. Addresses are stored in their original
/// (mixed-case) form; comparison is done case-insensitively via the canonical
/// EVM-address normalization. Mirrors `registry.bridgeExecutionTargets`.
fn bridge_execution_targets(provider: &str, chain_id: i64) -> Option<&'static [&'static str]> {
    match normalize_bridge_provider(provider).as_str() {
        "lifi" => lifi_execution_targets(chain_id),
        "across" => across_execution_targets(chain_id),
        _ => None,
    }
}

fn lifi_execution_targets(chain_id: i64) -> Option<&'static [&'static str]> {
    let targets: &'static [&'static str] = match chain_id {
        1 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        10 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        56 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        100 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        137 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        143 => &["0x026F252016A7C47CDEf1F05a3Fc9E20C92a49C37"],
        146 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        252 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        324 => &["0x341e94069f53234fE6DabeF707aD424830525715"],
        480 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        999 => &["0x0a0758d937d1059c356D4714e57F5df0239bce1A"],
        5000 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        4326 => &["0x026F252016A7C47CDEf1F05a3Fc9E20C92a49C37"],
        8453 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        42161 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        42220 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        43114 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        57073 => &["0x864b314D4C5a0399368609581d3E8933a63b9232"],
        59144 => &["0xDE1E598b81620773454588B85D6b5D4eEC32573e"],
        80094 => &["0xf909c4Ae16622898b885B89d7F839E0244851c66"],
        81457 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        167000 => &["0x3A9A5dBa8FE1C4Da98187cE4755701BCA182f63b"],
        534352 => &["0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"],
        _ => return None,
    };
    Some(targets)
}

fn across_execution_targets(chain_id: i64) -> Option<&'static [&'static str]> {
    let targets: &'static [&'static str] = match chain_id {
        1 => &[
            "0x5c7BCd6E7De5423a257D81B442095A1a6ced35C5",
            "0x767e4c20F521a829dE4Ffc40C25176676878147f",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
            "0x5616194d65638086a3191B1fEF436f503ff329eC",
            "0x89004EA51Bac007FEc55976967135b2Aa6e838d4",
            "0x4607BceaF7b22cb0c46882FFc9fAB3c6efe66e5a",
        ],
        10 => &[
            "0x3E7448657409278C9d6E192b92F2b69B234FCc42",
            "0x6f26Bf09B1C792e3228e5467807a900A503c0281",
            "0x767e4c20F521a829dE4Ffc40C25176676878147f",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
            "0x986E476F93a423d7a4CD0baF362c5E0903268142",
            "0x6f4A733c7889f038D77D4f540182Dda17423CcbF",
        ],
        56 => &[
            "0x4e8E101924eDE233C13e2D8622DC8aED2872d505",
            "0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
        ],
        137 => &[
            "0xaBa0F11D55C5dDC52cD0Cb2cd052B621d45159d5",
            "0xF9735e425A36d22636EF4cb75c7a6c63378290CA",
            "0x9295ee1d8C5b022Be115A2AD3c30C72E34e7F096",
            "0x767e4c20F521a829dE4Ffc40C25176676878147f",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
            "0x473dEBE3dB7338E03E3c8Dc8e980bb1DACb25bc5",
            "0xC6A21E6A57777F2183312c19e614DD6054b1A54F",
            "0x9220Fa27ae680E4e8D9733932128FA73362E0393",
            "0xC2dCB88873E00c9d401De2CBBa4C6A28f8A6e2c2",
        ],
        143 => &[
            "0xd2ecb3afe598b746F8123CaE365a598DA831A449",
            "0xe9b0666DFfC176Df6686726CB9aaC78fD83D20d7",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
            "0xCbf361EE59Cc74b9d6e7Af947fe4136828faf2C5",
            "0xa3dE5F042EFD4C732498883100A2d319BbB3c1A1",
        ],
        324 => &[
            "0xE0B015E54d54fc84a6cB9B666099c46adE9335FF",
            "0x672b9ba0CE73b69b5F940362F0ee36AAA3F02986",
            "0x5a148a9260c1f670429361c34d40b477280F01a9",
        ],
        480 => &[
            "0x09aea4b2242abC8bb4BB78D537A67a245A7bEC64",
            "0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
            "0x1c8243198570658f818FC56538f2c837C2a32958",
        ],
        999 => &[
            "0x35E63eA3eb0fb7A3bc543C71FB66412e1F6B0E04",
            "0xF1BF00D947267Da5cC63f8c8A60568c59FA31bCb",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
            "0x1c709Fd0Db6A6B877Ddb19ae3D485B7b4ADD879f",
        ],
        4326 => &[
            "0x3Db06DA8F0a24A525f314eeC954fC5c6a973d40E",
            "0xf0aBCe137a493185c5E768F275E7E931109f8981",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
            "0x5BE9F2a2f00475406f09e5bE82c06eFf206721d9",
        ],
        8453 => &[
            "0x7CFaBF2eA327009B39f40078011B0Fb714b65926",
            "0x09aea4b2242abC8bb4BB78D537A67a245A7bEC64",
            "0x767e4c20F521a829dE4Ffc40C25176676878147f",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
            "0xA7A8d1efC1EE3E69999D370380949092251a5c20",
            "0xbcfbCE9D92A516e3e7b0762AE218B4194adE34b4",
        ],
        42161 => &[
            "0xC456398D5eE3B93828252e48beDEDbc39e03368E",
            "0xe35e9842fceaCA96570B734083f4a58e8F7C5f2A",
            "0x767e4c20F521a829dE4Ffc40C25176676878147f",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
            "0xce1FFE01eBB4f8521C12e74363A396ee3d337E1B",
            "0x2ac5Ee3796E027dA274fbDe84c82173a65868940",
            "0xF633b72A4C2Fb73b77A379bf72864A825aD35b6D",
        ],
        57073 => &[
            "0xeF684C38F94F48775959ECf2012D7E864ffb9dd4",
            "0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
            "0x1bE0bCd689Eac8e37346934BfafE8cd0dD231eEE",
            "0x06C61D54958a0772Ee8aF41789466d39FfeaeB13",
        ],
        59144 => &[
            "0x7E63A5f1a8F0B4d0934B2f2327DAED3F6bb2ee75",
            "0xE0BCff426509723B18D6b2f0D8F4602d143bE3e0",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
            "0x60eB88A83434f13095B0A138cdCBf5078Aa5005C",
        ],
        81457 => &[
            "0x2D509190Ed0172ba588407D4c2df918F955Cc6E1",
            "0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
        ],
        534352 => &[
            "0x3baD7AD0728f9917d1Bf08af5782dCbD516cDd96",
            "0x89415a82d909a7238d69094C3Dd1dCC1aCbDa85C",
            "0x10D8b8DaA26d307489803e10477De69C0492B610",
        ],
        _ => return None,
    };
    Some(targets)
}

/// Whether the canonical execution-target allowlist covers `(provider, chain)`.
/// Mirrors `registry.HasBridgeExecutionTargetPolicy`.
pub fn has_bridge_execution_target_policy(provider: &str, chain_id: i64) -> bool {
    bridge_execution_targets(provider, chain_id).is_some()
}

/// Whether `target` is an allowed canonical bridge execution target on
/// `(provider, chain)`, compared case-insensitively. Mirrors
/// `registry.IsAllowedBridgeExecutionTarget`.
pub fn is_allowed_bridge_execution_target(provider: &str, chain_id: i64, target: &str) -> bool {
    let targets = match bridge_execution_targets(provider, chain_id) {
        Some(targets) => targets,
        None => return false,
    };
    let normalized = match normalize_bridge_execution_target(target) {
        Some(normalized) => normalized,
        None => return false,
    };
    targets
        .iter()
        .filter_map(|t| normalize_bridge_execution_target(t))
        .any(|allowed| allowed == normalized)
}

/// Normalize a bridge provider name (lower-cased, trimmed). Mirrors the Go
/// helper `normalizeBridgeProvider`.
fn normalize_bridge_provider(provider: &str) -> String {
    provider.trim().to_ascii_lowercase()
}

/// Normalize a bridge execution-target address to its canonical lower-cased
/// form, or `None` if it is not a valid EVM hex address. Mirrors the Go helper
/// `normalizeBridgeExecutionTarget` (`common.IsHexAddress` +
/// `strings.ToLower(common.HexToAddress(..).Hex())`).
fn normalize_bridge_execution_target(target: &str) -> Option<String> {
    let clean = target.trim();
    if !address::is_hex_address(clean) {
        return None;
    }
    address::parse(clean)
        .ok()
        .map(|addr| addr.to_hex().to_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// ABI fragments (parity with internal/registry/abis.go)
// ---------------------------------------------------------------------------

/// Minimal ERC-20 ABI (allowance/approve/transfer). Mirrors
/// `registry.ERC20MinimalABI`.
pub const ERC20_MINIMAL_ABI: &str = r#"[
    {"name":"allowance","type":"function","stateMutability":"view","inputs":[{"name":"owner","type":"address"},{"name":"spender","type":"address"}],"outputs":[{"name":"","type":"uint256"}]},
    {"name":"approve","type":"function","stateMutability":"nonpayable","inputs":[{"name":"spender","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"","type":"bool"}]},
    {"name":"transfer","type":"function","stateMutability":"nonpayable","inputs":[{"name":"to","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"","type":"bool"}]}
]"#;

/// ERC-4626 vault ABI (asset/deposit/withdraw). Mirrors
/// `registry.ERC4626VaultABI`.
pub const ERC4626_VAULT_ABI: &str = r#"[
    {"name":"asset","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]},
    {"name":"deposit","type":"function","stateMutability":"nonpayable","inputs":[{"name":"assets","type":"uint256"},{"name":"receiver","type":"address"}],"outputs":[{"name":"shares","type":"uint256"}]},
    {"name":"withdraw","type":"function","stateMutability":"nonpayable","inputs":[{"name":"assets","type":"uint256"},{"name":"receiver","type":"address"},{"name":"owner","type":"address"}],"outputs":[{"name":"shares","type":"uint256"}]}
]"#;

/// Uniswap V3 QuoterV2 ABI. Mirrors `registry.UniswapV3QuoterV2ABI`.
pub const UNISWAP_V3_QUOTER_V2_ABI: &str = r#"[
    {"name":"quoteExactInputSingle","type":"function","stateMutability":"nonpayable","inputs":[{"name":"params","type":"tuple","components":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"amountIn","type":"uint256"},{"name":"fee","type":"uint24"},{"name":"sqrtPriceLimitX96","type":"uint160"}]}],"outputs":[{"name":"amountOut","type":"uint256"},{"name":"sqrtPriceX96After","type":"uint160"},{"name":"initializedTicksCrossed","type":"uint32"},{"name":"gasEstimate","type":"uint256"}]}
]"#;

/// Uniswap V3 Router ABI. Mirrors `registry.UniswapV3RouterABI`.
pub const UNISWAP_V3_ROUTER_ABI: &str = r#"[
    {"name":"exactInputSingle","type":"function","stateMutability":"payable","inputs":[{"name":"params","type":"tuple","components":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"fee","type":"uint24"},{"name":"recipient","type":"address"},{"name":"amountIn","type":"uint256"},{"name":"amountOutMinimum","type":"uint256"},{"name":"sqrtPriceLimitX96","type":"uint160"}]}],"outputs":[{"name":"amountOut","type":"uint256"}]}
]"#;

/// Tempo Stablecoin DEX ABI. Mirrors `registry.TempoStablecoinDEXABI`.
pub const TEMPO_STABLECOIN_DEX_ABI: &str = r#"[
    {"name":"quoteSwapExactAmountIn","type":"function","stateMutability":"view","inputs":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"amountIn","type":"uint128"}],"outputs":[{"name":"amountOut","type":"uint128"}]},
    {"name":"quoteSwapExactAmountOut","type":"function","stateMutability":"view","inputs":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"amountOut","type":"uint128"}],"outputs":[{"name":"amountIn","type":"uint128"}]},
    {"name":"swapExactAmountIn","type":"function","stateMutability":"nonpayable","inputs":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"amountIn","type":"uint128"},{"name":"minAmountOut","type":"uint128"}],"outputs":[{"name":"amountOut","type":"uint128"}]},
    {"name":"swapExactAmountOut","type":"function","stateMutability":"nonpayable","inputs":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"amountOut","type":"uint128"},{"name":"maxAmountIn","type":"uint128"}],"outputs":[{"name":"amountIn","type":"uint128"}]}
]"#;

/// Tempo TIP-20 metadata ABI. Mirrors `registry.TempoTIP20MetadataABI`.
pub const TEMPO_TIP20_METADATA_ABI: &str = r#"[
    {"name":"currency","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"string"}]},
    {"name":"quoteToken","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]}
]"#;

/// Aave PoolAddressesProvider ABI. Mirrors `registry.AavePoolAddressProviderABI`.
pub const AAVE_POOL_ADDRESS_PROVIDER_ABI: &str = r#"[
    {"name":"getPool","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]},
    {"name":"getAddress","type":"function","stateMutability":"view","inputs":[{"name":"id","type":"bytes32"}],"outputs":[{"name":"","type":"address"}]}
]"#;

/// Aave Pool ABI. Mirrors `registry.AavePoolABI`.
pub const AAVE_POOL_ABI: &str = r#"[
    {"name":"supply","type":"function","stateMutability":"nonpayable","inputs":[{"name":"asset","type":"address"},{"name":"amount","type":"uint256"},{"name":"onBehalfOf","type":"address"},{"name":"referralCode","type":"uint16"}],"outputs":[]},
    {"name":"withdraw","type":"function","stateMutability":"nonpayable","inputs":[{"name":"asset","type":"address"},{"name":"amount","type":"uint256"},{"name":"to","type":"address"}],"outputs":[{"name":"","type":"uint256"}]},
    {"name":"borrow","type":"function","stateMutability":"nonpayable","inputs":[{"name":"asset","type":"address"},{"name":"amount","type":"uint256"},{"name":"interestRateMode","type":"uint256"},{"name":"referralCode","type":"uint16"},{"name":"onBehalfOf","type":"address"}],"outputs":[]},
    {"name":"repay","type":"function","stateMutability":"nonpayable","inputs":[{"name":"asset","type":"address"},{"name":"amount","type":"uint256"},{"name":"interestRateMode","type":"uint256"},{"name":"onBehalfOf","type":"address"}],"outputs":[{"name":"","type":"uint256"}]}
]"#;

/// Aave Rewards ABI. Mirrors `registry.AaveRewardsABI`.
pub const AAVE_REWARDS_ABI: &str = r#"[
    {"name":"claimRewards","type":"function","stateMutability":"nonpayable","inputs":[{"name":"assets","type":"address[]"},{"name":"amount","type":"uint256"},{"name":"to","type":"address"},{"name":"reward","type":"address"}],"outputs":[{"name":"","type":"uint256"}]}
]"#;

/// Moonwell Comptroller ABI. Mirrors `registry.MoonwellComptrollerABI`.
pub const MOONWELL_COMPTROLLER_ABI: &str = r#"[
    {"name":"getAllMarkets","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address[]"}]},
    {"name":"getAssetsIn","type":"function","stateMutability":"view","inputs":[{"name":"account","type":"address"}],"outputs":[{"name":"","type":"address[]"}]},
    {"name":"checkMembership","type":"function","stateMutability":"view","inputs":[{"name":"account","type":"address"},{"name":"mToken","type":"address"}],"outputs":[{"name":"","type":"bool"}]},
    {"name":"enterMarkets","type":"function","stateMutability":"nonpayable","inputs":[{"name":"mTokens","type":"address[]"}],"outputs":[{"name":"","type":"uint256[]"}]},
    {"name":"markets","type":"function","stateMutability":"view","inputs":[{"name":"","type":"address"}],"outputs":[{"name":"isListed","type":"bool"},{"name":"collateralFactorMantissa","type":"uint256"}]},
    {"name":"oracle","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]}
]"#;

/// Moonwell mToken ABI. Mirrors `registry.MoonwellMTokenABI`.
pub const MOONWELL_MTOKEN_ABI: &str = r#"[
    {"name":"underlying","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]},
    {"name":"symbol","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"string"}]},
    {"name":"decimals","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"uint8"}]},
    {"name":"supplyRatePerTimestamp","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"uint256"}]},
    {"name":"borrowRatePerTimestamp","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"uint256"}]},
    {"name":"totalSupply","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"uint256"}]},
    {"name":"totalBorrowsCurrent","type":"function","stateMutability":"nonpayable","inputs":[],"outputs":[{"name":"","type":"uint256"}]},
    {"name":"exchangeRateCurrent","type":"function","stateMutability":"nonpayable","inputs":[],"outputs":[{"name":"","type":"uint256"}]},
    {"name":"getCash","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"uint256"}]},
    {"name":"getAccountSnapshot","type":"function","stateMutability":"view","inputs":[{"name":"account","type":"address"}],"outputs":[{"name":"","type":"uint256"},{"name":"","type":"uint256"},{"name":"","type":"uint256"},{"name":"","type":"uint256"}]},
    {"name":"mint","type":"function","stateMutability":"nonpayable","inputs":[{"name":"mintAmount","type":"uint256"}],"outputs":[{"name":"","type":"uint256"}]},
    {"name":"redeemUnderlying","type":"function","stateMutability":"nonpayable","inputs":[{"name":"redeemAmount","type":"uint256"}],"outputs":[{"name":"","type":"uint256"}]},
    {"name":"borrow","type":"function","stateMutability":"nonpayable","inputs":[{"name":"borrowAmount","type":"uint256"}],"outputs":[{"name":"","type":"uint256"}]},
    {"name":"repayBorrow","type":"function","stateMutability":"nonpayable","inputs":[{"name":"repayAmount","type":"uint256"}],"outputs":[{"name":"","type":"uint256"}]}
]"#;

/// Moonwell Oracle ABI. Mirrors `registry.MoonwellOracleABI`.
pub const MOONWELL_ORACLE_ABI: &str = r#"[
    {"name":"getUnderlyingPrice","type":"function","stateMutability":"view","inputs":[{"name":"mToken","type":"address"}],"outputs":[{"name":"","type":"uint256"}]}
]"#;

/// Moonwell minimal ERC-20 ABI. Mirrors `registry.MoonwellERC20MinimalABI`.
pub const MOONWELL_ERC20_MINIMAL_ABI: &str = r#"[
    {"name":"symbol","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"string"}]},
    {"name":"decimals","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"uint8"}]}
]"#;

/// Multicall3 ABI. Mirrors `registry.Multicall3ABI`.
pub const MULTICALL3_ABI: &str = r#"[
    {"name":"aggregate3","type":"function","stateMutability":"payable","inputs":[{"name":"calls","type":"tuple[]","components":[{"name":"target","type":"address"},{"name":"allowFailure","type":"bool"},{"name":"callData","type":"bytes"}]}],"outputs":[{"name":"returnData","type":"tuple[]","components":[{"name":"success","type":"bool"},{"name":"returnData","type":"bytes"}]}]}
]"#;

/// Morpho Blue ABI. Mirrors `registry.MorphoBlueABI`.
pub const MORPHO_BLUE_ABI: &str = r#"[
    {"name":"supply","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"data","type":"bytes"}],"outputs":[{"name":"assetsSupplied","type":"uint256"},{"name":"sharesSupplied","type":"uint256"}]},
    {"name":"withdraw","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"receiver","type":"address"}],"outputs":[{"name":"assetsWithdrawn","type":"uint256"},{"name":"sharesWithdrawn","type":"uint256"}]},
    {"name":"borrow","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"receiver","type":"address"}],"outputs":[{"name":"assetsBorrowed","type":"uint256"},{"name":"sharesBorrowed","type":"uint256"}]},
    {"name":"repay","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"data","type":"bytes"}],"outputs":[{"name":"assetsRepaid","type":"uint256"},{"name":"sharesRepaid","type":"uint256"}]}
]"#;

#[cfg(test)]
mod tests {
    //! These assert the contract this crate owns (default RPC map,
    //! RPC-resolution precedence, canonical contract lookups, bridge guardrail
    //! allowlists, and ABI-fragment validity).
    //!
    //! Cases are ported from `internal/registry/registry_test.go` and
    //! `contracts_test.go`, plus fresh spec-driven assertions for the
    //! `ResolveRPCURL` trim/precedence/error contract and for ABI parse parity
    //! via `defi_evm::abi`.
    use super::*;

    // The canonical LiFi Diamond shared across most major EVM chains.
    const LIFI_DIAMOND: &str = "0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE";

    // ---------- 1. default RPC map (DefaultRPCURL) ----------

    #[test]
    fn default_rpc_url_known_chains_nonempty() {
        for chain in [1_i64, 8453, 167000, 4217, 42161, 10] {
            let url = default_rpc_url(chain)
                .unwrap_or_else(|| panic!("expected default rpc for chain {chain}"));
            assert!(
                !url.is_empty(),
                "rpc url for chain {chain} must be non-empty"
            );
            assert!(
                url.starts_with("http"),
                "rpc url for chain {chain} must be a url, got {url:?}"
            );
        }
    }

    #[test]
    fn default_rpc_url_exact_values() {
        // Exact-match a few canonical entries from internal/registry/rpc.go.
        assert_eq!(default_rpc_url(1), Some("https://eth.llamarpc.com"));
        assert_eq!(default_rpc_url(8453), Some("https://mainnet.base.org"));
        assert_eq!(
            default_rpc_url(167000),
            Some("https://rpc.mainnet.taiko.xyz")
        );
        assert_eq!(default_rpc_url(4217), Some("https://rpc.tempo.xyz"));
    }

    #[test]
    fn default_rpc_url_unknown_chain_is_none() {
        assert_eq!(default_rpc_url(999999), None);
    }

    // ---------- 2. ResolveRPCURL (override > default > error) ----------

    #[test]
    fn resolve_rpc_url_override_wins_and_is_trimmed() {
        let got =
            resolve_rpc_url(" https://rpc.example.test ", 1).expect("override should resolve");
        assert_eq!(got, "https://rpc.example.test");
    }

    #[test]
    fn resolve_rpc_url_blank_override_falls_back_to_default() {
        let got = resolve_rpc_url("", 1).expect("default should resolve");
        assert_eq!(got, "https://eth.llamarpc.com");
        // Whitespace-only override is treated as blank.
        let got_ws = resolve_rpc_url("   ", 1).expect("default should resolve");
        assert_eq!(got_ws, "https://eth.llamarpc.com");
    }

    #[test]
    fn resolve_rpc_url_missing_default_is_usage_error() {
        let err = resolve_rpc_url("", 999999).unwrap_err();
        assert_eq!(err.code, Code::Usage);
        // Message references the offending chain id and the override flag.
        let msg = err.to_string();
        assert!(
            msg.contains("999999"),
            "message should name the chain id: {msg}"
        );
        assert!(
            msg.contains("--rpc-url"),
            "message should mention --rpc-url: {msg}"
        );
    }

    // ---------- 3. UniswapV3Contracts ----------

    #[test]
    fn uniswap_v3_contracts_supported_chain() {
        let (quoter, router) =
            uniswap_v3_contracts(167000).expect("taiko mainnet contracts must exist");
        assert!(!quoter.is_empty() && !router.is_empty());
        // Taiko hoodi is also covered.
        assert!(uniswap_v3_contracts(167013).is_some());
    }

    #[test]
    fn uniswap_v3_contracts_unsupported_chain_is_none() {
        assert_eq!(uniswap_v3_contracts(1), None);
    }

    // ---------- 4. AavePoolAddressProvider ----------

    #[test]
    fn aave_pool_address_provider_covered_chains() {
        for chain in [1_i64, 8453, 42161, 10, 137, 43114] {
            let addr = aave_pool_address_provider(chain)
                .unwrap_or_else(|| panic!("expected aave provider for chain {chain}"));
            assert!(!addr.is_empty());
        }
    }

    #[test]
    fn aave_pool_address_provider_uncovered_chain_is_none() {
        assert_eq!(aave_pool_address_provider(167000), None);
    }

    // ---------- 5. MoonwellComptroller ----------

    #[test]
    fn moonwell_comptroller_covered_chains() {
        assert!(moonwell_comptroller(8453).is_some());
        assert!(moonwell_comptroller(10).is_some());
        assert_eq!(moonwell_comptroller(1), None);
    }

    // ---------- 6. Tempo addresses (TempoStablecoinDEX / TempoFeeToken) ----------

    #[test]
    fn tempo_stablecoin_dex_only_tempo_chains() {
        for chain in [4217_i64, 42431, 31318] {
            let addr = tempo_stablecoin_dex(chain)
                .unwrap_or_else(|| panic!("expected tempo dex for chain {chain}"));
            assert!(!addr.is_empty());
        }
        assert_eq!(tempo_stablecoin_dex(1), None);
        assert_eq!(tempo_stablecoin_dex(8453), None);
    }

    #[test]
    fn tempo_fee_token_only_tempo_chains() {
        for chain in [4217_i64, 42431, 31318] {
            let addr = tempo_fee_token(chain)
                .unwrap_or_else(|| panic!("expected tempo fee token for chain {chain}"));
            assert!(!addr.is_empty());
        }
        assert_eq!(tempo_fee_token(1), None);
        assert_eq!(tempo_fee_token(8453), None);
    }

    // ---------- 7. BridgeSettlementURL ----------

    #[test]
    fn bridge_settlement_url_known_providers() {
        assert_eq!(bridge_settlement_url("lifi"), Some(LIFI_SETTLEMENT_URL));
        assert_eq!(bridge_settlement_url("across"), Some(ACROSS_SETTLEMENT_URL));
    }

    #[test]
    fn bridge_settlement_url_is_case_and_space_insensitive() {
        assert_eq!(bridge_settlement_url("  LiFi "), Some(LIFI_SETTLEMENT_URL));
        assert_eq!(bridge_settlement_url("ACROSS"), Some(ACROSS_SETTLEMENT_URL));
    }

    #[test]
    fn bridge_settlement_url_unknown_provider_is_none() {
        assert_eq!(bridge_settlement_url("unknown"), None);
    }

    // ---------- 8. IsAllowedBridgeSettlementURL ----------

    #[test]
    fn settlement_url_empty_endpoint_allowed() {
        assert!(is_allowed_bridge_settlement_url("lifi", ""));
    }

    #[test]
    fn settlement_url_canonical_allowed() {
        assert!(is_allowed_bridge_settlement_url(
            "lifi",
            LIFI_SETTLEMENT_URL
        ));
        // Canonical endpoint with the explicit default https port is allowed.
        assert!(is_allowed_bridge_settlement_url(
            "lifi",
            "https://li.quest:443/v1/status"
        ));
    }

    #[test]
    fn settlement_url_loopback_allowed_for_dev() {
        assert!(is_allowed_bridge_settlement_url(
            "across",
            "http://127.0.0.1:8080/status"
        ));
    }

    #[test]
    fn settlement_url_cross_provider_rejected() {
        assert!(!is_allowed_bridge_settlement_url(
            "lifi",
            ACROSS_SETTLEMENT_URL
        ));
    }

    #[test]
    fn settlement_url_non_https_non_loopback_rejected() {
        assert!(!is_allowed_bridge_settlement_url(
            "lifi",
            "http://li.quest/v1/status"
        ));
    }

    #[test]
    fn settlement_url_wrong_path_rejected() {
        assert!(!is_allowed_bridge_settlement_url(
            "lifi",
            "https://li.quest/v1/other"
        ));
    }

    #[test]
    fn settlement_url_malformed_rejected() {
        assert!(!is_allowed_bridge_settlement_url("across", "not-a-url"));
    }

    #[test]
    fn settlement_url_wrong_explicit_port_rejected() {
        // A non-default explicit port must not normalize to the canonical 443.
        // (Go: normalizedURLPort("...:8443") == "8443" != "443".)
        assert!(!is_allowed_bridge_settlement_url(
            "lifi",
            "https://li.quest:8443/v1/status"
        ));
    }

    #[test]
    fn settlement_url_trailing_slash_path_allowed() {
        // normalizedURLPath strips a single trailing slash, so the canonical path
        // with a trailing slash is equivalent. (Go ground truth: true.)
        assert!(is_allowed_bridge_settlement_url(
            "lifi",
            "https://li.quest/v1/status/"
        ));
    }

    #[test]
    fn settlement_url_host_and_scheme_case_insensitive() {
        // Host and scheme are compared via EqualFold in Go; both must match
        // case-insensitively. (Go ground truth: both true.)
        assert!(is_allowed_bridge_settlement_url(
            "lifi",
            "https://LI.QUEST/v1/status"
        ));
        assert!(is_allowed_bridge_settlement_url(
            "lifi",
            "HTTPS://li.quest/v1/status"
        ));
    }

    #[test]
    fn settlement_url_query_string_is_ignored() {
        // parsed.Path excludes the query, so a query string does not change the
        // canonical-path comparison. (Go ground truth: true.)
        assert!(is_allowed_bridge_settlement_url(
            "lifi",
            "https://li.quest/v1/status?x=1"
        ));
    }

    #[test]
    fn settlement_url_no_scheme_authority_rejected() {
        // Without a `scheme://authority`, Go's url.Parse leaves Hostname() empty
        // and the guardrail rejects it. (Go ground truth: false.)
        assert!(!is_allowed_bridge_settlement_url(
            "lifi",
            "li.quest/v1/status"
        ));
    }

    #[test]
    fn settlement_url_localhost_loopback_allowed() {
        // `localhost` is treated as loopback (dev), independent of canonical host.
        assert!(is_allowed_bridge_settlement_url(
            "across",
            "http://localhost/status"
        ));
    }

    // ---------- 9. HasBridgeExecutionTargetPolicy ----------

    #[test]
    fn lifi_target_policy_covers_all_major_evm_chains() {
        let lifi_chains: [i64; 20] = [
            1, 10, 56, 100, 137, 146, 252, 324, 480, 5000, 8453, 42161, 42220, 43114, 57073, 59144,
            80094, 81457, 167000, 534352,
        ];
        for chain in lifi_chains {
            assert!(
                has_bridge_execution_target_policy("lifi", chain),
                "expected lifi target policy coverage for chain {chain}"
            );
        }
    }

    #[test]
    fn across_target_policy_covers_supported_chains() {
        for chain in [1_i64, 10, 137, 8453, 42161] {
            assert!(
                has_bridge_execution_target_policy("across", chain),
                "expected across target policy coverage for chain {chain}"
            );
        }
    }

    #[test]
    fn target_policy_rejects_uncovered_and_unknown() {
        assert!(!has_bridge_execution_target_policy("across", 43114));
        assert!(!has_bridge_execution_target_policy("unknown", 1));
    }

    // ---------- 10. IsAllowedBridgeExecutionTarget ----------

    #[test]
    fn lifi_standard_diamond_allowed_on_standard_chains() {
        let standard: [i64; 15] = [
            1, 10, 56, 100, 137, 146, 252, 480, 5000, 8453, 42161, 42220, 43114, 81457, 534352,
        ];
        for chain in standard {
            assert!(
                is_allowed_bridge_execution_target("lifi", chain, LIFI_DIAMOND),
                "expected canonical lifi diamond allowed on chain {chain}"
            );
        }
    }

    #[test]
    fn lifi_target_is_case_insensitive() {
        assert!(is_allowed_bridge_execution_target(
            "lifi",
            8453,
            "0x1231deb6f5749ef6ce6943a275a1d3e7486f4eae"
        ));
    }

    #[test]
    fn lifi_unknown_target_rejected() {
        assert!(!is_allowed_bridge_execution_target(
            "lifi",
            8453,
            "0x1111111111111111111111111111111111111111"
        ));
    }

    #[test]
    fn lifi_chain_specific_diamond_only_on_its_own_chain() {
        // zkSync (324) uses a non-standard diamond address.
        assert!(is_allowed_bridge_execution_target(
            "lifi",
            324,
            "0x341e94069f53234fE6DabeF707aD424830525715"
        ));
        // The standard diamond must NOT be accepted on zkSync.
        assert!(!is_allowed_bridge_execution_target(
            "lifi",
            324,
            LIFI_DIAMOND
        ));
    }

    #[test]
    fn across_canonical_target_case_insensitive() {
        assert!(is_allowed_bridge_execution_target(
            "across",
            1,
            "0x767e4c20F521a829dE4Ffc40C25176676878147f"
        ));
        assert!(is_allowed_bridge_execution_target(
            "across",
            1,
            "0x767E4C20F521A829DE4FFC40C25176676878147F"
        ));
    }

    #[test]
    fn execution_target_rejects_malformed_empty_wrong_provider_and_uncovered_chain() {
        assert!(!is_allowed_bridge_execution_target(
            "across",
            1,
            "not-an-address"
        ));
        assert!(!is_allowed_bridge_execution_target("lifi", 1, ""));
        // Across target on a chain Across does not cover.
        assert!(!is_allowed_bridge_execution_target(
            "across",
            43114,
            "0x767e4c20F521a829dE4Ffc40C25176676878147f"
        ));
        // The LiFi diamond is not an allowed Across target.
        assert!(!is_allowed_bridge_execution_target(
            "across",
            1,
            "0x1231DeB6f5749EF6Ce6943a275A1D3E7486F4EaE"
        ));
    }

    // ---------- 11. ABI fragments parse via defi_evm::abi ----------

    // Each fragment paired with one function name we expect to extract from it.
    // Parity with the Go `abi.JSON(strings.NewReader(raw))` parse test, but
    // strengthened: we also assert a known method is present (a "[]" stub passes
    // a bare json-parse but has no functions, so it must fail this check).
    const ABI_FRAGMENTS: &[(&str, &str)] = &[
        (ERC20_MINIMAL_ABI, "approve"),
        (ERC4626_VAULT_ABI, "deposit"),
        (UNISWAP_V3_QUOTER_V2_ABI, "quoteExactInputSingle"),
        (UNISWAP_V3_ROUTER_ABI, "exactInputSingle"),
        (TEMPO_STABLECOIN_DEX_ABI, "swapExactAmountIn"),
        (TEMPO_TIP20_METADATA_ABI, "currency"),
        (AAVE_POOL_ADDRESS_PROVIDER_ABI, "getPool"),
        (AAVE_POOL_ABI, "supply"),
        (AAVE_REWARDS_ABI, "claimRewards"),
        (MOONWELL_COMPTROLLER_ABI, "getAllMarkets"),
        (MOONWELL_MTOKEN_ABI, "mint"),
        (MOONWELL_ORACLE_ABI, "getUnderlyingPrice"),
        (MOONWELL_ERC20_MINIMAL_ABI, "symbol"),
        (MULTICALL3_ABI, "aggregate3"),
        (MORPHO_BLUE_ABI, "supply"),
    ];

    #[test]
    fn all_abi_fragments_parse_and_expose_known_function() {
        for (raw, func) in ABI_FRAGMENTS {
            let parsed = defi_evm::abi::Function::from_abi_json(raw, func);
            assert!(
                parsed.is_ok(),
                "ABI fragment for {func:?} must parse and expose that function; got {:?}",
                parsed.err()
            );
        }
    }
}
