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
	sortBy = strings.ToLower(strings.TrimSpace(sortBy))
	if sortBy == "" {
		sortBy = "apy_total"
	}

	sort.Slice(items, func(i, j int) bool {
		a, b := items[i], items[j]
		switch sortBy {
		case "apy_total":
			if a.APYTotal != b.APYTotal {
				return a.APYTotal > b.APYTotal
			}
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
		return strings.Compare(a.OpportunityID, b.OpportunityID) < 0
	})
}
