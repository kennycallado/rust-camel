use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::config::rabbitmq_reconnect_default;

fn lapin_err(message: &str) -> lapin::Error {
    lapin::Error::from(io::Error::new(
        io::ErrorKind::ConnectionRefused,
        message.to_string(),
    ))
}

/// Counting acker for the disposition-under-lock regression. It implements
/// the engine's crate-private `DeliveryAcker` (no public test hook).
#[derive(Default)]
struct RecordingAcker {
    acks: AtomicUsize,
}

impl RecordingAcker {
    fn ack_count(&self) -> usize {
        self.acks.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl crate::consumer::DeliveryAcker for RecordingAcker {
    async fn ack(&self) -> Result<(), CamelError> {
        self.acks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn nack(&self, _requeue: bool) -> Result<(), CamelError> {
        Ok(())
    }
}

/// Records when the pending connect future is dropped and signals a
/// [`Notify`], so a test can await the drop deterministically instead of
/// settling with a sleep (a paused clock does not waive the lint scanner).
struct DropFlag {
    dropped: Arc<AtomicBool>,
    signal: Arc<tokio::sync::Notify>,
}

impl DropFlag {
    fn new(dropped: Arc<AtomicBool>, signal: Arc<tokio::sync::Notify>) -> Self {
        Self { dropped, signal }
    }
}

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
        self.signal.notify_one();
    }
}

/// Test lifecycle context: hands out a swappable parent shutdown token
/// exactly like a slot-bound `RegistryComponentContext` (a fresh token on
/// every runtime start). No route-scoped token is involved: the only
/// cancel source under test is the runtime parent.
struct FakeLifecycleContext {
    parent: std::sync::Mutex<CancellationToken>,
}

impl FakeLifecycleContext {
    fn new(parent: CancellationToken) -> Self {
        Self {
            parent: std::sync::Mutex::new(parent),
        }
    }

    /// Simulate a runtime stop/start: the next `shutdown_token()`
    /// resolution observes `parent`.
    fn swap(&self, parent: CancellationToken) {
        *self
            .parent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = parent;
    }
}

impl ComponentContext for FakeLifecycleContext {
    fn resolve_component(&self, _scheme: &str) -> Option<Arc<dyn camel_component_api::Component>> {
        None
    }

    fn resolve_language(&self, _name: &str) -> Option<Arc<dyn camel_language_api::Language>> {
        None
    }

    fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
        Arc::new(camel_api::NoOpMetrics)
    }

    fn platform_service(&self) -> Arc<dyn camel_api::PlatformService> {
        Arc::new(camel_api::NoopPlatformService::default())
    }

    fn register_route_health_check(
        &self,
        _route_id: &str,
        _check: Arc<dyn camel_api::AsyncHealthCheck>,
    ) {
    }

    fn unregister_route_health_check(&self, _route_id: &str) {}

    fn shutdown_token(&self) -> Option<CancellationToken> {
        Some(
            self.parent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
    }
}

#[tokio::test]
async fn retry_policy_bounds_connect_attempts() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&attempts);
    let connect_fn: ConnectFn = Arc::new(move |_url: &str| {
        counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(lapin_err("connect refused")) })
    });
    let manager = Arc::new(RabbitConnectionManager::new(
        "amqp://localhost:5672",
        NetworkRetryPolicy {
            enabled: true,
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            ..NetworkRetryPolicy::default()
        },
        connect_fn,
    ));

    let result = manager.connect().await;

    assert!(result.is_err(), "exhausted retries must return Err");
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "max_attempts=2 must yield exactly 2 connect attempts"
    );
}

#[tokio::test]
async fn note_failure_spawns_single_reconnect() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let counter = Arc::clone(&attempts);
    let started_fn = Arc::clone(&started);
    let connect_fn: ConnectFn = Arc::new(move |_url: &str| {
        counter.fetch_add(1, Ordering::SeqCst);
        let started = Arc::clone(&started_fn);
        Box::pin(async move {
            started.notify_one();
            std::future::pending::<Result<lapin::Connection, lapin::Error>>().await
        })
    });
    let manager = Arc::new(RabbitConnectionManager::new(
        "amqp://localhost:5672",
        rabbitmq_reconnect_default(),
        connect_fn,
    ));

    manager.note_failure();
    manager.note_failure();
    started.notified().await;
    tokio::task::yield_now().await;

    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "single-flight must spawn exactly one reconnect attempt"
    );
}

