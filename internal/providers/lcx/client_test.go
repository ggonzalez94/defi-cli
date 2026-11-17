package lcx

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

type quoteReq struct {
	ChainID  int64  `json:"chainId"`
	TokenIn  string `json:"tokenIn"`
	TokenOut string `json:"tokenOut"`
	AmountIn string `json:"amountIn"`
}

func quoteBody(chainID int64, spender, tokenIn, tokenOut, amountIn, amountOut string) string {
	return `{"success":true,"data":{"quote":{
		"quoteId":"q-1",
		"chainId":` + jsonInt(chainID) + `,
		"input":{"token":"` + tokenIn + `","amount":"` + amountIn + `","decimals":6},
		"output":{"token":"` + tokenOut + `","amount":"` + amountOut + `","decimals":18,"minimumAmount":"1"},
		"priceImpactPct":0.012,
		"gasFeeUSD":0.034,
		"gasEstimateWei":"120000000000",
		"spender":"` + spender + `",
		"venues":["uniswap_v3","aerodrome_slipstream"],
		"expiresAt":1790000120,
		"stale":false
	}}}`
}

func jsonInt(v int64) string {
	buf, _ := json.Marshal(v)
	return string(buf)
}

func newTestClient(t *testing.T, handler http.HandlerFunc) *Client {
	t.Helper()
	srv := httptest.NewServer(handler)
	t.Cleanup(srv.Close)
	c := New(httpx.New(1*time.Second, 0), "test-key")
	c.baseURL = srv.URL
	return c
}

func TestQuoteSwapSendsRequestAndParsesResponse(t *testing.T) {
	chain, _ := id.ParseChain("base")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("WETH", chain)

	var got quoteReq
	c := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "expected POST", http.StatusMethodNotAllowed)
			return
		}
		if r.URL.Path != "/v1/quote" {
			http.Error(w, "unexpected path", http.StatusNotFound)
			return
		}
		if r.Header.Get("Authorization") != "Bearer test-key" {
			http.Error(w, "missing API key header", http.StatusUnauthorized)
			return
		}
		if err := json.NewDecoder(r.Body).Decode(&got); err != nil {
			http.Error(w, "invalid JSON payload", http.StatusBadRequest)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, quoteBody(8453, strings.ToLower(router1), got.TokenIn, got.TokenOut, got.AmountIn, "312500000000000"))
	})
	fixedNow := time.Date(2026, time.November, 17, 9, 0, 0, 0, time.UTC)
	c.now = func() time.Time { return fixedNow }

	quote, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetIn,
		ToAsset:         assetOut,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}

	if got.ChainID != 8453 {
		t.Fatalf("unexpected chain id in payload: %d", got.ChainID)
	}
	if got.TokenIn != assetIn.Address || got.TokenOut != assetOut.Address {
		t.Fatalf("unexpected token addresses in payload: %+v", got)
	}
	if got.AmountIn != "1000000" {
		t.Fatalf("unexpected amountIn in payload: %s", got.AmountIn)
	}

	if quote.Provider != "lcx" {
		t.Fatalf("expected provider lcx, got %s", quote.Provider)
	}
	if quote.TradeType != "exact-input" {
		t.Fatalf("expected trade_type exact-input, got %s", quote.TradeType)
	}
	if quote.EstimatedOut.AmountBaseUnits != "312500000000000" || quote.EstimatedOut.AmountDecimal != "0.0003125" {
		t.Fatalf("unexpected output amount: %+v", quote.EstimatedOut)
	}
	if quote.EstimatedGasUSD != 0.034 {
		t.Fatalf("unexpected gas USD: %v", quote.EstimatedGasUSD)
	}
	if quote.PriceImpactPct != 0.012 {
		t.Fatalf("unexpected price impact: %v", quote.PriceImpactPct)
	}
	if quote.Route != "uniswap_v3, aerodrome_slipstream" {
		t.Fatalf("unexpected route: %s", quote.Route)
	}
	if quote.FetchedAt != fixedNow.Format(time.RFC3339) {
		t.Fatalf("unexpected fetched_at: %s", quote.FetchedAt)
	}
}

func TestQuoteSwapSendsZeroAddressForNativeCoin(t *testing.T) {
	chain, _ := id.ParseChain("hyperevm")
	assetIn, _ := id.ParseAsset("HYPE", chain)
	assetOut, _ := id.ParseAsset("USDC", chain)

	var got quoteReq
	c := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		if err := json.NewDecoder(r.Body).Decode(&got); err != nil {
			http.Error(w, "invalid JSON payload", http.StatusBadRequest)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, quoteBody(999, router2, got.TokenIn, got.TokenOut, got.AmountIn, "25000000"))
	})

	quote, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetIn,
		ToAsset:         assetOut,
		AmountBaseUnits: "1000000000000000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}
	if got.TokenIn != nativeToken {
		t.Fatalf("expected native coin as zero address, got %s", got.TokenIn)
	}
	if quote.FromAssetID != assetIn.AssetID {
		t.Fatalf("expected from asset id %s, got %s", assetIn.AssetID, quote.FromAssetID)
	}
	if quote.EstimatedOut.AmountDecimal != "25" {
		t.Fatalf("unexpected output decimal amount: %s", quote.EstimatedOut.AmountDecimal)
	}
}

