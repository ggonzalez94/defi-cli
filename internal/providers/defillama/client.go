package defillama

import (
	"context"
	"crypto/sha1"
	"encoding/hex"
	"fmt"
	"math"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"time"

	clierr "github.com/gustavo/defi-cli/internal/errors"
	"github.com/gustavo/defi-cli/internal/httpx"
	"github.com/gustavo/defi-cli/internal/id"
	"github.com/gustavo/defi-cli/internal/model"
	"github.com/gustavo/defi-cli/internal/providers"
)

const (
	defaultAPIBase    = "https://api.llama.fi"
	defaultYieldsBase = "https://yields.llama.fi"
)

type Client struct {
	http       *httpx.Client
	apiBase    string
	yieldsBase string
	now        func() time.Time
}

func New(httpClient *httpx.Client) *Client {
	return &Client{
		http:       httpClient,
		apiBase:    defaultAPIBase,
		yieldsBase: defaultYieldsBase,
		now:        time.Now,
	}
}

func (c *Client) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:        "defillama",
		Type:        "market+yields",
		RequiresKey: false,
		Capabilities: []string{
			"chains.top",
			"protocols.top",
			"lend.markets",
			"lend.rates",
			"yield.opportunities",
		},
	}
}

type chainResp struct {
	Name string  `json:"name"`
	TVL  float64 `json:"tvl"`
}

func (c *Client) ChainsTop(ctx context.Context, limit int) ([]model.ChainTVL, error) {
	url := c.apiBase + "/v2/chains"
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "build chains request", err)
	}
	var resp []chainResp
	if _, err := c.http.DoJSON(ctx, req, &resp); err != nil {
		return nil, err
	}

	sort.Slice(resp, func(i, j int) bool {
		return resp[i].TVL > resp[j].TVL
	})
	if limit <= 0 || limit > len(resp) {
		limit = len(resp)
	}
	out := make([]model.ChainTVL, 0, limit)
	for i := 0; i < limit; i++ {
		item := resp[i]
		chainID := ""
		if chain, err := id.ParseChain(item.Name); err == nil {
			chainID = chain.CAIP2
		}
		out = append(out, model.ChainTVL{Rank: i + 1, Chain: item.Name, ChainID: chainID, TVLUSD: item.TVL})
	}
	return out, nil
}

type protocolResp struct {
	Name     string  `json:"name"`
	Category string  `json:"category"`
	TVL      float64 `json:"tvl"`
}

func (c *Client) ProtocolsTop(ctx context.Context, category string, limit int) ([]model.ProtocolTVL, error) {
	url := c.apiBase + "/protocols"
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "build protocols request", err)
	}
	var resp []protocolResp
	if _, err := c.http.DoJSON(ctx, req, &resp); err != nil {
		return nil, err
	}

	normCategory := strings.ToLower(strings.TrimSpace(category))
	filtered := make([]protocolResp, 0, len(resp))
	for _, p := range resp {
		if normCategory != "" && strings.ToLower(p.Category) != normCategory {
			continue
		}
		filtered = append(filtered, p)
	}

	sort.Slice(filtered, func(i, j int) bool {
		return filtered[i].TVL > filtered[j].TVL
	})
	if limit <= 0 || limit > len(filtered) {
		limit = len(filtered)
	}

	out := make([]model.ProtocolTVL, 0, limit)
	for i := 0; i < limit; i++ {
		item := filtered[i]
		out = append(out, model.ProtocolTVL{Rank: i + 1, Protocol: item.Name, Category: item.Category, TVLUSD: item.TVL})
	}
	return out, nil
}

type poolsEnvelope struct {
	Status string      `json:"status"`
	Data   []poolEntry `json:"data"`
}

