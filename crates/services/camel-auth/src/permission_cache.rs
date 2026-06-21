//! Caching wrapper for [`PermissionEvaluator`] with separate positive/negative TTLs.
//!
//! Mirrors [`CachingTokenIntrospector`](crate::introspection::CachingTokenIntrospector):
//! `RwLock<HashMap>` for reads, `Mutex<()>` to prevent thundering-herd stampedes,
//! and lazy eviction when the cache exceeds capacity.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock};

use crate::permission::{PermissionDecision, PermissionEvaluator, PermissionRequest};
use crate::types::AuthError;

/// Configuration for [`CachingPermissionEvaluator`].
#[derive(Debug, Clone)]
pub struct PermissionCacheOptions {
    /// TTL for granted decisions. Default 30 s — shorter than token introspection (60 s)
    /// because authorization decisions can change faster than identity claims.
    pub positive_ttl: Duration,
    /// TTL for denied decisions. Default 5 s — allows quick recovery after permissions are granted.
    pub negative_ttl: Duration,
    /// Maximum number of cache entries before eviction kicks in.
    pub max_entries: usize,
}

impl Default for PermissionCacheOptions {
    fn default() -> Self {
        Self {
            positive_ttl: Duration::from_secs(30),
            negative_ttl: Duration::from_secs(5),
            max_entries: 10_000,
        }
    }
}

struct CachedPermissionEntry {
    decision: PermissionDecision,
    inserted_at: Instant,
}

/// Generic caching wrapper around any [`PermissionEvaluator`].
///
/// Uses SHA-256 over the canonicalised request fields (null-byte separated) as
/// the cache key, so no sensitive principal data is stored verbatim.
pub struct CachingPermissionEvaluator {
    inner: Arc<dyn PermissionEvaluator>,
    cache: Arc<RwLock<HashMap<String, CachedPermissionEntry>>>,
    in_flight: Mutex<()>,
    options: PermissionCacheOptions,
}

impl CachingPermissionEvaluator {
    pub fn new(inner: Arc<dyn PermissionEvaluator>, options: PermissionCacheOptions) -> Self {
        Self {
            inner,
            cache: Arc::new(RwLock::new(HashMap::new())),
            in_flight: Mutex::new(()),
            options,
        }
    }

