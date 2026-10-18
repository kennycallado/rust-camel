//! Per-profile consumer dispatch: the consumer announces its profile as
//! `x-cxf-profile` metadata on every `OpenConsumerStream`.

mod support;

use std::sync::Arc;
use std::time::Duration;

use camel_api::{Message, RuntimeQuery, RuntimeQueryResult, SupervisionConfig};
use camel_component_api::Body;
use camel_component_api::NetworkRetryPolicy;
use camel_component_api::NoopRuntimeObservability;
use camel_component_api::consumer::{Consumer, ConsumerContext, ExchangeEnvelope};
use camel_component_api::test_support::acquire_deadline;
use camel_component_cxf::config::{CxfPoolConfig, CxfProfileConfig};
use camel_component_cxf::consumer::CxfConsumer;
use camel_component_cxf::proto::ConsumerRequest;
use camel_component_cxf::{BridgeSlot, CxfBridgePool, CxfComponent};
use camel_core::{CamelContext, RouteDefinition};
use support::mock_bridge::{MockState, spawn_mock_bridge, wait_for_open_count};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

async fn mock_pool(
    profiles: Vec<CxfProfileConfig>,
) -> Result<(Arc<CxfBridgePool>, MockState), BoxErr> {
    let (port, state) = spawn_mock_bridge().await?;
    let endpoint = format!("http://127.0.0.1:{port}");
    let channel = tokio::time::timeout(
        Duration::from_secs(5),
        Channel::from_shared(endpoint)?.connect(),
    )
    .await??;

    let slot = BridgeSlot::new_ready_for_test(channel);
    let pool = CxfBridgePool::from_config(CxfPoolConfig {
        profiles,
        listener_tls: None,
        reconnect: NetworkRetryPolicy {
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
            jitter_factor: 0.0,
            ..NetworkRetryPolicy::default()
        },
        ..CxfPoolConfig::default()
    })?;
    pool.insert_slot_for_test(CxfBridgePool::slot_key(), slot);
    Ok((Arc::new(pool), state))
}

fn profile_config(name: &str) -> CxfProfileConfig {
    CxfProfileConfig {
        name: name.to_string(),
        address: None,
        wsdl_path: "/wsdl/hello.wsdl".to_string(),
        service_name: "HelloService".to_string(),
        port_name: "HelloPort".to_string(),
        security: Default::default(),
    }
}

fn consumer_ctx(
    route_id: &str,
) -> (
    ConsumerContext,
    mpsc::Receiver<ExchangeEnvelope>,
    CancellationToken,
) {
    let (tx, rx) = mpsc::channel(16);
    let token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, token.clone(), route_id.to_string());
    (ctx, rx, token)
}

#[tokio::test]
async fn consumer_sends_profile_metadata_on_open() -> Result<(), BoxErr> {
    let (pool, state) = mock_pool(vec![]).await?;
    let mut consumer = CxfConsumer::new(pool, "v11".into(), Arc::new(NoopRuntimeObservability));
    let (ctx, _rx, _token) = consumer_ctx("route-v11");

    consumer.start(ctx).await?;

    let seen = wait_for_open_count(&state, 1, Duration::from_secs(5)).await?;
    assert_eq!(seen, vec![Some("v11".to_string())]);

    let stopped = tokio::time::timeout(Duration::from_secs(10), consumer.stop()).await;
    assert!(matches!(stopped, Ok(Ok(()))), "stop failed: {stopped:?}");
    Ok(())
}

// --- Tracing capture helper (convention: docs/testing/tracing-capture-guards.md) ---

#[derive(Clone)]
struct CapturingWriter {
    sink: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl std::io::Write for CapturingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sink.lock().unwrap().extend_from_slice(buf); // allow-unwrap: test-only
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
/// test binary, so callsite interest is never cached as `never` by
/// subscriber-less sibling tests.
fn ensure_global_tracing_default() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INIT.set(()).is_ok() {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    }
}

