package cache

import (
	"fmt"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

func TestCacheSetGetFreshAndStale(t *testing.T) {
	tmp := t.TempDir()
	store, err := Open(filepath.Join(tmp, "cache.db"), filepath.Join(tmp, "cache.lock"), 5*time.Minute)
	if err != nil {
		t.Fatalf("Open cache failed: %v", err)
	}
	defer store.Close()

	if err := store.Set("k1", []byte(`{"v":1}`), 1*time.Second); err != nil {
		t.Fatalf("Set failed: %v", err)
	}

	res, err := store.Get("k1", 5*time.Second)
	if err != nil {
		t.Fatalf("Get fresh failed: %v", err)
	}
	if !res.Hit || res.Stale {
		t.Fatalf("expected fresh hit, got %+v", res)
	}

	time.Sleep(1200 * time.Millisecond)
	res, err = store.Get("k1", 5*time.Second)
	if err != nil {
		t.Fatalf("Get stale failed: %v", err)
	}
	if !res.Hit || !res.Stale || res.TooStale {
		t.Fatalf("expected stale within budget, got %+v", res)
	}
}

func TestCacheTooStale(t *testing.T) {
	tmp := t.TempDir()
	store, err := Open(filepath.Join(tmp, "cache.db"), filepath.Join(tmp, "cache.lock"), 5*time.Minute)
	if err != nil {
		t.Fatalf("Open cache failed: %v", err)
	}
	defer store.Close()

	if err := store.Set("k2", []byte(`{"v":2}`), 1*time.Second); err != nil {
		t.Fatalf("Set failed: %v", err)
	}
	time.Sleep(1300 * time.Millisecond)
	res, err := store.Get("k2", 10*time.Millisecond)
	if err != nil {
		t.Fatalf("Get failed: %v", err)
	}
	if !res.TooStale {
		t.Fatalf("expected too stale, got %+v", res)
	}
}

func TestPruneRemovesExpiredEntries(t *testing.T) {
	tmp := t.TempDir()
	store, err := Open(filepath.Join(tmp, "cache.db"), filepath.Join(tmp, "cache.lock"), 5*time.Minute)
	if err != nil {
		t.Fatalf("Open cache failed: %v", err)
	}
	defer store.Close()

	// Insert an entry with a very short TTL.
	if err := store.Set("prunable", []byte(`"old"`), 1*time.Second); err != nil {
		t.Fatalf("Set failed: %v", err)
	}
	// Insert a long-lived entry.
	if err := store.Set("keeper", []byte(`"keep"`), 1*time.Hour); err != nil {
		t.Fatalf("Set failed: %v", err)
	}

	// Wait long enough for the 1s TTL to fully expire at Unix-second
	// granularity. 1200ms can land in the same second as creation;
	// 2100ms guarantees at least one full second has elapsed.
	time.Sleep(2100 * time.Millisecond)

	// Prune with zero max_stale so expired entries are removed immediately.
	if err := store.Prune(0); err != nil {
		t.Fatalf("Prune failed: %v", err)
	}

	// The expired entry should be gone (miss).
	res, err := store.Get("prunable", 1*time.Hour)
	if err != nil {
		t.Fatalf("Get prunable failed: %v", err)
	}
	if res.Hit {
		t.Fatalf("expected prunable to be evicted, but got hit")
	}

	// The long-lived entry should still be there.
	res, err = store.Get("keeper", 1*time.Hour)
	if err != nil {
		t.Fatalf("Get keeper failed: %v", err)
	}
	if !res.Hit {
		t.Fatalf("expected keeper to still be present")
	}
}

func TestPrunePreservesStaleWithinMaxStale(t *testing.T) {
	tmp := t.TempDir()
	// Use a short max_stale for Open so startup prune does not interfere.
	store, err := Open(filepath.Join(tmp, "cache.db"), filepath.Join(tmp, "cache.lock"), 10*time.Minute)
	if err != nil {
		t.Fatalf("Open cache failed: %v", err)
	}
	defer store.Close()

	// Insert an entry with 1s TTL — it will expire quickly but should
	// remain within a generous max_stale window.
	if err := store.Set("stale-ok", []byte(`"fallback"`), 1*time.Second); err != nil {
		t.Fatalf("Set failed: %v", err)
	}

	// Wait for TTL to expire.
	time.Sleep(2100 * time.Millisecond)

	// Prune with a large max_stale window; the stale entry should survive.
	if err := store.Prune(10 * time.Minute); err != nil {
		t.Fatalf("Prune failed: %v", err)
	}

	res, err := store.Get("stale-ok", 10*time.Minute)
	if err != nil {
		t.Fatalf("Get stale-ok failed: %v", err)
	}
	if !res.Hit {
		t.Fatalf("expected stale-ok to be preserved within max_stale window, but got miss")
	}
	if !res.Stale {
		t.Fatalf("expected stale-ok to be stale, got fresh")
	}
	if res.TooStale {
		t.Fatalf("expected stale-ok to be within max_stale, got TooStale")
	}

	// Now prune with zero max_stale — the entry should be removed.
	if err := store.Prune(0); err != nil {
		t.Fatalf("Prune (zero max_stale) failed: %v", err)
	}

	res, err = store.Get("stale-ok", 10*time.Minute)
	if err != nil {
		t.Fatalf("Get stale-ok after zero-max-stale prune failed: %v", err)
	}
	if res.Hit {
		t.Fatalf("expected stale-ok to be evicted after zero max_stale prune, but got hit")
	}
}

func TestPruneMaxStaleFloor(t *testing.T) {
	tests := []struct {
		name     string
		input    time.Duration
		expected time.Duration
	}{
		{name: "zero gets floored", input: 0, expected: time.Hour},
		{name: "30s gets floored", input: 30 * time.Second, expected: time.Hour},
		{name: "59m gets floored", input: 59 * time.Minute, expected: time.Hour},
		{name: "1h stays", input: time.Hour, expected: time.Hour},
		{name: "2h stays", input: 2 * time.Hour, expected: 2 * time.Hour},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := pruneMaxStale(tc.input)
			if got != tc.expected {
				t.Fatalf("pruneMaxStale(%v) = %v, want %v", tc.input, got, tc.expected)
			}
		})
	}
}

