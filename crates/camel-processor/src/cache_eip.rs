//! Cache EIP — outcome-aware Segment implementation.
//!
//! Implements the Caching pattern (lookup → on-miss sub-pipeline → write-back)
//! at the `OutcomePipeline` layer (one layer above Tower), mirroring
//! [`IdempotentConsumerSegment`]. On a cache HIT the body is reconstructed from
//! the stored [`CacheEntry`] and the on-miss sub-pipeline is skipped entirely;
//! on a MISS the sub-pipeline runs and its result body is written back into the
//! repository (subject to `max_entry_bytes`).
//!
//! # Why Segment-mode (NOT Process-mode)
//!
//! Same rationale as the idempotent consumer: a Tower `Service<Exchange>` cannot
//! propagate `PipelineOutcome::Stopped` distinctly from `Ok(ex)`. By implementing
//! [`OutcomePipeline`] directly, a `Stopped` from the on-miss sub-pipeline flows
//! out with the Exchange intact and NO write-back occurs (ADR-0024, ADR-0025).
//!
//! # Contract C1 (ADR-0023)
//!
//! [`CacheRepository::get`] / [`CacheRepository::set`] surface backend failures
//! as `Err(CamelError)`. The segment propagates those as `PipelineOutcome::Failed`
//! — it NEVER treats a failed read as a miss.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;

use camel_api::body::Body;
use camel_api::cache::{CacheEntry, CacheRepository, ContentType};
use camel_api::{CamelError, Exchange, OutcomePipeline, OutcomeSegment, PipelineOutcome};
use camel_component_api::RuntimeObservability;

use crate::MessageIdExpression;

/// Outcome-aware Cache segment (Caching EIP).
///
/// Wraps a named [`CacheRepository`] and an on-miss sub-pipeline
/// ([`OutcomeSegment`]). On each exchange:
///
/// 1. Evaluate `key_expr`. `None` → not cacheable; forward directly to the
///    on-miss sub-pipeline (no lookup, no write-back).
/// 2. `repository.get(&key)`:
///    - `Err(e)` → `Failed(e)` (contract C1).
///    - `Ok(Some(entry))` → HIT: reconstruct `Body` from the entry, set it on
///      the exchange, return `Completed` (skip on-miss).
///    - `Ok(None)` → MISS: proceed to step 3.
/// 3. Run the on-miss sub-pipeline.
///    - `Stopped(ex)` / `Failed(e)` → propagate as-is (NO write-back).
///    - `Completed(ex)` → proceed to write-back.
/// 4. Write-back the resulting body (when it fits `max_entry_bytes`):
///    - materialized variants (`Bytes`/`Text`/`Json`/`Xml`) → serialize, store.
///    - `Stream` → materialize via [`Body::into_bytes`] (consumes the body,
///      replaces it with `Body::Bytes`); `StreamLimitExceeded` propagates.
///    - `Empty` / oversized body → pass through uncached, return `Completed`.
pub struct CacheService {
    repository: Arc<dyn CacheRepository>,
    /// Cached `repository.name()` for OTel span tagging (Task 3.3).
    repository_name: String,
    key_expr: MessageIdExpression,
    ttl: Option<Duration>,
    max_entry_bytes: usize,
    on_miss: OutcomeSegment,
    rt: Arc<dyn RuntimeObservability>,
}

impl CacheService {
    /// Build a new cache segment.
    ///
    /// `repository_name` is derived from `repository.name()` so OTel tags stay
    /// in sync with the resolved backend.
    pub fn new(
        repository: Arc<dyn CacheRepository>,
        key_expr: MessageIdExpression,
        ttl: Option<Duration>,
        max_entry_bytes: usize,
        on_miss: OutcomeSegment,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Self {
        let repository_name = repository.name().to_string();
        Self {
            repository,
            repository_name,
            key_expr,
            ttl,
            max_entry_bytes,
            on_miss,
            rt,
        }
    }

    /// The configured repository name (for OTel tagging).
    pub fn repository_name(&self) -> &str {
        &self.repository_name
    }
}

/// Shared write-back tail for materialized bodies.
///
/// Checks `max_entry_bytes`, builds a [`CacheEntry`], stores via
/// the repository, and returns `Completed(exchange)`. The exchange body
/// is not modified — it passes through as-is. On oversized body, logs a
/// debug! skip message and returns `Completed(exchange)` without storing.
/// On repository error, returns `Failed(e)`.
#[allow(clippy::too_many_arguments)]
async fn write_back(
    repository: &Arc<dyn CacheRepository>,
    repository_name: &str,
    max_entry_bytes: usize,
    ttl: Option<Duration>,
    exchange: Exchange,
    key: &str,
    serialized: Vec<u8>,
    content_type: ContentType,
) -> PipelineOutcome {
    if serialized.len() <= max_entry_bytes {
        let entry = CacheEntry {
            bytes: serialized,
            content_type,
            expires_at: None,
        };
        match repository.set(key, entry, ttl).await {
            Ok(()) => {}
            Err(e) => {
                if matches!(&e, CamelError::Config(msg) if msg.starts_with("cache: max_entries")) {
                    tracing::debug!(
                        repository = %repository_name,
                        key = %key,
                        "cache at capacity, skipping write-back"
                    ); // log-policy: g:cache:capacity-full-skip
                } else {
                    return PipelineOutcome::Failed(e);
                }
            }
        }
    } else {
        // log-policy: g:cache:oversized-skip
        tracing::debug!(
            repository = %repository_name,
            key = %key,
            len = serialized.len(),
            max = max_entry_bytes,
            "cache write-back skipped: body exceeds max_entry_bytes"
        );
    }
    PipelineOutcome::Completed(exchange)
}

impl Clone for CacheService {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            repository_name: self.repository_name.clone(),
            key_expr: Arc::clone(&self.key_expr),
            ttl: self.ttl,
            max_entry_bytes: self.max_entry_bytes,
            on_miss: self.on_miss.clone(),
            rt: Arc::clone(&self.rt),
        }
    }
}

