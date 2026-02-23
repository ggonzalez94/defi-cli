package bungee

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

func TestQuoteBridgeAutoRoute(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if got := r.URL.Path; got != "/api/v1/bungee/quote" {
			t.Fatalf("unexpected path: %s", got)
		}
		q := r.URL.Query()
		if q.Get("originChainId") != "1" || q.Get("destinationChainId") != "8453" {
			t.Fatalf("unexpected chain ids: %s -> %s", q.Get("originChainId"), q.Get("destinationChainId"))
		}
		if q.Get("inputAmount") != "1000000" {
			t.Fatalf("unexpected input amount: %s", q.Get("inputAmount"))
		}
		_, _ = w.Write([]byte(`{
			"success": true,
			"result": {
				"originChainId": 1,
				"destinationChainId": 8453,
				"autoRoute": {
					"estimatedTime": 10,
					"gasFee": {"feeInUsd": 0.00563382},
					"routeDetails": {"name": "Bungee Protocol"},
					"output": {"amount": "995000", "token": {"decimals": 6}},
					"outputAmount": "999735"
				}
			}
		}`))
	}))
	defer srv.Close()

	chainFrom, _ := id.ParseChain("ethereum")
	chainTo, _ := id.ParseChain("base")
	assetFrom, _ := id.ParseAsset("USDC", chainFrom)
	assetTo, _ := id.ParseAsset("USDC", chainTo)

	c := NewBridge(httpx.New(time.Second, 0), "", "")
	c.baseURL = srv.URL + "/api/v1"
	got, err := c.QuoteBridge(context.Background(), providers.BridgeQuoteRequest{
		FromChain:       chainFrom,
		ToChain:         chainTo,
		FromAsset:       assetFrom,
		ToAsset:         assetTo,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteBridge failed: %v", err)
	}
	if got.Provider != "bungee" {
		t.Fatalf("unexpected provider: %s", got.Provider)
	}
	if got.EstimatedOut.AmountBaseUnits != "999735" {
		t.Fatalf("unexpected out amount: %s", got.EstimatedOut.AmountBaseUnits)
	}
	if got.EstimatedFeeUSD != 0.00563382 {
		t.Fatalf("unexpected fee usd: %v", got.EstimatedFeeUSD)
	}
	if got.EstimatedTimeS != 10 {
		t.Fatalf("unexpected service time: %d", got.EstimatedTimeS)
	}
	if got.Route != "bungee:auto:bungee protocol" {
		t.Fatalf("unexpected route: %s", got.Route)
	}
}

func TestQuoteSwapHyperEVM(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		if q.Get("originChainId") != "999" || q.Get("destinationChainId") != "999" {
			t.Fatalf("unexpected chain ids: %s -> %s", q.Get("originChainId"), q.Get("destinationChainId"))
		}
		if q.Get("userAddress") != defaultEVMUserAddress || q.Get("receiverAddress") != defaultEVMUserAddress {
			t.Fatalf("unexpected evm placeholder addresses: %s / %s", q.Get("userAddress"), q.Get("receiverAddress"))
		}
		_, _ = w.Write([]byte(`{
			"success": true,
			"result": {
				"originChainId": 999,
				"destinationChainId": 999,
				"autoRoute": {
					"gasFee": {"feeInUsd": 0.04},
					"estimatedTime": 7,
					"userTxs": [{"stepType": "swap", "swapRoutes": [{"usedDexName": "HyperSwap"}]}],
					"output": {"amount": "1000000000000000000", "token": {"decimals": 18}},
					"outputAmount": "1000000000000000001"
				}
			}
		}`))
	}))
	defer srv.Close()

	chain, _ := id.ParseChain("hyperevm")
	assetFrom, _ := id.ParseAsset("USDC", chain)
	assetTo, _ := id.ParseAsset("WHYPE", chain)

	c := NewSwap(httpx.New(time.Second, 0), "", "")
	c.baseURL = srv.URL + "/api/v1"
	got, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetFrom,
		ToAsset:         assetTo,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}
	if got.Provider != "bungee" {
		t.Fatalf("unexpected provider: %s", got.Provider)
	}
	if got.ChainID != chain.CAIP2 {
		t.Fatalf("unexpected chain id: %s", got.ChainID)
	}
	if got.EstimatedOut.AmountBaseUnits != "1000000000000000001" {
		t.Fatalf("unexpected out amount: %s", got.EstimatedOut.AmountBaseUnits)
	}
	if got.EstimatedOut.Decimals != 18 {
		t.Fatalf("expected output decimals=18, got %d", got.EstimatedOut.Decimals)
	}
	if got.EstimatedGasUSD != 0.04 {
		t.Fatalf("unexpected gas usd: %v", got.EstimatedGasUSD)
	}
	if got.Route != "bungee:auto:swap(hyperswap)" {
		t.Fatalf("unexpected route: %s", got.Route)
	}
}

