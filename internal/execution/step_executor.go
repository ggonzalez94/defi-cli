package execution

import (
	"context"
	"fmt"
	"strconv"
	"strings"

	"github.com/ethereum/go-ethereum/common"
)

// StepExecutor abstracts per-step transaction execution so different chain
// runtimes (EVM EIP-1559, Tempo, etc.) can be plugged in.
type StepExecutor interface {
	// ExecuteStep executes a single action step. It handles policy validation,
	// simulation, gas estimation, nonce management, signing, broadcasting,
	// and receipt polling. The caller is responsible for persistence.
	ExecuteStep(ctx context.Context, store *Store, action *Action, step *ActionStep, opts ExecuteOptions) error

	// EstimateStep returns a gas/fee estimate for a single step without
	// broadcasting a transaction.
	EstimateStep(ctx context.Context, action *Action, step *ActionStep, opts EstimateOptions) (StepGasEstimate, error)

	// EffectiveSender returns the address that will sign and send transactions.
	EffectiveSender() common.Address
}

// StepGasEstimate holds gas and fee estimates for a single action step.
type StepGasEstimate struct {
	GasEstimateRaw       string `json:"gas_estimate_raw"`
	GasLimit             string `json:"gas_limit"`
	BaseFeePerGasWei     string `json:"base_fee_per_gas_wei"`
	MaxPriorityFeeWei    string `json:"max_priority_fee_per_gas_wei"`
	MaxFeePerGasWei      string `json:"max_fee_per_gas_wei"`
	EffectiveGasPriceWei string `json:"effective_gas_price_wei"`
	LikelyFeeWei         string `json:"likely_fee_wei"`
	WorstCaseFeeWei      string `json:"worst_case_fee_wei"`
	FeeUnit              string `json:"fee_unit,omitempty"`  // "ETH" or "USDC.e" etc
	FeeToken             string `json:"fee_token,omitempty"` // token address for non-ETH fees
}

// IsTempoChain returns true if the given numeric chain ID belongs to a Tempo
// network (mainnet, testnet, or devnet).
func IsTempoChain(chainID int64) bool {
	switch chainID {
	case 4217, 42431, 31318:
		return true
	}
	return false
}

// ParseEVMChainID extracts the numeric chain ID from a CAIP-2 string like
// "eip155:4217". It returns an error if the format is invalid.
func ParseEVMChainID(caip2 string) (int64, error) {
	trimmed := strings.TrimSpace(caip2)
	if trimmed == "" {
		return 0, fmt.Errorf("empty chain id")
	}
	after, found := strings.CutPrefix(strings.ToLower(trimmed), "eip155:")
	if !found {
		// Try parsing as a plain numeric chain ID.
		v, err := strconv.ParseInt(trimmed, 10, 64)
		if err != nil {
			return 0, fmt.Errorf("invalid CAIP-2 chain id %q", caip2)
		}
		return v, nil
	}
	v, err := strconv.ParseInt(after, 10, 64)
	if err != nil {
		return 0, fmt.Errorf("invalid CAIP-2 chain id %q: %w", caip2, err)
	}
	return v, nil
}
