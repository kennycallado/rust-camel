pub mod bundle;
mod config;

pub use bundle::MasterBundle;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{CamelError, MetricsCollector, PlatformService};
use camel_component_api::{
    BoxProcessor, Component, ComponentContext, Consumer, ConsumerContext, Endpoint,
    ExchangeEnvelope, ProducerContext, parse_uri,
};
use camel_language_api::Language;
use tokio::task::JoinHandle;
use tokio::time::{interval, timeout};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::{MasterComponentConfig, MasterUriConfig};

const DELEGATE_RETRY_INTERVAL: Duration = Duration::from_millis(200);

pub struct MasterComponent {
    drain_timeout_ms: u64,
    delegate_retry_max_attempts: Option<u32>,
}

impl MasterComponent {
    pub fn new(config: MasterComponentConfig) -> Self {
        Self {
            drain_timeout_ms: config.drain_timeout_ms,
            delegate_retry_max_attempts: config.delegate_retry_max_attempts,
        }
    }
}

impl Default for MasterComponent {
    fn default() -> Self {
        Self::new(MasterComponentConfig::default())
    }
}

impl Component for MasterComponent {
    fn scheme(&self) -> &str {
        "master"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let parsed = MasterUriConfig::parse(uri)?;
        let delegate_parts = parse_uri(&parsed.delegate_uri)?;
        let delegate_scheme = delegate_parts.scheme;
        let delegate_component = ctx
            .resolve_component(&delegate_scheme)
            .ok_or_else(|| CamelError::ComponentNotFound(delegate_scheme.clone()))?;

        Ok(Box::new(MasterEndpoint {
            uri: uri.to_string(),
            lock_name: parsed.lock_name,
            delegate_uri: parsed.delegate_uri,
            delegate_component,
            metrics: ctx.metrics(),
            platform_service: ctx.platform_service(),
            drain_timeout: Duration::from_millis(self.drain_timeout_ms),
            delegate_retry_max_attempts: self.delegate_retry_max_attempts,
        }))
    }
}

struct MasterEndpoint {
    uri: String,
    lock_name: String,
    delegate_uri: String,
    delegate_component: Arc<dyn Component>,
    metrics: Arc<dyn MetricsCollector>,
    platform_service: Arc<dyn PlatformService>,
    drain_timeout: Duration,
    delegate_retry_max_attempts: Option<u32>,
}

impl Endpoint for MasterEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(MasterConsumer::new(
            self.lock_name.clone(),
            self.delegate_uri.clone(),
            Arc::clone(&self.delegate_component),
            Arc::clone(&self.metrics),
            Arc::clone(&self.platform_service),
            self.drain_timeout,
            self.delegate_retry_max_attempts,
        )))
    }

    fn create_producer(&self, ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        let delegate_ctx = MasterDelegateContext {
            delegate_component: Arc::clone(&self.delegate_component),
            metrics: Arc::clone(&self.metrics),
            platform_service: Arc::clone(&self.platform_service),
        };

        self.delegate_component
            .create_endpoint(&self.delegate_uri, &delegate_ctx)?
            .create_producer(ctx)
    }
}

struct MasterDelegateContext {
    delegate_component: Arc<dyn Component>,
    metrics: Arc<dyn MetricsCollector>,
    platform_service: Arc<dyn PlatformService>,
}

impl ComponentContext for MasterDelegateContext {
    fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
        if self.delegate_component.scheme() == scheme {
            Some(Arc::clone(&self.delegate_component))
        } else {
            None
        }
    }

    fn resolve_language(&self, _name: &str) -> Option<Arc<dyn Language>> {
        None
    }

    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::clone(&self.metrics)
    }

    fn platform_service(&self) -> Arc<dyn PlatformService> {
        Arc::clone(&self.platform_service)
    }
}

struct MasterConsumer {
    lock_name: String,
    delegate_uri: String,
    delegate_component: Arc<dyn Component>,
    metrics: Arc<dyn MetricsCollector>,
    platform_service: Arc<dyn PlatformService>,
    drain_timeout: Duration,
    delegate_retry_max_attempts: Option<u32>,
    leadership_task: Option<JoinHandle<()>>,
    stop_token: Option<CancellationToken>,
}

