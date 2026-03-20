package kamino

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

func TestLendMarketsRejectsNonSolanaChain(t *testing.T) {
	chain, _ := id.ParseChain("ethereum")
	asset, _ := id.ParseAsset("USDC", chain)
	c := New(httpx.New(2*time.Second, 0))
	_, err := c.LendMarkets(context.Background(), "kamino", chain, asset)
	if err == nil {
		t.Fatal("expected unsupported chain error")
	}
}

func TestLendMarketsAndRatesFromKaminoAPI(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v2/kamino-market", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"lendingMarket":"market-primary","name":"Main Market","isPrimary":true,"isCurated":false},
			{"lendingMarket":"market-jup","name":"JUP Market","isPrimary":false,"isCurated":false}
		]`))
	})
	mux.HandleFunc("/kamino-market/market-primary/reserves/metrics", func(w http.ResponseWriter, r *http.Request) {
		if got := r.URL.Query().Get("env"); got != "mainnet-beta" {
			t.Fatalf("expected env=mainnet-beta, got %q", got)
		}
		_, _ = w.Write([]byte(`[
			{
				"reserve":"reserve-usdc-main",
				"liquidityToken":"USDC",
				"liquidityTokenMint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
				"borrowApy":"0.045",
				"supplyApy":"0.032",
				"totalSupplyUsd":"1000000",
				"totalBorrowUsd":"500000"
			}
		]`))
	})
	mux.HandleFunc("/kamino-market/market-jup/reserves/metrics", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{
				"reserve":"reserve-usdc-jup",
				"liquidityToken":"USDC",
				"liquidityTokenMint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
				"borrowApy":"0.025",
				"supplyApy":"0.020",
				"totalSupplyUsd":"2000000",
				"totalBorrowUsd":"1000000"
			},
			{
				"reserve":"reserve-sol-jup",
				"liquidityToken":"SOL",
				"liquidityTokenMint":"So11111111111111111111111111111111111111112",
				"borrowApy":"0.01",
				"supplyApy":"0.005",
				"totalSupplyUsd":"100",
				"totalBorrowUsd":"1"
			}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	chain, _ := id.ParseChain("solana")
	asset, _ := id.ParseAsset("USDC", chain)
	c := New(httpx.New(2*time.Second, 0))
	c.baseURL = srv.URL

	markets, err := c.LendMarkets(context.Background(), "kamino", chain, asset)
	if err != nil {
		t.Fatalf("LendMarkets failed: %v", err)
	}
	if len(markets) != 2 {
		t.Fatalf("expected 2 usdc markets, got %d", len(markets))
	}
	if markets[0].TVLUSD != 2000000 {
		t.Fatalf("expected sorted market with highest tvl first, got %+v", markets)
	}
	if markets[0].SupplyAPY != 2 {
		t.Fatalf("expected APY in percentage points, got %+v", markets[0])
	}
	if markets[0].Provider != "kamino" || markets[0].ProviderNativeIDKind != model.NativeIDKindPoolID || markets[0].ProviderNativeID == "" {
		t.Fatalf("expected kamino provider id metadata, got %+v", markets[0])
	}

	rates, err := c.LendRates(context.Background(), "kamino", chain, asset)
	if err != nil {
		t.Fatalf("LendRates failed: %v", err)
	}
	if len(rates) != 2 {
		t.Fatalf("expected 2 usdc rates, got %d", len(rates))
	}
	if rates[0].Utilization != 0.5 {
		t.Fatalf("expected utilization 0.5, got %+v", rates[0])
	}
	if rates[0].Provider != "kamino" || rates[0].ProviderNativeIDKind != model.NativeIDKindPoolID || rates[0].ProviderNativeID == "" {
		t.Fatalf("expected kamino provider id metadata, got %+v", rates[0])
	}
}

func TestYieldOpportunitiesFiltersByAPYAndTVL(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v2/kamino-market", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"lendingMarket":"market-primary","name":"Main Market","isPrimary":true,"isCurated":false}
		]`))
	})
	mux.HandleFunc("/kamino-market/market-primary/reserves/metrics", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{
				"reserve":"reserve-1",
				"liquidityToken":"USDC",
				"liquidityTokenMint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
				"borrowApy":"0.03",
				"supplyApy":"0.04",
				"totalSupplyUsd":"1000000",
				"totalBorrowUsd":"400000"
			},
			{
				"reserve":"reserve-2",
				"liquidityToken":"USDC",
				"liquidityTokenMint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
				"borrowApy":"0.02",
				"supplyApy":"0.005",
				"totalSupplyUsd":"1000",
				"totalBorrowUsd":"200"
			}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	chain, _ := id.ParseChain("solana")
	asset, _ := id.ParseAsset("USDC", chain)
	c := New(httpx.New(2*time.Second, 0))
	c.baseURL = srv.URL

	opps, err := c.YieldOpportunities(context.Background(), providers.YieldRequest{
		Chain:     chain,
		Asset:     asset,
		Limit:     10,
		MinTVLUSD: 50000,
		MinAPY:    1,
		SortBy:    "apy_total",
	})
	if err != nil {
		t.Fatalf("YieldOpportunities failed: %v", err)
	}
	if len(opps) != 1 {
		t.Fatalf("expected 1 filtered opportunity, got %d", len(opps))
	}
	if opps[0].Provider != "kamino" || opps[0].Protocol != "kamino" {
		t.Fatalf("unexpected opportunity provider/protocol: %+v", opps[0])
	}
	if opps[0].ProviderNativeIDKind != model.NativeIDKindPoolID || opps[0].ProviderNativeID != "reserve-1" {
		t.Fatalf("expected kamino provider-native id metadata, got %+v", opps[0])
	}
	if opps[0].APYTotal != 4 {
		t.Fatalf("expected APY total 4, got %+v", opps[0])
	}
	if opps[0].LiquidityUSD != 600000 {
		t.Fatalf("expected liquidity_usd = totalSupplyUsd-totalBorrowUsd (600000), got %+v", opps[0])
	}
	if len(opps[0].BackingAssets) != 1 || opps[0].BackingAssets[0].SharePct != 100 {
		t.Fatalf("expected single backing asset at 100%%, got %+v", opps[0].BackingAssets)
	}
}

