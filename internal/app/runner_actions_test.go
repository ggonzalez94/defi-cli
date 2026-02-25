package app

import (
	"bytes"
	"encoding/json"
	"fmt"
	"strings"
	"testing"

	execsigner "github.com/ggonzalez94/defi-cli/internal/execution/signer"
)

const runSignerTestPrivateKey = "59c6995e998f97a5a0044976f0945388cf9b7e5e5f4f9d2d9d8f1f5b7f6d11d1"

func TestResolveActionID(t *testing.T) {
	id, err := resolveActionID("act_123")
	if err != nil {
		t.Fatalf("resolveActionID failed: %v", err)
	}
	if id != "act_123" {
		t.Fatalf("unexpected action id: %s", id)
	}

	if _, err := resolveActionID(""); err == nil {
		t.Fatal("expected error when action id is missing")
	}
}

func TestResolveRunSignerAndFromAddressDefaultsToSigner(t *testing.T) {
	t.Setenv(execsigner.EnvPrivateKey, runSignerTestPrivateKey)
	txSigner, fromAddress, err := resolveRunSignerAndFromAddress("local", execsigner.KeySourceEnv, "", "")
	if err != nil {
		t.Fatalf("resolveRunSignerAndFromAddress failed: %v", err)
	}
	if txSigner == nil {
		t.Fatal("expected non-nil signer")
	}
	if fromAddress == "" {
		t.Fatal("expected non-empty from address")
	}
	if !strings.EqualFold(fromAddress, txSigner.Address().Hex()) {
		t.Fatalf("expected from address %s to match signer %s", fromAddress, txSigner.Address().Hex())
	}
}

func TestResolveRunSignerAndFromAddressRejectsMismatch(t *testing.T) {
	t.Setenv(execsigner.EnvPrivateKey, runSignerTestPrivateKey)
	_, _, err := resolveRunSignerAndFromAddress("local", execsigner.KeySourceEnv, "", "0x0000000000000000000000000000000000000001")
	if err == nil {
		t.Fatal("expected mismatch error")
	}
	if !strings.Contains(err.Error(), "--from-address") {
		t.Fatalf("expected --from-address mismatch error, got: %v", err)
	}
}

func TestResolveRunSignerAndFromAddressUsesPrivateKeyOverride(t *testing.T) {
	t.Setenv(execsigner.EnvPrivateKey, "")
	txSigner, fromAddress, err := resolveRunSignerAndFromAddress("local", execsigner.KeySourceAuto, runSignerTestPrivateKey, "")
	if err != nil {
		t.Fatalf("resolveRunSignerAndFromAddress failed with private key override: %v", err)
	}
	if txSigner == nil {
		t.Fatal("expected non-nil signer")
	}
	if fromAddress == "" {
		t.Fatal("expected non-empty from address")
	}
	if !strings.EqualFold(fromAddress, txSigner.Address().Hex()) {
		t.Fatalf("expected from address %s to match signer %s", fromAddress, txSigner.Address().Hex())
	}
}

func TestParseExecuteOptionsRejectsGasMultiplierLTEOne(t *testing.T) {
	if _, err := parseExecuteOptions(true, "2s", "2m", 1, "", "", false, false); err == nil {
		t.Fatal("expected gas multiplier <= 1 to fail")
	}
}

func TestParseExecuteOptionsAcceptsGasMultiplierAboveOne(t *testing.T) {
	opts, err := parseExecuteOptions(true, "2s", "2m", 1.05, "", "", true, true)
	if err != nil {
		t.Fatalf("expected parseExecuteOptions to succeed, got %v", err)
	}
	if opts.GasMultiplier != 1.05 {
		t.Fatalf("expected gas multiplier 1.05, got %f", opts.GasMultiplier)
	}
	if !opts.AllowMaxApproval {
		t.Fatal("expected AllowMaxApproval=true")
	}
	if !opts.UnsafeProviderTx {
		t.Fatal("expected UnsafeProviderTx=true")
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
	if !shouldOpenActionStore("actions show") {
		t.Fatal("expected actions show to require action store")
	}
	if shouldOpenActionStore("swap quote") {
		t.Fatal("did not expect swap quote to require action store")
	}
	if shouldOpenActionStore("lend markets") {
		t.Fatal("did not expect lend markets to require action store")
	}
}

func TestActionsCommandHasNoStatusAlias(t *testing.T) {
	state := &runtimeState{}
	actionsCmd := state.newActionsCommand()

	names := map[string]struct{}{}
	for _, cmd := range actionsCmd.Commands() {
		names[cmd.Name()] = struct{}{}
	}

	if _, ok := names["list"]; !ok {
		t.Fatal("expected actions list command to be present")
	}
	if _, ok := names["show"]; !ok {
		t.Fatal("expected actions show command to be present")
	}
	if _, ok := names["status"]; ok {
		t.Fatal("did not expect deprecated actions status alias")
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
	if shouldOpenCache("actions show") {
		t.Fatal("did not expect actions show to open cache")
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

func TestRunnerMorphoLendPlanRequiresMarketID(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{
		"lend", "supply", "plan",
		"--provider", "morpho",
		"--chain", "1",
		"--asset", "USDC",
		"--amount", "1000000",
		"--from-address", "0x00000000000000000000000000000000000000aa",
	})
	if code != 2 {
		t.Fatalf("expected usage exit code 2, got %d stderr=%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "--market-id") {
		t.Fatalf("expected market-id guidance in error output, got: %s", stderr.String())
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

func TestRunnerActionsStatusRejected(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"actions", "status"})
	if code != 2 {
		t.Fatalf("expected usage exit code 2, got %d stderr=%s", code, stderr.String())
	}

	var env map[string]any
	if err := json.Unmarshal(stderr.Bytes(), &env); err != nil {
		t.Fatalf("failed to parse error envelope: %v output=%s", err, stderr.String())
	}
	errBody, ok := env["error"].(map[string]any)
	if !ok {
		t.Fatalf("expected error body, got %+v", env["error"])
	}
	msg, _ := errBody["message"].(string)
	if !strings.Contains(msg, "unknown actions subcommand") {
		t.Fatalf("expected unknown actions subcommand message, got %q", msg)
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
