package actionbuilder

import (
	"context"
	"fmt"
	"sort"
	"strings"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/execution/planner"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

type Registry struct {
	swapProviders   map[string]providers.SwapProvider
	bridgeProviders map[string]providers.BridgeProvider
}

func New(swapProviders map[string]providers.SwapProvider, bridgeProviders map[string]providers.BridgeProvider) *Registry {
	return &Registry{
		swapProviders:   swapProviders,
		bridgeProviders: bridgeProviders,
	}
}

func (r *Registry) Configure(swapProviders map[string]providers.SwapProvider, bridgeProviders map[string]providers.BridgeProvider) {
	r.swapProviders = swapProviders
	r.bridgeProviders = bridgeProviders
}

func (r *Registry) BuildSwapAction(ctx context.Context, providerName, op string, req providers.SwapQuoteRequest, opts providers.SwapExecutionOptions) (execution.Action, string, error) {
	providerName = strings.ToLower(strings.TrimSpace(providerName))
	if providerName == "" {
		return execution.Action{}, "", clierr.New(clierr.CodeUsage, "--provider is required")
	}
	provider, ok := r.swapProviders[providerName]
	if !ok {
		return execution.Action{}, "", clierr.New(clierr.CodeUnsupported, "unsupported swap provider")
	}
	execProvider, ok := provider.(providers.SwapExecutionProvider)
	if !ok {
		switch strings.ToLower(strings.TrimSpace(op)) {
		case "plan", "planning":
			return execution.Action{}, provider.Info().Name, clierr.New(clierr.CodeUnsupported, fmt.Sprintf("provider %s does not support swap planning", providerName))
		default:
			return execution.Action{}, provider.Info().Name, clierr.New(clierr.CodeUnsupported, fmt.Sprintf("provider %s does not support swap execution", providerName))
		}
	}
	action, err := execProvider.BuildSwapAction(ctx, req, opts)
	return action, provider.Info().Name, err
}

func (r *Registry) BuildBridgeAction(ctx context.Context, providerName string, req providers.BridgeQuoteRequest, opts providers.BridgeExecutionOptions) (execution.Action, string, error) {
	providerName = strings.ToLower(strings.TrimSpace(providerName))
	if providerName == "" {
		return execution.Action{}, "", clierr.New(clierr.CodeUsage, "--provider is required")
	}
	provider, ok := r.bridgeProviders[providerName]
	if !ok {
		return execution.Action{}, "", clierr.New(clierr.CodeUnsupported, "unsupported bridge provider")
	}
	execProvider, ok := provider.(providers.BridgeExecutionProvider)
	if !ok {
		return execution.Action{}, provider.Info().Name, clierr.New(
			clierr.CodeUnsupported,
			fmt.Sprintf("bridge provider %q is quote-only; execution providers: %s", providerName, strings.Join(r.BridgeExecutionProviderNames(), ",")),
		)
	}
	action, err := execProvider.BuildBridgeAction(ctx, req, opts)
	return action, provider.Info().Name, err
}

func (r *Registry) BridgeExecutionProviderNames() []string {
	names := make([]string, 0, len(r.bridgeProviders))
	for name, provider := range r.bridgeProviders {
		if _, ok := provider.(providers.BridgeExecutionProvider); ok {
			names = append(names, name)
		}
	}
	sort.Strings(names)
	return names
}

type LendRequest struct {
	Protocol            string
	Verb                planner.AaveLendVerb
	Chain               id.Chain
	Asset               id.Asset
	MarketID            string
	AmountBaseUnits     string
	Sender              string
	Recipient           string
	OnBehalfOf          string
	InterestRateMode    int64
	Simulate            bool
	RPCURL              string
	PoolAddress         string
	PoolAddressProvider string
}

func (r *Registry) BuildLendAction(ctx context.Context, req LendRequest) (execution.Action, error) {
	protocol := normalizeLendingProtocol(req.Protocol)
	if protocol == "" {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "--protocol is required")
	}
	switch protocol {
	case "aave":
		return planner.BuildAaveLendAction(ctx, planner.AaveLendRequest{
			Verb:                  req.Verb,
			Chain:                 req.Chain,
			Asset:                 req.Asset,
			AmountBaseUnits:       req.AmountBaseUnits,
			Sender:                req.Sender,
			Recipient:             req.Recipient,
			OnBehalfOf:            req.OnBehalfOf,
			InterestRateMode:      req.InterestRateMode,
			Simulate:              req.Simulate,
			RPCURL:                req.RPCURL,
			PoolAddress:           req.PoolAddress,
			PoolAddressesProvider: req.PoolAddressProvider,
		})
	case "morpho":
		return planner.BuildMorphoLendAction(ctx, planner.MorphoLendRequest{
			Verb:            req.Verb,
			Chain:           req.Chain,
			Asset:           req.Asset,
			MarketID:        req.MarketID,
			AmountBaseUnits: req.AmountBaseUnits,
			Sender:          req.Sender,
			Recipient:       req.Recipient,
			OnBehalfOf:      req.OnBehalfOf,
			Simulate:        req.Simulate,
			RPCURL:          req.RPCURL,
		})
	default:
		return execution.Action{}, clierr.New(clierr.CodeUnsupported, "lend execution currently supports protocol=aave|morpho")
	}
}

