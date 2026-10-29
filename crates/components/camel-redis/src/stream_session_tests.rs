//! Session- and wire-level tests for the Redis Streams consumer.
//! Sibling file via `#[path]` so the production module stays scannable;
//! still in-crate for private-function access.

use super::*;
use crate::topology::FakeTopology;
use crate::transport_error::io_reset_error;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, mpsc, oneshot};
use tracing_subscriber::layer::SubscriberExt;

type ReadOutcome = Result<Option<Vec<StreamEntry>>, CamelError>;

fn io_reset() -> CamelError {
    redis_error_raw(io_reset_error())
}

fn entry(id: &str, fields: &[(&str, &str)]) -> StreamEntry {
    StreamEntry {
        id: id.to_string(),
        fields: fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        delivery_count: 1,
    }
}

fn batch(entries: Vec<StreamEntry>) -> ReadOutcome {
    Ok(Some(entries))
}

#[test]
fn exchange_single_field_body_and_headers() {
    let entry = entry("1-1", &[("payload", "hello")]);

    let exchange = build_exchange_from_stream_entry("s1", "g", "c", &entry);

    assert_eq!(exchange.input.body.as_text(), Some("hello"));
    assert_eq!(
        exchange.input.headers.len(),
        5,
        "single-field entry must set exactly the five delivery headers"
    );
    assert_eq!(
        exchange.input.header("CamelRedis.Stream"),
        Some(&serde_json::json!("s1"))
    );
    assert_eq!(
        exchange.input.header("CamelRedis.StreamId"),
        Some(&serde_json::json!("1-1"))
    );
    assert_eq!(
        exchange.input.header("CamelRedis.StreamGroup"),
        Some(&serde_json::json!("g"))
    );
    assert_eq!(
        exchange.input.header("CamelRedis.StreamConsumer"),
        Some(&serde_json::json!("c"))
    );
    assert_eq!(
        exchange.input.header("CamelRedis.StreamDeliveryCount"),
        Some(&serde_json::json!("1"))
    );
}

#[test]
fn exchange_multi_field_body_is_json_object() {
    let entry = entry("2-2", &[("b", "2"), ("a", "1")]);

    let exchange = build_exchange_from_stream_entry("s1", "g", "c", &entry);

    let body = exchange
        .input
        .body
        .as_text()
        .expect("multi-field body must be text");
    assert_eq!(
        body, r#"{"a":"1","b":"2"}"#,
        "multi-field body must be byte-for-byte alphabetical"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(body).expect("multi-field body must be JSON");
    assert_eq!(parsed, serde_json::json!({"a": "1", "b": "2"}));
}

/// Test double for [`StreamReadIo`]: programmable connect/read/ack outcomes
/// and recorded successful acks.
///
/// Once the read queue drains, `read_group` pends forever, so session tests
/// end by cancellation rather than by inventing extra entries.
struct FakeStreamIo {
    connect_outcomes: Vec<Result<(), CamelError>>,
    read_queue: Vec<ReadOutcome>,
    ack_outcomes: VecDeque<Result<i64, CamelError>>,
    connects: Arc<AtomicUsize>,
    connect_changed: Arc<Notify>,
    acks: Arc<Mutex<Vec<String>>>,
    ack_changed: Arc<Notify>,
    /// Ordered trace of read/ack operations (`read`, `ack:<ids>`).
    events: Arc<Mutex<Vec<String>>>,
    /// Set when the read half is dropped (socket closed).
    dropped: Arc<AtomicBool>,
    drop_changed: Arc<Notify>,
    /// Fires when the fake begins a read that will block forever.
    read_entered: Option<oneshot::Sender<()>>,
    /// Panic instead of reading, to exercise reader-panic propagation.
    panic_on_read: bool,
    gate: Option<oneshot::Receiver<()>>,
    gate_entered: Option<oneshot::Sender<()>>,
    /// Withhold the next `XACK` reply until the test releases this gate.
    ack_gate: Option<oneshot::Receiver<()>>,
    /// Fires when the gated `XACK` begins.
    ack_gate_entered: Option<oneshot::Sender<()>>,
}

impl FakeStreamIo {
    fn new(connect_outcomes: Vec<Result<(), CamelError>>, read_outcomes: Vec<ReadOutcome>) -> Self {
        Self {
            connect_outcomes,
            read_queue: read_outcomes,
            ack_outcomes: VecDeque::new(),
            connects: Arc::new(AtomicUsize::new(0)),
            connect_changed: Arc::new(Notify::new()),
            acks: Arc::new(Mutex::new(Vec::new())),
            ack_changed: Arc::new(Notify::new()),
            events: Arc::new(Mutex::new(Vec::new())),
            dropped: Arc::new(AtomicBool::new(false)),
            drop_changed: Arc::new(Notify::new()),
            read_entered: None,
            panic_on_read: false,
            gate: None,
            gate_entered: None,
            ack_gate: None,
            ack_gate_entered: None,
        }
    }

    fn with_ack_outcomes(mut self, outcomes: Vec<Result<i64, CamelError>>) -> Self {
        self.ack_outcomes = outcomes.into();
        self
    }

    /// Gate the first successful connect return behind `gate`; `entered`
    /// fires just before the fake blocks on it.
    fn with_connect_gate(
        mut self,
        gate: oneshot::Receiver<()>,
        entered: oneshot::Sender<()>,
    ) -> Self {
        self.gate = Some(gate);
        self.gate_entered = Some(entered);
        self
    }

    /// Withhold the next `XACK` reply until `gate` fires; `entered` fires
    /// just before the fake blocks on it.
    fn with_ack_gate(mut self, gate: oneshot::Receiver<()>, entered: oneshot::Sender<()>) -> Self {
        self.ack_gate = Some(gate);
        self.ack_gate_entered = Some(entered);
        self
    }

    /// Report `entered` when a read that will block forever begins.
    fn with_read_entered(mut self, entered: oneshot::Sender<()>) -> Self {
        self.read_entered = Some(entered);
        self
    }

    /// Panic on every read instead of returning a batch.
    fn that_panics_on_read(mut self) -> Self {
        self.panic_on_read = true;
        self
    }
}

impl Drop for FakeStreamIo {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
        self.drop_changed.notify_waiters();
    }
}

#[async_trait]
impl StreamReadIo for FakeStreamIo {
    async fn connect(&mut self, _client: &redis::Client) -> Result<(), CamelError> {
        let idx = self.connects.load(Ordering::SeqCst);
        self.connects.fetch_add(1, Ordering::SeqCst);
        self.connect_changed.notify_waiters();

        if let Some(entered) = self.gate_entered.take() {
            let _ = entered.send(());
        }
        if let Some(gate) = self.gate.take() {
            let _ = gate.await;
        }

        match self.connect_outcomes.get(idx) {
            Some(outcome) => outcome.clone(),
            None => self.connect_outcomes.last().cloned().unwrap_or(Ok(())),
        }
    }

    async fn read_group(
        &mut self,
        _params: &StreamConsumerParams,
    ) -> Result<Option<Vec<StreamEntry>>, CamelError> {
        self.events
            .lock()
            .expect("events mutex")
            .push("read".into());
        if self.panic_on_read {
            panic!("fake stream read panic");
        }
        if !self.read_queue.is_empty() {
            return self.read_queue.remove(0);
        }
        if let Some(entered) = self.read_entered.take() {
            let _ = entered.send(());
        }
        std::future::pending().await
    }

    async fn ack(&mut self, _key: &str, _group: &str, ids: &[String]) -> Result<i64, CamelError> {
        self.events
            .lock()
            .expect("events mutex")
            .push(format!("ack:{}", ids.join(",")));
        if let Some(entered) = self.ack_gate_entered.take() {
            let _ = entered.send(());
        }
        if let Some(gate) = self.ack_gate.take() {
            let _ = gate.await;
        }
        let outcome = self.ack_outcomes.pop_front().unwrap_or(Ok(1));
        if outcome.is_ok() {
            let mut acks = self.acks.lock().expect("acks mutex");
            acks.extend(ids.iter().cloned());
            drop(acks);
            self.ack_changed.notify_waiters();
        }
        outcome
    }
}

/// One withheld `recover` reply.
struct RecoverGate {
    /// Zero-based `recover` call index the gate applies to.
    index: usize,
    /// Fires when the gated call begins.
    entered: oneshot::Sender<()>,
    /// The gated call blocks until the test releases it.
    release: oneshot::Receiver<()>,
}

/// Shared handles for asserting on a [`FakeRecoveryIo`] after it has been
/// moved into the session.
#[derive(Clone)]
struct RecoveryHandles {
    connects: Arc<AtomicUsize>,
    connect_changed: Arc<Notify>,
    recover_calls: Arc<AtomicUsize>,
    recover_changed: Arc<Notify>,
    recover_cursors: Arc<Mutex<Vec<String>>>,
    recover_budgets: Arc<Mutex<Vec<u64>>>,
    acks: Arc<Mutex<Vec<String>>>,
    ack_changed: Arc<Notify>,
}

/// Test double for [`StreamRecoveryIo`]: programmable connect/recover/ack
/// outcomes, per-call withheld replies, and recorded call arguments.
///
/// `recover` outcomes are indexed by call number (retries therefore consume
/// their own slot); once the list drains, the fake returns an empty pass
/// that keeps `cursor` as the next cursor.
struct FakeRecoveryIo {
    connect_outcomes: Vec<Result<(), CamelError>>,
    recover_outcomes: Vec<Result<RecoveryBatch, CamelError>>,
    recover_gate: Option<RecoverGate>,
    ack_outcomes: VecDeque<Result<i64, CamelError>>,
    connects: Arc<AtomicUsize>,
    connect_changed: Arc<Notify>,
    recover_calls: Arc<AtomicUsize>,
    recover_changed: Arc<Notify>,
    recover_cursors: Arc<Mutex<Vec<String>>>,
    recover_budgets: Arc<Mutex<Vec<u64>>>,
    acks: Arc<Mutex<Vec<String>>>,
    ack_changed: Arc<Notify>,
}

