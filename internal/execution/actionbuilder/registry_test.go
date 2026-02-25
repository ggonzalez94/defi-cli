package actionbuilder

import (
	"context"
	"strings"
	"testing"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution/planner"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/model"
	"github.com/ggonzalez94/defi-cli/internal/providers"
)

func TestBuildSwapActionRejectsQuoteOnlyProvider(t *testing.T) {
	reg := New(map[string]providers.SwapProvider{
		"quoteonly": swapQuoteOnlyProvider{},
	}, nil)

	_, _, err := reg.BuildSwapAction(context.Background(), "quoteonly", "plan", providers.SwapQuoteRequest{}, providers.SwapExecutionOptions{})
	if err == nil {
		t.Fatal("expected quote-only swap provider to fail for plan")
	}
	if !strings.Contains(strings.ToLower(err.Error()), "does not support swap planning") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestBuildBridgeActionRejectsQuoteOnlyProvider(t *testing.T) {
	reg := New(nil, map[string]providers.BridgeProvider{
		"quoteonly": bridgeQuoteOnlyProvider{},
	})

	_, _, err := reg.BuildBridgeAction(context.Background(), "quoteonly", providers.BridgeQuoteRequest{}, providers.BridgeExecutionOptions{})
	if err == nil {
		t.Fatal("expected quote-only bridge provider to fail for execution")
	}
	if !strings.Contains(strings.ToLower(err.Error()), "quote-only") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestBuildLendActionRejectsUnsupportedProtocol(t *testing.T) {
	reg := New(nil, nil)
	_, err := reg.BuildLendAction(context.Background(), LendRequest{Protocol: "kamino"})
	if err == nil {
		t.Fatal("expected unsupported protocol error")
	}
	cErr, ok := clierr.As(err)
	if !ok || cErr.Code != clierr.CodeUnsupported {
		t.Fatalf("expected unsupported cli error, got %v", err)
	}
}

func TestBuildRewardsClaimActionRejectsUnsupportedProtocol(t *testing.T) {
	reg := New(nil, nil)
	_, err := reg.BuildRewardsClaimAction(context.Background(), RewardsClaimRequest{Protocol: "morpho"})
	if err == nil {
		t.Fatal("expected unsupported protocol error")
	}
	cErr, ok := clierr.As(err)
	if !ok || cErr.Code != clierr.CodeUnsupported {
		t.Fatalf("expected unsupported cli error, got %v", err)
	}
}

func TestBuildApprovalActionRoutesToPlanner(t *testing.T) {
	reg := New(nil, nil)
	chain, err := id.ParseChain("1")
	if err != nil {
		t.Fatalf("parse chain: %v", err)
	}
	asset, err := id.ParseAsset("USDC", chain)
	if err != nil {
		t.Fatalf("parse asset: %v", err)
	}

	action, err := reg.BuildApprovalAction(planner.ApprovalRequest{
		Chain:           chain,
		Asset:           asset,
		AmountBaseUnits: "1000",
		Sender:          "0x00000000000000000000000000000000000000aa",
		Spender:         "0x00000000000000000000000000000000000000bb",
		Simulate:        true,
		RPCURL:          "https://eth.llamarpc.com",
	})
	if err != nil {
		t.Fatalf("BuildApprovalAction failed: %v", err)
	}
	if action.IntentType != "approve" {
		t.Fatalf("unexpected intent: %s", action.IntentType)
	}
}

type swapQuoteOnlyProvider struct{}

func (swapQuoteOnlyProvider) Info() model.ProviderInfo {
	return model.ProviderInfo{Name: "quoteonly", Type: "swap"}
}

func (swapQuoteOnlyProvider) QuoteSwap(context.Context, providers.SwapQuoteRequest) (model.SwapQuote, error) {
	return model.SwapQuote{}, nil
}

type bridgeQuoteOnlyProvider struct{}

func (bridgeQuoteOnlyProvider) Info() model.ProviderInfo {
	return model.ProviderInfo{Name: "quoteonly", Type: "bridge"}
}

func (bridgeQuoteOnlyProvider) QuoteBridge(context.Context, providers.BridgeQuoteRequest) (model.BridgeQuote, error) {
	return model.BridgeQuote{}, nil
}
