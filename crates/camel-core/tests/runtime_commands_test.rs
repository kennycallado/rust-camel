use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use camel_api::runtime::{
    CanonicalAggregateSpec, CanonicalAggregateStrategySpec, CanonicalCircuitBreakerSpec,
    CanonicalSplitAggregationSpec, CanonicalSplitExpressionSpec, CanonicalStepSpec,
};
use camel_api::{
    CanonicalRouteSpec, LanguageExpressionDef, RuntimeCommand, RuntimeCommandBus,
    RuntimeCommandResult, RuntimeQuery, RuntimeQueryBus, RuntimeQueryResult,
};
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::lifecycle::domain::DomainError;
use camel_core::spawn_controller_actor;
use camel_core::{
    DefaultRouteController, InMemoryCommandDedup, InMemoryEventPublisher, InMemoryProjectionStore,
    InMemoryRouteRepository, Registry, RouteRepositoryPort, RouteRuntimeAggregate,
    RouteRuntimeState, RuntimeBus, RuntimeExecutionAdapter,
};
use tokio::sync::RwLock;

fn simple_languages() -> camel_core::route_controller::SharedLanguageRegistry {
    let mut map: std::collections::HashMap<String, Arc<dyn camel_language_api::Language>> =
        std::collections::HashMap::new();
    map.insert(
        "simple".to_string(),
        Arc::new(camel_language_simple::SimpleLanguage),
    );
    Arc::new(std::sync::Mutex::new(map))
}

#[tokio::test]
async fn register_then_start_route_updates_write_model() {
    let runtime = RuntimeBus::new(
        Arc::new(InMemoryRouteRepository::default()),
        Arc::new(InMemoryProjectionStore::default()),
        Arc::new(InMemoryEventPublisher::default()),
        Arc::new(InMemoryCommandDedup::default()),
    );

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("r1", "timer:tick"),
            command_id: "c1".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "r1".into(),
            command_id: "c2".into(),
            causation_id: Some("c1".into()),
        })
        .await
        .unwrap();

    let agg = runtime.repo().load("r1").await.unwrap().unwrap();
    assert!(matches!(agg.state(), RouteRuntimeState::Started));
}

#[derive(Default)]
struct ConflictRouteRepository {
    route: RwLock<Option<RouteRuntimeAggregate>>,
}

impl ConflictRouteRepository {
    fn with_route(route: RouteRuntimeAggregate) -> Self {
        Self {
            route: RwLock::new(Some(route)),
        }
    }
}

#[async_trait]
impl RouteRepositoryPort for ConflictRouteRepository {
    async fn load(&self, _route_id: &str) -> Result<Option<RouteRuntimeAggregate>, DomainError> {
        Ok(self.route.read().await.clone())
    }

    async fn save(&self, aggregate: RouteRuntimeAggregate) -> Result<(), DomainError> {
        *self.route.write().await = Some(aggregate);
        Ok(())
    }

    async fn save_if_version(
        &self,
        _aggregate: RouteRuntimeAggregate,
        _expected_version: u64,
    ) -> Result<(), DomainError> {
        Err(DomainError::InvalidState(
            "forced optimistic lock conflict".to_string(),
        ))
    }

    async fn delete(&self, _route_id: &str) -> Result<(), DomainError> {
        *self.route.write().await = None;
        Ok(())
    }
}

#[derive(Default)]
struct FailFirstSaveIfVersionRepository {
    route: RwLock<Option<RouteRuntimeAggregate>>,
    save_if_version_attempts: AtomicUsize,
}

#[async_trait]
impl RouteRepositoryPort for FailFirstSaveIfVersionRepository {
    async fn load(&self, _route_id: &str) -> Result<Option<RouteRuntimeAggregate>, DomainError> {
        Ok(self.route.read().await.clone())
    }

    async fn save(&self, aggregate: RouteRuntimeAggregate) -> Result<(), DomainError> {
        *self.route.write().await = Some(aggregate);
        Ok(())
    }

    async fn save_if_version(
        &self,
        aggregate: RouteRuntimeAggregate,
        expected_version: u64,
    ) -> Result<(), DomainError> {
        if self.save_if_version_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(DomainError::InvalidState(
                "forced post-effect save_if_version failure".to_string(),
            ));
        }