type RewardsClaimRequest struct {
	Protocol            string
	Chain               id.Chain
	Sender              string
	Recipient           string
	Assets              []string
	RewardToken         string
	AmountBaseUnits     string
	Simulate            bool
	RPCURL              string
	ControllerAddress   string
	PoolAddressProvider string
}

func (r *Registry) BuildRewardsClaimAction(ctx context.Context, req RewardsClaimRequest) (execution.Action, error) {
	protocol := normalizeLendingProtocol(req.Protocol)
	if protocol == "" {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "--protocol is required")
	}
	if protocol != "aave" {
		return execution.Action{}, clierr.New(clierr.CodeUnsupported, "rewards execution currently supports only protocol=aave")
	}
	return planner.BuildAaveRewardsClaimAction(ctx, planner.AaveRewardsClaimRequest{
		Chain:                 req.Chain,
		Sender:                req.Sender,
		Recipient:             req.Recipient,
		Assets:                req.Assets,
		RewardToken:           req.RewardToken,
		AmountBaseUnits:       req.AmountBaseUnits,
		Simulate:              req.Simulate,
		RPCURL:                req.RPCURL,
		ControllerAddress:     req.ControllerAddress,
		PoolAddressesProvider: req.PoolAddressProvider,
	})
}

type RewardsCompoundRequest struct {
	Protocol            string
	Chain               id.Chain
	Sender              string
	Recipient           string
	OnBehalfOf          string
	Assets              []string
	RewardToken         string
	AmountBaseUnits     string
	Simulate            bool
	RPCURL              string
	ControllerAddress   string
	PoolAddress         string
	PoolAddressProvider string
}

func (r *Registry) BuildRewardsCompoundAction(ctx context.Context, req RewardsCompoundRequest) (execution.Action, error) {
	protocol := normalizeLendingProtocol(req.Protocol)
	if protocol == "" {
		return execution.Action{}, clierr.New(clierr.CodeUsage, "--protocol is required")
	}
	if protocol != "aave" {
		return execution.Action{}, clierr.New(clierr.CodeUnsupported, "rewards execution currently supports only protocol=aave")
	}
	return planner.BuildAaveRewardsCompoundAction(ctx, planner.AaveRewardsCompoundRequest{
		Chain:                 req.Chain,
		Sender:                req.Sender,
		Recipient:             req.Recipient,
		Assets:                req.Assets,
		RewardToken:           req.RewardToken,
		AmountBaseUnits:       req.AmountBaseUnits,
		Simulate:              req.Simulate,
		RPCURL:                req.RPCURL,
		ControllerAddress:     req.ControllerAddress,
		PoolAddress:           req.PoolAddress,
		PoolAddressesProvider: req.PoolAddressProvider,
		OnBehalfOf:            req.OnBehalfOf,
	})
}

func (r *Registry) BuildApprovalAction(req planner.ApprovalRequest) (execution.Action, error) {
	return planner.BuildApprovalAction(req)
}

func normalizeLendingProtocol(v string) string {
	return strings.ToLower(strings.TrimSpace(v))
}
