package tempo

import (
	"context"
	"fmt"
	"math/big"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

var (
	tempoDEXABI   = mustABI(registry.TempoStablecoinDEXABI)
	tempoERC20    = mustABI(registry.ERC20MinimalABI)
	tempoTIP20ABI = mustABI(registry.TempoTIP20MetadataABI)
)

type Client struct {
	now func() time.Time
}

func New() *Client {
	return &Client{now: time.Now}
}

func (c *Client) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:        "tempo",
		Type:        "swap",
		RequiresKey: false,
		Capabilities: []string{
			"swap.quote",
			"swap.plan",
			"swap.execute",
		},
	}
}

func (c *Client) QuoteSwap(ctx context.Context, req providers.SwapQuoteRequest) (model.SwapQuote, error) {
	rpcURL, dexAddr, err := c.chainConfig(req.Chain, req.RPCURL)
	if err != nil {
		return model.SwapQuote{}, err
	}
	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return model.SwapQuote{}, clierr.Wrap(clierr.CodeUnavailable, "connect tempo rpc", err)
	}
	defer client.Close()

	tradeType := req.TradeType
	if tradeType == "" {
		tradeType = providers.SwapTradeTypeExactInput
	}
	switch tradeType {
	case providers.SwapTradeTypeExactInput, providers.SwapTradeTypeExactOutput:
	default:
		return model.SwapQuote{}, clierr.New(clierr.CodeUnsupported, "tempo swap type must be exact-input or exact-output")
	}

	amount, err := parseUint128(req.AmountBaseUnits)
	if err != nil {
		return model.SwapQuote{}, err
	}
	tokenIn := common.HexToAddress(req.FromAsset.Address)
	tokenOut := common.HexToAddress(req.ToAsset.Address)
	if err := validateUSDPair(ctx, client, req.FromAsset, req.ToAsset, tokenIn, tokenOut); err != nil {
		return model.SwapQuote{}, err
	}

	inputAmount := amount
	estimatedOut := amount
	switch tradeType {
	case providers.SwapTradeTypeExactInput:
		estimatedOut, err = c.quoteExactAmountIn(ctx, client, dexAddr, req.FromAsset, req.ToAsset, tokenIn, tokenOut, amount)
		if err != nil {
			return model.SwapQuote{}, err
		}
	case providers.SwapTradeTypeExactOutput:
		inputAmount, err = c.quoteExactAmountOut(ctx, client, dexAddr, req.FromAsset, req.ToAsset, tokenIn, tokenOut, amount)
		if err != nil {
			return model.SwapQuote{}, err
		}
	}

	inputDecimals := req.FromAsset.Decimals
	if inputDecimals <= 0 {
		inputDecimals = 18
	}
	outputDecimals := req.ToAsset.Decimals
	if outputDecimals <= 0 {
		outputDecimals = 18
	}

	return model.SwapQuote{
		Provider:    "tempo",
		ChainID:     req.Chain.CAIP2,
		FromAssetID: req.FromAsset.AssetID,
		ToAssetID:   req.ToAsset.AssetID,
		TradeType:   string(tradeType),
		InputAmount: model.AmountInfo{
			AmountBaseUnits: inputAmount.String(),
			AmountDecimal:   id.FormatDecimalCompat(inputAmount.String(), inputDecimals),
			Decimals:        inputDecimals,
		},
		EstimatedOut: model.AmountInfo{
			AmountBaseUnits: estimatedOut.String(),
			AmountDecimal:   id.FormatDecimalCompat(estimatedOut.String(), outputDecimals),
			Decimals:        outputDecimals,
		},
		EstimatedGasUSD: 0,
		PriceImpactPct:  0,
		Route:           "tempo-dex",
		SourceURL:       "https://tempo.xyz",
		FetchedAt:       c.now().UTC().Format(time.RFC3339),
	}, nil
}

