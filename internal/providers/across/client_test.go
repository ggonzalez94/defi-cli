package across

import "testing"

func TestBaseUnitMathHelpers(t *testing.T) {
	if compareBaseUnits("100", "99") <= 0 {
		t.Fatal("compareBaseUnits expected 100 > 99")
	}
	if out := subtractBaseUnits("1000", "1"); out != "999" {
		t.Fatalf("unexpected subtraction result: %s", out)
	}
	if out := subtractBaseUnits("1", "2"); out != "0" {
		t.Fatalf("unexpected underflow result: %s", out)
	}
}
