package app

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/config"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/ggonzalez94/defi-cli/internal/version"
	"github.com/spf13/cobra"
)

func findProviderInfo(items []map[string]any, name string) (map[string]any, bool) {
	for _, item := range items {
		rawName, ok := item["name"].(string)
		if !ok {
			continue
		}
		if strings.EqualFold(rawName, name) {
			return item, true
		}
	}
	return nil, false
}

func TestTrimRootPath(t *testing.T) {
	if got := trimRootPath("defi yield opportunities"); got != "yield opportunities" {
		t.Fatalf("unexpected trim result: %s", got)
	}
}

func TestSplitCSV(t *testing.T) {
	items := splitCSV("Aave, morpho ,")
	if len(items) != 2 || items[0] != "aave" || items[1] != "morpho" {
		t.Fatalf("unexpected split: %#v", items)
	}
}

func TestSelectYieldProvidersDefaultsFilterByChainFamily(t *testing.T) {
	state := &runtimeState{
		yieldProviders: map[string]providers.YieldProvider{
			"aave":   nil,
			"morpho": nil,
			"kamino": nil,
		},
	}

	tests := []struct {
		name       string
		chainInput string
		want       []string
	}{
		{name: "evm", chainInput: "base", want: []string{"aave", "morpho"}},
		{name: "solana", chainInput: "solana", want: []string{"kamino"}},
	}

	for _, tc := range tests {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			chain, err := id.ParseChain(tc.chainInput)
			if err != nil {
				t.Fatalf("parse chain: %v", err)
			}
			got, err := state.selectYieldProviders(nil, chain)
			if err != nil {
				t.Fatalf("selectYieldProviders failed: %v", err)
			}
			if len(got) != len(tc.want) {
				t.Fatalf("expected %v providers, got %v", tc.want, got)
			}
			for i := range got {
				if got[i] != tc.want[i] {
					t.Fatalf("expected providers %v, got %v", tc.want, got)
				}
			}
		})
	}
}

func TestSelectYieldProvidersExplicitFilterBypassesChainDefaults(t *testing.T) {
	state := &runtimeState{
		yieldProviders: map[string]providers.YieldProvider{
			"aave":   nil,
			"kamino": nil,
		},
	}
	chain, err := id.ParseChain("base")
	if err != nil {
		t.Fatalf("parse chain: %v", err)
	}

	got, err := state.selectYieldProviders([]string{"kamino"}, chain)
	if err != nil {
		t.Fatalf("selectYieldProviders failed: %v", err)
	}
	if len(got) != 1 || got[0] != "kamino" {
		t.Fatalf("expected explicit provider selection to be preserved, got %v", got)
	}
}

func TestParseChainAssetFilterAllowsUnknownSymbol(t *testing.T) {
	chain, err := id.ParseChain("ethereum")
	if err != nil {
		t.Fatalf("parse chain: %v", err)
	}
	asset, err := parseChainAssetFilter(chain, "NOTAREALTOKEN")
	if err != nil {
		t.Fatalf("expected unknown symbol to be accepted, got err=%v", err)
	}
	if asset.Symbol != "NOTAREALTOKEN" {
		t.Fatalf("expected NOTAREALTOKEN symbol, got %+v", asset)
	}
	if asset.AssetID != "" {
		t.Fatalf("expected empty asset id for non-registry symbol, got %s", asset.AssetID)
	}
}

func TestParseChainAssetFilterRejectsUnknownAddress(t *testing.T) {
	chain, err := id.ParseChain("ethereum")
	if err != nil {
		t.Fatalf("parse chain: %v", err)
	}
	_, err = parseChainAssetFilter(chain, "0x0000000000000000000000000000000000000001")
	if err == nil {
		t.Fatal("expected error for address without known chain symbol")
	}
}

func TestRunnerProvidersList(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"providers", "list", "--results-only"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}
	var out []map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &out); err != nil {
		t.Fatalf("failed to parse output json: %v output=%s", err, stdout.String())
	}
	if len(out) == 0 {
		t.Fatalf("expected providers output, got empty")
	}
	fibrousInfo, ok := findProviderInfo(out, "fibrous")
	if !ok {
		t.Fatalf("expected fibrous provider in providers list, got %#v", out)
	}
	if requiresKey, ok := fibrousInfo["requires_key"].(bool); !ok || requiresKey {
		t.Fatalf("expected fibrous requires_key=false, got %#v", fibrousInfo["requires_key"])
	}
	jupiterCount := 0
	for _, item := range out {
		name, _ := item["name"].(string)
		if strings.EqualFold(name, "jupiter") {
			jupiterCount++
		}
	}
	if jupiterCount != 1 {
		t.Fatalf("expected exactly one jupiter provider entry, got %d", jupiterCount)
	}
}

