package execution

import (
	"context"
	"fmt"
	"math/big"
	"sort"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/common/hexutil"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
)

type EstimateBlockTag string

const (
	EstimateBlockTagLatest  EstimateBlockTag = "latest"
	EstimateBlockTagPending EstimateBlockTag = "pending"
)

type EstimateOptions struct {
	StepIDs            []string
	GasMultiplier      float64
	MaxFeeGwei         string
	MaxPriorityFeeGwei string
	BlockTag           EstimateBlockTag
}

type ActionGasEstimate struct {
	ActionID      string                        `json:"action_id"`
	EstimatedAt   string                        `json:"estimated_at"`
	BlockTag      string                        `json:"block_tag"`
	Steps         []ActionGasEstimateStep       `json:"steps"`
	TotalsByChain []ActionGasEstimateChainTotal `json:"totals_by_chain"`
}

type ActionGasEstimateStep struct {
	StepID                  string     `json:"step_id"`
	Type                    StepType   `json:"type"`
	Status                  StepStatus `json:"status"`
	ChainID                 string     `json:"chain_id"`
	GasEstimateRaw          string     `json:"gas_estimate_raw"`
	GasLimit                string     `json:"gas_limit"`
	BaseFeePerGasWei        string     `json:"base_fee_per_gas_wei"`
	MaxPriorityFeePerGasWei string     `json:"max_priority_fee_per_gas_wei"`
	MaxFeePerGasWei         string     `json:"max_fee_per_gas_wei"`
	EffectiveGasPriceWei    string     `json:"effective_gas_price_wei"`
	LikelyFeeWei            string     `json:"likely_fee_wei"`
	WorstCaseFeeWei         string     `json:"worst_case_fee_wei"`
}

type ActionGasEstimateChainTotal struct {
	ChainID         string `json:"chain_id"`
	LikelyFeeWei    string `json:"likely_fee_wei"`
	WorstCaseFeeWei string `json:"worst_case_fee_wei"`
}

func DefaultEstimateOptions() EstimateOptions {
	return EstimateOptions{
		GasMultiplier: 1.2,
		BlockTag:      EstimateBlockTagPending,
	}
}

