package id

import (
	"fmt"
	"regexp"
	"sort"
	"strconv"
	"strings"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
)

var (
	eip155ChainPattern      = regexp.MustCompile(`^eip155:[0-9]+$`)
	solanaChainPattern      = regexp.MustCompile(`^solana:[1-9A-HJ-NP-Za-km-z]{32,44}$`)
	evmAddressPattern       = regexp.MustCompile(`^0x[0-9a-fA-F]{40}$`)
	solanaTokenMintPattern  = regexp.MustCompile(`^[1-9A-HJ-NP-Za-km-z]{32,44}$`)
	eip155AssetPattern      = regexp.MustCompile(`^eip155:[0-9]+/erc20:0x[0-9a-fA-F]{40}$`)
	solanaTokenAssetPattern = regexp.MustCompile(`^solana:[1-9A-HJ-NP-Za-km-z]{32,44}/token:[1-9A-HJ-NP-Za-km-z]{32,44}$`)
)

const (
	solanaMainnetRef = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"
	solanaDevnetRef  = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1"
	solanaTestnetRef = "4uhcVJyU9pJkvQyS88uRDiswHXSCkY3z"
)

const (
	solanaMainnetCAIP2 = "solana:" + solanaMainnetRef
	solanaDevnetCAIP2  = "solana:" + solanaDevnetRef
	solanaTestnetCAIP2 = "solana:" + solanaTestnetRef
)

type Chain struct {
	Name       string
	Slug       string
	CAIP2      string
	EVMChainID int64
}

func (c Chain) Namespace() string {
	return chainNamespace(c.CAIP2)
}

func (c Chain) IsEVM() bool {
	return c.Namespace() == "eip155"
}

func (c Chain) IsSolana() bool {
	return c.Namespace() == "solana"
}

type Asset struct {
	ChainID  string
	AssetID  string
	Address  string
	Symbol   string
	Decimals int
}

type Token struct {
	Symbol   string
	Address  string
	Decimals int
}

var chainBySlug = map[string]Chain{
	"ethereum":      {Name: "Ethereum", Slug: "ethereum", CAIP2: "eip155:1", EVMChainID: 1},
	"mainnet":       {Name: "Ethereum", Slug: "ethereum", CAIP2: "eip155:1", EVMChainID: 1},
	"optimism":      {Name: "Optimism", Slug: "optimism", CAIP2: "eip155:10", EVMChainID: 10},
	"op mainnet":    {Name: "Optimism", Slug: "optimism", CAIP2: "eip155:10", EVMChainID: 10},
	"op-mainnet":    {Name: "Optimism", Slug: "optimism", CAIP2: "eip155:10", EVMChainID: 10},
	"bsc":           {Name: "BSC", Slug: "bsc", CAIP2: "eip155:56", EVMChainID: 56},
	"gnosis":        {Name: "Gnosis", Slug: "gnosis", CAIP2: "eip155:100", EVMChainID: 100},
	"xdai":          {Name: "Gnosis", Slug: "gnosis", CAIP2: "eip155:100", EVMChainID: 100},
	"polygon":       {Name: "Polygon", Slug: "polygon", CAIP2: "eip155:137", EVMChainID: 137},
	"sonic":         {Name: "Sonic", Slug: "sonic", CAIP2: "eip155:146", EVMChainID: 146},
	"fraxtal":       {Name: "Fraxtal", Slug: "fraxtal", CAIP2: "eip155:252", EVMChainID: 252},
	"zksync":        {Name: "zkSync Era", Slug: "zksync", CAIP2: "eip155:324", EVMChainID: 324},
	"zksync era":    {Name: "zkSync Era", Slug: "zksync", CAIP2: "eip155:324", EVMChainID: 324},
	"zksync-era":    {Name: "zkSync Era", Slug: "zksync", CAIP2: "eip155:324", EVMChainID: 324},
	"worldchain":    {Name: "World Chain", Slug: "world-chain", CAIP2: "eip155:480", EVMChainID: 480},
	"world chain":   {Name: "World Chain", Slug: "world-chain", CAIP2: "eip155:480", EVMChainID: 480},
	"world-chain":   {Name: "World Chain", Slug: "world-chain", CAIP2: "eip155:480", EVMChainID: 480},
	"mantle":        {Name: "Mantle", Slug: "mantle", CAIP2: "eip155:5000", EVMChainID: 5000},
	"base":          {Name: "Base", Slug: "base", CAIP2: "eip155:8453", EVMChainID: 8453},
	"blast":         {Name: "Blast", Slug: "blast", CAIP2: "eip155:81457", EVMChainID: 81457},
	"berachain":     {Name: "Berachain", Slug: "berachain", CAIP2: "eip155:80094", EVMChainID: 80094},
	"arbitrum":      {Name: "Arbitrum", Slug: "arbitrum", CAIP2: "eip155:42161", EVMChainID: 42161},
	"avalanche":     {Name: "Avalanche", Slug: "avalanche", CAIP2: "eip155:43114", EVMChainID: 43114},
	"linea":         {Name: "Linea", Slug: "linea", CAIP2: "eip155:59144", EVMChainID: 59144},
	"ink":           {Name: "Ink", Slug: "ink", CAIP2: "eip155:57073", EVMChainID: 57073},
	"scroll":        {Name: "Scroll", Slug: "scroll", CAIP2: "eip155:534352", EVMChainID: 534352},
	"celo":          {Name: "Celo", Slug: "celo", CAIP2: "eip155:42220", EVMChainID: 42220},
	"taiko":         {Name: "Taiko", Slug: "taiko", CAIP2: "eip155:167000", EVMChainID: 167000},
	"taiko alethia": {Name: "Taiko", Slug: "taiko", CAIP2: "eip155:167000", EVMChainID: 167000},
	"taiko-alethia": {Name: "Taiko", Slug: "taiko", CAIP2: "eip155:167000", EVMChainID: 167000},
	"solana":        {Name: "Solana", Slug: "solana", CAIP2: solanaMainnetCAIP2},
	"solana-mainnet": {
		Name: "Solana", Slug: "solana", CAIP2: solanaMainnetCAIP2,
	},
	"mainnet-beta":   {Name: "Solana", Slug: "solana", CAIP2: solanaMainnetCAIP2},
	"solana-devnet":  {Name: "Solana Devnet", Slug: "solana-devnet", CAIP2: solanaDevnetCAIP2},
	"solana-testnet": {Name: "Solana Testnet", Slug: "solana-testnet", CAIP2: solanaTestnetCAIP2},
}

