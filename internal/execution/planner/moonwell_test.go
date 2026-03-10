package planner

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

	"github.com/ethereum/go-ethereum/common"
	"github.com/ggonzalez94/defi-cli/internal/id"
)

const (
	testMToken     = "0x0000000000000000000000000000000000000011"
	testUnderlying = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48" // USDC on Ethereum (used as Base USDC for test purposes)
	testSender     = "0x00000000000000000000000000000000000000AA"
	testRecipient  = "0x00000000000000000000000000000000000000BB"
)

// newMoonwellPlannerRPCServer creates a mock that dispatches by selector:
// - allowance → returns the given allowance
// - checkMembership → returns the given isMember bool
func newMoonwellPlannerRPCServer(t *testing.T, allowance *big.Int, isMember bool) *httptest.Server {
	t.Helper()
	allowanceSel := hex.EncodeToString(plannerERC20ABI.Methods["allowance"].ID)
	checkMembershipSel := hex.EncodeToString(moonwellComptrollerABI.Methods["checkMembership"].ID)

	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var req plannerRPCRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		if req.Method != "eth_call" {
			writePlannerRPCError(w, req.ID, -32601, fmt.Sprintf("method not supported: %s", req.Method))
			return
		}
		var callObj struct {
			Data  string `json:"data"`
			Input string `json:"input"`
		}
		if err := json.Unmarshal(req.Params[0], &callObj); err != nil {
			writePlannerRPCError(w, req.ID, -32602, "bad params")
			return
		}
		rawData := callObj.Data
		if rawData == "" {
			rawData = callObj.Input
		}
		data, _ := hex.DecodeString(strings.TrimPrefix(rawData, "0x"))
		if len(data) < 4 {
			writePlannerRPCError(w, req.ID, -32602, "data too short")
			return
		}
		selector := hex.EncodeToString(data[:4])

		switch selector {
		case allowanceSel:
			encoded, _ := plannerERC20ABI.Methods["allowance"].Outputs.Pack(allowance)
			writePlannerRPCResult(w, req.ID, "0x"+hex.EncodeToString(encoded))
		case checkMembershipSel:
			encoded, _ := moonwellComptrollerABI.Methods["checkMembership"].Outputs.Pack(isMember)
			writePlannerRPCResult(w, req.ID, "0x"+hex.EncodeToString(encoded))
		default:
			// Fallback: return allowance (backward compat for non-Moonwell tests).
			encoded, _ := plannerERC20ABI.Methods["allowance"].Outputs.Pack(allowance)
			writePlannerRPCResult(w, req.ID, "0x"+hex.EncodeToString(encoded))
		}
	}))
}

func TestBuildMoonwellSupplyWithExplicitMToken(t *testing.T) {
	rpc := newMoonwellPlannerRPCServer(t, big.NewInt(0), false)
	defer rpc.Close()

	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}
	asset := id.Asset{Address: testUnderlying, AssetID: "eip155:8453/erc20:" + testUnderlying}

	action, err := BuildMoonwellLendAction(context.Background(), MoonwellLendRequest{
		Verb:            AaveVerbSupply,
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000000",
		Sender:          testSender,
		Recipient:       testRecipient,
		Simulate:        true,
		RPCURL:          rpc.URL,
		MTokenAddress:   testMToken,
	})
	if err != nil {
		t.Fatalf("BuildMoonwellLendAction supply failed: %v", err)
	}
	if action.IntentType != "lend_supply" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
	}
	if action.Provider != "moonwell" {
		t.Fatalf("unexpected provider: %s", action.Provider)
	}
	// Should have approval + enterMarkets + supply steps.
	if len(action.Steps) != 3 {
		t.Fatalf("expected 3 steps (approval + enterMarkets + supply), got %d", len(action.Steps))
	}
	if action.Steps[0].Type != "approval" {
		t.Fatalf("expected first step approval, got %s", action.Steps[0].Type)
	}
	if action.Steps[1].StepID != "moonwell-enter-market" {
		t.Fatalf("expected second step moonwell-enter-market, got %s", action.Steps[1].StepID)
	}
	if action.Steps[2].Type != "lend_call" {
		t.Fatalf("expected third step lend_call, got %s", action.Steps[2].Type)
	}
	if !strings.EqualFold(action.Steps[2].Target, testMToken) {
		t.Fatalf("unexpected lend target: %s", action.Steps[2].Target)
	}
	if action.Steps[2].StepID != "moonwell-supply" {
		t.Fatalf("unexpected step ID: %s", action.Steps[2].StepID)
	}
}