fn capture_sink() -> (Arc<std::sync::Mutex<Vec<u8>>>, impl tracing::Subscriber) {
    ensure_global_tracing_default();
    let sink: Arc<std::sync::Mutex<Vec<u8>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = CapturingWriter {
        sink: Arc::clone(&sink),
    };
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    (sink, subscriber)
}

#[tokio::test]
async fn invalid_argument_on_open_is_terminal() -> Result<(), BoxErr> {
    let (pool, state) = mock_pool(vec![]).await?;
    acquire_deadline(
        &state.open_stream_rejections,
        "open_stream_rejections",
        Duration::from_secs(5),
    )
    .await
    .push_back(tonic::Code::InvalidArgument);

    ensure_global_tracing_default();
    let (sink, subscriber) = capture_sink();
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut consumer = CxfConsumer::new(pool, "v11".into(), Arc::new(NoopRuntimeObservability));
    let (ctx, _rx, _token) = consumer_ctx("route-v11");
    consumer.start(ctx).await?;
    let handle = consumer
        .background_task_handle()
        .ok_or("consumer has no background task handle")?;

    let outcome = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(
        matches!(outcome, Ok(Ok(Ok(())))),
        "terminal path must return Ok(()): {outcome:?}"
    );

    let seen = acquire_deadline(
        &state.open_stream_profiles,
        "open_stream_profiles",
        Duration::from_secs(5),
    )
    .await
    .clone();
    assert_eq!(seen, vec![Some("v11".to_string())]);

    let bytes = sink.lock().unwrap().clone(); // allow-unwrap: test-only
    let out = String::from_utf8(bytes)?;
    let error_lines: Vec<&str> = out.lines().filter(|l| l.contains(" ERROR ")).collect();
    assert_eq!(
        error_lines.len(),
        1,
        "expected exactly one ERROR line: {out}"
    );
    assert!(error_lines[0].contains("v11"), "{}", error_lines[0]);
    assert!(
        error_lines[0].contains("INVALID_ARGUMENT"),
        "{}",
        error_lines[0]
    );
    Ok(())
}

#[tokio::test]
async fn transport_unavailable_on_open_still_retries() -> Result<(), BoxErr> {
    let (pool, state) = mock_pool(vec![]).await?;
    acquire_deadline(
        &state.open_stream_rejections,
        "open_stream_rejections",
        Duration::from_secs(5),
    )
    .await
    .push_back(tonic::Code::Unavailable);

    let mut consumer = CxfConsumer::new(pool, "v11".into(), Arc::new(NoopRuntimeObservability));
    let (ctx, _rx, _token) = consumer_ctx("route-v11");
    consumer.start(ctx).await?;

    let seen = wait_for_open_count(&state, 2, Duration::from_secs(5)).await?;
    assert_eq!(seen, vec![Some("v11".to_string()), Some("v11".to_string())]);

    let stopped = tokio::time::timeout(Duration::from_secs(10), consumer.stop()).await;
    assert!(matches!(stopped, Ok(Ok(()))), "stop failed: {stopped:?}");
    Ok(())
}

