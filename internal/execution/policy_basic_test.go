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

func TestValidateSwapPolicyTempoDEX(t *testing.T) {
	action := &Action{Provider: "tempo"}
	step := &ActionStep{
		Type:   StepTypeSwap,
		Target: "0x00000000000000000000000000000000000000cd",
	}
	err := validateStepPolicy(action, step, 4217, policyTempoSwapExactIn, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected tempo dex target mismatch to fail")
	}
}

func TestValidateTempoSwapBatchedCallsPass(t *testing.T) {
	// Build a valid approve + swap batched step.
	dexAddr := "0xdec0000000000000000000000000000000000000"
	tokenIn := "0x20c0000000000000000000000000000000000000"

	approveData, err := policyERC20ABI.Pack("approve", common.HexToAddress(dexAddr), big.NewInt(1000))
	if err != nil {
		t.Fatalf("pack approve calldata: %v", err)
	}
	swapData, err := policyTempoDEXABI.Pack("swapExactAmountIn",
		common.HexToAddress(tokenIn),
		common.HexToAddress("0x20c000000000000000000000b9537d11c60e8b50"),
		big.NewInt(1000),
		big.NewInt(990),
	)
	if err != nil {
		t.Fatalf("pack swap calldata: %v", err)
	}

	action := &Action{Provider: "tempo", InputAmount: "1000", Metadata: map[string]any{"token_in": tokenIn}}
	step := &ActionStep{
		Type:   StepTypeSwap,
		Target: "",
		Data:   "",
		Calls: []StepCall{
			{Target: tokenIn, Data: "0x" + common.Bytes2Hex(approveData), Value: "0"},
			{Target: dexAddr, Data: "0x" + common.Bytes2Hex(swapData), Value: "0"},
		},
	}

	// Chain 4217 is Tempo mainnet.
	if err := validateSwapPolicy(action, step, 4217, nil, ExecuteOptions{}); err != nil {
		t.Fatalf("expected batched tempo swap to pass, got err=%v", err)
	}
}

func TestValidateTempoSwapBatchedCallsRejectsWrongDEX(t *testing.T) {
	wrongDEX := "0x00000000000000000000000000000000000000FF"
	tokenIn := "0x20c0000000000000000000000000000000000000"

	swapData, err := policyTempoDEXABI.Pack("swapExactAmountIn",
		common.HexToAddress(tokenIn),
		common.HexToAddress("0x20c000000000000000000000b9537d11c60e8b50"),
		big.NewInt(1000),
		big.NewInt(990),
	)
	if err != nil {
		t.Fatalf("pack swap calldata: %v", err)
	}

	action := &Action{Provider: "tempo"}
	step := &ActionStep{
		Type:   StepTypeSwap,
		Target: "",
		Data:   "",
		Calls: []StepCall{
			{Target: wrongDEX, Data: "0x" + common.Bytes2Hex(swapData), Value: "0"},
		},
	}

	err = validateSwapPolicy(action, step, 4217, nil, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected wrong DEX target to fail")
	}
	if !strings.Contains(err.Error(), "canonical stablecoin dex") {
		t.Fatalf("expected canonical dex mismatch message, got err=%v", err)
	}
}

func TestValidateTempoSwapBatchedCallsRejectsUnknownSelector(t *testing.T) {
	action := &Action{Provider: "tempo"}
	step := &ActionStep{
		Type:   StepTypeSwap,
		Target: "",
		Data:   "",
		Calls: []StepCall{
			{Target: "0xdec0000000000000000000000000000000000000", Data: "0xdeadbeef", Value: "0"},
		},
	}

	err := validateSwapPolicy(action, step, 4217, nil, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected unknown selector to fail")
	}
	if !strings.Contains(err.Error(), "unrecognized selector") {
		t.Fatalf("expected unrecognized selector message, got err=%v", err)
	}
}

func TestValidateTempoSwapBatchedCallsRejectsApproveOnly(t *testing.T) {
	dexAddr := "0xdec0000000000000000000000000000000000000"
	tokenIn := "0x20c0000000000000000000000000000000000000"

	approveData, err := policyERC20ABI.Pack("approve", common.HexToAddress(dexAddr), big.NewInt(1000))
	if err != nil {
		t.Fatalf("pack approve calldata: %v", err)
	}

	action := &Action{Provider: "tempo", InputAmount: "1000"}
	step := &ActionStep{
		Type:   StepTypeSwap,
		Target: "",
		Data:   "",
		Calls: []StepCall{
			{Target: tokenIn, Data: "0x" + common.Bytes2Hex(approveData), Value: "0"},
		},
	}

	err = validateSwapPolicy(action, step, 4217, nil, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected approve-only batch to fail")
	}
	if !strings.Contains(err.Error(), "at least one swap call") {
		t.Fatalf("expected missing swap call message, got err=%v", err)
	}
}

