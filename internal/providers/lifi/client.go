package lifi

import (
	"context"
	"fmt"
	"math/big"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

type Client struct {
	http    *httpx.Client
	baseURL string
	now     func() time.Time
}

func New(httpClient *httpx.Client) *Client {
	return &Client{http: httpClient, baseURL: registry.LiFiBaseURL, now: time.Now}
}

func (c *Client) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:        "lifi",
		Type:        "bridge",
		RequiresKey: false,
		Capabilities: []string{
			"bridge.quote",
			"bridge.plan",
			"bridge.execute",
		},
	}
}

type quoteResponse struct {
	ID       string `json:"id"`
	Estimate struct {
		ToAmount        string `json:"toAmount"`
		ToAmountMin     string `json:"toAmountMin"`
		ApprovalAddress string `json:"approvalAddress"`
		FeeCosts        []struct {
			AmountUSD string `json:"amountUSD"`
		} `json:"feeCosts"`
		GasCosts []struct {
			AmountUSD string `json:"amountUSD"`
		} `json:"gasCosts"`
		ExecutionDuration int64 `json:"executionDuration"`
	} `json:"estimate"`
	ToolDetails struct {
		Key  string `json:"key"`
		Name string `json:"name"`
	} `json:"toolDetails"`
	Tool               string      `json:"tool"`
	IncludedSteps      []quoteStep `json:"includedSteps"`
	TransactionRequest struct {
		To       string `json:"to"`
		From     string `json:"from"`
		Data     string `json:"data"`
		Value    string `json:"value"`
		ChainID  int64  `json:"chainId"`
		GasLimit string `json:"gasLimit"`
		GasPrice string `json:"gasPrice"`
	} `json:"transactionRequest"`
}

type quoteStep struct {
	Action struct {
		ToChainID int64 `json:"toChainId"`
		ToToken   struct {
			Address  string `json:"address"`
			Decimals int    `json:"decimals"`
		} `json:"toToken"`
	} `json:"action"`
	Estimate struct {
		ToAmount string `json:"toAmount"`
	} `json:"estimate"`
}

