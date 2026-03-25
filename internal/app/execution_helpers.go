package app

import (
	"context"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum/common"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	execsigner "github.com/ggonzalez94/defi-cli/internal/execution/signer"
	"github.com/spf13/cobra"
)

const executionStepRPCOverhead = 15 * time.Second

type submitExecutionInputs struct {
	Signer      string
	KeySource   string
	PrivateKey  string
	FromAddress string
}

type resolvedSubmitExecution struct {
	txSigner   execsigner.Signer
	evmBackend execution.EVMSubmitBackend
	sender     string
}

func (s *runtimeState) executeActionWithTimeout(action *execution.Action, txSigner execsigner.Signer, evmBackend execution.EVMSubmitBackend, opts execution.ExecuteOptions) error {
	timeout := estimateExecutionTimeout(action, opts)
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	return execution.ExecuteAction(ctx, s.actionStore, action, txSigner, evmBackend, opts)
}

func resolveActionExecutionBackend(cmd *cobra.Command, action execution.Action, input submitExecutionInputs) (resolvedSubmitExecution, error) {
	switch strings.ToLower(strings.TrimSpace(string(action.ExecutionBackend))) {
	case "", string(execution.ExecutionBackendLegacyLocal):
		txSigner, err := newExecutionSigner(input.Signer, input.KeySource, input.PrivateKey)
		if err != nil {
			return resolvedSubmitExecution{}, err
		}
		sender := effectiveSenderAddress(txSigner)
		return resolvedSubmitExecution{
			txSigner:   txSigner,
			evmBackend: execution.NewLocalSubmitBackend(txSigner),
			sender:     sender,
		}, nil
	case string(execution.ExecutionBackendOWS):
		if strings.TrimSpace(action.WalletID) == "" {
			return resolvedSubmitExecution{}, clierr.New(clierr.CodeUsage, "wallet-backed action is missing persisted wallet_id")
		}
		if usesLegacySignerFlags(cmd) {
			return resolvedSubmitExecution{}, clierr.New(clierr.CodeUsage, "wallet-backed actions do not accept legacy signer flags (--signer, --key-source, --private-key)")
		}
		if !common.IsHexAddress(action.FromAddress) {
			return resolvedSubmitExecution{}, clierr.New(clierr.CodeUsage, "wallet-backed action is missing a valid planned sender address")
		}
		sender := common.HexToAddress(action.FromAddress)
		return resolvedSubmitExecution{
			evmBackend: execution.NewOWSSubmitBackend(action.WalletID, sender),
			sender:     sender.Hex(),
		}, nil
	case string(execution.ExecutionBackendTempo):
		txSigner, err := newExecutionSigner("tempo", input.KeySource, input.PrivateKey)
		if err != nil {
			return resolvedSubmitExecution{}, err
		}
		return resolvedSubmitExecution{
			txSigner: txSigner,
			sender:   effectiveSenderAddress(txSigner),
		}, nil
	default:
		return resolvedSubmitExecution{}, clierr.New(clierr.CodeUnsupported, "unsupported execution backend for submit")
	}
}

func usesLegacySignerFlags(cmd *cobra.Command) bool {
	if cmd == nil {
		return false
	}
	for _, name := range []string{"signer", "key-source", "private-key"} {
		flag := cmd.Flags().Lookup(name)
		if flag != nil && flag.Changed {
			return true
		}
	}
	return false
}

func validateExecutionSender(action execution.Action, expectedSender, actualSender string) error {
	if strings.TrimSpace(expectedSender) != "" && !strings.EqualFold(strings.TrimSpace(expectedSender), actualSender) {
		return clierr.New(clierr.CodeSigner, "signer address does not match --from-address")
	}
	if strings.TrimSpace(action.FromAddress) != "" && !strings.EqualFold(strings.TrimSpace(action.FromAddress), actualSender) {
		return clierr.New(clierr.CodeSigner, "signer address does not match planned action sender")
	}
	return nil
}

// Execution timeout is derived from remaining action wait stages so short provider
// request timeouts do not cancel transaction confirmation/settlement polling early.
func estimateExecutionTimeout(action *execution.Action, opts execution.ExecuteOptions) time.Duration {
	stepTimeout := opts.StepTimeout
	if stepTimeout <= 0 {
		stepTimeout = execution.DefaultExecuteOptions().StepTimeout
	}
	stages := 0
	steps := 0
	if action != nil {
		for _, step := range action.Steps {
			if step.Status == execution.StepStatusConfirmed {
				continue
			}
			steps++
			stages++
			if step.Type == execution.StepTypeBridge {
				// Bridge steps wait for source receipt and destination settlement.
				stages++
			}
		}
	}
	if stages <= 0 {
		stages = 1
	}
	if steps <= 0 {
		steps = 1
	}
	// Add per-step RPC headroom for chain-id/simulation/gas/fee/nonce/broadcast work
	// so long-running receipt/settlement waits are less likely to be cut off early.
	return time.Duration(stages)*stepTimeout + time.Duration(steps)*executionStepRPCOverhead
}
