#![allow(dead_code)]

use std::time::Duration;

use reqwest::Client;
use serde_json::{Value, json};
use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::sync::OnceCell;

use super::wait::wait_until;

/// OpenWire broker port.
pub const ACTIVEMQ_PORT: u16 = 61616;
/// Jetty web console / Jolokia port.
pub const ACTIVEMQ_WEB_PORT: u16 = 8161;

const ACTIVEMQ_USER: &str = "admin";
const ACTIVEMQ_PASSWORD: &str = "admin";

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

/// A dedicated ActiveMQ Classic broker (its own container) plus a Jolokia
/// client for its web console.
///
/// Tests that must assert broker-side state — a queue's `EnqueueCount` delta or
/// the set of live connection client IDs — use a dedicated broker rather than
/// [`shared_activemq`], because the shared broker also carries the shared
/// bridge pool and any other parallel test's connections. Counting those would
/// be racy; an isolated broker makes the counts exact.
pub struct ActiveMqBroker {
    // Held only to keep the container alive for the test's lifetime.
    _container: ContainerAsync<GenericImage>,
    client: Client,
    pub broker_url: String,
    pub jolokia_url: String,
}

impl ActiveMqBroker {
    /// Starts an isolated ActiveMQ Classic container with the web console
    /// exposed, then blocks until Jolokia answers.
    pub async fn start() -> Self {
        let image = GenericImage::new("apache/activemq-classic", "5.18.3")
            .with_exposed_port(ContainerPort::Tcp(ACTIVEMQ_PORT))
            .with_exposed_port(ContainerPort::Tcp(ACTIVEMQ_WEB_PORT))
            .with_wait_for(WaitFor::message_on_stdout(
                "Listening for connections at: tcp://",
            ))
            .with_startup_timeout(Duration::from_secs(120));

        let container = image
            .start()
            .await
            .expect("dedicated ActiveMQ container failed to start");
        let broker_port = container
            .get_host_port_ipv4(ACTIVEMQ_PORT)
            .await
            .expect("dedicated ActiveMQ broker port not available");
        let web_port = container
            .get_host_port_ipv4(ACTIVEMQ_WEB_PORT)
            .await
            .expect("dedicated ActiveMQ web console port not available");

        let broker = Self {
            _container: container,
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("build reqwest client for Jolokia"),
            broker_url: format!("tcp://127.0.0.1:{broker_port}"),
            jolokia_url: format!("http://127.0.0.1:{web_port}/api/jolokia/"),
        };
        broker.wait_ready().await;
        eprintln!(
            "dedicated ActiveMQ ready at: {} (Jolokia {})",
            broker.broker_url, broker.jolokia_url
        );
        broker
    }

    /// Blocks until the web console's Jolokia agent answers.
    async fn wait_ready(&self) {
        let client = self.client.clone();
        let url = self.jolokia_url.clone();
        wait_until(
            "ActiveMQ Jolokia web console",
            Duration::from_secs(120),
            Duration::from_millis(300),
            move || {
                let client = client.clone();
                let url = url.clone();
                async move {
                    match jolokia_request(&client, &url, &json!({ "type": "version" })).await {
                        Ok(v) if v.get("status").and_then(Value::as_u64) == Some(200) => Ok(true),
                        Ok(v) => Err(format!("Jolokia not ready yet: {v}")),
                        Err(e) => Err(e),
                    }
                }
            },
        )
        .await
        .expect("ActiveMQ Jolokia web console did not become ready");
    }

    async fn jolokia(&self, request: &Value) -> Result<Value, String> {
        jolokia_request(&self.client, &self.jolokia_url, request).await
    }

    /// Actual broker-side `EnqueueCount` for `queue`. Returns 0 when the queue
    /// MBean does not exist yet (the queue never received a message).
    pub async fn enqueue_count(&self, queue: &str) -> Result<u64, String> {
        let mbean = format!(
            "org.apache.activemq:type=Broker,brokerName=localhost,\
             destinationType=Queue,destinationName={queue}"
        );
        let response = self
            .jolokia(&json!({
                "type": "read",
                "mbean": mbean,
                "attribute": ["EnqueueCount"],
            }))
            .await?;

        if response.get("status").and_then(Value::as_u64) != Some(200) {
            let not_found = response
                .get("error_type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.contains("InstanceNotFound"));
            if not_found {
                return Ok(0);
            }
            return Err(format!("Jolokia EnqueueCount read failed: {response}"));
        }
        response["value"]["EnqueueCount"]
            .as_u64()
            .ok_or_else(|| format!("EnqueueCount missing in Jolokia response: {response}"))
    }

    /// Client IDs of the broker's live connections (`connectionViewType=clientId`).
    /// Two processes sharing one frozen client ID collapse into a single entry
    /// here, so a distinct set is direct broker-side evidence of coexistence.
    pub async fn client_ids(&self) -> Result<Vec<String>, String> {
        let response = self
            .jolokia(&json!({
                "type": "search",
                "mbean": "org.apache.activemq:type=Broker,brokerName=localhost,\
                          connectionViewType=clientId,*",
            }))
            .await?;
        if response.get("status").and_then(Value::as_u64) != Some(200) {
            return Err(format!("Jolokia connection search failed: {response}"));
        }

        let names = response["value"].as_array().cloned().unwrap_or_default();
        let mut ids = Vec::with_capacity(names.len());
        for name in names {
            let name = name
                .as_str()
                .ok_or_else(|| format!("unexpected connection MBean name: {name}"))?;
            let read = self
                .jolokia(&json!({
                    "type": "read",
                    "mbean": name,
                    "attribute": ["ClientId"],
                }))
                .await?;
            if let Some(id) = read["value"]["ClientId"].as_str() {
                ids.push(id.to_string());
            }
        }
        Ok(ids)
    }
}

/// Sends one Jolokia request as authenticated JSON and parses the JSON reply.
async fn jolokia_request(client: &Client, url: &str, request: &Value) -> Result<Value, String> {
    let body =
        serde_json::to_string(request).map_err(|e| format!("serialize Jolokia body: {e}"))?;
    let response = client
        .post(url)
        .basic_auth(ACTIVEMQ_USER, Some(ACTIVEMQ_PASSWORD))
        // ActiveMQ's Jolokia servlet rejects requests that omit an Origin
        // header ("Origin null is not allowed to call this agent"). The value
        // itself is not validated by the default policy.
        .header("Origin", "http://127.0.0.1")
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Jolokia request to {url} failed: {e}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("read Jolokia body from {url}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| {
        format!("Jolokia response from {url} was not JSON (HTTP {status}): {e}: {text}")
    })
}
