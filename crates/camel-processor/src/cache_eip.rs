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

// ── Singleflight miss coalescing (cache-admin task 2.4) ──

/// Terminal state a coalescing leader publishes for its waiters.
///
/// Mirrors the leader's own `PipelineOutcome` in a clonable shape so
/// every waiter of the wave receives it (waiters clone-read; nobody
/// consumes the slot).
#[derive(Clone)]
pub(crate) enum CoalesceTerminal {
    /// Leader completed: waiters adopt the leader's resulting body.
    Completed(Body),
    /// Leader failed: waiters fail with the same error (anti-burst).
    Failed(CamelError),
    /// Leader stopped: waiters stop their own exchanges (branch-filter).
    Stopped,
}

/// One in-flight coalescing wave for a resolved cache key.
///
/// `terminal` is a write-once slot filled BEFORE `notify_waiters()` is
/// called, so a woken waiter always re-reads a filled slot (no lost
/// wakeup: `notify_waiters` alone wakes only currently-registered
/// waiters).
struct InFlight {
    terminal: std::sync::Mutex<Option<CoalesceTerminal>>,
    notify: tokio::sync::Notify,
}

impl Default for InFlight {
    fn default() -> Self {
        Self {
            terminal: std::sync::Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        }
    }
}

impl InFlight {
    /// Publish the terminal state. Write-once: an already-filled slot is
    /// never cleared or overwritten (a late `LeaderGuard::drop` after a
    /// normal completion is a no-op here).
    fn publish(&self, terminal: CoalesceTerminal) {
        if let Ok(mut slot) = self.terminal.lock()
            && slot.is_none()
        {
            *slot = Some(terminal);
        }
    }

    /// Clone-read the terminal state, if published.
    fn terminal_snapshot(&self) -> Option<CoalesceTerminal> {
        match self.terminal.lock() {
            Ok(slot) => (*slot).clone(),
            Err(_) => None,
        }
    }
}

/// In-flight coalescing waves, keyed by resolved cache key, scoped per
/// compiled route-step instance (shared across `CacheService` clones).
type InFlightMap = std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<InFlight>>>;

/// Cancellation guard for the coalescing leader.
///
/// If the leader future is dropped before it publishes its terminal
/// state (route shutdown, task abort), `Drop` publishes a cancellation
/// terminal (`Failed`) into the write-once slot, wakes waiters, and
/// removes the map entry — so waiters are never stranded. On normal
/// completion the slot is already filled (publish is skipped) and the
/// entry is already retired; both `Drop` actions become no-ops.
struct LeaderGuard {
    key: String,
    map: Arc<InFlightMap>,
    cell: Arc<InFlight>,
}

impl LeaderGuard {
    /// Remove the map entry iff it still identifies `cell`.
    ///
    /// The `Arc::ptr_eq` identity check keeps a late guard (or a
    /// completed leader) from evicting a NEWER wave's entry for the
    /// same key.
    fn retire(map: &Arc<InFlightMap>, key: &str, cell: &Arc<InFlight>) {
        if let Ok(mut map) = map.lock()
            && map
                .get(key)
                .is_some_and(|current| Arc::ptr_eq(current, cell))
        {
            map.remove(key);
        }
    }
}

impl Drop for LeaderGuard {
    fn drop(&mut self) {
        // Write-once: no-op when the leader already published a terminal.
        self.cell
            .publish(CoalesceTerminal::Failed(CamelError::Config(
                "cache coalesce leader cancelled".into(),
            )));
        self.cell.notify.notify_waiters();
        Self::retire(&self.map, &self.key, &self.cell);
    }
}

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
///
/// With `coalesce_misses` enabled ([`CacheService::with_coalesce`]),
/// concurrent misses on the same resolved key are coalesced
/// (singleflight): the first exchange (leader) runs `on_miss` and the
/// single write-back `set`; concurrent exchanges (waiters) await the
/// leader's terminal state instead of running `on_miss`. HIT, key-`None`,
/// and `coalesce_misses == false` paths bypass the in-flight map
/// entirely.
pub struct CacheService {
    repository: Arc<dyn CacheRepository>,
    /// Cached `repository.name()` for OTel span tagging (Task 3.3).
    repository_name: String,
    key_expr: MessageIdExpression,
    ttl: Option<Duration>,
    max_entry_bytes: usize,
    on_miss: OutcomeSegment,
    rt: Arc<dyn RuntimeObservability>,
    /// Singleflight miss coalescing toggle (default `false`).
    coalesce_misses: bool,
    /// In-flight coalescing waves. `Clone` clones the `Arc`, so every
    /// service clone of one compiled route-step shares the same map.
    inflight: Arc<InFlightMap>,
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
            coalesce_misses: false,
            inflight: Arc::new(InFlightMap::default()),
        }
    }

    /// Enable (or explicitly disable) singleflight miss coalescing.
    ///
    /// With coalescing on, concurrent misses on the same resolved key
    /// run the `on_miss` sub-pipeline exactly once per wave (leader
    /// runs + writes back; waiters receive the leader's terminal state).
    pub fn with_coalesce(mut self, coalesce_misses: bool) -> Self {
        self.coalesce_misses = coalesce_misses;
        self
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
            payload_path: None,
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
            coalesce_misses: self.coalesce_misses,
            inflight: Arc::clone(&self.inflight),
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

            // 3./4. MISS flow, singleflight-coalesced when enabled.
            if self.coalesce_misses {
                return self.coalesced_miss(exchange, key).await;
            }
            self.run_miss(exchange, key).await
        })
    }
}

impl CacheService {
    /// The un-coalesced MISS flow (spec steps 3-4): run the on-miss
    /// sub-pipeline, then write the resulting body back (subject to
    /// `max_entry_bytes` and the materialization policy). Also the
    /// leader's flow under coalescing.
    async fn run_miss(&mut self, exchange: Exchange, key: String) -> PipelineOutcome {
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
                    payload_path: None,
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
    }

