package ows

import (
	"context"
	"errors"
	"reflect"
	"testing"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
)

func TestSendUnsignedTxBuildsOwsCommand(t *testing.T) {
	t.Setenv(EnvOWSToken, "test-passphrase")

	origLookPath := lookPathFunc
	origRunner := runCommandFunc
	t.Cleanup(func() {
		lookPathFunc = origLookPath
		runCommandFunc = origRunner
	})

	var gotBin string
	var gotArgs []string
	var gotEnv []string
	lookPathFunc = func(file string) (string, error) {
		if file != "ows" {
			t.Fatalf("unexpected binary lookup: %q", file)
		}
		return "/usr/local/bin/ows", nil
	}
	runCommandFunc = func(_ context.Context, bin string, args []string, env []string) ([]byte, []byte, error) {
		gotBin = bin
		gotArgs = append([]string(nil), args...)
		gotEnv = append([]string(nil), env...)
		return []byte(`{"txHash":"0xabc123"}`), nil, nil
	}

	result, err := SendUnsignedTx(
		context.Background(),
		"wallet-1",
		"eip155:1",
		[]byte{0x01, 0x02, 0x03},
		"https://rpc.example",
	)
	if err != nil {
		t.Fatalf("SendUnsignedTx failed: %v", err)
	}

	if result.TxHash != "0xabc123" {
		t.Fatalf("expected tx hash 0xabc123, got %q", result.TxHash)
	}
	if result.Chain != "eip155:1" {
		t.Fatalf("expected chain eip155:1, got %q", result.Chain)
	}
	if gotBin != "/usr/local/bin/ows" {
		t.Fatalf("expected ows binary path to be captured, got %q", gotBin)
	}

	wantArgs := []string{
		"sign", "send-tx",
		"--wallet", "wallet-1",
		"--chain", "eip155:1",
		"--tx", "0x010203",
		"--json",
		"--rpc-url", "https://rpc.example",
	}
	if !reflect.DeepEqual(gotArgs, wantArgs) {
		t.Fatalf("unexpected args: got %#v want %#v", gotArgs, wantArgs)
	}

	if !containsEnvValue(gotEnv, "OWS_PASSPHRASE=test-passphrase") {
		t.Fatalf("expected child env to include OWS_PASSPHRASE, env=%v", gotEnv)
	}
}

func TestSendUnsignedTxMapsPolicyDenial(t *testing.T) {
	t.Setenv(EnvOWSToken, "test-passphrase")

	origLookPath := lookPathFunc
	origRunner := runCommandFunc
	t.Cleanup(func() {
		lookPathFunc = origLookPath
		runCommandFunc = origRunner
	})

	lookPathFunc = func(string) (string, error) {
		return "/usr/local/bin/ows", nil
	}
	runCommandFunc = func(context.Context, string, []string, []string) ([]byte, []byte, error) {
		return nil, []byte("policy denied by wallet policy"), errors.New("exit status 1")
	}

	_, err := SendUnsignedTx(context.Background(), "wallet-1", "eip155:1", []byte{0x02}, "")
	if err == nil {
		t.Fatal("expected policy denial to return an error")
	}

	typed, ok := clierr.As(err)
	if !ok {
		t.Fatalf("expected cli error type, got %T", err)
	}
	if typed.Code != clierr.CodeActionPolicy {
		t.Fatalf("expected policy error code %d, got %d", clierr.CodeActionPolicy, typed.Code)
	}
}

func containsEnvValue(values []string, item string) bool {
	for _, value := range values {
		if value == item {
			return true
		}
	}
	return false
}