#[tokio::test(start_paused = true)]
async fn connection_within_bounded_when_unreachable() {
    let connect_fn: ConnectFn = Arc::new(|_url: &str| {
        Box::pin(async { std::future::pending::<Result<lapin::Connection, lapin::Error>>().await })
    });
    let manager = Arc::new(RabbitConnectionManager::new(
        "amqp://localhost:5672",
        rabbitmq_reconnect_default(),
        connect_fn,
    ));

    let start = tokio::time::Instant::now();
    let result = manager.connection_within(Duration::from_millis(100)).await;

    assert!(
        result.is_err(),
        "unreachable broker must not yield a connection"
    );
    assert!(
        start.elapsed() <= Duration::from_millis(200),
        "bounded wait must return quickly, took {:?}",
        start.elapsed()
    );
}

#[tokio::test(start_paused = true)]
async fn dropping_last_manager_arc_cancels_pending_reconnect() {
    let started = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let dropped_signal = Arc::new(tokio::sync::Notify::new());
    let started_fn = Arc::clone(&started);
    let dropped_fn = Arc::clone(&dropped);
    let dropped_signal_fn = Arc::clone(&dropped_signal);
    let connect_fn: ConnectFn = Arc::new(move |_url: &str| {
        let started = Arc::clone(&started_fn);
        let dropped = Arc::clone(&dropped_fn);
        let dropped_signal = Arc::clone(&dropped_signal_fn);
        Box::pin(async move {
            started.notify_one();
            let _guard = DropFlag::new(dropped, dropped_signal);
            std::future::pending::<Result<lapin::Connection, lapin::Error>>().await
        })
    });
    let manager = Arc::new(RabbitConnectionManager::new(
        "amqp://localhost:5672",
        rabbitmq_reconnect_default(),
        connect_fn,
    ));

    manager.ensure_connecting();
    started.notified().await;
    let weak = Arc::downgrade(&manager);
    let tasks = Arc::clone(&manager.tasks);
    drop(manager);
    // Await the drop signal instead of a settling sleep: Drop aborts the
    // tracked attempt, and its `DropFlag` drop notifies exactly once.
    dropped_signal.notified().await;

    assert!(
        dropped.load(Ordering::SeqCst),
        "dropping the last Arc must cancel and drop the pending connect attempt"
    );
    assert!(
        weak.upgrade().is_none(),
        "no strong manager reference may survive the retry"
    );
    // No lifecycle context is bound (`RabbitConnectionManager::new`), so
    // the manager must NOT mint a token: it drops the pending attempt by
    // aborting its tracked tasks. An emptied registry proves `abort_all`
    // ran on Drop (master-amended derived cancellation remedy).
    assert!(
        tasks
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "the no-context path must abort and drain its tracked tasks on Drop"
    );
}

