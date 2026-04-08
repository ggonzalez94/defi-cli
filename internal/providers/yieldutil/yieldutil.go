package yieldutil

import (
	"math"
	"sort"
	"strings"

	"github.com/ggonzalez94/defi-cli/internal/model"
)

func PositiveFirst(values ...float64) float64 {
	for _, value := range values {
		if value > 0 && !math.IsNaN(value) && !math.IsInf(value, 0) {
			return value
		}
	}
	return 0
}

func Sort(items []model.YieldOpportunity, sortBy string) {
	sortBy = normalizeSortBy(sortBy)
	sort.Slice(items, func(i, j int) bool {
		return Compare(items[i], items[j], sortBy)
	})
}

// Compare reports whether a should sort before b for the given sortBy key.
func Compare(a, b model.YieldOpportunity, sortBy string) bool {
	switch normalizeSortBy(sortBy) {
	case "tvl_usd":
		if a.TVLUSD != b.TVLUSD {
			return a.TVLUSD > b.TVLUSD
		}
	case "liquidity_usd":
		if a.LiquidityUSD != b.LiquidityUSD {
			return a.LiquidityUSD > b.LiquidityUSD
		}
	default:
		if a.APYTotal != b.APYTotal {
			return a.APYTotal > b.APYTotal
		}
	}
	if a.APYTotal != b.APYTotal {
		return a.APYTotal > b.APYTotal
	}
	if a.TVLUSD != b.TVLUSD {
		return a.TVLUSD > b.TVLUSD
	}
	if a.LiquidityUSD != b.LiquidityUSD {
		return a.LiquidityUSD > b.LiquidityUSD
	}
	return a.OpportunityID < b.OpportunityID
}

func normalizeSortBy(sortBy string) string {
	s := strings.ToLower(strings.TrimSpace(sortBy))
	if s == "" {
		return "apy_total"
	}
	return s
}