func TestQuoteSwapRejectsMismatchedQuote(t *testing.T) {
	chain, _ := id.ParseChain("base")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("WETH", chain)
	other := "0x1111111111111111111111111111111111111111"

	cases := []struct {
		name string
		want string
		body func(req quoteReq) string
	}{
		{"wrong spender", "spender", func(req quoteReq) string {
			return quoteBody(8453, other, req.TokenIn, req.TokenOut, req.AmountIn, "1")
		}},
		{"other chain router", "spender", func(req quoteReq) string {
			return quoteBody(8453, router2, req.TokenIn, req.TokenOut, req.AmountIn, "1")
		}},
		{"missing spender", "spender", func(req quoteReq) string {
			return strings.Replace(quoteBody(8453, router1, req.TokenIn, req.TokenOut, req.AmountIn, "1"), `"spender":"`+router1+`"`, `"spender":null`, 1)
		}},
		{"wrong chain", "chain", func(req quoteReq) string {
			return quoteBody(1, router1, req.TokenIn, req.TokenOut, req.AmountIn, "1")
		}},
		{"wrong input amount", "input amount", func(req quoteReq) string {
			return quoteBody(8453, router1, req.TokenIn, req.TokenOut, "999", "1")
		}},
		{"wrong input token", "tokens", func(req quoteReq) string {
			return quoteBody(8453, router1, other, req.TokenOut, req.AmountIn, "1")
		}},
		{"wrong output token", "tokens", func(req quoteReq) string {
			return quoteBody(8453, router1, req.TokenIn, other, req.AmountIn, "1")
		}},
		{"missing output amount", "output amount", func(req quoteReq) string {
			return quoteBody(8453, router1, req.TokenIn, req.TokenOut, req.AmountIn, "")
		}},
		{"success false", "success=false", func(req quoteReq) string {
			return `{"success":false,"code":"no_route","error":"no_route","message":"No route"}`
		}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			c := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
				var req quoteReq
				if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
					http.Error(w, "invalid JSON payload", http.StatusBadRequest)
					return
				}
				w.Header().Set("Content-Type", "application/json")
				_, _ = io.WriteString(w, tc.body(req))
			})
			_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
				Chain: chain, FromAsset: assetIn, ToAsset: assetOut, AmountBaseUnits: "1000000", AmountDecimal: "1",
			})
			cliErr, ok := clierr.As(err)
			if !ok || cliErr.Code != clierr.CodeUnavailable || !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("expected unavailable error mentioning %q, got %v", tc.want, err)
			}
		})
	}
}

func TestQuoteSwapMapsUnauthorizedToAuthError(t *testing.T) {
	chain, _ := id.ParseChain("base")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("WETH", chain)
	c := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusUnauthorized)
		_, _ = io.WriteString(w, `{"success":false,"code":"INVALID_API_KEY","error":"INVALID_API_KEY","message":"Invalid API key"}`)
	})
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain: chain, FromAsset: assetIn, ToAsset: assetOut, AmountBaseUnits: "1000000", AmountDecimal: "1",
	})
	cliErr, ok := clierr.As(err)
	if !ok || cliErr.Code != clierr.CodeAuth {
		t.Fatalf("expected auth error, got %v", err)
	}
}

func TestQuoteSwapRequiresAPIKey(t *testing.T) {
	chain, _ := id.ParseChain("base")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("WETH", chain)
	called := false
	c := newTestClient(t, func(w http.ResponseWriter, r *http.Request) { called = true })
	c.apiKey = ""
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain: chain, FromAsset: assetIn, ToAsset: assetOut, AmountBaseUnits: "1000000", AmountDecimal: "1",
	})
	cliErr, ok := clierr.As(err)
	if !ok || cliErr.Code != clierr.CodeAuth {
		t.Fatalf("expected missing API key error, got %v", err)
	}
	if called {
		t.Fatal("expected no request without an API key")
	}
}

func TestQuoteSwapRejectsUnsupportedChains(t *testing.T) {
	for _, input := range []string{"taiko", "solana"} {
		chain, _ := id.ParseChain(input)
		assetIn, _ := id.ParseAsset("USDC", chain)
		assetOut, _ := id.ParseAsset("WETH", chain)
		c := New(httpx.New(1*time.Second, 0), "test-key")
		_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
			Chain: chain, FromAsset: assetIn, ToAsset: assetOut, AmountBaseUnits: "1000000", AmountDecimal: "1",
		})
		cliErr, ok := clierr.As(err)
		if !ok || cliErr.Code != clierr.CodeUnsupported {
			t.Fatalf("%s: expected unsupported error, got %v", input, err)
		}
	}
}

func TestQuoteSwapRejectsExactOutput(t *testing.T) {
	chain, _ := id.ParseChain("base")
	assetIn, _ := id.ParseAsset("USDC", chain)
	assetOut, _ := id.ParseAsset("WETH", chain)
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
