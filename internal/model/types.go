package model

import "time"

const EnvelopeVersion = "v1"

const (
	NativeIDKindCompositeMarketAsset = "composite_market_asset"
	NativeIDKindMarketID             = "market_id"
	NativeIDKindPoolID               = "pool_id"
)

type Envelope struct {
	Version  string       `json:"version"`
	Success  bool         `json:"success"`
	Data     any          `json:"data,omitempty"`
	Error    *ErrorBody   `json:"error"`
	Warnings []string     `json:"warnings,omitempty"`
	Meta     EnvelopeMeta `json:"meta"`
}

type ErrorBody struct {
	Code    int    `json:"code"`
	Type    string `json:"type"`
	Message string `json:"message"`
}

type EnvelopeMeta struct {
	RequestID string           `json:"request_id"`
	Timestamp time.Time        `json:"timestamp"`
	Command   string           `json:"command"`
	Providers []ProviderStatus `json:"providers,omitempty"`
	Cache     CacheStatus      `json:"cache"`
	Partial   bool             `json:"partial"`
}

type ProviderStatus struct {
	Name      string `json:"name"`
	Status    string `json:"status"`
	LatencyMS int64  `json:"latency_ms"`
}

type CacheStatus struct {
	Status string `json:"status"`
	AgeMS  int64  `json:"age_ms"`
	Stale  bool   `json:"stale"`
}

type ProviderInfo struct {
	Name           string                   `json:"name"`
	Type           string                   `json:"type"`
	RequiresKey    bool                     `json:"requires_key"`
	Capabilities   []string                 `json:"capabilities"`
	KeyEnvVarName  string                   `json:"key_env_var,omitempty"`
	CapabilityAuth []ProviderCapabilityAuth `json:"capability_auth,omitempty"`
}

type ProviderCapabilityAuth struct {
	Capability  string `json:"capability"`
	KeyEnvVar   string `json:"key_env_var"`
	Description string `json:"description,omitempty"`
}

type ChainTVL struct {
	Rank    int     `json:"rank"`
	Chain   string  `json:"chain"`
	ChainID string  `json:"chain_id"`
	TVLUSD  float64 `json:"tvl_usd"`
}

type ChainAssetTVL struct {
	Rank    int     `json:"rank"`
	Chain   string  `json:"chain"`
	ChainID string  `json:"chain_id"`
	Asset   string  `json:"asset"`
	AssetID string  `json:"asset_id"`
	TVLUSD  float64 `json:"tvl_usd"`
}

type ProtocolTVL struct {
	Rank     int     `json:"rank"`
	Protocol string  `json:"protocol"`
	Category string  `json:"category"`
	TVLUSD   float64 `json:"tvl_usd"`
}

type ProtocolCategory struct {
	Name      string  `json:"name"`
	Protocols int     `json:"protocols"`
	TVLUSD    float64 `json:"tvl_usd"`
}

type AssetResolution struct {
	Input       string `json:"input"`
	ChainID     string `json:"chain_id"`
	Symbol      string `json:"symbol"`
	AssetID     string `json:"asset_id"`
	Address     string `json:"address"`
	Decimals    int    `json:"decimals"`
	ResolvedBy  string `json:"resolved_by"`
	Unambiguous bool   `json:"unambiguous"`
}

type LendMarket struct {
	Protocol             string  `json:"protocol"`
	Provider             string  `json:"provider"`
	ChainID              string  `json:"chain_id"`
	AssetID              string  `json:"asset_id"`
	ProviderNativeID     string  `json:"provider_native_id,omitempty"`
	ProviderNativeIDKind string  `json:"provider_native_id_kind,omitempty"`
	SupplyAPY            float64 `json:"supply_apy"`
	BorrowAPY            float64 `json:"borrow_apy"`
	TVLUSD               float64 `json:"tvl_usd"`
	LiquidityUSD         float64 `json:"liquidity_usd"`
	SourceURL            string  `json:"source_url,omitempty"`
	FetchedAt            string  `json:"fetched_at"`
}

type LendRate struct {
	Protocol             string  `json:"protocol"`
	Provider             string  `json:"provider"`
	ChainID              string  `json:"chain_id"`
	AssetID              string  `json:"asset_id"`
	ProviderNativeID     string  `json:"provider_native_id,omitempty"`
	ProviderNativeIDKind string  `json:"provider_native_id_kind,omitempty"`
	SupplyAPY            float64 `json:"supply_apy"`
	BorrowAPY            float64 `json:"borrow_apy"`
	Utilization          float64 `json:"utilization"`
	SourceURL            string  `json:"source_url,omitempty"`
	FetchedAt            string  `json:"fetched_at"`
}

type AmountInfo struct {
	AmountBaseUnits string `json:"amount_base_units"`
	AmountDecimal   string `json:"amount_decimal"`
	Decimals        int    `json:"decimals"`
}

type FeeAmount struct {
	AmountBaseUnits string  `json:"amount_base_units,omitempty"`
	AmountDecimal   string  `json:"amount_decimal,omitempty"`
	AmountUSD       float64 `json:"amount_usd,omitempty"`
}

type BridgeFeeBreakdown struct {
	LPFee                     *FeeAmount `json:"lp_fee,omitempty"`
	RelayerFee                *FeeAmount `json:"relayer_fee,omitempty"`
	GasFee                    *FeeAmount `json:"gas_fee,omitempty"`
	TotalFeeBaseUnits         string     `json:"total_fee_base_units,omitempty"`
	TotalFeeDecimal           string     `json:"total_fee_decimal,omitempty"`
	TotalFeeUSD               float64    `json:"total_fee_usd,omitempty"`
	ConsistentWithAmountDelta *bool      `json:"consistent_with_amount_delta,omitempty"`
}

