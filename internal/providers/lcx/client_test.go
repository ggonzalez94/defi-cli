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
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

var (
	testNow        = time.Date(2026, time.November, 17, 9, 0, 0, 0, time.UTC)
	testExpiresAt  = testNow.Unix() + 120
	routerBase, _  = registry.LCXLibertyRouter(8453)
	routerHyper, _ = registry.LCXLibertyRouter(999)
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
		"expiresAt":` + jsonInt(testExpiresAt) + `,
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
	c.now = func() time.Time { return testNow }
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
		_, _ = io.WriteString(w, quoteBody(8453, strings.ToLower(routerBase), got.TokenIn, got.TokenOut, got.AmountIn, "312500000000000"))
	})

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
	if quote.FetchedAt != testNow.Format(time.RFC3339) {
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
		_, _ = io.WriteString(w, quoteBody(999, routerHyper, got.TokenIn, got.TokenOut, got.AmountIn, "25000000"))
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

const (
	unichainUSDC = "0x078d782b760474a361dda0af3839290b0ef57ad6"
	unichainWETH = "0x4200000000000000000000000000000000000006"
)

func quotedDecimalsClient(t *testing.T, chainID int64, strip ...string) *Client {
	t.Helper()
	router, _ := registry.LCXLibertyRouter(chainID)
	return newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		var req quoteReq
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, "invalid JSON payload", http.StatusBadRequest)
			return
		}
		body := quoteBody(chainID, router, req.TokenIn, req.TokenOut, req.AmountIn, "800000000000000")
		for _, field := range strip {
			body = strings.Replace(body, field, ``, 1)
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, body)
	})
}

func TestQuoteSwapUsesQuotedDecimalsForUnregisteredTokens(t *testing.T) {
	chain, _ := id.ParseChain("130")
	assetIn, _ := id.ParseAsset(unichainUSDC, chain)
	assetOut, _ := id.ParseAsset(unichainWETH, chain)
	if assetIn.Decimals != 0 || assetOut.Decimals != 0 {
		t.Fatalf("expected unregistered tokens to parse without decimals, got %d/%d", assetIn.Decimals, assetOut.Decimals)
	}

	c := quotedDecimalsClient(t, 130)
	quote, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       assetIn,
		ToAsset:         assetOut,
		AmountBaseUnits: "2500000",
		// What the runner derives with its 18-decimal fallback; it must not be echoed.
		AmountDecimal: "0.0000000000025",
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}
	if quote.InputAmount.AmountBaseUnits != "2500000" || quote.InputAmount.AmountDecimal != "2.5" || quote.InputAmount.Decimals != 6 {
		t.Fatalf("unexpected input amount: %+v", quote.InputAmount)
	}
	if quote.EstimatedOut.AmountDecimal != "0.0008" || quote.EstimatedOut.Decimals != 18 {
		t.Fatalf("unexpected output amount: %+v", quote.EstimatedOut)
	}
}

