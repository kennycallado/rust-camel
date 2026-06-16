//! Producer-level response cache with single-flight coalescing.
//!
//! The cache lives at the producer level — lookups happen BEFORE the
//! semaphore/retry/timeout layer. Single-flight waiters hold ZERO permits.
//! Materialized-ONLY; streaming never caches.
//!
//! ## Architecture
//!
//! - `get()` for cache hits (fast path, no allocation for the leader)
//! - `acquire()` returns either a `LeaderHandle` (first caller) or a
//!   `Waiter` receiver (subsequent concurrent callers)
//! - `LeaderHandle::complete()` stores the result and fans it out to waiters
//! - LeaderHandle's `Drop` sends a cancellation error so waiters never hang
//! - TTL + LRU eviction (per-entry monotonic `access_order`; `trim()` runs on
//!   leader-complete when `max_entries` is set; `in_flight` entries never trimmed)

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dashmap::{DashMap, mapref::entry::Entry};
use tokio::sync::watch;

use crate::error::LlmError;
use crate::provider::{ChatRequest, LlmUsage};

// ---------------------------------------------------------------------------
// Exported types
// ---------------------------------------------------------------------------

/// A cached response entry.
#[derive(Debug)]
pub struct CachedEntry {
    /// The full response text.
    pub text: String,
    /// Token usage (if available from the original response).
    pub usage: Option<LlmUsage>,
    /// When this entry was created (for TTL check).
    pub stored_at: Instant,
    /// Monotonic access sequence number (from cache.access_counter).
    pub access_order: AtomicU64,
}

/// Outcome of `ProducerCache::acquire()`.
pub enum Slot {
    /// This caller is the leader — run provider work, then call
    /// `LeaderHandle::complete(result)`.
    Leader(LeaderHandle),
    /// This caller is a waiter — await the leader's result via `wait()`.
    Waiter {
        /// Subscribe to the leader's completion signal.
        rx: FlightReceiver,
    },
}

/// RAII handle: the leader MUST call `complete()`. If dropped without
/// completing (caller cancelled/panicked), Drop fans out a cancellation
/// error to waiters and removes the in-flight slot.
pub struct LeaderHandle {
    key: String,
    cache: Arc<ProducerCache>,
    tx: Option<FlightSender>,
}

impl LeaderHandle {
    /// Finalise the leader's work and fan out the result to all waiters.
    /// On success, stores the entry in the cache. On error, propagates
    /// the error to waiters (no per-waiter retry).
    pub fn complete(mut self, result: Result<(String, Option<LlmUsage>), LlmError>) {
        match result {
            Ok((text, usage)) => {
                let seq = self.cache.access_counter.fetch_add(1, Ordering::Relaxed);
                let entry = Arc::new(CachedEntry {
                    text,
                    usage,
                    stored_at: Instant::now(),
                    access_order: AtomicU64::new(seq),
                });
                self.cache
                    .entries
                    .insert(self.key.clone(), Arc::clone(&entry));
                if let Some(tx) = self.tx.take() {
                    let _ = tx.send(Some(Ok(entry)));
                }
                self.cache.trim();
            }
            Err(e) => {
                if let Some(tx) = self.tx.take() {
                    let _ = tx.send(Some(Err(e)));
                }
            }
        }
    }
}

impl Drop for LeaderHandle {
    fn drop(&mut self) {
        // Only send cancellation if complete() was never called
        // (tx still present).
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Some(Err(LlmError::Provider(
                "single-flight leader cancelled".into(),
            ))));
        }
        self.cache.in_flight.remove(&self.key);
    }
}

// ---------------------------------------------------------------------------
// Cache implementation
// ---------------------------------------------------------------------------

/// Type alias for the in-flight single-flight map.
/// Type alias for the watch channel sender used by single-flight.
type FlightSender = watch::Sender<Option<Result<Arc<CachedEntry>, LlmError>>>;
/// Type alias for the watch channel receiver used by waiters.
type FlightReceiver = watch::Receiver<Option<Result<Arc<CachedEntry>, LlmError>>>;
/// Type alias for the in-flight single-flight map.
type InFlightMap = DashMap<String, FlightSender>;

/// Producer-level response cache with single-flight coalescing.
///
/// Only materialised (non-streaming) responses are cached. The cache key
/// is a hash of the canonicalised request tuple. TTL eviction is always
/// active; optional LRU eviction via `max_entries`.
pub struct ProducerCache {
    ttl: Duration,
    entries: DashMap<String, Arc<CachedEntry>>,
    in_flight: InFlightMap,
    max_entries: Option<usize>,
    /// Monotonic counter for LRU ordering.
    access_counter: AtomicU64,
}