/// Master-amended task 1.3 (derived cancellation remedy): the manager
/// derives a manager-local child of the CURRENT runtime shutdown token at
/// each reconnect activation, so cancelling the runtime parent stops a
/// pending attempt. The parent is supplied through a lifecycle context
/// exactly like a slot-bound `RegistryComponentContext`; no route-scoped
/// token exists. After the cancel the flight must settle to
/// `Disconnected` (never strand `Connecting`), so a fresh parent in the
/// same slot (runtime restart) admits a new attempt.
#[tokio::test]
async fn runtime_parent_cancel_stops_pending_reconnect() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let counter = Arc::clone(&attempts);
    let started_fn = Arc::clone(&started);
    let dropped_fn = Arc::clone(&dropped);
    let connect_fn: ConnectFn = Arc::new(move |_url: &str| {
        counter.fetch_add(1, Ordering::SeqCst);
        let started = Arc::clone(&started_fn);
        let dropped = Arc::clone(&dropped_fn);
        Box::pin(async move {
            started.notify_one();
            // This test observes the drop through the status watch, so the
            // per-attempt signal is unused and created inline.
            let _guard = DropFlag::new(dropped, Arc::new(tokio::sync::Notify::new()));
            std::future::pending::<Result<lapin::Connection, lapin::Error>>().await
        })
    });

    // Runtime parent root, created inside the test (lint-exempt) and
    // supplied through the lifecycle context. There is no route-scoped
    // token here: the runtime parent is the only cancel source.
    let parent = CancellationToken::new();
    let context = Arc::new(FakeLifecycleContext::new(parent.clone()));
    let context_dyn: Arc<dyn ComponentContext> = context.clone();
    let manager = Arc::new(
        RabbitConnectionManager::new(
            "amqp://localhost:5672",
            rabbitmq_reconnect_default(),
            connect_fn,
        )
        .with_lifecycle_context(context_dyn),
    );

    manager.ensure_connecting();
    started.notified().await;
    assert!(
        !dropped.load(Ordering::SeqCst),
        "the pending attempt must stay alive before the runtime parent cancels"
    );

    // Cancelling the runtime parent must drop the pending attempt through
    // the derived manager-local child token.
    let mut status_rx = manager.status_tx.subscribe();
    parent.cancel();
    // Wait for the cancelled flight to settle `Disconnected` (never strand
    // `Connecting`) via the watch change, not a settling sleep. The status
    // is published after the attempt future (and its `DropFlag`) is dropped.
    while *status_rx.borrow() != ConnStatus::Disconnected {
        status_rx
            .changed()
            .await
            .expect("status watch must stay open");
    }
    assert!(
        dropped.load(Ordering::SeqCst),
        "cancelling the runtime parent must drop the pending connect attempt"
    );
    assert_eq!(
        *status_rx.borrow(),
        ConnStatus::Disconnected,
        "a cancelled flight must settle to Disconnected, never strand Connecting"
    );

    // Runtime restart: the same slot resolves a fresh, uncancelled parent,
    // so the settled flight must admit a new attempt.
    let fresh = CancellationToken::new();
    context.swap(fresh.clone());
    manager.ensure_connecting();
    started.notified().await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "a cancelled flight must admit a new attempt after a runtime restart"
    );

    fresh.cancel();
    drop(manager);
}

/// Master-fix regression (derived cancellation remedy): a runtime parent
/// cancel observed by the live-connection error listener must demote the
/// connection (clear the slot, settle `Disconnected`) WITHOUT re-arming a
/// doomed reconnect against the already-cancelled parent. The demotion is
/// driven through the state seam (a live `lapin::Connection` cannot be
/// constructed off-broker), then a fresh parent in the same slot admits a
/// new attempt.
#[tokio::test]
async fn runtime_parent_cancel_demotes_live_connection_and_rearms() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let counter = Arc::clone(&attempts);
    let started_fn = Arc::clone(&started);
    let connect_fn: ConnectFn = Arc::new(move |_url: &str| {
        counter.fetch_add(1, Ordering::SeqCst);
        let started = Arc::clone(&started_fn);
        Box::pin(async move {
            started.notify_one();
            std::future::pending::<Result<lapin::Connection, lapin::Error>>().await
        })
    });

    let parent = CancellationToken::new();
    let context = Arc::new(FakeLifecycleContext::new(parent.clone()));
    let context_dyn: Arc<dyn ComponentContext> = context.clone();
    let manager = Arc::new(
        RabbitConnectionManager::new(
            "amqp://localhost:5672",
            rabbitmq_reconnect_default(),
            connect_fn,
        )
        .with_lifecycle_context(context_dyn),
    );

    // Simulate a live connection's watched state through the seam.
    manager.seed_published_state(1, ConnStatus::Connected).await;

    // The runtime parent cancels: the listener demotes the live connection
    // and must NOT re-arm against the cancelled parent.
    assert!(
        manager.demote_connection_for_generation(1),
        "a Connected manager must own the demotion transition"
    );
    assert_eq!(
        *manager.status_tx.borrow(),
        ConnStatus::Disconnected,
        "a parent cancel must settle the live connection to Disconnected"
    );
    assert!(
        !manager.demote_connection_for_generation(1),
        "demotion must be idempotent (no repeated claim / respawn loop)"
    );
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        0,
        "demotion must not spawn a reconnect against the cancelled parent"
    );

    // Runtime restart: the same slot resolves a fresh parent, so the next
    // activation re-arms and admits a new attempt.
    let fresh = CancellationToken::new();
    context.swap(fresh.clone());
    manager.ensure_connecting();
    started.notified().await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "a fresh parent must admit a reconnect after the parent-cancel demotion"
    );

    fresh.cancel();
    drop(manager);
}

