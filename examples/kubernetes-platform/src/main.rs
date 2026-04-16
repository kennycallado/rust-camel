use std::error::Error;
use std::time::Duration;

use camel_api::platform::{LeaderElector, LeadershipHandle, PlatformIdentity};
use camel_platform_kubernetes::{KubernetesLeaderElector, LeaderElectorConfig};
use testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use testcontainers_modules::k3s::K3s;

/// Internal K3s API secure port.
const KUBE_SECURE_PORT: u16 = 6443;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pod {
    Alpha,
    Beta,
}

impl Pod {
    fn name(self) -> &'static str {
        match self {
            Self::Alpha => "pod-alpha",
            Self::Beta => "pod-beta",
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    println!("[1/6] Starting local K3s cluster in Docker...");
    let (_container, client) = start_k3s().await;
    println!("      K3s API is ready");

    let config = LeaderElectorConfig {
        lease_name: "demo-leader".to_string(),
        namespace: "default".to_string(),
        lease_duration: Duration::from_secs(10),
        renew_deadline: Duration::from_secs(8),
        retry_period: Duration::from_secs(1),
    };

    println!("[2/6] Creating two electors for lease '{}'", config.lease_name);
    let elector_alpha = KubernetesLeaderElector::new(client.clone(), config.clone());
    let elector_beta = KubernetesLeaderElector::new(client.clone(), config.clone());

    println!("[3/6] Starting pod-alpha and pod-beta");
    let mut handle_alpha = Some(
        elector_alpha
            .start(PlatformIdentity::local(Pod::Alpha.name()))
            .await?,
    );
    let mut handle_beta = Some(
        elector_beta
            .start(PlatformIdentity::local(Pod::Beta.name()))
            .await?,
    );

    println!("[4/6] Waiting up to 30s for first leader...");
    let first_leader = wait_for_leader(&handle_alpha, &handle_beta, Duration::from_secs(30)).await?;
    println!("      ✅ Initial leader is {}", first_leader.name());

    tokio::time::sleep(Duration::from_secs(3)).await;

    println!("[5/6] Forcing leader step-down: {}", first_leader.name());
    match first_leader {
        Pod::Alpha => {
            let handle = handle_alpha.take().ok_or_else(|| {
                std::io::Error::other("pod-alpha handle missing before step-down")
            })?;
            handle.step_down().await?;
        }
        Pod::Beta => {
            let handle = handle_beta.take().ok_or_else(|| {
                std::io::Error::other("pod-beta handle missing before step-down")
            })?;
            handle.step_down().await?;
        }
    }

    println!("[6/6] Waiting up to 20s for failover...");
    let new_leader = wait_for_leader(&handle_alpha, &handle_beta, Duration::from_secs(20)).await?;
    println!("      ✅ New leader is {}", new_leader.name());
    println!("Demo complete.");

    cleanup_handle(handle_alpha).await;
    cleanup_handle(handle_beta).await;

    Ok(())
}

async fn cleanup_handle(handle: Option<LeadershipHandle>) {
    if let Some(handle) = handle {
        let _ = tokio::time::timeout(Duration::from_secs(5), handle.step_down()).await;
    }
}

async fn wait_for_leader(
    handle_alpha: &Option<LeadershipHandle>,
    handle_beta: &Option<LeadershipHandle>,
    timeout: Duration,
) -> Result<Pod, Box<dyn Error>> {
    let winner = tokio::time::timeout(timeout, async {
        loop {
            let alpha_is_leader = handle_alpha.as_ref().is_some_and(LeadershipHandle::is_leader);
            let beta_is_leader = handle_beta.as_ref().is_some_and(LeadershipHandle::is_leader);

            if alpha_is_leader {
                return Pod::Alpha;
            }
            if beta_is_leader {
                return Pod::Beta;
            }

            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| std::io::Error::other("timed out waiting for leader"))?;

    Ok(winner)
}

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
    // `apiserver_version()` can succeed before Leases are available, so we poll
    // an actual Lease list to confirm the coordination API group is healthy.
    let timeout = std::time::Duration::from_secs(90);
    let start = std::time::Instant::now();
    let leases: kube::Api<k8s_openapi::api::coordination::v1::Lease> =
        kube::Api::namespaced(client.clone(), "default");
    loop {
        match leases.list(&kube::api::ListParams::default()).await {
            Ok(_) => break,
            Err(_) if start.elapsed() < timeout => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            Err(err) => panic!("k3s Lease API not ready after 90s: {err}"),
        }
    }

    (container, client)
}
