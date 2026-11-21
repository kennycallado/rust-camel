//! Unit tests for the SSE frame parser (task 1.1, change `sse-consumer`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use camel_api::Body;
use camel_api::Exchange;
use camel_api::ExchangePattern;
use camel_api::Message;
use camel_component_api::CamelError;
use camel_component_api::Component;
use camel_component_api::ComponentBundle;
use camel_component_api::ComponentRegistrar;
use camel_component_api::Consumer;
use camel_component_api::ConsumerContext;
use camel_component_api::Endpoint;
use camel_component_api::ExchangeEnvelope;
use camel_component_api::NoOpComponentContext;
use camel_component_api::ProducerContext;
use camel_component_api::RuntimeObservability;
use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::test_support::RecordingRuntimeObservability;
use camel_component_api::test_support::http::HttpTestServer;
use camel_component_api::test_support::http::RecordedRequest;
use camel_component_api::test_support::http::Responder;
use camel_component_api::test_support::http::Response;
use camel_component_api::test_support::http::ResponseHeaders;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::SseComponent;
use super::SseConsumer;
use super::SseCore;
use super::SseFrame;
use super::SseFrameParser;
use super::next_delay;
use crate::config::ExpiredCertAllowEntry;
use crate::config::HttpConfig;
use crate::config::TlsConfig;

fn only_event(frames: Vec<SseFrame>) -> super::SseEventFrame {
    assert_eq!(frames.len(), 1, "expected exactly one frame: {frames:?}");
    match frames.into_iter().next() {
        Some(SseFrame::Event(event)) => event,
        other => panic!("expected an event frame, got {other:?}"),
    }
}

#[test]
fn sse_parser_one_exchange_worth_of_frames_per_event() {
    let mut parser = SseFrameParser::new(1024);
    let frames = parser.push(b"data: hello\n\n").expect("push must succeed");
    let event = only_event(frames);
    assert_eq!(event.data, "hello");
}

#[test]
fn sse_parser_multiline_data_joins_with_newline_no_trailing() {
    let mut parser = SseFrameParser::new(1024);
    let frames = parser
        .push(b"data: a\ndata: b\ndata: c\n\n")
        .expect("push must succeed");
    let event = only_event(frames);
    assert_eq!(event.data, "a\nb\nc");
}

#[test]
fn sse_parser_comment_only_block_yields_comment_frame() {
    let mut parser = SseFrameParser::new(1024);
    let frames = parser.push(b": keep-alive\n\n").expect("push must succeed");
    assert_eq!(frames.len(), 1, "expected exactly one frame: {frames:?}");
    assert!(matches!(frames[0], SseFrame::Comment));
    assert!(
        !frames
            .iter()
            .any(|frame| matches!(frame, SseFrame::Event(_))),
        "comment block must not yield an event"
    );
}

#[test]
fn sse_parser_empty_data_block_suppressed() {
    let mut parser = SseFrameParser::new(1024);
    let event_only = parser.push(b"event: tick\n\n").expect("push must succeed");
    assert!(
        event_only.is_empty(),
        "event without data must be suppressed: {event_only:?}"
    );

    let empty_value = parser.push(b"data:\n\n").expect("push must succeed");
    assert!(
        empty_value.is_empty(),
        "empty data value must be suppressed: {empty_value:?}"
    );
}

#[test]
fn sse_parser_headers_event_id_retry_captured() {
    let mut parser = SseFrameParser::new(1024);
    let frames = parser
        .push(b"event: tick\nid: 42\nretry: 5000\ndata: x\n\n")
        .expect("push must succeed");
    let event = only_event(frames);
    assert_eq!(event.event.as_deref(), Some("tick"));
    assert_eq!(event.id.as_deref(), Some("42"));
    assert_eq!(event.retry_ms, Some(5000));
}

#[test]
fn sse_parser_invalid_retry_field_is_none() {
    let mut parser = SseFrameParser::new(1024);
    let frames = parser
        .push(b"retry: abc\ndata: x\n\n")
        .expect("push must succeed");
    let event = only_event(frames);
    assert_eq!(event.retry_ms, None);
}

#[test]
fn sse_parser_crlf_cr_lf_equivalent() {
    let mut lf_parser = SseFrameParser::new(1024);
    let lf_frames = lf_parser
        .push(b"data: a\n\ndata: b\n\n")
        .expect("LF push must succeed");

    let mut crlf_parser = SseFrameParser::new(1024);
    let crlf_frames = crlf_parser
        .push(b"data: a\r\n\r\ndata: b\r\n\r\n")
        .expect("CRLF push must succeed");

    let mut cr_parser = SseFrameParser::new(1024);
    let cr_frames = cr_parser
        .push(b"data: a\r\rdata: b\r\r")
        .expect("CR push must succeed");

    assert_eq!(lf_frames, crlf_frames);
    assert_eq!(lf_frames, cr_frames);
    assert_eq!(lf_frames.len(), 2);
}

#[test]
fn sse_parser_bom_stripped_only_at_stream_start() {
    let mut parser = SseFrameParser::new(1024);
    let first = parser
        .push(b"\xEF\xBB\xBFdata: a\n\n")
        .expect("first push must succeed");
    let first_event = only_event(first);
    assert_eq!(first_event.data, "a");

    let second = parser
        .push(b"data: \xEF\xBB\xBFb\n\n")
        .expect("second push must succeed");
    let second_event = only_event(second);
    assert!(
        second_event.data.contains('\u{FEFF}'),
        "BOM after stream start must pass through verbatim: {:?}",
        second_event.data
    );
}

#[test]
fn sse_parser_split_across_pushes() {
    let mut parser = SseFrameParser::new(1024);
    let first = parser.push(b"data: he").expect("first push must succeed");
    assert!(
        first.is_empty(),
        "partial line must not dispatch: {first:?}"
    );

    let second = parser
        .push(b"llo\n\nevent: t\ndata: x\n\n")
        .expect("second push must succeed");
    assert_eq!(second.len(), 2, "expected two frames: {second:?}");
    match (&second[0], &second[1]) {
        (SseFrame::Event(first_event), SseFrame::Event(second_event)) => {
            assert_eq!(first_event.data, "hello");
            assert_eq!(second_event.event.as_deref(), Some("t"));
            assert_eq!(second_event.data, "x");
        }
        other => panic!("expected two event frames, got {other:?}"),
    }
}

#[test]
fn sse_parser_eof_discards_unterminated_block() {
    let mut parser = SseFrameParser::new(1024);
    let frames = parser.push(b"data: partial").expect("push must succeed");
    assert!(frames.is_empty(), "unterminated block must not dispatch");
    let tail = parser.finish().expect("finish must succeed");
    assert!(tail.is_empty(), "finish must discard the block: {tail:?}");
}

#[test]
fn sse_parser_oversize_event_errors_within_bound() {
    let mut parser = SseFrameParser::new(8);
    let err = parser
        .push(b"data: 1234567890123\n\n")
        .expect_err("oversize event must error");
    match err {
        CamelError::Io(message) => assert!(
            message.contains("maxEventBytes"),
            "error must mention maxEventBytes: {message}"
        ),
        other => panic!("expected CamelError::Io, got {other:?}"),
    }
    assert!(
        parser.data.len() <= 8,
        "buffered data must never exceed the cap, got {} bytes",
        parser.data.len()
    );
}

#[test]
fn sse_parser_oversize_id_marked_not_stored() {
    let mut parser = SseFrameParser::new(1024 * 1024);
    let oversized_id = "a".repeat(1100);
    let stream = format!("id: {oversized_id}\ndata: x\n\n");
    let frames = parser.push(stream.as_bytes()).expect("push must succeed");
    let event = only_event(frames);
    assert_eq!(event.id.as_deref(), Some(oversized_id.as_str()));
    assert!(event.id_over_cap, "oversize id must be marked over cap");

    let normal = parser
        .push(b"id: 42\ndata: x\n\n")
        .expect("push must succeed");
    let normal_event = only_event(normal);
    assert_eq!(normal_event.id.as_deref(), Some("42"));
    assert!(
        !normal_event.id_over_cap,
        "42-byte-capped id must be storable"
    );
}