impl FakeRecoveryIo {
    fn new() -> Self {
        Self {
            connect_outcomes: Vec::new(),
            recover_outcomes: Vec::new(),
            recover_gate: None,
            ack_outcomes: VecDeque::new(),
            connects: Arc::new(AtomicUsize::new(0)),
            connect_changed: Arc::new(Notify::new()),
            recover_calls: Arc::new(AtomicUsize::new(0)),
            recover_changed: Arc::new(Notify::new()),
            recover_cursors: Arc::new(Mutex::new(Vec::new())),
            recover_budgets: Arc::new(Mutex::new(Vec::new())),
            acks: Arc::new(Mutex::new(Vec::new())),
            ack_changed: Arc::new(Notify::new()),
        }
    }

    fn with_recover_batches(mut self, batches: Vec<RecoveryBatch>) -> Self {
        self.recover_outcomes = batches.into_iter().map(Ok).collect();
        self
    }

    fn with_recover_outcomes(mut self, outcomes: Vec<Result<RecoveryBatch, CamelError>>) -> Self {
        self.recover_outcomes = outcomes;
        self
    }

    fn with_connect_outcomes(mut self, outcomes: Vec<Result<(), CamelError>>) -> Self {
        self.connect_outcomes = outcomes;
        self
    }

    fn with_ack_outcomes(mut self, outcomes: Vec<Result<i64, CamelError>>) -> Self {
        self.ack_outcomes = outcomes.into();
        self
    }

    /// Withhold the `recover` call at `index` until `release` fires.
    fn with_recover_gate(
        mut self,
        index: usize,
        entered: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    ) -> Self {
        self.recover_gate = Some(RecoverGate {
            index,
            entered,
            release,
        });
        self
    }

    fn handles(&self) -> RecoveryHandles {
        RecoveryHandles {
            connects: Arc::clone(&self.connects),
            connect_changed: Arc::clone(&self.connect_changed),
            recover_calls: Arc::clone(&self.recover_calls),
            recover_changed: Arc::clone(&self.recover_changed),
            recover_cursors: Arc::clone(&self.recover_cursors),
            recover_budgets: Arc::clone(&self.recover_budgets),
            acks: Arc::clone(&self.acks),
            ack_changed: Arc::clone(&self.ack_changed),
        }
    }
}

#[async_trait]
impl StreamRecoveryIo for FakeRecoveryIo {
    async fn connect(&mut self, _client: &redis::Client) -> Result<(), CamelError> {
        let idx = self.connects.load(Ordering::SeqCst);
        self.connects.fetch_add(1, Ordering::SeqCst);
        self.connect_changed.notify_waiters();
        match self.connect_outcomes.get(idx) {
            Some(outcome) => outcome.clone(),
            None => self.connect_outcomes.last().cloned().unwrap_or(Ok(())),
        }
    }

    async fn recover(
        &mut self,
        _key: &str,
        _group: &str,
        _consumer: &str,
        _min_idle_ms: u64,
        cursor: &str,
        budget: u64,
    ) -> Result<RecoveryBatch, CamelError> {
        let idx = self.recover_calls.load(Ordering::SeqCst);
        self.recover_calls.fetch_add(1, Ordering::SeqCst);
        self.recover_cursors
            .lock()
            .expect("cursors mutex")
            .push(cursor.to_string());
        self.recover_budgets
            .lock()
            .expect("budgets mutex")
            .push(budget);
        self.recover_changed.notify_waiters();

        if let Some(gate) = self.recover_gate.take() {
            if gate.index == idx {
                let _ = gate.entered.send(());
                let _ = gate.release.await;
            } else {
                self.recover_gate = Some(gate);
            }
        }

        match self.recover_outcomes.get(idx) {
            Some(outcome) => outcome.clone(),
            None => Ok(RecoveryBatch {
                pending_count: None,
                recovered: Vec::new(),
                next_cursor: cursor.to_string(),
            }),
        }
    }

    async fn recover_ack(
        &mut self,
        _key: &str,
        _group: &str,
        ids: &[String],
    ) -> Result<i64, CamelError> {
        let outcome = self.ack_outcomes.pop_front().unwrap_or(Ok(1));
        if outcome.is_ok() {
            let mut acks = self.acks.lock().expect("acks mutex");
            acks.extend(ids.iter().cloned());
            drop(acks);
            self.ack_changed.notify_waiters();
        }
        outcome
    }
}

fn fake_recovery() -> Box<dyn StreamRecoveryIo> {
    Box::new(FakeRecoveryIo::new())
}

fn recovered(id: &str, fields: &[(&str, &str)], delivery_count: Option<u64>) -> RecoveredEntry {
    RecoveredEntry {
        entry: entry(id, fields),
        delivery_count,
    }
}

fn recovery_batch(
    pending_count: Option<u64>,
    recovered: Vec<RecoveredEntry>,
    next_cursor: &str,
) -> RecoveryBatch {
    RecoveryBatch {
        pending_count,
        recovered,
        next_cursor: next_cursor.to_string(),
    }
}

fn recovery_params(min_idle_ms: u64) -> StreamConsumerParams {
    let mut params = stream_params();
    params.recover_enabled = true;
    params.recover_min_idle_ms = min_idle_ms;
    params
}

/// Session harness driving [`session_core`] directly with test-held
/// channels (no read task).
struct SessionRun {
    msg_tx: mpsc::Sender<ReadTaskMsg>,
    cmd_rx: mpsc::Receiver<ReadTaskCmd>,
    cancel: CancellationToken,
    handle: JoinHandle<Result<(), CamelError>>,
}

fn spawn_core<D, F>(
    recovery: Box<dyn StreamRecoveryIo>,
    params: StreamConsumerParams,
    cancel: CancellationToken,
    on_ready: Option<Box<dyn FnOnce() + Send>>,
    deliver: D,
) -> SessionRun
where
    D: FnMut(StreamEntry) -> F + Send + 'static,
    F: Future<Output = DeliveryOutcome> + Send + 'static,
{
    let (msg_tx, msg_rx) = mpsc::channel(8);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let topology = Arc::new(FakeTopology::addrs(vec!["redis://a:6379".into()]));
    let session_cancel = cancel.clone();
    let handle = tokio::spawn(async move {
        session_core(
            msg_rx,
            cmd_tx,
            recovery,
            topology,
            params,
            fast_policy(3),
            session_cancel,
            on_ready,
            deliver,
        )
        .await
    });
    SessionRun {
        msg_tx,
        cmd_rx,
        cancel,
        handle,
    }
}

fn fast_policy(max_attempts: u32) -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        max_attempts,
        initial_delay: Duration::from_millis(0),
        ..NetworkRetryPolicy::default()
    }
}

fn stream_params() -> StreamConsumerParams {
    StreamConsumerParams {
        key: "stream".to_string(),
        group: "g".to_string(),
        consumer: "c".to_string(),
        count: Some(10),
        block_ms: 1_000,
        recover_enabled: false,
        recover_min_idle_ms: 60000,
        recover_batch: 10,
    }
}

/// Await `condition` without sleeping; `change` wakes the waiters on every
/// state mutation.
async fn wait_until(change: &Notify, mut condition: impl FnMut() -> bool) {
    loop {
        let notified = change.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if condition() {
            return;
        }
        notified.await; // allow-test-wait: every caller wraps wait_until in tokio::time::timeout
    }
}

async fn wait_for_acks(acks: &Mutex<Vec<String>>, ack_changed: &Notify, target: usize) {
    tokio::time::timeout(
        Duration::from_secs(5),
        wait_until(ack_changed, || {
            acks.lock().expect("acks mutex").len() >= target
        }),
    )
    .await
    .expect("acks must arrive before the timeout");
}

/// Wait (bounded) until the read half is dropped, proving the read socket
/// was closed rather than leaked.
async fn wait_for_drop(dropped: &AtomicBool, drop_changed: &Notify) {
    tokio::time::timeout(
        Duration::from_secs(5),
        wait_until(drop_changed, || dropped.load(Ordering::SeqCst)),
    )
    .await
    .expect("read half must be dropped before the timeout");
}

/// Build the `NOGROUP` error the real `XPENDING`/`XAUTOCLAIM` replies map
/// to when the consumer group does not exist.
fn nogroup_error() -> CamelError {
    redis_error_raw(redis::RedisError::from((
        redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
        "NOGROUP No such consumer group 'g'",
    )))
}

/// Receive one command with a bounded wait.
async fn recv_cmd(rx: &mut mpsc::Receiver<ReadTaskCmd>) -> ReadTaskCmd {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("a read-task command must arrive before the timeout")
        .expect("the command channel must stay open")
}

/// Join a session task with a bounded wait.
async fn join_session(run: SessionRun) -> Result<(), CamelError> {
    tokio::time::timeout(Duration::from_secs(5), run.handle)
        .await
        .expect("the session must finish before the timeout")
        .expect("the session task must not panic")
}

/// Cancel a session and join it, asserting the orderly `Ok` teardown.
async fn finish_ok(run: SessionRun) -> Result<(), CamelError> {
    run.cancel.cancel();
    join_session(run).await
}

async fn wait_for_recover_calls(handles: &RecoveryHandles, target: usize) {
    tokio::time::timeout(
        Duration::from_secs(5),
        wait_until(&handles.recover_changed, || {
            handles.recover_calls.load(Ordering::SeqCst) >= target
        }),
    )
    .await
    .expect("the recovery pass must run before the timeout");
}

async fn wait_for_recovery_acks(handles: &RecoveryHandles, target: usize) {
    tokio::time::timeout(
        Duration::from_secs(5),
        wait_until(&handles.ack_changed, || {
            handles.acks.lock().expect("acks mutex").len() >= target
        }),
    )
    .await
    .expect("the recovery ack must arrive before the timeout");
}

/// One captured tracing event.
#[derive(Clone, Debug, Default)]
struct CapturedEvent {
    message: String,
    fields: Vec<(String, String)>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Default)]
struct EventVisitor {
    message: String,
    fields: Vec<(String, String)>,
}

impl tracing::field::Visit for EventVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        if field.name() == "message" {
            self.message = rendered;
        } else {
            self.fields.push((field.name().to_string(), rendered));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields
                .push((field.name().to_string(), value.to_string()));
        }
    }
}

