package model

import "time"

const EnvelopeVersion = "v1"

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
	Name          string   `json:"name"`
	Type          string   `json:"type"`
	RequiresKey   bool     `json:"requires_key"`
	Capabilities  []string `json:"capabilities"`
	KeyEnvVarName string   `json:"key_env_var,omitempty"`
}

type ChainTVL struct {
	Rank    int     `json:"rank"`
	Chain   string  `json:"chain"`
	ChainID string  `json:"chain_id"`
	TVLUSD  float64 `json:"tvl_usd"`
}

type ProtocolTVL struct {
	Rank     int     `json:"rank"`
	Protocol string  `json:"protocol"`
	Category string  `json:"category"`
	TVLUSD   float64 `json:"tvl_usd"`
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
	Protocol     string  `json:"protocol"`
	ChainID      string  `json:"chain_id"`
	AssetID      string  `json:"asset_id"`
	SupplyAPY    float64 `json:"supply_apy"`
	BorrowAPY    float64 `json:"borrow_apy"`
	TVLUSD       float64 `json:"tvl_usd"`
	LiquidityUSD float64 `json:"liquidity_usd"`
	SourceURL    string  `json:"source_url,omitempty"`
	FetchedAt    string  `json:"fetched_at"`
}

type LendRate struct {
	Protocol    string  `json:"protocol"`
	ChainID     string  `json:"chain_id"`
	AssetID     string  `json:"asset_id"`
	SupplyAPY   float64 `json:"supply_apy"`
	BorrowAPY   float64 `json:"borrow_apy"`
	Utilization float64 `json:"utilization"`
	SourceURL   string  `json:"source_url,omitempty"`
	FetchedAt   string  `json:"fetched_at"`
}

type AmountInfo struct {
	AmountBaseUnits string `json:"amount_base_units"`
	AmountDecimal   string `json:"amount_decimal"`
	Decimals        int    `json:"decimals"`
}

type BridgeQuote struct {
	Provider        string     `json:"provider"`
	FromChainID     string     `json:"from_chain_id"`
	ToChainID       string     `json:"to_chain_id"`
	FromAssetID     string     `json:"from_asset_id"`
	ToAssetID       string     `json:"to_asset_id"`
	InputAmount     AmountInfo `json:"input_amount"`
	EstimatedOut    AmountInfo `json:"estimated_out"`
	EstimatedFeeUSD float64    `json:"estimated_fee_usd"`
	EstimatedTimeS  int64      `json:"estimated_time_s"`
	Route           string     `json:"route"`
	SourceURL       string     `json:"source_url,omitempty"`
	FetchedAt       string     `json:"fetched_at"`
}

type SwapQuote struct {
	Provider        string     `json:"provider"`
	ChainID         string     `json:"chain_id"`
	FromAssetID     string     `json:"from_asset_id"`
	ToAssetID       string     `json:"to_asset_id"`
	InputAmount     AmountInfo `json:"input_amount"`
	EstimatedOut    AmountInfo `json:"estimated_out"`
	EstimatedGasUSD float64    `json:"estimated_gas_usd"`
	PriceImpactPct  float64    `json:"price_impact_pct"`
	Route           string     `json:"route"`
	SourceURL       string     `json:"source_url,omitempty"`
	FetchedAt       string     `json:"fetched_at"`
}

type YieldOpportunity struct {
	OpportunityID   string   `json:"opportunity_id"`
	Provider        string   `json:"provider"`
	Protocol        string   `json:"protocol"`
	ChainID         string   `json:"chain_id"`
	AssetID         string   `json:"asset_id"`
	Type            string   `json:"type"`
	APYBase         float64  `json:"apy_base"`
	APYReward       float64  `json:"apy_reward"`
	APYTotal        float64  `json:"apy_total"`
	TVLUSD          float64  `json:"tvl_usd"`
	LiquidityUSD    float64  `json:"liquidity_usd"`
	LockupDays      float64  `json:"lockup_days"`
	WithdrawalTerms string   `json:"withdrawal_terms"`
	RiskLevel       string   `json:"risk_level"`
	RiskReasons     []string `json:"risk_reasons,omitempty"`
	Score           float64  `json:"score"`
	SourceURL       string   `json:"source_url,omitempty"`
	FetchedAt       string   `json:"fetched_at"`
}
