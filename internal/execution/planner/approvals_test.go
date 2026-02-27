package planner

import (
	"testing"

	"github.com/ggonzalez94/defi-cli/internal/id"
)

func TestBuildApprovalAction(t *testing.T) {
	chain, err := id.ParseChain("taiko")
	if err != nil {
		t.Fatalf("parse chain: %v", err)
	}
	asset, err := id.ParseAsset("USDC", chain)
	if err != nil {
		t.Fatalf("parse asset: %v", err)
	}
	action, err := BuildApprovalAction(ApprovalRequest{
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000000",
		Sender:          "0x00000000000000000000000000000000000000AA",
		Spender:         "0x00000000000000000000000000000000000000BB",
		Simulate:        true,
		RPCURL:          "http://127.0.0.1:8545",
	})
	if err != nil {
		t.Fatalf("BuildApprovalAction failed: %v", err)
	}
	if action.IntentType != "approve" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
	}
	if action.Provider != "native" {
		t.Fatalf("unexpected provider: %s", action.Provider)
	}
	if len(action.Steps) != 1 {
		t.Fatalf("expected one approval step, got %d", len(action.Steps))
	}
	if action.Steps[0].Type != "approval" {
		t.Fatalf("unexpected step type: %s", action.Steps[0].Type)
	}
}

func TestBuildApprovalActionRejectsInvalidAmount(t *testing.T) {
	chain, _ := id.ParseChain("taiko")
	asset, _ := id.ParseAsset("USDC", chain)
	_, err := BuildApprovalAction(ApprovalRequest{
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "0",
		Sender:          "0x00000000000000000000000000000000000000AA",
		Spender:         "0x00000000000000000000000000000000000000BB",
	})
	if err == nil {
		t.Fatal("expected invalid amount error")
	}
}