    /// The coalesced MISS flow (singleflight, cache-admin task 2.4).
    ///
    /// The first exchange on a key (leader) inserts the in-flight cell
    /// and runs [`CacheService::run_miss`] (on_miss + the single
    /// write-back `set`) under a [`LeaderGuard`]; concurrent misses on
    /// the same key (waiters) do NOT run on_miss — they await the
    /// leader's terminal state and clone-read it.
    ///
    /// Protocol (cancellation-safe, race-free):
    /// - Waiter registration is atomic with the map lookup: the
    ///   pinned-`Notified` `enable()` happens while STILL holding the
    ///   map lock, so the leader's publish+notify cannot slip between
    ///   the lookup and the registration.
    /// - The terminal slot is filled BEFORE `notify_waiters()`, and a
    ///   woken waiter re-reads the slot (no lost wakeup —
    ///   `notify_waiters` alone wakes only currently-registered
    ///   waiters).
    /// - Map removal happens only on `Arc::ptr_eq` identity with this
    ///   leader's cell (a late guard cannot evict a newer wave's entry).
    /// - The slot is write-once: once filled it is never cleared or
    ///   overwritten.
    async fn coalesced_miss(&mut self, exchange: Exchange, key: String) -> PipelineOutcome {
        let inflight = Arc::clone(&self.inflight);

        // Resolve the role under ONE short lock scope; the guard (and
        // the lock Result) never crosses an await. A waiter registers
        // its Notified (Box::pin + enable) while STILL holding the map
        // lock — registration atomic with the lookup. The registration
        // borrows the outer `wave` binding (which outlives the guard),
        // so it can be awaited after the guard is gone.
        let mut wave: Option<Arc<InFlight>> = None;
        let mut registered = None;
        match inflight.lock() {
            Ok(mut map) => {
                match map.get(&key).cloned() {
                    Some(existing) => {
                        // WAITER: claim the existing wave, then register
                        // (pin + enable) while STILL holding the lock.
                        wave = Some(existing);
                        if let Some(cell) = wave.as_ref() {
                            let mut notified = Box::pin(cell.notify.notified());
                            notified.as_mut().enable();
                            registered = Some(notified);
                        }
                    }
                    None => {
                        // LEADER: claim the key.
                        let cell = Arc::new(InFlight::default());
                        map.insert(key.clone(), Arc::clone(&cell));
                        wave = Some(cell);
                    }
                }
            }
            Err(poisoned) => drop(poisoned),
        }
        let Some(cell_ref) = wave.as_ref() else {
            // Unreachable on the Ok path (both arms set `wave`); a
            // poisoned in-flight map degrades to un-coalesced
            // execution rather than stranding exchanges.
            return self.run_miss(exchange, key).await;
        };

        if let Some(notified) = registered {
            // WAITER. The slot may already be filled (leader finished
            // between registration and this read): clone-read without
            // parking. Otherwise await the leader's notify and re-read.
            let terminal = match cell_ref.terminal_snapshot() {
                Some(t) => t,
                None => {
                    notified.await;
                    cell_ref.terminal_snapshot().unwrap_or_else(|| {
                        CoalesceTerminal::Failed(CamelError::Config(
                            "cache coalesce waiter woke without a terminal state".into(),
                        ))
                    })
                }
            };
            match terminal {
                CoalesceTerminal::Completed(body) => {
                    let mut exchange = exchange;
                    exchange.input.body = body;
                    PipelineOutcome::Completed(exchange)
                }
                CoalesceTerminal::Failed(e) => PipelineOutcome::Failed(e),
                CoalesceTerminal::Stopped => PipelineOutcome::Stopped(exchange),
            }
        } else {
            // LEADER. Run the miss flow under a cancellation guard,
            // publish the terminal state, wake waiters, retire the
            // map entry.
            let cell = Arc::clone(cell_ref);
            let _guard = LeaderGuard {
                key: key.clone(),
                map: Arc::clone(&inflight),
                cell: Arc::clone(&cell),
            };
            let outcome = self.run_miss(exchange, key.clone()).await;
            let terminal = match &outcome {
                PipelineOutcome::Completed(ex) => {
                    CoalesceTerminal::Completed(ex.input.body.clone())
                }
                PipelineOutcome::Failed(e) => CoalesceTerminal::Failed(e.clone()),
                PipelineOutcome::Stopped(_) => CoalesceTerminal::Stopped,
            };
            // Slot BEFORE notify: woken waiters re-read a filled slot.
            cell.publish(terminal);
            cell.notify.notify_waiters();
            LeaderGuard::retire(&inflight, &key, &cell);
            // Guard drop is a no-op now: the write-once slot is
            // filled and the entry is retired.
            outcome
        }
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
// CacheInvalidateService — invalidate a single cache entry or a namespace
// ===========================================================================

/// Exchange property set to the number of entries removed by a successful
/// `cache_invalidate` step — always `1` for exact-key (removal is not
/// observable by the backend), the returned count for a namespace purge. Not
/// set when the key/prefix expression resolves to `None` or the backend
/// reports an error.
pub const CAMEL_CACHE_INVALIDATED_COUNT: &str = "CamelCacheInvalidatedCount";

/// The invalidation target of a [`CacheInvalidateService`]: an exact key or a
/// namespace prefix.
#[derive(Clone)]
pub enum CacheInvalidateTarget {
    /// Invalidate the single entry under the resolved key.
    Key(MessageIdExpression),
    /// Invalidate every entry whose key starts with the resolved prefix.
    Prefix(MessageIdExpression),
}

/// Outcome-aware segment that invalidates a single cache entry or a namespace.
///
/// Evaluates the configured [`CacheInvalidateTarget`]:
/// - [`Key`](CacheInvalidateTarget::Key): expression `None` → `Completed`
///   (nothing to invalidate); `Some(key)` → `repository.invalidate(&key).await`.
///   - `Err(e)` → `Failed(e)`.
///   - `Ok(())` → sets `CAMEL_CACHE_INVALIDATED_COUNT = 1`, emits
///     `camel.cache.invalidations` +1, `Completed(exchange)`.
/// - [`Prefix`](CacheInvalidateTarget::Prefix): expression `None` → `Completed`;
///   `Some(prefix)` → `repository.invalidate_prefix(&prefix).await`.
///   - `Err(e)` → `Failed(e)` (an unsupported backend surfaces as failure —
///     fail-closed).
///   - `Ok(count)` → sets `CAMEL_CACHE_INVALIDATED_COUNT = count`, emits
///     `camel.cache.invalidations` +1, `Completed(exchange)`.
pub struct CacheInvalidateService {
    repository: Arc<dyn CacheRepository>,
    target: CacheInvalidateTarget,
    rt: Arc<dyn RuntimeObservability>,
    repository_name: String,
}

impl CacheInvalidateService {
    pub fn new(
        repository: Arc<dyn CacheRepository>,
        target: CacheInvalidateTarget,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Self {
        let repository_name = repository.name().to_string();
        Self {
            repository,
            target,
            rt,
            repository_name,
        }
    }
}

impl Clone for CacheInvalidateService {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            target: self.target.clone(),
            rt: Arc::clone(&self.rt),
            repository_name: self.repository_name.clone(),
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
            match self.target.clone() {
                CacheInvalidateTarget::Key(key_expr) => {
                    let key = match key_expr(&exchange) {
                        Some(k) => k,
                        None => return PipelineOutcome::Completed(exchange),
                    };
                    match self.repository.invalidate(&key).await {
                        Err(e) => PipelineOutcome::Failed(e),
                        Ok(()) => {
                            self.rt.metrics().record_counter(
                                "camel.cache.invalidations",
                                1.0_f64,
                                &[("repository", &self.repository_name)],
                            );
                            let mut exchange = exchange;
                            exchange.set_property(
                                CAMEL_CACHE_INVALIDATED_COUNT,
                                serde_json::Value::from(1u64),
                            );
                            PipelineOutcome::Completed(exchange)
                        }
                    }
                }
                CacheInvalidateTarget::Prefix(prefix_expr) => {
                    let prefix = match prefix_expr(&exchange) {
                        Some(p) => p,
                        None => return PipelineOutcome::Completed(exchange),
                    };
                    match self.repository.invalidate_prefix(&prefix).await {
                        Err(e) => PipelineOutcome::Failed(e),
                        Ok(count) => {
                            self.rt.metrics().record_counter(
                                "camel.cache.invalidations",
                                1.0_f64,
                                &[("repository", &self.repository_name)],
                            );
                            let mut exchange = exchange;
                            exchange.set_property(
                                CAMEL_CACHE_INVALIDATED_COUNT,
                                serde_json::Value::from(count),
                            );
                            PipelineOutcome::Completed(exchange)
                        }
                    }
                }
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
    rt: Arc<dyn RuntimeObservability>,
    repository_name: String,
}

impl CachePeekStaleService {
    pub fn new(
        repository: Arc<dyn CacheRepository>,
        key_expr: MessageIdExpression,
        miss_policy: PeekStaleMissPolicy,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Self {
        let repository_name = repository.name().to_string();
        Self {
            repository,
            key_expr,
            miss_policy,
            rt,
            repository_name,
        }
    }
}

impl Clone for CachePeekStaleService {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            key_expr: Arc::clone(&self.key_expr),
            miss_policy: self.miss_policy,
            rt: Arc::clone(&self.rt),
            repository_name: self.repository_name.clone(),
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
                    // Emit peek_stale_served (fresh or stale — both are serves).
                    self.rt.metrics().record_counter(
                        "camel.cache.peek_stale_served",
                        1.0_f64,
                        &[("repository", &self.repository_name)],
                    );
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
// CacheClearService — remove all entries from the repository
// ===========================================================================

/// Outcome-aware segment that clears the entire cache repository.
///
/// - `Err(e)` → `Failed(e)`.
/// - `Ok(())` → `Completed(exchange)` with the body unchanged.
pub struct CacheClearService {
    repository: Arc<dyn CacheRepository>,
}

impl CacheClearService {
    pub fn new(repository: Arc<dyn CacheRepository>) -> Self {
        Self { repository }
    }
}

impl Clone for CacheClearService {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
        }
    }
}

impl OutcomePipeline for CacheClearService {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            match self.repository.clear().await {
                Err(e) => PipelineOutcome::Failed(e),
                Ok(()) => PipelineOutcome::Completed(exchange),
            }
        })
    }
}

// ===========================================================================
// CacheStatsService — emit the repository stats as a JSON body
// ===========================================================================

/// Outcome-aware segment that replaces the exchange body with a JSON snapshot
/// of the repository's [`CacheStats`].
pub struct CacheStatsService {
    repository: Arc<dyn CacheRepository>,
    repository_name: String,
}

impl CacheStatsService {
    pub fn new(repository: Arc<dyn CacheRepository>) -> Self {
        let repository_name = repository.name().to_string();
        Self {
            repository,
            repository_name,
        }
    }
}

impl Clone for CacheStatsService {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            repository_name: self.repository_name.clone(),
        }
    }
}

