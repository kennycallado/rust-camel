use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use camel_api::security_policy::SecurityPolicy;
use camel_api::{CamelError, Exchange};
use camel_auth::{CredentialSource, TokenAuthenticator};

/// A message sent from a consumer to the route pipeline.
///
/// Fire-and-forget exchanges use `reply_tx = None`.
/// Request-reply exchanges (e.g. `direct:`) provide a `reply_tx` so the
/// pipeline result can be sent back to the consumer.
pub struct ExchangeEnvelope {
    pub exchange: Exchange,
    pub reply_tx: Option<oneshot::Sender<Result<Exchange, CamelError>>>,
}

/// Declares when the runtime may consider a Consumer "started".
///
/// `Immediate` (default) preserves the classic fire-and-forget semantics:
/// `spawn_consumer_task` returns as soon as the consumer task is spawned,
/// matching the behaviour of timer, file, direct and similar polling
/// consumers whose `start()` IS the lifetime loop.
///
/// `Explicit` is for resource-binding consumers (HTTP, WebSocket, …) whose
/// `start()` returns control only after the resource (e.g. `TcpListener`)
/// is bound and ready. The consumer MUST call `ConsumerContext::mark_ready`
/// after a successful bind so the runtime can await readiness and propagate
/// pre-ready `start()` errors as proper startup failures.
///
/// Adding this as a default-returning trait method keeps every existing
/// `Consumer` impl backward compatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConsumerStartupMode {
    /// Consumer's `start()` IS the lifetime loop. The runtime treats the
    /// consumer as ready the moment `start()` is invoked. (Default)
    #[default]
    Immediate,
    /// Consumer binds/registers a resource inside `start()` and signals
    /// readiness explicitly via `ConsumerContext::mark_ready()`.
    Explicit,
}

/// Internal state of a [`StartupSignal`].
#[derive(Clone, Debug)]
enum StartupState {
    /// Consumer has not yet signalled readiness or failure.
    Pending,
    /// Consumer signalled readiness via `mark_ready()`.
    Ready,
    /// Consumer's `start()` returned an `Err` before signalling readiness.
    Failed(String),
}

/// Shared handle used by a Consumer to signal readiness (or failure) to the
/// runtime's [`StartupReceiver`].
///
/// Constructed in a pair via [`StartupSignal::pair`]. The signal is held by
/// the consumer side (via [`ConsumerContext`]); the receiver is returned to
/// the route controller.
#[derive(Clone)]
pub struct StartupSignal {
    tx: watch::Sender<StartupState>,
}

impl StartupSignal {
    /// Create a `(signal, receiver)` pair seeded in the `Pending` state.
    pub fn pair() -> (Self, StartupReceiver) {
        let (tx, rx) = watch::channel(StartupState::Pending);
        (Self { tx }, StartupReceiver { rx })
    }

    /// Mark the consumer as ready. Idempotent — subsequent calls are no-ops
    /// once the state has transitioned out of `Pending`.
    ///
    /// Returns `true` if this call transitioned `Pending → Ready`, `false`
    /// if the state was already `Ready` or `Failed`. The runtime uses the
    /// return value to detect Explicit consumers that returned `Ok` from
    /// `start()` without calling `mark_ready` (a contract violation that
    /// would hang the controller without the defensive fallback in
    /// `spawn_consumer_task`).
    pub fn mark_ready(&self) -> bool {
        self.tx.send_if_modified(|s| {
            if matches!(*s, StartupState::Pending) {
                *s = StartupState::Ready;
                true
            } else {
                false
            }
        })
    }

    /// Mark the consumer's startup as failed with `err`. Idempotent.
    pub fn mark_failed(&self, err: String) {
        self.tx.send_if_modified(|s| {
            if matches!(*s, StartupState::Pending) {
                *s = StartupState::Failed(err);
                true
            } else {
                false
            }
        });
    }
}

impl std::fmt::Debug for StartupSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartupSignal")
            .field("state", &self.tx.borrow())
            .finish()
    }
}

/// Receiver half of the consumer startup handshake. Resolves once the
/// consumer calls [`ConsumerContext::mark_ready`] (Ok) or its `start()`
/// returns an `Err` first (Err).
///
/// For [`ConsumerStartupMode::Immediate`] consumers the receiver is
/// pre-resolved at construction time (see [`StartupReceiver::immediate`]).
pub struct StartupReceiver {
    rx: watch::Receiver<StartupState>,
}

