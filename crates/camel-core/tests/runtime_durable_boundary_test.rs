use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use camel_api::{
    CanonicalRouteSpec, RuntimeCommand, RuntimeCommandBus, RuntimeCommandResult,
};
use camel_core::{
    InMemoryRuntimeStore, JournalDurability, RedbJournalOptions, RedbRuntimeEventJournal,
    RuntimeBus, RuntimeEvent, RuntimeEventJournalPort,
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

// ── Fail-first wrapper ────────────────────────────────────────────────────────

#[derive(Clone)]
struct FailFirstAppendJournal {
    attempts: Arc<AtomicUsize>,
    inner: Arc<dyn RuntimeEventJournalPort>,
}

impl FailFirstAppendJournal {
    fn new(inner: Arc<dyn RuntimeEventJournalPort>) -> Self {
        Self {
            attempts: Arc::new(AtomicUsize::new(0)),
            inner,
        }
    }
}

#[async_trait]
impl RuntimeEventJournalPort for FailFirstAppendJournal {
    async fn append_batch(&self, events: &[RuntimeEvent]) -> Result<(), DomainError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            return Err(DomainError::InvalidState("forced first append failure".to_string()));
        }
        self.inner.append_batch(events).await
    }

    async fn load_all(&self) -> Result<Vec<RuntimeEvent>, DomainError> {
        self.inner.load_all().await
    }

    async fn append_command_id(&self, id: &str) -> Result<(), DomainError> {
        self.inner.append_command_id(id).await
    }

    async fn remove_command_id(&self, id: &str) -> Result<(), DomainError> {
        self.inner.remove_command_id(id).await
    }

    async fn load_command_ids(&self) -> Result<Vec<String>, DomainError> {
        self.inner.load_command_ids().await
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn accepted_command_is_duplicate_after_restart() {
    let dir = tempdir().unwrap();
    let journal = new_journal(dir.path().join("durable-restart.db")).await;

    let writer_store = InMemoryRuntimeStore::default().with_journal(journal.clone());
    let writer_runtime = RuntimeBus::new(
        Arc::new(writer_store.clone()),
        Arc::new(writer_store.clone()),
        Arc::new(writer_store.clone()),
        Arc::new(writer_store.clone()),
    )
    .with_uow(Arc::new(writer_store.clone()));

    let result = writer_runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("durable-r1", "timer:tick"),
            command_id: "dur-c1".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();
    assert!(
        matches!(result, RuntimeCommandResult::RouteRegistered { .. }),
        "first command must be accepted"
    );

    // Simulate restart: new store + runtime, same journal.
    let cold_store = InMemoryRuntimeStore::default().with_journal(journal.clone());
    let cold_runtime = RuntimeBus::new(
        Arc::new(cold_store.clone()),
        Arc::new(cold_store.clone()),
        Arc::new(cold_store.clone()),
        Arc::new(cold_store.clone()),
    )
    .with_uow(Arc::new(cold_store.clone()));

    let result = cold_runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("durable-r1", "timer:tick"),
            command_id: "dur-c1".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();
    assert!(
        matches!(result, RuntimeCommandResult::Duplicate { ref command_id } if command_id == "dur-c1"),
        "duplicate result after restart, got: {result:?}"
    );
}

#[tokio::test]
async fn failed_persist_does_not_consume_command_id() {
    let dir = tempdir().unwrap();
    let inner = new_journal(dir.path().join("durable-fail-first.db")).await
        as Arc<dyn RuntimeEventJournalPort>;
    let flaky = Arc::new(FailFirstAppendJournal::new(inner));

    let store = InMemoryRuntimeStore::default().with_journal(flaky);
    let runtime = RuntimeBus::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
    )
    .with_uow(Arc::new(store.clone()));

    // First attempt fails (journal error).
    let err = runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("durable-r2", "timer:tick"),
            command_id: "dur-c2".to_string(),
            causation_id: None,
        })
        .await
        .expect_err("first attempt must fail");
    assert!(
        err.to_string().contains("forced first append failure"),
        "unexpected error: {err}"
    );

    // Second attempt with the same command_id must succeed (not treated as duplicate).
    let result = runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("durable-r2", "timer:tick"),
            command_id: "dur-c2".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();
    assert!(
        matches!(result, RuntimeCommandResult::RouteRegistered { ref route_id } if route_id == "durable-r2"),
        "second attempt must be accepted, got: {result:?}"
    );
}
