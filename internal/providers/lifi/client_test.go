package lifi

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/big"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

type lifiRPCRequest struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id"`
	Method  string          `json:"method"`
}

func TestQuoteBridge(t *testing.T) {
	quoteServer := newLiFiQuoteServer(t, "0x0000000000000000000000000000000000000ABC")
	defer quoteServer.Close()

	c := New(httpx.New(2*time.Second, 0))
	c.baseURL = quoteServer.URL
	fromChain, _ := id.ParseChain("ethereum")
	toChain, _ := id.ParseChain("base")
	fromAsset, _ := id.ParseAsset("USDC", fromChain)
	toAsset, _ := id.ParseAsset("USDC", toChain)

	quote, err := c.QuoteBridge(context.Background(), providers.BridgeQuoteRequest{
		FromChain:       fromChain,
		ToChain:         toChain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteBridge failed: %v", err)
	}
	if quote.Provider != "lifi" {
		t.Fatalf("unexpected provider: %s", quote.Provider)
	}
	if quote.EstimatedOut.AmountBaseUnits != "950000" {
		t.Fatalf("unexpected estimated out: %s", quote.EstimatedOut.AmountBaseUnits)
	}
	if quote.EstimatedFeeUSD <= 0 {
		t.Fatalf("expected positive fee estimate, got %f", quote.EstimatedFeeUSD)
	}
}

func TestBuildBridgeActionAddsApprovalStep(t *testing.T) {
	quoteServer := newLiFiQuoteServer(t, "0x0000000000000000000000000000000000000ABC")
	defer quoteServer.Close()
	rpcServer := newLiFiRPCServer(t, big.NewInt(0))
	defer rpcServer.Close()

	c := New(httpx.New(2*time.Second, 0))
	c.baseURL = quoteServer.URL

	fromChain, _ := id.ParseChain("ethereum")
	toChain, _ := id.ParseChain("base")
	fromAsset, _ := id.ParseAsset("USDC", fromChain)
	toAsset, _ := id.ParseAsset("USDC", toChain)

	action, err := c.BuildBridgeAction(context.Background(), providers.BridgeQuoteRequest{
		FromChain:       fromChain,
		ToChain:         toChain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	}, providers.BridgeExecutionOptions{
		Sender:      "0x00000000000000000000000000000000000000AA",
		Recipient:   "0x00000000000000000000000000000000000000BB",
		SlippageBps: 50,
		Simulate:    true,
		RPCURL:      rpcServer.URL,
	})
	if err != nil {
		t.Fatalf("BuildBridgeAction failed: %v", err)
	}
	if action.IntentType != "bridge" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
	}
	if len(action.Steps) != 2 {
		t.Fatalf("expected approval + bridge steps, got %d", len(action.Steps))
	}
	if action.Steps[0].Type != "approval" {
		t.Fatalf("expected first step approval, got %s", action.Steps[0].Type)
	}
	if action.Steps[1].Type != "bridge_send" {
		t.Fatalf("expected second step bridge_send, got %s", action.Steps[1].Type)
	}
}

func TestBuildBridgeActionSkipsApprovalWhenSpenderMissing(t *testing.T) {
	quoteServer := newLiFiQuoteServer(t, "")
	defer quoteServer.Close()

	c := New(httpx.New(2*time.Second, 0))
	c.baseURL = quoteServer.URL

	fromChain, _ := id.ParseChain("ethereum")
	toChain, _ := id.ParseChain("base")
	fromAsset, _ := id.ParseAsset("USDC", fromChain)
	toAsset, _ := id.ParseAsset("USDC", toChain)

	action, err := c.BuildBridgeAction(context.Background(), providers.BridgeQuoteRequest{
		FromChain:       fromChain,
		ToChain:         toChain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	}, providers.BridgeExecutionOptions{
		Sender:    "0x00000000000000000000000000000000000000AA",
		Simulate:  true,
		RPCURL:    "http://127.0.0.1:1",
		Recipient: "0x00000000000000000000000000000000000000AA",
	})
	if err != nil {
		t.Fatalf("BuildBridgeAction failed: %v", err)
	}
	if len(action.Steps) != 1 {
		t.Fatalf("expected bridge-only step, got %d", len(action.Steps))
	}
	if action.Steps[0].Type != "bridge_send" {
		t.Fatalf("expected bridge_send step, got %s", action.Steps[0].Type)
	}
}

func newLiFiQuoteServer(t *testing.T, approvalAddress string) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = fmt.Fprintf(w, `{
			"estimate": {
				"toAmount": "950000",
				"toAmountMin": "940000",
				"approvalAddress": %q,
				"feeCosts": [{"amountUSD":"0.40"}],
				"gasCosts": [{"amountUSD":"0.60"}],
				"executionDuration": 120
			},
			"toolDetails": {"name":"across"},
			"tool": "across",
			"transactionRequest": {
				"to": "0x0000000000000000000000000000000000000DDD",
				"from": "0x00000000000000000000000000000000000000AA",
				"data": "0x1234",
				"value": "0x0",
				"chainId": 1
			}
		}`, approvalAddress)
	}))
}

func newLiFiRPCServer(t *testing.T, allowance *big.Int) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var req lifiRPCRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		switch req.Method {
		case "eth_call":
			encoded, err := lifiERC20ABI.Methods["allowance"].Outputs.Pack(allowance)
			if err != nil {
				t.Fatalf("pack allowance response: %v", err)
			}
			writeLiFiRPCResult(w, req.ID, "0x"+hex.EncodeToString(encoded))
		default:
			writeLiFiRPCError(w, req.ID, -32601, fmt.Sprintf("method not supported in test: %s", req.Method))
		}
	}))
}

func writeLiFiRPCResult(w http.ResponseWriter, id json.RawMessage, result any) {
	w.Header().Set("Content-Type", "application/json")
	_, _ = fmt.Fprintf(w, `{"jsonrpc":"2.0","id":%s,"result":%q}`, rawLiFiID(id), result)
}

func writeLiFiRPCError(w http.ResponseWriter, id json.RawMessage, code int, message string) {
	w.Header().Set("Content-Type", "application/json")
	_, _ = fmt.Fprintf(w, `{"jsonrpc":"2.0","id":%s,"error":{"code":%d,"message":%q}}`, rawLiFiID(id), code, message)
}

func rawLiFiID(id json.RawMessage) string {
	if len(id) == 0 {
		return "1"
	}
	return string(id)
}
