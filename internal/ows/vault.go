package ows

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/ggonzalez94/defi-cli/internal/fsutil"
)

type Wallet struct {
	ID        string          `json:"id"`
	Name      string          `json:"name"`
	CreatedAt string          `json:"created_at"`
	Accounts  []WalletAccount `json:"accounts"`
}

type WalletAccount struct {
	AccountID      string `json:"account_id"`
	Address        string `json:"address"`
	ChainID        string `json:"chain_id"`
	DerivationPath string `json:"derivation_path"`
}

func ResolveWalletRef(vaultDir, ref string) (Wallet, error) {
	ref = strings.TrimSpace(ref)
	if ref == "" {
		return Wallet{}, errors.New("wallet reference is required")
	}

	vaultPath, err := resolveVaultPath(vaultDir)
	if err != nil {
		return Wallet{}, err
	}

	wallets, err := loadWallets(vaultPath)
	if err != nil {
		return Wallet{}, err
	}

	var idMatches []Wallet
	for _, wallet := range wallets {
		if wallet.ID == ref {
			idMatches = append(idMatches, wallet)
		}
	}
	switch len(idMatches) {
	case 1:
		return idMatches[0], nil
	case 0:
		// fall through to name matching
	default:
		return Wallet{}, fmt.Errorf("ambiguous wallet id %q", ref)
	}

	var nameMatches []Wallet
	for _, wallet := range wallets {
		if wallet.Name == ref {
			nameMatches = append(nameMatches, wallet)
		}
	}
	switch len(nameMatches) {
	case 1:
		return nameMatches[0], nil
	case 0:
		return Wallet{}, fmt.Errorf("wallet %q not found", ref)
	default:
		return Wallet{}, fmt.Errorf("ambiguous wallet name %q", ref)
	}
}

func SenderAddressForChain(wallet Wallet, chainID string) (string, error) {
	chainID = strings.TrimSpace(chainID)
	if chainID == "" {
		return "", errors.New("chain id is required")
	}

	for _, account := range wallet.Accounts {
		if account.ChainID == chainID && account.Address != "" {
			return account.Address, nil
		}
	}

	if strings.HasPrefix(chainID, "eip155:") {
		for _, account := range wallet.Accounts {
			if strings.HasPrefix(account.ChainID, "eip155:") && account.Address != "" {
				return account.Address, nil
			}
		}
	}

	return "", fmt.Errorf("wallet %q has no account for chain %q", wallet.ID, chainID)
}

func resolveVaultPath(vaultDir string) (string, error) {
	if strings.TrimSpace(vaultDir) == "" {
		vaultDir = "~/.ows"
	}
	return fsutil.NormalizePath(vaultDir)
}

func loadWallets(vaultPath string) ([]Wallet, error) {
	pattern := filepath.Join(vaultPath, "wallets", "*.json")
	matches, err := filepath.Glob(pattern)
	if err != nil {
		return nil, fmt.Errorf("list wallet metadata: %w", err)
	}
	wallets := make([]Wallet, 0, len(matches))
	for _, path := range matches {
		data, err := os.ReadFile(path)
		if err != nil {
			return nil, fmt.Errorf("read wallet metadata %s: %w", path, err)
		}
		var wallet Wallet
		if err := json.Unmarshal(data, &wallet); err != nil {
			return nil, fmt.Errorf("decode wallet metadata %s: %w", path, err)
		}
		wallets = append(wallets, wallet)
	}
	return wallets, nil
}