/// A caller that abandons `connect()` while the connect attempt is still
/// pending must not cancel the single-flight owner: the detached task keeps
/// running, `Connecting` eventually exits, and the slot admits a next
/// attempt. Regression for reviewer finding (a), master-amended task 1.3.
#[tokio::test]
async fn caller_abandoned_connect_does_not_wedge_single_flight() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let counter = Arc::clone(&attempts);
    let started_fn = Arc::clone(&started);
    let release_fn = Arc::clone(&release);
    let connect_fn: ConnectFn = Arc::new(move |_url: &str| {
        counter.fetch_add(1, Ordering::SeqCst);
        let started = Arc::clone(&started_fn);
        let release = Arc::clone(&release_fn);
        Box::pin(async move {
            started.notify_one();
            release.notified().await;
            Err(lapin_err("connect refused"))
        })
    });
    let manager = Arc::new(RabbitConnectionManager::new(
        "amqp://localhost:5672",
        NetworkRetryPolicy {
            enabled: true,
            max_attempts: 1,
            initial_delay: Duration::from_millis(1),
            ..NetworkRetryPolicy::default()
        },
        connect_fn,
    ));

    // Spawn a `connect()` caller, wait until the attempt is provably
    // in-flight, then abandon the caller.
    let caller = tokio::spawn({
        let manager = Arc::clone(&manager);
        async move { manager.connect().await }
    });
    started.notified().await;
    assert_eq!(
        *manager.status_tx.borrow(),
        ConnStatus::Connecting,
        "the claim must publish Connecting before the attempt starts"
    );
    caller.abort();
    let _ = caller.await;

    // The detached flight still owns the transition: a fresh bounded wait
    // observes Connecting, not a permanently wedged slot.
    let waiter = tokio::spawn({
        let manager = Arc::clone(&manager);
        async move { manager.connection_within(Duration::from_millis(50)).await }
    });
    tokio::task::yield_now().await;
    assert_eq!(
        *manager.status_tx.borrow(),
        ConnStatus::Connecting,
        "abandoning the caller must not cancel the single-flight owner"
    );

    // Let the pending attempt fail; the owned flight must publish the
    // terminal Disconnected state and settle the waiter.
    release.notify_one();
    let waited = waiter.await.expect("waiter task must not panic");
    assert!(waited.is_err(), "a failed single-flight settles as Err");
    assert_eq!(
        *manager.status_tx.borrow(),
        ConnStatus::Disconnected,
        "the owner must exit Connecting after the attempt fails"
    );

    // The slot is free again: a next attempt is admitted.
    manager.ensure_connecting();
    started.notified().await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "a settled single-flight must admit a next attempt"
    );
    release.notify_one();
    tokio::task::yield_now().await;
}

