package id

import "testing"

func TestNormalizeAmountBaseUnits(t *testing.T) {
	base, dec, err := NormalizeAmount("1000000", "", 6)
	if err != nil {
		t.Fatalf("NormalizeAmount failed: %v", err)
	}
	if base != "1000000" || dec != "1" {
		t.Fatalf("unexpected result: base=%s dec=%s", base, dec)
	}
}

func TestNormalizeAmountDecimal(t *testing.T) {
	base, dec, err := NormalizeAmount("", "1.25", 6)
	if err != nil {
		t.Fatalf("NormalizeAmount failed: %v", err)
	}
	if base != "1250000" || dec != "1.25" {
		t.Fatalf("unexpected result: base=%s dec=%s", base, dec)
	}
}

func TestNormalizeAmountValidation(t *testing.T) {
	if _, _, err := NormalizeAmount("10", "1", 6); err == nil {
		t.Fatal("expected mutual exclusivity error")
	}
	if _, _, err := NormalizeAmount("", "1.1234567", 6); err == nil {
		t.Fatal("expected precision error")
	}
	if got := FormatDecimalCompat("0", 6); got != "0" {
		t.Fatalf("unexpected zero format: %s", got)
	}
}
