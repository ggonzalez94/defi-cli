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
