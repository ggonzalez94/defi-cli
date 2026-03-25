package app

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/ows"
)

func TestResolveExecutionIdentityFromWallet(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	writeOWSWalletFixture(t, home, ows.Wallet{
		ID:        "wallet-123",
		Name:      "Agent Wallet",
		CreatedAt: "2026-03-25T00:00:00Z",
		Accounts: []ows.WalletAccount{
			{
				AccountID:      "acc-1",
				Address:        "0x000000000000000000000000000000000000dead",
				ChainID:        "eip155:1",
				DerivationPath: "m/44'/60'/0'/0/0",
			},
		},
	})

	got, err := resolveExecutionIdentity("wallet-123", "", "1")
	if err != nil {
		t.Fatalf("resolveExecutionIdentity failed: %v", err)
	}
	if got.WalletID != "wallet-123" {
		t.Fatalf("expected wallet id wallet-123, got %q", got.WalletID)
	}
	if got.WalletName != "Agent Wallet" {
		t.Fatalf("expected wallet name Agent Wallet, got %q", got.WalletName)
	}
	if got.FromAddress != "0x000000000000000000000000000000000000dEaD" {
		t.Fatalf("expected canonical sender, got %q", got.FromAddress)
	}
	if got.ExecutionBackend != execution.ExecutionBackendOWS {
		t.Fatalf("expected backend %q, got %q", execution.ExecutionBackendOWS, got.ExecutionBackend)
	}
	if len(got.Warnings) != 0 {
		t.Fatalf("expected no warnings, got %#v", got.Warnings)
	}
}

func TestResolveExecutionIdentityFromDeprecatedFromAddress(t *testing.T) {
	got, err := resolveExecutionIdentity("", "0x000000000000000000000000000000000000dead", "1")
	if err != nil {
		t.Fatalf("resolveExecutionIdentity failed: %v", err)
	}
	if got.WalletID != "" || got.WalletName != "" {
		t.Fatalf("expected empty wallet metadata, got id=%q name=%q", got.WalletID, got.WalletName)
	}
	if got.FromAddress != "0x000000000000000000000000000000000000dEaD" {
		t.Fatalf("expected canonical sender, got %q", got.FromAddress)
	}
	if got.ExecutionBackend != execution.ExecutionBackendLegacyLocal {
		t.Fatalf("expected backend %q, got %q", execution.ExecutionBackendLegacyLocal, got.ExecutionBackend)
	}
	if len(got.Warnings) != 1 || !strings.Contains(strings.ToLower(got.Warnings[0]), "deprecated") {
		t.Fatalf("expected one deprecation warning, got %#v", got.Warnings)
	}
}

func TestResolveExecutionIdentityRejectsWalletAndFromAddressTogether(t *testing.T) {
	_, err := resolveExecutionIdentity("wallet-123", "0x000000000000000000000000000000000000dEaD", "1")
	if err == nil {
		t.Fatal("expected resolveExecutionIdentity to fail")
	}
	typed, ok := clierr.As(err)
	if !ok {
		t.Fatalf("expected cli error type, got %T: %v", err, err)
	}
	if typed.Code != clierr.CodeUsage {
		t.Fatalf("expected usage error code, got %d", typed.Code)
	}
}

func TestResolveExecutionIdentityRejectsMissingIdentity(t *testing.T) {
	_, err := resolveExecutionIdentity("", "", "1")
	if err == nil {
		t.Fatal("expected resolveExecutionIdentity to fail")
	}
	typed, ok := clierr.As(err)
	if !ok {
		t.Fatalf("expected cli error type, got %T: %v", err, err)
	}
	if typed.Code != clierr.CodeUsage {
		t.Fatalf("expected usage error code, got %d", typed.Code)
	}
}

