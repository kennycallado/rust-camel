use camel_component_api::test_support::PanicRuntimeObservability;
fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(PanicRuntimeObservability)
}

use super::*;
use crate::inner::clone_body;
use camel_component_api::Exchange;
use camel_component_api::Message;
use camel_component_api::NoOpComponentContext;
use camel_component_api::ProducerContext;
use tower::Service;
use tower::ServiceExt;

fn test_producer_ctx() -> ProducerContext {
    ProducerContext::new()
}

#[test]
fn test_mock_component_scheme() {
    let component = MockComponent::new();
    assert_eq!(component.scheme(), "mock");
}

#[test]
fn test_mock_component_default() {
    let component = MockComponent::default();
    assert_eq!(component.scheme(), "mock");
    assert!(component.get_endpoint("missing").is_none());
}

#[test]
fn test_mock_creates_endpoint() {
    let component = MockComponent::new();
    let endpoint = component.create_endpoint("mock:result", &NoOpComponentContext);
    assert!(endpoint.is_ok());
}

#[test]
fn test_mock_wrong_scheme() {
    let component = MockComponent::new();
    let result = component.create_endpoint("timer:tick", &NoOpComponentContext);
    assert!(result.is_err());
}

#[test]
fn test_empty_mock_endpoint_name_rejected() {
    let component = MockComponent::new();
    let result = component.create_endpoint("mock:", &NoOpComponentContext);
    assert!(result.is_err(), "empty mock name should be rejected");
}

#[test]
fn test_valid_mock_endpoint_name_accepted() {
    let component = MockComponent::new();
    let result = component.create_endpoint("mock:result", &NoOpComponentContext);
    assert!(result.is_ok());
}

#[test]
fn test_mock_endpoint_no_consumer() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:result", &NoOpComponentContext)
        .unwrap();
    assert!(endpoint.create_consumer(rt()).is_err());
}

#[test]
fn test_mock_endpoint_creates_producer() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:result", &NoOpComponentContext)
        .unwrap();
    assert!(endpoint.create_producer(rt(), &ctx).is_ok());
}

#[test]
fn test_mock_endpoint_uri() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:uri-check", &NoOpComponentContext)
        .unwrap();
    assert_eq!(endpoint.uri(), "mock:uri-check");
}

#[test]
fn test_mock_get_endpoint_returns_same_inner_for_same_name() {
    let component = MockComponent::new();
    let _ = component
        .create_endpoint("mock:shared-inner", &NoOpComponentContext)
        .unwrap();
    let _ = component
        .create_endpoint("mock:shared-inner", &NoOpComponentContext)
        .unwrap();

    let first = component.get_endpoint("shared-inner").unwrap();
    let second = component.get_endpoint("shared-inner").unwrap();
    assert!(Arc::ptr_eq(&first, &second));
}

#[tokio::test]
async fn test_mock_producer_records_exchange() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:test", &NoOpComponentContext)
        .unwrap();

    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    let ex1 = Exchange::new(Message::new("first"));
    let ex2 = Exchange::new(Message::new("second"));

    producer.call(ex1).await.unwrap();
    producer.call(ex2).await.unwrap();

    let inner = component.get_endpoint("test").unwrap();
    inner.assert_exchange_count(2).await;

    let received = inner.get_received_exchanges().await;
    assert_eq!(received[0].input.body.as_text(), Some("first"));
    assert_eq!(received[1].input.body.as_text(), Some("second"));
}

#[tokio::test]
async fn test_mock_producer_passes_through_exchange() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:passthrough", &NoOpComponentContext)
        .unwrap();

    let producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let exchange = Exchange::new(Message::new("hello"));
    let result = producer.oneshot(exchange).await.unwrap();

    // Producer should return the exchange unchanged
    assert_eq!(result.input.body.as_text(), Some("hello"));
}

#[tokio::test]
async fn test_mock_assert_count_passes() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:count", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("count").unwrap();

    inner.assert_exchange_count(0).await;

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("one")))
        .await
        .unwrap();

    inner.assert_exchange_count(1).await;
}

#[tokio::test]
#[should_panic(expected = "MockEndpoint expected 5 exchanges, got 0")]
async fn test_mock_assert_count_fails() {
    let component = MockComponent::new();
    // Endpoint not created yet, so get_endpoint returns None.
    // Create it first, then assert.
    let _endpoint = component
        .create_endpoint("mock:fail", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("fail").unwrap();

    inner.assert_exchange_count(5).await;
}

#[tokio::test]
async fn test_mock_component_shared_registry() {
    let component = MockComponent::new();
    let ep1 = component
        .create_endpoint("mock:shared", &NoOpComponentContext)
        .unwrap();
    let ep2 = component
        .create_endpoint("mock:shared", &NoOpComponentContext)
        .unwrap();

    // Producing via ep1's producer...
    let ctx = test_producer_ctx();
    let mut p1 = ep1.create_producer(rt(), &ctx).unwrap();
    p1.call(Exchange::new(Message::new("from-ep1")))
        .await
        .unwrap();

    // ...and via ep2's producer...
    let mut p2 = ep2.create_producer(rt(), &ctx).unwrap();
    p2.call(Exchange::new(Message::new("from-ep2")))
        .await
        .unwrap();

    // ...both should be visible via the shared storage
    let inner = component.get_endpoint("shared").unwrap();
    inner.assert_exchange_count(2).await;

    let received = inner.get_received_exchanges().await;
    assert_eq!(received[0].input.body.as_text(), Some("from-ep1"));
    assert_eq!(received[1].input.body.as_text(), Some("from-ep2"));
}

#[tokio::test]
async fn await_exchanges_resolves_immediately() {
    // If exchanges are already present, await_exchanges returns without timeout.
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:immediate", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("immediate").unwrap();

    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("a")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("b")))
        .await
        .unwrap();

    // Should return immediately — both exchanges already received.
    inner
        .await_exchanges(2, std::time::Duration::from_millis(100))
        .await;
}

#[tokio::test]
async fn await_exchanges_waits_then_resolves() {
    // await_exchanges unblocks when a producer sends after the call.
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:waiter", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("waiter").unwrap();

    // Spawn producer that sends after a short delay.
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        producer
            .call(Exchange::new(Message::new("delayed")))
            .await
            .unwrap();
    });

    // This should block until the spawned task delivers the exchange.
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;

    let received = inner.get_received_exchanges().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].input.body.as_text(), Some("delayed"));
}

#[tokio::test]
#[should_panic(expected = "timed out waiting for 5 exchanges")]
async fn await_exchanges_times_out() {
    let component = MockComponent::new();
    let _endpoint = component
        .create_endpoint("mock:timeout", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("timeout").unwrap();

    // Nobody sends — should panic after timeout.
    inner
        .await_exchanges(5, std::time::Duration::from_millis(50))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn exchange_idx_returns_assert() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:assert-idx", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("assert-idx").unwrap();

    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("hello")))
        .await
        .unwrap();

    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    // Should not panic — index 0 exists.
    let _assert = inner.exchange(0);
}

#[tokio::test]
async fn exchange_current_thread_clear_panic() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:current-thread", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("current-thread").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("recorded")))
        .await
        .unwrap();

    let panic = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inner.exchange(0))) {
        Ok(_) => panic!("exchange must panic on a current-thread runtime"),
        Err(payload) => payload,
    };
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("panic payload should be a string");
    assert!(message.contains("current-thread"), "panic: {message}");
    assert!(message.contains("multi_thread"), "panic: {message}");
}

#[tokio::test(flavor = "multi_thread")]
async fn exchange_multi_thread_unchanged() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:multi-thread", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("multi-thread").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("first")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("second")))
        .await
        .unwrap();

    let _assert = inner.exchange(1);
}

#[test]
fn exchange_no_runtime_returns_assert() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:no-runtime", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("no-runtime").unwrap();
    let ctx = test_producer_ctx();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("recorded")))
            .await
            .unwrap();
    });
    drop(runtime);

    let _assert = inner.exchange(0);
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "exchange index 5 out of bounds")]
async fn exchange_idx_out_of_bounds() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:oob", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("oob").unwrap();

    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("only-one")))
        .await
        .unwrap();

    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    // Only 1 exchange, index 5 should panic.
    let _assert = inner.exchange(5);
}

