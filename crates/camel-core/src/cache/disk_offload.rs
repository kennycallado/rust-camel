//! Disk-payload offload decorator for [`CacheRepository`] backends.
//!
//! [`DiskOffloadRepository`] wraps any backend (the "index") and moves entry
//! payloads to content-addressed blob files under a dedicated directory,
//! storing only a relative file name in the index row. Index rows stay
//! small; the payload is re-injected on `get`/`peek_stale`.
//!
//! # Blob lifecycle
//!
//! Blob names are `{blake3-128hex(key)}.{death_epoch_secs}.{blake3-128hex(
//! bytes || content_type-discriminant)}.blob`. The death epoch —
//! `expires_at + stale_retention + sweep_interval` — is encoded in the name
//! so the background sweeper can reclaim dead blobs by file name alone,
//! without consulting the index.
//!
//! # Failure policy
//!
//! - A blob write that fails falls back to storing the entry inline in the
//!   index (WARN + `inner.set` with the original entry): the decorator never
//!   converts its own file-write failure into a cache-write `Err`.
//! - A vanished or corrupt blob row degrades to a miss (`Ok(None)` + WARN).
//! - A blob that exists but cannot be read (e.g. `PermissionDenied`)
//!   surfaces as `Err` per ADR-0023 Contract C1.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::cache::CacheEntry;
use camel_api::cache::CacheRepository;
use camel_api::cache::CacheStats;
use camel_api::cache::ContentType;
use parking_lot::Mutex;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// Injectable wall clock for death-epoch math and deterministic tests.
///
/// Mirrors `ClockFn` in `camel-redis-repo::cache_repo`.
pub type OffloadClock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// The default production clock: [`SystemTime::now`].
pub fn default_offload_clock() -> OffloadClock {
    Arc::new(SystemTime::now)
}

/// Max attempts to open a unique tmp file before giving up on a name.
const TMP_NAME_ATTEMPTS: u32 = 8;

