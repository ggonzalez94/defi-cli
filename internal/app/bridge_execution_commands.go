package app

import (
	"context"
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	execsigner "github.com/ggonzalez94/defi-cli/internal/execution/signer"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/spf13/cobra"
)

func (s *runtimeState) addBridgeExecutionSubcommands(root *cobra.Command) {
	buildRequest := func(fromArg, toArg, assetArg, toAssetArg, amountBase, amountDecimal, fromAmountForGas string) (providers.BridgeQuoteRequest, error) {
		fromChain, err := id.ParseChain(fromArg)
		if err != nil {
			return providers.BridgeQuoteRequest{}, err
		}
		toChain, err := id.ParseChain(toArg)
		if err != nil {
			return providers.BridgeQuoteRequest{}, err
		}
		fromAsset, err := id.ParseAsset(assetArg, fromChain)
		if err != nil {
			return providers.BridgeQuoteRequest{}, err
		}
		toAssetInput := strings.TrimSpace(toAssetArg)
		if toAssetInput == "" {
			if fromAsset.Symbol == "" {
				return providers.BridgeQuoteRequest{}, clierr.New(clierr.CodeUsage, "destination asset cannot be inferred, provide --to-asset")
			}
			toAssetInput = fromAsset.Symbol
		}
		toAsset, err := id.ParseAsset(toAssetInput, toChain)
		if err != nil {
			return providers.BridgeQuoteRequest{}, clierr.Wrap(clierr.CodeUsage, "resolve destination asset", err)
		}
		decimals := fromAsset.Decimals
		if decimals <= 0 {
			decimals = 18
		}
		base, decimal, err := id.NormalizeAmount(amountBase, amountDecimal, decimals)
		if err != nil {
			return providers.BridgeQuoteRequest{}, err
		}
		return providers.BridgeQuoteRequest{
			FromChain:        fromChain,
			ToChain:          toChain,
			FromAsset:        fromAsset,
			ToAsset:          toAsset,
			AmountBaseUnits:  base,
			AmountDecimal:    decimal,
			FromAmountForGas: strings.TrimSpace(fromAmountForGas),
		}, nil
	}

	var planProviderArg, planFromArg, planToArg, planAssetArg, planToAssetArg string
	var planAmountBase, planAmountDecimal, planFromAddress, planRecipient, planFromAmountForGas string
	var planSlippageBps int64
	var planSimulate bool
	var planRPCURL string
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist a bridge action plan",
		RunE: func(cmd *cobra.Command, _ []string) error {
			providerName := strings.ToLower(strings.TrimSpace(planProviderArg))
			if providerName == "" {
				return clierr.New(clierr.CodeUsage, "--provider is required")
			}
			reqStruct, err := buildRequest(planFromArg, planToArg, planAssetArg, planToAssetArg, planAmountBase, planAmountDecimal, planFromAmountForGas)
			if err != nil {
				return err
			}
			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, providerInfoName, err := s.actionBuilderRegistry().BuildBridgeAction(ctx, providerName, reqStruct, providers.BridgeExecutionOptions{
				Sender:           planFromAddress,
				Recipient:        planRecipient,
				SlippageBps:      planSlippageBps,
				Simulate:         planSimulate,
				RPCURL:           planRPCURL,
				FromAmountForGas: planFromAmountForGas,
			})
			if strings.TrimSpace(providerInfoName) == "" {
				providerInfoName = providerName
			}
			statuses := []model.ProviderStatus{{Name: providerInfoName, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
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
	planCmd.Flags().StringVar(&planProviderArg, "provider", "", "Bridge provider (across|lifi)")
	planCmd.Flags().StringVar(&planFromArg, "from", "", "Source chain")
	planCmd.Flags().StringVar(&planToArg, "to", "", "Destination chain")
	planCmd.Flags().StringVar(&planAssetArg, "asset", "", "Asset on source chain")
	planCmd.Flags().StringVar(&planToAssetArg, "to-asset", "", "Destination asset override")
	planCmd.Flags().StringVar(&planAmountBase, "amount", "", "Amount in base units")
	planCmd.Flags().StringVar(&planAmountDecimal, "amount-decimal", "", "Amount in decimal units")
	planCmd.Flags().StringVar(&planFromAmountForGas, "from-amount-for-gas", "", "Optional amount in source token base units to reserve for destination native gas (LiFi)")
	planCmd.Flags().StringVar(&planFromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&planRecipient, "recipient", "", "Recipient address (defaults to --from-address)")
	planCmd.Flags().Int64Var(&planSlippageBps, "slippage-bps", 50, "Max slippage in basis points")
	planCmd.Flags().BoolVar(&planSimulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&planRPCURL, "rpc-url", "", "RPC URL override for source chain")
	_ = planCmd.MarkFlagRequired("from")
	_ = planCmd.MarkFlagRequired("to")
	_ = planCmd.MarkFlagRequired("asset")
	_ = planCmd.MarkFlagRequired("from-address")
	_ = planCmd.MarkFlagRequired("provider")

	var runProviderArg, runFromArg, runToArg, runAssetArg, runToAssetArg string
	var runAmountBase, runAmountDecimal, runFromAddress, runRecipient, runFromAmountForGas string
	var runSlippageBps int64
	var runSimulate bool
	var runRPCURL string
	var runSigner, runKeySource, runPollInterval, runStepTimeout string
	var runGasMultiplier float64
	var runMaxFeeGwei, runMaxPriorityFeeGwei string
	var runAllowMaxApproval, runUnsafeProviderTx bool
	runCmd := &cobra.Command{
		Use:   "run",
		Short: "Plan and execute a bridge action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			providerName := strings.ToLower(strings.TrimSpace(runProviderArg))
			if providerName == "" {
				return clierr.New(clierr.CodeUsage, "--provider is required")
			}
			txSigner, runSenderAddress, err := resolveRunSignerAndFromAddress(runSigner, runKeySource, runFromAddress)
			if err != nil {
				return err
			}
			reqStruct, err := buildRequest(runFromArg, runToArg, runAssetArg, runToAssetArg, runAmountBase, runAmountDecimal, runFromAmountForGas)
			if err != nil {
				return err
			}
			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, providerInfoName, err := s.actionBuilderRegistry().BuildBridgeAction(ctx, providerName, reqStruct, providers.BridgeExecutionOptions{
				Sender:           runSenderAddress,
				Recipient:        runRecipient,
				SlippageBps:      runSlippageBps,
				Simulate:         runSimulate,
				RPCURL:           runRPCURL,
				FromAmountForGas: runFromAmountForGas,
			})
			if strings.TrimSpace(providerInfoName) == "" {
				providerInfoName = providerName
			}
			statuses := []model.ProviderStatus{{Name: providerInfoName, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
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
				runSimulate,
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
	runCmd.Flags().StringVar(&runProviderArg, "provider", "", "Bridge provider (across|lifi)")
	runCmd.Flags().StringVar(&runFromArg, "from", "", "Source chain")
	runCmd.Flags().StringVar(&runToArg, "to", "", "Destination chain")
	runCmd.Flags().StringVar(&runAssetArg, "asset", "", "Asset on source chain")
	runCmd.Flags().StringVar(&runToAssetArg, "to-asset", "", "Destination asset override")
	runCmd.Flags().StringVar(&runAmountBase, "amount", "", "Amount in base units")
	runCmd.Flags().StringVar(&runAmountDecimal, "amount-decimal", "", "Amount in decimal units")
	runCmd.Flags().StringVar(&runFromAmountForGas, "from-amount-for-gas", "", "Optional amount in source token base units to reserve for destination native gas (LiFi)")
	runCmd.Flags().StringVar(&runFromAddress, "from-address", "", "Sender EOA address (defaults to signer address)")
	runCmd.Flags().StringVar(&runRecipient, "recipient", "", "Recipient address (defaults to --from-address)")
	runCmd.Flags().Int64Var(&runSlippageBps, "slippage-bps", 50, "Max slippage in basis points")
	runCmd.Flags().BoolVar(&runSimulate, "simulate", true, "Run preflight simulation before submission")
	runCmd.Flags().StringVar(&runRPCURL, "rpc-url", "", "RPC URL override for source chain")
	runCmd.Flags().StringVar(&runSigner, "signer", "local", "Signer backend (local)")
	runCmd.Flags().StringVar(&runKeySource, "key-source", execsigner.KeySourceAuto, "Key source (auto|env|file|keystore)")
	runCmd.Flags().StringVar(&runPollInterval, "poll-interval", "2s", "Receipt polling interval")
	runCmd.Flags().StringVar(&runStepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	runCmd.Flags().Float64Var(&runGasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	runCmd.Flags().StringVar(&runMaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	runCmd.Flags().StringVar(&runMaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")
	runCmd.Flags().BoolVar(&runAllowMaxApproval, "allow-max-approval", false, "Allow approval amounts greater than planned input amount")
	runCmd.Flags().BoolVar(&runUnsafeProviderTx, "unsafe-provider-tx", false, "Bypass provider transaction guardrails for bridge/aggregator payloads")
	_ = runCmd.MarkFlagRequired("from")
	_ = runCmd.MarkFlagRequired("to")
	_ = runCmd.MarkFlagRequired("asset")
	_ = runCmd.MarkFlagRequired("provider")

	var submitActionID string
	var submitSimulate bool
	var submitSigner, submitKeySource, submitFromAddress, submitPollInterval, submitStepTimeout string
	var submitGasMultiplier float64
	var submitMaxFeeGwei, submitMaxPriorityFeeGwei string
	var submitAllowMaxApproval, submitUnsafeProviderTx bool
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing bridge action",
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
			if action.IntentType != "bridge" {
				return clierr.New(clierr.CodeUsage, "action is not a bridge intent")
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
		Short: "Get bridge action status",
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
}
