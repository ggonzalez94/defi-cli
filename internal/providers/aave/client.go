package aave

import (
	"context"
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"time"

	clierr "github.com/gustavo/defi-cli/internal/errors"
	"github.com/gustavo/defi-cli/internal/httpx"
	"github.com/gustavo/defi-cli/internal/id"
	"github.com/gustavo/defi-cli/internal/model"
	"github.com/gustavo/defi-cli/internal/providers"
)

const defaultEndpoint = "https://api.v3.aave.com/graphql"

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
		Name:        "aave",
		Type:        "lending+yield",
		RequiresKey: false,
		Capabilities: []string{
			"lend.markets",
			"lend.rates",
			"yield.opportunities",
		},
	}
}

const marketsQuery = `query Markets($request: MarketsRequest!) {
  markets(request: $request) {
    name
    chain { chainId name }
    reserves {
      underlyingToken { address symbol decimals }
      size { usd }
      supplyInfo { apy { value } total { value } }
      borrowInfo { apy { value } total { usd } utilizationRate { value } }
    }
  }
}`

type marketsResponse struct {
	Data struct {
		Markets []aaveMarket `json:"markets"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type aaveMarket struct {
	Name  string `json:"name"`
	Chain struct {
		ChainID int64  `json:"chainId"`
		Name    string `json:"name"`
	} `json:"chain"`
	Reserves []aaveReserve `json:"reserves"`
}

type aaveReserve struct {
	UnderlyingToken struct {
		Address  string `json:"address"`
		Symbol   string `json:"symbol"`
		Decimals int    `json:"decimals"`
	} `json:"underlyingToken"`
	Size struct {
		USD string `json:"usd"`
	} `json:"size"`
	SupplyInfo struct {
		APY struct {
			Value string `json:"value"`
		} `json:"apy"`
		Total struct {
			Value string `json:"value"`
		} `json:"total"`
	} `json:"supplyInfo"`
	BorrowInfo *struct {
		APY struct {
			Value string `json:"value"`
		} `json:"apy"`
		Total struct {
			USD string `json:"usd"`
		} `json:"total"`
		UtilizationRate struct {
			Value string `json:"value"`
		} `json:"utilizationRate"`
	} `json:"borrowInfo"`
}

func (c *Client) LendMarkets(ctx context.Context, protocol string, chain id.Chain, asset id.Asset) ([]model.LendMarket, error) {
	if !strings.EqualFold(protocol, "aave") {
		return nil, clierr.New(clierr.CodeUnsupported, "aave adapter supports only protocol=aave")
	}
	markets, err := c.fetchMarkets(ctx, chain)
	if err != nil {
		return nil, err
	}

	out := make([]model.LendMarket, 0)
	for _, m := range markets {
		for _, r := range m.Reserves {
			if !matchesReserveAsset(r, asset) {
				continue
			}
			supplyAPY := parseFloat(r.SupplyInfo.APY.Value) * 100
			borrowAPY := 0.0
			if r.BorrowInfo != nil {
				borrowAPY = parseFloat(r.BorrowInfo.APY.Value) * 100
			}
			tvlUSD := parseFloat(r.Size.USD)
			if tvlUSD <= 0 {
				continue
			}

			out = append(out, model.LendMarket{
				Protocol:     "aave",
				ChainID:      chain.CAIP2,
				AssetID:      canonicalAssetID(asset, r.UnderlyingToken.Address),
				SupplyAPY:    supplyAPY,
				BorrowAPY:    borrowAPY,
				TVLUSD:       tvlUSD,
				LiquidityUSD: tvlUSD,
				SourceURL:    "https://app.aave.com",
				FetchedAt:    c.now().UTC().Format(time.RFC3339),
			})
		}
	}

	sort.Slice(out, func(i, j int) bool {
		if out[i].TVLUSD != out[j].TVLUSD {
			return out[i].TVLUSD > out[j].TVLUSD
		}
		return out[i].AssetID < out[j].AssetID
	})
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "no aave lending market for requested chain/asset")
	}
	return out, nil
}

func (c *Client) LendRates(ctx context.Context, protocol string, chain id.Chain, asset id.Asset) ([]model.LendRate, error) {
	if !strings.EqualFold(protocol, "aave") {
		return nil, clierr.New(clierr.CodeUnsupported, "aave adapter supports only protocol=aave")
	}
	markets, err := c.fetchMarkets(ctx, chain)
	if err != nil {
		return nil, err
	}

	out := make([]model.LendRate, 0)
	for _, m := range markets {
		for _, r := range m.Reserves {
			if !matchesReserveAsset(r, asset) {
				continue
			}
			supplyAPY := parseFloat(r.SupplyInfo.APY.Value) * 100
			borrowAPY := 0.0
			utilization := 0.0
			if r.BorrowInfo != nil {
				borrowAPY = parseFloat(r.BorrowInfo.APY.Value) * 100
				utilization = parseFloat(r.BorrowInfo.UtilizationRate.Value)
			}
			out = append(out, model.LendRate{
				Protocol:    "aave",
				ChainID:     chain.CAIP2,
				AssetID:     canonicalAssetID(asset, r.UnderlyingToken.Address),
				SupplyAPY:   supplyAPY,
				BorrowAPY:   borrowAPY,
				Utilization: utilization,
				SourceURL:   "https://app.aave.com",
				FetchedAt:   c.now().UTC().Format(time.RFC3339),
			})
		}
	}

	sort.Slice(out, func(i, j int) bool {
		if out[i].SupplyAPY != out[j].SupplyAPY {
			return out[i].SupplyAPY > out[j].SupplyAPY
		}
		return out[i].AssetID < out[j].AssetID
	})
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "no aave lending rates for requested chain/asset")
	}
	return out, nil
}

func (c *Client) YieldOpportunities(ctx context.Context, req providers.YieldRequest) ([]model.YieldOpportunity, error) {
	markets, err := c.fetchMarkets(ctx, req.Chain)
	if err != nil {
		return nil, err
	}

	maxRisk := riskOrder(req.MaxRisk)
	if maxRisk == 0 {
		maxRisk = riskOrder("high")
	}

	out := make([]model.YieldOpportunity, 0)
	for _, m := range markets {
		for _, r := range m.Reserves {
			if !matchesReserveAsset(r, req.Asset) {
				continue
			}
			apy := parseFloat(r.SupplyInfo.APY.Value) * 100
			tvl := parseFloat(r.Size.USD)
			if (apy == 0 || tvl == 0) && !req.IncludeIncomplete {
				continue
			}
			if apy < req.MinAPY {
				continue
			}
			if tvl < req.MinTVLUSD {
				continue
			}

			riskLevel, reasons := riskFromSymbol(r.UnderlyingToken.Symbol)
			if riskOrder(riskLevel) > maxRisk {
				continue
			}

			assetID := canonicalAssetID(req.Asset, r.UnderlyingToken.Address)
			opportunityID := hashOpportunity("aave", req.Chain.CAIP2, fmt.Sprintf("%s:%s", m.Name, strings.ToLower(r.UnderlyingToken.Address)), assetID)
			out = append(out, model.YieldOpportunity{
				OpportunityID:   opportunityID,
				Provider:        "aave",
				Protocol:        "aave",
				ChainID:         req.Chain.CAIP2,
				AssetID:         assetID,
				Type:            "lend",
				APYBase:         apy,
				APYReward:       0,
				APYTotal:        apy,
				TVLUSD:          tvl,
				LiquidityUSD:    tvl,
				LockupDays:      0,
				WithdrawalTerms: "variable",
				RiskLevel:       riskLevel,
				RiskReasons:     reasons,
				Score:           scoreOpportunity(apy, tvl, tvl, riskLevel),
				SourceURL:       "https://app.aave.com",
				FetchedAt:       c.now().UTC().Format(time.RFC3339),
			})
		}
	}

	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "no aave yield opportunities for requested chain/asset")
	}
	sortYield(out, req.SortBy)
	if req.Limit <= 0 || req.Limit > len(out) {
		req.Limit = len(out)
	}
	return out[:req.Limit], nil
}

func (c *Client) fetchMarkets(ctx context.Context, chain id.Chain) ([]aaveMarket, error) {
	body, err := json.Marshal(map[string]any{
		"query": marketsQuery,
		"variables": map[string]any{
			"request": map[string]any{
				"chainIds": []int64{chain.EVMChainID},
			},
		},
	})
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "marshal aave query", err)
	}

	var resp marketsResponse
	if _, err := httpx.DoBodyJSON(ctx, c.http, http.MethodPost, c.endpoint, body, nil, &resp); err != nil {
		return nil, err
	}
	if len(resp.Errors) > 0 {
		return nil, clierr.New(clierr.CodeUnavailable, fmt.Sprintf("aave graphql error: %s", resp.Errors[0].Message))
	}
	if len(resp.Data.Markets) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "aave has no market for requested chain")
	}
	return resp.Data.Markets, nil
}

func matchesReserveAsset(r aaveReserve, asset id.Asset) bool {
	if strings.EqualFold(strings.TrimSpace(r.UnderlyingToken.Address), strings.TrimSpace(asset.Address)) {
		return true
	}
	return strings.EqualFold(strings.TrimSpace(r.UnderlyingToken.Symbol), strings.TrimSpace(asset.Symbol))
}

func canonicalAssetID(asset id.Asset, address string) string {
	addr := strings.ToLower(strings.TrimSpace(address))
	if addr == "" {
		return asset.AssetID
	}
	return fmt.Sprintf("%s/erc20:%s", asset.ChainID, addr)
}

func parseFloat(v string) float64 {
	f, err := strconv.ParseFloat(strings.TrimSpace(v), 64)
	if err != nil {
		return 0
	}
	if math.IsNaN(f) || math.IsInf(f, 0) {
		return 0
	}
	return f
}

func riskFromSymbol(symbol string) (string, []string) {
	s := strings.ToUpper(strings.TrimSpace(symbol))
	switch s {
	case "USDC", "USDT", "DAI", "GHO":
		return "low", []string{"stablecoin asset"}
	default:
		return "medium", []string{"variable asset exposure"}
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
