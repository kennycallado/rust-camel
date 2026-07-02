//! In-memory idempotent repository backed by `DashMap`.
//!
//! Uses interior mutability (DashMap) so all `&self` methods work without
//! a lock at the repository level for READS. Writes of new keys are
//! serialized by a per-instance `write_guard` mutex (see below).
//!
//! Batch 1 (H10): every repo carries a `max_entries` cap (default
//! `DEFAULT_MAX_ENTRIES = 100_000`). When the cap is reached, the OLDEST
//! inserted entry is evicted on the write path to make room for the new
//! one. Deterministic and clock-free: O(1) amortized under the cap, one
//! O(n) scan per insert-at-cap to find the oldest entry.
//!
//! # Semantics: bounded at-most-once
//!
//! Eviction trades at-most-once strength for a memory bound: once a key
//! is evicted at cap, a duplicate of that key is re-admitted as new and
//! would be REPROCESSED. Under a unique-key flood larger than
//! `max_entries`, the oldest keys lose dedup protection first. Upgrade
//! path for stronger guarantees: TTL-based or persistent repository.
//!
//! # Concurrency
//!
//! The cap check + evict + insert sequence is not atomic on a bare
//! DashMap. `add` holds `write_guard` across the critical section (no
//! `.await` inside) so the `len <= max_entries` invariant holds under
//! concurrent writers. Reads stay lock-free.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use camel_api::{CamelError, IdempotentRepository};
use dashmap::DashMap;

/// Default cap on the number of keys the repo may hold. A malicious
/// caller flooding unique keys can grow the map up to this size before
/// the write path begins evicting the oldest entries. 100_000 is large
/// enough to be invisible to legitimate workloads and small enough to
/// bound memory to a few MB at most. Tunable via `new_with_max_entries`.
pub const DEFAULT_MAX_ENTRIES: usize = 100_000;

#[derive(Debug)]
struct Entry {
    /// Monotonically increasing insertion-order counter. Lower = older.
    seq: u64,
}

#[derive(Debug)]
pub struct MemoryIdempotentRepository {
    name: String,
    keys: DashMap<String, Entry>,
    max_entries: usize,
    /// Per-instance monotonic insertion-order counter. `fetch_add` with
    /// `Relaxed` is enough: writers are serialized by `write_guard`, and
    /// u64 wrap (2^64 inserts) is unreachable in practice.
    next_seq: AtomicU64,
    /// Serializes the write path (cap check + evict + insert) so the
    /// `len <= max_entries` invariant holds under concurrent writers.
    /// The `()` payload cannot be corrupted, so a poisoned guard is
    /// recovered with `into_inner()` — no panic, no `.expect(`.
    write_guard: Mutex<()>,
}

impl MemoryIdempotentRepository {
    /// Create a new repository with the diagnostic name and the default
    /// cap (`DEFAULT_MAX_ENTRIES`). Production-safe.
    pub fn new(name: impl Into<String>) -> Self {
        Self::new_with_max_entries(name, DEFAULT_MAX_ENTRIES)
    }

    /// Create a new repository with a custom cap. Operators may tune the
    /// cap per deployment. A cap of 0 would deny every `add`; the cap is
    /// validated at construction (panics for 0 — programmer error, the
    /// production `new` path never passes 0).
    pub fn new_with_max_entries(name: impl Into<String>, max_entries: usize) -> Self {
        assert!(max_entries > 0, "max_entries must be > 0");
        Self {
            name: name.into(),
            keys: DashMap::new(),
            max_entries,
            next_seq: AtomicU64::new(0),
            write_guard: Mutex::new(()),
        }
    }

    /// Current cap. Public so callers (and tests) can inspect it.
    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    /// Current key count.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// True if no keys are stored.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

#[async_trait::async_trait]
impl IdempotentRepository for MemoryIdempotentRepository {
    fn name(&self) -> &str {
        &self.name
    }

    async fn contains(&self, key: &str) -> Result<bool, CamelError> {
        Ok(self.keys.contains_key(key))
    }

    async fn add(&self, key: &str) -> Result<bool, CamelError> {
        // H10 Batch 1: enforce the cap. The whole check→evict→insert
        // sequence runs under `write_guard` so concurrent adders cannot
        // overshoot the cap (non-atomic check-then-act on a bare
        // DashMap). No `.await` while the guard is held.
        let _w = self.write_guard.lock().unwrap_or_else(|p| p.into_inner());
        if !self.keys.contains_key(key) && self.keys.len() >= self.max_entries {
            // Find the entry with the smallest seq. The temporary shard
            // guards from `iter()` die at the end of this statement —
            // `remove` below runs with no shard lock held.
            let oldest = self
                .keys
                .iter()
                .min_by_key(|e| e.value().seq)
                .map(|e| e.key().clone());
            if let Some(k) = oldest {
                self.keys.remove(&k);
            }
        }
        // `insert` returns `None` if the key was not present (newly inserted),
        // `Some(_)` if the key already existed.
        let prior = self.keys.insert(
            key.to_string(),
            Entry {
                seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
            },
        );
        Ok(prior.is_none())
    }

