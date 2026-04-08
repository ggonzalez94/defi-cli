package app

import (
	"context"

	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/execution/actionbuilder"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/spf13/cobra"
)

func (s *runtimeState) newTransferCommand() *cobra.Command {
	root := &cobra.Command{Use: "transfer", Short: "ERC-20 transfer execution commands"}

	type transferArgs struct {
		ChainArg      string `json:"chain" flag:"chain" required:"true" format:"chain"`
		AssetArg      string `json:"asset" flag:"asset" required:"true" format:"asset"`
		AmountBase    string `json:"amount" flag:"amount" format:"base-units"`
		AmountDecimal string `json:"amount_decimal" flag:"amount-decimal" format:"decimal-amount"`
		WalletRef     string `json:"wallet" flag:"wallet" format:"identifier"`
		FromAddress   string `json:"from_address" flag:"from-address" format:"evm-address"`
		Recipient     string `json:"recipient" flag:"recipient" required:"true" format:"evm-address"`
		Simulate      bool   `json:"simulate" flag:"simulate"`
		RPCURL        string `json:"rpc_url" flag:"rpc-url" format:"url"`
	}
	type transferSubmitArgs struct {
		ActionID           string  `json:"action_id" flag:"action-id" required:"true" format:"action-id"`
		Simulate           bool    `json:"simulate" flag:"simulate"`
		Signer             string  `json:"signer" flag:"signer" enum:"local,tempo"`
		KeySource          string  `json:"key_source" flag:"key-source" enum:"auto,env,file,keystore"`
		PrivateKey         string  `json:"private_key" flag:"private-key" format:"hex"`
		FromAddress        string  `json:"from_address" flag:"from-address" format:"evm-address"`
		PollInterval       string  `json:"poll_interval" flag:"poll-interval" format:"duration"`
		StepTimeout        string  `json:"step_timeout" flag:"step-timeout" format:"duration"`
		GasMultiplier      float64 `json:"gas_multiplier" flag:"gas-multiplier"`
		MaxFeeGwei         string  `json:"max_fee_gwei" flag:"max-fee-gwei"`
		MaxPriorityFeeGwei string  `json:"max_priority_fee_gwei" flag:"max-priority-fee-gwei"`
		FeeToken           string  `json:"fee_token" flag:"fee-token" format:"evm-address"`
	}
	buildAction := func(args transferArgs) (execution.Action, error) {
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
		return s.actionBuilderRegistry().BuildTransferAction(actionbuilder.TransferRequest{
			Chain:           chain,
			Asset:           asset,
			AmountBaseUnits: base,
			Sender:          args.FromAddress,
			Recipient:       args.Recipient,
			Simulate:        args.Simulate,
			RPCURL:          args.RPCURL,
		})
	}

	var plan transferArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist an ERC-20 transfer action plan",
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
	planCmd.Flags().StringVar(&plan.AmountBase, "amount", "", "Amount in base units")
	planCmd.Flags().StringVar(&plan.AmountDecimal, "amount-decimal", "", "Amount in decimal units")
	planCmd.Flags().StringVar(&plan.WalletRef, "wallet", "", "Wallet identifier or name")
	planCmd.Flags().StringVar(&plan.FromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&plan.Recipient, "recipient", "", "Recipient EOA address")
	planCmd.Flags().BoolVar(&plan.Simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.RPCURL, "rpc-url", "", "RPC URL override for the selected chain")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("asset")
	_ = planCmd.MarkFlagRequired("recipient")
	configureStructuredInput[transferArgs](planCmd, structuredInputOptions{
		Mutation:         true,
		InputConstraints: standardExecutionIdentityInputConstraints(),
	})

	var submit executionSubmitArgs
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing ERC-20 transfer action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			return s.runSubmitAction(cmd, submit, "transfer", "action is not a transfer intent")
		},
	}
	registerSubmitFlags(submitCmd, &submit, "transfer")
	annotateStructuredSubmitCommand(submitCmd, transferSubmitArgs{})

	statusCmd := s.newStatusCommand("transfer", "transfer", "action is not a transfer intent")

	root.AddCommand(planCmd)
	root.AddCommand(submitCmd)
	root.AddCommand(statusCmd)
	return root
}
