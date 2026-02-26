package aave

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

func TestLendMarketsAndYield(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{
			"data": {
				"markets": [
					{
						"name": "AaveV3Ethereum",
						"address": "0x87870Bca3F3fD6335C3F4ce8392D69350B4fA4E2",
						"chain": {"chainId": 1, "name": "Ethereum"},
						"reserves": [
								{
									"underlyingToken": {"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "symbol": "USDC", "decimals": 6},
									"aToken": {"address": "0x71Aef7b30728b9BB371578f36c5A1f1502a5723e"},
									"size": {"usd": "1000000"},
									"supplyInfo": {"apy": {"value": "0.03"}, "total": {"value": "1000000"}},
									"borrowInfo": {"apy": {"value": "0.05"}, "total": {"usd": "500000"}, "utilizationRate": {"value": "0.4"}, "availableLiquidity": {"usd": "600000"}}
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
	if markets[0].ProviderNativeID == "" {
		t.Fatalf("expected provider native id, got %+v", markets[0])
	}
	if markets[0].Provider != "aave" || markets[0].ProviderNativeIDKind != model.NativeIDKindCompositeMarketAsset {
		t.Fatalf("expected provider/native id kind metadata, got %+v", markets[0])
	}

	opps, err := client.YieldOpportunities(context.Background(), providers.YieldRequest{Chain: chain, Asset: asset, Limit: 10})
	if err != nil {
		t.Fatalf("YieldOpportunities failed: %v", err)
	}
	if len(opps) != 1 || opps[0].Provider != "aave" {
		t.Fatalf("unexpected yield response: %+v", opps)
	}
	if opps[0].ProviderNativeID == "" || opps[0].ProviderNativeIDKind != model.NativeIDKindCompositeMarketAsset {
		t.Fatalf("expected yield provider native id metadata, got %+v", opps[0])
	}
	if opps[0].LiquidityUSD != 600000 {
		t.Fatalf("expected liquidity 600000 from borrowInfo.availableLiquidity, got %+v", opps[0])
	}
	if len(opps[0].BackingAssets) != 1 || opps[0].BackingAssets[0].SharePct != 100 {
		t.Fatalf("expected single backing asset at 100%%, got %+v", opps[0].BackingAssets)
	}
}

func TestLendMarketsPrefersAddressMatchOverSymbol(t *testing.T) {
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
								"underlyingToken": {"address": "0x0000000000000000000000000000000000000001", "symbol": "USDC", "decimals": 6},
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

	_, err := client.LendMarkets(context.Background(), "aave", chain, asset)
	if err == nil {
		t.Fatal("expected no market match due address mismatch")
	}
}

func TestLendPositionsTypeSplit(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "application/json")

		switch {
		case strings.Contains(string(body), "MarketAddresses"):
			_, _ = w.Write([]byte(`{
				"data": {
					"markets": [
						{"address": "0x1111111111111111111111111111111111111111"}
					]
				}
			}`))
		case strings.Contains(string(body), "Positions"):
			_, _ = w.Write([]byte(`{
				"data": {
					"userSupplies": [
						{
							"market": {"address": "0x1111111111111111111111111111111111111111"},
							"currency": {"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "symbol": "USDC", "decimals": 6},
							"balance": {"amount": {"raw": "1000000", "decimals": 6, "value": "1"}, "usd": "1"},
							"apy": {"value": "0.03"},
							"isCollateral": false,
							"canBeCollateral": true
						},
						{
							"market": {"address": "0x1111111111111111111111111111111111111111"},
							"currency": {"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "symbol": "USDC", "decimals": 6},
							"balance": {"amount": {"raw": "2000000", "decimals": 6, "value": "2"}, "usd": "2"},
							"apy": {"value": "0.03"},
							"isCollateral": true,
							"canBeCollateral": true
						}
					],
					"userBorrows": [
						{
							"market": {"address": "0x1111111111111111111111111111111111111111"},
							"currency": {"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "symbol": "USDC", "decimals": 6},
							"debt": {"amount": {"raw": "500000", "decimals": 6, "value": "0.5"}, "usd": "0.5"},
							"apy": {"value": "0.05"}
						}
					]
				}
			}`))
		default:
			_, _ = w.Write([]byte(`{"errors":[{"message":"unexpected query"}]}`))
		}
	}))
	defer srv.Close()

	client := New(httpx.New(2*time.Second, 0))
	client.endpoint = srv.URL
	chain, _ := id.ParseChain("ethereum")
	account := "0x000000000000000000000000000000000000dEaD"

	all, err := client.LendPositions(context.Background(), providers.LendPositionsRequest{
		Chain:        chain,
		Account:      account,
		PositionType: providers.LendPositionTypeAll,
	})
	if err != nil {
		t.Fatalf("LendPositions(all) failed: %v", err)
	}
	if len(all) != 3 {
		t.Fatalf("expected 3 positions, got %d", len(all))
	}
	counts := map[string]int{}
	for _, item := range all {
		counts[item.PositionType]++
	}
	if counts[string(providers.LendPositionTypeSupply)] != 1 {
		t.Fatalf("expected one supply position, got %+v", counts)
	}
	if counts[string(providers.LendPositionTypeCollateral)] != 1 {
		t.Fatalf("expected one collateral position, got %+v", counts)
	}
	if counts[string(providers.LendPositionTypeBorrow)] != 1 {
		t.Fatalf("expected one borrow position, got %+v", counts)
	}

	supplyOnly, err := client.LendPositions(context.Background(), providers.LendPositionsRequest{
		Chain:        chain,
		Account:      account,
		PositionType: providers.LendPositionTypeSupply,
	})
	if err != nil {
		t.Fatalf("LendPositions(supply) failed: %v", err)
	}
	if len(supplyOnly) != 1 || supplyOnly[0].PositionType != string(providers.LendPositionTypeSupply) {
		t.Fatalf("expected non-collateral supply-only row, got %+v", supplyOnly)
	}

	collateralOnly, err := client.LendPositions(context.Background(), providers.LendPositionsRequest{
		Chain:        chain,
		Account:      account,
		PositionType: providers.LendPositionTypeCollateral,
	})
	if err != nil {
		t.Fatalf("LendPositions(collateral) failed: %v", err)
	}
	if len(collateralOnly) != 1 || collateralOnly[0].PositionType != string(providers.LendPositionTypeCollateral) {
		t.Fatalf("expected collateral-only row, got %+v", collateralOnly)
	}
}

func TestYieldHistoryAPY(t *testing.T) {
	fixedNow := time.Date(2026, 2, 26, 20, 0, 0, 0, time.UTC)
	start := fixedNow.Add(-6 * time.Hour)
	market := "0x1111111111111111111111111111111111111111"
	underlying := "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		if !strings.Contains(string(body), "SupplyAPYHistory") {
			t.Fatalf("expected SupplyAPYHistory query, got %s", string(body))
		}
		if !strings.Contains(string(body), "\"window\":\"LAST_DAY\"") {
			t.Fatalf("expected LAST_DAY window, got %s", string(body))
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(fmt.Sprintf(`{
			"data": {
				"supplyAPYHistory": [
					{"date": %q, "avgRate": {"value": "0.02"}},
					{"date": %q, "avgRate": {"value": "0.018"}}
				]
			}
		}`, fixedNow.Add(-5*time.Hour).Format(time.RFC3339), fixedNow.Add(-3*time.Hour).Format(time.RFC3339))))
	}))
	defer srv.Close()

	client := New(httpx.New(2*time.Second, 0))
	client.endpoint = srv.URL
	client.now = func() time.Time { return fixedNow }

	series, err := client.YieldHistory(context.Background(), providers.YieldHistoryRequest{
		Opportunity: model.YieldOpportunity{
			OpportunityID:        "opp-1",
			Provider:             "aave",
			Protocol:             "aave",
			ChainID:              "eip155:1",
			AssetID:              "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
			ProviderNativeID:     "aave:eip155:1:" + market + ":" + underlying,
			ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
			SourceURL:            "https://app.aave.com",
		},
		StartTime: start,
		EndTime:   fixedNow,
		Interval:  providers.YieldHistoryIntervalHour,
		Metrics:   []providers.YieldHistoryMetric{providers.YieldHistoryMetricAPYTotal},
	})
	if err != nil {
		t.Fatalf("YieldHistory failed: %v", err)
	}
	if len(series) != 1 {
		t.Fatalf("expected one series, got %+v", series)
	}
	if series[0].Metric != string(providers.YieldHistoryMetricAPYTotal) {
		t.Fatalf("unexpected metric: %+v", series[0])
	}
	if len(series[0].Points) != 2 {
		t.Fatalf("expected two points, got %+v", series[0].Points)
	}
	if series[0].Points[0].Value != 2 {
		t.Fatalf("expected first point value 2, got %+v", series[0].Points[0])
	}
}

func TestYieldHistoryRejectsUnsupportedMetric(t *testing.T) {
	client := New(httpx.New(2*time.Second, 0))
	client.now = func() time.Time { return time.Date(2026, 2, 26, 20, 0, 0, 0, time.UTC) }

	_, err := client.YieldHistory(context.Background(), providers.YieldHistoryRequest{
		Opportunity: model.YieldOpportunity{
			Provider:         "aave",
			Protocol:         "aave",
			ChainID:          "eip155:1",
			ProviderNativeID: "aave:eip155:1:0x1111111111111111111111111111111111111111:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
		},
		StartTime: client.now().UTC().Add(-time.Hour),
		EndTime:   client.now().UTC(),
		Interval:  providers.YieldHistoryIntervalHour,
		Metrics:   []providers.YieldHistoryMetric{providers.YieldHistoryMetricTVLUSD},
	})
	if err == nil {
		t.Fatal("expected unsupported metric error")
	}
}