    /// Deterministic SHA-256 cache key derived from all request fields.
    ///
    /// Each field is separated by a `\x00` null byte so that `"ab" + "c"` and
    /// `"a" + "bc"` cannot collide. Scopes are hashed in order with their own
    /// separators. The JSON `context` is canonicalised (object keys sorted
    /// recursively via BTreeMap) before serialisation so that semantically
    /// equivalent inputs `{"b":2,"a":1}` and `{"a":1,"b":2}` produce the same
    /// key regardless of serde_json's `preserve_order` feature being enabled.
    fn cache_key(request: &PermissionRequest) -> String {
        let mut hasher = Sha256::new();
        hasher.update(request.principal.subject.as_bytes());
        hasher.update(b"\x00");
        hasher.update(request.principal.issuer.as_bytes());
        hasher.update(b"\x00");
        hasher.update(request.resource.as_bytes());
        hasher.update(b"\x00");
        hasher.update(request.action.as_bytes());
        hasher.update(b"\x00");
        for s in &request.requested_scopes {
            hasher.update(s.as_bytes());
            hasher.update(b"\x00");
        }
        let canonical = canonicalize_json(&request.context);
        let context_str = serde_json::to_string(&canonical).unwrap_or_default();
        hasher.update(context_str.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Return the TTL that applies to a given decision.
    fn ttl_for(&self, decision: &PermissionDecision) -> Duration {
        match decision {
            PermissionDecision::Granted => self.options.positive_ttl,
            PermissionDecision::Denied { .. } => self.options.negative_ttl,
        }
    }

    async fn evict_if_needed(&self) {
        let mut cache = self.cache.write().await;
        if cache.len() < self.options.max_entries {
            return;
        }
        let now = Instant::now();
        // First pass: remove expired entries.
        cache.retain(|_, entry| {
            let ttl = match &entry.decision {
                PermissionDecision::Granted => self.options.positive_ttl,
                PermissionDecision::Denied { .. } => self.options.negative_ttl,
            };
            now.duration_since(entry.inserted_at) < ttl
        });
        // Second pass: if still over capacity, evict the oldest entry.
        if cache.len() >= self.options.max_entries {
            let oldest_key = cache
                .iter()
                .min_by_key(|(_, e)| e.inserted_at)
                .map(|(k, _)| k.clone());
            if let Some(key) = oldest_key {
                cache.remove(&key);
            }
        }
    }
}

impl fmt::Debug for CachingPermissionEvaluator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachingPermissionEvaluator")
            .field("positive_ttl", &self.options.positive_ttl)
            .field("negative_ttl", &self.options.negative_ttl)
            .field("max_entries", &self.options.max_entries)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl PermissionEvaluator for CachingPermissionEvaluator {
    async fn evaluate(&self, request: PermissionRequest) -> Result<PermissionDecision, AuthError> {
        let key = Self::cache_key(&request);
        let now = Instant::now();

        // 1. Fast-path: read cache, check TTL based on decision type.
        {
            let cache = self.cache.read().await;
            if let Some(entry) = cache.get(&key) {
                let ttl = self.ttl_for(&entry.decision);
                if now.duration_since(entry.inserted_at) < ttl {
                    tracing::debug!(target: "camel_auth::permission_cache", cache_outcome = "hit");
                    return Ok(entry.decision.clone());
                }
            }
        }

        // 2. Acquire in-flight lock → double-check → call inner.
        let _guard = self.in_flight.lock().await;

        {
            let cache = self.cache.read().await;
            if let Some(entry) = cache.get(&key) {
                let ttl = self.ttl_for(&entry.decision);
                if now.duration_since(entry.inserted_at) < ttl {
                    tracing::debug!(target: "camel_auth::permission_cache", cache_outcome = "hit_after_wait");
                    return Ok(entry.decision.clone());
                }
            }
        }

        tracing::debug!(target: "camel_auth::permission_cache", cache_outcome = "miss");

        let decision = self.inner.evaluate(request).await?;

        // 3. Lazy eviction.
        self.evict_if_needed().await;

        // 4. Insert.
        {
            let mut cache = self.cache.write().await;
            cache.insert(
                key,
                CachedPermissionEntry {
                    decision: decision.clone(),
                    inserted_at: Instant::now(),
                },
            );
        }

        Ok(decision)
    }
}

/// Recursively sort all JSON object keys in a value tree.
///
/// Arrays are recursed into; leaves (string, number, bool, null) are unchanged.
/// Uses BTreeMap for deterministic ordering regardless of serde_json's
/// `preserve_order` feature being enabled workspace-wide.
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

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::security_policy::Principal;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_principal() -> Principal {
        Principal {
            subject: "alice".into(),
            issuer: "https://keycloak.example.com/realms/test".into(),
            audience: vec!["camel-api".into()],
            roles: vec!["admin".into()],
            scopes: vec!["read".into()],
            claims: json!({}),
        }
    }

    fn test_request(resource: &str, context: serde_json::Value) -> PermissionRequest {
        PermissionRequest {
            principal: test_principal(),
            resource: resource.into(),
            action: "read".into(),
            requested_scopes: vec!["read".into()],
            context,
        }
    }

    struct CountingEvaluator {
        count: AtomicUsize,
        decision: PermissionDecision,
    }

    #[async_trait]
    impl PermissionEvaluator for CountingEvaluator {
        async fn evaluate(
            &self,
            _request: PermissionRequest,
        ) -> Result<PermissionDecision, AuthError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(self.decision.clone())
        }
    }

    fn default_opts() -> PermissionCacheOptions {
        PermissionCacheOptions {
            positive_ttl: Duration::from_secs(30),
            negative_ttl: Duration::from_secs(5),
            max_entries: 10_000,
        }
    }

