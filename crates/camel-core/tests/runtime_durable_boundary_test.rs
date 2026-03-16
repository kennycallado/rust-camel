use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use camel_api::{
    CamelError, CanonicalRouteSpec, RuntimeCommand, RuntimeCommandBus, RuntimeCommandResult,
};
use camel_core::{
    FileRuntimeEventJournal, InMemoryRuntimeStore, RuntimeBus, RuntimeEvent,
    RuntimeEventJournalPort,
};
use tempfile::tempdir;

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
    async fn append_batch(&self, events: &[RuntimeEvent]) -> Result<(), CamelError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            return Err(CamelError::Io("forced first append failure".to_string()));
        }
        self.inner.append_batch(events).await
    }

    async fn load_all(&self) -> Result<Vec<RuntimeEvent>, CamelError> {
        self.inner.load_all().await
    }
}

#[tokio::test]
async fn accepted_command_is_duplicate_after_restart() {
    let dir = tempdir().unwrap();
    let journal =
        Arc::new(FileRuntimeEventJournal::new(dir.path().join("durable-restart.jsonl")).unwrap());

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
            spec: CanonicalRouteSpec::new("dur-r1", "timer:tick"),
            command_id: "dur-cmd-1".to_string(),
            causation_id: None,
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

    let result = cold_runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("dur-r1", "timer:tick"),
            command_id: "dur-cmd-1".to_string(),
            causation_id: None,
        })
        .await
        .expect("command should return duplicate instead of failing once dedup is durable");

    assert!(
        matches!(
            result,
            RuntimeCommandResult::Duplicate { ref command_id } if command_id == "dur-cmd-1"
        ),
        "expected duplicate result after restart, got: {result:?}"
    );
}

#[tokio::test]
async fn failed_persist_does_not_consume_command_id() {
    let dir = tempdir().unwrap();
    let inner = Arc::new(
        FileRuntimeEventJournal::new(dir.path().join("durable-fail-first.jsonl")).unwrap(),
    ) as Arc<dyn RuntimeEventJournalPort>;
    let flaky = Arc::new(FailFirstAppendJournal::new(inner));

    let store = InMemoryRuntimeStore::default().with_journal(flaky);
    let runtime = RuntimeBus::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
    )
    .with_uow(Arc::new(store.clone()));

    let first = runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("dur-r2", "timer:tick"),
            command_id: "dur-cmd-2".to_string(),
            causation_id: None,
        })
        .await;
    assert!(
        first.is_err(),
        "first execution must fail on forced append error"
    );

    let second = runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: CanonicalRouteSpec::new("dur-r2", "timer:tick"),
            command_id: "dur-cmd-2".to_string(),
            causation_id: None,
        })
        .await
        .expect("second execution should be accepted once persist succeeds");

    assert!(
        matches!(second, RuntimeCommandResult::RouteRegistered { ref route_id } if route_id == "dur-r2"),
        "expected successful retry with same command_id, got: {second:?}"
    );
}