impl OutcomePipeline for CacheService {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            // 1. Evaluate key. None → not cacheable, bypass straight to on_miss.
            let key = match (self.key_expr)(&exchange) {
                Some(k) => k,
                None => return self.on_miss.run(exchange).await,
            };

            // 2. Lookup (contract C1: propagate Err, never treat as miss).
            match self.repository.get(&key).await {
                Err(e) => return PipelineOutcome::Failed(e),
                Ok(Some(entry)) => {
                    // HIT: record metric, reconstruct body, skip on-miss sub-pipeline.
                    self.rt.metrics().record_counter(
                        "camel.cache.hits",
                        1.0_f64,
                        &[("repository", &self.repository_name)],
                    );
                    match reconstruct_body(&entry) {
                        Ok(body) => {
                            let mut exchange = exchange;
                            exchange.input.body = body;
                            return PipelineOutcome::Completed(exchange);
                        }
                        Err(e) => return PipelineOutcome::Failed(e),
                    }
                }
                Ok(None) => {
                    // MISS: record metric, fall through to on-miss sub-pipeline.
                    self.rt.metrics().record_counter(
                        "camel.cache.misses",
                        1.0_f64,
                        &[("repository", &self.repository_name)],
                    );
                }
            }

            // 3. Run the on-miss sub-pipeline.
            let mut exchange = match self.on_miss.run(exchange).await {
                PipelineOutcome::Stopped(ex) => return PipelineOutcome::Stopped(ex),
                PipelineOutcome::Failed(e) => return PipelineOutcome::Failed(e),
                PipelineOutcome::Completed(ex) => ex,
            };

            // 4. Write-back. Take the body out so the Stream arm can consume it.
            let body = std::mem::replace(&mut exchange.input.body, Body::Empty);
            match body {
                Body::Bytes(b) => {
                    let serialized = b.to_vec();
                    exchange.input.body = Body::Bytes(b);
                    write_back(
                        &self.repository,
                        &self.repository_name,
                        self.max_entry_bytes,
                        self.ttl,
                        exchange,
                        &key,
                        serialized,
                        ContentType::Bytes,
                    )
                    .await
                }
                Body::Text(s) => {
                    let serialized = s.as_bytes().to_vec();
                    exchange.input.body = Body::Text(s);
                    write_back(
                        &self.repository,
                        &self.repository_name,
                        self.max_entry_bytes,
                        self.ttl,
                        exchange,
                        &key,
                        serialized,
                        ContentType::Text,
                    )
                    .await
                }
                Body::Json(v) => {
                    let serialized = match serde_json::to_vec(&v) {
                        Ok(b) => b,
                        Err(e) => {
                            exchange.input.body = Body::Json(v);
                            return PipelineOutcome::Failed(CamelError::TypeConversionFailed(
                                e.to_string(),
                            ));
                        }
                    };
                    exchange.input.body = Body::Json(v);
                    write_back(
                        &self.repository,
                        &self.repository_name,
                        self.max_entry_bytes,
                        self.ttl,
                        exchange,
                        &key,
                        serialized,
                        ContentType::Json,
                    )
                    .await
                }
                Body::Xml(s) => {
                    let serialized = s.as_bytes().to_vec();
                    exchange.input.body = Body::Xml(s);
                    write_back(
                        &self.repository,
                        &self.repository_name,
                        self.max_entry_bytes,
                        self.ttl,
                        exchange,
                        &key,
                        serialized,
                        ContentType::Xml,
                    )
                    .await
                }
                Body::Stream(stream_body) => {
                    // Materialize (consumes the stream). StreamLimitExceeded propagates.
                    let materialized = match Body::Stream(stream_body)
                        .into_bytes(self.max_entry_bytes)
                        .await
                    {
                        Ok(b) => b,
                        Err(e) => return PipelineOutcome::Failed(e),
                    };
                    // into_bytes already enforced max_entry_bytes, so it fits by construction.
                    let entry = CacheEntry {
                        bytes: materialized.to_vec(),
                        content_type: ContentType::Bytes,
                        expires_at: None,
                    };
                    if let Err(e) = self.repository.set(&key, entry, self.ttl).await {
                        // Degrade capacity-exceeded to uncached — same policy as write_back.
                        if matches!(&e, CamelError::Config(msg) if msg.starts_with("cache: max_entries"))
                        {
                            tracing::debug!(
                                repository = %self.repository_name,
                                key = %key,
                                "cache at capacity, skipping write-back for stream"
                            ); // log-policy: g:cache:capacity-full-skip
                            exchange.input.body = Body::Bytes(materialized);
                            return PipelineOutcome::Completed(exchange);
                        }
                        exchange.input.body = Body::Bytes(materialized);
                        return PipelineOutcome::Failed(e);
                    }
                    exchange.input.body = Body::Bytes(materialized);
                    PipelineOutcome::Completed(exchange)
                }
                _ => {
                    // Empty (or any future variant): pass through uncached.
                    exchange.input.body = body;
                    PipelineOutcome::Completed(exchange)
                }
            }
        })
    }
}

/// Reconstruct a [`Body`] from a stored [`CacheEntry`].
///
/// Maps each [`ContentType`] back to the matching `Body` variant, decoding
/// UTF-8 / JSON failures into `CamelError::TypeConversionFailed`.
fn reconstruct_body(entry: &CacheEntry) -> Result<Body, CamelError> {
    match entry.content_type {
        ContentType::Bytes => Ok(Body::Bytes(Bytes::from(entry.bytes.clone()))),
        ContentType::Text => {
            let s = String::from_utf8(entry.bytes.clone()).map_err(|e| {
                CamelError::TypeConversionFailed(format!("cached text is not valid UTF-8: {e}"))
            })?;
            Ok(Body::Text(s))
        }
        ContentType::Json => {
            let v = serde_json::from_slice(&entry.bytes).map_err(|e| {
                CamelError::TypeConversionFailed(format!("cached bytes are not valid JSON: {e}"))
            })?;
            Ok(Body::Json(v))
        }
        ContentType::Xml => {
            let s = String::from_utf8(entry.bytes.clone()).map_err(|e| {
                CamelError::TypeConversionFailed(format!("cached xml is not valid UTF-8: {e}"))
            })?;
            Ok(Body::Xml(s))
        }
    }
}