func (c *Client) QuoteBridge(ctx context.Context, req providers.BridgeQuoteRequest) (model.BridgeQuote, error) {
	if !req.FromChain.IsEVM() || !req.ToChain.IsEVM() {
		return model.BridgeQuote{}, clierr.New(clierr.CodeUnsupported, "lifi bridge quotes support only EVM chains")
	}

	fromAmountForGas, err := normalizeOptionalBaseUnits(req.FromAmountForGas)
	if err != nil {
		return model.BridgeQuote{}, clierr.Wrap(clierr.CodeUsage, "parse bridge gas reserve amount", err)
	}
	vals := url.Values{}
	vals.Set("fromChain", strconv.FormatInt(req.FromChain.EVMChainID, 10))
	vals.Set("toChain", strconv.FormatInt(req.ToChain.EVMChainID, 10))
	vals.Set("fromToken", req.FromAsset.Address)
	vals.Set("toToken", req.ToAsset.Address)
	vals.Set("fromAmount", req.AmountBaseUnits)
	vals.Set("slippage", "0.005")
	vals.Set("fromAddress", "0x0000000000000000000000000000000000000001")
	if fromAmountForGas != "" {
		vals.Set("fromAmountForGas", fromAmountForGas)
	}

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

	nativeEstimate := destinationNativeEstimate(resp.IncludedSteps, req.ToChain.EVMChainID)
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
		FromAmountForGas:           fromAmountForGas,
		EstimatedDestinationNative: nativeEstimate,
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
	fromAmountForGas, err := normalizeOptionalBaseUnits(firstNonEmpty(opts.FromAmountForGas, req.FromAmountForGas))
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUsage, "parse bridge gas reserve amount", err)
	}

	vals := url.Values{}
	vals.Set("fromChain", strconv.FormatInt(req.FromChain.EVMChainID, 10))
	vals.Set("toChain", strconv.FormatInt(req.ToChain.EVMChainID, 10))
	vals.Set("fromToken", strings.ToLower(req.FromAsset.Address))
	vals.Set("toToken", strings.ToLower(req.ToAsset.Address))
	vals.Set("fromAmount", req.AmountBaseUnits)
	vals.Set("slippage", formatSlippage(slippageBps))
	vals.Set("fromAddress", sender)
	vals.Set("toAddress", recipient)
	if fromAmountForGas != "" {
		vals.Set("fromAmountForGas", fromAmountForGas)
	}

	reqURL := c.baseURL + "/quote?" + vals.Encode()
	hReq, err := http.NewRequestWithContext(ctx, http.MethodGet, reqURL, nil)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "build lifi execution quote request", err)
	}
	var resp quoteResponse
	if _, err := c.http.DoJSON(ctx, hReq, &resp); err != nil {
		return execution.Action{}, err
	}
	if strings.TrimSpace(resp.TransactionRequest.To) == "" || strings.TrimSpace(resp.TransactionRequest.Data) == "" {
		return execution.Action{}, clierr.New(clierr.CodeUnavailable, "lifi quote missing executable transaction payload")
	}
	if resp.TransactionRequest.ChainID != 0 && resp.TransactionRequest.ChainID != req.FromChain.EVMChainID {
		return execution.Action{}, clierr.New(clierr.CodeActionPlan, "lifi transaction chain does not match source chain")
	}

	rpcURL, err := registry.ResolveRPCURL(opts.RPCURL, req.FromChain.EVMChainID)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUsage, "resolve rpc url", err)
	}
	nativeEstimate := destinationNativeEstimate(resp.IncludedSteps, req.ToChain.EVMChainID)

	action := execution.NewAction(execution.NewActionID(), "bridge", req.FromChain.CAIP2, execution.Constraints{
		SlippageBps: slippageBps,
		Simulate:    opts.Simulate,
	})
	action.Provider = "lifi"
	action.FromAddress = sender
	action.ToAddress = recipient
	action.InputAmount = req.AmountBaseUnits
	action.Metadata = map[string]any{
		"to_chain_id":      req.ToChain.CAIP2,
		"from_asset_id":    req.FromAsset.AssetID,
		"to_asset_id":      req.ToAsset.AssetID,
		"route":            firstNonEmpty(resp.ToolDetails.Name, resp.Tool),
		"approval_spender": resp.Estimate.ApprovalAddress,
	}
	if fromAmountForGas != "" {
		action.Metadata["from_amount_for_gas"] = fromAmountForGas
	}
	if nativeEstimate != nil {
		action.Metadata["estimated_destination_native_base_units"] = nativeEstimate.AmountBaseUnits
	}

	if shouldAddApproval(req.FromAsset.Address, resp.Estimate.ApprovalAddress) {
		if !common.IsHexAddress(resp.Estimate.ApprovalAddress) {
			return execution.Action{}, clierr.New(clierr.CodeActionPlan, "lifi quote returned invalid approval address")
		}
		client, err := ethclient.DialContext(ctx, rpcURL)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeUnavailable, "connect source chain rpc for allowance check", err)
		}
		defer client.Close()

		amountIn, ok := new(big.Int).SetString(req.AmountBaseUnits, 10)
		if !ok {
			return execution.Action{}, clierr.New(clierr.CodeUsage, "invalid amount base units")
		}
		tokenAddr := common.HexToAddress(req.FromAsset.Address)
		ownerAddr := common.HexToAddress(sender)
		spenderAddr := common.HexToAddress(resp.Estimate.ApprovalAddress)
		allowanceData, err := lifiERC20ABI.Pack("allowance", ownerAddr, spenderAddr)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack allowance call", err)
		}
		allowanceRaw, err := client.CallContract(ctx, ethereum.CallMsg{From: ownerAddr, To: &tokenAddr, Data: allowanceData}, nil)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeUnavailable, "read allowance", err)
		}
		allowanceOut, err := lifiERC20ABI.Unpack("allowance", allowanceRaw)
		if err != nil || len(allowanceOut) == 0 {
			return execution.Action{}, clierr.Wrap(clierr.CodeUnavailable, "decode allowance", err)
		}
		currentAllowance, ok := allowanceOut[0].(*big.Int)
		if !ok {
			return execution.Action{}, clierr.New(clierr.CodeUnavailable, "invalid allowance response type")
		}
		if currentAllowance.Cmp(amountIn) < 0 {
			approveData, err := lifiERC20ABI.Pack("approve", spenderAddr, amountIn)
			if err != nil {
				return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack approve calldata", err)
			}
			action.Steps = append(action.Steps, execution.ActionStep{
				StepID:      "approve-bridge-token",
				Type:        execution.StepTypeApproval,
				Status:      execution.StepStatusPending,
				ChainID:     req.FromChain.CAIP2,
				RPCURL:      rpcURL,
				Description: "Approve bridge spender for source token",
				Target:      tokenAddr.Hex(),
				Data:        "0x" + common.Bytes2Hex(approveData),
				Value:       "0",
			})
		}
	}

	bridgeValue, err := hexToDecimal(resp.TransactionRequest.Value)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeActionPlan, "parse bridge transaction value", err)
	}
	statusEndpoint := registry.LiFiSettlementURL
	action.Steps = append(action.Steps, execution.ActionStep{
		StepID:      "bridge-transfer",
		Type:        execution.StepTypeBridge,
		Status:      execution.StepStatusPending,
		ChainID:     req.FromChain.CAIP2,
		RPCURL:      rpcURL,
		Description: "Bridge transfer via LiFi route",
		Target:      common.HexToAddress(resp.TransactionRequest.To).Hex(),
		Data:        ensureHexPrefix(resp.TransactionRequest.Data),
		Value:       bridgeValue,
		ExpectedOutputs: map[string]string{
			"to_amount_min":                firstNonEmpty(resp.Estimate.ToAmountMin, resp.Estimate.ToAmount),
			"settlement_provider":          "lifi",
			"settlement_status_endpoint":   statusEndpoint,
			"settlement_bridge":            firstNonEmpty(resp.ToolDetails.Key, resp.Tool),
			"settlement_from_chain":        strconv.FormatInt(req.FromChain.EVMChainID, 10),
			"settlement_to_chain":          strconv.FormatInt(req.ToChain.EVMChainID, 10),
			"settlement_quote_response_id": resp.ID,
		},
	})
	if nativeEstimate != nil {
		action.Steps[len(action.Steps)-1].ExpectedOutputs["destination_native_estimated"] = nativeEstimate.AmountBaseUnits
	}
	return action, nil
}

