//! ClaimCheckRepository trait — pluggable payload store for Claim Check EIP.
//!
//! Contract C1: `set()` and `get()` work with `Message` payloads (NOT key-only
//! like `IdempotentRepository`). The repository owns the payload until
//! `remove()` or `get_and_remove()` is called.

use crate::CamelError;
use crate::message::Message;

/// Pluggable payload store for the Claim Check EIP.
///
/// Stashes large message payloads by key so the Exchange carries only a
/// lightweight reference (the key). Supports single-value keys and LIFO
/// stacks (for push/pop operations).
///
/// # Contract (C1)
///
/// - `set` stores or overwrites a payload by key.
/// - `get` returns a clone of the payload without removing it. Returns
///   `Err(CamelError::NotFound(...))` if the key does not exist.
/// - `get_and_remove` returns and removes in one atomic step.
/// - `remove` succeeds even if the key does not exist (no-op).
/// - `push` appends to a LIFO stack for the given key.
/// - `pop` removes and returns the top of the LIFO stack. Returns
///   `Err(CamelError::NotFound(...))` if the stack is empty.
#[async_trait::async_trait]
pub trait ClaimCheckRepository: Send + Sync + std::fmt::Debug + 'static {
    /// Human-readable name for diagnostics.
    fn name(&self) -> &str;

    /// Store `payload` under `key`. Overwrites any existing value.
    async fn set(&self, key: &str, payload: Message) -> Result<(), CamelError>;

    /// Retrieve payload by key without removing it.
    ///
    /// Returns `Err(CamelError::NotFound(...))` if the key does not exist.
    async fn get(&self, key: &str) -> Result<Message, CamelError>;

    /// Retrieve and remove in one atomic step.
    ///
    /// Returns `Err(CamelError::NotFound(...))` if the key does not exist.
    async fn get_and_remove(&self, key: &str) -> Result<Message, CamelError>;

    /// Remove payload by key. Succeeds even if the key does not exist.
    async fn remove(&self, key: &str) -> Result<(), CamelError>;

    /// Push payload onto a LIFO stack for `key`.
    async fn push(&self, key: &str, payload: Message) -> Result<(), CamelError>;

    /// Pop payload from a LIFO stack for `key`.
    ///
    /// Returns `Err(CamelError::NotFound(...))` if the stack is empty.
    async fn pop(&self, key: &str) -> Result<Message, CamelError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;
    use std::sync::Arc;

    /// Stub that returns empty bodies for all read operations.
    #[derive(Debug)]
    struct StubRepo;

    #[async_trait::async_trait]
    impl ClaimCheckRepository for StubRepo {
        fn name(&self) -> &str {
            "stub"
        }

        async fn set(&self, _key: &str, _payload: Message) -> Result<(), CamelError> {
            Ok(())
        }

        async fn get(&self, _key: &str) -> Result<Message, CamelError> {
            Ok(Message::new(Body::Empty))
        }

        async fn get_and_remove(&self, _key: &str) -> Result<Message, CamelError> {
            Ok(Message::new(Body::Empty))
        }

        async fn remove(&self, _key: &str) -> Result<(), CamelError> {
            Ok(())
        }

        async fn push(&self, _key: &str, _payload: Message) -> Result<(), CamelError> {
            Ok(())
        }

        async fn pop(&self, _key: &str) -> Result<Message, CamelError> {
            Ok(Message::new(Body::Empty))
        }
    }

    /// Stub compiles and can be stored as `Arc<dyn ClaimCheckRepository>`.
    #[test]
    fn stub_is_object_safe() {
        let repo: Arc<dyn ClaimCheckRepository> = Arc::new(StubRepo);
        assert_eq!(repo.name(), "stub");
    }

    #[tokio::test]
    async fn stub_set_and_get() {
        let repo = StubRepo;
        repo.set("k", Message::new(Body::Text("v".into())))
            .await
            .unwrap();
        let body = repo.get("k").await.unwrap();
        assert!(body.body.is_empty());
    }

    #[tokio::test]
    async fn stub_get_and_remove() {
        let repo = StubRepo;
        let body = repo.get_and_remove("k").await.unwrap();
        assert!(body.body.is_empty());
    }

    #[tokio::test]
    async fn stub_remove_is_ok() {
        let repo = StubRepo;
        repo.remove("k").await.unwrap();
    }

    #[tokio::test]
    async fn stub_push_pop() {
        let repo = StubRepo;
        repo.push("k", Message::new(Body::Text("v".into())))
            .await
            .unwrap();
        let body = repo.pop("k").await.unwrap();
        assert!(body.body.is_empty());
    }
}
