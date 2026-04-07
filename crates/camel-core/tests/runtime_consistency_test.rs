use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use camel_api::{
    CamelError, CanonicalRouteSpec, RuntimeCommand, RuntimeCommandBus, RuntimeQuery,
    RuntimeQueryResult, SupervisionConfig,
};
use camel_component_api::{Component, ConcurrencyModel, Consumer, ConsumerContext, Endpoint};
use camel_component_timer::TimerComponent;
use camel_core::{CamelContext, RouteDefinition};
use camel_core::{
    InMemoryCommandDedup, InMemoryEventPublisher, InMemoryProjectionStore, InMemoryRouteRepository,
    RuntimeBus,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_start_commands_preserve_single_winner_semantics() {
    let runtime = Arc::new(RuntimeBus::new(
        Arc::new(InMemoryRouteRepository::default()),
        Arc::new(InMemoryProjectionStore::default()),
        Arc::new(InMemoryEventPublisher::default()),
        Arc::new(InMemoryCommandDedup::default()),
    ));

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("concurrent-r1", "timer:tick"),
            command_id: "cmd-register".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    let mut handles = Vec::new();
    for i in 0..12 {
        let rt = Arc::clone(&runtime);
        handles.push(tokio::spawn(async move {
            rt.execute(RuntimeCommand::StartRoute {
                route_id: "concurrent-r1".into(),
                command_id: format!("cmd-start-{i}"),
                causation_id: Some("cmd-register".into()),
            })
            .await
        }));
    }

    let mut ok = 0usize;
    let mut conflicts = 0usize;
    for handle in handles {
        let result = handle.await.expect("task join failed");
        match result {
            Ok(_) => ok += 1,
            Err(err) => {
                let text = err.to_string();
                assert!(
                    text.contains("optimistic lock conflict")
                        || text.contains("invalid transition"),
                    "unexpected concurrent error: {text}"
                );
                conflicts += 1;
            }
        }
    }

    assert_eq!(ok, 1, "exactly one start should succeed");
    assert_eq!(ok + conflicts, 12);

    let aggregate = runtime.repo().load("concurrent-r1").await.unwrap().unwrap();
    assert!(matches!(
        aggregate.state(),
        camel_core::RouteRuntimeState::Started
    ));
}

#[tokio::test]
async fn connected_runtime_query_reads_projection_source_of_truth() {
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    let runtime = ctx.runtime();

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("rq-projection", "timer:tick"),
            command_id: "c1".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    let out = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "rq-projection".into(),
        })
        .await
        .unwrap();

    match out {
        RuntimeQueryResult::RouteStatus { status, .. } => {
            assert_eq!(status, "Registered");
        }
        other => panic!("unexpected query result: {other:?}"),
    }
}

struct CrashingConsumer;

#[async_trait]
impl Consumer for CrashingConsumer {
    async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
        Err(CamelError::RouteError("boom".into()))
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
}

struct CrashingEndpoint;

impl Endpoint for CrashingEndpoint {
    fn uri(&self) -> &str {
        "crash:test"
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(CrashingConsumer))
    }

    fn create_producer(
        &self,
        _ctx: &camel_api::ProducerContext,
    ) -> Result<camel_api::BoxProcessor, CamelError> {
        Err(CamelError::RouteError("no producer".into()))
    }
}

struct CrashingComponent;

impl Component for CrashingComponent {
    fn scheme(&self) -> &str {
        "crash"
    }

    fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(CrashingEndpoint))
    }
}

struct CrashOnceThenHoldConsumer {
    starts: Arc<AtomicU32>,
}

#[async_trait]
impl Consumer for CrashOnceThenHoldConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        if self.starts.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(CamelError::RouteError("first run crash".into()));
        }
        ctx.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
}

struct CrashOnceThenHoldEndpoint {
    starts: Arc<AtomicU32>,
}

impl Endpoint for CrashOnceThenHoldEndpoint {
    fn uri(&self) -> &str {
        "crash-once-hold:test"
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(CrashOnceThenHoldConsumer {
            starts: Arc::clone(&self.starts),
        }))
    }

    fn create_producer(
        &self,
        _ctx: &camel_api::ProducerContext,
    ) -> Result<camel_api::BoxProcessor, CamelError> {
        Err(CamelError::RouteError("no producer".into()))
    }
}