func TestValidateTempoSwapBatchedCallsRejectsApproveOnWrongToken(t *testing.T) {
	dexAddr := "0xdec0000000000000000000000000000000000000"
	tokenIn := "0x20c0000000000000000000000000000000000000"
	wrongToken := "0xba00000000000000000000000000000000000000"

	approveData, err := policyERC20ABI.Pack("approve", common.HexToAddress(dexAddr), big.NewInt(1000))
	if err != nil {
		t.Fatalf("pack approve calldata: %v", err)
	}
	swapData, err := policyTempoDEXABI.Pack("swapExactAmountIn",
		common.HexToAddress(tokenIn),
		common.HexToAddress("0x20c000000000000000000000b9537d11c60e8b50"),
		big.NewInt(1000),
		big.NewInt(990),
	)
	if err != nil {
		t.Fatalf("pack swap calldata: %v", err)
	}

	action := &Action{
		Provider:    "tempo",
		InputAmount: "1000",
		Metadata:    map[string]any{"token_in": tokenIn},
	}
	step := &ActionStep{
		Type: StepTypeSwap,
		Calls: []StepCall{
			{Target: wrongToken, Data: "0x" + common.Bytes2Hex(approveData), Value: "0"},
			{Target: dexAddr, Data: "0x" + common.Bytes2Hex(swapData), Value: "0"},
		},
	}

	err = validateSwapPolicy(action, step, 4217, nil, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected approve on wrong token to fail")
	}
	if !strings.Contains(err.Error(), "input token") {
		t.Fatalf("expected input token mismatch message, got err=%v", err)
	}
}

func TestValidateTempoSwapBatchedCallsRejectsExtraApproval(t *testing.T) {
	dexAddr := "0xdec0000000000000000000000000000000000000"
	tokenIn := "0x20c0000000000000000000000000000000000000"

	approveData, err := policyERC20ABI.Pack("approve", common.HexToAddress(dexAddr), big.NewInt(500))
	if err != nil {
		t.Fatalf("pack approve calldata: %v", err)
	}
	swapData, err := policyTempoDEXABI.Pack("swapExactAmountIn",
		common.HexToAddress(tokenIn),
		common.HexToAddress("0x20c000000000000000000000b9537d11c60e8b50"),
		big.NewInt(1000),
		big.NewInt(990),
	)
	if err != nil {
		t.Fatalf("pack swap calldata: %v", err)
	}

	action := &Action{
		Provider:    "tempo",
		InputAmount: "1000",
		Metadata:    map[string]any{"token_in": tokenIn},
	}
	step := &ActionStep{
		Type: StepTypeSwap,
		Calls: []StepCall{
			{Target: tokenIn, Data: "0x" + common.Bytes2Hex(approveData), Value: "0"},
			{Target: tokenIn, Data: "0x" + common.Bytes2Hex(approveData), Value: "0"},
			{Target: dexAddr, Data: "0x" + common.Bytes2Hex(swapData), Value: "0"},
		},
	}

	err = validateSwapPolicy(action, step, 4217, nil, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected duplicate approve to fail")
	}
	if !strings.Contains(err.Error(), "more than one approve") {
		t.Fatalf("expected duplicate approve message, got err=%v", err)
	}
}

func TestValidateTempoSwapBatchedCallsRejectsApproveWithValue(t *testing.T) {
	dexAddr := "0xdec0000000000000000000000000000000000000"
	tokenIn := "0x20c0000000000000000000000000000000000000"

	approveData, err := policyERC20ABI.Pack("approve", common.HexToAddress(dexAddr), big.NewInt(1000))
	if err != nil {
		t.Fatalf("pack approve calldata: %v", err)
	}
	swapData, err := policyTempoDEXABI.Pack("swapExactAmountIn",
		common.HexToAddress(tokenIn),
		common.HexToAddress("0x20c000000000000000000000b9537d11c60e8b50"),
		big.NewInt(1000),
		big.NewInt(990),
	)
	if err != nil {
		t.Fatalf("pack swap calldata: %v", err)
	}

	action := &Action{
		Provider:    "tempo",
		InputAmount: "1000",
		Metadata:    map[string]any{"token_in": tokenIn},
	}
	step := &ActionStep{
		Type: StepTypeSwap,
		Calls: []StepCall{
			{Target: tokenIn, Data: "0x" + common.Bytes2Hex(approveData), Value: "100"},
			{Target: dexAddr, Data: "0x" + common.Bytes2Hex(swapData), Value: "0"},
		},
	}

	err = validateSwapPolicy(action, step, 4217, nil, ExecuteOptions{})
	if err == nil {
		t.Fatal("expected approve with non-zero value to fail")
	}
	if !strings.Contains(err.Error(), "zero value") {
		t.Fatalf("expected zero value message, got err=%v", err)
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

func TestValidateBridgePolicyAllowsCanonicalLiFiTarget(t *testing.T) {
	action := &Action{Provider: "lifi"}
	step := &ActionStep{
		Type:   StepTypeBridge,
		Target: "0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE",
		ExpectedOutputs: map[string]string{
			"settlement_provider":        "lifi",
			"settlement_status_endpoint": "https://li.quest/v1/status",
		},
	}
	if err := validateStepPolicy(action, step, 1, []byte{0x01}, ExecuteOptions{}); err != nil {
		t.Fatalf("expected canonical lifi target to pass, got err=%v", err)
	}
}

func TestValidateBridgePolicySkipsTargetCheckOnUncoveredChain(t *testing.T) {
	// Chain 43114 (Avalanche) has no Across target policy, so the target check
	// should be skipped and the step should pass regardless of the target address.
	action := &Action{Provider: "across"}
	step := &ActionStep{
		Type:   StepTypeBridge,
		Target: "0x1111111111111111111111111111111111111111",
		ExpectedOutputs: map[string]string{
			"settlement_provider":        "across",
			"settlement_status_endpoint": "https://app.across.to/api/deposit/status",
		},
	}
	if err := validateStepPolicy(action, step, 43114, []byte{0x01}, ExecuteOptions{}); err != nil {
		t.Fatalf("expected uncovered chain to skip target check, got err=%v", err)
	}
}
