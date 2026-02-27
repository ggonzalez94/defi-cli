package taikoswap

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/big"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

type rpcRequest struct {
	JSONRPC string            `json:"jsonrpc"`
	ID      json.RawMessage   `json:"id"`
	Method  string            `json:"method"`
	Params  []json.RawMessage `json:"params"`
}

func TestQuoteSwapChoosesBestFeeRoute(t *testing.T) {
	server := newMockRPCServer(t, false)
	defer server.Close()

	c := New()
	chain, _ := id.ParseChain("taiko")
	fromAsset, _ := id.ParseAsset("USDC", chain)
	toAsset, _ := id.ParseAsset("WETH", chain)
	quote, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain: chain, FromAsset: fromAsset, ToAsset: toAsset, AmountBaseUnits: "1000000", AmountDecimal: "1", RPCURL: server.URL,
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}
	if quote.Provider != "taikoswap" {
		t.Fatalf("unexpected provider: %s", quote.Provider)
	}
	if !strings.Contains(quote.Route, "fee-500") {
		t.Fatalf("expected best fee tier 500 in route, got %s", quote.Route)
	}
	if quote.EstimatedOut.AmountBaseUnits != "2000" {
		t.Fatalf("expected estimated out 2000, got %s", quote.EstimatedOut.AmountBaseUnits)
	}
}

func TestBuildSwapActionAddsApprovalWhenNeeded(t *testing.T) {
	server := newMockRPCServer(t, true)
	defer server.Close()

	c := New()
	chain, _ := id.ParseChain("taiko")
	fromAsset, _ := id.ParseAsset("USDC", chain)
	toAsset, _ := id.ParseAsset("WETH", chain)
	action, err := c.BuildSwapAction(context.Background(), providers.SwapQuoteRequest{
		Chain: chain, FromAsset: fromAsset, ToAsset: toAsset, AmountBaseUnits: "1000000", AmountDecimal: "1",
	}, providers.SwapExecutionOptions{
		Sender:      "0x00000000000000000000000000000000000000AA",
		Recipient:   "0x00000000000000000000000000000000000000BB",
		SlippageBps: 100,
		Simulate:    true,
		RPCURL:      server.URL,
	})
	if err != nil {
		t.Fatalf("BuildSwapAction failed: %v", err)
	}
	if action.IntentType != "swap" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
	}
	if len(action.Steps) != 2 {
		t.Fatalf("expected approval + swap steps, got %d", len(action.Steps))
	}
	if action.Steps[0].Type != "approval" {
		t.Fatalf("expected first step approval, got %s", action.Steps[0].Type)
	}
	if action.Steps[1].Type != "swap" {
		t.Fatalf("expected second step swap, got %s", action.Steps[1].Type)
	}
}

func TestBuildSwapActionRequiresSender(t *testing.T) {
	c := New()
	chain, _ := id.ParseChain("taiko")
	fromAsset, _ := id.ParseAsset("USDC", chain)
	toAsset, _ := id.ParseAsset("WETH", chain)
	_, err := c.BuildSwapAction(context.Background(), providers.SwapQuoteRequest{
		Chain: chain, FromAsset: fromAsset, ToAsset: toAsset, AmountBaseUnits: "1000000", AmountDecimal: "1",
	}, providers.SwapExecutionOptions{})
	if err == nil {
		t.Fatal("expected missing sender error")
	}
}

func TestBuildSwapActionRejectsInvalidSender(t *testing.T) {
	c := New()
	chain, _ := id.ParseChain("taiko")
	fromAsset, _ := id.ParseAsset("USDC", chain)
	toAsset, _ := id.ParseAsset("WETH", chain)
	_, err := c.BuildSwapAction(context.Background(), providers.SwapQuoteRequest{
		Chain: chain, FromAsset: fromAsset, ToAsset: toAsset, AmountBaseUnits: "1000000", AmountDecimal: "1",
	}, providers.SwapExecutionOptions{Sender: "not-an-address"})
	if err == nil {
		t.Fatal("expected invalid sender error")
	}
}

