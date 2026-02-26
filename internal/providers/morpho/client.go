package morpho

import (
	"context"
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/big"
	"net/http"
	"sort"
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/ggonzalez94/defi-cli/internal/providers/yieldutil"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

const defaultEndpoint = registry.MorphoGraphQLEndpoint

type Client struct {
	http     *httpx.Client
	endpoint string
	now      func() time.Time
}

func New(httpClient *httpx.Client) *Client {
	return &Client{http: httpClient, endpoint: defaultEndpoint, now: time.Now}
}

func (c *Client) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:        "morpho",
		Type:        "lending+yield",
		RequiresKey: false,
		Capabilities: []string{
			"lend.markets",
			"lend.rates",
			"lend.positions",
			"yield.opportunities",
			"lend.plan",
			"lend.execute",
		},
	}
}

const marketsQuery = `query Markets($first:Int,$where:MarketFilters,$orderBy:MarketOrderBy,$orderDirection:OrderDirection){
  markets(first:$first, where:$where, orderBy:$orderBy, orderDirection:$orderDirection){
    items{
      id
      uniqueKey
      irmAddress
      loanAsset{ address symbol decimals chain{ id network } }
      collateralAsset{ address symbol }
      state{ supplyApy borrowApy utilization supplyAssetsUsd liquidityAssetsUsd totalLiquidityUsd }
    }
  }
}`

const positionsQuery = `query Positions($first:Int,$where:MarketPositionFilters,$orderBy:MarketPositionOrderBy,$orderDirection:OrderDirection){
  marketPositions(first:$first, where:$where, orderBy:$orderBy, orderDirection:$orderDirection){
    items{
      id
      market{
        uniqueKey
        loanAsset{ address symbol decimals chain{ id network } }
        collateralAsset{ address symbol decimals }
        state{ supplyApy borrowApy }
      }
      state{
        supplyAssets
        supplyAssetsUsd
        borrowAssets
        borrowAssetsUsd
        collateral
        collateralUsd
      }
    }
  }
}`

const vaultsYieldQuery = `query Vaults($first:Int,$skip:Int,$where:VaultFilters,$orderBy:VaultOrderBy,$orderDirection:OrderDirection){
  vaults(first:$first, skip:$skip, where:$where, orderBy:$orderBy, orderDirection:$orderDirection){
    items{
      address
      name
      symbol
      asset{ address symbol }
      state{
        netApy
        totalAssetsUsd
        allocation{
          supplyAssetsUsd
          market{
            collateralAsset{ address symbol }
          }
        }
      }
      liquidity{ usd }
    }
  }
}`

const vaultV2sYieldQuery = `query VaultV2s($first:Int,$skip:Int,$where:VaultV2sFilters,$orderBy:VaultV2OrderBy,$orderDirection:OrderDirection){
  vaultV2s(first:$first, skip:$skip, where:$where, orderBy:$orderBy, orderDirection:$orderDirection){
    items{
      address
      name
      symbol
      asset{ address symbol }
      netApy
      totalAssetsUsd
      liquidityUsd
      liquidityData{
        __typename
        ... on MarketV1LiquidityData {
          market{
            collateralAsset{ address symbol }
          }
        }
        ... on MetaMorphoLiquidityData {
          metaMorpho{
            state{
              allocation{
                supplyAssetsUsd
                market{
                  collateralAsset{ address symbol }
                }
              }
            }
          }
        }
      }
    }
  }
}`

const (
	yieldVaultPageSize = 200
	yieldVaultMaxPages = 20
)

