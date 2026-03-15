use std::sync::Arc;

use async_trait::async_trait;
use camel_api::{
    CamelError, RuntimeCommand, RuntimeCommandBus, RuntimeQuery, RuntimeQueryBus,
    RuntimeQueryResult,
};
use camel_core::{
    FileRuntimeEventJournal, InMemoryRuntimeStore, ProjectionStorePort, RouteRepositoryPort,
    RouteRuntimeAggregate, RouteRuntimeState, RouteStatusProjection, RuntimeBus, RuntimeEvent,
    RuntimeEventJournalPort, RuntimeUnitOfWorkPort,
};
use tempfile::tempdir;

#[derive(Clone)]
struct FailingJournal;

#[async_trait]
impl RuntimeEventJournalPort for FailingJournal {
    async fn append_batch(&self, _events: &[RuntimeEvent]) -> Result<(), CamelError> {
        Err(CamelError::Io("forced journal failure".to_string()))
    }

    async fn load_all(&self) -> Result<Vec<RuntimeEvent>, CamelError> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn uow_write_is_atomic_when_journal_append_fails() {
    let store = InMemoryRuntimeStore::default().with_journal(Arc::new(FailingJournal));
    let runtime = RuntimeBus::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
    )
    .with_uow(Arc::new(store.clone()));

    let err = runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: camel_api::CanonicalRouteSpec::new("journal-r1", "timer:tick"),
            command_id: "cmd-j-1".to_string(),
            causation_id: None,
        })
        .await
        .expect_err("register should fail when journal append fails");

    assert!(
        err.to_string().contains("forced journal failure"),
        "unexpected error: {err}"
    );
    assert!(
        store.load("journal-r1").await.unwrap().is_none(),
        "aggregate must not be persisted on journal failure"
    );
    assert!(
        store.get_status("journal-r1").await.unwrap().is_none(),
        "projection must not be persisted on journal failure"
    );
    assert!(
        store.snapshot_events().await.is_empty(),
        "in-memory events must remain empty on journal failure"
    );
}

#[tokio::test]
async fn file_journal_persists_and_replays_runtime_events() {
    let dir = tempdir().unwrap();
    let journal_path = dir.path().join("runtime-events.jsonl");
    let journal = Arc::new(FileRuntimeEventJournal::new(journal_path).unwrap());
    let store = InMemoryRuntimeStore::default().with_journal(journal.clone());

    let runtime = RuntimeBus::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
    )
    .with_uow(Arc::new(store));

    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: camel_api::CanonicalRouteSpec::new("journal-r2", "timer:tick"),
            command_id: "cmd-j-2".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "journal-r2".to_string(),
            command_id: "cmd-j-3".to_string(),
            causation_id: Some("cmd-j-2".to_string()),
        })
        .await
        .unwrap();

    let replayed = journal.load_all().await.unwrap();
    assert!(replayed.iter().any(
        |e| matches!(e, RuntimeEvent::RouteRegistered { route_id } if route_id == "journal-r2")
    ));
    assert!(
        replayed.iter().any(
            |e| matches!(e, RuntimeEvent::RouteStarted { route_id } if route_id == "journal-r2")
        )
    );
}

#[tokio::test]
async fn optimistic_conflict_does_not_append_journal_events() {
    let dir = tempdir().unwrap();
    let journal = Arc::new(
        FileRuntimeEventJournal::new(dir.path().join("runtime-events-conflict.jsonl")).unwrap(),
    );
    let store = InMemoryRuntimeStore::default().with_journal(journal.clone());

    store
        .save(RouteRuntimeAggregate::new("journal-r3"))
        .await
        .unwrap();

    let err = store
        .persist_upsert(
            RouteRuntimeAggregate::from_snapshot("journal-r3", RouteRuntimeState::Started, 1),
            Some(99),
            RouteStatusProjection {
                route_id: "journal-r3".to_string(),
                status: "Started".to_string(),
            },
            &[RuntimeEvent::RouteStarted {
                route_id: "journal-r3".to_string(),
            }],
        )
        .await
        .expect_err("expected optimistic lock conflict");

    assert!(
        err.to_string().contains("optimistic lock conflict"),
        "unexpected error: {err}"
    );
    let replayed = journal.load_all().await.unwrap();
    assert!(
        replayed.is_empty(),
        "journal must not append events when optimistic check fails"
    );
}

#[tokio::test]
async fn runtime_bus_recovers_projection_from_journal_on_first_query() {
    let dir = tempfile::tempdir().unwrap();
    let journal =
        Arc::new(FileRuntimeEventJournal::new(dir.path().join("runtime-recovery.jsonl")).unwrap());

    let writer_store = InMemoryRuntimeStore::default().with_journal(journal.clone());
    let writer_runtime = RuntimeBus::new(
        Arc::new(writer_store.clone()),
        Arc::new(writer_store.clone()),
        Arc::new(writer_store.clone()),
        Arc::new(writer_store.clone()),
    )
    .with_uow(Arc::new(writer_store.clone()));

    writer_runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: camel_api::CanonicalRouteSpec::new("journal-r4", "timer:tick"),
            command_id: "recovery-c1".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();
    writer_runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "journal-r4".to_string(),
            command_id: "recovery-c2".to_string(),
            causation_id: Some("recovery-c1".to_string()),
        })
        .await
        .unwrap();

    let cold_store = InMemoryRuntimeStore::default().with_journal(journal.clone());
    let cold_runtime = RuntimeBus::new(
        Arc::new(cold_store.clone()),
        Arc::new(cold_store.clone()),
        Arc::new(cold_store.clone()),
        Arc::new(cold_store.clone()),
    )
    .with_uow(Arc::new(cold_store.clone()));

    let status = cold_runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "journal-r4".to_string(),
        })
        .await
        .unwrap();

    assert_eq!(
        status,
        RuntimeQueryResult::RouteStatus {
            route_id: "journal-r4".to_string(),
            status: "Started".to_string(),
        }
    );
}
