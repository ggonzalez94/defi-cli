package execution

import (
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"math/big"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution/signer"
)

type ExecuteOptions struct {
	Simulate           bool
	PollInterval       time.Duration
	StepTimeout        time.Duration
	GasMultiplier      float64
	MaxFeeGwei         string
	MaxPriorityFeeGwei string
}

func DefaultExecuteOptions() ExecuteOptions {
	return ExecuteOptions{
		Simulate:      true,
		PollInterval:  2 * time.Second,
		StepTimeout:   2 * time.Minute,
		GasMultiplier: 1.2,
	}
}

func ExecuteAction(ctx context.Context, store *Store, action *Action, txSigner signer.Signer, opts ExecuteOptions) error {
	if action == nil {
		return clierr.New(clierr.CodeInternal, "missing action")
	}
	if txSigner == nil {
		return clierr.New(clierr.CodeSigner, "missing signer")
	}
	if len(action.Steps) == 0 {
		return clierr.New(clierr.CodeUsage, "action has no executable steps")
	}
	if opts.PollInterval <= 0 {
		opts.PollInterval = 2 * time.Second
	}
	if opts.StepTimeout <= 0 {
		opts.StepTimeout = 2 * time.Minute
	}
	if opts.GasMultiplier <= 1 {
		opts.GasMultiplier = 1.2
	}
	action.Status = ActionStatusRunning
	action.FromAddress = txSigner.Address().Hex()
	action.Touch()
	if store != nil {
		_ = store.Save(*action)
	}

	for i := range action.Steps {
		step := &action.Steps[i]
		if step.Status == StepStatusConfirmed {
			continue
		}
		if strings.TrimSpace(step.RPCURL) == "" {
			markStepFailed(action, step, "missing rpc url")
			if store != nil {
				_ = store.Save(*action)
			}
			return clierr.New(clierr.CodeUsage, "missing rpc url for action step")
		}
		if strings.TrimSpace(step.Target) == "" {
			markStepFailed(action, step, "missing target")
			if store != nil {
				_ = store.Save(*action)
			}
			return clierr.New(clierr.CodeUsage, "missing target for action step")
		}
		client, err := ethclient.DialContext(ctx, step.RPCURL)
		if err != nil {
			markStepFailed(action, step, err.Error())
			if store != nil {
				_ = store.Save(*action)
			}
			return clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
		}

		if err := executeStep(ctx, client, txSigner, step, opts); err != nil {
			client.Close()
			markStepFailed(action, step, err.Error())
			if store != nil {
				_ = store.Save(*action)
			}
			return err
		}
		client.Close()
		action.Touch()
		if store != nil {
			_ = store.Save(*action)
		}
	}
	action.Status = ActionStatusCompleted
	action.Touch()
	if store != nil {
		_ = store.Save(*action)
	}
	return nil
}

