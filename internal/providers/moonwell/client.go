package moonwell

import (
	"context"
	"crypto/sha1"
	"encoding/hex"
	"fmt"
	"math"
	"math/big"
	"sort"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
	"github.com/ggonzalez94/defi-cli/internal/providers/yieldutil"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

const secondsPerYear = 365.25 * 24 * 3600

// Multicall3 is deployed at a standard address on all major EVM chains.
var multicall3Addr = common.HexToAddress("0xcA11bde05977b3631167028862bE2a173976CA11")

// multicall3Call matches Multicall3.Call3 struct.
type multicall3Call struct {
	Target       common.Address
	AllowFailure bool
	CallData     []byte
}

// multicall3Result matches Multicall3.Result struct.
type multicall3Result struct {
	Success    bool
	ReturnData []byte
}

type Client struct {
	now         func() time.Time
	rpcOverride string // used in tests to point at a mock RPC server
}

func New() *Client {
	return &Client{now: time.Now}
}

func (c *Client) Info() model.ProviderInfo {
	return model.ProviderInfo{
		Name:        "moonwell",
		Type:        "lending+yield",
		RequiresKey: false,
		Capabilities: []string{
			"lend.markets",
			"lend.rates",
			"lend.positions",
			"yield.opportunities",
			"yield.positions",
			"lend.plan",
			"lend.execute",
			"yield.plan",
			"yield.execute",
		},
	}
}

// ── internal market struct ──────────────────────────────────────────────

type moonwellMarket struct {
	MTokenAddress      string
	UnderlyingAddress  string
	UnderlyingSymbol   string
	UnderlyingDecimals int
	SupplyAPY          float64 // percentage points
	BorrowAPY          float64
	TVLUSD             float64
	TotalBorrowsUSD    float64
	LiquidityUSD       float64
	Utilization        float64
}

// ── LendingProvider ─────────────────────────────────────────────────────

func (c *Client) LendMarkets(ctx context.Context, provider string, chain id.Chain, asset id.Asset) ([]model.LendMarket, error) {
	if !chain.IsEVM() {
		return nil, clierr.New(clierr.CodeUnsupported, "moonwell supports only EVM chains")
	}
	markets, comptroller, err := c.fetchMarkets(ctx, chain, c.rpcOverride)
	if err != nil {
		return nil, err
	}
	_ = comptroller

	out := make([]model.LendMarket, 0, len(markets))
	for _, m := range markets {
		if !matchesAsset(m.UnderlyingAddress, m.UnderlyingSymbol, asset) {
			continue
		}
		assetID := canonicalAssetIDForChain(chain.CAIP2, m.UnderlyingAddress)
		if assetID == "" {
			continue
		}
		nativeID := providerNativeID("moonwell", chain.CAIP2, comptroller, m.UnderlyingAddress)
		out = append(out, model.LendMarket{
			Protocol:             "moonwell",
			Provider:             "moonwell",
			ChainID:              chain.CAIP2,
			AssetID:              assetID,
			ProviderNativeID:     nativeID,
			ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
			SupplyAPY:            m.SupplyAPY,
			BorrowAPY:            m.BorrowAPY,
			TVLUSD:               m.TVLUSD,
			LiquidityUSD:         m.LiquidityUSD,
			SourceURL:            "https://moonwell.fi",
			FetchedAt:            c.now().UTC().Format(time.RFC3339),
		})
	}

	sort.Slice(out, func(i, j int) bool {
		if out[i].TVLUSD != out[j].TVLUSD {
			return out[i].TVLUSD > out[j].TVLUSD
		}
		return out[i].AssetID < out[j].AssetID
	})
	return out, nil
}

func (c *Client) LendRates(ctx context.Context, provider string, chain id.Chain, asset id.Asset) ([]model.LendRate, error) {
	if !chain.IsEVM() {
		return nil, clierr.New(clierr.CodeUnsupported, "moonwell supports only EVM chains")
	}
	markets, comptroller, err := c.fetchMarkets(ctx, chain, c.rpcOverride)
	if err != nil {
		return nil, err
	}

	out := make([]model.LendRate, 0, len(markets))
	for _, m := range markets {
		if !matchesAsset(m.UnderlyingAddress, m.UnderlyingSymbol, asset) {
			continue
		}
		assetID := canonicalAssetIDForChain(chain.CAIP2, m.UnderlyingAddress)
		if assetID == "" {
			continue
		}
		nativeID := providerNativeID("moonwell", chain.CAIP2, comptroller, m.UnderlyingAddress)
		out = append(out, model.LendRate{
			Protocol:             "moonwell",
			Provider:             "moonwell",
			ChainID:              chain.CAIP2,
			AssetID:              assetID,
			ProviderNativeID:     nativeID,
			ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
			SupplyAPY:            m.SupplyAPY,
			BorrowAPY:            m.BorrowAPY,
			Utilization:          m.Utilization,
			SourceURL:            "https://moonwell.fi",
			FetchedAt:            c.now().UTC().Format(time.RFC3339),
		})
	}

	sort.Slice(out, func(i, j int) bool {
		if out[i].SupplyAPY != out[j].SupplyAPY {
			return out[i].SupplyAPY > out[j].SupplyAPY
		}
		return out[i].AssetID < out[j].AssetID
	})
	return out, nil
}

// ── LendingPositionsProvider ────────────────────────────────────────────

func (c *Client) LendPositions(ctx context.Context, req providers.LendPositionsRequest) ([]model.LendPosition, error) {
	if !req.Chain.IsEVM() {
		return nil, clierr.New(clierr.CodeUnsupported, "moonwell supports only EVM chains")
	}
	account := normalizeEVMAddress(req.Account)
	if account == "" {
		return nil, clierr.New(clierr.CodeUsage, "lend positions requires a valid EVM address")
	}

	rpcOverride := c.rpcOverride
	if req.RPCURL != "" {
		rpcOverride = req.RPCURL
	}
	rpcURL, err := registry.ResolveRPCURL(rpcOverride, req.Chain.EVMChainID)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeUnsupported, "resolve rpc url", err)
	}
	comptrollerAddr, ok := registry.MoonwellComptroller(req.Chain.EVMChainID)
	if !ok {
		return nil, clierr.New(clierr.CodeUnsupported, "moonwell is not supported on this chain")
	}

	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
	}
	defer client.Close()

	comptroller := common.HexToAddress(comptrollerAddr)
	accountAddr := common.HexToAddress(account)

	// Get all markets + collateral set + oracle (3 sequential RPC calls).
	allMarkets, err := callGetAllMarkets(ctx, client, comptroller)
	if err != nil {
		return nil, err
	}
	collateralSet, err := callGetAssetsIn(ctx, client, comptroller, accountAddr)
	if err != nil {
		return nil, err
	}
	oracleAddr, err := callOracle(ctx, client, comptroller)
	if err != nil {
		return nil, err
	}

	// Batch all per-market calls via multicall:
	// Per mToken: getAccountSnapshot, underlying, supplyRate, borrowRate, getUnderlyingPrice
	// Per underlying: symbol, decimals (phase 2 after we know underlying addresses)
	const posCallsPerMarket = 5 // snapshot, underlying, supplyRate, borrowRate, price
	snapshotCalls := make([]multicall3Call, 0, len(allMarkets)*posCallsPerMarket)
	underlyingCD, _ := mTokenABI.Pack("underlying")
	supplyRateCD, _ := mTokenABI.Pack("supplyRatePerTimestamp")
	borrowRateCD, _ := mTokenABI.Pack("borrowRatePerTimestamp")

	for _, mt := range allMarkets {
		snapshotCD, _ := mTokenABI.Pack("getAccountSnapshot", accountAddr)
		priceCD, _ := oracleABI.Pack("getUnderlyingPrice", mt)
		snapshotCalls = append(snapshotCalls,
			multicall3Call{Target: mt, AllowFailure: true, CallData: snapshotCD},
			multicall3Call{Target: mt, AllowFailure: true, CallData: underlyingCD},
			multicall3Call{Target: mt, AllowFailure: true, CallData: supplyRateCD},
			multicall3Call{Target: mt, AllowFailure: true, CallData: borrowRateCD},
			multicall3Call{Target: oracleAddr, AllowFailure: true, CallData: priceCD},
		)
	}

	phase1Results, err := execMulticall3(ctx, client, snapshotCalls)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "multicall positions", err)
	}

	// Parse phase 1, collect underlying addresses for phase 2 metadata.
	type posMarket struct {
		mToken       common.Address
		underlying   common.Address
		errCode      *big.Int
		mTokenBal    *big.Int
		borrowBal    *big.Int
		exchangeRate *big.Int
		supplyRate   *big.Int
		borrowRate   *big.Int
		priceMantissa *big.Int
	}
	posMarkets := make([]posMarket, 0)

	for i, mt := range allMarkets {
		base := i * posCallsPerMarket
		r := phase1Results[base : base+posCallsPerMarket]

		// getAccountSnapshot
		if !r[0].Success || len(r[0].ReturnData) < 128 {
			continue
		}
		snapDec, err := mTokenABI.Unpack("getAccountSnapshot", r[0].ReturnData)
		if err != nil || len(snapDec) < 4 {
			continue
		}
		errCode := asBigInt(snapDec[0])
		mTokenBal := asBigInt(snapDec[1])
		borrowBal := asBigInt(snapDec[2])
		exchangeRate := asBigInt(snapDec[3])

		if errCode.Sign() != 0 || (mTokenBal.Sign() == 0 && borrowBal.Sign() == 0) {
			continue
		}

		// underlying
		if !r[1].Success || len(r[1].ReturnData) < 32 {
			continue
		}
		ulDec, err := mTokenABI.Unpack("underlying", r[1].ReturnData)
		if err != nil || len(ulDec) == 0 {
			continue
		}
		underlying, ok := ulDec[0].(common.Address)
		if !ok {
			continue
		}

		posMarkets = append(posMarkets, posMarket{
			mToken:        mt,
			underlying:    underlying,
			errCode:       errCode,
			mTokenBal:     mTokenBal,
			borrowBal:     borrowBal,
			exchangeRate:  exchangeRate,
			supplyRate:    decodeUint256Result(r[2], mTokenABI, "supplyRatePerTimestamp"),
			borrowRate:    decodeUint256Result(r[3], mTokenABI, "borrowRatePerTimestamp"),
			priceMantissa: decodeUint256Result(r[4], oracleABI, "getUnderlyingPrice"),
		})
	}

	if len(posMarkets) == 0 {
		return []model.LendPosition{}, nil
	}

	// Phase 2: get symbol + decimals for each underlying.
	symbolCD, _ := erc20ABI.Pack("symbol")
	decimalsCD, _ := erc20ABI.Pack("decimals")
	phase2Calls := make([]multicall3Call, 0, len(posMarkets)*2)
	for _, pm := range posMarkets {
		phase2Calls = append(phase2Calls,
			multicall3Call{Target: pm.underlying, AllowFailure: true, CallData: symbolCD},
			multicall3Call{Target: pm.underlying, AllowFailure: true, CallData: decimalsCD},
		)
	}

	phase2Results, err := execMulticall3(ctx, client, phase2Calls)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "multicall position metadata", err)
	}

	filterType := providers.LendPositionType(strings.ToLower(strings.TrimSpace(string(req.PositionType))))
	out := make([]model.LendPosition, 0)

	for i, pm := range posMarkets {
		base := i * 2
		var symbol string
		if phase2Results[base].Success && len(phase2Results[base].ReturnData) >= 32 {
			dec, err := erc20ABI.Unpack("symbol", phase2Results[base].ReturnData)
			if err == nil && len(dec) > 0 {
				symbol, _ = dec[0].(string)
			}
		}
		var decimals int
		if phase2Results[base+1].Success && len(phase2Results[base+1].ReturnData) >= 32 {
			dec, err := erc20ABI.Unpack("decimals", phase2Results[base+1].ReturnData)
			if err == nil && len(dec) > 0 {
				d, _ := dec[0].(uint8)
				decimals = int(d)
			}
		}
		if symbol == "" || decimals == 0 {
			continue
		}

		ulAddr := strings.ToLower(pm.underlying.Hex())
		if !matchesAsset(ulAddr, symbol, req.Asset) {
			continue
		}
		assetID := canonicalAssetIDForChain(req.Chain.CAIP2, ulAddr)
		if assetID == "" {
			continue
		}
		nativeID := providerNativeID("moonwell", req.Chain.CAIP2, comptrollerAddr, ulAddr)
		priceUSD := mantissaToUSD(pm.priceMantissa, decimals)

		// Supply position.
		if pm.mTokenBal.Sign() > 0 {
			underlyingBal := new(big.Int).Mul(pm.mTokenBal, pm.exchangeRate)
			underlyingBal.Div(underlyingBal, big.NewInt(1e18))

			posType := providers.LendPositionTypeSupply
			if collateralSet[pm.mToken] {
				posType = providers.LendPositionTypeCollateral
			}
			if matchesPositionType(filterType, posType) {
				amountUSD := bigIntToFloat(underlyingBal, decimals) * priceUSD
				out = append(out, model.LendPosition{
					Protocol:             "moonwell",
					Provider:             "moonwell",
					ChainID:              req.Chain.CAIP2,
					AccountAddress:       account,
					PositionType:         string(posType),
					AssetID:              assetID,
					ProviderNativeID:     nativeID,
					ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
					Amount:               amountInfoFromBigInt(underlyingBal, decimals),
					AmountUSD:            amountUSD,
					APY:                  rateToAPY(pm.supplyRate),
					SourceURL:            "https://moonwell.fi",
					FetchedAt:            c.now().UTC().Format(time.RFC3339),
				})
			}
		}

		// Borrow position.
		if pm.borrowBal.Sign() > 0 && matchesPositionType(filterType, providers.LendPositionTypeBorrow) {
			amountUSD := bigIntToFloat(pm.borrowBal, decimals) * priceUSD
			out = append(out, model.LendPosition{
				Protocol:             "moonwell",
				Provider:             "moonwell",
				ChainID:              req.Chain.CAIP2,
				AccountAddress:       account,
				PositionType:         string(providers.LendPositionTypeBorrow),
				AssetID:              assetID,
				ProviderNativeID:     nativeID,
				ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
				Amount:               amountInfoFromBigInt(pm.borrowBal, decimals),
				AmountUSD:            amountUSD,
				APY:                  rateToAPY(pm.borrowRate),
				SourceURL:            "https://moonwell.fi",
				FetchedAt:            c.now().UTC().Format(time.RFC3339),
			})
		}
	}

	sortLendPositions(out)
	if req.Limit > 0 && len(out) > req.Limit {
		out = out[:req.Limit]
	}
	return out, nil
}

