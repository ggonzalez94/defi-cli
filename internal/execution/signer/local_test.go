package signer

import (
	"math/big"
	"os"
	"path/filepath"
	"strings"
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

func TestNewLocalSignerFromEnvFileAllowsNonStrictPermissions(t *testing.T) {
	dir := t.TempDir()
	keyFile := filepath.Join(dir, "key.txt")
	if err := os.WriteFile(keyFile, []byte(testPrivateKey), 0o644); err != nil {
		t.Fatalf("write key file: %v", err)
	}
	t.Setenv(EnvPrivateKeyFile, keyFile)
	if _, err := NewLocalSignerFromEnv(KeySourceFile); err != nil {
		t.Fatalf("expected non-strict permission key file to load: %v", err)
	}
}

func TestNewLocalSignerFromEnvAutoUsesDefaultKeyFile(t *testing.T) {
	cfgDir := t.TempDir()
	keyDir := filepath.Join(cfgDir, "defi")
	keyFile := filepath.Join(keyDir, "key.hex")
	if err := os.MkdirAll(keyDir, 0o755); err != nil {
		t.Fatalf("create config dir: %v", err)
	}
	if err := os.WriteFile(keyFile, []byte(testPrivateKey), 0o644); err != nil {
		t.Fatalf("write key file: %v", err)
	}
	t.Setenv("XDG_CONFIG_HOME", cfgDir)
	t.Setenv(EnvPrivateKey, "")
	t.Setenv(EnvPrivateKeyFile, "")
	t.Setenv(EnvKeystorePath, "")

	s, err := NewLocalSignerFromEnv(KeySourceAuto)
	if err != nil {
		t.Fatalf("expected auto key-source to use default key path: %v", err)
	}
	if s.Address() == (common.Address{}) {
		t.Fatal("expected non-zero signer address")
	}
}

func TestNewLocalSignerFromInputsPrivateKeyOverride(t *testing.T) {
	t.Setenv(EnvPrivateKey, "")
	t.Setenv(EnvPrivateKeyFile, "")
	t.Setenv(EnvKeystorePath, "")

	s, err := NewLocalSignerFromInputs(KeySourceAuto, testPrivateKey)
	if err != nil {
		t.Fatalf("expected private key override to initialize signer: %v", err)
	}
	if s.Address() == (common.Address{}) {
		t.Fatal("expected non-zero signer address")
	}
}

func TestNewLocalSignerFromInputsOverrideWinsOverFileSource(t *testing.T) {
	t.Setenv(EnvPrivateKeyFile, "/tmp/does-not-exist")
	s, err := NewLocalSignerFromInputs(KeySourceFile, testPrivateKey)
	if err != nil {
		t.Fatalf("expected private key override to win over file key-source: %v", err)
	}
	if s.Address() == (common.Address{}) {
		t.Fatal("expected non-zero signer address")
	}
}

func TestDefaultPrivateKeyPathUsesXDGConfigHome(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", "/tmp/defi-config-home")
	got := defaultPrivateKeyPath()
	want := "/tmp/defi-config-home/defi/key.hex"
	if got != want {
		t.Fatalf("expected %q, got %q", want, got)
	}
}

func TestNewLocalSignerFromInputsMissingKeyErrorIncludesSimplePathHint(t *testing.T) {
	t.Setenv(EnvPrivateKey, "")
	t.Setenv(EnvPrivateKeyFile, "")
	t.Setenv(EnvKeystorePath, "")
	t.Setenv(EnvKeystorePassword, "")
	t.Setenv(EnvKeystorePasswordFile, "")

	_, err := NewLocalSignerFromInputs(KeySourceAuto, "")
	if err == nil {
		t.Fatal("expected missing key error")
	}
	msg := err.Error()
	if !strings.Contains(msg, defaultPrivateKeyHintPath) {
		t.Fatalf("expected missing key message to include %q, got: %s", defaultPrivateKeyHintPath, msg)
	}
	if !strings.Contains(msg, "--private-key") {
		t.Fatalf("expected missing key message to include --private-key, got: %s", msg)
	}
}

func ptrAddress(v common.Address) *common.Address { return &v }
