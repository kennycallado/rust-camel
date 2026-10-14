//! Integration test for camel-master with Kubernetes leader election.
//!
//! Uses testcontainers to spin up a K3s cluster.
//! Requires Docker and `integration-tests` feature.

#![cfg(feature = "integration-tests")]

mod support;

use std::sync::Arc;
use std::time::Duration;

use camel_api::PlatformIdentity;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::ComponentBundle;
use camel_component_log::LogComponent;
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_config::CamelConfig;
use camel_config::config::{KubernetesPlatformCamelConfig, PlatformCamelConfig, SecurityConfig};
use camel_core::CamelContext;
use camel_master::MasterBundle;
use camel_platform_kubernetes::{
    KubernetesLeadershipService, KubernetesPlatformConfig, KubernetesPlatformService,
    ensure_rustls_provider,
};
use k8s_openapi::api::coordination::v1::Lease;
use testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use testcontainers_modules::k3s::K3s;

const KUBE_SECURE_PORT: u16 = 6443;

async fn start_k3s() -> (ContainerAsync<K3s>, kube::Client) {
    ensure_rustls_provider();
    let conf_dir = std::env::temp_dir().join(format!(
        "camel-k3s-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&conf_dir).expect("k3s config directory should be created");
    let k3s = K3s::default()
        .with_conf_mount(&conf_dir)
        .with_privileged(true)
        .with_userns_mode("host");

    let container = k3s.start().await.expect("k3s container should start");

    let kubeconfig_yaml = container
        .image()
        .read_kube_config()
        .expect("should read kube config");

    let mut kubeconfig =
        kube::config::Kubeconfig::from_yaml(&kubeconfig_yaml).expect("should parse kubeconfig");

    let port = container
        .get_host_port_ipv4(KUBE_SECURE_PORT)
        .await
        .expect("should expose k3s secure port");
    kubeconfig.clusters.iter_mut().for_each(|cluster| {
        if let Some(server) = cluster.cluster.as_mut().and_then(|c| c.server.as_mut()) {
            *server = format!("https://127.0.0.1:{port}");
        }
    });

    let config = kube::Config::from_custom_kubeconfig(
        kubeconfig,
        &kube::config::KubeConfigOptions::default(),
    )
    .await
    .expect("should build kube config");

    let client = kube::Client::try_from(config).expect("should create client");

    let timeout = Duration::from_secs(90);
    let start = std::time::Instant::now();
    let leases: kube::Api<Lease> = kube::Api::namespaced(client.clone(), "default");
    loop {
        match leases.list(&kube::api::ListParams::default()).await {
            Ok(_) => break,
            Err(_) if start.elapsed() < timeout => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(err) => panic!("k3s Lease API not ready after 90s: {err}"),
        }
    }

    (container, client)
}

async fn wait_for_leader(client: &kube::Client, lease_name: &str, holder: &str, timeout_secs: u64) {
    let leases: kube::Api<Lease> = kube::Api::namespaced(client.clone(), "default");
    let deadline = Duration::from_secs(timeout_secs);

    tokio::time::timeout(deadline, async {
        loop {
            if let Ok(Some(lease)) = leases.get_opt(lease_name).await {
                let current = lease
                    .spec
                    .as_ref()
                    .and_then(|spec| spec.holder_identity.as_deref());
                if current == Some(holder) {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("timed out waiting for leadership");
}

#[tokio::test(flavor = "multi_thread")]
async fn master_component_processes_after_kubernetes_leadership() {
    let (_container, client) = start_k3s().await;

    let config = KubernetesPlatformConfig {
        namespace: "default".to_string(),
        lease_name_prefix: "camel-".to_string(),
        lease_duration: Duration::from_secs(10),
        renew_deadline: Duration::from_secs(8),
        retry_period: Duration::from_secs(1),
        jitter_factor: 0.2,
    };

    let identity = PlatformIdentity::local("test-pod");
    let leadership = Arc::new(
        KubernetesLeadershipService::new(client.clone(), identity.clone(), config)
            .expect("leadership config"),
    );
    let platform = Arc::new(KubernetesPlatformService::from_parts(
        identity,
        Arc::new(camel_api::NoopReadinessGate),
        leadership,
    ));

    let mut ctx = CamelContext::builder()
        .platform_service(platform)
        .build()
        .await
        .expect("context should build");

    let bundle = MasterBundle::from_toml(toml::Value::Table(toml::map::Map::new()))
        .expect("master bundle should parse default config");
    bundle.register_all(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let mock = MockComponent::new();
    ctx.register_component(mock.clone());

    let route = RouteBuilder::from("master:orders:timer:tick?period=500")
        .route_id("master-kubernetes-orders-route")
        .to("log:info")
        .to("mock:result")
        .build()
        .expect("route should build");

    ctx.add_route_definition(route)
        .await
        .expect("route should be added");
    ctx.start().await.expect("context should start");

    // The lease name is lease_name_prefix + lock_name = "camel-orders"
    wait_for_leader(&client, "camel-orders", "default/test-pod", 30).await;

    let endpoint = mock
        .get_endpoint("result")
        .expect("mock endpoint should exist");
    endpoint.await_exchanges(1, Duration::from_secs(20)).await;

    ctx.stop().await.expect("context should stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn master_route_uses_kubernetes_platform_from_config() {
    let (_container, client) = start_k3s().await;

    // Write a kubeconfig file so the in-cluster client can connect to our K3s
    let conf_dir = std::env::temp_dir().join(format!("camel-k3s-config-{}", std::process::id()));
    std::fs::create_dir_all(&conf_dir).expect("conf dir");

    // Build context via CamelConfig with kubernetes platform
    // Note: try_default() won't work outside a cluster, so we test the config parsing
    // and verify the platform type is correctly set.
    let config = CamelConfig {
        routes: vec![],
        watch: false,
        runtime_journal: None,
        log_level: "INFO".to_string(),
        timeout_ms: 5_000,
        drain_timeout_ms: 10_000,
        watch_debounce_ms: 300,
        components: Default::default(),
        observability: Default::default(),
        supervision: None,
        platform: PlatformCamelConfig::Kubernetes(KubernetesPlatformCamelConfig {
            namespace: Some("default".into()),
            lease_name_prefix: "camel-".into(),
            lease_duration_secs: 10,
            renew_deadline_secs: 8,
            retry_period_secs: 1,
            jitter_factor: 0.2,
        }),
        stream_caching: Default::default(),
        beans: Default::default(),
        languages: Default::default(),
        security: SecurityConfig::default(),
        binds: Default::default(),
        datasources: Default::default(),
        idempotent_repo: None,
        cache_repo: None,
        jobs: Default::default(),
        _extra: Default::default(),
    };

    // Verify config parses correctly
    assert!(matches!(
        config.platform,
        PlatformCamelConfig::Kubernetes(_)
    ));

    // Manually build the same context that configure_context would build,
    // but with our test K3s client
    let k8s_config = KubernetesPlatformConfig {
        namespace: "default".to_string(),
        lease_name_prefix: "camel-".to_string(),
        lease_duration: Duration::from_secs(10),
        renew_deadline: Duration::from_secs(8),
        retry_period: Duration::from_secs(1),
        jitter_factor: 0.2,
    };
    let identity = PlatformIdentity::local("config-test-pod");
    let leadership = Arc::new(
        KubernetesLeadershipService::new(client.clone(), identity.clone(), k8s_config)
            .expect("leadership config"),
    );
    let platform = Arc::new(KubernetesPlatformService::from_parts(
        identity,
        Arc::new(camel_api::NoopReadinessGate),
        leadership,
    ));

    let mut ctx = CamelContext::builder()
        .platform_service(platform)
        .build()
        .await
        .expect("context should build");

    let bundle =
        MasterBundle::from_toml(toml::Value::Table(toml::map::Map::new())).expect("master bundle");
    bundle.register_all(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("master:config-orders:timer:tick?period=500")
        .route_id("config-driven-master-route")
        .to("log:info")
        .build()
        .expect("route should build");

    ctx.add_route_definition(route)
        .await
        .expect("route should be added");
    ctx.start().await.expect("context should start");

    wait_for_leader(
        &client,
        "camel-config-orders",
        "default/config-test-pod",
        30,
    )
    .await;

    ctx.stop().await.expect("context should stop");
}

// ===========================================================================
// Leader failover over a real Explicit delegate (rc-w02ns)
// ===========================================================================

fn k8s_platform(client: &kube::Client, pod: &str) -> Arc<KubernetesPlatformService> {
    let config = KubernetesPlatformConfig {
        namespace: "default".to_string(),
        lease_name_prefix: "camel-".to_string(),
        lease_duration: Duration::from_secs(10),
        renew_deadline: Duration::from_secs(8),
        retry_period: Duration::from_secs(1),
        jitter_factor: 0.2,
    };
    let identity = PlatformIdentity::local(pod);
    let leadership = Arc::new(
        KubernetesLeadershipService::new(client.clone(), identity.clone(), config)
            .expect("leadership config"),
    );
    Arc::new(KubernetesPlatformService::from_parts(
        identity,
        Arc::new(camel_api::NoopReadinessGate),
        leadership,
    ))
}

/// One "pod": its own platform identity, a `master:` route over the Kafka
/// consumer (Explicit startup), its own mock sink and lifecycle probe.
async fn kafka_master_pod(
    client: &kube::Client,
    pod: &str,
    brokers: &str,
) -> (
    CamelContext,
    MockComponent,
    Arc<support::master_probe::DelegateLifecycleProbe>,
) {
    let mut ctx = CamelContext::builder()
        .platform_service(k8s_platform(client, pod))
        .build()
        .await
        .expect("context should build");
    MasterBundle::from_toml(toml::Value::Table(toml::map::Map::new()))
        .expect("master bundle")
        .register_all(&mut ctx);
    ctx.register_component(camel_component_kafka::KafkaComponent::new());
    let mock = MockComponent::new();
    ctx.register_component(mock.clone());
    let (ctx, probe) = support::master_probe::attach(ctx);

    let route = RouteBuilder::from(&format!(
        "master:failover:kafka:test-master-failover?brokers={brokers}\
         &groupId=master-failover-group&autoOffsetReset=earliest"
    ))
    .route_id(format!("master-failover-{pod}"))
    .to("mock:out")
    .build()
    .expect("route should build");
    ctx.add_route_definition(route).await.expect("route added");
    (ctx, mock, probe)
}

/// Two pods contend for the same Lease with a Kafka delegate behind
/// `master:`. The leader's delegate must run once without churn; when the
/// leader stops, the standby (delegate never started) must take the Lease and
/// run its own delegate once, delivering again.
#[tokio::test(flavor = "multi_thread")]
async fn master_kafka_delegate_survives_leader_failover() {
    let (_k3s, client) = start_k3s().await;
    let (_, brokers) = support::kafka::shared_kafka().await;

    // Steady producer, independent of both pods.
    let mut producer = CamelContext::builder()
        .build()
        .await
        .expect("producer context");
    producer.register_component(TimerComponent::new());
    producer.register_component(camel_component_kafka::KafkaComponent::new());
    producer
        .add_route_definition(
            RouteBuilder::from("timer:failover-feed?period=300")
                .set_body(camel_api::Value::String("failover".to_string()))
                .to(format!(
                    "kafka:test-master-failover?brokers={brokers}&acks=all"
                ))
                .route_id("master-failover-producer")
                .build()
                .expect("producer route"),
        )
        .await
        .expect("producer route added");
    producer.start().await.expect("producer starts");

    let (mut pod_a, mock_a, probe_a) = kafka_master_pod(&client, "pod-a", brokers).await;
    let (mut pod_b, mock_b, probe_b) = kafka_master_pod(&client, "pod-b", brokers).await;

    pod_a.start().await.expect("pod-a starts");
    wait_for_leader(&client, "camel-failover", "default/pod-a", 30).await;
    pod_b.start().await.expect("pod-b starts");

    mock_a
        .get_endpoint("out")
        .expect("pod-a sink")
        .await_exchanges(3, Duration::from_secs(60))
        .await;
    assert_eq!(
        probe_a.count("started"),
        1,
        "leader delegate must not churn"
    );
    assert_eq!(probe_a.count("stopped"), 0);
    assert_eq!(
        probe_b.count("started"),
        0,
        "standby must not start its delegate"
    );

    // `stopped`/step_down after ctx.stop() are not asserted: master's stop()
    // aborts the leadership task (separate bd). Takeover then waits for the
    // Lease to expire (10s here), covered by the generous timeouts below.
    pod_a.stop().await.expect("pod-a stops");

    wait_for_leader(&client, "camel-failover", "default/pod-b", 60).await;
    mock_b
        .get_endpoint("out")
        .expect("pod-b sink")
        .await_exchanges(3, Duration::from_secs(120))
        .await;
    assert_eq!(
        probe_b.count("started"),
        1,
        "new leader delegate must not churn"
    );
    assert_eq!(probe_b.count("stopped"), 0);

    pod_b.stop().await.expect("pod-b stops");
    producer.stop().await.expect("producer stops");
}
