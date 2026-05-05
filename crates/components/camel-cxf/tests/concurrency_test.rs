mod support;

use std::sync::Arc;

use camel_component_api::{Body, Exchange, Message};
use camel_component_cxf::config::CxfPoolConfig;
use camel_component_cxf::producer::CxfProducer;
use camel_component_cxf::{BridgeSlot, CxfBridgePool};
use support::mock_bridge::{MockState, spawn_mock_bridge};
use tokio::sync::Mutex;
use tonic::transport::Endpoint;
use tower::Service;

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn make_producer() -> (CxfProducer, MockState) {
    let (port, state) = spawn_mock_bridge().await.expect("mock bridge");
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect()
        .await
        .expect("connect");

    let producer = CxfProducer::from_channel(
        channel,
        "test".to_string(),
        "service.wsdl".to_string(),
        "TestService".to_string(),
        "TestPort".to_string(),
        Some("http://localhost:8080/ws".to_string()),
        "defaultOperation".to_string(),
    );
    (producer, state)
}

async fn make_pool_with_ready_slot() -> (Arc<CxfBridgePool>, MockState) {
    let (port, state) = spawn_mock_bridge().await.expect("mock bridge");
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect()
        .await
        .expect("connect");

    let key = CxfBridgePool::slot_key();
    let slot = BridgeSlot::new_ready_for_test(channel);

    let pool_config = CxfPoolConfig {
        profiles: vec![],
        ..CxfPoolConfig::default()
    };
    let pool = Arc::new(CxfBridgePool::from_config(pool_config).expect("valid config"));
    pool.insert_slot_for_test(key, slot);

    (pool, state)
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// Spawn 20 tasks, each calling producer.call with a unique exchange.
/// Since CxfProducer::call takes &mut self, we wrap the producer in Arc<Mutex<>>,
/// which serializes access — so invocations are sequential, not truly concurrent.
#[tokio::test]
async fn test_producer_20_serial_invocations_succeed() {
    let (producer, state) = make_producer().await;
    *state.invoke_response_payload.lock().await = Some(b"ok-response".to_vec());

    let producer = Arc::new(Mutex::new(producer));
    let num_tasks = 20;

    let mut handles = Vec::with_capacity(num_tasks);
    for i in 0..num_tasks {
        let producer = Arc::clone(&producer);
        let handle = tokio::spawn(async move {
            let exchange = Exchange::new(Message::new(Body::Text(format!("request-{i}"))));
            let mut guard = producer.lock().await;
            let out = guard.call(exchange).await.expect("call should succeed");
            // Verify response body is set
            assert!(
                matches!(out.input.body, Body::Bytes(_)),
                "expected Bytes body, got {:?}",
                out.input.body
            );
        });
        handles.push(handle);
    }

    for (i, handle) in handles.into_iter().enumerate() {
        handle
            .await
            .unwrap_or_else(|e| panic!("task {i} panicked: {e}"));
    }
}

/// Create a pool with one ready slot. Spawn 20 tasks, each calling pool.get_channel.
/// Verify all succeed and return a valid channel.
#[tokio::test]
async fn test_pool_get_channel_concurrent_access() {
    let (pool, _state) = make_pool_with_ready_slot().await;

    let num_tasks = 20;
    let mut handles = Vec::with_capacity(num_tasks);

    for i in 0..num_tasks {
        let pool = Arc::clone(&pool);
        let handle = tokio::spawn(async move {
            let channel = pool
                .get_channel()
                .await
                .expect("get_channel should succeed");
            // Verify the channel is usable by creating a client and calling health
            let mut client =
                camel_component_cxf::proto::cxf_bridge_client::CxfBridgeClient::new(channel);
            let resp = client
                .health(camel_component_cxf::proto::HealthRequest {})
                .await
                .expect("health call");
            assert!(
                resp.into_inner().healthy,
                "task {i}: bridge should be healthy"
            );
        });
        handles.push(handle);
    }

    for (i, handle) in handles.into_iter().enumerate() {
        handle
            .await
            .unwrap_or_else(|e| panic!("task {i} panicked: {e}"));
    }
}

/// Create a pool with one ready slot. Call restart_slot on it.
/// Verify the pool doesn't panic and remains in a valid state.
/// Note: no requests are in flight during restart — this just verifies no panic.
#[tokio::test]
async fn test_pool_restart_slot_no_panic() {
    let (pool, _state) = make_pool_with_ready_slot().await;
    let key = CxfBridgePool::slot_key();

    // Call restart_slot — should not panic
    pool.restart_slot(&key);

    // Verify we can still call shutdown without panic
    pool.shutdown().await.expect("shutdown should succeed");
}
