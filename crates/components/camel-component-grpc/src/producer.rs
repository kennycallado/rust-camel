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
    let json_str = serde_json::to_string(&json).map_err(|e| {
        CamelError::TypeConversionFailed(format!("failed to serialize JSON: {e}"))
    })?;
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
        CamelError::TypeConversionFailed(format!(
            "failed to serialize protobuf to JSON: {e}"
        ))
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
