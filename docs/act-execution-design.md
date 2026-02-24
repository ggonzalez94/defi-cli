# Execution ("act") Design for `defi-cli`

Status: Phase 2 Implemented (swap/bridge/lend/rewards/approvals execution)  
Author: CLI architecture proposal  
Last Updated: 2026-02-23

## Implementation Status (Current)

Implemented in this repository:

- `swap plan|run|submit|status` command family
- `bridge plan|run|submit|status` (LiFi execution planner)
- `approvals plan|run|submit|status`
- `lend supply|withdraw|borrow|repay plan|run|submit|status` (Aave)
- `rewards claim|compound plan|run|submit|status` (Aave)
- `actions list|status` action inspection commands
- Local signer backend (`env|file|keystore`) with signer abstraction
- Sqlite-backed action persistence with resumable step states
- TaikoSwap on-chain quote + swap action planning (approval + swap steps)
- Centralized execution registry (`internal/registry`) for endpoints, contract addresses, and ABI fragments
- Simulation, gas estimation, signing, submission, and receipt tracking in execution engine
- Nightly live execution-planning smoke workflow (`.github/workflows/nightly-execution-smoke.yml`)

Not yet implemented from full roadmap:

- Additional signer backends (`safe`, external wallets, hardware)
- Broader execution provider coverage beyond current defaults (TaikoSwap/Across/LiFi/Aave/Morpho)

## 1. Problem Statement

`defi-cli` currently focuses on read-only data retrieval (`quote`, `markets`, `yield`, `bridge details`, etc).  
We want to add execution capability ("act") so the CLI can perform transactions across protocols and chains while preserving:

- Stable machine-consumable JSON envelope
- Stable exit code behavior
- Deterministic IDs/amount normalization
- Safety and auditability

## 2. Goals and Non-Goals

### Goals

- Add a safe, deterministic execution workflow for major user actions.
- Support multi-protocol and multi-chain execution with resumable state.
- Keep provider integrations modular and testable.
- Maintain a clear source of truth for API endpoints, contract addresses, and ABIs.

### Non-Goals (v1)

- Fully autonomous rebalancing without explicit user invocation.
- Strategy DSL / scheduling engine.
- Support for every protocol from day one.

## 3. Core Architectural Decision

Execution should be modeled as a two-phase workflow:

1. `plan`: produce a deterministic action plan (steps, calldata/intents, constraints, expected outputs).
2. `execute`: execute an existing plan and track state until terminal status.

This separates route selection from transaction submission, improves reproducibility, and enables audit/resume.

## 4. Integration with Existing CLI

### 4.1 Command Surface

Execution should be integrated into existing domains instead of a separate top-level `act` namespace.

Examples:

- `defi swap quote ...` (existing)
- `defi swap plan ...` (new, plan only)
- `defi swap run ...` (new, plan + execute in one invocation)
- `defi swap submit --plan-id ...` (new, execute an existing saved plan)
- `defi swap status --action-id ...` (new lifecycle tracking)

Equivalent command families should exist for:

- `bridge`
- `lend`
- `rewards` (new command group for claim/compound)

This keeps the API intuitive by action domain and avoids a catch-all command surface.

### 4.2 Code Integration (proposed package layout)

```text
internal/
  execution/
    planner.go           # generic planning orchestration
    executor.go          # execution orchestration
    tracker.go           # status polling + lifecycle transitions
    store.go             # action persistence (sqlite)
    types.go             # ActionPlan, ActionStep, statuses
    simulate.go          # preflight simulation hooks
    signer/
      signer.go          # signer interface
      local.go           # local key signer (v1)
      txbuilder.go       # EIP-1559 tx assembly
  registry/
    loader.go            # endpoint/address/abi loader + validation
    types.go
  providers/
    types.go             # extend interfaces for execution capabilities
    taikoswap/           # quote + execution planner for taiko swap
```

`internal/app/runner.go` adds domain-specific subcommands (`swap plan/run/submit/status`, etc) while reusing envelope/output/error handling patterns.

### 4.3 Provider Capability Model

Add capability-specific interfaces (without breaking read-only interfaces):

- `SwapExecutionPlanner`
- `BridgeExecutionPlanner`
- `LendExecutionPlanner`
- `RewardsExecutionPlanner`

Each provider returns provider-specific plan steps in a shared normalized action format.

