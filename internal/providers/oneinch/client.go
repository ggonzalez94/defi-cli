package oneinch

import (
	"context"
	"fmt"
	"net/url"
	"strconv"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

const defaultBase = "https://api.1inch.dev"

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
		Name:          "1inch",
		Type:          "swap",
		RequiresKey:   true,
		KeyEnvVarName: "DEFI_1INCH_API_KEY",
		Capabilities: []string{
			"swap.quote",
		},
		CapabilityAuth: []model.ProviderCapabilityAuth{
			{
				Capability: "swap.quote",
				KeyEnvVar:  "DEFI_1INCH_API_KEY",
			},
		},
	}
}

type quoteResponse struct {
	DstAmount string  `json:"dstAmount"`
	Gas       float64 `json:"gas"`
}

func (c *Client) QuoteSwap(ctx context.Context, req providers.SwapQuoteRequest) (model.SwapQuote, error) {
	tradeType := req.TradeType
	if tradeType == "" {
		tradeType = providers.SwapTradeTypeExactInput
	}
	if tradeType != providers.SwapTradeTypeExactInput {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnsupported, "1inch supports only --type exact-input")
	}

	if !req.Chain.IsEVM() {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnsupported, "1inch swap quotes support only EVM chains")
	}
	if c.apiKey == "" {
		return model.SwapQuote{}, clierr.New(clierr.CodeAuth, "missing required API key for 1inch (DEFI_1INCH_API_KEY)")
	}
	chainID := strconv.FormatInt(req.Chain.EVMChainID, 10)
	vals := url.Values{}
	vals.Set("src", req.FromAsset.Address)
	vals.Set("dst", req.ToAsset.Address)
	vals.Set("amount", req.AmountBaseUnits)
	vals.Set("includeGas", "true")

	var resp quoteResponse
	if err := c.http.GetJSON(ctx, fmt.Sprintf("%s/swap/v6.0/%s/quote?%s", c.baseURL, chainID, vals.Encode()), map[string]string{"Authorization": "Bearer " + c.apiKey}, &resp); err != nil {
		return model.SwapQuote{}, err
	}
	if resp.DstAmount == "" {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnavailable, "1inch quote missing destination amount")
	}

	return model.SwapQuote{
		Provider:    "1inch",
		ChainID:     req.Chain.CAIP2,
		FromAssetID: req.FromAsset.AssetID,
		ToAssetID:   req.ToAsset.AssetID,
		TradeType:   string(tradeType),
		InputAmount: model.AmountInfo{
			AmountBaseUnits: req.AmountBaseUnits,
			AmountDecimal:   req.AmountDecimal,
			Decimals:        req.FromAsset.Decimals,
		},
		EstimatedOut: providers.AmountInfoFromBase(resp.DstAmount, req.ToAsset.Decimals),
		EstimatedGasUSD: 0,
		PriceImpactPct:  0,
		Route:           "1inch",
		SourceURL:       "https://app.1inch.io",
		FetchedAt:       c.now().UTC().Format(time.RFC3339),
	}, nil
}
