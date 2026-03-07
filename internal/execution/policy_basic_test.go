package execution

import (
	"math/big"
	"strings"
	"testing"

	"github.com/ethereum/go-ethereum/common"
)

func TestValidateApprovalPolicyBounded(t *testing.T) {
	data, err := policyERC20ABI.Pack("approve", common.HexToAddress("0x00000000000000000000000000000000000000ab"), big.NewInt(100))
	if err != nil {
		t.Fatalf("pack approval calldata: %v", err)
	}
	action := &Action{InputAmount: "100"}
	step := &ActionStep{Type: StepTypeApproval, Target: "0x00000000000000000000000000000000000000cd"}

	if err := validateStepPolicy(action, step, 1, data, ExecuteOptions{}); err != nil {
		t.Fatalf("expected bounded approval to pass, got err=%v", err)
	}
}

func TestValidateApprovalPolicyRejectsUnlimitedByDefault(t *testing.T) {
	data, err := policyERC20ABI.Pack("approve", common.HexToAddress("0x00000000000000000000000000000000000000ab"), big.NewInt(101))
	if err != nil {
		t.Fatalf("pack approval calldata: %v", err)
	}
	action := &Action{InputAmount: "100"}
	step := &ActionStep{Type: StepTypeApproval, Target: "0x00000000000000000000000000000000000000cd"}

	err = validateStepPolicy(action, step, 1, data, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected bounded-approval validation to fail")
	}
	if !strings.Contains(err.Error(), "allow-max-approval") {
		t.Fatalf("expected override hint, got err=%v", err)
	}
}

func TestValidateApprovalPolicyAllowsOverride(t *testing.T) {
	data, err := policyERC20ABI.Pack("approve", common.HexToAddress("0x00000000000000000000000000000000000000ab"), big.NewInt(101))
	if err != nil {
		t.Fatalf("pack approval calldata: %v", err)
	}
	action := &Action{InputAmount: "100"}
	step := &ActionStep{Type: StepTypeApproval, Target: "0x00000000000000000000000000000000000000cd"}

	if err := validateStepPolicy(action, step, 1, data, ExecuteOptions{AllowMaxApproval: true}); err != nil {
		t.Fatalf("expected approval override to pass, got err=%v", err)
	}
}

func TestValidateTransferPolicyMatchesAction(t *testing.T) {
	data, err := policyERC20ABI.Pack("transfer", common.HexToAddress("0x00000000000000000000000000000000000000ab"), big.NewInt(100))
	if err != nil {
		t.Fatalf("pack transfer calldata: %v", err)
	}
	action := &Action{
		InputAmount: "100",
		ToAddress:   "0x00000000000000000000000000000000000000ab",
		Metadata: map[string]any{
			"asset_address": "0x00000000000000000000000000000000000000cd",
		},
	}
	step := &ActionStep{
		Type:   StepTypeTransfer,
		Target: "0x00000000000000000000000000000000000000cd",
	}

	if err := validateStepPolicy(action, step, 1, data, ExecuteOptions{}); err != nil {
		t.Fatalf("expected transfer policy to pass, got err=%v", err)
	}
}

func TestValidateTransferPolicyRejectsRecipientMismatch(t *testing.T) {
	data, err := policyERC20ABI.Pack("transfer", common.HexToAddress("0x00000000000000000000000000000000000000ab"), big.NewInt(100))
	if err != nil {
		t.Fatalf("pack transfer calldata: %v", err)
	}
	action := &Action{
		InputAmount: "100",
		ToAddress:   "0x00000000000000000000000000000000000000ff",
	}
	step := &ActionStep{
		Type:   StepTypeTransfer,
		Target: "0x00000000000000000000000000000000000000cd",
	}

	err = validateStepPolicy(action, step, 1, data, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected transfer recipient mismatch to fail")
	}
	if !strings.Contains(err.Error(), "to_address") {
		t.Fatalf("expected to_address mismatch hint, got err=%v", err)
	}
}

