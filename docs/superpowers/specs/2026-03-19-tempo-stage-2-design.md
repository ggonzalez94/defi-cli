# Tempo Stage 2: Native Execution & Agent Wallets

Status: Approved
Date: 2026-03-19
Scope: Enable real transaction execution on Tempo chain + agent wallet signer support

## 1. Problem

Stage 1 added Tempo swap quotes and execution _planning_ (building `Action` with calldata steps). But `submit` cannot work because:

- The executor builds standard EIP-1559 (type 2) transactions. Tempo uses a custom type `0x76` with fee-token payments, batched calls, parallelizable nonces, and Keychain signatures.
- `actions estimate` is blocked because the estimator assumes native-gas pricing. Tempo fees are denominated in USD stablecoins.
- The signer only supports local ECDSA keys where key address = sender. Tempo agent wallets use delegated access keys where the key signs on behalf of a separate wallet address.

## 2. Tempo Transaction Model (Type 0x76)

Empirically confirmed from on-chain data and Tempo documentation:

```
0x76 || rlp([
    chain_id,
    max_priority_fee_per_gas,
    max_fee_per_gas,
    gas_limit,
    calls,                   // RLP list of Call structs (batching)
    access_list,
    nonce_key,               // parallelizable nonces
    nonce,
    valid_before,            // time-bounded validity
    valid_after,
    fee_token,               // pay gas in any USD stablecoin
    fee_payer_signature,
    aa_authorization_list,
    key_authorization?,      // for delegating key authority
    sender_signature         // secp256k1, P256, WebAuthn, or Keychain
])
```

Key differences from standard EVM:
- `fee_token`: fees paid in stablecoin (e.g., USDC.e), not native ETH
- `calls`: array of batched calls (approve + swap in ONE atomic tx)
- `nonce_key`: parallel nonce lanes
- Keychain signatures: access key signs on behalf of wallet address

Standard type 2 (EIP-1559) transactions also work on Tempo for EOAs with funds.

## 3. Fee Model

Confirmed empirically:
- `eth_estimateGas` works on Tempo RPC with standard call payloads
- `baseFeePerGas` and `gasPrice` are in 18-decimal USD (same arithmetic as wei, but the unit is USD)
- Fee calculation: `gasUsed * effectiveGasPrice` = fee in 18-decimal USD
- Convert to fee token: divide by `10^(18 - tokenDecimals)` (USDC.e/pathUSD = 6 decimals, so divide by 10^12)
- FeeManager contract (`0xfeec...000`) collects max fee upfront, refunds unused portion after execution
- Fee AMM auto-converts between stablecoins at ~0.997 rate if user's fee token differs from validator's

Real example: a type 0x76 tx used 70,287 gas at 21 gwei effective price = $0.001476 USD fee.

## 4. Architecture

### 4.1 StepExecutor Interface

Extract execution into a pluggable interface to keep EVM and Tempo paths cleanly separated:

```go
// internal/execution/step_executor.go

// StepGasEstimate holds per-step gas estimation results.
// For EVM chains, FeeUnit is "ETH" and FeeToken is empty.
// For Tempo chains, FeeUnit is the fee token symbol and FeeToken is the token address.
type StepGasEstimate struct {
    GasEstimate   uint64
    LikelyFee     string // human-readable decimal in fee token units
    WorstCaseFee  string
    FeeUnit       string // e.g., "ETH", "USDC.e"
    FeeToken      string // token address (empty for native gas chains)
}

type StepExecutor interface {
    // ExecuteStep submits a single step on-chain. It handles tx construction,
    // signing, broadcast, and receipt polling. It does NOT handle action-level
    // orchestration (status lifecycle, persistence, approval state visibility).
    ExecuteStep(ctx context.Context, store *Store, action *Action, step *ActionStep, opts ExecuteOptions) error

    // EstimateStep returns gas and fee estimates for a single step.
    EstimateStep(ctx context.Context, action *Action, step *ActionStep) (StepGasEstimate, error)

    // EffectiveSender returns the address that will be the tx sender (msg.sender).
    // For EVM: the signer's derived address. For Tempo agent wallets: the wallet address.
    EffectiveSender() common.Address
}
```