/// Capturing `tracing` layer for pass-summary and skip-warning assertions.
#[derive(Clone, Default)]
struct LogCapture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl LogCapture {
    fn events(&self) -> Vec<CapturedEvent> {
        self.events.lock().expect("events mutex").clone()
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LogCapture {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);
        self.events
            .lock()
            .expect("events mutex")
            .push(CapturedEvent {
                message: visitor.message,
                fields: visitor.fields,
            });
    }
}

/// Install a capturing subscriber for the current test thread.
fn capture_logs() -> (LogCapture, tracing::subscriber::DefaultGuard) {
    let capture = LogCapture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let dispatch = tracing::Dispatch::new(subscriber);
    let guard = tracing::dispatcher::set_default(&dispatch);
    (capture, guard)
}

#[tokio::test]
async fn session_delivers_and_acks_each_entry() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(
        vec![Ok(())],
        vec![batch(vec![
            entry("1-1", &[("payload", "hello")]),
            entry("2-2", &[("a", "1")]),
        ])],
    )
    .with_ack_outcomes(vec![Ok(1), Ok(1)]);
    let acks = io.acks.clone();
    let ack_changed = io.ack_changed.clone();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let seen = delivered.clone();
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let policy = fast_policy(2);
    let params = stream_params();

    let handle = tokio::spawn(async move {
        let deliver = move |entry: StreamEntry| {
            let seen = seen.clone();
            async move {
                seen.lock().expect("delivered mutex").push(entry.id);
                DeliveryOutcome::Completed
            }
        };
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            params,
            policy,
            cancel_task,
            None,
            deliver,
        )
        .await
    });

    wait_for_acks(&acks, &ack_changed, 2).await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");

    assert!(result.is_ok(), "session must end Ok: {result:?}");
    assert_eq!(
        *acks.lock().expect("acks mutex"),
        vec!["1-1".to_string(), "2-2".to_string()],
        "both entries must be acked in delivery order"
    );
    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec!["1-1".to_string(), "2-2".to_string()],
        "both entries must be delivered in order"
    );
}

#[tokio::test]
async fn session_no_ack_when_deliver_fails() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(
        vec![Ok(())],
        vec![batch(vec![entry("1-1", &[]), entry("2-2", &[])])],
    )
    .with_ack_outcomes(vec![Ok(1)]);
    let acks = io.acks.clone();
    let ack_changed = io.ack_changed.clone();
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let policy = fast_policy(2);
    let params = stream_params();

    let handle = tokio::spawn(async move {
        let deliver = move |entry: StreamEntry| async move {
            if entry.id == "1-1" {
                DeliveryOutcome::Failed
            } else {
                DeliveryOutcome::Completed
            }
        };
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            params,
            policy,
            cancel_task,
            None,
            deliver,
        )
        .await
    });

    wait_for_acks(&acks, &ack_changed, 1).await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");

    assert!(result.is_ok(), "session must end Ok: {result:?}");
    assert_eq!(
        *acks.lock().expect("acks mutex"),
        vec!["2-2".to_string()],
        "a Failed delivery must not be acked"
    );
}

#[tokio::test]
async fn session_channel_gone_returns_err_without_ack() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(
        vec![Ok(())],
        vec![batch(vec![entry("1-1", &[]), entry("2-2", &[])])],
    );
    let acks = io.acks.clone();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let seen = delivered.clone();
    let cancel = CancellationToken::new();
    let policy = fast_policy(2);
    let params = stream_params();

    let deliver = move |entry: StreamEntry| {
        let seen = seen.clone();
        async move {
            seen.lock().expect("delivered mutex").push(entry.id.clone());
            if entry.id == "1-1" {
                DeliveryOutcome::ChannelGone
            } else {
                DeliveryOutcome::Completed
            }
        }
    };

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            params,
            policy,
            cancel,
            None,
            deliver,
        ),
    )
    .await
    .expect("session must terminate on ChannelGone");

    match result {
        Err(CamelError::ChannelClosed) => {}
        other => {
            panic!("ChannelGone must end the session with ChannelClosed: {other:?}")
        }
    }
    assert!(
        acks.lock().expect("acks mutex").is_empty(),
        "ChannelGone must not ack the delivered entry"
    );
    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec!["1-1".to_string()],
        "session must stop delivering after ChannelGone"
    );
}

#[tokio::test]
async fn session_stops_batch_processing_on_cancellation() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(
        vec![Ok(())],
        vec![batch(vec![entry("1-1", &[]), entry("2-2", &[])])],
    );
    let acks = io.acks.clone();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let seen = delivered.clone();
    let cancel = CancellationToken::new();
    let cancel_in_deliver = cancel.clone();
    let policy = fast_policy(2);
    let params = stream_params();

    let deliver = move |entry: StreamEntry| {
        let seen = seen.clone();
        let cancel_in_deliver = cancel_in_deliver.clone();
        async move {
            seen.lock().expect("delivered mutex").push(entry.id.clone());
            if entry.id == "1-1" {
                cancel_in_deliver.cancel();
            }
            DeliveryOutcome::Completed
        }
    };

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            params,
            policy,
            cancel,
            None,
            deliver,
        ),
    )
    .await
    .expect("cancelled session must finish");

    assert!(result.is_ok(), "cancelled session must end Ok: {result:?}");
    assert!(
        acks.lock().expect("acks mutex").is_empty(),
        "the pre-XACK cancellation point must not ack an entry delivered before the cancel"
    );
    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec!["1-1".to_string()],
        "cancellation must stop the batch before delivering 2-2"
    );
}

#[tokio::test]
async fn read_task_acks_precede_next_read() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(
        vec![Ok(())],
        vec![
            batch(vec![entry("1-1", &[])]),
            batch(vec![entry("2-2", &[])]),
        ],
    )
    .with_ack_outcomes(vec![Ok(1), Ok(1)]);
    let events = io.events.clone();
    let acks = io.acks.clone();
    let ack_changed = io.ack_changed.clone();
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let mut params = stream_params();
    params.block_ms = 0;

    let handle = tokio::spawn(async move {
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            params,
            fast_policy(2),
            cancel_task,
            None,
            |_entry: StreamEntry| async { DeliveryOutcome::Completed },
        )
        .await
    });

    wait_for_acks(&acks, &ack_changed, 2).await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");
    assert!(result.is_ok(), "session must end Ok: {result:?}");

    let events = events.lock().expect("events mutex").clone();
    let read_positions: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| event.as_str() == "read")
        .map(|(idx, _)| idx)
        .collect();
    let first_ack = events
        .iter()
        .position(|event| event.as_str() == "ack:1-1")
        .expect("the first batch must be acked");
    assert!(
        read_positions.len() >= 2,
        "the read task must issue a second read: {events:?}"
    );
    assert!(
        first_ack < read_positions[1],
        "the first batch's XACK must precede the second read: {events:?}"
    );
}

#[tokio::test]
async fn read_task_aborted_on_cancel_closes_io() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let (entered_tx, entered_rx) = oneshot::channel();
    let io = FakeStreamIo::new(vec![Ok(())], vec![]).with_read_entered(entered_tx);
    let acks = io.acks.clone();
    let dropped = io.dropped.clone();
    let drop_changed = io.drop_changed.clone();
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();

    let handle = tokio::spawn(async move {
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            stream_params(),
            fast_policy(2),
            cancel_task,
            None,
            |_entry: StreamEntry| async { DeliveryOutcome::Completed },
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .expect("the read must begin")
        .expect("the read gate sender must live");
    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");
    assert!(result.is_ok(), "cancelled session must end Ok: {result:?}");
    assert!(
        acks.lock().expect("acks mutex").is_empty(),
        "a cancelled blocked read must not ack anything"
    );
    wait_for_drop(&dropped, &drop_changed).await;
}

#[tokio::test]
async fn session_drop_aborts_read_task() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let (entered_tx, entered_rx) = oneshot::channel();
    let io = FakeStreamIo::new(vec![Ok(())], vec![]).with_read_entered(entered_tx);
    let dropped = io.dropped.clone();
    let drop_changed = io.drop_changed.clone();

    let handle = tokio::spawn(async move {
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            stream_params(),
            fast_policy(2),
            CancellationToken::new(),
            None,
            |_entry: StreamEntry| async { DeliveryOutcome::Completed },
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .expect("the read must begin")
        .expect("the read gate sender must live");

    // Dropping the session future drops the abort-on-drop guard.
    handle.abort();
    let _ = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("the aborted session task must finish");
    wait_for_drop(&dropped, &drop_changed).await;
}

#[tokio::test]
async fn reader_panic_propagates_terminal_error() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(vec![Ok(())], vec![]).that_panics_on_read();

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            stream_params(),
            fast_policy(2),
            CancellationToken::new(),
            None,
            |_entry: StreamEntry| async { DeliveryOutcome::Completed },
        ),
    )
    .await
    .expect("a reader panic must not hang the session");

    assert!(
        result.is_err(),
        "a reader panic must fail supervision instead of detaching: {result:?}"
    );
}

#[tokio::test]
async fn reader_control_channel_closure_propagates() {
    let (msg_tx, msg_rx) = mpsc::channel::<ReadTaskMsg>(8);
    let (cmd_tx, cmd_rx) = mpsc::channel::<ReadTaskCmd>(8);
    let reader = tokio::spawn(async move {
        // Close the read-outcome channel without cancelling. Keep the
        // command receiver alive so the session sees the read-outcome
        // closure, not a closed command channel.
        drop(msg_tx);
        let _cmd_rx = cmd_rx;
        std::future::pending::<()>().await;
    });

    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        session_core(
            msg_rx,
            cmd_tx,
            fake_recovery(),
            Arc::new(topology),
            stream_params(),
            fast_policy(2),
            CancellationToken::new(),
            None,
            |_entry: StreamEntry| async { DeliveryOutcome::Completed },
        ),
    )
    .await
    .expect("session_core must finish on channel closure");

    reader.abort();
    assert!(
        result.is_err(),
        "an unexpected read-outcome channel closure must fail supervision: {result:?}"
    );
}

