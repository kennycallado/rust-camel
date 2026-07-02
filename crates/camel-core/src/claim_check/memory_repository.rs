//! In-memory claim check repository backed by `DashMap`.
//!
//! Batch 1 (H11): every repo carries a `max_entries` cap (default
//! `DEFAULT_MAX_ENTRIES = 100_000`). When the cap is reached on a write,
//! the OLDEST inserted entry is evicted on the write path. Deterministic
//! and clock-free: O(1) amortized under the cap, one O(n) scan per
//! insert-at-cap.
//!
//! # Concurrency
//!
//! The cap check + evict + insert sequence is not atomic on a bare
//! DashMap. `set` / `push` hold `write_guard` across the critical
//! section (no `.await` inside) so the `len <= max_entries` invariant
//! holds under concurrent writers. Reads stay lock-free.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use camel_api::CamelError;
use camel_api::ClaimCheckRepository;
use camel_api::message::Message;
use dashmap::DashMap;

/// Default cap on the number of keys / stacks the repo may hold.
/// See `MemoryIdempotentRepository::DEFAULT_MAX_ENTRIES` for the rationale.
pub const DEFAULT_MAX_ENTRIES: usize = 100_000;

#[derive(Debug)]
struct SingleEntry {
    payload: Message,
    /// Monotonically increasing insertion-order counter. Lower = older.
    seq: u64,
}

#[derive(Debug)]
struct StackEntry {
    payloads: VecDeque<Message>,
    /// Monotonically increasing insertion-order counter for the stack.
    seq: u64,
}

#[derive(Debug)]
pub struct MemoryClaimCheckRepository {
    name: String,
    keys: DashMap<String, SingleEntry>,
    stacks: DashMap<String, StackEntry>,
    max_entries: usize,
    /// Per-instance monotonic insertion-order counter (shared by `keys`
    /// and `stacks` — ordering only needs to be comparable within each
    /// map). `Relaxed` is enough: writers are serialized by `write_guard`.
    next_seq: AtomicU64,
    /// Serializes new-key writes (cap check + evict + insert) on BOTH
    /// maps. The `()` payload cannot be corrupted, so a poisoned guard
    /// is recovered with `into_inner()` — no panic, no `.expect(`.
    write_guard: Mutex<()>,
}

impl MemoryClaimCheckRepository {
    /// Create a new repository with the diagnostic name and the default
    /// cap (`DEFAULT_MAX_ENTRIES`). Production-safe.
    pub fn new(name: impl Into<String>) -> Self {
        Self::new_with_max_entries(name, DEFAULT_MAX_ENTRIES)
    }

    /// Create a new repository with a custom cap. Operators may tune the
    /// cap per deployment. A cap of 0 would deny every `set` / `push`;
    /// the cap is validated at construction (panics for 0).
    pub fn new_with_max_entries(name: impl Into<String>, max_entries: usize) -> Self {
        assert!(max_entries > 0, "max_entries must be > 0");
        Self {
            name: name.into(),
            keys: DashMap::new(),
            stacks: DashMap::new(),
            max_entries,
            next_seq: AtomicU64::new(0),
            write_guard: Mutex::new(()),
        }
    }

    /// Current cap. Public so callers (and tests) can inspect it.
    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    /// Current size of the `keys` (single-value) map.
    pub fn keys_len(&self) -> usize {
        self.keys.len()
    }

    /// Current size of the `stacks` (LIFO) map.
    pub fn stacks_len(&self) -> usize {
        self.stacks.len()
    }
}

#[async_trait::async_trait]
impl ClaimCheckRepository for MemoryClaimCheckRepository {
    fn name(&self) -> &str {
        &self.name
    }

