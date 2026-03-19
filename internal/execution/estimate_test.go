package execution

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
)

type estimateRPCRequest struct {
	JSONRPC string            `json:"jsonrpc"`
	ID      json.RawMessage   `json:"id"`
	Method  string            `json:"method"`
	Params  []json.RawMessage `json:"params"`
}

func TestEstimateActionGasSingleStep(t *testing.T) {
	rpc := newEstimateRPCServer(t)
	defer rpc.Close()

	action := Action{
		ActionID:    "act_test",
		FromAddress: "0x00000000000000000000000000000000000000aa",
		Steps: []ActionStep{{
			StepID:  "swap-step",
			Type:    StepTypeSwap,
			Status:  StepStatusPending,
			ChainID: "eip155:1",
			RPCURL:  rpc.URL,
			Target:  "0x00000000000000000000000000000000000000bb",
			Data:    "0x",
			Value:   "0",
		}},
	}

	estimate, err := EstimateActionGas(context.Background(), action, DefaultEstimateOptions())
	if err != nil {
		t.Fatalf("EstimateActionGas failed: %v", err)
	}
	if estimate.ActionID != "act_test" {
		t.Fatalf("unexpected action id: %s", estimate.ActionID)
	}
	if estimate.BlockTag != string(EstimateBlockTagPending) {
		t.Fatalf("expected block tag pending, got %s", estimate.BlockTag)
	}
	if len(estimate.Steps) != 1 {
		t.Fatalf("expected one estimated step, got %d", len(estimate.Steps))
	}
	step := estimate.Steps[0]
	if step.StepID != "swap-step" {
		t.Fatalf("unexpected step id: %s", step.StepID)
	}
	if step.GasEstimateRaw != "21000" {
		t.Fatalf("expected raw gas 21000, got %s", step.GasEstimateRaw)
	}
	if step.GasLimit != "25200" {
		t.Fatalf("expected gas limit 25200, got %s", step.GasLimit)
	}
	if step.BaseFeePerGasWei != "1000000000" {
		t.Fatalf("expected base fee 1 gwei, got %s", step.BaseFeePerGasWei)
	}
	if step.MaxPriorityFeePerGasWei != "2000000000" {
		t.Fatalf("expected tip cap 2 gwei, got %s", step.MaxPriorityFeePerGasWei)
	}
	if step.MaxFeePerGasWei != "4000000000" {
		t.Fatalf("expected fee cap 4 gwei, got %s", step.MaxFeePerGasWei)
	}
	if step.EffectiveGasPriceWei != "3000000000" {
		t.Fatalf("expected effective gas price 3 gwei, got %s", step.EffectiveGasPriceWei)
	}
	if step.LikelyFeeWei != "75600000000000" {
		t.Fatalf("unexpected likely fee: %s", step.LikelyFeeWei)
	}
	if step.WorstCaseFeeWei != "100800000000000" {
		t.Fatalf("unexpected worst-case fee: %s", step.WorstCaseFeeWei)
	}
	if len(estimate.TotalsByChain) != 1 {
		t.Fatalf("expected one chain total, got %d", len(estimate.TotalsByChain))
	}
	total := estimate.TotalsByChain[0]
	if total.ChainID != "eip155:1" {
		t.Fatalf("unexpected chain total id: %s", total.ChainID)
	}
	if total.LikelyFeeWei != step.LikelyFeeWei {
		t.Fatalf("expected likely fee total %s, got %s", step.LikelyFeeWei, total.LikelyFeeWei)
	}
	if total.WorstCaseFeeWei != step.WorstCaseFeeWei {
		t.Fatalf("expected worst-case fee total %s, got %s", step.WorstCaseFeeWei, total.WorstCaseFeeWei)
	}
}

