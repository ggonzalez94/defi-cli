package defillama

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/ggonzalez94/defi-cli/internal/providers/yieldutil"
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

func TestChainsAssetsRequiresAPIKey(t *testing.T) {
	chain, _ := id.ParseChain("ethereum")
	c := New(httpx.New(2*time.Second, 0), "")
	_, err := c.ChainsAssets(context.Background(), chain, id.Asset{}, 20)
	if err == nil {
		t.Fatal("expected API key error")
	}
	if code := clierr.ExitCode(err); code != int(clierr.CodeAuth) {
		t.Fatalf("expected auth exit code, got %d err=%v", code, err)
	}
}

func TestChainsAssetsSortsAggregatesAndLimits(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/test-key/api/chainAssets", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"Ethereum":{
				"canonical":{"total":"250.5","breakdown":{"USDC":"100","USDT":"150.5"}},
				"native":{"total":"50","breakdown":{"ETH":"50"}},
				"thirdParty":{"total":"205","breakdown":{"WBTC":"80","USDC":"125"}}
			},
			"Arbitrum":{"canonical":{"total":"10","breakdown":{"USDC":"10"}}},
			"timestamp":1752843956
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	chain, _ := id.ParseChain("ethereum")
	c := New(httpx.New(2*time.Second, 0), "test-key")
	c.bridgeBaseURL = srv.URL

	items, err := c.ChainsAssets(context.Background(), chain, id.Asset{}, 3)
	if err != nil {
		t.Fatalf("ChainsAssets failed: %v", err)
	}
	if len(items) != 3 {
		t.Fatalf("expected 3 results, got %d", len(items))
	}
	if items[0].Asset != "USDC" || items[0].TVLUSD != 225 {
		t.Fatalf("unexpected first item: %+v", items[0])
	}
	if items[1].Asset != "USDT" || items[1].TVLUSD != 150.5 {
		t.Fatalf("unexpected second item: %+v", items[1])
	}
	if items[2].Asset != "WBTC" || items[2].TVLUSD != 80 {
		t.Fatalf("unexpected third item: %+v", items[2])
	}
	if items[0].Rank != 1 || items[1].Rank != 2 || items[2].Rank != 3 {
		t.Fatalf("expected sequential rank values, got %+v", items)
	}
	if items[0].Chain != "Ethereum" || items[0].ChainID != "eip155:1" {
		t.Fatalf("unexpected chain normalization: %+v", items[0])
	}
	if !strings.HasPrefix(items[0].AssetID, "eip155:1/erc20:") {
		t.Fatalf("expected known asset ID for USDC, got %q", items[0].AssetID)
	}
}

func TestChainsAssetsFiltersByAsset(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/test-key/api/chainAssets", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"Ethereum":{
				"canonical":{"total":"250.5","breakdown":{"USDC":"100","USDT":"150.5"}},
				"native":{"total":"50","breakdown":{"ETH":"50"}},
				"thirdParty":{"total":"205","breakdown":{"WBTC":"80","USDC":"125"}}
			},
			"timestamp":1752843956
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	chain, _ := id.ParseChain("ethereum")
	asset, _ := id.ParseAsset("USDC", chain)
	c := New(httpx.New(2*time.Second, 0), "test-key")
	c.bridgeBaseURL = srv.URL

	items, err := c.ChainsAssets(context.Background(), chain, asset, 20)
	if err != nil {
		t.Fatalf("ChainsAssets failed: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected one result, got %d", len(items))
	}
	if items[0].Asset != "USDC" || items[0].TVLUSD != 225 {
		t.Fatalf("unexpected filtered result: %+v", items[0])
	}
	if items[0].AssetID != asset.AssetID {
		t.Fatalf("expected canonical asset id %s, got %s", asset.AssetID, items[0].AssetID)
	}
}

func TestYieldSortDeterministic(t *testing.T) {
	opps := []model.YieldOpportunity{
		{OpportunityID: "b", APYTotal: 10, TVLUSD: 100, LiquidityUSD: 50},
		{OpportunityID: "a", APYTotal: 10, TVLUSD: 100, LiquidityUSD: 50},
	}
	yieldutil.Sort(opps, "apy_total")
	if opps[0].OpportunityID != "a" {
		t.Fatalf("expected lexicographic tie-break, got %+v", opps)
	}
}