#[test]
fn sse_parser_lf_only_field_without_space() {
    let mut parser = SseFrameParser::new(1024);
    let frames = parser.push(b"data:x\n\n").expect("push must succeed");
    let event = only_event(frames);
    assert_eq!(event.data, "x");
}

#[test]
fn sse_parser_single_line_flood_bounded() {
    let mut parser = SseFrameParser::new(8);
    let mut flood = Vec::from(b"data: ".as_slice());
    flood.extend_from_slice(&[b'a'; 4096]);
    let err = parser
        .push(&flood)
        .expect_err("an unterminated data line past the cap must error");
    match err {
        CamelError::Io(message) => assert!(
            message.contains("maxEventBytes"),
            "error must mention maxEventBytes: {message}"
        ),
        other => panic!("expected CamelError::Io, got {other:?}"),
    }
    assert!(
        parser.line_buf.len() + parser.data.len() <= 64,
        "buffering must stop at the bound, got line_buf={} data={}",
        parser.line_buf.len(),
        parser.data.len()
    );
}

#[test]
fn sse_parser_field_line_flood_bounded() {
    // Guard 2: non-data field lines stop at the 64 KiB field-line cap.
    let mut parser = SseFrameParser::new(8);
    let mut flood = Vec::from(b"event: ".as_slice());
    flood.extend_from_slice(&[b'a'; 70_000]);
    let err = parser
        .push(&flood)
        .expect_err("a field line past the 64 KiB cap must error");
    match err {
        CamelError::Io(message) => assert!(
            message.contains("maxEventBytes"),
            "error must mention maxEventBytes: {message}"
        ),
        other => panic!("expected CamelError::Io, got {other:?}"),
    }
    assert!(
        parser.line_buf.len() <= 65_552,
        "field-line buffering must stay at the 64 KiB cap, got {}",
        parser.line_buf.len()
    );
}

#[test]
fn sse_parser_invalid_utf8_data_errors_strictly() {
    let mut parser = SseFrameParser::new(1024);
    let err = parser
        .push(b"data: \xFF\xFE\n\n")
        .expect_err("invalid UTF-8 data must error");
    match err {
        CamelError::Io(message) => {
            assert!(
                message.contains("invalid UTF-8"),
                "error must name UTF-8: {message}"
            );
            assert!(
                !message.contains('\u{FFFD}'),
                "error must not carry replacement characters: {message}"
            );
        }
        other => panic!("expected CamelError::Io, got {other:?}"),
    }
}

#[test]
fn sse_parser_valid_utf8_multibyte_data_ok() {
    // Two data lines, each holding complete multi-byte characters, join
    // with `\n` as usual.
    let mut parser = SseFrameParser::new(1024);
    let frames = parser
        .push("data: héllo\ndata: wörld\n\n".as_bytes())
        .expect("push must succeed");
    let event = only_event(frames);
    assert_eq!(event.data, "héllo\nwörld");

    // Accumulate-then-convert: one data value whose codepoint is split
    // across two pushes decodes from the concatenated bytes.
    let mut split = SseFrameParser::new(1024);
    let first = split.push(b"data: \xC3").expect("first push must succeed");
    assert!(
        first.is_empty(),
        "partial line must not dispatch: {first:?}"
    );
    let second = split.push(b"\xA9\n\n").expect("second push must succeed");
    let event = only_event(second);
    assert_eq!(event.data, "é");
}

// ---------------------------------------------------------------------------
// Config, component and endpoint (task 1.2, change `sse-consumer`)
// ---------------------------------------------------------------------------

fn test_component() -> SseComponent {
    SseComponent::new(HttpConfig::default())
}

#[test]
fn sse_config_defaults_pinned() {
    let endpoint = test_component()
        .build_endpoint("sse://127.0.0.1:1/stream")
        .expect("default sse URI must resolve");
    let config = endpoint.config;
    assert_eq!(config.max_event_bytes, 1_048_576, "maxEventBytes default");
    assert_eq!(config.idle_timeout_ms, 90_000, "idleTimeoutMs default");
    assert_eq!(
        config.connect_timeout_ms, 10_000,
        "connectTimeoutMs default"
    );
    assert_eq!(config.retry_initial_ms, 3_000, "retryInitialMs default");
    assert_eq!(config.retry_max_ms, 60_000, "retryMaxMs default");
}

#[test]
fn sse_endpoint_rejects_userinfo_url() {
    let err = test_component()
        .build_endpoint("sse://user:pass@127.0.0.1:1/stream?allowInternal=true")
        .expect_err("userinfo in an sse URL must be rejected");
    assert!(
        matches!(err, CamelError::EndpointCreationFailed(_)),
        "expected EndpointCreationFailed, got: {err:?}"
    );
    assert!(
        err.to_string().contains("userinfo"),
        "error must mention userinfo, got: {err}"
    );
}

#[test]
fn sse_endpoint_rejects_invalid_auth_headers() {
    let err = test_component()
        .build_endpoint("sse://127.0.0.1:1/stream?authHeaders=Bad%20Header=x")
        .expect_err("an invalid authHeaders header name must be rejected");
    assert!(
        matches!(err, CamelError::EndpointCreationFailed(_)),
        "expected EndpointCreationFailed, got: {err:?}"
    );
}

#[test]
fn sse_endpoint_rejects_invalid_auth_header_value() {
    // `%0A` decodes to a real newline inside the value; the interior control
    // byte survives trimming and fails `HeaderValue::from_str`.
    let err = test_component()
        .build_endpoint("sse://127.0.0.1:1/stream?authHeaders=X-Key=a%0Ab")
        .expect_err("a control byte in an authHeaders value must be rejected");
    assert!(
        matches!(err, CamelError::EndpointCreationFailed(_)),
        "expected EndpointCreationFailed, got: {err:?}"
    );
}

#[test]
fn sse_endpoint_debug_redacts_auth_headers() {
    let endpoint = test_component()
        .build_endpoint("sse://127.0.0.1:1/stream?authHeaders=X-Api-Key=sekret")
        .expect("authHeaders URI must resolve");

    let debug = format!("{endpoint:?}");
    assert!(
        !debug.contains("sekret"),
        "the Debug surface must not render authHeaders values: {debug}"
    );
    assert!(
        debug.contains("x-api-key"),
        "header names may stay visible, got: {debug}"
    );
}

#[test]
fn sse_retry_bounds_clamped_at_endpoint_creation() {
    let endpoint = test_component()
        .build_endpoint("sse://127.0.0.1:1/stream?retryInitialMs=100&retryMaxMs=999999")
        .expect("out-of-range retry bounds must clamp, not fail");
    assert_eq!(endpoint.config.retry_initial_ms, 1_000, "floor clamped");
    assert_eq!(endpoint.config.retry_max_ms, 60_000, "ceiling clamped");
}

#[test]
fn sse_endpoint_producer_rejected() {
    let endpoint = test_component()
        .build_endpoint("sse://127.0.0.1:1/stream")
        .expect("default sse URI must resolve");
    let err = endpoint
        .create_producer(Arc::new(NoopRuntimeObservability), &ProducerContext::new())
        .expect_err("sse must not create a producer");
    assert!(
        matches!(err, CamelError::Config(_)),
        "expected Config, got: {err:?}"
    );
    assert!(
        err.to_string().contains("consumer-only"),
        "error must mention consumer-only, got: {err}"
    );
}

struct RecordingRegistrar {
    schemes: Vec<String>,
}

impl ComponentRegistrar for RecordingRegistrar {
    fn register_component_dyn(&mut self, component: Arc<dyn camel_component_api::Component>) {
        self.schemes.push(component.scheme().to_string());
    }
}

#[test]
fn sse_component_registered_by_bundle() {
    let bundle = crate::HttpBundle::from_toml(toml::Value::Table(toml::map::Map::new()))
        .expect("empty http bundle toml must use defaults");
    let mut registrar = RecordingRegistrar {
        schemes: Vec::new(),
    };

    bundle.register_all(&mut registrar);

    assert_eq!(
        registrar.schemes,
        vec!["http", "https", "sse"],
        "http/https registrations must remain and sse must be added"
    );

    let capabilities = test_component().metadata().capabilities;
    assert!(
        capabilities.supports_consumer,
        "sse must declare consumer capability"
    );
    assert!(
        !capabilities.supports_producer,
        "sse must not declare producer capability"
    );
}