        let mut guard = self.route.write().await;
        let current = guard.as_ref().ok_or_else(|| {
            DomainError::InvalidState("optimistic lock conflict: route not found".to_string())
        })?;
        if current.version() != expected_version {
            return Err(DomainError::InvalidState(format!(
                "optimistic lock conflict: expected version {expected_version}, actual {}",
                current.version()
            )));
        }

        *guard = Some(aggregate);
        Ok(())
    }

    async fn delete(&self, _route_id: &str) -> Result<(), DomainError> {
        *self.route.write().await = None;
        Ok(())
    }
}

#[tokio::test]
async fn start_route_surfaces_optimistic_lock_conflict() {
    let runtime = RuntimeBus::new(
        Arc::new(ConflictRouteRepository::with_route(
            RouteRuntimeAggregate::new("r-conflict"),
        )),
        Arc::new(InMemoryProjectionStore::default()),
        Arc::new(InMemoryEventPublisher::default()),
        Arc::new(InMemoryCommandDedup::default()),
    );

    let err = runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "r-conflict".into(),
            command_id: "c-conflict".into(),
            causation_id: None,
        })
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("forced optimistic lock conflict"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn connected_runtime_retry_recovers_after_post_effect_persistence_failure() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .expect("registry lock poisoned")
        .register(std::sync::Arc::new(TimerComponent::new()));
    let (controller, _actor_join) = spawn_controller_actor(DefaultRouteController::with_languages(
        Arc::clone(&registry),
        simple_languages(),
        Arc::new(camel_api::NoopLeaderElector),
    ));

    let repo = Arc::new(FailFirstSaveIfVersionRepository::default());
    let runtime = Arc::new(
        RuntimeBus::new(
            repo.clone(),
            Arc::new(InMemoryProjectionStore::default()),
            Arc::new(InMemoryEventPublisher::default()),
            Arc::new(InMemoryCommandDedup::default()),
        )
        .with_execution(Arc::new(RuntimeExecutionAdapter::new(controller.clone()))),
    );

    let runtime_handle: Arc<dyn camel_api::RuntimeHandle> = runtime.clone();
    controller.set_runtime_handle(runtime_handle).await.unwrap();

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("retry-r1", "timer:tick"),
            command_id: "c-register".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    let first_error = runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "retry-r1".into(),
            command_id: "c-start-retry".into(),
            causation_id: Some("c-register".into()),
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(
        first_error.contains("forced post-effect save_if_version failure"),
        "unexpected error: {first_error}"
    );

    let retry_result = runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "retry-r1".into(),
            command_id: "c-start-retry".into(),
            causation_id: Some("c-register".into()),
        })
        .await
        .unwrap();
    assert!(
        matches!(
            retry_result,
            RuntimeCommandResult::RouteStateChanged { ref route_id, ref status }
            if route_id == "retry-r1" && status == "Started"
        ),
        "unexpected retry result: {retry_result:?}"
    );

    let projection = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "retry-r1".into(),
        })
        .await
        .unwrap();
    assert!(
        matches!(
            projection,
            RuntimeQueryResult::RouteStatus { ref status, .. } if status == "Started"
        ),
        "projection must converge to Started after retry: {projection:?}"
    );

    let aggregate = runtime.repo().load("retry-r1").await.unwrap().unwrap();
    assert_eq!(aggregate.state(), &RouteRuntimeState::Started);
}

#[tokio::test]
async fn connected_runtime_lifecycle_requires_registered_aggregate() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .expect("registry lock poisoned")
        .register(std::sync::Arc::new(TimerComponent::new()));
    let (controller, _actor_join) = spawn_controller_actor(DefaultRouteController::with_languages(
        Arc::clone(&registry),
        simple_languages(),
        Arc::new(camel_api::NoopLeaderElector),
    ));

    let runtime = Arc::new(
        RuntimeBus::new(
            Arc::new(InMemoryRouteRepository::default()),
            Arc::new(InMemoryProjectionStore::default()),
            Arc::new(InMemoryEventPublisher::default()),
            Arc::new(InMemoryCommandDedup::default()),
        )
        .with_execution(Arc::new(RuntimeExecutionAdapter::new(controller.clone()))),
    );

    let runtime_handle: Arc<dyn camel_api::RuntimeHandle> = runtime.clone();
    controller.set_runtime_handle(runtime_handle).await.unwrap();

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("connected-r1", "timer:tick"),
            command_id: "c-register".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    runtime.repo().delete("connected-r1").await.unwrap();

    let err = runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "connected-r1".into(),
            command_id: "c-start".into(),
            causation_id: Some("c-register".into()),
        })
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("not found"),
        "expected missing aggregate error, got: {err}"
    );
}

