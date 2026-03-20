package tempo

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/big"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

type rpcRequest struct {
	JSONRPC string            `json:"jsonrpc"`
	ID      json.RawMessage   `json:"id"`
	Method  string            `json:"method"`
	Params  []json.RawMessage `json:"params"`
}

type callObject struct {
	To    string `json:"to"`
	Input string `json:"input"`
	Data  string `json:"data"`
}

type mockRPCConfig struct {
	allowance        *big.Int
	quoteExactIn     *big.Int
	quoteExactOut    *big.Int
	quoteExactInErr  string
	quoteExactOutErr string
}

func TestQuoteSwapExactInput(t *testing.T) {
	server := newMockRPCServer(t, mockRPCConfig{allowance: big.NewInt(0)})
	defer server.Close()

	c := New()
	chain, _ := id.ParseChain("tempo")
	fromAsset, _ := id.ParseAsset("pathUSD", chain)
	toAsset, _ := id.ParseAsset("USDC.e", chain)
	quote, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
		RPCURL:          server.URL,
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}
	if quote.Provider != "tempo" {
		t.Fatalf("unexpected provider: %s", quote.Provider)
	}
	if quote.TradeType != "exact-input" {
		t.Fatalf("unexpected trade type: %s", quote.TradeType)
	}
	if quote.InputAmount.AmountBaseUnits != "1000000" {
		t.Fatalf("unexpected input amount: %s", quote.InputAmount.AmountBaseUnits)
	}
	if quote.EstimatedOut.AmountBaseUnits != "980000" {
		t.Fatalf("unexpected output amount: %s", quote.EstimatedOut.AmountBaseUnits)
	}
}

func TestQuoteSwapExactOutput(t *testing.T) {
	server := newMockRPCServer(t, mockRPCConfig{allowance: big.NewInt(0)})
	defer server.Close()

	c := New()
	chain, _ := id.ParseChain("tempo")
	fromAsset, _ := id.ParseAsset("pathUSD", chain)
	toAsset, _ := id.ParseAsset("USDC.e", chain)
	quote, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
		RPCURL:          server.URL,
		TradeType:       providers.SwapTradeTypeExactOutput,
	})
	if err != nil {
		t.Fatalf("QuoteSwap failed: %v", err)
	}
	if quote.TradeType != "exact-output" {
		t.Fatalf("unexpected trade type: %s", quote.TradeType)
	}
	if quote.InputAmount.AmountBaseUnits != "1010100" {
		t.Fatalf("unexpected quoted input amount: %s", quote.InputAmount.AmountBaseUnits)
	}
	if quote.EstimatedOut.AmountBaseUnits != "1000000" {
		t.Fatalf("unexpected output amount: %s", quote.EstimatedOut.AmountBaseUnits)
	}
}

func TestBuildSwapActionBatchesApproveAndSwapForExactInput(t *testing.T) {
	server := newMockRPCServer(t, mockRPCConfig{allowance: big.NewInt(0)})
	defer server.Close()

	c := New()
	chain, _ := id.ParseChain("tempo")
	fromAsset, _ := id.ParseAsset("pathUSD", chain)
	toAsset, _ := id.ParseAsset("USDC.e", chain)
	action, err := c.BuildSwapAction(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	}, providers.SwapExecutionOptions{
		Sender:      "0x00000000000000000000000000000000000000AA",
		SlippageBps: 100,
		Simulate:    true,
		RPCURL:      server.URL,
	})
	if err != nil {
		t.Fatalf("BuildSwapAction failed: %v", err)
	}
	if action.Provider != "tempo" {
		t.Fatalf("unexpected provider: %s", action.Provider)
	}
	if len(action.Steps) != 1 {
		t.Fatalf("expected 1 batched step, got %d", len(action.Steps))
	}
	step := action.Steps[0]
	if step.StepID != "tempo-swap-exact-input" {
		t.Fatalf("unexpected swap step id: %s", step.StepID)
	}
	if step.Type != "swap" {
		t.Fatalf("expected swap step type, got %s", step.Type)
	}
	if len(step.Calls) != 2 {
		t.Fatalf("expected 2 calls (approve + swap), got %d", len(step.Calls))
	}
	// First call is the ERC-20 approve.
	if !strings.HasPrefix(step.Calls[0].Data, "0x095ea7b3") {
		t.Fatalf("expected approve selector in first call, got %s", step.Calls[0].Data[:10])
	}
	// Second call is the swap.
	if step.Calls[1].Target == "" {
		t.Fatal("expected non-empty target in swap call")
	}
}

