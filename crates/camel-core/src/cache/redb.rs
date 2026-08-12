//! Redb-backed persistent cache repository.
//!
//! Mirrors `idempotent/redb_repository.rs`: all redb I/O is offloaded to
//! `tokio::task::spawn_blocking` because `redb::Database` is blocking. Values
//! are `serde_json`-serialized [`CacheEntry`] blobs.
//!
//! # Sweep
//!
//! A background task wakes every `sweep_interval` and reclaims entries whose
//! `expires_at + stale_retention < now`. It is bound to the context's
//! [`CancellationToken`]: when the context shuts down, the sweep exits. The
//! token is context-owned — dropping this repository aborts only the sweep
//! task handle, never the token (cancelling it would tear down the whole
//! context).

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::cache::CacheEntry;
use camel_api::cache::CacheRepository;
use camel_api::cache::CacheStats;
use parking_lot::Mutex;
use redb::ReadableDatabase;
use redb::ReadableTable;
use redb::ReadableTableMetadata;
use redb::TableDefinition;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

// ── Table definition ──────────────────────────────────────────────────────────

/// `key → serde_json(CacheEntry)`. Mirrors the table-definition style of
/// `idempotent/redb_repository.rs`.
const CACHE_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("cache_entries");

// ── Repository ────────────────────────────────────────────────────────────────

/// Redb-backed implementation of [`CacheRepository`].
///
/// All counters are `Arc<AtomicU64>` so the spawned sweep task can update
/// them via cloned references. `sweep_interval` is consumed by the
/// constructor to build the ticker and is not stored.
pub struct RedbCacheRepository {
    name: String,
    db: Arc<redb::Database>,
    stale_retention: Duration,
    max_entries: Option<usize>,
    hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
    evictions: Arc<AtomicU64>,
    /// Best-effort approximation of `table.len()` for stats display.
    /// The authoritative count is `table.len()` inside write transactions,
    /// used for capacity enforcement. Sweep and invalidate decrement via
    /// saturating-fetch-sub to prevent underflow.
    entries: Arc<AtomicU64>,
    /// Context-owned shutdown token. Cloned into the sweep task; never
    /// cancelled by this repository (doing so would shut down the entire
    /// context when a single repo is dropped).
    shutdown_token: CancellationToken,
    sweep_handle: Mutex<Option<JoinHandle<()>>>,
}

impl RedbCacheRepository {
    /// Open (or create) the redb database at `path`, seed the `entries`
    /// counter from `table.len()`, and spawn the background sweep task.
    ///
    /// `shutdown_token` is the **context's** token — binding the sweep to
    /// context shutdown. The whole open sequence runs in
    /// `spawn_blocking` because `redb::Database::create` is blocking.
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        stale_retention: Duration,
        max_entries: Option<usize>,
        sweep_interval: Duration,
        shutdown_token: CancellationToken,
    ) -> Result<Self, CamelError> {
        let name = name.into();
        let path: PathBuf = path.into();
        let path_for_db = path.clone();
        let (db, initial_len) = tokio::task::spawn_blocking(move || {
            if let Some(parent) = path_for_db.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CamelError::Io(format!("redb create_dir_all: {e}")))?;
            }
            let db = redb::Database::create(&path_for_db)
                .map_err(|e| CamelError::Io(format!("redb open: {e}")))?;
            // Create the table on first open AND read the persisted entry
            // count in a single write txn so the counter survives reopen.
            let len = {
                let wtx = db
                    .begin_write()
                    .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
                // `table` borrows `wtx` mutably, so it must drop before
                // `wtx.commit()` (which moves `wtx`). Read `len` then drop.
                let len = {
                    let table = wtx
                        .open_table(CACHE_TABLE)
                        .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
                    table
                        .len()
                        .map_err(|e| CamelError::Io(format!("redb len: {e}")))?
                };
                wtx.commit()
                    .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
                len
            };
            Ok::<_, CamelError>((Arc::new(db), len))
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))??;

        let hits = Arc::new(AtomicU64::new(0));
        let misses = Arc::new(AtomicU64::new(0));
        let evictions = Arc::new(AtomicU64::new(0));
        let entries = Arc::new(AtomicU64::new(initial_len));

        // Spawn the sweep loop. All shared state is captured as cloned Arcs;
        // the context token clone drives termination.
        let db_clone = Arc::clone(&db);
        let evictions_clone = Arc::clone(&evictions);
        let entries_clone = Arc::clone(&entries);
        let token_clone = shutdown_token.clone();
        let retention = stale_retention;
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(sweep_interval);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let db = Arc::clone(&db_clone);
                        let reclaimed = tokio::task::spawn_blocking(move || {
                            sweep_reclaim(&db, retention).unwrap_or(0)
                        })
                        .await
                        .unwrap_or(0);
                        evictions_clone.fetch_add(reclaimed, Ordering::Relaxed);
                        let current = entries_clone.load(Ordering::Relaxed);
                        let sub = std::cmp::min(current, reclaimed);
                        entries_clone.fetch_sub(sub, Ordering::Relaxed);
                    }
                    _ = token_clone.cancelled() => break,
                }
            }
        });

        Ok(Self {
            name,
            db,
            stale_retention,
            max_entries,
            hits,
            misses,
            evictions,
            entries,
            shutdown_token,
            sweep_handle: Mutex::new(Some(handle)),
        })
    }

    /// Run a single reclamation pass and return the number of entries
    /// reclaimed. Tests call this directly for deterministic sweep coverage
    /// without waiting on the background ticker.
    #[cfg(test)]
    pub(crate) async fn sweep_once(&self) -> Result<u64, CamelError> {
        let db = Arc::clone(&self.db);
        let retention = self.stale_retention;
        let reclaimed = tokio::task::spawn_blocking(move || sweep_reclaim(&db, retention))
            .await
            .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))??;
        self.evictions.fetch_add(reclaimed, Ordering::Relaxed);
        let current = self.entries.load(Ordering::Relaxed);
        let sub = std::cmp::min(current, reclaimed);
        self.entries.fetch_sub(sub, Ordering::Relaxed);
        Ok(reclaimed)
    }
}

