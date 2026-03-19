package execution

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"sort"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/common/hexutil"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/ethereum/go-ethereum/rpc"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/registry"
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
	FeeUnit                 string     `json:"fee_unit,omitempty"`
	FeeToken                string     `json:"fee_token,omitempty"`
}

type ActionGasEstimateChainTotal struct {
	ChainID         string `json:"chain_id"`
	LikelyFeeWei    string `json:"likely_fee_wei"`
	WorstCaseFeeWei string `json:"worst_case_fee_wei"`
	FeeUnit         string `json:"fee_unit,omitempty"`
	FeeToken        string `json:"fee_token,omitempty"`
}

type preparedEstimateStep struct {
	Step     ActionStep
	Msg      ethereum.CallMsg   // primary call msg (first call for batched steps)
	Msgs     []ethereum.CallMsg // all call msgs (len > 1 for batched Tempo steps)
	ChainKey string
	Client   *ethclient.Client
}

type estimateSimulateBlockResult struct {
	Calls []estimateSimulateCallResult `json:"calls"`
}

type estimateSimulateCallResult struct {
	GasUsed *hexutil.Uint64           `json:"gasUsed"`
	Status  *hexutil.Uint64           `json:"status"`
	Error   *estimateSimulateRPCError `json:"error,omitempty"`
}