func TestEstimateActionGasCanonicalizesStepChainID(t *testing.T) {
	rpc := newEstimateRPCServer(t)
	defer rpc.Close()

	action := Action{
		ActionID:    "act_chain",
		FromAddress: "0x00000000000000000000000000000000000000aa",
		Steps: []ActionStep{{
			StepID:  "swap-step",
			Type:    StepTypeSwap,
			Status:  StepStatusPending,
			ChainID: "",
			RPCURL:  rpc.URL,
			Target:  "0x00000000000000000000000000000000000000bb",
			Data:    "0x",
			Value:   "0",
		}},
	}

	estimate, err := EstimateActionGas(context.Background(), action, DefaultEstimateOptions())
	if err != nil {
		t.Fatalf("EstimateActionGas failed: %v", err)
	}
	if got := estimate.Steps[0].ChainID; got != "eip155:1" {
		t.Fatalf("expected canonical step chain id eip155:1, got %s", got)
	}
	if got := estimate.TotalsByChain[0].ChainID; got != "eip155:1" {
		t.Fatalf("expected canonical totals chain id eip155:1, got %s", got)
	}
}

func TestEstimateActionGasFiltersSteps(t *testing.T) {
	rpc := newEstimateRPCServer(t)
	defer rpc.Close()

	action := Action{
		ActionID:    "act_filter",
		FromAddress: "0x00000000000000000000000000000000000000aa",
		Steps: []ActionStep{
			{
				StepID:  "first-step",
				Type:    StepTypeApproval,
				Status:  StepStatusPending,
				ChainID: "eip155:1",
				RPCURL:  rpc.URL,
				Target:  "0x00000000000000000000000000000000000000bb",
				Data:    "0x",
				Value:   "0",
			},
			{
				StepID:  "second-step",
				Type:    StepTypeSwap,
				Status:  StepStatusPending,
				ChainID: "eip155:1",
				RPCURL:  rpc.URL,
				Target:  "0x00000000000000000000000000000000000000cc",
				Data:    "0x",
				Value:   "0",
			},
		},
	}

	opts := DefaultEstimateOptions()
	opts.StepIDs = []string{"second-step"}

	estimate, err := EstimateActionGas(context.Background(), action, opts)
	if err != nil {
		t.Fatalf("EstimateActionGas failed: %v", err)
	}
	if len(estimate.Steps) != 1 {
		t.Fatalf("expected one estimated step, got %d", len(estimate.Steps))
	}
	if estimate.Steps[0].StepID != "second-step" {
		t.Fatalf("expected second-step, got %s", estimate.Steps[0].StepID)
	}
}

func TestEstimateActionGasFilterNoMatches(t *testing.T) {
	rpc := newEstimateRPCServer(t)
	defer rpc.Close()

	action := Action{
		ActionID:    "act_filter_none",
		FromAddress: "0x00000000000000000000000000000000000000aa",
		Steps: []ActionStep{{
			StepID:  "only-step",
			Type:    StepTypeSwap,
			Status:  StepStatusPending,
			ChainID: "eip155:1",
			RPCURL:  rpc.URL,
			Target:  "0x00000000000000000000000000000000000000bb",
			Data:    "0x",
			Value:   "0",
		}},
	}

	opts := DefaultEstimateOptions()
	opts.StepIDs = []string{"missing-step"}
	if _, err := EstimateActionGas(context.Background(), action, opts); err == nil {
		t.Fatal("expected no-match filter error")
	}
}

