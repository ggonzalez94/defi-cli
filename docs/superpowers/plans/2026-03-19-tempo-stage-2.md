# Tempo Stage 2: Native Execution & Agent Wallets — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable real transaction execution on Tempo chain using native type 0x76 transactions with fee-token payments and batched calls, plus agent wallet signer support via the Tempo CLI.

**Architecture:** Extract the current monolithic executor into a `StepExecutor` interface with EVM and Tempo implementations. The EVM path is an exact extraction of existing code (zero behavior change). The Tempo path uses `github.com/tempoxyz/tempo-go` for type 0x76 tx construction, fee-token handling, and batched calls. Agent wallet support reads delegated access keys from the `tempo` CLI.

**Tech Stack:** Go 1.24, `github.com/tempoxyz/tempo-go` v0.3.0, `github.com/ethereum/go-ethereum` v1.14.12 (existing)

**Spec:** `docs/superpowers/specs/2026-03-19-tempo-stage-2-design.md`

---

### Task 0: Spike — Validate tempo-go SDK

**Files:**
- Temporary: `/tmp/tempo-spike/main.go` (deleted after spike)
- Modify: `go.mod`, `go.sum`

The purpose is to validate three unknowns before committing to the full implementation.

- [ ] **Step 1: Add tempo-go dependency**

```bash
cd /Users/gustavo/apps/defi-cli-worktrees/tempo-stage-2
go get github.com/tempoxyz/tempo-go@v0.3.0
```

Verify it resolves without conflicts with go-ethereum v1.14.12.

- [ ] **Step 2: Write a spike script that tests tx construction and signing**

Write a temporary Go test in the project that:
1. Creates a `signer.NewSigner` from a throwaway key
2. Builds a `transaction.New()` with `ChainID=4217`, `Calls`, `FeeToken`, gas params
3. Signs with `transaction.SignTransaction`
4. Verifies the signed tx is non-nil

```go
// internal/execution/tempo_spike_test.go (temporary, deleted after spike)
package execution

import (
    "math/big"
    "testing"

    "github.com/ethereum/go-ethereum/common"
    temposigner "github.com/tempoxyz/tempo-go/pkg/signer"
    "github.com/tempoxyz/tempo-go/pkg/transaction"
)

func TestTempoGoSpike(t *testing.T) {
    // 1. Create signer from a throwaway key
    s, err := temposigner.NewSigner("0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80")
    if err != nil {
        t.Fatalf("create signer: %v", err)
    }
    t.Logf("signer address: %s", s.Address().Hex())

    // 2. Build a Tempo transaction
    tx := transaction.New()
    tx.ChainID = big.NewInt(4217)
    tx.MaxFeePerGas = big.NewInt(2_000_000_000)
    tx.MaxPriorityFeePerGas = big.NewInt(1_000_000_000)
    tx.Gas = 100_000
    feeToken := common.HexToAddress("0x20c000000000000000000000b9537d11c60e8b50")
    tx.FeeToken = &feeToken
    dexAddr := common.HexToAddress("0x0000000000000000000000000000000000000001")
    tx.Calls = []transaction.Call{{
        To:    &dexAddr,
        Value: big.NewInt(0),
        Data:  []byte{0xde, 0xad},
    }}

    // 3. Sign
    err = transaction.SignTransaction(tx, s)
    if err != nil {
        t.Fatalf("sign tx: %v", err)
    }
    t.Logf("signed tx OK")
}
```

Run: `go test ./internal/execution/ -run TestTempoGoSpike -v`

- [ ] **Step 3: Validate eth_getTransactionReceipt for type 0x76**

```bash
cast receipt 0x255305650c59ffdf4e49c2e46e8ad0ad9b303e8327ddd90fed347ca5dc17055b --rpc-url https://rpc.tempo.xyz
```

Confirm standard receipt fields (status, gasUsed, effectiveGasPrice, logs) are present.

- [ ] **Step 4: Clean up spike and commit dependency**

Delete `internal/execution/tempo_spike_test.go`. Keep the `go.mod`/`go.sum` changes.

