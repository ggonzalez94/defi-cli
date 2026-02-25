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
	policyERC20ABI       = mustPolicyABI(registry.ERC20MinimalABI)
	policyTaikoRouterABI = mustPolicyABI(registry.TaikoSwapRouterABI)

	policyApproveSelector = policyERC20ABI.Methods["approve"].ID
	policyTaikoSwapMethod = policyTaikoRouterABI.Methods["exactInputSingle"].ID
)

func validateStepPolicy(action *Action, step *ActionStep, chainID int64, data []byte, opts ExecuteOptions) error {
	if step == nil {
		return clierr.New(clierr.CodeInternal, "missing action step")
	}
	if !common.IsHexAddress(step.Target) {
		return clierr.New(clierr.CodeUsage, "invalid step target address")
	}

	switch step.Type {
	case StepTypeApproval:
		return validateApprovalPolicy(action, data, opts)
	case StepTypeSwap:
		return validateSwapPolicy(action, step, chainID, data)
	case StepTypeBridge:
		return validateBridgePolicy(action, step, opts)
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

func validateSwapPolicy(action *Action, step *ActionStep, chainID int64, data []byte) error {
	if action == nil || !strings.EqualFold(strings.TrimSpace(action.Provider), "taikoswap") {
		return nil
	}
	if len(data) < 4 || !bytes.Equal(data[:4], policyTaikoSwapMethod) {
		return clierr.New(clierr.CodeActionPlan, "taikoswap swap step must call exactInputSingle")
	}
	_, router, ok := registry.TaikoSwapContracts(chainID)
	if !ok {
		return clierr.New(clierr.CodeActionPlan, "taikoswap swap step has unsupported chain")
	}
	expectedRouter := common.HexToAddress(router).Hex()
	if !strings.EqualFold(common.HexToAddress(step.Target).Hex(), expectedRouter) {
		return clierr.New(clierr.CodeActionPlan, "taikoswap swap step target does not match canonical router")
	}
	return nil
}

func validateBridgePolicy(action *Action, step *ActionStep, opts ExecuteOptions) error {
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

func mustPolicyABI(raw string) abi.ABI {
	parsed, err := abi.JSON(strings.NewReader(raw))
	if err != nil {
		panic(err)
	}
	return parsed
}
