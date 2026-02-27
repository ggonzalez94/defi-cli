package registry

import (
	"fmt"
	"strings"
)

// Canonical default EVM RPC endpoints by chain ID.
// These values are used whenever a command does not pass --rpc-url.
var defaultRPCByChainID = map[int64]string{
	1:      "https://eth.llamarpc.com",
	10:     "https://mainnet.optimism.io",
	56:     "https://bsc-dataseed.binance.org",
	100:    "https://rpc.gnosischain.com",
	137:    "https://polygon-rpc.com",
	146:    "https://rpc.soniclabs.com",
	252:    "https://rpc.frax.com",
	324:    "https://mainnet.era.zksync.io",
	480:    "https://worldchain-mainnet.g.alchemy.com/public",
	5000:   "https://rpc.mantle.xyz",
	8453:   "https://mainnet.base.org",
	42220:  "https://forno.celo.org",
	42161:  "https://arb1.arbitrum.io/rpc",
	43114:  "https://api.avax.network/ext/bc/C/rpc",
	57073:  "https://rpc-gel.inkonchain.com",
	59144:  "https://rpc.linea.build",
	80094:  "https://rpc.berachain.com",
	81457:  "https://rpc.blast.io",
	167000: "https://rpc.mainnet.taiko.xyz",
	167013: "https://rpc.hoodi.taiko.xyz",
	534352: "https://rpc.scroll.io",
}

func DefaultRPCURL(chainID int64) (string, bool) {
	value, ok := defaultRPCByChainID[chainID]
	return value, ok
}

func ResolveRPCURL(override string, chainID int64) (string, error) {
	if strings.TrimSpace(override) != "" {
		return strings.TrimSpace(override), nil
	}
	if value, ok := DefaultRPCURL(chainID); ok {
		return value, nil
	}
	return "", fmt.Errorf("no default rpc configured for chain id %d; provide --rpc-url", chainID)
}
