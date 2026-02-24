package kamino

import (
	"context"
	"crypto/sha1"
	"encoding/hex"
	"fmt"
	"math"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/ggonzalez94/defi-cli/internal/providers/yieldutil"
)

const (
	defaultBase        = "https://api.kamino.finance"
	solanaMainnetCAIP2 = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"
	marketFetchWorkers = 6
)

type Client struct {
	http    *httpx.Client
	baseURL string
	now     func() time.Time
}

func New(httpClient *httpx.Client) *Client {
	return &Client{http: httpClient, baseURL: defaultBase, now: time.Now}
}

func (c *Client) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:        "kamino",
		Type:        "lending+yield",
		RequiresKey: false,
		Capabilities: []string{
			"lend.markets",
			"lend.rates",
			"yield.opportunities",
		},
	}
}

type marketInfo struct {
	LendingMarket string `json:"lendingMarket"`
	Name          string `json:"name"`
	IsPrimary     bool   `json:"isPrimary"`
	IsCurated     bool   `json:"isCurated"`
}

type reserveMetric struct {
	Reserve            string `json:"reserve"`
	LiquidityToken     string `json:"liquidityToken"`
	LiquidityTokenMint string `json:"liquidityTokenMint"`
	BorrowAPY          string `json:"borrowApy"`
	SupplyAPY          string `json:"supplyApy"`
	TotalSupplyUSD     string `json:"totalSupplyUsd"`
	TotalBorrowUSD     string `json:"totalBorrowUsd"`
}

type reserveWithMarket struct {
	Market  marketInfo
	Reserve reserveMetric
}

func (c *Client) LendMarkets(ctx context.Context, protocol string, chain id.Chain, asset id.Asset) ([]model.LendMarket, error) {
	if !strings.EqualFold(strings.TrimSpace(protocol), "kamino") {
		return nil, clierr.New(clierr.CodeUnsupported, "kamino adapter supports only protocol=kamino")
	}
	reserves, err := c.fetchReserves(ctx, chain)
	if err != nil {
		return nil, err
	}

	fetchedAt := c.now().UTC().Format(time.RFC3339)
	out := make([]model.LendMarket, 0, len(reserves))
	for _, item := range reserves {
		if !matchesReserveAsset(item.Reserve, asset) {
			continue
		}
		supplyUSD := parseNonNegative(item.Reserve.TotalSupplyUSD)
		borrowUSD := parseNonNegative(item.Reserve.TotalBorrowUSD)
		tvl := yieldutil.PositiveFirst(supplyUSD, borrowUSD)
		if tvl <= 0 {
			continue
		}
		liquidityUSD := supplyUSD - borrowUSD
		if liquidityUSD <= 0 {
			liquidityUSD = tvl
		}
		assetID := reserveAssetID(chain.CAIP2, asset.AssetID, item.Reserve.LiquidityTokenMint)
		out = append(out, model.LendMarket{
			Protocol:             "kamino",
			Provider:             "kamino",
			ChainID:              chain.CAIP2,
			AssetID:              assetID,
			ProviderNativeID:     strings.TrimSpace(item.Reserve.Reserve),
			ProviderNativeIDKind: model.NativeIDKindPoolID,
			SupplyAPY:            ratioToPercent(item.Reserve.SupplyAPY),
			BorrowAPY:            ratioToPercent(item.Reserve.BorrowAPY),
			TVLUSD:               tvl,
			LiquidityUSD:         liquidityUSD,
			SourceURL:            marketURL(item.Market.LendingMarket),
			FetchedAt:            fetchedAt,
		})
	}

	sort.Slice(out, func(i, j int) bool {
		if out[i].TVLUSD != out[j].TVLUSD {
			return out[i].TVLUSD > out[j].TVLUSD
		}
		return strings.Compare(out[i].AssetID, out[j].AssetID) < 0
	})
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "no kamino lending market for requested chain/asset")
	}
	return out, nil
}

