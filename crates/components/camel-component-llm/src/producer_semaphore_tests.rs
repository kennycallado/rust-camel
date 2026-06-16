// Semaphore concurrency tests: max concurrency enforcement,
// stream drop cancellation, permit lifecycle.

use std::sync::Arc;
use std::time::Duration;

use camel_api::Body;
use tower::Service;

use crate::provider::LlmProvider;
use crate::provider::mock::{MockMode, MockProvider};

use super::producer_test_helpers::{
    drain_one_chunk, drain_stream, make_exchange, make_producer_with_semaphore,
};

#[tokio::test]
async fn semaphore_bounds_inflight_materialized() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_delay(Duration::from_millis(80))
            .with_concurrent_tracker(),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let producer = make_producer_with_semaphore(provider, false, 2);
    let mut handles = vec![];
    for _ in 0..8 {
        let mut p = producer.clone();
        handles.push(tokio::spawn(async move {
            p.call(make_exchange(Body::Text("x".into()))).await
        }));
    }
    for h in handles {
        let _ = h.await;
    }
    assert_eq!(
        mock.max_concurrent(),
        2,
        "with 8 concurrent 80ms calls at max_concurrency=2, peak must be exactly 2"
    );
}

#[tokio::test]
async fn dropping_stream_body_drops_upstream_stream() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_delay(Duration::from_millis(100))
            .with_cancellation_tracking(),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let mut producer = make_producer_with_semaphore(provider, true, 1);
    let mut out = producer
        .call(make_exchange(Body::Text("x".into())))
        .await
        .unwrap();
    drain_one_chunk(&mut out).await; // start the stream
    drop(out); // drops Body::Stream -> PermitStream -> inner stream
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        mock.was_cancelled(),
        "dropping Body::Stream must drop the upstream provider stream, firing the Mock's CancelGuard"
    );
}

#[tokio::test]
async fn streaming_permit_held_until_stream_consumed() {
    let mock = Arc::new(
        MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_delay(Duration::from_millis(30))
            .with_concurrent_tracker(),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let producer = make_producer_with_semaphore(provider, true, 1);

    // First call: take the stream but do NOT consume it yet.
    let mut p1 = producer.clone();
    let ex1 = make_exchange(Body::Text("a".into()));
    let mut out1 = p1.call(ex1).await.expect("first call ok");

    // body is now a Stream — permit still held. A second call must block.
    let mut p2 = producer.clone();
    let second = tokio::spawn(async move { p2.call(make_exchange(Body::Text("b".into()))).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !second.is_finished(),
        "second call must wait while first stream is unconsumed"
    );

    // now drain the first stream (consuming it releases the permit)
    drain_stream(&mut out1).await;

    // second can now proceed
    let _ = second.await;
}