func TestEstimateActionGasUsesSequentialSimulationWhenAvailable(t *testing.T) {
	var estimateCalls int
	rpc := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var req estimateRPCRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		switch req.Method {
		case "eth_chainId":
			writeEstimateRPCResult(t, w, req.ID, "0x1")
		case "eth_simulateV1":
			writeEstimateRPCResult(t, w, req.ID, []map[string]any{
				{
					"calls": []map[string]any{
						{"gasUsed": "0x5208", "status": "0x1"},
						{"gasUsed": "0x1d4c0", "status": "0x1"},
					},
				},
			})
		case "eth_estimateGas":
			estimateCalls++
			writeEstimateRPCError(w, req.ID, -32000, "legacy estimate should not be called")
		case "eth_maxPriorityFeePerGas":
			writeEstimateRPCResult(t, w, req.ID, "0x77359400")
		case "eth_getBlockByNumber":
			writeEstimateRPCResult(t, w, req.ID, map[string]any{
				"baseFeePerGas": "0x3b9aca00",
			})
		default:
			writeEstimateRPCError(w, req.ID, -32601, fmt.Sprintf("method not supported in test: %s", req.Method))
		}
	}))
	defer rpc.Close()

	action := Action{
		ActionID:    "act_seq_sim",
		FromAddress: "0x00000000000000000000000000000000000000aa",
		Steps: []ActionStep{
			{
				StepID:  "approve-step",
				Type:    StepTypeApproval,
				Status:  StepStatusPending,
				ChainID: "eip155:1",
				RPCURL:  rpc.URL,
				Target:  "0x00000000000000000000000000000000000000bb",
				Data:    "0x",
				Value:   "0",
			},
			{
				StepID:  "deposit-step",
				Type:    StepTypeLend,
				Status:  StepStatusPending,
				ChainID: "eip155:1",
				RPCURL:  rpc.URL,
				Target:  "0x00000000000000000000000000000000000000cc",
				Data:    "0x",
				Value:   "0",
			},
		},
	}

	estimate, err := EstimateActionGas(context.Background(), action, DefaultEstimateOptions())
	if err != nil {
		t.Fatalf("EstimateActionGas failed: %v", err)
	}
	if len(estimate.Steps) != 2 {
		t.Fatalf("expected two estimated steps, got %d", len(estimate.Steps))
	}
	if got := estimate.Steps[0].GasEstimateRaw; got != "21000" {
		t.Fatalf("unexpected first-step gas estimate: %s", got)
	}
	if got := estimate.Steps[1].GasEstimateRaw; got != "120000" {
		t.Fatalf("unexpected second-step gas estimate: %s", got)
	}
	if estimateCalls != 0 {
		t.Fatalf("expected no legacy eth_estimateGas calls, got %d", estimateCalls)
	}
}

func TestEstimateActionGasFallsBackWhenSequentialSimulationUnavailable(t *testing.T) {
	rpc := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var req estimateRPCRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		switch req.Method {
		case "eth_chainId":
			writeEstimateRPCResult(t, w, req.ID, "0x1")
		case "eth_simulateV1":
			writeEstimateRPCError(w, req.ID, -32601, "the method eth_simulateV1 does not exist/is not available")
		case "eth_estimateGas":
			writeEstimateRPCResult(t, w, req.ID, "0x5208")
		case "eth_maxPriorityFeePerGas":
			writeEstimateRPCResult(t, w, req.ID, "0x77359400")
		case "eth_getBlockByNumber":
			writeEstimateRPCResult(t, w, req.ID, map[string]any{
				"baseFeePerGas": "0x3b9aca00",
			})
		default:
			writeEstimateRPCError(w, req.ID, -32601, fmt.Sprintf("method not supported in test: %s", req.Method))
		}
	}))
	defer rpc.Close()

	action := Action{
		ActionID:    "act_seq_fallback",
		FromAddress: "0x00000000000000000000000000000000000000aa",
		Steps: []ActionStep{
			{
				StepID:  "approve-step",
				Type:    StepTypeApproval,
				Status:  StepStatusPending,
				ChainID: "eip155:1",
				RPCURL:  rpc.URL,
				Target:  "0x00000000000000000000000000000000000000bb",
				Data:    "0x",
				Value:   "0",
			},
			{
				StepID:  "deposit-step",
				Type:    StepTypeLend,
				Status:  StepStatusPending,
				ChainID: "eip155:1",
				RPCURL:  rpc.URL,
				Target:  "0x00000000000000000000000000000000000000cc",
				Data:    "0x",
				Value:   "0",
			},
		},
	}

	estimate, err := EstimateActionGas(context.Background(), action, DefaultEstimateOptions())
	if err != nil {
		t.Fatalf("EstimateActionGas failed: %v", err)
	}
	if len(estimate.Steps) != 2 {
		t.Fatalf("expected two estimated steps, got %d", len(estimate.Steps))
	}
	if got := estimate.Steps[0].GasEstimateRaw; got != "21000" {
		t.Fatalf("unexpected first-step gas estimate: %s", got)
	}
	if got := estimate.Steps[1].GasEstimateRaw; got != "21000" {
		t.Fatalf("unexpected second-step gas estimate: %s", got)
	}
}

