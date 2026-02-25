package execution

import (
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"math/big"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution/signer"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

type ExecuteOptions struct {
	Simulate           bool
	PollInterval       time.Duration
	StepTimeout        time.Duration
	GasMultiplier      float64
	MaxFeeGwei         string
	MaxPriorityFeeGwei string
	AllowMaxApproval   bool
	UnsafeProviderTx   bool
}

var (
	settlementHTTPClient = httpx.New(10*time.Second, 2)
	signerNonceLocks     sync.Map
)

func DefaultExecuteOptions() ExecuteOptions {
	return ExecuteOptions{
		Simulate:      true,
		PollInterval:  2 * time.Second,
		StepTimeout:   2 * time.Minute,
		GasMultiplier: 1.2,
	}
}

func ExecuteAction(ctx context.Context, store *Store, action *Action, txSigner signer.Signer, opts ExecuteOptions) error {
	if action == nil {
		return clierr.New(clierr.CodeInternal, "missing action")
	}
	if txSigner == nil {
		return clierr.New(clierr.CodeSigner, "missing signer")
	}
	if len(action.Steps) == 0 {
		return clierr.New(clierr.CodeUsage, "action has no executable steps")
	}
	if opts.PollInterval <= 0 {
		opts.PollInterval = 2 * time.Second
	}
	if opts.StepTimeout <= 0 {
		opts.StepTimeout = 2 * time.Minute
	}
	if opts.GasMultiplier <= 1 {
		return clierr.New(clierr.CodeUsage, "gas multiplier must be > 1")
	}
	persist := func() {
		action.Touch()
		if store != nil {
			_ = store.Save(*action)
		}
	}
	action.Status = ActionStatusRunning
	action.FromAddress = txSigner.Address().Hex()
	persist()

	for i := range action.Steps {
		step := &action.Steps[i]
		if step.Status == StepStatusConfirmed {
			continue
		}
		if strings.TrimSpace(step.RPCURL) == "" {
			markStepFailed(action, step, "missing rpc url")
			persist()
			return clierr.New(clierr.CodeUsage, "missing rpc url for action step")
		}
		if strings.TrimSpace(step.Target) == "" {
			markStepFailed(action, step, "missing target")
			persist()
			return clierr.New(clierr.CodeUsage, "missing target for action step")
		}
		if !common.IsHexAddress(step.Target) {
			markStepFailed(action, step, "invalid target address")
			persist()
			return clierr.New(clierr.CodeUsage, "invalid target address for action step")
		}
		client, err := ethclient.DialContext(ctx, step.RPCURL)
		if err != nil {
			markStepFailed(action, step, err.Error())
			persist()
			return clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
		}

		if err := executeStep(ctx, client, txSigner, action, step, opts, persist); err != nil {
			client.Close()
			if step.Status != StepStatusFailed {
				markStepFailed(action, step, err.Error())
			}
			persist()
			return err
		}
		client.Close()
		persist()
	}
	action.Status = ActionStatusCompleted
	persist()
	return nil
}

func executeStep(ctx context.Context, client *ethclient.Client, txSigner signer.Signer, action *Action, step *ActionStep, opts ExecuteOptions, persist func()) error {
	chainID, err := client.ChainID(ctx)
	if err != nil {
		return clierr.Wrap(clierr.CodeUnavailable, "read chain id", err)
	}
	if step.ChainID != "" {
		expected := fmt.Sprintf("eip155:%d", chainID.Int64())
		if !strings.EqualFold(strings.TrimSpace(step.ChainID), expected) {
			return clierr.New(clierr.CodeActionPlan, fmt.Sprintf("step chain mismatch: expected %s, got %s", expected, step.ChainID))
		}
	}
	if !common.IsHexAddress(step.Target) {
		return clierr.New(clierr.CodeUsage, "invalid step target address")
	}
	target := common.HexToAddress(step.Target)
	step.Target = target.Hex()
	data, err := decodeHex(step.Data)
	if err != nil {
		return clierr.Wrap(clierr.CodeUsage, "decode step calldata", err)
	}
	if err := validateStepPolicy(action, step, chainID.Int64(), data, opts); err != nil {
		return err
	}
	value, ok := new(big.Int).SetString(step.Value, 10)
	if !ok {
		return clierr.New(clierr.CodeUsage, "invalid step value")
	}
	msg := ethereum.CallMsg{From: txSigner.Address(), To: &target, Value: value, Data: data}
	if txHash, ok := normalizeStepTxHash(step.TxHash); ok {
		step.Status = StepStatusSubmitted
		safePersist(persist)
		return waitForStepConfirmation(ctx, client, step, msg, txHash, opts, persist)
	}

	if opts.Simulate {
		if _, err := client.CallContract(ctx, msg, nil); err != nil {
			return wrapEVMExecutionError(clierr.CodeActionSim, "simulate step (eth_call)", err)
		}
		step.Status = StepStatusSimulated
		safePersist(persist)
	}

	gasLimit, err := client.EstimateGas(ctx, msg)
	if err != nil {
		return wrapEVMExecutionError(clierr.CodeActionSim, "estimate gas", err)
	}
	gasLimit = uint64(float64(gasLimit) * opts.GasMultiplier)
	if gasLimit == 0 {
		return clierr.New(clierr.CodeActionSim, "estimate gas returned zero")
	}

	tipCap, err := resolveTipCap(ctx, client, opts.MaxPriorityFeeGwei)
	if err != nil {
		return err
	}
	header, err := client.HeaderByNumber(ctx, nil)
	if err != nil {
		return clierr.Wrap(clierr.CodeUnavailable, "fetch latest header", err)
	}
	baseFee := header.BaseFee
	if baseFee == nil {
		baseFee = big.NewInt(1_000_000_000)
	}
	feeCap, err := resolveFeeCap(baseFee, tipCap, opts.MaxFeeGwei)
	if err != nil {
		return err
	}
	unlockNonce := acquireSignerNonceLock(chainID, txSigner.Address())
	defer unlockNonce()
	nonce, err := client.PendingNonceAt(ctx, txSigner.Address())
	if err != nil {
		return clierr.Wrap(clierr.CodeUnavailable, "fetch nonce", err)
	}

	tx := types.NewTx(&types.DynamicFeeTx{
		ChainID:   chainID,
		Nonce:     nonce,
		GasTipCap: tipCap,
		GasFeeCap: feeCap,
		Gas:       gasLimit,
		To:        &target,
		Value:     value,
		Data:      data,
	})
	signed, err := txSigner.SignTx(chainID, tx)
	if err != nil {
		return clierr.Wrap(clierr.CodeSigner, "sign transaction", err)
	}
	if err := client.SendTransaction(ctx, signed); err != nil {
		return wrapEVMExecutionError(clierr.CodeUnavailable, "broadcast transaction", err)
	}
	step.Status = StepStatusSubmitted
	step.TxHash = signed.Hash().Hex()
	safePersist(persist)
	return waitForStepConfirmation(ctx, client, step, msg, signed.Hash(), opts, persist)
}

func waitForStepConfirmation(ctx context.Context, client *ethclient.Client, step *ActionStep, msg ethereum.CallMsg, txHash common.Hash, opts ExecuteOptions, persist func()) error {
	waitCtx, cancel := context.WithTimeout(ctx, opts.StepTimeout)
	defer cancel()
	ticker := time.NewTicker(opts.PollInterval)
	defer ticker.Stop()
	for {
		receipt, err := client.TransactionReceipt(waitCtx, txHash)
		if err == nil && receipt != nil {
			if receipt.Status == types.ReceiptStatusSuccessful {
				if err := verifyBridgeSettlement(ctx, step, txHash.Hex(), opts); err != nil {
					return err
				}
				step.Status = StepStatusConfirmed
				safePersist(persist)
				return nil
			}
			if reason := decodeReceiptRevertReason(waitCtx, client, msg, receipt.BlockNumber); reason != "" {
				return clierr.New(clierr.CodeUnavailable, "transaction reverted on-chain: "+reason)
			}
			return clierr.New(clierr.CodeUnavailable, "transaction reverted on-chain")
		}
		if waitCtx.Err() != nil {
			return clierr.Wrap(clierr.CodeActionTimeout, "timed out waiting for receipt", waitCtx.Err())
		}
		if err != nil && !errors.Is(err, ethereum.NotFound) {
			// Ignore transient RPC polling failures until timeout.
		}
		select {
		case <-waitCtx.Done():
			return clierr.Wrap(clierr.CodeActionTimeout, "timed out waiting for receipt", waitCtx.Err())
		case <-ticker.C:
		}
	}
}

func safePersist(persist func()) {
	if persist == nil {
		return
	}
	persist()
}

func normalizeStepTxHash(value string) (common.Hash, bool) {
	hash := strings.TrimSpace(value)
	if hash == "" {
		return common.Hash{}, false
	}
	decoded, err := decodeHex(hash)
	if err != nil || len(decoded) != common.HashLength {
		return common.Hash{}, false
	}
	return common.HexToHash(hash), true
}

func acquireSignerNonceLock(chainID *big.Int, signerAddress common.Address) func() {
	key := strings.ToLower(chainID.String() + ":" + signerAddress.Hex())
	lockAny, _ := signerNonceLocks.LoadOrStore(key, &sync.Mutex{})
	lock := lockAny.(*sync.Mutex)
	lock.Lock()
	return lock.Unlock
}

func wrapEVMExecutionError(code clierr.Code, operation string, err error) error {
	revert := decodeRevertFromError(err)
	if revert == "" {
		return clierr.Wrap(code, operation, err)
	}
	return clierr.Wrap(code, operation+": "+revert, err)
}

func decodeReceiptRevertReason(ctx context.Context, client *ethclient.Client, msg ethereum.CallMsg, blockNumber *big.Int) string {
	if client == nil {
		return ""
	}
	callCtx := ctx
	if callCtx == nil {
		callCtx = context.Background()
	}
	callCtx, cancel := context.WithTimeout(callCtx, 5*time.Second)
	defer cancel()
	_, err := client.CallContract(callCtx, msg, blockNumber)
	return decodeRevertFromError(err)
}

type rpcDataError interface {
	error
	ErrorData() interface{}
}

func decodeRevertFromError(err error) string {
	if err == nil {
		return ""
	}
	var dataErr rpcDataError
	if errors.As(err, &dataErr) {
		return decodeRevertData(dataErr.ErrorData())
	}
	return ""
}

func decodeRevertData(data any) string {
	bytesData, ok := normalizeErrorData(data)
	if !ok || len(bytesData) == 0 {
		return ""
	}
	if reason, err := abi.UnpackRevert(bytesData); err == nil && strings.TrimSpace(reason) != "" {
		return reason
	}
	if len(bytesData) >= 4 {
		return fmt.Sprintf("custom error selector 0x%s", hex.EncodeToString(bytesData[:4]))
	}
	return ""
}

func normalizeErrorData(data any) ([]byte, bool) {
	switch v := data.(type) {
	case []byte:
		if len(v) == 0 {
			return nil, false
		}
		return v, true
	case string:
		decoded, err := decodeHex(v)
		if err != nil || len(decoded) == 0 {
			return nil, false
		}
		return decoded, true
	default:
		return nil, false
	}
}

func verifyBridgeSettlement(ctx context.Context, step *ActionStep, sourceTxHash string, opts ExecuteOptions) error {
	if step == nil || step.Type != StepTypeBridge {
		return nil
	}
	if step.ExpectedOutputs == nil {
		return nil
	}
	provider := strings.ToLower(strings.TrimSpace(step.ExpectedOutputs["settlement_provider"]))
	if provider == "" {
		return nil
	}
	switch provider {
	case "lifi":
		statusEndpoint := strings.TrimSpace(step.ExpectedOutputs["settlement_status_endpoint"])
		if statusEndpoint == "" {
			statusEndpoint = registry.LiFiSettlementURL
		}
		return waitForLiFiSettlement(ctx, step, sourceTxHash, statusEndpoint, opts)
	case "across":
		statusEndpoint := strings.TrimSpace(step.ExpectedOutputs["settlement_status_endpoint"])
		if statusEndpoint == "" {
			statusEndpoint = registry.AcrossSettlementURL
		}
		return waitForAcrossSettlement(ctx, step, sourceTxHash, statusEndpoint, opts)
	default:
		return clierr.New(clierr.CodeUnsupported, fmt.Sprintf("unsupported bridge settlement provider %q", provider))
	}
}

type liFiStatusResponse struct {
	Status           string `json:"status"`
	Substatus        string `json:"substatus"`
	SubstatusMessage string `json:"substatusMessage"`
	Message          string `json:"message"`
	Code             int    `json:"code"`
	LiFiExplorerLink string `json:"lifiExplorerLink"`
	Receiving        struct {
		TxHash string `json:"txHash"`
		Amount string `json:"amount"`
	} `json:"receiving"`
}

func waitForLiFiSettlement(ctx context.Context, step *ActionStep, sourceTxHash, statusEndpoint string, opts ExecuteOptions) error {
	waitCtx, cancel := context.WithTimeout(ctx, opts.StepTimeout)
	defer cancel()
	ticker := time.NewTicker(opts.PollInterval)
	defer ticker.Stop()

	for {
		resp, err := queryLiFiStatus(waitCtx, sourceTxHash, statusEndpoint, step.ExpectedOutputs)
		if err == nil {
			status := strings.ToUpper(strings.TrimSpace(resp.Status))
			if status != "" {
				setStepOutput(step, "settlement_status", status)
			}
			if strings.TrimSpace(resp.Substatus) != "" {
				setStepOutput(step, "settlement_substatus", strings.TrimSpace(resp.Substatus))
			}
			if strings.TrimSpace(resp.SubstatusMessage) != "" {
				setStepOutput(step, "settlement_message", strings.TrimSpace(resp.SubstatusMessage))
			}
			if strings.TrimSpace(resp.LiFiExplorerLink) != "" {
				setStepOutput(step, "settlement_explorer_url", strings.TrimSpace(resp.LiFiExplorerLink))
			}
			if strings.TrimSpace(resp.Receiving.TxHash) != "" {
				setStepOutput(step, "destination_tx_hash", strings.TrimSpace(resp.Receiving.TxHash))
			}

			switch status {
			case "DONE":
				return nil
			case "FAILED", "INVALID":
				msg := firstNonEmpty(strings.TrimSpace(resp.SubstatusMessage), strings.TrimSpace(resp.Message), "LiFi transfer reported failure")
				return clierr.New(clierr.CodeUnavailable, "bridge settlement failed: "+msg)
			}
		}
		if waitCtx.Err() != nil {
			return clierr.Wrap(clierr.CodeActionTimeout, "timed out waiting for bridge settlement", waitCtx.Err())
		}
		select {
		case <-waitCtx.Done():
			return clierr.Wrap(clierr.CodeActionTimeout, "timed out waiting for bridge settlement", waitCtx.Err())
		case <-ticker.C:
		}
	}
}

type acrossStatusResponse struct {
	Status           string `json:"status"`
	Message          string `json:"message"`
	Error            string `json:"error"`
	DepositTxHash    string `json:"depositTxHash"`
	FillTx           string `json:"fillTx"`
	DepositRefundTx  string `json:"depositRefundTxHash"`
	OriginChainID    int64  `json:"originChainId"`
	DestinationChain int64  `json:"destinationChainId"`
}

func waitForAcrossSettlement(ctx context.Context, step *ActionStep, sourceTxHash, statusEndpoint string, opts ExecuteOptions) error {
	waitCtx, cancel := context.WithTimeout(ctx, opts.StepTimeout)
	defer cancel()
	ticker := time.NewTicker(opts.PollInterval)
	defer ticker.Stop()

	for {
		resp, err := queryAcrossStatus(waitCtx, sourceTxHash, statusEndpoint, step.ExpectedOutputs)
		if err == nil {
			status := strings.ToLower(strings.TrimSpace(resp.Status))
			if status != "" {
				setStepOutput(step, "settlement_status", status)
			}
			if strings.TrimSpace(resp.FillTx) != "" {
				setStepOutput(step, "destination_tx_hash", strings.TrimSpace(resp.FillTx))
			}
			if strings.TrimSpace(resp.DepositRefundTx) != "" {
				setStepOutput(step, "refund_tx_hash", strings.TrimSpace(resp.DepositRefundTx))
			}

			switch status {
			case "filled":
				return nil
			case "refunded":
				return clierr.New(clierr.CodeUnavailable, "bridge settlement refunded")
			case "pending", "unfilled":
				// keep polling
			default:
				if strings.TrimSpace(status) != "" {
					// Keep polling unknown statuses until timeout.
				}
			}
		}
		if waitCtx.Err() != nil {
			return clierr.Wrap(clierr.CodeActionTimeout, "timed out waiting for bridge settlement", waitCtx.Err())
		}
		select {
		case <-waitCtx.Done():
			return clierr.Wrap(clierr.CodeActionTimeout, "timed out waiting for bridge settlement", waitCtx.Err())
		case <-ticker.C:
		}
	}
}

func queryLiFiStatus(ctx context.Context, sourceTxHash, statusEndpoint string, expected map[string]string) (liFiStatusResponse, error) {
	var out liFiStatusResponse

	endpoint := strings.TrimSpace(statusEndpoint)
	if endpoint == "" {
		endpoint = registry.LiFiSettlementURL
	}
	parsed, err := url.Parse(endpoint)
	if err != nil {
		return out, err
	}
	query := parsed.Query()
	query.Set("txHash", strings.TrimPrefix(strings.TrimPrefix(strings.TrimSpace(sourceTxHash), "0x"), "0X"))
	if bridge := strings.TrimSpace(expected["settlement_bridge"]); bridge != "" {
		query.Set("bridge", bridge)
	}
	if fromChain := strings.TrimSpace(expected["settlement_from_chain"]); fromChain != "" {
		query.Set("fromChain", fromChain)
	}
	if toChain := strings.TrimSpace(expected["settlement_to_chain"]); toChain != "" {
		query.Set("toChain", toChain)
	}
	parsed.RawQuery = query.Encode()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, parsed.String(), nil)
	if err != nil {
		return out, err
	}
	if _, err := settlementHTTPClient.DoJSON(ctx, req, &out); err != nil {
		return out, clierr.Wrap(clierr.CodeUnavailable, "query lifi settlement status", err)
	}
	if out.Code != 0 && out.Status == "" {
		// LiFi can report pending/non-indexed transfers with API-level codes.
		if out.Code == 1003 || out.Code == 1011 {
			return out, nil
		}
		return out, errors.New(firstNonEmpty(strings.TrimSpace(out.Message), "unexpected status response"))
	}
	return out, nil
}