func TestBuildSwapActionRejectsInvalidRecipient(t *testing.T) {
	c := New()
	chain, _ := id.ParseChain("taiko")
	fromAsset, _ := id.ParseAsset("USDC", chain)
	toAsset, _ := id.ParseAsset("WETH", chain)
	_, err := c.BuildSwapAction(context.Background(), providers.SwapQuoteRequest{
		Chain: chain, FromAsset: fromAsset, ToAsset: toAsset, AmountBaseUnits: "1000000", AmountDecimal: "1",
	}, providers.SwapExecutionOptions{
		Sender:    "0x00000000000000000000000000000000000000AA",
		Recipient: "not-an-address",
	})
	if err == nil {
		t.Fatal("expected invalid recipient error")
	}
}

func TestBuildSwapActionUsesRPCOverride(t *testing.T) {
	server := newMockRPCServer(t, true)
	defer server.Close()

	c := New()
	chain, _ := id.ParseChain("taiko")
	fromAsset, _ := id.ParseAsset("USDC", chain)
	toAsset, _ := id.ParseAsset("WETH", chain)
	action, err := c.BuildSwapAction(context.Background(), providers.SwapQuoteRequest{
		Chain: chain, FromAsset: fromAsset, ToAsset: toAsset, AmountBaseUnits: "1000000", AmountDecimal: "1",
	}, providers.SwapExecutionOptions{
		Sender:      "0x00000000000000000000000000000000000000AA",
		SlippageBps: 100,
		Simulate:    true,
		RPCURL:      server.URL,
	})
	if err != nil {
		t.Fatalf("BuildSwapAction failed with rpc override: %v", err)
	}
	if len(action.Steps) == 0 {
		t.Fatal("expected non-empty steps")
	}
	for i := range action.Steps {
		if action.Steps[i].RPCURL != server.URL {
			t.Fatalf("expected step %d rpc override %q, got %q", i, server.URL, action.Steps[i].RPCURL)
		}
	}
}

func newMockRPCServer(t *testing.T, includeAllowance bool) *httptest.Server {
	t.Helper()

	var mu sync.Mutex
	callCount := 0

	handler := func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var req rpcRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		switch req.Method {
		case "eth_call":
			mu.Lock()
			callCount++
			index := callCount
			mu.Unlock()

			if includeAllowance && index == 5 {
				allowancePayload, err := erc20ABI.Methods["allowance"].Outputs.Pack(big.NewInt(0))
				if err != nil {
					t.Fatalf("pack allowance output: %v", err)
				}
				writeRPCResult(w, req.ID, "0x"+hex.EncodeToString(allowancePayload))
				return
			}

			amountOut := big.NewInt(0)
			switch index {
			case 1:
				amountOut = big.NewInt(1000)
			case 2:
				amountOut = big.NewInt(2000)
			case 3:
				amountOut = big.NewInt(1500)
			default:
				amountOut = big.NewInt(500)
			}
			out, err := quoterABI.Methods["quoteExactInputSingle"].Outputs.Pack(
				amountOut,
				big.NewInt(0), // sqrtPriceX96After
				uint32(0),     // initializedTicksCrossed
				big.NewInt(70_000),
			)
			if err != nil {
				t.Fatalf("pack quote output: %v", err)
			}
			writeRPCResult(w, req.ID, "0x"+hex.EncodeToString(out))
		default:
			writeRPCError(w, req.ID, -32601, fmt.Sprintf("method not supported in test: %s", req.Method))
		}
	}

	return httptest.NewServer(http.HandlerFunc(handler))
}

func writeRPCResult(w http.ResponseWriter, id json.RawMessage, result any) {
	w.Header().Set("Content-Type", "application/json")
	_, _ = fmt.Fprintf(w, `{"jsonrpc":"2.0","id":%s,"result":%q}`, rawIDOrDefault(id), result)
}

func writeRPCError(w http.ResponseWriter, id json.RawMessage, code int, message string) {
	w.Header().Set("Content-Type", "application/json")
	_, _ = fmt.Fprintf(w, `{"jsonrpc":"2.0","id":%s,"error":{"code":%d,"message":%q}}`, rawIDOrDefault(id), code, message)
}

func rawIDOrDefault(id json.RawMessage) string {
	if len(id) == 0 {
		return "1"
	}
	return string(id)
}
