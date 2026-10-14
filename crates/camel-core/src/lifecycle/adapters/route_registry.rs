//! Route registry — wraps `HashMap<String, ManagedRoute>` with read-only query methods.
//!
//! Extracted from [`DefaultRouteController`](super::route_controller::DefaultRouteController)
//! to reduce the size of the monolithic controller and isolate route-map logic.

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::{BoxProcessor, InFlightGauge};

use super::consumer_management;
use super::route_helpers::{ManagedRoute, handle_is_running, inferred_lifecycle_label};

pub(super) const DEFAULT_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Wraps a `HashMap<String, ManagedRoute>` and provides query/mutation helpers.
///
/// Routes tied on `startup_order` start in definition (insertion) order and
/// stop in the reverse order, like Camel Java. `HashMap` iteration order is
/// random, so each route also carries an insertion sequence number.
pub(crate) struct RouteRegistry {
    routes: HashMap<String, ManagedRoute>,
    /// Insertion sequence per route id. Replacing a route keeps its slot;
    /// removing it frees the slot (a later re-add goes last).
    seq: HashMap<String, u64>,
    next_seq: u64,
}

impl RouteRegistry {
    /// Create an empty registry.
    pub(super) fn new() -> Self {
        Self {
            routes: HashMap::new(),
            seq: HashMap::new(),
            next_seq: 0,
        }
    }

    // ── Map accessors (delegated from controller) ──

    pub(super) fn get(&self, route_id: &str) -> Option<&ManagedRoute> {
        self.routes.get(route_id)
    }

    pub(super) fn get_mut(&mut self, route_id: &str) -> Option<&mut ManagedRoute> {
        self.routes.get_mut(route_id)
    }

    pub(super) fn contains_key(&self, route_id: &str) -> bool {
        self.routes.contains_key(route_id)
    }

    pub(super) fn insert(
        &mut self,
        route_id: String,
        managed: ManagedRoute,
    ) -> Option<ManagedRoute> {
        if !self.seq.contains_key(&route_id) {
            self.seq.insert(route_id.clone(), self.next_seq);
            self.next_seq += 1;
        }
        self.routes.insert(route_id, managed)
    }

    pub(super) fn remove(&mut self, route_id: &str) -> Option<ManagedRoute> {
        self.seq.remove(route_id);
        self.routes.remove(route_id)
    }

    /// `(id, startup_order, seq)` for routes passing `keep`, sorted
    /// ascending by `(startup_order, seq)` — a total order, so ties are
    /// deterministic.
    fn sorted_by_start(&self, keep: impl Fn(&ManagedRoute) -> bool) -> Vec<(String, i32)> {
        let mut rows: Vec<(String, i32, u64)> = self
            .routes
            .iter()
            .filter(|(_, r)| keep(r))
            .map(|(id, r)| {
                (
                    id.clone(),
                    r.definition.startup_order(),
                    self.seq.get(id).copied().unwrap_or(u64::MAX),
                )
            })
            .collect();
        rows.sort_by(|a, b| (a.1, a.2, &a.0).cmp(&(b.1, b.2, &b.0)));
        rows.into_iter().map(|(id, order, _)| (id, order)).collect()
    }

    #[allow(dead_code)]
    pub(super) fn iter(&self) -> impl Iterator<Item = (&String, &ManagedRoute)> {
        self.routes.iter()
    }