func TestRunnerErrorEnvelopeIgnoresResultsOnly(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"chains", "top", "--enable-commands", "yield opportunities", "--results-only"})
	if code != 16 {
		t.Fatalf("expected exit 16, got %d stderr=%s", code, stderr.String())
	}
	var env map[string]any
	if err := json.Unmarshal(stderr.Bytes(), &env); err != nil {
		t.Fatalf("failed to parse error envelope: %v output=%s", err, stderr.String())
	}
	if env["success"] != false {
		t.Fatalf("expected success=false, got %v", env["success"])
	}
}

func TestRunnerVersion(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"version"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}
	if got := stdout.String(); got != version.CLIVersion+"\n" {
		t.Fatalf("unexpected version output: %q", got)
	}
}

func TestRunnerVersionLong(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"version", "--long"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}
	out := stdout.String()
	if !strings.Contains(out, version.CLIVersion) {
		t.Fatalf("expected long version output to include version %q, got %q", version.CLIVersion, out)
	}
	if !strings.Contains(out, "commit:") {
		t.Fatalf("expected long version output to include commit field, got %q", out)
	}
	if !strings.Contains(out, "built:") {
		t.Fatalf("expected long version output to include build date field, got %q", out)
	}
}

func TestRunnerVersionBypassesCacheOpen(t *testing.T) {
	setUnopenableCacheEnv(t)

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"version"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}
	if got := strings.TrimSpace(stdout.String()); got != version.CLIVersion {
		t.Fatalf("unexpected version output: %q", got)
	}
}

func TestRunnerSchemaBypassesCacheOpen(t *testing.T) {
	setUnopenableCacheEnv(t)

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"schema", "yield opportunities", "--results-only"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}
	var schema map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &schema); err != nil {
		t.Fatalf("failed to parse schema json: %v output=%s", err, stdout.String())
	}
	if schema["path"] != "defi yield opportunities" {
		t.Fatalf("unexpected schema path: %v", schema["path"])
	}
}

func TestRunnerProvidersListBypassesCacheOpen(t *testing.T) {
	setUnopenableCacheEnv(t)

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"providers", "list", "--results-only"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}
	var out []map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &out); err != nil {
		t.Fatalf("failed to parse providers output json: %v output=%s", err, stdout.String())
	}
	if len(out) == 0 {
		t.Fatalf("expected providers output, got empty")
	}
	fibrousInfo, ok := findProviderInfo(out, "fibrous")
	if !ok {
		t.Fatalf("expected fibrous provider in providers list, got %#v", out)
	}
	if requiresKey, ok := fibrousInfo["requires_key"].(bool); !ok || requiresKey {
		t.Fatalf("expected fibrous requires_key=false, got %#v", fibrousInfo["requires_key"])
	}
}

func TestRunnerAssetsResolveFallsBackWhenCacheUnavailable(t *testing.T) {
	setUnopenableCacheEnv(t)

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"assets", "resolve", "--chain", "1", "--asset", "USDC", "--results-only"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}
	var out map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &out); err != nil {
		t.Fatalf("failed to parse assets resolve output json: %v output=%s", err, stdout.String())
	}
	if out["asset_id"] == "" {
		t.Fatalf("expected asset_id in output, got %+v", out)
	}
	if chainID, _ := out["chain_id"].(string); chainID != "eip155:1" {
		t.Fatalf("expected chain_id eip155:1, got %q", chainID)
	}
}

