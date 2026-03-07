use async_trait::async_trait;
use camel_api::{Body, CamelError, Exchange, Message};
use camel_component::{ConcurrencyModel, Consumer, ConsumerContext};
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer as RdConsumer, StreamConsumer};
// Import rdkafka::Message trait to bring .topic(), .key(), .payload(), etc. into scope.
// The alias `_` prevents a name conflict with camel_api::Message.
use rdkafka::message::Message as _;
use rdkafka::message::OwnedMessage;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

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

    exchange
}

async fn run_consumer_loop(
    config: KafkaConfig,
    ctx: ConsumerContext,
    cancel_token: CancellationToken,
    ready: Arc<Notify>,
) -> Result<(), CamelError> {
    use rdkafka::consumer::CommitMode;

    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &config.brokers)
        .set("group.id", &config.group_id)
        .set("auto.offset.reset", &config.auto_offset_reset)
        .set("session.timeout.ms", config.session_timeout_ms.to_string())
        .set("enable.auto.commit", "false")
        .set("fetch.wait.max.ms", config.poll_timeout_ms.to_string())
        .set("max.poll.records", config.max_poll_records.to_string())
        .create()
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

    // Poll assignment in background until partitions are assigned, then notify.
    // This allows callers holding a `ready` handle to wait without arbitrary sleeps.
    {
        let consumer_ref = &consumer;
        let ready_ref = ready.clone();
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(50));
        loop {
            interval.tick().await;
            if cancel_token.is_cancelled() {
                return Ok(());
            }
            match consumer_ref.assignment() {
                Ok(tpl) if tpl.count() > 0 => {
                    ready_ref.notify_waiters();
                    break;
                }
                _ => {}
            }
        }
    }

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
}
