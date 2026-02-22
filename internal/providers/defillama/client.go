package defillama

import (
	"context"
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math"
	"net/http"
	"net/url"
	"sort"
	"strconv"
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

const (
	defaultAPIBase      = "https://api.llama.fi"
	defaultYieldsBase   = "https://yields.llama.fi"
	defaultBridgeAPIURL = "https://pro-api.llama.fi"
)

type Client struct {
	http          *httpx.Client
	apiBase       string
	yieldsBase    string
	bridgeBaseURL string
	apiKey        string
	now           func() time.Time
}

func New(httpClient *httpx.Client, apiKey string) *Client {
	return &Client{
		http:          httpClient,
		apiBase:       defaultAPIBase,
		yieldsBase:    defaultYieldsBase,
		bridgeBaseURL: defaultBridgeAPIURL,
		apiKey:        strings.TrimSpace(apiKey),
		now:           time.Now,
	}
}

func (c *Client) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:        "defillama",
		Type:        "market+yields",
		RequiresKey: false,
		Capabilities: []string{
			"chains.top",
			"chains.assets",
			"protocols.top",
			"protocols.categories",
			"lend.markets",
			"lend.rates",
			"yield.opportunities",
			"bridge.list",
			"bridge.details",
		},
		KeyEnvVarName: "DEFI_DEFILLAMA_API_KEY",
		CapabilityAuth: []model.ProviderCapabilityAuth{
			{
				Capability:  "chains.assets",
				KeyEnvVar:   "DEFI_DEFILLAMA_API_KEY",
				Description: "Required for chain-level TVL by asset endpoint",
			},
			{
				Capability:  "bridge.details",
				KeyEnvVar:   "DEFI_DEFILLAMA_API_KEY",
				Description: "Required for bridge analytics details endpoint",
			},
			{
				Capability:  "bridge.list",
				KeyEnvVar:   "DEFI_DEFILLAMA_API_KEY",
				Description: "Required for bridge analytics list endpoint",
			},
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

type chainAssetsCategory struct {
	Breakdown map[string]any `json:"breakdown"`
}

func (c *Client) ChainsAssets(ctx context.Context, chain id.Chain, asset id.Asset, limit int) ([]model.ChainAssetTVL, error) {
	if err := c.requireChainAssetsAPIKey(); err != nil {
		return nil, err
	}

	endpoint := c.chainAssetsURL(nil)
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, endpoint, nil)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "build chain assets request", err)
	}

	var raw map[string]json.RawMessage
	if _, err := c.http.DoJSON(ctx, req, &raw); err != nil {
		return nil, err
	}

	assetsBySymbol, chainName, err := selectChainAssetBreakdown(raw, chain)
	if err != nil {
		return nil, err
	}

	filterSymbol := strings.ToUpper(strings.TrimSpace(asset.Symbol))
	out := make([]model.ChainAssetTVL, 0, len(assetsBySymbol))
	for symbol, tvl := range assetsBySymbol {
		if filterSymbol != "" && symbol != filterSymbol {
			continue
		}
		if tvl <= 0 {
			continue
		}
		out = append(out, model.ChainAssetTVL{
			Chain:   chainName,
			ChainID: chain.CAIP2,
			Asset:   symbol,
			AssetID: knownAssetID(chain, symbol),
			TVLUSD:  tvl,
		})
	}

	if len(out) == 0 {
		if filterSymbol != "" {
			return nil, clierr.New(clierr.CodeUnavailable, "no chain asset tvl found for requested chain/asset")
		}
		return nil, clierr.New(clierr.CodeUnavailable, "no chain asset tvl found for requested chain")
	}

	sort.Slice(out, func(i, j int) bool {
		if out[i].TVLUSD != out[j].TVLUSD {
			return out[i].TVLUSD > out[j].TVLUSD
		}
		return strings.Compare(out[i].Asset, out[j].Asset) < 0
	})
	if limit > 0 && len(out) > limit {
		out = out[:limit]
	}
	for i := range out {
		out[i].Rank = i + 1
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

func (c *Client) ProtocolsCategories(ctx context.Context) ([]model.ProtocolCategory, error) {
	url := c.apiBase + "/protocols"
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "build protocols request", err)
	}
	var resp []protocolResp
	if _, err := c.http.DoJSON(ctx, req, &resp); err != nil {
		return nil, err
	}

	type catAgg struct {
		name      string
		protocols int
		tvl       float64
	}
	agg := map[string]*catAgg{}
	for _, p := range resp {
		cat := strings.TrimSpace(p.Category)
		if cat == "" {
			continue
		}
		key := strings.ToLower(cat)
		entry, ok := agg[key]
		if !ok {
			entry = &catAgg{name: cat}
			agg[key] = entry
		}
		entry.protocols++
		entry.tvl += p.TVL
	}

	out := make([]model.ProtocolCategory, 0, len(agg))
	for _, entry := range agg {
		out = append(out, model.ProtocolCategory{
			Name:      entry.name,
			Protocols: entry.protocols,
			TVLUSD:    entry.tvl,
		})
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].TVLUSD != out[j].TVLUSD {
			return out[i].TVLUSD > out[j].TVLUSD
		}
		if out[i].Protocols != out[j].Protocols {
			return out[i].Protocols > out[j].Protocols
		}
		return strings.Compare(strings.ToLower(out[i].Name), strings.ToLower(out[j].Name)) < 0
	})
	return out, nil
}

