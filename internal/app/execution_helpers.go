package app

import (
	"context"

	"github.com/ggonzalez94/defi-cli/internal/execution"
	execsigner "github.com/ggonzalez94/defi-cli/internal/execution/signer"
)

func (s *runtimeState) executeActionWithTimeout(action *execution.Action, txSigner execsigner.Signer, opts execution.ExecuteOptions) error {
	ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
	defer cancel()
	return execution.ExecuteAction(ctx, s.actionStore, action, txSigner, opts)
}
