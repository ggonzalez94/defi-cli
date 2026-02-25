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
