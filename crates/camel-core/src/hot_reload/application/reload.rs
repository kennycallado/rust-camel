//! Route reload coordinator.
//!
//! Compares a new set of route definitions against the currently running routes
//! and computes the minimal set of actions: SWAP, RESTART, ADD, REMOVE, or SKIP.

use std::time::Duration;

use camel_api::CamelError;

use crate::context::RuntimeExecutionHandle;
use crate::hot_reload::domain::ReloadAction;
#[cfg(test)]
use crate::lifecycle::adapters::route_controller::DefaultRouteController;
use crate::lifecycle::application::route_definition::RouteDefinition;

use super::reload_actions;

pub struct FunctionReloadContext {
    pub invoker: std::sync::Arc<dyn camel_api::function::FunctionInvoker>,
    pub generation: u64,
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
                reload_actions::apply_swap(
                    route_id,
                    &mut new_definitions,
                    controller,
                    drain_timeout,
                    function_ctx,
                    &mut errors,
                )
                .await;
            }

            ReloadAction::Add { route_id } => {
                reload_actions::apply_add(
                    route_id,
                    &mut new_definitions,
                    controller,
                    function_ctx,
                    &mut errors,
                )
                .await;
            }

            ReloadAction::Remove { route_id } => {
                reload_actions::apply_remove(
                    route_id,
                    controller,
                    drain_timeout,
                    function_ctx,
                    &mut errors,
                )
                .await;
            }

            ReloadAction::Restart { route_id } => {
                reload_actions::apply_restart(
                    route_id,
                    &mut new_definitions,
                    controller,
                    drain_timeout,
                    function_ctx,
                    &mut errors,
                )
                .await;
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
    use std::sync::Arc;

    fn make_controller() -> DefaultRouteController {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        DefaultRouteController::new(
            registry,
            Arc::new(camel_api::NoopPlatformService::default()),
        )
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
