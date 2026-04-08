package aave

import (
	"context"
	"fmt"
	"sort"
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
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
			"lend.positions",
			"yield.opportunities",
			"yield.positions",
			"yield.history",
			"lend.plan",
			"lend.execute",
			"yield.plan",
			"yield.execute",
			"rewards.plan",
			"rewards.execute",
		},
	}
}

const marketsQuery = `query Markets($request: MarketsRequest!) {
  markets(request: $request) {
    name
    address
    chain { chainId name }
    reserves {
      underlyingToken { address symbol decimals }
      aToken { address }
      size { usd }
      supplyInfo { apy { value } total { value } }
      borrowInfo { apy { value } total { usd } utilizationRate { value } availableLiquidity { usd } }
    }
  }
}`

const marketAddressesQuery = `query MarketAddresses($request: MarketsRequest!) {
  markets(request: $request) {
    address
  }
}`

const positionsQuery = `query Positions($suppliesRequest: UserSuppliesRequest!, $borrowsRequest: UserBorrowsRequest!) {
  userSupplies(request: $suppliesRequest) {
    market { address }
    currency { address symbol decimals }
    balance { amount { raw decimals value } usd }
    apy { value }
    isCollateral
    canBeCollateral
  }
  userBorrows(request: $borrowsRequest) {
    market { address }
    currency { address symbol decimals }
    debt { amount { raw decimals value } usd }
    apy { value }
  }
}`

const supplyAPYHistoryQuery = `query SupplyAPYHistory($request: SupplyAPYHistoryRequest!) {
  supplyAPYHistory(request: $request) {
    date
    avgRate { value }
  }
}`

type marketsData struct {
	Markets []aaveMarket `json:"markets"`
}

type marketAddressesData struct {
	Markets []struct {
		Address string `json:"address"`
	} `json:"markets"`
}

type positionsData struct {
	UserSupplies []aaveUserSupply `json:"userSupplies"`
	UserBorrows  []aaveUserBorrow `json:"userBorrows"`
}

type supplyAPYHistoryData struct {
	SupplyAPYHistory []struct {
		Date    string `json:"date"`
		AvgRate struct {
			Value string `json:"value"`
		} `json:"avgRate"`
	} `json:"supplyAPYHistory"`
}

type aaveMarket struct {
	Name    string `json:"name"`
	Address string `json:"address"`
	Chain   struct {
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
	AToken struct {
		Address string `json:"address"`
	} `json:"aToken"`
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
		AvailableLiquidity struct {
			USD string `json:"usd"`
		} `json:"availableLiquidity"`
	} `json:"borrowInfo"`
}

type aaveUserSupply struct {
	Market struct {
		Address string `json:"address"`
	} `json:"market"`
	Currency struct {
		Address  string `json:"address"`
		Symbol   string `json:"symbol"`
		Decimals int    `json:"decimals"`
	} `json:"currency"`
	Balance struct {
		Amount struct {
			Raw      string `json:"raw"`
			Decimals int    `json:"decimals"`
			Value    string `json:"value"`
		} `json:"amount"`
		USD string `json:"usd"`
	} `json:"balance"`
	APY struct {
		Value string `json:"value"`
	} `json:"apy"`
	IsCollateral    bool `json:"isCollateral"`
	CanBeCollateral bool `json:"canBeCollateral"`
}

type aaveUserBorrow struct {
	Market struct {
		Address string `json:"address"`
	} `json:"market"`
	Currency struct {
		Address  string `json:"address"`
		Symbol   string `json:"symbol"`
		Decimals int    `json:"decimals"`
	} `json:"currency"`
	Debt struct {
		Amount struct {
			Raw      string `json:"raw"`
			Decimals int    `json:"decimals"`
			Value    string `json:"value"`
		} `json:"amount"`
		USD string `json:"usd"`
	} `json:"debt"`
	APY struct {
		Value string `json:"value"`
	} `json:"apy"`
}

