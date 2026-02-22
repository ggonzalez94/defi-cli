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

func TestParseChainExpandedCoverage(t *testing.T) {
	tests := []struct {
		input   string
		chainID int64
		caip2   string
		slug    string
	}{
		{input: "mantle", chainID: 5000, caip2: "eip155:5000", slug: "mantle"},
		{input: "ink", chainID: 57073, caip2: "eip155:57073", slug: "ink"},
		{input: "scroll", chainID: 534352, caip2: "eip155:534352", slug: "scroll"},
		{input: "berachain", chainID: 80094, caip2: "eip155:80094", slug: "berachain"},
		{input: "gnosis", chainID: 100, caip2: "eip155:100", slug: "gnosis"},
		{input: "op mainnet", chainID: 10, caip2: "eip155:10", slug: "optimism"},
		{input: "op-mainnet", chainID: 10, caip2: "eip155:10", slug: "optimism"},
		{input: "xdai", chainID: 100, caip2: "eip155:100", slug: "gnosis"},
		{input: "linea", chainID: 59144, caip2: "eip155:59144", slug: "linea"},
		{input: "sonic", chainID: 146, caip2: "eip155:146", slug: "sonic"},
		{input: "blast", chainID: 81457, caip2: "eip155:81457", slug: "blast"},
		{input: "fraxtal", chainID: 252, caip2: "eip155:252", slug: "fraxtal"},
		{input: "world chain", chainID: 480, caip2: "eip155:480", slug: "world-chain"},
		{input: "world-chain", chainID: 480, caip2: "eip155:480", slug: "world-chain"},
		{input: "worldchain", chainID: 480, caip2: "eip155:480", slug: "world-chain"},
		{input: "celo", chainID: 42220, caip2: "eip155:42220", slug: "celo"},
		{input: "zksync", chainID: 324, caip2: "eip155:324", slug: "zksync"},
		{input: "zksync era", chainID: 324, caip2: "eip155:324", slug: "zksync"},
		{input: "zksync-era", chainID: 324, caip2: "eip155:324", slug: "zksync"},
		{input: "5000", chainID: 5000, caip2: "eip155:5000", slug: "mantle"},
		{input: "324", chainID: 324, caip2: "eip155:324", slug: "zksync"},
		{input: "80094", chainID: 80094, caip2: "eip155:80094", slug: "berachain"},
		{input: "81457", chainID: 81457, caip2: "eip155:81457", slug: "blast"},
		{input: "252", chainID: 252, caip2: "eip155:252", slug: "fraxtal"},
		{input: "480", chainID: 480, caip2: "eip155:480", slug: "world-chain"},
	}

	for _, tc := range tests {
		tc := tc
		t.Run(tc.input, func(t *testing.T) {
			chain, err := ParseChain(tc.input)
			if err != nil {
				t.Fatalf("ParseChain(%s) failed: %v", tc.input, err)
			}
			if chain.EVMChainID != tc.chainID {
				t.Fatalf("expected chain id %d, got %d", tc.chainID, chain.EVMChainID)
			}
			if chain.CAIP2 != tc.caip2 {
				t.Fatalf("expected CAIP2 %s, got %s", tc.caip2, chain.CAIP2)
			}
			if chain.Slug != tc.slug {
				t.Fatalf("expected slug %s, got %s", tc.slug, chain.Slug)
			}
		})
	}
}

func TestParseAssetExpandedChainRegistry(t *testing.T) {
	tests := []struct {
		chainInput string
		symbol     string
	}{
		{chainInput: "mantle", symbol: "USDC"},
		{chainInput: "ink", symbol: "USDC"},
		{chainInput: "scroll", symbol: "USDC"},
		{chainInput: "gnosis", symbol: "USDC"},
		{chainInput: "linea", symbol: "USDC"},
		{chainInput: "sonic", symbol: "USDC"},
		{chainInput: "celo", symbol: "USDC"},
		{chainInput: "zksync", symbol: "USDC"},
	}

	for _, tc := range tests {
		tc := tc
		t.Run(tc.chainInput, func(t *testing.T) {
			chain, err := ParseChain(tc.chainInput)
			if err != nil {
				t.Fatalf("ParseChain(%s) failed: %v", tc.chainInput, err)
			}
			asset, err := ParseAsset(tc.symbol, chain)
			if err != nil {
				t.Fatalf("ParseAsset(%s) failed: %v", tc.symbol, err)
			}
			if asset.Symbol != "USDC" {
				t.Fatalf("expected symbol USDC, got %s", asset.Symbol)
			}
			if asset.ChainID != chain.CAIP2 {
				t.Fatalf("expected chain id %s, got %s", chain.CAIP2, asset.ChainID)
			}
		})
	}
}

func TestParseAssetRequiresAddressOnAddressOnlyChains(t *testing.T) {
	chain, err := ParseChain("blast")
	if err != nil {
		t.Fatalf("ParseChain(blast) failed: %v", err)
	}
	_, err = ParseAsset("USDC", chain)
	if err == nil {
		t.Fatal("expected symbol lookup to fail for address-only chain")
	}
}