func TestQuoteSwapRejectsMissingDecimalsForUnregisteredTokens(t *testing.T) {
	unichain, _ := id.ParseChain("130")
	uniIn, _ := id.ParseAsset(unichainUSDC, unichain)
	uniOut, _ := id.ParseAsset(unichainWETH, unichain)
	base, _ := id.ParseChain("base")
	baseIn, _ := id.ParseAsset("USDC", base)
	baseOut, _ := id.ParseAsset("WETH", base)
	const inDecimals, outDecimals = `,"decimals":6`, `,"decimals":18`

	cases := []struct {
		name    string
		chain   id.Chain
		in, out id.Asset
		strip   []string
		want    string
	}{
		{"input decimals missing", unichain, uniIn, uniOut, []string{inDecimals}, "input token decimals"},
		{"output decimals missing", unichain, uniIn, uniOut, []string{outDecimals}, "output token decimals"},
		{"registered tokens need no quoted decimals", base, baseIn, baseOut, []string{inDecimals, outDecimals}, ""},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			c := quotedDecimalsClient(t, tc.chain.EVMChainID, tc.strip...)
			quote, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
				Chain: tc.chain, FromAsset: tc.in, ToAsset: tc.out, AmountBaseUnits: "2500000", AmountDecimal: "2.5",
			})
			if tc.want == "" {
				if err != nil {
					t.Fatalf("QuoteSwap failed: %v", err)
				}
				if quote.InputAmount.Decimals != tc.in.Decimals || quote.EstimatedOut.Decimals != tc.out.Decimals {
					t.Fatalf("expected registry decimals, got %+v / %+v", quote.InputAmount, quote.EstimatedOut)
				}
				return
			}
			cliErr, ok := clierr.As(err)
			if !ok || cliErr.Code != clierr.CodeUnavailable || !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("expected unavailable error mentioning %q, got %v (quote %+v)", tc.want, err, quote)
			}
		})
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
			return quoteBody(8453, routerHyper, req.TokenIn, req.TokenOut, req.AmountIn, "1")
		}},
		{"missing spender", "spender", func(req quoteReq) string {
			return strings.Replace(quoteBody(8453, routerBase, req.TokenIn, req.TokenOut, req.AmountIn, "1"), `"spender":"`+routerBase+`"`, `"spender":null`, 1)
		}},
		{"wrong chain", "chain", func(req quoteReq) string {
			return quoteBody(1, routerBase, req.TokenIn, req.TokenOut, req.AmountIn, "1")
		}},
		{"wrong input amount", "input amount", func(req quoteReq) string {
			return quoteBody(8453, routerBase, req.TokenIn, req.TokenOut, "999", "1")
		}},
		{"wrong input token", "tokens", func(req quoteReq) string {
			return quoteBody(8453, routerBase, other, req.TokenOut, req.AmountIn, "1")
		}},
		{"wrong output token", "tokens", func(req quoteReq) string {
			return quoteBody(8453, routerBase, req.TokenIn, other, req.AmountIn, "1")
		}},
		{"missing output amount", "output amount", func(req quoteReq) string {
			return quoteBody(8453, routerBase, req.TokenIn, req.TokenOut, req.AmountIn, "")
		}},
		{"stale", "stale", func(req quoteReq) string {
			return strings.Replace(quoteBody(8453, routerBase, req.TokenIn, req.TokenOut, req.AmountIn, "1"), `"stale":false`, `"stale":true`, 1)
		}},
		{"expired", "expired", func(req quoteReq) string {
			return strings.Replace(quoteBody(8453, routerBase, req.TokenIn, req.TokenOut, req.AmountIn, "1"), `"expiresAt":`+jsonInt(testExpiresAt), `"expiresAt":`+jsonInt(testNow.Unix()-1), 1)
		}},
		{"missing expiry", "expired", func(req quoteReq) string {
			return strings.Replace(quoteBody(8453, routerBase, req.TokenIn, req.TokenOut, req.AmountIn, "1"), `"expiresAt":`+jsonInt(testExpiresAt)+`,`, ``, 1)
		}},
		{"output decimals too large", "decimals", func(req quoteReq) string {
			return strings.Replace(quoteBody(8453, routerBase, req.TokenIn, req.TokenOut, req.AmountIn, "1"), `"decimals":18`, `"decimals":1000000000`, 1)
		}},
		{"output decimals above uint256", "decimals", func(req quoteReq) string {
			return strings.Replace(quoteBody(8453, routerBase, req.TokenIn, req.TokenOut, req.AmountIn, "1"), `"decimals":18`, `"decimals":78`, 1)
		}},
		{"negative output decimals", "decimals", func(req quoteReq) string {
			return strings.Replace(quoteBody(8453, routerBase, req.TokenIn, req.TokenOut, req.AmountIn, "1"), `"decimals":18`, `"decimals":-1`, 1)
		}},
		{"input decimals too large", "decimals", func(req quoteReq) string {
			return strings.Replace(quoteBody(8453, routerBase, req.TokenIn, req.TokenOut, req.AmountIn, "1"), `"decimals":6`, `"decimals":1000000000`, 1)
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