func queryAcrossStatus(ctx context.Context, sourceTxHash, statusEndpoint string, expected map[string]string) (acrossStatusResponse, error) {
	var out acrossStatusResponse

	endpoint := strings.TrimSpace(statusEndpoint)
	if endpoint == "" {
		endpoint = registry.AcrossSettlementURL
	}
	parsed, err := url.Parse(endpoint)
	if err != nil {
		return out, err
	}
	query := parsed.Query()
	query.Set("depositTxHash", strings.TrimSpace(sourceTxHash))
	if origin := strings.TrimSpace(expected["settlement_origin_chain"]); origin != "" {
		query.Set("originChainId", origin)
	}
	if recipient := strings.TrimSpace(expected["settlement_recipient"]); recipient != "" {
		query.Set("recipient", recipient)
	}
	parsed.RawQuery = query.Encode()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, parsed.String(), nil)
	if err != nil {
		return out, err
	}
	if _, err := settlementHTTPClient.DoJSON(ctx, req, &out); err != nil {
		return out, clierr.Wrap(clierr.CodeUnavailable, "query across settlement status", err)
	}
	if strings.TrimSpace(out.Error) != "" {
		if strings.EqualFold(strings.TrimSpace(out.Error), "DepositNotFoundException") {
			return out, nil
		}
		return out, errors.New(firstNonEmpty(strings.TrimSpace(out.Message), strings.TrimSpace(out.Error), "unexpected across status response"))
	}
	return out, nil
}

