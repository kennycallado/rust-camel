use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::OnceCell;

use camel_api::{
    CamelError, RuntimeCommand, RuntimeCommandBus, RuntimeCommandResult, RuntimeQuery,
    RuntimeQueryBus, RuntimeQueryResult,
};

use crate::application::commands::{CommandDeps, execute_command};
use crate::application::queries::{QueryDeps, execute_query};
use crate::domain::{RouteRuntimeAggregate, RouteRuntimeState, RuntimeEvent};
use crate::ports::{
    CommandDedupPort, EventPublisherPort, ProjectionStorePort, RouteRepositoryPort,
    RouteStatusProjection, RuntimeExecutionPort, RuntimeUnitOfWorkPort,
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

    /// Seed runtime aggregate/projection/event state for routes registered through
    /// non-CQRS bootstrap paths (e.g. CamelContext::add_route_definition).
    pub async fn bootstrap_register_route(&self, route_id: String) -> Result<(), CamelError> {
        if self.repo.load(&route_id).await?.is_some() {
            return Err(CamelError::RouteError(format!(
                "route '{route_id}' is already registered in runtime state"
            )));
        }

        let aggregate =
            RouteRuntimeAggregate::from_snapshot(route_id.clone(), RouteRuntimeState::Stopped, 0);
        let projection = RouteStatusProjection {
            route_id: route_id.clone(),
            status: "Stopped".to_string(),
        };
        let events = vec![
            RuntimeEvent::RouteRegistered {
                route_id: route_id.clone(),
            },
            RuntimeEvent::RouteStopped { route_id },
        ];

        if let Some(uow) = &self.uow {
            uow.persist_upsert(aggregate, None, projection, &events)
                .await
        } else {
            self.repo.save(aggregate).await?;
            self.projections.upsert_status(projection).await?;
            self.events.publish(&events).await
        }
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
