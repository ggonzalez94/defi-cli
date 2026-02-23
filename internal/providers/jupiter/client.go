package jupiter

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

const (
	defaultLiteBase    = "https://lite-api.jup.ag/swap/v1"
	defaultProBase     = "https://api.jup.ag/swap/v1"
	solanaMainnetCAIP2 = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"
)

type Client struct {
	http    *httpx.Client
	baseURL string
	apiKey  string
	now     func() time.Time
}

func New(httpClient *httpx.Client, apiKey string) *Client {
	apiKey = strings.TrimSpace(apiKey)
	baseURL := defaultLiteBase
	if apiKey != "" {
		baseURL = defaultProBase
	}
	return &Client{
		http:    httpClient,
		baseURL: baseURL,
		apiKey:  apiKey,
		now:     time.Now,
	}
}

func (c *Client) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:          "jupiter",
		Type:          "swap",
		RequiresKey:   false,
		KeyEnvVarName: "DEFI_JUPITER_API_KEY",
		Capabilities: []string{
			"swap.quote",
		},
		CapabilityAuth: []model.ProviderCapabilityAuth{
			{
				Capability:  "swap.quote",
				KeyEnvVar:   "DEFI_JUPITER_API_KEY",
				Description: "Optional API key for higher Jupiter API limits",
			},
		},
	}
}

type quoteResponse struct {
	OutAmount      string `json:"outAmount"`
	PriceImpactPct string `json:"priceImpactPct"`
	RoutePlan      []struct {
		SwapInfo struct {
			Label string `json:"label"`
		} `json:"swapInfo"`
	} `json:"routePlan"`
}

func (c *Client) QuoteSwap(ctx context.Context, req providers.SwapQuoteRequest) (model.SwapQuote, error) {
	if !req.Chain.IsSolana() {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnsupported, "jupiter swap quotes support only Solana chains")
	}
	if req.Chain.CAIP2 != solanaMainnetCAIP2 {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnsupported, "jupiter swap quotes support only Solana mainnet")
	}

	vals := url.Values{}
	vals.Set("inputMint", req.FromAsset.Address)
	vals.Set("outputMint", req.ToAsset.Address)
	vals.Set("amount", req.AmountBaseUnits)
	vals.Set("slippageBps", "50")

	endpoint := fmt.Sprintf("%s/quote?%s", strings.TrimRight(c.baseURL, "/"), vals.Encode())
	hReq, err := http.NewRequestWithContext(ctx, http.MethodGet, endpoint, nil)
	if err != nil {
		return model.SwapQuote{}, clierr.Wrap(clierr.CodeInternal, "build jupiter quote request", err)
	}
	if c.apiKey != "" {
		hReq.Header.Set("x-api-key", c.apiKey)
	}

	var resp quoteResponse
	if _, err := c.http.DoJSON(ctx, hReq, &resp); err != nil {
		return model.SwapQuote{}, err
	}
	if strings.TrimSpace(resp.OutAmount) == "" {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnavailable, "jupiter quote missing output amount")
	}

	return model.SwapQuote{
		Provider:    "jupiter",
		ChainID:     req.Chain.CAIP2,
		FromAssetID: req.FromAsset.AssetID,
		ToAssetID:   req.ToAsset.AssetID,
		InputAmount: model.AmountInfo{
			AmountBaseUnits: req.AmountBaseUnits,
			AmountDecimal:   req.AmountDecimal,
			Decimals:        req.FromAsset.Decimals,
		},
		EstimatedOut: model.AmountInfo{
			AmountBaseUnits: resp.OutAmount,
			AmountDecimal:   id.FormatDecimalCompat(resp.OutAmount, req.ToAsset.Decimals),
			Decimals:        req.ToAsset.Decimals,
		},
		EstimatedGasUSD: 0,
		PriceImpactPct:  parsePriceImpactPct(resp.PriceImpactPct),
		Route:           routeFromPlan(resp.RoutePlan),
		SourceURL:       "https://jup.ag",
		FetchedAt:       c.now().UTC().Format(time.RFC3339),
	}, nil
}

func parsePriceImpactPct(v string) float64 {
	f, err := strconv.ParseFloat(strings.TrimSpace(v), 64)
	if err != nil {
		return 0
	}
	if f < 0 {
		return 0
	}
	return f
}

func routeFromPlan(plan []struct {
	SwapInfo struct {
		Label string `json:"label"`
	} `json:"swapInfo"`
}) string {
	if len(plan) == 0 {
		return "jupiter"
	}

	parts := make([]string, 0, len(plan))
	for _, hop := range plan {
		label := strings.TrimSpace(hop.SwapInfo.Label)
		if label == "" {
			continue
		}
		if len(parts) == 0 || parts[len(parts)-1] != label {
			parts = append(parts, label)
		}
	}
	if len(parts) == 0 {
		return "jupiter"
	}
	return strings.Join(parts, " > ")
}