func TestBuildMoonwellSupplySkipsApprovalWhenSufficient(t *testing.T) {
	// Allowance already large enough + already entered — should skip both.
	rpc := newMoonwellPlannerRPCServer(t, new(big.Int).SetUint64(10_000_000), true)
	defer rpc.Close()

	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}
	asset := id.Asset{Address: testUnderlying, AssetID: "eip155:8453/erc20:" + testUnderlying}

	action, err := BuildMoonwellLendAction(context.Background(), MoonwellLendRequest{
		Verb:            AaveVerbSupply,
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000000",
		Sender:          testSender,
		Simulate:        true,
		RPCURL:          rpc.URL,
		MTokenAddress:   testMToken,
	})
	if err != nil {
		t.Fatalf("BuildMoonwellLendAction failed: %v", err)
	}
	if len(action.Steps) != 1 {
		t.Fatalf("expected 1 step (supply only, no approval or enterMarkets), got %d", len(action.Steps))
	}
	if action.Steps[0].StepID != "moonwell-supply" {
		t.Fatalf("unexpected step ID: %s", action.Steps[0].StepID)
	}
}

func TestBuildMoonwellWithdraw(t *testing.T) {
	rpc := newPlannerRPCServer(t, big.NewInt(0))
	defer rpc.Close()

	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}
	asset := id.Asset{Address: testUnderlying, AssetID: "eip155:8453/erc20:" + testUnderlying}

	action, err := BuildMoonwellLendAction(context.Background(), MoonwellLendRequest{
		Verb:            AaveVerbWithdraw,
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "500000",
		Sender:          testSender,
		Simulate:        true,
		RPCURL:          rpc.URL,
		MTokenAddress:   testMToken,
	})
	if err != nil {
		t.Fatalf("BuildMoonwellLendAction withdraw failed: %v", err)
	}
	if action.IntentType != "lend_withdraw" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
	}
	// Withdraw has no approval step.
	if len(action.Steps) != 1 {
		t.Fatalf("expected 1 step (withdraw only), got %d", len(action.Steps))
	}
	if action.Steps[0].StepID != "moonwell-withdraw" {
		t.Fatalf("unexpected step ID: %s", action.Steps[0].StepID)
	}
	if !strings.EqualFold(action.Steps[0].Target, testMToken) {
		t.Fatalf("unexpected target: %s", action.Steps[0].Target)
	}
}

func TestBuildMoonwellBorrow(t *testing.T) {
	rpc := newPlannerRPCServer(t, big.NewInt(0))
	defer rpc.Close()

	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}
	asset := id.Asset{Address: testUnderlying, AssetID: "eip155:8453/erc20:" + testUnderlying}

	action, err := BuildMoonwellLendAction(context.Background(), MoonwellLendRequest{
		Verb:            AaveVerbBorrow,
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "250000",
		Sender:          testSender,
		Simulate:        true,
		RPCURL:          rpc.URL,
		MTokenAddress:   testMToken,
	})
	if err != nil {
		t.Fatalf("BuildMoonwellLendAction borrow failed: %v", err)
	}
	if action.IntentType != "lend_borrow" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
	}
	// Borrow has no approval step.
	if len(action.Steps) != 1 {
		t.Fatalf("expected 1 step (borrow only), got %d", len(action.Steps))
	}
	if action.Steps[0].StepID != "moonwell-borrow" {
		t.Fatalf("unexpected step ID: %s", action.Steps[0].StepID)
	}
}

