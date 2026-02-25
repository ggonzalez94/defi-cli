package app

import (
	"context"
	"strings"
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
		protocol            string
		chainArg            string
		assetArg            string
		marketID            string
		amountBase          string
		amountDecimal       string
		fromAddress         string
		recipient           string
		onBehalfOf          string
		interestRateMode    int64
		simulate            bool
		rpcURL              string
		poolAddress         string
		poolAddressProvider string
	}
	buildAction := func(ctx context.Context, args lendArgs) (execution.Action, error) {
		chain, asset, err := parseChainAsset(args.chainArg, args.assetArg)
		if err != nil {
			return execution.Action{}, err
		}
		decimals := asset.Decimals
		if decimals <= 0 {
			decimals = 18
		}
		base, _, err := id.NormalizeAmount(args.amountBase, args.amountDecimal, decimals)
		if err != nil {
			return execution.Action{}, err
		}
		return s.actionBuilderRegistry().BuildLendAction(ctx, actionbuilder.LendRequest{
			Protocol:            args.protocol,
			Verb:                verb,
			Chain:               chain,
			Asset:               asset,
			MarketID:            args.marketID,
			AmountBaseUnits:     base,
			Sender:              args.fromAddress,
			Recipient:           args.recipient,
			OnBehalfOf:          args.onBehalfOf,
			InterestRateMode:    args.interestRateMode,
			Simulate:            args.simulate,
			RPCURL:              args.rpcURL,
			PoolAddress:         args.poolAddress,
			PoolAddressProvider: args.poolAddressProvider,
		})
	}

	var plan lendArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist a lend action plan",
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, err := buildAction(ctx, plan)
			providerName := normalizeLendingProtocol(plan.protocol)
			if providerName == "" {
				providerName = "lend"
			}
			statuses := []model.ProviderStatus{{Name: providerName, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
			if err != nil {
				s.captureCommandDiagnostics(nil, statuses, false)
				return err
			}
			if err := s.ensureActionStore(); err != nil {
				return err
			}
			if err := s.actionStore.Save(action); err != nil {
				return clierr.Wrap(clierr.CodeInternal, "persist planned action", err)
			}
			s.captureCommandDiagnostics(nil, statuses, false)
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), statuses, false)
		},
	}
	planCmd.Flags().StringVar(&plan.protocol, "protocol", "", "Lending protocol (aave|morpho)")
	planCmd.Flags().StringVar(&plan.chainArg, "chain", "", "Chain identifier")
	planCmd.Flags().StringVar(&plan.assetArg, "asset", "", "Asset symbol/address/CAIP-19")
	planCmd.Flags().StringVar(&plan.marketID, "market-id", "", "Morpho market unique key (required for --protocol morpho)")
	planCmd.Flags().StringVar(&plan.amountBase, "amount", "", "Amount in base units")
	planCmd.Flags().StringVar(&plan.amountDecimal, "amount-decimal", "", "Amount in decimal units")
	planCmd.Flags().StringVar(&plan.fromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&plan.recipient, "recipient", "", "Recipient address (defaults to --from-address)")
	planCmd.Flags().StringVar(&plan.onBehalfOf, "on-behalf-of", "", "Position owner address (defaults to --from-address)")
	planCmd.Flags().Int64Var(&plan.interestRateMode, "interest-rate-mode", 2, "Aave borrow/repay mode (1=stable,2=variable)")
	planCmd.Flags().BoolVar(&plan.simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.rpcURL, "rpc-url", "", "RPC URL override for the selected chain")
	planCmd.Flags().StringVar(&plan.poolAddress, "pool-address", "", "Aave pool address override")
	planCmd.Flags().StringVar(&plan.poolAddressProvider, "pool-address-provider", "", "Aave pool address provider override")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("asset")
	_ = planCmd.MarkFlagRequired("from-address")
	_ = planCmd.MarkFlagRequired("protocol")

	var run lendArgs
	var runSigner, runKeySource, runPrivateKey, runPollInterval, runStepTimeout string
	var runGasMultiplier float64
	var runMaxFeeGwei, runMaxPriorityFeeGwei string
	var runAllowMaxApproval, runUnsafeProviderTx bool
	runCmd := &cobra.Command{
		Use:   "run",
		Short: "Plan and execute a lend action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			txSigner, runSenderAddress, err := resolveRunSignerAndFromAddress(runSigner, runKeySource, runPrivateKey, run.fromAddress)
			if err != nil {
				return err
			}
			runArgs := run
			runArgs.fromAddress = runSenderAddress

			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, err := buildAction(ctx, runArgs)
			providerName := normalizeLendingProtocol(run.protocol)
			if providerName == "" {
				providerName = "lend"
			}
			statuses := []model.ProviderStatus{{Name: providerName, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
			if err != nil {
				s.captureCommandDiagnostics(nil, statuses, false)
				return err
			}
			if err := s.ensureActionStore(); err != nil {
				return err
			}
			if err := s.actionStore.Save(action); err != nil {
				return clierr.Wrap(clierr.CodeInternal, "persist planned action", err)
			}
			execOpts, err := parseExecuteOptions(
				run.simulate,
				runPollInterval,
				runStepTimeout,
				runGasMultiplier,
				runMaxFeeGwei,
				runMaxPriorityFeeGwei,
				runAllowMaxApproval,
				runUnsafeProviderTx,
			)
			if err != nil {
				s.captureCommandDiagnostics(nil, statuses, false)
				return err
			}
			if err := s.executeActionWithTimeout(&action, txSigner, execOpts); err != nil {
				s.captureCommandDiagnostics(nil, statuses, false)
				return err
			}
			s.captureCommandDiagnostics(nil, statuses, false)
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), statuses, false)
		},
	}
	runCmd.Flags().StringVar(&run.protocol, "protocol", "", "Lending protocol (aave|morpho)")
	runCmd.Flags().StringVar(&run.chainArg, "chain", "", "Chain identifier")
	runCmd.Flags().StringVar(&run.assetArg, "asset", "", "Asset symbol/address/CAIP-19")
	runCmd.Flags().StringVar(&run.marketID, "market-id", "", "Morpho market unique key (required for --protocol morpho)")
	runCmd.Flags().StringVar(&run.amountBase, "amount", "", "Amount in base units")
	runCmd.Flags().StringVar(&run.amountDecimal, "amount-decimal", "", "Amount in decimal units")
	runCmd.Flags().StringVar(&run.fromAddress, "from-address", "", "Sender EOA address (defaults to signer address)")
	runCmd.Flags().StringVar(&run.recipient, "recipient", "", "Recipient address (defaults to --from-address)")
	runCmd.Flags().StringVar(&run.onBehalfOf, "on-behalf-of", "", "Position owner address (defaults to --from-address)")
	runCmd.Flags().Int64Var(&run.interestRateMode, "interest-rate-mode", 2, "Aave borrow/repay mode (1=stable,2=variable)")
	runCmd.Flags().BoolVar(&run.simulate, "simulate", true, "Run preflight simulation before submission")
	runCmd.Flags().StringVar(&run.rpcURL, "rpc-url", "", "RPC URL override for the selected chain")
	runCmd.Flags().StringVar(&run.poolAddress, "pool-address", "", "Aave pool address override")
	runCmd.Flags().StringVar(&run.poolAddressProvider, "pool-address-provider", "", "Aave pool address provider override")
	runCmd.Flags().StringVar(&runSigner, "signer", "local", "Signer backend (local)")
	runCmd.Flags().StringVar(&runKeySource, "key-source", execsigner.KeySourceAuto, "Key source (auto|env|file|keystore)")
	runCmd.Flags().StringVar(&runPrivateKey, "private-key", "", "Private key hex override for local signer (less safe)")
	runCmd.Flags().StringVar(&runPollInterval, "poll-interval", "2s", "Receipt polling interval")
	runCmd.Flags().StringVar(&runStepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	runCmd.Flags().Float64Var(&runGasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	runCmd.Flags().StringVar(&runMaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	runCmd.Flags().StringVar(&runMaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")
	runCmd.Flags().BoolVar(&runAllowMaxApproval, "allow-max-approval", false, "Allow approval amounts greater than planned input amount")
	runCmd.Flags().BoolVar(&runUnsafeProviderTx, "unsafe-provider-tx", false, "Bypass provider transaction guardrails for bridge/aggregator payloads")
	_ = runCmd.MarkFlagRequired("chain")
	_ = runCmd.MarkFlagRequired("asset")
	_ = runCmd.MarkFlagRequired("protocol")

	var submitActionID string
	var submitSimulate bool
	var submitSigner, submitKeySource, submitPrivateKey, submitFromAddress, submitPollInterval, submitStepTimeout string
	var submitGasMultiplier float64
	var submitMaxFeeGwei, submitMaxPriorityFeeGwei string
	var submitAllowMaxApproval, submitUnsafeProviderTx bool
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing lend action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			actionID, err := resolveActionID(submitActionID)
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
			txSigner, err := newExecutionSigner(submitSigner, submitKeySource, submitPrivateKey)
			if err != nil {
				return err
			}
			if strings.TrimSpace(submitFromAddress) != "" && !strings.EqualFold(strings.TrimSpace(submitFromAddress), txSigner.Address().Hex()) {
				return clierr.New(clierr.CodeSigner, "signer address does not match --from-address")
			}
			if strings.TrimSpace(action.FromAddress) != "" && !strings.EqualFold(strings.TrimSpace(action.FromAddress), txSigner.Address().Hex()) {
				return clierr.New(clierr.CodeSigner, "signer address does not match planned action sender")
			}
			execOpts, err := parseExecuteOptions(
				submitSimulate,
				submitPollInterval,
				submitStepTimeout,
				submitGasMultiplier,
				submitMaxFeeGwei,
				submitMaxPriorityFeeGwei,
				submitAllowMaxApproval,
				submitUnsafeProviderTx,
			)
			if err != nil {
				return err
			}
			if err := s.executeActionWithTimeout(&action, txSigner, execOpts); err != nil {
				return err
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), nil, false)
		},
	}
	submitCmd.Flags().StringVar(&submitActionID, "action-id", "", "Action identifier")
	submitCmd.Flags().BoolVar(&submitSimulate, "simulate", true, "Run preflight simulation before submission")
	submitCmd.Flags().StringVar(&submitSigner, "signer", "local", "Signer backend (local)")
	submitCmd.Flags().StringVar(&submitKeySource, "key-source", execsigner.KeySourceAuto, "Key source (auto|env|file|keystore)")
	submitCmd.Flags().StringVar(&submitPrivateKey, "private-key", "", "Private key hex override for local signer (less safe)")
	submitCmd.Flags().StringVar(&submitFromAddress, "from-address", "", "Expected sender EOA address")
	submitCmd.Flags().StringVar(&submitPollInterval, "poll-interval", "2s", "Receipt polling interval")
	submitCmd.Flags().StringVar(&submitStepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	submitCmd.Flags().Float64Var(&submitGasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	submitCmd.Flags().StringVar(&submitMaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	submitCmd.Flags().StringVar(&submitMaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")
	submitCmd.Flags().BoolVar(&submitAllowMaxApproval, "allow-max-approval", false, "Allow approval amounts greater than planned input amount")
	submitCmd.Flags().BoolVar(&submitUnsafeProviderTx, "unsafe-provider-tx", false, "Bypass provider transaction guardrails for bridge/aggregator payloads")

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
	statusCmd.Flags().StringVar(&statusActionID, "action-id", "", "Action identifier")

	root.AddCommand(planCmd)
	root.AddCommand(runCmd)
	root.AddCommand(submitCmd)
	root.AddCommand(statusCmd)
	return root
}
