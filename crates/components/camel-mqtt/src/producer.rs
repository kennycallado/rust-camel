// MQTT Producer: publishes messages to a broker topic as a Tower Service.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::Service;
use tracing::warn;

use camel_api::{Body, CamelError, Exchange, Value};
use camel_component_api::NetworkRetryPolicy;
use rumqttc::{AsyncClient, MqttOptions, QoS};

use crate::client_id::build_client_id_with_override;
use crate::config::{MqttBrokerConfig, MqttEndpointConfig, check_payload_len};
use crate::headers::{CAMEL_MQTT_QOS, CAMEL_MQTT_RETAIN, CAMEL_MQTT_TOPIC};

const MAX_INFLIGHT: usize = 16;

pub fn validate_publish_topic(topic: &str) -> Result<(), CamelError> {
    if topic.contains('+') || topic.contains('#') {
        return Err(CamelError::Config(format!(
            "mqtt producer: topic must not contain wildcards (+, #): {topic}"
        )));
    }
    Ok(())
}

/// Resolve outbound QoS from an optional CamelMqttQos header, falling back to the
/// configured default. Explicit header values are validated: "0"/"1"/"2" accepted;
/// anything else rejected (no silent fallback).
fn resolve_outbound_qos(header: Option<&str>, default: QoS) -> Result<QoS, CamelError> {
    match header {
        Some("0") => Ok(QoS::AtMostOnce),
        Some("1") => Ok(QoS::AtLeastOnce),
        Some("2") => Ok(QoS::ExactlyOnce),
        Some(v) => Err(CamelError::Config(format!(
            "mqtt producer: invalid CamelMqttQos header '{v}' (expected 0, 1, or 2)"
        ))),
        None => Ok(default),
    }
}

/// Convert a Body to bytes for publishing (mirrors camel-jms producer).
fn body_to_bytes(body: &Body) -> Result<Vec<u8>, CamelError> {
    match body {
        Body::Text(s) => Ok(s.as_bytes().to_vec()),
        Body::Xml(s) => Ok(s.as_bytes().to_vec()),
        Body::Bytes(b) => Ok(b.to_vec()),
        Body::Json(v) => serde_json::to_vec(v)
            .map_err(|e| CamelError::ProcessorError(format!("JSON error: {e}"))),
        Body::Empty => Ok(vec![]),
        Body::Stream(_) => Err(CamelError::ProcessorError(
            "Body::Stream must be materialized before sending to MQTT".into(),
        )),
    }
}

struct DriverHandle {
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

impl Drop for DriverHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.handle.abort();
    }
}

#[derive(Clone)]
pub struct MqttProducer {
    client: AsyncClient,
    config: MqttEndpointConfig,
    semaphore: Arc<Semaphore>,
    _driver: Arc<DriverHandle>,
}

impl MqttProducer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: MqttEndpointConfig,
        broker: MqttBrokerConfig,
        uri: &str,
        client_id_prefix: &str,
        route_id: Option<&str>,
        fallback_reconnect: NetworkRetryPolicy,
    ) -> Result<Self, CamelError> {
        let (host, port) = broker.host_port()?;
        let route_component = route_id.unwrap_or("producer");
        let client_id = build_client_id_with_override(
            client_id_prefix,
            route_component,
            uri,
            config.client_id_override.as_deref(),
        );

        let mut mqtt_opts = MqttOptions::new(&client_id, (host, port));
        mqtt_opts.set_keep_alive(config.keep_alive_secs as u16);
        mqtt_opts.set_clean_session(config.clean_session);

        if let (Some(user), Some(pass)) = (&broker.username, &broker.password) {
            mqtt_opts.set_credentials(user, pass.as_bytes().to_vec());
        }

        #[cfg(feature = "tls")]
        if broker.is_tls() {
            let tls_config = crate::tls::build_tls_config(broker.tls_ca_cert.as_deref())?;
            mqtt_opts.set_transport(rumqttc::Transport::tls_with_config(tls_config));
        }
        #[cfg(not(feature = "tls"))]
        if broker.is_tls() {
            return Err(CamelError::Config(
                "mqtt: mqtts:// requires the 'tls' feature on camel-component-mqtt".into(),
            ));
        }

        let (client, mut eventloop) = AsyncClient::builder(mqtt_opts)
            .capacity(MAX_INFLIGHT)
            .build();

        let reconnect = config.effective_reconnect(&fallback_reconnect);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move {
            let mut attempt: u32 = 0;
            loop {
                tokio::select! {
                    biased;
                    _ = cancel_clone.cancelled() => break,
                    result = eventloop.poll() => {
                        if let Err(e) = result {
                            // CORRECTION APPLIED: warn! (not error!) — this is a retried
                            // transient connection error; warn! needs no log-policy annotation
                            // and the producer driver has no runtime for a replacement signal.
                            warn!(error = %e, "MQTT producer EventLoop error; will retry");
                            let delay = reconnect.delay_for(attempt);
                            attempt = attempt.saturating_add(1);
                            tokio::select! {
                                biased;
                                _ = cancel_clone.cancelled() => break,
                                _ = tokio::time::sleep(delay) => {}
                            }
                        } else {
                            attempt = 0;
                        }
                    }
                }
            }
        });

        Ok(Self {
            client,
            config,
            semaphore: Arc::new(Semaphore::new(MAX_INFLIGHT)),
            _driver: Arc::new(DriverHandle { cancel, handle }),
        })
    }
}

