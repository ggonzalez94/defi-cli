//! Action persistence store.
//!
//! Go source: `internal/execution/store.go` (+ `store_test.go`). A sqlite-backed,
//! file-locked store for persisting planned/executing [`crate::action::Action`]
//! records so that `actions list|show`, `submit`, and `status` can reload an
//! action across CLI invocations.
//!
//! Established workspace pattern (see `defi-cache::store`): the sqlite
//! [`rusqlite::Connection`] is `!Sync` and is guarded by a [`Mutex`]; the
//! cross-process advisory lock (`fd_lock::RwLock<File>`, whose `write()` needs
//! `&mut`) is likewise behind a [`Mutex`]. Writes are serialized through both,
//! mirroring Go's single connection + `gofrs/flock`.

use std::fs;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use defi_errors::{Code, Error};
use rusqlite::{params, Connection};

use crate::action::Action;

/// sqlite-backed, file-locked action store (mirrors Go `execution.Store`).
///
/// The sqlite [`Connection`] is `!Sync`, so it is guarded by a [`Mutex`]; the
/// cross-process advisory lock (an `fd_lock::RwLock<File>`, whose `write()`
/// needs `&mut`) is likewise behind a [`Mutex`]. Saves are serialized through
/// these two locks, matching Go's single `*sql.DB` plus `gofrs/flock`.
pub struct Store {
    conn: Mutex<Connection>,
    lock: Mutex<fd_lock::RwLock<File>>,
}

