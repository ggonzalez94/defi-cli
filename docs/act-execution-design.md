# Execution Component Design (`plan|run|submit|status`)

Status: Implemented (v1)  
Last Updated: 2026-02-24  
Scope: Current implementation in this branch (not a forward-looking proposal)

## 1. Purpose

`defi-cli` started as read-only retrieval. The execution component adds safe transaction workflows while preserving the existing CLI contract:

- Stable envelope output
- Deterministic command semantics
- Clear execution lifecycle and resumability

Execution is integrated inside existing domain commands (for example `swap`, `bridge`, `lend`) instead of a separate top-level `act` namespace.

## 2. Current Execution Surface

| Domain | Commands | Selector Requirement | Execution Coverage |
|---|---|---|---|
| Swap | `swap plan|run|submit|status` | `--provider` required | `taikoswap` execution today |
| Bridge | `bridge plan|run|submit|status` | `--provider` required | `across`, `lifi` execution |
| Lend | `lend <supply|withdraw|borrow|repay> plan|run|submit|status` | `--protocol` required | `aave`, `morpho` execution (`morpho` requires `--market-id`) |
| Rewards | `rewards <claim|compound> plan|run|submit|status` | `--protocol` required | `aave` execution |
| Approvals | `approvals plan|run|submit|status` | no provider selector | native ERC-20 approval execution |
| Action inspection | `actions list|show` | optional `--status` filter | persisted action inspection |

Notes:

- Multi-provider commands do not have implicit defaults. Users must pass `--provider` or `--protocol`.

## 3. Architecture Overview

### 3.1 Command Integration

Execution wiring lives in `internal/app/runner.go` and domain files:

- `internal/app/bridge_execution_commands.go`
- `internal/app/lend_execution_commands.go`
- `internal/app/rewards_command.go`
- `internal/app/approvals_command.go`

Design decision:

- Keep execution verbs under the same domain as read paths (`swap`, `bridge`, `lend`, etc).

Tradeoff:

- Better command discoverability and API consistency, but more command wiring complexity in each domain.

### 3.2 Unified ActionBuilder Registry

Command handlers route action construction through a shared registry:

- `internal/execution/actionbuilder/registry.go`

Registry responsibility:

- Resolve provider-backed action builders for swap/bridge.
- Resolve planner-backed action builders for lend/rewards/approvals.
- Keep command-level orchestration (`plan|run|submit|status`) consistent across domains.

Design decision:

- Centralize action-construction dispatch while preserving domain-specific provider/planner implementations.

Tradeoff:

- Better consistency and less duplicated dispatch logic in command files, at the cost of one additional abstraction layer.

### 3.3 Capability Interfaces

Execution providers are opt-in capability interfaces in `internal/providers/types.go`:

- `SwapExecutionProvider`
- `BridgeExecutionProvider`

Lend/rewards/approvals currently use internal planners in `internal/execution/planner` instead of provider interfaces.

Design decision:

- Capability interfaces avoid forcing all providers to implement execution.

Tradeoff:

- Mixed architecture today (provider-based for swap/bridge, planner-based for lend/rewards) increases conceptual surface.

### 3.4 Action Model

Canonical action model is in `internal/execution/types.go`:

- `Action`: intent metadata + ordered steps
- `ActionStep`: executable transaction step
- `Constraints`: execution constraints

Lifecycle states:

- Action: `planned -> running -> completed|failed`
- Step: `pending -> simulated -> submitted -> confirmed|failed`

Step order is the dependency model (no separate DAG). This keeps execution deterministic and straightforward.

### 3.5 Persistence

Persistence is in `internal/execution/store.go` (SQLite + file lock):

- single `actions` table
- full action JSON blob stored in `payload`
- indexed by `status` and `updated_at`

Design decision:

- JSON blob persistence with a light relational index.

Tradeoff:

- Easy compatibility/migrations and exact replay of serialized actions, but weaker SQL-level querying of step internals.

## 4. Command Semantics

### 4.1 `plan`

- Builds an action and persists it to action store.
- Performs planning-time checks required by each planner/provider (for example allowance reads, route fetches, address resolution).
- Does not broadcast transactions.

### 4.2 `run`

- Performs plan + execute in one invocation.
- Persists action first, then executes steps.

### 4.3 `submit`

- Loads a previously persisted action by `--action-id`.
- Executes remaining steps.

### 4.4 `status` and `actions`

- Domain `status` commands fetch one action.
- `actions list` gives cross-domain recent actions.
- `actions show` fetches any action by ID.

## 5. Signing and Key Handling

Signer abstractions:

- Interface: `internal/execution/signer/signer.go`
- Local signer implementation: `internal/execution/signer/local.go`
- Command-level signer setup: `newExecutionSigner(...)` in `internal/app/runner.go`

Supported backend today:

- `--signer local` only (other backends intentionally not implemented yet)

Key sources:

- `--key-source auto|env|file|keystore`
- `--private-key` (run/submit one-off override)
- Environment variables:
  - `DEFI_PRIVATE_KEY`
  - `DEFI_PRIVATE_KEY_FILE`
  - `DEFI_KEYSTORE_PATH`
  - `DEFI_KEYSTORE_PASSWORD`
  - `DEFI_KEYSTORE_PASSWORD_FILE`

`auto` precedence in current code:

1. `--private-key` (when provided)
2. `DEFI_PRIVATE_KEY`
3. `DEFI_PRIVATE_KEY_FILE`
4. `~/.config/defi/key.hex` (or `$XDG_CONFIG_HOME/defi/key.hex` when `XDG_CONFIG_HOME` is set; fallback only when file is present)
5. `DEFI_KEYSTORE_PATH` (+ password input)

Security controls:

- run flows derive sender from signer when omitted; if `--from-address` is provided it must match signer address
- optional `--from-address` signer-address check in submit flows

Design decision:

- Local key signing first, with backend abstraction retained for future expansion.

Tradeoff:

- Fast delivery and low integration complexity now, but no hardware wallet, Safe, or remote signer support yet.

## 6. Endpoint, Contract, and ABI Management

Canonical execution metadata currently lives in `internal/registry/execution_data.go`:

- Execution endpoint constants:
  - LiFi quote/status endpoints
  - Across quote/status endpoints
  - Morpho GraphQL endpoint used by execution planners
- Contract address registries:
  - TaikoSwap contracts by chain
  - Aave PoolAddressesProvider by chain
- ABI fragments:
  - ERC-20 minimal
  - TaikoSwap quoter/router
  - Aave pool/rewards/provider
  - Morpho Blue

Important nuance:

- Execution-critical endpoints are centralized; quote-only/read-only provider endpoints may still remain adapter-local.

Design decision:

- Compile-time Go registry values instead of external YAML/JSON loading.

Tradeoff:

- Strong type safety and fewer runtime failure modes, but lower operational flexibility for hotfixing metadata without a release.

## 7. Execution Engine, Simulation, and Consistency

Core executor: `internal/execution/executor.go`.

Per step execution flow:

1. Validate RPC URL, target, and chain match.
2. Apply lightweight pre-sign policy checks (approval bounds, TaikoSwap target/selector checks, bridge settlement metadata checks).
3. Optional simulation (`eth_call`) when `--simulate=true`.
4. Gas estimation (`eth_estimateGas`) with configurable multiplier.
5. EIP-1559 fee resolution (suggested or overridden by flags).
6. Nonce resolution from pending state.
7. Local signing and broadcast.
8. Receipt polling until success/failure/timeout.

Bridge-specific consistency:

- For `bridge_send` steps, executor also waits for destination settlement via provider APIs:
  - LiFi `/status`
  - Across `/deposit/status`
- Settlement metadata is persisted in `step.expected_outputs` (for example destination tx hash, settlement status).

Context and timeout behavior:

- Command timeout is propagated to run/submit execution via `executeActionWithTimeout(...)`.
- Per-step timeout and poll interval are configurable (`--step-timeout`, `--poll-interval`).

Design decision:

- Simulation defaults to on, bridge completion requires both source receipt and provider settlement, and pre-sign policy checks are fail-closed by default.

Tradeoff:

- Better safety and operational visibility, but slower execution paths and dependence on provider status APIs.
- Advanced users may need explicit overrides (`--allow-max-approval`, `--unsafe-provider-tx`) for provider-specific edge cases.

Current limitation:

- Bridge settlement success is API-confirmed; no universal destination on-chain balance verification is enforced yet.

## 8. Dependency Strategy (`cast` / Foundry)

Decision:

- Do not require `forge cast` as a runtime dependency.

Rationale:

- Runtime binary dependency increases installation complexity.
- Native Go (`go-ethereum`) gives deterministic behavior in CI and releases.

Tradeoff:

- Less convenient ad-hoc debugging for some users who prefer Foundry tooling, but cleaner production runtime.

## 9. Testing and Nightly Drift Checks

Standard quality gates:

- `go test ./...`
- `go test -race ./...`
- `go vet ./...`

Execution-related tests include planner, executor, settlement polling, and command wiring coverage.

Nightly workflow:

- Workflow: `.github/workflows/nightly-execution-smoke.yml`
- Script: `scripts/nightly_execution_smoke.sh`
- Current scope: live smoke for quote/plan paths across key execution surfaces.

Design decision:

- Nightly job validates external dependency drift without requiring broadcast transactions.

Tradeoff:

- Detects endpoint/RPC/contract drift early, but does not prove end-to-end transaction broadcasting on every run.

## 10. Major Decisions and Tradeoffs Summary

| Decision | Why | Tradeoff |
|---|---|---|
| Keep execution under domain commands | Consistent CLI API and easier discoverability | More domain-specific wiring |
| Remove defaults for multi-provider commands | Avoid ambiguous behavior and future provider-addition regressions | More required flags for users |
| Local signer only for v1 | Fast, reliable implementation | No external signer ecosystems yet |
| Store action payload as JSON blob | Easy persistence and replay semantics | Limited SQL-native analytics on steps |
| Compile-time registry | Type-safe and deterministic | Slower metadata hotfix cadence |
| Runtime simulation + settlement polling | Better safety and finality confidence | Longer run time and external API dependency |
| No `cast` runtime dependency | Portable binary releases | Less shell-tool parity for debugging |

## 11. Known Gaps and Next Increments

- Additional signer backends (`safe`, hardware wallets, remote signers).
- Swap execution for additional providers beyond `taikoswap`.
- Registry centralization for all execution endpoints (not just selected constants).
- Stronger destination-chain verification for bridge completion beyond provider API status.
- Plan freshness/revalidation policy (block drift / quote drift thresholds) before submit.