func TestValidateTransferPolicyRejectsAmountMismatch(t *testing.T) {
	data, err := policyERC20ABI.Pack("transfer", common.HexToAddress("0x00000000000000000000000000000000000000ab"), big.NewInt(101))
	if err != nil {
		t.Fatalf("pack transfer calldata: %v", err)
	}
	action := &Action{
		InputAmount: "100",
		ToAddress:   "0x00000000000000000000000000000000000000ab",
	}
	step := &ActionStep{
		Type:   StepTypeTransfer,
		Target: "0x00000000000000000000000000000000000000cd",
	}

	err = validateStepPolicy(action, step, 1, data, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected transfer amount mismatch to fail")
	}
	if !strings.Contains(err.Error(), "does not match") {
		t.Fatalf("expected amount mismatch message, got err=%v", err)
	}
}

func TestValidateTransferPolicyRequiresAssetAddressMetadata(t *testing.T) {
	data, err := policyERC20ABI.Pack("transfer", common.HexToAddress("0x00000000000000000000000000000000000000ab"), big.NewInt(100))
	if err != nil {
		t.Fatalf("pack transfer calldata: %v", err)
	}
	action := &Action{
		InputAmount: "100",
		ToAddress:   "0x00000000000000000000000000000000000000ab",
	}
	step := &ActionStep{
		Type:   StepTypeTransfer,
		Target: "0x00000000000000000000000000000000000000cd",
	}

	err = validateStepPolicy(action, step, 1, data, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected missing asset metadata to fail")
	}
	if !strings.Contains(err.Error(), "asset_address") {
		t.Fatalf("expected asset_address validation message, got err=%v", err)
	}
}

func TestValidateSwapPolicyTaikoRouter(t *testing.T) {
	action := &Action{Provider: "taikoswap"}
	step := &ActionStep{
		Type:   StepTypeSwap,
		Target: "0x00000000000000000000000000000000000000cd",
	}
	err := validateStepPolicy(action, step, 167000, policyUniswapV3SwapMethod, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected taikoswap router mismatch to fail")
	}
}

func TestValidateBridgePolicyEndpointGuard(t *testing.T) {
	action := &Action{Provider: "lifi"}
	step := &ActionStep{
		Type:   StepTypeBridge,
		Target: "0x00000000000000000000000000000000000000cd",
		ExpectedOutputs: map[string]string{
			"settlement_provider":        "lifi",
			"settlement_status_endpoint": "https://evil.example/status",
		},
	}
	err := validateStepPolicy(action, step, 1, []byte{0x01}, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected invalid settlement endpoint to fail")
	}
	if err := validateStepPolicy(action, step, 1, []byte{0x01}, ExecuteOptions{UnsafeProviderTx: true}); err != nil {
		t.Fatalf("expected unsafe provider override to pass, got err=%v", err)
	}
}

func TestValidateBridgePolicyTargetGuard(t *testing.T) {
	action := &Action{Provider: "lifi"}
	step := &ActionStep{
		Type:   StepTypeBridge,
		Target: "0x1111111111111111111111111111111111111111",
		ExpectedOutputs: map[string]string{
			"settlement_provider":        "lifi",
			"settlement_status_endpoint": "https://li.quest/v1/status",
		},
	}
	err := validateStepPolicy(action, step, 1, []byte{0x01}, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected disallowed bridge target to fail")
	}
	if !strings.Contains(err.Error(), "execution contract") {
		t.Fatalf("expected target guard message, got err=%v", err)
	}
	if err := validateStepPolicy(action, step, 1, []byte{0x01}, ExecuteOptions{UnsafeProviderTx: true}); err != nil {
		t.Fatalf("expected unsafe provider override to bypass target guard, got err=%v", err)
	}
}

func TestValidateBridgePolicyAllowsCanonicalTarget(t *testing.T) {
	action := &Action{Provider: "across"}
	step := &ActionStep{
		Type:   StepTypeBridge,
		Target: "0x767e4c20F521a829dE4Ffc40C25176676878147f",
		ExpectedOutputs: map[string]string{
			"settlement_provider":        "across",
			"settlement_status_endpoint": "https://app.across.to/api/deposit/status",
		},
	}
	if err := validateStepPolicy(action, step, 8453, []byte{0x01}, ExecuteOptions{}); err != nil {
		t.Fatalf("expected canonical across target to pass, got err=%v", err)
	}
}
