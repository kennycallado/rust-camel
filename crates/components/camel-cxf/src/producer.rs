use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use camel_component_api::{Body, CamelError, Exchange, Value};
use tower::Service;
use tracing::debug;

use crate::error::CxfError;
use crate::pool::{BridgeState, CxfBridgePool};
use crate::proto::{SoapRequest, cxf_bridge_client::CxfBridgeClient};

fn is_transport_error(status: &tonic::Status) -> bool {
    match status.code() {
        tonic::Code::Unavailable => true,
        tonic::Code::Internal => {
            let msg = status.message().to_lowercase();
            msg.contains("transport")
                || msg.contains("connection")
                || msg.contains("channel")
                || msg.contains("broken pipe")
                || msg.contains("io error")
        }
        _ => false,
    }
}

#[derive(Clone)]
pub struct CxfProducer {
    pool: Arc<CxfBridgePool>,
    profile_name: String,
    wsdl_path: String,
    service_name: String,
    port_name: String,
    address: Option<String>,
    operation: String,
}

impl CxfProducer {
    pub fn new(
        pool: Arc<CxfBridgePool>,
        profile_name: String,
        wsdl_path: String,
        service_name: String,
        port_name: String,
        address: Option<String>,
        operation: String,
    ) -> Self {
        Self {
            pool,
            profile_name,
            wsdl_path,
            service_name,
            port_name,
            address,
            operation,
        }
    }