type estimateSimulateRPCError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
	Data    string `json:"data,omitempty"`
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

	rpcClients := make(map[string]*ethclient.Client)
	defer func() {
		for _, client := range rpcClients {
			if client != nil {
				client.Close()
			}
		}
	}()

	prepared := make([]preparedEstimateStep, 0, len(selected))
	for _, step := range selected {
		if strings.TrimSpace(step.RPCURL) == "" {
			return ActionGasEstimate{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("step %s is missing rpc_url", step.StepID))
		}

		// Tempo steps use batched Calls; EVM steps use single Target/Data.
		hasCalls := len(step.Calls) > 0
		if !hasCalls {
			if strings.TrimSpace(step.Target) == "" || !common.IsHexAddress(strings.TrimSpace(step.Target)) {
				return ActionGasEstimate{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("step %s has invalid target address", step.StepID))
			}
		}

		client := rpcClients[strings.TrimSpace(step.RPCURL)]
		if client == nil {
			var err error
			client, err = ethclient.DialContext(ctx, strings.TrimSpace(step.RPCURL))
			if err != nil {
				return ActionGasEstimate{}, clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
			}
			rpcClients[strings.TrimSpace(step.RPCURL)] = client
		}

		chainID, err := client.ChainID(ctx)
		if err != nil {
			return ActionGasEstimate{}, clierr.Wrap(clierr.CodeUnavailable, "read chain id", err)
		}
		chainKey := fmt.Sprintf("eip155:%d", chainID.Int64())
		if strings.TrimSpace(step.ChainID) != "" {
			if !strings.EqualFold(strings.TrimSpace(step.ChainID), chainKey) {
				return ActionGasEstimate{}, clierr.New(clierr.CodeActionPlan, fmt.Sprintf("step chain mismatch: expected %s, got %s", chainKey, step.ChainID))
			}
		}

		// Build call messages: for batched Calls, build one per call; otherwise single.
		var msgs []ethereum.CallMsg
		if hasCalls {
			for _, c := range step.Calls {
				m, mErr := stepCallToCallMsg(c, fromAddress)
				if mErr != nil {
					return ActionGasEstimate{}, mErr
				}
				msgs = append(msgs, m)
			}
		} else {
			msg, mErr := actionStepCallMsg(step, fromAddress)
			if mErr != nil {
				return ActionGasEstimate{}, mErr
			}
			msgs = []ethereum.CallMsg{msg}
		}

		prepared = append(prepared, preparedEstimateStep{
			Step:     step,
			Msg:      msgs[0], // primary call msg (used for sequential simulation)
			Msgs:     msgs,    // all call msgs (used for per-call estimation on Tempo)
			ChainKey: chainKey,
			Client:   client,
		})
	}

	rawGasFromSimulation, err := estimateGasSequentialWhereSupported(ctx, prepared, blockTag)
	if err != nil {
		return ActionGasEstimate{}, wrapEVMExecutionError(clierr.CodeActionSim, "simulate action (eth_simulateV1)", err)
	}

	byChainLikely := map[string]*big.Int{}
	byChainWorst := map[string]*big.Int{}
	byChainFeeUnit := map[string]string{}
	byChainFeeToken := map[string]string{}
	estimatedSteps := make([]ActionGasEstimateStep, 0, len(prepared))

	for _, preparedStep := range prepared {
		step := preparedStep.Step
		client := preparedStep.Client
		chainKey := preparedStep.ChainKey
		msg := preparedStep.Msg

		// Parse numeric chain ID from chainKey for Tempo detection.
		numericChainID, _ := ParseEVMChainID(chainKey)
		isTempo := IsTempoChain(numericChainID)

		// Estimate raw gas.
		var rawGas uint64
		if isTempo && len(preparedStep.Msgs) > 1 {
			// For batched Tempo steps, estimate each call and sum.
			for _, m := range preparedStep.Msgs {
				gas, gasErr := estimateGasWithBlockTag(ctx, client, m, blockTag)
				if gasErr != nil {
					return ActionGasEstimate{}, wrapEVMExecutionError(clierr.CodeActionSim, "estimate gas", gasErr)
				}
				rawGas += gas
			}
		} else {
			rawGas = rawGasFromSimulation[strings.ToLower(strings.TrimSpace(step.StepID))]
			if rawGas == 0 {
				var err error
				rawGas, err = estimateGasWithBlockTag(ctx, client, msg, blockTag)
				if err != nil {
					return ActionGasEstimate{}, wrapEVMExecutionError(clierr.CodeActionSim, "estimate gas", err)
				}
			}
		}
		gasLimit := uint64(float64(rawGas) * opts.GasMultiplier)
		if gasLimit == 0 {
			return ActionGasEstimate{}, clierr.New(clierr.CodeActionSim, "estimate gas returned zero")
		}

		tipCap, err := resolveTipCap(ctx, client, opts.MaxPriorityFeeGwei)
		if err != nil {
			return ActionGasEstimate{}, err
		}
		baseFee, err := baseFeeAtBlockTag(ctx, client, blockTag)
		if err != nil {
			return ActionGasEstimate{}, err
		}
		feeCap, err := resolveFeeCap(baseFee, tipCap, opts.MaxFeeGwei)
		if err != nil {
			return ActionGasEstimate{}, err
		}

		effectiveGasPrice := new(big.Int).Add(new(big.Int).Set(baseFee), tipCap)
		if effectiveGasPrice.Cmp(feeCap) > 0 {
			effectiveGasPrice = new(big.Int).Set(feeCap)
		}

		gasLimitBI := new(big.Int).SetUint64(gasLimit)
		likelyFee := new(big.Int).Mul(new(big.Int).Set(gasLimitBI), effectiveGasPrice)
		worstFee := new(big.Int).Mul(new(big.Int).Set(gasLimitBI), feeCap)

		// For Tempo chains, convert fee from 18-decimal gas price to fee-token base units.
		// On Tempo, gasPrice is in 18-decimal USD and fee token (USDC.e) has 6 decimals,
		// so: fee_token_units = fee_wei / 10^(18-6) = fee_wei / 10^12
		var feeUnit, feeToken string
		if isTempo {
			if ft, ok := registry.TempoFeeToken(numericChainID); ok {
				feeToken = ft
				feeUnit = tempoFeeTokenSymbol(ft)
			}
			if feeUnit != "" {
				// Convert 18-decimal denominated fees to 6-decimal token units.
				divisor := new(big.Int).Exp(big.NewInt(10), big.NewInt(12), nil) // 10^(18-6)
				likelyFee = new(big.Int).Div(likelyFee, divisor)
				worstFee = new(big.Int).Div(worstFee, divisor)
			}
		}

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
			FeeUnit:                 feeUnit,
			FeeToken:                feeToken,
		})

		if _, ok := byChainLikely[chainKey]; !ok {
			byChainLikely[chainKey] = big.NewInt(0)
		}
		if _, ok := byChainWorst[chainKey]; !ok {
			byChainWorst[chainKey] = big.NewInt(0)
		}
		byChainLikely[chainKey].Add(byChainLikely[chainKey], likelyFee)
		byChainWorst[chainKey].Add(byChainWorst[chainKey], worstFee)
		if feeUnit != "" {
			byChainFeeUnit[chainKey] = feeUnit
		}
		if feeToken != "" {
			byChainFeeToken[chainKey] = feeToken
		}
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
			FeeUnit:         byChainFeeUnit[chainID],
			FeeToken:        byChainFeeToken[chainID],
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

