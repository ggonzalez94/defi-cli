//! Open Wallet Standard (OWS) backend client.
//!
//! Mirrors `internal/ows`. Wallet-backed submit uses a persisted `wallet_id`
//! plus `DEFI_OWS_TOKEN`. This crate shells out to the `ows` CLI for signing +
//! broadcast (`ows sign send-tx`) and reads local OWS vault wallet metadata to
//! resolve a wallet reference and its per-chain sender address.
//!
//! The GREEN-phase port mirrors Go `internal/ows`: input validation +
//! exit-code mapping, exact `ows sign send-tx` command construction, failure
//! classification (policy denial vs. generic signer failure), tx-hash parsing
//! and validation, and local vault wallet-metadata resolution.

use std::path::{Path, PathBuf};

use defi_errors::Error;
// Re-export the error code enum so callers map OWS failures without depending on
// `defi-errors` directly.
pub use defi_errors::Code;

/// Environment variable carrying the OWS passphrase token used to unlock the
/// vault for `ows sign send-tx`. Mirrors Go `EnvOWSToken`.
pub const ENV_OWS_TOKEN: &str = "DEFI_OWS_TOKEN";

/// Result of a successful `ows sign send-tx` broadcast.
///
/// Field DECLARATION order is part of the JSON contract (spec §2.3): `tx_hash`
/// then `chain`. Mirrors Go `SendTxResult`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SendTxResult {
    pub tx_hash: String,
    pub chain: String,
}

/// A locally stored OWS wallet's metadata. Mirrors Go `Wallet`.
///
/// serde field names + declaration order copied from `internal/ows/vault.go`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Wallet {
    pub id: String,
    pub name: String,
    pub created_at: String,
    #[serde(default)]
    pub accounts: Vec<WalletAccount>,
}

/// A single per-chain account inside an OWS wallet. Mirrors Go `WalletAccount`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WalletAccount {
    pub account_id: String,
    pub address: String,
    pub chain_id: String,
    pub derivation_path: String,
}

/// Abstraction over locating + running the external `ows` binary.
///
/// This is the idiomatic Rust replacement for Go's package-level
/// `lookPathFunc` / `runCommandFunc` test seams: instead of mutable global
/// function pointers, [`send_unsigned_tx`] takes a `&dyn CommandRunner` so tests
/// inject a fake and production uses the real PATH/subprocess implementation.
pub trait CommandRunner {
    /// Resolve an executable name (e.g. `"ows"`) to a full path, like
    /// `exec.LookPath`. Returns the resolved path or an error if not found.
    fn look_path(&self, file: &str) -> Result<String, std::io::Error>;

    /// Run `bin args...` with the given extra environment entries (each
    /// `KEY=VALUE`) appended to the inherited environment. Returns
    /// `(stdout, stderr, run_failed)`: `run_failed` is `Some(detail)` when the
    /// process exits non-zero (mirroring Go returning a non-nil `error`), else
    /// `None` on a clean exit.
    fn run(&self, bin: &str, args: &[String], env: &[(String, String)]) -> CommandOutput;
}

/// Output of a [`CommandRunner::run`] invocation.
#[derive(Debug, Clone, Default)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// `Some(detail)` when the process failed (non-zero exit); `None` on success.
    pub run_error: Option<String>,
}

/// Sign and broadcast an unsigned EVM transaction via the `ows` CLI.
///
/// Mirrors Go `SendUnsignedTx`. Validates inputs, requires the `ows` binary on
/// PATH and a non-empty [`ENV_OWS_TOKEN`], builds the `ows sign send-tx` arg
/// vector, runs it with `OWS_PASSPHRASE` injected, and parses + validates the
/// returned tx hash.
pub fn send_unsigned_tx(
    runner: &dyn CommandRunner,
    token: Option<&str>,
    wallet_id: &str,
    chain_id: &str,
    tx_bytes: &[u8],
    rpc_url: &str,
) -> Result<SendTxResult, Error> {
    let wallet_id = wallet_id.trim();
    if wallet_id.is_empty() {
        return Err(Error::new(Code::Usage, "wallet id is required"));
    }
    let chain_id = chain_id.trim();
    if chain_id.is_empty() {
        return Err(Error::new(Code::Usage, "chain id is required"));
    }
    if tx_bytes.is_empty() {
        return Err(Error::new(Code::Usage, "unsigned tx bytes are required"));
    }

    let ows_bin = runner
        .look_path("ows")
        .map_err(|err| Error::wrap(Code::Unavailable, "ows CLI not found in PATH", err))?;

    let token = token.map(str::trim).unwrap_or("");
    if token.is_empty() {
        return Err(Error::new(
            Code::Signer,
            "missing DEFI_OWS_TOKEN for OWS passphrase",
        ));
    }

    let args = build_send_tx_args(wallet_id, chain_id, tx_bytes, rpc_url);
    let env = vec![("OWS_PASSPHRASE".to_string(), token.to_string())];

    let output = runner.run(&ows_bin, &args, &env);
    if output.run_error.is_some() {
        return Err(classify_command_failure(&output));
    }

    parse_send_tx_result(&output.stdout, chain_id)
        .map_err(|err| Error::wrap(Code::Signer, "parse ows send-tx response", err))
}

