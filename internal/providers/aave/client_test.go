package aave

import (
	"context"
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
	if markets[0].ProviderNativeID == "" {
		t.Fatalf("expected provider native id, got %+v", markets[0])
	}
	if markets[0].Provider != "aave" || markets[0].ProviderNativeIDKind != model.NativeIDKindCompositeMarketAsset {
		t.Fatalf("expected provider/native id kind metadata, got %+v", markets[0])
	}

	opps, err := client.YieldOpportunities(context.Background(), providers.YieldRequest{Chain: chain, Asset: asset, Limit: 10, MaxRisk: "high"})
	if err != nil {
		t.Fatalf("YieldOpportunities failed: %v", err)
	}
	if len(opps) != 1 || opps[0].Provider != "aave" {
		t.Fatalf("unexpected yield response: %+v", opps)
	}
	if opps[0].ProviderNativeID == "" || opps[0].ProviderNativeIDKind != model.NativeIDKindCompositeMarketAsset {
		t.Fatalf("expected yield provider native id metadata, got %+v", opps[0])
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