// ── YieldProvider ───────────────────────────────────────────────────────

func (c *Client) YieldOpportunities(ctx context.Context, req providers.YieldRequest) ([]model.YieldOpportunity, error) {
	markets, comptroller, err := c.fetchMarkets(ctx, req.Chain, c.rpcOverride)
	if err != nil {
		return nil, err
	}

	out := make([]model.YieldOpportunity, 0, len(markets))
	for _, m := range markets {
		if !matchesAsset(m.UnderlyingAddress, m.UnderlyingSymbol, req.Asset) {
			continue
		}
		if (m.SupplyAPY == 0 || m.TVLUSD == 0) && !req.IncludeIncomplete {
			continue
		}
		if m.SupplyAPY < req.MinAPY {
			continue
		}
		if m.TVLUSD < req.MinTVLUSD {
			continue
		}

		assetID := canonicalAssetIDForChain(req.Chain.CAIP2, m.UnderlyingAddress)
		if assetID == "" {
			continue
		}
		nativeID := providerNativeID("moonwell", req.Chain.CAIP2, comptroller, m.UnderlyingAddress)
		opportunityID := hashOpportunity("moonwell", req.Chain.CAIP2, nativeID, assetID)

		out = append(out, model.YieldOpportunity{
			OpportunityID:        opportunityID,
			Provider:             "moonwell",
			Protocol:             "moonwell",
			ChainID:              req.Chain.CAIP2,
			AssetID:              assetID,
			ProviderNativeID:     nativeID,
			ProviderNativeIDKind: model.NativeIDKindCompositeMarketAsset,
			Type:                 "lend",
			APYBase:              m.SupplyAPY,
			APYReward:            0,
			APYTotal:             m.SupplyAPY,
			TVLUSD:               m.TVLUSD,
			LiquidityUSD:         m.LiquidityUSD,
			LockupDays:           0,
			WithdrawalTerms:      "variable",
			BackingAssets: []model.YieldBackingAsset{{
				AssetID:  assetID,
				Symbol:   m.UnderlyingSymbol,
				SharePct: 100,
			}},
			SourceURL: "https://moonwell.fi",
			FetchedAt: c.now().UTC().Format(time.RFC3339),
		})
	}

	if len(out) == 0 {
		return nil, clierr.New(clierr.CodeUnavailable, "no moonwell yield opportunities for requested chain/asset")
	}
	yieldutil.Sort(out, req.SortBy)
	if req.Limit <= 0 || req.Limit > len(out) {
		req.Limit = len(out)
	}
	return out[:req.Limit], nil
}