/// [`CacheRepository`] decorator that offloads entry payloads to disk.
///
/// Wraps any index backend; see the [module docs](self) for the blob
/// lifecycle and failure policy. `stale_retention`, `sweep_interval`, and
/// `payload_max_ttl` must be non-zero (the payload intervals at least one
/// second — the death epoch truncates to whole seconds) — enforced by
/// `CacheRepoConfig` validation, not here.
pub struct DiskOffloadRepository {
    /// Decorated index backend (memory, redb, redis, …).
    inner: Arc<dyn CacheRepository>,
    /// Directory holding offloaded payload blobs.
    dir: PathBuf,
    /// How long an expired entry stays peekable before reclamation.
    stale_retention: Duration,
    /// Background sweep cadence; its length is the death-epoch grace.
    sweep_interval: Duration,
    /// Fabricated TTL for entries stored without an explicit one.
    payload_max_ttl: Duration,
    /// Wall clock for death-epoch math.
    clock: OffloadClock,
    /// Background payload sweeper; aborted on Drop.
    sweep_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl DiskOffloadRepository {
    /// Wrap `inner` with disk payload offload into `dir` (production clock).
    ///
    /// The `shutdown_token` stops the background payload sweeper; it is
    /// owned by the sweeper task and never cancelled by the decorator.
    pub fn new(
        inner: Arc<dyn CacheRepository>,
        dir: PathBuf,
        stale_retention: Duration,
        sweep_interval: Duration,
        payload_max_ttl: Duration,
        shutdown_token: CancellationToken,
    ) -> Self {
        Self::with_clock(
            inner,
            dir,
            stale_retention,
            sweep_interval,
            payload_max_ttl,
            shutdown_token,
            default_offload_clock(),
        )
    }

    /// Test seam: [`Self::new`] with an injected [`OffloadClock`].
    ///
    /// The injected clock drives death-epoch math only; the spawned
    /// sweeper always sweeps on the real clock.
    pub fn with_clock(
        inner: Arc<dyn CacheRepository>,
        dir: PathBuf,
        stale_retention: Duration,
        sweep_interval: Duration,
        payload_max_ttl: Duration,
        shutdown_token: CancellationToken,
        clock: OffloadClock,
    ) -> Self {
        // The sweeper must observe real file ages, so it never uses the
        // injected decorator clock. The token moves into the task: the
        // decorator only aborts the task on Drop, never cancels the
        // context-owned token.
        let sweep_handle = spawn_sweeper(dir.clone(), sweep_interval, shutdown_token);
        Self {
            inner,
            dir,
            stale_retention,
            sweep_interval,
            payload_max_ttl,
            clock,
            sweep_handle: Mutex::new(Some(sweep_handle)),
        }
    }

    /// Write `entry`'s payload to its content-addressed blob file and
    /// return the final file name.
    ///
    /// Tmp-then-rename so a partially written blob is never visible under
    /// its final name. All I/O is async `tokio::fs` (the house file-I/O
    /// style, matching `camel-file`'s `atomic_write`).
    async fn write_blob(
        &self,
        key: &str,
        entry: &CacheEntry,
        death_epoch: u64,
    ) -> std::io::Result<String> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let dest_name = blob_filename(key, death_epoch, entry);
        let dest_path = self.dir.join(&dest_name);
        let (mut file, tmp_path) = self.open_tmp_exclusive(key, &dest_name).await?;

        // Best-effort tmp cleanup on failure: a leaked `.tmp` would never
        // be reclaimed by the epoch sweeper.
        if let Err(e) = file.write_all(&entry.bytes).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(e);
        }
        if let Err(e) = file.sync_all().await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(e);
        }
        if let Err(e) = tokio::fs::rename(&tmp_path, &dest_path).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(e);
        }
        self.fsync_dir_best_effort().await;
        Ok(dest_name)
    }

    /// Open a unique exclusive tmp file next to `dest_name`, retrying name
    /// collisions with a fresh nonce (bounded by [`TMP_NAME_ATTEMPTS`]).
    ///
    /// The nonce hashes `key || clock_nanos || attempt_counter`, so retries
    /// still produce fresh names under a frozen test clock.
    async fn open_tmp_exclusive(
        &self,
        key: &str,
        dest_name: &str,
    ) -> std::io::Result<(tokio::fs::File, PathBuf)> {
        let clock_nanos = (self.clock)()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut last_collision: Option<std::io::Error> = None;
        for attempt in 0..TMP_NAME_ATTEMPTS {
            let mut hasher = blake3::Hasher::new();
            hasher.update(key.as_bytes());
            hasher.update(&clock_nanos.to_le_bytes());
            hasher.update(&attempt.to_le_bytes());
            let nonce = hasher_128hex(hasher);
            let tmp_path = self.dir.join(format!("{dest_name}.{nonce}.tmp"));
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)
                .await
            {
                Ok(file) => return Ok((file, tmp_path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_collision = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_collision
            .unwrap_or_else(|| std::io::Error::other("tmp blob name collisions exhausted")))
    }

    /// Best-effort fsync of the blob directory so the rename itself is
    /// durable. Failures are WARNed and ignored: the blob is already
    /// renamed, and a directory-fsync failure must not fail the write.
    async fn fsync_dir_best_effort(&self) {
        let result = match tokio::fs::File::open(&self.dir).await {
            Ok(dir_file) => dir_file.sync_all().await,
            Err(e) => Err(e),
        };
        if let Err(e) = result {
            warn!(
                dir = %self.dir.display(),
                error = %e,
                "cache blob directory fsync failed (best-effort, ignored)"
            );
        }
    }

    /// Re-inject the offloaded payload into an index row (shared by `get`
    /// and `peek_stale`).
    ///
    /// Rows without `payload_path` pass through untouched (legacy/inline).
    /// A corrupt path or a vanished blob degrades to a miss; a blob that
    /// exists but cannot be read surfaces as `Err` (Contract C1).
    async fn hydrate(
        &self,
        key: &str,
        mut entry: CacheEntry,
    ) -> Result<Option<CacheEntry>, CamelError> {
        let Some(raw_path) = entry.payload_path.clone() else {
            return Ok(Some(entry));
        };
        let Some(name) = sanitize_blob_name(&raw_path) else {
            warn!(
                key = key,
                backend = self.inner.name(),
                payload_path = %raw_path,
                "corrupt cache row: payload_path must be a bare file name; treating as miss"
            );
            return Ok(None);
        };
        let blob_path = self.dir.join(name);
        match tokio::fs::read(&blob_path).await {
            Ok(bytes) => {
                entry.bytes = bytes;
                entry.payload_path = None;
                Ok(Some(entry))
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                warn!(
                    key = key,
                    backend = self.inner.name(),
                    blob = %blob_path.display(),
                    "cache payload blob gone; treating as miss"
                );
                Ok(None)
            }
            Err(e) => Err(CamelError::Io(format!(
                "cache payload blob read '{}': {e}",
                blob_path.display()
            ))),
        }
    }

    /// Best-effort unlink of every entry of the payload dir.
    ///
    /// Per-file `NotFound` is success (a concurrent sweeper or replica may
    /// have reclaimed the blob already); any other per-file error WARNs
    /// and iteration continues. [`Self::clear`] must never surface its
    /// own unlink failures as `Err`.
    async fn unlink_payload_dir_best_effort(&self) {
        let mut read_dir = match tokio::fs::read_dir(&self.dir).await {
            Ok(read_dir) => read_dir,
            // No dir = nothing was ever offloaded; nothing to unlink.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                warn!(
                    dir = %self.dir.display(),
                    error = %e,
                    "cache payload dir read failed during clear (best-effort, skipped)"
                );
                return;
            }
        };
        loop {
            let entry = match read_dir.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => return,
                Err(e) => {
                    warn!(
                        dir = %self.dir.display(),
                        error = %e,
                        "cache payload dir iteration failed during clear (best-effort, stopped)"
                    );
                    return;
                }
            };
            let path = entry.path();
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                // NotFound = a concurrent sweeper or replica won the race.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    warn!(
                        dir = %self.dir.display(),
                        blob = %path.display(),
                        error = %e,
                        "cache payload blob unlink failed during clear (best-effort, skipped)"
                    );
                }
            }
        }
    }
}

