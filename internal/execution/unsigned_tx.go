package execution

import (
	"fmt"
	"math/big"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/rlp"
)

type unsignedDynamicFeePayload struct {
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

type unsignedAccessListPayload struct {
	ChainID    *big.Int
	Nonce      uint64
	GasPrice   *big.Int
	Gas        uint64
	To         *common.Address `rlp:"nil"`
	Value      *big.Int
	Data       []byte
	AccessList types.AccessList
}

// EncodeUnsignedTypedTx encodes the unsigned typed-transaction envelope used
// for external signing via OWS. Signature fields are intentionally omitted.
func EncodeUnsignedTypedTx(tx *types.Transaction) ([]byte, error) {
	if tx == nil {
		return nil, fmt.Errorf("transaction is required")
	}

	switch tx.Type() {
	case types.DynamicFeeTxType:
		payload, err := rlp.EncodeToBytes(unsignedDynamicFeePayload{
			ChainID:   tx.ChainId(),
			Nonce:     tx.Nonce(),
			GasTipCap: tx.GasTipCap(),
			GasFeeCap: tx.GasFeeCap(),
			Gas:       tx.Gas(),
			To:        tx.To(),
			Value:     tx.Value(),
			Data:      tx.Data(),
			Access:    tx.AccessList(),
		})
		if err != nil {
			return nil, fmt.Errorf("encode unsigned dynamic fee tx: %w", err)
		}
		return append([]byte{types.DynamicFeeTxType}, payload...), nil
	case types.AccessListTxType:
		payload, err := rlp.EncodeToBytes(unsignedAccessListPayload{
			ChainID:    tx.ChainId(),
			Nonce:      tx.Nonce(),
			GasPrice:   tx.GasPrice(),
			Gas:        tx.Gas(),
			To:         tx.To(),
			Value:      tx.Value(),
			Data:       tx.Data(),
			AccessList: tx.AccessList(),
		})
		if err != nil {
			return nil, fmt.Errorf("encode unsigned access-list tx: %w", err)
		}
		return append([]byte{types.AccessListTxType}, payload...), nil
	default:
		return nil, fmt.Errorf("unsupported transaction type: 0x%x", tx.Type())
	}
}