impl MasterConsumer {
    fn new(
        lock_name: String,
        delegate_uri: String,
        delegate_component: Arc<dyn Component>,
        metrics: Arc<dyn MetricsCollector>,
        platform_service: Arc<dyn PlatformService>,
        drain_timeout: Duration,
        delegate_retry_max_attempts: Option<u32>,
    ) -> Self {
        Self {
            lock_name,
            delegate_uri,
            delegate_component,
            metrics,
            platform_service,
            drain_timeout,
            delegate_retry_max_attempts,
            leadership_task: None,
            stop_token: None,
        }
    }
}

enum DelegateState {
    Inactive,
    Active {
        run_token: CancellationToken,
        handle: JoinHandle<()>,
    },
}

async fn stop_delegate(state: &mut DelegateState, drain_timeout: Duration) {
    if let DelegateState::Active {
        run_token,
        mut handle,
    } = std::mem::replace(state, DelegateState::Inactive)
    {
        run_token.cancel();
        match timeout(drain_timeout, &mut handle).await {
            Ok(_) => {}
            Err(_) => {
                warn!("master delegate shutdown timed out, aborting");
                handle.abort();
            }
        }
    }
}

struct ReconcileContext<'a> {
    lock_name: &'a str,
    delegate_component: &'a Arc<dyn Component>,
    delegate_uri: &'a str,
    sender: &'a tokio::sync::mpsc::Sender<ExchangeEnvelope>,
    parent_cancel: &'a CancellationToken,
    drain_timeout: Duration,
    metrics: &'a Arc<dyn MetricsCollector>,
    platform_service: &'a Arc<dyn PlatformService>,
}

async fn reconcile_event(
    event: camel_api::LeadershipEvent,
    state: &mut DelegateState,
    ctx: &ReconcileContext<'_>,
) {
    match event {
        camel_api::LeadershipEvent::StartedLeading => {
            info!(lock = %ctx.lock_name, "master leadership acquired");
            stop_delegate(state, ctx.drain_timeout).await;

            let delegate_ctx = MasterDelegateContext {
                delegate_component: Arc::clone(ctx.delegate_component),
                metrics: Arc::clone(ctx.metrics),
                platform_service: Arc::clone(ctx.platform_service),
            };

            let endpoint = match ctx
                .delegate_component
                .create_endpoint(ctx.delegate_uri, &delegate_ctx)
            {
                Ok(endpoint) => endpoint,
                Err(err) => {
                    warn!(lock = %ctx.lock_name, "failed to create delegate endpoint: {err}");
                    return;
                }
            };

            let mut consumer = match endpoint.create_consumer() {
                Ok(consumer) => consumer,
                Err(err) => {
                    warn!(lock = %ctx.lock_name, "failed to create delegate consumer: {err}");
                    return;
                }
            };

            let run_token = ctx.parent_cancel.child_token();
            let delegate_ctx = ConsumerContext::new(ctx.sender.clone(), run_token.clone());
            let handle = tokio::spawn(async move {
                let _ = consumer.start(delegate_ctx).await;
                let _ = consumer.stop().await;
            });

            *state = DelegateState::Active { run_token, handle };
        }
        camel_api::LeadershipEvent::StoppedLeading => {
            info!(lock = %ctx.lock_name, "master leadership lost");
            stop_delegate(state, ctx.drain_timeout).await;
        }
    }
}

