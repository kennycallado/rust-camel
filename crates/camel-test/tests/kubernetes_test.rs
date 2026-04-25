//! Integration tests for Kubernetes platform leader election.
//!
//! Uses testcontainers to spin up a K3s cluster for testing.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

use std::time::Duration;

use camel_api::platform::{LeadershipEvent, PlatformIdentity};
use camel_platform_kubernetes::{KubernetesLeadershipService, KubernetesPlatformConfig};
use testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use testcontainers_modules::k3s::K3s;

/// Wait until the handle reports leadership or timeout.
async fn wait_for_leader(handle: &camel_api::platform::LeadershipHandle, timeout_secs: u64) {
    let deadline = Duration::from_secs(timeout_secs);
    tokio::time::timeout(deadline, async {
        loop {
            if handle.is_leader() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("timed out waiting for leadership");
}

/// Internal K3s API secure port.
const KUBE_SECURE_PORT: u16 = 6443;

/// Start a K3s container and return a configured kube client.
async fn start_k3s() -> (ContainerAsync<K3s>, kube::Client) {
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

    // Wait until both the core API and the Coordination/Leases API are ready.
    let timeout = Duration::from_secs(90);
    let start = std::time::Instant::now();
    let leases: kube::Api<k8s_openapi::api::coordination::v1::Lease> =
        kube::Api::namespaced(client.clone(), "default");
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

fn test_config() -> KubernetesPlatformConfig {
    KubernetesPlatformConfig {
        namespace: "default".to_string(),
        lease_name_prefix: "camel-".to_string(),
        lease_duration: Duration::from_secs(10),
        renew_deadline: Duration::from_secs(8),
        retry_period: Duration::from_secs(1),
        jitter_factor: 0.2,
    }
}

#[tokio::test]
async fn test_single_instance_becomes_leader() {
    let (_container, client) = start_k3s().await;

    let leadership =
        KubernetesLeadershipService::new(client, PlatformIdentity::local("pod-a"), test_config())?;
    let handle = leadership
        .start("single-leader")
        .await
        .expect("start should succeed");

    wait_for_leader(&handle, 30).await;
    assert!(handle.is_leader(), "single instance should become leader");

    let event = handle.events.borrow().clone();
    assert_eq!(event, Some(LeadershipEvent::StartedLeading));
}

#[tokio::test]
async fn test_duplicate_lock_reuses_cached_loop() {
    let (_container, client) = start_k3s().await;

    let leadership =
        KubernetesLeadershipService::new(client, PlatformIdentity::local("pod-a"), test_config())?;
    let first = leadership
        .start("dup-lock")
        .await
        .expect("first start should succeed");

    let second = leadership
        .start("dup-lock")
        .await
        .expect("duplicate lock should reuse existing loop");

    wait_for_leader(&first, 30).await;
    assert!(first.is_leader());
    assert!(second.is_leader());
}

#[tokio::test]
async fn test_step_down_releases_leadership() {
    let (_container, client) = start_k3s().await;

    let leadership =
        KubernetesLeadershipService::new(client, PlatformIdentity::local("pod-a"), test_config())?;
    let handle = leadership
        .start("step-down")
        .await
        .expect("start should succeed");

    wait_for_leader(&handle, 30).await;
    assert!(handle.is_leader());

    let result = tokio::time::timeout(Duration::from_secs(5), handle.step_down()).await;
    assert!(result.is_ok(), "step_down should not hang");
}

#[tokio::test]
async fn test_two_instances_only_one_leads() {
    let (_container, client) = start_k3s().await;

    let leadership_a = KubernetesLeadershipService::new(
        client.clone(),
        PlatformIdentity::local("pod-a"),
        test_config(),
    )?;
    let leadership_b = KubernetesLeadershipService::new(
        client.clone(),
        PlatformIdentity::local("pod-b"),
        test_config(),
    )?;

    let mut handle_a = Some(
        leadership_a
            .start("contention")
            .await
            .expect("start A should succeed"),
    )?;
    let mut handle_b = Some(
        leadership_b
            .start("contention")
            .await
            .expect("start B should succeed"),
    )?;

    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let a_leads = handle_a.as_ref().is_some_and(|h| h.is_leader());
            let b_leads = handle_b.as_ref().is_some_and(|h| h.is_leader());
            if a_leads || b_leads {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("one instance should become leader within 30s");

    let a_leads = handle_a.as_ref().is_some_and(|h| h.is_leader());
    let b_leads = handle_b.as_ref().is_some_and(|h| h.is_leader());
    assert_ne!(
        a_leads, b_leads,
        "exactly one instance must be leader at a time"
    )?;

    let stepped_down_a = if a_leads {
        let handle = handle_a
            .take()
            .expect("handle A should exist when A is leader");
        let result = tokio::time::timeout(Duration::from_secs(5), handle.step_down()).await;
        assert!(
            result.is_ok(),
            "step_down for current leader A should not hang"
        )?;
        true
    } else {
        let handle = handle_b
            .take()
            .expect("handle B should exist when B is leader");
        let result = tokio::time::timeout(Duration::from_secs(5), handle.step_down()).await;
        assert!(
            result.is_ok(),
            "step_down for current leader B should not hang"
        )?;
        false
    };

    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let a_now = handle_a.as_ref().is_some_and(|h| h.is_leader());
            let b_now = handle_b.as_ref().is_some_and(|h| h.is_leader());
            if a_now || b_now {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("remaining instance should take leadership after failover");

    let leader_a_after = if stepped_down_a {
        false
    } else {
        handle_a.as_ref().is_some_and(|h| h.is_leader())
    };
    let leader_b_after = if stepped_down_a {
        handle_b.as_ref().is_some_and(|h| h.is_leader())
    } else {
        false
    };
    assert_ne!(
        leader_a_after, leader_b_after,
        "exactly one instance must be leader after failover"
    )?;
}
