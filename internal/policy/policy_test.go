package policy

import "testing"

func TestCheckCommandAllowed(t *testing.T) {
	if err := CheckCommandAllowed(nil, "yield opportunities"); err != nil {
		t.Fatalf("unexpected error with empty allowlist: %v", err)
	}
	if err := CheckCommandAllowed([]string{"yield opportunities"}, "yield opportunities"); err != nil {
		t.Fatalf("expected command to be allowed: %v", err)
	}
	if err := CheckCommandAllowed([]string{"chains top"}, "yield opportunities"); err == nil {
		t.Fatal("expected command to be blocked")
	}
}