#[tokio::test]
async fn connected_runtime_start_tolerates_suspended_controller_drift() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .expect("registry lock poisoned")
        .register(std::sync::Arc::new(TimerComponent::new()));

    let controller_impl = DefaultRouteController::with_languages(
        Arc::clone(&registry),
        simple_languages(),
        Arc::new(camel_api::NoopLeaderElector),
    );
    let (controller, _actor_join) = spawn_controller_actor(controller_impl);

    let runtime = Arc::new(
        RuntimeBus::new(
            Arc::new(InMemoryRouteRepository::default()),
            Arc::new(InMemoryProjectionStore::default()),
            Arc::new(InMemoryEventPublisher::default()),
            Arc::new(InMemoryCommandDedup::default()),
        )
        .with_execution(Arc::new(RuntimeExecutionAdapter::new(controller.clone()))),
    );

    let runtime_handle: Arc<dyn camel_api::RuntimeHandle> = runtime.clone();
    controller.set_runtime_handle(runtime_handle).await.unwrap();

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("drift-start-r1", "timer:tick"),
            command_id: "c-register".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    {
        // Simulate legacy drift: lifecycle was changed through controller path only.
        controller.start_route("drift-start-r1").await.unwrap();
        controller.suspend_route("drift-start-r1").await.unwrap();
    }

    // Aggregate still sees route as Registered; Start should succeed and reconcile side effect.
    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "drift-start-r1".into(),
            command_id: "c-start".into(),
            causation_id: Some("c-register".into()),
        })
        .await
        .unwrap();

    let query = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "drift-start-r1".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        query,
        RuntimeQueryResult::RouteStatus { ref status, .. } if status == "Started"
    ));
}

#[tokio::test]
async fn connected_runtime_suspend_tolerates_already_suspended_controller_drift() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .expect("registry lock poisoned")
        .register(std::sync::Arc::new(TimerComponent::new()));

    let controller_impl = DefaultRouteController::with_languages(
        Arc::clone(&registry),
        simple_languages(),
        Arc::new(camel_api::NoopLeaderElector),
    );
    let (controller, _actor_join) = spawn_controller_actor(controller_impl);

    let runtime = Arc::new(
        RuntimeBus::new(
            Arc::new(InMemoryRouteRepository::default()),
            Arc::new(InMemoryProjectionStore::default()),
            Arc::new(InMemoryEventPublisher::default()),
            Arc::new(InMemoryCommandDedup::default()),
        )
        .with_execution(Arc::new(RuntimeExecutionAdapter::new(controller.clone()))),
    );

    let runtime_handle: Arc<dyn camel_api::RuntimeHandle> = runtime.clone();
    controller.set_runtime_handle(runtime_handle).await.unwrap();

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("drift-suspend-r1", "timer:tick"),
            command_id: "c-register".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "drift-suspend-r1".into(),
            command_id: "c-start".into(),
            causation_id: Some("c-register".into()),
        })
        .await
        .unwrap();

    {
        // Drift: controller already suspended, aggregate still Started.
        controller.suspend_route("drift-suspend-r1").await.unwrap();
    }

    runtime
        .execute(RuntimeCommand::SuspendRoute {
            route_id: "drift-suspend-r1".into(),
            command_id: "c-suspend".into(),
            causation_id: Some("c-start".into()),
        })
        .await
        .unwrap();

    let query = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "drift-suspend-r1".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        query,
        RuntimeQueryResult::RouteStatus { ref status, .. } if status == "Suspended"
    ));
}

