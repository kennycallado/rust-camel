use crate::lifecycle::domain::DomainError;
use crate::lifecycle::domain::RouteRuntimeState;

use super::*;
use crate::health_registry::HealthCheckRegistry;
use crate::lifecycle::ports::InFlightCountResult;
use camel_api::{AsyncHealthCheck, CheckResult, HealthStatus};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::lifecycle::domain::RuntimeEvent;
use async_trait::async_trait;

#[derive(Clone, Default)]
struct InMemoryTestRepo {
    routes: Arc<Mutex<HashMap<String, RouteRuntimeAggregate>>>,
}

struct AlwaysHealthyCheck {
    check_name: String,
}

#[async_trait]
impl AsyncHealthCheck for AlwaysHealthyCheck {
    fn name(&self) -> &str {
        &self.check_name
    }

    async fn check(&self) -> CheckResult {
        CheckResult::healthy(&self.check_name)
    }
}

fn healthy_check(name: &str) -> Arc<dyn AsyncHealthCheck> {
    Arc::new(AlwaysHealthyCheck {
        check_name: name.to_string(),
    })
}

#[async_trait]
impl RouteRepositoryPort for InMemoryTestRepo {
    async fn load(&self, route_id: &str) -> Result<Option<RouteRuntimeAggregate>, DomainError> {
        Ok(self
            .routes
            .lock()
            .expect("lock test routes")
            .get(route_id)
            .cloned())
    }

    async fn save(&self, aggregate: RouteRuntimeAggregate) -> Result<(), DomainError> {
        self.routes
            .lock()
            .expect("lock test routes")
            .insert(aggregate.route_id().to_string(), aggregate);
        Ok(())
    }

    async fn save_if_version(
        &self,
        aggregate: RouteRuntimeAggregate,
        expected_version: u64,
    ) -> Result<(), DomainError> {
        let route_id = aggregate.route_id().to_string();
        let mut routes = self.routes.lock().expect("lock test routes");
        let current = routes.get(&route_id).ok_or_else(|| {
            DomainError::InvalidState(format!(
                "optimistic lock conflict for route '{route_id}': route not found"
            ))
        })?;

        if current.version() != expected_version {
            return Err(DomainError::InvalidState(format!(
                "optimistic lock conflict for route '{route_id}': expected version {expected_version}, actual {}",
                current.version()
            )));
        }

        routes.insert(route_id, aggregate);
        Ok(())
    }

    async fn delete(&self, route_id: &str) -> Result<(), DomainError> {
        self.routes
            .lock()
            .expect("lock test routes")
            .remove(route_id);
        Ok(())
    }
}

#[derive(Clone, Default)]
struct InMemoryTestProjectionStore {
    statuses: Arc<Mutex<HashMap<String, RouteStatusProjection>>>,
}

#[async_trait]
impl ProjectionStorePort for InMemoryTestProjectionStore {
    async fn upsert_status(&self, status: RouteStatusProjection) -> Result<(), DomainError> {
        self.statuses
            .lock()
            .expect("lock test statuses")
            .insert(status.route_id.clone(), status);
        Ok(())
    }

    async fn get_status(
        &self,
        route_id: &str,
    ) -> Result<Option<RouteStatusProjection>, DomainError> {
        Ok(self
            .statuses
            .lock()
            .expect("lock test statuses")
            .get(route_id)
            .cloned())
    }

    async fn list_statuses(&self) -> Result<Vec<RouteStatusProjection>, DomainError> {
        Ok(self
            .statuses
            .lock()
            .expect("lock test statuses")
            .values()
            .cloned()
            .collect())
    }

    async fn remove_status(&self, route_id: &str) -> Result<(), DomainError> {
        self.statuses
            .lock()
            .expect("lock test statuses")
            .remove(route_id);
        Ok(())
    }
}

#[derive(Clone, Default)]
struct InMemoryTestEventPublisher {
    events: Arc<Mutex<Vec<RuntimeEvent>>>,
}