impl OutcomePipeline for CacheStatsService {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        mut exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            let s = self.repository.stats().await;
            exchange.input.body = Body::Json(serde_json::json!({
                "repository": self.repository_name,
                "hits": s.hits,
                "misses": s.misses,
                "evictions": s.evictions,
                "entries": s.entries,
                "peek_stale_served": s.peek_stale_served,
                "invalidations": s.invalidations,
                "bytes": s.bytes,
            }));
            PipelineOutcome::Completed(exchange)
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
    use camel_api::cache::CacheStats;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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
        clear_call_count: Arc<AtomicU64>,
        clear_should_fail: Arc<AtomicBool>,
        invalidate_should_fail: Arc<AtomicBool>,
        prefix_unsupported: Arc<AtomicBool>,
        stats_override: std::sync::Mutex<CacheStats>,
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

        pub fn clear_call_count(&self) -> u64 {
            self.clear_call_count.load(Ordering::SeqCst)
        }

        pub fn set_should_fail_clear(&self, v: bool) {
            self.clear_should_fail.store(v, Ordering::SeqCst);
        }

        pub fn set_should_fail_invalidate(&self, v: bool) {
            self.invalidate_should_fail.store(v, Ordering::SeqCst);
        }

        /// Force `invalidate_prefix` to report the backend-naming unsupported
        /// error (mirrors the default `CacheRepository::invalidate_prefix`).
        pub fn set_prefix_unsupported(&self, v: bool) {
            self.prefix_unsupported.store(v, Ordering::SeqCst);
        }

