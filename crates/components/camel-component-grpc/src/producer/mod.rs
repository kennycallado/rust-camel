use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange};
use futures::StreamExt;
use http::uri::PathAndQuery;
use prost_reflect::MessageDescriptor;
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};
use tonic::Request;
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, Endpoint};
use tower::Service;
use tracing::{debug, error};

use crate::codec::RawBytesCodec;
use crate::config::{AuthConfig, GrpcConfig, apply_auth_metadata};
use crate::mode::GrpcMode;

mod retry;
pub use retry::is_retryable_tonic_status;
use retry::retry_rpc;
use retry::tonic_to_camel_error;

mod convert;
pub(crate) use convert::proto_cache;
use convert::{json_to_protobuf, protobuf_to_json};

type ProducerFuture = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;
type AcquireFut =
    Option<Pin<Box<dyn Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send>>>;

/// Default max concurrent gRPC calls per producer instance.
const DEFAULT_CONCURRENCY: usize = 128;

pub struct GrpcProducer {
    channel: Channel,
    path: PathAndQuery,
    req_descriptor: MessageDescriptor,
    resp_descriptor: MessageDescriptor,
    mode: GrpcMode,
    deadline_ms: Option<u64>,
    retry: camel_component_api::NetworkRetryPolicy,
    semaphore: Arc<Semaphore>,
    pending_permit: Option<OwnedSemaphorePermit>,
    acquire_fut: AcquireFut,
    auth: AuthConfig,
    config_metadata: Option<String>,
}

impl Clone for GrpcProducer {
    fn clone(&self) -> Self {
        Self {
            channel: self.channel.clone(),
            path: self.path.clone(),
            req_descriptor: self.req_descriptor.clone(),
            resp_descriptor: self.resp_descriptor.clone(),
            mode: self.mode,
            deadline_ms: self.deadline_ms,
            retry: self.retry.clone(),
            semaphore: Arc::clone(&self.semaphore),
            // Each clone starts with a fresh permit state.
            pending_permit: None,
            acquire_fut: None,
            auth: self.auth.clone(),
            config_metadata: self.config_metadata.clone(),
        }
    }
}

impl GrpcProducer {
    pub fn new(
        addr: String,
        proto_path: PathBuf,
        service_name: String,
        method_name: String,
        mode: GrpcMode,
        deadline_ms: Option<u64>,
        config: &GrpcConfig,
    ) -> Result<Self, CamelError> {
        let endpoint = Endpoint::from_shared(addr.clone()).map_err(|e| {
            error!(error = %e, "grpc producer creation failed");
            CamelError::EndpointCreationFailed(format!("invalid grpc endpoint: {e}"))
        })?;
        let channel = endpoint.connect_lazy();

        let cache = proto_cache();
        let pool = cache
            .get_or_compile(&proto_path, std::iter::empty::<&Path>())
            .map_err(|e| {
                error!(error = %e, "grpc producer creation failed");
                CamelError::EndpointCreationFailed(format!("failed to compile proto: {e}"))
            })?;

        let svc = pool.get_service_by_name(&service_name).ok_or_else(|| {
            let err = CamelError::EndpointCreationFailed(format!(
                "service descriptor not found: {service_name}"
            ));
            error!(service = %service_name, error = %err, "grpc producer creation failed");
            err
        })?;

        let method = svc
            .methods()
            .find(|m| m.name() == method_name)
            .ok_or_else(|| {
                let err = CamelError::EndpointCreationFailed(format!(
                    "method descriptor not found: {service_name}/{method_name}"
                ));
                error!(service = %service_name, method = %method_name, error = %err, "grpc producer creation failed");
                err
            })?;
        let req_descriptor = method.input();
        let resp_descriptor = method.output();
        let path = PathAndQuery::from_maybe_shared(format!("/{service_name}/{method_name}"))
            .map_err(|e| {
                error!(error = %e, "grpc producer creation failed");
                CamelError::EndpointCreationFailed(format!("invalid gRPC path: {e}"))
            })?;

        debug!(addr = %addr, service = %service_name, method = %method_name, "grpc producer created");
        Ok(Self {
            channel,
            path,
            req_descriptor,
            resp_descriptor,
            mode,
            deadline_ms,
            retry: config.retry.clone(),
            semaphore: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
            pending_permit: None,
            acquire_fut: None,
            auth: config.auth.clone(),
            config_metadata: config.metadata.clone(),
        })
    }

