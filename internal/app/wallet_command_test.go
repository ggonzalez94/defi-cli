package app

import (
	"bytes"
	"encoding/json"
	"fmt"
	"testing"

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