func TestOpenWithZeroMaxStalePreservesStale(t *testing.T) {
	tmp := t.TempDir()
	dbPath := filepath.Join(tmp, "cache.db")
	lockPath := filepath.Join(tmp, "cache.lock")

	// Open with large maxStale and insert a short-TTL entry.
	store, err := Open(dbPath, lockPath, 10*time.Minute)
	if err != nil {
		t.Fatalf("Open failed: %v", err)
	}
	if err := store.Set("fragile", []byte(`"data"`), 1*time.Second); err != nil {
		t.Fatalf("Set failed: %v", err)
	}
	store.Close()

	// Wait for TTL to expire.
	time.Sleep(2100 * time.Millisecond)

	// Re-open with maxStale=0. The prune floor should prevent eviction.
	store2, err := Open(dbPath, lockPath, 0)
	if err != nil {
		t.Fatalf("Open (zero maxStale) failed: %v", err)
	}
	defer store2.Close()

	res, err := store2.Get("fragile", time.Hour)
	if err != nil {
		t.Fatalf("Get fragile failed: %v", err)
	}
	if !res.Hit {
		t.Fatal("expected stale entry to survive startup prune with zero maxStale (floor should apply)")
	}
}

func TestCacheConcurrentOpenAndSet(t *testing.T) {
	tmp := t.TempDir()
	dbPath := filepath.Join(tmp, "cache.db")
	lockPath := filepath.Join(tmp, "cache.lock")

	const workers = 16
	const iterations = 40

	var wg sync.WaitGroup
	errCh := make(chan error, workers)
	for worker := 0; worker < workers; worker++ {
		wg.Add(1)
		go func(workerID int) {
			defer wg.Done()

			store, err := Open(dbPath, lockPath, 5*time.Minute)
			if err != nil {
				errCh <- fmt.Errorf("worker %d open: %w", workerID, err)
				return
			}
			defer store.Close()

			for i := 0; i < iterations; i++ {
				key := fmt.Sprintf("worker-%d-key-%d", workerID, i)
				if err := store.Set(key, []byte(`{"ok":true}`), time.Minute); err != nil {
					errCh <- fmt.Errorf("worker %d set iter %d: %w", workerID, i, err)
					return
				}
				res, err := store.Get(key, time.Minute)
				if err != nil {
					errCh <- fmt.Errorf("worker %d get iter %d: %w", workerID, i, err)
					return
				}
				if !res.Hit {
					errCh <- fmt.Errorf("worker %d get iter %d: expected hit", workerID, i)
					return
				}
			}
		}(worker)
	}
	wg.Wait()
	close(errCh)
	for err := range errCh {
		t.Fatal(err)
	}
}