#[tokio::test]
async fn session_acked_entries_stay_acked_after_cancel() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(
        vec![Ok(())],
        vec![
            batch(vec![entry("1-1", &[])]),
            batch(vec![entry("2-2", &[])]),
        ],
    )
    .with_ack_outcomes(vec![Ok(1), Ok(1)]);
    let acks = io.acks.clone();
    let ack_changed = io.ack_changed.clone();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let seen = delivered.clone();
    let cancel = CancellationToken::new();
    let cancel_in_deliver = cancel.clone();

    let handle = tokio::spawn(async move {
        let deliver = move |entry: StreamEntry| {
            let seen = seen.clone();
            let cancel_in_deliver = cancel_in_deliver.clone();
            async move {
                seen.lock().expect("delivered mutex").push(entry.id.clone());
                if entry.id == "2-2" {
                    cancel_in_deliver.cancel();
                }
                DeliveryOutcome::Completed
            }
        };
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            stream_params(),
            fast_policy(2),
            cancel,
            None,
            deliver,
        )
        .await
    });

    wait_for_acks(&acks, &ack_changed, 1).await;
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");
    assert!(result.is_ok(), "cancelled session must end Ok: {result:?}");
    assert_eq!(
        *acks.lock().expect("acks mutex"),
        vec!["1-1".to_string()],
        "the ack completed before the cancel must stay acked"
    );
    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec!["1-1".to_string(), "2-2".to_string()],
        "the second batch must have been delivered before the cancel stopped its ack"
    );
}

#[tokio::test]
async fn session_block_timeout_keeps_connection_then_delivers() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(
        vec![Ok(())],
        vec![Ok(None), batch(vec![entry("1-1", &[("payload", "hello")])])],
    );
    let acks = io.acks.clone();
    let ack_changed = io.ack_changed.clone();
    let connects = io.connects.clone();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let seen = delivered.clone();
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let policy = fast_policy(2);
    let params = stream_params();

    let handle = tokio::spawn(async move {
        let deliver = move |entry: StreamEntry| {
            let seen = seen.clone();
            async move {
                seen.lock().expect("delivered mutex").push(entry.id);
                DeliveryOutcome::Completed
            }
        };
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            params,
            policy,
            cancel_task,
            None,
            deliver,
        )
        .await
    });

    wait_for_acks(&acks, &ack_changed, 1).await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");

    assert!(result.is_ok(), "session must end Ok: {result:?}");
    assert_eq!(
        connects.load(Ordering::SeqCst),
        1,
        "an Ok(None) BLOCK timeout must keep the connection"
    );
    assert_eq!(
        *acks.lock().expect("acks mutex"),
        vec!["1-1".to_string()],
        "only the real entry must be acked"
    );
    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec!["1-1".to_string()],
        "the BLOCK timeout must not deliver a phantom entry"
    );
}

#[tokio::test]
async fn session_cancel_while_blocked_no_ack() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(vec![Ok(())], vec![]);
    let connects = io.connects.clone();
    let connect_changed = io.connect_changed.clone();
    let acks = io.acks.clone();
    let cancel = CancellationToken::new();
    let cancel_control = cancel.clone();

    let controller = tokio::spawn(async move {
        wait_until(&connect_changed, || connects.load(Ordering::SeqCst) >= 1).await;
        cancel_control.cancel();
    });

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            stream_params(),
            fast_policy(2),
            cancel,
            None,
            |_entry| async { DeliveryOutcome::Completed },
        ),
    )
    .await
    .expect("cancelled session must finish");

    tokio::time::timeout(Duration::from_secs(5), controller)
        .await
        .expect("controller must finish")
        .expect("controller must not panic");
    assert!(result.is_ok(), "cancelled session must end Ok: {result:?}");
    assert!(
        acks.lock().expect("acks mutex").is_empty(),
        "a blocked read must not ack anything"
    );
}

#[tokio::test]
async fn session_transient_read_error_reconnects_and_delivers() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(
        vec![Ok(()), Ok(())],
        vec![Err(io_reset()), batch(vec![entry("1-1", &[])])],
    )
    .with_ack_outcomes(vec![Ok(1)]);
    let acks = io.acks.clone();
    let ack_changed = io.ack_changed.clone();
    let connects = io.connects.clone();
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let policy = fast_policy(2);
    let params = stream_params();

    let handle = tokio::spawn(async move {
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            params,
            policy,
            cancel_task,
            None,
            |_entry| async { DeliveryOutcome::Completed },
        )
        .await
    });

    wait_for_acks(&acks, &ack_changed, 1).await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");

    assert!(result.is_ok(), "session must end Ok: {result:?}");
    assert_eq!(
        connects.load(Ordering::SeqCst),
        2,
        "a transient read error must reconnect"
    );
    assert_eq!(
        *acks.lock().expect("acks mutex"),
        vec!["1-1".to_string()],
        "the entry read after reconnect must be acked"
    );
}

#[tokio::test]
async fn session_non_transient_read_error_fails_supervision() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let nogroup = redis_error_raw(redis::RedisError::from((
        redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
        "NOGROUP No such consumer group 'g'",
    )));
    let io = FakeStreamIo::new(vec![Ok(())], vec![Err(nogroup)]);
    let connects = io.connects.clone();

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            stream_params(),
            fast_policy(3),
            CancellationToken::new(),
            None,
            |_entry| async { DeliveryOutcome::Completed },
        ),
    )
    .await
    .expect("session must finish");

    assert!(
        result.is_err(),
        "a NOGROUP response must fail supervision, got Ok"
    );
    assert_eq!(
        connects.load(Ordering::SeqCst),
        1,
        "a non-transient read error must not reconnect"
    );
}

#[tokio::test]
async fn session_fires_on_ready_once_after_connect() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let (gate_tx, gate_rx) = oneshot::channel();
    let (entered_tx, entered_rx) = oneshot::channel();
    let io = FakeStreamIo::new(vec![Ok(()), Ok(())], vec![Err(io_reset())])
        .with_connect_gate(gate_rx, entered_tx);
    let connects = io.connects.clone();
    let connect_changed = io.connect_changed.clone();
    let ready_count = Arc::new(AtomicUsize::new(0));
    let ready_for_callback = ready_count.clone();
    let on_ready: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(move || {
        ready_for_callback.fetch_add(1, Ordering::SeqCst);
    }));
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let policy = fast_policy(2);
    let params = stream_params();

    let handle = tokio::spawn(async move {
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            params,
            policy,
            cancel_task,
            on_ready,
            |_entry| async { DeliveryOutcome::Completed },
        )
        .await
    });

    entered_rx
        .await
        .expect("session must reach the gated connect");
    assert_eq!(
        ready_count.load(Ordering::SeqCst),
        0,
        "on_ready must not fire before the connect completes"
    );

    gate_tx.send(()).expect("release the gated connect");
    tokio::time::timeout(
        Duration::from_secs(5),
        wait_until(&connect_changed, || connects.load(Ordering::SeqCst) >= 2),
    )
    .await
    .expect("session must reconnect after the transient read error");

    assert_eq!(
        ready_count.load(Ordering::SeqCst),
        1,
        "on_ready must fire exactly once across both connections"
    );

    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");
    assert!(result.is_ok(), "session must end Ok: {result:?}");
}

#[tokio::test]
async fn session_transient_ack_error_reconnects_no_replay() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(
        vec![Ok(()), Ok(())],
        vec![
            batch(vec![entry("1-1", &[])]),
            batch(vec![entry("2-2", &[])]),
        ],
    )
    .with_ack_outcomes(vec![Err(io_reset()), Ok(1)]);
    let acks = io.acks.clone();
    let ack_changed = io.ack_changed.clone();
    let connects = io.connects.clone();
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let policy = fast_policy(2);
    let params = stream_params();

    let handle = tokio::spawn(async move {
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            params,
            policy,
            cancel_task,
            None,
            |_entry| async { DeliveryOutcome::Completed },
        )
        .await
    });

    wait_for_acks(&acks, &ack_changed, 1).await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");

    assert!(result.is_ok(), "session must end Ok: {result:?}");
    assert_eq!(
        connects.load(Ordering::SeqCst),
        2,
        "a transient ack error must reconnect"
    );
    assert_eq!(
        *acks.lock().expect("acks mutex"),
        vec!["2-2".to_string()],
        "the unknown-outcome ack must never be retried after reconnect"
    );
    assert!(
        !acks
            .lock()
            .expect("acks mutex")
            .contains(&"1-1".to_string()),
        "the 1-1 ack must never land: its outcome is unknown"
    );
}

#[tokio::test]
async fn session_budget_exhausted_ack_error_fails_supervision() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let exhausted = crate::retry::retry_budget_exhausted(
        &fast_policy(2),
        "acknowledging stream entries",
        "connection reset",
    );
    let io = FakeStreamIo::new(vec![Ok(())], vec![batch(vec![entry("1-1", &[])])])
        .with_ack_outcomes(vec![Err(exhausted)]);
    let acks = io.acks.clone();

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        stream_session(
            Box::new(io),
            fake_recovery(),
            Arc::new(topology),
            stream_params(),
            fast_policy(2),
            CancellationToken::new(),
            None,
            |_entry| async { DeliveryOutcome::Completed },
        ),
    )
    .await
    .expect("session must finish");

    assert!(
        result.is_err(),
        "a budget-exhausted ack error must fail supervision, got Ok"
    );
    assert!(
        acks.lock().expect("acks mutex").is_empty(),
        "the budget-exhausted ack must never record the entry id"
    );
}

fn bulk(value: &str) -> redis::Value {
    redis::Value::BulkString(value.as_bytes().to_vec())
}

fn claimed_entry(id: &str, fields: &[(&str, &str)]) -> redis::Value {
    let pairs = fields
        .iter()
        .flat_map(|(field, value)| [bulk(field), bulk(value)])
        .collect();
    redis::Value::Array(vec![bulk(id), redis::Value::Array(pairs)])
}

fn claim_reply(cursor: &str, entries: Vec<redis::Value>) -> redis::Value {
    redis::Value::Array(vec![bulk(cursor), redis::Value::Array(entries)])
}

