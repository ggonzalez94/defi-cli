package execution

import (
	"bytes"
	"fmt"
	"math/big"
	"strings"

	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/common"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

var (
	policyERC20ABI           = mustPolicyABI(registry.ERC20MinimalABI)
	policyUniswapV3RouterABI = mustPolicyABI(registry.UniswapV3RouterABI)
	policyTempoDEXABI        = mustPolicyABI(registry.TempoStablecoinDEXABI)

	policyApproveSelector     = policyERC20ABI.Methods["approve"].ID
	policyTransferSelector    = policyERC20ABI.Methods["transfer"].ID
	policyUniswapV3SwapMethod = policyUniswapV3RouterABI.Methods["exactInputSingle"].ID
	policyTempoSwapExactIn    = policyTempoDEXABI.Methods["swapExactAmountIn"].ID
	policyTempoSwapExactOut   = policyTempoDEXABI.Methods["swapExactAmountOut"].ID
)

func validateStepPolicy(action *Action, step *ActionStep, chainID int64, data []byte, opts ExecuteOptions) error {
	if step == nil {
		return clierr.New(clierr.CodeInternal, "missing action step")
	}
	// Batched steps (Calls populated) may have empty Target/Data. Skip the
	// single-target address check for those; the provider-specific handler
	// will validate each call's target individually.
	if len(step.Calls) == 0 && !common.IsHexAddress(step.Target) {
		return clierr.New(clierr.CodeUsage, "invalid step target address")
	}

	switch step.Type {
	case StepTypeApproval:
		return validateApprovalPolicy(action, data, opts)
	case StepTypeTransfer:
		return validateTransferPolicy(action, step, data)
	case StepTypeSwap:
		return validateSwapPolicy(action, step, chainID, data, opts)
	case StepTypeBridge:
		return validateBridgePolicy(action, step, chainID, opts)
	default:
		return nil
	}
}

func validateApprovalPolicy(action *Action, data []byte, opts ExecuteOptions) error {
	if len(data) < 4 || !bytes.Equal(data[:4], policyApproveSelector) {
		return clierr.New(clierr.CodeActionPlan, "approval step must use ERC20 approve(spender,amount)")
	}
	args, err := policyERC20ABI.Methods["approve"].Inputs.Unpack(data[4:])
	if err != nil || len(args) != 2 {
		return clierr.New(clierr.CodeActionPlan, "approval step calldata is invalid")
	}
	spender, ok := toAddress(args[0])
	if !ok || spender == (common.Address{}) {
		return clierr.New(clierr.CodeActionPlan, "approval step has invalid spender")
	}
	amount, ok := toBigInt(args[1])
	if !ok || amount.Sign() <= 0 {
		return clierr.New(clierr.CodeActionPlan, "approval step has invalid approval amount")
	}
	if opts.AllowMaxApproval {
		return nil
	}
	if action == nil {
		return clierr.New(clierr.CodeActionPlan, "cannot validate approval bounds without action context")
	}
	requested, ok := parsePositiveBaseUnits(action.InputAmount)
	if !ok {
		return clierr.New(clierr.CodeActionPlan, "cannot validate approval bounds for non-numeric input amount; use --allow-max-approval to override")
	}
	if amount.Cmp(requested) > 0 {
		return clierr.New(
			clierr.CodeActionPlan,
			fmt.Sprintf("approval amount %s exceeds requested input amount %s; use --allow-max-approval to override", amount.String(), requested.String()),
		)
	}
	return nil
}

func validateTransferPolicy(action *Action, step *ActionStep, data []byte) error {
	if len(data) < 4 || !bytes.Equal(data[:4], policyTransferSelector) {
		return clierr.New(clierr.CodeActionPlan, "transfer step must use ERC20 transfer(to,amount)")
	}
	args, err := policyERC20ABI.Methods["transfer"].Inputs.Unpack(data[4:])
	if err != nil || len(args) != 2 {
		return clierr.New(clierr.CodeActionPlan, "transfer step calldata is invalid")
	}
	recipient, ok := toAddress(args[0])
	if !ok || recipient == (common.Address{}) {
		return clierr.New(clierr.CodeActionPlan, "transfer step has invalid recipient")
	}
	amount, ok := toBigInt(args[1])
	if !ok || amount.Sign() <= 0 {
		return clierr.New(clierr.CodeActionPlan, "transfer step has invalid transfer amount")
	}
	if action == nil {
		return nil
	}
	requested, ok := parsePositiveBaseUnits(action.InputAmount)
	if !ok {
		return clierr.New(clierr.CodeActionPlan, "cannot validate transfer amount for non-numeric input amount")
	}
	if amount.Cmp(requested) != 0 {
		return clierr.New(
			clierr.CodeActionPlan,
			fmt.Sprintf("transfer amount %s does not match requested input amount %s", amount.String(), requested.String()),
		)
	}
	if strings.TrimSpace(action.ToAddress) != "" && !strings.EqualFold(strings.TrimSpace(action.ToAddress), recipient.Hex()) {
		return clierr.New(clierr.CodeActionPlan, "transfer recipient does not match action to_address")
	}
	if strings.TrimSpace(step.Target) != "" && !common.IsHexAddress(step.Target) {
		return clierr.New(clierr.CodeActionPlan, "transfer step has invalid token target")
	}
	assetAddress := strings.TrimSpace(metadataString(action.Metadata, "asset_address"))
	if assetAddress == "" {
		return clierr.New(clierr.CodeActionPlan, "transfer action missing asset_address metadata")
	}
	if !common.IsHexAddress(assetAddress) {
		return clierr.New(clierr.CodeActionPlan, "transfer action metadata has invalid asset_address")
	}
	if !strings.EqualFold(common.HexToAddress(step.Target).Hex(), common.HexToAddress(assetAddress).Hex()) {
		return clierr.New(clierr.CodeActionPlan, "transfer step target does not match action asset_address")
	}
	return nil
}

func validateSwapPolicy(action *Action, step *ActionStep, chainID int64, data []byte, opts ExecuteOptions) error {
	if action == nil {
		return nil
	}
	switch strings.ToLower(strings.TrimSpace(action.Provider)) {
	case "taikoswap":
		if len(data) < 4 || !bytes.Equal(data[:4], policyUniswapV3SwapMethod) {
			return clierr.New(clierr.CodeActionPlan, "taikoswap swap step must call exactInputSingle")
		}
		_, router, ok := registry.UniswapV3Contracts(chainID)
		if !ok {
			return clierr.New(clierr.CodeActionPlan, "taikoswap swap step has unsupported chain")
		}
		expectedRouter := common.HexToAddress(router).Hex()
		if !strings.EqualFold(common.HexToAddress(step.Target).Hex(), expectedRouter) {
			return clierr.New(clierr.CodeActionPlan, "taikoswap swap step target does not match canonical router")
		}
	case "tempo":
		// Batched calls: validate each call individually.
		// Always enter this path when Calls is populated, regardless of
		// whether legacy Data is also set, to match validateStepPolicyCalls
		// in the executor and prevent tampered actions from bypassing
		// batched validation.
		if len(step.Calls) > 0 {
			return validateTempoSwapCalls(chainID, step.Calls, action, opts)
		}
		// Legacy single-target validation.
		if len(data) < 4 || (!bytes.Equal(data[:4], policyTempoSwapExactIn) && !bytes.Equal(data[:4], policyTempoSwapExactOut)) {
			return clierr.New(clierr.CodeActionPlan, "tempo swap step must call swapExactAmountIn or swapExactAmountOut")
		}
		dexAddr, ok := registry.TempoStablecoinDEX(chainID)
		if !ok {
			return clierr.New(clierr.CodeActionPlan, "tempo swap step has unsupported chain")
		}
		expectedDEX := common.HexToAddress(dexAddr).Hex()
		if !strings.EqualFold(common.HexToAddress(step.Target).Hex(), expectedDEX) {
			return clierr.New(clierr.CodeActionPlan, "tempo swap step target does not match canonical stablecoin dex")
		}
	}
	return nil
}

// validateTempoSwapCalls validates each call in a batched Tempo swap step.
// Recognized selectors are ERC-20 approve and Tempo DEX swap methods.
// At least one swap call (swapExactAmountIn or swapExactAmountOut) is required.
func validateTempoSwapCalls(chainID int64, calls []StepCall, action *Action, opts ExecuteOptions) error {
	dexAddr, ok := registry.TempoStablecoinDEX(chainID)
	if !ok {
		return clierr.New(clierr.CodeActionPlan, "tempo swap step has unsupported chain")
	}
	expectedDEX := common.HexToAddress(dexAddr).Hex()

	hasSwapCall := false
	approveCount := 0
	for i, call := range calls {
		data, err := decodeHex(call.Data)
		if err != nil {
			return clierr.Wrap(clierr.CodeActionPlan, fmt.Sprintf("tempo swap call %d has invalid data", i), err)
		}
		if len(data) < 4 {
			return clierr.New(clierr.CodeActionPlan, fmt.Sprintf("tempo swap call %d has insufficient calldata", i))
		}
		selector := data[:4]
		switch {
		case bytes.Equal(selector, policyApproveSelector):
			approveCount++
			if approveCount > 1 {
				return clierr.New(clierr.CodeActionPlan, "tempo swap step contains more than one approve call")
			}
			// Approve must not send value.
			if strings.TrimSpace(call.Value) != "" && strings.TrimSpace(call.Value) != "0" {
				return clierr.New(clierr.CodeActionPlan, fmt.Sprintf("tempo swap call %d approve must have zero value", i))
			}
			// Approve target must be the action's input token.
			if action != nil {
				expectedToken := strings.TrimSpace(metadataString(action.Metadata, "token_in"))
				if expectedToken != "" && !strings.EqualFold(common.HexToAddress(call.Target).Hex(), common.HexToAddress(expectedToken).Hex()) {
					return clierr.New(clierr.CodeActionPlan, fmt.Sprintf("tempo swap call %d approve target does not match action input token", i))
				}
			}
			// Validate spender and amount for approve calls.
			args, abiErr := policyERC20ABI.Methods["approve"].Inputs.Unpack(data[4:])
			if abiErr != nil || len(args) != 2 {
				return clierr.New(clierr.CodeActionPlan, fmt.Sprintf("tempo swap call %d has invalid approve calldata", i))
			}
			spender, spenderOK := toAddress(args[0])
			if !spenderOK || spender == (common.Address{}) {
				return clierr.New(clierr.CodeActionPlan, fmt.Sprintf("tempo swap call %d has invalid approve spender", i))
			}
			if !strings.EqualFold(spender.Hex(), expectedDEX) {
				return clierr.New(clierr.CodeActionPlan, fmt.Sprintf("tempo swap call %d approve spender does not match canonical stablecoin dex", i))
			}
			if !opts.AllowMaxApproval {
				amount, amountOK := toBigInt(args[1])
				if !amountOK || amount.Sign() <= 0 {
					return clierr.New(clierr.CodeActionPlan, fmt.Sprintf("tempo swap call %d has invalid approve amount", i))
				}
				if action != nil {
					requested, reqOK := parsePositiveBaseUnits(action.InputAmount)
					if !reqOK {
						return clierr.New(clierr.CodeActionPlan, "cannot validate approval bounds for non-numeric input amount; use --allow-max-approval to override")
					}
					if amount.Cmp(requested) > 0 {
						return clierr.New(
							clierr.CodeActionPlan,
							fmt.Sprintf("tempo swap call %d approval amount %s exceeds requested input amount %s; use --allow-max-approval to override", i, amount.String(), requested.String()),
						)
					}
				}
			}
		case bytes.Equal(selector, policyTempoSwapExactIn), bytes.Equal(selector, policyTempoSwapExactOut):
			// Swap calls must target the canonical DEX.
			if !strings.EqualFold(common.HexToAddress(call.Target).Hex(), expectedDEX) {
				return clierr.New(clierr.CodeActionPlan, "tempo swap call target does not match canonical stablecoin dex")
			}
			hasSwapCall = true
		default:
			return clierr.New(clierr.CodeActionPlan, fmt.Sprintf("tempo swap call %d has unrecognized selector 0x%x", i, selector))
		}
	}
	if !hasSwapCall {
		return clierr.New(clierr.CodeActionPlan, "tempo swap step must contain at least one swap call (swapExactAmountIn or swapExactAmountOut)")
	}
	return nil
}

func validateBridgePolicy(action *Action, step *ActionStep, chainID int64, opts ExecuteOptions) error {
	if opts.UnsafeProviderTx {
		return nil
	}
	provider := ""
	if step.ExpectedOutputs != nil {
		provider = strings.ToLower(strings.TrimSpace(step.ExpectedOutputs["settlement_provider"]))
	}
	if provider == "" && action != nil {
		provider = strings.ToLower(strings.TrimSpace(action.Provider))
	}
	if provider != "lifi" && provider != "across" {
		return clierr.New(clierr.CodeActionPlan, "bridge step has unknown settlement provider; use --unsafe-provider-tx to override")
	}
	if action != nil && strings.TrimSpace(action.Provider) != "" && !strings.EqualFold(strings.TrimSpace(action.Provider), provider) {
		return clierr.New(clierr.CodeActionPlan, "bridge step provider does not match action provider")
	}
	statusEndpoint := ""
	if step.ExpectedOutputs != nil {
		statusEndpoint = strings.TrimSpace(step.ExpectedOutputs["settlement_status_endpoint"])
	}
	if !registry.IsAllowedBridgeSettlementURL(provider, statusEndpoint) {
		return clierr.New(clierr.CodeActionPlan, "bridge step settlement endpoint is not allowed; use --unsafe-provider-tx to override")
	}
	// Enforce canonical target checks only on provider/chain pairs with explicit registry coverage.
	if registry.HasBridgeExecutionTargetPolicy(provider, chainID) && !registry.IsAllowedBridgeExecutionTarget(provider, chainID, step.Target) {
		return clierr.New(clierr.CodeActionPlan, "bridge step target is not an allowed provider execution contract; use --unsafe-provider-tx to override")
	}
	return nil
}

func parsePositiveBaseUnits(value string) (*big.Int, bool) {
	v := strings.TrimSpace(value)
	if v == "" {
		return nil, false
	}
	parsed, ok := new(big.Int).SetString(v, 10)
	if !ok || parsed.Sign() <= 0 {
		return nil, false
	}
	return parsed, true
}

func toAddress(v any) (common.Address, bool) {
	switch value := v.(type) {
	case common.Address:
		return value, true
	case *common.Address:
		if value == nil {
			return common.Address{}, false
		}
		return *value, true
	default:
		return common.Address{}, false
	}
}

func toBigInt(v any) (*big.Int, bool) {
	switch value := v.(type) {
	case *big.Int:
		if value == nil {
			return nil, false
		}
		return value, true
	case big.Int:
		cpy := value
		return &cpy, true
	default:
		return nil, false
	}
}

func metadataString(metadata map[string]any, key string) string {
	if metadata == nil {
		return ""
	}
	raw, ok := metadata[key]
	if !ok || raw == nil {
		return ""
	}
	switch value := raw.(type) {
	case string:
		return value
	default:
		return ""
	}
}

func mustPolicyABI(raw string) abi.ABI {
	parsed, err := abi.JSON(strings.NewReader(raw))
	if err != nil {
		panic(err)
	}
	return parsed
}
