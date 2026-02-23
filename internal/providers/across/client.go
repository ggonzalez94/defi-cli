package across

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

	estOut := pickNumberString(fees, "outputAmount")
	if estOut == "" {
		estOut = subtractBaseUnits(req.AmountBaseUnits, feeBase)
	}
	feeUSD := pickFloat(fees, "totalRelayFeeUsd", "feeUsd")
	if feeUSD == 0 {
		feeUSD = approximateStableUSD(req.FromAsset.Symbol, feeBase, req.FromAsset.Decimals)
	}
	estTime := int64(pickFloat(fees, "estimatedFillTimeSec", "estimatedFillTime"))
	if estTime == 0 {
		estTime = 120
	}
	feeBreakdown := buildAcrossFeeBreakdown(req, fees, feeBase, estOut, feeUSD)

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
		FeeBreakdown:    feeBreakdown,
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
			if out := numberString(v); out != "" {
				return out
			}
		}
	}
	return ""
}

func pickFloat(m map[string]any, keys ...string) float64 {
	for _, key := range keys {
		if v, ok := m[key]; ok {
			if out, ok := floatValue(v); ok {
				return out
			}
		}
	}
	return 0
}

func numberString(v any) string {
	switch t := v.(type) {
	case string:
		s := strings.TrimSpace(t)
		if s == "" {
			return ""
		}
		return trimLeadingZeros(s)
	case float64:
		return trimLeadingZeros(strconv.FormatFloat(t, 'f', 0, 64))
	case map[string]any:
		if out := numberString(t["total"]); out != "" {
			return out
		}
		return numberString(t["amount"])
	default:
		return ""
	}
}

func floatValue(v any) (float64, bool) {
	switch t := v.(type) {
	case float64:
		return t, true
	case string:
		if strings.TrimSpace(t) == "" {
			return 0, false
		}
		f, err := strconv.ParseFloat(strings.TrimSpace(t), 64)
		if err != nil {
			return 0, false
		}
		return f, true
	case map[string]any:
		if f, ok := floatValue(t["usd"]); ok {
			return f, true
		}
		if f, ok := floatValue(t["value"]); ok {
			return f, true
		}
		return 0, false
	default:
		return 0, false
	}
}

func buildAcrossFeeBreakdown(req providers.BridgeQuoteRequest, fees map[string]any, totalFeeBase, estimatedOut string, totalFeeUSD float64) *model.BridgeFeeBreakdown {
	lpFeeBase := pickNumberString(fees, "lpFee", "lpFeeTotal")
	relayerFeeBase := pickNumberString(fees, "relayerCapitalFee", "capitalFeeTotal")
	gasFeeBase := pickNumberString(fees, "relayerGasFee", "relayGasFeeTotal")

	breakdown := &model.BridgeFeeBreakdown{
		LPFee:             feeAmountFromBase(lpFeeBase, req.FromAsset.Decimals),
		RelayerFee:        feeAmountFromBase(relayerFeeBase, req.FromAsset.Decimals),
		GasFee:            feeAmountFromBase(gasFeeBase, req.FromAsset.Decimals),
		TotalFeeBaseUnits: trimLeadingZeros(totalFeeBase),
		TotalFeeUSD:       totalFeeUSD,
	}

	if breakdown.TotalFeeBaseUnits == "" {
		breakdown.TotalFeeBaseUnits = "0"
	}
	breakdown.TotalFeeDecimal = id.FormatDecimalCompat(breakdown.TotalFeeBaseUnits, req.FromAsset.Decimals)
	if strings.TrimSpace(estimatedOut) != "" {
		delta := subtractBaseUnits(req.AmountBaseUnits, estimatedOut)
		consistent := compareBaseUnits(delta, breakdown.TotalFeeBaseUnits) == 0
		breakdown.ConsistentWithAmountDelta = &consistent
	}

	if breakdown.LPFee == nil && breakdown.RelayerFee == nil && breakdown.GasFee == nil && breakdown.TotalFeeUSD == 0 && breakdown.TotalFeeBaseUnits == "0" && breakdown.ConsistentWithAmountDelta == nil {
		return nil
	}
	return breakdown
}

func feeAmountFromBase(amountBase string, decimals int) *model.FeeAmount {
	amountBase = trimLeadingZeros(amountBase)
	if amountBase == "" || amountBase == "0" {
		return nil
	}
	return &model.FeeAmount{
		AmountBaseUnits: amountBase,
		AmountDecimal:   id.FormatDecimalCompat(amountBase, decimals),
	}
}

func approximateStableUSD(symbol, amountBase string, decimals int) float64 {
	if !isLikelyStableSymbol(symbol) {
		return 0
	}
	amountDecimal := id.FormatDecimalCompat(amountBase, decimals)
	if strings.TrimSpace(amountDecimal) == "" {
		return 0
	}
	v, err := strconv.ParseFloat(amountDecimal, 64)
	if err != nil {
		return 0
	}
	return v
}

func isLikelyStableSymbol(symbol string) bool {
	switch strings.ToUpper(strings.TrimSpace(symbol)) {
	case "USDC", "USDT", "USDT0", "DAI", "USDE", "USDS", "USD1", "FRAX", "GHO", "TUSD", "LUSD", "EURS", "PYUSD":
		return true
	default:
		return false
	}
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
