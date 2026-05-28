use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{BackoffConfig, BackoffState};
use camel_component_api::{
    Body, CamelError, ConcurrencyModel, Consumer, ConsumerContext, Exchange, Message,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::component::{
    BRIDGE_TRANSPORT_ERROR_PREFIX, BridgeState, JmsBridgePool, is_bridge_transport_error,
};
use crate::config::{DestinationType, ExchangePattern, JmsEndpointConfig, JmsTransactionMode};
use crate::headers::apply_jms_headers;
use crate::proto::{JmsMessage, SubscribeRequest, bridge_service_client::BridgeServiceClient};

pub struct JmsConsumer {
    pool: Arc<JmsBridgePool>,
    broker_name: String,
    endpoint_config: JmsEndpointConfig,
    reconnect_interval_ms: u64,
    cancel_token: Option<CancellationToken>,
    task_handles: Vec<JoinHandle<Result<(), CamelError>>>,
}

impl JmsConsumer {
    pub fn new(
        pool: Arc<JmsBridgePool>,
        broker_name: String,
        endpoint_config: JmsEndpointConfig,
        reconnect_interval_ms: u64,
    ) -> Self {
        Self {
            pool,
            broker_name,
            endpoint_config,
            reconnect_interval_ms,
            cancel_token: None,
            task_handles: Vec::new(),
        }
    }
}

fn build_exchange(msg: &JmsMessage, map_jms_headers: bool) -> Exchange {
    let body_bytes = msg.body.clone();
    let body = if msg.content_type.starts_with("text/") {
        match String::from_utf8(body_bytes.clone()) {
            Ok(s) => Body::Text(s),
            Err(_) => Body::Bytes(bytes::Bytes::from(body_bytes)),
        }
    } else if msg.content_type.contains("json") {
        match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
            Ok(v) => Body::Json(v),
            Err(_) => Body::Bytes(bytes::Bytes::from(body_bytes)),
        }
    } else if body_bytes.is_empty() {
        Body::Empty
    } else {
        Body::Bytes(bytes::Bytes::from(body_bytes))
    };

    let mut exchange = Exchange::new(Message::new(body));
    // JMS-018: gate header mapping behind config flag
    if map_jms_headers {
        apply_jms_headers(&mut exchange, msg);
    }
    exchange
}

fn destination(endpoint_config: &JmsEndpointConfig) -> String {
    format!(
        "{}:{}",
        match endpoint_config.destination_type {
            DestinationType::Queue => "queue",
            DestinationType::Topic => "topic",
        },
        endpoint_config.destination_name
    )
}

async fn await_ready_channel(
    pool: &JmsBridgePool,
    broker_name: &str,
) -> Result<Channel, CamelError> {
    let slot = pool.get_or_create_slot(broker_name).await?;
    let mut rx = slot.state_rx.clone();

    loop {
        match &*rx.borrow() {
            BridgeState::Ready { channel } => return Ok(channel.clone()),
            BridgeState::Stopped => {
                return Err(CamelError::ProcessorError(format!(
                    "JMS broker '{}' is stopped",
                    broker_name
                )));
            }
            _ => {}
        }

        if rx.changed().await.is_err() {
            return Err(CamelError::ProcessorError(format!(
                "JMS broker '{}' state channel closed",
                broker_name
            )));
        }
    }
}