#[test]
fn xautoclaim_reply_two_element_shape_parses() {
    let reply = claim_reply("5-0", vec![claimed_entry("1-1", &[("payload", "hello")])]);

    let parsed = parse_claim_reply(reply).expect("two-element XAUTOCLAIM reply must parse");

    assert_eq!(parsed.next_cursor, "5-0");
    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(parsed.entries[0].entry.id, "1-1");
    assert_eq!(
        parsed.entries[0].entry.fields,
        vec![("payload".to_string(), "hello".to_string())]
    );
    assert_eq!(
        parsed.entries[0].delivery_count, None,
        "the claim reply itself carries no delivery count"
    );
}

#[test]
fn xautoclaim_reply_three_element_shape_parses() {
    let reply = redis::Value::Array(vec![
        bulk("0-0"),
        redis::Value::Array(vec![claimed_entry("2-2", &[("a", "1")])]),
        redis::Value::Array(vec![bulk("3-3")]),
    ]);

    let parsed = parse_claim_reply(reply).expect("three-element XAUTOCLAIM reply must parse");

    assert_eq!(parsed.next_cursor, "0-0");
    assert_eq!(
        parsed.entries.len(),
        1,
        "deleted ids must not become entries"
    );
    assert_eq!(parsed.entries[0].entry.id, "2-2");
}

#[test]
fn xautoclaim_empty_entries_parses() {
    let replies = [
        claim_reply("0-0", vec![]),
        redis::Value::Array(vec![
            bulk("0-0"),
            redis::Value::Array(vec![]),
            redis::Value::Array(vec![]),
        ]),
    ];

    for reply in replies {
        let parsed = parse_claim_reply(reply).expect("an empty XAUTOCLAIM entry list must parse");
        assert!(parsed.entries.is_empty());
        assert_eq!(parsed.next_cursor, "0-0", "the loop is drained");
    }
}

#[test]
fn xpending_summary_count_parses() {
    let summary = redis::Value::Array(vec![
        redis::Value::Int(2),
        bulk("1-1"),
        bulk("2-2"),
        redis::Value::Array(vec![bulk("c"), bulk("2")]),
    ]);

    assert_eq!(
        pending_summary_gate(&summary).expect("summary count must parse"),
        Some(2)
    );
    assert_eq!(
        pending_summary_gate(&redis::Value::Nil).expect("nil summary is empty"),
        None
    );
}

#[test]
fn targeted_xpending_row_parses_delivery_count() {
    let reply = redis::Value::Array(vec![redis::Value::Array(vec![
        bulk("1-1"),
        bulk("c"),
        bulk("42000"),
        redis::Value::Int(3),
    ])]);

    assert_eq!(
        parse_targeted_xpending(&reply).expect("targeted XPENDING row must parse"),
        Some(3)
    );
    assert_eq!(
        parse_targeted_xpending(&redis::Value::Array(vec![]))
            .expect("an empty targeted XPENDING reply is not an error"),
        None
    );
}

#[test]
fn exchange_carries_delivery_count_from_entry() {
    let entry = StreamEntry {
        id: "9-9".to_string(),
        fields: vec![("payload".to_string(), "hello".to_string())],
        delivery_count: 4,
    };

    let exchange = build_exchange_from_stream_entry("s1", "g", "c", &entry);

    assert_eq!(
        exchange.input.header("CamelRedis.StreamDeliveryCount"),
        Some(&serde_json::json!("4"))
    );
}

#[test]
fn claim_loop_continues_after_empty_nonzero_cursor() {
    let mut driver = ClaimLoopDriver::new("s", "g", "c", 60_000, "0-0".to_string(), 5);

    let first = match driver.advance(None) {
        ClaimStep::Cmd(args) => args,
        other => panic!("the first step must issue a command: {other:?}"),
    };
    assert_eq!(
        first,
        vec![
            "XAUTOCLAIM".to_string(),
            "s".to_string(),
            "g".to_string(),
            "c".to_string(),
            "60000".to_string(),
            "0-0".to_string(),
            "COUNT".to_string(),
            "5".to_string(),
        ]
    );

    let second = match driver.advance(Some(claim_reply("5-0", vec![]))) {
        ClaimStep::Cmd(args) => args,
        other => panic!("an empty non-zero cursor must continue: {other:?}"),
    };
    assert_eq!(second[5], "5-0", "the next frame resumes from the cursor");

    let done = driver.advance(Some(claim_reply(
        "0-0",
        vec![claimed_entry("7-7", &[("a", "1")])],
    )));
    match done {
        ClaimStep::Done(Ok(result)) => {
            assert_eq!(result.recovered.len(), 1);
            assert_eq!(result.recovered[0].entry.id, "7-7");
            assert_eq!(result.next_cursor, "0-0");
            assert_eq!(result.commands_issued, 2, "exactly two claim frames");
        }
        other => panic!("a 0-0 cursor must finish the loop: {other:?}"),
    }
}

#[test]
fn claim_loop_skips_nil_entries_deleted_while_pending() {
    let mut driver = ClaimLoopDriver::new("s", "g", "c", 60_000, "0-0".to_string(), 1);

    match driver.advance(None) {
        ClaimStep::Cmd(_) => {}
        other => panic!("the first step must issue a command: {other:?}"),
    }

    // Redis 7.0+ reports entries deleted while pending as NIL items inside
    // the entries array; they are already gone from the PEL.
    let reply = redis::Value::Array(vec![
        bulk("5-0"),
        redis::Value::Array(vec![
            redis::Value::Nil,
            claimed_entry("7-7", &[("a", "1")]),
            redis::Value::Nil,
        ]),
    ]);
    match driver.advance(Some(reply)) {
        ClaimStep::Done(Ok(result)) => {
            assert_eq!(
                result.recovered.len(),
                1,
                "nil items must not become entries"
            );
            assert_eq!(result.recovered[0].entry.id, "7-7");
            assert_eq!(result.next_cursor, "5-0");
        }
        other => panic!("nil entries must not fail the claim loop: {other:?}"),
    }
}

#[test]
fn claim_loop_uses_remaining_budget_and_cursor() {
    let mut driver = ClaimLoopDriver::new("s", "g", "c", 60_000, "0-0".to_string(), 5);

    let first = match driver.advance(None) {
        ClaimStep::Cmd(args) => args,
        other => panic!("the first step must issue a command: {other:?}"),
    };
    assert_eq!(
        first,
        vec![
            "XAUTOCLAIM".to_string(),
            "s".to_string(),
            "g".to_string(),
            "c".to_string(),
            "60000".to_string(),
            "0-0".to_string(),
            "COUNT".to_string(),
            "5".to_string(),
        ]
    );

    let second = match driver.advance(Some(claim_reply(
        "3-0",
        vec![
            claimed_entry("1-1", &[("a", "1")]),
            claimed_entry("2-2", &[("a", "2")]),
            claimed_entry("3-3", &[("a", "3")]),
            claimed_entry("4-4", &[("a", "4")]),
        ],
    ))) {
        ClaimStep::Cmd(args) => args,
        other => panic!("a partial claim must continue: {other:?}"),
    };
    assert_eq!(
        second,
        vec![
            "XAUTOCLAIM".to_string(),
            "s".to_string(),
            "g".to_string(),
            "c".to_string(),
            "60000".to_string(),
            "3-0".to_string(),
            "COUNT".to_string(),
            "1".to_string(),
        ],
        "the next frame resumes from the cursor with the remaining budget"
    );

    let done = driver.advance(Some(claim_reply(
        "0-0",
        vec![claimed_entry("5-5", &[("a", "5")])],
    )));
    match done {
        ClaimStep::Done(Ok(result)) => {
            let ids: Vec<&str> = result
                .recovered
                .iter()
                .map(|recovered| recovered.entry.id.as_str())
                .collect();
            assert_eq!(ids, vec!["1-1", "2-2", "3-3", "4-4", "5-5"]);
            assert_eq!(result.next_cursor, "0-0");
            assert_eq!(result.commands_issued, 2, "exactly two claim frames");
        }
        other => panic!("a 0-0 cursor must finish the loop: {other:?}"),
    }
}

#[test]
fn claim_loop_stops_at_exactly_the_command_cap() {
    let mut driver = ClaimLoopDriver::new("s", "g", "c", 1_000, "0-0".to_string(), 5);
    let mut frames = 0_u64;
    let mut step = driver.start();
    loop {
        match step {
            ClaimStep::Cmd(_) => {
                frames += 1;
                step = driver.advance(Some(claim_reply("5-0", vec![])));
            }
            ClaimStep::Done(result) => {
                let result = result.expect("stopping at the command cap must be clean");
                assert_eq!(result.commands_issued, 5);
                assert_eq!(result.next_cursor, "5-0", "Done carries the last cursor");
                break;
            }
        }
    }
    assert_eq!(
        frames, 5,
        "the driver must issue exactly the budgeted frames"
    );
}

#[test]
fn pending_summary_gate_ends_pass_on_nil_or_zero() {
    assert_eq!(
        pending_summary_gate(&redis::Value::Nil).expect("nil is empty"),
        None
    );
    let zero = redis::Value::Array(vec![
        redis::Value::Int(0),
        bulk("1-1"),
        bulk("1-1"),
        redis::Value::Array(vec![]),
    ]);
    assert_eq!(
        pending_summary_gate(&zero).expect("zero pending parses"),
        Some(0)
    );
    let positive = redis::Value::Array(vec![
        redis::Value::Int(2),
        bulk("1-1"),
        bulk("2-2"),
        redis::Value::Array(vec![bulk("c"), bulk("2")]),
    ]);
    assert_eq!(
        pending_summary_gate(&positive).expect("positive pending parses"),
        Some(2)
    );
    let malformed = redis::Value::BulkString(b"not-a-summary".to_vec());
    assert!(
        pending_summary_gate(&malformed).is_err(),
        "a malformed summary must be an error"
    );
    // Both early outcomes end the pass before any XAUTOCLAIM is issued.
    assert_eq!(pending_summary_gate(&redis::Value::Nil).expect("nil"), None);
    assert_eq!(
        pending_summary_gate(&zero).expect("zero"),
        Some(0),
        "zero pending and nil both terminate the recovery pass"
    );
}

