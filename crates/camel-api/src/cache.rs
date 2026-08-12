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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CacheStats {
    /// Number of cache hits.
    pub hits: u64,
    /// Number of cache misses.
    pub misses: u64,
    /// Number of entries evicted.
    pub evictions: u64,
    /// Current number of entries in the cache.
    pub entries: u64,
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
            }
        );
    }
}
