package cache

import (
	"path/filepath"
	"testing"
	"time"
)

func TestCacheSetGetFreshAndStale(t *testing.T) {
	tmp := t.TempDir()
	store, err := Open(filepath.Join(tmp, "cache.db"), filepath.Join(tmp, "cache.lock"))
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
	store, err := Open(filepath.Join(tmp, "cache.db"), filepath.Join(tmp, "cache.lock"))
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
	store, err := Open(filepath.Join(tmp, "cache.db"), filepath.Join(tmp, "cache.lock"))
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

	// Wait for the short entry to expire.
	time.Sleep(1200 * time.Millisecond)

	if err := store.Prune(); err != nil {
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
