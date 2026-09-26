use std::sync::Arc;

use async_trait::async_trait;
use camel_api::{
    CamelError, RuntimeCommand, RuntimeCommandBus, RuntimeQuery, RuntimeQueryBus,
    RuntimeQueryResult,
};
use camel_core::lifecycle::domain::DomainError;
use camel_core::{
    CamelContext, InMemoryRuntimeStore, JournalDurability, ProjectionStorePort, RedbJournalOptions,
    RedbRuntimeEventJournal, RouteDefinition, RouteRepositoryPort, RouteRuntimeAggregate,
    RouteRuntimeState, RouteStatusProjection, RuntimeBus, RuntimeEvent, RuntimeEventJournalPort,
    RuntimeUnitOfWorkPort,
};
use tempfile::tempdir;

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
        Err(DomainError::InvalidState(
            "forced journal failure".to_string(),
        ))
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

// ── Boot-cycle battery (journstart2 task 1.3) ────────────────────────────────
//
// End-to-end proof for the journal-derived boot nonce: context command IDs
// are `context:{op}:{route_id}:{nonce}:{seq}` with the nonce derived from
// the recovered durable dedup store, so auto-startup routes start on every
// boot instead of being suppressed as duplicates of an earlier boot's
// recorded command IDs.

struct HoldConsumer;

#[async_trait]
impl camel_component_api::Consumer for HoldConsumer {
    async fn start(&mut self, ctx: camel_component_api::ConsumerContext) -> Result<(), CamelError> {
        ctx.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> camel_component_api::ConcurrencyModel {
        camel_component_api::ConcurrencyModel::Sequential
    }
}

struct HoldEndpoint;

impl camel_component_api::Endpoint for HoldEndpoint {
    fn uri(&self) -> &str {
        "hold:test"
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
        Ok(Box::new(HoldConsumer))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &camel_api::ProducerContext,
    ) -> Result<camel_api::BoxProcessor, CamelError> {
        Err(CamelError::RouteError("no producer".to_string()))
    }
}

struct HoldComponent;

impl camel_component_api::Component for HoldComponent {
    fn scheme(&self) -> &str {
        "hold"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn camel_component_api::Endpoint>, CamelError> {
        Ok(Box::new(HoldEndpoint))
    }
}

/// `(event name, route id)` for every `RuntimeEvent` variant, so the battery
/// can filter the journal stream per route and assert name order.
fn asserted_event(event: &RuntimeEvent) -> Option<(&'static str, &str)> {
    match event {
        RuntimeEvent::RouteRegistered { route_id } => Some(("RouteRegistered", route_id)),
        RuntimeEvent::RouteStartRequested { route_id } => Some(("RouteStartRequested", route_id)),
        RuntimeEvent::RouteStarted { route_id } => Some(("RouteStarted", route_id)),
        RuntimeEvent::RouteFailed { route_id, .. } => Some(("RouteFailed", route_id)),
        RuntimeEvent::RouteStopped { route_id } => Some(("RouteStopped", route_id)),
        RuntimeEvent::RouteSuspended { route_id } => Some(("RouteSuspended", route_id)),
        RuntimeEvent::RouteResumed { route_id } => Some(("RouteResumed", route_id)),
        RuntimeEvent::RouteReloaded { route_id } => Some(("RouteReloaded", route_id)),
        RuntimeEvent::RouteRemoved { route_id } => Some(("RouteRemoved", route_id)),
    }
}

/// Canonical per-boot journal sequence for one auto-startup route:
/// register (declarative) → start request → started, and the shutdown
/// stop's `RouteStopped` after it.
fn expected_boot_sequence(boots: u32) -> Vec<&'static str> {
    (0..boots)
        .flat_map(|_| {
            [
                "RouteRegistered",
                "RouteStartRequested",
                "RouteStarted",
                "RouteStopped",
            ]
        })
        .collect()
}

fn manual_bus(store: &InMemoryRuntimeStore) -> RuntimeBus {
    RuntimeBus::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
    )
    .with_uow(Arc::new(store.clone()))
}