type bridgeListEnvelope struct {
	Bridges []bridgeListItem `json:"bridges"`
}

type bridgeListItem struct {
	ID                  int      `json:"id"`
	Name                string   `json:"name"`
	DisplayName         string   `json:"displayName"`
	Slug                string   `json:"slug"`
	DestinationChain    any      `json:"destinationChain"`
	URL                 string   `json:"url"`
	Chains              []string `json:"chains"`
	LastHourlyVolume    *float64 `json:"lastHourlyVolume"`
	Last24hVolume       *float64 `json:"last24hVolume"`
	LastDailyVolume     *float64 `json:"lastDailyVolume"`
	VolumePrevDay       *float64 `json:"volumePrevDay"`
	DayBeforeLastVolume *float64 `json:"dayBeforeLastVolume"`
	VolumePrev2Day      *float64 `json:"volumePrev2Day"`
	WeeklyVolume        *float64 `json:"weeklyVolume"`
	MonthlyVolume       *float64 `json:"monthlyVolume"`
}

type bridgeDetailResponse struct {
	ID                  int                           `json:"id"`
	Name                string                        `json:"name"`
	DisplayName         string                        `json:"displayName"`
	DestinationChain    any                           `json:"destinationChain"`
	LastHourlyVolume    *float64                      `json:"lastHourlyVolume"`
	Last24hVolume       *float64                      `json:"last24hVolume"`
	LastDailyVolume     *float64                      `json:"lastDailyVolume"`
	CurrentDayVolume    *float64                      `json:"currentDayVolume"`
	VolumePrevDay       *float64                      `json:"volumePrevDay"`
	DayBeforeLastVolume *float64                      `json:"dayBeforeLastVolume"`
	VolumePrev2Day      *float64                      `json:"volumePrev2Day"`
	WeeklyVolume        *float64                      `json:"weeklyVolume"`
	MonthlyVolume       *float64                      `json:"monthlyVolume"`
	LastHourlyTxs       bridgeTxCounts                `json:"lastHourlyTxs"`
	CurrentDayTxs       bridgeTxCounts                `json:"currentDayTxs"`
	PrevDayTxs          bridgeTxCounts                `json:"prevDayTxs"`
	DayBeforeLastTxs    bridgeTxCounts                `json:"dayBeforeLastTxs"`
	WeeklyTxs           bridgeTxCounts                `json:"weeklyTxs"`
	MonthlyTxs          bridgeTxCounts                `json:"monthlyTxs"`
	ChainBreakdown      map[string]bridgeChainMetrics `json:"chainBreakdown"`
}

