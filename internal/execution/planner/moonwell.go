package planner

import (
	"context"
	"fmt"
	"math/big"
	"strings"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/multicall"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

type MoonwellLendRequest struct {
	Verb            AaveLendVerb // reuse same verb type: supply/withdraw/borrow/repay
	Chain           id.Chain
	Asset           id.Asset
	AmountBaseUnits string
	Sender          string
	Recipient       string
	Simulate        bool
	RPCURL          string
	MTokenAddress   string // optional explicit mToken; auto-resolved if empty
}

func BuildMoonwellLendAction(ctx context.Context, req MoonwellLendRequest) (execution.Action, error) {
	verb := strings.ToLower(strings.TrimSpace(string(req.Verb)))
	sender := strings.TrimSpace(req.Sender)
	if !common.IsHexAddress(sender) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "lend action requires sender address")
	}
	recipient := strings.TrimSpace(req.Recipient)
	if recipient == "" {
		recipient = sender
	}
	if !common.IsHexAddress(recipient) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "invalid recipient address")
	}
	if !strings.EqualFold(recipient, sender) {
		return execution.Action{}, clierr.New(clierr.CodeUnsupported, "moonwell does not support alternate recipients; Compound v2 calls operate on msg.sender only")
	}
	if !common.IsHexAddress(req.Asset.Address) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "moonwell lend asset must resolve to an ERC20 address")
	}
	amount, ok := new(big.Int).SetString(strings.TrimSpace(req.AmountBaseUnits), 10)
	if !ok || amount.Sign() <= 0 {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "lend amount must be a positive integer in base units")
	}
	rpcURL, err := registry.ResolveRPCURL(req.RPCURL, req.Chain.EVMChainID)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUsage, "resolve rpc url", err)
	}

	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
	}
	defer client.Close()

	senderAddr := common.HexToAddress(sender)
	recipientAddr := common.HexToAddress(recipient)
	tokenAddr := common.HexToAddress(req.Asset.Address)

	// Resolve mToken address.
	mTokenAddr, err := resolveMoonwellMToken(ctx, client, req.Chain, req.MTokenAddress, tokenAddr)
	if err != nil {
		return execution.Action{}, err
	}

	action := execution.NewAction(execution.NewActionID(), "lend_"+verb, req.Chain.CAIP2, execution.Constraints{Simulate: req.Simulate})
	action.Provider = "moonwell"
	action.FromAddress = senderAddr.Hex()
	action.ToAddress = recipientAddr.Hex()
	action.InputAmount = amount.String()
	action.Metadata = map[string]any{
		"protocol":       "moonwell",
		"asset_id":       req.Asset.AssetID,
		"mtoken":         mTokenAddr.Hex(),
		"lending_action": verb,
	}

	switch verb {
	case string(AaveVerbSupply):
		// Supply: approve underlying → enterMarkets (if needed) → mToken.mint(amount)
		if err := appendApprovalIfNeeded(ctx, client, &action, req.Chain.CAIP2, rpcURL, tokenAddr, senderAddr, mTokenAddr, amount, "Approve token for Moonwell supply"); err != nil {
			return execution.Action{}, err
		}
		// Enable mToken as collateral if not already entered.
		if err := appendEnterMarketsIfNeeded(ctx, client, &action, req.Chain, rpcURL, senderAddr, mTokenAddr); err != nil {
			return execution.Action{}, err
		}
		data, err := moonwellMTokenABI.Pack("mint", amount)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack moonwell mint calldata", err)
		}
		appendStep(&action, "moonwell-supply", execution.StepTypeLend, req.Chain.CAIP2, rpcURL, "Supply asset to Moonwell", mTokenAddr.Hex(), data)

	case string(AaveVerbWithdraw):
		// Withdraw: mToken.redeemUnderlying(amount)
		data, err := moonwellMTokenABI.Pack("redeemUnderlying", amount)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack moonwell redeemUnderlying calldata", err)
		}
		appendStep(&action, "moonwell-withdraw", execution.StepTypeLend, req.Chain.CAIP2, rpcURL, "Withdraw asset from Moonwell", mTokenAddr.Hex(), data)

	case string(AaveVerbBorrow):
		// Borrow: mToken.borrow(amount) — requires collateral
		data, err := moonwellMTokenABI.Pack("borrow", amount)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack moonwell borrow calldata", err)
		}
		appendStep(&action, "moonwell-borrow", execution.StepTypeLend, req.Chain.CAIP2, rpcURL, "Borrow asset from Moonwell", mTokenAddr.Hex(), data)

	case string(AaveVerbRepay):
		// Repay: approve underlying → mToken, then mToken.repayBorrow(amount)
		if err := appendApprovalIfNeeded(ctx, client, &action, req.Chain.CAIP2, rpcURL, tokenAddr, senderAddr, mTokenAddr, amount, "Approve token for Moonwell repay"); err != nil {
			return execution.Action{}, err
		}
		data, err := moonwellMTokenABI.Pack("repayBorrow", amount)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack moonwell repayBorrow calldata", err)
		}
		appendStep(&action, "moonwell-repay", execution.StepTypeLend, req.Chain.CAIP2, rpcURL, "Repay borrowed asset on Moonwell", mTokenAddr.Hex(), data)

	default:
		return execution.Action{}, clierr.New(clierr.CodeUsage, "unsupported moonwell lend action verb")
	}

	return action, nil
}

