package ows

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"strings"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
)

const EnvOWSToken = "DEFI_OWS_TOKEN"

type SendTxResult struct {
	TxHash string `json:"tx_hash"`
	Chain  string `json:"chain"`
}

type sendTxCLIResult struct {
	TxHashSnake string `json:"tx_hash"`
	TxHashCamel string `json:"txHash"`
	Chain       string `json:"chain"`
}

type commandRunner func(ctx context.Context, bin string, args []string, env []string) ([]byte, []byte, error)

var (
	lookPathFunc                 = exec.LookPath
	runCommandFunc commandRunner = runCommand
)

func SendUnsignedTx(ctx context.Context, walletID, chainID string, txHex []byte, rpcURL string) (SendTxResult, error) {
	walletID = strings.TrimSpace(walletID)
	if walletID == "" {
		return SendTxResult{}, clierr.New(clierr.CodeUsage, "wallet id is required")
	}
	chainID = strings.TrimSpace(chainID)
	if chainID == "" {
		return SendTxResult{}, clierr.New(clierr.CodeUsage, "chain id is required")
	}
	if len(txHex) == 0 {
		return SendTxResult{}, clierr.New(clierr.CodeUsage, "unsigned tx bytes are required")
	}

	owsBin, err := lookPathFunc("ows")
	if err != nil {
		return SendTxResult{}, clierr.Wrap(clierr.CodeUnavailable, "ows CLI not found in PATH", err)
	}

	token := strings.TrimSpace(os.Getenv(EnvOWSToken))
	if token == "" {
		return SendTxResult{}, clierr.New(clierr.CodeSigner, "missing DEFI_OWS_TOKEN for OWS passphrase")
	}

	args := []string{
		"sign", "send-tx",
		"--wallet", walletID,
		"--chain", chainID,
		"--tx", "0x" + hex.EncodeToString(txHex),
		"--json",
	}
	if trimmedRPC := strings.TrimSpace(rpcURL); trimmedRPC != "" {
		args = append(args, "--rpc-url", trimmedRPC)
	}

	env := append(os.Environ(), "OWS_PASSPHRASE="+token)
	stdout, stderr, err := runCommandFunc(ctx, owsBin, args, env)
	if err != nil {
		return SendTxResult{}, classifyCommandFailure(err, stdout, stderr)
	}

	result, err := parseSendTxResult(stdout, chainID)
	if err != nil {
		return SendTxResult{}, clierr.Wrap(clierr.CodeSigner, "parse ows send-tx response", err)
	}
	return result, nil
}

func runCommand(ctx context.Context, bin string, args []string, env []string) ([]byte, []byte, error) {
	cmd := exec.CommandContext(ctx, bin, args...)
	cmd.Env = env

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()
	return stdout.Bytes(), stderr.Bytes(), err
}

func parseSendTxResult(out []byte, fallbackChain string) (SendTxResult, error) {
	var parsed sendTxCLIResult
	if err := json.Unmarshal(out, &parsed); err != nil {
		return SendTxResult{}, err
	}

	txHash := strings.TrimSpace(parsed.TxHashSnake)
	if txHash == "" {
		txHash = strings.TrimSpace(parsed.TxHashCamel)
	}
	if txHash == "" {
		return SendTxResult{}, fmt.Errorf("missing tx hash in ows response")
	}
	if !IsTxHash(txHash) {
		return SendTxResult{}, fmt.Errorf("invalid tx hash in ows response: %q", txHash)
	}

	chain := strings.TrimSpace(parsed.Chain)
	if chain == "" {
		chain = strings.TrimSpace(fallbackChain)
	}
	return SendTxResult{
		TxHash: txHash,
		Chain:  chain,
	}, nil
}

func classifyCommandFailure(runErr error, stdout, stderr []byte) error {
	detail := strings.TrimSpace(string(stderr))
	if detail == "" {
		detail = strings.TrimSpace(string(stdout))
	}

	if isPolicyDeniedDetail(detail) {
		if detail == "" {
			return clierr.Wrap(clierr.CodeActionPolicy, "ows policy denied transaction", runErr)
		}
		return clierr.Wrap(clierr.CodeActionPolicy, "ows policy denied transaction", fmt.Errorf("%s: %w", detail, runErr))
	}

	if detail == "" {
		return clierr.Wrap(clierr.CodeSigner, "ows send-tx command failed", runErr)
	}
	return clierr.Wrap(clierr.CodeSigner, "ows send-tx command failed", fmt.Errorf("%s: %w", detail, runErr))
}

func isPolicyDeniedDetail(detail string) bool {
	lower := strings.ToLower(strings.TrimSpace(detail))
	if lower == "" {
		return false
	}
	if strings.Contains(lower, "policy_denied") {
		return true
	}
	normalized := strings.NewReplacer("_", " ", "-", " ").Replace(lower)
	return strings.Contains(normalized, "policy denied") || strings.Contains(normalized, "denied by policy")
}

func IsTxHash(value string) bool {
	trimmed := strings.TrimSpace(value)
	if len(trimmed) != 66 || !strings.HasPrefix(trimmed, "0x") {
		return false
	}
	_, err := hex.DecodeString(trimmed[2:])
	return err == nil
}