impl StartupReceiver {
    /// Construct a receiver that is already resolved as `Ok`. Used for
    /// [`ConsumerStartupMode::Immediate`] consumers so the controller can
    /// uniformly `await` every receiver without changing behaviour.
    pub fn immediate() -> Self {
        let (tx, rx) = watch::channel(StartupState::Ready);
        // Drop the sender — state is fixed at Ready. Receiver will never
        // observe a closure error since it already holds Ready.
        let _ = tx;
        Self { rx }
    }

    /// Wait for the consumer to become ready or fail. Resolves:
    /// - `Ok(())` if the consumer signalled readiness.
    /// - `Err(CamelError::RouteError(_))` if the consumer's `start()`
    ///   returned an error before signalling readiness.
    /// - `Err(CamelError::RouteError(_))` if the signal sender was dropped
    ///   without either transition (programming-contract violation).
    pub async fn await_ready(mut self) -> Result<(), CamelError> {
        loop {
            match &*self.rx.borrow() {
                StartupState::Pending => {}
                StartupState::Ready => return Ok(()),
                StartupState::Failed(msg) => {
                    return Err(CamelError::RouteError(msg.clone()));
                }
            }
            if self.rx.changed().await.is_err() {
                return Err(CamelError::RouteError(
                    "consumer startup signal dropped without resolving".to_string(),
                ));
            }
        }
    }
}

impl std::fmt::Debug for StartupReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartupReceiver")
            .field("state", &self.rx.borrow())
            .finish()
    }
}

/// Context provided to a Consumer, allowing it to send exchanges into the route.
#[derive(Clone)]
pub struct ConsumerContext {
    sender: mpsc::Sender<ExchangeEnvelope>,
    cancel_token: CancellationToken,
    route_id: String,
    startup: StartupSignal,
}

impl ConsumerContext {
    /// Create a new consumer context wrapping the given channel sender.
    ///
    /// The `route_id` identifies the route this consumer is bound to, enabling
    /// ADR-0012 per-route metrics and health observations.
    ///
    /// The startup signal defaults to a fresh `Pending` pair; the consumer
    /// can call [`Self::mark_ready`] once it has bound its resource. For
    /// [`ConsumerStartupMode::Immediate`] consumers the runtime ignores
    /// the signal (it constructs an already-resolved receiver instead).
    pub fn new(
        sender: mpsc::Sender<ExchangeEnvelope>,
        cancel_token: CancellationToken,
        route_id: String,
    ) -> Self {
        let (startup, _unused_receiver) = StartupSignal::pair();
        // The receiver is dropped here: `spawn_consumer_task` constructs its
        // own `(signal, receiver)` pair and replaces this one via
        // `with_startup` so the controller holds the matching receiver.
        let _ = _unused_receiver;
        Self {
            sender,
            cancel_token,
            route_id,
            startup,
        }
    }

    /// Replace the startup signal carried by this context. Used by
    /// `spawn_consumer_task` to install the signal whose matching receiver
    /// is returned to the route controller.
    pub fn with_startup(mut self, startup: StartupSignal) -> Self {
        self.startup = startup;
        self
    }

    /// Returns a clone of the internal [`StartupSignal`] so callers (e.g.
    /// `spawn_consumer_task`) can drive failure propagation independently of
    /// the consumer's own `mark_ready()` call.
    pub fn startup_signal(&self) -> StartupSignal {
        self.startup.clone()
    }

    /// Mark this consumer's startup as complete. Only meaningful for
    /// [`ConsumerStartupMode::Explicit`] consumers — `Immediate` consumers
    /// never need to call this because the runtime resolves their startup
    /// receiver at construction time.
    ///
    /// Idempotent.
    pub fn mark_ready(&self) {
        let _ = self.startup.mark_ready();
    }

    /// Returns a future that resolves when shutdown is requested.
    /// Use in `tokio::select!` inside consumer loops.
    pub async fn cancelled(&self) {
        self.cancel_token.cancelled().await
    }

