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
		{input: "monad", chainID: 143, caip2: "eip155:143", slug: "monad"},
		{input: "linea", chainID: 59144, caip2: "eip155:59144", slug: "linea"},
		{input: "sonic", chainID: 146, caip2: "eip155:146", slug: "sonic"},
		{input: "blast", chainID: 81457, caip2: "eip155:81457", slug: "blast"},
		{input: "fraxtal", chainID: 252, caip2: "eip155:252", slug: "fraxtal"},
		{input: "world chain", chainID: 480, caip2: "eip155:480", slug: "world-chain"},
		{input: "world-chain", chainID: 480, caip2: "eip155:480", slug: "world-chain"},
		{input: "worldchain", chainID: 480, caip2: "eip155:480", slug: "world-chain"},
		{input: "hyperevm", chainID: 999, caip2: "eip155:999", slug: "hyperevm"},
		{input: "hyper evm", chainID: 999, caip2: "eip155:999", slug: "hyperevm"},
		{input: "hyper-evm", chainID: 999, caip2: "eip155:999", slug: "hyperevm"},
		{input: "citrea", chainID: 4114, caip2: "eip155:4114", slug: "citrea"},
		{input: "megaeth", chainID: 4326, caip2: "eip155:4326", slug: "megaeth"},
		{input: "mega eth", chainID: 4326, caip2: "eip155:4326", slug: "megaeth"},
		{input: "mega-eth", chainID: 4326, caip2: "eip155:4326", slug: "megaeth"},
		{input: "celo", chainID: 42220, caip2: "eip155:42220", slug: "celo"},
		{input: "taiko", chainID: 167000, caip2: "eip155:167000", slug: "taiko"},
		{input: "taiko alethia", chainID: 167000, caip2: "eip155:167000", slug: "taiko"},
		{input: "taiko-alethia", chainID: 167000, caip2: "eip155:167000", slug: "taiko"},
		{input: "zksync", chainID: 324, caip2: "eip155:324", slug: "zksync"},
		{input: "zksync era", chainID: 324, caip2: "eip155:324", slug: "zksync"},
		{input: "zksync-era", chainID: 324, caip2: "eip155:324", slug: "zksync"},
		{input: "5000", chainID: 5000, caip2: "eip155:5000", slug: "mantle"},
		{input: "324", chainID: 324, caip2: "eip155:324", slug: "zksync"},
		{input: "80094", chainID: 80094, caip2: "eip155:80094", slug: "berachain"},
		{input: "81457", chainID: 81457, caip2: "eip155:81457", slug: "blast"},
		{input: "252", chainID: 252, caip2: "eip155:252", slug: "fraxtal"},
		{input: "480", chainID: 480, caip2: "eip155:480", slug: "world-chain"},
		{input: "999", chainID: 999, caip2: "eip155:999", slug: "hyperevm"},
		{input: "4114", chainID: 4114, caip2: "eip155:4114", slug: "citrea"},
		{input: "4326", chainID: 4326, caip2: "eip155:4326", slug: "megaeth"},
		{input: "143", chainID: 143, caip2: "eip155:143", slug: "monad"},
		{input: "167000", chainID: 167000, caip2: "eip155:167000", slug: "taiko"},
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
		{chainInput: "hyperevm", symbol: "USDC"},
		{chainInput: "monad", symbol: "USDC"},
		{chainInput: "citrea", symbol: "USDC"},
		{chainInput: "megaeth", symbol: "USDT"},
		{chainInput: "celo", symbol: "USDC"},
		{chainInput: "taiko", symbol: "USDC"},
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
			if asset.Symbol != tc.symbol {
				t.Fatalf("expected symbol %s, got %s", tc.symbol, asset.Symbol)
			}
			if asset.ChainID != chain.CAIP2 {
				t.Fatalf("expected chain id %s, got %s", chain.CAIP2, asset.ChainID)
			}
		})
	}
}

func TestParseAssetHyperEVMAddressAndCAIP19(t *testing.T) {
	chain, err := ParseChain("hyperevm")
	if err != nil {
		t.Fatalf("ParseChain(hyperevm) failed: %v", err)
	}

	asset, err := ParseAsset("0xb88339cb7199b77e23db6e890353e22632ba630f", chain)
	if err != nil {
		t.Fatalf("ParseAsset(hyperevm address) failed: %v", err)
	}
	if asset.Symbol != "USDC" {
		t.Fatalf("expected USDC, got %s", asset.Symbol)
	}

	caip := "eip155:999/erc20:0x5555555555555555555555555555555555555555"
	asset, err = ParseAsset(caip, chain)
	if err != nil {
		t.Fatalf("ParseAsset(hyperevm caip19) failed: %v", err)
	}
	if asset.Symbol != "WHYPE" {
		t.Fatalf("expected WHYPE, got %s", asset.Symbol)
	}
}

