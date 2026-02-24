package across

import (
	"context"
	"fmt"
	"math/big"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum/common"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
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
			"bridge.plan",
			"bridge.execute",
		},
	}
}

func (c *Client) QuoteBridge(ctx context.Context, req providers.BridgeQuoteRequest) (model.BridgeQuote, error) {
	if !req.FromChain.IsEVM() || !req.ToChain.IsEVM() {
		return model.BridgeQuote{}, clierr.New(clierr.CodeUnsupported, "across bridge quotes support only EVM chains")
	}
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

	feeBaseAbs := pickNumberString(fees, "totalRelayFee", "relayFeeTotal")
	feeBase := feeBaseAbs
	hasAbsoluteFee := strings.TrimSpace(feeBaseAbs) != ""
	if !hasAbsoluteFee {
		feeBase = "0"
	}

	estOut := pickNumberString(fees, "outputAmount")
	hasProviderOutputAmount := strings.TrimSpace(estOut) != ""
	if !hasProviderOutputAmount && hasAbsoluteFee {
		estOut = subtractBaseUnits(req.AmountBaseUnits, feeBase)
	}
	if strings.TrimSpace(estOut) == "" {
		estOut = req.AmountBaseUnits
	}
	feeUSD := pickFloat(fees, "totalRelayFeeUsd", "feeUsd")
	if feeUSD == 0 && hasAbsoluteFee {
		feeUSD = approximateStableUSD(req.FromAsset.Symbol, feeBase, req.FromAsset.Decimals)
	}
	estTime := int64(pickFloat(fees, "estimatedFillTimeSec", "estimatedFillTime"))
	if estTime == 0 {
		estTime = 120
	}
	feeBreakdown := buildAcrossFeeBreakdown(req, fees, feeBaseAbs, estOut, feeUSD, hasProviderOutputAmount)

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

type swapApprovalResponse struct {
	ApprovalTxns []struct {
		ChainID int64  `json:"chainId"`
		To      string `json:"to"`
		Data    string `json:"data"`
		Value   string `json:"value"`
	} `json:"approvalTxns"`
	SwapTx struct {
		ChainID int64  `json:"chainId"`
		To      string `json:"to"`
		Data    string `json:"data"`
		Value   string `json:"value"`
	} `json:"swapTx"`
	MinOutputAmount      string `json:"minOutputAmount"`
	ExpectedOutputAmount string `json:"expectedOutputAmount"`
	ExpectedFillTime     int64  `json:"expectedFillTime"`
	Steps                struct {
		Bridge struct {
			OutputAmount string `json:"outputAmount"`
		} `json:"bridge"`
	} `json:"steps"`
	Fees struct {
		Total struct {
			AmountUSD string `json:"amountUsd"`
		} `json:"total"`
	} `json:"fees"`
}

func (c *Client) BuildBridgeAction(ctx context.Context, req providers.BridgeQuoteRequest, opts providers.BridgeExecutionOptions) (execution.Action, error) {
	sender := strings.TrimSpace(opts.Sender)
	if sender == "" {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "bridge execution requires sender address")
	}
	if !common.IsHexAddress(sender) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "bridge execution sender must be a valid EVM address")
	}
	recipient := strings.TrimSpace(opts.Recipient)
	if recipient == "" {
		recipient = sender
	}
	if !common.IsHexAddress(recipient) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "bridge execution recipient must be a valid EVM address")
	}
	if !common.IsHexAddress(req.FromAsset.Address) || !common.IsHexAddress(req.ToAsset.Address) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "bridge execution requires ERC20 token addresses for from/to assets")
	}
	slippageBps := opts.SlippageBps
	if slippageBps <= 0 {
		slippageBps = 50
	}
	if slippageBps >= 10_000 {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "slippage bps must be less than 10000")
	}

	vals := url.Values{}
	vals.Set("amount", req.AmountBaseUnits)
	vals.Set("inputToken", req.FromAsset.Address)
	vals.Set("outputToken", req.ToAsset.Address)
	vals.Set("originChainId", strconv.FormatInt(req.FromChain.EVMChainID, 10))
	vals.Set("destinationChainId", strconv.FormatInt(req.ToChain.EVMChainID, 10))
	vals.Set("depositor", sender)
	vals.Set("recipient", recipient)
	vals.Set("slippage", formatSlippage(slippageBps))

	reqURL := c.baseURL + "/swap/approval?" + vals.Encode()
	hReq, err := http.NewRequestWithContext(ctx, http.MethodGet, reqURL, nil)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "build across execution request", err)
	}
	var resp swapApprovalResponse
	if _, err := c.http.DoJSON(ctx, hReq, &resp); err != nil {
		return execution.Action{}, err
	}
	if strings.TrimSpace(resp.SwapTx.To) == "" || strings.TrimSpace(resp.SwapTx.Data) == "" {
		return execution.Action{}, clierr.New(clierr.CodeUnavailable, "across execution response missing swap transaction payload")
	}
	if resp.SwapTx.ChainID != 0 && resp.SwapTx.ChainID != req.FromChain.EVMChainID {
		return execution.Action{}, clierr.New(clierr.CodeActionPlan, "across swap transaction chain does not match source chain")
	}

	rpcURL, err := execution.ResolveRPCURL(opts.RPCURL, req.FromChain.EVMChainID)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUsage, "resolve rpc url", err)
	}

	action := execution.NewAction(execution.NewActionID(), "bridge", req.FromChain.CAIP2, execution.Constraints{
		SlippageBps: slippageBps,
		Simulate:    opts.Simulate,
	})
	action.Provider = "across"
	action.FromAddress = common.HexToAddress(sender).Hex()
	action.ToAddress = common.HexToAddress(recipient).Hex()
	action.InputAmount = req.AmountBaseUnits
	action.Metadata = map[string]any{
		"to_chain_id":   req.ToChain.CAIP2,
		"from_asset_id": req.FromAsset.AssetID,
		"to_asset_id":   req.ToAsset.AssetID,
		"route":         "across",
	}

	for i, approval := range resp.ApprovalTxns {
		if strings.TrimSpace(approval.To) == "" || strings.TrimSpace(approval.Data) == "" {
			continue
		}
		if approval.ChainID != 0 && approval.ChainID != req.FromChain.EVMChainID {
			continue
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      fmt.Sprintf("approve-bridge-token-%d", i+1),
			Type:        execution.StepTypeApproval,
			Status:      execution.StepStatusPending,
			ChainID:     req.FromChain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Approve across bridge contract for source token",
			Target:      common.HexToAddress(approval.To).Hex(),
			Data:        ensureHexPrefix(approval.Data),
			Value:       normalizeTransactionValue(approval.Value),
		})
	}

	swapValue := normalizeTransactionValue(resp.SwapTx.Value)
	action.Steps = append(action.Steps, execution.ActionStep{
		StepID:      "bridge-transfer",
		Type:        execution.StepTypeBridge,
		Status:      execution.StepStatusPending,
		ChainID:     req.FromChain.CAIP2,
		RPCURL:      rpcURL,
		Description: "Bridge transfer via Across",
		Target:      common.HexToAddress(resp.SwapTx.To).Hex(),
		Data:        ensureHexPrefix(resp.SwapTx.Data),
		Value:       swapValue,
		ExpectedOutputs: map[string]string{
			"to_amount_min":                firstNonEmpty(resp.MinOutputAmount, resp.ExpectedOutputAmount, resp.Steps.Bridge.OutputAmount),
			"settlement_provider":          "across",
			"settlement_status_endpoint":   c.baseURL + "/deposit/status",
			"settlement_origin_chain":      strconv.FormatInt(req.FromChain.EVMChainID, 10),
			"settlement_recipient":         common.HexToAddress(recipient).Hex(),
			"settlement_destination_chain": strconv.FormatInt(req.ToChain.EVMChainID, 10),
		},
	})
	return action, nil
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