type marketsResponse struct {
	Data struct {
		Markets struct {
			Items []morphoMarket `json:"items"`
		} `json:"markets"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type positionsResponse struct {
	Data struct {
		MarketPositions struct {
			Items []morphoMarketPosition `json:"items"`
		} `json:"marketPositions"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type vaultsResponse struct {
	Data struct {
		Vaults struct {
			Items []morphoVault `json:"items"`
		} `json:"vaults"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type vaultV2sResponse struct {
	Data struct {
		VaultV2s struct {
			Items []morphoVaultV2 `json:"items"`
		} `json:"vaultV2s"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type morphoMarket struct {
	ID         string `json:"id"`
	UniqueKey  string `json:"uniqueKey"`
	IRMAddress string `json:"irmAddress"`
	LoanAsset  struct {
		Address  string `json:"address"`
		Symbol   string `json:"symbol"`
		Decimals int    `json:"decimals"`
		Chain    struct {
			ID      int64  `json:"id"`
			Network string `json:"network"`
		} `json:"chain"`
	} `json:"loanAsset"`
	CollateralAsset *struct {
		Address string `json:"address"`
		Symbol  string `json:"symbol"`
	} `json:"collateralAsset"`
	State struct {
		SupplyAPY          float64 `json:"supplyApy"`
		BorrowAPY          float64 `json:"borrowApy"`
		Utilization        float64 `json:"utilization"`
		SupplyAssetsUSD    float64 `json:"supplyAssetsUsd"`
		LiquidityAssetsUSD float64 `json:"liquidityAssetsUsd"`
		TotalLiquidityUSD  float64 `json:"totalLiquidityUsd"`
	} `json:"state"`
}

type morphoMarketPosition struct {
	ID     string `json:"id"`
	Market struct {
		UniqueKey string `json:"uniqueKey"`
		LoanAsset struct {
			Address  string `json:"address"`
			Symbol   string `json:"symbol"`
			Decimals int    `json:"decimals"`
			Chain    struct {
				ID      int64  `json:"id"`
				Network string `json:"network"`
			} `json:"chain"`
		} `json:"loanAsset"`
		CollateralAsset *struct {
			Address  string `json:"address"`
			Symbol   string `json:"symbol"`
			Decimals int    `json:"decimals"`
		} `json:"collateralAsset"`
		State *struct {
			SupplyAPY float64 `json:"supplyApy"`
			BorrowAPY float64 `json:"borrowApy"`
		} `json:"state"`
	} `json:"market"`
	State *struct {
		SupplyAssets    bigintString `json:"supplyAssets"`
		SupplyAssetsUSD float64      `json:"supplyAssetsUsd"`
		BorrowAssets    bigintString `json:"borrowAssets"`
		BorrowAssetsUSD float64      `json:"borrowAssetsUsd"`
		Collateral      bigintString `json:"collateral"`
		CollateralUSD   float64      `json:"collateralUsd"`
	} `json:"state"`
}

type morphoVault struct {
	Address string `json:"address"`
	Name    string `json:"name"`
	Symbol  string `json:"symbol"`
	Asset   *struct {
		Address string `json:"address"`
		Symbol  string `json:"symbol"`
	} `json:"asset"`
	State *struct {
		NetAPY         float64            `json:"netApy"`
		TotalAssetsUSD float64            `json:"totalAssetsUsd"`
		Allocation     []marketAllocation `json:"allocation"`
	} `json:"state"`
	Liquidity *struct {
		USD float64 `json:"usd"`
	} `json:"liquidity"`
}

type morphoVaultV2 struct {
	Address      string  `json:"address"`
	Name         string  `json:"name"`
	Symbol       string  `json:"symbol"`
	NetAPY       float64 `json:"netApy"`
	TotalAssets  float64 `json:"totalAssetsUsd"`
	LiquidityUSD float64 `json:"liquidityUsd"`
	Asset        *struct {
		Address string `json:"address"`
		Symbol  string `json:"symbol"`
	} `json:"asset"`
	LiquidityData *struct {
		TypeName string `json:"__typename"`
		Market   *struct {
			CollateralAsset *struct {
				Address string `json:"address"`
				Symbol  string `json:"symbol"`
			} `json:"collateralAsset"`
		} `json:"market"`
		MetaMorpho *struct {
			State *struct {
				Allocation []marketAllocation `json:"allocation"`
			} `json:"state"`
		} `json:"metaMorpho"`
	} `json:"liquidityData"`
}

type marketAllocation struct {
	SupplyAssetsUSD float64 `json:"supplyAssetsUsd"`
	Market          *struct {
		CollateralAsset *struct {
			Address string `json:"address"`
			Symbol  string `json:"symbol"`
		} `json:"collateralAsset"`
	} `json:"market"`
}

type vaultYieldCandidate struct {
	Address          string
	AssetAddress     string
	NetAPYPercent    float64
	TotalAssetsUSD   float64
	LiquidityUSD     float64
	CollateralShares []collateralShare
}

type collateralShare struct {
	Symbol string
	USD    float64
}

func (c *Client) LendMarkets(ctx context.Context, provider string, chain id.Chain, asset id.Asset) ([]model.LendMarket, error) {
	if !strings.EqualFold(provider, "morpho") {
		return nil, clierr.New(clierr.CodeUnsupported, "morpho adapter supports only provider=morpho")
	}
	markets, err := c.fetchMarkets(ctx, chain, asset)
	if err != nil {
		return nil, err
	}

	out := make([]model.LendMarket, 0, len(markets))
	for _, m := range markets {
		tvl := yieldutil.PositiveFirst(m.State.SupplyAssetsUSD, m.State.TotalLiquidityUSD, m.State.LiquidityAssetsUSD)
		if tvl <= 0 {
			continue
		}
		supplyAPY := m.State.SupplyAPY * 100
		borrowAPY := m.State.BorrowAPY * 100
		out = append(out, model.LendMarket{
			Protocol:             "morpho",
			Provider:             "morpho",
			ChainID:              chain.CAIP2,
			AssetID:              canonicalAssetID(asset, m.LoanAsset.Address),
			ProviderNativeID:     strings.TrimSpace(m.UniqueKey),
			ProviderNativeIDKind: model.NativeIDKindMarketID,
			SupplyAPY:            supplyAPY,
			BorrowAPY:            borrowAPY,
			TVLUSD:               tvl,
			LiquidityUSD:         yieldutil.PositiveFirst(m.State.LiquidityAssetsUSD, m.State.TotalLiquidityUSD, tvl),
			SourceURL:            "https://app.morpho.org",
			FetchedAt:            c.now().UTC().Format(time.RFC3339),
		})
	}

	sort.Slice(out, func(i, j int) bool {
		if out[i].TVLUSD != out[j].TVLUSD {
			return out[i].TVLUSD > out[j].TVLUSD
		}
		return out[i].AssetID < out[j].AssetID
	})
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "no morpho lending market for requested chain/asset")
	}
	return out, nil
}

func (c *Client) LendRates(ctx context.Context, provider string, chain id.Chain, asset id.Asset) ([]model.LendRate, error) {
	if !strings.EqualFold(provider, "morpho") {
		return nil, clierr.New(clierr.CodeUnsupported, "morpho adapter supports only provider=morpho")
	}
	markets, err := c.fetchMarkets(ctx, chain, asset)
	if err != nil {
		return nil, err
	}

	out := make([]model.LendRate, 0, len(markets))
	for _, m := range markets {
		out = append(out, model.LendRate{
			Protocol:             "morpho",
			Provider:             "morpho",
			ChainID:              chain.CAIP2,
			AssetID:              canonicalAssetID(asset, m.LoanAsset.Address),
			ProviderNativeID:     strings.TrimSpace(m.UniqueKey),
			ProviderNativeIDKind: model.NativeIDKindMarketID,
			SupplyAPY:            m.State.SupplyAPY * 100,
			BorrowAPY:            m.State.BorrowAPY * 100,
			Utilization:          m.State.Utilization,
			SourceURL:            "https://app.morpho.org",
			FetchedAt:            c.now().UTC().Format(time.RFC3339),
		})
	}

	sort.Slice(out, func(i, j int) bool {
		if out[i].SupplyAPY != out[j].SupplyAPY {
			return out[i].SupplyAPY > out[j].SupplyAPY
		}
		return out[i].AssetID < out[j].AssetID
	})
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "no morpho lending rates for requested chain/asset")
	}
	return out, nil
}