// ── YieldPositionsProvider ──────────────────────────────────────────────

func (c *Client) YieldPositions(ctx context.Context, req providers.YieldPositionsRequest) ([]model.YieldPosition, error) {
	lendRows, err := c.LendPositions(ctx, providers.LendPositionsRequest{
		Chain:        req.Chain,
		Account:      req.Account,
		Asset:        req.Asset,
		PositionType: providers.LendPositionTypeAll,
		Limit:        req.Limit,
		RPCURL:       req.RPCURL,
	})
	if err != nil {
		return nil, err
	}

	out := make([]model.YieldPosition, 0, len(lendRows))
	for _, row := range lendRows {
		switch row.PositionType {
		case string(providers.LendPositionTypeSupply), string(providers.LendPositionTypeCollateral):
		default:
			continue
		}
		opportunityID := ""
		if strings.TrimSpace(row.ProviderNativeID) != "" {
			opportunityID = hashOpportunity("moonwell", row.ChainID, row.ProviderNativeID, row.AssetID)
		}
		out = append(out, model.YieldPosition{
			Protocol:             "moonwell",
			Provider:             "moonwell",
			ChainID:              row.ChainID,
			AccountAddress:       row.AccountAddress,
			PositionType:         "deposit",
			OpportunityID:        opportunityID,
			AssetID:              row.AssetID,
			ProviderNativeID:     row.ProviderNativeID,
			ProviderNativeIDKind: row.ProviderNativeIDKind,
			Amount:               row.Amount,
			AmountUSD:            row.AmountUSD,
			APYTotal:             row.APY,
			SourceURL:            row.SourceURL,
			FetchedAt:            row.FetchedAt,
		})
	}

	sortYieldPositions(out)
	if req.Limit > 0 && len(out) > req.Limit {
		out = out[:req.Limit]
	}
	return out, nil
}

