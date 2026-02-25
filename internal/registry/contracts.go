package registry

// Canonical Uniswap V3-compatible contracts used by swap execution/quoting.
// Today this map includes Taiko deployments and can be extended chain-by-chain.
var uniswapV3ContractsByChainID = map[int64]struct {
	QuoterV2 string
	Router   string
}{
	167000: {
		QuoterV2: "0xcBa70D57be34aA26557B8E80135a9B7754680aDb",
		Router:   "0x1A0c3a0Cfd1791FAC7798FA2b05208B66aaadfeD",
	},
	167013: {
		QuoterV2: "0xAC8D93657DCc5C0dE9d9AF2772aF9eA3A032a1C6",
		Router:   "0x482233e4DBD56853530fA1918157CE59B60dF230",
	},
}

func UniswapV3Contracts(chainID int64) (quoterV2 string, router string, ok bool) {
	contracts, ok := uniswapV3ContractsByChainID[chainID]
	if !ok {
		return "", "", false
	}
	return contracts.QuoterV2, contracts.Router, true
}

// Canonical Aave V3 PoolAddressesProvider contracts used by planners.
var aavePoolAddressProviderByChainID = map[int64]string{
	1:     "0x2f39d218133AFaB8F2B819B1066c7E434Ad94E9e", // Ethereum
	10:    "0xa97684ead0e402dC232d5A977953DF7ECBaB3CDb", // Optimism
	137:   "0xa97684ead0e402dC232d5A977953DF7ECBaB3CDb", // Polygon
	8453:  "0xe20fCBdBfFC4Dd138cE8b2E6FBb6CB49777ad64D", // Base
	42161: "0xa97684ead0e402dC232d5A977953DF7ECBaB3CDb", // Arbitrum
	43114: "0xa97684ead0e402dC232d5A977953DF7ECBaB3CDb", // Avalanche
}

func AavePoolAddressProvider(chainID int64) (string, bool) {
	value, ok := aavePoolAddressProviderByChainID[chainID]
	return value, ok
}