func TestLendMarketsPrefersMintMatchOverSymbol(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v2/kamino-market", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"lendingMarket":"market-primary","name":"Main Market","isPrimary":true,"isCurated":false}
		]`))
	})
	mux.HandleFunc("/kamino-market/market-primary/reserves/metrics", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{
				"reserve":"reserve-usdc-other",
				"liquidityToken":"USDC",
				"liquidityTokenMint":"USDCwNeWRongMint111111111111111111111111111",
				"borrowApy":"0.045",
				"supplyApy":"0.032",
				"totalSupplyUsd":"1000000",
				"totalBorrowUsd":"500000"
			}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	chain, _ := id.ParseChain("solana")
	asset, _ := id.ParseAsset("USDC", chain)
	c := New(httpx.New(2*time.Second, 0))
	c.baseURL = srv.URL

	_, err := c.LendMarkets(context.Background(), "kamino", chain, asset)
	if err == nil {
		t.Fatal("expected no market match due mint mismatch")
	}
}

func TestLendMarketsFailsWhenAnyMarketReserveFetchFails(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v2/kamino-market", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"lendingMarket":"market-good","name":"Good Market","isPrimary":true,"isCurated":false},
			{"lendingMarket":"market-fail","name":"Fail Market","isPrimary":false,"isCurated":false}
		]`))
	})
	mux.HandleFunc("/kamino-market/market-good/reserves/metrics", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{
				"reserve":"reserve-usdc-good",
				"liquidityToken":"USDC",
				"liquidityTokenMint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
				"borrowApy":"0.03",
				"supplyApy":"0.02",
				"totalSupplyUsd":"1000000",
				"totalBorrowUsd":"500000"
			}
		]`))
	})
	mux.HandleFunc("/kamino-market/market-fail/reserves/metrics", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusServiceUnavailable)
		_, _ = w.Write([]byte(`{"error":"temporary failure"}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	chain, _ := id.ParseChain("solana")
	asset, _ := id.ParseAsset("USDC", chain)
	c := New(httpx.New(2*time.Second, 0))
	c.baseURL = srv.URL

	_, err := c.LendMarkets(context.Background(), "kamino", chain, asset)
	if err == nil {
		t.Fatal("expected reserve fetch failure to fail command")
	}
}

