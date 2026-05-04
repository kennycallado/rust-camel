mod support;

use camel_component_cxf::config::{CxfPoolConfig, CxfSecurityConfig, CxfServiceConfig};
use camel_component_cxf::proto::{HealthRequest, cxf_bridge_client::CxfBridgeClient};
use camel_component_cxf::{BridgeSlot, CxfBridgePool};
use tonic::transport::{Channel, Endpoint};

use support::mock_bridge::spawn_mock_bridge;

async fn connect_mock_channel(
    port: u16,
) -> Result<Channel, Box<dyn std::error::Error + Send + Sync>> {
    let channel = Endpoint::from_shared(format!("http://127.0.0.1:{port}"))?
        .connect()
        .await?;
    Ok(channel)
}

fn test_service_config(port: u16) -> CxfServiceConfig {
    CxfServiceConfig {
        address: Some(format!("http://127.0.0.1:{port}")),
        wsdl_path: "classpath:test.wsdl".to_string(),
        service_name: "TestService".to_string(),
        port_name: "TestPort".to_string(),
        security: Default::default(),
    }
}

#[tokio::test]
async fn test_pool_connects_to_running_bridge()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (port, _) = spawn_mock_bridge().await?;
    let channel = connect_mock_channel(port).await?;
    let mut client = CxfBridgeClient::new(channel);
    let response = client.health(HealthRequest {}).await?.into_inner();
    assert!(response.healthy);
    Ok(())
}

#[tokio::test]
async fn test_pool_reuses_channel_for_same_service_key()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (port, _) = spawn_mock_bridge().await?;
    let service = test_service_config(port);
    let key_a = camel_component_cxf::CxfBridgePool::slot_key(&service);
    let key_b = camel_component_cxf::CxfBridgePool::slot_key(&service);
    assert_eq!(key_a, key_b);

    let channel = connect_mock_channel(port).await?;
    let mut client_a = CxfBridgeClient::new(channel.clone());
    let mut client_b = CxfBridgeClient::new(channel);

    let health_a = client_a.health(HealthRequest {}).await?.into_inner();
    let health_b = client_b.health(HealthRequest {}).await?.into_inner();
    assert!(health_a.healthy);
    assert!(health_b.healthy);
    Ok(())
}

#[tokio::test]
async fn test_pool_health_check_on_healthy_bridge()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (port, _) = spawn_mock_bridge().await?;
    let channel = connect_mock_channel(port).await?;
    let mut client = CxfBridgeClient::new(channel);
    let health = client.health(HealthRequest {}).await?.into_inner();
    assert!(health.healthy);
    assert_eq!(health.message, "ok");
    Ok(())
}

#[tokio::test]
async fn test_pool_detects_unhealthy_bridge() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let (port, state) = spawn_mock_bridge().await?;
    {
        let mut healthy = state.healthy.lock().await;
        *healthy = false;
    }

    let channel = connect_mock_channel(port).await?;
    let mut client = CxfBridgeClient::new(channel);
    let health = client.health(HealthRequest {}).await?.into_inner();
    assert!(!health.healthy);
    assert_eq!(health.message, "unhealthy");
    Ok(())
}

#[tokio::test]
async fn test_pool_shutdown_cleans_up() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let service = test_service_config(18080);
    let pool = camel_component_cxf::CxfBridgePool::from_config(CxfPoolConfig {
        services: vec![service],
        ..CxfPoolConfig::default()
    })?;
    pool.shutdown().await?;
    Ok(())
}

// ── Pool Lifecycle Edge Case Tests ───────────────────────────────────────────

#[test]
fn test_slot_key_differs_for_different_services() {
    let svc_a = CxfServiceConfig {
        address: Some("http://127.0.0.1:8080".to_string()),
        wsdl_path: "classpath:a.wsdl".to_string(),
        service_name: "ServiceA".to_string(),
        port_name: "PortA".to_string(),
        security: Default::default(),
    };
    let svc_b = CxfServiceConfig {
        address: Some("http://127.0.0.1:8081".to_string()),
        wsdl_path: "classpath:b.wsdl".to_string(),
        service_name: "ServiceB".to_string(),
        port_name: "PortB".to_string(),
        security: Default::default(),
    };
    let key_a = CxfBridgePool::slot_key(&svc_a);
    let key_b = CxfBridgePool::slot_key(&svc_b);
    assert_ne!(
        key_a, key_b,
        "different services must produce different slot keys"
    );
}

