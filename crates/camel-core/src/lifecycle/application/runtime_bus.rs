use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::OnceCell;

use camel_api::{
    CamelError, RuntimeCommand, RuntimeCommandBus, RuntimeCommandResult, RuntimeQuery,
    RuntimeQueryBus, RuntimeQueryResult,
};

use crate::lifecycle::application::commands::{
    CommandDeps, execute_command, handle_register_internal,
};
use crate::lifecycle::application::queries::{QueryDeps, execute_query};
use crate::lifecycle::application::route_definition::RouteDefinition;
use crate::lifecycle::ports::RouteRegistrationPort;
use crate::lifecycle::ports::{
    CommandDedupPort, EventPublisherPort, ProjectionStorePort, RouteRepositoryPort,
    RuntimeExecutionPort, RuntimeUnitOfWorkPort,
};

pub struct RuntimeBus {
    repo: Arc<dyn RouteRepositoryPort>,
    projections: Arc<dyn ProjectionStorePort>,
    events: Arc<dyn EventPublisherPort>,
    dedup: Arc<dyn CommandDedupPort>,
    uow: Option<Arc<dyn RuntimeUnitOfWorkPort>>,
    execution: Option<Arc<dyn RuntimeExecutionPort>>,
    journal_recovered_once: OnceCell<()>,
}

impl RuntimeBus {
    pub fn new(
        repo: Arc<dyn RouteRepositoryPort>,
        projections: Arc<dyn ProjectionStorePort>,
        events: Arc<dyn EventPublisherPort>,
        dedup: Arc<dyn CommandDedupPort>,
    ) -> Self {
        Self {
            repo,
            projections,
            events,
            dedup,
            uow: None,
            execution: None,
            journal_recovered_once: OnceCell::new(),
        }
    }

    pub fn with_uow(mut self, uow: Arc<dyn RuntimeUnitOfWorkPort>) -> Self {
        self.uow = Some(uow);
        self
    }

    pub fn with_execution(mut self, execution: Arc<dyn RuntimeExecutionPort>) -> Self {
        self.execution = Some(execution);
        self
    }

    pub fn repo(&self) -> &Arc<dyn RouteRepositoryPort> {
        &self.repo
    }

    fn deps(&self) -> CommandDeps {
        CommandDeps {
            repo: Arc::clone(&self.repo),
            projections: Arc::clone(&self.projections),
            events: Arc::clone(&self.events),
            uow: self.uow.clone(),
            execution: self.execution.clone(),
        }
    }

    fn query_deps(&self) -> QueryDeps {
        QueryDeps {
            projections: Arc::clone(&self.projections),
        }
    }

    async fn ensure_journal_recovered(&self) -> Result<(), CamelError> {
        let Some(uow) = &self.uow else {
            return Ok(());
        };

        self.journal_recovered_once
            .get_or_try_init(|| async {
                uow.recover_from_journal().await?;
                Ok::<(), CamelError>(())
            })
            .await?;
        Ok(())
    }
}

#[async_trait]
impl RuntimeCommandBus for RuntimeBus {
    async fn execute(&self, cmd: RuntimeCommand) -> Result<RuntimeCommandResult, CamelError> {
        self.ensure_journal_recovered().await?;
        let command_id = cmd.command_id().to_string();
        if !self.dedup.first_seen(&command_id).await? {
            return Ok(RuntimeCommandResult::Duplicate { command_id });
        }
        let deps = self.deps();
        match execute_command(&deps, cmd).await {
            Ok(result) => Ok(result),
            Err(err) => {
                let _ = self.dedup.forget_seen(&command_id).await;
                Err(err)
            }
        }
    }
}

#[async_trait]
impl RuntimeQueryBus for RuntimeBus {
    async fn ask(&self, query: RuntimeQuery) -> Result<RuntimeQueryResult, CamelError> {
        self.ensure_journal_recovered().await?;

        match query {
            RuntimeQuery::InFlightCount { route_id } => {
                if let Some(execution) = &self.execution {
                    execution.in_flight_count(&route_id).await.map_err(Into::into)
                } else {
                    Ok(RuntimeQueryResult::RouteNotFound { route_id })
                }
            }
            other => {
                let deps = self.query_deps();
                execute_query(&deps, other).await
            }
        }
    }
}