// Regression: a valid stream entry whose field value is not valid UTF-8
// must fail decoding. Substituting an empty string would corrupt the
// payload and let the entry be acknowledged as delivered.
#[test]
fn invalid_utf8_field_fails_decoding_instead_of_becoming_empty() {
    let reply = redis::Value::Array(vec![redis::Value::Array(vec![
        redis::Value::BulkString(b"events".to_vec()),
        redis::Value::Array(vec![redis::Value::Array(vec![
            redis::Value::BulkString(b"1-1".to_vec()),
            redis::Value::Array(vec![
                redis::Value::BulkString(b"payload".to_vec()),
                redis::Value::BulkString(b"\xff\xfe".to_vec()),
            ]),
        ])]),
    ])]);

    let err = entries_from_reply(reply).expect_err("invalid UTF-8 must fail decoding");

    assert!(
        err.to_string().contains("UTF-8"),
        "decode failure must name the UTF-8 failure: {err}"
    );
    assert!(
        !crate::transport_error::is_transient_redis_error(&err),
        "a field decode failure must be non-transient so supervision fires: {err}"
    );
}
#[tokio::test]
async fn recovery_delivers_claimed_entries_and_acks_after_success() {
    let fake = FakeRecoveryIo::new().with_recover_batches(vec![recovery_batch(
        Some(1),
        vec![recovered("1-1", &[("payload", "hello")], Some(3))],
        "0-0",
    )]);
    let handles = fake.handles();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&delivered);
    let deliver = move |entry: StreamEntry| {
        let seen = Arc::clone(&seen);
        async move {
            let exchange = build_exchange_from_stream_entry("stream", "g", "c", &entry);
            let count = exchange
                .input
                .header("CamelRedis.StreamDeliveryCount")
                .cloned();
            seen.lock().expect("delivered mutex").push(count);
            DeliveryOutcome::Completed
        }
    };
    let mut run = spawn_core(
        Box::new(fake),
        recovery_params(60_000),
        CancellationToken::new(),
        None,
        deliver,
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    assert!(matches!(
        recv_cmd(&mut run.cmd_rx).await,
        ReadTaskCmd::BatchDone
    ));
    let result = finish_ok(run).await;
    assert!(result.is_ok(), "session must end Ok: {result:?}");

    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec![Some(serde_json::json!("3"))],
        "the claimed entry must be delivered with its claimed delivery count"
    );
    assert_eq!(
        *handles.acks.lock().expect("acks mutex"),
        vec!["1-1".to_string()],
        "a successful delivery must be acknowledged exactly once on the recovery half"
    );
    assert_eq!(
        *handles.recover_budgets.lock().expect("budgets mutex"),
        vec![10],
        "the pass must claim within the configured recovery batch budget"
    );
}

#[tokio::test]
async fn recovery_failed_delivery_keeps_entry_pending() {
    let fake = FakeRecoveryIo::new().with_recover_batches(vec![recovery_batch(
        Some(2),
        vec![
            recovered("1-1", &[], Some(1)),
            recovered("2-2", &[], Some(1)),
        ],
        "0-0",
    )]);
    let handles = fake.handles();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&delivered);
    let deliver = move |entry: StreamEntry| {
        let seen = Arc::clone(&seen);
        async move {
            seen.lock().expect("delivered mutex").push(entry.id.clone());
            if entry.id == "1-1" {
                DeliveryOutcome::Failed
            } else {
                DeliveryOutcome::Completed
            }
        }
    };
    let mut run = spawn_core(
        Box::new(fake),
        recovery_params(60_000),
        CancellationToken::new(),
        None,
        deliver,
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    assert!(matches!(
        recv_cmd(&mut run.cmd_rx).await,
        ReadTaskCmd::BatchDone
    ));
    let result = finish_ok(run).await;
    assert!(result.is_ok(), "session must end Ok: {result:?}");

    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec!["1-1".to_string(), "2-2".to_string()],
        "a Failed entry must not stop the next claimed entry"
    );
    assert_eq!(
        *handles.acks.lock().expect("acks mutex"),
        vec!["2-2".to_string()],
        "only the accepted claimed entry may be acknowledged"
    );
}

#[tokio::test]
async fn recovery_channel_closed_fails_supervision() {
    let fake = FakeRecoveryIo::new().with_recover_batches(vec![recovery_batch(
        Some(1),
        vec![recovered("1-1", &[], Some(1))],
        "0-0",
    )]);
    let handles = fake.handles();
    let run = spawn_core(
        Box::new(fake),
        recovery_params(60_000),
        CancellationToken::new(),
        None,
        |_entry: StreamEntry| async { DeliveryOutcome::ChannelGone },
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    let result = join_session(run).await;

    match result {
        Err(CamelError::ChannelClosed) => {}
        other => panic!("ChannelGone must end the session with ChannelClosed: {other:?}"),
    }
    assert!(
        handles.acks.lock().expect("acks mutex").is_empty(),
        "ChannelGone must not acknowledge the claimed entry"
    );
}

#[tokio::test]
async fn recovery_cancel_between_entries_no_further_ack() {
    let fake = FakeRecoveryIo::new().with_recover_batches(vec![recovery_batch(
        Some(2),
        vec![
            recovered("1-1", &[], Some(1)),
            recovered("2-2", &[], Some(1)),
        ],
        "0-0",
    )]);
    let handles = fake.handles();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let delivered_changed = Arc::new(Notify::new());
    let seen = Arc::clone(&delivered);
    let changed = Arc::clone(&delivered_changed);
    let deliver = move |entry: StreamEntry| {
        let seen = Arc::clone(&seen);
        let changed = Arc::clone(&changed);
        async move {
            seen.lock().expect("delivered mutex").push(entry.id.clone());
            changed.notify_waiters();
            if entry.id == "2-2" {
                // Cancel arrives while the second claimed entry is in
                // flight; it must not be acknowledged.
                std::future::pending::<()>().await;
            }
            DeliveryOutcome::Completed
        }
    };
    let run = spawn_core(
        Box::new(fake),
        recovery_params(60_000),
        CancellationToken::new(),
        None,
        deliver,
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    wait_for_recovery_acks(&handles, 1).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        wait_until(&delivered_changed, || {
            delivered
                .lock()
                .expect("delivered mutex")
                .contains(&"2-2".to_string())
        }),
    )
    .await
    .expect("the second claimed entry must start delivery");
    let result = finish_ok(run).await;
    assert!(result.is_ok(), "session must end Ok: {result:?}");

    assert_eq!(
        *handles.acks.lock().expect("acks mutex"),
        vec!["1-1".to_string()],
        "cancellation before the second ack decision must leave it unacknowledged"
    );
}

#[tokio::test]
async fn recovery_cancel_while_claim_reply_withheld() {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let fake = FakeRecoveryIo::new().with_recover_gate(0, entered_tx, release_rx);
    let handles = fake.handles();
    let run = spawn_core(
        Box::new(fake),
        recovery_params(60_000),
        CancellationToken::new(),
        None,
        |_entry: StreamEntry| async { DeliveryOutcome::Completed },
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .expect("the recovery pass must begin")
        .expect("the gate sender must live");

    run.cancel.cancel();
    let result = join_session(run).await;
    // Keep the release sender alive until after the cancel is observed so
    // the withheld reply never resolves on its own.
    drop(release_tx);
    assert!(result.is_ok(), "cancelled session must end Ok: {result:?}");
    assert!(
        handles.acks.lock().expect("acks mutex").is_empty(),
        "a cancelled claim reply must not acknowledge anything"
    );
}

#[tokio::test]
async fn recovery_completion_after_cancel_no_ack() {
    let fake = FakeRecoveryIo::new().with_recover_batches(vec![recovery_batch(
        Some(1),
        vec![recovered("1-1", &[], Some(1))],
        "0-0",
    )]);
    let handles = fake.handles();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered_tx)));
    let release = Arc::new(Mutex::new(Some(release_rx)));
    let entered_for_deliver = Arc::clone(&entered);
    let release_for_deliver = Arc::clone(&release);
    let deliver = move |_entry: StreamEntry| {
        let entered = Arc::clone(&entered_for_deliver);
        let release = Arc::clone(&release_for_deliver);
        async move {
            if let Some(sender) = entered.lock().expect("entered mutex").take() {
                let _ = sender.send(());
            }
            let gate = release.lock().expect("release mutex").take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            DeliveryOutcome::Completed
        }
    };
    let run = spawn_core(
        Box::new(fake),
        recovery_params(60_000),
        CancellationToken::new(),
        None,
        deliver,
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .expect("delivery must begin")
        .expect("the entered sender must live");

    run.cancel.cancel();
    // The completion resolves only after the cancel won.
    let _ = release_tx.send(());
    let result = join_session(run).await;
    assert!(result.is_ok(), "cancelled session must end Ok: {result:?}");
    assert!(
        handles.acks.lock().expect("acks mutex").is_empty(),
        "a completion observed after cancel must not be acknowledged"
    );
}

