package planner

import (
	"context"
	"math/big"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/ggonzalez94/defi-cli/internal/id"
)

func TestBuildMorphoVaultYieldActionDeposit(t *testing.T) {
	rpc := newPlannerRPCServer(t, big.NewInt(0))
	defer rpc.Close()
	graphql := newMorphoVaultGraphQLServer(t)
	defer graphql.Close()

	prev := morphoGraphQLEndpoint
	morphoGraphQLEndpoint = graphql.URL
	t.Cleanup(func() { morphoGraphQLEndpoint = prev })

	chain, err := id.ParseChain("ethereum")
	if err != nil {
		t.Fatalf("parse chain: %v", err)
	}
	asset, err := id.ParseAsset("USDC", chain)
	if err != nil {
		t.Fatalf("parse asset: %v", err)
	}

	action, err := BuildMorphoVaultYieldAction(context.Background(), MorphoVaultYieldRequest{
		Verb:            MorphoVaultYieldVerbDeposit,
		Chain:           chain,
		Asset:           asset,
		VaultAddress:    "0x1111111111111111111111111111111111111111",
		AmountBaseUnits: "1000000",
		Sender:          "0x00000000000000000000000000000000000000AA",
		Recipient:       "0x00000000000000000000000000000000000000BB",
		Simulate:        true,
		RPCURL:          rpc.URL,
	})
	if err != nil {
		t.Fatalf("BuildMorphoVaultYieldAction failed: %v", err)
	}
	if action.IntentType != "yield_deposit" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
	}
	if action.Provider != "morpho" {
		t.Fatalf("unexpected provider: %s", action.Provider)
	}
	if len(action.Steps) != 2 {
		t.Fatalf("expected approval + deposit steps, got %d", len(action.Steps))
	}
	if action.Steps[0].Type != "approval" {
		t.Fatalf("expected first step approval, got %s", action.Steps[0].Type)
	}
	if action.Steps[1].Type != "lend_call" {
		t.Fatalf("expected second step lend_call, got %s", action.Steps[1].Type)
	}
	if !strings.EqualFold(action.Steps[1].Target, "0x1111111111111111111111111111111111111111") {
		t.Fatalf("unexpected vault target: %s", action.Steps[1].Target)
	}
	if got, _ := action.Metadata["vault_kind"].(string); got != "vault" {
		t.Fatalf("expected vault kind metadata, got %+v", action.Metadata)
	}
}

func TestBuildMorphoVaultYieldActionWithdraw(t *testing.T) {
	rpc := newPlannerRPCServer(t, big.NewInt(0))
	defer rpc.Close()
	graphql := newMorphoVaultGraphQLServer(t)
	defer graphql.Close()

	prev := morphoGraphQLEndpoint
	morphoGraphQLEndpoint = graphql.URL
	t.Cleanup(func() { morphoGraphQLEndpoint = prev })

	chain, err := id.ParseChain("ethereum")
	if err != nil {
		t.Fatalf("parse chain: %v", err)
	}
	asset, err := id.ParseAsset("USDC", chain)
	if err != nil {
		t.Fatalf("parse asset: %v", err)
	}

	action, err := BuildMorphoVaultYieldAction(context.Background(), MorphoVaultYieldRequest{
		Verb:            MorphoVaultYieldVerbWithdraw,
		Chain:           chain,
		Asset:           asset,
		VaultAddress:    "0x1111111111111111111111111111111111111111",
		AmountBaseUnits: "1000000",
		Sender:          "0x00000000000000000000000000000000000000AA",
		Recipient:       "0x00000000000000000000000000000000000000BB",
		OnBehalfOf:      "0x00000000000000000000000000000000000000AA",
		Simulate:        true,
		RPCURL:          rpc.URL,
	})
	if err != nil {
		t.Fatalf("BuildMorphoVaultYieldAction failed: %v", err)
	}
	if action.IntentType != "yield_withdraw" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
	}
	if len(action.Steps) != 1 {
		t.Fatalf("expected one withdraw step, got %d", len(action.Steps))
	}
	if action.Steps[0].Type != "lend_call" {
		t.Fatalf("expected lend_call step, got %s", action.Steps[0].Type)
	}
}

func TestBuildMorphoVaultYieldActionRequiresVaultAddress(t *testing.T) {
	chain, _ := id.ParseChain("ethereum")
	asset, _ := id.ParseAsset("USDC", chain)
	_, err := BuildMorphoVaultYieldAction(context.Background(), MorphoVaultYieldRequest{
		Verb:            MorphoVaultYieldVerbDeposit,
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000000",
		Sender:          "0x00000000000000000000000000000000000000AA",
	})
	if err == nil {
		t.Fatal("expected missing vault address error")
	}
}

func newMorphoVaultGraphQLServer(t *testing.T) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{
			"data": {
				"vaultByAddress": {
					"address": "0x1111111111111111111111111111111111111111",
					"listed": true,
					"asset": {
						"address": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
						"symbol": "USDC",
						"decimals": 6,
						"chain": {"id": 1}
					}
				}
			}
		}`))
	}))
}
