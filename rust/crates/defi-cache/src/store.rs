//! sqlite-backed cache store.
//!
//! Public interface (mirrors `internal/cache/cache.go`):
//!   - [`Store::open`] / `Drop` (RAII close)
//!   - [`Store::get`] → [`CacheResult`]
//!   - [`Store::set`]
//!   - [`Store::prune`]
//!   - [`prune_max_stale`] (1h floor helper)
//!
//! Freshness/staleness contract (spec §2.5): a fresh hit (`age <= ttl`) skips
//! provider calls; expired entries re-fetch; stale entries are served only
//! within `max_stale` on temporary provider failure.

use std::fs;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use defi_errors::{Code, Error};
use rusqlite::{params, Connection, OptionalExtension};

/// How long sqlite waits on a locked database before erroring. Matches Go's
/// `PRAGMA busy_timeout=5000`, so concurrent writers serialized by the file
/// lock never surface a "database is locked" error.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Outcome of a cache lookup (mirrors Go `cache.Result`).
///
/// Field declaration order mirrors the Go struct so any future serde-derived
/// rendering preserves contract field order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheResult {
    /// Whether an entry exists for the key.
    pub hit: bool,
    /// Raw stored bytes (empty on miss).
    pub value: Vec<u8>,
    /// Age of the entry (now - created_at); clamped to zero if negative.
    pub age: Duration,
    /// `age > ttl`.
    pub stale: bool,
    /// `stale && age > ttl + max_stale`.
    pub too_stale: bool,
}

/// sqlite-backed, file-locked cache store (mirrors Go `cache.Store`).
///
/// The sqlite [`Connection`] is `!Sync`, so it is guarded by a [`Mutex`]; the
/// cross-process advisory lock (an `fd_lock::RwLock<File>`, whose `write()`
/// needs `&mut`) is likewise behind a [`Mutex`]. Concurrency across threads or
/// processes is serialized through these two locks, matching Go's single
/// connection (`SetMaxOpenConns(1)`) plus `gofrs/flock`.
pub struct Store {
    conn: Mutex<Connection>,
    lock: Mutex<fd_lock::RwLock<File>>,
}

