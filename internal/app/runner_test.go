package app

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/crypto"
	"github.com/ggonzalez94/defi-cli/internal/config"
	"github.com/ggonzalez94/defi-cli/internal/execution"
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

func TestParseYieldHistoryMetricsDedupesAndValidates(t *testing.T) {
	metrics, err := parseYieldHistoryMetrics("apy_total,tvl_usd,apy_total")
	if err != nil {
		t.Fatalf("parseYieldHistoryMetrics failed: %v", err)
	}
	if len(metrics) != 2 {
		t.Fatalf("expected 2 metrics, got %+v", metrics)
	}
	if metrics[0] != providers.YieldHistoryMetricAPYTotal || metrics[1] != providers.YieldHistoryMetricTVLUSD {
		t.Fatalf("unexpected metric order: %+v", metrics)
	}

	if _, err := parseYieldHistoryMetrics("foo"); err == nil {
		t.Fatal("expected invalid metric error")
	}
}

func TestYieldHistoryCommandCallsProvider(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	fixedNow := time.Date(2026, 2, 26, 20, 0, 0, 0, time.UTC)
	fakeProvider := &fakeYieldHistoryProvider{
		name: "aave",
		opportunities: []model.YieldOpportunity{
			{
				OpportunityID:        "opp-1",
				Provider:             "aave",
				Protocol:             "aave",
				ChainID:              "eip155:1",
				AssetID:              "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
				ProviderNativeID:     "aave:eip155:1:0x1111111111111111111111111111111111111111:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
				ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
				SourceURL:            "https://app.aave.com",
			},
		},
		series: []model.YieldHistorySeries{
			{
				OpportunityID: "opp-1",
				Provider:      "aave",
				Metric:        "apy_total",
				Interval:      "hour",
				Points: []model.YieldHistoryPoint{
					{Timestamp: "2026-02-26T19:00:00Z", Value: 3.1},
				},
			},
		},
	}
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    func() time.Time { return fixedNow },
		},
		settings: config.Settings{
			OutputMode:   "json",
			ResultsOnly:  true,
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		yieldProviders: map[string]providers.YieldProvider{
			"aave": fakeProvider,
		},
	}

	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newYieldCommand())
	root.SetArgs([]string{
		"yield", "history",
		"--chain", "1",
		"--asset", "USDC",
		"--providers", "aave",
		"--metrics", "apy_total",
		"--interval", "hour",
		"--window", "24h",
		"--limit", "1",
	})
	if err := root.Execute(); err != nil {
		t.Fatalf("yield history command failed: %v stderr=%s", err, stderr.String())
	}

	if fakeProvider.historyCalls != 1 {
		t.Fatalf("expected one history call, got %d", fakeProvider.historyCalls)
	}
	if fakeProvider.lastHistoryReq.Interval != providers.YieldHistoryIntervalHour {
		t.Fatalf("expected hour interval, got %+v", fakeProvider.lastHistoryReq.Interval)
	}
	if got := fakeProvider.lastHistoryReq.EndTime.UTC(); !got.Equal(fixedNow) {
		t.Fatalf("expected end time %s, got %s", fixedNow, got)
	}
	if got := fakeProvider.lastHistoryReq.StartTime.UTC(); !got.Equal(fixedNow.Add(-24 * time.Hour)) {
		t.Fatalf("expected start time %s, got %s", fixedNow.Add(-24*time.Hour), got)
	}

	var out []map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &out); err != nil {
		t.Fatalf("failed parsing output json: %v output=%s", err, stdout.String())
	}
	if len(out) != 1 {
		t.Fatalf("expected one series row, got %+v", out)
	}
	if out[0]["metric"] != "apy_total" {
		t.Fatalf("expected metric apy_total, got %+v", out[0])
	}
}