Two implementations:
- `EVMStepExecutor` — current logic extracted unchanged, uses go-ethereum + local signer. `EffectiveSender()` returns `signer.Address()`.
- `TempoStepExecutor` — new, uses `github.com/tempoxyz/tempo-go`, handles `Calls` batching, fee-token, Keychain signing. `EffectiveSender()` returns `TempoSigner.WalletAddress()` when available, otherwise `signer.Address()`.

Resolver:
```go
func ResolveStepExecutor(chainID int64, txSigner signer.Signer) StepExecutor {
    if IsTempoChain(chainID) {
        return NewTempoStepExecutor(txSigner)
    }
    return NewEVMStepExecutor(txSigner)
}
```

`ExecuteAction` changes:
- Resolves the executor once per action (all steps in an action share the same chain)
- Uses `executor.EffectiveSender()` to stamp `action.FromAddress` (instead of the current `txSigner.Address().Hex()`)
- Delegates step submission to `executor.ExecuteStep()`
- Retains ownership of: status lifecycle, persistence, timeout, and EVM-specific post-step hooks

EVM-specific post-step hooks (approval state visibility via `ensurePostConfirmationStateVisible`, bridge settlement via `verifyBridgeSettlement`) remain in `ExecuteAction` guarded by step type checks. These do not apply to Tempo batched steps because:
- Tempo batched steps embed approvals inside `Calls` — they execute atomically, so no post-approval visibility wait is needed
- Tempo has no bridge steps (Tempo is not a bridge destination chain)

If future Tempo step types need post-step hooks, they can be added to the `StepExecutor` interface as optional methods.

Benefits:
- EVM path untouched — zero regression risk
- Each executor is a focused, testable unit
- Adding a third chain type = new implementation, not a new branch
- `EffectiveSender()` cleanly solves the key-address vs wallet-address divergence

### 4.2 Action Model Extension

Add optional `Calls` field to `ActionStep` for Tempo batching:

```go
type StepCall struct {
    Target string `json:"target"`
    Data   string `json:"data"`
    Value  string `json:"value"`
}

type ActionStep struct {
    // ... existing fields unchanged ...
    Target string     `json:"target"`           // used by EVMStepExecutor
    Data   string     `json:"data"`
    Value  string     `json:"value"`
    Calls  []StepCall `json:"calls,omitempty"`   // used by TempoStepExecutor
}
```

Semantics:
- `Calls` non-empty → `TempoStepExecutor` builds `tx.Calls` from it
- `Calls` empty/nil → `EVMStepExecutor` uses `Target`/`Data`/`Value`
- Backwards-compatible: existing persisted actions deserialize with `Calls` as nil

### 4.3 Tempo Provider Changes

`BuildSwapAction` in `internal/providers/tempo/client.go` changes from producing 2 steps (approve + swap) to:
- Approval needed: 1 step with `Calls: []StepCall{{approve}, {swap}}`
- No approval needed: 1 step with `Calls: []StepCall{{swap}}`

This gives atomic approve+swap in a single Tempo transaction.

### 4.4 TempoStepExecutor Flow

Per step:
1. Connect to Tempo RPC
2. Policy check (validate each `Call` in `step.Calls` — target/selector validation)
3. Optional simulation (`eth_call` per call, or batched)
4. Gas estimation (`eth_estimateGas`)
5. Fee resolution (gas price from RPC, denominated in 18-decimal USD)
6. Nonce resolution (standard pending nonce for now; `nonceKey` parallelism is future optimization)
7. Build type 0x76 tx using `tempo-go`:
   - `tx.Calls` from `step.Calls`
   - `tx.FeeToken` from `--fee-token` flag or chain default (registry)
   - `tx.MaxFeePerGas`, `tx.MaxPriorityFeePerGas` from RPC
   - `tx.Gas` from estimation with multiplier
8. Sign with `tempo-go` signer (`transaction.SignTransaction`)
9. Broadcast via `tempo-go` client (`client.SendTransaction`)
10. Poll `eth_getTransactionReceipt` until confirmed/failed/timeout

### 4.5 Fee-token Gas Estimation

