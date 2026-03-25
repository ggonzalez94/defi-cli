package execution

import (
	"bytes"
	"math/big"
	"strings"
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/rlp"
)

type unsignedDynamicFeeEnvelope struct {
	ChainID   *big.Int
	Nonce     uint64
	GasTipCap *big.Int
	GasFeeCap *big.Int
	Gas       uint64
	To        *common.Address `rlp:"nil"`
	Value     *big.Int
	Data      []byte
	Access    types.AccessList
}

type unsignedAccessListEnvelope struct {
	ChainID    *big.Int
	Nonce      uint64
	GasPrice   *big.Int
	Gas        uint64
	To         *common.Address `rlp:"nil"`
	Value      *big.Int
	Data       []byte
	AccessList types.AccessList
}

func TestEncodeUnsignedDynamicFeeTx(t *testing.T) {
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
		ChainID:    big.NewInt(1),
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

	payload, err := rlp.EncodeToBytes(unsignedDynamicFeeEnvelope{
		ChainID:   big.NewInt(1),
		Nonce:     7,
		GasTipCap: big.NewInt(2_000_000_000),
		GasFeeCap: big.NewInt(30_000_000_000),
		Gas:       21000,
		To:        &to,
		Value:     big.NewInt(12345),
		Data:      []byte{0x12, 0x34},
		Access:    accessList,
	})
	if err != nil {
		t.Fatalf("rlp encode expected payload: %v", err)
	}
	want := append([]byte{types.DynamicFeeTxType}, payload...)
	if !bytes.Equal(got, want) {
		t.Fatalf("unexpected unsigned encoding: got %x want %x", got, want)
	}
}

func TestEncodeUnsignedAccessListTx(t *testing.T) {
	to := common.HexToAddress("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
	accessList := types.AccessList{
		{
			Address: common.HexToAddress("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
		},
	}
	tx := types.NewTx(&types.AccessListTx{
		ChainID:    big.NewInt(10),
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

	payload, err := rlp.EncodeToBytes(unsignedAccessListEnvelope{
		ChainID:    big.NewInt(10),
		Nonce:      19,
		GasPrice:   big.NewInt(3_000_000_000),
		Gas:        85_000,
		To:         &to,
		Value:      big.NewInt(77),
		Data:       []byte{0xde, 0xad, 0xbe, 0xef},
		AccessList: accessList,
	})
	if err != nil {
		t.Fatalf("rlp encode expected payload: %v", err)
	}
	want := append([]byte{types.AccessListTxType}, payload...)
	if !bytes.Equal(got, want) {
		t.Fatalf("unexpected unsigned encoding: got %x want %x", got, want)
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