var chainByID = map[int64]Chain{
	1:      chainBySlug["ethereum"],
	10:     chainBySlug["optimism"],
	56:     chainBySlug["bsc"],
	100:    chainBySlug["gnosis"],
	137:    chainBySlug["polygon"],
	146:    chainBySlug["sonic"],
	252:    chainBySlug["fraxtal"],
	324:    chainBySlug["zksync"],
	480:    chainBySlug["world-chain"],
	5000:   chainBySlug["mantle"],
	8453:   chainBySlug["base"],
	42220:  chainBySlug["celo"],
	42161:  chainBySlug["arbitrum"],
	43114:  chainBySlug["avalanche"],
	57073:  chainBySlug["ink"],
	59144:  chainBySlug["linea"],
	80094:  chainBySlug["berachain"],
	81457:  chainBySlug["blast"],
	167000: chainBySlug["taiko"],
	534352: chainBySlug["scroll"],
}

var chainByCAIP2 = func() map[string]Chain {
	out := make(map[string]Chain, len(chainBySlug))
	for _, chain := range chainBySlug {
		out[chain.CAIP2] = chain
	}
	return out
}()

// Small bootstrap registry for deterministic asset parsing on Tier-1 chains.
var tokenRegistry = map[string][]Token{
	"eip155:1": {
		{Symbol: "USDC", Address: "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", Decimals: 6},
		{Symbol: "USDT", Address: "0xdac17f958d2ee523a2206206994597c13d831ec7", Decimals: 6},
		{Symbol: "DAI", Address: "0x6b175474e89094c44da98b954eedeac495271d0f", Decimals: 18},
		{Symbol: "WETH", Address: "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", Decimals: 18},
	},
	"eip155:8453": {
		{Symbol: "USDC", Address: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", Decimals: 6},
		{Symbol: "DAI", Address: "0x50c5725949A6F0c72E6C4a641F24049A917DB0Cb", Decimals: 18},
		{Symbol: "WETH", Address: "0x4200000000000000000000000000000000000006", Decimals: 18},
	},
	"eip155:42161": {
		{Symbol: "USDC", Address: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831", Decimals: 6},
		{Symbol: "USDT", Address: "0xFd086bC7CD5C481DCC9C85ebe478A1C0b69FCbb9", Decimals: 6},
		{Symbol: "DAI", Address: "0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1", Decimals: 18},
		{Symbol: "WETH", Address: "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1", Decimals: 18},
	},
	"eip155:10": {
		{Symbol: "USDC", Address: "0x7F5c764cBc14f9669B88837ca1490cCa17c31607", Decimals: 6},
		{Symbol: "USDT", Address: "0x94b008aA00579c1307B0EF2c499aD98a8ce58e58", Decimals: 6},
		{Symbol: "DAI", Address: "0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1", Decimals: 18},
		{Symbol: "WETH", Address: "0x4200000000000000000000000000000000000006", Decimals: 18},
	},
	"eip155:137": {
		{Symbol: "USDC", Address: "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359", Decimals: 6},
		{Symbol: "USDT", Address: "0xc2132D05D31c914a87C6611C10748AEb04B58e8F", Decimals: 6},
		{Symbol: "DAI", Address: "0x8f3Cf7ad23Cd3CaDbD9735AFf958023239c6A063", Decimals: 18},
		{Symbol: "WETH", Address: "0x7ceB23fD6bC0adD59E62ac25578270cFf1b9f619", Decimals: 18},
	},
	"eip155:56": {
		{Symbol: "USDC", Address: "0x8ac76a51cc950d9822d68b83fe1ad97b32cd580d", Decimals: 18},
		{Symbol: "USDT", Address: "0x55d398326f99059fF775485246999027B3197955", Decimals: 18},
		{Symbol: "DAI", Address: "0x1AF3F329e8BE154074D8769D1FFa4eE058B1DBc3", Decimals: 18},
		{Symbol: "WETH", Address: "0x2170Ed0880ac9A755fd29B2688956BD959F933F8", Decimals: 18},
	},
	"eip155:43114": {
		{Symbol: "USDC", Address: "0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E", Decimals: 6},
		{Symbol: "USDT", Address: "0x9702230A8Ea53601f5cD2dc00fDBc13d4dF4A8c7", Decimals: 6},
		{Symbol: "DAI", Address: "0xd586E7F844cEa2F87f50152665BCbc2C279D8d70", Decimals: 18},
		{Symbol: "WETH", Address: "0x49D5c2BdFfac6CE2BFdB6640F4F80f226bc10bAB", Decimals: 18},
	},
	"eip155:100": {
		{Symbol: "USDC", Address: "0xDDAfbb505ad214D7b80b1f830fcCc89B60fb7A83", Decimals: 6},
		{Symbol: "WETH", Address: "0x6A023CCd1ff6F2045C3309768eAd9E68F978f6e1", Decimals: 18},
	},
	"eip155:146": {
		{Symbol: "USDC", Address: "0x29219dd400f2Bf60E5a23d13Be72B486D4038894", Decimals: 6},
		{Symbol: "WETH", Address: "0x50c42dEAcD8Fc9773493ED674b675bE577f2634b", Decimals: 18},
	},
	"eip155:324": {
		{Symbol: "USDC", Address: "0x1d17CBcF0D6D143135aE902365D2E5e2A16538D4", Decimals: 6},
		{Symbol: "USDT", Address: "0x493257fD37EDB34451f62EDf8D2a0C418852bA4C", Decimals: 6},
		{Symbol: "WETH", Address: "0x5AEa5775959fBC2557Cc8789bC1bf90A239D9a91", Decimals: 18},
	},
	"eip155:5000": {
		{Symbol: "USDC", Address: "0x09Bc4E0D864854c6aFB6eB9A9cdF58aC190D0dF9", Decimals: 6},
		{Symbol: "WETH", Address: "0xdEAddEaDdeadDEadDEADDEAddEADDEAddead1111", Decimals: 18},
	},
	"eip155:42220": {
		{Symbol: "USDC", Address: "0xcebA9300f2b948710d2653dD7B07f33A8B32118C", Decimals: 6},
		{Symbol: "WETH", Address: "0xD221812de1BD094f35587EE8E174B07B6167D9Af", Decimals: 18},
	},
	"eip155:57073": {
		{Symbol: "USDC", Address: "0x2D270e6886d130D724215A266106e6832161EAEd", Decimals: 6},
		{Symbol: "WETH", Address: "0x4200000000000000000000000000000000000006", Decimals: 18},
	},
	"eip155:59144": {
		{Symbol: "USDC", Address: "0x176211869cA2b568f2A7D4EE941E073a821EE1ff", Decimals: 6},
		{Symbol: "USDT", Address: "0xA219439258ca9da29E9Cc4cE5596924745e12B93", Decimals: 6},
		{Symbol: "WETH", Address: "0xe5D7C2a44FfDDf6b295A15c148167daaAf5Cf34f", Decimals: 18},
	},
	"eip155:534352": {
		{Symbol: "USDC", Address: "0x06eFdBFf2a14a7c8E15944D1F4A48F9F95F663A4", Decimals: 6},
		{Symbol: "WETH", Address: "0x5300000000000000000000000000000000000004", Decimals: 18},
	},
	"eip155:167000": {
		{Symbol: "USDC", Address: "0x07d83526730c7438048D55A4fc0b850e2aaB6f0b", Decimals: 6},
		{Symbol: "WETH", Address: "0xA51894664A773981C6C112C43ce576f315d5b1B6", Decimals: 18},
	},
	solanaMainnetCAIP2: {
		{Symbol: "USDC", Address: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", Decimals: 6},
		{Symbol: "USDT", Address: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", Decimals: 6},
		{Symbol: "SOL", Address: "So11111111111111111111111111111111111111112", Decimals: 9},
		{Symbol: "JUP", Address: "JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN", Decimals: 6},
		{Symbol: "JTO", Address: "jtojtomepa8beP8AuQc6eXt5FriJwfFMwGQx2v2f9mCL", Decimals: 9},
	},
}

func ParseChain(input string) (Chain, error) {
	raw := strings.TrimSpace(input)
	if raw == "" {
		return Chain{}, clierr.New(clierr.CodeUsage, "chain is required")
	}
	norm := strings.ToLower(raw)

	if chain, ok := chainBySlug[norm]; ok {
		return chain, nil
	}

	if eip155ChainPattern.MatchString(norm) {
		parts := strings.Split(norm, ":")
		id, _ := strconv.ParseInt(parts[1], 10, 64)
		if known, ok := chainByID[id]; ok {
			return known, nil
		}
		return Chain{Name: fmt.Sprintf("EVM-%d", id), Slug: fmt.Sprintf("evm-%d", id), CAIP2: norm, EVMChainID: id}, nil
	}

	if solanaChainPattern.MatchString(raw) {
		if known, ok := chainByCAIP2[raw]; ok {
			return known, nil
		}
		return Chain{Name: "Solana", Slug: "solana-custom", CAIP2: raw}, nil
	}

	if id, err := strconv.ParseInt(norm, 10, 64); err == nil {
		if chain, ok := chainByID[id]; ok {
			return chain, nil
		}
		return Chain{Name: fmt.Sprintf("EVM-%d", id), Slug: fmt.Sprintf("evm-%d", id), CAIP2: fmt.Sprintf("eip155:%d", id), EVMChainID: id}, nil
	}

	return Chain{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("unsupported chain input: %s", input))
}

func ParseAsset(input string, chain Chain) (Asset, error) {
	raw := strings.TrimSpace(input)
	if raw == "" {
		return Asset{}, clierr.New(clierr.CodeUsage, "asset is required")
	}

	if strings.Contains(raw, "/") {
		if !eip155AssetPattern.MatchString(raw) && !solanaTokenAssetPattern.MatchString(raw) {
			return Asset{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("invalid CAIP-19 asset format: %s", input))
		}
		parts := strings.SplitN(raw, "/", 2)
		if len(parts) != 2 {
			return Asset{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("invalid CAIP-19 asset format: %s", input))
		}
		if parts[0] != chain.CAIP2 {
			return Asset{}, clierr.New(clierr.CodeUsage, "asset chain does not match --chain")
		}
		assetParts := strings.SplitN(parts[1], ":", 2)
		if len(assetParts) != 2 {
			return Asset{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("invalid CAIP-19 asset format: %s", input))
		}
		assetNamespace := strings.ToLower(strings.TrimSpace(assetParts[0]))
		address := strings.TrimSpace(assetParts[1])
		if chain.IsEVM() {
			if assetNamespace != "erc20" || !evmAddressPattern.MatchString(address) {
				return Asset{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("invalid CAIP-19 asset format: %s", input))
			}
		} else if chain.IsSolana() {
			if assetNamespace != "token" || !solanaTokenMintPattern.MatchString(address) {
				return Asset{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("invalid CAIP-19 asset format: %s", input))
			}
		} else {
			return Asset{}, clierr.New(clierr.CodeUnsupported, fmt.Sprintf("unsupported chain namespace: %s", chain.Namespace()))
		}
		addr := normalizeTokenAddress(chain.CAIP2, address)
		token, _ := findTokenByAddress(chain.CAIP2, addr)
		return Asset{ChainID: chain.CAIP2, AssetID: canonicalAssetID(chain.CAIP2, addr), Address: addr, Symbol: token.Symbol, Decimals: token.Decimals}, nil
	}

	if chain.IsEVM() && evmAddressPattern.MatchString(raw) {
		addr := normalizeTokenAddress(chain.CAIP2, raw)
		token, _ := findTokenByAddress(chain.CAIP2, addr)
		return Asset{ChainID: chain.CAIP2, AssetID: canonicalAssetID(chain.CAIP2, addr), Address: addr, Symbol: token.Symbol, Decimals: token.Decimals}, nil
	}

	if chain.IsSolana() && solanaTokenMintPattern.MatchString(raw) {
		addr := normalizeTokenAddress(chain.CAIP2, raw)
		token, _ := findTokenByAddress(chain.CAIP2, addr)
		return Asset{ChainID: chain.CAIP2, AssetID: canonicalAssetID(chain.CAIP2, addr), Address: addr, Symbol: token.Symbol, Decimals: token.Decimals}, nil
	}

	matches := findTokensBySymbol(chain.CAIP2, raw)
	if len(matches) == 0 {
		return Asset{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("symbol %s not found in registry for chain %s", input, chain.CAIP2))
	}
	if len(matches) > 1 {
		addresses := make([]string, 0, len(matches))
		for _, m := range matches {
			addresses = append(addresses, m.Address)
		}
		sort.Strings(addresses)
		return Asset{}, clierr.New(clierr.CodeUsage, fmt.Sprintf("symbol %s is ambiguous on chain %s, use address or CAIP-19 (%s)", input, chain.CAIP2, strings.Join(addresses, ", ")))
	}
	t := matches[0]
	addr := normalizeTokenAddress(chain.CAIP2, t.Address)
	return Asset{
		ChainID:  chain.CAIP2,
		AssetID:  canonicalAssetID(chain.CAIP2, addr),
		Address:  addr,
		Symbol:   strings.ToUpper(t.Symbol),
		Decimals: t.Decimals,
	}, nil
}

func chainNamespace(caip2 string) string {
	parts := strings.SplitN(strings.TrimSpace(caip2), ":", 2)
	if len(parts) != 2 {
		return ""
	}
	return strings.ToLower(parts[0])
}

func canonicalAssetID(chainID, address string) string {
	switch chainNamespace(chainID) {
	case "eip155":
		return fmt.Sprintf("%s/erc20:%s", chainID, strings.ToLower(strings.TrimSpace(address)))
	case "solana":
		return fmt.Sprintf("%s/token:%s", chainID, strings.TrimSpace(address))
	default:
		return fmt.Sprintf("%s/asset:%s", chainID, strings.TrimSpace(address))
	}
}

func normalizeTokenAddress(chainID, address string) string {
	address = strings.TrimSpace(address)
	if chainNamespace(chainID) == "eip155" {
		return strings.ToLower(address)
	}
	return address
}

func tokenAddressEqual(chainID, a, b string) bool {
	a = strings.TrimSpace(a)
	b = strings.TrimSpace(b)
	if chainNamespace(chainID) == "eip155" {
		return strings.EqualFold(a, b)
	}
	return a == b
}

func findTokenByAddress(chainID, address string) (Token, bool) {
	for _, t := range tokenRegistry[chainID] {
		if tokenAddressEqual(chainID, t.Address, address) {
			return Token{
				Symbol:   strings.ToUpper(t.Symbol),
				Address:  normalizeTokenAddress(chainID, t.Address),
				Decimals: t.Decimals,
			}, true
		}
	}
	return Token{}, false
}

func findTokensBySymbol(chainID, symbol string) []Token {
	matches := []Token{}
	for _, t := range tokenRegistry[chainID] {
		if strings.EqualFold(t.Symbol, symbol) {
			matches = append(matches, Token{
				Symbol:   strings.ToUpper(t.Symbol),
				Address:  normalizeTokenAddress(chainID, t.Address),
				Decimals: t.Decimals,
			})
		}
	}
	return matches
}

func KnownToken(chainID, symbol string) (Token, bool) {
	matches := findTokensBySymbol(chainID, symbol)
	if len(matches) != 1 {
		return Token{}, false
	}
	return matches[0], true
}

func LookupByAddress(chainID, address string) (Token, bool) {
	return findTokenByAddress(chainID, normalizeTokenAddress(chainID, address))
}
