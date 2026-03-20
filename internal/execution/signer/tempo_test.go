package signer

import (
	"math/big"
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/crypto"
	"github.com/tempoxyz/tempo-go/pkg/transaction"
)

func TestNewTempoWalletSigner(t *testing.T) {
	// Generate a deterministic test key.
	pk, err := crypto.GenerateKey()
	if err != nil {
		t.Fatalf("generate key: %v", err)
	}
	keyHex := "0x" + common.Bytes2Hex(crypto.FromECDSA(pk))
	walletAddr := common.HexToAddress("0x1111111111111111111111111111111111111111")

	s, err := NewTempoWalletSigner(walletAddr, keyHex)
	if err != nil {
		t.Fatalf("NewTempoWalletSigner: %v", err)
	}
	if s.WalletAddress() != walletAddr {
		t.Fatalf("expected wallet address %s, got %s", walletAddr.Hex(), s.WalletAddress().Hex())
	}
	expectedKeyAddr := crypto.PubkeyToAddress(pk.PublicKey)
	if s.Address() != expectedKeyAddr {
		t.Fatalf("expected key address %s, got %s", expectedKeyAddr.Hex(), s.Address().Hex())
	}
}

func TestTempoWalletSignerWalletAddressDiffersFromKeyAddress(t *testing.T) {
	pk, err := crypto.GenerateKey()
	if err != nil {
		t.Fatalf("generate key: %v", err)
	}
	keyHex := common.Bytes2Hex(crypto.FromECDSA(pk))
	walletAddr := common.HexToAddress("0x2222222222222222222222222222222222222222")

	s, err := NewTempoWalletSigner(walletAddr, keyHex)
	if err != nil {
		t.Fatalf("NewTempoWalletSigner: %v", err)
	}
	if s.WalletAddress() == s.Address() {
		t.Fatal("expected wallet address to differ from key address")
	}
}

func TestTempoWalletSignerSignTempoTx(t *testing.T) {
	pk, err := crypto.GenerateKey()
	if err != nil {
		t.Fatalf("generate key: %v", err)
	}
	keyHex := "0x" + common.Bytes2Hex(crypto.FromECDSA(pk))
	walletAddr := common.HexToAddress("0x3333333333333333333333333333333333333333")

	s, err := NewTempoWalletSigner(walletAddr, keyHex)
	if err != nil {
		t.Fatalf("NewTempoWalletSigner: %v", err)
	}

	target := common.HexToAddress("0x4444444444444444444444444444444444444444")
	tx := transaction.NewBuilder(big.NewInt(4217)).
		SetGas(21000).
		SetMaxFeePerGas(big.NewInt(1000000000)).
		SetMaxPriorityFeePerGas(big.NewInt(100000000)).
		SetNonce(0).
		AddCall(target, big.NewInt(0), []byte{0x01, 0x02}).
		Build()

	if err := s.SignTempoTx(tx); err != nil {
		t.Fatalf("SignTempoTx: %v", err)
	}
	if tx.Signature == nil {
		t.Fatal("expected tx to have signature after signing")
	}

	// Verify signature recovers to the key address.
	recovered, err := transaction.VerifySignature(tx)
	if err != nil {
		t.Fatalf("VerifySignature: %v", err)
	}
	if recovered != s.Address() {
		t.Fatalf("expected recovered address %s, got %s", s.Address().Hex(), recovered.Hex())
	}
}

func TestTempoWalletSignerRejectsEVMSignTx(t *testing.T) {
	pk, err := crypto.GenerateKey()
	if err != nil {
		t.Fatalf("generate key: %v", err)
	}
	keyHex := common.Bytes2Hex(crypto.FromECDSA(pk))
	walletAddr := common.HexToAddress("0x5555555555555555555555555555555555555555")

	s, err := NewTempoWalletSigner(walletAddr, keyHex)
	if err != nil {
		t.Fatalf("NewTempoWalletSigner: %v", err)
	}

	if _, err := s.SignTx(big.NewInt(1), nil); err == nil {
		t.Fatal("expected SignTx to return error for TempoWalletSigner")
	}
}

func TestTempoWalletSignerPrivateKeyReturnsNil(t *testing.T) {
	pk, err := crypto.GenerateKey()
	if err != nil {
		t.Fatalf("generate key: %v", err)
	}
	keyHex := common.Bytes2Hex(crypto.FromECDSA(pk))
	walletAddr := common.HexToAddress("0x6666666666666666666666666666666666666666")

	s, err := NewTempoWalletSigner(walletAddr, keyHex)
	if err != nil {
		t.Fatalf("NewTempoWalletSigner: %v", err)
	}

	if s.PrivateKey() != nil {
		t.Fatal("expected PrivateKey() to return nil for TempoWalletSigner")
	}
}

func TestTempoWalletSignerTempoGoSigner(t *testing.T) {
	pk, err := crypto.GenerateKey()
	if err != nil {
		t.Fatalf("generate key: %v", err)
	}
	keyHex := common.Bytes2Hex(crypto.FromECDSA(pk))
	walletAddr := common.HexToAddress("0x7777777777777777777777777777777777777777")

	s, err := NewTempoWalletSigner(walletAddr, keyHex)
	if err != nil {
		t.Fatalf("NewTempoWalletSigner: %v", err)
	}

	inner := s.TempoGoSigner()
	if inner == nil {
		t.Fatal("expected TempoGoSigner to return non-nil signer")
	}
	if inner.Address() != s.Address() {
		t.Fatalf("expected inner signer address %s, got %s", s.Address().Hex(), inner.Address().Hex())
	}
}

func TestNewTempoWalletSignerRejectsInvalidKey(t *testing.T) {
	walletAddr := common.HexToAddress("0x8888888888888888888888888888888888888888")
	if _, err := NewTempoWalletSigner(walletAddr, "not-a-valid-hex-key"); err == nil {
		t.Fatal("expected error for invalid private key")
	}
}