```bash
rm internal/execution/tempo_spike_test.go
git add go.mod go.sum
git commit -m "chore: add tempo-go v0.3.0 dependency"
```

---

### Task 1: StepExecutor Interface + EVM Extraction

**Files:**
- Create: `internal/execution/step_executor.go`
- Create: `internal/execution/evm_executor.go`
- Modify: `internal/execution/executor.go`
- Create: `internal/execution/evm_executor_test.go`

This task extracts the current step execution logic into the `StepExecutor` interface without changing any behavior. The EVM path must be a pure refactor — all existing tests must pass unchanged.

- [ ] **Step 1: Create the StepExecutor interface**

Create `internal/execution/step_executor.go`:

```go
package execution

import (
    "context"

    "github.com/ethereum/go-ethereum/common"
    "github.com/ggonzalez94/defi-cli/internal/execution/signer"
    "github.com/ggonzalez94/defi-cli/internal/registry"
)

// StepGasEstimate holds per-step gas estimation results.
type StepGasEstimate struct {
    GasEstimateRaw       string `json:"gas_estimate_raw"`
    GasLimit             string `json:"gas_limit"`
    BaseFeePerGasWei     string `json:"base_fee_per_gas_wei"`
    MaxPriorityFeeWei    string `json:"max_priority_fee_per_gas_wei"`
    MaxFeePerGasWei      string `json:"max_fee_per_gas_wei"`
    EffectiveGasPriceWei string `json:"effective_gas_price_wei"`
    LikelyFeeWei         string `json:"likely_fee_wei"`
    WorstCaseFeeWei      string `json:"worst_case_fee_wei"`
    FeeUnit              string `json:"fee_unit,omitempty"`
    FeeToken             string `json:"fee_token,omitempty"`
}

// StepExecutor handles the chain-specific transaction construction,
// signing, broadcast, and receipt polling for a single action step.
type StepExecutor interface {
    ExecuteStep(ctx context.Context, store *Store, action *Action, step *ActionStep, opts ExecuteOptions, persist func() error) (*StepResult, error)
    EstimateStep(ctx context.Context, action *Action, step *ActionStep, opts EstimateOptions) (StepGasEstimate, error)
    EffectiveSender() common.Address
}

// StepResult holds the outputs from a successfully executed step.
type StepResult struct {
    ConfirmedBlock int64 // block number where the tx was confirmed
}

// IsTempoChain returns true for known Tempo chain IDs.
func IsTempoChain(chainID int64) bool {
    switch chainID {
    case 4217, 42431, 31318:
        return true
    }
    return false
}

// ResolveStepExecutor returns the appropriate executor for the chain.
func ResolveStepExecutor(chainID int64, txSigner signer.Signer) StepExecutor {
    if IsTempoChain(chainID) {
        return NewTempoStepExecutor(txSigner)
    }
    return NewEVMStepExecutor(txSigner)
}
```

Note: `NewTempoStepExecutor` will be a stub until Task 2. For now, have it panic so we don't accidentally use it before it's ready.

- [ ] **Step 2: Extract EVM executor**

Create `internal/execution/evm_executor.go` by extracting the current `executeStep` function from `executor.go`. The `EVMStepExecutor` struct wraps the existing signer and implements `StepExecutor`.

Key points:
- `EffectiveSender()` returns `signer.Address()`
- `ExecuteStep()` is the current `executeStep` function with its signature adapted to the interface
- `EstimateStep()` extracts per-step estimation logic from `EstimateActionGas`
- Keep all existing helper functions (`ensurePostConfirmationStateVisible`, `verifyBridgeSettlement`, nonce locking, etc.) in `executor.go` since they're called by `ExecuteAction`

- [ ] **Step 3: Modify ExecuteAction to use StepExecutor**

