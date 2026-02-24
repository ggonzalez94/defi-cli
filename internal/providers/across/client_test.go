package across

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

func TestBaseUnitMathHelpers(t *testing.T) {
	if compareBaseUnits("100", "99") <= 0 {
		t.Fatal("compareBaseUnits expected 100 > 99")
	}
	if out := subtractBaseUnits("1000", "1"); out != "999" {
		t.Fatalf("unexpected subtraction result: %s", out)
	}
	if out := subtractBaseUnits("1", "2"); out != "0" {
		t.Fatalf("unexpected underflow result: %s", out)
	}
}

func TestQuoteBridgeAcrossFeeBreakdownAndConsistency(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/limits":
			_, _ = w.Write([]byte(`{
				"minDeposit":"500007",
				"maxDeposit":"1954894537806"
			}`))
		case "/suggested-fees":
			_, _ = w.Write([]byte(`{
				"relayFeeTotal":"2633",
				"relayGasFeeTotal":"2533",
				"capitalFeeTotal":"100",
				"lpFee":{"total":"0"},
				"outputAmount":"997367",
				"estimatedFillTimeSec":5
			}`))
		default:
			t.Fatalf("unexpected path: %s", r.URL.Path)
		}
	}))
	defer srv.Close()

	fromChain, _ := id.ParseChain("ethereum")
	toChain, _ := id.ParseChain("base")
	fromAsset, _ := id.ParseAsset("USDC", fromChain)
	toAsset, _ := id.ParseAsset("USDC", toChain)

	c := New(httpx.New(time.Second, 0))
	c.baseURL = srv.URL

	got, err := c.QuoteBridge(context.Background(), providers.BridgeQuoteRequest{
		FromChain:       fromChain,
		ToChain:         toChain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteBridge failed: %v", err)
	}
	if got.EstimatedOut.AmountBaseUnits != "997367" {
		t.Fatalf("unexpected estimated out: %s", got.EstimatedOut.AmountBaseUnits)
	}
	if got.EstimatedFeeUSD <= 0 {
		t.Fatalf("expected non-zero fee usd fallback for stable asset, got %f", got.EstimatedFeeUSD)
	}
	if got.FeeBreakdown == nil {
		t.Fatal("expected fee breakdown")
	}
	if got.FeeBreakdown.TotalFeeBaseUnits != "2633" {
		t.Fatalf("unexpected total fee base units: %s", got.FeeBreakdown.TotalFeeBaseUnits)
	}
	if got.FeeBreakdown.GasFee == nil || got.FeeBreakdown.GasFee.AmountBaseUnits != "2533" {
		t.Fatalf("unexpected gas fee breakdown: %+v", got.FeeBreakdown.GasFee)
	}
	if got.FeeBreakdown.RelayerFee == nil || got.FeeBreakdown.RelayerFee.AmountBaseUnits != "100" {
		t.Fatalf("unexpected relayer fee breakdown: %+v", got.FeeBreakdown.RelayerFee)
	}
	if got.FeeBreakdown.ConsistentWithAmountDelta == nil || !*got.FeeBreakdown.ConsistentWithAmountDelta {
		t.Fatalf("expected consistency check true, got %+v", got.FeeBreakdown.ConsistentWithAmountDelta)
	}
}

func TestQuoteBridgeDoesNotTreatRelayFeePctAsBaseUnits(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/limits":
			_, _ = w.Write([]byte(`{
				"minDeposit":"1",
				"maxDeposit":"1954894537806"
			}`))
		case "/suggested-fees":
			_, _ = w.Write([]byte(`{
				"relayFeePct":"0.003",
				"feeUsd":1.23,
				"estimatedFillTimeSec":5
			}`))
		default:
			t.Fatalf("unexpected path: %s", r.URL.Path)
		}
	}))
	defer srv.Close()

	fromChain, _ := id.ParseChain("ethereum")
	toChain, _ := id.ParseChain("base")
	fromAsset, _ := id.ParseAsset("USDC", fromChain)
	toAsset, _ := id.ParseAsset("USDC", toChain)

	c := New(httpx.New(time.Second, 0))
	c.baseURL = srv.URL

	got, err := c.QuoteBridge(context.Background(), providers.BridgeQuoteRequest{
		FromChain:       fromChain,
		ToChain:         toChain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteBridge failed: %v", err)
	}
	if got.EstimatedOut.AmountBaseUnits != "1000000" {
		t.Fatalf("expected estimated out to remain input amount when only relayFeePct is present, got %s", got.EstimatedOut.AmountBaseUnits)
	}
	if got.EstimatedFeeUSD != 1.23 {
		t.Fatalf("unexpected fee usd: %f", got.EstimatedFeeUSD)
	}
	if got.FeeBreakdown == nil {
		t.Fatal("expected fee breakdown when fee usd is present")
	}
	if got.FeeBreakdown.TotalFeeBaseUnits != "" {
		t.Fatalf("expected no canonical total fee base units when absolute fee is unavailable, got %q", got.FeeBreakdown.TotalFeeBaseUnits)
	}
	if got.FeeBreakdown.TotalFeeDecimal != "" {
		t.Fatalf("expected no total fee decimal without canonical base units, got %q", got.FeeBreakdown.TotalFeeDecimal)
	}
	if got.FeeBreakdown.ConsistentWithAmountDelta != nil {
		t.Fatalf("expected consistency check to be omitted when output amount is not provider-reported, got %+v", got.FeeBreakdown.ConsistentWithAmountDelta)
	}
}

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

func TestApproximateStableUSDExcludesEURS(t *testing.T) {
	if isLikelyStableSymbol("EURS") {
		t.Fatal("EURS should not be treated as USD-pegged")
	}
	if got := approximateStableUSD("EURS", "1000000", 6); got != 0 {
		t.Fatalf("expected EURS USD approximation to be disabled, got %f", got)
	}
}
