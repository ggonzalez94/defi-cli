package providers

import (
	"crypto/sha1"
	"encoding/hex"
	"fmt"
	"math"
	"sort"
	"strconv"
	"strings"

	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
)

// ── Sorting ────────────────────────────────────────────────────────────

// SortLendPositions sorts lend positions by USD value desc, then type, asset, native ID.
func SortLendPositions(items []model.LendPosition) {
	sort.Slice(items, func(i, j int) bool {
		if items[i].AmountUSD != items[j].AmountUSD {
			return items[i].AmountUSD > items[j].AmountUSD
		}
		if items[i].PositionType != items[j].PositionType {
			return items[i].PositionType < items[j].PositionType
		}
		if items[i].AssetID != items[j].AssetID {
			return items[i].AssetID < items[j].AssetID
		}
		return items[i].ProviderNativeID < items[j].ProviderNativeID
	})
}

// SortYieldPositions sorts yield positions by USD value desc, then APY, asset, native ID.
func SortYieldPositions(items []model.YieldPosition) {
	sort.Slice(items, func(i, j int) bool {
		if items[i].AmountUSD != items[j].AmountUSD {
			return items[i].AmountUSD > items[j].AmountUSD
		}
		if items[i].APYTotal != items[j].APYTotal {
			return items[i].APYTotal > items[j].APYTotal
		}
		if items[i].AssetID != items[j].AssetID {
			return items[i].AssetID < items[j].AssetID
		}
		return items[i].ProviderNativeID < items[j].ProviderNativeID
	})
}

// ── Matching / filtering ───────────────────────────────────────────────

// MatchesPositionType returns true if the position matches the filter (empty or "all" matches everything).
func MatchesPositionType(filter, position LendPositionType) bool {
	if filter == "" || filter == LendPositionTypeAll {
		return true
	}
	return filter == position
}

// MatchesAsset returns true if the given address/symbol matches the asset filter.
// An empty asset filter matches everything.
func MatchesAsset(address, symbol string, asset id.Asset) bool {
	if a := strings.TrimSpace(asset.Address); a != "" {
		return strings.EqualFold(strings.TrimSpace(address), a)
	}
	if s := strings.TrimSpace(asset.Symbol); s != "" {
		return strings.EqualFold(strings.TrimSpace(symbol), s)
	}
	return true
}

// ── EVM address helpers ────────────────────────────────────────────────

// NormalizeEVMAddress lowercases and validates a 0x-prefixed 42-char hex address.
// Returns empty string for invalid input.
func NormalizeEVMAddress(address string) string {
	addr := strings.ToLower(strings.TrimSpace(address))
	if len(addr) != 42 || !strings.HasPrefix(addr, "0x") {
		return ""
	}
	return addr
}

// CanonicalAssetIDForChain builds a CAIP-19 asset ID from a chain ID and EVM address.
func CanonicalAssetIDForChain(chainID, address string) string {
	addr := NormalizeEVMAddress(address)
	if chainID == "" || addr == "" {
		return ""
	}
	return fmt.Sprintf("%s/erc20:%s", chainID, addr)
}

// ── Amount helpers ─────────────────────────────────────────────────────

// NormalizeBaseUnits sanitises a base-unit string, returning "0" for empty/non-numeric input.
func NormalizeBaseUnits(v string) string {
	clean := strings.TrimSpace(v)
	if clean == "" {
		return "0"
	}
	for _, r := range clean {
		if r < '0' || r > '9' {
			return "0"
		}
	}
	return clean
}

// AmountInfoFromBase builds an AmountInfo from a base-unit string and decimal count.
func AmountInfoFromBase(base string, decimals int) model.AmountInfo {
	if decimals < 0 {
		decimals = 0
	}
	return model.AmountInfo{
		AmountBaseUnits: base,
		AmountDecimal:   id.FormatDecimalCompat(base, decimals),
		Decimals:        decimals,
	}
}

// AmountInfoFromRaw normalizes a raw string to base units, then builds AmountInfo.
func AmountInfoFromRaw(raw string, decimals int) model.AmountInfo {
	return AmountInfoFromBase(NormalizeBaseUnits(raw), decimals)
}

// PtrAmountInfo returns a pointer to a copy of the given AmountInfo.
func PtrAmountInfo(v model.AmountInfo) *model.AmountInfo {
	copy := v
	return &copy
}

// ── Hashing / IDs ──────────────────────────────────────────────────────

// HashOpportunity produces a deterministic SHA-1 hex hash for opportunity dedup.
func HashOpportunity(provider, chainID, marketID, assetID string) string {
	seed := strings.Join([]string{provider, chainID, marketID, assetID}, "|")
	h := sha1.Sum([]byte(seed))
	return hex.EncodeToString(h[:])
}

// ── Parsing ────────────────────────────────────────────────────────────

// ParseFloat parses a string to float64, returning 0 for errors, NaN, or Inf.
func ParseFloat(v string) float64 {
	f, err := strconv.ParseFloat(strings.TrimSpace(v), 64)
	if err != nil {
		return 0
	}
	if math.IsNaN(f) || math.IsInf(f, 0) {
		return 0
	}
	return f
}