In `executor.go`, modify `ExecuteAction` to:
1. Parse chain ID from the first step's `ChainID` field
2. Call `ResolveStepExecutor(chainID, txSigner)`
3. Use `executor.EffectiveSender()` to stamp `action.FromAddress` (replacing `txSigner.Address().Hex()`)
4. Delegate step execution to `executor.ExecuteStep()` inside the step loop
5. Keep post-step hooks (`ensurePostConfirmationStateVisible`, `verifyBridgeSettlement`) in `ExecuteAction`, called after `ExecuteStep` returns, guarded by step type

- [ ] **Step 4: Run all existing tests**

```bash
go test ./internal/execution/... -v -count=1
go test ./internal/app/... -v -count=1
go test ./... -count=1
go vet ./...
```

All must pass unchanged. This is a pure refactor — no behavior changes.

- [ ] **Step 5: Commit**

```bash
git add internal/execution/step_executor.go internal/execution/evm_executor.go internal/execution/executor.go
git commit -m "refactor: extract StepExecutor interface with EVM implementation"
```

---

### Task 2: Action Model Extension (Calls field)

**Files:**
- Modify: `internal/execution/types.go`
- Create: `internal/execution/types_test.go` (or add to existing)

- [ ] **Step 1: Write test for Calls serialization**

```go
func TestActionStepCallsSerialization(t *testing.T) {
    step := ActionStep{
        StepID:  "test",
        Type:    StepTypeSwap,
        Target:  "",
        Data:    "",
        Value:   "",
        Calls: []StepCall{
            {Target: "0xabc", Data: "0x1234", Value: "0"},
            {Target: "0xdef", Data: "0x5678", Value: "0"},
        },
    }
    data, err := json.Marshal(step)
    require.NoError(t, err)
    require.Contains(t, string(data), `"calls"`)

    var decoded ActionStep
    require.NoError(t, json.Unmarshal(data, &decoded))
    require.Len(t, decoded.Calls, 2)
    require.Equal(t, "0xabc", decoded.Calls[0].Target)
}

func TestActionStepCallsOmittedWhenEmpty(t *testing.T) {
    step := ActionStep{StepID: "test", Type: StepTypeSwap, Target: "0xabc", Data: "0x1234", Value: "0"}
    data, err := json.Marshal(step)
    require.NoError(t, err)
    require.NotContains(t, string(data), `"calls"`)
}
```

- [ ] **Step 2: Add StepCall type and Calls field**

In `internal/execution/types.go`:

```go
// StepCall represents a single call within a batched Tempo transaction step.
type StepCall struct {
    Target string `json:"target"`
    Data   string `json:"data"`
    Value  string `json:"value"`
}
```

Add to `ActionStep`:
```go
Calls []StepCall `json:"calls,omitempty"`
```

- [ ] **Step 3: Run tests**

```bash
go test ./internal/execution/... -v -count=1
```

- [ ] **Step 4: Commit**

```bash
git add internal/execution/types.go internal/execution/types_test.go
git commit -m "feat: add StepCall type and Calls field to ActionStep for Tempo batching"
```

---

### Task 3: Tempo Provider — Batched BuildSwapAction

**Files:**
- Modify: `internal/providers/tempo/client.go` (on branch — `git show cca9373:...`)
- Modify: `internal/providers/tempo/client_test.go`

This task modifies the Tempo provider's `BuildSwapAction` to produce batched steps (1 step with `Calls` array) instead of 2 separate steps.

- [ ] **Step 1: Write test for batched swap action**

Add to `internal/providers/tempo/client_test.go`:

```go
func TestBuildSwapAction_BatchesCalls(t *testing.T) {
    // Setup httptest server or mock that returns allowance=0 (needs approval)
    // Call BuildSwapAction
    // Assert: action.Steps has exactly 1 step
    // Assert: step.Calls has 2 entries (approve + swap)
    // Assert: step.Calls[0].Target is the token address (approve)
    // Assert: step.Calls[1].Target is the DEX address (swap)
    // Assert: step.Type is StepTypeSwap
}

func TestBuildSwapAction_SingleCallWhenApproved(t *testing.T) {
    // Setup mock that returns large allowance (no approval needed)
    // Call BuildSwapAction
    // Assert: action.Steps has exactly 1 step
    // Assert: step.Calls has 1 entry (swap only)
}
```