#[tokio::test]
async fn rejected_profile_does_not_crash_or_restart_route() -> Result<(), BoxErr> {
    let (pool, state) = mock_pool(vec![profile_config("v11")]).await?;
    acquire_deadline(
        &state.open_stream_rejections,
        "open_stream_rejections",
        Duration::from_secs(5),
    )
    .await
    .push_back(tonic::Code::InvalidArgument);

    let mut ctx = CamelContext::builder()
        .supervision(SupervisionConfig {
            max_attempts: Some(3),
            initial_delay: Duration::from_millis(10),
            backoff_multiplier: 1.0,
            max_delay: Duration::from_millis(100),
        })
        .build()
        .await?;
    ctx.register_component(CxfComponent::new(Arc::clone(&pool)));
    ctx.add_route_definition(
        RouteDefinition::new(
            "cxf://http://127.0.0.1:1/cxf/v11?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort&profile=v11",
            vec![],
        )
        .with_route_id("cxf-rejected-r1"),
    )
    .await?;

    ctx.start().await?;

    wait_for_open_count(&state, 1, Duration::from_secs(5)).await?;
    // A restart would open again (accepted, recorded) within 75 supervision delays.
    let second = wait_for_open_count(&state, 2, Duration::from_millis(750)).await;
    assert!(
        second.is_err(),
        "route restarted after rejection: {second:?}"
    );

    let status = match ctx
        .runtime()
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "cxf-rejected-r1".into(),
        })
        .await?
    {
        RuntimeQueryResult::RouteStatus { status, .. } => status,
        other => return Err(format!("unexpected query result: {other:?}").into()),
    };
    assert_ne!(status, "Failed", "route must not be marked failed");

    let stopped = tokio::time::timeout(Duration::from_secs(10), ctx.stop()).await;
    assert!(matches!(stopped, Ok(Ok(()))), "stop failed: {stopped:?}");
    Ok(())
}

fn v11_consumer(pool: &Arc<CxfBridgePool>) -> CxfConsumer {
    CxfConsumer::new(
        Arc::clone(pool),
        "v11".into(),
        Arc::new(NoopRuntimeObservability),
    )
}

async fn stop_bounded(consumer: &mut CxfConsumer) -> Result<(), BoxErr> {
    let stopped = tokio::time::timeout(Duration::from_secs(10), consumer.stop()).await;
    assert!(matches!(stopped, Ok(Ok(()))), "stop failed: {stopped:?}");
    Ok(())
}

#[tokio::test]
async fn stopped_consumer_profile_can_start_again() -> Result<(), BoxErr> {
    let (pool, _state) = mock_pool(vec![]).await?;
    let mut a = v11_consumer(&pool);
    let (ctx_a, _rx_a, _t_a) = consumer_ctx("route-a");
    a.start(ctx_a).await?;
    stop_bounded(&mut a).await?;

    let mut b = v11_consumer(&pool);
    let (ctx_b, _rx_b, _t_b) = consumer_ctx("route-b");
    b.start(ctx_b).await?;
    stop_bounded(&mut b).await?;
    Ok(())
}

#[tokio::test]
async fn aborted_task_and_dropped_consumer_release_profile() -> Result<(), BoxErr> {
    let (pool, _state) = mock_pool(vec![]).await?;
    let mut a = v11_consumer(&pool);
    let (ctx_a, _rx_a, _t_a) = consumer_ctx("route-a");
    a.start(ctx_a).await?;
    let h = a
        .background_task_handle()
        .ok_or("consumer has no background task handle")?;
    h.abort();
    let joined = tokio::time::timeout(Duration::from_secs(5), h).await;
    assert!(
        matches!(&joined, Ok(Err(e)) if e.is_cancelled()),
        "expected cancelled JoinError: {joined:?}"
    );
    drop(a);

    let mut b = v11_consumer(&pool);
    let (ctx_b, _rx_b, _t_b) = consumer_ctx("route-b");
    b.start(ctx_b).await?;
    stop_bounded(&mut b).await?;
    Ok(())
}