func TestResolveExecutionIdentityRejectsWalletOnTempoChain(t *testing.T) {
	_, err := resolveExecutionIdentity("wallet-123", "", "tempo")
	if err == nil {
		t.Fatal("expected resolveExecutionIdentity to fail")
	}
	typed, ok := clierr.As(err)
	if !ok {
		t.Fatalf("expected cli error type, got %T: %v", err, err)
	}
	if typed.Code != clierr.CodeUnsupported {
		t.Fatalf("expected unsupported error code, got %d", typed.Code)
	}
}

func TestResolveExecutionIdentityRejectsWalletOnNonEVMChain(t *testing.T) {
	_, err := resolveExecutionIdentity("wallet-123", "", "solana")
	if err == nil {
		t.Fatal("expected resolveExecutionIdentity to fail")
	}
	typed, ok := clierr.As(err)
	if !ok {
		t.Fatalf("expected cli error type, got %T: %v", err, err)
	}
	if typed.Code != clierr.CodeUnsupported {
		t.Fatalf("expected unsupported error code, got %d", typed.Code)
	}
}

func TestResolveExecutionIdentityWalletNotFoundIsUsage(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)

	_, err := resolveExecutionIdentity("wallet-does-not-exist", "", "1")
	if err == nil {
		t.Fatal("expected resolveExecutionIdentity to fail")
	}
	typed, ok := clierr.As(err)
	if !ok {
		t.Fatalf("expected cli error type, got %T: %v", err, err)
	}
	if typed.Code != clierr.CodeUsage {
		t.Fatalf("expected usage error code, got %d", typed.Code)
	}
}

func TestResolveExecutionIdentityWalletVaultDecodeFailureIsUnavailable(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	walletsDir := filepath.Join(home, ".ows", "wallets")
	if err := os.MkdirAll(walletsDir, 0o755); err != nil {
		t.Fatalf("mkdir wallets: %v", err)
	}
	if err := os.WriteFile(filepath.Join(walletsDir, "broken.json"), []byte("{"), 0o644); err != nil {
		t.Fatalf("write broken wallet fixture: %v", err)
	}

	_, err := resolveExecutionIdentity("wallet-123", "", "1")
	if err == nil {
		t.Fatal("expected resolveExecutionIdentity to fail")
	}
	typed, ok := clierr.As(err)
	if !ok {
		t.Fatalf("expected cli error type, got %T: %v", err, err)
	}
	if typed.Code != clierr.CodeUnavailable {
		t.Fatalf("expected unavailable error code, got %d", typed.Code)
	}
}

func TestResolveExecutionIdentityRejectsInvalidWalletSender(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	writeOWSWalletFixture(t, home, ows.Wallet{
		ID:        "wallet-123",
		Name:      "Broken Wallet",
		CreatedAt: "2026-03-25T00:00:00Z",
		Accounts: []ows.WalletAccount{
			{
				AccountID:      "acc-1",
				Address:        "not-an-evm-address",
				ChainID:        "eip155:1",
				DerivationPath: "m/44'/60'/0'/0/0",
			},
		},
	})

	_, err := resolveExecutionIdentity("wallet-123", "", "1")
	if err == nil {
		t.Fatal("expected resolveExecutionIdentity to fail")
	}
	typed, ok := clierr.As(err)
	if !ok {
		t.Fatalf("expected cli error type, got %T: %v", err, err)
	}
	if typed.Code != clierr.CodeUnavailable {
		t.Fatalf("expected unavailable error code, got %d", typed.Code)
	}
}

func writeOWSWalletFixture(t *testing.T, home string, wallet ows.Wallet) {
	t.Helper()
	walletsDir := filepath.Join(home, ".ows", "wallets")
	if err := os.MkdirAll(walletsDir, 0o755); err != nil {
		t.Fatalf("mkdir wallets: %v", err)
	}
	path := filepath.Join(walletsDir, wallet.ID+".json")
	data, err := json.MarshalIndent(wallet, "", "  ")
	if err != nil {
		t.Fatalf("marshal wallet: %v", err)
	}
	if err := os.WriteFile(path, data, 0o644); err != nil {
		t.Fatalf("write wallet fixture: %v", err)
	}
}