func TestEstimateActionGasTempoFeeToken(t *testing.T) {
	rpc := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var req estimateRPCRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		switch req.Method {
		case "eth_chainId":
			// Tempo mainnet chain ID 4217 = 0x1079
			writeEstimateRPCResult(t, w, req.ID, "0x1079")
		case "eth_estimateGas":
			writeEstimateRPCResult(t, w, req.ID, "0x5208") // 21000
		case "eth_maxPriorityFeePerGas":
			writeEstimateRPCResult(t, w, req.ID, "0x0") // 0 tip on Tempo
		case "eth_getBlockByNumber":
			// Tempo baseFee is in 18-decimal USD: e.g. 1e12 wei = 0.000001 USD
			writeEstimateRPCResult(t, w, req.ID, map[string]any{
				"baseFeePerGas": "0xe8d4a51000", // 1_000_000_000_000 = 1e12
			})
		default:
			writeEstimateRPCError(w, req.ID, -32601, fmt.Sprintf("method not supported in test: %s", req.Method))
		}
	}))
	defer rpc.Close()

	action := Action{
		ActionID:    "act_tempo_fee",
		FromAddress: "0x00000000000000000000000000000000000000aa",
		Steps: []ActionStep{{
			StepID:  "swap-step",
			Type:    StepTypeSwap,
			Status:  StepStatusPending,
			ChainID: "eip155:4217",
			RPCURL:  rpc.URL,
			Target:  "0x00000000000000000000000000000000000000bb",
			Data:    "0x",
			Value:   "0",
		}},
	}

	estimate, err := EstimateActionGas(context.Background(), action, DefaultEstimateOptions())
	if err != nil {
		t.Fatalf("EstimateActionGas failed: %v", err)
	}
	if len(estimate.Steps) != 1 {
		t.Fatalf("expected one step, got %d", len(estimate.Steps))
	}
	step := estimate.Steps[0]
	if step.FeeUnit != "USDC.e" {
		t.Fatalf("expected fee_unit USDC.e, got %q", step.FeeUnit)
	}
	if step.FeeToken == "" {
		t.Fatal("expected non-empty fee_token for Tempo step")
	}

	// Verify that chain totals also carry fee metadata.
	if len(estimate.TotalsByChain) != 1 {
		t.Fatalf("expected one chain total, got %d", len(estimate.TotalsByChain))
	}
	total := estimate.TotalsByChain[0]
	if total.FeeUnit != "USDC.e" {
		t.Fatalf("expected chain total fee_unit USDC.e, got %q", total.FeeUnit)
	}
	if total.FeeToken == "" {
		t.Fatal("expected non-empty chain total fee_token")
	}

	// Gas: rawGas=21000, gasLimit=21000*1.2=25200
	// BaseFee=1e12 (on Tempo), tipCap=0, effectiveGasPrice=1e12, feeCap=2*1e12+0=2e12
	// likelyFeeWei = 25200 * 1e12 = 25200000000000000
	// After dividing by 10^12 (token conversion): 25200
	if step.LikelyFeeWei != "25200" {
		t.Fatalf("expected likely fee 25200 (USDC.e base units), got %s", step.LikelyFeeWei)
	}
}

