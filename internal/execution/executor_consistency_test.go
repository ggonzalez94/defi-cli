package execution

import (
	"bytes"
	"context"
	"errors"
	"math/big"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
)

func TestApprovalExpectationFromCallMsg(t *testing.T) {
	token := common.HexToAddress("0x00000000000000000000000000000000000000aa")
	owner := common.HexToAddress("0x00000000000000000000000000000000000000bb")
	spender := common.HexToAddress("0x00000000000000000000000000000000000000cc")
	amount := big.NewInt(42)

	data, err := policyERC20ABI.Pack("approve", spender, amount)
	if err != nil {
		t.Fatalf("pack approve calldata: %v", err)
	}

	msg := ethereum.CallMsg{
		From: owner,
		To:   &token,
		Data: data,
	}

	out, ok, err := approvalExpectationFromCallMsg(msg)
	if err != nil {
		t.Fatalf("approvalExpectationFromCallMsg returned error: %v", err)
	}
	if !ok {
		t.Fatal("expected approval expectation to be detected")
	}
	if out.Token != token {
		t.Fatalf("unexpected token: %s", out.Token.Hex())
	}
	if out.Owner != owner {
		t.Fatalf("unexpected owner: %s", out.Owner.Hex())
	}
	if out.Spender != spender {
		t.Fatalf("unexpected spender: %s", out.Spender.Hex())
	}
	if out.Amount.Cmp(amount) != 0 {
		t.Fatalf("unexpected amount: %s", out.Amount.String())
	}
}

func TestApprovalExpectationFromCallMsgIgnoresNonApproval(t *testing.T) {
	token := common.HexToAddress("0x00000000000000000000000000000000000000aa")
	owner := common.HexToAddress("0x00000000000000000000000000000000000000bb")
	recipient := common.HexToAddress("0x00000000000000000000000000000000000000cc")
	amount := big.NewInt(42)

	data, err := policyERC20ABI.Pack("transfer", recipient, amount)
	if err != nil {
		t.Fatalf("pack transfer calldata: %v", err)
	}

	msg := ethereum.CallMsg{
		From: owner,
		To:   &token,
		Data: data,
	}

	_, ok, err := approvalExpectationFromCallMsg(msg)
	if err != nil {
		t.Fatalf("approvalExpectationFromCallMsg returned error: %v", err)
	}
	if ok {
		t.Fatal("expected non-approval calldata to be ignored")
	}
}

func TestWaitForAllowanceAtLeastRetriesUntilSufficient(t *testing.T) {
	token := common.HexToAddress("0x00000000000000000000000000000000000000aa")
	owner := common.HexToAddress("0x00000000000000000000000000000000000000bb")
	spender := common.HexToAddress("0x00000000000000000000000000000000000000cc")

	caller := &mockContractCaller{
		allowances: []*big.Int{
			big.NewInt(0),
			big.NewInt(5),
			big.NewInt(10),
		},
	}

	ctx, cancel := context.WithTimeout(context.Background(), 150*time.Millisecond)
	defer cancel()

	err := waitForAllowanceAtLeast(ctx, caller, approvalExpectation{
		Token:   token,
		Owner:   owner,
		Spender: spender,
		Amount:  big.NewInt(10),
	}, 5*time.Millisecond)
	if err != nil {
		t.Fatalf("waitForAllowanceAtLeast returned error: %v", err)
	}
	if caller.calls < 3 {
		t.Fatalf("expected repeated allowance checks, got %d calls", caller.calls)
	}
}

func TestWaitForAllowanceAtLeastTimesOut(t *testing.T) {
	token := common.HexToAddress("0x00000000000000000000000000000000000000aa")
	owner := common.HexToAddress("0x00000000000000000000000000000000000000bb")
	spender := common.HexToAddress("0x00000000000000000000000000000000000000cc")

	caller := &mockContractCaller{
		allowances: []*big.Int{big.NewInt(0)},
	}

	ctx, cancel := context.WithTimeout(context.Background(), 35*time.Millisecond)
	defer cancel()

	err := waitForAllowanceAtLeast(ctx, caller, approvalExpectation{
		Token:   token,
		Owner:   owner,
		Spender: spender,
		Amount:  big.NewInt(1),
	}, 5*time.Millisecond)
	if err == nil {
		t.Fatal("expected timeout error")
	}
	typed, ok := clierr.As(err)
	if !ok {
		t.Fatalf("expected typed cli error, got %T", err)
	}
	if typed.Code != clierr.CodeActionTimeout {
		t.Fatalf("expected action timeout code, got %v", typed.Code)
	}
}

func TestWaitForRPCHeadAtLeast(t *testing.T) {
	reader := &mockHeaderReader{
		heads: []*big.Int{
			big.NewInt(100),
			big.NewInt(101),
			big.NewInt(102),
		},
	}
	ctx, cancel := context.WithTimeout(context.Background(), 150*time.Millisecond)
	defer cancel()

	if err := waitForRPCHeadAtLeast(ctx, reader, big.NewInt(102), 5*time.Millisecond); err != nil {
		t.Fatalf("waitForRPCHeadAtLeast returned error: %v", err)
	}
	if reader.calls < 3 {
		t.Fatalf("expected multiple head checks, got %d", reader.calls)
	}
}

func TestWaitForRPCHeadAtLeastTimesOut(t *testing.T) {
	reader := &mockHeaderReader{
		heads: []*big.Int{big.NewInt(100)},
	}
	ctx, cancel := context.WithTimeout(context.Background(), 35*time.Millisecond)
	defer cancel()

	err := waitForRPCHeadAtLeast(ctx, reader, big.NewInt(105), 5*time.Millisecond)
	if err == nil {
		t.Fatal("expected timeout error")
	}
	typed, ok := clierr.As(err)
	if !ok {
		t.Fatalf("expected typed cli error, got %T", err)
	}
	if typed.Code != clierr.CodeActionTimeout {
		t.Fatalf("expected action timeout code, got %v", typed.Code)
	}
}

type mockContractCaller struct {
	allowances []*big.Int
	calls      int
	err        error
}

func (m *mockContractCaller) CallContract(_ context.Context, msg ethereum.CallMsg, _ *big.Int) ([]byte, error) {
	m.calls++
	if m.err != nil {
		return nil, m.err
	}
	if msg.To == nil {
		return nil, errors.New("missing token address")
	}
	if len(msg.Data) < 4 || !bytes.Equal(msg.Data[:4], policyERC20ABI.Methods["allowance"].ID) {
		return nil, errors.New("unexpected calldata selector")
	}
	idx := m.calls - 1
	if idx < 0 {
		idx = 0
	}
	if idx >= len(m.allowances) {
		idx = len(m.allowances) - 1
	}
	value := big.NewInt(0)
	if len(m.allowances) > 0 && m.allowances[idx] != nil {
		value = m.allowances[idx]
	}
	out, err := policyERC20ABI.Methods["allowance"].Outputs.Pack(value)
	if err != nil {
		return nil, err
	}
	return out, nil
}

type mockHeaderReader struct {
	heads []*big.Int
	calls int
}

func (m *mockHeaderReader) HeaderByNumber(_ context.Context, _ *big.Int) (*types.Header, error) {
	m.calls++
	idx := m.calls - 1
	if idx < 0 {
		idx = 0
	}
	if idx >= len(m.heads) {
		idx = len(m.heads) - 1
	}
	number := big.NewInt(0)
	if len(m.heads) > 0 && m.heads[idx] != nil {
		number = new(big.Int).Set(m.heads[idx])
	}
	return &types.Header{Number: number}, nil
}