func TestProtocolsTopSortsDescending(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/protocols", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"name":"Aave","category":"Lending","tvl":10000,"chains":["Ethereum","Polygon"],"chainTvls":{"Ethereum":7000,"Polygon":3000}},
			{"name":"Lido","category":"Liquid Staking","tvl":30000,"chains":["Ethereum"],"chainTvls":{"Ethereum":30000}},
			{"name":"Uniswap","category":"Dexes","tvl":20000,"chains":["Ethereum","Arbitrum","Base"],"chainTvls":{"Ethereum":12000,"Arbitrum":5000,"Base":3000}}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL
	items, err := c.ProtocolsTop(context.Background(), "", "", 0)
	if err != nil {
		t.Fatalf("ProtocolsTop failed: %v", err)
	}
	if len(items) != 3 {
		t.Fatalf("expected 3 items, got %d", len(items))
	}
	if items[0].Protocol != "Lido" || items[0].Rank != 1 || items[0].TVLUSD != 30000 {
		t.Fatalf("expected Lido first with TVL 30000, got %+v", items[0])
	}
	if items[0].Chains != 1 {
		t.Fatalf("expected 1 chain for Lido, got %d", items[0].Chains)
	}
	if items[1].Protocol != "Uniswap" || items[1].Chains != 3 {
		t.Fatalf("expected Uniswap second with 3 chains, got %+v", items[1])
	}
}

func TestProtocolsTopFiltersByChain(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/protocols", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"name":"Aave","category":"Lending","tvl":10000,"chains":["Ethereum","Polygon"],"chainTvls":{"Ethereum":7000,"Polygon":3000,"Ethereum-staking":500}},
			{"name":"Lido","category":"Liquid Staking","tvl":30000,"chains":["Ethereum"],"chainTvls":{"Ethereum":30000}},
			{"name":"PancakeSwap","category":"Dexes","tvl":8000,"chains":["BSC"],"chainTvls":{"BSC":8000}},
			{"name":"Uniswap","category":"Dexes","tvl":20000,"chains":["Ethereum","Arbitrum","Base"],"chainTvls":{"Ethereum":12000,"Arbitrum":5000,"Base":3000}}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsTop(context.Background(), "", "Ethereum", 0)
	if err != nil {
		t.Fatalf("ProtocolsTop failed: %v", err)
	}
	if len(items) != 3 {
		t.Fatalf("expected 3 Ethereum items, got %d", len(items))
	}
	// Sorted by chain-specific TVL: Lido (30000), Uniswap (12000), Aave (7000)
	if items[0].Protocol != "Lido" || items[0].TVLUSD != 30000 {
		t.Fatalf("expected Lido first with chain TVL 30000, got %+v", items[0])
	}
	if items[1].Protocol != "Uniswap" || items[1].TVLUSD != 12000 {
		t.Fatalf("expected Uniswap second with chain TVL 12000, got %+v", items[1])
	}
	if items[2].Protocol != "Aave" || items[2].TVLUSD != 7000 {
		t.Fatalf("expected Aave third with chain TVL 7000, got %+v", items[2])
	}
}

func TestProtocolsTopChainAndCategoryFilter(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/protocols", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"name":"Aave","category":"Lending","tvl":10000,"chains":["Ethereum","Polygon"],"chainTvls":{"Ethereum":7000,"Polygon":3000}},
			{"name":"Lido","category":"Liquid Staking","tvl":30000,"chains":["Ethereum"],"chainTvls":{"Ethereum":30000}},
			{"name":"Morpho","category":"Lending","tvl":5000,"chains":["Ethereum","Base"],"chainTvls":{"Ethereum":4000,"Base":1000}}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsTop(context.Background(), "Lending", "Ethereum", 0)
	if err != nil {
		t.Fatalf("ProtocolsTop failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 Lending+Ethereum items, got %d", len(items))
	}
	if items[0].Protocol != "Aave" || items[0].TVLUSD != 7000 {
		t.Fatalf("expected Aave first with chain TVL 7000, got %+v", items[0])
	}
	if items[1].Protocol != "Morpho" || items[1].TVLUSD != 4000 {
		t.Fatalf("expected Morpho second with chain TVL 4000, got %+v", items[1])
	}
}

