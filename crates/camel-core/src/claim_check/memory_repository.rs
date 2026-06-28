//! In-memory claim check repository backed by `DashMap`.
//!
//! Uses interior mutability (DashMap) so all `&self` methods work without
//! a lock at the repository level. This is safe for concurrent access from
//! multiple pipeline steps.

use camel_api::CamelError;
use camel_api::ClaimCheckRepository;
use camel_api::message::Message;
use dashmap::DashMap;
use std::collections::VecDeque;

/// In-memory claim check repository backed by `DashMap<String, Message>` for
/// single-value keys and `DashMap<String, VecDeque<Message>>` for LIFO stacks.
///
/// # Thread safety
///
/// `DashMap` provides concurrent read/write access. All trait methods take
/// `&self`, so the repository can be shared via `Arc<dyn ClaimCheckRepository>`.
#[derive(Debug)]
pub struct MemoryClaimCheckRepository {
    name: String,
    /// Single-value keys (set/get/get_and_remove/remove).
    keys: DashMap<String, Message>,
    /// Stack keys for push/pop.
    stacks: DashMap<String, VecDeque<Message>>,
}

impl MemoryClaimCheckRepository {
    /// Create a new repository with the given diagnostic name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            keys: DashMap::new(),
            stacks: DashMap::new(),
        }
    }
}

#[async_trait::async_trait]
impl ClaimCheckRepository for MemoryClaimCheckRepository {
    fn name(&self) -> &str {
        &self.name
    }

    async fn set(&self, key: &str, payload: Message) -> Result<(), CamelError> {
        self.keys.insert(key.to_string(), payload);
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Message, CamelError> {
        self.keys
            .get(key)
            .map(|r| r.clone())
            .ok_or_else(|| CamelError::RouteError(format!("Claim check key not found: {key}")))
    }

    async fn get_and_remove(&self, key: &str) -> Result<Message, CamelError> {
        self.keys
            .remove(key)
            .map(|(_, v)| v)
            .ok_or_else(|| CamelError::RouteError(format!("Claim check key not found: {key}")))
    }

    async fn remove(&self, key: &str) -> Result<(), CamelError> {
        self.keys.remove(key);
        Ok(())
    }

    async fn push(&self, key: &str, payload: Message) -> Result<(), CamelError> {
        self.stacks
            .entry(key.to_string())
            .or_default()
            .push_back(payload);
        Ok(())
    }

    async fn pop(&self, key: &str) -> Result<Message, CamelError> {
        self.stacks
            .entry(key.to_string())
            .or_default()
            .pop_back()
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
}
