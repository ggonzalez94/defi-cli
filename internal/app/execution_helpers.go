package app

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum/common"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	execsigner "github.com/ggonzalez94/defi-cli/internal/execution/signer"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/ows"
	"github.com/spf13/cobra"
)

const executionStepRPCOverhead = 15 * time.Second

type submitExecutionInputs struct {
	Signer      string
	KeySource   string
	PrivateKey  string
	FromAddress string
}

type resolvedSubmitExecution struct {
	txSigner   execsigner.Signer
	evmBackend execution.EVMSubmitBackend
	sender     string
}

func (s *runtimeState) executeActionWithTimeout(action *execution.Action, txSigner execsigner.Signer, evmBackend execution.EVMSubmitBackend, opts execution.ExecuteOptions) error {
	timeout := estimateExecutionTimeout(action, opts)
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	return execution.ExecuteAction(ctx, s.actionStore, action, txSigner, evmBackend, opts)
}

func resolveActionExecutionBackend(cmd *cobra.Command, action execution.Action, input submitExecutionInputs) (resolvedSubmitExecution, error) {
	switch strings.ToLower(strings.TrimSpace(string(action.ExecutionBackend))) {
	case "", string(execution.ExecutionBackendLegacyLocal):
		signerBackend := strings.ToLower(strings.TrimSpace(input.Signer))
		if signerBackend == "" {
			signerBackend = "local"
		}
		if signerBackend != "local" {
			return resolvedSubmitExecution{}, clierr.New(clierr.CodeUsage, "legacy actions only support --signer local; tempo submit requires execution_backend=tempo")
		}
		txSigner, err := newExecutionSigner("local", input.KeySource, input.PrivateKey)
		if err != nil {
			return resolvedSubmitExecution{}, err
		}
		sender := effectiveSenderAddress(txSigner)
		return resolvedSubmitExecution{
			txSigner:   txSigner,
			evmBackend: execution.NewLocalSubmitBackend(txSigner),
			sender:     sender,
		}, nil
	case string(execution.ExecutionBackendOWS):
		if strings.TrimSpace(action.WalletID) == "" {
			return resolvedSubmitExecution{}, clierr.New(clierr.CodeUsage, "wallet-backed action is missing persisted wallet_id")
		}
		if usesLegacySignerFlags(cmd) {
			return resolvedSubmitExecution{}, clierr.New(clierr.CodeUsage, "wallet-backed actions do not accept legacy signer flags (--signer, --key-source, --private-key)")
		}
		sender, err := resolvePersistedOWSSender(action)
		if err != nil {
			return resolvedSubmitExecution{}, err
		}
		return resolvedSubmitExecution{
			evmBackend: execution.NewOWSSubmitBackend(action.WalletID, common.HexToAddress(sender)),
			sender:     sender,
		}, nil
	case string(execution.ExecutionBackendTempo):
		txSigner, err := newExecutionSigner("tempo", input.KeySource, input.PrivateKey)
		if err != nil {
			return resolvedSubmitExecution{}, err
		}
		return resolvedSubmitExecution{
			txSigner: txSigner,
			sender:   effectiveSenderAddress(txSigner),
		}, nil
	default:
		return resolvedSubmitExecution{}, clierr.New(clierr.CodeUnsupported, "unsupported execution backend for submit")
	}
}

func usesLegacySignerFlags(cmd *cobra.Command) bool {
	if cmd == nil {
		return false
	}
	for _, name := range []string{"signer", "key-source", "private-key"} {
		flag := cmd.Flags().Lookup(name)
		if flag != nil && flag.Changed {
			return true
		}
	}
	return false
}