    /// Returns true if shutdown has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// Returns the route_id this consumer is bound to.
    ///
    /// Available for ADR-0012 metrics/health calls that require a route_id
    /// (categories (b′), (e), (g)). Set at construction time by the route
    /// controller when spawning the consumer task.
    pub fn route_id(&self) -> &str {
        &self.route_id
    }

    /// Returns a clone of the `CancellationToken`.
    ///
    /// Useful for consumers that spawn per-request tasks and need to propagate
    /// shutdown to each task. See `HttpConsumer` for an example.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Returns a clone of the channel sender for manual exchange submission.
    ///
    /// Useful for consumers that spawn per-request tasks (e.g., `HttpConsumer`)
    /// where each task independently sends exchanges into the pipeline.
    /// For simple consumers, prefer `send()` or `send_and_wait()` instead.
    pub fn sender(&self) -> mpsc::Sender<ExchangeEnvelope> {
        self.sender.clone()
    }

    /// Send an exchange into the route pipeline (fire-and-forget).
    pub async fn send(&self, exchange: Exchange) -> Result<(), CamelError> {
        self.sender
            .send(ExchangeEnvelope {
                exchange,
                reply_tx: None,
            })
            .await
            .map_err(|_| CamelError::ChannelClosed)
    }

    /// Send an exchange and wait for the pipeline result (request-reply).
    ///
    /// Returns `Ok(exchange)` on success or `Err(e)` if the pipeline failed
    /// without an error handler absorbing the error.
    pub async fn send_and_wait(&self, exchange: Exchange) -> Result<Exchange, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender
            .send(ExchangeEnvelope {
                exchange,
                reply_tx: Some(reply_tx),
            })
            .await
            .map_err(|_| CamelError::ChannelClosed)?;
        reply_rx.await.map_err(|_| CamelError::ChannelClosed)?
    }
}

/// Security context passed to a consumer before `start()`.
///
/// Carries the `SecurityPolicy` and `TokenAuthenticator` from the route
/// controller so consumers (e.g. WebSocket) can register auth state
/// before accepting connections.
pub struct SecurityContext {
    pub policy: Arc<dyn SecurityPolicy>,
    pub authenticator: Arc<dyn TokenAuthenticator>,
    pub credential_sources: Vec<CredentialSource>,
}

impl SecurityContext {
    pub fn new(
        policy: impl SecurityPolicy + 'static,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            policy: Arc::new(policy),
            authenticator,
            credential_sources: vec![CredentialSource::AuthorizationHeader],
        }
    }

    pub fn from_arc(
        policy: Arc<dyn SecurityPolicy>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            policy,
            authenticator,
            credential_sources: vec![CredentialSource::AuthorizationHeader],
        }
    }

    pub fn with_credential_sources(mut self, sources: Vec<CredentialSource>) -> Self {
        self.credential_sources = sources;
        self
    }
}

impl Clone for SecurityContext {
    fn clone(&self) -> Self {
        Self {
            policy: Arc::clone(&self.policy),
            authenticator: Arc::clone(&self.authenticator),
            credential_sources: self.credential_sources.clone(),
        }
    }
}

impl std::fmt::Debug for SecurityContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityContext")
            .field("policy", &"<SecurityPolicy>")
            .field("authenticator", &"<TokenAuthenticator>")
            .field("credential_sources", &self.credential_sources)
            .finish()
    }
}

/// How a consumer's exchanges should be processed by the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConcurrencyModel {
    /// Exchanges are processed one at a time, in order. Default for polling
    /// consumers (timer, file) and synchronous consumers (direct).
    Sequential,
    /// Exchanges are processed concurrently via `tokio::spawn`. Optional
    /// semaphore limit (`max`). `None` means unbounded (channel buffer is
    /// the only backpressure).
    Concurrent { max: Option<usize> },
}