func TestBuildSwapActionSingleCallWhenApproved(t *testing.T) {
	// Set allowance high enough so no approve call is needed.
	server := newMockRPCServer(t, mockRPCConfig{allowance: big.NewInt(9999999)})
	defer server.Close()

	c := New()
	chain, _ := id.ParseChain("tempo")
	fromAsset, _ := id.ParseAsset("pathUSD", chain)
	toAsset, _ := id.ParseAsset("USDC.e", chain)
	action, err := c.BuildSwapAction(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	}, providers.SwapExecutionOptions{
		Sender:      "0x00000000000000000000000000000000000000AA",
		SlippageBps: 100,
		Simulate:    true,
		RPCURL:      server.URL,
	})
	if err != nil {
		t.Fatalf("BuildSwapAction failed: %v", err)
	}
	if len(action.Steps) != 1 {
		t.Fatalf("expected 1 step, got %d", len(action.Steps))
	}
	step := action.Steps[0]
	if len(step.Calls) != 1 {
		t.Fatalf("expected 1 call (swap only), got %d", len(step.Calls))
	}
	if step.Target != "" {
		t.Fatalf("expected empty Target for batched step, got %s", step.Target)
	}
	if step.Data != "" {
		t.Fatalf("expected empty Data for batched step, got %s", step.Data)
	}
}

func TestBuildSwapActionExactOutputUsesMaxInput(t *testing.T) {
	server := newMockRPCServer(t, mockRPCConfig{allowance: big.NewInt(0)})
	defer server.Close()

	c := New()
	chain, _ := id.ParseChain("tempo")
	fromAsset, _ := id.ParseAsset("pathUSD", chain)
	toAsset, _ := id.ParseAsset("USDC.e", chain)
	action, err := c.BuildSwapAction(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
		TradeType:       providers.SwapTradeTypeExactOutput,
	}, providers.SwapExecutionOptions{
		Sender:      "0x00000000000000000000000000000000000000AA",
		SlippageBps: 100,
		Simulate:    true,
		RPCURL:      server.URL,
	})
	if err != nil {
		t.Fatalf("BuildSwapAction failed: %v", err)
	}
	if action.InputAmount != "1020201" {
		t.Fatalf("expected max input amount 1020201, got %s", action.InputAmount)
	}
	if len(action.Steps) != 1 {
		t.Fatalf("expected 1 batched step, got %d", len(action.Steps))
	}
	step := action.Steps[0]
	if step.StepID != "tempo-swap-exact-output" {
		t.Fatalf("unexpected swap step id: %s", step.StepID)
	}
	// With zero allowance, should have approve + swap calls.
	if len(step.Calls) != 2 {
		t.Fatalf("expected 2 calls (approve + swap), got %d", len(step.Calls))
	}
}

func TestBuildSwapActionRejectsRecipientMismatch(t *testing.T) {
	c := New()
	chain, _ := id.ParseChain("tempo")
	fromAsset, _ := id.ParseAsset("pathUSD", chain)
	toAsset, _ := id.ParseAsset("USDC.e", chain)
	_, err := c.BuildSwapAction(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		AmountDecimal:   "1",
	}, providers.SwapExecutionOptions{
		Sender:    "0x00000000000000000000000000000000000000AA",
		Recipient: "0x00000000000000000000000000000000000000BB",
	})
	if err == nil {
		t.Fatal("expected recipient mismatch to fail")
	}
}

func TestQuoteSwapRejectsNonUSDCurrency(t *testing.T) {
	server := newMockRPCServer(t, mockRPCConfig{allowance: big.NewInt(0)})
	defer server.Close()

	c := New()
	chain, _ := id.ParseChain("tempo")
	fromAsset, _ := id.ParseAsset("USDC.e", chain)
	toAsset, _ := id.ParseAsset("EURC.e", chain)
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		RPCURL:          server.URL,
	})
	if err == nil {
		t.Fatal("expected non-USD pair to fail")
	}
	cErr, ok := clierr.As(err)
	if !ok || cErr.Code != clierr.CodeUnsupported {
		t.Fatalf("expected unsupported error, got %v", err)
	}
	if !strings.Contains(err.Error(), "USD-denominated TIP-20s") {
		t.Fatalf("expected USD-only guidance, got %v", err)
	}
}

func TestQuoteSwapClassifiesPairDoesNotExistAsUnsupported(t *testing.T) {
	server := newMockRPCServer(t, mockRPCConfig{
		allowance:       big.NewInt(0),
		quoteExactInErr: "execution reverted: PairDoesNotExist",
	})
	defer server.Close()

	c := New()
	chain, _ := id.ParseChain("tempo")
	fromAsset, _ := id.ParseAsset("pathUSD", chain)
	toAsset, _ := id.ParseAsset("USDC.e", chain)
	_, err := c.QuoteSwap(context.Background(), providers.SwapQuoteRequest{
		Chain:           chain,
		FromAsset:       fromAsset,
		ToAsset:         toAsset,
		AmountBaseUnits: "1000000",
		RPCURL:          server.URL,
	})
	if err == nil {
		t.Fatal("expected PairDoesNotExist to fail")
	}
	cErr, ok := clierr.As(err)
	if !ok || cErr.Code != clierr.CodeUnsupported {
		t.Fatalf("expected unsupported error, got %v", err)
	}
	if !strings.Contains(err.Error(), "does not support") {
		t.Fatalf("expected pair support guidance, got %v", err)
	}
}