func TestYieldHistoryCommandFailsWhenProviderHasNoHistorySupport(t *testing.T) {
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
		yieldProviders: map[string]providers.YieldProvider{
			"aave": &fakeYieldProviderNoHistory{name: "aave"},
		},
	}

	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newYieldCommand())
	root.SetArgs([]string{
		"yield", "history",
		"--chain", "1",
		"--asset", "USDC",
		"--providers", "aave",
	})
	if err := root.Execute(); err == nil {
		t.Fatalf("expected yield history to fail without history provider support; stderr=%s", stderr.String())
	}
}

func TestYieldPositionsCommandCallsProvider(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	fakeProvider := &fakeYieldHistoryProvider{
		name: "morpho",
		positions: []model.YieldPosition{
			{
				Protocol:             "morpho",
				Provider:             "morpho",
				ChainID:              "eip155:1",
				AccountAddress:       "0x000000000000000000000000000000000000dEaD",
				PositionType:         "deposit",
				OpportunityID:        "opp-1",
				AssetID:              "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
				ProviderNativeID:     "0x1111111111111111111111111111111111111111",
				ProviderNativeIDKind: model.NativeIDKindVaultAddress,
				Amount: model.AmountInfo{
					AmountBaseUnits: "1000000",
					AmountDecimal:   "1",
					Decimals:        6,
				},
				AmountUSD: 1,
				APYTotal:  4.2,
				SourceURL: "https://app.morpho.org",
				FetchedAt: "2026-02-26T20:00:00Z",
			},
		},
	}
	state := &runtimeState{
		runner: &Runner{
			stdout: &stdout,
			stderr: &stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			ResultsOnly:  true,
			Timeout:      2 * time.Second,
			CacheEnabled: false,
		},
		yieldProviders: map[string]providers.YieldProvider{
			"morpho": fakeProvider,
		},
	}

	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newYieldCommand())
	root.SetArgs([]string{
		"yield", "positions",
		"--chain", "1",
		"--address", "0x000000000000000000000000000000000000dEaD",
		"--providers", "morpho",
		"--limit", "5",
	})
	if err := root.Execute(); err != nil {
		t.Fatalf("yield positions command failed: %v stderr=%s", err, stderr.String())
	}

	if fakeProvider.positionCalls != 1 {
		t.Fatalf("expected one positions call, got %d", fakeProvider.positionCalls)
	}
	if fakeProvider.lastPositionReq.Chain.CAIP2 != "eip155:1" {
		t.Fatalf("unexpected chain in request: %+v", fakeProvider.lastPositionReq)
	}
	if fakeProvider.lastPositionReq.Account != "0x000000000000000000000000000000000000dEaD" {
		t.Fatalf("unexpected account in request: %+v", fakeProvider.lastPositionReq)
	}

	var out []map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &out); err != nil {
		t.Fatalf("failed parsing output json: %v output=%s", err, stdout.String())
	}
	if len(out) != 1 {
		t.Fatalf("expected one yield position row, got %+v", out)
	}
	if out[0]["provider"] != "morpho" {
		t.Fatalf("expected morpho provider row, got %+v", out[0])
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
	tempoInfo, ok := findProviderInfo(out, "tempo")
	if !ok {
		t.Fatalf("expected tempo provider in providers list, got %#v", out)
	}
	if requiresKey, ok := tempoInfo["requires_key"].(bool); !ok || requiresKey {
		t.Fatalf("expected tempo requires_key=false, got %#v", tempoInfo["requires_key"])
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

func TestRunnerChainsList(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"chains", "list", "--results-only"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}
	var out []map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &out); err != nil {
		t.Fatalf("failed to parse output json: %v output=%s", err, stdout.String())
	}
	if len(out) == 0 {
		t.Fatal("expected at least one chain in output")
	}

	// Verify each entry has required fields.
	for _, item := range out {
		if _, ok := item["name"].(string); !ok {
			t.Fatalf("missing name field: %+v", item)
		}
		if _, ok := item["slug"].(string); !ok {
			t.Fatalf("missing slug field: %+v", item)
		}
		if _, ok := item["caip2"].(string); !ok {
			t.Fatalf("missing caip2 field: %+v", item)
		}
		if _, ok := item["namespace"].(string); !ok {
			t.Fatalf("missing namespace field: %+v", item)
		}
	}

	// Verify Ethereum is present.
	var ethFound bool
	for _, item := range out {
		if item["slug"] == "ethereum" {
			ethFound = true
			if item["caip2"] != "eip155:1" {
				t.Fatalf("expected ethereum caip2 eip155:1, got %v", item["caip2"])
			}
			if item["namespace"] != "eip155" {
				t.Fatalf("expected eip155 namespace, got %v", item["namespace"])
			}
		}
	}
	if !ethFound {
		t.Fatal("expected ethereum in chains list output")
	}
}