/// Pinned `sse:` metadata surface (task 1.7): the derive-generated descriptor
/// must expose exactly the task 1.2 options, with their types, defaults and
/// the `secret` flag on `authHeaders`, and consumer-only capabilities.
///
/// This is the machine-checked copy of the `sse` scheme descriptor:
/// `schemas/component-metadata.json` is the JSON Schema of the
/// `ComponentMetadata` *struct* (schemes are runtime registrations), so the
/// descriptor itself is only observable through `Component::metadata()`.
#[test]
fn sse_metadata_surface_pinned() {
    use camel_api::component_metadata::OptionKind;

    let metadata = test_component().metadata();
    assert_eq!(metadata.scheme, "sse", "descriptor scheme");
    assert!(
        metadata.capabilities.supports_consumer,
        "sse must declare consumer capability"
    );
    assert!(
        !metadata.capabilities.supports_producer,
        "sse must stay consumer-only"
    );

    let expected: [(&str, OptionKind, Option<&str>, bool); 8] = [
        ("maxEventBytes", OptionKind::Int, Some("1048576"), false),
        ("idleTimeoutMs", OptionKind::Int, Some("90000"), false),
        ("connectTimeoutMs", OptionKind::Int, Some("10000"), false),
        ("retryInitialMs", OptionKind::Int, Some("3000"), false),
        ("retryMaxMs", OptionKind::Int, Some("60000"), false),
        ("allowInternal", OptionKind::Bool, Some("false"), false),
        ("allowCleartext", OptionKind::Bool, Some("false"), false),
        ("authHeaders", OptionKind::String, None, true),
    ];
    let actual: Vec<(&str, OptionKind, Option<&str>, bool)> = metadata
        .uri_options
        .iter()
        .map(|option| {
            (
                option.name.as_str(),
                option.kind.clone(),
                option.default_value.as_deref(),
                option.secret,
            )
        })
        .collect();

    assert_eq!(
        actual, expected,
        "the sse: uri_options must match the task 1.2 contract"
    );
}

/// MVP exclusions (spec requirement "MVP exclusions"): no max-events cap, no
/// total-bytes cap, no body/request option, and no producer.
#[test]
fn sse_exclusions_surface_check() {
    let component = test_component();
    let metadata = component.metadata();

    let names: Vec<&str> = metadata
        .uri_options
        .iter()
        .map(|option| option.name.as_str())
        .collect();
    assert!(
        !names.contains(&"maxEvents"),
        "MVP excludes a max-events cap, got: {names:?}"
    );
    assert!(
        !names.contains(&"totalBytes"),
        "MVP excludes a total-bytes cap, got: {names:?}"
    );
    for name in &names {
        let lower = name.to_ascii_lowercase();
        for banned in ["body", "request", "payload"] {
            assert!(
                !lower.contains(banned),
                "sse: must expose no body/request option, found '{name}'"
            );
        }
    }
    assert!(
        !metadata.capabilities.supports_producer,
        "sse: must remain consumer-only, so no request surface"
    );

    let endpoint = component
        .build_endpoint("sse://127.0.0.1:1/stream")
        .expect("default sse URI must resolve");
    let err = endpoint
        .create_producer(Arc::new(NoopRuntimeObservability), &ProducerContext::new())
        .expect_err("sse must not create a producer (MVP exclusion)");
    assert!(
        matches!(err, CamelError::Config(_)),
        "expected Config, got: {err:?}"
    );
    assert!(
        err.to_string().contains("consumer-only"),
        "error must mention consumer-only, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Consumer connect + stream loop (task 1.3, change `sse-consumer`)
// ---------------------------------------------------------------------------

const ROUTE_ID: &str = "sse-test";
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);

fn sse_response_headers() -> ResponseHeaders {
    ResponseHeaders::new(
        200,
        "OK",
        vec![("Content-Type".to_string(), "text/event-stream".to_string())],
    )
}

/// One started `SseConsumer` bound to an mpsc-backed [`ConsumerContext`].
struct Harness {
    /// Kept alive for the whole test: dropping the consumer would tear the
    /// wiring down before the spawned task observes cancellation.
    _consumer: Box<dyn Consumer>,
    rx: mpsc::Receiver<ExchangeEnvelope>,
    cancel: CancellationToken,
    task: Option<JoinHandle<Result<(), CamelError>>>,
}

impl Harness {
    /// Next dispatched envelope, bounded by [`EVENT_TIMEOUT`].
    async fn next_envelope(&mut self) -> ExchangeEnvelope {
        tokio::time::timeout(EVENT_TIMEOUT, self.rx.recv())
            .await
            .expect("an exchange must arrive within the test timeout")
            .expect("consumer context channel must stay open")
    }

    /// Next dispatched exchange, bounded by [`EVENT_TIMEOUT`].
    async fn next_exchange(&mut self) -> Exchange {
        self.next_envelope().await.exchange
    }

    /// Joins the background task, bounded so a wedged consumer fails the test
    /// instead of hanging it.
    async fn task_result(&mut self) -> Result<(), CamelError> {
        self.task_result_within(EVENT_TIMEOUT).await
    }

    async fn task_result_within(&mut self, budget: Duration) -> Result<(), CamelError> {
        let task = self.task.take().expect("background task handle");
        tokio::time::timeout(budget, task)
            .await
            .expect("consumer task must finish within its budget")
            .expect("consumer task must not panic")
    }

    fn task_is_finished(&self) -> bool {
        self.task
            .as_ref()
            .expect("background task handle")
            .is_finished()
    }

    async fn shutdown(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
        }
    }
}

async fn start_consumer(
    uri: &str,
    config: HttpConfig,
    runtime: Arc<dyn RuntimeObservability>,
    channel_capacity: usize,
) -> Harness {
    start_consumer_with_channel(uri, config, runtime, mpsc::channel(channel_capacity)).await
}

async fn start_consumer_with_channel(
    uri: &str,
    config: HttpConfig,
    runtime: Arc<dyn RuntimeObservability>,
    channel: (
        mpsc::Sender<ExchangeEnvelope>,
        mpsc::Receiver<ExchangeEnvelope>,
    ),
) -> Harness {
    let endpoint = SseComponent::new(config)
        .create_endpoint(uri, &NoOpComponentContext)
        .expect("sse endpoint must build");
    let consumer = endpoint.create_consumer(runtime).expect("sse consumer");
    start_harness(consumer, channel).await
}

/// Starts any consumer implementation; the readiness handshake is the
/// consumer's own (`Explicit` startup waits for `mark_ready`).
async fn start_harness(
    mut consumer: Box<dyn Consumer>,
    (tx, rx): (
        mpsc::Sender<ExchangeEnvelope>,
        mpsc::Receiver<ExchangeEnvelope>,
    ),
) -> Harness {
    let cancel = CancellationToken::new();
    consumer
        .start(ConsumerContext::new(
            tx,
            cancel.clone(),
            ROUTE_ID.to_string(),
        ))
        .await
        .expect("sse consumer start");
    let task = consumer.background_task_handle();
    Harness {
        _consumer: consumer,
        rx,
        cancel,
        task,
    }
}

/// Starts an `SseConsumer` with a narrowed retry budget. The URI config has
/// no attempt knob, so the consumer is built directly from the typed endpoint
/// (same module access) and the policy is overridden before the task starts.
async fn start_consumer_with_budget(
    uri: &str,
    config: HttpConfig,
    runtime: Arc<dyn RuntimeObservability>,
    channel_capacity: usize,
    max_attempts: u32,
) -> Harness {
    let endpoint = SseComponent::new(config)
        .build_endpoint(uri)
        .expect("sse endpoint must build");
    let mut consumer = SseConsumer::new(
        endpoint.uri,
        endpoint.config,
        endpoint.auth_headers,
        endpoint.http_config,
        endpoint.pinned_cache,
        endpoint.client,
        runtime,
    );
    consumer.set_retry_max_attempts(max_attempts);
    start_harness(Box::new(consumer), mpsc::channel(channel_capacity)).await
}

fn sse_uri(port: u16, query: &str) -> String {
    format!("sse://127.0.0.1:{port}/events?{query}")
}

