package app

import (
	"bytes"
	"encoding/json"
	"testing"
)

func TestTrimRootPath(t *testing.T) {
	if got := trimRootPath("defi yield opportunities"); got != "yield opportunities" {
		t.Fatalf("unexpected trim result: %s", got)
	}
}

func TestSplitCSV(t *testing.T) {
	items := splitCSV("Aave, morpho ,")
	if len(items) != 2 || items[0] != "aave" || items[1] != "morpho" {
		t.Fatalf("unexpected split: %#v", items)
	}
}

func TestRunnerProvidersList(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"providers", "list", "--results-only"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}
	var out []map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &out); err != nil {
		t.Fatalf("failed to parse output json: %v output=%s", err, stdout.String())
	}
	if len(out) == 0 {
		t.Fatalf("expected providers output, got empty")
	}
}

func TestRunnerErrorEnvelopeIgnoresResultsOnly(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"chains", "top", "--enable-commands", "yield opportunities", "--results-only"})
	if code != 16 {
		t.Fatalf("expected exit 16, got %d stderr=%s", code, stderr.String())
	}
	var env map[string]any
	if err := json.Unmarshal(stderr.Bytes(), &env); err != nil {
		t.Fatalf("failed to parse error envelope: %v output=%s", err, stderr.String())
	}
	if env["success"] != false {
		t.Fatalf("expected success=false, got %v", env["success"])
	}
}
