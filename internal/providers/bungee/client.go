package bungee

import (
	"context"
	"net/http"
	"net/url"
	"sort"
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
	defaultBase           = "https://public-backend.bungee.exchange/api/v1"
	defaultDedicatedBase  = "https://dedicated-backend.bungee.exchange/api/v1"
	defaultEVMUserAddress = "0x0000000000000000000000000000000000000001"
)

type mode string

const (
	modeBridge mode = "bridge"
	modeSwap   mode = "swap"
)

type Client struct {
	http             *httpx.Client
	baseURL          string
	dedicatedBaseURL string
	apiKey           string
	affiliate        string
	mode             mode
	now              func() time.Time
}

func NewBridge(httpClient *httpx.Client, apiKey, affiliate string) *Client {
	return &Client{
		http:             httpClient,
		baseURL:          defaultBase,
		dedicatedBaseURL: defaultDedicatedBase,
		apiKey:           apiKey,
		affiliate:        affiliate,
		mode:             modeBridge,
		now:              time.Now,
	}
}

func NewSwap(httpClient *httpx.Client, apiKey, affiliate string) *Client {
	return &Client{
		http:             httpClient,
		baseURL:          defaultBase,
		dedicatedBaseURL: defaultDedicatedBase,
		apiKey:           apiKey,
		affiliate:        affiliate,
		mode:             modeSwap,
		now:              time.Now,
	}
}

func (c *Client) Info() model.ProviderInfo {
	if c.mode == modeSwap {
		return model.ProviderInfo{
			Name:        "bungee",
			Type:        "swap",
			RequiresKey: false,
			Capabilities: []string{
				"swap.quote",
			},
			CapabilityAuth: []model.ProviderCapabilityAuth{
				{
					Capability:  "swap.quote",
					KeyEnvVar:   "DEFI_BUNGEE_API_KEY",
					Description: "Optional dedicated backend mode (requires both API key and affiliate)",
				},
				{
					Capability:  "swap.quote",
					KeyEnvVar:   "DEFI_BUNGEE_AFFILIATE",
					Description: "Optional dedicated backend mode (requires both API key and affiliate)",
				},
			},
		}
	}
	return model.ProviderInfo{
		Name:        "bungee",
		Type:        "bridge",
		RequiresKey: false,
		Capabilities: []string{
			"bridge.quote",
		},
		CapabilityAuth: []model.ProviderCapabilityAuth{
			{
				Capability:  "bridge.quote",
				KeyEnvVar:   "DEFI_BUNGEE_API_KEY",
				Description: "Optional dedicated backend mode (requires both API key and affiliate)",
			},
			{
				Capability:  "bridge.quote",
				KeyEnvVar:   "DEFI_BUNGEE_AFFILIATE",
				Description: "Optional dedicated backend mode (requires both API key and affiliate)",
			},
		},
	}
}

type quoteResponse struct {
	Success bool        `json:"success"`
	Result  quoteResult `json:"result"`
	Error   any         `json:"error"`
}

type quoteResult struct {
	OriginChainID      int64           `json:"originChainId"`
	DestinationChainID int64           `json:"destinationChainId"`
	Output             quoteOutput     `json:"output"`
	AutoRoute          *quoteAutoRoute `json:"autoRoute"`
	UserTxs            []quoteUserTx   `json:"userTxs"`
}

type quoteOutput struct {
	Amount   string `json:"amount"`
	Decimals int    `json:"decimals"`
	Token    struct {
		Decimals int `json:"decimals"`
	} `json:"token"`
}

type quoteAutoRoute struct {
	Output        quoteOutput   `json:"output"`
	OutputAmount  string        `json:"outputAmount"`
	EstimatedTime int64         `json:"estimatedTime"`
	GasFee        *quoteGasFee  `json:"gasFee"`
	RouteDetails  quoteDetails  `json:"routeDetails"`
	UserTxs       []quoteUserTx `json:"userTxs"`
}

type quoteGasFee struct {
	FeeInUSD float64 `json:"feeInUsd"`
}

type quoteUserTx struct {
	StepType     string             `json:"stepType"`
	RouteDetails quoteDetails       `json:"routeDetails"`
	SwapRoutes   []quoteSwapRoute   `json:"swapRoutes"`
	BridgeRoutes []quoteBridgeRoute `json:"bridgeRoutes"`
}

type quoteDetails struct {
	Name string `json:"name"`
}

type quoteSwapRoute struct {
	UsedDexName string `json:"usedDexName"`
}