## 5. Source of Truth for Endpoints, Addresses, ABIs

### 5.1 Registry Design

Track interaction metadata in a versioned registry under repository control:

```text
internal/registry/data/
  providers/
    uniswap.yaml
    taikoswap.yaml
    across.yaml
    lifi.yaml
  contracts/
    taiko-mainnet.yaml
    ethereum-mainnet.yaml
  abis/
    uniswap/
      quoter_v2.json
      swap_router_02.json
      universal_router.json
    erc20/
      erc20_minimal.json
      permit2.json
```

### 5.2 What each file tracks

#### Provider endpoint entry

- Provider name and version
- Base URLs and path templates (e.g. quote/swap/status endpoints)
- Auth method and env var names
- Supported chains per endpoint
- Rate-limit hints and timeout defaults

#### Contract entry

- `chain_id` (CAIP/EVM ID)
- protocol name
- contract role (router, quoter, factory, permit2, etc)
- address
- ABI reference path
- source verification URL (block explorer / repo)
- metadata (deployed block, notes)

#### ABI entry

- Canonical ABI JSON (minimal ABI fragments where possible)
- Optional selector map for validation
- Version/source metadata

### 5.3 Validation Requirements

Add validation checks in CI/unit tests:

- Address format and non-zero checks
- ABI JSON parse + required method presence
- Registry schema validation
- Provider endpoint presence for declared capabilities

Optional integration validation (nightly):

- `eth_getCode` non-empty for configured addresses
- dry-run `eth_call` smoke on critical view methods

### 5.4 Override Mechanism

Support local override for rapid hotfixes:

- `DEFI_REGISTRY_PATH=/path/to/registry-overrides`

Precedence:

1. CLI flags (if exposed)
2. env override registry
3. bundled registry in repo

## 6. Forge `cast` Dependency Decision

### 6.1 Recommendation

Do **not** make `cast` a runtime dependency.

Reasoning:

- Adds external binary dependency for all users.
- Makes release artifacts less self-contained.
- Process spawning is slower and harder to test deterministically.
- Native Go JSON-RPC and ABI encoding is more portable and CI-friendly.

### 6.2 Where `cast` should be used

Use `cast` as developer tooling only:

- `scripts/verify/*.sh` for parity checks
- troubleshooting registry/address issues
- reproducing on-chain call results in docs/tests

## 7. Signer Architecture (v1 local key)

### 7.1 Scope

v1 supports a local key signer only, while keeping the signer layer extensible for:

- external wallet providers
- Safe/multisig
- hardware signers
- remote signers

### 7.2 Signer Interface

Use a signer abstraction in `internal/execution/signer/signer.go`:

- `Address() string`
- `SignTx(chainID, tx) -> rawTx`
- `SignMessage(payload) -> signature` (future-proofing)

Execution orchestration consumes only this interface.

### 7.3 Local key ingestion (v1)

Avoid requiring private key values in CLI args. Preferred sources:

1. `DEFI_PRIVATE_KEY_FILE` (hex key in file, strict file-permission checks)
2. `DEFI_KEYSTORE_PATH` + `DEFI_KEYSTORE_PASSWORD` or `DEFI_KEYSTORE_PASSWORD_FILE`
3. `DEFI_PRIVATE_KEY` (supported, but discouraged in shell history environments)

Optional explicit flag:

- `--key-source env|file|keystore` (for deterministic automation)

### 7.4 Transaction signing flow

For each executable step:

1. Resolve sender address from signer.
2. Fetch nonce (`eth_getTransactionCount`, pending).
3. Build tx params:
   - EIP-1559 (`maxFeePerGas`, `maxPriorityFeePerGas`) by default
   - `gasLimit` from simulation/estimation with safety multiplier
4. Build unsigned tx from step data (`to`, `data`, `value`, `chainId`).
5. Sign locally.
6. Broadcast (`eth_sendRawTransaction`).
7. Persist tx hash and receipt status.

Implementation note: use native Go libraries for EVM tx construction/signing (e.g., go-ethereum transaction types and secp256k1 signing utilities), not shelling out to external binaries.

### 7.5 Security controls

- Never print private key material in logs or envelopes.
- Redact signer secrets in errors.
- Validate key/address match before first execution.
- Enforce minimum key file permissions for file-based keys.
- Add `--confirm-address` optional check for CI/ops workflows.

### 7.6 Agent and automation key handling

