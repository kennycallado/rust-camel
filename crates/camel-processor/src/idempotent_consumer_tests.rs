use super::*;
use async_trait::async_trait;
use camel_api::metrics::MetricsCollector;
use camel_api::{CamelError, Exchange, Message, Value};
use camel_component_api::health_registry::HealthCheckRegistry;
use camel_component_api::test_support::NoopRuntimeObservability;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::Mutex;

// ── Test helpers ──

/// In-memory mock repository backed by a `Mutex<HashSet>`. Allows tests
/// to pre-populate keys and inspect state after the segment runs.
#[derive(Debug, Default)]
struct MockRepo {
    keys: Mutex<HashSet<String>>,
    fail_contains: bool,
    /// `add` reports the key already exists even for an absent key,
    /// emulating the eager reservation race without pre-adding the key
    /// (pre-adding would take the contains-hit path instead).
    force_add_duplicate: bool,
    /// `add` fails with a synthetic error.
    fail_add: bool,
    /// `remove` fails with a synthetic error.
    fail_remove: bool,
    /// Ordered record of repository operations, for race-ordering asserts.
    ops: Mutex<Vec<&'static str>>,
    /// Parks the next `remove` mid-operation (raced-delivery tests only).
    remove_gate: std::sync::Mutex<Option<OpGate>>,
    /// Parks the next `add` mid-operation (raced-delivery tests only).
    add_gate: std::sync::Mutex<Option<OpGate>>,
}

/// Test-only rendezvous that parks one repository operation mid-flight.
///
/// When armed, the operation records its op-log entry, signals `entered`,
/// then awaits release. The test holds the matching [`OpGateHandle`], so the
/// interleaving is driven by handshakes and never by sleeping.
struct OpGate {
    entered: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

impl std::fmt::Debug for OpGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OpGate")
    }
}

/// Test-side end of an [`OpGate`]: await [`Self::entered`] to learn the
/// operation reached the gate, then [`Self::release`] to let it finish.
struct OpGateHandle {
    entered: Option<tokio::sync::oneshot::Receiver<()>>,
    release: tokio::sync::oneshot::Sender<()>,
}

impl OpGateHandle {
    /// Bounded await of the operation's entered handshake.
    async fn entered(&mut self) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        let receiver = self
            .entered
            .take()
            .expect("entered handshake is awaited exactly once");
        tokio::time::timeout(std::time::Duration::from_secs(5), receiver)
            .await
            .expect("gated repository operation must signal entered")
    }

    /// Let the parked operation proceed.
    fn release(self) {
        // The receiver may be gone if the operation's future was dropped;
        // that is a test bug, not a release failure.
        let _ = self.release.send(());
    }
}

impl MockRepo {
    fn new() -> Self {
        Self::default()
    }

    /// Arm the `remove` gate: the next `remove` records its op, signals the
    /// returned handle, then blocks until the handle is released.
    fn arm_remove_gate(&self) -> OpGateHandle {
        Self::arm(&self.remove_gate)
    }

    /// Arm the `add` gate: the next `add` records its op, signals the
    /// returned handle, then blocks until the handle is released.
    fn arm_add_gate(&self) -> OpGateHandle {
        Self::arm(&self.add_gate)
    }

    fn arm(slot: &std::sync::Mutex<Option<OpGate>>) -> OpGateHandle {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let mut guard = slot.lock().expect("op gate lock poisoned");
        *guard = Some(OpGate {
            entered: entered_tx,
            release: release_rx,
        });
        OpGateHandle {
            entered: Some(entered_rx),
            release: release_tx,
        }
    }

    /// Take a once-armed gate, signal that the operation entered it, and wait
    /// for the test to release it. A no-op when the slot is not armed.
    async fn pass_gate(slot: &std::sync::Mutex<Option<OpGate>>) {
        let gate = slot.lock().expect("op gate lock poisoned").take();
        if let Some(gate) = gate {
            let _ = gate.entered.send(());
            let _ = gate.release.await;
        }
    }

    fn failing_contains() -> Self {
        Self {
            fail_contains: true,
            ..Self::default()
        }
    }

    fn race_duplicate() -> Self {
        Self {
            force_add_duplicate: true,
            ..Self::default()
        }
    }

    fn failing_add() -> Self {
        Self {
            fail_add: true,
            ..Self::default()
        }
    }

    fn failing_remove() -> Self {
        Self {
            fail_remove: true,
            ..Self::default()
        }
    }

    async fn pre_add(&self, key: &str) {
        self.keys.lock().await.insert(key.to_string());
    }

    async fn contains_key(&self, key: &str) -> bool {
        self.keys.lock().await.contains(key)
    }

    async fn ops(&self) -> Vec<&'static str> {
        self.ops.lock().await.clone()
    }

    async fn record_op(&self, op: &'static str) {
        self.ops.lock().await.push(op);
    }
}

#[async_trait]
impl IdempotentRepository for MockRepo {
    fn name(&self) -> &str {
        "mock"
    }

    async fn contains(&self, key: &str) -> Result<bool, CamelError> {
        self.record_op("contains").await;
        if self.fail_contains {
            return Err(CamelError::ProcessorError(
                "synthetic contains failure".into(),
            ));
        }
        Ok(self.keys.lock().await.contains(key))
    }

    async fn add(&self, key: &str) -> Result<bool, CamelError> {
        self.record_op("add").await;
        Self::pass_gate(&self.add_gate).await;
        if self.fail_add {
            return Err(CamelError::ProcessorError("synthetic add failure".into()));
        }
        if self.force_add_duplicate {
            return Ok(false);
        }
        let mut guard = self.keys.lock().await;
        Ok(guard.insert(key.to_string()))
    }

    async fn remove(&self, key: &str) -> Result<(), CamelError> {
        self.record_op("remove").await;
        Self::pass_gate(&self.remove_gate).await;
        if self.fail_remove {
            return Err(CamelError::ProcessorError(
                "synthetic remove failure".into(),
            ));
        }
        self.keys.lock().await.remove(key);
        Ok(())
    }

    async fn clear(&self) -> Result<(), CamelError> {
        self.keys.lock().await.clear();
        Ok(())
    }
}

/// Invocation counter shared by every clone of a [`ScriptedChild`].
///
/// Clones of a segment clone the child through `clone_box`, so the counter
/// must be shared rather than copied: the raced-delivery tests assert
/// "exactly once" after both runs complete, and a `bool` cannot distinguish
/// one invocation from two. Reads are `SeqCst` so the assertion observes the
/// child's increment as soon as its `run` starts.
#[derive(Debug, Default)]
struct InvocationCounter {
    calls: std::sync::atomic::AtomicUsize,
}

