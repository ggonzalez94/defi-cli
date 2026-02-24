package lifi

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
	"strconv"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

const defaultBase = "https://li.quest/v1"

type Client struct {
	http    *httpx.Client
	baseURL string
	now     func() time.Time
}

func New(httpClient *httpx.Client) *Client {
	return &Client{http: httpClient, baseURL: defaultBase, now: time.Now}
}

func (c *Client) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:        "lifi",
		Type:        "bridge",
		RequiresKey: false,
		Capabilities: []string{
			"bridge.quote",
		},
	}
}

type quoteResponse struct {
	Estimate struct {
		ToAmount string `json:"toAmount"`
		FeeCosts []struct {
			AmountUSD string `json:"amountUSD"`
		} `json:"feeCosts"`
		GasCosts []struct {
			AmountUSD string `json:"amountUSD"`
		} `json:"gasCosts"`
		ExecutionDuration int64 `json:"executionDuration"`
	} `json:"estimate"`
	ToolDetails struct {
		Name string `json:"name"`
	} `json:"toolDetails"`
}

func (c *Client) QuoteBridge(ctx context.Context, req providers.BridgeQuoteRequest) (model.BridgeQuote, error) {
	if !req.FromChain.IsEVM() || !req.ToChain.IsEVM() {
		return model.BridgeQuote{}, clierr.New(clierr.CodeUnsupported, "lifi bridge quotes support only EVM chains")
	}
	vals := url.Values{}
	vals.Set("fromChain", strconv.FormatInt(req.FromChain.EVMChainID, 10))
	vals.Set("toChain", strconv.FormatInt(req.ToChain.EVMChainID, 10))
	vals.Set("fromToken", req.FromAsset.Address)
	vals.Set("toToken", req.ToAsset.Address)
	vals.Set("fromAmount", req.AmountBaseUnits)
	vals.Set("slippage", "0.005")
	vals.Set("fromAddress", "0x0000000000000000000000000000000000000001")

	url := c.baseURL + "/quote?" + vals.Encode()
	hReq, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return model.BridgeQuote{}, clierr.Wrap(clierr.CodeInternal, "build lifi quote request", err)
	}
	var resp quoteResponse
	if _, err := c.http.DoJSON(ctx, hReq, &resp); err != nil {
		return model.BridgeQuote{}, err
	}

	if resp.Estimate.ToAmount == "" {
		return model.BridgeQuote{}, clierr.New(clierr.CodeUnavailable, "lifi quote missing output amount")
	}

	protocolFeeUSD := 0.0
	for _, item := range resp.Estimate.FeeCosts {
		v, _ := strconv.ParseFloat(item.AmountUSD, 64)
		protocolFeeUSD += v
	}
	gasFeeUSD := 0.0
	for _, item := range resp.Estimate.GasCosts {
		v, _ := strconv.ParseFloat(item.AmountUSD, 64)
		gasFeeUSD += v
	}
	feeUSD := protocolFeeUSD + gasFeeUSD
	route := resp.ToolDetails.Name
	if route == "" {
		route = fmt.Sprintf("%s->%s", req.FromChain.Slug, req.ToChain.Slug)
	}

	feeBreakdown := &model.BridgeFeeBreakdown{
		TotalFeeUSD: feeUSD,
	}
	if protocolFeeUSD > 0 {
		feeBreakdown.RelayerFee = &model.FeeAmount{AmountUSD: protocolFeeUSD}
	}
	if gasFeeUSD > 0 {
		feeBreakdown.GasFee = &model.FeeAmount{AmountUSD: gasFeeUSD}
	}
	if feeBreakdown.RelayerFee == nil && feeBreakdown.GasFee == nil {
		feeBreakdown = nil
	}

	return model.BridgeQuote{
		Provider:    "lifi",
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
			AmountBaseUnits: resp.Estimate.ToAmount,
			AmountDecimal:   id.FormatDecimalCompat(resp.Estimate.ToAmount, req.ToAsset.Decimals),
			Decimals:        req.ToAsset.Decimals,
		},
		EstimatedFeeUSD: feeUSD,
		FeeBreakdown:    feeBreakdown,
		EstimatedTimeS:  resp.Estimate.ExecutionDuration,
		Route:           route,
		SourceURL:       "https://li.quest",
		FetchedAt:       c.now().UTC().Format(time.RFC3339),
	}, nil
}