func EstimateActionGas(ctx context.Context, action Action, opts EstimateOptions) (ActionGasEstimate, error) {
	if strings.TrimSpace(action.ActionID) == "" {
		return ActionGasEstimate{}, clierr.New(clierr.CodeUsage, "missing action id")
	}
	if len(action.Steps) == 0 {
		return ActionGasEstimate{}, clierr.New(clierr.CodeUsage, "action has no executable steps")
	}
	if opts.GasMultiplier <= 1 {
		return ActionGasEstimate{}, clierr.New(clierr.CodeUsage, "--gas-multiplier must be > 1")
	}
	blockTag, err := normalizeEstimateBlockTag(opts.BlockTag)
	if err != nil {
		return ActionGasEstimate{}, err
	}

	fromAddress := common.Address{}
	if strings.TrimSpace(action.FromAddress) != "" {
		if !common.IsHexAddress(strings.TrimSpace(action.FromAddress)) {
			return ActionGasEstimate{}, clierr.New(clierr.CodeUsage, "action has invalid from_address")
		}
		fromAddress = common.HexToAddress(strings.TrimSpace(action.FromAddress))
	}

	stepFilter := buildStepFilter(opts.StepIDs)
	selected := make([]ActionStep, 0, len(action.Steps))
	for _, step := range action.Steps {
		if !matchesStepFilter(stepFilter, step.StepID) {
			continue
		}
		selected = append(selected, step)
	}
	if len(selected) == 0 {
		return ActionGasEstimate{}, clierr.New(clierr.CodeUsage, "no action steps matched the requested --step-ids filter")
	}

	byChainLikely := map[string]*big.Int{}
	byChainWorst := map[string]*big.Int{}
	estimatedSteps := make([]ActionGasEstimateStep, 0, len(selected))

	for _, step := range selected {
		if strings.TrimSpace(step.RPCURL) == "" {
			return ActionGasEstimate{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("step %s is missing rpc_url", step.StepID))
		}
		if strings.TrimSpace(step.Target) == "" || !common.IsHexAddress(strings.TrimSpace(step.Target)) {
			return ActionGasEstimate{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("step %s has invalid target address", step.StepID))
		}

		client, err := ethclient.DialContext(ctx, strings.TrimSpace(step.RPCURL))
		if err != nil {
			return ActionGasEstimate{}, clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
		}

		msg, err := actionStepCallMsg(step, fromAddress)
		if err != nil {
			client.Close()
			return ActionGasEstimate{}, err
		}

		chainID, err := client.ChainID(ctx)
		if err != nil {
			client.Close()
			return ActionGasEstimate{}, clierr.Wrap(clierr.CodeUnavailable, "read chain id", err)
		}
		chainKey := fmt.Sprintf("eip155:%d", chainID.Int64())
		if strings.TrimSpace(step.ChainID) != "" {
			if !strings.EqualFold(strings.TrimSpace(step.ChainID), chainKey) {
				client.Close()
				return ActionGasEstimate{}, clierr.New(clierr.CodeActionPlan, fmt.Sprintf("step chain mismatch: expected %s, got %s", chainKey, step.ChainID))
			}
		}

		rawGas, err := estimateGasWithBlockTag(ctx, client, msg, blockTag)
		if err != nil {
			client.Close()
			return ActionGasEstimate{}, wrapEVMExecutionError(clierr.CodeActionSim, "estimate gas", err)
		}
		gasLimit := uint64(float64(rawGas) * opts.GasMultiplier)
		if gasLimit == 0 {
			client.Close()
			return ActionGasEstimate{}, clierr.New(clierr.CodeActionSim, "estimate gas returned zero")
		}

		tipCap, err := resolveTipCap(ctx, client, opts.MaxPriorityFeeGwei)
		if err != nil {
			client.Close()
			return ActionGasEstimate{}, err
		}
		baseFee, err := baseFeeAtBlockTag(ctx, client, blockTag)
		if err != nil {
			client.Close()
			return ActionGasEstimate{}, err
		}
		feeCap, err := resolveFeeCap(baseFee, tipCap, opts.MaxFeeGwei)
		if err != nil {
			client.Close()
			return ActionGasEstimate{}, err
		}
		client.Close()

		effectiveGasPrice := new(big.Int).Add(new(big.Int).Set(baseFee), tipCap)
		if effectiveGasPrice.Cmp(feeCap) > 0 {
			effectiveGasPrice = new(big.Int).Set(feeCap)
		}

		gasLimitBI := new(big.Int).SetUint64(gasLimit)
		likelyFee := new(big.Int).Mul(new(big.Int).Set(gasLimitBI), effectiveGasPrice)
		worstFee := new(big.Int).Mul(new(big.Int).Set(gasLimitBI), feeCap)

		estimatedSteps = append(estimatedSteps, ActionGasEstimateStep{
			StepID:                  step.StepID,
			Type:                    step.Type,
			Status:                  step.Status,
			ChainID:                 chainKey,
			GasEstimateRaw:          strconvUint64(rawGas),
			GasLimit:                strconvUint64(gasLimit),
			BaseFeePerGasWei:        baseFee.String(),
			MaxPriorityFeePerGasWei: tipCap.String(),
			MaxFeePerGasWei:         feeCap.String(),
			EffectiveGasPriceWei:    effectiveGasPrice.String(),
			LikelyFeeWei:            likelyFee.String(),
			WorstCaseFeeWei:         worstFee.String(),
		})

		if _, ok := byChainLikely[chainKey]; !ok {
			byChainLikely[chainKey] = big.NewInt(0)
		}
		if _, ok := byChainWorst[chainKey]; !ok {
			byChainWorst[chainKey] = big.NewInt(0)
		}
		byChainLikely[chainKey].Add(byChainLikely[chainKey], likelyFee)
		byChainWorst[chainKey].Add(byChainWorst[chainKey], worstFee)
	}

	totals := make([]ActionGasEstimateChainTotal, 0, len(byChainLikely))
	chainIDs := make([]string, 0, len(byChainLikely))
	for chainID := range byChainLikely {
		chainIDs = append(chainIDs, chainID)
	}
	sort.Strings(chainIDs)
	for _, chainID := range chainIDs {
		totals = append(totals, ActionGasEstimateChainTotal{
			ChainID:         chainID,
			LikelyFeeWei:    byChainLikely[chainID].String(),
			WorstCaseFeeWei: byChainWorst[chainID].String(),
		})
	}

	return ActionGasEstimate{
		ActionID:      action.ActionID,
		EstimatedAt:   time.Now().UTC().Format(time.RFC3339),
		BlockTag:      string(blockTag),
		Steps:         estimatedSteps,
		TotalsByChain: totals,
	}, nil
}