impl InvocationCounter {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn record(&self) {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Exact number of child invocations across every clone.
    fn count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Boolean view used by the single-delivery tests.
    fn was_invoked(&self) -> bool {
        self.count() > 0
    }
}

/// Test segment that returns a configurable outcome and records whether
/// it was invoked. Used to verify the segment's child-dispatch behaviour.
struct ScriptedChild {
    outcome: PipelineOutcome,
    invoked: Arc<InvocationCounter>,
}

impl OutcomePipeline for ScriptedChild {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        // Clone-safe on purpose: raced-delivery tests clone the owning
        // segment, and every clone must share the invocation counter (and
        // count separately) while scripting its own outcome.
        Box::new(Self {
            outcome: self.outcome.clone(),
            invoked: Arc::clone(&self.invoked),
        })
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        self.invoked.record();
        let outcome = std::mem::replace(
            &mut self.outcome,
            PipelineOutcome::Completed(Exchange::new(Message::new(""))),
        );
        Box::pin(async move { outcome_with_exchange(outcome, exchange) })
    }
}

/// Replace the placeholder exchange inside a scripted outcome with the
/// actual exchange passed to `run`. Keeps the variant, swaps the payload.
fn outcome_with_exchange(outcome: PipelineOutcome, exchange: Exchange) -> PipelineOutcome {
    match outcome {
        PipelineOutcome::Completed(_) => PipelineOutcome::Completed(exchange),
        PipelineOutcome::Stopped(_) => PipelineOutcome::Stopped(exchange),
        PipelineOutcome::Failed(e) => PipelineOutcome::Failed(e),
    }
}

fn exchange_with_id(id: &str) -> Exchange {
    let mut ex = Exchange::new(Message::new("payload"));
    ex.input.set_header("messageId", Value::String(id.into()));
    ex
}

fn header_message_id() -> MessageIdSource {
    MessageIdSource::Sync(Arc::new(|ex: &Exchange| {
        ex.input
            .header("messageId")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
    }))
}

fn build_segment(
    repo: Arc<MockRepo>,
    child_outcome: PipelineOutcome,
    eager: bool,
    remove_on_failure: bool,
) -> (IdempotentConsumerSegment, Arc<InvocationCounter>) {
    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
    build_segment_with_rt(repo, child_outcome, eager, remove_on_failure, rt)
}

fn build_segment_with_rt(
    repo: Arc<MockRepo>,
    child_outcome: PipelineOutcome,
    eager: bool,
    remove_on_failure: bool,
    rt: Arc<dyn RuntimeObservability>,
) -> (IdempotentConsumerSegment, Arc<InvocationCounter>) {
    let invoked = InvocationCounter::new();
    let child = ScriptedChild {
        outcome: child_outcome,
        invoked: invoked.clone(),
    };
    let segment = IdempotentConsumerSegment::new(
        repo,
        header_message_id(),
        OutcomeSegment::new(Box::new(child)),
        eager,
        remove_on_failure,
        OnMissingKey::Fail,
        rt,
    );
    (segment, invoked)
}

// ── Metrics test double ──

/// Records every `record_counter` call for test assertions.
type CounterRecording = Vec<(String, f64, Vec<(String, String)>)>;

#[derive(Clone, Default)]
struct RecordingMetrics {
    counters: Arc<std::sync::Mutex<CounterRecording>>,
}

impl RecordingMetrics {
    fn counters(&self) -> CounterRecording {
        self.counters
            .lock()
            .expect("metrics sink lock poisoned")
            .clone()
    }
}

impl MetricsCollector for RecordingMetrics {
    fn record_exchange_duration(&self, _route_id: &str, _duration: std::time::Duration) {}
    fn increment_errors(&self, _route_id: &str, _error_type: &str) {}
    fn increment_exchanges(&self, _route_id: &str) {}
    fn set_queue_depth(&self, _route_id: &str, _depth: usize) {}
    fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}
    fn record_counter(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        self.counters
            .lock()
            .expect("metrics sink lock poisoned")
            .push((
                name.to_string(),
                value,
                labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ));
    }
}

struct RecordingRt {
    collector: Arc<RecordingMetrics>,
}

impl RuntimeObservability for RecordingRt {
    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        self.collector.clone()
    }

    fn health(&self) -> Arc<dyn HealthCheckRegistry> {
        Arc::new(NoopRuntimeObservability)
    }
}

// ── Tracing capture helper for warn-log assertions ──

/// One captured tracing event: its level and the ANSI-stripped fmt line.
struct CapturedEvent {
    level: tracing::Level,
    message: String,
}

impl CapturedEvent {
    fn is_warn(&self) -> bool {
        self.level == tracing::Level::WARN
    }

    fn is_error(&self) -> bool {
        self.level == tracing::Level::ERROR
    }
}

