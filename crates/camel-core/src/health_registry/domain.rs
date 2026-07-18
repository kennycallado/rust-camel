//! Domain layer of the health_registry vertical slice.
//!
//! This module owns the `HealthCheckRegistry` aggregate and its private state
//! types. The aggregate is the bounded context's entity: it holds the per-route
//! probe state and a cancellation token. Inherent methods on the aggregate
//! that only mutate state (no I/O) live here. The use-case orchestration
//! (`check_all`) lives in the sibling `application` module. The external trait
//! delegation lives in the sibling ring-three module.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

pub(super) struct ForcedEntry {
    pub(super) name: String,
    pub(super) reason: String,
    pub(super) probe_generation_at_force: u64,
    pub(super) started_after_force: bool,
    pub(super) forced_at: Option<Instant>,
    pub(super) ttl: Option<Duration>,
}

pub(super) struct RouteHealth {
    pub(super) active: bool,
    pub(super) live: Vec<Arc<dyn camel_api::AsyncHealthCheck>>,
    pub(super) forced: Option<ForcedEntry>,
    pub(super) probe_generation: u64,
}

pub struct HealthCheckRegistry {
    entries: RwLock<HashMap<String, RouteHealth>>,
    default_timeout: Duration,
    cancel_token: CancellationToken,
    forced_ttl: Option<Duration>,
}

impl HealthCheckRegistry {
    pub fn new(default_timeout: Duration) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            default_timeout,
            cancel_token: CancellationToken::new(),
            forced_ttl: None,
        }
    }

    /// R4-L12: opt-in TTL for forced-unhealthy entries. When set, a forced entry
    /// whose age exceeds the TTL has its reason updated (but is NOT cleared —
    /// TTL alone never declares Ready). Recovery still requires both a later
    /// probe generation AND a post-force Started marker.
    pub fn with_forced_ttl(mut self, ttl: Duration) -> Self {
        self.forced_ttl = Some(ttl);
        self
    }

    pub fn register_for_route(&self, route_id: &str, check: Arc<dyn camel_api::AsyncHealthCheck>) {
        let mut entries = self.entries.write();
        let route_health = entries
            .entry(route_id.to_string())
            .or_insert_with(|| RouteHealth {
                active: false,
                live: Vec::new(),
                forced: None,
                probe_generation: 0,
            });
        let check_name = check.name();
        if let Some(existing) = route_health
            .live
            .iter()
            .position(|c| c.name() == check_name)
        {
            route_health.live[existing] = check;
        } else {
            route_health.live.push(check);
        }
        // R4-L12: advancing the probe generation is one of two conditions for
        // clearing a forced entry (the other is a post-force Started marker).
        // We no longer eagerly clear `forced` here — that was too aggressive
        // because any probe registration (even an old probe testing a different
        // dependency) would clear a force targeting the dead consumer.
        route_health.probe_generation += 1;
    }

    pub fn mark_route_started(&self, route_id: &str) {
        let mut entries = self.entries.write();
        if let Some(route_health) = entries.get_mut(route_id) {
            route_health.active = true;
            // R4-L12: a post-force Started marker is one of two conditions for
            // clearing a forced entry (the other is a later probe generation).
            if let Some(ref mut f) = route_health.forced {
                f.started_after_force = true;
            }
        }
    }

    pub fn mark_route_stopped(&self, route_id: &str) {
        let mut entries = self.entries.write();
        if let Some(route_health) = entries.get_mut(route_id) {
            route_health.active = false;
        }
    }

    pub fn unregister_for_route(&self, route_id: &str) {
        let mut entries = self.entries.write();
        entries.remove(route_id);
    }

    pub fn force_unhealthy_for_route(&self, route_id: &str, name: &str, reason: impl Into<String>) {
        let mut entries = self.entries.write();
        let route_health = entries
            .entry(route_id.to_string())
            .or_insert_with(|| RouteHealth {
                active: false,
                live: Vec::new(),
                forced: None,
                probe_generation: 0,
            });
        route_health.active = true;
        route_health.forced = Some(ForcedEntry {
            name: name.to_string(),
            reason: reason.into(),
            probe_generation_at_force: route_health.probe_generation,
            started_after_force: false,
            forced_at: Some(Instant::now()),
            ttl: self.forced_ttl,
        });
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Accessor for the use-case layer: locks the entries map for read access
    /// during probe aggregation. `pub(super)` so only this slice's `application`
    /// module can drive the orchestration.
    pub(super) fn entries(&self) -> &RwLock<HashMap<String, RouteHealth>> {
        &self.entries
    }

    /// Accessor for the use-case layer: returns the per-check default timeout
    /// used by `check_all` to bound individual probe duration.
    pub(super) fn default_timeout(&self) -> Duration {
        self.default_timeout
    }
}
