package planner

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/big"
	"net/http"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/ethclient"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/httpx"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/registry"
)

const defaultMorphoGraphQLEndpoint = "https://api.morpho.org/graphql"

var morphoGraphQLEndpoint = defaultMorphoGraphQLEndpoint

const morphoMarketByIDQuery = `query Market($chain:Int!,$key:String!){
  markets(first: 1, where:{ chainId_in: [$chain], uniqueKey_in: [$key], listed: true }){
    items{
      uniqueKey
      irmAddress
      lltv
      morphoBlue{ address }
      oracle{ address }
      loanAsset{ address symbol decimals chain{ id } }
      collateralAsset{ address symbol decimals }
      state{ supplyAssetsUsd liquidityAssetsUsd }
    }
  }
}`

type MorphoLendRequest struct {
	Verb            AaveLendVerb
	Chain           id.Chain
	Asset           id.Asset
	AmountBaseUnits string
	Sender          string
	Recipient       string
	OnBehalfOf      string
	Simulate        bool
	RPCURL          string
	MarketID        string
}

type morphoMarketByIDResponse struct {
	Data struct {
		Markets struct {
			Items []struct {
				UniqueKey string `json:"uniqueKey"`
				IRM       string `json:"irmAddress"`
				LLTV      string `json:"lltv"`
				Morpho    struct {
					Address string `json:"address"`
				} `json:"morphoBlue"`
				Oracle struct {
					Address string `json:"address"`
				} `json:"oracle"`
				LoanAsset struct {
					Address  string `json:"address"`
					Symbol   string `json:"symbol"`
					Decimals int    `json:"decimals"`
					Chain    struct {
						ID int64 `json:"id"`
					} `json:"chain"`
				} `json:"loanAsset"`
				CollateralAsset *struct {
					Address  string `json:"address"`
					Symbol   string `json:"symbol"`
					Decimals int    `json:"decimals"`
				} `json:"collateralAsset"`
			} `json:"items"`
		} `json:"markets"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type morphoMarketParamsABI struct {
	LoanToken       common.Address `abi:"loanToken"`
	CollateralToken common.Address `abi:"collateralToken"`
	Oracle          common.Address `abi:"oracle"`
	IRM             common.Address `abi:"irm"`
	LLTV            *big.Int       `abi:"lltv"`
}

func BuildMorphoLendAction(ctx context.Context, req MorphoLendRequest) (execution.Action, error) {
	verb := strings.ToLower(strings.TrimSpace(string(req.Verb)))
	sender, recipient, onBehalfOf, amount, rpcURL, tokenAddr, err := normalizeLendInputs(AaveLendRequest{
		Verb:            req.Verb,
		Chain:           req.Chain,
		Asset:           req.Asset,
		AmountBaseUnits: req.AmountBaseUnits,
		Sender:          req.Sender,
		Recipient:       req.Recipient,
		OnBehalfOf:      req.OnBehalfOf,
		Simulate:        req.Simulate,
		RPCURL:          req.RPCURL,
	})
	if err != nil {
		return execution.Action{}, err
	}

	marketID, err := normalizeMorphoMarketID(req.MarketID)
	if err != nil {
		return execution.Action{}, err
	}
	market, err := fetchMorphoMarketByID(ctx, req.Chain.EVMChainID, marketID)
	if err != nil {
		return execution.Action{}, err
	}
	if !strings.EqualFold(strings.TrimSpace(market.LoanAsset.Address), tokenAddr.Hex()) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "selected morpho market loan token does not match --asset")
	}
	if strings.TrimSpace(market.Morpho.Address) == "" || !common.IsHexAddress(market.Morpho.Address) {
		return execution.Action{}, clierr.New(clierr.CodeUnavailable, "morpho market missing executable morpho contract address")
	}
	if strings.TrimSpace(market.Oracle.Address) == "" || !common.IsHexAddress(market.Oracle.Address) {
		return execution.Action{}, clierr.New(clierr.CodeUnavailable, "morpho market missing oracle address")
	}
	if strings.TrimSpace(market.IRM) == "" || !common.IsHexAddress(market.IRM) {
		return execution.Action{}, clierr.New(clierr.CodeUnavailable, "morpho market missing irm address")
	}
	if market.CollateralAsset == nil || !common.IsHexAddress(market.CollateralAsset.Address) {
		return execution.Action{}, clierr.New(clierr.CodeUnavailable, "morpho market missing collateral token address")
	}
	lltv, ok := new(big.Int).SetString(strings.TrimSpace(market.LLTV), 10)
	if !ok || lltv.Sign() <= 0 {
		return execution.Action{}, clierr.New(clierr.CodeUnavailable, "morpho market returned invalid lltv")
	}

	morphoAddr := common.HexToAddress(market.Morpho.Address)
	loanToken := common.HexToAddress(market.LoanAsset.Address)
	params := morphoMarketParamsABI{
		LoanToken:       loanToken,
		CollateralToken: common.HexToAddress(market.CollateralAsset.Address),
		Oracle:          common.HexToAddress(market.Oracle.Address),
		IRM:             common.HexToAddress(market.IRM),
		LLTV:            lltv,
	}

	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
	}
	defer client.Close()

	action := execution.NewAction(execution.NewActionID(), "lend_"+verb, req.Chain.CAIP2, execution.Constraints{Simulate: req.Simulate})
	action.Provider = "morpho"
	action.FromAddress = sender.Hex()
	action.ToAddress = recipient.Hex()
	action.InputAmount = amount.String()
	action.Metadata = map[string]any{
		"protocol":             "morpho",
		"asset_id":             req.Asset.AssetID,
		"market_id":            marketID,
		"loan_token":           loanToken.Hex(),
		"collateral_token":     params.CollateralToken.Hex(),
		"oracle":               params.Oracle.Hex(),
		"irm":                  params.IRM.Hex(),
		"lltv":                 lltv.String(),
		"morpho_address":       morphoAddr.Hex(),
		"on_behalf_of":         onBehalfOf.Hex(),
		"recipient":            recipient.Hex(),
		"lending_action":       verb,
		"market_loan_symbol":   strings.ToUpper(strings.TrimSpace(market.LoanAsset.Symbol)),
		"market_collat_symbol": strings.ToUpper(strings.TrimSpace(market.CollateralAsset.Symbol)),
	}

	zero := big.NewInt(0)
	switch verb {
	case string(AaveVerbSupply):
		if err := appendApprovalIfNeeded(ctx, client, &action, req.Chain.CAIP2, rpcURL, loanToken, sender, morphoAddr, amount, "Approve token for Morpho supply"); err != nil {
			return execution.Action{}, err
		}
		data, err := morphoBlueABI.Pack("supply", params, amount, zero, onBehalfOf, []byte{})
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack morpho supply calldata", err)
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      "morpho-supply",
			Type:        execution.StepTypeLend,
			Status:      execution.StepStatusPending,
			ChainID:     req.Chain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Supply asset to Morpho market",
			Target:      morphoAddr.Hex(),
			Data:        "0x" + common.Bytes2Hex(data),
			Value:       "0",
		})
	case string(AaveVerbWithdraw):
		data, err := morphoBlueABI.Pack("withdraw", params, amount, zero, onBehalfOf, recipient)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack morpho withdraw calldata", err)
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      "morpho-withdraw",
			Type:        execution.StepTypeLend,
			Status:      execution.StepStatusPending,
			ChainID:     req.Chain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Withdraw supplied assets from Morpho market",
			Target:      morphoAddr.Hex(),
			Data:        "0x" + common.Bytes2Hex(data),
			Value:       "0",
		})
	case string(AaveVerbBorrow):
		data, err := morphoBlueABI.Pack("borrow", params, amount, zero, onBehalfOf, recipient)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack morpho borrow calldata", err)
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      "morpho-borrow",
			Type:        execution.StepTypeLend,
			Status:      execution.StepStatusPending,
			ChainID:     req.Chain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Borrow asset from Morpho market",
			Target:      morphoAddr.Hex(),
			Data:        "0x" + common.Bytes2Hex(data),
			Value:       "0",
		})
	case string(AaveVerbRepay):
		if err := appendApprovalIfNeeded(ctx, client, &action, req.Chain.CAIP2, rpcURL, loanToken, sender, morphoAddr, amount, "Approve token for Morpho repay"); err != nil {
			return execution.Action{}, err
		}
		data, err := morphoBlueABI.Pack("repay", params, amount, zero, onBehalfOf, []byte{})
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack morpho repay calldata", err)
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      "morpho-repay",
			Type:        execution.StepTypeLend,
			Status:      execution.StepStatusPending,
			ChainID:     req.Chain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Repay borrowed assets in Morpho market",
			Target:      morphoAddr.Hex(),
			Data:        "0x" + common.Bytes2Hex(data),
			Value:       "0",
		})
	default:
		return execution.Action{}, clierr.New(clierr.CodeUsage, "unsupported lend action verb")
	}

	return action, nil
}

func normalizeMorphoMarketID(marketID string) (string, error) {
	clean := strings.TrimSpace(marketID)
	if clean == "" {
		return "", clierr.New(clierr.CodeUsage, "morpho lend execution requires --market-id")
	}
	if !strings.HasPrefix(clean, "0x") && !strings.HasPrefix(clean, "0X") {
		return "", clierr.New(clierr.CodeUsage, "morpho --market-id must be a 0x-prefixed bytes32 value")
	}
	raw := strings.TrimPrefix(strings.TrimPrefix(clean, "0x"), "0X")
	if len(raw) != 64 {
		return "", clierr.New(clierr.CodeUsage, "morpho --market-id must be a 32-byte hex value")
	}
	if _, err := hex.DecodeString(raw); err != nil {
		return "", clierr.New(clierr.CodeUsage, "morpho --market-id must be valid hex")
	}
	return "0x" + strings.ToLower(raw), nil
}

func fetchMorphoMarketByID(ctx context.Context, chainID int64, marketID string) (struct {
	UniqueKey string `json:"uniqueKey"`
	IRM       string `json:"irmAddress"`
	LLTV      string `json:"lltv"`
	Morpho    struct {
		Address string `json:"address"`
	} `json:"morphoBlue"`
	Oracle struct {
		Address string `json:"address"`
	} `json:"oracle"`
	LoanAsset struct {
		Address  string `json:"address"`
		Symbol   string `json:"symbol"`
		Decimals int    `json:"decimals"`
		Chain    struct {
			ID int64 `json:"id"`
		} `json:"chain"`
	} `json:"loanAsset"`
	CollateralAsset *struct {
		Address  string `json:"address"`
		Symbol   string `json:"symbol"`
		Decimals int    `json:"decimals"`
	} `json:"collateralAsset"`
}, error) {
	var market struct {
		UniqueKey string `json:"uniqueKey"`
		IRM       string `json:"irmAddress"`
		LLTV      string `json:"lltv"`
		Morpho    struct {
			Address string `json:"address"`
		} `json:"morphoBlue"`
		Oracle struct {
			Address string `json:"address"`
		} `json:"oracle"`
		LoanAsset struct {
			Address  string `json:"address"`
			Symbol   string `json:"symbol"`
			Decimals int    `json:"decimals"`
			Chain    struct {
				ID int64 `json:"id"`
			} `json:"chain"`
		} `json:"loanAsset"`
		CollateralAsset *struct {
			Address  string `json:"address"`
			Symbol   string `json:"symbol"`
			Decimals int    `json:"decimals"`
		} `json:"collateralAsset"`
	}

	body, err := json.Marshal(map[string]any{
		"query": morphoMarketByIDQuery,
		"variables": map[string]any{
			"chain": chainID,
			"key":   marketID,
		},
	})
	if err != nil {
		return market, clierr.Wrap(clierr.CodeInternal, "marshal morpho market lookup query", err)
	}

	client := httpx.New(10*time.Second, 0)
	var resp morphoMarketByIDResponse
	if _, err := httpx.DoBodyJSON(ctx, client, http.MethodPost, morphoGraphQLEndpoint, body, nil, &resp); err != nil {
		return market, err
	}
	if len(resp.Errors) > 0 {
		return market, clierr.New(clierr.CodeUnavailable, fmt.Sprintf("morpho graphql error: %s", resp.Errors[0].Message))
	}
	if len(resp.Data.Markets.Items) == 0 {
		return market, clierr.New(clierr.CodeUsage, "morpho market-id not found for selected chain")
	}
	return resp.Data.Markets.Items[0], nil
}

var morphoBlueABI = mustPlannerABI(registry.MorphoBlueABI)