// ── RPC data fetching ───────────────────────────────────────────────────

// callsPerMarketPhase1 is the number of multicall sub-calls per mToken in phase 1.
// Order: underlying, supplyRate, borrowRate, totalSupply, exchangeRate, totalBorrows, getCash, price.
const callsPerMarketPhase1 = 8

// callsPerMarketPhase2 is the number of multicall sub-calls per underlying in phase 2.
// Order: symbol, decimals.
const callsPerMarketPhase2 = 2

func (c *Client) fetchMarkets(ctx context.Context, chain id.Chain, rpcOverride string) ([]moonwellMarket, string, error) {
	if !chain.IsEVM() {
		return nil, "", clierr.New(clierr.CodeUnsupported, "moonwell supports only EVM chains")
	}
	rpcURL, err := registry.ResolveRPCURL(rpcOverride, chain.EVMChainID)
	if err != nil {
		return nil, "", clierr.Wrap(clierr.CodeUnsupported, "resolve rpc url", err)
	}
	comptrollerAddr, ok := registry.MoonwellComptroller(chain.EVMChainID)
	if !ok {
		return nil, "", clierr.New(clierr.CodeUnsupported, "moonwell is not supported on this chain")
	}

	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return nil, "", clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
	}
	defer client.Close()

	comptroller := common.HexToAddress(comptrollerAddr)

	// 1. Get all mToken addresses + oracle (2 RPC calls).
	mTokens, err := callGetAllMarkets(ctx, client, comptroller)
	if err != nil {
		return nil, "", err
	}
	if len(mTokens) == 0 {
		return nil, comptrollerAddr, nil
	}
	oracleAddr, err := callOracle(ctx, client, comptroller)
	if err != nil {
		return nil, "", err
	}

	// 2. Phase 1 multicall: per-mToken data (underlying, rates, supply, exchange, borrows, cash, price).
	phase1Calls := make([]multicall3Call, 0, len(mTokens)*callsPerMarketPhase1)
	underlyingCD, _ := mTokenABI.Pack("underlying")
	supplyRateCD, _ := mTokenABI.Pack("supplyRatePerTimestamp")
	borrowRateCD, _ := mTokenABI.Pack("borrowRatePerTimestamp")
	totalSupplyCD, _ := mTokenABI.Pack("totalSupply")
	exchangeRateCD, _ := mTokenABI.Pack("exchangeRateCurrent")
	totalBorrowsCD, _ := mTokenABI.Pack("totalBorrowsCurrent")
	getCashCD, _ := mTokenABI.Pack("getCash")

	for _, mt := range mTokens {
		priceCD, _ := oracleABI.Pack("getUnderlyingPrice", mt)
		phase1Calls = append(phase1Calls,
			multicall3Call{Target: mt, AllowFailure: true, CallData: underlyingCD},
			multicall3Call{Target: mt, AllowFailure: true, CallData: supplyRateCD},
			multicall3Call{Target: mt, AllowFailure: true, CallData: borrowRateCD},
			multicall3Call{Target: mt, AllowFailure: true, CallData: totalSupplyCD},
			multicall3Call{Target: mt, AllowFailure: true, CallData: exchangeRateCD},
			multicall3Call{Target: mt, AllowFailure: true, CallData: totalBorrowsCD},
			multicall3Call{Target: mt, AllowFailure: true, CallData: getCashCD},
			multicall3Call{Target: oracleAddr, AllowFailure: true, CallData: priceCD},
		)
	}

	phase1Results, err := execMulticall3(ctx, client, phase1Calls)
	if err != nil {
		return nil, "", clierr.Wrap(clierr.CodeUnavailable, "multicall market data", err)
	}

	// Parse phase 1 results and collect underlying addresses for phase 2.
	type phase1Data struct {
		mToken     common.Address
		underlying common.Address
		supplyRate *big.Int
		borrowRate *big.Int
		totalSupply *big.Int
		exchangeRate *big.Int
		totalBorrows *big.Int
		cash         *big.Int
		priceUSD     float64
	}
	p1Parsed := make([]phase1Data, 0, len(mTokens))

	for i, mt := range mTokens {
		base := i * callsPerMarketPhase1
		r := phase1Results[base : base+callsPerMarketPhase1]

		// underlying (required)
		if !r[0].Success || len(r[0].ReturnData) < 32 {
			continue
		}
		decoded, err := mTokenABI.Unpack("underlying", r[0].ReturnData)
		if err != nil || len(decoded) == 0 {
			continue
		}
		underlying, ok := decoded[0].(common.Address)
		if !ok {
			continue
		}

		supplyRate := decodeUint256Result(r[1], mTokenABI, "supplyRatePerTimestamp")
		borrowRate := decodeUint256Result(r[2], mTokenABI, "borrowRatePerTimestamp")
		totalSupply := decodeUint256Result(r[3], mTokenABI, "totalSupply")
		exchangeRate := decodeUint256Result(r[4], mTokenABI, "exchangeRateCurrent")
		totalBorrows := decodeUint256Result(r[5], mTokenABI, "totalBorrowsCurrent")
		cash := decodeUint256Result(r[6], mTokenABI, "getCash")

		var priceUSD float64
		if r[7].Success && len(r[7].ReturnData) >= 32 {
			pDec, pErr := oracleABI.Unpack("getUnderlyingPrice", r[7].ReturnData)
			if pErr == nil && len(pDec) > 0 {
				priceMantissa := asBigInt(pDec[0])
				if priceMantissa.Sign() > 0 {
					// Decimals unknown yet; store raw mantissa temporarily.
					// We'll compute priceUSD after phase 2 when we have decimals.
					priceUSD = -1 // sentinel: raw mantissa stored separately
				}
			}
		}
		_ = priceUSD

		p1Parsed = append(p1Parsed, phase1Data{
			mToken:       mt,
			underlying:   underlying,
			supplyRate:   supplyRate,
			borrowRate:   borrowRate,
			totalSupply:  totalSupply,
			exchangeRate: exchangeRate,
			totalBorrows: totalBorrows,
			cash:         cash,
		})
	}

	// Store raw price mantissas separately — we need decimals from phase 2 to convert.
	priceMantissas := make([]*big.Int, len(p1Parsed))
	for i, p := range p1Parsed {
		idx := -1
		for j, mt := range mTokens {
			if mt == p.mToken {
				idx = j
				break
			}
		}
		if idx < 0 {
			priceMantissas[i] = new(big.Int)
			continue
		}
		r := phase1Results[idx*callsPerMarketPhase1+7]
		if r.Success && len(r.ReturnData) >= 32 {
			dec, err := oracleABI.Unpack("getUnderlyingPrice", r.ReturnData)
			if err == nil && len(dec) > 0 {
				priceMantissas[i] = asBigInt(dec[0])
				continue
			}
		}
		priceMantissas[i] = new(big.Int)
	}

	if len(p1Parsed) == 0 {
		return nil, comptrollerAddr, nil
	}

	// 3. Phase 2 multicall: symbol() + decimals() on each underlying.
	symbolCD, _ := erc20ABI.Pack("symbol")
	decimalsCD, _ := erc20ABI.Pack("decimals")

	phase2Calls := make([]multicall3Call, 0, len(p1Parsed)*callsPerMarketPhase2)
	for _, p := range p1Parsed {
		phase2Calls = append(phase2Calls,
			multicall3Call{Target: p.underlying, AllowFailure: true, CallData: symbolCD},
			multicall3Call{Target: p.underlying, AllowFailure: true, CallData: decimalsCD},
		)
	}

	phase2Results, err := execMulticall3(ctx, client, phase2Calls)
	if err != nil {
		return nil, "", clierr.Wrap(clierr.CodeUnavailable, "multicall token metadata", err)
	}

	// 4. Assemble markets.
	markets := make([]moonwellMarket, 0, len(p1Parsed))
	for i, p := range p1Parsed {
		base := i * callsPerMarketPhase2
		symbolRes := phase2Results[base]
		decimalsRes := phase2Results[base+1]

		var symbol string
		if symbolRes.Success && len(symbolRes.ReturnData) >= 32 {
			dec, err := erc20ABI.Unpack("symbol", symbolRes.ReturnData)
			if err == nil && len(dec) > 0 {
				symbol, _ = dec[0].(string)
			}
		}
		var decimals int
		if decimalsRes.Success && len(decimalsRes.ReturnData) >= 32 {
			dec, err := erc20ABI.Unpack("decimals", decimalsRes.ReturnData)
			if err == nil && len(dec) > 0 {
				d, _ := dec[0].(uint8)
				decimals = int(d)
			}
		}
		if symbol == "" || decimals == 0 {
			continue // can't use markets without metadata
		}

		// Convert price mantissa to USD using decimals.
		priceUSD := mantissaToUSD(priceMantissas[i], decimals)

		// TVL = totalSupply(mTokens) * exchangeRate / 1e18 → underlying units, then * priceUSD
		underlyingTotal := new(big.Int).Mul(p.totalSupply, p.exchangeRate)
		underlyingTotal.Div(underlyingTotal, big.NewInt(1e18))
		tvlUSD := bigIntToFloat(underlyingTotal, decimals) * priceUSD
		totalBorrowsUSD := bigIntToFloat(p.totalBorrows, decimals) * priceUSD
		liquidityUSD := bigIntToFloat(p.cash, decimals) * priceUSD

		var utilization float64
		if tvlUSD > 0 {
			utilization = totalBorrowsUSD / tvlUSD
		}

		markets = append(markets, moonwellMarket{
			MTokenAddress:      strings.ToLower(p.mToken.Hex()),
			UnderlyingAddress:  strings.ToLower(p.underlying.Hex()),
			UnderlyingSymbol:   symbol,
			UnderlyingDecimals: decimals,
			SupplyAPY:          rateToAPY(p.supplyRate),
			BorrowAPY:          rateToAPY(p.borrowRate),
			TVLUSD:             tvlUSD,
			TotalBorrowsUSD:    totalBorrowsUSD,
			LiquidityUSD:       liquidityUSD,
			Utilization:        utilization,
		})
	}

	return markets, comptrollerAddr, nil
}

