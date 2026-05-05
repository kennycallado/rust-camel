mod support;

use std::sync::Arc;

use camel_component_api::{Body, Exchange, Message, Value};
use camel_component_cxf::{
    BridgeSlot, CxfBridgePool,
    config::CxfPoolConfig,
    producer::CxfProducer,
};
use tonic::transport::Channel;
use tower::Service;

use crate::support::mock_bridge::spawn_mock_bridge;

async fn prepare_ready_slot(
    pool: &Arc<CxfBridgePool>,
    channel: Channel,
) {
    let key = CxfBridgePool::slot_key();
    let slot = BridgeSlot::new_ready_for_test(channel);
    pool.insert_slot_for_test(key, slot);
}

#[tokio::test]
async fn test_producer_with_profile_invokes_successfully() {
    let (port, state) = spawn_mock_bridge().await.expect("mock bridge should start");
    let pool_config = CxfPoolConfig {
        profiles: vec![],
        max_bridges: 1,
        bridge_start_timeout_ms: 10,
        health_check_interval_ms: 1_000,
        bridge_cache_dir: None,
        version: "0.1.0".to_string(),
        bind_address: None,
    };
    let pool = Arc::new(CxfBridgePool::from_config(pool_config).expect("pool should build"));
    let channel = camel_bridge::channel::connect_channel(port)
        .await
        .expect("channel should connect");
    prepare_ready_slot(&pool, channel).await;

    let mut producer = CxfProducer::new(
        pool,
        "test_profile".to_string(),
        "classpath:Hello.wsdl".to_string(),
        "HelloService".to_string(),
        "HelloPort".to_string(),
        Some(format!("http://127.0.0.1:{port}")),
        "defaultOp".to_string(),
    );
    let mut exchange = Exchange::new(Message::new(Body::Text("<ping/>".to_string())));
    exchange
        .input
        .set_header("CamelCxfOperation", Value::String("sayHello".to_string()));

    let out = producer
        .call(exchange)
        .await
        .expect("invoke should succeed");
    assert_eq!(out.input.body, Body::Bytes(bytes::Bytes::from("<ping/>")));

    let req = state
        .last_invoke
        .lock()
        .await
        .clone()
        .expect("mock should receive invoke");
    assert_eq!(req.operation, "sayHello");
    assert_eq!(req.security_profile, "test_profile");
}

#[tokio::test]
async fn test_producer_handles_empty_security_profile() {
    let (port, state) = spawn_mock_bridge().await.expect("mock bridge should start");
    let pool_config = CxfPoolConfig {
        profiles: vec![],
        max_bridges: 1,
        bridge_start_timeout_ms: 10,
        health_check_interval_ms: 1_000,
        bridge_cache_dir: None,
        version: "0.1.0".to_string(),
        bind_address: None,
    };
    let pool = Arc::new(CxfBridgePool::from_config(pool_config).expect("pool should build"));
    let channel = camel_bridge::channel::connect_channel(port)
        .await
        .expect("channel should connect");
    prepare_ready_slot(&pool, channel).await;

    let mut producer = CxfProducer::new(
        pool,
        "insecure".to_string(),
        "classpath:Hello.wsdl".to_string(),
        "HelloService".to_string(),
        "HelloPort".to_string(),
        Some(format!("http://127.0.0.1:{port}")),
        "emptySecurityOp".to_string(),
    );
    let exchange = Exchange::new(Message::new(Body::Text("<empty/>".to_string())));
    let out = producer
        .call(exchange)
        .await
        .expect("invoke should succeed");

    assert_eq!(out.input.body, Body::Bytes(bytes::Bytes::from("<empty/>")));
    let req = state
        .last_invoke
        .lock()
        .await
        .clone()
        .expect("mock should receive invoke");
    assert_eq!(req.operation, "emptySecurityOp");
    assert_eq!(req.security_profile, "insecure");
}
