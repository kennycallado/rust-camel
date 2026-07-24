#![allow(dead_code)]

use std::time::Duration;

use testcontainers::{ContainerAsync, runners::AsyncRunner};
use testcontainers_modules::kafka::apache;
use tokio::sync::OnceCell;

static KAFKA: OnceCell<(ContainerAsync<apache::Kafka>, String)> = OnceCell::const_new();

/// Shared Kafka container (KRaft, no Zookeeper), started once and reused across
/// all tests in the process. Returns the bootstrap brokers address
/// (e.g. `127.0.0.1:<mapped>`).
pub async fn shared_kafka() -> &'static (ContainerAsync<apache::Kafka>, String) {
    KAFKA
        .get_or_init(|| async {
            super::init_tracing();
            super::install_crypto_provider();

            let container = apache::Kafka::default()
                .start()
                .await
                .expect("Kafka container failed to start");
            let port = container
                .get_host_port_ipv4(apache::KAFKA_PORT)
                .await
                .expect("Kafka port not available");
            let brokers = format!("127.0.0.1:{port}");

            // Wait until the broker accepts producer round-trips — the
            // testcontainers readiness check only waits for KRaft log lines,
            // which fire before the broker can accept client traffic.
            super::wait::wait_until(
                "kafka broker readiness",
                Duration::from_secs(60),
                Duration::from_millis(750),
                || {
                    let brokers = brokers.clone();
                    async move { kafka_probe(&brokers).await }
                },
            )
            .await
            .expect("Kafka broker did not become ready");

            eprintln!("Kafka ready: {brokers}");
            (container, brokers)
        })
        .await
}

/// Lightweight readiness probe: attempts a TCP connect to the broker port.
async fn kafka_probe(brokers: &str) -> Result<bool, String> {
    tokio::net::TcpStream::connect(brokers)
        .await
        .map(|_| true)
        .map_err(|e| format!("tcp connect to {brokers} failed: {e}"))
}
