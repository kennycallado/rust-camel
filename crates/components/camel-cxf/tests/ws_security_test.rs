mod support;

use std::sync::Arc;

use camel_component_api::{Body, Exchange, Message, Value};
use camel_component_cxf::{
    BridgeSlot, CxfBridgePool,
    config::{CxfPoolConfig, CxfSecurityConfig, CxfServiceConfig},
    producer::CxfProducer,
};
use tonic::transport::Channel;
use tower::Service;

use crate::support::mock_bridge::spawn_mock_bridge;

fn security_config() -> CxfSecurityConfig {
    CxfSecurityConfig {
        username: Some("alice".to_string()),
        password: Some("secret".to_string()),
        keystore_path: Some("/tmp/keystore.jks".to_string()),
        keystore_password: Some("ks-pass".to_string()),
        truststore_path: Some("/tmp/truststore.jks".to_string()),
        truststore_password: Some("ts-pass".to_string()),
    }
}

fn service_config_for_port(port: u16, security: CxfSecurityConfig) -> CxfServiceConfig {
    CxfServiceConfig {
        address: Some(format!("http://127.0.0.1:{port}")),
        wsdl_path: "classpath:Hello.wsdl".to_string(),
        service_name: "HelloService".to_string(),
        port_name: "HelloPort".to_string(),
        security,
    }
}

async fn prepare_ready_slot(
    pool: &Arc<CxfBridgePool>,
    service: &CxfServiceConfig,
    channel: Channel,
) {
    let key = CxfBridgePool::slot_key(service);
    let slot = BridgeSlot::new_ready_for_test(channel);
    pool.insert_slot_for_test(key, slot);
}

#[test]
fn test_security_config_preserves_env_references() {
    let toml_str = r#"
[[services]]
wsdl_path = "service.wsdl"
service_name = "SecureService"
port_name = "SecurePort"

[services.security]
username = "${env:CXF_USER}"
password = "${env:CXF_PASS}"
keystore_path = "${env:KEYSTORE_PATH}"
keystore_password = "${env:KEYSTORE_PASSWORD}"
truststore_path = "${env:TRUSTSTORE_PATH}"
truststore_password = "${env:TRUSTSTORE_PASSWORD}"
"#;

    let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("config should parse");
    let sec = &cfg.services[0].security;

    assert_eq!(sec.username.as_deref(), Some("${env:CXF_USER}"));
    assert_eq!(sec.password.as_deref(), Some("${env:CXF_PASS}"));
    assert_eq!(sec.keystore_path.as_deref(), Some("${env:KEYSTORE_PATH}"));
    assert_eq!(
        sec.keystore_password.as_deref(),
        Some("${env:KEYSTORE_PASSWORD}")
    );
    assert_eq!(
        sec.truststore_path.as_deref(),
        Some("${env:TRUSTSTORE_PATH}")
    );
    assert_eq!(
        sec.truststore_password.as_deref(),
        Some("${env:TRUSTSTORE_PASSWORD}")
    );
}

#[tokio::test]
async fn test_producer_with_security_config_invokes_successfully() {
    let (port, state) = spawn_mock_bridge().await.expect("mock bridge should start");
    let service = service_config_for_port(port, security_config());
    let pool_config = CxfPoolConfig {
        services: vec![service.clone()],
        max_bridges: 1,
        bridge_start_timeout_ms: 10,
        health_check_interval_ms: 1_000,
        bridge_cache_dir: None,
        version: "0.1.0".to_string(),
    };
    let pool = Arc::new(CxfBridgePool::from_config(pool_config).expect("pool should build"));
    let channel = camel_bridge::channel::connect_channel(port)
        .await
        .expect("channel should connect");
    prepare_ready_slot(&pool, &service, channel).await;

    let mut producer = CxfProducer::new(pool, service, "defaultOp".to_string());
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
}

#[tokio::test]
async fn test_producer_handles_empty_security_values() {
    let (port, state) = spawn_mock_bridge().await.expect("mock bridge should start");
    let service = service_config_for_port(
        port,
        CxfSecurityConfig {
            username: Some(String::new()),
            password: None,
            keystore_path: Some(String::new()),
            keystore_password: None,
            truststore_path: Some(String::new()),
            truststore_password: None,
        },
    );
    let pool_config = CxfPoolConfig {
        services: vec![service.clone()],
        max_bridges: 1,
        bridge_start_timeout_ms: 10,
        health_check_interval_ms: 1_000,
        bridge_cache_dir: None,
        version: "0.1.0".to_string(),
    };
    let pool = Arc::new(CxfBridgePool::from_config(pool_config).expect("pool should build"));
    let channel = camel_bridge::channel::connect_channel(port)
        .await
        .expect("channel should connect");
    prepare_ready_slot(&pool, &service, channel).await;

    let mut producer = CxfProducer::new(pool, service, "emptySecurityOp".to_string());
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
}