type poolEntry struct {
	Pool       string   `json:"pool"`
	Chain      string   `json:"chain"`
	Project    string   `json:"project"`
	Symbol     string   `json:"symbol"`
	Underlying []string `json:"underlyingTokens"`
	APYBase    *float64 `json:"apyBase"`
	APYReward  *float64 `json:"apyReward"`
	APY        *float64 `json:"apy"`
	TVLUSD     *float64 `json:"tvlUsd"`
	ILRisk     string   `json:"ilRisk"`
	Stablecoin bool     `json:"stablecoin"`
	Exposure   string   `json:"exposure"`
	PoolMeta   string   `json:"poolMeta"`
	URL        string   `json:"url"`
	Timestamp  string   `json:"timestamp"`
}

func (c *Client) getPools(ctx context.Context) ([]poolEntry, error) {
	url := c.yieldsBase + "/pools"
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "build yields request", err)
	}
	var env poolsEnvelope
	if _, err := c.http.DoJSON(ctx, req, &env); err != nil {
		return nil, err
	}
	if len(env.Data) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "defillama yields returned no pools")
	}
	return env.Data, nil
}

func (c *Client) LendMarkets(ctx context.Context, protocol string, chain id.Chain, asset id.Asset) ([]model.LendMarket, error) {
	pools, err := c.getPools(ctx)
	if err != nil {
		return nil, err
	}
	protoMatchers := protocolMatcher(protocol)
	if len(protoMatchers) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "unsupported lending protocol")
	}

	out := make([]model.LendMarket, 0)
	for _, p := range pools {
		if !matchesChain(p.Chain, chain) {
			continue
		}
		if !matchesAny(p.Project, protoMatchers) {
			continue
		}
		if !matchesAssetSymbol(p.Symbol, asset.Symbol) {
			continue
		}

		apyTotal := numOrZero(p.APY)
		apyBase := numOrZero(p.APYBase)
		tvl := numOrZero(p.TVLUSD)
		if tvl <= 0 {
			continue
		}
		out = append(out, model.LendMarket{
			Protocol:     canonicalProtocol(p.Project, protocol),
			ChainID:      chain.CAIP2,
			AssetID:      asset.AssetID,
			SupplyAPY:    choosePositive(apyBase, apyTotal),
			BorrowAPY:    0,
			TVLUSD:       tvl,
			LiquidityUSD: tvl,
			SourceURL:    p.URL,
			FetchedAt:    c.now().UTC().Format(time.RFC3339),
		})
	}

	sort.Slice(out, func(i, j int) bool {
		return out[i].TVLUSD > out[j].TVLUSD
	})
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "no lend markets found for query")
	}
	return out, nil
}

func (c *Client) LendRates(ctx context.Context, protocol string, chain id.Chain, asset id.Asset) ([]model.LendRate, error) {
	pools, err := c.getPools(ctx)
	if err != nil {
		return nil, err
	}
	protoMatchers := protocolMatcher(protocol)
	if len(protoMatchers) == 0 {
		return nil, clierr.New(clierr.CodeUnsupported, "unsupported lending protocol")
	}

	out := make([]model.LendRate, 0)
	for _, p := range pools {
		if !matchesChain(p.Chain, chain) {
			continue
		}
		if !matchesAny(p.Project, protoMatchers) {
			continue
		}
		if !matchesAssetSymbol(p.Symbol, asset.Symbol) {
			continue
		}

		apyTotal := numOrZero(p.APY)
		apyBase := numOrZero(p.APYBase)
		out = append(out, model.LendRate{
			Protocol:    canonicalProtocol(p.Project, protocol),
			ChainID:     chain.CAIP2,
			AssetID:     asset.AssetID,
			SupplyAPY:   choosePositive(apyBase, apyTotal),
			BorrowAPY:   0,
			Utilization: 0,
			SourceURL:   p.URL,
			FetchedAt:   c.now().UTC().Format(time.RFC3339),
		})
	}

	sort.Slice(out, func(i, j int) bool {
		return out[i].SupplyAPY > out[j].SupplyAPY
	})
	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "no lend rates found for query")
	}
	return out, nil
}