impl Service<Exchange> for MqttProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Backpressure enforced in call() via semaphore acquire.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let client = self.client.clone();
        let config = self.config.clone();
        let semaphore = self.semaphore.clone();

        Box::pin(async move {
            let _permit = semaphore
                .acquire()
                .await
                .map_err(|e| CamelError::ProcessorError(e.to_string()))?;

            let topic = exchange
                .input
                .header(CAMEL_MQTT_TOPIC)
                .and_then(Value::as_str)
                .map(|s| s.to_string())
                .or_else(|| config.publish_topic.clone())
                .ok_or_else(|| {
                    CamelError::Config(
                        "mqtt producer: no topic — set URI path or CamelMqttTopic header".into(),
                    )
                })?;

            validate_publish_topic(&topic)?;

            let qos = resolve_outbound_qos(
                exchange
                    .input
                    .header(CAMEL_MQTT_QOS)
                    .and_then(Value::as_str),
                config.qos.to_rumqttc(),
            )?;

            let retain = match exchange
                .input
                .header(CAMEL_MQTT_RETAIN)
                .and_then(Value::as_str)
            {
                Some("true") => true,
                Some("false") => false,
                _ => config.retain,
            };

            let payload = body_to_bytes(&exchange.input.body)?;
            check_payload_len(payload.len(), config.max_payload_bytes)?;

            // rumqttc publish signature: topic, qos, retain, payload
            client
                .publish(topic, qos, retain, payload)
                .await
                .map_err(|e| CamelError::ProcessorError(e.to_string()))?;

            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn producer_rejects_wildcard_topic() {
        assert!(validate_publish_topic("sensors/#").is_err());
        assert!(validate_publish_topic("sensors/+/temp").is_err());
        assert!(validate_publish_topic("sensors/temp").is_ok());
    }

    #[test]
    fn outbound_qos_resolution() {
        assert_eq!(
            resolve_outbound_qos(Some("0"), QoS::AtLeastOnce).unwrap(),
            QoS::AtMostOnce
        );
        assert_eq!(
            resolve_outbound_qos(Some("1"), QoS::AtMostOnce).unwrap(),
            QoS::AtLeastOnce
        );
        assert_eq!(
            resolve_outbound_qos(Some("2"), QoS::AtMostOnce).unwrap(),
            QoS::ExactlyOnce
        );
        assert!(resolve_outbound_qos(Some("9"), QoS::AtLeastOnce).is_err());
        assert!(resolve_outbound_qos(Some("high"), QoS::AtLeastOnce).is_err());
        assert_eq!(
            resolve_outbound_qos(None, QoS::ExactlyOnce).unwrap(),
            QoS::ExactlyOnce
        );
    }

    #[test]
    fn payload_limit_rejects_oversized() {
        assert!(matches!(
            check_payload_len(100, 50),
            Err(CamelError::StreamLimitExceeded(_))
        ));
        assert!(check_payload_len(50, 50).is_ok());
    }

    #[test]
    fn body_text_to_bytes() {
        let b = body_to_bytes(&Body::Text("hello".to_string())).unwrap();
        assert_eq!(b, b"hello");
    }

    #[test]
    fn body_empty_to_bytes() {
        let b = body_to_bytes(&Body::Empty).unwrap();
        assert!(b.is_empty());
    }

    #[test]
    fn body_bytes_to_bytes() {
        let b = body_to_bytes(&Body::Bytes(bytes::Bytes::from_static(&[1, 2, 3]))).unwrap();
        assert_eq!(b, vec![1, 2, 3]);
    }

    #[test]
    fn body_xml_to_bytes() {
        let b = body_to_bytes(&Body::Xml("<root/>".to_string())).unwrap();
        assert_eq!(b, b"<root/>");
    }

    #[test]
    fn body_json_to_bytes() {
        let val = serde_json::json!({"key": "value"});
        let b = body_to_bytes(&Body::Json(val)).unwrap();
        assert_eq!(b, br#"{"key":"value"}"#);
    }
}
