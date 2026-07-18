//! Per-action reload handlers extracted from `execute_reload_actions`.
//!
//! Each top-level function corresponds to one `ReloadAction` variant (Swap, Add, Remove, Restart).
//! Shared helpers used by these handlers live here too.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use camel_api::function::{FunctionDiff, PrepareToken};
use camel_api::{BoxProcessor, CamelError, StepLifecycle};

use crate::hot_reload::application::drain::drain_route;
use crate::hot_reload::application::reload::{FunctionReloadContext, ReloadError};
use crate::hot_reload::ports::ReloadExecutorPort;
use crate::lifecycle::application::route_definition::RouteDefinition;

// ============================================================================
// Shared helpers
// ============================================================================

pub(super) fn compute_function_diff_for_route(
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

pub(super) fn next_reload_command_id(op: &str, route_id: &str) -> String {
    let seq = RELOAD_COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("reload:{op}:{route_id}:{seq}")
}

pub(super) fn is_invalid_stop_transition(err: &CamelError) -> bool {
    err.to_string().contains("invalid transition")
}

pub(super) fn should_stop_before_mutation(runtime_status: Option<&str>) -> bool {
    !matches!(runtime_status, Some("Registered" | "Stopped"))
}

pub(super) fn should_start_after_restart(runtime_status: Option<&str>) -> bool {
    !matches!(runtime_status, Some("Registered" | "Stopped"))
}

// ============================================================================
// Action handlers
// ============================================================================

/// Apply a Swap action: atomic pipeline replacement (zero-downtime).
///
/// Checks the running route AND the new pipeline for lifecycle-bearing
/// steps before attempting the atomic swap.  If either carries lifecycle,
/// uses the Restart path: stop → swap_raw → start.
///
/// ## Staging contract (Case B)
///
/// Function resolution is lazy (via `FunctionInvoker` — the `FunctionStep` calls
/// `invoker.invoke(id, exchange)` at runtime).  `prepare_reload` registers new
/// function definitions in the invoker; `finalize_reload` commits them (and
/// unregisters the old ones); `rollback_reload` unregisters the newly-prepared
/// ones.  This means **rollback must NOT happen before the Restart path's
/// stop/swap/start** — the compiled pipeline `p` references FunctionIds that
/// need to be registered at runtime.
///
/// The staging lifecycle is therefore:
/// - **Success** (atomic or Restart) → `finalize_reload`
/// - **Failure** (any step after prepare) → `rollback_reload`
pub(super) async fn apply_swap(
    route_id: String,
    new_definitions: &mut Vec<RouteDefinition>,
    controller: &dyn ReloadExecutorPort,
    drain_timeout: Duration,
    function_ctx: Option<&FunctionReloadContext>,
    errors: &mut Vec<ReloadError>,
) {
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
            return;
        }
    };

    let in_flight = controller.in_flight_count(&route_id).await.unwrap_or(0);

    // Compile to a CompiledPipeline to preserve lifecycle handles for BOTH
    // code paths.  Oracle Fix 1: the stateless (function_ctx == None) path
    // also preserves lifecycle so that resequencer/aggregator handles are
    // stored in the PipelineAssembly and drained on future stops.
    #[allow(unused_mut)]
    let (p, mut lifecycle): (BoxProcessor, Vec<Arc<dyn StepLifecycle>>) =
        if let Some(ctx) = function_ctx {
            match controller
                .compile_route_definition_pipeline(def, ctx.generation)
                .await
            {
                Ok(compiled) => (compiled.processor, compiled.lifecycle),
                Err(e) => {
                    errors.push(ReloadError {
                        route_id,
                        action: "Swap (compile)".into(),
                        error: e,
                    });
                    return;
                }
            }
        } else {
            // Oracle Fix 1: use lifecycle-preserving dry compilation.
            // Even without function context, the route may carry stateful
            // lifecycle-bearing steps (resequencer, aggregator).
            match controller.compile_route_definition_dry_pipeline(def).await {
                Ok(compiled) => (compiled.processor, compiled.lifecycle),
                Err(e) => {
                    errors.push(ReloadError {
                        route_id,
                        action: "Swap (compile)".into(),
                        error: e,
                    });
                    return;
                }
            }
        };

    // Inject lifecycle from the test-only seam on the executor port,
    // simulating a lifecycle-bearing route (e.g. resequencer).  In production
    // the method does not exist on `ReloadExecutorPort` — this block is compiled
    // out (the port method itself is `#[cfg(test)]`).
    #[cfg(test)]
    {
        if let Some(injected) = controller.take_test_lifecycle_inject() {
            lifecycle = injected;
        }
    }

    // Compute function diff and prepare reload.  Both the token and the
    // diff are kept alive for the duration of this function so that any
    // branch can finalize (on success) or rollback (on failure).
    let (prepare_token, function_diff): (Option<PrepareToken>, Option<FunctionDiff>) =
        if let Some(ctx) = function_ctx {
            let diff = compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);
            match ctx
                .invoker
                .prepare_reload(diff.clone(), ctx.generation)
                .await
            {
                Ok(token) => (Some(token), Some(diff)),
                Err(e) => {
                    errors.push(ReloadError {
                        route_id: route_id.clone(),
                        action: "Swap (prepare)".into(),
                        error: CamelError::ProcessorError(format!("{e}")),
                    });
                    return;
                }
            }
        } else {
            (None, None)
        };

    // Guard: if either the running route or the new pipeline carries
    // lifecycle handles, skip the atomic swap and use the Restart path
    // (stop → swap_raw → start).  Checking both sides eliminates the
    // need for a reactive lifecycle-rejection fallback.
    let running_has_lifecycle = controller.route_has_lifecycle(&route_id).await;
    let new_has_lifecycle = !lifecycle.is_empty();

    if running_has_lifecycle || new_has_lifecycle {
        tracing::debug!(
            route_id = %route_id,
            running_has_lifecycle,
            new_has_lifecycle,
            "Lifecycle-bearing route — using Restart path (stop → swap_raw → start)"
        );

        // 1. Stop route (drains lifecycle handles)
        if let Err(stop_err) = controller.stop_route_reload(&route_id).await {
            rollback_staging(function_ctx, prepare_token.as_ref(), &route_id).await;
            errors.push(ReloadError {
                route_id,
                action: "Swap (restart-stop)".into(),
                error: stop_err,
            });
            return;
        }

        // 2. Drain in-flight exchanges
        let _ = drain_route(&route_id, "swap-restart", controller, drain_timeout).await;

        // 3. Raw pipeline swap (no lifecycle check — route is stopped)
        //    Thread lifecycle so the new PipelineAssembly carries handles.
        if let Err(swap_err) = controller
            .swap_route_pipeline_raw(&route_id, p, lifecycle)
            .await
        {
            rollback_staging(function_ctx, prepare_token.as_ref(), &route_id).await;
            errors.push(ReloadError {
                route_id,
                action: "Swap (restart-raw)".into(),
                error: swap_err,
            });
            return;
        }

        // 4. Start route (re-creates consumer with new pipeline)
        if let Err(start_err) = controller.start_route_reload(&route_id).await {
            rollback_staging(function_ctx, prepare_token.as_ref(), &route_id).await;
            errors.push(ReloadError {
                route_id,
                action: "Swap (restart-start)".into(),
                error: start_err,
            });
            return;
        }

        // Restart SUCCESS — commit staging
        if let Some(ctx) = function_ctx
            && let Some(ref diff) = function_diff
            && let Err(e) = ctx.invoker.finalize_reload(diff, ctx.generation).await
        {
            errors.push(ReloadError {
                route_id: route_id.clone(),
                action: "Swap (restart-finalize)".into(),
                error: CamelError::ProcessorError(format!("{e}")),
            });
            return;
        }

        tracing::info!(
            route_id = %route_id,
            "hot-reload: route restarted via Restart path"
        );
        return;
    }

    // Atomic swap — both sides are lifecycle-empty, will succeed.
    let result = controller.swap_route_pipeline(&route_id, p).await;
    if let Err(e) = result {
        rollback_staging(function_ctx, prepare_token.as_ref(), &route_id).await;
        errors.push(ReloadError {
            route_id,
            action: "Swap".into(),
            error: e,
        });
        return;
    }

    // Atomic swap SUCCESS — commit staging
    if let Some(ctx) = function_ctx
        && let Some(ref diff) = function_diff
        && let Err(e) = ctx.invoker.finalize_reload(diff, ctx.generation).await
    {
        errors.push(ReloadError {
            route_id: route_id.clone(),
            action: "Finalize".into(),
            error: CamelError::ProcessorError(format!("{e}")),
        });
    }
    if in_flight > 0 {
        tracing::debug!(
            route_id = %route_id,
            action = "swap",
            in_flight = in_flight,
            "hot-reload: swapped route pipeline ({} exchanges continuing with previous pipeline)",
            in_flight
        );
    } else {
        tracing::debug!(route_id = %route_id, "hot-reload: swapped route pipeline");
    }
}

