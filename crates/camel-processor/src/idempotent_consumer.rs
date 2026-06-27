//! Idempotent Consumer EIP — outcome-aware Segment implementation.
//!
//! Implements the Idempotent Consumer pattern at the `OutcomePipeline` layer
//! (one layer above Tower), so that `PipelineOutcome::Stopped` from the child
//! sub-pipeline propagates with the Exchange intact (ADR-0024, ADR-0025).
//!
//! # Why Segment-mode (NOT Process-mode)
//!
//! `compose_pipeline()` translates `PipelineOutcome::Stopped → Ok(ex)` for
//! Tower `Service<Exchange>` consumers. If this EIP were a Tower Service,
//! a duplicate-detected `Stopped` would become `Ok(ex)` and downstream steps
//! would re-process the duplicate. By implementing `OutcomePipeline` directly,
//! the segment returns `PipelineOutcome::Completed(ex)` for duplicates and
//! skips the child sub-pipeline entirely.
//!
//! # Contract C1
//!
//! `IdempotentRepository::contains` returns `Result<bool, CamelError>` because
//! backends (Redis, JDBC) can have transient read failures. The segment
//! propagates `Err` as `PipelineOutcome::Failed` — it NEVER treats a failed
//! read as "not a duplicate."

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use camel_api::{Exchange, IdempotentRepository, OutcomePipeline, OutcomeSegment, PipelineOutcome};

/// Synchronous closure that extracts the message-id key from an Exchange.
///
/// Returns `None` when no key can be extracted — in that case the segment
/// forwards the exchange to the child sub-pipeline without idempotency
/// (matching Apache Camel semantics).
pub type MessageIdExpression = Arc<dyn Fn(&Exchange) -> Option<String> + Send + Sync>;

/// Outcome-aware Idempotent Consumer segment.
///
/// Wraps a child sub-pipeline (`OutcomeSegment`) and a named
/// `IdempotentRepository`. On each exchange:
///
/// 1. Extract message-id via `message_id(&exchange)`. `None` → forward to child.
/// 2. Check `repo.contains(&key)`:
///    - `Ok(true)` → duplicate, return `Completed(exchange)` (skip child).
///    - `Ok(false)` → proceed.
///    - `Err(e)` → `Failed(e)` (contract C1).
/// 3. If `eager`: `repo.add(&key)` before child. `Ok(false)` = race duplicate
///    → `Completed`. `Err` → `Failed`.
/// 4. Run child pipeline.
/// 5. Post-child:
///    - `Completed` + non-eager → `repo.add(&key)` (best-effort).
///    - `Stopped` → no add (pipeline terminating).
///    - `Failed` + `eager` + `remove_on_failure` → `repo.remove(&key)` (best-effort).
///
/// # Eager mode + Stopped
///
/// In eager mode, the key is added BEFORE the child pipeline runs. If the child
/// returns `Stopped`, the key remains registered — subsequent deliveries of the
/// same message ID are treated as duplicates and return `Completed` immediately.
/// This is intentional: a Stop indicates the route chose to terminate, and
/// re-processing the same message would be incorrect.
pub struct IdempotentConsumerSegment {
    repository: Arc<dyn IdempotentRepository>,
    message_id: MessageIdExpression,
    child_pipeline: OutcomeSegment,
    eager: bool,
    remove_on_failure: bool,
}

impl IdempotentConsumerSegment {
    /// Build a new segment from a resolved repository, message-id extractor,
    /// child sub-pipeline, and behaviour flags.
    pub fn new(
        repository: Arc<dyn IdempotentRepository>,
        message_id: MessageIdExpression,
        child_pipeline: OutcomeSegment,
        eager: bool,
        remove_on_failure: bool,
    ) -> Self {
        Self {
            repository,
            message_id,
            child_pipeline,
            eager,
            remove_on_failure,
        }
    }
}

impl Clone for IdempotentConsumerSegment {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            message_id: Arc::clone(&self.message_id),
            child_pipeline: self.child_pipeline.clone(),
            eager: self.eager,
            remove_on_failure: self.remove_on_failure,
        }
    }
}