func TestBuildMoonwellRepay(t *testing.T) {
	rpc := newPlannerRPCServer(t, big.NewInt(0))
	defer rpc.Close()

	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}
	asset := id.Asset{Address: testUnderlying, AssetID: "eip155:8453/erc20:" + testUnderlying}

	action, err := BuildMoonwellLendAction(context.Background(), MoonwellLendRequest{
		Verb:            AaveVerbRepay,
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "750000",
		Sender:          testSender,
		Recipient:       testRecipient,
		Simulate:        true,
		RPCURL:          rpc.URL,
		MTokenAddress:   testMToken,
	})
	if err != nil {
		t.Fatalf("BuildMoonwellLendAction repay failed: %v", err)
	}
	if action.IntentType != "lend_repay" {
		t.Fatalf("unexpected intent type: %s", action.IntentType)
	}
	// Repay has approval + repay steps.
	if len(action.Steps) != 2 {
		t.Fatalf("expected 2 steps (approval + repay), got %d", len(action.Steps))
	}
	if action.Steps[0].Type != "approval" {
		t.Fatalf("expected first step approval, got %s", action.Steps[0].Type)
	}
	if action.Steps[1].StepID != "moonwell-repay" {
		t.Fatalf("unexpected step ID: %s", action.Steps[1].StepID)
	}
}

func TestBuildMoonwellRequiresSender(t *testing.T) {
	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}
	asset := id.Asset{Address: testUnderlying, AssetID: "eip155:8453/erc20:" + testUnderlying}

	_, err := BuildMoonwellLendAction(context.Background(), MoonwellLendRequest{
		Verb:            AaveVerbSupply,
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000000",
		MTokenAddress:   testMToken,
	})
	if err == nil {
		t.Fatal("expected missing sender error")
	}
}

func TestBuildMoonwellRejectsInvalidAmount(t *testing.T) {
	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}
	asset := id.Asset{Address: testUnderlying, AssetID: "eip155:8453/erc20:" + testUnderlying}

	_, err := BuildMoonwellLendAction(context.Background(), MoonwellLendRequest{
		Verb:            AaveVerbSupply,
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "0",
		Sender:          testSender,
		MTokenAddress:   testMToken,
	})
	if err == nil {
		t.Fatal("expected invalid amount error")
	}
}

func TestBuildMoonwellRejectsUnsupportedVerb(t *testing.T) {
	rpc := newPlannerRPCServer(t, big.NewInt(0))
	defer rpc.Close()

	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}
	asset := id.Asset{Address: testUnderlying, AssetID: "eip155:8453/erc20:" + testUnderlying}

	_, err := BuildMoonwellLendAction(context.Background(), MoonwellLendRequest{
		Verb:            AaveLendVerb("invalid"),
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000000",
		Sender:          testSender,
		RPCURL:          rpc.URL,
		MTokenAddress:   testMToken,
	})
	if err == nil {
		t.Fatal("expected unsupported verb error")
	}
}