func TestRunnerChainsListBypassesCache(t *testing.T) {
	if shouldOpenCache("chains list") {
		t.Fatal("chains list should bypass cache initialization")
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
	tempoInfo, ok := findProviderInfo(out, "tempo")
	if !ok {
		t.Fatalf("expected tempo provider in providers list, got %#v", out)
	}
	if requiresKey, ok := tempoInfo["requires_key"].(bool); !ok || requiresKey {
		t.Fatalf("expected tempo requires_key=false, got %#v", tempoInfo["requires_key"])
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

func TestRunnerProtocolsFees(t *testing.T) {
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
			protocolFees: []model.ProtocolFees{
				{Rank: 1, Protocol: "Lido", Category: "Liquid Staking", Fees24hUSD: 8000000, Fees7dUSD: 55000000, Fees30dUSD: 200000000, Chains: 1},
			},
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newProtocolsCommand())
	root.SetArgs([]string{"protocols", "fees"})
	if err := root.Execute(); err != nil {
		t.Fatalf("expected protocols fees command success, err=%v stderr=%s", err, stderr.String())
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
		t.Fatalf("expected non-empty fees list")
	}
	first, ok := data[0].(map[string]any)
	if !ok {
		t.Fatalf("expected first item to be object, got %T", data[0])
	}
	if _, ok := first["protocol"]; !ok {
		t.Fatalf("expected 'protocol' field, got %+v", first)
	}
	if _, ok := first["fees_24h_usd"]; !ok {
		t.Fatalf("expected 'fees_24h_usd' field, got %+v", first)
	}
}

func TestRunnerProtocolsRevenue(t *testing.T) {
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
			protocolRevenue: []model.ProtocolRevenue{
				{Rank: 1, Protocol: "Lido", Category: "Liquid Staking", Revenue24hUSD: 5000000, Revenue7dUSD: 35000000, Revenue30dUSD: 130000000, Chains: 1},
			},
		},
	}
	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newProtocolsCommand())
	root.SetArgs([]string{"protocols", "revenue"})
	if err := root.Execute(); err != nil {
		t.Fatalf("expected protocols revenue command success, err=%v stderr=%s", err, stderr.String())
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
		t.Fatalf("expected non-empty revenue list")
	}
	first, ok := data[0].(map[string]any)
	if !ok {
		t.Fatalf("expected first item to be object, got %T", data[0])
	}
	if _, ok := first["protocol"]; !ok {
		t.Fatalf("expected 'protocol' field, got %+v", first)
	}
	if _, ok := first["revenue_24h_usd"]; !ok {
		t.Fatalf("expected 'revenue_24h_usd' field, got %+v", first)
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

func TestRunnerLendPositionsCallsProvider(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	aaveProvider := &fakeLendingProvider{
		name: "aave",
		positions: []model.LendPosition{
			{
				Provider:       "aave",
				ChainID:        "eip155:1",
				AccountAddress: "0x000000000000000000000000000000000000dead",
				PositionType:   "collateral",
				AssetID:        "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
			},
		},
	}
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
		lendingProviders: map[string]providers.LendingProvider{
			"aave": aaveProvider,
		},
	}

	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newLendCommand())
	root.SetArgs([]string{
		"lend", "positions",
		"--provider", "aave",
		"--chain", "1",
		"--address", "0x000000000000000000000000000000000000dEaD",
		"--asset", "USDC",
		"--type", "collateral",
		"--limit", "5",
	})

	if err := root.Execute(); err != nil {
		t.Fatalf("lend positions command failed: %v stderr=%s", err, stderr.String())
	}
	if aaveProvider.calls != 1 {
		t.Fatalf("expected provider call once, got %d", aaveProvider.calls)
	}
	if aaveProvider.lastReq.PositionType != providers.LendPositionTypeCollateral {
		t.Fatalf("expected collateral request type, got %s", aaveProvider.lastReq.PositionType)
	}
	if !strings.EqualFold(aaveProvider.lastReq.Account, "0x000000000000000000000000000000000000dead") {
		t.Fatalf("unexpected account passed to provider: %s", aaveProvider.lastReq.Account)
	}
	if !strings.EqualFold(aaveProvider.lastReq.Asset.Symbol, "USDC") {
		t.Fatalf("expected USDC asset filter, got %+v", aaveProvider.lastReq.Asset)
	}

	var env map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &env); err != nil {
		t.Fatalf("failed to parse output json: %v output=%s", err, stdout.String())
	}
	if env["success"] != true {
		t.Fatalf("expected success=true, got %v", env["success"])
	}
}

