use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};

use bytes::BytesMut;
use camel_api::{Body, CamelError, Exchange};
use camel_proto_compiler::ProtoCache;
use futures::StreamExt;
use http::uri::PathAndQuery;
use prost::Message as _;
use prost_reflect::{DynamicMessage, MessageDescriptor};
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Status};
use tower::Service;

use crate::codec::RawBytesCodec;
use crate::mode::GrpcMode;

type ProducerFuture = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

static PROTO_CACHE: OnceLock<ProtoCache> = OnceLock::new();

fn proto_cache() -> &'static ProtoCache {
    PROTO_CACHE.get_or_init(ProtoCache::new)
}

fn json_to_protobuf(
    json: serde_json::Value,
    desc: MessageDescriptor,
) -> Result<Vec<u8>, CamelError> {
    let json_str = serde_json::to_string(&json)
        .map_err(|e| CamelError::TypeConversionFailed(format!("failed to serialize JSON: {e}")))?;
    let mut de = serde_json::Deserializer::from_str(&json_str);
    let dyn_msg = DynamicMessage::deserialize(desc, &mut de).map_err(|e| {
        CamelError::TypeConversionFailed(format!("failed to parse JSON into protobuf: {e}"))
    })?;
    let mut buf = BytesMut::new();
    dyn_msg.encode(&mut buf).map_err(|e| {
        CamelError::TypeConversionFailed(format!("failed to encode protobuf message: {e}"))
    })?;
    Ok(buf.to_vec())
}

fn protobuf_to_json(
    bytes: Vec<u8>,
    desc: MessageDescriptor,
) -> Result<serde_json::Value, CamelError> {
    let dyn_msg = DynamicMessage::decode(desc, bytes.as_slice()).map_err(|e| {
        CamelError::TypeConversionFailed(format!("failed to decode protobuf bytes: {e}"))
    })?;
    serde_json::to_value(&dyn_msg).map_err(|e| {
        CamelError::TypeConversionFailed(format!("failed to serialize protobuf to JSON: {e}"))
    })
}

#[derive(Clone)]
pub struct GrpcProducer {
    channel: Channel,
    path: PathAndQuery,
    req_descriptor: MessageDescriptor,
    resp_descriptor: MessageDescriptor,
    mode: GrpcMode,
}

