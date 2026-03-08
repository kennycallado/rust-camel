use async_trait::async_trait;
use camel_api::{Body, CamelError, Exchange, Message};
use camel_component::{ConcurrencyModel, Consumer, ConsumerContext};
use rdkafka::client::ClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{
    Consumer as RdConsumer, ConsumerContext as RdConsumerContext, Rebalance, StreamConsumer,
};
// Import rdkafka::Message trait to bring .topic(), .key(), .payload(), etc. into scope.
// The alias `_` prevents a name conflict with camel_api::Message.
use rdkafka::message::Message as _;
#[cfg(feature = "otel")]
use rdkafka::message::Headers as _;
use rdkafka::message::OwnedMessage;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// ReadyContext — notifies when the consumer gets its first partition assignment
// ---------------------------------------------------------------------------

/// A custom rdkafka context that fires an `Arc<Notify>` when the consumer
/// receives its first partition assignment via the rebalance callback.
/// This avoids polling `assignment()` in a tight loop (which itself requires
/// `recv()` to drive the rebalance protocol — a deadlock).
struct ReadyContext {
    ready: Arc<Notify>,
}

impl ClientContext for ReadyContext {}

impl RdConsumerContext for ReadyContext {
    fn post_rebalance(&self, rebalance: &Rebalance<'_>) {
        if matches!(rebalance, Rebalance::Assign(_)) {
            // Partitions were assigned — signal any waiters.
            self.ready.notify_waiters();
        }
    }
}

type ReadyStreamConsumer = StreamConsumer<ReadyContext>;

use crate::config::KafkaConfig;

pub struct KafkaConsumer {
    config: KafkaConfig,
    cancel_token: Option<CancellationToken>,
    task_handle: Option<JoinHandle<Result<(), CamelError>>>,
    /// Notified once the consumer has received its first partition assignment.
    ready: Arc<Notify>,
}

impl KafkaConsumer {
    pub fn new(config: KafkaConfig) -> Self {
        Self {
            config,
            cancel_token: None,
            task_handle: None,
            ready: Arc::new(Notify::new()),
        }
    }

    /// Returns a handle that resolves once the consumer has been assigned
    /// at least one partition.  Useful in tests to avoid arbitrary sleeps.
    pub fn ready_signal(&self) -> Arc<Notify> {
        self.ready.clone()
    }
}

#[async_trait]
impl Consumer for KafkaConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        let cancel_token = CancellationToken::new();
        self.cancel_token = Some(cancel_token.clone());

        let config = self.config.clone();
        let ready = self.ready.clone();

        info!(
            topic = %config.topic,
            brokers = %config.brokers,
            group_id = %config.group_id,
            "Starting Kafka consumer"
        );

        let handle = tokio::spawn(run_consumer_loop(config, ctx, cancel_token, ready));
        self.task_handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        info!("Stopping Kafka consumer for topic '{}'", self.config.topic);

        if let Some(token) = &self.cancel_token {
            token.cancel();
        }

        if let Some(handle) = self.task_handle.take() {
            match handle.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => error!("Consumer task error: {}", e),
                Err(e) => error!("Failed to join consumer task: {}", e),
            }
        }

        self.cancel_token = None;
        info!("Kafka consumer stopped");
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        // Kafka consumers process messages sequentially within a partition group
        // to preserve offset ordering and at-least-once commit semantics.
        ConcurrencyModel::Sequential
    }
}

