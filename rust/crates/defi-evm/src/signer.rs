//! Local-key transaction signing (secp256k1 → EIP-55 address, EIP-1559 tx
//! signing). Scaffold stub — Phase 2 (RED).
//!
//! This module owns the **cryptographic signing half** of the machine contract
//! that the Go tree reached for via go-ethereum's `crypto` + `core/types`
//! packages (`crypto.HexToECDSA`, `crypto.PubkeyToAddress`,
//! `types.LatestSignerForChainID`, `types.SignTx`). It is the single canonical
//! place a raw secp256k1 private key turns into (a) the EIP-55 signing address
//! the executor reports as `EffectiveSender()` and validates persisted actions
//! against, and (b) a *signed* EIP-1559 (`DynamicFeeTx`) transaction whose bytes
//! get broadcast via `eth_sendRawTransaction`. Both are load-bearing for the
//! machine contract: the address is what flows into `from_address`/sender
//! checks, and the signed-tx bytes must be a valid, chain-id-bound, recoverable
//! signature or the broadcast (and the on-chain effect) is wrong.
//!
//! The Go `signer.Signer` interface is:
//! ```go
//! type Signer interface {
//!     Address() common.Address
//!     SignTx(chainID *big.Int, tx *types.Transaction) (*types.Transaction, error)
//! }
//! ```
//! and `LocalSigner` implements it by holding an `*ecdsa.PrivateKey`, deriving
//! `crypto.PubkeyToAddress(pub)` once at construction, and signing with
//! `types.SignTx(tx, types.LatestSignerForChainID(chainID), pk)` (which, for a
//! `DynamicFeeTx`, is the EIP-1559 / EIP-2718 typed-tx signature scheme).
//!
//! The idiomatic Rust port wraps `alloy-signer-local`'s `PrivateKeySigner`
//! (key → address) plus `alloy-consensus`'s `TxEip1559` / `SignableTransaction`
//! (chain-id-bound EIP-1559 signing) behind a small [`LocalSigner`] type. The
//! **scope split vs. the Go `internal/execution/signer` package**:
//!
//! - **OWNED HERE (`defi-evm::signer`)** — the pure crypto + EVM-tx primitives:
//!   parse a hex secp256k1 key, derive its EIP-55 address, sign an EIP-1559
//!   transaction so the signature recovers to that address and is bound to the
//!   given chain id, and surface the raw RLP-encoded signed-tx bytes + tx hash
//!   for broadcast. No `std::env`, no filesystem, no keystore JSON, no `tempo`
//!   shell-out.
//!
//! - **NOT here (lives in `defi-config` / `defi-execution` L2–L3)** — the
//!   *key-source orchestration*: env-var precedence (`DEFI_PRIVATE_KEY` >
//!   `DEFI_PRIVATE_KEY_FILE` > auto-discovered `~/.config/defi/key.hex` >
//!   keystore), `--private-key` override winning over a file source, V3 keystore
//!   decryption, path normalization, and the missing-key usage-error hint. Those
//!   are I/O + config-precedence concerns (the spec's `flags>env>file>defaults`
//!   invariant) that read into a hex key string and then call into THIS module.
//!   Re-testing them here would calcify filesystem/env coupling into the crypto
//!   crate; the ported Go `local_test.go` cases that assert env/file/auto
//!   precedence belong to the `defi-config`/`defi-execution` RED suites, NOT
//!   here. (See the SKIP list at the bottom of this comment.)
//!
//! - **NOT here (Tempo, bespoke — `defi-execution::tempo_executor` /
//!   `defi-execution::signer`)** — `TempoWalletSigner` (type 0x76 batched-call
//!   signing, smart-wallet address ≠ key address) and `NewTempoSignerFromCLI`
//!   (`tempo wallet -j whoami` shell-out, expiry warnings). The spec (§7) treats
//!   Tempo 0x76 + the CLI shell-out as a separate execution path covered by
//!   shell-out parity + fixtures, so the Tempo `tempo_test.go` cases are ported
//!   in the `defi-execution` Tempo RED suite, not here.
//!
//! # Success criteria (contract this module must preserve)
//!
//! 1. **Hex key parsing parity with `crypto.HexToECDSA`** — [`LocalSigner::from_hex`]:
//!    accepts a 64-hex-digit secp256k1 private key with an **optional** `0x`/`0X`
//!    prefix and surrounding whitespace trimmed (the Go `parseHexKey` does
//!    `TrimSpace` then `TrimPrefix(..,"0x")` before `crypto.HexToECDSA`); rejects
//!    empty, non-hex, wrong-length, and out-of-range (>= secp256k1 group order or
//!    zero) keys with a `defi_errors`-typed [`crate::Error`] (no panic/unwrap in
//!    lib code).
//!
//! 2. **Address derivation parity with `crypto.PubkeyToAddress`** —
//!    [`LocalSigner::address`]: derives the canonical EIP-55 checksummed
//!    [`crate::address::Address`] from the key's public key. Verified against the
//!    well-known go-ethereum test vector: private key
//!    `59c6995e998f97a5a0044976f0945388cf9b7e5e5f4f9d2d9d8f1f5b7f6d11d1`
//!    (the `testPrivateKey` from `local_test.go`) derives address
//!    `0x96216849c49358B10257cb55b28eA603c874b05E` (the canonical Hardhat/Anvil
//!    account-1 address for that key). Address is computed once at construction
//!    and is never the zero address for a valid key (the Go tests assert
//!    `s.Address() != common.Address{}`).
//!
//! 3. **EIP-1559 signing parity with `types.SignTx` + `LatestSignerForChainID`** —
//!    [`LocalSigner::sign_eip1559`]: given a chain id and an unsigned
//!    [`Eip1559Tx`] (the Rust analogue of go-ethereum's `types.DynamicFeeTx` the
//!    executor builds in `evm_executor.go`), returns a [`SignedTx`] whose
//!    signature **recovers to `self.address()`** and is **bound to the supplied
//!    chain id** (EIP-155 replay protection via the typed-tx chain id field). The
//!    signed payload's leading byte is the EIP-2718 type byte `0x02`
//!    (DynamicFee). The same `(key, chain_id, tx)` triple is deterministic —
//!    signing twice yields identical bytes (RFC-6979 deterministic ECDSA, as
//!    go-ethereum uses).
//!
//! 4. **Signed-tx output is broadcast-ready** — [`SignedTx::raw`] returns the
//!    RLP-encoded `0x02 || rlp([...])` bytes that go straight into
//!    [`crate::rpc::RpcClient::send_raw_transaction`] (Go: `client.SendTransaction(signed)`),
//!    and [`SignedTx::hash`] returns the 32-byte keccak256 tx hash go-ethereum's
//!    `signed.Hash()` produced (the value the executor records as
//!    `step.TxHash`). The hash is over the *signed* typed-tx encoding.
//!
//! 5. **Chain-id binding is observable** — signing the same tx under two
//!    different chain ids produces two different signatures / hashes (the EIP-155
//!    replay-protection property `LatestSignerForChainID` provides). A signature
//!    produced for chain id N does not recover-validate as chain id M.
//!
//! 6. **Typed, no-panic surface** — every fallible entry point returns a
//!    [`crate::Error`] (typed via `defi_errors::Code`), never `unwrap`/`expect`/
//!    `panic` in non-test code. A signing failure maps to
//!    [`defi_errors::Code::Signer`] (Go wrapped sign failures as
//!    `clierr.Wrap(clierr.CodeSigner, "sign transaction", err)` in
//!    `backend_local.go`); an un-parseable key is also a `Signer`-coded error.
//!
//! # Ported Go test cases (and their new home)
//!
//! From `internal/execution/signer/local_test.go`:
//!   - `TestNewLocalSignerFromEnvHex` (key → non-zero address → SignTx succeeds):
//!     the *crypto core* (hex key → address → sign EIP-1559 → recover) is ported
//!     HERE as criteria 1–3; the *env-var plumbing* (`t.Setenv(EnvPrivateKey..)`)
//!     is SKIPPED here and ported in `defi-config`/`defi-execution`.
//!   - `TestNewLocalSignerFromEnvFile`, `…FileAllowsNonStrictPermissions`,
//!     `…AutoUsesDefaultKeyFile`, `TestDefaultPrivateKeyPathUsesXDGConfigHome`,
//!     `TestNewLocalSignerFromInputsPrivateKeyOverride`,
//!     `…OverrideWinsOverFileSource`,
//!     `…MissingKeyErrorIncludesSimplePathHint` → **SKIPPED here** (filesystem /
//!     env / config-precedence; belong to the key-source-resolution module in
//!     `defi-config`/`defi-execution`, per the scope split above).
//!
//! From `internal/execution/signer/tempo_test.go`: ALL skipped here (Tempo 0x76 +
//! `tempo` CLI shell-out are bespoke and ported in the `defi-execution` Tempo
//! RED suite).
//!
//! Fresh spec-driven additions HERE: deterministic-signing, chain-id-binding
//! divergence, EIP-2718 type byte, recover-to-address, and the no-panic typed
//! error surface — none of which the Go unit tests asserted directly but which
//! the machine contract (a correct, recoverable, chain-bound broadcast tx)
//! depends on.