func TestQuoteSwapHandlesNullGasFee(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"success": true,
			"result": {
				"originChainId": 1,
				"destinationChainId": 1,
				"autoRoute": {
					"estimatedTime": 10,
					"gasFee": null,
					"routeDetails": {"name": "Bungee Protocol"},
					"output": {"amount": "1999735", "token": {"decimals": 6}}
				}
			}
		}`))
	}))
	defer srv.Close()

	chain, _ := id.ParseChain("ethereum")
	assetFrom, _ := id.ParseAsset("USDC", chain)
	assetTo, _ := id.ParseAsset("USDT", chain)

	c := NewSwap(httpx.New(time.Second, 0), "", "")
	c.baseURL = srv.URL + "/api/v1"
	got, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetFrom,
		ToAsset:         assetTo,
		AmountBaseUnits: "2000000",
		AmountDecimal:   "2",
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}
	if got.EstimatedGasUSD != 0 {
		t.Fatalf("expected zero gas usd, got %v", got.EstimatedGasUSD)
	}
	if got.EstimatedOut.AmountBaseUnits != "1999735" {
		t.Fatalf("unexpected out amount: %s", got.EstimatedOut.AmountBaseUnits)
	}
}

func TestQuoteBridgeNoAutoRouteReturnsEmptyRoute(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"success": true,
			"result": {
				"originChainId": 1,
				"destinationChainId": 8453,
				"output": {"amount": "999735", "token": {"decimals": 6}}
			}
		}`))
	}))
	defer srv.Close()

	chainFrom, _ := id.ParseChain("ethereum")
	chainTo, _ := id.ParseChain("base")
	assetFrom, _ := id.ParseAsset("USDC", chainFrom)
	assetTo, _ := id.ParseAsset("USDC", chainTo)

	c := NewBridge(httpx.New(time.Second, 0), "", "")
	c.baseURL = srv.URL + "/api/v1"
	got, err := c.QuoteBridge(context.Background(), providers.BridgeQuoteRequest{
		FromChain:       chainFrom,
		ToChain:         chainTo,
		FromAsset:       assetFrom,
		ToAsset:         assetTo,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteBridge failed: %v", err)
	}
	if got.Route != "" {
		t.Fatalf("unexpected route: %s", got.Route)
	}
}

func TestQuoteHandlesUnsuccessfulEnvelope(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{"success": false, "error": {"message":"no routes found"}}`))
	}))
	defer srv.Close()

	chain, _ := id.ParseChain("ethereum")
	assetFrom, _ := id.ParseAsset("USDC", chain)
	assetTo, _ := id.ParseAsset("USDT", chain)

	c := NewSwap(httpx.New(time.Second, 0), "", "")
	c.baseURL = srv.URL + "/api/v1"
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetFrom,
		ToAsset:         assetTo,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err == nil {
		t.Fatal("expected quote error")
	}
}

func TestQuoteUsesDedicatedBackendAndHeadersWhenAPIKeyAndAffiliateProvided(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if got := r.URL.Path; got != "/api/v1/bungee/quote" {
			t.Fatalf("unexpected path: %s", got)
		}
		if got := r.Header.Get("x-api-key"); got != "test-key" {
			t.Fatalf("unexpected x-api-key header: %q", got)
		}
		if got := r.Header.Get("affiliate"); got != "test-affiliate" {
			t.Fatalf("unexpected affiliate header: %q", got)
		}
		_, _ = w.Write([]byte(`{
			"success": true,
			"result": {
				"autoRoute": {
					"outputAmount": "999735",
					"output": {"token": {"decimals": 6}}
				}
			}
		}`))
	}))
	defer srv.Close()

	chainFrom, _ := id.ParseChain("ethereum")
	chainTo, _ := id.ParseChain("base")
	assetFrom, _ := id.ParseAsset("USDC", chainFrom)
	assetTo, _ := id.ParseAsset("USDC", chainTo)

	c := NewBridge(httpx.New(time.Second, 0), "test-key", "test-affiliate")
	c.baseURL = srv.URL + "/unused-public"
	c.dedicatedBaseURL = srv.URL + "/api/v1"
	_, err := c.QuoteBridge(context.Background(), providers.BridgeQuoteRequest{
		FromChain:       chainFrom,
		ToChain:         chainTo,
		FromAsset:       assetFrom,
		ToAsset:         assetTo,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteBridge failed: %v", err)
	}
}

func TestQuoteUsesPublicBackendWhenDedicatedConfigIsIncomplete(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if got := r.URL.Path; got != "/api/v1/bungee/quote" {
			t.Fatalf("unexpected path: %s", got)
		}
		if got := r.Header.Get("x-api-key"); got != "" {
			t.Fatalf("unexpected x-api-key header: %q", got)
		}
		if got := r.Header.Get("affiliate"); got != "" {
			t.Fatalf("unexpected affiliate header: %q", got)
		}
		_, _ = w.Write([]byte(`{
			"success": true,
			"result": {
				"autoRoute": {
					"outputAmount": "999735",
					"output": {"token": {"decimals": 6}}
				}
			}
		}`))
	}))
	defer srv.Close()

	chainFrom, _ := id.ParseChain("ethereum")
	chainTo, _ := id.ParseChain("base")
	assetFrom, _ := id.ParseAsset("USDC", chainFrom)
	assetTo, _ := id.ParseAsset("USDC", chainTo)

	c := NewBridge(httpx.New(time.Second, 0), "test-key", "")
	c.baseURL = srv.URL + "/api/v1"
	c.dedicatedBaseURL = srv.URL + "/unused-dedicated"
	_, err := c.QuoteBridge(context.Background(), providers.BridgeQuoteRequest{
		FromChain:       chainFrom,
		ToChain:         chainTo,
		FromAsset:       assetFrom,
		ToAsset:         assetTo,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteBridge failed: %v", err)
	}
}
