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

func TestLCXLibertyRouter(t *testing.T) {
	const (
		routerA = "0x0000000000b1ae06c96aB73FF4360d7AeAaB56F0"
		routerB = "0x0000000000654902B6Ed6cC240cc81CB4c4a3dff"
	)
	cases := []struct {
		chainID int64
		want    string
		wantOK  bool
	}{
		{1, routerA, true},
		{10, routerA, true},
		{8453, routerA, true},
		{42161, routerA, true},
		{56, routerB, true},
		{137, routerB, true},
		{324, routerB, true},
		{999, routerB, true},
		{9745, routerB, true},
		{167000, "", false},
		{4217, "", false},
	}
	for _, tc := range cases {
		addr, ok := LCXLibertyRouter(tc.chainID)
		if ok != tc.wantOK || addr != tc.want {
			t.Fatalf("LCXLibertyRouter(%d): got (%q, %v), want (%q, %v)", tc.chainID, addr, ok, tc.want, tc.wantOK)
		}
	}
	if len(lcxLibertyRouterByChainID) != 22 {
		t.Fatalf("expected 22 LCX Liberty DEX chains, got %d", len(lcxLibertyRouterByChainID))
	}
}
