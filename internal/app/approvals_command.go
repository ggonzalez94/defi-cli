package app

import (
	"context"

	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/execution/planner"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/spf13/cobra"
)

func (s *runtimeState) newApprovalsCommand() *cobra.Command {
	root := &cobra.Command{Use: "approvals", Short: "Approval execution commands"}

	type approvalArgs struct {
		ChainArg      string `json:"chain" flag:"chain" required:"true" format:"chain"`
		AssetArg      string `json:"asset" flag:"asset" required:"true" format:"asset"`
		Spender       string `json:"spender" flag:"spender" required:"true" format:"evm-address"`
		AmountBase    string `json:"amount" flag:"amount" format:"base-units"`
		AmountDecimal string `json:"amount_decimal" flag:"amount-decimal" format:"decimal-amount"`
		WalletRef     string `json:"wallet" flag:"wallet" format:"identifier"`
		FromAddress   string `json:"from_address" flag:"from-address" format:"evm-address"`
		Simulate      bool   `json:"simulate" flag:"simulate"`
		RPCURL        string `json:"rpc_url" flag:"rpc-url" format:"url"`
	}
	buildAction := func(args approvalArgs) (execution.Action, error) {
		chain, err := id.ParseChain(args.ChainArg)
		if err != nil {
			return execution.Action{}, err
		}
		asset, err := id.ParseAsset(args.AssetArg, chain)
		if err != nil {
			return execution.Action{}, err
		}
		base, _, err := normalizeAssetAmount(args.AmountBase, args.AmountDecimal, asset.Decimals)
		if err != nil {
			return execution.Action{}, err
		}
		return s.actionBuilderRegistry().BuildApprovalAction(planner.ApprovalRequest{
			Chain:           chain,
			Asset:           asset,
			AmountBaseUnits: base,
			Sender:          args.FromAddress,
			Spender:         args.Spender,
			Simulate:        args.Simulate,
			RPCURL:          args.RPCURL,
		})
	}

	var plan approvalArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist an approval action plan",
		RunE: func(cmd *cobra.Command, _ []string) error {
			return s.runPlanAction(cmd, planActionConfig{
				ProviderName: "native",
				WalletRef:    plan.WalletRef,
				FromAddress:  plan.FromAddress,
				ChainArg:     plan.ChainArg,
				BuildAction: func(_ context.Context, fromAddr string) (execution.Action, error) {
					p := plan
					p.FromAddress = fromAddr
					return buildAction(p)
				},
			})
		},
	}
	planCmd.Flags().StringVar(&plan.ChainArg, "chain", "", "Chain identifier")
	planCmd.Flags().StringVar(&plan.AssetArg, "asset", "", "Asset symbol/address/CAIP-19")
	planCmd.Flags().StringVar(&plan.Spender, "spender", "", "Spender address")
	planCmd.Flags().StringVar(&plan.AmountBase, "amount", "", "Amount in base units")
	planCmd.Flags().StringVar(&plan.AmountDecimal, "amount-decimal", "", "Amount in decimal units")
	planCmd.Flags().StringVar(&plan.WalletRef, "wallet", "", "Wallet identifier or name")
	planCmd.Flags().StringVar(&plan.FromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().BoolVar(&plan.Simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.RPCURL, "rpc-url", "", "RPC URL override for the selected chain")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("asset")
	_ = planCmd.MarkFlagRequired("spender")
	configureStructuredInput[approvalArgs](planCmd, structuredInputOptions{
		Mutation:         true,
		InputConstraints: standardExecutionIdentityInputConstraints(),
	})

	root.AddCommand(planCmd)
	s.addSubmitAndStatus(root, "approval", "approve", "action is not an approval intent")
	return root
}