    #[tokio::test]
    async fn cache_hit_avoids_repeated_call() {
        let inner = Arc::new(CountingEvaluator {
            count: AtomicUsize::new(0),
            decision: PermissionDecision::Granted,
        });
        let caching = CachingPermissionEvaluator::new(inner.clone(), default_opts());

        let req = test_request("/orders/123", json!({}));
        let d1 = caching.evaluate(req.clone()).await.unwrap();
        let d2 = caching.evaluate(req.clone()).await.unwrap();

        assert_eq!(d1, PermissionDecision::Granted);
        assert_eq!(d2, PermissionDecision::Granted);
        assert_eq!(inner.count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cache_negative_ttl_shorter() {
        let inner = Arc::new(CountingEvaluator {
            count: AtomicUsize::new(0),
            decision: PermissionDecision::Denied {
                reason: "forbidden".into(),
            },
        });
        let opts = PermissionCacheOptions {
            positive_ttl: Duration::from_secs(30),
            negative_ttl: Duration::from_millis(50),
            max_entries: 10_000,
        };
        let caching = CachingPermissionEvaluator::new(inner.clone(), opts);

        let req = test_request("/secret", json!({}));
        let d1 = caching.evaluate(req.clone()).await.unwrap();
        assert!(matches!(d1, PermissionDecision::Denied { .. }));

        tokio::time::sleep(Duration::from_millis(100)).await;

        let d2 = caching.evaluate(req.clone()).await.unwrap();
        assert!(matches!(d2, PermissionDecision::Denied { .. }));

        // Inner evaluator was called twice — once initially, once after negative TTL expired.
        assert_eq!(inner.count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cache_key_is_deterministic() {
        let req = test_request("/orders/123", json!({"source": "api"}));
        let key1 = CachingPermissionEvaluator::cache_key(&req);
        let key2 = CachingPermissionEvaluator::cache_key(&req);
        assert_eq!(key1, key2, "same request must produce the same key");
        assert_eq!(key1.len(), 64, "SHA-256 hex digest is 64 chars");
    }

    #[test]
    fn cache_key_differs_for_different_resources() {
        let req_a = test_request("/orders/123", json!({}));
        let req_b = test_request("/orders/456", json!({}));
        let key_a = CachingPermissionEvaluator::cache_key(&req_a);
        let key_b = CachingPermissionEvaluator::cache_key(&req_b);
        assert_ne!(
            key_a, key_b,
            "different resources must produce different keys"
        );
    }

    #[test]
    fn cache_key_stable_for_json_context_with_same_semantics() {
        // serde_json serialises maps with sorted keys, so {"b":"2","a":"1"} and
        // {"a":"1","b":"2"} must produce identical cache keys.
        let req_a = test_request("/orders", json!({"b":"2","a":"1"}));
        let req_b = test_request("/orders", json!({"a":"1","b":"2"}));
        let key_a = CachingPermissionEvaluator::cache_key(&req_a);
        let key_b = CachingPermissionEvaluator::cache_key(&req_b);
        assert_eq!(
            key_a, key_b,
            "semantically equivalent JSON contexts must produce the same key"
        );
    }

    #[test]
    fn options_default_values() {
        let opts = PermissionCacheOptions::default();
        assert_eq!(opts.positive_ttl, Duration::from_secs(30));
        assert_eq!(opts.negative_ttl, Duration::from_secs(5));
        assert_eq!(opts.max_entries, 10_000);
    }

    #[test]
    fn debug_does_not_leak_inner_state() {
        let inner = Arc::new(CountingEvaluator {
            count: AtomicUsize::new(0),
            decision: PermissionDecision::Granted,
        });
        let caching = CachingPermissionEvaluator::new(inner, default_opts());
        let debug = format!("{caching:?}");
        assert!(debug.contains("CachingPermissionEvaluator"));
        assert!(debug.contains("positive_ttl"));
        assert!(debug.contains("negative_ttl"));
    }
}