func TestRunnerProtocolsCategories(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		marketProvider: fakeMarketProvider{
			categories: []model.ProtocolCategory{
				{Name: "Lending", Protocols: 2, TVLUSD: 15000},
			},
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newProtocolsCommand())
	root.SetArgs([]string{"protocols", "categories"})
	if err := root.Execute(); err != nil {
		t.Fatalf("expected protocols categories command success, err=%v stderr=%s", err, stderr.String())
	}

	var env map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &env); err != nil {
		t.Fatalf("failed to parse output json: %v output=%s", err, stdout.String())
	}
	if env["success"] != true {
		t.Fatalf("expected success=true, got %v", env["success"])
	}
	data, ok := env["data"].([]any)
	if !ok {
		t.Fatalf("expected data to be an array, got %T", env["data"])
	}
	if len(data) == 0 {
		t.Fatalf("expected non-empty categories list")
	}
	first, ok := data[0].(map[string]any)
	if !ok {
		t.Fatalf("expected first item to be object, got %T", data[0])
	}
	if _, ok := first["name"]; !ok {
		t.Fatalf("expected 'name' field in category, got %+v", first)
	}
	if _, ok := first["protocols"]; !ok {
		t.Fatalf("expected 'protocols' field in category, got %+v", first)
	}
	if _, ok := first["tvl_usd"]; !ok {
		t.Fatalf("expected 'tvl_usd' field in category, got %+v", first)
	}
}

func TestRunnerChainsAssets(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		marketProvider: fakeMarketProvider{
			chainAssets: []model.ChainAssetTVL{
				{
					Rank:    1,
					Chain:   "Ethereum",
					ChainID: "eip155:1",
					Asset:   "USDC",
					AssetID: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
					TVLUSD:  12345.67,
				},
			},
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newChainsCommand())
	root.SetArgs([]string{"chains", "assets", "--chain", "1", "--asset", "USDC"})
	if err := root.Execute(); err != nil {
		t.Fatalf("expected chains assets command success, err=%v stderr=%s", err, stderr.String())
	}

	var env map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &env); err != nil {
		t.Fatalf("failed to parse output json: %v output=%s", err, stdout.String())
	}
	if env["success"] != true {
		t.Fatalf("expected success=true, got %v", env["success"])
	}
	data, ok := env["data"].([]any)
	if !ok {
		t.Fatalf("expected data to be an array, got %T", env["data"])
	}
	if len(data) != 1 {
		t.Fatalf("expected one chain asset item, got %d", len(data))
	}
	first, ok := data[0].(map[string]any)
	if !ok {
		t.Fatalf("expected first item to be object, got %T", data[0])
	}
	if _, ok := first["asset"]; !ok {
		t.Fatalf("expected 'asset' field in output, got %+v", first)
	}
	if _, ok := first["asset_id"]; !ok {
		t.Fatalf("expected 'asset_id' field in output, got %+v", first)
	}
	if _, ok := first["tvl_usd"]; !ok {
		t.Fatalf("expected 'tvl_usd' field in output, got %+v", first)
	}
}

func TestRunnerChainsAssetsAllowsUnknownSymbolFilter(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		marketProvider: fakeMarketProvider{
			expectedAssetSymbol: "UNI",
			chainAssets: []model.ChainAssetTVL{
				{
					Rank:    1,
					Chain:   "Ethereum",
					ChainID: "eip155:1",
					Asset:   "UNI",
					AssetID: "",
					TVLUSD:  456.78,
				},
			},
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newChainsCommand())
	root.SetArgs([]string{"chains", "assets", "--chain", "1", "--asset", "UNI"})
	if err := root.Execute(); err != nil {
		t.Fatalf("expected chains assets command success for unknown symbol, err=%v stderr=%s", err, stderr.String())
	}

	var env map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &env); err != nil {
		t.Fatalf("failed to parse output json: %v output=%s", err, stdout.String())
	}
	if env["success"] != true {
		t.Fatalf("expected success=true, got %v", env["success"])
	}
}

func TestRunnerBridgeListRejectsProviderFlag(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"bridge", "list", "--provider", "unknown"})
	if code != 2 {
		t.Fatalf("expected exit 2, got %d stderr=%s", code, stderr.String())
	}
	var env map[string]any
	if err := json.Unmarshal(stderr.Bytes(), &env); err != nil {
		t.Fatalf("failed to parse error envelope: %v output=%s", err, stderr.String())
	}
	errBody, ok := env["error"].(map[string]any)
	if !ok {
		t.Fatalf("expected error body, got %+v", env["error"])
	}
	if errBody["type"] != "usage_error" {
		t.Fatalf("expected usage_error type, got %v", errBody["type"])
	}
	msg, _ := errBody["message"].(string)
	if !strings.Contains(strings.ToLower(msg), "unknown flag") {
		t.Fatalf("expected unknown flag message, got %q", msg)
	}
}

