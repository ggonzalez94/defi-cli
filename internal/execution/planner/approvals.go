package planner

import (
	"fmt"
	"math/big"
	"strings"

	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/common"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

type ApprovalRequest struct {
	Chain           id.Chain
	Asset           id.Asset
	AmountBaseUnits string
	Sender          string
	Spender         string
	Simulate        bool
	RPCURL          string
}

func BuildApprovalAction(req ApprovalRequest) (execution.Action, error) {
	sender := strings.TrimSpace(req.Sender)
	if sender == "" {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "approval requires sender address")
	}
	if !common.IsHexAddress(sender) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "approval sender must be a valid EVM address")
	}
	spender := strings.TrimSpace(req.Spender)
	if spender == "" {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "approval requires spender address")
	}
	if !common.IsHexAddress(spender) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "approval spender must be a valid EVM address")
	}
	if !common.IsHexAddress(req.Asset.Address) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "approval requires ERC20 token address")
	}
	amount, ok := new(big.Int).SetString(strings.TrimSpace(req.AmountBaseUnits), 10)
	if !ok || amount.Sign() <= 0 {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "approval amount must be a positive integer in base units")
	}

	rpcURL, err := registry.ResolveRPCURL(req.RPCURL, req.Chain.EVMChainID)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUsage, "resolve rpc url", err)
	}

	approveData, err := plannerERC20ABI.Pack("approve", common.HexToAddress(spender), amount)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack approval calldata", err)
	}
	action := execution.NewAction(execution.NewActionID(), "approve", req.Chain.CAIP2, execution.Constraints{Simulate: req.Simulate})
	action.Provider = "native"
	action.FromAddress = common.HexToAddress(sender).Hex()
	action.ToAddress = common.HexToAddress(spender).Hex()
	action.InputAmount = amount.String()
	action.Metadata = map[string]any{
		"asset_id": req.Asset.AssetID,
		"spender":  common.HexToAddress(spender).Hex(),
	}
	action.Steps = append(action.Steps, execution.ActionStep{
		StepID:      "approve-token",
		Type:        execution.StepTypeApproval,
		Status:      execution.StepStatusPending,
		ChainID:     req.Chain.CAIP2,
		RPCURL:      rpcURL,
		Description: fmt.Sprintf("Approve %s for spender", strings.ToUpper(req.Asset.Symbol)),
		Target:      common.HexToAddress(req.Asset.Address).Hex(),
		Data:        "0x" + common.Bytes2Hex(approveData),
		Value:       "0",
	})
	return action, nil
}

var plannerERC20ABI = mustPlannerABI(registry.ERC20MinimalABI)

func mustPlannerABI(raw string) abi.ABI {
	parsed, err := abi.JSON(strings.NewReader(raw))
	if err != nil {
		panic(err)
	}
	return parsed
}
