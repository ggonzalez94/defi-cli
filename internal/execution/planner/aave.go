package planner

import (
	"context"
	"fmt"
	"math/big"
	"strings"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/crypto"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

type AaveLendVerb string

const (
	AaveVerbSupply   AaveLendVerb = "supply"
	AaveVerbWithdraw AaveLendVerb = "withdraw"
	AaveVerbBorrow   AaveLendVerb = "borrow"
	AaveVerbRepay    AaveLendVerb = "repay"
)

type AaveLendRequest struct {
	Verb                  AaveLendVerb
	Chain                 id.Chain
	Asset                 id.Asset
	AmountBaseUnits       string
	Sender                string
	Recipient             string
	OnBehalfOf            string
	InterestRateMode      int64
	Simulate              bool
	RPCURL                string
	PoolAddress           string
	PoolAddressesProvider string
}

type AaveRewardsClaimRequest struct {
	Chain                 id.Chain
	Sender                string
	Recipient             string
	Assets                []string
	RewardToken           string
	AmountBaseUnits       string
	Simulate              bool
	RPCURL                string
	ControllerAddress     string
	PoolAddressesProvider string
}

type AaveRewardsCompoundRequest struct {
	Chain                 id.Chain
	Sender                string
	Recipient             string
	Assets                []string
	RewardToken           string
	AmountBaseUnits       string
	Simulate              bool
	RPCURL                string
	ControllerAddress     string
	PoolAddress           string
	PoolAddressesProvider string
	OnBehalfOf            string
}

func BuildAaveLendAction(ctx context.Context, req AaveLendRequest) (execution.Action, error) {
	verb := strings.ToLower(strings.TrimSpace(string(req.Verb)))
	sender, recipient, onBehalfOf, amount, rpcURL, tokenAddr, err := normalizeLendInputs(req)
	if err != nil {
		return execution.Action{}, err
	}

	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
	}
	defer client.Close()

	poolAddr, err := resolveAavePoolAddress(ctx, client, req.Chain, req.PoolAddress, req.PoolAddressesProvider)
	if err != nil {
		return execution.Action{}, err
	}
	action := execution.NewAction(execution.NewActionID(), "lend_"+verb, req.Chain.CAIP2, execution.Constraints{Simulate: req.Simulate})
	action.Provider = "aave"
	action.FromAddress = sender.Hex()
	action.ToAddress = recipient.Hex()
	action.InputAmount = amount.String()
	action.Metadata = map[string]any{
		"protocol":       "aave",
		"asset_id":       req.Asset.AssetID,
		"pool":           poolAddr.Hex(),
		"on_behalf_of":   onBehalfOf.Hex(),
		"recipient":      recipient.Hex(),
		"rate_mode":      req.InterestRateMode,
		"lending_action": verb,
	}

	switch verb {
	case string(AaveVerbSupply):
		if err := appendApprovalIfNeeded(ctx, client, &action, req.Chain.CAIP2, rpcURL, tokenAddr, sender, poolAddr, amount, "Approve token for Aave supply"); err != nil {
			return execution.Action{}, err
		}
		data, err := aavePoolABI.Pack("supply", tokenAddr, amount, onBehalfOf, uint16(0))
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack aave supply calldata", err)
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      "aave-supply",
			Type:        execution.StepTypeLend,
			Status:      execution.StepStatusPending,
			ChainID:     req.Chain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Supply asset to Aave",
			Target:      poolAddr.Hex(),
			Data:        "0x" + common.Bytes2Hex(data),
			Value:       "0",
		})
	case string(AaveVerbWithdraw):
		data, err := aavePoolABI.Pack("withdraw", tokenAddr, amount, recipient)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack aave withdraw calldata", err)
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      "aave-withdraw",
			Type:        execution.StepTypeLend,
			Status:      execution.StepStatusPending,
			ChainID:     req.Chain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Withdraw asset from Aave",
			Target:      poolAddr.Hex(),
			Data:        "0x" + common.Bytes2Hex(data),
			Value:       "0",
		})
	case string(AaveVerbBorrow):
		rateMode := req.InterestRateMode
		if rateMode == 0 {
			rateMode = 2
		}
		if rateMode != 1 && rateMode != 2 {
			return execution.Action{}, clierr.New(clierr.CodeUsage, "borrow interest rate mode must be 1 (stable) or 2 (variable)")
		}
		data, err := aavePoolABI.Pack("borrow", tokenAddr, amount, big.NewInt(rateMode), uint16(0), onBehalfOf)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack aave borrow calldata", err)
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      "aave-borrow",
			Type:        execution.StepTypeLend,
			Status:      execution.StepStatusPending,
			ChainID:     req.Chain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Borrow asset from Aave",
			Target:      poolAddr.Hex(),
			Data:        "0x" + common.Bytes2Hex(data),
			Value:       "0",
		})
	case string(AaveVerbRepay):
		rateMode := req.InterestRateMode
		if rateMode == 0 {
			rateMode = 2
		}
		if rateMode != 1 && rateMode != 2 {
			return execution.Action{}, clierr.New(clierr.CodeUsage, "repay interest rate mode must be 1 (stable) or 2 (variable)")
		}
		if err := appendApprovalIfNeeded(ctx, client, &action, req.Chain.CAIP2, rpcURL, tokenAddr, sender, poolAddr, amount, "Approve token for Aave repay"); err != nil {
			return execution.Action{}, err
		}
		data, err := aavePoolABI.Pack("repay", tokenAddr, amount, big.NewInt(rateMode), onBehalfOf)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack aave repay calldata", err)
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      "aave-repay",
			Type:        execution.StepTypeLend,
			Status:      execution.StepStatusPending,
			ChainID:     req.Chain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Repay borrowed asset on Aave",
			Target:      poolAddr.Hex(),
			Data:        "0x" + common.Bytes2Hex(data),
			Value:       "0",
		})
	default:
		return execution.Action{}, clierr.New(clierr.CodeUsage, "unsupported lend action verb")
	}

	return action, nil
}

