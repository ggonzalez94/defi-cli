package app

import (
	"context"
	"strings"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/execution/actionbuilder"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/spf13/cobra"
)

func (s *runtimeState) newRewardsCommand() *cobra.Command {
	root := &cobra.Command{Use: "rewards", Short: "Rewards claim and compound execution commands"}
	root.AddCommand(s.newRewardsClaimCommand())
	root.AddCommand(s.newRewardsCompoundCommand())
	return root
}

func (s *runtimeState) newRewardsClaimCommand() *cobra.Command {
	root := &cobra.Command{Use: "claim", Short: "Claim rewards"}
	const expectedIntent = "claim_rewards"

	type claimArgs struct {
		Provider            string   `json:"provider" flag:"provider" required:"true" enum:"aave"`
		ChainArg            string   `json:"chain" flag:"chain" required:"true" format:"chain"`
		WalletRef           string   `json:"wallet" flag:"wallet" format:"identifier"`
		FromAddress         string   `json:"from_address" flag:"from-address" format:"evm-address"`
		Recipient           string   `json:"recipient" flag:"recipient" format:"evm-address"`
		Assets              []string `json:"assets" flag:"assets" required:"true" format:"evm-address"`
		RewardToken         string   `json:"reward_token" flag:"reward-token" required:"true" format:"evm-address"`
		AmountBase          string   `json:"amount" flag:"amount" format:"base-units"`
		Simulate            bool     `json:"simulate" flag:"simulate"`
		RPCURL              string   `json:"rpc_url" flag:"rpc-url" format:"url"`
		ControllerAddress   string   `json:"controller_address" flag:"controller-address" format:"evm-address"`
		PoolAddressProvider string   `json:"pool_address_provider" flag:"pool-address-provider" format:"evm-address"`
	}
	type claimSubmitArgs struct {
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
		AllowMaxApproval   bool    `json:"allow_max_approval" flag:"allow-max-approval"`
		UnsafeProviderTx   bool    `json:"unsafe_provider_tx" flag:"unsafe-provider-tx"`
		FeeToken           string  `json:"fee_token" flag:"fee-token" format:"evm-address"`
	}
	buildAction := func(ctx context.Context, args claimArgs) (execution.Action, error) {
		chain, err := id.ParseChain(args.ChainArg)
		if err != nil {
			return execution.Action{}, err
		}
		assets := normalizeStringSlice(args.Assets)
		if len(assets) == 0 {
			return execution.Action{}, clierr.New(clierr.CodeUsage, "--assets is required")
		}
		amount := strings.TrimSpace(args.AmountBase)
		if amount == "" {
			amount = "max"
		}
		return s.actionBuilderRegistry().BuildRewardsClaimAction(ctx, actionbuilder.RewardsClaimRequest{
			Provider:            args.Provider,
			Chain:               chain,
			Sender:              args.FromAddress,
			Recipient:           args.Recipient,
			Assets:              assets,
			RewardToken:         args.RewardToken,
			AmountBaseUnits:     amount,
			Simulate:            args.Simulate,
			RPCURL:              args.RPCURL,
			ControllerAddress:   args.ControllerAddress,
			PoolAddressProvider: args.PoolAddressProvider,
		})
	}

	var plan claimArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist a rewards-claim action plan",
		RunE: func(cmd *cobra.Command, _ []string) error {
			providerName := providers.NormalizeLendingProvider(plan.Provider)
			if providerName == "" {
				providerName = strings.TrimSpace(plan.Provider)
			}
			if providerName == "" {
				providerName = "unknown"
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
	planCmd.Flags().StringVar(&plan.Provider, "provider", "", "Rewards provider (aave)")
	planCmd.Flags().StringVar(&plan.ChainArg, "chain", "", "Chain identifier")
	planCmd.Flags().StringVar(&plan.WalletRef, "wallet", "", "Wallet identifier or name")
	planCmd.Flags().StringVar(&plan.FromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&plan.Recipient, "recipient", "", "Recipient address (defaults to the resolved sender address)")
	planCmd.Flags().StringSliceVar(&plan.Assets, "assets", nil, "Comma-separated rewards source asset addresses")
	planCmd.Flags().StringVar(&plan.RewardToken, "reward-token", "", "Reward token address")
	planCmd.Flags().StringVar(&plan.AmountBase, "amount", "", "Claim amount in base units (defaults to max)")
	planCmd.Flags().BoolVar(&plan.Simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.RPCURL, "rpc-url", "", "RPC URL override for the selected chain")
	planCmd.Flags().StringVar(&plan.ControllerAddress, "controller-address", "", "Aave incentives controller address override")
	planCmd.Flags().StringVar(&plan.PoolAddressProvider, "pool-address-provider", "", "Aave pool address provider override")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("assets")
	_ = planCmd.MarkFlagRequired("reward-token")
	_ = planCmd.MarkFlagRequired("provider")
	configureStructuredInput[claimArgs](planCmd, structuredInputOptions{
		Mutation:         true,
		InputConstraints: standardExecutionIdentityInputConstraints(),
	})

	var submit executionSubmitArgs
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing rewards-claim action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			return s.runSubmitAction(cmd, submit, expectedIntent, "action is not a rewards claim intent")
		},
	}
	registerSubmitFlags(submitCmd, &submit, "rewards claim")
	annotateStructuredSubmitCommand(submitCmd, claimSubmitArgs{})

	statusCmd := s.newStatusCommand("rewards-claim", expectedIntent, "action is not a rewards claim intent")

	root.AddCommand(planCmd)
	root.AddCommand(submitCmd)
	root.AddCommand(statusCmd)
	return root
}

