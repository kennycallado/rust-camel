#![cfg(feature = "integration-tests")]

use std::time::Duration;

use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::sync::OnceCell;

pub const ARTEMIS_PORT: u16 = 61616;

/// Shared Artemis container — started once and reused across all tests.
static ARTEMIS: OnceCell<(ContainerAsync<GenericImage>, String)> = OnceCell::const_new();

/// Return a reference to the shared Artemis container and its broker URL.
pub async fn shared_artemis() -> &'static (ContainerAsync<GenericImage>, String) {
    ARTEMIS
        .get_or_init(|| async {
            let image = GenericImage::new("apache/activemq-artemis", "latest-alpine")
                .with_exposed_port(ContainerPort::Tcp(ARTEMIS_PORT))
                .with_wait_for(WaitFor::message_on_stdout("AMQ221020: Started EPOLL Acceptor at 0.0.0.0:61616"))
                .with_env_var("ARTEMIS_USER", "artemis")
                .with_env_var("ARTEMIS_PASSWORD", "artemis")
                .with_startup_timeout(Duration::from_secs(120));

            let container = image.start().await.expect("Artemis container failed to start");
            let port = container
                .get_host_port_ipv4(ARTEMIS_PORT)
                .await
                .expect("Artemis port not available");

            let broker_url = format!("tcp://127.0.0.1:{port}");
            eprintln!("Artemis ready at: {broker_url}");
            (container, broker_url)
        })
        .await
}
