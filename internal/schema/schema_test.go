package schema

import (
	"testing"

	"github.com/spf13/cobra"
)

func TestBuildSchema(t *testing.T) {
	root := &cobra.Command{Use: "defi"}
	root.PersistentFlags().Bool("json", false, "Output JSON")
	child := &cobra.Command{Use: "yield", Short: "yield cmds"}
	leaf := &cobra.Command{Use: "plan", Short: "create a yield action plan"}
	leaf.Flags().String("provider", "", "Yield provider (aave|morpho)")
	leaf.Flags().Int("limit", 20, "limit results")
	_ = leaf.MarkFlagRequired("provider")
	if err := SetFlagMetadata(leaf.Flags(), "provider", FlagMetadata{Format: "provider"}); err != nil {
		t.Fatalf("SetFlagMetadata failed: %v", err)
	}
	req, err := SchemaFromFlagBindings(leaf, struct {
		Provider string `json:"provider" flag:"provider" required:"true" enum:"aave,morpho" format:"provider"`
		Limit    int    `json:"limit" flag:"limit"`
	}{})
	if err != nil {
		t.Fatalf("SchemaFromFlagBindings failed: %v", err)
	}
	if err := SetCommandMetadata(leaf, CommandMetadata{
		Mutation:   true,
		InputModes: []string{"flags", "json"},
		Request:    &req,
		Response:   &TypeSchema{Type: "object"},
	}); err != nil {
		t.Fatalf("SetCommandMetadata failed: %v", err)
	}
	child.AddCommand(leaf)
	root.AddCommand(child)

	s, err := Build(root, "yield plan")
	if err != nil {
		t.Fatalf("Build failed: %v", err)
	}
	if s.Path != "defi yield plan" {
		t.Fatalf("unexpected path: %s", s.Path)
	}
	if !s.Mutation {
		t.Fatal("expected mutation metadata to be present")
	}
	if len(s.InputModes) != 2 {
		t.Fatalf("unexpected input modes: %#v", s.InputModes)
	}
	if s.Request == nil || len(s.Request.Fields) != 2 {
		t.Fatalf("expected request schema fields, got %#v", s.Request)
	}
	if len(s.Flags) != 3 {
		t.Fatalf("unexpected flags: %+v", s.Flags)
	}
	if s.Flags[0].Name != "json" || s.Flags[0].Scope != "inherited" {
		t.Fatalf("expected inherited json flag, got %+v", s.Flags[0])
	}
	if s.Flags[2].Name != "provider" || !s.Flags[2].Required {
		t.Fatalf("expected required provider flag, got %+v", s.Flags[2])
	}
	if got := s.Flags[2].Enum; len(got) != 2 || got[0] != "aave" || got[1] != "morpho" {
		t.Fatalf("unexpected provider enum: %#v", got)
	}
}