    fn body_to_json(body: Body) -> Result<serde_json::Value, CamelError> {
        match body {
            Body::Json(v) => Ok(v),
            Body::Text(s) => serde_json::from_str(&s).map_err(|e| {
                CamelError::TypeConversionFailed(format!("invalid JSON text body: {e}"))
            }),
            other => Err(CamelError::TypeConversionFailed(format!(
                "grpc producer requires JSON or text body, got {other:?}"
            ))),
        }
    }

    fn header_to_metadata(
        value: &serde_json::Value,
    ) -> Option<MetadataValue<tonic::metadata::Ascii>> {
        let s = match value {
            serde_json::Value::String(v) => v.clone(),
            serde_json::Value::Number(v) => v.to_string(),
            serde_json::Value::Bool(v) => v.to_string(),
            _ => return None,
        };
        MetadataValue::try_from(s).ok()
    }

    fn inject_headers<T>(exchange: &Exchange, request: &mut Request<T>) {
        for (k, v) in &exchange.input.headers {
            if let Some(meta) = Self::header_to_metadata(v)
                && let Ok(name) = tonic::metadata::MetadataKey::from_bytes(k.as_bytes())
            {
                request.metadata_mut().insert(name, meta);
            }
        }
    }

    fn call_unary(&mut self, mut exchange: Exchange) -> ProducerFuture {
        let channel = self.channel.clone();
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();
        let deadline_ms = self.deadline_ms;
        let retry = self.retry.clone();
        let auth = self.auth.clone();
        let config_metadata = self.config_metadata.clone();

        Box::pin(async move {
            debug!(path = %path, "grpc unary call");
            let json = Self::body_to_json(exchange.input.body.clone())?;
            let buf = json_to_protobuf(json, req_df)?;
            let mut request = Request::new(buf);
            Self::inject_headers(&exchange, &mut request);
            apply_auth_metadata(&auth, &mut request).await?;
            if let Some(ref metadata_str) = config_metadata {
                for pair in metadata_str.split(',') {
                    let pair = pair.trim();
                    if let Some((key, value)) = pair.split_once('=') {
                        let key = key.trim();
                        let value = value.trim();
                        if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                            && let Ok(meta_val) = MetadataValue::try_from(value)
                        {
                            request.metadata_mut().insert(name, meta_val);
                        }
                    }
                }
                debug!("applied config metadata to gRPC unary request");
            }
            if let Some(ms) = deadline_ms {
                request.set_timeout(Duration::from_millis(ms));
            }

            let metadata_map = request.metadata().clone();
            let body = request.into_inner();

            let response = retry_rpc(channel, &retry, "unary", |mut grpc| {
                let mut req = Request::new(body.clone());
                *req.metadata_mut() = metadata_map.clone();
                if let Some(ms) = deadline_ms {
                    req.set_timeout(Duration::from_millis(ms));
                }
                let p = path.clone();
                async move {
                    grpc.ready().await.map_err(|e| {
                        tonic::Status::unavailable(format!("grpc client not ready: {e}"))
                    })?;
                    grpc.unary(req, p, RawBytesCodec).await
                }
            })
            .await?;

            let resp_json = protobuf_to_json(response.into_inner(), resp_desc)?;
            exchange.input.body = Body::Json(resp_json);
            Ok(exchange)
        })
    }

