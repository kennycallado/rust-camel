//! Claim Check EIP processor.
//!
//! Stashes the message body into a `ClaimCheckRepository` and replaces it with
//! a lightweight key reference (text body containing the key). Later steps can
//! retrieve (Get) or retrieve+remove (GetAndRemove) the body, or use LIFO
//! stack operations (Push/Pop).
//!
//! # Process mode
//!
//! This is a **Process-mode** Tower `Service<Exchange>` — transforms the body
//! in place, no child sub-pipeline. No `StepLifecycle` (holds only
//! `Arc<dyn ClaimCheckRepository>`, no background work).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::body::Body;
use camel_api::{CamelError, ClaimCheckRepository, Exchange};

/// Extracts a claim-check key string from the exchange (e.g. from header/property/body).
/// Returns an error if the key cannot be resolved (null or empty).
pub type KeyExpression = Arc<dyn Fn(&Exchange) -> Result<String, CamelError> + Send + Sync>;

/// Claim Check operation variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimCheckOp {
    /// Stash the body in the repository; replace exchange body with key reference.
    Set,
    /// Retrieve the body from the repository by key; replace exchange body.
    Get,
    /// Retrieve and remove in one atomic step.
    GetAndRemove,
    /// Push current body onto a LIFO stack for this key; replace body with key reference.
    Push,
    /// Pop body from a LIFO stack for this key; replace exchange body.
    Pop,
}

/// Claim Check EIP processor.
///
/// Transforms the exchange body to/from a `ClaimCheckRepository` using the
/// configured operation and key expression.
#[derive(Clone)]
pub struct ClaimCheckService {
    repository: Arc<dyn ClaimCheckRepository>,
    operation: ClaimCheckOp,
    key_expression: KeyExpression,
}

impl std::fmt::Debug for ClaimCheckService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimCheckService")
            .field("repository", &self.repository.name())
            .field("operation", &self.operation)
            .finish()
    }
}

impl ClaimCheckService {
    /// Create a new `ClaimCheckService`.
    pub fn new(
        repository: Arc<dyn ClaimCheckRepository>,
        operation: ClaimCheckOp,
        key_expression: KeyExpression,
    ) -> Self {
        Self {
            repository,
            operation,
            key_expression,
        }
    }
}