- [ ] **Step 2: Modify BuildSwapAction to produce batched Calls**

In `internal/providers/tempo/client.go`, change `BuildSwapAction` to:
1. Build the approval calldata as before
2. Build the swap calldata as before
3. Instead of appending separate steps, create a single step with `Calls`:

```go
var calls []execution.StepCall
if allowance.Cmp(approvalAmount) < 0 {
    approveData, err := tempoERC20.Pack("approve", dexAddr, approvalAmount)
    if err != nil {
        return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack tempo approve calldata", err)
    }
    calls = append(calls, execution.StepCall{
        Target: tokenIn.Hex(),
        Data:   "0x" + common.Bytes2Hex(approveData),
        Value:  "0",
    })
}
calls = append(calls, execution.StepCall{
    Target: dexAddr.Hex(),
    Data:   "0x" + common.Bytes2Hex(swapData),
    Value:  "0",
})

action.Steps = append(action.Steps, execution.ActionStep{
    StepID:          stepID,
    Type:            execution.StepTypeSwap,
    Status:          execution.StepStatusPending,
    ChainID:         req.Chain.CAIP2,
    RPCURL:          rpcURL,
    Description:     description,
    Calls:           calls,
    ExpectedOutputs: expected,
})
```

- [ ] **Step 3: Run tests**

```bash
go test ./internal/providers/tempo/... -v -count=1
go test ./... -count=1
```

- [ ] **Step 4: Commit**

```bash
git add internal/providers/tempo/client.go internal/providers/tempo/client_test.go
git commit -m "feat(tempo): batch approve+swap into single step with Calls"
```

---

### Task 4: Registry Additions

**Files:**
- Modify: `internal/registry/contracts.go`
- Create: `internal/registry/contracts_test.go` (or add to existing)

- [ ] **Step 1: Write tests for TempoFeeToken**

```go
func TestTempoFeeToken(t *testing.T) {
    token, ok := TempoFeeToken(4217)
    require.True(t, ok)
    require.Equal(t, "0x20c000000000000000000000b9537d11c60e8b50", strings.ToLower(token))

    _, ok = TempoFeeToken(1)
    require.False(t, ok)
}
```

- [ ] **Step 2: Add TempoFeeToken to registry**

In `internal/registry/contracts.go`:

```go
var tempoFeeTokenByChainID = map[int64]string{
    4217:  "0x20c000000000000000000000b9537d11c60e8b50", // USDC.e mainnet
    42431: "0x20c0000000000000000000000000000000000001", // AlphaUSD testnet
    31318: "0x20c0000000000000000000000000000000000001", // AlphaUSD devnet
}

func TempoFeeToken(chainID int64) (string, bool) {
    addr, ok := tempoFeeTokenByChainID[chainID]
    return addr, ok
}
```

Note: Testnet/devnet fee token addresses should be verified during spike. Use the USDC.e address confirmed on mainnet. For testnet/devnet, use AlphaUSD if that's what's available, or use the same USDC.e pattern.

- [ ] **Step 3: Run tests and commit**

```bash
go test ./internal/registry/... -v -count=1
git add internal/registry/contracts.go internal/registry/contracts_test.go
git commit -m "feat: add TempoFeeToken registry entries"
```

---

### Task 5: TempoStepExecutor — Core Execution

**Files:**
- Create: `internal/execution/tempo_executor.go`
- Create: `internal/execution/tempo_executor_test.go`

This is the core task. The `TempoStepExecutor` builds type 0x76 transactions and submits them via `tempo-go`.

- [ ] **Step 1: Write test for TempoStepExecutor.ExecuteStep**

Test with an httptest-based RPC mock that:
1. Returns gas estimate for `eth_estimateGas`
2. Returns gas price for `eth_gasPrice`
3. Returns pending nonce for `eth_getTransactionCount`
4. Accepts `eth_sendRawTransaction` and returns a tx hash
5. Returns a success receipt for `eth_getTransactionReceipt`

