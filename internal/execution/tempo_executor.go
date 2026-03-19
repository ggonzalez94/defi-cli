package execution

import (
	"context"
	"crypto/ecdsa"
	"fmt"
	"math/big"
	"strings"
	"sync"
	"time"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution/signer"
	"github.com/ggonzalez94/defi-cli/internal/registry"
	temposigner "github.com/tempoxyz/tempo-go/pkg/signer"
	"github.com/tempoxyz/tempo-go/pkg/transaction"

	tempoclient "github.com/tempoxyz/tempo-go/pkg/client"
)

// privateKeyProvider is satisfied by LocalSigner (and any future signer that
// can expose the raw ECDSA key for tempo-go signing).
type privateKeyProvider interface {
	PrivateKey() *ecdsa.PrivateKey
}

// TempoStepExecutor executes action steps as Tempo type 0x76 transactions.
// It batches multiple calls into a single Tempo transaction, sets a
// stablecoin fee token, and uses the tempo-go SDK for signing/broadcast.
type TempoStepExecutor struct {
	txSigner    signer.Signer
	tempoSigner *temposigner.Signer
	rpcClients  map[string]*ethclient.Client
	mu          sync.Mutex
}

// NewTempoStepExecutor creates a TempoStepExecutor. If the provided signer
// implements TempoSigner (e.g. TempoWalletSigner), its TempoGoSigner() is
// used directly. Otherwise, if the signer exposes a PrivateKey(), a tempo-go
// signer is derived automatically. ExecuteStep will return an error at signing
// time if neither path produces a tempo signer.
func NewTempoStepExecutor(txSigner signer.Signer) *TempoStepExecutor {
	var ts *temposigner.Signer
	if tempoS, ok := txSigner.(signer.TempoSigner); ok {
		ts = tempoS.TempoGoSigner()
	} else if pkp, ok := txSigner.(privateKeyProvider); ok {
		if pk := pkp.PrivateKey(); pk != nil {
			ts = temposigner.NewSignerFromKey(pk)
		}
	}
	return &TempoStepExecutor{
		txSigner:    txSigner,
		tempoSigner: ts,
		rpcClients:  make(map[string]*ethclient.Client),
	}
}

// EffectiveSender returns the address that will act as the on-chain sender.
// For TempoSigner (smart-wallet), this is the wallet address, not the signing
// key address.
func (e *TempoStepExecutor) EffectiveSender() common.Address {
	if ts, ok := e.txSigner.(signer.TempoSigner); ok {
		return ts.WalletAddress()
	}
	return e.txSigner.Address()
}

// Close closes all cached RPC client connections.
func (e *TempoStepExecutor) Close() {
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
func (e *TempoStepExecutor) getClient(ctx context.Context, rpcURL string) (*ethclient.Client, error) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if client := e.rpcClients[rpcURL]; client != nil {
		return client, nil
	}
	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "connect tempo rpc", err)
	}
	e.rpcClients[rpcURL] = client
	return client, nil
}

