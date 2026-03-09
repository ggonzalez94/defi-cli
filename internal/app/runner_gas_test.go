package app

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"math/big"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
)

func TestChainsGasBypassesCache(t *testing.T) {
	if shouldOpenCache("chains gas") {
		t.Fatal("chains gas should bypass cache initialization")
	}
}

func TestWeiToGwei(t *testing.T) {
	tests := []struct {
		name string
		wei  *big.Int
		want string
	}{
		{name: "nil", wei: nil, want: "0"},
		{name: "zero", wei: big.NewInt(0), want: "0.000000"},
		{name: "1 gwei", wei: big.NewInt(1_000_000_000), want: "1.000000"},
		{name: "30.5 gwei", wei: big.NewInt(30_500_000_000), want: "30.500000"},
		{name: "sub-gwei", wei: big.NewInt(500_000), want: "0.000500"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := weiToGwei(tc.wei)
			if got != tc.want {
				t.Fatalf("weiToGwei(%v) = %q, want %q", tc.wei, got, tc.want)
			}
		})
	}
}

func TestFetchGasPriceEIP1559(t *testing.T) {
	srv := newMockRPCServer(t, mockRPCConfig{
		baseFeeHex:     "0x3B9ACA00",  // 1 gwei
		priorityFeeHex: "0x77359400",  // 2 gwei
		gasPriceHex:    "0xB2D05E00",  // 3 gwei
		blockNumberHex: "0x10",        // block 16
	})
	defer srv.Close()

	chain := id.Chain{Name: "Ethereum", Slug: "ethereum", CAIP2: "eip155:1", EVMChainID: 1}
	now := func() time.Time { return time.Date(2026, 3, 9, 12, 0, 0, 0, time.UTC) }

	result, err := fetchGasPrice(context.Background(), chain, srv.URL, now)
	if err != nil {
		t.Fatalf("fetchGasPrice failed: %v", err)
	}

	if result.ChainID != "eip155:1" {
		t.Fatalf("expected chain_id eip155:1, got %s", result.ChainID)
	}
	if result.ChainName != "Ethereum" {
		t.Fatalf("expected chain_name Ethereum, got %s", result.ChainName)
	}
	if result.BlockNumber != 16 {
		t.Fatalf("expected block_number 16, got %d", result.BlockNumber)
	}
	if !result.EIP1559 {
		t.Fatal("expected eip1559=true")
	}
	if result.BaseFeeGwei != "1.000000" {
		t.Fatalf("expected base_fee_gwei 1.000000, got %s", result.BaseFeeGwei)
	}
	if result.PriorityFeeGwei != "2.000000" {
		t.Fatalf("expected priority_fee_gwei 2.000000, got %s", result.PriorityFeeGwei)
	}
	if result.GasPriceGwei != "3.000000" {
		t.Fatalf("expected gas_price_gwei 3.000000, got %s", result.GasPriceGwei)
	}
	if result.FetchedAt != "2026-03-09T12:00:00Z" {
		t.Fatalf("unexpected fetched_at: %s", result.FetchedAt)
	}
}

func TestFetchGasPriceLegacy(t *testing.T) {
	srv := newMockRPCServer(t, mockRPCConfig{
		baseFeeHex:     "", // no base fee = legacy chain
		gasPriceHex:    "0x12A05F200", // 5 gwei
		blockNumberHex: "0x5",
	})
	defer srv.Close()

	chain := id.Chain{Name: "TestLegacy", Slug: "legacy", CAIP2: "eip155:999", EVMChainID: 999}
	now := func() time.Time { return time.Date(2026, 3, 9, 12, 0, 0, 0, time.UTC) }

	result, err := fetchGasPrice(context.Background(), chain, srv.URL, now)
	if err != nil {
		t.Fatalf("fetchGasPrice failed: %v", err)
	}

	if result.EIP1559 {
		t.Fatal("expected eip1559=false for legacy chain")
	}
	if result.BaseFeeGwei != "" {
		t.Fatalf("expected empty base_fee_gwei for legacy chain, got %s", result.BaseFeeGwei)
	}
	if result.PriorityFeeGwei != "" {
		t.Fatalf("expected empty priority_fee_gwei for legacy chain, got %s", result.PriorityFeeGwei)
	}
	if result.GasPriceGwei != "5.000000" {
		t.Fatalf("expected gas_price_gwei 5.000000, got %s", result.GasPriceGwei)
	}
}

func TestChainsGasRejectsNonEVM(t *testing.T) {
	var stdout, stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"chains", "gas", "--chain", "solana"})
	if code == 0 {
		t.Fatal("expected non-zero exit code for non-EVM chain")
	}
	if !strings.Contains(stderr.String(), "EVM") {
		t.Fatalf("expected EVM-only error message, got: %s", stderr.String())
	}
}

