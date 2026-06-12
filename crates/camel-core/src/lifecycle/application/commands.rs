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

use crate::health_registry::HealthCheckRegistry;
use crate::lifecycle::application::route_definition::{
    BuilderStep, DeclarativeWhenStep, RouteDefinition,
};
use crate::lifecycle::domain::{
    DomainError, RouteLifecycleCommand, RouteRuntimeAggregate, RuntimeEvent,
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
    pub health_registry: Option<Arc<HealthCheckRegistry>>,
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

    // 1. Create aggregate and persist FIRST
    let (mut aggregate, events) = RouteRuntimeAggregate::register(route_id.clone());

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

    // 2. Execute runtime side effect
    if let Some(execution) = &deps.execution {
        let route_definition = canonical_to_route_definition(spec)?;
        if let Err(runtime_err) = execution.register_route(route_definition).await {
            // 3. Compensate: mark as Failed
            let version_before_fail = aggregate.version();
            let fail_events = aggregate.fail(runtime_err.to_string());
            let fail_proj = project_from_aggregate(&aggregate);
            if let Some(uow) = &deps.uow {
                if let Err(persist_err) = uow
                    .persist_upsert(
                        aggregate.clone(),
                        Some(version_before_fail),
                        fail_proj,
                        &fail_events,
                    )
                    .await
                {
                    // log-policy: system-broken
                    tracing::error!(
                        route_id = %route_id,
                        runtime_error = %runtime_err,
                        persist_error = %persist_err,
                        "INCONSISTENCY: runtime register failed and Failed-state persist also failed"
                    );
                }
            } else {
                match deps
                    .repo
                    .save_if_version(aggregate.clone(), version_before_fail)
                    .await
                {
                    Ok(()) => {
                        if let Err(proj_err) =
                            upsert_projection_with_reconciliation(&*deps.projections, fail_proj)
                                .await
                        {
                            // log-policy: system-broken
                            tracing::error!(
                                route_id = %route_id,
                                projection_error = %proj_err,
                                "INCONSISTENCY: projection update failed after compensation persist"
                            );
                        }
                        if let Err(pub_err) = deps.events.publish(&fail_events).await {
                            // log-policy: system-broken
                            tracing::error!(
                                route_id = %route_id,
                                publish_error = %pub_err,
                                "INCONSISTENCY: failed to publish failure events during compensation"
                            );
                        }
                    }
                    Err(persist_err) => {
                        // log-policy: system-broken
                        tracing::error!(
                            route_id = %route_id,
                            persist_error = %persist_err,
                            "INCONSISTENCY: compensation persist failed — concurrent modification, events not published"
                        );
                    }
                }
            }
            return Err(CamelError::RouteError(format!(
                "runtime registration failed for route '{route_id}': {runtime_err}"
            )));
        }
    }

    Ok(RuntimeCommandResult::RouteRegistered { route_id })
}

// NOTE: handle_register_internal uses runtime-first ordering intentionally.
// It is called from runtime_bus for internal routes where the runtime context
// is already validated. The public handle_register uses persist-first for
// crash-safety. See the Phase 1 remediation plan for the design rationale.
// TODO(unify-register): After Phase 1, these two functions should be unified
// (tracked as follow-up bd issue).
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
            // log-policy: system-broken
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

