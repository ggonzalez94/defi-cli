//! WS4b — Open Wallet Standard end-to-end contract checks against the *real*
//! `ows` CLI.
//!
//! The unit tests in `src/lib.rs` exercise `send_unsigned_tx` / vault resolution
//! against an injected [`defi_ows::CommandRunner`] fake, which pins the *Rust*
//! side of the contract (exact `ows sign send-tx` arg vector, env injection,
//! failure classification, tx-hash parsing, vault-metadata decoding). What they
//! cannot prove is that the *real* `ows` binary actually accepts that arg vector
//! and emits metadata in the shape the Rust structs decode. This integration
//! test closes that gap **without broadcasting** (no funds, no live RPC, no
//! passphrase), so it stays deterministic and safe.
//!
//! ## CI safety / skip behavior
//!
//! `ows` is an external, optional dependency that is absent on CI runners. Every
//! test here **skips gracefully** (returns early after printing to stderr) when
//! `ows` is not on `PATH`. The always-on coverage remains the mocked unit tests;
//! this file only *adds* signal when a real `ows` is installed locally
//! (`which ows`).
//!
//! ## What is (and is not) covered, and why
//!
//! Covered against the real binary:
//!  * the `ows sign send-tx` flag surface (`--wallet --chain --tx --json
//!    [--rpc-url]`) matches exactly what [`defi_ows::build_send_tx_args`]
//!    produces — asserted both from `--help` and by driving the real binary with
//!    that exact vector against a non-existent wallet so it fails *before* any
//!    broadcast;
//!  * the Rust failure classification (`Code::Signer` for a generic command
//!    failure) holds when the real binary rejects the request;
//!  * a wallet-metadata file written in the real on-disk format (including the
//!    `ows_version` field the structs intentionally ignore) round-trips through
//!    [`defi_ows::load_wallets`] / [`defi_ows::resolve_wallet_ref`] /
//!    [`defi_ows::sender_address_for_chain`].
//!
//! Deliberately **not** covered here (documented blocker — see the bottom of this
//! file): a true signing+broadcast round-trip through `ows sign send-tx`. That
//! needs an unlocked wallet passphrase, on-chain funds, and a live RPC, none of
//! which belong in an offline/deterministic test. The success-path JSON decoding
//! (`tx_hash`/`txHash`) is pinned by the mocked unit tests instead.

use std::path::PathBuf;
use std::process::Command;

use defi_ows::{
    build_send_tx_args, load_wallets, resolve_wallet_ref, send_unsigned_tx,
    sender_address_for_chain, Code, CommandOutput, CommandRunner, Wallet, WalletAccount,
};

/// A production-shaped [`CommandRunner`] that shells out to the real `ows`
/// binary. This is the missing real analogue of the unit tests' `FakeRunner`:
/// it resolves `ows` on `PATH` and runs it as a child process, capturing
/// stdout/stderr/exit so [`send_unsigned_tx`]'s classification + parsing run
/// against genuine CLI output.
struct RealOwsRunner;

impl CommandRunner for RealOwsRunner {
    fn look_path(&self, file: &str) -> Result<String, std::io::Error> {
        which(file).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "executable file not found in $PATH",
            )
        })
    }

    fn run(&self, bin: &str, args: &[String], env: &[(String, String)]) -> CommandOutput {
        let mut cmd = Command::new(bin);
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        match cmd.output() {
            Ok(out) => CommandOutput {
                stdout: out.stdout,
                stderr: out.stderr,
                run_error: if out.status.success() {
                    None
                } else {
                    Some(format!("exit status {}", out.status))
                },
            },
            Err(err) => CommandOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                run_error: Some(err.to_string()),
            },
        }
    }
}

