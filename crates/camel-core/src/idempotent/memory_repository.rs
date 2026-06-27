//! In-memory idempotent repository backed by `DashMap`.
//!
//! Uses interior mutability (DashMap) so all `&self` methods work without
//! a lock at the repository level. This is safe for concurrent access from
//! multiple pipeline steps.

use camel_api::{CamelError, IdempotentRepository};
use dashmap::DashMap;

/// In-memory idempotent repository backed by `DashMap<String, ()>`.
///
/// # Thread safety
///
/// `DashMap` provides concurrent read/write access. All trait methods take
/// `&self`, so the repository can be shared via `Arc<dyn IdempotentRepository>`.
#[derive(Debug)]
pub struct MemoryIdempotentRepository {
    name: String,
    keys: DashMap<String, ()>,
}

impl MemoryIdempotentRepository {
    /// Create a new repository with the given diagnostic name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            keys: DashMap::new(),
        }
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
        // `insert` returns `None` if the key was not present (newly inserted),
        // `Some(())` if the key already existed.
        let existed = self.keys.insert(key.to_string(), ()).is_some();
        Ok(!existed)
    }

    async fn remove(&self, key: &str) -> Result<(), CamelError> {
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
}
