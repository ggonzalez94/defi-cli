package policy

import (
	"strings"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
)

func CheckCommandAllowed(allowlist []string, commandPath string) error {
	if len(allowlist) == 0 {
		return nil
	}
	normPath := normalize(commandPath)
	for _, allowed := range allowlist {
		if normalize(allowed) == normPath {
			return nil
		}
	}
	return clierr.New(clierr.CodeBlocked, "command blocked by --enable-commands policy")
}

func normalize(v string) string {
	parts := strings.Fields(strings.ToLower(strings.TrimSpace(v)))
	return strings.Join(parts, " ")
}
