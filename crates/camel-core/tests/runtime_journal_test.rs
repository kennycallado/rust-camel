use std::sync::Arc;

use async_trait::async_trait;
use camel_api::{
    RuntimeCommand, RuntimeCommandBus, RuntimeQuery, RuntimeQueryBus,
    RuntimeQueryResult,
};
use camel_core::{
    InMemoryRuntimeStore, JournalDurability, ProjectionStorePort, RedbJournalOptions,
    RedbRuntimeEventJournal, RouteRepositoryPort, RouteRuntimeAggregate, RouteRuntimeState,
    RouteStatusProjection, RuntimeBus, RuntimeEvent, RuntimeEventJournalPort,
    RuntimeUnitOfWorkPort,
};
use tempfile::tempdir;
use camel_core::lifecycle::domain::DomainError;

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn new_journal(path: std::path::PathBuf) -> Arc<RedbRuntimeEventJournal> {
    Arc::new(
        RedbRuntimeEventJournal::new(
            path,
            RedbJournalOptions {
                durability: JournalDurability::Eventual,
                compaction_threshold_events: 10_000,
            },
        )
        .await
        .unwrap(),
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct FailingJournal;

#[async_trait]
impl RuntimeEventJournalPort for FailingJournal {
    async fn append_batch(&self, _events: &[RuntimeEvent]) -> Result<(), DomainError> {
        Err(DomainError::InvalidState("forced journal failure".to_string()))
    }

    async fn load_all(&self) -> Result<Vec<RuntimeEvent>, DomainError> {
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
async fn redb_journal_persists_and_replays_runtime_events() {
    let dir = tempdir().unwrap();
    let journal = new_journal(dir.path().join("runtime-events.db")).await;
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
    assert_eq!(
        replayed.len(),
        3,
        "journal must contain exactly 3 events, got {replayed:?}"
    );
    assert!(
        matches!(&replayed[0], RuntimeEvent::RouteRegistered { route_id } if route_id == "journal-r2"),
        "event[0] must be RouteRegistered, got {:?}",
        replayed[0]
    );
    assert!(
        matches!(&replayed[1], RuntimeEvent::RouteStartRequested { route_id } if route_id == "journal-r2"),
        "event[1] must be RouteStartRequested, got {:?}",
        replayed[1]
    );
    assert!(
        matches!(&replayed[2], RuntimeEvent::RouteStarted { route_id } if route_id == "journal-r2"),
        "event[2] must be RouteStarted, got {:?}",
        replayed[2]
    );
}

#[tokio::test]
async fn optimistic_conflict_does_not_append_journal_events() {
    let dir = tempdir().unwrap();
    let journal = new_journal(dir.path().join("optimistic.db")).await;
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
    let dir = tempdir().unwrap();
    let journal = new_journal(dir.path().join("runtime-recovery.db")).await;

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

#[tokio::test]
async fn accepted_command_id_survives_restart() {
    let dir = tempdir().unwrap();
    let journal = new_journal(dir.path().join("cmdid.db")).await;

    journal.append_command_id("c-persist-1").await.unwrap();
    drop(journal);

    // Simulate restart: open fresh journal on same file.
    let journal2 = new_journal(dir.path().join("cmdid.db")).await;
    let ids = journal2.load_command_ids().await.unwrap();
    assert!(
        ids.contains(&"c-persist-1".to_string()),
        "command_id must survive journal restart"
    );
}

// ── Helper: journal with low compaction threshold ────────────────────────────

async fn new_journal_with_threshold(
    path: std::path::PathBuf,
    threshold: u64,
) -> Arc<RedbRuntimeEventJournal> {
    Arc::new(
        RedbRuntimeEventJournal::new(
            path,
            RedbJournalOptions {
                durability: JournalDurability::Eventual,
                compaction_threshold_events: threshold,
            },
        )
        .await
        .unwrap(),
    )
}

#[tokio::test]
async fn redb_journal_records_route_removed_through_full_lifecycle() {
    let dir = tempdir().unwrap();
    let journal = new_journal(dir.path().join("remove-lifecycle.db")).await;
    let store = InMemoryRuntimeStore::default().with_journal(journal.clone());

    let runtime = RuntimeBus::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
    )
    .with_uow(Arc::new(store.clone()));

    let rid = "remove-r1";

    // Full lifecycle: Register -> Start -> Stop -> Remove.
    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: camel_api::CanonicalRouteSpec::new(rid, "timer:tick"),
            command_id: "rm-c1".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: rid.to_string(),
            command_id: "rm-c2".to_string(),
            causation_id: Some("rm-c1".to_string()),
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::StopRoute {
            route_id: rid.to_string(),
            command_id: "rm-c3".to_string(),
            causation_id: Some("rm-c2".to_string()),
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::RemoveRoute {
            route_id: rid.to_string(),
            command_id: "rm-c4".to_string(),
            causation_id: Some("rm-c3".to_string()),
        })
        .await
        .unwrap();

    let replayed = journal.load_all().await.unwrap();
    assert_eq!(
        replayed.len(),
        5,
        "journal must contain exactly 5 events, got {replayed:?}"
    );

    assert!(
        matches!(&replayed[0], RuntimeEvent::RouteRegistered { route_id } if route_id == rid),
        "event[0] must be RouteRegistered, got {:?}",
        replayed[0]
    );
    assert!(
        matches!(&replayed[1], RuntimeEvent::RouteStartRequested { route_id } if route_id == rid),
        "event[1] must be RouteStartRequested, got {:?}",
        replayed[1]
    );
    assert!(
        matches!(&replayed[2], RuntimeEvent::RouteStarted { route_id } if route_id == rid),
        "event[2] must be RouteStarted, got {:?}",
        replayed[2]
    );
    assert!(
        matches!(&replayed[3], RuntimeEvent::RouteStopped { route_id } if route_id == rid),
        "event[3] must be RouteStopped, got {:?}",
        replayed[3]
    );
    assert!(
        matches!(&replayed[4], RuntimeEvent::RouteRemoved { route_id } if route_id == rid),
        "event[4] must be RouteRemoved, got {:?}",
        replayed[4]
    );

    // Verify aggregate is deleted from store.
    assert!(
        store.load(rid).await.unwrap().is_none(),
        "aggregate must be deleted after RemoveRoute"
    );
    assert!(
        store.get_status(rid).await.unwrap().is_none(),
        "projection must be deleted after RemoveRoute"
    );
}

#[tokio::test]
async fn redb_journal_compaction_through_bus_removes_deleted_route_events() {
    let dir = tempdir().unwrap();
    // Use a very low threshold so compaction fires quickly.
    let journal = new_journal_with_threshold(dir.path().join("compact-bus.db"), 3).await;
    let store = InMemoryRuntimeStore::default().with_journal(journal.clone());

    let runtime = RuntimeBus::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
    )
    .with_uow(Arc::new(store));

    // Create a "doomed" route and remove it — its events should be compacted.
    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: camel_api::CanonicalRouteSpec::new("doomed", "timer:tick"),
            command_id: "comp-c1".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "doomed".to_string(),
            command_id: "comp-c2".to_string(),
            causation_id: Some("comp-c1".to_string()),
        })
        .await
        .unwrap();

    runtime
        .execute(RuntimeCommand::StopRoute {
            route_id: "doomed".to_string(),
            command_id: "comp-c3".to_string(),
            causation_id: Some("comp-c2".to_string()),
        })
        .await
        .unwrap();

    // RemoveRoute produces RouteRemoved — compaction triggers on next append
    // when event_count >= threshold (3).
    runtime
        .execute(RuntimeCommand::RemoveRoute {
            route_id: "doomed".to_string(),
            command_id: "comp-c4".to_string(),
            causation_id: Some("comp-c3".to_string()),
        })
        .await
        .unwrap();

    // At 4 events now (>= threshold 3), the next append triggers compaction
    // which removes all events for routes that have a RouteRemoved.
    // Add a live route — this append should trigger compaction.
    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: camel_api::CanonicalRouteSpec::new("live-after-compact", "timer:tick"),
            command_id: "comp-c5".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();

    let replayed = journal.load_all().await.unwrap();

    // After compaction, all events for "doomed" (Registered, Started,
    // Stopped, Removed) must be gone.
    let doomed_events: Vec<_> = replayed
        .iter()
        .filter(|e| {
            let rid = match e {
                RuntimeEvent::RouteRegistered { route_id }
                | RuntimeEvent::RouteStartRequested { route_id }
                | RuntimeEvent::RouteStarted { route_id }
                | RuntimeEvent::RouteFailed { route_id, .. }
                | RuntimeEvent::RouteStopped { route_id }
                | RuntimeEvent::RouteSuspended { route_id }
                | RuntimeEvent::RouteResumed { route_id }
                | RuntimeEvent::RouteReloaded { route_id }
                | RuntimeEvent::RouteRemoved { route_id } => route_id.as_str(),
            };
            rid == "doomed"
        })
        .collect();
    assert!(
        doomed_events.is_empty(),
        "compacted route events must be removed, but found: {doomed_events:?}"
    );

    // The live route event must survive.
    let live_events: Vec<_> = replayed
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::RouteRegistered { route_id } if route_id == "live-after-compact"))
        .collect();
    assert_eq!(
        live_events.len(),
        1,
        "live route must survive compaction, got: {live_events:?}"
    );
}
