//! Route reload coordinator.
//!
//! Compares a new set of route definitions against the currently running routes
//! and computes the minimal set of actions: SWAP, RESTART, ADD, REMOVE, or SKIP.

use camel_api::CamelError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::context::RuntimeExecutionHandle;
use crate::hot_reload::application::drain::drain_route;
use crate::hot_reload::domain::ReloadAction;
#[cfg(test)]
use crate::lifecycle::adapters::route_controller::DefaultRouteController;
use crate::lifecycle::application::route_definition::RouteDefinition;

pub struct FunctionReloadContext {
    pub invoker: std::sync::Arc<dyn camel_api::function::FunctionInvoker>,
    pub generation: u64,
}

fn compute_function_diff_for_route(
    invoker: &std::sync::Arc<dyn camel_api::function::FunctionInvoker>,
    route_id: &str,
    generation: u64,
) -> camel_api::function::FunctionDiff {
    let current_refs: std::collections::HashSet<_> = invoker
        .function_refs_for_route(route_id)
        .into_iter()
        .collect();
    let staged_defs = invoker.staged_defs_for_route(route_id, generation);
    let staged_refs: std::collections::HashSet<_> = staged_defs
        .iter()
        .map(|(def, rid)| (def.id.clone(), rid.clone()))
        .collect();

    let removed: Vec<_> = current_refs.difference(&staged_refs).cloned().collect();
    let added_refs: std::collections::HashSet<_> =
        staged_refs.difference(&current_refs).cloned().collect();
    let added: Vec<_> = staged_defs
        .into_iter()
        .filter(|(def, rid)| added_refs.contains(&(def.id.clone(), rid.clone())))
        .collect();
    let unchanged: Vec<_> = current_refs
        .intersection(&staged_refs)
        .map(|(id, _)| id.clone())
        .collect();

    camel_api::function::FunctionDiff {
        added,
        removed,
        unchanged,
    }
}

static RELOAD_COMMAND_SEQ: AtomicU64 = AtomicU64::new(0);

fn next_reload_command_id(op: &str, route_id: &str) -> String {
    let seq = RELOAD_COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("reload:{op}:{route_id}:{seq}")
}

fn is_invalid_stop_transition(err: &CamelError) -> bool {
    err.to_string().contains("invalid transition")
}

fn should_stop_before_mutation(runtime_status: Option<&str>) -> bool {
    !matches!(runtime_status, Some("Registered" | "Stopped"))
}

fn should_start_after_restart(runtime_status: Option<&str>) -> bool {
    !matches!(runtime_status, Some("Registered" | "Stopped"))
}

/// A non-fatal error during reload action execution.
///
/// The watcher logs these and continues watching for future changes.
#[derive(Debug)]
pub struct ReloadError {
    pub route_id: String,
    pub action: String,
    pub error: CamelError,
}

/// Compute the diff between new definitions and active routes.
#[cfg(test)]
fn compute_reload_actions(
    new_definitions: &[RouteDefinition],
    controller: &DefaultRouteController,
) -> Vec<ReloadAction> {
    let active_ids: std::collections::HashSet<String> =
        controller.route_ids().into_iter().collect();
    let mut new_ids = std::collections::HashSet::new();
    let mut actions = Vec::new();

    for def in new_definitions {
        let route_id = def.route_id().to_string();
        new_ids.insert(route_id.clone());

        if active_ids.contains(&route_id) {
            // Route exists — check what changed
            if let Some(from_uri) = controller.route_from_uri(&route_id) {
                if from_uri != def.from_uri() {
                    actions.push(ReloadAction::Restart { route_id });
                } else {
                    let existing_hash = controller.route_source_hash(&route_id);
                    let new_hash = def.source_hash();
                    match (existing_hash, new_hash) {
                        (Some(h_existing), Some(h_new)) if h_existing == h_new => {
                            actions.push(ReloadAction::Skip { route_id });
                        }
                        _ => {
                            actions.push(ReloadAction::Swap { route_id });
                        }
                    }
                }
            }
        } else {
            actions.push(ReloadAction::Add { route_id });
        }
    }

    // Routes in active but not in new definitions → remove
    for id in &active_ids {
        if !new_ids.contains(id) {
            actions.push(ReloadAction::Remove {
                route_id: id.clone(),
            });
        }
    }

    actions
}

/// Compute reload actions using runtime projection route IDs as primary source.
///
/// This variant is used by the file watcher hard-cut path where runtime projection
/// is authoritative for route existence.
pub(crate) fn compute_reload_actions_from_runtime_snapshot(
    new_definitions: &[RouteDefinition],
    runtime_route_ids: &[String],
    runtime_source_hash: &dyn Fn(&str) -> Option<u64>,
) -> Vec<ReloadAction> {
    let active_ids: std::collections::HashSet<String> = runtime_route_ids.iter().cloned().collect();
    let mut new_ids = std::collections::HashSet::new();
    let mut actions = Vec::new();

    for def in new_definitions {
        let route_id = def.route_id().to_string();
        new_ids.insert(route_id.clone());

        if active_ids.contains(&route_id) {
            let existing_hash = runtime_source_hash(&route_id);
            let new_hash = def.source_hash();
            match (existing_hash, new_hash) {
                (Some(h_existing), Some(h_new)) if h_existing == h_new => {
                    actions.push(ReloadAction::Skip { route_id });
                }
                _ => {
                    actions.push(ReloadAction::Restart { route_id });
                }
            }
        } else {
            actions.push(ReloadAction::Add { route_id });
        }
    }

    for id in &active_ids {
        if !new_ids.contains(id) {
            actions.push(ReloadAction::Remove {
                route_id: id.clone(),
            });
        }
    }

    actions
}