/// Background consumer loop, extracted so it can be spawned multiple times
/// for concurrent consumers (JMS-011).
async fn consumer_loop(
    pool: &JmsBridgePool,
    broker_name: &str,
    endpoint_config: &JmsEndpointConfig,
    reconnect_interval_ms: u64,
    cancel: CancellationToken,
    ctx: &ConsumerContext,
    idx: u32,
) {
    let destination = destination(endpoint_config);
    let map_headers = endpoint_config.map_jms_headers;
    let selector = endpoint_config.message_selector.clone();
    let mut consecutive_transport_failures: u32 = 0;
    let mut backoff = BackoffState::new(BackoffConfig {
        initial_delay: Duration::from_millis(reconnect_interval_ms),
        multiplier: 2.0,
        max_delay: Duration::from_secs(30),
    });

    // JMS-010: message selector — pass to bridge subscription when available
    // TODO(JMS-010): pass selector to bridge subscription
    let _selector = selector;

    loop {
        let channel = tokio::select! {
            _ = cancel.cancelled() => {
                info!(
                    broker = %broker_name,
                    destination = %destination,
                    consumer_idx = idx,
                    "JMS consumer cancelled"
                );
                break;
            }
            _ = ctx.cancelled() => {
                info!(
                    broker = %broker_name,
                    destination = %destination,
                    consumer_idx = idx,
                    "JMS consumer context cancelled"
                );
                break;
            }
            result = await_ready_channel(pool, broker_name) => {
                match result {
                    Ok(channel) => channel,
                    Err(e) => {
                        warn!(
                            broker = %broker_name,
                            destination = %destination,
                            consumer_idx = idx,
                            error = %e,
                            "JMS consumer waiting for ready bridge failed"
                        );
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = ctx.cancelled() => break,
                            _ = tokio::time::sleep(backoff.next_delay()) => {}
                        }
                        continue;
                    }
                }
            }
        };

        let mut client = BridgeServiceClient::new(channel);
        let mut stream = match client
            .subscribe(SubscribeRequest {
                destination: destination.clone(),
                subscription_id: Uuid::new_v4().to_string(),
            })
            .await
            .map_err(|e| {
                CamelError::ProcessorError(format!(
                    "{BRIDGE_TRANSPORT_ERROR_PREFIX}subscribe error: {e}"
                ))
            }) {
            Ok(resp) => {
                consecutive_transport_failures = 0;
                backoff.reset();
                info!(
                    broker = %broker_name,
                    destination = %destination,
                    consumer_idx = idx,
                    "JMS consumer subscribed successfully"
                );
                resp.into_inner()
            }
            Err(e) => {
                if is_bridge_transport_error(&e) {
                    consecutive_transport_failures += 1;
                    if consecutive_transport_failures >= 2 {
                        warn!(
                            broker = %broker_name,
                            destination = %destination,
                            consumer_idx = idx,
                            failures = consecutive_transport_failures,
                            "JMS subscribe transport failures exceeded threshold; refreshing channel"
                        );
                        if let Err(refresh_err) = pool.refresh_slot_channel(broker_name).await {
                            warn!(
                                broker = %broker_name,
                                destination = %destination,
                                consumer_idx = idx,
                                error = %refresh_err,
                                "JMS channel refresh failed; requesting bridge restart"
                            );
                            pool.restart_slot(broker_name);
                        }
                        consecutive_transport_failures = 0;
                    }
                } else {
                    consecutive_transport_failures = 0;
                }
                warn!(
                    broker = %broker_name,
                    destination = %destination,
                    consumer_idx = idx,
                    error = %e,
                    "JMS subscribe failed; retrying"
                );
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ctx.cancelled() => break,
                    _ = tokio::time::sleep(backoff.next_delay()) => {}
                }
                continue;
            }
        };

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!(
                        broker = %broker_name,
                        destination = %destination,
                        consumer_idx = idx,
                        "JMS consumer cancelled"
                    );
                    return;
                }
                _ = ctx.cancelled() => {
                    info!(
                        broker = %broker_name,
                        destination = %destination,
                        consumer_idx = idx,
                        "JMS consumer context cancelled"
                    );
                    return;
                }
                msg = stream.message() => {
                    match msg {
                        Ok(Some(jms_msg)) => {
                            consecutive_transport_failures = 0;
                            backoff.reset();
                            let exchange = build_exchange(&jms_msg, map_headers);
                            if let Err(e) = ctx.send(exchange).await {
                                error!(
                                    broker = %broker_name,
                                    consumer_idx = idx,
                                    "JMS consumer route error: {e}"
                                );
                            }
                        }
                        Ok(None) => {
                            info!(
                                broker = %broker_name,
                                destination = %destination,
                                consumer_idx = idx,
                                "JMS stream ended; reconnecting"
                            );
                            break;
                        }
                        Err(e) => {
                            let subscribe_err = CamelError::ProcessorError(format!(
                                "{BRIDGE_TRANSPORT_ERROR_PREFIX}subscribe error: {e}"
                            ));
                            if is_bridge_transport_error(&subscribe_err) {
                                consecutive_transport_failures += 1;
                                if consecutive_transport_failures >= 2 {
                                    warn!(
                                        broker = %broker_name,
                                        destination = %destination,
                                        consumer_idx = idx,
                                        failures = consecutive_transport_failures,
                                        "JMS stream transport failures exceeded threshold; refreshing channel"
                                    );
                                    if let Err(refresh_err) =
                                        pool.refresh_slot_channel(broker_name).await
                                    {
                                        warn!(
                                            broker = %broker_name,
                                            destination = %destination,
                                            consumer_idx = idx,
                                            error = %refresh_err,
                                            "JMS channel refresh failed; requesting bridge restart"
                                        );
                                        pool.restart_slot(broker_name);
                                    }
                                    consecutive_transport_failures = 0;
                                }
                            } else {
                                consecutive_transport_failures = 0;
                            }
                            warn!(
                                broker = %broker_name,
                                destination = %destination,
                                consumer_idx = idx,
                                error = %subscribe_err,
                                "JMS stream error; reconnecting"
                            );
                            break;
                        }
                    }
                }
            }
        }

        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ctx.cancelled() => break,
            _ = tokio::time::sleep(backoff.next_delay()) => {}
        }
    }
}