func TestYieldHistoryFromSourceMarket(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/kamino-market/market-primary/reserves/reserve-1/metrics/history", func(w http.ResponseWriter, r *http.Request) {
		if got := r.URL.Query().Get("frequency"); got != "hour" {
			t.Fatalf("expected frequency=hour, got %q", got)
		}
		_, _ = w.Write([]byte(`{
			"reserve":"reserve-1",
			"history":[
				{
					"timestamp":"2026-02-25T00:00:00Z",
					"metrics":{"supplyInterestAPY":0.03,"depositTvl":"1000000"}
				},
				{
					"timestamp":"2026-02-25T01:00:00Z",
					"metrics":{"supplyInterestAPY":0.031,"depositTvl":"1100000"}
				}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0))
	c.baseURL = srv.URL
	c.now = func() time.Time { return time.Date(2026, 2, 26, 20, 0, 0, 0, time.UTC) }

	series, err := c.YieldHistory(context.Background(), providers.YieldHistoryRequest{
		Opportunity: model.YieldOpportunity{
			OpportunityID:        "opp-1",
			Provider:             "kamino",
			Protocol:             "kamino",
			ChainID:              "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
			AssetID:              "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp/token:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
			ProviderNativeID:     "reserve-1",
			ProviderNativeIDKind: model.NativeIDKindPoolID,
			SourceURL:            "https://app.kamino.finance/lending/market-primary",
		},
		StartTime: time.Date(2026, 2, 25, 0, 0, 0, 0, time.UTC),
		EndTime:   time.Date(2026, 2, 25, 2, 0, 0, 0, time.UTC),
		Interval:  providers.YieldHistoryIntervalHour,
		Metrics: []providers.YieldHistoryMetric{
			providers.YieldHistoryMetricAPYTotal,
			providers.YieldHistoryMetricTVLUSD,
		},
	})
	if err != nil {
		t.Fatalf("YieldHistory failed: %v", err)
	}
	if len(series) != 2 {
		t.Fatalf("expected two series, got %+v", series)
	}
	byMetric := map[string]model.YieldHistorySeries{}
	for _, item := range series {
		byMetric[item.Metric] = item
	}
	apy := byMetric[string(providers.YieldHistoryMetricAPYTotal)]
	if len(apy.Points) != 2 || apy.Points[0].Value != 3 {
		t.Fatalf("unexpected apy points: %+v", apy.Points)
	}
	tvl := byMetric[string(providers.YieldHistoryMetricTVLUSD)]
	if len(tvl.Points) != 2 || tvl.Points[1].Value != 1100000 {
		t.Fatalf("unexpected tvl points: %+v", tvl.Points)
	}
}

func TestYieldHistoryResolvesMarketFromReserve(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v2/kamino-market", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"lendingMarket":"market-primary","name":"Main Market","isPrimary":true,"isCurated":false}
		]`))
	})
	mux.HandleFunc("/kamino-market/market-primary/reserves/metrics", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{
				"reserve":"reserve-1",
				"liquidityToken":"USDC",
				"liquidityTokenMint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
				"borrowApy":"0.03",
				"supplyApy":"0.04",
				"totalSupplyUsd":"1000000",
				"totalBorrowUsd":"400000"
			}
		]`))
	})
	mux.HandleFunc("/kamino-market/market-primary/reserves/reserve-1/metrics/history", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"reserve":"reserve-1",
			"history":[
				{"timestamp":"2026-02-25T00:00:00Z","metrics":{"supplyInterestAPY":0.03,"depositTvl":"1000000"}}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0))
	c.baseURL = srv.URL
	c.now = func() time.Time { return time.Date(2026, 2, 26, 20, 0, 0, 0, time.UTC) }

	series, err := c.YieldHistory(context.Background(), providers.YieldHistoryRequest{
		Opportunity: model.YieldOpportunity{
			OpportunityID:        "opp-1",
			Provider:             "kamino",
			Protocol:             "kamino",
			ChainID:              "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
			AssetID:              "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp/token:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
			ProviderNativeID:     "reserve-1",
			ProviderNativeIDKind: model.NativeIDKindPoolID,
		},
		StartTime: time.Date(2026, 2, 25, 0, 0, 0, 0, time.UTC),
		EndTime:   time.Date(2026, 2, 25, 2, 0, 0, 0, time.UTC),
		Interval:  providers.YieldHistoryIntervalDay,
		Metrics:   []providers.YieldHistoryMetric{providers.YieldHistoryMetricAPYTotal},
	})
	if err != nil {
		t.Fatalf("YieldHistory failed: %v", err)
	}
	if len(series) != 1 || len(series[0].Points) != 1 {
		t.Fatalf("unexpected series: %+v", series)
	}
}