Expected automation pattern:

1. Agent injects key source via environment variables before command execution.
2. CLI resolves signer source via normal precedence (`flags > env > config > defaults`).
3. CLI emits signer address metadata only (never key material).

Recommended usage for agents/CI:

- Use short-lived per-command environment injection.
- Prefer file/keystore based key sources over raw-key env values.
- Set `--confirm-address` for high-safety pipelines.

## 8. Command API Design (Draft)

### 8.1 Common Principles

- `plan` is safe/read-only by default.
- `run` performs plan + execute in one command (with explicit confirmation flag).
- `submit` executes an already-created plan.
- Every command returns standard envelope with `action_id` and step-level metadata.
- No hidden side effects in plan phase.
- Avoid overloaded verbs. Command names should directly describe behavior.

### 8.2 Command Sketch

#### Plan commands

- `defi swap plan --provider taikoswap --chain taiko --from-asset USDC --to-asset WETH --amount 1000000`
- `defi bridge plan --provider lifi --from 1 --to 8453 --asset USDC --amount 1000000`
- `defi lend supply plan --protocol aave --chain 1 --asset USDC --amount 1000000`
- `defi approvals plan --chain taiko --asset USDC --spender <addr> --amount 1000000`
- `defi rewards claim plan --protocol aave --chain 1 --asset AAVE`

#### Run commands (plan + execute)

- `defi swap run --provider taikoswap --chain taiko --from-asset USDC --to-asset WETH --amount 1000000 --yes`
- `defi bridge run --provider lifi --from 1 --to 8453 --asset USDC --amount 1000000 --yes`
- `defi lend supply run --protocol aave --chain 1 --asset USDC --amount 1000000 --yes`
- `defi approvals run --chain taiko --asset USDC --spender <addr> --amount 1000000 --yes`

#### Submit commands (execute existing plan)

- `defi swap submit --plan-id <id> --yes`
- `defi bridge submit --plan-id <id> --yes`
- `defi lend supply submit --plan-id <id> --yes`

#### Lifecycle commands

- `defi swap status --action-id <id>`
- `defi bridge status --action-id <id>`
- `defi lend status --action-id <id>`
- `defi actions list --status pending` (optional global view)
- `defi actions resume --action-id <id>` (optional global resume)

### 8.3 Global Execution Flags (proposed)

- `--wallet` (address only mode)
- `--signer` (`local|external|walletconnect|safe`) (future)
- `--simulate` (default true for `run` and `submit`)
- `--slippage-bps`
- `--deadline`
- `--max-fee-gwei`, `--max-priority-fee-gwei`
- `--nonce-policy` (`next|fixed`)
- `--yes` (required for `run`/`submit`)

## 9. Action Plan and Tracking Model

### 9.1 ActionPlan (normalized)

Core fields:

- `action_id`
- `intent_type` (`swap`, `bridge`, `lend_supply`, `approve`, `claim`, etc)
- `created_at`
- `constraints` (slippage, deadline, policy)
- `steps[]`

Each step includes:

- `step_id`
- `chain_id`
- `type` (`approval`, `swap`, `bridge_send`, `bridge_finalize`, `lend_call`, `claim`)
- `target` (contract / endpoint)
- `call_data` or provider instruction payload
- `value`
- `expected_outputs`
- `depends_on[]`
- `status`

### 9.2 Persistence (sqlite)

Add action tables:

- `actions`
- `action_steps`
- `action_events`

Track:

- tx hashes
- bridge transfer IDs/message IDs
- retries
- error details (mapped to CLI error codes)

### 9.3 Status Lifecycle

`planned -> validated -> awaiting_signature -> submitted -> confirmed -> completed`

Failure paths:

`failed`, `timed_out`, `partial` (multi-step actions)

## 10. Main Use Cases (Phase 1 Scope)

### 10.1 Swap Execute

User flow:

1. Build swap plan (route + approvals + minOut constraint).
2. Simulate each transaction step.
3. Execute approval (if required).
4. Execute swap.
5. Confirm and return final out amount and tx hash.

Initial support:

- single-chain swap
- exact input
- taikoswap + existing aggregator providers where feasible

### 10.2 Bridge Execute

User flow:

1. Plan source transaction and destination settlement expectations.
2. Execute source chain tx.
3. Track async transfer status.
4. Mark complete when destination settlement confirms.

Initial support:

- bridge-only transfers first
- bridge+destination swap as phase 2 extension

### 10.3 Approve / Revoke

User flow:

1. Plan approval delta (exact amount by default).
2. Execute and confirm.
3. Optional revoke command sets allowance to zero.

### 10.4 Lend Actions

Initial verbs:

- `supply`
- `withdraw`
- `borrow`
- `repay`

Each action follows plan + execute with health-factor and liquidity checks in planning.

### 10.5 Rewards Claim / Compound

Initial verbs:

- `claim`
- `compound` (where protocol supports single-tx or known workflow)

## 11. Safety, Policy, and UX Guardrails

- Policy allowlist checks (protocol, spender, chain, asset).
- Simulation-before-execution default (opt-out should be explicit and discouraged).
- Slippage and deadline required for swap-like actions.
- Exact approval default, unlimited approval only with explicit flag.
- Step-by-step decoded previews before execution.
- Stable and explicit partial-failure semantics.

## 12. Simulation, Consistency, and Nightly Validation

### 12.1 Simulation layers

Use layered checks to reduce execution surprises:

1. Static plan validation
   - chain/provider support
   - token/asset normalization
   - spender and target allowlist checks
2. Preflight state checks
   - balances
   - allowances
   - protocol preconditions (where available)
3. Transaction simulation
   - `eth_call` with exact tx payload (`to`, `data`, `value`, `from`)
   - gas estimation (`eth_estimateGas`) with margin policy
4. Optional deep trace simulation
   - `debug_traceCall` when RPC supports it
   - classify likely failure reasons for better errors

### 12.2 Consistency between plan and execution

Each plan should record:

- simulation block number
- simulation timestamp
- RPC endpoint fingerprint
- route/quote hash
- slippage/deadline constraints

Execution should enforce revalidation triggers:

- plan age exceeds configured max age
- chain head drift exceeds configured block delta
- quote hash changes (provider route changed)
- simulation indicates constraints are now unsafe

When any trigger fails, command exits with a deterministic replan-required error.

### 12.3 Cross-chain consistency model

For bridge flows:

- source-chain tx is simulated and executed deterministically.
- destination outcome is tracked asynchronously via provider status APIs and/or chain events.
- action remains `pending` until destination settlement reaches terminal status.

Bridge plans must include explicit timeout/SLA metadata per provider route.

### 12.4 Nightly validation jobs

Add a nightly workflow (separate from PR CI) for live-environment checks:

1. Registry integrity
   - schema validation
   - ABI parsing
   - non-zero address checks
2. On-chain contract liveness
   - `eth_getCode` must be non-empty for configured contracts
3. Critical method smoke calls
   - e.g., quoter/factory read calls on representative pairs
4. Provider endpoint liveness
   - health checks and minimal quote/simulation calls
5. Drift report artifact
   - list failing chains/providers/contracts
   - include first-seen timestamp for regressions

Failures should open/annotate issues but not block all contributor PRs by default.

### 12.5 Test strategy split

- PR CI: deterministic unit tests + mocked RPC/provider tests.
- Nightly CI: live RPC/provider validation.
- Optional weekly: broader matrix with additional chains/providers.

## 13. Exit Code Extensions (proposed)

Existing exit code contract remains. Add action-specific codes:

- `20`: action plan validation failed
- `21`: simulation failed
- `22`: execution rejected by policy
- `23`: action timed out / pending too long
- `24`: signer unavailable / signing failed

## 14. Rollout Plan

### Phase 0: Foundations

- Add domain command scaffolding (`swap|bridge|lend|rewards` with `plan|run|submit|status`).
- Add action storage and status plumbing.
- Add registry framework and validators.

### Phase 1: Core Execution

- swap run/submit (single-chain)
- approve/revoke
- status/resume/list

### Phase 2: Cross-Chain

- bridge run/submit with async tracking

### Phase 3: Lending + Rewards

- supply/withdraw/borrow/repay
- claim/compound

### Phase 4: UX and Automation Hardening

- richer signer integrations
- policy presets
- batched operations and optional smart account flows

## 15. Open Questions

- Which signer backend should be next after local key (`external wallet`, `Safe`, or remote signer)?
- How much protocol-specific simulation is required beyond `eth_call`?
- What SLA should define `timed_out` for bridges by provider/route type?
- What should default plan-expiry and block-drift thresholds be per command type?