/// Roll back function staging after a failed reload attempt.
///
/// Safe to call when `function_ctx` or `prepare_token` is `None` — the
/// function becomes a no-op.  Rollback failures are logged as warnings
/// instead of pushed to errors (the caller already pushes the primary error).
async fn rollback_staging(
    function_ctx: Option<&FunctionReloadContext>,
    prepare_token: Option<&PrepareToken>,
    route_id: &str,
) {
    let Some(ctx) = function_ctx else { return };
    let Some(token) = prepare_token else { return };
    if let Err(e) = ctx
        .invoker
        .rollback_reload(token.clone(), ctx.generation)
        .await
    {
        tracing::warn!(
            route_id = %route_id,
            error = %e,
            "hot-reload: failed to rollback function staging"
        );
    }
}

/// Apply an Add action: add a new route and start it.
pub(super) async fn apply_add(
    route_id: String,
    new_definitions: &mut Vec<RouteDefinition>,
    controller: &dyn ReloadExecutorPort,
    function_ctx: Option<&FunctionReloadContext>,
    errors: &mut Vec<ReloadError>,
) {
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
            return;
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
                return;
            }
        };

        let diff = compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);

        let prepare_token = match ctx.invoker.prepare_reload(diff, ctx.generation).await {
            Ok(token) => token,
            Err(e) => {
                ctx.invoker.discard_staging(ctx.generation);
                errors.push(ReloadError {
                    route_id,
                    action: "Add (prepare)".into(),
                    error: CamelError::ProcessorError(format!("{e}")),
                });
                return;
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
            return;
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
            return;
        }

        let diff = compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);
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
            return;
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