type quoteBridgeRoute struct {
	UsedBridgeNames []string `json:"usedBridgeNames"`
}

func (c *Client) QuoteBridge(ctx context.Context, req providers.BridgeQuoteRequest) (model.BridgeQuote, error) {
	resp, err := c.quote(ctx, req.FromChain, req.ToChain, req.FromAsset.Address, req.ToAsset.Address, req.AmountBaseUnits)
	if err != nil {
		return model.BridgeQuote{}, err
	}
	outAmount, outDecimals, feeUSD, serviceTime, route, err := summarizeQuote(resp, req.ToAsset.Decimals)
	if err != nil {
		return model.BridgeQuote{}, err
	}

	return model.BridgeQuote{
		Provider:    "bungee",
		FromChainID: req.FromChain.CAIP2,
		ToChainID:   req.ToChain.CAIP2,
		FromAssetID: req.FromAsset.AssetID,
		ToAssetID:   req.ToAsset.AssetID,
		InputAmount: model.AmountInfo{
			AmountBaseUnits: req.AmountBaseUnits,
			AmountDecimal:   req.AmountDecimal,
			Decimals:        req.FromAsset.Decimals,
		},
		EstimatedOut: model.AmountInfo{
			AmountBaseUnits: outAmount,
			AmountDecimal:   id.FormatDecimalCompat(outAmount, outDecimals),
			Decimals:        outDecimals,
		},
		EstimatedFeeUSD: feeUSD,
		EstimatedTimeS:  serviceTime,
		Route:           route,
		SourceURL:       "https://www.bungee.exchange",
		FetchedAt:       c.now().UTC().Format(time.RFC3339),
	}, nil
}

func (c *Client) QuoteSwap(ctx context.Context, req providers.SwapQuoteRequest) (model.SwapQuote, error) {
	resp, err := c.quote(ctx, req.Chain, req.Chain, req.FromAsset.Address, req.ToAsset.Address, req.AmountBaseUnits)
	if err != nil {
		return model.SwapQuote{}, err
	}
	outAmount, outDecimals, feeUSD, _, route, err := summarizeQuote(resp, req.ToAsset.Decimals)
	if err != nil {
		return model.SwapQuote{}, err
	}

	return model.SwapQuote{
		Provider:    "bungee",
		ChainID:     req.Chain.CAIP2,
		FromAssetID: req.FromAsset.AssetID,
		ToAssetID:   req.ToAsset.AssetID,
		InputAmount: model.AmountInfo{
			AmountBaseUnits: req.AmountBaseUnits,
			AmountDecimal:   req.AmountDecimal,
			Decimals:        req.FromAsset.Decimals,
		},
		EstimatedOut: model.AmountInfo{
			AmountBaseUnits: outAmount,
			AmountDecimal:   id.FormatDecimalCompat(outAmount, outDecimals),
			Decimals:        outDecimals,
		},
		EstimatedGasUSD: feeUSD,
		PriceImpactPct:  0,
		Route:           route,
		SourceURL:       "https://www.bungee.exchange",
		FetchedAt:       c.now().UTC().Format(time.RFC3339),
	}, nil
}

func (c *Client) quote(ctx context.Context, fromChain, toChain id.Chain, fromToken, toToken, amountBase string) (quoteResponse, error) {
	vals := url.Values{}
	vals.Set("originChainId", strconv.FormatInt(bungeeChainID(fromChain), 10))
	vals.Set("destinationChainId", strconv.FormatInt(bungeeChainID(toChain), 10))
	vals.Set("inputToken", fromToken)
	vals.Set("outputToken", toToken)
	vals.Set("inputAmount", amountBase)
	vals.Set("userAddress", defaultAddressForChain(fromChain))
	vals.Set("receiverAddress", defaultAddressForChain(toChain))

	base := c.baseURL
	apiKey, affiliate, useDedicated := c.dedicatedAuth()
	if useDedicated {
		base = c.dedicatedBaseURL
	}
	url := base + "/bungee/quote?" + vals.Encode()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return quoteResponse{}, clierr.Wrap(clierr.CodeInternal, "build bungee quote request", err)
	}
	if useDedicated {
		req.Header.Set("x-api-key", apiKey)
		req.Header.Set("affiliate", affiliate)
	}

	var resp quoteResponse
	if _, err := c.http.DoJSON(ctx, req, &resp); err != nil {
		return quoteResponse{}, err
	}
	if !resp.Success {
		return quoteResponse{}, clierr.New(clierr.CodeUnavailable, bungeeError(resp.Error))
	}
	return resp, nil
}