The test should verify:
- The tx is correctly constructed with `Calls` from the step
- The receipt is polled and step status is updated to confirmed
- `step.TxHash` is set

- [ ] **Step 2: Write test for TempoStepExecutor.EffectiveSender**

```go
func TestTempoEffectiveSender_LocalSigner(t *testing.T) {
    s, _ := signer.NewLocalSigner(signer.LocalSignerConfig{
        PrivateKeyHex: "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    })
    executor := NewTempoStepExecutor(s)
    // With a regular signer, EffectiveSender = signer address
    require.Equal(t, s.Address(), executor.EffectiveSender())
}
```

- [ ] **Step 3: Implement TempoStepExecutor**

Create `internal/execution/tempo_executor.go`:

Key implementation details:
- `ExecuteStep` builds a `transaction.Transaction` from `step.Calls`
- Sets `tx.FeeToken` from step metadata or registry default
- Uses `eth_estimateGas` for gas, `eth_gasPrice` for fee rate
- Signs with `transaction.SignTransaction(tx, tempoSigner)`
- Broadcasts with `tempo-go` client's `SendTransaction`
- Polls `eth_getTransactionReceipt` until confirmed (reuse existing receipt polling logic pattern from EVM executor)
- `EffectiveSender` returns signer address for regular signers, wallet address for TempoSigner (phase 2)
- `EstimateStep` computes gas estimate with fee-token denomination

The executor should handle the `--fee-token` flag value via `ExecuteOptions.FeeToken` (add this field to `ExecuteOptions`).

- [ ] **Step 4: Add FeeToken to ExecuteOptions**

In `executor.go`, add to `ExecuteOptions`:
```go
FeeToken string // optional; Tempo-only, defaults to chain's primary USDC
```

- [ ] **Step 5: Run all tests**

```bash
go test ./internal/execution/... -v -count=1
go test ./... -count=1
go vet ./...
```

- [ ] **Step 6: Commit**

```bash
git add internal/execution/tempo_executor.go internal/execution/tempo_executor_test.go internal/execution/executor.go
git commit -m "feat: add TempoStepExecutor for type 0x76 transaction execution"
```

---

### Task 6: Policy Updates for Batched Calls

**Files:**
- Modify: `internal/execution/policy_basic.go`
- Modify: `internal/execution/policy_basic_test.go`

- [ ] **Step 1: Write test for batched Tempo swap policy**

```go
func TestValidateStepPolicy_TempoBatchedSwap(t *testing.T) {
    // Create step with Calls (approve + swap)
    // Verify policy passes when targets/selectors match DEX and token
    // Verify policy fails when DEX target doesn't match registry
}
```

- [ ] **Step 2: Update validateSwapPolicy for Calls**

In `validateSwapPolicy`, when `action.Provider == "tempo"` and `step.Calls` is non-empty:
- Iterate each call in `step.Calls`
- For calls with approve selector: validate target is a known token
- For calls with tempo swap selector: validate target matches `registry.TempoStablecoinDEX(chainID)`

When `step.Calls` is empty, fall through to existing `step.Target`/`step.Data` validation.

- [ ] **Step 3: Run tests and commit**

```bash
go test ./internal/execution/... -v -count=1
git add internal/execution/policy_basic.go internal/execution/policy_basic_test.go
git commit -m "feat: extend swap policy validation for Tempo batched Calls"
```

---

### Task 7: Fee-token Gas Estimation

**Files:**
- Modify: `internal/execution/estimate.go`
- Modify: `internal/execution/estimate_test.go` (or the runner test)

- [ ] **Step 1: Write test for Tempo fee-token estimation**

```go
func TestEstimateActionGas_TempoFeeToken(t *testing.T) {
    // Create a Tempo action with batched Calls
    // Mock RPC that returns gas estimate and gas price
    // Verify estimate output includes fee_unit="USDC.e" and fee_token address
    // Verify fee amounts are in fee-token base units, not ETH wei
}
```