/// Apply a Remove action: stop and delete a route (with function context or with raw controller).
pub(super) async fn apply_remove(
    route_id: String,
    controller: &dyn ReloadExecutorPort,
    drain_timeout: Duration,
    function_ctx: Option<&FunctionReloadContext>,
    errors: &mut Vec<ReloadError>,
) {
    if let Some(ctx) = function_ctx {
        let diff = compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);

        let runtime_status = match controller.runtime_route_status(&route_id).await {
            Ok(status) => status,
            Err(e) => {
                errors.push(ReloadError {
                    route_id: route_id.clone(),
                    action: "Remove (status)".into(),
                    error: e,
                });
                return;
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
                return;
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
            return;
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
                return;
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
                return;
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

/// Apply a Restart action: stop, remove, re-add, and optionally restart a route.
pub(super) async fn apply_restart(
    route_id: String,
    new_definitions: &mut Vec<RouteDefinition>,
    controller: &dyn ReloadExecutorPort,
    drain_timeout: Duration,
    function_ctx: Option<&FunctionReloadContext>,
    errors: &mut Vec<ReloadError>,
) {
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
            return;
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
                return;
            }
        };

        let diff = compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);
        let prepare_token = match ctx.invoker.prepare_reload(diff, ctx.generation).await {
            Ok(token) => token,
            Err(e) => {
                errors.push(ReloadError {
                    route_id,
                    action: "Restart (prepare)".into(),
                    error: CamelError::ProcessorError(format!("{e}")),
                });
                return;
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
                return;
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
                return;
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
            return;
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
            return;
        }

        let diff = compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);
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
            tracing::debug!(
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
                return;
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
                return;
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
            return;
        }

        if let Err(e) = controller.add_route_definition(def).await {
            errors.push(ReloadError {
                route_id,
                action: "Restart (add)".into(),
                error: e,
            });
            return;
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
            tracing::debug!(
                route_id = %route_id,
                "hot-reload: restart applied while preserving stopped lifecycle state"
            );
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "reload_actions_tests.rs"]
mod tests;
