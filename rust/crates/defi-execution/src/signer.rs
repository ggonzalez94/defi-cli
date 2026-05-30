//! Signer abstraction: key-source resolution (local) + Tempo smart-wallet signer.
//!
//! Go source: `internal/execution/signer/{signer.go,local.go,tempo.go}`.
//!
//! ## Scope split (why this module is *not* the crypto core)
//!
//! The **pure crypto + EVM-tx primitives** — parse a hex secp256k1 key, derive
//! its EIP-55 address, and sign an EIP-1559 transaction so it recovers to that
//! address and binds the chain id — already live in [`defi_evm::signer`]
//! ([`defi_evm::signer::LocalSigner`]). The Go `internal/execution/signer`
//! package layered three further concerns on top of go-ethereum's `crypto` /
//! `core/types`, and *those* are what this module owns:
//!
//! 1. **Local key-source orchestration** (`local.go`): the
//!    `flags > env > file > defaults` precedence over a private key —
//!    `DEFI_PRIVATE_KEY` (hex) > `DEFI_PRIVATE_KEY_FILE` > an auto-discovered
//!    `~/.config/defi/key.hex` (XDG-aware) > a V3 keystore
//!    (`DEFI_KEYSTORE_PATH` + `DEFI_KEYSTORE_PASSWORD`/`…_FILE`), the
//!    `--private-key` override that beats every source, path normalization, the
//!    `auto|env|file|keystore` key-source selector, and the missing-key usage
//!    hint. This produces a **hex key string** that is then handed to
//!    [`defi_evm::signer::LocalSigner::from_hex`] for the actual crypto.
//!
//! 2. **Tempo smart-wallet signer** (`tempo.go`): `TempoWalletSigner` —
//!    a signing-key EOA whose on-chain sender is a *different* smart-wallet
//!    address; signs Tempo type-0x76 transactions (recoverable to the key EOA),
//!    refuses standard EVM `SignTx`, and reports `None` for the raw-EVM
//!    private-key accessor (the key is owned by the Tempo signer).
//!
//! 3. **Tempo CLI discovery** (`tempo.go`): `NewTempoSignerFromCLI` parses
//!    `tempo wallet -j whoami` JSON, rejects a not-ready / expired wallet, and
//!    surfaces a near-expiry warning. The shell-out itself is bespoke (spec §7);
//!    the parse + readiness/expiry decision is what carries contract weight and
//!    is tested here through an injectable whoami source.
//!
//! ===========================================================================
//! SUCCESS CRITERIA (RED phase — written before implementation; tests below
//! MUST fail to compile / assert until GREEN). The Rust port of this module is
//! "correct" iff:
//! ===========================================================================
//!
//! ### A. Key-source selector parity (`KeySourceAuto|Env|File|Keystore`)
//! A1. [`KeySource::parse`] is **case-insensitive** and trims surrounding
//!     whitespace (Go: `strings.ToLower(strings.TrimSpace(source))`), mapping
//!     `"auto"|"env"|"file"|"keystore"` (and `"AUTO"`, `"  File  "`, …) to the
//!     matching variant.
//! A2. An **empty / whitespace-only** source defaults to [`KeySource::Auto`]
//!     (Go: `if source == "" { source = KeySourceAuto }`).
//! A3. Any other value is an `Err` whose message names the four valid sources
//!     (Go: `unsupported key source %q (expected auto|env|file|keystore)`),
//!     typed [`defi_errors::Code::Usage`].
//!
//! ### B. Local key-source precedence (the `flags > env > file > defaults` core)
//! B1. **Env hex wins**: with `DEFI_PRIVATE_KEY` set, `Env` source resolves to
//!     that hex and produces a signer whose address is non-zero (Go:
//!     `TestNewLocalSignerFromEnvHex`). The resolved address equals the
//!     `defi_evm` derivation for that key.
//! B2. **Env file**: with `DEFI_PRIVATE_KEY_FILE` pointing at a file containing
//!     a hex key, `File` source reads + trims it and resolves a non-zero signer
//!     (Go: `TestNewLocalSignerFromEnvFile`). File **permissions are not
//!     enforced** — a `0o644` key file still loads (Go:
//!     `…FileAllowsNonStrictPermissions`).
//! B3. **Auto uses the default key file**: with no env hex/file/keystore set but
//!     a key present at `$XDG_CONFIG_HOME/defi/key.hex`, `Auto` source discovers
//!     and loads it (Go: `TestNewLocalSignerFromEnvAutoUsesDefaultKeyFile`).
//! B4. **`--private-key` override beats everything**: a non-empty override
//!     resolves to that key even under `File` source with a bogus
//!     `DEFI_PRIVATE_KEY_FILE`, and even when env hex is set (Go:
//!     `TestNewLocalSignerFromInputsPrivateKeyOverride`,
//!     `…OverrideWinsOverFileSource`).
//! B5. **Source isolation**: `Env` source ignores file/keystore inputs; `File`
//!     source ignores env-hex/keystore; `Keystore` source ignores
//!     env-hex/file. (Go: the per-source clearing in `NewLocalSignerFromInputs`.)
//! B6. **Missing-key error**: with no key available, resolution fails with a
//!     [`defi_errors::Code::Usage`] error whose message contains BOTH the
//!     `--private-key` hint AND the simple default path hint
//!     `~/.config/defi/key.hex` (Go:
//!     `…MissingKeyErrorIncludesSimplePathHint`).
//!
//! ### C. Default key path resolution
//! C1. [`default_private_key_path`] honors `XDG_CONFIG_HOME` first:
//!     `XDG_CONFIG_HOME=/tmp/x` → `/tmp/x/defi/key.hex` (Go:
//!     `TestDefaultPrivateKeyPathUsesXDGConfigHome`).
//! C2. Falls back to `<home>/.config/defi/key.hex` when `XDG_CONFIG_HOME`
//!     is unset; `None` when neither XDG nor home is resolvable.
//! C3. Auto discovery returns the path only when a **regular file** exists there
//!     (a directory at that path is ignored — Go `discoverDefaultPrivateKeyFile`
//!     skips `info.IsDir()`).
//!
//! ### D. `TempoWalletSigner` (smart-wallet ≠ key EOA)
//! D1. [`TempoWalletSigner::new`] accepts a hex key with optional `0x`/`0X`
//!     prefix + whitespace (Go: `TrimPrefix(TrimSpace,"0x")`), derives the key
//!     EOA address, and stores the provided wallet address. (Go:
//!     `TestNewTempoWalletSigner`.)
//! D2. `wallet_address()` is the on-chain sender; `address()` is the signing-key
//!     EOA; for an arbitrary wallet address they **differ** (Go:
//!     `…WalletAddressDiffersFromKeyAddress`).
//! D3. [`TempoWalletSigner::sign_tempo_tx`] attaches a signature; the signature
//!     **recovers to the key EOA** `address()` (Go: `…SignTempoTx` +
//!     `VerifySignature`).
//! D4. Standard EVM signing is **rejected**: `sign_evm_tx` returns an `Err`
//!     ([`defi_errors::Code::Unsupported`] — "use SignTempoTx for Tempo chains")
//!     (Go: `…RejectsEVMSignTx`).
//! D5. [`TempoWalletSigner::private_key_hex`] returns `None` — the raw EVM key
//!     accessor is not exposed (the key is owned by the Tempo signer; Go:
//!     `PrivateKey()` returns nil → `…PrivateKeyReturnsNil`).
//! D6. An invalid key is rejected with a typed [`defi_errors::Code::Signer`]
//!     error (Go: `…RejectsInvalidKey`).
//!
//! ### E. Tempo CLI whoami parse + readiness/expiry decision
//! E1. A `ready: true` whoami with a future `expires_at` → a configured
//!     `TempoWalletSigner` (wallet = `wallet`, key = `key.key`) and **no**
//!     warnings.
//! E2. `ready: false` → an `Err` (not logged in) — typed
//!     [`defi_errors::Code::Signer`].
//! E3. An `expires_at` in the **past** → an `Err` (expired key).
//! E4. An `expires_at` < 24h away → success **with** a near-expiry warning
//!     string mentioning expiry.
//! E5. Malformed JSON → an `Err` (parse failure), never a panic.
//!
//! ## Ported Go test cases (and what is intentionally SKIPPED here)
//! - `local_test.go`: the env/file/auto/override/missing-key/default-path cases
//!   are ported (criteria A–C) — but re-expressed against an **injected `Env`**
//!   ([`defi_config::Env`] / [`defi_config::MapEnv`]) instead of `t.Setenv`,
//!   because Rust tests share one process and run in parallel, so a global env
//!   would be racy. The injected-env precedence contract is the real behavior
//!   this module owns; reading process-global `getenv` is not.
//! - `local_test.go`'s pure crypto assertion (`SignTx succeeds`) is owned by
//!   [`defi_evm::signer`] and is SKIPPED here (no duplicate crypto vectors).
//! - V3-keystore *decryption* itself is delegated; here we only assert that the
//!   `Keystore` source path is selected/isolated and that a missing
//!   password/file is a typed error (the scrypt/aes-128-ctr decryption parity
//!   is a `defi-evm`/dedicated-keystore concern, not key-source orchestration).
//! - `tempo_test.go`: all ported (criteria D) except the exact tempo-go
//!   `transaction.Tx` builder/RLP encoding, which is bespoke and owned by
//!   [`crate::tempo_executor`]; here the contract is the *signer* behavior
//!   (addresses, recover-to-key, reject EVM, no raw key).

