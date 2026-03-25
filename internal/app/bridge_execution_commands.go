package app

import (
	"context"
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
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

	type bridgePlanArgs struct {
		Provider         string `json:"provider" flag:"provider" required:"true" enum:"across,lifi"`
		FromArg          string `json:"from" flag:"from" required:"true" format:"chain"`
		ToArg            string `json:"to" flag:"to" required:"true" format:"chain"`
		AssetArg         string `json:"asset" flag:"asset" required:"true" format:"asset"`
		ToAssetArg       string `json:"to_asset" flag:"to-asset" format:"asset"`
		AmountBase       string `json:"amount" flag:"amount" format:"base-units"`
		AmountDecimal    string `json:"amount_decimal" flag:"amount-decimal" format:"decimal-amount"`
		FromAmountForGas string `json:"from_amount_for_gas" flag:"from-amount-for-gas" format:"base-units"`
		WalletRef        string `json:"wallet" flag:"wallet" format:"identifier"`
		FromAddress      string `json:"from_address" flag:"from-address" format:"evm-address"`
		Recipient        string `json:"recipient" flag:"recipient" format:"evm-address"`
		SlippageBps      int64  `json:"slippage_bps" flag:"slippage-bps"`
		Simulate         bool   `json:"simulate" flag:"simulate"`
		RPCURL           string `json:"rpc_url" flag:"rpc-url" format:"url"`
	}
	type bridgeSubmitArgs struct {
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
	var plan bridgePlanArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist a bridge action plan",
		RunE: func(cmd *cobra.Command, _ []string) error {
			providerName := strings.ToLower(strings.TrimSpace(plan.Provider))
			if providerName == "" {
				return clierr.New(clierr.CodeUsage, "--provider is required")
			}
			identity, err := resolveExecutionIdentity(plan.WalletRef, plan.FromAddress, plan.FromArg)
			if err != nil {
				return err
			}
			reqStruct, err := buildRequest(plan.FromArg, plan.ToArg, plan.AssetArg, plan.ToAssetArg, plan.AmountBase, plan.AmountDecimal, plan.FromAmountForGas)
			if err != nil {
				return err
			}
			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, providerInfoName, err := s.actionBuilderRegistry().BuildBridgeAction(ctx, providerName, reqStruct, providers.BridgeExecutionOptions{
				Sender:           identity.FromAddress,
				Recipient:        plan.Recipient,
				SlippageBps:      plan.SlippageBps,
				Simulate:         plan.Simulate,
				RPCURL:           plan.RPCURL,
				FromAmountForGas: plan.FromAmountForGas,
			})
			if strings.TrimSpace(providerInfoName) == "" {
				providerInfoName = providerName
			}
			statuses := []model.ProviderStatus{{Name: providerInfoName, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
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
	planCmd.Flags().StringVar(&plan.Provider, "provider", "", "Bridge provider (across|lifi)")
	planCmd.Flags().StringVar(&plan.FromArg, "from", "", "Source chain")
	planCmd.Flags().StringVar(&plan.ToArg, "to", "", "Destination chain")
	planCmd.Flags().StringVar(&plan.AssetArg, "asset", "", "Asset on source chain")
	planCmd.Flags().StringVar(&plan.ToAssetArg, "to-asset", "", "Destination asset override")
	planCmd.Flags().StringVar(&plan.AmountBase, "amount", "", "Amount in base units")
	planCmd.Flags().StringVar(&plan.AmountDecimal, "amount-decimal", "", "Amount in decimal units")
	planCmd.Flags().StringVar(&plan.FromAmountForGas, "from-amount-for-gas", "", "Optional amount in source token base units to reserve for destination native gas (LiFi)")
	planCmd.Flags().StringVar(&plan.WalletRef, "wallet", "", "Wallet identifier or name")
	planCmd.Flags().StringVar(&plan.FromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&plan.Recipient, "recipient", "", "Recipient address (defaults to --from-address)")
	planCmd.Flags().Int64Var(&plan.SlippageBps, "slippage-bps", 50, "Max slippage in basis points")
	planCmd.Flags().BoolVar(&plan.Simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.RPCURL, "rpc-url", "", "RPC URL override for source chain")
	_ = planCmd.MarkFlagRequired("from")
	_ = planCmd.MarkFlagRequired("to")
	_ = planCmd.MarkFlagRequired("asset")
	_ = planCmd.MarkFlagRequired("provider")
	configureStructuredInput[bridgePlanArgs](planCmd, structuredInputOptions{Mutation: true})

	var submit bridgeSubmitArgs
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing bridge action",
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
			if action.IntentType != "bridge" {
				return clierr.New(clierr.CodeUsage, "action is not a bridge intent")
			}
			if action.Status == execution.ActionStatusCompleted {
				return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, []string{"action already completed"}, cacheMetaBypass(), nil, false)
			}
			txSigner, err := newExecutionSigner(submit.Signer, submit.KeySource, submit.PrivateKey)
			if err != nil {
				return err
			}
			senderAddr := effectiveSenderAddress(txSigner)
			if strings.TrimSpace(submit.FromAddress) != "" && !strings.EqualFold(strings.TrimSpace(submit.FromAddress), senderAddr) {
				return clierr.New(clierr.CodeSigner, "signer address does not match --from-address")
			}
			if strings.TrimSpace(action.FromAddress) != "" && !strings.EqualFold(strings.TrimSpace(action.FromAddress), senderAddr) {
				return clierr.New(clierr.CodeSigner, "signer address does not match planned action sender")
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
			if err := s.executeActionWithTimeout(&action, txSigner, execOpts); err != nil {
				return err
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), nil, false)
		},
	}
	submitCmd.Flags().StringVar(&submit.ActionID, "action-id", "", "Action identifier returned by bridge plan")
	submitCmd.Flags().BoolVar(&submit.Simulate, "simulate", true, "Run preflight simulation before submission")
	submitCmd.Flags().StringVar(&submit.Signer, "signer", "local", "Signer backend (local|tempo)")
	submitCmd.Flags().StringVar(&submit.KeySource, "key-source", execsigner.KeySourceAuto, "Key source (auto|env|file|keystore)")
	submitCmd.Flags().StringVar(&submit.PrivateKey, "private-key", "", "Private key hex override for local signer (less safe)")
	submitCmd.Flags().StringVar(&submit.FromAddress, "from-address", "", "Expected sender EOA address")
	submitCmd.Flags().StringVar(&submit.PollInterval, "poll-interval", "2s", "Receipt polling interval")
	submitCmd.Flags().StringVar(&submit.StepTimeout, "step-timeout", "2m", "Timeout per bridge wait stage (receipt or settlement polling)")
	submitCmd.Flags().Float64Var(&submit.GasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	submitCmd.Flags().StringVar(&submit.MaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	submitCmd.Flags().StringVar(&submit.MaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")
	submitCmd.Flags().BoolVar(&submit.AllowMaxApproval, "allow-max-approval", false, "Allow approval amounts greater than planned input amount (needed for some provider routes, e.g. Across max approvals)")
	submitCmd.Flags().BoolVar(&submit.UnsafeProviderTx, "unsafe-provider-tx", false, "Bypass provider transaction guardrails for bridge/aggregator payloads")
	submitCmd.Flags().StringVar(&submit.FeeToken, "fee-token", "", "Fee token address for Tempo chains (defaults to chain USDC.e)")
	annotateStructuredSubmitCommand(submitCmd, bridgeSubmitArgs{})

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
			if action.IntentType != "bridge" {
				return clierr.New(clierr.CodeUsage, "action is not a bridge intent")
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), nil, false)
		},
	}
	statusCmd.Flags().StringVar(&statusActionID, "action-id", "", "Action identifier returned by bridge plan")
	annotateExecutionStatusCommand(statusCmd)

	root.AddCommand(planCmd)
	root.AddCommand(submitCmd)
	root.AddCommand(statusCmd)
}
