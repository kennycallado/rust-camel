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

use crate::lifecycle::application::ports::{
    EventPublisherPort, ProjectionStorePort, RouteRepositoryPort, RouteStatusProjection,
    RuntimeExecutionPort, RuntimeUnitOfWorkPort,
};
use crate::lifecycle::application::route_definition::{
    BuilderStep, DeclarativeWhenStep, RouteDefinition,
};
use crate::lifecycle::domain::{
    DomainError, RouteLifecycleCommand, RouteRuntimeAggregate, RuntimeEvent,
};
use camel_component_api::HealthCheckRegistry as HealthCheckRegistryTrait;
use camel_processor::LogLevel;

pub struct CommandDeps {
    pub repo: Arc<dyn RouteRepositoryPort>,
    pub projections: Arc<dyn ProjectionStorePort>,
    pub events: Arc<dyn EventPublisherPort>,
    pub uow: Option<Arc<dyn RuntimeUnitOfWorkPort>>,
    pub execution: Option<Arc<dyn RuntimeExecutionPort>>,
    pub health_registry: Option<Arc<dyn HealthCheckRegistryTrait>>,
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
        RuntimeCommand::ReloadTlsCerts { .. } => Err(CamelError::Config(
            "ReloadTlsCerts not handled: should be intercepted in execute()".into(),
        )),
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

// NOTE: Public registration and internal registration intentionally use
// different consistency policies.
//
// `handle_register` accepts CanonicalRouteSpec at the command boundary,
// validates the public contract, persists state first for crash-safe replay,
// then compensates runtime failure by recording Failed state.
//
// `handle_register_internal` is used by RuntimeBus internals that already
// hold a RouteDefinition. It installs runtime first, then persists state;
// if persistence fails, it rolls back runtime registration to avoid leaving
// an internal transient route orphaned.
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
                &format!("route failed: {error}"),
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
        deps.events.publish(&intent_events).await?;
        match upsert_projection_with_reconciliation(
            &*deps.projections,
            project_from_aggregate(&aggregate),
        )
        .await
        {
            Ok(None) => {}
            Ok(Some(primary_error)) => {
                tracing::warn!(
                    route_id = %route_id,
                    projection_error = %primary_error,
                    "projection upsert failed primary but reconciliation succeeded — read-side consistent"
                );
            }
            Err(e) => {
                // log-policy: system-broken
                tracing::error!(
                    route_id = %route_id,
                    error = %e,
                    "projection upsert + reconciliation both failed after persist"
                );
                return Err(e);
            }
        }
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
            deps.events.publish(&confirm_events).await?;
            match upsert_projection_with_reconciliation(
                &*deps.projections,
                project_from_aggregate(&aggregate),
            )
            .await
            {
                Ok(None) => {}
                Ok(Some(primary_error)) => {
                    tracing::warn!(
                        route_id = %route_id,
                        projection_error = %primary_error,
                        "projection upsert failed primary but reconciliation succeeded after confirm-start — read-side consistent"
                    );
                }
                Err(e) => {
                    // log-policy: system-broken
                    tracing::error!(
                        route_id = %route_id,
                        error = %e,
                        "projection + reconciliation both failed after confirm-start"
                    );
                    return Err(e);
                }
            }
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

/// Boot reconciliation (H8): scan all persisted route projections and fail
/// any route still in a transient state (`Starting` / `Stopping`). A route
/// persisting in a transient state after process restart means the previous
/// run crashed between Phase 1 (persisted intent) and Phase 2 (runtime
/// side-effect) of the two-phase lifecycle. The runtime that owned the
/// pending transition is gone, so the route would otherwise be stuck
/// pending manual intervention and would also be rejected by
/// `auto_startup` (Starting→Starting is invalid).
///
/// Called from `CamelContext::start()` before `auto_startup_route_ids()`.
pub(crate) async fn reconcile_transient_states(deps: &CommandDeps) -> Result<(), CamelError> {
    let statuses = deps.projections.list_statuses().await?;

    for proj in statuses {
        let is_transient = proj.status == "Starting" || proj.status == "Stopping";
        if !is_transient {
            continue;
        }

        let label = proj.status.clone();
        tracing::warn!(
            route_id = %proj.route_id,
            state = %label,
            "Boot reconciliation: route in transient state after restart — failing it"
        );

        let Some(mut aggregate) = deps.repo.load(&proj.route_id).await? else {
            tracing::warn!(
                route_id = %proj.route_id,
                "Boot reconciliation: projection has no aggregate — skipping"
            );
            continue;
        };

        let fail_events =
            aggregate.fail(format!("boot reconciliation: interrupted during {label}"));
        let fail_proj = project_from_aggregate(&aggregate);

        if let Some(uow) = &deps.uow {
            uow.persist_upsert(aggregate, None, fail_proj, &fail_events)
                .await?;
        } else {
            deps.repo.save(aggregate).await?;
            if let Err(e) =
                upsert_projection_with_reconciliation(&*deps.projections, fail_proj).await
            {
                // log-policy: system-broken
                tracing::error!(
                    route_id = %proj.route_id,
                    error = %e,
                    "boot reconciliation: projection upsert failed"
                );
            }
            if let Err(e) = deps.events.publish(&fail_events).await {
                // log-policy: system-broken
                tracing::error!(
                    route_id = %proj.route_id,
                    error = %e,
                    "boot reconciliation: event publish failed"
                );
            }
        }
    }
    Ok(())
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
                max_delay_ms: camel_api::DEFAULT_MAX_DELAY_MS,
            },
        }),
    }
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
