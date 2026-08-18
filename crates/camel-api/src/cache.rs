//! # Cache Repository
//!
//! Pluggable cache backend abstraction for the Caching EIP.
//!
//! ## Contract
//!
//! Per ADR-0023 Contract C1, backend failures (network errors, timeouts, storage
//! unavailability) MUST surface as `Err(CamelError)`, never as a silent miss.
//! A `get` that returns `Ok(None)` means the key is definitively absent or
//! expired — not "I couldn't check."

use std::time::{Duration, SystemTime};

use crate::CamelError;

/// A cached entry with its content type and optional expiry.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CacheEntry {
    /// The raw cached bytes.
    pub bytes: Vec<u8>,
    /// The content type of the cached data.
    pub content_type: ContentType,
    /// When this entry expires (if set). `None` means no expiry.
    pub expires_at: Option<SystemTime>,
}

/// The content type classification for a cached entry.
///
/// exhaustive-by-contract: closed 4-variant set; out-of-crate CacheService matches all variants
/// for content_type→Body reconstruction (ADR-0049 §Exceptions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ContentType {
    /// Arbitrary binary data.
    Bytes,
    /// UTF-8 text.
    Text,
    /// JSON-encoded data.
    Json,
    /// XML-encoded data.
    Xml,
}

/// Cache usage statistics.
///
/// Backends construct this with struct literals — NOT `#[non_exhaustive]`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CacheStats {
    /// Number of cache hits.
    pub hits: u64,
    /// Number of cache misses.
    pub misses: u64,
    /// Number of entries evicted.
    pub evictions: u64,
    /// Current number of entries in the cache.
    pub entries: u64,
    /// Number of peek_stale serves (fresh or stale).
    pub peek_stale_served: u64,
    /// Number of successful invalidation operations.
    pub invalidations: u64,
    /// Total stored payload bytes when the backend can report it; None = cannot.
    pub bytes: Option<u64>,
}

/// Pluggable cache backend.
///
/// Implementations MUST be `Send + Sync + 'static` and propagate all backend
/// failures as `Err(CamelError)` (Contract C1).
#[async_trait::async_trait]
pub trait CacheRepository: Send + Sync + std::fmt::Debug + 'static {
    /// A human-readable name for this cache backend.
    fn name(&self) -> &str;

    /// Retrieve a value by key.
    ///
    /// Returns `Ok(None)` if the key is absent or expired. All backend failures
    /// MUST be returned as `Err(CamelError)`.
    async fn get(&self, key: &str) -> Result<Option<CacheEntry>, CamelError>;

    /// Store a value with an optional TTL.
    ///
    /// The implementation computes `expires_at` from `ttl` and stores it in the
    /// `CacheEntry`. If `ttl` is `None`, the entry has no expiry.
    async fn set(
        &self,
        key: &str,
        value: CacheEntry,
        ttl: Option<Duration>,
    ) -> Result<(), CamelError>;

    /// Peek at a stale (expired but not yet evicted) entry.
    ///
    /// Returns `Ok(None)` if the key is absent or not yet expired.
    async fn peek_stale(&self, key: &str) -> Result<Option<CacheEntry>, CamelError>;

    /// Remove a specific key from the cache.
    async fn invalidate(&self, key: &str) -> Result<(), CamelError>;

    /// Remove all entries from the cache.
    async fn clear(&self) -> Result<(), CamelError>;

    /// Remove every entry whose key starts with `prefix`, returning the removed count.
    ///
    /// Default implementation reports the limitation for backends without key
    /// iteration — it does NOT return `Ok(0)` pretending an empty namespace.
    /// Backends with ordered keys override this with range deletion.
    async fn invalidate_prefix(&self, prefix: &str) -> Result<u64, CamelError> {
        let _ = prefix;
        Err(CamelError::Config(format!(
            "cache backend '{}' does not support invalidate_prefix (no key iteration)",
            self.name()
        )))
    }

    /// Return current cache statistics.
    ///
    /// Default implementation returns zeroed stats.
    fn stats(&self) -> CacheStats {
        CacheStats::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_entry_construction() {
        let entry = CacheEntry {
            bytes: vec![b'x'],
            content_type: ContentType::Bytes,
            expires_at: None,
        };
        assert_eq!(entry.bytes.len(), 1);
        assert_eq!(entry.content_type, ContentType::Bytes);
    }

    #[test]
    fn cache_stats_default() {
        assert_eq!(
            CacheStats::default(),
            CacheStats {
                hits: 0,
                misses: 0,
                evictions: 0,
                entries: 0,
                peek_stale_served: 0,
                invalidations: 0,
                bytes: None,
            }
        );
    }

    #[test]
    fn cache_stats_serialize_round_trip() {
        let stats = CacheStats {
            hits: 2,
            misses: 1,
            evictions: 0,
            entries: 3,
            peek_stale_served: 4,
            invalidations: 1,
            bytes: None,
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("\"peek_stale_served\""));
        assert!(json.contains("\"bytes\":null"));
        let back: CacheStats = serde_json::from_str(&json).unwrap();
        assert_eq!(stats, back);
    }

    /// A backend with no key-iteration support: exercises the default
    /// `invalidate_prefix` (which must report the limitation, not fake `Ok(0)`).
    struct NoIter;

    impl std::fmt::Debug for NoIter {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("NoIter")
        }
    }

    #[async_trait::async_trait]
    impl CacheRepository for NoIter {
        fn name(&self) -> &str {
            "noiter"
        }

        async fn get(&self, _key: &str) -> Result<Option<CacheEntry>, CamelError> {
            Ok(None)
        }

        async fn set(
            &self,
            _key: &str,
            _value: CacheEntry,
            _ttl: Option<Duration>,
        ) -> Result<(), CamelError> {
            Ok(())
        }

        async fn peek_stale(&self, _key: &str) -> Result<Option<CacheEntry>, CamelError> {
            Ok(None)
        }

        async fn invalidate(&self, _key: &str) -> Result<(), CamelError> {
            Ok(())
        }

        async fn clear(&self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn default_invalidate_prefix_returns_err_naming_backend() {
        let repo = NoIter;
        let err = repo.invalidate_prefix("ns:").await.unwrap_err();
        assert!(
            format!("{err}").contains("noiter"),
            "error must name the backend, got: {err}"
        );
    }
}
