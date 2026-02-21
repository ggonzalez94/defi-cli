package across

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	clierr "github.com/gustavo/defi-cli/internal/errors"
	"github.com/gustavo/defi-cli/internal/httpx"
	"github.com/gustavo/defi-cli/internal/id"
	"github.com/gustavo/defi-cli/internal/model"
	"github.com/gustavo/defi-cli/internal/providers"
)

const defaultBase = "https://app.across.to/api"

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
		Name:        "across",
		Type:        "bridge",
		RequiresKey: false,
		Capabilities: []string{
			"bridge.quote",
		},
	}
}

func (c *Client) QuoteBridge(ctx context.Context, req providers.BridgeQuoteRequest) (model.BridgeQuote, error) {
	chainFrom := strconv.FormatInt(req.FromChain.EVMChainID, 10)
	chainTo := strconv.FormatInt(req.ToChain.EVMChainID, 10)

	vals := url.Values{}
	vals.Set("originChainId", chainFrom)
	vals.Set("destinationChainId", chainTo)
	vals.Set("token", req.FromAsset.Address)
	vals.Set("amount", req.AmountBaseUnits)

	limitsURL := c.baseURL + "/limits?" + vals.Encode()
	limitsReq, err := http.NewRequestWithContext(ctx, http.MethodGet, limitsURL, nil)
	if err != nil {
		return model.BridgeQuote{}, clierr.Wrap(clierr.CodeInternal, "build across limits request", err)
	}

	var limits map[string]any
	if _, err := c.http.DoJSON(ctx, limitsReq, &limits); err != nil {
		return model.BridgeQuote{}, err
	}

	if !checkAmountWithinLimits(req.AmountBaseUnits, limits) {
		return model.BridgeQuote{}, clierr.New(clierr.CodeUsage, "amount is outside across bridge limits")
	}

	feesURL := c.baseURL + "/suggested-fees?" + vals.Encode()
	feesReq, err := http.NewRequestWithContext(ctx, http.MethodGet, feesURL, nil)
	if err != nil {
		return model.BridgeQuote{}, clierr.Wrap(clierr.CodeInternal, "build across fees request", err)
	}

	var fees map[string]any
	if _, err := c.http.DoJSON(ctx, feesReq, &fees); err != nil {
		return model.BridgeQuote{}, err
	}

	feeBase := pickNumberString(fees, "totalRelayFee", "relayFeeTotal", "relayFeePct")
	if feeBase == "" {
		feeBase = "0"
	}

	estOut := subtractBaseUnits(req.AmountBaseUnits, feeBase)
	feeUSD := pickFloat(fees, "totalRelayFeeUsd", "feeUsd")
	estTime := int64(pickFloat(fees, "estimatedFillTimeSec", "estimatedFillTime"))
	if estTime == 0 {
		estTime = 120
	}

	return model.BridgeQuote{
		Provider:    "across",
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
			AmountBaseUnits: estOut,
			AmountDecimal:   id.FormatDecimalCompat(estOut, req.ToAsset.Decimals),
			Decimals:        req.ToAsset.Decimals,
		},
		EstimatedFeeUSD: feeUSD,
		EstimatedTimeS:  estTime,
		Route:           fmt.Sprintf("%s->%s", req.FromChain.Slug, req.ToChain.Slug),
		SourceURL:       "https://app.across.to",
		FetchedAt:       c.now().UTC().Format(time.RFC3339),
	}, nil
}

func checkAmountWithinLimits(amount string, limits map[string]any) bool {
	min := pickNumberString(limits, "minDeposit", "minLimit")
	max := pickNumberString(limits, "maxDeposit", "maxLimit")
	if min != "" && compareBaseUnits(amount, min) < 0 {
		return false
	}
	if max != "" && compareBaseUnits(amount, max) > 0 {
		return false
	}
	return true
}

func pickNumberString(m map[string]any, keys ...string) string {
	for _, key := range keys {
		if v, ok := m[key]; ok {
			switch t := v.(type) {
			case string:
				if t != "" {
					return t
				}
			case float64:
				return strconv.FormatFloat(t, 'f', 0, 64)
			}
		}
	}
	return ""
}

func pickFloat(m map[string]any, keys ...string) float64 {
	for _, key := range keys {
		if v, ok := m[key]; ok {
			switch t := v.(type) {
			case float64:
				return t
			case string:
				f, _ := strconv.ParseFloat(t, 64)
				return f
			}
		}
	}
	return 0
}

func compareBaseUnits(a, b string) int {
	a = trimLeadingZeros(a)
	b = trimLeadingZeros(b)
	if len(a) != len(b) {
		if len(a) < len(b) {
			return -1
		}
		return 1
	}
	if a < b {
		return -1
	}
	if a > b {
		return 1
	}
	return 0
}

func subtractBaseUnits(amount, fee string) string {
	if compareBaseUnits(amount, fee) <= 0 {
		return "0"
	}
	ai := toDigits(amount)
	bi := toDigits(fee)
	carry := 0
	res := make([]byte, 0, len(ai))
	i := len(ai) - 1
	j := len(bi) - 1
	for i >= 0 {
		a := int(ai[i]-'0') - carry
		b := 0
		if j >= 0 {
			b = int(bi[j] - '0')
		}
		if a < b {
			a += 10
			carry = 1
		} else {
			carry = 0
		}
		res = append(res, byte(a-b)+'0')
		i--
		j--
	}
	for i, j := 0, len(res)-1; i < j; i, j = i+1, j-1 {
		res[i], res[j] = res[j], res[i]
	}
	return trimLeadingZeros(string(res))
}

func trimLeadingZeros(v string) string {
	v = strings.TrimLeft(v, "0")
	if v == "" {
		return "0"
	}
	return v
}

func toDigits(v string) string {
	v = strings.TrimSpace(v)
	if v == "" {
		return "0"
	}
	for _, r := range v {
		if r < '0' || r > '9' {
			return "0"
		}
	}
	return trimLeadingZeros(v)
}
