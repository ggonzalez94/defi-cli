package morpho

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/gustavo/defi-cli/internal/httpx"
	"github.com/gustavo/defi-cli/internal/id"
	"github.com/gustavo/defi-cli/internal/providers"
)

func TestLendRatesAndYield(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{
			"data": {
				"markets": {
					"items": [
						{
							"uniqueKey": "m1",
							"loanAsset": {"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "symbol": "USDC", "decimals": 6, "chain": {"id": 1, "network": "ethereum"}},
							"collateralAsset": {"address": "0x111", "symbol": "WETH"},
							"state": {"supplyApy": 0.02, "borrowApy": 0.03, "utilization": 0.5, "supplyAssetsUsd": 2000000, "liquidityAssetsUsd": 1000000, "totalLiquidityUsd": 1200000}
						}
					]
				}
			}
		}`))
	}))
	defer srv.Close()

	client := New(httpx.New(2*time.Second, 0))
	client.endpoint = srv.URL
	chain, _ := id.ParseChain("ethereum")
	asset, _ := id.ParseAsset("USDC", chain)

	rates, err := client.LendRates(context.Background(), "morpho", chain, asset)
	if err != nil {
		t.Fatalf("LendRates failed: %v", err)
	}
	if len(rates) != 1 {
		t.Fatalf("expected 1 rate, got %d", len(rates))
	}
	if rates[0].SupplyAPY != 2 {
		t.Fatalf("expected supply apy 2, got %f", rates[0].SupplyAPY)
	}

	opps, err := client.YieldOpportunities(context.Background(), providers.YieldRequest{Chain: chain, Asset: asset, Limit: 10, MaxRisk: "high"})
	if err != nil {
		t.Fatalf("YieldOpportunities failed: %v", err)
	}
	if len(opps) != 1 || opps[0].Provider != "morpho" {
		t.Fatalf("unexpected opportunities: %+v", opps)
	}
}
