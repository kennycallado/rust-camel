mod support;

use std::task::{Context, Poll};
use std::time::Duration;

use camel_component_api::{Body, Exchange, Message, Value};
use camel_component_cxf::producer::CxfProducer;
use support::mock_bridge::{MockState, ScriptedInvokeOutcome, spawn_mock_bridge};
use tonic::transport::{Channel, Endpoint};
use tower::Service;

/// Bound for reading mock-bridge observed state. The dispatch has already
/// completed, so the uncontended lock resolves at once; the bound turns a
/// stalled dispatch into a fast failure instead of a hung suite.
const STATE_LOCK_BUDGET: Duration = Duration::from_secs(5);

/// Helper: poll_ready + call in one step (required by Tower contract).
async fn ready_call(
    producer: &mut CxfProducer,
    exchange: Exchange,
) -> Result<Exchange, camel_component_api::CamelError> {
    {
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        match producer.poll_ready(&mut cx) {
            Poll::Ready(Ok(())) => {}
            other => panic!("poll_ready unexpected: {other:?}"),
        }
    }
    producer.call(exchange).await
}

async fn make_producer() -> (CxfProducer, MockState) {
    make_producer_with_default_timeout(None).await
}

async fn make_producer_with_default_timeout(
    default_timeout_ms: Option<u64>,
) -> (CxfProducer, MockState) {
    let (port, state) = spawn_mock_bridge().await.expect("mock bridge");
    let channel = tokio::time::timeout(
        Duration::from_secs(5),
        Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .expect("endpoint")
            .connect(),
    )
    .await
    .expect("connect timed out after 5s")
    .expect("connect");

    let rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability> =
        std::sync::Arc::new(camel_component_api::NoOpComponentContext);
    let producer = CxfProducer::from_channel(
        channel,
        "test".to_string(),
        "service.wsdl".to_string(),
        "TestService".to_string(),
        "TestPort".to_string(),
        Some("http://localhost:8080/ws".to_string()),
        "defaultOperation".to_string(),
        default_timeout_ms,
        rt,
    );
    (producer, state)
}

#[tokio::test]
async fn test_invoke_success_sets_response_body() {
    let (mut producer, state) = make_producer().await;
    *state.invoke_response_payload.lock().await = Some(b"ok-response".to_vec());
    let exchange = Exchange::new(Message::new(Body::Text("request".to_string())));

    let out = ready_call(&mut producer, exchange).await.expect("success");
    assert_eq!(
        out.input.body,
        Body::Bytes(bytes::Bytes::from_static(b"ok-response"))
    );
}

#[tokio::test]
async fn test_invoke_fault_returns_error() {
    let (mut producer, state) = make_producer().await;
    *state.invoke_fault.lock().await = Some(("Server".to_string(), "Boom".to_string()));
    let exchange = Exchange::new(Message::new(Body::Text("request".to_string())));

    let err = ready_call(&mut producer, exchange)
        .await
        .expect_err("fault error");
    let msg = err.to_string();
    assert!(msg.contains("fault"));
    assert!(msg.contains("Server"));
    assert!(msg.contains("Boom"));
}

#[tokio::test]
async fn test_invoke_timeout_returns_transport_error() {
    let channel: Channel = Endpoint::from_static("http://127.0.0.1:9")
        .connect_timeout(Duration::from_millis(100))
        .timeout(Duration::from_millis(100))
        .connect_lazy();
    let rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability> =
        std::sync::Arc::new(camel_component_api::NoOpComponentContext);
    let mut producer = CxfProducer::from_channel(
        channel,
        "test".to_string(),
        "service.wsdl".to_string(),
        "TestService".to_string(),
        "TestPort".to_string(),
        Some("http://localhost:8080/ws".to_string()),
        "defaultOperation".to_string(),
        None,
        rt,
    );

    let exchange = Exchange::new(Message::new(Body::Text("request".to_string())));
    let err = ready_call(&mut producer, exchange)
        .await
        .expect_err("transport error");
    assert!(err.to_string().contains("CXF gRPC invoke"));
}

#[tokio::test]
async fn test_operation_from_header_overrides_default() {
    let (mut producer, state) = make_producer().await;
    let mut msg = Message::new(Body::Text("request".to_string()));
    msg.set_header(
        "CamelCxfOperation",
        Value::String("headerOperation".to_string()),
    );
    let exchange = Exchange::new(msg);

    let _ = ready_call(&mut producer, exchange).await.expect("success");
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.operation, "headerOperation");
}

#[tokio::test]
async fn test_operation_from_uri_when_no_header() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Text("request".to_string())));

    let _ = ready_call(&mut producer, exchange).await.expect("success");
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.operation, "defaultOperation");
}

#[tokio::test]
async fn test_body_conversion_text_to_bytes() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Text("hello".to_string())));

    let _ = ready_call(&mut producer, exchange).await.expect("success");
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.payload, b"hello");
}