use std::path::PathBuf;

use alloy::primitives::{keccak256, Signature, U256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;
use defi_config::Env;
use defi_errors::{Code, Error};
use defi_evm::address::Address;
use defi_evm::signer::{Eip1559Tx, LocalSigner, SignedTx};

// =============================================================================
// Environment-variable names + key-source selector values.
//
// Parity with the Go `internal/execution/signer` package constants
// (`EnvPrivateKey`, `KeySourceAuto`, …). These names are the env-var contract a
// caller sets to feed a private key into local-signer resolution.
// =============================================================================

/// `DEFI_PRIVATE_KEY` — a hex secp256k1 private key (highest env precedence).
pub const ENV_PRIVATE_KEY: &str = "DEFI_PRIVATE_KEY";
/// `DEFI_PRIVATE_KEY_FILE` — path to a file holding a hex private key.
pub const ENV_PRIVATE_KEY_FILE: &str = "DEFI_PRIVATE_KEY_FILE";
/// `DEFI_KEYSTORE_PATH` — path to a V3 keystore JSON file.
pub const ENV_KEYSTORE_PATH: &str = "DEFI_KEYSTORE_PATH";
/// `DEFI_KEYSTORE_PASSWORD` — the keystore decryption password.
pub const ENV_KEYSTORE_PASSWORD: &str = "DEFI_KEYSTORE_PASSWORD";
/// `DEFI_KEYSTORE_PASSWORD_FILE` — path to a file holding the keystore password.
pub const ENV_KEYSTORE_PASSWORD_FILE: &str = "DEFI_KEYSTORE_PASSWORD_FILE";

/// `defi/key.hex` relative to `$XDG_CONFIG_HOME` (or `~/.config`).
const DEFAULT_PRIVATE_KEY_RELATIVE_PATH: &str = "defi/key.hex";
/// The simple default-path hint surfaced in the missing-key usage error.
const DEFAULT_PRIVATE_KEY_HINT_PATH: &str = "~/.config/defi/key.hex";

/// The local key-source selector (`auto|env|file|keystore`).
///
/// Parity with the Go `KeySource*` constants: chooses which key inputs are
/// honored. `Auto` keeps every input and lets [`resolve_private_key_hex`] apply
/// the `env-hex > env-file/default-file > keystore` precedence; each explicit
/// source isolates its own input class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// Honor every input in precedence order (`env hex > file > keystore`).
    Auto,
    /// Only the `DEFI_PRIVATE_KEY` hex env var.
    Env,
    /// Only a key file (`DEFI_PRIVATE_KEY_FILE` or the auto-discovered default).
    File,
    /// Only a V3 keystore (`DEFI_KEYSTORE_PATH` + password).
    Keystore,
}

impl KeySource {
    /// Parse a key-source selector, parity with Go
    /// `strings.ToLower(strings.TrimSpace(source))` + the `switch`.
    ///
    /// Case-insensitive and whitespace-trimmed. An empty/whitespace-only value
    /// defaults to [`KeySource::Auto`]. Any other value is a
    /// [`Code::Usage`] error naming the four valid sources.
    pub fn parse(source: &str) -> Result<KeySource, Error> {
        let normalized = source.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "" | "auto" => Ok(KeySource::Auto),
            "env" => Ok(KeySource::Env),
            "file" => Ok(KeySource::File),
            "keystore" => Ok(KeySource::Keystore),
            other => Err(Error::new(
                Code::Usage,
                format!("unsupported key source {other:?} (expected auto|env|file|keystore)"),
            )),
        }
    }
}

/// The hex private-key inputs resolved from the [`Env`] for a given
/// [`KeySource`], after applying source isolation + the `--private-key` override.
///
/// The Rust analogue of the local `LocalSignerConfig` the Go `loadPrivateKey`
/// consumes — but only the parts this module owns (keystore decryption is
/// delegated; here we just detect the keystore source + its missing-password
/// error). Field declaration order mirrors the Go config struct.
struct ResolvedKeyInputs {
    private_key_hex: String,
    private_key_file: String,
    keystore_path: String,
    keystore_password: String,
    keystore_password_file: String,
}

/// Read the raw key inputs for `source` from `env`, apply per-source isolation
/// and the `--private-key` override, mirroring Go `NewLocalSignerFromInputs`.
fn resolve_key_inputs(
    source: KeySource,
    private_key_override: &str,
    env: &dyn Env,
) -> ResolvedKeyInputs {
    let mut private_key_hex = trimmed_var(env, ENV_PRIVATE_KEY);
    let mut private_key_file = trimmed_var(env, ENV_PRIVATE_KEY_FILE);
    let mut keystore_path = trimmed_var(env, ENV_KEYSTORE_PATH);
    let mut keystore_password = trimmed_var(env, ENV_KEYSTORE_PASSWORD);
    let mut keystore_password_file = trimmed_var(env, ENV_KEYSTORE_PASSWORD_FILE);

    // No explicit key file → fall back to the auto-discovered default file.
    if private_key_file.is_empty() {
        if let Some(path) = discover_default_private_key_file(env) {
            private_key_file = path.to_string_lossy().into_owned();
        }
    }

    match source {
        // Keep every input; precedence is applied in `load_private_key_hex`.
        KeySource::Auto => {}
        KeySource::Env => {
            private_key_file.clear();
            keystore_path.clear();
            keystore_password.clear();
            keystore_password_file.clear();
        }
        KeySource::File => {
            private_key_hex.clear();
            keystore_path.clear();
            keystore_password.clear();
            keystore_password_file.clear();
        }
        KeySource::Keystore => {
            private_key_hex.clear();
            private_key_file.clear();
        }
    }

    // `--private-key` beats every source.
    let override_trimmed = private_key_override.trim();
    if !override_trimmed.is_empty() {
        private_key_hex = override_trimmed.to_string();
        private_key_file.clear();
        keystore_path.clear();
        keystore_password.clear();
        keystore_password_file.clear();
    }

    ResolvedKeyInputs {
        private_key_hex,
        private_key_file,
        keystore_path,
        keystore_password,
        keystore_password_file,
    }
}

/// Resolve the hex private key string for `source` from `env`, parity with the
/// Go `NewLocalSignerFromInputs` → `loadPrivateKey` precedence
/// (`env hex > key file > keystore`), with the `--private-key` override winning
/// over everything.
///
/// Returns the trimmed hex key (any `0x` prefix preserved as read). On a missing
/// key it returns a [`Code::Usage`] error whose message cites both
/// `--private-key` and the simple default-path hint `~/.config/defi/key.hex`.
///
/// Keystore *decryption* is delegated (it is a `defi-evm`/dedicated-keystore
/// concern); here a keystore source with no password is surfaced as a typed
/// error, but a fully configured keystore is not decrypted in this module.
pub fn resolve_private_key_hex(
    source: KeySource,
    private_key_override: &str,
    env: &dyn Env,
) -> Result<String, Error> {
    let inputs = resolve_key_inputs(source, private_key_override, env);
    load_private_key_hex(&inputs)
}

