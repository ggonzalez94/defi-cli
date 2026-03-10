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

func (s *runtimeState) addYieldExecutionSubcommands(root *cobra.Command) {
	root.AddCommand(s.newYieldVerbExecutionCommand(actionbuilder.YieldVerbDeposit, "Deposit assets into a yield product"))
	root.AddCommand(s.newYieldVerbExecutionCommand(actionbuilder.YieldVerbWithdraw, "Withdraw assets from a yield product"))
}

func (s *runtimeState) newYieldVerbExecutionCommand(verb actionbuilder.YieldVerb, short string) *cobra.Command {
	root := &cobra.Command{
		Use:   string(verb),
		Short: short,
	}
	expectedIntent := "yield_" + string(verb)

	type yieldArgs struct {
		Provider            string `json:"provider" flag:"provider" required:"true" enum:"aave,morpho,moonwell"`
		ChainArg            string `json:"chain" flag:"chain" required:"true" format:"chain"`
		AssetArg            string `json:"asset" flag:"asset" required:"true" format:"asset"`
		VaultAddress        string `json:"vault_address" flag:"vault-address" format:"evm-address"`
		AmountBase          string `json:"amount" flag:"amount" format:"base-units"`
		AmountDecimal       string `json:"amount_decimal" flag:"amount-decimal" format:"decimal-amount"`
		FromAddress         string `json:"from_address" flag:"from-address" required:"true" format:"evm-address"`
		Recipient           string `json:"recipient" flag:"recipient" format:"evm-address"`
		OnBehalfOf          string `json:"on_behalf_of" flag:"on-behalf-of" format:"evm-address"`
		Simulate            bool   `json:"simulate" flag:"simulate"`
		RPCURL              string `json:"rpc_url" flag:"rpc-url" format:"url"`
		PoolAddress         string `json:"pool_address" flag:"pool-address" format:"evm-address"`
		PoolAddressProvider string `json:"pool_address_provider" flag:"pool-address-provider" format:"evm-address"`
	}
	type yieldSubmitArgs struct {
		ActionID           string  `json:"action_id" flag:"action-id" required:"true" format:"action-id"`
		Simulate           bool    `json:"simulate" flag:"simulate"`
		Signer             string  `json:"signer" flag:"signer" enum:"local"`
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
	}
	buildAction := func(ctx context.Context, args yieldArgs) (execution.Action, error) {
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
		return s.actionBuilderRegistry().BuildYieldAction(ctx, actionbuilder.YieldRequest{
			Provider:            args.Provider,
			Verb:                verb,
			Chain:               chain,
			Asset:               asset,
			VaultAddress:        args.VaultAddress,
			AmountBaseUnits:     base,
			Sender:              args.FromAddress,
			Recipient:           args.Recipient,
			OnBehalfOf:          args.OnBehalfOf,
			Simulate:            args.Simulate,
			RPCURL:              args.RPCURL,
			PoolAddress:         args.PoolAddress,
			PoolAddressProvider: args.PoolAddressProvider,
		})
	}

	var plan yieldArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist a yield action plan",
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
			defer cancel()
			start := time.Now()
			action, err := buildAction(ctx, plan)
			providerName := normalizeLendingProvider(plan.Provider)
			if providerName == "" {
				providerName = "yield"
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
	planCmd.Flags().StringVar(&plan.Provider, "provider", "", "Yield provider (aave|morpho)")
	planCmd.Flags().StringVar(&plan.ChainArg, "chain", "", "Chain identifier")
	planCmd.Flags().StringVar(&plan.AssetArg, "asset", "", "Asset symbol/address/CAIP-19")
	planCmd.Flags().StringVar(&plan.VaultAddress, "vault-address", "", "Morpho vault address (required for --provider morpho)")
	planCmd.Flags().StringVar(&plan.AmountBase, "amount", "", "Amount in base units")
	planCmd.Flags().StringVar(&plan.AmountDecimal, "amount-decimal", "", "Amount in decimal units")
	planCmd.Flags().StringVar(&plan.FromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&plan.Recipient, "recipient", "", "Recipient address (defaults to --from-address)")
	planCmd.Flags().StringVar(&plan.OnBehalfOf, "on-behalf-of", "", "Position owner address (defaults to --from-address)")
	planCmd.Flags().BoolVar(&plan.Simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.RPCURL, "rpc-url", "", "RPC URL override for the selected chain")
	planCmd.Flags().StringVar(&plan.PoolAddress, "pool-address", "", "Aave pool address override")
	planCmd.Flags().StringVar(&plan.PoolAddressProvider, "pool-address-provider", "", "Aave pool address provider override")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("asset")
	_ = planCmd.MarkFlagRequired("from-address")
	_ = planCmd.MarkFlagRequired("provider")
	configureStructuredInput[yieldArgs](planCmd, structuredInputOptions{Mutation: true})

	var submit yieldSubmitArgs
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing yield action",
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
				return clierr.New(clierr.CodeUsage, "action intent does not match yield verb")
			}
			if action.Status == execution.ActionStatusCompleted {
				return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, []string{"action already completed"}, cacheMetaBypass(), nil, false)
			}
			txSigner, err := newExecutionSigner(submit.Signer, submit.KeySource, submit.PrivateKey)
			if err != nil {
				return err
			}
			if strings.TrimSpace(submit.FromAddress) != "" && !strings.EqualFold(strings.TrimSpace(submit.FromAddress), txSigner.Address().Hex()) {
				return clierr.New(clierr.CodeSigner, "signer address does not match --from-address")
			}
			if strings.TrimSpace(action.FromAddress) != "" && !strings.EqualFold(strings.TrimSpace(action.FromAddress), txSigner.Address().Hex()) {
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
	submitCmd.Flags().StringVar(&submit.ActionID, "action-id", "", "Action identifier returned by yield plan")
	submitCmd.Flags().BoolVar(&submit.Simulate, "simulate", true, "Run preflight simulation before submission")
	submitCmd.Flags().StringVar(&submit.Signer, "signer", "local", "Signer backend (local)")
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
	annotateStructuredSubmitCommand(submitCmd, yieldSubmitArgs{})

	var statusActionID string
	statusCmd := &cobra.Command{
		Use:   "status",
		Short: "Get yield action status",
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
				return clierr.New(clierr.CodeUsage, "action intent does not match yield verb")
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), nil, false)
		},
	}
	statusCmd.Flags().StringVar(&statusActionID, "action-id", "", "Action identifier returned by yield plan")
	annotateExecutionStatusCommand(statusCmd)

	root.AddCommand(planCmd)
	root.AddCommand(submitCmd)
	root.AddCommand(statusCmd)
	return root
}
