package uniswap

import (
	"context"
	"encoding/json"
	"net/http"
	"strconv"
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

const defaultBase = "https://trade-api.gateway.uniswap.org"

// quoteOnlySwapper is a deterministic placeholder for quote retrieval flows.
const quoteOnlySwapper = "0x0000000000000000000000000000000000000001"

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
		CapabilityAuth: []model.ProviderCapabilityAuth{
			{
				Capability: "swap.quote",
				KeyEnvVar:  "DEFI_UNISWAP_API_KEY",
			},
		},
	}
}

type quoteResponse struct {
	Quote struct {
		Input struct {
			Amount string `json:"amount"`
		} `json:"input"`
		Output struct {
			Amount string `json:"amount"`
		} `json:"output"`
		GasFeeUSD json.RawMessage `json:"gasFeeUSD"`
	} `json:"quote"`
	AmountIn  string          `json:"amountIn"`
	AmountOut string          `json:"amountOut"`
	GasUSD    json.RawMessage `json:"gasUSD"`
}

func (c *Client) QuoteSwap(ctx context.Context, req providers.SwapQuoteRequest) (model.SwapQuote, error) {
	if !req.Chain.IsEVM() {
		return model.SwapQuote{}, clierr.New(clierr.CodeUnsupported, "uniswap swap quotes support only EVM chains")
	}
	if c.apiKey == "" {
		return model.SwapQuote{}, clierr.New(clierr.CodeAuth, "missing required API key for uniswap (DEFI_UNISWAP_API_KEY)")
	}

	tradeType := req.TradeType
	if tradeType == "" {
		tradeType = providers.SwapTradeTypeExactInput
	}
	switch tradeType {
	case providers.SwapTradeTypeExactInput, providers.SwapTradeTypeExactOutput:
	default:
		return model.SwapQuote{}, clierr.New(clierr.CodeUnsupported, "uniswap swap type must be exact-input or exact-output")
	}

	payload := map[string]any{
		"tokenInChainId":  req.Chain.EVMChainID,
		"tokenOutChainId": req.Chain.EVMChainID,
		"tokenIn":         req.FromAsset.Address,
		"tokenOut":        req.ToAsset.Address,
		"amount":          req.AmountBaseUnits,
		"type":            uniswapTradeType(tradeType),
		"swapper":         quoteOnlySwapper,
	}
	if req.SlippagePct != nil {
		payload["slippageTolerance"] = *req.SlippagePct
	} else {
		payload["autoSlippage"] = "DEFAULT"
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

	inputAmountBase := req.AmountBaseUnits
	inputAmountDecimal := req.AmountDecimal
	if tradeType == providers.SwapTradeTypeExactOutput {
		inputAmountBase = resp.AmountIn
		if inputAmountBase == "" {
			inputAmountBase = resp.Quote.Input.Amount
		}
		if inputAmountBase == "" {
			return model.SwapQuote{}, clierr.New(clierr.CodeUnavailable, "uniswap exact-output quote missing input amount")
		}
		inputAmountDecimal = id.FormatDecimalCompat(inputAmountBase, req.FromAsset.Decimals)
	}

	gasUSD, err := parseJSONFloat(resp.GasUSD)
	if err != nil {
		return model.SwapQuote{}, clierr.Wrap(clierr.CodeUnavailable, "decode uniswap gasUSD", err)
	}
	if gasUSD == 0 {
		gasUSD, err = parseJSONFloat(resp.Quote.GasFeeUSD)
		if err != nil {
			return model.SwapQuote{}, clierr.Wrap(clierr.CodeUnavailable, "decode uniswap quote.gasFeeUSD", err)
		}
	}

	return model.SwapQuote{
		Provider:    "uniswap",
		ChainID:     req.Chain.CAIP2,
		FromAssetID: req.FromAsset.AssetID,
		ToAssetID:   req.ToAsset.AssetID,
		TradeType:   string(tradeType),
		InputAmount: model.AmountInfo{
			AmountBaseUnits: inputAmountBase,
			AmountDecimal:   inputAmountDecimal,
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

func parseJSONFloat(raw json.RawMessage) (float64, error) {
	if len(raw) == 0 {
		return 0, nil
	}
	trimmed := strings.TrimSpace(string(raw))
	if trimmed == "" || trimmed == "null" {
		return 0, nil
	}

	var value float64
	if err := json.Unmarshal(raw, &value); err == nil {
		return value, nil
	}

	var valueStr string
	if err := json.Unmarshal(raw, &valueStr); err == nil {
		parsed, parseErr := strconv.ParseFloat(valueStr, 64)
		if parseErr != nil {
			return 0, parseErr
		}
		return parsed, nil
	}

	return 0, clierr.New(clierr.CodeUnavailable, "expected numeric or string-encoded numeric value")
}

func uniswapTradeType(t providers.SwapTradeType) string {
	if t == providers.SwapTradeTypeExactOutput {
		return "EXACT_OUTPUT"
	}
	return "EXACT_INPUT"
}