/// Apply the `env hex > key file > keystore` precedence to resolved inputs and
/// return a hex private-key string (parity with Go `loadPrivateKey`).
fn load_private_key_hex(inputs: &ResolvedKeyInputs) -> Result<String, Error> {
    if !inputs.private_key_hex.trim().is_empty() {
        return Ok(inputs.private_key_hex.trim().to_string());
    }
    if !inputs.private_key_file.trim().is_empty() {
        let contents = std::fs::read_to_string(inputs.private_key_file.trim())
            .map_err(|e| Error::wrap(Code::Usage, "read private key file", io_cause(e)))?;
        let key = contents.trim();
        if key.is_empty() {
            return Err(Error::new(Code::Usage, "empty private key"));
        }
        return Ok(key.to_string());
    }
    if !inputs.keystore_path.trim().is_empty() {
        // Keystore decryption is delegated; this module only owns the
        // source-selection + missing-password contract.
        let mut password = inputs.keystore_password.trim().to_string();
        if password.is_empty() && !inputs.keystore_password_file.trim().is_empty() {
            let contents =
                std::fs::read_to_string(inputs.keystore_password_file.trim()).map_err(|e| {
                    Error::wrap(Code::Usage, "read keystore password file", io_cause(e))
                })?;
            password = contents.trim().to_string();
        }
        if password.is_empty() {
            return Err(Error::new(Code::Usage, "keystore password is required"));
        }
        return Err(Error::new(
            Code::Unsupported,
            "keystore decryption is not supported in this build; use --private-key or DEFI_PRIVATE_KEY",
        ));
    }
    Err(Error::new(
        Code::Usage,
        format!(
            "missing signing key: pass --private-key, set {ENV_PRIVATE_KEY}, set {ENV_PRIVATE_KEY_FILE}, or put key at {DEFAULT_PRIVATE_KEY_HINT_PATH} (XDG_CONFIG_HOME override); alternatively set {ENV_KEYSTORE_PATH} (+ {ENV_KEYSTORE_PASSWORD} or {ENV_KEYSTORE_PASSWORD_FILE})"
        ),
    ))
}

/// Build a [`defi_evm::signer::LocalSigner`] from resolved key inputs, parity
/// with Go `NewLocalSignerFromInputs`.
///
/// Resolves the hex key via [`resolve_private_key_hex`] (env/file/keystore
/// precedence + `--private-key` override) and hands it to the crypto core
/// [`LocalSigner::from_hex`]. A missing key is a [`Code::Usage`] error; an
/// un-parseable key is a [`Code::Signer`] error (from the crypto core).
pub fn local_signer_from_inputs(
    source: KeySource,
    private_key_override: &str,
    env: &dyn Env,
) -> Result<LocalSigner, Error> {
    let hex = resolve_private_key_hex(source, private_key_override, env)?;
    LocalSigner::from_hex(&hex)
}

/// The default private-key path: `$XDG_CONFIG_HOME/defi/key.hex`, else
/// `<home>/.config/defi/key.hex`, else `None`.
///
/// Parity with Go `defaultPrivateKeyPath` (`XDG_CONFIG_HOME` first, then
/// `os.UserHomeDir()/.config`). Returns `None` only when neither XDG nor home is
/// resolvable.
pub fn default_private_key_path(env: &dyn Env) -> Option<PathBuf> {
    let base = match trimmed_var(env, "XDG_CONFIG_HOME") {
        b if !b.is_empty() => PathBuf::from(b),
        _ => env.home_dir()?.join(".config"),
    };
    Some(base.join(DEFAULT_PRIVATE_KEY_RELATIVE_PATH))
}

/// Return the default key path only when a **regular file** exists there.
///
/// Parity with Go `discoverDefaultPrivateKeyFile`: a directory at that path is
/// ignored (Go skips `info.IsDir()`), and a missing path yields `None`.
fn discover_default_private_key_file(env: &dyn Env) -> Option<PathBuf> {
    let path = default_private_key_path(env)?;
    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_file() => Some(path),
        _ => None,
    }
}

/// An env var, trimmed; empty string when unset or whitespace-only.
fn trimmed_var(env: &dyn Env, key: &str) -> String {
    env.var(key)
        .map(|v| v.trim().to_string())
        .unwrap_or_default()
}

// =============================================================================
// Tempo type-0x76 transaction + smart-wallet signer.
//
// The exact tempo-go `transaction.Tx` RLP encoding is bespoke and owned by
// `crate::tempo_executor`; here the contract is the *signer* behavior — the
// signature recovers to the signing-key EOA, EVM signing is rejected, and the
// raw key accessor is not exposed.
// =============================================================================

/// A single batched call within a Tempo type-0x76 transaction.
///
/// The Rust analogue of tempo-go's `transaction.Call`: a target address, a wei
/// value, and calldata. The Tempo executor batches `approve` + `swap` into one
/// tx as an ordered list of these. Consumed by [`crate::tempo_executor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempoCall {
    /// The call target address.
    pub to: Address,
    /// The wei value forwarded with the call.
    pub value: U256,
    /// The ABI-encoded calldata bytes.
    pub data: Vec<u8>,
}

/// The Tempo transaction type prefix byte (`0x76`).
///
/// Parity with tempo-go `encodeWithPrefix` (prefix `"76"` for `FormatNormal`).
/// The signed/serialized bytes that go to `eth_sendRawTransaction` are
/// `0x76 || rlp([...])`.
pub const TEMPO_TX_TYPE: u8 = 0x76;

/// An (optionally signed) Tempo type-0x76 transaction.
///
/// A builder for the Tempo batched-call transaction. The fields and their RLP
/// layout mirror tempo-go's `transaction.Tx` byte-for-byte (the on-wire format
/// owned by `tempoxyz/tempo-go/pkg/transaction`), so [`Self::serialize`] and
/// [`Self::signing_hash`] reproduce tempo-go's `Serialize` / `GetSignPayload`
/// exactly. Self-paid (no fee payer) transactions only: `nonceKey`,
/// `validBefore`, `validAfter` default to 0, `accessList` and the
/// `authorizationList` are always empty, and the `feePayerSignatureOrSender`
/// field is the empty byte-string.
#[derive(Debug, Clone)]
pub struct TempoTx {
    /// EIP-155 chain id the signature is bound to.
    pub chain_id: u64,
    /// Account nonce.
    pub nonce: u64,
    /// `maxPriorityFeePerGas` (the tip cap), in wei.
    pub max_priority_fee_per_gas: u128,
    /// `maxFeePerGas` (the fee cap), in wei.
    pub max_fee_per_gas: u128,
    /// Gas limit.
    pub gas: u64,
    /// The ordered batched calls (`approve` + `swap` are atomic in one tx).
    pub calls: Vec<TempoCall>,
    /// The stablecoin fee-token address (`U256::ZERO`/`Address::ZERO` → native).
    pub fee_token: Address,
    /// The attached signature (`None` until signed).
    signature: Option<Signature>,
}

impl TempoTx {
    /// Begin a new unsigned Tempo transaction bound to `chain_id`.
    pub fn new(chain_id: u64) -> Self {
        TempoTx {
            chain_id,
            nonce: 0,
            max_priority_fee_per_gas: 0,
            max_fee_per_gas: 0,
            gas: 0,
            calls: Vec::new(),
            fee_token: Address::ZERO,
            signature: None,
        }
    }

    /// Set the gas limit (builder style).
    pub fn gas(mut self, gas: u64) -> Self {
        self.gas = gas;
        self
    }

    /// Set `maxFeePerGas` (builder style).
    pub fn max_fee_per_gas(mut self, v: u128) -> Self {
        self.max_fee_per_gas = v;
        self
    }

    /// Set `maxPriorityFeePerGas` (builder style).
    pub fn max_priority_fee_per_gas(mut self, v: u128) -> Self {
        self.max_priority_fee_per_gas = v;
        self
    }

    /// Set the account nonce (builder style).
    pub fn nonce(mut self, nonce: u64) -> Self {
        self.nonce = nonce;
        self
    }

    /// Set the stablecoin fee token (builder style). [`Address::ZERO`] → native.
    pub fn fee_token(mut self, token: Address) -> Self {
        self.fee_token = token;
        self
    }