#[async_trait]
impl EventPublisherPort for InMemoryTestEventPublisher {
    async fn publish(&self, events: &[RuntimeEvent]) -> Result<(), DomainError> {
        self.events
            .lock()
            .expect("lock test events")
            .extend(events.iter().cloned());
        Ok(())
    }
}

#[derive(Clone, Default)]
struct TrackingExecutionPort {
    registered: Arc<Mutex<Vec<String>>>,
    removed: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone, Default)]
struct ConfigurableExecutionPort {
    fail_register: Arc<Mutex<bool>>,
    fail_start: Arc<Mutex<bool>>,
    fail_suspend_once: Arc<Mutex<bool>>,
    fail_suspend_retry: Arc<Mutex<bool>>,
    fail_resume: Arc<Mutex<bool>>,
    fail_remove_must_stopped: Arc<Mutex<bool>>,
    fail_remove_retry: Arc<Mutex<bool>>,
    stop_called: Arc<Mutex<u32>>,
}

impl ConfigurableExecutionPort {
    fn with_register_failure() -> Self {
        Self {
            fail_register: Arc::new(Mutex::new(true)),
            ..Self::default()
        }
    }

    fn with_suspend_failure_once() -> Self {
        Self {
            fail_suspend_once: Arc::new(Mutex::new(true)),
            ..Self::default()
        }
    }

    fn with_suspend_failure_once_and_retry() -> Self {
        Self {
            fail_suspend_once: Arc::new(Mutex::new(true)),
            fail_suspend_retry: Arc::new(Mutex::new(true)),
            ..Self::default()
        }
    }

    fn with_start_and_resume_failure() -> Self {
        Self {
            fail_start: Arc::new(Mutex::new(true)),
            fail_resume: Arc::new(Mutex::new(true)),
            ..Self::default()
        }
    }

    fn with_remove_requires_stop() -> Self {
        Self {
            fail_remove_must_stopped: Arc::new(Mutex::new(true)),
            ..Self::default()
        }
    }

    fn with_remove_requires_stop_and_retry_failure() -> Self {
        Self {
            fail_remove_must_stopped: Arc::new(Mutex::new(true)),
            fail_remove_retry: Arc::new(Mutex::new(true)),
            ..Self::default()
        }
    }
}

impl TrackingExecutionPort {
    fn new() -> Self {
        Self::default()
    }

    fn remove_called(&self, route_id: &str) -> bool {
        self.removed
            .lock()
            .expect("lock removed routes")
            .iter()
            .any(|id| id == route_id)
    }
}

#[async_trait]
impl RuntimeExecutionPort for TrackingExecutionPort {
    async fn register_route(&self, definition: RouteDefinition) -> Result<(), DomainError> {
        self.registered
            .lock()
            .expect("lock registered routes")
            .push(definition.route_id().to_string());
        Ok(())
    }

    async fn start_route(&self, _route_id: &str) -> Result<(), DomainError> {
        Ok(())
    }

    async fn stop_route(&self, _route_id: &str) -> Result<(), DomainError> {
        Ok(())
    }

    async fn suspend_route(&self, _route_id: &str) -> Result<(), DomainError> {
        Ok(())
    }

    async fn resume_route(&self, _route_id: &str) -> Result<(), DomainError> {
        Ok(())
    }

    async fn reload_route(&self, _route_id: &str) -> Result<(), DomainError> {
        Ok(())
    }

    async fn remove_route(&self, route_id: &str) -> Result<(), DomainError> {
        self.removed
            .lock()
            .expect("lock removed routes")
            .push(route_id.to_string());
        Ok(())
    }

    async fn in_flight_count(&self, route_id: &str) -> Result<InFlightCountResult, DomainError> {
        Ok(InFlightCountResult::RouteNotFound {
            route_id: route_id.to_string(),
        })
    }
}

#[async_trait]
impl RuntimeExecutionPort for ConfigurableExecutionPort {
    async fn register_route(&self, _definition: RouteDefinition) -> Result<(), DomainError> {
        if *self.fail_register.lock().expect("fail_register") {
            return Err(DomainError::InvalidState("register failed".into()));
        }
        Ok(())
    }

