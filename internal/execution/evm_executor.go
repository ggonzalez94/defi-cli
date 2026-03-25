package execution

import (
	"context"
	"fmt"
	"math/big"
	"strings"
	"sync"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
)

// EVMStepExecutor executes action steps as EIP-1559 transactions on
// EVM-compatible chains. It manages its own RPC client connections internally.
type EVMStepExecutor struct {
	backend    EVMSubmitBackend
	rpcClients map[string]*ethclient.Client
	mu         sync.Mutex
}

// NewEVMStepExecutor creates an EVMStepExecutor backed by the given submit backend.
func NewEVMStepExecutor(backend EVMSubmitBackend) *EVMStepExecutor {
	return &EVMStepExecutor{
		backend:    backend,
		rpcClients: make(map[string]*ethclient.Client),
	}
}

// EffectiveSender returns the address that will sign and send transactions.
func (e *EVMStepExecutor) EffectiveSender() common.Address {
	if e == nil || e.backend == nil {
		return common.Address{}
	}
	return e.backend.EffectiveSender()
}

// Close closes all cached RPC client connections.
func (e *EVMStepExecutor) Close() {
	e.mu.Lock()
	defer e.mu.Unlock()
	for _, client := range e.rpcClients {
		if client != nil {
			client.Close()
		}
	}
	e.rpcClients = make(map[string]*ethclient.Client)
}

// getClient returns a cached or newly created ethclient for the given RPC URL.
func (e *EVMStepExecutor) getClient(ctx context.Context, rpcURL string) (*ethclient.Client, error) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if client := e.rpcClients[rpcURL]; client != nil {
		return client, nil
	}
	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
	}
	e.rpcClients[rpcURL] = client
	return client, nil
}

// ExecuteStep executes a single action step as an EIP-1559 transaction.
// It preserves exactly the same behavior as the former executeStep function:
// chain ID validation, policy checks, simulation, gas estimation, nonce
// management, signing, broadcast, and receipt polling.
//
// The caller (ExecuteAction) is responsible for persistence and post-step
// hooks (ensurePostConfirmationStateVisible, verifyBridgeSettlement).
func (e *EVMStepExecutor) ExecuteStep(ctx context.Context, store *Store, action *Action, step *ActionStep, opts ExecuteOptions) error {
	rpcURL := strings.TrimSpace(step.RPCURL)
	client, err := e.getClient(ctx, rpcURL)
	if err != nil {
		return err
	}

	chainID, err := client.ChainID(ctx)
	if err != nil {
		return clierr.Wrap(clierr.CodeUnavailable, "read chain id", err)
	}
	if step.ChainID != "" {
		expected := fmt.Sprintf("eip155:%d", chainID.Int64())
		if !strings.EqualFold(strings.TrimSpace(step.ChainID), expected) {
			return clierr.New(clierr.CodeActionPlan, fmt.Sprintf("step chain mismatch: expected %s, got %s", expected, step.ChainID))
		}
	}
	if !common.IsHexAddress(step.Target) {
		return clierr.New(clierr.CodeUsage, "invalid step target address")
	}
	target := common.HexToAddress(step.Target)
	step.Target = target.Hex()
	data, err := decodeHex(step.Data)
	if err != nil {
		return clierr.Wrap(clierr.CodeUsage, "decode step calldata", err)
	}
	if err := validateStepPolicy(action, step, chainID.Int64(), data, opts); err != nil {
		return err
	}
	value, ok := new(big.Int).SetString(step.Value, 10)
	if !ok {
		return clierr.New(clierr.CodeUsage, "invalid step value")
	}
	sender := e.EffectiveSender()
	if sender == (common.Address{}) {
		return clierr.New(clierr.CodeSigner, "missing EVM submission backend sender")
	}
	msg := ethereum.CallMsg{From: sender, To: &target, Value: value, Data: data}

	// Build a persist callback for the receipt-polling phase.
	persist := func() error {
		action.Touch()
		if store != nil {
			if err := store.Save(*action); err != nil {
				return clierr.Wrap(clierr.CodeInternal, "persist action state", err)
			}
		}
		return nil
	}

	if txHash, ok := normalizeStepTxHash(step.TxHash); ok {
		step.Status = StepStatusSubmitted
		step.Error = ""
		if err := safePersist(persist); err != nil {
			return err
		}
		confirmedBlock, err := waitForStepConfirmation(ctx, client, step, msg, txHash, opts, persist)
		if err != nil {
			return err
		}
		storeConfirmedBlock(step, confirmedBlock)
		return nil
	}

	if opts.Simulate {
		if _, err := client.CallContract(ctx, msg, nil); err != nil {
			return wrapEVMExecutionError(clierr.CodeActionSim, "simulate step (eth_call)", err)
		}
		step.Status = StepStatusSimulated
		step.Error = ""
		if err := safePersist(persist); err != nil {
			return err
		}
	}

	gasLimit, err := client.EstimateGas(ctx, msg)
	if err != nil {
		return wrapEVMExecutionError(clierr.CodeActionSim, "estimate gas", err)
	}
	gasLimit = uint64(float64(gasLimit) * opts.GasMultiplier)
	if gasLimit == 0 {
		return clierr.New(clierr.CodeActionSim, "estimate gas returned zero")
	}

	tipCap, err := resolveTipCap(ctx, client, opts.MaxPriorityFeeGwei)
	if err != nil {
		return err
	}
	header, err := client.HeaderByNumber(ctx, nil)
	if err != nil {
		return clierr.Wrap(clierr.CodeUnavailable, "fetch latest header", err)
	}
	baseFee := header.BaseFee
	if baseFee == nil {
		baseFee = big.NewInt(1_000_000_000)
	}
	feeCap, err := resolveFeeCap(baseFee, tipCap, opts.MaxFeeGwei)
	if err != nil {
		return err
	}
	unlockNonce := acquireSignerNonceLock(chainID, sender)
	defer unlockNonce()
	nonce, err := client.PendingNonceAt(ctx, sender)
	if err != nil {
		return clierr.Wrap(clierr.CodeUnavailable, "fetch nonce", err)
	}

	tx := types.NewTx(&types.DynamicFeeTx{
		ChainID:   chainID,
		Nonce:     nonce,
		GasTipCap: tipCap,
		GasFeeCap: feeCap,
		Gas:       gasLimit,
		To:        &target,
		Value:     value,
		Data:      data,
	})
	txHash, err := e.backend.SubmitDynamicFeeTx(ctx, rpcURL, chainID, tx)
	if err != nil {
		return err
	}
	step.Status = StepStatusSubmitted
	step.TxHash = txHash.Hex()
	step.Error = ""
	if err := safePersist(persist); err != nil {
		return err
	}
	confirmedBlock, err := waitForStepConfirmation(ctx, client, step, msg, txHash, opts, persist)
	if err != nil {
		return err
	}
	storeConfirmedBlock(step, confirmedBlock)
	return nil
}

// storeConfirmedBlock records the confirmed block number in the step's
// ExpectedOutputs so the caller can use it for cross-step ordering.
func storeConfirmedBlock(step *ActionStep, block *big.Int) {
	if step == nil || block == nil {
		return
	}
	setStepOutput(step, "_confirmed_block_number", block.String())
}

// EstimateStep returns a gas/fee estimate for a single step.
// Not yet implemented — will be wired in a later task.
func (e *EVMStepExecutor) EstimateStep(_ context.Context, _ *Action, _ *ActionStep, _ EstimateOptions) (StepGasEstimate, error) {
	return StepGasEstimate{}, fmt.Errorf("EVMStepExecutor.EstimateStep not yet implemented")
}
