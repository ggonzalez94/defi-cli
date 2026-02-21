package providers

import (
	"context"

	"github.com/gustavo/defi-cli/internal/id"
	"github.com/gustavo/defi-cli/internal/model"
)

type Provider interface {
	Info() model.ProviderInfo
}

type MarketDataProvider interface {
	Provider
	ChainsTop(ctx context.Context, limit int) ([]model.ChainTVL, error)
	ProtocolsTop(ctx context.Context, category string, limit int) ([]model.ProtocolTVL, error)
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

type BridgeQuoteRequest struct {
	FromChain       id.Chain
	ToChain         id.Chain
	FromAsset       id.Asset
	ToAsset         id.Asset
	AmountBaseUnits string
	AmountDecimal   string
}

type SwapProvider interface {
	Provider
	QuoteSwap(ctx context.Context, req SwapQuoteRequest) (model.SwapQuote, error)
}

type SwapQuoteRequest struct {
	Chain           id.Chain
	FromAsset       id.Asset
	ToAsset         id.Asset
	AmountBaseUnits string
	AmountDecimal   string
}