// ===========================================================================
// CacheInvalidateService — invalidate a single cache entry
// ===========================================================================

/// Outcome-aware segment that invalidates a single cache entry.
///
/// Evaluates `key_expr`:
/// - `None` → `Completed(exchange)` (nothing to invalidate).
/// - `Some(key)` → `repository.invalidate(&key).await`.
///   - `Err(e)` → `Failed(e)`.
///   - `Ok(())` → `Completed(exchange)`.
pub struct CacheInvalidateService {
    repository: Arc<dyn CacheRepository>,
    key_expr: MessageIdExpression,
}

impl CacheInvalidateService {
    pub fn new(repository: Arc<dyn CacheRepository>, key_expr: MessageIdExpression) -> Self {
        Self {
            repository,
            key_expr,
        }
    }
}

impl Clone for CacheInvalidateService {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            key_expr: Arc::clone(&self.key_expr),
        }
    }
}

impl OutcomePipeline for CacheInvalidateService {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            let key = match (self.key_expr)(&exchange) {
                Some(k) => k,
                None => return PipelineOutcome::Completed(exchange),
            };
            match self.repository.invalidate(&key).await {
                Err(e) => PipelineOutcome::Failed(e),
                Ok(()) => PipelineOutcome::Completed(exchange),
            }
        })
    }
}

// ===========================================================================
// CachePeekStaleService — serve a stale entry after expiry
// ===========================================================================

/// Exchange property set to `true` when a `cache_peek_stale` HIT occurred.
pub const CAMEL_CACHE_PEEK_HIT: &str = "CamelCachePeekHit";
/// Exchange property set to `true` when the served entry was stale (post-expiry).
pub const CAMEL_CACHE_PEEK_STALE: &str = "CamelCachePeekStale";

/// On-miss policy for [`CachePeekStaleService`].
///
/// - [`Stop`](PeekStaleMissPolicy::Stop) (default) preserves the
///   `CircuitBreaker.fallback` absence-Stops contract.
/// - [`Continue`](PeekStaleMissPolicy::Continue) leaves the body untouched on
///   MISS so `choice` can branch on [`CAMEL_CACHE_PEEK_HIT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeekStaleMissPolicy {
    /// MISS Stops the branch (no stale available — `CircuitBreaker.fallback`).
    Stop,
    /// MISS continues with the body unchanged.
    Continue,
}

impl PeekStaleMissPolicy {
    /// Parses the canonical/DSL `cache_peek_stale.on_miss` knob:
    /// absent or `"stop"` → [`Stop`](Self::Stop), `"continue"` →
    /// [`Continue`](Self::Continue). Any other value fails closed naming
    /// the step.
    pub fn parse_on_miss(raw: Option<&str>) -> Result<Self, CamelError> {
        match raw {
            None | Some("stop") => Ok(Self::Stop),
            Some("continue") => Ok(Self::Continue),
            Some(other) => Err(CamelError::Config(format!(
                "cache_peek_stale: invalid on_miss '{other}'; must be \"stop\" or \"continue\""
            ))),
        }
    }
}

/// Outcome-aware segment that serves a stale (post-expiry) cache entry.
///
/// Evaluates `key_expr`:
/// - `None` → `Stopped(exchange)` with a `debug` log (an anomalous key
///   resolution is fail-closed, not a miss).
/// - `Some(key)` → `repository.peek_stale(&key).await`.
///   - `Err(e)` → `Failed(e)`.
///   - `Ok(Some(entry))` → reconstruct body from entry, set
///     `CamelCachePeekHit=true` and `CamelCachePeekStale` (true when the
///     entry's `expires_at` has elapsed at evaluation time; false when absent
///     or not elapsed), return `Completed(exchange)`.
///   - `Ok(None)` → MISS (absence), governed by [`PeekStaleMissPolicy`]:
///     - `Stop` (default): set `CamelCachePeekHit=false` and
///       `CamelCachePeekStale=false`, log at `debug`, return `Stopped(exchange)`
///       (absence in `CircuitBreaker.fallback` means "no stale available").
///     - `Continue`: set `CamelCachePeekHit=false` and
///       `CamelCachePeekStale=false`, leave the body unchanged, return
///       `Completed(exchange)` so `choice` can branch on `CamelCachePeekHit`.
pub struct CachePeekStaleService {
    repository: Arc<dyn CacheRepository>,
    key_expr: MessageIdExpression,
    miss_policy: PeekStaleMissPolicy,
}

impl CachePeekStaleService {
    pub fn new(
        repository: Arc<dyn CacheRepository>,
        key_expr: MessageIdExpression,
        miss_policy: PeekStaleMissPolicy,
    ) -> Self {
        Self {
            repository,
            key_expr,
            miss_policy,
        }
    }
}

impl Clone for CachePeekStaleService {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            key_expr: Arc::clone(&self.key_expr),
            miss_policy: self.miss_policy,
        }
    }
}

/// Write the peek-result exchange properties under [`CAMEL_CACHE_PEEK_HIT`] and
/// [`CAMEL_CACHE_PEEK_STALE`] as `serde_json::Value::Bool` values.
fn set_peek_properties(exchange: &mut Exchange, hit: bool, stale: bool) {
    exchange.set_property(CAMEL_CACHE_PEEK_HIT, serde_json::Value::Bool(hit));
    exchange.set_property(CAMEL_CACHE_PEEK_STALE, serde_json::Value::Bool(stale));
}

