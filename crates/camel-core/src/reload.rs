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

use crate::route::RouteDefinition;
use crate::route_controller::DefaultRouteController;

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

/// Compute the diff between new definitions and active routes.
pub fn compute_reload_actions(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
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
}
