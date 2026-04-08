package app

import (
	"context"

	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/execution/actionbuilder"
	"github.com/ggonzalez94/defi-cli/internal/execution/planner"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/spf13/cobra"
)

func (s *runtimeState) addLendExecutionSubcommands(root *cobra.Command) {
	root.AddCommand(s.newLendVerbExecutionCommand(planner.AaveVerbSupply, "Supply assets to a lending protocol"))
	root.AddCommand(s.newLendVerbExecutionCommand(planner.AaveVerbWithdraw, "Withdraw assets from a lending protocol"))
	root.AddCommand(s.newLendVerbExecutionCommand(planner.AaveVerbBorrow, "Borrow assets from a lending protocol"))
	root.AddCommand(s.newLendVerbExecutionCommand(planner.AaveVerbRepay, "Repay borrowed assets on a lending protocol"))
}

func (s *runtimeState) newLendVerbExecutionCommand(verb planner.AaveLendVerb, short string) *cobra.Command {
	root := &cobra.Command{
		Use:   string(verb),
		Short: short,
	}
	expectedIntent := "lend_" + string(verb)

	type lendArgs struct {
		Provider            string `json:"provider" flag:"provider" required:"true" enum:"aave,morpho,moonwell"`
		ChainArg            string `json:"chain" flag:"chain" required:"true" format:"chain"`
		AssetArg            string `json:"asset" flag:"asset" required:"true" format:"asset"`
		MarketID            string `json:"market_id" flag:"market-id" format:"bytes32"`
		AmountBase          string `json:"amount" flag:"amount" format:"base-units"`
		AmountDecimal       string `json:"amount_decimal" flag:"amount-decimal" format:"decimal-amount"`
		WalletRef           string `json:"wallet" flag:"wallet" format:"identifier"`
		FromAddress         string `json:"from_address" flag:"from-address" format:"evm-address"`
		Recipient           string `json:"recipient" flag:"recipient" format:"evm-address"`
		OnBehalfOf          string `json:"on_behalf_of" flag:"on-behalf-of" format:"evm-address"`
		InterestRateMode    int64  `json:"interest_rate_mode" flag:"interest-rate-mode"`
		Simulate            bool   `json:"simulate" flag:"simulate"`
		RPCURL              string `json:"rpc_url" flag:"rpc-url" format:"url"`
		PoolAddress         string `json:"pool_address" flag:"pool-address" format:"evm-address"`
		PoolAddressProvider string `json:"pool_address_provider" flag:"pool-address-provider" format:"evm-address"`
	}
	buildAction := func(ctx context.Context, args lendArgs) (execution.Action, error) {
		chain, asset, err := parseChainAsset(args.ChainArg, args.AssetArg)
		if err != nil {
			return execution.Action{}, err
		}
		decimals := asset.Decimals
		if decimals <= 0 {
			decimals = 18
		}
		base, _, err := id.NormalizeAmount(args.AmountBase, args.AmountDecimal, decimals)
		if err != nil {
			return execution.Action{}, err
		}
		return s.actionBuilderRegistry().BuildLendAction(ctx, actionbuilder.LendRequest{
			Provider:            args.Provider,
			Verb:                verb,
			Chain:               chain,
			Asset:               asset,
			MarketID:            args.MarketID,
			AmountBaseUnits:     base,
			Sender:              args.FromAddress,
			Recipient:           args.Recipient,
			OnBehalfOf:          args.OnBehalfOf,
			InterestRateMode:    args.InterestRateMode,
			Simulate:            args.Simulate,
			RPCURL:              args.RPCURL,
			PoolAddress:         args.PoolAddress,
			PoolAddressProvider: args.PoolAddressProvider,
		})
	}

	var plan lendArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist a lend action plan",
		RunE: func(cmd *cobra.Command, _ []string) error {
			providerName := providers.NormalizeLendingProvider(plan.Provider)
			if providerName == "" {
				providerName = "lend"
			}
			return s.runPlanAction(cmd, planActionConfig{
				ProviderName: providerName,
				WalletRef:    plan.WalletRef,
				FromAddress:  plan.FromAddress,
				ChainArg:     plan.ChainArg,
				BuildAction: func(ctx context.Context, fromAddr string) (execution.Action, error) {
					p := plan
					p.FromAddress = fromAddr
					return buildAction(ctx, p)
				},
			})
		},
	}
	planCmd.Flags().StringVar(&plan.Provider, "provider", "", "Lending provider (aave|morpho|moonwell)")
	planCmd.Flags().StringVar(&plan.ChainArg, "chain", "", "Chain identifier")
	planCmd.Flags().StringVar(&plan.AssetArg, "asset", "", "Asset symbol/address/CAIP-19")
	planCmd.Flags().StringVar(&plan.MarketID, "market-id", "", "Morpho market unique key (required for --provider morpho)")
	planCmd.Flags().StringVar(&plan.AmountBase, "amount", "", "Amount in base units")
	planCmd.Flags().StringVar(&plan.AmountDecimal, "amount-decimal", "", "Amount in decimal units")
	planCmd.Flags().StringVar(&plan.WalletRef, "wallet", "", "Wallet identifier or name")
	planCmd.Flags().StringVar(&plan.FromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&plan.Recipient, "recipient", "", "Recipient address (defaults to the resolved sender address)")
	planCmd.Flags().StringVar(&plan.OnBehalfOf, "on-behalf-of", "", "Position owner address (defaults to the resolved sender address)")
	planCmd.Flags().Int64Var(&plan.InterestRateMode, "interest-rate-mode", 2, "Aave borrow/repay mode (1=stable,2=variable)")
	planCmd.Flags().BoolVar(&plan.Simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.RPCURL, "rpc-url", "", "RPC URL override for the selected chain")
	planCmd.Flags().StringVar(&plan.PoolAddress, "pool-address", "", "Aave pool address override")
	planCmd.Flags().StringVar(&plan.PoolAddressProvider, "pool-address-provider", "", "Aave pool address provider override")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("asset")
	_ = planCmd.MarkFlagRequired("provider")
	configureStructuredInput[lendArgs](planCmd, structuredInputOptions{
		Mutation:         true,
		InputConstraints: standardExecutionIdentityInputConstraints(),
	})

	var submit executionSubmitArgs
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing lend action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			return s.runSubmitAction(cmd, submit, expectedIntent, "action intent does not match lend verb")
		},
	}
	registerSubmitFlags(submitCmd, &submit, "lend")
	annotateStructuredSubmitCommand(submitCmd, standardSubmitSchema{})

	statusCmd := s.newStatusCommand("lend", expectedIntent, "action intent does not match lend verb")

	root.AddCommand(planCmd)
	root.AddCommand(submitCmd)
	root.AddCommand(statusCmd)
	return root
}
