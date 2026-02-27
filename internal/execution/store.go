package execution

import (
	"context"
	"database/sql"
	"encoding/json"
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

func OpenStore(path, lockPath string) (*Store, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return nil, fmt.Errorf("create action store directory: %w", err)
	}
	if err := os.MkdirAll(filepath.Dir(lockPath), 0o755); err != nil {
		return nil, fmt.Errorf("create action lock directory: %w", err)
	}
	db, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, fmt.Errorf("open action sqlite: %w", err)
	}

	queries := []string{
		"PRAGMA journal_mode=WAL;",
		"PRAGMA synchronous=NORMAL;",
		`CREATE TABLE IF NOT EXISTS actions (
			action_id TEXT PRIMARY KEY,
			intent_type TEXT NOT NULL,
			status TEXT NOT NULL,
			chain_id TEXT NOT NULL,
			created_at INTEGER NOT NULL,
			updated_at INTEGER NOT NULL,
			payload BLOB NOT NULL
		);`,
		"CREATE INDEX IF NOT EXISTS idx_actions_status_updated ON actions(status, updated_at DESC);",
	}
	for _, q := range queries {
		if _, err := db.Exec(q); err != nil {
			_ = db.Close()
			return nil, fmt.Errorf("init action schema: %w", err)
		}
	}
	return &Store{db: db, lock: flock.New(lockPath)}, nil
}

func (s *Store) Close() error {
	if s == nil || s.db == nil {
		return nil
	}
	return s.db.Close()
}

func (s *Store) Save(action Action) error {
	if stringsTrim(action.ActionID) == "" {
		return fmt.Errorf("save action: missing action id")
	}
	locked, err := s.lock.TryLockContext(context.Background(), 5*time.Second)
	if err != nil {
		return fmt.Errorf("lock action store: %w", err)
	}
	if !locked {
		return fmt.Errorf("lock action store: timeout acquiring lock")
	}
	defer func() { _ = s.lock.Unlock() }()

	payload, err := json.Marshal(action)
	if err != nil {
		return fmt.Errorf("marshal action: %w", err)
	}
	createdUnix, _ := parseRFC3339Unix(action.CreatedAt)
	updatedUnix, _ := parseRFC3339Unix(action.UpdatedAt)
	if createdUnix == 0 {
		createdUnix = time.Now().UTC().Unix()
	}
	if updatedUnix == 0 {
		updatedUnix = time.Now().UTC().Unix()
	}

	_, err = s.db.Exec(`
		INSERT INTO actions (action_id, intent_type, status, chain_id, created_at, updated_at, payload)
		VALUES (?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT(action_id) DO UPDATE SET
			intent_type=excluded.intent_type,
			status=excluded.status,
			chain_id=excluded.chain_id,
			updated_at=excluded.updated_at,
			payload=excluded.payload
	`, action.ActionID, action.IntentType, action.Status, action.ChainID, createdUnix, updatedUnix, payload)
	if err != nil {
		return fmt.Errorf("save action: %w", err)
	}
	return nil
}

func (s *Store) Get(actionID string) (Action, error) {
	var payload []byte
	err := s.db.QueryRow("SELECT payload FROM actions WHERE action_id = ?", actionID).Scan(&payload)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return Action{}, fmt.Errorf("action not found: %s", actionID)
		}
		return Action{}, fmt.Errorf("read action: %w", err)
	}
	var action Action
	if err := json.Unmarshal(payload, &action); err != nil {
		return Action{}, fmt.Errorf("decode action payload: %w", err)
	}
	return action, nil
}

func (s *Store) List(status string, limit int) ([]Action, error) {
	if limit <= 0 {
		limit = 20
	}
	var (
		rows *sql.Rows
		err  error
	)
	if stringsTrim(status) == "" {
		rows, err = s.db.Query("SELECT payload FROM actions ORDER BY updated_at DESC LIMIT ?", limit)
	} else {
		rows, err = s.db.Query("SELECT payload FROM actions WHERE status = ? ORDER BY updated_at DESC LIMIT ?", status, limit)
	}
	if err != nil {
		return nil, fmt.Errorf("list actions: %w", err)
	}
	defer rows.Close()

	actions := make([]Action, 0)
	for rows.Next() {
		var payload []byte
		if err := rows.Scan(&payload); err != nil {
			return nil, fmt.Errorf("scan action row: %w", err)
		}
		var action Action
		if err := json.Unmarshal(payload, &action); err != nil {
			return nil, fmt.Errorf("decode action row: %w", err)
		}
		actions = append(actions, action)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate action rows: %w", err)
	}
	return actions, nil
}

func stringsTrim(v string) string {
	return strings.TrimSpace(v)
}

func parseRFC3339Unix(v string) (int64, bool) {
	t, err := time.Parse(time.RFC3339, v)
	if err != nil {
		return 0, false
	}
	return t.UTC().Unix(), true
}