        pub fn set_stats(&self, stats: CacheStats) {
            *self.stats_override.lock().unwrap() = stats; // allow-unwrap: test-only
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
            // Read first, then yield: concurrent same-key lookups that
            // reach `get` before a write-back all observe the miss
            // (without the yield the uncontended tokio Mutex fast path
            // completes without rescheduling, serializing "concurrent"
            // exchanges and hiding the per-exchange behavior under test).
            let found = self.entries.lock().await.get(key).cloned();
            tokio::task::yield_now().await;
            Ok(found)
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
            if self.invalidate_should_fail.load(Ordering::SeqCst) {
                return Err(CamelError::ProcessorError(
                    "synthetic invalidate failure".into(),
                ));
            }
            self.entries.lock().await.remove(key);
            Ok(())
        }

        async fn invalidate_prefix(&self, prefix: &str) -> Result<u64, CamelError> {
            if self.prefix_unsupported.load(Ordering::SeqCst) {
                return Err(CamelError::Config(format!(
                    "cache backend '{}' does not support invalidate_prefix (no key iteration)",
                    self.name()
                )));
            }
            let mut entries = self.entries.lock().await;
            let keys: Vec<String> = entries
                .keys()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect();
            let count = keys.len() as u64;
            for k in keys {
                entries.remove(&k);
            }
            Ok(count)
        }

        async fn clear(&self) -> Result<(), CamelError> {
            self.clear_call_count.fetch_add(1, Ordering::SeqCst);
            if self.clear_should_fail.load(Ordering::SeqCst) {
                return Err(CamelError::ProcessorError("synthetic clear failure".into()));
            }
            self.entries.lock().await.clear();
            Ok(())
        }

