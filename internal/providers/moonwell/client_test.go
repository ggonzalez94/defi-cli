package moonwell

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"math/big"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

// ── Test RPC helpers ────────────────────────────────────────────────────

type jsonRPCRequest struct {
	JSONRPC string        `json:"jsonrpc"`
	Method  string        `json:"method"`
	Params  []interface{} `json:"params"`
	ID      interface{}   `json:"id"`
}

type jsonRPCResponse struct {
	JSONRPC string      `json:"jsonrpc"`
	ID      interface{} `json:"id"`
	Result  string      `json:"result,omitempty"`
	Error   interface{} `json:"error,omitempty"`
}

func selectorHex(a abi.ABI, method string) string {
	m, ok := a.Methods[method]
	if !ok {
		return ""
	}
	return hex.EncodeToString(m.ID)
}

func packOutput(sig string, vals ...interface{}) string {
	a, _ := abi.JSON(strings.NewReader(sig))
	out, _ := a.Methods["f"].Outputs.Pack(vals...)
	return "0x" + hex.EncodeToString(out)
}

func encodeAddresses(addrs []common.Address) string {
	return packOutput(`[{"name":"f","type":"function","outputs":[{"type":"address[]"}]}]`, addrs)
}

func encodeAddress(addr common.Address) string {
	return packOutput(`[{"name":"f","type":"function","outputs":[{"type":"address"}]}]`, addr)
}

func encodeString(s string) string {
	return packOutput(`[{"name":"f","type":"function","outputs":[{"type":"string"}]}]`, s)
}

func encodeUint8(v uint8) string {
	return packOutput(`[{"name":"f","type":"function","outputs":[{"type":"uint8"}]}]`, v)
}

func encodeUint256(v *big.Int) string {
	return packOutput(`[{"name":"f","type":"function","outputs":[{"type":"uint256"}]}]`, v)
}

func encodeSnapshot(errCode, mTokenBal, borrowBal, exchangeRate *big.Int) string {
	return packOutput(
		`[{"name":"f","type":"function","outputs":[{"type":"uint256"},{"type":"uint256"},{"type":"uint256"},{"type":"uint256"}]}]`,
		errCode, mTokenBal, borrowBal, exchangeRate,
	)
}

// Test addresses.
var (
	testComptroller = common.HexToAddress("0xfBb21d0380beE3312B33c4353c8936a0F13EF26C")
	testOracle      = common.HexToAddress("0xEC942bE8A8114bFD0396A5052c36027f2cA6a9d0")
	testMTokenUSDC  = common.HexToAddress("0xEdc817A28E8B93B03976FBd4a3dDBc9f7D176c22")
	testUSDC        = common.HexToAddress("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913")
	testAccount     = common.HexToAddress("0x000000000000000000000000000000000000dEaD")
)

// dispatchSingleCall resolves a single eth_call given the target and calldata.
// Returns the hex-encoded result or "0x" on unknown.
func dispatchSingleCall(to string, dataHex string, cSel, mSel, eSel map[string]string, oSel string,
	supplyRate, borrowRate, totalSupply, exchangeRate, totalBorrows, cash, price, mTokenBal, borrowBal *big.Int) string {

	selector := ""
	if len(dataHex) >= 8 {
		selector = dataHex[:8]
	}
	to = strings.ToLower(to)

	switch {
	case to == strings.ToLower(testComptroller.Hex()):
		switch selector {
		case cSel["getAllMarkets"]:
			return encodeAddresses([]common.Address{testMTokenUSDC})
		case cSel["oracle"]:
			return encodeAddress(testOracle)
		case cSel["getAssetsIn"]:
			return encodeAddresses([]common.Address{testMTokenUSDC})
		}
	case to == strings.ToLower(testOracle.Hex()):
		if selector == oSel {
			return encodeUint256(price)
		}
	case to == strings.ToLower(testMTokenUSDC.Hex()):
		switch selector {
		case mSel["underlying"]:
			return encodeAddress(testUSDC)
		case mSel["supplyRatePerTimestamp"]:
			return encodeUint256(supplyRate)
		case mSel["borrowRatePerTimestamp"]:
			return encodeUint256(borrowRate)
		case mSel["totalSupply"]:
			return encodeUint256(totalSupply)
		case mSel["exchangeRateCurrent"]:
			return encodeUint256(exchangeRate)
		case mSel["totalBorrowsCurrent"]:
			return encodeUint256(totalBorrows)
		case mSel["getCash"]:
			return encodeUint256(cash)
		case mSel["getAccountSnapshot"]:
			return encodeSnapshot(big.NewInt(0), mTokenBal, borrowBal, exchangeRate)
		}
	case to == strings.ToLower(testUSDC.Hex()):
		switch selector {
		case eSel["symbol"]:
			return encodeString("USDC")
		case eSel["decimals"]:
			return encodeUint8(6)
		}
	}
	return "0x"
}