/// Reclaim entries whose `expires_at + stale_retention < now`.
///
/// Entries with `expires_at = None` (no expiry) are never reclaimed. Returns
/// the reclaimed count. Used by both the background sweep loop (errors mapped
/// to `0`) and [`RedbCacheRepository::sweep_once`] (errors propagated).
fn sweep_reclaim(db: &redb::Database, stale_retention: Duration) -> Result<u64, CamelError> {
    let txn = db
        .begin_write()
        .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
    let reclaimed = {
        let mut table = txn
            .open_table(CACHE_TABLE)
            .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
        let now = SystemTime::now();
        // Collect keys first — `table.iter()` borrows the table immutably and
        // would conflict with `remove` (mirrors `idempotent/redb_repository`
        // clear pattern).
        let mut to_delete: Vec<String> = Vec::new();
        for row in table
            .iter()
            .map_err(|e| CamelError::Io(format!("redb iter: {e}")))?
        {
            let (k, v) = row.map_err(|e| CamelError::Io(format!("redb iter item: {e}")))?;
            let entry: CacheEntry = serde_json::from_slice(v.value())
                .map_err(|e| CamelError::Io(format!("cache deserialization: {e}")))?;
            let should_delete = match entry.expires_at {
                Some(exp) => match exp.checked_add(stale_retention) {
                    // Threshold in the past → past stale-retention window.
                    Some(threshold) => threshold < now,
                    // SystemTime addition overflowed → treat as reclaimable.
                    None => true,
                },
                None => false,
            };
            if should_delete {
                to_delete.push(k.value().to_string());
            }
        }
        for k in &to_delete {
            let _ = table
                .remove(k.as_str())
                .map_err(|e| CamelError::Io(format!("redb remove: {e}")))?;
        }
        to_delete.len() as u64
    };
    txn.commit()
        .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
    Ok(reclaimed)
}

// ── CacheRepository impl ──────────────────────────────────────────────────────

#[async_trait]
impl CacheRepository for RedbCacheRepository {
    fn name(&self) -> &str {
        &self.name
    }

