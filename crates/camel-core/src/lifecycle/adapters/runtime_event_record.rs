//! Private Serde mirror for [`RuntimeEvent`] + a `#[serde(with)]` bridge.
//!
//! The entity (`lifecycle/domain/runtime_event.rs`) must not derive Serde
//! (ADR-0045 §4 — "no Serde in entities"). Persistence/CLI serialization goes
//! through this adapter-local record so the on-disk wire shape is unchanged.
use serde::{Deserialize, Serialize};

use crate::lifecycle::domain::RuntimeEvent;

/// Serde-only mirror of [`RuntimeEvent`]. Identical variant/field names keep the
/// JSON wire format byte-compatible with pre-Tier-B journals.
// variant names must match the legacy wire format — renaming breaks byte-compat
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum RuntimeEventRecord {
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

impl From<&RuntimeEvent> for RuntimeEventRecord {
    fn from(e: &RuntimeEvent) -> Self {
        match e {
            RuntimeEvent::RouteRegistered { route_id } => RuntimeEventRecord::RouteRegistered {
                route_id: route_id.clone(),
            },
            RuntimeEvent::RouteStartRequested { route_id } => {
                RuntimeEventRecord::RouteStartRequested {
                    route_id: route_id.clone(),
                }
            }
            RuntimeEvent::RouteStarted { route_id } => RuntimeEventRecord::RouteStarted {
                route_id: route_id.clone(),
            },
            RuntimeEvent::RouteFailed { route_id, error } => RuntimeEventRecord::RouteFailed {
                route_id: route_id.clone(),
                error: error.clone(),
            },
            RuntimeEvent::RouteStopped { route_id } => RuntimeEventRecord::RouteStopped {
                route_id: route_id.clone(),
            },
            RuntimeEvent::RouteSuspended { route_id } => RuntimeEventRecord::RouteSuspended {
                route_id: route_id.clone(),
            },
            RuntimeEvent::RouteResumed { route_id } => RuntimeEventRecord::RouteResumed {
                route_id: route_id.clone(),
            },
            RuntimeEvent::RouteReloaded { route_id } => RuntimeEventRecord::RouteReloaded {
                route_id: route_id.clone(),
            },
            RuntimeEvent::RouteRemoved { route_id } => RuntimeEventRecord::RouteRemoved {
                route_id: route_id.clone(),
            },
        }
    }
}

impl From<RuntimeEventRecord> for RuntimeEvent {
    fn from(r: RuntimeEventRecord) -> Self {
        match r {
            RuntimeEventRecord::RouteRegistered { route_id } => {
                RuntimeEvent::RouteRegistered { route_id }
            }
            RuntimeEventRecord::RouteStartRequested { route_id } => {
                RuntimeEvent::RouteStartRequested { route_id }
            }
            RuntimeEventRecord::RouteStarted { route_id } => {
                RuntimeEvent::RouteStarted { route_id }
            }
            RuntimeEventRecord::RouteFailed { route_id, error } => {
                RuntimeEvent::RouteFailed { route_id, error }
            }
            RuntimeEventRecord::RouteStopped { route_id } => {
                RuntimeEvent::RouteStopped { route_id }
            }
            RuntimeEventRecord::RouteSuspended { route_id } => {
                RuntimeEvent::RouteSuspended { route_id }
            }
            RuntimeEventRecord::RouteResumed { route_id } => {
                RuntimeEvent::RouteResumed { route_id }
            }
            RuntimeEventRecord::RouteReloaded { route_id } => {
                RuntimeEvent::RouteReloaded { route_id }
            }
            RuntimeEventRecord::RouteRemoved { route_id } => {
                RuntimeEvent::RouteRemoved { route_id }
            }
        }
    }
}

/// `#[serde(with = "runtime_event_serde")]` bridge: (de)serialize `RuntimeEvent`
/// through `RuntimeEventRecord` so the entity needs no Serde derive.
pub(crate) mod runtime_event_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{RuntimeEvent, RuntimeEventRecord};

    pub(crate) fn serialize<S>(event: &RuntimeEvent, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        RuntimeEventRecord::from(event).serialize(ser)
    }

    pub(crate) fn deserialize<'de, D>(de: D) -> Result<RuntimeEvent, D::Error>
    where
        D: Deserializer<'de>,
    {
        RuntimeEventRecord::deserialize(de).map(RuntimeEvent::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_roundtrips_through_json_identically_to_legacy_event() {
        let original = RuntimeEvent::RouteFailed {
            route_id: "r1".into(),
            error: "boom".into(),
        };
        let bytes = serde_json::to_vec(&RuntimeEventRecord::from(&original)).unwrap();
        // Legacy journals serialized the enum directly; mirror must match byte-for-byte.
        let legacy = br#"{"RouteFailed":{"route_id":"r1","error":"boom"}}"#;
        assert_eq!(bytes.as_slice(), &legacy[..]);
    }
}