    /// Append a batched call (builder style).
    pub fn add_call(mut self, to: Address, value: U256, data: Vec<u8>) -> Self {
        self.calls.push(TempoCall { to, value, data });
        self
    }

    /// True once a signature has been attached via
    /// [`TempoWalletSigner::sign_tempo_tx`].
    pub fn is_signed(&self) -> bool {
        self.signature.is_some()
    }

    /// Encode the RLP field list (excluding the trailing signature envelope),
    /// parity with tempo-go `buildRLPList` for a self-paid normal-format tx.
    ///
    /// The 13 sender-payload fields, in tempo-go declaration order:
    /// `[chainId, maxPriorityFeePerGas, maxFeePerGas, gas, calls, accessList,
    /// nonceKey, nonce, validBefore, validAfter, feeToken,
    /// feePayerSignatureOrSender, authorizationList]`.
    fn encode_field_payload(&self, out: &mut Vec<u8>) {
        // 0: chainId
        encode_uint_bytes(self.chain_id as u128, out);
        // 1: maxPriorityFeePerGas
        encode_uint_bytes(self.max_priority_fee_per_gas, out);
        // 2: maxFeePerGas
        encode_uint_bytes(self.max_fee_per_gas, out);
        // 3: gas
        encode_uint_bytes(self.gas as u128, out);
        // 4: calls = [[to, value, data], ...]
        encode_calls(&self.calls, out);
        // 5: accessList (always empty)
        encode_empty_list(out);
        // 6: nonceKey (always 0 → empty)
        encode_uint_bytes(0, out);
        // 7: nonce
        encode_uint_bytes(self.nonce as u128, out);
        // 8: validBefore (always 0 → empty)
        encode_uint_bytes(0, out);
        // 9: validAfter (always 0 → empty)
        encode_uint_bytes(0, out);
        // 10: feeToken (20 bytes, or empty when zero/native)
        encode_fee_token(self.fee_token, out);
        // 11: feePayerSignatureOrSender (empty byte-string, no fee payer)
        encode_bytes(&[], out);
        // 12: authorizationList (always empty)
        encode_empty_list(out);
    }

    /// The keccak256 signing hash over `0x76 || rlp([13 sender fields])`,
    /// parity with tempo-go `GetSignPayload` (`SerializeForSigning` → strips the
    /// signature → `Serialize(ForSigning, FormatNormal)` → `ComputeHash`).
    ///
    /// Self-paid only, so `SerializeForSigning` includes `feeToken` and the
    /// `feePayerSignatureOrSender` field is the empty byte-string.
    fn signing_hash(&self) -> alloy::primitives::B256 {
        let mut payload: Vec<u8> = Vec::new();
        self.encode_field_payload(&mut payload);

        let mut wire: Vec<u8> = Vec::with_capacity(1 + payload.len() + 9);
        wire.push(TEMPO_TX_TYPE);
        encode_list_header(payload.len(), &mut wire);
        wire.extend_from_slice(&payload);
        keccak256(&wire)
    }

    /// The signed, broadcast-ready bytes: `0x76 || rlp([14 fields])`, parity with
    /// tempo-go `Serialize(tx, nil)` for a signed self-paid tx.
    ///
    /// Appends the secp256k1 signature envelope (a 65-byte `r||s||yParity` string)
    /// as the 14th RLP field. Errors if the tx is unsigned (typed [`Code::Signer`]).
    pub fn serialize(&self) -> Result<Vec<u8>, Error> {
        let sig = self
            .signature
            .ok_or_else(|| Error::new(Code::Signer, "tempo tx is not signed"))?;

        let mut payload: Vec<u8> = Vec::new();
        self.encode_field_payload(&mut payload);
        // 13: signatureEnvelope — secp256k1 raw 65 bytes (r||s||yParity), encoded
        // as an RLP byte-string (`b841 || 65 bytes`).
        encode_bytes(&sig.as_rsy(), &mut payload);

        let mut wire: Vec<u8> = Vec::with_capacity(1 + payload.len() + 9);
        wire.push(TEMPO_TX_TYPE);
        encode_list_header(payload.len(), &mut wire);
        wire.extend_from_slice(&payload);
        Ok(wire)
    }

    /// The on-chain transaction hash: `keccak256(serialize())`, parity with
    /// tempo-go `ComputeHash(Serialize(tx, nil))`. Errors if the tx is unsigned.
    pub fn tx_hash(&self) -> Result<[u8; 32], Error> {
        Ok(keccak256(self.serialize()?).0)
    }

    /// Recover the signing address from the attached signature.
    ///
    /// Returns the key EOA the signature recovers to; errors if the tx is
    /// unsigned or the signature does not recover (typed [`Code::Signer`]).
    pub fn recover_signer(&self) -> Result<Address, Error> {
        let sig = self
            .signature
            .ok_or_else(|| Error::new(Code::Signer, "tempo tx is not signed"))?;
        let hash = self.signing_hash();
        sig.recover_address_from_prehash(&hash)
            .map(Address::from)
            .map_err(|e| Error::wrap(Code::Signer, "recover tempo signer", msg_cause(e)))
    }
}

// =============================================================================
// Tempo type-0x76 RLP encoding helpers.
//
// These reproduce tempo-go's `serialize.go` encoding rules byte-for-byte over
// `alloy_rlp` primitives:
//   - `bigIntToBytes`/`uint64ToBytes`: minimal big-endian, empty for 0.
//   - byte-strings: the standard RLP rule (single byte < 0x80 → itself; empty →
//     0x80; else header + payload) via `alloy_rlp`'s `[u8]` `Encodable`.
//   - lists: an explicit list header over the concatenated child payload.
// =============================================================================

/// Encode an unsigned integer as an RLP byte-string with minimal big-endian
/// bytes (empty for 0), parity with tempo-go `bigIntToBytes`/`uint64ToBytes`.
fn encode_uint_bytes(v: u128, out: &mut Vec<u8>) {
    if v == 0 {
        encode_bytes(&[], out);
        return;
    }
    let be = v.to_be_bytes();
    let start = be.iter().position(|&b| b != 0).unwrap_or(be.len());
    encode_bytes(&be[start..], out);
}

/// Encode raw bytes as an RLP byte-string (standard single-byte/empty/header
/// rules), via `alloy_rlp`'s `[u8]` `Encodable`.
fn encode_bytes(bytes: &[u8], out: &mut Vec<u8>) {
    alloy_rlp::Encodable::encode(bytes, out);
}

/// Encode the fee-token field: 20 address bytes when set, empty when zero
/// (native), parity with tempo-go `encodeFeeToken`.
fn encode_fee_token(token: Address, out: &mut Vec<u8>) {
    if token.is_zero() {
        encode_bytes(&[], out);
    } else {
        encode_bytes(&token.as_bytes(), out);
    }
}

/// Encode the calls field as `[[to, value, data], ...]`, parity with tempo-go
/// `encodeCalls` (each call a 3-tuple of byte-strings).
fn encode_calls(calls: &[TempoCall], out: &mut Vec<u8>) {
    let mut payload: Vec<u8> = Vec::new();
    for call in calls {
        let mut tuple: Vec<u8> = Vec::new();
        // 0: to (20 bytes; empty for contract creation, unused here)
        encode_bytes(&call.to.as_bytes(), &mut tuple);
        // 1: value (minimal big-endian, empty for 0)
        encode_u256_bytes(call.value, &mut tuple);
        // 2: data (raw bytes)
        encode_bytes(&call.data, &mut tuple);

        encode_list_header(tuple.len(), &mut payload);
        payload.extend_from_slice(&tuple);
    }
    encode_list_header(payload.len(), out);
    out.extend_from_slice(&payload);
}

/// Encode a [`U256`] call value as an RLP byte-string with minimal big-endian
/// bytes (empty for 0), parity with tempo-go `(*big.Int).Bytes()`.
fn encode_u256_bytes(v: U256, out: &mut Vec<u8>) {
    if v.is_zero() {
        encode_bytes(&[], out);
        return;
    }
    let be = v.to_be_bytes::<32>();
    let start = be.iter().position(|&b| b != 0).unwrap_or(be.len());
    encode_bytes(&be[start..], out);
}

