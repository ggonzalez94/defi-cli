package app

import (
	"testing"

	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

func TestNormalizeLendingProvider(t *testing.T) {
	if got := normalizeLendingProvider("AAVE-V3"); got != "aave" {
		t.Fatalf("expected aave, got %s", got)
	}
	if got := normalizeLendingProvider("morpho-blue"); got != "morpho" {
		t.Fatalf("expected morpho, got %s", got)
	}
	if got := normalizeLendingProvider("kamino-finance"); got != "kamino" {
		t.Fatalf("expected kamino, got %s", got)
	}
}

func TestParseLendPositionType(t *testing.T) {
	tests := []struct {
		name    string
		input   string
		want    providers.LendPositionType
		wantErr bool
	}{
		{name: "default", input: "", want: providers.LendPositionTypeAll},
		{name: "all", input: "all", want: providers.LendPositionTypeAll},
		{name: "supply", input: "supply", want: providers.LendPositionTypeSupply},
		{name: "borrow", input: "borrow", want: providers.LendPositionTypeBorrow},
		{name: "collateral", input: "collateral", want: providers.LendPositionTypeCollateral},
		{name: "invalid", input: "debt", wantErr: true},
	}

	for _, tc := range tests {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			got, err := parseLendPositionType(tc.input)
			if tc.wantErr {
				if err == nil {
					t.Fatalf("expected error for input %q", tc.input)
				}
				return
			}
			if err != nil {
				t.Fatalf("parseLendPositionType failed: %v", err)
			}
			if got != tc.want {
				t.Fatalf("expected %q, got %q", tc.want, got)
			}
		})
	}
}

func TestSelectYieldProviders(t *testing.T) {
	s := &runtimeState{yieldProviders: map[string]providers.YieldProvider{}}
	// Use nil implementations via map key presence for selection behavior.
	s.yieldProviders["aave"] = nil
	s.yieldProviders["morpho"] = nil
	chain, err := id.ParseChain("base")
	if err != nil {
		t.Fatalf("parse chain: %v", err)
	}

	items, err := s.selectYieldProviders([]string{"aave"}, chain)
	if err != nil {
		t.Fatalf("selectYieldProviders failed: %v", err)
	}
	if len(items) != 1 || items[0] != "aave" {
		t.Fatalf("unexpected items: %#v", items)
	}

	if _, err := s.selectYieldProviders([]string{"unknown"}, chain); err == nil {
		t.Fatal("expected unsupported provider error")
	}
}