func (c *Client) BuildSwapAction(ctx context.Context, req providers.SwapQuoteRequest, opts providers.SwapExecutionOptions) (execution.Action, error) {
	sender := strings.TrimSpace(opts.Sender)
	if sender == "" {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "swap execution requires sender address")
	}
	if !common.IsHexAddress(sender) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "swap execution sender must be a valid EVM address")
	}
	recipient := strings.TrimSpace(opts.Recipient)
	if recipient == "" {
		recipient = sender
	}
	if !common.IsHexAddress(recipient) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "swap execution recipient must be a valid EVM address")
	}
	if !strings.EqualFold(recipient, sender) {
		return execution.Action{}, clierr.New(clierr.CodeUnsupported, "tempo swap execution currently settles to the sender only; omit --recipient or set it equal to --from-address")
	}

	rpcURL, dexAddr, err := c.chainConfig(req.Chain, opts.RPCURL)
	if err != nil {
		return execution.Action{}, err
	}
	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUnavailable, "connect tempo rpc", err)
	}
	defer client.Close()

	tradeType := req.TradeType
	if tradeType == "" {
		tradeType = providers.SwapTradeTypeExactInput
	}
	switch tradeType {
	case providers.SwapTradeTypeExactInput, providers.SwapTradeTypeExactOutput:
	default:
		return execution.Action{}, clierr.New(clierr.CodeUnsupported, "tempo swap type must be exact-input or exact-output")
	}

	amount, err := parseUint128(req.AmountBaseUnits)
	if err != nil {
		return execution.Action{}, err
	}
	slippage := opts.SlippageBps
	if slippage <= 0 {
		slippage = 50
	}
	if slippage >= 10_000 {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "slippage bps must be less than 10000")
	}

	tokenIn := common.HexToAddress(req.FromAsset.Address)
	tokenOut := common.HexToAddress(req.ToAsset.Address)
	senderAddr := common.HexToAddress(sender)
	if err := validateUSDPair(ctx, client, req.FromAsset, req.ToAsset, tokenIn, tokenOut); err != nil {
		return execution.Action{}, err
	}

	action := execution.NewAction(execution.NewActionID(), "swap", req.Chain.CAIP2, execution.Constraints{SlippageBps: slippage, Simulate: opts.Simulate})
	action.Provider = "tempo"
	action.FromAddress = senderAddr.Hex()
	action.ToAddress = senderAddr.Hex()
	action.Metadata = map[string]any{
		"trade_type": string(tradeType),
		"token_in":   tokenIn.Hex(),
		"token_out":  tokenOut.Hex(),
		"route":      "tempo-dex",
	}

	var (
		approvalAmount *big.Int
		swapData       []byte
		stepID         string
		description    string
		expected       map[string]string
	)
	switch tradeType {
	case providers.SwapTradeTypeExactInput:
		quotedOut, err := c.quoteExactAmountIn(ctx, client, dexAddr, req.FromAsset, req.ToAsset, tokenIn, tokenOut, amount)
		if err != nil {
			return execution.Action{}, err
		}
		minAmountOut := applySlippageFloor(quotedOut, slippage)
		swapData, err = tempoDEXABI.Pack("swapExactAmountIn", tokenIn, tokenOut, toUint128(amount), toUint128(minAmountOut))
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack tempo exact-input swap calldata", err)
		}
		action.InputAmount = amount.String()
		action.Metadata["quoted_amount_out"] = quotedOut.String()
		action.Metadata["amount_out_min"] = minAmountOut.String()
		approvalAmount = amount
		stepID = "tempo-swap-exact-input"
		description = "Swap exact input via Tempo Stablecoin DEX"
		expected = map[string]string{"amount_out_min": minAmountOut.String()}
	case providers.SwapTradeTypeExactOutput:
		quotedIn, err := c.quoteExactAmountOut(ctx, client, dexAddr, req.FromAsset, req.ToAsset, tokenIn, tokenOut, amount)
		if err != nil {
			return execution.Action{}, err
		}
		maxAmountIn := applySlippageCeil(quotedIn, slippage)
		swapData, err = tempoDEXABI.Pack("swapExactAmountOut", tokenIn, tokenOut, toUint128(amount), toUint128(maxAmountIn))
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack tempo exact-output swap calldata", err)
		}
		action.InputAmount = maxAmountIn.String()
		action.Metadata["desired_amount_out"] = amount.String()
		action.Metadata["quoted_amount_in"] = quotedIn.String()
		action.Metadata["amount_in_max"] = maxAmountIn.String()
		approvalAmount = maxAmountIn
		stepID = "tempo-swap-exact-output"
		description = "Swap exact output via Tempo Stablecoin DEX"
		expected = map[string]string{"amount_in_max": maxAmountIn.String(), "amount_out": amount.String()}
	}

	allowance, err := readAllowance(ctx, client, tokenIn, senderAddr, dexAddr)
	if err != nil {
		return execution.Action{}, err
	}
	if allowance.Cmp(approvalAmount) < 0 {
		approveData, err := tempoERC20.Pack("approve", dexAddr, approvalAmount)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack tempo approve calldata", err)
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      "approve-token-in",
			Type:        execution.StepTypeApproval,
			Status:      execution.StepStatusPending,
			ChainID:     req.Chain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Approve token spending for Tempo Stablecoin DEX",
			Target:      tokenIn.Hex(),
			Data:        "0x" + common.Bytes2Hex(approveData),
			Value:       "0",
		})
	}

	action.Steps = append(action.Steps, execution.ActionStep{
		StepID:          stepID,
		Type:            execution.StepTypeSwap,
		Status:          execution.StepStatusPending,
		ChainID:         req.Chain.CAIP2,
		RPCURL:          rpcURL,
		Description:     description,
		Target:          dexAddr.Hex(),
		Data:            "0x" + common.Bytes2Hex(swapData),
		Value:           "0",
		ExpectedOutputs: expected,
	})
	return action, nil
}

