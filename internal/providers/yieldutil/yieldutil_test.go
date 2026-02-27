package yieldutil

import (
	"math"
	"testing"

	"github.com/ggonzalez94/defi-cli/internal/model"
)

func TestPositiveFirst(t *testing.T) {
	got := PositiveFirst(math.NaN(), -1, 0, 4, 5)
	if got != 4 {
		t.Fatalf("expected first positive finite value, got %v", got)
	}
}

func TestSort(t *testing.T) {
	items := []model.YieldOpportunity{
		{OpportunityID: "b", APYTotal: 8, TVLUSD: 100, LiquidityUSD: 40},
		{OpportunityID: "a", APYTotal: 8, TVLUSD: 100, LiquidityUSD: 30},
		{OpportunityID: "c", APYTotal: 4, TVLUSD: 90, LiquidityUSD: 20},
	}
	Sort(items, "apy_total")
	if items[0].OpportunityID != "b" || items[1].OpportunityID != "a" || items[2].OpportunityID != "c" {
		t.Fatalf("unexpected sort order: %#v", items)
	}
}