func newTestRPCServer(t *testing.T) *httptest.Server {
	t.Helper()

	cSel := map[string]string{
		"getAllMarkets": selectorHex(comptrollerABI, "getAllMarkets"),
		"oracle":       selectorHex(comptrollerABI, "oracle"),
		"getAssetsIn":  selectorHex(comptrollerABI, "getAssetsIn"),
	}
	mSel := map[string]string{
		"underlying":             selectorHex(mTokenABI, "underlying"),
		"supplyRatePerTimestamp": selectorHex(mTokenABI, "supplyRatePerTimestamp"),
		"borrowRatePerTimestamp": selectorHex(mTokenABI, "borrowRatePerTimestamp"),
		"totalSupply":            selectorHex(mTokenABI, "totalSupply"),
		"exchangeRateCurrent":    selectorHex(mTokenABI, "exchangeRateCurrent"),
		"totalBorrowsCurrent":    selectorHex(mTokenABI, "totalBorrowsCurrent"),
		"getCash":                selectorHex(mTokenABI, "getCash"),
		"getAccountSnapshot":     selectorHex(mTokenABI, "getAccountSnapshot"),
	}
	eSel := map[string]string{
		"symbol":   selectorHex(erc20ABI, "symbol"),
		"decimals": selectorHex(erc20ABI, "decimals"),
	}
	oABI, _ := abi.JSON(strings.NewReader(registry.MoonwellOracleABI))
	oSel := selectorHex(oABI, "getUnderlyingPrice")
	mc3Sel := selectorHex(mc3ABI, "aggregate3")

	supplyRate := big.NewInt(951293759)
	borrowRate := big.NewInt(1585489599)
	totalSupply := new(big.Int).Mul(big.NewInt(100_000_000), big.NewInt(1e8))
	exchangeRate := big.NewInt(2e14)
	totalBorrows := new(big.Int).Mul(big.NewInt(500_000), big.NewInt(1e6))
	cash := new(big.Int).Mul(big.NewInt(500_000), big.NewInt(1e6))
	price := new(big.Int).Exp(big.NewInt(10), big.NewInt(30), nil)
	mTokenBal := new(big.Int).Mul(big.NewInt(10_000), big.NewInt(1e8))
	borrowBal := new(big.Int).Mul(big.NewInt(1_000), big.NewInt(1e6))

	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req jsonRPCRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, "bad request", 400)
			return
		}

		if req.Method != "eth_call" {
			json.NewEncoder(w).Encode(jsonRPCResponse{JSONRPC: "2.0", ID: req.ID, Result: "0x"})
			return
		}

		params, ok := req.Params[0].(map[string]interface{})
		if !ok {
			json.NewEncoder(w).Encode(jsonRPCResponse{JSONRPC: "2.0", ID: req.ID, Result: "0x"})
			return
		}
		dataHex, _ := params["data"].(string)
		if dataHex == "" {
			dataHex, _ = params["input"].(string)
		}
		toHex, _ := params["to"].(string)
		dataHex = strings.TrimPrefix(dataHex, "0x")
		selector := ""
		if len(dataHex) >= 8 {
			selector = dataHex[:8]
		}
		to := strings.ToLower(toHex)

		// Handle Multicall3.aggregate3 — decode Call3[], dispatch each, re-encode Result[].
		if to == strings.ToLower(multicall3Addr.Hex()) && selector == mc3Sel {
			rawData, _ := hex.DecodeString(dataHex)
			decoded, err := mc3ABI.Methods["aggregate3"].Inputs.Unpack(rawData[4:])
			if err != nil {
				t.Logf("aggregate3 unpack error: %v", err)
				json.NewEncoder(w).Encode(jsonRPCResponse{JSONRPC: "2.0", ID: req.ID, Result: "0x"})
				return
			}
			calls := decoded[0].([]struct {
				Target       common.Address `json:"target"`
				AllowFailure bool           `json:"allowFailure"`
				CallData     []byte         `json:"callData"`
			})

			type mc3Result struct {
				Success    bool
				ReturnData []byte
			}
			results := make([]mc3Result, len(calls))
			for i, call := range calls {
				subData := hex.EncodeToString(call.CallData)
				subResult := dispatchSingleCall(call.Target.Hex(), subData, cSel, mSel, eSel, oSel,
					supplyRate, borrowRate, totalSupply, exchangeRate, totalBorrows, cash, price, mTokenBal, borrowBal)
				subBytes, _ := hex.DecodeString(strings.TrimPrefix(subResult, "0x"))
				results[i] = mc3Result{Success: subResult != "0x", ReturnData: subBytes}
			}

			// Encode as aggregate3 output: tuple[](bool success, bytes returnData)
			encoded, err := mc3ABI.Methods["aggregate3"].Outputs.Pack(results)
			if err != nil {
				t.Logf("aggregate3 pack error: %v", err)
				json.NewEncoder(w).Encode(jsonRPCResponse{JSONRPC: "2.0", ID: req.ID, Result: "0x"})
				return
			}
			json.NewEncoder(w).Encode(jsonRPCResponse{JSONRPC: "2.0", ID: req.ID, Result: "0x" + hex.EncodeToString(encoded)})
			return
		}

		// Handle direct (non-multicall) calls.
		result := dispatchSingleCall(to, dataHex, cSel, mSel, eSel, oSel,
			supplyRate, borrowRate, totalSupply, exchangeRate, totalBorrows, cash, price, mTokenBal, borrowBal)
		json.NewEncoder(w).Encode(jsonRPCResponse{JSONRPC: "2.0", ID: req.ID, Result: result})
	}))
}

