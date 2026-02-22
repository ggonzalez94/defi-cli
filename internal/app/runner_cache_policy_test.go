package app

import (
	"bytes"
	"context"
	"encoding/json"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/ggonzalez94/defi-cli/internal/cache"
	"github.com/ggonzalez94/defi-cli/internal/config"
	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/model"
)

type cachePolicyEnvelope struct {
	Success  bool           `json:"success"`
	Data     map[string]any `json:"data"`
	Warnings []string       `json:"warnings"`
	Meta     struct {
		Cache     model.CacheStatus      `json:"cache"`
		Providers []model.ProviderStatus `json:"providers"`
	} `json:"meta"`
}

func TestRunCachedCommandFetchesProviderAfterTTLExpiry(t *testing.T) {
	state, stdout := newCachePolicyTestState(t, 5*time.Minute, false)
	key := "runner-cache-policy-fetch-after-ttl"
	if err := state.cache.Set(key, []byte(`{"source":"cache"}`), time.Second); err != nil {
		t.Fatalf("cache set failed: %v", err)
	}
	time.Sleep(1200 * time.Millisecond)

	fetchCalls := 0
	err := state.runCachedCommand("test command", key, time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
		fetchCalls++
		return map[string]any{"source": "provider"}, []model.ProviderStatus{{Name: "test-provider", Status: "ok", LatencyMS: 1}}, nil, false, nil
	})
	if err != nil {
		t.Fatalf("runCachedCommand failed: %v", err)
	}
	if fetchCalls != 1 {
		t.Fatalf("expected provider fetch after ttl expiry, got calls=%d", fetchCalls)
	}

	env := decodeCachePolicyEnvelope(t, stdout)
	if !env.Success {
		t.Fatalf("expected success envelope, got %#v", env)
	}
	if env.Data["source"] != "provider" {
		t.Fatalf("expected provider data after ttl expiry, got %#v", env.Data)
	}
	if env.Meta.Cache.Status != "write" || env.Meta.Cache.Stale {
		t.Fatalf("expected cache write metadata, got %+v", env.Meta.Cache)
	}
	if len(env.Meta.Providers) != 1 || env.Meta.Providers[0].Name != "test-provider" {
		t.Fatalf("expected provider metadata in response, got %+v", env.Meta.Providers)
	}
}

func TestRunCachedCommandFallsBackToStaleOnProviderFailure(t *testing.T) {
	state, stdout := newCachePolicyTestState(t, 5*time.Second, false)
	key := "runner-cache-policy-fallback-stale"
	if err := state.cache.Set(key, []byte(`{"source":"cache"}`), time.Second); err != nil {
		t.Fatalf("cache set failed: %v", err)
	}
	time.Sleep(1200 * time.Millisecond)

	fetchCalls := 0
	err := state.runCachedCommand("test command", key, time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
		fetchCalls++
		return nil, []model.ProviderStatus{{Name: "test-provider", Status: "unavailable", LatencyMS: 1}}, nil, false, clierr.New(clierr.CodeUnavailable, "provider unavailable")
	})
	if err != nil {
		t.Fatalf("expected stale fallback success, got error: %v", err)
	}
	if fetchCalls != 1 {
		t.Fatalf("expected exactly one provider fetch attempt, got %d", fetchCalls)
	}

	env := decodeCachePolicyEnvelope(t, stdout)
	if env.Data["source"] != "cache" {
		t.Fatalf("expected stale cache fallback data, got %#v", env.Data)
	}
	if env.Meta.Cache.Status != "hit" || !env.Meta.Cache.Stale {
		t.Fatalf("expected stale cache hit metadata, got %+v", env.Meta.Cache)
	}
	if len(env.Meta.Providers) != 1 || env.Meta.Providers[0].Status != "unavailable" {
		t.Fatalf("expected provider failure metadata, got %+v", env.Meta.Providers)
	}
	if !containsWarning(env.Warnings, "provider fetch failed; serving stale data within max-stale budget") {
		t.Fatalf("expected stale fallback warning, got %+v", env.Warnings)
	}
}

