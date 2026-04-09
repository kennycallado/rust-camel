use std::sync::Arc;
use std::time::Duration;

use camel_api::{
    AggregatorConfig,
    aggregator::AggregationStrategy as CanonicalAggregateStrategy,
    circuit_breaker::CircuitBreakerConfig,
    splitter::{
        AggregationStrategy as CanonicalSplitAggregation, SplitterConfig, split_body_json_array,
        split_body_lines,
    },
};
use camel_api::{CamelError, RuntimeCommand, RuntimeCommandResult};

use crate::lifecycle::application::route_definition::{
    BuilderStep, DeclarativeWhenStep, RouteDefinition,
};
use crate::lifecycle::domain::{
    DomainError, RouteLifecycleCommand, RouteRuntimeAggregate, RouteRuntimeState,
};
use crate::lifecycle::ports::{
    EventPublisherPort, ProjectionStorePort, RouteRepositoryPort, RouteStatusProjection,
    RuntimeExecutionPort, RuntimeUnitOfWorkPort,
};
use camel_processor::LogLevel;

pub struct CommandDeps {
    pub repo: Arc<dyn RouteRepositoryPort>,
    pub projections: Arc<dyn ProjectionStorePort>,
    pub events: Arc<dyn EventPublisherPort>,
    pub uow: Option<Arc<dyn RuntimeUnitOfWorkPort>>,
    pub execution: Option<Arc<dyn RuntimeExecutionPort>>,
}

pub async fn execute_command(
    deps: &CommandDeps,
    cmd: RuntimeCommand,
) -> Result<RuntimeCommandResult, CamelError> {
    match cmd {
        RuntimeCommand::RegisterRoute { spec, .. } => handle_register(deps, spec).await,
        RuntimeCommand::StartRoute { route_id, .. } => {
            handle_lifecycle(deps, route_id, RouteLifecycleCommand::Start).await
        }
        RuntimeCommand::StopRoute { route_id, .. } => {
            handle_lifecycle(deps, route_id, RouteLifecycleCommand::Stop).await
        }
        RuntimeCommand::SuspendRoute { route_id, .. } => {
            handle_lifecycle(deps, route_id, RouteLifecycleCommand::Suspend).await
        }
        RuntimeCommand::ResumeRoute { route_id, .. } => {
            handle_lifecycle(deps, route_id, RouteLifecycleCommand::Resume).await
        }
        RuntimeCommand::ReloadRoute { route_id, .. } => {
            handle_lifecycle(deps, route_id, RouteLifecycleCommand::Reload).await
        }
        RuntimeCommand::FailRoute {
            route_id, error, ..
        } => handle_lifecycle(deps, route_id, RouteLifecycleCommand::Fail(error)).await,
        RuntimeCommand::RemoveRoute { route_id, .. } => handle_remove(deps, route_id).await,
    }
}

async fn handle_register(
    deps: &CommandDeps,
    spec: camel_api::CanonicalRouteSpec,
) -> Result<RuntimeCommandResult, CamelError> {
    spec.validate_contract()?;
    let route_id = spec.route_id.clone();

    if deps.repo.load(&route_id).await?.is_some() {
        return Err(DomainError::AlreadyExists(route_id).into());
    }

    if let Some(execution) = &deps.execution {
        let route_definition = canonical_to_route_definition(spec)?;
        execution.register_route(route_definition).await?;
    }

    let (aggregate, events) = RouteRuntimeAggregate::register(route_id.clone());
    if let Some(uow) = &deps.uow {
        uow.persist_upsert(
            aggregate.clone(),
            None,
            project_from_aggregate(&aggregate),
            &events,
        )
        .await?;
    } else {
        deps.repo.save(aggregate.clone()).await?;

        if let Some(primary_error) = upsert_projection_with_reconciliation(
            &*deps.projections,
            project_from_aggregate(&aggregate),
        )
        .await?
        {
            deps.events.publish(&events).await?;
            return Err(CamelError::RouteError(format!(
                "post-effect reconciliation recovered after runtime persistence error: {primary_error}"
            )));
        }

        deps.events.publish(&events).await?;
    }

    Ok(RuntimeCommandResult::RouteRegistered { route_id })
}

