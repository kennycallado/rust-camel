//! In-memory cache backend backed by [`moka`].
//!
//! Size-eviction only. No TTL-based expiration is configured at the
//! moka level — expiry is checked in-band during [`CacheRepository::get`].

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

/// In-memory cache repository with size-based eviction.
///
/// Uses [`moka::future::Cache`] with a max-capacity bound. Expiry is
/// checked in-band in [`get`](CacheRepository::get) — no
/// `expire_after` or `time_to_live` is configured on the moka cache.
pub struct MemoryCacheRepository {
    name: String,
    inner: moka::future::Cache<String, CacheEntry>,
    hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
    evictions: Arc<AtomicU64>,
    peek_stale_served: Arc<AtomicU64>,
    invalidations: Arc<AtomicU64>,
}

impl std::fmt::Debug for MemoryCacheRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryCacheRepository")
            .field("name", &self.name)
            .field("stats", &self.stats_snapshot())
            .finish()
    }
}

impl MemoryCacheRepository {
    /// Create a new memory-backed cache repository.
    ///
    /// `max_capacity` is the maximum number of entries before eviction.
    pub fn new(name: impl Into<String>, max_capacity: usize) -> Self {
        let hits = Arc::new(AtomicU64::new(0));
        let misses = Arc::new(AtomicU64::new(0));
        let evictions = Arc::new(AtomicU64::new(0));
        let peek_stale_served = Arc::new(AtomicU64::new(0));
        let invalidations = Arc::new(AtomicU64::new(0));
        let evictions_clone = Arc::clone(&evictions);

        let inner = moka::future::CacheBuilder::new(max_capacity as u64)
            .eviction_listener(move |_key, _value, cause| {
                // Only count capacity-driven size evictions — NOT explicit invalidate() calls.
                if cause == moka::notification::RemovalCause::Size {
                    evictions_clone.fetch_add(1, Ordering::Relaxed);
                }
            })
            .build();

        Self {
            name: name.into(),
            inner,
            hits,
            misses,
            evictions,
            peek_stale_served,
            invalidations,
        }
    }

    /// Synchronous snapshot of the current stats, shared by the async trait
    /// method and the [`std::fmt::Debug`] impl (which cannot await).
    fn stats_snapshot(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            entries: self.inner.entry_count(),
            peek_stale_served: self.peek_stale_served.load(Ordering::Relaxed),
            invalidations: self.invalidations.load(Ordering::Relaxed),
            bytes: None,
        }
    }
}

#[async_trait]
impl CacheRepository for MemoryCacheRepository {
    fn name(&self) -> &str {
        &self.name
    }