func (c *Client) dedicatedAuth() (apiKey, affiliate string, ok bool) {
	apiKey = strings.TrimSpace(c.apiKey)
	affiliate = strings.TrimSpace(c.affiliate)
	return apiKey, affiliate, apiKey != "" && affiliate != ""
}

func summarizeQuote(resp quoteResponse, fallbackDecimals int) (amountBase string, decimals int, feeUSD float64, serviceTime int64, route string, err error) {
	amountBase = strings.TrimSpace(resp.Result.Output.Amount)
	decimals = positiveOrFallback(resp.Result.Output.Token.Decimals, positiveOrFallback(resp.Result.Output.Decimals, fallbackDecimals))

	if resp.Result.AutoRoute != nil {
		auto := resp.Result.AutoRoute
		if v := strings.TrimSpace(auto.Output.Amount); v != "" {
			amountBase = v
		}
		if v := strings.TrimSpace(auto.OutputAmount); v != "" {
			amountBase = v
		}
		decimals = positiveOrFallback(auto.Output.Token.Decimals, positiveOrFallback(auto.Output.Decimals, decimals))
		if auto.GasFee != nil {
			feeUSD = auto.GasFee.FeeInUSD
		}
		serviceTime = auto.EstimatedTime
		if details := autoRouteDetails(auto.UserTxs, auto.RouteDetails.Name); details != "" {
			route = "bungee:auto:" + details
		}
	}
	if amountBase == "" {
		return "", 0, 0, 0, "", clierr.New(clierr.CodeUnavailable, "bungee quote missing output amount")
	}
	if decimals <= 0 {
		decimals = fallbackDecimals
	}
	if decimals < 0 {
		decimals = 0
	}
	return amountBase, decimals, feeUSD, serviceTime, route, nil
}

func autoRouteDetails(userTxs []quoteUserTx, routeName string) string {
	if routeName = strings.TrimSpace(routeName); routeName != "" {
		return strings.ToLower(routeName)
	}
	steps := make([]string, 0, len(userTxs))
	for _, tx := range userTxs {
		step := strings.ToLower(strings.TrimSpace(tx.StepType))
		switch step {
		case "swap":
			names := make([]string, 0, len(tx.SwapRoutes))
			for _, r := range tx.SwapRoutes {
				if n := strings.ToLower(strings.TrimSpace(r.UsedDexName)); n != "" {
					names = append(names, n)
				}
			}
			sort.Strings(names)
			if len(names) > 0 {
				steps = append(steps, "swap("+strings.Join(uniqueStrings(names), "+")+")")
			} else {
				steps = append(steps, "swap")
			}
		case "bridge":
			names := make([]string, 0, len(tx.BridgeRoutes))
			for _, r := range tx.BridgeRoutes {
				for _, bridge := range r.UsedBridgeNames {
					if n := strings.ToLower(strings.TrimSpace(bridge)); n != "" {
						names = append(names, n)
					}
				}
			}
			sort.Strings(names)
			if len(names) > 0 {
				steps = append(steps, "bridge("+strings.Join(uniqueStrings(names), "+")+")")
			} else {
				steps = append(steps, "bridge")
			}
		default:
			if name := strings.ToLower(strings.TrimSpace(tx.RouteDetails.Name)); name != "" {
				steps = append(steps, name)
			} else if step != "" {
				steps = append(steps, step)
			}
		}
	}
	return strings.Join(steps, "->")
}

func uniqueStrings(items []string) []string {
	if len(items) <= 1 {
		return items
	}
	out := make([]string, 0, len(items))
	prev := ""
	for i, item := range items {
		if i == 0 || item != prev {
			out = append(out, item)
		}
		prev = item
	}
	return out
}

func defaultAddressForChain(chain id.Chain) string {
	_ = chain
	return defaultEVMUserAddress
}

func bungeeChainID(chain id.Chain) int64 {
	// Bungee currently expects HyperEVM quotes on chain ID 999.
	if chain.CAIP2 == "eip155:998" {
		return 999
	}
	return chain.EVMChainID
}

func positiveOrFallback(v, fallback int) int {
	if v > 0 {
		return v
	}
	return fallback
}

func bungeeError(v any) string {
	switch t := v.(type) {
	case nil:
		return "bungee quote failed"
	case string:
		if msg := strings.TrimSpace(t); msg != "" {
			return msg
		}
	case map[string]any:
		if msg, ok := t["message"].(string); ok && strings.TrimSpace(msg) != "" {
			return strings.TrimSpace(msg)
		}
	}
	return "bungee quote failed"
}
