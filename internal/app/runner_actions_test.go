package app

import (
	"bytes"
	"encoding/json"
	"fmt"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/schema"
	"github.com/spf13/cobra"
)

func TestResolveActionID(t *testing.T) {
	id, err := resolveActionID("act_0123456789abcdef0123456789abcdef")
	if err != nil {
		t.Fatalf("resolveActionID failed: %v", err)
	}
	if id != "act_0123456789abcdef0123456789abcdef" {
		t.Fatalf("unexpected action id: %s", id)
	}

	if _, err := resolveActionID(""); err == nil {
		t.Fatal("expected error when action id is missing")
	}
	if _, err := resolveActionID("act_invalid"); err == nil {
		t.Fatal("expected invalid action id to fail")
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
	if !shouldOpenActionStore("swap plan") {
		t.Fatal("expected swap plan to require action store")
	}
	if !shouldOpenActionStore("bridge plan") {
		t.Fatal("expected bridge plan to require action store")
	}
	if !shouldOpenActionStore("approvals submit") {
		t.Fatal("expected approvals submit to require action store")
	}
	if !shouldOpenActionStore("transfer plan") {
		t.Fatal("expected transfer plan to require action store")
	}
	if !shouldOpenActionStore("lend supply status") {
		t.Fatal("expected lend supply status to require action store")
	}
	if !shouldOpenActionStore("yield deposit plan") {
		t.Fatal("expected yield deposit plan to require action store")
	}
	if !shouldOpenActionStore("rewards claim plan") {
		t.Fatal("expected rewards claim plan to require action store")
	}
	if !shouldOpenActionStore("actions list") {
		t.Fatal("expected actions list to require action store")
	}
	if !shouldOpenActionStore("actions show") {
		t.Fatal("expected actions show to require action store")
	}
	if !shouldOpenActionStore("actions estimate") {
		t.Fatal("expected actions estimate to require action store")
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
	if _, ok := names["estimate"]; !ok {
		t.Fatal("expected actions estimate command to be present")
	}
	if _, ok := names["status"]; ok {
		t.Fatal("did not expect deprecated actions status alias")
	}
}

func TestShouldOpenCacheBypassesExecutionCommands(t *testing.T) {
	if shouldOpenCache("swap submit") {
		t.Fatal("did not expect swap submit to open cache")
	}
	if shouldOpenCache("bridge submit") {
		t.Fatal("did not expect bridge submit to open cache")
	}
	if shouldOpenCache("approvals status") {
		t.Fatal("did not expect approvals status to open cache")
	}
	if shouldOpenCache("transfer status") {
		t.Fatal("did not expect transfer status to open cache")
	}
	if shouldOpenCache("lend borrow plan") {
		t.Fatal("did not expect lend borrow plan to open cache")
	}
	if shouldOpenCache("yield withdraw submit") {
		t.Fatal("did not expect yield withdraw submit to open cache")
	}
	if shouldOpenCache("rewards compound status") {
		t.Fatal("did not expect rewards compound status to open cache")
	}
	if shouldOpenCache("actions show") {
		t.Fatal("did not expect actions show to open cache")
	}
	if shouldOpenCache("actions estimate") {
		t.Fatal("did not expect actions estimate to open cache")
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
		"approvals plan",
		"transfer plan",
		"approvals submit",
		"lend supply plan",
		"lend repay submit",
		"yield deposit plan",
		"yield withdraw status",
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

func TestRunnerTransferPlanSchemaIncludesStructuredInputMetadata(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"schema", "transfer plan", "--results-only"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}

	var doc map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &doc); err != nil {
		t.Fatalf("failed to parse schema output: %v output=%s", err, stdout.String())
	}
	if mutation, _ := doc["mutation"].(bool); !mutation {
		t.Fatalf("expected transfer plan to be marked as mutation, got %#v", doc["mutation"])
	}
	inputModes, ok := doc["input_modes"].([]any)
	if !ok || len(inputModes) == 0 {
		t.Fatalf("expected input modes in schema, got %#v", doc["input_modes"])
	}
	request, ok := doc["request"].(map[string]any)
	if !ok {
		t.Fatalf("expected request schema, got %#v", doc["request"])
	}
	fields, ok := request["fields"].([]any)
	if !ok || len(fields) == 0 {
		t.Fatalf("expected request fields, got %#v", request["fields"])
	}
	foundRecipient := false
	for _, item := range fields {
		field, ok := item.(map[string]any)
		if !ok {
			continue
		}
		if field["name"] == "recipient" {
			foundRecipient = true
			if required, _ := field["required"].(bool); !required {
				t.Fatalf("expected recipient to be required, got %#v", field)
			}
		}
	}
	if !foundRecipient {
		t.Fatalf("expected recipient field in request schema, got %#v", fields)
	}
}

func TestRunnerTransferSubmitSchemaIncludesStructuredInputMetadata(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"schema", "transfer submit", "--results-only"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}

	var doc map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &doc); err != nil {
		t.Fatalf("failed to parse schema output: %v output=%s", err, stdout.String())
	}
	if mutation, _ := doc["mutation"].(bool); !mutation {
		t.Fatalf("expected transfer submit to be marked as mutation, got %#v", doc["mutation"])
	}
	inputModes, ok := doc["input_modes"].([]any)
	if !ok || len(inputModes) == 0 {
		t.Fatalf("expected input modes in schema, got %#v", doc["input_modes"])
	}
	request, ok := doc["request"].(map[string]any)
	if !ok {
		t.Fatalf("expected request schema, got %#v", doc["request"])
	}
	fields, ok := request["fields"].([]any)
	if !ok || len(fields) == 0 {
		t.Fatalf("expected request fields, got %#v", request["fields"])
	}
	foundActionID := false
	foundSigner := false
	for _, item := range fields {
		field, ok := item.(map[string]any)
		if !ok {
			continue
		}
		switch field["name"] {
		case "action_id":
			foundActionID = true
			if required, _ := field["required"].(bool); !required {
				t.Fatalf("expected action_id to be required, got %#v", field)
			}
		case "signer":
			foundSigner = true
			schemaDoc, _ := field["schema"].(map[string]any)
			enumValues, _ := schemaDoc["enum"].([]any)
			if len(enumValues) != 1 || enumValues[0] != "local" {
				t.Fatalf("expected signer enum [local], got %#v", schemaDoc["enum"])
			}
		}
	}
	if !foundActionID {
		t.Fatalf("expected action_id field in request schema, got %#v", fields)
	}
	if !foundSigner {
		t.Fatalf("expected signer field in request schema, got %#v", fields)
	}
}

