package uniswap

import (
	"context"
	"encoding/json"
	"net/http"
	"time"

	clierr "github.com/gustavo/defi-cli/internal/errors"
	"github.com/gustavo/defi-cli/internal/httpx"
	"github.com/gustavo/defi-cli/internal/id"
	"github.com/gustavo/defi-cli/internal/model"
	"github.com/gustavo/defi-cli/internal/providers"
)

const defaultBase = "https://trade-api.gateway.uniswap.org"

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
		Name:          "uniswap",
		Type:          "swap",
		RequiresKey:   true,
		KeyEnvVarName: "DEFI_UNISWAP_API_KEY",
		Capabilities: []string{
			"swap.quote",
		},
	}
}

type quoteResponse struct {
	Quote struct {
		Output struct {
			Amount string `json:"amount"`
		} `json:"output"`
		GasFeeUSD float64 `json:"gasFeeUSD"`
	} `json:"quote"`
	AmountOut string  `json:"amountOut"`
	GasUSD    float64 `json:"gasUSD"`
}

func (c *Client) QuoteSwap(ctx context.Context, req providers.SwapQuoteRequest) (model.SwapQuote, error) {
	if c.apiKey == "" {
		return model.SwapQuote{}, clierr.New(clierr.CodeAuth, "missing required API key for uniswap (DEFI_UNISWAP_API_KEY)")
	}

	payload := map[string]any{
		"tokenInChainId":  req.Chain.EVMChainID,
		"tokenOutChainId": req.Chain.EVMChainID,
		"tokenIn":         req.FromAsset.Address,
		"tokenOut":        req.ToAsset.Address,
		"amount":          req.AmountBaseUnits,
		"type":            "EXACT_INPUT",
	}
	buf, err := json.Marshal(payload)
	if err != nil {
		return model.SwapQuote{}, clierr.Wrap(clierr.CodeInternal, "marshal uniswap request", err)
	}

	headers := map[string]string{
		"x-api-key": c.apiKey,
	}
	var resp quoteResponse
	if _, err := httpx.DoBodyJSON(ctx, c.http, http.MethodPost, c.baseURL+"/v1/quote", buf, headers, &resp); err != nil {
		return model.SwapQuote{}, err
	}

	amountOut := resp.AmountOut
	if amountOut == "" {
		amountOut = resp.Quote.Output.Amount
	}
	if amountOut == "" {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnavailable, "uniswap quote missing output amount")
	}
	gasUSD := resp.GasUSD
	if gasUSD == 0 {
		gasUSD = resp.Quote.GasFeeUSD
	}

	return model.SwapQuote{
		Provider:    "uniswap",
		ChainID:     req.Chain.CAIP2,
		FromAssetID: req.FromAsset.AssetID,
		ToAssetID:   req.ToAsset.AssetID,
		InputAmount: model.AmountInfo{
			AmountBaseUnits: req.AmountBaseUnits,
			AmountDecimal:   req.AmountDecimal,
			Decimals:        req.FromAsset.Decimals,
		},
		EstimatedOut: model.AmountInfo{
			AmountBaseUnits: amountOut,
			AmountDecimal:   id.FormatDecimalCompat(amountOut, req.ToAsset.Decimals),
			Decimals:        req.ToAsset.Decimals,
		},
		EstimatedGasUSD: gasUSD,
		PriceImpactPct:  0,
		Route:           "uniswap",
		SourceURL:       "https://app.uniswap.org",
		FetchedAt:       c.now().UTC().Format(time.RFC3339),
	}, nil
}
