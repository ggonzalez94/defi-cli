package id

import (
	"fmt"
	"math/big"
	"regexp"
	"strings"

	clierr "github.com/gustavo/defi-cli/internal/errors"
)

var decimalPattern = regexp.MustCompile(`^[0-9]+(\.[0-9]+)?$`)

func NormalizeAmount(baseUnits, decimal string, decimals int) (string, string, error) {
	if baseUnits != "" && decimal != "" {
		return "", "", clierr.New(clierr.CodeUsage, "use either --amount or --amount-decimal, not both")
	}
	if baseUnits == "" && decimal == "" {
		return "", "", clierr.New(clierr.CodeUsage, "amount is required")
	}
	if decimals < 0 {
		return "", "", clierr.New(clierr.CodeUsage, "decimals must be >= 0")
	}

	if baseUnits != "" {
		if _, ok := new(big.Int).SetString(baseUnits, 10); !ok {
			return "", "", clierr.New(clierr.CodeUsage, "--amount must be a positive integer string")
		}
		if strings.HasPrefix(baseUnits, "-") {
			return "", "", clierr.New(clierr.CodeUsage, "--amount must be non-negative")
		}
		return baseUnits, formatDecimal(baseUnits, decimals), nil
	}

	if !decimalPattern.MatchString(decimal) {
		return "", "", clierr.New(clierr.CodeUsage, "--amount-decimal must be in decimal form like 1.23")
	}
	base, err := decimalToBaseUnits(decimal, decimals)
	if err != nil {
		return "", "", err
	}
	return base, normalizeDecimal(decimal), nil
}

func formatDecimal(baseUnits string, decimals int) string {
	n := new(big.Int)
	n.SetString(baseUnits, 10)
	if decimals == 0 {
		return n.String()
	}

	s := n.String()
	if len(s) <= decimals {
		pad := strings.Repeat("0", decimals-len(s)+1)
		s = pad + s
	}
	intPart := s[:len(s)-decimals]
	fracPart := s[len(s)-decimals:]
	fracPart = strings.TrimRight(fracPart, "0")
	if fracPart == "" {
		return intPart
	}
	return intPart + "." + fracPart
}

func decimalToBaseUnits(decimal string, decimals int) (string, error) {
	parts := strings.SplitN(decimal, ".", 2)
	intPart := parts[0]
	fracPart := ""
	if len(parts) == 2 {
		fracPart = parts[1]
	}
	if len(fracPart) > decimals {
		return "", clierr.New(clierr.CodeUsage, fmt.Sprintf("decimal precision exceeds token decimals (%d)", decimals))
	}

	fracPart = fracPart + strings.Repeat("0", decimals-len(fracPart))
	combined := intPart + fracPart
	combined = strings.TrimLeft(combined, "0")
	if combined == "" {
		return "0", nil
	}
	if _, ok := new(big.Int).SetString(combined, 10); !ok {
		return "", clierr.New(clierr.CodeUsage, "invalid decimal amount")
	}
	return combined, nil
}

func normalizeDecimal(v string) string {
	if !strings.Contains(v, ".") {
		out := strings.TrimLeft(v, "0")
		if out == "" {
			return "0"
		}
		return out
	}
	parts := strings.SplitN(v, ".", 2)
	intPart := strings.TrimLeft(parts[0], "0")
	if intPart == "" {
		intPart = "0"
	}
	fracPart := strings.TrimRight(parts[1], "0")
	if fracPart == "" {
		return intPart
	}
	return intPart + "." + fracPart
}

// FormatDecimalCompat converts base-unit integer strings into decimal strings.
func FormatDecimalCompat(baseUnits string, decimals int) string {
	return formatDecimal(baseUnits, decimals)
}
