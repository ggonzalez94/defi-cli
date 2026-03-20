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

func TestHasBridgeExecutionTargetPolicy(t *testing.T) {
	// LiFi must cover all major EVM chains where the Diamond is deployed.
	lifiChains := []struct {
		chainID int64
		name    string
	}{
		{1, "Ethereum"},
		{10, "Optimism"},
		{56, "BSC"},
		{100, "Gnosis"},
		{137, "Polygon"},
		{146, "Sonic"},
		{252, "Fraxtal"},
		{324, "zkSync"},
		{480, "World Chain"},
		{5000, "Mantle"},
		{8453, "Base"},
		{42161, "Arbitrum"},
		{42220, "Celo"},
		{43114, "Avalanche"},
		{57073, "Ink"},
		{59144, "Linea"},
		{80094, "Berachain"},
		{81457, "Blast"},
		{167000, "Taiko"},
		{534352, "Scroll"},
	}
	for _, tc := range lifiChains {
		if !HasBridgeExecutionTargetPolicy("lifi", tc.chainID) {
			t.Fatalf("expected lifi target policy coverage for %s (chain %d)", tc.name, tc.chainID)
		}
	}

	// Across must cover its supported chains.
	acrossChains := []struct {
		chainID int64
		name    string
	}{
		{1, "Ethereum"},
		{10, "Optimism"},
		{137, "Polygon"},
		{8453, "Base"},
		{42161, "Arbitrum"},
	}
	for _, tc := range acrossChains {
		if !HasBridgeExecutionTargetPolicy("across", tc.chainID) {
			t.Fatalf("expected across target policy coverage for %s (chain %d)", tc.name, tc.chainID)
		}
	}

	if HasBridgeExecutionTargetPolicy("across", 43114) {
		t.Fatal("did not expect across target policy coverage for unsupported chain")
	}
	if HasBridgeExecutionTargetPolicy("unknown", 1) {
		t.Fatal("did not expect target policy coverage for unknown provider")
	}
}

func TestIsAllowedBridgeExecutionTarget(t *testing.T) {
	// The standard LiFi Diamond address is shared across most major chains.
	lifiDiamond := "0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE"
	standardLiFiChains := []struct {
		chainID int64
		name    string
	}{
		{1, "Ethereum"},
		{10, "Optimism"},
		{56, "BSC"},
		{100, "Gnosis"},
		{137, "Polygon"},
		{146, "Sonic"},
		{252, "Fraxtal"},
		{480, "World Chain"},
		{5000, "Mantle"},
		{8453, "Base"},
		{42161, "Arbitrum"},
		{42220, "Celo"},
		{43114, "Avalanche"},
		{81457, "Blast"},
		{534352, "Scroll"},
	}
	for _, tc := range standardLiFiChains {
		if !IsAllowedBridgeExecutionTarget("lifi", tc.chainID, lifiDiamond) {
			t.Fatalf("expected canonical lifi diamond to be allowed on %s (chain %d)", tc.name, tc.chainID)
		}
	}

	// Case-insensitive: all-lowercase form of the same LiFi diamond address must also pass.
	if !IsAllowedBridgeExecutionTarget("lifi", 8453, "0x1231deb6f5749ef6ce6943a275a1d3e7486f4eae") {
		t.Fatal("expected lowercase lifi target to be allowed (case-insensitive)")
	}
	if IsAllowedBridgeExecutionTarget("lifi", 8453, "0x1111111111111111111111111111111111111111") {
		t.Fatal("did not expect unknown lifi target to be allowed")
	}

	// Chains with non-standard LiFi Diamond addresses should accept their specific address.
	if !IsAllowedBridgeExecutionTarget("lifi", 324, "0x341e94069f53234fE6DabeF707aD424830525715") {
		t.Fatal("expected lifi zkSync-specific diamond to be allowed")
	}
	// Standard diamond must NOT be accepted on chains with non-standard addresses.
	if IsAllowedBridgeExecutionTarget("lifi", 324, lifiDiamond) {
		t.Fatal("did not expect standard lifi diamond on zkSync (uses different address)")
	}

	if !IsAllowedBridgeExecutionTarget("across", 1, "0x767e4c20F521a829dE4Ffc40C25176676878147f") {
		t.Fatal("expected canonical across target to be allowed on mainnet")
	}
	// Case-insensitive: all-uppercase hex also matches.
	if !IsAllowedBridgeExecutionTarget("across", 1, "0x767E4C20F521A829DE4FFC40C25176676878147F") {
		t.Fatal("expected uppercase across target to be allowed (case-insensitive)")
	}
	if IsAllowedBridgeExecutionTarget("across", 1, "not-an-address") {
		t.Fatal("did not expect malformed target to be allowed")
	}
	if IsAllowedBridgeExecutionTarget("across", 43114, "0x767e4c20F521a829dE4Ffc40C25176676878147f") {
		t.Fatal("did not expect target without chain coverage to be allowed")
	}
	if IsAllowedBridgeExecutionTarget("across", 1, "0x1231DeB6f5749EF6Ce6943a275A1D3E7486F4EaE") {
		t.Fatal("did not expect unrelated provider target to be allowed")
	}
	// Empty target must not be allowed.
	if IsAllowedBridgeExecutionTarget("lifi", 1, "") {
		t.Fatal("did not expect empty target to be allowed")
	}
}