impl OutcomePipeline for IdempotentConsumerSegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            // 1. Extract message-id. None → no idempotency possible, forward to child.
            let key = match (self.message_id)(&exchange) {
                Some(k) => k,
                None => return self.child_pipeline.run(exchange).await,
            };

            // 2. Check repository (contract C1: propagate Err, never treat as non-duplicate).
            match self.repository.contains(&key).await {
                Ok(true) => return PipelineOutcome::Completed(exchange),
                Ok(false) => {}
                Err(e) => return PipelineOutcome::Failed(e),
            }

            // 3. Eager mode: reserve the key before running the child.
            if self.eager {
                match self.repository.add(&key).await {
                    Ok(true) => {}                                            // reserved, proceed
                    Ok(false) => return PipelineOutcome::Completed(exchange), // race duplicate
                    Err(e) => return PipelineOutcome::Failed(e),
                }
            }

            // 4. Run child sub-pipeline.
            let outcome = self.child_pipeline.run(exchange).await;

            // 5. Post-child bookkeeping (best-effort; log transient errors).
            match &outcome {
                PipelineOutcome::Completed(_) if !self.eager => {
                    if let Err(e) = self.repository.add(&key).await {
                        tracing::warn!(
                            error = %e,
                            repository = %self.repository.name(),
                            key = %key,
                            "idempotent repository add failed post-success; next delivery may re-process"
                        );
                    }
                }
                PipelineOutcome::Failed(_) if self.eager && self.remove_on_failure => {
                    if let Err(e) = self.repository.remove(&key).await {
                        tracing::warn!(
                            error = %e,
                            repository = %self.repository.name(),
                            key = %key,
                            "idempotent repository remove failed on failure rollback"
                        );
                    }
                }
                _ => {}
            }

            outcome
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use camel_api::{CamelError, Exchange, Message, Value};
    use std::collections::HashSet;
    use tokio::sync::Mutex;

    // ── Test helpers ──

    /// In-memory mock repository backed by a `Mutex<HashSet>`. Allows tests
    /// to pre-populate keys and inspect state after the segment runs.
    #[derive(Debug, Default)]
    struct MockRepo {
        keys: Mutex<HashSet<String>>,
        fail_contains: bool,
    }

    impl MockRepo {
        fn new() -> Self {
            Self::default()
        }

        fn failing_contains() -> Self {
            Self {
                keys: Mutex::new(HashSet::new()),
                fail_contains: true,
            }
        }

        async fn pre_add(&self, key: &str) {
            self.keys.lock().await.insert(key.to_string());
        }

        async fn contains_key(&self, key: &str) -> bool {
            self.keys.lock().await.contains(key)
        }
    }

    #[async_trait]
    impl IdempotentRepository for MockRepo {
        fn name(&self) -> &str {
            "mock"
        }

        async fn contains(&self, key: &str) -> Result<bool, CamelError> {
            if self.fail_contains {
                return Err(CamelError::ProcessorError(
                    "synthetic contains failure".into(),
                ));
            }
            Ok(self.keys.lock().await.contains(key))
        }

        async fn add(&self, key: &str) -> Result<bool, CamelError> {
            let mut guard = self.keys.lock().await;
            Ok(guard.insert(key.to_string()))
        }

        async fn remove(&self, key: &str) -> Result<(), CamelError> {
            self.keys.lock().await.remove(key);
            Ok(())
        }

        async fn clear(&self) -> Result<(), CamelError> {
            self.keys.lock().await.clear();
            Ok(())
        }
    }

    /// Test segment that returns a configurable outcome and records whether
    /// it was invoked. Used to verify the segment's child-dispatch behaviour.
    struct ScriptedChild {
        outcome: PipelineOutcome,
        invoked: Arc<std::sync::atomic::AtomicBool>,
    }

    impl OutcomePipeline for ScriptedChild {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            // clone_box is required by the trait but unused by these tests
            // (each test constructs a fresh segment).
            unreachable!("clone_box not used in idempotent_consumer tests")
        }

        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            self.invoked
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let outcome = std::mem::replace(
                &mut self.outcome,
                PipelineOutcome::Completed(Exchange::new(Message::new(""))),
            );
            Box::pin(async move { outcome_with_exchange(outcome, exchange) })
        }
    }

    /// Replace the placeholder exchange inside a scripted outcome with the
    /// actual exchange passed to `run`. Keeps the variant, swaps the payload.
    fn outcome_with_exchange(outcome: PipelineOutcome, exchange: Exchange) -> PipelineOutcome {
        match outcome {
            PipelineOutcome::Completed(_) => PipelineOutcome::Completed(exchange),
            PipelineOutcome::Stopped(_) => PipelineOutcome::Stopped(exchange),
            PipelineOutcome::Failed(e) => PipelineOutcome::Failed(e),
        }
    }

    fn exchange_with_id(id: &str) -> Exchange {
        let mut ex = Exchange::new(Message::new("payload"));
        ex.input.set_header("messageId", Value::String(id.into()));
        ex
    }

    fn header_message_id() -> MessageIdExpression {
        Arc::new(|ex: &Exchange| {
            ex.input
                .header("messageId")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        })
    }

    fn build_segment(
        repo: Arc<MockRepo>,
        child_outcome: PipelineOutcome,
        eager: bool,
        remove_on_failure: bool,
    ) -> (
        IdempotentConsumerSegment,
        Arc<std::sync::atomic::AtomicBool>,
    ) {
        let invoked = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let child = ScriptedChild {
            outcome: child_outcome,
            invoked: invoked.clone(),
        };
        let segment = IdempotentConsumerSegment::new(
            repo,
            header_message_id(),
            OutcomeSegment::new(Box::new(child)),
            eager,
            remove_on_failure,
        );
        (segment, invoked)
    }

    // ── Test 1: duplicate key short-circuits to Completed without touching child ──
    #[tokio::test]
    async fn duplicate_key_returns_completed_without_running_child() {
        let repo = Arc::new(MockRepo::new());
        repo.pre_add("dup-1").await;
        let (mut segment, child_invoked) = build_segment(
            repo.clone(),
            PipelineOutcome::Failed(stub_error()),
            false,
            false,
        );

        let ex = exchange_with_id("dup-1");
        let outcome = segment.run(ex).await;

        assert!(matches!(outcome, PipelineOutcome::Completed(_)));
        assert!(
            !child_invoked.load(std::sync::atomic::Ordering::SeqCst),
            "child must NOT run when key is a duplicate"
        );
        // key still present
        assert!(repo.contains_key("dup-1").await);
    }

    // ── Test 2: new key runs the child and returns its outcome ──
    #[tokio::test]
    async fn new_key_runs_child_and_returns_child_outcome() {
        let repo = Arc::new(MockRepo::new());
        let (mut segment, child_invoked) = build_segment(
            repo.clone(),
            PipelineOutcome::Completed(Exchange::new(Message::new(""))),
            false,
            false,
        );

        let ex = exchange_with_id("new-1");
        let outcome = segment.run(ex).await;

        assert!(matches!(outcome, PipelineOutcome::Completed(_)));
        assert!(
            child_invoked.load(std::sync::atomic::Ordering::SeqCst),
            "child MUST run when key is new"
        );
        // Non-eager mode: key added after child Completed.
        assert!(
            repo.contains_key("new-1").await,
            "non-eager mode must add key after successful child run"
        );
    }

    // ── Test 3: failed repo read propagates as Failed (contract C1) ──
    #[tokio::test]
    async fn failed_repo_read_propagates_error() {
        let repo = Arc::new(MockRepo::failing_contains());
        let (mut segment, child_invoked) = build_segment(
            repo,
            PipelineOutcome::Completed(stub_exchange()),
            false,
            false,
        );

        let ex = exchange_with_id("any");
        let outcome = segment.run(ex).await;

        match outcome {
            PipelineOutcome::Failed(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("synthetic contains failure"),
                    "expected synthetic failure in error, got: {msg}"
                );
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(
            !child_invoked.load(std::sync::atomic::Ordering::SeqCst),
            "child must NOT run when repo read fails"
        );
    }

    // ── Test 4: Stopped from child propagates as Stopped (segment-mode contract) ──
    #[tokio::test]
    async fn stopped_child_propagates_stopped() {
        let repo = Arc::new(MockRepo::new());
        let (mut segment, child_invoked) = build_segment(
            repo.clone(),
            PipelineOutcome::Stopped(stub_exchange()),
            false,
            false,
        );

        let ex = exchange_with_id("stop-1");
        let outcome = segment.run(ex).await;

        assert!(
            matches!(outcome, PipelineOutcome::Stopped(_)),
            "Stopped from child MUST propagate as Stopped (segment-mode contract)"
        );
        assert!(
            child_invoked.load(std::sync::atomic::Ordering::SeqCst),
            "child must run before its Stopped can propagate"
        );
        // Stopped path must NOT register the key (pipeline terminating).
        assert!(
            !repo.contains_key("stop-1").await,
            "Stopped outcome must not register the key"
        );
    }

    // ── Test 5: eager mode + remove_on_failure rolls back key on Failed ──
    #[tokio::test]
    async fn eager_mode_removes_key_on_failure_when_configured() {
        let repo = Arc::new(MockRepo::new());
        let (mut segment, _child_invoked) = build_segment(
            repo.clone(),
            PipelineOutcome::Failed(stub_error()),
            true, // eager
            true, // remove_on_failure
        );

        let ex = exchange_with_id("eager-fail");
        let outcome = segment.run(ex).await;

        assert!(matches!(outcome, PipelineOutcome::Failed(_)));
        assert!(
            !repo.contains_key("eager-fail").await,
            "eager + remove_on_failure must roll back the key on failure"
        );
    }

    // ── Test 6: missing message-id forwards to child without idempotency ──
    #[tokio::test]
    async fn missing_message_id_forwards_to_child() {
        let repo = Arc::new(MockRepo::new());
        let (mut segment, child_invoked) = build_segment(
            repo.clone(),
            PipelineOutcome::Completed(stub_exchange()),
            false,
            false,
        );

        // Exchange WITHOUT a messageId header.
        let ex = Exchange::new(Message::new("no-id"));
        let outcome = segment.run(ex).await;

        assert!(matches!(outcome, PipelineOutcome::Completed(_)));
        assert!(
            child_invoked.load(std::sync::atomic::Ordering::SeqCst),
            "child must run when message-id cannot be extracted"
        );
    }

    fn stub_exchange() -> Exchange {
        Exchange::new(Message::new(""))
    }

    fn stub_error() -> CamelError {
        CamelError::ProcessorError("child failed".into())
    }
}
