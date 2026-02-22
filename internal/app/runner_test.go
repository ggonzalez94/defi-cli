package app

import (
	"bytes"
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/config"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/version"
	"github.com/spf13/cobra"
)

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
	categories []model.ProtocolCategory
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
