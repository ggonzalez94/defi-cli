package morpho

import (
	"context"
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math"
	"net/http"
	"sort"
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

const defaultEndpoint = "https://api.morpho.org/graphql"

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
			"yield.opportunities",
		},
	}
}

const marketsQuery = `query Markets($first:Int,$where:MarketFilters,$orderBy:MarketOrderBy,$orderDirection:OrderDirection){
  markets(first:$first, where:$where, orderBy:$orderBy, orderDirection:$orderDirection){
    items{
      uniqueKey
      loanAsset{ address symbol decimals chain{ id network } }
      collateralAsset{ address symbol }
      state{ supplyApy borrowApy utilization supplyAssetsUsd liquidityAssetsUsd totalLiquidityUsd }
    }
  }
}`

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

type morphoMarket struct {
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

func (c *Client) LendMarkets(ctx context.Context, protocol string, chain id.Chain, asset id.Asset) ([]model.LendMarket, error) {
	if !strings.EqualFold(protocol, "morpho") {
		return nil, clierr.New(clierr.CodeUnsupported, "morpho adapter supports only protocol=morpho")
	}
	markets, err := c.fetchMarkets(ctx, chain, asset)
	if err != nil {
		return nil, err
	}

	out := make([]model.LendMarket, 0, len(markets))
	for _, m := range markets {
		tvl := positiveFirst(m.State.SupplyAssetsUSD, m.State.TotalLiquidityUSD, m.State.LiquidityAssetsUSD)
		if tvl <= 0 {
			continue
		}
		supplyAPY := m.State.SupplyAPY * 100
		borrowAPY := m.State.BorrowAPY * 100
		out = append(out, model.LendMarket{
			Protocol:     "morpho",
			ChainID:      chain.CAIP2,
			AssetID:      canonicalAssetID(asset, m.LoanAsset.Address),
			SupplyAPY:    supplyAPY,
			BorrowAPY:    borrowAPY,
			TVLUSD:       tvl,
			LiquidityUSD: positiveFirst(m.State.LiquidityAssetsUSD, m.State.TotalLiquidityUSD, tvl),
			SourceURL:    "https://app.morpho.org",
			FetchedAt:    c.now().UTC().Format(time.RFC3339),
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

func (c *Client) LendRates(ctx context.Context, protocol string, chain id.Chain, asset id.Asset) ([]model.LendRate, error) {
	if !strings.EqualFold(protocol, "morpho") {
		return nil, clierr.New(clierr.CodeUnsupported, "morpho adapter supports only protocol=morpho")
	}
	markets, err := c.fetchMarkets(ctx, chain, asset)
	if err != nil {
		return nil, err
	}

	out := make([]model.LendRate, 0, len(markets))
	for _, m := range markets {
		out = append(out, model.LendRate{
			Protocol:    "morpho",
			ChainID:     chain.CAIP2,
			AssetID:     canonicalAssetID(asset, m.LoanAsset.Address),
			SupplyAPY:   m.State.SupplyAPY * 100,
			BorrowAPY:   m.State.BorrowAPY * 100,
			Utilization: m.State.Utilization,
			SourceURL:   "https://app.morpho.org",
			FetchedAt:   c.now().UTC().Format(time.RFC3339),
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

func (c *Client) YieldOpportunities(ctx context.Context, req providers.YieldRequest) ([]model.YieldOpportunity, error) {
	markets, err := c.fetchMarkets(ctx, req.Chain, req.Asset)
	if err != nil {
		return nil, err
	}
	maxRisk := riskOrder(req.MaxRisk)
	if maxRisk == 0 {
		maxRisk = riskOrder("high")
	}

	out := make([]model.YieldOpportunity, 0, len(markets))
	for _, m := range markets {
		apy := m.State.SupplyAPY * 100
		tvl := positiveFirst(m.State.SupplyAssetsUSD, m.State.TotalLiquidityUSD, m.State.LiquidityAssetsUSD)
		if (apy == 0 || tvl == 0) && !req.IncludeIncomplete {
			continue
		}
		if apy < req.MinAPY || tvl < req.MinTVLUSD {
			continue
		}

		riskLevel, reasons := riskFromCollateral(m.CollateralAsset)
		if riskOrder(riskLevel) > maxRisk {
			continue
		}
		liq := positiveFirst(m.State.LiquidityAssetsUSD, m.State.TotalLiquidityUSD, tvl)
		assetID := canonicalAssetID(req.Asset, m.LoanAsset.Address)
		out = append(out, model.YieldOpportunity{
			OpportunityID:   hashOpportunity("morpho", req.Chain.CAIP2, m.UniqueKey, assetID),
			Provider:        "morpho",
			Protocol:        "morpho",
			ChainID:         req.Chain.CAIP2,
			AssetID:         assetID,
			Type:            "lend",
			APYBase:         apy,
			APYReward:       0,
			APYTotal:        apy,
			TVLUSD:          tvl,
			LiquidityUSD:    liq,
			LockupDays:      0,
			WithdrawalTerms: "variable",
			RiskLevel:       riskLevel,
			RiskReasons:     reasons,
			Score:           scoreOpportunity(apy, tvl, liq, riskLevel),
			SourceURL:       "https://app.morpho.org",
			FetchedAt:       c.now().UTC().Format(time.RFC3339),
		})
	}

	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "no morpho yield opportunities for requested chain/asset")
	}
	sortYield(out, req.SortBy)
	if req.Limit <= 0 || req.Limit > len(out) {
		req.Limit = len(out)
	}
	return out[:req.Limit], nil
}

func (c *Client) fetchMarkets(ctx context.Context, chain id.Chain, asset id.Asset) ([]morphoMarket, error) {
	body, err := json.Marshal(map[string]any{
		"query": marketsQuery,
		"variables": map[string]any{
			"first":          100,
			"orderBy":        "SupplyAssetsUsd",
			"orderDirection": "Desc",
			"where": map[string]any{
				"chainId_in":          []int64{chain.EVMChainID},
				"listed":              true,
				"loanAssetAddress_in": []string{strings.ToLower(asset.Address)},
			},
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

func canonicalAssetID(asset id.Asset, address string) string {
	addr := strings.ToLower(strings.TrimSpace(address))
	if addr == "" {
		return asset.AssetID
	}
	return fmt.Sprintf("%s/erc20:%s", asset.ChainID, addr)
}

func positiveFirst(values ...float64) float64 {
	for _, v := range values {
		if v > 0 && !math.IsNaN(v) && !math.IsInf(v, 0) {
			return v
		}
	}
	return 0
}

func riskFromCollateral(collateral *struct {
	Address string `json:"address"`
	Symbol  string `json:"symbol"`
}) (string, []string) {
	if collateral == nil {
		return "medium", []string{"missing collateral metadata"}
	}
	s := strings.ToUpper(strings.TrimSpace(collateral.Symbol))
	switch s {
	case "USDC", "USDT", "DAI", "USDE":
		return "low", []string{"stable collateral"}
	default:
		return "medium", []string{"non-stable collateral"}
	}
}

func riskOrder(v string) int {
	switch strings.ToLower(strings.TrimSpace(v)) {
	case "low":
		return 1
	case "medium":
		return 2
	case "high":
		return 3
	case "unknown":
		return 4
	default:
		return 0
	}
}

func scoreOpportunity(apyTotal, tvlUSD, liquidityUSD float64, riskLevel string) float64 {
	apyNorm := clamp(apyTotal, 0, 100) / 100
	tvlNorm := clamp(math.Log10(tvlUSD+1)/10, 0, 1)
	liqNorm := 0.0
	if tvlUSD > 0 {
		liqNorm = clamp(liquidityUSD/math.Max(tvlUSD, 1), 0, 1)
	}

	riskPenalty := map[string]float64{
		"low":     0.10,
		"medium":  0.30,
		"high":    0.60,
		"unknown": 0.45,
	}[strings.ToLower(strings.TrimSpace(riskLevel))]

	scoreRaw := 0.45*apyNorm + 0.30*tvlNorm + 0.20*liqNorm - 0.25*riskPenalty
	return math.Round(clamp(scoreRaw, 0, 1)*100*100) / 100
}

func clamp(v, min, max float64) float64 {
	if v < min {
		return min
	}
	if v > max {
		return max
	}
	return v
}

func hashOpportunity(provider, chainID, marketID, assetID string) string {
	seed := strings.Join([]string{provider, chainID, marketID, assetID}, "|")
	h := sha1.Sum([]byte(seed))
	return hex.EncodeToString(h[:])
}

func sortYield(items []model.YieldOpportunity, sortBy string) {
	sortBy = strings.ToLower(strings.TrimSpace(sortBy))
	if sortBy == "" {
		sortBy = "score"
	}

	sort.Slice(items, func(i, j int) bool {
		a, b := items[i], items[j]
		switch sortBy {
		case "apy_total":
			if a.APYTotal != b.APYTotal {
				return a.APYTotal > b.APYTotal
			}
		case "tvl_usd":
			if a.TVLUSD != b.TVLUSD {
				return a.TVLUSD > b.TVLUSD
			}
		case "liquidity_usd":
			if a.LiquidityUSD != b.LiquidityUSD {
				return a.LiquidityUSD > b.LiquidityUSD
			}
		default:
			if a.Score != b.Score {
				return a.Score > b.Score
			}
		}
		if a.APYTotal != b.APYTotal {
			return a.APYTotal > b.APYTotal
		}
		if a.TVLUSD != b.TVLUSD {
			return a.TVLUSD > b.TVLUSD
		}
		return strings.Compare(a.OpportunityID, b.OpportunityID) < 0
	})
}
