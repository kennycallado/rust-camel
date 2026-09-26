//! Disk payload store for the [`crate::cache::offload::OffloadRepository`]
//! decorator.
//!
//! [`DiskPayloadStore`] holds content-addressed payload blob files under a
//! dedicated directory, addressed by the blob names the decorator
//! computes. A background sweeper reclaims dead blobs by their
//! name-encoded death epoch and stale `.tmp` leftovers by age, exiting
//! when the context-owned shutdown token fires.
//!
//! # Blob lifecycle
//!
//! Blob names are `{blake3-128hex(key)}.{death_epoch_secs}.{blake3-128hex(
//! bytes || content_type-discriminant)}.blob`. The death epoch is encoded
//! in the name so the sweeper can reclaim dead blobs by file name alone,
//! without consulting the index.
//!
//! # Failure policy
//!
//! - Writes are tmp-then-rename so a partially written blob is never
//!   visible under its final name; write failures surface to the
//!   decorator, whose inline fallback applies.
//! - A vanished blob reads as `Ok(None)`; a blob that exists but cannot
//!   be read (e.g. `PermissionDenied`) surfaces as `Err` per ADR-0023
//!   Contract C1.
//! - `unlink` counts an already-absent blob as success; `clear` never
//!   fails — per-blob unlink failures WARN and the scan continues.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use camel_api::CamelError;
use parking_lot::Mutex;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::cache::offload::{PayloadStore, hasher_128hex, parse_death_epoch, sanitize_blob_name};

/// Max attempts to open a unique tmp file before giving up on a name.
const TMP_NAME_ATTEMPTS: u32 = 8;

/// [`PayloadStore`] that keeps offloaded payload blobs as files under
/// `dir`, with a background death-epoch sweeper.
///
/// The sweeper is spawned on construction and aborted on [`Drop`]; its
/// shutdown token is owned by the sweeper task and never cancelled by
/// the store. All sweeping runs on the real clock — it must observe
/// actual file ages (the decorator's injectable test clock never
/// reaches the store).
pub struct DiskPayloadStore {
    /// Directory holding offloaded payload blobs.
    dir: PathBuf,
    /// Background payload sweeper; aborted on Drop.
    sweep_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl DiskPayloadStore {
    /// Create a store writing blobs into `dir` (production clock).
    ///
    /// The `shutdown_token` stops the background payload sweeper; it is
    /// owned by the sweeper task and never cancelled by the store.
    pub fn new(dir: PathBuf, sweep_interval: Duration, shutdown_token: CancellationToken) -> Self {
        let sweep_handle = spawn_sweeper(dir.clone(), sweep_interval, shutdown_token);
        Self {
            dir,
            sweep_handle: Mutex::new(Some(sweep_handle)),
        }
    }

    /// Write `bytes` to their content-addressed blob file `dest_name`.
    ///
    /// Tmp-then-rename so a partially written blob is never visible under
    /// its final name. All I/O is async `tokio::fs` (the house file-I/O
    /// style, matching `camel-file`'s `atomic_write`).
    async fn write_blob(&self, dest_name: &str, bytes: &[u8]) -> std::io::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let dest_path = self.dir.join(dest_name);
        let (mut file, tmp_path) = self.open_tmp_exclusive(dest_name).await?;

        // Best-effort tmp cleanup on failure: a leaked `.tmp` would never
        // be reclaimed by the epoch sweeper.
        if let Err(e) = file.write_all(bytes).await {
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
        Ok(())
    }

    /// Open a unique exclusive tmp file next to `dest_name`, retrying name
    /// collisions with a fresh nonce (bounded by [`TMP_NAME_ATTEMPTS`]).
    ///
    /// The nonce hashes `dest_name || clock_nanos || attempt_counter`, so
    /// retries still produce fresh names under a frozen clock.
    async fn open_tmp_exclusive(
        &self,
        dest_name: &str,
    ) -> std::io::Result<(tokio::fs::File, PathBuf)> {
        let clock_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut last_collision: Option<std::io::Error> = None;
        for attempt in 0..TMP_NAME_ATTEMPTS {
            let mut hasher = blake3::Hasher::new();
            hasher.update(dest_name.as_bytes());
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

    /// Best-effort unlink of every entry of the payload dir.
    ///
    /// Per-file `NotFound` is success (a concurrent sweeper or replica may
    /// have reclaimed the blob already); any other per-file error WARNs
    /// and iteration continues. [`Self::clear`] must never surface its
    /// own unlink failures as `Err`.
    async fn clear_payload_dir_best_effort(&self) {
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
impl PayloadStore for DiskPayloadStore {
    async fn put(
        &self,
        name: &str,
        bytes: &[u8],
        _death_epoch: SystemTime,
    ) -> Result<(), CamelError> {
        // Trait contract (ADR-0065): a payload name is a bare single path
        // component. The same sanitize guard `read`/`unlink` apply keeps a
        // separator or `..` from ever escaping the payload dir through a
        // write.
        let name = sanitize_blob_name(name).ok_or_else(|| {
            CamelError::Config(format!(
                "cache payload name must be a bare file name, got '{name}'"
            ))
        })?;
        // The death epoch is already encoded in the blob name; the
        // filename-epoch sweeper owns reclamation, so the deadline needs
        // no separate storage on the disk tier.
        self.write_blob(name, bytes).await.map_err(|e| {
            CamelError::Io(format!(
                "cache blob write '{}': {e}",
                self.dir.join(name).display()
            ))
        })
    }

    async fn read(&self, name: &str) -> Result<Option<Vec<u8>>, CamelError> {
        let Some(name) = sanitize_blob_name(name) else {
            return Err(CamelError::Io(format!(
                "cache payload name must be a bare file name, got '{name}'"
            )));
        };
        let blob_path = self.dir.join(name);
        match tokio::fs::read(&blob_path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(CamelError::Io(format!(
                "cache payload blob read '{}': {e}",
                blob_path.display()
            ))),
        }
    }

    async fn unlink(&self, name: &str) -> Result<(), CamelError> {
        let Some(name) = sanitize_blob_name(name) else {
            return Err(CamelError::Io(format!(
                "cache payload name must be a bare file name, got '{name}'"
            )));
        };
        let blob_path = self.dir.join(name);
        match tokio::fs::remove_file(&blob_path).await {
            Ok(()) => Ok(()),
            // NotFound = a concurrent sweeper or replica won the race.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CamelError::Io(format!(
                "cache payload blob unlink '{}': {e}",
                blob_path.display()
            ))),
        }
    }

    /// Reclaim payload space now: best-effort unlink of every entry of
    /// the payload dir. Unlink failures never turn `clear` into `Err` —
    /// each failure WARNs and the rest of the dir is still attempted.
    async fn clear(&self) {
        self.clear_payload_dir_best_effort().await;
    }
}

impl std::fmt::Debug for DiskPayloadStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskPayloadStore")
            .field("dir", &self.dir)
            .field("sweep_attached", &self.sweep_handle.lock().is_some())
            .finish()
    }
}

