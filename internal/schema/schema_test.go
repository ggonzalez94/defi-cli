package schema

import (
	"testing"

	"github.com/spf13/cobra"
)

func TestBuildSchema(t *testing.T) {
	root := &cobra.Command{Use: "defi"}
	child := &cobra.Command{Use: "yield", Short: "yield cmds"}
	leaf := &cobra.Command{Use: "opportunities", Short: "rank opportunities"}
	leaf.Flags().Int("limit", 20, "limit results")
	child.AddCommand(leaf)
	root.AddCommand(child)

	s, err := Build(root, "yield opportunities")
	if err != nil {
		t.Fatalf("Build failed: %v", err)
	}
	if s.Path != "defi yield opportunities" {
		t.Fatalf("unexpected path: %s", s.Path)
	}
	if len(s.Flags) != 1 || s.Flags[0].Name != "limit" {
		t.Fatalf("unexpected flags: %+v", s.Flags)
	}
}