/// Execute a list of reload actions against a live controller.
///
/// Non-fatal: errors for individual routes are collected and returned.
/// The caller should log them as warnings and continue watching.
///
/// `new_definitions` is consumed — each definition is moved to the controller for Add/Swap/Restart.
pub async fn execute_reload_actions(
    actions: Vec<ReloadAction>,
    mut new_definitions: Vec<RouteDefinition>,
    controller: &RuntimeExecutionHandle,
    drain_timeout: Duration,
    function_ctx: Option<&FunctionReloadContext>,
) -> Vec<ReloadError> {
    let mut errors = Vec::new();

    for action in actions {
        match action {
            ReloadAction::Swap { route_id } => {
                let def_pos = new_definitions
                    .iter()
                    .position(|d| d.route_id() == route_id);
                let def = match def_pos {
                    Some(pos) => new_definitions.remove(pos),
                    None => {
                        errors.push(ReloadError {
                            route_id: route_id.clone(),
                            action: "Swap".into(),
                            error: CamelError::RouteError(format!(
                                "No definition found for route '{}'",
                                route_id
                            )),
                        });
                        continue;
                    }
                };

                let in_flight = controller.in_flight_count(&route_id).await.unwrap_or(0);

                let pipeline = if let Some(ctx) = function_ctx {
                    controller
                        .compile_route_definition_with_generation(def, ctx.generation)
                        .await
                } else {
                    controller.compile_route_definition(def).await
                };

                match pipeline {
                    Ok(p) => {
                        let prepare_token = if let Some(ctx) = function_ctx {
                            let diff = compute_function_diff_for_route(
                                &ctx.invoker,
                                &route_id,
                                ctx.generation,
                            );
                            match ctx.invoker.prepare_reload(diff, ctx.generation).await {
                                Ok(token) => Some(token),
                                Err(e) => {
                                    errors.push(ReloadError {
                                        route_id: route_id.clone(),
                                        action: "Swap (prepare)".into(),
                                        error: CamelError::ProcessorError(format!("{e}")),
                                    });
                                    continue;
                                }
                            }
                        } else {
                            None
                        };

                        let result = controller.swap_route_pipeline(&route_id, p).await;
                        if let Err(e) = result {
                            if let Some(ctx) = function_ctx
                                && let Some(ref token) = prepare_token
                            {
                                let _ = ctx
                                    .invoker
                                    .rollback_reload(token.clone(), ctx.generation)
                                    .await;
                            }
                            errors.push(ReloadError {
                                route_id,
                                action: "Swap".into(),
                                error: e,
                            });
                        } else {
                            if let Some(ctx) = function_ctx {
                                let diff = compute_function_diff_for_route(
                                    &ctx.invoker,
                                    &route_id,
                                    ctx.generation,
                                );
                                if let Err(e) =
                                    ctx.invoker.finalize_reload(&diff, ctx.generation).await
                                {
                                    errors.push(ReloadError {
                                        route_id: route_id.clone(),
                                        action: "Finalize".into(),
                                        error: CamelError::ProcessorError(format!("{e}")),
                                    });
                                }
                            }
                            if in_flight > 0 {
                                tracing::info!(
                                    route_id = %route_id,
                                    action = "swap",
                                    in_flight = in_flight,
                                    "hot-reload: swapped route pipeline ({} exchanges continuing with previous pipeline)",
                                    in_flight
                                );
                            } else {
                                tracing::info!(route_id = %route_id, "hot-reload: swapped route pipeline");
                            }
                        }
                    }
                    Err(e) => {
                        errors.push(ReloadError {
                            route_id,
                            action: "Swap (compile)".into(),
                            error: e,
                        });
                    }
                }
            }

            ReloadAction::Add { route_id } => {
                let def_pos = new_definitions
                    .iter()
                    .position(|d| d.route_id() == route_id);
                let def = match def_pos {
                    Some(pos) => new_definitions.remove(pos),
                    None => {
                        errors.push(ReloadError {
                            route_id: route_id.clone(),
                            action: "Add".into(),
                            error: CamelError::RouteError(format!(
                                "No definition found for route '{}'",
                                route_id
                            )),
                        });
                        continue;
                    }
                };

                if let Some(ctx) = function_ctx {
                    let prepared = match controller
                        .prepare_route_definition_with_generation(def, ctx.generation)
                        .await
                    {
                        Ok(p) => p,
                        Err(e) => {
                            ctx.invoker.discard_staging(ctx.generation);
                            errors.push(ReloadError {
                                route_id,
                                action: "Add (prepare-route)".into(),
                                error: e,
                            });
                            continue;
                        }
                    };

                    let diff =
                        compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);

                    let prepare_token = match ctx.invoker.prepare_reload(diff, ctx.generation).await
                    {
                        Ok(token) => token,
                        Err(e) => {
                            ctx.invoker.discard_staging(ctx.generation);
                            errors.push(ReloadError {
                                route_id,
                                action: "Add (prepare)".into(),
                                error: CamelError::ProcessorError(format!("{e}")),
                            });
                            continue;
                        }
                    };

                    if let Err(e) = controller.insert_prepared_route(prepared).await {
                        let _ = ctx
                            .invoker
                            .rollback_reload(prepare_token, ctx.generation)
                            .await;
                        errors.push(ReloadError {
                            route_id,
                            action: "Add (insert)".into(),
                            error: e,
                        });
                        continue;
                    }

                    if let Err(e) = controller.register_route_aggregate(route_id.clone()).await {
                        let _ = controller
                            .remove_route_preserving_functions(route_id.clone())
                            .await;
                        let _ = ctx
                            .invoker
                            .rollback_reload(prepare_token, ctx.generation)
                            .await;
                        errors.push(ReloadError {
                            route_id,
                            action: "Add (aggregate)".into(),
                            error: e,
                        });
                        continue;
                    }

                    let diff =
                        compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);
                    if let Err(e) = ctx.invoker.finalize_reload(&diff, ctx.generation).await {
                        errors.push(ReloadError {
                            route_id: route_id.clone(),
                            action: "Finalize".into(),
                            error: CamelError::ProcessorError(format!("{e}")),
                        });
                    }
                } else {
                    if let Err(e) = controller.add_route_definition(def).await {
                        errors.push(ReloadError {
                            route_id,
                            action: "Add".into(),
                            error: e,
                        });
                        continue;
                    }
                }

                let start_result = controller
                    .execute_runtime_command(camel_api::RuntimeCommand::StartRoute {
                        route_id: route_id.clone(),
                        command_id: next_reload_command_id("add-start", &route_id),
                        causation_id: None,
                    })
                    .await;
                if let Err(e) = start_result {
                    errors.push(ReloadError {
                        route_id,
                        action: "Add (start)".into(),
                        error: e,
                    });
                } else {
                    tracing::info!(route_id = %route_id, "hot-reload: added and started route");
                }
            }

            ReloadAction::Remove { route_id } => {
                if let Some(ctx) = function_ctx {
                    let diff =
                        compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);

                    let runtime_status = match controller.runtime_route_status(&route_id).await {
                        Ok(status) => status,
                        Err(e) => {
                            errors.push(ReloadError {
                                route_id: route_id.clone(),
                                action: "Remove (status)".into(),
                                error: e,
                            });
                            continue;
                        }
                    };

                    if should_stop_before_mutation(runtime_status.as_deref()) {
                        let stop_result = controller
                            .execute_runtime_command(camel_api::RuntimeCommand::StopRoute {
                                route_id: route_id.clone(),
                                command_id: next_reload_command_id("remove-stop", &route_id),
                                causation_id: None,
                            })
                            .await;
                        if let Err(e) = stop_result
                            && !is_invalid_stop_transition(&e)
                        {
                            errors.push(ReloadError {
                                route_id: route_id.clone(),
                                action: "Remove (stop)".into(),
                                error: e,
                            });
                            continue;
                        }

                        let _ = drain_route(&route_id, "remove", controller, drain_timeout).await;
                    }

                    if let Err(e) = controller
                        .remove_route_preserving_functions(route_id.clone())
                        .await
                    {
                        errors.push(ReloadError {
                            route_id,
                            action: "Remove".into(),
                            error: e,
                        });
                        continue;
                    }

                    if let Err(e) = ctx.invoker.finalize_reload(&diff, ctx.generation).await {
                        errors.push(ReloadError {
                            route_id: route_id.clone(),
                            action: "Finalize".into(),
                            error: CamelError::ProcessorError(format!("{e}")),
                        });
                    }

                    tracing::info!(route_id = %route_id, "hot-reload: stopped and removed route");
                } else {
                    let runtime_status = match controller.runtime_route_status(&route_id).await {
                        Ok(status) => status,
                        Err(e) => {
                            errors.push(ReloadError {
                                route_id,
                                action: "Remove (status)".into(),
                                error: e,
                            });
                            continue;
                        }
                    };

                    if should_stop_before_mutation(runtime_status.as_deref()) {
                        let stop_result = controller
                            .execute_runtime_command(camel_api::RuntimeCommand::StopRoute {
                                route_id: route_id.clone(),
                                command_id: next_reload_command_id("remove-stop", &route_id),
                                causation_id: None,
                            })
                            .await;
                        if let Err(e) = stop_result
                            && !is_invalid_stop_transition(&e)
                        {
                            errors.push(ReloadError {
                                route_id: route_id.clone(),
                                action: "Remove (stop)".into(),
                                error: e,
                            });
                            continue;
                        }

                        let _ = drain_route(&route_id, "remove", controller, drain_timeout).await;
                    }

                    let remove_result = controller
                        .execute_runtime_command(camel_api::RuntimeCommand::RemoveRoute {
                            route_id: route_id.clone(),
                            command_id: next_reload_command_id("remove", &route_id),
                            causation_id: None,
                        })
                        .await;
                    match remove_result {
                        Ok(_) => {
                            tracing::info!(route_id = %route_id, "hot-reload: stopped and removed route");
                        }
                        Err(e) => {
                            errors.push(ReloadError {
                                route_id,
                                action: "Remove".into(),
                                error: e,
                            });
                        }
                    }
                }
            }

            ReloadAction::Restart { route_id } => {
                tracing::info!(route_id = %route_id, "hot-reload: restarting route (from_uri changed)");

                let def_pos = new_definitions
                    .iter()
                    .position(|d| d.route_id() == route_id);
                let def = match def_pos {
                    Some(pos) => new_definitions.remove(pos),
                    None => {
                        errors.push(ReloadError {
                            route_id: route_id.clone(),
                            action: "Restart".into(),
                            error: CamelError::RouteError(format!(
                                "No definition found for route '{}'",
                                route_id
                            )),
                        });
                        continue;
                    }
                };

                if let Some(ctx) = function_ctx {
                    let prepared = match controller
                        .prepare_route_definition_with_generation(def, ctx.generation)
                        .await
                    {
                        Ok(p) => p,
                        Err(e) => {
                            errors.push(ReloadError {
                                route_id,
                                action: "Restart (prepare-route)".into(),
                                error: e,
                            });
                            continue;
                        }
                    };

                    let diff =
                        compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);
                    let prepare_token = match ctx.invoker.prepare_reload(diff, ctx.generation).await
                    {
                        Ok(token) => token,
                        Err(e) => {
                            errors.push(ReloadError {
                                route_id,
                                action: "Restart (prepare)".into(),
                                error: CamelError::ProcessorError(format!("{e}")),
                            });
                            continue;
                        }
                    };

                    let runtime_status = match controller.runtime_route_status(&route_id).await {
                        Ok(status) => status,
                        Err(e) => {
                            let _ = ctx
                                .invoker
                                .rollback_reload(prepare_token, ctx.generation)
                                .await;
                            errors.push(ReloadError {
                                route_id,
                                action: "Restart (status)".into(),
                                error: e,
                            });
                            continue;
                        }
                    };

                    if should_stop_before_mutation(runtime_status.as_deref()) {
                        let stop_result = controller
                            .execute_runtime_command(camel_api::RuntimeCommand::StopRoute {
                                route_id: route_id.clone(),
                                command_id: next_reload_command_id("restart-stop", &route_id),
                                causation_id: None,
                            })
                            .await;
                        if let Err(e) = stop_result
                            && !is_invalid_stop_transition(&e)
                        {
                            let _ = ctx
                                .invoker
                                .rollback_reload(prepare_token, ctx.generation)
                                .await;
                            errors.push(ReloadError {
                                route_id,
                                action: "Restart (stop)".into(),
                                error: e,
                            });
                            continue;
                        }

                        let _ = drain_route(&route_id, "restart", controller, drain_timeout).await;
                    }

                    if let Err(e) = controller
                        .remove_route_preserving_functions(route_id.clone())
                        .await
                    {
                        let _ = ctx
                            .invoker
                            .rollback_reload(prepare_token, ctx.generation)
                            .await;
                        errors.push(ReloadError {
                            route_id,
                            action: "Restart (remove)".into(),
                            error: e,
                        });
                        continue;
                    }

                    if let Err(e) = controller.insert_prepared_route(prepared).await {
                        let _ = ctx
                            .invoker
                            .rollback_reload(prepare_token, ctx.generation)
                            .await;
                        errors.push(ReloadError {
                            route_id,
                            action: "Restart (insert)".into(),
                            error: e,
                        });
                        continue;
                    }

                    let diff =
                        compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);
                    if let Err(e) = ctx.invoker.finalize_reload(&diff, ctx.generation).await {
                        errors.push(ReloadError {
                            route_id: route_id.clone(),
                            action: "Finalize".into(),
                            error: CamelError::ProcessorError(format!("{e}")),
                        });
                    }

                    if should_start_after_restart(runtime_status.as_deref()) {
                        let start_result = controller
                            .execute_runtime_command(camel_api::RuntimeCommand::StartRoute {
                                route_id: route_id.clone(),
                                command_id: next_reload_command_id("restart-start", &route_id),
                                causation_id: None,
                            })
                            .await;
                        if let Err(e) = start_result {
                            errors.push(ReloadError {
                                route_id,
                                action: "Restart (start)".into(),
                                error: e,
                            });
                        } else {
                            tracing::info!(
                                route_id = %route_id,
                                "hot-reload: route restarted successfully"
                            );
                        }
                    } else {
                        tracing::info!(
                            route_id = %route_id,
                            "hot-reload: restart applied while preserving stopped lifecycle state"
                        );
                    }
                } else {
                    let runtime_status = match controller.runtime_route_status(&route_id).await {
                        Ok(status) => status,
                        Err(e) => {
                            errors.push(ReloadError {
                                route_id,
                                action: "Restart (status)".into(),
                                error: e,
                            });
                            continue;
                        }
                    };

                    if should_stop_before_mutation(runtime_status.as_deref()) {
                        let stop_result = controller
                            .execute_runtime_command(camel_api::RuntimeCommand::StopRoute {
                                route_id: route_id.clone(),
                                command_id: next_reload_command_id("restart-stop", &route_id),
                                causation_id: None,
                            })
                            .await;
                        if let Err(e) = stop_result
                            && !is_invalid_stop_transition(&e)
                        {
                            errors.push(ReloadError {
                                route_id,
                                action: "Restart (stop)".into(),
                                error: e,
                            });
                            continue;
                        }

                        let _ = drain_route(&route_id, "restart", controller, drain_timeout).await;
                    }

                    if let Err(e) = controller
                        .execute_runtime_command(camel_api::RuntimeCommand::RemoveRoute {
                            route_id: route_id.clone(),
                            command_id: next_reload_command_id("restart-remove", &route_id),
                            causation_id: None,
                        })
                        .await
                    {
                        errors.push(ReloadError {
                            route_id,
                            action: "Restart (remove)".into(),
                            error: e,
                        });
                        continue;
                    }

                    if let Err(e) = controller.add_route_definition(def).await {
                        errors.push(ReloadError {
                            route_id,
                            action: "Restart (add)".into(),
                            error: e,
                        });
                        continue;
                    }

                    if should_start_after_restart(runtime_status.as_deref()) {
                        let start_result = controller
                            .execute_runtime_command(camel_api::RuntimeCommand::StartRoute {
                                route_id: route_id.clone(),
                                command_id: next_reload_command_id("restart-start", &route_id),
                                causation_id: None,
                            })
                            .await;
                        if let Err(e) = start_result {
                            errors.push(ReloadError {
                                route_id,
                                action: "Restart (start)".into(),
                                error: e,
                            });
                        } else {
                            tracing::info!(
                                route_id = %route_id,
                                "hot-reload: route restarted successfully"
                            );
                        }
                    } else {
                        tracing::info!(
                            route_id = %route_id,
                            "hot-reload: restart applied while preserving stopped lifecycle state"
                        );
                    }
                }
            }

            ReloadAction::Skip { route_id } => {
                tracing::debug!(route_id = %route_id, "hot-reload: skipped unchanged route");
            }
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::adapters::route_controller::DefaultRouteController;
    use crate::shared::components::domain::Registry;
    use camel_api::function::{FunctionDiff, FunctionId};
    use camel_api::{Exchange, ExchangePatch, FunctionInvocationError, FunctionInvoker};
    use std::sync::Arc;
    use std::time::Duration;

    fn make_controller() -> DefaultRouteController {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        DefaultRouteController::new(
            registry,
            Arc::new(camel_api::NoopPlatformService::default()),
        )
    }

    #[test]
    fn helper_next_reload_command_id_increments_and_keeps_prefix() {
        let one = next_reload_command_id("restart-stop", "r1");
        let two = next_reload_command_id("restart-stop", "r1");
        assert!(one.starts_with("reload:restart-stop:r1:"));
        assert!(two.starts_with("reload:restart-stop:r1:"));
        assert_ne!(one, two);
    }

    #[test]
    fn helper_should_stop_before_mutation_respects_status() {
        assert!(!should_stop_before_mutation(Some("Registered")));
        assert!(!should_stop_before_mutation(Some("Stopped")));
        assert!(should_stop_before_mutation(Some("Started")));
        assert!(should_stop_before_mutation(None));
    }

    #[test]
    fn helper_should_start_after_restart_respects_status() {
        assert!(!should_start_after_restart(Some("Registered")));
        assert!(!should_start_after_restart(Some("Stopped")));
        assert!(should_start_after_restart(Some("Started")));
        assert!(should_start_after_restart(None));
    }

    #[test]
    fn helper_invalid_stop_transition_detects_marker() {
        let err = CamelError::RouteError("invalid transition: Started -> Started".into());
        assert!(is_invalid_stop_transition(&err));

        let other = CamelError::RouteError("route missing".into());
        assert!(!is_invalid_stop_transition(&other));
    }

    #[test]
    fn test_new_route_detected_as_add() {
        let controller = make_controller();
        let defs = vec![RouteDefinition::new("timer:tick", vec![]).with_route_id("new-route")];
        let actions = compute_reload_actions(&defs, &controller);
        assert_eq!(
            actions,
            vec![ReloadAction::Add {
                route_id: "new-route".into()
            }]
        );
    }

    #[tokio::test]
    async fn test_removed_route_detected() {
        let mut controller = make_controller();
        let def = RouteDefinition::new("timer:tick", vec![]).with_route_id("old-route");
        controller.add_route(def).await.unwrap();

        let actions = compute_reload_actions(&[], &controller);
        assert_eq!(
            actions,
            vec![ReloadAction::Remove {
                route_id: "old-route".into()
            }]
        );
    }

    #[tokio::test]
    async fn test_same_from_uri_detected_as_swap() {
        let mut controller = make_controller();
        let def = RouteDefinition::new("timer:tick", vec![])
            .with_route_id("my-route")
            .with_source_hash(100);
        controller.add_route(def).await.unwrap();

        let new_defs = vec![
            RouteDefinition::new("timer:tick", vec![])
                .with_route_id("my-route")
                .with_source_hash(200),
        ];
        let actions = compute_reload_actions(&new_defs, &controller);
        assert_eq!(
            actions,
            vec![ReloadAction::Swap {
                route_id: "my-route".into()
            }]
        );
    }

    #[tokio::test]
    async fn test_changed_from_uri_detected_as_restart() {
        let mut controller = make_controller();
        let def = RouteDefinition::new("timer:tick", vec![]).with_route_id("my-route");
        controller.add_route(def).await.unwrap();

        let new_defs =
            vec![RouteDefinition::new("timer:tock?period=500", vec![]).with_route_id("my-route")];
        let actions = compute_reload_actions(&new_defs, &controller);
        assert_eq!(
            actions,
            vec![ReloadAction::Restart {
                route_id: "my-route".into()
            }]
        );
    }

    #[tokio::test]
    async fn test_runtime_snapshot_drives_remove_set() {
        let mut controller = make_controller();
        controller
            .add_route(RouteDefinition::new("timer:tick", vec![]).with_route_id("runtime-route"))
            .await
            .unwrap();
        controller
            .add_route(RouteDefinition::new("timer:ghost", vec![]).with_route_id("ghost-route"))
            .await
            .unwrap();

        let runtime_ids = vec!["runtime-route".to_string()];
        let actions =
            compute_reload_actions_from_runtime_snapshot(&[], &runtime_ids, &|_id: &str| None);
        assert_eq!(
            actions,
            vec![ReloadAction::Remove {
                route_id: "runtime-route".into()
            }]
        );
    }

    #[test]
    fn test_runtime_snapshot_existing_routes_map_to_restart() {
        let defs = vec![
            RouteDefinition::new("timer:tick", vec![])
                .with_route_id("runtime-r1")
                .with_source_hash(10),
            RouteDefinition::new("timer:tock", vec![])
                .with_route_id("runtime-r2")
                .with_source_hash(20),
        ];
        let runtime_ids = vec!["runtime-r1".to_string(), "runtime-r2".to_string()];
        let runtime_hashes = std::collections::HashMap::from([
            ("runtime-r1".to_string(), 11u64),
            ("runtime-r2".to_string(), 22u64),
        ]);

        let actions =
            compute_reload_actions_from_runtime_snapshot(&defs, &runtime_ids, &|id: &str| {
                runtime_hashes.get(id).copied()
            });
        assert_eq!(
            actions,
            vec![
                ReloadAction::Restart {
                    route_id: "runtime-r1".into()
                },
                ReloadAction::Restart {
                    route_id: "runtime-r2".into()
                }
            ]
        );
    }

    #[tokio::test]
    async fn test_same_hash_detected_as_skip() {
        let mut controller = make_controller();
        let def = RouteDefinition::new("timer:tick", vec![])
            .with_route_id("my-route")
            .with_source_hash(42);
        controller.add_route(def).await.unwrap();

        let new_defs = vec![
            RouteDefinition::new("timer:tick", vec![])
                .with_route_id("my-route")
                .with_source_hash(42),
        ];
        let actions = compute_reload_actions(&new_defs, &controller);
        assert_eq!(
            actions,
            vec![ReloadAction::Skip {
                route_id: "my-route".into()
            }]
        );
    }

    #[tokio::test]
    async fn test_none_hash_detected_as_swap() {
        let mut controller = make_controller();
        let def = RouteDefinition::new("timer:tick", vec![]).with_route_id("my-route");
        controller.add_route(def).await.unwrap();

        let new_defs = vec![
            RouteDefinition::new("timer:tick", vec![])
                .with_route_id("my-route")
                .with_source_hash(99),
        ];
        let actions = compute_reload_actions(&new_defs, &controller);
        assert_eq!(
            actions,
            vec![ReloadAction::Swap {
                route_id: "my-route".into()
            }]
        );
    }

    #[test]
    fn test_runtime_snapshot_same_hash_detected_as_skip() {
        let defs = vec![
            RouteDefinition::new("timer:tick", vec![])
                .with_route_id("r1")
                .with_source_hash(42),
        ];
        let runtime_ids = vec!["r1".to_string()];
        let runtime_hashes = std::collections::HashMap::from([("r1".to_string(), 42u64)]);

        let actions =
            compute_reload_actions_from_runtime_snapshot(&defs, &runtime_ids, &|id: &str| {
                runtime_hashes.get(id).copied()
            });
        assert_eq!(
            actions,
            vec![ReloadAction::Skip {
                route_id: "r1".into()
            }]
        );
    }

    #[test]
    fn test_runtime_snapshot_mixed_actions_cover_all_decisions() {
        let defs = vec![
            RouteDefinition::new("timer:tick", vec![])
                .with_route_id("existing-same")
                .with_source_hash(10),
            RouteDefinition::new("timer:tock", vec![])
                .with_route_id("existing-diff")
                .with_source_hash(20),
            RouteDefinition::new("timer:new", vec![])
                .with_route_id("brand-new")
                .with_source_hash(30),
        ];
        let runtime_ids = vec![
            "existing-same".to_string(),
            "existing-diff".to_string(),
            "orphan".to_string(),
        ];
        let runtime_hashes = std::collections::HashMap::from([
            ("existing-same".to_string(), 10u64),
            ("existing-diff".to_string(), 999u64),
            ("orphan".to_string(), 77u64),
        ]);

        let actions =
            compute_reload_actions_from_runtime_snapshot(&defs, &runtime_ids, &|id: &str| {
                runtime_hashes.get(id).copied()
            });

        assert_eq!(
            actions,
            vec![
                ReloadAction::Skip {
                    route_id: "existing-same".into()
                },
                ReloadAction::Restart {
                    route_id: "existing-diff".into()
                },
                ReloadAction::Add {
                    route_id: "brand-new".into()
                },
                ReloadAction::Remove {
                    route_id: "orphan".into()
                }
            ]
        );
    }

    #[test]
    fn test_runtime_snapshot_missing_runtime_hash_for_existing_route_restarts() {
        let defs = vec![
            RouteDefinition::new("timer:tick", vec![])
                .with_route_id("r1")
                .with_source_hash(42),
        ];
        let runtime_ids = vec!["r1".to_string()];

        let actions =
            compute_reload_actions_from_runtime_snapshot(&defs, &runtime_ids, &|_id: &str| None);

        assert_eq!(
            actions,
            vec![ReloadAction::Restart {
                route_id: "r1".into()
            }]
        );
    }

    #[test]
    fn test_runtime_snapshot_missing_new_hash_for_existing_route_restarts() {
        let defs = vec![RouteDefinition::new("timer:tick", vec![]).with_route_id("r1")];
        let runtime_ids = vec!["r1".to_string()];
        let runtime_hashes = std::collections::HashMap::from([("r1".to_string(), 42u64)]);

        let actions =
            compute_reload_actions_from_runtime_snapshot(&defs, &runtime_ids, &|id: &str| {
                runtime_hashes.get(id).copied()
            });

        assert_eq!(
            actions,
            vec![ReloadAction::Restart {
                route_id: "r1".into()
            }]
        );
    }

    #[test]
    fn test_runtime_snapshot_new_only_route_maps_to_add() {
        let defs = vec![
            RouteDefinition::new("timer:tick", vec![])
                .with_route_id("new-only")
                .with_source_hash(1),
        ];
        let runtime_ids: Vec<String> = vec![];

        let actions =
            compute_reload_actions_from_runtime_snapshot(&defs, &runtime_ids, &|_id: &str| None);

        assert_eq!(
            actions,
            vec![ReloadAction::Add {
                route_id: "new-only".into()
            }]
        );
    }

    // ---- execute_reload_actions tests ----
    // These use a full CamelContext with real components so that start/stop work.

    #[tokio::test]
    async fn test_execute_add_action_inserts_route() {
        use crate::CamelContext;
        use camel_component_timer::TimerComponent;

        let mut ctx = CamelContext::builder().build().await.unwrap();
        ctx.register_component(TimerComponent::new());
        ctx.start().await.unwrap();

        let def = RouteDefinition::new("timer:tick?period=50&repeatCount=1", vec![])
            .with_route_id("exec-add-test");
        let actions = vec![ReloadAction::Add {
            route_id: "exec-add-test".into(),
        }];
        let errors = execute_reload_actions(
            actions,
            vec![def],
            &ctx.runtime_execution_handle(),
            Duration::from_secs(10),
            None,
        )
        .await;
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        assert_eq!(
            ctx.runtime_execution_handle()
                .controller_route_count_for_test()
                .await,
            1
        );

        ctx.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_execute_remove_action_deletes_route() {
        use crate::CamelContext;
        use camel_component_timer::TimerComponent;

        let mut ctx = CamelContext::builder().build().await.unwrap();
        ctx.register_component(TimerComponent::new());
        ctx.start().await.unwrap();

        // Add route through context so runtime aggregate/projection are seeded.
        let def =
            RouteDefinition::new("timer:tick?period=100", vec![]).with_route_id("exec-remove-test");
        ctx.add_route_definition(def).await.unwrap();
        assert_eq!(
            ctx.runtime_execution_handle()
                .controller_route_count_for_test()
                .await,
            1
        );

        let actions = vec![ReloadAction::Remove {
            route_id: "exec-remove-test".into(),
        }];
        let errors = execute_reload_actions(
            actions,
            vec![],
            &ctx.runtime_execution_handle(),
            Duration::from_secs(10),
            None,
        )
        .await;
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        assert_eq!(
            ctx.runtime_execution_handle()
                .controller_route_count_for_test()
                .await,
            0
        );

        ctx.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_execute_swap_action_replaces_pipeline() {
        use crate::CamelContext;
        use camel_component_timer::TimerComponent;

        let mut ctx = CamelContext::builder().build().await.unwrap();
        ctx.register_component(TimerComponent::new());
        ctx.start().await.unwrap();

        // Add route through context so runtime aggregate/projection are seeded.
        let def =
            RouteDefinition::new("timer:tick?period=100", vec![]).with_route_id("exec-swap-test");
        ctx.add_route_definition(def).await.unwrap();

        // Swap with same from_uri (exercises compile + swap_pipeline code path)
        let new_def =
            RouteDefinition::new("timer:tick?period=100", vec![]).with_route_id("exec-swap-test");
        let actions = vec![ReloadAction::Swap {
            route_id: "exec-swap-test".into(),
        }];
        let errors = execute_reload_actions(
            actions,
            vec![new_def],
            &ctx.runtime_execution_handle(),
            Duration::from_secs(10),
            None,
        )
        .await;
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        // Route should still exist after swap
        assert_eq!(
            ctx.runtime_execution_handle()
                .controller_route_count_for_test()
                .await,
            1
        );

        ctx.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_execute_restart_action_preserves_registered_lifecycle_state() {
        use crate::CamelContext;
        use camel_api::{RuntimeQuery, RuntimeQueryResult};
        use camel_component_timer::TimerComponent;

        let mut ctx = CamelContext::builder().build().await.unwrap();
        ctx.register_component(TimerComponent::new());
        ctx.start().await.unwrap();

        // Add route through context so runtime aggregate/projection are seeded.
        let initial = RouteDefinition::new("timer:tick?period=100", vec![])
            .with_route_id("exec-restart-test");
        ctx.add_route_definition(initial).await.unwrap();

        // Route is seeded as Registered by context registration.
        let before = ctx
            .runtime()
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: "exec-restart-test".into(),
            })
            .await
            .unwrap();
        match before {
            RuntimeQueryResult::RouteStatus { status, .. } => assert_eq!(status, "Registered"),
            other => panic!("unexpected query result: {other:?}"),
        }

        let replacement = RouteDefinition::new("timer:tick?period=250", vec![])
            .with_route_id("exec-restart-test");
        let actions = vec![ReloadAction::Restart {
            route_id: "exec-restart-test".into(),
        }];
        let errors = execute_reload_actions(
            actions,
            vec![replacement],
            &ctx.runtime_execution_handle(),
            Duration::from_secs(10),
            None,
        )
        .await;
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        // Restart re-adds through RuntimeExecutionHandle::add_route_definition,
        // which now goes through InternalRuntimeCommandBus and preserves Registered state.
        let after = ctx
            .runtime()
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: "exec-restart-test".into(),
            })
            .await
            .unwrap();
        match after {
            RuntimeQueryResult::RouteStatus { status, .. } => assert_eq!(status, "Registered"),
            other => panic!("unexpected query result: {other:?}"),
        }

        assert_eq!(
            ctx.runtime_route_status("exec-restart-test").await.unwrap(),
            Some("Registered".to_string())
        );

        ctx.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_execute_swap_action_missing_definition_returns_error() {
        use crate::CamelContext;

        let ctx = CamelContext::builder().build().await.unwrap();
        let errors = execute_reload_actions(
            vec![ReloadAction::Swap {
                route_id: "missing-swap-def".into(),
            }],
            vec![],
            &ctx.runtime_execution_handle(),
            Duration::from_millis(1),
            None,
        )
        .await;

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].action, "Swap");
        assert_eq!(errors[0].route_id, "missing-swap-def");
    }

    #[tokio::test]
    async fn test_execute_add_action_missing_definition_returns_error() {
        use crate::CamelContext;

        let ctx = CamelContext::builder().build().await.unwrap();
        let errors = execute_reload_actions(
            vec![ReloadAction::Add {
                route_id: "missing-add-def".into(),
            }],
            vec![],
            &ctx.runtime_execution_handle(),
            Duration::from_millis(1),
            None,
        )
        .await;

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].action, "Add");
        assert_eq!(errors[0].route_id, "missing-add-def");
    }

    #[tokio::test]
    async fn test_execute_remove_action_status_error_returns_error() {
        use crate::CamelContext;

        let ctx = CamelContext::builder().build().await.unwrap();
        let errors = execute_reload_actions(
            vec![ReloadAction::Remove {
                route_id: "missing-remove-route".into(),
            }],
            vec![],
            &ctx.runtime_execution_handle(),
            Duration::from_millis(1),
            None,
        )
        .await;

        assert_eq!(errors.len(), 1);
        assert!(errors[0].action.starts_with("Remove"));
        assert_eq!(errors[0].route_id, "missing-remove-route");
    }

    #[tokio::test]
    async fn test_execute_restart_action_missing_definition_returns_error() {
        use crate::CamelContext;

        let ctx = CamelContext::builder().build().await.unwrap();
        let errors = execute_reload_actions(
            vec![ReloadAction::Restart {
                route_id: "missing-restart-def".into(),
            }],
            vec![],
            &ctx.runtime_execution_handle(),
            Duration::from_millis(1),
            None,
        )
        .await;

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].action, "Restart");
        assert_eq!(errors[0].route_id, "missing-restart-def");
    }

    #[tokio::test]
    async fn test_execute_skip_action_returns_no_errors() {
        use crate::CamelContext;

        let ctx = CamelContext::builder().build().await.unwrap();
        let errors = execute_reload_actions(
            vec![ReloadAction::Skip {
                route_id: "skip-only-route".into(),
            }],
            vec![],
            &ctx.runtime_execution_handle(),
            Duration::from_millis(1),
            None,
        )
        .await;

        assert!(errors.is_empty());
    }

    #[test]
    fn compute_function_diff_all_added() {
        use camel_api::function::PrepareToken;
        use camel_api::{FunctionDefinition, FunctionInvokerSync};
        use std::sync::Mutex;

        struct TestInvoker {
            staged: Mutex<Vec<(FunctionDefinition, Option<String>)>>,
        }
        impl FunctionInvokerSync for TestInvoker {
            fn stage_pending(&self, _def: FunctionDefinition, _route_id: Option<&str>, _gen: u64) {}
            fn discard_staging(&self, _generation: u64) {}
            fn begin_reload(&self) -> u64 {
                0
            }
            fn function_refs_for_route(
                &self,
                _route_id: &str,
            ) -> Vec<(FunctionId, Option<String>)> {
                vec![]
            }
            fn staged_refs_for_route(
                &self,
                _route_id: &str,
                _generation: u64,
            ) -> Vec<(FunctionId, Option<String>)> {
                vec![]
            }
            fn staged_defs_for_route(
                &self,
                _route_id: &str,
                _generation: u64,
            ) -> Vec<(FunctionDefinition, Option<String>)> {
                self.staged.lock().unwrap().clone()
            }
        }
        #[async_trait::async_trait]
        impl FunctionInvoker for TestInvoker {
            async fn register(
                &self,
                _def: FunctionDefinition,
                _route_id: Option<&str>,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn unregister(
                &self,
                _id: &FunctionId,
                _route_id: Option<&str>,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn invoke(
                &self,
                _id: &FunctionId,
                _exchange: &Exchange,
            ) -> Result<ExchangePatch, FunctionInvocationError> {
                Ok(ExchangePatch::default())
            }
            async fn prepare_reload(
                &self,
                _diff: FunctionDiff,
                _generation: u64,
            ) -> Result<PrepareToken, FunctionInvocationError> {
                Ok(PrepareToken::default())
            }
            async fn finalize_reload(
                &self,
                _diff: &FunctionDiff,
                _generation: u64,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn rollback_reload(
                &self,
                _token: PrepareToken,
                _generation: u64,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn commit_reload(
                &self,
                _diff: FunctionDiff,
                _generation: u64,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn commit_staged(&self) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
        }

        let staged_def = FunctionDefinition {
            id: FunctionId::compute("deno", "fn1", 5000),
            runtime: "deno".into(),
            source: "fn1".into(),
            timeout_ms: 5000,
            route_id: Some("route-a".into()),
            step_index: Some(0),
        };
        let invoker: Arc<dyn FunctionInvoker> = Arc::new(TestInvoker {
            staged: Mutex::new(vec![(staged_def.clone(), Some("route-a".into()))]),
        });

        let diff = compute_function_diff_for_route(&invoker, "route-a", 0);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.removed.len(), 0);
        assert_eq!(diff.unchanged.len(), 0);
    }

    #[test]
    fn compute_function_diff_all_removed() {
        use camel_api::function::PrepareToken;
        use camel_api::{FunctionDefinition, FunctionInvokerSync};

        struct TestInvoker;
        impl FunctionInvokerSync for TestInvoker {
            fn stage_pending(&self, _def: FunctionDefinition, _route_id: Option<&str>, _gen: u64) {}
            fn discard_staging(&self, _generation: u64) {}
            fn begin_reload(&self) -> u64 {
                0
            }
            fn function_refs_for_route(
                &self,
                _route_id: &str,
            ) -> Vec<(FunctionId, Option<String>)> {
                vec![(
                    FunctionId::compute("deno", "old-fn", 5000),
                    Some("route-b".into()),
                )]
            }
            fn staged_refs_for_route(
                &self,
                _route_id: &str,
                _generation: u64,
            ) -> Vec<(FunctionId, Option<String>)> {
                vec![]
            }
            fn staged_defs_for_route(
                &self,
                _route_id: &str,
                _generation: u64,
            ) -> Vec<(FunctionDefinition, Option<String>)> {
                vec![]
            }
        }
        #[async_trait::async_trait]
        impl FunctionInvoker for TestInvoker {
            async fn register(
                &self,
                _def: FunctionDefinition,
                _route_id: Option<&str>,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn unregister(
                &self,
                _id: &FunctionId,
                _route_id: Option<&str>,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn invoke(
                &self,
                _id: &FunctionId,
                _exchange: &Exchange,
            ) -> Result<ExchangePatch, FunctionInvocationError> {
                Ok(ExchangePatch::default())
            }
            async fn prepare_reload(
                &self,
                _diff: FunctionDiff,
                _generation: u64,
            ) -> Result<PrepareToken, FunctionInvocationError> {
                Ok(PrepareToken::default())
            }
            async fn finalize_reload(
                &self,
                _diff: &FunctionDiff,
                _generation: u64,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn rollback_reload(
                &self,
                _token: PrepareToken,
                _generation: u64,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn commit_reload(
                &self,
                _diff: FunctionDiff,
                _generation: u64,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn commit_staged(&self) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
        }

        let invoker: Arc<dyn FunctionInvoker> = Arc::new(TestInvoker);
        let diff = compute_function_diff_for_route(&invoker, "route-b", 0);
        assert_eq!(diff.added.len(), 0);
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.unchanged.len(), 0);
    }

    #[test]
    fn compute_function_diff_unchanged() {
        use camel_api::function::PrepareToken;
        use camel_api::{FunctionDefinition, FunctionInvokerSync};

        let fn_id = FunctionId::compute("deno", "same-fn", 5000);
        let pair = (fn_id.clone(), Some("route-c".into()));

        struct TestInvoker {
            pair: (FunctionId, Option<String>),
        }
        impl FunctionInvokerSync for TestInvoker {
            fn stage_pending(&self, _def: FunctionDefinition, _route_id: Option<&str>, _gen: u64) {}
            fn discard_staging(&self, _generation: u64) {}
            fn begin_reload(&self) -> u64 {
                0
            }
            fn function_refs_for_route(
                &self,
                _route_id: &str,
            ) -> Vec<(FunctionId, Option<String>)> {
                vec![self.pair.clone()]
            }
            fn staged_refs_for_route(
                &self,
                _route_id: &str,
                _generation: u64,
            ) -> Vec<(FunctionId, Option<String>)> {
                vec![self.pair.clone()]
            }
            fn staged_defs_for_route(
                &self,
                _route_id: &str,
                _generation: u64,
            ) -> Vec<(FunctionDefinition, Option<String>)> {
                vec![(
                    FunctionDefinition {
                        id: self.pair.0.clone(),
                        runtime: "deno".into(),
                        source: "same".into(),
                        timeout_ms: 5000,
                        route_id: Some("route-c".into()),
                        step_index: Some(0),
                    },
                    self.pair.1.clone(),
                )]
            }
        }
        #[async_trait::async_trait]
        impl FunctionInvoker for TestInvoker {
            async fn register(
                &self,
                _def: FunctionDefinition,
                _route_id: Option<&str>,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn unregister(
                &self,
                _id: &FunctionId,
                _route_id: Option<&str>,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn invoke(
                &self,
                _id: &FunctionId,
                _exchange: &Exchange,
            ) -> Result<ExchangePatch, FunctionInvocationError> {
                Ok(ExchangePatch::default())
            }
            async fn prepare_reload(
                &self,
                _diff: FunctionDiff,
                _generation: u64,
            ) -> Result<PrepareToken, FunctionInvocationError> {
                Ok(PrepareToken::default())
            }
            async fn finalize_reload(
                &self,
                _diff: &FunctionDiff,
                _generation: u64,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn rollback_reload(
                &self,
                _token: PrepareToken,
                _generation: u64,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn commit_reload(
                &self,
                _diff: FunctionDiff,
                _generation: u64,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn commit_staged(&self) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
        }

        let invoker: Arc<dyn FunctionInvoker> = Arc::new(TestInvoker { pair });
        let diff = compute_function_diff_for_route(&invoker, "route-c", 0);
        assert_eq!(diff.added.len(), 0);
        assert_eq!(diff.removed.len(), 0);
        assert_eq!(diff.unchanged.len(), 1);
    }

    #[test]
    fn reload_error_debug_format() {
        let err = ReloadError {
            route_id: "r1".into(),
            action: "Swap".into(),
            error: CamelError::RouteError("test error".into()),
        };
        let debug = format!("{:?}", err);
        assert!(debug.contains("r1"));
        assert!(debug.contains("Swap"));
    }
}
