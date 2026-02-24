package signer

import (
	"math/big"
	"os"
	"path/filepath"
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
)

const testPrivateKey = "59c6995e998f97a5a0044976f0945388cf9b7e5e5f4f9d2d9d8f1f5b7f6d11d1"

func TestNewLocalSignerFromEnvHex(t *testing.T) {
	t.Setenv(EnvPrivateKey, testPrivateKey)
	s, err := NewLocalSignerFromEnv(KeySourceEnv)
	if err != nil {
		t.Fatalf("NewLocalSignerFromEnv failed: %v", err)
	}
	if s.Address() == (common.Address{}) {
		t.Fatal("expected non-zero signer address")
	}
	tx := types.NewTx(&types.LegacyTx{
		Nonce:    0,
		To:       ptrAddress(common.HexToAddress("0x0000000000000000000000000000000000000001")),
		Value:    big.NewInt(0),
		Gas:      21_000,
		GasPrice: big.NewInt(1),
	})
	if _, err := s.SignTx(common.Big1, tx); err != nil {
		t.Fatalf("SignTx failed: %v", err)
	}
}

func TestNewLocalSignerFromEnvFile(t *testing.T) {
	dir := t.TempDir()
	keyFile := filepath.Join(dir, "key.txt")
	if err := os.WriteFile(keyFile, []byte(testPrivateKey), 0o600); err != nil {
		t.Fatalf("write key file: %v", err)
	}
	t.Setenv(EnvPrivateKeyFile, keyFile)

	s, err := NewLocalSignerFromEnv(KeySourceFile)
	if err != nil {
		t.Fatalf("NewLocalSignerFromEnv failed: %v", err)
	}
	if s.Address() == (common.Address{}) {
		t.Fatal("expected non-zero signer address")
	}
}

func TestNewLocalSignerRejectsInsecurePermissions(t *testing.T) {
	dir := t.TempDir()
	keyFile := filepath.Join(dir, "key.txt")
	if err := os.WriteFile(keyFile, []byte(testPrivateKey), 0o644); err != nil {
		t.Fatalf("write key file: %v", err)
	}
	t.Setenv(EnvPrivateKeyFile, keyFile)
	if _, err := NewLocalSignerFromEnv(KeySourceFile); err == nil {
		t.Fatal("expected insecure permissions error")
	}
}

func ptrAddress(v common.Address) *common.Address { return &v }