func (c *Client) LendMarkets(ctx context.Context, provider string, chain id.Chain, asset id.Asset) ([]model.LendMarket, error) {
	if err := providers.ValidateProvider(provider, "aave"); err != nil {
		return nil, err
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
			supplyAPY := providers.ParseFloat(r.SupplyInfo.APY.Value) * 100
			borrowAPY := 0.0
			if r.BorrowInfo != nil {
				borrowAPY = providers.ParseFloat(r.BorrowInfo.APY.Value) * 100
			}
			tvlUSD := providers.ParseFloat(r.Size.USD)
			if tvlUSD <= 0 {
				continue
			}

			out = append(out, model.LendMarket{
				Protocol:             "aave",
				Provider:             "aave",
				ChainID:              chain.CAIP2,
				AssetID:              providers.CanonicalAssetID(asset, r.UnderlyingToken.Address),
				ProviderNativeID:     providers.ProviderNativeID("aave", chain.CAIP2, m.Address, r.UnderlyingToken.Address),
				ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
				SupplyAPY:            supplyAPY,
				BorrowAPY:            borrowAPY,
				TVLUSD:               tvlUSD,
				LiquidityUSD:         tvlUSD,
				SourceURL:            "https://app.aave.com",
				FetchedAt:            c.now().UTC().Format(time.RFC3339),
			})
		}
	}

	providers.SortLendMarkets(out)
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "no aave lending market for requested chain/asset")
	}
	return out, nil
}

func (c *Client) LendRates(ctx context.Context, provider string, chain id.Chain, asset id.Asset) ([]model.LendRate, error) {
	if err := providers.ValidateProvider(provider, "aave"); err != nil {
		return nil, err
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
			supplyAPY := providers.ParseFloat(r.SupplyInfo.APY.Value) * 100
			borrowAPY := 0.0
			utilization := 0.0
			if r.BorrowInfo != nil {
				borrowAPY = providers.ParseFloat(r.BorrowInfo.APY.Value) * 100
				utilization = providers.ParseFloat(r.BorrowInfo.UtilizationRate.Value)
			}
			out = append(out, model.LendRate{
				Protocol:             "aave",
				Provider:             "aave",
				ChainID:              chain.CAIP2,
				AssetID:              providers.CanonicalAssetID(asset, r.UnderlyingToken.Address),
				ProviderNativeID:     providers.ProviderNativeID("aave", chain.CAIP2, m.Address, r.UnderlyingToken.Address),
				ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
				SupplyAPY:            supplyAPY,
				BorrowAPY:            borrowAPY,
				Utilization:          utilization,
				SourceURL:            "https://app.aave.com",
				FetchedAt:            c.now().UTC().Format(time.RFC3339),
			})
		}
	}

	providers.SortLendRates(out)
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "no aave lending rates for requested chain/asset")
	}
	return out, nil
}

func (c *Client) LendPositions(ctx context.Context, req providers.LendPositionsRequest) ([]model.LendPosition, error) {
	if !req.Chain.IsEVM() {
		return nil, clierr.New(clierr.CodeUnsupported, "aave supports only EVM chains")
	}
	account := providers.NormalizeEVMAddress(req.Account)
	if account == "" {
		return nil, clierr.New(clierr.CodeUsage, "aave positions requires a valid EVM account address")
	}

	marketAddresses, err := c.fetchMarketAddresses(ctx, req.Chain)
	if err != nil {
		return nil, err
	}
	markets := make([]map[string]any, 0, len(marketAddresses))
	for _, address := range marketAddresses {
		markets = append(markets, map[string]any{
			"address": address,
			"chainId": req.Chain.EVMChainID,
		})
	}

	var data positionsData
	if err := providers.PostGraphQL(ctx, c.http, c.endpoint, positionsQuery, map[string]any{
		"suppliesRequest": map[string]any{
			"markets":         markets,
			"user":            account,
			"collateralsOnly": false,
			"orderBy": map[string]any{
				"balance": "DESC",
			},
		},
		"borrowsRequest": map[string]any{
			"markets": markets,
			"user":    account,
			"orderBy": map[string]any{
				"debt": "DESC",
			},
		},
	}, &data, "aave"); err != nil {
		return nil, err
	}

	filterType := req.PositionType
	if filterType == "" {
		filterType = providers.LendPositionTypeAll
	}
	out := make([]model.LendPosition, 0, len(data.UserSupplies)+len(data.UserBorrows))
	for _, supply := range data.UserSupplies {
		positionType := providers.LendPositionTypeSupply
		if supply.IsCollateral {
			positionType = providers.LendPositionTypeCollateral
		}
		if !providers.MatchesPositionType(filterType, positionType) {
			continue
		}
		if !providers.MatchesAsset(supply.Currency.Address, supply.Currency.Symbol, req.Asset) {
			continue
		}

		assetID := providers.CanonicalAssetIDForChain(req.Chain.CAIP2, supply.Currency.Address)
		if assetID == "" {
			continue
		}
		amount := providers.AmountInfoFromRaw(supply.Balance.Amount.Raw, supply.Currency.Decimals)
		out = append(out, model.LendPosition{
			Protocol:             "aave",
			Provider:             "aave",
			ChainID:              req.Chain.CAIP2,
			AccountAddress:       account,
			PositionType:         string(positionType),
			AssetID:              assetID,
			ProviderNativeID:     providers.ProviderNativeID("aave", req.Chain.CAIP2, supply.Market.Address, supply.Currency.Address),
			ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
			Amount:               amount,
			AmountUSD:            providers.ParseFloat(supply.Balance.USD),
			APY:                  providers.ParseFloat(supply.APY.Value) * 100,
			SourceURL:            "https://app.aave.com",
			FetchedAt:            c.now().UTC().Format(time.RFC3339),
		})
	}

	for _, borrow := range data.UserBorrows {
		if !providers.MatchesPositionType(filterType, providers.LendPositionTypeBorrow) {
			continue
		}
		if !providers.MatchesAsset(borrow.Currency.Address, borrow.Currency.Symbol, req.Asset) {
			continue
		}

		assetID := providers.CanonicalAssetIDForChain(req.Chain.CAIP2, borrow.Currency.Address)
		if assetID == "" {
			continue
		}
		amount := providers.AmountInfoFromRaw(borrow.Debt.Amount.Raw, borrow.Currency.Decimals)
		out = append(out, model.LendPosition{
			Protocol:             "aave",
			Provider:             "aave",
			ChainID:              req.Chain.CAIP2,
			AccountAddress:       account,
			PositionType:         string(providers.LendPositionTypeBorrow),
			AssetID:              assetID,
			ProviderNativeID:     providers.ProviderNativeID("aave", req.Chain.CAIP2, borrow.Market.Address, borrow.Currency.Address),
			ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
			Amount:               amount,
			AmountUSD:            providers.ParseFloat(borrow.Debt.USD),
			APY:                  providers.ParseFloat(borrow.APY.Value) * 100,
			SourceURL:            "https://app.aave.com",
			FetchedAt:            c.now().UTC().Format(time.RFC3339),
		})
	}

	return providers.FinalizeLendPositions(out, req.Limit), nil
}