impl Service<Exchange> for ClaimCheckService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let repository = self.repository.clone();
        let operation = self.operation.clone();
        let key_result = (self.key_expression)(&exchange);

        Box::pin(async move {
            let key = key_result?;
            match operation {
                ClaimCheckOp::Set => {
                    let body = std::mem::replace(&mut exchange.input.body, Body::Empty);
                    repository.set(&key, body).await?;
                    exchange.input.body = Body::Text(key);
                    Ok(exchange)
                }
                ClaimCheckOp::Get => {
                    let body = repository.get(&key).await?;
                    exchange.input.body = body;
                    Ok(exchange)
                }
                ClaimCheckOp::GetAndRemove => {
                    let body = repository.get_and_remove(&key).await?;
                    exchange.input.body = body;
                    Ok(exchange)
                }
                ClaimCheckOp::Push => {
                    let body = std::mem::replace(&mut exchange.input.body, Body::Empty);
                    repository.push(&key, body).await?;
                    exchange.input.body = Body::Text(key);
                    Ok(exchange)
                }
                ClaimCheckOp::Pop => {
                    let body = repository.pop(&key).await?;
                    exchange.input.body = body;
                    Ok(exchange)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Exchange, Message};
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;
    use tower::ServiceExt;

    /// In-memory test repository backed by `Mutex<HashMap>`.
    #[derive(Debug)]
    struct TestRepo {
        name: String,
        keys: Mutex<HashMap<String, Body>>,
        stacks: Mutex<HashMap<String, VecDeque<Body>>>,
    }

    impl TestRepo {
        fn new(name: impl Into<String>) -> Self {
            Self {
                name: name.into(),
                keys: Mutex::new(HashMap::new()),
                stacks: Mutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl ClaimCheckRepository for TestRepo {
        fn name(&self) -> &str {
            &self.name
        }

        async fn set(&self, key: &str, payload: Body) -> Result<(), CamelError> {
            self.keys
                .lock()
                .expect("mutex poisoned") // allow-unwrap
                .insert(key.to_string(), payload);
            Ok(())
        }

        async fn get(&self, key: &str) -> Result<Body, CamelError> {
            self.keys
                .lock()
                .expect("mutex poisoned") // allow-unwrap
                .get(key)
                .cloned()
                .ok_or_else(|| CamelError::RouteError(format!("Claim check key not found: {key}")))
        }

        async fn get_and_remove(&self, key: &str) -> Result<Body, CamelError> {
            self.keys
                .lock()
                .expect("mutex poisoned") // allow-unwrap
                .remove(key)
                .ok_or_else(|| CamelError::RouteError(format!("Claim check key not found: {key}")))
        }

        async fn remove(&self, key: &str) -> Result<(), CamelError> {
            self.keys
                .lock()
                .expect("mutex poisoned") // allow-unwrap
                .remove(key);
            Ok(())
        }

        async fn push(&self, key: &str, payload: Body) -> Result<(), CamelError> {
            self.stacks
                .lock()
                .expect("mutex poisoned") // allow-unwrap
                .entry(key.to_string())
                .or_default()
                .push_back(payload);
            Ok(())
        }

        async fn pop(&self, key: &str) -> Result<Body, CamelError> {
            self.stacks
                .lock()
                .expect("mutex poisoned") // allow-unwrap
                .get_mut(key)
                .and_then(|s| s.pop_back())
                .ok_or_else(|| {
                    CamelError::RouteError(format!("Claim check stack empty for key: {key}"))
                })
        }
    }

    fn make_key_expr(key: &str) -> KeyExpression {
        let k = key.to_string();
        Arc::new(move |_ex: &Exchange| Ok(k.clone()))
    }

    fn make_exchange(body: Body) -> Exchange {
        Exchange::new(Message::new(body))
    }

    #[tokio::test]
    async fn set_moves_body_to_repo() {
        let repo = Arc::new(TestRepo::new("test"));
        let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Set, make_key_expr("mykey"));
        let exchange = make_exchange(Body::Text("secret-data".to_string()));

        let result = svc.oneshot(exchange).await.unwrap();
        // Exchange body is now the key reference
        assert_eq!(result.input.body, Body::Text("mykey".to_string()));

        // The original body is stashed in the repo
        let stashed = repo.get("mykey").await.unwrap();
        assert_eq!(stashed, Body::Text("secret-data".to_string()));
    }

    #[tokio::test]
    async fn get_restores_body() {
        let repo = Arc::new(TestRepo::new("test"));
        repo.set("k1", Body::Text("stashed-body".to_string()))
            .await
            .unwrap();

        let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Get, make_key_expr("k1"));
        let exchange = make_exchange(Body::Empty);

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body, Body::Text("stashed-body".to_string()));
    }

    #[tokio::test]
    async fn get_and_remove_restores_and_deletes() {
        let repo = Arc::new(TestRepo::new("test"));
        repo.set("k2", Body::Text("will-be-removed".to_string()))
            .await
            .unwrap();

        let svc = ClaimCheckService::new(
            repo.clone(),
            ClaimCheckOp::GetAndRemove,
            make_key_expr("k2"),
        );
        let exchange = make_exchange(Body::Empty);

        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body, Body::Text("will-be-removed".to_string()));

        // Repo should be empty after get_and_remove
        let err = repo.get("k2").await.unwrap_err();
        assert!(
            matches!(&err, CamelError::RouteError(msg) if msg.contains("not found")),
            "expected RouteError with 'not found', got: {err:?}"
        );
    }

    #[tokio::test]
    async fn push_pop_lifo() {
        let repo = Arc::new(TestRepo::new("test"));
        let first = Body::Text("first".to_string());
        let second = Body::Text("second".to_string());

        // Push first
        let svc =
            ClaimCheckService::new(repo.clone(), ClaimCheckOp::Push, make_key_expr("stack-key"));
        svc.oneshot(make_exchange(first)).await.unwrap();

        // Push second
        let svc =
            ClaimCheckService::new(repo.clone(), ClaimCheckOp::Push, make_key_expr("stack-key"));
        svc.oneshot(make_exchange(second.clone())).await.unwrap();

        // Pop should return second (LIFO)
        let svc =
            ClaimCheckService::new(repo.clone(), ClaimCheckOp::Pop, make_key_expr("stack-key"));
        let result = svc.oneshot(make_exchange(Body::Empty)).await.unwrap();
        assert_eq!(result.input.body, second);

        // Pop should return first
        let svc =
            ClaimCheckService::new(repo.clone(), ClaimCheckOp::Pop, make_key_expr("stack-key"));
        let result = svc.oneshot(make_exchange(Body::Empty)).await.unwrap();
        assert_eq!(result.input.body, Body::Text("first".to_string()));
    }

    #[tokio::test]
    async fn get_missing_propagates_error() {
        let repo = Arc::new(TestRepo::new("test"));
        let svc = ClaimCheckService::new(
            repo.clone(),
            ClaimCheckOp::Get,
            make_key_expr("nonexistent"),
        );
        let exchange = make_exchange(Body::Empty);
        let result = svc.oneshot(exchange).await;
        assert!(
            matches!(&result, Err(CamelError::RouteError(msg)) if msg.contains("not found")),
            "expected Err with 'not found', got: {result:?}"
        );
    }

    #[tokio::test]
    async fn set_overwrites_existing() {
        let repo = Arc::new(TestRepo::new("test"));
        repo.set("k", Body::Text("old".to_string())).await.unwrap();

        let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Set, make_key_expr("k"));
        let exchange = make_exchange(Body::Text("new".to_string()));
        svc.oneshot(exchange).await.unwrap();

        let stashed = repo.get("k").await.unwrap();
        assert_eq!(stashed, Body::Text("new".to_string()));
    }

    #[tokio::test]
    async fn pop_empty_stack_propagates_error() {
        let repo = Arc::new(TestRepo::new("test"));
        let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Pop, make_key_expr("empty"));
        let exchange = make_exchange(Body::Empty);
        let result = svc.oneshot(exchange).await;
        assert!(
            matches!(&result, Err(CamelError::RouteError(msg)) if msg.contains("empty")),
            "expected Err with 'empty', got: {result:?}"
        );
    }
}