func (c *Client) YieldOpportunities(ctx context.Context, req providers.YieldRequest) ([]model.YieldOpportunity, error) {
	pools, err := c.getPools(ctx)
	if err != nil {
		return nil, err
	}

	allowedProtocols := map[string]struct{}{}
	if len(req.Providers) > 0 {
		for _, p := range req.Providers {
			allowedProtocols[strings.ToLower(strings.TrimSpace(p))] = struct{}{}
		}
	}

	maxRiskScore := riskOrder(req.MaxRisk)
	if maxRiskScore == 0 {
		maxRiskScore = riskOrder("high")
	}

	out := make([]model.YieldOpportunity, 0)
	warnings := 0
	for _, p := range pools {
		if !matchesChain(p.Chain, req.Chain) {
			continue
		}
		if !matchesAssetSymbol(p.Symbol, req.Asset.Symbol) {
			continue
		}
		if len(allowedProtocols) > 0 {
			if _, ok := allowedProtocols[strings.ToLower(p.Project)]; !ok {
				continue
			}
		}

		apyTotal := numOrZero(p.APY)
		tvl := numOrZero(p.TVLUSD)
		if (apyTotal == 0 || tvl == 0) && !req.IncludeIncomplete {
			continue
		}
		if apyTotal < req.MinAPY {
			continue
		}
		if tvl < req.MinTVLUSD {
			continue
		}

		riskLevel, riskReasons := deriveRisk(p)
		if riskOrder(riskLevel) > maxRiskScore {
			continue
		}
		if apyTotal == 0 || tvl == 0 {
			warnings++
		}

		liq := tvl
		score := scoreOpportunity(apyTotal, tvl, liq, riskLevel)
		oppID := opportunityID("defillama", req.Chain.CAIP2, p.Pool, req.Asset.AssetID)

		out = append(out, model.YieldOpportunity{
			OpportunityID:   oppID,
			Provider:        "defillama",
			Protocol:        p.Project,
			ChainID:         req.Chain.CAIP2,
			AssetID:         req.Asset.AssetID,
			Type:            deriveType(p),
			APYBase:         numOrZero(p.APYBase),
			APYReward:       numOrZero(p.APYReward),
			APYTotal:        apyTotal,
			TVLUSD:          tvl,
			LiquidityUSD:    liq,
			LockupDays:      0,
			WithdrawalTerms: "variable",
			RiskLevel:       riskLevel,
			RiskReasons:     riskReasons,
			Score:           score,
			SourceURL:       p.URL,
			FetchedAt:       c.now().UTC().Format(time.RFC3339),
		})
	}

	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "no yield opportunities found for query")
	}

	sortYield(out, req.SortBy)
	if req.Limit <= 0 || req.Limit > len(out) {
		req.Limit = len(out)
	}
	out = out[:req.Limit]
	if warnings > 0 && !req.IncludeIncomplete {
		return out, clierr.New(clierr.CodeUnavailable, "internal filtering inconsistency")
	}
	return out, nil
}

func protocolMatcher(protocol string) []string {
	switch strings.ToLower(strings.TrimSpace(protocol)) {
	case "aave":
		return []string{"aave", "aave-v2", "aave-v3"}
	case "morpho":
		return []string{"morpho", "morpho-blue"}
	case "spark":
		return []string{"spark"}
	default:
		return nil
	}
}

func matchesAny(value string, candidates []string) bool {
	value = strings.ToLower(strings.TrimSpace(value))
	for _, c := range candidates {
		if value == strings.ToLower(strings.TrimSpace(c)) {
			return true
		}
	}
	return false
}

func canonicalProtocol(observed, fallback string) string {
	if observed == "" {
		return strings.ToLower(fallback)
	}
	if strings.HasPrefix(strings.ToLower(observed), "aave") {
		return "aave"
	}
	if strings.HasPrefix(strings.ToLower(observed), "morpho") {
		return "morpho"
	}
	if strings.HasPrefix(strings.ToLower(observed), "spark") {
		return "spark"
	}
	return strings.ToLower(observed)
}