/// A stale failure clear must not erase a connection published by a newer
/// generation, and an in-window failure must not claim a slot a
/// `Connecting` flight owns. The state seam publishes generations without
/// inventing a `lapin::Connection` (which cannot be constructed off a
/// broker). Regression for reviewer findings (b) and master final-fix
/// finding 1 (store-then-publish window), master-amended task 1.3.
#[tokio::test]
async fn deferred_failure_clear_does_not_erase_new_generation() {
    let connect_fn: ConnectFn = Arc::new(|_url: &str| {
        Box::pin(async { std::future::pending::<Result<lapin::Connection, lapin::Error>>().await })
    });
    let manager = Arc::new(RabbitConnectionManager::new(
        "amqp://localhost:5672",
        rabbitmq_reconnect_default(),
        connect_fn,
    ));

    // A reconnect published a NEWER generation (state seam: bump the
    // generation and mark the slot Connected; no fake Connection needed).
    manager.seed_published_state(1, ConnStatus::Connected).await;

    // A deferred clear captured generation 0; applying it must decline and
    // leave generation 1 / Connected intact.
    let cleared = manager.apply_failure_clear(0).await;
    assert!(
        !cleared,
        "a stale failure clear must not erase a newer generation"
    );
    assert_eq!(
        manager.inner.read().await.generation,
        1,
        "the newer generation must survive the stale clear"
    );
    assert_eq!(
        *manager.status_tx.borrow(),
        ConnStatus::Connected,
        "the newer connection's live status must survive the stale clear"
    );

    // The newer generation's own failure may still clear the slot.
    assert!(
        manager.apply_failure_clear(1).await,
        "the current generation's clear must still apply"
    );

    // Store-then-publish window: `run_reconnect` has stored the fresh
    // generation (2) but has not yet broadcast Connected. A failure here
    // must not claim the slot — the Connecting flight owns it, and a claim
    // would clear the connection it is about to publish.
    manager
        .seed_published_state(2, ConnStatus::Connecting)
        .await;
    assert!(
        !manager.demote_connection_for_generation(2),
        "a Connecting flight owns the slot; an in-window failure must not claim a clear"
    );
    manager.note_failure();
    assert_eq!(
        *manager.status_tx.borrow(),
        ConnStatus::Connecting,
        "an in-window failure must leave the Connecting flight in charge"
    );
    assert_eq!(
        manager.inner.read().await.generation,
        2,
        "the in-window failure must not replace or erase the fresh generation"
    );
}

/// Reviewer Race 1 regression: a failure reported for a connection
/// generation that has already been replaced must not demote the successor.
/// The old unqualified `note_failure` captured the CURRENT generation, so a
/// late old-generation failure erased g+1, flipped the live status to
/// `Disconnected`, and started a replacement reconnect.
#[tokio::test]
async fn late_old_generation_failure_preserves_new_connection() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&attempts);
    let connect_fn: ConnectFn = Arc::new(move |_url: &str| {
        counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { std::future::pending::<Result<lapin::Connection, lapin::Error>>().await })
    });
    let manager = Arc::new(RabbitConnectionManager::new(
        "amqp://localhost:5672",
        rabbitmq_reconnect_default(),
        connect_fn,
    ));

    // Origin connection published as generation 1, then replaced by a
    // reconnected generation 2 (both `Connected`).
    manager.seed_published_state(1, ConnStatus::Connected).await;
    manager.seed_published_state(2, ConnStatus::Connected).await;

    // A late failure for the OLD generation 1 must be ignored entirely.
    manager.note_failure_for_generation(1);

    assert_eq!(
        manager.current_generation(),
        2,
        "the successor generation must survive a late old-generation failure"
    );
    assert_eq!(
        *manager.status_tx.borrow(),
        ConnStatus::Connected,
        "a late old-generation failure must not demote the successor status"
    );
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        0,
        "a late old-generation failure must not start a replacement reconnect"
    );
    assert_eq!(
        manager.inner.read().await.generation,
        2,
        "the successor's published state must not be cleared"
    );
}