func estimateGasSequentialWhereSupported(ctx context.Context, prepared []preparedEstimateStep, blockTag EstimateBlockTag) (map[string]uint64, error) {
	if len(prepared) < 2 {
		return map[string]uint64{}, nil
	}
	byRPC := make(map[string][]preparedEstimateStep)
	order := make([]string, 0, len(prepared))
	for _, step := range prepared {
		key := strings.TrimSpace(step.Step.RPCURL)
		if _, ok := byRPC[key]; !ok {
			order = append(order, key)
		}
		byRPC[key] = append(byRPC[key], step)
	}

	out := make(map[string]uint64)
	for _, rpcURL := range order {
		group := byRPC[rpcURL]
		if len(group) < 2 {
			continue
		}
		groupEstimates, supported, err := estimateGasSequentialGroup(ctx, group, blockTag)
		if err != nil {
			return nil, err
		}
		if !supported {
			continue
		}
		for stepID, gas := range groupEstimates {
			out[strings.ToLower(strings.TrimSpace(stepID))] = gas
		}
	}
	return out, nil
}

func estimateGasSequentialGroup(ctx context.Context, group []preparedEstimateStep, blockTag EstimateBlockTag) (map[string]uint64, bool, error) {
	if len(group) < 2 {
		return map[string]uint64{}, false, nil
	}
	if group[0].Client == nil {
		return nil, false, fmt.Errorf("missing rpc client for sequential simulation")
	}

	calls := make([]any, 0, len(group))
	for _, step := range group {
		calls = append(calls, callArgFromCallMsg(step.Msg))
	}

	opts := map[string]any{
		"blockStateCalls": []any{
			map[string]any{
				"calls": calls,
			},
		},
	}

	var raw json.RawMessage
	if err := group[0].Client.Client().CallContext(ctx, &raw, "eth_simulateV1", opts, blockNumberOrHashForEstimateTag(blockTag)); err != nil {
		if isSimulateMethodUnsupported(err) {
			return nil, false, nil
		}
		return nil, false, err
	}
	blocks, err := decodeSimulateBlocks(raw)
	if err != nil {
		return nil, false, err
	}
	if len(blocks) == 0 {
		return nil, false, fmt.Errorf("eth_simulateV1 returned no blocks")
	}
	if len(blocks[0].Calls) != len(group) {
		return nil, false, fmt.Errorf("eth_simulateV1 returned %d calls for %d requested steps", len(blocks[0].Calls), len(group))
	}

	out := make(map[string]uint64, len(group))
	for i, call := range blocks[0].Calls {
		step := group[i].Step
		if call.Error != nil {
			return nil, false, fmt.Errorf("simulate step %s failed: %s", step.StepID, simulateCallErrorText(call.Error))
		}
		if call.Status != nil && uint64(*call.Status) == 0 {
			return nil, false, fmt.Errorf("simulate step %s reverted", step.StepID)
		}
		if call.GasUsed == nil {
			return nil, false, fmt.Errorf("simulate step %s did not return gasUsed", step.StepID)
		}
		gas := uint64(*call.GasUsed)
		if gas == 0 {
			return nil, false, fmt.Errorf("simulate step %s returned zero gas", step.StepID)
		}
		out[step.StepID] = gas
	}
	return out, true, nil
}

