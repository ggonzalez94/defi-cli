package lifi

import (
	"context"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

func TestQuoteBridgeRejectsNonEVMChains(t *testing.T) {
	fromChain, _ := id.ParseChain("solana")
	toChain, _ := id.ParseChain("base")
	fromAsset, _ := id.ParseAsset("USDC", fromChain)
	toAsset, _ := id.ParseAsset("USDC", toChain)

	c := New(httpx.New(1*time.Second, 0))
	_, err := c.QuoteBridge(context.Background(), providers.BridgeQuoteRequest{
		FromChain:       fromChain,
		ToChain:         toChain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err == nil {
		t.Fatal("expected unsupported chain error")
	}
}