func BuildAaveRewardsClaimAction(ctx context.Context, req AaveRewardsClaimRequest) (execution.Action, error) {
	sender := strings.TrimSpace(req.Sender)
	if !common.IsHexAddress(sender) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "rewards claim requires sender address")
	}
	recipient := strings.TrimSpace(req.Recipient)
	if recipient == "" {
		recipient = sender
	}
	if !common.IsHexAddress(recipient) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "invalid rewards recipient address")
	}
	if !common.IsHexAddress(req.RewardToken) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "reward token must be an address")
	}
	assets, err := normalizeAddressList(req.Assets)
	if err != nil {
		return execution.Action{}, err
	}
	if len(assets) == 0 {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "rewards claim requires at least one asset in --assets")
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

	controller, err := resolveIncentivesController(ctx, client, req.Chain, req.ControllerAddress, req.PoolAddressesProvider)
	if err != nil {
		return execution.Action{}, err
	}
	amount, err := parseRewardAmount(req.AmountBaseUnits)
	if err != nil {
		return execution.Action{}, err
	}
	assetAddrs := make([]common.Address, 0, len(assets))
	for _, a := range assets {
		assetAddrs = append(assetAddrs, common.HexToAddress(a))
	}
	data, err := aaveRewardsABI.Pack("claimRewards", assetAddrs, amount, common.HexToAddress(recipient), common.HexToAddress(req.RewardToken))
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack rewards claim calldata", err)
	}
	action := execution.NewAction(execution.NewActionID(), "claim_rewards", req.Chain.CAIP2, execution.Constraints{Simulate: req.Simulate})
	action.Provider = "aave"
	action.FromAddress = common.HexToAddress(sender).Hex()
	action.ToAddress = common.HexToAddress(recipient).Hex()
	action.InputAmount = amount.String()
	action.Metadata = map[string]any{
		"protocol":          "aave",
		"controller":        controller.Hex(),
		"reward_token":      common.HexToAddress(req.RewardToken).Hex(),
		"assets":            assets,
		"amount_base_units": amount.String(),
	}
	action.Steps = append(action.Steps, execution.ActionStep{
		StepID:      "aave-claim-rewards",
		Type:        execution.StepTypeClaim,
		Status:      execution.StepStatusPending,
		ChainID:     req.Chain.CAIP2,
		RPCURL:      rpcURL,
		Description: "Claim rewards from Aave incentives controller",
		Target:      controller.Hex(),
		Data:        "0x" + common.Bytes2Hex(data),
		Value:       "0",
	})
	return action, nil
}

