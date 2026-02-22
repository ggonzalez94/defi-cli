package defillama

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

func TestChainsTopSortsDescending(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v2/chains", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[ {"name":"B","tvl":2}, {"name":"A","tvl":3} ]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL
	items, err := c.ChainsTop(context.Background(), 2)
	if err != nil {
		t.Fatalf("ChainsTop failed: %v", err)
	}
	if len(items) != 2 || items[0].Chain != "A" {
		t.Fatalf("unexpected ordering: %+v", items)
	}
}

func TestYieldScoreAndSortDeterministic(t *testing.T) {
	opps := []model.YieldOpportunity{
		{OpportunityID: "b", Score: 50, APYTotal: 10, TVLUSD: 100},
		{OpportunityID: "a", Score: 50, APYTotal: 10, TVLUSD: 100},
	}
	sortYield(opps, "score")
	if opps[0].OpportunityID != "a" {
		t.Fatalf("expected lexicographic tie-break, got %+v", opps)
	}

	score := scoreOpportunity(20, 1_000_000, 700_000, "low")
	if score <= 0 || score > 100 {
		t.Fatalf("score out of range: %f", score)
	}
}

func TestYieldOpportunitiesFilters(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/pools", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"status":"success",
			"data":[
				{"pool":"p1","chain":"Base","project":"aave-v3","symbol":"USDC","apy":5,"apyBase":4,"apyReward":1,"tvlUsd":1000000,"ilRisk":"no","stablecoin":true},
				{"pool":"p2","chain":"Base","project":"curve","symbol":"USDC","apy":2,"tvlUsd":10000,"ilRisk":"yes","stablecoin":false}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	chain, _ := id.ParseChain("base")
	asset, _ := id.ParseAsset("USDC", chain)
	c := New(httpx.New(2*time.Second, 0), "")
	c.yieldsBase = srv.URL
	got, err := c.YieldOpportunities(context.Background(), providers.YieldRequest{
		Chain:     chain,
		Asset:     asset,
		Limit:     20,
		MinTVLUSD: 50_000,
		MinAPY:    0,
		MaxRisk:   "high",
		SortBy:    "score",
	})
	if err != nil {
		t.Fatalf("YieldOpportunities failed: %v", err)
	}
	if len(got) != 1 {
		t.Fatalf("expected one result, got %d (%v)", len(got), fmt.Sprintf("%+v", got))
	}
	if got[0].Protocol != "aave-v3" {
		t.Fatalf("unexpected protocol: %+v", got[0])
	}
}

func TestProtocolsCategoriesAggregation(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/protocols", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"name":"Aave V3","category":"Lending","tvl":10000},
			{"name":"Morpho","category":"Lending","tvl":5000},
			{"name":"Uniswap","category":"Dexes","tvl":20000},
			{"name":"Curve","category":"Dexes","tvl":8000},
			{"name":"Lido","category":"Liquid Staking","tvl":30000},
			{"name":"Empty","category":"","tvl":100},
			{"name":"Spaces","category":"  ","tvl":50}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL
	cats, err := c.ProtocolsCategories(context.Background())
	if err != nil {
		t.Fatalf("ProtocolsCategories failed: %v", err)
	}

	if len(cats) != 3 {
		t.Fatalf("expected 3 categories, got %d: %+v", len(cats), cats)
	}

	// Sorted by TVL descending: Liquid Staking (30000), Dexes (28000), Lending (15000)
	if cats[0].Name != "Liquid Staking" || cats[0].Protocols != 1 || cats[0].TVLUSD != 30000 {
		t.Fatalf("unexpected first category: %+v", cats[0])
	}
	if cats[1].Name != "Dexes" || cats[1].Protocols != 2 || cats[1].TVLUSD != 28000 {
		t.Fatalf("unexpected second category: %+v", cats[1])
	}
	if cats[2].Name != "Lending" || cats[2].Protocols != 2 || cats[2].TVLUSD != 15000 {
		t.Fatalf("unexpected third category: %+v", cats[2])
	}
}

func TestProtocolsCategoriesEmpty(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/protocols", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL
	cats, err := c.ProtocolsCategories(context.Background())
	if err != nil {
		t.Fatalf("ProtocolsCategories failed: %v", err)
	}
	if len(cats) != 0 {
		t.Fatalf("expected 0 categories, got %d", len(cats))
	}
}

func TestProtocolsCategoriesDeterministicTieBreak(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/protocols", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"name":"P1","category":"zeta","tvl":1000},
			{"name":"P2","category":"Alpha","tvl":1000},
			{"name":"P3","category":"alpha","tvl":1000}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL
	cats, err := c.ProtocolsCategories(context.Background())
	if err != nil {
		t.Fatalf("ProtocolsCategories failed: %v", err)
	}
	if len(cats) != 2 {
		t.Fatalf("expected 2 categories, got %d", len(cats))
	}
	// TVL is tied at 1000; category with more protocols should come first.
	if cats[0].Name != "Alpha" || cats[0].Protocols != 2 {
		t.Fatalf("unexpected first category: %+v", cats[0])
	}
	if cats[1].Name != "zeta" || cats[1].Protocols != 1 {
		t.Fatalf("unexpected second category: %+v", cats[1])
	}
}

func TestProtocolMatcherSupportsKamino(t *testing.T) {
	got := protocolMatcher("kamino")
	if len(got) == 0 {
		t.Fatal("expected kamino protocol matchers")
	}
	if got[0] != "kamino" {
		t.Fatalf("unexpected first matcher: %#v", got)
	}
}

func TestListBridgesRequiresAPIKey(t *testing.T) {
	c := New(httpx.New(2*time.Second, 0), "")
	_, err := c.ListBridges(context.Background(), providers.BridgeListRequest{Limit: 5, IncludeChains: true})
	if err == nil {
		t.Fatal("expected API key error")
	}
}

func TestListBridgesSortsAndLimits(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/test-key/bridges/bridges", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"bridges":[
				{"id":1,"name":"b","displayName":"Bridge B","slug":"bridge-b","last24hVolume":150,"weeklyVolume":1000,"monthlyVolume":5000,"chains":["Base","Ethereum"]},
				{"id":2,"name":"a","displayName":"Bridge A","slug":"bridge-a","last24hVolume":250,"weeklyVolume":900,"monthlyVolume":6000,"chains":["Ethereum","Base"]},
				{"id":3,"name":"c","displayName":"Bridge C","slug":"bridge-c","last24hVolume":90,"weeklyVolume":700,"monthlyVolume":2000,"chains":["Arbitrum"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "test-key")
	c.bridgeBaseURL = srv.URL
	got, err := c.ListBridges(context.Background(), providers.BridgeListRequest{Limit: 2, IncludeChains: true})
	if err != nil {
		t.Fatalf("ListBridges failed: %v", err)
	}
	if len(got) != 2 {
		t.Fatalf("expected 2 items, got %d", len(got))
	}
	if got[0].BridgeID != 2 || got[1].BridgeID != 1 {
		t.Fatalf("unexpected ordering: %+v", got)
	}
	if len(got[0].Chains) != 2 || got[0].Chains[0] != "Base" || got[0].Chains[1] != "Ethereum" {
		t.Fatalf("expected deterministic chain ordering, got %+v", got[0].Chains)
	}
}

func TestBridgeDetailsBySlugIncludesBreakdown(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/test-key/bridges/bridges", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"bridges":[
				{"id":84,"name":"layerzero","displayName":"LayerZero","slug":"layerzero"}
			]
		}`))
	})
	mux.HandleFunc("/test-key/bridges/bridge/84", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"id":84,
			"name":"layerzero",
			"displayName":"LayerZero",
			"last24hVolume":123.45,
			"weeklyVolume":999.1,
			"monthlyVolume":4200.7,
			"lastHourlyTxs":{"deposits":1,"withdrawals":2},
			"currentDayTxs":{"deposits":0,"withdrawals":0},
			"prevDayTxs":{"deposits":10,"withdrawals":20},
			"dayBeforeLastTxs":{"deposits":7,"withdrawals":8},
			"weeklyTxs":{"deposits":100,"withdrawals":200},
			"monthlyTxs":{"deposits":300,"withdrawals":400},
			"chainBreakdown":{
				"Base":{
					"last24hVolume":80,
					"weeklyVolume":600,
					"monthlyVolume":2000,
					"lastHourlyTxs":{"deposits":1,"withdrawals":1},
					"currentDayTxs":{"deposits":0,"withdrawals":0},
					"prevDayTxs":{"deposits":5,"withdrawals":6},
					"dayBeforeLastTxs":{"deposits":2,"withdrawals":3},
					"weeklyTxs":{"deposits":50,"withdrawals":60},
					"monthlyTxs":{"deposits":100,"withdrawals":110}
				},
				"Arbitrum":{
					"last24hVolume":40,
					"weeklyVolume":300,
					"monthlyVolume":1500,
					"lastHourlyTxs":{"deposits":0,"withdrawals":1},
					"currentDayTxs":{"deposits":0,"withdrawals":0},
					"prevDayTxs":{"deposits":2,"withdrawals":1},
					"dayBeforeLastTxs":{"deposits":2,"withdrawals":1},
					"weeklyTxs":{"deposits":20,"withdrawals":10},
					"monthlyTxs":{"deposits":30,"withdrawals":20}
				}
			}
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "test-key")
	c.bridgeBaseURL = srv.URL
	got, err := c.BridgeDetails(context.Background(), providers.BridgeDetailsRequest{
		Bridge:                "layerzero",
		IncludeChainBreakdown: true,
	})
	if err != nil {
		t.Fatalf("BridgeDetails failed: %v", err)
	}
	if got.BridgeID != 84 || got.Name != "layerzero" {
		t.Fatalf("unexpected bridge details: %+v", got)
	}
	if len(got.ChainBreakdown) != 2 {
		t.Fatalf("expected chain breakdown entries, got %+v", got.ChainBreakdown)
	}
	if got.ChainBreakdown[0].Chain != "Base" {
		t.Fatalf("expected highest-volume chain first, got %+v", got.ChainBreakdown)
	}
	if got.ChainBreakdown[0].ChainID != "eip155:8453" {
		t.Fatalf("expected CAIP chain id for Base, got %+v", got.ChainBreakdown[0])
	}
}