pub(crate) async fn handle_register_internal(
    deps: &CommandDeps,
    def: RouteDefinition,
) -> Result<RuntimeCommandResult, CamelError> {
    let route_id = def.route_id().to_string();

    if deps.repo.load(&route_id).await?.is_some() {
        return Err(DomainError::AlreadyExists(route_id).into());
    }

    if let Some(execution) = &deps.execution {
        execution.register_route(def).await?;
    }

    let (aggregate, events) = RouteRuntimeAggregate::register(route_id.clone());

    let persist_result: Result<(), CamelError> = if let Some(uow) = &deps.uow {
        uow.persist_upsert(
            aggregate.clone(),
            None,
            project_from_aggregate(&aggregate),
            &events,
        )
        .await
        .map_err(Into::into)
    } else {
        match deps.repo.save(aggregate.clone()).await {
            Ok(()) => {
                if let Some(primary_error) = upsert_projection_with_reconciliation(
                    &*deps.projections,
                    project_from_aggregate(&aggregate),
                )
                .await?
                {
                    deps.events.publish(&events).await?;
                    Err(CamelError::RouteError(format!(
                        "post-effect reconciliation recovered after runtime persistence error: {primary_error}"
                    )))
                } else {
                    deps.events.publish(&events).await.map_err(Into::into)
                }
            }
            Err(err) => Err(err.into()),
        }
    };

    if let Err(persist_err) = persist_result {
        if let Some(execution) = &deps.execution
            && let Err(rollback_err) = execution.remove_route(&route_id).await
        {
            tracing::error!(
                route_id = %route_id,
                persist_error = %persist_err,
                rollback_error = %rollback_err,
                "INCONSISTENCY: route installed but state persist failed and rollback also failed — manual reconciliation required"
            );
            return Err(CamelError::RouteError(format!(
                "route '{route_id}' registration inconsistency: persist failed ({persist_err}) and rollback failed ({rollback_err}) — manual reconciliation required"
            )));
        }
        return Err(persist_err);
    }

    Ok(RuntimeCommandResult::RouteRegistered { route_id })
}

async fn handle_lifecycle(
    deps: &CommandDeps,
    route_id: String,
    command: RouteLifecycleCommand,
) -> Result<RuntimeCommandResult, CamelError> {
    let mut aggregate = deps
        .repo
        .load(&route_id)
        .await?
        .ok_or_else(|| CamelError::RouteError(format!("route '{route_id}' not found")))?;
    let expected_version = aggregate.version();

    let execution_command = command.clone();
    let events = aggregate.apply_command(command)?;

    if matches!(execution_command, RouteLifecycleCommand::Reload) && deps.execution.is_none() {
        return Err(CamelError::RouteError(
            "reload requires connected runtime route controller".to_string(),
        ));
    }

    if let Some(execution) = &deps.execution {
        apply_runtime_lifecycle(execution.as_ref(), &route_id, &execution_command).await?;
    }

    if let Some(uow) = &deps.uow {
        uow.persist_upsert(
            aggregate.clone(),
            Some(expected_version),
            project_from_aggregate(&aggregate),
            &events,
        )
        .await?;
    } else {
        deps.repo
            .save_if_version(aggregate.clone(), expected_version)
            .await?;

        let projection_recovered_error = upsert_projection_with_reconciliation(
            &*deps.projections,
            project_from_aggregate(&aggregate),
        )
        .await?;
        deps.events.publish(&events).await?;
        if let Some(primary_error) = projection_recovered_error {
            return Err(CamelError::RouteError(format!(
                "post-effect reconciliation recovered after runtime persistence error: {primary_error}"
            )));
        }
    }

    let status = state_label(aggregate.state()).to_string();
    Ok(RuntimeCommandResult::RouteStateChanged { route_id, status })
}