func TestRunnerTransferPlanAcceptsStructuredInputJSON(t *testing.T) {
	actionStorePath := filepath.Join(t.TempDir(), "actions.db")
	actionLockPath := filepath.Join(t.TempDir(), "actions.lock")
	t.Setenv("DEFI_ACTIONS_PATH", actionStorePath)
	t.Setenv("DEFI_ACTIONS_LOCK_PATH", actionLockPath)

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{
		"transfer", "plan",
		"--input-json", `{"chain":"taiko","asset":"USDC","amount":"1000000","from_address":"0x00000000000000000000000000000000000000aa","recipient":"0x00000000000000000000000000000000000000bb"}`,
		"--results-only",
	})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}

	var action map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &action); err != nil {
		t.Fatalf("failed to parse transfer plan output: %v output=%s", err, stdout.String())
	}
	if action["intent_type"] != "transfer" {
		t.Fatalf("expected transfer intent, got %#v", action["intent_type"])
	}
}

func TestRunnerTransferPlanRejectsInheritedStructuredInputFields(t *testing.T) {
	actionStorePath := filepath.Join(t.TempDir(), "actions.db")
	actionLockPath := filepath.Join(t.TempDir(), "actions.lock")
	t.Setenv("DEFI_ACTIONS_PATH", actionStorePath)
	t.Setenv("DEFI_ACTIONS_LOCK_PATH", actionLockPath)

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{
		"transfer", "plan",
		"--input-json", `{"chain":"taiko","asset":"USDC","amount":"1000000","from_address":"0x00000000000000000000000000000000000000aa","recipient":"0x00000000000000000000000000000000000000bb","timeout":"1s"}`,
	})
	if code != 2 {
		t.Fatalf("expected usage exit 2, got %d stderr=%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "structured input field") || !strings.Contains(stderr.String(), "timeout") || !strings.Contains(stderr.String(), "not supported") {
		t.Fatalf("expected inherited flag rejection, got stderr=%s", stderr.String())
	}
}

func TestRunnerTransferSubmitAcceptsStructuredInputJSON(t *testing.T) {
	actionStorePath := filepath.Join(t.TempDir(), "actions.db")
	actionLockPath := filepath.Join(t.TempDir(), "actions.lock")
	t.Setenv("DEFI_ACTIONS_PATH", actionStorePath)
	t.Setenv("DEFI_ACTIONS_LOCK_PATH", actionLockPath)

	store, err := execution.OpenStore(actionStorePath, actionLockPath)
	if err != nil {
		t.Fatalf("open action store: %v", err)
	}
	defer store.Close()

	action := execution.NewAction("act_0123456789abcdef0123456789abcdef", "transfer", "eip155:167000", execution.Constraints{Simulate: true})
	action.Status = execution.ActionStatusCompleted
	if err := store.Save(action); err != nil {
		t.Fatalf("save action: %v", err)
	}

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{
		"transfer", "submit",
		"--input-json", `{"action_id":"act_0123456789abcdef0123456789abcdef"}`,
		"--results-only",
	})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}

	var result map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &result); err != nil {
		t.Fatalf("failed to parse transfer submit output: %v output=%s", err, stdout.String())
	}
	if result["action_id"] != "act_0123456789abcdef0123456789abcdef" {
		t.Fatalf("unexpected action_id: %#v", result["action_id"])
	}
	if result["status"] != string(execution.ActionStatusCompleted) {
		t.Fatalf("unexpected status: %#v", result["status"])
	}
}