    async fn get(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        let db = Arc::clone(&self.db);
        let key = key.to_string();
        let result =
            tokio::task::spawn_blocking(move || -> Result<Option<CacheEntry>, CamelError> {
                let rtx = db
                    .begin_read()
                    .map_err(|e| CamelError::Io(format!("redb begin_read: {e}")))?;
                let table = rtx
                    .open_table(CACHE_TABLE)
                    .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
                match table
                    .get(key.as_str())
                    .map_err(|e| CamelError::Io(format!("redb get: {e}")))?
                {
                    Some(guard) => {
                        let entry: CacheEntry = serde_json::from_slice(guard.value())
                            .map_err(|e| CamelError::Io(format!("cache deserialization: {e}")))?;
                        Ok(Some(entry))
                    }
                    None => Ok(None),
                }
            })
            .await
            .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))??;
        // Expiry check happens outside the blocking closure, mirroring
        // `MemoryCacheRepository::get` semantics.
        match result {
            Some(entry) => {
                let expired = entry
                    .expires_at
                    .map(|e| e <= SystemTime::now())
                    .unwrap_or(false);
                if expired {
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    Ok(None)
                } else {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    Ok(Some(entry))
                }
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                Ok(None)
            }
        }
    }

    async fn set(
        &self,
        key: &str,
        mut value: CacheEntry,
        ttl: Option<Duration>,
    ) -> Result<(), CamelError> {
        value.expires_at = ttl.map(|d| SystemTime::now() + d);
        let serialized = serde_json::to_vec(&value)
            .map_err(|e| CamelError::Io(format!("cache serialization: {e}")))?;
        let db = Arc::clone(&self.db);
        let key = key.to_string();
        let max_entries = self.max_entries;
        let was_new = tokio::task::spawn_blocking(move || {
            let txn = db
                .begin_write()
                .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
            let was_new = {
                let mut table = txn
                    .open_table(CACHE_TABLE)
                    .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
                // Scope `prior` so its immutable borrow of `table` ends before
                // the mutable `insert` below (redb guards borrow the table).
                let is_new = {
                    let prior = table
                        .get(key.as_str())
                        .map_err(|e| CamelError::Io(format!("redb get: {e}")))?;
                    // Capacity check only applies to genuinely new keys —
                    // overwrites don't grow the table.
                    if prior.is_none()
                        && let Some(max) = max_entries
                    {
                        let count = table
                            .len()
                            .map_err(|e| CamelError::Io(format!("redb len: {e}")))?
                            as usize;
                        if count >= max {
                            return Err(CamelError::Config(format!(
                                "cache: max_entries ({max}) exceeded"
                            )));
                        }
                    }
                    prior.is_none()
                };
                table
                    .insert(key.as_str(), serialized.as_slice())
                    .map_err(|e| CamelError::Io(format!("redb insert: {e}")))?;
                is_new
            };
            txn.commit()
                .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
            Ok::<bool, CamelError>(was_new)
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))??;
        if was_new {
            self.entries.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    async fn peek_stale(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        let db = Arc::clone(&self.db);
        let key = key.to_string();
        tokio::task::spawn_blocking(move || {
            let rtx = db
                .begin_read()
                .map_err(|e| CamelError::Io(format!("redb begin_read: {e}")))?;
            let table = rtx
                .open_table(CACHE_TABLE)
                .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
            match table
                .get(key.as_str())
                .map_err(|e| CamelError::Io(format!("redb get: {e}")))?
            {
                Some(guard) => {
                    let entry: CacheEntry = serde_json::from_slice(guard.value())
                        .map_err(|e| CamelError::Io(format!("cache deserialization: {e}")))?;
                    Ok(Some(entry))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))?
    }

    async fn invalidate(&self, key: &str) -> Result<(), CamelError> {
        let db = Arc::clone(&self.db);
        let key = key.to_string();
        let was_present = tokio::task::spawn_blocking(move || {
            let txn = db
                .begin_write()
                .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
            let was_present = {
                let mut table = txn
                    .open_table(CACHE_TABLE)
                    .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
                // `remove` is idempotent per the trait contract; the returned
                // Option tells us whether a value was actually present. Read
                // `.is_some()` now so the guard (borrowing `table`) drops
                // before `commit` moves `txn`.
                table
                    .remove(key.as_str())
                    .map_err(|e| CamelError::Io(format!("redb remove: {e}")))?
                    .is_some()
            };
            txn.commit()
                .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
            Ok::<bool, CamelError>(was_present)
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))??;
        if was_present {
            let current = self.entries.load(Ordering::Relaxed);
            let sub = std::cmp::min(current, 1);
            self.entries.fetch_sub(sub, Ordering::Relaxed);
        }
        Ok(())
    }

    async fn clear(&self) -> Result<(), CamelError> {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || {
            let txn = db
                .begin_write()
                .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
            {
                let mut table = txn
                    .open_table(CACHE_TABLE)
                    .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
                // Collect-then-remove: `iter()` borrows immutably and cannot
                // coexist with `remove`. Mirrors `idempotent/redb_repository`
                // clear pattern.
                let keys: Vec<String> = table
                    .iter()
                    .map_err(|e| CamelError::Io(format!("redb iter: {e}")))?
                    .map(|r| {
                        r.map(|(k, _v)| k.value().to_string())
                            .map_err(|e| CamelError::Io(format!("redb iter item: {e}")))
                    })
                    .collect::<Result<_, _>>()?;
                for k in &keys {
                    let _ = table
                        .remove(k.as_str())
                        .map_err(|e| CamelError::Io(format!("redb remove: {e}")))?;
                }
            }
            txn.commit()
                .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
            Ok::<_, CamelError>(())
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))??;
        self.entries.store(0, Ordering::Relaxed);
        Ok(())
    }

    fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            entries: self.entries.load(Ordering::Relaxed),
        }
    }
}