// decodeUint256Result decodes a single uint256 from a multicall result.
func decodeUint256Result(r multicall3Result, a abi.ABI, method string) *big.Int {
	if !r.Success || len(r.ReturnData) < 32 {
		return new(big.Int)
	}
	dec, err := a.Unpack(method, r.ReturnData)
	if err != nil || len(dec) == 0 {
		return new(big.Int)
	}
	return asBigInt(dec[0])
}

// mantissaToUSD converts an oracle price mantissa to a USD float.
// Moonwell oracle returns price scaled by 10^(36 - underlyingDecimals).
func mantissaToUSD(priceMantissa *big.Int, underlyingDecimals int) float64 {
	if priceMantissa == nil || priceMantissa.Sign() == 0 {
		return 0
	}
	scalePow := 36 - underlyingDecimals
	if scalePow < 0 {
		scalePow = 0
	}
	scale := new(big.Float).SetInt(new(big.Int).Exp(big.NewInt(10), big.NewInt(int64(scalePow)), nil))
	priceFloat := new(big.Float).SetInt(priceMantissa)
	priceFloat.Quo(priceFloat, scale)
	result, _ := priceFloat.Float64()
	return result
}

// execMulticall3 batches multiple contract calls into a single Multicall3.aggregate3 RPC call.
func execMulticall3(ctx context.Context, client *ethclient.Client, calls []multicall3Call) ([]multicall3Result, error) {
	if len(calls) == 0 {
		return nil, nil
	}

	// Build the aggregate3 input as a tuple array.
	type call3Tuple struct {
		Target       common.Address `abi:"target"`
		AllowFailure bool           `abi:"allowFailure"`
		CallData     []byte         `abi:"callData"`
	}
	tuples := make([]call3Tuple, len(calls))
	for i, c := range calls {
		tuples[i] = call3Tuple{Target: c.Target, AllowFailure: c.AllowFailure, CallData: c.CallData}
	}

	data, err := mc3ABI.Pack("aggregate3", tuples)
	if err != nil {
		return nil, fmt.Errorf("pack aggregate3: %w", err)
	}

	mc3 := multicall3Addr
	out, err := client.CallContract(ctx, ethereum.CallMsg{To: &mc3, Data: data}, nil)
	if err != nil {
		return nil, fmt.Errorf("call aggregate3: %w", err)
	}

	decoded, err := mc3ABI.Unpack("aggregate3", out)
	if err != nil {
		return nil, fmt.Errorf("decode aggregate3: %w", err)
	}
	if len(decoded) == 0 {
		return nil, fmt.Errorf("empty aggregate3 response")
	}

	// decoded[0] is []struct{Success bool; ReturnData []byte}
	type resultTuple struct {
		Success    bool   `abi:"success"`
		ReturnData []byte `abi:"returnData"`
	}

	// The ABI decoder returns a slice of anonymous structs.
	rawResults, ok := decoded[0].([]struct {
		Success    bool   `json:"success"`
		ReturnData []byte `json:"returnData"`
	})
	if !ok {
		return nil, fmt.Errorf("unexpected aggregate3 result type: %T", decoded[0])
	}

	results := make([]multicall3Result, len(rawResults))
	for i, r := range rawResults {
		results[i] = multicall3Result{Success: r.Success, ReturnData: r.ReturnData}
	}
	return results, nil
}

