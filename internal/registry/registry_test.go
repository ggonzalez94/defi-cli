package registry

import (
	"strings"
	"testing"

	"github.com/ethereum/go-ethereum/accounts/abi"
)

func TestUniswapV3Contracts(t *testing.T) {
	quoter, router, ok := UniswapV3Contracts(167000)
	if !ok {
		t.Fatal("expected taiko mainnet contracts to exist")
	}
	if quoter == "" || router == "" {
		t.Fatalf("unexpected empty uniswap-v3 contract values: quoter=%q router=%q", quoter, router)
	}

	if _, _, ok := UniswapV3Contracts(1); ok {
		t.Fatal("did not expect uniswap-v3 contracts for unsupported chain")
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
		UniswapV3QuoterV2ABI,
		UniswapV3RouterABI,
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

func TestDefaultRPCURL(t *testing.T) {
	if rpc, ok := DefaultRPCURL(167000); !ok || rpc == "" {
		t.Fatalf("expected taiko mainnet rpc default, got ok=%v rpc=%q", ok, rpc)
	}
	if rpc, ok := DefaultRPCURL(8453); !ok || rpc == "" {
		t.Fatalf("expected base rpc default, got ok=%v rpc=%q", ok, rpc)
	}
	if _, ok := DefaultRPCURL(999999); ok {
		t.Fatal("did not expect rpc default for unsupported chain")
	}
}

func TestResolveRPCURL(t *testing.T) {
	override, err := ResolveRPCURL(" https://rpc.example.test ", 1)
	if err != nil {
		t.Fatalf("resolve with override: %v", err)
	}
	if override != "https://rpc.example.test" {
		t.Fatalf("unexpected override value: %q", override)
	}

	defaultRPC, err := ResolveRPCURL("", 1)
	if err != nil {
		t.Fatalf("resolve with default: %v", err)
	}
	if defaultRPC == "" {
		t.Fatal("expected non-empty default rpc")
	}

	if _, err := ResolveRPCURL("", 999999); err == nil {
		t.Fatal("expected missing chain default rpc error")
	}
}

func TestBridgeSettlementURL(t *testing.T) {
	got, ok := BridgeSettlementURL("lifi")
	if !ok || got != LiFiSettlementURL {
		t.Fatalf("unexpected lifi settlement url: ok=%v url=%q", ok, got)
	}
	got, ok = BridgeSettlementURL("across")
	if !ok || got != AcrossSettlementURL {
		t.Fatalf("unexpected across settlement url: ok=%v url=%q", ok, got)
	}
	if _, ok := BridgeSettlementURL("unknown"); ok {
		t.Fatal("did not expect settlement url for unknown provider")
	}
}

func TestIsAllowedBridgeSettlementURL(t *testing.T) {
	if !IsAllowedBridgeSettlementURL("lifi", "") {
		t.Fatal("expected empty endpoint to be allowed")
	}
	if !IsAllowedBridgeSettlementURL("lifi", LiFiSettlementURL) {
		t.Fatal("expected canonical lifi endpoint to be allowed")
	}
	if !IsAllowedBridgeSettlementURL("lifi", "https://li.quest:443/v1/status") {
		t.Fatal("expected canonical endpoint with explicit default port to be allowed")
	}
	if IsAllowedBridgeSettlementURL("lifi", AcrossSettlementURL) {
		t.Fatal("did not expect across endpoint to be allowed for lifi")
	}
	if IsAllowedBridgeSettlementURL("lifi", "http://li.quest/v1/status") {
		t.Fatal("did not expect non-https endpoint to be allowed for non-loopback")
	}
	if IsAllowedBridgeSettlementURL("lifi", "https://li.quest/v1/other") {
		t.Fatal("did not expect non-canonical lifi path to be allowed")
	}
	if !IsAllowedBridgeSettlementURL("across", "http://127.0.0.1:8080/status") {
		t.Fatal("expected loopback endpoint to be allowed for tests/dev")
	}
	if IsAllowedBridgeSettlementURL("across", "not-a-url") {
		t.Fatal("did not expect malformed endpoint to be allowed")
	}
}