#[tokio::test]
async fn immediate_restart_after_abort_and_stop_keeps_new_registration() -> Result<(), BoxErr> {
    let (pool, _state) = mock_pool(vec![]).await?;
    let mut a = v11_consumer(&pool);
    let (ctx_a, _rx_a, _t_a) = consumer_ctx("route-a");
    a.start(ctx_a).await?;
    let h = a
        .background_task_handle()
        .ok_or("consumer has no background task handle")?;
    h.abort();
    stop_bounded(&mut a).await?;

    let mut b = v11_consumer(&pool);
    let (ctx_b, _rx_b, _t_b) = consumer_ctx("route-b");
    b.start(ctx_b).await?;

    // Late exit of the old task and drop of the old consumer.
    let _ = tokio::time::timeout(Duration::from_secs(5), h).await;
    drop(a);

    assert!(pool.consumer_profile_active_for_test("v11"));
    let mut c = v11_consumer(&pool);
    let (ctx_c, _rx_c, _t_c) = consumer_ctx("route-c");
    match c.start(ctx_c).await {
        Err(camel_api::CamelError::Config(m)) => assert!(m.contains("'v11'"), "{m}"),
        other => return Err(format!("expected Config error, got {other:?}").into()),
    }
    stop_bounded(&mut b).await?;
    Ok(())
}

/// Local copy of `consumer_emission_test.rs::wait_for_consumer_request_sender`,
/// bounded by a 5 s deadline and yielding instead of sleeping.
async fn wait_for_consumer_request_sender(
    state: &MockState,
) -> Result<mpsc::Sender<Result<ConsumerRequest, tonic::Status>>, BoxErr> {
    let found = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(tx) = acquire_deadline(
                &state.consumer_requests_tx,
                "consumer_requests_tx",
                Duration::from_secs(5),
            )
            .await
            .clone()
            {
                return tx;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    found.map_err(|_| "consumer request sender not available".into())
}

async fn reply_ok(mut rx: mpsc::Receiver<ExchangeEnvelope>) {
    let envelope = match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
        Ok(Some(envelope)) => envelope,
        Ok(None) | Err(_) => return,
    };
    let mut exchange = envelope.exchange;
    exchange.output = Some(Message::new(Body::Text("<ok/>".into())));
    if let Some(reply_tx) = envelope.reply_tx {
        let _ = reply_tx.send(Ok(exchange));
    }
}

#[tokio::test]
async fn duplicate_profile_fails_at_start_and_first_keeps_receiving() -> Result<(), BoxErr> {
    let (pool, state) = mock_pool(vec![]).await?;
    let mut a = v11_consumer(&pool);
    let (ctx_a, rx_a, _t_a) = consumer_ctx("route-a");
    a.start(ctx_a).await?;
    wait_for_open_count(&state, 1, Duration::from_secs(5)).await?;

    let mut b = v11_consumer(&pool);
    let (ctx_b, _rx_b, _t_b) = consumer_ctx("route-b");
    match b.start(ctx_b).await {
        Err(camel_api::CamelError::Config(m)) => assert!(m.contains("'v11'"), "{m}"),
        other => return Err(format!("expected Config error, got {other:?}").into()),
    }

    let handler = tokio::spawn(reply_ok(rx_a));
    let requests_tx = wait_for_consumer_request_sender(&state).await?;
    let completion = state.expect_response_completion("dup-1");
    requests_tx
        .send(Ok(ConsumerRequest {
            request_id: "dup-1".into(),
            operation: String::new(),
            payload: b"<hello/>".to_vec(),
            headers: Default::default(),
            soap_action: String::new(),
            security_profile: "v11".into(),
            signer: None,
        }))
        .await?;
    tokio::time::timeout(Duration::from_secs(5), completion)
        .await
        .map_err(|_| "response completion not reached within timeout")?
        .map_err(|_| "response completion signal dropped")?;

    let recorded = acquire_deadline(
        &state.consumer_responses_received,
        "consumer_responses_received",
        Duration::from_secs(5),
    )
    .await
    .clone();
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0].request_id, "dup-1");
    assert!(!recorded[0].fault, "{:?}", recorded[0]);

    let opens = acquire_deadline(
        &state.open_stream_profiles,
        "open_stream_profiles",
        Duration::from_secs(5),
    )
    .await
    .len();
    assert_eq!(opens, 1, "B must never open a stream");

    let () = tokio::time::timeout(Duration::from_secs(5), handler).await??;
    stop_bounded(&mut a).await?;
    Ok(())
}
