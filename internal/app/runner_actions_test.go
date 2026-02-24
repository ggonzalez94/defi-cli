package app

import (
	"bytes"
	"encoding/json"
	"fmt"
	"testing"
)

func TestResolveActionID(t *testing.T) {
	id, err := resolveActionID("act_123", "")
	if err != nil {
		t.Fatalf("resolveActionID failed: %v", err)
	}
	if id != "act_123" {
		t.Fatalf("unexpected action id: %s", id)
	}

	id, err = resolveActionID("", "act_456")
	if err != nil {
		t.Fatalf("resolveActionID with plan id failed: %v", err)
	}
	if id != "act_456" {
		t.Fatalf("unexpected plan-id resolution: %s", id)
	}

	if _, err := resolveActionID("act_1", "act_2"); err == nil {
		t.Fatal("expected mismatch error when action and plan id differ")
	}
}

func TestShouldOpenActionStore(t *testing.T) {
	if !shouldOpenActionStore("swap run") {
		t.Fatal("expected swap run to require action store")
	}
	if !shouldOpenActionStore("bridge plan") {
		t.Fatal("expected bridge plan to require action store")
	}
	if !shouldOpenActionStore("approvals submit") {
		t.Fatal("expected approvals submit to require action store")
	}
	if !shouldOpenActionStore("lend supply status") {
		t.Fatal("expected lend supply status to require action store")
	}
	if !shouldOpenActionStore("rewards claim run") {
		t.Fatal("expected rewards claim run to require action store")
	}
	if !shouldOpenActionStore("actions list") {
		t.Fatal("expected actions list to require action store")
	}
	if shouldOpenActionStore("swap quote") {
		t.Fatal("did not expect swap quote to require action store")
	}
	if shouldOpenActionStore("lend markets") {
		t.Fatal("did not expect lend markets to require action store")
	}
}

func TestShouldOpenCacheBypassesExecutionCommands(t *testing.T) {
	if shouldOpenCache("swap run") {
		t.Fatal("did not expect swap run to open cache")
	}
	if shouldOpenCache("bridge submit") {
		t.Fatal("did not expect bridge submit to open cache")
	}
	if shouldOpenCache("approvals status") {
		t.Fatal("did not expect approvals status to open cache")
	}
	if shouldOpenCache("lend borrow plan") {
		t.Fatal("did not expect lend borrow plan to open cache")
	}
	if shouldOpenCache("rewards compound run") {
		t.Fatal("did not expect rewards compound run to open cache")
	}
	if !shouldOpenCache("lend rates") {
		t.Fatal("expected lend rates to open cache")
	}
	if !shouldOpenCache("bridge quote") {
		t.Fatal("expected bridge quote to open cache")
	}
}

func TestRunnerExecutionCommandsInSchema(t *testing.T) {
	paths := []string{
		"bridge plan",
		"bridge run",
		"approvals plan",
		"approvals run",
		"lend supply plan",
		"lend repay submit",
		"rewards claim plan",
		"rewards compound status",
	}
	for _, path := range paths {
		t.Run(path, func(t *testing.T) {
			var stdout bytes.Buffer
			var stderr bytes.Buffer
			r := NewRunnerWithWriters(&stdout, &stderr)
			code := r.Run([]string{"schema", path, "--results-only"})
			if code != 0 {
				t.Fatalf("expected exit 0 for %q, got %d stderr=%s", path, code, stderr.String())
			}
			var doc map[string]any
			if err := json.Unmarshal(stdout.Bytes(), &doc); err != nil {
				t.Fatalf("failed to parse schema output for %q: %v output=%s", path, err, stdout.String())
			}
			if got, _ := doc["path"].(string); got != fmt.Sprintf("defi %s", path) {
				t.Fatalf("unexpected schema path for %q: got %q", path, got)
			}
		})
	}
}

func TestRunnerSwapPlanRequiresFromAddress(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{
		"swap", "plan",
		"--chain", "taiko",
		"--from-asset", "USDC",
		"--to-asset", "WETH",
		"--amount", "1000000",
	})
	if code != 2 {
		t.Fatalf("expected usage exit code 2, got %d stderr=%s", code, stderr.String())
	}
}

func TestRunnerActionsListBypassesCacheOpen(t *testing.T) {
	setUnopenableCacheEnv(t)

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"actions", "list", "--results-only"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}

	var out []map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &out); err != nil {
		t.Fatalf("failed to parse actions output json: %v output=%s", err, stdout.String())
	}
}

func TestRunnerExecutionStatusBypassesCacheOpen(t *testing.T) {
	setUnopenableCacheEnv(t)

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"approvals", "status", "--action-id", "act_missing"})
	if code != 2 {
		t.Fatalf("expected usage exit code 2, got %d stderr=%s", code, stderr.String())
	}
}