impl GrpcProducer {
    pub fn new(
        addr: String,
        proto_path: PathBuf,
        service_name: String,
        method_name: String,
        mode: GrpcMode,
    ) -> Result<Self, CamelError> {
        let endpoint = Endpoint::from_shared(addr).map_err(|e| {
            CamelError::EndpointCreationFailed(format!("invalid grpc endpoint: {e}"))
        })?;
        let channel = endpoint.connect_lazy();

        let cache = proto_cache();
        let pool = cache
            .get_or_compile(&proto_path, std::iter::empty::<&Path>())
            .map_err(|e| {
                CamelError::EndpointCreationFailed(format!("failed to compile proto: {e}"))
            })?;

        let svc = pool.get_service_by_name(&service_name).ok_or_else(|| {
            CamelError::EndpointCreationFailed(format!(
                "service descriptor not found: {service_name}"
            ))
        })?;

        let method = svc
            .methods()
            .find(|m| m.name() == method_name)
            .ok_or_else(|| {
                CamelError::EndpointCreationFailed(format!(
                    "method descriptor not found: {service_name}/{method_name}"
                ))
            })?;
        let req_descriptor = method.input();
        let resp_descriptor = method.output();
        let path = PathAndQuery::from_maybe_shared(format!("/{service_name}/{method_name}"))
            .map_err(|e| CamelError::EndpointCreationFailed(format!("invalid gRPC path: {e}")))?;

        Ok(Self {
            channel,
            path,
            req_descriptor,
            resp_descriptor,
            mode,
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

    fn tonic_to_camel_error(status: Status) -> CamelError {
        CamelError::ProcessorError(format!("grpc call failed: {status}"))
    }

    fn call_unary(&mut self, mut exchange: Exchange) -> ProducerFuture {
        let mut grpc = tonic::client::Grpc::new(self.channel.clone());
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();

        Box::pin(async move {
            let json = Self::body_to_json(exchange.input.body.clone())?;
            let buf = json_to_protobuf(json, req_df)?;
            let mut request = Request::new(buf);
            Self::inject_headers(&exchange, &mut request);

            grpc.ready()
                .await
                .map_err(|e| CamelError::ProcessorError(format!("grpc client not ready: {e}")))?;

            let response = grpc
                .unary(request, path, RawBytesCodec)
                .await
                .map_err(Self::tonic_to_camel_error)?;

            let resp_json = protobuf_to_json(response.into_inner(), resp_desc)?;
            exchange.input.body = Body::Json(resp_json);
            Ok(exchange)
        })
    }

    fn call_server_streaming(&mut self, mut exchange: Exchange) -> ProducerFuture {
        let mut grpc = tonic::client::Grpc::new(self.channel.clone());
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();

        Box::pin(async move {
            let json = Self::body_to_json(exchange.input.body.clone())?;
            let buf = json_to_protobuf(json, req_df)?;
            let mut request = Request::new(buf);
            Self::inject_headers(&exchange, &mut request);

            grpc.ready()
                .await
                .map_err(|e| CamelError::ProcessorError(format!("grpc client not ready: {e}")))?;

            let response = grpc
                .server_streaming(request, path, RawBytesCodec)
                .await
                .map_err(Self::tonic_to_camel_error)?;

            let mut results = Vec::new();
            let mut stream = response.into_inner();
            while let Some(item) = stream.next().await {
                let bytes = item.map_err(Self::tonic_to_camel_error)?;
                let resp_json = protobuf_to_json(bytes, resp_desc.clone())?;
                results.push(resp_json);
            }

            exchange.input.body = Body::Json(serde_json::Value::Array(results));
            Ok(exchange)
        })
    }

    fn call_client_streaming(&mut self, mut exchange: Exchange) -> ProducerFuture {
        let mut grpc = tonic::client::Grpc::new(self.channel.clone());
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();

        Box::pin(async move {
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

            let stream = futures::stream::iter(encoded);

            let mut request = Request::new(stream);
            Self::inject_headers(&exchange, &mut request);

            grpc.ready()
                .await
                .map_err(|e| CamelError::ProcessorError(format!("grpc client not ready: {e}")))?;

            let response = grpc
                .client_streaming(request, path, RawBytesCodec)
                .await
                .map_err(Self::tonic_to_camel_error)?;

            let resp_json = protobuf_to_json(response.into_inner(), resp_desc)?;
            exchange.input.body = Body::Json(resp_json);
            Ok(exchange)
        })
    }

    fn call_bidi(&mut self, mut exchange: Exchange) -> ProducerFuture {
        let mut grpc = tonic::client::Grpc::new(self.channel.clone());
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();

        Box::pin(async move {
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

            let stream = futures::stream::iter(encoded);

            let mut request = Request::new(stream);
            Self::inject_headers(&exchange, &mut request);

            grpc.ready()
                .await
                .map_err(|e| CamelError::ProcessorError(format!("grpc client not ready: {e}")))?;

            let response = grpc
                .streaming(request, path, RawBytesCodec)
                .await
                .map_err(Self::tonic_to_camel_error)?;

            let mut results = Vec::new();
            let mut stream = response.into_inner();
            while let Some(item) = stream.next().await {
                let bytes = item.map_err(Self::tonic_to_camel_error)?;
                let resp_json = protobuf_to_json(bytes, resp_desc.clone())?;
                results.push(resp_json);
            }

            exchange.input.body = Body::Json(serde_json::Value::Array(results));
            Ok(exchange)
        })
    }
}

impl Service<Exchange> for GrpcProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> ProducerFuture {
        match self.mode {
            GrpcMode::Unary => self.call_unary(exchange),
            GrpcMode::ServerStreaming => self.call_server_streaming(exchange),
            GrpcMode::ClientStreaming => self.call_client_streaming(exchange),
            GrpcMode::Bidi => self.call_bidi(exchange),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GrpcProducer;
    use crate::GrpcMode;
    use camel_api::{Body, CamelError, Exchange, Message};
    use tonic::Request;

    fn exchange_with_headers(headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::default();
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
    }

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

    #[test]
    fn test_tonic_to_camel_error_unavailable() {
        let status = tonic::Status::unavailable("service down");
        let err = GrpcProducer::tonic_to_camel_error(status);
        assert!(matches!(err, CamelError::ProcessorError(_)));
        assert!(err.to_string().contains("grpc call failed"));
        assert!(err.to_string().contains("service down"));
    }

    #[test]
    fn test_tonic_to_camel_error_not_found() {
        let status = tonic::Status::not_found("method not found");
        let err = GrpcProducer::tonic_to_camel_error(status);
        assert!(matches!(err, CamelError::ProcessorError(_)));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_tonic_to_camel_error_deadline_exceeded() {
        let status = tonic::Status::deadline_exceeded("timeout");
        let err = GrpcProducer::tonic_to_camel_error(status);
        assert!(matches!(err, CamelError::ProcessorError(_)));
        assert!(err.to_string().contains("grpc call failed"));
        assert!(err.to_string().to_lowercase().contains("deadline"));
    }

    #[test]
    fn test_grpc_mode_derives() {
        let mode = GrpcMode::Unary;
        let _ = format!("{mode:?}");
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
}