var lifiERC20ABI = mustLifiABI(registry.ERC20MinimalABI)

func mustLifiABI(raw string) abi.ABI {
	parsed, err := abi.JSON(strings.NewReader(raw))
	if err != nil {
		panic(err)
	}
	return parsed
}

func shouldAddApproval(tokenAddr, spender string) bool {
	if strings.TrimSpace(tokenAddr) == "" || strings.TrimSpace(spender) == "" {
		return false
	}
	if !common.IsHexAddress(tokenAddr) || !common.IsHexAddress(spender) {
		return false
	}
	return !strings.EqualFold(strings.TrimSpace(tokenAddr), "0x0000000000000000000000000000000000000000")
}

func destinationNativeEstimate(steps []quoteStep, destinationChainID int64) *model.AmountInfo {
	for _, step := range steps {
		if step.Action.ToChainID != destinationChainID {
			continue
		}
		addr := strings.TrimSpace(step.Action.ToToken.Address)
		if !isNativeTokenAddress(addr) {
			continue
		}
		amount := strings.TrimSpace(step.Estimate.ToAmount)
		if amount == "" {
			continue
		}
		decimals := step.Action.ToToken.Decimals
		if decimals <= 0 {
			decimals = 18
		}
		return &model.AmountInfo{
			AmountBaseUnits: amount,
			AmountDecimal:   id.FormatDecimalCompat(amount, decimals),
			Decimals:        decimals,
		}
	}
	return nil
}

func isNativeTokenAddress(addr string) bool {
	if strings.EqualFold(addr, "0x0000000000000000000000000000000000000000") {
		return true
	}
	return strings.EqualFold(addr, "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
}

func normalizeOptionalBaseUnits(v string) (string, error) {
	clean := strings.TrimSpace(v)
	if clean == "" {
		return "", nil
	}
	amount, ok := new(big.Int).SetString(clean, 10)
	if !ok {
		return "", fmt.Errorf("amount must be an integer base-unit value")
	}
	if amount.Sign() <= 0 {
		return "", fmt.Errorf("amount must be greater than zero")
	}
	return amount.String(), nil
}

func formatSlippage(bps int64) string {
	return strconv.FormatFloat(float64(bps)/10000, 'f', 6, 64)
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if strings.TrimSpace(v) != "" {
			return v
		}
	}
	return ""
}

func ensureHexPrefix(v string) string {
	clean := strings.TrimSpace(v)
	if strings.HasPrefix(clean, "0x") || strings.HasPrefix(clean, "0X") {
		return clean
	}
	return "0x" + clean
}

func hexToDecimal(v string) (string, error) {
	clean := strings.TrimSpace(v)
	if clean == "" {
		return "0", nil
	}
	clean = strings.TrimPrefix(clean, "0x")
	clean = strings.TrimPrefix(clean, "0X")
	n := new(big.Int)
	if _, ok := n.SetString(clean, 16); !ok {
		return "", fmt.Errorf("invalid hex value %q", v)
	}
	return n.String(), nil
}
