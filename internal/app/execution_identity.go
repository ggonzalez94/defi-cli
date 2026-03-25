package app

import (
	"strings"

	"github.com/ethereum/go-ethereum/common"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	"github.com/ggonzalez94/defi-cli/internal/id"
	"github.com/ggonzalez94/defi-cli/internal/ows"
)

type executionIdentity struct {
	WalletID         string
	WalletName       string
	FromAddress      string
	ExecutionBackend execution.ExecutionBackend
	Warnings         []string
}

func resolveExecutionIdentity(walletRef, fromAddress, chainArg string) (executionIdentity, error) {
	walletRef = strings.TrimSpace(walletRef)
	fromAddress = strings.TrimSpace(fromAddress)

	if walletRef != "" && fromAddress != "" {
		return executionIdentity{}, clierr.New(clierr.CodeUsage, "use only one identity input: --wallet or --from-address")
	}
	if walletRef == "" && fromAddress == "" {
		return executionIdentity{}, clierr.New(clierr.CodeUsage, "exactly one identity input is required: --wallet or --from-address")
	}

	if walletRef != "" {
		chain, err := id.ParseChain(chainArg)
		if err != nil {
			return executionIdentity{}, err
		}
		if !chain.IsEVM() {
			return executionIdentity{}, clierr.New(clierr.CodeUnsupported, "--wallet planning currently supports EVM chains only")
		}
		if isTempoChain(chain.CAIP2) {
			return executionIdentity{}, clierr.New(clierr.CodeUnsupported, "--wallet planning is not supported on Tempo chains yet; use --from-address")
		}

		wallet, err := ows.ResolveWalletRef("", walletRef)
		if err != nil {
			return executionIdentity{}, clierr.Wrap(clierr.CodeUsage, "resolve --wallet", err)
		}
		sender, err := ows.SenderAddressForChain(wallet, chain.CAIP2)
		if err != nil {
			return executionIdentity{}, clierr.Wrap(clierr.CodeUsage, "resolve wallet sender for chain", err)
		}
		if !common.IsHexAddress(sender) {
			return executionIdentity{}, clierr.New(clierr.CodeUsage, "wallet sender address must be a valid EVM hex address")
		}

		return executionIdentity{
			WalletID:         wallet.ID,
			WalletName:       wallet.Name,
			FromAddress:      common.HexToAddress(sender).Hex(),
			ExecutionBackend: execution.ExecutionBackendOWS,
		}, nil
	}

	if !common.IsHexAddress(fromAddress) {
		return executionIdentity{}, clierr.New(clierr.CodeUsage, "--from-address must be a valid EVM hex address")
	}
	return executionIdentity{
		FromAddress:      common.HexToAddress(fromAddress).Hex(),
		ExecutionBackend: execution.ExecutionBackendLegacyLocal,
		Warnings:         []string{"--from-address is deprecated for planning; use --wallet instead"},
	}, nil
}

func isTempoChain(chainID string) bool {
	switch strings.TrimSpace(chainID) {
	case "eip155:4217", "eip155:42431", "eip155:31318":
		return true
	default:
		return false
	}
}
