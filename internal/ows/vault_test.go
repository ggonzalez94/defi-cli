package ows

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestResolveWalletRefByID(t *testing.T) {
	vaultDir := t.TempDir()
	writeWalletFixture(t, vaultDir, Wallet{
		ID:        "wallet-123",
		Name:      "alice",
		CreatedAt: "2026-03-25T00:00:00Z",
		Accounts: []WalletAccount{
			{
				AccountID:      "account-1",
				Address:        "0x000000000000000000000000000000000000dEaD",
				ChainID:        "eip155:1",
				DerivationPath: "m/44'/60'/0'/0/0",
			},
		},
	})
	writeWalletFixture(t, vaultDir, Wallet{
		ID:        "wallet-999",
		Name:      "alice",
		CreatedAt: "2026-03-25T00:00:00Z",
	})

	got, err := ResolveWalletRef(vaultDir, "wallet-123")
	if err != nil {
		t.Fatalf("ResolveWalletRef failed: %v", err)
	}
	if got.ID != "wallet-123" {
		t.Fatalf("expected wallet id wallet-123, got %q", got.ID)
	}
	if got.Name != "alice" {
		t.Fatalf("expected wallet name alice, got %q", got.Name)
	}
}

func TestResolveWalletRefByName(t *testing.T) {
	vaultDir := t.TempDir()
	writeWalletFixture(t, vaultDir, Wallet{
		ID:        "wallet-123",
		Name:      "alice",
		CreatedAt: "2026-03-25T00:00:00Z",
	})

	got, err := ResolveWalletRef(vaultDir, "alice")
	if err != nil {
		t.Fatalf("ResolveWalletRef failed: %v", err)
	}
	if got.ID != "wallet-123" {
		t.Fatalf("expected wallet id wallet-123, got %q", got.ID)
	}
}

func TestResolveWalletRefRejectsAmbiguousName(t *testing.T) {
	vaultDir := t.TempDir()
	writeWalletFixture(t, vaultDir, Wallet{
		ID:        "wallet-1",
		Name:      "alice",
		CreatedAt: "2026-03-25T00:00:00Z",
	})
	writeWalletFixture(t, vaultDir, Wallet{
		ID:        "wallet-2",
		Name:      "alice",
		CreatedAt: "2026-03-25T00:00:01Z",
	})

	_, err := ResolveWalletRef(vaultDir, "alice")
	if err == nil {
		t.Fatal("expected ambiguous name lookup to fail")
	}
}

func TestResolveWalletSenderAddressUsesEVMAccount(t *testing.T) {
	wallet := Wallet{
		ID:   "wallet-123",
		Name: "alice",
		Accounts: []WalletAccount{
			{
				AccountID:      "account-1",
				Address:        "0x000000000000000000000000000000000000dEaD",
				ChainID:        "solana:mainnet",
				DerivationPath: "m/44'/501'/0'/0'",
			},
			{
				AccountID:      "account-2",
				Address:        "0x1111111111111111111111111111111111111111",
				ChainID:        "eip155:1",
				DerivationPath: "m/44'/60'/0'/0/0",
			},
		},
	}

	got, err := SenderAddressForChain(wallet, "eip155:8453")
	if err != nil {
		t.Fatalf("SenderAddressForChain failed: %v", err)
	}
	if got != "0x1111111111111111111111111111111111111111" {
		t.Fatalf("expected fallback EVM address, got %q", got)
	}
}

func TestResolveWalletSenderAddressFailsWithoutMatchingFamily(t *testing.T) {
	wallet := Wallet{
		ID:   "wallet-123",
		Name: "alice",
		Accounts: []WalletAccount{
			{
				AccountID:      "account-1",
				Address:        "So11111111111111111111111111111111111111112",
				ChainID:        "solana:mainnet",
				DerivationPath: "m/44'/501'/0'/0'",
			},
		},
	}

	_, err := SenderAddressForChain(wallet, "eip155:1")
	if err == nil {
		t.Fatal("expected missing EVM family lookup to fail")
	}
}

func writeWalletFixture(t *testing.T, vaultDir string, wallet Wallet) {
	t.Helper()

	walletsDir := filepath.Join(vaultDir, "wallets")
	if err := os.MkdirAll(walletsDir, 0o755); err != nil {
		t.Fatalf("mkdir wallets: %v", err)
	}
	path := filepath.Join(walletsDir, wallet.ID+".json")
	data, err := jsonMarshalIndent(wallet)
	if err != nil {
		t.Fatalf("marshal wallet: %v", err)
	}
	if err := os.WriteFile(path, data, 0o644); err != nil {
		t.Fatalf("write wallet fixture: %v", err)
	}
}

func jsonMarshalIndent(v any) ([]byte, error) {
	return json.MarshalIndent(v, "", "  ")
}