// ── Tests ───────────────────────────────────────────────────────────────

func TestLendMarketsAndYield(t *testing.T) {
	srv := newTestRPCServer(t)
	defer srv.Close()

	client := New()
	client.rpcOverride = srv.URL
	client.now = func() time.Time { return time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC) }

	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}
	asset := id.Asset{Symbol: "USDC", ChainID: "eip155:8453"}

	markets, err := client.LendMarkets(context.Background(), "moonwell", chain, asset)
	if err != nil {
		t.Fatalf("LendMarkets failed: %v", err)
	}
	if len(markets) != 1 {
		t.Fatalf("expected 1 market, got %d", len(markets))
	}
	if markets[0].Provider != "moonwell" || markets[0].Protocol != "moonwell" {
		t.Fatalf("expected moonwell provider, got %+v", markets[0])
	}
	if markets[0].ProviderNativeID == "" || markets[0].ProviderNativeIDKind != model.NativeIDKindCompositeMarketAsset {
		t.Fatalf("expected provider native id metadata, got %+v", markets[0])
	}
	if markets[0].SupplyAPY <= 0 {
		t.Fatalf("expected positive supply APY, got %f", markets[0].SupplyAPY)
	}
	if markets[0].BorrowAPY <= 0 {
		t.Fatalf("expected positive borrow APY, got %f", markets[0].BorrowAPY)
	}
	if markets[0].TVLUSD <= 0 {
		t.Fatalf("expected positive TVL, got %f", markets[0].TVLUSD)
	}

	// Rates
	rates, err := client.LendRates(context.Background(), "moonwell", chain, asset)
	if err != nil {
		t.Fatalf("LendRates failed: %v", err)
	}
	if len(rates) != 1 || rates[0].Utilization <= 0 {
		t.Fatalf("expected 1 rate with positive utilization, got %+v", rates)
	}

	// Yield opportunities
	opps, err := client.YieldOpportunities(context.Background(), providers.YieldRequest{Chain: chain, Asset: asset, Limit: 10})
	if err != nil {
		t.Fatalf("YieldOpportunities failed: %v", err)
	}
	if len(opps) != 1 || opps[0].Provider != "moonwell" {
		t.Fatalf("unexpected yield response: %+v", opps)
	}
	if opps[0].Type != "lend" || opps[0].WithdrawalTerms != "variable" {
		t.Fatalf("unexpected yield type/terms: %+v", opps[0])
	}
	if len(opps[0].BackingAssets) != 1 || opps[0].BackingAssets[0].SharePct != 100 || opps[0].BackingAssets[0].Symbol != "USDC" {
		t.Fatalf("unexpected backing assets: %+v", opps[0].BackingAssets)
	}
}