func (c *Client) LendPositions(ctx context.Context, req providers.LendPositionsRequest) ([]model.LendPosition, error) {
	if !req.Chain.IsEVM() {
		return nil, clierr.New(clierr.CodeUnsupported, "morpho supports only EVM chains")
	}
	account := normalizeEVMAddress(req.Account)
	if account == "" {
		return nil, clierr.New(clierr.CodeUsage, "morpho positions requires a valid EVM account address")
	}
	filterType := req.PositionType
	if filterType == "" {
		filterType = providers.LendPositionTypeAll
	}

	first := req.Limit
	if first <= 0 {
		first = 200
	} else if first < 50 {
		first = 50
	}
	body, err := json.Marshal(map[string]any{
		"query": positionsQuery,
		"variables": map[string]any{
			"first":          first,
			"orderBy":        "SupplyShares",
			"orderDirection": "Desc",
			"where": map[string]any{
				"userAddress_in": []string{account},
				"chainId_in":     []int64{req.Chain.EVMChainID},
				"marketListed":   true,
			},
		},
	})
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "marshal morpho positions query", err)
	}

	var resp positionsResponse
	if _, err := httpx.DoBodyJSON(ctx, c.http, http.MethodPost, c.endpoint, body, nil, &resp); err != nil {
		return nil, err
	}
	if len(resp.Errors) > 0 {
		return nil, clierr.New(clierr.CodeUnavailable, fmt.Sprintf("morpho graphql error: %s", resp.Errors[0].Message))
	}

	out := make([]model.LendPosition, 0, len(resp.Data.MarketPositions.Items)*2)
	for _, item := range resp.Data.MarketPositions.Items {
		if item.State == nil {
			continue
		}

		loanAssetID := canonicalAssetIDForChain(req.Chain.CAIP2, item.Market.LoanAsset.Address)
		if loanAssetID != "" {
			if matchesPositionType(filterType, providers.LendPositionTypeSupply) &&
				matchesPositionAsset(item.Market.LoanAsset.Address, item.Market.LoanAsset.Symbol, req.Asset) {
				base := item.State.SupplyAssets.normalized()
				if base != "0" {
					supplyAPY := 0.0
					if item.Market.State != nil {
						supplyAPY = item.Market.State.SupplyAPY * 100
					}
					out = append(out, model.LendPosition{
						Protocol:             "morpho",
						Provider:             "morpho",
						ChainID:              req.Chain.CAIP2,
						AccountAddress:       account,
						PositionType:         string(providers.LendPositionTypeSupply),
						AssetID:              loanAssetID,
						ProviderNativeID:     strings.TrimSpace(item.Market.UniqueKey),
						ProviderNativeIDKind: model.NativeIDKindMarketID,
						Amount:               amountInfoFromBase(base, item.Market.LoanAsset.Decimals),
						AmountUSD:            item.State.SupplyAssetsUSD,
						APY:                  supplyAPY,
						SourceURL:            "https://app.morpho.org",
						FetchedAt:            c.now().UTC().Format(time.RFC3339),
					})
				}
			}

			if matchesPositionType(filterType, providers.LendPositionTypeBorrow) &&
				matchesPositionAsset(item.Market.LoanAsset.Address, item.Market.LoanAsset.Symbol, req.Asset) {
				base := item.State.BorrowAssets.normalized()
				if base != "0" {
					borrowAPY := 0.0
					if item.Market.State != nil {
						borrowAPY = item.Market.State.BorrowAPY * 100
					}
					out = append(out, model.LendPosition{
						Protocol:             "morpho",
						Provider:             "morpho",
						ChainID:              req.Chain.CAIP2,
						AccountAddress:       account,
						PositionType:         string(providers.LendPositionTypeBorrow),
						AssetID:              loanAssetID,
						ProviderNativeID:     strings.TrimSpace(item.Market.UniqueKey),
						ProviderNativeIDKind: model.NativeIDKindMarketID,
						Amount:               amountInfoFromBase(base, item.Market.LoanAsset.Decimals),
						AmountUSD:            item.State.BorrowAssetsUSD,
						APY:                  borrowAPY,
						SourceURL:            "https://app.morpho.org",
						FetchedAt:            c.now().UTC().Format(time.RFC3339),
					})
				}
			}
		}

		if item.Market.CollateralAsset != nil &&
			matchesPositionType(filterType, providers.LendPositionTypeCollateral) &&
			matchesPositionAsset(item.Market.CollateralAsset.Address, item.Market.CollateralAsset.Symbol, req.Asset) {
			base := item.State.Collateral.normalized()
			collateralAssetID := canonicalAssetIDForChain(req.Chain.CAIP2, item.Market.CollateralAsset.Address)
			if base != "0" && collateralAssetID != "" {
				out = append(out, model.LendPosition{
					Protocol:             "morpho",
					Provider:             "morpho",
					ChainID:              req.Chain.CAIP2,
					AccountAddress:       account,
					PositionType:         string(providers.LendPositionTypeCollateral),
					AssetID:              collateralAssetID,
					ProviderNativeID:     strings.TrimSpace(item.Market.UniqueKey),
					ProviderNativeIDKind: model.NativeIDKindMarketID,
					Amount:               amountInfoFromBase(base, item.Market.CollateralAsset.Decimals),
					AmountUSD:            item.State.CollateralUSD,
					APY:                  0,
					SourceURL:            "https://app.morpho.org",
					FetchedAt:            c.now().UTC().Format(time.RFC3339),
				})
			}
		}
	}

	sortLendPositions(out)
	if req.Limit > 0 && len(out) > req.Limit {
		out = out[:req.Limit]
	}
	return out, nil
}

