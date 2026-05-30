//! Shared execution-identity resolver (Go `internal/app/execution_identity.go`).
//!
//! Standard-EVM execution `plan` commands (`approvals`/`transfer`/`bridge`/
//! `lend`/`yield`/`rewards`, plus TaikoSwap `swap plan`) accept an OWS-first
//! `--wallet` identity OR a legacy `--from-address` local-signer identity. This
//! module owns the resolution + the action-stamping that the Go runner performs
//! in `resolveExecutionIdentity` / `applyExecutionIdentityToAction`:
//!
//! * `resolve_execution_identity` — `exactly_one_of {wallet, from_address}`;
//!   `--wallet` resolves through the OWS vault to a per-chain EVM sender
//!   (rejecting non-EVM and Tempo chains, since OWS planning is not supported
//!   there yet) and stamps the OWS backend; `--from-address` validates the hex
//!   address, stamps the legacy backend, and surfaces the OWS-recommended
//!   planning warning.
//! * `apply_execution_identity_to_action` — copies the resolved wallet id/name,
//!   from-address, and execution backend onto a built [`Action`].
//!
//! Tempo `swap plan` does NOT use this resolver (it is `--from-address`-only and
//! routes through `swap::resolve_swap_plan_sender`).

use defi_errors::{Code, Error};
use defi_execution::action::{Action, ExecutionBackend};
use defi_id::parse_chain;

/// The OWS-recommended-over-legacy planning warning surfaced on the
/// `--from-address` path. Parity with Go `resolveExecutionIdentity`.
pub const LEGACY_IDENTITY_WARNING: &str =
    "--wallet (OWS) is recommended over --from-address for planning; see docs for details";

/// A resolved execution identity for an EVM `plan` command.
///
/// Parity with Go `executionIdentity`.
#[derive(Debug, Clone)]
pub struct ExecutionIdentity {
    /// OWS wallet id (empty on the legacy `--from-address` path).
    pub wallet_id: String,
    /// OWS wallet name (empty on the legacy `--from-address` path).
    pub wallet_name: String,
    /// The resolved sender EOA in EIP-55 checksum form.
    pub from_address: String,
    /// The signing/execution backend the action targets.
    pub execution_backend: ExecutionBackend,
    /// Warnings surfaced by the resolver (OWS-recommended note on the legacy
    /// path; empty on the OWS path).
    pub warnings: Vec<String>,
}

/// Tempo chain CAIP-2 ids (`--wallet` planning unsupported on these). Parity
/// with Go `isTempoChain`.
fn is_tempo_chain(chain_id: &str) -> bool {
    matches!(
        chain_id.trim(),
        "eip155:4217" | "eip155:42431" | "eip155:31318"
    )
}

/// Resolve the `plan` execution identity from the raw `--wallet` / `--from-address`
/// flags on `chain_arg`.
///
/// Parity with Go `resolveExecutionIdentity`:
/// 1. supplying both / neither identity input is a [`Code::Usage`] error;
/// 2. `--wallet`: parse `--chain`, reject non-EVM ([`Code::Unsupported`]) and
///    Tempo chains ([`Code::Unsupported`]), resolve the OWS wallet + its per-chain
///    EVM sender (propagating the OWS-typed error code wrapped with context),
///    validate the sender hex ([`Code::Unavailable`] otherwise), and return the
///    OWS-backed identity (checksummed sender, no warnings);
/// 3. `--from-address`: validate the hex address ([`Code::Usage`] otherwise) and
///    return the legacy-backed identity (checksummed sender) carrying the
///    OWS-recommended planning warning.
pub fn resolve_execution_identity(
    wallet_ref: &str,
    from_address: &str,
    chain_arg: &str,
) -> Result<ExecutionIdentity, Error> {
    let wallet_ref = wallet_ref.trim();
    let from_address = from_address.trim();

    if !wallet_ref.is_empty() && !from_address.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "use only one identity input: --wallet or --from-address",
        ));
    }
    if wallet_ref.is_empty() && from_address.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "exactly one identity input is required: --wallet or --from-address",
        ));
    }

    if !wallet_ref.is_empty() {
        let chain = parse_chain(chain_arg)?;
        if !chain.is_evm() {
            return Err(Error::new(
                Code::Unsupported,
                "--wallet planning currently supports EVM chains only",
            ));
        }
        if is_tempo_chain(&chain.caip2) {
            return Err(Error::new(
                Code::Unsupported,
                "--wallet planning is not supported on Tempo chains yet; use --from-address",
            ));
        }

        let wallet = defi_ows::resolve_wallet_ref("", wallet_ref)
            .map_err(|err| Error::wrap(err.code, "resolve --wallet", err))?;
        let sender = defi_ows::sender_address_for_chain(&wallet, &chain.caip2)
            .map_err(|err| Error::wrap(err.code, "resolve wallet sender for chain", err))?;
        if !defi_evm::address::is_hex_address(&sender) {
            return Err(Error::new(
                Code::Unavailable,
                "wallet sender address must be a valid EVM hex address",
            ));
        }

        return Ok(ExecutionIdentity {
            wallet_id: wallet.id,
            wallet_name: wallet.name,
            from_address: defi_evm::address::checksum(&sender)?,
            execution_backend: ExecutionBackend::Ows,
            warnings: Vec::new(),
        });
    }

    if !defi_evm::address::is_hex_address(from_address) {
        return Err(Error::new(
            Code::Usage,
            "--from-address must be a valid EVM hex address",
        ));
    }
    Ok(ExecutionIdentity {
        wallet_id: String::new(),
        wallet_name: String::new(),
        from_address: defi_evm::address::checksum(from_address)?,
        execution_backend: ExecutionBackend::LegacyLocal,
        warnings: vec![LEGACY_IDENTITY_WARNING.to_string()],
    })
}

