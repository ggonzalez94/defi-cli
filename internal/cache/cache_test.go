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

			store, err := Open(dbPath, lockPath)
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