#[tokio::test]
async fn test_body_conversion_xml_to_bytes() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Xml("<x/>".to_string())));

    let _ = ready_call(&mut producer, exchange).await.expect("success");
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.payload, b"<x/>");
}

#[tokio::test]
async fn test_body_conversion_bytes_passthrough() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Bytes(bytes::Bytes::from_static(b"raw"))));

    let _ = ready_call(&mut producer, exchange).await.expect("success");
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.payload, b"raw");
}

#[tokio::test]
async fn test_body_empty_sends_zero_length_payload() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Empty));

    let _ = ready_call(&mut producer, exchange).await.expect("success");
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert!(req.payload.is_empty());
}

#[tokio::test]
async fn test_invoke_bridge_returns_soap_fault() {
    let (mut producer, state) = make_producer().await;
    *state.invoke_fault.lock().await =
        Some(("soap:Client".to_string(), "Invalid request".to_string()));
    let exchange = Exchange::new(Message::new(Body::Text("<bad/>".to_string())));
    let err = ready_call(&mut producer, exchange)
        .await
        .expect_err("fault");
    let msg = err.to_string();
    assert!(msg.contains("fault"));
    assert!(msg.contains("soap:Client"));
    assert!(msg.contains("Invalid request"));
}

#[tokio::test]
async fn test_invoke_sends_address_in_request() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Text("req".to_string())));
    let _ = ready_call(&mut producer, exchange).await;
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert!(req.address.contains("localhost:8080/ws"));
}

#[tokio::test]
async fn test_invoke_with_custom_timeout_forwarded() {
    let (mut producer, state) = make_producer().await;
    let mut msg = Message::new(Body::Text("req".to_string()));
    msg.set_header("CamelCxfTimeoutMs", Value::String("5000".to_string()));
    let exchange = Exchange::new(msg);
    let _ = ready_call(&mut producer, exchange).await;
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.timeout_ms, 5000);
}

#[tokio::test]
async fn test_multiple_sequential_invocations() {
    let (mut producer, state) = make_producer().await;
    *state.invoke_response_payload.lock().await = Some(b"response".to_vec());
    for i in 0..5 {
        let exchange = Exchange::new(Message::new(Body::Text(format!("request-{i}"))));
        let out = ready_call(&mut producer, exchange).await.expect("success");
        assert_eq!(
            out.input.body,
            Body::Bytes(bytes::Bytes::from_static(b"response"))
        );
    }
}

#[tokio::test]
async fn test_invoke_json_body_converted_to_bytes() {
    let (mut producer, state) = make_producer().await;
    let json_val = serde_json::json!({"key": "value"});
    let exchange = Exchange::new(Message::new(Body::Json(json_val)));
    let _ = ready_call(&mut producer, exchange).await;
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    let parsed: serde_json::Value = serde_json::from_slice(&req.payload).unwrap();
    assert_eq!(parsed["key"], "value");
}

#[tokio::test]
async fn test_invoke_sends_security_profile() {
    let (mut producer, state) = make_producer().await;
    let exchange = Exchange::new(Message::new(Body::Text("req".to_string())));
    let _ = ready_call(&mut producer, exchange).await;
    let req = state.last_invoke.lock().await.clone().expect("last invoke");
    assert_eq!(req.security_profile, "test");
}

#[tokio::test]
async fn timeout_ms_zero_uses_default() {
    // 0 is a live wire value: the bridge substitutes its own default. The
    // producer must forward 0 rather than resolving the configured default
    // locally.
    let (mut producer, state) = make_producer_with_default_timeout(Some(5000)).await;
    let mut msg = Message::new(Body::Text("req".to_string()));
    msg.set_header("CamelCxfTimeoutMs", Value::String("0".to_string()));
    let exchange = Exchange::new(msg);

    let _ = ready_call(&mut producer, exchange).await.expect("success");
    let req = tokio::time::timeout(STATE_LOCK_BUDGET, state.last_invoke.lock())
        .await
        .expect("last_invoke lock must not hang")
        .clone()
        .expect("last invoke");
    assert_eq!(req.timeout_ms, 0, "0 must reach the wire unchanged");
}

#[tokio::test]
async fn timeout_ms_over_i32_max_is_rejected() {
    let (mut producer, state) = make_producer().await;
    let mut msg = Message::new(Body::Text("req".to_string()));
    msg.set_header("CamelCxfTimeoutMs", Value::String("2147483648".to_string()));
    let exchange = Exchange::new(msg);

    let err = ready_call(&mut producer, exchange)
        .await
        .expect_err("timeout above i32 max must be rejected");
    let text = err.to_string();
    assert!(
        text.contains("cxf timeout_ms exceeds 2147483647"),
        "got: {text}"
    );
    assert!(
        text.contains("2147483647"),
        "error must name the bound: {text}"
    );
    assert!(
        state.last_invoke.lock().await.is_none(),
        "no dispatch may occur for an out-of-range timeout"
    );
}