func TestEstimateActionGasTempoBatchedCalls(t *testing.T) {
	rpc := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var req estimateRPCRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		switch req.Method {
		case "eth_chainId":
			writeEstimateRPCResult(t, w, req.ID, "0x1079") // 4217
		case "eth_estimateGas":
			writeEstimateRPCResult(t, w, req.ID, "0x5208") // 21000
		case "eth_maxPriorityFeePerGas":
			writeEstimateRPCResult(t, w, req.ID, "0x0")
		case "eth_getBlockByNumber":
			writeEstimateRPCResult(t, w, req.ID, map[string]any{
				"baseFeePerGas": "0xe8d4a51000",
			})
		default:
			writeEstimateRPCError(w, req.ID, -32601, fmt.Sprintf("method not supported in test: %s", req.Method))
		}
	}))
	defer rpc.Close()

	// Step with batched Calls (empty Target) — Tempo-style.
	action := Action{
		ActionID:    "act_tempo_batch",
		FromAddress: "0x00000000000000000000000000000000000000aa",
		Steps: []ActionStep{{
			StepID:  "batch-step",
			Type:    StepTypeSwap,
			Status:  StepStatusPending,
			ChainID: "eip155:4217",
			RPCURL:  rpc.URL,
			Target:  "",
			Calls: []StepCall{
				{Target: "0x00000000000000000000000000000000000000bb", Data: "0x", Value: "0"},
				{Target: "0x00000000000000000000000000000000000000cc", Data: "0x", Value: "0"},
			},
		}},
	}

	estimate, err := EstimateActionGas(context.Background(), action, DefaultEstimateOptions())
	if err != nil {
		t.Fatalf("EstimateActionGas failed: %v", err)
	}
	if len(estimate.Steps) != 1 {
		t.Fatalf("expected one step, got %d", len(estimate.Steps))
	}
	// Two calls each estimating 21000 gas => raw gas = 42000
	step := estimate.Steps[0]
	if step.GasEstimateRaw != "42000" {
		t.Fatalf("expected raw gas 42000 for batched calls, got %s", step.GasEstimateRaw)
	}
	if step.FeeUnit != "USDC.e" {
		t.Fatalf("expected fee_unit USDC.e, got %q", step.FeeUnit)
	}
}

func newEstimateRPCServer(t *testing.T) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var req estimateRPCRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		switch req.Method {
		case "eth_chainId":
			writeEstimateRPCResult(t, w, req.ID, "0x1")
		case "eth_estimateGas":
			if len(req.Params) < 2 {
				writeEstimateRPCError(w, req.ID, -32602, "missing block tag")
				return
			}
			var tag string
			if err := json.Unmarshal(req.Params[1], &tag); err != nil {
				writeEstimateRPCError(w, req.ID, -32602, "invalid block tag")
				return
			}
			if tag != "pending" && tag != "latest" {
				writeEstimateRPCError(w, req.ID, -32602, "unsupported block tag")
				return
			}
			writeEstimateRPCResult(t, w, req.ID, "0x5208")
		case "eth_maxPriorityFeePerGas":
			writeEstimateRPCResult(t, w, req.ID, "0x77359400")
		case "eth_getBlockByNumber":
			writeEstimateRPCResult(t, w, req.ID, map[string]any{
				"baseFeePerGas": "0x3b9aca00",
			})
		default:
			writeEstimateRPCError(w, req.ID, -32601, fmt.Sprintf("method not supported in test: %s", req.Method))
		}
	}))
}

func writeEstimateRPCResult(t *testing.T, w http.ResponseWriter, id json.RawMessage, result any) {
	t.Helper()
	w.Header().Set("Content-Type", "application/json")
	resp := map[string]any{
		"jsonrpc": "2.0",
		"id":      decodeEstimateRPCID(id),
		"result":  result,
	}
	if err := json.NewEncoder(w).Encode(resp); err != nil {
		t.Fatalf("encode rpc result: %v", err)
	}
}

func writeEstimateRPCError(w http.ResponseWriter, id json.RawMessage, code int, message string) {
	w.Header().Set("Content-Type", "application/json")
	resp := map[string]any{
		"jsonrpc": "2.0",
		"id":      decodeEstimateRPCID(id),
		"error": map[string]any{
			"code":    code,
			"message": message,
		},
	}
	_ = json.NewEncoder(w).Encode(resp)
}

func decodeEstimateRPCID(raw json.RawMessage) any {
	if len(raw) == 0 {
		return 1
	}
	var out any
	if err := json.Unmarshal(raw, &out); err != nil {
		return 1
	}
	return out
}
