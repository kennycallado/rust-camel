/// Default stream cache threshold (128 KB).
///
/// Streams exceeding this size will fail with [`CamelError::StreamLimitExceeded`].
/// This is intentionally conservative for OOM protection.
/// For general-purpose materialization, use [`Body::materialize()`] (10 MB default).
pub const DEFAULT_STREAM_CACHE_THRESHOLD: usize = 128 * 1024;

/// Configuration for [`StreamCacheService`](camel_processor::StreamCacheService).
///
/// Controls the maximum bytes a `Body::Stream` may consume when materialized
/// into `Body::Bytes`. Can be set globally via `[stream_caching]` in Camel.toml
/// or per-step via `stream_cache: { threshold: N }` in YAML routes.
#[derive(Debug, Clone, Copy)]
pub struct StreamCacheConfig {
    /// Maximum bytes to buffer when materializing a stream.
    pub threshold: usize,
}

impl Default for StreamCacheConfig {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_STREAM_CACHE_THRESHOLD,
        }
    }
}

impl StreamCacheConfig {
    /// Create a new config with the given threshold in bytes.
    pub fn new(threshold: usize) -> Self {
        Self { threshold }
    }
}