#[tokio::test]
async fn auto_start_route_starts_on_every_boot_with_journal() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("boot-cycle.db");
    const ROUTE_ID: &str = "boot-cycle-r";

    for boot in 1..=3 {
        let journal = new_journal(path.clone()).await;
        let store = InMemoryRuntimeStore::default().with_journal(journal.clone());
        let mut ctx = CamelContext::builder()
            .runtime_store(store)
            .build()
            .await
            .unwrap();
        ctx.register_component(HoldComponent);
        ctx.add_route_definition(
            RouteDefinition::new("hold:test", Vec::new())
                .with_route_id(ROUTE_ID)
                .with_auto_startup(true),
        )
        .await
        .unwrap();

        ctx.start().await.unwrap();
        assert_eq!(
            ctx.runtime_route_status(ROUTE_ID).await.unwrap(),
            Some("Started".to_string()),
            "boot {boot}: auto-startup route must reach Started"
        );

        // Graceful shutdown issues a context StopRoute through the runtime
        // command bus; it must be accepted, not classified as a duplicate
        // of an earlier boot's recorded stop ID.
        ctx.stop().await.unwrap();

        // Per boot the journal gains exactly one
        // Registered → StartRequested → Started → Stopped sequence for the
        // route, appended after every earlier boot's sequences. On the
        // unfixed code (four-segment context command IDs) boot 2+'s
        // StartRoute was suppressed as a duplicate and this sequence never
        // appeared again.
        let route_events: Vec<&'static str> = journal
            .load_all()
            .await
            .unwrap()
            .iter()
            .filter_map(|event| {
                asserted_event(event)
                    .filter(|(_, route_id)| *route_id == ROUTE_ID)
                    .map(|(name, _)| name)
            })
            .collect();
        assert_eq!(
            route_events,
            expected_boot_sequence(boot),
            "boot {boot}: journal event sequence for '{ROUTE_ID}'"
        );

        // Reopen rule: drop EVERY live handle before the next boot re-opens
        // the journal, or the redb lock is still held.
        drop(ctx);
        drop(journal);
    }
}

#[tokio::test]
async fn crash_reboot_started_route_boots_and_starts() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("crash-reboot.db");
    const ROUTE_ID: &str = "crash-reboot-r";

    // Boot 1: start the auto-startup route, then crash — drop every handle
    // WITHOUT ctx.stop(), so the journal records the route as Started with
    // no RouteStopped. This is the state the suppression bug masked.
    {
        let journal = new_journal(path.clone()).await;
        let store = InMemoryRuntimeStore::default().with_journal(journal.clone());
        let mut ctx = CamelContext::builder()
            .runtime_store(store)
            .build()
            .await
            .unwrap();
        ctx.register_component(HoldComponent);
        ctx.add_route_definition(
            RouteDefinition::new("hold:test", Vec::new())
                .with_route_id(ROUTE_ID)
                .with_auto_startup(true),
        )
        .await
        .unwrap();

        ctx.start().await.unwrap();
        assert_eq!(
            ctx.runtime_route_status(ROUTE_ID).await.unwrap(),
            Some("Started".to_string()),
            "boot 1: auto-startup route must reach Started"
        );

        let route_events: Vec<&'static str> = journal
            .load_all()
            .await
            .unwrap()
            .iter()
            .filter_map(|event| {
                asserted_event(event)
                    .filter(|(_, route_id)| *route_id == ROUTE_ID)
                    .map(|(name, _)| name)
            })
            .collect();
        assert_eq!(
            route_events,
            ["RouteRegistered", "RouteStartRequested", "RouteStarted"],
            "boot 1 (crash): journal must end at Started without RouteStopped"
        );

        // Reopen rule: drop EVERY live handle before boot 2 re-opens the
        // journal, or the redb lock is still held.
        drop(ctx);
        drop(journal);
    }

    // Boot 2: rebuild over the SAME journal. The recovered aggregate is
    // Started (no RouteStopped); the boot-2 StartRoute must be issued for
    // real — accepted, not suppressed as a duplicate of boot 1's recorded
    // command ID.
    {
        let journal = new_journal(path.clone()).await;
        let store = InMemoryRuntimeStore::default().with_journal(journal.clone());
        let mut ctx = CamelContext::builder()
            .runtime_store(store)
            .build()
            .await
            .unwrap();
        ctx.register_component(HoldComponent);
        ctx.add_route_definition(
            RouteDefinition::new("hold:test", Vec::new())
                .with_route_id(ROUTE_ID)
                .with_auto_startup(true),
        )
        .await
        .unwrap();

        ctx.start().await.unwrap();
        assert_eq!(
            ctx.runtime_route_status(ROUTE_ID).await.unwrap(),
            Some("Started".to_string()),
            "boot 2: crash-recovered Started route must reach Started again"
        );

        // The boot-2 triplet is appended after boot 1's crash triplet: the
        // StartRoute was issued and journaled, not suppressed.
        let route_events: Vec<&'static str> = journal
            .load_all()
            .await
            .unwrap()
            .iter()
            .filter_map(|event| {
                asserted_event(event)
                    .filter(|(_, route_id)| *route_id == ROUTE_ID)
                    .map(|(name, _)| name)
            })
            .collect();
        assert_eq!(
            route_events,
            [
                "RouteRegistered",
                "RouteStartRequested",
                "RouteStarted",
                "RouteRegistered",
                "RouteStartRequested",
                "RouteStarted",
            ],
            "boot 2: start must be re-issued and journaled, not suppressed"
        );

        drop(ctx);
        drop(journal);
    }
}

