//! End-to-end tests for the template hot-reload control plane:
//!
//! - `RuntimeBus::execute(ReloadTemplates)` intercepts BEFORE journal recovery
//!   and dedup (mirroring `ReloadTlsCerts`), dispatches to
//!   `TemplateReloadRegistry::global().reload_route(route_id)`, and returns
//!   `TemplatesReloaded { route_id }` on success.
//! - Because the intercept precedes dedup, repeated `ReloadTemplates` with the
//!   SAME `command_id` each invoke `reload_route` (dedup is bypassed).
//! - Because the intercept precedes journal recovery, no UoW is required.
//! - The intercept does NOT mutate `RouteStatus` and writes nothing to the
//!   journal.
//!
//! These tests verify the *intercept path*. The per-target reload mechanics
//! (build/validate/commit, all-or-nothing, serialization) are covered by the
//! unit tests in `camel-component-api/src/template_reload.rs`.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{
    MetricsCollector, RuntimeCommand, RuntimeCommandBus, RuntimeCommandResult, RuntimeQuery,
    RuntimeQueryBus, RuntimeQueryResult,
};
use camel_component_api::template_reload::{
    TemplateReloadRegistry, TemplateReloadStaged, TemplateReloadTarget,
};
use camel_core::{
    InMemoryCommandDedup, InMemoryEventPublisher, InMemoryProjectionStore, InMemoryRouteRepository,
    JournalDurability, RedbJournalOptions, RedbRuntimeEventJournal, RuntimeBus,
    RuntimeEventJournalPort,
};
use tempfile::tempdir;

// ── Fake reload target (commit-spy) ──────────────────────────────────────────

/// Concrete staged marker. `Box<FakeStaged>` coerces to `Box<dyn Any>`.
struct FakeStaged;
impl TemplateReloadStaged for FakeStaged {
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

/// Minimal reload target that always succeeds and records every `commit` in a
/// shared counter. Each successful `reload_route` produces exactly one commit,
/// so `commit_calls` is the reload-invocation count.
struct FakeTarget {
    route: String,
    commit_calls: Arc<AtomicUsize>,
    generation: AtomicU64,
}

impl FakeTarget {
    fn new(route: &str) -> Arc<Self> {
        Arc::new(Self {
            route: route.to_string(),
            commit_calls: Arc::new(AtomicUsize::new(0)),
            generation: AtomicU64::new(0),
        })
    }

    /// Coerce to the erased trait-object Arc that `register` expects. Bind to a
    /// concrete-typed local so CoerceUnsized fires at the return site.
    fn as_dyn(self: &Arc<Self>) -> Arc<dyn TemplateReloadTarget> {
        let concrete: Arc<Self> = Arc::clone(self);
        concrete
    }
}

#[async_trait]
impl TemplateReloadTarget for FakeTarget {
    fn route_id(&self) -> &str {
        &self.route
    }
    fn reload_timeout(&self) -> Duration {
        Duration::from_secs(5)
    }
    fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    async fn build(&self) -> Result<(Box<dyn TemplateReloadStaged>, u64), camel_api::CamelError> {
        // Return the current generation so the validate phase always passes.
        Ok((Box::new(FakeStaged), self.generation.load(Ordering::SeqCst)))
    }
    fn commit(&self, _staged: Box<dyn TemplateReloadStaged>) {
        self.commit_calls.fetch_add(1, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

// ── Test harness ─────────────────────────────────────────────────────────────

/// Minimal `RuntimeBus` with in-memory adapters and NO UoW. The intercept path
/// runs BEFORE journal/dedup, so no UoW is required for `ReloadTemplates`.
fn build_test_bus() -> RuntimeBus {
    RuntimeBus::new(
        Arc::new(InMemoryRouteRepository::default()),
        Arc::new(InMemoryProjectionStore::default()),
        Arc::new(InMemoryEventPublisher::default()),
        Arc::new(InMemoryCommandDedup::default()),
    )
}

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

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn reload_templates_bypasses_dedup() {
    // The ReloadTemplates intercept runs BEFORE the dedup check, so issuing the
    // same command_id three times MUST invoke reload_route three times. This
    // mirrors tls_reload_test.rs:bypasses_dedup: reloads are idempotent and not
    // journaled, so dedup is intentionally skipped.
    let route = "tpl-bypass-dedup";
    let target = FakeTarget::new(route);
    let commit_calls = Arc::clone(&target.commit_calls);
    let _guard = TemplateReloadRegistry::global().register(target.as_dyn());

    let bus = build_test_bus();
    for i in 0..3 {
        let _ = bus
            .execute(RuntimeCommand::ReloadTemplates {
                route_id: route.to_string(),
                command_id: "same-cmd-id".into(),
                causation_id: None,
            })
            .await
            .unwrap_or_else(|e| panic!("reload #{i} should succeed: {e}"));
    }

    assert_eq!(
        commit_calls.load(Ordering::SeqCst),
        3,
        "ReloadTemplates must bypass dedup and invoke reload_route every time"
    );
    // _guard dropped on scope exit → target unregistered.
}

#[tokio::test]
async fn reload_templates_does_not_require_journal() {
    // No UoW attached — if the intercept didn't run, the bus would fall through
    // to execute_command and hit the safety-net Config error (or a journal
    // recovery error). With the intercept in place, the call succeeds.
    let route = "tpl-no-journal";
    let target = FakeTarget::new(route);
    let _guard = TemplateReloadRegistry::global().register(target.as_dyn());

    let bus = build_test_bus();
    let result = bus
        .execute(RuntimeCommand::ReloadTemplates {
            route_id: route.to_string(),
            command_id: "cmd-no-uow".into(),
            causation_id: None,
        })
        .await;

    assert!(
        result.is_ok(),
        "intercept must bypass UoW/journal: {result:?}"
    );
    let result = result.unwrap();
    assert!(
        matches!(result, RuntimeCommandResult::TemplatesReloaded { ref route_id } if route_id == route),
        "expected TemplatesReloaded, got {result:?}"
    );
}

#[tokio::test]
async fn reload_templates_route_status_unchanged() {
    // A started route's status MUST NOT change across a ReloadTemplates, and
    // the journal MUST receive zero new events (the intercept does not persist
    // lifecycle intent). Uses the redb journal harness from
    // runtime_journal_test.rs to observe real journal appends.
    let route = "tpl-status-unchanged";
    let dir = tempdir().unwrap();
    let journal = new_journal(dir.path().join("tpl-status.db")).await;
    let store = camel_core::InMemoryRuntimeStore::default().with_journal(journal.clone());

    let runtime = RuntimeBus::new(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        Arc::new(store.clone()),
    )
    .with_uow(Arc::new(store.clone()));

    // Register + Start → journal holds 3 events (Registered, StartRequested,
    // Started); projection status == "Started".
    runtime
        .execute(RuntimeCommand::RegisterRoute {
            spec: camel_api::CanonicalRouteSpec::new(route, "timer:tick"),
            command_id: "tpl-status-c1".to_string(),
            causation_id: None,
        })
        .await
        .unwrap();
    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: route.to_string(),
            command_id: "tpl-status-c2".to_string(),
            causation_id: Some("tpl-status-c1".to_string()),
        })
        .await
        .unwrap();

    let events_before = journal.load_all().await.unwrap().len();

    let status_before = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: route.to_string(),
        })
        .await
        .unwrap();
    assert_eq!(
        status_before,
        RuntimeQueryResult::RouteStatus {
            route_id: route.to_string(),
            status: "Started".to_string(),
        }
    );

