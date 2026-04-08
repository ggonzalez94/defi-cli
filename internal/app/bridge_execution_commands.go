package app

import (
	"context"
	"strings"
	"time"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
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
	planCmd.Flags().StringVar(&plan.Recipient, "recipient", "", "Recipient address (defaults to the resolved sender address)")
	planCmd.Flags().Int64Var(&plan.SlippageBps, "slippage-bps", 50, "Max slippage in basis points")
	planCmd.Flags().BoolVar(&plan.Simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.RPCURL, "rpc-url", "", "RPC URL override for source chain")
	_ = planCmd.MarkFlagRequired("from")
	_ = planCmd.MarkFlagRequired("to")
	_ = planCmd.MarkFlagRequired("asset")
	_ = planCmd.MarkFlagRequired("provider")
	configureStructuredInput[bridgePlanArgs](planCmd, structuredInputOptions{
		Mutation:         true,
		InputConstraints: standardExecutionIdentityInputConstraints(),
	})

	var submit executionSubmitArgs
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing bridge action",
		RunE: func(cmd *cobra.Command, _ []string) error {
			return s.runSubmitAction(cmd, submit, "bridge", "action is not a bridge intent")
		},
	}
	registerSubmitFlags(submitCmd, &submit, "bridge")
	annotateStructuredSubmitCommand(submitCmd, standardSubmitSchema{})

	statusCmd := s.newStatusCommand("bridge", "bridge", "action is not a bridge intent")

	root.AddCommand(planCmd)
	root.AddCommand(submitCmd)
	root.AddCommand(statusCmd)
}
