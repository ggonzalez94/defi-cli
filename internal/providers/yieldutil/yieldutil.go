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

func RiskOrder(v string) int {
	switch strings.ToLower(strings.TrimSpace(v)) {
	case "low":
		return 1
	case "medium":
		return 2
	case "high":
		return 3
	case "unknown":
		return 4
	default:
		return 0
	}
}

func ScoreOpportunity(apyTotal, tvlUSD, liquidityUSD float64, riskLevel string) float64 {
	apyNorm := clamp(apyTotal, 0, 100) / 100
	tvlNorm := clamp(math.Log10(tvlUSD+1)/10, 0, 1)
	liqNorm := 0.0
	if tvlUSD > 0 {
		liqNorm = clamp(liquidityUSD/math.Max(tvlUSD, 1), 0, 1)
	}

	riskPenalty := map[string]float64{
		"low":     0.10,
		"medium":  0.30,
		"high":    0.60,
		"unknown": 0.45,
	}[strings.ToLower(strings.TrimSpace(riskLevel))]

	scoreRaw := 0.45*apyNorm + 0.30*tvlNorm + 0.20*liqNorm - 0.25*riskPenalty
	return math.Round(clamp(scoreRaw, 0, 1)*100*100) / 100
}

func Sort(items []model.YieldOpportunity, sortBy string) {
	sortBy = strings.ToLower(strings.TrimSpace(sortBy))
	if sortBy == "" {
		sortBy = "score"
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
			if a.Score != b.Score {
				return a.Score > b.Score
			}
		}
		if a.APYTotal != b.APYTotal {
			return a.APYTotal > b.APYTotal
		}
		if a.TVLUSD != b.TVLUSD {
			return a.TVLUSD > b.TVLUSD
		}
		return strings.Compare(a.OpportunityID, b.OpportunityID) < 0
	})
}

func clamp(v, min, max float64) float64 {
	if v < min {
		return min
	}
	if v > max {
		return max
	}
	return v
}