    // Register a reload target so the intercept has something to dispatch to.
    let target = FakeTarget::new(route);
    let _guard = TemplateReloadRegistry::global().register(target.as_dyn());

    let reload_result = runtime
        .execute(RuntimeCommand::ReloadTemplates {
            route_id: route.to_string(),
            command_id: "tpl-status-c3".to_string(),
            causation_id: None,
        })
        .await;
    assert!(
        reload_result.is_ok(),
        "ReloadTemplates should succeed: {reload_result:?}"
    );

    // Status unchanged.
    let status_after = runtime
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: route.to_string(),
        })
        .await
        .unwrap();
    assert_eq!(
        status_after, status_before,
        "RouteStatus must be unchanged by ReloadTemplates"
    );

    // Zero journal writes.
    let events_after = journal.load_all().await.unwrap().len();
    assert_eq!(
        events_after, events_before,
        "ReloadTemplates must not append journal events (before={events_before}, after={events_after})"
    );
    // _guard dropped on scope end → target unregistered.
}

// ── rc-d3pj: metrics counter verification ────────────────────────────────────

/// Mock MetricsCollector that records `record_counter` calls.
struct RecordingMetrics {
    counters: std::sync::Mutex<Vec<CounterRecord>>,
}

#[derive(Clone, Debug)]
struct CounterRecord {
    name: String,
    value: f64,
    labels: Vec<(String, String)>,
}

impl MetricsCollector for RecordingMetrics {
    fn record_exchange_duration(&self, _: &str, _: Duration) {}
    fn increment_errors(&self, _: &str, _: &str) {}
    fn increment_exchanges(&self, _: &str) {}
    fn set_queue_depth(&self, _: &str, _: usize) {}
    fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
    fn record_counter(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        self.counters.lock().unwrap().push(CounterRecord {
            name: name.to_string(),
            value,
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        });
    }
}

/// rc-d3pj: ReloadTemplates must record `template_reloads_total` once per
/// successful reload when a metrics handle is threaded into RuntimeBus.
#[tokio::test]
async fn reload_templates_records_counter() {
    let route = "tpl-metrics-counter";
    let target = FakeTarget::new(route);
    let _guard = TemplateReloadRegistry::global().register(target.as_dyn());

    let metrics = Arc::new(RecordingMetrics {
        counters: std::sync::Mutex::new(Vec::new()),
    });

    let bus = RuntimeBus::new(
        Arc::new(InMemoryRouteRepository::default()),
        Arc::new(InMemoryProjectionStore::default()),
        Arc::new(InMemoryEventPublisher::default()),
        Arc::new(InMemoryCommandDedup::default()),
    )
    .with_metrics(Arc::clone(&metrics) as Arc<dyn MetricsCollector>);

    let result = bus
        .execute(RuntimeCommand::ReloadTemplates {
            route_id: route.to_string(),
            command_id: "tpl-metrics-1".to_string(),
            causation_id: None,
        })
        .await;
    assert!(result.is_ok(), "reload should succeed: {result:?}");

    let recorded = metrics
        .counters
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        recorded.len(),
        1,
        "exactly one counter must be recorded: {recorded:?}"
    );
    assert_eq!(recorded[0].name, "template_reloads_total");
    assert_eq!(recorded[0].value, 1.0);
    assert_eq!(
        recorded[0].labels,
        vec![("route_id".to_string(), route.to_string())],
        "label must include route_id: {:?}",
        recorded[0].labels
    );
}
