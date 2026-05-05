use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_component_api::{
    Body, CamelError, ConcurrencyModel, Consumer, ConsumerContext, Exchange, Message, Value,
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
    task_handle: Option<JoinHandle<()>>,
}

impl CxfConsumer {
    pub fn new(pool: Arc<CxfBridgePool>, profile_name: String) -> Self {
        Self {
            pool,
            profile_name,
            cancel_token: None,
            task_handle: None,
        }
    }
}

fn build_exchange(req: &ConsumerRequest) -> Exchange {
    let body_bytes = req.payload.clone();
    let body = if body_bytes.is_empty() {
        Body::Empty
    } else {
        match String::from_utf8(body_bytes.clone()) {
            Ok(s) => Body::Text(s),
            Err(_) => Body::Bytes(bytes::Bytes::from(body_bytes)),
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

fn build_response_body(exchange: &Exchange) -> Vec<u8> {
    let body = exchange
        .output
        .as_ref()
        .map(|m| &m.body)
        .unwrap_or(&exchange.input.body);
    match body {
        Body::Text(s) => s.as_bytes().to_vec(),
        Body::Xml(s) => s.as_bytes().to_vec(),
        Body::Bytes(b) => b.to_vec(),
        Body::Json(v) => serde_json::to_vec(v).unwrap_or_default(),
        Body::Empty => vec![],
        Body::Stream(_) => vec![],
    }
}

fn is_bridge_transport_error(err: &CamelError) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("transport") || msg.contains("connection") || msg.contains("unavailable")
}

fn reconnect_delay(attempt: u32) -> Duration {
    let delay_ms = 500u64.saturating_mul(2u64.saturating_pow(attempt.min(16)));
    Duration::from_millis(delay_ms.min(30_000))
}

async fn await_ready_channel(
    pool: Arc<CxfBridgePool>,
) -> Result<Channel, CamelError> {
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

        let handle = tokio::spawn(async move {
            let mut consecutive_transport_failures: u32 = 0;
            let mut reconnect_attempt: u32 = 0;
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
                                let delay = reconnect_delay(reconnect_attempt);
                                reconnect_attempt = reconnect_attempt.saturating_add(1);
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

                let mut client = CxfBridgeClient::new(channel);
                let (response_tx, response_rx) = mpsc::channel::<ConsumerResponse>(32);
                let response_stream = ReceiverStream::new(response_rx);

                let stream_result = client.open_consumer_stream(response_stream).await;

                let mut stream = match stream_result {
                    Ok(resp) => {
                        consecutive_transport_failures = 0;
                        reconnect_attempt = 0;
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
                        let delay = reconnect_delay(reconnect_attempt);
                        reconnect_attempt = reconnect_attempt.saturating_add(1);
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
                            info!("CXF consumer cancelled");
                            return;
                        }
                        _ = ctx.cancelled() => {
                            info!("CXF consumer context cancelled");
                            return;
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
                                            let payload = build_response_body(&resp_exchange);
                                            ConsumerResponse {
                                                request_id,
                                                payload,
                                                fault: false,
                                                fault_code: String::new(),
                                                fault_string: String::new(),
                                                security_profile,
                                            }
                                        }
                                        Err(e) => {
                                            error!("CXF consumer route error: {e}");
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

                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ctx.cancelled() => break,
                    _ = tokio::time::sleep({
                        let delay = reconnect_delay(reconnect_attempt);
                        reconnect_attempt = reconnect_attempt.saturating_add(1);
                        delay
                    }) => {}
                }
            }
        });

        self.task_handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(cancel) = self.cancel_token.take() {
            cancel.cancel();
        }
        if let Some(handle) = self.task_handle.take()
            && let Err(join_err) = handle.await
        {
            warn!("CXF consumer task panicked on stop: {join_err}");
        }
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CxfPoolConfig;

    fn test_pool() -> Arc<CxfBridgePool> {
        let pool_config = CxfPoolConfig {
            profiles: vec![],
            max_bridges: 1,
            bridge_start_timeout_ms: 5_000,
            health_check_interval_ms: 5_000,
            bridge_cache_dir: None,
            version: "0.1.0".to_string(),
            bind_address: None,
        };
        Arc::new(CxfBridgePool::from_config(pool_config).unwrap())
    }

    #[test]
    fn consumer_new() {
        let pool = test_pool();
        let consumer = CxfConsumer::new(pool, "baleares".to_string());
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
        let mut consumer = CxfConsumer::new(pool, "test".to_string());
        assert!(consumer.stop().await.is_ok());
    }

    #[test]
    fn build_response_body_from_text() {
        let exchange = Exchange::new(Message::new(Body::Text(
            "<response>ok</response>".to_string(),
        )));
        let body = build_response_body(&exchange);
        assert_eq!(body, b"<response>ok</response>");
    }

    #[test]
    fn build_response_body_from_empty() {
        let exchange = Exchange::new(Message::new(Body::Empty));
        let body = build_response_body(&exchange);
        assert!(body.is_empty());
    }

    #[test]
    fn build_response_body_from_json() {
        let json = serde_json::json!({"status": "ok", "code": 200});
        let exchange = Exchange::new(Message::new(Body::Json(json.clone())));
        let body = build_response_body(&exchange);
        let expected = serde_json::to_vec(&json).unwrap();
        assert_eq!(body, expected);
    }

    #[test]
    fn build_response_body_from_xml() {
        let xml = "<soap:Envelope><soap:Body><hello>world</hello></soap:Body></soap:Envelope>";
        let exchange = Exchange::new(Message::new(Body::Xml(xml.to_string())));
        let body = build_response_body(&exchange);
        assert_eq!(body, xml.as_bytes());
    }

    #[test]
    fn build_response_body_from_bytes() {
        let raw = b"binary\x00data\xff".to_vec();
        let exchange = Exchange::new(Message::new(Body::Bytes(bytes::Bytes::from(raw.clone()))));
        let body = build_response_body(&exchange);
        assert_eq!(body, raw);
    }

    #[test]
    fn reconnect_delay_sequence_monotonically_increases() {
        let d0 = reconnect_delay(0);
        let d1 = reconnect_delay(1);
        let d2 = reconnect_delay(2);
        let d3 = reconnect_delay(3);
        assert!(d0 < d1, "{d0:?} should be < {d1:?}");
        assert!(d1 < d2, "{d1:?} should be < {d2:?}");
        assert!(d2 < d3, "{d2:?} should be < {d3:?}");
        // Verify cap at 30s for large attempts
        assert_eq!(reconnect_delay(10), Duration::from_secs(30));
        assert_eq!(reconnect_delay(20), Duration::from_secs(30));
        assert_eq!(reconnect_delay(100), Duration::from_secs(30));
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
    fn reconnect_delay_backoff_caps_at_30s() {
        assert_eq!(reconnect_delay(0), Duration::from_millis(500));
        assert_eq!(reconnect_delay(1), Duration::from_secs(1));
        assert_eq!(reconnect_delay(2), Duration::from_secs(2));
        assert_eq!(reconnect_delay(10), Duration::from_secs(30));
    }
}
