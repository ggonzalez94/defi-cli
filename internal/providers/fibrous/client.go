package fibrous

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
	"sort"
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

const defaultBase = "https://api.fibrous.finance"

// chainSlugs maps EVM chain IDs to Fibrous API chain slug identifiers.
var chainSlugs = map[int64]string{
	999:  "hyperevm",
	4114: "citrea",
	8453: "base",
}

type Client struct {
	http    *httpx.Client
	baseURL string
	now     func() time.Time
}

func New(httpClient *httpx.Client) *Client {
	return &Client{
		http:    httpClient,
		baseURL: defaultBase,
		now:     time.Now,
	}
}

func (c *Client) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:         "fibrous",
		Type:         "swap",
		RequiresKey:  false,
		Capabilities: []string{"swap.quote"},
	}
}

type routeResponse struct {
	Success               bool     `json:"success"`
	OutputAmount          string   `json:"outputAmount"`
	EstimatedGasUsedInUsd *float64 `json:"estimatedGasUsedInUsd"`
}

func (c *Client) QuoteSwap(ctx context.Context, req providers.SwapQuoteRequest) (model.SwapQuote, error) {
	tradeType := req.TradeType
	if tradeType == "" {
		tradeType = providers.SwapTradeTypeExactInput
	}
	if tradeType != providers.SwapTradeTypeExactInput {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnsupported, "fibrous supports only --type exact-input")
	}

	chainSlug, ok := chainSlugs[req.Chain.EVMChainID]
	if !ok {
		supported := make([]string, 0, len(chainSlugs))
		for _, slug := range chainSlugs {
			supported = append(supported, slug)
		}
		sort.Strings(supported)
		return model.SwapQuote{}, clierr.New(clierr.CodeUnsupported,
			fmt.Sprintf("fibrous does not support chain %s (supported: %s)", req.Chain.Slug, strings.Join(supported, ", ")))
	}

	vals := url.Values{}
	vals.Set("amount", req.AmountBaseUnits)
	vals.Set("tokenInAddress", req.FromAsset.Address)
	vals.Set("tokenOutAddress", req.ToAsset.Address)

	endpoint := fmt.Sprintf("%s/%s/route?%s", c.baseURL, chainSlug, vals.Encode())
	hReq, err := http.NewRequestWithContext(ctx, http.MethodGet, endpoint, nil)
	if err != nil {
		return model.SwapQuote{}, clierr.Wrap(clierr.CodeInternal, "build fibrous route request", err)
	}

	var resp routeResponse
	if _, err := c.http.DoJSON(ctx, hReq, &resp); err != nil {
		return model.SwapQuote{}, err
	}

	if !resp.Success {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnavailable, "fibrous route returned success=false")
	}
	if resp.OutputAmount == "" {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnavailable, "fibrous route missing output amount")
	}
	estimatedGasUSD := 0.0
	if resp.EstimatedGasUsedInUsd != nil {
		estimatedGasUSD = *resp.EstimatedGasUsedInUsd
	}

	return model.SwapQuote{
		Provider:    "fibrous",
		ChainID:     req.Chain.CAIP2,
		FromAssetID: req.FromAsset.AssetID,
		ToAssetID:   req.ToAsset.AssetID,
		TradeType:   string(tradeType),
		InputAmount: model.AmountInfo{
			AmountBaseUnits: req.AmountBaseUnits,
			AmountDecimal:   req.AmountDecimal,
			Decimals:        req.FromAsset.Decimals,
		},
		EstimatedOut: model.AmountInfo{
			AmountBaseUnits: resp.OutputAmount,
			AmountDecimal:   id.FormatDecimalCompat(resp.OutputAmount, req.ToAsset.Decimals),
			Decimals:        req.ToAsset.Decimals,
		},
		EstimatedGasUSD: estimatedGasUSD,
		PriceImpactPct:  0,
		Route:           "fibrous",
		SourceURL:       "https://fibrous.finance",
		FetchedAt:       c.now().UTC().Format(time.RFC3339),
	}, nil
}