/// Classify a non-zero `ows send-tx` exit into a typed [`Error`].
///
/// Mirrors Go `classifyCommandFailure`: prefer stderr detail, fall back to
/// stdout; a policy-denial signal maps to [`Code::ActionPolicy`], anything else
/// to [`Code::Signer`].
fn classify_command_failure(output: &CommandOutput) -> Error {
    let mut detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if detail.is_empty() {
        detail = String::from_utf8_lossy(&output.stdout).trim().to_string();
    }

    if is_policy_denied_detail(&detail) {
        if detail.is_empty() {
            return Error::new(Code::ActionPolicy, "ows policy denied transaction");
        }
        return Error::wrap(
            Code::ActionPolicy,
            "ows policy denied transaction",
            DetailError(detail),
        );
    }

    if detail.is_empty() {
        return Error::new(Code::Signer, "ows send-tx command failed");
    }
    Error::wrap(
        Code::Signer,
        "ows send-tx command failed",
        DetailError(detail),
    )
}

/// Whether a command's stderr/stdout `detail` signals an OWS policy denial.
///
/// Mirrors Go `isPolicyDeniedDetail`: case-insensitive match for `policy_denied`
/// or, after normalizing `_`/`-` to spaces, `policy denied` / `denied by policy`.
fn is_policy_denied_detail(detail: &str) -> bool {
    let lower = detail.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    if lower.contains("policy_denied") {
        return true;
    }
    let normalized = lower.replace(['_', '-'], " ");
    normalized.contains("policy denied") || normalized.contains("denied by policy")
}

/// A lightweight error carrying a free-form detail string for [`Error::wrap`].
#[derive(Debug)]
struct DetailError(String);

impl std::fmt::Display for DetailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DetailError {}

/// Build the `ows sign send-tx` argument vector. Mirrors the Go arg assembly.
///
/// Exposed so the RED tests can assert the exact, ordered arg list without
/// mocking a full subprocess; `--rpc-url <url>` is appended only when `rpc_url`
/// is non-empty (after trimming).
pub fn build_send_tx_args(
    wallet_id: &str,
    chain_id: &str,
    tx_bytes: &[u8],
    rpc_url: &str,
) -> Vec<String> {
    let mut args = vec![
        "sign".to_string(),
        "send-tx".to_string(),
        "--wallet".to_string(),
        wallet_id.to_string(),
        "--chain".to_string(),
        chain_id.to_string(),
        "--tx".to_string(),
        format!("0x{}", hex::encode(tx_bytes)),
        "--json".to_string(),
    ];
    let trimmed_rpc = rpc_url.trim();
    if !trimmed_rpc.is_empty() {
        args.push("--rpc-url".to_string());
        args.push(trimmed_rpc.to_string());
    }
    args
}

/// Parse an `ows sign send-tx --json` response into a [`SendTxResult`].
///
/// Mirrors Go `parseSendTxResult`: accepts either `tx_hash` (snake) or `txHash`
/// (camel), preferring snake; validates the hash via [`is_tx_hash`]; falls back
/// to `fallback_chain` when the response omits `chain`.
pub fn parse_send_tx_result(out: &[u8], fallback_chain: &str) -> Result<SendTxResult, Error> {
    #[derive(serde::Deserialize, Default)]
    struct SendTxCliResult {
        #[serde(default)]
        tx_hash: String,
        #[serde(default, rename = "txHash")]
        tx_hash_camel: String,
        #[serde(default)]
        chain: String,
    }

    let parsed: SendTxCliResult = serde_json::from_slice(out)
        .map_err(|err| Error::wrap(Code::Signer, "decode ows send-tx response", err))?;

    let mut tx_hash = parsed.tx_hash.trim().to_string();
    if tx_hash.is_empty() {
        tx_hash = parsed.tx_hash_camel.trim().to_string();
    }
    if tx_hash.is_empty() {
        return Err(Error::new(Code::Signer, "missing tx hash in ows response"));
    }
    if !is_tx_hash(&tx_hash) {
        return Err(Error::new(
            Code::Signer,
            format!("invalid tx hash in ows response: {tx_hash:?}"),
        ));
    }

    let mut chain = parsed.chain.trim().to_string();
    if chain.is_empty() {
        chain = fallback_chain.trim().to_string();
    }

    Ok(SendTxResult { tx_hash, chain })
}

/// Whether `value` is a canonical 0x-prefixed 32-byte (66-char) hex tx hash.
///
/// Mirrors Go `IsTxHash`.
pub fn is_tx_hash(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.len() != 66 || !trimmed.starts_with("0x") {
        return false;
    }
    hex::decode(&trimmed[2..]).is_ok()
}

/// Resolve a wallet reference (`id` first, then `name`) against the OWS vault.
///
/// Mirrors Go `ResolveWalletRef`. `vault_dir` empty → defaults to `~/.ows`.
/// Reads `<vault>/wallets/*.json`. Resolution order: exact `id` match (ambiguous
/// id → error), else exact `name` match (ambiguous name → error, no match →
/// error).
pub fn resolve_wallet_ref(vault_dir: &str, reference: &str) -> Result<Wallet, Error> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err(Error::new(Code::Usage, "wallet reference is required"));
    }

    let vault_path = resolve_vault_path(vault_dir)?;
    let wallets = load_wallets(&vault_path)?;

    let id_matches: Vec<&Wallet> = wallets.iter().filter(|w| w.id == reference).collect();
    match id_matches.as_slice() {
        [only] => return Ok((*only).clone()),
        [] => {} // fall through to name matching
        _ => {
            return Err(Error::new(
                Code::Usage,
                format!("ambiguous wallet id {reference:?}"),
            ))
        }
    }

    let name_matches: Vec<&Wallet> = wallets.iter().filter(|w| w.name == reference).collect();
    match name_matches.as_slice() {
        [only] => Ok((*only).clone()),
        [] => Err(Error::new(
            Code::Usage,
            format!("wallet {reference:?} not found"),
        )),
        _ => Err(Error::new(
            Code::Usage,
            format!("ambiguous wallet name {reference:?}"),
        )),
    }
}