use alloy::consensus::transaction::SignerRecoverable;
use alloy::consensus::{SignableTransaction, Signed, TxEip1559};
use alloy::eips::eip2718::Encodable2718;
use alloy::primitives::{Bytes, TxKind, B256, U256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;
use defi_errors::{Code, Error};

use crate::address::Address;

/// An unsigned EIP-1559 (`DynamicFeeTx`) transaction body.
///
/// The Rust analogue of the `types.DynamicFeeTx` the executor builds in
/// `evm_executor.go`: the fee fields resolved via [`crate::rpc::resolve_tip_cap`]
/// / [`crate::rpc::resolve_fee_cap`], plus `to`/`value`/`input`. Sign it with
/// [`LocalSigner::sign_eip1559`].
#[derive(Debug, Clone)]
pub struct Eip1559Tx {
    /// EIP-155 chain id the signature is bound to.
    pub chain_id: u64,
    /// Account nonce.
    pub nonce: u64,
    /// `maxPriorityFeePerGas` (the tip cap), in wei.
    pub max_priority_fee_per_gas: u128,
    /// `maxFeePerGas` (the fee cap), in wei.
    pub max_fee_per_gas: u128,
    /// Gas limit.
    pub gas_limit: u64,
    /// Destination address; `None` denotes a contract-creation tx.
    pub to: Option<Address>,
    /// Wei value transferred.
    pub value: U256,
    /// Calldata.
    pub input: Vec<u8>,
}

impl Eip1559Tx {
    /// Lower this body into alloy's consensus `TxEip1559`, binding the chain id.
    fn to_consensus(&self, chain_id: u64) -> TxEip1559 {
        TxEip1559 {
            chain_id,
            nonce: self.nonce,
            gas_limit: self.gas_limit,
            max_fee_per_gas: self.max_fee_per_gas,
            max_priority_fee_per_gas: self.max_priority_fee_per_gas,
            to: match self.to {
                Some(addr) => TxKind::Call(addr.into_inner()),
                None => TxKind::Create,
            },
            value: self.value,
            access_list: Default::default(),
            input: Bytes::from(self.input.clone()),
        }
    }
}

/// A signed EIP-1559 transaction, broadcast-ready.
///
/// [`SignedTx::raw`] yields the EIP-2718 (`0x02 || rlp(...)`) bytes that go into
/// `eth_sendRawTransaction`; [`SignedTx::hash`] is the keccak256 tx hash
/// go-ethereum's `signed.Hash()` produced (recorded as `step.TxHash`).
#[derive(Debug, Clone)]
pub struct SignedTx {
    inner: Signed<TxEip1559>,
}

impl SignedTx {
    /// The RLP-encoded EIP-2718 typed-tx bytes (leading byte `0x02`).
    pub fn raw(&self) -> Vec<u8> {
        self.inner.encoded_2718()
    }

    /// The 32-byte keccak256 transaction hash.
    pub fn hash(&self) -> [u8; 32] {
        self.inner.hash().0
    }

    /// Recover the signing address from the signature (must equal the signer's
    /// address; the chain id is bound into the signature for replay protection).
    pub fn recover_signer(&self) -> Result<Address, Error> {
        self.inner
            .recover_signer()
            .map(Address::from)
            .map_err(|e| Error::wrap(Code::Signer, "recover signer", boxed(e)))
    }
}

/// A local secp256k1 signer: a private key plus its derived EIP-55 address.
///
/// Parity with the Go `LocalSigner` (`crypto.HexToECDSA` + `PubkeyToAddress` +
/// `types.SignTx`). Owns only the pure crypto + EVM-tx primitives; key-source
/// orchestration (env/file/keystore precedence) lives in `defi-config` /
/// `defi-execution`.
#[derive(Debug, Clone)]
pub struct LocalSigner {
    inner: PrivateKeySigner,
    address: Address,
}

impl LocalSigner {
    /// Parse a hex secp256k1 private key, parity with `crypto.HexToECDSA`.
    ///
    /// Trims surrounding whitespace, accepts an optional `0x`/`0X` prefix, then
    /// requires exactly 64 hex digits encoding an in-range secp256k1 key.
    /// Rejects empty / non-hex / wrong-length / out-of-range keys with a typed
    /// [`Code::Signer`] error (no panic/unwrap).
    pub fn from_hex(key: &str) -> Result<Self, Error> {
        let trimmed = key.trim();
        let body = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        if body.len() != 64 || !body.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::new(
                Code::Signer,
                "private key must be 64 hex digits",
            ));
        }
        let mut bytes = [0u8; 32];
        for (i, slot) in bytes.iter_mut().enumerate() {
            let hi = nibble(body.as_bytes()[i * 2]);
            let lo = nibble(body.as_bytes()[i * 2 + 1]);
            match (hi, lo) {
                (Some(hi), Some(lo)) => *slot = (hi << 4) | lo,
                _ => {
                    return Err(Error::new(
                        Code::Signer,
                        "private key must be 64 hex digits",
                    ))
                }
            }
        }
        let inner = PrivateKeySigner::from_bytes(&B256::from(bytes))
            .map_err(|e| Error::wrap(Code::Signer, "parse private key", boxed(e)))?;
        let address = Address::from(inner.address());
        Ok(LocalSigner { inner, address })
    }

    /// The EIP-55 checksummed signing address (`crypto.PubkeyToAddress`).
    pub fn address(&self) -> Address {
        self.address
    }

    /// Sign an EIP-1559 transaction bound to `chain_id`, parity with
    /// `types.SignTx(tx, LatestSignerForChainID(chainID), pk)`.
    ///
    /// The returned [`SignedTx`] recovers to [`self.address()`](Self::address),
    /// is chain-id bound (EIP-155 replay protection), and is deterministic for a
    /// given `(key, chain_id, tx)` triple (RFC-6979 ECDSA).
    pub fn sign_eip1559(&self, chain_id: u64, tx: &Eip1559Tx) -> Result<SignedTx, Error> {
        let consensus = tx.to_consensus(chain_id);
        let hash = consensus.signature_hash();
        let signature = self
            .inner
            .sign_hash_sync(&hash)
            .map_err(|e| Error::wrap(Code::Signer, "sign transaction", boxed(e)))?;
        let signed = consensus.into_signed(signature);
        Ok(SignedTx { inner: signed })
    }
}

