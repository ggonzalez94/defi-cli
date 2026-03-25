# Open Wallet Migration Design

Date: 2026-03-25

## Summary

Replace `defi-cli`'s default raw-key execution model with an OWS-first wallet model built for agents. New execution flows become wallet-based and agent-token-gated. Legacy local-key execution remains temporarily available as a deprecated compatibility lane, but the architecture and UX center on OWS from the start.

## Context

Issue `#49` asks to replace wallet management with Open Wallet Standard (OWS).

Today, `defi-cli` execution is built around signer backends:

- `local` signer loads raw EVM private keys from env, file, default `~/.config/defi/key.hex`, or keystore
- `tempo` signer shells out to the Tempo CLI and reconstructs a signer in-process

This has three problems:

1. The primary model assumes raw key access in the `defi-cli` process.
2. Authorization and policy are CLI-local concerns rather than wallet-layer concerns.
3. The current UX is address-first (`--from-address`) rather than wallet-first.

OWS provides a better long-term model for agents:

- wallet identity is stable and local-first
- agents use scoped API tokens instead of raw keys
- policies are enforced before key material is decrypted
- owner credentials and agent credentials have different security semantics

## Goals

- Make OWS the default execution path for new actions.
- Make wallet identity the primary execution input.
- Keep the normal execution path agent-only.
- Fail closed when wallet or chain access is not allowed by OWS policy.
- Preserve `defi-cli` transaction-shape guardrails and action lifecycle.
- Limit backward compatibility to a narrow, deprecated legacy path.

## Non-Goals

- No owner-mode signing in the normal `submit` path.
- No `defi-cli` policy override for OWS authorization decisions.
- No profile system layered on top of OWS in the first migration.
- No implicit upgrade of legacy actions into OWS-backed actions.
- No immediate Tempo unification if OWS does not support Tempo-native execution cleanly.

## Approved Decisions

### 1. Identity model

New execution flows are wallet-based.

- Plan commands accept a single identity input: `--wallet`
- `--wallet` accepts either an OWS wallet name or OWS wallet ID
- Planning resolves the wallet immediately and fails unless the resolution is exact
- The resolved chain-specific sender address is used for planning
- Planned actions persist:
  - `wallet_id` as the stable authority
  - `wallet_name` as optional display metadata
  - `from_address` as the resolved sender used by existing planners and checks
- Submit does not re-resolve by wallet name; it uses the persisted `wallet_id`

This keeps the UX minimal for agents while making persistence machine-safe.

### 2. Agent-only execution

The normal `defi-cli submit` path is agent-only.

- `defi-cli` accepts OWS agent tokens for authorized execution
- owner-mode signing is not supported in the normal `submit` flow
- if owner-mode is ever needed later, it must be an explicit admin/emergency surface, not a hidden alternate credential path in `submit`

This keeps the execution security model clear: normal automation uses delegated agent access only.

### 3. Policy boundary

OWS is the authority for signer authorization.

- OWS decides whether an agent token may use a wallet on a chain
- `defi-cli` does not add a policy override flag
- `defi-cli` keeps its own transaction-shape and provider guardrails

This creates a clean separation:

- OWS answers "may this agent sign?"
- `defi-cli` answers "does this planned action still match expected transaction safety constraints?"

### 4. Fail-closed behavior

When a token lacks access:

- no fallback to owner-mode
- no fallback to legacy local signing
- no automatic wallet switching
- no automatic profile/token selection

The remediation is outside `defi-cli`: create or update policy, mint a new token, then retry with delegated access.

### 5. Legacy compatibility strategy

Backward compatibility is intentionally narrow.

- legacy local signing remains temporarily available as deprecated compatibility
- new flows are OWS-native by default
- old actions keep running for a deprecation window, but only through the legacy lane
- new actions should not be executable through either backend interchangeably

Compatibility means "old automations do not break immediately," not "two permanent first-class architectures."

### 6. Tempo exception

Tempo remains a temporary exception in the first OWS migration.

Current Tempo execution depends on Tempo-specific smart-wallet and transaction behavior. OWS does not currently appear to cover Tempo-native type `0x76` transaction execution as a drop-in replacement.

Decision:

- standard EVM execution moves to OWS-first flows
- Tempo keeps its explicit backend for now
- the user-facing wallet-first identity model should still remain coherent
- Tempo can be unified later if OWS gains native support

## Proposed Architecture

### OWS integration boundary

Add a dedicated OWS integration package that owns:

- wallet reference resolution (`--wallet` name or ID -> exact wallet)
- chain/account address lookup for planning
- token-backed execution via OWS
- translation of OWS failures into stable `defi-cli` errors

This avoids leaking OWS logic into command handlers and planners.

