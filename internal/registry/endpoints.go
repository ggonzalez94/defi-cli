package registry

import (
	"net"
	"net/url"
	"strings"
)

const (
	// Execution provider endpoints.
	LiFiBaseURL         = "https://li.quest/v1"
	LiFiSettlementURL   = "https://li.quest/v1/status"
	AcrossBaseURL       = "https://app.across.to/api"
	AcrossSettlementURL = "https://app.across.to/api/deposit/status"

	// Shared GraphQL endpoint used by Morpho adapter and execution planner.
	MorphoGraphQLEndpoint = "https://api.morpho.org/graphql"
)

func BridgeSettlementURL(provider string) (string, bool) {
	switch strings.ToLower(strings.TrimSpace(provider)) {
	case "lifi":
		return LiFiSettlementURL, true
	case "across":
		return AcrossSettlementURL, true
	default:
		return "", false
	}
}

func IsAllowedBridgeSettlementURL(provider, endpoint string) bool {
	if strings.TrimSpace(endpoint) == "" {
		return true
	}
	parsed, err := url.Parse(strings.TrimSpace(endpoint))
	if err != nil {
		return false
	}
	if strings.TrimSpace(parsed.Hostname()) == "" {
		return false
	}
	if isLoopbackHost(parsed.Hostname()) {
		scheme := strings.ToLower(strings.TrimSpace(parsed.Scheme))
		return scheme == "" || scheme == "http" || scheme == "https"
	}
	if !strings.EqualFold(strings.TrimSpace(parsed.Scheme), "https") {
		return false
	}
	allowedRaw, ok := BridgeSettlementURL(provider)
	if !ok {
		return false
	}
	allowed, err := url.Parse(allowedRaw)
	if err != nil {
		return false
	}
	if !strings.EqualFold(parsed.Scheme, allowed.Scheme) {
		return false
	}
	if !strings.EqualFold(parsed.Hostname(), allowed.Hostname()) {
		return false
	}
	if normalizedURLPort(parsed) != normalizedURLPort(allowed) {
		return false
	}
	return normalizedURLPath(parsed.Path) == normalizedURLPath(allowed.Path)
}

func isLoopbackHost(host string) bool {
	h := strings.TrimSpace(strings.ToLower(host))
	if h == "localhost" {
		return true
	}
	ip := net.ParseIP(h)
	return ip != nil && ip.IsLoopback()
}

func normalizedURLPort(parsed *url.URL) string {
	if parsed == nil {
		return ""
	}
	if port := strings.TrimSpace(parsed.Port()); port != "" {
		return port
	}
	switch strings.ToLower(strings.TrimSpace(parsed.Scheme)) {
	case "http":
		return "80"
	case "https":
		return "443"
	default:
		return ""
	}
}

func normalizedURLPath(path string) string {
	p := strings.TrimSpace(path)
	if p == "" {
		return "/"
	}
	p = strings.TrimSuffix(p, "/")
	if p == "" {
		return "/"
	}
	return p
}