impl fmt::Debug for RedbCacheRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedbCacheRepository")
            .field("name", &self.name)
            .field("stale_retention", &self.stale_retention)
            .field("max_entries", &self.max_entries)
            .field("shutdown_cancelled", &self.shutdown_token.is_cancelled())
            .field("stats", &self.stats())
            .finish()
    }
}

impl Drop for RedbCacheRepository {
    fn drop(&mut self) {
        // Abort ONLY the sweep task. Never cancel the context-owned token —
        // that would shut down the entire context when one repo drops.
        if let Some(handle) = self.sweep_handle.lock().take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tempfile::tempdir;

    fn entry() -> CacheEntry {
        CacheEntry {
            bytes: vec![1, 2, 3],
            content_type: camel_api::cache::ContentType::Bytes,
            expires_at: None,
        }
    }

    /// Open a repo at `<tmp>/cache.redb` with a 60s stale-retention, no cap,
    /// and a 1h sweep interval (so the background loop stays dormant during
    /// sequential tests).
    async fn new_repo(tmp: &TempDir, shutdown_token: CancellationToken) -> RedbCacheRepository {
        let path = tmp.path().join("cache.redb");
        RedbCacheRepository::new(
            "redb",
            path,
            Duration::from_secs(60),
            None,
            Duration::from_secs(3600),
            shutdown_token,
        )
        .await
        .expect("open redb cache repo")
    }

    #[tokio::test]
    async fn entries_survive_handle_drop_and_reopen() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("cache.redb");
        let token = CancellationToken::new();
        // Keep a clone so the sweep can be stopped via the real shutdown path
        // (token cancellation) before reopening the same file. redb rejects
        // reopening a file whose in-process handle is still alive, and the
        // sweep task holds a cloned Arc<Database> until it exits.
        let token_for_shutdown = token.clone();
        {
            let repo = RedbCacheRepository::new(
                "redb",
                path.clone(),
                Duration::from_secs(60),
                None,
                Duration::from_secs(3600),
                token,
            )
            .await
            .expect("open repo");
            repo.set("k", entry(), Some(Duration::from_secs(3600)))
                .await
                .expect("set");
            assert_eq!(
                repo.stats().entries,
                1,
                "entries counter must be 1 after first insert"
            );
            // Stop the sweep via its bound token and await the task so its
            // cloned Arc<Database> releases before the repo drops. Bind the
            // handle separately so the MutexGuard drops before the `.await`
            // (clippy::await-holding-lock).
            token_for_shutdown.cancel();
            let sweep_handle = repo.sweep_handle.lock().take();
            if let Some(handle) = sweep_handle {
                handle
                    .await
                    .expect("sweep task must exit cleanly on token cancel");
            }
            // repo dropped here — handle already taken; self.db Arc → 0 → DB
            // closes → redb frees its in-process lock.
        }
        // Reopen with a fresh token; entries must be reloaded from table.len().
        let token2 = CancellationToken::new();
        let repo = RedbCacheRepository::new(
            "redb",
            path,
            Duration::from_secs(60),
            None,
            Duration::from_secs(3600),
            token2,
        )
        .await
        .expect("reopen repo");
        let found = repo.get("k").await.expect("get after reopen");
        assert!(
            found.is_some(),
            "persisted entry must survive drop + reopen"
        );
        assert_eq!(
            repo.stats().entries,
            1,
            "entries counter must be restored from table.len() on reopen"
        );
    }