// ── RPC call helpers ────────────────────────────────────────────────────

func callGetAllMarkets(ctx context.Context, client *ethclient.Client, comptroller common.Address) ([]common.Address, error) {
	data, err := comptrollerABI.Pack("getAllMarkets")
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "pack getAllMarkets", err)
	}
	out, err := client.CallContract(ctx, ethereum.CallMsg{To: &comptroller, Data: data}, nil)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "call getAllMarkets", err)
	}
	decoded, err := comptrollerABI.Unpack("getAllMarkets", out)
	if err != nil || len(decoded) == 0 {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "decode getAllMarkets", err)
	}
	addrs, ok := decoded[0].([]common.Address)
	if !ok {
		return nil, clierr.New(clierr.CodeUnavailable, "invalid getAllMarkets response")
	}
	return addrs, nil
}

func callGetAssetsIn(ctx context.Context, client *ethclient.Client, comptroller, account common.Address) (map[common.Address]bool, error) {
	data, err := comptrollerABI.Pack("getAssetsIn", account)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "pack getAssetsIn", err)
	}
	out, err := client.CallContract(ctx, ethereum.CallMsg{To: &comptroller, Data: data}, nil)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "call getAssetsIn", err)
	}
	decoded, err := comptrollerABI.Unpack("getAssetsIn", out)
	if err != nil || len(decoded) == 0 {
		return nil, clierr.Wrap(clierr.CodeUnavailable, "decode getAssetsIn", err)
	}
	addrs, ok := decoded[0].([]common.Address)
	if !ok {
		return nil, clierr.New(clierr.CodeUnavailable, "invalid getAssetsIn response")
	}
	set := make(map[common.Address]bool, len(addrs))
	for _, addr := range addrs {
		set[addr] = true
	}
	return set, nil
}