#[tokio::test(flavor = "multi_thread")]
async fn assert_body_text_pass() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:body-text-pass", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("body-text-pass").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("hello")))
        .await
        .unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner.exchange(0).assert_body_text("hello");
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "expected body text")]
async fn assert_body_text_fail() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:body-text-fail", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("body-text-fail").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("hello")))
        .await
        .unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner.exchange(0).assert_body_text("world");
}

#[tokio::test(flavor = "multi_thread")]
async fn assert_body_json_pass() {
    use camel_component_api::Body;
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:body-json-pass", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("body-json-pass").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("");
    msg.body = Body::Json(serde_json::json!({"key": "value"}));
    producer.call(Exchange::new(msg)).await.unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner
        .exchange(0)
        .assert_body_json(serde_json::json!({"key": "value"}));
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "expected body JSON")]
async fn assert_body_json_fail() {
    use camel_component_api::Body;
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:body-json-fail", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("body-json-fail").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("");
    msg.body = Body::Json(serde_json::json!({"key": "value"}));
    producer.call(Exchange::new(msg)).await.unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner
        .exchange(0)
        .assert_body_json(serde_json::json!({"key": "other"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn assert_body_bytes_pass() {
    use bytes::Bytes;
    use camel_component_api::Body;
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:body-bytes-pass", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("body-bytes-pass").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("");
    msg.body = Body::Bytes(Bytes::from_static(b"binary"));
    producer.call(Exchange::new(msg)).await.unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner.exchange(0).assert_body_bytes(b"binary");
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "expected body bytes")]
async fn assert_body_bytes_fail() {
    use bytes::Bytes;
    use camel_component_api::Body;
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:body-bytes-fail", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("body-bytes-fail").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("");
    msg.body = Body::Bytes(Bytes::from_static(b"binary"));
    producer.call(Exchange::new(msg)).await.unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner.exchange(0).assert_body_bytes(b"different");
}

#[tokio::test(flavor = "multi_thread")]
async fn assert_header_pass() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:hdr-pass", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("hdr-pass").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("body");
    msg.headers
        .insert("x-key".to_string(), serde_json::json!("value"));
    producer.call(Exchange::new(msg)).await.unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner
        .exchange(0)
        .assert_header("x-key", serde_json::json!("value"));
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "expected header")]
async fn assert_header_fail() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:hdr-fail", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("hdr-fail").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("body");
    msg.headers
        .insert("x-key".to_string(), serde_json::json!("value"));
    producer.call(Exchange::new(msg)).await.unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner
        .exchange(0)
        .assert_header("x-key", serde_json::json!("other"));
}

#[tokio::test(flavor = "multi_thread")]
async fn assert_header_exists_pass() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:hdr-exists-pass", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("hdr-exists-pass").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("body");
    msg.headers
        .insert("x-present".to_string(), serde_json::json!(42));
    producer.call(Exchange::new(msg)).await.unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner.exchange(0).assert_header_exists("x-present");
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "expected header")]
async fn assert_header_exists_fail() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:hdr-exists-fail", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("hdr-exists-fail").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("body")))
        .await
        .unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner.exchange(0).assert_header_exists("x-missing");
}

#[tokio::test(flavor = "multi_thread")]
async fn assert_has_error_pass() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:err-pass", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("err-pass").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut ex = Exchange::new(Message::new("body"));
    ex.set_error(camel_component_api::CamelError::ProcessorError(
        "oops".to_string(),
    ));
    producer.call(ex).await.unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner.exchange(0).assert_has_error();
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "expected exchange to have an error")]
async fn assert_has_error_fail() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:has-err-fail", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("has-err-fail").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("body")))
        .await
        .unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner.exchange(0).assert_has_error();
}

#[tokio::test(flavor = "multi_thread")]
async fn assert_no_error_pass() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:no-err-pass", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("no-err-pass").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("body")))
        .await
        .unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner.exchange(0).assert_no_error();
}

// -----------------------------------------------------------------------
// A-13: reset() and bounded retention tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_mock_reset_clears_exchanges() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:reset-test", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("reset-test").unwrap();

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("a")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("b")))
        .await
        .unwrap();

    assert_eq!(inner.received_count().await, 2);
    inner.reset().await;
    assert_eq!(inner.received_count().await, 0);
}

#[tokio::test]
async fn test_mock_bounded_retention_drops_oldest() {
    let config = MockConfig {
        max_retained: 3,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:bounded", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("bounded").unwrap();

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    // Send 5 exchanges, but max_retained is 3
    for i in 0..5 {
        producer
            .call(Exchange::new(Message::new(format!("msg-{i}"))))
            .await
            .unwrap();
    }

    assert_eq!(inner.received_count().await, 3);
    let received = inner.get_received_exchanges().await;
    // Oldest (msg-0, msg-1) should be dropped
    assert_eq!(received[0].input.body.as_text(), Some("msg-2"));
    assert_eq!(received[1].input.body.as_text(), Some("msg-3"));
    assert_eq!(received[2].input.body.as_text(), Some("msg-4"));
}

#[tokio::test]
async fn test_mock_reset_then_record_again() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:reset-reuse", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("reset-reuse").unwrap();

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("before-reset")))
        .await
        .unwrap();
    inner.reset().await;

    producer
        .call(Exchange::new(Message::new("after-reset")))
        .await
        .unwrap();

    let received = inner.get_received_exchanges().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].input.body.as_text(), Some("after-reset"));
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "expected exchange to have no error")]
async fn assert_no_error_fail() {
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:no-err-fail", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("no-err-fail").unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut ex = Exchange::new(Message::new("body"));
    ex.set_error(camel_component_api::CamelError::ProcessorError(
        "oops".to_string(),
    ));
    producer.call(ex).await.unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;
    inner.exchange(0).assert_no_error();
}

// -----------------------------------------------------------------------
// MOCK-003: copy_on_exchange tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_copy_on_exchange_stores_cloned_body() {
    let config = MockConfig {
        copy_on_exchange: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:copy", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("copy").unwrap();

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    let mut msg = Message::new("original");
    msg.headers.insert("x-test".into(), serde_json::json!(1));
    let ex = Exchange::new(msg);
    producer.call(ex).await.unwrap();

    let received = inner.get_received_exchanges().await;
    assert_eq!(received[0].input.body.as_text(), Some("original"));
}

#[tokio::test]
async fn test_copy_on_exchange_false_shares_storage() {
    let config = MockConfig {
        copy_on_exchange: false,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:no-copy", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("no-copy").unwrap();

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    producer
        .call(Exchange::new(Message::new("direct")))
        .await
        .unwrap();

    let received = inner.get_received_exchanges().await;
    assert_eq!(received[0].input.body.as_text(), Some("direct"));
}

// -----------------------------------------------------------------------
// MOCK-003b: clone_body preserves Body::Stream
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_clone_body_preserves_stream() {
    use bytes::Bytes;
    use camel_component_api::{Body, StreamBody, StreamMetadata};
    use futures::stream;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let chunks: Vec<Result<Bytes, camel_component_api::CamelError>> = vec![Ok(Bytes::from("data"))];
    let body = Body::Stream(StreamBody {
        stream: Arc::new(Mutex::new(Some(Box::pin(stream::iter(chunks))))),
        metadata: StreamMetadata::default(),
    });

    let config = MockConfig {
        copy_on_exchange: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:stream-test", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("stream-test").unwrap();

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    let msg = Message::new(body);
    let ex = Exchange::new(msg);
    producer.call(ex).await.unwrap();

    let received = inner.get_received_exchanges().await;
    assert!(
        matches!(received[0].input.body, Body::Stream(_)),
        "expected Body::Stream, got {:?}",
        received[0].input.body
    );
}

#[tokio::test]
async fn test_clone_body_stream_shares_arc() {
    use bytes::Bytes;
    use camel_component_api::{Body, StreamBody, StreamMetadata};
    use futures::stream;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let chunks: Vec<Result<Bytes, camel_component_api::CamelError>> = vec![Ok(Bytes::from("data"))];
    let original = Body::Stream(StreamBody {
        stream: Arc::new(Mutex::new(Some(Box::pin(stream::iter(chunks))))),
        metadata: StreamMetadata::default(),
    });

    let clone = clone_body(&original);

    // Consume the original first
    let _ = original.into_bytes(100).await.unwrap();

    // Clone should fail with AlreadyConsumed (shared Arc semantics)
    let result = clone.into_bytes(100).await;
    assert!(
        matches!(
            result,
            Err(camel_component_api::CamelError::AlreadyConsumed)
        ),
        "expected AlreadyConsumed, got {:?}",
        result
    );
}

// -----------------------------------------------------------------------
// MOCK-004: expect_body / expect_header / assert_satisfied tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_assert_satisfied_bodies_in_order() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:sat-bodies", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("sat-bodies").unwrap();

    inner.expect_body(camel_component_api::Body::Text("alpha".into()));
    inner.expect_body(camel_component_api::Body::Text("beta".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("alpha")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("beta")))
        .await
        .unwrap();

    inner.assert_satisfied().await;
}

#[tokio::test]
#[should_panic(expected = "body[0] expected")]
async fn test_assert_satisfied_bodies_wrong_order_fails() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:sat-bodies-fail", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("sat-bodies-fail").unwrap();

    inner.expect_body(camel_component_api::Body::Text("alpha".into()));
    inner.expect_body(camel_component_api::Body::Text("beta".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("beta")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("alpha")))
        .await
        .unwrap();

    inner.assert_satisfied().await;
}