func (c *Client) YieldOpportunities(ctx context.Context, req providers.YieldRequest) ([]model.YieldOpportunity, error) {
	vaults, err := c.fetchYieldVaultCandidates(ctx, req.Chain, req.Asset)
	if err != nil {
		return nil, err
	}
	maxRisk := yieldutil.RiskOrder(req.MaxRisk)
	if maxRisk == 0 {
		maxRisk = yieldutil.RiskOrder("high")
	}

	out := make([]model.YieldOpportunity, 0, len(vaults))
	for _, vault := range vaults {
		apy := vault.NetAPYPercent
		tvl := vault.TotalAssetsUSD
		if (apy == 0 || tvl == 0) && !req.IncludeIncomplete {
			continue
		}
		if apy < req.MinAPY || tvl < req.MinTVLUSD {
			continue
		}

		riskLevel, reasons := riskFromCollateralShares(vault.CollateralShares)
		if yieldutil.RiskOrder(riskLevel) > maxRisk {
			continue
		}
		liq := yieldutil.PositiveFirst(vault.LiquidityUSD, tvl)
		assetID := canonicalAssetID(req.Asset, vault.AssetAddress)
		vaultAddress := normalizeEVMAddress(vault.Address)
		if vaultAddress == "" {
			continue
		}
		out = append(out, model.YieldOpportunity{
			OpportunityID:        hashOpportunity("morpho", req.Chain.CAIP2, vaultAddress, assetID),
			Provider:             "morpho",
			Protocol:             "morpho",
			ChainID:              req.Chain.CAIP2,
			AssetID:              assetID,
			ProviderNativeID:     vaultAddress,
			ProviderNativeIDKind: model.NativeIDKindVaultAddress,
			Type:                 "lend",
			APYBase:              apy,
			APYReward:            0,
			APYTotal:             apy,
			TVLUSD:               tvl,
			LiquidityUSD:         liq,
			LockupDays:           0,
			WithdrawalTerms:      "variable",
			RiskLevel:            riskLevel,
			RiskReasons:          reasons,
			Score:                yieldutil.ScoreOpportunity(apy, tvl, liq, riskLevel),
			SourceURL:            sourceURLForVault(vaultAddress),
			FetchedAt:            c.now().UTC().Format(time.RFC3339),
		})
	}

	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "no morpho yield opportunities for requested chain/asset")
	}
	yieldutil.Sort(out, req.SortBy)
	if req.Limit <= 0 || req.Limit > len(out) {
		req.Limit = len(out)
	}
	return out[:req.Limit], nil
}