func TestRunnerLendPositionsRejectsInvalidType(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	aaveProvider := &fakeLendingProvider{name: "aave"}
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
		lendingProviders: map[string]providers.LendingProvider{
			"aave": aaveProvider,
		},
	}

	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newLendCommand())
	root.SetArgs([]string{
		"lend", "positions",
		"--provider", "aave",
		"--chain", "1",
		"--address", "0x000000000000000000000000000000000000dEaD",
		"--type", "debt",
	})

	if err := root.Execute(); err == nil {
		t.Fatalf("expected invalid type error, stderr=%s", stderr.String())
	}
	if aaveProvider.calls != 0 {
		t.Fatalf("expected provider not to be called, got %d calls", aaveProvider.calls)
	}
}

func TestRunnerLendPositionsRejectsInvalidEVMAddress(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	aaveProvider := &fakeLendingProvider{name: "aave"}
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
		lendingProviders: map[string]providers.LendingProvider{
			"aave": aaveProvider,
		},
	}

	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newLendCommand())
	root.SetArgs([]string{
		"lend", "positions",
		"--provider", "aave",
		"--chain", "1",
		"--address", "not-an-address",
	})

	if err := root.Execute(); err == nil {
		t.Fatalf("expected invalid address error, stderr=%s", stderr.String())
	}
	if aaveProvider.calls != 0 {
		t.Fatalf("expected provider not to be called, got %d calls", aaveProvider.calls)
	}
}

