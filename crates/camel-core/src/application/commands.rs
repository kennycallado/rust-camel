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

use crate::domain::{BuilderStep, DeclarativeWhenStep, RouteDefinition};
use crate::domain::{
    RouteLifecycleCommand, RouteRuntimeAggregate, RouteRuntimeState, RuntimeEvent,
};
use crate::ports::{
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
        return Err(CamelError::RouteError(format!(
            "route '{route_id}' is already registered"
        )));
    }

    if let Some(execution) = &deps.execution {
        let route_definition = canonical_to_route_definition(spec)?;
        execution.register_route(route_definition).await?;
    }

    let aggregate = RouteRuntimeAggregate::new(route_id.clone());
    let events = vec![RuntimeEvent::RouteRegistered {
        route_id: route_id.clone(),
    }];
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
        RouteLifecycleCommand::Stop => execution.stop_route(route_id).await,
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
        RouteLifecycleCommand::Reload => execution.reload_route(route_id).await,
        RouteLifecycleCommand::Fail(_) => Ok(()),
    }
}

async fn handle_remove(
    deps: &CommandDeps,
    route_id: String,
) -> Result<RuntimeCommandResult, CamelError> {
    let aggregate = deps
        .repo
        .load(&route_id)
        .await?
        .ok_or_else(|| CamelError::RouteError(format!("route '{route_id}' not found")))?;

    match aggregate.state() {
        RouteRuntimeState::Registered | RouteRuntimeState::Stopped => {}
        _ => {
            return Err(CamelError::RouteError(format!(
                "invalid transition: {:?} -> Removed",
                aggregate.state()
            )));
        }
    }

    if let Some(execution) = &deps.execution {
        remove_runtime_route_with_recovery(execution.as_ref(), &route_id).await?;
    }

    let events = vec![RuntimeEvent::RouteRemoved {
        route_id: route_id.clone(),
    }];
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
        Err(err) => Err(err),
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
    }
}