/// Minimal `PATH` lookup (no external crate): returns the first executable
/// entry named `file` across `PATH`, like `exec.LookPath`.
fn which(file: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(file);
        if is_executable_file(&candidate) {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(unix)]
fn is_executable_file(path: &PathBuf) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(md) => md.is_file() && (md.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(path: &PathBuf) -> bool {
    path.is_file()
}

/// Resolve the real `ows` binary, returning `None` (and logging) when it is not
/// installed so the caller can skip without failing CI.
fn ows_bin_or_skip(test: &str) -> Option<String> {
    match which("ows") {
        Some(p) => Some(p),
        None => {
            eprintln!(
                "[skip] {test}: `ows` not found on PATH; \
                 run `which ows` to install (https://github.com/openwalletstandard) \
                 then re-run `cargo test -p defi-ows --test ows_cli_e2e` for real-CLI coverage"
            );
            None
        }
    }
}

/// The real `ows sign send-tx --help` usage text must name exactly the flags the
/// Rust arg builder emits. This catches an upstream rename of any of
/// `--wallet`/`--chain`/`--tx`/`--json`/`--rpc-url` that the mocked unit tests
/// (which assert the Rust side only) could never see.
#[test]
fn real_ows_send_tx_help_lists_the_flags_we_build() {
    let Some(bin) = ows_bin_or_skip("real_ows_send_tx_help_lists_the_flags_we_build") else {
        return;
    };

    let out = Command::new(&bin)
        .args(["sign", "send-tx", "--help"])
        .output()
        .expect("spawn ows sign send-tx --help");
    // `--help` exits 0 and prints usage to stdout.
    assert!(
        out.status.success(),
        "`ows sign send-tx --help` should exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let help = String::from_utf8_lossy(&out.stdout);

    for flag in ["--wallet", "--chain", "--tx", "--json", "--rpc-url"] {
        assert!(
            help.contains(flag),
            "real `ows sign send-tx --help` must document {flag}; \
             a rename here breaks build_send_tx_args. help:\n{help}"
        );
    }
}

/// Drive the *real* `ows` through [`send_unsigned_tx`] with the exact arg vector
/// the Rust builder produces, targeting a wallet that does not exist. The real
/// binary parses the args and fails on wallet lookup **before any broadcast**,
/// which proves (a) the arg contract is accepted end-to-end and (b) the Rust
/// failure classification maps a generic non-zero exit to [`Code::Signer`].
///
/// This is the safe, deterministic substitute for a real signing round-trip: a
/// missing wallet can never spend funds or touch an RPC.
#[test]
fn real_ows_accepts_arg_vector_and_classifies_failure() {
    let test = "real_ows_accepts_arg_vector_and_classifies_failure";
    if ows_bin_or_skip(test).is_none() {
        return;
    }

    let runner = RealOwsRunner;
    // A UUID-shaped name that cannot collide with a real vault wallet.
    let bogus_wallet = "defi-e2e-00000000-0000-4000-8000-000000000000";
    let err = send_unsigned_tx(
        &runner,
        Some("not-a-real-passphrase"),
        bogus_wallet,
        "eip155:1",
        &[0x01, 0x02, 0x03],
        // An unroutable RPC: the wallet lookup fails first, but even if ordering
        // changed upstream this endpoint resolves to nothing.
        "http://127.0.0.1:1/defi-e2e-must-not-broadcast",
    )
    .expect_err("a non-existent wallet must make `ows sign send-tx` fail");

    // The real binary rejects an unknown wallet; the Rust side classifies any
    // non-policy command failure as Code::Signer (see classify_command_failure).
    assert_eq!(
        err.code,
        Code::Signer,
        "real ows wallet-not-found failure must classify as Signer; got: {err}"
    );
    // Sanity: the surfaced detail should mention the wallet (the real binary's
    // "wallet not found: '<name>'" message), confirming we actually reached and
    // parsed the real CLI's stderr rather than short-circuiting.
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("send-tx") || msg.contains("wallet") || msg.contains("not found"),
        "error should surface the real ows failure detail; got: {err}"
    );
}

/// The arg vector the Rust builder emits is exactly the positional/flag order
/// the real binary's usage string declares
/// (`send-tx [OPTIONS] --chain <CHAIN> --wallet <WALLET> --tx <TX>`). We assert
/// our builder uses those flag *names* (order is flag-based, not positional, so
/// the binary accepts any ordering — verified by the run test above).
#[test]
fn build_send_tx_args_uses_real_flag_names() {
    let args = build_send_tx_args(
        "wallet-ref",
        "eip155:8453",
        &[0xde, 0xad, 0xbe, 0xef],
        "https://rpc.example",
    );
    // Sanity-pin the exact vector the real binary is driven with.
    let want: Vec<String> = [
        "sign",
        "send-tx",
        "--wallet",
        "wallet-ref",
        "--chain",
        "eip155:8453",
        "--tx",
        "0xdeadbeef",
        "--json",
        "--rpc-url",
        "https://rpc.example",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    assert_eq!(args, want);
}

/// A wallet-metadata file written in the **real** on-disk format that the live
/// `ows` vault uses (taken verbatim from `~/.ows/wallets/*.json`: an
/// `ows_version` field, an `account_id` of the form `<chain>:<addr>`, and the
/// full multi-chain account list) must decode through the Rust vault helpers.
/// The structs intentionally ignore `ows_version`; this pins that tolerance
/// against the actual schema rather than a hand-rolled fixture.
#[test]
fn real_format_wallet_metadata_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wallets = dir.path().join("wallets");
    std::fs::create_dir_all(&wallets).expect("mkdir wallets");

    // Verbatim shape of a real `ows` vault wallet file (see
    // `~/.ows/wallets/<id>.json`): note the leading `ows_version`, the
    // `<chain>:<addr>` `account_id`s, and a non-EVM (solana) account mixed in.
    let real_format = r#"{
        "ows_version": 2,
        "id": "defi-e2e-001c33d3-0088-4768-bc80-24275bc27e91",
        "name": "defi-e2e-wallet",
        "created_at": "2026-04-14T18:33:18.829006+00:00",
        "accounts": [
            {
                "account_id": "eip155:1:0x8b9271867dD72d53a3CEBfC045821De8AaB0A764",
                "address": "0x8b9271867dD72d53a3CEBfC045821De8AaB0A764",
                "chain_id": "eip155:1",
                "derivation_path": "m/44'/60'/0'/0/0"
            },
            {
                "account_id": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp:2K4ok3PBXbVJSYkRGrR8Nyjaj9m8ufvUHxAeYWYdjY3v",
                "address": "2K4ok3PBXbVJSYkRGrR8Nyjaj9m8ufvUHxAeYWYdjY3v",
                "chain_id": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                "derivation_path": "m/44'/501'/0'/0'"
            }
        ]
    }"#;
    std::fs::write(
        wallets.join("defi-e2e-001c33d3-0088-4768-bc80-24275bc27e91.json"),
        real_format,
    )
    .expect("write real-format wallet fixture");

    // load_wallets tolerates the extra `ows_version` field.
    let loaded = load_wallets(dir.path()).expect("load real-format wallet");
    assert_eq!(loaded.len(), 1, "exactly one fixture wallet");
    let w = &loaded[0];
    assert_eq!(w.id, "defi-e2e-001c33d3-0088-4768-bc80-24275bc27e91");
    assert_eq!(w.name, "defi-e2e-wallet");
    assert_eq!(w.accounts.len(), 2, "both accounts decode");
    assert_eq!(
        w.accounts[0].account_id,
        "eip155:1:0x8b9271867dD72d53a3CEBfC045821De8AaB0A764"
    );

    // resolve_wallet_ref against the real on-disk layout (vault root -> wallets/).
    let vault_dir = dir.path().to_str().expect("utf8 vault path");
    let by_id = resolve_wallet_ref(vault_dir, "defi-e2e-001c33d3-0088-4768-bc80-24275bc27e91")
        .expect("resolve real-format wallet by id");
    assert_eq!(by_id.name, "defi-e2e-wallet");
    let by_name = resolve_wallet_ref(vault_dir, "defi-e2e-wallet")
        .expect("resolve real-format wallet by name");
    assert_eq!(by_name.id, "defi-e2e-001c33d3-0088-4768-bc80-24275bc27e91");

    // sender_address_for_chain: exact EVM match, EVM family fallback, and the
    // non-EVM rejection — all against the real metadata shape.
    assert_eq!(
        sender_address_for_chain(&by_id, "eip155:1").expect("exact evm match"),
        "0x8b9271867dD72d53a3CEBfC045821De8AaB0A764"
    );
    assert_eq!(
        sender_address_for_chain(&by_id, "eip155:8453").expect("evm family fallback"),
        "0x8b9271867dD72d53a3CEBfC045821De8AaB0A764",
        "an eip155 request with no exact match falls back to any eip155 account"
    );
    let err = sender_address_for_chain(&by_id, "bitcoin:000000000019d6689c085ae165831e93")
        .expect_err("non-evm chain with no matching account must fail");
    assert_eq!(err.code, Code::Usage);
}

/// Type-level guard: the real-runner used above conforms to [`CommandRunner`]
/// and the wallet structs are constructible from the public API, so this test
/// file keeps compiling against the same surface `defi-app` consumes. (Pure
/// compile/no-network assertion.)
#[test]
fn public_surface_is_stable() {
    let _runner: &dyn CommandRunner = &RealOwsRunner;
    let _w = Wallet {
        id: "w".into(),
        name: "n".into(),
        created_at: "t".into(),
        accounts: vec![WalletAccount {
            account_id: "eip155:1:0xabc".into(),
            address: "0xabc".into(),
            chain_id: "eip155:1".into(),
            derivation_path: "m/44'/60'/0'/0/0".into(),
        }],
    };
    assert_eq!(_w.accounts.len(), 1);
}

// =============================================================================
// DOCUMENTED BLOCKER — full signing/broadcast round-trip (deferred)
//
// A true Rust -> `ows sign send-tx` -> on-chain broadcast e2e is intentionally
// NOT implemented here. Two independent blockers:
//
//  1. Production wiring gap: `OwsSubmitBackend` (in `defi-execution`) dispatches
//     the encoded unsigned tx through an injectable `send_hook` that is left
//     unset in production builds ("wallet-backed submit is not available in this
//     build"). Nothing in `defi-app` constructs a real `CommandRunner` and binds
//     `send_hook` to `defi_ows::send_unsigned_tx`. Until that glue exists, the
//     real broadcast path cannot be reached from the binary; `RealOwsRunner`
//     above is the reference impl for that future wiring.
//
//  2. Environmental: a real broadcast needs an unlocked wallet passphrase
//     (`OWS_PASSPHRASE` / `DEFI_OWS_TOKEN`), on-chain funds for gas, and a live
//     RPC — none of which can run offline/deterministically in CI.
//
// How to run the real round-trip manually once (1) is wired:
//
//   # create a throwaway wallet and fund it on a testnet
//   ows wallet create --name defi-e2e
//   # plan an action with the Rust binary using --wallet defi-e2e
//   defi transfer plan --wallet defi-e2e --chain eip155:11155111 ...
//   # broadcast (real ows shell-out), passphrase via env
//   DEFI_OWS_TOKEN=<passphrase> defi transfer submit --action <id>
//
// The success-path JSON contract (`tx_hash` snake / `txHash` camel, chain
// fallback, malformed-hash rejection) is already pinned by the mocked unit
// tests in `src/lib.rs` (criteria B/C/D), which this file complements rather
// than duplicates.
// =============================================================================
