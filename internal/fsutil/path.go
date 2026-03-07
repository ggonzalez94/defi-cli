package fsutil

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

func ContainsControlChars(value string) bool {
	for _, r := range value {
		if r < 0x20 {
			return true
		}
	}
	return false
}

func NormalizePath(input string) (string, error) {
	value := strings.TrimSpace(input)
	if value == "" {
		return "", nil
	}
	if ContainsControlChars(value) {
		return "", fmt.Errorf("path contains control characters")
	}
	if value == "~" {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("resolve home directory: %w", err)
		}
		value = home
	} else if strings.HasPrefix(value, "~/") {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("resolve home directory: %w", err)
		}
		value = filepath.Join(home, value[2:])
	}
	cleaned := filepath.Clean(value)
	absPath, err := filepath.Abs(cleaned)
	if err != nil {
		return "", fmt.Errorf("resolve absolute path: %w", err)
	}
	return absPath, nil
}
