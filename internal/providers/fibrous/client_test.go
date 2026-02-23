package fibrous

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

func newTestClient(srv *httptest.Server) *Client {
	c := New(httpx.New(2*time.Second, 0))
	c.baseURL = srv.URL
	return c
}

func TestQuoteSwap_Success(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/base/route", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			http.Error(w, "expected GET", http.StatusMethodNotAllowed)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{
			"success": true,
			"inputAmount": "1000000",
			"outputAmount": "471974940000000000",
			"estimatedGasUsed": "200000",
			"estimatedGasUsedInUsd": 0.05,
			"inputToken": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
			"outputToken": "0x4200000000000000000000000000000000000006"
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	chain, _ := id.ParseChain("base")
	fromAsset, _ := id.ParseAsset("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913", chain)
	toAsset, _ := id.ParseAsset("0x4200000000000000000000000000000000000006", chain)

	c := newTestClient(srv)
	quote, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}
	if quote.Provider != "fibrous" {
		t.Errorf("expected provider=fibrous, got %s", quote.Provider)
	}
	if quote.ChainID != "eip155:8453" {
		t.Errorf("expected chain_id=eip155:8453, got %s", quote.ChainID)
	}
	if quote.InputAmount.AmountBaseUnits != "1000000" {
		t.Errorf("unexpected input amount: %s", quote.InputAmount.AmountBaseUnits)
	}
	if quote.EstimatedOut.AmountBaseUnits != "471974940000000000" {
		t.Errorf("unexpected output amount: %s", quote.EstimatedOut.AmountBaseUnits)
	}
	if quote.EstimatedGasUSD != 0.05 {
		t.Errorf("unexpected gas USD: %f", quote.EstimatedGasUSD)
	}
	if quote.FetchedAt == "" {
		t.Error("expected non-empty FetchedAt")
	}
}

func TestQuoteSwap_UnsupportedChain(t *testing.T) {
	srv := httptest.NewServer(http.NewServeMux())
	defer srv.Close()

	chain, _ := id.ParseChain("ethereum")
	fromAsset, _ := id.ParseAsset("USDC", chain)
	toAsset, _ := id.ParseAsset("WETH", chain)

	c := newTestClient(srv)
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err == nil {
		t.Fatal("expected unsupported chain error")
	}
}

func TestQuoteSwap_APIError(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/base/route", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"success": false}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	chain, _ := id.ParseChain("base")
	fromAsset, _ := id.ParseAsset("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913", chain)
	toAsset, _ := id.ParseAsset("0x4200000000000000000000000000000000000006", chain)

	c := newTestClient(srv)
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	})
	if err == nil {
		t.Fatal("expected error for success=false response")
	}
}

func TestQuoteSwap_HyperEVM(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/hyperevm/route", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{
			"success": true,
			"inputAmount": "1000000000000000000",
			"outputAmount": "998000000000000000",
			"estimatedGasUsed": "150000",
			"estimatedGasUsedInUsd": 0.001,
			"inputToken": "0x5555555555555555555555555555555555555555",
			"outputToken": "0x6666666666666666666666666666666666666666"
		}`))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	chain, _ := id.ParseChain("hyperevm")
	fromAsset := id.Asset{
		ChainID:  "eip155:999",
		AssetID:  "eip155:999/erc20:0x5555555555555555555555555555555555555555",
		Address:  "0x5555555555555555555555555555555555555555",
		Decimals: 18,
	}
	toAsset := id.Asset{
		ChainID:  "eip155:999",
		AssetID:  "eip155:999/erc20:0x6666666666666666666666666666666666666666",
		Address:  "0x6666666666666666666666666666666666666666",
		Decimals: 18,
	}

	c := newTestClient(srv)
	quote, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000000000000000",
		AmountDecimal:   "1",
	})
	if err != nil {
		t.Fatalf("QuoteSwap HyperEVM failed: %v", err)
	}
	if quote.ChainID != "eip155:999" {
		t.Errorf("expected chain eip155:999, got %s", quote.ChainID)
	}
	if quote.EstimatedOut.AmountBaseUnits != "998000000000000000" {
		t.Errorf("unexpected output: %s", quote.EstimatedOut.AmountBaseUnits)
	}
}

func TestInfo(t *testing.T) {
	c := New(httpx.New(1*time.Second, 0))
	info := c.Info()
	if info.Name != "fibrous" {
		t.Errorf("expected name=fibrous, got %s", info.Name)
	}
	if info.RequiresKey {
		t.Error("expected RequiresKey=false")
	}
	if len(info.Capabilities) == 0 {
		t.Error("expected at least one capability")
	}
}