    async fn start_route(&self, _route_id: &str) -> Result<(), DomainError> {
        if *self.fail_start.lock().expect("fail_start") {
            return Err(DomainError::InvalidState("start failed".into()));
        }
        Ok(())
    }

    async fn stop_route(&self, _route_id: &str) -> Result<(), DomainError> {
        let mut calls = self.stop_called.lock().expect("stop_called");
        *calls += 1;
        Ok(())
    }

    async fn suspend_route(&self, _route_id: &str) -> Result<(), DomainError> {
        let mut fail_once = self.fail_suspend_once.lock().expect("fail_suspend_once");
        if *fail_once {
            *fail_once = false;
            return Err(DomainError::InvalidState("suspend failed".into()));
        }
        if *self.fail_suspend_retry.lock().expect("fail_suspend_retry") {
            return Err(DomainError::InvalidState("suspend retry failed".into()));
        }
        Ok(())
    }

    async fn resume_route(&self, _route_id: &str) -> Result<(), DomainError> {
        if *self.fail_resume.lock().expect("fail_resume") {
            return Err(DomainError::InvalidState("resume failed".into()));
        }
        Ok(())
    }

    async fn reload_route(&self, _route_id: &str) -> Result<(), DomainError> {
        Ok(())
    }

    async fn remove_route(&self, _route_id: &str) -> Result<(), DomainError> {
        let mut first = self
            .fail_remove_must_stopped
            .lock()
            .expect("fail_remove_must_stopped");
        if *first {
            *first = false;
            return Err(DomainError::InvalidState("must be stopped first".into()));
        }
        if *self.fail_remove_retry.lock().expect("fail_remove_retry") {
            return Err(DomainError::InvalidState("remove retry failed".into()));
        }
        Ok(())
    }

    async fn in_flight_count(&self, route_id: &str) -> Result<InFlightCountResult, DomainError> {
        Ok(InFlightCountResult::RouteNotFound {
            route_id: route_id.to_string(),
        })
    }
}

#[derive(Clone, Default)]
struct FailingSaveRepository {
    inner: InMemoryTestRepo,
}

#[async_trait]
impl RouteRepositoryPort for FailingSaveRepository {
    async fn load(&self, route_id: &str) -> Result<Option<RouteRuntimeAggregate>, DomainError> {
        self.inner.load(route_id).await
    }

    async fn save(&self, _aggregate: RouteRuntimeAggregate) -> Result<(), DomainError> {
        Err(DomainError::InvalidState(
            "simulated repository save failure".to_string(),
        ))
    }

    async fn save_if_version(
        &self,
        aggregate: RouteRuntimeAggregate,
        expected_version: u64,
    ) -> Result<(), DomainError> {
        self.inner
            .save_if_version(aggregate, expected_version)
            .await
    }

    async fn delete(&self, route_id: &str) -> Result<(), DomainError> {
        self.inner.delete(route_id).await
    }
}

fn build_test_deps_no_execution() -> CommandDeps {
    let repo: Arc<dyn RouteRepositoryPort> = Arc::new(InMemoryTestRepo::default());
    let projections: Arc<dyn ProjectionStorePort> =
        Arc::new(InMemoryTestProjectionStore::default());
    let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher::default());
    CommandDeps {
        repo,
        projections,
        events,
        uow: None,
        execution: None,
        health_registry: None,
    }
}

fn build_test_deps_with_failing_repo(execution: Arc<TrackingExecutionPort>) -> CommandDeps {
    let repo: Arc<dyn RouteRepositoryPort> = Arc::new(FailingSaveRepository::default());
    let projections: Arc<dyn ProjectionStorePort> =
        Arc::new(InMemoryTestProjectionStore::default());
    let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher::default());
    let execution: Arc<dyn RuntimeExecutionPort> = execution;
    CommandDeps {
        repo,
        projections,
        events,
        uow: None,
        execution: Some(execution),
        health_registry: None,
    }
}