func buildAcrossFeeBreakdown(req providers.BridgeQuoteRequest, fees map[string]any, totalFeeBase, estimatedOut string, totalFeeUSD float64, hasProviderOutputAmount bool) *model.BridgeFeeBreakdown {
	lpFeeBase := pickNumberString(fees, "lpFee", "lpFeeTotal")
	relayerFeeBase := pickNumberString(fees, "relayerCapitalFee", "capitalFeeTotal")
	gasFeeBase := pickNumberString(fees, "relayerGasFee", "relayGasFeeTotal")

	breakdown := &model.BridgeFeeBreakdown{
		LPFee:       feeAmountFromBase(lpFeeBase, req.FromAsset.Decimals),
		RelayerFee:  feeAmountFromBase(relayerFeeBase, req.FromAsset.Decimals),
		GasFee:      feeAmountFromBase(gasFeeBase, req.FromAsset.Decimals),
		TotalFeeUSD: totalFeeUSD,
	}

	if strings.TrimSpace(totalFeeBase) != "" {
		breakdown.TotalFeeBaseUnits = trimLeadingZeros(totalFeeBase)
		breakdown.TotalFeeDecimal = id.FormatDecimalCompat(breakdown.TotalFeeBaseUnits, req.FromAsset.Decimals)
	}
	if hasProviderOutputAmount && breakdown.TotalFeeBaseUnits != "" && strings.TrimSpace(estimatedOut) != "" {
		delta := subtractBaseUnits(req.AmountBaseUnits, estimatedOut)
		consistent := compareBaseUnits(delta, breakdown.TotalFeeBaseUnits) == 0
		breakdown.ConsistentWithAmountDelta = &consistent
	}

	if breakdown.LPFee == nil && breakdown.RelayerFee == nil && breakdown.GasFee == nil && breakdown.TotalFeeUSD == 0 && breakdown.TotalFeeBaseUnits == "" && breakdown.ConsistentWithAmountDelta == nil {
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
	case "USDC", "USDT", "USDT0", "DAI", "USDE", "USDS", "USD1", "FRAX", "GHO", "TUSD", "LUSD", "PYUSD":
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

func formatSlippage(bps int64) string {
	return strconv.FormatFloat(float64(bps)/10000, 'f', 6, 64)
}

func ensureHexPrefix(v string) string {
	clean := strings.TrimSpace(v)
	if strings.HasPrefix(clean, "0x") || strings.HasPrefix(clean, "0X") {
		return clean
	}
	return "0x" + clean
}

func normalizeTransactionValue(v string) string {
	clean := strings.TrimSpace(v)
	if clean == "" {
		return "0"
	}
	if strings.HasPrefix(clean, "0x") || strings.HasPrefix(clean, "0X") {
		n := new(big.Int)
		if _, ok := n.SetString(strings.TrimPrefix(strings.TrimPrefix(clean, "0x"), "0X"), 16); ok {
			return n.String()
		}
		return "0"
	}
	if n, ok := new(big.Int).SetString(clean, 10); ok {
		return n.String()
	}
	return "0"
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if strings.TrimSpace(v) != "" {
			return strings.TrimSpace(v)
		}
	}
	return ""
}
