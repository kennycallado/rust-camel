use camel_api::{Body, CamelError, Exchange};
use rdkafka::config::ClientConfig;
#[cfg(feature = "otel")]
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord};
use serde_json::json;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tower::Service;
use tracing::{debug, error};

use crate::config::{KafkaConfig, apply_security_config};

#[derive(Clone)]
pub struct KafkaProducer {
    config: KafkaConfig,
    producer: Arc<FutureProducer>,
}

impl KafkaProducer {
    pub fn new(config: KafkaConfig) -> Result<Self, CamelError> {
        let mut cc = ClientConfig::new();
        cc.set("bootstrap.servers", &config.brokers)
            .set("message.timeout.ms", config.request_timeout_ms.to_string())
            .set("acks", &config.acks);

        apply_security_config(&config, &mut cc);

        let producer: FutureProducer = cc.create().map_err(|e| {
            CamelError::ProcessorError(format!("Failed to create Kafka producer: {}", e))
        })?;

        Ok(Self {
            config,
            producer: Arc::new(producer),
        })
    }

    pub fn body_to_bytes(body: &Body) -> Result<Vec<u8>, CamelError> {
        match body {
            Body::Text(s) => Ok(s.as_bytes().to_vec()),
            Body::Xml(s) => Ok(s.as_bytes().to_vec()),
            Body::Bytes(b) => Ok(b.to_vec()),
            Body::Json(v) => Ok(serde_json::to_string(v)
                .map_err(|e| {
                    CamelError::ProcessorError(format!("JSON serialization error: {}", e))
                })?
                .into_bytes()),
            Body::Empty => Ok(vec![]),
            Body::Stream(_) => Err(CamelError::ProcessorError(
                "Body::Stream must be materialized before sending to Kafka".to_string(),
            )),
        }
    }

    pub fn resolve_topic<'a>(
        exchange: &'a Exchange,
        config: &'a KafkaConfig,
    ) -> Result<&'a str, CamelError> {
        // Check for header override first
        if let Some(v) = exchange.input.header("CamelKafkaTopic")
            && let Some(s) = v.as_str()
            && !s.is_empty()
        {
            return Ok(s);
        }
        // Fall back to config
        if config.topic.is_empty() {
            return Err(CamelError::ProcessorError(
                "No Kafka topic specified".to_string(),
            ));
        }
        Ok(&config.topic)
    }
}

