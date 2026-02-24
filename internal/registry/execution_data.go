package registry

const (
	// Execution provider endpoints.
	LiFiBaseURL = "https://li.quest/v1"
)

// Canonical contracts used by TaikoSwap execution/quoting.
var taikoSwapContractsByChainID = map[int64]struct {
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

func TaikoSwapContracts(chainID int64) (quoterV2 string, router string, ok bool) {
	contracts, ok := taikoSwapContractsByChainID[chainID]
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

// ABI fragments used across execution planners/providers.
const (
	ERC20MinimalABI = `[
		{"name":"allowance","type":"function","stateMutability":"view","inputs":[{"name":"owner","type":"address"},{"name":"spender","type":"address"}],"outputs":[{"name":"","type":"uint256"}]},
		{"name":"approve","type":"function","stateMutability":"nonpayable","inputs":[{"name":"spender","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"","type":"bool"}]}
	]`

	TaikoSwapQuoterV2ABI = `[
		{"name":"quoteExactInputSingle","type":"function","stateMutability":"nonpayable","inputs":[{"name":"params","type":"tuple","components":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"amountIn","type":"uint256"},{"name":"fee","type":"uint24"},{"name":"sqrtPriceLimitX96","type":"uint160"}]}],"outputs":[{"name":"amountOut","type":"uint256"},{"name":"sqrtPriceX96After","type":"uint160"},{"name":"initializedTicksCrossed","type":"uint32"},{"name":"gasEstimate","type":"uint256"}]}
	]`

	TaikoSwapRouterABI = `[
		{"name":"exactInputSingle","type":"function","stateMutability":"payable","inputs":[{"name":"params","type":"tuple","components":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"fee","type":"uint24"},{"name":"recipient","type":"address"},{"name":"amountIn","type":"uint256"},{"name":"amountOutMinimum","type":"uint256"},{"name":"sqrtPriceLimitX96","type":"uint160"}]}],"outputs":[{"name":"amountOut","type":"uint256"}]}
	]`

	AavePoolAddressProviderABI = `[
		{"name":"getPool","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]},
		{"name":"getAddress","type":"function","stateMutability":"view","inputs":[{"name":"id","type":"bytes32"}],"outputs":[{"name":"","type":"address"}]}
	]`

	AavePoolABI = `[
		{"name":"supply","type":"function","stateMutability":"nonpayable","inputs":[{"name":"asset","type":"address"},{"name":"amount","type":"uint256"},{"name":"onBehalfOf","type":"address"},{"name":"referralCode","type":"uint16"}],"outputs":[]},
		{"name":"withdraw","type":"function","stateMutability":"nonpayable","inputs":[{"name":"asset","type":"address"},{"name":"amount","type":"uint256"},{"name":"to","type":"address"}],"outputs":[{"name":"","type":"uint256"}]},
		{"name":"borrow","type":"function","stateMutability":"nonpayable","inputs":[{"name":"asset","type":"address"},{"name":"amount","type":"uint256"},{"name":"interestRateMode","type":"uint256"},{"name":"referralCode","type":"uint16"},{"name":"onBehalfOf","type":"address"}],"outputs":[]},
		{"name":"repay","type":"function","stateMutability":"nonpayable","inputs":[{"name":"asset","type":"address"},{"name":"amount","type":"uint256"},{"name":"interestRateMode","type":"uint256"},{"name":"onBehalfOf","type":"address"}],"outputs":[{"name":"","type":"uint256"}]}
	]`

	AaveRewardsABI = `[
		{"name":"claimRewards","type":"function","stateMutability":"nonpayable","inputs":[{"name":"assets","type":"address[]"},{"name":"amount","type":"uint256"},{"name":"to","type":"address"},{"name":"reward","type":"address"}],"outputs":[{"name":"","type":"uint256"}]}
	]`

	MorphoBlueABI = `[
		{"name":"supply","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"data","type":"bytes"}],"outputs":[{"name":"assetsSupplied","type":"uint256"},{"name":"sharesSupplied","type":"uint256"}]},
		{"name":"withdraw","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"receiver","type":"address"}],"outputs":[{"name":"assetsWithdrawn","type":"uint256"},{"name":"sharesWithdrawn","type":"uint256"}]},
		{"name":"borrow","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"receiver","type":"address"}],"outputs":[{"name":"assetsBorrowed","type":"uint256"},{"name":"sharesBorrowed","type":"uint256"}]},
		{"name":"repay","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"data","type":"bytes"}],"outputs":[{"name":"assetsRepaid","type":"uint256"},{"name":"sharesRepaid","type":"uint256"}]}
	]`
)
