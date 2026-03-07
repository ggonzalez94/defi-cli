package app

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"math/big"
	"strings"
	"testing"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ggonzalez94/defi-cli/internal/id"
)

func TestWalletBalanceMissingChain(t *testing.T) {
	var stdout, stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"wallet", "balance", "--address", "0x000000000000000000000000000000000000dEaD"})
	if code != 2 {
		t.Fatalf("expected exit 2 (usage), got %d stderr=%s", code, stderr.String())
	}
}

func TestWalletBalanceMissingAddress(t *testing.T) {
	var stdout, stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"wallet", "balance", "--chain", "1"})
	if code != 2 {
		t.Fatalf("expected exit 2 (usage), got %d stderr=%s", code, stderr.String())
	}
}

func TestWalletBalanceInvalidAddress(t *testing.T) {
	var stdout, stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"wallet", "balance", "--chain", "1", "--address", "notanaddress"})
	if code != 2 {
		t.Fatalf("expected exit 2, got %d stderr=%s", code, stderr.String())
	}
}

func TestWalletBalanceUnsupportedSolana(t *testing.T) {
	var stdout, stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"wallet", "balance", "--chain", "solana", "--address", "0x000000000000000000000000000000000000dEaD"})
	if code != 13 {
		t.Fatalf("expected exit 13 (unsupported), got %d stderr=%s", code, stderr.String())
	}
}

func TestWalletBalanceErrorEnvelope(t *testing.T) {
	var stdout, stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"wallet", "balance", "--chain", "1"})
	if code == 0 {
		t.Fatal("expected non-zero exit code")
	}
	var env map[string]any
	if err := json.Unmarshal(stderr.Bytes(), &env); err != nil {
		t.Fatalf("error output should be valid JSON envelope: %v raw=%s", err, stderr.String())
	}
	if env["success"] != false {
		t.Fatalf("expected success=false, got %v", env["success"])
	}
}

func TestNativeSymbol(t *testing.T) {
	tests := []struct {
		chainID int64
		want    string
	}{
		{1, "ETH"},
		{8453, "ETH"},
		{42161, "ETH"},
		{137, "POL"},
		{56, "BNB"},
		{43114, "AVAX"},
		{100, "XDAI"},
		{5000, "MNT"},
		{42220, "CELO"},
		{146, "S"},
		{80094, "BERA"},
		{999, "HYPE"},
		{143, "MON"},
		{4114, "cBTC"},
	}
	for _, tc := range tests {
		t.Run(fmt.Sprintf("chain_%d", tc.chainID), func(t *testing.T) {
			chain := id.Chain{EVMChainID: tc.chainID}
			got := nativeSymbol(chain)
			if got != tc.want {
				t.Fatalf("nativeSymbol(chain %d) = %q, want %q", tc.chainID, got, tc.want)
			}
		})
	}
}

func TestNativeAssetID(t *testing.T) {
	tests := []struct {
		name    string
		chain   id.Chain
		wantID  string
		wantSym string
	}{
		{name: "ethereum", chain: id.Chain{CAIP2: "eip155:1", EVMChainID: 1}, wantID: "eip155:1/slip44:60", wantSym: "ETH"},
		{name: "bsc", chain: id.Chain{CAIP2: "eip155:56", EVMChainID: 56}, wantID: "eip155:56/slip44:714", wantSym: "BNB"},
		{name: "gnosis", chain: id.Chain{CAIP2: "eip155:100", EVMChainID: 100}, wantID: "eip155:100/slip44:700", wantSym: "XDAI"},
		{name: "polygon", chain: id.Chain{CAIP2: "eip155:137", EVMChainID: 137}, wantID: "eip155:137/slip44:966", wantSym: "POL"},
		{name: "monad", chain: id.Chain{CAIP2: "eip155:143", EVMChainID: 143}, wantID: "eip155:143/slip44:268435779", wantSym: "MON"},
		{name: "sonic", chain: id.Chain{CAIP2: "eip155:146", EVMChainID: 146}, wantID: "eip155:146/slip44:10007", wantSym: "S"},
		{name: "avalanche", chain: id.Chain{CAIP2: "eip155:43114", EVMChainID: 43114}, wantID: "eip155:43114/slip44:9000", wantSym: "AVAX"},
		{name: "celo", chain: id.Chain{CAIP2: "eip155:42220", EVMChainID: 42220}, wantID: "eip155:42220/slip44:52752", wantSym: "CELO"},
		{name: "berachain", chain: id.Chain{CAIP2: "eip155:80094", EVMChainID: 80094}, wantID: "eip155:80094/slip44:8008", wantSym: "BERA"},
		{name: "hyperevm", chain: id.Chain{CAIP2: "eip155:999", EVMChainID: 999}, wantID: "eip155:999/slip44:2457", wantSym: "HYPE"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if got := nativeAssetID(tc.chain); got != tc.wantID {
				t.Fatalf("nativeAssetID(%s) = %q, want %q", tc.name, got, tc.wantID)
			}
			if got := nativeSymbol(tc.chain); got != tc.wantSym {
				t.Fatalf("nativeSymbol(%s) = %q, want %q", tc.name, got, tc.wantSym)
			}
		})
	}
}