/// Reviewer boundary-finding regression (P2 continuation): a channel-open
/// failure from an OLD attempt must be reported against the CAPTURED origin
/// connection, never the generation read after the await.
///
/// Deterministic model of the race: the old attempt captures origin generation
/// 1 while `Connected`; a reconnect publishes successor generation 2 during the
/// blocked-in-flight window; the old attempt then fails and reports through the
/// production error handler with origin 1 / dead. The successor must survive
/// (generation 2, status `Connected`, no replacement reconnect). The pre-fix
/// caller read `current_generation()` after the await, re-labelling the old
/// failure as the successor generation and tearing down the live connection.
#[tokio::test]
async fn late_old_channel_open_failure_preserves_new_generation() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&attempts);
    let connect_fn: ConnectFn = Arc::new(move |_url: &str| {
        counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { std::future::pending::<Result<lapin::Connection, lapin::Error>>().await })
    });
    let manager = Arc::new(RabbitConnectionManager::new(
        "amqp://localhost:5672",
        rabbitmq_reconnect_default(),
        connect_fn,
    ));

    // Origin connection published as generation 1. The old channel-open attempt
    // captures its origin identity BEFORE its `channel.open` await.
    manager.seed_published_state(1, ConnStatus::Connected).await;
    let origin_generation = manager.current_generation();
    let origin_connected = true;

    // The attempt is still blocked when a reconnect publishes successor
    // generation 2. The captured origin (1) is fixed before the successor (2)
    // exists — the in-flight window this regression guards.
    manager.seed_published_state(2, ConnStatus::Connected).await;

    // The old attempt returns dead and reports through the production error
    // handler using its CAPTURED origin 1, not the current generation 2.
    manager.handle_channel_open_failure(origin_generation, !origin_connected);

    assert_eq!(
        manager.current_generation(),
        2,
        "a late old channel-open failure must not relabel itself as the successor generation"
    );
    assert_eq!(
        *manager.status_tx.borrow(),
        ConnStatus::Connected,
        "a late old channel-open failure must not demote the live successor"
    );
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        0,
        "a late old channel-open failure must not start a replacement reconnect"
    );
    assert_eq!(
        manager.inner.read().await.generation,
        2,
        "the successor's published state must not be cleared"
    );
}

/// Reviewer Race 2 regression: `current_generation()` must stay
/// authoritative while a reconnect writer holds the state lock. The old
/// `try_read` accessor returned 0 on a busy lock, so a valid disposition on
/// a healthy live connection was judged stale and dropped forever.
#[tokio::test]
async fn current_generation_remains_authoritative_during_state_write_lock() {
    let connect_fn: ConnectFn = Arc::new(|_url: &str| {
        Box::pin(async { std::future::pending::<Result<lapin::Connection, lapin::Error>>().await })
    });
    let manager = Arc::new(RabbitConnectionManager::new(
        "amqp://localhost:5672",
        rabbitmq_reconnect_default(),
        connect_fn,
    ));
    manager.seed_published_state(1, ConnStatus::Connected).await;

    // Hold the state write guard: the old `try_read` accessor would report
    // 0 here.
    let guard = manager.inner.write().await;
    let generation = manager.current_generation();
    assert_eq!(
        generation, 1,
        "the canonical generation must not be masked by lock contention"
    );

    // A valid disposition on the live generation must ack, not drop.
    let acker = RecordingAcker::default();
    crate::consumer::apply_disposition(
        crate::consumer::Disposition::Ack,
        manager.current_generation() == generation,
        &acker,
    )
    .await
    .expect("apply_disposition must not error");
    assert_eq!(
        acker.ack_count(),
        1,
        "a valid ack must not be dropped while a state write is in flight"
    );
    drop(guard);
}

#[test]
fn retry_policy_defaults_match_jms() {
    let connect_fn: ConnectFn = Arc::new(|_url: &str| {
        Box::pin(async { std::future::pending::<Result<lapin::Connection, lapin::Error>>().await })
    });
    let manager = RabbitConnectionManager::new(
        "amqp://localhost:5672",
        rabbitmq_reconnect_default(),
        connect_fn,
    );

    assert_eq!(manager.retry.max_attempts, 0, "unlimited attempts");
    assert_eq!(manager.retry.initial_delay, Duration::from_secs(5));
    assert_eq!(manager.retry.max_delay, Duration::from_secs(30));
}