#[allow(clippy::collapsible_if)]
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

    // Two-phase Start: dispatch to dedicated handler
    if matches!(execution_command, RouteLifecycleCommand::Start) {
        return handle_lifecycle_start(deps, aggregate, expected_version).await;
    }
    // Fail command: always handled via apply_command (no runtime side effect)
    if let RouteLifecycleCommand::Fail(error) = &execution_command {
        let events = aggregate.apply_command(execution_command.clone())?;
        if let Some(health_registry) = &deps.health_registry {
            health_registry.force_unhealthy_for_route(
                &route_id,
                &format!("route:{route_id}"),
                format!("route failed: {error}"),
            );
        }
        return persist_and_return(deps, aggregate, expected_version, events, route_id).await;
    }

    // Reload guard: runtime execution port required
    if matches!(execution_command, RouteLifecycleCommand::Reload) && deps.execution.is_none() {
        return Err(CamelError::RouteError(
            "reload requires connected runtime route controller".to_string(),
        ));
    }

    // Pre-validate domain transition before executing runtime side effect
    {
        let mut validation_agg = aggregate.clone();
        let validation_events = validation_agg.apply_command(execution_command.clone())?;
        if validation_events.is_empty() {
            // Idempotent: already in target state, no-op
            return Ok(RuntimeCommandResult::RouteStateChanged {
                route_id,
                status: format!("{:?}", aggregate.state()),
            });
        }
    }

    // Stop/Suspend/Resume/Reload: execute runtime FIRST, then apply_command on success
    if let Some(execution) = &deps.execution {
        if let Err(runtime_err) =
            apply_runtime_lifecycle(execution.as_ref(), &route_id, &execution_command).await
        {
            // Runtime execution failed: persist Failed state as compensation
            let fail_events = aggregate.fail(runtime_err.to_string());
            let fail_proj = project_from_aggregate(&aggregate);
            if let Some(uow) = &deps.uow {
                if let Err(persist_err) = uow
                    .persist_upsert(
                        aggregate.clone(),
                        Some(expected_version),
                        fail_proj,
                        &fail_events,
                    )
                    .await
                {
                    // log-policy: system-broken
                    tracing::error!(
                        route_id = %route_id,
                        runtime_error = %runtime_err,
                        persist_error = %persist_err,
                        "INCONSISTENCY: runtime operation failed and Failed-state persist also failed"
                    );
                }
            } else {
                match deps
                    .repo
                    .save_if_version(aggregate.clone(), expected_version)
                    .await
                {
                    Ok(()) => {
                        if let Err(proj_err) =
                            upsert_projection_with_reconciliation(&*deps.projections, fail_proj)
                                .await
                        {
                            // log-policy: system-broken
                            tracing::error!(
                                route_id = %route_id,
                                projection_error = %proj_err,
                                "INCONSISTENCY: projection update failed after compensation persist"
                            );
                        }
                        if let Err(pub_err) = deps.events.publish(&fail_events).await {
                            // log-policy: system-broken
                            tracing::error!(
                                route_id = %route_id,
                                publish_error = %pub_err,
                                "INCONSISTENCY: failed to publish failure events during compensation"
                            );
                        }
                    }
                    Err(persist_err) => {
                        // log-policy: system-broken
                        tracing::error!(
                            route_id = %route_id,
                            persist_error = %persist_err,
                            "INCONSISTENCY: compensation persist failed — concurrent modification, events not published"
                        );
                    }
                }
            }
            return Err(CamelError::RouteError(format!(
                "runtime operation failed for route '{route_id}': {runtime_err}"
            )));
        }
    }

    // Runtime succeeded: apply command and persist
    let events = aggregate.apply_command(execution_command.clone())?;
    persist_and_return(deps, aggregate, expected_version, events, route_id).await
}