    #[tokio::test]
    async fn peek_stale_returns_post_expiry_entry_on_redb() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let repo = new_repo(&dir, token).await;
        repo.set("k", entry(), Some(Duration::from_millis(1)))
            .await
            .expect("set");
        tokio::time::sleep(Duration::from_millis(10)).await;
        let stale = repo.peek_stale("k").await.expect("peek_stale");
        assert!(
            stale.is_some(),
            "peek_stale must return the expired-but-present entry"
        );
    }

    #[tokio::test]
    async fn sweep_once_removes_entries_past_stale_retention() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let path = dir.path().join("cache.redb");
        // stale_retention=10ms; entry expires at +1ms; sleep 50ms so
        // (expires_at + 10ms) is well in the past → reclaimable.
        let repo = RedbCacheRepository::new(
            "redb",
            path,
            Duration::from_millis(10),
            None,
            Duration::from_secs(3600),
            token,
        )
        .await
        .expect("open repo");
        // Neutralize the background sweep so its immediate first tick (which
        // fires whenever the runtime next polls it) cannot steal the reclaim
        // from sweep_once. This makes the test deterministic: sweep_once is
        // the sole reclaimer.
        if let Some(handle) = repo.sweep_handle.lock().take() {
            handle.abort();
        }
        repo.set("k", entry(), Some(Duration::from_millis(1)))
            .await
            .expect("set");
        tokio::time::sleep(Duration::from_millis(50)).await;
        let reclaimed = repo.sweep_once().await.expect("sweep_once");
        assert!(
            reclaimed >= 1,
            "sweep_once must reclaim at least 1 entry, got {reclaimed}"
        );
        let stale = repo.peek_stale("k").await.expect("peek_stale after sweep");
        assert!(
            stale.is_none(),
            "entry must be gone after sweep_once reclaimed it"
        );
    }

    #[tokio::test]
    async fn sweep_stops_on_context_shutdown() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let path = dir.path().join("cache.redb");
        // Short sweep interval so the loop is definitely armed.
        let repo = RedbCacheRepository::new(
            "redb",
            path,
            Duration::from_secs(60),
            None,
            Duration::from_millis(10),
            token.clone(),
        )
        .await
        .expect("open repo");
        token.cancel();
        // Take the handle out so we can await it directly; Drop will see None.
        let handle = repo
            .sweep_handle
            .lock()
            .take()
            .expect("sweep handle must be present after construct");
        let completed = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(
            completed.is_ok(),
            "sweep task must complete within 5s of context shutdown"
        );
    }

    #[tokio::test]
    async fn redb_errors_surface_as_err() {
        let dir = tempdir().expect("tempdir");
        // A regular file where a directory is required — `create_dir_all`
        // must fail, surfacing as Err(CamelError::Io(_)).
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"not a dir").expect("write blocker");
        let path = blocker.join("cache.redb");
        let result = RedbCacheRepository::new(
            "redb",
            path,
            Duration::from_secs(60),
            None,
            Duration::from_secs(3600),
            CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(result, Err(CamelError::Io(_))),
            "expected Err(CamelError::Io(_)), got {result:?}"
        );
    }

    #[tokio::test]
    async fn overwrite_does_not_inflate_entries() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let repo = new_repo(&dir, token).await;
        repo.set("k", entry(), None).await.expect("first set");
        repo.set("k", entry(), None).await.expect("second set");
        assert_eq!(
            repo.stats().entries,
            1,
            "overwriting an existing key must not inflate the entries counter"
        );
    }

    #[tokio::test]
    async fn max_entries_rejects_new_key_allows_overwrite() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let path = dir.path().join("cache.redb");
        let repo = RedbCacheRepository::new(
            "redb",
            path,
            Duration::from_secs(60),
            Some(2),
            Duration::from_secs(3600),
            token,
        )
        .await
        .expect("open repo");
        repo.set("a", entry(), None).await.expect("set a");
        repo.set("b", entry(), None).await.expect("set b");
        let over = repo.set("c", entry(), None).await;
        assert!(
            over.is_err(),
            "third distinct key must be rejected at max_entries, got {over:?}"
        );
        let overw = repo.set("a", entry(), None).await;
        assert!(
            overw.is_ok(),
            "overwrite of an existing key must succeed at max_entries, got {overw:?}"
        );
    }
}