func resolvePersistedOWSSender(action execution.Action) (string, error) {
	chainID := strings.TrimSpace(action.ChainID)
	if chainID == "" {
		for _, step := range action.Steps {
			if strings.TrimSpace(step.ChainID) != "" {
				chainID = strings.TrimSpace(step.ChainID)
				break
			}
		}
	}
	if chainID == "" {
		return "", clierr.New(clierr.CodeUsage, "wallet-backed action is missing chain id for sender resolution")
	}

	wallet, err := ows.ResolveWalletRef("", action.WalletID)
	if err != nil {
		return "", clierr.Wrap(classifyWalletResolveErrorCode(err), "resolve persisted wallet_id", err)
	}
	sender, err := ows.SenderAddressForChain(wallet, chainID)
	if err != nil {
		return "", clierr.Wrap(classifyWalletSenderErrorCode(err), "resolve wallet sender for action chain", err)
	}
	if !common.IsHexAddress(sender) {
		return "", clierr.New(clierr.CodeUnavailable, "resolved wallet sender must be a valid EVM hex address")
	}
	canonicalSender := common.HexToAddress(sender).Hex()
	if strings.TrimSpace(action.FromAddress) != "" && !strings.EqualFold(strings.TrimSpace(action.FromAddress), canonicalSender) {
		return "", clierr.New(clierr.CodeSigner, "planned action sender does not match resolved wallet sender")
	}
	return canonicalSender, nil
}

func validateExecutionSender(action execution.Action, expectedSender, actualSender string) error {
	if strings.TrimSpace(expectedSender) != "" && !strings.EqualFold(strings.TrimSpace(expectedSender), actualSender) {
		return clierr.New(clierr.CodeSigner, "signer address does not match --from-address")
	}
	if strings.TrimSpace(action.FromAddress) != "" && !strings.EqualFold(strings.TrimSpace(action.FromAddress), actualSender) {
		return clierr.New(clierr.CodeSigner, "signer address does not match planned action sender")
	}
	return nil
}

// executionSubmitArgs holds the common set of fields used by all execution
// submit commands. Each command keeps its own typed struct (with json/flag tags
// for schema annotation) and passes these fields to runSubmitAction.
type executionSubmitArgs struct {
	ActionID           string
	Simulate           bool
	Signer             string
	KeySource          string
	PrivateKey         string
	FromAddress        string
	PollInterval       string
	StepTimeout        string
	GasMultiplier      float64
	MaxFeeGwei         string
	MaxPriorityFeeGwei string
	AllowMaxApproval   bool
	UnsafeProviderTx   bool
	FeeToken           string
}

// registerSubmitFlags registers the flags shared by all execution submit commands.
func registerSubmitFlags(cmd *cobra.Command, args *executionSubmitArgs, commandName string) {
	cmd.Flags().StringVar(&args.ActionID, "action-id", "", fmt.Sprintf("Action identifier returned by %s plan", commandName))
	cmd.Flags().BoolVar(&args.Simulate, "simulate", true, "Run preflight simulation before submission")
	cmd.Flags().StringVar(&args.Signer, "signer", "local", "Signer backend (local|tempo)")
	cmd.Flags().StringVar(&args.KeySource, "key-source", execsigner.KeySourceAuto, "Key source (auto|env|file|keystore)")
	cmd.Flags().StringVar(&args.PrivateKey, "private-key", "", "Private key hex override for local signer (less safe)")
	cmd.Flags().StringVar(&args.FromAddress, "from-address", "", "Expected sender EOA address")
	cmd.Flags().StringVar(&args.PollInterval, "poll-interval", "2s", "Receipt polling interval")
	cmd.Flags().StringVar(&args.StepTimeout, "step-timeout", "2m", "Per-step receipt timeout")
	cmd.Flags().Float64Var(&args.GasMultiplier, "gas-multiplier", 1.2, "Gas estimate safety multiplier")
	cmd.Flags().StringVar(&args.MaxFeeGwei, "max-fee-gwei", "", "Optional EIP-1559 max fee (gwei)")
	cmd.Flags().StringVar(&args.MaxPriorityFeeGwei, "max-priority-fee-gwei", "", "Optional EIP-1559 max priority fee (gwei)")
	cmd.Flags().BoolVar(&args.AllowMaxApproval, "allow-max-approval", false, "Allow approval amounts greater than planned input amount")
	cmd.Flags().BoolVar(&args.UnsafeProviderTx, "unsafe-provider-tx", false, "Bypass provider transaction guardrails for bridge/aggregator payloads")
	cmd.Flags().StringVar(&args.FeeToken, "fee-token", "", "Fee token address for Tempo chains (defaults to chain USDC.e)")
}