async fn apply_runtime_lifecycle(
    execution: &dyn RuntimeExecutionPort,
    route_id: &str,
    command: &RouteLifecycleCommand,
) -> Result<(), CamelError> {
    match command {
        RouteLifecycleCommand::Start => match execution.start_route(route_id).await {
            Ok(()) => Ok(()),
            Err(start_error) => execution.resume_route(route_id).await.map_err(|resume_error| {
                CamelError::RouteError(format!(
                    "runtime execution recovery failed for Start on route '{route_id}': start_error={start_error}; resume_error={resume_error}"
                ))
            }),
        },
        RouteLifecycleCommand::Stop => execution.stop_route(route_id).await.map_err(Into::into),
        RouteLifecycleCommand::Suspend => match execution.suspend_route(route_id).await {
            Ok(()) => Ok(()),
            Err(suspend_error) => {
                execution.resume_route(route_id).await.map_err(|resume_error| {
                    CamelError::RouteError(format!(
                        "runtime execution recovery failed for Suspend on route '{route_id}': suspend_error={suspend_error}; resume_error={resume_error}"
                    ))
                })?;
                execution
                    .suspend_route(route_id)
                    .await
                    .map_err(|retry_error| {
                        CamelError::RouteError(format!(
                            "runtime execution recovery failed for Suspend on route '{route_id}': suspend_error={suspend_error}; retry_error={retry_error}"
                        ))
                    })
            }
        },
        RouteLifecycleCommand::Resume => match execution.resume_route(route_id).await {
            Ok(()) => Ok(()),
            Err(resume_error) => execution.start_route(route_id).await.map_err(|start_error| {
                CamelError::RouteError(format!(
                    "runtime execution recovery failed for Resume on route '{route_id}': resume_error={resume_error}; start_error={start_error}"
                ))
            }),
        },
        RouteLifecycleCommand::Reload => execution.reload_route(route_id).await.map_err(Into::into),
        RouteLifecycleCommand::Fail(_) => Ok(()),
        RouteLifecycleCommand::Remove => execution.remove_route(route_id).await.map_err(Into::into),
    }
}

async fn handle_remove(
    deps: &CommandDeps,
    route_id: String,
) -> Result<RuntimeCommandResult, CamelError> {
    let mut aggregate = deps
        .repo
        .load(&route_id)
        .await?
        .ok_or_else(|| CamelError::RouteError(format!("route '{route_id}' not found")))?;

    let events = aggregate.apply_command(RouteLifecycleCommand::Remove)?;

    if let Some(execution) = &deps.execution {
        remove_runtime_route_with_recovery(execution.as_ref(), &route_id).await?;
    }

    if let Some(uow) = &deps.uow {
        uow.persist_delete(&route_id, &events).await?;
    } else {
        deps.repo.delete(&route_id).await?;
        deps.projections.remove_status(&route_id).await?;
        deps.events.publish(&events).await?;
    }

    Ok(RuntimeCommandResult::RouteStateChanged {
        route_id,
        status: "Removed".to_string(),
    })
}

async fn remove_runtime_route_with_recovery(
    execution: &dyn RuntimeExecutionPort,
    route_id: &str,
) -> Result<(), CamelError> {
    match execution.remove_route(route_id).await {
        Ok(()) => Ok(()),
        Err(remove_error) if remove_error.to_string().contains("must be stopped") => {
            execution.stop_route(route_id).await.map_err(|stop_error| {
                CamelError::RouteError(format!(
                    "runtime execution recovery failed for Remove on route '{route_id}': remove_error={remove_error}; stop_error={stop_error}"
                ))
            })?;
            execution.remove_route(route_id).await.map_err(|retry_error| {
                CamelError::RouteError(format!(
                    "runtime execution recovery failed for Remove on route '{route_id}': remove_error={remove_error}; retry_remove_error={retry_error}"
                ))
            })
        }
        Err(err) => Err(err.into()),
    }
}

fn project_from_aggregate(aggregate: &RouteRuntimeAggregate) -> RouteStatusProjection {
    RouteStatusProjection {
        route_id: aggregate.route_id().to_string(),
        status: state_label(aggregate.state()).to_string(),
    }
}

fn state_label(state: &RouteRuntimeState) -> &'static str {
    match state {
        RouteRuntimeState::Registered => "Registered",
        RouteRuntimeState::Starting => "Starting",
        RouteRuntimeState::Started => "Started",
        RouteRuntimeState::Suspended => "Suspended",
        RouteRuntimeState::Stopping => "Stopping",
        RouteRuntimeState::Stopped => "Stopped",
        RouteRuntimeState::Failed(_) => "Failed",
    }
}

