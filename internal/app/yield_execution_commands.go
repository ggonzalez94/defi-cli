package app

import (
	"context"

	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/execution/actionbuilder"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/spf13/cobra"
)

func (s *runtimeState) addYieldExecutionSubcommands(root *cobra.Command) {
	root.AddCommand(s.newYieldVerbExecutionCommand(actionbuilder.YieldVerbDeposit, "Deposit assets into a yield product"))
	root.AddCommand(s.newYieldVerbExecutionCommand(actionbuilder.YieldVerbWithdraw, "Withdraw assets from a yield product"))
}

func (s *runtimeState) newYieldVerbExecutionCommand(verb actionbuilder.YieldVerb, short string) *cobra.Command {
	root := &cobra.Command{
		Use:   string(verb),
		Short: short,
	}
	expectedIntent := "yield_" + string(verb)

	type yieldArgs struct {
		Provider            string `json:"provider" flag:"provider" required:"true" enum:"aave,morpho,moonwell"`
		ChainArg            string `json:"chain" flag:"chain" required:"true" format:"chain"`
		AssetArg            string `json:"asset" flag:"asset" required:"true" format:"asset"`
		VaultAddress        string `json:"vault_address" flag:"vault-address" format:"evm-address"`
		AmountBase          string `json:"amount" flag:"amount" format:"base-units"`
		AmountDecimal       string `json:"amount_decimal" flag:"amount-decimal" format:"decimal-amount"`
		WalletRef           string `json:"wallet" flag:"wallet" format:"identifier"`
		FromAddress         string `json:"from_address" flag:"from-address" format:"evm-address"`
		Recipient           string `json:"recipient" flag:"recipient" format:"evm-address"`
		OnBehalfOf          string `json:"on_behalf_of" flag:"on-behalf-of" format:"evm-address"`
		Simulate            bool   `json:"simulate" flag:"simulate"`
		RPCURL              string `json:"rpc_url" flag:"rpc-url" format:"url"`
		PoolAddress         string `json:"pool_address" flag:"pool-address" format:"evm-address"`
		PoolAddressProvider string `json:"pool_address_provider" flag:"pool-address-provider" format:"evm-address"`
	}
	buildAction := func(ctx context.Context, args yieldArgs) (execution.Action, error) {
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
		return s.actionBuilderRegistry().BuildYieldAction(ctx, actionbuilder.YieldRequest{
			Provider:            args.Provider,
			Verb:                verb,
			Chain:               chain,
			Asset:               asset,
			VaultAddress:        args.VaultAddress,
			AmountBaseUnits:     base,
			Sender:              args.FromAddress,
			Recipient:           args.Recipient,
			OnBehalfOf:          args.OnBehalfOf,
			Simulate:            args.Simulate,
			RPCURL:              args.RPCURL,
			PoolAddress:         args.PoolAddress,
			PoolAddressProvider: args.PoolAddressProvider,
		})
	}

	var plan yieldArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist a yield action plan",
		RunE: func(cmd *cobra.Command, _ []string) error {
			providerName := providers.NormalizeLendingProvider(plan.Provider)
			if providerName == "" {
				providerName = "yield"
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
	planCmd.Flags().StringVar(&plan.Provider, "provider", "", "Yield provider (aave|morpho|moonwell)")
	planCmd.Flags().StringVar(&plan.ChainArg, "chain", "", "Chain identifier")
	planCmd.Flags().StringVar(&plan.AssetArg, "asset", "", "Asset symbol/address/CAIP-19")
	planCmd.Flags().StringVar(&plan.VaultAddress, "vault-address", "", "Morpho vault address (required for --provider morpho)")
	planCmd.Flags().StringVar(&plan.AmountBase, "amount", "", "Amount in base units")
	planCmd.Flags().StringVar(&plan.AmountDecimal, "amount-decimal", "", "Amount in decimal units")
	planCmd.Flags().StringVar(&plan.WalletRef, "wallet", "", "Wallet identifier or name")
	planCmd.Flags().StringVar(&plan.FromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&plan.Recipient, "recipient", "", "Recipient address (defaults to the resolved sender address)")
	planCmd.Flags().StringVar(&plan.OnBehalfOf, "on-behalf-of", "", "Position owner address (defaults to the resolved sender address)")
	planCmd.Flags().BoolVar(&plan.Simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.RPCURL, "rpc-url", "", "RPC URL override for the selected chain")
	planCmd.Flags().StringVar(&plan.PoolAddress, "pool-address", "", "Aave pool address override")
	planCmd.Flags().StringVar(&plan.PoolAddressProvider, "pool-address-provider", "", "Aave pool address provider override")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("asset")
	_ = planCmd.MarkFlagRequired("provider")
	configureStructuredInput[yieldArgs](planCmd, structuredInputOptions{
		Mutation:         true,
		InputConstraints: standardExecutionIdentityInputConstraints(),
	})

	root.AddCommand(planCmd)
	s.addSubmitAndStatus(root, "yield", expectedIntent, "action intent does not match yield verb")
	return root
}