impl ProducerCache {
    /// Create a new cache with the given TTL and optional max entries.
    pub fn new(ttl: Duration, max_entries: Option<usize>) -> Self {
        Self {
            ttl,
            entries: DashMap::new(),
            in_flight: DashMap::new(),
            max_entries,
            access_counter: AtomicU64::new(0),
        }
    }

    /// Look up a cached entry by key.
    /// Returns `None` if the key is absent or the entry has expired (TTL).
    /// Expired entries are removed lazily on access.
    /// On hit, bumps the entry's access order for LRU tracking.
    pub fn get(&self, key: &str) -> Option<(String, Option<LlmUsage>)> {
        let e = self.entries.get(key)?;
        if e.stored_at.elapsed() > self.ttl {
            drop(e);
            self.entries.remove(key);
            return None;
        }
        let seq = self.access_counter.fetch_add(1, Ordering::Relaxed);
        e.access_order.store(seq, Ordering::Relaxed);
        Some((e.text.clone(), e.usage))
    }

    /// Single-flight acquire: the first caller becomes the leader;
    /// subsequent callers become waiters.
    ///
    /// The leader receives a `LeaderHandle` and MUST call `complete()`
    /// when the provider work is done (or errored).
    pub fn acquire(self: &Arc<Self>, key: &str) -> Slot {
        match self.in_flight.entry(key.to_string()) {
            Entry::Occupied(o) => Slot::Waiter {
                rx: o.get().subscribe(),
            },
            Entry::Vacant(v) => {
                let (tx, _rx) = watch::channel(None);
                v.insert(tx.clone());
                Slot::Leader(LeaderHandle {
                    key: key.to_string(),
                    cache: Arc::clone(self),
                    tx: Some(tx),
                })
            }
        }
    }

    /// Evict oldest entries if the cache exceeds `max_entries`.
    ///
    /// Uses the monotonic `access_order` counter (bumped on insert and
    /// every cache hit). Entries with the lowest sequence numbers are
    /// considered least-recently-used and removed first.
    ///
    /// `in_flight` entries are NEVER trimmed — single-flight correctness
    /// depends on them.
    fn trim(&self) {
        if let Some(max) = self.max_entries
            && self.entries.len() > max
        {
            let mut entries: Vec<(String, u64)> = self
                .entries
                .iter()
                .map(|e| {
                    (
                        e.key().clone(),
                        e.value().access_order.load(Ordering::Relaxed),
                    )
                })
                .collect();
            entries.sort_by_key(|(_, seq)| *seq);
            let to_remove = entries.len().saturating_sub(max);
            for (key, _) in entries.into_iter().take(to_remove) {
                self.entries.remove(&key);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Waiter wait function
// ---------------------------------------------------------------------------

/// Await the leader's result.
///
/// Resolves when the leader sends its result via the watch channel.
/// If the sender was dropped (leader panicked/dropped without completing),
/// returns an error.
pub async fn wait(rx: &mut FlightReceiver) -> Result<Arc<CachedEntry>, LlmError> {
    match rx.changed().await {
        Ok(()) => rx.borrow().clone().unwrap_or_else(|| {
            Err(LlmError::Provider(
                "single-flight leader dropped without result".into(),
            ))
        }),
        Err(_) => rx.borrow().clone().unwrap_or_else(|| {
            Err(LlmError::Provider(
                "single-flight leader dropped without result".into(),
            ))
        }),
    }
}

// ---------------------------------------------------------------------------
// Canonical key computation
// ---------------------------------------------------------------------------

/// Recursively sort all JSON object keys in a value tree.
/// Arrays are recursed into; leaves (string, number, bool, null) are unchanged.
/// Uses BTreeMap for deterministic ordering regardless of serde_json's
/// preserve_order feature (which uses IndexMap internally).
fn canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), canonicalize_json(v)))
                .collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(canonicalize_json).collect())
        }
        other => other.clone(),
    }
}

