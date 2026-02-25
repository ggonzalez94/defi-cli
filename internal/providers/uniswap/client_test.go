package uniswap

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

func TestQuoteSwapIncludesRequiredSwapper(t *testing.T) {
	chain, _ := id.ParseChain("ethereum")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("DAI", chain)

	type quoteReq struct {
		TokenInChainID  int64    `json:"tokenInChainId"`
		TokenOutChainID int64    `json:"tokenOutChainId"`
		TokenIn         string   `json:"tokenIn"`
		TokenOut        string   `json:"tokenOut"`
		Amount          string   `json:"amount"`
		Type            string   `json:"type"`
		Swapper         string   `json:"swapper"`
		AutoSlippage    string   `json:"autoSlippage"`
		SlippageTol     *float64 `json:"slippageTolerance"`
	}
	var got quoteReq

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "expected POST", http.StatusMethodNotAllowed)
			return
		}
		if r.URL.Path != "/v1/quote" {
			http.Error(w, "unexpected path", http.StatusNotFound)
			return
		}
		if r.Header.Get("x-api-key") != "test-key" {
			http.Error(w, "missing API key header", http.StatusUnauthorized)
			return
		}
		if err := json.NewDecoder(r.Body).Decode(&got); err != nil {
			http.Error(w, "invalid JSON payload", http.StatusBadRequest)
			return
		}

		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"quote":{"output":{"amount":"999847836538317147"},"gasFeeUSD":"0.1589"}}`)
	}))
	defer srv.Close()

	fixedNow := time.Date(2026, time.February, 25, 17, 30, 0, 0, time.UTC)
	c := New(httpx.New(1*time.Second, 0), "test-key")
	c.baseURL = srv.URL
	c.now = func() time.Time { return fixedNow }

	quote, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetIn,
		ToAsset:         assetOut,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}

	if got.TokenInChainID != 1 || got.TokenOutChainID != 1 {
		t.Fatalf("unexpected chain ids in payload: %+v", got)
	}
	if got.TokenIn != assetIn.Address || got.TokenOut != assetOut.Address {
		t.Fatalf("unexpected token addresses in payload: %+v", got)
	}
	if got.Amount != "1000000" {
		t.Fatalf("unexpected amount in payload: %s", got.Amount)
	}
	if got.Type != "EXACT_INPUT" {
		t.Fatalf("unexpected swap type in payload: %s", got.Type)
	}
	if got.Swapper != quoteOnlySwapper {
		t.Fatalf("expected swapper=%s, got %s", quoteOnlySwapper, got.Swapper)
	}
	if got.AutoSlippage != "DEFAULT" {
		t.Fatalf("expected autoSlippage=DEFAULT, got %s", got.AutoSlippage)
	}
	if got.SlippageTol != nil {
		t.Fatalf("expected slippageTolerance to be omitted, got %v", *got.SlippageTol)
	}

	if quote.Provider != "uniswap" {
		t.Fatalf("expected provider uniswap, got %s", quote.Provider)
	}
	if quote.TradeType != "exact-input" {
		t.Fatalf("expected trade_type exact-input, got %s", quote.TradeType)
	}
	if quote.EstimatedOut.AmountBaseUnits != "999847836538317147" {
		t.Fatalf("unexpected output amount: %s", quote.EstimatedOut.AmountBaseUnits)
	}
	if quote.EstimatedGasUSD != 0.1589 {
		t.Fatalf("unexpected gas USD: %v", quote.EstimatedGasUSD)
	}
	if quote.FetchedAt != fixedNow.Format(time.RFC3339) {
		t.Fatalf("unexpected fetched_at: %s", quote.FetchedAt)
	}
}

func TestQuoteSwapUsesManualSlippageOverride(t *testing.T) {
	chain, _ := id.ParseChain("ethereum")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("DAI", chain)

	type quoteReq struct {
		AutoSlippage string   `json:"autoSlippage"`
		SlippageTol  *float64 `json:"slippageTolerance"`
	}
	var got quoteReq

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if err := json.NewDecoder(r.Body).Decode(&got); err != nil {
			http.Error(w, "invalid JSON payload", http.StatusBadRequest)
			return
		}

		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"quote":{"output":{"amount":"1000000000000000000"},"gasFeeUSD":"0.1"}}`)
	}))
	defer srv.Close()

	slippage := 1.25
	c := New(httpx.New(1*time.Second, 0), "test-key")
	c.baseURL = srv.URL

	quote, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetIn,
		ToAsset:         assetOut,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
		SlippagePct:     &slippage,
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}

	if got.AutoSlippage != "" {
		t.Fatalf("expected autoSlippage to be omitted, got %s", got.AutoSlippage)
	}
	if got.SlippageTol == nil {
		t.Fatal("expected slippageTolerance to be set")
	}
	if *got.SlippageTol != slippage {
		t.Fatalf("expected slippageTolerance=%v, got %v", slippage, *got.SlippageTol)
	}
	if quote.EstimatedGasUSD != 0.1 {
		t.Fatalf("unexpected gas USD: %v", quote.EstimatedGasUSD)
	}
	if quote.TradeType != "exact-input" {
		t.Fatalf("expected trade_type exact-input, got %s", quote.TradeType)
	}
}

