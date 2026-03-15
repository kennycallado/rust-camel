//! Route reload coordinator.
//!
//! Compares a new set of route definitions against the currently running routes
//! and computes the minimal set of actions: SWAP, RESTART, ADD, or REMOVE.
//!
//! # Why no Skip action?
//!
//! We cannot reliably detect when a route is truly unchanged because:
//! 1. `BoxProcessor` (the pipeline) is type-erased and cannot be compared for equality
//! 2. Partial comparison (only metadata) risks false negatives—silently ignoring changes
//!
//! Since `Swap` is an atomic pointer swap via ArcSwap (nanoseconds), the cost of
//! "unnecessary" swaps is negligible. Simplicity and correctness outweigh the
//! theoretical benefit of Skip.

use camel_api::CamelError;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::context::RuntimeExecutionHandle;
use crate::route::RouteDefinition;
#[cfg(test)]
use crate::route_controller::RouteControllerInternal;

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

/// Actions the coordinator can take per route.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ReloadAction {
    /// Pipeline may have changed — atomic swap (zero-downtime).
    ///
    /// This action is taken when the route exists and `from_uri` is unchanged.
    /// Even if the pipeline is identical, swapping is harmless (atomic pointer swap).
    #[cfg_attr(not(test), allow(dead_code))]
    Swap { route_id: String },
    /// Consumer (from_uri) changed — must stop and restart.
    Restart { route_id: String },
    /// New route — add and start.
    Add { route_id: String },
    /// Route removed from config — stop and delete.
    Remove { route_id: String },
}

/// A non-fatal error during reload action execution.
///
/// The watcher logs these and continues watching for future changes.
#[derive(Debug)]
pub(crate) struct ReloadError {
    pub route_id: String,
    pub action: String,
    pub error: CamelError,
}