struct CrashOnceThenHoldComponent {
    starts: Arc<AtomicU32>,
}

impl Component for CrashOnceThenHoldComponent {
    fn scheme(&self) -> &str {
        "crash-once-hold"
    }

    fn create_endpoint(&self, _uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(CrashOnceThenHoldEndpoint {
            starts: Arc::clone(&self.starts),
        }))
    }
}

#[tokio::test]
async fn consumer_crash_updates_runtime_projection_to_failed() {
    let mut ctx = CamelContext::new();
    ctx.register_component(CrashingComponent);
    let runtime = ctx.runtime();

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("crash-sync-r1", "crash:test"),
            command_id: "c-register".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "crash-sync-r1".into(),
            command_id: "c-start".into(),
            causation_id: Some("c-register".into()),
        })
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let out = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "crash-sync-r1".into(),
        })
        .await
        .unwrap();

    match out {
        RuntimeQueryResult::RouteStatus { status, .. } => {
            assert_eq!(status, "Failed");
        }
        other => panic!("unexpected query result: {other:?}"),
    }
}

#[tokio::test]
async fn supervision_restart_updates_runtime_projection_to_started() {
    let starts = Arc::new(AtomicU32::new(0));
    let mut ctx = CamelContext::with_supervision(SupervisionConfig {
        max_attempts: Some(5),
        initial_delay: Duration::from_millis(30),
        backoff_multiplier: 1.0,
        max_delay: Duration::from_secs(1),
    });
    ctx.register_component(CrashOnceThenHoldComponent {
        starts: Arc::clone(&starts),
    });
    ctx.add_route_definition(
        RouteDefinition::new("crash-once-hold:test", vec![]).with_route_id("supervised-r1"),
    )
    .await
    .unwrap();
    ctx.start().await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let status = match ctx
            .runtime()
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: "supervised-r1".into(),
            })
            .await
            .unwrap()
        {
            RuntimeQueryResult::RouteStatus { status, .. } => status,
            other => panic!("unexpected query result: {other:?}"),
        };

        if starts.load(Ordering::SeqCst) >= 2 && status == "Started" {
            break;
        }

        assert!(
            Instant::now() <= deadline,
            "expected supervised route to recover to Started via runtime projection; last status={status}, starts={}",
            starts.load(Ordering::SeqCst)
        );

        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    ctx.stop().await.unwrap();
}

#[tokio::test]
async fn supervision_respects_runtime_stopped_state_and_skips_restart() {
    let starts = Arc::new(AtomicU32::new(0));
    let mut ctx = CamelContext::with_supervision(SupervisionConfig {
        max_attempts: Some(5),
        initial_delay: Duration::from_millis(200),
        backoff_multiplier: 1.0,
        max_delay: Duration::from_secs(1),
    });
    ctx.register_component(CrashOnceThenHoldComponent {
        starts: Arc::clone(&starts),
    });
    ctx.add_route_definition(
        RouteDefinition::new("crash-once-hold:test", vec![]).with_route_id("supervised-stop-r1"),
    )
    .await
    .unwrap();
    ctx.start().await.unwrap();

    let fail_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let status = match ctx
            .runtime()
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: "supervised-stop-r1".into(),
            })
            .await
            .unwrap()
        {
            RuntimeQueryResult::RouteStatus { status, .. } => status,
            other => panic!("unexpected query result: {other:?}"),
        };

        if status == "Failed" {
            break;
        }

        assert!(
            Instant::now() <= fail_deadline,
            "expected route to crash into Failed state before manual stop; last status={status}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    ctx.runtime()
        .execute(RuntimeCommand::StopRoute {
            route_id: "supervised-stop-r1".into(),
            command_id: "manual-stop-after-crash".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(350)).await;

    let status = match ctx
        .runtime()
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "supervised-stop-r1".into(),
        })
        .await
        .unwrap()
    {
        RuntimeQueryResult::RouteStatus { status, .. } => status,
        other => panic!("unexpected query result: {other:?}"),
    };
    assert_eq!(status, "Stopped");
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "supervision must not restart a route manually transitioned to Stopped in runtime state"
    );

    ctx.stop().await.unwrap();
}