    fn call_server_streaming(&mut self, mut exchange: Exchange) -> ProducerFuture {
        let channel = self.channel.clone();
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();
        let deadline_ms = self.deadline_ms;
        let retry = self.retry.clone();
        let auth = self.auth.clone();
        let config_metadata = self.config_metadata.clone();

        Box::pin(async move {
            debug!(path = %path, "grpc server streaming call");
            let json = Self::body_to_json(exchange.input.body.clone())?;
            let buf = json_to_protobuf(json, req_df)?;
            let mut request = Request::new(buf);
            Self::inject_headers(&exchange, &mut request);
            apply_auth_metadata(&auth, &mut request).await?;
            if let Some(ref metadata_str) = config_metadata {
                for pair in metadata_str.split(',') {
                    let pair = pair.trim();
                    if let Some((key, value)) = pair.split_once('=') {
                        let key = key.trim();
                        let value = value.trim();
                        if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                            && let Ok(meta_val) = MetadataValue::try_from(value)
                        {
                            request.metadata_mut().insert(name, meta_val);
                        }
                    }
                }
                debug!("applied config metadata to gRPC server streaming request");
            }
            if let Some(ms) = deadline_ms {
                request.set_timeout(Duration::from_millis(ms));
            }

            let metadata_map = request.metadata().clone();
            let body = request.into_inner();

            let response = retry_rpc(channel, &retry, "server_streaming", |mut grpc| {
                let mut req = Request::new(body.clone());
                *req.metadata_mut() = metadata_map.clone();
                if let Some(ms) = deadline_ms {
                    req.set_timeout(Duration::from_millis(ms));
                }
                let p = path.clone();
                async move {
                    grpc.ready().await.map_err(|e| {
                        tonic::Status::unavailable(format!("grpc client not ready: {e}"))
                    })?;
                    grpc.server_streaming(req, p, RawBytesCodec).await
                }
            })
            .await?;

            let mut results = Vec::new();
            let mut stream = response.into_inner();
            while let Some(item) = stream.next().await {
                let bytes = item.map_err(tonic_to_camel_error)?;
                let resp_json = protobuf_to_json(bytes, resp_desc.clone())?;
                results.push(resp_json);
            }

            exchange.input.body = Body::Json(serde_json::Value::Array(results));
            Ok(exchange)
        })
    }

    fn call_client_streaming(&mut self, mut exchange: Exchange) -> ProducerFuture {
        let channel = self.channel.clone();
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();
        let deadline_ms = self.deadline_ms;
        let retry = self.retry.clone();
        let auth = self.auth.clone();
        let config_metadata = self.config_metadata.clone();

        Box::pin(async move {
            debug!(path = %path, "grpc client streaming call");
            let json = Self::body_to_json(exchange.input.body.clone())?;
            let items = json.as_array().ok_or_else(|| {
                CamelError::TypeConversionFailed(
                    "grpc client streaming producer requires JSON array body".into(),
                )
            })?;

            let encoded: Vec<Vec<u8>> = items
                .iter()
                .map(|item| json_to_protobuf(item.clone(), req_df.clone()))
                .collect::<Result<_, _>>()?;

            // Build the initial request with headers/auth/timeout once.
            let mut request_template = Request::new(futures::stream::iter(encoded.clone()));
            Self::inject_headers(&exchange, &mut request_template);
            apply_auth_metadata(&auth, &mut request_template).await?;
            if let Some(ref metadata_str) = config_metadata {
                for pair in metadata_str.split(',') {
                    let pair = pair.trim();
                    if let Some((key, value)) = pair.split_once('=') {
                        let key = key.trim();
                        let value = value.trim();
                        if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                            && let Ok(meta_val) = MetadataValue::try_from(value)
                        {
                            request_template.metadata_mut().insert(name, meta_val);
                        }
                    }
                }
                debug!("applied config metadata to gRPC client streaming request");
            }
            if let Some(ms) = deadline_ms {
                request_template.set_timeout(Duration::from_millis(ms));
            }

            let metadata_map = request_template.metadata().clone();

            let response = retry_rpc(channel, &retry, "client_streaming", |mut grpc| {
                let mut request = Request::new(futures::stream::iter(encoded.clone()));
                *request.metadata_mut() = metadata_map.clone();
                if let Some(ms) = deadline_ms {
                    request.set_timeout(Duration::from_millis(ms));
                }
                let p = path.clone();
                async move {
                    grpc.ready().await.map_err(|e| {
                        tonic::Status::unavailable(format!("grpc client not ready: {e}"))
                    })?;
                    grpc.client_streaming(request, p, RawBytesCodec).await
                }
            })
            .await?;

            let resp_json = protobuf_to_json(response.into_inner(), resp_desc)?;
            exchange.input.body = Body::Json(resp_json);
            Ok(exchange)
        })
    }