func callOracle(ctx context.Context, client *ethclient.Client, comptroller common.Address) (common.Address, error) {
	data, err := comptrollerABI.Pack("oracle")
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeInternal, "pack oracle", err)
	}
	out, err := client.CallContract(ctx, ethereum.CallMsg{To: &comptroller, Data: data}, nil)
	if err != nil {
		return common.Address{}, clierr.Wrap(clierr.CodeUnavailable, "call oracle", err)
	}
	decoded, err := comptrollerABI.Unpack("oracle", out)
	if err != nil || len(decoded) == 0 {
		return common.Address{}, clierr.Wrap(clierr.CodeUnavailable, "decode oracle", err)
	}
	addr, ok := decoded[0].(common.Address)
	if !ok {
		return common.Address{}, clierr.New(clierr.CodeUnavailable, "invalid oracle response")
	}
	return addr, nil
}

func callGetAccountSnapshot(ctx context.Context, client *ethclient.Client, mToken, account common.Address) (*big.Int, *big.Int, *big.Int, *big.Int, error) {
	data, err := mTokenABI.Pack("getAccountSnapshot", account)
	if err != nil {
		return nil, nil, nil, nil, clierr.Wrap(clierr.CodeInternal, "pack getAccountSnapshot", err)
	}
	out, err := client.CallContract(ctx, ethereum.CallMsg{To: &mToken, Data: data}, nil)
	if err != nil {
		return nil, nil, nil, nil, clierr.Wrap(clierr.CodeUnavailable, "call getAccountSnapshot", err)
	}
	decoded, err := mTokenABI.Unpack("getAccountSnapshot", out)
	if err != nil || len(decoded) < 4 {
		return nil, nil, nil, nil, clierr.Wrap(clierr.CodeUnavailable, "decode getAccountSnapshot", err)
	}
	errCode := asBigInt(decoded[0])
	mTokenBal := asBigInt(decoded[1])
	borrowBal := asBigInt(decoded[2])
	exchangeRate := asBigInt(decoded[3])
	return errCode, mTokenBal, borrowBal, exchangeRate, nil
}


