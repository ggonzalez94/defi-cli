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

func setUnopenableCacheEnv(t *testing.T) {
	t.Helper()
	t.Setenv("DEFI_CACHE_PATH", "/dev/null/cache.db")
	t.Setenv("DEFI_CACHE_LOCK_PATH", "/dev/null/cache.lock")
}