/// A Consumer receives data from an external system and submits Exchanges
/// to the Route's Pipeline via the [`ConsumerContext`].
///
/// # Shutdown Contract
///
/// The Runtime guarantees the following lifecycle:
///
/// 1. `start()` is called once. The Runtime spawns a task that owns the Consumer.
/// 2. On route stop, the Runtime cancels the [`ConsumerContext`] cancel token.
/// 3. The spawned task calls `stop()` on ALL exit paths after `start()` succeeds
///    (clean exit, crash, cancellation, natural completion).
/// 4. `background_task_handle()` is a supervision hook for crash propagation (ADR-0007),
///    NOT the shutdown API.
///
/// Component authors MUST ensure:
///
/// - `stop()` cancels all component-owned inner tasks and cleans up registrations/resources.
/// - If inner tasks use a private `CancellationToken`, `stop()` MUST cancel it.
/// - Best practice: inner tasks should use the [`ConsumerContext`] cancel token (or a child)
///   so they respond to runtime shutdown without waiting for `stop()`.
/// - If using a private token, `stop()` must cancel it to ensure prompt cleanup.
/// - `background_task_handle()` returns the `JoinHandle` of the primary background task,
///   if any. The Runtime monitors this handle for unexpected exits (crash propagation).
#[async_trait]
pub trait Consumer: Send + Sync {
    /// Start consuming messages, sending them through the provided context.
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError>;

    /// Stop consuming messages and clean up all resources.
    ///
    /// Called by the Runtime on every exit path after `start()` succeeds.
    /// See the [Shutdown Contract](#shutdown-contract) above.
    async fn stop(&mut self) -> Result<(), CamelError>;

    /// Temporarily suspend consuming messages without fully stopping.
    ///
    /// Default: no-op, returns `Ok(())`.
    async fn suspend(&self) -> Result<(), CamelError> {
        Ok(())
    }

    /// Resume consuming after a previous suspension.
    ///
    /// Default: no-op, returns `Ok(())`.
    async fn resume(&self) -> Result<(), CamelError> {
        Ok(())
    }

