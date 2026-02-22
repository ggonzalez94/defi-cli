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

	chain, err = ParseChain("solana")
	if err != nil {
		t.Fatalf("ParseChain(solana) failed: %v", err)
	}
	if !chain.IsSolana() {
		t.Fatalf("expected solana chain, got %+v", chain)
	}

	chain, err = ParseChain("solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp")
	if err != nil {
		t.Fatalf("ParseChain(caip2 solana) failed: %v", err)
	}
	if chain.CAIP2 != "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp" {
		t.Fatalf("unexpected solana CAIP2: %s", chain.CAIP2)
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

func TestParseAssetSolanaSymbolAndMint(t *testing.T) {
	chain, _ := ParseChain("solana")

	asset, err := ParseAsset("USDC", chain)
	if err != nil {
		t.Fatalf("ParseAsset(USDC) on solana failed: %v", err)
	}
	if asset.AssetID != "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp/token:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" {
		t.Fatalf("unexpected solana asset ID: %s", asset.AssetID)
	}

	asset2, err := ParseAsset("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", chain)
	if err != nil {
		t.Fatalf("ParseAsset(mint) on solana failed: %v", err)
	}
	if asset2.Symbol != "USDC" {
		t.Fatalf("expected USDC symbol, got %s", asset2.Symbol)
	}

	asset3, err := ParseAsset("solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp/token:So11111111111111111111111111111111111111112", chain)
	if err != nil {
		t.Fatalf("ParseAsset(caip19) on solana failed: %v", err)
	}
	if asset3.Symbol != "SOL" {
		t.Fatalf("expected SOL symbol, got %s", asset3.Symbol)
	}
}

func TestParseAssetChainMismatch(t *testing.T) {
	chain, _ := ParseChain("base")
	_, err := ParseAsset("eip155:1/erc20:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", chain)
	if err == nil {
		t.Fatal("expected chain mismatch error")
	}
}

func TestParseAssetSolanaChainMismatch(t *testing.T) {
	chain, _ := ParseChain("solana")
	_, err := ParseAsset("solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1/token:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", chain)
	if err == nil {
		t.Fatal("expected chain mismatch error")
	}
}