/// Decode a single ASCII hex digit to its nibble value.
fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// A concrete, `Send + Sync` std error carrying a display message.
///
/// Records the underlying alloy/crypto error text as the `cause` of a typed
/// [`Error`] without depending on each foreign error type implementing the exact
/// `Error + Send + Sync + 'static` bound [`Error::wrap`] requires.
#[derive(Debug)]
struct MsgError(String);

impl std::fmt::Display for MsgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MsgError {}

/// Capture an arbitrary error's display text as a concrete [`MsgError`] cause.
fn boxed<E: std::fmt::Display>(e: E) -> MsgError {
    MsgError(e.to_string())
}

#[cfg(test)]
mod tests {
    //! RED phase: these reference the not-yet-implemented public API of this
    //! module. They MUST fail to compile / fail assertions until GREEN.
    //!
    //! All vectors are deterministic and offline. The signing key is the
    //! well-known go-ethereum / Hardhat test key from `local_test.go`
    //! (`testPrivateKey`); its address is the canonical Anvil account-1 value,
    //! independently reproducible (no network, no fixtures).
    use super::*;

    use alloy::primitives::{Address as AlloyAddress, U256};

    /// The `testPrivateKey` constant from `internal/execution/signer/local_test.go`.
    const TEST_KEY: &str = "59c6995e998f97a5a0044976f0945388cf9b7e5e5f4f9d2d9d8f1f5b7f6d11d1";
    /// The EIP-55 address `crypto.PubkeyToAddress` derives for `TEST_KEY`.
    ///
    /// Verified against the authoritative Go oracle
    /// (`crypto.PubkeyToAddress(crypto.HexToECDSA(TEST_KEY).PublicKey).Hex()`):
    /// the RED draft asserted `0x96216849c49358B10257cb55b28eA603c874b05E`, but
    /// that is the wrong account for this key — go-ethereum (and alloy's
    /// `PrivateKeySigner`) both derive the value below.
    const TEST_ADDR: &str = "0x14DDBd1fe5026E58A12eE8691cAEbFD24bb10eef";