impl Drop for DiskPayloadStore {
    fn drop(&mut self) {
        // Abort ONLY the sweep task. Never cancel the context-owned token —
        // that would shut down the entire context when one store drops.
        if let Some(handle) = self.sweep_handle.lock().take() {
            handle.abort();
        }
    }
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
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
struct SweepStats {
    /// Dead blobs unlinked this pass.
    blobs_unlinked: u64,
    /// Bytes reclaimed with those dead blobs.
    blob_bytes_reclaimed: u64,
    /// Stale tmp files unlinked this pass.
    tmps_unlinked: u64,
    /// Blobs still on disk after the pass (live, orphan pre-epoch, or
    /// foreign names — anything the sweep kept; a blob that vanishes
    /// mid-pass via the ENOENT race is counted here until the next pass).
    live_blobs: u64,
    /// Total bytes of those surviving blobs.
    live_blob_bytes: u64,
}

async fn sweep_payload_dir(dir: &Path, now: SystemTime, sweep_interval: Duration) -> SweepStats {
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(read_dir) => read_dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SweepStats::default(),
        Err(e) => {
            warn!(
                dir = %dir.display(),
                error = %e,
                "cache payload dir read failed during sweep (skipped)"
            );
            return SweepStats::default();
        }
    };
    let mut stats = SweepStats::default();
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
        let is_tmp = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".tmp"));
        let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
        match unlink_payload_file(&path, now, sweep_interval).await {
            Ok(true) => {
                if is_tmp {
                    stats.tmps_unlinked += 1;
                } else {
                    stats.blobs_unlinked += 1;
                    stats.blob_bytes_reclaimed += size;
                }
            }
            Ok(false) => {
                if !is_tmp {
                    stats.live_blobs += 1;
                    stats.live_blob_bytes += size;
                }
            }
            Err(e) => warn!(
                dir = %dir.display(),
                file = %path.display(),
                error = %e,
                "cache payload file unlink failed during sweep (skipped)"
            ),
        }
    }
    stats
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
                    let s = sweep_payload_dir(&dir, SystemTime::now(), sweep_interval).await;
                    // Per-pass volume observability (bd rc-h3dp): live
                    // bytes are the high-water baseline operators compare
                    // against the eager-reclaim trigger; reclaimed bytes
                    // show the pass's cleanup.
                    info!(
                        dir = %dir.display(),
                        live_blobs = s.live_blobs,
                        live_blob_bytes = s.live_blob_bytes,
                        blobs_unlinked = s.blobs_unlinked,
                        blob_bytes_reclaimed = s.blob_bytes_reclaimed,
                        tmps_unlinked = s.tmps_unlinked,
                        "cache payload sweep pass"
                    );
                }
                _ = shutdown_token.cancelled() => break,
            }
        }
    })
}

#[cfg(test)]
#[path = "disk_offload_tests.rs"]
mod disk_offload_tests;

#[cfg(test)]
#[path = "disk_offload_reclaim_tests.rs"]
mod disk_offload_reclaim_tests;
