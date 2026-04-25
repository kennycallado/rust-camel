use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use camel_api::{CanonicalRouteSpec, RuntimeCommand, RuntimeCommandBus, RuntimeHandle};
use camel_component_timer::TimerComponent;
use camel_core::Registry;
use camel_core::lifecycle::domain::DomainError;
use camel_core::spawn_controller_actor;
use camel_core::{
    DefaultRouteController, InMemoryCommandDedup, InMemoryEventPublisher, InMemoryRouteRepository,
    ProjectionStorePort, RouteStatusProjection, RuntimeBus, RuntimeExecutionAdapter,
};
use tokio::sync::RwLock;

#[derive(Default)]
struct FlakyProjectionStore {
    statuses: RwLock<HashMap<String, RouteStatusProjection>>,
    fail_next_upsert: AtomicBool,
}

impl FlakyProjectionStore {
    fn fail_next_upsert(&self) {
        self.fail_next_upsert.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl ProjectionStorePort for FlakyProjectionStore {
    async fn upsert_status(&self, status: RouteStatusProjection) -> Result<(), DomainError> {
        if self.fail_next_upsert.swap(false, Ordering::SeqCst) {
            return Err(DomainError::InvalidState(
                "injected projection failure".to_string(),
            ));
        }
        self.statuses
            .write()
            .await
            .insert(status.route_id.clone(), status);
        Ok(())
    }

    async fn get_status(
        &self,
        route_id: &str,
    ) -> Result<Option<RouteStatusProjection>, DomainError> {
        Ok(self.statuses.read().await.get(route_id).cloned())
    }

    async fn list_statuses(&self) -> Result<Vec<RouteStatusProjection>, DomainError> {
        Ok(self.statuses.read().await.values().cloned().collect())
    }

    async fn remove_status(&self, route_id: &str) -> Result<(), DomainError> {
        self.statuses.write().await.remove(route_id);
        Ok(())
    }
}

fn empty_languages() -> camel_core::route_controller::SharedLanguageRegistry {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

#[tokio::test]
async fn controller_success_plus_projection_failure_triggers_reconciliation() {
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .expect("registry lock poisoned")
        .register(std::sync::Arc::new(TimerComponent::new()));

    let (controller, _actor_join) = spawn_controller_actor(DefaultRouteController::with_languages(
        Arc::clone(&registry),
        empty_languages(),
        Arc::new(camel_api::NoopPlatformService::default()),
    ));

    let repo = Arc::new(InMemoryRouteRepository::default());
    let projections = Arc::new(FlakyProjectionStore::default());
    let events = Arc::new(InMemoryEventPublisher::default());
    let dedup = Arc::new(InMemoryCommandDedup::default());

    let runtime = Arc::new(
        RuntimeBus::new(repo, projections.clone(), events, dedup)
            .with_execution(Arc::new(RuntimeExecutionAdapter::new(controller.clone()))),
    );

    let runtime_handle: Arc<dyn RuntimeHandle> = runtime.clone();
    controller.set_runtime_handle(runtime_handle).await.unwrap();

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("fault-r1", "timer:tick"),
            command_id: "cmd-register".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();

    projections.fail_next_upsert();

    let err = runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "fault-r1".to_string(),
            command_id: "cmd-start".to_string(),
            causation_id: Some("cmd-register".to_string()),
        })
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("post-effect reconciliation"),
        "unexpected error: {err}"
    );

    let status = projections.get_status("fault-r1").await.unwrap().unwrap();
    assert_eq!(status.status, "Started");

    let _ = runtime
        .execute(RuntimeCommand::StopRoute {
            route_id: "fault-r1".to_string(),
            command_id: "cmd-stop".to_string(),
            causation_id: Some("cmd-start".to_string()),
        })
        .await;
}
