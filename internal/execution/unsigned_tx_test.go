package execution

import (
	"math/big"
	"strings"
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/crypto"
)

func TestEncodeUnsignedDynamicFeeTx(t *testing.T) {
	chainID := big.NewInt(1)
	to := common.HexToAddress("0x1111111111111111111111111111111111111111")
	accessList := types.AccessList{
		{
			Address: common.HexToAddress("0x2222222222222222222222222222222222222222"),
			StorageKeys: []common.Hash{
				common.HexToHash("0x01"),
			},
		},
	}
	tx := types.NewTx(&types.DynamicFeeTx{
		ChainID:    chainID,
		Nonce:      7,
		GasTipCap:  big.NewInt(2_000_000_000),
		GasFeeCap:  big.NewInt(30_000_000_000),
		Gas:        21000,
		To:         &to,
		Value:      big.NewInt(12345),
		Data:       []byte{0x12, 0x34},
		AccessList: accessList,
	})

	got, err := EncodeUnsignedTypedTx(tx)
	if err != nil {
		t.Fatalf("EncodeUnsignedTypedTx failed: %v", err)
	}

	if len(got) == 0 || got[0] != types.DynamicFeeTxType {
		t.Fatalf("expected type prefix 0x%02x, got %x", types.DynamicFeeTxType, got)
	}

	gotHash := crypto.Keccak256Hash(got)
	wantHash := types.NewLondonSigner(chainID).Hash(tx)
	if gotHash != wantHash {
		t.Fatalf("unexpected signing hash: got %s want %s", gotHash.Hex(), wantHash.Hex())
	}
}

func TestEncodeUnsignedAccessListTx(t *testing.T) {
	chainID := big.NewInt(10)
	to := common.HexToAddress("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
	accessList := types.AccessList{
		{
			Address: common.HexToAddress("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
		},
	}
	tx := types.NewTx(&types.AccessListTx{
		ChainID:    chainID,
		Nonce:      19,
		GasPrice:   big.NewInt(3_000_000_000),
		Gas:        85_000,
		To:         &to,
		Value:      big.NewInt(77),
		Data:       []byte{0xde, 0xad, 0xbe, 0xef},
		AccessList: accessList,
	})

	got, err := EncodeUnsignedTypedTx(tx)
	if err != nil {
		t.Fatalf("EncodeUnsignedTypedTx failed: %v", err)
	}

	if len(got) == 0 || got[0] != types.AccessListTxType {
		t.Fatalf("expected type prefix 0x%02x, got %x", types.AccessListTxType, got)
	}

	gotHash := crypto.Keccak256Hash(got)
	wantHash := types.NewLondonSigner(chainID).Hash(tx)
	if gotHash != wantHash {
		t.Fatalf("unexpected signing hash: got %s want %s", gotHash.Hex(), wantHash.Hex())
	}
}

func TestEncodeUnsignedTypedTxRejectsLegacyTx(t *testing.T) {
	to := common.HexToAddress("0x3333333333333333333333333333333333333333")
	tx := types.NewTx(&types.LegacyTx{
		Nonce:    1,
		To:       &to,
		Value:    big.NewInt(1),
		Gas:      21_000,
		GasPrice: big.NewInt(1_000_000_000),
	})

	_, err := EncodeUnsignedTypedTx(tx)
	if err == nil {
		t.Fatal("expected legacy tx rejection")
	}
	if !strings.Contains(err.Error(), "unsupported transaction type") {
		t.Fatalf("expected unsupported transaction type error, got: %v", err)
	}
}