#[tokio::test]
async fn boot_nonce_fails_closed_on_max_penultimate() {
    const ADVERSARIAL: &str = "context:start:r:18446744073709551615:0";
    let dir = tempdir().unwrap();
    let path = dir.path().join("max-penultimate.db");

    // Seed: record the adversarial ID durably via a bus-executed register.
    let journal = new_journal(path.clone()).await;
    let store = InMemoryRuntimeStore::default().with_journal(journal.clone());
    let runtime = manual_bus(&store);
    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: camel_api::CanonicalRouteSpec::new("r", "timer:tick"),
            command_id: ADVERSARIAL.to_string(),
            causation_id: None,
        })
        .await
        .unwrap();

    drop(runtime);
    drop(store);
    drop(journal);

    // Rebuild over the same journal and recover: the deterministic nonce
    // value space is exhausted, so derivation must fail closed naming the
    // offending recorded ID.
    let journal2 = new_journal(path.clone()).await;
    let store2 = InMemoryRuntimeStore::default().with_journal(journal2);
    store2.recover_from_journal().await.unwrap();
    let err = store2
        .recovered_boot_nonce()
        .await
        .expect_err("boot nonce derivation must fail closed at u64::MAX penultimate");
    let msg = err.to_string();
    assert!(
        msg.contains(ADVERSARIAL),
        "error must name the offending recorded ID, got: {msg}"
    );
    assert!(
        msg.contains("clean or rotate the journal"),
        "error must tell the operator to clean or rotate the journal, got: {msg}"
    );

    // The boot fails closed end-to-end: the recovery failure propagates at
    // every entry point, so no context command ID is issued for this boot.
    let runtime2 = manual_bus(&store2);
    let exec_err = runtime2
        .execute(RuntimeCommand::RegisterRoute {
            spec: camel_api::CanonicalRouteSpec::new("post-recovery", "timer:tick"),
            command_id: "post-recovery-cmd".to_string(),
            causation_id: None,
        })
        .await
        .expect_err("execute must fail closed when boot nonce derivation fails");
    let exec_msg = exec_err.to_string();
    assert!(
        exec_msg.contains(ADVERSARIAL),
        "execute error must carry the offending recorded ID, got: {exec_msg}"
    );
}