func BuildAaveRewardsCompoundAction(ctx context.Context, req AaveRewardsCompoundRequest) (execution.Action, error) {
	if strings.EqualFold(strings.TrimSpace(req.AmountBaseUnits), "max") {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "compound requires an explicit --amount in base units (max is unsupported)")
	}
	claimAction, err := BuildAaveRewardsClaimAction(ctx, AaveRewardsClaimRequest{
		Chain:                 req.Chain,
		Sender:                req.Sender,
		Recipient:             req.Recipient,
		Assets:                req.Assets,
		RewardToken:           req.RewardToken,
		AmountBaseUnits:       req.AmountBaseUnits,
		Simulate:              req.Simulate,
		RPCURL:                req.RPCURL,
		ControllerAddress:     req.ControllerAddress,
		PoolAddressesProvider: req.PoolAddressesProvider,
	})
	if err != nil {
		return execution.Action{}, err
	}
	claimAction.ActionID = execution.NewActionID()
	claimAction.IntentType = "compound_rewards"
	claimAction.Metadata["compound"] = true

	rpcURL, err := registry.ResolveRPCURL(req.RPCURL, req.Chain.EVMChainID)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUsage, "resolve rpc url", err)
	}
	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
	}
	defer client.Close()

	poolAddr, err := resolveAavePoolAddress(ctx, client, req.Chain, req.PoolAddress, req.PoolAddressesProvider)
	if err != nil {
		return execution.Action{}, err
	}
	amount, ok := new(big.Int).SetString(strings.TrimSpace(req.AmountBaseUnits), 10)
	if !ok || amount.Sign() <= 0 {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "compound amount must be a positive integer in base units")
	}
	sender := common.HexToAddress(strings.TrimSpace(req.Sender))
	onBehalfOf := sender
	if strings.TrimSpace(req.OnBehalfOf) != "" {
		onBehalfOf = common.HexToAddress(req.OnBehalfOf)
	}
	rewardAddr := common.HexToAddress(req.RewardToken)
	if err := appendApprovalIfNeeded(ctx, client, &claimAction, req.Chain.CAIP2, rpcURL, rewardAddr, sender, poolAddr, amount, "Approve reward token for Aave supply"); err != nil {
		return execution.Action{}, err
	}
	supplyData, err := aavePoolABI.Pack("supply", rewardAddr, amount, onBehalfOf, uint16(0))
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack aave compound supply calldata", err)
	}
	claimAction.Steps = append(claimAction.Steps, execution.ActionStep{
		StepID:      "aave-compound-supply",
		Type:        execution.StepTypeLend,
		Status:      execution.StepStatusPending,
		ChainID:     req.Chain.CAIP2,
		RPCURL:      rpcURL,
		Description: "Supply claimed reward token to Aave",
		Target:      poolAddr.Hex(),
		Data:        "0x" + common.Bytes2Hex(supplyData),
		Value:       "0",
	})
	claimAction.Metadata["pool"] = poolAddr.Hex()
	claimAction.Metadata["on_behalf_of"] = onBehalfOf.Hex()
	return claimAction, nil
}

func normalizeLendInputs(req AaveLendRequest) (common.Address, common.Address, common.Address, *big.Int, string, common.Address, error) {
	sender := strings.TrimSpace(req.Sender)
	if !common.IsHexAddress(sender) {
		return common.Address{}, common.Address{}, common.Address{}, nil, "", common.Address{}, clierr.New(clierr.CodeUsage, "lend action requires sender address")
	}
	recipient := strings.TrimSpace(req.Recipient)
	if recipient == "" {
		recipient = sender
	}
	if !common.IsHexAddress(recipient) {
		return common.Address{}, common.Address{}, common.Address{}, nil, "", common.Address{}, clierr.New(clierr.CodeUsage, "invalid recipient address")
	}
	onBehalfOf := strings.TrimSpace(req.OnBehalfOf)
	if onBehalfOf == "" {
		onBehalfOf = sender
	}
	if !common.IsHexAddress(onBehalfOf) {
		return common.Address{}, common.Address{}, common.Address{}, nil, "", common.Address{}, clierr.New(clierr.CodeUsage, "invalid on-behalf-of address")
	}
	if !common.IsHexAddress(req.Asset.Address) {
		return common.Address{}, common.Address{}, common.Address{}, nil, "", common.Address{}, clierr.New(clierr.CodeUsage, "lend asset must resolve to an ERC20 address")
	}
	amount, ok := new(big.Int).SetString(strings.TrimSpace(req.AmountBaseUnits), 10)
	if !ok || amount.Sign() <= 0 {
		return common.Address{}, common.Address{}, common.Address{}, nil, "", common.Address{}, clierr.New(clierr.CodeUsage, "lend amount must be a positive integer in base units")
	}
	rpcURL, err := registry.ResolveRPCURL(req.RPCURL, req.Chain.EVMChainID)
	if err != nil {
		return common.Address{}, common.Address{}, common.Address{}, nil, "", common.Address{}, clierr.Wrap(clierr.CodeUsage, "resolve rpc url", err)
	}
	return common.HexToAddress(sender), common.HexToAddress(recipient), common.HexToAddress(onBehalfOf), amount, rpcURL, common.HexToAddress(req.Asset.Address), nil
}

