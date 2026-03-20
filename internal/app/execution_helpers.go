package app

import (
	"context"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/execution"
	execsigner "github.com/ggonzalez94/defi-cli/internal/execution/signer"
)

const executionStepRPCOverhead = 15 * time.Second

func (s *runtimeState) executeActionWithTimeout(action *execution.Action, txSigner execsigner.Signer, opts execution.ExecuteOptions) error {
	timeout := estimateExecutionTimeout(action, opts)
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	return execution.ExecuteAction(ctx, s.actionStore, action, txSigner, opts)
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