func (c *Client) fetchYieldVaultCandidates(ctx context.Context, chain id.Chain, asset id.Asset) ([]vaultYieldCandidate, error) {
	if !chain.IsEVM() {
		return nil, clierr.New(clierr.CodeUnsupported, "morpho supports only EVM chains")
	}

	vaults, err := c.fetchVaults(ctx, chain, asset)
	if err != nil {
		return nil, err
	}
	vaultV2s, err := c.fetchVaultV2s(ctx, chain)
	if err != nil {
		return nil, err
	}

	out := make([]vaultYieldCandidate, 0, len(vaults)+len(vaultV2s))
	for _, vault := range vaults {
		assetAddress := ""
		assetSymbol := ""
		if vault.Asset != nil {
			assetAddress = vault.Asset.Address
			assetSymbol = vault.Asset.Symbol
		}
		if !matchesVaultAsset(assetAddress, assetSymbol, asset) {
			continue
		}
		netAPY := 0.0
		tvl := 0.0
		if vault.State != nil {
			netAPY = vault.State.NetAPY * 100
			tvl = vault.State.TotalAssetsUSD
		}
		liquidity := 0.0
		if vault.Liquidity != nil {
			liquidity = vault.Liquidity.USD
		}
		out = append(out, vaultYieldCandidate{
			Address:          vault.Address,
			AssetAddress:     assetAddress,
			NetAPYPercent:    netAPY,
			TotalAssetsUSD:   tvl,
			LiquidityUSD:     liquidity,
			CollateralShares: collateralSharesFromAllocation(0, nil, allocationFromVault(vault)),
		})
	}
	for _, vault := range vaultV2s {
		assetAddress := ""
		assetSymbol := ""
		if vault.Asset != nil {
			assetAddress = vault.Asset.Address
			assetSymbol = vault.Asset.Symbol
		}
		if !matchesVaultAsset(assetAddress, assetSymbol, asset) {
			continue
		}
		out = append(out, vaultYieldCandidate{
			Address:          vault.Address,
			AssetAddress:     assetAddress,
			NetAPYPercent:    vault.NetAPY * 100,
			TotalAssetsUSD:   vault.TotalAssets,
			LiquidityUSD:     vault.LiquidityUSD,
			CollateralShares: collateralSharesFromVaultV2(vault),
		})
	}
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "morpho has no yield vault for requested chain/asset")
	}
	return out, nil
}

