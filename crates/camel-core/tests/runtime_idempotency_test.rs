use std::sync::Arc;

use camel_api::{CanonicalRouteSpec, RuntimeCommand, RuntimeCommandBus, RuntimeCommandResult};
use camel_core::{
    InMemoryCommandDedup, InMemoryEventPublisher, InMemoryProjectionStore, InMemoryRouteRepository,
    RuntimeBus,
};

#[tokio::test]
async fn duplicate_start_command_is_not_reapplied() {
    let runtime = RuntimeBus::new(
        Arc::new(InMemoryRouteRepository::default()),
        Arc::new(InMemoryProjectionStore::default()),
        Arc::new(InMemoryEventPublisher::default()),
        Arc::new(InMemoryCommandDedup::default()),
    );

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("idempotent-r1", "timer:tick"),
            command_id: "cmd-register".into(),
            causation_id: None,
        })
        .await
        .unwrap();

    let first = runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "idempotent-r1".into(),
            command_id: "cmd-start".into(),
            causation_id: Some("cmd-register".into()),
        })
        .await
        .unwrap();

    let second = runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "idempotent-r1".into(),
            command_id: "cmd-start".into(),
            causation_id: Some("cmd-register".into()),
        })
        .await
        .unwrap();

    assert!(matches!(
        first,
        RuntimeCommandResult::RouteStateChanged { .. }
    ));
    assert!(matches!(
        second,
        RuntimeCommandResult::Duplicate { command_id } if command_id == "cmd-start"
    ));
}
