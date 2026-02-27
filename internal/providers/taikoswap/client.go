package taikoswap

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
	feeTiers = []uint32{100, 500, 3000, 10000}

	quoterABI = mustABI(registry.UniswapV3QuoterV2ABI)
	erc20ABI  = mustABI(registry.ERC20MinimalABI)
	routerABI = mustABI(registry.UniswapV3RouterABI)
)

type Client struct {
	now func() time.Time
}

func New() *Client {
	return &Client{now: time.Now}
}

func (c *Client) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:        "taikoswap",
		Type:        "swap",
		RequiresKey: false,
		Capabilities: []string{
			"swap.quote",
			"swap.plan",
			"swap.execute",
		},
	}
}

type quoteExactInputSingleParams struct {
	TokenIn           common.Address `abi:"tokenIn"`
	TokenOut          common.Address `abi:"tokenOut"`
	AmountIn          *big.Int       `abi:"amountIn"`
	Fee               *big.Int       `abi:"fee"`
	SqrtPriceLimitX96 *big.Int       `abi:"sqrtPriceLimitX96"`
}

type exactInputSingleParams struct {
	TokenIn           common.Address `abi:"tokenIn"`
	TokenOut          common.Address `abi:"tokenOut"`
	Fee               *big.Int       `abi:"fee"`
	Recipient         common.Address `abi:"recipient"`
	AmountIn          *big.Int       `abi:"amountIn"`
	AmountOutMinimum  *big.Int       `abi:"amountOutMinimum"`
	SqrtPriceLimitX96 *big.Int       `abi:"sqrtPriceLimitX96"`
}

func (c *Client) QuoteSwap(ctx context.Context, req providers.SwapQuoteRequest) (model.SwapQuote, error) {
	rpcURL, quoter, _, err := c.chainConfig(req.Chain, req.RPCURL)
	if err != nil {
		return model.SwapQuote{}, err
	}
	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return model.SwapQuote{}, clierr.Wrap(clierr.CodeUnavailable, "connect taiko rpc", err)
	}
	defer client.Close()
	amountIn, ok := new(big.Int).SetString(req.AmountBaseUnits, 10)
	if !ok {
		return model.SwapQuote{}, clierr.New(clierr.CodeUsage, "invalid amount base units")
	}
	from := common.HexToAddress(req.FromAsset.Address)
	to := common.HexToAddress(req.ToAsset.Address)
	quoteOut, bestFee, _, err := quoteBestFee(ctx, client, quoter, from, to, amountIn)
	if err != nil {
		return model.SwapQuote{}, err
	}
	return model.SwapQuote{
		Provider:    "taikoswap",
		ChainID:     req.Chain.CAIP2,
		FromAssetID: req.FromAsset.AssetID,
		ToAssetID:   req.ToAsset.AssetID,
		InputAmount: model.AmountInfo{AmountBaseUnits: req.AmountBaseUnits, AmountDecimal: req.AmountDecimal, Decimals: req.FromAsset.Decimals},
		EstimatedOut: model.AmountInfo{
			AmountBaseUnits: quoteOut.String(),
			AmountDecimal:   id.FormatDecimalCompat(quoteOut.String(), req.ToAsset.Decimals),
			Decimals:        req.ToAsset.Decimals,
		},
		EstimatedGasUSD: 0,
		PriceImpactPct:  0,
		Route:           fmt.Sprintf("taikoswap-v3-fee-%d", bestFee),
		SourceURL:       "https://swap.taiko.xyz",
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
	rpcURL, quoter, router, err := c.chainConfig(req.Chain, opts.RPCURL)
	if err != nil {
		return execution.Action{}, err
	}
	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUnavailable, "connect taiko rpc", err)
	}
	defer client.Close()

	amountIn, ok := new(big.Int).SetString(req.AmountBaseUnits, 10)
	if !ok {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "invalid amount base units")
	}
	fromToken := common.HexToAddress(req.FromAsset.Address)
	toToken := common.HexToAddress(req.ToAsset.Address)
	recipient := strings.TrimSpace(opts.Recipient)
	if recipient == "" {
		recipient = sender
	}
	if !common.IsHexAddress(recipient) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "swap execution recipient must be a valid EVM address")
	}
	recipientAddr := common.HexToAddress(recipient)
	senderAddr := common.HexToAddress(sender)

	quotedOut, bestFee, _, err := quoteBestFee(ctx, client, quoter, fromToken, toToken, amountIn)
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
	amountOutMin := new(big.Int).Mul(quotedOut, big.NewInt(10_000-slippage))
	amountOutMin.Div(amountOutMin, big.NewInt(10_000))

	action := execution.NewAction(execution.NewActionID(), "swap", req.Chain.CAIP2, execution.Constraints{SlippageBps: slippage, Simulate: opts.Simulate})
	action.Provider = "taikoswap"
	action.FromAddress = senderAddr.Hex()
	action.ToAddress = recipientAddr.Hex()
	action.InputAmount = req.AmountBaseUnits
	action.Metadata = map[string]any{
		"token_in":       fromToken.Hex(),
		"token_out":      toToken.Hex(),
		"fee":            bestFee,
		"quoted_amount":  quotedOut.String(),
		"amount_out_min": amountOutMin.String(),
	}

	allowanceData, err := erc20ABI.Pack("allowance", senderAddr, router)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack allowance call", err)
	}
	allowanceOut, err := client.CallContract(ctx, ethereum.CallMsg{From: senderAddr, To: &fromToken, Data: allowanceData}, nil)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUnavailable, "read allowance", err)
	}
	values, err := erc20ABI.Unpack("allowance", allowanceOut)
	if err != nil || len(values) == 0 {
		return execution.Action{}, clierr.Wrap(clierr.CodeUnavailable, "decode allowance", err)
	}
	allowance, ok := values[0].(*big.Int)
	if !ok {
		return execution.Action{}, clierr.New(clierr.CodeUnavailable, "invalid allowance response")
	}

	if allowance.Cmp(amountIn) < 0 {
		approveData, err := erc20ABI.Pack("approve", router, amountIn)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack approve calldata", err)
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      "approve-token-in",
			Type:        execution.StepTypeApproval,
			Status:      execution.StepStatusPending,
			ChainID:     req.Chain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Approve token spending for swap router",
			Target:      fromToken.Hex(),
			Data:        "0x" + common.Bytes2Hex(approveData),
			Value:       "0",
		})
	}

	swapData, err := routerABI.Pack("exactInputSingle", exactInputSingleParams{
		TokenIn:           fromToken,
		TokenOut:          toToken,
		Fee:               big.NewInt(int64(bestFee)),
		Recipient:         recipientAddr,
		AmountIn:          amountIn,
		AmountOutMinimum:  amountOutMin,
		SqrtPriceLimitX96: big.NewInt(0),
	})
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack swap calldata", err)
	}
	action.Steps = append(action.Steps, execution.ActionStep{
		StepID:      "swap-exact-input-single",
		Type:        execution.StepTypeSwap,
		Status:      execution.StepStatusPending,
		ChainID:     req.Chain.CAIP2,
		RPCURL:      rpcURL,
		Description: "Swap exact input via TaikoSwap router",
		Target:      router.Hex(),
		Data:        "0x" + common.Bytes2Hex(swapData),
		Value:       "0",
		ExpectedOutputs: map[string]string{
			"amount_out_min": amountOutMin.String(),
		},
	})
	return action, nil
}