#[tokio::test]
async fn fail_route_forces_unhealthy_health_entry() {
    let deps = build_test_deps_no_execution();
    let health_registry = Arc::new(HealthCheckRegistry::new(Duration::from_secs(5)));
    health_registry.register_for_route("route-f", healthy_check("consumer"));

    let deps = CommandDeps {
        health_registry: Some(health_registry.clone()),
        ..deps
    };

    let def = RouteDefinition::new("timer:test", vec![]).with_route_id("route-f");
    handle_register_internal(&deps, def).await.unwrap();

    handle_lifecycle(
        &deps,
        "route-f".to_string(),
        RouteLifecycleCommand::Fail("boom".to_string()),
    )
    .await
    .unwrap();

    let report = health_registry.check_all().await;
    assert_eq!(report.status, HealthStatus::Unhealthy);
    assert_eq!(report.services.len(), 1);
    assert_eq!(report.services[0].name, "route:route-f");
    assert_eq!(
        report.services[0].message.as_deref(),
        Some("route failed: boom")
    );

    health_registry.register_for_route("route-f", healthy_check("consumer"));
    // R4-L12: recovery gate requires both a new probe generation AND a
    // post-force Started marker.
    health_registry.mark_route_started("route-f");
    let report = health_registry.check_all().await;
    assert_eq!(report.status, HealthStatus::Healthy);
    assert_eq!(report.services[0].name, "consumer");
}

#[tokio::test]
async fn handle_register_internal_persists_registered_state() {
    let deps = build_test_deps_no_execution();
    let def = RouteDefinition::new("timer:test", vec![]).with_route_id("route-a");
    let result = handle_register_internal(&deps, def).await;
    assert!(result.is_ok());

    let aggregate = deps.repo.load("route-a").await.unwrap().unwrap();
    assert_eq!(aggregate.state(), &RouteRuntimeState::Registered);
}

#[tokio::test]
async fn handle_register_internal_compensates_on_persist_failure() {
    let execution = Arc::new(TrackingExecutionPort::new());
    let deps = build_test_deps_with_failing_repo(execution.clone());
    let def = RouteDefinition::new("timer:test", vec![]).with_route_id("route-b");
    let result = handle_register_internal(&deps, def).await;
    assert!(result.is_err());
    assert!(execution.remove_called("route-b"));
}

#[tokio::test]
async fn duplicate_route_id_is_rejected() {
    let deps = build_test_deps_no_execution();
    let def1 = RouteDefinition::new("timer:test", vec![]).with_route_id("route-c");
    let def2 = RouteDefinition::new("timer:other", vec![]).with_route_id("route-c");
    handle_register_internal(&deps, def1).await.unwrap();

    let result = handle_register_internal(&deps, def2).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}