func setStepOutput(step *ActionStep, key, value string) {
	if step == nil || strings.TrimSpace(key) == "" {
		return
	}
	if step.ExpectedOutputs == nil {
		step.ExpectedOutputs = map[string]string{}
	}
	step.ExpectedOutputs[key] = value
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if strings.TrimSpace(v) != "" {
			return strings.TrimSpace(v)
		}
	}
	return ""
}

func resolveTipCap(ctx context.Context, client *ethclient.Client, overrideGwei string) (*big.Int, error) {
	if strings.TrimSpace(overrideGwei) != "" {
		v, err := parseGwei(overrideGwei)
		if err != nil {
			return nil, clierr.Wrap(clierr.CodeUsage, "parse --max-priority-fee-gwei", err)
		}
		return v, nil
	}
	tipCap, err := client.SuggestGasTipCap(ctx)
	if err != nil {
		return big.NewInt(2_000_000_000), nil // 2 gwei fallback
	}
	return tipCap, nil
}

func resolveFeeCap(baseFee, tipCap *big.Int, overrideGwei string) (*big.Int, error) {
	if strings.TrimSpace(overrideGwei) != "" {
		v, err := parseGwei(overrideGwei)
		if err != nil {
			return nil, clierr.Wrap(clierr.CodeUsage, "parse --max-fee-gwei", err)
		}
		if v.Cmp(tipCap) < 0 {
			return nil, clierr.New(clierr.CodeUsage, "--max-fee-gwei must be >= --max-priority-fee-gwei")
		}
		return v, nil
	}
	feeCap := new(big.Int).Mul(baseFee, big.NewInt(2))
	feeCap.Add(feeCap, tipCap)
	return feeCap, nil
}