async fn upsert_projection_with_reconciliation(
    projections: &dyn ProjectionStorePort,
    status: RouteStatusProjection,
) -> Result<Option<String>, CamelError> {
    match projections.upsert_status(status.clone()).await {
        Ok(()) => Ok(None),
        Err(primary_error) => {
            projections
                .upsert_status(status)
                .await
                .map_err(|reconcile_error| {
                    CamelError::RouteError(format!(
                        "post-effect reconciliation failed: primary={primary_error}; reconcile={reconcile_error}"
                    ))
                })?;
            Ok(Some(primary_error.to_string()))
        }
    }
}

fn canonical_to_route_definition(
    spec: camel_api::CanonicalRouteSpec,
) -> Result<RouteDefinition, CamelError> {
    let route_id = spec.route_id.clone();
    let steps = canonical_steps_to_builder_steps(spec.steps)?;
    let mut definition = RouteDefinition::new(spec.from, steps).with_route_id(&route_id);
    if let Some(cb) = spec.circuit_breaker {
        definition = definition.with_circuit_breaker(
            CircuitBreakerConfig::new()
                .failure_threshold(cb.failure_threshold)
                .open_duration(Duration::from_millis(cb.open_duration_ms)),
        );
    }

    Ok(definition)
}

fn canonical_steps_to_builder_steps(
    steps: Vec<camel_api::runtime::CanonicalStepSpec>,
) -> Result<Vec<BuilderStep>, CamelError> {
    let mut converted = Vec::with_capacity(steps.len());
    for step in steps {
        converted.push(canonical_step_to_builder_step(step)?);
    }
    Ok(converted)
}

