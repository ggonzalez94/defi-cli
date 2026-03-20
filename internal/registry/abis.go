package registry

// ABI fragments used across execution planners/providers.
const (
	ERC20MinimalABI = `[
		{"name":"allowance","type":"function","stateMutability":"view","inputs":[{"name":"owner","type":"address"},{"name":"spender","type":"address"}],"outputs":[{"name":"","type":"uint256"}]},
		{"name":"approve","type":"function","stateMutability":"nonpayable","inputs":[{"name":"spender","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"","type":"bool"}]},
		{"name":"transfer","type":"function","stateMutability":"nonpayable","inputs":[{"name":"to","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"","type":"bool"}]}
	]`

	ERC4626VaultABI = `[
		{"name":"asset","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]},
		{"name":"deposit","type":"function","stateMutability":"nonpayable","inputs":[{"name":"assets","type":"uint256"},{"name":"receiver","type":"address"}],"outputs":[{"name":"shares","type":"uint256"}]},
		{"name":"withdraw","type":"function","stateMutability":"nonpayable","inputs":[{"name":"assets","type":"uint256"},{"name":"receiver","type":"address"},{"name":"owner","type":"address"}],"outputs":[{"name":"shares","type":"uint256"}]}
	]`

	UniswapV3QuoterV2ABI = `[
		{"name":"quoteExactInputSingle","type":"function","stateMutability":"nonpayable","inputs":[{"name":"params","type":"tuple","components":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"amountIn","type":"uint256"},{"name":"fee","type":"uint24"},{"name":"sqrtPriceLimitX96","type":"uint160"}]}],"outputs":[{"name":"amountOut","type":"uint256"},{"name":"sqrtPriceX96After","type":"uint160"},{"name":"initializedTicksCrossed","type":"uint32"},{"name":"gasEstimate","type":"uint256"}]}
	]`

	UniswapV3RouterABI = `[
		{"name":"exactInputSingle","type":"function","stateMutability":"payable","inputs":[{"name":"params","type":"tuple","components":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"fee","type":"uint24"},{"name":"recipient","type":"address"},{"name":"amountIn","type":"uint256"},{"name":"amountOutMinimum","type":"uint256"},{"name":"sqrtPriceLimitX96","type":"uint160"}]}],"outputs":[{"name":"amountOut","type":"uint256"}]}
	]`

	TempoStablecoinDEXABI = `[
		{"name":"quoteSwapExactAmountIn","type":"function","stateMutability":"view","inputs":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"amountIn","type":"uint128"}],"outputs":[{"name":"amountOut","type":"uint128"}]},
		{"name":"quoteSwapExactAmountOut","type":"function","stateMutability":"view","inputs":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"amountOut","type":"uint128"}],"outputs":[{"name":"amountIn","type":"uint128"}]},
		{"name":"swapExactAmountIn","type":"function","stateMutability":"nonpayable","inputs":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"amountIn","type":"uint128"},{"name":"minAmountOut","type":"uint128"}],"outputs":[{"name":"amountOut","type":"uint128"}]},
		{"name":"swapExactAmountOut","type":"function","stateMutability":"nonpayable","inputs":[{"name":"tokenIn","type":"address"},{"name":"tokenOut","type":"address"},{"name":"amountOut","type":"uint128"},{"name":"maxAmountIn","type":"uint128"}],"outputs":[{"name":"amountIn","type":"uint128"}]}
	]`

	TempoTIP20MetadataABI = `[
		{"name":"currency","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"string"}]},
		{"name":"quoteToken","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]}
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
