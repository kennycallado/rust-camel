//! IdempotentRepository trait — pluggable key store for Idempotent Consumer EIP.
//!
//! Contract C1: `contains()` returns `Result<bool, CamelError>` (NOT `bool`)
//! because backends (Redis, JDBC) can have transient read failures. The
//! Idempotent Consumer propagates `Err`, never treats a failed read as
//! "not a duplicate."

use crate::CamelError;

/// Pluggable key store for the Idempotent Consumer EIP (and similar patterns).
///
/// # Contract (C1)
///
/// - `contains` returns `Result<bool, CamelError>`. A transient backend failure
///   MUST propagate as `Err(CamelError)`, NOT as `Ok(false)`.
/// - `add` returns `Ok(true)` if the key was newly inserted, `Ok(false)` if it
///   was already present.
/// - `remove` succeeds even if the key does not exist.
/// - `clear` removes all keys.
#[async_trait::async_trait]
pub trait IdempotentRepository: Send + Sync + std::fmt::Debug + 'static {
    /// Human-readable name for diagnostics.
    fn name(&self) -> &str;

    /// Check whether `key` exists in the repository.
    ///
    /// Returns `Err` on transient backend failure (contract C1).
    async fn contains(&self, key: &str) -> Result<bool, CamelError>;

    /// Insert `key` if absent.
    ///
    /// Returns `Ok(true)` if newly added, `Ok(false)` if already present.
    async fn add(&self, key: &str) -> Result<bool, CamelError>;

    /// Remove `key` if present. Succeeds even if the key does not exist.
    async fn remove(&self, key: &str) -> Result<(), CamelError>;

    /// Remove all keys from the repository.
    async fn clear(&self) -> Result<(), CamelError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Stub that always returns Ok(false) for contains, Ok(true) for add.
    #[derive(Debug)]
    struct StubRepo;

    #[async_trait::async_trait]
    impl IdempotentRepository for StubRepo {
        fn name(&self) -> &str {
            "stub"
        }

        async fn contains(&self, _key: &str) -> Result<bool, CamelError> {
            Ok(false)
        }

        async fn add(&self, _key: &str) -> Result<bool, CamelError> {
            Ok(true)
        }

        async fn remove(&self, _key: &str) -> Result<(), CamelError> {
            Ok(())
        }

        async fn clear(&self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    /// Stub compiles and can be stored as `Arc<dyn IdempotentRepository>`.
    #[test]
    fn stub_is_object_safe() {
        let repo: Arc<dyn IdempotentRepository> = Arc::new(StubRepo);
        assert_eq!(repo.name(), "stub");
    }

    #[tokio::test]
    async fn stub_contains_returns_false() {
        let repo = StubRepo;
        let result = repo.contains("any-key").await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn stub_add_returns_true() {
        let repo = StubRepo;
        let result = repo.add("any-key").await.unwrap();
        assert!(result);
    }
}