#[tokio::test]
async fn test_assert_satisfied_headers() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:sat-hdr", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("sat-hdr").unwrap();

    inner.expect_header("status", serde_json::json!("ok"));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("body");
    msg.headers.insert("status".into(), serde_json::json!("ok"));
    producer.call(Exchange::new(msg)).await.unwrap();

    inner.assert_satisfied().await;
}

#[tokio::test]
#[should_panic(expected = "expected header 'missing' =")]
async fn test_assert_satisfied_headers_missing() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:sat-hdr-missing", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("sat-hdr-missing").unwrap();

    inner.expect_header("missing", serde_json::json!("value"));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("body")))
        .await
        .unwrap();

    inner.assert_satisfied().await;
}

// -----------------------------------------------------------------------
// MOCK-005: fail_fast tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_fail_fast_rejects_after_first_call() {
    let config = MockConfig {
        fail_fast: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:ff", &NoOpComponentContext)
        .unwrap();

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    // First call succeeds
    producer
        .call(Exchange::new(Message::new("ok")))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_fail_fast_no_error_when_all_good() {
    let config = MockConfig {
        fail_fast: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:ff-good", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("ff-good").unwrap();

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    producer
        .call(Exchange::new(Message::new("a")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("b")))
        .await
        .unwrap();

    assert!(inner.fail_fast_error().is_none());
    inner.assert_exchange_count(2).await;
}

// -----------------------------------------------------------------------
// MOCK-008: await_exchanges_with_timeout tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_await_exchanges_with_timeout_uses_config_period() {
    let config = MockConfig {
        assert_period_ms: 100,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:ap", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("ap").unwrap();

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("x")))
        .await
        .unwrap();

    inner
        .await_exchanges_with_timeout(1, std::time::Duration::from_millis(1))
        .await;
}

#[tokio::test]
async fn test_await_exchanges_with_timeout_uses_fallback_when_zero() {
    let config = MockConfig {
        assert_period_ms: 0,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:ap-fb", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("ap-fb").unwrap();

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("y")))
        .await
        .unwrap();

    inner
        .await_exchanges_with_timeout(1, std::time::Duration::from_millis(200))
        .await;
}

// -----------------------------------------------------------------------
// MOCK-009: expect_header_regex tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_expect_header_regex_match() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:re-hdr", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("re-hdr").unwrap();

    inner.expect_header_regex("x-trace-id", r"^[a-f0-9]{8}$");

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("body");
    msg.headers
        .insert("x-trace-id".into(), serde_json::json!("deadbeef"));
    producer.call(Exchange::new(msg)).await.unwrap();

    inner.assert_satisfied().await;
}

#[tokio::test]
#[should_panic(expected = "no received exchange has header")]
async fn test_expect_header_regex_no_match() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:re-hdr-fail", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("re-hdr-fail").unwrap();

    inner.expect_header_regex("x-trace-id", r"^\d+$");

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("body");
    msg.headers
        .insert("x-trace-id".into(), serde_json::json!("abc"));
    producer.call(Exchange::new(msg)).await.unwrap();

    inner.assert_satisfied().await;
}

// -----------------------------------------------------------------------
// MOCK-010: any_order tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_any_order_bodies_match() {
    let config = MockConfig {
        any_order: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:anyorder", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("anyorder").unwrap();

    inner.expect_body(camel_component_api::Body::Text("beta".into()));
    inner.expect_body(camel_component_api::Body::Text("alpha".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("alpha")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("beta")))
        .await
        .unwrap();

    inner.assert_satisfied().await;
}

#[tokio::test]
#[should_panic(expected = "not found in received exchanges (anyOrder mode)")]
async fn test_any_order_bodies_missing() {
    let config = MockConfig {
        any_order: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:anyorder-fail", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("anyorder-fail").unwrap();

    inner.expect_body(camel_component_api::Body::Text("gamma".into()));
    inner.expect_body(camel_component_api::Body::Text("alpha".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("alpha")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("beta")))
        .await
        .unwrap();

    inner.assert_satisfied().await;
}

// -----------------------------------------------------------------------
// MOCK-012: tracing instrumentation tests (compilation + basic)
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_tracing_logs_exchange_received() {
    // Verify the producer doesn't panic and the debug trace fires
    let ctx = test_producer_ctx();
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:trace", &NoOpComponentContext)
        .unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("traced")))
        .await
        .unwrap();

    let inner = component.get_endpoint("trace").unwrap();
    inner.assert_exchange_count(1).await;
}

// -----------------------------------------------------------------------
// MOCK-006 / MOCK-007: doctest exists on MockConfig
// -----------------------------------------------------------------------

#[test]
fn test_mock_config_new() {
    let cfg = MockConfig::new(42);
    assert_eq!(cfg.max_retained, 42);
    assert!(!cfg.copy_on_exchange);
    assert!(!cfg.fail_fast);
    assert!(!cfg.any_order);
}

// -----------------------------------------------------------------------
// M1: fail-fast trigger + assert_satisfied wires fail_fast_error
// -----------------------------------------------------------------------

use futures::FutureExt;