func resolveAavePoolAddress(ctx context.Context, client *ethclient.Client, chain id.Chain, poolAddress string, poolProvider string) (common.Address, error) {
	if strings.TrimSpace(poolAddress) != "" {
		if !common.IsHexAddress(poolAddress) {
			return common.Address{}, clierr.New(clierr.CodeUsage, "invalid --pool-address")
		}
		return common.HexToAddress(poolAddress), nil
	}
	providerAddr := strings.TrimSpace(poolProvider)
	if providerAddr == "" {
		if discovered, ok := registry.AavePoolAddressProvider(chain.EVMChainID); ok {
			providerAddr = discovered
		}
	}
	if providerAddr == "" {
		return common.Address{}, clierr.New(clierr.CodeUnsupported, "aave pool address provider is unavailable for this chain; pass --pool-address or --pool-address-provider")
	}
	if !common.IsHexAddress(providerAddr) {
		return common.Address{}, clierr.New(clierr.CodeUsage, "invalid --pool-address-provider")
	}
	provider := common.HexToAddress(providerAddr)
	callData, err := aavePoolAddressProviderABI.Pack("getPool")
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeInternal, "pack getPool calldata", err)
	}
	out, err := client.CallContract(ctx, ethereum.CallMsg{To: &provider, Data: callData}, nil)
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeUnavailable, "fetch aave pool address", err)
	}
	decoded, err := aavePoolAddressProviderABI.Unpack("getPool", out)
	if err != nil || len(decoded) == 0 {
		return common.Address{}, clierr.Wrap(clierr.CodeUnavailable, "decode aave pool address", err)
	}
	pool, ok := decoded[0].(common.Address)
	if !ok {
		if ptr, ok := decoded[0].(*common.Address); ok && ptr != nil {
			pool = *ptr
		} else {
			return common.Address{}, clierr.New(clierr.CodeUnavailable, "invalid aave pool response")
		}
	}
	if pool == (common.Address{}) {
		return common.Address{}, clierr.New(clierr.CodeUnavailable, "aave pool address is zero")
	}
	return pool, nil
}

func resolveIncentivesController(ctx context.Context, client *ethclient.Client, chain id.Chain, controllerAddress string, poolProvider string) (common.Address, error) {
	if strings.TrimSpace(controllerAddress) != "" {
		if !common.IsHexAddress(controllerAddress) {
			return common.Address{}, clierr.New(clierr.CodeUsage, "invalid --controller-address")
		}
		return common.HexToAddress(controllerAddress), nil
	}
	providerAddr := strings.TrimSpace(poolProvider)
	if providerAddr == "" {
		if discovered, ok := registry.AavePoolAddressProvider(chain.EVMChainID); ok {
			providerAddr = discovered
		}
	}
	if providerAddr == "" {
		return common.Address{}, clierr.New(clierr.CodeUnsupported, "aave incentives controller is unavailable for this chain; pass --controller-address")
	}
	if !common.IsHexAddress(providerAddr) {
		return common.Address{}, clierr.New(clierr.CodeUsage, "invalid --pool-address-provider")
	}
	provider := common.HexToAddress(providerAddr)
	slot := crypto.Keccak256Hash([]byte("INCENTIVES_CONTROLLER"))
	callData, err := aavePoolAddressProviderABI.Pack("getAddress", slot)
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeInternal, "pack getAddress calldata", err)
	}
	out, err := client.CallContract(ctx, ethereum.CallMsg{To: &provider, Data: callData}, nil)
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeUnavailable, "fetch incentives controller address", err)
	}
	decoded, err := aavePoolAddressProviderABI.Unpack("getAddress", out)
	if err != nil || len(decoded) == 0 {
		return common.Address{}, clierr.Wrap(clierr.CodeUnavailable, "decode incentives controller address", err)
	}
	controller, ok := decoded[0].(common.Address)
	if !ok {
		if ptr, ok := decoded[0].(*common.Address); ok && ptr != nil {
			controller = *ptr
		} else {
			return common.Address{}, clierr.New(clierr.CodeUnavailable, "invalid incentives controller response")
		}
	}
	if controller == (common.Address{}) {
		return common.Address{}, clierr.New(clierr.CodeUnavailable, "incentives controller address is zero")
	}
	return controller, nil
}

