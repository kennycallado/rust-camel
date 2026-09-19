use super::*;
use std::sync::Mutex;

use camel_component_api::{Body, ExchangeEnvelope};
use tokio_util::sync::CancellationToken;

type ErrorLog = Arc<Mutex<Vec<(String, String)>>>;

// -------------------------------------------------------------------
// Recording metrics/runtime double (pattern: camel-timer
// tests::RecordingRuntime, itself from camel-direct RecordingMetrics)
// -------------------------------------------------------------------

struct RecordingMetrics {
    errors: ErrorLog,
}

impl camel_api::MetricsCollector for RecordingMetrics {
    fn record_exchange_duration(&self, _: &str, _: std::time::Duration) {}
    fn increment_errors(&self, route_id: &str, error_type: &str) {
        self.errors
            .lock()
            .unwrap()
            .push((route_id.to_string(), error_type.to_string()));
    }
    fn increment_exchanges(&self, _: &str) {}
    fn set_queue_depth(&self, _: &str, _: usize) {}
    fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
}

struct RecordingRuntime {
    metrics_collector: Arc<RecordingMetrics>,
}

impl RecordingRuntime {
    fn new(errors: ErrorLog) -> Self {
        Self {
            metrics_collector: Arc::new(RecordingMetrics { errors }),
        }
    }
}

impl RuntimeObservability for RecordingRuntime {
    fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
        Arc::clone(&self.metrics_collector) as Arc<dyn camel_api::MetricsCollector>
    }
    fn health(&self) -> Arc<dyn camel_component_api::HealthCheckRegistry> {
        panic!("RecordingRuntime::health not used in this test")
    }
}

// -------------------------------------------------------------------
// Reader test doubles
// -------------------------------------------------------------------

/// StreamReader double over a shared byte buffer: reads consume from the
/// front, mimicking a pipe. Empty buffer = EOF.
struct SharedReader(Arc<Mutex<Vec<u8>>>);

impl SharedReader {
    fn new(input: &[u8]) -> Self {
        Self(Arc::new(Mutex::new(input.to_vec())))
    }

    fn drain(&self, max: usize) -> Vec<u8> {
        let mut guard = self.0.lock().unwrap();
        let n = max.min(guard.len());
        guard.drain(..n).collect()
    }
}

#[async_trait::async_trait]
impl StreamReader for SharedReader {
    async fn read_until_newline(&mut self, buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        let mut guard = self.0.lock().unwrap();
        if guard.is_empty() {
            return Ok(0);
        }
        let upto = guard
            .iter()
            .position(|&b| b == b'\n')
            .map_or(guard.len(), |i| i + 1);
        let chunk: Vec<u8> = guard.drain(..upto).collect();
        buf.extend_from_slice(&chunk);
        Ok(chunk.len())
    }

    async fn read_exact_chunk(&mut self, n: usize, buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        let chunk = self.drain(n);
        let len = chunk.len();
        buf.extend_from_slice(&chunk);
        Ok(len)
    }

    async fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        let chunk = self.drain(usize::MAX);
        let len = chunk.len();
        buf.extend_from_slice(&chunk);
        Ok(len)
    }
}

/// Reader whose second `read_until_newline` holds a gate until the test
/// opens it. Proves the consumer sends envelope 1 BEFORE starting the
/// next read: a read-ahead consumer blocks in the gate and trips the
/// test's timeout; completing the gated read before the test recorded
/// consumption panics.
struct GatedReader {
    data: SharedReader,
    gate: Arc<tokio::sync::Notify>,
    consumed: Arc<AtomicBool>,
    calls: usize,
}

#[async_trait::async_trait]
impl StreamReader for GatedReader {
    async fn read_until_newline(&mut self, buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        let call = self.calls;
        self.calls += 1;
        if call == 1 {
            self.gate.notified().await;
            assert!(
                self.consumed.load(Ordering::SeqCst),
                "second read completed before the first envelope was consumed"
            );
        }
        self.data.read_until_newline(buf).await
    }