func TestResolveMoonwellMTokenExplicit(t *testing.T) {
	addr, err := resolveMoonwellMToken(context.Background(), nil, id.Chain{EVMChainID: 8453}, testMToken, common.Address{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !strings.EqualFold(addr.Hex(), testMToken) {
		t.Fatalf("unexpected address: %s", addr.Hex())
	}
}

func TestResolveMoonwellMTokenInvalidExplicit(t *testing.T) {
	_, err := resolveMoonwellMToken(context.Background(), nil, id.Chain{EVMChainID: 8453}, "not-hex", common.Address{})
	if err == nil {
		t.Fatal("expected invalid address error")
	}
}

func TestResolveMoonwellMTokenAutoResolve(t *testing.T) {
	mTokenAddr := common.HexToAddress(testMToken)
	underlyingAddr := common.HexToAddress(testUnderlying)

	getAllMarketsSel := hex.EncodeToString(moonwellComptrollerABI.Methods["getAllMarkets"].ID)
	underlyingSel := hex.EncodeToString(moonwellMTokenABI.Methods["underlying"].ID)
	mc3Sel := hex.EncodeToString(plannerMC3ABI.Methods["aggregate3"].ID)

	// dispatchSingle handles an individual contract call and returns the hex-encoded result.
	dispatchSingle := func(selector string) string {
		switch selector {
		case getAllMarketsSel:
			encoded, _ := moonwellComptrollerABI.Methods["getAllMarkets"].Outputs.Pack([]common.Address{mTokenAddr})
			return hex.EncodeToString(encoded)
		case underlyingSel:
			encoded, _ := moonwellMTokenABI.Methods["underlying"].Outputs.Pack(underlyingAddr)
			return hex.EncodeToString(encoded)
		default:
			return ""
		}
	}

	// Build a mock RPC that handles getAllMarkets + aggregate3 (batched underlying).
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer r.Body.Close()
		var req plannerRPCRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		if req.Method != "eth_call" {
			writePlannerRPCError(w, req.ID, -32601, "unsupported")
			return
		}
		var callObj struct {
			To    string `json:"to"`
			Data  string `json:"data"`
			Input string `json:"input"`
		}
		if err := json.Unmarshal(req.Params[0], &callObj); err != nil {
			writePlannerRPCError(w, req.ID, -32602, "bad params")
			return
		}
		rawData := callObj.Data
		if rawData == "" {
			rawData = callObj.Input
		}
		data, _ := hex.DecodeString(strings.TrimPrefix(rawData, "0x"))
		if len(data) < 4 {
			writePlannerRPCError(w, req.ID, -32602, "data too short")
			return
		}
		selector := hex.EncodeToString(data[:4])

		// Handle Multicall3 aggregate3.
		if strings.EqualFold(callObj.To, plannerMC3Addr.Hex()) && selector == mc3Sel {
			decoded, err := plannerMC3ABI.Methods["aggregate3"].Inputs.Unpack(data[4:])
			if err != nil {
				writePlannerRPCError(w, req.ID, -32602, "unpack aggregate3")
				return
			}
			subcalls := decoded[0].([]struct {
				Target       common.Address `json:"target"`
				AllowFailure bool           `json:"allowFailure"`
				CallData     []byte         `json:"callData"`
			})
			type mc3Res struct {
				Success    bool
				ReturnData []byte
			}
			results := make([]mc3Res, len(subcalls))
			for i, sc := range subcalls {
				if len(sc.CallData) < 4 {
					results[i] = mc3Res{Success: false}
					continue
				}
				subSel := hex.EncodeToString(sc.CallData[:4])
				resHex := dispatchSingle(subSel)
				if resHex == "" {
					results[i] = mc3Res{Success: false}
				} else {
					resBytes, _ := hex.DecodeString(resHex)
					results[i] = mc3Res{Success: true, ReturnData: resBytes}
				}
			}
			encoded, _ := plannerMC3ABI.Methods["aggregate3"].Outputs.Pack(results)
			writePlannerRPCResult(w, req.ID, "0x"+hex.EncodeToString(encoded))
			return
		}

		// Handle direct calls (getAllMarkets).
		resHex := dispatchSingle(selector)
		if resHex != "" {
			writePlannerRPCResult(w, req.ID, "0x"+resHex)
		} else {
			writePlannerRPCError(w, req.ID, -32601, fmt.Sprintf("unknown selector: %s", selector))
		}
	}))
	defer srv.Close()

	// Use a chain with known comptroller (Base = 8453).
	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}
	asset := id.Asset{Address: testUnderlying, AssetID: "eip155:8453/erc20:" + testUnderlying}

	action, err := BuildMoonwellLendAction(context.Background(), MoonwellLendRequest{
		Verb:            AaveVerbWithdraw,
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000000",
		Sender:          testSender,
		Simulate:        true,
		RPCURL:          srv.URL,
		// MTokenAddress intentionally empty — triggers auto-resolution.
	})
	if err != nil {
		t.Fatalf("auto-resolve failed: %v", err)
	}
	if !strings.EqualFold(action.Steps[0].Target, testMToken) {
		t.Fatalf("unexpected target after auto-resolve: %s", action.Steps[0].Target)
	}
}

func TestResolveMoonwellMTokenUnsupportedChain(t *testing.T) {
	// Chain 999 has no comptroller entry.
	_, err := resolveMoonwellMToken(context.Background(), nil, id.Chain{EVMChainID: 999}, "", common.Address{})
	if err == nil {
		t.Fatal("expected unsupported chain error")
	}
	if !strings.Contains(err.Error(), "not supported") {
		t.Fatalf("unexpected error: %v", err)
	}
}