#[async_trait]
impl CacheRepository for DiskOffloadRepository {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn get(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        match self.inner.get(key).await? {
            Some(entry) => self.hydrate(key, entry).await,
            None => Ok(None),
        }
    }

    async fn set(
        &self,
        key: &str,
        mut entry: CacheEntry,
        ttl: Option<Duration>,
    ) -> Result<(), CamelError> {
        let effective_ttl = ttl.unwrap_or(self.payload_max_ttl);
        // Death epoch = expiry + retention + sweep grace, saturating in
        // Duration space (a pre-epoch clock clamps to the Unix epoch),
        // truncated to whole seconds for the blob filename.
        let death_epoch = (self.clock)()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .saturating_add(effective_ttl)
            .saturating_add(self.stale_retention)
            .saturating_add(self.sweep_interval)
            .as_secs();

        match self.write_blob(key, &entry, death_epoch).await {
            Ok(dest_name) => {
                entry.bytes = Vec::new();
                entry.payload_path = Some(dest_name);
                // The ttl MUST be Some: every inner overwrites
                // `expires_at` from the ttl argument, so None would wipe
                // the fabricated expiry. The inner recomputes `expires_at`
                // from its own clock; the sub-second skew is absorbed by
                // the death-epoch grace.
                self.inner.set(key, entry, Some(effective_ttl)).await
            }
            Err(e) => {
                warn!(
                    key = key,
                    backend = self.inner.name(),
                    dir = %self.dir.display(),
                    error = %e,
                    "cache blob write failed; storing entry inline instead"
                );
                // Inline fallback with the original, unstripped entry: the
                // decorator never converts its own file-write failure into
                // a cache-write error. The CAPPED ttl keeps the spec's
                // no-TTL semantic (payload_max_ttl) even for degraded rows
                // — an uncapped inline row would never be reclaimed.
                self.inner.set(key, entry, Some(effective_ttl)).await
            }
        }
    }