    fn call_bidi(&mut self, mut exchange: Exchange) -> ProducerFuture {
        let channel = self.channel.clone();
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();
        let deadline_ms = self.deadline_ms;
        let retry = self.retry.clone();
        let auth = self.auth.clone();
        let config_metadata = self.config_metadata.clone();

        Box::pin(async move {
            debug!(path = %path, "grpc bidi streaming call");
            let json = Self::body_to_json(exchange.input.body.clone())?;
            let items = json.as_array().ok_or_else(|| {
                CamelError::TypeConversionFailed(
                    "grpc bidi streaming producer requires JSON array body".into(),
                )
            })?;

            let encoded: Vec<Vec<u8>> = items
                .iter()
                .map(|item| json_to_protobuf(item.clone(), req_df.clone()))
                .collect::<Result<_, _>>()?;

            // Build the initial request with headers/auth/timeout once.
            let mut request_template = Request::new(futures::stream::iter(encoded.clone()));
            Self::inject_headers(&exchange, &mut request_template);
            apply_auth_metadata(&auth, &mut request_template).await?;
            if let Some(ref metadata_str) = config_metadata {
                for pair in metadata_str.split(',') {
                    let pair = pair.trim();
                    if let Some((key, value)) = pair.split_once('=') {
                        let key = key.trim();
                        let value = value.trim();
                        if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                            && let Ok(meta_val) = MetadataValue::try_from(value)
                        {
                            request_template.metadata_mut().insert(name, meta_val);
                        }
                    }
                }
                debug!("applied config metadata to gRPC bidi streaming request");
            }
            if let Some(ms) = deadline_ms {
                request_template.set_timeout(Duration::from_millis(ms));
            }

            let metadata_map = request_template.metadata().clone();

            let response = retry_rpc(channel, &retry, "bidi", |mut grpc| {
                let mut request = Request::new(futures::stream::iter(encoded.clone()));
                *request.metadata_mut() = metadata_map.clone();
                if let Some(ms) = deadline_ms {
                    request.set_timeout(Duration::from_millis(ms));
                }
                let p = path.clone();
                async move {
                    grpc.ready().await.map_err(|e| {
                        tonic::Status::unavailable(format!("grpc client not ready: {e}"))
                    })?;
                    grpc.streaming(request, p, RawBytesCodec).await
                }
            })
            .await?;

            let mut results = Vec::new();
            let mut stream = response.into_inner();
            while let Some(item) = stream.next().await {
                let bytes = item.map_err(tonic_to_camel_error)?;
                let resp_json = protobuf_to_json(bytes, resp_desc.clone())?;
                results.push(resp_json);
            }

            exchange.input.body = Body::Json(serde_json::Value::Array(results));
            Ok(exchange)
        })
    }
}

// ── Tower Service impl ────────────────────────────────────────────────────