func TestProtocolsTopChainFilterCaseInsensitive(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/protocols", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"name":"Aave","category":"Lending","tvl":10000,"chains":["Ethereum"],"chainTvls":{"Ethereum":10000}},
			{"name":"PancakeSwap","category":"Dexes","tvl":8000,"chains":["BSC"],"chainTvls":{"BSC":8000}}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsTop(context.Background(), "", "ethereum", 0)
	if err != nil {
		t.Fatalf("ProtocolsTop failed: %v", err)
	}
	if len(items) != 1 || items[0].Protocol != "Aave" {
		t.Fatalf("expected only Aave for 'ethereum' filter, got %+v", items)
	}
}

func TestProtocolsTopChainFallbackToTotalTVL(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/protocols", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"name":"OldProtocol","category":"Lending","tvl":5000,"chains":["Ethereum"],"chainTvls":{}}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsTop(context.Background(), "", "Ethereum", 0)
	if err != nil {
		t.Fatalf("ProtocolsTop failed: %v", err)
	}
	if len(items) != 1 || items[0].TVLUSD != 5000 {
		t.Fatalf("expected fallback to total TVL 5000, got %+v", items)
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

func TestStablecoinsTopSortsAndLimits(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/stablecoins", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"peggedAssets":[
				{"name":"Tether","symbol":"USDT","pegType":"peggedUSD","pegMechanism":"fiat-backed",
				 "circulating":{"peggedUSD":120000000000},"circulatingPrevDay":{"peggedUSD":119500000000},
				 "circulatingPrevWeek":{"peggedUSD":118000000000},"circulatingPrevMonth":{"peggedUSD":115000000000},
				 "chains":["Ethereum","Tron","BSC","Arbitrum","Solana"],"price":1.0001},
				{"name":"USD Coin","symbol":"USDC","pegType":"peggedUSD","pegMechanism":"fiat-backed",
				 "circulating":{"peggedUSD":55000000000},"circulatingPrevDay":{"peggedUSD":54800000000},
				 "circulatingPrevWeek":{"peggedUSD":54000000000},"circulatingPrevMonth":{"peggedUSD":52000000000},
				 "chains":["Ethereum","Base","Solana"],"price":0.9999},
				{"name":"Dai","symbol":"DAI","pegType":"peggedUSD","pegMechanism":"crypto-backed",
				 "circulating":{"peggedUSD":5000000000},"circulatingPrevDay":{"peggedUSD":4990000000},
				 "circulatingPrevWeek":{"peggedUSD":4900000000},"circulatingPrevMonth":{"peggedUSD":4800000000},
				 "chains":["Ethereum","Polygon"],"price":1.0},
				{"name":"STASIS EURO","symbol":"EURS","pegType":"peggedEUR","pegMechanism":"fiat-backed",
				 "circulating":{"peggedUSD":100000000},"circulatingPrevDay":{"peggedUSD":99000000},
				 "circulatingPrevWeek":{"peggedUSD":98000000},"circulatingPrevMonth":{"peggedUSD":95000000},
				 "chains":["Ethereum"],"price":1.1}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.stablecoinsAPIURL = srv.URL

	items, err := c.StablecoinsTop(context.Background(), "", 2)
	if err != nil {
		t.Fatalf("StablecoinsTop failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 items, got %d", len(items))
	}
	if items[0].Symbol != "USDT" || items[0].Rank != 1 {
		t.Fatalf("expected USDT first, got %+v", items[0])
	}
	if items[1].Symbol != "USDC" || items[1].Rank != 2 {
		t.Fatalf("expected USDC second, got %+v", items[1])
	}
	if items[0].CirculatingUSD != 120000000000 {
		t.Fatalf("unexpected circulating for USDT: %+v", items[0])
	}
	if items[0].Chains != 5 {
		t.Fatalf("expected 5 chains for USDT, got %d", items[0].Chains)
	}
	if items[0].Price != 1.0001 {
		t.Fatalf("unexpected price for USDT: %f", items[0].Price)
	}
	expectedDayChange := 120000000000.0 - 119500000000.0
	if items[0].DayChangeUSD != expectedDayChange {
		t.Fatalf("unexpected day change: got %f, want %f", items[0].DayChangeUSD, expectedDayChange)
	}
}

func TestStablecoinsTopFiltersByPegType(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/stablecoins", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"peggedAssets":[
				{"name":"Tether","symbol":"USDT","pegType":"peggedUSD","pegMechanism":"fiat-backed",
				 "circulating":{"peggedUSD":120000000000},"circulatingPrevDay":{"peggedUSD":119500000000},
				 "circulatingPrevWeek":{"peggedUSD":118000000000},"circulatingPrevMonth":{"peggedUSD":115000000000},
				 "chains":["Ethereum"],"price":1.0},
				{"name":"STASIS EURO","symbol":"EURS","pegType":"peggedEUR","pegMechanism":"fiat-backed",
				 "circulating":{"peggedUSD":100000000},"circulatingPrevDay":{"peggedUSD":99000000},
				 "circulatingPrevWeek":{"peggedUSD":98000000},"circulatingPrevMonth":{"peggedUSD":95000000},
				 "chains":["Ethereum"],"price":1.1}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.stablecoinsAPIURL = srv.URL

	items, err := c.StablecoinsTop(context.Background(), "peggedEUR", 20)
	if err != nil {
		t.Fatalf("StablecoinsTop failed: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 EUR-pegged item, got %d", len(items))
	}
	if items[0].Symbol != "EURS" || items[0].PegType != "peggedEUR" {
		t.Fatalf("unexpected filtered result: %+v", items[0])
	}
}

func TestStablecoinsTopNonUSDPegCirculating(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/stablecoins", func(w http.ResponseWriter, r *http.Request) {
		// DefiLlama uses peg-specific keys: peggedEUR for EUR stablecoins.
		_, _ = w.Write([]byte(`{
			"peggedAssets":[
				{"name":"STASIS EURO","symbol":"EURS","pegType":"peggedEUR","pegMechanism":"fiat-backed",
				 "circulating":{"peggedEUR":100000000},"circulatingPrevDay":{"peggedEUR":99000000},
				 "circulatingPrevWeek":{"peggedEUR":98000000},"circulatingPrevMonth":{"peggedEUR":95000000},
				 "chains":["Ethereum"],"price":1.1},
				{"name":"Tether","symbol":"USDT","pegType":"peggedUSD","pegMechanism":"fiat-backed",
				 "circulating":{"peggedUSD":50000000},"circulatingPrevDay":{"peggedUSD":49000000},
				 "circulatingPrevWeek":{"peggedUSD":48000000},"circulatingPrevMonth":{"peggedUSD":47000000},
				 "chains":["Ethereum"],"price":1.0}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.stablecoinsAPIURL = srv.URL

	items, err := c.StablecoinsTop(context.Background(), "", 0)
	if err != nil {
		t.Fatalf("StablecoinsTop failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 items, got %d", len(items))
	}
	// EUR stablecoin should sort first (100M > 50M) and have correct circulating value.
	if items[0].Symbol != "EURS" || items[0].CirculatingUSD != 100000000 {
		t.Fatalf("expected EURS first with circulating 100000000, got %+v", items[0])
	}
	expectedDayChange := 100000000.0 - 99000000.0
	if items[0].DayChangeUSD != expectedDayChange {
		t.Fatalf("expected day change %f for EUR stablecoin, got %f", expectedDayChange, items[0].DayChangeUSD)
	}
}

func TestStablecoinsTopNullPrice(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/stablecoins", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"peggedAssets":[
				{"name":"NoPrice","symbol":"NP","pegType":"peggedUSD","pegMechanism":"algo",
				 "circulating":{"peggedUSD":1000},"circulatingPrevDay":{"peggedUSD":1000},
				 "circulatingPrevWeek":{"peggedUSD":1000},"circulatingPrevMonth":{"peggedUSD":1000},
				 "chains":["Ethereum"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.stablecoinsAPIURL = srv.URL

	items, err := c.StablecoinsTop(context.Background(), "", 20)
	if err != nil {
		t.Fatalf("StablecoinsTop failed: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 item, got %d", len(items))
	}
	if items[0].Price != 0 {
		t.Fatalf("expected zero price for null, got %f", items[0].Price)
	}
}

func TestStablecoinChainsSortsAndLimits(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/stablecoinchains", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"gecko_id":"ethereum","totalCirculatingUSD":{"peggedUSD":90000000000,"peggedEUR":500000000},"tokenSymbol":"ETH","name":"Ethereum"},
			{"gecko_id":"tron","totalCirculatingUSD":{"peggedUSD":60000000000},"tokenSymbol":"TRX","name":"Tron"},
			{"gecko_id":"binancecoin","totalCirculatingUSD":{"peggedUSD":8000000000,"peggedEUR":200000000},"tokenSymbol":"BNB","name":"BSC"},
			{"gecko_id":"solana","totalCirculatingUSD":{"peggedUSD":12000000000},"tokenSymbol":"SOL","name":"Solana"}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.stablecoinsAPIURL = srv.URL

	items, err := c.StablecoinChains(context.Background(), 3)
	if err != nil {
		t.Fatalf("StablecoinChains failed: %v", err)
	}
	if len(items) != 3 {
		t.Fatalf("expected 3 items, got %d", len(items))
	}
	if items[0].Chain != "Ethereum" || items[0].Rank != 1 {
		t.Fatalf("expected Ethereum first, got %+v", items[0])
	}
	if items[0].CirculatingUSD != 90500000000 {
		t.Fatalf("expected aggregated USD+EUR for Ethereum, got %f", items[0].CirculatingUSD)
	}
	if items[0].DominantPegType != "peggedUSD" {
		t.Fatalf("expected peggedUSD dominant, got %s", items[0].DominantPegType)
	}
	if items[1].Chain != "Tron" || items[1].Rank != 2 {
		t.Fatalf("expected Tron second, got %+v", items[1])
	}
	if items[2].Chain != "Solana" || items[2].Rank != 3 {
		t.Fatalf("expected Solana third, got %+v", items[2])
	}
}

func TestStablecoinChainsSkipsZeroSupply(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/stablecoinchains", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"gecko_id":"ethereum","totalCirculatingUSD":{"peggedUSD":90000000000},"tokenSymbol":"ETH","name":"Ethereum"},
			{"gecko_id":"dead","totalCirculatingUSD":{"peggedUSD":0},"tokenSymbol":"DEAD","name":"DeadChain"},
			{"gecko_id":"empty","totalCirculatingUSD":{},"tokenSymbol":null,"name":"EmptyChain"}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.stablecoinsAPIURL = srv.URL

	items, err := c.StablecoinChains(context.Background(), 0)
	if err != nil {
		t.Fatalf("StablecoinChains failed: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 item (zero/empty filtered), got %d", len(items))
	}
	if items[0].Chain != "Ethereum" {
		t.Fatalf("expected Ethereum only, got %s", items[0].Chain)
	}
}

func TestStablecoinChainsNoLimit(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/stablecoinchains", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`[
			{"gecko_id":"ethereum","totalCirculatingUSD":{"peggedUSD":90000000000},"tokenSymbol":"ETH","name":"Ethereum"},
			{"gecko_id":"tron","totalCirculatingUSD":{"peggedUSD":60000000000},"tokenSymbol":"TRX","name":"Tron"}
		]`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.stablecoinsAPIURL = srv.URL

	items, err := c.StablecoinChains(context.Background(), 0)
	if err != nil {
		t.Fatalf("StablecoinChains failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected all 2 items with limit 0, got %d", len(items))
	}
}

func TestProtocolsFeesSortsAndLimits(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/fees", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"Uniswap","category":"Dexs","total24h":5000000,"total7d":30000000,"total30d":120000000,"change_1d":5.2,"change_7d":-2.1,"change_1m":10.5,"chains":["Ethereum","Arbitrum","Base"]},
				{"name":"Aave","category":"Lending","total24h":2000000,"total7d":12000000,"total30d":50000000,"change_1d":1.5,"change_7d":3.0,"change_1m":-5.0,"chains":["Ethereum","Polygon"]},
				{"name":"Lido","category":"Liquid Staking","total24h":8000000,"total7d":55000000,"total30d":200000000,"change_1d":-1.0,"change_7d":0.5,"change_1m":15.0,"chains":["Ethereum"]},
				{"name":"Dead","category":"Dexs","total24h":null,"chains":[]},
				{"name":"Tiny","category":"Dexs","total24h":0,"chains":["BSC"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsFees(context.Background(), "", "", 2)
	if err != nil {
		t.Fatalf("ProtocolsFees failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 items, got %d", len(items))
	}
	if items[0].Protocol != "Lido" || items[0].Rank != 1 {
		t.Fatalf("expected Lido first, got %+v", items[0])
	}
	if items[0].Fees24hUSD != 8000000 || items[0].Chains != 1 {
		t.Fatalf("unexpected Lido values: %+v", items[0])
	}
	if items[1].Protocol != "Uniswap" || items[1].Rank != 2 {
		t.Fatalf("expected Uniswap second, got %+v", items[1])
	}
}

func TestProtocolsFeesFiltersByCategory(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/fees", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"Uniswap","category":"Dexs","total24h":5000000,"chains":["Ethereum"]},
				{"name":"Aave","category":"Lending","total24h":2000000,"chains":["Ethereum"]},
				{"name":"Curve","category":"Dexs","total24h":1000000,"chains":["Ethereum"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsFees(context.Background(), "Dexs", "", 0)
	if err != nil {
		t.Fatalf("ProtocolsFees with category filter failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 Dexs items, got %d", len(items))
	}
	if items[0].Protocol != "Uniswap" {
		t.Fatalf("expected Uniswap first, got %s", items[0].Protocol)
	}
	if items[1].Protocol != "Curve" {
		t.Fatalf("expected Curve second, got %s", items[1].Protocol)
	}
}

func TestProtocolsFeesFiltersByChain(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/fees", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"Uniswap","category":"Dexs","total24h":5000000,"chains":["Ethereum","Arbitrum","Base"]},
				{"name":"PancakeSwap","category":"Dexs","total24h":8000000,"chains":["BSC"]},
				{"name":"Aave","category":"Lending","total24h":2000000,"chains":["Ethereum","Polygon"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsFees(context.Background(), "", "Ethereum", 0)
	if err != nil {
		t.Fatalf("ProtocolsFees with chain filter failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 Ethereum items, got %d", len(items))
	}
	if items[0].Protocol != "Uniswap" {
		t.Fatalf("expected Uniswap first, got %s", items[0].Protocol)
	}
	if items[1].Protocol != "Aave" {
		t.Fatalf("expected Aave second, got %s", items[1].Protocol)
	}
}

func TestProtocolsFeesFiltersByCategoryAndChain(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/fees", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"Uniswap","category":"Dexs","total24h":5000000,"chains":["Ethereum","Arbitrum"]},
				{"name":"Aave","category":"Lending","total24h":2000000,"chains":["Ethereum","Polygon"]},
				{"name":"Curve","category":"Dexs","total24h":1000000,"chains":["Ethereum"]},
				{"name":"PancakeSwap","category":"Dexs","total24h":8000000,"chains":["BSC"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsFees(context.Background(), "Dexs", "Ethereum", 0)
	if err != nil {
		t.Fatalf("ProtocolsFees with category+chain filter failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 Dexs+Ethereum items, got %d", len(items))
	}
	if items[0].Protocol != "Uniswap" {
		t.Fatalf("expected Uniswap first, got %s", items[0].Protocol)
	}
	if items[1].Protocol != "Curve" {
		t.Fatalf("expected Curve second, got %s", items[1].Protocol)
	}
}

func TestProtocolsFeesSkipsNullAndZero(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/fees", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"NullFees","category":"Dexs","total24h":null,"chains":[]},
				{"name":"ZeroFees","category":"Dexs","total24h":0,"chains":["Ethereum"]},
				{"name":"NegativeFees","category":"Dexs","total24h":-100,"chains":["Ethereum"]},
				{"name":"ValidFees","category":"Dexs","total24h":500,"chains":["Ethereum"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsFees(context.Background(), "", "", 0)
	if err != nil {
		t.Fatalf("ProtocolsFees failed: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 valid item, got %d", len(items))
	}
	if items[0].Protocol != "ValidFees" {
		t.Fatalf("expected ValidFees, got %s", items[0].Protocol)
	}
}

func TestProtocolsRevenueSortsAndLimits(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/fees", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Query().Get("dataType") != "dailyRevenue" {
			t.Errorf("expected dataType=dailyRevenue, got %s", r.URL.Query().Get("dataType"))
		}
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"Uniswap","category":"Dexs","total24h":3000000,"total7d":18000000,"total30d":70000000,"change_1d":4.2,"change_7d":-1.1,"change_1m":8.5,"chains":["Ethereum","Arbitrum","Base"]},
				{"name":"Aave","category":"Lending","total24h":1000000,"total7d":6000000,"total30d":25000000,"change_1d":2.5,"change_7d":4.0,"change_1m":-3.0,"chains":["Ethereum","Polygon"]},
				{"name":"Lido","category":"Liquid Staking","total24h":5000000,"total7d":35000000,"total30d":130000000,"change_1d":-0.5,"change_7d":1.5,"change_1m":12.0,"chains":["Ethereum"]},
				{"name":"Dead","category":"Dexs","total24h":null,"chains":[]},
				{"name":"Tiny","category":"Dexs","total24h":0,"chains":["BSC"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsRevenue(context.Background(), "", "", 2)
	if err != nil {
		t.Fatalf("ProtocolsRevenue failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 items, got %d", len(items))
	}
	if items[0].Protocol != "Lido" || items[0].Rank != 1 {
		t.Fatalf("expected Lido first, got %+v", items[0])
	}
	if items[0].Revenue24hUSD != 5000000 || items[0].Chains != 1 {
		t.Fatalf("unexpected Lido values: %+v", items[0])
	}
	if items[1].Protocol != "Uniswap" || items[1].Rank != 2 {
		t.Fatalf("expected Uniswap second, got %+v", items[1])
	}
}

func TestProtocolsRevenueFiltersByCategory(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/fees", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"Uniswap","category":"Dexs","total24h":3000000,"chains":["Ethereum"]},
				{"name":"Aave","category":"Lending","total24h":1000000,"chains":["Ethereum"]},
				{"name":"Curve","category":"Dexs","total24h":500000,"chains":["Ethereum"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsRevenue(context.Background(), "Dexs", "", 0)
	if err != nil {
		t.Fatalf("ProtocolsRevenue with category filter failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 Dexs items, got %d", len(items))
	}
	if items[0].Protocol != "Uniswap" {
		t.Fatalf("expected Uniswap first, got %s", items[0].Protocol)
	}
	if items[1].Protocol != "Curve" {
		t.Fatalf("expected Curve second, got %s", items[1].Protocol)
	}
}

func TestProtocolsRevenueFiltersByChain(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/fees", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"Uniswap","category":"Dexs","total24h":3000000,"chains":["Ethereum","Arbitrum","Base"]},
				{"name":"PancakeSwap","category":"Dexs","total24h":5000000,"chains":["BSC"]},
				{"name":"Aave","category":"Lending","total24h":1000000,"chains":["Ethereum","Polygon"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsRevenue(context.Background(), "", "Ethereum", 0)
	if err != nil {
		t.Fatalf("ProtocolsRevenue with chain filter failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 Ethereum items, got %d", len(items))
	}
	if items[0].Protocol != "Uniswap" {
		t.Fatalf("expected Uniswap first, got %s", items[0].Protocol)
	}
	if items[1].Protocol != "Aave" {
		t.Fatalf("expected Aave second, got %s", items[1].Protocol)
	}
}

func TestProtocolsRevenueFiltersByCategoryAndChain(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/fees", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"Uniswap","category":"Dexs","total24h":3000000,"chains":["Ethereum","Arbitrum"]},
				{"name":"Aave","category":"Lending","total24h":1000000,"chains":["Ethereum"]},
				{"name":"PancakeSwap","category":"Dexs","total24h":5000000,"chains":["BSC"]},
				{"name":"Curve","category":"Dexs","total24h":500000,"chains":["Ethereum"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsRevenue(context.Background(), "Dexs", "Ethereum", 0)
	if err != nil {
		t.Fatalf("ProtocolsRevenue with category+chain filter failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 Dexs+Ethereum items, got %d", len(items))
	}
	if items[0].Protocol != "Uniswap" {
		t.Fatalf("expected Uniswap first, got %s", items[0].Protocol)
	}
	if items[1].Protocol != "Curve" {
		t.Fatalf("expected Curve second, got %s", items[1].Protocol)
	}
}

func TestProtocolsRevenueSkipsNullAndZero(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/fees", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"NullRev","category":"Dexs","total24h":null,"chains":[]},
				{"name":"ZeroRev","category":"Dexs","total24h":0,"chains":["Ethereum"]},
				{"name":"NegRev","category":"Dexs","total24h":-100,"chains":["Ethereum"]},
				{"name":"ValidRev","category":"Dexs","total24h":500,"chains":["Ethereum"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.ProtocolsRevenue(context.Background(), "", "", 0)
	if err != nil {
		t.Fatalf("ProtocolsRevenue failed: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 valid item, got %d", len(items))
	}
	if items[0].Protocol != "ValidRev" {
		t.Fatalf("expected ValidRev, got %s", items[0].Protocol)
	}
}

func TestDexesVolumeSortsAndLimits(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/dexs", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"Uniswap","total24h":5000000,"total7d":30000000,"total30d":120000000,"change_1d":5.2,"change_7d":-2.1,"change_1m":10.5,"chains":["Ethereum","Arbitrum","Base"]},
				{"name":"Curve","total24h":2000000,"total7d":12000000,"total30d":50000000,"change_1d":1.5,"change_7d":3.0,"change_1m":-5.0,"chains":["Ethereum","Polygon"]},
				{"name":"PancakeSwap","total24h":8000000,"total7d":55000000,"total30d":200000000,"change_1d":-1.0,"change_7d":0.5,"change_1m":15.0,"chains":["BSC"]},
				{"name":"Dead","total24h":null,"chains":[]},
				{"name":"Tiny","total24h":0,"chains":["BSC"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.DexesVolume(context.Background(), "", 2)
	if err != nil {
		t.Fatalf("DexesVolume failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 items, got %d", len(items))
	}
	if items[0].Protocol != "PancakeSwap" || items[0].Rank != 1 {
		t.Fatalf("expected PancakeSwap first, got %+v", items[0])
	}
	if items[0].Volume24hUSD != 8000000 || items[0].Chains != 1 {
		t.Fatalf("unexpected PancakeSwap values: %+v", items[0])
	}
	if items[1].Protocol != "Uniswap" || items[1].Rank != 2 {
		t.Fatalf("expected Uniswap second, got %+v", items[1])
	}
}

func TestDexesVolumeFiltersByChain(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/dexs", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"Uniswap","total24h":5000000,"chains":["Ethereum","Arbitrum","Base"]},
				{"name":"PancakeSwap","total24h":8000000,"chains":["BSC"]},
				{"name":"SushiSwap","total24h":1000000,"chains":["Ethereum","Polygon"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.DexesVolume(context.Background(), "Ethereum", 0)
	if err != nil {
		t.Fatalf("DexesVolume with chain filter failed: %v", err)
	}
	if len(items) != 2 {
		t.Fatalf("expected 2 Ethereum items, got %d", len(items))
	}
	if items[0].Protocol != "Uniswap" {
		t.Fatalf("expected Uniswap first, got %s", items[0].Protocol)
	}
	if items[1].Protocol != "SushiSwap" {
		t.Fatalf("expected SushiSwap second, got %s", items[1].Protocol)
	}
}

func TestDexesVolumeSkipsNullAndZero(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/overview/dexs", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{
			"protocols":[
				{"name":"NullVol","total24h":null,"chains":[]},
				{"name":"ZeroVol","total24h":0,"chains":["Ethereum"]},
				{"name":"NegVol","total24h":-100,"chains":["Ethereum"]},
				{"name":"ValidVol","total24h":500,"chains":["Ethereum"]}
			]
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	c := New(httpx.New(2*time.Second, 0), "")
	c.apiBase = srv.URL

	items, err := c.DexesVolume(context.Background(), "", 0)
	if err != nil {
		t.Fatalf("DexesVolume failed: %v", err)
	}
	if len(items) != 1 {
		t.Fatalf("expected 1 valid item, got %d", len(items))
	}
	if items[0].Protocol != "ValidVol" {
		t.Fatalf("expected ValidVol, got %s", items[0].Protocol)
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