func TestFetchERC20BalanceRejectsShortResponse(t *testing.T) {
	client := stubWalletRPC{
		callContract: func(_ context.Context, msg ethereum.CallMsg, _ *big.Int) ([]byte, error) {
			if got := common.Bytes2Hex(msg.Data[:4]); got != common.Bytes2Hex(erc20BalanceOfSelector) {
				t.Fatalf("unexpected selector %s", got)
			}
			return []byte{}, nil
		},
	}

	_, err := fetchERC20Balance(context.Background(), client, id.Chain{CAIP2: "eip155:1", EVMChainID: 1}, common.HexToAddress("0x000000000000000000000000000000000000dEaD"), id.Asset{
		AssetID: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
		Address: "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
		Symbol:  "USDC",
	})
	if err == nil {
		t.Fatal("expected short ERC-20 response to fail")
	}
	if !strings.Contains(err.Error(), "balanceOf returned 0 bytes") {
		t.Fatalf("expected short response error, got %v", err)
	}
}

func TestFetchERC20BalanceFetchesOnChainDecimals(t *testing.T) {
	token := common.HexToAddress("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")
	client := stubWalletRPC{
		callContract: func(_ context.Context, msg ethereum.CallMsg, _ *big.Int) ([]byte, error) {
			if msg.To == nil || *msg.To != token {
				t.Fatalf("unexpected token target %v", msg.To)
			}
			switch common.Bytes2Hex(msg.Data[:4]) {
			case common.Bytes2Hex(erc20BalanceOfSelector):
				return encodeUint256(1234567), nil
			case common.Bytes2Hex(erc20DecimalsSelector):
				return encodeUint256(6), nil
			default:
				t.Fatalf("unexpected selector %s", common.Bytes2Hex(msg.Data[:4]))
				return nil, nil
			}
		},
	}

	got, err := fetchERC20Balance(context.Background(), client, id.Chain{CAIP2: "eip155:1", EVMChainID: 1}, common.HexToAddress("0x000000000000000000000000000000000000dEaD"), id.Asset{
		AssetID: "eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
		Address: token.Hex(),
		Symbol:  "USDC",
	})
	if err != nil {
		t.Fatalf("fetchERC20Balance failed: %v", err)
	}
	if got.Balance.Decimals != 6 {
		t.Fatalf("expected on-chain decimals 6, got %d", got.Balance.Decimals)
	}
	if got.Balance.AmountBaseUnits != "1234567" {
		t.Fatalf("unexpected base units %s", got.Balance.AmountBaseUnits)
	}
	if got.Balance.AmountDecimal != "1.234567" {
		t.Fatalf("unexpected decimal amount %s", got.Balance.AmountDecimal)
	}
}

type stubWalletRPC struct {
	balanceAt    func(context.Context, common.Address, *big.Int) (*big.Int, error)
	callContract func(context.Context, ethereum.CallMsg, *big.Int) ([]byte, error)
}

func (s stubWalletRPC) BalanceAt(ctx context.Context, account common.Address, blockNumber *big.Int) (*big.Int, error) {
	if s.balanceAt == nil {
		return nil, fmt.Errorf("unexpected BalanceAt(%s)", account.Hex())
	}
	return s.balanceAt(ctx, account, blockNumber)
}

func (s stubWalletRPC) CallContract(ctx context.Context, msg ethereum.CallMsg, blockNumber *big.Int) ([]byte, error) {
	if s.callContract == nil {
		return nil, fmt.Errorf("unexpected CallContract")
	}
	return s.callContract(ctx, msg, blockNumber)
}

func encodeUint256(v int64) []byte {
	out := make([]byte, 32)
	big.NewInt(v).FillBytes(out)
	return out
}