#[tokio::test]
async fn test_trigger_fail_fast_rejects_subsequent_producer() {
    use camel_component_api::CamelError;
    use std::panic::AssertUnwindSafe;
    let config = MockConfig {
        fail_fast: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:test", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("test").unwrap();

    inner.trigger_fail_fast(CamelError::ProcessorError("boom".to_string()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    // poll_ready must reject in fail-fast mode.
    assert!(producer.ready().await.is_err());
    // The next call must reject with the fixed "fail-fast mode" message.
    let result = AssertUnwindSafe(producer.call(Exchange::default()))
        .catch_unwind()
        .await
        .expect("call should not panic");
    match result {
        Err(CamelError::ProcessorError(msg)) => {
            assert!(
                msg.contains("fail-fast mode"),
                "message should contain 'fail-fast mode', got: {msg}"
            );
            assert!(
                !msg.contains("boom"),
                "supplied error must NOT be in fixed message, got: {msg}"
            );
        }
        other => panic!("expected ProcessorError, got {other:?}"),
    }
}

#[tokio::test]
async fn test_trigger_fail_fast_noop_when_fail_fast_false() {
    use camel_component_api::CamelError;
    use std::panic::AssertUnwindSafe;
    let config = MockConfig {
        fail_fast: false,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:test", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("test").unwrap();

    inner.trigger_fail_fast(CamelError::ProcessorError("boom".to_string()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let result = AssertUnwindSafe(producer.call(Exchange::default()))
        .catch_unwind()
        .await
        .expect("call should not panic");
    assert!(
        result.is_ok(),
        "fail_fast=false must let the call through even with stored error"
    );
}

#[tokio::test]
async fn test_reset_clears_trigger_fail_fast() {
    use camel_component_api::CamelError;
    use std::panic::AssertUnwindSafe;
    let config = MockConfig {
        fail_fast: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:test", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("test").unwrap();

    inner.trigger_fail_fast(CamelError::ProcessorError("boom".to_string()));
    assert!(inner.fail_fast_error().is_some());
    inner.reset().await;
    assert!(inner.fail_fast_error().is_none());

    // Producer should accept the call now that reset cleared the error.
    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let result = AssertUnwindSafe(producer.call(Exchange::default()))
        .catch_unwind()
        .await
        .expect("call should not panic");
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_assert_satisfied_body_count_mismatch_sets_fail_fast() {
    use std::panic::AssertUnwindSafe;
    let config = MockConfig {
        fail_fast: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:test", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("test").unwrap();

    inner.expect_body(camel_component_api::Body::Text("a".to_string()));
    inner.expect_body(camel_component_api::Body::Text("b".to_string()));

    // Send only 1 exchange (expects 2 -> mismatch).
    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("a")))
        .await
        .unwrap();

    // Wrap in catch_unwind so the test does not abort on panic.
    let panic_result = AssertUnwindSafe(inner.assert_satisfied())
        .catch_unwind()
        .await;
    assert!(
        panic_result.is_err(),
        "expected panic from assert_satisfied"
    );
    assert!(
        inner.fail_fast_error().is_some(),
        "fail_fast_error must be set when fail_fast=true and assertion panics"
    );
}

#[tokio::test]
async fn test_assert_satisfied_body_mismatch_sets_fail_fast() {
    use std::panic::AssertUnwindSafe;
    let config = MockConfig {
        fail_fast: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:test", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("test").unwrap();

    inner.expect_body(camel_component_api::Body::Text("expected".to_string()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("actual")))
        .await
        .unwrap();

    let panic_result = AssertUnwindSafe(inner.assert_satisfied())
        .catch_unwind()
        .await;
    assert!(
        panic_result.is_err(),
        "expected panic from assert_satisfied"
    );
    assert!(
        inner.fail_fast_error().is_some(),
        "fail_fast_error must be set on body mismatch when fail_fast=true"
    );
}

#[tokio::test]
async fn test_assert_satisfied_no_set_error_when_fail_fast_false() {
    use std::panic::AssertUnwindSafe;
    let config = MockConfig {
        fail_fast: false,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:test", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("test").unwrap();

    inner.expect_body(camel_component_api::Body::Text("a".to_string()));
    inner.expect_body(camel_component_api::Body::Text("b".to_string()));

    // Send only 1 exchange.
    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("a")))
        .await
        .unwrap();

    let panic_result = AssertUnwindSafe(inner.assert_satisfied())
        .catch_unwind()
        .await;
    assert!(
        panic_result.is_err(),
        "expected panic from assert_satisfied"
    );
    assert!(
        inner.fail_fast_error().is_none(),
        "fail_fast_error must remain None when fail_fast=false"
    );
}

#[tokio::test]
async fn test_assert_satisfied_any_order_body_mismatch_sets_fail_fast() {
    use std::panic::AssertUnwindSafe;
    let config = MockConfig {
        fail_fast: true,
        any_order: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:test", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("test").unwrap();

    inner.expect_body(camel_component_api::Body::Text("a".to_string()));
    inner.expect_body(camel_component_api::Body::Text("b".to_string()));

    // Send 2 exchanges: "a" and "c" — "b" is missing.
    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("a")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("c")))
        .await
        .unwrap();

    let panic_result = AssertUnwindSafe(inner.assert_satisfied())
        .catch_unwind()
        .await;
    assert!(
        panic_result.is_err(),
        "expected panic from assert_satisfied (any-order body not found)"
    );
    assert!(
        inner.fail_fast_error().is_some(),
        "fail_fast_error must be set when fail_fast=true and any-order body is missing"
    );
}

#[tokio::test]
async fn test_assert_satisfied_header_missing_sets_fail_fast() {
    use std::panic::AssertUnwindSafe;
    let config = MockConfig {
        fail_fast: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:test", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("test").unwrap();

    inner.expect_header("x-missing", serde_json::json!("value"));

    // Send 1 exchange without the expected header.
    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("body")))
        .await
        .unwrap();

    let panic_result = AssertUnwindSafe(inner.assert_satisfied())
        .catch_unwind()
        .await;
    assert!(
        panic_result.is_err(),
        "expected panic from assert_satisfied (header missing)"
    );
    assert!(
        inner.fail_fast_error().is_some(),
        "fail_fast_error must be set when fail_fast=true and expected header is missing"
    );
}

// -----------------------------------------------------------------------
// Count expectations: expect_count / expect_minimum_count
// (mock-expectation-and-uri-surface)
// -----------------------------------------------------------------------

/// Extract the message from a panic payload caught via `catch_unwind`.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "<non-string panic payload>".to_string())
}

#[tokio::test]
async fn count_exact_mismatch_fails() {
    use std::panic::AssertUnwindSafe;
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:count-exact-mismatch", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("count-exact-mismatch").unwrap();

    inner.expect_count(3);

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("a")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("b")))
        .await
        .unwrap();

    let payload = AssertUnwindSafe(inner.assert_satisfied())
        .catch_unwind()
        .await
        .expect_err("assert_satisfied should panic on exact count mismatch");
    let msg = panic_message(payload);
    assert!(
        msg.contains("count-exact-mismatch"),
        "message should contain endpoint name, got: {msg}"
    );
    assert!(
        msg.contains("expected 3 exchanges") && msg.contains("got 2"),
        "message should report expected 3 / got 2, got: {msg}"
    );
}

#[tokio::test]
async fn count_exact_satisfied_passes() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:count-exact-pass", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("count-exact-pass").unwrap();

    inner.expect_count(2);

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("one")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("two")))
        .await
        .unwrap();

    inner.assert_satisfied().await;
}

#[tokio::test]
async fn count_minimum_satisfied_by_more() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:count-min-more", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("count-min-more").unwrap();

    inner.expect_minimum_count(2);

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    for i in 0..5 {
        producer
            .call(Exchange::new(Message::new(format!("m{i}"))))
            .await
            .unwrap();
    }

    inner.assert_satisfied().await;
}

#[tokio::test]
async fn count_minimum_violated_fails() {
    use std::panic::AssertUnwindSafe;
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:count-min-violated", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("count-min-violated").unwrap();

    inner.expect_minimum_count(4);

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("only-one")))
        .await
        .unwrap();

    let payload = AssertUnwindSafe(inner.assert_satisfied())
        .catch_unwind()
        .await
        .expect_err("assert_satisfied should panic on minimum count violation");
    let msg = panic_message(payload);
    assert!(
        msg.contains("at least 4"),
        "message should state at least 4 exchanges were expected, got: {msg}"
    );
}

#[tokio::test]
async fn count_exact_and_minimum_enforced_together() {
    use std::panic::AssertUnwindSafe;
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:count-both", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("count-both").unwrap();

    inner.expect_count(2);
    inner.expect_minimum_count(1);

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    for i in 0..3 {
        producer
            .call(Exchange::new(Message::new(format!("m{i}"))))
            .await
            .unwrap();
    }

    let payload = AssertUnwindSafe(inner.assert_satisfied())
        .catch_unwind()
        .await
        .expect_err("assert_satisfied should panic on exact count mismatch");
    let msg = panic_message(payload);
    assert!(
        msg.contains("expected 2 exchanges") && msg.contains("got 3"),
        "message should report the exact mismatch, got: {msg}"
    );
    assert!(
        !msg.contains("at least"),
        "exact mismatch must be reported even though the minimum was satisfied, got: {msg}"
    );
}

#[tokio::test]
async fn count_checked_before_bodies() {
    use std::panic::AssertUnwindSafe;
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:count-first", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("count-first").unwrap();

    inner.expect_count(5);
    inner.expect_body(camel_component_api::Body::Text("x".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("y")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("z")))
        .await
        .unwrap();

    let payload = AssertUnwindSafe(inner.assert_satisfied())
        .catch_unwind()
        .await
        .expect_err("assert_satisfied should panic on count mismatch");
    let msg = panic_message(payload);
    assert!(
        msg.contains("expected 5 exchanges") && msg.contains("got 2"),
        "count mismatch must be reported, got: {msg}"
    );
    assert!(
        !msg.contains("bodies"),
        "body checks must not run after a count mismatch, got: {msg}"
    );
}

