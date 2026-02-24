package planner

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/big"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/ggonzalez94/defi-cli/internal/id"
)

type plannerRPCRequest struct {
	JSONRPC string            `json:"jsonrpc"`
	ID      json.RawMessage   `json:"id"`
	Method  string            `json:"method"`
	Params  []json.RawMessage `json:"params"`
}

func TestBuildAaveLendActionSupply(t *testing.T) {
	rpc := newPlannerRPCServer(t, big.NewInt(0))
	defer rpc.Close()

	chain, err := id.ParseChain("ethereum")
	if err != nil {
		t.Fatalf("parse chain: %v", err)
	}
	asset, err := id.ParseAsset("USDC", chain)
	if err != nil {
		t.Fatalf("parse asset: %v", err)
	}
	action, err := BuildAaveLendAction(context.Background(), AaveLendRequest{
		Verb:            AaveVerbSupply,
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000000",
		Sender:          "0x00000000000000000000000000000000000000AA",
		Recipient:       "0x00000000000000000000000000000000000000BB",
		Simulate:        true,
		RPCURL:          rpc.URL,
		PoolAddress:     "0x00000000000000000000000000000000000000CC",
	})
	if err != nil {
		t.Fatalf("BuildAaveLendAction failed: %v", err)
	}
	if action.IntentType != "lend_supply" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
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
	if !strings.EqualFold(action.Steps[1].Target, "0x00000000000000000000000000000000000000CC") {
		t.Fatalf("unexpected lend target: %s", action.Steps[1].Target)
	}
}

func TestBuildAaveRewardsCompoundAction(t *testing.T) {
	rpc := newPlannerRPCServer(t, big.NewInt(0))
	defer rpc.Close()

	chain, err := id.ParseChain("ethereum")
	if err != nil {
		t.Fatalf("parse chain: %v", err)
	}
	action, err := BuildAaveRewardsCompoundAction(context.Background(), AaveRewardsCompoundRequest{
		Chain:             chain,
		Sender:            "0x00000000000000000000000000000000000000AA",
		Recipient:         "0x00000000000000000000000000000000000000AA",
		Assets:            []string{"0x00000000000000000000000000000000000000D1"},
		RewardToken:       "0x00000000000000000000000000000000000000D2",
		AmountBaseUnits:   "1000",
		Simulate:          true,
		RPCURL:            rpc.URL,
		ControllerAddress: "0x00000000000000000000000000000000000000D3",
		PoolAddress:       "0x00000000000000000000000000000000000000D4",
	})
	if err != nil {
		t.Fatalf("BuildAaveRewardsCompoundAction failed: %v", err)
	}
	if action.IntentType != "compound_rewards" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
	}
	if len(action.Steps) != 3 {
		t.Fatalf("expected claim + approval + supply steps, got %d", len(action.Steps))
	}
	if action.Steps[0].Type != "claim" {
		t.Fatalf("expected first step claim, got %s", action.Steps[0].Type)
	}
	if action.Steps[1].Type != "approval" {
		t.Fatalf("expected second step approval, got %s", action.Steps[1].Type)
	}
	if action.Steps[2].Type != "lend_call" {
		t.Fatalf("expected third step lend_call, got %s", action.Steps[2].Type)
	}
}

func TestBuildAaveLendActionRequiresSender(t *testing.T) {
	chain, _ := id.ParseChain("ethereum")
	asset, _ := id.ParseAsset("USDC", chain)
	_, err := BuildAaveLendAction(context.Background(), AaveLendRequest{
		Verb:            AaveVerbSupply,
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000000",
	})
	if err == nil {
		t.Fatal("expected missing sender error")
	}
}

func newPlannerRPCServer(t *testing.T, allowance *big.Int) *httptest.Server {
	t.Helper()

	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var req plannerRPCRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		switch req.Method {
		case "eth_call":
			encoded, err := plannerERC20ABI.Methods["allowance"].Outputs.Pack(allowance)
			if err != nil {
				t.Fatalf("pack allowance response: %v", err)
			}
			writePlannerRPCResult(w, req.ID, "0x"+hex.EncodeToString(encoded))
		default:
			writePlannerRPCError(w, req.ID, -32601, fmt.Sprintf("method not supported in test: %s", req.Method))
		}
	}))
}

func writePlannerRPCResult(w http.ResponseWriter, id json.RawMessage, result any) {
	w.Header().Set("Content-Type", "application/json")
	_, _ = fmt.Fprintf(w, `{"jsonrpc":"2.0","id":%s,"result":%q}`, rawPlannerID(id), result)
}

func writePlannerRPCError(w http.ResponseWriter, id json.RawMessage, code int, message string) {
	w.Header().Set("Content-Type", "application/json")
	_, _ = fmt.Fprintf(w, `{"jsonrpc":"2.0","id":%s,"error":{"code":%d,"message":%q}}`, rawPlannerID(id), code, message)
}

func rawPlannerID(id json.RawMessage) string {
	if len(id) == 0 {
		return "1"
	}
	return string(id)
}