func matchesChain(input string, chain id.Chain) bool {
	normInput := strings.ToLower(strings.TrimSpace(input))
	if normInput == "" {
		return false
	}
	if strings.EqualFold(normInput, chain.Name) {
		return true
	}
	if strings.EqualFold(normInput, chain.Slug) {
		return true
	}
	if strings.Contains(normInput, " ") {
		normInput = strings.ReplaceAll(normInput, " ", "-")
	}
	return normInput == chain.Slug
}

func matchesAssetSymbol(symbolRaw string, expected string) bool {
	if strings.TrimSpace(expected) == "" {
		return true
	}
	symbolRaw = strings.ToUpper(strings.TrimSpace(symbolRaw))
	expected = strings.ToUpper(strings.TrimSpace(expected))
	for _, part := range strings.Split(symbolRaw, "-") {
		if strings.TrimSpace(part) == expected {
			return true
		}
	}
	for _, part := range strings.Split(symbolRaw, "/") {
		if strings.TrimSpace(part) == expected {
			return true
		}
	}
	return symbolRaw == expected
}

func numOrZero(v *float64) float64 {
	if v == nil {
		return 0
	}
	if math.IsNaN(*v) || math.IsInf(*v, 0) {
		return 0
	}
	return *v
}

func choosePositive(primary, fallback float64) float64 {
	if primary > 0 {
		return primary
	}
	if fallback > 0 {
		return fallback
	}
	return 0
}

func deriveRisk(p poolEntry) (string, []string) {
	reasons := []string{}
	if p.Stablecoin && strings.TrimSpace(p.ILRisk) == "no" {
		reasons = append(reasons, "stablecoin exposure")
		return "low", reasons
	}
	if strings.EqualFold(strings.TrimSpace(p.ILRisk), "yes") || strings.EqualFold(strings.TrimSpace(p.Exposure), "volatile") {
		reasons = append(reasons, "impermanent loss or volatile exposure")
		return "high", reasons
	}
	if strings.TrimSpace(p.ILRisk) == "" {
		reasons = append(reasons, "missing IL risk metadata")
		return "unknown", reasons
	}
	reasons = append(reasons, "non-stable pool")
	return "medium", reasons
}

func deriveType(p poolEntry) string {
	project := strings.ToLower(strings.TrimSpace(p.Project))
	meta := strings.ToLower(strings.TrimSpace(p.PoolMeta))
	symbol := strings.ToLower(strings.TrimSpace(p.Symbol))

	if strings.Contains(project, "aave") || strings.Contains(project, "morpho") || strings.Contains(project, "spark") {
		return "lend"
	}
	if strings.Contains(project, "pendle") {
		return "fixed_yield"
	}
	if strings.Contains(symbol, "-") || strings.Contains(symbol, "/") {
		if strings.EqualFold(strings.TrimSpace(p.ILRisk), "yes") {
			return "lp_volatile"
		}
		return "lp_stable"
	}
	if strings.Contains(meta, "stake") {
		return "staking"
	}
	return "lend"
}

func riskOrder(v string) int {
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

func scoreOpportunity(apyTotal, tvlUSD, liquidityUSD float64, riskLevel string) float64 {
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
	score := math.Round(clamp(scoreRaw, 0, 1)*100*100) / 100
	return score
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

func opportunityID(provider, chainID, marketID, assetID string) string {
	seed := strings.Join([]string{provider, chainID, marketID, assetID}, "|")
	h := sha1.Sum([]byte(seed))
	return hex.EncodeToString(h[:])
}

func sortYield(items []model.YieldOpportunity, sortBy string) {
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

func ParseQuoteAmount(v any) (string, error) {
	switch t := v.(type) {
	case string:
		if t == "" {
			return "", fmt.Errorf("empty quote amount")
		}
		if _, err := strconv.ParseFloat(t, 64); err != nil {
			return "", err
		}
		return t, nil
	case float64:
		return strconv.FormatFloat(t, 'f', -1, 64), nil
	default:
		return "", fmt.Errorf("unsupported amount type %T", v)
	}
}