/// Build an Exchange from an OwnedMessage.
///
/// Sets headers: CamelKafkaTopic, CamelKafkaPartition, CamelKafkaOffset,
/// CamelKafkaKey (if present), CamelKafkaTimestamp (if present), CamelKafkaGroupId
/// Sets property: "kafka.manual.commit" → JSON object with {topic, partition, offset}
pub fn build_exchange(msg: &OwnedMessage, group_id: &str) -> Exchange {
    // Payload
    let body = match msg.payload() {
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(s) => Body::Text(s.to_string()),
            Err(_) => Body::Bytes(bytes::Bytes::copy_from_slice(bytes)),
        },
        None => Body::Empty,
    };

    let mut exchange = Exchange::new(Message::new(body));

    exchange
        .input
        .set_header("CamelKafkaTopic", Value::String(msg.topic().to_string()));
    exchange
        .input
        .set_header("CamelKafkaPartition", Value::Number(msg.partition().into()));
    exchange
        .input
        .set_header("CamelKafkaOffset", Value::Number(msg.offset().into()));
    // let-chains: stable in Rust edition 2024
    if let Some(key_bytes) = msg.key()
        && let Ok(key_str) = std::str::from_utf8(key_bytes)
    {
        exchange
            .input
            .set_header("CamelKafkaKey", Value::String(key_str.to_string()));
    } else if msg.key().is_some() {
        warn!(
            topic = %msg.topic(),
            "Kafka message key is non-UTF-8 bytes; CamelKafkaKey header not set"
        );
    }
    if let Some(ts) = msg.timestamp().to_millis() {
        exchange
            .input
            .set_header("CamelKafkaTimestamp", Value::Number(ts.into()));
    }
    exchange
        .input
        .set_header("CamelKafkaGroupId", Value::String(group_id.to_string()));

    // Store commit metadata in properties for user reference
    exchange.properties.insert(
        "kafka.manual.commit".to_string(),
        serde_json::json!({
            "topic": msg.topic(),
            "partition": msg.partition(),
            "offset": msg.offset(),
        }),
    );

    // Extract W3C TraceContext headers for distributed tracing (otel feature only)
    #[cfg(feature = "otel")]
    {
        let mut headers_map = std::collections::HashMap::new();
        if let Some(headers) = msg.headers() {
            for i in 0..headers.count() {
                let header = headers.get(i);
                if let Some(value_bytes) = header.value {
                    if let Ok(v) = std::str::from_utf8(value_bytes) {
                        headers_map.insert(header.key.to_string(), v.to_string());
                    }
                }
            }
        }
        camel_otel::extract_into_exchange(&mut exchange, &headers_map);
    }

    exchange
}

