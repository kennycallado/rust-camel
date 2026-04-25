//! Tests that Kubernetes platform service implementations can be wired into
//! `CamelContextBuilder` via the platform service API.

use std::sync::Arc;

use camel_api::platform::{LeadershipService, NoopReadinessGate};
use camel_core::context::CamelContextBuilder;
use camel_platform_kubernetes::{
    KubernetesLeadershipService, KubernetesPlatformConfig, KubernetesPlatformService,
};

fn install_rustls_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// Verify that the builder accepts a `KubernetesPlatformService` via `.platform_service(...)`.
#[tokio::test]
async fn test_wire_platform_service_into_context() {
    install_rustls_provider();

    let client = match kube::Client::try_default().await {
        Ok(client) => client,
        Err(_) => return,
    };

    let identity = camel_api::PlatformIdentity::local("wiring-test-node");
    let leadership = Arc::new(
        KubernetesLeadershipService::new(
            client,
            identity.clone(),
            KubernetesPlatformConfig::default(),
        )
        .expect("leadership config"),
    );
    let platform_service = Arc::new(KubernetesPlatformService::from_parts(
        identity,
        Arc::new(NoopReadinessGate),
        leadership,
    ));

    let ctx = CamelContextBuilder::new()
        .platform_service(platform_service)
        .build()
        .await
        .expect("context build should succeed");

    let _ = ctx.platform_service();
}

/// Verify multiple handles for the same lock can coexist.
#[tokio::test]
async fn test_same_lock_allows_multiple_handles() {
    install_rustls_provider();

    let client = match kube::Client::try_default().await {
        Ok(client) => client,
        Err(_) => return,
    };

    let leadership = KubernetesLeadershipService::new(
        client,
        camel_api::PlatformIdentity::local("multi-handle-node"),
        KubernetesPlatformConfig::default(),
    )
    .expect("leadership config");

    let first = leadership
        .start("orders")
        .await
        .expect("first handle should start");
    let second = leadership
        .start("orders")
        .await
        .expect("second handle for same lock should also start");

    first.step_down().await.expect("first should step down");
    second.step_down().await.expect("second should step down");
}

/// Verify multiple distinct locks can start concurrently.
#[tokio::test]
async fn test_multiple_distinct_locks_can_start() {
    install_rustls_provider();

    let client = match kube::Client::try_default().await {
        Ok(client) => client,
        Err(_) => return,
    };

    let leadership = KubernetesLeadershipService::new(
        client,
        camel_api::PlatformIdentity::local("multi-lock-node"),
        KubernetesPlatformConfig::default(),
    )
    .expect("leadership config");

    let lock_a = leadership
        .start("orders")
        .await
        .expect("first lock should start");
    let lock_b = leadership
        .start("payments")
        .await
        .expect("distinct lock should also start");

    lock_a
        .step_down()
        .await
        .expect("orders lock should step down");
    lock_b
        .step_down()
        .await
        .expect("payments lock should step down");
}

/// Verify new API can be built through `CamelContextBuilder`.
#[tokio::test]
async fn test_platform_service_try_default_compiles_with_builder() {
    install_rustls_provider();

    let platform_service =
        match KubernetesPlatformService::try_default(KubernetesPlatformConfig::default()).await {
            Ok(platform_service) => Arc::new(platform_service),
            Err(_) => return,
        };

    let ctx = CamelContextBuilder::new()
        .platform_service(platform_service)
        .build()
        .await
        .expect("context build should succeed");

    let _ = ctx.platform_service().identity();
}