The current `EstimateActionGas` in `internal/execution/estimate.go` rejects Tempo actions upfront and uses `step.Target`/`step.Data` to build `ethereum.CallMsg`. Both need to change:

**Estimation dispatch**: `EstimateActionGas` delegates per-step estimation to `StepExecutor.EstimateStep()` instead of building call messages directly. The `EVMStepExecutor.EstimateStep()` contains the current logic (unchanged). The `TempoStepExecutor.EstimateStep()` handles batched `Calls`:
- For steps with `Calls`: estimate gas for the full batched call set (using `eth_estimateGas` with the combined calldata, or per-call estimation summed — validated during spike)
- Compute fee in fee-token units using `gasEstimate * gasPrice / 10^(18 - tokenDecimals)`

**Aggregate totals**: The existing `ActionGasEstimateChainTotal` uses `likely_fee_wei` and `worst_case_fee_wei` field names. For Tempo chains, these fields contain fee-token base units (not wei). To avoid breaking existing consumers:
- Add `fee_unit` and `fee_token` fields to `ActionGasEstimateChainTotal` (omitted for EVM chains)
- When `fee_unit` is present, consumers know `*_fee_wei` values are in the fee token's base units, not ETH wei
- The field names are kept for JSON stability; the semantic change is documented in the `fee_unit` field's presence

**Per-step response** (from `StepGasEstimate`):

```json
{
  "gas_estimate": 150000,
  "likely_fee": "0.32",
  "worst_case_fee": "0.64",
  "fee_unit": "USDC.e",
  "fee_token": "0x20c000000000000000000000b9537d11c60e8b50"
}
```

Standard EVM chains: `fee_unit` = `"ETH"` (or native token), `fee_token` omitted.

### 4.6 Registry Additions

- `TempoFeeToken(chainID int64) (string, bool)` — default fee token per Tempo chain
- `IsTempoChain(chainID int64) bool` — chain detection for executor routing

Tempo chain IDs (already registered in stage 1 branch at `cca9373`):
- `4217` — Tempo mainnet (`tempo`, `presto`)
- `42431` — Tempo Moderato testnet (`moderato`, `tempo testnet`)
- `31318` — Tempo devnet (`tempo devnet`)

Stage 1 already added to the registry (in branch `cca9373`, not yet on `main`):
- `TempoStablecoinDEX(chainID) (string, bool)` — DEX contract address per chain
- RPC entries for all three chain IDs
- Bootstrap token entries in `internal/id/id.go`
- Tempo ABI fragments in `internal/registry/abis.go`

These remain unchanged. Stage 2 adds only `TempoFeeToken` and `IsTempoChain`.

### 4.7 Policy Updates

`validateSwapPolicy` for Tempo steps:
- Iterate `step.Calls` instead of checking single `step.Target`/`step.Data`
- Validate each call's target/selector against the expected contract (DEX for swap calls, token for approve calls)

## 5. Agent Wallet Signer (Phase 2)

### 5.1 Signer Design

New interface alongside existing `Signer` (not replacing it):

```go
type TempoSigner interface {
    WalletAddress() common.Address
    SignTempoTx(tx *transaction.Transaction) error
}
```

`TempoStepExecutor` checks: if signer implements `TempoSigner`, use `WalletAddress()` as the tx sender and `SignTempoTx` for signing. Otherwise (regular `Signer` on Tempo chain), use the key address directly as sender with plain secp256k1 — works for users with their own funded EOA.

### 5.2 Wallet Discovery

`--signer tempo` triggers:

1. Check if `tempo` CLI exists on PATH
2. If not found: error with install instructions (`curl -fsSL https://tempo.xyz/install | sh`)
3. Run `tempo wallet -j whoami`, parse JSON
4. If `ready: false`: error with `"run 'tempo wallet login' to set up your agent wallet"`
5. Extract `key.key` (private key hex) and `wallet` (wallet address)
6. Check `key.expires_at` hasn't passed; warn if `spending_limit.remaining` is low
7. Construct `TempoWalletSigner` using `tempo-go`'s `signer.NewSigner(keyHex)` internally

The `TempoWalletSigner`:
- `WalletAddress()` returns the wallet address (funds holder, tx sender)
- `SignTempoTx()` signs with the access key, producing a Keychain-wrapped signature (access key signs on behalf of wallet)

