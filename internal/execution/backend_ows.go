package execution

import (
	"context"
	"fmt"
	"math/big"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/ows"
)

var sendUnsignedTxFunc = func(ctx context.Context, walletID, chainID string, txHex []byte, rpcURL string) (string, error) {
	result, err := ows.SendUnsignedTx(ctx, walletID, chainID, txHex, rpcURL)
	if err != nil {
		return "", err
	}
	return result.TxHash, nil
}

type owsSubmitBackend struct {
	walletID string
	sender   common.Address
}

func NewOWSSubmitBackend(walletID string, sender common.Address) EVMSubmitBackend {
	return &owsSubmitBackend{
		walletID: walletID,
		sender:   sender,
	}
}

func (b *owsSubmitBackend) EffectiveSender() common.Address {
	if b == nil {
		return common.Address{}
	}
	return b.sender
}

func (b *owsSubmitBackend) SubmitDynamicFeeTx(ctx context.Context, rpcURL string, chainID *big.Int, tx *types.Transaction) (common.Hash, error) {
	if b == nil || b.walletID == "" {
		return common.Hash{}, clierr.New(clierr.CodeUsage, "wallet id is required for wallet-backed submit")
	}
	if chainID == nil {
		return common.Hash{}, clierr.New(clierr.CodeUsage, "chain id is required for wallet-backed submit")
	}
	encoded, err := EncodeUnsignedTypedTx(tx)
	if err != nil {
		return common.Hash{}, clierr.Wrap(clierr.CodeUsage, "encode unsigned transaction", err)
	}
	txHash, err := sendUnsignedTxFunc(ctx, b.walletID, fmt.Sprintf("eip155:%s", chainID.String()), encoded, rpcURL)
	if err != nil {
		return common.Hash{}, err
	}
	if !ows.IsTxHash(txHash) {
		return common.Hash{}, clierr.New(clierr.CodeSigner, fmt.Sprintf("ows submit returned invalid tx hash %q", txHash))
	}
	hash := common.HexToHash(txHash)
	if hash == (common.Hash{}) {
		return common.Hash{}, clierr.New(clierr.CodeSigner, "ows submit returned empty tx hash")
	}
	return hash, nil
}