/// Resolve the OWS vault directory, expanding `~`/`~/` and defaulting to
/// `~/.ows` when `vault_dir` is blank. Mirrors Go `resolveVaultPath`.
fn resolve_vault_path(vault_dir: &str) -> Result<PathBuf, Error> {
    let value = vault_dir.trim();
    let value = if value.is_empty() { "~/.ows" } else { value };
    expand_user_path(value)
}

/// Expand a leading `~`/`~/` against the user's home directory.
///
/// Mirrors the home-expansion portion of Go `fsutil.NormalizePath`. The vault
/// path is used as a directory root for a `wallets/*.json` listing, so it does
/// not need Go's full `Clean`/`Abs` canonicalization for correct lookup.
fn expand_user_path(value: &str) -> Result<PathBuf, Error> {
    if value == "~" {
        return home_dir();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return Ok(home_dir()?.join(rest));
    }
    Ok(PathBuf::from(value))
}

/// The user home directory (`os.UserHomeDir` equivalent, reading `$HOME`).
fn home_dir() -> Result<PathBuf, Error> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| Error::new(Code::Internal, "resolve home directory"))
}

/// The sender address for `chain_id` within `wallet`.
///
/// Mirrors Go `SenderAddressForChain`: exact `chain_id` match first; for
/// `eip155:*` chains, fall back to ANY `eip155:*` account; else error.
pub fn sender_address_for_chain(wallet: &Wallet, chain_id: &str) -> Result<String, Error> {
    let chain_id = chain_id.trim();
    if chain_id.is_empty() {
        return Err(Error::new(Code::Usage, "chain id is required"));
    }

    for account in &wallet.accounts {
        if account.chain_id == chain_id && !account.address.is_empty() {
            return Ok(account.address.clone());
        }
    }

    if chain_id.starts_with("eip155:") {
        for account in &wallet.accounts {
            if account.chain_id.starts_with("eip155:") && !account.address.is_empty() {
                return Ok(account.address.clone());
            }
        }
    }

    Err(Error::new(
        Code::Usage,
        format!(
            "wallet {:?} has no account for chain {:?}",
            wallet.id, chain_id
        ),
    ))
}

/// Load all wallet metadata files from a resolved vault directory.
///
/// Helper exposed for tests that write fixtures and assert decoding. Reads
/// `<vault_path>/wallets/*.json` (lexicographic glob order, like Go's
/// `filepath.Glob`).
pub fn load_wallets(vault_path: &Path) -> Result<Vec<Wallet>, Error> {
    let wallets_dir = vault_path.join("wallets");

    // Collect `*.json` entries. A missing directory yields no matches, mirroring
    // Go's `filepath.Glob` (which returns an empty slice for a non-existent dir).
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(&wallets_dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => return Err(Error::wrap(Code::Internal, "list wallet metadata", err)),
    };
    // `filepath.Glob` returns lexicographically sorted matches; replicate that
    // so wallet ordering (used for ambiguity detection) is deterministic.
    paths.sort();

    let mut wallets = Vec::with_capacity(paths.len());
    for path in paths {
        let data = std::fs::read(&path).map_err(|err| {
            Error::wrap(
                Code::Internal,
                format!("read wallet metadata {}", path.display()),
                err,
            )
        })?;
        let wallet: Wallet = serde_json::from_slice(&data).map_err(|err| {
            Error::wrap(
                Code::Internal,
                format!("decode wallet metadata {}", path.display()),
                err,
            )
        })?;
        wallets.push(wallet);
    }
    Ok(wallets)
}

