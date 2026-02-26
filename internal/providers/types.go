package providers

import (
	"context"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
)

type Provider interface {
	Info() model.ProviderInfo
}

type MarketDataProvider interface {
	Provider
	ChainsTop(ctx context.Context, limit int) ([]model.ChainTVL, error)
	ChainsAssets(ctx context.Context, chain id.Chain, asset id.Asset, limit int) ([]model.ChainAssetTVL, error)
	ProtocolsTop(ctx context.Context, category string, limit int) ([]model.ProtocolTVL, error)
	ProtocolsCategories(ctx context.Context) ([]model.ProtocolCategory, error)
}

type LendingProvider interface {
	Provider
	LendMarkets(ctx context.Context, provider string, chain id.Chain, asset id.Asset) ([]model.LendMarket, error)
	LendRates(ctx context.Context, provider string, chain id.Chain, asset id.Asset) ([]model.LendRate, error)
}

type LendPositionType string

const (
	LendPositionTypeAll        LendPositionType = "all"
	LendPositionTypeSupply     LendPositionType = "supply"
	LendPositionTypeBorrow     LendPositionType = "borrow"
	LendPositionTypeCollateral LendPositionType = "collateral"
)

type LendPositionsRequest struct {
	Chain        id.Chain
	Account      string
	Asset        id.Asset
	PositionType LendPositionType
	Limit        int
}

type LendingPositionsProvider interface {
	Provider
	LendPositions(ctx context.Context, req LendPositionsRequest) ([]model.LendPosition, error)
}

type YieldProvider interface {
	Provider
	YieldOpportunities(ctx context.Context, req YieldRequest) ([]model.YieldOpportunity, error)
}

type YieldHistoryMetric string

const (
	YieldHistoryMetricAPYTotal YieldHistoryMetric = "apy_total"
	YieldHistoryMetricTVLUSD   YieldHistoryMetric = "tvl_usd"
)

type YieldHistoryInterval string

const (
	YieldHistoryIntervalHour YieldHistoryInterval = "hour"
	YieldHistoryIntervalDay  YieldHistoryInterval = "day"
)

type YieldHistoryRequest struct {
	Opportunity model.YieldOpportunity
	StartTime   time.Time
	EndTime     time.Time
	Interval    YieldHistoryInterval
	Metrics     []YieldHistoryMetric
}

type YieldHistoryProvider interface {
	Provider
	YieldHistory(ctx context.Context, req YieldHistoryRequest) ([]model.YieldHistorySeries, error)
}

type YieldRequest struct {
	Chain             id.Chain
	Asset             id.Asset
	Limit             int
	MinTVLUSD         float64
	MinAPY            float64
	Providers         []string
	SortBy            string
	IncludeIncomplete bool
}

type BridgeProvider interface {
	Provider
	QuoteBridge(ctx context.Context, req BridgeQuoteRequest) (model.BridgeQuote, error)
}

type BridgeExecutionProvider interface {
	BridgeProvider
	BuildBridgeAction(ctx context.Context, req BridgeQuoteRequest, opts BridgeExecutionOptions) (execution.Action, error)
}

type BridgeDataProvider interface {
	Provider
	ListBridges(ctx context.Context, req BridgeListRequest) ([]model.BridgeSummary, error)
	BridgeDetails(ctx context.Context, req BridgeDetailsRequest) (model.BridgeDetails, error)
}

type BridgeQuoteRequest struct {
	FromChain        id.Chain
	ToChain          id.Chain
	FromAsset        id.Asset
	ToAsset          id.Asset
	AmountBaseUnits  string
	AmountDecimal    string
	FromAmountForGas string
}

type BridgeListRequest struct {
	Limit         int
	IncludeChains bool
}

type BridgeDetailsRequest struct {
	Bridge                string
	IncludeChainBreakdown bool
}

type BridgeExecutionOptions struct {
	Sender           string
	Recipient        string
	SlippageBps      int64
	Simulate         bool
	RPCURL           string
	FromAmountForGas string
}

type SwapProvider interface {
	Provider
	QuoteSwap(ctx context.Context, req SwapQuoteRequest) (model.SwapQuote, error)
}

type SwapExecutionProvider interface {
	SwapProvider
	BuildSwapAction(ctx context.Context, req SwapQuoteRequest, opts SwapExecutionOptions) (execution.Action, error)
}

type SwapTradeType string

const (
	SwapTradeTypeExactInput  SwapTradeType = "exact-input"
	SwapTradeTypeExactOutput SwapTradeType = "exact-output"
)

type SwapQuoteRequest struct {
	Chain           id.Chain
	FromAsset       id.Asset
	ToAsset         id.Asset
	AmountBaseUnits string
	AmountDecimal   string
	RPCURL          string
	TradeType       SwapTradeType
	SlippagePct     *float64
	Swapper         string
}

type SwapExecutionOptions struct {
	Sender      string
	Recipient   string
	SlippageBps int64
	Simulate    bool
	RPCURL      string
}