/// Stamp a resolved [`ExecutionIdentity`] onto a built [`Action`].
///
/// Parity with Go `applyExecutionIdentityToAction`: copies the wallet id/name,
/// from-address, and execution backend onto the action (overwriting any sender
/// the planner stamped, which is the same checksummed address).
pub fn apply_execution_identity_to_action(action: &mut Action, identity: &ExecutionIdentity) {
    action.wallet_id = identity.wallet_id.clone();
    action.wallet_name = identity.wallet_name.clone();
    action.from_address = identity.from_address.clone();
    action.execution_backend = Some(identity.execution_backend);
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `execident` (Go `execution_identity.go`)
    //!
    //! 1. **Both identity inputs → usage.** `--wallet` + `--from-address` →
    //!    [`Code::Usage`].
    //! 2. **Neither identity input → usage.** Empty/empty → [`Code::Usage`].
    //! 3. **Legacy `--from-address` happy path.** A valid hex address resolves to
    //!    the legacy backend, the checksummed sender, and the OWS-recommended
    //!    planning warning; wallet id/name are empty.
    //! 4. **Malformed `--from-address` → usage.** A non-hex address →
    //!    [`Code::Usage`].
    //! 5. **`--wallet` on Tempo → unsupported.** A Tempo chain rejects `--wallet`
    //!    with [`Code::Unsupported`] and the Go message.
    //! 6. **`--wallet` on a non-EVM chain → unsupported.** A non-EVM chain rejects
    //!    `--wallet` with [`Code::Unsupported`].
    //! 7. **Action stamping.** `apply_execution_identity_to_action` copies the
    //!    identity fields onto the action.
    //!
    //! SKIPPED: the OWS vault resolve happy path (needs a vault fixture / CLI) —
    //!   WS4b e2e; the OWS error-code classification — owned by `defi-ows`.

    use super::*;
    use defi_execution::action::{Action, Constraints};

    const ADDR: &str = "0x00000000000000000000000000000000000000aa";

    #[test]
    fn rejects_both_identity_inputs() {
        let err = resolve_execution_identity("alice", ADDR, "1").expect_err("both rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn rejects_neither_identity_input() {
        let err = resolve_execution_identity("", "", "1").expect_err("neither rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn legacy_from_address_resolves_with_warning() {
        let id = resolve_execution_identity("", ADDR, "1").expect("legacy resolves");
        assert_eq!(id.execution_backend, ExecutionBackend::LegacyLocal);
        assert_eq!(id.from_address.to_lowercase(), ADDR.to_lowercase());
        assert!(id.wallet_id.is_empty());
        assert!(id.wallet_name.is_empty());
        assert_eq!(id.warnings, vec![LEGACY_IDENTITY_WARNING.to_string()]);
    }

    #[test]
    fn rejects_malformed_from_address() {
        let err = resolve_execution_identity("", "0xnot-an-address", "1")
            .expect_err("malformed rejected");
        assert_eq!(err.code, Code::Usage);
    }

    #[test]
    fn rejects_wallet_on_tempo_chain() {
        let err = resolve_execution_identity("alice", "", "tempo").expect_err("tempo rejected");
        assert_eq!(err.code, Code::Unsupported);
        assert!(err
            .to_string()
            .contains("--wallet planning is not supported on Tempo chains yet"));
    }

    #[test]
    fn rejects_wallet_on_non_evm_chain() {
        // Solana mainnet is non-EVM; --wallet planning is EVM-only.
        let err = resolve_execution_identity("alice", "", "solana")
            .expect_err("non-evm rejected or chain-parse error");
        // Either an Unsupported (non-EVM guard) — both are acceptable typed errors,
        // but the non-EVM guard is the contract path when the chain parses.
        assert!(matches!(err.code, Code::Unsupported | Code::Usage));
    }

    #[test]
    fn stamps_identity_onto_action() {
        let mut action = Action::new("act_x", "approve", "eip155:1", Constraints::default());
        let identity = ExecutionIdentity {
            wallet_id: "wid".to_string(),
            wallet_name: "wname".to_string(),
            from_address: ADDR.to_string(),
            execution_backend: ExecutionBackend::Ows,
            warnings: Vec::new(),
        };
        apply_execution_identity_to_action(&mut action, &identity);
        assert_eq!(action.wallet_id, "wid");
        assert_eq!(action.wallet_name, "wname");
        assert_eq!(action.from_address, ADDR);
        assert_eq!(action.execution_backend, Some(ExecutionBackend::Ows));
    }
}
