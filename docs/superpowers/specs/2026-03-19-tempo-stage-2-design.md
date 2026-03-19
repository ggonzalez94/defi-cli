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
type StepExecutor interface {
    ExecuteStep(ctx context.Context, store *Store, action *Action, step *ActionStep, opts ExecuteOptions) error
    EstimateStep(ctx context.Context, action *Action, step *ActionStep) (GasEstimate, error)
}
```

Two implementations:
- `EVMStepExecutor` — current logic extracted unchanged, uses go-ethereum + local signer
- `TempoStepExecutor` — new, uses `github.com/tempoxyz/tempo-go`, handles `Calls` batching, fee-token, Keychain signing

Resolver:
```go
func ResolveStepExecutor(chainID int64, signer signer.Signer) StepExecutor {
    if IsTempoChain(chainID) {
        return NewTempoStepExecutor(signer)
    }
    return NewEVMStepExecutor(signer)
}
```

Top-level `ExecuteAction` stays chain-agnostic: iterates steps, updates status, persists to store, delegates submission to the resolved executor. Shared orchestration (status lifecycle, persistence, timeout) lives once.

Benefits:
- EVM path untouched — zero regression risk
- Each executor is a focused, testable unit
- Adding a third chain type = new implementation, not a new branch

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

Remove the hardcoded Tempo rejection in `actions estimate`. The Tempo estimate path:

1. `eth_estimateGas` → gas units
2. `eth_gasPrice` → fee rate (18-decimal USD)
3. `gas * feeRate` → fee in 18-decimal USD
4. Read `decimals()` from fee token contract → convert to token base units via `10^(18 - decimals)`

Response extension (additive, backwards-compatible):

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

Existing entries (`TempoStablecoinDEX`, RPC map) unchanged.

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

### 5.4 Signer Precedence

When `--signer tempo`:
- `tempo wallet -j whoami` is the sole source
- `--private-key`, `DEFI_PRIVATE_KEY`, and other local signer env vars are rejected (mixing models is confusing)
- Error messages guide users to `tempo wallet login` and fund flows

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
- `internal/app/runner_actions_test.go` — remove Tempo estimate rejection, add fee-token estimate tests
- `go.mod` / `go.sum` — add `github.com/tempoxyz/tempo-go`

### Unchanged
- All existing EVM provider code
- All existing EVM execution tests
- Action store/persistence
- CLI schema/output envelope
