package execution

import (
	"context"
	"errors"
	"math/big"
	"strings"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
)

type testRPCDataError struct {
	msg  string
	data any
}

func (e testRPCDataError) Error() string { return e.msg }

func (e testRPCDataError) ErrorData() interface{} { return e.data }

func TestDecodeRevertDataReasonString(t *testing.T) {
	revertData := encodeErrorString(t, "slippage too high")
	reason := decodeRevertData(revertData)
	if reason != "slippage too high" {
		t.Fatalf("expected decoded revert reason, got %q", reason)
	}
}

func TestDecodeRevertDataCustomErrorSelector(t *testing.T) {
	revertData := common.FromHex("0x12345678")
	reason := decodeRevertData(revertData)
	if !strings.Contains(reason, "0x12345678") {
		t.Fatalf("expected custom error selector in reason, got %q", reason)
	}
}

func TestDecodeRevertFromErrorWithDataError(t *testing.T) {
	revertData := encodeErrorString(t, "insufficient output amount")
	err := testRPCDataError{
		msg:  "execution reverted",
		data: "0x" + common.Bytes2Hex(revertData),
	}
	reason := decodeRevertFromError(err)
	if reason != "insufficient output amount" {
		t.Fatalf("unexpected decoded reason: %q", reason)
	}
}

func TestWrapEVMExecutionErrorIncludesDecodedRevert(t *testing.T) {
	revertData := encodeErrorString(t, "panic path")
	rootErr := testRPCDataError{
		msg:  "execution reverted",
		data: "0x" + common.Bytes2Hex(revertData),
	}
	wrapped := wrapEVMExecutionError(clierr.CodeActionSim, "simulate step (eth_call)", rootErr)
	var typed *clierr.Error
	if !errors.As(wrapped, &typed) {
		t.Fatalf("expected typed cli error, got %T", wrapped)
	}
	if !strings.Contains(typed.Error(), "panic path") {
		t.Fatalf("expected decoded reason in wrapped error, got: %v", typed)
	}
}

func TestNormalizeStepTxHash(t *testing.T) {
	validHash := "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
	if _, ok := normalizeStepTxHash(validHash); !ok {
		t.Fatal("expected valid tx hash to parse")
	}
	if _, ok := normalizeStepTxHash("0x1234"); ok {
		t.Fatal("expected short tx hash to fail")
	}
}

func TestExecuteActionRejectsInvalidStepTargetBeforeRPCDial(t *testing.T) {
	action := NewAction("act_test", "swap", "eip155:1", Constraints{Simulate: true})
	action.Steps = append(action.Steps, ActionStep{
		StepID:  "step-1",
		Type:    StepTypeSwap,
		Status:  StepStatusPending,
		ChainID: "eip155:1",
		RPCURL:  "http://127.0.0.1:65535",
		Target:  "not-an-address",
		Data:    "0x",
		Value:   "0",
	})
	err := ExecuteAction(context.Background(), nil, &action, staticSigner{}, DefaultExecuteOptions())
	if err == nil {
		t.Fatal("expected invalid target error")
	}
	typed, ok := clierr.As(err)
	if !ok || typed.Code != clierr.CodeUsage {
		t.Fatalf("expected usage error, got %v", err)
	}
	if action.Steps[0].Status != StepStatusFailed {
		t.Fatalf("expected step to be marked failed, got %s", action.Steps[0].Status)
	}
}

func TestAcquireSignerNonceLockSerializesSameSignerChain(t *testing.T) {
	unlock := acquireSignerNonceLock(big.NewInt(1), common.HexToAddress("0x00000000000000000000000000000000000000aa"))
	secondAcquired := make(chan struct{})
	go func() {
		unlockSecond := acquireSignerNonceLock(big.NewInt(1), common.HexToAddress("0x00000000000000000000000000000000000000aa"))
		close(secondAcquired)
		unlockSecond()
	}()

	select {
	case <-secondAcquired:
		t.Fatal("expected second lock attempt to block while first lock is held")
	case <-time.After(50 * time.Millisecond):
	}
	unlock()
	select {
	case <-secondAcquired:
	case <-time.After(250 * time.Millisecond):
		t.Fatal("expected second lock attempt to acquire after unlock")
	}
}

func encodeErrorString(t *testing.T, reason string) []byte {
	t.Helper()
	stringTy, err := abi.NewType("string", "", nil)
	if err != nil {
		t.Fatalf("create abi string type: %v", err)
	}
	args := abi.Arguments{{Type: stringTy}}
	encoded, err := args.Pack(reason)
	if err != nil {
		t.Fatalf("pack revert reason: %v", err)
	}
	return append(common.FromHex("0x08c379a0"), encoded...)
}

type staticSigner struct{}

func (staticSigner) Address() common.Address {
	return common.HexToAddress("0x00000000000000000000000000000000000000aa")
}

func (staticSigner) SignTx(_ *big.Int, tx *types.Transaction) (*types.Transaction, error) {
	return tx, nil
}