func (c *Client) chainConfig(chain id.Chain, rpcOverride string) (rpc string, quoter common.Address, router common.Address, err error) {
	quoterRaw, routerRaw, ok := registry.UniswapV3Contracts(chain.EVMChainID)
	if !ok {
		return "", common.Address{}, common.Address{}, clierr.New(clierr.CodeUnsupported, "taikoswap only supports taiko mainnet/hoodi chains")
	}
	rpc, err = registry.ResolveRPCURL(rpcOverride, chain.EVMChainID)
	if err != nil {
		return "", common.Address{}, common.Address{}, clierr.Wrap(clierr.CodeUsage, "resolve rpc url", err)
	}
	return rpc, common.HexToAddress(quoterRaw), common.HexToAddress(routerRaw), nil
}

func quoteBestFee(ctx context.Context, client *ethclient.Client, quoter, tokenIn, tokenOut common.Address, amountIn *big.Int) (*big.Int, uint32, *big.Int, error) {
	var (
		bestOut *big.Int
		bestGas *big.Int
		bestFee uint32
	)
	for _, fee := range feeTiers {
		callData, err := quoterABI.Pack("quoteExactInputSingle", quoteExactInputSingleParams{
			TokenIn:           tokenIn,
			TokenOut:          tokenOut,
			AmountIn:          amountIn,
			Fee:               big.NewInt(int64(fee)),
			SqrtPriceLimitX96: big.NewInt(0),
		})
		if err != nil {
			return nil, 0, nil, clierr.Wrap(clierr.CodeInternal, "pack quoter calldata", err)
		}
		out, err := client.CallContract(ctx, ethereum.CallMsg{To: &quoter, Data: callData}, nil)
		if err != nil {
			continue
		}
		decoded, err := quoterABI.Unpack("quoteExactInputSingle", out)
		if err != nil || len(decoded) < 4 {
			continue
		}
		amountOut, ok := decoded[0].(*big.Int)
		if !ok || amountOut == nil || amountOut.Sign() <= 0 {
			continue
		}
		gasEstimate, ok := decoded[3].(*big.Int)
		if !ok || gasEstimate == nil {
			gasEstimate = big.NewInt(0)
		}
		if bestOut == nil || amountOut.Cmp(bestOut) > 0 || (amountOut.Cmp(bestOut) == 0 && gasEstimate.Cmp(bestGas) < 0) {
			bestOut = new(big.Int).Set(amountOut)
			bestGas = new(big.Int).Set(gasEstimate)
			bestFee = fee
		}
	}
	if bestOut == nil {
		return nil, 0, nil, clierr.New(clierr.CodeUnavailable, "taikoswap quote unavailable for token pair")
	}
	return bestOut, bestFee, bestGas, nil
}

func mustABI(raw string) abi.ABI {
	parsed, err := abi.JSON(strings.NewReader(raw))
	if err != nil {
		panic(err)
	}
	return parsed
}
