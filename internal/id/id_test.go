package id

import "testing"

func TestParseChainVariants(t *testing.T) {
	chain, err := ParseChain("base")
	if err != nil {
		t.Fatalf("ParseChain(base) failed: %v", err)
	}
	if chain.CAIP2 != "eip155:8453" {
		t.Fatalf("unexpected CAIP2: %s", chain.CAIP2)
	}

	chain, err = ParseChain("8453")
	if err != nil {
		t.Fatalf("ParseChain(8453) failed: %v", err)
	}
	if chain.Slug != "base" {
		t.Fatalf("unexpected slug: %s", chain.Slug)
	}

	chain, err = ParseChain("eip155:999999")
	if err != nil {
		t.Fatalf("ParseChain(eip155:999999) failed: %v", err)
	}
	if chain.EVMChainID != 999999 {
		t.Fatalf("unexpected chain ID: %d", chain.EVMChainID)
	}
}

func TestParseAssetSymbolAndAddress(t *testing.T) {
	chain, _ := ParseChain("ethereum")

	asset, err := ParseAsset("USDC", chain)
	if err != nil {
		t.Fatalf("ParseAsset(USDC) failed: %v", err)
	}
	if asset.AssetID == "" || asset.Decimals != 6 {
		t.Fatalf("unexpected asset result: %+v", asset)
	}

	asset2, err := ParseAsset("0xA0B86991C6218B36C1D19D4A2E9EB0CE3606EB48", chain)
	if err != nil {
		t.Fatalf("ParseAsset(address) failed: %v", err)
	}
	if asset2.Symbol != "USDC" {
		t.Fatalf("expected USDC, got %s", asset2.Symbol)
	}
}

func TestParseAssetChainMismatch(t *testing.T) {
	chain, _ := ParseChain("base")
	_, err := ParseAsset("eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", chain)
	if err == nil {
		t.Fatal("expected chain mismatch error")
	}
}
