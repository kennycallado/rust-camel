use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_component_api::{
    Body, CamelError, ConcurrencyModel, Consumer, ConsumerContext, Exchange, Message,
    NetworkRetryPolicy, RuntimeObservability,
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
    reconnect: NetworkRetryPolicy,
    cancel_token: Option<CancellationToken>,
    task_handles: Vec<JoinHandle<Result<(), CamelError>>>,
    /// Phase C ADR-0012: used for `metrics().increment_errors(…)` at
    /// consumer-send failure sites.
    runtime: Arc<dyn RuntimeObservability>,
}

impl JmsConsumer {
    pub fn new(
        pool: Arc<JmsBridgePool>,
        broker_name: String,
        endpoint_config: JmsEndpointConfig,
        reconnect: NetworkRetryPolicy,
        runtime: Arc<dyn RuntimeObservability>,
    ) -> Self {
        Self {
            pool,
            broker_name,
            endpoint_config,
            reconnect,
            cancel_token: None,
            task_handles: Vec::new(),
            runtime,
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
#[allow(clippy::too_many_arguments)]
async fn consumer_loop(
    pool: &JmsBridgePool,
    broker_name: &str,
    endpoint_config: &JmsEndpointConfig,
    reconnect: &NetworkRetryPolicy,
    cancel: CancellationToken,
    ctx: &ConsumerContext,
    idx: u32,
    runtime: Arc<dyn RuntimeObservability>,
) {
    let destination = destination(endpoint_config);
    let map_headers = endpoint_config.map_jms_headers;
    let selector = endpoint_config.message_selector.clone();
    let mut consecutive_transport_failures: u32 = 0;
    let mut attempt: u32 = 0;

    // JMS-010: message selector — pass to bridge subscription when available
    // TODO(JMS-010): pass selector to bridge subscription
    let _selector = selector;

    // Manual retry loops (not retry_async / retry_async_cancelable) — three
    // retry sites nested inside this outer loop:
    // - Retries are embedded in `tokio::select!` with cancellation tokens
    //   (cancel.cancelled(), ctx.cancelled()); retry_async_cancelable
    //   honours a single CancellationToken but does not interleave
    //   cancellation checks between every operation step.
    // - Inter-attempt side effects involve async lifecycle orchestration:
    //   transport failure counting, pool.restart_slot() for channel
    //   refresh, attempt counter shared across channel-await, subscribe,
    //   and stream-error retry sites. These are async operations, not
    //   just mutable borrows. The HRTB variant (bd rc-cvq) would only
    //   solve &mut state, not async side-effect orchestration.
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
                        attempt += 1;
                        if !reconnect.should_retry(attempt) {
                            warn!(
                                broker = %broker_name,
                                destination = %destination,
                                consumer_idx = idx,
                                attempts = attempt,
                                "JMS consumer max reconnect attempts reached; terminating"
                            );
                            return;
                        }
                        let delay = reconnect.delay_for(attempt - 1);
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = ctx.cancelled() => break,
                            _ = tokio::time::sleep(delay) => {}
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
                attempt = 0;
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
                attempt += 1;
                if !reconnect.should_retry(attempt) {
                    warn!(
                        broker = %broker_name,
                        destination = %destination,
                        consumer_idx = idx,
                        attempts = attempt,
                        "JMS consumer max subscribe attempts reached; terminating"
                    );
                    return;
                }
                let delay = reconnect.delay_for(attempt - 1);
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ctx.cancelled() => break,
                    _ = tokio::time::sleep(delay) => {}
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
                            attempt = 0;
                            let exchange = build_exchange(&jms_msg, map_headers);
                            if let Err(e) = ctx.send(exchange).await {
                                runtime.metrics().increment_errors(
                                    ctx.route_id(),
                                    "b-prime:jms:consumer-send",
                                );
                                // log-policy: outside-contract
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

        attempt += 1;
        if !reconnect.should_retry(attempt) {
            warn!(
                broker = %broker_name,
                destination = %destination,
                consumer_idx = idx,
                attempts = attempt,
                "JMS consumer max reconnect attempts reached; terminating"
            );
            break;
        }
        let delay = reconnect.delay_for(attempt - 1);
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ctx.cancelled() => break,
            _ = tokio::time::sleep(delay) => {}
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
        let reconnect = self.reconnect.clone();
        let cancel = CancellationToken::new();
        self.cancel_token = Some(cancel.clone());

        // JMS-011: spawn concurrent consumer tasks
        let consumer_count = endpoint_config.concurrent_consumers;
        let runtime = self.runtime.clone();
        // allow-unwrap: consumer_count validated >= 1 in from_uri
        let handles: Vec<JoinHandle<Result<(), CamelError>>> = (0..consumer_count)
            .map(|idx| {
                let pool = Arc::clone(&pool);
                let broker_name = broker_name.clone();
                let endpoint_config = endpoint_config.clone();
                let cancel = cancel.clone();
                let ctx = ctx.clone();
                let reconnect = reconnect.clone();
                let runtime = runtime.clone();

                tokio::spawn(async move {
                    consumer_loop(
                        &pool,
                        &broker_name,
                        &endpoint_config,
                        &reconnect,
                        cancel,
                        &ctx,
                        idx,
                        runtime,
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
    use crate::config::{JmsPoolConfig, jms_reconnect_default};
    use camel_component_api::test_support::PanicRuntimeObservability;
    use tokio::sync::mpsc;

    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

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
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );
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
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );

        // Simulate an already-started state by setting a cancel token directly.
        consumer.cancel_token = Some(CancellationToken::new());

        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-test-route".to_string(),
        );
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
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );

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
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );

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
        let mut consumer = JmsConsumer::new(
            pool,
            "default".to_string(),
            endpoint_cfg,
            jms_reconnect_default(),
            rt(),
        );

        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "jms-test-route-2".to_string(),
        );

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

    /// Regression: max_attempts=N → exactly N invocations (caught OpenSearch off-by-one 1f5c4c2a).
    /// Replicates the exact retry loop from `consumer_loop` (consumer.rs:166-177):
    ///   attempt starts at 0, incremented on error, !should_retry(attempt), delay_for(attempt-1)
    #[tokio::test]
    async fn retry_loop_invokes_operation_exactly_max_attempts_times() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::time::Duration;

        let policy = NetworkRetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        };

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = Arc::clone(&calls);
        let mut attempt: u32 = 0;

        loop {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            let result: Result<(), ()> = Err(());
            match result {
                Ok(_) => {
                    break;
                }
                Err(_) => {
                    attempt += 1;
                    if !policy.should_retry(attempt) {
                        break;
                    }
                    let delay = policy.delay_for(attempt - 1);
                    tokio::time::sleep(delay).await;
                    continue;
                }
            }
        }

        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "max_attempts=3 must yield exactly 3 invocations"
        );
    }
}
