package across

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

func TestBaseUnitMathHelpers(t *testing.T) {
	if compareBaseUnits("100", "99") <= 0 {
		t.Fatal("compareBaseUnits expected 100 > 99")
	}
	if out := subtractBaseUnits("1000", "1"); out != "999" {
		t.Fatalf("unexpected subtraction result: %s", out)
	}
	if out := subtractBaseUnits("1", "2"); out != "0" {
		t.Fatalf("unexpected underflow result: %s", out)
	}
}

func TestBuildBridgeAction(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/swap/approval":
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{
				"approvalTxns": [{
					"chainId": 1,
					"to": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
					"data": "0x095ea7b3",
					"value": "0"
				}],
				"swapTx": {
					"chainId": 1,
					"to": "0x5c7BCd6E7De5423a257D81B442095A1a6ced35C5",
					"data": "0xad5425c6",
					"value": "0x0"
				},
				"minOutputAmount": "990000",
				"expectedOutputAmount": "995000",
				"expectedFillTime": 5
			}`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0))
	c.baseURL = srv.URL
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
	})
	if err != nil {
		t.Fatalf("BuildBridgeAction failed: %v", err)
	}
	if action.Provider != "across" {
		t.Fatalf("unexpected provider: %s", action.Provider)
	}
	if len(action.Steps) != 2 {
		t.Fatalf("expected approval + bridge steps, got %d", len(action.Steps))
	}
	if action.Steps[1].ExpectedOutputs["settlement_provider"] != "across" {
		t.Fatalf("expected across settlement provider, got %q", action.Steps[1].ExpectedOutputs["settlement_provider"])
	}
}