impl Store {
    /// Open (creating dirs + schema) the sqlite action store at `path`, guarded
    /// by a cross-process file lock at `lock_path`.
    ///
    /// Mirrors Go `OpenStore`: creates the parent directories of both paths,
    /// opens the sqlite db, and initializes the `actions` table + the
    /// `idx_actions_status_updated` index.
    pub fn open(path: impl AsRef<Path>, lock_path: impl AsRef<Path>) -> Result<Store, Error> {
        let path = path.as_ref();
        let lock_path = lock_path.as_ref();

        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)
                    .map_err(|e| Error::wrap(Code::Internal, "create action store directory", e))?;
            }
        }
        if let Some(dir) = lock_path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)
                    .map_err(|e| Error::wrap(Code::Internal, "create action lock directory", e))?;
            }
        }

        // Cross-process advisory lock backing file. Held exclusively for schema
        // init below, then on every `save`.
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|e| Error::wrap(Code::Internal, "open action lock file", e))?;
        let file_lock = fd_lock::RwLock::new(lock_file);

        let conn = Connection::open(path)
            .map_err(|e| Error::wrap(Code::Internal, "open action sqlite", e))?;

        // Best-effort durability/concurrency pragmas (internal tuning, not
        // contract); WAL + NORMAL match the Go store.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| Error::wrap(Code::Internal, "init action schema", e))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| Error::wrap(Code::Internal, "init action schema", e))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS actions (\
                action_id TEXT PRIMARY KEY, \
                intent_type TEXT NOT NULL, \
                status TEXT NOT NULL, \
                chain_id TEXT NOT NULL, \
                created_at INTEGER NOT NULL, \
                updated_at INTEGER NOT NULL, \
                payload BLOB NOT NULL\
            );\
            CREATE INDEX IF NOT EXISTS idx_actions_status_updated \
                ON actions(status, updated_at DESC);",
        )
        .map_err(|e| Error::wrap(Code::Internal, "init action schema", e))?;

        Ok(Store {
            conn: Mutex::new(conn),
            lock: Mutex::new(file_lock),
        })
    }

    /// Persist (insert-or-update) an action keyed by its `action_id`.
    ///
    /// Mirrors Go `Save`: errors if `action_id` is blank; otherwise serializes
    /// the action to JSON and upserts on `action_id`, refreshing
    /// `intent_type`, `status`, `chain_id`, `updated_at`, and `payload`.
    pub fn save(&self, action: &Action) -> Result<(), Error> {
        if action.action_id.trim().is_empty() {
            return Err(Error::new(Code::Internal, "save action: missing action id"));
        }

        // Hold the cross-process exclusive lock for the whole write. `_file_guard`
        // keeps the fd-lock write guard alive until the end of this scope.
        let mut lock = self
            .lock
            .lock()
            .map_err(|_| Error::new(Code::Internal, "action store lock poisoned"))?;
        let _file_guard = lock
            .write()
            .map_err(|e| Error::wrap(Code::Internal, "lock action store", e))?;

        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::new(Code::Internal, "action store connection poisoned"))?;

        let payload = serde_json::to_vec(action)
            .map_err(|e| Error::wrap(Code::Internal, "marshal action", e))?;

        // The persisted timestamp columns mirror Go: parse the RFC3339 strings to
        // Unix seconds, falling back to "now" when blank/unparseable. The `status`
        // column stores the lowercase wire value used by `List`'s status filter.
        let created_unix = parse_rfc3339_unix(&action.created_at).unwrap_or_else(now_unix);
        let updated_unix = parse_rfc3339_unix(&action.updated_at).unwrap_or_else(now_unix);
        let status = status_wire(action);

        conn.execute(
            "INSERT INTO actions \
                (action_id, intent_type, status, chain_id, created_at, updated_at, payload) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(action_id) DO UPDATE SET \
                intent_type=excluded.intent_type, \
                status=excluded.status, \
                chain_id=excluded.chain_id, \
                updated_at=excluded.updated_at, \
                payload=excluded.payload",
            params![
                action.action_id,
                action.intent_type,
                status,
                action.chain_id,
                created_unix,
                updated_unix,
                payload
            ],
        )
        .map_err(|e| Error::wrap(Code::Internal, "save action", e))?;
        Ok(())
    }

    /// Load the action with `action_id`.
    ///
    /// Mirrors Go `Get`: errors (not-found) when no row matches; otherwise
    /// decodes the stored JSON payload back into an [`Action`].
    pub fn get(&self, action_id: &str) -> Result<Action, Error> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::new(Code::Internal, "action store connection poisoned"))?;

        let payload: Vec<u8> = conn
            .query_row(
                "SELECT payload FROM actions WHERE action_id = ?1",
                params![action_id],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    Error::new(Code::Internal, format!("action not found: {action_id}"))
                }
                other => Error::wrap(Code::Internal, "read action", other),
            })?;

        decode_action(&payload)
    }

    /// List actions, most-recently-updated first.
    ///
    /// Mirrors Go `List`: an empty `status` lists all; a non-empty `status`
    /// filters by it. `limit <= 0` defaults to 20. Ordered by `updated_at DESC`.
    pub fn list(&self, status: &str, limit: i64) -> Result<Vec<Action>, Error> {
        let limit = if limit <= 0 { 20 } else { limit };

        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::new(Code::Internal, "action store connection poisoned"))?;

        let payloads: Vec<Vec<u8>> = if status.trim().is_empty() {
            let mut stmt = conn
                .prepare("SELECT payload FROM actions ORDER BY updated_at DESC LIMIT ?1")
                .map_err(|e| Error::wrap(Code::Internal, "list actions", e))?;
            let rows = stmt
                .query_map(params![limit], |row| row.get::<_, Vec<u8>>(0))
                .map_err(|e| Error::wrap(Code::Internal, "list actions", e))?;
            collect_payloads(rows)?
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT payload FROM actions WHERE status = ?1 \
                     ORDER BY updated_at DESC LIMIT ?2",
                )
                .map_err(|e| Error::wrap(Code::Internal, "list actions", e))?;
            let rows = stmt
                .query_map(params![status, limit], |row| row.get::<_, Vec<u8>>(0))
                .map_err(|e| Error::wrap(Code::Internal, "list actions", e))?;
            collect_payloads(rows)?
        };

        let mut actions = Vec::with_capacity(payloads.len());
        for payload in &payloads {
            actions.push(decode_action(payload)?);
        }
        Ok(actions)
    }
}

/// Drain a rusqlite row iterator of BLOB payloads, surfacing the first scan
/// error as a typed [`Error`] (mirrors Go's `rows.Scan` / `rows.Err` checks).
fn collect_payloads(
    rows: impl Iterator<Item = rusqlite::Result<Vec<u8>>>,
) -> Result<Vec<Vec<u8>>, Error> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| Error::wrap(Code::Internal, "scan action row", e))?);
    }
    Ok(out)
}

/// Decode a stored JSON payload back into an [`Action`] (mirrors Go's
/// `json.Unmarshal` of the `payload` column).
fn decode_action(payload: &[u8]) -> Result<Action, Error> {
    serde_json::from_slice(payload)
        .map_err(|e| Error::wrap(Code::Internal, "decode action payload", e))
}

