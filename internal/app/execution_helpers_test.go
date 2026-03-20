package app

import (
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/execution"
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