#[tokio::test(start_paused = true)]
async fn recovery_empty_cursor_wraps() {
    let fake = FakeRecoveryIo::new().with_recover_batches(vec![
        recovery_batch(None, Vec::new(), "0-0"),
        recovery_batch(None, Vec::new(), "0-0"),
    ]);
    let handles = fake.handles();
    let mut run = spawn_core(
        Box::new(fake),
        recovery_params(1_000),
        CancellationToken::new(),
        None,
        |_entry: StreamEntry| async { DeliveryOutcome::Completed },
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    assert!(matches!(
        recv_cmd(&mut run.cmd_rx).await,
        ReadTaskCmd::BatchDone
    ));
    assert_eq!(
        handles.recover_calls.load(Ordering::SeqCst),
        1,
        "the initial pass must issue exactly one recover call"
    );

    tokio::time::advance(Duration::from_millis(1_000)).await;
    wait_for_recover_calls(&handles, 2).await;

    assert_eq!(
        *handles.recover_cursors.lock().expect("cursors mutex"),
        vec!["0-0".to_string(), "0-0".to_string()],
        "a wrapped 0-0 cursor is passed unchanged to the next pass"
    );
    assert_eq!(
        handles.recover_calls.load(Ordering::SeqCst),
        2,
        "each pass must issue exactly one recover call"
    );
    let result = finish_ok(run).await;
    assert!(result.is_ok(), "session must end Ok: {result:?}");
}

#[tokio::test(start_paused = true)]
async fn recovery_cursor_passed_across_passes_and_reconnect() {
    let fake = FakeRecoveryIo::new()
        .with_connect_outcomes(vec![Ok(()), Ok(())])
        .with_recover_outcomes(vec![
            Ok(recovery_batch(None, Vec::new(), "5-0")),
            Err(io_reset()),
            Ok(recovery_batch(None, Vec::new(), "5-0")),
        ]);
    let handles = fake.handles();
    let mut run = spawn_core(
        Box::new(fake),
        recovery_params(1_000),
        CancellationToken::new(),
        None,
        |_entry: StreamEntry| async { DeliveryOutcome::Completed },
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    assert!(matches!(
        recv_cmd(&mut run.cmd_rx).await,
        ReadTaskCmd::BatchDone
    ));

    tokio::time::advance(Duration::from_millis(1_000)).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        wait_until(&handles.connect_changed, || {
            handles.connects.load(Ordering::SeqCst) >= 2
        }),
    )
    .await
    .expect("the recovery half must reconnect after the transient error");
    wait_for_recover_calls(&handles, 3).await;

    let second_cursor = handles
        .recover_cursors
        .lock()
        .expect("cursors mutex")
        .get(2)
        .cloned();
    assert_eq!(
        second_cursor.as_deref(),
        Some("5-0"),
        "the retried pass after a recovery reconnect must resume from the retained cursor"
    );
    assert!(
        handles.connects.load(Ordering::SeqCst) >= 2,
        "a transient recovery error must reconnect the recovery half"
    );
    let result = finish_ok(run).await;
    assert!(result.is_ok(), "session must end Ok: {result:?}");
}

#[tokio::test]
async fn recovery_ack_transient_error_reconnects_recovery_half_only() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(vec![Ok(())], vec![]);
    let read_connects = io.connects.clone();
    let fake = FakeRecoveryIo::new()
        .with_recover_batches(vec![recovery_batch(
            Some(1),
            vec![recovered("1-1", &[], Some(1))],
            "0-0",
        )])
        .with_ack_outcomes(vec![Err(io_reset())]);
    let handles = fake.handles();
    let ready_count = Arc::new(AtomicUsize::new(0));
    let ready_for_callback = Arc::clone(&ready_count);
    let on_ready: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(move || {
        ready_for_callback.fetch_add(1, Ordering::SeqCst);
    }));
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let params = recovery_params(60_000);

    let handle = tokio::spawn(async move {
        stream_session(
            Box::new(io),
            Box::new(fake),
            Arc::new(topology),
            params,
            fast_policy(3),
            cancel_task,
            on_ready,
            |_entry: StreamEntry| async { DeliveryOutcome::Completed },
        )
        .await
    });

    // The transient ack error reconnects only the recovery half.
    tokio::time::timeout(
        Duration::from_secs(5),
        wait_until(&handles.connect_changed, || {
            handles.connects.load(Ordering::SeqCst) >= 2
        }),
    )
    .await
    .expect("the recovery half must reconnect after the transient ack error");

    assert!(
        handles.acks.lock().expect("acks mutex").is_empty(),
        "the unknown-outcome recovery ack must not be recorded"
    );
    assert_eq!(
        read_connects.load(Ordering::SeqCst),
        1,
        "a recovery ack reconnect must leave the read half untouched"
    );
    assert!(
        handles.connects.load(Ordering::SeqCst) >= 2,
        "the recovery half must reconnect under its own budget"
    );
    assert_eq!(
        ready_count.load(Ordering::SeqCst),
        1,
        "on_ready must fire exactly once across the recovery reconnect"
    );

    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");
    assert!(result.is_ok(), "session must end Ok: {result:?}");
}

#[tokio::test]
async fn recovery_empty_targeted_count_skips_entry() {
    let (capture, _guard) = capture_logs();
    let fake = FakeRecoveryIo::new().with_recover_batches(vec![recovery_batch(
        Some(2),
        vec![
            recovered("1-1", &[("payload", "skip")], None),
            recovered("2-2", &[("payload", "ok")], Some(1)),
        ],
        "0-0",
    )]);
    let handles = fake.handles();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&delivered);
    let deliver = move |entry: StreamEntry| {
        let seen = Arc::clone(&seen);
        async move {
            seen.lock().expect("delivered mutex").push(entry.id.clone());
            DeliveryOutcome::Completed
        }
    };
    let mut run = spawn_core(
        Box::new(fake),
        recovery_params(60_000),
        CancellationToken::new(),
        None,
        deliver,
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    assert!(matches!(
        recv_cmd(&mut run.cmd_rx).await,
        ReadTaskCmd::BatchDone
    ));
    let result = finish_ok(run).await;
    assert!(result.is_ok(), "session must end Ok: {result:?}");

    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec!["2-2".to_string()],
        "a claimed entry without a delivery count must be skipped"
    );
    assert_eq!(
        *handles.acks.lock().expect("acks mutex"),
        vec!["2-2".to_string()],
        "the skipped entry must not be acknowledged"
    );
    let events = capture.events();
    assert!(
        events.iter().any(|event| {
            event.message.contains("no delivery count") && event.field("id") == Some("1-1")
        }),
        "skipping a claimed entry must warn with its id: {events:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn recovery_pass_after_connect_and_by_deadline() {
    let fake = FakeRecoveryIo::new().with_recover_batches(vec![
        recovery_batch(None, Vec::new(), "0-0"),
        recovery_batch(None, Vec::new(), "0-0"),
    ]);
    let handles = fake.handles();
    let mut run = spawn_core(
        Box::new(fake),
        recovery_params(1_000),
        CancellationToken::new(),
        None,
        |_entry: StreamEntry| async { DeliveryOutcome::Completed },
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    assert!(matches!(
        recv_cmd(&mut run.cmd_rx).await,
        ReadTaskCmd::BatchDone
    ));
    assert_eq!(
        handles.recover_calls.load(Ordering::SeqCst),
        1,
        "one pass must run right after connect, before the first read"
    );

    tokio::time::advance(Duration::from_millis(1_000)).await;
    wait_for_recover_calls(&handles, 2).await;
    assert_eq!(
        handles.recover_calls.load(Ordering::SeqCst),
        2,
        "exactly one more pass must fire at the deadline"
    );

    let result = finish_ok(run).await;
    assert!(result.is_ok(), "session must end Ok: {result:?}");
}

#[tokio::test(start_paused = true)]
async fn recovery_deadline_fires_while_read_side_xack_withheld() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let (ack_entered_tx, ack_entered_rx) = oneshot::channel();
    let (ack_release_tx, ack_release_rx) = oneshot::channel();
    let io = FakeStreamIo::new(vec![Ok(())], vec![batch(vec![entry("1-1", &[])])])
        .with_ack_gate(ack_release_rx, ack_entered_tx);
    let va = io.acks.clone();
    let va_changed = io.ack_changed.clone();
    let fake = FakeRecoveryIo::new();
    let handles = fake.handles();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&delivered);
    let deliver = move |entry: StreamEntry| {
        let seen = Arc::clone(&seen);
        async move {
            seen.lock().expect("delivered mutex").push(entry.id.clone());
            DeliveryOutcome::Completed
        }
    };
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let params = recovery_params(1_000);

    let handle = tokio::spawn(async move {
        stream_session(
            Box::new(io),
            Box::new(fake),
            Arc::new(topology),
            params,
            fast_policy(3),
            cancel_task,
            None,
            deliver,
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(5), ack_entered_rx)
        .await
        .expect("the read-side ack must begin")
        .expect("the ack gate sender must live");
    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec!["1-1".to_string()],
        "the batch entry must be delivered once before the ack"
    );
    assert_eq!(
        handles.recover_calls.load(Ordering::SeqCst),
        1,
        "the initial recovery pass runs before the first read"
    );

    tokio::time::advance(Duration::from_millis(1_000)).await;
    wait_for_recover_calls(&handles, 2).await;
    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec!["1-1".to_string()],
        "a deadline pass must not restart the in-flight read batch"
    );

    let _ = ack_release_tx.send(());
    wait_for_acks(&va, &va_changed, 1).await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");
    assert!(result.is_ok(), "session must end Ok: {result:?}");
    assert_eq!(
        *va.lock().expect("acks mutex"),
        vec!["1-1".to_string()],
        "the withheld read-side ack must land once after release"
    );
}

#[tokio::test(start_paused = true)]
async fn recovery_deadline_fires_while_pipeline_completion_withheld() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let io = FakeStreamIo::new(vec![Ok(())], vec![]);
    let fake = FakeRecoveryIo::new().with_recover_batches(vec![recovery_batch(
        Some(2),
        vec![
            recovered("1-1", &[], Some(1)),
            recovered("2-2", &[], Some(1)),
        ],
        "0-0",
    )]);
    let handles = fake.handles();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered_tx)));
    let release = Arc::new(Mutex::new(Some(release_rx)));
    let seen = Arc::clone(&delivered);
    let entered_for_deliver = Arc::clone(&entered);
    let release_for_deliver = Arc::clone(&release);
    let deliver = move |entry: StreamEntry| {
        let seen = Arc::clone(&seen);
        let entered = Arc::clone(&entered_for_deliver);
        let release = Arc::clone(&release_for_deliver);
        async move {
            seen.lock().expect("delivered mutex").push(entry.id.clone());
            if entry.id == "1-1" {
                if let Some(sender) = entered.lock().expect("entered mutex").take() {
                    let _ = sender.send(());
                }
                let gate = release.lock().expect("release mutex").take();
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
            }
            DeliveryOutcome::Completed
        }
    };
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let params = recovery_params(1_000);

    let handle = tokio::spawn(async move {
        stream_session(
            Box::new(io),
            Box::new(fake),
            Arc::new(topology),
            params,
            fast_policy(3),
            cancel_task,
            None,
            deliver,
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .expect("the claimed delivery must begin")
        .expect("the entered sender must live");
    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec!["1-1".to_string()],
        "only one claimed entry may be in flight"
    );
    assert_eq!(handles.recover_calls.load(Ordering::SeqCst), 1);

    tokio::time::advance(Duration::from_millis(1_000)).await;
    wait_for_recover_calls(&handles, 2).await;
    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec!["1-1".to_string()],
        "the withheld delivery must not be dropped or restarted by a deadline pass"
    );

    let _ = release_tx.send(());
    wait_for_recovery_acks(&handles, 2).await;
    assert_eq!(
        *delivered.lock().expect("delivered mutex"),
        vec!["1-1".to_string(), "2-2".to_string()],
        "the next claimed entry must be delivered after the first completion settles"
    );
    assert_eq!(
        *handles.acks.lock().expect("acks mutex"),
        vec!["1-1".to_string(), "2-2".to_string()],
        "the preserved completion must be acknowledged before the next entry"
    );

    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");
    assert!(result.is_ok(), "session must end Ok: {result:?}");
}