func TestRunnerBridgeDetailsRequiresBridgeFlag(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"bridge", "details"})
	if code != 2 {
		t.Fatalf("expected exit 2, got %d stderr=%s", code, stderr.String())
	}
	var env map[string]any
	if err := json.Unmarshal(stderr.Bytes(), &env); err != nil {
		t.Fatalf("failed to parse error envelope: %v output=%s", err, stderr.String())
	}
	errBody, ok := env["error"].(map[string]any)
	if !ok {
		t.Fatalf("expected error body, got %+v", env["error"])
	}
	if errBody["type"] != "usage_error" {
		t.Fatalf("expected usage_error type, got %v", errBody["type"])
	}
	msg, _ := errBody["message"].(string)
	if !strings.Contains(msg, "required flag") {
		t.Fatalf("expected required flag message, got %q", msg)
	}
}

func TestSwapDefaultsToJupiterForSolana(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	oneinch := &fakeSwapProvider{name: "1inch"}
	jupiter := &fakeSwapProvider{name: "jupiter"}
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		swapProviders: map[string]providers.SwapProvider{
			"1inch":   oneinch,
			"jupiter": jupiter,
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newSwapCommand())
	root.SetArgs([]string{
		"swap", "quote",
		"--chain", "solana",
		"--from-asset", "USDC",
		"--to-asset", "USDT",
		"--amount", "1000000",
	})
	if err := root.Execute(); err != nil {
		t.Fatalf("swap command failed: %v stderr=%s", err, stderr.String())
	}
	if jupiter.calls != 1 {
		t.Fatalf("expected jupiter provider call, got %d", jupiter.calls)
	}
	if oneinch.calls != 0 {
		t.Fatalf("expected no 1inch calls, got %d", oneinch.calls)
	}
}

func TestSwapDefaultsToOneInchForEVM(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	oneinch := &fakeSwapProvider{name: "1inch"}
	jupiter := &fakeSwapProvider{name: "jupiter"}
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		swapProviders: map[string]providers.SwapProvider{
			"1inch":   oneinch,
			"jupiter": jupiter,
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newSwapCommand())
	root.SetArgs([]string{
		"swap", "quote",
		"--chain", "base",
		"--from-asset", "USDC",
		"--to-asset", "DAI",
		"--amount", "1000000",
	})
	if err := root.Execute(); err != nil {
		t.Fatalf("swap command failed: %v stderr=%s", err, stderr.String())
	}
	if oneinch.calls != 1 {
		t.Fatalf("expected 1inch provider call, got %d", oneinch.calls)
	}
	if jupiter.calls != 0 {
		t.Fatalf("expected no jupiter calls, got %d", jupiter.calls)
	}
}

func TestSwapSlippageOverridePassedToProvider(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	uniswap := &fakeSwapProvider{name: "uniswap"}
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		swapProviders: map[string]providers.SwapProvider{
			"uniswap": uniswap,
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newSwapCommand())
	root.SetArgs([]string{
		"swap", "quote",
		"--provider", "uniswap",
		"--chain", "1",
		"--from-asset", "USDC",
		"--to-asset", "DAI",
		"--amount", "1000000",
		"--slippage-pct", "1.25",
	})
	if err := root.Execute(); err != nil {
		t.Fatalf("swap command failed: %v stderr=%s", err, stderr.String())
	}

	if uniswap.lastReq.SlippagePct == nil {
		t.Fatal("expected slippage override to be passed to provider")
	}
	if *uniswap.lastReq.SlippagePct != 1.25 {
		t.Fatalf("expected slippage=1.25, got %v", *uniswap.lastReq.SlippagePct)
	}
}

func TestSwapSlippageOverrideValidation(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	uniswap := &fakeSwapProvider{name: "uniswap"}
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		swapProviders: map[string]providers.SwapProvider{
			"uniswap": uniswap,
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newSwapCommand())
	root.SetArgs([]string{
		"swap", "quote",
		"--provider", "uniswap",
		"--chain", "1",
		"--from-asset", "USDC",
		"--to-asset", "DAI",
		"--amount", "1000000",
		"--slippage-pct", "0",
	})
	if err := root.Execute(); err == nil {
		t.Fatalf("expected validation error, stderr=%s", stderr.String())
	}
	if uniswap.calls != 0 {
		t.Fatalf("expected provider not to be called on invalid slippage, got %d calls", uniswap.calls)
	}
}

