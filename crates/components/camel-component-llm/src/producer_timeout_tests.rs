// Timeout enforcement tests: total deadline (materialized),
// per-activity timeout (streaming), and no-timeout baseline.

use std::sync::Arc;
use std::time::Duration;

use camel_api::Body;
use tower::Service;

use crate::LlmEndpointConfig;
use crate::config::LlmOperation;
use crate::producer::LlmProducer;
use crate::provider::mock::{MockMode, MockProvider};

use super::producer_test_helpers::{make_exchange, make_producer_with_timeout};

#[tokio::test]
async fn materialized_total_timeout_fires() {
    let provider = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into())).with_delay(Duration::from_millis(200)),
    );
    let mut producer = make_producer_with_timeout(provider, false, Duration::from_millis(50));
    let res = producer.call(make_exchange(Body::Text("x".into()))).await;
    let err = res.unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("timeout"),
        "got: {err}"
    );
}

#[tokio::test]
async fn streaming_activity_timeout_fires() {
    let provider = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into())).with_delay(Duration::from_millis(200)),
    );
    let mut producer = make_producer_with_timeout(provider, true, Duration::from_millis(50));
    let mut out = producer
        .call(make_exchange(Body::Text("x".into())))
        .await
        .unwrap();
    let body = std::mem::replace(&mut out.input.body, Body::Empty);
    if let Body::Stream(sb) = body {
        let mut guard = sb.stream.lock().await;
        if let Some(stream) = guard.as_mut() {
            use futures::StreamExt;
            let res = stream.next().await.unwrap();
            assert!(
                res.is_err(),
                "streaming activity timeout should produce an error"
            );
            let err = res.unwrap_err();
            assert!(
                err.to_string().to_lowercase().contains("timeout"),
                "streaming timeout error should mention 'timeout', got: {err}"
            );
        }
    }
}

#[tokio::test]
async fn materialized_no_timeout_does_not_fire() {
    // With no timeout configured, a slow provider should complete normally.
    let provider = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into())).with_delay(Duration::from_millis(20)),
    );
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("x".into()));
    producer.handle_chat(&mut exchange).await.expect("chat ok");
    match &exchange.input.body {
        Body::Text(s) => assert_eq!(s, "ok"),
        other => panic!("expected Text, got {other:?}"),
    }
}
