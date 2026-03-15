use std::sync::Arc;

use camel_api::{
    CanonicalRouteSpec, RuntimeCommand, RuntimeCommandBus, RuntimeQuery, RuntimeQueryBus,
    RuntimeQueryResult,
};
use camel_core::{
    InMemoryCommandDedup, InMemoryEventPublisher, InMemoryProjectionStore, InMemoryRouteRepository,
    RuntimeBus,
};

#[tokio::test]
async fn get_route_status_reads_projection_not_write_model() {
    let runtime = RuntimeBus::new(
        Arc::new(InMemoryRouteRepository::default()),
        Arc::new(InMemoryProjectionStore::default()),
        Arc::new(InMemoryEventPublisher::default()),
        Arc::new(InMemoryCommandDedup::default()),
    );

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("r1", "timer:tick"),
            command_id: "c1".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "r1".into(),
            command_id: "c2".into(),
            causation_id: Some("c1".into()),
        })
        .await
        .unwrap();

    let out = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "r1".into(),
        })
        .await
        .unwrap();

    match out {
        RuntimeQueryResult::RouteStatus { status, .. } => assert_eq!(status, "Started"),
        _ => panic!("unexpected query result variant"),
    }
}
