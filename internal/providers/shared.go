package providers

import (
	"context"
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum/common"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers/yieldutil"
)

// SortLendMarkets sorts lend markets by TVL desc, then asset ID asc.
func SortLendMarkets(items []model.LendMarket) {
	sort.Slice(items, func(i, j int) bool {
		if items[i].TVLUSD != items[j].TVLUSD {
			return items[i].TVLUSD > items[j].TVLUSD
		}
		return items[i].AssetID < items[j].AssetID
	})
}

// SortLendRates sorts lend rates by supply APY desc, then asset ID asc.
func SortLendRates(items []model.LendRate) {
	sort.Slice(items, func(i, j int) bool {
		if items[i].SupplyAPY != items[j].SupplyAPY {
			return items[i].SupplyAPY > items[j].SupplyAPY
		}
		return items[i].AssetID < items[j].AssetID
	})
}

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
	out := v
	return &out
}

// CanonicalAssetID builds a CAIP-19 asset ID from an Asset and EVM address.
// Falls back to asset.AssetID when the address is empty or invalid.
func CanonicalAssetID(asset id.Asset, address string) string {
	addr := NormalizeEVMAddress(address)
	if addr == "" {
		return asset.AssetID
	}
	return fmt.Sprintf("%s/erc20:%s", asset.ChainID, addr)
}

// ProviderNativeID builds a composite native ID from provider, chain, and two addresses.
func ProviderNativeID(provider, chainID, addr1, addr2 string) string {
	return fmt.Sprintf("%s:%s:%s:%s", provider, chainID, NormalizeEVMAddress(addr1), NormalizeEVMAddress(addr2))
}

// HashOpportunity produces a deterministic SHA-1 hex hash for opportunity dedup.
func HashOpportunity(provider, chainID, nativeID, assetID string) string {
	seed := strings.Join([]string{provider, chainID, nativeID, assetID}, "|")
	h := sha1.Sum([]byte(seed))
	return hex.EncodeToString(h[:])
}

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

// FirstNonEmpty returns the first non-blank value from the list, trimmed.
func FirstNonEmpty(values ...string) string {
	for _, v := range values {
		if s := strings.TrimSpace(v); s != "" {
			return s
		}
	}
	return ""
}

// FormatSlippageBps formats a basis-points integer as a decimal fraction string (e.g. 50 → "0.005000").
func FormatSlippageBps(bps int64) string {
	return strconv.FormatFloat(float64(bps)/10000, 'f', 6, 64)
}

// EnsureHexPrefix prepends "0x" if the trimmed string doesn't already have it.
func EnsureHexPrefix(v string) string {
	clean := strings.TrimSpace(v)
	if strings.HasPrefix(clean, "0x") || strings.HasPrefix(clean, "0X") {
		return clean
	}
	return "0x" + clean
}

// ValidateEVMSender validates and returns a trimmed sender address.
// The operation string is used in error messages (e.g. "bridge execution", "swap execution").
func ValidateEVMSender(sender, operation string) (string, error) {
	s := strings.TrimSpace(sender)
	if s == "" {
		return "", clierr.New(clierr.CodeUsage, operation+" requires sender address")
	}
	if !common.IsHexAddress(s) {
		return "", clierr.New(clierr.CodeUsage, operation+" sender must be a valid EVM address")
	}
	return s, nil
}

// ValidateEVMRecipient validates and returns a trimmed recipient address, defaulting to sender if empty.
func ValidateEVMRecipient(recipient, sender, operation string) (string, error) {
	r := strings.TrimSpace(recipient)
	if r == "" {
		r = sender
	}
	if !common.IsHexAddress(r) {
		return "", clierr.New(clierr.CodeUsage, operation+" recipient must be a valid EVM address")
	}
	return r, nil
}

// NormalizeSlippageBps returns a validated slippage value, defaulting to 50 bps if non-positive.
func NormalizeSlippageBps(bps int64) (int64, error) {
	if bps <= 0 {
		bps = 50
	}
	if bps >= 10_000 {
		return 0, clierr.New(clierr.CodeUsage, "slippage bps must be less than 10000")
	}
	return bps, nil
}

// FinalizeLendPositions sorts and applies limit to lend positions.
func FinalizeLendPositions(out []model.LendPosition, limit int) []model.LendPosition {
	SortLendPositions(out)
	if limit > 0 && len(out) > limit {
		out = out[:limit]
	}
	return out
}

// FinalizeYieldPositions sorts and applies limit to yield positions.
func FinalizeYieldPositions(out []model.YieldPosition, limit int) []model.YieldPosition {
	SortYieldPositions(out)
	if limit > 0 && len(out) > limit {
		out = out[:limit]
	}
	return out
}

