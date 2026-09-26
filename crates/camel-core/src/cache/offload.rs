//! Payload-offload decorator for [`CacheRepository`] backends, with a
//! pluggable [`PayloadStore`].
//!
//! [`OffloadRepository`] wraps any backend (the "index") and moves entry
//! payloads into a [`PayloadStore`] (local disk, redis, …), storing only
//! the store-side blob name in the index row. Index rows stay small; the
//! payload is re-injected on `get`/`peek_stale`.
//!
//! # Blob lifecycle
//!
//! Blob names are `{blake3-128hex(key)}.{death_epoch_secs}.{blake3-128hex(
//! bytes || content_type-discriminant)}.blob`. The death epoch —
//! `expires_at + stale_retention + sweep_interval` — is encoded in the
//! name so store-side reclaimers can reclaim dead payloads by name alone,
//! without consulting the index.
//!
//! # Failure policy
//!
//! - A payload put that fails falls back to storing the entry inline in
//!   the index (WARN + `inner.set` with the original entry): the
//!   decorator never converts a store failure into a cache-write `Err`.
//! - A vanished or corrupt blob row degrades to a miss (`Ok(None)` + WARN).
//! - A payload that exists but cannot be read (e.g. `PermissionDenied`)
//!   surfaces as `Err` per ADR-0023 Contract C1.

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
use tracing::warn;

/// Injectable wall clock for death-epoch math and deterministic tests.
///
/// Mirrors `ClockFn` in `camel-redis-repo::cache_repo`.
pub type OffloadClock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// The default production clock: [`SystemTime::now`].
pub fn default_offload_clock() -> OffloadClock {
    Arc::new(SystemTime::now)
}

/// Pluggable payload storage behind [`OffloadRepository`].
///
/// The decorator owns ALL policy — blob naming, death-epoch math, the
/// index-row shape, and every fallback (ADR-0065) — and calls the store
/// with finished blob names. Implementations only move opaque bytes.
#[async_trait]
pub trait PayloadStore: Send + Sync {
    /// Store `bytes` under `name` until `death_epoch`.
    ///
    /// `name` is the decorator's content-addressed blob name; stores must
    /// reject names that are not bare single path components. A store that
    /// cannot honor `death_epoch` returns `Err` — the decorator's inline
    /// fallback applies, so a payload is never stored without its
    /// deadline.
    async fn put(
        &self,
        name: &str,
        bytes: &[u8],
        death_epoch: SystemTime,
    ) -> Result<(), CamelError>;

    /// Read the payload stored under `name`.
    ///
    /// `Ok(None)` = the payload is gone (reclaimed, evicted, lost): the
    /// decorator degrades to a MISS with WARN. An existing-but-unreadable
    /// payload is `Err` (ADR-0023 Contract C1), never a silent `None`.
    async fn read(&self, name: &str) -> Result<Option<Vec<u8>>, CamelError>;

    /// Remove the payload stored under `name`.
    ///
    /// An already-absent payload counts as success (a concurrent
    /// reclaimer won the race).
    async fn unlink(&self, name: &str) -> Result<(), CamelError>;

    /// Best-effort bulk reclamation of every stored payload.
    ///
    /// Default: no-op — a store whose payloads carry their own deadline
    /// (e.g. a redis EXAT) or a background sweeper needs no eager clear.
    /// Implementers must WARN and continue on per-payload failures: the
    /// decorator never surfaces `clear` failures as `Err` (ADR-0065
    /// failure policy).
    async fn clear(&self) {}
}

/// [`CacheRepository`] decorator that offloads entry payloads to a
/// [`PayloadStore`].
///
/// Wraps any index backend; see the [module docs](self) for the blob
/// lifecycle and failure policy. `stale_retention`, `sweep_interval`, and
/// `payload_max_ttl` must be non-zero (the payload intervals at least one
/// second — the death epoch truncates to whole seconds) — enforced by
/// `CacheRepoConfig` validation, not here.
pub struct OffloadRepository {
    /// Decorated index backend (memory, redb, redis, …).
    inner: Arc<dyn CacheRepository>,
    /// Payload store holding the offloaded bytes.
    store: Arc<dyn PayloadStore>,
    /// How long an expired entry stays peekable before reclamation.
    stale_retention: Duration,
    /// Background sweep cadence of the store; its length is the
    /// death-epoch grace.
    sweep_interval: Duration,
    /// Fabricated TTL for entries stored without an explicit one.
    payload_max_ttl: Duration,
    /// Wall clock for death-epoch math.
    clock: OffloadClock,
}

