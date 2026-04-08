package planner

import (
	"context"
	"fmt"
	"math/big"
	"strings"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/accounts/abi"
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
		appendStep(&action, "aave-supply", execution.StepTypeLend, req.Chain.CAIP2, rpcURL, "Supply asset to Aave", poolAddr.Hex(), data)
	case string(AaveVerbWithdraw):
		data, err := aavePoolABI.Pack("withdraw", tokenAddr, amount, recipient)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack aave withdraw calldata", err)
		}
		appendStep(&action, "aave-withdraw", execution.StepTypeLend, req.Chain.CAIP2, rpcURL, "Withdraw asset from Aave", poolAddr.Hex(), data)
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
		appendStep(&action, "aave-borrow", execution.StepTypeLend, req.Chain.CAIP2, rpcURL, "Borrow asset from Aave", poolAddr.Hex(), data)
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
		appendStep(&action, "aave-repay", execution.StepTypeLend, req.Chain.CAIP2, rpcURL, "Repay borrowed asset on Aave", poolAddr.Hex(), data)
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
	appendStep(&action, "aave-claim-rewards", execution.StepTypeClaim, req.Chain.CAIP2, rpcURL, "Claim rewards from Aave incentives controller", controller.Hex(), data)
	return action, nil
}

func BuildAaveRewardsCompoundAction(ctx context.Context, req AaveRewardsCompoundRequest) (execution.Action, error) {
	if strings.EqualFold(strings.TrimSpace(req.AmountBaseUnits), "max") {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "compound requires an explicit --amount in base units (max is unsupported)")
	}
	senderInput := strings.TrimSpace(req.Sender)
	recipientInput := strings.TrimSpace(req.Recipient)
	if recipientInput != "" && !strings.EqualFold(recipientInput, senderInput) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "compound requires --recipient to match --from-address")
	}
	claimAction, err := BuildAaveRewardsClaimAction(ctx, AaveRewardsClaimRequest{
		Chain:                 req.Chain,
		Sender:                senderInput,
		Recipient:             senderInput,
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
	onBehalfOfInput := strings.TrimSpace(req.OnBehalfOf)
	if onBehalfOfInput != "" {
		if !common.IsHexAddress(onBehalfOfInput) {
			return execution.Action{}, clierr.New(clierr.CodeUsage, "invalid on-behalf-of address")
		}
		onBehalfOf = common.HexToAddress(onBehalfOfInput)
	}
	rewardAddr := common.HexToAddress(req.RewardToken)
	if err := appendApprovalIfNeeded(ctx, client, &claimAction, req.Chain.CAIP2, rpcURL, rewardAddr, sender, poolAddr, amount, "Approve reward token for Aave supply"); err != nil {
		return execution.Action{}, err
	}
	supplyData, err := aavePoolABI.Pack("supply", rewardAddr, amount, onBehalfOf, uint16(0))
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack aave compound supply calldata", err)
	}
	appendStep(&claimAction, "aave-compound-supply", execution.StepTypeLend, req.Chain.CAIP2, rpcURL, "Supply claimed reward token to Aave", poolAddr.Hex(), supplyData)
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
	provider, err := resolveAaveProviderAddr(chain, poolProvider)
	if err != nil {
		if ce, ok := clierr.As(err); ok && ce.Code == clierr.CodeUsage {
			return common.Address{}, err
		}
		return common.Address{}, clierr.New(clierr.CodeUnsupported, "aave pool address provider is unavailable for this chain; pass --pool-address or --pool-address-provider")
	}
	callData, err := aavePoolAddressProviderABI.Pack("getPool")
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeInternal, "pack getPool calldata", err)
	}
	return callContractForAddress(ctx, client, provider, aavePoolAddressProviderABI, "getPool", callData, "aave pool address")
}