#[test]
fn canonical_step_conversion_covers_common_variants() {
    use camel_api::LanguageExpressionDef;
    use camel_api::runtime::{
        CanonicalSplitAggregationSpec, CanonicalSplitExpressionSpec, CanonicalStepSpec,
        CanonicalWhenSpec,
    };

    let to = canonical_step_to_builder_step(CanonicalStepSpec::To {
        uri: "log:out".into(),
    })
    .unwrap();
    assert!(matches!(to, BuilderStep::To(_)));

    let log = canonical_step_to_builder_step(CanonicalStepSpec::Log {
        message: "hello".into(),
    })
    .unwrap();
    assert!(matches!(log, BuilderStep::Log { .. }));

    let filter = canonical_step_to_builder_step(CanonicalStepSpec::Filter {
        predicate: LanguageExpressionDef {
            language: "simple".into(),
            source: "${body} != null".into(),
        },
        steps: vec![CanonicalStepSpec::Stop],
    })
    .unwrap();
    assert!(matches!(filter, BuilderStep::DeclarativeFilter { .. }));

    let choice = canonical_step_to_builder_step(CanonicalStepSpec::Choice {
        whens: vec![CanonicalWhenSpec {
            predicate: LanguageExpressionDef {
                language: "simple".into(),
                source: "${body} == 1".into(),
            },
            steps: vec![CanonicalStepSpec::To {
                uri: "mock:a".into(),
            }],
        }],
        otherwise: Some(vec![CanonicalStepSpec::To {
            uri: "mock:b".into(),
        }]),
    })
    .unwrap();
    assert!(matches!(choice, BuilderStep::DeclarativeChoice { .. }));

    let split_lines = canonical_step_to_builder_step(CanonicalStepSpec::Split {
        expression: CanonicalSplitExpressionSpec::BodyLines,
        aggregation: CanonicalSplitAggregationSpec::CollectAll,
        parallel: true,
        parallel_limit: Some(4),
        stop_on_exception: true,
        steps: vec![CanonicalStepSpec::Stop],
    })
    .unwrap();
    assert!(matches!(split_lines, BuilderStep::Split { .. }));

    let split_language = canonical_step_to_builder_step(CanonicalStepSpec::Split {
        expression: CanonicalSplitExpressionSpec::Language(LanguageExpressionDef {
            language: "simple".into(),
            source: "${body.items}".into(),
        }),
        aggregation: CanonicalSplitAggregationSpec::Original,
        parallel: false,
        parallel_limit: None,
        stop_on_exception: false,
        steps: vec![CanonicalStepSpec::Stop],
    })
    .unwrap();
    assert!(matches!(
        split_language,
        BuilderStep::DeclarativeSplit { .. }
    ));

    let wire_tap = canonical_step_to_builder_step(CanonicalStepSpec::WireTap {
        uri: "direct:audit".into(),
    })
    .unwrap();
    assert!(matches!(wire_tap, BuilderStep::WireTap { .. }));

    let script = canonical_step_to_builder_step(CanonicalStepSpec::Script {
        expression: LanguageExpressionDef {
            language: "simple".into(),
            source: "${body}".into(),
        },
    })
    .unwrap();
    assert!(matches!(script, BuilderStep::DeclarativeScript { .. }));

    let delay = canonical_step_to_builder_step(CanonicalStepSpec::Delay {
        delay_ms: 500,
        dynamic_header: None,
    })
    .unwrap();
    assert!(matches!(delay, BuilderStep::Delay { .. }));

    let aggregate = canonical_step_to_builder_step(CanonicalStepSpec::Aggregate(
        camel_api::runtime::CanonicalAggregateSpec {
            header: "corr-id".into(),
            completion_size: Some(5),
            completion_timeout_ms: Some(1000),
            correlation_key: None,
            force_completion_on_stop: Some(true),
            discard_on_timeout: Some(false),
            strategy: camel_api::runtime::CanonicalAggregateStrategySpec::CollectAll,
            max_buckets: Some(100),
            bucket_ttl_ms: Some(60000),
        },
    ))
    .unwrap();
    assert!(matches!(aggregate, BuilderStep::Aggregate { .. }));
}

#[test]
fn canonical_route_conversion_with_circuit_breaker() {
    use camel_api::runtime::{CanonicalCircuitBreakerSpec, CanonicalRouteSpec, CanonicalStepSpec};

    let spec = CanonicalRouteSpec {
        route_id: "route-x".into(),
        from: "timer:tick".into(),
        steps: vec![CanonicalStepSpec::Stop],
        circuit_breaker: Some(CanonicalCircuitBreakerSpec {
            failure_threshold: 3,
            open_duration_ms: 500,
        }),
        auto_startup: None,
        startup_order: None,
        concurrency: None,
        version: camel_api::runtime::CANONICAL_CONTRACT_VERSION,
    };

    let route = canonical_to_route_definition(spec).unwrap();
    assert_eq!(route.route_id(), "route-x");
    assert_eq!(route.from_uri(), "timer:tick");
}

#[tokio::test]
async fn apply_runtime_lifecycle_start_recovery_error_path() {
    let execution = ConfigurableExecutionPort::with_start_and_resume_failure();
    let err = apply_runtime_lifecycle(&execution, "r1", &RouteLifecycleCommand::Start)
        .await
        .expect_err("must fail");
    assert!(
        err.to_string()
            .contains("runtime execution recovery failed for Start")
    );
}

