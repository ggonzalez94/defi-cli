package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestLoadPrecedenceFlagsOverEnvOverFile(t *testing.T) {
	tmp := t.TempDir()
	configPath := filepath.Join(tmp, "config.yaml")
	if err := os.WriteFile(configPath, []byte("output: plain\nretries: 1\n"), 0o644); err != nil {
		t.Fatalf("write config: %v", err)
	}

	t.Setenv("DEFI_OUTPUT", "json")
	flags := GlobalFlags{ConfigPath: configPath, Plain: true, Retries: 5}
	settings, err := Load(flags)
	if err != nil {
		t.Fatalf("Load failed: %v", err)
	}
	if settings.OutputMode != "plain" {
		t.Fatalf("expected flag to win, got output=%s", settings.OutputMode)
	}
	if settings.Retries != 5 {
		t.Fatalf("expected retries from flags, got %d", settings.Retries)
	}
}

func TestLoadMutuallyExclusiveOutputFlags(t *testing.T) {
	_, err := Load(GlobalFlags{JSON: true, Plain: true})
	if err == nil {
		t.Fatal("expected error with --json and --plain")
	}
}

func TestLoadAllowsZeroMaxStale(t *testing.T) {
	settings, err := Load(GlobalFlags{MaxStale: "0s"})
	if err != nil {
		t.Fatalf("Load failed: %v", err)
	}
	if settings.MaxStale != 0 {
		t.Fatalf("expected max stale 0s, got %s", settings.MaxStale)
	}
}

func TestLoadDefiLlamaAPIKeyFromEnv(t *testing.T) {
	t.Setenv("DEFI_DEFILLAMA_API_KEY", "key-123")
	settings, err := Load(GlobalFlags{})
	if err != nil {
		t.Fatalf("Load failed: %v", err)
	}
	if settings.DefiLlamaAPIKey != "key-123" {
		t.Fatalf("expected DefiLlama API key from env, got %q", settings.DefiLlamaAPIKey)
	}
}

func TestLoadExecutionPathsFromEnv(t *testing.T) {
	t.Setenv("DEFI_ACTIONS_PATH", "/tmp/defi-actions.db")
	t.Setenv("DEFI_ACTIONS_LOCK_PATH", "/tmp/defi-actions.lock")
	settings, err := Load(GlobalFlags{})
	if err != nil {
		t.Fatalf("Load failed: %v", err)
	}
	if settings.ActionStorePath != "/tmp/defi-actions.db" {
		t.Fatalf("expected action store path from env, got %q", settings.ActionStorePath)
	}
	if settings.ActionLockPath != "/tmp/defi-actions.lock" {
		t.Fatalf("expected action lock path from env, got %q", settings.ActionLockPath)
	}
}

func TestLoadTaikoRPCFromEnv(t *testing.T) {
	t.Setenv("DEFI_TAIKO_MAINNET_RPC_URL", "https://rpc.example.mainnet")
	t.Setenv("DEFI_TAIKO_HOODI_RPC_URL", "https://rpc.example.hoodi")
	settings, err := Load(GlobalFlags{})
	if err != nil {
		t.Fatalf("Load failed: %v", err)
	}
	if settings.TaikoMainnetRPC != "https://rpc.example.mainnet" {
		t.Fatalf("unexpected mainnet rpc: %q", settings.TaikoMainnetRPC)
	}
	if settings.TaikoHoodiRPC != "https://rpc.example.hoodi" {
		t.Fatalf("unexpected hoodi rpc: %q", settings.TaikoHoodiRPC)
	}
}