/// Write an RLP list header for a payload of `payload_len` bytes.
fn encode_list_header(payload_len: usize, out: &mut Vec<u8>) {
    alloy_rlp::Header {
        list: true,
        payload_length: payload_len,
    }
    .encode(out);
}

/// Encode an empty RLP list (`0xc0`).
fn encode_empty_list(out: &mut Vec<u8>) {
    encode_list_header(0, out);
}

/// A Tempo smart-wallet signer: a signing-key EOA whose on-chain sender is a
/// *different* smart-wallet address.
///
/// Parity with Go `TempoWalletSigner`: [`Self::address`] is the signing-key EOA;
/// [`Self::wallet_address`] is the smart-wallet that acts as the on-chain sender.
/// Signs Tempo type-0x76 transactions (recoverable to the key EOA), refuses
/// standard EVM signing, and does not expose the raw EVM private key.
#[derive(Debug, Clone)]
pub struct TempoWalletSigner {
    wallet_addr: Address,
    inner: PrivateKeySigner,
    key_addr: Address,
}

impl TempoWalletSigner {
    /// Build a [`TempoWalletSigner`] from a smart-wallet address and a hex key.
    ///
    /// Parity with Go `NewTempoWalletSigner`: trims whitespace + an optional
    /// `0x`/`0X` prefix on the key, derives the key EOA, and stores the provided
    /// wallet address. An invalid key is a typed [`Code::Signer`] error.
    pub fn new(wallet_addr: Address, private_key_hex: &str) -> Result<Self, Error> {
        // Reuse the crypto core's hex-key parsing + address derivation so the
        // key-parse contract (trim, optional 0x, 64-hex, in-range) is identical
        // to the local signer and typed `Code::Signer` on failure.
        let local = LocalSigner::from_hex(private_key_hex)?;
        let key_addr = local.address();
        let trimmed = private_key_hex.trim();
        let body = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        let inner: PrivateKeySigner = body
            .parse()
            .map_err(|e| Error::wrap(Code::Signer, "create tempo signer", msg_cause(e)))?;
        Ok(TempoWalletSigner {
            wallet_addr,
            inner,
            key_addr,
        })
    }

    /// The signing-key EOA address (`crypto.PubkeyToAddress`).
    pub fn address(&self) -> Address {
        self.key_addr
    }

    /// The smart-wallet address that acts as the on-chain sender.
    pub fn wallet_address(&self) -> Address {
        self.wallet_addr
    }

    /// Sign a Tempo type-0x76 transaction in place.
    ///
    /// Attaches a signature over [`TempoTx::signing_hash`] that recovers to
    /// [`Self::address`]. After signing, [`TempoTx::is_signed`] is `true`.
    pub fn sign_tempo_tx(&self, tx: &mut TempoTx) -> Result<(), Error> {
        let hash = tx.signing_hash();
        let sig = self
            .inner
            .sign_hash_sync(&hash)
            .map_err(|e| Error::wrap(Code::Signer, "sign tempo tx", msg_cause(e)))?;
        tx.signature = Some(sig);
        Ok(())
    }

    /// Standard EVM signing is **unsupported** for a Tempo wallet signer.
    ///
    /// Parity with Go `SignTx`: Tempo chains use type-0x76 transactions which must
    /// be signed via [`Self::sign_tempo_tx`]. Returns a [`Code::Unsupported`]
    /// error.
    pub fn sign_evm_tx(&self, _chain_id: u64, _tx: &Eip1559Tx) -> Result<SignedTx, Error> {
        Err(Error::new(
            Code::Unsupported,
            "TempoWalletSigner does not support EVM SignTx; use SignTempoTx for Tempo chains",
        ))
    }

    /// The raw EVM private key is **not** exposed (parity with Go `PrivateKey()`
    /// returning `nil`): the key is owned by the Tempo signer.
    pub fn private_key_hex(&self) -> Option<String> {
        None
    }
}

/// The JSON shape of `tempo wallet -j whoami`.
///
/// Field names mirror the Go `tempoWhoamiResponse`. Only the fields that carry
/// the readiness/expiry decision + the wallet/key addresses are modeled.
#[derive(Debug, serde::Deserialize)]
struct TempoWhoamiResponse {
    #[serde(default)]
    ready: bool,
    #[serde(default)]
    wallet: String,
    #[serde(default)]
    key: TempoWhoamiKey,
}

#[derive(Debug, Default, serde::Deserialize)]
struct TempoWhoamiKey {
    #[serde(default)]
    key: String,
    #[serde(default)]
    expires_at: String,
}

/// Parse `tempo wallet -j whoami` JSON into a configured [`TempoWalletSigner`]
/// plus any non-fatal warnings.
///
/// Parity with the parse + readiness/expiry decision half of Go
/// `NewTempoSignerFromCLI` (the shell-out itself is bespoke; see spec §7):
/// - `ready: false` → a [`Code::Signer`] error (not logged in).
/// - an `expires_at` in the past → a [`Code::Signer`] error (expired key).
/// - an `expires_at` < 24h away → success WITH a near-expiry warning.
/// - malformed JSON → a [`Code::Signer`] error (never a panic).
pub fn tempo_signer_from_whoami(json: &str) -> Result<(TempoWalletSigner, Vec<String>), Error> {
    let resp: TempoWhoamiResponse = serde_json::from_str(json)
        .map_err(|e| Error::wrap(Code::Signer, "parse tempo wallet output", msg_cause(e)))?;

    if !resp.ready {
        return Err(Error::new(
            Code::Signer,
            "tempo wallet is not logged in; run 'tempo wallet login' to set up your agent wallet",
        ));
    }

    let mut warnings: Vec<String> = Vec::new();
    if !resp.key.expires_at.is_empty() {
        if let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(&resp.key.expires_at) {
            let expiry = expiry.with_timezone(&chrono::Utc);
            let now = chrono::Utc::now();
            if now > expiry {
                return Err(Error::new(
                    Code::Signer,
                    "tempo wallet access key has expired; run 'tempo wallet login' to refresh",
                ));
            }
            let until = expiry - now;
            if until < chrono::Duration::hours(24) {
                let hours = (until.num_minutes() as f64 / 60.0).round() as i64;
                warnings.push(format!("tempo wallet key expires in {hours}h"));
            }
        }
    }

    let wallet_addr = Address::from(parse_whoami_wallet(&resp.wallet));
    let signer = TempoWalletSigner::new(wallet_addr, &resp.key.key)?;
    Ok((signer, warnings))
}

/// Parse the whoami `wallet` field leniently, parity with go-ethereum
/// `common.HexToAddress` (which never errors — it right-aligns/truncates).
///
/// An invalid wallet string yields the zero address rather than failing the
/// whole signer construction, matching the Go behavior where `HexToAddress` on a
/// non-hex value silently produces the zero address.
fn parse_whoami_wallet(raw: &str) -> alloy::primitives::Address {
    match defi_evm::address::parse(raw.trim()) {
        Ok(addr) => addr.into_inner(),
        Err(_) => alloy::primitives::Address::ZERO,
    }
}

/// A concrete, `Send + Sync` std error carrying an error's display text.
///
/// Records a foreign error's message as the `cause` of a typed [`Error`] without
/// depending on each foreign type implementing the `Error + Send + Sync` bound
/// [`Error::wrap`] requires.
#[derive(Debug)]
struct MsgError(String);

impl std::fmt::Display for MsgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MsgError {}

/// Capture an arbitrary error's display text as a concrete [`MsgError`] cause.
fn msg_cause<E: std::fmt::Display>(e: E) -> MsgError {
    MsgError(e.to_string())
}

/// Capture an `io::Error`'s display text as a [`MsgError`] cause.
fn io_cause(e: std::io::Error) -> MsgError {
    MsgError(e.to_string())
}

#[cfg(test)]
mod tests {
    //! RED phase: these reference the not-yet-implemented public API of this
    //! module (`KeySource`, `resolve_private_key_hex`, `default_private_key_path`,
    //! `local_signer_from_inputs`, `TempoWalletSigner`, `TempoTx`,
    //! `tempo_signer_from_whoami`, env-var name constants). They MUST fail to
    //! compile / fail assertions until GREEN.
    //!
    //! All vectors are deterministic and offline. The signing key is the
    //! well-known go-ethereum / Hardhat test key from
    //! `internal/execution/signer/local_test.go`; its address is the canonical
    //! value `defi_evm::signer::LocalSigner` (and go-ethereum's
    //! `crypto.PubkeyToAddress`) derive — independently reproducible, no network.

