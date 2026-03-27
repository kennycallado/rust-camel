use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::OnceCell;

use camel_api::{
    CamelError, RuntimeCommand, RuntimeCommandBus, RuntimeCommandResult, RuntimeQuery,
    RuntimeQueryBus, RuntimeQueryResult,
};

use crate::application::commands::{CommandDeps, execute_command, handle_register_internal};
use crate::application::internal_commands::InternalRuntimeCommandBus;
use crate::application::queries::{QueryDeps, execute_query};
use crate::application::route_types::RouteDefinition;
use crate::ports::{
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
        let deps = self.query_deps();
        execute_query(&deps, query).await
    }
}

#[async_trait]
impl InternalRuntimeCommandBus for RuntimeBus {
    async fn register_route(&self, def: RouteDefinition) -> Result<(), CamelError> {
        self.ensure_journal_recovered().await?;
        let deps = self.deps();
        handle_register_internal(&deps, def).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    use crate::application::internal_commands::InternalRuntimeCommandBus;
    use crate::application::route_types::RouteDefinition;
    use crate::domain::{RouteRuntimeAggregate, RuntimeEvent};
    use crate::ports::RouteStatusProjection;

    #[derive(Clone, Default)]
    struct InMemoryTestRepo {
        routes: Arc<Mutex<HashMap<String, RouteRuntimeAggregate>>>,
    }

    #[async_trait]
    impl RouteRepositoryPort for InMemoryTestRepo {
        async fn load(&self, route_id: &str) -> Result<Option<RouteRuntimeAggregate>, CamelError> {
            Ok(self
                .routes
                .lock()
                .expect("lock test routes")
                .get(route_id)
                .cloned())
        }

        async fn save(&self, aggregate: RouteRuntimeAggregate) -> Result<(), CamelError> {
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
        ) -> Result<(), CamelError> {
            let route_id = aggregate.route_id().to_string();
            let mut routes = self.routes.lock().expect("lock test routes");
            let current = routes.get(&route_id).ok_or_else(|| {
                CamelError::RouteError(format!(
                    "optimistic lock conflict for route '{route_id}': route not found"
                ))
            })?;

            if current.version() != expected_version {
                return Err(CamelError::RouteError(format!(
                    "optimistic lock conflict for route '{route_id}': expected version {expected_version}, actual {}",
                    current.version()
                )));
            }

            routes.insert(route_id, aggregate);
            Ok(())
        }

        async fn delete(&self, route_id: &str) -> Result<(), CamelError> {
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
        async fn upsert_status(&self, status: RouteStatusProjection) -> Result<(), CamelError> {
            self.statuses
                .lock()
                .expect("lock test statuses")
                .insert(status.route_id.clone(), status);
            Ok(())
        }

        async fn get_status(
            &self,
            route_id: &str,
        ) -> Result<Option<RouteStatusProjection>, CamelError> {
            Ok(self
                .statuses
                .lock()
                .expect("lock test statuses")
                .get(route_id)
                .cloned())
        }

        async fn list_statuses(&self) -> Result<Vec<RouteStatusProjection>, CamelError> {
            Ok(self
                .statuses
                .lock()
                .expect("lock test statuses")
                .values()
                .cloned()
                .collect())
        }

        async fn remove_status(&self, route_id: &str) -> Result<(), CamelError> {
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
        async fn publish(&self, _events: &[RuntimeEvent]) -> Result<(), CamelError> {
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct InMemoryTestDedup {
        seen: Arc<Mutex<HashSet<String>>>,
    }

    #[async_trait]
    impl CommandDedupPort for InMemoryTestDedup {
        async fn first_seen(&self, command_id: &str) -> Result<bool, CamelError> {
            let mut seen = self.seen.lock().expect("lock dedup set");
            Ok(seen.insert(command_id.to_string()))
        }

        async fn forget_seen(&self, command_id: &str) -> Result<(), CamelError> {
            self.seen.lock().expect("lock dedup set").remove(command_id);
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
}