impl OffloadRepository {
    /// Wrap `inner` with payload offload through `store` (production
    /// clock).
    pub fn new(
        inner: Arc<dyn CacheRepository>,
        store: Arc<dyn PayloadStore>,
        stale_retention: Duration,
        sweep_interval: Duration,
        payload_max_ttl: Duration,
    ) -> Self {
        Self::with_clock(
            inner,
            store,
            stale_retention,
            sweep_interval,
            payload_max_ttl,
            default_offload_clock(),
        )
    }

    /// Test seam: [`Self::new`] with an injected [`OffloadClock`].
    ///
    /// The injected clock drives death-epoch math only; the store's own
    /// reclaim machinery (sweeper, TTL) always runs on the real clock.
    pub fn with_clock(
        inner: Arc<dyn CacheRepository>,
        store: Arc<dyn PayloadStore>,
        stale_retention: Duration,
        sweep_interval: Duration,
        payload_max_ttl: Duration,
        clock: OffloadClock,
    ) -> Self {
        Self {
            inner,
            store,
            stale_retention,
            sweep_interval,
            payload_max_ttl,
            clock,
        }
    }

    /// Re-inject the offloaded payload into an index row (shared by `get`
    /// and `peek_stale`).
    ///
    /// Rows without `payload_path` pass through untouched (legacy/inline).
    /// A corrupt path or a vanished payload degrades to a miss; a payload
    /// that exists but cannot be read surfaces as `Err` (Contract C1).
    pub(crate) async fn hydrate(
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
        match self.store.read(name).await {
            Ok(Some(bytes)) => {
                entry.bytes = bytes;
                entry.payload_path = None;
                Ok(Some(entry))
            }
            Ok(None) => {
                warn!(
                    key = key,
                    backend = self.inner.name(),
                    blob = %name,
                    "cache payload blob gone; treating as miss"
                );
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Best-effort eager unlink of a key's predecessor payload after a
    /// successful overwrite (ADR-0065, amendment "bd rc-uteoa").
    ///
    /// Row-guided, no store scan: only the name the pre-swap index row
    /// carried is eligible, and only when it passes
    /// [`sanitize_blob_name`], carries a parseable death epoch, and starts
    /// with the current key's blake3-128 filename prefix — a corrupt row
    /// naming another key's payload (or a foreign name) is never unlinked.
    /// `keep_name` is the fresh payload's name on the successful-put path:
    /// a same-second identical rewrite reuses the name, and only the
    /// fresh payload owns it, so an equal name skips the reclaim. On the
    /// inline-fallback path no fresh payload owns any name; callers pass
    /// `None` to disable the equal-name guard. An absent payload counts
    /// as reclaimed by someone else; any other unlink failure WARNs once
    /// and leaves the payload to its store's reclaimer at the death
    /// epoch. The function never returns `Err`: the reclaim adds no
    /// failure mode to `set`.
    async fn reclaim_predecessor(
        &self,
        key: &str,
        old_name: Option<&str>,
        keep_name: Option<&str>,
    ) {
        let Some(old_name) = old_name else {
            return;
        };
        if keep_name == Some(old_name) {
            return;
        }
        let key_prefix = format!("{}.", blake3_128hex(key.as_bytes()));
        let eligible = sanitize_blob_name(old_name).is_some()
            && parse_death_epoch(old_name).is_some()
            && old_name.starts_with(&key_prefix);
        if !eligible {
            return;
        }
        if let Err(e) = self.store.unlink(old_name).await {
            warn!(
                key = key,
                backend = self.inner.name(),
                error = %e,
                "eager reclaim of predecessor blob failed; sweeper reclaims it at its death epoch"
            );
        }
    }
}

#[async_trait]
impl CacheRepository for OffloadRepository {
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
        // Capture the predecessor's blob name before the index swap so a
        // successful overwrite can reclaim it eagerly (ADR-0065 amendment,
        // "bd rc-uteoa"). The SILENT maintenance read keeps the capture off
        // every counted path — a phantom miss on first write or a phantom
        // hit on overwrite would distort /ops/cache/stats and
        // camel_cache_{hits,misses}_total. A failed read only skips the
        // reclaim — the write proceeds unchanged in every case.
        let old_name = match self.inner.peek_row_silent(key).await {
            Ok(Some(row)) => row.payload_path,
            Ok(None) => None,
            Err(e) => {
                warn!(
                    key = key,
                    backend = self.inner.name(),
                    error = %e,
                    "pre-swap row read failed; skipping eager reclaim"
                );
                None
            }
        };
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
        // The same instant as a deadline, for stores that carry their own
        // expiration next to the name-encoded epoch (e.g. a redis EXAT).
        // The u64 filename seconds can exceed the platform's SystemTime
        // range only for absurd clocks; the clamp stays a bounded
        // deadline either way.
        let deadline = UNIX_EPOCH
            .checked_add(Duration::from_secs(death_epoch))
            .unwrap_or(UNIX_EPOCH);
        let dest_name = blob_filename(key, death_epoch, &entry);

        match self.store.put(&dest_name, &entry.bytes, deadline).await {
            Ok(()) => {
                entry.bytes = Vec::new();
                // Clone: `dest_name` is still needed for the equal-name
                // guard after `entry` (carrying the same name) moves into
                // the inner set.
                entry.payload_path = Some(dest_name.clone());
                // The ttl MUST be Some: every inner overwrites
                // `expires_at` from the ttl argument, so None would wipe
                // the fabricated expiry. The inner recomputes `expires_at`
                // from its own clock; the sub-second skew is absorbed by
                // the death-epoch grace.
                let result = self.inner.set(key, entry, Some(effective_ttl)).await;
                // Reclaim only after the inner accepted the swap: on an
                // error the surviving row may still reference the
                // predecessor payload.
                if result.is_ok() {
                    self.reclaim_predecessor(key, old_name.as_deref(), Some(&dest_name))
                        .await;
                }
                result
            }
            Err(e) => {
                warn!(
                    key = key,
                    backend = self.inner.name(),
                    error = %e,
                    "cache blob write failed; storing entry inline instead"
                );
                // Inline fallback with the original, unstripped entry: the
                // decorator never converts a store-write failure into a
                // cache-write error. The CAPPED ttl keeps the spec's
                // no-TTL semantic (payload_max_ttl) even for degraded rows
                // — an uncapped inline row would never be reclaimed. The
                // new row no longer references the predecessor, so the
                // reclaim runs with the equal-name guard disabled (the
                // failed write left no fresh payload owning that name).
                let result = self.inner.set(key, entry, Some(effective_ttl)).await;
                if result.is_ok() {
                    self.reclaim_predecessor(key, old_name.as_deref(), None)
                        .await;
                }
                result
            }
        }
    }

    async fn peek_stale(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        match self.inner.peek_stale(key).await? {
            Some(entry) => self.hydrate(key, entry).await,
            None => Ok(None),
        }
    }

    /// Delegate-only: the index row is dropped here; the payload becomes
    /// an orphan reclaimed asynchronously at its name-encoded death
    /// epoch.
    async fn invalidate(&self, key: &str) -> Result<(), CamelError> {
        self.inner.invalidate(key).await
    }

    /// Reclaim payload space now: best-effort bulk reclamation in the
    /// store, then delegate to the index. Store failures never turn
    /// `clear` into `Err` — the store WARNs per payload and continues
    /// ([`PayloadStore::clear`] contract).
    async fn clear(&self) -> Result<(), CamelError> {
        self.store.clear().await;
        self.inner.clear().await
    }

    /// Delegate-only: the returned count is index-scoped; payloads are
    /// reclaimed asynchronously at their name-encoded death epoch.
    async fn invalidate_prefix(&self, prefix: &str) -> Result<u64, CamelError> {
        self.inner.invalidate_prefix(prefix).await
    }

    async fn stats(&self) -> CacheStats {
        self.inner.stats().await
    }
}

impl std::fmt::Debug for OffloadRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OffloadRepository")
            .field("inner", &self.inner)
            .field("stale_retention", &self.stale_retention)
            .field("sweep_interval", &self.sweep_interval)
            .field("payload_max_ttl", &self.payload_max_ttl)
            .finish()
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
pub(crate) fn hasher_128hex(hasher: blake3::Hasher) -> String {
    let hex = hasher.finalize().to_hex().to_string();
    hex[..32].to_string()
}

/// blake3-128 hex of a single byte slice.
pub(crate) fn blake3_128hex(data: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(data);
    hasher_128hex(hasher)
}

/// 128-bit content fingerprint: `blake3(bytes || content_type discriminant)`.
pub(crate) fn content_fingerprint(entry: &CacheEntry) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&entry.bytes);
    hasher.update(&[content_type_discriminant(entry.content_type)]);
    hasher_128hex(hasher)
}