- [ ] **Step 2: Modify EstimateActionGas to delegate to StepExecutor**

Update `EstimateActionGas` to:
1. Parse chain ID from each step
2. For Tempo steps: use `TempoStepExecutor.EstimateStep()` which handles `Calls` and fee-token math
3. For EVM steps: use `EVMStepExecutor.EstimateStep()` (existing logic extracted)
4. Add `FeeUnit` and `FeeToken` fields to `ActionGasEstimateStep` and `ActionGasEstimateChainTotal`

- [ ] **Step 3: Run tests and commit**

```bash
go test ./internal/execution/... -v -count=1
go test ./... -count=1
git add internal/execution/estimate.go internal/execution/estimate_test.go
git commit -m "feat: add fee-token gas estimation for Tempo actions"
```

---

### Task 8: Runner Wiring — --fee-token flag + Tempo submit

**Files:**
- Modify: `internal/app/runner.go`
- Modify: `internal/app/runner_test.go`

- [ ] **Step 1: Add --fee-token flag to execution commands**

In `runner.go`, add `--fee-token` flag to swap/approvals/transfer plan and submit commands. Wire it into `ExecuteOptions.FeeToken`.

- [ ] **Step 2: Wire TempoStepExecutor in executeActionWithTimeout**

The `executeActionWithTimeout` helper calls `execution.ExecuteAction`. Since `ExecuteAction` now resolves the executor internally via `ResolveStepExecutor`, no change is needed here — it just works. But verify with a test.

- [ ] **Step 3: Write integration test for Tempo swap submit**

Add to `internal/app/runner_test.go`:

```go
func TestSwapSubmit_TempoProvider(t *testing.T) {
    // Create a pre-planned Tempo swap action in the action store
    // Mock the Tempo RPC endpoints
    // Run swap submit --action-id <id>
    // Verify the action status is completed
}
```

- [ ] **Step 4: Run all tests**

```bash
go test ./internal/app/... -v -count=1
go test ./... -count=1
go vet ./...
```

- [ ] **Step 5: Commit**

```bash
git add internal/app/runner.go internal/app/runner_test.go
git commit -m "feat: wire --fee-token flag and Tempo executor in submit commands"
```

---

### Task 9: Agent Wallet Signer (Phase 2)

**Files:**
- Create: `internal/execution/signer/tempo.go`
- Create: `internal/execution/signer/tempo_test.go`
- Modify: `internal/app/runner.go`

- [ ] **Step 1: Write test for TempoWalletSigner**

```go
func TestTempoWalletSigner_WalletAddress(t *testing.T) {
    // Create signer from known key + wallet address
    // Verify WalletAddress() returns the wallet address, not the key address
}

func TestTempoWalletSigner_SignTempoTx(t *testing.T) {
    // Create signer, build a tempo tx, sign it
    // Verify signature is valid
}
```

- [ ] **Step 2: Define TempoSigner interface and TempoWalletSigner**

Create `internal/execution/signer/tempo.go`:

```go
package signer

import (
    "encoding/json"
    "fmt"
    "os/exec"
    "strings"
    "time"

    "github.com/ethereum/go-ethereum/common"
    temposigner "github.com/tempoxyz/tempo-go/pkg/signer"
    "github.com/tempoxyz/tempo-go/pkg/transaction"
)

// TempoSigner extends the base Signer with Tempo-specific capabilities.
type TempoSigner interface {
    WalletAddress() common.Address
    SignTempoTx(tx *transaction.Transaction) error
}

// TempoWalletSigner signs Tempo transactions using a delegated access key
// from the tempo wallet CLI.
type TempoWalletSigner struct {
    walletAddr common.Address
    inner      *temposigner.Signer // from tempo-go
    keyAddr    common.Address
}

func NewTempoWalletSigner(walletAddr common.Address, privateKeyHex string) (*TempoWalletSigner, error) {
    inner, err := temposigner.NewSigner(privateKeyHex)
    if err != nil {
        return nil, fmt.Errorf("create tempo signer: %w", err)
    }
    return &TempoWalletSigner{
        walletAddr: walletAddr,
        inner:      inner,
        keyAddr:    inner.Address(),
    }, nil
}

func (s *TempoWalletSigner) Address() common.Address { return s.keyAddr }
func (s *TempoWalletSigner) WalletAddress() common.Address { return s.walletAddr }

func (s *TempoWalletSigner) SignTempoTx(tx *transaction.Transaction) error {
    return transaction.SignTransaction(tx, s.inner)
}

// Also implement the base Signer interface so it can be passed to ExecuteAction.
// SignTx is not used for Tempo chains but satisfies the interface.
func (s *TempoWalletSigner) SignTx(chainID *big.Int, tx *types.Transaction) (*types.Transaction, error) {
    return nil, fmt.Errorf("TempoWalletSigner does not support standard EVM SignTx; use SignTempoTx for Tempo chains")
}
```