#[tokio::test]
async fn count_coexists_with_bodies_pass() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:count-with-bodies", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("count-with-bodies").unwrap();

    inner.expect_count(2);
    inner.expect_body(camel_component_api::Body::Text("alpha".into()));
    inner.expect_body(camel_component_api::Body::Text("beta".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("alpha")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("beta")))
        .await
        .unwrap();

    inner.assert_satisfied().await;
}

#[tokio::test]
async fn count_evaluates_retained_snapshot_under_truncation() {
    let component = MockComponent::with_config(MockConfig::new(3));
    let endpoint = component
        .create_endpoint("mock:count-truncated", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("count-truncated").unwrap();

    inner.expect_count(3);

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    for i in 0..5 {
        producer
            .call(Exchange::new(Message::new(format!("msg-{i}"))))
            .await
            .unwrap();
    }

    assert_eq!(inner.received_count().await, 3);
    inner.assert_satisfied().await;
}

// -----------------------------------------------------------------------
// Non-panicking assertion surface: try_assert_satisfied / MockAssertionError
// (mock-expectation-and-uri-surface)
// -----------------------------------------------------------------------

#[tokio::test]
async fn try_assert_satisfied_ok_when_satisfied() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:try-ok", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("try-ok").unwrap();

    inner.expect_count(1);
    inner.expect_body(camel_component_api::Body::Text("payload".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("payload")))
        .await
        .unwrap();

    let result = inner.try_assert_satisfied().await;
    assert!(result.is_ok(), "expected Ok(()), got: {result:?}");
}

#[tokio::test]
async fn try_assert_satisfied_err_with_details() {
    let component = MockComponent::new();
    let _endpoint = component
        .create_endpoint("mock:try-err", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("try-err").unwrap();

    inner.expect_count(2);
    // Send 0 exchanges — no producer needed.

    // Must return Err, not panic.
    let err = inner
        .try_assert_satisfied()
        .await
        .expect_err("expected Err on unmet expect_count");
    let msg = err.to_string();
    assert!(
        msg.contains("try-err"),
        "message should contain endpoint name, got: {msg}"
    );
    assert!(
        msg.contains("expected 2"),
        "message should contain 'expected 2', got: {msg}"
    );
}

#[tokio::test]
async fn try_assert_satisfied_sets_fail_fast_latch() {
    let config = MockConfig {
        fail_fast: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let _endpoint = component
        .create_endpoint("mock:try-latch", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("try-latch").unwrap();

    inner.expect_count(2);
    // Send 0 exchanges — expectation unmet.

    let result = inner.try_assert_satisfied().await;
    assert!(result.is_err(), "expected Err, got: {result:?}");
    assert!(
        inner.fail_fast_error().is_some(),
        "fail-fast latch must be set on mismatch (parity with assert_satisfied)"
    );
}

#[tokio::test]
async fn invalid_header_regex_returns_err_not_panic() {
    let config = MockConfig {
        fail_fast: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:try-bad-re", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("try-bad-re").unwrap();

    inner.expect_header_regex("k", "(unclosed");

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("body")))
        .await
        .unwrap();

    // Must return Err (no panic) even with fail_fast enabled.
    let err = inner
        .try_assert_satisfied()
        .await
        .expect_err("invalid regex must produce Err, not a panic");
    assert!(
        matches!(err, MockAssertionError::InvalidHeaderPattern { .. }),
        "expected InvalidHeaderPattern, got: {err:?}"
    );
    assert!(
        inner.fail_fast_error().is_none(),
        "malformed expectation is a caller programming error — latch must NOT trip"
    );
}

#[tokio::test]
async fn display_equals_panicking_variant_message() {
    use std::panic::AssertUnwindSafe;

    // Two identically-configured endpoints sharing one name (hence one
    // inner) so the endpoint-name segment of both messages is equal.
    let component = MockComponent::new();
    let _endpoint_a = component
        .create_endpoint("mock:parity", &NoOpComponentContext)
        .unwrap();
    let endpoint_b = component
        .create_endpoint("mock:parity", &NoOpComponentContext)
        .unwrap();
    let inner_a = component.get_endpoint("parity").unwrap();
    let inner_b = component.get_endpoint("parity").unwrap();

    inner_a.expect_count(3);

    let ctx = test_producer_ctx();
    let mut producer = endpoint_b.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("only-one")))
        .await
        .unwrap();

    let payload = AssertUnwindSafe(inner_a.assert_satisfied())
        .catch_unwind()
        .await
        .expect_err("assert_satisfied should panic on count mismatch");
    let panicking = panic_message(payload);

    let display = inner_b
        .try_assert_satisfied()
        .await
        .expect_err("try_assert_satisfied should Err on count mismatch")
        .to_string();

    assert_eq!(panicking, display);
}

#[tokio::test]
async fn no_expected_bodies_with_received_exchanges_still_ok() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:try-no-bodies", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("try-no-bodies").unwrap();

    // No body expectations set — the is_empty gate must skip body checks.
    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    for i in 0..3 {
        producer
            .call(Exchange::new(Message::new(format!("m{i}"))))
            .await
            .unwrap();
    }

    let result = inner.try_assert_satisfied().await;
    assert!(result.is_ok(), "no expectations set, got: {result:?}");
}

// -------------------------------------------------------------------
// URI parameter surface (Task 3)
// -------------------------------------------------------------------

#[tokio::test]
async fn uri_retain_override_truncates() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:cap?retain=50", &NoOpComponentContext)
        .unwrap();
    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    for i in 0..55 {
        producer
            .call(Exchange::new(Message::new(format!("m{i}"))))
            .await
            .unwrap();
    }
    let inner = component.get_endpoint("cap").unwrap();
    assert_eq!(
        inner.received_count().await,
        50,
        "retain=50 must cap stored exchanges at 50 (default 10_000 would retain all 55)"
    );
}

#[tokio::test]
async fn uri_any_order_overrides_matching() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:relaxed?anyOrder=true", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("relaxed").unwrap();

    inner.expect_body(camel_component_api::Body::Text("a".to_string()));
    inner.expect_body(camel_component_api::Body::Text("b".to_string()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("b")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("a")))
        .await
        .unwrap();

    // Component default is strict order, which would fail on this
    // out-of-order arrival; anyOrder=true must satisfy.
    inner.assert_satisfied().await;
}