func (c *Client) LendRates(ctx context.Context, protocol string, chain id.Chain, asset id.Asset) ([]model.LendRate, error) {
	if !strings.EqualFold(strings.TrimSpace(protocol), "kamino") {
		return nil, clierr.New(clierr.CodeUnsupported, "kamino adapter supports only protocol=kamino")
	}
	reserves, err := c.fetchReserves(ctx, chain)
	if err != nil {
		return nil, err
	}

	fetchedAt := c.now().UTC().Format(time.RFC3339)
	out := make([]model.LendRate, 0, len(reserves))
	for _, item := range reserves {
		if !matchesReserveAsset(item.Reserve, asset) {
			continue
		}
		supplyUSD := parseNonNegative(item.Reserve.TotalSupplyUSD)
		borrowUSD := parseNonNegative(item.Reserve.TotalBorrowUSD)
		utilization := 0.0
		if supplyUSD > 0 {
			utilization = borrowUSD / supplyUSD
		}
		assetID := reserveAssetID(chain.CAIP2, asset.AssetID, item.Reserve.LiquidityTokenMint)
		out = append(out, model.LendRate{
			Protocol:             "kamino",
			Provider:             "kamino",
			ChainID:              chain.CAIP2,
			AssetID:              assetID,
			ProviderNativeID:     strings.TrimSpace(item.Reserve.Reserve),
			ProviderNativeIDKind: model.NativeIDKindPoolID,
			SupplyAPY:            ratioToPercent(item.Reserve.SupplyAPY),
			BorrowAPY:            ratioToPercent(item.Reserve.BorrowAPY),
			Utilization:          math.Min(math.Max(utilization, 0), 1),
			SourceURL:            marketURL(item.Market.LendingMarket),
			FetchedAt:            fetchedAt,
		})
	}

	sort.Slice(out, func(i, j int) bool {
		if out[i].SupplyAPY != out[j].SupplyAPY {
			return out[i].SupplyAPY > out[j].SupplyAPY
		}
		return strings.Compare(out[i].AssetID, out[j].AssetID) < 0
	})
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "no kamino lending rates for requested chain/asset")
	}
	return out, nil
}

func (c *Client) YieldOpportunities(ctx context.Context, req providers.YieldRequest) ([]model.YieldOpportunity, error) {
	reserves, err := c.fetchReserves(ctx, req.Chain)
	if err != nil {
		return nil, err
	}

	maxRisk := yieldutil.RiskOrder(req.MaxRisk)
	if maxRisk == 0 {
		maxRisk = yieldutil.RiskOrder("high")
	}

	out := make([]model.YieldOpportunity, 0, len(reserves))
	fetchedAt := c.now().UTC().Format(time.RFC3339)
	for _, item := range reserves {
		if !matchesReserveAsset(item.Reserve, req.Asset) {
			continue
		}

		apy := ratioToPercent(item.Reserve.SupplyAPY)
		tvl := parseNonNegative(item.Reserve.TotalSupplyUSD)
		if (apy == 0 || tvl == 0) && !req.IncludeIncomplete {
			continue
		}
		if apy < req.MinAPY {
			continue
		}
		if tvl < req.MinTVLUSD {
			continue
		}

		riskLevel, reasons := riskFromSymbol(item.Reserve.LiquidityToken)
		if yieldutil.RiskOrder(riskLevel) > maxRisk {
			continue
		}

		borrowUSD := parseNonNegative(item.Reserve.TotalBorrowUSD)
		liquidityUSD := tvl - borrowUSD
		if liquidityUSD <= 0 {
			liquidityUSD = tvl
		}

		assetID := reserveAssetID(req.Chain.CAIP2, req.Asset.AssetID, item.Reserve.LiquidityTokenMint)
		seed := strings.Join([]string{
			"kamino",
			req.Chain.CAIP2,
			item.Market.LendingMarket,
			item.Reserve.Reserve,
			assetID,
		}, "|")
		out = append(out, model.YieldOpportunity{
			OpportunityID:        hashOpportunity(seed),
			Provider:             "kamino",
			Protocol:             "kamino",
			ChainID:              req.Chain.CAIP2,
			AssetID:              assetID,
			ProviderNativeID:     strings.TrimSpace(item.Reserve.Reserve),
			ProviderNativeIDKind: model.NativeIDKindPoolID,
			Type:                 "lend",
			APYBase:              apy,
			APYReward:            0,
			APYTotal:             apy,
			TVLUSD:               tvl,
			LiquidityUSD:         liquidityUSD,
			LockupDays:           0,
			WithdrawalTerms:      "variable",
			RiskLevel:            riskLevel,
			RiskReasons:          reasons,
			Score:                yieldutil.ScoreOpportunity(apy, tvl, liquidityUSD, riskLevel),
			SourceURL:            marketURL(item.Market.LendingMarket),
			FetchedAt:            fetchedAt,
		})
	}

	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "no kamino yield opportunities for requested chain/asset")
	}
	yieldutil.Sort(out, req.SortBy)
	if req.Limit <= 0 || req.Limit > len(out) {
		req.Limit = len(out)
	}
	return out[:req.Limit], nil
}

