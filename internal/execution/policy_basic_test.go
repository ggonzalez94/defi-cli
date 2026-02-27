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
