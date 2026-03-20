package planner

import (
	"context"
	"math/big"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/ggonzalez94/defi-cli/internal/id"
)

func TestBuildMorphoLendActionSupply(t *testing.T) {
	rpc := newPlannerRPCServer(t, big.NewInt(0))
	defer rpc.Close()
	morpho := newMorphoGraphQLServer(t)
	defer morpho.Close()

	prev := morphoGraphQLEndpoint
	morphoGraphQLEndpoint = morpho.URL
	t.Cleanup(func() { morphoGraphQLEndpoint = prev })

	chain, err := id.ParseChain("ethereum")
	if err != nil {
		t.Fatalf("parse chain: %v", err)
	}
	asset, err := id.ParseAsset("USDC", chain)
	if err != nil {
		t.Fatalf("parse asset: %v", err)
	}

	action, err := BuildMorphoLendAction(context.Background(), MorphoLendRequest{
		Verb:            AaveVerbSupply,
		Chain:           chain,
		Asset:           asset,
		MarketID:        "0x64d65c9a2d91c36d56fbc42d69e979335320169b3df63bf92789e2c8883fcc64",
		AmountBaseUnits: "1000000",
		Sender:          "0x00000000000000000000000000000000000000AA",
		Recipient:       "0x00000000000000000000000000000000000000BB",
		Simulate:        true,
		RPCURL:          rpc.URL,
	})
	if err != nil {
		t.Fatalf("BuildMorphoLendAction failed: %v", err)
	}
	if action.IntentType != "lend_supply" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
	}
	if action.Provider != "morpho" {
		t.Fatalf("unexpected provider: %s", action.Provider)
	}
	if len(action.Steps) != 2 {
		t.Fatalf("expected approval + lend steps, got %d", len(action.Steps))
	}
	if action.Steps[0].Type != "approval" {
		t.Fatalf("expected first step approval, got %s", action.Steps[0].Type)
	}
	if action.Steps[1].Type != "lend_call" {
		t.Fatalf("expected second step lend_call, got %s", action.Steps[1].Type)
	}
	if action.Steps[1].Target != "0xBBBBBbbBBb9cC5e90e3b3Af64bdAF62C37EEFFCb" {
		t.Fatalf("unexpected morpho target: %s", action.Steps[1].Target)
	}
}

func TestBuildMorphoLendActionRequiresMarketID(t *testing.T) {
	chain, _ := id.ParseChain("ethereum")
	asset, _ := id.ParseAsset("USDC", chain)
	_, err := BuildMorphoLendAction(context.Background(), MorphoLendRequest{
		Verb:            AaveVerbSupply,
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000000",
		Sender:          "0x00000000000000000000000000000000000000AA",
	})
	if err == nil {
		t.Fatal("expected missing market id error")
	}
}

func newMorphoGraphQLServer(t *testing.T) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{
			"data": {
				"markets": {
					"items": [{
						"uniqueKey": "0x64d65c9a2d91c36d56fbc42d69e979335320169b3df63bf92789e2c8883fcc64",
						"irmAddress": "0x870aC11D48B15DB9a138Cf899d20F13F79Ba00BC",
						"lltv": "860000000000000000",
						"morphoBlue": {"address":"0xBBBBBbbBBb9cC5e90e3b3Af64bdAF62C37EEFFCb"},
						"oracle": {"address":"0xA6D6950c9F177F1De7f7757FB33539e3Ec60182a"},
						"loanAsset": {"address":"0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48","symbol":"USDC","decimals":6,"chain":{"id":1}},
						"collateralAsset": {"address":"0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf","symbol":"cbBTC","decimals":8}
					}]
				}
			}
		}`))
	}))
}