func newMockRPCServer(t *testing.T, cfg mockRPCConfig) *httptest.Server {
	t.Helper()
	if cfg.allowance == nil {
		cfg.allowance = big.NewInt(0)
	}
	if cfg.quoteExactIn == nil {
		cfg.quoteExactIn = big.NewInt(980000)
	}
	if cfg.quoteExactOut == nil {
		cfg.quoteExactOut = big.NewInt(1010100)
	}

	handler := func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var req rpcRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		switch req.Method {
		case "eth_call":
			var call callObject
			if len(req.Params) == 0 {
				writeRPCError(w, req.ID, -32602, "missing params")
				return
			}
			if err := json.Unmarshal(req.Params[0], &call); err != nil {
				writeRPCError(w, req.ID, -32602, err.Error())
				return
			}
			callData := call.Data
			if callData == "" {
				callData = call.Input
			}
			switch {
			case strings.HasPrefix(callData, "0x"+hex.EncodeToString(tempoTIP20ABI.Methods["currency"].ID)):
				currency, ok := tempoTokenCurrency(strings.ToLower(call.To))
				if !ok {
					writeRPCError(w, req.ID, -32000, "execution reverted: UnknownToken")
					return
				}
				out, err := tempoTIP20ABI.Methods["currency"].Outputs.Pack(currency)
				if err != nil {
					t.Fatalf("pack currency output: %v", err)
				}
				writeRPCResult(w, req.ID, "0x"+hex.EncodeToString(out))
			case strings.HasPrefix(callData, "0x"+hex.EncodeToString(tempoDEXABI.Methods["quoteSwapExactAmountIn"].ID)):
				if cfg.quoteExactInErr != "" {
					writeRPCError(w, req.ID, -32000, cfg.quoteExactInErr)
					return
				}
				out, err := tempoDEXABI.Methods["quoteSwapExactAmountIn"].Outputs.Pack(cfg.quoteExactIn)
				if err != nil {
					t.Fatalf("pack quoteExactAmountIn output: %v", err)
				}
				writeRPCResult(w, req.ID, "0x"+hex.EncodeToString(out))
			case strings.HasPrefix(callData, "0x"+hex.EncodeToString(tempoDEXABI.Methods["quoteSwapExactAmountOut"].ID)):
				if cfg.quoteExactOutErr != "" {
					writeRPCError(w, req.ID, -32000, cfg.quoteExactOutErr)
					return
				}
				out, err := tempoDEXABI.Methods["quoteSwapExactAmountOut"].Outputs.Pack(cfg.quoteExactOut)
				if err != nil {
					t.Fatalf("pack quoteExactAmountOut output: %v", err)
				}
				writeRPCResult(w, req.ID, "0x"+hex.EncodeToString(out))
			case strings.HasPrefix(callData, "0x"+hex.EncodeToString(tempoERC20.Methods["allowance"].ID)):
				out, err := tempoERC20.Methods["allowance"].Outputs.Pack(cfg.allowance)
				if err != nil {
					t.Fatalf("pack allowance output: %v", err)
				}
				writeRPCResult(w, req.ID, "0x"+hex.EncodeToString(out))
			default:
				writeRPCError(w, req.ID, -32601, fmt.Sprintf("unsupported eth_call data %s", callData))
			}
		default:
			writeRPCError(w, req.ID, -32601, fmt.Sprintf("unsupported method %s", req.Method))
		}
	}

	return httptest.NewServer(http.HandlerFunc(handler))
}

func tempoTokenCurrency(token string) (string, bool) {
	switch strings.ToLower(token) {
	case strings.ToLower("0x20c0000000000000000000000000000000000000"):
		return "USD", true
	case strings.ToLower("0x20c000000000000000000000b9537d11c60e8b50"):
		return "USD", true
	case strings.ToLower("0x20c0000000000000000000001621e21f71cf12fb"):
		return "EUR", true
	case strings.ToLower("0x20c00000000000000000000014f22ca97301eb73"):
		return "USD", true
	default:
		return "", false
	}
}

func writeRPCResult(w http.ResponseWriter, id json.RawMessage, result any) {
	w.Header().Set("Content-Type", "application/json")
	_, _ = fmt.Fprintf(w, `{"jsonrpc":"2.0","id":%s,"result":%q}`, rawIDOrDefault(id), result)
}

func writeRPCError(w http.ResponseWriter, id json.RawMessage, code int, message string) {
	w.Header().Set("Content-Type", "application/json")
	_, _ = fmt.Fprintf(w, `{"jsonrpc":"2.0","id":%s,"error":{"code":%d,"message":%q}}`, rawIDOrDefault(id), code, message)
}

func rawIDOrDefault(id json.RawMessage) string {
	if len(id) == 0 {
		return "1"
	}
	return string(id)
}