func TestChainsGasRequiresChainFlag(t *testing.T) {
	var stdout, stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"chains", "gas"})
	if code == 0 {
		t.Fatal("expected non-zero exit code when --chain is missing")
	}
}

func TestChainsGasEndToEndWithMockRPC(t *testing.T) {
	srv := newMockRPCServer(t, mockRPCConfig{
		baseFeeHex:     "0x3B9ACA00",
		priorityFeeHex: "0x77359400",
		gasPriceHex:    "0xB2D05E00",
		blockNumberHex: "0x10",
	})
	defer srv.Close()

	var stdout, stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"chains", "gas", "--chain", "1", "--rpc-url", srv.URL, "--results-only"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}

	var result model.GasPrice
	if err := json.Unmarshal(stdout.Bytes(), &result); err != nil {
		t.Fatalf("failed to parse output: %v output=%s", err, stdout.String())
	}
	if result.ChainID != "eip155:1" {
		t.Fatalf("expected chain_id eip155:1, got %s", result.ChainID)
	}
	if !result.EIP1559 {
		t.Fatal("expected eip1559=true")
	}
	if result.BaseFeeGwei != "1.000000" {
		t.Fatalf("expected base_fee_gwei 1.000000, got %s", result.BaseFeeGwei)
	}
}

// --- mock RPC server ---

type mockRPCConfig struct {
	baseFeeHex     string // empty string means no baseFee (legacy)
	priorityFeeHex string
	gasPriceHex    string
	blockNumberHex string
}

func newMockRPCServer(t *testing.T, cfg mockRPCConfig) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var reqs []json.RawMessage
		batch := false

		var raw json.RawMessage
		if err := json.NewDecoder(r.Body).Decode(&raw); err != nil {
			http.Error(w, "bad json", 400)
			return
		}

		trimmed := bytes.TrimSpace(raw)
		if len(trimmed) > 0 && trimmed[0] == '[' {
			batch = true
			if err := json.Unmarshal(trimmed, &reqs); err != nil {
				http.Error(w, "bad batch", 400)
				return
			}
		} else {
			reqs = []json.RawMessage{raw}
		}

		var results []json.RawMessage
		for _, reqRaw := range reqs {
			var req struct {
				ID     json.RawMessage `json:"id"`
				Method string          `json:"method"`
			}
			if err := json.Unmarshal(reqRaw, &req); err != nil {
				continue
			}

			var resp string
			switch req.Method {
			case "eth_getBlockByNumber":
				baseFee := "null"
				if cfg.baseFeeHex != "" {
					baseFee = fmt.Sprintf("%q", cfg.baseFeeHex)
				}
				resp = fmt.Sprintf(`{"jsonrpc":"2.0","id":%s,"result":{"number":"%s","baseFeePerGas":%s,"hash":"0x0000000000000000000000000000000000000000000000000000000000000000","parentHash":"0x0000000000000000000000000000000000000000000000000000000000000000","sha3Uncles":"0x0000000000000000000000000000000000000000000000000000000000000000","miner":"0x0000000000000000000000000000000000000000","stateRoot":"0x0000000000000000000000000000000000000000000000000000000000000000","transactionsRoot":"0x0000000000000000000000000000000000000000000000000000000000000000","receiptsRoot":"0x0000000000000000000000000000000000000000000000000000000000000000","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","difficulty":"0x0","totalDifficulty":"0x0","size":"0x0","gasLimit":"0x0","gasUsed":"0x0","timestamp":"0x0","extraData":"0x","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","nonce":"0x0000000000000000","uncles":[],"transactions":[]}}`,
					req.ID, cfg.blockNumberHex, baseFee)
			case "eth_gasPrice":
				resp = fmt.Sprintf(`{"jsonrpc":"2.0","id":%s,"result":"%s"}`, req.ID, cfg.gasPriceHex)
			case "eth_maxPriorityFeePerGas":
				if cfg.priorityFeeHex != "" {
					resp = fmt.Sprintf(`{"jsonrpc":"2.0","id":%s,"result":"%s"}`, req.ID, cfg.priorityFeeHex)
				} else {
					resp = fmt.Sprintf(`{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"method not found"}}`, req.ID)
				}
			default:
				resp = fmt.Sprintf(`{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"method not found"}}`, req.ID)
			}
			results = append(results, json.RawMessage(resp))
		}

		w.Header().Set("Content-Type", "application/json")
		if batch {
			json.NewEncoder(w).Encode(results)
		} else if len(results) > 0 {
			w.Write(results[0])
		}
	}))
}
