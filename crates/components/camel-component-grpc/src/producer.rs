use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};

use bytes::BytesMut;
use camel_api::{Body, CamelError, Exchange};
use camel_proto_compiler::ProtoCache;
use http::uri::PathAndQuery;
use prost::Message as _;
use prost_reflect::{DynamicMessage, MessageDescriptor};
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Status};
use tower::Service;

use crate::codec::RawBytesCodec;

static PROTO_CACHE: OnceLock<ProtoCache> = OnceLock::new();

fn proto_cache() -> &'static ProtoCache {
    PROTO_CACHE.get_or_init(ProtoCache::new)
}

#[derive(Clone)]
pub struct GrpcProducer {
    channel: Channel,
    path: PathAndQuery,
    req_descriptor: MessageDescriptor,
    resp_descriptor: MessageDescriptor,
}

impl GrpcProducer {
    pub fn new(
        addr: String,
        proto_path: PathBuf,
        service_name: String,
        method_name: String,
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

    fn tonic_to_camel_error(status: Status) -> CamelError {
        CamelError::ProcessorError(format!("grpc call failed: {status}"))
    }
}

impl Service<Exchange> for GrpcProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let mut grpc = tonic::client::Grpc::new(self.channel.clone());
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();

        Box::pin(async move {
            let json = Self::body_to_json(exchange.input.body.clone())?;
            let json_str = serde_json::to_string(&json).map_err(|e| {
                CamelError::TypeConversionFailed(format!("failed to serialize JSON: {e}"))
            })?;
            let mut de = serde_json::Deserializer::from_str(&json_str);
            let req_dyn = DynamicMessage::deserialize(req_df, &mut de).map_err(|e| {
                CamelError::TypeConversionFailed(format!("failed to parse JSON into protobuf: {e}"))
            })?;
            let mut req_buf = BytesMut::new();
            req_dyn.encode(&mut req_buf).map_err(|e| {
                CamelError::TypeConversionFailed(format!("failed to encode protobuf message: {e}"))
            })?;
            let mut request = Request::new(req_buf.to_vec());
            for (k, v) in &exchange.input.headers {
                if let Some(meta) = Self::header_to_metadata(v)
                    && let Ok(name) = tonic::metadata::MetadataKey::from_bytes(k.as_bytes())
                {
                    request.metadata_mut().insert(name, meta);
                }
            }
            grpc.ready()
                .await
                .map_err(|e| CamelError::ProcessorError(format!("grpc client not ready: {e}")))?;
            let response = grpc
                .unary(request, path, RawBytesCodec)
                .await
                .map_err(Self::tonic_to_camel_error)?;
            let resp_dyn = DynamicMessage::decode(resp_desc, response.get_ref().as_slice())
                .map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "failed to decode protobuf bytes: {e}"
                    ))
                })?;
            let resp_json = serde_json::to_value(&resp_dyn).map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "failed to serialize protobuf to JSON: {e}"
                ))
            })?;
            exchange.input.body = Body::Json(resp_json);
            Ok(exchange)
        })
    }
}