    // Canonical target the Go SignTx test sends to.
    const TARGET: &str = "0x0000000000000000000000000000000000000001";

    /// Build the canonical unsigned EIP-1559 tx the executor would construct
    /// (mirrors the `types.DynamicFeeTx` in `evm_executor.go`).
    fn sample_tx(chain_id: u64) -> Eip1559Tx {
        Eip1559Tx {
            chain_id,
            nonce: 0,
            max_priority_fee_per_gas: 1_000_000_000, // 1 gwei tip
            max_fee_per_gas: 2_000_000_000,          // 2 gwei cap
            gas_limit: 21_000,
            to: Some(crate::address::parse(TARGET).expect("valid target")),
            value: U256::ZERO,
            input: vec![],
        }
    }

    // ===================================================================
    // 1. Hex key parsing parity with crypto.HexToECDSA
    // ===================================================================

    #[test]
    fn from_hex_accepts_bare_key() {
        assert!(LocalSigner::from_hex(TEST_KEY).is_ok());
    }

    #[test]
    fn from_hex_accepts_0x_prefix() {
        let s = LocalSigner::from_hex(&format!("0x{TEST_KEY}")).expect("0x-prefixed key");
        assert_eq!(s.address().to_hex(), TEST_ADDR);
    }

    #[test]
    fn from_hex_accepts_uppercase_prefix_and_whitespace() {
        // Go parseHexKey: TrimSpace then TrimPrefix(.., "0x").
        let s = LocalSigner::from_hex(&format!("  0X{TEST_KEY}  ")).expect("trim + 0X prefix");
        assert_eq!(s.address().to_hex(), TEST_ADDR);
    }