func (s *runtimeState) newRewardsCompoundCommand() *cobra.Command {
	root := &cobra.Command{Use: "compound", Short: "Compound rewards by claim + resupply"}
	const expectedIntent = "compound_rewards"

	type compoundArgs struct {
		Provider            string   `json:"provider" flag:"provider" required:"true" enum:"aave"`
		ChainArg            string   `json:"chain" flag:"chain" required:"true" format:"chain"`
		WalletRef           string   `json:"wallet" flag:"wallet" format:"identifier"`
		FromAddress         string   `json:"from_address" flag:"from-address" format:"evm-address"`
		Recipient           string   `json:"recipient" flag:"recipient" format:"evm-address"`
		OnBehalfOf          string   `json:"on_behalf_of" flag:"on-behalf-of" format:"evm-address"`
		Assets              []string `json:"assets" flag:"assets" required:"true" format:"evm-address"`
		RewardToken         string   `json:"reward_token" flag:"reward-token" required:"true" format:"evm-address"`
		AmountBase          string   `json:"amount" flag:"amount" required:"true" format:"base-units"`
		Simulate            bool     `json:"simulate" flag:"simulate"`
		RPCURL              string   `json:"rpc_url" flag:"rpc-url" format:"url"`
		ControllerAddress   string   `json:"controller_address" flag:"controller-address" format:"evm-address"`
		PoolAddress         string   `json:"pool_address" flag:"pool-address" format:"evm-address"`
		PoolAddressProvider string   `json:"pool_address_provider" flag:"pool-address-provider" format:"evm-address"`
	}
	type compoundSubmitArgs struct {
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
		AllowMaxApproval   bool    `json:"allow_max_approval" flag:"allow-max-approval"`
		UnsafeProviderTx   bool    `json:"unsafe_provider_tx" flag:"unsafe-provider-tx"`
		FeeToken           string  `json:"fee_token" flag:"fee-token" format:"evm-address"`
	}
	buildAction := func(ctx context.Context, args compoundArgs) (execution.Action, error) {
		chain, err := id.ParseChain(args.ChainArg)
		if err != nil {
			return execution.Action{}, err
		}
		assets := normalizeStringSlice(args.Assets)
		if len(assets) == 0 {
			return execution.Action{}, clierr.New(clierr.CodeUsage, "--assets is required")
		}
		amount := strings.TrimSpace(args.AmountBase)
		if amount == "" {
			return execution.Action{}, clierr.New(clierr.CodeUsage, "--amount is required")
		}
		return s.actionBuilderRegistry().BuildRewardsCompoundAction(ctx, actionbuilder.RewardsCompoundRequest{
			Provider:            args.Provider,
			Chain:               chain,
			Sender:              args.FromAddress,
			Recipient:           args.Recipient,
			OnBehalfOf:          args.OnBehalfOf,
			Assets:              assets,
			RewardToken:         args.RewardToken,
			AmountBaseUnits:     amount,
			Simulate:            args.Simulate,
			RPCURL:              args.RPCURL,
			ControllerAddress:   args.ControllerAddress,
			PoolAddress:         args.PoolAddress,
			PoolAddressProvider: args.PoolAddressProvider,
		})
	}

	var plan compoundArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist a rewards-compound action plan",
		RunE: func(cmd *cobra.Command, _ []string) error {
			providerName := providers.NormalizeLendingProvider(plan.Provider)
			if providerName == "" {
				providerName = strings.TrimSpace(plan.Provider)
			}
			if providerName == "" {
				providerName = "unknown"
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
	planCmd.Flags().StringVar(&plan.Provider, "provider", "", "Rewards provider (aave)")
	planCmd.Flags().StringVar(&plan.ChainArg, "chain", "", "Chain identifier")
	planCmd.Flags().StringVar(&plan.WalletRef, "wallet", "", "Wallet identifier or name")
	planCmd.Flags().StringVar(&plan.FromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&plan.Recipient, "recipient", "", "Recipient address (defaults to the resolved sender address)")
	planCmd.Flags().StringVar(&plan.OnBehalfOf, "on-behalf-of", "", "Aave onBehalfOf address for compounding supply")
	planCmd.Flags().StringSliceVar(&plan.Assets, "assets", nil, "Comma-separated rewards source asset addresses")
	planCmd.Flags().StringVar(&plan.RewardToken, "reward-token", "", "Reward token address")
	planCmd.Flags().StringVar(&plan.AmountBase, "amount", "", "Compound amount in base units")
	planCmd.Flags().BoolVar(&plan.Simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.RPCURL, "rpc-url", "", "RPC URL override for the selected chain")
	planCmd.Flags().StringVar(&plan.ControllerAddress, "controller-address", "", "Aave incentives controller address override")
	planCmd.Flags().StringVar(&plan.PoolAddress, "pool-address", "", "Aave pool address override")
	planCmd.Flags().StringVar(&plan.PoolAddressProvider, "pool-address-provider", "", "Aave pool address provider override")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("assets")
	_ = planCmd.MarkFlagRequired("reward-token")
	_ = planCmd.MarkFlagRequired("amount")
	_ = planCmd.MarkFlagRequired("provider")
	configureStructuredInput[compoundArgs](planCmd, structuredInputOptions{
		Mutation:         true,
		InputConstraints: standardExecutionIdentityInputConstraints(),
	})

	var submit executionSubmitArgs
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing rewards-compound action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			return s.runSubmitAction(cmd, submit, expectedIntent, "action is not a rewards compound intent")
		},
	}
	registerSubmitFlags(submitCmd, &submit, "rewards compound")
	annotateStructuredSubmitCommand(submitCmd, compoundSubmitArgs{})

	statusCmd := s.newStatusCommand("rewards-compound", expectedIntent, "action is not a rewards compound intent")

	root.AddCommand(planCmd)
	root.AddCommand(submitCmd)
	root.AddCommand(statusCmd)
	return root
}