// appendEnterMarketsIfNeeded checks if the sender has already entered the mToken market
// as collateral. If not, appends a Comptroller.enterMarkets([mToken]) step.
func appendEnterMarketsIfNeeded(ctx context.Context, client *ethclient.Client, action *execution.Action, chain id.Chain, rpcURL string, sender, mToken common.Address) error {
	comptrollerAddr, ok := registry.MoonwellComptroller(chain.EVMChainID)
	if !ok {
		// No comptroller — skip check; enterMarkets not possible.
		return nil
	}
	comptroller := common.HexToAddress(comptrollerAddr)

	// Check if already a member.
	checkData, err := moonwellComptrollerABI.Pack("checkMembership", sender, mToken)
	if err != nil {
		return clierr.Wrap(clierr.CodeInternal, "pack checkMembership", err)
	}
	checkOut, err := client.CallContract(ctx, ethereum.CallMsg{To: &comptroller, Data: checkData}, nil)
	if err != nil {
		return clierr.Wrap(clierr.CodeUnavailable, "call checkMembership", err)
	}
	decoded, err := moonwellComptrollerABI.Unpack("checkMembership", checkOut)
	if err != nil {
		return clierr.Wrap(clierr.CodeUnavailable, "decode checkMembership", err)
	}
	isMember, ok := decoded[0].(bool)
	if ok && isMember {
		return nil // already entered
	}

	// Build enterMarkets calldata.
	enterData, err := moonwellComptrollerABI.Pack("enterMarkets", []common.Address{mToken})
	if err != nil {
		return clierr.Wrap(clierr.CodeInternal, "pack enterMarkets", err)
	}
	appendStep(action, "moonwell-enter-market", execution.StepTypeLend, chain.CAIP2, rpcURL, "Enable asset as collateral on Moonwell", comptroller.Hex(), enterData)
	return nil
}

// resolveMoonwellMToken resolves the mToken contract for a given underlying asset.
// If mTokenAddress is provided explicitly (via --pool-address), use it directly.
// Otherwise, call Comptroller.getAllMarkets() and batch-resolve underlying() via Multicall3.
func resolveMoonwellMToken(ctx context.Context, client *ethclient.Client, chain id.Chain, mTokenAddress string, underlying common.Address) (common.Address, error) {
	if strings.TrimSpace(mTokenAddress) != "" {
		if !common.IsHexAddress(mTokenAddress) {
			return common.Address{}, clierr.New(clierr.CodeUsage, "invalid --pool-address (mToken address)")
		}
		return common.HexToAddress(mTokenAddress), nil
	}

	comptrollerAddr, ok := registry.MoonwellComptroller(chain.EVMChainID)
	if !ok {
		return common.Address{}, clierr.New(clierr.CodeUnsupported, "moonwell is not supported on this chain; pass --pool-address with the mToken address")
	}
	comptroller := common.HexToAddress(comptrollerAddr)

	// RPC call 1: getAllMarkets().
	data, err := moonwellComptrollerABI.Pack("getAllMarkets")
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeInternal, "pack getAllMarkets", err)
	}
	out, err := client.CallContract(ctx, ethereum.CallMsg{To: &comptroller, Data: data}, nil)
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeUnavailable, "call getAllMarkets", err)
	}
	decoded, err := moonwellComptrollerABI.Unpack("getAllMarkets", out)
	if err != nil || len(decoded) == 0 {
		return common.Address{}, clierr.Wrap(clierr.CodeUnavailable, "decode getAllMarkets", err)
	}
	markets, ok := decoded[0].([]common.Address)
	if !ok {
		return common.Address{}, clierr.New(clierr.CodeUnavailable, "invalid getAllMarkets response")
	}

	// RPC call 2: batch underlying() for all markets via Multicall3.
	underlyingCD, err := moonwellMTokenABI.Pack("underlying")
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeInternal, "pack underlying calldata", err)
	}

	calls := make([]multicall.Call, len(markets))
	for i, mt := range markets {
		calls[i] = multicall.Call{Target: mt, AllowFailure: true, CallData: underlyingCD}
	}

	results, err := multicall.Aggregate3(ctx, client, calls)
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeUnavailable, "multicall3 underlying resolution", err)
	}

	for i, r := range results {
		if !r.Success || len(r.ReturnData) < 32 {
			continue
		}
		addr := common.BytesToAddress(r.ReturnData[12:32])
		if strings.EqualFold(addr.Hex(), underlying.Hex()) {
			return markets[i], nil
		}
	}

	return common.Address{}, clierr.New(clierr.CodeUnsupported, fmt.Sprintf("no moonwell mToken found for underlying %s on chain %d; pass --pool-address with the mToken address", underlying.Hex(), chain.EVMChainID))
}

var moonwellMTokenABI = mustPlannerABI(registry.MoonwellMTokenABI)
var moonwellComptrollerABI = mustPlannerABI(registry.MoonwellComptrollerABI)
