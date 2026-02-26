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

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/ggonzalez94/defi-cli/internal/providers/yieldutil"
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
			"yield.history",
			"lend.plan",
			"lend.execute",
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
      borrowInfo { apy { value } total { usd } utilizationRate { value } }
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

type marketsResponse struct {
	Data struct {
		Markets []aaveMarket `json:"markets"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type marketAddressesResponse struct {
	Data struct {
		Markets []struct {
			Address string `json:"address"`
		} `json:"markets"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type positionsResponse struct {
	Data struct {
		UserSupplies []aaveUserSupply `json:"userSupplies"`
		UserBorrows  []aaveUserBorrow `json:"userBorrows"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type supplyAPYHistoryResponse struct {
	Data struct {
		SupplyAPYHistory []struct {
			Date    string `json:"date"`
			AvgRate struct {
				Value string `json:"value"`
			} `json:"avgRate"`
		} `json:"supplyAPYHistory"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
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
	if !strings.EqualFold(provider, "aave") {
		return nil, clierr.New(clierr.CodeUnsupported, "aave adapter supports only provider=aave")
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
				Protocol:             "aave",
				Provider:             "aave",
				ChainID:              chain.CAIP2,
				AssetID:              canonicalAssetID(asset, r.UnderlyingToken.Address),
				ProviderNativeID:     providerNativeID("aave", chain.CAIP2, m.Address, r.UnderlyingToken.Address),
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

func (c *Client) LendRates(ctx context.Context, provider string, chain id.Chain, asset id.Asset) ([]model.LendRate, error) {
	if !strings.EqualFold(provider, "aave") {
		return nil, clierr.New(clierr.CodeUnsupported, "aave adapter supports only provider=aave")
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
				Protocol:             "aave",
				Provider:             "aave",
				ChainID:              chain.CAIP2,
				AssetID:              canonicalAssetID(asset, r.UnderlyingToken.Address),
				ProviderNativeID:     providerNativeID("aave", chain.CAIP2, m.Address, r.UnderlyingToken.Address),
				ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
				SupplyAPY:            supplyAPY,
				BorrowAPY:            borrowAPY,
				Utilization:          utilization,
				SourceURL:            "https://app.aave.com",
				FetchedAt:            c.now().UTC().Format(time.RFC3339),
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

func (c *Client) LendPositions(ctx context.Context, req providers.LendPositionsRequest) ([]model.LendPosition, error) {
	if !req.Chain.IsEVM() {
		return nil, clierr.New(clierr.CodeUnsupported, "aave supports only EVM chains")
	}
	account := normalizeEVMAddress(req.Account)
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

	body, err := json.Marshal(map[string]any{
		"query": positionsQuery,
		"variables": map[string]any{
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
		},
	})
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "marshal aave positions query", err)
	}

	var resp positionsResponse
	if _, err := httpx.DoBodyJSON(ctx, c.http, http.MethodPost, c.endpoint, body, nil, &resp); err != nil {
		return nil, err
	}
	if len(resp.Errors) > 0 {
		return nil, clierr.New(clierr.CodeUnavailable, fmt.Sprintf("aave graphql error: %s", resp.Errors[0].Message))
	}

	filterType := req.PositionType
	if filterType == "" {
		filterType = providers.LendPositionTypeAll
	}
	out := make([]model.LendPosition, 0, len(resp.Data.UserSupplies)+len(resp.Data.UserBorrows))
	for _, supply := range resp.Data.UserSupplies {
		positionType := providers.LendPositionTypeSupply
		if supply.IsCollateral {
			positionType = providers.LendPositionTypeCollateral
		}
		if !matchesPositionType(filterType, positionType) {
			continue
		}
		if !matchesPositionAsset(supply.Currency.Address, supply.Currency.Symbol, req.Asset) {
			continue
		}

		assetID := canonicalAssetIDForChain(req.Chain.CAIP2, supply.Currency.Address)
		if assetID == "" {
			continue
		}
		amount := amountInfoFromRaw(supply.Balance.Amount.Raw, supply.Currency.Decimals)
		out = append(out, model.LendPosition{
			Protocol:             "aave",
			Provider:             "aave",
			ChainID:              req.Chain.CAIP2,
			AccountAddress:       account,
			PositionType:         string(positionType),
			AssetID:              assetID,
			ProviderNativeID:     providerNativeID("aave", req.Chain.CAIP2, supply.Market.Address, supply.Currency.Address),
			ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
			Amount:               amount,
			AmountUSD:            parseFloat(supply.Balance.USD),
			APY:                  parseFloat(supply.APY.Value) * 100,
			SourceURL:            "https://app.aave.com",
			FetchedAt:            c.now().UTC().Format(time.RFC3339),
		})
	}

	for _, borrow := range resp.Data.UserBorrows {
		if !matchesPositionType(filterType, providers.LendPositionTypeBorrow) {
			continue
		}
		if !matchesPositionAsset(borrow.Currency.Address, borrow.Currency.Symbol, req.Asset) {
			continue
		}

		assetID := canonicalAssetIDForChain(req.Chain.CAIP2, borrow.Currency.Address)
		if assetID == "" {
			continue
		}
		amount := amountInfoFromRaw(borrow.Debt.Amount.Raw, borrow.Currency.Decimals)
		out = append(out, model.LendPosition{
			Protocol:             "aave",
			Provider:             "aave",
			ChainID:              req.Chain.CAIP2,
			AccountAddress:       account,
			PositionType:         string(providers.LendPositionTypeBorrow),
			AssetID:              assetID,
			ProviderNativeID:     providerNativeID("aave", req.Chain.CAIP2, borrow.Market.Address, borrow.Currency.Address),
			ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
			Amount:               amount,
			AmountUSD:            parseFloat(borrow.Debt.USD),
			APY:                  parseFloat(borrow.APY.Value) * 100,
			SourceURL:            "https://app.aave.com",
			FetchedAt:            c.now().UTC().Format(time.RFC3339),
		})
	}

	sortLendPositions(out)
	if req.Limit > 0 && len(out) > req.Limit {
		out = out[:req.Limit]
	}
	return out, nil
}

func (c *Client) YieldOpportunities(ctx context.Context, req providers.YieldRequest) ([]model.YieldOpportunity, error) {
	markets, err := c.fetchMarkets(ctx, req.Chain)
	if err != nil {
		return nil, err
	}

	maxRisk := yieldutil.RiskOrder(req.MaxRisk)
	if maxRisk == 0 {
		maxRisk = yieldutil.RiskOrder("high")
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
			if yieldutil.RiskOrder(riskLevel) > maxRisk {
				continue
			}

			assetID := canonicalAssetID(req.Asset, r.UnderlyingToken.Address)
			normalizedMarket := normalizeEVMAddress(m.Address)
			normalizedUnderlying := normalizeEVMAddress(r.UnderlyingToken.Address)
			nativeID := providerNativeID("aave", req.Chain.CAIP2, normalizedMarket, normalizedUnderlying)
			opportunityID := hashOpportunity("aave", req.Chain.CAIP2, nativeID, assetID)
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
				LiquidityUSD:         tvl,
				LockupDays:           0,
				WithdrawalTerms:      "variable",
				RiskLevel:            riskLevel,
				RiskReasons:          reasons,
				Score:                yieldutil.ScoreOpportunity(apy, tvl, tvl, riskLevel),
				SourceURL:            "https://app.aave.com",
				FetchedAt:            c.now().UTC().Format(time.RFC3339),
			})
		}
	}

	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "no aave yield opportunities for requested chain/asset")
	}
	yieldutil.Sort(out, req.SortBy)
	if req.Limit <= 0 || req.Limit > len(out) {
		req.Limit = len(out)
	}
	return out[:req.Limit], nil
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

	body, err := json.Marshal(map[string]any{
		"query": supplyAPYHistoryQuery,
		"variables": map[string]any{
			"request": map[string]any{
				"market":          marketAddress,
				"underlyingToken": underlyingAddress,
				"window":          window,
				"chainId":         chain.EVMChainID,
			},
		},
	})
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "marshal aave history query", err)
	}

	var resp supplyAPYHistoryResponse
	if _, err := httpx.DoBodyJSON(ctx, c.http, http.MethodPost, c.endpoint, body, nil, &resp); err != nil {
		return nil, err
	}
	if len(resp.Errors) > 0 {
		return nil, clierr.New(clierr.CodeUnavailable, fmt.Sprintf("aave graphql error: %s", resp.Errors[0].Message))
	}

	points := make([]model.YieldHistoryPoint, 0, len(resp.Data.SupplyAPYHistory))
	for _, sample := range resp.Data.SupplyAPYHistory {
		ts, ok := parseAPITime(sample.Date)
		if !ok {
			continue
		}
		if ts.Before(req.StartTime) || ts.After(req.EndTime) {
			continue
		}
		points = append(points, model.YieldHistoryPoint{
			Timestamp: ts.UTC().Format(time.RFC3339),
			Value:     parseFloat(sample.AvgRate.Value) * 100,
		})
	}
	if req.Interval == providers.YieldHistoryIntervalDay {
		points = averagePointsByDay(points)
	} else {
		sortHistoryPoints(points)
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

func (c *Client) fetchMarketAddresses(ctx context.Context, chain id.Chain) ([]string, error) {
	if !chain.IsEVM() {
		return nil, clierr.New(clierr.CodeUnsupported, "aave supports only EVM chains")
	}
	body, err := json.Marshal(map[string]any{
		"query": marketAddressesQuery,
		"variables": map[string]any{
			"request": map[string]any{
				"chainIds": []int64{chain.EVMChainID},
			},
		},
	})
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "marshal aave market-address query", err)
	}

	var resp marketAddressesResponse
	if _, err := httpx.DoBodyJSON(ctx, c.http, http.MethodPost, c.endpoint, body, nil, &resp); err != nil {
		return nil, err
	}
	if len(resp.Errors) > 0 {
		return nil, clierr.New(clierr.CodeUnavailable, fmt.Sprintf("aave graphql error: %s", resp.Errors[0].Message))
	}
	if len(resp.Data.Markets) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "aave has no market for requested chain")
	}
	out := make([]string, 0, len(resp.Data.Markets))
	for _, market := range resp.Data.Markets {
		address := normalizeEVMAddress(market.Address)
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

func normalizeEVMAddress(address string) string {
	addr := strings.ToLower(strings.TrimSpace(address))
	if len(addr) != 42 || !strings.HasPrefix(addr, "0x") {
		return ""
	}
	return addr
}

func providerNativeID(provider, chainID, marketAddress, underlyingAddress string) string {
	return fmt.Sprintf("%s:%s:%s:%s", provider, chainID, normalizeEVMAddress(marketAddress), normalizeEVMAddress(underlyingAddress))
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
	marketAddress := normalizeEVMAddress(parts[0])
	underlyingAddress := normalizeEVMAddress(parts[1])
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

func parseAPITime(v string) (time.Time, bool) {
	raw := strings.TrimSpace(v)
	if raw == "" {
		return time.Time{}, false
	}
	ts, err := time.Parse(time.RFC3339, raw)
	if err == nil {
		return ts.UTC(), true
	}
	ts, err = time.Parse(time.RFC3339Nano, raw)
	if err == nil {
		return ts.UTC(), true
	}
	return time.Time{}, false
}

func sortHistoryPoints(points []model.YieldHistoryPoint) {
	sort.Slice(points, func(i, j int) bool {
		return strings.Compare(points[i].Timestamp, points[j].Timestamp) < 0
	})
}

func averagePointsByDay(points []model.YieldHistoryPoint) []model.YieldHistoryPoint {
	if len(points) == 0 {
		return nil
	}
	sortHistoryPoints(points)
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

func amountInfoFromRaw(raw string, decimals int) model.AmountInfo {
	if decimals < 0 {
		decimals = 0
	}
	base := normalizeBaseUnits(raw)
	return model.AmountInfo{
		AmountBaseUnits: base,
		AmountDecimal:   id.FormatDecimalCompat(base, decimals),
		Decimals:        decimals,
	}
}

func normalizeBaseUnits(v string) string {
	clean := strings.TrimSpace(v)
	if clean == "" {
		return "0"
	}
	for _, r := range clean {
		if r < '0' || r > '9' {
			return "0"
		}
	}
	return clean
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

func hashOpportunity(provider, chainID, marketID, assetID string) string {
	seed := strings.Join([]string{provider, chainID, marketID, assetID}, "|")
	h := sha1.Sum([]byte(seed))
	return hex.EncodeToString(h[:])
}