#[tokio::test]
async fn apply_runtime_lifecycle_suspend_recovery_paths() {
    let ok_after_retry = ConfigurableExecutionPort::with_suspend_failure_once();
    apply_runtime_lifecycle(&ok_after_retry, "r1", &RouteLifecycleCommand::Suspend)
        .await
        .expect("second suspend attempt should succeed");

    let fail_retry = ConfigurableExecutionPort::with_suspend_failure_once_and_retry();
    let err = apply_runtime_lifecycle(&fail_retry, "r1", &RouteLifecycleCommand::Suspend)
        .await
        .expect_err("retry failure expected");
    assert!(err.to_string().contains("retry_error"));
}

#[tokio::test]
async fn apply_runtime_lifecycle_resume_recovery_error_path() {
    let execution = ConfigurableExecutionPort::with_start_and_resume_failure();
    let err = apply_runtime_lifecycle(&execution, "r1", &RouteLifecycleCommand::Resume)
        .await
        .expect_err("must fail");
    assert!(
        err.to_string()
            .contains("runtime execution recovery failed for Resume")
    );
}

#[tokio::test]
async fn remove_runtime_route_with_recovery_paths() {
    let execution = ConfigurableExecutionPort::with_remove_requires_stop();
    remove_runtime_route_with_recovery(&execution, "r1")
        .await
        .expect("remove after stop should succeed");
    assert_eq!(*execution.stop_called.lock().unwrap(), 1);

    let fail_retry = ConfigurableExecutionPort::with_remove_requires_stop_and_retry_failure();
    let err = remove_runtime_route_with_recovery(&fail_retry, "r2")
        .await
        .expect_err("retry remove should fail");
    assert!(err.to_string().contains("retry_remove_error"));
}

/// Projection store that ALWAYS fails `upsert_status`. Used to test that
/// `upsert_projection_with_reconciliation` returning `Err` (both primary +
/// retry failed) is propagated, not swallowed.
#[derive(Clone)]
struct FailingProjectionStore;

#[async_trait]
impl ProjectionStorePort for FailingProjectionStore {
    async fn upsert_status(&self, _status: RouteStatusProjection) -> Result<(), DomainError> {
        Err(DomainError::InvalidState("projection failure".into()))
    }

    async fn get_status(
        &self,
        _route_id: &str,
    ) -> Result<Option<RouteStatusProjection>, DomainError> {
        Ok(None)
    }

    async fn list_statuses(&self) -> Result<Vec<RouteStatusProjection>, DomainError> {
        Ok(vec![])
    }

    async fn remove_status(&self, _route_id: &str) -> Result<(), DomainError> {
        Ok(())
    }
}

#[tokio::test]
async fn projection_error_on_start_phase1_is_not_swallowed() {
    // Register route using a working projection store first
    let working_repo: Arc<dyn RouteRepositoryPort> = Arc::new(InMemoryTestRepo::default());
    let working_projections: Arc<dyn ProjectionStorePort> =
        Arc::new(InMemoryTestProjectionStore::default());
    let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher::default());
    let working_deps = CommandDeps {
        repo: working_repo.clone(),
        projections: working_projections,
        events: events.clone(),
        uow: None,
        execution: None,
        health_registry: None,
    };
    let def = RouteDefinition::new("timer:test", vec![]).with_route_id("route-proj-fail-p1");
    handle_register_internal(&working_deps, def).await.unwrap();

    // Now recreate deps with a failing projection store for the start command.
    // Use the same repo (shared Arc) so the route aggregate is visible,
    // but swap in the FailingProjectionStore that fails on the first upsert.
    let fail_projections: Arc<dyn ProjectionStorePort> = Arc::new(FailingProjectionStore);
    let fail_deps = CommandDeps {
        repo: working_repo,
        projections: fail_projections,
        events,
        uow: None,
        execution: None,
        health_registry: None,
    };

    // Start the route — projection error should propagate, not be swallowed
    let err = execute_command(
        &fail_deps,
        RuntimeCommand::StartRoute {
            route_id: "route-proj-fail-p1".to_string(),
            command_id: "test".to_string(),
            causation_id: None,
        },
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("projection failure"),
        "expected projection failure, got: {err}"
    );
}

// --- Two-phase Start integration tests ---

