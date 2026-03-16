use std::collections::HashMap;
use std::sync::Arc;

use camel_api::{CanonicalRouteSpec, RuntimeCommand, RuntimeCommandBus};
use camel_core::{
    InMemoryCommandDedup, InMemoryEventPublisher, InMemoryProjectionStore, InMemoryRouteRepository,
};
use camel_core::{ProjectionStorePort, RuntimeBus, RuntimeEvent};

#[tokio::test]
async fn replayed_events_match_projection_terminal_state() {
    let projections = Arc::new(InMemoryProjectionStore::default());
    let events = Arc::new(InMemoryEventPublisher::default());
    let runtime = RuntimeBus::new(
        Arc::new(InMemoryRouteRepository::default()),
        projections.clone(),
        events.clone(),
        Arc::new(InMemoryCommandDedup::default()),
    );

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("replay-r1", "timer:tick"),
            command_id: "cmd-register".into(),
            causation_id: None,
        })
        .await
        .unwrap();
    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "replay-r1".into(),
            command_id: "cmd-start".into(),
            causation_id: Some("cmd-register".into()),
        })
        .await
        .unwrap();
    runtime
        .execute(RuntimeCommand::StopRoute {
            route_id: "replay-r1".into(),
            command_id: "cmd-stop".into(),
            causation_id: Some("cmd-start".into()),
        })
        .await
        .unwrap();

    let replayed = replay_status(events.snapshot().await);
    let projection = projections
        .get_status("replay-r1")
        .await
        .unwrap()
        .expect("projection should exist");

    let replayed_status = replayed
        .get("replay-r1")
        .expect("replay should include route");

    assert_eq!(replayed_status, &projection.status);
}

fn replay_status(events: Vec<RuntimeEvent>) -> HashMap<String, String> {
    let mut status_by_route = HashMap::new();

    for event in events {
        match event {
            RuntimeEvent::RouteRegistered { route_id } => {
                status_by_route.insert(route_id, "Registered".to_string());
            }
            RuntimeEvent::RouteStartRequested { route_id } => {
                status_by_route.insert(route_id, "Starting".to_string());
            }
            RuntimeEvent::RouteStarted { route_id } => {
                status_by_route.insert(route_id, "Started".to_string());
            }
            RuntimeEvent::RouteFailed { route_id, .. } => {
                status_by_route.insert(route_id, "Failed".to_string());
            }
            RuntimeEvent::RouteStopped { route_id } => {
                status_by_route.insert(route_id, "Stopped".to_string());
            }
            RuntimeEvent::RouteSuspended { route_id } => {
                status_by_route.insert(route_id, "Suspended".to_string());
            }
            RuntimeEvent::RouteResumed { route_id } => {
                status_by_route.insert(route_id, "Started".to_string());
            }
            RuntimeEvent::RouteReloaded { route_id } => {
                status_by_route.insert(route_id, "Started".to_string());
            }
            RuntimeEvent::RouteRemoved { route_id } => {
                status_by_route.remove(&route_id);
            }
        }
    }

    status_by_route
}