func (c *Client) YieldOpportunities(ctx context.Context, req providers.YieldRequest) ([]model.YieldOpportunity, error) {
	markets, err := c.fetchMarkets(ctx, req.Chain)
	if err != nil {
		return nil, err
	}

	out := make([]model.YieldOpportunity, 0)
	for _, m := range markets {
		for _, r := range m.Reserves {
			if !matchesReserveAsset(r, req.Asset) {
				continue
			}
			apy := providers.ParseFloat(r.SupplyInfo.APY.Value) * 100
			tvl := providers.ParseFloat(r.Size.USD)
			if (apy == 0 || tvl == 0) && !req.IncludeIncomplete {
				continue
			}
			if apy < req.MinAPY {
				continue
			}
			if tvl < req.MinTVLUSD {
				continue
			}

			assetID := providers.CanonicalAssetID(req.Asset, r.UnderlyingToken.Address)
			liquidityUSD := tvl
			if r.BorrowInfo != nil {
				liquidityUSD = providers.ParseFloat(r.BorrowInfo.AvailableLiquidity.USD)
			}
			nativeID := providers.ProviderNativeID("aave", req.Chain.CAIP2, m.Address, r.UnderlyingToken.Address)
			opportunityID := providers.HashOpportunity("aave", req.Chain.CAIP2, nativeID, assetID)
			out = append(out, model.YieldOpportunity{
				OpportunityID:        opportunityID,
				Provider:             "aave",
				Protocol:             "aave",
				ChainID:              req.Chain.CAIP2,
				AssetID:              assetID,
				ProviderNativeID:     nativeID,
				ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
				Type:                 "lend",
				APYBase:              apy,
				APYReward:            0,
				APYTotal:             apy,
				TVLUSD:               tvl,
				LiquidityUSD:         liquidityUSD,
				LockupDays:           0,
				WithdrawalTerms:      "variable",
				BackingAssets: []model.YieldBackingAsset{{
					AssetID:  assetID,
					Symbol:   strings.TrimSpace(r.UnderlyingToken.Symbol),
					SharePct: 100,
				}},
				SourceURL: "https://app.aave.com",
				FetchedAt: c.now().UTC().Format(time.RFC3339),
			})
		}
	}

	return providers.FinalizeYieldOpportunities(out, "aave", req.SortBy, req.Limit)
}

