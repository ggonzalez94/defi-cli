package signer

import (
	"context"
	"crypto/ecdsa"
	"encoding/json"
	"fmt"
	"math/big"
	"os/exec"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	temposigner "github.com/tempoxyz/tempo-go/pkg/signer"
	"github.com/tempoxyz/tempo-go/pkg/transaction"
)

// TempoSigner extends the base Signer with Tempo-specific capabilities.
// Implementations can return the smart-wallet address (WalletAddress) that
// differs from the signing key address (Address).
type TempoSigner interface {
	Signer
	WalletAddress() common.Address
	SignTempoTx(tx *transaction.Tx) error
	TempoGoSigner() *temposigner.Signer
}

// TempoWalletSigner wraps a tempo-go signer and associates it with a
// smart-wallet address. The signing key address (Address) is the EOA
// that signs transactions, while WalletAddress is the smart-wallet
// that acts as the on-chain sender.
type TempoWalletSigner struct {
	walletAddr common.Address
	inner      *temposigner.Signer
	keyAddr    common.Address
}

// NewTempoWalletSigner creates a TempoWalletSigner from a wallet address and
// a hex-encoded private key. The wallet address is the smart-wallet that
// owns the signing key.
func NewTempoWalletSigner(walletAddr common.Address, privateKeyHex string) (*TempoWalletSigner, error) {
	clean := strings.TrimPrefix(strings.TrimSpace(privateKeyHex), "0x")
	inner, err := temposigner.NewSigner("0x" + clean)
	if err != nil {
		return nil, fmt.Errorf("create tempo signer: %w", err)
	}
	return &TempoWalletSigner{
		walletAddr: walletAddr,
		inner:      inner,
		keyAddr:    inner.Address(),
	}, nil
}

// Address returns the signing key EOA address.
func (s *TempoWalletSigner) Address() common.Address { return s.keyAddr }

// WalletAddress returns the smart-wallet address that acts as the on-chain sender.
func (s *TempoWalletSigner) WalletAddress() common.Address { return s.walletAddr }

// TempoGoSigner returns the underlying tempo-go signer for direct SDK usage.
func (s *TempoWalletSigner) TempoGoSigner() *temposigner.Signer { return s.inner }

// SignTempoTx signs a Tempo transaction using the underlying tempo-go signer.
func (s *TempoWalletSigner) SignTempoTx(tx *transaction.Tx) error {
	return transaction.SignTransaction(tx, s.inner)
}

// SignTx is not supported for TempoWalletSigner. Tempo chains use type 0x76
// transactions which must be signed via SignTempoTx.
func (s *TempoWalletSigner) SignTx(_ *big.Int, _ *types.Transaction) (*types.Transaction, error) {
	return nil, fmt.Errorf("TempoWalletSigner does not support EVM SignTx; use SignTempoTx for Tempo chains")
}

// PrivateKey returns nil for TempoWalletSigner (key is managed internally
// by the tempo-go signer; callers should use TempoGoSigner() instead).
func (s *TempoWalletSigner) PrivateKey() *ecdsa.PrivateKey { return nil }

// tempoWhoamiResponse models the JSON output of `tempo wallet -j whoami`.
type tempoWhoamiResponse struct {
	Ready  bool   `json:"ready"`
	Wallet string `json:"wallet"`
	Key    struct {
		Address       string `json:"address"`
		Key           string `json:"key"`
		ChainID       int    `json:"chain_id"`
		SpendingLimit struct {
			Remaining string `json:"remaining"`
		} `json:"spending_limit"`
		ExpiresAt string `json:"expires_at"`
	} `json:"key"`
}

// NewTempoSignerFromCLI discovers Tempo wallet configuration by shelling out
// to the `tempo` CLI (`tempo wallet -j whoami`). It returns a configured
// TempoWalletSigner, any non-fatal warnings (e.g. key nearing expiry), or
// an error if the wallet is not ready.
func NewTempoSignerFromCLI() (*TempoWalletSigner, []string, error) {
	tempoBin, err := exec.LookPath("tempo")
	if err != nil {
		return nil, nil, fmt.Errorf("tempo CLI is required for --signer tempo. Install: curl -fsSL https://tempo.xyz/install | sh")
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	out, err := exec.CommandContext(ctx, tempoBin, "wallet", "-j", "whoami").Output()
	if err != nil {
		return nil, nil, fmt.Errorf("failed to query tempo wallet: %w", err)
	}

	var resp tempoWhoamiResponse
	if err := json.Unmarshal(out, &resp); err != nil {
		return nil, nil, fmt.Errorf("parse tempo wallet output: %w", err)
	}

	if !resp.Ready {
		return nil, nil, fmt.Errorf("tempo wallet is not logged in; run 'tempo wallet login' to set up your agent wallet")
	}

	var warnings []string
	if resp.Key.ExpiresAt != "" {
		if expiry, parseErr := time.Parse(time.RFC3339, resp.Key.ExpiresAt); parseErr == nil {
			if time.Now().After(expiry) {
				return nil, nil, fmt.Errorf("tempo wallet access key has expired; run 'tempo wallet login' to refresh")
			}
			if time.Until(expiry) < 24*time.Hour {
				warnings = append(warnings, fmt.Sprintf("tempo wallet key expires in %s", time.Until(expiry).Round(time.Hour)))
			}
		}
	}

	walletAddr := common.HexToAddress(resp.Wallet)
	s, err := NewTempoWalletSigner(walletAddr, resp.Key.Key)
	if err != nil {
		return nil, nil, err
	}
	return s, warnings, nil
}
