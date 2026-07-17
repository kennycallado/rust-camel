/// Internal runtime event contract used by domain/application/ports.
///
/// API-layer event types can map to this contract at crate boundaries.
/// Serde lives in the adapter ring (`runtime_event_record`), not on the entity
/// (ADR-0045 §4).
#[derive(Debug, Clone, PartialEq, Eq)]
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