func actionStepCallMsg(step ActionStep, from common.Address) (ethereum.CallMsg, error) {
	target := common.HexToAddress(strings.TrimSpace(step.Target))
	data, err := decodeHex(step.Data)
	if err != nil {
		return ethereum.CallMsg{}, clierr.Wrap(clierr.CodeUsage, "decode step calldata", err)
	}
	value, err := parseNonNegativeBaseUnits(step.Value)
	if err != nil {
		return ethereum.CallMsg{}, clierr.Wrap(clierr.CodeUsage, "parse step value", err)
	}
	return ethereum.CallMsg{
		From:  from,
		To:    &target,
		Value: value,
		Data:  data,
	}, nil
}

func parseNonNegativeBaseUnits(raw string) (*big.Int, error) {
	clean := strings.TrimSpace(raw)
	if clean == "" {
		return big.NewInt(0), nil
	}
	value, ok := new(big.Int).SetString(clean, 10)
	if !ok {
		return nil, fmt.Errorf("invalid base-units integer")
	}
	if value.Sign() < 0 {
		return nil, fmt.Errorf("value must be non-negative")
	}
	return value, nil
}

func normalizeEstimateBlockTag(input EstimateBlockTag) (EstimateBlockTag, error) {
	switch strings.ToLower(strings.TrimSpace(string(input))) {
	case "", string(EstimateBlockTagPending):
		return EstimateBlockTagPending, nil
	case string(EstimateBlockTagLatest):
		return EstimateBlockTagLatest, nil
	default:
		return "", clierr.New(clierr.CodeUsage, "--block-tag must be one of: pending,latest")
	}
}

func buildStepFilter(stepIDs []string) map[string]struct{} {
	if len(stepIDs) == 0 {
		return nil
	}
	out := make(map[string]struct{}, len(stepIDs))
	for _, stepID := range stepIDs {
		if normalized := strings.ToLower(strings.TrimSpace(stepID)); normalized != "" {
			out[normalized] = struct{}{}
		}
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

func matchesStepFilter(filter map[string]struct{}, stepID string) bool {
	if len(filter) == 0 {
		return true
	}
	_, ok := filter[strings.ToLower(strings.TrimSpace(stepID))]
	return ok
}

func estimateGasWithBlockTag(ctx context.Context, client *ethclient.Client, msg ethereum.CallMsg, blockTag EstimateBlockTag) (uint64, error) {
	arg := map[string]any{
		"from": msg.From.Hex(),
	}
	if msg.To != nil {
		arg["to"] = msg.To.Hex()
	}
	if len(msg.Data) > 0 {
		arg["data"] = hexutil.Bytes(msg.Data)
	}
	if msg.Value != nil {
		arg["value"] = (*hexutil.Big)(msg.Value)
	}

	var estimated hexutil.Uint64
	if err := client.Client().CallContext(ctx, &estimated, "eth_estimateGas", arg, string(blockTag)); err != nil {
		if blockTag == EstimateBlockTagPending {
			if retryErr := client.Client().CallContext(ctx, &estimated, "eth_estimateGas", arg, string(EstimateBlockTagLatest)); retryErr == nil {
				return uint64(estimated), nil
			}
		}
		fallback, fallbackErr := client.EstimateGas(ctx, msg)
		if fallbackErr == nil {
			return fallback, nil
		}
		return 0, err
	}
	return uint64(estimated), nil
}

func baseFeeAtBlockTag(ctx context.Context, client *ethclient.Client, blockTag EstimateBlockTag) (*big.Int, error) {
	var block struct {
		BaseFeePerGas *hexutil.Big `json:"baseFeePerGas"`
	}
	if err := client.Client().CallContext(ctx, &block, "eth_getBlockByNumber", string(blockTag), false); err != nil {
		if blockTag == EstimateBlockTagPending {
			if retryErr := client.Client().CallContext(ctx, &block, "eth_getBlockByNumber", string(EstimateBlockTagLatest), false); retryErr == nil {
				if block.BaseFeePerGas == nil {
					return big.NewInt(1_000_000_000), nil
				}
				return new(big.Int).Set((*big.Int)(block.BaseFeePerGas)), nil
			}
		}
		header, headerErr := client.HeaderByNumber(ctx, nil)
		if headerErr == nil {
			if header.BaseFee == nil {
				return big.NewInt(1_000_000_000), nil
			}
			return new(big.Int).Set(header.BaseFee), nil
		}
		return nil, clierr.Wrap(clierr.CodeUnavailable, "fetch latest header", err)
	}
	if block.BaseFeePerGas == nil {
		return big.NewInt(1_000_000_000), nil
	}
	return new(big.Int).Set((*big.Int)(block.BaseFeePerGas)), nil
}

func strconvUint64(v uint64) string {
	return new(big.Int).SetUint64(v).String()
}