type BridgeVolumes struct {
	LastHourlyUSD float64 `json:"last_hourly_usd"`
	Last24hUSD    float64 `json:"last_24h_usd"`
	LastDailyUSD  float64 `json:"last_daily_usd"`
	PrevDayUSD    float64 `json:"prev_day_usd"`
	Prev2DayUSD   float64 `json:"prev_2d_usd"`
	WeeklyUSD     float64 `json:"weekly_usd"`
	MonthlyUSD    float64 `json:"monthly_usd"`
}

type BridgeTxCounts struct {
	Deposits    int64 `json:"deposits"`
	Withdrawals int64 `json:"withdrawals"`
}

type BridgeTransactions struct {
	LastHourly BridgeTxCounts `json:"last_hourly"`
	CurrentDay BridgeTxCounts `json:"current_day"`
	PrevDay    BridgeTxCounts `json:"prev_day"`
	Prev2Day   BridgeTxCounts `json:"prev_2d"`
	Weekly     BridgeTxCounts `json:"weekly"`
	Monthly    BridgeTxCounts `json:"monthly"`
}

type BridgeSummary struct {
	BridgeID         int           `json:"bridge_id"`
	Name             string        `json:"name"`
	DisplayName      string        `json:"display_name"`
	Slug             string        `json:"slug,omitempty"`
	DestinationChain string        `json:"destination_chain,omitempty"`
	URL              string        `json:"url,omitempty"`
	Chains           []string      `json:"chains,omitempty"`
	Volumes          BridgeVolumes `json:"volumes"`
	LastUpdatedUNIX  int64         `json:"last_updated_unix"`
	FetchedAt        string        `json:"fetched_at"`
}

type BridgeChainDetails struct {
	Chain        string             `json:"chain"`
	ChainID      string             `json:"chain_id,omitempty"`
	Volumes      BridgeVolumes      `json:"volumes"`
	Transactions BridgeTransactions `json:"transactions"`
}

type BridgeDetails struct {
	BridgeID         int                  `json:"bridge_id"`
	Name             string               `json:"name"`
	DisplayName      string               `json:"display_name"`
	DestinationChain string               `json:"destination_chain,omitempty"`
	Volumes          BridgeVolumes        `json:"volumes"`
	Transactions     BridgeTransactions   `json:"transactions"`
	ChainBreakdown   []BridgeChainDetails `json:"chain_breakdown,omitempty"`
	LastUpdatedUNIX  int64                `json:"last_updated_unix"`
	FetchedAt        string               `json:"fetched_at"`
}

type BridgeQuote struct {
	Provider                   string              `json:"provider"`
	FromChainID                string              `json:"from_chain_id"`
	ToChainID                  string              `json:"to_chain_id"`
	FromAssetID                string              `json:"from_asset_id"`
	ToAssetID                  string              `json:"to_asset_id"`
	InputAmount                AmountInfo          `json:"input_amount"`
	FromAmountForGas           string              `json:"from_amount_for_gas,omitempty"`
	EstimatedDestinationNative *AmountInfo         `json:"estimated_destination_native,omitempty"`
	EstimatedOut               AmountInfo          `json:"estimated_out"`
	EstimatedFeeUSD            float64             `json:"estimated_fee_usd"`
	FeeBreakdown               *BridgeFeeBreakdown `json:"fee_breakdown,omitempty"`
	EstimatedTimeS             int64               `json:"estimated_time_s"`
	Route                      string              `json:"route"`
	SourceURL                  string              `json:"source_url,omitempty"`
	FetchedAt                  string              `json:"fetched_at"`
}

type SwapQuote struct {
	Provider        string     `json:"provider"`
	ChainID         string     `json:"chain_id"`
	FromAssetID     string     `json:"from_asset_id"`
	ToAssetID       string     `json:"to_asset_id"`
	TradeType       string     `json:"trade_type"`
	InputAmount     AmountInfo `json:"input_amount"`
	EstimatedOut    AmountInfo `json:"estimated_out"`
	EstimatedGasUSD float64    `json:"estimated_gas_usd"`
	PriceImpactPct  float64    `json:"price_impact_pct"`
	Route           string     `json:"route"`
	SourceURL       string     `json:"source_url,omitempty"`
	FetchedAt       string     `json:"fetched_at"`
}

type YieldOpportunity struct {
	OpportunityID        string   `json:"opportunity_id"`
	Provider             string   `json:"provider"`
	Protocol             string   `json:"protocol"`
	ChainID              string   `json:"chain_id"`
	AssetID              string   `json:"asset_id"`
	ProviderNativeID     string   `json:"provider_native_id,omitempty"`
	ProviderNativeIDKind string   `json:"provider_native_id_kind,omitempty"`
	Type                 string   `json:"type"`
	APYBase              float64  `json:"apy_base"`
	APYReward            float64  `json:"apy_reward"`
	APYTotal             float64  `json:"apy_total"`
	TVLUSD               float64  `json:"tvl_usd"`
	LiquidityUSD         float64  `json:"liquidity_usd"`
	LockupDays           float64  `json:"lockup_days"`
	WithdrawalTerms      string   `json:"withdrawal_terms"`
	RiskLevel            string   `json:"risk_level"`
	RiskReasons          []string `json:"risk_reasons,omitempty"`
	Score                float64  `json:"score"`
	SourceURL            string   `json:"source_url,omitempty"`
	FetchedAt            string   `json:"fetched_at"`
}