func TestAnnotateStructuredFlagCommandRequestSchemaUsesRequiredFlagMetadata(t *testing.T) {
	var query string
	cmd := &cobra.Command{Use: "quote"}
	cmd.Flags().StringVar(&query, "query", "", "Search query")
	_ = cmd.MarkFlagRequired("query")

	annotateStructuredFlagCommand(cmd, structuredInputOptions{})

	meta := schema.CommandMetadataFor(cmd)
	if meta.Request == nil || len(meta.Request.Fields) != 1 {
		t.Fatalf("expected single request field, got %#v", meta.Request)
	}
	field := meta.Request.Fields[0]
	if field.Name != "query" || !field.Required {
		t.Fatalf("expected required query field, got %#v", field)
	}
}

func TestRunnerBridgeDetailsSchemaIncludesAuthMetadata(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"schema", "bridge details", "--results-only"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}

	var doc map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &doc); err != nil {
		t.Fatalf("failed to parse schema output: %v output=%s", err, stdout.String())
	}
	auth, ok := doc["auth"].([]any)
	if !ok || len(auth) == 0 {
		t.Fatalf("expected auth metadata, got %#v", doc["auth"])
	}
	first, ok := auth[0].(map[string]any)
	if !ok {
		t.Fatalf("unexpected auth metadata shape: %#v", auth[0])
	}
	envVars, ok := first["env_vars"].([]any)
	if !ok || len(envVars) == 0 || envVars[0] != "DEFI_DEFILLAMA_API_KEY" {
		t.Fatalf("unexpected auth env vars: %#v", first["env_vars"])
	}
}

func TestRunnerBridgeQuoteSchemaIncludesRequiredProviderMetadata(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"schema", "bridge quote", "--results-only"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, stderr.String())
	}

	var doc map[string]any
	if err := json.Unmarshal(stdout.Bytes(), &doc); err != nil {
		t.Fatalf("failed to parse schema output: %v output=%s", err, stdout.String())
	}

	request, ok := doc["request"].(map[string]any)
	if !ok {
		t.Fatalf("expected request schema, got %#v", doc["request"])
	}
	fields, ok := request["fields"].([]any)
	if !ok {
		t.Fatalf("expected request fields, got %#v", request["fields"])
	}

	foundProvider := false
	for _, item := range fields {
		field, ok := item.(map[string]any)
		if !ok || field["name"] != "provider" {
			continue
		}
		foundProvider = true
		if required, _ := field["required"].(bool); !required {
			t.Fatalf("expected request.provider to be required, got %#v", field)
		}
		schemaDoc, _ := field["schema"].(map[string]any)
		enumValues, _ := schemaDoc["enum"].([]any)
		if len(enumValues) != 3 || enumValues[0] != "across" || enumValues[1] != "lifi" || enumValues[2] != "bungee" {
			t.Fatalf("unexpected provider enum: %#v", schemaDoc["enum"])
		}
	}
	if !foundProvider {
		t.Fatalf("expected provider field in request schema, got %#v", fields)
	}
}