    async fn read_exact_chunk(&mut self, n: usize, buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        self.data.read_exact_chunk(n, buf).await
    }

    async fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        self.data.read_to_end(buf).await
    }
}

/// Reader whose reads never complete (models a stdin with no input and
/// no EOF) — used for the cancellation test.
struct BlockingReader;

#[async_trait::async_trait]
impl StreamReader for BlockingReader {
    async fn read_until_newline(&mut self, _buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        std::future::pending::<()>().await;
        Ok(0)
    }

    async fn read_exact_chunk(
        &mut self,
        _n: usize,
        _buf: &mut Vec<u8>,
    ) -> Result<usize, CamelError> {
        std::future::pending::<()>().await;
        Ok(0)
    }

    async fn read_to_end(&mut self, _buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        std::future::pending::<()>().await;
        Ok(0)
    }
}

/// Reader whose reads always fail with an IO error — models a genuine
/// source failure (EIO and friends), which is NOT EOF.
struct ErroringReader;

#[async_trait::async_trait]
impl StreamReader for ErroringReader {
    async fn read_until_newline(&mut self, _buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        Err(CamelError::Io("simulated EIO".to_string()))
    }

    async fn read_exact_chunk(
        &mut self,
        _n: usize,
        _buf: &mut Vec<u8>,
    ) -> Result<usize, CamelError> {
        Err(CamelError::Io("simulated EIO".to_string()))
    }

    async fn read_to_end(&mut self, _buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        Err(CamelError::Io("simulated EIO".to_string()))
    }
}

/// Cap on bytes returned per `read_exact_chunk` call for
/// [`DripReader`], smaller than the frame size used in its test.
const DRIP_MAX_CHUNK: usize = 2;

/// Reader double that delivers at most [`DRIP_MAX_CHUNK`] bytes per
/// `read_exact_chunk` call, modeling a trickling stdin. Proves fixed
/// framing accumulates short reads into a full frame instead of
/// emitting a premature partial frame.
struct DripReader {
    data: SharedReader,
}

#[async_trait::async_trait]
impl StreamReader for DripReader {
    async fn read_until_newline(&mut self, buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        self.data.read_until_newline(buf).await
    }

    async fn read_exact_chunk(&mut self, n: usize, buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        self.data.read_exact_chunk(n.min(DRIP_MAX_CHUNK), buf).await
    }

    async fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        self.data.read_to_end(buf).await
    }
}

/// Models `cat /dev/zero | camel run <raw-route>`: an endless source
/// that never EOFs, so raw or line accumulation would grow without
/// bound. To keep the test cheap it never materializes the 100 MiB
/// bound: it reports the violation exactly where the production
/// `StdinReader` detects it — the pre-growth `ensure_frame_bound`
/// check — by returning `StreamLimitExceeded` on the first read. Fixed
/// framing is not exercised here: `size` is config-validated within
/// the bound, so a fixed frame can never trip it.
struct UnboundedSourceReader;

#[async_trait::async_trait]
impl StreamReader for UnboundedSourceReader {
    async fn read_until_newline(&mut self, _buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        // Endless zeros hold no `\n`; accumulation would cross the bound.
        Err(CamelError::StreamLimitExceeded(MAX_MATERIALIZE_BYTES))
    }

    async fn read_exact_chunk(
        &mut self,
        _n: usize,
        _buf: &mut Vec<u8>,
    ) -> Result<usize, CamelError> {
        panic!("fixed framing cannot exceed the config-validated bound; not used in this test")
    }

    async fn read_to_end(&mut self, _buf: &mut Vec<u8>) -> Result<usize, CamelError> {
        Err(CamelError::StreamLimitExceeded(MAX_MATERIALIZE_BYTES))
    }
}

// -------------------------------------------------------------------
// Fixtures
// -------------------------------------------------------------------

fn errors_log() -> ErrorLog {
    Arc::new(Mutex::new(Vec::new()))
}