### Action model changes

Actions gain explicit execution identity and backend metadata for new flows.

Recommended fields:

- `wallet_id`
- `wallet_name` optional
- `execution_backend`

Backend values:

- `ows` for new OWS-backed actions
- `legacy_local` for deprecated compatibility actions
- Tempo-specific backend remains separate as needed

This is better than inferring behavior from whichever flags happen to be present at submit time.

### Planner interaction

Planners should remain mostly unchanged.

They should continue to receive a resolved sender address and produce the same deterministic action structure they do today. OWS should be resolved before planner invocation rather than pushed down into every planner.

### Submit interaction

Submit should route by persisted backend, not by heuristic runtime reconstruction.

- OWS-backed actions submit through the OWS backend using the persisted `wallet_id`
- legacy actions submit through the deprecated local-key path
- Tempo actions continue to use the Tempo-specific backend until explicitly migrated

## UX Model

### New path

Plan:

```bash
defi <intent> ... --wallet <wallet-ref>
```

Submit:

```bash
DEFI_OWS_TOKEN=ows_key_... defi <intent> submit --action-id <id>
```

Normal UX rules:

- `--wallet` is the only primary execution identity input
- no signer-selection flags in the primary path
- no `--from-address` in the normal path
- token comes from env only

### Plan output

Plan output should echo resolved identity clearly:

- `wallet_id`
- `wallet_name` when available
- `from_address`

This helps agents move from human-friendly refs to stable identifiers immediately.

## Error Handling

### OWS authorization errors

OWS authorization failures should map to `action_policy_error` rather than generic signer failure when the problem is policy denial rather than infrastructure.

Error payloads should include:

- resolved `wallet_id`
- requested chain ID
- denial source (`ows_policy`)

### Other failure classes

- missing token, malformed token, expired token, or OWS unavailability should fail before execution starts
- chain/account resolution failures during planning should fail before an action is persisted
- transaction-shape guardrails remain active regardless of backend

## Onboarding

Onboarding happens outside the `submit` path:

1. create or import wallet with `ows`
2. create policy
3. mint agent API key
4. fund wallet
5. use `defi-cli ... --wallet <ref>`

This adds one-time setup, but keeps runtime execution clear and auditable.

## Migration Plan

### Phase 1: Introduce OWS-first planning

- add `--wallet` to plan commands
- resolve wallet through OWS
- persist `wallet_id`, optional `wallet_name`, `from_address`, and `execution_backend = "ows"`
- keep legacy flags and local submission available temporarily for compatibility
- make new docs/examples OWS-first immediately

### Phase 2: Make wallet-based UX primary

- make `--wallet` the primary required identity input for new planning flows
- remove `--from-address` from the normal UX
- make submit for OWS-backed actions rely on persisted `wallet_id` plus env token only
- emit deprecation warnings for legacy local-key usage

### Phase 3: Remove legacy path

- remove local signer and raw-key inputs
- continue treating Tempo as explicit exception until separately migrated

## Technical Debt Controls

To avoid introducing a long-lived dual architecture:

- new OWS path is the only path that receives product improvements
- legacy support is limited to compatibility for old actions
- planners and core submit flow should not deeply model both identity systems
- no mixed-mode actions that can switch between OWS and local at submit time
- no implicit upgrade from legacy actions to OWS actions

Compatibility is temporary and contained.

## Testing Strategy

### Wallet resolution

- wallet ID resolves directly
- wallet name resolves uniquely
- ambiguous wallet refs fail
- missing wallet refs fail
- missing chain account fails clearly

### Action persistence

- OWS-planned actions store `wallet_id`, `from_address`, and backend marker
- legacy actions without `wallet_id` remain bound to the deprecated path only

### Execution and error mapping

- missing token
- expired token
- wallet not in scope
- chain denied by policy
- OWS unavailable
- error mapping to stable envelopes and exit codes

### Smoke coverage

- at least one OWS-backed EVM execution path end-to-end
- Tempo explicitly verified as exception behavior, not accidentally routed into unsupported OWS logic

## Risks

### Onboarding friction

OWS introduces more setup before first execution. This is acceptable because the payoff is a cleaner and more secure agent execution model.

### Breakage risk for existing automation

Changing the default execution model will break existing local-key-centered setups unless migration is explicit and well documented. This is why the legacy lane exists temporarily.

### Tempo divergence

Tempo will remain a special case until OWS support is clear. This is acceptable as long as the exception is explicit and isolated.

## Recommendation

Proceed with an OWS-first, wallet-based execution model centered on `--wallet`, persisted `wallet_id`, agent-only submit, fail-closed policy behavior, and a narrow deprecated compatibility lane for legacy local signing.
