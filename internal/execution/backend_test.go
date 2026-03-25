package execution

import (
	"context"
	"math/big"
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
)

type stubEVMSubmitBackend struct {
	sender common.Address
}

func (s stubEVMSubmitBackend) EffectiveSender() common.Address {
	return s.sender
}

func (s stubEVMSubmitBackend) SubmitDynamicFeeTx(context.Context, string, *big.Int, *types.Transaction) (common.Hash, error) {
	return common.Hash{}, nil
}

func TestResolveExecutionBackendUsesOWSForWalletActions(t *testing.T) {
	backend := stubEVMSubmitBackend{sender: common.HexToAddress("0x00000000000000000000000000000000000000aa")}
	action := &Action{
		ChainID:          "eip155:1",
		ExecutionBackend: ExecutionBackendOWS,
		WalletID:         "wallet-123",
	}

	exec, err := ResolveExecutionBackend(action, staticSigner{}, backend)
	if err != nil {
		t.Fatalf("ResolveExecutionBackend failed: %v", err)
	}
	evmExec, ok := exec.(*EVMStepExecutor)
	if !ok {
		t.Fatalf("expected EVMStepExecutor, got %T", exec)
	}
	if evmExec.backend != backend {
		t.Fatalf("expected OWS backend to be preserved")
	}
}

func TestResolveExecutionBackendUsesLegacyForLegacyActions(t *testing.T) {
	backend := stubEVMSubmitBackend{sender: common.HexToAddress("0x00000000000000000000000000000000000000aa")}
	action := &Action{
		ChainID:          "eip155:1",
		ExecutionBackend: ExecutionBackendLegacyLocal,
	}

	exec, err := ResolveExecutionBackend(action, staticSigner{}, backend)
	if err != nil {
		t.Fatalf("ResolveExecutionBackend failed: %v", err)
	}
	if _, ok := exec.(*EVMStepExecutor); !ok {
		t.Fatalf("expected EVMStepExecutor, got %T", exec)
	}
}

func TestResolveExecutionBackendUsesTempoForTempoActions(t *testing.T) {
	action := &Action{
		ChainID:          "eip155:4217",
		ExecutionBackend: ExecutionBackendTempo,
	}

	exec, err := ResolveExecutionBackend(action, staticSigner{}, nil)
	if err != nil {
		t.Fatalf("ResolveExecutionBackend failed: %v", err)
	}
	if _, ok := exec.(*TempoStepExecutor); !ok {
		t.Fatalf("expected TempoStepExecutor, got %T", exec)
	}
}

func TestOWSSubmitRejectsMalformedTxHash(t *testing.T) {
	prevSendUnsignedTx := sendUnsignedTxFunc
	sendUnsignedTxFunc = func(context.Context, string, string, []byte, string) (string, error) {
		return "0xabc123", nil
	}
	t.Cleanup(func() {
		sendUnsignedTxFunc = prevSendUnsignedTx
	})

	backend := NewOWSSubmitBackend("wallet-123", common.HexToAddress("0x00000000000000000000000000000000000000aa"))
	target := common.HexToAddress("0x00000000000000000000000000000000000000bb")
	tx := types.NewTx(&types.DynamicFeeTx{
		ChainID:   big.NewInt(1),
		Nonce:     7,
		GasTipCap: big.NewInt(1),
		GasFeeCap: big.NewInt(2),
		Gas:       21_000,
		To:        &target,
		Value:     big.NewInt(0),
	})

	_, err := backend.SubmitDynamicFeeTx(context.Background(), "https://rpc.example", big.NewInt(1), tx)
	if err == nil {
		t.Fatal("expected malformed tx hash to fail")
	}
	typed, ok := clierr.As(err)
	if !ok || typed.Code != clierr.CodeSigner {
		t.Fatalf("expected signer error, got %v", err)
	}
}
