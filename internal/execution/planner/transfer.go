package planner

import (
	"fmt"
	"math/big"
	"strings"

	"github.com/ethereum/go-ethereum/common"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

type TransferRequest struct {
	Chain           id.Chain
	Asset           id.Asset
	AmountBaseUnits string
	Sender          string
	Recipient       string
	Simulate        bool
	RPCURL          string
}

func BuildTransferAction(req TransferRequest) (execution.Action, error) {
	if !req.Chain.IsEVM() {
		return execution.Action{}, clierr.New(clierr.CodeUnsupported, "transfer currently supports EVM chains only")
	}

	sender := strings.TrimSpace(req.Sender)
	if sender == "" {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "transfer requires sender address")
	}
	if !common.IsHexAddress(sender) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "transfer sender must be a valid EVM address")
	}

	recipient := strings.TrimSpace(req.Recipient)
	if recipient == "" {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "transfer requires recipient address")
	}
	if !common.IsHexAddress(recipient) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "transfer recipient must be a valid EVM address")
	}
	if common.HexToAddress(recipient) == (common.Address{}) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "transfer recipient cannot be zero address")
	}

	if !common.IsHexAddress(req.Asset.Address) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "transfer requires ERC20 token address")
	}

	amount, ok := new(big.Int).SetString(strings.TrimSpace(req.AmountBaseUnits), 10)
	if !ok || amount.Sign() <= 0 {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "transfer amount must be a positive integer in base units")
	}

	rpcURL, err := registry.ResolveRPCURL(req.RPCURL, req.Chain.EVMChainID)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUsage, "resolve rpc url", err)
	}

	transferData, err := plannerERC20ABI.Pack("transfer", common.HexToAddress(recipient), amount)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack transfer calldata", err)
	}

	action := execution.NewAction(execution.NewActionID(), "transfer", req.Chain.CAIP2, execution.Constraints{Simulate: req.Simulate})
	action.Provider = "native"
	action.FromAddress = common.HexToAddress(sender).Hex()
	action.ToAddress = common.HexToAddress(recipient).Hex()
	action.InputAmount = amount.String()
	action.Metadata = map[string]any{
		"asset_id":      req.Asset.AssetID,
		"asset_address": common.HexToAddress(req.Asset.Address).Hex(),
		"recipient":     common.HexToAddress(recipient).Hex(),
	}
	action.Steps = append(action.Steps, execution.ActionStep{
		StepID:      "transfer-token",
		Type:        execution.StepTypeTransfer,
		Status:      execution.StepStatusPending,
		ChainID:     req.Chain.CAIP2,
		RPCURL:      rpcURL,
		Description: fmt.Sprintf("Transfer %s to recipient", strings.ToUpper(req.Asset.Symbol)),
		Target:      common.HexToAddress(req.Asset.Address).Hex(),
		Data:        "0x" + common.Bytes2Hex(transferData),
		Value:       "0",
	})
	return action, nil
}
