use camel_component_api::{Body, CamelError, Exchange};
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

use crate::config::{KafkaEndpointConfig, apply_security_config};

#[derive(Clone)]
pub struct KafkaProducer {
    config: KafkaEndpointConfig,
    producer: Arc<FutureProducer>,
}

impl KafkaProducer {
    pub fn new(config: KafkaEndpointConfig) -> Result<Self, CamelError> {
        let brokers = config.brokers.as_ref().expect("brokers must be resolved");
        let request_timeout_ms = config
            .request_timeout_ms
            .expect("request_timeout_ms must be resolved");

        let mut cc = ClientConfig::new();
        cc.set("bootstrap.servers", brokers)
            .set("message.timeout.ms", request_timeout_ms.to_string())
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
        config: &'a KafkaEndpointConfig,
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

    pub fn resolve_record_key(exchange: &Exchange) -> Option<String> {
        exchange
            .input
            .header("CamelKafkaKey")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
    }

    pub fn resolve_record_partition(exchange: &Exchange) -> Option<i32> {
        exchange
            .input
            .header("CamelKafkaPartition")
            .and_then(|v| v.as_i64().map(|n| n as i32))
    }

    pub fn resolve_request_timeout(config: &KafkaEndpointConfig) -> Duration {
        Duration::from_millis(
            config
                .request_timeout_ms
                .expect("request_timeout_ms must be resolved") as u64,
        )
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

            let key = Self::resolve_record_key(&exchange);
            let partition = Self::resolve_record_partition(&exchange);
            let timeout = Self::resolve_request_timeout(&config);

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
    use camel_api::StreamBody;
    use camel_component_api::Message;
    use futures::stream;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn make_config() -> KafkaEndpointConfig {
        KafkaEndpointConfig::from_uri("kafka:test-topic?brokers=localhost:9092").unwrap()
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

    #[test]
    fn test_resolve_topic_errors_when_config_topic_empty() {
        let mut config = make_config();
        config.topic.clear();
        let exchange = Exchange::new(Message::default());

        let err = KafkaProducer::resolve_topic(&exchange, &config)
            .expect_err("empty topic should be rejected");
        assert!(err.to_string().contains("No Kafka topic specified"));
    }

    #[test]
    fn test_body_xml_to_bytes() {
        let body = Body::Xml("<root>ok</root>".to_string());
        let bytes = KafkaProducer::body_to_bytes(&body).expect("xml to bytes");
        assert_eq!(bytes, b"<root>ok</root>");
    }

    #[tokio::test]
    async fn test_call_fails_fast_when_topic_missing() {
        let mut config = make_config();
        config.resolve_defaults();
        config.topic.clear();

        let mut producer = KafkaProducer::new(config).expect("producer should build");
        let exchange = Exchange::new(Message::new(Body::Text("hello".to_string())));

        let err = producer
            .call(exchange)
            .await
            .expect_err("missing topic must fail");
        assert!(err.to_string().contains("No Kafka topic specified"));
    }

    #[tokio::test]
    async fn test_call_fails_fast_for_stream_body() {
        let mut config = make_config();
        config.resolve_defaults();
        let mut producer = KafkaProducer::new(config).expect("producer should build");

        let stream = stream::iter(vec![Ok(Bytes::from("chunk"))]);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: Default::default(),
        });
        let exchange = Exchange::new(Message::new(body));

        let err = producer
            .call(exchange)
            .await
            .expect_err("stream body must fail before network send");
        assert!(
            err.to_string()
                .contains("Body::Stream must be materialized before sending to Kafka")
        );
    }

    #[test]
    fn test_resolve_record_key_from_header() {
        let mut msg = Message::default();
        msg.set_header("CamelKafkaKey", serde_json::json!("k-1"));
        let ex = Exchange::new(msg);
        assert_eq!(
            KafkaProducer::resolve_record_key(&ex),
            Some("k-1".to_string())
        );
    }

    #[test]
    fn test_resolve_record_key_ignores_non_string() {
        let mut msg = Message::default();
        msg.set_header("CamelKafkaKey", serde_json::json!(123));
        let ex = Exchange::new(msg);
        assert_eq!(KafkaProducer::resolve_record_key(&ex), None);
    }

    #[test]
    fn test_resolve_record_partition_from_header() {
        let mut msg = Message::default();
        msg.set_header("CamelKafkaPartition", serde_json::json!(3));
        let ex = Exchange::new(msg);
        assert_eq!(KafkaProducer::resolve_record_partition(&ex), Some(3));
    }

    #[test]
    fn test_resolve_record_partition_ignores_non_numeric() {
        let mut msg = Message::default();
        msg.set_header("CamelKafkaPartition", serde_json::json!("p1"));
        let ex = Exchange::new(msg);
        assert_eq!(KafkaProducer::resolve_record_partition(&ex), None);
    }

    #[test]
    fn test_resolve_request_timeout_from_config() {
        let mut config = make_config();
        config.resolve_defaults();
        config.request_timeout_ms = Some(4321);
        assert_eq!(
            KafkaProducer::resolve_request_timeout(&config),
            Duration::from_millis(4321)
        );
    }
}
