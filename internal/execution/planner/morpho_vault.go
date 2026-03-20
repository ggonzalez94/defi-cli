package planner

import (
	"context"
	"encoding/json"
	"fmt"
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

const morphoVaultByAddressQuery = `query VaultByAddress($address:String!,$chainId:Int!){
  vaultByAddress(address:$address, chainId:$chainId){
    address
    listed
    asset{ address symbol decimals chain{ id } }
  }
}`

const morphoVaultV2ByAddressQuery = `query VaultV2ByAddress($address:String!,$chainId:Int!){
  vaultV2ByAddress(address:$address, chainId:$chainId){
    address
    listed
    asset{ address symbol decimals chain{ id } }
  }
}`

type MorphoVaultYieldVerb string

const (
	MorphoVaultYieldVerbDeposit  MorphoVaultYieldVerb = "deposit"
	MorphoVaultYieldVerbWithdraw MorphoVaultYieldVerb = "withdraw"
)

type MorphoVaultYieldRequest struct {
	Verb            MorphoVaultYieldVerb
	Chain           id.Chain
	Asset           id.Asset
	VaultAddress    string
	AmountBaseUnits string
	Sender          string
	Recipient       string
	OnBehalfOf      string
	Simulate        bool
	RPCURL          string
}

type morphoVaultLookupResponse struct {
	Data struct {
		VaultByAddress *struct {
			Address string `json:"address"`
			Listed  bool   `json:"listed"`
			Asset   *struct {
				Address  string `json:"address"`
				Symbol   string `json:"symbol"`
				Decimals int    `json:"decimals"`
				Chain    struct {
					ID int64 `json:"id"`
				} `json:"chain"`
			} `json:"asset"`
		} `json:"vaultByAddress"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type morphoVaultV2LookupResponse struct {
	Data struct {
		VaultV2ByAddress *struct {
			Address string `json:"address"`
			Listed  bool   `json:"listed"`
			Asset   *struct {
				Address  string `json:"address"`
				Symbol   string `json:"symbol"`
				Decimals int    `json:"decimals"`
				Chain    struct {
					ID int64 `json:"id"`
				} `json:"chain"`
			} `json:"asset"`
		} `json:"vaultV2ByAddress"`
	} `json:"data"`
	Errors []struct {
		Message string `json:"message"`
	} `json:"errors"`
}

type morphoVaultMetadata struct {
	Address       string
	AssetAddress  string
	AssetSymbol   string
	AssetDecimals int
	Kind          string
}

func BuildMorphoVaultYieldAction(ctx context.Context, req MorphoVaultYieldRequest) (execution.Action, error) {
	if !req.Chain.IsEVM() {
		return execution.Action{}, clierr.New(clierr.CodeUnsupported, "morpho vault execution supports only EVM chains")
	}
	verb := strings.ToLower(strings.TrimSpace(string(req.Verb)))
	if verb != string(MorphoVaultYieldVerbDeposit) && verb != string(MorphoVaultYieldVerbWithdraw) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "yield action must be deposit or withdraw")
	}

	sender, recipient, onBehalfOf, amount, rpcURL, tokenAddr, err := normalizeLendInputs(AaveLendRequest{
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

	if verb == string(MorphoVaultYieldVerbWithdraw) && sender != onBehalfOf {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "morpho vault withdraw currently requires --on-behalf-of to match sender")
	}

	vaultRaw := strings.TrimSpace(req.VaultAddress)
	if !common.IsHexAddress(vaultRaw) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "morpho vault yield execution requires a valid --vault-address")
	}
	vault := common.HexToAddress(vaultRaw)

	vaultMeta, err := fetchMorphoVaultByAddress(ctx, req.Chain.EVMChainID, vault.Hex())
	if err != nil {
		return execution.Action{}, err
	}
	if !strings.EqualFold(vaultMeta.AssetAddress, tokenAddr.Hex()) {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "selected morpho vault asset does not match --asset")
	}

	client, err := ethclient.DialContext(ctx, rpcURL)
	if err != nil {
		return execution.Action{}, clierr.Wrap(clierr.CodeUnavailable, "connect rpc", err)
	}
	defer client.Close()

	action := execution.NewAction(execution.NewActionID(), "yield_"+verb, req.Chain.CAIP2, execution.Constraints{Simulate: req.Simulate})
	action.Provider = "morpho"
	action.FromAddress = sender.Hex()
	action.ToAddress = recipient.Hex()
	action.InputAmount = amount.String()
	action.Metadata = map[string]any{
		"protocol":             "morpho",
		"asset_id":             req.Asset.AssetID,
		"vault_address":        vault.Hex(),
		"vault_kind":           vaultMeta.Kind,
		"vault_asset":          common.HexToAddress(vaultMeta.AssetAddress).Hex(),
		"vault_asset_symbol":   strings.ToUpper(strings.TrimSpace(vaultMeta.AssetSymbol)),
		"vault_asset_decimals": vaultMeta.AssetDecimals,
		"yield_action":         verb,
		"yield_product":        "vault",
		"recipient":            recipient.Hex(),
		"on_behalf_of":         onBehalfOf.Hex(),
	}

	switch verb {
	case string(MorphoVaultYieldVerbDeposit):
		if err := appendApprovalIfNeeded(ctx, client, &action, req.Chain.CAIP2, rpcURL, tokenAddr, sender, vault, amount, "Approve token for Morpho vault deposit"); err != nil {
			return execution.Action{}, err
		}
		data, err := erc4626VaultABI.Pack("deposit", amount, recipient)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack morpho vault deposit calldata", err)
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      "morpho-vault-deposit",
			Type:        execution.StepTypeLend,
			Status:      execution.StepStatusPending,
			ChainID:     req.Chain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Deposit asset into Morpho vault",
			Target:      vault.Hex(),
			Data:        "0x" + common.Bytes2Hex(data),
			Value:       "0",
		})
	case string(MorphoVaultYieldVerbWithdraw):
		data, err := erc4626VaultABI.Pack("withdraw", amount, recipient, onBehalfOf)
		if err != nil {
			return execution.Action{}, clierr.Wrap(clierr.CodeInternal, "pack morpho vault withdraw calldata", err)
		}
		action.Steps = append(action.Steps, execution.ActionStep{
			StepID:      "morpho-vault-withdraw",
			Type:        execution.StepTypeLend,
			Status:      execution.StepStatusPending,
			ChainID:     req.Chain.CAIP2,
			RPCURL:      rpcURL,
			Description: "Withdraw asset from Morpho vault",
			Target:      vault.Hex(),
			Data:        "0x" + common.Bytes2Hex(data),
			Value:       "0",
		})
	}

	return action, nil
}

func fetchMorphoVaultByAddress(ctx context.Context, chainID int64, address string) (morphoVaultMetadata, error) {
	meta := morphoVaultMetadata{}
	lookupAddress := common.HexToAddress(address).Hex()

	body, err := json.Marshal(map[string]any{
		"query": morphoVaultByAddressQuery,
		"variables": map[string]any{
			"address": lookupAddress,
			"chainId": chainID,
		},
	})
	if err != nil {
		return meta, clierr.Wrap(clierr.CodeInternal, "marshal morpho vault lookup query", err)
	}

	client := httpx.New(10*time.Second, 0)
	var resp morphoVaultLookupResponse
	if _, err := httpx.DoBodyJSON(ctx, client, http.MethodPost, morphoGraphQLEndpoint, body, nil, &resp); err != nil {
		return meta, err
	}
	if len(resp.Errors) > 0 && !isMorphoLookupNotFound(resp.Errors[0].Message) {
		return meta, clierr.New(clierr.CodeUnavailable, fmt.Sprintf("morpho graphql error: %s", resp.Errors[0].Message))
	}
	if resp.Data.VaultByAddress != nil {
		if !resp.Data.VaultByAddress.Listed {
			return meta, clierr.New(clierr.CodeUnsupported, "morpho vault is not listed")
		}
		if resp.Data.VaultByAddress.Asset == nil || !common.IsHexAddress(resp.Data.VaultByAddress.Asset.Address) {
			return meta, clierr.New(clierr.CodeUnavailable, "morpho vault missing asset metadata")
		}
		return morphoVaultMetadata{
			Address:       common.HexToAddress(resp.Data.VaultByAddress.Address).Hex(),
			AssetAddress:  common.HexToAddress(resp.Data.VaultByAddress.Asset.Address).Hex(),
			AssetSymbol:   strings.TrimSpace(resp.Data.VaultByAddress.Asset.Symbol),
			AssetDecimals: resp.Data.VaultByAddress.Asset.Decimals,
			Kind:          "vault",
		}, nil
	}

	body, err = json.Marshal(map[string]any{
		"query": morphoVaultV2ByAddressQuery,
		"variables": map[string]any{
			"address": lookupAddress,
			"chainId": chainID,
		},
	})
	if err != nil {
		return meta, clierr.Wrap(clierr.CodeInternal, "marshal morpho vault-v2 lookup query", err)
	}

	var respV2 morphoVaultV2LookupResponse
	if _, err := httpx.DoBodyJSON(ctx, client, http.MethodPost, morphoGraphQLEndpoint, body, nil, &respV2); err != nil {
		return meta, err
	}
	if len(respV2.Errors) > 0 {
		if isMorphoLookupNotFound(respV2.Errors[0].Message) {
			return meta, clierr.New(clierr.CodeUsage, "morpho vault address not found for selected chain")
		}
		return meta, clierr.New(clierr.CodeUnavailable, fmt.Sprintf("morpho graphql error: %s", respV2.Errors[0].Message))
	}
	if respV2.Data.VaultV2ByAddress == nil {
		return meta, clierr.New(clierr.CodeUsage, "morpho vault address not found for selected chain")
	}
	if !respV2.Data.VaultV2ByAddress.Listed {
		return meta, clierr.New(clierr.CodeUnsupported, "morpho vault is not listed")
	}
	if respV2.Data.VaultV2ByAddress.Asset == nil || !common.IsHexAddress(respV2.Data.VaultV2ByAddress.Asset.Address) {
		return meta, clierr.New(clierr.CodeUnavailable, "morpho vault missing asset metadata")
	}
	return morphoVaultMetadata{
		Address:       common.HexToAddress(respV2.Data.VaultV2ByAddress.Address).Hex(),
		AssetAddress:  common.HexToAddress(respV2.Data.VaultV2ByAddress.Asset.Address).Hex(),
		AssetSymbol:   strings.TrimSpace(respV2.Data.VaultV2ByAddress.Asset.Symbol),
		AssetDecimals: respV2.Data.VaultV2ByAddress.Asset.Decimals,
		Kind:          "vault_v2",
	}, nil
}

func isMorphoLookupNotFound(message string) bool {
	return strings.Contains(strings.ToLower(strings.TrimSpace(message)), "no results matching given parameters")
}

var erc4626VaultABI = mustPlannerABI(registry.ERC4626VaultABI)