func TestConfigureStructuredInputSetsRequiredFlagsFromJSON(t *testing.T) {
	type transferBinding struct {
		Chain       string `json:"chain" flag:"chain" required:"true"`
		Asset       string `json:"asset" flag:"asset" required:"true"`
		FromAddress string `json:"from_address" flag:"from-address" required:"true"`
		Recipient   string `json:"recipient" flag:"recipient" required:"true"`
	}

	var binding transferBinding
	cmd := &cobra.Command{Use: "plan"}
	cmd.Flags().StringVar(&binding.Chain, "chain", "", "Chain identifier")
	cmd.Flags().StringVar(&binding.Asset, "asset", "", "Asset")
	cmd.Flags().StringVar(&binding.FromAddress, "from-address", "", "Sender")
	cmd.Flags().StringVar(&binding.Recipient, "recipient", "", "Recipient")
	_ = cmd.MarkFlagRequired("chain")
	_ = cmd.MarkFlagRequired("asset")
	_ = cmd.MarkFlagRequired("from-address")
	_ = cmd.MarkFlagRequired("recipient")
	configureStructuredInput[transferBinding](cmd, structuredInputOptions{Mutation: true})

	if err := cmd.Flags().Set("input-json", `{"chain":"taiko","asset":"USDC","from_address":"0x00000000000000000000000000000000000000aa","recipient":"0x00000000000000000000000000000000000000bb"}`); err != nil {
		t.Fatalf("set input-json: %v", err)
	}
	if cmd.PreRunE == nil {
		t.Fatal("expected structured input pre-run to be configured")
	}
	if err := cmd.PreRunE(cmd, nil); err != nil {
		t.Fatalf("PreRunE failed: %v", err)
	}
	if got := binding.Chain; got != "taiko" {
		t.Fatalf("expected chain from structured input, got %q", got)
	}
	for _, name := range []string{"chain", "asset", "from-address", "recipient"} {
		if !cmd.Flags().Lookup(name).Changed {
			t.Fatalf("expected %s to be marked changed from structured input", name)
		}
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

func TestRunnerTransferPlanRequiresRecipient(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{
		"transfer", "plan",
		"--chain", "taiko",
		"--asset", "USDC",
		"--amount", "1000000",
		"--from-address", "0x00000000000000000000000000000000000000aa",
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

func TestRunnerMorphoYieldDepositPlanRequiresVaultAddress(t *testing.T) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{
		"yield", "deposit", "plan",
		"--provider", "morpho",
		"--chain", "1",
		"--asset", "USDC",
		"--amount", "1000000",
		"--from-address", "0x00000000000000000000000000000000000000aa",
	})
	if code != 2 {
		t.Fatalf("expected usage exit code 2, got %d stderr=%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "--vault-address") {
		t.Fatalf("expected vault-address guidance in error output, got: %s", stderr.String())
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

func TestRunnerActionsEstimateRejectsTempoActions(t *testing.T) {
	actionStorePath := filepath.Join(t.TempDir(), "actions.db")
	actionLockPath := filepath.Join(t.TempDir(), "actions.lock")
	t.Setenv("DEFI_ACTIONS_PATH", actionStorePath)
	t.Setenv("DEFI_ACTIONS_LOCK_PATH", actionLockPath)

	store, err := execution.OpenStore(actionStorePath, actionLockPath)
	if err != nil {
		t.Fatalf("open action store: %v", err)
	}
	defer store.Close()

	action := execution.NewAction("act_0123456789abcdef0123456789abcdef", "swap", "eip155:4217", execution.Constraints{Simulate: true})
	if err := store.Save(action); err != nil {
		t.Fatalf("save action: %v", err)
	}

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"actions", "estimate", "--action-id", action.ActionID})
	if code != 13 {
		t.Fatalf("expected unsupported exit code 13, got %d stderr=%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "Tempo actions") {
		t.Fatalf("expected Tempo estimate rejection, got stderr=%s", stderr.String())
	}
}

func TestRunnerExecutionStatusBypassesCacheOpen(t *testing.T) {
	setUnopenableCacheEnv(t)

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"approvals", "status", "--action-id", "act_0123456789abcdef0123456789abcdef"})
	if code != 2 {
		t.Fatalf("expected usage exit code 2, got %d stderr=%s", code, stderr.String())
	}
}

func TestRunnerSwapStatusRejectsNonSwapIntent(t *testing.T) {
	actionStorePath := filepath.Join(t.TempDir(), "actions.db")
	actionLockPath := filepath.Join(t.TempDir(), "actions.lock")
	t.Setenv("DEFI_ACTIONS_PATH", actionStorePath)
	t.Setenv("DEFI_ACTIONS_LOCK_PATH", actionLockPath)

	store, err := execution.OpenStore(actionStorePath, actionLockPath)
	if err != nil {
		t.Fatalf("open action store: %v", err)
	}
	defer store.Close()

	action := execution.NewAction("act_0123456789abcdef0123456789abcdef", "bridge", "eip155:1", execution.Constraints{Simulate: true})
	if err := store.Save(action); err != nil {
		t.Fatalf("save action: %v", err)
	}

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	r := NewRunnerWithWriters(&stdout, &stderr)
	code := r.Run([]string{"swap", "status", "--action-id", action.ActionID})
	if code != 2 {
		t.Fatalf("expected usage exit code 2, got %d stderr=%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "action is not a swap intent") {
		t.Fatalf("expected swap intent validation error, got stderr=%s", stderr.String())
	}
}

func TestParseActionEstimateOptionsRejectsGasMultiplierLTEOne(t *testing.T) {
	if _, err := parseActionEstimateOptions("", 1, "", "", "pending"); err == nil {
		t.Fatal("expected gas multiplier <= 1 to fail")
	}
}

func TestParseActionEstimateOptionsRejectsUnknownBlockTag(t *testing.T) {
	if _, err := parseActionEstimateOptions("", 1.2, "", "", "safe"); err == nil {
		t.Fatal("expected unknown block tag to fail")
	}
}
