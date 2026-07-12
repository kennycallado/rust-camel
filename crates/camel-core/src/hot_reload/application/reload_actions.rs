//! Per-action reload handlers extracted from `execute_reload_actions`.
//!
//! Each top-level function corresponds to one `ReloadAction` variant (Swap, Add, Remove, Restart).
//! Shared helpers used by these handlers live here too.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use camel_api::function::{FunctionDiff, PrepareToken};
use camel_api::{BoxProcessor, CamelError, StepLifecycle};

use crate::context::RuntimeExecutionHandle;
use crate::hot_reload::application::drain::drain_route;
use crate::hot_reload::application::reload::{FunctionReloadContext, ReloadError};
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
    controller: &RuntimeExecutionHandle,
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

    // Inject lifecycle from the test-only field on RuntimeExecutionHandle,
    // simulating a lifecycle-bearing route (e.g. resequencer).  In production
    // the field does not exist — this block is compiled out.
    #[cfg(test)]
    {
        if let Some(injected) = controller.test_lifecycle_inject.lock().unwrap().take() {
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
    controller: &RuntimeExecutionHandle,
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
    controller: &RuntimeExecutionHandle,
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
    controller: &RuntimeExecutionHandle,
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
mod tests {
    use super::*;
    use camel_api::function::{FunctionDiff, FunctionId, PrepareToken};
    use camel_api::{
        Exchange, ExchangePatch, FunctionDefinition, FunctionInvocationError, FunctionInvoker,
        FunctionInvokerSync,
    };
    use std::sync::Arc;

    // -----------------------------------------------------------------------
    // Reusable mock invoker
    // -----------------------------------------------------------------------

    struct MockInvoker {
        function_refs: Vec<(FunctionId, Option<String>)>,
        staged_refs: Vec<(FunctionId, Option<String>)>,
        staged_defs: Vec<(FunctionDefinition, Option<String>)>,
    }

    impl FunctionInvokerSync for MockInvoker {
        fn stage_pending(&self, _: FunctionDefinition, _: Option<&str>, _: u64) {}
        fn discard_staging(&self, _: u64) {}
        fn begin_reload(&self) -> u64 {
            0
        }
        fn function_refs_for_route(&self, _: &str) -> Vec<(FunctionId, Option<String>)> {
            self.function_refs.clone()
        }
        fn staged_refs_for_route(&self, _: &str, _: u64) -> Vec<(FunctionId, Option<String>)> {
            self.staged_refs.clone()
        }
        fn staged_defs_for_route(
            &self,
            _: &str,
            _: u64,
        ) -> Vec<(FunctionDefinition, Option<String>)> {
            self.staged_defs.clone()
        }
    }

    #[async_trait::async_trait]
    impl FunctionInvoker for MockInvoker {
        async fn register(
            &self,
            _: FunctionDefinition,
            _: Option<&str>,
        ) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
        async fn unregister(
            &self,
            _: &FunctionId,
            _: Option<&str>,
        ) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
        async fn invoke(
            &self,
            _: &FunctionId,
            _: &Exchange,
        ) -> Result<ExchangePatch, FunctionInvocationError> {
            Ok(ExchangePatch::default())
        }
        async fn prepare_reload(
            &self,
            _: FunctionDiff,
            _: u64,
        ) -> Result<PrepareToken, FunctionInvocationError> {
            Ok(PrepareToken::default())
        }
        async fn finalize_reload(
            &self,
            _: &FunctionDiff,
            _: u64,
        ) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
        async fn rollback_reload(
            &self,
            _: PrepareToken,
            _: u64,
        ) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
        async fn commit_reload(
            &self,
            _: FunctionDiff,
            _: u64,
        ) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
        async fn commit_staged(&self) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helper tests
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // compute_function_diff_for_route tests
    // -----------------------------------------------------------------------

    #[test]
    fn compute_function_diff_all_added() {
        let staged_def = FunctionDefinition {
            id: FunctionId::compute("deno", "fn1", 5000),
            runtime: "deno".into(),
            source: "fn1".into(),
            timeout_ms: 5000,
            route_id: Some("route-a".into()),
            step_index: Some(0),
        };
        let invoker: Arc<dyn FunctionInvoker> = Arc::new(MockInvoker {
            function_refs: vec![],
            staged_refs: vec![(staged_def.id.clone(), Some("route-a".into()))],
            staged_defs: vec![(staged_def.clone(), Some("route-a".into()))],
        });

        let diff = compute_function_diff_for_route(&invoker, "route-a", 0);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.removed.len(), 0);
        assert_eq!(diff.unchanged.len(), 0);
    }

    #[test]
    fn compute_function_diff_all_removed() {
        let invoker: Arc<dyn FunctionInvoker> = Arc::new(MockInvoker {
            function_refs: vec![(
                FunctionId::compute("deno", "old-fn", 5000),
                Some("route-b".into()),
            )],
            staged_refs: vec![],
            staged_defs: vec![],
        });

        let diff = compute_function_diff_for_route(&invoker, "route-b", 0);
        assert_eq!(diff.added.len(), 0);
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.unchanged.len(), 0);
    }

    #[test]
    fn compute_function_diff_unchanged() {
        let fn_id = FunctionId::compute("deno", "same-fn", 5000);
        let pair = (fn_id.clone(), Some("route-c".into()));

        let invoker: Arc<dyn FunctionInvoker> = Arc::new(MockInvoker {
            function_refs: vec![pair.clone()],
            staged_refs: vec![pair.clone()],
            staged_defs: vec![(
                FunctionDefinition {
                    id: fn_id,
                    runtime: "deno".into(),
                    source: "same".into(),
                    timeout_ms: 5000,
                    route_id: Some("route-c".into()),
                    step_index: Some(0),
                },
                Some("route-c".into()),
            )],
        });

        let diff = compute_function_diff_for_route(&invoker, "route-c", 0);
        assert_eq!(diff.added.len(), 0);
        assert_eq!(diff.removed.len(), 0);
        assert_eq!(diff.unchanged.len(), 1);
    }

    // ── Integration: apply_swap Restart path for lifecycle-bearing routes ──

    #[tokio::test]
    async fn apply_swap_restart_path_for_lifecycle_route() {
        // Verifies the full stop→swap_raw→start sequence:
        //   1. swap_pipeline rejects the lifecycle-bearing route
        //   2. The Restart path stops the route, drains, raw-swaps, and starts
        //   3. No errors are produced
        //   4. The route exists with the new pipeline after restart
        //
        // Because function resolution is lazy (Case B — FunctionStep calls
        // invoker.invoke(id) at runtime), the staging contract is:
        //   - finalize_reload on success (NOT premature rollback)
        //   - rollback_reload on failure

        use crate::context::RuntimeExecutionHandle;
        use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
        use crate::lifecycle::adapters::in_memory::InMemoryRuntimeStore;
        use crate::lifecycle::adapters::route_controller::DefaultRouteController;
        use crate::lifecycle::adapters::runtime_execution::RuntimeExecutionAdapter;
        use crate::lifecycle::application::runtime_bus::RuntimeBus;
        use crate::shared::components::domain::Registry;
        use camel_api::{StepLifecycle, StepShutdownReason};
        use std::sync::Arc;

        // ── Lifecycle handle for the test route ──
        #[derive(Debug)]
        struct TestLifecycle;
        #[async_trait::async_trait]
        impl StepLifecycle for TestLifecycle {
            fn name(&self) -> &'static str {
                "test-lifecycle"
            }
            async fn shutdown(
                &self,
                _reason: StepShutdownReason,
            ) -> Result<(), camel_api::CamelError> {
                Ok(())
            }
        }

        // ── Build controller with a route injected with lifecycle ──
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        registry
            .lock()
            .unwrap()
            .register(Arc::new(camel_component_timer::TimerComponent::new()));
        let mut controller = DefaultRouteController::new(
            registry,
            Arc::new(camel_api::NoopPlatformService::default()),
        );

        // Add route via the normal path (compiles identity pipeline).
        controller
            .add_route(
                RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                    .with_route_id("restart-integration"),
            )
            .await
            .unwrap();

        // Inject lifecycle into the pipeline so swap_pipeline rejects it.
        controller
            .set_route_lifecycle_for_test(
                "restart-integration",
                vec![Arc::new(TestLifecycle) as Arc<dyn StepLifecycle>],
            )
            .expect("set_route_lifecycle_for_test should succeed");

        // Spawn the actor — the route is pre-injected with lifecycle.
        let (ctrl_handle, _actor_join) = spawn_controller_actor(controller);

        // ── Build minimal RuntimeBus ──
        let store = InMemoryRuntimeStore::default();
        let execution = Arc::new(RuntimeExecutionAdapter::new(ctrl_handle.clone()));
        let runtime_bus = Arc::new(
            RuntimeBus::new(
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
            )
            .with_execution(execution),
        );

        // ── Build RuntimeExecutionHandle ──
        let handle = RuntimeExecutionHandle {
            controller: ctrl_handle.clone(),
            runtime: runtime_bus,
            function_invoker: None,
            test_lifecycle_inject: Arc::new(std::sync::Mutex::new(None)),
        };

        // ── Call apply_swap ──
        let mut new_defs = vec![
            RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                .with_route_id("restart-integration"),
        ];
        let mut errors = vec![];
        apply_swap(
            "restart-integration".into(),
            &mut new_defs,
            &handle,
            Duration::from_secs(5),
            None,
            &mut errors,
        )
        .await;

        // ── Assert ──
        assert!(
            errors.is_empty(),
            "apply_swap should succeed for lifecycle route via Restart path; got {errors:?}"
        );

        let exists = ctrl_handle
            .route_exists("restart-integration")
            .await
            .unwrap();
        assert!(exists, "route should exist after Restart path completes");

        let pipeline = ctrl_handle
            .get_pipeline("restart-integration")
            .await
            .unwrap();
        assert!(
            pipeline.is_some(),
            "pipeline should be present after Restart path completes"
        );

        // Cleanup: stop the running route (timer consumer was started).
        let _ = ctrl_handle.stop_route("restart-integration").await;
    }

    // ── Proactive guard: lifecycle-bearing compiled pipeline → Restart path ──

    /// Verify that when the compiled pipeline carries lifecycle handles,
    /// the proactive guard skips the atomic swap and uses the Restart path
    /// directly.  The Restart path threads lifecycle into
    /// `swap_pipeline_raw`, so a subsequent atomic swap attempt is rejected.
    #[tokio::test]
    async fn apply_swap_proactive_restart_preserves_lifecycle() {
        use crate::context::RuntimeExecutionHandle;
        use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
        use crate::lifecycle::adapters::in_memory::InMemoryRuntimeStore;
        use crate::lifecycle::adapters::route_controller::DefaultRouteController;
        use crate::lifecycle::adapters::runtime_execution::RuntimeExecutionAdapter;
        use crate::lifecycle::application::runtime_bus::RuntimeBus;
        use crate::shared::components::domain::Registry;
        use camel_api::{IdentityProcessor, StepLifecycle, StepShutdownReason};
        use std::sync::Arc;

        // ── Lifecycle handle for the compiled pipeline ──
        #[derive(Debug)]
        struct GuardTestLifecycle;
        #[async_trait::async_trait]
        impl StepLifecycle for GuardTestLifecycle {
            fn name(&self) -> &'static str {
                "guard-test-lifecycle"
            }
            async fn shutdown(
                &self,
                _reason: StepShutdownReason,
            ) -> Result<(), camel_api::CamelError> {
                Ok(())
            }
        }

        // ── Build controller with a plain (lifecycle-free) route ──
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        registry
            .lock()
            .unwrap()
            .register(Arc::new(camel_component_timer::TimerComponent::new()));
        let mut controller = DefaultRouteController::new(
            registry,
            Arc::new(camel_api::NoopPlatformService::default()),
        );

        controller
            .add_route(
                RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                    .with_route_id("proactive-restart"),
            )
            .await
            .unwrap();

        // NOTE: do NOT call set_route_lifecycle_for_test — the OLD route
        // has no lifecycle, so an atomic swap would normally succeed.
        // Instead, inject lifecycle into the NEW compiled pipeline via the
        // test-only field on RuntimeExecutionHandle.

        let (ctrl_handle, _actor_join) = spawn_controller_actor(controller);

        // ── Build minimal RuntimeBus ──
        let store = InMemoryRuntimeStore::default();
        let execution = Arc::new(RuntimeExecutionAdapter::new(ctrl_handle.clone()));
        let runtime_bus = Arc::new(
            RuntimeBus::new(
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
            )
            .with_execution(execution),
        );

        let handle = RuntimeExecutionHandle {
            controller: ctrl_handle.clone(),
            runtime: runtime_bus,
            function_invoker: None,
            test_lifecycle_inject: Arc::new(std::sync::Mutex::new(Some(vec![Arc::new(
                GuardTestLifecycle,
            )
                as Arc<dyn StepLifecycle>]))),
        };

        // ── Call apply_swap (lifecycle injected → should go Restart path) ──
        let mut new_defs = vec![
            RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                .with_route_id("proactive-restart"),
        ];
        let mut errors = vec![];
        apply_swap(
            "proactive-restart".into(),
            &mut new_defs,
            &handle,
            Duration::from_secs(5),
            None,
            &mut errors,
        )
        .await;

        assert!(
            errors.is_empty(),
            "apply_swap should succeed via proactive Restart path; got {errors:?}"
        );

        let exists = ctrl_handle.route_exists("proactive-restart").await.unwrap();
        assert!(
            exists,
            "route should exist after proactive Restart path completes"
        );

        let pipeline = ctrl_handle.get_pipeline("proactive-restart").await.unwrap();
        assert!(
            pipeline.is_some(),
            "pipeline should be present after proactive Restart path completes"
        );

        // ── Verify lifecycle was preserved ──
        // If the atomic swap had been used, the pipeline assembly would have
        // lifecycle == vec![].  Since we injected lifecycle, the Restart path
        // should have threaded it into swap_pipeline_raw, making the route
        // lifecycle-bearing.  A subsequent atomic swap MUST be rejected.
        let identity = camel_api::BoxProcessor::new(IdentityProcessor);
        let swap_result = ctrl_handle
            .swap_pipeline("proactive-restart", identity)
            .await;
        assert!(
            swap_result.is_err(),
            "atomic swap should be rejected after Restart path preserves lifecycle"
        );
        let err_msg = swap_result.unwrap_err().to_string();
        assert!(
            err_msg.contains("lifecycle-bearing"),
            "rejection should mention lifecycle-bearing; got: {err_msg}"
        );

        // Cleanup
        let _ = ctrl_handle.stop_route("proactive-restart").await;
    }

    /// Verify that a route WITHOUT lifecycle-bearing steps uses the atomic
    /// swap (fast path) and a subsequent atomic swap still succeeds.
    #[tokio::test]
    async fn apply_swap_atomic_for_non_lifecycle_route() {
        use crate::context::RuntimeExecutionHandle;
        use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
        use crate::lifecycle::adapters::in_memory::InMemoryRuntimeStore;
        use crate::lifecycle::adapters::route_controller::DefaultRouteController;
        use crate::lifecycle::adapters::runtime_execution::RuntimeExecutionAdapter;
        use crate::lifecycle::application::runtime_bus::RuntimeBus;
        use crate::shared::components::domain::Registry;
        use camel_api::IdentityProcessor;
        use std::sync::Arc;

        // ── Build controller with a plain route ──
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        registry
            .lock()
            .unwrap()
            .register(Arc::new(camel_component_timer::TimerComponent::new()));
        let mut controller = DefaultRouteController::new(
            registry,
            Arc::new(camel_api::NoopPlatformService::default()),
        );

        controller
            .add_route(
                RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                    .with_route_id("atomic-swap"),
            )
            .await
            .unwrap();

        let (ctrl_handle, _actor_join) = spawn_controller_actor(controller);

        let store = InMemoryRuntimeStore::default();
        let execution = Arc::new(RuntimeExecutionAdapter::new(ctrl_handle.clone()));
        let runtime_bus = Arc::new(
            RuntimeBus::new(
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
            )
            .with_execution(execution),
        );

        let handle = RuntimeExecutionHandle {
            controller: ctrl_handle.clone(),
            runtime: runtime_bus,
            function_invoker: None,
            test_lifecycle_inject: Arc::new(std::sync::Mutex::new(None)),
        };

        // ── First swap: should use atomic path (no lifecycle) ──
        let mut new_defs = vec![
            RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                .with_route_id("atomic-swap"),
        ];
        let mut errors = vec![];
        apply_swap(
            "atomic-swap".into(),
            &mut new_defs,
            &handle,
            Duration::from_secs(5),
            None,
            &mut errors,
        )
        .await;

        assert!(
            errors.is_empty(),
            "apply_swap should succeed for non-lifecycle route via atomic swap; got {errors:?}"
        );

        let exists = ctrl_handle.route_exists("atomic-swap").await.unwrap();
        assert!(exists, "route should exist after atomic swap");

        let pipeline = ctrl_handle.get_pipeline("atomic-swap").await.unwrap();
        assert!(
            pipeline.is_some(),
            "pipeline should be present after atomic swap"
        );

        // ── Verify atomic swap was used: subsequent atomic swap succeeds ──
        let identity = camel_api::BoxProcessor::new(IdentityProcessor);
        let swap_result = ctrl_handle.swap_pipeline("atomic-swap", identity).await;
        assert!(
            swap_result.is_ok(),
            "subsequent atomic swap should succeed (route has no lifecycle)"
        );

        // Cleanup
        let _ = ctrl_handle.stop_route("atomic-swap").await;
    }

    /// Verify that lifecycle handles are concretely preserved in the
    /// PipelineAssembly after the proactive Restart path completes.
    /// Uses the direct `DefaultRouteController` (before actor spawn) to
    /// inspect the pipeline assembly's lifecycle field.
    #[tokio::test]
    async fn apply_swap_lifecycle_handles_preserved_in_assembly() {
        use crate::context::RuntimeExecutionHandle;
        use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
        use crate::lifecycle::adapters::in_memory::InMemoryRuntimeStore;
        use crate::lifecycle::adapters::route_controller::DefaultRouteController;
        use crate::lifecycle::adapters::runtime_execution::RuntimeExecutionAdapter;
        use crate::lifecycle::application::runtime_bus::RuntimeBus;
        use crate::shared::components::domain::Registry;
        use camel_api::{IdentityProcessor, StepLifecycle, StepShutdownReason};
        use std::sync::Arc;

        // ── Lifecycle handle ──
        #[derive(Debug)]
        struct PreserveTestLifecycle;
        #[async_trait::async_trait]
        impl StepLifecycle for PreserveTestLifecycle {
            fn name(&self) -> &'static str {
                "preserve-test-lifecycle"
            }
            async fn shutdown(
                &self,
                _reason: StepShutdownReason,
            ) -> Result<(), camel_api::CamelError> {
                Ok(())
            }
        }

        // ── Build controller ──
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        registry
            .lock()
            .unwrap()
            .register(Arc::new(camel_component_timer::TimerComponent::new()));
        let mut controller = DefaultRouteController::new(
            registry,
            Arc::new(camel_api::NoopPlatformService::default()),
        );

        controller
            .add_route(
                RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                    .with_route_id("preserve-lifecycle"),
            )
            .await
            .unwrap();

        let (ctrl_handle, _actor_join) = spawn_controller_actor(controller);

        // Build RuntimeBus + handle with test lifecycle injection
        let store = InMemoryRuntimeStore::default();
        let execution = Arc::new(RuntimeExecutionAdapter::new(ctrl_handle.clone()));
        let runtime_bus = Arc::new(
            RuntimeBus::new(
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
            )
            .with_execution(execution),
        );
        let handle = RuntimeExecutionHandle {
            controller: ctrl_handle.clone(),
            runtime: runtime_bus,
            function_invoker: None,
            test_lifecycle_inject: Arc::new(std::sync::Mutex::new(Some(vec![Arc::new(
                PreserveTestLifecycle,
            )
                as Arc<dyn StepLifecycle>]))),
        };

        // Call apply_swap
        let mut new_defs = vec![
            RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                .with_route_id("preserve-lifecycle"),
        ];
        let mut errors = vec![];
        apply_swap(
            "preserve-lifecycle".into(),
            &mut new_defs,
            &handle,
            Duration::from_secs(5),
            None,
            &mut errors,
        )
        .await;

        assert!(
            errors.is_empty(),
            "apply_swap should succeed; got {errors:?}"
        );

        // ── Verify post-swap: lifecycle preserved in PipelineAssembly ──
        // After the Restart path, swap_pipeline_raw stores lifecycle in the
        // assembly.  A subsequent atomic swap must be rejected because the
        // assembly now carries lifecycle handles.
        let identity = camel_api::BoxProcessor::new(IdentityProcessor);
        let swap_result = ctrl_handle
            .swap_pipeline("preserve-lifecycle", identity)
            .await;
        assert!(
            swap_result.is_err(),
            "atomic swap should be rejected — lifecycle was preserved"
        );
        let err_msg = swap_result.unwrap_err().to_string();
        assert!(
            err_msg.contains("lifecycle-bearing"),
            "rejection should mention lifecycle-bearing; got: {err_msg}"
        );

        // Cleanup
        let _ = ctrl_handle.stop_route("preserve-lifecycle").await;
    }

    // ── Oracle Fix 2: stateless → resequencer swap regression test ──
    //
    // Verifies that when function_ctx is None and the compiled route carries
    // lifecycle handles (as a real resequencer would), the swap goes through
    // the Restart path (not atomic) and lifecycle is preserved in the
    // PipelineAssembly.

    /// Oracle Fix 2: verify that a stateless-to-lifecycle swap via
    /// `function_ctx == None` uses the Restart path and preserves lifecycle
    /// handles in the PipelineAssembly.
    ///
    /// Steps:
    /// 1. Start a non-lifecycle route
    /// 2. Hot-swap with injected lifecycle + `function_ctx: None`
    /// 3. Assert Restart path was used (subsequent atomic swap fails)
    /// 4. Assert PipelineAssembly has the lifecycle handle
    /// 5. Assert old route's lifecycle drains on stop
    #[tokio::test]
    async fn oracle_fix2_stateless_to_resequencer_swap_preserves_lifecycle() {
        use crate::context::RuntimeExecutionHandle;
        use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
        use crate::lifecycle::adapters::in_memory::InMemoryRuntimeStore;
        use crate::lifecycle::adapters::route_controller::DefaultRouteController;
        use crate::lifecycle::adapters::runtime_execution::RuntimeExecutionAdapter;
        use crate::lifecycle::application::runtime_bus::RuntimeBus;
        use crate::shared::components::domain::Registry;
        use camel_api::{IdentityProcessor, StepLifecycle, StepShutdownReason};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        // ── Lifecycle handle that records shutdown ──
        #[derive(Debug)]
        struct OracleFix2Lifecycle {
            drained: Arc<AtomicBool>,
        }
        #[async_trait::async_trait]
        impl StepLifecycle for OracleFix2Lifecycle {
            fn name(&self) -> &'static str {
                "oracle-fix2-lifecycle"
            }
            async fn shutdown(
                &self,
                _reason: StepShutdownReason,
            ) -> Result<(), camel_api::CamelError> {
                self.drained.store(true, Ordering::SeqCst);
                Ok(())
            }
        }

        let drained_flag = Arc::new(AtomicBool::new(false));
        let lifecycle: Arc<dyn StepLifecycle> = Arc::new(OracleFix2Lifecycle {
            drained: Arc::clone(&drained_flag),
        });

        // ── Build controller with a plain non-lifecycle route ──
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        registry
            .lock()
            .unwrap()
            .register(Arc::new(camel_component_timer::TimerComponent::new()));
        let mut controller = DefaultRouteController::new(
            registry,
            Arc::new(camel_api::NoopPlatformService::default()),
        );

        controller
            .add_route(
                RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                    .with_route_id("oracle-fix2-test"),
            )
            .await
            .unwrap();

        // Old route has NO lifecycle (plain timer → stateless).
        // NEW route simulates resequencer via test_lifecycle_inject.

        let (ctrl_handle, _actor_join) = spawn_controller_actor(controller);

        let store = InMemoryRuntimeStore::default();
        let execution = Arc::new(RuntimeExecutionAdapter::new(ctrl_handle.clone()));
        let runtime_bus = Arc::new(
            RuntimeBus::new(
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
                Arc::new(store.clone()),
            )
            .with_execution(execution),
        );

        let handle = RuntimeExecutionHandle {
            controller: ctrl_handle.clone(),
            runtime: runtime_bus,
            function_invoker: None,
            // Inject lifecycle to simulate a resequencer route compiled via
            // function_ctx == None path.
            test_lifecycle_inject: Arc::new(std::sync::Mutex::new(Some(vec![
                Arc::clone(&lifecycle) as Arc<dyn StepLifecycle>,
            ]))),
        };

        // ── Swap: function_ctx == None ──
        let mut new_defs = vec![
            RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                .with_route_id("oracle-fix2-test"),
        ];
        let mut errors = vec![];
        apply_swap(
            "oracle-fix2-test".into(),
            &mut new_defs,
            &handle,
            Duration::from_secs(5),
            None, // ← Oracle Fix 2: no function context
            &mut errors,
        )
        .await;

        // Oracle assertion 3: swap should succeed
        assert!(
            errors.is_empty(),
            "apply_swap should succeed for oracle fix 2; got {errors:?}"
        );

        let exists = ctrl_handle.route_exists("oracle-fix2-test").await.unwrap();
        assert!(exists, "route should exist after oracle fix 2 swap");

        // Oracle assertion 3 (cont'd): Restart path was used → subsequent
        // atomic swap is rejected because PipelineAssembly now carries lifecycle.
        let identity = camel_api::BoxProcessor::new(IdentityProcessor);
        let swap_result = ctrl_handle
            .swap_pipeline("oracle-fix2-test", identity)
            .await;
        assert!(
            swap_result.is_err(),
            "atomic swap should be rejected — Restart path stored lifecycle; got {:?}",
            swap_result
        );
        let err_msg = swap_result.unwrap_err().to_string();
        assert!(
            err_msg.contains("lifecycle-bearing"),
            "rejection should mention lifecycle-bearing; got: {err_msg}"
        );

        // Oracle assertion 4: PipelineAssembly has the lifecycle handle
        // (verified via rejection above — the assembly IS lifecycle-bearing).

        // Oracle assertion 5: Stop the route and verify lifecycle drains.
        // The `stop_route` method drains lifecycle handles via
        // consumer_management::stop_route which iterates assembly.lifecycle.
        let handle_ref = ctrl_handle.clone();
        let stop_result = handle_ref.stop_route("oracle-fix2-test").await;
        assert!(
            stop_result.is_ok(),
            "stop_route should succeed; got {stop_result:?}"
        );

        // Verify drain flag was set
        assert!(
            drained_flag.load(Ordering::SeqCst),
            "lifecycle.shutdown should have been called via drain on stop_route"
        );
    }
}
