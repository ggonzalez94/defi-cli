package uniswap

import (
	"context"
	"testing"
	"time"

	"github.com/gustavo/defi-cli/internal/httpx"
	"github.com/gustavo/defi-cli/internal/id"
	"github.com/gustavo/defi-cli/internal/providers"
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