/// Blob name: `{key-hash}.{death_epoch}.{fingerprint}.blob`.
fn blob_filename(key: &str, death_epoch: u64, entry: &CacheEntry) -> String {
    format!(
        "{}.{}.{}.blob",
        blake3_128hex(key.as_bytes()),
        death_epoch,
        content_fingerprint(entry)
    )
}

/// Death epoch (second dot-separated component) of a blob name, if it
/// parses as `u64`.
pub(crate) fn parse_death_epoch(file_name: &str) -> Option<u64> {
    file_name.split('.').nth(1)?.parse().ok()
}

/// Accept only a bare file name: non-empty, no `/`, no `\`, no `..`.
///
/// Absolute paths necessarily contain a separator on both Unix and Windows,
/// so the separator checks subsume the absolute-path rejection. Everything
/// else is treated as a corrupt row.
pub(crate) fn sanitize_blob_name(path: &str) -> Option<&str> {
    if path.is_empty() || path.contains('/') || path.contains('\\') || path.contains("..") {
        return None;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::MemoryCacheRepository;

    fn entry(bytes: Vec<u8>, content_type: ContentType) -> CacheEntry {
        CacheEntry {
            bytes,
            payload_path: None,
            content_type,
            expires_at: None,
        }
    }

    /// Store whose `put` always fails: models a payload store rejecting
    /// the write (e.g. a redis EXAT overflow). The decorator must fall
    /// back to inline storage, never surface the failure.
    struct FailingPutStore;

    #[async_trait]
    impl PayloadStore for FailingPutStore {
        async fn put(
            &self,
            _name: &str,
            _bytes: &[u8],
            _death_epoch: SystemTime,
        ) -> Result<(), CamelError> {
            Err(CamelError::Io(
                "failing-put-store: injected put failure".into(),
            ))
        }

        async fn read(&self, _name: &str) -> Result<Option<Vec<u8>>, CamelError> {
            Ok(None)
        }

        async fn unlink(&self, _name: &str) -> Result<(), CamelError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn offload_repository_inline_fallback_on_store_put_failure() {
        let inner = Arc::new(MemoryCacheRepository::new("test", 100));
        let repo = OffloadRepository::new(
            inner,
            Arc::new(FailingPutStore),
            Duration::from_secs(168 * 3600),
            Duration::from_secs(3600),
            Duration::from_secs(24 * 3600),
        );

        let payload = vec![7; 128];
        repo.set(
            "k",
            entry(payload.clone(), ContentType::Bytes),
            Some(Duration::from_secs(60)),
        )
        .await
        .expect("set must fall back inline, never Err");

        let got = repo.get("k").await.expect("get").expect("present");
        assert_eq!(
            got.bytes, payload,
            "bytes must be present, stored inline in the index"
        );
        assert_eq!(got.content_type, ContentType::Bytes);
        assert!(got.payload_path.is_none(), "fallback row stays inline");
    }

    /// Store that accepts every put but never serves a read: models a
    /// payload vanished from the store (reclaimed, evicted, lost).
    struct ForgetfulStore;

    #[async_trait]
    impl PayloadStore for ForgetfulStore {
        async fn put(
            &self,
            _name: &str,
            _bytes: &[u8],
            _death_epoch: SystemTime,
        ) -> Result<(), CamelError> {
            Ok(())
        }

        async fn read(&self, _name: &str) -> Result<Option<Vec<u8>>, CamelError> {
            Ok(None)
        }

        async fn unlink(&self, _name: &str) -> Result<(), CamelError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn offload_repository_missing_payload_is_miss() {
        let inner = Arc::new(MemoryCacheRepository::new("test", 100));
        let repo = OffloadRepository::new(
            inner,
            Arc::new(ForgetfulStore),
            Duration::from_secs(168 * 3600),
            Duration::from_secs(3600),
            Duration::from_secs(24 * 3600),
        );

        repo.set(
            "k",
            entry(vec![1, 2, 3], ContentType::Bytes),
            Some(Duration::from_secs(60)),
        )
        .await
        .expect("set");

        assert_eq!(
            repo.get("k").await.expect("get"),
            None,
            "a vanished payload degrades to a MISS, never an error"
        );
    }
}
