package execution

import (
	"crypto/ecdsa"
	"math/big"
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/crypto"
)

// testTempoSigner implements signer.Signer and privateKeyProvider for testing.
type testTempoSigner struct {
	pk   *ecdsa.PrivateKey
	addr common.Address
}

func newTestTempoSigner(t *testing.T) *testTempoSigner {
	t.Helper()
	pk, err := crypto.HexToECDSA("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80")
	if err != nil {
		t.Fatalf("parse test key: %v", err)
	}
	return &testTempoSigner{
		pk:   pk,
		addr: crypto.PubkeyToAddress(pk.PublicKey),
	}
}

func (s *testTempoSigner) Address() common.Address       { return s.addr }
func (s *testTempoSigner) PrivateKey() *ecdsa.PrivateKey { return s.pk }
func (s *testTempoSigner) SignTx(chainID *big.Int, tx *types.Transaction) (*types.Transaction, error) {
	signer := types.LatestSignerForChainID(chainID)
	return types.SignTx(tx, signer, s.pk)
}

func TestTempoStepExecutorEffectiveSender(t *testing.T) {
	ts := newTestTempoSigner(t)
	exec := NewTempoStepExecutor(ts)
	defer exec.Close()

	got := exec.EffectiveSender()
	if got != ts.addr {
		t.Fatalf("expected EffectiveSender %s, got %s", ts.addr.Hex(), got.Hex())
	}
}

func TestTempoStepExecutorCreatesTempoSigner(t *testing.T) {
	ts := newTestTempoSigner(t)
	exec := NewTempoStepExecutor(ts)
	defer exec.Close()

	if exec.tempoSigner == nil {
		t.Fatal("expected tempo signer to be created from private key provider")
	}
	if exec.tempoSigner.Address() != ts.addr {
		t.Fatalf("expected tempo signer address %s, got %s", ts.addr.Hex(), exec.tempoSigner.Address().Hex())
	}
}

func TestTempoStepExecutorRejectsNilSigner(t *testing.T) {
	// A signer that does not implement privateKeyProvider.
	exec := NewTempoStepExecutor(&noPrivateKeySigner{})
	defer exec.Close()

	if exec.tempoSigner != nil {
		t.Fatal("expected nil tempo signer for non-key-provider")
	}
}

// noPrivateKeySigner implements signer.Signer but not privateKeyProvider.
type noPrivateKeySigner struct{}

func (s *noPrivateKeySigner) Address() common.Address { return common.Address{} }
func (s *noPrivateKeySigner) SignTx(_ *big.Int, tx *types.Transaction) (*types.Transaction, error) {
	return tx, nil
}
