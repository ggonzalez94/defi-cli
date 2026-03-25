package execution

import (
	"context"
	"math/big"
	"strings"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution/signer"
)

// EVMSubmitBackend owns the final signing+broadcast step for standard EVM
// transactions while EVMStepExecutor retains simulation, gas estimation,
// nonce management, receipt polling, and settlement checks.
type EVMSubmitBackend interface {
	EffectiveSender() common.Address
	SubmitDynamicFeeTx(ctx context.Context, rpcURL string, chainID *big.Int, tx *types.Transaction) (common.Hash, error)
}

func ResolveExecutionBackend(action *Action, txSigner signer.Signer, evmBackend EVMSubmitBackend) (StepExecutor, error) {
	if action == nil {
		return nil, clierr.New(clierr.CodeInternal, "missing action")
	}

	switch normalizeExecutionBackend(action.ExecutionBackend) {
	case ExecutionBackendTempo:
		if txSigner == nil {
			return nil, clierr.New(clierr.CodeSigner, "missing tempo signer")
		}
		return NewTempoStepExecutor(txSigner), nil
	case ExecutionBackendOWS:
		if evmBackend == nil {
			return nil, clierr.New(clierr.CodeSigner, "missing wallet-backed EVM submission backend")
		}
		return NewEVMStepExecutor(evmBackend), nil
	case ExecutionBackendLegacyLocal:
		if evmBackend == nil {
			if txSigner == nil {
				return nil, clierr.New(clierr.CodeSigner, "missing local signer")
			}
			evmBackend = NewLocalSubmitBackend(txSigner)
		}
		return NewEVMStepExecutor(evmBackend), nil
	default:
		return nil, clierr.New(clierr.CodeUnsupported, "unsupported execution backend")
	}
}

func normalizeExecutionBackend(backend ExecutionBackend) ExecutionBackend {
	switch ExecutionBackend(strings.ToLower(strings.TrimSpace(string(backend)))) {
	case "", ExecutionBackendLegacyLocal:
		return ExecutionBackendLegacyLocal
	case ExecutionBackendOWS:
		return ExecutionBackendOWS
	case ExecutionBackendTempo:
		return ExecutionBackendTempo
	default:
		return backend
	}
}
