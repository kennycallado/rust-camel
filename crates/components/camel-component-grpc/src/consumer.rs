use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
use base64::Engine;
use bytes::BytesMut;
use camel_api::{Body, CamelError, Exchange, Message};
use camel_component_api::{ConcurrencyModel, Consumer, ConsumerContext, ExchangeEnvelope};
use camel_proto_compiler::ProtoCache;
use prost::Message as _;
use prost_reflect::{DynamicMessage, MessageDescriptor};
use tokio::sync::mpsc;
use tonic::Status;

use crate::server::GrpcDispatchTable;
use crate::server::GrpcServerRegistry;

static PROTO_CACHE: OnceLock<ProtoCache> = OnceLock::new();

fn proto_cache() -> &'static ProtoCache {
    PROTO_CACHE.get_or_init(ProtoCache::new)
}

const RESERVED_METADATA_KEYS: &[&str] = &[
    "content-type",
    "te",
    "grpc-encoding",
    "grpc-accept-encoding",
    "grpc-status",
    "grpc-message",
    "grpc-status-details-bin",
    "user-agent",
];

fn extract_metadata(metadata: &tonic::metadata::MetadataMap) -> Vec<(String, serde_json::Value)> {
    let mut headers = Vec::new();
    for key_and_value in metadata.iter() {
        use tonic::metadata::KeyAndValueRef;
        match key_and_value {
            KeyAndValueRef::Ascii(key, value) => {
                let key_str = key.as_str();
                if RESERVED_METADATA_KEYS.contains(&key_str) {
                    continue;
                }
                if let Ok(v) = value.to_str() {
                    headers.push((key_str.to_string(), serde_json::Value::String(v.to_string())));
                }
            }
            KeyAndValueRef::Binary(key, value) => {
                let key_str = key.as_str();
                if RESERVED_METADATA_KEYS.contains(&key_str) {
                    continue;
                }
                let encoded = base64::engine::general_purpose::STANDARD.encode(value);
                headers.push((format!("bin:{key_str}"), serde_json::Value::String(encoded)));
            }
        }
    }
    headers
}

pub(crate) struct GrpcRequestEnvelope {
    pub metadata: tonic::metadata::MetadataMap,
    pub body: Vec<u8>,
    pub reply_tx: tokio::sync::oneshot::Sender<GrpcReply>,
}

pub(crate) enum GrpcReply {
    Ok(Vec<u8>),
    Err(tonic::Status),
}

pub struct GrpcConsumer {
    host: String,
    port: u16,
    path: String,
    proto_path: PathBuf,
    service_name: String,
    method_name: String,
}

impl GrpcConsumer {
    pub fn new(
        host: String,
        port: u16,
        path: String,
        proto_path: PathBuf,
        service_name: String,
        method_name: String,
    ) -> Self {
        Self {
            host,
            port,
            path,
            proto_path,
            service_name,
            method_name,
        }
    }

    fn resolve_descriptors(&self) -> Result<(MessageDescriptor, MessageDescriptor), CamelError> {
        let cache = proto_cache();
        let pool = cache
            .get_or_compile(&self.proto_path, std::iter::empty::<&std::path::Path>())
            .map_err(|e| {
                CamelError::EndpointCreationFailed(format!("failed to compile proto: {e}"))
            })?;

        let svc = pool.get_service_by_name(&self.service_name).ok_or_else(|| {
            CamelError::EndpointCreationFailed(format!(
                "service descriptor not found: {}",
                self.service_name
            ))
        })?;

        let method = svc
            .methods()
            .find(|m| m.name() == self.method_name)
            .ok_or_else(|| {
                CamelError::EndpointCreationFailed(format!(
                    "method descriptor not found: {}/{}",
                    self.service_name, self.method_name
                ))
            })?;

        Ok((method.input(), method.output()))
    }

    pub async fn start_with_listener(
        &mut self,
        ctx: ConsumerContext,
        listener: tokio::net::TcpListener,
    ) -> Result<(), CamelError> {
        let dispatch = GrpcServerRegistry::global()
            .get_or_spawn_with_listener(listener, &self.host, self.port)
            .await?;
        self.start_inner(ctx, dispatch).await
    }

