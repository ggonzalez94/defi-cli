package signer

import (
	"crypto/ecdsa"
	"errors"
	"fmt"
	"math/big"
	"os"
	"path/filepath"
	"strings"

	"github.com/ethereum/go-ethereum/accounts/keystore"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/crypto"
)

const (
	EnvPrivateKey           = "DEFI_PRIVATE_KEY"
	EnvPrivateKeyFile       = "DEFI_PRIVATE_KEY_FILE"
	EnvKeystorePath         = "DEFI_KEYSTORE_PATH"
	EnvKeystorePassword     = "DEFI_KEYSTORE_PASSWORD"
	EnvKeystorePasswordFile = "DEFI_KEYSTORE_PASSWORD_FILE"

	KeySourceAuto     = "auto"
	KeySourceEnv      = "env"
	KeySourceFile     = "file"
	KeySourceKeystore = "keystore"

	defaultPrivateKeyRelativePath = "defi/key.hex"
)

type LocalSigner struct {
	privateKey *ecdsa.PrivateKey
	address    common.Address
}

func (s *LocalSigner) Address() common.Address {
	return s.address
}

func (s *LocalSigner) SignTx(chainID *big.Int, tx *types.Transaction) (*types.Transaction, error) {
	if s == nil || s.privateKey == nil {
		return nil, errors.New("local signer is not initialized")
	}
	signer := types.LatestSignerForChainID(chainID)
	return types.SignTx(tx, signer, s.privateKey)
}

func NewLocalSignerFromEnv(source string) (*LocalSigner, error) {
	return NewLocalSignerFromInputs(source, "")
}

func NewLocalSignerFromInputs(source, privateKeyOverride string) (*LocalSigner, error) {
	source = strings.ToLower(strings.TrimSpace(source))
	if source == "" {
		source = KeySourceAuto
	}
	privateKeyHex := strings.TrimSpace(os.Getenv(EnvPrivateKey))
	privateKeyFile := strings.TrimSpace(os.Getenv(EnvPrivateKeyFile))
	keystorePath := strings.TrimSpace(os.Getenv(EnvKeystorePath))
	keystorePassword := strings.TrimSpace(os.Getenv(EnvKeystorePassword))
	keystorePasswordFile := strings.TrimSpace(os.Getenv(EnvKeystorePasswordFile))
	if privateKeyFile == "" {
		privateKeyFile = discoverDefaultPrivateKeyFile()
	}

	switch source {
	case KeySourceAuto:
		// Keep all values to preserve precedence in loadPrivateKey.
	case KeySourceEnv:
		privateKeyFile = ""
		keystorePath = ""
		keystorePassword = ""
		keystorePasswordFile = ""
	case KeySourceFile:
		privateKeyHex = ""
		keystorePath = ""
		keystorePassword = ""
		keystorePasswordFile = ""
	case KeySourceKeystore:
		privateKeyHex = ""
		privateKeyFile = ""
	default:
		return nil, fmt.Errorf("unsupported key source %q (expected %s|%s|%s|%s)", source, KeySourceAuto, KeySourceEnv, KeySourceFile, KeySourceKeystore)
	}
	if strings.TrimSpace(privateKeyOverride) != "" {
		privateKeyHex = strings.TrimSpace(privateKeyOverride)
		privateKeyFile = ""
		keystorePath = ""
		keystorePassword = ""
		keystorePasswordFile = ""
	}

	return NewLocalSigner(LocalSignerConfig{
		PrivateKeyHex:        privateKeyHex,
		PrivateKeyFile:       privateKeyFile,
		KeystorePath:         keystorePath,
		KeystorePassword:     keystorePassword,
		KeystorePasswordFile: keystorePasswordFile,
	})
}

type LocalSignerConfig struct {
	PrivateKeyHex        string
	PrivateKeyFile       string
	KeystorePath         string
	KeystorePassword     string
	KeystorePasswordFile string
}

func NewLocalSigner(cfg LocalSignerConfig) (*LocalSigner, error) {
	pk, err := loadPrivateKey(cfg)
	if err != nil {
		return nil, err
	}
	pub, ok := pk.Public().(*ecdsa.PublicKey)
	if !ok {
		return nil, fmt.Errorf("invalid ECDSA public key")
	}
	addr := crypto.PubkeyToAddress(*pub)
	return &LocalSigner{privateKey: pk, address: addr}, nil
}

func loadPrivateKey(cfg LocalSignerConfig) (*ecdsa.PrivateKey, error) {
	if strings.TrimSpace(cfg.PrivateKeyHex) != "" {
		return parseHexKey(cfg.PrivateKeyHex)
	}
	if strings.TrimSpace(cfg.PrivateKeyFile) != "" {
		buf, err := os.ReadFile(cfg.PrivateKeyFile)
		if err != nil {
			return nil, fmt.Errorf("read private key file: %w", err)
		}
		return parseHexKey(string(buf))
	}
	if strings.TrimSpace(cfg.KeystorePath) != "" {
		password := cfg.KeystorePassword
		if strings.TrimSpace(password) == "" && strings.TrimSpace(cfg.KeystorePasswordFile) != "" {
			buf, err := os.ReadFile(cfg.KeystorePasswordFile)
			if err != nil {
				return nil, fmt.Errorf("read keystore password file: %w", err)
			}
			password = strings.TrimSpace(string(buf))
		}
		if strings.TrimSpace(password) == "" {
			return nil, fmt.Errorf("keystore password is required")
		}
		buf, err := os.ReadFile(cfg.KeystorePath)
		if err != nil {
			return nil, fmt.Errorf("read keystore file: %w", err)
		}
		key, err := keystore.DecryptKey(buf, password)
		if err != nil {
			return nil, fmt.Errorf("decrypt keystore: %w", err)
		}
		return key.PrivateKey, nil
	}
	return nil, fmt.Errorf("missing signing key: set %s or %s or %s", EnvPrivateKey, EnvPrivateKeyFile, EnvKeystorePath)
}

func parseHexKey(raw string) (*ecdsa.PrivateKey, error) {
	clean := strings.TrimSpace(raw)
	clean = strings.TrimPrefix(clean, "0x")
	if clean == "" {
		return nil, fmt.Errorf("empty private key")
	}
	pk, err := crypto.HexToECDSA(clean)
	if err != nil {
		return nil, fmt.Errorf("parse private key: %w", err)
	}
	return pk, nil
}

func discoverDefaultPrivateKeyFile() string {
	base := strings.TrimSpace(os.Getenv("XDG_CONFIG_HOME"))
	if base == "" {
		home, err := os.UserHomeDir()
		if err != nil || strings.TrimSpace(home) == "" {
			return ""
		}
		base = filepath.Join(home, ".config")
	}
	path := filepath.Join(base, defaultPrivateKeyRelativePath)
	if path == "" {
		return ""
	}
	info, err := os.Stat(path)
	if err != nil {
		return ""
	}
	if info.IsDir() {
		return ""
	}
	return path
}