func TestRunnerLendPositionsRequiresProviderCapability(t *testing.T) {
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
		lendingProviders: map[string]providers.LendingProvider{
			"kamino": &fakeLendingProviderNoPositions{name: "kamino"},
		},
	}

	root := &cobra.Command{Use: "defi"}
	root.SilenceUsage = true
	root.SilenceErrors = true
	root.SetOut(&stdout)
	root.SetErr(&stderr)
	root.AddCommand(state.newLendCommand())
	root.SetArgs([]string{
		"lend", "positions",
		"--provider", "kamino",
		"--chain", "solana",
		"--address", "6dM4QgP1VnRfx6TVV1t5hBf3ytA5Qn2ATqNnSboP8qz5",
	})

	err := root.Execute()
	if err == nil {
		t.Fatalf("expected unsupported capability error, stderr=%s", stderr.String())
	}
	if !strings.Contains(strings.ToLower(err.Error()), "does not support positions") {
		t.Fatalf("expected capability error message, got: %v", err)
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

func TestSwapQuoteWithJupiterForSolana(t *testing.T) {
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
		"--provider", "jupiter",
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

func TestSwapQuoteWithOneInchForEVM(t *testing.T) {
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
		"--provider", "1inch",
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
		"--from-address", "0x000000000000000000000000000000000000dEaD",
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
	if uniswap.lastReq.Swapper != "0x000000000000000000000000000000000000dEaD" {
		t.Fatalf("expected swapper to be forwarded, got %s", uniswap.lastReq.Swapper)
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
		"--from-address", "0x000000000000000000000000000000000000dEaD",
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
		"--from-address", "0x000000000000000000000000000000000000dEaD",
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
	if uniswap.lastReq.Swapper != "0x000000000000000000000000000000000000dEaD" {
		t.Fatalf("expected swapper to be forwarded, got %s", uniswap.lastReq.Swapper)
	}
}

func TestSwapExactOutputTempoPassedToProvider(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	tempoProvider := &fakeSwapProvider{name: "tempo"}
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
			"tempo": tempoProvider,
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
		"--provider", "tempo-dex",
		"--chain", "tempo",
		"--from-asset", "USDC.e",
		"--to-asset", "EURC.e",
		"--type", "exact-output",
		"--amount-out", "1000000",
	})
	if err := root.Execute(); err != nil {
		t.Fatalf("swap command failed: %v stderr=%s", err, stderr.String())
	}

	if tempoProvider.lastReq.TradeType != providers.SwapTradeTypeExactOutput {
		t.Fatalf("expected trade type exact-output, got %s", tempoProvider.lastReq.TradeType)
	}
	if tempoProvider.lastReq.AmountBaseUnits != "1000000" {
		t.Fatalf("unexpected amount base units: %s", tempoProvider.lastReq.AmountBaseUnits)
	}
	if tempoProvider.calls != 1 {
		t.Fatalf("expected tempo provider call, got %d", tempoProvider.calls)
	}
}

func TestSwapExactOutputRequiresExplicitProvider(t *testing.T) {
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
	if err := root.Execute(); err == nil {
		t.Fatalf("expected provider requirement error, stderr=%s", stderr.String())
	}

	if oneinch.calls != 0 {
		t.Fatalf("expected 1inch not to be called, got %d calls", oneinch.calls)
	}
	if uniswap.calls != 0 {
		t.Fatalf("expected uniswap not to be called, got %d calls", uniswap.calls)
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
		t.Fatalf("expected provider requirement error, stderr=%s", stderr.String())
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
		"--from-address", "0x000000000000000000000000000000000000dEaD",
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
		"--from-address", "0x000000000000000000000000000000000000dEaD",
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
	protocolFees        []model.ProtocolFees
	protocolRevenue     []model.ProtocolRevenue
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

func (f fakeMarketProvider) ProtocolsTop(context.Context, string, string, int) ([]model.ProtocolTVL, error) {
	return nil, nil
}

func (f fakeMarketProvider) ProtocolsCategories(context.Context) ([]model.ProtocolCategory, error) {
	return f.categories, nil
}

func (f fakeMarketProvider) StablecoinsTop(context.Context, string, int) ([]model.Stablecoin, error) {
	return nil, nil
}

func (f fakeMarketProvider) StablecoinChains(context.Context, int) ([]model.StablecoinChain, error) {
	return nil, nil
}

func (f fakeMarketProvider) ProtocolsFees(context.Context, string, string, int) ([]model.ProtocolFees, error) {
	return f.protocolFees, nil
}

func (f fakeMarketProvider) ProtocolsRevenue(context.Context, string, string, int) ([]model.ProtocolRevenue, error) {
	return f.protocolRevenue, nil
}

func (f fakeMarketProvider) DexesVolume(context.Context, string, int) ([]model.DexVolume, error) {
	return nil, nil
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

type fakeLendingProvider struct {
	name      string
	positions []model.LendPosition
	err       error
	calls     int
	lastReq   providers.LendPositionsRequest
}

func (f *fakeLendingProvider) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:         f.name,
		Type:         "lending",
		RequiresKey:  false,
		Capabilities: []string{"lend.markets", "lend.rates", "lend.positions"},
	}
}

func (f *fakeLendingProvider) LendMarkets(context.Context, string, id.Chain, id.Asset) ([]model.LendMarket, error) {
	return nil, nil
}

func (f *fakeLendingProvider) LendRates(context.Context, string, id.Chain, id.Asset) ([]model.LendRate, error) {
	return nil, nil
}

func (f *fakeLendingProvider) LendPositions(_ context.Context, req providers.LendPositionsRequest) ([]model.LendPosition, error) {
	f.calls++
	f.lastReq = req
	if f.err != nil {
		return nil, f.err
	}
	return f.positions, nil
}

type fakeLendingProviderNoPositions struct {
	name string
}

func (f *fakeLendingProviderNoPositions) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:         f.name,
		Type:         "lending",
		RequiresKey:  false,
		Capabilities: []string{"lend.markets", "lend.rates"},
	}
}

