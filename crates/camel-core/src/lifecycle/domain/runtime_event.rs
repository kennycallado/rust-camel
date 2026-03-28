use serde::{Deserialize, Serialize};

/// Internal runtime event contract used by domain/application/ports.
///
/// API-layer event types can map to this contract at crate boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeEvent {
    RouteRegistered { route_id: String },
    RouteStartRequested { route_id: String },
    RouteStarted { route_id: String },
    RouteFailed { route_id: String, error: String },
    RouteStopped { route_id: String },
    RouteSuspended { route_id: String },
    RouteResumed { route_id: String },
    RouteReloaded { route_id: String },
    RouteRemoved { route_id: String },
}