// =============================================================================
// SUCCESS CRITERIA (RED phase — tests written before implementation)
//
// This module (Go source: internal/ows) owns the Open Wallet Standard backend
// client: shelling out to the `ows` CLI to sign + broadcast EVM transactions,
// and reading local OWS vault wallet metadata. The Rust port is "correct" iff:
//
//  A. send_unsigned_tx — input validation & exit-code mapping (spec §2.2):
//     1. blank wallet_id   -> Err(Code::Usage)        ("wallet id is required")
//     2. blank chain_id    -> Err(Code::Usage)        ("chain id is required")
//     3. empty tx_bytes    -> Err(Code::Usage)        ("unsigned tx bytes ...")
//     4. `ows` not on PATH -> Err(Code::Unavailable)  ("ows CLI not found ...")
//     5. missing/blank token -> Err(Code::Signer)     ("missing DEFI_OWS_TOKEN")
//     (Go uses CodeUsage / CodeUnavailable / CodeSigner respectively.)
//
//  B. send_unsigned_tx — command construction (ported from
//     TestSendUnsignedTxBuildsOwsCommand):
//     6. looks up the binary named exactly "ows".
//     7. arg vector is EXACTLY, in order:
//          sign send-tx --wallet <id> --chain <chain> --tx 0x<hex> --json
//          [--rpc-url <url>]   (rpc-url only when non-empty, appended LAST)
//        tx hex is lowercase hex of the raw bytes, 0x-prefixed (0x010203 for
//        [1,2,3]).
//     8. the child env includes OWS_PASSPHRASE=<token>.
//     9. on success the parsed SendTxResult.chain falls back to the requested
//        chain_id when the CLI omits `chain`.
//
//  C. send_unsigned_tx — failure classification (ported from
//     TestSendUnsignedTxMapsPolicyDenial + ...MapsPolicyDeniedCodeStyle):
//    10. a non-zero exit whose stderr/stdout signals a policy denial
//        ("policy denied by wallet policy", or a JSON body containing
//        "POLICY_DENIED") -> Err(Code::ActionPolicy).
//    11. any other non-zero exit -> Err(Code::Signer).
//    12. a malformed tx hash in an otherwise-successful response ->
//        Err(Code::Signer) (ported from TestSendUnsignedTxRejectsMalformedTxHash).
//
//  D. parse_send_tx_result (ported from TestParseSendTxResultRejectsMalformedTxHash):
//    13. prefers snake `tx_hash`; falls back to camel `txHash`.
//    14. rejects a malformed hash ("0xabc123") with an error.
//    15. missing chain in the body falls back to the supplied fallback chain.
//
//  E. is_tx_hash (Go IsTxHash) — fresh spec-driven boundary tests:
//    16. accepts a 0x + 64 lowercase/uppercase hex char string (len 66).
//    17. rejects: missing 0x prefix, wrong length, non-hex chars,
//        surrounding whitespace is trimmed before the length check passes only
//        for a clean 66-char core.
//
//  F. resolve_wallet_ref (ported from TestResolveWalletRefByID / ByName /
//     RejectsAmbiguousName):
//    18. blank reference -> error.
//    19. exact `id` match returns that wallet even when another wallet shares
//        the same `name` (id takes precedence over name).
//    20. no id match but a single `name` match returns it.
//    21. duplicate `name` -> ambiguous error (no match panics nothing; returns
//        Err).
//    22. reference matching nothing -> Err.
//
//  G. sender_address_for_chain (ported from
//     TestResolveWalletSenderAddressUsesEVMAccount /
//     ...FailsWithoutMatchingFamily):
//    23. exact chain_id match wins.
//    24. eip155:* request with no exact match falls back to ANY eip155:* account
//        (e.g. request eip155:8453, only eip155:1 present -> eip155:1's address).
//    25. eip155:* request with only a non-eip155 account (e.g. solana:*) -> Err.
//    26. blank chain_id -> Err.
//
//  H. JSON contract (spec §2.3): SendTxResult serializes with field DECLARATION
//     order tx_hash, chain; Wallet/WalletAccount round-trip with their snake_case
//     JSON keys (id, name, created_at, accounts / account_id, address, chain_id,
//     derivation_path).
//
// Test-mapping notes:
//  - The Go tests use mutable package-level seams (lookPathFunc/runCommandFunc).
//    The idiomatic Rust port injects a `CommandRunner` trait object instead, so
//    these tests use an in-test `FakeRunner` rather than swapping globals.
//  - There is NO httptest server in this Go module (it shells out to a CLI), so
//    wiremock does not apply; the subprocess seam is the correct analogue.
//  - SKIPPED Go internals: `runCommand` (thin os/exec wrapper) and
//    `classifyCommandFailure`/`isPolicyDeniedDetail` as standalone symbols — they
//    are exercised end-to-end through send_unsigned_tx's classification tests
//    (criteria 10/11), which is the meaningful contract, not the helper shape.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // ---- Test harness: an injectable fake CommandRunner ------------------

    /// Records what the unit asked for and replays a scripted response.
    struct FakeRunner {
        /// `Some(path)` to resolve `ows` to; `None` to simulate "not on PATH".
        look_path: Option<String>,
        output: CommandOutput,
        captured: RefCell<Option<Captured>>,
    }

    #[derive(Clone, Debug)]
    struct Captured {
        bin: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    }

    impl FakeRunner {
        fn ok(stdout: &str) -> Self {
            FakeRunner {
                look_path: Some("/usr/local/bin/ows".to_string()),
                output: CommandOutput {
                    stdout: stdout.as_bytes().to_vec(),
                    stderr: Vec::new(),
                    run_error: None,
                },
                captured: RefCell::new(None),
            }
        }

        fn failing(stderr: &str) -> Self {
            FakeRunner {
                look_path: Some("/usr/local/bin/ows".to_string()),
                output: CommandOutput {
                    stdout: Vec::new(),
                    stderr: stderr.as_bytes().to_vec(),
                    run_error: Some("exit status 1".to_string()),
                },
                captured: RefCell::new(None),
            }
        }

        fn missing_binary() -> Self {
            FakeRunner {
                look_path: None,
                output: CommandOutput::default(),
                captured: RefCell::new(None),
            }
        }

        fn captured(&self) -> Captured {
            self.captured
                .borrow()
                .clone()
                .expect("runner.run should have been called")
        }
    }

    impl CommandRunner for FakeRunner {
        fn look_path(&self, file: &str) -> Result<String, std::io::Error> {
            match &self.look_path {
                Some(p) => {
                    assert_eq!(file, "ows", "must look up the binary named exactly 'ows'");
                    Ok(p.clone())
                }
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "executable file not found in $PATH",
                )),
            }
        }

        fn run(&self, bin: &str, args: &[String], env: &[(String, String)]) -> CommandOutput {
            *self.captured.borrow_mut() = Some(Captured {
                bin: bin.to_string(),
                args: args.to_vec(),
                env: env.to_vec(),
            });
            self.output.clone()
        }
    }

    const VALID_HASH: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";

    fn assert_code(err: &Error, want: Code) {
        assert_eq!(err.code, want, "error: {err}");
    }

    // ---- Criterion group A: input validation -----------------------------

    #[test]
    fn send_rejects_blank_wallet_id() {
        let runner = FakeRunner::ok(&format!(r#"{{"tx_hash":"{VALID_HASH}"}}"#));
        let err = send_unsigned_tx(&runner, Some("pass"), "   ", "eip155:1", &[1, 2, 3], "")
            .expect_err("blank wallet id must fail");
        assert_code(&err, Code::Usage);
    }

    #[test]
    fn send_rejects_blank_chain_id() {
        let runner = FakeRunner::ok(&format!(r#"{{"tx_hash":"{VALID_HASH}"}}"#));
        let err = send_unsigned_tx(&runner, Some("pass"), "wallet-1", "", &[1, 2, 3], "")
            .expect_err("blank chain id must fail");
        assert_code(&err, Code::Usage);
    }

    #[test]
    fn send_rejects_empty_tx_bytes() {
        let runner = FakeRunner::ok(&format!(r#"{{"tx_hash":"{VALID_HASH}"}}"#));
        let err = send_unsigned_tx(&runner, Some("pass"), "wallet-1", "eip155:1", &[], "")
            .expect_err("empty tx bytes must fail");
        assert_code(&err, Code::Usage);
    }

    #[test]
    fn send_maps_missing_binary_to_unavailable() {
        let runner = FakeRunner::missing_binary();
        let err = send_unsigned_tx(&runner, Some("pass"), "wallet-1", "eip155:1", &[1], "")
            .expect_err("missing ows binary must fail");
        assert_code(&err, Code::Unavailable);
    }

    #[test]
    fn send_maps_missing_token_to_signer() {
        let runner = FakeRunner::ok(&format!(r#"{{"tx_hash":"{VALID_HASH}"}}"#));
        // No token (None) and blank token must both fail with Signer.
        let err = send_unsigned_tx(&runner, None, "wallet-1", "eip155:1", &[1], "")
            .expect_err("missing token must fail");
        assert_code(&err, Code::Signer);

        let err2 = send_unsigned_tx(&runner, Some("   "), "wallet-1", "eip155:1", &[1], "")
            .expect_err("blank token must fail");
        assert_code(&err2, Code::Signer);
    }

    // ---- Criterion group B: command construction --------------------------

    #[test]
    fn send_builds_exact_ows_command_with_rpc_url() {
        // Ported from TestSendUnsignedTxBuildsOwsCommand.
        let runner = FakeRunner::ok(&format!(r#"{{"txHash":"{VALID_HASH}"}}"#));
        let result = send_unsigned_tx(
            &runner,
            Some("test-passphrase"),
            "wallet-1",
            "eip155:1",
            &[0x01, 0x02, 0x03],
            "https://rpc.example",
        )
        .expect("send should succeed");

        assert_eq!(result.tx_hash, VALID_HASH);
        // chain falls back to the requested chain when CLI omits it.
        assert_eq!(result.chain, "eip155:1");

        let cap = runner.captured();
        assert_eq!(cap.bin, "/usr/local/bin/ows");
        let want_args: Vec<String> = [
            "sign",
            "send-tx",
            "--wallet",
            "wallet-1",
            "--chain",
            "eip155:1",
            "--tx",
            "0x010203",
            "--json",
            "--rpc-url",
            "https://rpc.example",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(cap.args, want_args, "exact ordered ows arg vector");

        assert!(
            cap.env
                .iter()
                .any(|(k, v)| k == "OWS_PASSPHRASE" && v == "test-passphrase"),
            "child env must inject OWS_PASSPHRASE; got {:?}",
            cap.env
        );
    }

    #[test]
    fn send_omits_rpc_url_arg_when_blank() {
        let runner = FakeRunner::ok(&format!(r#"{{"tx_hash":"{VALID_HASH}"}}"#));
        send_unsigned_tx(
            &runner,
            Some("pass"),
            "wallet-1",
            "eip155:1",
            &[0xab],
            "   ",
        )
        .expect("send should succeed");
        let cap = runner.captured();
        assert!(
            !cap.args.iter().any(|a| a == "--rpc-url"),
            "blank rpc-url must not add the flag; got {:?}",
            cap.args
        );
        // last arg is --json when no rpc-url.
        assert_eq!(cap.args.last().map(String::as_str), Some("--json"));
    }

    // ---- Criterion group C: failure classification -----------------------

    #[test]
    fn send_maps_plain_policy_denial_to_action_policy() {
        // Ported from TestSendUnsignedTxMapsPolicyDenial.
        let runner = FakeRunner::failing("policy denied by wallet policy");
        let err = send_unsigned_tx(&runner, Some("pass"), "wallet-1", "eip155:1", &[0x02], "")
            .expect_err("policy denial must fail");
        assert_code(&err, Code::ActionPolicy);
    }

    #[test]
    fn send_maps_policy_denied_code_style_to_action_policy() {
        // Ported from TestSendUnsignedTxMapsPolicyDeniedCodeStyle.
        let runner =
            FakeRunner::failing(r#"{"code":"POLICY_DENIED","message":"blocked by wallet policy"}"#);
        let err = send_unsigned_tx(&runner, Some("pass"), "wallet-1", "eip155:1", &[0x02], "")
            .expect_err("policy denial (code style) must fail");
        assert_code(&err, Code::ActionPolicy);
    }

    #[test]
    fn send_maps_other_command_failure_to_signer() {
        let runner = FakeRunner::failing("rpc endpoint unreachable");
        let err = send_unsigned_tx(&runner, Some("pass"), "wallet-1", "eip155:1", &[0x02], "")
            .expect_err("generic command failure must fail");
        assert_code(&err, Code::Signer);
    }

    #[test]
    fn send_rejects_malformed_tx_hash_with_signer() {
        // Ported from TestSendUnsignedTxRejectsMalformedTxHash: a clean exit but
        // an invalid tx hash in the body must map to Signer (parse failure).
        let runner = FakeRunner::ok(r#"{"txHash":"0xabc123"}"#);
        let err = send_unsigned_tx(&runner, Some("pass"), "wallet-1", "eip155:1", &[0x02], "")
            .expect_err("malformed tx hash must fail");
        assert_code(&err, Code::Signer);
    }

    // ---- build_send_tx_args (direct, to pin the contract) ----------------

    #[test]
    fn build_args_appends_rpc_url_last_when_present() {
        let args = build_send_tx_args("w", "eip155:10", &[0xde, 0xad], "https://r");
        let want: Vec<String> = [
            "sign",
            "send-tx",
            "--wallet",
            "w",
            "--chain",
            "eip155:10",
            "--tx",
            "0xdead",
            "--json",
            "--rpc-url",
            "https://r",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(args, want);
    }

    #[test]
    fn build_args_omits_rpc_url_when_blank() {
        let args = build_send_tx_args("w", "eip155:10", &[0x00], "");
        assert!(!args.contains(&"--rpc-url".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("--json"));
        // tx encoding is lowercase, 0x-prefixed, of the raw bytes.
        assert!(args.contains(&"0x00".to_string()));
    }

    // ---- Criterion group D: parse_send_tx_result -------------------------

    #[test]
    fn parse_prefers_snake_tx_hash() {
        let body = format!(r#"{{"tx_hash":"{VALID_HASH}","txHash":"0xdeadbeef"}}"#);
        let res = parse_send_tx_result(body.as_bytes(), "eip155:1").expect("parse ok");
        assert_eq!(res.tx_hash, VALID_HASH);
    }

    #[test]
    fn parse_falls_back_to_camel_tx_hash() {
        let body = format!(r#"{{"txHash":"{VALID_HASH}"}}"#);
        let res = parse_send_tx_result(body.as_bytes(), "eip155:1").expect("parse ok");
        assert_eq!(res.tx_hash, VALID_HASH);
    }

    #[test]
    fn parse_falls_back_to_supplied_chain_when_missing() {
        let body = format!(r#"{{"tx_hash":"{VALID_HASH}"}}"#);
        let res = parse_send_tx_result(body.as_bytes(), "eip155:8453").expect("parse ok");
        assert_eq!(res.chain, "eip155:8453");
    }

    #[test]
    fn parse_keeps_response_chain_over_fallback() {
        let body = format!(r#"{{"tx_hash":"{VALID_HASH}","chain":"eip155:137"}}"#);
        let res = parse_send_tx_result(body.as_bytes(), "eip155:1").expect("parse ok");
        assert_eq!(res.chain, "eip155:137");
    }

    #[test]
    fn parse_rejects_malformed_tx_hash() {
        // Ported from TestParseSendTxResultRejectsMalformedTxHash.
        let err = parse_send_tx_result(br#"{"txHash":"0xabc123"}"#, "eip155:1")
            .expect_err("malformed tx hash must fail");
        // The malformed-hash branch is signer-coded (mirrors Go fmt.Errorf wrapped
        // as CodeSigner by the caller; parse_send_tx_result itself emits Signer).
        assert_code(&err, Code::Signer);
    }

    #[test]
    fn parse_rejects_missing_tx_hash() {
        let err = parse_send_tx_result(br#"{"chain":"eip155:1"}"#, "eip155:1")
            .expect_err("missing tx hash must fail");
        assert_code(&err, Code::Signer);
    }

    #[test]
    fn parse_rejects_non_json_body() {
        // A non-JSON CLI response is a decode failure, also signer-coded.
        let err = parse_send_tx_result(b"not json at all", "eip155:1")
            .expect_err("non-json body must fail");
        assert_code(&err, Code::Signer);
    }

    // ---- Criterion group E: is_tx_hash boundaries ------------------------

    #[test]
    fn is_tx_hash_accepts_valid_64_nibble_hash() {
        assert!(is_tx_hash(VALID_HASH));
        // uppercase hex is also valid.
        assert!(is_tx_hash(
            "0xABCDEF0000000000000000000000000000000000000000000000000000000000"
        ));
    }

    #[test]
    fn is_tx_hash_rejects_bad_inputs() {
        assert!(!is_tx_hash("0xabc123"), "too short");
        assert!(
            !is_tx_hash("1111111111111111111111111111111111111111111111111111111111111111"),
            "missing 0x prefix"
        );
        assert!(!is_tx_hash(&format!("{VALID_HASH}00")), "too long");
        assert!(
            !is_tx_hash("0xZZ11111111111111111111111111111111111111111111111111111111111111"),
            "non-hex chars"
        );
        assert!(!is_tx_hash(""), "empty");
    }

    // ---- Criterion group F: resolve_wallet_ref ---------------------------

    fn write_wallet_fixture(vault_dir: &Path, wallet: &Wallet) {
        let wallets_dir = vault_dir.join("wallets");
        std::fs::create_dir_all(&wallets_dir).expect("mkdir wallets");
        let path = wallets_dir.join(format!("{}.json", wallet.id));
        let data = serde_json::to_vec_pretty(wallet).expect("marshal wallet");
        std::fs::write(path, data).expect("write wallet fixture");
    }

    fn evm_account(addr: &str, chain: &str) -> WalletAccount {
        WalletAccount {
            account_id: "account-1".to_string(),
            address: addr.to_string(),
            chain_id: chain.to_string(),
            derivation_path: "m/44'/60'/0'/0/0".to_string(),
        }
    }

    #[test]
    fn resolve_rejects_blank_reference() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_wallet_ref(dir.path().to_str().unwrap(), "   ")
            .expect_err("blank ref must fail");
        assert_code(&err, Code::Usage);
    }

    #[test]
    fn resolve_by_id_takes_precedence_over_name() {
        // Ported from TestResolveWalletRefByID: two wallets share name "alice";
        // resolving by id "wallet-123" must return that exact wallet.
        let dir = tempfile::tempdir().unwrap();
        write_wallet_fixture(
            dir.path(),
            &Wallet {
                id: "wallet-123".to_string(),
                name: "alice".to_string(),
                created_at: "2026-03-25T00:00:00Z".to_string(),
                accounts: vec![evm_account(
                    "0x000000000000000000000000000000000000dEaD",
                    "eip155:1",
                )],
            },
        );
        write_wallet_fixture(
            dir.path(),
            &Wallet {
                id: "wallet-999".to_string(),
                name: "alice".to_string(),
                created_at: "2026-03-25T00:00:00Z".to_string(),
                accounts: vec![],
            },
        );

        let got =
            resolve_wallet_ref(dir.path().to_str().unwrap(), "wallet-123").expect("resolve by id");
        assert_eq!(got.id, "wallet-123");
        assert_eq!(got.name, "alice");
    }

    #[test]
    fn resolve_by_name_when_no_id_match() {
        // Ported from TestResolveWalletRefByName.
        let dir = tempfile::tempdir().unwrap();
        write_wallet_fixture(
            dir.path(),
            &Wallet {
                id: "wallet-123".to_string(),
                name: "alice".to_string(),
                created_at: "2026-03-25T00:00:00Z".to_string(),
                accounts: vec![],
            },
        );
        let got =
            resolve_wallet_ref(dir.path().to_str().unwrap(), "alice").expect("resolve by name");
        assert_eq!(got.id, "wallet-123");
    }

    #[test]
    fn resolve_rejects_ambiguous_name() {
        // Ported from TestResolveWalletRefRejectsAmbiguousName.
        let dir = tempfile::tempdir().unwrap();
        write_wallet_fixture(
            dir.path(),
            &Wallet {
                id: "wallet-1".to_string(),
                name: "alice".to_string(),
                created_at: "2026-03-25T00:00:00Z".to_string(),
                accounts: vec![],
            },
        );
        write_wallet_fixture(
            dir.path(),
            &Wallet {
                id: "wallet-2".to_string(),
                name: "alice".to_string(),
                created_at: "2026-03-25T00:00:01Z".to_string(),
                accounts: vec![],
            },
        );
        let err = resolve_wallet_ref(dir.path().to_str().unwrap(), "alice")
            .expect_err("ambiguous name must fail");
        assert_code(&err, Code::Usage);
        assert!(
            err.to_string().contains("ambiguous wallet name"),
            "ambiguous-name message must surface the reason: {err}"
        );
    }

    #[test]
    fn resolve_rejects_unknown_reference() {
        let dir = tempfile::tempdir().unwrap();
        write_wallet_fixture(
            dir.path(),
            &Wallet {
                id: "wallet-1".to_string(),
                name: "alice".to_string(),
                created_at: "2026-03-25T00:00:00Z".to_string(),
                accounts: vec![],
            },
        );
        let err = resolve_wallet_ref(dir.path().to_str().unwrap(), "nobody")
            .expect_err("unknown ref must fail");
        assert_code(&err, Code::Usage);
        assert!(
            err.to_string().contains("not found"),
            "unknown-ref message must say not found: {err}"
        );
    }

    // ---- Criterion group G: sender_address_for_chain ---------------------

    #[test]
    fn sender_falls_back_to_any_evm_account() {
        // Ported from TestResolveWalletSenderAddressUsesEVMAccount: requesting
        // eip155:8453 with only a solana account + an eip155:1 account returns
        // the eip155:1 address (family fallback).
        let wallet = Wallet {
            id: "wallet-123".to_string(),
            name: "alice".to_string(),
            created_at: String::new(),
            accounts: vec![
                WalletAccount {
                    account_id: "account-1".to_string(),
                    address: "0x000000000000000000000000000000000000dEaD".to_string(),
                    chain_id: "solana:mainnet".to_string(),
                    derivation_path: "m/44'/501'/0'/0'".to_string(),
                },
                WalletAccount {
                    account_id: "account-2".to_string(),
                    address: "0x1111111111111111111111111111111111111111".to_string(),
                    chain_id: "eip155:1".to_string(),
                    derivation_path: "m/44'/60'/0'/0/0".to_string(),
                },
            ],
        };
        let got = sender_address_for_chain(&wallet, "eip155:8453").expect("evm fallback");
        assert_eq!(got, "0x1111111111111111111111111111111111111111");
    }

    #[test]
    fn sender_prefers_exact_chain_match() {
        let wallet = Wallet {
            id: "w".to_string(),
            name: "n".to_string(),
            created_at: String::new(),
            accounts: vec![
                evm_account("0xAAAA000000000000000000000000000000000001", "eip155:1"),
                evm_account("0xBBBB000000000000000000000000000000000002", "eip155:8453"),
            ],
        };
        let got = sender_address_for_chain(&wallet, "eip155:8453").expect("exact match");
        assert_eq!(got, "0xBBBB000000000000000000000000000000000002");
    }

    #[test]
    fn sender_skips_exact_match_with_empty_address() {
        // Go requires `account.Address != ""` even on an exact chain match, so an
        // exact-chain account with a blank address must NOT be returned; the EVM
        // family fallback should pick the next populated eip155 account instead.
        let wallet = Wallet {
            id: "w".to_string(),
            name: "n".to_string(),
            created_at: String::new(),
            accounts: vec![
                evm_account("", "eip155:8453"),
                evm_account("0xCCCC000000000000000000000000000000000003", "eip155:1"),
            ],
        };
        let got = sender_address_for_chain(&wallet, "eip155:8453")
            .expect("blank exact-match address must be skipped");
        assert_eq!(got, "0xCCCC000000000000000000000000000000000003");
    }

    #[test]
    fn sender_fails_without_matching_family() {
        // Ported from TestResolveWalletSenderAddressFailsWithoutMatchingFamily.
        let wallet = Wallet {
            id: "wallet-123".to_string(),
            name: "alice".to_string(),
            created_at: String::new(),
            accounts: vec![WalletAccount {
                account_id: "account-1".to_string(),
                address: "So11111111111111111111111111111111111111112".to_string(),
                chain_id: "solana:mainnet".to_string(),
                derivation_path: "m/44'/501'/0'/0'".to_string(),
            }],
        };
        let err = sender_address_for_chain(&wallet, "eip155:1")
            .expect_err("no matching family must fail");
        assert_code(&err, Code::Usage);
    }

    #[test]
    fn sender_rejects_blank_chain_id() {
        let wallet = Wallet {
            id: "w".to_string(),
            name: "n".to_string(),
            created_at: String::new(),
            accounts: vec![evm_account(
                "0xAAAA000000000000000000000000000000000001",
                "eip155:1",
            )],
        };
        let err = sender_address_for_chain(&wallet, "  ").expect_err("blank chain must fail");
        assert_code(&err, Code::Usage);
    }

    // ---- Criterion group H: JSON contract --------------------------------

    #[test]
    fn send_tx_result_serializes_in_declaration_order() {
        let r = SendTxResult {
            tx_hash: VALID_HASH.to_string(),
            chain: "eip155:1".to_string(),
        };
        let json = serde_json::to_string(&r).unwrap();
        let tx_pos = json.find("\"tx_hash\"").expect("tx_hash present");
        let chain_pos = json.find("\"chain\"").expect("chain present");
        assert!(tx_pos < chain_pos, "tx_hash must precede chain: {json}");
    }

    #[test]
    fn wallet_round_trips_with_snake_case_keys() {
        let json = r#"{
            "id": "wallet-1",
            "name": "alice",
            "created_at": "2026-03-25T00:00:00Z",
            "accounts": [
                {
                    "account_id": "account-1",
                    "address": "0x1111111111111111111111111111111111111111",
                    "chain_id": "eip155:1",
                    "derivation_path": "m/44'/60'/0'/0/0"
                }
            ]
        }"#;
        let wallet: Wallet = serde_json::from_str(json).expect("decode wallet");
        assert_eq!(wallet.id, "wallet-1");
        assert_eq!(wallet.name, "alice");
        assert_eq!(wallet.created_at, "2026-03-25T00:00:00Z");
        assert_eq!(wallet.accounts.len(), 1);
        assert_eq!(wallet.accounts[0].account_id, "account-1");
        assert_eq!(wallet.accounts[0].chain_id, "eip155:1");
        assert_eq!(wallet.accounts[0].derivation_path, "m/44'/60'/0'/0/0");
    }

    #[test]
    fn wallet_decodes_without_accounts_field() {
        // Go's loadWallets tolerates wallets with no accounts array.
        let wallet: Wallet =
            serde_json::from_str(r#"{"id":"w","name":"n","created_at":"t"}"#).expect("decode");
        assert!(wallet.accounts.is_empty());
    }
}
