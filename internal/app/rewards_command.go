package app

import (
	"context"
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/execution/actionbuilder"
	execsigner "github.com/ggonzalez94/defi-cli/internal/execution/signer"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
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
		protocol            string
		chainArg            string
		fromAddress         string
		recipient           string
		assetsCSV           string
		rewardToken         string
		amountBase          string
		simulate            bool
		rpcURL              string
		controllerAddress   string
		poolAddressProvider string
	}
	buildAction := func(ctx context.Context, args claimArgs) (execution.Action, error) {
		chain, err := id.ParseChain(args.chainArg)
		if err != nil {
			return execution.Action{}, err
		}
		assets := splitCSV(args.assetsCSV)
		if len(assets) == 0 {
			return execution.Action{}, clierr.New(clierr.CodeUsage, "--assets is required")
		}
		amount := strings.TrimSpace(args.amountBase)
		if amount == "" {
			amount = "max"
		}
		return s.actionBuilderRegistry().BuildRewardsClaimAction(ctx, actionbuilder.RewardsClaimRequest{
			Protocol:            args.protocol,
			Chain:               chain,
			Sender:              args.fromAddress,
			Recipient:           args.recipient,
			Assets:              assets,
			RewardToken:         args.rewardToken,
			AmountBaseUnits:     amount,
			Simulate:            args.simulate,
			RPCURL:              args.rpcURL,
			ControllerAddress:   args.controllerAddress,
			PoolAddressProvider: args.poolAddressProvider,
		})
	}

	var plan claimArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist a rewards-claim action plan",
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, err := buildAction(ctx, plan)
			statuses := []model.ProviderStatus{{Name: "aave", Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
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
	planCmd.Flags().StringVar(&plan.protocol, "protocol", "", "Rewards protocol (aave)")
	planCmd.Flags().StringVar(&plan.chainArg, "chain", "", "Chain identifier")
	planCmd.Flags().StringVar(&plan.fromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&plan.recipient, "recipient", "", "Recipient address (defaults to --from-address)")
	planCmd.Flags().StringVar(&plan.assetsCSV, "assets", "", "Comma-separated rewards source asset addresses")
	planCmd.Flags().StringVar(&plan.rewardToken, "reward-token", "", "Reward token address")
	planCmd.Flags().StringVar(&plan.amountBase, "amount", "", "Claim amount in base units (defaults to max)")
	planCmd.Flags().BoolVar(&plan.simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.rpcURL, "rpc-url", "", "RPC URL override for the selected chain")
	planCmd.Flags().StringVar(&plan.controllerAddress, "controller-address", "", "Aave incentives controller address override")
	planCmd.Flags().StringVar(&plan.poolAddressProvider, "pool-address-provider", "", "Aave pool address provider override")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("from-address")
	_ = planCmd.MarkFlagRequired("assets")
	_ = planCmd.MarkFlagRequired("reward-token")
	_ = planCmd.MarkFlagRequired("protocol")

	var run claimArgs
	var runSigner, runKeySource, runPollInterval, runStepTimeout string
	var runGasMultiplier float64
	var runMaxFeeGwei, runMaxPriorityFeeGwei string
	runCmd := &cobra.Command{
		Use:   "run",
		Short: "Plan and execute a rewards-claim action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			txSigner, runSenderAddress, err := resolveRunSignerAndFromAddress(runSigner, runKeySource, run.fromAddress)
			if err != nil {
				return err
			}
			runArgs := run
			runArgs.fromAddress = runSenderAddress

			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, err := buildAction(ctx, runArgs)
			statuses := []model.ProviderStatus{{Name: "aave", Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
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
			execOpts, err := parseExecuteOptions(run.simulate, runPollInterval, runStepTimeout, runGasMultiplier, runMaxFeeGwei, runMaxPriorityFeeGwei)
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
	runCmd.Flags().StringVar(&run.protocol, "protocol", "", "Rewards protocol (aave)")
	runCmd.Flags().StringVar(&run.chainArg, "chain", "", "Chain identifier")
	runCmd.Flags().StringVar(&run.fromAddress, "from-address", "", "Sender EOA address (defaults to signer address)")
	runCmd.Flags().StringVar(&run.recipient, "recipient", "", "Recipient address (defaults to --from-address)")
	runCmd.Flags().StringVar(&run.assetsCSV, "assets", "", "Comma-separated rewards source asset addresses")
	runCmd.Flags().StringVar(&run.rewardToken, "reward-token", "", "Reward token address")
	runCmd.Flags().StringVar(&run.amountBase, "amount", "", "Claim amount in base units (defaults to max)")
	runCmd.Flags().BoolVar(&run.simulate, "simulate", true, "Run preflight simulation before submission")
	runCmd.Flags().StringVar(&run.rpcURL, "rpc-url", "", "RPC URL override for the selected chain")
	runCmd.Flags().StringVar(&run.controllerAddress, "controller-address", "", "Aave incentives controller address override")
	runCmd.Flags().StringVar(&run.poolAddressProvider, "pool-address-provider", "", "Aave pool address provider override")
	runCmd.Flags().StringVar(&runSigner, "signer", "local", "Signer backend (local)")
	runCmd.Flags().StringVar(&runKeySource, "key-source", execsigner.KeySourceAuto, "Key source (auto|env|file|keystore)")
	runCmd.Flags().StringVar(&runPollInterval, "poll-interval", "2s", "Receipt polling interval")
	runCmd.Flags().StringVar(&runStepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	runCmd.Flags().Float64Var(&runGasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	runCmd.Flags().StringVar(&runMaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	runCmd.Flags().StringVar(&runMaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")
	_ = runCmd.MarkFlagRequired("chain")
	_ = runCmd.MarkFlagRequired("assets")
	_ = runCmd.MarkFlagRequired("reward-token")
	_ = runCmd.MarkFlagRequired("protocol")

	var submitActionID string
	var submitSimulate bool
	var submitSigner, submitKeySource, submitFromAddress, submitPollInterval, submitStepTimeout string
	var submitGasMultiplier float64
	var submitMaxFeeGwei, submitMaxPriorityFeeGwei string
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing rewards-claim action",
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
				return clierr.New(clierr.CodeUsage, "action is not a rewards claim intent")
			}
			if action.Status == execution.ActionStatusCompleted {
				return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, []string{"action already completed"}, cacheMetaBypass(), nil, false)
			}
			txSigner, err := newExecutionSigner(submitSigner, submitKeySource)
			if err != nil {
				return err
			}
			if strings.TrimSpace(submitFromAddress) != "" && !strings.EqualFold(strings.TrimSpace(submitFromAddress), txSigner.Address().Hex()) {
				return clierr.New(clierr.CodeSigner, "signer address does not match --from-address")
			}
			if strings.TrimSpace(action.FromAddress) != "" && !strings.EqualFold(strings.TrimSpace(action.FromAddress), txSigner.Address().Hex()) {
				return clierr.New(clierr.CodeSigner, "signer address does not match planned action sender")
			}
			execOpts, err := parseExecuteOptions(submitSimulate, submitPollInterval, submitStepTimeout, submitGasMultiplier, submitMaxFeeGwei, submitMaxPriorityFeeGwei)
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
	submitCmd.Flags().StringVar(&submitFromAddress, "from-address", "", "Expected sender EOA address")
	submitCmd.Flags().StringVar(&submitPollInterval, "poll-interval", "2s", "Receipt polling interval")
	submitCmd.Flags().StringVar(&submitStepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	submitCmd.Flags().Float64Var(&submitGasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	submitCmd.Flags().StringVar(&submitMaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	submitCmd.Flags().StringVar(&submitMaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")

	var statusActionID string
	statusCmd := &cobra.Command{
		Use:   "status",
		Short: "Get rewards-claim action status",
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
				return clierr.New(clierr.CodeUsage, "action is not a rewards claim intent")
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

func (s *runtimeState) newRewardsCompoundCommand() *cobra.Command {
	root := &cobra.Command{Use: "compound", Short: "Compound rewards by claim + resupply"}
	const expectedIntent = "compound_rewards"

	type compoundArgs struct {
		protocol            string
		chainArg            string
		fromAddress         string
		recipient           string
		onBehalfOf          string
		assetsCSV           string
		rewardToken         string
		amountBase          string
		simulate            bool
		rpcURL              string
		controllerAddress   string
		poolAddress         string
		poolAddressProvider string
	}
	buildAction := func(ctx context.Context, args compoundArgs) (execution.Action, error) {
		chain, err := id.ParseChain(args.chainArg)
		if err != nil {
			return execution.Action{}, err
		}
		assets := splitCSV(args.assetsCSV)
		if len(assets) == 0 {
			return execution.Action{}, clierr.New(clierr.CodeUsage, "--assets is required")
		}
		amount := strings.TrimSpace(args.amountBase)
		if amount == "" {
			return execution.Action{}, clierr.New(clierr.CodeUsage, "--amount is required")
		}
		return s.actionBuilderRegistry().BuildRewardsCompoundAction(ctx, actionbuilder.RewardsCompoundRequest{
			Protocol:            args.protocol,
			Chain:               chain,
			Sender:              args.fromAddress,
			Recipient:           args.recipient,
			OnBehalfOf:          args.onBehalfOf,
			Assets:              assets,
			RewardToken:         args.rewardToken,
			AmountBaseUnits:     amount,
			Simulate:            args.simulate,
			RPCURL:              args.rpcURL,
			ControllerAddress:   args.controllerAddress,
			PoolAddress:         args.poolAddress,
			PoolAddressProvider: args.poolAddressProvider,
		})
	}

	var plan compoundArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist a rewards-compound action plan",
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, err := buildAction(ctx, plan)
			statuses := []model.ProviderStatus{{Name: "aave", Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
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
	planCmd.Flags().StringVar(&plan.protocol, "protocol", "", "Rewards protocol (aave)")
	planCmd.Flags().StringVar(&plan.chainArg, "chain", "", "Chain identifier")
	planCmd.Flags().StringVar(&plan.fromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&plan.recipient, "recipient", "", "Recipient address (defaults to --from-address)")
	planCmd.Flags().StringVar(&plan.onBehalfOf, "on-behalf-of", "", "Aave onBehalfOf address for compounding supply")
	planCmd.Flags().StringVar(&plan.assetsCSV, "assets", "", "Comma-separated rewards source asset addresses")
	planCmd.Flags().StringVar(&plan.rewardToken, "reward-token", "", "Reward token address")
	planCmd.Flags().StringVar(&plan.amountBase, "amount", "", "Compound amount in base units")
	planCmd.Flags().BoolVar(&plan.simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.rpcURL, "rpc-url", "", "RPC URL override for the selected chain")
	planCmd.Flags().StringVar(&plan.controllerAddress, "controller-address", "", "Aave incentives controller address override")
	planCmd.Flags().StringVar(&plan.poolAddress, "pool-address", "", "Aave pool address override")
	planCmd.Flags().StringVar(&plan.poolAddressProvider, "pool-address-provider", "", "Aave pool address provider override")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("from-address")
	_ = planCmd.MarkFlagRequired("assets")
	_ = planCmd.MarkFlagRequired("reward-token")
	_ = planCmd.MarkFlagRequired("amount")
	_ = planCmd.MarkFlagRequired("protocol")

	var run compoundArgs
	var runSigner, runKeySource, runPollInterval, runStepTimeout string
	var runGasMultiplier float64
	var runMaxFeeGwei, runMaxPriorityFeeGwei string
	runCmd := &cobra.Command{
		Use:   "run",
		Short: "Plan and execute a rewards-compound action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			txSigner, runSenderAddress, err := resolveRunSignerAndFromAddress(runSigner, runKeySource, run.fromAddress)
			if err != nil {
				return err
			}
			runArgs := run
			runArgs.fromAddress = runSenderAddress

			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, err := buildAction(ctx, runArgs)
			statuses := []model.ProviderStatus{{Name: "aave", Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
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
			execOpts, err := parseExecuteOptions(run.simulate, runPollInterval, runStepTimeout, runGasMultiplier, runMaxFeeGwei, runMaxPriorityFeeGwei)
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
	runCmd.Flags().StringVar(&run.protocol, "protocol", "", "Rewards protocol (aave)")
	runCmd.Flags().StringVar(&run.chainArg, "chain", "", "Chain identifier")
	runCmd.Flags().StringVar(&run.fromAddress, "from-address", "", "Sender EOA address (defaults to signer address)")
	runCmd.Flags().StringVar(&run.recipient, "recipient", "", "Recipient address (defaults to --from-address)")
	runCmd.Flags().StringVar(&run.onBehalfOf, "on-behalf-of", "", "Aave onBehalfOf address for compounding supply")
	runCmd.Flags().StringVar(&run.assetsCSV, "assets", "", "Comma-separated rewards source asset addresses")
	runCmd.Flags().StringVar(&run.rewardToken, "reward-token", "", "Reward token address")
	runCmd.Flags().StringVar(&run.amountBase, "amount", "", "Compound amount in base units")
	runCmd.Flags().BoolVar(&run.simulate, "simulate", true, "Run preflight simulation before submission")
	runCmd.Flags().StringVar(&run.rpcURL, "rpc-url", "", "RPC URL override for the selected chain")
	runCmd.Flags().StringVar(&run.controllerAddress, "controller-address", "", "Aave incentives controller address override")
	runCmd.Flags().StringVar(&run.poolAddress, "pool-address", "", "Aave pool address override")
	runCmd.Flags().StringVar(&run.poolAddressProvider, "pool-address-provider", "", "Aave pool address provider override")
	runCmd.Flags().StringVar(&runSigner, "signer", "local", "Signer backend (local)")
	runCmd.Flags().StringVar(&runKeySource, "key-source", execsigner.KeySourceAuto, "Key source (auto|env|file|keystore)")
	runCmd.Flags().StringVar(&runPollInterval, "poll-interval", "2s", "Receipt polling interval")
	runCmd.Flags().StringVar(&runStepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	runCmd.Flags().Float64Var(&runGasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	runCmd.Flags().StringVar(&runMaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	runCmd.Flags().StringVar(&runMaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")
	_ = runCmd.MarkFlagRequired("chain")
	_ = runCmd.MarkFlagRequired("assets")
	_ = runCmd.MarkFlagRequired("reward-token")
	_ = runCmd.MarkFlagRequired("amount")
	_ = runCmd.MarkFlagRequired("protocol")

	var submitActionID string
	var submitSimulate bool
	var submitSigner, submitKeySource, submitFromAddress, submitPollInterval, submitStepTimeout string
	var submitGasMultiplier float64
	var submitMaxFeeGwei, submitMaxPriorityFeeGwei string
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing rewards-compound action",
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
				return clierr.New(clierr.CodeUsage, "action is not a rewards compound intent")
			}
			if action.Status == execution.ActionStatusCompleted {
				return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, []string{"action already completed"}, cacheMetaBypass(), nil, false)
			}
			txSigner, err := newExecutionSigner(submitSigner, submitKeySource)
			if err != nil {
				return err
			}
			if strings.TrimSpace(submitFromAddress) != "" && !strings.EqualFold(strings.TrimSpace(submitFromAddress), txSigner.Address().Hex()) {
				return clierr.New(clierr.CodeSigner, "signer address does not match --from-address")
			}
			if strings.TrimSpace(action.FromAddress) != "" && !strings.EqualFold(strings.TrimSpace(action.FromAddress), txSigner.Address().Hex()) {
				return clierr.New(clierr.CodeSigner, "signer address does not match planned action sender")
			}
			execOpts, err := parseExecuteOptions(submitSimulate, submitPollInterval, submitStepTimeout, submitGasMultiplier, submitMaxFeeGwei, submitMaxPriorityFeeGwei)
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
	submitCmd.Flags().StringVar(&submitFromAddress, "from-address", "", "Expected sender EOA address")
	submitCmd.Flags().StringVar(&submitPollInterval, "poll-interval", "2s", "Receipt polling interval")
	submitCmd.Flags().StringVar(&submitStepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	submitCmd.Flags().Float64Var(&submitGasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	submitCmd.Flags().StringVar(&submitMaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	submitCmd.Flags().StringVar(&submitMaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")

	var statusActionID string
	statusCmd := &cobra.Command{
		Use:   "status",
		Short: "Get rewards-compound action status",
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
				return clierr.New(clierr.CodeUsage, "action is not a rewards compound intent")
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