#[tokio::test]
async fn two_phase_start_happy_path() {
    let execution = Arc::new(TrackingExecutionPort::new());
    let repo: Arc<dyn RouteRepositoryPort> = Arc::new(InMemoryTestRepo::default());
    let projections: Arc<dyn ProjectionStorePort> =
        Arc::new(InMemoryTestProjectionStore::default());
    let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher::default());
    let deps = CommandDeps {
        repo: repo.clone(),
        projections,
        events,
        uow: None,
        execution: Some(execution),
        health_registry: None,
    };

    // Register a route
    let def = RouteDefinition::new("timer:test", vec![]).with_route_id("route-start-happy");
    handle_register_internal(&deps, def).await.unwrap();

    // Start the route
    let result = execute_command(
        &deps,
        RuntimeCommand::StartRoute {
            route_id: "route-start-happy".to_string(),
            command_id: "test".to_string(),
            causation_id: None,
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(result, RuntimeCommandResult::RouteStateChanged { status, .. } if status == "Started")
    );

    // Verify stored aggregate is in Started state
    let aggregate = deps.repo.load("route-start-happy").await.unwrap().unwrap();
    assert_eq!(*aggregate.state(), RouteRuntimeState::Started);
}

#[tokio::test]
async fn two_phase_start_compensation_on_runtime_failure() {
    let execution = Arc::new(ConfigurableExecutionPort::default());
    *execution.fail_resume.lock().unwrap() = true;
    *execution.fail_start.lock().unwrap() = true;
    let repo: Arc<dyn RouteRepositoryPort> = Arc::new(InMemoryTestRepo::default());
    let projections: Arc<dyn ProjectionStorePort> =
        Arc::new(InMemoryTestProjectionStore::default());
    let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher::default());
    let deps = CommandDeps {
        repo: repo.clone(),
        projections,
        events,
        uow: None,
        execution: Some(execution),
        health_registry: None,
    };

    // Register a route
    let def = RouteDefinition::new("timer:test", vec![]).with_route_id("route-start-fail");
    handle_register_internal(&deps, def).await.unwrap();

    // Start should fail at runtime
    let err = execute_command(
        &deps,
        RuntimeCommand::StartRoute {
            route_id: "route-start-fail".to_string(),
            command_id: "test".to_string(),
            causation_id: None,
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("start failed"));

    // Verify stored aggregate is in Failed state (compensation)
    let aggregate = deps.repo.load("route-start-fail").await.unwrap().unwrap();
    assert!(matches!(aggregate.state(), RouteRuntimeState::Failed(_)));
}

#[tokio::test]
async fn two_phase_start_idempotent_re_start() {
    let execution = Arc::new(TrackingExecutionPort::new());
    let repo: Arc<dyn RouteRepositoryPort> = Arc::new(InMemoryTestRepo::default());
    let projections: Arc<dyn ProjectionStorePort> =
        Arc::new(InMemoryTestProjectionStore::default());
    let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher::default());
    let deps = CommandDeps {
        repo: repo.clone(),
        projections,
        events,
        uow: None,
        execution: Some(execution),
        health_registry: None,
    };

    // Register a route
    let def = RouteDefinition::new("timer:test", vec![]).with_route_id("route-start-re");
    handle_register_internal(&deps, def).await.unwrap();

    // Start the route (first time)
    execute_command(
        &deps,
        RuntimeCommand::StartRoute {
            route_id: "route-start-re".to_string(),
            command_id: "test".to_string(),
            causation_id: None,
        },
    )
    .await
    .unwrap();

    // Re-start should fail with state-specific error
    let err = execute_command(
        &deps,
        RuntimeCommand::StartRoute {
            route_id: "route-start-re".to_string(),
            command_id: "test".to_string(),
            causation_id: None,
        },
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("is in Started state, cannot start")
    );

    // Verify aggregate stays in Started state
    let aggregate = deps.repo.load("route-start-re").await.unwrap().unwrap();
    assert_eq!(*aggregate.state(), RouteRuntimeState::Started);
}