type bridgeChainMetrics struct {
	LastHourlyVolume    *float64       `json:"lastHourlyVolume"`
	Last24hVolume       *float64       `json:"last24hVolume"`
	LastDailyVolume     *float64       `json:"lastDailyVolume"`
	CurrentDayVolume    *float64       `json:"currentDayVolume"`
	VolumePrevDay       *float64       `json:"volumePrevDay"`
	DayBeforeLastVolume *float64       `json:"dayBeforeLastVolume"`
	VolumePrev2Day      *float64       `json:"volumePrev2Day"`
	WeeklyVolume        *float64       `json:"weeklyVolume"`
	MonthlyVolume       *float64       `json:"monthlyVolume"`
	LastHourlyTxs       bridgeTxCounts `json:"lastHourlyTxs"`
	CurrentDayTxs       bridgeTxCounts `json:"currentDayTxs"`
	PrevDayTxs          bridgeTxCounts `json:"prevDayTxs"`
	DayBeforeLastTxs    bridgeTxCounts `json:"dayBeforeLastTxs"`
	WeeklyTxs           bridgeTxCounts `json:"weeklyTxs"`
	MonthlyTxs          bridgeTxCounts `json:"monthlyTxs"`
}

type bridgeTxCounts struct {
	Deposits    float64 `json:"deposits"`
	Withdrawals float64 `json:"withdrawals"`
}

func (c *Client) ListBridges(ctx context.Context, req providers.BridgeListRequest) ([]model.BridgeSummary, error) {
	items, err := c.fetchBridgeList(ctx, req.IncludeChains)
	if err != nil {
		return nil, err
	}
	if len(items) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "defillama bridges returned no data")
	}

	fetchedAt := c.now().UTC()
	out := make([]model.BridgeSummary, 0, len(items))
	for _, item := range items {
		out = append(out, model.BridgeSummary{
			BridgeID:         item.ID,
			Name:             item.Name,
			DisplayName:      item.DisplayName,
			Slug:             item.Slug,
			DestinationChain: normalizeDestinationChain(item.DestinationChain),
			URL:              strings.TrimSpace(item.URL),
			Chains:           normalizeStringSlice(item.Chains),
			Volumes: bridgeVolumesFromParts(
				item.LastHourlyVolume,
				item.Last24hVolume,
				item.LastDailyVolume,
				item.VolumePrevDay,
				item.DayBeforeLastVolume,
				item.VolumePrev2Day,
				item.WeeklyVolume,
				item.MonthlyVolume,
			),
			LastUpdatedUNIX: fetchedAt.Unix(),
			FetchedAt:       fetchedAt.Format(time.RFC3339),
		})
	}

	sort.Slice(out, func(i, j int) bool {
		if out[i].Volumes.Last24hUSD != out[j].Volumes.Last24hUSD {
			return out[i].Volumes.Last24hUSD > out[j].Volumes.Last24hUSD
		}
		if out[i].Volumes.WeeklyUSD != out[j].Volumes.WeeklyUSD {
			return out[i].Volumes.WeeklyUSD > out[j].Volumes.WeeklyUSD
		}
		return strings.Compare(out[i].Name, out[j].Name) < 0
	})

	if req.Limit > 0 && len(out) > req.Limit {
		out = out[:req.Limit]
	}
	return out, nil
}