func TestSwapExactOutputPassedToProvider(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	uniswap := &fakeSwapProvider{name: "uniswap"}
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		swapProviders: map[string]providers.SwapProvider{
			"uniswap": uniswap,
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newSwapCommand())
	root.SetArgs([]string{
		"swap", "quote",
		"--provider", "uniswap",
		"--chain", "1",
		"--from-asset", "USDC",
		"--to-asset", "DAI",
		"--type", "exact-output",
		"--amount-out", "1000000000000000000",
	})
	if err := root.Execute(); err != nil {
		t.Fatalf("swap command failed: %v stderr=%s", err, stderr.String())
	}

	if uniswap.lastReq.TradeType != providers.SwapTradeTypeExactOutput {
		t.Fatalf("expected trade type exact-output, got %s", uniswap.lastReq.TradeType)
	}
	if uniswap.lastReq.AmountBaseUnits != "1000000000000000000" {
		t.Fatalf("unexpected amount base units: %s", uniswap.lastReq.AmountBaseUnits)
	}
	if uniswap.lastReq.AmountDecimal != "1" {
		t.Fatalf("unexpected amount decimal: %s", uniswap.lastReq.AmountDecimal)
	}
}

func TestSwapExactOutputDefaultsToUniswapOnEVM(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	oneinch := &fakeSwapProvider{name: "1inch"}
	uniswap := &fakeSwapProvider{name: "uniswap"}
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		swapProviders: map[string]providers.SwapProvider{
			"1inch":   oneinch,
			"uniswap": uniswap,
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newSwapCommand())
	root.SetArgs([]string{
		"swap", "quote",
		"--chain", "base",
		"--from-asset", "USDC",
		"--to-asset", "DAI",
		"--type", "exact-output",
		"--amount-out", "1000000000000000000",
	})
	if err := root.Execute(); err != nil {
		t.Fatalf("swap command failed: %v stderr=%s", err, stderr.String())
	}

	if oneinch.calls != 0 {
		t.Fatalf("expected 1inch not to be called, got %d calls", oneinch.calls)
	}
	if uniswap.calls != 1 {
		t.Fatalf("expected uniswap to be called once, got %d calls", uniswap.calls)
	}
	if uniswap.lastReq.TradeType != providers.SwapTradeTypeExactOutput {
		t.Fatalf("expected trade type exact-output, got %s", uniswap.lastReq.TradeType)
	}
}

func TestSwapExactOutputWithoutProviderRejectedOnSolana(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	jupiter := &fakeSwapProvider{name: "jupiter"}
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		swapProviders: map[string]providers.SwapProvider{
			"jupiter": jupiter,
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newSwapCommand())
	root.SetArgs([]string{
		"swap", "quote",
		"--chain", "solana",
		"--from-asset", "USDC",
		"--to-asset", "SOL",
		"--type", "exact-output",
		"--amount-out", "1000000",
	})
	if err := root.Execute(); err == nil {
		t.Fatalf("expected unsupported error, stderr=%s", stderr.String())
	}
	if jupiter.calls != 0 {
		t.Fatalf("expected jupiter not to be called, got %d calls", jupiter.calls)
	}
}

func TestSwapExactOutputRequiresOutputAmount(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	uniswap := &fakeSwapProvider{name: "uniswap"}
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		swapProviders: map[string]providers.SwapProvider{
			"uniswap": uniswap,
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newSwapCommand())
	root.SetArgs([]string{
		"swap", "quote",
		"--provider", "uniswap",
		"--chain", "1",
		"--from-asset", "USDC",
		"--to-asset", "DAI",
		"--type", "exact-output",
	})
	if err := root.Execute(); err == nil {
		t.Fatalf("expected validation error, stderr=%s", stderr.String())
	}
	if uniswap.calls != 0 {
		t.Fatalf("expected provider not to be called on invalid amount flags, got %d calls", uniswap.calls)
	}
}

