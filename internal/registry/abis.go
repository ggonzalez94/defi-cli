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

	MoonwellComptrollerABI = `[
		{"name":"getAllMarkets","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address[]"}]},
		{"name":"getAssetsIn","type":"function","stateMutability":"view","inputs":[{"name":"account","type":"address"}],"outputs":[{"name":"","type":"address[]"}]},
		{"name":"checkMembership","type":"function","stateMutability":"view","inputs":[{"name":"account","type":"address"},{"name":"mToken","type":"address"}],"outputs":[{"name":"","type":"bool"}]},
		{"name":"enterMarkets","type":"function","stateMutability":"nonpayable","inputs":[{"name":"mTokens","type":"address[]"}],"outputs":[{"name":"","type":"uint256[]"}]},
		{"name":"markets","type":"function","stateMutability":"view","inputs":[{"name":"","type":"address"}],"outputs":[{"name":"isListed","type":"bool"},{"name":"collateralFactorMantissa","type":"uint256"}]},
		{"name":"oracle","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]}
	]`

	MoonwellMTokenABI = `[
		{"name":"underlying","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]},
		{"name":"symbol","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"string"}]},
		{"name":"decimals","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"uint8"}]},
		{"name":"supplyRatePerTimestamp","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"uint256"}]},
		{"name":"borrowRatePerTimestamp","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"uint256"}]},
		{"name":"totalSupply","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"uint256"}]},
		{"name":"totalBorrowsCurrent","type":"function","stateMutability":"nonpayable","inputs":[],"outputs":[{"name":"","type":"uint256"}]},
		{"name":"exchangeRateCurrent","type":"function","stateMutability":"nonpayable","inputs":[],"outputs":[{"name":"","type":"uint256"}]},
		{"name":"getCash","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"uint256"}]},
		{"name":"getAccountSnapshot","type":"function","stateMutability":"view","inputs":[{"name":"account","type":"address"}],"outputs":[{"name":"","type":"uint256"},{"name":"","type":"uint256"},{"name":"","type":"uint256"},{"name":"","type":"uint256"}]},
		{"name":"mint","type":"function","stateMutability":"nonpayable","inputs":[{"name":"mintAmount","type":"uint256"}],"outputs":[{"name":"","type":"uint256"}]},
		{"name":"redeemUnderlying","type":"function","stateMutability":"nonpayable","inputs":[{"name":"redeemAmount","type":"uint256"}],"outputs":[{"name":"","type":"uint256"}]},
		{"name":"borrow","type":"function","stateMutability":"nonpayable","inputs":[{"name":"borrowAmount","type":"uint256"}],"outputs":[{"name":"","type":"uint256"}]},
		{"name":"repayBorrow","type":"function","stateMutability":"nonpayable","inputs":[{"name":"repayAmount","type":"uint256"}],"outputs":[{"name":"","type":"uint256"}]}
	]`

	MoonwellOracleABI = `[
		{"name":"getUnderlyingPrice","type":"function","stateMutability":"view","inputs":[{"name":"mToken","type":"address"}],"outputs":[{"name":"","type":"uint256"}]}
	]`

	MoonwellERC20MinimalABI = `[
		{"name":"symbol","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"string"}]},
		{"name":"decimals","type":"function","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"uint8"}]}
	]`

	Multicall3ABI = `[
		{"name":"aggregate3","type":"function","stateMutability":"payable","inputs":[{"name":"calls","type":"tuple[]","components":[{"name":"target","type":"address"},{"name":"allowFailure","type":"bool"},{"name":"callData","type":"bytes"}]}],"outputs":[{"name":"returnData","type":"tuple[]","components":[{"name":"success","type":"bool"},{"name":"returnData","type":"bytes"}]}]}
	]`

	MorphoBlueABI = `[
		{"name":"supply","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"data","type":"bytes"}],"outputs":[{"name":"assetsSupplied","type":"uint256"},{"name":"sharesSupplied","type":"uint256"}]},
		{"name":"withdraw","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"receiver","type":"address"}],"outputs":[{"name":"assetsWithdrawn","type":"uint256"},{"name":"sharesWithdrawn","type":"uint256"}]},
		{"name":"borrow","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"receiver","type":"address"}],"outputs":[{"name":"assetsBorrowed","type":"uint256"},{"name":"sharesBorrowed","type":"uint256"}]},
		{"name":"repay","type":"function","stateMutability":"nonpayable","inputs":[{"name":"marketParams","type":"tuple","components":[{"name":"loanToken","type":"address"},{"name":"collateralToken","type":"address"},{"name":"oracle","type":"address"},{"name":"irm","type":"address"},{"name":"lltv","type":"uint256"}]},{"name":"assets","type":"uint256"},{"name":"shares","type":"uint256"},{"name":"onBehalf","type":"address"},{"name":"data","type":"bytes"}],"outputs":[{"name":"assetsRepaid","type":"uint256"},{"name":"sharesRepaid","type":"uint256"}]}
	]`
)