async fn run_consumer_loop(
    config: KafkaConfig,
    ctx: ConsumerContext,
    cancel_token: CancellationToken,
    ready: Arc<Notify>,
) -> Result<(), CamelError> {
    use rdkafka::consumer::CommitMode;

    let consumer: ReadyStreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &config.brokers)
        .set("group.id", &config.group_id)
        .set("auto.offset.reset", &config.auto_offset_reset)
        .set("session.timeout.ms", config.session_timeout_ms.to_string())
        .set("enable.auto.commit", "false")
        .set("fetch.wait.max.ms", config.poll_timeout_ms.to_string())
        // Note: max.poll.records is a Java Kafka client concept; librdkafka uses
        // queued.max.messages.kbytes / max.partition.fetch.bytes instead.
        .create_with_context(ReadyContext { ready })
        .map_err(|e| {
            CamelError::ProcessorError(format!("Failed to create Kafka consumer: {}", e))
        })?;

    consumer.subscribe(&[config.topic.as_str()]).map_err(|e| {
        CamelError::ProcessorError(format!(
            "Failed to subscribe to topic '{}': {}",
            config.topic, e
        ))
    })?;

    info!(topic = %config.topic, "Kafka consumer subscribed");

    // The ReadyContext::post_rebalance callback fires `ready.notify_waiters()`
    // when partitions are assigned. No polling loop needed — recv() drives the
    // rebalance protocol automatically.

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(topic = %config.topic, "Kafka consumer received shutdown signal");
                break;
            }
            msg_result = consumer.recv() => {
                match msg_result {
                    Err(e) => {
                        warn!(error = %e, "Kafka consumer error, continuing");
                        // Brief backoff to avoid tight error loops on transient failures
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    Ok(msg) => {
                        // Must detach before await point (BorrowedMessage not 'static)
                        let owned = msg.detach();
                        let exchange = build_exchange(&owned, &config.group_id);
                        if let Err(e) = ctx.send(exchange).await {
                            error!(error = %e, "Failed to send exchange to pipeline");
                        }
                        // Commit offset after dispatching (at-least-once semantics)
                        let tpl = {
                            use rdkafka::TopicPartitionList;
                            let mut tpl = TopicPartitionList::new();
                            tpl.add_partition_offset(
                                owned.topic(),
                                owned.partition(),
                                rdkafka::Offset::Offset(owned.offset() + 1),
                            ).expect("topic/partition from valid message should never fail");
                            tpl
                        };
                        if let Err(e) = consumer.commit(&tpl, CommitMode::Async) {
                            warn!(error = %e, "Failed to commit Kafka offset");
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdkafka::Timestamp;

    fn make_config() -> KafkaConfig {
        KafkaConfig::from_uri("kafka:test-topic?brokers=localhost:9092&groupId=test-group").unwrap()
    }

    fn make_msg(
        payload: Option<Vec<u8>>,
        key: Option<Vec<u8>>,
        topic: &str,
        timestamp: Timestamp,
        partition: i32,
        offset: i64,
    ) -> OwnedMessage {
        OwnedMessage::new(
            payload,
            key,
            topic.to_string(),
            timestamp,
            partition,
            offset,
            None,
        )
    }

    #[test]
    fn test_consumer_new() {
        let config = make_config();
        let consumer = KafkaConsumer::new(config);
        assert!(consumer.cancel_token.is_none());
        assert!(consumer.task_handle.is_none());
    }

    #[test]
    fn test_concurrency_model_is_sequential() {
        let config = make_config();
        let consumer = KafkaConsumer::new(config);
        assert_eq!(consumer.concurrency_model(), ConcurrencyModel::Sequential);
    }

    #[tokio::test]
    async fn test_consumer_stop_without_start() {
        let config = make_config();
        let mut consumer = KafkaConsumer::new(config);
        // stop() before start() should be a no-op, not panic
        let result = consumer.stop().await;
        assert!(result.is_ok());
    }

    // --- build_exchange tests ---

    #[test]
    fn test_build_exchange_empty_payload() {
        let msg = make_msg(None, None, "t", Timestamp::NotAvailable, 0, 0);
        let ex = build_exchange(&msg, "g");
        assert!(matches!(ex.input.body, Body::Empty));
    }

    #[test]
    fn test_build_exchange_binary_payload() {
        let bad_utf8 = vec![0xFF, 0xFE, 0x00];
        let msg = make_msg(
            Some(bad_utf8.clone()),
            None,
            "t",
            Timestamp::NotAvailable,
            0,
            0,
        );
        let ex = build_exchange(&msg, "g");
        assert!(
            matches!(&ex.input.body, Body::Bytes(b) if b.as_ref() == bad_utf8.as_slice()),
            "expected Body::Bytes with the original bytes"
        );
    }

    #[test]
    fn test_build_exchange_text_payload() {
        let msg = make_msg(
            Some(b"hello".to_vec()),
            None,
            "t",
            Timestamp::NotAvailable,
            0,
            0,
        );
        let ex = build_exchange(&msg, "g");
        assert!(
            matches!(&ex.input.body, Body::Text(s) if s == "hello"),
            "expected Body::Text(\"hello\")"
        );
    }

    #[test]
    fn test_build_exchange_with_key_sets_header() {
        let msg = make_msg(
            None,
            Some(b"my-key".to_vec()),
            "t",
            Timestamp::NotAvailable,
            0,
            0,
        );
        let ex = build_exchange(&msg, "g");
        assert_eq!(
            ex.input.header("CamelKafkaKey"),
            Some(&Value::String("my-key".to_string()))
        );
    }

    #[test]
    fn test_build_exchange_without_key_no_header() {
        let msg = make_msg(None, None, "t", Timestamp::NotAvailable, 0, 0);
        let ex = build_exchange(&msg, "g");
        assert!(
            ex.input.header("CamelKafkaKey").is_none(),
            "CamelKafkaKey header should be absent when message has no key"
        );
    }

    #[test]
    fn test_build_exchange_manual_commit_is_json_object() {
        let msg = make_msg(None, None, "my-topic", Timestamp::NotAvailable, 2, 42);
        let ex = build_exchange(&msg, "g");
        let val = ex
            .properties
            .get("kafka.manual.commit")
            .expect("kafka.manual.commit property must be present");
        assert!(
            matches!(val, Value::Object(_)),
            "kafka.manual.commit must be a JSON Object, got: {:?}",
            val
        );
        // Verify the object contains the expected fields
        let obj = val.as_object().expect("already asserted Object above");
        assert_eq!(obj.get("topic").and_then(|v| v.as_str()), Some("my-topic"));
        assert_eq!(obj.get("partition").and_then(|v| v.as_i64()), Some(2));
        assert_eq!(obj.get("offset").and_then(|v| v.as_i64()), Some(42));
    }

    #[test]
    fn test_build_exchange_binary_key_no_header() {
        let binary_key = vec![0xff, 0xfe];
        let msg = make_msg(None, Some(binary_key), "t", Timestamp::NotAvailable, 0, 0);
        let ex = build_exchange(&msg, "g");
        assert!(
            ex.input.header("CamelKafkaKey").is_none(),
            "CamelKafkaKey header should be absent for non-UTF-8 key bytes"
        );
    }

    #[test]
    fn test_build_exchange_group_id_header() {
        let msg = make_msg(None, None, "t", Timestamp::NotAvailable, 0, 0);
        let ex = build_exchange(&msg, "my-group");
        assert_eq!(
            ex.input.header("CamelKafkaGroupId"),
            Some(&Value::String("my-group".to_string()))
        );
    }

    // Integration tests — require running Kafka
    // Run with: KAFKA_BROKERS=localhost:9092 cargo test -p camel-component-kafka -- --ignored

    #[tokio::test]
    #[ignore]
    async fn test_consumer_receives_messages() {
        use camel_component::ExchangeEnvelope;
        use std::time::Duration;
        use tokio::sync::mpsc;

        let brokers = std::env::var("KAFKA_BROKERS").unwrap_or("localhost:9092".to_string());
        let config = KafkaConfig::from_uri(&format!(
            "kafka:test-consumer-recv?brokers={}&groupId=test-recv&autoOffsetReset=earliest",
            brokers
        ))
        .unwrap();

        let mut consumer = KafkaConsumer::new(config.clone());
        let ready = consumer.ready_signal();
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let cancel_token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel_token.clone());

        // Start consumer first, then wait for partition assignment before producing
        consumer.start(ctx).await.unwrap();

        // Wait until partitions are assigned (condition-based, no arbitrary sleep)
        tokio::time::timeout(std::time::Duration::from_secs(30), ready.notified())
            .await
            .expect("Consumer should receive partition assignment within 30s");

        let prod_config =
            KafkaConfig::from_uri(&format!("kafka:test-consumer-recv?brokers={}", brokers))
                .unwrap();
        let mut producer = crate::producer::KafkaProducer::new(prod_config).unwrap();
        let msg = camel_api::Message::new(camel_api::Body::Text("hello-kafka".to_string()));
        use tower::Service as _;
        producer.call(camel_api::Exchange::new(msg)).await.unwrap();

        let _envelope = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("Should receive within 10s")
            .expect("Channel not closed");

        cancel_token.cancel();
        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn test_consumer_headers_populated() {
        use camel_component::ExchangeEnvelope;
        use std::time::Duration;
        use tokio::sync::mpsc;

        let brokers = std::env::var("KAFKA_BROKERS").unwrap_or("localhost:9092".to_string());
        let config = KafkaConfig::from_uri(&format!(
            "kafka:test-headers?brokers={}&groupId=test-headers&autoOffsetReset=earliest",
            brokers
        ))
        .unwrap();

        // Start consumer first, then wait for partition assignment before producing
        let mut consumer = KafkaConsumer::new(config.clone());
        let ready = consumer.ready_signal();
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let cancel_token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel_token.clone());
        consumer.start(ctx).await.unwrap();

        // Wait until partitions are assigned (condition-based, no arbitrary sleep)
        tokio::time::timeout(std::time::Duration::from_secs(30), ready.notified())
            .await
            .expect("Consumer should receive partition assignment within 30s");

        let prod_config =
            KafkaConfig::from_uri(&format!("kafka:test-headers?brokers={}", brokers)).unwrap();
        let mut producer = crate::producer::KafkaProducer::new(prod_config).unwrap();
        let msg = camel_api::Message::new(camel_api::Body::Text("headers-test".to_string()));
        use tower::Service as _;
        producer.call(camel_api::Exchange::new(msg)).await.unwrap();

        let envelope = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let ex = &envelope.exchange;

        assert!(ex.input.header("CamelKafkaTopic").is_some());
        assert!(ex.input.header("CamelKafkaPartition").is_some());
        assert!(ex.input.header("CamelKafkaOffset").is_some());
        assert!(ex.input.header("CamelKafkaGroupId").is_some());
        assert!(ex.properties.contains_key("kafka.manual.commit"));

        cancel_token.cancel();
        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn test_consumer_graceful_stop() {
        use camel_component::ExchangeEnvelope;
        use std::time::Duration;
        use tokio::sync::mpsc;

        let brokers = std::env::var("KAFKA_BROKERS").unwrap_or("localhost:9092".to_string());
        let config = KafkaConfig::from_uri(&format!(
            "kafka:test-graceful-stop?brokers={}&groupId=test-stop",
            brokers
        ))
        .unwrap();

        let mut consumer = KafkaConsumer::new(config);
        let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let cancel_token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel_token.clone());

        consumer.start(ctx).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        cancel_token.cancel();
        let result = consumer.stop().await;
        assert!(result.is_ok());
    }

    // ---------------------------------------------------------------------------
    // OTel propagation tests (only compiled with 'otel' feature)
    // ---------------------------------------------------------------------------

    #[cfg(feature = "otel")]
    mod otel_tests {
        use super::*;
        use camel_api::message::Message;
        use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState};
        use opentelemetry::Context;
        use rdkafka::message::{Header, OwnedHeaders};
        use std::collections::HashMap;

        fn make_traceparent(trace_id_hex: &str, span_id_hex: &str, sampled: bool) -> String {
            let flags = if sampled { "01" } else { "00" };
            format!("00-{}-{}-{}", trace_id_hex, span_id_hex, flags)
        }

        #[test]
        fn test_inject_from_exchange_produces_traceparent() {
            // Create an exchange with a valid span context
            let mut exchange = Exchange::new(Message::new(camel_api::Body::Text("test".to_string())));
            
            let trace_id = TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").unwrap();
            let span_id = SpanId::from_hex("00f067aa0ba902b7").unwrap();
            let span_context = SpanContext::new(
                trace_id,
                span_id,
                TraceFlags::SAMPLED,
                true,
                TraceState::default(),
            );
            exchange.otel_context = Context::new().with_remote_span_context(span_context);

            // Inject into a HashMap (simulating what producer does)
            let mut headers_map = HashMap::new();
            camel_otel::inject_from_exchange(&exchange, &mut headers_map);

            // Verify traceparent is present
            assert!(
                headers_map.contains_key("traceparent"),
                "Headers should contain traceparent after injection"
            );

            let traceparent = headers_map.get("traceparent").unwrap();
            assert!(traceparent.starts_with("00-"), "traceparent should start with version 00");
        }

        #[test]
        fn test_extract_into_exchange_populates_otel_context() {
            // Create a headers HashMap with traceparent
            let mut headers_map = HashMap::new();
            let traceparent = make_traceparent("4bf92f3577b34da6a3ce929d0e0e4736", "00f067aa0ba902b7", true);
            headers_map.insert("traceparent".to_string(), traceparent);

            // Create an exchange and extract context
            let mut exchange = Exchange::new(Message::new(camel_api::Body::Text("test".to_string())));
            
            // Verify initial context is invalid
            assert!(
                !exchange.otel_context.span().span_context().is_valid(),
                "Exchange should start with invalid span context"
            );

            // Extract context from headers
            camel_otel::extract_into_exchange(&mut exchange, &headers_map);

            // Verify context is now valid
            assert!(
                exchange.otel_context.span().span_context().is_valid(),
                "Exchange should have valid span context after extraction"
            );
        }

        #[test]
        fn test_kafka_headers_roundtrip() {
            // Simulate the full flow: inject into HashMap -> convert to Kafka headers -> extract back
            
            // Step 1: Create exchange with span context
            let mut exchange = Exchange::new(Message::new(camel_api::Body::Text("test".to_string())));
            let trace_id = TraceId::from_hex("12345678901234567890123456789012").unwrap();
            let span_id = SpanId::from_hex("1234567890123456").unwrap();
            let span_context = SpanContext::new(
                trace_id,
                span_id,
                TraceFlags::SAMPLED,
                true,
                TraceState::default(),
            );
            exchange.otel_context = Context::new().with_remote_span_context(span_context);

            // Step 2: Inject into HashMap (producer logic)
            let mut headers_map = HashMap::new();
            camel_otel::inject_from_exchange(&exchange, &mut headers_map);

            // Step 3: Convert to Kafka OwnedHeaders (producer logic)
            let mut kafka_headers = OwnedHeaders::new();
            for (key, value) in &headers_map {
                kafka_headers = kafka_headers.insert(Header {
                    key,
                    value: Some(value.as_bytes()),
                });
            }

            // Step 4: Extract back from Kafka headers to HashMap (consumer logic)
            let mut extracted_map = HashMap::new();
            for i in 0..kafka_headers.count() {
                let header = kafka_headers.get(i);
                if let Some(value_bytes) = header.value {
                    if let Ok(v) = std::str::from_utf8(value_bytes) {
                        extracted_map.insert(header.key.to_string(), v.to_string());
                    }
                }
            }

            // Step 5: Extract into new exchange (consumer logic)
            let mut new_exchange = Exchange::new(Message::new(camel_api::Body::Text("test".to_string())));
            camel_otel::extract_into_exchange(&mut new_exchange, &extracted_map);

            // Step 6: Verify the span context was preserved
            // Note: span() returns a SpanRef that borrows from context, so we need to bind it
            let original_span = exchange.otel_context.span();
            let original_sc = original_span.span_context();
            let extracted_span = new_exchange.otel_context.span();
            let extracted_sc = extracted_span.span_context();
            
            assert!(extracted_sc.is_valid(), "Extracted span context should be valid");
            assert_eq!(
                original_sc.trace_id(),
                extracted_sc.trace_id(),
                "Trace ID should be preserved"
            );
            assert_eq!(
                original_sc.span_id(),
                extracted_sc.span_id(),
                "Span ID should be preserved"
            );
        }
    }
}