func TestParseAssetExpandedTop20AndTaikoSymbols(t *testing.T) {
	tests := []struct {
		chainInput string
		symbol     string
	}{
		{chainInput: "ethereum", symbol: "AAVE"},
		{chainInput: "ethereum", symbol: "WBTC"},
		{chainInput: "ethereum", symbol: "USD1"},
		{chainInput: "base", symbol: "USDE"},
		{chainInput: "base", symbol: "USDS"},
		{chainInput: "base", symbol: "CBBTC"},
		{chainInput: "base", symbol: "SNX"},
		{chainInput: "arbitrum", symbol: "MORPHO"},
		{chainInput: "arbitrum", symbol: "ARB"},
		{chainInput: "bsc", symbol: "CAKE"},
		{chainInput: "bsc", symbol: "WBNB"},
		{chainInput: "ethereum", symbol: "CRVUSD"},
		{chainInput: "ethereum", symbol: "TUSD"},
		{chainInput: "avalanche", symbol: "EURC"},
		{chainInput: "avalanche", symbol: "WAVAX"},
		{chainInput: "base", symbol: "FRAX"},
		{chainInput: "fraxtal", symbol: "FRAX"},
		{chainInput: "ethereum", symbol: "LDO"},
		{chainInput: "arbitrum", symbol: "UNI"},
		{chainInput: "base", symbol: "ZRO"},
		{chainInput: "scroll", symbol: "ETHFI"},
		{chainInput: "optimism", symbol: "OP"},
		{chainInput: "optimism", symbol: "USDT0"},
		{chainInput: "taiko", symbol: "TAIKO"},
	}

	for _, tc := range tests {
		tc := tc
		t.Run(tc.chainInput+"-"+tc.symbol, func(t *testing.T) {
			chain, err := ParseChain(tc.chainInput)
			if err != nil {
				t.Fatalf("ParseChain(%s) failed: %v", tc.chainInput, err)
			}
			asset, err := ParseAsset(tc.symbol, chain)
			if err != nil {
				t.Fatalf("ParseAsset(%s) failed: %v", tc.symbol, err)
			}
			if asset.Symbol != tc.symbol {
				t.Fatalf("expected symbol %s, got %s", tc.symbol, asset.Symbol)
			}
			if asset.ChainID != chain.CAIP2 {
				t.Fatalf("expected chain id %s, got %s", chain.CAIP2, asset.ChainID)
			}
		})
	}
}

func TestParseAssetFraxtalFraxAddress(t *testing.T) {
	chain, err := ParseChain("fraxtal")
	if err != nil {
		t.Fatalf("ParseChain(fraxtal) failed: %v", err)
	}

	asset, err := ParseAsset("FRAX", chain)
	if err != nil {
		t.Fatalf("ParseAsset(FRAX) failed: %v", err)
	}

	if asset.Address != "0xfc00000000000000000000000000000000000001" {
		t.Fatalf("unexpected FRAX address on fraxtal: %s", asset.Address)
	}
	if asset.Decimals != 18 {
		t.Fatalf("unexpected FRAX decimals on fraxtal: %d", asset.Decimals)
	}
}

func TestParseAssetRequiresAddressWhenSymbolMissingOnChain(t *testing.T) {
	chain, err := ParseChain("blast")
	if err != nil {
		t.Fatalf("ParseChain(blast) failed: %v", err)
	}
	_, err = ParseAsset("USDC", chain)
	if err == nil {
		t.Fatal("expected symbol lookup to fail when symbol is missing on chain")
	}
}

func TestParseAssetMegaETHBootstrapAddresses(t *testing.T) {
	chain, err := ParseChain("megaeth")
	if err != nil {
		t.Fatalf("ParseChain(megaeth) failed: %v", err)
	}

	tests := []struct {
		symbol  string
		address string
	}{
		{symbol: "MEGA", address: "0x28b7e77f82b25b95953825f1e3ea0e36c1c29861"},
		{symbol: "USDT", address: "0xb8ce59fc3717ada4c02eadf9682a9e934f625ebb"},
		{symbol: "WETH", address: "0x4200000000000000000000000000000000000006"},
	}

	for _, tc := range tests {
		asset, err := ParseAsset(tc.symbol, chain)
		if err != nil {
			t.Fatalf("ParseAsset(%s) failed: %v", tc.symbol, err)
		}
		if asset.Address != tc.address {
			t.Fatalf("expected %s address %s, got %s", tc.symbol, tc.address, asset.Address)
		}
	}
}

func TestParseAssetFibrousChainBootstrapAddresses(t *testing.T) {
	tests := []struct {
		chainInput string
		symbol     string
		address    string
	}{
		{chainInput: "hyperevm", symbol: "WHYPE", address: "0x5555555555555555555555555555555555555555"},
		{chainInput: "hyperevm", symbol: "HYPE", address: "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"},
		{chainInput: "monad", symbol: "WMON", address: "0x3bd359c1119da7da1d913d1c4d2b7c461115433a"},
		{chainInput: "monad", symbol: "USDC", address: "0x754704bc059f8c67012fed69bc8a327a5aafb603"},
		{chainInput: "monad", symbol: "MON", address: "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"},
		{chainInput: "citrea", symbol: "WCBTC", address: "0x3100000000000000000000000000000000000006"},
		{chainInput: "citrea", symbol: "CBTC", address: "0x0000000000000000000000000000000000000000"},
	}

	for _, tc := range tests {
		chain, err := ParseChain(tc.chainInput)
		if err != nil {
			t.Fatalf("ParseChain(%s) failed: %v", tc.chainInput, err)
		}
		asset, err := ParseAsset(tc.symbol, chain)
		if err != nil {
			t.Fatalf("ParseAsset(%s) failed: %v", tc.symbol, err)
		}
		if asset.Address != tc.address {
			t.Fatalf("expected %s on %s to resolve to %s, got %s", tc.symbol, tc.chainInput, tc.address, asset.Address)
		}
	}
}