func TestRunCachedCommandRejectsStaleWhenBeyondMaxStale(t *testing.T) {
	state, _ := newCachePolicyTestState(t, 10*time.Millisecond, false)
	key := "runner-cache-policy-too-stale"
	if err := state.cache.Set(key, []byte(`{"source":"cache"}`), time.Second); err != nil {
		t.Fatalf("cache set failed: %v", err)
	}
	time.Sleep(1300 * time.Millisecond)

	fetchCalls := 0
	err := state.runCachedCommand("test command", key, time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
		fetchCalls++
		return nil, []model.ProviderStatus{{Name: "test-provider", Status: "unavailable", LatencyMS: 1}}, nil, false, clierr.New(clierr.CodeUnavailable, "provider unavailable")
	})
	if fetchCalls != 1 {
		t.Fatalf("expected provider fetch attempt before stale rejection, got %d", fetchCalls)
	}
	if err == nil {
		t.Fatal("expected stale rejection error, got nil")
	}
	if code := clierr.ExitCode(err); code != int(clierr.CodeStale) {
		t.Fatalf("expected stale exit code %d, got %d err=%v", int(clierr.CodeStale), code, err)
	}
	if !strings.Contains(err.Error(), "cached data exceeded stale budget") {
		t.Fatalf("expected stale budget message, got %v", err)
	}
}

func TestRunCachedCommandRejectsStaleIfFetchDelayPushesBeyondMaxStale(t *testing.T) {
	state, _ := newCachePolicyTestState(t, 2*time.Second, false)
	key := "runner-cache-policy-crosses-budget-during-fetch"
	if err := state.cache.Set(key, []byte(`{"source":"cache"}`), time.Second); err != nil {
		t.Fatalf("cache set failed: %v", err)
	}
	time.Sleep(1200 * time.Millisecond)

	fetchCalls := 0
	err := state.runCachedCommand("test command", key, time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
		fetchCalls++
		time.Sleep(2 * time.Second)
		return nil, []model.ProviderStatus{{Name: "test-provider", Status: "unavailable", LatencyMS: 2000}}, nil, false, clierr.New(clierr.CodeUnavailable, "provider unavailable")
	})
	if fetchCalls != 1 {
		t.Fatalf("expected one provider fetch attempt, got %d", fetchCalls)
	}
	if err == nil {
		t.Fatal("expected stale rejection after delayed fetch failure, got nil")
	}
	if code := clierr.ExitCode(err); code != int(clierr.CodeStale) {
		t.Fatalf("expected stale exit code %d, got %d err=%v", int(clierr.CodeStale), code, err)
	}
	if !strings.Contains(err.Error(), "cached data exceeded stale budget") {
		t.Fatalf("expected stale budget message, got %v", err)
	}
}

func TestRunCachedCommandDoesNotFallbackStaleOnAuthFailure(t *testing.T) {
	state, _ := newCachePolicyTestState(t, 5*time.Second, false)
	key := "runner-cache-policy-no-fallback-auth"
	if err := state.cache.Set(key, []byte(`{"source":"cache"}`), time.Second); err != nil {
		t.Fatalf("cache set failed: %v", err)
	}
	time.Sleep(1200 * time.Millisecond)

	err := state.runCachedCommand("test command", key, time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
		return nil, []model.ProviderStatus{{Name: "test-provider", Status: "auth_error", LatencyMS: 1}}, nil, false, clierr.New(clierr.CodeAuth, "missing api key")
	})
	if err == nil {
		t.Fatal("expected auth error, got nil")
	}
	if code := clierr.ExitCode(err); code != int(clierr.CodeAuth) {
		t.Fatalf("expected auth exit code %d, got %d err=%v", int(clierr.CodeAuth), code, err)
	}
}