    #[allow(dead_code)]
    pub(super) fn iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut ManagedRoute)> {
        self.routes.iter_mut()
    }

    // ── Query methods ──

    /// Returns the number of routes in the registry.
    pub(super) fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Returns `true` if a route with the given ID exists.
    pub(super) fn route_exists(&self, route_id: &str) -> bool {
        self.routes.contains_key(route_id)
    }

    /// Returns all route IDs.
    pub(super) fn route_ids(&self) -> Vec<String> {
        self.routes.keys().cloned().collect()
    }

    /// Returns the source hash of a route, if it exists.
    pub(super) fn route_source_hash(&self, route_id: &str) -> Option<u64> {
        self.routes
            .get(route_id)
            .and_then(|m| m.definition.source_hash())
    }

    /// Returns route IDs that should auto-start, sorted by startup order
    /// (ascending), ties in definition order.
    pub(super) fn auto_startup_route_ids(&self) -> Vec<String> {
        self.auto_startup_sorted()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// Returns route IDs in shutdown order: the exact reverse of startup order.
    pub(super) fn shutdown_route_ids(&self) -> Vec<String> {
        self.shutdown_sorted()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// Returns the in-flight exchange count for a route, if the route has UoW tracking.
    pub(super) fn in_flight_count(&self, route_id: &str) -> Option<u64> {
        self.routes
            .get(route_id)
            .map(|r| r.in_flight.as_ref().map_or(0, |c| c.total()))
    }

    /// Returns a clone of the in-flight gauge for a route (for use in hot-reload).
    pub(super) fn in_flight_counter(&self, route_id: &str) -> Option<Arc<InFlightGauge>> {
        self.routes
            .get(route_id)
            .and_then(|r| r.in_flight.as_ref().map(Arc::clone))
    }

    /// Returns the `from_uri` of a route, if it exists.
    pub(super) fn route_from_uri(&self, route_id: &str) -> Option<String> {
        self.routes.get(route_id).map(|r| r.from_uri.clone())
    }

    /// Returns the current pipeline for a route.
    pub(super) fn get_pipeline(&self, route_id: &str) -> Option<BoxProcessor> {
        self.routes
            .get(route_id)
            .map(|r| super::pipeline_runtime::get_pipeline(&r.pipeline))
    }

    // ── Lifecycle helpers ──

    /// Internal stop implementation.
    pub(super) async fn stop_route(&mut self, route_id: &str) -> Result<(), camel_api::CamelError> {
        consumer_management::stop_route_internal(
            &mut self.routes,
            route_id,
            DEFAULT_SHUTDOWN_TIMEOUT,
        )
        .await
    }

    /// Check if a route is running.
    #[allow(dead_code)]
    pub(super) fn is_route_running(&self, route_id: &str) -> bool {
        self.routes.get(route_id).is_some_and(|m| {
            handle_is_running(&m.consumer_handle) || handle_is_running(&m.pipeline_handle)
        })
    }

    /// Return a human-readable lifecycle label for a route.
    #[allow(dead_code)]
    pub(super) fn lifecycle_label(&self, route_id: &str) -> Option<&'static str> {
        self.routes
            .get(route_id)
            .map(|m| inferred_lifecycle_label(m))
    }

    /// Iterate over routes with their IDs for startup (auto-startup, sorted).
    pub(super) fn auto_startup_sorted(&self) -> Vec<(String, i32)> {
        self.sorted_by_start(|r| r.definition.auto_startup())
    }

    /// Iterate over routes with their IDs for shutdown (reverse of startup).
    pub(super) fn shutdown_sorted(&self) -> Vec<(String, i32)> {
        let mut pairs = self.sorted_by_start(|_| true);
        pairs.reverse();
        pairs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::adapters::route_runtime_state;
    use crate::lifecycle::application::route_definition::RouteDefinition;

    fn example_managed() -> ManagedRoute {
        ManagedRoute {
            definition: RouteDefinition::new("timer:test", vec![])
                .with_route_id("test")
                .to_info(),
            from_uri: "timer:test".into(),
            pipeline: crate::lifecycle::adapters::pipeline_runtime::new_identity_pipeline(),
            concurrency: None,
            consumer_handle: None,
            pipeline_handle: None,
            consumer_cancel_token: tokio_util::sync::CancellationToken::new(),
            pipeline_cancel_token: tokio_util::sync::CancellationToken::new(),
            channel_sender: None,
            in_flight: None,
            drain_in_flight: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            aggregate_split: None,
            agg_service: None,
            compiled: route_runtime_state::CompiledRoute {
                security_policy: None,
                security_authenticator: None,
                provider_registry: None,
                security_plan: None,
            },
        }
    }

    #[test]
    fn route_registry_new_is_empty() {
        let reg = RouteRegistry::new();
        assert_eq!(reg.route_count(), 0);
        assert!(reg.route_ids().is_empty());
    }

    #[test]
    fn route_registry_insert_and_contains() {
        let mut reg = RouteRegistry::new();
        reg.insert("r1".into(), example_managed());
        assert!(reg.contains_key("r1"));
        assert_eq!(reg.route_count(), 1);
    }

    #[test]
    fn route_registry_remove() {
        let mut reg = RouteRegistry::new();
        reg.insert("r1".into(), example_managed());
        assert!(reg.remove("r1").is_some());
        assert!(!reg.contains_key("r1"));
        assert_eq!(reg.route_count(), 0);
    }

    #[test]
    fn route_registry_route_exists_matches() {
        let mut reg = RouteRegistry::new();
        reg.insert("r1".into(), example_managed());
        assert!(reg.route_exists("r1"));
        assert!(!reg.route_exists("r2"));
    }

    #[test]
    fn route_registry_route_ids() {
        let mut reg = RouteRegistry::new();
        reg.insert("a".into(), example_managed());
        reg.insert("b".into(), example_managed());
        let mut ids = reg.route_ids();
        ids.sort();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn route_registry_source_hash_returns_none_when_absent() {
        let reg = RouteRegistry::new();
        assert_eq!(reg.route_source_hash("missing"), None);
    }

    #[test]
    fn route_registry_get_and_get_mut() {
        let mut reg = RouteRegistry::new();
        reg.insert("r1".into(), example_managed());
        assert!(reg.get("r1").is_some());
        assert!(reg.get_mut("r1").is_some());
        assert!(reg.get("r2").is_none());
        assert!(reg.get_mut("r2").is_none());
    }

    #[test]
    fn route_registry_auto_startup_and_shutdown_ordering() {
        let mut reg = RouteRegistry::new();

        let low = ManagedRoute {
            definition: RouteDefinition::new("timer:a", vec![])
                .with_route_id("low")
                .with_startup_order(1)
                .with_auto_startup(true)
                .to_info(),
            ..example_managed()
        };
        let high = ManagedRoute {
            definition: RouteDefinition::new("timer:b", vec![])
                .with_route_id("high")
                .with_startup_order(100)
                .with_auto_startup(true)
                .to_info(),
            ..example_managed()
        };
        let no_auto = ManagedRoute {
            definition: RouteDefinition::new("timer:c", vec![])
                .with_route_id("noauto")
                .with_startup_order(50)
                .with_auto_startup(false)
                .to_info(),
            ..example_managed()
        };

        reg.insert("high".into(), high);
        reg.insert("low".into(), low);
        reg.insert("noauto".into(), no_auto);

        // auto_startup_route_ids should include only auto-startup routes, sorted by startup_order ascending
        assert_eq!(reg.auto_startup_route_ids(), vec!["low", "high"]);

        // shutdown_route_ids should include all routes, sorted descending
        assert_eq!(reg.shutdown_route_ids(), vec!["high", "noauto", "low"]);

        // auto_startup_sorted should return pairs (id, order) for auto-startup routes
        let sorted = reg.auto_startup_sorted();
        assert_eq!(sorted.len(), 2);
        assert_eq!(sorted[0], ("low".to_string(), 1));
        assert_eq!(sorted[1], ("high".to_string(), 100));

        // shutdown_sorted should return all pairs sorted descending
        let shutdown = reg.shutdown_sorted();
        assert_eq!(shutdown.len(), 3);
        assert_eq!(shutdown[0], ("high".to_string(), 100));
        assert_eq!(shutdown[1], ("noauto".to_string(), 50));
        assert_eq!(shutdown[2], ("low".to_string(), 1));
    }

    /// rc-xd7tq: routes tied on `startup_order` start in definition order
    /// and stop in the reverse (LIFO). Many ids make a lucky `HashMap`
    /// iteration order (1/n!) practically impossible.
    #[test]
    fn route_registry_ties_follow_definition_order() {
        let mut reg = RouteRegistry::new();
        let ids: Vec<String> = (0..24).map(|i| format!("tied-{i}")).collect();
        for id in &ids {
            let managed = ManagedRoute {
                definition: RouteDefinition::new("timer:t", vec![])
                    .with_route_id(id.as_str())
                    .with_auto_startup(true)
                    .to_info(),
                ..example_managed()
            };
            reg.insert(id.clone(), managed);
        }
        assert_eq!(reg.auto_startup_route_ids(), ids);
        let startup: Vec<String> = reg
            .auto_startup_sorted()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(startup, ids);

        let mut reversed = ids.clone();
        reversed.reverse();
        assert_eq!(reg.shutdown_route_ids(), reversed);
        let shutdown: Vec<String> = reg
            .shutdown_sorted()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(shutdown, reversed);
    }

    /// rc-xd7tq: replacing a route (hot reload) keeps its original slot;
    /// removing and re-adding moves it to the end.
    #[test]
    fn route_registry_replace_keeps_slot_remove_readds_last() {
        let mut reg = RouteRegistry::new();
        let make = |id: &str| ManagedRoute {
            definition: RouteDefinition::new("timer:t", vec![])
                .with_route_id(id)
                .with_auto_startup(true)
                .to_info(),
            ..example_managed()
        };
        for id in ["a", "b", "c"] {
            reg.insert(id.into(), make(id));
        }
        reg.insert("a".into(), make("a"));
        assert_eq!(reg.auto_startup_route_ids(), vec!["a", "b", "c"]);

        reg.remove("a");
        reg.insert("a".into(), make("a"));
        assert_eq!(reg.auto_startup_route_ids(), vec!["b", "c", "a"]);
    }

    #[test]
    fn route_registry_in_flight_count_defaults_to_zero() {
        let mut reg = RouteRegistry::new();
        reg.insert("r1".into(), example_managed());
        assert_eq!(reg.in_flight_count("r1"), Some(0));
        assert_eq!(reg.in_flight_count("missing"), None);
    }

    #[test]
    fn route_registry_in_flight_counter_and_from_uri() {
        let mut reg = RouteRegistry::new();
        reg.insert("r1".into(), example_managed());
        // in_flight is None when UoW is not configured, so counter returns None
        assert!(reg.in_flight_counter("r1").is_none());
        assert_eq!(reg.in_flight_count("r1"), Some(0));
        assert_eq!(reg.route_from_uri("r1"), Some("timer:test".into()));
        assert_eq!(reg.route_from_uri("missing"), None);
    }

    #[test]
    fn route_registry_get_pipeline() {
        let mut reg = RouteRegistry::new();
        reg.insert("r1".into(), example_managed());
        assert!(reg.get_pipeline("r1").is_some());
        assert!(reg.get_pipeline("missing").is_none());
    }

    #[test]
    fn route_registry_is_route_running() {
        let mut reg = RouteRegistry::new();
        reg.insert("r1".into(), example_managed());
        assert!(!reg.is_route_running("r1"));
        assert!(!reg.is_route_running("missing"));
    }

    #[test]
    fn route_registry_lifecycle_label() {
        let mut reg = RouteRegistry::new();
        reg.insert("r1".into(), example_managed());
        assert_eq!(reg.lifecycle_label("r1"), Some("Stopped"));
        assert_eq!(reg.lifecycle_label("missing"), None);
    }
}