    #[test]
    fn from_hex_rejects_empty() {
        assert!(LocalSigner::from_hex("").is_err());
        assert!(LocalSigner::from_hex("   ").is_err());
        assert!(LocalSigner::from_hex("0x").is_err());
    }

    #[test]
    fn from_hex_rejects_non_hex() {
        assert!(LocalSigner::from_hex("not-a-valid-hex-key").is_err());
        // 64 chars but with a non-hex char.
        let bad = format!("zz{}", &TEST_KEY[2..]);
        assert!(LocalSigner::from_hex(&bad).is_err());
    }

    #[test]
    fn from_hex_rejects_wrong_length() {
        // 63 hex digits (too short) and 65 (too long).
        assert!(LocalSigner::from_hex(&TEST_KEY[..63]).is_err());
        assert!(LocalSigner::from_hex(&format!("{TEST_KEY}a")).is_err());
    }

    #[test]
    fn from_hex_rejects_out_of_range_key() {
        // All-zero key is invalid (not in [1, n-1]); crypto.HexToECDSA rejects it.
        assert!(LocalSigner::from_hex(
            "0000000000000000000000000000000000000000000000000000000000000000"
        )
        .is_err());
    }

    #[test]
    fn from_hex_error_is_signer_coded_and_displayable() {
        let err = LocalSigner::from_hex("not-a-valid-hex-key").unwrap_err();
        assert_eq!(err.code, defi_errors::Code::Signer);
        assert!(!err.to_string().is_empty());
    }

    // ===================================================================
    // 2. Address derivation parity with crypto.PubkeyToAddress
    // ===================================================================

    #[test]
    fn address_matches_go_ethereum_vector() {
        let s = LocalSigner::from_hex(TEST_KEY).expect("valid key");
        assert_eq!(s.address().to_hex(), TEST_ADDR);
    }

    #[test]
    fn address_is_never_zero_for_valid_key() {
        // Mirrors the Go assertion `s.Address() != common.Address{}`.
        let s = LocalSigner::from_hex(TEST_KEY).expect("valid key");
        assert!(!s.address().is_zero());
    }

    #[test]
    fn address_equals_alloy_signer_local_derivation() {
        // The signing address must equal what alloy-signer-local derives for the
        // same key (independent oracle that we wrap the same secp256k1 → address
        // derivation go-ethereum's crypto.PubkeyToAddress performs).
        use alloy::signers::local::PrivateKeySigner;
        let want: PrivateKeySigner = TEST_KEY.parse().expect("alloy local signer from key");
        let want_addr: AlloyAddress = want.address();

        let s = LocalSigner::from_hex(TEST_KEY).expect("valid key");
        assert_eq!(s.address().to_hex(), want_addr.to_checksum(None));
    }

    // ===================================================================
    // 3. EIP-1559 signing parity with types.SignTx + LatestSignerForChainID
    // ===================================================================

