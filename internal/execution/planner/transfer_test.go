package planner

import (
	"testing"

	"github.com/ggonzalez94/defi-cli/internal/id"
)

func TestBuildTransferAction(t *testing.T) {
	chain, err := id.ParseChain("taiko")
	if err != nil {
		t.Fatalf("parse chain: %v", err)
	}
	asset, err := id.ParseAsset("USDC", chain)
	if err != nil {
		t.Fatalf("parse asset: %v", err)
	}
	action, err := BuildTransferAction(TransferRequest{
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000000",
		Sender:          "0x00000000000000000000000000000000000000AA",
		Recipient:       "0x00000000000000000000000000000000000000BB",
		Simulate:        true,
		RPCURL:          "http://127.0.0.1:8545",
	})
	if err != nil {
		t.Fatalf("BuildTransferAction failed: %v", err)
	}
	if action.IntentType != "transfer" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
	}
	if action.Provider != "native" {
		t.Fatalf("unexpected provider: %s", action.Provider)
	}
	if len(action.Steps) != 1 {
		t.Fatalf("expected one transfer step, got %d", len(action.Steps))
	}
	if action.Steps[0].Type != "transfer" {
		t.Fatalf("unexpected step type: %s", action.Steps[0].Type)
	}
}

func TestBuildTransferActionRejectsInvalidAmount(t *testing.T) {
	chain, _ := id.ParseChain("taiko")
	asset, _ := id.ParseAsset("USDC", chain)
	_, err := BuildTransferAction(TransferRequest{
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "0",
		Sender:          "0x00000000000000000000000000000000000000AA",
		Recipient:       "0x00000000000000000000000000000000000000BB",
	})
	if err == nil {
		t.Fatal("expected invalid amount error")
	}
}

func TestBuildTransferActionRejectsZeroRecipient(t *testing.T) {
	chain, _ := id.ParseChain("taiko")
	asset, _ := id.ParseAsset("USDC", chain)
	_, err := BuildTransferAction(TransferRequest{
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000",
		Sender:          "0x00000000000000000000000000000000000000AA",
		Recipient:       "0x0000000000000000000000000000000000000000",
	})
	if err == nil {
		t.Fatal("expected zero-recipient error")
	}
}
