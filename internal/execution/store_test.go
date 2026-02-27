package execution

import (
	"path/filepath"
	"testing"
)

func TestStoreSaveGetList(t *testing.T) {
	dir := t.TempDir()
	store, err := OpenStore(filepath.Join(dir, "actions.db"), filepath.Join(dir, "actions.lock"))
	if err != nil {
		t.Fatalf("OpenStore failed: %v", err)
	}
	t.Cleanup(func() { _ = store.Close() })

	action := NewAction(NewActionID(), "swap", "eip155:167000", Constraints{SlippageBps: 50, Simulate: true})
	action.Status = ActionStatusPlanned
	action.Steps = append(action.Steps, ActionStep{
		StepID:  "swap-1",
		Type:    StepTypeSwap,
		Status:  StepStatusPending,
		ChainID: "eip155:167000",
		Target:  "0x0000000000000000000000000000000000000001",
		Data:    "0x",
		Value:   "0",
	})
	if err := store.Save(action); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	got, err := store.Get(action.ActionID)
	if err != nil {
		t.Fatalf("Get failed: %v", err)
	}
	if got.ActionID != action.ActionID {
		t.Fatalf("unexpected action id: %s", got.ActionID)
	}
	if got.IntentType != "swap" {
		t.Fatalf("unexpected intent type: %s", got.IntentType)
	}

	got.Status = ActionStatusCompleted
	if err := store.Save(got); err != nil {
		t.Fatalf("Save update failed: %v", err)
	}
	completed, err := store.List(string(ActionStatusCompleted), 10)
	if err != nil {
		t.Fatalf("List failed: %v", err)
	}
	if len(completed) != 1 {
		t.Fatalf("expected one completed action, got %d", len(completed))
	}
}

func TestStoreGetMissingAction(t *testing.T) {
	dir := t.TempDir()
	store, err := OpenStore(filepath.Join(dir, "actions.db"), filepath.Join(dir, "actions.lock"))
	if err != nil {
		t.Fatalf("OpenStore failed: %v", err)
	}
	t.Cleanup(func() { _ = store.Close() })

	if _, err := store.Get("missing"); err == nil {
		t.Fatal("expected missing action error")
	}
}