    use super::*;

    use defi_config::{Env, MapEnv};
    use std::path::PathBuf;

    /// `testPrivateKey` from `internal/execution/signer/local_test.go`.
    const TEST_KEY: &str = "59c6995e998f97a5a0044976f0945388cf9b7e5e5f4f9d2d9d8f1f5b7f6d11d1";
    /// EIP-55 address derived for `TEST_KEY` (oracle: `defi_evm::signer`).
    const TEST_ADDR: &str = "0x14DDBd1fe5026E58A12eE8691cAEbFD24bb10eef";

    /// The simple missing-key path hint (`local.go` `defaultPrivateKeyHintPath`).
    const HINT_PATH: &str = "~/.config/defi/key.hex";

    // --- helpers --------------------------------------------------------

    /// An empty injected env rooted at a temp home with no relevant vars set.
    fn empty_env(home: &std::path::Path) -> MapEnv {
        MapEnv::with_home(home.to_path_buf())
    }

    // ===================================================================
    // A. Key-source selector parity
    // ===================================================================

    #[test]
    fn key_source_parse_is_case_insensitive_and_trims() {
        // A1: case-insensitive + whitespace-trimmed.
        assert_eq!(KeySource::parse("auto").unwrap(), KeySource::Auto);
        assert_eq!(KeySource::parse("AUTO").unwrap(), KeySource::Auto);
        assert_eq!(KeySource::parse("  Env ").unwrap(), KeySource::Env);
        assert_eq!(KeySource::parse("FILE").unwrap(), KeySource::File);
        assert_eq!(KeySource::parse(" keystore").unwrap(), KeySource::Keystore);
    }

    #[test]
    fn key_source_empty_defaults_to_auto() {
        // A2.
        assert_eq!(KeySource::parse("").unwrap(), KeySource::Auto);
        assert_eq!(KeySource::parse("   ").unwrap(), KeySource::Auto);
    }

