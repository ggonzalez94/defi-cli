package registry

import (
	"strings"
	"testing"

	"github.com/ethereum/go-ethereum/accounts/abi"
)

func TestTaikoSwapContracts(t *testing.T) {
	quoter, router, ok := TaikoSwapContracts(167000)
	if !ok {
		t.Fatal("expected taiko mainnet contracts to exist")
	}
	if quoter == "" || router == "" {
		t.Fatalf("unexpected empty taikoswap contract values: quoter=%q router=%q", quoter, router)
	}

	if _, _, ok := TaikoSwapContracts(1); ok {
		t.Fatal("did not expect taikoswap contracts for unsupported chain")
	}
}

func TestAavePoolAddressProvider(t *testing.T) {
	cases := []int64{1, 8453, 42161, 10, 137, 43114}
	for _, chainID := range cases {
		addr, ok := AavePoolAddressProvider(chainID)
		if !ok || addr == "" {
			t.Fatalf("expected aave pool address provider for chain %d", chainID)
		}
	}
	if _, ok := AavePoolAddressProvider(167000); ok {
		t.Fatal("did not expect aave pool address provider for unsupported chain")
	}
}

func TestExecutionABIConstantsParse(t *testing.T) {
	abis := []string{
		ERC20MinimalABI,
		TaikoSwapQuoterV2ABI,
		TaikoSwapRouterABI,
		AavePoolAddressProviderABI,
		AavePoolABI,
		AaveRewardsABI,
		MorphoBlueABI,
	}
	for _, raw := range abis {
		if _, err := abi.JSON(strings.NewReader(raw)); err != nil {
			t.Fatalf("failed to parse abi json: %v", err)
		}
	}
}