func (f *fakeLendingProviderNoPositions) LendMarkets(context.Context, string, id.Chain, id.Asset) ([]model.LendMarket, error) {
	return nil, nil
}

func (f *fakeLendingProviderNoPositions) LendRates(context.Context, string, id.Chain, id.Asset) ([]model.LendRate, error) {
	return nil, nil
}

type fakeYieldHistoryProvider struct {
	name            string
	opportunities   []model.YieldOpportunity
	positions       []model.YieldPosition
	series          []model.YieldHistorySeries
	err             error
	calls           int
	positionCalls   int
	historyCalls    int
	lastYieldReq    providers.YieldRequest
	lastPositionReq providers.YieldPositionsRequest
	lastHistoryReq  providers.YieldHistoryRequest
}

func (f *fakeYieldHistoryProvider) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:         f.name,
		Type:         "yield",
		RequiresKey:  false,
		Capabilities: []string{"yield.opportunities", "yield.positions", "yield.history"},
	}
}

func (f *fakeYieldHistoryProvider) YieldOpportunities(_ context.Context, req providers.YieldRequest) ([]model.YieldOpportunity, error) {
	f.calls++
	f.lastYieldReq = req
	if f.err != nil {
		return nil, f.err
	}
	return f.opportunities, nil
}

func (f *fakeYieldHistoryProvider) YieldHistory(_ context.Context, req providers.YieldHistoryRequest) ([]model.YieldHistorySeries, error) {
	f.historyCalls++
	f.lastHistoryReq = req
	if f.err != nil {
		return nil, f.err
	}
	return f.series, nil
}

func (f *fakeYieldHistoryProvider) YieldPositions(_ context.Context, req providers.YieldPositionsRequest) ([]model.YieldPosition, error) {
	f.positionCalls++
	f.lastPositionReq = req
	if f.err != nil {
		return nil, f.err
	}
	return f.positions, nil
}

type fakeYieldProviderNoHistory struct {
	name string
}

func (f *fakeYieldProviderNoHistory) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:         f.name,
		Type:         "yield",
		RequiresKey:  false,
		Capabilities: []string{"yield.opportunities"},
	}
}

func (f *fakeYieldProviderNoHistory) YieldOpportunities(context.Context, providers.YieldRequest) ([]model.YieldOpportunity, error) {
	return nil, nil
}

func setUnopenableCacheEnv(t *testing.T) {
	t.Helper()
	t.Setenv("DEFI_CACHE_PATH", "/dev/null/cache.db")
	t.Setenv("DEFI_CACHE_LOCK_PATH", "/dev/null/cache.lock")
}