/// Build a consumer on the injected reader plus a live context wired to
/// an mpsc channel; the test asserts on the received envelopes.
fn test_setup(
    config: StreamConfig,
    reader: Box<dyn StreamReader>,
) -> (
    StreamConsumer,
    ConsumerContext,
    tokio::sync::mpsc::Receiver<ExchangeEnvelope>,
    CancellationToken,
    ErrorLog,
) {
    let errors = errors_log();
    let (tx, rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
    let token = CancellationToken::new();
    let consumer = StreamConsumer::with_reader(
        config,
        Arc::new(RecordingRuntime::new(Arc::clone(&errors))),
        reader,
    );
    let context = ConsumerContext::new(tx, token.clone(), "stream-test-route".to_string());
    (consumer, context, rx, token, errors)
}

async fn recv_text(rx: &mut tokio::sync::mpsc::Receiver<ExchangeEnvelope>) -> String {
    let envelope = rx.recv().await.expect("expected an envelope");
    match &envelope.exchange.input.body {
        Body::Text(text) => text.clone(),
        other => panic!("expected Text body, got {other:?}"),
    }
}

async fn recv_bytes(rx: &mut tokio::sync::mpsc::Receiver<ExchangeEnvelope>) -> Vec<u8> {
    let envelope = rx.recv().await.expect("expected an envelope");
    match &envelope.exchange.input.body {
        Body::Bytes(bytes) => bytes.to_vec(),
        other => panic!("expected Bytes body, got {other:?}"),
    }
}

// -------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------

#[tokio::test]
async fn one_line_one_exchange() {
    let (mut consumer, ctx, mut rx, _token, _errors) = test_setup(
        StreamConfig::from_uri("stream:in").unwrap(),
        Box::new(SharedReader::new(b"a\nb\nc\n")),
    );
    let handle = tokio::spawn(async move { consumer.start(ctx).await });

    assert_eq!(recv_text(&mut rx).await, "a");
    assert_eq!(recv_text(&mut rx).await, "b");
    assert_eq!(recv_text(&mut rx).await, "c");
    handle.await.unwrap().unwrap();
    assert!(matches!(
        rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
}

#[tokio::test]
async fn final_unterminated_line_emits() {
    let (mut consumer, ctx, mut rx, _token, _errors) = test_setup(
        StreamConfig::from_uri("stream:in").unwrap(),
        Box::new(SharedReader::new(b"x\ny")),
    );
    let handle = tokio::spawn(async move { consumer.start(ctx).await });

    assert_eq!(recv_text(&mut rx).await, "x");
    assert_eq!(recv_text(&mut rx).await, "y");
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn crlf_stripped() {
    let (mut consumer, ctx, mut rx, _token, _errors) = test_setup(
        StreamConfig::from_uri("stream:in").unwrap(),
        Box::new(SharedReader::new(b"w\r\n")),
    );
    let handle = tokio::spawn(async move { consumer.start(ctx).await });

    assert_eq!(recv_text(&mut rx).await, "w");
    handle.await.unwrap().unwrap();
}

/// A trailing `\r` with no `\n` is DATA, not a terminator: only a `\r`
/// that precedes a stripped `\n` may be stripped. (Apache Camel's
/// stream: also splits on `\r`; this component diverges — documented on
/// the component page.)
#[tokio::test]
async fn cr_only_line_preserved() {
    let (mut consumer, ctx, mut rx, _token, _errors) = test_setup(
        StreamConfig::from_uri("stream:in").unwrap(),
        Box::new(SharedReader::new(b"x\r")),
    );
    let handle = tokio::spawn(async move { consumer.start(ctx).await });

    assert_eq!(recv_text(&mut rx).await, "x\r");
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn raw_frame_one_exchange() {
    let (mut consumer, ctx, mut rx, _token, _errors) = test_setup(
        StreamConfig::from_uri("stream:in?frame=raw").unwrap(),
        Box::new(SharedReader::new(b"l1\nl2\nl3\n")),
    );
    let handle = tokio::spawn(async move { consumer.start(ctx).await });

    assert_eq!(recv_bytes(&mut rx).await, b"l1\nl2\nl3\n");
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn raw_empty_zero_exchanges() {
    let (mut consumer, ctx, mut rx, _token, _errors) = test_setup(
        StreamConfig::from_uri("stream:in?frame=raw").unwrap(),
        Box::new(SharedReader::new(b"")),
    );
    let handle = tokio::spawn(async move { consumer.start(ctx).await });

    handle.await.unwrap().unwrap();
    assert!(matches!(
        rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
}

#[tokio::test]
async fn fixed_frames_and_partial_chunk() {
    let (mut consumer, ctx, mut rx, _token, _errors) = test_setup(
        StreamConfig::from_uri("stream:in?frame=fixed&size=4").unwrap(),
        Box::new(SharedReader::new(b"0123456789")),
    );
    let handle = tokio::spawn(async move { consumer.start(ctx).await });

    assert_eq!(recv_bytes(&mut rx).await, b"0123");
    assert_eq!(recv_bytes(&mut rx).await, b"4567");
    assert_eq!(recv_bytes(&mut rx).await, b"89");
    handle.await.unwrap().unwrap();
}

/// Fixed framing must accumulate across short reads: the drip reader
/// returns at most 2 bytes per read call, so the single 4-byte frame can
/// only materialize if `read_exact_chunk` loops. 6 input bytes at
/// `size=4` yield one full frame plus a 2-byte partial at EOF.
#[tokio::test]
async fn fixed_frames_short_reads_accumulate() {
    let (mut consumer, ctx, mut rx, _token, _errors) = test_setup(
        StreamConfig::from_uri("stream:in?frame=fixed&size=4").unwrap(),
        Box::new(DripReader {
            data: SharedReader::new(b"012345"),
        }),
    );
    let handle = tokio::spawn(async move { consumer.start(ctx).await });

    assert_eq!(recv_bytes(&mut rx).await, b"0123");
    assert_eq!(recv_bytes(&mut rx).await, b"45");
    handle.await.unwrap().unwrap();
}

/// LOAD-BEARING: a read IO error is NOT EOF — it must fail `start`
/// loudly (CamelError::Io), send no envelope, and reset the started
/// flag so the consumer can be restarted after the failure.
#[tokio::test]
async fn read_io_error_fails_loudly() {
    let errors = errors_log();
    let mut consumer = StreamConsumer::with_reader(
        StreamConfig::from_uri("stream:in").unwrap(),
        Arc::new(RecordingRuntime::new(errors)),
        Box::new(ErroringReader),
    );

    let (tx1, mut rx1) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
    let ctx1 = ConsumerContext::new(
        tx1,
        CancellationToken::new(),
        "stream-test-route".to_string(),
    );
    let first = consumer.start(ctx1).await;
    assert!(
        matches!(&first, Err(CamelError::Io(msg)) if msg.contains("simulated EIO")),
        "IO error must fail loudly, got: {first:?}"
    );
    assert!(
        rx1.try_recv().is_err(),
        "no envelope may be sent when the read fails"
    );

    // The failure path must reset the started flag: a second start
    // retries the read (IO error again), not "already started".
    let (tx2, _rx2) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
    let ctx2 = ConsumerContext::new(
        tx2,
        CancellationToken::new(),
        "stream-test-route".to_string(),
    );
    let second = consumer.start(ctx2).await;
    assert!(
        matches!(second, Err(CamelError::Io(_))),
        "second start must retry (flag reset), got: {second:?}"
    );
}

/// LOAD-BEARING: a frame past the materialization bound is NOT EOF and
/// NOT a silent skip — it fails `start` loudly with the same
/// `StreamLimitExceeded` variant the producer's `into_bytes` bound
/// uses, sends no envelope, and resets the started flag. The
/// [`UnboundedSourceReader`] double reports the violation on the first
/// read without materializing 100 MiB (see its doc comment for the
/// honest-cheap wiring).
#[tokio::test]
async fn frame_bound_exceeded_fails_loudly() {
    let errors = errors_log();
    let mut consumer = StreamConsumer::with_reader(
        StreamConfig::from_uri("stream:in?frame=raw").unwrap(),
        Arc::new(RecordingRuntime::new(errors)),
        Box::new(UnboundedSourceReader),
    );

    let (tx1, mut rx1) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
    let ctx1 = ConsumerContext::new(
        tx1,
        CancellationToken::new(),
        "stream-test-route".to_string(),
    );
    let first = consumer.start(ctx1).await;
    assert!(
        matches!(
            &first,
            Err(CamelError::StreamLimitExceeded(MAX_MATERIALIZE_BYTES))
        ),
        "bound violation must fail loudly, got: {first:?}"
    );
    assert!(
        rx1.try_recv().is_err(),
        "no envelope may be sent when the bound is exceeded"
    );

    // The failure path must reset the started flag, same as the IO
    // error path: a second start retries (limit again), not "already
    // started".
    let (tx2, _rx2) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
    let ctx2 = ConsumerContext::new(
        tx2,
        CancellationToken::new(),
        "stream-test-route".to_string(),
    );
    let second = consumer.start(ctx2).await;
    assert!(
        matches!(second, Err(CamelError::StreamLimitExceeded(_))),
        "second start must retry (flag reset), got: {second:?}"
    );
}

// LOAD-BEARING (ruling R2): EOF with no input is a graceful `Ok(())`,
// never an error.
#[tokio::test]
async fn eof_no_input_completes_gracefully() {
    let (mut consumer, ctx, mut rx, _token, _errors) = test_setup(
        StreamConfig::from_uri("stream:in").unwrap(),
        Box::new(SharedReader::new(b"")),
    );
    let handle = tokio::spawn(async move { consumer.start(ctx).await });

    handle.await.unwrap().unwrap();
    assert!(matches!(
        rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
}

#[tokio::test]
async fn cancellation_stops_cleanly() {
    let (mut consumer, ctx, mut rx, token, _errors) = test_setup(
        StreamConfig::from_uri("stream:in").unwrap(),
        Box::new(BlockingReader),
    );
    let handle = tokio::spawn(async move { consumer.start(ctx).await });

    // Let the consumer enter the never-completing read, then cancel.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await; // allow-test-sleep: settle window so the consumer parks in the read before cancel (clean-stop is the assertion, not the path)
    token.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
        .await
        .expect("consumer must stop after cancellation")
        .unwrap();
    assert!(matches!(result, Ok(())));
    assert!(matches!(
        rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
}

#[tokio::test]
async fn invalid_utf8_line_skipped_with_metric() {
    let (mut consumer, ctx, mut rx, _token, errors) = test_setup(
        StreamConfig::from_uri("stream:in").unwrap(),
        Box::new(SharedReader::new(b"ok\n\xff\xfe bad\nok2\n")),
    );
    let handle = tokio::spawn(async move { consumer.start(ctx).await });

    assert_eq!(recv_text(&mut rx).await, "ok");
    assert_eq!(recv_text(&mut rx).await, "ok2");
    handle.await.unwrap().unwrap();
    assert_eq!(
        *errors.lock().unwrap(),
        vec![(
            "stream-test-route".to_string(),
            "b-prime:stream:decode".to_string()
        )]
    );
}

#[tokio::test]
async fn closed_channel_ends_loop_with_metric() {
    let errors = errors_log();
    let (tx, rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
    drop(rx);

    let mut consumer = StreamConsumer::with_reader(
        StreamConfig::from_uri("stream:in").unwrap(),
        Arc::new(RecordingRuntime::new(Arc::clone(&errors))),
        Box::new(SharedReader::new(b"a\n")),
    );
    let ctx = ConsumerContext::new(
        tx,
        CancellationToken::new(),
        "stream-test-route".to_string(),
    );

    consumer.start(ctx).await.unwrap();
    assert_eq!(
        *errors.lock().unwrap(),
        vec![(
            "stream-test-route".to_string(),
            "b-prime:stream:fire-send".to_string()
        )]
    );
}

#[tokio::test]
async fn sequential_backpressure_no_read_ahead() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let consumed = Arc::new(AtomicBool::new(false));
    let reader = GatedReader {
        data: SharedReader::new(b"a\nb\n"),
        gate: Arc::clone(&gate),
        consumed: Arc::clone(&consumed),
        calls: 0,
    };
    let (mut consumer, ctx, mut rx, _token, _errors) = test_setup(
        StreamConfig::from_uri("stream:in").unwrap(),
        Box::new(reader),
    );
    let handle = tokio::spawn(async move { consumer.start(ctx).await });

    // The second read is gated: a read-ahead consumer would be stuck in
    // the gate before ever sending, and this recv would time out.
    let first = tokio::time::timeout(std::time::Duration::from_secs(2), recv_text(&mut rx))
        .await
        .expect("read-ahead detected: first envelope never arrived");
    assert_eq!(first, "a");

    consumed.store(true, Ordering::SeqCst);
    gate.notify_one();

    // Timeout guard on the second phase too: a post-gate deadlock must
    // fail this test with a clear message, not hang the whole suite.
    let (second, start_result) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        (recv_text(&mut rx).await, handle.await)
    })
    .await
    .expect("post-gate deadlock: second envelope or consumer completion never arrived");
    assert_eq!(second, "b");
    start_result.unwrap().unwrap();
}

#[tokio::test]
async fn double_start_rejected() {
    let (mut consumer, ctx, _rx, _token, _errors) = test_setup(
        StreamConfig::from_uri("stream:in").unwrap(),
        Box::new(SharedReader::new(b"")),
    );
    consumer.mark_started_for_test();

    let err = consumer.start(ctx).await.unwrap_err();
    assert!(err.to_string().contains("already started"), "got: {err}");
}

// -------------------------------------------------------------------
// Endpoint wiring (Task 2.2)
// -------------------------------------------------------------------

/// Build a `StreamEndpoint` from a URI (private struct, same crate).
fn endpoint(uri: &str) -> StreamEndpoint {
    StreamEndpoint {
        uri: uri.to_string(),
        config: StreamConfig::from_uri(uri).unwrap(),
    }
}

#[test]
fn endpoint_creates_consumer_for_in() {
    let endpoint = endpoint("stream:in");
    let consumer = endpoint.create_consumer(Arc::new(RecordingRuntime::new(errors_log())));
    assert!(consumer.is_ok(), "stream:in must create a consumer");
}

#[test]
fn endpoint_rejects_consumer_for_out_and_err() {
    for uri in ["stream:out", "stream:err"] {
        let endpoint = endpoint(uri);
        let result = endpoint.create_consumer(Arc::new(RecordingRuntime::new(errors_log())));
        match result {
            Err(CamelError::EndpointCreationFailed(msg)) => {
                assert!(
                    msg.contains("stream:in"),
                    "error must name stream:in, got: {msg}"
                );
            }
            _other => panic!("expected EndpointCreationFailed for {uri}"),
        }
    }
}

#[test]
fn endpoint_rejects_producer_for_in() {
    let endpoint = endpoint("stream:in");
    let ctx = ProducerContext::new();
    let result = endpoint.create_producer(Arc::new(RecordingRuntime::new(errors_log())), &ctx);
    match result {
        Err(CamelError::EndpointCreationFailed(msg)) => {
            assert!(
                msg.contains("stream:out") && msg.contains("stream:err"),
                "error must name stream:out/stream:err, got: {msg}"
            );
        }
        other => {
            panic!("expected EndpointCreationFailed for stream:in producer, got: {other:?}")
        }
    }
}