#[tokio::test]
async fn connected_runtime_resume_tolerates_already_started_controller_drift() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .expect("registry lock poisoned")
        .register(std::sync::Arc::new(TimerComponent::new()));

    let controller_impl = DefaultRouteController::with_languages(
        Arc::clone(&registry),
        simple_languages(),
        Arc::new(camel_api::NoopLeaderElector),
    );
    let (controller, _actor_join) = spawn_controller_actor(controller_impl);

    let runtime = Arc::new(
        RuntimeBus::new(
            Arc::new(InMemoryRouteRepository::default()),
            Arc::new(InMemoryProjectionStore::default()),
            Arc::new(InMemoryEventPublisher::default()),
            Arc::new(InMemoryCommandDedup::default()),
        )
        .with_execution(Arc::new(RuntimeExecutionAdapter::new(controller.clone()))),
    );

    let runtime_handle: Arc<dyn camel_api::RuntimeHandle> = runtime.clone();
    controller.set_runtime_handle(runtime_handle).await.unwrap();

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("drift-resume-r1", "timer:tick"),
            command_id: "c-register".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "drift-resume-r1".into(),
            command_id: "c-start".into(),
            causation_id: Some("c-register".into()),
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::SuspendRoute {
            route_id: "drift-resume-r1".into(),
            command_id: "c-suspend".into(),
            causation_id: Some("c-start".into()),
        })
        .await
        .unwrap();

    {
        // Drift: controller already resumed, aggregate still Suspended.
        controller.resume_route("drift-resume-r1").await.unwrap();
    }

    runtime
        .execute(RuntimeCommand::ResumeRoute {
            route_id: "drift-resume-r1".into(),
            command_id: "c-resume".into(),
            causation_id: Some("c-suspend".into()),
        })
        .await
        .unwrap();

    let query = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "drift-resume-r1".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        query,
        RuntimeQueryResult::RouteStatus { ref status, .. } if status == "Started"
    ));
}

#[tokio::test]
async fn remove_route_fails_when_route_is_not_stopped() {
    let mut ctx = camel_core::CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    let runtime = ctx.runtime();

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("remove-r1", "timer:tick"),
            command_id: "c-register".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "remove-r1".into(),
            command_id: "c-start".into(),
            causation_id: Some("c-register".into()),
        })
        .await
        .unwrap();

    let err = runtime
        .execute(RuntimeCommand::RemoveRoute {
            route_id: "remove-r1".into(),
            command_id: "c-remove".into(),
            causation_id: Some("c-start".into()),
        })
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("invalid transition") || err.contains("must be stopped"),
        "expected remove guard for started route, got: {err}"
    );
}

#[tokio::test]
async fn connected_runtime_reload_requires_registered_aggregate() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .expect("registry lock poisoned")
        .register(std::sync::Arc::new(TimerComponent::new()));
    let (controller, _actor_join) = spawn_controller_actor(DefaultRouteController::with_languages(
        Arc::clone(&registry),
        simple_languages(),
        Arc::new(camel_api::NoopLeaderElector),
    ));

    let runtime = Arc::new(
        RuntimeBus::new(
            Arc::new(InMemoryRouteRepository::default()),
            Arc::new(InMemoryProjectionStore::default()),
            Arc::new(InMemoryEventPublisher::default()),
            Arc::new(InMemoryCommandDedup::default()),
        )
        .with_execution(Arc::new(RuntimeExecutionAdapter::new(controller.clone()))),
    );

    let runtime_handle: Arc<dyn camel_api::RuntimeHandle> = runtime.clone();
    controller.set_runtime_handle(runtime_handle).await.unwrap();

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("connected-reload-r1", "timer:tick"),
            command_id: "c-register".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    runtime.repo().delete("connected-reload-r1").await.unwrap();

    let err = runtime
        .execute(RuntimeCommand::ReloadRoute {
            route_id: "connected-reload-r1".into(),
            command_id: "c-reload".into(),
            causation_id: Some("c-register".into()),
        })
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("not found"),
        "expected missing aggregate error, got: {err}"
    );
}

#[tokio::test]
async fn connected_runtime_remove_tolerates_started_controller_drift() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .expect("registry lock poisoned")
        .register(std::sync::Arc::new(TimerComponent::new()));

    let controller_impl = DefaultRouteController::with_languages(
        Arc::clone(&registry),
        simple_languages(),
        Arc::new(camel_api::NoopLeaderElector),
    );
    let (controller, _actor_join) = spawn_controller_actor(controller_impl);

    let runtime = Arc::new(
        RuntimeBus::new(
            Arc::new(InMemoryRouteRepository::default()),
            Arc::new(InMemoryProjectionStore::default()),
            Arc::new(InMemoryEventPublisher::default()),
            Arc::new(InMemoryCommandDedup::default()),
        )
        .with_execution(Arc::new(RuntimeExecutionAdapter::new(controller.clone()))),
    );

    let runtime_handle: Arc<dyn camel_api::RuntimeHandle> = runtime.clone();
    controller.set_runtime_handle(runtime_handle).await.unwrap();

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("drift-remove-r1", "timer:tick"),
            command_id: "c-register".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    {
        // Simulate drift: controller lifecycle moved independently of aggregate state.
        controller.start_route("drift-remove-r1").await.unwrap();
    }

    runtime
        .execute(RuntimeCommand::RemoveRoute {
            route_id: "drift-remove-r1".into(),
            command_id: "c-remove".into(),
            causation_id: Some("c-register".into()),
        })
        .await
        .unwrap();

    let query_err = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "drift-remove-r1".into(),
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(
        query_err.contains("not found"),
        "expected removed route to be absent from runtime projection, got: {query_err}"
    );
}

