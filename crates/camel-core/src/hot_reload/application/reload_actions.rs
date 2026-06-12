//! Per-action reload handlers extracted from `execute_reload_actions`.
//!
//! Each top-level function corresponds to one `ReloadAction` variant (Swap, Add, Remove, Restart).
//! Shared helpers used by these handlers live here too.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use camel_api::CamelError;

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
pub(super) async fn apply_swap(
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
                let diff = compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);
                match ctx.invoker.prepare_reload(diff, ctx.generation).await {
                    Ok(token) => Some(token),
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
                    let diff =
                        compute_function_diff_for_route(&ctx.invoker, &route_id, ctx.generation);
                    if let Err(e) = ctx.invoker.finalize_reload(&diff, ctx.generation).await {
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
            tracing::info!(
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
}
