package app

import (
	"context"
	"fmt"
	"math/big"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/registry"
	"github.com/ggonzalez94/defi-cli/internal/schema"
	"github.com/spf13/cobra"
)

func (s *runtimeState) newWalletCommand() *cobra.Command {
	root := &cobra.Command{Use: "wallet", Short: "Wallet helpers"}

	var chainArg string
	var addressArg string
	var assetArg string
	var rpcURLArg string

	balanceCmd := &cobra.Command{
		Use:   "balance",
		Short: "Query native or ERC-20 token balance for an address",
		RunE: func(cmd *cobra.Command, args []string) error {
			if chainArg == "" {
				return clierr.New(clierr.CodeUsage, "--chain is required")
			}
			if addressArg == "" {
				return clierr.New(clierr.CodeUsage, "--address is required")
			}
			chain, err := id.ParseChain(chainArg)
			if err != nil {
				return err
			}
			if !chain.IsEVM() {
				return clierr.New(clierr.CodeUnsupported, "wallet balance currently supports EVM chains only")
			}
			addr := strings.TrimSpace(addressArg)
			if !common.IsHexAddress(addr) {
				return clierr.New(clierr.CodeUsage, "--address must be a valid EVM hex address")
			}
			address := common.HexToAddress(addr)

			var asset *id.Asset
			if assetArg != "" {
				a, err := id.ParseAsset(assetArg, chain)
				if err != nil {
					return err
				}
				asset = &a
			}

			req := map[string]any{"chain": chain.CAIP2, "address": addr}
			if asset != nil {
				req["asset"] = asset.AssetID
			}
			key := cacheKey(trimRootPath(cmd.CommandPath()), req)

			return s.runCachedCommand(trimRootPath(cmd.CommandPath()), key, 15*time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
				rpcURL, err := registry.ResolveRPCURL(rpcURLArg, chain.EVMChainID)
				if err != nil {
					return nil, nil, nil, false, clierr.Wrap(clierr.CodeUnsupported, "resolve rpc", err)
				}

				start := time.Now()
				result, err := fetchBalance(ctx, rpcURL, chain, address, asset)
				latency := time.Since(start).Milliseconds()
				providerName := fmt.Sprintf("rpc:%s", chain.Slug)
				statuses := []model.ProviderStatus{{Name: providerName, Status: statusFromErr(err), LatencyMS: latency}}
				if err != nil {
					return nil, statuses, nil, false, clierr.Wrap(clierr.CodeUnavailable, "fetch balance", err)
				}
				result.FetchedAt = s.runner.now().UTC().Format(time.RFC3339)
				return result, statuses, nil, false, nil
			})
		},
	}

	balanceCmd.Flags().StringVar(&chainArg, "chain", "", "Chain identifier (CAIP-2, chain ID, or slug)")
	balanceCmd.Flags().StringVar(&addressArg, "address", "", "Wallet address to query")
	balanceCmd.Flags().StringVar(&assetArg, "asset", "", "ERC-20 token (symbol, address, or CAIP-19); omit for native balance")
	balanceCmd.Flags().StringVar(&rpcURLArg, "rpc-url", "", "Override chain default RPC endpoint")
	_ = schema.SetFlagMetadata(balanceCmd.Flags(), "chain", schema.FlagMetadata{Required: true, Format: "chain"})
	_ = schema.SetFlagMetadata(balanceCmd.Flags(), "address", schema.FlagMetadata{Required: true, Format: "evm-address"})
	_ = schema.SetFlagMetadata(balanceCmd.Flags(), "asset", schema.FlagMetadata{Format: "asset"})
	_ = schema.SetFlagMetadata(balanceCmd.Flags(), "rpc-url", schema.FlagMetadata{Format: "url"})

	balanceResponse := schema.TypeSchema{
		Type:        "object",
		Description: "Wallet balance with canonical identifiers and base/decimal amounts",
	}
	_ = schema.SetCommandMetadata(balanceCmd, schema.CommandMetadata{Response: &balanceResponse})

	root.AddCommand(balanceCmd)
	return root
}

type walletRPCClient interface {
	BalanceAt(ctx context.Context, account common.Address, blockNumber *big.Int) (*big.Int, error)
	CallContract(ctx context.Context, msg ethereum.CallMsg, blockNumber *big.Int) ([]byte, error)
}

// fetchBalance queries the on-chain balance for a native token or ERC-20.
func fetchBalance(ctx context.Context, rpcURL string, chain id.Chain, address common.Address, asset *id.Asset) (model.WalletBalance, error) {
	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return model.WalletBalance{}, fmt.Errorf("dial rpc: %w", err)
	}
	defer client.Close()

	if asset == nil {
		return fetchNativeBalance(ctx, client, chain, address)
	}
	return fetchERC20Balance(ctx, client, chain, address, *asset)
}

