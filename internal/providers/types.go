package providers

import (
	"context"

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
	LendMarkets(ctx context.Context, protocol string, chain id.Chain, asset id.Asset) ([]model.LendMarket, error)
	LendRates(ctx context.Context, protocol string, chain id.Chain, asset id.Asset) ([]model.LendRate, error)
}

type YieldProvider interface {
	Provider
	YieldOpportunities(ctx context.Context, req YieldRequest) ([]model.YieldOpportunity, error)
}

type YieldRequest struct {
	Chain             id.Chain
	Asset             id.Asset
	Limit             int
	MinTVLUSD         float64
	MinAPY            float64
	MaxRisk           string
	Providers         []string
	SortBy            string
	IncludeIncomplete bool
}

type BridgeProvider interface {
	Provider
	QuoteBridge(ctx context.Context, req BridgeQuoteRequest) (model.BridgeQuote, error)
}

type BridgeDataProvider interface {
	Provider
	ListBridges(ctx context.Context, req BridgeListRequest) ([]model.BridgeSummary, error)
	BridgeDetails(ctx context.Context, req BridgeDetailsRequest) (model.BridgeDetails, error)
}

type BridgeQuoteRequest struct {
	FromChain       id.Chain
	ToChain         id.Chain
	FromAsset       id.Asset
	ToAsset         id.Asset
	AmountBaseUnits string
	AmountDecimal   string
}

type BridgeListRequest struct {
	Limit         int
	IncludeChains bool
}

type BridgeDetailsRequest struct {
	Bridge                string
	IncludeChainBreakdown bool
}

type SwapProvider interface {
	Provider
	QuoteSwap(ctx context.Context, req SwapQuoteRequest) (model.SwapQuote, error)
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
	TradeType       SwapTradeType
	SlippagePct     *float64
}