// ExecuteStep builds, signs, and broadcasts a Tempo type 0x76 transaction for
// the given action step. Batched calls in step.Calls are combined into a
// single Tempo transaction.
func (e *TempoStepExecutor) ExecuteStep(ctx context.Context, store *Store, action *Action, step *ActionStep, opts ExecuteOptions) error {
	if e.tempoSigner == nil {
		return clierr.New(clierr.CodeSigner, "tempo signer required; provide a local signing key (--private-key, DEFI_PRIVATE_KEY, or key file)")
	}

	rpcURL := strings.TrimSpace(step.RPCURL)
	ethClient, err := e.getClient(ctx, rpcURL)
	if err != nil {
		return err
	}

	chainID, err := ParseEVMChainID(step.ChainID)
	if err != nil {
		return clierr.Wrap(clierr.CodeUsage, "parse step chain id", err)
	}

	// Build calls from step.Calls; fall back to single Target/Data/Value.
	calls := step.Calls
	if len(calls) == 0 && strings.TrimSpace(step.Target) != "" {
		calls = []StepCall{{
			Target: step.Target,
			Data:   step.Data,
			Value:  step.Value,
		}}
	}
	if len(calls) == 0 {
		return clierr.New(clierr.CodeUsage, "step has no calls")
	}

	// Policy validation.
	if err := validateStepPolicyCalls(action, step, chainID, calls, opts); err != nil {
		return err
	}

	// Resolve fee token.
	feeTokenAddr := common.Address{}
	if ft := strings.TrimSpace(opts.FeeToken); ft != "" && common.IsHexAddress(ft) {
		feeTokenAddr = common.HexToAddress(ft)
	} else if ft, ok := registry.TempoFeeToken(chainID); ok {
		feeTokenAddr = common.HexToAddress(ft)
	}

	// Build tempo-go transaction calls.
	txCalls := make([]transaction.Call, 0, len(calls))
	for _, c := range calls {
		target := common.HexToAddress(c.Target)
		data, err := decodeHex(c.Data)
		if err != nil {
			return clierr.Wrap(clierr.CodeUsage, "decode call data", err)
		}
		value := big.NewInt(0)
		if strings.TrimSpace(c.Value) != "" {
			v, ok := new(big.Int).SetString(strings.TrimSpace(c.Value), 10)
			if ok {
				value = v
			}
		}
		txCalls = append(txCalls, transaction.Call{
			To:    &target,
			Value: value,
			Data:  data,
		})
	}

	// Estimate gas via eth_estimateGas for each call, summing results.
	// For a batched transaction the combined gas must cover all calls.
	var totalGas uint64
	for _, c := range txCalls {
		msg := ethereum.CallMsg{
			From:  e.txSigner.Address(),
			To:    c.To,
			Value: c.Value,
			Data:  c.Data,
		}
		gas, err := ethClient.EstimateGas(ctx, msg)
		if err != nil {
			return wrapEVMExecutionError(clierr.CodeActionSim, "estimate gas", err)
		}
		totalGas += gas
	}
	totalGas = uint64(float64(totalGas) * opts.GasMultiplier)
	if totalGas == 0 {
		return clierr.New(clierr.CodeActionSim, "estimate gas returned zero")
	}

	// Fee params.
	tipCap, err := resolveTipCap(ctx, ethClient, opts.MaxPriorityFeeGwei)
	if err != nil {
		return err
	}
	header, err := ethClient.HeaderByNumber(ctx, nil)
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

	// Nonce.
	tempoClient := tempoclient.New(rpcURL)
	nonce, err := tempoClient.GetTransactionCount(ctx, e.txSigner.Address().Hex())
	if err != nil {
		return clierr.Wrap(clierr.CodeUnavailable, "fetch tempo nonce", err)
	}

	// Build & sign transaction.
	tx := transaction.NewBuilder(big.NewInt(chainID)).
		SetGas(totalGas).
		SetMaxFeePerGas(feeCap).
		SetMaxPriorityFeePerGas(tipCap).
		SetNonce(nonce).
		SetFeeToken(feeTokenAddr).
		Build()
	tx.Calls = txCalls

	if err := transaction.SignTransaction(tx, e.tempoSigner); err != nil {
		return clierr.Wrap(clierr.CodeSigner, "sign tempo transaction", err)
	}

	serialized, err := transaction.Serialize(tx, nil)
	if err != nil {
		return clierr.Wrap(clierr.CodeInternal, "serialize tempo transaction", err)
	}

	// Build a persist callback.
	persist := func() error {
		action.Touch()
		if store != nil {
			if err := store.Save(*action); err != nil {
				return clierr.Wrap(clierr.CodeInternal, "persist action state", err)
			}
		}
		return nil
	}

	// Broadcast.
	txHashHex, err := tempoClient.SendRawTransaction(ctx, serialized)
	if err != nil {
		return clierr.Wrap(clierr.CodeUnavailable, "broadcast tempo transaction", err)
	}

	step.Status = StepStatusSubmitted
	step.TxHash = txHashHex
	step.Error = ""
	if err := safePersist(persist); err != nil {
		return err
	}

	// Poll for receipt.
	txHash := common.HexToHash(txHashHex)
	confirmedBlock, err := waitForTempoReceipt(ctx, ethClient, step, txHash, opts, persist)
	if err != nil {
		return err
	}
	storeConfirmedBlock(step, confirmedBlock)
	return nil
}

// waitForTempoReceipt polls eth_getTransactionReceipt until the transaction
// is confirmed or the step timeout elapses.
func waitForTempoReceipt(ctx context.Context, client *ethclient.Client, step *ActionStep, txHash common.Hash, opts ExecuteOptions, persist func() error) (*big.Int, error) {
	waitCtx, cancel := context.WithTimeout(ctx, opts.StepTimeout)
	defer cancel()

	pollInterval := opts.PollInterval
	if pollInterval <= 0 {
		pollInterval = 2 * time.Second
	}
	ticker := time.NewTicker(pollInterval)
	defer ticker.Stop()

	for {
		receipt, err := client.TransactionReceipt(waitCtx, txHash)
		if err == nil && receipt != nil {
			if receipt.Status == types.ReceiptStatusSuccessful {
				step.Status = StepStatusConfirmed
				step.Error = ""
				if err := safePersist(persist); err != nil {
					return nil, err
				}
				if receipt.BlockNumber == nil {
					return nil, nil
				}
				return new(big.Int).Set(receipt.BlockNumber), nil
			}
			return nil, clierr.New(clierr.CodeUnavailable, "tempo transaction reverted on-chain")
		}
		if waitCtx.Err() != nil {
			return nil, clierr.Wrap(clierr.CodeActionTimeout, "timed out waiting for tempo receipt", waitCtx.Err())
		}
		select {
		case <-waitCtx.Done():
			return nil, clierr.Wrap(clierr.CodeActionTimeout, "timed out waiting for tempo receipt", waitCtx.Err())
		case <-ticker.C:
		}
	}
}

// EstimateStep returns a gas/fee estimate for a Tempo step.
func (e *TempoStepExecutor) EstimateStep(_ context.Context, _ *Action, _ *ActionStep, _ EstimateOptions) (StepGasEstimate, error) {
	return StepGasEstimate{}, fmt.Errorf("TempoStepExecutor.EstimateStep not yet implemented")
}

// validateStepPolicyCalls validates policy for steps that use batched Calls.
// For steps with non-empty single Target/Data it delegates to the existing
// validateStepPolicy. For batched Calls it runs provider-specific checks.
func validateStepPolicyCalls(action *Action, step *ActionStep, chainID int64, calls []StepCall, opts ExecuteOptions) error {
	// If the step has a populated Target (non-batched), use legacy validation.
	if strings.TrimSpace(step.Target) != "" && common.IsHexAddress(step.Target) {
		data, err := decodeHex(step.Data)
		if err != nil {
			return clierr.Wrap(clierr.CodeUsage, "decode step calldata", err)
		}
		return validateStepPolicy(action, step, chainID, data, opts)
	}

	// Batched calls: validate each call individually.
	if step.Type == StepTypeSwap && action != nil && strings.EqualFold(strings.TrimSpace(action.Provider), "tempo") {
		return validateTempoSwapCalls(chainID, calls)
	}

	return nil
}
