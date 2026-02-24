package app

import (
	"context"
	"fmt"
	"sort"
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
			provider, ok := s.bridgeProviders[providerName]
			if !ok {
				return clierr.New(clierr.CodeUnsupported, "unsupported bridge provider")
			}
			execProvider, ok := provider.(providers.BridgeExecutionProvider)
			if !ok {
				return clierr.New(clierr.CodeUnsupported, fmt.Sprintf("bridge provider %q is quote-only; execution providers: %s", providerName, strings.Join(bridgeExecutionProviderNames(s.bridgeProviders), ",")))
			}
			reqStruct, err := buildRequest(planFromArg, planToArg, planAssetArg, planToAssetArg, planAmountBase, planAmountDecimal, planFromAmountForGas)
			if err != nil {
				return err
			}
			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, err := execProvider.BuildBridgeAction(ctx, reqStruct, providers.BridgeExecutionOptions{
				Sender:           planFromAddress,
				Recipient:        planRecipient,
				SlippageBps:      planSlippageBps,
				Simulate:         planSimulate,
				RPCURL:           planRPCURL,
				FromAmountForGas: planFromAmountForGas,
			})
			statuses := []model.ProviderStatus{{Name: provider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
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
	var runSimulate, runYes bool
	var runRPCURL string
	var runSigner, runKeySource, runConfirmAddress, runPollInterval, runStepTimeout string
	var runGasMultiplier float64
	var runMaxFeeGwei, runMaxPriorityFeeGwei string
	runCmd := &cobra.Command{
		Use:   "run",
		Short: "Plan and execute a bridge action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			if !runYes {
				return clierr.New(clierr.CodeUsage, "bridge run requires --yes")
			}
			providerName := strings.ToLower(strings.TrimSpace(runProviderArg))
			if providerName == "" {
				return clierr.New(clierr.CodeUsage, "--provider is required")
			}
			provider, ok := s.bridgeProviders[providerName]
			if !ok {
				return clierr.New(clierr.CodeUnsupported, "unsupported bridge provider")
			}
			execProvider, ok := provider.(providers.BridgeExecutionProvider)
			if !ok {
				return clierr.New(clierr.CodeUnsupported, fmt.Sprintf("bridge provider %q is quote-only; execution providers: %s", providerName, strings.Join(bridgeExecutionProviderNames(s.bridgeProviders), ",")))
			}
			reqStruct, err := buildRequest(runFromArg, runToArg, runAssetArg, runToAssetArg, runAmountBase, runAmountDecimal, runFromAmountForGas)
			if err != nil {
				return err
			}
			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, err := execProvider.BuildBridgeAction(ctx, reqStruct, providers.BridgeExecutionOptions{
				Sender:           runFromAddress,
				Recipient:        runRecipient,
				SlippageBps:      runSlippageBps,
				Simulate:         runSimulate,
				RPCURL:           runRPCURL,
				FromAmountForGas: runFromAmountForGas,
			})
			statuses := []model.ProviderStatus{{Name: provider.Info().Name, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
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
			txSigner, err := newExecutionSigner(runSigner, runKeySource, runConfirmAddress)
			if err != nil {
				s.captureCommandDiagnostics(nil, statuses, false)
				return err
			}
			if !strings.EqualFold(strings.TrimSpace(runFromAddress), txSigner.Address().Hex()) {
				s.captureCommandDiagnostics(nil, statuses, false)
				return clierr.New(clierr.CodeSigner, "signer address does not match --from-address")
			}
			execOpts, err := parseExecuteOptions(runSimulate, runPollInterval, runStepTimeout, runGasMultiplier, runMaxFeeGwei, runMaxPriorityFeeGwei)
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
	runCmd.Flags().StringVar(&runFromAddress, "from-address", "", "Sender EOA address")
	runCmd.Flags().StringVar(&runRecipient, "recipient", "", "Recipient address (defaults to --from-address)")
	runCmd.Flags().Int64Var(&runSlippageBps, "slippage-bps", 50, "Max slippage in basis points")
	runCmd.Flags().BoolVar(&runSimulate, "simulate", true, "Run preflight simulation before submission")
	runCmd.Flags().StringVar(&runRPCURL, "rpc-url", "", "RPC URL override for source chain")
	runCmd.Flags().StringVar(&runSigner, "signer", "local", "Signer backend (local)")
	runCmd.Flags().StringVar(&runKeySource, "key-source", execsigner.KeySourceAuto, "Key source (auto|env|file|keystore)")
	runCmd.Flags().StringVar(&runConfirmAddress, "confirm-address", "", "Require signer address to match this value")
	runCmd.Flags().StringVar(&runPollInterval, "poll-interval", "2s", "Receipt polling interval")
	runCmd.Flags().StringVar(&runStepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	runCmd.Flags().Float64Var(&runGasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	runCmd.Flags().StringVar(&runMaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	runCmd.Flags().StringVar(&runMaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")
	runCmd.Flags().BoolVar(&runYes, "yes", false, "Confirm execution")
	_ = runCmd.MarkFlagRequired("from")
	_ = runCmd.MarkFlagRequired("to")
	_ = runCmd.MarkFlagRequired("asset")
	_ = runCmd.MarkFlagRequired("from-address")
	_ = runCmd.MarkFlagRequired("provider")

	var submitActionID, submitPlanID string
	var submitYes, submitSimulate bool
	var submitSigner, submitKeySource, submitConfirmAddress, submitPollInterval, submitStepTimeout string
	var submitGasMultiplier float64
	var submitMaxFeeGwei, submitMaxPriorityFeeGwei string
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing bridge action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			if !submitYes {
				return clierr.New(clierr.CodeUsage, "bridge submit requires --yes")
			}
			actionID, err := resolveActionID(submitActionID, submitPlanID)
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
	submitCmd.Flags().StringVar(&submitPlanID, "plan-id", "", "Deprecated alias for --action-id")
	submitCmd.Flags().BoolVar(&submitYes, "yes", false, "Confirm execution")
	submitCmd.Flags().BoolVar(&submitSimulate, "simulate", true, "Run preflight simulation before submission")
	submitCmd.Flags().StringVar(&submitSigner, "signer", "local", "Signer backend (local)")
	submitCmd.Flags().StringVar(&submitKeySource, "key-source", execsigner.KeySourceAuto, "Key source (auto|env|file|keystore)")
	submitCmd.Flags().StringVar(&submitConfirmAddress, "confirm-address", "", "Require signer address to match this value")
	submitCmd.Flags().StringVar(&submitPollInterval, "poll-interval", "2s", "Receipt polling interval")
	submitCmd.Flags().StringVar(&submitStepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	submitCmd.Flags().Float64Var(&submitGasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	submitCmd.Flags().StringVar(&submitMaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	submitCmd.Flags().StringVar(&submitMaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")

	var statusActionID, statusPlanID string
	statusCmd := &cobra.Command{
		Use:   "status",
		Short: "Get bridge action status",
		RunE: func(cmd *cobra.Command, _ []string) error {
			actionID, err := resolveActionID(statusActionID, statusPlanID)
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
	statusCmd.Flags().StringVar(&statusPlanID, "plan-id", "", "Deprecated alias for --action-id")

	root.AddCommand(planCmd)
	root.AddCommand(runCmd)
	root.AddCommand(submitCmd)
	root.AddCommand(statusCmd)
}

func bridgeExecutionProviderNames(all map[string]providers.BridgeProvider) []string {
	names := make([]string, 0, len(all))
	for name, provider := range all {
		if _, ok := provider.(providers.BridgeExecutionProvider); ok {
			names = append(names, name)
		}
	}
	sort.Strings(names)
	return names
}