func resolveIncentivesController(ctx context.Context, client *ethclient.Client, chain id.Chain, controllerAddress string, poolProvider string) (common.Address, error) {
	if strings.TrimSpace(controllerAddress) != "" {
		if !common.IsHexAddress(controllerAddress) {
			return common.Address{}, clierr.New(clierr.CodeUsage, "invalid --controller-address")
		}
		return common.HexToAddress(controllerAddress), nil
	}
	provider, err := resolveAaveProviderAddr(chain, poolProvider)
	if err != nil {
		if ce, ok := clierr.As(err); ok && ce.Code == clierr.CodeUsage {
			return common.Address{}, err
		}
		return common.Address{}, clierr.New(clierr.CodeUnsupported, "aave incentives controller is unavailable for this chain; pass --controller-address")
	}
	slot := crypto.Keccak256Hash([]byte("INCENTIVES_CONTROLLER"))
	callData, err := aavePoolAddressProviderABI.Pack("getAddress", slot)
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeInternal, "pack getAddress calldata", err)
	}
	return callContractForAddress(ctx, client, provider, aavePoolAddressProviderABI, "getAddress", callData, "incentives controller address")
}

// resolveAaveProviderAddr resolves the Aave pool address provider for a chain,
// using the explicit value if given, otherwise falling back to the registry.
func resolveAaveProviderAddr(chain id.Chain, poolProvider string) (common.Address, error) {
	providerAddr := strings.TrimSpace(poolProvider)
	if providerAddr == "" {
		if discovered, ok := registry.AavePoolAddressProvider(chain.EVMChainID); ok {
			providerAddr = discovered
		}
	}
	if providerAddr == "" {
		return common.Address{}, fmt.Errorf("no provider address available")
	}
	if !common.IsHexAddress(providerAddr) {
		return common.Address{}, clierr.New(clierr.CodeUsage, "invalid --pool-address-provider")
	}
	return common.HexToAddress(providerAddr), nil
}

// callContractForAddress calls a contract method that returns a single address,
// handling ABI unpacking, type assertion (value or pointer), and zero-address validation.
func callContractForAddress(ctx context.Context, client *ethclient.Client, target common.Address, contractABI abi.ABI, method string, callData []byte, label string) (common.Address, error) {
	out, err := client.CallContract(ctx, ethereum.CallMsg{To: &target, Data: callData}, nil)
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeUnavailable, "fetch "+label, err)
	}
	decoded, err := contractABI.Unpack(method, out)
	if err != nil || len(decoded) == 0 {
		return common.Address{}, clierr.Wrap(clierr.CodeUnavailable, "decode "+label, err)
	}
	addr, ok := decoded[0].(common.Address)
	if !ok {
		if ptr, ok := decoded[0].(*common.Address); ok && ptr != nil {
			addr = *ptr
		} else {
			return common.Address{}, clierr.New(clierr.CodeUnavailable, "invalid "+label+" response")
		}
	}
	if addr == (common.Address{}) {
		return common.Address{}, clierr.New(clierr.CodeUnavailable, label+" is zero")
	}
	return addr, nil
}

func appendApprovalIfNeeded(ctx context.Context, client *ethclient.Client, action *execution.Action, chainID, rpcURL string, token, owner, spender common.Address, amount *big.Int, description string) error {
	currentAllowance, err := execution.ReadTokenAllowance(ctx, client, token, owner, spender)
	if err != nil {
		return err
	}
	if currentAllowance.Cmp(amount) >= 0 {
		return nil
	}
	approveData, err := plannerERC20ABI.Pack("approve", spender, amount)
	if err != nil {
		return clierr.Wrap(clierr.CodeInternal, "pack approve calldata", err)
	}
	appendStep(action, fmt.Sprintf("approve-%s", strings.TrimPrefix(strings.ToLower(token.Hex()), "0x")), execution.StepTypeApproval, chainID, rpcURL, description, token.Hex(), approveData)
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

var aavePoolAddressProviderABI = registry.MustParseABI(registry.AavePoolAddressProviderABI)

var aavePoolABI = registry.MustParseABI(registry.AavePoolABI)

var aaveRewardsABI = registry.MustParseABI(registry.AaveRewardsABI)