func (c *Client) fetchReserves(ctx context.Context, chain id.Chain) ([]reserveWithMarket, error) {
	if !chain.IsSolana() {
		return nil, clierr.New(clierr.CodeUnsupported, "kamino supports only Solana chains")
	}
	if chain.CAIP2 != solanaMainnetCAIP2 {
		return nil, clierr.New(clierr.CodeUnsupported, "kamino supports only Solana mainnet")
	}

	marketsURL := fmt.Sprintf("%s/v2/kamino-market", strings.TrimRight(c.baseURL, "/"))
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, marketsURL, nil)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "build kamino markets request", err)
	}

	var markets []marketInfo
	if _, err := c.http.DoJSON(ctx, req, &markets); err != nil {
		return nil, err
	}
	if len(markets) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "kamino returned no lending markets")
	}

	sort.Slice(markets, func(i, j int) bool {
		if markets[i].IsPrimary != markets[j].IsPrimary {
			return markets[i].IsPrimary
		}
		if markets[i].IsCurated != markets[j].IsCurated {
			return markets[i].IsCurated
		}
		return strings.Compare(markets[i].LendingMarket, markets[j].LendingMarket) < 0
	})

	type marketResult struct {
		market   marketInfo
		reserves []reserveMetric
		err      error
	}
	results := make([]marketResult, len(markets))

	workerLimit := marketFetchWorkers
	if workerLimit <= 0 {
		workerLimit = 1
	}
	if workerLimit > len(markets) {
		workerLimit = len(markets)
	}
	sem := make(chan struct{}, workerLimit)
	var wg sync.WaitGroup
	for i, market := range markets {
		wg.Add(1)
		go func(index int, market marketInfo) {
			defer wg.Done()

			select {
			case sem <- struct{}{}:
			case <-ctx.Done():
				results[index] = marketResult{market: market, err: ctx.Err()}
				return
			}
			defer func() { <-sem }()

			reserves, err := c.fetchMarketReserves(ctx, market.LendingMarket)
			results[index] = marketResult{market: market, reserves: reserves, err: err}
		}(i, market)
	}
	wg.Wait()

	collected := make([]reserveWithMarket, 0, len(markets)*8)
	var firstErr error
	for _, result := range results {
		if result.err != nil {
			if firstErr == nil {
				firstErr = result.err
			}
			continue
		}
		for _, reserve := range result.reserves {
			collected = append(collected, reserveWithMarket{Market: result.market, Reserve: reserve})
		}
	}
	if firstErr != nil {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "kamino reserve fetch incomplete", firstErr)
	}
	if len(collected) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "kamino returned no reserves")
	}
	return collected, nil
}

func matchesReserveAsset(reserve reserveMetric, asset id.Asset) bool {
	if strings.TrimSpace(asset.Address) != "" {
		return strings.TrimSpace(reserve.LiquidityTokenMint) == strings.TrimSpace(asset.Address)
	}
	return strings.EqualFold(strings.TrimSpace(reserve.LiquidityToken), strings.TrimSpace(asset.Symbol))
}

func (c *Client) fetchMarketReserves(ctx context.Context, marketPubkey string) ([]reserveMetric, error) {
	endpoint := fmt.Sprintf(
		"%s/kamino-market/%s/reserves/metrics?env=mainnet-beta",
		strings.TrimRight(c.baseURL, "/"),
		strings.TrimSpace(marketPubkey),
	)
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, endpoint, nil)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "build kamino reserves request", err)
	}
	var reserves []reserveMetric
	if _, err := c.http.DoJSON(ctx, req, &reserves); err != nil {
		return nil, err
	}
	return reserves, nil
}

func reserveAssetID(chainID, fallbackAssetID, mint string) string {
	mint = strings.TrimSpace(mint)
	if mint == "" {
		return fallbackAssetID
	}
	return fmt.Sprintf("%s/token:%s", chainID, mint)
}

func marketURL(pubkey string) string {
	pubkey = strings.TrimSpace(pubkey)
	if pubkey == "" {
		return "https://app.kamino.finance"
	}
	return "https://app.kamino.finance/lending/" + pubkey
}

func ratioToPercent(v string) float64 {
	ratio := parseNonNegative(v)
	return ratio * 100
}

func parseNonNegative(v string) float64 {
	f, err := strconv.ParseFloat(strings.TrimSpace(v), 64)
	if err != nil || math.IsNaN(f) || math.IsInf(f, 0) || f < 0 {
		return 0
	}
	return f
}

func hashOpportunity(seed string) string {
	sum := sha1.Sum([]byte(seed))
	return hex.EncodeToString(sum[:])
}

func riskFromSymbol(symbol string) (string, []string) {
	switch strings.ToUpper(strings.TrimSpace(symbol)) {
	case "USDC", "USDT", "DAI", "USDE", "PYUSD":
		return "low", []string{"stablecoin collateral and borrow profile"}
	default:
		return "medium", []string{"non-stable asset volatility"}
	}
}
