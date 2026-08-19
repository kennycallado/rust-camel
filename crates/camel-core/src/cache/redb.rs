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
use std::ops::Bound;
use std::path::Path;
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
/// them via cloned references. `cache_size`, `sweep_interval`, and
/// `stale_retention` are retained so the propagation seam required by the
/// eip-cache spec can expose them via accessors.
pub struct RedbCacheRepository {
    name: String,
    db: Arc<redb::Database>,
    stale_retention: Duration,
    max_entries: Option<usize>,
    /// redb page-cache size in bytes, passed to `redb::Builder::set_cache_size`.
    cache_size: usize,
    /// Recorded background-sweep interval, consumed by the spawned sweep task.
    sweep_interval: Duration,
    hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
    evictions: Arc<AtomicU64>,
    peek_stale_served: Arc<AtomicU64>,
    invalidations: Arc<AtomicU64>,
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
        cache_size: usize,
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
            let db = redb::Builder::new()
                .set_cache_size(cache_size)
                .create(&path_for_db)
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
        let peek_stale_served = Arc::new(AtomicU64::new(0));
        let invalidations = Arc::new(AtomicU64::new(0));
        let entries = Arc::new(AtomicU64::new(initial_len));

        // Container memory guardrail — diagnostic only, never fails
        // construction. Warns once when the redb page cache cannot fit within
        // the container's cgroup memory limit.
        emit_memory_guardrail(
            cache_size,
            Path::new("/sys/fs/cgroup/memory.max"),
            Path::new("/sys/fs/cgroup/memory/memory.limit_in_bytes"),
        );

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
            cache_size,
            sweep_interval,
            hits,
            misses,
            evictions,
            peek_stale_served,
            invalidations,
            entries,
            shutdown_token,
            sweep_handle: Mutex::new(Some(handle)),
        })
    }

    /// Recorded redb page-cache size in bytes (propagation seam for the
    /// eip-cache spec).
    pub fn cache_size(&self) -> usize {
        self.cache_size
    }

    /// Recorded background-sweep interval.
    pub fn sweep_interval(&self) -> std::time::Duration {
        self.sweep_interval
    }

    /// Recorded stale-retention window.
    pub fn stale_retention(&self) -> std::time::Duration {
        self.stale_retention
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

// ── Memory guardrail ──────────────────────────────────────────────────────────

/// Read the container memory limit (bytes) from the cgroup filesystem,
/// preferring cgroup v2 (`memory.max`) with cgroup v1
/// (`memory.limit_in_bytes`) as fallback.
///
/// v2 `"max"` (unlimited) or unparseable content falls through to v1; a v1
/// value above 16 TiB is the v1 "unlimited" sentinel and is reported as no
/// limit. Missing/unreadable files at either path fall through to `None`.
/// All reads are best-effort (`std::fs::read_to_string(...).ok()`) — this is a
/// diagnostic seam, never a failure path.
pub(crate) fn memory_limit_from_paths(v2: &Path, v1: &Path) -> Option<u64> {
    if let Ok(content) = std::fs::read_to_string(v2)
        && let Ok(bytes) = content.trim().parse::<u64>()
    {
        return Some(bytes);
    }
    if let Ok(content) = std::fs::read_to_string(v1)
        && let Ok(bytes) = content.trim().parse::<u64>()
        // cgroup v1 "unlimited" sentinel: anything above 16 TiB.
        && bytes <= 17_592_186_044_416
    {
        return Some(bytes);
    }
    None
}

/// Diagnostic-only guardrail: when the configured redb cache size exceeds the
/// container's cgroup memory limit, emit a single warning naming both values.
/// Never fails — a missing limit or unreadable files simply skip the warning.
pub(crate) fn emit_memory_guardrail(cache_size: usize, v2: &Path, v1: &Path) {
    let cache_size = cache_size as u64;
    if let Some(limit) = memory_limit_from_paths(v2, v1)
        && cache_size > limit
    {
        tracing::warn!(
            "redb cache_size ({cache_size} bytes) exceeds container memory limit ({limit} bytes)"
        );
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

/// Compute the smallest string that sorts after every string beginning with
/// `prefix`, as the exclusive upper [`Bound`] of a range scan.
///
/// The successor is `prefix` with its last Unicode scalar value incremented by
/// one, skipping the UTF-16 surrogate gap (U+D7FF → U+E000). A trailing
/// U+10FFFF carries into the preceding scalar; an empty or all-U+10FFFF
/// prefix has no successor, so the bound is [`Bound::Unbounded`].
fn successor_bound(prefix: &str) -> Bound<String> {
    match prefix.chars().last() {
        // Empty string has no scalar to increment — match every key.
        None => Bound::Unbounded,
        Some(last) => {
            let rest = &prefix[..prefix.len() - last.len_utf8()];
            match increment_scalar(last) {
                Some(next) => {
                    let mut s = String::with_capacity(rest.len() + next.len_utf8());
                    s.push_str(rest);
                    s.push(next);
                    Bound::Excluded(s)
                }
                // U+10FFFF has no successor scalar — carry into the rest.
                None => successor_bound(rest),
            }
        }
    }
}

/// Increment a Unicode scalar value by one, skipping the surrogate range.
///
/// Returns `None` for U+10FFFF (the maximum scalar has no successor).
fn increment_scalar(c: char) -> Option<char> {
    match c {
        // U+D7FF jumps over the surrogate range to U+E000.
        '\u{D7FF}' => Some('\u{E000}'),
        // U+10FFFF is the maximum scalar value.
        '\u{10FFFF}' => None,
        // Surrogates are not valid `char`, so every remaining scalar +1 is valid.
        _ => char::from_u32(c as u32 + 1),
    }
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
        if result.is_some() {
            self.peek_stale_served.fetch_add(1, Ordering::Relaxed);
        }
        Ok(result)
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
        self.invalidations.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn invalidate_prefix(&self, prefix: &str) -> Result<u64, CamelError> {
        let db = Arc::clone(&self.db);
        let prefix = prefix.to_string();
        let deleted = tokio::task::spawn_blocking(move || -> Result<u64, CamelError> {
            // Collect matching keys in a read txn, then delete in one write
            // txn (mirrors the collect-then-remove pattern of `clear`).
            let keys: Vec<String> = {
                let rtx = db
                    .begin_read()
                    .map_err(|e| CamelError::Io(format!("redb begin_read: {e}")))?;
                let table = rtx
                    .open_table(CACHE_TABLE)
                    .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
                // `successor_bound` yields an owned bound; redb's range needs
                // `&str` bounds for `&str` keys (`String` is not `Borrow<&str>`).
                let upper: Bound<String> = successor_bound(&prefix);
                let upper_ref: Bound<&str> = match &upper {
                    Bound::Included(s) => Bound::Included(s.as_str()),
                    Bound::Excluded(s) => Bound::Excluded(s.as_str()),
                    Bound::Unbounded => Bound::Unbounded,
                };
                let mut keys = Vec::new();
                for row in table
                    .range::<&str>((Bound::Included(prefix.as_str()), upper_ref))
                    .map_err(|e| CamelError::Io(format!("redb range: {e}")))?
                {
                    let (k, _v) =
                        row.map_err(|e| CamelError::Io(format!("redb range item: {e}")))?;
                    keys.push(k.value().to_string());
                }
                keys
            };
            let wtx = db
                .begin_write()
                .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
            {
                let mut table = wtx
                    .open_table(CACHE_TABLE)
                    .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
                for k in &keys {
                    let _ = table
                        .remove(k.as_str())
                        .map_err(|e| CamelError::Io(format!("redb remove: {e}")))?;
                }
            }
            wtx.commit()
                .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
            Ok(keys.len() as u64)
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))??;
        self.invalidations.fetch_add(1, Ordering::Relaxed);
        if deleted > 0 {
            let current = self.entries.load(Ordering::Relaxed);
            let sub = std::cmp::min(current, deleted);
            self.entries.fetch_sub(sub, Ordering::Relaxed);
        }
        Ok(deleted)
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

    async fn stats(&self) -> CacheStats {
        let db = Arc::clone(&self.db);
        let bytes = tokio::task::spawn_blocking(move || total_bytes(&db))
            .await
            .unwrap_or_default();
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            entries: self.entries.load(Ordering::Relaxed),
            peek_stale_served: self.peek_stale_served.load(Ordering::Relaxed),
            invalidations: self.invalidations.load(Ordering::Relaxed),
            bytes,
        }
    }
}

/// Sum every entry's `bytes.len()` over the full table range.
///
/// Called only inside `spawn_blocking` from `stats()`; `None` when the table
/// cannot be read or any entry fails to deserialize — the `bytes` field is a
/// best-effort report, never an error.
fn total_bytes(db: &redb::Database) -> Option<u64> {
    let rtx = db.begin_read().ok()?;
    let table = rtx.open_table(CACHE_TABLE).ok()?;
    let mut total: u64 = 0;
    for row in table.iter().ok()? {
        let (_key, value) = row.ok()?;
        let entry: CacheEntry = serde_json::from_slice(value.value()).ok()?;
        total = total.saturating_add(entry.bytes.len() as u64);
    }
    Some(total)
}

impl fmt::Debug for RedbCacheRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedbCacheRepository")
            .field("name", &self.name)
            .field("stale_retention", &self.stale_retention)
            .field("max_entries", &self.max_entries)
            .field("cache_size", &self.cache_size)
            .field("sweep_interval", &self.sweep_interval)
            .field("shutdown_cancelled", &self.shutdown_token.is_cancelled())
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
    /// a 256 MiB cache size, and a 1h sweep interval (so the background loop
    /// stays dormant during sequential tests).
    async fn new_repo(tmp: &TempDir, shutdown_token: CancellationToken) -> RedbCacheRepository {
        new_repo_with(
            tmp,
            shutdown_token,
            Duration::from_secs(60),
            None,
            256 * 1024 * 1024,
            Duration::from_secs(3600),
        )
        .await
    }

    /// Full-parameter variant of [`new_repo`] for tests that need custom
    /// stale-retention, cap, cache size, or sweep interval values.
    async fn new_repo_with(
        tmp: &TempDir,
        shutdown_token: CancellationToken,
        stale_retention: Duration,
        max_entries: Option<usize>,
        cache_size: usize,
        sweep_interval: Duration,
    ) -> RedbCacheRepository {
        let path = tmp.path().join("cache.redb");
        RedbCacheRepository::new(
            "redb",
            path,
            stale_retention,
            max_entries,
            cache_size,
            sweep_interval,
            shutdown_token,
        )
        .await
        .expect("open redb cache repo")
    }

    #[tokio::test]
    async fn cache_size_recorded_and_accessible() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let repo = new_repo_with(
            &dir,
            token,
            Duration::from_secs(60),
            None,
            536_870_912,
            Duration::from_secs(3600),
        )
        .await;
        assert_eq!(repo.cache_size(), 536_870_912);
    }

    #[tokio::test]
    async fn sweep_interval_recorded_and_accessible() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let repo = new_repo_with(
            &dir,
            token,
            Duration::from_secs(60),
            None,
            256 * 1024 * 1024,
            Duration::from_secs(1800),
        )
        .await;
        assert_eq!(repo.sweep_interval(), Duration::from_secs(1800));
    }

    #[tokio::test]
    async fn stale_retention_recorded_and_accessible() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let repo = new_repo_with(
            &dir,
            token,
            Duration::from_secs(3600),
            None,
            256 * 1024 * 1024,
            Duration::from_secs(3600),
        )
        .await;
        assert_eq!(repo.stale_retention(), Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn explicit_cache_size_round_trip() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let repo = new_repo_with(
            &dir,
            token,
            Duration::from_secs(60),
            None,
            512 * 1024 * 1024,
            Duration::from_secs(3600),
        )
        .await;
        repo.set("k", entry(), Some(Duration::from_secs(3600)))
            .await
            .expect("set");
        let found = repo.get("k").await.expect("get");
        assert!(
            found.is_some(),
            "entry must round-trip through the builder-opened database"
        );
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
                256 * 1024 * 1024,
                Duration::from_secs(3600),
                token,
            )
            .await
            .expect("open repo");
            repo.set("k", entry(), Some(Duration::from_secs(3600)))
                .await
                .expect("set");
            assert_eq!(
                repo.stats().await.entries,
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
            256 * 1024 * 1024,
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
            repo.stats().await.entries,
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
            256 * 1024 * 1024,
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
            256 * 1024 * 1024,
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
            256 * 1024 * 1024,
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
            repo.stats().await.entries,
            1,
            "overwriting an existing key must not inflate the entries counter"
        );
    }

    #[tokio::test]
    async fn stats_reports_bytes_sum() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let repo = new_repo(&dir, token).await;
        let a = CacheEntry {
            bytes: vec![1, 2, 3],
            content_type: camel_api::cache::ContentType::Bytes,
            expires_at: None,
        };
        let b = CacheEntry {
            bytes: vec![1, 2, 3, 4, 5],
            content_type: camel_api::cache::ContentType::Bytes,
            expires_at: None,
        };
        repo.set("a", a, None).await.expect("set a");
        repo.set("b", b, None).await.expect("set b");
        assert_eq!(repo.stats().await.bytes, Some(8));
    }

    #[tokio::test]
    async fn stats_counters_reported_alongside_bytes() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let repo = new_repo(&dir, token).await;
        let a = CacheEntry {
            bytes: vec![1, 2, 3],
            content_type: camel_api::cache::ContentType::Bytes,
            expires_at: None,
        };
        repo.set("a", a, None).await.expect("set a");
        let s = repo.stats().await;
        assert_eq!(s.entries, 1);
        assert_eq!(s.bytes, Some(3));
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
            256 * 1024 * 1024,
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

    #[tokio::test]
    async fn invalidate_prefix_removes_namespace_only() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let repo = new_repo(&dir, token).await;
        repo.set("rainviewer:a", entry(), None)
            .await
            .expect("set rainviewer:a");
        repo.set("rainviewer:b", entry(), None)
            .await
            .expect("set rainviewer:b");
        repo.set("gibs:a", entry(), None).await.expect("set gibs:a");
        let deleted = repo
            .invalidate_prefix("rainviewer:")
            .await
            .expect("invalidate_prefix");
        assert_eq!(deleted, 2, "only the rainviewer namespace must be removed");
        assert!(
            repo.get("rainviewer:a").await.expect("get").is_none(),
            "rainviewer:a must be gone"
        );
        assert!(
            repo.get("rainviewer:b").await.expect("get").is_none(),
            "rainviewer:b must be gone"
        );
        assert!(
            repo.get("gibs:a").await.expect("get").is_some(),
            "gibs:a must survive"
        );
    }

    #[tokio::test]
    async fn invalidate_prefix_does_not_delete_successor_key() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let repo = new_repo(&dir, token).await;
        repo.set("ns:", entry(), None).await.expect("set ns:");
        repo.set("ns;", entry(), None).await.expect("set ns;");
        let deleted = repo
            .invalidate_prefix("ns:")
            .await
            .expect("invalidate_prefix");
        assert_eq!(deleted, 1, "only the ns: key must be removed");
        assert!(
            repo.get("ns:").await.expect("get").is_none(),
            "ns: must be gone"
        );
        assert!(
            repo.get("ns;").await.expect("get").is_some(),
            "successor key ns; must survive"
        );
    }

    // ── cgroup memory-limit guardrail ─────────────────────────────────────────

    /// Runs `f` under a thread-local default `fmt` subscriber that appends into
    /// a shared buffer, then returns the captured text.
    fn capture_guardrail(f: impl FnOnce()) -> String {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt::Subscriber::builder()
            .with_writer(TestWriter {
                buf: Arc::clone(&buf),
            })
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let captured = buf.lock().clone();
        String::from_utf8(captured).expect("captured output must be UTF-8")
    }

    /// `fmt` writer that appends into a shared `Arc<Mutex<Vec<u8>>>`.
    struct TestWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl std::io::Write for TestWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TestWriter {
        type Writer = TestWriter;

        fn make_writer(&'a self) -> Self::Writer {
            TestWriter {
                buf: Arc::clone(&self.buf),
            }
        }
    }

    #[test]
    fn cgroup_v2_limit_parsed() {
        let dir = tempdir().expect("tempdir");
        let v2 = dir.path().join("memory.max");
        std::fs::write(&v2, "805306368\n").expect("write v2");
        let missing_v1 = dir.path().join("missing-v1");
        assert_eq!(memory_limit_from_paths(&v2, &missing_v1), Some(805_306_368));
    }

    #[test]
    fn cgroup_v2_max_means_unlimited() {
        let dir = tempdir().expect("tempdir");
        let v2 = dir.path().join("memory.max");
        std::fs::write(&v2, "max").expect("write v2");
        let missing_v1 = dir.path().join("missing-v1");
        assert_eq!(memory_limit_from_paths(&v2, &missing_v1), None);
    }

    #[test]
    fn cgroup_v2_malformed_falls_through() {
        let dir = tempdir().expect("tempdir");
        let v2 = dir.path().join("memory.max");
        std::fs::write(&v2, "not-a-number").expect("write v2");
        let v1 = dir.path().join("memory.limit_in_bytes");
        std::fs::write(&v1, "1073741824").expect("write v1");
        assert_eq!(memory_limit_from_paths(&v2, &v1), Some(1_073_741_824));
    }

    #[test]
    fn cgroup_v1_sentinel_unlimited() {
        let dir = tempdir().expect("tempdir");
        let missing_v2 = dir.path().join("missing-v2");
        let v1 = dir.path().join("memory.limit_in_bytes");
        std::fs::write(&v1, "9223372036854771712").expect("write v1");
        assert_eq!(memory_limit_from_paths(&missing_v2, &v1), None);
    }

    #[test]
    fn cgroup_v1_exactly_16tib_is_a_limit() {
        let dir = tempdir().expect("tempdir");
        let missing_v2 = dir.path().join("missing-v2");
        let v1 = dir.path().join("memory.limit_in_bytes");
        std::fs::write(&v1, "17592186044416\n").expect("write v1");
        assert_eq!(
            memory_limit_from_paths(&missing_v2, &v1),
            Some(17_592_186_044_416)
        );
    }

    #[test]
    fn successor_bound_unit_tests() {
        assert_eq!(
            successor_bound("ns:"),
            std::ops::Bound::Excluded("ns;".to_string())
        );
        // Prefix ending U+D7FF must jump the surrogate gap to U+E000.
        assert_eq!(
            successor_bound("a\u{D7FF}"),
            std::ops::Bound::Excluded("a\u{E000}".to_string())
        );
        // Prefix ending U+E000 increments to U+E001.
        assert_eq!(
            successor_bound("a\u{E000}"),
            std::ops::Bound::Excluded("a\u{E001}".to_string())
        );
        // Trailing U+10FFFF carries into the preceding scalar: "a…" → "b".
        assert_eq!(
            successor_bound("a\u{10FFFF}"),
            std::ops::Bound::Excluded("b".to_string())
        );
        // A prefix of only U+10FFFF has no successor.
        assert_eq!(
            successor_bound("\u{10FFFF}\u{10FFFF}"),
            std::ops::Bound::Unbounded
        );
    }

    #[tokio::test]
    async fn invalidate_prefix_empty_prefix_removes_all_seeded() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let repo = new_repo(&dir, token).await;
        repo.set("ns:a", entry(), None).await.expect("set ns:a");
        repo.set("ns:b", entry(), None).await.expect("set ns:b");
        repo.set("other:c", entry(), None)
            .await
            .expect("set other:c");
        let deleted = repo.invalidate_prefix("").await.expect("invalidate_prefix");
        assert_eq!(deleted, 3, "empty prefix must remove every entry");
    }

    #[tokio::test]
    async fn invalidate_prefix_empty_namespace_returns_zero() {
        let dir = tempdir().expect("tempdir");
        let token = CancellationToken::new();
        let repo = new_repo(&dir, token).await;
        let deleted = repo
            .invalidate_prefix("ns:")
            .await
            .expect("invalidate_prefix");
        assert_eq!(deleted, 0, "absent namespace must report zero removals");
    }

    #[test]
    fn cgroup_files_missing() {
        let dir = tempdir().expect("tempdir");
        let missing_v2 = dir.path().join("missing-v2");
        let missing_v1 = dir.path().join("missing-v1");
        assert_eq!(memory_limit_from_paths(&missing_v2, &missing_v1), None);
    }

    #[test]
    fn guardrail_warns_when_exceeds() {
        let dir = tempdir().expect("tempdir");
        let v2 = dir.path().join("memory.max");
        std::fs::write(&v2, "805306368\n").expect("write v2");
        let missing_v1 = dir.path().join("missing-v1");
        let output = capture_guardrail(|| {
            emit_memory_guardrail(1_073_741_824, &v2, &missing_v1);
        });
        assert!(output.contains("1073741824"), "output: {output}");
        assert!(output.contains("805306368"), "output: {output}");
        assert_eq!(
            output.matches("exceeds container memory limit").count(),
            1,
            "warn line must appear exactly once: {output}"
        );
    }

    #[test]
    fn guardrail_silent_when_fits() {
        let dir = tempdir().expect("tempdir");
        let v2 = dir.path().join("memory.max");
        std::fs::write(&v2, "805306368\n").expect("write v2");
        let missing_v1 = dir.path().join("missing-v1");
        let output = capture_guardrail(|| {
            emit_memory_guardrail(268_435_456, &v2, &missing_v1);
        });
        assert!(output.is_empty(), "expected no output, got: {output}");
    }

    #[test]
    fn guardrail_silent_when_files_missing() {
        let dir = tempdir().expect("tempdir");
        let missing_v2 = dir.path().join("missing-v2");
        let missing_v1 = dir.path().join("missing-v1");
        let output = capture_guardrail(|| {
            emit_memory_guardrail(1_073_741_824, &missing_v2, &missing_v1);
        });
        assert!(output.is_empty(), "expected no output, got: {output}");
    }
}