#[async_trait]
impl Consumer for JmsConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        // Reject double-start (JMS-006)
        if self.cancel_token.is_some() {
            return Err(CamelError::EndpointCreationFailed(
                "JMS consumer already started".into(),
            ));
        }

        // JMS-012: warn when session transaction mode requested but not implemented
        if self.endpoint_config.transaction_mode == JmsTransactionMode::Session {
            warn!("JMS session transaction mode not yet implemented; using None");
        }

        // JMS-005: warn when InOut pattern requested but not implemented
        if self.endpoint_config.exchange_pattern == ExchangePattern::InOut {
            warn!("JMS InOut pattern not yet implemented");
        }

        // JMS-017: Pre-flight check — fail fast when bridge is unavailable.
        // Probe the bridge slot before spawning the background loop so that
        // missing bridge binaries or unreachable brokers are reported immediately
        // instead of looping silently.
        {
            let slot = self.pool.get_or_create_slot(&self.broker_name).await?;
            match &*slot.state_rx.borrow() {
                BridgeState::Ready { .. } => {} // bridge is up — proceed
                BridgeState::Degraded(reason) => {
                    return Err(CamelError::ProcessorError(format!(
                        "JMS bridge not available: {}",
                        reason
                    )));
                }
                other => {
                    return Err(CamelError::ProcessorError(format!(
                        "JMS bridge not available: {:?}",
                        other
                    )));
                }
            }
        }

        let pool = Arc::clone(&self.pool);
        let broker_name = self.broker_name.clone();
        let endpoint_config = self.endpoint_config.clone();
        let reconnect_interval_ms = self.reconnect_interval_ms;
        let cancel = CancellationToken::new();
        self.cancel_token = Some(cancel.clone());

        // JMS-011: spawn concurrent consumer tasks
        let consumer_count = endpoint_config.concurrent_consumers;
        // allow-unwrap: consumer_count validated >= 1 in from_uri
        let handles: Vec<JoinHandle<Result<(), CamelError>>> = (0..consumer_count)
            .map(|idx| {
                let pool = Arc::clone(&pool);
                let broker_name = broker_name.clone();
                let endpoint_config = endpoint_config.clone();
                let cancel = cancel.clone();
                let ctx = ctx.clone();

                tokio::spawn(async move {
                    consumer_loop(
                        &pool,
                        &broker_name,
                        &endpoint_config,
                        reconnect_interval_ms,
                        cancel,
                        &ctx,
                        idx,
                    )
                    .await;
                    Ok(())
                })
            })
            .collect();

        // Store all handles so stop() can abort/join every concurrent consumer.
        self.task_handles = handles;

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(cancel) = self.cancel_token.take() {
            cancel.cancel();
        }
        let handles = std::mem::take(&mut self.task_handles);
        for mut handle in handles {
            if tokio::time::timeout(Duration::from_secs(5), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
                let _ = handle.await;
                warn!("JMS consumer task did not stop in 5s; aborted");
            }
        }
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }

    fn background_task_handle(
        &mut self,
    ) -> Option<tokio::task::JoinHandle<Result<(), CamelError>>> {
        // JMS may have multiple concurrent consumer handles; return the first.
        // The remaining handles are joined in stop().
        self.task_handles.pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrokerType;
    use crate::config::JmsPoolConfig;
    use tokio::sync::mpsc;

    #[test]
    fn build_exchange_text_body() {
        let msg = JmsMessage {
            message_id: "ID:1".to_string(),
            body: b"hello world".to_vec(),
            content_type: "text/plain".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(&msg, true);
        assert!(matches!(ex.input.body, Body::Text(_)));
    }

    #[test]
    fn build_exchange_binary_body() {
        let msg = JmsMessage {
            message_id: "ID:2".to_string(),
            body: vec![0x00, 0x01, 0x02],
            content_type: "application/octet-stream".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(&msg, true);
        assert!(matches!(ex.input.body, Body::Bytes(_)));
    }

    #[test]
    fn build_exchange_json_body() {
        let msg = JmsMessage {
            message_id: "ID:json".to_string(),
            body: br#"{"ok":true}"#.to_vec(),
            content_type: "application/json".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(&msg, true);
        assert!(matches!(ex.input.body, Body::Json(_)));
    }

    #[test]
    fn build_exchange_empty_body() {
        let msg = JmsMessage {
            message_id: "ID:3".to_string(),
            body: vec![],
            content_type: "".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(&msg, true);
        assert!(matches!(ex.input.body, Body::Empty));
    }

    #[test]
    fn build_exchange_without_header_mapping() {
        // JMS-018: when map_jms_headers is false, JMS headers must not be set
        let msg = JmsMessage {
            message_id: "ID:header-test".to_string(),
            correlation_id: "CORR:123".to_string(),
            timestamp: 1700000000,
            destination: "queue:orders".to_string(),
            body: b"hello".to_vec(),
            headers: Default::default(),
            content_type: "text/plain".to_string(),
        };
        let ex = build_exchange(&msg, false);
        assert!(ex.input.header("JMSMessageID").is_none());
        assert!(ex.input.header("JMSCorrelationID").is_none());
        assert!(ex.input.header("JMSTimestamp").is_none());
        assert!(ex.input.header("JMSDestination").is_none());
        assert!(ex.input.header("Content-Type").is_none());
    }

    #[tokio::test]
    async fn stop_without_start_is_noop() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(pool, "default".to_string(), endpoint_cfg, 50);
        assert!(consumer.stop().await.is_ok());
    }

    // ── JMS-006: Consumer double-start guard ──────────────────────────────────

    #[tokio::test]
    async fn consumer_double_start_returns_error() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(pool, "default".to_string(), endpoint_cfg, 50);

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

    // ── JMS-002: Consumer stop joins task handle ─────────────────────────────

    #[tokio::test]
    async fn test_jms_consumer_stop_joins() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(pool, "default".to_string(), endpoint_cfg, 50);

        // Simulate a started consumer with a task that respects cancellation.
        let cancel = CancellationToken::new();
        consumer.cancel_token = Some(cancel.clone());
        consumer.task_handles = vec![tokio::spawn({
            let cancel = cancel.clone();
            async move {
                // Task waits for cancellation then exits cleanly.
                cancel.cancelled().await;
                Ok(())
            }
        })];

        // stop() must not panic and must join the task.
        let result = consumer.stop().await;
        assert!(result.is_ok(), "stop must succeed: {:?}", result.err());
        // Verify the handles were taken (tasks were joined).
        assert!(
            consumer.task_handles.is_empty(),
            "task_handles must be cleared after stop"
        );
    }

    // ── JMS-002: Consumer task panic is absorbed (Pattern S) ─────────────────

    #[tokio::test]
    async fn stop_absorbs_consumer_task_panic() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(pool, "default".to_string(), endpoint_cfg, 50);

        // Manually set a task handle that will panic.
        consumer.task_handles = vec![tokio::spawn(async {
            panic!("simulated consumer panic");
        })];
        // Give the panic time to materialize.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Pattern S: panic is absorbed, stop returns Ok.
        let result = consumer.stop().await;
        assert!(
            result.is_ok(),
            "stop must absorb panic and return Ok: {:?}",
            result.err()
        );
    }

    // ── JMS-017: Missing bridge returns Err immediately ───────────────────────

    #[tokio::test]
    async fn missing_bridge_returns_err_within_1s() {
        struct EnvGuard {
            key: &'static str,
            prev: Option<std::ffi::OsString>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                if let Some(v) = &self.prev {
                    unsafe { std::env::set_var(self.key, v) };
                } else {
                    unsafe { std::env::remove_var(self.key) };
                }
            }
        }

        let env_key = "CAMEL_JMS_BRIDGE_BINARY_PATH";
        let _guard = EnvGuard {
            key: env_key,
            prev: std::env::var_os(env_key),
        };
        unsafe { std::env::set_var(env_key, "/bin/false") };

        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig {
                brokers: std::collections::HashMap::from([(
                    "default".to_string(),
                    crate::config::BrokerConfig {
                        broker_url: "tcp://localhost:61616".to_string(),
                        broker_type: BrokerType::ActiveMq,
                        username: None,
                        password: None,
                    },
                )]),
                bridge_start_timeout_ms: 100,
                ..JmsPoolConfig::default()
            })
            .unwrap(),
        );
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(pool, "default".to_string(), endpoint_cfg, 50);

        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());

        let start = std::time::Instant::now();
        let result = consumer.start(ctx).await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "expected Err when bridge missing, got Ok");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("JMS bridge not available"),
            "error must mention bridge unavailability: {}",
            msg
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "must fail within 1s, took {:?}",
            elapsed
        );

        // Cleanup: stop the health monitors etc.
        let _ = consumer.stop().await;
    }
}
