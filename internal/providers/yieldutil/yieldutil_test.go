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

func TestRiskOrder(t *testing.T) {
	if RiskOrder("low") != 1 || RiskOrder("medium") != 2 || RiskOrder("high") != 3 || RiskOrder("unknown") != 4 {
		t.Fatalf("unexpected risk order mapping")
	}
	if RiskOrder("n/a") != 0 {
		t.Fatalf("expected unknown mapping to be 0")
	}
}

func TestScoreOpportunity(t *testing.T) {
	score := ScoreOpportunity(10, 1_000_000, 250_000, "medium")
	if score != 20 {
		t.Fatalf("unexpected score: %v", score)
	}
}

func TestSort(t *testing.T) {
	items := []model.YieldOpportunity{
		{OpportunityID: "b", Score: 10, APYTotal: 8, TVLUSD: 100, LiquidityUSD: 40},
		{OpportunityID: "a", Score: 10, APYTotal: 8, TVLUSD: 100, LiquidityUSD: 30},
		{OpportunityID: "c", Score: 20, APYTotal: 4, TVLUSD: 90, LiquidityUSD: 20},
	}
	Sort(items, "score")
	if items[0].OpportunityID != "c" || items[1].OpportunityID != "a" || items[2].OpportunityID != "b" {
		t.Fatalf("unexpected sort order: %#v", items)
	}
}