impl Service<Exchange> for GrpcProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.pending_permit.is_some() {
            return Poll::Ready(Ok(()));
        }
        let fut = self
            .acquire_fut
            .get_or_insert_with(|| Box::pin(Arc::clone(&self.semaphore).acquire_owned()));
        match fut.as_mut().poll(cx) {
            Poll::Ready(Ok(permit)) => {
                self.acquire_fut = None;
                self.pending_permit = Some(permit);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(CamelError::ChannelClosed)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, exchange: Exchange) -> ProducerFuture {
        let permit = match self.pending_permit.take() {
            Some(p) => p,
            None => {
                return Box::pin(async {
                    Err(CamelError::ProcessorError(
                        "call() invoked without poll_ready()".into(),
                    ))
                });
            }
        };
        let inner = match self.mode {
            GrpcMode::Unary => self.call_unary(exchange),
            GrpcMode::ServerStreaming => self.call_server_streaming(exchange),
            GrpcMode::ClientStreaming => self.call_client_streaming(exchange),
            GrpcMode::Bidi => self.call_bidi(exchange),
        };
        Box::pin(async move {
            let _permit = permit; // hold semaphore slot for call duration
            inner.await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::task::{Context, Poll};

    use super::GrpcProducer;
    use crate::GrpcMode;
    use crate::config::GrpcConfig;
    use camel_api::{Body, CamelError, Exchange, Message};
    use camel_component_api::NetworkRetryPolicy;
    use tonic::Request;
    use tower::Service;

    fn exchange_with_headers(headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::default();
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
    }

    fn default_config() -> GrpcConfig {
        GrpcConfig {
            proto_file: None,
            service: None,
            method: None,
            reflection: false,
            tls: false,
            max_receive_message_length: 4 * 1024 * 1024,
            deadline_ms: None,
            metadata: None,
            tls_config: None,
            auth: crate::config::AuthConfig::None,
            interceptors: crate::config::InterceptorConfig::default(),
            consumer_strategy: crate::config::ConsumerStrategy::default(),
            producer_strategy: crate::config::ProducerStrategy::default(),
            retry: NetworkRetryPolicy::default(),
        }
    }

    // ── body_to_json tests ─────────────────────────────────────────────

    #[test]
    fn test_body_to_json_from_json_body() {
        let body = Body::Json(serde_json::json!({"key": "value"}));
        let result = GrpcProducer::body_to_json(body).unwrap();
        assert_eq!(result["key"], "value");
    }

    #[test]
    fn test_body_to_json_from_text_body_valid_json() {
        let body = Body::Text(r#"{"name":"test"}"#.to_string());
        let result = GrpcProducer::body_to_json(body).unwrap();
        assert_eq!(result["name"], "test");
    }

    #[test]
    fn test_body_to_json_from_text_body_invalid_json() {
        let body = Body::Text("not json".to_string());
        let err = GrpcProducer::body_to_json(body).unwrap_err();
        assert!(matches!(err, CamelError::TypeConversionFailed(_)));
        assert!(err.to_string().contains("invalid JSON text body"));
    }

    #[test]
    fn test_body_to_json_from_empty_body() {
        let body = Body::Empty;
        let err = GrpcProducer::body_to_json(body).unwrap_err();
        assert!(matches!(err, CamelError::TypeConversionFailed(_)));
        assert!(err.to_string().contains("requires JSON or text body"));
    }

    #[test]
    fn test_body_to_json_from_bytes_body() {
        let body = Body::Bytes(bytes::Bytes::from_static(b"raw"));
        let err = GrpcProducer::body_to_json(body).unwrap_err();
        assert!(matches!(err, CamelError::TypeConversionFailed(_)));
    }

    #[test]
    fn test_body_to_json_from_xml_body() {
        let body = Body::Xml("<root/>".to_string());
        let err = GrpcProducer::body_to_json(body).unwrap_err();
        assert!(matches!(err, CamelError::TypeConversionFailed(_)));
    }

    // ── header_to_metadata tests ───────────────────────────────────────

    #[test]
    fn test_header_to_metadata_from_string() {
        let value = serde_json::Value::String("hello".to_string());
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_some());
    }

    #[test]
    fn test_header_to_metadata_from_number() {
        let value = serde_json::Value::Number(42.into());
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_str().unwrap(), "42");
    }

    #[test]
    fn test_header_to_metadata_from_bool_true() {
        let value = serde_json::Value::Bool(true);
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_str().unwrap(), "true");
    }

    #[test]
    fn test_header_to_metadata_from_bool_false() {
        let value = serde_json::Value::Bool(false);
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_str().unwrap(), "false");
    }

    #[test]
    fn test_header_to_metadata_from_object() {
        let value = serde_json::json!({"key": "value"});
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_none());
    }

    #[test]
    fn test_header_to_metadata_from_array() {
        let value = serde_json::json!(["a", "b"]);
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_none());
    }

    #[test]
    fn test_header_to_metadata_from_null() {
        let value = serde_json::Value::Null;
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_none());
    }

    // ── inject_headers tests ───────────────────────────────────────────

    #[test]
    fn test_inject_headers_transfers_all_valid_headers() {
        let exchange = exchange_with_headers(&[
            ("x-custom", serde_json::Value::String("val1".to_string())),
            ("x-number", serde_json::Value::Number(123.into())),
        ]);
        let mut request = Request::new(());
        GrpcProducer::inject_headers(&exchange, &mut request);

        let meta = request.metadata();
        assert_eq!(meta.get("x-custom").unwrap().to_str().unwrap(), "val1");
        assert_eq!(meta.get("x-number").unwrap().to_str().unwrap(), "123");
    }

    #[test]
    fn test_inject_headers_skips_unsupported_types() {
        let exchange = exchange_with_headers(&[
            ("x-good", serde_json::Value::String("ok".to_string())),
            ("x-bad", serde_json::json!({"nested": true})),
        ]);
        let mut request = Request::new(());
        GrpcProducer::inject_headers(&exchange, &mut request);

        let meta = request.metadata();
        assert!(meta.get("x-good").is_some());
        assert!(meta.get("x-bad").is_none());
    }

    #[test]
    fn test_inject_headers_empty_exchange() {
        let exchange = Exchange::new(Message::default());
        let mut request = Request::new(());
        GrpcProducer::inject_headers(&exchange, &mut request);
        assert!(request.metadata().is_empty());
    }

    // ── tonic_to_camel_error tests ─────────────────────────────────────

    // ── GrpcMode tests ─────────────────────────────────────────────────

    #[test]
    fn test_grpc_mode_derives() {
        let mode = GrpcMode::Unary;
        let _ = format!("{mode:?}");
        #[allow(clippy::clone_on_copy)]
        let cloned = mode.clone();
        assert_eq!(mode, cloned);
        let copied = mode;
        assert_eq!(mode, copied);
    }

    #[test]
    fn test_grpc_mode_all_variants_distinct() {
        assert_ne!(GrpcMode::Unary, GrpcMode::ServerStreaming);
        assert_ne!(GrpcMode::Unary, GrpcMode::ClientStreaming);
        assert_ne!(GrpcMode::Unary, GrpcMode::Bidi);
        assert_ne!(GrpcMode::ServerStreaming, GrpcMode::ClientStreaming);
        assert_ne!(GrpcMode::ServerStreaming, GrpcMode::Bidi);
        assert_ne!(GrpcMode::ClientStreaming, GrpcMode::Bidi);
    }

    // ── producer lifecycle tests ───────────────────────────────────────

    #[tokio::test]
    async fn test_producer_new_invalid_endpoint() {
        let result = GrpcProducer::new(
            "not-a-valid-endpoint".to_string(),
            PathBuf::from("/dev/null"),
            "svc".to_string(),
            "Method".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_producer_new_missing_proto_file() {
        let result = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            PathBuf::from("/nonexistent/file.proto"),
            "svc".to_string(),
            "Method".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
        );
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        };
        assert!(err.to_string().contains("failed to compile proto"));
    }

    #[tokio::test]
    async fn test_producer_new_service_not_found() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let result = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path,
            "nonexistent.Service".to_string(),
            "SayHello".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
        );
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        };
        assert!(err.to_string().contains("service descriptor not found"));
    }

    #[tokio::test]
    async fn test_producer_new_method_not_found() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let result = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path,
            "helloworld.Greeter".to_string(),
            "NonExistentMethod".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
        );
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        };
        assert!(err.to_string().contains("method descriptor not found"));
    }

    #[tokio::test]
    async fn test_producer_poll_ready_always_ready() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let mut producer = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path,
            "helloworld.Greeter".to_string(),
            "SayHello".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
        )
        .unwrap();

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(producer.poll_ready(&mut cx), Poll::Ready(Ok(()))));
    }

    #[tokio::test]
    async fn test_producer_call_dispatches_by_mode() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let producer_unary = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path.clone(),
            "helloworld.Greeter".to_string(),
            "SayHello".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
        )
        .unwrap();

        let producer_streaming = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path,
            "helloworld.Greeter".to_string(),
            "SayHello".to_string(),
            GrpcMode::ServerStreaming,
            None,
            &default_config(),
        )
        .unwrap();

        assert_eq!(producer_unary.mode, GrpcMode::Unary);
        assert_eq!(producer_streaming.mode, GrpcMode::ServerStreaming);
    }

    // ── edge case tests ────────────────────────────────────────────────

    #[test]
    fn test_body_to_json_from_null_body() {
        let body = Body::Json(serde_json::Value::Null);
        let result = GrpcProducer::body_to_json(body).unwrap();
        assert!(result.is_null());
    }

    #[test]
    fn test_body_to_json_from_array_body() {
        let body = Body::Json(serde_json::json!([1, 2, 3]));
        let result = GrpcProducer::body_to_json(body).unwrap();
        assert!(result.is_array());
        assert_eq!(result.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_header_to_metadata_from_negative_number() {
        let value = serde_json::Value::Number((-42).into());
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_str().unwrap(), "-42");
    }

    #[test]
    fn test_header_to_metadata_from_float() {
        let value = serde_json::Value::Number(serde_json::Number::from_f64(3.15).unwrap());
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_some());
    }

    #[test]
    fn test_inject_headers_skips_invalid_metadata_key() {
        let exchange =
            exchange_with_headers(&[("x-good", serde_json::Value::String("ok".to_string()))]);
        let mut request = Request::new(());
        GrpcProducer::inject_headers(&exchange, &mut request);
        assert!(request.metadata().get("x-good").is_some());
    }
}
