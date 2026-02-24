package cache

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
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

const (
	lockAcquireTimeout = 5 * time.Second
	lockRetryInterval  = 20 * time.Millisecond
	sqliteMaxRetries   = 6
	sqliteRetryBase    = 10 * time.Millisecond
)

func Open(path, lockPath string) (*Store, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return nil, fmt.Errorf("create cache directory: %w", err)
	}
	if err := os.MkdirAll(filepath.Dir(lockPath), 0o755); err != nil {
		return nil, fmt.Errorf("create lock directory: %w", err)
	}
	lock := flock.New(lockPath)
	unlock, err := acquireFileLock(lock, lockAcquireTimeout)
	if err != nil {
		return nil, err
	}
	defer unlock()

	db, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, fmt.Errorf("open sqlite cache: %w", err)
	}
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)
	db.SetConnMaxIdleTime(0)
	db.SetConnMaxLifetime(0)

	queries := []string{
		"PRAGMA journal_mode=WAL;",
		"PRAGMA synchronous=NORMAL;",
		"PRAGMA busy_timeout=5000;",
		"CREATE TABLE IF NOT EXISTS cache_entries (key TEXT PRIMARY KEY, value BLOB NOT NULL, created_at INTEGER NOT NULL, ttl_seconds INTEGER NOT NULL);",
	}
	for _, query := range queries {
		if err := execWithRetry(db, query); err != nil {
			_ = db.Close()
			return nil, fmt.Errorf("init cache schema: %w", err)
		}
	}

	return &Store{db: db, lock: lock}, nil
}

func (s *Store) Close() error {
	if s == nil || s.db == nil {
		return nil
	}
	return s.db.Close()
}

func (s *Store) Get(key string, maxStale time.Duration) (Result, error) {
	var value []byte
	var createdUnix int64
	var ttlSeconds int64
	err := withSQLiteRetry(func() error {
		return s.db.QueryRow("SELECT value, created_at, ttl_seconds FROM cache_entries WHERE key = ?", key).Scan(&value, &createdUnix, &ttlSeconds)
	})
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
	unlock, err := acquireFileLock(s.lock, lockAcquireTimeout)
	if err != nil {
		return err
	}
	defer unlock()

	createdUnix := time.Now().UTC().Unix()
	ttlSeconds := int64(ttl.Seconds())
	if ttlSeconds <= 0 {
		ttlSeconds = 1
	}
	err = execWithRetry(s.db, `
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

func acquireFileLock(lock *flock.Flock, timeout time.Duration) (func(), error) {
	if lock == nil {
		return func() {}, nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	locked, err := lock.TryLockContext(ctx, lockRetryInterval)
	if err != nil {
		return nil, fmt.Errorf("lock cache: %w", err)
	}
	if !locked {
		return nil, fmt.Errorf("lock cache: timeout acquiring lock")
	}
	return func() { _ = lock.Unlock() }, nil
}

func execWithRetry(db *sql.DB, query string, args ...any) error {
	return withSQLiteRetry(func() error {
		_, err := db.Exec(query, args...)
		return err
	})
}

func withSQLiteRetry(op func() error) error {
	var err error
	delay := sqliteRetryBase
	for attempt := 0; attempt < sqliteMaxRetries; attempt++ {
		err = op()
		if err == nil || !isSQLiteBusyErr(err) {
			return err
		}
		time.Sleep(delay)
		if delay < 250*time.Millisecond {
			delay *= 2
		}
	}
	return err
}

func isSQLiteBusyErr(err error) bool {
	if err == nil {
		return false
	}
	msg := strings.ToLower(err.Error())
	return strings.Contains(msg, "database is locked") || strings.Contains(msg, "database is busy")
}
