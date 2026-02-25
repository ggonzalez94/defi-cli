package jupiter

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

func TestQuoteSwapRejectsNonSolanaChains(t *testing.T) {
	chain, _ := id.ParseChain("ethereum")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("DAI", chain)

	c := New(httpx.New(2*time.Second, 0), "")
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetIn,
		ToAsset:         assetOut,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err == nil {
		t.Fatal("expected non-solana chain error")
	}
}

func TestQuoteSwapRejectsNonMainnetSolanaChain(t *testing.T) {
	chain := id.Chain{
		Name:  "Solana Devnet",
		Slug:  "solana-devnet",
		CAIP2: "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
	}
	c := New(httpx.New(2*time.Second, 0), "")
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{Chain: chain})
	if err == nil {
		t.Fatal("expected non-mainnet solana chain error")
	}
}

func TestQuoteSwapParsesJupiterResponse(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/quote", func(w http.ResponseWriter, r *http.Request) {
		if got := r.Header.Get("x-api-key"); got != "test-key" {
			t.Fatalf("expected x-api-key header, got %q", got)
		}
		_, _ = w.Write([]byte(`{
			"outAmount":"1995000",
			"priceImpactPct":"0.13",
			"routePlan":[
				{"swapInfo":{"label":"Meteora"}},
				{"swapInfo":{"label":"Orca"}}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	chain, _ := id.ParseChain("solana")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("USDT", chain)

	c := New(httpx.New(2*time.Second, 0), "test-key")
	c.baseURL = srv.URL
	got, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetIn,
		ToAsset:         assetOut,
		AmountBaseUnits: "2000000",
		AmountDecimal:   "2",
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}
	if got.Provider != "jupiter" {
		t.Fatalf("unexpected provider: %+v", got)
	}
	if got.TradeType != "exact-input" {
		t.Fatalf("unexpected trade type: %s", got.TradeType)
	}
	if got.EstimatedOut.AmountBaseUnits != "1995000" {
		t.Fatalf("unexpected amount out: %+v", got.EstimatedOut)
	}
	if got.PriceImpactPct != 0.13 {
		t.Fatalf("unexpected price impact: %f", got.PriceImpactPct)
	}
	if got.Route != "Meteora > Orca" {
		t.Fatalf("unexpected route: %s", got.Route)
	}
}

func TestQuoteSwapRejectsExactOutput(t *testing.T) {
	chain, _ := id.ParseChain("solana")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("USDT", chain)

	c := New(httpx.New(2*time.Second, 0), "")
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetIn,
		ToAsset:         assetOut,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
		TradeType:       providers.SwapTradeTypeExactOutput,
	})
	if err == nil {
		t.Fatal("expected unsupported exact-output error")
	}
}