    async fn peek_stale(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        match self.inner.peek_stale(key).await? {
            Some(entry) => self.hydrate(key, entry).await,
            None => Ok(None),
        }
    }

    /// Delegate-only: the index row is dropped here; the payload blob
    /// becomes an orphan reclaimed asynchronously at its
    /// filename-encoded death epoch.
    async fn invalidate(&self, key: &str) -> Result<(), CamelError> {
        self.inner.invalidate(key).await
    }

    /// Reclaim payload space now: best-effort unlink of every entry of
    /// the payload dir, then delegate to the index. Unlink failures
    /// never turn `clear` into `Err` — each failure WARNs and the rest
    /// of the dir is still attempted.
    async fn clear(&self) -> Result<(), CamelError> {
        self.unlink_payload_dir_best_effort().await;
        self.inner.clear().await
    }

    /// Delegate-only: the returned count is index-scoped; payload blobs
    /// are reclaimed asynchronously at their filename-encoded death epoch.
    async fn invalidate_prefix(&self, prefix: &str) -> Result<u64, CamelError> {
        self.inner.invalidate_prefix(prefix).await
    }

    async fn stats(&self) -> CacheStats {
        self.inner.stats().await
    }
}

impl std::fmt::Debug for DiskOffloadRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskOffloadRepository")
            .field("inner", &self.inner)
            .field("dir", &self.dir)
            .field("stale_retention", &self.stale_retention)
            .field("sweep_interval", &self.sweep_interval)
            .field("payload_max_ttl", &self.payload_max_ttl)
            .field("sweep_attached", &self.sweep_handle.lock().is_some())
            .finish()
    }
}

impl Drop for DiskOffloadRepository {
    fn drop(&mut self) {
        // Abort ONLY the sweep task. Never cancel the context-owned token —
        // that would shut down the entire context when one repo drops.
        if let Some(handle) = self.sweep_handle.lock().take() {
            handle.abort();
        }
    }
}

// ── Filename helpers ─────────────────────────────────────────────────────────

/// One-byte discriminant of the closed [`ContentType`] enum, mixed into the
/// content fingerprint for domain separation (identical bytes under
/// different content types produce different fingerprints). Exhaustive
/// match — the enum is closed by contract (ADR-0049 §Exceptions).
fn content_type_discriminant(content_type: ContentType) -> u8 {
    match content_type {
        ContentType::Bytes => 0,
        ContentType::Text => 1,
        ContentType::Json => 2,
        ContentType::Xml => 3,
    }
}

/// Finalize a hasher to its first 128 bits as 32 lowercase hex chars.
fn hasher_128hex(hasher: blake3::Hasher) -> String {
    let hex = hasher.finalize().to_hex().to_string();
    hex[..32].to_string()
}

/// blake3-128 hex of a single byte slice.
fn blake3_128hex(data: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(data);
    hasher_128hex(hasher)
}

/// 128-bit content fingerprint: `blake3(bytes || content_type discriminant)`.
fn content_fingerprint(entry: &CacheEntry) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&entry.bytes);
    hasher.update(&[content_type_discriminant(entry.content_type)]);
    hasher_128hex(hasher)
}

/// Blob file name: `{key-hash}.{death_epoch}.{fingerprint}.blob`.
fn blob_filename(key: &str, death_epoch: u64, entry: &CacheEntry) -> String {
    format!(
        "{}.{}.{}.blob",
        blake3_128hex(key.as_bytes()),
        death_epoch,
        content_fingerprint(entry)
    )
}

/// Death epoch (second dot-separated component) of a blob file name, if it
/// parses as `u64`.
fn parse_death_epoch(file_name: &str) -> Option<u64> {
    file_name.split('.').nth(1)?.parse().ok()
}