func TestLendPositions(t *testing.T) {
	srv := newTestRPCServer(t)
	defer srv.Close()

	client := New()
	client.rpcOverride = srv.URL
	client.now = func() time.Time { return time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC) }

	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}

	positions, err := client.LendPositions(context.Background(), providers.LendPositionsRequest{
		Chain:        chain,
		Account:      testAccount.Hex(),
		PositionType: providers.LendPositionTypeAll,
	})
	if err != nil {
		t.Fatalf("LendPositions failed: %v", err)
	}
	if len(positions) != 2 {
		t.Fatalf("expected 2 positions (collateral + borrow), got %d: %+v", len(positions), positions)
	}

	var hasCollateral, hasBorrow bool
	for _, p := range positions {
		if p.PositionType == string(providers.LendPositionTypeCollateral) {
			hasCollateral = true
			if p.Provider != "moonwell" {
				t.Fatalf("expected moonwell provider, got %+v", p)
			}
			if p.AmountUSD <= 0 {
				t.Fatalf("expected positive supply USD, got %f", p.AmountUSD)
			}
		}
		if p.PositionType == string(providers.LendPositionTypeBorrow) {
			hasBorrow = true
			if p.AmountUSD <= 0 {
				t.Fatalf("expected positive borrow USD, got %f", p.AmountUSD)
			}
		}
	}
	if !hasCollateral || !hasBorrow {
		t.Fatalf("expected both collateral and borrow, got %+v", positions)
	}
}

func TestLendPositionsFiltering(t *testing.T) {
	srv := newTestRPCServer(t)
	defer srv.Close()

	client := New()
	client.rpcOverride = srv.URL
	client.now = func() time.Time { return time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC) }

	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}

	collateral, err := client.LendPositions(context.Background(), providers.LendPositionsRequest{
		Chain: chain, Account: testAccount.Hex(), PositionType: providers.LendPositionTypeCollateral,
	})
	if err != nil {
		t.Fatalf("failed: %v", err)
	}
	if len(collateral) != 1 || collateral[0].PositionType != string(providers.LendPositionTypeCollateral) {
		t.Fatalf("expected 1 collateral, got %+v", collateral)
	}

	borrows, err := client.LendPositions(context.Background(), providers.LendPositionsRequest{
		Chain: chain, Account: testAccount.Hex(), PositionType: providers.LendPositionTypeBorrow,
	})
	if err != nil {
		t.Fatalf("failed: %v", err)
	}
	if len(borrows) != 1 || borrows[0].PositionType != string(providers.LendPositionTypeBorrow) {
		t.Fatalf("expected 1 borrow, got %+v", borrows)
	}
}

func TestYieldPositions(t *testing.T) {
	srv := newTestRPCServer(t)
	defer srv.Close()

	client := New()
	client.rpcOverride = srv.URL
	client.now = func() time.Time { return time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC) }

	chain := id.Chain{CAIP2: "eip155:8453", EVMChainID: 8453}

	positions, err := client.YieldPositions(context.Background(), providers.YieldPositionsRequest{
		Chain: chain, Account: testAccount.Hex(),
	})
	if err != nil {
		t.Fatalf("YieldPositions failed: %v", err)
	}
	if len(positions) != 1 {
		t.Fatalf("expected 1 yield position, got %d", len(positions))
	}
	if positions[0].PositionType != "deposit" || positions[0].Provider != "moonwell" {
		t.Fatalf("unexpected: %+v", positions[0])
	}
}

func TestUnsupportedChain(t *testing.T) {
	client := New()
	chain := id.Chain{CAIP2: "eip155:999", EVMChainID: 999}
	asset := id.Asset{Symbol: "USDC", ChainID: "eip155:999"}

	_, err := client.LendMarkets(context.Background(), "moonwell", chain, asset)
	if err == nil {
		t.Fatalf("expected error for unsupported chain")
	}
}

func TestRateToAPY(t *testing.T) {
	rate := big.NewInt(951293759) // ~3% APY (rate per second scaled by 1e18)
	apy := rateToAPY(rate)
	if apy < 2.9 || apy > 3.1 {
		t.Fatalf("expected ~3%% APY, got %f", apy)
	}
	if rateToAPY(big.NewInt(0)) != 0 {
		t.Fatalf("expected 0 APY for zero rate")
	}
}

func TestBigIntToFloat(t *testing.T) {
	v := big.NewInt(1_000_000)
	f := bigIntToFloat(v, 6)
	if f != 1.0 {
		t.Fatalf("expected 1.0, got %f", f)
	}
}