    #[test]
    fn sign_eip1559_succeeds_for_chain_one() {
        // Mirrors local_test.go: SignTx(common.Big1, legacy/dynamic tx) succeeds.
        let s = LocalSigner::from_hex(TEST_KEY).expect("valid key");
        assert!(s.sign_eip1559(1, &sample_tx(1)).is_ok());
    }

    #[test]
    fn sign_eip1559_recovers_to_signer_address() {
        let s = LocalSigner::from_hex(TEST_KEY).expect("valid key");
        let signed = s.sign_eip1559(1, &sample_tx(1)).expect("sign");
        assert_eq!(
            signed.recover_signer().expect("recover").to_hex(),
            s.address().to_hex()
        );
    }

    #[test]
    fn sign_eip1559_is_deterministic() {
        // go-ethereum uses RFC-6979 deterministic ECDSA: same input → same bytes.
        let s = LocalSigner::from_hex(TEST_KEY).expect("valid key");
        let a = s.sign_eip1559(1, &sample_tx(1)).expect("sign a");
        let b = s.sign_eip1559(1, &sample_tx(1)).expect("sign b");
        assert_eq!(a.raw(), b.raw());
        assert_eq!(a.hash(), b.hash());
    }

    #[test]
    fn signed_payload_has_eip2718_dynamic_fee_type_byte() {
        let s = LocalSigner::from_hex(TEST_KEY).expect("valid key");
        let signed = s.sign_eip1559(7, &sample_tx(7)).expect("sign");
        let raw = signed.raw();
        assert!(!raw.is_empty(), "raw signed tx must be non-empty");
        assert_eq!(raw[0], 0x02, "EIP-1559 typed tx envelope byte is 0x02");
    }

    // ===================================================================
    // 4. Signed-tx output is broadcast-ready
    // ===================================================================

    #[test]
    fn signed_raw_is_nonempty_and_hash_is_32_bytes() {
        let s = LocalSigner::from_hex(TEST_KEY).expect("valid key");
        let signed = s.sign_eip1559(1, &sample_tx(1)).expect("sign");
        assert!(!signed.raw().is_empty());
        assert_eq!(signed.hash().len(), 32, "tx hash is keccak256 → 32 bytes");
    }

    #[test]
    fn signed_hash_changes_with_nonce() {
        // Different tx contents → different signed hash (sanity the hash covers
        // the payload, not just a constant).
        let s = LocalSigner::from_hex(TEST_KEY).expect("valid key");
        let mut tx2 = sample_tx(1);
        tx2.nonce = 1;
        let h0 = s.sign_eip1559(1, &sample_tx(1)).expect("sign 0").hash();
        let h1 = s.sign_eip1559(1, &tx2).expect("sign 1").hash();
        assert_ne!(h0, h1);
    }

    // ===================================================================
    // 5. Chain-id binding is observable (EIP-155 replay protection)
    // ===================================================================

    #[test]
    fn different_chain_ids_produce_different_signatures() {
        let s = LocalSigner::from_hex(TEST_KEY).expect("valid key");
        let on_1 = s.sign_eip1559(1, &sample_tx(1)).expect("sign chain 1");
        let on_10 = s.sign_eip1559(10, &sample_tx(10)).expect("sign chain 10");
        assert_ne!(
            on_1.raw(),
            on_10.raw(),
            "chain id must bind into the signature"
        );
        assert_ne!(on_1.hash(), on_10.hash());
    }

    #[test]
    fn signing_still_recovers_per_chain() {
        // Recovery must hold independent of chain id (the signer address is
        // chain-agnostic; only replay protection differs).
        let s = LocalSigner::from_hex(TEST_KEY).expect("valid key");
        for cid in [1u64, 10, 8453, 42161] {
            let signed = s.sign_eip1559(cid, &sample_tx(cid)).expect("sign");
            assert_eq!(
                signed.recover_signer().expect("recover").to_hex(),
                s.address().to_hex(),
                "recovery failed for chain {cid}"
            );
        }
    }

    // ===================================================================
    // 6. Typed, no-panic surface
    // ===================================================================

    #[test]
    fn from_hex_never_panics_on_garbage() {
        // A spread of malformed inputs must all return Err, never panic.
        for bad in [
            "",
            "   ",
            "0x",
            "g",
            "0xZZ",
            "12345",
            &"f".repeat(63),
            &"f".repeat(65),
        ] {
            assert!(
                LocalSigner::from_hex(bad).is_err(),
                "expected Err for {bad:?}"
            );
        }
    }
}
