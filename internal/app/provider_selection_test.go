package app

import (
	"testing"

	"github.com/ggonzalez94/defi-cli/internal/providers"
)

func TestNormalizeLendingProtocol(t *testing.T) {
	if got := normalizeLendingProtocol("AAVE-V3"); got != "aave" {
		t.Fatalf("expected aave, got %s", got)
	}
	if got := normalizeLendingProtocol("morpho-blue"); got != "morpho" {
		t.Fatalf("expected morpho, got %s", got)
	}
}

func TestSelectYieldProviders(t *testing.T) {
	s := &runtimeState{yieldProviders: map[string]providers.YieldProvider{}}
	// Use nil implementations via map key presence for selection behavior.
	s.yieldProviders["defillama"] = nil
	s.yieldProviders["aave"] = nil

	items, err := s.selectYieldProviders([]string{"aave"})
	if err != nil {
		t.Fatalf("selectYieldProviders failed: %v", err)
	}
	if len(items) != 1 || items[0] != "aave" {
		t.Fatalf("unexpected items: %#v", items)
	}

	if _, err := s.selectYieldProviders([]string{"unknown"}); err == nil {
		t.Fatal("expected unsupported provider error")
	}
}