    async fn remove(&self, key: &str) -> Result<(), CamelError> {
        // Membership only shrinks — cannot violate the cap; no guard.
        self.keys.remove(key);
        Ok(())
    }

    async fn clear(&self) -> Result<(), CamelError> {
        self.keys.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_repo() -> MemoryIdempotentRepository {
        MemoryIdempotentRepository::new("test")
    }

    #[tokio::test]
    async fn add_then_contains() {
        let repo = new_repo();
        repo.add("key-1").await.unwrap();
        assert!(repo.contains("key-1").await.unwrap());
    }

    #[tokio::test]
    async fn second_add_returns_false() {
        let repo = new_repo();
        assert!(repo.add("key-1").await.unwrap(), "first add should be true");
        assert!(
            !repo.add("key-1").await.unwrap(),
            "second add should be false"
        );
    }

    #[tokio::test]
    async fn remove_then_contains_false() {
        let repo = new_repo();
        repo.add("key-1").await.unwrap();
        repo.remove("key-1").await.unwrap();
        assert!(!repo.contains("key-1").await.unwrap());
    }

    #[tokio::test]
    async fn clear_removes_all() {
        let repo = new_repo();
        repo.add("a").await.unwrap();
        repo.add("b").await.unwrap();
        repo.clear().await.unwrap();
        assert!(!repo.contains("a").await.unwrap());
        assert!(!repo.contains("b").await.unwrap());
    }

    #[tokio::test]
    async fn contains_returns_false_for_missing_key() {
        let repo = new_repo();
        assert!(!repo.contains("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn remove_nonexistent_is_ok() {
        let repo = new_repo();
        // Removing a key that was never added should be a no-op success.
        repo.remove("never-added").await.unwrap();
    }

    #[test]
    fn name_returns_configured_name() {
        let repo = MemoryIdempotentRepository::new("my-cache");
        assert_eq!(repo.name(), "my-cache");
    }

    // ── H10 Batch 1: max_entries cap (evict-oldest) on in-mem idempotent repo ──

    /// H10: when the repo is at `max_entries`, the next `add` with a new
    /// key evicts the OLDEST entry (insertion-order) and inserts the new
    /// one. The map size stays `<= max_entries` forever. This bounds a
    /// write-only unique-key flood without a clock (O(1) amortized under
    /// the cap; one O(n) scan per insert-at-cap).
    ///
    /// Determinism: real time, no `start_paused`, no `Instant` math.
    #[tokio::test]
    async fn test_idempotent_repo_caps_at_max_entries_evicting_oldest() {
        let repo = MemoryIdempotentRepository::new_with_max_entries("test", 4);
        for i in 0..4 {
            repo.add(&format!("k{i}")).await.unwrap();
        }
        assert_eq!(repo.len(), 4);
        // The first inserted is "k0"; it MUST be evicted by the 5th add.
        repo.add("k4").await.unwrap();
        assert_eq!(repo.len(), 4, "cap must be enforced");
        assert!(
            !repo.contains("k0").await.unwrap(),
            "oldest (k0) must be evicted"
        );
        // The remaining 4 are the most recently inserted.
        for i in 1..=4 {
            assert!(
                repo.contains(&format!("k{i}")).await.unwrap(),
                "k{i} must be present after eviction"
            );
        }
    }

    /// H10 semantics ceiling (documented trade-off, review fix I-2):
    /// eviction opens a DUPLICATE window. Once a key is evicted at cap,
    /// re-`add`ing it succeeds as if new — that exchange would be
    /// REPROCESSED. The cap bounds memory, NOT the at-most-once
    /// guarantee, under a >cap unique-key flood. Upgrade path for
    /// stronger dedup: persistent repository (out of Batch 1 scope).
    #[tokio::test]
    async fn test_idempotent_evicted_key_readmitted_as_new() {
        let repo = MemoryIdempotentRepository::new_with_max_entries("test", 2);
        repo.add("k0").await.unwrap();
        repo.add("k1").await.unwrap();
        repo.add("k2").await.unwrap(); // evicts k0
        assert!(!repo.contains("k0").await.unwrap(), "k0 must be evicted");
        // The duplicate window: the evicted key re-admits as "new".
        assert!(
            repo.add("k0").await.unwrap(),
            "evicted key must re-admit as new (documented at-most-once ceiling)"
        );
    }

    /// The default `new` constructor uses the safe production cap
    /// (`DEFAULT_MAX_ENTRIES = 100_000`). Operators tune it via
    /// `new_with_max_entries(name, cap)`.
    #[test]
    fn test_idempotent_repo_default_max_entries_is_100_000() {
        let repo = MemoryIdempotentRepository::new("test");
        assert_eq!(repo.max_entries(), DEFAULT_MAX_ENTRIES);
    }

    /// `max_entries` is plumbed through `new_with_max_entries`.
    #[test]
    fn test_idempotent_repo_new_with_max_entries_sets_cap() {
        let repo = MemoryIdempotentRepository::new_with_max_entries("test", 7);
        assert_eq!(repo.max_entries(), 7);
    }
}
