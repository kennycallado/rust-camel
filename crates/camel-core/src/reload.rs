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

use std::sync::Arc;

use camel_api::CamelError;
use tokio::sync::Mutex;

use crate::route::RouteDefinition;
use crate::route_controller::RouteControllerInternal;

/// Actions the coordinator can take per route.
#[derive(Debug, Clone, PartialEq)]
pub enum ReloadAction {
    /// Pipeline may have changed — atomic swap (zero-downtime).
    ///
    /// This action is taken when the route exists and `from_uri` is unchanged.
    /// Even if the pipeline is identical, swapping is harmless (atomic pointer swap).
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
pub struct ReloadError {
    pub route_id: String,
    pub action: String,
    pub error: CamelError,
}

/// Compute the diff between new definitions and active routes.
pub fn compute_reload_actions(
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

/// Execute a list of reload actions against a live controller.
///
/// Non-fatal: errors for individual routes are collected and returned.
/// The caller should log them as warnings and continue watching.
///
/// `new_definitions` is consumed — each definition is moved to the controller for Add/Swap/Restart.
pub async fn execute_reload_actions(
    actions: Vec<ReloadAction>,
    mut new_definitions: Vec<RouteDefinition>,
    controller: &Arc<Mutex<dyn RouteControllerInternal>>,
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

                // Compile new pipeline then swap — two separate lock acquisitions
                let pipeline = controller.lock().await.compile_route_definition(def);
                match pipeline {
                    Ok(p) => {
                        let result = controller.lock().await.swap_pipeline(&route_id, p);
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

                let add_result = controller.lock().await.add_route(def);
                match add_result {
                    Ok(()) => {
                        // Start the new route — lock held across await via tokio::sync::Mutex
                        let start_result =
                            controller.lock().await.start_route_reload(&route_id).await;
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
                // Stop first, then remove
                let stop_result = controller.lock().await.stop_route_reload(&route_id).await;
                if let Err(e) = stop_result {
                    errors.push(ReloadError {
                        route_id: route_id.clone(),
                        action: "Remove (stop)".into(),
                        error: e,
                    });
                    continue;
                }

                let remove_result = controller.lock().await.remove_route(&route_id);
                match remove_result {
                    Ok(()) => {
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

                // Stop → remove → add → start
                let stop_result = controller.lock().await.stop_route_reload(&route_id).await;
                if let Err(e) = stop_result {
                    errors.push(ReloadError {
                        route_id,
                        action: "Restart (stop)".into(),
                        error: e,
                    });
                    continue;
                }

                if let Err(e) = controller.lock().await.remove_route(&route_id) {
                    errors.push(ReloadError {
                        route_id,
                        action: "Restart (remove)".into(),
                        error: e,
                    });
                    continue;
                }

                if let Err(e) = controller.lock().await.add_route(def) {
                    errors.push(ReloadError {
                        route_id,
                        action: "Restart (add)".into(),
                        error: e,
                    });
                    continue;
                }

                let start_result = controller.lock().await.start_route_reload(&route_id).await;
                if let Err(e) = start_result {
                    errors.push(ReloadError {
                        route_id,
                        action: "Restart (start)".into(),
                        error: e,
                    });
                } else {
                    tracing::info!(route_id = %route_id, "hot-reload: route restarted successfully");
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
        let errors = execute_reload_actions(actions, vec![def], ctx.route_controller()).await;
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        assert_eq!(ctx.route_controller().lock().await.route_count(), 1);

        ctx.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_execute_remove_action_deletes_route() {
        use crate::CamelContext;
        use camel_component_timer::TimerComponent;

        let mut ctx = CamelContext::new();
        ctx.register_component(TimerComponent::new());
        ctx.start().await.unwrap();

        // Add a route first via the controller directly, then start it
        let def =
            RouteDefinition::new("timer:tick?period=100", vec![]).with_route_id("exec-remove-test");
        ctx.route_controller().lock().await.add_route(def).unwrap();
        ctx.route_controller()
            .lock()
            .await
            .start_route_reload("exec-remove-test")
            .await
            .unwrap();
        assert_eq!(ctx.route_controller().lock().await.route_count(), 1);

        let actions = vec![ReloadAction::Remove {
            route_id: "exec-remove-test".into(),
        }];
        let errors = execute_reload_actions(actions, vec![], ctx.route_controller()).await;
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        assert_eq!(ctx.route_controller().lock().await.route_count(), 0);

        ctx.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_execute_swap_action_replaces_pipeline() {
        use crate::CamelContext;
        use camel_component_timer::TimerComponent;

        let mut ctx = CamelContext::new();
        ctx.register_component(TimerComponent::new());
        ctx.start().await.unwrap();

        // Add a route via the controller directly
        let def =
            RouteDefinition::new("timer:tick?period=100", vec![]).with_route_id("exec-swap-test");
        ctx.route_controller().lock().await.add_route(def).unwrap();

        // Swap with same from_uri (exercises compile + swap_pipeline code path)
        let new_def =
            RouteDefinition::new("timer:tick?period=100", vec![]).with_route_id("exec-swap-test");
        let actions = vec![ReloadAction::Swap {
            route_id: "exec-swap-test".into(),
        }];
        let errors = execute_reload_actions(actions, vec![new_def], ctx.route_controller()).await;
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        // Route should still exist after swap
        assert_eq!(ctx.route_controller().lock().await.route_count(), 1);

        ctx.stop().await.unwrap();
    }
}