- [ ] **Step 3: Add wallet discovery from tempo CLI**

In the same file, add:

```go
type tempoWhoamiResponse struct {
    Ready   bool   `json:"ready"`
    Wallet  string `json:"wallet"`
    Key     struct {
        Address       string `json:"address"`
        Key           string `json:"key"`
        ChainID       int    `json:"chain_id"`
        SpendingLimit struct {
            Remaining string `json:"remaining"`
        } `json:"spending_limit"`
        ExpiresAt string `json:"expires_at"`
    } `json:"key"`
}

func NewTempoSignerFromCLI() (*TempoWalletSigner, []string, error) {
    // Returns signer + warnings (e.g., low balance, near expiry)
    tempoBin, err := exec.LookPath("tempo")
    if err != nil {
        return nil, nil, fmt.Errorf("tempo CLI is required for --signer tempo. Install: curl -fsSL https://tempo.xyz/install | sh")
    }

    out, err := exec.Command(tempoBin, "wallet", "-j", "whoami").Output()
    if err != nil {
        return nil, nil, fmt.Errorf("failed to query tempo wallet: %w", err)
    }

    var resp tempoWhoamiResponse
    if err := json.Unmarshal(out, &resp); err != nil {
        return nil, nil, fmt.Errorf("parse tempo wallet output: %w", err)
    }

    if !resp.Ready {
        return nil, nil, fmt.Errorf("tempo wallet is not logged in; run 'tempo wallet login' to set up your agent wallet")
    }

    var warnings []string

    // Check expiry
    if resp.Key.ExpiresAt != "" {
        expiry, err := time.Parse(time.RFC3339, resp.Key.ExpiresAt)
        if err == nil && time.Now().After(expiry) {
            return nil, nil, fmt.Errorf("tempo wallet access key has expired; run 'tempo wallet login' to refresh")
        }
        if err == nil && time.Until(expiry) < 24*time.Hour {
            warnings = append(warnings, fmt.Sprintf("tempo wallet key expires in %s", time.Until(expiry).Round(time.Hour)))
        }
    }

    // Check spending limit
    // (parse remaining as float, warn if low)

    walletAddr := common.HexToAddress(resp.Wallet)
    signer, err := NewTempoWalletSigner(walletAddr, resp.Key.Key)
    if err != nil {
        return nil, nil, err
    }
    return signer, warnings, nil
}
```

- [ ] **Step 4: Wire --signer tempo in runner.go**

In `newExecutionSigner`, add the `"tempo"` case:

```go
case "tempo":
    if privateKey != "" || strings.TrimSpace(os.Getenv(EnvPrivateKey)) != "" {
        return nil, clierr.New(clierr.CodeUsage, "--signer tempo cannot be combined with --private-key or local key env vars; tempo wallet manages keys automatically")
    }
    tempoSigner, warnings, err := signer.NewTempoSignerFromCLI()
    if err != nil {
        return nil, clierr.Wrap(clierr.CodeSigner, "tempo wallet", err)
    }
    for _, w := range warnings {
        // emit as envelope warning
    }
    return tempoSigner, nil
```