### 5.3 Keychain Signature

The Tempo tx signature for access keys wraps the secp256k1 sig:
```
Keychain { user_address: walletAddr, signature: { Secp256k1: sig } }
```

If `tempo-go`'s `signer.NewSigner` handles this automatically when the key is registered as an access key, we use it directly. If not, we wrap manually after signing. This is an implementation detail to validate during the spike.

### 5.4 Signer Precedence and Error Handling

When `--signer tempo`:
- `tempo wallet -j whoami` is the sole source
- `--private-key`, `DEFI_PRIVATE_KEY`, and other local signer env vars are rejected with exit code `2` (usage error): `"--signer tempo cannot be combined with --private-key or local key env vars; tempo wallet manages keys automatically"`
- If `tempo` CLI not found: exit code `24` (signer unavailable): `"tempo CLI is required for --signer tempo. Install: curl -fsSL https://tempo.xyz/install | sh"`
- If `ready: false`: exit code `24`: `"tempo wallet is not logged in; run 'tempo wallet login' to set up your agent wallet"`
- If `key.expires_at` has passed: exit code `24`: `"tempo wallet access key has expired; run 'tempo wallet login' to refresh"`
- If `spending_limit.remaining` is below a threshold: emit a warning in the response envelope but proceed

## 6. Phasing

### Phase 1 (must-have): Tempo Executor

- Add `tempo-go` dependency
- `StepExecutor` interface + `EVMStepExecutor` (extract current logic) + `TempoStepExecutor`
- `Calls` field on `ActionStep`
- Tempo provider builds batched steps
- Fee-token gas estimation in `actions estimate`
- Registry additions (`TempoFeeToken`, `IsTempoChain`)
- Policy updates for batched calls
- Works with `--signer local` for users with funded EOAs on Tempo

### Phase 2 (should-have): Agent Wallet Signer

- `TempoSigner` interface
- `TempoWalletSigner` implementation (reads from `tempo` CLI)
- `--signer tempo` flag wiring
- Keychain signature support
- Pre-flight checks (expiry, spending limit warnings)

Phase 2 layers cleanly on Phase 1. If Keychain wrapping in Go hits friction, Phase 1 ships independently as a fully functional Tempo execution path.

## 7. Spike Validation (Before Full Implementation)

Three unknowns to validate early:

1. **`tempo-go` SDK compatibility**: Does it work with the go-ethereum version in `go.mod`? Does `signer.NewSigner` produce Keychain-wrapped signatures for access keys?
2. **`eth_estimateGas` for batched calls**: Does Tempo RPC accept estimation requests that match the type 0x76 call structure, or do we estimate per-call and sum?
3. **Receipt polling**: Confirm `eth_getTransactionReceipt` works identically for type 0x76 txs.

## 8. Files Changed

### New files
- `internal/execution/step_executor.go` — `StepExecutor` interface + resolver
- `internal/execution/evm_executor.go` — extracted EVM logic
- `internal/execution/tempo_executor.go` — Tempo execution path
- `internal/execution/signer/tempo.go` — `TempoSigner` interface + `TempoWalletSigner` (phase 2)

### Modified files
- `internal/execution/types.go` — add `StepCall`, `Calls` field to `ActionStep`
- `internal/execution/executor.go` — `ExecuteAction` delegates to `StepExecutor`
- `internal/execution/policy_basic.go` — handle batched `Calls` in policy validation
- `internal/providers/tempo/client.go` — `BuildSwapAction` produces batched steps
- `internal/registry/contracts.go` — `TempoFeeToken()`, `IsTempoChain()`
- `internal/app/runner.go` — `--signer tempo` flag, `--fee-token` flag, executor wiring
- `internal/execution/estimate.go` — delegate per-step estimation to `StepExecutor.EstimateStep()`, add `fee_unit`/`fee_token` to chain totals
- `internal/app/runner_actions_test.go` — add fee-token estimate tests for Tempo steps
- `go.mod` / `go.sum` — add `github.com/tempoxyz/tempo-go`

### Unchanged
- All existing EVM provider code
- All existing EVM execution tests
- Action store/persistence
- CLI schema/output envelope