/// Master-authorized task 1.4 regression (real docker broker): a
/// channel-scoped publish failure (404 NOT_FOUND on a missing exchange)
/// must recreate ONLY the producer channel. The shared connection stays
/// healthy — same `Arc` identity, same generation, `Connected` status —
/// and the next publish reuses it and round-trips. A full reconnect here
/// would replace the connection and bump the generation (the red-first
/// failure mode under the pre-amendment `mark_channel_dead`, which called
/// `note_failure` for every publish error).
///
/// The bounded status watch also locks the connection error listener: a
/// soft AMQP error reaches `Connection::events_listener()` as
/// `Event::Error`, so an unclassified listener would demote the live
/// connection even though the producer no longer does.
#[tokio::test]
async fn channel_publish_failure_preserves_connection_and_next_publish_succeeds() {
    use crate::config::RabbitEndpointConfig;
    use crate::producer::RabbitProducer;
    use camel_api::{Body, Exchange, Message};
    use camel_component_api::RuntimeObservability;
    use camel_component_api::test_support::RecordingRuntimeObservability;
    use lapin::options::BasicGetOptions;
    use lapin::types::ShortString;
    use tower::Service;

    let Some(fx) = docker_fixture::require_fixture() else {
        return;
    };
    let queue = format!(
        "chan-isolation-{}-{}",
        std::process::id(),
        docker_fixture::nanos()
    );
    // Raw fixture connection+channel: declare the queue and read the
    // message back (`_raw_conn` stays bound for the whole test).
    let (_raw_conn, raw_channel) = fx.declare_queue(&queue).await;

    let broker = RabbitBrokerConfig {
        url: fx.amqp_url.clone(),
        username: None,
        password: None,
        vhost: None,
    };
    let manager = Arc::new(RabbitConnectionManager::from_broker_config(
        &broker,
        rabbitmq_reconnect_default(),
    ));

    // Establish the healthy connection and capture its identity.
    let (conn_before, gen_before) = manager
        .connection_within(PUBLISH_DISCONNECTED_BOUND)
        .await
        .expect("initial connect to the fixture must succeed");

    // Subscribe before the failing publish so a listener-driven demotion is
    // directly observable.
    let mut status_rx = manager.status_tx.subscribe();

    let rt: Arc<dyn RuntimeObservability> = RecordingRuntimeObservability::new(true);

    // Publish to a non-existent exchange: the broker closes the channel
    // with a soft 404, surfaced through the confirm wait.
    let ghost_cfg = RabbitEndpointConfig::from_uri(&format!(
        "rabbitmq:ghost.{}?routingKey=k",
        docker_fixture::nanos()
    ))
    .expect("ghost URI parses");
    let mut ghost_producer = RabbitProducer::new(ghost_cfg, Arc::clone(&manager), Arc::clone(&rt));
    let ghost_result = ghost_producer.call(Exchange::default()).await;
    assert!(
        ghost_result.is_err(),
        "publishing to a missing exchange must fail"
    );

    // The channel-scoped 404 must not demote the live connection: the
    // listener processes the connection's soft-error event well within this
    // bound, and no status change may follow.
    assert!(
        tokio::time::timeout(Duration::from_millis(250), status_rx.changed())
            .await
            .is_err(),
        "a channel-scoped 404 must not change the manager's Connected status"
    );
    let (conn_after, gen_after) = manager
        .connection_within(PUBLISH_DISCONNECTED_BOUND)
        .await
        .expect("the connection must stay live after a channel-scoped failure");
    // Concrete identity evidence for the amendment log: the same `Arc`
    // pointer and the same generation before and after the 404.
    eprintln!(
        "channel-isolation evidence: same_connection={} gen_before={gen_before} \
         gen_after={gen_after}",
        Arc::ptr_eq(&conn_before, &conn_after)
    );
    assert!(
        Arc::ptr_eq(&conn_before, &conn_after),
        "the connection Arc identity must be preserved across a channel-scoped failure"
    );
    assert_eq!(
        gen_before, gen_after,
        "the generation must not change for a channel-scoped failure"
    );

    // Next publish on the healthy connection (a fresh channel) succeeds and
    // round-trips.
    let ok_cfg = RabbitEndpointConfig::from_uri(&format!("rabbitmq:default?queue={queue}"))
        .expect("ok URI parses");
    let mut ok_producer = RabbitProducer::new(ok_cfg, Arc::clone(&manager), Arc::clone(&rt));
    ok_producer
        .call(Exchange::new(Message::new(Body::Text(
            "after-404".to_string(),
        ))))
        .await
        .expect("the next publish on the same connection must succeed");

    let got = raw_channel
        .basic_get(
            ShortString::from(queue.as_str()),
            BasicGetOptions { no_ack: true },
        )
        .await
        .expect("basic_get must not error")
        .expect("the post-404 message must be on the queue");
    assert_eq!(
        got.data, b"after-404",
        "the body must round-trip after the channel-scoped failure"
    );
}
