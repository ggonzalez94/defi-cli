package cache

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/gofrs/flock"
	_ "modernc.org/sqlite"
)

type Store struct {
	db   *sql.DB
	lock *flock.Flock
}

type Result struct {
	Hit      bool
	Value    []byte
	Age      time.Duration
	Stale    bool
	TooStale bool
}

func Open(path, lockPath string) (*Store, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return nil, fmt.Errorf("create cache directory: %w", err)
	}
	if err := os.MkdirAll(filepath.Dir(lockPath), 0o755); err != nil {
		return nil, fmt.Errorf("create lock directory: %w", err)
	}

	db, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, fmt.Errorf("open sqlite cache: %w", err)
	}

	queries := []string{
		"PRAGMA journal_mode=WAL;",
		"PRAGMA synchronous=NORMAL;",
		"CREATE TABLE IF NOT EXISTS cache_entries (key TEXT PRIMARY KEY, value BLOB NOT NULL, created_at INTEGER NOT NULL, ttl_seconds INTEGER NOT NULL);",
	}
	for _, query := range queries {
		if _, err := db.Exec(query); err != nil {
			_ = db.Close()
			return nil, fmt.Errorf("init cache schema: %w", err)
		}
	}

	store := &Store{db: db, lock: flock.New(lockPath)}
	// Prune expired entries on startup to prevent unbounded growth.
	_ = store.Prune()
	return store, nil
}

func (s *Store) Close() error {
	if s == nil || s.db == nil {
		return nil
	}
	return s.db.Close()
}

// Prune deletes all cache entries whose TTL has fully expired (age > ttl).
// It is called automatically on Open and can be called manually.
func (s *Store) Prune() error {
	if s == nil || s.db == nil {
		return nil
	}
	nowUnix := time.Now().UTC().Unix()
	_, err := s.db.Exec("DELETE FROM cache_entries WHERE created_at + ttl_seconds < ?", nowUnix)
	if err != nil {
		return fmt.Errorf("prune cache: %w", err)
	}
	return nil
}

func (s *Store) Get(key string, maxStale time.Duration) (Result, error) {
	var value []byte
	var createdUnix int64
	var ttlSeconds int64
	err := s.db.QueryRow("SELECT value, created_at, ttl_seconds FROM cache_entries WHERE key = ?", key).Scan(&value, &createdUnix, &ttlSeconds)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return Result{Hit: false}, nil
		}
		return Result{}, fmt.Errorf("cache read: %w", err)
	}

	created := time.Unix(createdUnix, 0).UTC()
	age := time.Since(created)
	if age < 0 {
		age = 0
	}
	ttl := time.Duration(ttlSeconds) * time.Second
	stale := age > ttl
	tooStale := stale && maxStale >= 0 && age > ttl+maxStale

	return Result{
		Hit:      true,
		Value:    value,
		Age:      age,
		Stale:    stale,
		TooStale: tooStale,
	}, nil
}

func (s *Store) Set(key string, value []byte, ttl time.Duration) error {
	locked, err := s.lock.TryLockContext(context.Background(), 5*time.Second)
	if err != nil {
		return fmt.Errorf("lock cache: %w", err)
	}
	if !locked {
		return fmt.Errorf("lock cache: timeout acquiring lock")
	}
	defer func() { _ = s.lock.Unlock() }()

	createdUnix := time.Now().UTC().Unix()
	ttlSeconds := int64(ttl.Seconds())
	if ttlSeconds <= 0 {
		ttlSeconds = 1
	}
	_, err = s.db.Exec(`
		INSERT INTO cache_entries (key, value, created_at, ttl_seconds)
		VALUES (?, ?, ?, ?)
		ON CONFLICT(key) DO UPDATE SET
			value=excluded.value,
			created_at=excluded.created_at,
			ttl_seconds=excluded.ttl_seconds
	`, key, value, createdUnix, ttlSeconds)
	if err != nil {
		return fmt.Errorf("cache write: %w", err)
	}
	return nil
}
