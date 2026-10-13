#![allow(dead_code)]

use std::time::Duration;

use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::net::TcpStream;
use tokio::sync::OnceCell;
use tokio::time::{Instant, sleep};

pub const ARTEMIS_PORT: u16 = 61616;
/// Explicit credentials for the dedicated Artemis test fixture (mandatory
/// auth). The Phase-5 crash proof passes these to both the probe and the
/// producer bridge; it never relies on ambient ANONYMOUS_LOGIN.
pub const ARTEMIS_USER: &str = "artemis";
pub const ARTEMIS_PASSWORD: &str = "artemis";

async fn wait_for_broker_port(port: u16, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        // Per-attempt connect bound (lintwiden D4.3): a timed-out attempt is
        // treated like any failed connect — the loop deadline still governs
        // the overall wait.
        let attempt = tokio::time::timeout(
            Duration::from_secs(5),
            TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        .unwrap_or_else(|_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "tcp connect attempt exceeded 5s",
            ))
        });
        match attempt {
            Ok(stream) => {
                drop(stream);
                return Ok(());
            }
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "Artemis broker port {port} did not become reachable within {:?}: {err}",
                        timeout
                    ));
                }
                sleep(Duration::from_millis(250)).await;
            }
        }
    }
}

/// Shared Artemis container — started once and reused across all tests.
/// Uses `ANONYMOUS_LOGIN=true` for broad compatibility.
static ARTEMIS: OnceCell<(ContainerAsync<GenericImage>, String)> = OnceCell::const_new();

/// Shared Artemis container with **mandatory auth** (no ANONYMOUS_LOGIN).
/// Used to test that the bridge authenticates correctly and that the health
/// check does not time out under real auth conditions.
static ARTEMIS_AUTH: OnceCell<(ContainerAsync<GenericImage>, String)> = OnceCell::const_new();

/// Artemis startup is relatively heavy and can be flaky when two containers
/// (anonymous + mandatory-auth variants) initialize at the same time.
/// Serialize startup to reduce CI nondeterminism while keeping test execution
/// itself parallel.
static ARTEMIS_STARTUP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Return a reference to the shared Artemis container and its broker URL.
pub async fn shared_artemis() -> &'static (ContainerAsync<GenericImage>, String) {
    ARTEMIS
        .get_or_init(|| async {
            let _startup_guard = ARTEMIS_STARTUP_LOCK.lock().await;
            let image = GenericImage::new("apache/activemq-artemis", "2.36.0-alpine")
                .with_exposed_port(ContainerPort::Tcp(ARTEMIS_PORT))
                .with_wait_for(WaitFor::message_on_stdout("AMQ221020"))
                .with_env_var("ARTEMIS_USER", ARTEMIS_USER)
                .with_env_var("ARTEMIS_PASSWORD", ARTEMIS_PASSWORD)
                .with_env_var("ANONYMOUS_LOGIN", "true")
                .with_startup_timeout(Duration::from_secs(120));

            let container = image
                .start()
                .await
                .expect("Artemis container failed to start");
            let port = container
                .get_host_port_ipv4(ARTEMIS_PORT)
                .await
                .expect("Artemis port not available");
            wait_for_broker_port(port, Duration::from_secs(30))
                .await
                .expect("Artemis broker did not become reachable");

            let broker_url = format!("tcp://127.0.0.1:{port}");
            eprintln!("Artemis ready at: {broker_url}");
            (container, broker_url)
        })
        .await
}

/// Return a reference to an Artemis container with **mandatory auth**.
/// ANONYMOUS_LOGIN is intentionally absent — the broker rejects unauthenticated
/// connections. Credentials: artemis / artemis.
pub async fn shared_artemis_auth() -> &'static (ContainerAsync<GenericImage>, String) {
    ARTEMIS_AUTH
        .get_or_init(|| async {
            let _startup_guard = ARTEMIS_STARTUP_LOCK.lock().await;
            let image = GenericImage::new("apache/activemq-artemis", "2.36.0-alpine")
                .with_exposed_port(ContainerPort::Tcp(ARTEMIS_PORT))
                .with_wait_for(WaitFor::message_on_stdout("AMQ221020"))
                .with_env_var("ARTEMIS_USER", ARTEMIS_USER)
                .with_env_var("ARTEMIS_PASSWORD", ARTEMIS_PASSWORD)
                // ANONYMOUS_LOGIN intentionally not set → mandatory auth
                .with_startup_timeout(Duration::from_secs(120));

            let container = image
                .start()
                .await
                .expect("Artemis auth container failed to start");
            let port = container
                .get_host_port_ipv4(ARTEMIS_PORT)
                .await
                .expect("Artemis auth port not available");
            wait_for_broker_port(port, Duration::from_secs(30))
                .await
                .expect("Artemis auth broker did not become reachable");

            let broker_url = format!("tcp://127.0.0.1:{port}");
            eprintln!("Artemis (auth) ready at: {broker_url}");
            (container, broker_url)
        })
        .await
}

/// A dedicated Artemis broker (its own container, mandatory auth).
///
/// Mirrors [`super::activemq::ActiveMqBroker`]: tests that must assert exact
/// broker-side/destination state for a single queue use an isolated broker so a
/// shared container's other connections cannot perturb the observation. The
/// fixture requires authentication (`ARTEMIS_USER`/`ARTEMIS_PASSWORD`), so the
/// bridge must present these credentials explicitly — there is no
/// `ANONYMOUS_LOGIN` fallback.
pub struct ArtemisBroker {
    // Held only to keep the container alive for the test's lifetime.
    _container: ContainerAsync<GenericImage>,
    pub broker_url: String,
}

impl ArtemisBroker {
    /// Starts an isolated Artemis container and blocks until the broker port
    /// accepts connections.
    pub async fn start() -> Self {
        let image = GenericImage::new("apache/activemq-artemis", "2.36.0-alpine")
            .with_exposed_port(ContainerPort::Tcp(ARTEMIS_PORT))
            .with_wait_for(WaitFor::message_on_stdout("AMQ221020"))
            .with_env_var("ARTEMIS_USER", ARTEMIS_USER)
            .with_env_var("ARTEMIS_PASSWORD", ARTEMIS_PASSWORD)
            .with_startup_timeout(Duration::from_secs(120));

        let container = image
            .start()
            .await
            .expect("dedicated Artemis container failed to start");
        let port = container
            .get_host_port_ipv4(ARTEMIS_PORT)
            .await
            .expect("dedicated Artemis broker port not available");
        wait_for_broker_port(port, Duration::from_secs(30))
            .await
            .expect("dedicated Artemis broker did not become reachable");

        let broker_url = format!("tcp://127.0.0.1:{port}");
        eprintln!("dedicated Artemis ready at: {broker_url}");
        Self {
            _container: container,
            broker_url,
        }
    }
}