#[tokio::test]
async fn register_route_accepts_advanced_canonical_steps() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .expect("registry lock poisoned")
        .register(std::sync::Arc::new(TimerComponent::new()));
    registry
        .lock()
        .expect("registry lock poisoned")
        .register(std::sync::Arc::new(MockComponent::new()));

    let (controller, _actor_join) = spawn_controller_actor(DefaultRouteController::with_languages(
        Arc::clone(&registry),
        simple_languages(),
        Arc::new(camel_api::NoopLeaderElector),
    ));
    let runtime = Arc::new(
        RuntimeBus::new(
            Arc::new(InMemoryRouteRepository::default()),
            Arc::new(InMemoryProjectionStore::default()),
            Arc::new(InMemoryEventPublisher::default()),
            Arc::new(InMemoryCommandDedup::default()),
        )
        .with_execution(Arc::new(RuntimeExecutionAdapter::new(controller.clone()))),
    );

    let runtime_handle: Arc<dyn camel_api::RuntimeHandle> = runtime.clone();
    controller.set_runtime_handle(runtime_handle).await.unwrap();

    let route = CanonicalRouteSpec {
        route_id: "advanced-canonical-r1".to_string(),
        from: "timer:tick".to_string(),
        steps: vec![
            CanonicalStepSpec::Filter {
                predicate: LanguageExpressionDef {
                    language: "simple".to_string(),
                    source: "true".to_string(),
                },
                steps: vec![CanonicalStepSpec::To {
                    uri: "mock:filtered".to_string(),
                }],
            },
            CanonicalStepSpec::Choice {
                whens: vec![camel_api::runtime::CanonicalWhenSpec {
                    predicate: LanguageExpressionDef {
                        language: "simple".to_string(),
                        source: "true".to_string(),
                    },
                    steps: vec![CanonicalStepSpec::To {
                        uri: "mock:choice-a".to_string(),
                    }],
                }],
                otherwise: Some(vec![CanonicalStepSpec::To {
                    uri: "mock:choice-other".to_string(),
                }]),
            },
            CanonicalStepSpec::Split {
                expression: CanonicalSplitExpressionSpec::BodyLines,
                aggregation: CanonicalSplitAggregationSpec::CollectAll,
                parallel: false,
                parallel_limit: None,
                stop_on_exception: true,
                steps: vec![CanonicalStepSpec::To {
                    uri: "mock:split".to_string(),
                }],
            },
            CanonicalStepSpec::Aggregate {
                config: CanonicalAggregateSpec {
                    header: "orderId".to_string(),
                    completion_size: Some(2),
                    completion_timeout_ms: None,
                    correlation_key: None,
                    force_completion_on_stop: None,
                    discard_on_timeout: None,
                    strategy: CanonicalAggregateStrategySpec::CollectAll,
                    max_buckets: Some(100),
                    bucket_ttl_ms: Some(60_000),
                },
            },
            CanonicalStepSpec::WireTap {
                uri: "mock:tap".to_string(),
            },
            CanonicalStepSpec::Script {
                expression: LanguageExpressionDef {
                    language: "simple".to_string(),
                    source: "${body}".to_string(),
                },
            },
            CanonicalStepSpec::Stop,
        ],
        circuit_breaker: Some(CanonicalCircuitBreakerSpec {
            failure_threshold: 3,
            open_duration_ms: 250,
        }),
        version: 1,
    };

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: route,
            command_id: "c-register-advanced".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    let status = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "advanced-canonical-r1".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        status,
        RuntimeQueryResult::RouteStatus { ref status, .. } if status == "Registered"
    ));
}
