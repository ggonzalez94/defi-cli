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
