package app

import (
	"strings"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/ows"
)

func TestEstimateExecutionTimeout_DefaultStepTimeout(t *testing.T) {
	got := estimateExecutionTimeout(nil, execution.ExecuteOptions{})
	want := 2*time.Minute + executionStepRPCOverhead
	if got != want {
		t.Fatalf("expected default timeout %s, got %s", want, got)
	}
}

func TestEstimateExecutionTimeout_CountsRemainingStages(t *testing.T) {
	action := &execution.Action{
		Steps: []execution.ActionStep{
			{Type: execution.StepTypeApproval, Status: execution.StepStatusPending},
			{Type: execution.StepTypeBridge, Status: execution.StepStatusPending},
			{Type: execution.StepTypeSwap, Status: execution.StepStatusConfirmed},
		},
	}
	got := estimateExecutionTimeout(action, execution.ExecuteOptions{StepTimeout: 45 * time.Second})
	// approval=1 stage, bridge=2 stages, confirmed swap=0 stages
	// plus per-step RPC overhead for approval + bridge send steps.
	want := 3*45*time.Second + 2*executionStepRPCOverhead
	if got != want {
		t.Fatalf("expected timeout %s, got %s", want, got)
	}
}

func TestResolvePersistedOWSSenderRejectsMismatch(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	writeOWSWalletFixture(t, home, ows.Wallet{
		ID:        "wallet-123",
		Name:      "Agent Wallet",
		CreatedAt: "2026-03-25T00:00:00Z",
		Accounts: []ows.WalletAccount{
			{
				AccountID:      "acc-1",
				Address:        "0x000000000000000000000000000000000000dead",
				ChainID:        "eip155:1",
				DerivationPath: "m/44'/60'/0'/0/0",
			},
		},
	})

	_, err := resolvePersistedOWSSender(execution.Action{
		ChainID:     "eip155:1",
		FromAddress: "0x00000000000000000000000000000000000000AA",
		WalletID:    "wallet-123",
	})
	if err == nil {
		t.Fatal("expected sender mismatch error")
	}
	if !strings.Contains(strings.ToLower(err.Error()), "wallet sender") {
		t.Fatalf("expected wallet sender mismatch error, got %v", err)
	}
}
