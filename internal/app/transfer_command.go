package app

import (
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

func (s *runtimeState) newTransferCommand() *cobra.Command {
	root := &cobra.Command{Use: "transfer", Short: "ERC-20 transfer execution commands"}

	type transferArgs struct {
		ChainArg      string `json:"chain" flag:"chain" required:"true" format:"chain"`
		AssetArg      string `json:"asset" flag:"asset" required:"true" format:"asset"`
		AmountBase    string `json:"amount" flag:"amount" format:"base-units"`
		AmountDecimal string `json:"amount_decimal" flag:"amount-decimal" format:"decimal-amount"`
		FromAddress   string `json:"from_address" flag:"from-address" required:"true" format:"evm-address"`
		Recipient     string `json:"recipient" flag:"recipient" required:"true" format:"evm-address"`
		Simulate      bool   `json:"simulate" flag:"simulate"`
		RPCURL        string `json:"rpc_url" flag:"rpc-url" format:"url"`
	}
	type transferSubmitArgs struct {
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
	}
	buildAction := func(args transferArgs) (execution.Action, error) {
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
		return s.actionBuilderRegistry().BuildTransferAction(actionbuilder.TransferRequest{
			Chain:           chain,
			Asset:           asset,
			AmountBaseUnits: base,
			Sender:          args.FromAddress,
			Recipient:       args.Recipient,
			Simulate:        args.Simulate,
			RPCURL:          args.RPCURL,
		})
	}

	var plan transferArgs
	planCmd := &cobra.Command{
		Use:   "plan",
		Short: "Create and persist an ERC-20 transfer action plan",
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
	planCmd.Flags().StringVar(&plan.ChainArg, "chain", "", "Chain identifier")
	planCmd.Flags().StringVar(&plan.AssetArg, "asset", "", "Asset symbol/address/CAIP-19")
	planCmd.Flags().StringVar(&plan.AmountBase, "amount", "", "Amount in base units")
	planCmd.Flags().StringVar(&plan.AmountDecimal, "amount-decimal", "", "Amount in decimal units")
	planCmd.Flags().StringVar(&plan.FromAddress, "from-address", "", "Sender EOA address")
	planCmd.Flags().StringVar(&plan.Recipient, "recipient", "", "Recipient EOA address")
	planCmd.Flags().BoolVar(&plan.Simulate, "simulate", true, "Include simulation checks during execution")
	planCmd.Flags().StringVar(&plan.RPCURL, "rpc-url", "", "RPC URL override for the selected chain")
	_ = planCmd.MarkFlagRequired("chain")
	_ = planCmd.MarkFlagRequired("asset")
	_ = planCmd.MarkFlagRequired("from-address")
	_ = planCmd.MarkFlagRequired("recipient")
	configureStructuredInput[transferArgs](planCmd, structuredInputOptions{Mutation: true})

	var submit transferSubmitArgs
	submitCmd := &cobra.Command{
		Use:   "submit",
		Short: "Execute an existing ERC-20 transfer action",
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
			if action.IntentType != "transfer" {
				return clierr.New(clierr.CodeUsage, "action is not a transfer intent")
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
				false,
				false,
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
	submitCmd.Flags().StringVar(&submit.ActionID, "action-id", "", "Action identifier returned by transfer plan")
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
	annotateStructuredSubmitCommand(submitCmd, transferSubmitArgs{})

	var statusActionID string
	statusCmd := &cobra.Command{
		Use:   "status",
		Short: "Get transfer action status",
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
			if action.IntentType != "transfer" {
				return clierr.New(clierr.CodeUsage, "action is not a transfer intent")
			}
			return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), nil, false)
		},
	}
	statusCmd.Flags().StringVar(&statusActionID, "action-id", "", "Action identifier returned by transfer plan")
	annotateExecutionStatusCommand(statusCmd)

	root.AddCommand(planCmd)
	root.AddCommand(submitCmd)
	root.AddCommand(statusCmd)
	return root
}