func TestRunCachedCommandStrictPartialErrorPreservesDiagnostics(t *testing.T) {
	state, _ := newCachePolicyTestState(t, 5*time.Second, false)
	state.settings.Strict = true
	key := "runner-cache-policy-strict-partial"

	err := state.runCachedCommand("test command", key, time.Second, func(ctx context.Context) (any, []model.ProviderStatus, []string, bool, error) {
		return map[string]any{"source": "provider"},
			[]model.ProviderStatus{
				{Name: "aave", Status: "ok", LatencyMS: 12},
				{Name: "morpho", Status: "unavailable", LatencyMS: 34},
			},
			[]string{"provider morpho failed: timeout"},
			true,
			nil
	})
	if err == nil {
		t.Fatal("expected strict partial error, got nil")
	}
	if code := clierr.ExitCode(err); code != int(clierr.CodePartialStrict) {
		t.Fatalf("expected partial strict exit code %d, got %d err=%v", int(clierr.CodePartialStrict), code, err)
	}

	stderrBuf, ok := state.runner.stderr.(*bytes.Buffer)
	if !ok {
		t.Fatalf("expected stderr buffer, got %T", state.runner.stderr)
	}
	state.renderError("test command", err, state.lastWarnings, state.lastProviders, state.lastPartial)

	var env struct {
		Success  bool            `json:"success"`
		Warnings []string        `json:"warnings"`
		Error    model.ErrorBody `json:"error"`
		Meta     struct {
			Partial   bool                   `json:"partial"`
			Providers []model.ProviderStatus `json:"providers"`
		} `json:"meta"`
	}
	if decodeErr := json.Unmarshal(stderrBuf.Bytes(), &env); decodeErr != nil {
		t.Fatalf("decode error envelope failed: %v output=%s", decodeErr, stderrBuf.String())
	}
	if env.Success {
		t.Fatalf("expected success=false, got %+v", env)
	}
	if env.Error.Type != "partial_results" {
		t.Fatalf("expected partial_results error type, got %+v", env.Error)
	}
	if !env.Meta.Partial {
		t.Fatalf("expected meta.partial=true, got %+v", env.Meta)
	}
	if len(env.Meta.Providers) != 2 {
		t.Fatalf("expected provider statuses in error meta, got %+v", env.Meta.Providers)
	}
	if !containsWarning(env.Warnings, "provider morpho failed: timeout") {
		t.Fatalf("expected warning propagation, got %+v", env.Warnings)
	}
}

func newCachePolicyTestState(t *testing.T, maxStale time.Duration, noStale bool) (*runtimeState, *bytes.Buffer) {
	t.Helper()
	tmp := t.TempDir()
	store, err := cache.Open(filepath.Join(tmp, "cache.db"), filepath.Join(tmp, "cache.lock"))
	if err != nil {
		t.Fatalf("open cache failed: %v", err)
	}
	t.Cleanup(func() { _ = store.Close() })

	stdout := &bytes.Buffer{}
	stderr := &bytes.Buffer{}
	state := &runtimeState{
		runner: &Runner{
			stdout: stdout,
			stderr: stderr,
			now:    time.Now,
		},
		settings: config.Settings{
			OutputMode:   "json",
			Timeout:      2 * time.Second,
			CacheEnabled: true,
			MaxStale:     maxStale,
			NoStale:      noStale,
		},
		cache: store,
	}
	return state, stdout
}

func decodeCachePolicyEnvelope(t *testing.T, buf *bytes.Buffer) cachePolicyEnvelope {
	t.Helper()
	var env cachePolicyEnvelope
	if err := json.Unmarshal(buf.Bytes(), &env); err != nil {
		t.Fatalf("decode envelope failed: %v output=%s", err, buf.String())
	}
	return env
}

func containsWarning(warnings []string, target string) bool {
	for _, warning := range warnings {
		if warning == target {
			return true
		}
	}
	return false
}
