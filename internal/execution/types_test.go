package execution

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestActionStepCallsRoundTrip(t *testing.T) {
	step := ActionStep{
		StepID:  "step-1",
		Type:    StepTypeSwap,
		Status:  StepStatusPending,
		ChainID: "eip155:4217",
		Target:  "0x00000000000000000000000000000000000000aa",
		Data:    "0x",
		Value:   "0",
		Calls: []StepCall{
			{
				Target: "0x00000000000000000000000000000000000000bb",
				Data:   "0xabcdef",
				Value:  "1000",
			},
			{
				Target: "0x00000000000000000000000000000000000000cc",
				Data:   "0x123456",
				Value:  "0",
			},
		},
	}

	data, err := json.Marshal(step)
	if err != nil {
		t.Fatalf("marshal step: %v", err)
	}

	var decoded ActionStep
	if err := json.Unmarshal(data, &decoded); err != nil {
		t.Fatalf("unmarshal step: %v", err)
	}

	if len(decoded.Calls) != 2 {
		t.Fatalf("expected 2 calls, got %d", len(decoded.Calls))
	}
	if decoded.Calls[0].Target != step.Calls[0].Target {
		t.Fatalf("call[0] target mismatch: %s vs %s", decoded.Calls[0].Target, step.Calls[0].Target)
	}
	if decoded.Calls[0].Data != step.Calls[0].Data {
		t.Fatalf("call[0] data mismatch: %s vs %s", decoded.Calls[0].Data, step.Calls[0].Data)
	}
	if decoded.Calls[0].Value != step.Calls[0].Value {
		t.Fatalf("call[0] value mismatch: %s vs %s", decoded.Calls[0].Value, step.Calls[0].Value)
	}
	if decoded.Calls[1].Target != step.Calls[1].Target {
		t.Fatalf("call[1] target mismatch: %s vs %s", decoded.Calls[1].Target, step.Calls[1].Target)
	}
}

func TestActionStepCallsOmittedWhenEmpty(t *testing.T) {
	step := ActionStep{
		StepID:  "step-no-calls",
		Type:    StepTypeSwap,
		Status:  StepStatusPending,
		ChainID: "eip155:1",
		Target:  "0x00000000000000000000000000000000000000aa",
		Data:    "0x",
		Value:   "0",
	}

	data, err := json.Marshal(step)
	if err != nil {
		t.Fatalf("marshal step: %v", err)
	}

	if strings.Contains(string(data), `"calls"`) {
		t.Fatalf("expected calls to be omitted from JSON when nil, got: %s", string(data))
	}

	// Also verify empty slice is omitted
	step.Calls = []StepCall{}
	data, err = json.Marshal(step)
	if err != nil {
		t.Fatalf("marshal step with empty calls: %v", err)
	}
	// Note: Go's json.Marshal does NOT omit empty slices with omitempty (only nil slices).
	// This is expected Go behavior. Verify the round-trip still works.
	var decoded ActionStep
	if err := json.Unmarshal(data, &decoded); err != nil {
		t.Fatalf("unmarshal step with empty calls: %v", err)
	}
	if len(decoded.Calls) != 0 {
		t.Fatalf("expected 0 calls after round-trip, got %d", len(decoded.Calls))
	}
}

func TestActionStepCallsNilOmitted(t *testing.T) {
	step := ActionStep{
		StepID:  "step-nil-calls",
		Type:    StepTypeSwap,
		Status:  StepStatusPending,
		ChainID: "eip155:1",
		Target:  "0x00000000000000000000000000000000000000aa",
		Data:    "0x",
		Value:   "0",
		Calls:   nil,
	}

	data, err := json.Marshal(step)
	if err != nil {
		t.Fatalf("marshal step: %v", err)
	}

	if strings.Contains(string(data), `"calls"`) {
		t.Fatalf("expected calls to be omitted from JSON when nil, got: %s", string(data))
	}
}

func TestActionRoundTripIncludesWalletMetadata(t *testing.T) {
	action := NewAction("action-wallet-roundtrip", "swap", "eip155:1", Constraints{})
	action.FromAddress = "0x00000000000000000000000000000000000000aa"
	action.WalletID = "wallet-123"
	action.WalletName = "Agent Wallet"
	action.ExecutionBackend = ExecutionBackendOWS

	data, err := json.Marshal(action)
	if err != nil {
		t.Fatalf("marshal action: %v", err)
	}

	jsonBody := string(data)
	if !strings.Contains(jsonBody, `"wallet_id":"wallet-123"`) {
		t.Fatalf("expected wallet_id in JSON, got: %s", jsonBody)
	}
	if !strings.Contains(jsonBody, `"wallet_name":"Agent Wallet"`) {
		t.Fatalf("expected wallet_name in JSON, got: %s", jsonBody)
	}
	if !strings.Contains(jsonBody, `"from_address":"0x00000000000000000000000000000000000000aa"`) {
		t.Fatalf("expected from_address in JSON, got: %s", jsonBody)
	}
	if !strings.Contains(jsonBody, `"execution_backend":"ows"`) {
		t.Fatalf("expected execution_backend in JSON, got: %s", jsonBody)
	}

	var decoded Action
	if err := json.Unmarshal(data, &decoded); err != nil {
		t.Fatalf("unmarshal action: %v", err)
	}
	if decoded.WalletID != action.WalletID {
		t.Fatalf("wallet_id mismatch: %s vs %s", decoded.WalletID, action.WalletID)
	}
	if decoded.WalletName != action.WalletName {
		t.Fatalf("wallet_name mismatch: %s vs %s", decoded.WalletName, action.WalletName)
	}
	if decoded.ExecutionBackend != action.ExecutionBackend {
		t.Fatalf("execution_backend mismatch: %s vs %s", decoded.ExecutionBackend, action.ExecutionBackend)
	}
	if decoded.FromAddress != action.FromAddress {
		t.Fatalf("from_address mismatch: %s vs %s", decoded.FromAddress, action.FromAddress)
	}
}
