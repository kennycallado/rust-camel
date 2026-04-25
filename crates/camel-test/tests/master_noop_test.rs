//! Regression tests for camel-master component.
//!
//! Tests the NoopPlatformService default wiring to ensure multiple master routes
//! with the same lock name can coexist.

use std::sync::Arc;
use std::time::Duration;

use camel_api::NoopPlatformService;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::ComponentBundle;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use camel_master::MasterBundle;

/// Two master routes with the same lock name should both start when using
/// NoopPlatformService (the default). This verifies the fix for the bug where
/// the old NoopLeaderElector's `AlreadyStarted` prevented the second route.
#[tokio::test]
async fn two_master_routes_same_lock_both_start_with_noop() {
    let platform = Arc::new(NoopPlatformService::default());
    let mut ctx = CamelContext::builder()
        .platform_service(platform)
        .build()
        .await
        .unwrap();

    MasterBundle::from_toml(toml::Value::Table(toml::map::Map::new()))
        .unwrap()
        .register_all(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let r1 = RouteBuilder::from("master:mylock:timer:tick?period=1000")
        .route_id("master-route-1")
        .to("log:info?showBody=false")
        .build()
        .unwrap();
    let r2 = RouteBuilder::from("master:mylock:timer:tick?period=1000")
        .route_id("master-route-2")
        .to("log:info?showBody=false")
        .build()
        .unwrap();

    ctx.add_route_definition(r1).await.unwrap();
    ctx.add_route_definition(r2).await.unwrap();
    ctx.start().await.unwrap();

    // Both routes should be running — give them time to tick
    tokio::time::sleep(Duration::from_secs(2)).await;
    ctx.stop().await.unwrap();
}

/// Two master routes with different lock names should both start independently.
#[tokio::test]
async fn two_master_routes_different_locks_both_start_with_noop() {
    let platform = Arc::new(NoopPlatformService::default());
    let mut ctx = CamelContext::builder()
        .platform_service(platform)
        .build()
        .await
        .unwrap();

    MasterBundle::from_toml(toml::Value::Table(toml::map::Map::new()))
        .unwrap()
        .register_all(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let r1 = RouteBuilder::from("master:orders:timer:tick?period=1000")
        .route_id("master-orders")
        .to("log:info?showBody=false")
        .build()
        .unwrap();
    let r2 = RouteBuilder::from("master:billing:timer:tick?period=1000")
        .route_id("master-billing")
        .to("log:info?showBody=false")
        .build()
        .unwrap();

    ctx.add_route_definition(r1).await.unwrap();
    ctx.add_route_definition(r2).await.unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;
    ctx.stop().await.unwrap();
}
