package app

import (
	"context"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/execution/actionbuilder"
	"github.com/ggonzalez94/defi-cli/internal/execution/planner"
	execsigner "github.com/ggonzalez94/defi-cli/internal/execution/signer"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
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
	type lendSubmitArgs struct {
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
			identity, err := resolveExecutionIdentity(plan.WalletRef, plan.FromAddress, plan.ChainArg)
			if err != nil {
				return err
			}
			resolvedPlan := plan
			resolvedPlan.FromAddress = identity.FromAddress
			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, err := buildAction(ctx, resolvedPlan)
			providerName := normalizeLendingProvider(plan.Provider)
			if providerName == "" {
				providerName = "lend"
			}
			statuses := []model.ProviderStatus{{Name: providerName, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
			if err != nil {
				s.captureCommandDiagnostics(nil, statuses, false)
				return err
			}
			applyExecutionIdentityToAction(&action, identity)
			if err := s.ensureActionStore(); err != nil {
				return err
			}
			if err := s.actionStore.Save(action); err != nil {
				return clierr.Wrap(clierr.CodeInternal, "persist planned action", err)
			}
			s.captureCommandDiagnostics(nil, statuses, false)
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, identity.Warnings, cacheMetaBypass(), statuses, false)
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

	var submit lendSubmitArgs
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing lend action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			actionID, err := resolveActionID(submit.ActionID)
			if err != nil {
				return err
			}
			if err := s.ensureActionStore(); err != nil {
				return err
			}
			action, err := s.actionStore.Get(actionID)
			if err != nil {
				return clierr.Wrap(clierr.CodeUsage, "load action", err)
			}
			if action.IntentType != expectedIntent {
				return clierr.New(clierr.CodeUsage, "action intent does not match lend verb")
			}
			if action.Status == execution.ActionStatusCompleted {
				return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, []string{"action already completed"}, cacheMetaBypass(), nil, false)
			}
			resolvedExec, err := resolveActionExecutionBackend(cmd, action, submitExecutionInputs{
				Signer:      submit.Signer,
				KeySource:   submit.KeySource,
				PrivateKey:  submit.PrivateKey,
				FromAddress: submit.FromAddress,
			})
			if err != nil {
				return err
			}
			if err := validateExecutionSender(action, submit.FromAddress, resolvedExec.sender); err != nil {
				return err
			}
			execOpts, err := parseExecuteOptions(
				submit.Simulate,
				submit.PollInterval,
				submit.StepTimeout,
				submit.GasMultiplier,
				submit.MaxFeeGwei,
				submit.MaxPriorityFeeGwei,
				submit.AllowMaxApproval,
				submit.UnsafeProviderTx,
				submit.FeeToken,
			)
			if err != nil {
				return err
			}
			if err := s.executeActionWithTimeout(&action, resolvedExec.txSigner, resolvedExec.evmBackend, execOpts); err != nil {
				return err
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), nil, false)
		},
	}
	submitCmd.Flags().StringVar(&submit.ActionID, "action-id", "", "Action identifier returned by lend plan")
	submitCmd.Flags().BoolVar(&submit.Simulate, "simulate", true, "Run preflight simulation before submission")
	submitCmd.Flags().StringVar(&submit.Signer, "signer", "local", "Signer backend (local|tempo)")
	submitCmd.Flags().StringVar(&submit.KeySource, "key-source", execsigner.KeySourceAuto, "Key source (auto|env|file|keystore)")
	submitCmd.Flags().StringVar(&submit.PrivateKey, "private-key", "", "Private key hex override for local signer (less safe)")
	submitCmd.Flags().StringVar(&submit.FromAddress, "from-address", "", "Expected sender EOA address")
	submitCmd.Flags().StringVar(&submit.PollInterval, "poll-interval", "2s", "Receipt polling interval")
	submitCmd.Flags().StringVar(&submit.StepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	submitCmd.Flags().Float64Var(&submit.GasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	submitCmd.Flags().StringVar(&submit.MaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	submitCmd.Flags().StringVar(&submit.MaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")
	submitCmd.Flags().BoolVar(&submit.AllowMaxApproval, "allow-max-approval", false, "Allow approval amounts greater than planned input amount")
	submitCmd.Flags().BoolVar(&submit.UnsafeProviderTx, "unsafe-provider-tx", false, "Bypass provider transaction guardrails for bridge/aggregator payloads")
	submitCmd.Flags().StringVar(&submit.FeeToken, "fee-token", "", "Fee token address for Tempo chains (defaults to chain USDC.e)")
	annotateStructuredSubmitCommand(submitCmd, lendSubmitArgs{})

	var statusActionID string
	statusCmd := &cobra.Command{
		Use:   "status",
		Short: "Get lend action status",
		RunE: func(cmd *cobra.Command, _ []string) error {
			actionID, err := resolveActionID(statusActionID)
			if err != nil {
				return err
			}
			if err := s.ensureActionStore(); err != nil {
				return err
			}
			action, err := s.actionStore.Get(actionID)
			if err != nil {
				return clierr.Wrap(clierr.CodeUsage, "load action", err)
			}
			if action.IntentType != expectedIntent {
				return clierr.New(clierr.CodeUsage, "action intent does not match lend verb")
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), nil, false)
		},
	}
	statusCmd.Flags().StringVar(&statusActionID, "action-id", "", "Action identifier returned by lend plan")
	annotateExecutionStatusCommand(statusCmd)

	root.AddCommand(planCmd)
	root.AddCommand(submitCmd)
	root.AddCommand(statusCmd)
	return root
}
