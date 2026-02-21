package defillama

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/gustavo/defi-cli/internal/httpx"
	"github.com/gustavo/defi-cli/internal/id"
	"github.com/gustavo/defi-cli/internal/model"
	"github.com/gustavo/defi-cli/internal/providers"
)

func TestChainsTopSortsDescending(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v2/chains", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[ {"name":"B","tvl":2}, {"name":"A","tvl":3} ]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0))
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
	c := New(httpx.New(2*time.Second, 0))
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