/// Accept only a bare file name: non-empty, no `/`, no `\`, no `..`.
///
/// Absolute paths necessarily contain a separator on both Unix and Windows,
/// so the separator checks subsume the absolute-path rejection. Everything
/// else is treated as a corrupt row.
fn sanitize_blob_name(path: &str) -> Option<&str> {
    if path.is_empty() || path.contains('/') || path.contains('\\') || path.contains("..") {
        return None;
    }
    Some(path)
}

// ── Payload sweeper ─────────────────────────────────────────────────────────

/// Unlink one payload-dir file if it is dead: `.blob` files by their
/// name-encoded death epoch, `.tmp` leftovers by age.
///
/// `Ok(true)` = unlinked here; `Ok(false)` = kept (still live, a foreign
/// name without a parseable epoch, or vanished between listing and unlink —
/// the ENOENT race counts as reclaimed-by-someone-else, never an error).
/// Any other error is returned for the sweep loop to WARN over. All filesystem
/// access is async (`tokio::fs`), keeping the sweeper off blocked
/// runtime workers.
async fn unlink_payload_file(
    path: &Path,
    now: SystemTime,
    sweep_interval: Duration,
) -> std::io::Result<bool> {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return Ok(false);
    };
    // Clamp a pre-epoch clock to the Unix epoch, matching `set`'s
    // death-epoch math.
    let now_secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let dead = if name.ends_with(".blob") {
        // Strictly-before: a blob dying exactly `now` survives this pass
        // (the filename epoch is whole seconds; the next tick reclaims).
        parse_death_epoch(name).is_some_and(|death| death < now_secs)
    } else if name.ends_with(".tmp") {
        let threshold = now.checked_sub(sweep_interval).unwrap_or(UNIX_EPOCH);
        let mtime = match tokio::fs::metadata(path).await {
            Ok(meta) => meta.modified()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        mtime < threshold
    } else {
        return Ok(false);
    };
    if !dead {
        return Ok(false);
    }
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// One sweep pass over `dir`: reclaim dead blobs by their filename
/// death epoch and stale `.tmp` leftovers by age.
///
/// Per-file `NotFound` (a concurrent sweeper or replica won the race)
/// counts as success; other per-file errors WARN and the scan
/// continues. A missing dir is not an error — nothing was ever
/// offloaded. Returns `(blobs_unlinked, tmps_unlinked)`.
async fn sweep_payload_dir(dir: &Path, now: SystemTime, sweep_interval: Duration) -> (u64, u64) {
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(read_dir) => read_dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (0, 0),
        Err(e) => {
            warn!(
                dir = %dir.display(),
                error = %e,
                "cache payload dir read failed during sweep (skipped)"
            );
            return (0, 0);
        }
    };
    let mut blobs = 0u64;
    let mut tmps = 0u64;
    loop {
        let entry = match read_dir.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(e) => {
                warn!(
                    dir = %dir.display(),
                    error = %e,
                    "cache payload dir iteration failed during sweep (stopped)"
                );
                break;
            }
        };
        let path = entry.path();
        match unlink_payload_file(&path, now, sweep_interval).await {
            Ok(true) => {
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".tmp"))
                {
                    tmps += 1;
                } else {
                    blobs += 1;
                }
            }
            Ok(false) => {}
            Err(e) => warn!(
                dir = %dir.display(),
                file = %path.display(),
                error = %e,
                "cache payload file unlink failed during sweep (skipped)"
            ),
        }
    }
    (blobs, tmps)
}

/// Spawn the background payload sweeper for `dir`.
///
/// Mirrors the redb sweep loop: tick every `sweep_interval`, reclaim
/// dead blobs and stale tmp files, exit when `shutdown_token` fires.
/// The sweep always runs on the REAL clock (`SystemTime::now`), never
/// an injected decorator clock — it must observe actual file ages.
fn spawn_sweeper(
    dir: PathBuf,
    sweep_interval: Duration,
    shutdown_token: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(sweep_interval);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    sweep_payload_dir(&dir, SystemTime::now(), sweep_interval).await;
                }
                _ = shutdown_token.cancelled() => break,
            }
        }
    })
}

#[cfg(test)]
#[path = "disk_offload_tests.rs"]
mod disk_offload_tests;