func (c *Client) BridgeDetails(ctx context.Context, req providers.BridgeDetailsRequest) (model.BridgeDetails, error) {
	bridgeRef := strings.TrimSpace(req.Bridge)
	if bridgeRef == "" {
		return model.BridgeDetails{}, clierr.New(clierr.CodeUsage, "bridge identifier is required")
	}
	bridgeID, err := c.resolveBridgeID(ctx, bridgeRef)
	if err != nil {
		return model.BridgeDetails{}, err
	}

	if err := c.requireBridgeAPIKey(); err != nil {
		return model.BridgeDetails{}, err
	}

	endpoint := c.bridgeURL(fmt.Sprintf("/bridge/%d", bridgeID), nil)
	hReq, err := http.NewRequestWithContext(ctx, http.MethodGet, endpoint, nil)
	if err != nil {
		return model.BridgeDetails{}, clierr.Wrap(clierr.CodeInternal, "build bridge details request", err)
	}

	var resp bridgeDetailResponse
	if _, err := c.http.DoJSON(ctx, hReq, &resp); err != nil {
		return model.BridgeDetails{}, err
	}

	fetchedAt := c.now().UTC()
	details := model.BridgeDetails{
		BridgeID:         resp.ID,
		Name:             resp.Name,
		DisplayName:      resp.DisplayName,
		DestinationChain: normalizeDestinationChain(resp.DestinationChain),
		Volumes: bridgeVolumesFromParts(
			resp.LastHourlyVolume,
			resp.Last24hVolume,
			resp.LastDailyVolume,
			resp.VolumePrevDay,
			resp.DayBeforeLastVolume,
			resp.VolumePrev2Day,
			resp.WeeklyVolume,
			resp.MonthlyVolume,
		),
		Transactions: model.BridgeTransactions{
			LastHourly: txCountsFrom(resp.LastHourlyTxs),
			CurrentDay: txCountsFrom(resp.CurrentDayTxs),
			PrevDay:    txCountsFrom(resp.PrevDayTxs),
			Prev2Day:   txCountsFrom(resp.DayBeforeLastTxs),
			Weekly:     txCountsFrom(resp.WeeklyTxs),
			Monthly:    txCountsFrom(resp.MonthlyTxs),
		},
		LastUpdatedUNIX: fetchedAt.Unix(),
		FetchedAt:       fetchedAt.Format(time.RFC3339),
	}

	if !req.IncludeChainBreakdown {
		return details, nil
	}

	breakdown := make([]model.BridgeChainDetails, 0, len(resp.ChainBreakdown))
	for chainName, chain := range resp.ChainBreakdown {
		chainID := ""
		if parsed, parseErr := id.ParseChain(chainName); parseErr == nil {
			chainID = parsed.CAIP2
		}
		breakdown = append(breakdown, model.BridgeChainDetails{
			Chain:   chainName,
			ChainID: chainID,
			Volumes: bridgeVolumesFromParts(
				chain.LastHourlyVolume,
				chain.Last24hVolume,
				chain.LastDailyVolume,
				chain.VolumePrevDay,
				chain.DayBeforeLastVolume,
				chain.VolumePrev2Day,
				chain.WeeklyVolume,
				chain.MonthlyVolume,
			),
			Transactions: model.BridgeTransactions{
				LastHourly: txCountsFrom(chain.LastHourlyTxs),
				CurrentDay: txCountsFrom(chain.CurrentDayTxs),
				PrevDay:    txCountsFrom(chain.PrevDayTxs),
				Prev2Day:   txCountsFrom(chain.DayBeforeLastTxs),
				Weekly:     txCountsFrom(chain.WeeklyTxs),
				Monthly:    txCountsFrom(chain.MonthlyTxs),
			},
		})
	}
	sort.Slice(breakdown, func(i, j int) bool {
		if breakdown[i].Volumes.Last24hUSD != breakdown[j].Volumes.Last24hUSD {
			return breakdown[i].Volumes.Last24hUSD > breakdown[j].Volumes.Last24hUSD
		}
		return strings.Compare(breakdown[i].Chain, breakdown[j].Chain) < 0
	})
	details.ChainBreakdown = breakdown

	return details, nil
}

func (c *Client) fetchBridgeList(ctx context.Context, includeChains bool) ([]bridgeListItem, error) {
	if err := c.requireBridgeAPIKey(); err != nil {
		return nil, err
	}

	query := url.Values{}
	if includeChains {
		query.Set("includeChains", "true")
	}
	endpoint := c.bridgeURL("/bridges", query)
	hReq, err := http.NewRequestWithContext(ctx, http.MethodGet, endpoint, nil)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "build bridges request", err)
	}

	var resp bridgeListEnvelope
	if _, err := c.http.DoJSON(ctx, hReq, &resp); err != nil {
		return nil, err
	}
	return resp.Bridges, nil
}