fn canonical_step_to_builder_step(
    step: camel_api::runtime::CanonicalStepSpec,
) -> Result<BuilderStep, CamelError> {
    match step {
        camel_api::runtime::CanonicalStepSpec::To { uri } => Ok(BuilderStep::To(uri)),
        camel_api::runtime::CanonicalStepSpec::Log { message } => Ok(BuilderStep::Log {
            level: LogLevel::Info,
            message,
        }),
        camel_api::runtime::CanonicalStepSpec::WireTap { uri } => Ok(BuilderStep::WireTap { uri }),
        camel_api::runtime::CanonicalStepSpec::Script { expression } => {
            Ok(BuilderStep::DeclarativeScript { expression })
        }
        camel_api::runtime::CanonicalStepSpec::Filter { predicate, steps } => {
            Ok(BuilderStep::DeclarativeFilter {
                predicate,
                steps: canonical_steps_to_builder_steps(steps)?,
            })
        }
        camel_api::runtime::CanonicalStepSpec::Choice { whens, otherwise } => {
            let mut converted_whens = Vec::with_capacity(whens.len());
            for when in whens {
                converted_whens.push(DeclarativeWhenStep {
                    predicate: when.predicate,
                    steps: canonical_steps_to_builder_steps(when.steps)?,
                });
            }
            let converted_otherwise = match otherwise {
                Some(steps) => Some(canonical_steps_to_builder_steps(steps)?),
                None => None,
            };
            Ok(BuilderStep::DeclarativeChoice {
                whens: converted_whens,
                otherwise: converted_otherwise,
            })
        }
        camel_api::runtime::CanonicalStepSpec::Split {
            expression,
            aggregation,
            parallel,
            parallel_limit,
            stop_on_exception,
            steps,
        } => {
            let aggregation = match aggregation {
                camel_api::runtime::CanonicalSplitAggregationSpec::LastWins => {
                    CanonicalSplitAggregation::LastWins
                }
                camel_api::runtime::CanonicalSplitAggregationSpec::CollectAll => {
                    CanonicalSplitAggregation::CollectAll
                }
                camel_api::runtime::CanonicalSplitAggregationSpec::Original => {
                    CanonicalSplitAggregation::Original
                }
            };
            let steps = canonical_steps_to_builder_steps(steps)?;
            match expression {
                camel_api::runtime::CanonicalSplitExpressionSpec::BodyLines => {
                    let mut config = SplitterConfig::new(split_body_lines())
                        .aggregation(aggregation)
                        .parallel(parallel)
                        .stop_on_exception(stop_on_exception);
                    if let Some(limit) = parallel_limit {
                        config = config.parallel_limit(limit);
                    }
                    Ok(BuilderStep::Split { config, steps })
                }
                camel_api::runtime::CanonicalSplitExpressionSpec::BodyJsonArray => {
                    let mut config = SplitterConfig::new(split_body_json_array())
                        .aggregation(aggregation)
                        .parallel(parallel)
                        .stop_on_exception(stop_on_exception);
                    if let Some(limit) = parallel_limit {
                        config = config.parallel_limit(limit);
                    }
                    Ok(BuilderStep::Split { config, steps })
                }
                camel_api::runtime::CanonicalSplitExpressionSpec::Language(expression) => {
                    Ok(BuilderStep::DeclarativeSplit {
                        expression,
                        aggregation,
                        parallel,
                        parallel_limit,
                        stop_on_exception,
                        steps,
                    })
                }
            }
        }
        camel_api::runtime::CanonicalStepSpec::Aggregate { config } => {
            let mut builder = AggregatorConfig::correlate_by(config.header)
                .complete_when_size(config.completion_size.unwrap_or(1));
            builder = match config.strategy {
                camel_api::runtime::CanonicalAggregateStrategySpec::CollectAll => {
                    builder.strategy(CanonicalAggregateStrategy::CollectAll)
                }
            };
            if let Some(max_buckets) = config.max_buckets {
                builder = builder.max_buckets(max_buckets);
            }
            if let Some(ttl_ms) = config.bucket_ttl_ms {
                builder = builder.bucket_ttl(Duration::from_millis(ttl_ms));
            }
            Ok(BuilderStep::Aggregate {
                config: builder.build(),
            })
        }
        camel_api::runtime::CanonicalStepSpec::Stop => Ok(BuilderStep::Stop),
        camel_api::runtime::CanonicalStepSpec::Delay {
            delay_ms,
            dynamic_header,
        } => Ok(BuilderStep::Delay {
            config: camel_api::DelayConfig {
                delay_ms,
                dynamic_header,
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use crate::lifecycle::domain::DomainError;

    use super::*;
    use camel_api::RuntimeQueryResult;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use crate::lifecycle::domain::RuntimeEvent;
    use async_trait::async_trait;

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
        fail_start: Arc<Mutex<bool>>,
        fail_suspend_once: Arc<Mutex<bool>>,
        fail_suspend_retry: Arc<Mutex<bool>>,
        fail_resume: Arc<Mutex<bool>>,
        fail_remove_must_stopped: Arc<Mutex<bool>>,
        fail_remove_retry: Arc<Mutex<bool>>,
        stop_called: Arc<Mutex<u32>>,
    }

    impl ConfigurableExecutionPort {
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

        async fn in_flight_count(&self, route_id: &str) -> Result<RuntimeQueryResult, DomainError> {
            Ok(RuntimeQueryResult::RouteNotFound {
                route_id: route_id.to_string(),
            })
        }
    }

    #[async_trait]
    impl RuntimeExecutionPort for ConfigurableExecutionPort {
        async fn register_route(&self, _definition: RouteDefinition) -> Result<(), DomainError> {
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

        async fn in_flight_count(&self, route_id: &str) -> Result<RuntimeQueryResult, DomainError> {
            Ok(RuntimeQueryResult::RouteNotFound {
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
        }
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
    }

    #[test]
    fn canonical_route_conversion_with_circuit_breaker() {
        use camel_api::runtime::{
            CanonicalCircuitBreakerSpec, CanonicalRouteSpec, CanonicalStepSpec,
        };

        let spec = CanonicalRouteSpec {
            route_id: "route-x".into(),
            from: "timer:tick".into(),
            steps: vec![CanonicalStepSpec::Stop],
            circuit_breaker: Some(CanonicalCircuitBreakerSpec {
                failure_threshold: 3,
                open_duration_ms: 500,
            }),
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

    #[test]
    fn state_label_covers_all_states() {
        assert_eq!(state_label(&RouteRuntimeState::Registered), "Registered");
        assert_eq!(state_label(&RouteRuntimeState::Starting), "Starting");
        assert_eq!(state_label(&RouteRuntimeState::Started), "Started");
        assert_eq!(state_label(&RouteRuntimeState::Suspended), "Suspended");
        assert_eq!(state_label(&RouteRuntimeState::Stopping), "Stopping");
        assert_eq!(state_label(&RouteRuntimeState::Stopped), "Stopped");
        assert_eq!(
            state_label(&RouteRuntimeState::Failed("e".into())),
            "Failed"
        );
    }
}