- [ ] **Step 5: Update TempoStepExecutor to use TempoSigner**

In `tempo_executor.go`, update `ExecuteStep` and `EffectiveSender`:
- Check if signer implements `TempoSigner` interface
- If yes: use `WalletAddress()` for effective sender, `SignTempoTx()` for signing
- If no: use `signer.Address()` for sender, build standard secp256k1 signature

- [ ] **Step 6: Run all tests**

```bash
go test ./internal/execution/signer/... -v -count=1
go test ./... -count=1
go vet ./...
```

- [ ] **Step 7: Commit**

```bash
git add internal/execution/signer/tempo.go internal/execution/signer/tempo_test.go internal/app/runner.go internal/execution/tempo_executor.go
git commit -m "feat: add --signer tempo with agent wallet discovery from tempo CLI"
```

---

### Task 10: Documentation & Changelog

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `README.md`
- Modify: `AGENTS.md` (which is CLAUDE.md in this repo)
- Modify: `docs/guides/swap.mdx`
- Modify: `docs/concepts/providers-and-auth.mdx`

- [ ] **Step 1: Update CHANGELOG.md**

Add under `## [Unreleased]`:

**Added:**
- Tempo native execution: `swap submit` now broadcasts type 0x76 transactions with fee-token payments on Tempo mainnet, testnet, and devnet.
- Batched approve+swap in a single atomic transaction on Tempo (no separate approval tx).
- `--signer tempo` for agent wallet support via the Tempo CLI, with delegated access keys, spending limits, and expiry checks.
- `--fee-token` flag for execution commands (Tempo-only, defaults to USDC.e).
- `actions estimate` now supports Tempo actions with fee-token-denominated gas estimates.

**Changed:**
- Execution engine refactored to `StepExecutor` interface; EVM path extracted unchanged, Tempo path added.
- `actions estimate` response now includes `fee_unit` and `fee_token` fields for Tempo chains.

- [ ] **Step 2: Update README.md caveats**

Update the Quotes and Execution caveats sections to reflect:
- `actions estimate` now works for Tempo (remove the "intentionally unavailable" caveat)
- Document `--signer tempo` and `--fee-token`

- [ ] **Step 3: Update AGENTS.md**

Update the non-obvious-but-important section with:
- `--signer tempo` reads agent wallet from `tempo wallet -j whoami`
- Tempo execution uses type 0x76 transactions with batched calls
- `--fee-token` defaults to USDC.e on Tempo mainnet

- [ ] **Step 4: Update Mintlify docs**

Update `docs/guides/swap.mdx` with Tempo execution examples.
Update `docs/concepts/providers-and-auth.mdx` with `--signer tempo` documentation.

- [ ] **Step 5: Commit**

```bash
git add CHANGELOG.md README.md AGENTS.md docs/
git commit -m "docs: document Tempo native execution, agent wallet signer, and fee-token estimation"
```

---

### Task 11: Final Verification

- [ ] **Step 1: Full test suite**

```bash
go test ./... -count=1
go test -race ./...
go vet ./...
```

- [ ] **Step 2: Build binary**

```bash
go build -o defi ./cmd/defi
```

- [ ] **Step 3: Smoke test Tempo commands**

```bash
./defi providers list --results-only | grep tempo
./defi swap quote --provider tempo --chain tempo --from-asset USDC.e --to-asset pathUSD --amount 1000000 --results-only
./defi swap plan --provider tempo --chain tempo --from-asset USDC.e --to-asset pathUSD --amount 1000000 --from-address 0xd341c019aea93ee863bb8cbb84b92dc23b546781 --results-only
```

Verify plan output shows batched Calls in the action steps.

- [ ] **Step 4: Smoke test actions estimate**

```bash
./defi actions estimate --action-id <id-from-above> --results-only
```

Verify output includes `fee_unit: "USDC.e"` and fee amounts are in stablecoin denomination.

- [ ] **Step 5: Clean up**

```bash
rm -f ./defi
```