func (c *Client) resolveBridgeID(ctx context.Context, ref string) (int, error) {
	if idNum, err := strconv.Atoi(strings.TrimSpace(ref)); err == nil {
		if idNum <= 0 {
			return 0, clierr.New(clierr.CodeUsage, "bridge id must be > 0")
		}
		return idNum, nil
	}

	items, err := c.fetchBridgeList(ctx, false)
	if err != nil {
		return 0, err
	}
	normRef := strings.ToLower(strings.TrimSpace(ref))

	exact := make([]bridgeListItem, 0, 1)
	for _, item := range items {
		if bridgeMatchesExact(item, normRef) {
			exact = append(exact, item)
		}
	}
	if len(exact) == 1 {
		return exact[0].ID, nil
	}
	if len(exact) > 1 {
		return 0, clierr.New(clierr.CodeUsage, "bridge reference is ambiguous; use bridge id")
	}

	partial := make([]bridgeListItem, 0, 3)
	for _, item := range items {
		if bridgeMatchesPartial(item, normRef) {
			partial = append(partial, item)
		}
	}
	if len(partial) == 1 {
		return partial[0].ID, nil
	}
	if len(partial) > 1 {
		return 0, clierr.New(clierr.CodeUsage, "bridge reference matched multiple bridges; use bridge id")
	}
	return 0, clierr.New(clierr.CodeUsage, fmt.Sprintf("bridge not found: %s", ref))
}

func bridgeMatchesExact(item bridgeListItem, ref string) bool {
	return strings.EqualFold(item.Name, ref) ||
		strings.EqualFold(item.DisplayName, ref) ||
		strings.EqualFold(item.Slug, ref)
}

func bridgeMatchesPartial(item bridgeListItem, ref string) bool {
	name := strings.ToLower(item.Name)
	displayName := strings.ToLower(item.DisplayName)
	slug := strings.ToLower(item.Slug)
	return strings.Contains(name, ref) || strings.Contains(displayName, ref) || strings.Contains(slug, ref)
}

func bridgeVolumesFromParts(lastHourly, last24h, lastDaily, prevDay, dayBeforeLast, prev2Day, weekly, monthly *float64) model.BridgeVolumes {
	return model.BridgeVolumes{
		LastHourlyUSD: valOrZero(lastHourly),
		Last24hUSD:    firstNonNilFloat(last24h, lastDaily, prevDay),
		LastDailyUSD:  firstNonNilFloat(lastDaily, prevDay),
		PrevDayUSD:    firstNonNilFloat(prevDay, lastDaily),
		Prev2DayUSD:   firstNonNilFloat(prev2Day, dayBeforeLast),
		WeeklyUSD:     valOrZero(weekly),
		MonthlyUSD:    valOrZero(monthly),
	}
}

func txCountsFrom(v bridgeTxCounts) model.BridgeTxCounts {
	return model.BridgeTxCounts{
		Deposits:    int64(v.Deposits),
		Withdrawals: int64(v.Withdrawals),
	}
}

func valOrZero(v *float64) float64 {
	if v == nil {
		return 0
	}
	return *v
}

func firstNonNilFloat(values ...*float64) float64 {
	for _, value := range values {
		if value != nil {
			return *value
		}
	}
	return 0
}

func normalizeStringSlice(items []string) []string {
	if len(items) == 0 {
		return nil
	}
	seen := make(map[string]struct{}, len(items))
	out := make([]string, 0, len(items))
	for _, item := range items {
		clean := strings.TrimSpace(item)
		if clean == "" {
			continue
		}
		key := strings.ToLower(clean)
		if _, ok := seen[key]; ok {
			continue
		}
		seen[key] = struct{}{}
		out = append(out, clean)
	}
	sort.Strings(out)
	return out
}

func normalizeDestinationChain(v any) string {
	switch t := v.(type) {
	case string:
		clean := strings.TrimSpace(t)
		if strings.EqualFold(clean, "false") {
			return ""
		}
		return clean
	case bool:
		if !t {
			return ""
		}
		return "true"
	default:
		return ""
	}
}

func (c *Client) requireChainAssetsAPIKey() error {
	if strings.TrimSpace(c.apiKey) == "" {
		return clierr.New(clierr.CodeAuth, "defillama chain asset tvl requires DEFI_DEFILLAMA_API_KEY")
	}
	return nil
}

func (c *Client) requireBridgeAPIKey() error {
	if strings.TrimSpace(c.apiKey) == "" {
		return clierr.New(clierr.CodeAuth, "defillama bridge data requires DEFI_DEFILLAMA_API_KEY")
	}
	return nil
}