// FinalizeYieldOpportunities sorts, checks for empty, and applies limit to yield opportunities.
func FinalizeYieldOpportunities(out []model.YieldOpportunity, provider, sortBy string, limit int) ([]model.YieldOpportunity, error) {
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, fmt.Sprintf("no %s yield opportunities for requested chain/asset", provider))
	}
	yieldutil.Sort(out, sortBy)
	if limit <= 0 || limit > len(out) {
		limit = len(out)
	}
	return out[:limit], nil
}

// SortHistoryPoints sorts yield history points by timestamp ascending.
func SortHistoryPoints(points []model.YieldHistoryPoint) {
	sort.Slice(points, func(i, j int) bool {
		return points[i].Timestamp < points[j].Timestamp
	})
}

// ParseAPITime parses an RFC3339 (or RFC3339Nano) timestamp string, returning UTC time.
func ParseAPITime(v string) (time.Time, bool) {
	raw := strings.TrimSpace(v)
	if raw == "" {
		return time.Time{}, false
	}
	ts, err := time.Parse(time.RFC3339, raw)
	if err == nil {
		return ts.UTC(), true
	}
	ts, err = time.Parse(time.RFC3339Nano, raw)
	if err == nil {
		return ts.UTC(), true
	}
	return time.Time{}, false
}

// ValidateProvider checks that the given provider string matches the expected name (case-insensitive).
func ValidateProvider(provider, expected string) error {
	if !strings.EqualFold(strings.TrimSpace(provider), expected) {
		return clierr.New(clierr.CodeUnsupported, expected+" adapter supports only provider="+expected)
	}
	return nil
}

// GraphQLError represents a single error from a GraphQL response.
type GraphQLError struct {
	Message string `json:"message"`
}

// CheckGraphQLErrors returns an error if the GraphQL response contains errors.
func CheckGraphQLErrors(errs []GraphQLError, provider string) error {
	if len(errs) > 0 {
		return clierr.New(clierr.CodeUnavailable, fmt.Sprintf("%s graphql error: %s", provider, errs[0].Message))
	}
	return nil
}

// PostGraphQL executes a GraphQL query against the given endpoint, unmarshals the "data"
// portion of the response into result, and returns an error if the response contains GraphQL errors.
// Use this for standard GraphQL calls; for calls that need custom error handling (e.g. morpho
// no-results fallback), use the manual marshal/DoBodyJSON/CheckGraphQLErrors approach.
func PostGraphQL(ctx context.Context, client *httpx.Client, endpoint string, query string, variables map[string]any, result any, provider string) error {
	body, err := json.Marshal(map[string]any{
		"query":     query,
		"variables": variables,
	})
	if err != nil {
		return clierr.Wrap(clierr.CodeInternal, "marshal "+provider+" graphql query", err)
	}
	var raw struct {
		Data   json.RawMessage `json:"data"`
		Errors []GraphQLError  `json:"errors"`
	}
	if _, err := httpx.DoBodyJSON(ctx, client, http.MethodPost, endpoint, body, nil, &raw); err != nil {
		return err
	}
	if err := CheckGraphQLErrors(raw.Errors, provider); err != nil {
		return err
	}
	if result != nil {
		return json.Unmarshal(raw.Data, result)
	}
	return nil
}

// IsMorphoNoResultsError reports whether a GraphQL error message indicates no results were found.
func IsMorphoNoResultsError(message string) bool {
	return strings.Contains(strings.ToLower(strings.TrimSpace(message)), "no results matching given parameters")
}

// LendToYieldPositions converts supply/collateral lend positions to yield deposit positions.
func LendToYieldPositions(provider string, lendRows []model.LendPosition) []model.YieldPosition {
	out := make([]model.YieldPosition, 0, len(lendRows))
	for _, row := range lendRows {
		switch row.PositionType {
		case string(LendPositionTypeSupply), string(LendPositionTypeCollateral):
		default:
			continue
		}
		opportunityID := ""
		if strings.TrimSpace(row.ProviderNativeID) != "" {
			opportunityID = HashOpportunity(provider, row.ChainID, row.ProviderNativeID, row.AssetID)
		}
		out = append(out, model.YieldPosition{
			Protocol:             provider,
			Provider:             provider,
			ChainID:              row.ChainID,
			AccountAddress:       row.AccountAddress,
			PositionType:         "deposit",
			OpportunityID:        opportunityID,
			AssetID:              row.AssetID,
			ProviderNativeID:     row.ProviderNativeID,
			ProviderNativeIDKind: row.ProviderNativeIDKind,
			Amount:               row.Amount,
			AmountUSD:            row.AmountUSD,
			APYTotal:             row.APY,
			SourceURL:            row.SourceURL,
			FetchedAt:            row.FetchedAt,
		})
	}
	return out
}