func (c *Client) YieldPositions(ctx context.Context, req providers.YieldPositionsRequest) ([]model.YieldPosition, error) {
	lendRows, err := c.LendPositions(ctx, providers.LendPositionsRequest{
		Chain:        req.Chain,
		Account:      req.Account,
		Asset:        req.Asset,
		PositionType: providers.LendPositionTypeAll,
		Limit:        req.Limit,
	})
	if err != nil {
		return nil, err
	}

	out := providers.LendToYieldPositions("aave", lendRows)
	return providers.FinalizeYieldPositions(out, req.Limit), nil
}

func (c *Client) YieldHistory(ctx context.Context, req providers.YieldHistoryRequest) ([]model.YieldHistorySeries, error) {
	if !strings.EqualFold(strings.TrimSpace(req.Opportunity.Provider), "aave") {
		return nil, clierr.New(clierr.CodeUnsupported, "aave history supports only aave opportunities")
	}
	if !req.StartTime.Before(req.EndTime) {
		return nil, clierr.New(clierr.CodeUsage, "history start time must be before end time")
	}
	metricSet := make(map[providers.YieldHistoryMetric]struct{}, len(req.Metrics))
	for _, metric := range req.Metrics {
		metricSet[metric] = struct{}{}
	}
	for metric := range metricSet {
		if metric != providers.YieldHistoryMetricAPYTotal {
			return nil, clierr.New(clierr.CodeUnsupported, "aave history supports only metric=apy_total")
		}
	}

	chain, err := id.ParseChain(req.Opportunity.ChainID)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeUsage, "parse aave opportunity chain", err)
	}
	if !chain.IsEVM() {
		return nil, clierr.New(clierr.CodeUnsupported, "aave supports only EVM chains")
	}

	marketAddress, underlyingAddress, err := parseOpportunityNativeID(req.Opportunity)
	if err != nil {
		return nil, err
	}
	window, err := historyWindow(req.StartTime, req.EndTime, c.now().UTC())
	if err != nil {
		return nil, err
	}

	var data supplyAPYHistoryData
	if err := providers.PostGraphQL(ctx, c.http, c.endpoint, supplyAPYHistoryQuery, map[string]any{
		"request": map[string]any{
			"market":          marketAddress,
			"underlyingToken": underlyingAddress,
			"window":          window,
			"chainId":         chain.EVMChainID,
		},
	}, &data, "aave"); err != nil {
		return nil, err
	}

	points := make([]model.YieldHistoryPoint, 0, len(data.SupplyAPYHistory))
	for _, sample := range data.SupplyAPYHistory {
		ts, ok := providers.ParseAPITime(sample.Date)
		if !ok {
			continue
		}
		if ts.Before(req.StartTime) || ts.After(req.EndTime) {
			continue
		}
		points = append(points, model.YieldHistoryPoint{
			Timestamp: ts.UTC().Format(time.RFC3339),
			Value:     providers.ParseFloat(sample.AvgRate.Value) * 100,
		})
	}
	if req.Interval == providers.YieldHistoryIntervalDay {
		points = averagePointsByDay(points)
	} else {
		providers.SortHistoryPoints(points)
	}
	if len(points) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "no aave historical points for requested range")
	}

	series := []model.YieldHistorySeries{
		{
			OpportunityID:        req.Opportunity.OpportunityID,
			Provider:             "aave",
			Protocol:             req.Opportunity.Protocol,
			ChainID:              req.Opportunity.ChainID,
			AssetID:              req.Opportunity.AssetID,
			ProviderNativeID:     req.Opportunity.ProviderNativeID,
			ProviderNativeIDKind: req.Opportunity.ProviderNativeIDKind,
			Metric:               string(providers.YieldHistoryMetricAPYTotal),
			Interval:             string(req.Interval),
			StartTime:            req.StartTime.UTC().Format(time.RFC3339),
			EndTime:              req.EndTime.UTC().Format(time.RFC3339),
			Points:               points,
			SourceURL:            req.Opportunity.SourceURL,
			FetchedAt:            c.now().UTC().Format(time.RFC3339),
		},
	}
	return series, nil
}

func (c *Client) fetchMarkets(ctx context.Context, chain id.Chain) ([]aaveMarket, error) {
	if !chain.IsEVM() {
		return nil, clierr.New(clierr.CodeUnsupported, "aave supports only EVM chains")
	}
	var data marketsData
	if err := providers.PostGraphQL(ctx, c.http, c.endpoint, marketsQuery, map[string]any{
		"request": map[string]any{
			"chainIds": []int64{chain.EVMChainID},
		},
	}, &data, "aave"); err != nil {
		return nil, err
	}
	if len(data.Markets) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "aave has no market for requested chain")
	}
	return data.Markets, nil
}