func TestOWSSubmitRejectsLegacySignerFlags(t *testing.T) {
	actionStorePath := filepath.Join(t.TempDir(), "actions.db")
	actionLockPath := filepath.Join(t.TempDir(), "actions.lock")
	t.Setenv("DEFI_ACTIONS_PATH", actionStorePath)
	t.Setenv("DEFI_ACTIONS_LOCK_PATH", actionLockPath)

	store, err := execution.OpenStore(actionStorePath, actionLockPath)
	if err != nil {
		t.Fatalf("open action store: %v", err)
	}
	defer store.Close()

	action := execution.NewAction("act_0123456789abcdef0123456789abcdef", "transfer", "eip155:167000", execution.Constraints{Simulate: true})
	action.FromAddress = "0x00000000000000000000000000000000000000AA"
	action.WalletID = "wallet-123"
	action.ExecutionBackend = execution.ExecutionBackendOWS
	if err := store.Save(action); err != nil {
		t.Fatalf("save action: %v", err)
	}

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{
		"transfer", "submit",
		"--action-id", action.ActionID,
		"--private-key", "0x1234",
	})
	if code != 2 {
		t.Fatalf("expected usage exit 2, got %d stderr=%s", code, stderr.String())
	}
	if !strings.Contains(strings.ToLower(stderr.String()), "legacy signer") {
		t.Fatalf("expected legacy signer rejection, got stderr=%s", stderr.String())
	}
}

func TestLegacySubmitStillLoadsLocalSigner(t *testing.T) {
	actionStorePath := filepath.Join(t.TempDir(), "actions.db")
	actionLockPath := filepath.Join(t.TempDir(), "actions.lock")
	t.Setenv("DEFI_ACTIONS_PATH", actionStorePath)
	t.Setenv("DEFI_ACTIONS_LOCK_PATH", actionLockPath)

	privateKeyHex := "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
	privateKey, err := crypto.HexToECDSA(privateKeyHex)
	if err != nil {
		t.Fatalf("parse private key: %v", err)
	}
	t.Setenv("DEFI_PRIVATE_KEY", privateKeyHex)

	store, err := execution.OpenStore(actionStorePath, actionLockPath)
	if err != nil {
		t.Fatalf("open action store: %v", err)
	}
	defer store.Close()

	action := execution.NewAction("act_fedcba9876543210fedcba9876543210", "transfer", "eip155:167000", execution.Constraints{Simulate: true})
	action.FromAddress = crypto.PubkeyToAddress(privateKey.PublicKey).Hex()
	action.ExecutionBackend = execution.ExecutionBackendLegacyLocal
	if err := store.Save(action); err != nil {
		t.Fatalf("save action: %v", err)
	}

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{
		"transfer", "submit",
		"--action-id", action.ActionID,
	})
	if code != 2 {
		t.Fatalf("expected usage exit 2, got %d stderr=%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "action has no executable steps") {
		t.Fatalf("expected submit to get past signer loading, got stderr=%s", stderr.String())
	}
}

func TestLegacySubmitRejectsTempoSignerOverride(t *testing.T) {
	actionStorePath := filepath.Join(t.TempDir(), "actions.db")
	actionLockPath := filepath.Join(t.TempDir(), "actions.lock")
	t.Setenv("DEFI_ACTIONS_PATH", actionStorePath)
	t.Setenv("DEFI_ACTIONS_LOCK_PATH", actionLockPath)

	store, err := execution.OpenStore(actionStorePath, actionLockPath)
	if err != nil {
		t.Fatalf("open action store: %v", err)
	}
	defer store.Close()

	action := execution.NewAction("act_00112233445566778899aabbccddeeff", "transfer", "eip155:167000", execution.Constraints{Simulate: true})
	action.FromAddress = "0x00000000000000000000000000000000000000AA"
	action.ExecutionBackend = execution.ExecutionBackendLegacyLocal
	if err := store.Save(action); err != nil {
		t.Fatalf("save action: %v", err)
	}

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{
		"transfer", "submit",
		"--action-id", action.ActionID,
		"--signer", "tempo",
	})
	if code != 2 {
		t.Fatalf("expected usage exit 2, got %d stderr=%s", code, stderr.String())
	}
	if !strings.Contains(strings.ToLower(stderr.String()), "legacy") || !strings.Contains(strings.ToLower(stderr.String()), "local") {
		t.Fatalf("expected legacy local-only rejection, got stderr=%s", stderr.String())
	}
}