func (c *Client) fetchMarkets(ctx context.Context, chain id.Chain, asset id.Asset) ([]morphoMarket, error) {
	if !chain.IsEVM() {
		return nil, clierr.New(clierr.CodeUnsupported, "morpho supports only EVM chains")
	}
	where := map[string]any{
		"chainId_in": []int64{chain.EVMChainID},
		"listed":     true,
	}
	if addr := strings.TrimSpace(asset.Address); addr != "" {
		where["loanAssetAddress_in"] = []string{strings.ToLower(addr)}
	}
	body, err := json.Marshal(map[string]any{
		"query": marketsQuery,
		"variables": map[string]any{
			"first":          100,
			"orderBy":        "SupplyAssetsUsd",
			"orderDirection": "Desc",
			"where":          where,
		},
	})
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "marshal morpho query", err)
	}

	var resp marketsResponse
	if _, err := httpx.DoBodyJSON(ctx, c.http, http.MethodPost, c.endpoint, body, nil, &resp); err != nil {
		return nil, err
	}
	if len(resp.Errors) > 0 {
		return nil, clierr.New(clierr.CodeUnavailable, fmt.Sprintf("morpho graphql error: %s", resp.Errors[0].Message))
	}
	if len(resp.Data.Markets.Items) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "morpho has no market for requested chain/asset")
	}
	return resp.Data.Markets.Items, nil
}

func (c *Client) fetchVaults(ctx context.Context, chain id.Chain, asset id.Asset) ([]morphoVault, error) {
	where := map[string]any{
		"chainId_in": []int64{chain.EVMChainID},
		"listed":     true,
	}
	if addr := normalizeEVMAddress(asset.Address); addr != "" {
		where["assetAddress_in"] = []string{addr}
	} else if symbol := strings.TrimSpace(asset.Symbol); symbol != "" {
		where["assetSymbol_in"] = []string{symbol}
	}

	out := make([]morphoVault, 0, yieldVaultPageSize)
	for page := 0; page < yieldVaultMaxPages; page++ {
		body, err := json.Marshal(map[string]any{
			"query": vaultsYieldQuery,
			"variables": map[string]any{
				"first": yieldVaultPageSize,
				"skip":  page * yieldVaultPageSize,
				"where": where,
			},
		})
		if err != nil {
			return nil, clierr.Wrap(clierr.CodeInternal, "marshal morpho vault query", err)
		}

		var resp vaultsResponse
		if _, err := httpx.DoBodyJSON(ctx, c.http, http.MethodPost, c.endpoint, body, nil, &resp); err != nil {
			return nil, err
		}
		if len(resp.Errors) > 0 {
			return nil, clierr.New(clierr.CodeUnavailable, fmt.Sprintf("morpho graphql error: %s", resp.Errors[0].Message))
		}
		out = append(out, resp.Data.Vaults.Items...)
		if len(resp.Data.Vaults.Items) < yieldVaultPageSize {
			break
		}
	}

	return out, nil
}