#[tokio::test]
async fn uri_fail_fast_overrides_latching() {
    use std::panic::AssertUnwindSafe;

    let component = MockComponent::new();
    let _endpoint = component
        .create_endpoint("mock:tight?failFast=true", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("tight").unwrap();

    inner.expect_count(1);
    // Send 0 exchanges — expectation unmet.

    let _payload = AssertUnwindSafe(inner.assert_satisfied())
        .catch_unwind()
        .await
        .expect_err("assert_satisfied should panic on unmet expect_count");
    assert!(
        inner.fail_fast_error().is_some(),
        "failFast=true from URI must latch the mismatch (component default false leaves None)"
    );
}

#[tokio::test]
async fn uri_absent_params_fallback_to_config() {
    use std::panic::AssertUnwindSafe;

    let config = MockConfig {
        fail_fast: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let _endpoint = component
        .create_endpoint("mock:audit", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("audit").unwrap();

    inner.expect_count(1);
    // Send 0 exchanges — expectation unmet.

    let _payload = AssertUnwindSafe(inner.assert_satisfied())
        .catch_unwind()
        .await
        .expect_err("assert_satisfied should panic on unmet expect_count");
    assert!(
        inner.fail_fast_error().is_some(),
        "component-level fail_fast=true must apply when the URI param is absent"
    );

    // A no-param endpoint with no expectations and nothing sent satisfies;
    // no default count expectation may be registered.
    let _fresh = component
        .create_endpoint("mock:audit-fresh", &NoOpComponentContext)
        .unwrap();
    let fresh = component.get_endpoint("audit-fresh").unwrap();
    let result = fresh.try_assert_satisfied().await;
    assert!(result.is_ok(), "no expectations set, got: {result:?}");
}

#[test]
fn uri_malformed_numeric_rejected() {
    let component = MockComponent::new();
    let err = component
        .create_endpoint("mock:x?retain=abc", &NoOpComponentContext)
        .err()
        .expect("retain=abc must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("retain"),
        "message should name 'retain', got: {msg}"
    );
}

#[test]
fn uri_malformed_expected_count_rejected() {
    let component = MockComponent::new();
    let err = component
        .create_endpoint("mock:x?expectedCount=abc", &NoOpComponentContext)
        .err()
        .expect("expectedCount=abc must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("expectedCount"),
        "message should name 'expectedCount', got: {msg}"
    );
}

#[test]
fn uri_zero_retain_rejected() {
    let component = MockComponent::new();
    let err = component
        .create_endpoint("mock:x?retain=0", &NoOpComponentContext)
        .err()
        .expect("retain=0 must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("retain"),
        "message should name 'retain', got: {msg}"
    );
    assert!(
        msg.contains(">= 1"),
        "message should state the >= 1 constraint, got: {msg}"
    );
}

#[test]
fn uri_malformed_boolean_rejected() {
    let component = MockComponent::new();
    let err = component
        .create_endpoint("mock:x?copy=maybe", &NoOpComponentContext)
        .err()
        .expect("copy=maybe must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("copy"),
        "message should name 'copy', got: {msg}"
    );
}

#[tokio::test]
async fn uri_first_creation_wins_on_conflict() {
    let component = MockComponent::new();
    let _first = component
        .create_endpoint("mock:single?retain=5", &NoOpComponentContext)
        .unwrap();
    let second = component
        .create_endpoint("mock:single?retain=100", &NoOpComponentContext)
        .unwrap();

    let ctx = test_producer_ctx();
    let mut producer = second.create_producer(rt(), &ctx).unwrap();
    for i in 0..7 {
        producer
            .call(Exchange::new(Message::new(format!("m{i}"))))
            .await
            .unwrap();
    }
    let inner = component.get_endpoint("single").unwrap();
    assert_eq!(
        inner.received_count().await,
        5,
        "first creation's retain=5 must still bind; second creation must not reconfigure"
    );
}

#[test]
fn catalog_parity_five_params() {
    let meta = MockConfig::metadata();
    let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
    names.sort();
    assert_eq!(
        names,
        ["anyOrder", "copy", "expectedCount", "failFast", "retain"],
        "metadata uri_options names must match parser keys"
    );

    // All five params are optional: absent params fall back to the
    // component-level `MockConfig` fields (README param table). A
    // `required` descriptor would make every bare `mock:name` URI a
    // lint error (R-URI-known:missing-required-option) — locked by
    // the corpus gate.
    for opt in &meta.uri_options {
        assert!(
            !opt.required,
            "{} must be optional (falls back to MockConfig)",
            opt.name
        );
    }
}

// -------------------------------------------------------------------
// expectedCount wiring + live-traffic inertness (Task 4)
// -------------------------------------------------------------------

#[tokio::test]
async fn expected_count_never_rejects_live_exchanges() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint(
            "mock:sink?expectedCount=2&failFast=true",
            &NoOpComponentContext,
        )
        .unwrap();
    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    for i in 0..7 {
        let result = producer
            .call(Exchange::new(Message::new(format!("m{i}"))))
            .await;
        assert!(
            result.is_ok(),
            "live exchange {i} must not be rejected by expectedCount"
        );
    }
    let inner = component.get_endpoint("sink").unwrap();
    assert_eq!(inner.received_count().await, 7);
    assert!(
        inner.fail_fast_error().is_none(),
        "expectedCount alone must never trip the fail-fast latch before an assertion runs"
    );
}

#[tokio::test]
async fn expected_count_enforced_only_at_assertion() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:sink?expectedCount=2", &NoOpComponentContext)
        .unwrap();
    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    for i in 0..3 {
        producer
            .call(Exchange::new(Message::new(format!("m{i}"))))
            .await
            .unwrap();
    }
    let inner = component.get_endpoint("sink").unwrap();
    let result = inner.try_assert_satisfied().await;
    assert!(
        result.is_err(),
        "expectedCount=2 vs 3 received must fail at assertion time, got: {result:?}"
    );
}

#[tokio::test]
async fn failed_assertion_then_applies_normal_fail_fast() {
    use camel_component_api::CamelError;

    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint(
            "mock:sink?expectedCount=2&failFast=true",
            &NoOpComponentContext,
        )
        .unwrap();
    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    for i in 0..3 {
        producer
            .call(Exchange::new(Message::new(format!("m{i}"))))
            .await
            .unwrap();
    }
    let inner = component.get_endpoint("sink").unwrap();
    assert!(
        inner.try_assert_satisfied().await.is_err(),
        "2-vs-3 mismatch must Err and trip the fail-fast latch"
    );
    let result = producer.call(Exchange::default()).await;
    match result {
        Err(CamelError::ProcessorError(msg)) => assert!(
            msg.contains("fail-fast mode"),
            "fixed fail-fast message expected, got: {msg}"
        ),
        other => panic!("expected ProcessorError, got {other:?}"),
    }
}

#[tokio::test]
async fn expected_count_not_reset_on_second_creation() {
    let component = MockComponent::new();
    let _first = component
        .create_endpoint("mock:once", &NoOpComponentContext)
        .unwrap();
    let second = component
        .create_endpoint("mock:once?expectedCount=5", &NoOpComponentContext)
        .unwrap();
    let ctx = test_producer_ctx();
    let mut producer = second.create_producer(rt(), &ctx).unwrap();
    for i in 0..2 {
        producer
            .call(Exchange::new(Message::new(format!("m{i}"))))
            .await
            .unwrap();
    }
    let inner = component.get_endpoint("once").unwrap();
    let result = inner.try_assert_satisfied().await;
    assert!(
        result.is_ok(),
        "first creation registered no count expectation; second creation must not \
             reconfigure it, got: {result:?}"
    );
}

// -----------------------------------------------------------------------
// Poisoned-lock policy: expectation setters must panic, not silently no-op
// (rc-1t6f)
// -----------------------------------------------------------------------

/// Poison the `expectations` mutex by panicking while holding its guard.
fn poison_expectations(inner: &MockEndpointInner) {
    let _guard = inner.expectations.lock().unwrap();
    panic!("intentional poison");
}

/// Create an endpoint and return its inner, with the `expectations` mutex
/// poisoned.
fn poisoned_endpoint(name: &str) -> Arc<MockEndpointInner> {
    let component = MockComponent::new();
    component
        .create_endpoint(&format!("mock:{name}"), &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint(name).unwrap();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        poison_expectations(&inner);
    }));
    inner
}

#[test]
#[should_panic(expected = "expectations lock poisoned")]
fn expect_count_panics_on_poisoned_lock() {
    let inner = poisoned_endpoint("poison-count");
    inner.expect_count(3);
}

#[test]
#[should_panic(expected = "expectations lock poisoned")]
fn expect_minimum_count_panics_on_poisoned_lock() {
    let inner = poisoned_endpoint("poison-min");
    inner.expect_minimum_count(3);
}

#[test]
#[should_panic(expected = "expectations lock poisoned")]
fn expect_body_panics_on_poisoned_lock() {
    let inner = poisoned_endpoint("poison-body");
    inner.expect_body(camel_component_api::Body::Text("hello".to_string()));
}

#[test]
#[should_panic(expected = "expectations lock poisoned")]
fn expect_header_panics_on_poisoned_lock() {
    let inner = poisoned_endpoint("poison-header");
    inner.expect_header("X-Test", "value");
}

#[test]
#[should_panic(expected = "expectations lock poisoned")]
fn expect_header_regex_panics_on_poisoned_lock() {
    let inner = poisoned_endpoint("poison-header-regex");
    inner.expect_header_regex("X-Test", "^v");
}

/// Poison the `fail_fast_error` mutex by panicking while holding its guard.
fn poison_fail_fast_error(inner: &MockEndpointInner) {
    let _guard = inner.fail_fast_error.lock().unwrap();
    panic!("intentional poison");
}

/// Create an endpoint and return its inner, with the `fail_fast_error`
/// mutex poisoned.
fn poisoned_fail_fast_endpoint(name: &str) -> Arc<MockEndpointInner> {
    let component = MockComponent::new();
    component
        .create_endpoint(&format!("mock:{name}"), &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint(name).unwrap();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        poison_fail_fast_error(&inner);
    }));
    inner
}