/// Compute a canonical cache key for a chat request.
///
/// The key includes:
/// - provider_id (to isolate caches between providers)
/// - model (after header/URI/config resolution)
/// - messages (in conversation order — NOT sorted)
/// - temperature, max_tokens, stop, system_prompt
/// - tools (sorted by name; parameters with sorted JSON object keys)
/// - tool_choice
/// - extra (sorted JSON object keys)
///
/// ALL nested JSON objects are recursively canonicalised (key-sorted),
/// so different insertion orders at any depth produce the same key.
///
/// The canonical bytes are hashed with `std::hash::DefaultHasher` and
/// returned as a hex string.
pub fn canonical_key(provider_id: &str, req: &ChatRequest) -> String {
    // Convert extra to sorted map with recursively canonicalised values.
    let extra_sorted: BTreeMap<String, serde_json::Value> = req
        .extra
        .iter()
        .map(|(k, v)| (k.clone(), canonicalize_json(v)))
        .collect();

    // Convert tools: sort by name, canonically sorted parameters
    let mut tools_canonical: Vec<serde_json::Value> = req
        .tools
        .iter()
        .map(|t| {
            let params_sorted: BTreeMap<String, serde_json::Value> = t
                .parameters
                .iter()
                .map(|(k, v)| (k.clone(), canonicalize_json(v)))
                .collect();
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "parameters": params_sorted,
            })
        })
        .collect();
    tools_canonical.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });

    let key_obj = serde_json::json!({
        "provider": provider_id,
        "model": req.model,
        "messages": req.messages,
        "temperature": req.temperature,
        "max_tokens": req.max_tokens,
        "stop": req.stop,
        "system_prompt": req.system_prompt,
        "tool_choice": req.tool_choice,
        "tools": tools_canonical,
        "extra": extra_sorted,
    });

    // Hash the canonical JSON string for a compact key.
    let json_str = key_obj.to_string();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    json_str.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ChatMessage, ToolChoice, ToolDefinition};

    #[test]
    fn canonical_key_is_deterministic() {
        let req1 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hello")]);
        let req2 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hello")]);
        let k1 = canonical_key("openai", &req1);
        let k2 = canonical_key("openai", &req2);
        assert_eq!(k1, k2, "identical requests must have identical keys");
    }

    #[test]
    fn canonical_key_differs_on_model() {
        let req1 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hello")]);
        let req2 = ChatRequest::new("gpt-4o-mini", vec![ChatMessage::user("hello")]);
        let k1 = canonical_key("openai", &req1);
        let k2 = canonical_key("openai", &req2);
        assert_ne!(k1, k2, "different models must produce different keys");
    }

    #[test]
    fn canonical_key_differs_on_messages() {
        let req1 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hello")]);
        let req2 = ChatRequest::new(
            "gpt-4o",
            vec![ChatMessage::user("first"), ChatMessage::user("second")],
        );
        let k1 = canonical_key("openai", &req1);
        let k2 = canonical_key("openai", &req2);
        assert_ne!(k1, k2, "different messages must produce different keys");
    }

    #[test]
    fn canonical_key_differs_on_tools() {
        let mut req1 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hi")]);
        req1.tools = vec![ToolDefinition {
            name: "weather".into(),
            description: "Get weather".into(),
            parameters: serde_json::Map::new(),
        }];
        let req2 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hi")]);
        let k1 = canonical_key("openai", &req1);
        let k2 = canonical_key("openai", &req2);
        assert_ne!(k1, k2, "different tools must produce different keys");
    }

    #[test]
    fn canonical_key_differs_on_provider() {
        let req = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hello")]);
        let k1 = canonical_key("openai", &req);
        let k2 = canonical_key("ollama", &req);
        assert_ne!(k1, k2, "different providers must produce different keys");
    }

    #[test]
    fn canonical_key_sorts_extra_keys() {
        let mut req1 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hi")]);
        req1.extra = {
            let mut m = serde_json::Map::new();
            m.insert("z_field".into(), serde_json::Value::Number(1.into()));
            m.insert("a_field".into(), serde_json::Value::String("val".into()));
            m
        };
        // Same extra, different insertion order — must produce same key
        let mut req2 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hi")]);
        req2.extra = {
            let mut m = serde_json::Map::new();
            m.insert("a_field".into(), serde_json::Value::String("val".into()));
            m.insert("z_field".into(), serde_json::Value::Number(1.into()));
            m
        };
        let k1 = canonical_key("openai", &req1);
        let k2 = canonical_key("openai", &req2);
        assert_eq!(
            k1, k2,
            "different insertion order of extra keys must produce same key"
        );
    }

    #[test]
    fn canonical_key_sorts_tool_parameter_keys() {
        let mut req1 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hi")]);
        req1.tools = vec![ToolDefinition {
            name: "weather".into(),
            description: "Get weather".into(),
            parameters: {
                let mut m = serde_json::Map::new();
                m.insert("location".into(), serde_json::Value::String("NYC".into()));
                m.insert("unit".into(), serde_json::Value::String("celsius".into()));
                m
            },
        }];
        // Same parameters, different insertion order
        let mut req2 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hi")]);
        req2.tools = vec![ToolDefinition {
            name: "weather".into(),
            description: "Get weather".into(),
            parameters: {
                let mut m = serde_json::Map::new();
                m.insert("unit".into(), serde_json::Value::String("celsius".into()));
                m.insert("location".into(), serde_json::Value::String("NYC".into()));
                m
            },
        }];
        let k1 = canonical_key("openai", &req1);
        let k2 = canonical_key("openai", &req2);
        assert_eq!(
            k1, k2,
            "different insertion order of tool parameter keys must produce same key"
        );
    }

    #[test]
    fn canonical_key_deeply_sorts_nested_object_keys() {
        let nested_1 = serde_json::json!({
            "b": 2,
            "a": 1,
            "nested": {
                "z": "last",
                "m": "middle",
                "a": "first",
            },
        });
        let nested_2 = serde_json::json!({
            "a": 1,
            "b": 2,
            "nested": {
                "a": "first",
                "m": "middle",
                "z": "last",
            },
        });

        let mut req1 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hi")]);
        req1.tools = vec![ToolDefinition {
            name: "weather".into(),
            description: "Get weather".into(),
            parameters: serde_json::Map::new(),
        }];
        req1.extra = nested_1.as_object().unwrap().clone();

        let mut req2 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hi")]);
        req2.tools = vec![ToolDefinition {
            name: "weather".into(),
            description: "Get weather".into(),
            parameters: serde_json::Map::new(),
        }];
        req2.extra = nested_2.as_object().unwrap().clone();

        let k1 = canonical_key("openai", &req1);
        let k2 = canonical_key("openai", &req2);
        assert_eq!(
            k1, k2,
            "nested objects with different key ordering must produce same key"
        );
    }

    #[test]
    fn canonical_key_differs_on_tool_choice() {
        let mut req1 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hi")]);
        req1.tool_choice = Some(ToolChoice::Auto);
        let mut req2 = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hi")]);
        req2.tool_choice = Some(ToolChoice::None);
        let k1 = canonical_key("openai", &req1);
        let k2 = canonical_key("openai", &req2);
        assert_ne!(k1, k2, "different tool_choice must produce different keys");
    }

    #[test]
    fn cache_get_returns_none_for_expired_entry() {
        let cache = ProducerCache::new(Duration::from_nanos(1), None);
        cache.entries.insert(
            "k".into(),
            Arc::new(CachedEntry {
                text: "v".into(),
                usage: None,
                stored_at: Instant::now() - Duration::from_secs(10),
                access_order: AtomicU64::new(0),
            }),
        );
        // Entry stored_at is 10s ago, ttl is 1ns => expired
        assert!(cache.get("k").is_none());
    }

    #[tokio::test]
    async fn single_flight_leader_then_waiter() {
        let cache = Arc::new(ProducerCache::new(Duration::from_secs(60), None));
        let key = "test-key";

        // Acquire as leader
        let slot1 = Arc::clone(&cache).acquire(key);
        let leader = match slot1 {
            Slot::Leader(h) => h,
            _ => panic!("expected Leader"),
        };

        // Acquire again as waiter
        let slot2 = Arc::clone(&cache).acquire(key);
        let mut rx = match slot2 {
            Slot::Waiter { rx } => rx,
            _ => panic!("expected Waiter"),
        };

        // Complete the leader with a result
        leader.complete(Ok(("hello".into(), None)));

        // Waiter must receive the result
        let entry = wait(&mut rx).await.expect("waiter got result");
        assert_eq!(entry.text, "hello");
    }

    #[test]
    fn lru_evicts_oldest_on_overflow() {
        let cache = ProducerCache::new(Duration::from_secs(60), Some(2));

        // Insert three entries with keys ordered by insertion.
        // We bypass the single-flight path and insert directly to
        // control the access_order sequence precisely.
        for i in 0..3 {
            let seq = cache.access_counter.fetch_add(1, Ordering::Relaxed);
            let key = format!("k{i}");
            cache.entries.insert(
                key,
                Arc::new(CachedEntry {
                    text: format!("v{i}"),
                    usage: None,
                    stored_at: Instant::now(),
                    access_order: AtomicU64::new(seq),
                }),
            );
        }

        // Three entries inserted, max is 2 => oldest must be evicted
        assert_eq!(cache.entries.len(), 3, "all 3 inserted before trim");
        cache.trim();
        assert_eq!(cache.entries.len(), 2, "trim must evict oldest entry");
        assert!(cache.entries.contains_key("k1"), "k1 must survive");
        assert!(cache.entries.contains_key("k2"), "k2 must survive");
        assert!(
            !cache.entries.contains_key("k0"),
            "k0 (oldest) must be evicted"
        );
    }

    #[test]
    fn lru_updates_access_order_on_hit() {
        let cache = ProducerCache::new(Duration::from_secs(60), Some(2));

        // Insert three entries directly, no trim yet.
        for i in 0..3 {
            let seq = cache.access_counter.fetch_add(1, Ordering::Relaxed);
            let key = format!("k{i}");
            cache.entries.insert(
                key,
                Arc::new(CachedEntry {
                    text: format!("v{i}"),
                    usage: None,
                    stored_at: Instant::now(),
                    access_order: AtomicU64::new(seq),
                }),
            );
        }

        // Access k0 — bumps its access_order above k1 and k2
        let seq = cache.access_counter.fetch_add(1, Ordering::Relaxed);
        cache
            .entries
            .get("k0")
            .unwrap()
            .access_order
            .store(seq, Ordering::Relaxed);

        // Now trim — should evict k1 (oldest access_order), keep k0 and k2
        cache.trim();
        assert_eq!(cache.entries.len(), 2, "trim must evict one entry");
        assert!(
            cache.entries.contains_key("k0"),
            "k0 (accessed) must survive"
        );
        assert!(cache.entries.contains_key("k2"), "k2 must survive");
        assert!(
            !cache.entries.contains_key("k1"),
            "k1 (oldest after k0 access) must be evicted"
        );
    }

    #[test]
    fn lru_noop_when_under_max() {
        let cache = ProducerCache::new(Duration::from_secs(60), Some(10));
        for i in 0..3 {
            let seq = cache.access_counter.fetch_add(1, Ordering::Relaxed);
            cache.entries.insert(
                format!("k{i}"),
                Arc::new(CachedEntry {
                    text: format!("v{i}"),
                    usage: None,
                    stored_at: Instant::now(),
                    access_order: AtomicU64::new(seq),
                }),
            );
        }
        // Under max => no eviction
        cache.trim();
        assert_eq!(cache.entries.len(), 3, "no eviction when under max");
    }

    #[test]
    fn lru_noop_when_max_entries_none() {
        let cache = ProducerCache::new(Duration::from_secs(60), None);
        for i in 0..10 {
            let seq = cache.access_counter.fetch_add(1, Ordering::Relaxed);
            cache.entries.insert(
                format!("k{i}"),
                Arc::new(CachedEntry {
                    text: format!("v{i}"),
                    usage: None,
                    stored_at: Instant::now(),
                    access_order: AtomicU64::new(seq),
                }),
            );
        }
        // No max_entries => trim is a no-op
        cache.trim();
        assert_eq!(
            cache.entries.len(),
            10,
            "no eviction when max_entries is None"
        );
    }

    #[tokio::test]
    async fn single_flight_leader_error_propagates_to_waiter() {
        let cache = Arc::new(ProducerCache::new(Duration::from_secs(60), None));
        let key = "err-key";

        let slot1 = Arc::clone(&cache).acquire(key);
        let leader = match slot1 {
            Slot::Leader(h) => h,
            _ => panic!("expected Leader"),
        };
        let slot2 = Arc::clone(&cache).acquire(key);
        let mut rx = match slot2 {
            Slot::Waiter { rx } => rx,
            _ => panic!("expected Waiter"),
        };

        leader.complete(Err(LlmError::Network("boom".into())));

        let err = wait(&mut rx).await.unwrap_err();
        assert!(
            err.to_string().contains("boom"),
            "waiter must receive the leader's error"
        );
    }

    #[tokio::test]
    async fn lru_safe_under_concurrent_complete() {
        let cache: Arc<ProducerCache> =
            Arc::new(ProducerCache::new(Duration::from_secs(60), Some(5)));
        let mut handles = Vec::new();
        for i in 0..20 {
            let cache = Arc::clone(&cache);
            handles.push(tokio::spawn(async move {
                let key = format!("key{i}");
                match cache.acquire(&key) {
                    Slot::Leader(h) => {
                        h.complete(Ok((format!("text{i}"), None)));
                    }
                    _ => panic!("expected leader for new key"),
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert!(
            cache.entries.len() <= 5,
            "entries should be bounded by max_entries (got {})",
            cache.entries.len()
        );
    }
}
