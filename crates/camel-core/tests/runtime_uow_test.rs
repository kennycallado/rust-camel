use std::sync::Arc;

use camel_api::{CanonicalRouteSpec, RuntimeCommand, RuntimeCommandBus};
use camel_core::{
    InMemoryRuntimeStore, ProjectionStorePort, RouteRepositoryPort, RouteRuntimeState, RuntimeBus,
};

#[tokio::test]
async fn unified_uow_persists_route_state_projection_and_events() {
    let store = InMemoryRuntimeStore::default();
    let runtime = RuntimeBus::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
    )
    .with_uow(Arc::new(store.clone()));

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("uow-r1", "timer:tick"),
            command_id: "uow-c1".into(),
            causation_id: None,
        })
        .await
        .unwrap();
    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "uow-r1".into(),
            command_id: "uow-c2".into(),
            causation_id: Some("uow-c1".into()),
        })
        .await
        .unwrap();

    let aggregate = store.load("uow-r1").await.unwrap().unwrap();
    assert!(matches!(aggregate.state(), RouteRuntimeState::Started));

    let projection = store.get_status("uow-r1").await.unwrap().unwrap();
    assert_eq!(projection.status, "Started");

    let events = store.snapshot_events().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, camel_core::RuntimeEvent::RouteRegistered { route_id } if route_id == "uow-r1"))
    );
    assert!(events.iter().any(
        |e| matches!(e, camel_core::RuntimeEvent::RouteStarted { route_id } if route_id == "uow-r1")
    ));
}