func appendApprovalIfNeeded(ctx context.Context, client *ethclient.Client, action *execution.Action, chainID, rpcURL string, token, owner, spender common.Address, amount *big.Int, description string) error {
	allowanceData, err := plannerERC20ABI.Pack("allowance", owner, spender)
	if err != nil {
		return clierr.Wrap(clierr.CodeInternal, "pack allowance calldata", err)
	}
	allowanceRaw, err := client.CallContract(ctx, ethereum.CallMsg{From: owner, To: &token, Data: allowanceData}, nil)
	if err != nil {
		return clierr.Wrap(clierr.CodeUnavailable, "read token allowance", err)
	}
	allowanceOut, err := plannerERC20ABI.Unpack("allowance", allowanceRaw)
	if err != nil || len(allowanceOut) == 0 {
		return clierr.Wrap(clierr.CodeUnavailable, "decode token allowance", err)
	}
	currentAllowance, ok := allowanceOut[0].(*big.Int)
	if !ok {
		return clierr.New(clierr.CodeUnavailable, "invalid allowance response")
	}
	if currentAllowance.Cmp(amount) >= 0 {
		return nil
	}
	approveData, err := plannerERC20ABI.Pack("approve", spender, amount)
	if err != nil {
		return clierr.Wrap(clierr.CodeInternal, "pack approve calldata", err)
	}
	action.Steps = append(action.Steps, execution.ActionStep{
		StepID:      fmt.Sprintf("approve-%s", strings.TrimPrefix(strings.ToLower(token.Hex()), "0x")),
		Type:        execution.StepTypeApproval,
		Status:      execution.StepStatusPending,
		ChainID:     chainID,
		RPCURL:      rpcURL,
		Description: description,
		Target:      token.Hex(),
		Data:        "0x" + common.Bytes2Hex(approveData),
		Value:       "0",
	})
	return nil
}

func normalizeAddressList(values []string) ([]string, error) {
	out := make([]string, 0, len(values))
	seen := make(map[string]struct{}, len(values))
	for _, value := range values {
		for _, part := range strings.Split(value, ",") {
			norm := strings.TrimSpace(part)
			if norm == "" {
				continue
			}
			if !common.IsHexAddress(norm) {
				return nil, clierr.New(clierr.CodeUsage, fmt.Sprintf("invalid address in --assets: %s", norm))
			}
			canonical := common.HexToAddress(norm).Hex()
			if _, ok := seen[canonical]; ok {
				continue
			}
			seen[canonical] = struct{}{}
			out = append(out, canonical)
		}
	}
	return out, nil
}

func parseRewardAmount(v string) (*big.Int, error) {
	clean := strings.TrimSpace(v)
	if clean == "" || strings.EqualFold(clean, "max") {
		max := new(big.Int)
		max.Sub(new(big.Int).Lsh(big.NewInt(1), 256), big.NewInt(1))
		return max, nil
	}
	amount, ok := new(big.Int).SetString(clean, 10)
	if !ok || amount.Sign() <= 0 {
		return nil, clierr.New(clierr.CodeUsage, "reward amount must be a positive integer in base units or 'max'")
	}
	return amount, nil
}

var aavePoolAddressProviderABI = mustPlannerABI(registry.AavePoolAddressProviderABI)

var aavePoolABI = mustPlannerABI(registry.AavePoolABI)

var aaveRewardsABI = mustPlannerABI(registry.AaveRewardsABI)