func fetchNativeBalance(ctx context.Context, client walletRPCClient, chain id.Chain, address common.Address) (model.WalletBalance, error) {
	balance, err := client.BalanceAt(ctx, address, nil)
	if err != nil {
		return model.WalletBalance{}, fmt.Errorf("eth_getBalance: %w", err)
	}

	decimals := 18
	baseUnits := balance.String()
	decimalStr := id.FormatDecimalCompat(baseUnits, decimals)

	return model.WalletBalance{
		ChainID:        chain.CAIP2,
		AccountAddress: strings.ToLower(address.Hex()),
		AssetType:      "native",
		AssetID:        nativeAssetID(chain),
		Symbol:         nativeSymbol(chain),
		Balance: model.AmountInfo{
			AmountBaseUnits: baseUnits,
			AmountDecimal:   decimalStr,
			Decimals:        decimals,
		},
	}, nil
}

var (
	// erc20BalanceOfSelector is the 4-byte selector for balanceOf(address).
	erc20BalanceOfSelector = common.Hex2Bytes("70a08231")
	// erc20DecimalsSelector is the 4-byte selector for decimals().
	erc20DecimalsSelector = common.Hex2Bytes("313ce567")
)

func fetchERC20Balance(ctx context.Context, client walletRPCClient, chain id.Chain, address common.Address, asset id.Asset) (model.WalletBalance, error) {
	if asset.Address == "" {
		return model.WalletBalance{}, fmt.Errorf("asset address is required for ERC-20 balance query")
	}
	tokenAddr := common.HexToAddress(asset.Address)

	// Build balanceOf(address) calldata: selector + abi-encoded address.
	calldata := make([]byte, 4+32)
	copy(calldata[:4], erc20BalanceOfSelector)
	copy(calldata[4+12:], address.Bytes())

	result, err := client.CallContract(ctx, ethereum.CallMsg{
		To:   &tokenAddr,
		Data: calldata,
	}, nil)
	if err != nil {
		return model.WalletBalance{}, fmt.Errorf("balanceOf call: %w", err)
	}
	if len(result) < 32 {
		return model.WalletBalance{}, fmt.Errorf("balanceOf returned %d bytes; target address may not be an ERC-20 contract", len(result))
	}

	balance := new(big.Int).SetBytes(result[:32])

	decimals := asset.Decimals
	if decimals <= 0 {
		decimals, err = fetchERC20Decimals(ctx, client, tokenAddr)
		if err != nil {
			return model.WalletBalance{}, fmt.Errorf("decimals() call: %w", err)
		}
	}
	baseUnits := balance.String()
	decimalStr := id.FormatDecimalCompat(baseUnits, decimals)

	return model.WalletBalance{
		ChainID:        chain.CAIP2,
		AccountAddress: strings.ToLower(address.Hex()),
		AssetType:      "erc20",
		AssetID:        asset.AssetID,
		Symbol:         asset.Symbol,
		Balance: model.AmountInfo{
			AmountBaseUnits: baseUnits,
			AmountDecimal:   decimalStr,
			Decimals:        decimals,
		},
	}, nil
}

// fetchERC20Decimals queries the on-chain decimals() for a token contract.
func fetchERC20Decimals(ctx context.Context, client walletRPCClient, token common.Address) (int, error) {
	result, err := client.CallContract(ctx, ethereum.CallMsg{
		To:   &token,
		Data: erc20DecimalsSelector,
	}, nil)
	if err != nil {
		return 0, err
	}
	if len(result) < 32 {
		return 0, fmt.Errorf("decimals() returned %d bytes; target may not be an ERC-20 contract", len(result))
	}
	d := new(big.Int).SetBytes(result[:32])
	if !d.IsInt64() || d.Int64() < 0 || d.Int64() > 255 {
		return 0, fmt.Errorf("decimals() returned invalid value: %s", d.String())
	}
	return int(d.Int64()), nil
}

func nativeAssetID(chain id.Chain) string {
	_, slip44Ref := nativeAssetInfo(chain)
	return chain.CAIP2 + "/slip44:" + slip44Ref
}

func nativeAssetInfo(chain id.Chain) (symbol string, slip44Ref string) {
	switch chain.EVMChainID {
	case 1, 10, 324, 480, 4326, 534352, 57073, 59144, 81457, 167000, 167013, 42161, 8453:
		return "ETH", "60"
	case 56:
		return "BNB", "714"
	case 100:
		return "XDAI", "700"
	case 137:
		return "POL", "966"
	case 143:
		return "MON", "268435779"
	case 146:
		return "S", "10007"
	case 252:
		return "frxETH", "60"
	case 999:
		return "HYPE", "2457"
	case 4114:
		return "cBTC", "60"
	case 5000:
		return "MNT", "60"
	case 42220:
		return "CELO", "52752"
	case 43114:
		return "AVAX", "9000"
	case 80094:
		return "BERA", "8008"
	default:
		return "ETH", "60"
	}
}

// nativeSymbol returns the conventional native token symbol for a chain.
func nativeSymbol(chain id.Chain) string {
	symbol, _ := nativeAssetInfo(chain)
	return symbol
}
