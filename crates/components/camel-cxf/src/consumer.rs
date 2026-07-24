use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_component_api::{
    Body, CamelError, ConcurrencyModel, Consumer, ConsumerContext, ConsumerStartupMode, Exchange,
    Message, Value,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;
use tracing::{error, info, warn};

use crate::pool::{BridgeState, CxfBridgePool};
use crate::proto::{ConsumerRequest, ConsumerResponse, cxf_bridge_client::CxfBridgeClient};

pub struct CxfConsumer {
    pool: Arc<CxfBridgePool>,
    /// Profile name — used for debugging/logging; the bridge sends security_profile
    /// via ConsumerRequest so the consumer doesn't need to send it explicitly.
    #[allow(dead_code)]
    profile_name: String,
    cancel_token: Option<CancellationToken>,
    task_handle: Option<JoinHandle<Result<(), CamelError>>>,
    /// Phase B will use this for `rt.metrics().increment_errors(...)` and
    /// `rt.health().force_unhealthy_for_route(...)` calls per ADR-0012.
    #[allow(dead_code)]
    runtime: Arc<dyn camel_component_api::RuntimeObservability>,
}

impl CxfConsumer {
    pub fn new(
        pool: Arc<CxfBridgePool>,
        profile_name: String,
        runtime: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Self {
        Self {
            pool,
            profile_name,
            cancel_token: None,
            task_handle: None,
            runtime,
        }
    }
}

fn build_exchange(req: &ConsumerRequest) -> Exchange {
    let body = if req.payload.is_empty() {
        Body::Empty
    } else {
        match std::str::from_utf8(&req.payload) {
            Ok(s) => Body::Text(s.to_string()),
            Err(_) => Body::Bytes(bytes::Bytes::from(req.payload.clone())),
        }
    };

    let mut exchange = Exchange::new(Message::new(body));

    if !req.request_id.is_empty() {
        exchange
            .input
            .set_header("CxfRequestId", Value::String(req.request_id.clone()));
    }
    if !req.operation.is_empty() {
        exchange
            .input
            .set_header("CxfOperation", Value::String(req.operation.clone()));
    }
    if !req.soap_action.is_empty() {
        exchange
            .input
            .set_header("CxfSoapAction", Value::String(req.soap_action.clone()));
    }
    if !req.security_profile.is_empty() {
        exchange.input.set_header(
            "CxfSecurityProfile",
            Value::String(req.security_profile.clone()),
        );
    }
    for (k, v) in &req.headers {
        exchange.input.set_header(k, Value::String(v.clone()));
    }

    exchange
}

fn build_response_body(exchange: &Exchange) -> Result<Vec<u8>, CamelError> {
    let body = exchange
        .output
        .as_ref()
        .map(|m| &m.body)
        .unwrap_or(&exchange.input.body);
    match body {
        Body::Text(s) => Ok(s.as_bytes().to_vec()),
        Body::Xml(s) => Ok(s.as_bytes().to_vec()),
        Body::Bytes(b) => Ok(b.to_vec()),
        Body::Json(v) => serde_json::to_vec(v).map_err(|e| {
            CamelError::ProcessorError(format!("JSON serialization error in CXF response: {e}"))
        }),
        Body::Empty => Ok(vec![]),
        Body::Stream(_) => Err(CamelError::ProcessorError(
            "streaming body not supported by CXF component".into(),
        )),
    }
}

fn is_bridge_transport_error(err: &CamelError) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("transport") || msg.contains("connection") || msg.contains("unavailable")
}

async fn await_ready_channel(pool: Arc<CxfBridgePool>) -> Result<Channel, CamelError> {
    let key = CxfBridgePool::slot_key();
    let slot = pool
        .get_or_create_slot(&key)
        .await
        .map_err(|e| CamelError::ProcessorError(format!("CXF slot error: {e}")))?;
    let mut rx = slot.state_rx.clone();

    loop {
        match &*rx.borrow() {
            BridgeState::Ready { channel } => return Ok(channel.clone()),
            BridgeState::Stopped => {
                return Err(CamelError::ProcessorError(format!(
                    "CXF bridge '{}' is stopped",
                    slot.key
                )));
            }
            _ => {}
        }

        if rx.changed().await.is_err() {
            return Err(CamelError::ProcessorError(format!(
                "CXF bridge '{}' state channel closed",
                slot.key
            )));
        }
    }
}

#[async_trait]
impl Consumer for CxfConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        let pool = Arc::clone(&self.pool);
        let cancel = CancellationToken::new();
        self.cancel_token = Some(cancel.clone());

        // Eagerly create/probe the bridge slot BEFORE spawning the consumer
        // task so a missing bridge binary or an aborted bridge process is
        // reported as a startup error instead of hanging the route controller.
        // Mirrors the JMS consumer's pre-flight (camel-jms/src/consumer.rs).
        {
            let key = CxfBridgePool::slot_key();
            let slot = pool
                .get_or_create_slot(&key)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("CXF slot error: {e}")))?;
            match &*slot.state_rx.borrow() {
                BridgeState::Ready { .. } | BridgeState::Starting => {} // bridge up or coming up
                BridgeState::Degraded(reason) => {
                    return Err(CamelError::ProcessorError(format!(
                        "CXF bridge not available: {reason}"
                    )));
                }
                other => {
                    return Err(CamelError::ProcessorError(format!(
                        "CXF bridge not available: {other:?}"
                    )));
                }
            }
        }

        let runtime = Arc::clone(&self.runtime);
        let ctx_task = ctx.clone();
        let handle: JoinHandle<Result<(), CamelError>> = tokio::spawn(async move {
            let ctx = ctx_task;
            let mut consecutive_transport_failures: u32 = 0;
            let mut reconnect_attempt: u32 = 0;
            // Manual retry loop (not retry_async) because:
            // - Retries are embedded inside `tokio::select!` with cancellation
            //   tokens (ctx.cancelled(), cancel.cancelled()); retry_async's
            //   tight loop cannot interleave cancellation checks between
            //   delay and retry.
            // - Inter-attempt side effects (channel refresh via
            //   pool.refresh_slot_channel, transport failure counting,
            //   reconnect_attempt reset on success) must run between retries.
            loop {
                let channel = tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("CXF consumer cancelled");
                        break;
                    }
                    _ = ctx.cancelled() => {
                        info!("CXF consumer context cancelled");
                        break;
                    }
                    result = await_ready_channel(Arc::clone(&pool)) => {
                        match result {
                            Ok(channel) => channel,
                            Err(e) => {
                                warn!(error = %e, "CXF consumer waiting for ready bridge failed");
                                if pool.reconnect.should_retry(reconnect_attempt + 1) {
                                    let delay = pool.reconnect.delay_for(reconnect_attempt);
                                    reconnect_attempt = reconnect_attempt.saturating_add(1);
                                    tokio::select! {
                                        _ = cancel.cancelled() => break,
                                        _ = ctx.cancelled() => break,
                                        _ = tokio::time::sleep(delay) => {}
                                    }
                                    continue;
                                }
                                warn!("CXF consumer reconnect attempts exhausted");
                                break;
                            }
                        }
                    }
                };

                let mut client = CxfBridgeClient::new(channel);
                let (response_tx, response_rx) = mpsc::channel::<ConsumerResponse>(32);
                let response_stream = ReceiverStream::new(response_rx);

                let stream_result = client.open_consumer_stream(response_stream).await;

                let mut stream = match stream_result {
                    Ok(resp) => {
                        consecutive_transport_failures = 0;
                        reconnect_attempt = 0;
                        // Readiness is signalled from start() (see ctx.mark_ready()
                        // after spawn), NOT here: open_consumer_stream is a
                        // bidirectional-streaming RPC whose `.await` only resolves
                        // once the bridge flushes response headers, which it defers
                        // until the first inbound SOAP request is dispatched. Gating
                        // route startup on this would deadlock (no request can arrive
                        // until the route is started).
                        info!("CXF consumer stream opened successfully");
                        resp.into_inner()
                    }
                    Err(e) => {
                        let err = CamelError::ProcessorError(format!("CXF gRPC stream error: {e}"));
                        if is_bridge_transport_error(&err) {
                            consecutive_transport_failures += 1;
                            if consecutive_transport_failures >= 2 {
                                warn!(
                                    failures = consecutive_transport_failures,
                                    "CXF stream transport failures exceeded threshold; refreshing channel"
                                );
                                let key = CxfBridgePool::slot_key();
                                if let Err(refresh_err) = pool.refresh_slot_channel(&key).await {
                                    warn!(error = %refresh_err, "CXF channel refresh failed; requesting bridge restart");
                                    pool.restart_slot(&key);
                                }
                                consecutive_transport_failures = 0;
                            }
                        } else {
                            consecutive_transport_failures = 0;
                        }
                        warn!(error = %e, "CXF consumer stream open failed; retrying");
                        if pool.reconnect.should_retry(reconnect_attempt + 1) {
                            let delay = pool.reconnect.delay_for(reconnect_attempt);
                            reconnect_attempt = reconnect_attempt.saturating_add(1);
                            tokio::select! {
                                _ = cancel.cancelled() => break,
                                _ = ctx.cancelled() => break,
                                _ = tokio::time::sleep(delay) => {}
                            }
                            continue;
                        }
                        warn!("CXF consumer reconnect attempts exhausted");
                        break;
                    }
                };

                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            info!("CXF consumer cancelled");
                            return Ok(());
                        }
                        _ = ctx.cancelled() => {
                            info!("CXF consumer context cancelled");
                            return Ok(());
                        }
                        msg = stream.message() => {
                            match msg {
                                Ok(Some(req)) => {
                                    let exchange = build_exchange(&req);
                                    let request_id = req.request_id.clone();
                                    let security_profile = req.security_profile.clone();

                                    let result = ctx.send_and_wait(exchange).await;

                                    let response = match result {
                                        Ok(resp_exchange) => {
                                            match build_response_body(&resp_exchange) {
                                                Ok(payload) => ConsumerResponse {
                                                    request_id,
                                                    payload,
                                                    fault: false,
                                                    fault_code: String::new(),
                                                    fault_string: String::new(),
                                                    security_profile,
                                                },
                                                Err(e) => {
                                                    runtime.metrics().increment_errors(ctx.route_id(), "b-prime:cxf:response-marshalling");
                                                    // log-policy: outside-contract
                                                    error!("CXF consumer response body error: {e}");
                                                    ConsumerResponse {
                                                        request_id,
                                                        payload: vec![],
                                                        fault: true,
                                                        fault_code: "soap:Server".to_string(),
                                                        fault_string: e.to_string(),
                                                        security_profile,
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            // log-policy: handler-owned
                                            warn!("CXF consumer route error: {e}");
                                            ConsumerResponse {
                                                request_id,
                                                payload: vec![],
                                                fault: true,
                                                fault_code: "soap:Server".to_string(),
                                                fault_string: e.to_string(),
                                                security_profile,
                                            }
                                        }
                                    };

                                    if let Err(send_err) = response_tx.send(response).await {
                                        warn!("Failed to send CXF consumer response: {send_err}");
                                    }
                                }
                                Ok(None) => {
                                    info!("CXF consumer stream ended; reconnecting");
                                    break;
                                }
                                Err(e) => {
                                    let err = CamelError::ProcessorError(format!("CXF gRPC stream error: {e}"));
                                    if is_bridge_transport_error(&err) {
                                        consecutive_transport_failures += 1;
                                        if consecutive_transport_failures >= 2 {
                                            warn!(
                                                failures = consecutive_transport_failures,
                                                "CXF stream transport failures exceeded threshold; refreshing channel"
                                            );
                                            let key = CxfBridgePool::slot_key();
                                            if let Err(refresh_err) = pool.refresh_slot_channel(&key).await {
                                                warn!(error = %refresh_err, "CXF channel refresh failed; requesting bridge restart");
                                                pool.restart_slot(&key);
                                            }
                                            consecutive_transport_failures = 0;
                                        }
                                    } else {
                                        consecutive_transport_failures = 0;
                                    }
                                    warn!(error = %e, "CXF consumer stream error; reconnecting");
                                    break;
                                }
                            }
                        }
                    }
                }

                if pool.reconnect.should_retry(reconnect_attempt + 1) {
                    let delay = pool.reconnect.delay_for(reconnect_attempt);
                    reconnect_attempt = reconnect_attempt.saturating_add(1);
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = ctx.cancelled() => break,
                        _ = tokio::time::sleep(delay) => {}
                    }
                } else {
                    warn!("CXF consumer reconnect attempts exhausted");
                    break;
                }
            }
            Ok(())
        });

        self.task_handle = Some(handle);

        // Signal route-startup readiness now. The bridge slot was probed above
        // (fail-fast on Degraded/Stopped), so the bridge process is running.
        // The gRPC consumer stream is opened lazily inside the spawned task and
        // cannot be gated on here without deadlocking (see the note at the
        // open_consumer_stream call site). Mirrors camel-jms's best-effort
        // mark_ready in start().
        ctx.mark_ready();

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(cancel) = self.cancel_token.take() {
            cancel.cancel();
        }
        if let Some(mut handle) = self.task_handle.take()
            && tokio::time::timeout(Duration::from_secs(5), &mut handle)
                .await
                .is_err()
        {
            handle.abort();
            let _ = handle.await;
            warn!("CXF consumer task did not stop in 5s; aborted");
        }
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }

    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    fn background_task_handle(
        &mut self,
    ) -> Option<tokio::task::JoinHandle<Result<(), CamelError>>> {
        self.task_handle.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CxfPoolConfig;
    use camel_component_api::NetworkRetryPolicy;
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

    fn test_pool() -> Arc<CxfBridgePool> {
        let pool_config = CxfPoolConfig {
            profiles: vec![],
            max_bridges: 1,
            bridge_start_timeout_ms: 5_000,
            health_check_interval_ms: 5_000,
            bridge_cache_dir: None,
            version: "0.1.0".to_string(),
            bind_address: None,
            reconnect: NetworkRetryPolicy::default(),
        };
        Arc::new(CxfBridgePool::from_config(pool_config).unwrap())
    }

    #[test]
    fn consumer_new() {
        let pool = test_pool();
        let consumer = CxfConsumer::new(pool, "baleares".to_string(), test_rt());
        assert!(consumer.cancel_token.is_none());
        assert!(consumer.task_handle.is_none());
        assert_eq!(consumer.profile_name, "baleares");
    }

    #[test]
    fn build_exchange_with_payload() {
        let req = ConsumerRequest {
            request_id: "req-1".to_string(),
            operation: "sayHello".to_string(),
            payload: b"<soap:Body><hello>world</hello></soap:Body>".to_vec(),
            headers: [("CustomHeader".to_string(), "custom-value".to_string())]
                .into_iter()
                .collect(),
            soap_action: "urn:sayHello".to_string(),
            security_profile: "baleares".to_string(),
        };

        let exchange = build_exchange(&req);

        assert!(matches!(exchange.input.body, Body::Text(_)));
        if let Body::Text(s) = &exchange.input.body {
            assert!(s.contains("<hello>world</hello>"));
        }
        assert_eq!(
            exchange
                .input
                .header("CxfRequestId")
                .and_then(|v| v.as_str()),
            Some("req-1")
        );
        assert_eq!(
            exchange
                .input
                .header("CxfOperation")
                .and_then(|v| v.as_str()),
            Some("sayHello")
        );
        assert_eq!(
            exchange
                .input
                .header("CxfSoapAction")
                .and_then(|v| v.as_str()),
            Some("urn:sayHello")
        );
        assert_eq!(
            exchange
                .input
                .header("CxfSecurityProfile")
                .and_then(|v| v.as_str()),
            Some("baleares")
        );
        assert_eq!(
            exchange
                .input
                .header("CustomHeader")
                .and_then(|v| v.as_str()),
            Some("custom-value")
        );
    }

    #[test]
    fn build_exchange_empty_payload() {
        let req = ConsumerRequest {
            request_id: "req-2".to_string(),
            operation: String::new(),
            payload: vec![],
            headers: Default::default(),
            soap_action: String::new(),
            security_profile: String::new(),
        };

        let exchange = build_exchange(&req);

        assert!(matches!(exchange.input.body, Body::Empty));
        assert_eq!(
            exchange
                .input
                .header("CxfRequestId")
                .and_then(|v| v.as_str()),
            Some("req-2")
        );
    }

    #[tokio::test]
    async fn stop_without_start_is_noop() {
        let pool = test_pool();
        let mut consumer = CxfConsumer::new(pool, "test".to_string(), test_rt());
        assert!(consumer.stop().await.is_ok());
    }

    #[tokio::test]
    async fn test_consumer_stop_cleans_up() {
        let pool = test_pool();
        let mut consumer = CxfConsumer::new(pool, "test".to_string(), test_rt());

        // Simulate the internal state that start() would set up,
        // but without actually trying to connect to a bridge.
        let cancel = CancellationToken::new();
        consumer.cancel_token = Some(cancel.clone());

        // Spawn a trivial task that respects cancellation.
        let handle = tokio::spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = tokio::time::sleep(Duration::from_secs(300)) => {}
            }
            Ok(())
        });
        consumer.task_handle = Some(handle);

        // Stop must cancel the token and await the handle within timeout.
        let stop_result = consumer.stop().await;
        assert!(stop_result.is_ok(), "stop should succeed: {stop_result:?}");
        assert!(
            consumer.task_handle.is_none(),
            "handle should be taken after stop"
        );
        assert!(
            consumer.cancel_token.is_none(),
            "cancel should be taken after stop"
        );
    }

    #[test]
    fn build_response_body_from_text() {
        let exchange = Exchange::new(Message::new(Body::Text(
            "<response>ok</response>".to_string(),
        )));
        let body = build_response_body(&exchange).unwrap();
        assert_eq!(body, b"<response>ok</response>");
    }

    #[test]
    fn build_response_body_from_empty() {
        let exchange = Exchange::new(Message::new(Body::Empty));
        let body = build_response_body(&exchange).unwrap();
        assert!(body.is_empty());
    }

    #[test]
    fn build_response_body_from_json() {
        let json = serde_json::json!({"status": "ok", "code": 200});
        let exchange = Exchange::new(Message::new(Body::Json(json.clone())));
        let body = build_response_body(&exchange).unwrap();
        let expected = serde_json::to_vec(&json).unwrap();
        assert_eq!(body, expected);
    }

    #[test]
    fn build_response_body_from_xml() {
        let xml = "<soap:Envelope><soap:Body><hello>world</hello></soap:Body></soap:Envelope>";
        let exchange = Exchange::new(Message::new(Body::Xml(xml.to_string())));
        let body = build_response_body(&exchange).unwrap();
        assert_eq!(body, xml.as_bytes());
    }

    #[test]
    fn build_response_body_from_bytes() {
        let raw = b"binary\x00data\xff".to_vec();
        let exchange = Exchange::new(Message::new(Body::Bytes(bytes::Bytes::from(raw.clone()))));
        let body = build_response_body(&exchange).unwrap();
        assert_eq!(body, raw);
    }

    #[test]
    fn build_response_body_stream_returns_error() {
        use camel_component_api::StreamBody;
        use futures::stream;
        use tokio::sync::Mutex;

        let s = stream::empty::<Result<bytes::Bytes, CamelError>>();
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(s)))),
            metadata: Default::default(),
        });
        let exchange = Exchange::new(Message::new(body));
        let result = build_response_body(&exchange);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("streaming body not supported"),
            "got: {err}"
        );
    }

    #[test]
    fn reconnect_policy_delay_for_grows_exponentially() {
        let p = NetworkRetryPolicy {
            initial_delay: Duration::from_millis(500),
            multiplier: 2.0,
            max_delay: Duration::from_secs(30),
            jitter_factor: 0.0,
            ..NetworkRetryPolicy::default()
        };
        let d0 = p.delay_for(0);
        let d1 = p.delay_for(1);
        let d2 = p.delay_for(2);
        let d3 = p.delay_for(3);
        assert!(d0 < d1, "{d0:?} should be < {d1:?}");
        assert!(d1 < d2, "{d1:?} should be < {d2:?}");
        assert!(d2 < d3, "{d2:?} should be < {d3:?}");
        // Verify cap at 30s for large attempts
        assert_eq!(p.delay_for(10), Duration::from_secs(30));
        assert_eq!(p.delay_for(20), Duration::from_secs(30));
        assert_eq!(p.delay_for(100), Duration::from_secs(30));
    }

    #[test]
    fn build_exchange_preserves_operation_header() {
        let req = ConsumerRequest {
            request_id: String::new(),
            operation: "sayHello".to_string(),
            payload: b"<hello/>".to_vec(),
            headers: Default::default(),
            soap_action: String::new(),
            security_profile: String::new(),
        };
        let exchange = build_exchange(&req);
        assert_eq!(
            exchange
                .input
                .header("CxfOperation")
                .and_then(|v| v.as_str()),
            Some("sayHello")
        );
    }

    #[test]
    fn is_bridge_transport_error_detects_transport() {
        assert!(is_bridge_transport_error(&CamelError::ProcessorError(
            "transport error".to_string()
        )));
        assert!(is_bridge_transport_error(&CamelError::ProcessorError(
            "connection refused".to_string()
        )));
        assert!(is_bridge_transport_error(&CamelError::ProcessorError(
            "UNAVAILABLE".to_string()
        )));
        assert!(!is_bridge_transport_error(&CamelError::ProcessorError(
            "SOAP fault".to_string()
        )));
    }

    #[test]
    fn reconnect_policy_delay_for_exact_values() {
        let p = NetworkRetryPolicy {
            initial_delay: Duration::from_millis(500),
            multiplier: 2.0,
            max_delay: Duration::from_secs(30),
            jitter_factor: 0.0,
            ..NetworkRetryPolicy::default()
        };
        assert_eq!(p.delay_for(0), Duration::from_millis(500));
        assert_eq!(p.delay_for(1), Duration::from_secs(1));
        assert_eq!(p.delay_for(2), Duration::from_secs(2));
        assert_eq!(p.delay_for(10), Duration::from_secs(30));
    }
}
