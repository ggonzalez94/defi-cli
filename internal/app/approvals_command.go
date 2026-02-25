package app

import (
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/execution/planner"
	execsigner "github.com/ggonzalez94/defi-cli/internal/execution/signer"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/spf13/cobra"
)

func (s *runtimeState) newApprovalsCommand() *cobra.Command {
	root := &cobra.Command{Use: "approvals", Short: "Approval execution commands"}

	type approvalArgs struct {
		chainArg      string
		assetArg      string
		spender       string
		amountBase    string
		amountDecimal string
		fromAddress   string
		simulate      bool
		rpcURL        string
	}
	buildAction := func(args approvalArgs) (execution.Action, error) {
		chain, err := id.ParseChain(args.chainArg)
		if err != nil {
			return execution.Action{}, err
		}
		asset, err := id.ParseAsset(args.assetArg, chain)
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
		return s.actionBuilderRegistry().BuildApprovalAction(planner.ApprovalRequest{
			Chain:           chain,
			Asset:           asset,
			AmountBaseUnits: base,
			Sender:          args.fromAddress,
			Spender:         args.spender,
			Simulate:        args.simulate,
			RPCURL:          args.rpcURL,
		})
	}

	var plan approvalArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist an approval action plan",
		RunE: func(cmd *cobra.Command, _ []string) error {
			start := time.Now()
			action, err := buildAction(plan)
			status := []model.ProviderStatus{{Name: "native", Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
			if err != nil {
				s.captureCommandDiagnostics(nil, status, false)
				return err
			}
			if err := s.ensureActionStore(); err != nil {
				return err
			}
			if err := s.actionStore.Save(action); err != nil {
				return clierr.Wrap(clierr.CodeInternal, "persist planned action", err)
			}
			s.captureCommandDiagnostics(nil, status, false)
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), status, false)
		},
	}
	planCmd.Flags().StringVar(&plan.chainArg, "chain", "", "Chain identifier")
	planCmd.Flags().StringVar(&plan.assetArg, "asset", "", "Asset symbol/address/CAIP-19")
	planCmd.Flags().StringVar(&plan.spender, "spender", "", "Spender address")
	planCmd.Flags().StringVar(&plan.amountBase, "amount", "", "Amount in base units")
	planCmd.Flags().StringVar(&plan.amountDecimal, "amount-decimal", "", "Amount in decimal units")
	planCmd.Flags().StringVar(&plan.fromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().BoolVar(&plan.simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.rpcURL, "rpc-url", "", "RPC URL override for the selected chain")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("asset")
	_ = planCmd.MarkFlagRequired("spender")
	_ = planCmd.MarkFlagRequired("from-address")

	var run approvalArgs
	var runSigner, runKeySource, runConfirmAddress, runPollInterval, runStepTimeout string
	var runGasMultiplier float64
	var runMaxFeeGwei, runMaxPriorityFeeGwei string
	runCmd := &cobra.Command{
		Use:   "run",
		Short: "Plan and execute an approval action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			start := time.Now()
			action, err := buildAction(run)
			status := []model.ProviderStatus{{Name: "native", Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
			if err != nil {
				s.captureCommandDiagnostics(nil, status, false)
				return err
			}
			if err := s.ensureActionStore(); err != nil {
				return err
			}
			if err := s.actionStore.Save(action); err != nil {
				return clierr.Wrap(clierr.CodeInternal, "persist planned action", err)
			}
			txSigner, err := newExecutionSigner(runSigner, runKeySource, runConfirmAddress)
			if err != nil {
				return err
			}
			if !strings.EqualFold(strings.TrimSpace(run.fromAddress), txSigner.Address().Hex()) {
				return clierr.New(clierr.CodeSigner, "signer address does not match --from-address")
			}
			execOpts, err := parseExecuteOptions(run.simulate, runPollInterval, runStepTimeout, runGasMultiplier, runMaxFeeGwei, runMaxPriorityFeeGwei)
			if err != nil {
				return err
			}
			if err := s.executeActionWithTimeout(&action, txSigner, execOpts); err != nil {
				return err
			}
			s.captureCommandDiagnostics(nil, status, false)
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), status, false)
		},
	}
	runCmd.Flags().StringVar(&run.chainArg, "chain", "", "Chain identifier")
	runCmd.Flags().StringVar(&run.assetArg, "asset", "", "Asset symbol/address/CAIP-19")
	runCmd.Flags().StringVar(&run.spender, "spender", "", "Spender address")
	runCmd.Flags().StringVar(&run.amountBase, "amount", "", "Amount in base units")
	runCmd.Flags().StringVar(&run.amountDecimal, "amount-decimal", "", "Amount in decimal units")
	runCmd.Flags().StringVar(&run.fromAddress, "from-address", "", "Sender EOA address")
	runCmd.Flags().BoolVar(&run.simulate, "simulate", true, "Run preflight simulation before submission")
	runCmd.Flags().StringVar(&run.rpcURL, "rpc-url", "", "RPC URL override for the selected chain")
	runCmd.Flags().StringVar(&runSigner, "signer", "local", "Signer backend (local)")
	runCmd.Flags().StringVar(&runKeySource, "key-source", execsigner.KeySourceAuto, "Key source (auto|env|file|keystore)")
	runCmd.Flags().StringVar(&runConfirmAddress, "confirm-address", "", "Require signer address to match this value")
	runCmd.Flags().StringVar(&runPollInterval, "poll-interval", "2s", "Receipt polling interval")
	runCmd.Flags().StringVar(&runStepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	runCmd.Flags().Float64Var(&runGasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	runCmd.Flags().StringVar(&runMaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	runCmd.Flags().StringVar(&runMaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")
	_ = runCmd.MarkFlagRequired("chain")
	_ = runCmd.MarkFlagRequired("asset")
	_ = runCmd.MarkFlagRequired("spender")
	_ = runCmd.MarkFlagRequired("from-address")

	var submitActionID string
	var submitSimulate bool
	var submitSigner, submitKeySource, submitConfirmAddress, submitPollInterval, submitStepTimeout string
	var submitGasMultiplier float64
	var submitMaxFeeGwei, submitMaxPriorityFeeGwei string
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing approval action",
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
			if action.IntentType != "approve" {
				return clierr.New(clierr.CodeUsage, "action is not an approval intent")
			}
			txSigner, err := newExecutionSigner(submitSigner, submitKeySource, submitConfirmAddress)
			if err != nil {
				return err
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
	submitCmd.Flags().StringVar(&submitConfirmAddress, "confirm-address", "", "Require signer address to match this value")
	submitCmd.Flags().StringVar(&submitPollInterval, "poll-interval", "2s", "Receipt polling interval")
	submitCmd.Flags().StringVar(&submitStepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	submitCmd.Flags().Float64Var(&submitGasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	submitCmd.Flags().StringVar(&submitMaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	submitCmd.Flags().StringVar(&submitMaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")

	var statusActionID string
	statusCmd := &cobra.Command{
		Use:   "status",
		Short: "Get approval action status",
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