/// `MakeWriter` that appends formatted events to a shared byte sink.
#[derive(Clone)]
struct CapturingWriter {
    sink: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl std::io::Write for CapturingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sink
            .lock()
            .expect("capture sink lock poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturingWriter {
    type Writer = CapturingWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Install a bare registry as the process-global tracing default, once per
/// test binary. `tracing` caches a callsite's `Interest` process-wide from
/// its FIRST macro execution; subscriber-less sibling tests in this binary
/// can cache `Interest::never` for the shared `warn!` callsite, silently
/// dropping a later thread-local capture. The global registry heals prior
/// poison and floors future evaluations at `sometimes` (fix pattern:
/// c3853198; bd rc-img5). See `docs/testing/tracing-capture-guards.md`.
fn ensure_global_tracing_default() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INIT.set(()).is_ok() {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    }
}

/// Install a thread-local capture subscriber and return its sink plus the
/// guard that keeps it active.
///
/// Capture is thread-local, so it is safe next to other tests; only the
/// `set_global_default` call above is process-wide and first-wins. The
/// caller reads the sink with [`captured_events`] before dropping the
/// guard.
fn capture_events() -> (
    Arc<std::sync::Mutex<Vec<u8>>>,
    tracing::subscriber::DefaultGuard,
) {
    ensure_global_tracing_default();
    let sink = Arc::new(std::sync::Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        // TRACE keeps a downgraded `debug!` visible as a DEBUG event, so a
        // level regression fails the WARN count instead of vanishing.
        .with_max_level(tracing::Level::TRACE)
        .with_writer(CapturingWriter {
            sink: Arc::clone(&sink),
        })
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (sink, guard)
}

/// Parse the captured fmt output into `(level, message)` pairs.
fn captured_events(sink: &std::sync::Mutex<Vec<u8>>) -> Vec<CapturedEvent> {
    let raw = sink.lock().expect("capture sink lock poisoned");
    let text = String::from_utf8_lossy(&raw);
    text.lines().filter_map(parse_event).collect()
}

fn parse_event(line: &str) -> Option<CapturedEvent> {
    const LEVELS: [(&str, tracing::Level); 5] = [
        ("TRACE", tracing::Level::TRACE),
        ("DEBUG", tracing::Level::DEBUG),
        ("INFO", tracing::Level::INFO),
        ("WARN", tracing::Level::WARN),
        ("ERROR", tracing::Level::ERROR),
    ];
    let line = line.trim_start();
    let (level, rest) = LEVELS
        .iter()
        .find_map(|(token, level)| line.strip_prefix(token).map(|rest| (*level, rest)))?;
    Some(CapturedEvent {
        level,
        message: rest.trim_start().to_string(),
    })
}

// ── Test 1: duplicate key short-circuits to Completed without touching child ──
#[tokio::test]
async fn duplicate_key_returns_completed_without_running_child() {
    let repo = Arc::new(MockRepo::new());
    repo.pre_add("dup-1").await;
    let (mut segment, child_invoked) = build_segment(
        repo.clone(),
        PipelineOutcome::Failed(stub_error()),
        false,
        false,
    );

    let ex = exchange_with_id("dup-1");
    let outcome = segment.run(ex).await;

    assert!(matches!(outcome, PipelineOutcome::Completed(_)));
    assert!(
        !child_invoked.was_invoked(),
        "child must NOT run when key is a duplicate"
    );
    // key still present
    assert!(repo.contains_key("dup-1").await);
}

// ── Test 2: new key runs the child and returns its outcome ──
#[tokio::test]
async fn new_key_runs_child_and_returns_child_outcome() {
    let repo = Arc::new(MockRepo::new());
    let (mut segment, child_invoked) = build_segment(
        repo.clone(),
        PipelineOutcome::Completed(Exchange::new(Message::new(""))),
        false,
        false,
    );

    let ex = exchange_with_id("new-1");
    let outcome = segment.run(ex).await;

    assert!(matches!(outcome, PipelineOutcome::Completed(_)));
    assert!(
        child_invoked.was_invoked(),
        "child MUST run when key is new"
    );
    // Non-eager mode: key added after child Completed.
    assert!(
        repo.contains_key("new-1").await,
        "non-eager mode must add key after successful child run"
    );
}

// ── Test 3: failed repo read propagates as Failed (contract C1) ──
#[tokio::test]
async fn failed_repo_read_propagates_error() {
    let repo = Arc::new(MockRepo::failing_contains());
    let (mut segment, child_invoked) = build_segment(
        repo,
        PipelineOutcome::Completed(stub_exchange()),
        false,
        false,
    );

    let ex = exchange_with_id("any");
    let outcome = segment.run(ex).await;

    match outcome {
        PipelineOutcome::Failed(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("synthetic contains failure"),
                "expected synthetic failure in error, got: {msg}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(
        !child_invoked.was_invoked(),
        "child must NOT run when repo read fails"
    );
}

// ── Test 4: Stopped from child propagates as Stopped (segment-mode contract) ──
#[tokio::test]
async fn stopped_child_propagates_stopped() {
    let repo = Arc::new(MockRepo::new());
    let (mut segment, child_invoked) = build_segment(
        repo.clone(),
        PipelineOutcome::Stopped(stub_exchange()),
        false,
        false,
    );

    let ex = exchange_with_id("stop-1");
    let outcome = segment.run(ex).await;

    assert!(
        matches!(outcome, PipelineOutcome::Stopped(_)),
        "Stopped from child MUST propagate as Stopped (segment-mode contract)"
    );
    assert!(
        child_invoked.was_invoked(),
        "child must run before its Stopped can propagate"
    );
    // Stopped path must NOT register the key (pipeline terminating).
    assert!(
        !repo.contains_key("stop-1").await,
        "Stopped outcome must not register the key"
    );
}

// ── Test 5: eager mode + remove_on_failure rolls back key on Failed ──
#[tokio::test]
async fn eager_mode_removes_key_on_failure_when_configured() {
    let repo = Arc::new(MockRepo::new());
    let (mut segment, _child_invoked) = build_segment(
        repo.clone(),
        PipelineOutcome::Failed(stub_error()),
        true, // eager
        true, // remove_on_failure
    );

    let ex = exchange_with_id("eager-fail");
    let outcome = segment.run(ex).await;

    assert!(matches!(outcome, PipelineOutcome::Failed(_)));
    assert!(
        !repo.contains_key("eager-fail").await,
        "eager + remove_on_failure must roll back the key on failure"
    );
}

fn stub_exchange() -> Exchange {
    Exchange::new(Message::new(""))
}

fn stub_error() -> CamelError {
    CamelError::ProcessorError("child failed".into())
}

// ── Fallible MessageIdSource (task 1.5) ─────────────────────────────

#[tokio::test]
async fn message_id_async_error_propagates() {
    use camel_api::BoxValueFuture;

    let repo = Arc::new(MockRepo::new());
    let invoked = InvocationCounter::new();
    let child = ScriptedChild {
        outcome: PipelineOutcome::Completed(Exchange::new(Message::new(""))),
        invoked: invoked.clone(),
    };

    let source = MessageIdSource::Async(Arc::new(|_: &Exchange| {
        Box::pin(async { Err(CamelError::ProcessorError("id boom".into())) }) as BoxValueFuture
    }));
    let mut segment = IdempotentConsumerSegment::new(
        repo.clone(),
        source,
        OutcomeSegment::new(Box::new(child)),
        false,
        false,
        OnMissingKey::Fail,
        Arc::new(NoopRuntimeObservability),
    );

    let outcome = segment.run(exchange_with_id("any")).await;

    assert!(
        matches!(outcome, PipelineOutcome::Failed(ref e) if e.to_string().contains("id boom")),
        "an async message-id failure must fail the step"
    );
    assert!(
        !invoked.was_invoked(),
        "child must NOT run when the message-id expression fails"
    );
    assert!(!repo.contains_key("any").await);
}

// ── on_missing_key: parse matrix ─────────────────────────────────────

#[test]
fn on_missing_key_parse_matrix() {
    assert_eq!(
        OnMissingKey::parse(None).expect("None is the default"),
        OnMissingKey::Fail
    );
    assert_eq!(
        OnMissingKey::parse(Some("fail")).expect("fail parses"),
        OnMissingKey::Fail
    );
    assert_eq!(
        OnMissingKey::parse(Some("process")).expect("process parses"),
        OnMissingKey::Process
    );
    let err = OnMissingKey::parse(Some("skip"))
        .expect_err("invalid on_missing_key must be rejected")
        .to_string();
    assert!(
        err.contains("on_missing_key"),
        "error must name the field: {err}"
    );
    assert!(err.contains("skip"), "error must name the value: {err}");
    assert!(
        err.contains("\"fail\"") && err.contains("\"process\""),
        "error must name the accepted values: {err}"
    );
}

// ── on_missing_key: fail is the default (sync + async arms) ─────────

#[tokio::test]
async fn missing_key_fail_is_default_sync_arm() {
    let repo = Arc::new(MockRepo::new());
    let (mut segment, child_invoked) = build_segment(
        repo.clone(),
        PipelineOutcome::Completed(stub_exchange()),
        false,
        false,
    );

    // Exchange WITHOUT a messageId header → the sync extractor returns None.
    let outcome = segment.run(Exchange::new(Message::new("no-id"))).await;

    assert!(
        matches!(
            outcome,
            PipelineOutcome::Failed(ref e)
                if e.to_string().contains("idempotent_consumer")
                    && e.to_string().contains("mock")
        ),
        "missing key under fail must name the step and the repository"
    );
    assert!(
        !child_invoked.was_invoked(),
        "child must NOT run when the key is missing under fail"
    );
    assert!(
        !repo.contains_key("").await,
        "a missing key must never be registered as an empty key"
    );
}

#[tokio::test]
async fn missing_key_fail_is_default_async_arm() {
    use camel_api::BoxValueFuture;

    let repo = Arc::new(MockRepo::new());
    let invoked = InvocationCounter::new();
    let child = ScriptedChild {
        outcome: PipelineOutcome::Completed(stub_exchange()),
        invoked: invoked.clone(),
    };
    let source = MessageIdSource::Async(Arc::new(|_: &Exchange| {
        Box::pin(async { Ok(Value::Null) }) as BoxValueFuture
    }));
    let mut segment = IdempotentConsumerSegment::new(
        repo,
        source,
        OutcomeSegment::new(Box::new(child)),
        false,
        false,
        OnMissingKey::Fail,
        Arc::new(NoopRuntimeObservability),
    );

    let outcome = segment.run(exchange_with_id("any")).await;

    assert!(
        matches!(
            outcome,
            PipelineOutcome::Failed(ref e)
                if e.to_string().contains("idempotent_consumer")
                    && e.to_string().contains("mock")
        ),
        "null async key under fail must name the step and the repository"
    );
    assert!(
        !invoked.was_invoked(),
        "child must NOT run when the async key is null under fail"
    );
}

// ── on_missing_key: empty string is treated as an absent key ────────

#[tokio::test]
async fn empty_key_treated_as_absent_sync_arm() {
    let repo = Arc::new(MockRepo::new());
    let invoked = InvocationCounter::new();
    let child = ScriptedChild {
        outcome: PipelineOutcome::Completed(stub_exchange()),
        invoked: invoked.clone(),
    };
    let source = MessageIdSource::Sync(Arc::new(|_: &Exchange| Some(String::new())));
    let mut segment = IdempotentConsumerSegment::new(
        repo.clone(),
        source,
        OutcomeSegment::new(Box::new(child)),
        false,
        false,
        OnMissingKey::Fail,
        Arc::new(NoopRuntimeObservability),
    );

    let outcome = segment.run(Exchange::new(Message::new("empty-key"))).await;

    assert!(
        matches!(
            outcome,
            PipelineOutcome::Failed(ref e)
                if e.to_string().contains("idempotent_consumer")
                    && e.to_string().contains("mock")
        ),
        "Some(\"\") must behave exactly like an absent key under fail"
    );
    assert!(
        !invoked.was_invoked(),
        "child must NOT run when the sync key is empty under fail"
    );
    assert!(
        !repo.contains_key("").await,
        "an empty-string key must never be registered in the repository"
    );
}

#[tokio::test]
async fn empty_key_treated_as_absent_async_arm() {
    use camel_api::BoxValueFuture;

    // `failing_contains` makes any repository access fail loudly.
    let repo = Arc::new(MockRepo::failing_contains());
    let invoked = InvocationCounter::new();
    let child = ScriptedChild {
        outcome: PipelineOutcome::Completed(stub_exchange()),
        invoked: invoked.clone(),
    };
    let source = MessageIdSource::Async(Arc::new(|_: &Exchange| {
        Box::pin(async { Ok(Value::String(String::new())) }) as BoxValueFuture
    }));
    let mut segment = IdempotentConsumerSegment::new(
        repo,
        source,
        OutcomeSegment::new(Box::new(child)),
        false,
        false,
        OnMissingKey::Process,
        Arc::new(NoopRuntimeObservability),
    );

    let outcome = segment.run(exchange_with_id("any")).await;

    assert!(
        matches!(outcome, PipelineOutcome::Completed(_)),
        "an empty async key under process must forward to the child"
    );
    assert!(
        invoked.was_invoked(),
        "child must run when the async key is empty under process"
    );
}

// ── on_missing_key: process forwards on both arms, repo untouched ───

#[tokio::test]
async fn missing_key_process_forwards_with_warning() {
    use camel_api::BoxValueFuture;

    // Sync arm: extractor returns None; `failing_contains` proves the
    // repository is never consulted.
    let sync_repo = Arc::new(MockRepo::failing_contains());
    let sync_invoked = InvocationCounter::new();
    let sync_child = ScriptedChild {
        outcome: PipelineOutcome::Completed(stub_exchange()),
        invoked: sync_invoked.clone(),
    };
    let sync_source = MessageIdSource::Sync(Arc::new(|_: &Exchange| None));
    let mut sync_segment = IdempotentConsumerSegment::new(
        sync_repo,
        sync_source,
        OutcomeSegment::new(Box::new(sync_child)),
        false,
        false,
        OnMissingKey::Process,
        Arc::new(NoopRuntimeObservability),
    );

    let (sync_sink, sync_guard) = capture_events();
    let sync_outcome = sync_segment.run(Exchange::new(Message::new("no-id"))).await;
    assert!(
        matches!(sync_outcome, PipelineOutcome::Completed(_)),
        "sync missing key under process must complete"
    );
    assert!(
        sync_invoked.was_invoked(),
        "child must run for a missing sync key under process"
    );
    let sync_warns: Vec<String> = captured_events(&sync_sink)
        .into_iter()
        .filter(CapturedEvent::is_warn)
        .map(|event| event.message)
        .collect();
    drop(sync_guard);
    assert_eq!(
        sync_warns.len(),
        1,
        "sync arm must emit exactly one WARN: {sync_warns:?}"
    );
    assert!(
        sync_warns[0].contains("on_missing_key = 'process'"),
        "sync WARN must name the process policy, got: {:?}",
        sync_warns[0]
    );

    // Async arm: expression evaluates to null; repository untouched.
    let async_repo = Arc::new(MockRepo::failing_contains());
    let async_invoked = InvocationCounter::new();
    let async_child = ScriptedChild {
        outcome: PipelineOutcome::Completed(stub_exchange()),
        invoked: async_invoked.clone(),
    };
    let async_source = MessageIdSource::Async(Arc::new(|_: &Exchange| {
        Box::pin(async { Ok(Value::Null) }) as BoxValueFuture
    }));
    let mut async_segment = IdempotentConsumerSegment::new(
        async_repo,
        async_source,
        OutcomeSegment::new(Box::new(async_child)),
        false,
        false,
        OnMissingKey::Process,
        Arc::new(NoopRuntimeObservability),
    );

    let (async_sink, async_guard) = capture_events();
    let async_outcome = async_segment.run(exchange_with_id("any")).await;
    assert!(
        matches!(async_outcome, PipelineOutcome::Completed(_)),
        "async null key under process must complete"
    );
    assert!(
        async_invoked.was_invoked(),
        "child must run for a null async key under process"
    );
    let async_warns: Vec<String> = captured_events(&async_sink)
        .into_iter()
        .filter(CapturedEvent::is_warn)
        .map(|event| event.message)
        .collect();
    drop(async_guard);
    assert_eq!(
        async_warns.len(),
        1,
        "async arm must emit exactly one WARN: {async_warns:?}"
    );
    assert!(
        async_warns[0].contains("on_missing_key = 'process'"),
        "async WARN must name the process policy, got: {:?}",
        async_warns[0]
    );
}

// ── Duplicate / bookkeeping observability (task 1.2) ────────────────

fn recording_rt() -> (Arc<dyn RuntimeObservability>, Arc<RecordingMetrics>) {
    let collector = Arc::new(RecordingMetrics::default());
    let rt = Arc::new(RecordingRt {
        collector: Arc::clone(&collector),
    });
    (rt, collector)
}

#[tokio::test]
async fn duplicate_contains_hit_sets_property_and_counter() {
    let repo = Arc::new(MockRepo::new());
    repo.pre_add("dup-1").await;
    let (rt, metrics) = recording_rt();
    let (mut segment, child_invoked) = build_segment_with_rt(
        repo.clone(),
        PipelineOutcome::Failed(stub_error()),
        false,
        false,
        rt,
    );

    let outcome = segment.run(exchange_with_id("dup-1")).await;

    let ex = match outcome {
        PipelineOutcome::Completed(ex) => ex,
        other => panic!("expected Completed for a duplicate, got {other:?}"),
    };
    assert_eq!(
        ex.property(CAMEL_IDEMPOTENT_DUPLICATE),
        Some(&Value::Bool(true)),
        "duplicate skip must set the camel.idempotent.duplicate property"
    );
    assert!(
        !child_invoked.was_invoked(),
        "child must NOT run when key is a duplicate"
    );
    let counters = metrics.counters();
    let duplicates: Vec<_> = counters
        .iter()
        .filter(|(name, _, _)| name == "camel.idempotent.duplicates")
        .collect();
    assert_eq!(
        duplicates.len(),
        1,
        "duplicate counter must be recorded exactly once: {counters:?}"
    );
    assert_eq!(duplicates[0].1, 1.0_f64);
    assert!(
        duplicates[0]
            .2
            .contains(&("repository".to_string(), "mock".to_string())),
        "duplicate counter must carry the repository label: {:?}",
        duplicates[0]
    );
}

#[tokio::test]
async fn eager_race_duplicate_sets_property_and_counter() {
    // `race_duplicate` reports `contains == false` but `add == false`:
    // the eager reservation loses the race without the key being
    // pre-added (pre-adding would take the contains-hit path).
    let repo = Arc::new(MockRepo::race_duplicate());
    let (rt, metrics) = recording_rt();
    let (mut segment, child_invoked) = build_segment_with_rt(
        repo.clone(),
        PipelineOutcome::Failed(stub_error()),
        true, // eager
        true, // remove_on_failure
        rt,
    );

    let outcome = segment.run(exchange_with_id("race-1")).await;

    let ex = match outcome {
        PipelineOutcome::Completed(ex) => ex,
        other => panic!("expected Completed for a race duplicate, got {other:?}"),
    };
    assert_eq!(
        ex.property(CAMEL_IDEMPOTENT_DUPLICATE),
        Some(&Value::Bool(true)),
        "eager race duplicate must set the camel.idempotent.duplicate property"
    );
    assert!(
        !child_invoked.was_invoked(),
        "child must NOT run when the eager reservation loses the race"
    );
    assert_eq!(
        repo.ops().await,
        vec!["contains", "add"],
        "race path must consult contains then add"
    );
    let counters = metrics.counters();
    let duplicates: Vec<_> = counters
        .iter()
        .filter(|(name, _, _)| name == "camel.idempotent.duplicates")
        .collect();
    assert_eq!(
        duplicates.len(),
        1,
        "duplicate counter must be recorded exactly once: {counters:?}"
    );
    assert!(
        duplicates[0]
            .2
            .contains(&("repository".to_string(), "mock".to_string())),
        "duplicate counter must carry the repository label: {:?}",
        duplicates[0]
    );
}

#[tokio::test]
async fn non_eager_add_failure_keeps_success_with_error_metric() {
    let repo = Arc::new(MockRepo::failing_add());
    let (rt, metrics) = recording_rt();
    let (mut segment, child_invoked) = build_segment_with_rt(
        repo.clone(),
        PipelineOutcome::Completed(stub_exchange()),
        false, // non-eager: add happens post-success
        false,
        rt,
    );

    let (sink, guard) = capture_events();
    let outcome = segment.run(exchange_with_id("add-fail")).await;
    let errors: Vec<CapturedEvent> = captured_events(&sink)
        .into_iter()
        .filter(CapturedEvent::is_error)
        .collect();
    drop(guard);

    assert!(
        matches!(outcome, PipelineOutcome::Completed(_)),
        "a post-success add failure must keep the exchange successful"
    );
    assert!(
        child_invoked.was_invoked(),
        "child must run before the post-success add"
    );
    assert!(
        !repo.contains_key("add-fail").await,
        "the failed add must leave the key absent from the repository"
    );
    let counters = metrics.counters();
    let add_failures: Vec<_> = counters
        .iter()
        .filter(|(name, _, _)| name == "camel.idempotent.add_failures")
        .collect();
    assert_eq!(
        add_failures.len(),
        1,
        "add-failure counter must be recorded exactly once: {counters:?}"
    );
    assert!(
        add_failures[0]
            .2
            .contains(&("repository".to_string(), "mock".to_string())),
        "add-failure counter must carry the repository label: {:?}",
        add_failures[0]
    );
    assert_eq!(
        errors.len(),
        1,
        "post-success add failure must emit exactly one ERROR log: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    assert!(
        errors[0].message.contains("post-success"),
        "ERROR log must describe the post-success add failure, got: {:?}",
        errors[0].message
    );
}

// ── Pending-rollback state machine (task 1.3) ───────────────────────

type SharedRollbackState = Arc<std::sync::Mutex<PendingRollbacks>>;

fn shared_rollback_state() -> SharedRollbackState {
    Arc::new(std::sync::Mutex::new(PendingRollbacks::default()))
}

/// Build a segment that shares `rollback_state` with other segments.
///
/// The segment is also clone-safe (task 1.4): clones share the rollback
/// state, the repository, and the child invocation counter, so the
/// raced-delivery tests can drive two clones over one key.
fn build_segment_sharing_state(
    repo: Arc<MockRepo>,
    child_outcome: PipelineOutcome,
    eager: bool,
    remove_on_failure: bool,
    rollback_state: SharedRollbackState,
) -> (IdempotentConsumerSegment, Arc<InvocationCounter>) {
    let (mut segment, invoked) = build_segment(repo, child_outcome, eager, remove_on_failure);
    segment.rollback_state = rollback_state;
    (segment, invoked)
}

/// Drive a real failed eager rollback (child `Failed`, `remove` `Err`) so
/// `key` lands in the pending-rollback state, and return the repository
/// and the shared state that now holds it.
async fn seed_pending_via_failed_rollback(key: &str) -> (Arc<MockRepo>, SharedRollbackState) {
    let repo = Arc::new(MockRepo::failing_remove());
    let state = shared_rollback_state();
    let (mut segment, child_invoked) = build_segment_sharing_state(
        repo.clone(),
        PipelineOutcome::Failed(CamelError::Io("redis down".into())),
        true,
        true,
        Arc::clone(&state),
    );

    let outcome = segment.run(exchange_with_id(key)).await;

    assert!(
        matches!(outcome, PipelineOutcome::Failed(_)),
        "seed run must fail with the child error"
    );
    assert!(
        child_invoked.was_invoked(),
        "seed run must run the child before its rollback fails"
    );
    let pending = pending_keys(&state);
    assert!(
        pending.contains(&key.to_string()),
        "seed run must queue {key:?} for pending-rollback recovery: {pending:?}"
    );
    (repo, state)
}

fn in_flight_keys(state: &SharedRollbackState) -> Vec<String> {
    state
        .lock()
        .expect("rollback state lock poisoned")
        .in_flight
        .iter()
        .cloned()
        .collect()
}

fn pending_keys(state: &SharedRollbackState) -> Vec<String> {
    state
        .lock()
        .expect("rollback state lock poisoned")
        .pending
        .iter()
        .cloned()
        .collect()
}

// ── Test 1.3-1: a failed rollback fails the exchange with the child error ──
#[tokio::test]
async fn failed_rollback_preserves_child_error_and_logs_context() {
    let repo = Arc::new(MockRepo::failing_remove());
    let (mut segment, child_invoked) = build_segment(
        repo.clone(),
        PipelineOutcome::Failed(CamelError::Io("redis down".into())),
        true, // eager
        true, // remove_on_failure
    );

    let (sink, guard) = capture_events();
    let outcome = segment.run(exchange_with_id("rb-1")).await;
    let errors: Vec<CapturedEvent> = captured_events(&sink)
        .into_iter()
        .filter(CapturedEvent::is_error)
        .collect();
    drop(guard);

    // Structural regression: `on_exceptions` matches the child's variant,
    // so the child error must survive the failed rollback unwrapped.
    match &outcome {
        PipelineOutcome::Failed(CamelError::Io(msg)) => assert!(
            msg.contains("redis down"),
            "the child's Io error message must be preserved verbatim, got: {msg}"
        ),
        other => panic!("expected Failed(CamelError::Io), got {other:?}"),
    }
    assert!(
        child_invoked.was_invoked(),
        "child must run before its rollback can fail"
    );
    assert_eq!(
        errors.len(),
        1,
        "failed rollback must emit exactly one ERROR log: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    let message = &errors[0].message;
    assert!(
        message.contains("child_error") && message.contains("redis down"),
        "ERROR log must carry the child error as structured context, got: {message:?}"
    );
    assert!(
        message.contains("rollback_error") && message.contains("synthetic remove failure"),
        "ERROR log must carry the rollback error as structured context, got: {message:?}"
    );
    assert_eq!(
        pending_keys(&segment.rollback_state),
        vec!["rb-1".to_string()],
        "the key must be queued for pending-rollback recovery"
    );
    assert!(
        in_flight_keys(&segment.rollback_state).is_empty(),
        "a queued key must not be left in flight"
    );
    assert!(
        repo.contains_key("rb-1").await,
        "the stale eager reservation must remain until recovery removes it"
    );
}

// ── Test 1.3-2: a pending removal that fails again fails without repo/child ──
#[tokio::test]
async fn pending_removal_fails_again_fails_without_repo_or_child() {
    let (_seeding_repo, state) = seed_pending_via_failed_rollback("k").await;
    let repo = Arc::new(MockRepo::failing_remove());
    let (mut segment, child_invoked) = build_segment_sharing_state(
        repo.clone(),
        PipelineOutcome::Completed(stub_exchange()),
        true,
        true,
        Arc::clone(&state),
    );

    let ops_before = repo.ops().await.len();
    let outcome = segment.run(exchange_with_id("k")).await;
    let ops = repo.ops().await;

    match &outcome {
        PipelineOutcome::Failed(e) => assert!(
            e.to_string().contains("synthetic remove failure"),
            "the exchange must fail with the removal error, got: {e}"
        ),
        other => panic!("expected Failed with the removal error, got {other:?}"),
    }
    assert!(
        !child_invoked.was_invoked(),
        "child must NOT run when the recovery removal fails"
    );
    assert_eq!(
        &ops[ops_before..],
        &["remove"],
        "a failed recovery must touch only the removal, not contains/add"
    );
    assert_eq!(
        pending_keys(&state),
        vec!["k".to_string()],
        "the key must return to the pending state"
    );
    assert!(
        in_flight_keys(&state).is_empty(),
        "a failed recovery must not leave the key in flight"
    );
}

// ── Test 1.3-3: a pending rollback is retried, then the child runs once ──
#[tokio::test]
async fn pending_rollback_retried_then_child_runs_exactly_once() {
    let (_seeding_repo, state) = seed_pending_via_failed_rollback("k").await;
    let repo = Arc::new(MockRepo::new());
    let (mut segment, child_invoked) = build_segment_sharing_state(
        repo.clone(),
        PipelineOutcome::Completed(stub_exchange()),
        true,
        false,
        Arc::clone(&state),
    );

    let outcome = segment.run(exchange_with_id("k")).await;

    let ex = match outcome {
        PipelineOutcome::Completed(ex) => ex,
        other => {
            panic!("the recovery owner must run the child and return its outcome, got {other:?}")
        }
    };
    assert!(
        child_invoked.was_invoked(),
        "the recovery owner must run the child exactly once"
    );
    assert_eq!(
        ex.property(CAMEL_IDEMPOTENT_DUPLICATE),
        None,
        "the recovered delivery must never be reported as a duplicate"
    );
    assert!(
        in_flight_keys(&state).is_empty(),
        "the marker must be cleared once the reservation step resolved"
    );
    assert!(
        pending_keys(&state).is_empty(),
        "the pending key must have been consumed by its recovery owner"
    );

    // No-wedge: a third delivery follows the normal duplicate path.
    let (mut follow_up, follow_up_invoked) = build_segment_sharing_state(
        repo,
        PipelineOutcome::Failed(stub_error()),
        true,
        false,
        Arc::clone(&state),
    );
    let follow_up_outcome = follow_up.run(exchange_with_id("k")).await;
    let ex = match follow_up_outcome {
        PipelineOutcome::Completed(ex) => ex,
        other => panic!("a settled key must follow the duplicate path, got {other:?}"),
    };
    assert_eq!(
        ex.property(CAMEL_IDEMPOTENT_DUPLICATE),
        Some(&Value::Bool(true)),
        "the settled key must be reported as an observable duplicate"
    );
    assert!(
        !follow_up_invoked.was_invoked(),
        "the duplicate delivery must not run the child"
    );
}

// ── Test 1.3-4: eager reservation resolution clears the marker ──
#[tokio::test]
async fn marker_cleared_on_eager_reservation_resolution() {
    let (_seeding_repo, state) = seed_pending_via_failed_rollback("k").await;
    let repo = Arc::new(MockRepo::new());
    let (mut segment, child_invoked) = build_segment_sharing_state(
        repo.clone(),
        PipelineOutcome::Stopped(stub_exchange()),
        true, // eager: the marker spans removal -> reservation
        false,
        Arc::clone(&state),
    );

    let outcome = segment.run(exchange_with_id("k")).await;

    match &outcome {
        PipelineOutcome::Stopped(_) => {}
        other => panic!("the recovery owner must return the child outcome, got {other:?}"),
    }
    assert!(
        child_invoked.was_invoked(),
        "the recovery owner must run the child"
    );
    assert!(
        in_flight_keys(&state).is_empty(),
        "eager reservation resolution must clear the in-flight marker"
    );

    let (mut follow_up, follow_up_invoked) = build_segment_sharing_state(
        repo,
        PipelineOutcome::Failed(stub_error()),
        true,
        false,
        Arc::clone(&state),
    );
    let follow_up_outcome = follow_up.run(exchange_with_id("k")).await;
    let ex = match follow_up_outcome {
        PipelineOutcome::Completed(ex) => ex,
        other => panic!("a cleared marker must follow the contains-hit path, got {other:?}"),
    };
    assert_eq!(
        ex.property(CAMEL_IDEMPOTENT_DUPLICATE),
        Some(&Value::Bool(true)),
        "the follow-up must be an observable duplicate, not a recovery-in-progress error"
    );
    assert!(
        !follow_up_invoked.was_invoked(),
        "the duplicate follow-up must not run the child"
    );
}

// ── Test 1.3-5: lazy bookkeeping resolution clears the marker ──
#[tokio::test]
async fn marker_cleared_on_lazy_bookkeeping_resolution() {
    let (_seeding_repo, state) = seed_pending_via_failed_rollback("k").await;
    let repo = Arc::new(MockRepo::new());
    let (mut segment, child_invoked) = build_segment_sharing_state(
        repo.clone(),
        PipelineOutcome::Completed(stub_exchange()),
        false, // lazy: the marker spans removal -> post-child add
        false,
        Arc::clone(&state),
    );

    let outcome = segment.run(exchange_with_id("k")).await;

    match &outcome {
        PipelineOutcome::Completed(_) => {}
        other => panic!("the recovery owner must return the child outcome, got {other:?}"),
    }
    assert!(
        child_invoked.was_invoked(),
        "the recovery owner must run the child"
    );
    assert!(
        in_flight_keys(&state).is_empty(),
        "the lazy post-child bookkeeping must clear the in-flight marker"
    );

    let (mut follow_up, follow_up_invoked) = build_segment_sharing_state(
        repo,
        PipelineOutcome::Failed(stub_error()),
        false,
        false,
        Arc::clone(&state),
    );
    let follow_up_outcome = follow_up.run(exchange_with_id("k")).await;
    let ex = match follow_up_outcome {
        PipelineOutcome::Completed(ex) => ex,
        other => panic!("a cleared marker must follow the contains-hit path, got {other:?}"),
    };
    assert_eq!(
        ex.property(CAMEL_IDEMPOTENT_DUPLICATE),
        Some(&Value::Bool(true)),
        "the follow-up must be an observable duplicate, not a recovery-in-progress error"
    );
    assert!(
        !follow_up_invoked.was_invoked(),
        "the duplicate follow-up must not run the child"
    );
}

// ── Test 1.3-6: fail paths clear the in-flight marker too ──
#[tokio::test]
async fn in_flight_failure_paths_clear_the_marker() {
    let (_seeding_repo, state) = seed_pending_via_failed_rollback("k").await;
    let repo = Arc::new(MockRepo::failing_add());
    let (mut segment, child_invoked) = build_segment_sharing_state(
        repo.clone(),
        PipelineOutcome::Completed(stub_exchange()),
        true,
        false,
        Arc::clone(&state),
    );

    let outcome = segment.run(exchange_with_id("k")).await;

    match &outcome {
        PipelineOutcome::Failed(e) => assert!(
            e.to_string().contains("synthetic add failure"),
            "the reservation add error must fail the exchange, got: {e}"
        ),
        other => panic!("expected Failed from the reservation add, got {other:?}"),
    }
    assert!(
        !child_invoked.was_invoked(),
        "child must NOT run when the reservation add fails"
    );
    assert!(
        in_flight_keys(&state).is_empty(),
        "the reservation fail path must clear the in-flight marker"
    );

    let (mut follow_up, _follow_up_invoked) = build_segment_sharing_state(
        repo,
        PipelineOutcome::Completed(stub_exchange()),
        true,
        false,
        Arc::clone(&state),
    );
    let follow_up_outcome = follow_up.run(exchange_with_id("k")).await;
    match &follow_up_outcome {
        PipelineOutcome::Failed(e) => assert!(
            !e.to_string().contains("pending-rollback recovery"),
            "no wedged in-flight key: the follow-up must not hit the recovery error, got: {e}"
        ),
        other => panic!("expected Failed from the still-failing add, got {other:?}"),
    }
}

// ── Test 1.3-7: the eager rollback retry runs the child again ──
#[tokio::test]
async fn eager_rollback_then_retry_runs_child_again() {
    let repo = Arc::new(MockRepo::new());
    let state = shared_rollback_state();
    let (mut first, first_invoked) = build_segment_sharing_state(
        repo.clone(),
        PipelineOutcome::Failed(stub_error()),
        true,
        true,
        Arc::clone(&state),
    );

    let first_outcome = first.run(exchange_with_id("eager-retry")).await;

    match &first_outcome {
        PipelineOutcome::Failed(_) => {}
        other => panic!("the first delivery must fail with the child error, got {other:?}"),
    }
    assert!(
        first_invoked.was_invoked(),
        "the first delivery must run the child"
    );
    assert!(
        !repo.contains_key("eager-retry").await,
        "eager rollback must remove the reservation"
    );

    let (mut second, second_invoked) = build_segment_sharing_state(
        repo,
        PipelineOutcome::Failed(stub_error()),
        true,
        true,
        Arc::clone(&state),
    );
    let second_outcome = second.run(exchange_with_id("eager-retry")).await;

    match &second_outcome {
        PipelineOutcome::Failed(e) => assert!(
            e.to_string().contains("child failed"),
            "the retry must surface the child failure, got: {e}"
        ),
        other => {
            panic!("the retry must run the child again, not skip it as a duplicate, got {other:?}")
        }
    }
    assert!(
        second_invoked.was_invoked(),
        "the eager rollback retry must run the child a second time"
    );
}

// ── Test 1.3-8: eager without rollback keeps at-most-once, observably ──
#[tokio::test]
async fn eager_without_rollback_keeps_at_most_once_with_observable_skip() {
    let repo = Arc::new(MockRepo::new());
    let (rt, metrics) = recording_rt();
    let (mut segment, child_invoked) = build_segment_with_rt(
        repo.clone(),
        PipelineOutcome::Failed(stub_error()),
        true,  // eager
        false, // explicit at-most-once opt-in
        rt,
    );

    let first_outcome = segment.run(exchange_with_id("at-most-once")).await;

    match &first_outcome {
        PipelineOutcome::Failed(_) => {}
        other => panic!("the first delivery must fail with the child error, got {other:?}"),
    }
    assert!(
        child_invoked.was_invoked(),
        "the first delivery must run the child"
    );
    assert!(
        repo.contains_key("at-most-once").await,
        "eager without rollback must keep the reservation"
    );

    let second_outcome = segment.run(exchange_with_id("at-most-once")).await;

    let ex = match second_outcome {
        PipelineOutcome::Completed(ex) => ex,
        other => {
            panic!("the retry must skip the child as an observable duplicate, got {other:?}")
        }
    };
    assert_eq!(
        ex.property(CAMEL_IDEMPOTENT_DUPLICATE),
        Some(&Value::Bool(true)),
        "the at-most-once skip must set the duplicate property"
    );
    let counters = metrics.counters();
    let duplicates: Vec<_> = counters
        .iter()
        .filter(|(name, _, _)| name == "camel.idempotent.duplicates")
        .collect();
    assert_eq!(
        duplicates.len(),
        1,
        "the at-most-once skip must increment the duplicate counter once: {counters:?}"
    );
}

// ── Raced delivery windows (task 1.4) ────────────────────────────────
//
// Both windows are pinned by handshakes and bounded timeouts, never by
// sleeps: the recovery owner parks inside a gated repository operation, the
// raced clone runs to completion while the owner is parked, and only then is
// the owner released.

/// Raced delivery while the recovery owner is still inside the removal: the
/// in-flight marker spans the removal, so the second clone fails loudly
/// without touching the repository or the child.
#[tokio::test]
async fn raced_delivery_during_removal_fails_loudly() {
    const KEY: &str = "raced-during-removal";
    let (_seeding_repo, state) = seed_pending_via_failed_rollback(KEY).await;
    let repo = Arc::new(MockRepo::new());
    let mut remove_gate = repo.arm_remove_gate();
    let (base, invoked) = build_segment_sharing_state(
        Arc::clone(&repo),
        PipelineOutcome::Completed(stub_exchange()),
        true,
        true,
        Arc::clone(&state),
    );
    let mut owner = base.clone();
    let mut racer = base.clone();

    // A: takes ownership and parks inside the gated removal.
    let owner_task = tokio::spawn(async move { owner.run(exchange_with_id(KEY)).await });
    remove_gate
        .entered()
        .await
        .expect("the owner must signal that it entered the removal");

    // B: same key, same shared state, owner not finished with the removal.
    let racer_outcome =
        tokio::time::timeout(Duration::from_secs(5), racer.run(exchange_with_id(KEY)))
            .await
            .expect("the raced delivery must not block");
    let ops_at_racer_return = repo.ops().await;

    match &racer_outcome {
        PipelineOutcome::Failed(e) => assert!(
            e.to_string().contains("pending-rollback recovery"),
            "the raced delivery must name the recovery-in-progress reason, got: {e}"
        ),
        other => panic!("the raced delivery must fail loudly, got {other:?}"),
    }
    assert_eq!(
        invoked.count(),
        0,
        "the raced delivery must not run the child"
    );
    assert_eq!(
        ops_at_racer_return,
        vec!["remove"],
        "the raced delivery must perform no repository operation of its own"
    );

    // Release the owner: it completes the removal, reserves, and runs the child.
    remove_gate.release();
    let owner_outcome = tokio::time::timeout(Duration::from_secs(5), owner_task)
        .await
        .expect("the owner must complete once the gate is released")
        .expect("the owner task must not panic");
    match &owner_outcome {
        PipelineOutcome::Completed(_) => {}
        other => panic!("the recovery owner must run the child and complete, got {other:?}"),
    }
    assert_eq!(
        invoked.count(),
        1,
        "the recovery owner must run the child exactly once"
    );

    // A settled key follows the normal observable-duplicate path.
    let mut follow_up = base.clone();
    let follow_up_outcome =
        tokio::time::timeout(Duration::from_secs(5), follow_up.run(exchange_with_id(KEY)))
            .await
            .expect("the follow-up delivery must not block");
    let ex = match follow_up_outcome {
        PipelineOutcome::Completed(ex) => ex,
        other => panic!("a settled key must follow the duplicate path, got {other:?}"),
    };
    assert_eq!(
        ex.property(CAMEL_IDEMPOTENT_DUPLICATE),
        Some(&Value::Bool(true)),
        "the follow-up must be an observable duplicate"
    );
    assert_eq!(
        invoked.count(),
        1,
        "the duplicate follow-up must not run the child"
    );
}

/// Raced delivery between a successful removal and the owner's reservation:
/// premature marker clearing here would let the second clone reserve and win,
/// so this window is what pins the marker to span removal -> reservation.
#[tokio::test]
async fn raced_delivery_after_removal_before_reservation_fails_loudly() {
    const KEY: &str = "raced-after-removal";
    let (_seeding_repo, state) = seed_pending_via_failed_rollback(KEY).await;
    let repo = Arc::new(MockRepo::new());
    let mut add_gate = repo.arm_add_gate();
    let (base, invoked) = build_segment_sharing_state(
        Arc::clone(&repo),
        PipelineOutcome::Completed(stub_exchange()),
        true,
        true,
        Arc::clone(&state),
    );
    let mut owner = base.clone();
    let mut racer = base.clone();

    // A: removal succeeds, then parks inside the pre-child reservation add.
    let owner_task = tokio::spawn(async move { owner.run(exchange_with_id(KEY)).await });
    add_gate
        .entered()
        .await
        .expect("the owner must signal that it entered the reservation add");
    let ops_before_racer = repo.ops().await;

    // B: same key inside the removal -> reservation window.
    let racer_outcome =
        tokio::time::timeout(Duration::from_secs(5), racer.run(exchange_with_id(KEY)))
            .await
            .expect("the raced delivery must not block");
    let ops_after_racer = repo.ops().await;

    match &racer_outcome {
        PipelineOutcome::Failed(e) => assert!(
            e.to_string().contains("pending-rollback recovery"),
            "the raced delivery must name the recovery-in-progress reason, got: {e}"
        ),
        other => panic!("the raced delivery must fail loudly, got {other:?}"),
    }
    assert_eq!(
        invoked.count(),
        0,
        "the raced delivery must not run the child"
    );
    assert_eq!(
        ops_before_racer,
        vec!["remove", "contains", "add"],
        "the owner must have removed and reached the reservation add before the race: {ops_before_racer:?}"
    );
    assert_eq!(
        ops_after_racer, ops_before_racer,
        "the raced delivery must perform no contains/add operation of its own"
    );

    // Release the owner: the reservation resolves Ok(true), then the child runs.
    add_gate.release();
    let owner_outcome = tokio::time::timeout(Duration::from_secs(5), owner_task)
        .await
        .expect("the owner must complete once the gate is released")
        .expect("the owner task must not panic");
    match &owner_outcome {
        PipelineOutcome::Completed(_) => {}
        other => panic!("the recovery owner must run the child and complete, got {other:?}"),
    }
    assert!(
        repo.contains_key(KEY).await,
        "the owner's reservation add must have resolved to a stored key"
    );
    assert_eq!(
        invoked.count(),
        1,
        "the recovery owner must run the child exactly once, the raced delivery never"
    );
}