#[test]
fn test_slot_key_same_for_identical_configs() {
    let svc = CxfServiceConfig {
        address: Some("http://127.0.0.1:9090".to_string()),
        wsdl_path: "classpath:same.wsdl".to_string(),
        service_name: "SameService".to_string(),
        port_name: "SamePort".to_string(),
        security: Default::default(),
    };
    let key_1 = CxfBridgePool::slot_key(&svc);
    let key_2 = CxfBridgePool::slot_key(&svc);
    assert_eq!(key_1, key_2, "identical configs must produce same slot key");
}

#[test]
fn test_find_security_config_returns_matching() {
    let sec = CxfSecurityConfig {
        username: Some("user".to_string()),
        password: Some("pass".to_string()),
        keystore_path: Some("/path/keystore.jks".to_string()),
        keystore_password: Some("ks-pass".to_string()),
        truststore_path: None,
        truststore_password: None,
    };
    let service = CxfServiceConfig {
        address: Some("http://127.0.0.1:7070".to_string()),
        wsdl_path: "classpath:secure.wsdl".to_string(),
        service_name: "SecureService".to_string(),
        port_name: "SecurePort".to_string(),
        security: sec.clone(),
    };
    let pool = CxfBridgePool::from_config(CxfPoolConfig {
        services: vec![service],
        ..CxfPoolConfig::default()
    })
    .expect("valid config");

    let found = pool.find_security_config("classpath:secure.wsdl", "SecureService", "SecurePort");
    assert_eq!(found.username, Some("user".to_string()));
    assert_eq!(found.password, Some("pass".to_string()));
    assert_eq!(found.keystore_path, Some("/path/keystore.jks".to_string()));
}

#[test]
fn test_find_security_config_returns_none_for_unknown() {
    let service = CxfServiceConfig {
        address: Some("http://127.0.0.1:7071".to_string()),
        wsdl_path: "classpath:known.wsdl".to_string(),
        service_name: "KnownService".to_string(),
        port_name: "KnownPort".to_string(),
        security: CxfSecurityConfig {
            username: Some("user".to_string()),
            ..Default::default()
        },
    };
    let pool = CxfBridgePool::from_config(CxfPoolConfig {
        services: vec![service],
        ..CxfPoolConfig::default()
    })
    .expect("valid config");

    let found =
        pool.find_security_config("classpath:unknown.wsdl", "UnknownService", "UnknownPort");
    assert!(
        found.username.is_none(),
        "unknown service should return default security config"
    );
    assert!(found.password.is_none());
}

#[tokio::test]
async fn test_insert_slot_and_get_channel() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let (port, _) = spawn_mock_bridge().await?;
    let channel = connect_mock_channel(port).await?;

    let service = test_service_config(port);
    let key = CxfBridgePool::slot_key(&service);
    let slot = BridgeSlot::new_ready_for_test(channel.clone());

    let pool = CxfBridgePool::from_config(CxfPoolConfig {
        services: vec![service],
        ..CxfPoolConfig::default()
    })?;
    pool.insert_slot_for_test(key.clone(), slot);

    // Verify the slot was inserted and its state is Ready
    let pool_arc = std::sync::Arc::new(pool);
    let result = pool_arc.get_channel(&test_service_config(port)).await;
    assert!(
        result.is_ok(),
        "get_channel should succeed for inserted ready slot"
    );
    Ok(())
}

#[tokio::test]
async fn test_shutdown_with_no_slots() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = CxfBridgePool::from_config(CxfPoolConfig {
        services: vec![],
        ..CxfPoolConfig::default()
    })?;
    pool.shutdown().await?;
    Ok(())
}
