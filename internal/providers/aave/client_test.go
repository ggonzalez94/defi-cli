package aave

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

func TestLendMarketsAndYield(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{
			"data": {
				"markets": [
					{
						"name": "AaveV3Ethereum",
						"chain": {"chainId": 1, "name": "Ethereum"},
						"reserves": [
							{
								"underlyingToken": {"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "symbol": "USDC", "decimals": 6},
								"size": {"usd": "1000000"},
								"supplyInfo": {"apy": {"value": "0.03"}, "total": {"value": "1000000"}},
								"borrowInfo": {"apy": {"value": "0.05"}, "total": {"usd": "500000"}, "utilizationRate": {"value": "0.4"}}
							}
						]
					}
				]
			}
		}`))
	}))
	defer srv.Close()

	client := New(httpx.New(2*time.Second, 0))
	client.endpoint = srv.URL
	chain, _ := id.ParseChain("ethereum")
	asset, _ := id.ParseAsset("USDC", chain)

	markets, err := client.LendMarkets(context.Background(), "aave", chain, asset)
	if err != nil {
		t.Fatalf("LendMarkets failed: %v", err)
	}
	if len(markets) != 1 {
		t.Fatalf("expected 1 market, got %d", len(markets))
	}
	if markets[0].SupplyAPY != 3 {
		t.Fatalf("expected supply apy 3, got %f", markets[0].SupplyAPY)
	}

	opps, err := client.YieldOpportunities(context.Background(), providers.YieldRequest{Chain: chain, Asset: asset, Limit: 10, MaxRisk: "high"})
	if err != nil {
		t.Fatalf("YieldOpportunities failed: %v", err)
	}
	if len(opps) != 1 || opps[0].Provider != "aave" {
		t.Fatalf("unexpected yield response: %+v", opps)
	}
}