func (c *Client) chainConfig(chain id.Chain, rpcOverride string) (string, common.Address, error) {
	dexRaw, ok := registry.TempoStablecoinDEX(chain.EVMChainID)
	if !ok {
		return "", common.Address{}, clierr.New(clierr.CodeUnsupported, "tempo swap provider supports only tempo mainnet, moderato testnet, and devnet")
	}
	rpcURL, err := registry.ResolveRPCURL(rpcOverride, chain.EVMChainID)
	if err != nil {
		return "", common.Address{}, clierr.Wrap(clierr.CodeUsage, "resolve rpc url", err)
	}
	return rpcURL, common.HexToAddress(dexRaw), nil
}

func (c *Client) quoteExactAmountIn(ctx context.Context, client *ethclient.Client, dexAddr common.Address, fromAsset, toAsset id.Asset, tokenIn, tokenOut common.Address, amountIn *big.Int) (*big.Int, error) {
	return callUint128Method(ctx, client, dexAddr, "quoteSwapExactAmountIn", tempoAssetLabel(fromAsset), tempoAssetLabel(toAsset), tokenIn, tokenOut, toUint128(amountIn))
}

func (c *Client) quoteExactAmountOut(ctx context.Context, client *ethclient.Client, dexAddr common.Address, fromAsset, toAsset id.Asset, tokenIn, tokenOut common.Address, amountOut *big.Int) (*big.Int, error) {
	return callUint128Method(ctx, client, dexAddr, "quoteSwapExactAmountOut", tempoAssetLabel(fromAsset), tempoAssetLabel(toAsset), tokenIn, tokenOut, toUint128(amountOut))
}

func callUint128Method(ctx context.Context, client *ethclient.Client, target common.Address, method, tokenInLabel, tokenOutLabel string, args ...any) (*big.Int, error) {
	callData, err := tempoDEXABI.Pack(method, args...)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "pack tempo dex calldata", err)
	}
	out, err := client.CallContract(ctx, ethereum.CallMsg{To: &target, Data: callData}, nil)
	if err != nil {
		return nil, classifyTempoSwapCallError(err, tokenInLabel, tokenOutLabel)
	}
	values, err := tempoDEXABI.Unpack(method, out)
	if err != nil || len(values) != 1 {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "decode tempo dex response", err)
	}
	amount, ok := values[0].(*big.Int)
	if !ok || amount == nil || amount.Sign() <= 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "tempo quote returned invalid amount")
	}
	return amount, nil
}

func readAllowance(ctx context.Context, client *ethclient.Client, token, owner, spender common.Address) (*big.Int, error) {
	callData, err := tempoERC20.Pack("allowance", owner, spender)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "pack tempo allowance calldata", err)
	}
	out, err := client.CallContract(ctx, ethereum.CallMsg{From: owner, To: &token, Data: callData}, nil)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "read allowance", err)
	}
	values, err := tempoERC20.Unpack("allowance", out)
	if err != nil || len(values) == 0 {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "decode allowance", err)
	}
	allowance, ok := values[0].(*big.Int)
	if !ok || allowance == nil {
		return nil, clierr.New(clierr.CodeUnavailable, "invalid allowance response")
	}
	return allowance, nil
}