func parseGwei(v string) (*big.Int, error) {
	clean := strings.TrimSpace(v)
	if clean == "" {
		return nil, fmt.Errorf("empty gwei value")
	}
	rat, ok := new(big.Rat).SetString(clean)
	if !ok {
		return nil, fmt.Errorf("invalid numeric value %q", v)
	}
	if rat.Sign() < 0 {
		return nil, fmt.Errorf("value must be non-negative")
	}
	scale := big.NewRat(1_000_000_000, 1)
	rat.Mul(rat, scale)
	out := new(big.Int)
	if !rat.IsInt() {
		return nil, fmt.Errorf("value must resolve to an integer wei amount")
	}
	out = new(big.Int).Set(rat.Num())
	return out, nil
}

func markStepFailed(action *Action, step *ActionStep, msg string) {
	step.Status = StepStatusFailed
	step.Error = msg
	action.Status = ActionStatusFailed
	action.Touch()
}

func decodeHex(v string) ([]byte, error) {
	clean := strings.TrimSpace(v)
	clean = strings.TrimPrefix(clean, "0x")
	if clean == "" {
		return []byte{}, nil
	}
	if len(clean)%2 != 0 {
		clean = "0" + clean
	}
	buf, err := hex.DecodeString(clean)
	if err != nil {
		return nil, fmt.Errorf("invalid hex: %w", err)
	}
	return buf, nil
}