#[tokio::test]
async fn handle_register_compensates_to_failed_on_runtime_failure() {
    let execution = Arc::new(ConfigurableExecutionPort::with_register_failure());
    let repo: Arc<dyn RouteRepositoryPort> = Arc::new(InMemoryTestRepo::default());
    let projections: Arc<dyn ProjectionStorePort> =
        Arc::new(InMemoryTestProjectionStore::default());
    let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher::default());
    let deps = CommandDeps {
        repo: repo.clone(),
        projections,
        events,
        uow: None,
        execution: Some(execution),
        health_registry: None,
    };

    use camel_api::runtime::{CanonicalRouteSpec, CanonicalStepSpec};

    let spec = CanonicalRouteSpec {
        route_id: "route-reg-fail".into(),
        from: "timer:test".into(),
        steps: vec![CanonicalStepSpec::Stop],
        circuit_breaker: None,
        auto_startup: None,
        startup_order: None,
        concurrency: None,
        version: camel_api::runtime::CANONICAL_CONTRACT_VERSION,
    };

    let result = execute_command(
        &deps,
        RuntimeCommand::RegisterRoute {
            spec,
            command_id: "test".to_string(),
            causation_id: None,
        },
    )
    .await;

    assert!(result.is_err());

    // Verify stored aggregate is in Failed state (compensation), NOT Registered
    let aggregate = deps.repo.load("route-reg-fail").await.unwrap().unwrap();
    assert!(matches!(aggregate.state(), RouteRuntimeState::Failed(_)));
}

// --- H8 boot reconciler tests ---

/// Helper: persist an aggregate in any state (with projection) to simulate
/// a crash-leftover persisted state.
async fn seed_route(deps: &CommandDeps, route_id: &str, state: RouteRuntimeState, version: u64) {
    let aggregate = RouteRuntimeAggregate::from_snapshot(route_id, state, version);
    deps.repo.save(aggregate.clone()).await.unwrap();
    deps.projections
        .upsert_status(project_from_aggregate(&aggregate))
        .await
        .unwrap();
}

#[tokio::test]
async fn boot_reconciler_fails_stale_starting_route() {
    let deps = build_test_deps_no_execution();
    seed_route(
        &deps,
        "route-stale-starting",
        RouteRuntimeState::Starting,
        1,
    )
    .await;

    reconcile_transient_states(&deps).await.unwrap();

    let reloaded = deps
        .repo
        .load("route-stale-starting")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(reloaded.state(), RouteRuntimeState::Failed(_)));
    let proj = deps
        .projections
        .get_status("route-stale-starting")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(proj.status, "Failed");
}

#[tokio::test]
async fn boot_reconciler_fails_stale_stopping_route() {
    let deps = build_test_deps_no_execution();
    seed_route(
        &deps,
        "route-stale-stopping",
        RouteRuntimeState::Stopping,
        1,
    )
    .await;

    reconcile_transient_states(&deps).await.unwrap();

    let reloaded = deps
        .repo
        .load("route-stale-stopping")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(reloaded.state(), RouteRuntimeState::Failed(_)));
    let proj = deps
        .projections
        .get_status("route-stale-stopping")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(proj.status, "Failed");
}

#[tokio::test]
async fn boot_reconciler_leaves_stable_states_untouched() {
    let deps = build_test_deps_no_execution();

    // Seed one route in each stable (non-transient) state.
    let stable_routes: Vec<(&str, RouteRuntimeState, u64)> = vec![
        ("route-stable-registered", RouteRuntimeState::Registered, 0),
        ("route-stable-started", RouteRuntimeState::Started, 2),
        ("route-stable-stopped", RouteRuntimeState::Stopped, 2),
        ("route-stable-suspended", RouteRuntimeState::Suspended, 2),
        (
            "route-stable-failed",
            RouteRuntimeState::Failed("preexisting".to_string()),
            1,
        ),
    ];
    for (id, state, version) in &stable_routes {
        seed_route(&deps, id, state.clone(), *version).await;
    }

    reconcile_transient_states(&deps).await.unwrap();

    for (id, expected_state, expected_version) in &stable_routes {
        let reloaded = deps.repo.load(id).await.unwrap().unwrap();
        assert_eq!(reloaded.state(), expected_state, "state changed for {id}");
        assert_eq!(
            reloaded.version(),
            *expected_version,
            "version changed for {id}"
        );
    }
}
