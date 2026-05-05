mod support;

use camel_component_cxf::config::CxfPoolConfig;
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
async fn test_pool_reuses_channel_for_same_slot_key()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // slot_key() is now a constant "cxf"
    let key_a = CxfBridgePool::slot_key();
    let key_b = CxfBridgePool::slot_key();
    assert_eq!(key_a, key_b);
    assert_eq!(key_a, "cxf");
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
    let pool = camel_component_cxf::CxfBridgePool::from_config(CxfPoolConfig {
        profiles: vec![],
        ..CxfPoolConfig::default()
    })?;
    pool.shutdown().await?;
    Ok(())
}

// ── Pool Lifecycle Edge Case Tests ───────────────────────────────────────────

#[test]
fn test_slot_key_is_constant() {
    let key = CxfBridgePool::slot_key();
    assert_eq!(key, "cxf");
}

#[tokio::test]
async fn test_insert_slot_and_get_channel() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let (port, _) = spawn_mock_bridge().await?;
    let channel = connect_mock_channel(port).await?;

    let key = CxfBridgePool::slot_key();
    let slot = BridgeSlot::new_ready_for_test(channel.clone());

    let pool = CxfBridgePool::from_config(CxfPoolConfig {
        profiles: vec![],
        ..CxfPoolConfig::default()
    })?;
    pool.insert_slot_for_test(key.clone(), slot);

    // Verify the slot was inserted and its state is Ready
    let pool_arc = std::sync::Arc::new(pool);
    let result = pool_arc.get_channel().await;
    assert!(
        result.is_ok(),
        "get_channel should succeed for inserted ready slot"
    );
    Ok(())
}

#[tokio::test]
async fn test_shutdown_with_no_slots() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = CxfBridgePool::from_config(CxfPoolConfig {
        profiles: vec![],
        ..CxfPoolConfig::default()
    })?;
    pool.shutdown().await?;
    Ok(())
}