    #[test]
    fn key_source_unknown_is_usage_error_naming_valid_sources() {
        // A3.
        let err = KeySource::parse("hsm").unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Usage);
        let msg = err.to_string();
        for src in ["auto", "env", "file", "keystore"] {
            assert!(msg.contains(src), "missing {src} in: {msg}");
        }
    }

    // ===================================================================
    // B. Local key-source precedence (env > file > default; override > all)
    // ===================================================================

    #[test]
    fn env_hex_source_resolves_to_that_key() {
        // B1: DEFI_PRIVATE_KEY set, Env source → that hex; non-zero address.
        let home = tempfile::tempdir().expect("tmp home");
        let env = empty_env(home.path()).set(ENV_PRIVATE_KEY, TEST_KEY);

        let hex = resolve_private_key_hex(KeySource::Env, "", &env).expect("resolve env hex");
        assert_eq!(hex.trim().trim_start_matches("0x"), TEST_KEY);

        let signer = local_signer_from_inputs(KeySource::Env, "", &env).expect("local signer");
        assert!(!signer.address().is_zero());
        assert_eq!(signer.address().to_hex(), TEST_ADDR);
    }

    #[test]
    fn env_file_source_reads_and_trims_key_file() {
        // B2 (+ non-strict permissions): DEFI_PRIVATE_KEY_FILE points at a key.
        let home = tempfile::tempdir().expect("tmp home");
        let key_file = home.path().join("key.txt");
        std::fs::write(&key_file, format!("{TEST_KEY}\n")).expect("write key file");
        // World-readable (0o644) must still load — perms are not enforced.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o644))
                .expect("chmod 644");
        }
        let env = empty_env(home.path())
            .set(ENV_PRIVATE_KEY_FILE, key_file.to_string_lossy().to_string());

        let signer = local_signer_from_inputs(KeySource::File, "", &env).expect("file signer");
        assert_eq!(signer.address().to_hex(), TEST_ADDR);
    }

    #[test]
    fn auto_source_discovers_default_key_file() {
        // B3: no env hex/file/keystore, key at $XDG_CONFIG_HOME/defi/key.hex.
        let home = tempfile::tempdir().expect("tmp home");
        let cfg = tempfile::tempdir().expect("tmp cfg");
        let key_dir = cfg.path().join("defi");
        std::fs::create_dir_all(&key_dir).expect("mkdir defi");
        std::fs::write(key_dir.join("key.hex"), TEST_KEY).expect("write default key");

        let env =
            empty_env(home.path()).set("XDG_CONFIG_HOME", cfg.path().to_string_lossy().to_string());

        let signer = local_signer_from_inputs(KeySource::Auto, "", &env).expect("auto signer");
        assert_eq!(signer.address().to_hex(), TEST_ADDR);
    }

    #[test]
    fn private_key_override_beats_all_sources() {
        // B4: override wins even under Auto with nothing else set.
        let home = tempfile::tempdir().expect("tmp home");
        let env = empty_env(home.path());
        let signer =
            local_signer_from_inputs(KeySource::Auto, TEST_KEY, &env).expect("override signer");
        assert_eq!(signer.address().to_hex(), TEST_ADDR);
    }

    #[test]
    fn private_key_override_wins_over_file_source_with_bogus_file() {
        // B4: override wins over a File source whose DEFI_PRIVATE_KEY_FILE is bogus.
        let home = tempfile::tempdir().expect("tmp home");
        let env = empty_env(home.path()).set(ENV_PRIVATE_KEY_FILE, "/tmp/does-not-exist");
        let signer =
            local_signer_from_inputs(KeySource::File, TEST_KEY, &env).expect("override over file");
        assert_eq!(signer.address().to_hex(), TEST_ADDR);
    }

    #[test]
    fn env_source_ignores_file_and_keystore_inputs() {
        // B5: with Env source and only a bogus key FILE set (no env hex),
        // resolution must NOT fall back to the file → missing-key error.
        let home = tempfile::tempdir().expect("tmp home");
        let env = empty_env(home.path())
            .set(ENV_PRIVATE_KEY_FILE, "/tmp/does-not-exist")
            .set(ENV_KEYSTORE_PATH, "/tmp/keystore.json");
        let err = resolve_private_key_hex(KeySource::Env, "", &env).unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    #[test]
    fn missing_key_error_includes_private_key_and_simple_path_hints() {
        // B6: no key anywhere → usage error citing --private-key AND the path hint.
        let home = tempfile::tempdir().expect("tmp home");
        let env = empty_env(home.path());
        let err = resolve_private_key_hex(KeySource::Auto, "", &env).unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Usage);
        let msg = err.to_string();
        assert!(
            msg.contains("--private-key"),
            "missing --private-key: {msg}"
        );
        assert!(msg.contains(HINT_PATH), "missing {HINT_PATH}: {msg}");
    }

    // ===================================================================
    // C. Default key path resolution
    // ===================================================================

    #[test]
    fn default_key_path_uses_xdg_config_home() {
        // C1.
        let env = MapEnv::default().set("XDG_CONFIG_HOME", "/tmp/defi-config-home");
        let got = default_private_key_path(&env).expect("xdg path");
        assert_eq!(got, PathBuf::from("/tmp/defi-config-home/defi/key.hex"));
    }

    #[test]
    fn default_key_path_falls_back_to_home_config() {
        // C2.
        let env = MapEnv::with_home("/home/agent");
        let got = default_private_key_path(&env).expect("home path");
        assert_eq!(got, PathBuf::from("/home/agent/.config/defi/key.hex"));
    }

    #[test]
    fn default_key_path_none_without_xdg_or_home() {
        // C2: neither XDG nor home → None.
        let env = MapEnv::default();
        assert!(default_private_key_path(&env).is_none());
    }

    #[test]
    fn auto_discovery_ignores_a_directory_at_the_key_path() {
        // C3: a *directory* at $XDG/defi/key.hex must NOT be treated as a key.
        let home = tempfile::tempdir().expect("tmp home");
        let cfg = tempfile::tempdir().expect("tmp cfg");
        std::fs::create_dir_all(cfg.path().join("defi").join("key.hex"))
            .expect("mkdir key.hex as dir");
        let env =
            empty_env(home.path()).set("XDG_CONFIG_HOME", cfg.path().to_string_lossy().to_string());

        // No usable key anywhere → missing-key error (the directory is ignored).
        let err = resolve_private_key_hex(KeySource::Auto, "", &env).unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Usage);
    }

    // ===================================================================
    // D. TempoWalletSigner
    // ===================================================================

    fn wallet_addr(hex: &str) -> defi_evm::address::Address {
        defi_evm::address::parse(hex).expect("valid wallet address")
    }

    #[test]
    fn tempo_wallet_signer_exposes_wallet_and_key_addresses() {
        // D1.
        let wallet = wallet_addr("0x1111111111111111111111111111111111111111");
        let s = TempoWalletSigner::new(wallet, TEST_KEY).expect("tempo signer");
        assert_eq!(s.wallet_address(), wallet);
        assert_eq!(s.address().to_hex(), TEST_ADDR);
    }

    #[test]
    fn tempo_wallet_address_differs_from_key_address() {
        // D2.
        let wallet = wallet_addr("0x2222222222222222222222222222222222222222");
        let s = TempoWalletSigner::new(wallet, TEST_KEY).expect("tempo signer");
        assert_ne!(s.wallet_address().to_hex(), s.address().to_hex());
    }

    #[test]
    fn tempo_wallet_signer_accepts_0x_prefixed_key() {
        // D1: optional 0x prefix + whitespace.
        let wallet = wallet_addr("0x3333333333333333333333333333333333333333");
        let s =
            TempoWalletSigner::new(wallet, &format!("  0x{TEST_KEY}  ")).expect("0x-prefixed key");
        assert_eq!(s.address().to_hex(), TEST_ADDR);
    }

    #[test]
    fn tempo_sign_recovers_to_key_address() {
        // D3: sign a tempo tx; signature recovers to the signing-key EOA.
        let wallet = wallet_addr("0x4444444444444444444444444444444444444444");
        let s = TempoWalletSigner::new(wallet, TEST_KEY).expect("tempo signer");

        let target = wallet_addr("0x5555555555555555555555555555555555555555");
        let mut tx = TempoTx::new(4217)
            .gas(21_000)
            .max_fee_per_gas(1_000_000_000)
            .max_priority_fee_per_gas(100_000_000)
            .nonce(0)
            .add_call(target, alloy::primitives::U256::ZERO, vec![0x01, 0x02]);

        s.sign_tempo_tx(&mut tx).expect("sign tempo tx");
        assert!(tx.is_signed(), "tx must carry a signature after signing");

        let recovered = tx.recover_signer().expect("recover");
        assert_eq!(recovered.to_hex(), s.address().to_hex());
    }

    #[test]
    fn tempo_wallet_signer_rejects_evm_sign() {
        // D4: standard EVM signing is unsupported.
        let wallet = wallet_addr("0x6666666666666666666666666666666666666666");
        let s = TempoWalletSigner::new(wallet, TEST_KEY).expect("tempo signer");

        let tx = defi_evm::signer::Eip1559Tx {
            chain_id: 1,
            nonce: 0,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 2,
            gas_limit: 21_000,
            to: Some(wallet),
            value: alloy::primitives::U256::ZERO,
            input: vec![],
        };
        let err = s.sign_evm_tx(1, &tx).unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Unsupported);
    }

    #[test]
    fn tempo_wallet_signer_does_not_expose_raw_private_key() {
        // D5: the raw EVM key accessor returns None.
        let wallet = wallet_addr("0x7777777777777777777777777777777777777777");
        let s = TempoWalletSigner::new(wallet, TEST_KEY).expect("tempo signer");
        assert!(s.private_key_hex().is_none());
    }

    #[test]
    fn tempo_wallet_signer_rejects_invalid_key() {
        // D6: typed signer error for a bad key.
        let wallet = wallet_addr("0x8888888888888888888888888888888888888888");
        let err = TempoWalletSigner::new(wallet, "not-a-valid-hex-key").unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Signer);
    }

    // ===================================================================
    // E. Tempo CLI whoami parse + readiness/expiry decision
    // ===================================================================

    /// A `ready` whoami JSON with the given `expires_at` (RFC3339 or empty).
    fn whoami_json(ready: bool, wallet: &str, key: &str, expires_at: &str) -> String {
        format!(
            r#"{{"ready":{ready},"wallet":"{wallet}","key":{{"address":"0xkey","key":"{key}","chain_id":4217,"spending_limit":{{"remaining":"100"}},"expires_at":"{expires_at}"}}}}"#
        )
    }

    #[test]
    fn whoami_ready_future_expiry_yields_signer_no_warnings() {
        // E1.
        let wallet = "0x9999999999999999999999999999999999999999";
        let future = "2999-01-01T00:00:00Z";
        let json = whoami_json(true, wallet, TEST_KEY, future);

        let (signer, warnings) = tempo_signer_from_whoami(&json).expect("ready whoami → signer");
        assert_eq!(
            signer.wallet_address().to_hex(),
            defi_evm::address::parse(wallet).unwrap().to_hex()
        );
        assert_eq!(signer.address().to_hex(), TEST_ADDR);
        assert!(warnings.is_empty(), "no warnings for far-future expiry");
    }

    #[test]
    fn whoami_not_ready_is_error() {
        // E2.
        let json = whoami_json(
            false,
            "0x9999999999999999999999999999999999999999",
            TEST_KEY,
            "",
        );
        let err = tempo_signer_from_whoami(&json).unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Signer);
    }

    #[test]
    fn whoami_expired_key_is_error() {
        // E3.
        let past = "2000-01-01T00:00:00Z";
        let json = whoami_json(
            true,
            "0x9999999999999999999999999999999999999999",
            TEST_KEY,
            past,
        );
        let err = tempo_signer_from_whoami(&json).unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Signer);
    }

    #[test]
    fn whoami_near_expiry_warns_but_succeeds() {
        // E4: expiry < 24h away → success WITH a warning mentioning expiry.
        let soon = (chrono::Utc::now() + chrono::Duration::hours(2))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let json = whoami_json(
            true,
            "0x9999999999999999999999999999999999999999",
            TEST_KEY,
            &soon,
        );
        let (_signer, warnings) =
            tempo_signer_from_whoami(&json).expect("near-expiry still succeeds");
        assert!(!warnings.is_empty(), "expected a near-expiry warning");
        assert!(
            warnings.iter().any(|w| w.to_lowercase().contains("expire")),
            "warning should mention expiry: {warnings:?}"
        );
    }

    #[test]
    fn whoami_malformed_json_is_error_not_panic() {
        // E5.
        let err = tempo_signer_from_whoami("{not json").unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    // ===================================================================
    // G. Tempo type-0x76 on-wire byte parity vs `tempo-go` (WS4a)
    // ===================================================================
    //
    // The reference bytes below were produced by a `tempo-go v0.3.0` oracle
    // (`tempoxyz/tempo-go/pkg/transaction.{GetSignPayload,SignTransaction,
    // Serialize,ComputeHash}`) for the fixed inputs in each case. secp256k1
    // signing is deterministic (RFC 6979) and low-S canonical in both
    // go-ethereum (`crypto.Sign`) and alloy (`k256`), so the signed bytes —
    // including `r`, `s`, and `yParity` — are reproducible and safe to pin.
    //
    // This is the byte-for-byte parity gate that pins the Rust [`TempoTx`] RLP
    // layout, signing hash, signature-envelope encoding, and tx hash against
    // tempo-go. It supersedes the prior bespoke domain-separated digest.

    /// Hardhat account #0 key — the key `tempo_executor_test.go` and the
    /// `tempo-go` oracle program both use.
    const HARDHAT_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    /// One fixed `tempo-go` reference vector.
    struct TempoVector {
        chain_id: u64,
        nonce: u64,
        gas: u64,
        max_priority_fee_per_gas: u128,
        max_fee_per_gas: u128,
        fee_token: &'static str, // "" → native (zero) fee token
        /// `(to, decimal_value, data_hex)` calls in order.
        calls: &'static [(&'static str, &'static str, &'static str)],
        signing_hash_hex: &'static str,
        signed_serialized_hex: &'static str,
        tx_hash_hex: &'static str,
    }

    fn build_tx(v: &TempoVector) -> TempoTx {
        let mut tx = TempoTx::new(v.chain_id)
            .gas(v.gas)
            .max_fee_per_gas(v.max_fee_per_gas)
            .max_priority_fee_per_gas(v.max_priority_fee_per_gas)
            .nonce(v.nonce);
        if !v.fee_token.is_empty() {
            tx = tx.fee_token(defi_evm::address::parse(v.fee_token).expect("fee token"));
        }
        for (to, value, data) in v.calls {
            let to = defi_evm::address::parse(to).expect("call to");
            let value = U256::from_str_radix(value, 10).expect("call value");
            let data = hex::decode(data.trim_start_matches("0x")).expect("call data");
            tx = tx.add_call(to, value, data);
        }
        tx
    }

    /// The fixed `tempo-go` golden vectors (captured offline; see header).
    const TEMPO_VECTORS: &[TempoVector] = &[
        // 1: batched approve+swap, AlphaUSD fee token, chain 4217, nonce 7.
        TempoVector {
            chain_id: 4217,
            nonce: 7,
            gas: 120_000,
            max_priority_fee_per_gas: 100_000_000,
            max_fee_per_gas: 1_500_000_000,
            fee_token: "0x20c0000000000000000000000000000000000001",
            calls: &[
                ("0x00000000000000000000000000000000000000bb", "0", "0xabcdef"),
                (
                    "0xdec0000000000000000000000000000000000000",
                    "1000",
                    "0x12345678",
                ),
            ],
            signing_hash_hex:
                "0xb224a6ae8f3733980423d386628f3cfa020b2bd5f35b45dcc4ed687d8977268f",
            signed_serialized_hex:
                "0x76f8ab8210798405f5e1008459682f008301d4c0f839da9400000000000000000000000000000000000000bb8083abcdefdd94dec00000000000000000000000000000000000008203e88412345678c0800780809420c000000000000000000000000000000000000180c0b841c3fa895ec3931398c74719538a63ab2ba569b2a2188db5e5211a65da3945c237544e1a71b40bb10164c2ffa477a7e44183594f0af3644865daac177c01a7d64b00",
            tx_hash_hex:
                "0x05ce203b9f8b60690407c919f6625a4b16538bef9b5fd807b895fdf214083568",
        },
        // 2: single call, empty data, zero (native) fee token, chain 4217, nonce 0.
        TempoVector {
            chain_id: 4217,
            nonce: 0,
            gas: 21_000,
            max_priority_fee_per_gas: 0,
            max_fee_per_gas: 1_000_000_000,
            fee_token: "",
            calls: &[("0x5555555555555555555555555555555555555555", "0", "0x")],
            signing_hash_hex:
                "0xd7d01c5776031839c6cbe640b64e505f45f8f192211e4d3e650aebe36aad701f",
            signed_serialized_hex:
                "0x76f87082107980843b9aca00825208d8d79455555555555555555555555555555555555555558080c0808080808080c0b8418ab4adf434ed3e81862f456e5cb6e6df49fcd87345eb37aade939ab5dcb3996513b670bf49fcc9415748e528a5513b3718972901743cbb161c7b178c2a70636d01",
            tx_hash_hex:
                "0x22cbb02db622fa66cbc813e5558930d93088a48699cc8f4f13a239b78ed5efff",
        },
        // 3: single call with value, large nonce, moderato chain 42431.
        TempoVector {
            chain_id: 42431,
            nonce: 1_000_000,
            gas: 500_000,
            max_priority_fee_per_gas: 2_000_000_000,
            max_fee_per_gas: 3_000_000_000,
            fee_token: "0x20c0000000000000000000000000000000000001",
            calls: &[(
                "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd",
                "123456789",
                "0xdeadbeef",
            )],
            signing_hash_hex:
                "0x0b1d773c4e5e03858a0879f92de93fa5669003d1f88cda219bf96f112c164fa9",
            signed_serialized_hex:
                "0x76f89482a5bf847735940084b2d05e008307a120e0df94abcdefabcdefabcdefabcdefabcdefabcdefabcd84075bcd1584deadbeefc080830f424080809420c000000000000000000000000000000000000180c0b8411faedd6b9c7dcd481ca66fff3a997b5687f0341e8dcd6053b84fa22b0f3f328452dc77716104dc188099cb9041f4f6147c94da842d15fb3641bf789441bf52cf00",
            tx_hash_hex:
                "0x0b5a843240419d81ae7d08647fc06d9455d42fedcd6df86c27c1a5cdee422f3c",
        },
    ];

    #[test]
    fn tempo_signing_hash_matches_tempo_go_oracle() {
        // G1: the sender signing payload hash (`0x76 || rlp(13 fields)`) is
        // byte-identical to tempo-go `GetSignPayload`. This pins the unsigned
        // RLP layout independent of signing.
        for v in TEMPO_VECTORS {
            let tx = build_tx(v);
            let got = format!("0x{}", hex::encode(tx.signing_hash().0));
            assert_eq!(
                got, v.signing_hash_hex,
                "signing-hash parity drift for chain {} nonce {}",
                v.chain_id, v.nonce
            );
        }
    }

    #[test]
    fn tempo_serialized_bytes_match_tempo_go_oracle() {
        // G2: the signed, broadcast-ready bytes (`0x76 || rlp(14 fields)`,
        // including the secp256k1 signature envelope) are byte-identical to
        // tempo-go `Serialize(tx, nil)`. This is the on-wire parity gate.
        let wallet = wallet_addr("0x1111111111111111111111111111111111111111");
        for v in TEMPO_VECTORS {
            let signer = TempoWalletSigner::new(wallet, HARDHAT_KEY).expect("signer");
            let mut tx = build_tx(v);
            signer.sign_tempo_tx(&mut tx).expect("sign");

            let got = format!("0x{}", hex::encode(tx.serialize().expect("serialize")));
            assert_eq!(
                got, v.signed_serialized_hex,
                "serialized-byte parity drift for chain {} nonce {}",
                v.chain_id, v.nonce
            );
        }
    }

    #[test]
    fn tempo_tx_hash_matches_tempo_go_oracle() {
        // G3: the on-chain tx hash (`keccak256(serialize())`) is byte-identical
        // to tempo-go `ComputeHash(Serialize(tx, nil))`.
        let wallet = wallet_addr("0x2222222222222222222222222222222222222222");
        for v in TEMPO_VECTORS {
            let signer = TempoWalletSigner::new(wallet, HARDHAT_KEY).expect("signer");
            let mut tx = build_tx(v);
            signer.sign_tempo_tx(&mut tx).expect("sign");

            let got = format!("0x{}", hex::encode(tx.tx_hash().expect("tx hash")));
            assert_eq!(
                got, v.tx_hash_hex,
                "tx-hash parity drift for chain {} nonce {}",
                v.chain_id, v.nonce
            );
        }
    }

    #[test]
    fn tempo_signature_recovers_to_key_after_real_layout() {
        // G4: with the real tempo-go signing hash, the attached signature still
        // recovers to the signing-key EOA (the property the smart-wallet sender
        // path relies on). Guards against a hash/recovery mismatch.
        let wallet = wallet_addr("0x3333333333333333333333333333333333333333");
        let signer = TempoWalletSigner::new(wallet, HARDHAT_KEY).expect("signer");
        let mut tx = build_tx(&TEMPO_VECTORS[0]);
        signer.sign_tempo_tx(&mut tx).expect("sign");
        assert_eq!(
            tx.recover_signer().expect("recover").to_hex(),
            signer.address().to_hex()
        );
    }

    #[test]
    fn tempo_serialize_unsigned_is_signer_error() {
        // Serializing/hashing an unsigned tx is a typed Signer error (no panic).
        let tx = build_tx(&TEMPO_VECTORS[0]);
        assert_eq!(tx.serialize().unwrap_err().code, Code::Signer);
        assert_eq!(tx.tx_hash().unwrap_err().code, Code::Signer);
        assert_eq!(tx.recover_signer().unwrap_err().code, Code::Signer);
    }
}
