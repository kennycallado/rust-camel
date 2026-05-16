use async_trait::async_trait;
use camel_component_api::{Body, CamelError, Exchange, Message};
use camel_component_api::{ConcurrencyModel, Consumer, ConsumerContext};
use rdkafka::client::ClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{
    Consumer as RdConsumer, ConsumerContext as RdConsumerContext, Rebalance, StreamConsumer,
};
// Import rdkafka::Message trait to bring .topic(), .key(), .payload(), etc. into scope.
// The alias `_` prevents a name conflict with component Message.
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

use crate::config::ResolvedKafkaEndpointConfig;
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

pub struct KafkaConsumer {
    config: ResolvedKafkaEndpointConfig,
    cancel_token: Option<CancellationToken>,
    task_handle: Option<JoinHandle<Result<(), CamelError>>>,
    /// Notified once the consumer has received its first partition assignment.
    ready: Arc<Notify>,
}

impl KafkaConsumer {
    pub fn new(config: ResolvedKafkaEndpointConfig) -> Self {
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
        // Reject double-start (KAFKA-006)
        if self.cancel_token.is_some() {
            return Err(CamelError::EndpointCreationFailed(
                "Kafka consumer already started".into(),
            ));
        }

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

        let mut result = Ok(());
        if let Some(handle) = self.task_handle.take() {
            match handle.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    error!("Consumer task error: {}", e);
                    result = Err(e);
                }
                Err(e) => {
                    error!("Failed to join consumer task: {}", e);
                    result = Err(CamelError::ProcessorError(format!(
                        "Consumer task panicked during shutdown: {e}"
                    )));
                }
            }
        }

        self.cancel_token = None;
        info!("Kafka consumer stopped");
        result
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
/// CamelKafkaKey (UTF-8), CamelKafkaKeyBytes (base64 for binary keys),
/// CamelKafkaTimestamp (if present), CamelKafkaGroupId
pub fn build_exchange(msg: &OwnedMessage, group_id: &str) -> Exchange {
    let body = resolve_payload_body(msg);
    let mut exchange = Exchange::new(Message::new(body));

    set_core_headers(&mut exchange, msg, group_id);

    if let Some(key_bytes) = msg.key() {
        match std::str::from_utf8(key_bytes) {
            Ok(s) => {
                exchange
                    .input
                    .set_header("CamelKafkaKey", Value::String(s.to_string()));
            }
            Err(_) => {
                // Binary key: preserve as base64 so it is not lost
                let encoded = base64_encode(key_bytes);
                exchange
                    .input
                    .set_header("CamelKafkaKeyBytes", Value::String(encoded));
                warn!(
                    topic = %msg.topic(),
                    "Kafka message key is non-UTF-8 bytes; stored as base64 in CamelKafkaKeyBytes"
                );
            }
        }
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
                if let Some(value_bytes) = header.value
                    && let Ok(v) = std::str::from_utf8(value_bytes)
                {
                    headers_map.insert(header.key.to_string(), v.to_string());
                }
            }
        }
        camel_otel::extract_into_exchange(&mut exchange, &headers_map);
    }

    exchange
}

