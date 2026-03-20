package registry

import "testing"

func TestTempoFeeToken(t *testing.T) {
	cases := []struct {
		chainID int64
		wantOK  bool
	}{
		{4217, true},
		{42431, true},
		{31318, true},
		{1, false},
		{8453, false},
	}
	for _, tc := range cases {
		addr, ok := TempoFeeToken(tc.chainID)
		if ok != tc.wantOK {
			t.Fatalf("TempoFeeToken(%d): got ok=%v, want ok=%v", tc.chainID, ok, tc.wantOK)
		}
		if tc.wantOK && addr == "" {
			t.Fatalf("TempoFeeToken(%d): expected non-empty address", tc.chainID)
		}
	}
}

func TestTempoStablecoinDEX(t *testing.T) {
	cases := []struct {
		chainID int64
		wantOK  bool
	}{
		{4217, true},
		{42431, true},
		{31318, true},
		{1, false},
		{8453, false},
	}
	for _, tc := range cases {
		addr, ok := TempoStablecoinDEX(tc.chainID)
		if ok != tc.wantOK {
			t.Fatalf("TempoStablecoinDEX(%d): got ok=%v, want ok=%v", tc.chainID, ok, tc.wantOK)
		}
		if tc.wantOK && addr == "" {
			t.Fatalf("TempoStablecoinDEX(%d): expected non-empty address", tc.chainID)
		}
	}
}