    fn body_to_bytes(body: &Body) -> Result<Vec<u8>, CamelError> {
        match body {
            Body::Text(s) => Ok(s.as_bytes().to_vec()),
            Body::Xml(s) => Ok(s.as_bytes().to_vec()),
            Body::Bytes(b) => Ok(b.to_vec()),
            Body::Json(v) => serde_json::to_vec(v)
                .map_err(|e| CamelError::ProcessorError(format!("JSON error: {e}"))),
            Body::Empty => Ok(vec![]),
            Body::Stream(_) => Err(CamelError::ProcessorError(
                "Body::Stream must be materialized before sending to CXF".to_string(),
            )),
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
impl CxfProducer {
    pub fn from_channel(
        channel: tonic::transport::Channel,
        profile_name: String,
        wsdl_path: String,
        service_name: String,
        port_name: String,
        address: Option<String>,
        operation: String,
    ) -> Self {
        use crate::config::CxfPoolConfig;
        use crate::pool::{BridgeSlot, BridgeState};
        use tokio::sync::watch;

        let pool = Arc::new(
            CxfBridgePool::from_config(CxfPoolConfig {
                profiles: vec![],
                max_bridges: 1,
                bridge_start_timeout_ms: 30_000,
                health_check_interval_ms: 5_000,
                bridge_cache_dir: None,
                version: crate::BRIDGE_VERSION.to_string(),
                bind_address: None,
            })
            .expect("valid test pool config"),
        );

        let key = CxfBridgePool::slot_key();
        let (state_tx, state_rx) = watch::channel(BridgeState::Ready { channel });
        let slot = Arc::new(BridgeSlot {
            key: key.clone(),
            configured_profiles: vec![],
            bind_address: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
        });
        pool.slots.insert(key, slot);

        Self {
            pool,
            profile_name,
            wsdl_path,
            service_name,
            port_name,
            address,
            operation,
        }
    }
}

impl Service<Exchange> for CxfProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if let Some(slot) = self.pool.slots.get(&CxfBridgePool::slot_key()) {
            match &*slot.state_rx.borrow() {
                BridgeState::Ready { .. } => return Poll::Ready(Ok(())),
                BridgeState::Starting | BridgeState::Restarting { .. } => {
                    // Spawn a task that waits for the next state change and then
                    // wakes this future. Avoids busy-poll while honouring Tower contract.
                    // Guard with try_current: in contexts without a Tokio runtime (e.g.
                    // unit tests) fall back to an immediate wake so the caller can retry.
                    let waker = cx.waker().clone();
                    let mut rx = slot.state_rx.clone();
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        handle.spawn(async move {
                            let _ = rx.changed().await;
                            waker.wake();
                        });
                    } else {
                        waker.wake_by_ref();
                    }
                    return Poll::Pending;
                }
                BridgeState::Degraded(reason) => {
                    return Poll::Ready(Err(CamelError::ProcessorError(format!(
                        "cxf bridge degraded: {reason}"
                    ))));
                }
                BridgeState::Stopped => {
                    return Poll::Ready(Err(CamelError::ProcessorError(
                        "cxf bridge stopped".to_string(),
                    )));
                }
            }
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let pool = Arc::clone(&self.pool);
        let profile_name = self.profile_name.clone();
        let wsdl_path = self.wsdl_path.clone();
        let service_name = self.service_name.clone();
        let port_name = self.port_name.clone();
        let address = self.address.clone();
        let configured_operation = self.operation.clone();

        Box::pin(async move {
            let channel = pool
                .get_channel()
                .await
                .map_err(|e| CamelError::ProcessorError(format!("CXF channel error: {e}")))?;

            let body = Self::body_to_bytes(&exchange.input.body)?;

            let operation = exchange
                .input
                .header("CamelCxfOperation")
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| configured_operation.clone());

            let timeout_ms = exchange
                .input
                .header("CamelCxfTimeoutMs")
                .and_then(|v| v.as_str().and_then(|s| s.parse::<i32>().ok()))
                .unwrap_or(0);

            let mut headers: HashMap<String, String> = HashMap::new();
            if let Some(soap_action) = exchange
                .input
                .header("CamelCxfSoapAction")
                .and_then(|v| v.as_str().map(str::to_string))
            {
                headers.insert("soapAction".to_string(), soap_action);
            }

            let request = SoapRequest {
                wsdl_path,
                address: address.unwrap_or_default(),
                service_name,
                port_name,
                operation: operation.clone(),
                payload: body,
                headers,
                timeout_ms,
                security_profile: profile_name.clone(),
            };

            let mut client = CxfBridgeClient::new(channel);
            let response = match client.invoke(request.clone()).await {
                Ok(r) => r.into_inner(),
                Err(status) if is_transport_error(&status) => {
                    let key = CxfBridgePool::slot_key();
                    if let Err(refresh_err) = pool.refresh_slot_channel(&key).await {
                        pool.restart_slot(&key);
                        return Err(CamelError::ProcessorError(format!(
                            "CXF gRPC invoke transport error: {status}; refresh failed: {refresh_err}"
                        )));
                    }

                    let refreshed = pool.get_channel().await.map_err(|e| {
                        CamelError::ProcessorError(format!("CXF channel refresh error: {e}"))
                    })?;
                    let mut retry_client = CxfBridgeClient::new(refreshed);
                    retry_client
                        .invoke(request)
                        .await
                        .map_err(|s| {
                            CamelError::ProcessorError(format!("CXF gRPC invoke error: {s}"))
                        })?
                        .into_inner()
                }
                Err(status) => {
                    return Err(CamelError::ProcessorError(format!(
                        "CXF gRPC invoke error: {status}"
                    )));
                }
            };

            if response.fault {
                return Err(CamelError::ProcessorError(
                    CxfError::Fault {
                        code: response.fault_code,
                        message: response.fault_string,
                    }
                    .to_string(),
                ));
            }

            debug!(operation = %operation, "CXF SOAP invocation completed");

            exchange.input.body = Body::Bytes(response.payload.into());

            for (k, v) in response.headers {
                exchange.input.set_header(&k, Value::String(v));
            }

            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{BridgeSlot, CxfBridgePool};
    use camel_component_api::StreamBody;
    use futures::stream;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio::sync::watch;

    fn test_pool() -> Arc<CxfBridgePool> {
        let pool_config = crate::config::CxfPoolConfig {
            profiles: vec![],
            max_bridges: 1,
            bridge_start_timeout_ms: 5_000,
            health_check_interval_ms: 5_000,
            bridge_cache_dir: None,
            version: "0.1.0".to_string(),
            bind_address: None,
        };
        Arc::new(crate::pool::CxfBridgePool::from_config(pool_config).unwrap())
    }

    #[test]
    fn producer_new() {
        let pool = test_pool();
        let producer = CxfProducer::new(
            pool,
            "baleares".to_string(),
            "/wsdl/hello.wsdl".to_string(),
            "HelloService".to_string(),
            "HelloPort".to_string(),
            Some("http://localhost:8080/service".to_string()),
            "sayHello".to_string(),
        );
        assert_eq!(producer.operation, "sayHello");
        assert_eq!(producer.profile_name, "baleares");
    }

    #[test]
    fn test_poll_ready_no_slot() {
        let pool = test_pool();
        let mut producer = CxfProducer::new(
            pool,
            "test".to_string(),
            "/wsdl/hello.wsdl".to_string(),
            "Svc".to_string(),
            "Port".to_string(),
            None,
            "op".to_string(),
        );
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let poll = producer.poll_ready(&mut cx);
        assert!(matches!(poll, Poll::Ready(Ok(()))));
    }

    #[tokio::test]
    async fn test_poll_ready_bridge_ready() {
        let pool = test_pool();
        let channel =
            tonic::transport::Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let slot = BridgeSlot::new_ready_for_test(channel);
        pool.insert_slot_for_test(CxfBridgePool::slot_key(), slot);

        let mut producer = CxfProducer::new(
            pool,
            "test".to_string(),
            "/wsdl/hello.wsdl".to_string(),
            "Svc".to_string(),
            "Port".to_string(),
            None,
            "op".to_string(),
        );
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let poll = producer.poll_ready(&mut cx);
        assert!(matches!(poll, Poll::Ready(Ok(()))));
    }

    #[tokio::test]
    async fn test_poll_ready_bridge_starting() {
        let pool = test_pool();
        let _channel =
            tonic::transport::Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let slot = BridgeSlot {
            key: CxfBridgePool::slot_key(),
            configured_profiles: vec![],
            bind_address: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
        };
        pool.insert_slot_for_test(CxfBridgePool::slot_key(), slot);

        let mut producer = CxfProducer::new(
            pool,
            "test".to_string(),
            "/wsdl/hello.wsdl".to_string(),
            "Svc".to_string(),
            "Port".to_string(),
            None,
            "op".to_string(),
        );
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let poll = producer.poll_ready(&mut cx);
        assert!(matches!(poll, Poll::Pending));
    }

    #[tokio::test]
    async fn test_poll_ready_bridge_degraded() {
        let pool = test_pool();
        let _channel =
            tonic::transport::Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let (state_tx, state_rx) =
            watch::channel(BridgeState::Degraded("connection lost".to_string()));
        let slot = BridgeSlot {
            key: CxfBridgePool::slot_key(),
            configured_profiles: vec![],
            bind_address: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
        };
        pool.insert_slot_for_test(CxfBridgePool::slot_key(), slot);

        let mut producer = CxfProducer::new(
            pool,
            "test".to_string(),
            "/wsdl/hello.wsdl".to_string(),
            "Svc".to_string(),
            "Port".to_string(),
            None,
            "op".to_string(),
        );
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let poll = producer.poll_ready(&mut cx);
        match poll {
            Poll::Ready(Err(e)) => assert!(e.to_string().contains("degraded")),
            other => panic!("expected Ready(Err), got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_poll_ready_bridge_stopped() {
        let pool = test_pool();
        let _channel =
            tonic::transport::Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let (state_tx, state_rx) = watch::channel(BridgeState::Stopped);
        let slot = BridgeSlot {
            key: CxfBridgePool::slot_key(),
            configured_profiles: vec![],
            bind_address: None,
            state_rx,
            state_tx,
            process: Arc::new(tokio::sync::Mutex::new(None)),
        };
        pool.insert_slot_for_test(CxfBridgePool::slot_key(), slot);

        let mut producer = CxfProducer::new(
            pool,
            "test".to_string(),
            "/wsdl/hello.wsdl".to_string(),
            "Svc".to_string(),
            "Port".to_string(),
            None,
            "op".to_string(),
        );
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let poll = producer.poll_ready(&mut cx);
        match poll {
            Poll::Ready(Err(e)) => assert!(e.to_string().contains("stopped")),
            other => panic!("expected Ready(Err), got: {other:?}"),
        }
    }

    #[test]
    fn body_text_to_bytes() {
        let b = CxfProducer::body_to_bytes(&Body::Text("hello".to_string())).unwrap();
        assert_eq!(b, b"hello");
    }

    #[test]
    fn body_xml_to_bytes() {
        let b = CxfProducer::body_to_bytes(&Body::Xml("<root/>".to_string())).unwrap();
        assert_eq!(b, b"<root/>");
    }

    #[test]
    fn body_bytes_to_bytes() {
        let b = CxfProducer::body_to_bytes(&Body::Bytes(bytes::Bytes::from("raw"))).unwrap();
        assert_eq!(b, b"raw");
    }

    #[test]
    fn body_json_to_bytes() {
        let b = CxfProducer::body_to_bytes(&Body::Json(serde_json::json!({"key": "val"}))).unwrap();
        assert_eq!(b, b"{\"key\":\"val\"}");
    }

    #[test]
    fn body_empty_to_bytes() {
        let b = CxfProducer::body_to_bytes(&Body::Empty).unwrap();
        assert!(b.is_empty());
    }

    #[test]
    fn body_stream_returns_error() {
        let s = stream::empty::<Result<bytes::Bytes, CamelError>>();
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(s)))),
            metadata: Default::default(),
        });
        let err = CxfProducer::body_to_bytes(&body).unwrap_err();
        assert!(err.to_string().contains("Stream"));
    }

    #[test]
    fn is_transport_error_detects_unavailable() {
        let status = tonic::Status::unavailable("connection dropped");
        assert!(is_transport_error(&status));
    }

    #[test]
    fn is_transport_error_detects_internal_transport_text() {
        let status = tonic::Status::internal("transport channel closed");
        assert!(is_transport_error(&status));
    }

    #[test]
    fn is_transport_error_ignores_non_transport_internal() {
        let status = tonic::Status::internal("soap fault");
        assert!(!is_transport_error(&status));
    }
}