/// Compute the diff between new definitions and active routes.
#[cfg(test)]
fn compute_reload_actions(
    new_definitions: &[RouteDefinition],
    controller: &dyn RouteControllerInternal,
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
                    // from_uri same — assume steps changed (we can't cheaply diff BoxProcessor)
                    actions.push(ReloadAction::Swap { route_id });
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
) -> Vec<ReloadAction> {
    let active_ids: std::collections::HashSet<String> = runtime_route_ids.iter().cloned().collect();
    let mut new_ids = std::collections::HashSet::new();
    let mut actions = Vec::new();

    for def in new_definitions {
        let route_id = def.route_id().to_string();
        new_ids.insert(route_id.clone());

        if active_ids.contains(&route_id) {
            actions.push(ReloadAction::Restart { route_id });
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
pub(crate) async fn execute_reload_actions(
    actions: Vec<ReloadAction>,
    mut new_definitions: Vec<RouteDefinition>,
    controller: &RuntimeExecutionHandle,
) -> Vec<ReloadError> {
    let mut errors = Vec::new();

    for action in actions {
        match action {
            ReloadAction::Swap { route_id } => {
                // Find and remove the matching definition by route_id
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

                // Compile new pipeline then swap using explicit execution handle operations.
                let pipeline = controller.compile_route_definition(def).await;
                match pipeline {
                    Ok(p) => {
                        let result = controller.swap_route_pipeline(&route_id, p).await;
                        if let Err(e) = result {
                            errors.push(ReloadError {
                                route_id,
                                action: "Swap".into(),
                                error: e,
                            });
                        } else {
                            tracing::info!(route_id = %route_id, "hot-reload: swapped route pipeline");
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

                let add_result = controller.add_route_definition(def).await;
                match add_result {
                    Ok(()) => {
                        if let Err(e) = controller.bootstrap_register_route(route_id.clone()).await
                        {
                            let rollback_error =
                                controller.remove_route_definition(&route_id).await.err();
                            let error = match rollback_error {
                                Some(rollback) => CamelError::RouteError(format!(
                                    "runtime bootstrap failed: {e}; rollback failed: {rollback}"
                                )),
                                None => e,
                            };
                            errors.push(ReloadError {
                                route_id,
                                action: "Add (register)".into(),
                                error,
                            });
                            continue;
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
                    Err(e) => {
                        errors.push(ReloadError {
                            route_id,
                            action: "Add".into(),
                            error: e,
                        });
                    }
                }
            }

            ReloadAction::Remove { route_id } => {
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

                // Stop → remove via runtime command bus (only if prior state was active/failed).
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

                if let Err(e) = controller.bootstrap_register_route(route_id.clone()).await {
                    let rollback_error = controller.remove_route_definition(&route_id).await.err();
                    let error = match rollback_error {
                        Some(rollback) => CamelError::RouteError(format!(
                            "runtime bootstrap failed: {e}; rollback failed: {rollback}"
                        )),
                        None => e,
                    };
                    errors.push(ReloadError {
                        route_id,
                        action: "Restart (register)".into(),
                        error,
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
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use crate::route_controller::DefaultRouteController;
    use camel_api::RouteController;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn make_controller() -> DefaultRouteController {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let mut controller = DefaultRouteController::new(registry);
        let controller_arc: Arc<Mutex<dyn RouteController>> = Arc::new(Mutex::new(
            DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new()))),
        ));
        controller.set_self_ref(controller_arc);
        controller
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

    #[test]
    fn test_removed_route_detected() {
        let mut controller = make_controller();
        let def = RouteDefinition::new("timer:tick", vec![]).with_route_id("old-route");
        controller.add_route(def).unwrap();

        let actions = compute_reload_actions(&[], &controller);
        assert_eq!(
            actions,
            vec![ReloadAction::Remove {
                route_id: "old-route".into()
            }]
        );
    }

    #[test]
    fn test_same_from_uri_detected_as_swap() {
        let mut controller = make_controller();
        let def = RouteDefinition::new("timer:tick", vec![]).with_route_id("my-route");
        controller.add_route(def).unwrap();

        let new_defs = vec![RouteDefinition::new("timer:tick", vec![]).with_route_id("my-route")];
        let actions = compute_reload_actions(&new_defs, &controller);
        assert_eq!(
            actions,
            vec![ReloadAction::Swap {
                route_id: "my-route".into()
            }]
        );
    }

    #[test]
    fn test_changed_from_uri_detected_as_restart() {
        let mut controller = make_controller();
        let def = RouteDefinition::new("timer:tick", vec![]).with_route_id("my-route");
        controller.add_route(def).unwrap();

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

    #[test]
    fn test_runtime_snapshot_drives_remove_set() {
        let mut controller = make_controller();
        controller
            .add_route(RouteDefinition::new("timer:tick", vec![]).with_route_id("runtime-route"))
            .unwrap();
        controller
            .add_route(RouteDefinition::new("timer:ghost", vec![]).with_route_id("ghost-route"))
            .unwrap();

        let runtime_ids = vec!["runtime-route".to_string()];
        let actions = compute_reload_actions_from_runtime_snapshot(&[], &runtime_ids);
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
            RouteDefinition::new("timer:tick", vec![]).with_route_id("runtime-r1"),
            RouteDefinition::new("timer:tock", vec![]).with_route_id("runtime-r2"),
        ];
        let runtime_ids = vec!["runtime-r1".to_string(), "runtime-r2".to_string()];

        let actions = compute_reload_actions_from_runtime_snapshot(&defs, &runtime_ids);
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

    // ---- execute_reload_actions tests ----
    // These use a full CamelContext with real components so that start/stop work.

    #[tokio::test]
    async fn test_execute_add_action_inserts_route() {
        use crate::CamelContext;
        use camel_component_timer::TimerComponent;

        let mut ctx = CamelContext::new();
        ctx.register_component(TimerComponent::new());
        ctx.start().await.unwrap();

        let def = RouteDefinition::new("timer:tick?period=50&repeatCount=1", vec![])
            .with_route_id("exec-add-test");
        let actions = vec![ReloadAction::Add {
            route_id: "exec-add-test".into(),
        }];
        let errors =
            execute_reload_actions(actions, vec![def], &ctx.runtime_execution_handle()).await;
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

        let mut ctx = CamelContext::new();
        ctx.register_component(TimerComponent::new());
        ctx.start().await.unwrap();

        // Add route through context so runtime aggregate/projection are seeded.
        let def =
            RouteDefinition::new("timer:tick?period=100", vec![]).with_route_id("exec-remove-test");
        ctx.add_route_definition(def).unwrap();
        assert_eq!(
            ctx.runtime_execution_handle()
                .controller_route_count_for_test()
                .await,
            1
        );

        let actions = vec![ReloadAction::Remove {
            route_id: "exec-remove-test".into(),
        }];
        let errors = execute_reload_actions(actions, vec![], &ctx.runtime_execution_handle()).await;
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

        let mut ctx = CamelContext::new();
        ctx.register_component(TimerComponent::new());
        ctx.start().await.unwrap();

        // Add route through context so runtime aggregate/projection are seeded.
        let def =
            RouteDefinition::new("timer:tick?period=100", vec![]).with_route_id("exec-swap-test");
        ctx.add_route_definition(def).unwrap();

        // Swap with same from_uri (exercises compile + swap_pipeline code path)
        let new_def =
            RouteDefinition::new("timer:tick?period=100", vec![]).with_route_id("exec-swap-test");
        let actions = vec![ReloadAction::Swap {
            route_id: "exec-swap-test".into(),
        }];
        let errors =
            execute_reload_actions(actions, vec![new_def], &ctx.runtime_execution_handle()).await;
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
    async fn test_execute_restart_action_preserves_stopped_runtime_state() {
        use crate::CamelContext;
        use camel_api::{RuntimeQuery, RuntimeQueryResult};
        use camel_component_timer::TimerComponent;

        let mut ctx = CamelContext::new();
        ctx.register_component(TimerComponent::new());
        ctx.start().await.unwrap();

        // Add route through context so runtime aggregate/projection are seeded.
        let initial = RouteDefinition::new("timer:tick?period=100", vec![])
            .with_route_id("exec-restart-test");
        ctx.add_route_definition(initial).unwrap();

        // Route is seeded as Stopped by runtime bootstrap.
        let before = ctx
            .runtime()
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: "exec-restart-test".into(),
            })
            .await
            .unwrap();
        match before {
            RuntimeQueryResult::RouteStatus { status, .. } => assert_eq!(status, "Stopped"),
            other => panic!("unexpected query result: {other:?}"),
        }

        let replacement = RouteDefinition::new("timer:tick?period=250", vec![])
            .with_route_id("exec-restart-test");
        let actions = vec![ReloadAction::Restart {
            route_id: "exec-restart-test".into(),
        }];
        let errors =
            execute_reload_actions(actions, vec![replacement], &ctx.runtime_execution_handle())
                .await;
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        // Restart should preserve prior lifecycle intent: if route was Stopped, keep it Stopped.
        let after = ctx
            .runtime()
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: "exec-restart-test".into(),
            })
            .await
            .unwrap();
        match after {
            RuntimeQueryResult::RouteStatus { status, .. } => assert_eq!(status, "Stopped"),
            other => panic!("unexpected query result: {other:?}"),
        }

        assert_eq!(
            ctx.runtime_route_status("exec-restart-test").await.unwrap(),
            Some("Stopped".to_string())
        );

        ctx.stop().await.unwrap();
    }
}