func executeStep(ctx context.Context, client *ethclient.Client, txSigner signer.Signer, step *ActionStep, opts ExecuteOptions) error {
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
	target := common.HexToAddress(step.Target)
	data, err := decodeHex(step.Data)
	if err != nil {
		return clierr.Wrap(clierr.CodeUsage, "decode step calldata", err)
	}
	value, ok := new(big.Int).SetString(step.Value, 10)
	if !ok {
		return clierr.New(clierr.CodeUsage, "invalid step value")
	}
	msg := ethereum.CallMsg{From: txSigner.Address(), To: &target, Value: value, Data: data}

	if opts.Simulate {
		if _, err := client.CallContract(ctx, msg, nil); err != nil {
			return clierr.Wrap(clierr.CodeActionSim, "simulate step (eth_call)", err)
		}
		step.Status = StepStatusSimulated
	}

	gasLimit, err := client.EstimateGas(ctx, msg)
	if err != nil {
		return clierr.Wrap(clierr.CodeActionSim, "estimate gas", err)
	}
	gasLimit = uint64(float64(gasLimit) * opts.GasMultiplier)

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

	nonce, err := client.PendingNonceAt(ctx, txSigner.Address())
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
	signed, err := txSigner.SignTx(chainID, tx)
	if err != nil {
		return clierr.Wrap(clierr.CodeSigner, "sign transaction", err)
	}
	if err := client.SendTransaction(ctx, signed); err != nil {
		return clierr.Wrap(clierr.CodeUnavailable, "broadcast transaction", err)
	}
	step.Status = StepStatusSubmitted
	step.TxHash = signed.Hash().Hex()

	waitCtx, cancel := context.WithTimeout(ctx, opts.StepTimeout)
	defer cancel()
	ticker := time.NewTicker(opts.PollInterval)
	defer ticker.Stop()
	for {
		receipt, err := client.TransactionReceipt(waitCtx, signed.Hash())
		if err == nil && receipt != nil {
			if receipt.Status == types.ReceiptStatusSuccessful {
				step.Status = StepStatusConfirmed
				return nil
			}
			return clierr.New(clierr.CodeUnavailable, "transaction reverted on-chain")
		}
		if waitCtx.Err() != nil {
			return clierr.Wrap(clierr.CodeActionTimeout, "timed out waiting for receipt", waitCtx.Err())
		}
		if err != nil && !errors.Is(err, ethereum.NotFound) {
			// Ignore transient RPC polling failures until timeout.
		}
		select {
		case <-waitCtx.Done():
			return clierr.Wrap(clierr.CodeActionTimeout, "timed out waiting for receipt", waitCtx.Err())
		case <-ticker.C:
		}
	}
}

func resolveTipCap(ctx context.Context, client *ethclient.Client, overrideGwei string) (*big.Int, error) {
	if strings.TrimSpace(overrideGwei) != "" {
		v, err := parseGwei(overrideGwei)
		if err != nil {
			return nil, clierr.Wrap(clierr.CodeUsage, "parse --max-priority-fee-gwei", err)
		}
		return v, nil
	}
	tipCap, err := client.SuggestGasTipCap(ctx)
	if err != nil {
		return big.NewInt(2_000_000_000), nil // 2 gwei fallback
	}
	return tipCap, nil
}

func resolveFeeCap(baseFee, tipCap *big.Int, overrideGwei string) (*big.Int, error) {
	if strings.TrimSpace(overrideGwei) != "" {
		v, err := parseGwei(overrideGwei)
		if err != nil {
			return nil, clierr.Wrap(clierr.CodeUsage, "parse --max-fee-gwei", err)
		}
		if v.Cmp(tipCap) < 0 {
			return nil, clierr.New(clierr.CodeUsage, "--max-fee-gwei must be >= --max-priority-fee-gwei")
		}
		return v, nil
	}
	feeCap := new(big.Int).Mul(baseFee, big.NewInt(2))
	feeCap.Add(feeCap, tipCap)
	return feeCap, nil
}

func parseGwei(v string) (*big.Int, error) {
	clean := strings.TrimSpace(v)
	if clean == "" {
		return nil, fmt.Errorf("empty gwei value")
	}
	rat, ok := new(big.Rat).SetString(clean)
	if !ok {
		return nil, fmt.Errorf("invalid numeric value %q", v)
	}
	if rat.Sign() < 0 {
		return nil, fmt.Errorf("value must be non-negative")
	}
	scale := big.NewRat(1_000_000_000, 1)
	rat.Mul(rat, scale)
	out := new(big.Int)
	if !rat.IsInt() {
		return nil, fmt.Errorf("value must resolve to an integer wei amount")
	}
	out = new(big.Int).Set(rat.Num())
	return out, nil
}

func markStepFailed(action *Action, step *ActionStep, msg string) {
	step.Status = StepStatusFailed
	step.Error = msg
	action.Status = ActionStatusFailed
	action.Touch()
}

func decodeHex(v string) ([]byte, error) {
	clean := strings.TrimSpace(v)
	clean = strings.TrimPrefix(clean, "0x")
	if clean == "" {
		return []byte{}, nil
	}
	if len(clean)%2 != 0 {
		clean = "0" + clean
	}
	buf, err := hex.DecodeString(clean)
	if err != nil {
		return nil, fmt.Errorf("invalid hex: %w", err)
	}
	return buf, nil
}