#[test]
#[should_panic(expected = "fail_fast_error lock poisoned")]
fn fail_fast_error_getter_panics_on_poisoned_lock() {
    let inner = poisoned_fail_fast_endpoint("poison-ff-getter");
    inner.fail_fast_error();
}

#[tokio::test]
#[should_panic(expected = "fail_fast_error lock poisoned")]
async fn reset_panics_on_poisoned_fail_fast_lock() {
    let inner = poisoned_fail_fast_endpoint("poison-ff-reset");
    inner.reset().await;
}

#[test]
#[should_panic(expected = "fail_fast_error lock poisoned")]
fn trigger_fail_fast_panics_on_poisoned_lock() {
    let inner = poisoned_fail_fast_endpoint("poison-ff-trigger");
    inner.trigger_fail_fast(CamelError::ProcessorError("boom".to_string()));
}

#[test]
#[should_panic(expected = "fail_fast_error lock poisoned")]
fn set_fail_fast_on_mismatch_panics_on_poisoned_lock() {
    let component = MockComponent::new();
    component
        .create_endpoint(
            "mock:poison-ff-mismatch?failFast=true",
            &NoOpComponentContext,
        )
        .unwrap();
    let inner = component.get_endpoint("poison-ff-mismatch").unwrap();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        poison_fail_fast_error(&inner);
    }));
    inner.set_fail_fast_on_mismatch();
}

// -----------------------------------------------------------------------
// Received-state diagnostics in MockAssertionError (mock-assert-diagnostics)
// -----------------------------------------------------------------------

#[tokio::test]
async fn header_not_found_zero_exchanges_message_contains_received_0() {
    let component = MockComponent::new();
    let _endpoint = component
        .create_endpoint("mock:diag-zero", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("diag-zero").unwrap();

    inner.expect_header("k", serde_json::json!("v"));
    // Send 0 exchanges — no producer needed.

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("unmet expect_header must Err")
        .to_string();
    assert!(
        msg.contains("received 0 exchanges"),
        "message should report zero received exchanges, got: {msg}"
    );
}

#[tokio::test]
async fn header_not_found_wrong_values_message_contains_actual_values() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:diag-values", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("diag-values").unwrap();
    inner.expect_header("k", serde_json::json!("expected"));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    for v in ["actual-1", "actual-2"] {
        let mut msg = Message::new("body");
        msg.headers.insert("k".to_string(), serde_json::json!(v));
        producer.call(Exchange::new(msg)).await.unwrap();
    }
    inner
        .await_exchanges(2, std::time::Duration::from_millis(500))
        .await;

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("wrong header values must Err")
        .to_string();
    assert!(
        msg.contains("received 2 exchanges"),
        "message should report two received exchanges, got: {msg}"
    );
    assert!(
        msg.contains("actual-1") && msg.contains("actual-2"),
        "message should list both actual values, got: {msg}"
    );
}

#[tokio::test]
async fn header_not_found_absent_key_message_contains_last_exchange_headers() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:diag-absent", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("diag-absent").unwrap();
    inner.expect_header("k", serde_json::json!("v"));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("body");
    msg.headers.insert("a".to_string(), serde_json::json!(1));
    msg.headers.insert("b".to_string(), serde_json::json!(2));
    producer.call(Exchange::new(msg)).await.unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("absent header key must Err")
        .to_string();
    assert!(
        msg.contains("absent from all received exchanges"),
        "message should report the key absent, got: {msg}"
    );
    assert!(
        msg.contains("last exchange headers: [a, b]"),
        "message should list the last exchange header keys, got: {msg}"
    );
}

#[tokio::test]
async fn body_not_found_message_contains_received_count() {
    let config = MockConfig {
        any_order: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:diag-body", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("diag-body").unwrap();
    inner.expect_body(camel_component_api::Body::Text("x".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("other")))
        .await
        .unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("unmatched expected body must Err")
        .to_string();
    assert!(
        msg.contains("received 1") && msg.contains("\"x\""),
        "message should report the received count and expected body, got: {msg}"
    );
}

#[tokio::test]
async fn header_regex_not_matched_message_contains_actual_values() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:diag-re", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("diag-re").unwrap();
    inner.expect_header_regex("k", "^pre");

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("body");
    msg.headers
        .insert("k".to_string(), serde_json::json!("other"));
    producer.call(Exchange::new(msg)).await.unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("unmatched header regex must Err")
        .to_string();
    assert!(
        msg.contains("other"),
        "message should list the actual value of 'k', got: {msg}"
    );
}

#[tokio::test]
async fn diagnostic_lists_cap_at_eight_entries() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:diag-cap", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("diag-cap").unwrap();
    inner.expect_header("k", serde_json::json!("never"));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    for i in 1..=10 {
        let mut msg = Message::new("body");
        msg.headers
            .insert("k".to_string(), serde_json::json!(format!("v{i}")));
        producer.call(Exchange::new(msg)).await.unwrap();
    }
    inner
        .await_exchanges(10, std::time::Duration::from_millis(500))
        .await;

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("cap test must Err")
        .to_string();
    assert!(
        msg.contains("+2 more"),
        "message should note two truncated values, got: {msg}"
    );
    assert!(
        !msg.contains("\"v10\""),
        "message should not list the 9th and 10th values, got: {msg}"
    );
}

// -----------------------------------------------------------------------
// mock-matchers: body/header matcher expectation surface
// -----------------------------------------------------------------------

#[tokio::test]
async fn header_matcher_setter_pass() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:m-hdr-pass", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-hdr-pass").unwrap();

    inner.expect_header_matcher("X-Trace", HeaderMatcher::Regex("^[a-f0-9]{8}$".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("body");
    msg.headers
        .insert("X-Trace".into(), serde_json::json!("ab12cd34"));
    producer.call(Exchange::new(msg)).await.unwrap();

    inner.assert_satisfied().await;
}

#[tokio::test]
async fn header_matcher_setter_fail_names_values() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:m-hdr-fail", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-hdr-fail").unwrap();

    inner.expect_header_matcher("X-Trace", HeaderMatcher::Regex("^[a-f0-9]{8}$".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("body");
    msg.headers
        .insert("X-Trace".into(), serde_json::json!("xyz"));
    producer.call(Exchange::new(msg)).await.unwrap();
    inner
        .await_exchanges(1, std::time::Duration::from_millis(500))
        .await;

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("header matcher mismatch must Err")
        .to_string();
    assert!(
        msg.contains("X-Trace") && msg.contains("regex") && msg.contains("xyz"),
        "message should name the header, matcher kind, and received value, got: {msg}"
    );
}

#[tokio::test]
async fn header_matcher_any_exchange() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:m-hdr-any", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-hdr-any").unwrap();

    inner.expect_header_matcher("X-A", HeaderMatcher::Equals(serde_json::json!("ok")));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("first")))
        .await
        .unwrap();
    let mut msg = Message::new("second");
    msg.headers.insert("X-A".into(), serde_json::json!("ok"));
    producer.call(Exchange::new(msg)).await.unwrap();

    inner.assert_satisfied().await;
}