func (c *Client) fetchVaultV2s(ctx context.Context, chain id.Chain) ([]morphoVaultV2, error) {
	where := map[string]any{
		"chainId_in": []int64{chain.EVMChainID},
		"listed":     true,
	}

	out := make([]morphoVaultV2, 0, yieldVaultPageSize)
	for page := 0; page < yieldVaultMaxPages; page++ {
		body, err := json.Marshal(map[string]any{
			"query": vaultV2sYieldQuery,
			"variables": map[string]any{
				"first": yieldVaultPageSize,
				"skip":  page * yieldVaultPageSize,
				"where": where,
			},
		})
		if err != nil {
			return nil, clierr.Wrap(clierr.CodeInternal, "marshal morpho vault-v2 query", err)
		}

		var resp vaultV2sResponse
		if _, err := httpx.DoBodyJSON(ctx, c.http, http.MethodPost, c.endpoint, body, nil, &resp); err != nil {
			return nil, err
		}
		if len(resp.Errors) > 0 {
			return nil, clierr.New(clierr.CodeUnavailable, fmt.Sprintf("morpho graphql error: %s", resp.Errors[0].Message))
		}
		out = append(out, resp.Data.VaultV2s.Items...)
		if len(resp.Data.VaultV2s.Items) < yieldVaultPageSize {
			break
		}
	}

	return out, nil
}

func matchesVaultAsset(vaultAssetAddress, vaultAssetSymbol string, asset id.Asset) bool {
	if addr := normalizeEVMAddress(asset.Address); addr != "" {
		return strings.EqualFold(normalizeEVMAddress(vaultAssetAddress), addr)
	}
	if symbol := strings.TrimSpace(asset.Symbol); symbol != "" {
		return strings.EqualFold(strings.TrimSpace(vaultAssetSymbol), symbol)
	}
	return true
}

func allocationFromVault(vault morphoVault) []marketAllocation {
	if vault.State == nil {
		return nil
	}
	return vault.State.Allocation
}

func collateralSharesFromVaultV2(vault morphoVaultV2) []collateralShare {
	if vault.LiquidityData == nil {
		if usd := yieldutil.PositiveFirst(vault.TotalAssets, vault.LiquidityUSD); usd > 0 {
			return []collateralShare{{USD: usd}}
		}
		return nil
	}

	switch vault.LiquidityData.TypeName {
	case "MarketV1LiquidityData":
		symbol := ""
		if vault.LiquidityData.Market != nil && vault.LiquidityData.Market.CollateralAsset != nil {
			symbol = vault.LiquidityData.Market.CollateralAsset.Symbol
		}
		usd := yieldutil.PositiveFirst(vault.TotalAssets, vault.LiquidityUSD)
		if usd <= 0 {
			return nil
		}
		return []collateralShare{{Symbol: symbol, USD: usd}}
	case "MetaMorphoLiquidityData":
		if vault.LiquidityData.MetaMorpho != nil && vault.LiquidityData.MetaMorpho.State != nil {
			shares := collateralSharesFromAllocation(vault.TotalAssets, nil, vault.LiquidityData.MetaMorpho.State.Allocation)
			if len(shares) > 0 {
				return shares
			}
		}
	}

	if usd := yieldutil.PositiveFirst(vault.TotalAssets, vault.LiquidityUSD); usd > 0 {
		return []collateralShare{{USD: usd}}
	}
	return nil
}

func collateralSharesFromAllocation(totalOverride float64, shares []collateralShare, allocation []marketAllocation) []collateralShare {
	total := 0.0
	for _, item := range allocation {
		if item.SupplyAssetsUSD > 0 {
			total += item.SupplyAssetsUSD
		}
	}
	for _, item := range allocation {
		if item.SupplyAssetsUSD <= 0 {
			continue
		}
		usd := item.SupplyAssetsUSD
		if totalOverride > 0 && total > 0 {
			usd = totalOverride * item.SupplyAssetsUSD / total
		}
		symbol := ""
		if item.Market != nil && item.Market.CollateralAsset != nil {
			symbol = item.Market.CollateralAsset.Symbol
		}
		shares = append(shares, collateralShare{Symbol: symbol, USD: usd})
	}
	return shares
}

var stableCollateralSymbols = map[string]struct{}{
	"USDC": {},
	"USDT": {},
	"DAI":  {},
	"USDE": {},
}