func callArgFromCallMsg(msg ethereum.CallMsg) map[string]any {
	arg := map[string]any{
		"from": msg.From,
	}
	if msg.To != nil {
		arg["to"] = msg.To
	}
	if len(msg.Data) > 0 {
		arg["input"] = hexutil.Bytes(msg.Data)
	}
	if msg.Value != nil {
		arg["value"] = (*hexutil.Big)(msg.Value)
	}
	if msg.Gas != 0 {
		arg["gas"] = hexutil.Uint64(msg.Gas)
	}
	if msg.GasPrice != nil {
		arg["gasPrice"] = (*hexutil.Big)(msg.GasPrice)
	}
	if msg.GasFeeCap != nil {
		arg["maxFeePerGas"] = (*hexutil.Big)(msg.GasFeeCap)
	}
	if msg.GasTipCap != nil {
		arg["maxPriorityFeePerGas"] = (*hexutil.Big)(msg.GasTipCap)
	}
	if msg.AccessList != nil {
		arg["accessList"] = msg.AccessList
	}
	return arg
}

func blockNumberOrHashForEstimateTag(blockTag EstimateBlockTag) rpc.BlockNumberOrHash {
	switch blockTag {
	case EstimateBlockTagLatest:
		return rpc.BlockNumberOrHashWithNumber(rpc.LatestBlockNumber)
	default:
		return rpc.BlockNumberOrHashWithNumber(rpc.PendingBlockNumber)
	}
}

func decodeSimulateBlocks(raw json.RawMessage) ([]estimateSimulateBlockResult, error) {
	trimmed := strings.TrimSpace(string(raw))
	if trimmed == "" || trimmed == "null" {
		return nil, fmt.Errorf("empty eth_simulateV1 response")
	}
	var blocks []estimateSimulateBlockResult
	if err := json.Unmarshal(raw, &blocks); err == nil {
		return blocks, nil
	}
	var block estimateSimulateBlockResult
	if err := json.Unmarshal(raw, &block); err == nil {
		return []estimateSimulateBlockResult{block}, nil
	}
	return nil, fmt.Errorf("decode eth_simulateV1 response")
}

func isSimulateMethodUnsupported(err error) bool {
	if err == nil {
		return false
	}
	var rpcErr rpc.Error
	if errors.As(err, &rpcErr) {
		switch rpcErr.ErrorCode() {
		case -32601, -32602:
			return true
		}
	}
	msg := strings.ToLower(err.Error())
	if strings.Contains(msg, "eth_simulatev1") && strings.Contains(msg, "not found") {
		return true
	}
	if strings.Contains(msg, "method not found") || strings.Contains(msg, "does not exist") || strings.Contains(msg, "unknown method") {
		return true
	}
	return false
}

func simulateCallErrorText(err *estimateSimulateRPCError) string {
	if err == nil {
		return "unknown simulation error"
	}
	if strings.TrimSpace(err.Message) != "" {
		return strings.TrimSpace(err.Message)
	}
	if strings.TrimSpace(err.Data) != "" {
		return strings.TrimSpace(err.Data)
	}
	return "unknown simulation error"
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

// stepCallToCallMsg converts a StepCall into an ethereum.CallMsg for gas estimation.
func stepCallToCallMsg(c StepCall, from common.Address) (ethereum.CallMsg, error) {
	if strings.TrimSpace(c.Target) == "" || !common.IsHexAddress(strings.TrimSpace(c.Target)) {
		return ethereum.CallMsg{}, clierr.New(clierr.CodeUsage, "batched call has invalid target address")
	}
	target := common.HexToAddress(strings.TrimSpace(c.Target))
	data, err := decodeHex(c.Data)
	if err != nil {
		return ethereum.CallMsg{}, clierr.Wrap(clierr.CodeUsage, "decode call data", err)
	}
	value, err := parseNonNegativeBaseUnits(c.Value)
	if err != nil {
		return ethereum.CallMsg{}, clierr.Wrap(clierr.CodeUsage, "parse call value", err)
	}
	return ethereum.CallMsg{
		From:  from,
		To:    &target,
		Value: value,
		Data:  data,
	}, nil
}

// tempoFeeTokenSymbol returns a human-readable symbol for known Tempo fee token addresses.
func tempoFeeTokenSymbol(addr string) string {
	// All known Tempo fee token addresses are USDC.e variants.
	// This can be extended with on-chain symbol() calls if needed.
	switch strings.ToLower(strings.TrimSpace(addr)) {
	case "0x20c000000000000000000000b9537d11c60e8b50", // mainnet
		"0x20c0000000000000000000000000000000000001": // testnet/devnet
		return "USDC.e"
	default:
		return "USDC.e"
	}
}