func validateUSDPair(ctx context.Context, client *ethclient.Client, fromAsset, toAsset id.Asset, tokenIn, tokenOut common.Address) error {
	fromCurrency, err := readTIP20Currency(ctx, client, tokenIn, fromAsset)
	if err != nil {
		return err
	}
	toCurrency, err := readTIP20Currency(ctx, client, tokenOut, toAsset)
	if err != nil {
		return err
	}
	if !strings.EqualFold(fromCurrency, "USD") || !strings.EqualFold(toCurrency, "USD") {
		return clierr.New(clierr.CodeUnsupported, fmt.Sprintf("tempo stablecoin dex supports only USD-denominated TIP-20s; got %s (%s) -> %s (%s)", tempoAssetLabel(fromAsset), fromCurrency, tempoAssetLabel(toAsset), toCurrency))
	}
	return nil
}

func readTIP20Currency(ctx context.Context, client *ethclient.Client, token common.Address, asset id.Asset) (string, error) {
	callData, err := tempoTIP20ABI.Pack("currency")
	if err != nil {
		return "", clierr.Wrap(clierr.CodeInternal, "pack tip20 currency calldata", err)
	}
	out, err := client.CallContract(ctx, ethereum.CallMsg{To: &token, Data: callData}, nil)
	if err != nil {
		if isTempoRevertError(err) {
			return "", clierr.New(clierr.CodeUnsupported, fmt.Sprintf("tempo swap asset %s is not a TIP-20 token with currency metadata", tempoAssetLabel(asset)))
		}
		return "", clierr.Wrap(clierr.CodeUnavailable, "read token currency", err)
	}
	values, err := tempoTIP20ABI.Unpack("currency", out)
	if err != nil || len(values) == 0 {
		return "", clierr.Wrap(clierr.CodeUnavailable, "decode token currency", err)
	}
	currency, ok := values[0].(string)
	if !ok || strings.TrimSpace(currency) == "" {
		return "", clierr.New(clierr.CodeUnavailable, "invalid token currency response")
	}
	return strings.ToUpper(strings.TrimSpace(currency)), nil
}

func classifyTempoSwapCallError(err error, tokenInLabel, tokenOutLabel string) error {
	if err == nil {
		return nil
	}
	if isTempoRevertError(err) {
		switch {
		case strings.Contains(err.Error(), "PairDoesNotExist"):
			return clierr.New(clierr.CodeUnsupported, fmt.Sprintf("tempo dex does not support %s -> %s", tokenInLabel, tokenOutLabel))
		case strings.Contains(err.Error(), "InsufficientLiquidity"):
			return clierr.New(clierr.CodeUnsupported, fmt.Sprintf("tempo dex has insufficient liquidity for %s -> %s", tokenInLabel, tokenOutLabel))
		default:
			return clierr.Wrap(clierr.CodeUnsupported, fmt.Sprintf("tempo dex rejected %s -> %s swap request", tokenInLabel, tokenOutLabel), err)
		}
	}
	return clierr.Wrap(clierr.CodeUnavailable, "query tempo dex", err)
}

func isTempoRevertError(err error) bool {
	return strings.Contains(strings.ToLower(err.Error()), "execution reverted")
}

func tempoAssetLabel(asset id.Asset) string {
	if strings.TrimSpace(asset.Symbol) != "" {
		return asset.Symbol
	}
	return asset.Address
}

func parseUint128(raw string) (*big.Int, error) {
	amount, ok := new(big.Int).SetString(strings.TrimSpace(raw), 10)
	if !ok || amount.Sign() <= 0 {
		return nil, clierr.New(clierr.CodeUsage, "swap amount must be a positive integer in base units")
	}
	if amount.BitLen() > 128 {
		return nil, clierr.New(clierr.CodeUsage, "swap amount exceeds uint128 bounds")
	}
	return amount, nil
}

func toUint128(v *big.Int) *big.Int {
	if v == nil {
		return big.NewInt(0)
	}
	return new(big.Int).Set(v)
}

func applySlippageFloor(amount *big.Int, bps int64) *big.Int {
	result := new(big.Int).Mul(new(big.Int).Set(amount), big.NewInt(10_000-bps))
	return result.Div(result, big.NewInt(10_000))
}

func applySlippageCeil(amount *big.Int, bps int64) *big.Int {
	numerator := new(big.Int).Mul(new(big.Int).Set(amount), big.NewInt(10_000+bps))
	numerator.Add(numerator, big.NewInt(9_999))
	return numerator.Div(numerator, big.NewInt(10_000))
}

func mustABI(raw string) abi.ABI {
	parsed, err := abi.JSON(strings.NewReader(raw))
	if err != nil {
		panic(fmt.Sprintf("parse tempo ABI: %v", err))
	}
	return parsed
}