impl Store {
    /// Open (creating dirs + schema) the sqlite cache at `path`, guarded by a
    /// cross-process file lock at `lock_path`. Runs a startup prune using
    /// [`prune_max_stale`]`(max_stale)`.
    pub fn open(
        path: impl AsRef<Path>,
        lock_path: impl AsRef<Path>,
        max_stale: Duration,
    ) -> Result<Store, Error> {
        let path = path.as_ref();
        let lock_path = lock_path.as_ref();

        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)
                    .map_err(|e| Error::wrap(Code::Internal, "create cache directory", e))?;
            }
        }
        if let Some(dir) = lock_path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)
                    .map_err(|e| Error::wrap(Code::Internal, "create lock directory", e))?;
            }
        }

        // Cross-process advisory lock backing file. Held exclusively for the
        // duration of schema init + startup prune below.
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|e| Error::wrap(Code::Internal, "open lock file", e))?;
        let mut file_lock = fd_lock::RwLock::new(lock_file);
        let guard = file_lock
            .write()
            .map_err(|e| Error::wrap(Code::Internal, "lock cache", e))?;

        let conn = Connection::open(path)
            .map_err(|e| Error::wrap(Code::Internal, "open sqlite cache", e))?;
        conn.busy_timeout(BUSY_TIMEOUT)
            .map_err(|e| Error::wrap(Code::Internal, "init cache schema", e))?;

        // Best-effort durability/concurrency pragmas (internal tuning, not
        // contract); WAL + NORMAL match the Go store.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| Error::wrap(Code::Internal, "init cache schema", e))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| Error::wrap(Code::Internal, "init cache schema", e))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cache_entries (\
                key TEXT PRIMARY KEY, \
                value BLOB NOT NULL, \
                created_at INTEGER NOT NULL, \
                ttl_seconds INTEGER NOT NULL\
            );",
        )
        .map_err(|e| Error::wrap(Code::Internal, "init cache schema", e))?;

        // Startup prune: discard entries past both TTL and the floored
        // max_stale window so the db cannot grow unbounded, while preserving
        // the stale fallback window. Best-effort — a prune failure must not
        // block cache usage (matches Go's `_ = store.pruneUnlocked(...)`).
        let _ = prune_in_conn(&conn, prune_max_stale(max_stale));

        // Release the cross-process lock; the connection lives on.
        drop(guard);

        Ok(Store {
            conn: Mutex::new(conn),
            lock: Mutex::new(file_lock),
        })
    }

    /// Look up `key`, computing freshness/staleness against `max_stale`.
    ///
    /// A miss returns `hit=false` with no error (mirrors Go's `sql.ErrNoRows`
    /// → `Result{Hit:false}`). Does not take the file lock; sqlite's busy
    /// timeout handles a concurrent writer.
    pub fn get(&self, key: &str, max_stale: Duration) -> Result<CacheResult, Error> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::new(Code::Internal, "cache connection poisoned"))?;

        let row: Option<(Vec<u8>, i64, i64)> = conn
            .query_row(
                "SELECT value, created_at, ttl_seconds FROM cache_entries WHERE key = ?1",
                params![key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|e| Error::wrap(Code::Internal, "cache read", e))?;

        let Some((value, created_unix, ttl_seconds)) = row else {
            return Ok(CacheResult {
                hit: false,
                value: Vec::new(),
                age: Duration::ZERO,
                stale: false,
                too_stale: false,
            });
        };

        let age = age_since(created_unix);
        let ttl = Duration::from_secs(ttl_seconds.max(0) as u64);
        let stale = age > ttl;
        // Go: `stale && maxStale >= 0 && age > ttl+maxStale`. `Duration` is
        // never negative, so the `>= 0` guard is always satisfied here.
        let too_stale = stale && age > ttl.saturating_add(max_stale);

        Ok(CacheResult {
            hit: true,
            value,
            age,
            stale,
            too_stale,
        })
    }

    /// Upsert `value` for `key` with the given `ttl` (floored to 1 second).
    ///
    /// A `ttl <= 0` is stored as `1` second (Go floors to 1) so the entry is a
    /// fresh hit on write rather than immediately expired.
    pub fn set(&self, key: &str, value: &[u8], ttl: Duration) -> Result<(), Error> {
        // Hold the cross-process exclusive lock for the whole write. `_f<...>`
        // bindings keep both the in-process mutex guard and the fd-lock write
        // guard alive until the end of this scope.
        let mut lock = self
            .lock
            .lock()
            .map_err(|_| Error::new(Code::Internal, "cache lock poisoned"))?;
        let _file_guard = lock
            .write()
            .map_err(|e| Error::wrap(Code::Internal, "lock cache", e))?;

        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::new(Code::Internal, "cache connection poisoned"))?;

        let created_unix = now_unix();
        let mut ttl_seconds = ttl.as_secs() as i64;
        if ttl_seconds <= 0 {
            ttl_seconds = 1;
        }

        conn.execute(
            "INSERT INTO cache_entries (key, value, created_at, ttl_seconds) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(key) DO UPDATE SET \
                value=excluded.value, \
                created_at=excluded.created_at, \
                ttl_seconds=excluded.ttl_seconds",
            params![key, value, created_unix, ttl_seconds],
        )
        .map_err(|e| Error::wrap(Code::Internal, "cache write", e))?;
        Ok(())
    }

    /// Delete entries past both their TTL and the `max_stale` fallback window.
    ///
    /// Entries within `(ttl, ttl+max_stale]` are preserved so the caller can
    /// serve them during temporary provider failures.
    pub fn prune(&self, max_stale: Duration) -> Result<(), Error> {
        let mut lock = self
            .lock
            .lock()
            .map_err(|_| Error::new(Code::Internal, "cache lock poisoned"))?;
        let _file_guard = lock
            .write()
            .map_err(|e| Error::wrap(Code::Internal, "lock cache", e))?;

        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::new(Code::Internal, "cache connection poisoned"))?;
        prune_in_conn(&conn, max_stale)
    }
}