    async fn set(&self, key: &str, payload: Message) -> Result<(), CamelError> {
        // Evict oldest on cap if this is a NEW key. Whole sequence under
        // `write_guard` — see module doc # Concurrency.
        let _w = self.write_guard.lock().unwrap_or_else(|p| p.into_inner());
        if !self.keys.contains_key(key) && self.keys.len() >= self.max_entries {
            // Shard guards from `iter()` die at the end of this
            // statement — `remove` runs with no shard lock held.
            let oldest = self
                .keys
                .iter()
                .min_by_key(|e| e.value().seq)
                .map(|e| e.key().clone());
            if let Some(k) = oldest {
                self.keys.remove(&k);
            }
        }
        self.keys.insert(
            key.to_string(),
            SingleEntry {
                payload,
                seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
            },
        );
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Message, CamelError> {
        self.keys
            .get(key)
            .map(|r| r.payload.clone())
            .ok_or_else(|| CamelError::RouteError(format!("Claim check key not found: {key}")))
    }

    async fn get_and_remove(&self, key: &str) -> Result<Message, CamelError> {
        self.keys
            .remove(key)
            .map(|(_, v)| v.payload)
            .ok_or_else(|| CamelError::RouteError(format!("Claim check key not found: {key}")))
    }

    async fn remove(&self, key: &str) -> Result<(), CamelError> {
        // Membership only shrinks — cannot violate the cap; no guard.
        self.keys.remove(key);
        Ok(())
    }

    async fn push(&self, key: &str, payload: Message) -> Result<(), CamelError> {
        // Evict oldest on cap if this is a NEW stack key. Whole sequence
        // under `write_guard` — see module doc # Concurrency.
        let _w = self.write_guard.lock().unwrap_or_else(|p| p.into_inner());
        if !self.stacks.contains_key(key) && self.stacks.len() >= self.max_entries {
            let oldest = self
                .stacks
                .iter()
                .min_by_key(|e| e.value().seq)
                .map(|e| e.key().clone());
            if let Some(k) = oldest {
                self.stacks.remove(&k);
            }
        }
        self.stacks
            .entry(key.to_string())
            .or_insert_with(|| StackEntry {
                payloads: VecDeque::new(),
                seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
            })
            .payloads
            .push_back(payload);
        Ok(())
    }

    async fn pop(&self, key: &str) -> Result<Message, CamelError> {
        // Mutates an existing stack in place — no membership change, no
        // cap impact, no guard needed.
        self.stacks
            .get_mut(key)
            .and_then(|mut entry| entry.payloads.pop_back())
            .ok_or_else(|| {
                CamelError::RouteError(format!("Claim check stack empty for key: {key}"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::body::Body;

    fn new_repo() -> MemoryClaimCheckRepository {
        MemoryClaimCheckRepository::new("test")
    }

    #[tokio::test]
    async fn set_then_get() {
        let repo = new_repo();
        let msg = Message::new(Body::Text("hello".to_string()));
        repo.set("key-1", msg.clone()).await.unwrap();
        let result = repo.get("key-1").await.unwrap();
        assert_eq!(result.body, Body::Text("hello".to_string()));
    }

    #[tokio::test]
    async fn get_and_remove_removes() {
        let repo = new_repo();
        let payload = Message::new(Body::Text("will-be-removed".to_string()));
        repo.set("key-1", payload.clone()).await.unwrap();

        let retrieved = repo.get_and_remove("key-1").await.unwrap();
        assert_eq!(retrieved.body, Body::Text("will-be-removed".to_string()));

        // Second get should fail
        let err = repo.get("key-1").await.unwrap_err();
        assert!(
            matches!(&err, CamelError::RouteError(msg) if msg.contains("not found")),
            "expected RouteError with 'not found', got: {err:?}"
        );
    }

    #[tokio::test]
    async fn push_pop_lifo() {
        let repo = new_repo();
        let first = Message::new(Body::Text("first".to_string()));
        let second = Message::new(Body::Text("second".to_string()));

        repo.push("stack-1", first).await.unwrap();
        repo.push("stack-1", second.clone()).await.unwrap();

        // Pop should return the last pushed (LIFO)
        let popped = repo.pop("stack-1").await.unwrap();
        assert_eq!(popped.body, Body::Text("second".to_string()));

        let popped = repo.pop("stack-1").await.unwrap();
        assert_eq!(popped.body, Body::Text("first".to_string()));
    }

    #[tokio::test]
    async fn get_missing_returns_err() {
        let repo = new_repo();
        let err = repo.get("nonexistent").await.unwrap_err();
        assert!(
            matches!(&err, CamelError::RouteError(msg) if msg.contains("not found")),
            "expected RouteError with 'not found', got: {err:?}"
        );
    }

    #[tokio::test]
    async fn pop_empty_stack_returns_err() {
        let repo = new_repo();
        let err = repo.pop("empty-stack").await.unwrap_err();
        assert!(
            matches!(&err, CamelError::RouteError(msg) if msg.contains("empty")),
            "expected RouteError with 'empty', got: {err:?}"
        );
    }

    #[tokio::test]
    async fn remove_nonexistent_is_ok() {
        let repo = new_repo();
        repo.remove("never-set").await.unwrap();
    }

    #[tokio::test]
    async fn set_overwrites() {
        let repo = new_repo();
        repo.set("k", Message::new(Body::Text("old".into())))
            .await
            .unwrap();
        repo.set("k", Message::new(Body::Text("new".into())))
            .await
            .unwrap();
        let body = repo.get("k").await.unwrap();
        assert_eq!(body.body, Body::Text("new".into()));
    }

    #[test]
    fn name_returns_configured_name() {
        let repo = MemoryClaimCheckRepository::new("my-check");
        assert_eq!(repo.name(), "my-check");
    }

    // ── H11 Batch 1: max_entries cap (evict-oldest) on in-mem claim-check repo ──

    /// H11: when the `keys` map is at `max_entries`, the next `set` with
    /// a new key evicts the oldest entry. The map size stays `<=
    /// max_entries` forever. This bounds a write-only unique-key flood
    /// without a clock (O(1) amortized under the cap; one O(n) scan per
    /// insert-at-cap).
    #[tokio::test]
    async fn test_claim_check_repo_caps_keys_at_max_entries_evicting_oldest() {
        let repo = MemoryClaimCheckRepository::new_with_max_entries("test", 3);
        for i in 0..3 {
            repo.set(&format!("k{i}"), Message::new(Body::Text(format!("v{i}"))))
                .await
                .unwrap();
        }
        assert_eq!(repo.keys_len(), 3);
        // 4th distinct key — the oldest (k0) must be evicted.
        repo.set("k3", Message::new(Body::Text("v3".to_string())))
            .await
            .unwrap();
        assert_eq!(repo.keys_len(), 3, "cap must be enforced on the keys map");
        // k0 was the oldest; it must be gone.
        let err = repo.get("k0").await.unwrap_err();
        assert!(matches!(err, CamelError::RouteError(_)));
        // k1, k2, k3 remain.
        for k in ["k1", "k2", "k3"] {
            assert!(repo.get(k).await.is_ok(), "{k} must be present");
        }
    }

    /// `push` is also subject to the cap. When the stacks map is at
    /// `max_entries`, the next `push` with a new key evicts the oldest
    /// stack.
    #[tokio::test]
    async fn test_claim_check_repo_caps_stacks_at_max_entries_evicting_oldest() {
        let repo = MemoryClaimCheckRepository::new_with_max_entries("test", 2);
        repo.push("s0", Message::new(Body::Text("a".into())))
            .await
            .unwrap();
        repo.push("s1", Message::new(Body::Text("b".into())))
            .await
            .unwrap();
        assert_eq!(repo.stacks_len(), 2);
        // 3rd distinct stack key — the oldest (s0) is evicted.
        repo.push("s2", Message::new(Body::Text("c".into())))
            .await
            .unwrap();
        assert_eq!(
            repo.stacks_len(),
            2,
            "cap must be enforced on the stacks map"
        );
        // s0 evicted; s1 and s2 remain.
        assert!(repo.pop("s0").await.is_err());
        assert!(repo.pop("s1").await.is_ok());
        assert!(repo.pop("s2").await.is_ok());
    }

    /// The default `new` constructor uses the safe production cap
    /// (`DEFAULT_MAX_ENTRIES = 100_000`).
    #[test]
    fn test_claim_check_repo_default_max_entries_is_100_000() {
        let repo = MemoryClaimCheckRepository::new("test");
        assert_eq!(repo.max_entries(), DEFAULT_MAX_ENTRIES);
    }
}