impl OutcomePipeline for CachePeekStaleService {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            let key = match (self.key_expr)(&exchange) {
                Some(k) => k,
                None => {
                    tracing::debug!(
                        step = "cache_peek_stale",
                        repository = %self.repository.name(),
                        "key expression resolved to None; stopping branch"
                    );
                    return PipelineOutcome::Stopped(exchange);
                }
            };
            match self.repository.peek_stale(&key).await {
                Err(e) => PipelineOutcome::Failed(e),
                Ok(Some(entry)) => {
                    // Staleness read before body reconstruction; reconstruct_body borrows the entry, so ordering is stylistic.
                    let stale = entry
                        .expires_at
                        .map(|t| t <= SystemTime::now())
                        .unwrap_or(false);
                    match reconstruct_body(&entry) {
                        Ok(body) => {
                            let mut exchange = exchange;
                            exchange.input.body = body;
                            set_peek_properties(&mut exchange, true, stale);
                            PipelineOutcome::Completed(exchange)
                        }
                        Err(e) => PipelineOutcome::Failed(e),
                    }
                }
                Ok(None) => match self.miss_policy {
                    PeekStaleMissPolicy::Stop => {
                        let mut exchange = exchange;
                        set_peek_properties(&mut exchange, false, false);
                        tracing::debug!(
                            step = "cache_peek_stale",
                            repository = %self.repository.name(),
                            "peek miss; stopping branch per on_miss=stop"
                        );
                        PipelineOutcome::Stopped(exchange)
                    }
                    PeekStaleMissPolicy::Continue => {
                        let mut exchange = exchange;
                        set_peek_properties(&mut exchange, false, false);
                        PipelineOutcome::Completed(exchange)
                    }
                },
            }
        })
    }
}

// ===========================================================================
// Test utilities
// ===========================================================================

#[cfg(test)]
mod test_utils {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use tokio::sync::Mutex;

    /// In-memory mock [`CacheRepository`] for cache segment tests. Allows tests
    /// to pre-seed entries, force `get`/`set` failures, and inspect the last
    /// `set` call (entry + TTL).
    #[derive(Debug, Default)]
    pub struct MockCacheRepository {
        name: String,
        entries: Arc<Mutex<HashMap<String, CacheEntry>>>,
        get_should_fail: Arc<AtomicBool>,
        set_should_fail: Arc<AtomicBool>,
        set_call_count: Arc<AtomicU32>,
        last_set_ttl: Arc<Mutex<Option<Duration>>>,
        invalidate_call_count: Arc<AtomicU32>,
        last_invalidate_key: Arc<Mutex<Option<String>>>,
    }