impl Service<Exchange> for KafkaProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let producer = self.producer.clone();

        Box::pin(async move {
            let topic = Self::resolve_topic(&exchange, &config)?.to_string();
            let payload = Self::body_to_bytes(&exchange.input.body)?;

            let key: Option<String> = exchange
                .input
                .header("CamelKafkaKey")
                .and_then(|v| v.as_str().map(|s| s.to_string()));

            let partition: Option<i32> = exchange
                .input
                .header("CamelKafkaPartition")
                .and_then(|v| v.as_i64().map(|n| n as i32));

            let timeout = Duration::from_millis(config.request_timeout_ms as u64);

            // Inject W3C TraceContext headers for distributed tracing (otel feature only)
            #[cfg(feature = "otel")]
            let otel_headers = {
                let mut headers_map = std::collections::HashMap::new();
                camel_otel::inject_from_exchange(&exchange, &mut headers_map);
                headers_map
            };

            let delivery_result = {
                let mut record = FutureRecord::to(&topic).payload(&payload);
                if let Some(ref k) = key {
                    record = record.key(k.as_str());
                }
                if let Some(p) = partition {
                    record = record.partition(p);
                }
                #[cfg(feature = "otel")]
                {
                    let mut owned_headers = OwnedHeaders::new();
                    for (key, value) in &otel_headers {
                        owned_headers = owned_headers.insert(Header {
                            key,
                            value: Some(value.as_bytes()),
                        });
                    }
                    record = record.headers(owned_headers);
                }
                producer.send(record, timeout).await
            };

            match delivery_result {
                Ok((partition_out, offset_out)) => {
                    debug!(
                        topic = %topic,
                        partition = partition_out,
                        offset = offset_out,
                        "Kafka message delivered"
                    );
                    exchange.input.set_header(
                        "CamelKafkaRecordMetadata",
                        json!({
                            "topic": topic,
                            "partition": partition_out,
                            "offset": offset_out,
                        }),
                    );
                    Ok(exchange)
                }
                Err((e, _)) => {
                    error!(error = %e, topic = %topic, "Kafka delivery failed");
                    Err(CamelError::ProcessorError(format!(
                        "Kafka delivery failed to topic '{}': {}",
                        topic, e
                    )))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use camel_api::{Message, StreamBody};
    use futures::stream;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn make_config() -> KafkaConfig {
        KafkaConfig::from_uri("kafka:test-topic?brokers=localhost:9092").unwrap()
    }

    #[test]
    fn test_body_text_to_bytes() {
        let body = Body::Text("hello world".to_string());
        let bytes = KafkaProducer::body_to_bytes(&body).unwrap();
        assert_eq!(bytes, b"hello world");
    }

    #[test]
    fn test_body_bytes_to_bytes() {
        let body = Body::Bytes(bytes::Bytes::from_static(b"raw bytes"));
        let bytes = KafkaProducer::body_to_bytes(&body).unwrap();
        assert_eq!(bytes, b"raw bytes");
    }

    #[test]
    fn test_body_json_to_bytes() {
        let body = Body::Json(json!({"key": "value"}));
        let bytes = KafkaProducer::body_to_bytes(&body).unwrap();
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.contains("key"));
        assert!(s.contains("value"));
    }

    #[test]
    fn test_body_empty_to_bytes() {
        let body = Body::Empty;
        let bytes = KafkaProducer::body_to_bytes(&body).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn test_body_stream_fails() {
        // Body::Stream takes a StreamBody struct
        let stream = stream::iter(vec![Ok(Bytes::from("data"))]);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: Default::default(),
        });
        let result = KafkaProducer::body_to_bytes(&body);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CamelError::ProcessorError(_)));
    }

    #[test]
    fn test_resolve_topic_from_config() {
        let config = make_config();
        let exchange = Exchange::new(Message::default());
        let topic = KafkaProducer::resolve_topic(&exchange, &config).unwrap();
        assert_eq!(topic, "test-topic");
    }

    #[test]
    fn test_resolve_topic_from_header_overrides_config() {
        let config = make_config();
        let mut msg = Message::default();
        msg.set_header(
            "CamelKafkaTopic",
            serde_json::Value::String("override-topic".to_string()),
        );
        let exchange = Exchange::new(msg);
        let topic = KafkaProducer::resolve_topic(&exchange, &config).unwrap();
        assert_eq!(topic, "override-topic");
    }

    #[test]
    fn test_resolve_topic_empty_header_falls_back_to_config() {
        let config = make_config();
        let mut msg = Message::default();
        msg.set_header("CamelKafkaTopic", serde_json::Value::String("".to_string()));
        let exchange = Exchange::new(msg);
        let topic = KafkaProducer::resolve_topic(&exchange, &config).unwrap();
        assert_eq!(topic, "test-topic");
    }

    #[test]
    fn test_resolve_topic_non_string_header_falls_back_to_config() {
        let config = make_config();
        let mut msg = Message::default();
        msg.set_header("CamelKafkaTopic", serde_json::Value::Number(42.into()));
        let exchange = Exchange::new(msg);
        let topic = KafkaProducer::resolve_topic(&exchange, &config).unwrap();
        assert_eq!(topic, "test-topic");
    }

    // Integration tests — require running Kafka
    // Run with: KAFKA_BROKERS=localhost:9092 cargo test -p camel-component-kafka -- --ignored

    #[tokio::test]
    #[ignore]
    async fn test_producer_sends_and_acks() {
        let brokers = std::env::var("KAFKA_BROKERS").unwrap_or("localhost:9092".to_string());
        let config =
            KafkaConfig::from_uri(&format!("kafka:test-integration?brokers={}", brokers)).unwrap();

        let mut producer = KafkaProducer::new(config).unwrap();
        let msg = Message::new(Body::Text("integration test".to_string()));
        let exchange = Exchange::new(msg);

        use tower::Service;
        let result = producer.call(exchange).await;
        assert!(result.is_ok());

        let out = result.unwrap();
        let metadata = out.input.header("CamelKafkaRecordMetadata");
        assert!(metadata.is_some());
    }

    #[tokio::test]
    #[ignore]
    async fn test_producer_resolves_topic_from_header() {
        let brokers = std::env::var("KAFKA_BROKERS").unwrap_or("localhost:9092".to_string());
        let config =
            KafkaConfig::from_uri(&format!("kafka:default-topic?brokers={}", brokers)).unwrap();

        let mut producer = KafkaProducer::new(config).unwrap();
        let mut msg = Message::new(Body::Text("test".to_string()));
        msg.set_header(
            "CamelKafkaTopic",
            serde_json::Value::String("override-topic".to_string()),
        );
        let exchange = Exchange::new(msg);

        let result = producer.call(exchange).await;
        assert!(result.is_ok());
        let out = result.unwrap();
        let meta = out.input.header("CamelKafkaRecordMetadata").unwrap();
        assert_eq!(meta["topic"].as_str().unwrap(), "override-topic");
    }

    #[tokio::test]
    #[ignore]
    async fn test_producer_delivery_error_on_unavailable_broker() {
        let config =
            KafkaConfig::from_uri("kafka:test?brokers=192.0.2.1:9092&requestTimeoutMs=3000")
                .unwrap();
        let mut producer = KafkaProducer::new(config).unwrap();
        let msg = Message::new(Body::Text("test".to_string()));
        let exchange = Exchange::new(msg);

        let result = producer.call(exchange).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CamelError::ProcessorError(_)));
    }
}