    async fn get(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        match self.inner.get(key).await {
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
        self.inner.insert(key.to_string(), value).await;
        Ok(())
    }

    async fn peek_stale(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        let entry = self.inner.get(key).await;
        if entry.is_some() {
            self.peek_stale_served.fetch_add(1, Ordering::Relaxed);
        }
        Ok(entry)
    }

    async fn invalidate(&self, key: &str) -> Result<(), CamelError> {
        self.inner.invalidate(key).await;
        self.invalidations.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn clear(&self) -> Result<(), CamelError> {
        self.inner.invalidate_all();
        Ok(())
    }

    async fn stats(&self) -> CacheStats {
        self.stats_snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> CacheEntry {
        CacheEntry {
            bytes: vec![1, 2, 3],
            payload_path: None,
            content_type: camel_api::cache::ContentType::Bytes,
            expires_at: None,
        }
    }

    #[tokio::test]
    async fn get_returns_none_on_miss_some_on_hit() {
        let repo = MemoryCacheRepository::new("test", 100);

        repo.set("k", entry(), Some(Duration::from_secs(3600)))
            .await
            .unwrap();
        let found = repo.get("k").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().bytes, vec![1, 2, 3]);
        assert_eq!(repo.get("absent").await.unwrap(), None);
    }

    #[tokio::test]
    async fn get_returns_none_after_expiry_peek_stale_returns_entry() {
        let repo = MemoryCacheRepository::new("test", 100);

        repo.set("k", entry(), Some(Duration::from_millis(1)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(repo.get("k").await.unwrap(), None);
        let stale = repo.peek_stale("k").await.unwrap();
        assert!(stale.is_some());
        assert_eq!(stale.unwrap().bytes, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn set_with_none_ttl_stores_without_expiry() {
        let repo = MemoryCacheRepository::new("test", 100);

        repo.set("k", entry(), None).await.unwrap();
        let found = repo.get("k").await.unwrap();
        assert!(found.is_some());
        assert!(found.unwrap().expires_at.is_none());
    }

    #[tokio::test]
    async fn invalidate_is_noop_on_absent_key() {
        let repo = MemoryCacheRepository::new("test", 100);
        repo.invalidate("absent").await.unwrap();
    }

    #[tokio::test]
    async fn max_capacity_bounds_entry_count() {
        let repo = MemoryCacheRepository::new("test", 2);

        repo.set("a", entry(), Some(Duration::from_secs(3600)))
            .await
            .unwrap();
        repo.set("b", entry(), Some(Duration::from_secs(3600)))
            .await
            .unwrap();
        repo.set("c", entry(), Some(Duration::from_secs(3600)))
            .await
            .unwrap();
        repo.inner.run_pending_tasks().await;
        assert!(repo.inner.entry_count() <= 2);
    }

    #[tokio::test]
    async fn stats_reflects_hits_misses_evictions_entries() {
        let repo = MemoryCacheRepository::new("test", 100);

        repo.set("k", entry(), Some(Duration::from_secs(3600)))
            .await
            .unwrap();
        repo.get("k").await.unwrap(); // hit
        repo.get("absent").await.unwrap(); // miss
        repo.inner.run_pending_tasks().await;

        let stats = repo.stats().await;
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert!(stats.entries >= 1, "entries was {}", stats.entries);
    }

    #[tokio::test]
    async fn stats_reports_peek_stale_served() {
        let repo = MemoryCacheRepository::new("test", 100);

        repo.set("k", entry(), Some(Duration::from_millis(1)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(repo.get("k").await.unwrap(), None); // miss/expired
        assert!(repo.peek_stale("k").await.unwrap().is_some());
        assert_eq!(repo.stats().await.peek_stale_served, 1);
    }

    #[tokio::test]
    async fn stats_reports_invalidations_per_operation() {
        let repo = MemoryCacheRepository::new("test", 100);

        repo.set("a", entry(), None).await.unwrap();
        repo.invalidate("a").await.unwrap();
        repo.invalidate("absent").await.unwrap();
        assert_eq!(repo.stats().await.invalidations, 2);
    }

    #[tokio::test]
    async fn clear_empties_repository() {
        let repo = MemoryCacheRepository::new("test", 100);

        repo.set("a", entry(), None).await.unwrap();
        repo.set("b", entry(), None).await.unwrap();
        repo.clear().await.unwrap();
        assert_eq!(repo.get("a").await.unwrap(), None);
        assert_eq!(repo.get("b").await.unwrap(), None);
    }

    #[tokio::test]
    async fn evictions_incremented_on_size_pressure() {
        let repo = MemoryCacheRepository::new("test", 1);

        repo.set("a", entry(), None).await.unwrap();
        repo.set("b", entry(), None).await.unwrap();
        repo.inner.run_pending_tasks().await;
        assert!(repo.stats().await.evictions >= 1);
    }

    #[tokio::test]
    async fn invalidate_prefix_on_memory_fails_naming_backend() {
        let repo = MemoryCacheRepository::new("test", 100);
        let err = repo.invalidate_prefix("ns:").await.unwrap_err();
        assert!(
            format!("{err}").contains("test"),
            "error must name the backend, got: {err}"
        );
    }
}