#[tokio::test]
async fn legacy_journal_boots_without_suppression() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("legacy-ids.db");
    const COLON_ROUTE: &str = "foo:0";

    // Seed a legacy journal: four-segment context command IDs, including
    // one for a route whose ID itself contains a colon, so the recorded
    // string's final segments are numerically ambiguous.
    let journal = new_journal(path.clone()).await;
    let store = InMemoryRuntimeStore::default().with_journal(journal.clone());
    let runtime = manual_bus(&store);
    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: camel_api::CanonicalRouteSpec::new(COLON_ROUTE, "timer:tick"),
            command_id: "context:start:foo:0:0".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();
    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: COLON_ROUTE.to_string(),
            command_id: "context:start:foo:0".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();
    runtime
        .execute(RuntimeCommand::StopRoute {
            route_id: COLON_ROUTE.to_string(),
            command_id: "context:stop:foo:0".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();
    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: camel_api::CanonicalRouteSpec::new("hello", "timer:tick"),
            command_id: "context:start:hello:0".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();

    drop(runtime);
    drop(store);
    drop(journal);

    // Rebuild: the derived nonce avoids every recorded penultimate. The
    // colon-route seed `context:start:foo:0:0` has penultimate 0, so the
    // nonce is never 0.
    let journal2 = new_journal(path.clone()).await;
    let store2 = InMemoryRuntimeStore::default().with_journal(journal2);
    store2.recover_from_journal().await.unwrap();
    let nonce = store2
        .recovered_boot_nonce()
        .await
        .expect("legacy recorded IDs must not exhaust the nonce space");
    assert!(
        nonce >= 1,
        "nonce must stay strictly above the colon-route legacy penultimate 0, got {nonce}"
    );
    drop(store2);

    // Full auto-start cycle over the legacy journal: the fresh context's
    // StartRoute for the colon route must be accepted, not suppressed.
    let journal3 = new_journal(path.clone()).await;
    let store3 = InMemoryRuntimeStore::default().with_journal(journal3);
    let mut ctx = CamelContext::builder()
        .runtime_store(store3)
        .build()
        .await
        .unwrap();
    ctx.register_component(HoldComponent);
    ctx.add_route_definition(
        RouteDefinition::new("hold:test", Vec::new())
            .with_route_id(COLON_ROUTE)
            .with_auto_startup(true),
    )
    .await
    .unwrap();
    ctx.start().await.unwrap();
    assert_eq!(
        ctx.runtime_route_status(COLON_ROUTE).await.unwrap(),
        Some("Started".to_string()),
        "legacy journal must not suppress auto-startup"
    );
}

#[tokio::test]
async fn boot_nonce_deterministic_across_recoveries() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("determinism.db");

    // Seed one tail-numeric recorded ID (penultimate segment 7).
    let journal = new_journal(path.clone()).await;
    let store = InMemoryRuntimeStore::default().with_journal(journal.clone());
    let runtime = manual_bus(&store);
    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: camel_api::CanonicalRouteSpec::new("d", "timer:tick"),
            command_id: "context:start:d:7:0".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();

    drop(runtime);
    drop(store);
    drop(journal);

    // Two sequential drop/rebuild/recover cycles over the unchanged journal
    // derive the same nonce — a pure function of the recorded IDs, no wall
    // clock.
    let first = {
        let journal = new_journal(path.clone()).await;
        let store = InMemoryRuntimeStore::default().with_journal(journal);
        store.recover_from_journal().await.unwrap();
        store.recovered_boot_nonce().await.unwrap()
    };

    let second = {
        let journal = new_journal(path.clone()).await;
        let store = InMemoryRuntimeStore::default().with_journal(journal);
        store.recover_from_journal().await.unwrap();
        store.recovered_boot_nonce().await.unwrap()
    };

    assert_eq!(
        first, second,
        "identical journal state must derive the identical nonce"
    );
    assert_eq!(
        first, 8,
        "nonce is 1 + the maximum recorded penultimate segment (7)"
    );
}

#[tokio::test]
async fn no_journal_boot_lifecycle_unchanged() {
    // No runtime journal configured: no recovery path runs and command IDs
    // gain only the constant zero nonce segment, so the auto-startup
    // lifecycle outcomes are exactly what they were before the change.
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(HoldComponent);
    ctx.add_route_definition(
        RouteDefinition::new("hold:test", Vec::new())
            .with_route_id("no-journal-r")
            .with_auto_startup(true),
    )
    .await
    .unwrap();

    ctx.start().await.unwrap();
    assert_eq!(
        ctx.runtime_route_status("no-journal-r").await.unwrap(),
        Some("Started".to_string()),
        "auto-startup must proceed identically without a journal"
    );

    ctx.stop().await.unwrap();
    assert_eq!(
        ctx.runtime_route_status("no-journal-r").await.unwrap(),
        Some("Stopped".to_string()),
        "stop lifecycle outcome must be unchanged without a journal"
    );
}