func (c *Client) chainAssetsURL(query url.Values) string {
	base := strings.TrimSuffix(c.bridgeBaseURL, "/")
	endpoint := fmt.Sprintf("%s/%s/api/chainAssets", base, c.apiKey)
	if len(query) > 0 {
		return endpoint + "?" + query.Encode()
	}
	return endpoint
}

func (c *Client) bridgeURL(path string, query url.Values) string {
	cleanPath := strings.TrimPrefix(strings.TrimSpace(path), "/")
	base := strings.TrimSuffix(c.bridgeBaseURL, "/")
	endpoint := fmt.Sprintf("%s/%s/bridges/%s", base, c.apiKey, cleanPath)
	if len(query) > 0 {
		return endpoint + "?" + query.Encode()
	}
	return endpoint
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

func selectChainAssetBreakdown(raw map[string]json.RawMessage, chain id.Chain) (map[string]float64, string, error) {
	type candidate struct {
		name   string
		rank   int
		assets map[string]float64
	}
	matches := make([]candidate, 0, 2)
	for name, body := range raw {
		if strings.EqualFold(strings.TrimSpace(name), "timestamp") {
			continue
		}
		if !matchesChain(name, chain) {
			continue
		}
		assets, err := parseChainAssetBreakdown(body)
		if err != nil {
			return nil, "", clierr.Wrap(clierr.CodeInternal, "parse defillama chain asset payload", err)
		}
		if len(assets) == 0 {
			continue
		}
		rank := 3
		switch {
		case strings.EqualFold(strings.TrimSpace(name), chain.Name):
			rank = 1
		case strings.EqualFold(strings.TrimSpace(name), chain.Slug):
			rank = 2
		}
		matches = append(matches, candidate{name: name, rank: rank, assets: assets})
	}

	if len(matches) == 0 {
		return nil, "", clierr.New(clierr.CodeUnsupported, "defillama has no chain asset data for requested chain")
	}
	sort.Slice(matches, func(i, j int) bool {
		if matches[i].rank != matches[j].rank {
			return matches[i].rank < matches[j].rank
		}
		return strings.Compare(strings.ToLower(matches[i].name), strings.ToLower(matches[j].name)) < 0
	})
	return matches[0].assets, matches[0].name, nil
}

func parseChainAssetBreakdown(raw json.RawMessage) (map[string]float64, error) {
	var categories map[string]chainAssetsCategory
	if err := json.Unmarshal(raw, &categories); err != nil {
		return nil, err
	}

	out := make(map[string]float64)
	for _, category := range categories {
		for symbol, value := range category.Breakdown {
			normSymbol := strings.ToUpper(strings.TrimSpace(symbol))
			if normSymbol == "" {
				continue
			}
			amount, ok := parseLooseFloat(value)
			if !ok || amount <= 0 {
				continue
			}
			out[normSymbol] += amount
		}
	}
	return out, nil
}

func parseLooseFloat(v any) (float64, bool) {
	switch t := v.(type) {
	case float64:
		if math.IsNaN(t) || math.IsInf(t, 0) {
			return 0, false
		}
		return t, true
	case json.Number:
		n, err := t.Float64()
		if err != nil || math.IsNaN(n) || math.IsInf(n, 0) {
			return 0, false
		}
		return n, true
	case string:
		value := strings.TrimSpace(t)
		if value == "" {
			return 0, false
		}
		n, err := strconv.ParseFloat(value, 64)
		if err != nil || math.IsNaN(n) || math.IsInf(n, 0) {
			return 0, false
		}
		return n, true
	case int:
		return float64(t), true
	case int64:
		return float64(t), true
	case int32:
		return float64(t), true
	case uint:
		return float64(t), true
	case uint64:
		return float64(t), true
	case uint32:
		return float64(t), true
	default:
		return 0, false
	}
}

func knownAssetID(chain id.Chain, symbol string) string {
	token, ok := id.KnownToken(chain.CAIP2, symbol)
	if !ok {
		return ""
	}
	return fmt.Sprintf("%s/erc20:%s", chain.CAIP2, strings.ToLower(token.Address))
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