    /// Declares this consumer's natural concurrency model.
    ///
    /// The runtime uses this to decide whether to process exchanges
    /// sequentially or spawn per-exchange. Consumers that accept inbound
    /// connections (HTTP, WebSocket, Kafka) should override this to return
    /// `ConcurrencyModel::Concurrent`.
    ///
    /// Default: `Sequential`.
    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }

    /// Declares how the runtime should wait for this consumer's startup.
    ///
    /// - [`ConsumerStartupMode::Immediate`] (default): the consumer's
    ///   `start()` IS the lifetime loop. The runtime treats the route as
    ///   started as soon as `start()` is invoked, preserving the existing
    ///   fire-and-forget semantics for timer/file/direct/… consumers.
    /// - [`ConsumerStartupMode::Explicit`]: the consumer binds/registers a
    ///   resource inside `start()` and MUST call
    ///   [`ConsumerContext::mark_ready`] after a successful bind. The
    ///   runtime awaits this signal (or an early `start()` error) before
    ///   treating the route as started, so HTTP/WebSocket listeners fail
    ///   fast when the bind fails instead of crashing the background task.
    ///
    /// Default: `Immediate`.
    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Immediate
    }

    /// Return a handle to the consumer's long-running background task so the
    /// runtime can monitor it for unexpected exits after `start()` returns `Ok`.
    ///
    /// Default: `None` — consumer's work completes entirely within `start()`.
    /// Override: return `Some(handle)` if `start()` spawns a detached task.
    ///
    /// **Contract:** the task must observe `ConsumerContext::cancelled()` so
    /// runtime shutdown is distinguishable from crash exits.
    ///
    /// This method is called at most once; implementations should use `.take()`
    /// to transfer ownership of the handle.
    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        None
    }

    /// Set the security context for this consumer.
    ///
    /// Called by the route controller before `start()` so the consumer
    /// can register auth state (e.g. WebSocket auth in `WsAppState`).
    ///
    /// Default: no-op, returns `Ok(())`.
    fn set_security_context(&mut self, _ctx: SecurityContext) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumer_context_exposes_route_id() {
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "test-route".to_string());
        assert_eq!(ctx.route_id(), "test-route");
    }

    #[tokio::test]
    async fn test_consumer_context_cancelled() {
        let (tx, _rx) = mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "test-route".to_string());

        assert!(!ctx.is_cancelled());
        token.cancel();
        ctx.cancelled().await;
        assert!(ctx.is_cancelled());
    }

    #[test]
    fn test_concurrency_model_default_is_sequential() {
        use super::ConcurrencyModel;

        struct DummyConsumer;

        #[async_trait::async_trait]
        impl super::Consumer for DummyConsumer {
            async fn start(&mut self, _ctx: super::ConsumerContext) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
        }

        let consumer = DummyConsumer;
        assert_eq!(consumer.concurrency_model(), ConcurrencyModel::Sequential);
    }

    #[test]
    fn test_concurrency_model_concurrent_override() {
        use super::ConcurrencyModel;

        struct ConcurrentConsumer;

        #[async_trait::async_trait]
        impl super::Consumer for ConcurrentConsumer {
            async fn start(&mut self, _ctx: super::ConsumerContext) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
            fn concurrency_model(&self) -> ConcurrencyModel {
                ConcurrencyModel::Concurrent { max: Some(16) }
            }
        }

        let consumer = ConcurrentConsumer;
        assert_eq!(
            consumer.concurrency_model(),
            ConcurrencyModel::Concurrent { max: Some(16) }
        );
    }

    // --- ConsumerStartupMode tests ---

    #[test]
    fn test_default_startup_mode_is_immediate() {
        struct DummyConsumer;

        #[async_trait::async_trait]
        impl super::Consumer for DummyConsumer {
            async fn start(&mut self, _ctx: super::ConsumerContext) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
        }

        let consumer = DummyConsumer;
        assert_eq!(
            consumer.startup_mode(),
            super::ConsumerStartupMode::Immediate
        );
    }

    #[test]
    fn test_startup_mode_explicit_override() {
        struct ExplicitConsumer;

        #[async_trait::async_trait]
        impl super::Consumer for ExplicitConsumer {
            async fn start(&mut self, _ctx: super::ConsumerContext) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
            fn startup_mode(&self) -> super::ConsumerStartupMode {
                super::ConsumerStartupMode::Explicit
            }
        }

        let consumer = ExplicitConsumer;
        assert_eq!(
            consumer.startup_mode(),
            super::ConsumerStartupMode::Explicit
        );
    }

    #[tokio::test]
    async fn test_startup_signal_mark_ready_resolves_receiver_ok() {
        let (signal, receiver) = StartupSignal::pair();
        // Not yet signalled — receiver should still be pending.
        assert!(matches!(*receiver.rx.borrow(), StartupState::Pending));

        // Mark ready — receiver must observe Ok.
        signal.mark_ready();
        let result = receiver.await_ready().await;
        assert!(result.is_ok(), "expected Ok after mark_ready");
    }

    #[tokio::test]
    async fn test_startup_signal_mark_failed_propagates_error() {
        let (signal, receiver) = StartupSignal::pair();
        signal.mark_failed("bind failed".to_string());
        let err = receiver
            .await_ready()
            .await
            .expect_err("expected Err after mark_failed");
        match err {
            CamelError::RouteError(msg) => assert!(msg.contains("bind failed")),
            other => panic!("expected RouteError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_startup_signal_idempotent_first_wins() {
        let (signal, receiver) = StartupSignal::pair();
        signal.mark_ready();
        // mark_failed after mark_ready must NOT override.
        signal.mark_failed("late failure".to_string());
        let result = receiver.await_ready().await;
        assert!(result.is_ok(), "first transition (Ready) wins");
    }

    #[tokio::test]
    async fn test_startup_receiver_immediate_is_pre_resolved_ok() {
        let receiver = StartupReceiver::immediate();
        let result = receiver.await_ready().await;
        assert!(result.is_ok(), "immediate receiver must resolve Ok");
    }

    #[tokio::test]
    async fn test_consumer_context_mark_ready_drives_signal() {
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(
            tx,
            CancellationToken::new(),
            "startup-test-route".to_string(),
        );
        let (signal, receiver) = StartupSignal::pair();
        let ctx = ctx.with_startup(signal);
        ctx.mark_ready();
        let result = receiver.await_ready().await;
        assert!(result.is_ok(), "ctx.mark_ready must resolve the receiver");
    }

    #[tokio::test]
    async fn test_startup_receiver_dropped_sender_returns_err() {
        // Build a signal/receiver pair and drop the signal without ever
        // transitioning — receiver must surface a contract-violation error.
        let (_signal, receiver) = StartupSignal::pair();
        drop(_signal);
        let err = receiver
            .await_ready()
            .await
            .expect_err("dropped signal must surface as Err");
        match err {
            CamelError::RouteError(msg) => assert!(msg.contains("dropped")),
            other => panic!("expected RouteError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_consumer_default_suspend_resume() {
        struct DummyConsumer;

        #[async_trait::async_trait]
        impl super::Consumer for DummyConsumer {
            async fn start(&mut self, _ctx: super::ConsumerContext) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
        }

        let consumer = DummyConsumer;
        assert!(consumer.suspend().await.is_ok());
        assert!(consumer.resume().await.is_ok());
    }

    // --- SecurityContext tests ---

    struct StubPolicy;

    #[async_trait::async_trait]
    impl SecurityPolicy for StubPolicy {
        async fn evaluate(
            &self,
            _exchange: &mut Exchange,
        ) -> Result<camel_api::security_policy::AuthorizationDecision, CamelError> {
            Ok(camel_api::security_policy::AuthorizationDecision::Granted {
                principal: camel_api::security_policy::Principal {
                    subject: "stub".into(),
                    issuer: "stub".into(),
                    audience: vec![],
                    scopes: vec![],
                    roles: vec![],
                    claims: serde_json::json!({}),
                },
            })
        }
    }

    struct StubAuthenticator;

    #[async_trait::async_trait]
    impl camel_auth::TokenAuthenticator for StubAuthenticator {
        async fn authenticate_bearer(
            &self,
            _token: &str,
        ) -> Result<camel_api::security_policy::Principal, CamelError> {
            Ok(camel_api::security_policy::Principal {
                subject: "stub".into(),
                issuer: "stub".into(),
                audience: vec![],
                scopes: vec![],
                roles: vec![],
                claims: serde_json::json!({}),
            })
        }
    }

    #[test]
    fn test_security_context_new() {
        let ctx = SecurityContext::new(StubPolicy, Arc::new(StubAuthenticator));
        assert!(Arc::strong_count(&ctx.policy) == 1);
        assert!(Arc::strong_count(&ctx.authenticator) == 1);
        assert_eq!(
            ctx.credential_sources,
            vec![camel_auth::CredentialSource::AuthorizationHeader]
        );
    }

    #[test]
    fn test_security_context_from_arc() {
        let policy: Arc<dyn SecurityPolicy> = Arc::new(StubPolicy);
        let authenticator: Arc<dyn camel_auth::TokenAuthenticator> = Arc::new(StubAuthenticator);
        let ctx = SecurityContext::from_arc(Arc::clone(&policy), Arc::clone(&authenticator));
        assert!(Arc::ptr_eq(&ctx.policy, &policy));
        assert!(Arc::ptr_eq(&ctx.authenticator, &authenticator));
        assert_eq!(
            ctx.credential_sources,
            vec![camel_auth::CredentialSource::AuthorizationHeader]
        );
    }

    #[test]
    fn test_security_context_clone_independent() {
        let ctx = SecurityContext::new(StubPolicy, Arc::new(StubAuthenticator));
        let cloned = ctx.clone();
        assert!(Arc::ptr_eq(&ctx.policy, &cloned.policy));
        assert!(Arc::ptr_eq(&ctx.authenticator, &cloned.authenticator));
        assert_eq!(ctx.credential_sources, cloned.credential_sources);
    }

    #[test]
    fn test_security_context_debug_redacts_traits() {
        let ctx = SecurityContext::new(StubPolicy, Arc::new(StubAuthenticator));
        let debug_str = format!("{ctx:?}");
        assert!(debug_str.contains("<SecurityPolicy>"));
        assert!(debug_str.contains("<TokenAuthenticator>"));
        assert!(debug_str.contains("credential_sources"));
    }

    #[test]
    fn test_security_context_with_credential_sources() {
        let ctx = SecurityContext::new(StubPolicy, Arc::new(StubAuthenticator))
            .with_credential_sources(vec![
                camel_auth::CredentialSource::Cookie {
                    name: "session".into(),
                },
                camel_auth::CredentialSource::AuthorizationHeader,
            ]);
        assert_eq!(ctx.credential_sources.len(), 2);
        assert!(matches!(
            &ctx.credential_sources[0],
            camel_auth::CredentialSource::Cookie { .. }
        ));
    }
}
