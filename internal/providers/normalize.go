package providers

import "strings"

// NormalizeLendingProvider canonicalizes supported lending provider aliases.
func NormalizeLendingProvider(input string) string {
	switch strings.ToLower(strings.TrimSpace(input)) {
	case "aave", "aave-v2", "aave-v3":
		return "aave"
	case "morpho", "morpho-blue":
		return "morpho"
	case "kamino", "kamino-lend", "kamino-finance":
		return "kamino"
	case "moonwell", "moonwell-v2":
		return "moonwell"
	default:
		return strings.ToLower(strings.TrimSpace(input))
	}
}

// NormalizeSwapProvider canonicalizes supported swap provider aliases.
func NormalizeSwapProvider(input string) string {
	switch strings.ToLower(strings.TrimSpace(input)) {
	case "tempo", "tempo-dex", "tempodex":
		return "tempo"
	default:
		return strings.ToLower(strings.TrimSpace(input))
	}
}