func TestSwapTypeValidation(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	uniswap := &fakeSwapProvider{name: "uniswap"}
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		swapProviders: map[string]providers.SwapProvider{
			"uniswap": uniswap,
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newSwapCommand())
	root.SetArgs([]string{
		"swap", "quote",
		"--provider", "uniswap",
		"--chain", "1",
		"--from-asset", "USDC",
		"--to-asset", "DAI",
		"--type", "limit-order",
		"--amount", "1000000",
	})
	if err := root.Execute(); err == nil {
		t.Fatalf("expected validation error, stderr=%s", stderr.String())
	}
	if uniswap.calls != 0 {
		t.Fatalf("expected provider not to be called on invalid type, got %d calls", uniswap.calls)
	}
}

func TestSwapSlippageOverrideRejectedForNonUniswap(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	oneinch := &fakeSwapProvider{name: "1inch"}
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		swapProviders: map[string]providers.SwapProvider{
			"1inch": oneinch,
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newSwapCommand())
	root.SetArgs([]string{
		"swap", "quote",
		"--provider", "1inch",
		"--chain", "1",
		"--from-asset", "USDC",
		"--to-asset", "DAI",
		"--amount", "1000000",
		"--slippage-pct", "1.0",
	})
	if err := root.Execute(); err == nil {
		t.Fatalf("expected validation error, stderr=%s", stderr.String())
	}
	if oneinch.calls != 0 {
		t.Fatalf("expected provider not to be called with unsupported slippage override, got %d calls", oneinch.calls)
	}
}

type fakeMarketProvider struct {
	categories          []model.ProtocolCategory
	chainAssets         []model.ChainAssetTVL
	expectedAssetSymbol string
}

func (f fakeMarketProvider) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:         "fake-market",
		Type:         "market",
		RequiresKey:  false,
		Capabilities: []string{"protocols.categories"},
	}
}

func (f fakeMarketProvider) ChainsTop(context.Context, int) ([]model.ChainTVL, error) {
	return nil, nil
}

func (f fakeMarketProvider) ChainsAssets(ctx context.Context, chain id.Chain, asset id.Asset, limit int) ([]model.ChainAssetTVL, error) {
	_ = ctx
	_ = chain
	_ = limit
	if strings.TrimSpace(f.expectedAssetSymbol) != "" && !strings.EqualFold(asset.Symbol, f.expectedAssetSymbol) {
		return nil, fmt.Errorf("unexpected asset symbol: %s", asset.Symbol)
	}
	return f.chainAssets, nil
}

func (f fakeMarketProvider) ProtocolsTop(context.Context, string, int) ([]model.ProtocolTVL, error) {
	return nil, nil
}

func (f fakeMarketProvider) ProtocolsCategories(context.Context) ([]model.ProtocolCategory, error) {
	return f.categories, nil
}

type fakeSwapProvider struct {
	name    string
	calls   int
	lastReq providers.SwapQuoteRequest
}

func (f *fakeSwapProvider) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:         f.name,
		Type:         "swap",
		RequiresKey:  false,
		Capabilities: []string{"swap.quote"},
	}
}

func (f *fakeSwapProvider) QuoteSwap(_ context.Context, req providers.SwapQuoteRequest) (model.SwapQuote, error) {
	f.calls++
	f.lastReq = req
	tradeType := req.TradeType
	if tradeType == "" {
		tradeType = providers.SwapTradeTypeExactInput
	}
	return model.SwapQuote{
		Provider:    f.name,
		ChainID:     req.Chain.CAIP2,
		FromAssetID: req.FromAsset.AssetID,
		ToAssetID:   req.ToAsset.AssetID,
		TradeType:   string(tradeType),
		InputAmount: model.AmountInfo{
			AmountBaseUnits: req.AmountBaseUnits,
			AmountDecimal:   req.AmountDecimal,
			Decimals:        req.FromAsset.Decimals,
		},
		EstimatedOut: model.AmountInfo{
			AmountBaseUnits: req.AmountBaseUnits,
			AmountDecimal:   req.AmountDecimal,
			Decimals:        req.ToAsset.Decimals,
		},
		Route: "test",
	}, nil
}

func setUnopenableCacheEnv(t *testing.T) {
	t.Helper()
	t.Setenv("DEFI_CACHE_PATH", "/dev/null/cache.db")
	t.Setenv("DEFI_CACHE_LOCK_PATH", "/dev/null/cache.lock")
}
