#![allow(dead_code)]

use std::time::Duration;

use testcontainers::{
    ContainerAsync, Image,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::sync::OnceCell;

pub const OPENSEARCH_HTTP_PORT: u16 = 9200;

#[derive(Debug, Clone)]
struct OpenSearchImage;

impl Image for OpenSearchImage {
    fn name(&self) -> &str {
        "opensearchproject/opensearch"
    }

    fn tag(&self) -> &str {
        "2.18.0"
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<
        Item = (
            impl Into<std::borrow::Cow<'_, str>>,
            impl Into<std::borrow::Cow<'_, str>>,
        ),
    > {
        vec![
            ("discovery.type".to_string(), "single-node".to_string()),
            (
                "OPENSEARCH_JAVA_OPTS".to_string(),
                "-Xms512m -Xmx512m".to_string(),
            ),
            ("DISABLE_SECURITY_PLUGIN".to_string(), "true".to_string()),
        ]
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &[ContainerPort::Tcp(OPENSEARCH_HTTP_PORT)]
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::Nothing]
    }
}

static OPENSEARCH: OnceCell<(ContainerAsync<OpenSearchImage>, String)> = OnceCell::const_new();

/// Shared OpenSearch container, started once and reused across all tests.
/// Returns the `host:port` (e.g. `127.0.0.1:<mapped>`) the component should
/// connect to.
pub async fn shared_opensearch() -> &'static str {
    OPENSEARCH
        .get_or_init(|| async {
            super::init_tracing();
            super::install_crypto_provider();

            let container = OpenSearchImage
                .start()
                .await
                .expect("OpenSearch container failed to start");
            let port = container
                .get_host_port_ipv4(OPENSEARCH_HTTP_PORT)
                .await
                .expect("OpenSearch port not available");
            let host_port = format!("127.0.0.1:{port}");

            // Wait for the HTTP endpoint to respond before returning — the
            // image uses WaitFor::Nothing so testcontainers returns as soon as
            // the container starts, before OpenSearch finishes booting.
            let client = reqwest::Client::new();
            super::wait::wait_until(
                "OpenSearch ready",
                Duration::from_secs(60),
                Duration::from_millis(500),
                || {
                    let client = client.clone();
                    let url = format!("http://{host_port}/");
                    async move {
                        let resp = client.get(&url).send().await;
                        match resp {
                            Ok(r) if r.status().is_success() => Ok(true),
                            Ok(r) => Err(format!("non-200 status: {}", r.status())),
                            Err(e) => Err(format!("connection failed: {e}")),
                        }
                    }
                },
            )
            .await
            .expect("OpenSearch did not become ready");

            eprintln!("OpenSearch ready at: {host_port}");
            (container, host_port)
        })
        .await
        .1
        .as_str()
}
