package app

import (
	"strings"
	"testing"

	"github.com/spf13/cobra"
)

func TestNormalizeAndValidateCommandFlagsHandlesStringArray(t *testing.T) {
	var assets []string
	cmd := &cobra.Command{Use: "test"}
	cmd.Flags().StringArrayVar(&assets, "assets", nil, "Asset filters")

	if err := cmd.Flags().Set("assets", "USDC"); err != nil {
		t.Fatalf("set assets: %v", err)
	}
	if err := normalizeAndValidateCommandFlags(cmd); err != nil {
		t.Fatalf("expected stringArray validation to succeed, got %v", err)
	}
}

func TestNormalizeAndValidateCommandFlagsRejectsControlCharsInStringArray(t *testing.T) {
	var assets []string
	cmd := &cobra.Command{Use: "test"}
	cmd.Flags().StringArrayVar(&assets, "assets", nil, "Asset filters")

	if err := cmd.Flags().Set("assets", "USDC\n"); err != nil {
		t.Fatalf("set assets: %v", err)
	}
	err := normalizeAndValidateCommandFlags(cmd)
	if err == nil {
		t.Fatal("expected stringArray validation to fail")
	}
	if !strings.Contains(err.Error(), "unsupported control characters") {
		t.Fatalf("unexpected error: %v", err)
	}
}
