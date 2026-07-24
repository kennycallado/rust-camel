#![allow(dead_code)]

use std::borrow::Cow;

use testcontainers::{
    ContainerAsync, Image,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::sync::OnceCell;

const SURREALDB_PORT: u16 = 8000;

#[derive(Debug)]
struct SurrealDbImage;

impl Image for SurrealDbImage {
    fn name(&self) -> &str {
        "surrealdb/surrealdb"
    }

    fn tag(&self) -> &str {
        "v3.1.4"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("Started web server")]
    }

    fn cmd(&self) -> impl IntoIterator<Item = impl Into<Cow<'_, str>>> {
        vec![
            "start".to_string(),
            "--user".to_string(),
            "root".to_string(),
            "--pass".to_string(),
            "root".to_string(),
            "--bind".to_string(),
            "0.0.0.0:8000".to_string(),
        ]
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        static PORTS: [ContainerPort; 1] = [ContainerPort::Tcp(SURREALDB_PORT)];
        &PORTS
    }
}

static SURREALDB: OnceCell<(ContainerAsync<SurrealDbImage>, String)> = OnceCell::const_new();

/// Shared SurrealDB container, started once and reused across all tests.
/// Returns the WebSocket endpoint (e.g. `ws://127.0.0.1:<mapped>`).
pub async fn shared_surrealdb() -> &'static str {
    SURREALDB
        .get_or_init(|| async {
            super::init_tracing();
            super::install_crypto_provider();

            let container = SurrealDbImage
                .start()
                .await
                .expect("SurrealDB container failed to start");
            let port = container
                .get_host_port_ipv4(SURREALDB_PORT)
                .await
                .expect("SurrealDB port not available");
            let endpoint = format!("ws://127.0.0.1:{port}");
            eprintln!("SurrealDB endpoint: {endpoint}");
            (container, endpoint)
        })
        .await
        .1
        .as_str()
}