#[tokio::test(start_paused = true)]
async fn recovery_deadline_fires_on_quiet_blocked_read() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
    let (entered_tx, entered_rx) = oneshot::channel();
    let io = FakeStreamIo::new(vec![Ok(())], vec![]).with_read_entered(entered_tx);
    let events = io.events.clone();
    let fake = FakeRecoveryIo::new();
    let handles = fake.handles();
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let mut params = recovery_params(1_000);
    params.block_ms = 0;

    let handle = tokio::spawn(async move {
        stream_session(
            Box::new(io),
            Box::new(fake),
            Arc::new(topology),
            params,
            fast_policy(3),
            cancel_task,
            None,
            |_entry: StreamEntry| async { DeliveryOutcome::Completed },
        )
        .await
    });

    // The read task is parked in a read that will not return.
    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .expect("the read must begin")
        .expect("the read gate sender must live");
    assert_eq!(
        handles.recover_calls.load(Ordering::SeqCst),
        1,
        "the initial pass must run before the first read"
    );

    // The deadline pass fires on the recovery half while the read is quiet.
    tokio::time::advance(Duration::from_millis(1_000)).await;
    wait_for_recover_calls(&handles, 2).await;
    assert_eq!(
        *events.lock().expect("events mutex"),
        vec!["read".to_string()],
        "the deadline pass must not end or restart the blocked read"
    );

    // Cancellation is the release that unblocks the read; the session
    // still tears down via its orderly Ok path.
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("session must finish")
        .expect("session task must not panic");
    assert!(result.is_ok(), "session must end Ok: {result:?}");
}

#[tokio::test]
async fn recovery_nogroup_fails_supervision() {
    let fake = FakeRecoveryIo::new().with_recover_outcomes(vec![Err(nogroup_error())]);
    let handles = fake.handles();
    let run = spawn_core(
        Box::new(fake),
        recovery_params(60_000),
        CancellationToken::new(),
        None,
        |_entry: StreamEntry| async { DeliveryOutcome::Completed },
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    let result = join_session(run).await;

    assert!(
        result.is_err(),
        "a NOGROUP recovery error must fail supervision, got: {result:?}"
    );
    assert_eq!(
        handles.recover_calls.load(Ordering::SeqCst),
        1,
        "a NOGROUP recovery error must not be retried"
    );
    assert_eq!(
        handles.connects.load(Ordering::SeqCst),
        1,
        "a NOGROUP recovery error must not reconnect"
    );
}

#[tokio::test]
async fn session_core_fires_on_ready_on_connected_and_recovers_before_first_read() {
    // Case 1: recovery enabled; the first pass runs before BatchDone.
    let (recover_entered_tx, recover_entered_rx) = oneshot::channel();
    let (recover_release_tx, recover_release_rx) = oneshot::channel();
    let fake = FakeRecoveryIo::new().with_recover_gate(0, recover_entered_tx, recover_release_rx);
    let handles = fake.handles();
    let ready_count = Arc::new(AtomicUsize::new(0));
    let ready_for_callback = Arc::clone(&ready_count);
    let on_ready: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(move || {
        ready_for_callback.fetch_add(1, Ordering::SeqCst);
    }));
    let mut run = spawn_core(
        Box::new(fake),
        recovery_params(60_000),
        CancellationToken::new(),
        on_ready,
        |_entry: StreamEntry| async { DeliveryOutcome::Completed },
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    tokio::time::timeout(Duration::from_secs(5), recover_entered_rx)
        .await
        .expect("the recovery pass must begin")
        .expect("the gate sender must live");

    assert_eq!(
        ready_count.load(Ordering::SeqCst),
        1,
        "on_ready must fire exactly once on Connected"
    );
    assert!(
        matches!(run.cmd_rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "no BatchDone may be sent before the first recovery pass completes"
    );

    let _ = recover_release_tx.send(());
    assert!(
        matches!(recv_cmd(&mut run.cmd_rx).await, ReadTaskCmd::BatchDone),
        "the start BatchDone must arrive after the first recovery pass"
    );
    assert_eq!(handles.recover_calls.load(Ordering::SeqCst), 1);
    let result = finish_ok(run).await;
    assert!(result.is_ok(), "session must end Ok: {result:?}");

    // Case 2: recovery disabled; only on_ready then BatchDone.
    let fake = FakeRecoveryIo::new();
    let disabled_handles = fake.handles();
    let ready_count = Arc::new(AtomicUsize::new(0));
    let ready_for_callback = Arc::clone(&ready_count);
    let on_ready: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(move || {
        ready_for_callback.fetch_add(1, Ordering::SeqCst);
    }));
    let mut run = spawn_core(
        Box::new(fake),
        stream_params(),
        CancellationToken::new(),
        on_ready,
        |_entry: StreamEntry| async { DeliveryOutcome::Completed },
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    assert!(
        matches!(recv_cmd(&mut run.cmd_rx).await, ReadTaskCmd::BatchDone),
        "a disabled session must send BatchDone immediately after Connected"
    );
    assert_eq!(
        ready_count.load(Ordering::SeqCst),
        1,
        "on_ready must fire exactly once with recovery disabled"
    );
    assert_eq!(
        disabled_handles.recover_calls.load(Ordering::SeqCst),
        0,
        "a disabled session must never issue a recovery command"
    );
    let result = finish_ok(run).await;
    assert!(result.is_ok(), "session must end Ok: {result:?}");
}

#[tokio::test(start_paused = true)]
async fn no_recovery_commands_when_disabled() {
    let fake = FakeRecoveryIo::new().with_recover_batches(vec![recovery_batch(
        Some(1),
        vec![recovered("1-1", &[], Some(1))],
        "0-0",
    )]);
    let handles = fake.handles();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&delivered);
    let deliver = move |entry: StreamEntry| {
        let seen = Arc::clone(&seen);
        async move {
            seen.lock().expect("delivered mutex").push(entry.id.clone());
            DeliveryOutcome::Completed
        }
    };
    let mut run = spawn_core(
        Box::new(fake),
        stream_params(),
        CancellationToken::new(),
        None,
        deliver,
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    assert!(matches!(
        recv_cmd(&mut run.cmd_rx).await,
        ReadTaskCmd::BatchDone
    ));

    tokio::time::advance(Duration::from_secs(600)).await;
    let result = finish_ok(run).await;
    assert!(result.is_ok(), "session must end Ok: {result:?}");
    assert_eq!(
        handles.recover_calls.load(Ordering::SeqCst),
        0,
        "recovery disabled must issue no recover call"
    );
    assert!(
        handles.acks.lock().expect("acks mutex").is_empty(),
        "recovery disabled must issue no recover_ack call"
    );
    assert!(
        delivered.lock().expect("delivered mutex").is_empty(),
        "recovery disabled must not deliver claimed entries"
    );
}

#[tokio::test]
async fn recovery_pass_logs_pending_count() {
    let (capture, _guard) = capture_logs();
    let fake = FakeRecoveryIo::new().with_recover_batches(vec![recovery_batch(
        Some(2),
        Vec::new(),
        "0-0",
    )]);
    let mut run = spawn_core(
        Box::new(fake),
        recovery_params(60_000),
        CancellationToken::new(),
        None,
        |_entry: StreamEntry| async { DeliveryOutcome::Completed },
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    assert!(matches!(
        recv_cmd(&mut run.cmd_rx).await,
        ReadTaskCmd::BatchDone
    ));
    let result = finish_ok(run).await;
    assert!(result.is_ok(), "session must end Ok: {result:?}");

    let summaries: Vec<CapturedEvent> = capture
        .events()
        .into_iter()
        .filter(|event| event.message.contains("recovery pass completed"))
        .collect();
    assert_eq!(
        summaries.len(),
        1,
        "exactly one pass-summary event must be logged: {summaries:?}"
    );
    let summary = &summaries[0];
    assert_eq!(
        summary.field("pending"),
        Some("2"),
        "the pass summary must carry the actual pending count, not its Option wrapper"
    );
    assert_eq!(
        summary.field("claimed"),
        Some("0"),
        "the pass summary must carry the claimed count"
    );
}

#[tokio::test]
async fn recovery_ack_non_transient_error_fails_supervision() {
    let fake = FakeRecoveryIo::new()
        .with_recover_batches(vec![recovery_batch(
            Some(1),
            vec![recovered("1-1", &[], Some(1))],
            "0-0",
        )])
        .with_ack_outcomes(vec![Err(nogroup_error())]);
    let handles = fake.handles();
    let run = spawn_core(
        Box::new(fake),
        recovery_params(60_000),
        CancellationToken::new(),
        None,
        |_entry: StreamEntry| async { DeliveryOutcome::Completed },
    );

    run.msg_tx
        .send(ReadTaskMsg::Connected)
        .await
        .expect("the session must accept Connected");
    let result = join_session(run).await;

    assert!(
        result.is_err(),
        "a non-transient recovery ack error must fail supervision, got: {result:?}"
    );
    assert_eq!(
        handles.recover_calls.load(Ordering::SeqCst),
        1,
        "a failed ack must not trigger another recovery pass"
    );
    assert_eq!(
        handles.connects.load(Ordering::SeqCst),
        1,
        "a non-transient ack error must not reconnect the recovery half"
    );
    assert!(
        handles.acks.lock().expect("acks mutex").is_empty(),
        "the failed ack must not be recorded as acknowledged"
    );
}