/// Delete cache entries past both TTL and the `max_stale` fallback window.
///
/// Prune rule (Go): `DELETE WHERE created_at + ttl_seconds + max_stale_sec < now`
/// with `max_stale_sec` floored at 0.
fn prune_in_conn(conn: &Connection, max_stale: Duration) -> Result<(), Error> {
    let max_stale_sec = max_stale.as_secs() as i64;
    let now = now_unix();
    conn.execute(
        "DELETE FROM cache_entries WHERE created_at + ttl_seconds + ?1 < ?2",
        params![max_stale_sec, now],
    )
    .map_err(|e| Error::wrap(Code::Internal, "prune cache", e))?;
    Ok(())
}

/// Floor `max_stale` at 1 hour for startup auto-prune (mirrors Go
/// `pruneMaxStale`): a small / zero `--max-stale` must not purge all stale rows.
pub fn prune_max_stale(max_stale: Duration) -> Duration {
    const PRUNE_FLOOR: Duration = Duration::from_secs(3600);
    if max_stale < PRUNE_FLOOR {
        PRUNE_FLOOR
    } else {
        max_stale
    }
}

/// Current time as a Unix timestamp (seconds). Pre-epoch clocks clamp to 0.
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Age (now - created_at) clamped to zero if negative, mirroring Go's
/// `created := time.Unix(createdUnix, 0); age := time.Since(created)`.
///
/// `created_at` is stored at whole-second granularity, but the comparison
/// against "now" keeps sub-second precision — so an entry written `1.2s` ago
/// with a `1s` TTL reads as stale, exactly as the Go store does. (Truncating
/// "now" to whole seconds would lose that and under-report age.)
fn age_since(created_unix: i64) -> Duration {
    let created = if created_unix >= 0 {
        UNIX_EPOCH + Duration::from_secs(created_unix as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs((-created_unix) as u64)
    };
    SystemTime::now()
        .duration_since(created)
        .unwrap_or(Duration::ZERO)
}

// =============================================================================
// SUCCESS CRITERIA (RED phase — tests written before implementation)
//
// This module (Go source: internal/cache/cache.go) owns the sqlite cache
// freshness/staleness contract (spec §2.5 behavioral invariant: "fresh hit
// (age <= ttl) skips provider calls; expired re-fetches; stale served only
// within max_stale on temporary provider failure"). The Rust port is "correct"
// iff:
//
//   1. SET → GET ROUND-TRIP. After `set(k, v, ttl)`, an immediate `get(k, _)`
//      returns hit=true, value==v, stale=false, too_stale=false, and a small
//      non-negative age. (Ports Go TestCacheSetGetFreshAndStale, fresh half.)
//
//   2. FRESHNESS BOUNDARY. An entry whose age exceeds its ttl but is still
//      within max_stale reports stale=true, too_stale=false. (Ports Go
//      TestCacheSetGetFreshAndStale, stale half.)
//        - fresh:  age <= ttl  → stale=false
//        - stale:  age >  ttl  → stale=true
//
//   3. TOO-STALE BOUNDARY. An entry past ttl AND past ttl+max_stale reports
//      too_stale=true. With max_stale very small (10ms) and the entry well past
//      ttl, the lookup is too_stale. (Ports Go TestCacheTooStale.)
//      Exact rule (Go): stale = age > ttl;
//                       too_stale = stale && max_stale >= 0 && age > ttl + max_stale.
//
//   4. MISS. `get` of an absent key returns hit=false, no error. (Implied by
//      Go's sql.ErrNoRows → Result{Hit:false}; asserted by the prune tests.)
//
//   5. TTL FLOOR. `set` with ttl <= 0 stores ttl_seconds=1 (Go floors to 1),
//      so the entry is initially a hit and becomes stale after ~1s — it is NOT
//      treated as already-expired-on-write. (Fresh spec-driven: covers the
//      `ttlSeconds <= 0 { ttlSeconds = 1 }` branch in Set.)
//
//   6. PRUNE REMOVES EXPIRED. After ttl fully expires, `prune(0)` evicts the
//      entry (subsequent get → miss); a long-TTL entry survives. (Ports Go
//      TestPruneRemovesExpiredEntries.) Prune rule (Go):
//        DELETE WHERE created_at + ttl_seconds + max_stale_sec < now
//        (max_stale_sec floored at 0).
//
//   7. PRUNE PRESERVES STALE WITHIN MAX_STALE. After ttl expires, `prune(big)`
//      keeps the (now-stale) entry; a later `prune(0)` evicts it. (Ports Go
//      TestPrunePreservesStaleWithinMaxStale.)
//
//   8. PRUNE_MAX_STALE FLOOR. prune_max_stale floors at 1h: {0, 30s, 59m} → 1h;
//      {1h, 2h} pass through unchanged. (Ports Go TestPruneMaxStaleFloor — table.)
//
//   9. OPEN STARTUP-PRUNE USES THE FLOOR. Opening with max_stale=0 must NOT
//      evict a recently-expired (short-TTL) stale entry, because the startup
//      prune floors max_stale to 1h. (Ports Go TestOpenWithZeroMaxStalePreservesStale.)
//
//  10. CONCURRENT OPEN+SET (cross-process file lock). Many concurrent
//      Open/Set/Get cycles against the same db+lock path all succeed with no
//      "database is locked" errors and every Set is immediately readable.
//      (Ports Go TestCacheConcurrentOpenAndSet; uses threads since the lock is
//      cross-process/cross-thread.)
//
// SKIPPED Go internals (would calcify non-idiomatic shape into Rust):
//   - the sqlite busy-retry backoff loop (withSQLiteRetry / isSQLiteBusyErr):
//     an implementation detail; criterion 10 asserts the OBSERVABLE outcome
//     (no lock errors under contention) instead of the retry mechanism.
//   - exact PRAGMA statements / connection-pool tuning (SetMaxOpenConns, WAL):
//     internal tuning, not contract.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Open a store in a fresh temp dir with a generous startup max_stale so the
    /// startup prune never interferes with the test's own entries.
    fn open_store(tmp: &TempDir, startup_max_stale: Duration) -> Store {
        let db = tmp.path().join("cache.db");
        let lock = tmp.path().join("cache.lock");
        Store::open(&db, &lock, startup_max_stale).expect("open cache store")
    }

    // ---- Criterion 1 + 2: fresh then stale within budget -----------------

    #[test]
    fn set_get_fresh_then_stale() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp, Duration::from_secs(300));

        store
            .set("k1", br#"{"v":1}"#, Duration::from_secs(1))
            .expect("set");

        let res = store.get("k1", Duration::from_secs(5)).expect("get fresh");
        assert!(res.hit, "expected fresh hit");
        assert!(!res.stale, "expected not stale immediately after set");
        assert!(!res.too_stale, "fresh entry is never too_stale");
        assert_eq!(res.value, br#"{"v":1}"#.to_vec(), "value round-trips");

        // Let the 1s TTL lapse but stay within the 5s max_stale budget.
        thread::sleep(Duration::from_millis(1200));
        let res = store.get("k1", Duration::from_secs(5)).expect("get stale");
        assert!(res.hit, "stale entry is still a hit");
        assert!(res.stale, "expected stale after ttl elapsed");
        assert!(!res.too_stale, "expected within max_stale budget");
    }

    // ---- Criterion 3: too stale ------------------------------------------

    #[test]
    fn get_reports_too_stale_past_max_stale() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp, Duration::from_secs(300));

        store
            .set("k2", br#"{"v":2}"#, Duration::from_secs(1))
            .expect("set");
        thread::sleep(Duration::from_millis(1300));

        // max_stale = 10ms, entry is ~300ms past ttl → too_stale.
        let res = store.get("k2", Duration::from_millis(10)).expect("get");
        assert!(res.hit, "entry still present");
        assert!(res.stale, "must be stale");
        assert!(res.too_stale, "expected too_stale past max_stale window");
    }

    // ---- Criterion 4: miss -----------------------------------------------

    #[test]
    fn get_absent_key_is_miss() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp, Duration::from_secs(300));

        let res = store
            .get("nonexistent", Duration::from_secs(60))
            .expect("get miss must not error");
        assert!(!res.hit, "absent key must be a miss");
        assert!(res.value.is_empty(), "miss carries no value");
    }

    // ---- Criterion 5: ttl floor (ttl <= 0 stored as 1s, not pre-expired) -

    #[test]
    fn set_with_zero_ttl_is_floored_to_one_second() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp, Duration::from_secs(300));

        store
            .set("zero-ttl", br#""x""#, Duration::ZERO)
            .expect("set zero ttl");

        // Immediately after write the entry is a fresh hit (ttl floored to 1s),
        // NOT already expired.
        let res = store.get("zero-ttl", Duration::from_secs(60)).expect("get");
        assert!(res.hit, "zero-ttl entry must be a hit (ttl floored to 1s)");
        assert!(!res.stale, "zero-ttl entry must be fresh right after write");
    }

    // ---- Criterion 5b: upsert overwrites value AND re-freshens -----------
    //
    // The runner re-`set`s a key after a successful re-fetch to refresh an
    // expired/stale entry. The Go store relies on `ON CONFLICT(key) DO UPDATE`
    // resetting BOTH value and created_at; if the Rust upsert only inserted (or
    // failed to reset created_at), a stale entry would never become fresh again
    // and the cache-freshness contract (spec §2.5) would silently break. No Go
    // test covers this branch, so assert it explicitly here.

    #[test]
    fn set_upserts_value_and_refreshens_stale_entry() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp, Duration::from_secs(600));

        // First write with a 1s TTL, then let it go stale.
        store
            .set("k", br#"{"v":1}"#, Duration::from_secs(1))
            .expect("set v1");
        thread::sleep(Duration::from_millis(1200));
        let res = store.get("k", Duration::from_secs(600)).expect("get stale");
        assert!(res.hit && res.stale, "precondition: entry is stale");
        assert_eq!(res.value, br#"{"v":1}"#.to_vec(), "value before upsert");

        // Re-set the SAME key with new bytes and a fresh longer TTL.
        store
            .set("k", br#"{"v":2}"#, Duration::from_secs(60))
            .expect("set v2 (upsert)");

        let res = store.get("k", Duration::from_secs(600)).expect("get fresh");
        assert!(res.hit, "upserted entry is a hit");
        assert_eq!(
            res.value,
            br#"{"v":2}"#.to_vec(),
            "upsert overwrote the value (no duplicate-key insert)"
        );
        assert!(
            !res.stale,
            "upsert reset created_at + ttl, so the entry is fresh again"
        );
    }

    // ---- Criterion 5c: opaque BLOB round-trip (non-UTF-8 bytes) -----------
    //
    // The cache stores opaque payloads in a BLOB column. Callers persist JSON
    // today, but the store must not corrupt arbitrary bytes (e.g. if it bound
    // the value as TEXT/String). Assert a non-UTF-8 / embedded-NUL payload
    // survives a round-trip byte-for-byte.

    #[test]
    fn set_get_preserves_arbitrary_binary_bytes() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp, Duration::from_secs(300));

        let payload: &[u8] = &[0x00, 0xFF, 0x10, 0x80, b'a', 0x00, 0xC3, 0x28];
        store.set("bin", payload, Duration::from_secs(60)).expect("set");

        let res = store.get("bin", Duration::from_secs(60)).expect("get");
        assert!(res.hit, "binary entry is a hit");
        assert_eq!(res.value, payload.to_vec(), "binary bytes round-trip intact");
    }

    // ---- Criterion 6: prune removes expired ------------------------------

    #[test]
    fn prune_removes_expired_keeps_long_lived() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp, Duration::from_secs(300));

        store
            .set("prunable", br#""old""#, Duration::from_secs(1))
            .expect("set prunable");
        store
            .set("keeper", br#""keep""#, Duration::from_secs(3600))
            .expect("set keeper");

        // 2100ms guarantees a full Unix-second has elapsed past the 1s TTL.
        thread::sleep(Duration::from_millis(2100));
        store.prune(Duration::ZERO).expect("prune");

        let res = store
            .get("prunable", Duration::from_secs(3600))
            .expect("get prunable");
        assert!(!res.hit, "expired entry must be evicted by prune(0)");

        let res = store
            .get("keeper", Duration::from_secs(3600))
            .expect("get keeper");
        assert!(res.hit, "long-lived entry must survive prune");
    }

    // ---- Criterion 7: prune preserves stale within max_stale -------------

    #[test]
    fn prune_preserves_stale_within_max_stale_then_evicts() {
        let tmp = TempDir::new().unwrap();
        let store = open_store(&tmp, Duration::from_secs(600));

        store
            .set("stale-ok", br#""fallback""#, Duration::from_secs(1))
            .expect("set");
        thread::sleep(Duration::from_millis(2100));

        // Big max_stale window: the stale entry survives.
        store.prune(Duration::from_secs(600)).expect("prune big");
        let res = store
            .get("stale-ok", Duration::from_secs(600))
            .expect("get after big prune");
        assert!(res.hit, "stale entry must survive within max_stale window");
        assert!(res.stale, "entry is stale");
        assert!(!res.too_stale, "still within max_stale");

        // Zero max_stale: the stale entry is now evicted.
        store.prune(Duration::ZERO).expect("prune zero");
        let res = store
            .get("stale-ok", Duration::from_secs(600))
            .expect("get after zero prune");
        assert!(!res.hit, "stale entry evicted after prune(0)");
    }

    // ---- Criterion 8: prune_max_stale floor (table) ----------------------

    #[test]
    fn prune_max_stale_floors_at_one_hour() {
        let hour = Duration::from_secs(3600);
        let cases: &[(Duration, Duration)] = &[
            (Duration::ZERO, hour),
            (Duration::from_secs(30), hour),
            (Duration::from_secs(59 * 60), hour),
            (hour, hour),
            (Duration::from_secs(2 * 3600), Duration::from_secs(2 * 3600)),
        ];
        for (input, expected) in cases {
            assert_eq!(
                prune_max_stale(*input),
                *expected,
                "prune_max_stale({input:?})"
            );
        }
    }

    // ---- Criterion 9: open startup-prune respects the floor --------------

    #[test]
    fn open_with_zero_max_stale_preserves_recently_expired() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("cache.db");
        let lock = tmp.path().join("cache.lock");

        {
            let store = Store::open(&db, &lock, Duration::from_secs(600)).expect("open big");
            store
                .set("fragile", br#""data""#, Duration::from_secs(1))
                .expect("set");
        } // close

        thread::sleep(Duration::from_millis(2100));

        // Re-open with max_stale=0; the startup prune floor (1h) must keep the
        // recently-expired stale entry.
        let store2 = Store::open(&db, &lock, Duration::ZERO).expect("reopen zero");
        let res = store2
            .get("fragile", Duration::from_secs(3600))
            .expect("get fragile");
        assert!(
            res.hit,
            "stale entry must survive startup prune with max_stale=0 (1h floor)"
        );
    }

    // ---- Criterion 10: concurrent open + set under the file lock ---------

    #[test]
    fn concurrent_open_and_set_no_lock_errors() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("cache.db");
        let lock = tmp.path().join("cache.lock");

        const WORKERS: usize = 16;
        const ITERS: usize = 40;

        let (tx, rx) = mpsc::channel::<String>();
        let mut handles = Vec::with_capacity(WORKERS);
        for worker in 0..WORKERS {
            let db = db.clone();
            let lock = lock.clone();
            let tx = tx.clone();
            handles.push(thread::spawn(move || {
                let store = match Store::open(&db, &lock, Duration::from_secs(300)) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = tx.send(format!("worker {worker} open: {e}"));
                        return;
                    }
                };
                for i in 0..ITERS {
                    let key = format!("worker-{worker}-key-{i}");
                    if let Err(e) = store.set(&key, br#"{"ok":true}"#, Duration::from_secs(60)) {
                        let _ = tx.send(format!("worker {worker} set {i}: {e}"));
                        return;
                    }
                    match store.get(&key, Duration::from_secs(60)) {
                        Ok(res) if res.hit => {}
                        Ok(_) => {
                            let _ = tx.send(format!("worker {worker} get {i}: expected hit"));
                            return;
                        }
                        Err(e) => {
                            let _ = tx.send(format!("worker {worker} get {i}: {e}"));
                            return;
                        }
                    }
                }
            }));
        }
        drop(tx);
        for h in handles {
            h.join().expect("worker thread panicked");
        }
        let errs: Vec<String> = rx.iter().collect();
        assert!(errs.is_empty(), "concurrent cache errors: {errs:?}");
    }
}
