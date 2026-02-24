package execution

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
)

func TestVerifyBridgeSettlementNoopForNonBridgeStep(t *testing.T) {
	step := &ActionStep{Type: StepTypeApproval}
	err := verifyBridgeSettlement(context.Background(), step, "0xabc", ExecuteOptions{
		PollInterval: 5 * time.Millisecond,
		StepTimeout:  100 * time.Millisecond,
	})
	if err != nil {
		t.Fatalf("expected no-op settlement verification, got err=%v", err)
	}
}

func TestVerifyBridgeSettlementLiFiSuccess(t *testing.T) {
	var calls int
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls++
		if got := r.URL.Query().Get("txHash"); got != "abc" {
			t.Fatalf("expected txHash query param without 0x prefix, got %q", got)
		}
		if calls == 1 {
			_, _ = fmt.Fprint(w, `{"status":"PENDING","substatus":"WAIT_DESTINATION_TRANSACTION"}`)
			return
		}
		_, _ = fmt.Fprint(w, `{"status":"DONE","substatus":"COMPLETED","receiving":{"txHash":"0xdestination"}}`)
	}))
	defer srv.Close()

	step := &ActionStep{
		Type: StepTypeBridge,
		ExpectedOutputs: map[string]string{
			"settlement_provider":        "lifi",
			"settlement_status_endpoint": srv.URL,
			"settlement_bridge":          "across",
			"settlement_from_chain":      "1",
			"settlement_to_chain":        "8453",
		},
	}
	err := verifyBridgeSettlement(context.Background(), step, "0xabc", ExecuteOptions{
		PollInterval: 5 * time.Millisecond,
		StepTimeout:  200 * time.Millisecond,
	})
	if err != nil {
		t.Fatalf("expected successful settlement verification, got err=%v", err)
	}
	if step.ExpectedOutputs["settlement_status"] != "DONE" {
		t.Fatalf("expected settlement status DONE, got %q", step.ExpectedOutputs["settlement_status"])
	}
	if step.ExpectedOutputs["destination_tx_hash"] != "0xdestination" {
		t.Fatalf("expected destination tx hash, got %q", step.ExpectedOutputs["destination_tx_hash"])
	}
}

func TestVerifyBridgeSettlementLiFiFailed(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = fmt.Fprint(w, `{"status":"FAILED","substatusMessage":"bridge route failed"}`)
	}))
	defer srv.Close()

	step := &ActionStep{
		Type: StepTypeBridge,
		ExpectedOutputs: map[string]string{
			"settlement_provider":        "lifi",
			"settlement_status_endpoint": srv.URL,
		},
	}
	err := verifyBridgeSettlement(context.Background(), step, "0xabc", ExecuteOptions{
		PollInterval: 5 * time.Millisecond,
		StepTimeout:  100 * time.Millisecond,
	})
	if err == nil {
		t.Fatal("expected settlement failure error")
	}
	if !strings.Contains(err.Error(), "bridge settlement failed") {
		t.Fatalf("expected bridge settlement failed error, got %v", err)
	}
}

func TestVerifyBridgeSettlementUnsupportedProvider(t *testing.T) {
	step := &ActionStep{
		Type: StepTypeBridge,
		ExpectedOutputs: map[string]string{
			"settlement_provider": "unknown",
		},
	}
	err := verifyBridgeSettlement(context.Background(), step, "0xabc", ExecuteOptions{
		PollInterval: 5 * time.Millisecond,
		StepTimeout:  100 * time.Millisecond,
	})
	if err == nil {
		t.Fatal("expected unsupported settlement provider error")
	}
	cErr, ok := clierr.As(err)
	if !ok || cErr.Code != clierr.CodeUnsupported {
		t.Fatalf("expected unsupported code, got err=%v", err)
	}
}

func TestVerifyBridgeSettlementAcrossSuccess(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if got := r.URL.Query().Get("depositTxHash"); got != "0xabc" {
			t.Fatalf("expected depositTxHash 0xabc, got %q", got)
		}
		if got := r.URL.Query().Get("originChainId"); got != "1" {
			t.Fatalf("expected originChainId=1, got %q", got)
		}
		_, _ = fmt.Fprint(w, `{"status":"filled","fillTx":"0xdestination"}`)
	}))
	defer srv.Close()

	step := &ActionStep{
		Type: StepTypeBridge,
		ExpectedOutputs: map[string]string{
			"settlement_provider":        "across",
			"settlement_status_endpoint": srv.URL,
			"settlement_origin_chain":    "1",
		},
	}
	err := verifyBridgeSettlement(context.Background(), step, "0xabc", ExecuteOptions{
		PollInterval: 5 * time.Millisecond,
		StepTimeout:  200 * time.Millisecond,
	})
	if err != nil {
		t.Fatalf("expected successful across settlement verification, got err=%v", err)
	}
	if step.ExpectedOutputs["settlement_status"] != "filled" {
		t.Fatalf("expected settlement status filled, got %q", step.ExpectedOutputs["settlement_status"])
	}
	if step.ExpectedOutputs["destination_tx_hash"] != "0xdestination" {
		t.Fatalf("expected destination tx hash, got %q", step.ExpectedOutputs["destination_tx_hash"])
	}
}

func TestVerifyBridgeSettlementAcrossRefunded(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = fmt.Fprint(w, `{"status":"refunded","depositRefundTxHash":"0xrefund"}`)
	}))
	defer srv.Close()

	step := &ActionStep{
		Type: StepTypeBridge,
		ExpectedOutputs: map[string]string{
			"settlement_provider":        "across",
			"settlement_status_endpoint": srv.URL,
		},
	}
	err := verifyBridgeSettlement(context.Background(), step, "0xabc", ExecuteOptions{
		PollInterval: 5 * time.Millisecond,
		StepTimeout:  100 * time.Millisecond,
	})
	if err == nil {
		t.Fatal("expected across refunded status to fail")
	}
	if !strings.Contains(err.Error(), "refunded") {
		t.Fatalf("expected refunded error, got %v", err)
	}
}