#[allow(clippy::collapsible_if)]
async fn handle_lifecycle_start(
    deps: &CommandDeps,
    mut aggregate: RouteRuntimeAggregate,
    expected_version: u64,
) -> Result<RuntimeCommandResult, CamelError> {
    let route_id = aggregate.route_id().to_string();

    let intent_events = aggregate.begin_start().map_err(|_| {
        CamelError::RouteError(format!(
            "invalid transition: route '{}' is in {:?} state, cannot start",
            route_id,
            aggregate.state(),
        ))
    })?;

    if intent_events.is_empty() {
        return Err(CamelError::RouteError(format!(
            "invalid transition: route '{}' is in {:?} state, cannot start",
            route_id,
            aggregate.state(),
        )));
    }

    // Phase 1: persist intent (Starting state)
    if let Some(uow) = &deps.uow {
        uow.persist_upsert(
            aggregate.clone(),
            Some(expected_version),
            project_from_aggregate(&aggregate),
            &intent_events,
        )
        .await?;
    } else {
        deps.repo
            .save_if_version(aggregate.clone(), expected_version)
            .await?;
        let _ = upsert_projection_with_reconciliation(
            &*deps.projections,
            project_from_aggregate(&aggregate),
        )
        .await?;
        deps.events.publish(&intent_events).await?;
    }

    // Phase 2: execute runtime side effect
    if let Some(execution) = &deps.execution {
        if let Err(runtime_err) =
            apply_runtime_lifecycle(execution.as_ref(), &route_id, &RouteLifecycleCommand::Start)
                .await
        {
            // Compensate: mark as Failed
            // Capture version before fail() bumps it — the stored aggregate
            // is at version V+1 (after begin_start), and fail() increments to V+2.
            let version_before_fail = aggregate.version();
            let fail_events = aggregate.fail(runtime_err.to_string());
            let fail_proj = project_from_aggregate(&aggregate);
            if let Some(uow) = &deps.uow {
                if let Err(persist_err) = uow
                    .persist_upsert(
                        aggregate.clone(),
                        Some(version_before_fail),
                        fail_proj,
                        &fail_events,
                    )
                    .await
                {
                    // log-policy: system-broken
                    tracing::error!(
                        route_id = %route_id,
                        runtime_error = %runtime_err,
                        persist_error = %persist_err,
                        "INCONSISTENCY: runtime start failed and Failed-state persist also failed -- manual reconciliation required"
                    );
                }
            } else {
                match deps
                    .repo
                    .save_if_version(aggregate.clone(), version_before_fail)
                    .await
                {
                    Ok(()) => {
                        if let Err(proj_err) =
                            upsert_projection_with_reconciliation(&*deps.projections, fail_proj)
                                .await
                        {
                            // log-policy: system-broken
                            tracing::error!(
                                route_id = %route_id,
                                projection_error = %proj_err,
                                "INCONSISTENCY: projection update failed after compensation persist"
                            );
                        }
                        if let Err(pub_err) = deps.events.publish(&fail_events).await {
                            // log-policy: system-broken
                            tracing::error!(
                                route_id = %route_id,
                                publish_error = %pub_err,
                                "INCONSISTENCY: failed to publish failure events during compensation"
                            );
                        }
                    }
                    Err(persist_err) => {
                        // log-policy: system-broken
                        tracing::error!(
                            route_id = %route_id,
                            persist_error = %persist_err,
                            "INCONSISTENCY: compensation persist failed — concurrent modification, events not published"
                        );
                    }
                }
            }
            return Err(CamelError::RouteError(format!(
                "runtime start failed for route '{route_id}': {runtime_err}"
            )));
        }
    }

    // Phase 2a: confirm Started
    let confirm_events = aggregate.confirm_start().map_err(CamelError::from)?;
    if !confirm_events.is_empty() {
        if let Some(uow) = &deps.uow {
            uow.persist_upsert(
                aggregate.clone(),
                Some(aggregate.version()),
                project_from_aggregate(&aggregate),
                &confirm_events,
            )
            .await?;
        } else {
            deps.repo
                .save_if_version(aggregate.clone(), aggregate.version())
                .await?;
            let _ = upsert_projection_with_reconciliation(
                &*deps.projections,
                project_from_aggregate(&aggregate),
            )
            .await?;
            deps.events.publish(&confirm_events).await?;
        }
    }

    Ok(RuntimeCommandResult::RouteStateChanged {
        route_id,
        status: "Started".to_string(),
    })
}

