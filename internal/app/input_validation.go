package app

import (
	"fmt"
	"regexp"
	"strings"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/fsutil"
	"github.com/ggonzalez94/defi-cli/internal/schema"
	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

var actionIDPattern = regexp.MustCompile(`(?i)^act_[0-9a-f]{32}$`)

func normalizeAndValidateCommandFlags(cmd *cobra.Command) error {
	var validationErr error
	cmd.Flags().VisitAll(func(flag *pflag.Flag) {
		if validationErr != nil || !flag.Changed || flag.Hidden {
			return
		}
		meta := schema.FlagMetadataFor(flag)
		switch flag.Value.Type() {
		case "string":
			value := flag.Value.String()
			if strings.EqualFold(meta.Format, "json") {
				return
			}
			if err := validateTextInput(flag.Name, meta.Format, value); err != nil {
				validationErr = err
				return
			}
			if strings.EqualFold(meta.Format, "path") {
				canonical, err := canonicalizeCLIPath(value)
				if err != nil {
					validationErr = clierr.Wrap(clierr.CodeUsage, "normalize --"+flag.Name, err)
					return
				}
				if err := flag.Value.Set(canonical); err != nil {
					validationErr = clierr.Wrap(clierr.CodeUsage, "set --"+flag.Name, err)
				}
			}
		case "stringSlice", "stringArray":
			values, err := cmd.Flags().GetStringSlice(flag.Name)
			if err != nil {
				validationErr = clierr.Wrap(clierr.CodeUsage, "read --"+flag.Name, err)
				return
			}
			for _, value := range values {
				if err := validateTextInput(flag.Name, meta.Format, value); err != nil {
					validationErr = err
					return
				}
			}
		}
	})
	return validationErr
}

func canonicalizeCLIPath(path string) (string, error) {
	path = strings.TrimSpace(path)
	if path == "" || path == "-" {
		return path, nil
	}
	return fsutil.NormalizePath(path)
}

func validateTextInput(name, format, value string) error {
	label := "--" + strings.TrimSpace(name)
	if fsutil.ContainsControlChars(value) {
		return clierr.New(clierr.CodeUsage, fmt.Sprintf("%s contains unsupported control characters", label))
	}
	if value == "" {
		return nil
	}

	normalizedFormat := strings.ToLower(strings.TrimSpace(format))
	if normalizedFormat == "url" || normalizedFormat == "path" || normalizedFormat == "json" {
		return nil
	}
	if shouldRejectReservedIdentifierChars(name, normalizedFormat) && strings.ContainsAny(value, "%?#") {
		return clierr.New(clierr.CodeUsage, fmt.Sprintf("%s contains reserved characters (%%, ?, #)", label))
	}
	if normalizedFormat == "action-id" {
		if !actionIDPattern.MatchString(strings.TrimSpace(value)) {
			return clierr.New(clierr.CodeUsage, "action id must match act_<32 hex chars>")
		}
	}
	return nil
}

func shouldRejectReservedIdentifierChars(name, format string) bool {
	switch format {
	case "action-id", "asset", "chain", "evm-address", "hex", "identifier", "provider":
		return true
	}
	switch strings.ToLower(strings.TrimSpace(name)) {
	case "action-id", "address", "asset", "assets", "bridge", "chain", "from", "to", "from-address", "to-address",
		"recipient", "on-behalf-of", "market-id", "vault-address", "pool-address", "pool-address-provider",
		"provider", "providers", "reward-token", "spender", "symbol", "type", "private-key", "from-asset", "to-asset":
		return true
	default:
		return false
	}
}