#[async_trait]
impl RouteRegistrationPort for RuntimeBus {
    async fn register_route(&self, def: RouteDefinition) -> Result<(), CamelError> {
        self.ensure_journal_recovered().await?;
        let deps = self.deps();
        handle_register_internal(&deps, def).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use crate::lifecycle::domain::DomainError;

    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    use crate::lifecycle::application::route_definition::RouteDefinition;
    use crate::lifecycle::domain::{RouteRuntimeAggregate, RuntimeEvent};
    use crate::lifecycle::ports::RouteRegistrationPort as InternalRuntimeCommandBus;
    use crate::lifecycle::ports::RouteStatusProjection;

    #[derive(Clone, Default)]
    struct InMemoryTestRepo {
        routes: Arc<Mutex<HashMap<String, RouteRuntimeAggregate>>>,
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
    struct InMemoryTestEventPublisher;

    #[async_trait]
    impl EventPublisherPort for InMemoryTestEventPublisher {
        async fn publish(&self, _events: &[RuntimeEvent]) -> Result<(), DomainError> {
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct InMemoryTestDedup {
        seen: Arc<Mutex<HashSet<String>>>,
    }

    #[derive(Clone, Default)]
    struct InspectableDedup {
        seen: Arc<Mutex<HashSet<String>>>,
        forget_calls: Arc<Mutex<u32>>,
    }

    #[async_trait]
    impl CommandDedupPort for InMemoryTestDedup {
        async fn first_seen(&self, command_id: &str) -> Result<bool, DomainError> {
            let mut seen = self.seen.lock().expect("lock dedup set");
            Ok(seen.insert(command_id.to_string()))
        }

        async fn forget_seen(&self, command_id: &str) -> Result<(), DomainError> {
            self.seen.lock().expect("lock dedup set").remove(command_id);
            Ok(())
        }
    }

    #[async_trait]
    impl CommandDedupPort for InspectableDedup {
        async fn first_seen(&self, command_id: &str) -> Result<bool, DomainError> {
            let mut seen = self.seen.lock().expect("lock dedup set");
            Ok(seen.insert(command_id.to_string()))
        }

        async fn forget_seen(&self, command_id: &str) -> Result<(), DomainError> {
            self.seen.lock().expect("lock dedup set").remove(command_id);
            let mut calls = self.forget_calls.lock().expect("forget calls");
            *calls += 1;
            Ok(())
        }
    }

    fn build_test_runtime_bus() -> RuntimeBus {
        let repo: Arc<dyn RouteRepositoryPort> = Arc::new(InMemoryTestRepo::default());
        let projections: Arc<dyn ProjectionStorePort> =
            Arc::new(InMemoryTestProjectionStore::default());
        let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher);
        let dedup: Arc<dyn CommandDedupPort> = Arc::new(InMemoryTestDedup::default());
        RuntimeBus::new(repo, projections, events, dedup)
    }

    #[derive(Default)]
    struct CountingUow {
        recover_calls: Arc<Mutex<u32>>,
    }

    #[derive(Default)]
    struct FailingRecoverUow;

    #[async_trait]
    impl RuntimeUnitOfWorkPort for CountingUow {
        async fn persist_upsert(
            &self,
            _aggregate: RouteRuntimeAggregate,
            _expected_version: Option<u64>,
            _projection: RouteStatusProjection,
            _events: &[RuntimeEvent],
        ) -> Result<(), DomainError> {
            Ok(())
        }

        async fn persist_delete(
            &self,
            _route_id: &str,
            _events: &[RuntimeEvent],
        ) -> Result<(), DomainError> {
            Ok(())
        }

        async fn recover_from_journal(&self) -> Result<(), DomainError> {
            let mut calls = self.recover_calls.lock().expect("recover_calls");
            *calls += 1;
            Ok(())
        }
    }

    #[async_trait]
    impl RuntimeUnitOfWorkPort for FailingRecoverUow {
        async fn persist_upsert(
            &self,
            _aggregate: RouteRuntimeAggregate,
            _expected_version: Option<u64>,
            _projection: RouteStatusProjection,
            _events: &[RuntimeEvent],
        ) -> Result<(), DomainError> {
            Ok(())
        }

        async fn persist_delete(
            &self,
            _route_id: &str,
            _events: &[RuntimeEvent],
        ) -> Result<(), DomainError> {
            Ok(())
        }

        async fn recover_from_journal(&self) -> Result<(), DomainError> {
            Err(DomainError::InvalidState("recover failed".into()))
        }
    }

    #[derive(Default)]
    struct InFlightExecutionPort;

    #[async_trait]
    impl RuntimeExecutionPort for InFlightExecutionPort {
        async fn register_route(&self, _definition: RouteDefinition) -> Result<(), DomainError> {
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
        async fn remove_route(&self, _route_id: &str) -> Result<(), DomainError> {
            Ok(())
        }
        async fn in_flight_count(&self, route_id: &str) -> Result<RuntimeQueryResult, DomainError> {
            if route_id == "known" {
                Ok(RuntimeQueryResult::InFlightCount {
                    route_id: route_id.to_string(),
                    count: 3,
                })
            } else {
                Ok(RuntimeQueryResult::RouteNotFound {
                    route_id: route_id.to_string(),
                })
            }
        }
    }

    #[tokio::test]
    async fn runtime_bus_implements_internal_command_bus() {
        let bus = build_test_runtime_bus();
        let def = RouteDefinition::new("timer:test", vec![]).with_route_id("internal-route");
        let result = InternalRuntimeCommandBus::register_route(&bus, def).await;
        assert!(
            result.is_ok(),
            "internal bus registration failed: {:?}",
            result
        );

        let status = bus
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: "internal-route".to_string(),
            })
            .await
            .unwrap();
        match status {
            RuntimeQueryResult::RouteStatus { status, .. } => {
                assert_eq!(status, "Registered");
            }
            _ => panic!("unexpected query result"),
        }
    }

    #[tokio::test]
    async fn execute_returns_duplicate_for_replayed_command_id() {
        use camel_api::runtime::{CanonicalRouteSpec, CanonicalStepSpec, RuntimeCommand};

        let bus = build_test_runtime_bus();

        let mut spec = CanonicalRouteSpec::new("dup-route", "timer:tick");
        spec.steps = vec![CanonicalStepSpec::Stop];

        let cmd = RuntimeCommand::RegisterRoute {
            spec: spec.clone(),
            command_id: "dup-cmd".into(),
            causation_id: None,
        };
        let first = bus.execute(cmd).await.unwrap();
        assert!(matches!(
            first,
            RuntimeCommandResult::RouteRegistered { route_id } if route_id == "dup-route"
        ));

        let second = bus
            .execute(RuntimeCommand::RegisterRoute {
                spec,
                command_id: "dup-cmd".into(),
                causation_id: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            second,
            RuntimeCommandResult::Duplicate { command_id } if command_id == "dup-cmd"
        ));
    }

    #[tokio::test]
    async fn ask_in_flight_count_without_execution_returns_route_not_found() {
        let bus = build_test_runtime_bus();
        let res = bus
            .ask(RuntimeQuery::InFlightCount {
                route_id: "missing".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            res,
            RuntimeQueryResult::RouteNotFound { route_id } if route_id == "missing"
        ));
    }

    #[tokio::test]
    async fn ask_in_flight_count_with_execution_delegates_to_adapter() {
        let repo: Arc<dyn RouteRepositoryPort> = Arc::new(InMemoryTestRepo::default());
        let projections: Arc<dyn ProjectionStorePort> =
            Arc::new(InMemoryTestProjectionStore::default());
        let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher);
        let dedup: Arc<dyn CommandDedupPort> = Arc::new(InMemoryTestDedup::default());
        let execution: Arc<dyn RuntimeExecutionPort> = Arc::new(InFlightExecutionPort);
        let bus = RuntimeBus::new(repo, projections, events, dedup).with_execution(execution);

        let known = bus
            .ask(RuntimeQuery::InFlightCount {
                route_id: "known".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            known,
            RuntimeQueryResult::InFlightCount { route_id, count }
            if route_id == "known" && count == 3
        ));
    }

    #[tokio::test]
    async fn journal_recovery_runs_once_even_with_multiple_commands() {
        use camel_api::runtime::{CanonicalRouteSpec, CanonicalStepSpec, RuntimeCommand};

        let repo: Arc<dyn RouteRepositoryPort> = Arc::new(InMemoryTestRepo::default());
        let projections: Arc<dyn ProjectionStorePort> =
            Arc::new(InMemoryTestProjectionStore::default());
        let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher);
        let dedup: Arc<dyn CommandDedupPort> = Arc::new(InMemoryTestDedup::default());
        let uow = Arc::new(CountingUow::default());
        let bus = RuntimeBus::new(repo, projections, events, dedup).with_uow(uow.clone());

        let mut spec_a = CanonicalRouteSpec::new("a", "timer:a");
        spec_a.steps = vec![CanonicalStepSpec::Stop];
        let mut spec_b = CanonicalRouteSpec::new("b", "timer:b");
        spec_b.steps = vec![CanonicalStepSpec::Stop];

        bus.execute(RuntimeCommand::RegisterRoute {
            spec: spec_a,
            command_id: "c-a".into(),
            causation_id: None,
        })
        .await
        .unwrap();

        bus.execute(RuntimeCommand::RegisterRoute {
            spec: spec_b,
            command_id: "c-b".into(),
            causation_id: None,
        })
        .await
        .unwrap();

        let calls = *uow.recover_calls.lock().expect("recover calls");
        assert_eq!(calls, 1, "journal recovery should run once");
    }

