#![allow(dead_code)]

use std::time::Duration;

use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::sync::OnceCell;

pub const ACTIVEMQ_PORT: u16 = 61616;

/// Shared ActiveMQ container — started once and reused across all tests.
/// The `String` is the broker URL (e.g. `tcp://127.0.0.1:<mapped-port>`).
static ACTIVEMQ: OnceCell<(ContainerAsync<GenericImage>, String)> = OnceCell::const_new();

/// Return a reference to the shared ActiveMQ container and its broker URL.
/// The container is started lazily on the first call and kept alive for the
/// lifetime of the test process.
pub async fn shared_activemq() -> &'static (ContainerAsync<GenericImage>, String) {
    ACTIVEMQ
        .get_or_init(|| async {
            let image = GenericImage::new("apache/activemq-classic", "5.18.3")
                .with_exposed_port(ContainerPort::Tcp(ACTIVEMQ_PORT))
                .with_wait_for(WaitFor::message_on_stdout(
                    "Listening for connections at: tcp://",
                ))
                .with_startup_timeout(Duration::from_secs(120));

            let container = image
                .start()
                .await
                .expect("ActiveMQ container failed to start");
            let port = container
                .get_host_port_ipv4(ACTIVEMQ_PORT)
                .await
                .expect("ActiveMQ port not available");

            let broker_url = format!("tcp://127.0.0.1:{port}");
            eprintln!("ActiveMQ ready at: {broker_url}");
            (container, broker_url)
        })
        .await
}