func TestQuoteSwapSupportsExactOutput(t *testing.T) {
	chain, _ := id.ParseChain("ethereum")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("DAI", chain)

	type quoteReq struct {
		Amount string `json:"amount"`
		Type   string `json:"type"`
	}
	var got quoteReq

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if err := json.NewDecoder(r.Body).Decode(&got); err != nil {
			http.Error(w, "invalid JSON payload", http.StatusBadRequest)
			return
		}

		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{
			"quote": {
				"input": {"amount": "1000900"},
				"output": {"amount": "1000000000000000000"},
				"gasFeeUSD": "0.12"
			}
		}`)
	}))
	defer srv.Close()

	c := New(httpx.New(1*time.Second, 0), "test-key")
	c.baseURL = srv.URL

	quote, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetIn,
		ToAsset:         assetOut,
		AmountBaseUnits: "1000000000000000000",
		AmountDecimal:   "1",
		TradeType:       providers.SwapTradeTypeExactOutput,
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}

	if got.Type != "EXACT_OUTPUT" {
		t.Fatalf("expected EXACT_OUTPUT payload type, got %s", got.Type)
	}
	if got.Amount != "1000000000000000000" {
		t.Fatalf("unexpected payload amount: %s", got.Amount)
	}
	if quote.TradeType != "exact-output" {
		t.Fatalf("expected trade_type exact-output, got %s", quote.TradeType)
	}
	if quote.InputAmount.AmountBaseUnits != "1000900" {
		t.Fatalf("unexpected input base amount: %s", quote.InputAmount.AmountBaseUnits)
	}
	if quote.InputAmount.AmountDecimal != "1.0009" {
		t.Fatalf("unexpected input decimal amount: %s", quote.InputAmount.AmountDecimal)
	}
	if quote.EstimatedOut.AmountBaseUnits != "1000000000000000000" {
		t.Fatalf("unexpected output amount: %s", quote.EstimatedOut.AmountBaseUnits)
	}
}

func TestQuoteSwapRequiresAPIKey(t *testing.T) {
	chain, _ := id.ParseChain("ethereum")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("DAI", chain)
	c := New(httpx.New(1*time.Second, 0), "")
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain: chain, FromAsset: assetIn, ToAsset: assetOut, AmountBaseUnits: "1000000", AmountDecimal: "1",
	})
	if err == nil {
		t.Fatal("expected missing API key error")
	}
}

func TestQuoteSwapRejectsNonEVMChain(t *testing.T) {
	chain, _ := id.ParseChain("solana")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("USDT", chain)
	c := New(httpx.New(1*time.Second, 0), "")
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain: chain, FromAsset: assetIn, ToAsset: assetOut, AmountBaseUnits: "1000000", AmountDecimal: "1",
	})
	if err == nil {
		t.Fatal("expected unsupported chain error")
	}
}
