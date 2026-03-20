package oneinch

import (
	"context"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

func TestQuoteSwapRequiresAPIKey(t *testing.T) {
	chain, _ := id.ParseChain("ethereum")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("DAI", chain)
	c := New(httpx.New(1*time.Second, 0), "")
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain: chain, FromAsset: assetIn, ToAsset: assetOut, AmountBaseUnits: "1000000", AmountDecimal: "1",
	})
	if err == nil {
		t.Fatal("expected missing API key error")
	}
}

func TestQuoteSwapRejectsNonEVMChain(t *testing.T) {
	chain, _ := id.ParseChain("solana")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("USDT", chain)
	c := New(httpx.New(1*time.Second, 0), "")
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain: chain, FromAsset: assetIn, ToAsset: assetOut, AmountBaseUnits: "1000000", AmountDecimal: "1",
	})
	if err == nil {
		t.Fatal("expected unsupported chain error")
	}
}

func TestQuoteSwapRejectsExactOutput(t *testing.T) {
	chain, _ := id.ParseChain("ethereum")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("DAI", chain)
	c := New(httpx.New(1*time.Second, 0), "test-key")
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetIn,
		ToAsset:         assetOut,
		AmountBaseUnits: "1000000000000000000",
		AmountDecimal:   "1",
		TradeType:       providers.SwapTradeTypeExactOutput,
	})
	if err == nil {
		t.Fatal("expected unsupported exact-output error")
	}
}