    async fn start_inner(
        &mut self,
        ctx: ConsumerContext,
        dispatch: GrpcDispatchTable,
    ) -> Result<(), CamelError> {
        // Resolve descriptors FIRST — can fail, must not leave stale dispatch entry
        let (req_desc, resp_desc) = self.resolve_descriptors()?;

        let (env_tx, mut env_rx) = mpsc::channel::<GrpcRequestEnvelope>(64);
        {
            let mut table = dispatch.write().await;
            if table.contains_key(&self.path) {
                return Err(CamelError::EndpointCreationFailed(format!(
                    "duplicate gRPC consumer path: {}",
                    self.path
                )));
            }
            table.insert(self.path.clone(), env_tx);
        }

        let path = self.path.clone();
        let host = self.host.clone();
        let port = self.port;
        let sender = ctx.sender();

        // Bounded concurrency: semaphore limits in-flight requests; JoinSet tracks them.
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(64));
        let mut join_set = tokio::task::JoinSet::new();

        loop {
            tokio::select! {
                biased;
                _ = ctx.cancelled() => {
                    break;
                }
                envelope = env_rx.recv() => {
                    let Some(envelope) = envelope else { break };

                    let sem = semaphore.clone();
                    let permit = sem.acquire_owned().await.expect("semaphore closed");
                    let req_desc = req_desc.clone();
                    let resp_desc = resp_desc.clone();
                    let sender = sender.clone();

                    join_set.spawn(async move {
                        let _permit = permit; // held for duration of task
                        let result = process_unary_request(
                            envelope.body,
                            envelope.metadata,
                            req_desc,
                            resp_desc,
                            sender,
                        )
                        .await;
                        let reply = match result {
                            Ok(bytes) => GrpcReply::Ok(bytes),
                            Err(status) => GrpcReply::Err(status),
                        };
                        let _ = envelope.reply_tx.send(reply);
                    });
                }
            }
        }

        // Wait for in-flight requests to complete before deregistering
        join_set.shutdown().await;

        GrpcServerRegistry::global()
            .unregister(&host, port, &path)
            .await;

        Ok(())
    }
}

#[async_trait]
impl Consumer for GrpcConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        let dispatch = GrpcServerRegistry::global()
            .get_or_spawn(&self.host, self.port)
            .await?;
        self.start_inner(ctx, dispatch).await
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        GrpcServerRegistry::global()
            .unregister(&self.host, self.port, &self.path)
            .await;
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Concurrent { max: None }
    }
}

async fn process_unary_request(
    body: Vec<u8>,
    metadata: tonic::metadata::MetadataMap,
    req_desc: MessageDescriptor,
    resp_desc: MessageDescriptor,
    sender: mpsc::Sender<ExchangeEnvelope>,
) -> Result<Vec<u8>, Status> {
    let req_dyn = DynamicMessage::decode(req_desc, body.as_slice()).map_err(|e| {
        Status::invalid_argument(format!("failed to decode protobuf: {e}"))
    })?;

    let json = serde_json::to_value(&req_dyn)
        .map_err(|e| Status::invalid_argument(format!("failed to convert protobuf to JSON: {e}")))?;

    let mut msg = Message::new(Body::Json(json));
    for (k, v) in extract_metadata(&metadata) {
        msg.set_header(k, v);
    }

    let exchange = Exchange::new(msg);

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let envelope = ExchangeEnvelope {
        exchange,
        reply_tx: Some(reply_tx),
    };

    sender
        .send(envelope)
        .await
        .map_err(|_| Status::internal("pipeline channel closed"))?;

    let result = reply_rx
        .await
        .map_err(|_| Status::internal("pipeline reply dropped"))?
        .map_err(|e| Status::internal(format!("pipeline error: {e}")))?;

    let resp_json = match result.input.body {
        Body::Json(v) => v,
        other => return Err(Status::internal(format!(
            "expected JSON response body from pipeline, got {other:?}"
        ))),
    };

    let json_str = serde_json::to_string(&resp_json)
        .map_err(|e| Status::internal(format!("failed to serialize response JSON: {e}")))?;
    let mut de = serde_json::Deserializer::from_str(&json_str);
    let resp_dyn = DynamicMessage::deserialize(resp_desc, &mut de)
        .map_err(|e| Status::internal(format!("failed to parse JSON into protobuf: {e}")))?;

    let mut buf = BytesMut::new();
    resp_dyn.encode(&mut buf).map_err(|e| {
        Status::internal(format!("failed to encode protobuf response: {e}"))
    })?;

    Ok(buf.to_vec())
}