func (s *runtimeState) newStatusCommand(commandName, expectedIntent, intentMismatchMsg string) *cobra.Command {
	var actionID string
	cmd := &cobra.Command{
		Use:   "status",
		Short: fmt.Sprintf("Get %s action status", commandName),
		RunE: func(cmd *cobra.Command, _ []string) error {
			return s.runStatusAction(cmd, actionID, expectedIntent, intentMismatchMsg)
		},
	}
	cmd.Flags().StringVar(&actionID, "action-id", "", fmt.Sprintf("Action identifier returned by %s plan", commandName))
	annotateExecutionStatusCommand(cmd)
	return cmd
}

func (s *runtimeState) runSubmitAction(cmd *cobra.Command, args executionSubmitArgs, expectedIntent, intentMismatchMsg string) error {
	actionID, err := resolveActionID(args.ActionID)
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
		return clierr.New(clierr.CodeUsage, intentMismatchMsg)
	}
	if action.Status == execution.ActionStatusCompleted {
		return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, []string{"action already completed"}, cacheMetaBypass(), nil, false)
	}
	resolvedExec, err := resolveActionExecutionBackend(cmd, action, submitExecutionInputs{
		Signer:      args.Signer,
		KeySource:   args.KeySource,
		PrivateKey:  args.PrivateKey,
		FromAddress: args.FromAddress,
	})
	if err != nil {
		return err
	}
	if err := validateExecutionSender(action, args.FromAddress, resolvedExec.sender); err != nil {
		return err
	}
	execOpts, err := parseExecuteOptions(
		args.Simulate,
		args.PollInterval,
		args.StepTimeout,
		args.GasMultiplier,
		args.MaxFeeGwei,
		args.MaxPriorityFeeGwei,
		args.AllowMaxApproval,
		args.UnsafeProviderTx,
		args.FeeToken,
	)
	if err != nil {
		return err
	}
	if err := s.executeActionWithTimeout(&action, resolvedExec.txSigner, resolvedExec.evmBackend, execOpts); err != nil {
		return err
	}
	return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), nil, false)
}

func (s *runtimeState) runStatusAction(cmd *cobra.Command, actionIDStr, expectedIntent, intentMismatchMsg string) error {
	actionID, err := resolveActionID(actionIDStr)
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
		return clierr.New(clierr.CodeUsage, intentMismatchMsg)
	}
	return s.emitSuccess(trimRootPath(cmd.CommandPath()), action, nil, cacheMetaBypass(), nil, false)
}

// planActionConfig holds the parameters for runPlanAction, which encapsulates the
// common plan command flow: resolve identity → build action → persist → emit.
type planActionConfig struct {
	ProviderName string
	WalletRef    string
	FromAddress  string
	ChainArg     string
	BuildAction  func(ctx context.Context, fromAddr string) (execution.Action, error)
}

func (s *runtimeState) runPlanAction(cmd *cobra.Command, cfg planActionConfig) error {
	identity, err := resolveExecutionIdentity(cfg.WalletRef, cfg.FromAddress, cfg.ChainArg)
	if err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(context.Background(), s.settings.Timeout)
	defer cancel()
	start := time.Now()
	action, err := cfg.BuildAction(ctx, identity.FromAddress)
	statuses := []model.ProviderStatus{{Name: cfg.ProviderName, Status: statusFromErr(err), LatencyMS: time.Since(start).Milliseconds()}}
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
}

// Execution timeout is derived from remaining action wait stages so short provider
// request timeouts do not cancel transaction confirmation/settlement polling early.
func estimateExecutionTimeout(action *execution.Action, opts execution.ExecuteOptions) time.Duration {
	stepTimeout := opts.StepTimeout
	if stepTimeout <= 0 {
		stepTimeout = execution.DefaultExecuteOptions().StepTimeout
	}
	stages := 0
	steps := 0
	if action != nil {
		for _, step := range action.Steps {
			if step.Status == execution.StepStatusConfirmed {
				continue
			}
			steps++
			stages++
			if step.Type == execution.StepTypeBridge {
				// Bridge steps wait for source receipt and destination settlement.
				stages++
			}
		}
	}
	if stages <= 0 {
		stages = 1
	}
	if steps <= 0 {
		steps = 1
	}
	// Add per-step RPC headroom for chain-id/simulation/gas/fee/nonce/broadcast work
	// so long-running receipt/settlement waits are less likely to be cut off early.
	return time.Duration(stages)*stepTimeout + time.Duration(steps)*executionStepRPCOverhead
}