#[tokio::test]
async fn legacy_negative_timeout_is_rejected() {
    // A legacy `-1` on the wire decodes to u64::MAX.
    let (mut producer, state) = make_producer().await;
    let mut msg = Message::new(Body::Text("req".to_string()));
    msg.set_header("CamelCxfTimeoutMs", Value::String(u64::MAX.to_string()));
    let exchange = Exchange::new(msg);

    let err = ready_call(&mut producer, exchange)
        .await
        .expect_err("legacy negative timeout must be rejected");
    assert!(
        err.to_string()
            .contains("cxf timeout_ms exceeds 2147483647"),
        "got: {err}"
    );
    assert!(
        state.last_invoke.lock().await.is_none(),
        "no dispatch may occur for a legacy negative timeout"
    );
}

#[tokio::test]
async fn timeout_ms_in_range_is_applied() {
    let (mut producer, state) = make_producer().await;
    let mut msg = Message::new(Body::Text("req".to_string()));
    msg.set_header("CamelCxfTimeoutMs", Value::String("5000".to_string()));
    let exchange = Exchange::new(msg);

    let _ = ready_call(&mut producer, exchange).await.expect("success");
    let req = tokio::time::timeout(STATE_LOCK_BUDGET, state.last_invoke.lock())
        .await
        .expect("last_invoke lock must not hang")
        .clone()
        .expect("last invoke");
    assert_eq!(req.timeout_ms, 5000);
}

/// Queues a scripted transport error followed by a success that must stay
/// unconsumed by the failing call.
async fn script_transport_error_then_success(
    state: &MockState,
    status: tonic::Status,
    payload: &[u8],
) {
    let mut queue = state.scripted_outcomes.lock().await; // allow-test-wait: mock state mutex — uncontended in-test
    queue.push_back(ScriptedInvokeOutcome::Error(status));
    queue.push_back(ScriptedInvokeOutcome::Success(payload.to_vec()));
}

#[tokio::test]
async fn cxf_empty_success_preserves_empty_bytes() {
    let (mut producer, state) = make_producer().await;
    // The bridge returns a successful, empty SoapResponse.
    *state.invoke_response_payload.lock().await = Some(Vec::new()); // allow-test-wait: mock state mutex — uncontended in-test
    let exchange = Exchange::new(Message::new(Body::Text("nonempty request".to_string())));

    let out = ready_call(&mut producer, exchange)
        .await
        .expect("empty success must complete");

    match out.input.body {
        Body::Bytes(b) => assert!(
            b.is_empty(),
            "successful empty reply must map to zero-length bytes, not the input: {b:?}"
        ),
        other => panic!("expected Body::Bytes, got {other:?}"),
    }
    assert_eq!(
        *state.invocation_count.lock().await,
        1,
        "exactly one invocation"
    );
}

#[tokio::test]
async fn cxf_transport_failure_never_replays_invocation() {
    for status in [
        tonic::Status::unavailable("connection dropped"),
        tonic::Status::internal("transport channel closed"),
    ] {
        let code = status.code();
        let (mut producer, state) = make_producer().await;
        script_transport_error_then_success(&state, status, b"unused-success").await;

        let exchange = Exchange::new(Message::new(Body::Text("request".to_string())));
        let err = ready_call(&mut producer, exchange)
            .await
            .expect_err("a transport failure must stay an error, never a replay");

        assert!(
            err.to_string().contains("CXF gRPC invoke"),
            "{code:?}: unexpected error: {err}"
        );
        assert_eq!(
            *state.invocation_count.lock().await,
            1,
            "{code:?}: a failed send must not be replayed"
        );
        assert_eq!(
            state.scripted_outcomes.lock().await.len(),
            1,
            "{code:?}: the queued success must stay unconsumed"
        );
    }
}

#[tokio::test]
async fn cxf_later_explicit_send_recovers_after_transport_failure() {
    let (mut producer, state) = make_producer().await;
    script_transport_error_then_success(
        &state,
        tonic::Status::unavailable("connection dropped"),
        b"second-response",
    )
    .await;

    // First explicit send fails without replaying.
    let first = Exchange::new(Message::new(Body::Text("first".to_string())));
    let err = ready_call(&mut producer, first)
        .await
        .expect_err("the first send must fail");
    assert!(
        err.to_string().contains("CXF gRPC invoke"),
        "unexpected error: {err}"
    );

    // A later explicit send acquires a fresh client and succeeds.
    let second = Exchange::new(Message::new(Body::Text("second".to_string())));
    let out = ready_call(&mut producer, second)
        .await
        .expect("the second explicit send must succeed");
    assert_eq!(
        out.input.body,
        Body::Bytes(bytes::Bytes::from_static(b"second-response"))
    );
    assert_eq!(
        *state.invocation_count.lock().await,
        2,
        "exactly two invocations across both explicit sends"
    );
    let last = state.last_invoke.lock().await.clone().expect("last invoke"); // allow-test-wait: mock state mutex — uncontended in-test
    assert_eq!(
        last.payload, b"second",
        "the second invocation must carry the second call's payload"
    );
}