        async fn stats(&self) -> CacheStats {
            self.stats_override.lock().unwrap().clone() // allow-unwrap: test-only
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
    use camel_api::cache::CacheStats;
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

    fn prefix_key() -> MessageIdExpression {
        Arc::new(|_| Some("ns:".to_string()))
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
                payload_path: None,
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
                payload_path: None,
                content_type: ContentType::Bytes,
                expires_at: None,
            },
            Body::Bytes(Bytes::from_static(b"raw")),
        )
        .await;
        run_case(
            CacheEntry {
                bytes: b"hi".to_vec(),
                payload_path: None,
                content_type: ContentType::Text,
                expires_at: None,
            },
            Body::Text("hi".into()),
        )
        .await;
        run_case(
            CacheEntry {
                bytes: br#"{"k":1}"#.to_vec(),
                payload_path: None,
                content_type: ContentType::Json,
                expires_at: None,
            },
            Body::Json(serde_json::json!({"k": 1})),
        )
        .await;
        run_case(
            CacheEntry {
                bytes: b"<a/>".to_vec(),
                payload_path: None,
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
                payload_path: None,
                content_type: ContentType::Text,
                expires_at: None,
            },
        )
        .await;
        let mut svc =
            CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop, noop_rt());

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert_eq!(ex.input.body, Body::Text("stale-payload".into()));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn cache_peek_stale_on_absence_stops_branch() {
        let _lock = PEEK_STALE_LOG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc =
            CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop, noop_rt());

        let outcome = svc.run(exchange()).await;

        assert!(
            matches!(outcome, PipelineOutcome::Stopped(_)),
            "expected Stopped when no stale entry, got {outcome:?}"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn cache_peek_stale_none_key_stops() {
        let _lock = PEEK_STALE_LOG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc =
            CachePeekStaleService::new(repo, none_key(), PeekStaleMissPolicy::Stop, noop_rt());

        let outcome = svc.run(exchange()).await;

        assert!(
            matches!(outcome, PipelineOutcome::Stopped(_)),
            "expected Stopped when key_expr returns None, got {outcome:?}"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn peek_stale_miss_stop_sets_properties_and_stops() {
        let _lock = PEEK_STALE_LOG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc =
            CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop, noop_rt());

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
        let mut svc =
            CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Continue, noop_rt());

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
                payload_path: None,
                content_type: ContentType::Bytes,
                expires_at: Some(SystemTime::now() - Duration::from_millis(1)),
            },
        )
        .await;
        let mut svc =
            CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop, noop_rt());

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
                payload_path: None,
                content_type: ContentType::Bytes,
                expires_at: Some(SystemTime::now() + Duration::from_secs(3600)),
            },
        )
        .await;
        let mut svc =
            CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop, noop_rt());

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

    // ── log-capture harness ──────────────────────────────────────────────────

    /// Records dispatched events from this module as `LEVEL field=value…`
    /// lines. `on_event` fires only for events actually dispatched to this
    /// layer: unlike fmt-writer capture it has no registration-time side
    /// effects and does not depend on the process-wide callsite interest
    /// cache, which parallel tests rebuild concurrently (bd rc-pna5).
    #[derive(Default)]
    struct EventRecorder {
        records: Arc<Mutex<Vec<String>>>,
    }

    /// Serializes tests that emit at the shared `cache_peek_stale` debug
    /// callsites (the miss-stop arm and the none-key arm).
    ///
    /// Why: tracing-core caches each callsite's interest process-wide. When a
    /// non-recorder thread first-registers one of these callsites while
    /// exactly one recorder is installed, `Rebuilder::JustOne` consults the
    /// *current thread's* default dispatcher (the no-op one) instead of the
    /// registered subscribers, caching `Interest::never()`. The recorder
    /// test's later emission at the same callsite is then silently filtered
    /// before it reaches the layer, yielding zero captured records. Holding
    /// this lock for the whole test body keeps recorder and non-recorder
    /// tests from overlapping on the same callsite, so the recorder test
    /// always first-registers the callsite with its own subscriber active.
    static PEEK_STALE_LOG_LOCK: Mutex<()> = Mutex::new(());

    impl EventRecorder {
        /// Installs the recorder as this thread's default subscriber and
        /// returns the shared record list plus the dispatcher guard.
        fn install(self) -> (Arc<Mutex<Vec<String>>>, tracing::subscriber::DefaultGuard) {
            use tracing_subscriber::prelude::*;
            let records = Arc::clone(&self.records);
            let guard = tracing_subscriber::registry().with(self).set_default();
            (records, guard)
        }
    }

    /// Formats each visited field as `name=debug-value`, preserving str
    /// quoting so `step="cache_peek_stale"`-style assertions keep working.
    struct FieldFmt<'a>(&'a mut String);

    impl tracing::field::Visit for FieldFmt<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write as _;
            let _ = write!(self.0, " {}={:?}", field.name(), value);
        }
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.record_debug(field, &value);
        }
    }

    impl<C> tracing_subscriber::Layer<C> for EventRecorder
    where
        C: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, C>,
        ) {
            let meta = event.metadata();
            if meta.target() != "camel_processor::cache_eip" {
                return;
            }
            let mut line = format!("{} ", meta.level());
            event.record(&mut FieldFmt(&mut line));
            self.records.lock().unwrap().push(line); // allow-unwrap: test-only
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn peek_stale_miss_stop_emits_debug_log() {
        let _lock = PEEK_STALE_LOG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc =
            CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop, noop_rt());

        let (records, _guard) = EventRecorder::default().install();
        let outcome = svc.run(exchange()).await;
        drop(_guard);

        assert!(matches!(outcome, PipelineOutcome::Stopped(_)));

        let captured = records.lock().unwrap().join("\n"); // allow-unwrap: test-only
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
    #[allow(clippy::await_holding_lock)]
    async fn peek_stale_key_none_stops_with_debug_log() {
        let _lock = PEEK_STALE_LOG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc =
            CachePeekStaleService::new(repo, none_key(), PeekStaleMissPolicy::Stop, noop_rt());

        let (records, _guard) = EventRecorder::default().install();
        let outcome = svc.run(exchange()).await;
        drop(_guard);

        assert!(matches!(outcome, PipelineOutcome::Stopped(_)));

        let captured = records.lock().unwrap().join("\n"); // allow-unwrap: test-only
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
                payload_path: None,
                content_type: ContentType::Bytes,
                expires_at: None,
            },
        )
        .await;
        let mut svc = CacheInvalidateService::new(
            repo.clone(),
            CacheInvalidateTarget::Key(fixed_key()),
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert_eq!(
            ex.property(CAMEL_CACHE_INVALIDATED_COUNT),
            Some(&serde_json::Value::from(1u64)),
            "exact-key success must set CamelCacheInvalidatedCount = 1"
        );
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
        let mut svc = CacheInvalidateService::new(
            repo.clone(),
            CacheInvalidateTarget::Key(none_key()),
            noop_rt(),
        );

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
                payload_path: None,
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

    // ── CacheClearService tests ──

    #[tokio::test]
    async fn cache_clear_calls_repository_clear() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.seed(
            "k",
            CacheEntry {
                bytes: b"v".to_vec(),
                payload_path: None,
                content_type: ContentType::Bytes,
                expires_at: None,
            },
        )
        .await;
        let mut svc = CacheClearService::new(repo.clone());

        let outcome = svc.run(exchange()).await;

        match outcome {
            PipelineOutcome::Completed(_) => {}
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(repo.clear_call_count(), 1, "clear must be called once");
        assert!(
            repo.stored_entry("k").await.is_none(),
            "entry must be removed after clear"
        );
    }

    #[tokio::test]
    async fn cache_clear_err_propagates_failed() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.set_should_fail_clear(true);
        let mut svc = CacheClearService::new(repo);

        let outcome = svc.run(exchange()).await;

        match outcome {
            PipelineOutcome::Failed(e) => {
                assert!(
                    e.to_string().contains("synthetic clear failure"),
                    "got: {e}"
                );
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    // ── CacheStatsService tests ──

    #[tokio::test]
    async fn cache_stats_sets_json_body() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.set_stats(CacheStats {
            hits: 2,
            misses: 1,
            evictions: 0,
            entries: 3,
            peek_stale_served: 4,
            invalidations: 1,
            bytes: None,
        });
        let mut svc = CacheStatsService::new(repo);

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        let expected = serde_json::json!({
            "repository": "mock",
            "hits": 2,
            "misses": 1,
            "evictions": 0,
            "entries": 3,
            "peek_stale_served": 4,
            "invalidations": 1,
            "bytes": null
        });
        assert_eq!(ex.input.body, Body::Json(expected));

        // Exact key-set assertion: the stats JSON snapshot contract is frozen
        // to the eight canonical keys (bd rc-22wj) — no extras, none missing.
        let Body::Json(v) = &ex.input.body else {
            panic!("expected Json body");
        };
        let keys: std::collections::BTreeSet<&str> = v
            .as_object()
            .expect("stats body must be a JSON object")
            .keys()
            .map(String::as_str)
            .collect();
        let expected_keys: std::collections::BTreeSet<&str> = [
            "repository",
            "hits",
            "misses",
            "evictions",
            "entries",
            "peek_stale_served",
            "invalidations",
            "bytes",
        ]
        .into_iter()
        .collect();
        assert_eq!(
            keys, expected_keys,
            "stats body must have exactly the eight canonical keys"
        );
    }

    // ── Peek/invalidate OTel counter tests ──

    #[tokio::test]
    async fn peek_stale_hit_emits_peek_served_counter() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.seed(
            "cache-key",
            CacheEntry {
                bytes: b"cached".to_vec(),
                payload_path: None,
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
        let mut svc = CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop, rt);

        let outcome = svc.run(exchange()).await;
        assert!(matches!(outcome, PipelineOutcome::Completed(_)));

        let recorded = counters.lock().unwrap().clone();
        assert!(
            recorded.contains(&(
                "camel.cache.peek_stale_served".to_string(),
                1.0,
                vec![("repository".to_string(), "mock".to_string())]
            )),
            "expected camel.cache.peek_stale_served counter, got: {recorded:?}"
        );
    }

    #[tokio::test]
    async fn invalidate_emits_invalidations_counter() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.seed(
            "cache-key",
            CacheEntry {
                bytes: b"to-go".to_vec(),
                payload_path: None,
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
        let mut svc =
            CacheInvalidateService::new(repo, CacheInvalidateTarget::Key(fixed_key()), rt);

        let outcome = svc.run(exchange()).await;
        assert!(matches!(outcome, PipelineOutcome::Completed(_)));

        let recorded = counters.lock().unwrap().clone();
        assert!(
            recorded.contains(&(
                "camel.cache.invalidations".to_string(),
                1.0,
                vec![("repository".to_string(), "mock".to_string())]
            )),
            "expected camel.cache.invalidations counter, got: {recorded:?}"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn peek_stale_miss_emits_no_peek_served_counter() {
        // Absent key -> MISS path must NOT emit camel.cache.peek_stale_served.
        let _lock = PEEK_STALE_LOG_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let collector = RecordingMetricsCollector::new();
        let counters = collector.counters.clone();
        let rt = Arc::new(TestOtelmRt {
            collector: Arc::new(collector),
        });
        let mut svc = CachePeekStaleService::new(repo, fixed_key(), PeekStaleMissPolicy::Stop, rt);

        let outcome = svc.run(exchange()).await;
        assert!(matches!(outcome, PipelineOutcome::Stopped(_)));

        let recorded = counters.lock().unwrap().clone();
        assert!(
            !recorded
                .iter()
                .any(|(name, _, _)| name == "camel.cache.peek_stale_served"),
            "expected zero camel.cache.peek_stale_served counters, got: {recorded:?}"
        );
    }

    #[tokio::test]
    async fn invalidate_err_emits_no_invalidations_counter() {
        // Failing invalidate -> Failed outcome must NOT emit camel.cache.invalidations.
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.seed(
            "cache-key",
            CacheEntry {
                bytes: b"to-go".to_vec(),
                payload_path: None,
                content_type: ContentType::Bytes,
                expires_at: None,
            },
        )
        .await;
        repo.set_should_fail_invalidate(true);
        let collector = RecordingMetricsCollector::new();
        let counters = collector.counters.clone();
        let rt = Arc::new(TestOtelmRt {
            collector: Arc::new(collector),
        });
        let mut svc =
            CacheInvalidateService::new(repo, CacheInvalidateTarget::Key(fixed_key()), rt);

        let outcome = svc.run(exchange()).await;
        assert!(matches!(outcome, PipelineOutcome::Failed(_)));

        let recorded = counters.lock().unwrap().clone();
        assert!(
            !recorded
                .iter()
                .any(|(name, _, _)| name == "camel.cache.invalidations"),
            "expected zero camel.cache.invalidations counters, got: {recorded:?}"
        );
    }

    // ── CacheInvalidateService prefix tests ──

    #[tokio::test]
    async fn cache_invalidate_prefix_removes_namespace_sets_count() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        for key in ["ns:one", "ns:two", "other:x"] {
            repo.seed(
                key,
                CacheEntry {
                    bytes: key.as_bytes().to_vec(),
                    payload_path: None,
                    content_type: ContentType::Bytes,
                    expires_at: None,
                },
            )
            .await;
        }

        let collector = RecordingMetricsCollector::new();
        let counters = collector.counters.clone();
        let rt = Arc::new(TestOtelmRt {
            collector: Arc::new(collector),
        });
        let mut svc = CacheInvalidateService::new(
            repo.clone(),
            CacheInvalidateTarget::Prefix(prefix_key()),
            rt,
        );

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert_eq!(
            ex.property(CAMEL_CACHE_INVALIDATED_COUNT),
            Some(&serde_json::Value::from(2u64)),
            "prefix purge must report the removed count"
        );
        assert!(
            repo.stored_entry("ns:one").await.is_none(),
            "ns:one must be removed"
        );
        assert!(
            repo.stored_entry("ns:two").await.is_none(),
            "ns:two must be removed"
        );
        assert!(
            repo.stored_entry("other:x").await.is_some(),
            "other:x must be preserved"
        );

        let recorded = counters.lock().unwrap().clone();
        assert!(
            recorded.contains(&(
                "camel.cache.invalidations".to_string(),
                1.0,
                vec![("repository".to_string(), "mock".to_string())]
            )),
            "expected one camel.cache.invalidations counter, got: {recorded:?}"
        );
    }

    #[tokio::test]
    async fn cache_invalidate_prefix_none_expr_completes() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let mut svc = CacheInvalidateService::new(
            repo.clone(),
            CacheInvalidateTarget::Prefix(none_key()),
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        let ex = match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected Completed, got {other:?}"),
        };
        assert_eq!(
            repo.invalidate_call_count(),
            0,
            "no invalidate calls when prefix expr resolves to None"
        );
        assert!(
            ex.property(CAMEL_CACHE_INVALIDATED_COUNT).is_none(),
            "no count property when prefix expr resolves to None"
        );
    }

    #[tokio::test]
    async fn cache_invalidate_prefix_unsupported_fails_closed() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        repo.set_prefix_unsupported(true);
        let mut svc = CacheInvalidateService::new(
            repo,
            CacheInvalidateTarget::Prefix(prefix_key()),
            noop_rt(),
        );

        let outcome = svc.run(exchange()).await;

        match outcome {
            PipelineOutcome::Failed(e) => {
                let msg = format!("{e}");
                assert!(
                    msg.contains("mock"),
                    "error must name the backend, got: {msg}"
                );
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    // ── Task 2.4: singleflight miss coalescing ──

    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Notify;

    /// Terminal behavior of the gated on-miss sub-pipeline.
    #[derive(Clone)]
    enum GatedOutcome {
        Complete(Body),
        Fail(CamelError),
        Stop,
    }

    /// Test on-miss sub-pipeline with two Notify gates: signals
    /// `leader_entered` as its FIRST action, parks on `release.notified()`,
    /// then bumps the invocation counter and returns the scripted outcome.
    /// Both gates are optional so the same struct serves ungated tests.
    #[derive(Clone)]
    struct GatedOnMiss {
        leader_entered: Option<Arc<Notify>>,
        release: Option<Arc<Notify>>,
        invocations: Arc<AtomicUsize>,
        outcome: GatedOutcome,
    }

    impl OutcomePipeline for GatedOnMiss {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(self.clone())
        }

        fn run<'a>(
            &'a mut self,
            mut exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let leader_entered = self.leader_entered.clone();
            let release = self.release.clone();
            let invocations = Arc::clone(&self.invocations);
            let outcome = self.outcome.clone();
            Box::pin(async move {
                // FIRST action: the leader is inside on_miss.
                if let Some(entered) = leader_entered.as_ref() {
                    entered.notify_one();
                }
                // Park until the test releases the wave.
                if let Some(release) = release.as_ref() {
                    release.notified().await;
                }
                invocations.fetch_add(1, Ordering::SeqCst);
                match outcome {
                    GatedOutcome::Complete(body) => {
                        exchange.input.body = body;
                        PipelineOutcome::Completed(exchange)
                    }
                    GatedOutcome::Fail(e) => PipelineOutcome::Failed(e),
                    GatedOutcome::Stop => PipelineOutcome::Stopped(exchange),
                }
            })
        }
    }

    /// Build a coalescing CacheService around a `GatedOnMiss`.
    fn build_gated_service(
        repo: Arc<MockCacheRepository>,
        outcome: GatedOutcome,
        leader_entered: Option<Arc<Notify>>,
        release: Option<Arc<Notify>>,
    ) -> (CacheService, Arc<AtomicUsize>) {
        let invocations = Arc::new(AtomicUsize::new(0));
        let on_miss = OutcomeSegment::new(Box::new(GatedOnMiss {
            leader_entered,
            release,
            invocations: Arc::clone(&invocations),
            outcome,
        }));
        let svc = CacheService::new(repo, fixed_key(), None, 1024, on_miss, noop_rt())
            .with_coalesce(true);
        (svc, invocations)
    }

    fn exchange_with_body(text: &str) -> Exchange {
        Exchange::new(Message::new(text))
    }

    #[tokio::test]
    async fn coalesce_three_concurrent_misses_fetch_once() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let leader_entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (svc, invocations) = build_gated_service(
            repo.clone(),
            GatedOutcome::Complete(Body::Text("fetched".into())),
            Some(Arc::clone(&leader_entered)),
            Some(Arc::clone(&release)),
        );

        // Register on leader_entered BEFORE spawning the leader so its
        // notify_one cannot be missed (enable-before-check).
        let entered = leader_entered.notified();
        tokio::pin!(entered);
        entered.as_mut().enable();

        let mut leader_svc = svc.clone();
        let leader = tokio::spawn(async move { leader_svc.run(exchange()).await });
        entered.await; // deterministic proof: the leader is inside on_miss.

        let mut w1_svc = svc.clone();
        let mut w1 = tokio::spawn(async move { w1_svc.run(exchange()).await });
        let mut w2_svc = svc.clone();
        let mut w2 = tokio::spawn(async move { w2_svc.run(exchange()).await });

        // Both waiters park (registered on the wave, not running on_miss).
        for waiter in [&mut w1, &mut w2] {
            if let Ok(done) = tokio::time::timeout(Duration::from_millis(50), waiter).await {
                panic!("waiter resolved before release: {done:?}")
            }
        }

        release.notify_waiters();

        let leader_ex = match leader.await.expect("leader task join") {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected leader Completed, got {other:?}"),
        };
        let w1_ex = match w1.await.expect("waiter 1 task join") {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected waiter 1 Completed, got {other:?}"),
        };
        let w2_ex = match w2.await.expect("waiter 2 task join") {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected waiter 2 Completed, got {other:?}"),
        };
        assert_eq!(leader_ex.input.body, Body::Text("fetched".into()));
        assert_eq!(w1_ex.input.body, Body::Text("fetched".into()));
        assert_eq!(w2_ex.input.body, Body::Text("fetched".into()));
        assert_eq!(invocations.load(Ordering::SeqCst), 1, "on_miss ran once");
        assert_eq!(repo.set_call_count(), 1, "single write-back set");
    }

    #[tokio::test]
    async fn coalesce_leader_failure_fails_waiters_once() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let leader_entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (svc, invocations) = build_gated_service(
            repo.clone(),
            GatedOutcome::Fail(stub_error("coalesce-boom")),
            Some(Arc::clone(&leader_entered)),
            Some(Arc::clone(&release)),
        );

        let entered = leader_entered.notified();
        tokio::pin!(entered);
        entered.as_mut().enable();

        let mut leader_svc = svc.clone();
        let leader = tokio::spawn(async move { leader_svc.run(exchange()).await });
        entered.await;

        let mut w1_svc = svc.clone();
        let mut w1 = tokio::spawn(async move { w1_svc.run(exchange()).await });
        let mut w2_svc = svc.clone();
        let mut w2 = tokio::spawn(async move { w2_svc.run(exchange()).await });

        for waiter in [&mut w1, &mut w2] {
            if let Ok(done) = tokio::time::timeout(Duration::from_millis(50), waiter).await {
                panic!("waiter resolved before release: {done:?}")
            }
        }

        release.notify_waiters();

        let leader_err = match leader.await.expect("leader task join") {
            PipelineOutcome::Failed(e) => e,
            other => panic!("expected leader Failed, got {other:?}"),
        };
        let w1_err = match w1.await.expect("waiter 1 task join") {
            PipelineOutcome::Failed(e) => e,
            other => panic!("expected waiter 1 Failed, got {other:?}"),
        };
        let w2_err = match w2.await.expect("waiter 2 task join") {
            PipelineOutcome::Failed(e) => e,
            other => panic!("expected waiter 2 Failed, got {other:?}"),
        };
        assert_eq!(format!("{leader_err}"), format!("{w1_err}"));
        assert_eq!(format!("{leader_err}"), format!("{w2_err}"));
        assert_eq!(invocations.load(Ordering::SeqCst), 1, "on_miss ran once");
        assert_eq!(repo.set_call_count(), 0, "no write-back on failure");
    }

    #[tokio::test]
    async fn coalesce_leader_stopped_stops_waiters() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let leader_entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (svc, invocations) = build_gated_service(
            repo.clone(),
            GatedOutcome::Stop,
            Some(Arc::clone(&leader_entered)),
            Some(Arc::clone(&release)),
        );

        let entered = leader_entered.notified();
        tokio::pin!(entered);
        entered.as_mut().enable();

        let mut leader_svc = svc.clone();
        let leader =
            tokio::spawn(async move { leader_svc.run(exchange_with_body("leader-orig")).await });
        entered.await;

        let mut w1_svc = svc.clone();
        let mut w1 =
            tokio::spawn(async move { w1_svc.run(exchange_with_body("waiter-orig")).await });

        if let Ok(done) = tokio::time::timeout(Duration::from_millis(50), &mut w1).await {
            panic!("waiter resolved before release: {done:?}")
        }

        release.notify_waiters();

        let leader_ex = match leader.await.expect("leader task join") {
            PipelineOutcome::Stopped(ex) => ex,
            other => panic!("expected leader Stopped, got {other:?}"),
        };
        let waiter_ex = match w1.await.expect("waiter task join") {
            PipelineOutcome::Stopped(ex) => ex,
            other => panic!("expected waiter Stopped, got {other:?}"),
        };
        assert_eq!(
            leader_ex.input.body,
            Body::Text("leader-orig".into()),
            "leader stopped with its own exchange"
        );
        assert_eq!(
            waiter_ex.input.body,
            Body::Text("waiter-orig".into()),
            "waiter stopped with its own exchange, body untouched"
        );
        assert_eq!(invocations.load(Ordering::SeqCst), 1, "on_miss ran once");
        assert_eq!(repo.set_call_count(), 0, "no write-back on stop");
    }

    #[tokio::test]
    async fn coalesce_leader_dropped_does_not_strand_waiters() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let leader_entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (svc, _invocations) = build_gated_service(
            repo,
            GatedOutcome::Complete(Body::Text("fetched".into())),
            Some(Arc::clone(&leader_entered)),
            Some(Arc::clone(&release)),
        );

        let entered = leader_entered.notified();
        tokio::pin!(entered);
        entered.as_mut().enable();

        let mut leader_svc = svc.clone();
        let leader = tokio::spawn(async move { leader_svc.run(exchange()).await });
        entered.await;

        let mut w1_svc = svc.clone();
        let mut w1 = tokio::spawn(async move { w1_svc.run(exchange()).await });

        if let Ok(done) = tokio::time::timeout(Duration::from_millis(50), &mut w1).await {
            panic!("waiter resolved before leader abort: {done:?}")
        }

        // Drop the leader future mid-flight: the cancellation guard must
        // publish a Failed terminal and retire the wave's map entry.
        leader.abort();

        let joined = tokio::time::timeout(Duration::from_secs(1), w1)
            .await
            .expect("waiter completes within 1s after leader drop")
            .expect("waiter task join");
        match joined {
            PipelineOutcome::Failed(e) => {
                let msg = format!("{e}");
                assert!(
                    msg.contains("cancelled"),
                    "expected cancellation terminal, got: {msg}"
                );
            }
            other => panic!("expected waiter Failed, got {other:?}"),
        }
        assert!(
            svc.inflight.lock().unwrap().is_empty(),
            "in-flight map must not retain the aborted wave's entry"
        );
    }

    #[tokio::test]
    async fn no_coalesce_runs_per_exchange() {
        let repo = Arc::new(MockCacheRepository::new("mock"));
        let invocations = Arc::new(AtomicUsize::new(0));
        let on_miss = OutcomeSegment::new(Box::new(GatedOnMiss {
            leader_entered: None,
            release: None,
            invocations: Arc::clone(&invocations),
            outcome: GatedOutcome::Complete(Body::Text("per-exchange".into())),
        }));
        // Default construction (no with_coalesce): per-exchange execution.
        let svc = CacheService::new(repo.clone(), fixed_key(), None, 1024, on_miss, noop_rt());

        let mut a = svc.clone();
        let mut b = svc.clone();
        let mut c = svc.clone();
        let (ra, rb, rc) = tokio::join!(a.run(exchange()), b.run(exchange()), c.run(exchange()));

        for outcome in [ra, rb, rc] {
            match outcome {
                PipelineOutcome::Completed(ex) => {
                    assert_eq!(ex.input.body, Body::Text("per-exchange".into()));
                }
                other => panic!("expected Completed, got {other:?}"),
            }
        }
        assert_eq!(
            invocations.load(Ordering::SeqCst),
            3,
            "on_miss ran per exchange"
        );
        assert_eq!(repo.set_call_count(), 3, "set called per exchange");
    }
}