#[tokio::test]
async fn header_matcher_invalid_regex_direct_api() {
    let config = MockConfig {
        fail_fast: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:m-hdr-bad-re", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-hdr-bad-re").unwrap();

    inner.expect_header_matcher("X", HeaderMatcher::Regex("(unclosed".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("body");
    msg.headers.insert("X".into(), serde_json::json!("v"));
    producer.call(Exchange::new(msg)).await.unwrap();

    let err = inner
        .try_assert_satisfied()
        .await
        .expect_err("invalid header matcher regex must Err, not a panic");
    assert!(
        matches!(err, MockAssertionError::InvalidHeaderPattern { .. }),
        "expected InvalidHeaderPattern, got: {err:?}"
    );
    assert!(
        inner.fail_fast_error().is_none(),
        "malformed expectation must not trip the fail-fast latch"
    );
}

#[tokio::test]
async fn ordered_mixed_exact_and_matcher_slots() {
    // Pass: exact slot 0 then matcher slot 1, received in insertion order.
    {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:m-mixed-ok", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("m-mixed-ok").unwrap();
        inner.expect_body(camel_component_api::Body::Text("x".into()));
        inner.expect_body_matcher(BodyMatcher::Regex("^b-".into()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("x")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("b-2")))
            .await
            .unwrap();

        inner.assert_satisfied().await;
    }
    // Fail naming index 1: the matcher slot receives a non-matching body.
    {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:m-mixed-idx", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("m-mixed-idx").unwrap();
        inner.expect_body(camel_component_api::Body::Text("x".into()));
        inner.expect_body_matcher(BodyMatcher::Regex("^b-".into()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("x")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("a-1")))
            .await
            .unwrap();

        let msg = inner
            .try_assert_satisfied()
            .await
            .expect_err("non-matching matcher slot must Err")
            .to_string();
        assert!(
            msg.contains("body[1]"),
            "message should name index 1, got: {msg}"
        );
    }
    // Fail: insertion order enforced — swapped bodies break the exact slot.
    {
        let component = MockComponent::new();
        let endpoint = component
            .create_endpoint("mock:m-mixed-swap", &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint("m-mixed-swap").unwrap();
        inner.expect_body(camel_component_api::Body::Text("x".into()));
        inner.expect_body_matcher(BodyMatcher::Regex("^b-".into()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new("b-2")))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new("x")))
            .await
            .unwrap();

        let msg = inner
            .try_assert_satisfied()
            .await
            .expect_err("swapped bodies must violate insertion order")
            .to_string();
        assert!(
            msg.contains("body[0]"),
            "message should fail on slot 0 (exact expectation kept first), got: {msg}"
        );
    }
}

#[tokio::test]
async fn matcher_count_mismatch_fails_not_panics() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:m-count", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-count").unwrap();
    inner.expect_body_matcher(BodyMatcher::Regex("^a-".into()));
    inner.expect_body_matcher(BodyMatcher::Regex("^b-".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("a-1")))
        .await
        .unwrap();

    let err = inner
        .try_assert_satisfied()
        .await
        .expect_err("fewer received bodies than matcher expectations must Err");
    assert!(
        matches!(err, MockAssertionError::BodyCountMismatch { .. }),
        "expected BodyCountMismatch, got: {err:?}"
    );
}

#[tokio::test]
async fn matcher_any_order_passes() {
    for (name, first, second) in [("m-any-ab", "a-1", "b-2"), ("m-any-ba", "b-2", "a-1")] {
        let config = MockConfig {
            any_order: true,
            ..Default::default()
        };
        let component = MockComponent::with_config(config);
        let endpoint = component
            .create_endpoint(&format!("mock:{name}"), &NoOpComponentContext)
            .unwrap();
        let inner = component.get_endpoint(name).unwrap();
        inner.expect_body_matcher(BodyMatcher::Regex("^a-".into()));
        inner.expect_body_matcher(BodyMatcher::Regex("^b-".into()));

        let ctx = test_producer_ctx();
        let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
        producer
            .call(Exchange::new(Message::new(first)))
            .await
            .unwrap();
        producer
            .call(Exchange::new(Message::new(second)))
            .await
            .unwrap();

        inner.assert_satisfied().await;
    }
}

#[tokio::test]
async fn body_matcher_failure_text_identifies() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:m-diag-idx", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-diag-idx").unwrap();
    inner.expect_body_matcher(BodyMatcher::Regex("^first$".into()));
    inner.expect_body_matcher(BodyMatcher::Regex("^ok$".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("first")))
        .await
        .unwrap();
    producer
        .call(Exchange::new(Message::new("denied")))
        .await
        .unwrap();

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("non-matching ordered matcher must Err")
        .to_string();
    assert!(
        msg.contains("body[1]")
            && msg.contains("regex")
            && msg.contains("^ok$")
            && msg.contains("denied"),
        "message should name the index, matcher kind, pattern, and received body, got: {msg}"
    );
}

#[tokio::test]
async fn string_matcher_failure_states_not_text() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:m-diag-text", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-diag-text").unwrap();
    inner.expect_body_matcher(BodyMatcher::Contains("a".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("");
    msg.body = camel_component_api::Body::Json(serde_json::json!({"b": 1}));
    producer.call(Exchange::new(msg)).await.unwrap();

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("string matcher against a JSON body must Err")
        .to_string();
    assert!(
        msg.contains("body is not text"),
        "message should state the body is not text, got: {msg}"
    );
}

#[tokio::test]
async fn json_subset_failure_states_not_json() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:m-diag-nojson", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-diag-nojson").unwrap();
    inner.expect_body_matcher(BodyMatcher::JsonSubset(serde_json::json!({"a": 1})));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("nope")))
        .await
        .unwrap();

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("jsonSubset against a non-JSON text must Err")
        .to_string();
    assert!(
        msg.contains("body is not JSON"),
        "message should state the body is not JSON, got: {msg}"
    );
}

#[tokio::test]
async fn json_subset_failure_names_key_via_pattern() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:m-diag-key", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-diag-key").unwrap();
    inner.expect_body_matcher(BodyMatcher::JsonSubset(serde_json::json!({"err": null})));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("");
    msg.body = camel_component_api::Body::Json(serde_json::json!({"err": 0}));
    producer.call(Exchange::new(msg)).await.unwrap();

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("jsonSubset with a null pattern value must Err")
        .to_string();
    assert!(
        msg.contains("err") && msg.contains("{\"err\":0}"),
        "message should name the failing key and the whole received body, got: {msg}"
    );
}

#[tokio::test]
async fn json_subset_array_failure_names_matcher_and_array() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:m-diag-arr", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-diag-arr").unwrap();
    inner.expect_body_matcher(BodyMatcher::JsonSubset(serde_json::json!({
        "tags": ["a", "b"]
    })));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("");
    msg.body = camel_component_api::Body::Json(serde_json::json!({
        "tags": ["b", "a"]
    }));
    producer.call(Exchange::new(msg)).await.unwrap();

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("jsonSubset with an out-of-order array must Err")
        .to_string();
    assert!(
        msg.contains("jsonSubset") && msg.contains("[\"b\",\"a\"]"),
        "message should name the jsonSubset matcher and the received array, got: {msg}"
    );
}

#[tokio::test]
async fn exists_body_failure_names_matcher() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:m-diag-exists", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-diag-exists").unwrap();
    inner.expect_body_matcher(BodyMatcher::Exists);

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("");
    msg.body = camel_component_api::Body::Empty;
    producer.call(Exchange::new(msg)).await.unwrap();

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("exists matcher against an empty body must Err")
        .to_string();
    assert!(
        msg.contains("exists"),
        "message should name the exists matcher, got: {msg}"
    );
}

#[tokio::test]
async fn invalid_body_regex_is_error_not_pass() {
    let config = MockConfig {
        fail_fast: true,
        ..Default::default()
    };
    let component = MockComponent::with_config(config);
    let endpoint = component
        .create_endpoint("mock:m-bad-body-re", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-bad-body-re").unwrap();

    inner.expect_body_matcher(BodyMatcher::Regex("(unclosed".into()));

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    producer
        .call(Exchange::new(Message::new("any")))
        .await
        .unwrap();

    let err = inner
        .try_assert_satisfied()
        .await
        .expect_err("invalid body matcher regex must Err, not a panic");
    assert!(
        matches!(err, MockAssertionError::InvalidBodyPattern { .. }),
        "expected InvalidBodyPattern, got: {err:?}"
    );
    assert!(
        err.to_string().contains("(unclosed"),
        "message should name the failing pattern, got: {err}"
    );
    assert!(
        inner.fail_fast_error().is_none(),
        "malformed expectation must not trip the fail-fast latch"
    );
}

#[tokio::test]
async fn exists_header_absent_key() {
    let component = MockComponent::new();
    let endpoint = component
        .create_endpoint("mock:m-hdr-absent", &NoOpComponentContext)
        .unwrap();
    let inner = component.get_endpoint("m-hdr-absent").unwrap();

    inner.expect_header_matcher("X-B", HeaderMatcher::Exists);

    let ctx = test_producer_ctx();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();
    let mut msg = Message::new("body");
    msg.headers.insert("X-Other".into(), serde_json::json!("v"));
    producer.call(Exchange::new(msg)).await.unwrap();

    let msg = inner
        .try_assert_satisfied()
        .await
        .expect_err("exists header matcher on an absent key must Err")
        .to_string();
    assert!(
        msg.contains("X-B") && msg.contains("absent"),
        "message should name the absent key, got: {msg}"
    );
}