func (c *Client) fetchMarketAddresses(ctx context.Context, chain id.Chain) ([]string, error) {
	if !chain.IsEVM() {
		return nil, clierr.New(clierr.CodeUnsupported, "aave supports only EVM chains")
	}
	var data marketAddressesData
	if err := providers.PostGraphQL(ctx, c.http, c.endpoint, marketAddressesQuery, map[string]any{
		"request": map[string]any{
			"chainIds": []int64{chain.EVMChainID},
		},
	}, &data, "aave"); err != nil {
		return nil, err
	}
	if len(data.Markets) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "aave has no market for requested chain")
	}
	out := make([]string, 0, len(data.Markets))
	for _, market := range data.Markets {
		address := providers.NormalizeEVMAddress(market.Address)
		if address != "" {
			out = append(out, address)
		}
	}
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "aave market list returned no valid addresses")
	}
	return out, nil
}

func matchesReserveAsset(r aaveReserve, asset id.Asset) bool {
	assetAddress := strings.TrimSpace(asset.Address)
	if assetAddress != "" {
		return strings.EqualFold(strings.TrimSpace(r.UnderlyingToken.Address), assetAddress)
	}
	return strings.EqualFold(strings.TrimSpace(r.UnderlyingToken.Symbol), strings.TrimSpace(asset.Symbol))
}

func parseOpportunityNativeID(op model.YieldOpportunity) (string, string, error) {
	nativeID := strings.TrimSpace(op.ProviderNativeID)
	if nativeID == "" {
		return "", "", clierr.New(clierr.CodeUsage, "aave opportunity missing provider_native_id")
	}
	prefix := fmt.Sprintf("aave:%s:", strings.TrimSpace(op.ChainID))
	if !strings.HasPrefix(strings.ToLower(nativeID), strings.ToLower(prefix)) {
		return "", "", clierr.New(clierr.CodeUsage, "invalid aave provider_native_id format")
	}
	suffix := nativeID[len(prefix):]
	parts := strings.SplitN(suffix, ":", 2)
	if len(parts) != 2 {
		return "", "", clierr.New(clierr.CodeUsage, "invalid aave provider_native_id format")
	}
	marketAddress := providers.NormalizeEVMAddress(parts[0])
	underlyingAddress := providers.NormalizeEVMAddress(parts[1])
	if marketAddress == "" || underlyingAddress == "" {
		return "", "", clierr.New(clierr.CodeUsage, "invalid aave provider_native_id addresses")
	}
	return marketAddress, underlyingAddress, nil
}

func historyWindow(start, end, now time.Time) (string, error) {
	if end.Before(now.Add(-2 * time.Hour)) {
		return "", clierr.New(clierr.CodeUnsupported, "aave history supports lookback windows ending near now")
	}
	span := end.Sub(start)
	switch {
	case span <= 24*time.Hour:
		return "LAST_DAY", nil
	case span <= 7*24*time.Hour:
		return "LAST_WEEK", nil
	case span <= 31*24*time.Hour:
		return "LAST_MONTH", nil
	case span <= 183*24*time.Hour:
		return "LAST_SIX_MONTHS", nil
	case span <= 366*24*time.Hour:
		return "LAST_YEAR", nil
	default:
		return "", clierr.New(clierr.CodeUnsupported, "aave history supports windows up to 1 year")
	}
}

func averagePointsByDay(points []model.YieldHistoryPoint) []model.YieldHistoryPoint {
	if len(points) == 0 {
		return nil
	}
	providers.SortHistoryPoints(points)
	type bucket struct {
		sum   float64
		count int
	}
	byDay := map[string]bucket{}
	for _, point := range points {
		ts, err := time.Parse(time.RFC3339, point.Timestamp)
		if err != nil {
			continue
		}
		day := ts.UTC().Format("2006-01-02")
		entry := byDay[day]
		entry.sum += point.Value
		entry.count++
		byDay[day] = entry
	}
	days := make([]string, 0, len(byDay))
	for day := range byDay {
		days = append(days, day)
	}
	sort.Strings(days)
	out := make([]model.YieldHistoryPoint, 0, len(days))
	for _, day := range days {
		entry := byDay[day]
		if entry.count == 0 {
			continue
		}
		out = append(out, model.YieldHistoryPoint{
			Timestamp: day + "T00:00:00Z",
			Value:     entry.sum / float64(entry.count),
		})
	}
	return out
}

