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
#[cfg(feature = "otel")]
use rdkafka::message::Headers as _;
use rdkafka::message::Message as _;
use rdkafka::message::OwnedMessage;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::apply_security_config;
use crate::manual_commit::{CommitRequest, KafkaManualCommit};

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

use crate::config::KafkaEndpointConfig;

pub struct KafkaConsumer {
    config: KafkaEndpointConfig,
    cancel_token: Option<CancellationToken>,
    task_handle: Option<JoinHandle<Result<(), CamelError>>>,
    /// Notified once the consumer has received its first partition assignment.
    ready: Arc<Notify>,
}

impl KafkaConsumer {
    pub fn new(config: KafkaEndpointConfig) -> Self {
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
            brokers = %config.brokers.as_deref().unwrap_or("<not set>"),
            group_id = %config.group_id.as_deref().unwrap_or("<not set>"),
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

pub fn resolve_payload_body(msg: &OwnedMessage) -> Body {
    match msg.payload() {
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(s) => Body::Text(s.to_string()),
            Err(_) => Body::Bytes(bytes::Bytes::copy_from_slice(bytes)),
        },
        None => Body::Empty,
    }
}

pub fn resolve_utf8_key(msg: &OwnedMessage) -> Option<String> {
    msg.key()
        .and_then(|key_bytes| std::str::from_utf8(key_bytes).ok())
        .map(|s| s.to_string())
}

pub fn resolve_timestamp_millis(msg: &OwnedMessage) -> Option<i64> {
    msg.timestamp().to_millis()
}

fn set_core_headers(exchange: &mut Exchange, msg: &OwnedMessage, group_id: &str) {
    exchange
        .input
        .set_header("CamelKafkaTopic", Value::String(msg.topic().to_string()));
    exchange
        .input
        .set_header("CamelKafkaPartition", Value::Number(msg.partition().into()));
    exchange
        .input
        .set_header("CamelKafkaOffset", Value::Number(msg.offset().into()));
    exchange
        .input
        .set_header("CamelKafkaGroupId", Value::String(group_id.to_string()));
}

/// Build an Exchange from an OwnedMessage.
///
/// Sets headers: CamelKafkaTopic, CamelKafkaPartition, CamelKafkaOffset,
/// CamelKafkaKey (if present), CamelKafkaTimestamp (if present), CamelKafkaGroupId
pub fn build_exchange(msg: &OwnedMessage, group_id: &str) -> Exchange {
    let body = resolve_payload_body(msg);
    let mut exchange = Exchange::new(Message::new(body));

    set_core_headers(&mut exchange, msg, group_id);

    if let Some(key) = resolve_utf8_key(msg) {
        exchange
            .input
            .set_header("CamelKafkaKey", Value::String(key));
    } else if msg.key().is_some() {
        warn!(
            topic = %msg.topic(),
            "Kafka message key is non-UTF-8 bytes; CamelKafkaKey header not set"
        );
    }

    if let Some(ts) = resolve_timestamp_millis(msg) {
        exchange
            .input
            .set_header("CamelKafkaTimestamp", Value::Number(ts.into()));
    }

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
    config: KafkaEndpointConfig,
    ctx: ConsumerContext,
    cancel_token: CancellationToken,
    ready: Arc<Notify>,
) -> Result<(), CamelError> {
    use rdkafka::consumer::CommitMode;

    let (commit_tx, commit_rx): (
        Option<mpsc::Sender<CommitRequest>>,
        Option<mpsc::Receiver<CommitRequest>>,
    ) = if config.allow_manual_commit {
        let (tx, rx) = mpsc::channel(32);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let mut client_cfg = ClientConfig::new();
    client_cfg
        .set(
            "bootstrap.servers",
            config.brokers.as_ref().expect("brokers must be resolved"),
        )
        .set(
            "group.id",
            config.group_id.as_ref().expect("group_id must be resolved"),
        )
        .set(
            "auto.offset.reset",
            config
                .auto_offset_reset
                .as_ref()
                .expect("auto_offset_reset must be resolved"),
        )
        .set(
            "session.timeout.ms",
            config
                .session_timeout_ms
                .expect("session_timeout_ms must be resolved")
                .to_string(),
        )
        .set("enable.auto.commit", "false")
        .set("fetch.wait.max.ms", config.poll_timeout_ms.to_string());

    apply_security_config(&config, &mut client_cfg);

    let consumer: ReadyStreamConsumer = client_cfg
        .create_with_context(ReadyContext { ready })
        .map_err(|e| {
            CamelError::ProcessorError(format!("Failed to create Kafka consumer: {}", e))
        })?;

    // Wrap in Arc to share between main loop and commit handler task
    let consumer = Arc::new(consumer);

    consumer.subscribe(&[config.topic.as_str()]).map_err(|e| {
        CamelError::ProcessorError(format!(
            "Failed to subscribe to topic '{}': {}",
            config.topic, e
        ))
    })?;

    info!(topic = %config.topic, "Kafka consumer subscribed");

    // Spawn commit handler task if manual commit is enabled
    let commit_handle = if let Some(mut rx) = commit_rx {
        let consumer_for_commits = consumer.clone();
        Some(tokio::spawn(async move {
            use rdkafka::TopicPartitionList;
            use rdkafka::consumer::CommitMode;
            while let Some(req) = rx.recv().await {
                let mut tpl = TopicPartitionList::new();
                tpl.add_partition_offset(
                    &req.topic,
                    req.partition,
                    rdkafka::Offset::Offset(req.offset + 1),
                )
                .expect("topic/partition from valid message should never fail");
                let result = consumer_for_commits
                    .commit(&tpl, CommitMode::Sync)
                    .map_err(|e| CamelError::ProcessorError(format!("Commit failed: {e}")));
                if let Some(reply_tx) = req.reply_tx {
                    let _ = reply_tx.send(result);
                } else if let Err(ref e) = result {
                    error!(error = %e, "Async Kafka commit failed");
                }
            }
        }))
    } else {
        None
    };

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
                        let group_id = config.group_id.as_deref().expect("group_id must be resolved");
                        let mut exchange = build_exchange(&owned, group_id);

                        if let Some(ref tx) = commit_tx {
                            // Manual commit mode: store handle in extensions, user is responsible for commit
                            let handle = KafkaManualCommit::new(
                                owned.topic().to_string(),
                                owned.partition(),
                                owned.offset(),
                                tx.clone(),
                            );
                            exchange.set_extension("kafka.manual_commit", Arc::new(handle));
                            if let Err(e) = ctx.send(exchange).await {
                                error!(error = %e, "Failed to send exchange to pipeline");
                            }
                        } else {
                            // Auto-commit mode: dispatch then commit (at-least-once semantics)
                            if let Err(e) = ctx.send(exchange).await {
                                error!(error = %e, "Failed to send exchange to pipeline");
                            }
                            let mut tpl = rdkafka::TopicPartitionList::new();
                            tpl.add_partition_offset(
                                owned.topic(),
                                owned.partition(),
                                rdkafka::Offset::Offset(owned.offset() + 1),
                            ).expect("topic/partition from valid message should never fail");
                            if let Err(e) = consumer.commit(&tpl, CommitMode::Async) {
                                warn!(error = %e, "Failed to commit Kafka offset");
                            }
                        }
                    }
                }
            }
        }
    }

    // Wait for the commit handler to drain in-flight commits
    if let Some(handle) = commit_handle {
        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), handle).await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdkafka::Timestamp;

    fn make_config() -> KafkaEndpointConfig {
        KafkaEndpointConfig::from_uri("kafka:test-topic?brokers=localhost:9092&groupId=test-group")
            .unwrap()
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
    fn test_resolve_payload_body_variants() {
        let msg_empty = make_msg(None, None, "t", Timestamp::NotAvailable, 0, 0);
        assert!(matches!(resolve_payload_body(&msg_empty), Body::Empty));

        let msg_text = make_msg(
            Some(b"hello".to_vec()),
            None,
            "t",
            Timestamp::NotAvailable,
            0,
            0,
        );
        assert!(matches!(resolve_payload_body(&msg_text), Body::Text(ref s) if s == "hello"));

        let msg_bin = make_msg(
            Some(vec![0xFF, 0x00]),
            None,
            "t",
            Timestamp::NotAvailable,
            0,
            0,
        );
        assert!(matches!(resolve_payload_body(&msg_bin), Body::Bytes(_)));
    }

    #[test]
    fn test_resolve_utf8_key_variants() {
        let msg_utf8 = make_msg(
            None,
            Some(b"my-key".to_vec()),
            "t",
            Timestamp::NotAvailable,
            0,
            0,
        );
        assert_eq!(resolve_utf8_key(&msg_utf8), Some("my-key".to_string()));

        let msg_bin = make_msg(
            None,
            Some(vec![0xFF, 0xFE]),
            "t",
            Timestamp::NotAvailable,
            0,
            0,
        );
        assert_eq!(resolve_utf8_key(&msg_bin), None);
    }

    #[test]
    fn test_resolve_timestamp_millis_variants() {
        let msg_ts = make_msg(None, None, "t", Timestamp::CreateTime(777), 0, 0);
        assert_eq!(resolve_timestamp_millis(&msg_ts), Some(777));

        let msg_none = make_msg(None, None, "t", Timestamp::NotAvailable, 0, 0);
        assert_eq!(resolve_timestamp_millis(&msg_none), None);
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

    #[test]
    fn test_build_exchange_sets_core_metadata_headers() {
        let msg = make_msg(None, None, "orders", Timestamp::NotAvailable, 7, 42);
        let ex = build_exchange(&msg, "group-a");

        assert_eq!(
            ex.input.header("CamelKafkaTopic"),
            Some(&Value::String("orders".to_string()))
        );
        assert_eq!(
            ex.input.header("CamelKafkaPartition"),
            Some(&Value::Number(7.into()))
        );
        assert_eq!(
            ex.input.header("CamelKafkaOffset"),
            Some(&Value::Number(42.into()))
        );
    }

    #[test]
    fn test_build_exchange_sets_timestamp_when_available() {
        let msg = make_msg(None, None, "t", Timestamp::CreateTime(123456), 0, 0);
        let ex = build_exchange(&msg, "g");
        assert_eq!(
            ex.input.header("CamelKafkaTimestamp"),
            Some(&Value::Number(123456.into()))
        );
    }

    #[test]
    fn test_ready_signal_returns_shared_notify_handle() {
        let consumer = KafkaConsumer::new(make_config());
        let ready_a = consumer.ready_signal();
        let ready_b = consumer.ready_signal();
        assert!(Arc::ptr_eq(&ready_a, &ready_b));
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
        let config = KafkaEndpointConfig::from_uri(&format!(
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
            KafkaEndpointConfig::from_uri(&format!("kafka:test-consumer-recv?brokers={}", brokers))
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
        let config = KafkaEndpointConfig::from_uri(&format!(
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
            KafkaEndpointConfig::from_uri(&format!("kafka:test-headers?brokers={}", brokers))
                .unwrap();
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
        let config = KafkaEndpointConfig::from_uri(&format!(
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
        use opentelemetry::Context;
        use opentelemetry::trace::{
            SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState,
        };
        use rdkafka::message::{Header, OwnedHeaders};
        use std::collections::HashMap;

        fn make_traceparent(trace_id_hex: &str, span_id_hex: &str, sampled: bool) -> String {
            let flags = if sampled { "01" } else { "00" };
            format!("00-{}-{}-{}", trace_id_hex, span_id_hex, flags)
        }

        #[test]
        fn test_inject_from_exchange_produces_traceparent() {
            // Create an exchange with a valid span context
            let mut exchange =
                Exchange::new(Message::new(camel_api::Body::Text("test".to_string())));

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
            assert!(
                traceparent.starts_with("00-"),
                "traceparent should start with version 00"
            );
        }

        #[test]
        fn test_extract_into_exchange_populates_otel_context() {
            // Create a headers HashMap with traceparent
            let mut headers_map = HashMap::new();
            let traceparent =
                make_traceparent("4bf92f3577b34da6a3ce929d0e0e4736", "00f067aa0ba902b7", true);
            headers_map.insert("traceparent".to_string(), traceparent);

            // Create an exchange and extract context
            let mut exchange =
                Exchange::new(Message::new(camel_api::Body::Text("test".to_string())));

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
            let mut exchange =
                Exchange::new(Message::new(camel_api::Body::Text("test".to_string())));
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
            let mut new_exchange =
                Exchange::new(Message::new(camel_api::Body::Text("test".to_string())));
            camel_otel::extract_into_exchange(&mut new_exchange, &extracted_map);

            // Step 6: Verify the span context was preserved
            // Note: span() returns a SpanRef that borrows from context, so we need to bind it
            let original_span = exchange.otel_context.span();
            let original_sc = original_span.span_context();
            let extracted_span = new_exchange.otel_context.span();
            let extracted_sc = extracted_span.span_context();

            assert!(
                extracted_sc.is_valid(),
                "Extracted span context should be valid"
            );
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