/// The lowercase wire value of an action's status, used for the `status` filter
/// column. Serializing through serde yields the same lowercase token the Go
/// store wrote (`action.Status` is an `ActionStatus` string constant).
fn status_wire(action: &Action) -> String {
    serde_json::to_value(action.status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Parse an RFC3339 timestamp to Unix seconds, mirroring Go
/// `parseRFC3339Unix`. Returns [`None`] for a blank or unparseable input so the
/// caller can fall back to "now".
fn parse_rfc3339_unix(v: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(v)
        .ok()
        .map(|t| t.timestamp())
}

/// Current time as a Unix timestamp (seconds). Pre-epoch clocks clamp to 0.
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// =============================================================================
// SUCCESS CRITERIA (RED phase — tests written before implementation)
//
// This module (Go source: internal/execution/store.go) owns persistence of
// execution Actions: the sqlite-backed, file-locked store behind
// `actions list|show`, `submit`, and `status`. The Rust port is "correct" iff:
//
//   1. OPEN CREATES PARENT DIRECTORIES + USABLE STORE. `Store::open(db, lock)`
//      succeeds even when the parent directories of `db` and `lock` do not yet
//      exist (it `mkdir -p`s them, like Go `OpenStore`'s two `MkdirAll`s), and
//      returns a store that is immediately usable for save/get/list.
//
//   2. SAVE → GET ROUND-TRIP. After `save(&action)`, `get(action_id)` returns
//      an action whose `action_id`, `intent_type`, and all other persisted
//      fields equal the saved action. (Ports Go TestStoreSaveGetList, save+get.)
//
//   3. STEPS + CONSTRAINTS SURVIVE THE ROUND-TRIP. A saved action carrying a
//      populated `ActionStep` (id/type/status/chain/target/data/value) and
//      non-default `Constraints` (slippage_bps, simulate) comes back byte-equal
//      via the JSON payload column. (Strengthens Go TestStoreSaveGetList, which
//      only re-checks id/intent.)
//
//   4. WALLET / EXECUTION-BACKEND METADATA SURVIVES. `execution_backend`,
//      `wallet_id`, and `wallet_name` round-trip through save→get unchanged.
//      (Ports Go TestStoreSaveGetPreservesExecutionBackend.)
//
//   5. SAVE IS AN UPSERT (no duplicate-key insert). Re-saving the SAME
//      `action_id` with a changed `status` updates the existing row in place:
//      a later `get` reflects the new status and `list` shows exactly one row
//      for that id (not two). (Ports the update half of Go TestStoreSaveGetList,
//      which re-saves with ActionStatusCompleted and expects len==1.)
//
//   6. SAVE REJECTS A BLANK ACTION ID. `save` of an action whose `action_id`
//      is empty (or whitespace-only) returns an error and persists nothing.
//      (Ports Go `Save`'s `stringsTrim(action.ActionID) == "" -> error`.)
//
//   7. GET OF A MISSING ACTION ERRORS. `get("missing")` returns an error (the
//      not-found case), not an empty/default action. (Ports Go
//      TestStoreGetMissingAction.)
//
//   8. LIST FILTERS BY STATUS. After saving actions with mixed statuses,
//      `list("completed", 10)` returns only completed actions; `list("", 10)`
//      (empty status) returns all of them. (Ports the list half of Go
//      TestStoreSaveGetList + Go `List`'s empty-vs-non-empty status branch.)
//
//   9. LIST ORDERS BY updated_at DESCENDING. With several actions whose
//      `updated_at` timestamps differ, `list("", n)` returns them newest-first.
//      (Asserts Go `List`'s `ORDER BY updated_at DESC`, which no Go test covers
//      directly but is load-bearing for `actions list`.)
//
//  10. LIST DEFAULT + RESPECTED LIMIT. `limit <= 0` defaults to 20 (Go's
//      `if limit <= 0 { limit = 20 }`), and a positive `limit` caps the result
//      count. With >20 actions saved and limit 0, at most 20 are returned;
//      with limit 3, at most 3.
//
//  11. EMPTY-STORE LIST IS Ok(EMPTY). `list("", 10)` on a fresh store returns
//      an empty Vec without error (Go returns `make([]Action,0)`, never nil),
//      and `list("completed", 10)` with no matches returns an empty Vec too.
//
// SKIPPED Go internals (would calcify non-idiomatic shape into Rust):
//   - exact PRAGMA statements (journal_mode=WAL, synchronous=NORMAL) and the
//     literal index name/SQL text: internal tuning, not observable contract;
//     covered indirectly by the behavioral round-trip + ordering criteria.
//   - the 5s flock TryLockContext timeout value: an implementation detail of
//     the cross-process lock; the OBSERVABLE contract (saves serialize and are
//     immediately readable) is what matters.
//   - storing created_at/updated_at as separate INTEGER columns vs. relying on
//     the JSON payload: an internal storage choice. Criteria assert the payload
//     round-trip + the updated_at-desc ORDER, not the column layout.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{
        Action, ActionStatus, ActionStep, Constraints, ExecutionBackend, StepStatus, StepType,
    };
    use tempfile::TempDir;

    /// Build a minimal valid action directly (no dependency on the sibling
    /// `action` module's constructors, so these tests fail on STORE behavior,
    /// not on a missing `Action::new`).
    fn make_action(action_id: &str, intent: &str, status: ActionStatus) -> Action {
        Action {
            action_id: action_id.to_string(),
            intent_type: intent.to_string(),
            provider: String::new(),
            status,
            chain_id: "eip155:167000".to_string(),
            from_address: String::new(),
            wallet_id: String::new(),
            wallet_name: String::new(),
            execution_backend: None,
            to_address: String::new(),
            input_amount: String::new(),
            created_at: "2026-05-28T00:00:00Z".to_string(),
            updated_at: "2026-05-28T00:00:00Z".to_string(),
            constraints: Constraints::default(),
            steps: Vec::new(),
            metadata: None,
            provider_data: None,
        }
    }

    /// Open a store under a NESTED, not-yet-existing directory so criterion 1
    /// (mkdir -p) is exercised on every test.
    fn open_store(tmp: &TempDir) -> Store {
        let db = tmp.path().join("nested").join("actions.db");
        let lock = tmp.path().join("nested").join("actions.lock");
        Store::open(&db, &lock).expect("OpenStore should create dirs + schema")
    }

    // ---- Criterion 1: open creates parent dirs + a usable store ----------

    #[test]
    fn open_creates_missing_directories() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a").join("b").join("c").join("actions.db");
        let lock = tmp.path().join("x").join("y").join("actions.lock");
        let store = Store::open(&db, &lock).expect("open must mkdir -p parent dirs");
        // Usable immediately: an empty list with no error.
        let all = store.list("", 10).expect("list on fresh store");
        assert!(all.is_empty(), "fresh store lists empty");
    }

    // ---- Criterion 2 + 3: save -> get round-trip incl. steps/constraints --

    #[test]
    fn save_then_get_round_trips_all_fields() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let mut action = make_action("act_roundtrip", "swap", ActionStatus::Planned);
        action.constraints = Constraints {
            slippage_bps: 50,
            deadline: String::new(),
            simulate: true,
        };
        action.steps.push(ActionStep {
            step_id: "swap-1".into(),
            step_type: StepType::Swap,
            status: StepStatus::Pending,
            chain_id: "eip155:167000".into(),
            rpc_url: String::new(),
            description: String::new(),
            target: "0x0000000000000000000000000000000000000001".into(),
            data: "0x".into(),
            value: "0".into(),
            calls: Vec::new(),
            expected_outputs: None,
            tx_hash: String::new(),
            error: String::new(),
        });

        store.save(&action).expect("save");
        let got = store.get("act_roundtrip").expect("get saved action");

        assert_eq!(got.action_id, "act_roundtrip");
        assert_eq!(got.intent_type, "swap");
        assert_eq!(got.status, ActionStatus::Planned);
        assert_eq!(got.chain_id, "eip155:167000");
        assert_eq!(got.constraints.slippage_bps, 50);
        assert!(got.constraints.simulate);
        assert_eq!(got.steps.len(), 1, "step must survive the round-trip");
        assert_eq!(got.steps[0].step_id, "swap-1");
        assert_eq!(got.steps[0].step_type, StepType::Swap);
        assert_eq!(
            got.steps[0].target,
            "0x0000000000000000000000000000000000000001"
        );
        assert_eq!(got.steps[0].value, "0");
    }

    // ---- Criterion 4: wallet / execution-backend metadata round-trips ----

    #[test]
    fn save_get_preserves_execution_backend_and_wallet() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let mut action = make_action("act_wallet", "swap", ActionStatus::Planned);
        action.execution_backend = Some(ExecutionBackend::Tempo);
        action.wallet_id = "wallet-tempo".into();
        action.wallet_name = "Tempo Agent Wallet".into();

        store.save(&action).expect("save");
        let got = store.get("act_wallet").expect("get");

        assert_eq!(got.execution_backend, Some(ExecutionBackend::Tempo));
        assert_eq!(got.wallet_id, "wallet-tempo");
        assert_eq!(got.wallet_name, "Tempo Agent Wallet");
    }

    // ---- Criterion 5: save is an upsert (status update, single row) -------

    #[test]
    fn save_upserts_existing_action_in_place() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let action = make_action("act_upsert", "swap", ActionStatus::Planned);
        store.save(&action).expect("save planned");

        // Re-save the SAME id with a new status.
        let mut updated = store.get("act_upsert").expect("get back");
        updated.status = ActionStatus::Completed;
        store.save(&updated).expect("save completed (upsert)");

        let got = store.get("act_upsert").expect("get after upsert");
        assert_eq!(
            got.status,
            ActionStatus::Completed,
            "status updated in place"
        );

        // Exactly one row for this id (no duplicate insert).
        let completed = store.list("completed", 10).expect("list completed");
        assert_eq!(completed.len(), 1, "upsert must not create a second row");
        assert_eq!(completed[0].action_id, "act_upsert");
    }

    // ---- Criterion 6: save rejects a blank action id ---------------------

    #[test]
    fn save_rejects_blank_action_id() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let blank = make_action("", "swap", ActionStatus::Planned);
        assert!(store.save(&blank).is_err(), "empty action id must error");

        let whitespace = make_action("   ", "swap", ActionStatus::Planned);
        assert!(
            store.save(&whitespace).is_err(),
            "whitespace-only action id must error"
        );

        // Nothing was persisted.
        let all = store.list("", 50).expect("list");
        assert!(all.is_empty(), "blank-id saves must persist nothing");
    }

    // ---- Criterion 7: get of a missing action errors ---------------------

    #[test]
    fn get_missing_action_errors() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp);
        assert!(store.get("missing").is_err(), "missing action must error");
    }

    // ---- Criterion 8: list filters by status -----------------------------

    #[test]
    fn list_filters_by_status() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp);

        store
            .save(&make_action("a-planned", "swap", ActionStatus::Planned))
            .expect("save 1");
        store
            .save(&make_action("b-completed", "swap", ActionStatus::Completed))
            .expect("save 2");
        store
            .save(&make_action(
                "c-completed",
                "lend_supply",
                ActionStatus::Completed,
            ))
            .expect("save 3");

        let completed = store.list("completed", 10).expect("list completed");
        assert_eq!(completed.len(), 2, "only completed actions");
        assert!(completed
            .iter()
            .all(|a| a.status == ActionStatus::Completed));

        let all = store.list("", 10).expect("list all");
        assert_eq!(all.len(), 3, "empty status lists everything");
    }

    // ---- Criterion 9: list orders by updated_at DESC ---------------------

    #[test]
    fn list_orders_by_updated_at_descending() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let mut older = make_action("older", "swap", ActionStatus::Planned);
        older.updated_at = "2026-05-28T00:00:01Z".into();
        let mut middle = make_action("middle", "swap", ActionStatus::Planned);
        middle.updated_at = "2026-05-28T00:00:02Z".into();
        let mut newer = make_action("newer", "swap", ActionStatus::Planned);
        newer.updated_at = "2026-05-28T00:00:03Z".into();

        // Save out of order; the store must order by updated_at, not insertion.
        store.save(&older).expect("save older");
        store.save(&newer).expect("save newer");
        store.save(&middle).expect("save middle");

        let listed = store.list("", 10).expect("list");
        let ids: Vec<&str> = listed.iter().map(|a| a.action_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["newer", "middle", "older"],
            "actions must be newest-first by updated_at"
        );
    }

    // ---- Criterion 10: default + respected limit -------------------------

    #[test]
    fn list_limit_defaults_to_twenty_and_caps_results() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp);

        for i in 0..30 {
            let mut a = make_action(&format!("act-{i:02}"), "swap", ActionStatus::Planned);
            // Distinct, increasing updated_at so ordering is deterministic.
            a.updated_at = format!("2026-05-28T00:{:02}:00Z", i);
            store.save(&a).expect("save");
        }

        let defaulted = store.list("", 0).expect("list default limit");
        assert_eq!(defaulted.len(), 20, "limit <= 0 defaults to 20");

        let capped = store.list("", 3).expect("list limit 3");
        assert_eq!(capped.len(), 3, "positive limit caps result count");
    }

    // ---- Criterion 11: empty-store list is Ok(empty) ---------------------

    #[test]
    fn list_on_empty_store_returns_empty_not_error() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp);

        let all = store.list("", 10).expect("list all on empty store");
        assert!(all.is_empty(), "all-list on empty store is empty");

        let filtered = store
            .list("completed", 10)
            .expect("filtered list on empty store");
        assert!(filtered.is_empty(), "no-match filtered list is empty");
    }
}