    #[tokio::test]
    async fn execute_on_command_error_forgets_dedup_marker() {
        use camel_api::runtime::{CanonicalRouteSpec, RuntimeCommand};

        let repo: Arc<dyn RouteRepositoryPort> = Arc::new(InMemoryTestRepo::default());
        let projections: Arc<dyn ProjectionStorePort> =
            Arc::new(InMemoryTestProjectionStore::default());
        let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher);
        let dedup = Arc::new(InspectableDedup::default());
        let dedup_port: Arc<dyn CommandDedupPort> = dedup.clone();

        let bus = RuntimeBus::new(repo, projections, events, dedup_port);

        // Invalid canonical contract: empty route_id -> execute_command should fail.
        let cmd = RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("", "timer:tick"),
            command_id: "err-cmd".into(),
            causation_id: None,
        };

        let err = bus.execute(cmd).await.expect_err("must fail");
        assert!(err.to_string().contains("route_id cannot be empty"));

        assert_eq!(*dedup.forget_calls.lock().expect("forget calls"), 1);
        assert!(!dedup.seen.lock().expect("seen").contains("err-cmd"));
    }

    #[tokio::test]
    async fn execute_propagates_recover_error_from_uow() {
        use camel_api::runtime::{CanonicalRouteSpec, CanonicalStepSpec, RuntimeCommand};

        let repo: Arc<dyn RouteRepositoryPort> = Arc::new(InMemoryTestRepo::default());
        let projections: Arc<dyn ProjectionStorePort> =
            Arc::new(InMemoryTestProjectionStore::default());
        let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher);
        let dedup: Arc<dyn CommandDedupPort> = Arc::new(InMemoryTestDedup::default());
        let uow: Arc<dyn RuntimeUnitOfWorkPort> = Arc::new(FailingRecoverUow);

        let bus = RuntimeBus::new(repo, projections, events, dedup).with_uow(uow);

        let mut spec = CanonicalRouteSpec::new("x", "timer:x");
        spec.steps = vec![CanonicalStepSpec::Stop];
        let err = bus
            .execute(RuntimeCommand::RegisterRoute {
                spec,
                command_id: "recover-err".into(),
                causation_id: None,
            })
            .await
            .expect_err("recover should fail");

        assert!(err.to_string().contains("recover failed"));
    }

    #[tokio::test]
    async fn ask_in_flight_count_with_execution_handles_unknown_route() {
        let repo: Arc<dyn RouteRepositoryPort> = Arc::new(InMemoryTestRepo::default());
        let projections: Arc<dyn ProjectionStorePort> =
            Arc::new(InMemoryTestProjectionStore::default());
        let events: Arc<dyn EventPublisherPort> = Arc::new(InMemoryTestEventPublisher);
        let dedup: Arc<dyn CommandDedupPort> = Arc::new(InMemoryTestDedup::default());
        let execution: Arc<dyn RuntimeExecutionPort> = Arc::new(InFlightExecutionPort);
        let bus = RuntimeBus::new(repo, projections, events, dedup).with_execution(execution);

        let unknown = bus
            .ask(RuntimeQuery::InFlightCount {
                route_id: "unknown".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            unknown,
            RuntimeQueryResult::RouteNotFound { route_id } if route_id == "unknown"
        ));
    }
}
