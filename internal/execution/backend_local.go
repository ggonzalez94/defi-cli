package execution

import (
	"context"
	"math/big"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution/signer"
)

type localSubmitBackend struct {
	txSigner signer.Signer
}

func NewLocalSubmitBackend(txSigner signer.Signer) EVMSubmitBackend {
	if txSigner == nil {
		return nil
	}
	return &localSubmitBackend{txSigner: txSigner}
}

func (b *localSubmitBackend) EffectiveSender() common.Address {
	if b == nil || b.txSigner == nil {
		return common.Address{}
	}
	return b.txSigner.Address()
}

func (b *localSubmitBackend) SubmitDynamicFeeTx(ctx context.Context, rpcURL string, chainID *big.Int, tx *types.Transaction) (common.Hash, error) {
	if b == nil || b.txSigner == nil {
		return common.Hash{}, clierr.New(clierr.CodeSigner, "missing local signer")
	}
	signed, err := b.txSigner.SignTx(chainID, tx)
	if err != nil {
		return common.Hash{}, clierr.Wrap(clierr.CodeSigner, "sign transaction", err)
	}
	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return common.Hash{}, clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
	}
	defer client.Close()
	if err := client.SendTransaction(ctx, signed); err != nil {
		return common.Hash{}, wrapEVMExecutionError(clierr.CodeUnavailable, "broadcast transaction", err)
	}
	return signed.Hash(), nil
}
