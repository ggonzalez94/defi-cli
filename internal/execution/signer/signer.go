package signer

import (
	"math/big"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
)

type Signer interface {
	Address() common.Address
	SignTx(chainID *big.Int, tx *types.Transaction) (*types.Transaction, error)
}
