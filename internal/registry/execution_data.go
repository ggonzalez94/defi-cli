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
	1: "0x2f39d218133AFaB8F2B819B1066c7E434Ad94E9e",
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
)