func riskFromCollateralShares(shares []collateralShare) (string, []string) {
	hasNonStable := false
	hasMissing := false
	hasKnownStable := false

	for _, share := range shares {
		if share.USD <= 0 {
			continue
		}
		symbol := strings.ToUpper(strings.TrimSpace(share.Symbol))
		if symbol == "" {
			hasMissing = true
			continue
		}
		if _, ok := stableCollateralSymbols[symbol]; ok {
			hasKnownStable = true
			continue
		}
		hasNonStable = true
	}

	switch {
	case hasNonStable:
		return "medium", []string{"non-stable collateral"}
	case hasMissing:
		return "medium", []string{"missing collateral metadata"}
	case hasKnownStable:
		return "low", []string{"stable collateral"}
	default:
		return "medium", []string{"missing collateral metadata"}
	}
}

func sourceURLForVault(address string) string {
	addr := normalizeEVMAddress(address)
	if addr == "" {
		return "https://app.morpho.org"
	}
	return fmt.Sprintf("https://app.morpho.org/vault/%s", addr)
}

func canonicalAssetID(asset id.Asset, address string) string {
	addr := strings.ToLower(strings.TrimSpace(address))
	if addr == "" {
		return asset.AssetID
	}
	return fmt.Sprintf("%s/erc20:%s", asset.ChainID, addr)
}

func canonicalAssetIDForChain(chainID, address string) string {
	addr := normalizeEVMAddress(address)
	if chainID == "" || addr == "" {
		return ""
	}
	return fmt.Sprintf("%s/erc20:%s", chainID, addr)
}

func hashOpportunity(provider, chainID, marketID, assetID string) string {
	seed := strings.Join([]string{provider, chainID, marketID, assetID}, "|")
	h := sha1.Sum([]byte(seed))
	return hex.EncodeToString(h[:])
}

type bigintString string

func (b *bigintString) UnmarshalJSON(data []byte) error {
	raw := strings.TrimSpace(string(data))
	if raw == "" || raw == "null" {
		*b = "0"
		return nil
	}
	if strings.HasPrefix(raw, "\"") {
		var s string
		if err := json.Unmarshal(data, &s); err != nil {
			return err
		}
		*b = bigintString(strings.TrimSpace(s))
		return nil
	}
	*b = bigintString(raw)
	return nil
}

func (b bigintString) normalized() string {
	raw := strings.TrimSpace(string(b))
	if raw == "" {
		return "0"
	}
	n, ok := new(big.Int).SetString(raw, 10)
	if !ok || n.Sign() <= 0 {
		return "0"
	}
	return n.String()
}

func normalizeEVMAddress(address string) string {
	addr := strings.ToLower(strings.TrimSpace(address))
	if len(addr) != 42 || !strings.HasPrefix(addr, "0x") {
		return ""
	}
	return addr
}

func matchesPositionType(filter, position providers.LendPositionType) bool {
	if filter == "" || filter == providers.LendPositionTypeAll {
		return true
	}
	return filter == position
}

func matchesPositionAsset(address, symbol string, asset id.Asset) bool {
	if strings.TrimSpace(asset.Address) != "" {
		return strings.EqualFold(strings.TrimSpace(address), strings.TrimSpace(asset.Address))
	}
	if strings.TrimSpace(asset.Symbol) != "" {
		return strings.EqualFold(strings.TrimSpace(symbol), strings.TrimSpace(asset.Symbol))
	}
	return true
}

func amountInfoFromBase(base string, decimals int) model.AmountInfo {
	if decimals < 0 {
		decimals = 0
	}
	return model.AmountInfo{
		AmountBaseUnits: base,
		AmountDecimal:   id.FormatDecimalCompat(base, decimals),
		Decimals:        decimals,
	}
}

func sortLendPositions(items []model.LendPosition) {
	sort.Slice(items, func(i, j int) bool {
		if items[i].AmountUSD != items[j].AmountUSD {
			return items[i].AmountUSD > items[j].AmountUSD
		}
		if items[i].PositionType != items[j].PositionType {
			return items[i].PositionType < items[j].PositionType
		}
		if items[i].AssetID != items[j].AssetID {
			return items[i].AssetID < items[j].AssetID
		}
		return items[i].ProviderNativeID < items[j].ProviderNativeID
	})
}
