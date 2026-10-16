#![allow(dead_code)]

use std::collections::BTreeMap;
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

pub const ACTIVEMQ_USER: &str = "admin";
pub const ACTIVEMQ_PASSWORD: &str = "admin";

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

    /// Docker container id, for freezing the broker with `docker pause`.
    pub fn container_id(&self) -> &str {
        self._container.id()
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

    /// Jolokia request variant for the `browse()` path. A non-JSON reply on
    /// this path must not leak the response body: a browsed Queue reply can
    /// carry message property values. Only the HTTP status, the serde error
    /// class, and the line/column are reported.
    async fn jolokia_safe(&self, request: &Value) -> Result<Value, String> {
        jolokia_request_safe(&self.client, &self.jolokia_url, request).await
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

    /// Actual broker-side `ConsumerCount` for `queue`. Returns 0 when the
    /// queue MBean does not exist yet (nothing has consumed or been enqueued).
    /// Used as an explicit readiness barrier: a subscriber is registered on
    /// the broker before its processor can accept an exchange.
    pub async fn consumer_count(&self, queue: &str) -> Result<u64, String> {
        let mbean = format!(
            "org.apache.activemq:type=Broker,brokerName=localhost,\
             destinationType=Queue,destinationName={queue}"
        );
        let response = self
            .jolokia(&json!({
                "type": "read",
                "mbean": mbean,
                "attribute": ["ConsumerCount"],
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
            return Err(format!("Jolokia ConsumerCount read failed: {response}"));
        }
        response["value"]["ConsumerCount"]
            .as_u64()
            .ok_or_else(|| format!("ConsumerCount missing in Jolokia response: {response}"))
    }

    /// Actual broker-side `QueueSize` for `queue`: messages on the queue,
    /// including ones dispatched to a consumer but not yet acknowledged
    /// (Classic counts them until the ack is processed). Returns 0 when the
    /// queue MBean does not exist.
    /// A terminal reject-then-log acknowledges the message, so the source
    /// queue drains; a broker-native dead-letter move is observable through
    /// the DLQ's own queue MBean (`enqueue_count("ActiveMQ.DLQ")`).
    pub async fn queue_size(&self, queue: &str) -> Result<u64, String> {
        self.read_queue_attribute(queue, "QueueSize").await
    }

    /// Generic broker-side queue attribute read (for example `QueueSize`,
    /// `EnqueueCount`, `DequeueCount`, `ConsumerCount`). Returns 0 when the
    /// queue MBean does not exist yet. Kept generic so the phase-5 disposition
    /// test records the exact attribute the broker exposes rather than
    /// inferring a disposition from client-side deliveries.
    pub async fn read_queue_attribute(&self, queue: &str, attribute: &str) -> Result<u64, String> {
        let mbean = format!(
            "org.apache.activemq:type=Broker,brokerName=localhost,\
             destinationType=Queue,destinationName={queue}"
        );
        let response = self
            .jolokia(&json!({
                "type": "read",
                "mbean": mbean,
                "attribute": [attribute],
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
            return Err(format!("Jolokia {attribute} read failed: {response}"));
        }
        response["value"][attribute]
            .as_u64()
            .ok_or_else(|| format!("{attribute} missing in Jolokia response: {response}"))
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

    /// Browses the messages currently on `queue` through the Classic Queue
    /// MBean `browse()` operation. Browse does not consume; the queue keeps its
    /// messages. Returns an empty vec when the queue MBean does not exist yet
    /// (nothing has been enqueued) or has no message.
    pub async fn browse_messages(&self, queue: &str) -> Result<Vec<BrowsedMessage>, String> {
        let mbean = format!(
            "org.apache.activemq:type=Broker,brokerName=localhost,\
             destinationType=Queue,destinationName={queue}"
        );
        let response = self
            .jolokia_safe(&json!({
                "type": "exec",
                "mbean": mbean,
                "operation": "browse()",
            }))
            .await?;

        if response.get("status").and_then(Value::as_u64) != Some(200) {
            let not_found = response
                .get("error_type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.contains("InstanceNotFound"));
            if not_found {
                return Ok(Vec::new());
            }
            // The response is never interpolated: it can carry browsed message
            // property values, which callers assert invisibly.
            let kind = response
                .get("error_type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            return Err(format!(
                "Jolokia browse() failed for queue {queue} ({kind})"
            ));
        }

        let entries = response["value"].as_array().cloned().unwrap_or_default();
        Ok(entries.iter().map(BrowsedMessage::from_jolokia).collect())
    }
}

/// One message currently on a Classic queue, read through the Queue MBean
/// `browse()` operation.
///
/// Every composite field the provider exposes is retained as a string. The JMS
/// user-property map is the provider's `PropertiesText` rendering (for example
/// `{traceId=abc, region=eu}`). No method prints a value; callers assert with
/// predicates so no credential value can reach a log or a failure message.
/// No `Debug` or other conveniences: a `Debug` render would print every broker
/// field value, including any credential a regression leaked.
pub struct BrowsedMessage {
    fields: BTreeMap<String, String>,
}

impl BrowsedMessage {
    /// Builds a browsed message from one Jolokia `browse()` entry
    /// (composite field name -> value).
    pub fn from_jolokia(entry: &Value) -> Self {
        let fields = entry
            .as_object()
            .map(|object| {
                object
                    .iter()
                    .map(|(name, value)| (name.clone(), json_scalar_to_string(value)))
                    .collect()
            })
            .unwrap_or_default();
        Self { fields }
    }

    /// The genuine broker-assigned `JMSMessageID` field, when the browse entry
    /// carries one. This is the provider's own message identity; it is read
    /// only so callers can recover the producer connection identifier from it.
    /// No other message field is formatted here (no body, no header values).
    pub fn jms_message_id(&self) -> Option<&str> {
        self.fields.get("JMSMessageID").map(String::as_str)
    }

    /// The provider's user-property text (`PropertiesText`), or `""` when the
    /// message carries no user properties.
    pub fn properties_text(&self) -> &str {
        self.fields
            .get("PropertiesText")
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Parsed `PropertiesText` user properties as name/value pairs.
    fn parsed_properties(&self) -> Vec<(String, String)> {
        parse_properties_text(self.properties_text())
    }

    /// True when the user-property map has a property whose normalized name
    /// equals the normalized `name`.
    pub fn has_property_name(&self, name: &str) -> bool {
        let wanted = normalize_property_name(name);
        self.parsed_properties()
            .iter()
            .any(|(key, _)| normalize_property_name(key) == wanted)
    }

    /// The exact value of the user property whose normalized name equals the
    /// normalized `name`, if present.
    pub fn property_value(&self, name: &str) -> Option<String> {
        let wanted = normalize_property_name(name);
        self.parsed_properties()
            .into_iter()
            .find(|(key, _)| normalize_property_name(key) == wanted)
            .map(|(_, value)| value)
    }

    /// True when any browsed field contains `needle`, compared
    /// case-insensitively. Used to prove a denied value is absent from every
    /// broker-visible field, not only the user-property map.
    pub fn any_field_contains(&self, needle: &str) -> bool {
        let needle = needle.to_lowercase();
        self.fields
            .values()
            .any(|value| value.to_lowercase().contains(&needle))
    }
}

/// Renders a Jolokia JSON scalar (or nested value) as a string for
/// [`BrowsedMessage`]. `null` becomes `""`.
fn json_scalar_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Normalizes a JMS property name for comparison: casefold and remove `-`,
/// `_`, and `.` (the same rule the producer's allowlist/denylist uses).
fn normalize_property_name(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '-' | '_' | '.'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Parses the provider's `PropertiesText` rendering (`{k=v, k2=v2}`) into
/// name/value pairs. Empty, `null`, and unbraced input yield no pairs. Values
/// that contain a comma-space are not expected for the sentinel headers this
/// helper is used with.
fn parse_properties_text(text: &str) -> Vec<(String, String)> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Vec::new();
    }
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or(trimmed);
    if inner.trim().is_empty() {
        return Vec::new();
    }
    inner
        .split(", ")
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((key.trim().to_string(), value.trim().to_string()))
        })
        .collect()
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

/// Response-body-free Jolokia request for the `browse()` path.
///
/// Identical to [`jolokia_request`] except for the non-JSON error: the browse
/// reply can carry browsed message property values, so the body is never
/// interpolated. Only the HTTP status, the serde error class, and the
/// line/column are reported.
async fn jolokia_request_safe(
    client: &Client,
    url: &str,
    request: &Value,
) -> Result<Value, String> {
    let body =
        serde_json::to_string(request).map_err(|e| format!("serialize Jolokia body: {e}"))?;
    let response = client
        .post(url)
        .basic_auth(ACTIVEMQ_USER, Some(ACTIVEMQ_PASSWORD))
        .header("Origin", "http://127.0.0.1")
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Jolokia request to {url} failed: {e}"))?;
    let status = response.status().to_string();
    let text = response
        .text()
        .await
        .map_err(|e| format!("read Jolokia body from {url}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| safe_json_parse_error(url, &status, &e))
}

/// Builds the non-JSON diagnostic for the browse path without echoing the
/// response body. Exposed to the focused sentinel test.
pub(crate) fn safe_json_parse_error(url: &str, status: &str, error: &serde_json::Error) -> String {
    format!(
        "Jolokia response from {url} was not JSON (HTTP {status}): {} at line {} column {}",
        json_error_class(error),
        error.line(),
        error.column()
    )
}

/// Stable, value-free serde error class label.
fn json_error_class(error: &serde_json::Error) -> &'static str {
    use serde_json::error::Category;
    match error.classify() {
        Category::Io => "io",
        Category::Syntax => "syntax",
        Category::Data => "data",
        Category::Eof => "eof",
    }
}