/// Waits (bounded) until the harness recorded `count` requests — a
/// synchronization barrier for tests that must act only once the consumer
/// connected.
async fn wait_for_requests(server: &HttpTestServer, count: usize) {
    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    while server.recorded().len() < count {
        assert!(
            tokio::time::Instant::now() < deadline,
            "harness never recorded {count} request(s)"
        );
        tokio::task::yield_now().await;
    }
}

/// Header lookup on a recorded request (recorder lowercases names).
fn recorded_header<'a>(request: &'a RecordedRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(header, _)| header.as_str() == name)
        .map(|(_, value)| value.as_str())
}

#[tokio::test]
async fn sse_consumer_emits_one_exchange_per_event() {
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    let server = HttpTestServer::start(Responder::Fixed(Response::staged(
        sse_response_headers(),
        chunk_rx,
    )))
    .await
    .expect("test server");
    chunk_tx
        .send(b"data: a\n\n".to_vec())
        .await
        .expect("first chunk queued");
    chunk_tx
        .send(b"data: b\n\n".to_vec())
        .await
        .expect("second chunk queued");
    drop(chunk_tx);

    let mut harness = start_consumer(
        &sse_uri(server.port(), "allowInternal=true"),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;

    let first = harness.next_exchange().await;
    assert_eq!(first.input.body, Body::Text("a".to_string()));
    assert_eq!(
        first.pattern,
        ExchangePattern::InOnly,
        "SSE events dispatch InOnly"
    );

    let second = harness.next_exchange().await;
    assert_eq!(second.input.body, Body::Text("b".to_string()));
    assert_eq!(second.pattern, ExchangePattern::InOnly);

    harness.shutdown().await;
}

#[tokio::test]
async fn sse_consumer_header_mapping() {
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    let server = HttpTestServer::start(Responder::Fixed(Response::staged(
        sse_response_headers(),
        chunk_rx,
    )))
    .await
    .expect("test server");
    chunk_tx
        .send(b"event: tick\nid: 42\nretry: 5000\ndata: x\n\n".to_vec())
        .await
        .expect("named event queued");
    chunk_tx
        .send(b"data: y\n\n".to_vec())
        .await
        .expect("bare event queued");
    drop(chunk_tx);

    let mut harness = start_consumer(
        &sse_uri(server.port(), "allowInternal=true"),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;

    let named = harness.next_exchange().await;
    assert_eq!(
        named.input.headers.get("CamelSseEvent"),
        Some(&serde_json::json!("tick"))
    );
    assert_eq!(
        named.input.headers.get("CamelSseId"),
        Some(&serde_json::json!("42"))
    );
    assert_eq!(
        named.input.headers.get("CamelSseRetry"),
        Some(&serde_json::json!("5000")),
        "retry must ride as a string"
    );

    let bare = harness.next_exchange().await;
    assert_eq!(
        bare.input.headers.get("CamelSseEvent"),
        Some(&serde_json::json!("message")),
        "a nameless event defaults to 'message'"
    );
    assert!(
        !bare.input.headers.contains_key("CamelSseId"),
        "no id, no CamelSseId"
    );
    assert!(
        !bare.input.headers.contains_key("CamelSseRetry"),
        "no retry, no CamelSseRetry"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn sse_consumer_terminal_wrong_content_type() {
    let server = HttpTestServer::start(Responder::Fixed(Response::with_status_and_headers(
        200,
        "OK",
        vec![("Content-Type".to_string(), "text/html".to_string())],
        "<html></html>",
    )))
    .await
    .expect("test server");

    let mut harness = start_consumer(
        &sse_uri(server.port(), "allowInternal=true"),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;

    let err = harness
        .task_result()
        .await
        .expect_err("a non-event-stream content type is terminal");
    assert!(
        err.to_string().contains("Content-Type"),
        "the task error must name the Content-Type deviation, got: {err}"
    );
    assert_eq!(
        server.recorded().len(),
        1,
        "a terminal answer must never be reconnected"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn sse_consumer_backpressure_next_read_waits_for_send() {
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    let server = HttpTestServer::start(Responder::Fixed(Response::staged(
        sse_response_headers(),
        chunk_rx,
    )))
    .await
    .expect("test server");
    chunk_tx
        .send(b"data: one\n\n".to_vec())
        .await
        .expect("first event queued");
    chunk_tx
        .send(b"data: two\n\n".to_vec())
        .await
        .expect("second event queued");
    drop(chunk_tx);

    // Capacity 1 with the single slot pre-filled: the context's `send` future
    // stays pending until the observer drains it — the send gate. The harness
    // cannot observe body reads directly, so the assertions cover the
    // dispatch boundary: while the gate is shut exactly one envelope may sit
    // in the queue, and once it opens the events arrive in wire order.
    //
    // Harness ceiling: a violating implementation that reads ahead parks on
    // the full queue after dispatching `send(two)` and presents the same
    // `rx.len() == 1`, so the capacity shape alone cannot discriminate a
    // queue-parked read-ahead from correct send-parked behavior — the
    // drop/reorder checks on the drain below are what catch a real violation.
    let (tx, rx) = mpsc::channel(1);
    tx.send(gate_envelope("gate"))
        .await
        .expect("queue pre-filled");
    let mut harness = start_consumer_with_channel(
        &sse_uri(server.port(), "allowInternal=true"),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        (tx, rx),
    )
    .await;
    wait_for_requests(&server, 1).await;

    // Give the consumer a bounded window to misbehave while the gate is shut:
    // a dispatch that bypassed the pending send would queue another envelope.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    while tokio::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        harness.rx.len(),
        1,
        "no dispatch may complete while the send gate is closed"
    );
    assert!(
        !harness.task_is_finished(),
        "the consumer must stay parked on the pending send"
    );

    // Open the gate: the pre-filled envelope drains, then events land in
    // order, one per completed send.
    let gate = harness.next_envelope().await;
    assert_eq!(gate.exchange.input.body, Body::Text("gate".to_string()));
    let first = harness.next_envelope().await;
    assert_eq!(first.exchange.input.body, Body::Text("one".to_string()));
    let second = harness.next_envelope().await;
    assert_eq!(second.exchange.input.body, Body::Text("two".to_string()));

    harness.shutdown().await;
}

fn gate_envelope(body: &str) -> ExchangeEnvelope {
    ExchangeEnvelope {
        exchange: Exchange::new(Message::new(Body::Text(body.to_string()))),
        reply_tx: None,
        in_flight_claim: None,
    }
}

#[tokio::test]
async fn sse_consumer_oversize_event_task_error_with_metric() {
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    let server = HttpTestServer::start(Responder::Fixed(Response::staged(
        sse_response_headers(),
        chunk_rx,
    )))
    .await
    .expect("test server");
    let mut oversized = Vec::from(b"data: ".as_slice());
    oversized.extend_from_slice(&[b'a'; 200]);
    oversized.extend_from_slice(b"\n\n");
    chunk_tx
        .send(oversized)
        .await
        .expect("oversize chunk queued");
    drop(chunk_tx);

    let runtime = RecordingRuntimeObservability::new(false);
    let observer: Arc<dyn RuntimeObservability> = runtime.clone();
    let mut harness = start_consumer(
        &sse_uri(server.port(), "allowInternal=true&maxEventBytes=64"),
        HttpConfig::default(),
        observer,
        16,
    )
    .await;

    let err = harness
        .task_result()
        .await
        .expect_err("an oversize event is a non-retryable task error");
    assert!(
        err.to_string().contains("maxEventBytes"),
        "the task error must mention maxEventBytes, got: {err}"
    );
    assert_eq!(
        runtime.errors(),
        vec![(ROUTE_ID.to_string(), "b-prime:sse:oversize".to_string())],
        "exactly one oversize metric emission"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn sse_consumer_invalid_utf8_task_error_with_metric() {
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    let server = HttpTestServer::start(Responder::Fixed(Response::staged(
        sse_response_headers(),
        chunk_rx,
    )))
    .await
    .expect("test server");
    chunk_tx
        .send(b"data: \xFF\xFE\n\n".to_vec())
        .await
        .expect("invalid UTF-8 chunk queued");
    drop(chunk_tx);

    let runtime = RecordingRuntimeObservability::new(false);
    let observer: Arc<dyn RuntimeObservability> = runtime.clone();
    let mut harness = start_consumer(
        &sse_uri(server.port(), "allowInternal=true"),
        HttpConfig::default(),
        observer,
        16,
    )
    .await;

    let err = harness
        .task_result()
        .await
        .expect_err("invalid UTF-8 event data is a non-retryable task error");
    let rendered = err.to_string();
    assert!(
        rendered.contains("UTF-8"),
        "the task error must name UTF-8, got: {rendered}"
    );
    assert!(
        !rendered.contains('\u{FFFD}'),
        "invalid UTF-8 must never be silenced as U+FFFD: {rendered}"
    );
    assert_eq!(
        runtime.errors(),
        vec![(ROUTE_ID.to_string(), "b-prime:sse:utf8".to_string())],
        "exactly one invalid-UTF-8 metric emission"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn sse_consumer_cancel_mid_read_stops_promptly() {
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    let server = HttpTestServer::start(Responder::Fixed(Response::staged(
        sse_response_headers(),
        chunk_rx,
    )))
    .await
    .expect("test server");
    chunk_tx
        .send(b"data: one\n\n".to_vec())
        .await
        .expect("event queued");
    // Deliberately keep `chunk_tx` alive: the server holds the stream open
    // after the first event, so the consumer parks in a read.

    let mut harness = start_consumer(
        &sse_uri(server.port(), "allowInternal=true"),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;
    let first = harness.next_exchange().await;
    assert_eq!(first.input.body, Body::Text("one".to_string()));

    harness.cancel.cancel();
    harness
        .task_result_within(Duration::from_secs(1))
        .await
        .expect("cancellation must stop the task promptly");
    assert!(
        harness.rx.try_recv().is_err(),
        "cancellation must not dispatch further exchanges"
    );

    drop(chunk_tx);
}

#[tokio::test]
async fn sse_consumer_sends_configured_headers_and_accept() {
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    let server = HttpTestServer::start(Responder::Fixed(Response::staged(
        sse_response_headers(),
        chunk_rx,
    )))
    .await
    .expect("test server");
    chunk_tx
        .send(b"data: x\n\n".to_vec())
        .await
        .expect("event queued");
    drop(chunk_tx);

    let mut harness = start_consumer(
        &sse_uri(
            server.port(),
            "allowInternal=true&authHeaders=X-Api-Key=secret",
        ),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;
    harness.next_exchange().await;

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 1, "exactly one connect for this stream");
    let request = &recorded[0];
    assert_eq!(
        recorded_header(request, "accept"),
        Some("text/event-stream"),
        "the SSE Accept header must ride the request"
    );
    assert_eq!(
        recorded_header(request, "x-api-key"),
        Some("secret"),
        "configured authHeaders must ride the request"
    );
    assert!(
        recorded_header(request, "last-event-id").is_none(),
        "no id stored yet, so no Last-Event-ID header"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn sse_consumer_json_data_stays_text() {
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    let server = HttpTestServer::start(Responder::Fixed(Response::staged(
        sse_response_headers(),
        chunk_rx,
    )))
    .await
    .expect("test server");
    chunk_tx
        .send(b"data: {\"a\":1}\n\n".to_vec())
        .await
        .expect("JSON-looking event queued");
    drop(chunk_tx);

    let mut harness = start_consumer(
        &sse_uri(server.port(), "allowInternal=true"),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;
    let exchange = harness.next_exchange().await;
    assert_eq!(
        exchange.input.body,
        Body::Text("{\"a\":1}".to_string()),
        "SSE data stays raw text; it is never parsed as JSON"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn sse_consumer_idle_timeout_includes_comments() {
    let (chunk_tx, chunk_rx) = mpsc::channel(64);
    let server = HttpTestServer::start(Responder::Fixed(Response::staged(
        sse_response_headers(),
        chunk_rx,
    )))
    .await
    .expect("test server");

    let mut harness = start_consumer(
        &sse_uri(server.port(), "allowInternal=true&idleTimeoutMs=500"),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;
    wait_for_requests(&server, 1).await;

    // 300 ms heartbeats for ~2 s: every comment byte resets the 500 ms idle
    // budget, so the consumer must stay on this one connection.
    for _ in 0..7 {
        chunk_tx
            .send(b":k\n\n".to_vec())
            .await
            .expect("heartbeat queued");
        tokio::time::sleep(Duration::from_millis(300)).await; // allow-test-sleep: heartbeats must land inside the 500 ms idle budget
    }

    assert!(
        !harness.task_is_finished(),
        "comment heartbeats reset the idle timer, so the task must still run"
    );
    assert_eq!(
        server.recorded().len(),
        1,
        "a healthy comment stream must never reconnect"
    );

    drop(chunk_tx);
    harness.shutdown().await;
}

#[tokio::test]
async fn sse_connect_redirect_strips_authorization_cross_origin() {
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    let server_b = HttpTestServer::start(Responder::Fixed(Response::staged(
        sse_response_headers(),
        chunk_rx,
    )))
    .await
    .expect("redirect target server");
    chunk_tx
        .send(b"data: redirected\n\n".to_vec())
        .await
        .expect("event queued");
    drop(chunk_tx);

    // Different port on 127.0.0.1 == different origin.
    let location = format!("http://127.0.0.1:{}/events", server_b.port());
    let server_a = HttpTestServer::start(Responder::Fixed(Response::with_status_and_headers(
        302,
        "Found",
        vec![("Location".to_string(), location)],
        Vec::new(),
    )))
    .await
    .expect("redirecting server");

    let mut harness = start_consumer(
        &sse_uri(
            server_a.port(),
            "allowInternal=true&authHeaders=Authorization=Bearer sekrit",
        ),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;

    let exchange = harness.next_exchange().await;
    assert_eq!(exchange.input.body, Body::Text("redirected".to_string()));

    let first_hop = server_a.recorded();
    assert_eq!(first_hop.len(), 1, "one request to the redirecting origin");
    assert_eq!(
        recorded_header(&first_hop[0], "authorization"),
        Some("Bearer sekrit"),
        "the first hop carries the configured credential"
    );

    let second_hop = server_b.recorded();
    assert_eq!(second_hop.len(), 1, "the redirect target was reached");
    assert!(
        recorded_header(&second_hop[0], "authorization").is_none(),
        "ADR-0081: cross-origin redirects must strip Authorization"
    );

    harness.shutdown().await;
}

// --- TLS harness (expired-cert allowlist over a real loopback handshake) ---

fn crypto_provider() -> Arc<rustls::crypto::CryptoProvider> {
    rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
}

/// One-shot loopback TLS server serving an SSE reply over a single
/// connection. Mirrors `crate::tls_harness::spawn_tls_server` (same bounded
/// accept/read/write discipline) but writes an event-stream response instead
/// of the shared `ok` reply.
async fn spawn_tls_sse_server(
    cert_pem: &str,
    key_pem: &str,
    body: Vec<u8>,
) -> (SocketAddr, JoinHandle<()>) {
    use crate::tls_harness::HARNESS_IO_TIMEOUT;

    let chain: Vec<_> = rustls_pemfile::certs(&mut std::io::Cursor::new(cert_pem.as_bytes()))
        .collect::<Result<_, _>>()
        .expect("server cert pem parses"); // allow-unwrap(test)
    let key = rustls_pemfile::private_key(&mut std::io::Cursor::new(key_pem.as_bytes()))
        .expect("server key pem parses") // allow-unwrap(test)
        .expect("server key pem carries a private key section"); // allow-unwrap(test)

    let server_config = rustls::ServerConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .expect("stock rustls provider supports the safe default protocol versions") // allow-unwrap(test)
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .expect("server TLS config builds"); // allow-unwrap(test)

    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener binds"); // allow-unwrap(test)
    let addr = listener
        .local_addr()
        .expect("loopback listener yields a local addr"); // allow-unwrap(test)

    let task = tokio::spawn(async move {
        let tcp = match tokio::time::timeout(HARNESS_IO_TIMEOUT, listener.accept()).await {
            Ok(Ok((stream, _))) => stream,
            _ => return,
        };
        let mut stream = match tokio::time::timeout(HARNESS_IO_TIMEOUT, acceptor.accept(tcp)).await
        {
            Ok(Ok(stream)) => stream,
            _ => return,
        };
        let mut buf = [0u8; 8192];
        let _ = tokio::time::timeout(HARNESS_IO_TIMEOUT, stream.read(&mut buf)).await;
        let mut reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        reply.extend_from_slice(&body);
        let _ = tokio::time::timeout(HARNESS_IO_TIMEOUT, stream.write_all(&reply)).await;
        let _ = tokio::time::timeout(HARNESS_IO_TIMEOUT, stream.shutdown()).await;
    });

    (addr, task)
}

#[tokio::test]
async fn sse_consumer_tls_allowlist_pinned_host() {
    let ca = crate::tls_harness::gen_test_ca();
    let leaf = crate::tls_harness::gen_expired_leaf_signed_by_ca(
        &ca,
        rcgen::SanType::IpAddress("127.0.0.1".parse().expect("loopback ip")), // allow-unwrap(test)
    );
    let fingerprint = crate::tls_harness::sha256_fingerprint_hex(leaf.cert.der().as_ref());
    let (addr, server_task) =
        spawn_tls_sse_server(&leaf.cert_pem, &leaf.key_pem, b"data: tls\n\n".to_vec()).await;

    // Exactly the `https:` producer-test shape: TLS on, one pinned expired
    // leaf for this host, loopback SSRF consent.
    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            expired_allowlist: vec![ExpiredCertAllowEntry {
                host: "127.0.0.1".to_string(),
                port: addr.port(),
                fingerprint,
            }],
            ..Default::default()
        }),
        allow_internal: true,
        ..Default::default()
    };

    let mut harness = start_consumer(
        &sse_uri(addr.port(), "allowInternal=true"),
        config,
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;

    let exchange = harness.next_exchange().await;
    assert_eq!(
        exchange.input.body,
        Body::Text("tls".to_string()),
        "events must flow over the pinned TLS connection"
    );

    harness.shutdown().await;
    server_task.abort();
}

// ---------------------------------------------------------------------------
// Scheme translation + strict-TLS fail-closed (task 1.3 review findings)
// ---------------------------------------------------------------------------

/// Consumer core for a URI/config pair, so the pure wire-URL translation can
/// be asserted without a server.
fn core_for(uri: &str, config: HttpConfig) -> SseCore {
    let endpoint = SseComponent::new(config)
        .build_endpoint(uri)
        .expect("sse endpoint must build");
    let retry_policy = super::retry_policy_for(&endpoint.config);
    SseCore {
        uri: endpoint.uri,
        config: endpoint.config,
        auth_headers: endpoint.auth_headers,
        http_config: endpoint.http_config,
        pinned_cache: endpoint.pinned_cache,
        client: endpoint.client,
        runtime: Arc::new(NoopRuntimeObservability),
        retry_policy,
        last_event_id: std::sync::Mutex::new(None),
    }
}

#[test]
fn sse_wire_url_scheme_selection() {
    // Secure by default: no TLS material and no cleartext consent.
    assert_eq!(
        core_for("sse://api.example.com/events", HttpConfig::default()).wire_url(),
        "https://api.example.com/events"
    );

    // Internal target with SSRF consent: loopback rides cleartext http.
    assert_eq!(
        core_for(
            "sse://127.0.0.1:8080/events?allowInternal=true",
            HttpConfig::default()
        )
        .wire_url(),
        "http://127.0.0.1:8080/events"
    );

    // A local name is an internal target too.
    assert_eq!(
        core_for(
            "sse://localhost:8080/events?allowInternal=true",
            HttpConfig::default()
        )
        .wire_url(),
        "http://localhost:8080/events"
    );

    // allowInternal alone never downgrades a public host.
    assert_eq!(
        core_for(
            "sse://api.example.com/events?allowInternal=true",
            HttpConfig::default()
        )
        .wire_url(),
        "https://api.example.com/events"
    );

    // Explicit public-cleartext consent (ADR-0081).
    assert_eq!(
        core_for(
            "sse://api.example.com/events?allowCleartext=true",
            HttpConfig::default()
        )
        .wire_url(),
        "http://api.example.com/events"
    );

    // TLS material wins over the internal-cleartext allowance.
    let tls_config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            ..TlsConfig::default()
        }),
        ..HttpConfig::default()
    };
    assert_eq!(
        core_for("sse://127.0.0.1:8080/events?allowInternal=true", tls_config).wire_url(),
        "https://127.0.0.1:8080/events"
    );
}

#[test]
fn sse_create_endpoint_fails_closed_on_broken_strict_tls_material() {
    // `tls.strict` + an unreadable CA: the component holds an emergency
    // client, so endpoint creation must refuse instead of serving a
    // silently downgraded verification path (mirrors the http/https
    // producer tests).
    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: true,
            ca_cert_path: Some("/nonexistent/sse-strict-ca.pem".to_string()),
            ..TlsConfig::default()
        }),
        ..HttpConfig::default()
    };

    let err = SseComponent::new(config)
        .create_endpoint(
            "sse://127.0.0.1:1/events?allowInternal=true",
            &NoOpComponentContext,
        )
        .err()
        .expect("strict TLS material failure must fail endpoint creation");
    assert!(
        err.to_string().contains("tls.strict"),
        "must name the strict knob: {err}"
    );
    assert!(
        err.to_string().contains("unreadable"),
        "must name the failure class: {err}"
    );
}

// ---------------------------------------------------------------------------
// Resubscription loop (task 1.4, change `sse-consumer`)
// ---------------------------------------------------------------------------

/// Serves each scripted response in order, then `fallback` forever. Route
/// closures are synchronous, so the responses are pre-built and popped from a
/// shared queue.
fn sequenced_route(responses: Vec<Response>, fallback: Response) -> Responder {
    let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(responses)));
    Responder::route(move |_request| {
        queue
            .lock()
            .expect("sequenced responder queue")
            .pop_front()
            .unwrap_or_else(|| fallback.clone())
    })
}

#[tokio::test]
async fn sse_reconnect_5xx_retried_then_succeeds() {
    let served: Arc<Mutex<Vec<u16>>> = Arc::new(Mutex::new(Vec::new()));
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    chunk_tx
        .send(b"data: ok\n\n".to_vec())
        .await
        .expect("event queued");
    let stream_rx = Arc::new(Mutex::new(Some(chunk_rx)));
    let server = HttpTestServer::start(Responder::route({
        let served = Arc::clone(&served);
        let stream_rx = Arc::clone(&stream_rx);
        move |_request| {
            let mut served = served.lock().expect("status log");
            if served.is_empty() {
                served.push(503);
                Response::with_status(503, "Service Unavailable", Vec::new())
            } else if served.len() == 1 {
                served.push(200);
                let rx = stream_rx
                    .lock()
                    .expect("stream slot")
                    .take()
                    .expect("stream served once");
                Response::staged(sse_response_headers(), rx)
            } else {
                served.push(503);
                Response::with_status(503, "Service Unavailable", Vec::new())
            }
        }
    }))
    .await
    .expect("test server");

    let mut harness = start_consumer(
        &sse_uri(
            server.port(),
            "allowInternal=true&retryInitialMs=1000&retryMaxMs=1000",
        ),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;

    let exchange = harness.next_exchange().await;
    assert_eq!(exchange.input.body, Body::Text("ok".to_string()));

    wait_for_requests(&server, 2).await;
    let recorded = server.recorded();
    assert_eq!(recorded.len(), 2, "503 then one retried subscribe");
    assert_eq!(
        served.lock().expect("status log")[..2],
        [503, 200],
        "the first subscribe got 503 and the retry got the stream"
    );
    assert!(
        recorded_header(&recorded[1], "last-event-id").is_none(),
        "no id was received before the reconnect, so no Last-Event-ID header"
    );
    assert!(
        !harness.task_is_finished(),
        "the retried stream is still open, so the task must still be running"
    );

    drop(chunk_tx);
    harness.shutdown().await;
}

#[tokio::test]
async fn sse_reconnect_budget_reset_on_first_complete_event_comment() {
    run_budget_reset_variant(b":k\n\n").await;
}

#[tokio::test]
async fn sse_reconnect_budget_reset_on_first_complete_event_event() {
    run_budget_reset_variant(b"data: x\n\n").await;
}

/// Observable-contract body for both budget-reset branches (comment and
/// event): the reconnect after the first parsed frame must use the INITIAL
/// 1 s delay, not the escalated 2 s an unreset counter would schedule.
async fn run_budget_reset_variant(first_frame: &'static [u8]) {
    let served_at: Arc<Mutex<Vec<tokio::time::Instant>>> = Arc::new(Mutex::new(Vec::new()));
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    chunk_tx
        .send(first_frame.to_vec())
        .await
        .expect("frame queued");
    drop(chunk_tx);
    let stream_rx = Arc::new(Mutex::new(Some(chunk_rx)));
    let server = HttpTestServer::start(Responder::route({
        let served_at = Arc::clone(&served_at);
        let stream_rx = Arc::clone(&stream_rx);
        move |_request| {
            let mut served_at = served_at.lock().expect("serve log");
            served_at.push(tokio::time::Instant::now());
            if served_at.len() == 2 {
                let rx = stream_rx
                    .lock()
                    .expect("stream slot")
                    .take()
                    .expect("stream served once");
                Response::staged(sse_response_headers(), rx)
            } else {
                Response::with_status(503, "Service Unavailable", Vec::new())
            }
        }
    }))
    .await
    .expect("test server");

    let mut harness = start_consumer_with_budget(
        &sse_uri(
            server.port(),
            "allowInternal=true&retryInitialMs=1000&retryMaxMs=2000",
        ),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
        2,
    )
    .await;

    let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
    while served_at.lock().expect("serve log").len() < 3 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the third request never arrived"
        );
        tokio::task::yield_now().await;
    }

    let elapsed = {
        let served_at = served_at.lock().expect("serve log");
        served_at[2].duration_since(served_at[1])
    };
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed <= Duration::from_millis(1500),
        "a parsed frame must reset the budget: expected the ~1 s initial delay, got {elapsed:?}"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn sse_reconnect_budget_exhaustion_is_task_error() {
    let server = HttpTestServer::start(Responder::Fixed(Response::with_status(
        503,
        "Service Unavailable",
        Vec::new(),
    )))
    .await
    .expect("test server");

    let mut harness = start_consumer_with_budget(
        &sse_uri(
            server.port(),
            "allowInternal=true&retryInitialMs=1000&retryMaxMs=1000",
        ),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
        2,
    )
    .await;

    let err = harness
        .task_result()
        .await
        .expect_err("an exhausted retry budget is a task error");
    assert!(
        err.to_string()
            .contains("retry budget exhausted after 2 attempts"),
        "the task error must name the exhausted budget, got: {err}"
    );
    assert_eq!(
        server.recorded().len(),
        3,
        "initial attempt plus two retries before the budget refuses"
    );

    harness.shutdown().await;
}

#[test]
fn sse_reconnect_server_retry_field_overrides_delay() {
    let cfg = SseComponent::new(HttpConfig::default())
        .build_endpoint("sse://127.0.0.1:1/events?allowInternal=true")
        .expect("sse endpoint must build")
        .config;

    assert_eq!(
        next_delay(0, None, &cfg),
        Duration::from_secs(3),
        "no override defaults to the 3 s initial delay"
    );
    assert_eq!(
        next_delay(0, Some(5000), &cfg),
        Duration::from_secs(5),
        "a server retry: field overrides the policy delay"
    );
    assert_eq!(
        next_delay(0, Some(50), &cfg),
        Duration::from_secs(1),
        "an override below the floor clamps to 1 s"
    );
    assert_eq!(
        next_delay(0, Some(70_000), &cfg),
        Duration::from_secs(60),
        "an override above the ceiling clamps to 60 s"
    );
    assert_eq!(
        next_delay(2, None, &cfg),
        Duration::from_secs(12),
        "the policy backoff escalates (3 s * 2^2)"
    );

    for delay in [
        next_delay(0, None, &cfg),
        next_delay(0, Some(5000), &cfg),
        next_delay(0, Some(50), &cfg),
        next_delay(0, Some(70_000), &cfg),
        next_delay(2, None, &cfg),
    ] {
        assert!(
            delay >= Duration::from_secs(1) && delay <= Duration::from_secs(60),
            "every scheduled delay stays inside [1 s, 60 s], got {delay:?}"
        );
    }
}

#[tokio::test]
async fn sse_reconnect_last_event_id_resent_from_memory() {
    let (tx1, rx1) = mpsc::channel(8);
    tx1.send(b"id: 42\ndata: a\n\n".to_vec())
        .await
        .expect("first id event queued");
    drop(tx1);

    let oversized_id = "a".repeat(1025);
    let (tx2, rx2) = mpsc::channel(8);
    tx2.send(b"id: 43\ndata: b\n\n".to_vec())
        .await
        .expect("second id event queued");
    tx2.send(format!("id: {oversized_id}\ndata: c\n\n").into_bytes())
        .await
        .expect("oversized id event queued");
    drop(tx2);

    // The third stream stays open: two reconnects are enough to observe the
    // stored id on each request.
    let (tx3, rx3) = mpsc::channel(8);
    let server = HttpTestServer::start(sequenced_route(
        vec![
            Response::staged(sse_response_headers(), rx1),
            Response::staged(sse_response_headers(), rx2),
            Response::staged(sse_response_headers(), rx3),
        ],
        Response::with_status(404, "Not Found", Vec::new()),
    ))
    .await
    .expect("test server");

    let mut harness = start_consumer(
        &sse_uri(
            server.port(),
            "allowInternal=true&retryInitialMs=1000&retryMaxMs=1000",
        ),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;

    let first = harness.next_exchange().await;
    assert_eq!(first.input.body, Body::Text("a".to_string()));
    let second = harness.next_exchange().await;
    assert_eq!(second.input.body, Body::Text("b".to_string()));
    let third = harness.next_exchange().await;
    assert_eq!(
        third.input.body,
        Body::Text("c".to_string()),
        "an over-cap id still reaches the exchange"
    );

    wait_for_requests(&server, 3).await;
    let recorded = server.recorded();
    assert_eq!(recorded.len(), 3);
    assert_eq!(
        recorded_header(&recorded[1], "last-event-id"),
        Some("42"),
        "the in-memory id rides the first reconnect"
    );
    assert_eq!(
        recorded_header(&recorded[2], "last-event-id"),
        Some("43"),
        "the over-cap id is never stored or resent"
    );

    // Keep the third stream open so the count stays at three.
    let _keep_open = tx3;
    harness.shutdown().await;
}

#[tokio::test]
async fn sse_reconnect_idle_timeout_fires_on_silence() {
    let (tx1, rx1) = mpsc::channel(8);
    tx1.send(b":k\n\n".to_vec()).await.expect("comment queued");
    let (tx2, rx2) = mpsc::channel(8);
    tx2.send(b"data: live\n\n".to_vec())
        .await
        .expect("event queued");
    let server = HttpTestServer::start(sequenced_route(
        vec![
            Response::staged(sse_response_headers(), rx1),
            Response::staged(sse_response_headers(), rx2),
        ],
        Response::with_status(404, "Not Found", Vec::new()),
    ))
    .await
    .expect("test server");

    let mut harness = start_consumer(
        &sse_uri(
            server.port(),
            "allowInternal=true&idleTimeoutMs=500&retryInitialMs=1000&retryMaxMs=1000",
        ),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;

    let exchange = harness.next_exchange().await;
    assert_eq!(
        exchange.input.body,
        Body::Text("live".to_string()),
        "silence past the idle bound is retriable, not terminal"
    );

    wait_for_requests(&server, 2).await;
    assert_eq!(
        server.recorded().len(),
        2,
        "exactly one reconnect after the idle timeout fired"
    );

    // Both senders stay alive: stream 1 stays silent (no EOF) and stream 2
    // stays open, so no extra reconnect is due.
    let _silent = tx1;
    let _live = tx2;
    harness.shutdown().await;
}

#[tokio::test]
async fn sse_reconnect_connect_timeout_bounded() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("silent listener binds");
    let port = listener
        .local_addr()
        .expect("silent listener yields an address")
        .port();
    let accepts = Arc::new(AtomicUsize::new(0));
    // Keeping the accepted streams alive holds the socket open with no bytes
    // written: reqwest never sees a response head, so the connect timeout is
    // what ends each attempt.
    let held: Arc<Mutex<Vec<tokio::net::TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
    let accept_task = tokio::spawn({
        let accepts = Arc::clone(&accepts);
        let held = Arc::clone(&held);
        async move {
            loop {
                let Ok((stream, _peer)) = listener.accept().await else {
                    return;
                };
                accepts.fetch_add(1, Ordering::SeqCst);
                held.lock().expect("held streams").push(stream);
            }
        }
    });

    let uri = format!(
        "sse://127.0.0.1:{port}/events?allowInternal=true&connectTimeoutMs=200&retryInitialMs=1000&retryMaxMs=1000"
    );
    let mut harness = start_consumer_with_budget(
        &uri,
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
        2,
    )
    .await;

    let err = harness
        .task_result_within(Duration::from_secs(10))
        .await
        .expect_err("connect timeouts exhaust the budget; they are not terminal");
    assert!(
        err.to_string().contains("retry budget exhausted"),
        "the task error must name the exhausted budget, got: {err}"
    );
    assert_eq!(
        accepts.load(Ordering::SeqCst),
        3,
        "initial attempt plus two retries: every attempt timed out and was retried"
    );

    accept_task.abort();
    harness.shutdown().await;
}

#[tokio::test]
async fn sse_reconnect_cancel_mid_sleep_stops_promptly() {
    let server = HttpTestServer::start(Responder::Fixed(Response::with_status(
        503,
        "Service Unavailable",
        Vec::new(),
    )))
    .await
    .expect("test server");

    let mut harness = start_consumer(
        &sse_uri(
            server.port(),
            "allowInternal=true&retryInitialMs=60000&retryMaxMs=60000",
        ),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;
    wait_for_requests(&server, 1).await;

    // The 503 schedules a 60 s sleep; cancellation must wake it immediately.
    harness.cancel.cancel();
    harness
        .task_result_within(Duration::from_secs(1))
        .await
        .expect("cancellation must abort the reconnect sleep promptly");
    assert_eq!(
        server.recorded().len(),
        1,
        "no second connect may start after cancellation"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn sse_reconnect_target_is_configured_url_not_redirect() {
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    chunk_tx
        .send(b"data: one\n\n".to_vec())
        .await
        .expect("event queued");
    drop(chunk_tx);
    let stream_rx = Arc::new(Mutex::new(Some(chunk_rx)));
    let attempts = Arc::new(AtomicUsize::new(0));
    let server = HttpTestServer::start(Responder::route({
        let attempts = Arc::clone(&attempts);
        let stream_rx = Arc::clone(&stream_rx);
        move |_request| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
            match attempt {
                1 => Response::with_status_and_headers(
                    302,
                    "Found",
                    vec![("Location".to_string(), "/alt".to_string())],
                    Vec::new(),
                ),
                2 => Response::staged(
                    sse_response_headers(),
                    stream_rx
                        .lock()
                        .expect("stream slot")
                        .take()
                        .expect("stream served once"),
                ),
                _ => Response::with_status(404, "Not Found", Vec::new()),
            }
        }
    }))
    .await
    .expect("test server");

    let mut harness = start_consumer(
        &sse_uri(
            server.port(),
            "allowInternal=true&retryInitialMs=1000&retryMaxMs=1000",
        ),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;

    let exchange = harness.next_exchange().await;
    assert_eq!(exchange.input.body, Body::Text("one".to_string()));

    // The reconnect lands on the configured path and its 404 is terminal.
    harness
        .task_result()
        .await
        .expect_err("the reconnect's 404 is terminal");

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 3, "redirect hop, stream, one reconnect");
    assert!(
        recorded[0].target.starts_with("/events"),
        "the initial connect uses the configured path: {}",
        recorded[0].target
    );
    assert!(
        recorded[1].target.starts_with("/alt"),
        "the connect follows the redirect within the same attempt: {}",
        recorded[1].target
    );
    assert!(
        recorded[2].target.starts_with("/events"),
        "the reconnect must target the configured URL, not the redirect path: {}",
        recorded[2].target
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn sse_reconnect_ssrf_revalidation_runs_per_connect() {
    let (tx1, rx1) = mpsc::channel(8);
    tx1.send(b"data: first\n\n".to_vec())
        .await
        .expect("first event queued");
    drop(tx1);
    let (tx2, rx2) = mpsc::channel(8);
    tx2.send(b"data: second\n\n".to_vec())
        .await
        .expect("second event queued");
    let server = HttpTestServer::start(sequenced_route(
        vec![
            Response::staged(sse_response_headers(), rx1),
            Response::staged(sse_response_headers(), rx2),
        ],
        Response::with_status(404, "Not Found", Vec::new()),
    ))
    .await
    .expect("test server");

    let mut harness = start_consumer(
        &sse_uri(
            server.port(),
            "allowInternal=true&retryInitialMs=1000&retryMaxMs=1000",
        ),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;

    let first = harness.next_exchange().await;
    assert_eq!(first.input.body, Body::Text("first".to_string()));
    let second = harness.next_exchange().await;
    assert_eq!(
        second.input.body,
        Body::Text("second".to_string()),
        "the reconnect delivered the second stream"
    );

    wait_for_requests(&server, 2).await;
    let recorded = server.recorded();
    assert_eq!(
        recorded.len(),
        2,
        "exactly one reconnect, each connect through the full security pass"
    );
    let host = format!("127.0.0.1:{}", server.port());
    for request in &recorded {
        assert!(
            request.target.starts_with("/events"),
            "both connects target the configured path: {}",
            request.target
        );
        assert_eq!(
            recorded_header(request, "host"),
            Some(host.as_str()),
            "both connects target the configured host"
        );
    }

    // Keep the second stream open so no third request appears.
    let _keep_open = tx2;
    harness.shutdown().await;
}

#[tokio::test]
async fn sse_consumer_204_terminal_no_reconnect() {
    let server = HttpTestServer::start(Responder::Fixed(Response::with_status(
        204,
        "No Content",
        Vec::new(),
    )))
    .await
    .expect("test server");

    let mut harness = start_consumer(
        &sse_uri(
            server.port(),
            "allowInternal=true&retryInitialMs=1000&retryMaxMs=1000",
        ),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;

    let err = harness
        .task_result()
        .await
        .expect_err("a 204 response is a terminal task error");
    assert!(
        err.to_string().contains("204"),
        "the task error must name the terminal status, got: {err}"
    );
    assert_eq!(
        server.recorded().len(),
        1,
        "a terminal status must never be reconnected"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn sse_consumer_403_terminal_no_reconnect() {
    let server = HttpTestServer::start(Responder::Fixed(Response::with_status(
        403,
        "Forbidden",
        Vec::new(),
    )))
    .await
    .expect("test server");

    let mut harness = start_consumer(
        &sse_uri(
            server.port(),
            "allowInternal=true&retryInitialMs=1000&retryMaxMs=1000",
        ),
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
    )
    .await;

    let err = harness
        .task_result()
        .await
        .expect_err("a 403 response is a terminal task error");
    assert!(
        err.to_string().contains("403"),
        "the task error must name the terminal status, got: {err}"
    );
    assert_eq!(
        server.recorded().len(),
        1,
        "a terminal status must never be reconnected"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn sse_reconnect_connection_refused_is_retriable() {
    // Bind-then-drop: the port accepts no connection when the consumer dials
    // it, so the send fails with a transport-class error (connection refused)
    // rather than a terminal policy rejection.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("probe listener binds");
    let port = listener
        .local_addr()
        .expect("probe listener yields an address")
        .port();
    drop(listener);

    let uri = format!(
        "sse://127.0.0.1:{port}/events?allowInternal=true&retryInitialMs=1000&retryMaxMs=1000"
    );
    let mut harness = start_consumer_with_budget(
        &uri,
        HttpConfig::default(),
        Arc::new(NoopRuntimeObservability),
        16,
        2,
    )
    .await;

    let err = harness
        .task_result()
        .await
        .expect_err("a refused connection is retried until the budget is exhausted");
    assert!(
        err.to_string().contains("retry budget exhausted"),
        "connection refused must take the Retriable arm (budget exhaustion), got: {err}"
    );

    harness.shutdown().await;
}
