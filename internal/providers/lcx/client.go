package lcx

import (
	"context"
	"encoding/json"
	"fmt"
	"math/big"
	"net/http"
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

const defaultBase = "https://swap-api.lcx.com"

const (
	router1 = "0x0000000000b1ae06c96aB73FF4360d7AeAaB56F0"
	router2 = "0x0000000000654902B6Ed6cC240cc81CB4c4a3dff"

	nativeToken       = "0x0000000000000000000000000000000000000000"
	nativePlaceholder = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
)

// routers is the published LCX Liberty DEX router per supported chain ID.
// Quotes whose spender is not this address are rejected.
var routers = map[int64]string{
	1:     router1,
	10:    router1,
	8453:  router1,
	42161: router1,
	25:    router2,
	56:    router2,
	100:   router2,
	130:   router2,
	137:   router2,
	143:   router2,
	146:   router2,
	324:   router2,
	480:   router2,
	999:   router2,
	1868:  router2,
	4326:  router2,
	4663:  router2,
	5042:  router2,
	9745:  router2,
	43114: router2,
	57073: router2,
	59144: router2,
}

type Client struct {
	http    *httpx.Client
	baseURL string
	apiKey  string
	now     func() time.Time
}

func New(httpClient *httpx.Client, apiKey string) *Client {
	return &Client{http: httpClient, baseURL: defaultBase, apiKey: apiKey, now: time.Now}
}

func (c *Client) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:          "lcx",
		Type:          "swap",
		RequiresKey:   true,
		KeyEnvVarName: "DEFI_LCX_API_KEY",
		Capabilities: []string{
			"swap.quote",
		},
		CapabilityAuth: []model.ProviderCapabilityAuth{
			{
				Capability: "swap.quote",
				KeyEnvVar:  "DEFI_LCX_API_KEY",
			},
		},
	}
}

type quote struct {
	ChainID int64 `json:"chainId"`
	Input   struct {
		Token  string `json:"token"`
		Amount string `json:"amount"`
	} `json:"input"`
	Output struct {
		Token    string `json:"token"`
		Amount   string `json:"amount"`
		Decimals *int   `json:"decimals"`
	} `json:"output"`
	PriceImpactPct *float64 `json:"priceImpactPct"`
	GasFeeUSD      *float64 `json:"gasFeeUSD"`
	Spender        string   `json:"spender"`
	Venues         []string `json:"venues"`
}

type quoteResponse struct {
	Success bool `json:"success"`
	Data    struct {
		Quote quote `json:"quote"`
	} `json:"data"`
}

func (c *Client) QuoteSwap(ctx context.Context, req providers.SwapQuoteRequest) (model.SwapQuote, error) {
	tradeType, err := providers.ValidateTradeType(req.TradeType, "lcx", providers.SwapTradeTypeExactInput)
	if err != nil {
		return model.SwapQuote{}, err
	}
	if !req.Chain.IsEVM() {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnsupported, "lcx swap quotes support only EVM chains")
	}
	router, ok := routers[req.Chain.EVMChainID]
	if !ok {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnsupported, fmt.Sprintf("lcx does not support chain %s", req.Chain.CAIP2))
	}
	if c.apiKey == "" {
		return model.SwapQuote{}, clierr.New(clierr.CodeAuth, "missing required API key for lcx (DEFI_LCX_API_KEY)")
	}

	tokenIn := apiToken(req.FromAsset.Address)
	tokenOut := apiToken(req.ToAsset.Address)
	buf, err := json.Marshal(map[string]any{
		"chainId":  req.Chain.EVMChainID,
		"tokenIn":  tokenIn,
		"tokenOut": tokenOut,
		"amountIn": req.AmountBaseUnits,
	})
	if err != nil {
		return model.SwapQuote{}, clierr.Wrap(clierr.CodeInternal, "marshal lcx request", err)
	}

	headers := map[string]string{"Authorization": "Bearer " + c.apiKey}
	var resp quoteResponse
	if _, err := httpx.DoBodyJSON(ctx, c.http, http.MethodPost, c.baseURL+"/v1/quote", buf, headers, &resp); err != nil {
		return model.SwapQuote{}, err
	}
	if !resp.Success {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnavailable, "lcx quote returned success=false")
	}
	q := resp.Data.Quote
	if err := validateQuote(q, req.Chain.EVMChainID, router, tokenIn, tokenOut, req.AmountBaseUnits); err != nil {
		return model.SwapQuote{}, err
	}

	outDecimals := req.ToAsset.Decimals
	if outDecimals <= 0 && q.Output.Decimals != nil {
		outDecimals = *q.Output.Decimals
	}
	gasUSD := 0.0
	if q.GasFeeUSD != nil {
		gasUSD = *q.GasFeeUSD
	}
	priceImpact := 0.0
	if q.PriceImpactPct != nil {
		priceImpact = *q.PriceImpactPct
	}
	route := "lcx"
	if len(q.Venues) > 0 {
		route = strings.Join(q.Venues, ", ")
	}

	return model.SwapQuote{
		Provider:    "lcx",
		ChainID:     req.Chain.CAIP2,
		FromAssetID: req.FromAsset.AssetID,
		ToAssetID:   req.ToAsset.AssetID,
		TradeType:   string(tradeType),
		InputAmount: model.AmountInfo{
			AmountBaseUnits: req.AmountBaseUnits,
			AmountDecimal:   req.AmountDecimal,
			Decimals:        req.FromAsset.Decimals,
		},
		EstimatedOut:    providers.AmountInfoFromBase(q.Output.Amount, outDecimals),
		EstimatedGasUSD: gasUSD,
		PriceImpactPct:  priceImpact,
		Route:           route,
		SourceURL:       "https://dex.lcx.com",
		FetchedAt:       c.now().UTC().Format(time.RFC3339),
	}, nil
}

// validateQuote rejects a quote that does not answer the request that was sent
// or that would settle through a contract other than the published router.
func validateQuote(q quote, chainID int64, router, tokenIn, tokenOut, amountIn string) error {
	wantIn, okWant := new(big.Int).SetString(amountIn, 10)
	gotIn, okGot := new(big.Int).SetString(strings.TrimSpace(q.Input.Amount), 10)
	switch {
	case q.ChainID != chainID:
		return clierr.New(clierr.CodeUnavailable, "lcx quote chain does not match request")
	case !strings.EqualFold(strings.TrimSpace(q.Spender), router):
		return clierr.New(clierr.CodeUnavailable, "lcx quote spender is not the published router")
	case !okWant || !okGot || wantIn.Cmp(gotIn) != 0:
		return clierr.New(clierr.CodeUnavailable, "lcx quote input amount does not match request")
	case !strings.EqualFold(strings.TrimSpace(q.Input.Token), tokenIn) || !strings.EqualFold(strings.TrimSpace(q.Output.Token), tokenOut):
		return clierr.New(clierr.CodeUnavailable, "lcx quote tokens do not match request")
	case q.Output.Amount == "" || providers.NormalizeBaseUnits(q.Output.Amount) != q.Output.Amount:
		return clierr.New(clierr.CodeUnavailable, "lcx quote missing output amount")
	}
	return nil
}

// apiToken maps the 0xEeee native placeholder used by the token registry to
// the zero address the LCX API expects for the native coin.
func apiToken(address string) string {
	addr := strings.TrimSpace(address)
	if strings.EqualFold(addr, nativePlaceholder) {
		return nativeToken
	}
	return addr
}
