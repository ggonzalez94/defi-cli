package morpho

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

func TestLendRatesAndYield(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "application/json")
		query := string(body)

		switch {
		case strings.Contains(query, "query Markets("):
			_, _ = w.Write([]byte(`{
				"data": {
					"markets": {
						"items": [
						{
							"id": "4f598145-0188-44dc-9e18-38a2817020a1",
							"uniqueKey": "m1",
							"irmAddress": "0x870aC11D48B15DB9a138Cf899d20F13F79Ba00BC",
							"loanAsset": {"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "symbol": "USDC", "decimals": 6, "chain": {"id": 1, "network": "ethereum"}},
							"collateralAsset": {"address": "0x111", "symbol": "WETH"},
							"state": {"supplyApy": 0.02, "borrowApy": 0.03, "utilization": 0.5, "supplyAssetsUsd": 2000000, "liquidityAssetsUsd": 1000000, "totalLiquidityUsd": 1200000}
						}
					]
					}
				}
			}`))
		case strings.Contains(query, "query Vaults("):
			_, _ = w.Write([]byte(`{
				"data": {
					"vaults": {
						"items": [
							{
								"address": "0x1111111111111111111111111111111111111111",
								"name": "Morpho USDC Vault",
								"symbol": "vUSDC",
								"asset": {"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "symbol": "USDC"},
								"state": {
									"netApy": 0.05,
									"totalAssetsUsd": 1000000,
									"allocation": [
										{
											"supplyAssetsUsd": 1000000,
											"market": {"collateralAsset": {"address": "0x111", "symbol": "WETH"}}
										}
									]
								},
								"liquidity": {"usd": 500000}
							}
						]
					}
				}
			}`))
		case strings.Contains(query, "query VaultV2s("):
			_, _ = w.Write([]byte(`{
				"data": {
					"vaultV2s": {
						"items": [
							{
								"address": "0x2222222222222222222222222222222222222222",
								"name": "Morpho USDC V2 Vault",
								"symbol": "v2USDC",
								"asset": {"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "symbol": "USDC"},
								"netApy": 0.03,
								"totalAssetsUsd": 2000000,
								"liquidityUsd": 1500000,
								"liquidityData": {
									"__typename": "MetaMorphoLiquidityData",
									"metaMorpho": {
										"state": {
											"allocation": [
												{
													"supplyAssetsUsd": 2000000,
													"market": {"collateralAsset": {"address": "0x6b175474e89094c44da98b954eedeac495271d0f", "symbol": "DAI"}}
												}
											]
										}
									}
								}
							},
							{
								"address": "0x3333333333333333333333333333333333333333",
								"name": "Morpho USDT V2 Vault",
								"symbol": "v2USDT",
								"asset": {"address": "0xdac17f958d2ee523a2206206994597c13d831ec7", "symbol": "USDT"},
								"netApy": 0.09,
								"totalAssetsUsd": 3000000,
								"liquidityUsd": 2500000,
								"liquidityData": {"__typename": "MetaMorphoLiquidityData"}
							}
						]
					}
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
	if rates[0].ProviderNativeID != "m1" {
		t.Fatalf("expected provider native id m1, got %+v", rates[0])
	}
	if rates[0].Provider != "morpho" || rates[0].ProviderNativeIDKind != model.NativeIDKindMarketID {
		t.Fatalf("expected morpho provider id metadata, got %+v", rates[0])
	}

	opps, err := client.YieldOpportunities(context.Background(), providers.YieldRequest{Chain: chain, Asset: asset, Limit: 10, MaxRisk: "high"})
	if err != nil {
		t.Fatalf("YieldOpportunities failed: %v", err)
	}
	if len(opps) != 2 {
		t.Fatalf("unexpected opportunities: %+v", opps)
	}

	byID := map[string]model.YieldOpportunity{}
	for _, opp := range opps {
		if opp.Provider != "morpho" {
			t.Fatalf("expected morpho provider, got %+v", opp)
		}
		byID[opp.ProviderNativeID] = opp
	}

	vaultOne, ok := byID["0x1111111111111111111111111111111111111111"]
	if !ok {
		t.Fatalf("expected first vault id in output, got %+v", byID)
	}
	if vaultOne.ProviderNativeIDKind != model.NativeIDKindVaultAddress {
		t.Fatalf("expected vault_address kind on first vault, got %+v", vaultOne)
	}
	if vaultOne.RiskLevel != "medium" || len(vaultOne.RiskReasons) == 0 || vaultOne.RiskReasons[0] != "non-stable collateral" {
		t.Fatalf("expected medium/non-stable risk on first vault, got %+v", vaultOne)
	}

	vaultTwo, ok := byID["0x2222222222222222222222222222222222222222"]
	if !ok {
		t.Fatalf("expected second vault id in output, got %+v", byID)
	}
	if vaultTwo.ProviderNativeIDKind != model.NativeIDKindVaultAddress {
		t.Fatalf("expected vault_address kind on second vault, got %+v", vaultTwo)
	}
	if vaultTwo.RiskLevel != "low" || len(vaultTwo.RiskReasons) == 0 || vaultTwo.RiskReasons[0] != "stable collateral" {
		t.Fatalf("expected low/stable risk on second vault, got %+v", vaultTwo)
	}
	if _, ok := byID["0x3333333333333333333333333333333333333333"]; ok {
		t.Fatalf("expected USDT vault to be filtered out for USDC request, got %+v", byID)
	}
}

func TestYieldOpportunitiesVaultMaxRiskFilter(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "application/json")
		query := string(body)
		switch {
		case strings.Contains(query, "query Vaults("):
			_, _ = w.Write([]byte(`{
				"data": {
					"vaults": {
						"items": [
							{
								"address": "0x1111111111111111111111111111111111111111",
								"name": "Morpho USDC Vault",
								"symbol": "vUSDC",
								"asset": {"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "symbol": "USDC"},
								"state": {
									"netApy": 0.06,
									"totalAssetsUsd": 1000000,
									"allocation": [
										{
											"supplyAssetsUsd": 1000000,
											"market": {"collateralAsset": {"address": "0x111", "symbol": "WETH"}}
										}
									]
								},
								"liquidity": {"usd": 700000}
							}
						]
					}
				}
			}`))
		case strings.Contains(query, "query VaultV2s("):
			_, _ = w.Write([]byte(`{
				"data": {
					"vaultV2s": {
						"items": [
							{
								"address": "0x2222222222222222222222222222222222222222",
								"name": "Morpho USDC V2 Vault",
								"symbol": "v2USDC",
								"asset": {"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "symbol": "USDC"},
								"netApy": 0.03,
								"totalAssetsUsd": 2000000,
								"liquidityUsd": 1800000,
								"liquidityData": {
									"__typename": "MetaMorphoLiquidityData",
									"metaMorpho": {
										"state": {
											"allocation": [
												{
													"supplyAssetsUsd": 2000000,
													"market": {"collateralAsset": {"address": "0x6b175474e89094c44da98b954eedeac495271d0f", "symbol": "DAI"}}
												}
											]
										}
									}
								}
							}
						]
					}
				}
			}`))
		default:
			_, _ = w.Write([]byte(`{"data":{"markets":{"items":[]}}}`))
		}
	}))
	defer srv.Close()

	client := New(httpx.New(2*time.Second, 0))
	client.endpoint = srv.URL
	chain, _ := id.ParseChain("ethereum")
	asset, _ := id.ParseAsset("USDC", chain)

	opps, err := client.YieldOpportunities(context.Background(), providers.YieldRequest{Chain: chain, Asset: asset, Limit: 10, MaxRisk: "low"})
	if err != nil {
		t.Fatalf("YieldOpportunities failed: %v", err)
	}
	if len(opps) != 1 {
		t.Fatalf("expected one low-risk vault after max-risk filter, got %+v", opps)
	}
	if opps[0].ProviderNativeID != "0x2222222222222222222222222222222222222222" {
		t.Fatalf("expected low-risk vault id, got %+v", opps[0])
	}
	if opps[0].RiskLevel != "low" {
		t.Fatalf("expected low risk, got %+v", opps[0])
	}
}

func TestLendPositionsTypeSplit(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "application/json")

		if !strings.Contains(string(body), "marketPositions") {
			_, _ = w.Write([]byte(`{"errors":[{"message":"unexpected query"}]}`))
			return
		}

		_, _ = w.Write([]byte(`{
			"data": {
				"marketPositions": {
					"items": [
						{
							"id": "position-1",
							"market": {
								"uniqueKey": "market-1",
								"loanAsset": {
									"address": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
									"symbol": "USDC",
									"decimals": 6,
									"chain": {"id": 1, "network": "ethereum"}
								},
								"collateralAsset": {
									"address": "0x4200000000000000000000000000000000000006",
									"symbol": "WETH",
									"decimals": 18
								},
								"state": {"supplyApy": 0.02, "borrowApy": 0.03}
							},
							"state": {
								"supplyAssets": "1500000",
								"supplyAssetsUsd": 1.5,
								"borrowAssets": "500000",
								"borrowAssetsUsd": 0.5,
								"collateral": "1000000000000000000",
								"collateralUsd": 2000
							}
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
		t.Fatalf("expected 3 distinct positions, got %d", len(all))
	}
	counts := map[string]int{}
	for _, item := range all {
		counts[item.PositionType]++
	}
	if counts[string(providers.LendPositionTypeSupply)] != 1 {
		t.Fatalf("expected one supply row, got %+v", counts)
	}
	if counts[string(providers.LendPositionTypeBorrow)] != 1 {
		t.Fatalf("expected one borrow row, got %+v", counts)
	}
	if counts[string(providers.LendPositionTypeCollateral)] != 1 {
		t.Fatalf("expected one collateral row, got %+v", counts)
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
		t.Fatalf("expected supply-only row, got %+v", supplyOnly)
	}

	usdcOnly, err := client.LendPositions(context.Background(), providers.LendPositionsRequest{
		Chain:        chain,
		Account:      account,
		PositionType: providers.LendPositionTypeAll,
		Asset: id.Asset{
			ChainID: chain.CAIP2,
			Symbol:  "USDC",
		},
	})
	if err != nil {
		t.Fatalf("LendPositions(asset=USDC) failed: %v", err)
	}
	if len(usdcOnly) != 2 {
		t.Fatalf("expected supply+borrow rows for USDC filter, got %+v", usdcOnly)
	}
}