/// Shared persist flow used by non-Start lifecycle commands.
async fn persist_and_return(
    deps: &CommandDeps,
    aggregate: RouteRuntimeAggregate,
    expected_version: u64,
    events: Vec<RuntimeEvent>,
    route_id: String,
) -> Result<RuntimeCommandResult, CamelError> {
    if events.is_empty() {
        let status = aggregate.state_label().to_string();
        return Ok(RuntimeCommandResult::RouteStateChanged { route_id, status });
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

    let status = aggregate.state_label().to_string();
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
    let expected_version = aggregate.version();

    // Pre-validate Remove transition on a cloned aggregate
    {
        let mut validation = aggregate.clone();
        if let Err(e) = validation.apply_command(RouteLifecycleCommand::Remove) {
            return Err(CamelError::RouteError(format!(
                "invalid transition for route '{route_id}': {e}"
            )));
        }
    }

    // Execute runtime stop
    if let Some(execution) = &deps.execution
        && let Err(stop_err) =
            remove_runtime_route_with_recovery(execution.as_ref(), &route_id).await
    {
        // Stop failed: fail() on ORIGINAL aggregate (no Remove applied)
        let fail_events = aggregate.fail(stop_err.to_string());
        let fail_proj = project_from_aggregate(&aggregate);
        if let Some(uow) = &deps.uow {
            if let Err(persist_err) = uow
                .persist_upsert(
                    aggregate.clone(),
                    Some(expected_version),
                    fail_proj,
                    &fail_events,
                )
                .await
            {
                // log-policy: system-broken
                tracing::error!(
                    route_id = %route_id,
                    runtime_error = %stop_err,
                    persist_error = %persist_err,
                    "INCONSISTENCY: remove runtime stop failed and Failed-state persist also failed"
                );
            }
        } else {
            match deps
                .repo
                .save_if_version(aggregate.clone(), expected_version)
                .await
            {
                Ok(()) => {
                    if let Err(proj_err) =
                        upsert_projection_with_reconciliation(&*deps.projections, fail_proj).await
                    {
                        // log-policy: system-broken
                        tracing::error!(
                            route_id = %route_id,
                            projection_error = %proj_err,
                            "INCONSISTENCY: projection update failed after compensation persist"
                        );
                    }
                    if let Err(pub_err) = deps.events.publish(&fail_events).await {
                        // log-policy: system-broken
                        tracing::error!(
                            route_id = %route_id,
                            publish_error = %pub_err,
                            "INCONSISTENCY: failed to publish failure events during compensation"
                        );
                    }
                }
                Err(persist_err) => {
                    // log-policy: system-broken
                    tracing::error!(
                        route_id = %route_id,
                        persist_error = %persist_err,
                        "INCONSISTENCY: compensation persist failed — concurrent modification, events not published"
                    );
                }
            }
        }
        return Err(CamelError::RouteError(format!(
            "route '{route_id}' stop failed during removal, state persisted as Failed: {stop_err}"
        )));
    }

    // Stop succeeded: apply Remove on real aggregate
    let events = aggregate.apply_command(RouteLifecycleCommand::Remove)?;

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

pub(crate) fn project_from_aggregate(aggregate: &RouteRuntimeAggregate) -> RouteStatusProjection {
    RouteStatusProjection {
        route_id: aggregate.route_id().to_string(),
        status: aggregate.state_label().to_string(),
    }
}

pub(crate) async fn upsert_projection_with_reconciliation(
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
                camel_api::runtime::CanonicalSplitExpressionSpec::Stream(stream_config) => {
                    Ok(BuilderStep::DeclarativeStreamSplit {
                        stream_config,
                        aggregation,
                        stop_on_exception,
                        steps,
                    })
                }
            }
        }
        camel_api::runtime::CanonicalStepSpec::Aggregate(config) => {
            let completion_size = config.completion_size.unwrap_or(1);
            let mut builder = AggregatorConfig::correlate_by(&config.header);

            match (config.completion_timeout_ms, completion_size) {
                (Some(timeout_ms), size) if timeout_ms > 0 && size > 1 => {
                    builder = builder
                        .complete_on_size_or_timeout(size, Duration::from_millis(timeout_ms));
                }
                (Some(timeout_ms), _) if timeout_ms > 0 => {
                    builder = builder.complete_on_timeout(Duration::from_millis(timeout_ms));
                }
                (_, size) => {
                    builder = builder.complete_when_size(size);
                }
            }

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
            if let Some(force) = config.force_completion_on_stop {
                builder = builder.force_completion_on_stop(force);
            }
            if let Some(discard) = config.discard_on_timeout {
                builder = builder.discard_on_timeout(discard);
            }

            let mut agg_config = builder.build()?;
            if let Some(expr) = config.correlation_key {
                use camel_api::aggregator::CorrelationStrategy;
                agg_config.correlation = CorrelationStrategy::Expression {
                    expr,
                    language: "simple".to_string(),
                };
            }

            Ok(BuilderStep::Aggregate { config: agg_config })
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

        async fn in_flight_count(
            &self,
            route_id: &str,
        ) -> Result<InFlightCountResult, DomainError> {
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

        async fn in_flight_count(
            &self,
            route_id: &str,
        ) -> Result<InFlightCountResult, DomainError> {
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
            health_registry: Some(Arc::clone(&health_registry)),
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
}