#[async_trait]
impl Consumer for MasterConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        if self.leadership_task.is_some() {
            return Ok(());
        }

        let handle = self
            .platform_service
            .leadership()
            .start(&self.lock_name)
            .await
            .map_err(|e| {
                CamelError::EndpointCreationFailed(format!("failed to start leader election: {e}"))
            })?;

        let lock_name = self.lock_name.clone();
        let delegate_uri = self.delegate_uri.clone();
        let delegate_component = Arc::clone(&self.delegate_component);
        let metrics = Arc::clone(&self.metrics);
        let platform_service = Arc::clone(&self.platform_service);
        let sender = context.sender();
        let parent_cancel = context.cancel_token();
        let drain_timeout = self.drain_timeout;
        let delegate_retry_max_attempts = self.delegate_retry_max_attempts;
        let mut events = handle.events.clone();

        let stop_token = CancellationToken::new();
        let stop_token_loop = stop_token.clone();
        let leadership_handle = handle;

        let task = tokio::spawn(async move {
            let mut state = DelegateState::Inactive;
            let mut is_leading = false;
            let mut delegate_attempts = 0u32;
            let mut retry_tick = interval(DELEGATE_RETRY_INTERVAL);

            let rctx = ReconcileContext {
                lock_name: &lock_name,
                delegate_component: &delegate_component,
                delegate_uri: &delegate_uri,
                sender: &sender,
                parent_cancel: &parent_cancel,
                drain_timeout,
                metrics: &metrics,
                platform_service: &platform_service,
            };

            let initial_event = { events.borrow().clone() };
            if let Some(initial_event) = initial_event {
                is_leading = matches!(&initial_event, camel_api::LeadershipEvent::StartedLeading);
                if is_leading {
                    delegate_attempts = 0;
                }
                reconcile_event(initial_event, &mut state, &rctx).await;
            }

            loop {
                tokio::select! {
                    _ = stop_token_loop.cancelled() => {
                        break;
                    }
                    _ = context.cancelled() => {
                        break;
                    }
                    changed = events.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let event = { events.borrow().clone() };
                        if let Some(event) = event {
                            let was_leading = is_leading;
                            is_leading = matches!(&event, camel_api::LeadershipEvent::StartedLeading);
                            if !was_leading && is_leading {
                                delegate_attempts = 0;
                            }
                            reconcile_event(event, &mut state, &rctx).await;
                        }
                    }
                    _ = retry_tick.tick() => {
                        if is_leading && matches!(state, DelegateState::Inactive) {
                            if let Some(max) = delegate_retry_max_attempts {
                                delegate_attempts = delegate_attempts.saturating_add(1);
                                if delegate_attempts > max {
                                    warn!(
                                        lock = %lock_name,
                                        attempts = max,
                                        "delegate start exceeded max attempts, stopping consumer"
                                    );
                                    break;
                                }
                            }
                            reconcile_event(
                                camel_api::LeadershipEvent::StartedLeading,
                                &mut state,
                                &rctx,
                            )
                            .await;
                        }
                    }
                }
            }

            stop_delegate(&mut state, drain_timeout).await;
            let _ = timeout(drain_timeout, leadership_handle.step_down()).await;
        });

        self.stop_token = Some(stop_token);
        self.leadership_task = Some(task);

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(token) = self.stop_token.take() {
            token.cancel();
        }

        if let Some(task) = self.leadership_task.take()
            && timeout(self.drain_timeout, task).await.is_err()
        {
            warn!("master leadership loop shutdown timed out");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use camel_api::{
        BoxProcessorExt, Exchange, LeadershipEvent, LeadershipHandle, LeadershipService, Message,
        NoOpMetrics, NoopPlatformService, NoopReadinessGate, PlatformError, PlatformIdentity,
        PlatformService, ReadinessGate,
    };
    use camel_component_api::NoOpComponentContext;
    use tokio::sync::{oneshot, watch};
    use tokio::time::{sleep, timeout};
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn parse_master_uri_valid() {
        let cfg = MasterUriConfig::parse("master:mylock:timer:tick?period=250").unwrap();
        assert_eq!(cfg.lock_name, "mylock");
        assert_eq!(cfg.delegate_uri, "timer:tick?period=250");
    }

    #[test]
    fn parse_master_uri_missing_lockname() {
        let err = MasterUriConfig::parse("master::timer:tick").unwrap_err();
        assert!(matches!(err, CamelError::InvalidUri(_)));
    }

    #[test]
    fn parse_master_uri_missing_delegate() {
        let err = MasterUriConfig::parse("master:mylock:").unwrap_err();
        assert!(matches!(err, CamelError::InvalidUri(_)));
    }

    #[test]
    fn endpoint_fails_when_delegate_component_missing() {
        let master = MasterComponent::default();
        let result =
            master.create_endpoint("master:lock-1:missing:delegate", &NoOpComponentContext);
        assert!(matches!(result, Err(CamelError::ComponentNotFound(_))));
    }

    #[test]
    fn delegate_scheme_is_parsed_from_delegate_uri() {
        let seen_scheme = Arc::new(AtomicBool::new(false));

        struct SchemeAwareContext {
            delegate: Arc<dyn Component>,
            seen_scheme: Arc<AtomicBool>,
        }

        impl ComponentContext for SchemeAwareContext {
            fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
                if scheme == "mock" {
                    self.seen_scheme.store(true, Ordering::SeqCst);
                    Some(Arc::clone(&self.delegate))
                } else {
                    None
                }
            }

            fn resolve_language(&self, _name: &str) -> Option<Arc<dyn Language>> {
                None
            }

            fn metrics(&self) -> Arc<dyn MetricsCollector> {
                Arc::new(NoOpMetrics)
            }

            fn platform_service(&self) -> Arc<dyn PlatformService> {
                Arc::new(NoopPlatformService::default())
            }
        }

        struct MockDelegateComponent;

        impl Component for MockDelegateComponent {
            fn scheme(&self) -> &str {
                "mock"
            }

            fn create_endpoint(
                &self,
                _uri: &str,
                _ctx: &dyn ComponentContext,
            ) -> Result<Box<dyn Endpoint>, CamelError> {
                Ok(Box::new(MockDelegateEndpoint))
            }
        }

        struct MockDelegateEndpoint;

        impl Endpoint for MockDelegateEndpoint {
            fn uri(&self) -> &str {
                "mock:delegate"
            }

            fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
                Err(CamelError::EndpointCreationFailed("not used".to_string()))
            }

            fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
                Err(CamelError::EndpointCreationFailed("not used".to_string()))
            }
        }

        let delegate = Arc::new(MockDelegateComponent);
        let ctx = SchemeAwareContext {
            delegate,
            seen_scheme: Arc::clone(&seen_scheme),
        };

        let master = MasterComponent::default();
        let endpoint = master
            .create_endpoint("master:mylock:mock:delegate?x=1", &ctx)
            .unwrap();

        assert_eq!(endpoint.uri(), "master:mylock:mock:delegate?x=1");
        assert!(seen_scheme.load(Ordering::SeqCst));
    }

    struct MockDelegateContext {
        delegate: Arc<dyn Component>,
    }

    impl ComponentContext for MockDelegateContext {
        fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
            if self.delegate.scheme() == scheme {
                Some(Arc::clone(&self.delegate))
            } else {
                None
            }
        }

        fn resolve_language(&self, _name: &str) -> Option<Arc<dyn Language>> {
            None
        }

        fn metrics(&self) -> Arc<dyn MetricsCollector> {
            Arc::new(NoOpMetrics)
        }

        fn platform_service(&self) -> Arc<dyn PlatformService> {
            Arc::new(NoopPlatformService::default())
        }
    }

    struct MockProducerDelegateComponent {
        create_endpoint_calls: Arc<AtomicUsize>,
        create_producer_calls: Arc<AtomicUsize>,
        fail_producer: bool,
    }

    impl Component for MockProducerDelegateComponent {
        fn scheme(&self) -> &str {
            "mock"
        }

        fn create_endpoint(
            &self,
            _uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            self.create_endpoint_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(MockProducerDelegateEndpoint {
                create_producer_calls: Arc::clone(&self.create_producer_calls),
                fail_producer: self.fail_producer,
            }))
        }
    }

    struct MockProducerDelegateEndpoint {
        create_producer_calls: Arc<AtomicUsize>,
        fail_producer: bool,
    }

    impl Endpoint for MockProducerDelegateEndpoint {
        fn uri(&self) -> &str {
            "mock:delegate"
        }

        fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
            Err(CamelError::EndpointCreationFailed(
                "not used in test".to_string(),
            ))
        }

        fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
            self.create_producer_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_producer {
                return Err(CamelError::ProcessorError(
                    "delegate producer failed".to_string(),
                ));
            }
            Ok(BoxProcessor::from_fn(
                |exchange| async move { Ok(exchange) },
            ))
        }
    }

    #[tokio::test]
    async fn producer_passthrough_delegates_and_produces() {
        let endpoint_calls = Arc::new(AtomicUsize::new(0));
        let producer_calls = Arc::new(AtomicUsize::new(0));
        let delegate = Arc::new(MockProducerDelegateComponent {
            create_endpoint_calls: Arc::clone(&endpoint_calls),
            create_producer_calls: Arc::clone(&producer_calls),
            fail_producer: false,
        });

        let ctx = MockDelegateContext {
            delegate: delegate.clone(),
        };

        let master = MasterComponent::default();
        let endpoint = master
            .create_endpoint("master:lock-1:mock:delegate", &ctx)
            .unwrap();
        let producer_ctx = ProducerContext::new();
        let producer = endpoint.create_producer(&producer_ctx).unwrap();

        let exchange = Exchange::new(Message::new("ok"));
        let result = producer.oneshot(exchange).await.unwrap();

        assert_eq!(result.input.body.as_text(), Some("ok"));
        assert_eq!(endpoint_calls.load(Ordering::SeqCst), 1);
        assert_eq!(producer_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn producer_passthrough_bubbles_delegate_errors() {
        let endpoint_calls = Arc::new(AtomicUsize::new(0));
        let producer_calls = Arc::new(AtomicUsize::new(0));
        let delegate = Arc::new(MockProducerDelegateComponent {
            create_endpoint_calls: Arc::clone(&endpoint_calls),
            create_producer_calls: Arc::clone(&producer_calls),
            fail_producer: true,
        });

        let ctx = MockDelegateContext {
            delegate: delegate.clone(),
        };

        let master = MasterComponent::default();
        let endpoint = master
            .create_endpoint("master:lock-1:mock:delegate", &ctx)
            .unwrap();
        let producer_ctx = ProducerContext::new();
        let err = endpoint.create_producer(&producer_ctx).unwrap_err();

        assert!(matches!(err, CamelError::ProcessorError(_)));
        assert_eq!(endpoint_calls.load(Ordering::SeqCst), 1);
        assert_eq!(producer_calls.load(Ordering::SeqCst), 1);
    }

    struct FakeLeadershipService {
        tx: Mutex<Option<watch::Sender<Option<LeadershipEvent>>>>,
        is_leader: Arc<AtomicBool>,
        initial: Option<LeadershipEvent>,
    }

    impl FakeLeadershipService {
        fn new(initial: Option<LeadershipEvent>) -> Self {
            let starts_as_leader = matches!(initial, Some(LeadershipEvent::StartedLeading));
            Self {
                tx: Mutex::new(None),
                is_leader: Arc::new(AtomicBool::new(starts_as_leader)),
                initial,
            }
        }

        async fn emit(&self, event: LeadershipEvent) {
            self.is_leader.store(
                matches!(event, LeadershipEvent::StartedLeading),
                Ordering::Release,
            );
            if let Some(tx) = self
                .tx
                .lock()
                .expect("mutex poisoned: fake elector sender")
                .as_ref()
            {
                let _ = tx.send(Some(event));
            }
        }
    }

    #[async_trait]
    impl LeadershipService for FakeLeadershipService {
        async fn start(&self, _lock_name: &str) -> Result<LeadershipHandle, PlatformError> {
            let (tx, rx) = watch::channel(self.initial.clone());
            *self.tx.lock().expect("mutex poisoned: fake elector sender") = Some(tx);

            let cancel = CancellationToken::new();
            let cancel_wait = cancel.clone();
            let (term_tx, term_rx) = oneshot::channel();
            tokio::spawn(async move {
                cancel_wait.cancelled().await;
                let _ = term_tx.send(());
            });

            Ok(LeadershipHandle::new(
                rx,
                Arc::clone(&self.is_leader),
                cancel,
                term_rx,
            ))
        }
    }

    struct FakePlatformService {
        identity: PlatformIdentity,
        readiness_gate: Arc<dyn ReadinessGate>,
        leadership: Arc<dyn LeadershipService>,
    }

    impl FakePlatformService {
        fn new(leadership: Arc<dyn LeadershipService>) -> Self {
            Self {
                identity: PlatformIdentity::local("master-tests"),
                readiness_gate: Arc::new(NoopReadinessGate),
                leadership,
            }
        }
    }

    impl PlatformService for FakePlatformService {
        fn identity(&self) -> PlatformIdentity {
            self.identity.clone()
        }

        fn readiness_gate(&self) -> Arc<dyn ReadinessGate> {
            Arc::clone(&self.readiness_gate)
        }

        fn leadership(&self) -> Arc<dyn LeadershipService> {
            Arc::clone(&self.leadership)
        }
    }

    struct FakeDelegateComponent {
        create_consumer_calls: Arc<AtomicUsize>,
        start_calls: Arc<AtomicUsize>,
    }

    impl Component for FakeDelegateComponent {
        fn scheme(&self) -> &str {
            "fake"
        }

        fn create_endpoint(
            &self,
            _uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(FakeDelegateEndpoint {
                create_consumer_calls: Arc::clone(&self.create_consumer_calls),
                start_calls: Arc::clone(&self.start_calls),
            }))
        }
    }

    struct FakeDelegateEndpoint {
        create_consumer_calls: Arc<AtomicUsize>,
        start_calls: Arc<AtomicUsize>,
    }

    impl Endpoint for FakeDelegateEndpoint {
        fn uri(&self) -> &str {
            "fake:delegate"
        }

        fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
            let epoch = self.create_consumer_calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(Box::new(FakeDelegateConsumer {
                epoch,
                start_calls: Arc::clone(&self.start_calls),
            }))
        }

        fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
            Err(CamelError::EndpointCreationFailed("not used".to_string()))
        }
    }

    struct FakeDelegateConsumer {
        epoch: usize,
        start_calls: Arc<AtomicUsize>,
    }

    struct FailingDelegateComponent {
        create_endpoint_calls: Arc<AtomicUsize>,
    }

    impl Component for FailingDelegateComponent {
        fn scheme(&self) -> &str {
            "failing"
        }

        fn create_endpoint(
            &self,
            _uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            self.create_endpoint_calls.fetch_add(1, Ordering::SeqCst);
            Err(CamelError::EndpointCreationFailed(
                "delegate endpoint creation failed".to_string(),
            ))
        }
    }

    #[async_trait]
    impl Consumer for FakeDelegateConsumer {
        async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
            self.start_calls.fetch_add(1, Ordering::SeqCst);
            context
                .send(Exchange::new(Message::new(format!("epoch-{}", self.epoch))))
                .await?;

            loop {
                tokio::select! {
                    _ = context.cancelled() => {
                        break;
                    }
                    _ = sleep(Duration::from_millis(20)) => {
                        context
                            .send(Exchange::new(Message::new(format!("epoch-{}", self.epoch))))
                            .await?;
                    }
                }
            }

            Ok(())
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    fn build_master_consumer(
        platform_service: Arc<dyn PlatformService>,
        create_consumer_calls: Arc<AtomicUsize>,
        start_calls: Arc<AtomicUsize>,
        delegate_retry_max_attempts: Option<u32>,
    ) -> MasterConsumer {
        MasterConsumer::new(
            "lock-a".to_string(),
            "fake:delegate".to_string(),
            Arc::new(FakeDelegateComponent {
                create_consumer_calls,
                start_calls,
            }),
            Arc::new(NoOpMetrics),
            platform_service,
            Duration::from_millis(500),
            delegate_retry_max_attempts,
        )
    }

    #[tokio::test]
    async fn starts_delegate_only_after_started_leading() {
        let leadership = Arc::new(FakeLeadershipService::new(None));
        let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
        let create_consumer_calls = Arc::new(AtomicUsize::new(0));
        let start_calls = Arc::new(AtomicUsize::new(0));
        let mut master = build_master_consumer(
            platform_service,
            Arc::clone(&create_consumer_calls),
            Arc::clone(&start_calls),
            Some(30),
        );

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel.clone());

        master.start(ctx).await.unwrap();

        sleep(Duration::from_millis(80)).await;
        assert!(rx.try_recv().is_err());
        assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 0);

        leadership.emit(LeadershipEvent::StartedLeading).await;

        let first = timeout(Duration::from_millis(500), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.exchange.input.body.as_text(), Some("epoch-1"));
        assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 1);
        assert_eq!(start_calls.load(Ordering::SeqCst), 1);

        cancel.cancel();
        master.stop().await.unwrap();
    }

    #[tokio::test]
    async fn stops_delegate_on_stopped_leading() {
        let leadership = Arc::new(FakeLeadershipService::new(None));
        let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
        let create_consumer_calls = Arc::new(AtomicUsize::new(0));
        let start_calls = Arc::new(AtomicUsize::new(0));
        let mut master = build_master_consumer(
            platform_service,
            Arc::clone(&create_consumer_calls),
            Arc::clone(&start_calls),
            Some(30),
        );

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel.clone());

        master.start(ctx).await.unwrap();
        leadership.emit(LeadershipEvent::StartedLeading).await;
        let _ = timeout(Duration::from_millis(500), rx.recv())
            .await
            .unwrap()
            .unwrap();

        leadership.emit(LeadershipEvent::StoppedLeading).await;
        sleep(Duration::from_millis(100)).await;
        while rx.try_recv().is_ok() {}
        assert!(
            timeout(Duration::from_millis(120), rx.recv())
                .await
                .is_err()
        );

        cancel.cancel();
        master.stop().await.unwrap();
    }

    #[tokio::test]
    async fn recreates_delegate_on_new_leadership_epoch() {
        let leadership = Arc::new(FakeLeadershipService::new(None));
        let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
        let create_consumer_calls = Arc::new(AtomicUsize::new(0));
        let start_calls = Arc::new(AtomicUsize::new(0));
        let mut master = build_master_consumer(
            platform_service,
            Arc::clone(&create_consumer_calls),
            Arc::clone(&start_calls),
            Some(30),
        );

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel.clone());

        master.start(ctx).await.unwrap();

        leadership.emit(LeadershipEvent::StartedLeading).await;
        let first = timeout(Duration::from_millis(500), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.exchange.input.body.as_text(), Some("epoch-1"));

        leadership.emit(LeadershipEvent::StoppedLeading).await;
        sleep(Duration::from_millis(120)).await;

        leadership.emit(LeadershipEvent::StartedLeading).await;
        let second = timeout(Duration::from_millis(500), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.exchange.input.body.as_text(), Some("epoch-2"));

        assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 2);
        assert_eq!(start_calls.load(Ordering::SeqCst), 2);

        cancel.cancel();
        master.stop().await.unwrap();
    }

    #[tokio::test]
    async fn stops_retrying_delegate_start_after_max_attempts() {
        let leadership = Arc::new(FakeLeadershipService::new(Some(
            LeadershipEvent::StartedLeading,
        )));
        let platform_service = Arc::new(FakePlatformService::new(leadership));
        let create_endpoint_calls = Arc::new(AtomicUsize::new(0));

        let mut master = MasterConsumer::new(
            "lock-a".to_string(),
            "failing:delegate".to_string(),
            Arc::new(FailingDelegateComponent {
                create_endpoint_calls: Arc::clone(&create_endpoint_calls),
            }),
            Arc::new(NoOpMetrics),
            platform_service,
            Duration::from_millis(500),
            Some(1),
        );

        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel.clone());

        master.start(ctx).await.unwrap();
        sleep(Duration::from_millis(750)).await;

        assert_eq!(create_endpoint_calls.load(Ordering::SeqCst), 2);

        cancel.cancel();
        master.stop().await.unwrap();
    }
}