// ── Utility helpers ─────────────────────────────────────────────────────

func rateToAPY(ratePerTimestamp *big.Int) float64 {
	if ratePerTimestamp == nil || ratePerTimestamp.Sign() == 0 {
		return 0
	}
	// APY ≈ ratePerSecond * secondsPerYear / 1e18 * 100  (linear approximation)
	rateFloat := new(big.Float).SetInt(ratePerTimestamp)
	rateFloat.Mul(rateFloat, big.NewFloat(secondsPerYear))
	rateFloat.Quo(rateFloat, big.NewFloat(1e18))
	rateFloat.Mul(rateFloat, big.NewFloat(100))
	result, _ := rateFloat.Float64()
	if math.IsNaN(result) || math.IsInf(result, 0) {
		return 0
	}
	return result
}

func bigIntToFloat(v *big.Int, decimals int) float64 {
	if v == nil || v.Sign() == 0 {
		return 0
	}
	f := new(big.Float).SetInt(v)
	divisor := new(big.Float).SetFloat64(math.Pow(10, float64(decimals)))
	f.Quo(f, divisor)
	result, _ := f.Float64()
	return result
}

func asBigInt(v interface{}) *big.Int {
	switch val := v.(type) {
	case *big.Int:
		if val == nil {
			return new(big.Int)
		}
		return val
	case big.Int:
		return &val
	default:
		return new(big.Int)
	}
}

func amountInfoFromBigInt(v *big.Int, decimals int) model.AmountInfo {
	if v == nil {
		v = new(big.Int)
	}
	base := v.String()
	return model.AmountInfo{
		AmountBaseUnits: base,
		AmountDecimal:   id.FormatDecimalCompat(base, decimals),
		Decimals:        decimals,
	}
}

func normalizeEVMAddress(address string) string {
	addr := strings.ToLower(strings.TrimSpace(address))
	if len(addr) != 42 || !strings.HasPrefix(addr, "0x") {
		return ""
	}
	return addr
}

func canonicalAssetIDForChain(chainID, address string) string {
	addr := normalizeEVMAddress(address)
	if chainID == "" || addr == "" {
		return ""
	}
	return fmt.Sprintf("%s/erc20:%s", chainID, addr)
}

func providerNativeID(provider, chainID, comptrollerAddress, underlyingAddress string) string {
	return fmt.Sprintf("%s:%s:%s:%s", provider, chainID, normalizeEVMAddress(comptrollerAddress), normalizeEVMAddress(underlyingAddress))
}

func hashOpportunity(provider, chainID, marketID, assetID string) string {
	seed := strings.Join([]string{provider, chainID, marketID, assetID}, "|")
	h := sha1.Sum([]byte(seed))
	return hex.EncodeToString(h[:])
}

func matchesAsset(address, symbol string, asset id.Asset) bool {
	assetAddress := strings.TrimSpace(asset.Address)
	if assetAddress != "" {
		return strings.EqualFold(strings.TrimSpace(address), assetAddress)
	}
	assetSymbol := strings.TrimSpace(asset.Symbol)
	if assetSymbol != "" {
		return strings.EqualFold(strings.TrimSpace(symbol), assetSymbol)
	}
	return true
}

func matchesPositionType(filter, position providers.LendPositionType) bool {
	if filter == "" || filter == providers.LendPositionTypeAll {
		return true
	}
	return filter == position
}

func sortLendPositions(items []model.LendPosition) {
	sort.Slice(items, func(i, j int) bool {
		if items[i].AmountUSD != items[j].AmountUSD {
			return items[i].AmountUSD > items[j].AmountUSD
		}
		if items[i].PositionType != items[j].PositionType {
			return items[i].PositionType < items[j].PositionType
		}
		if items[i].AssetID != items[j].AssetID {
			return items[i].AssetID < items[j].AssetID
		}
		return items[i].ProviderNativeID < items[j].ProviderNativeID
	})
}

func sortYieldPositions(items []model.YieldPosition) {
	sort.Slice(items, func(i, j int) bool {
		if items[i].AmountUSD != items[j].AmountUSD {
			return items[i].AmountUSD > items[j].AmountUSD
		}
		if items[i].APYTotal != items[j].APYTotal {
			return items[i].APYTotal > items[j].APYTotal
		}
		if items[i].AssetID != items[j].AssetID {
			return items[i].AssetID < items[j].AssetID
		}
		return items[i].ProviderNativeID < items[j].ProviderNativeID
	})
}

// ── ABI singletons ──────────────────────────────────────────────────────

var comptrollerABI = mustABI(registry.MoonwellComptrollerABI)
var mTokenABI = mustABI(registry.MoonwellMTokenABI)
var oracleABI = mustABI(registry.MoonwellOracleABI)
var erc20ABI = mustABI(registry.MoonwellERC20MinimalABI)
var mc3ABI = mustABI(registry.Multicall3ABI)

func mustABI(raw string) abi.ABI {
	parsed, err := abi.JSON(strings.NewReader(raw))
	if err != nil {
		panic(fmt.Sprintf("invalid ABI: %v", err))
	}
	return parsed
}
