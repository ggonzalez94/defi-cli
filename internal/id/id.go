package id

import (
	"fmt"
	"regexp"
	"sort"
	"strconv"
	"strings"

	clierr "github.com/gustavo/defi-cli/internal/errors"
)

var (
	chainPattern = regexp.MustCompile(`^eip155:[0-9]+$`)
	assetPattern = regexp.MustCompile(`^eip155:[0-9]+/erc20:0x[0-9a-fA-F]{40}$`)
	addrPattern  = regexp.MustCompile(`^0x[0-9a-fA-F]{40}$`)
)

type Chain struct {
	Name       string
	Slug       string
	CAIP2      string
	EVMChainID int64
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
	"ethereum":  {Name: "Ethereum", Slug: "ethereum", CAIP2: "eip155:1", EVMChainID: 1},
	"mainnet":   {Name: "Ethereum", Slug: "ethereum", CAIP2: "eip155:1", EVMChainID: 1},
	"base":      {Name: "Base", Slug: "base", CAIP2: "eip155:8453", EVMChainID: 8453},
	"arbitrum":  {Name: "Arbitrum", Slug: "arbitrum", CAIP2: "eip155:42161", EVMChainID: 42161},
	"optimism":  {Name: "Optimism", Slug: "optimism", CAIP2: "eip155:10", EVMChainID: 10},
	"polygon":   {Name: "Polygon", Slug: "polygon", CAIP2: "eip155:137", EVMChainID: 137},
	"avalanche": {Name: "Avalanche", Slug: "avalanche", CAIP2: "eip155:43114", EVMChainID: 43114},
	"bsc":       {Name: "BSC", Slug: "bsc", CAIP2: "eip155:56", EVMChainID: 56},
	"taiko":     {Name: "Taiko", Slug: "taiko", CAIP2: "eip155:167000", EVMChainID: 167000},
}

var chainByID = map[int64]Chain{
	1:      chainBySlug["ethereum"],
	10:     chainBySlug["optimism"],
	56:     chainBySlug["bsc"],
	137:    chainBySlug["polygon"],
	8453:   chainBySlug["base"],
	42161:  chainBySlug["arbitrum"],
	43114:  chainBySlug["avalanche"],
	167000: chainBySlug["taiko"],
}

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
}

func ParseChain(input string) (Chain, error) {
	norm := strings.TrimSpace(strings.ToLower(input))
	if norm == "" {
		return Chain{}, clierr.New(clierr.CodeUsage, "chain is required")
	}

	if chain, ok := chainBySlug[norm]; ok {
		return chain, nil
	}

	if chainPattern.MatchString(norm) {
		parts := strings.Split(norm, ":")
		id, _ := strconv.ParseInt(parts[1], 10, 64)
		if known, ok := chainByID[id]; ok {
			return known, nil
		}
		return Chain{Name: fmt.Sprintf("EVM-%d", id), Slug: fmt.Sprintf("evm-%d", id), CAIP2: norm, EVMChainID: id}, nil
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
	norm := strings.TrimSpace(input)
	if norm == "" {
		return Asset{}, clierr.New(clierr.CodeUsage, "asset is required")
	}

	if assetPattern.MatchString(norm) {
		parts := strings.Split(norm, "/")
		if parts[0] != chain.CAIP2 {
			return Asset{}, clierr.New(clierr.CodeUsage, "asset chain does not match --chain")
		}
		addr := strings.ToLower(strings.TrimPrefix(parts[1], "erc20:"))
		token, _ := findTokenByAddress(chain.CAIP2, addr)
		return Asset{ChainID: chain.CAIP2, AssetID: fmt.Sprintf("%s/erc20:%s", chain.CAIP2, addr), Address: addr, Symbol: token.Symbol, Decimals: token.Decimals}, nil
	}

	if addrPattern.MatchString(norm) {
		addr := strings.ToLower(norm)
		token, _ := findTokenByAddress(chain.CAIP2, addr)
		return Asset{ChainID: chain.CAIP2, AssetID: fmt.Sprintf("%s/erc20:%s", chain.CAIP2, addr), Address: addr, Symbol: token.Symbol, Decimals: token.Decimals}, nil
	}

	matches := findTokensBySymbol(chain.CAIP2, norm)
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
	addr := strings.ToLower(t.Address)
	return Asset{
		ChainID:  chain.CAIP2,
		AssetID:  fmt.Sprintf("%s/erc20:%s", chain.CAIP2, addr),
		Address:  addr,
		Symbol:   strings.ToUpper(t.Symbol),
		Decimals: t.Decimals,
	}, nil
}

func findTokenByAddress(chainID, address string) (Token, bool) {
	for _, t := range tokenRegistry[chainID] {
		if strings.EqualFold(t.Address, address) {
			return Token{Symbol: strings.ToUpper(t.Symbol), Address: strings.ToLower(t.Address), Decimals: t.Decimals}, true
		}
	}
	return Token{}, false
}

func findTokensBySymbol(chainID, symbol string) []Token {
	matches := []Token{}
	for _, t := range tokenRegistry[chainID] {
		if strings.EqualFold(t.Symbol, symbol) {
			matches = append(matches, Token{Symbol: strings.ToUpper(t.Symbol), Address: strings.ToLower(t.Address), Decimals: t.Decimals})
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
	return findTokenByAddress(chainID, strings.ToLower(address))
}