/// Minimal base64 encoder (no external dependency).
/// Uses the standard alphabet with RFC 4648 padding ('=').
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let chunks = input.chunks_exact(3);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | (chunk[2] as u32);
        out.push(ALPHABET[(n >> 18) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        out.push(ALPHABET[(n & 0x3F) as usize] as char);
    }

    match remainder {
        [a] => {
            let n = (*a as u32) << 16;
            out.push(ALPHABET[(n >> 18) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            out.push('=');
            out.push('=');
        }
        [a, b] => {
            let n = ((*a as u32) << 16) | ((*b as u32) << 8);
            out.push(ALPHABET[(n >> 18) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
            out.push('=');
        }
        _ => {}
    }

    out
}

async fn run_consumer_loop(
    config: ResolvedKafkaEndpointConfig,
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
        .set("bootstrap.servers", &config.brokers)
        .set("group.id", &config.group_id)
        .set("auto.offset.reset", &config.auto_offset_reset)
        .set("session.timeout.ms", config.session_timeout_ms.to_string())
        .set("enable.auto.commit", "false")
        .set("fetch.wait.max.ms", config.poll_timeout_ms.to_string())
        .set("client.id", &config.client_id);

    client_cfg.set(
        "partition.assignment.strategy",
        config.partition_assignment_strategy.to_rdkafka_str(),
    );

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
                if let Err(e) = tpl.add_partition_offset(
                    &req.topic,
                    req.partition,
                    rdkafka::Offset::Offset(req.offset + 1),
                ) {
                    let err = CamelError::ProcessorError(format!(
                        "Invalid topic/partition for commit: {e}"
                    ));
                    if let Some(reply_tx) = req.reply_tx {
                        let _ = reply_tx.send(Err(err));
                    } else {
                        error!(error = %err, "Invalid topic/partition for async commit");
                    }
                    continue;
                }
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

    // KAFKA-016: capped exponential backoff state
    let mut backoff_ms: u64 = 100;
    const MAX_BACKOFF_MS: u64 = 30_000;

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(topic = %config.topic, "Kafka consumer received shutdown signal");
                break;
            }
            msg_result = consumer.recv() => {
                match msg_result {
                    Err(e) => {
                        warn!(error = %e, backoff_ms = backoff_ms, "Kafka consumer error, backing off");
                        // KAFKA-016: capped exponential backoff
                        tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                        backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
                    }
                    Ok(msg) => {
                        // Reset backoff on successful receive
                        backoff_ms = 100;

                        // Must detach before await point (BorrowedMessage not 'static)
                        let owned = msg.detach();
                        let mut exchange = build_exchange(&owned, &config.group_id);

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
                            if let Err(e) = tpl.add_partition_offset(
                                owned.topic(),
                                owned.partition(),
                                rdkafka::Offset::Offset(owned.offset() + 1),
                            ) {
                                warn!(error = %e, "Failed to build TPL for auto-commit; skipping");
                            } else if let Err(e) = consumer.commit(&tpl, CommitMode::Async) {
                                warn!(error = %e, "Failed to commit Kafka offset");
                            }
                        }
                    }
                }
            }
        }
    }

    // KAFKA-003: Graceful shutdown — unsubscribe before dropping
    info!(topic = %config.topic, "Unsubscribing Kafka consumer");
    consumer.unsubscribe();

    // Wait for the commit handler to drain in-flight commits
    if let Some(handle) = commit_handle {
        let timeout = std::time::Duration::from_millis(config.commit_timeout_ms as u64);
        match tokio::time::timeout(timeout, handle).await {
            Ok(Ok(())) => {
                info!("Commit handler drained successfully");
            }
            Ok(Err(e)) => {
                warn!(error = %e, "Commit handler task failed during drain");
            }
            Err(_) => {
                warn!(
                    timeout_ms = config.commit_timeout_ms,
                    "Commit handler drain timed out; pending commits may be lost"
                );
            }
        }
    }

    // KAFKA-003: The consumer is dropped here after unsubscribe.
    // rdkafka handles graceful close on Drop — no explicit close() needed.
    info!(topic = %config.topic, "Kafka consumer dropped after unsubscribe");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KafkaEndpointConfig;
    use rdkafka::Timestamp;

    fn make_resolved_config() -> ResolvedKafkaEndpointConfig {
        KafkaEndpointConfig::from_uri("kafka:test-topic?brokers=localhost:9092&groupId=test-group")
            .unwrap()
            .resolve()
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
        let config = make_resolved_config();
        let consumer = KafkaConsumer::new(config);
        assert!(consumer.cancel_token.is_none());
        assert!(consumer.task_handle.is_none());
    }

    #[test]
    fn test_concurrency_model_is_sequential() {
        let config = make_resolved_config();
        let consumer = KafkaConsumer::new(config);
        assert_eq!(consumer.concurrency_model(), ConcurrencyModel::Sequential);
    }

    #[tokio::test]
    async fn test_consumer_stop_without_start() {
        let config = make_resolved_config();
        let mut consumer = KafkaConsumer::new(config);
        // stop() before start() should be a no-op, not panic
        let result = consumer.stop().await;
        assert!(result.is_ok());
    }

    // --- KAFKA-002: stop() propagates task errors ---

    #[tokio::test]
    async fn test_consumer_stop_propagates_task_error() {
        let config = make_resolved_config();
        let mut consumer = KafkaConsumer::new(config);

        // Simulate a task that returns an error
        let handle = tokio::spawn(async {
            Err(CamelError::ProcessorError(
                "simulated consumer failure".into(),
            ))
        });
        consumer.task_handle = Some(handle);
        consumer.cancel_token = Some(CancellationToken::new());

        let result = consumer.stop().await;
        assert!(result.is_err(), "stop() should propagate task error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("simulated consumer failure"),
            "error message should contain original error: {msg}"
        );
    }

    #[tokio::test]
    async fn test_consumer_stop_propagates_panic() {
        let config = make_resolved_config();
        let mut consumer = KafkaConsumer::new(config);

        // Simulate a task that panics
        let handle = tokio::spawn(async {
            panic!("consumer task panic");
        });
        consumer.task_handle = Some(handle);
        consumer.cancel_token = Some(CancellationToken::new());

        let result = consumer.stop().await;
        assert!(result.is_err(), "stop() should propagate panic as error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("panicked") || msg.contains("Consumer task panicked"),
            "error should mention panic: {msg}"
        );
    }

    // --- KAFKA-006: Consumer double-start guard ---

    #[tokio::test]
    async fn consumer_double_start_returns_error() {
        let config = make_resolved_config();
        let mut consumer = KafkaConsumer::new(config);

        // Simulate an already-started state by setting a cancel token directly.
        consumer.cancel_token = Some(CancellationToken::new());

        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        let result = consumer.start(ctx).await;
        assert!(result.is_err(), "second start must return an error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("already started"),
            "error must mention already started: {}",
            msg
        );
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
        let consumer = KafkaConsumer::new(make_resolved_config());
        let ready_a = consumer.ready_signal();
        let ready_b = consumer.ready_signal();
        assert!(Arc::ptr_eq(&ready_a, &ready_b));
    }

    // --- KAFKA-008: binary key preservation ---

    #[test]
    fn test_build_exchange_binary_key_preserved_as_base64() {
        let binary_key = vec![0xff, 0xfe, 0x01, 0x02];
        let msg = make_msg(
            None,
            Some(binary_key.clone()),
            "t",
            Timestamp::NotAvailable,
            0,
            0,
        );
        let ex = build_exchange(&msg, "g");

        // CamelKafkaKey should NOT be set for binary keys
        assert!(
            ex.input.header("CamelKafkaKey").is_none(),
            "CamelKafkaKey should be absent for non-UTF-8 key bytes"
        );

        // CamelKafkaKeyBytes should contain base64-encoded key
        let key_bytes_header = ex.input.header("CamelKafkaKeyBytes");
        assert!(
            key_bytes_header.is_some(),
            "CamelKafkaKeyBytes should be set for binary keys"
        );
        let encoded = key_bytes_header.unwrap().as_str().unwrap();
        // Verify round-trip: decode base64 and compare
        let decoded = base64_decode(encoded).expect("base64 should decode");
        assert_eq!(decoded, binary_key, "decoded key should match original");
    }

    #[test]
    fn test_build_exchange_utf8_key_still_works() {
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
        // CamelKafkaKeyBytes should NOT be set for UTF-8 keys
        assert!(
            ex.input.header("CamelKafkaKeyBytes").is_none(),
            "CamelKafkaKeyBytes should be absent for UTF-8 keys"
        );
    }

    // --- KAFKA-016: exponential backoff verification ---

    #[test]
    fn test_exponential_backoff_sequence() {
        // Verify the backoff logic: 100 → 200 → 400 → ... → 30000
        const MAX_BACKOFF_MS: u64 = 30_000;
        let mut backoff_ms: u64 = 100;
        let mut sequence = Vec::new();
        for _ in 0..20 {
            sequence.push(backoff_ms);
            backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
        }
        // First few values
        assert_eq!(sequence[0], 100);
        assert_eq!(sequence[1], 200);
        assert_eq!(sequence[2], 400);
        assert_eq!(sequence[3], 800);
        // Caps at MAX_BACKOFF_MS
        assert!(sequence.iter().all(|&v| v <= MAX_BACKOFF_MS));
        // Last values should all be capped
        assert_eq!(sequence[sequence.len() - 1], MAX_BACKOFF_MS);
        assert_eq!(sequence[sequence.len() - 2], MAX_BACKOFF_MS);
    }

    // --- KAFKA-004: commit drain timeout ---

    #[tokio::test]
    async fn test_commit_handler_drain_timeout_observed() {
        // Simulate a slow commit handler that doesn't finish within timeout
        let (tx, mut rx) = mpsc::channel::<CommitRequest>(1);

        // Spawn a handler that sleeps longer than any reasonable timeout
        let handle = tokio::spawn(async move {
            while let Some(_req) = rx.recv().await {
                tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
            }
        });

        // Send a commit request
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        tx.send(CommitRequest {
            topic: "t".into(),
            partition: 0,
            offset: 0,
            reply_tx: Some(reply_tx),
        })
        .await
        .unwrap();

        // Use a short timeout (10ms) — the handler sleeps 5s, so this will timeout
        let timeout = std::time::Duration::from_millis(10);
        let result = tokio::time::timeout(timeout, handle).await;
        assert!(
            result.is_err(),
            "commit handler should timeout with short deadline"
        );
        // Clean up: drop tx to close channel
        drop(tx);
        // The reply will never come, but that's expected
        drop(reply_rx);
    }

    // --- base64 round-trip helper ---

    fn base64_decode(input: &str) -> Option<Vec<u8>> {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut decode_map = [255u8; 256];
        for (i, &b) in alphabet.iter().enumerate() {
            decode_map[b as usize] = i as u8;
        }

        let mut out = Vec::new();
        for chunk in input.as_bytes().chunks_exact(4) {
            let mut vals = [0u8; 4];
            for (i, &b) in chunk.iter().enumerate() {
                if b == b'=' {
                    vals[i] = 0; // padding contributes zero bits
                } else {
                    let v = decode_map[b as usize];
                    if v == 255 {
                        return None;
                    }
                    vals[i] = v;
                }
            }
            let n = ((vals[0] as u32) << 18)
                | ((vals[1] as u32) << 12)
                | ((vals[2] as u32) << 6)
                | (vals[3] as u32);
            out.push((n >> 16) as u8);
            if chunk[2] != b'=' {
                out.push(((n >> 8) & 0xFF) as u8);
            }
            if chunk[3] != b'=' {
                out.push((n & 0xFF) as u8);
            }
        }

        Some(out)
    }

    // ---------------------------------------------------------------------------
    // OTel propagation tests (only compiled with 'otel' feature)
    // ---------------------------------------------------------------------------

    #[cfg(feature = "otel")]
    mod otel_tests {
        use super::*;
        use camel_component_api::Message;
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
            let mut exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
                "test".to_string(),
            )));

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
            let mut exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
                "test".to_string(),
            )));

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
            let mut exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
                "test".to_string(),
            )));
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
                if let Some(value_bytes) = header.value
                    && let Ok(v) = std::str::from_utf8(value_bytes)
                {
                    extracted_map.insert(header.key.to_string(), v.to_string());
                }
            }

            // Step 5: Extract into new exchange (consumer logic)
            let mut new_exchange = Exchange::new(Message::new(camel_component_api::Body::Text(
                "test".to_string(),
            )));
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
