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

func TestNormalizeAmountMax(t *testing.T) {
	base, dec, err := NormalizeAmount("max", "", 18)
	if err != nil {
		t.Fatalf("NormalizeAmount(max) failed: %v", err)
	}
	if base != MaxUint256 {
		t.Fatalf("expected MaxUint256, got %s", base)
	}
	if dec != "max" {
		t.Fatalf("expected decimal 'max', got %s", dec)
	}

	// Case-insensitive.
	base2, _, err := NormalizeAmount("MAX", "", 6)
	if err != nil {
		t.Fatalf("NormalizeAmount(MAX) failed: %v", err)
	}
	if base2 != MaxUint256 {
		t.Fatalf("expected MaxUint256 for MAX, got %s", base2)
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