    impl MockCacheRepository {
        pub fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                ..Default::default()
            }
        }

        pub fn invalidate_call_count(&self) -> u32 {
            self.invalidate_call_count.load(Ordering::SeqCst)
        }

        pub async fn last_invalidate_key(&self) -> Option<String> {
            self.last_invalidate_key.lock().await.clone()
        }

        /// Pre-seed a key so `get` returns a HIT.
        pub async fn seed(&self, key: &str, entry: CacheEntry) {
            self.entries.lock().await.insert(key.to_string(), entry);
        }

        pub fn set_get_should_fail(&self, v: bool) {
            self.get_should_fail.store(v, Ordering::SeqCst);
        }

        pub fn set_set_should_fail(&self, v: bool) {
            self.set_should_fail.store(v, Ordering::SeqCst);
        }

        pub fn set_call_count(&self) -> u32 {
            self.set_call_count.load(Ordering::SeqCst)
        }

        /// The TTL passed to the most recent `set` call.
        pub async fn last_set_ttl(&self) -> Option<Duration> {
            *self.last_set_ttl.lock().await
        }

        /// Inspect the entry currently stored for `key` (if any).
        pub async fn stored_entry(&self, key: &str) -> Option<CacheEntry> {
            self.entries.lock().await.get(key).cloned()
        }
    }

    #[async_trait]
    impl CacheRepository for MockCacheRepository {
        fn name(&self) -> &str {
            &self.name
        }

        async fn get(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
            if self.get_should_fail.load(Ordering::SeqCst) {
                return Err(CamelError::ProcessorError("synthetic get failure".into()));
            }
            Ok(self.entries.lock().await.get(key).cloned())
        }

        async fn set(
            &self,
            key: &str,
            value: CacheEntry,
            ttl: Option<Duration>,
        ) -> Result<(), CamelError> {
            self.set_call_count.fetch_add(1, Ordering::SeqCst);
            *self.last_set_ttl.lock().await = ttl;
            if self.set_should_fail.load(Ordering::SeqCst) {
                return Err(CamelError::ProcessorError("synthetic set failure".into()));
            }
            self.entries.lock().await.insert(key.to_string(), value);
            Ok(())
        }

        async fn peek_stale(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
            self.get(key).await
        }

        async fn invalidate(&self, key: &str) -> Result<(), CamelError> {
            self.invalidate_call_count.fetch_add(1, Ordering::SeqCst);
            *self.last_invalidate_key.lock().await = Some(key.to_string());
            self.entries.lock().await.remove(key);
            Ok(())
        }

        async fn clear(&self) -> Result<(), CamelError> {
            self.entries.lock().await.clear();
            Ok(())
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::test_utils::MockCacheRepository;
    use super::*;
    use camel_api::body::{StreamBody, StreamMetadata};
    use camel_api::metrics::NoOpMetrics;
    use camel_api::{Message, Value};
    use camel_component_api::health_registry::NoOpHealthCheckRegistry;
    use futures::stream;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::SystemTime;

    #[test]
    fn parse_on_miss_maps_absent_stop_and_continue() {
        assert_eq!(
            PeekStaleMissPolicy::parse_on_miss(None).unwrap(),
            PeekStaleMissPolicy::Stop
        );
        assert_eq!(
            PeekStaleMissPolicy::parse_on_miss(Some("stop")).unwrap(),
            PeekStaleMissPolicy::Stop
        );
        assert_eq!(
            PeekStaleMissPolicy::parse_on_miss(Some("continue")).unwrap(),
            PeekStaleMissPolicy::Continue
        );
    }

    #[test]
    fn parse_on_miss_rejects_unknown_value_naming_the_step() {
        let err = PeekStaleMissPolicy::parse_on_miss(Some("explode")).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("cache_peek_stale"), "got: {msg}");
        assert!(msg.contains("explode"), "got: {msg}");
    }

    /// Minimal no-op RuntimeObservability for tests that don't need OTel.
    #[derive(Clone)]
    struct NoopRt;

    impl camel_component_api::HealthCheckRegistry for NoopRt {
        fn force_unhealthy_for_route(&self, _: &str, _: &str, _: &str) {}
    }

    impl RuntimeObservability for NoopRt {
        fn metrics(&self) -> Arc<dyn camel_api::metrics::MetricsCollector> {
            Arc::new(NoOpMetrics)
        }
        fn health(&self) -> Arc<dyn camel_component_api::health_registry::HealthCheckRegistry> {
            Arc::new(NoOpHealthCheckRegistry)
        }
    }

    fn noop_rt() -> Arc<dyn RuntimeObservability> {
        Arc::new(NoopRt)
    }

    // ── Scripted on-miss sub-pipeline ──

    #[derive(Clone)]
    enum ScriptedOutcome {
        Complete,
        Stop,
        Fail(CamelError),
    }

    /// Test sub-pipeline: optionally replaces the body, then returns a
    /// scripted outcome. Records whether it was invoked.
    struct ScriptedOnMiss {
        body: Option<Body>,
        outcome: ScriptedOutcome,
        invoked: Arc<AtomicBool>,
    }

    impl OutcomePipeline for ScriptedOnMiss {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            // clone_box is required by the trait but unused by these tests.
            unreachable!("clone_box not used in cache_eip tests")
        }

        fn run<'a>(
            &'a mut self,
            mut exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            self.invoked.store(true, Ordering::SeqCst);
            let body = self.body.take();
            let outcome = self.outcome.clone();
            Box::pin(async move {
                if let Some(b) = body {
                    exchange.input.body = b;
                }
                match outcome {
                    ScriptedOutcome::Complete => PipelineOutcome::Completed(exchange),
                    ScriptedOutcome::Stop => PipelineOutcome::Stopped(exchange),
                    ScriptedOutcome::Fail(e) => PipelineOutcome::Failed(e),
                }
            })
        }
    }

    // ── Builders ──

    fn fixed_key() -> MessageIdExpression {
        Arc::new(|_| Some("cache-key".to_string()))
    }

    fn none_key() -> MessageIdExpression {
        Arc::new(|_| None)
    }

    /// Build a CacheService whose on-miss sets `body` and returns `outcome`.
    fn build_service(
        repo: Arc<MockCacheRepository>,
        key_expr: MessageIdExpression,
        max_entry_bytes: usize,
        body: Option<Body>,
        outcome: ScriptedOutcome,
        ttl: Option<Duration>,
        rt: Arc<dyn RuntimeObservability>,
    ) -> (CacheService, Arc<AtomicBool>) {
        let invoked = Arc::new(AtomicBool::new(false));
        let on_miss = OutcomeSegment::new(Box::new(ScriptedOnMiss {
            body,
            outcome,
            invoked: invoked.clone(),
        }));
        let svc = CacheService::new(repo, key_expr, ttl, max_entry_bytes, on_miss, rt);
        (svc, invoked)
    }

    fn exchange() -> Exchange {
        let mut ex = Exchange::new(Message::new(""));
        ex.input.set_header("ignored", Value::String("v".into()));
        ex
    }

    fn stream_body(data: &'static [u8]) -> Body {
        let chunks: Vec<Result<Bytes, CamelError>> = vec![Ok(Bytes::from_static(data))];
        let s = stream::iter(chunks);
        Body::Stream(StreamBody {
            stream: Arc::new(tokio::sync::Mutex::new(Some(Box::pin(s)))),
            metadata: StreamMetadata::default(),
        })
    }

    fn stub_error(msg: &str) -> CamelError {
        CamelError::ProcessorError(msg.into())
    }

    // ── Test 1: cache HIT short-circuits, on_miss NOT executed ──

    #[tokio::test]
    async fn cache_hit_short_circuits_on_miss() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.seed(
            "cache-key",
            CacheEntry {
                bytes: b"cached-payload".to_vec(),
                content_type: ContentType::Bytes,
                expires_at: None,
            },
        )
        .await;
        let (mut svc, on_miss_invoked) = build_service(
            repo,
            fixed_key(),
            1024,
            Some(Body::Bytes(Bytes::from_static(b"unreached"))),
            ScriptedOutcome::Complete,
            None,
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert_eq!(
            ex.input.body,
            Body::Bytes(Bytes::from_static(b"cached-payload"))
        );
        assert!(
            !on_miss_invoked.load(Ordering::SeqCst),
            "on_miss must NOT run on a cache HIT"
        );
    }

    // ── Test 2: cache MISS runs on_miss, writes back, continues ──

    #[tokio::test]
    async fn cache_miss_runs_on_miss_sets_continues() {
        let ttl = Duration::from_secs(30);
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let (mut svc, on_miss_invoked) = build_service(
            repo.clone(),
            fixed_key(),
            1024,
            Some(Body::Bytes(Bytes::from_static(b"x"))),
            ScriptedOutcome::Complete,
            Some(ttl),
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert!(on_miss_invoked.load(Ordering::SeqCst));
        assert_eq!(ex.input.body, Body::Bytes(Bytes::from_static(b"x")));
        assert_eq!(repo.set_call_count(), 1, "set must be called once on miss");
        let stored = repo
            .stored_entry("cache-key")
            .await
            .expect("entry must be stored");
        assert_eq!(stored.bytes, b"x");
        assert_eq!(stored.content_type, ContentType::Bytes);
        assert_eq!(repo.last_set_ttl().await, Some(ttl));
    }

    // ── Test 3: oversized materialized body skips write-back ──

    #[tokio::test]
    async fn cache_miss_oversized_materialized_body_skips_writeback() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        // max_entry_bytes = 4; on_miss produces 9 bytes.
        let (mut svc, _invoked) = build_service(
            repo.clone(),
            fixed_key(),
            4,
            Some(Body::Bytes(Bytes::from_static(b"oversized"))),
            ScriptedOutcome::Complete,
            None,
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        // Body passes through unchanged.
        assert_eq!(ex.input.body, Body::Bytes(Bytes::from_static(b"oversized")));
        assert_eq!(
            repo.set_call_count(),
            0,
            "set must NOT be called for oversized body"
        );
        assert!(repo.stored_entry("cache-key").await.is_none());
    }

    // ── Test 4: oversized Stream propagates StreamLimitExceeded ──

    #[tokio::test]
    async fn cache_miss_oversized_stream_propagates_err() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let (mut svc, _invoked) = build_service(
            repo.clone(),
            fixed_key(),
            4,
            Some(stream_body(b"way-too-big-stream")),
            ScriptedOutcome::Complete,
            None,
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        match outcome {
            PipelineOutcome::Failed(CamelError::StreamLimitExceeded(n)) => {
                assert_eq!(n, 4);
            }
            other => panic!("expected Failed(StreamLimitExceeded(4)), got {other:?}"),
        }
        assert_eq!(
            repo.set_call_count(),
            0,
            "set must NOT be called when stream exceeds limit"
        );
    }

    // ── Test 5: on_miss Stopped propagates without write-back ──

    #[tokio::test]
    async fn cache_on_miss_stopped_propagates_without_writeback() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let (mut svc, _invoked) = build_service(
            repo.clone(),
            fixed_key(),
            1024,
            None,
            ScriptedOutcome::Stop,
            None,
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        assert!(
            matches!(outcome, PipelineOutcome::Stopped(_)),
            "Stopped from on_miss MUST propagate as Stopped"
        );
        assert_eq!(
            repo.set_call_count(),
            0,
            "set must NOT be called when on_miss Stops"
        );
    }

    // ── Test 6: on_miss Err propagates without write-back ──

    #[tokio::test]
    async fn cache_on_miss_err_propagates_without_writeback() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let (mut svc, _invoked) = build_service(
            repo.clone(),
            fixed_key(),
            1024,
            None,
            ScriptedOutcome::Fail(stub_error("on-miss blew up")),
            None,
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        match outcome {
            PipelineOutcome::Failed(e) => {
                assert!(e.to_string().contains("on-miss blew up"), "got: {e}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(
            repo.set_call_count(),
            0,
            "set must NOT be called when on_miss fails"
        );
    }

    // ── Test 7: repository get Err propagates ──

    #[tokio::test]
    async fn cache_repository_get_err_propagates() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.set_get_should_fail(true);
        let (mut svc, on_miss_invoked) = build_service(
            repo,
            fixed_key(),
            1024,
            Some(Body::Bytes(Bytes::from_static(b"x"))),
            ScriptedOutcome::Complete,
            None,
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        match outcome {
            PipelineOutcome::Failed(e) => {
                assert!(e.to_string().contains("synthetic get failure"), "got: {e}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(
            !on_miss_invoked.load(Ordering::SeqCst),
            "on_miss must NOT run when get fails"
        );
    }

    // ── Test 8: repository set Err propagates ──

    #[tokio::test]
    async fn cache_repository_set_err_propagates() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.set_set_should_fail(true);
        let (mut svc, _invoked) = build_service(
            repo.clone(),
            fixed_key(),
            1024,
            Some(Body::Bytes(Bytes::from_static(b"x"))),
            ScriptedOutcome::Complete,
            None,
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        match outcome {
            PipelineOutcome::Failed(e) => {
                assert!(e.to_string().contains("synthetic set failure"), "got: {e}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(repo.set_call_count(), 1, "set was attempted (and failed)");
    }

    // ── Test 9: None key bypasses to on_miss, no set ──

    #[tokio::test]
    async fn cache_none_key_bypasses_to_on_miss() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let (mut svc, on_miss_invoked) = build_service(
            repo.clone(),
            none_key(),
            1024,
            Some(Body::Bytes(Bytes::from_static(b"x"))),
            ScriptedOutcome::Complete,
            None,
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert!(
            on_miss_invoked.load(Ordering::SeqCst),
            "on_miss MUST run when key is None"
        );
        assert_eq!(ex.input.body, Body::Bytes(Bytes::from_static(b"x")));
        assert_eq!(
            repo.set_call_count(),
            0,
            "set must NOT be called when key_expr returns None"
        );
    }

    // ── Extra: HIT reconstruction for each ContentType ──

    #[tokio::test]
    async fn cache_content_type_reconstruction() {
        async fn run_case(entry: CacheEntry, expected: Body) {
            let repo = Arc::new(MockCacheRepository::new("mock"));
            repo.seed("cache-key", entry).await;
            let (mut svc, on_miss_invoked) = build_service(
                repo,
                fixed_key(),
                1024,
                Some(Body::Bytes(Bytes::from_static(b"unreached"))),
                ScriptedOutcome::Complete,
                None,
                noop_rt(),
            );
            let outcome = svc.run(exchange()).await;
            let ex = match outcome {
                PipelineOutcome::Completed(ex) => ex,
                other => panic!("expected Completed, got {other:?}"),
            };
            assert_eq!(ex.input.body, expected);
            assert!(!on_miss_invoked.load(Ordering::SeqCst));
        }

        run_case(
            CacheEntry {
                bytes: b"raw".to_vec(),
                content_type: ContentType::Bytes,
                expires_at: None,
            },
            Body::Bytes(Bytes::from_static(b"raw")),
        )
        .await;
        run_case(
            CacheEntry {
                bytes: b"hi".to_vec(),
                content_type: ContentType::Text,
                expires_at: None,
            },
            Body::Text("hi".into()),
        )
        .await;
        run_case(
            CacheEntry {
                bytes: br#"{"k":1}"#.to_vec(),
                content_type: ContentType::Json,
                expires_at: None,
            },
            Body::Json(serde_json::json!({"k": 1})),
        )
        .await;
        run_case(
            CacheEntry {
                bytes: b"<a/>".to_vec(),
                content_type: ContentType::Xml,
                expires_at: None,
            },
            Body::Xml("<a/>".into()),
        )
        .await;
    }

    // ── Extra: Stream body write-back materializes into Body::Bytes ──

    #[tokio::test]
    async fn cache_miss_stream_body_is_materialized_and_cached() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let (mut svc, _invoked) = build_service(
            repo.clone(),
            fixed_key(),
            1024,
            Some(stream_body(b"chunky")),
            ScriptedOutcome::Complete,
            None,
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        // Stream is replaced by materialized Bytes.
        assert_eq!(ex.input.body, Body::Bytes(Bytes::from_static(b"chunky")));
        assert_eq!(repo.set_call_count(), 1);
        let stored = repo.stored_entry("cache-key").await.expect("stored");
        assert_eq!(stored.bytes, b"chunky");
        assert_eq!(stored.content_type, ContentType::Bytes);
    }

    // ── CachePeekStaleService tests ──

    #[tokio::test]
    async fn cache_peek_stale_serves_post_expiry_entry() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.seed(
            "cache-key",
            CacheEntry {
                bytes: b"stale-payload".to_vec(),
                content_type: ContentType::Text,
                expires_at: None,
            },
        )
        .await;
        let mut svc = CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop);

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert_eq!(ex.input.body, Body::Text("stale-payload".into()));
    }

    #[tokio::test]
    async fn cache_peek_stale_on_absence_stops_branch() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc = CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop);

        let outcome = svc.run(exchange()).await;

        assert!(
            matches!(outcome, PipelineOutcome::Stopped(_)),
            "expected Stopped when no stale entry, got {outcome:?}"
        );
    }

    #[tokio::test]
    async fn cache_peek_stale_none_key_stops() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc = CachePeekStaleService::new(repo, none_key(), PeekStaleMissPolicy::Stop);

        let outcome = svc.run(exchange()).await;

        assert!(
            matches!(outcome, PipelineOutcome::Stopped(_)),
            "expected Stopped when key_expr returns None, got {outcome:?}"
        );
    }

    #[tokio::test]
    async fn peek_stale_miss_stop_sets_properties_and_stops() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc = CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop);

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Stopped(ex) => ex,
            other => panic!("expected Stopped, got {other:?}"),
        };
        assert_eq!(ex.property(CAMEL_CACHE_PEEK_HIT), Some(&Value::Bool(false)));
        assert_eq!(
            ex.property(CAMEL_CACHE_PEEK_STALE),
            Some(&Value::Bool(false))
        );
    }

    #[tokio::test]
    async fn peek_stale_miss_continue_completes_with_body_untouched() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc = CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Continue);

        let mut ex = exchange();
        ex.input.body = Body::Text("orig".into());

        let outcome = svc.run(ex).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert_eq!(ex.input.body, Body::Text("orig".into()));
        assert_eq!(ex.property(CAMEL_CACHE_PEEK_HIT), Some(&Value::Bool(false)));
        assert_eq!(
            ex.property(CAMEL_CACHE_PEEK_STALE),
            Some(&Value::Bool(false))
        );
    }

    #[tokio::test]
    async fn peek_stale_hit_sets_hit_and_stale_properties() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.seed(
            "cache-key",
            CacheEntry {
                bytes: b"stale-payload".to_vec(),
                content_type: ContentType::Bytes,
                expires_at: Some(SystemTime::now() - Duration::from_millis(1)),
            },
        )
        .await;
        let mut svc = CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop);

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert_eq!(
            ex.input.body,
            Body::Bytes(Bytes::from_static(b"stale-payload"))
        );
        assert_eq!(ex.property(CAMEL_CACHE_PEEK_HIT), Some(&Value::Bool(true)));
        assert_eq!(
            ex.property(CAMEL_CACHE_PEEK_STALE),
            Some(&Value::Bool(true))
        );
    }

    #[tokio::test]
    async fn peek_stale_hit_fresh_sets_stale_false() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.seed(
            "cache-key",
            CacheEntry {
                bytes: b"fresh-payload".to_vec(),
                content_type: ContentType::Bytes,
                expires_at: Some(SystemTime::now() + Duration::from_secs(3600)),
            },
        )
        .await;
        let mut svc = CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop);

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert_eq!(ex.property(CAMEL_CACHE_PEEK_HIT), Some(&Value::Bool(true)));
        assert_eq!(
            ex.property(CAMEL_CACHE_PEEK_STALE),
            Some(&Value::Bool(false))
        );
    }

    // --- Tracing capture helper for debug-log assertions ---

    /// `MakeWriter` that appends formatted events to a shared `Vec<u8>` sink.
    #[derive(Clone)]
    struct CapturingWriter {
        sink: Arc<Mutex<Vec<u8>>>,
    }

    impl std::io::Write for CapturingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.sink.lock().unwrap().extend_from_slice(buf); // allow-unwrap: test-only
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturingWriter {
        type Writer = CapturingWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn debug_sink() -> (Arc<Mutex<Vec<u8>>>, impl tracing::Subscriber) {
        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let writer = CapturingWriter {
            sink: Arc::clone(&sink),
        };
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_ansi(false)
            .with_max_level(tracing_subscriber::filter::LevelFilter::DEBUG)
            .finish();
        (sink, subscriber)
    }

    #[tokio::test]
    async fn peek_stale_miss_stop_emits_debug_log() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc = CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop);

        let (sink, subscriber) = debug_sink();
        let _guard = tracing::subscriber::set_default(subscriber);
        // Parallel tests race tracing's per-callsite interest cache against
        // this thread-local subscriber; force a rebuild so the callsites
        // below re-evaluate against it (bd rc-u9hs).
        tracing::callsite::rebuild_interest_cache();
        let outcome = svc.run(exchange()).await;
        drop(_guard);

        assert!(matches!(outcome, PipelineOutcome::Stopped(_)));

        let captured = String::from_utf8(sink.lock().unwrap().clone()).unwrap(); // allow-unwrap: test-only
        let miss_records: Vec<&str> = captured
            .lines()
            .filter(|l| l.contains("peek miss"))
            .collect();
        assert_eq!(
            miss_records.len(),
            1,
            "expected exactly one DEBUG record containing \"peek miss\"; got: {captured}"
        );
        assert!(
            miss_records[0].contains("DEBUG"),
            "expected DEBUG level record; got: {captured}"
        );
        assert!(
            miss_records[0].contains("repository=mock"),
            "expected repository field in record; got: {captured}"
        );
        assert!(
            miss_records[0].contains("step=\"cache_peek_stale\""),
            "expected step field in record; got: {captured}"
        );
    }

    #[tokio::test]
    async fn peek_stale_key_none_stops_with_debug_log() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc = CachePeekStaleService::new(repo, none_key(), PeekStaleMissPolicy::Stop);

        let (sink, subscriber) = debug_sink();
        let _guard = tracing::subscriber::set_default(subscriber);
        // Parallel tests race tracing's per-callsite interest cache against
        // this thread-local subscriber; force a rebuild so the callsites
        // below re-evaluate against it (bd rc-u9hs).
        tracing::callsite::rebuild_interest_cache();
        let outcome = svc.run(exchange()).await;
        drop(_guard);

        assert!(matches!(outcome, PipelineOutcome::Stopped(_)));

        let captured = String::from_utf8(sink.lock().unwrap().clone()).unwrap(); // allow-unwrap: test-only
        let none_records: Vec<&str> = captured
            .lines()
            .filter(|l| l.contains("resolved to None"))
            .collect();
        assert_eq!(
            none_records.len(),
            1,
            "expected exactly one DEBUG record containing \"resolved to None\"; got: {captured}"
        );
        assert!(
            none_records[0].contains("DEBUG"),
            "expected DEBUG level record; got: {captured}"
        );
        assert!(
            none_records[0].contains("repository=mock"),
            "expected repository field in record; got: {captured}"
        );
        assert!(
            none_records[0].contains("step=\"cache_peek_stale\""),
            "expected step field in record; got: {captured}"
        );
    }

    // ── CacheInvalidateService tests ──

    #[tokio::test]
    async fn cache_invalidate_calls_repository_invalidate() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.seed(
            "cache-key",
            CacheEntry {
                bytes: b"to-go".to_vec(),
                content_type: ContentType::Bytes,
                expires_at: None,
            },
        )
        .await;
        let mut svc = CacheInvalidateService::new(repo.clone(), fixed_key());

        let outcome = svc.run(exchange()).await;

        let _ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert_eq!(
            repo.invalidate_call_count(),
            1,
            "invalidate must be called once"
        );
        assert_eq!(
            repo.last_invalidate_key().await,
            Some("cache-key".to_string()),
            "invalidate must be called with the correct key"
        );
        assert!(
            repo.stored_entry("cache-key").await.is_none(),
            "entry must be removed after invalidation"
        );
    }

    #[tokio::test]
    async fn cache_invalidate_none_key_completes() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc = CacheInvalidateService::new(repo.clone(), none_key());

        let outcome = svc.run(exchange()).await;

        let _ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert_eq!(
            repo.invalidate_call_count(),
            0,
            "invalidate must NOT be called when key_expr returns None"
        );
    }

    // ── OTel metrics tests ──

    /// Records every `record_counter` call for test assertions.
    type CounterRecording = Vec<(String, f64, Vec<(String, String)>)>;

    #[derive(Clone)]
    struct RecordingMetricsCollector {
        counters: Arc<Mutex<CounterRecording>>,
    }

    impl RecordingMetricsCollector {
        fn new() -> Self {
            Self {
                counters: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl camel_api::metrics::MetricsCollector for RecordingMetricsCollector {
        fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {}
        fn increment_errors(&self, _route_id: &str, _error_type: &str) {}
        fn increment_exchanges(&self, _route_id: &str) {}
        fn set_queue_depth(&self, _route_id: &str, _depth: usize) {}
        fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}
        fn record_counter(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
            self.counters.lock().unwrap().push((
                name.to_string(),
                value,
                labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ));
        }
    }

    #[derive(Clone)]
    struct TestOtelmRt {
        collector: Arc<RecordingMetricsCollector>,
    }

    impl camel_component_api::health_registry::HealthCheckRegistry for TestOtelmRt {
        fn force_unhealthy_for_route(&self, _: &str, _: &str, _: &str) {}
    }

    impl RuntimeObservability for TestOtelmRt {
        fn metrics(&self) -> Arc<dyn camel_api::metrics::MetricsCollector> {
            self.collector.clone()
        }
        fn health(&self) -> Arc<dyn camel_component_api::health_registry::HealthCheckRegistry> {
            Arc::new(NoOpHealthCheckRegistry)
        }
    }

    #[tokio::test]
    async fn cache_step_hit_increments_otel_counter() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.seed(
            "cache-key",
            CacheEntry {
                bytes: b"cached".to_vec(),
                content_type: ContentType::Bytes,
                expires_at: None,
            },
        )
        .await;
        let collector = RecordingMetricsCollector::new();
        let counters = collector.counters.clone();
        let rt = Arc::new(TestOtelmRt {
            collector: Arc::new(collector),
        });
        let (mut svc, _invoked) = build_service(
            repo,
            fixed_key(),
            1024,
            None,
            ScriptedOutcome::Complete,
            None,
            rt,
        );

        let outcome = svc.run(exchange()).await;
        assert!(matches!(outcome, PipelineOutcome::Completed(_)));

        let recorded = counters.lock().unwrap().clone();
        assert!(
            recorded.contains(&(
                "camel.cache.hits".to_string(),
                1.0,
                vec![("repository".to_string(), "mock".to_string())]
            )),
            "expected camel.cache.hits counter, got: {recorded:?}"
        );
    }

    #[tokio::test]
    async fn cache_step_miss_increments_otel_counter() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let collector = RecordingMetricsCollector::new();
        let counters = collector.counters.clone();
        let rt = Arc::new(TestOtelmRt {
            collector: Arc::new(collector),
        });
        let (mut svc, _invoked) = build_service(
            repo.clone(),
            fixed_key(),
            1024,
            Some(Body::Bytes(Bytes::from_static(b"x"))),
            ScriptedOutcome::Complete,
            None,
            rt,
        );

        let outcome = svc.run(exchange()).await;
        assert!(matches!(outcome, PipelineOutcome::Completed(_)));

        let recorded = counters.lock().unwrap().clone();
        assert!(
            recorded.contains(&(
                "camel.cache.misses".to_string(),
                1.0,
                vec![("repository".to_string(), "mock".to_string())]
            )),
            "expected camel.cache.misses counter, got: {recorded:?}"
        );
    }
}
