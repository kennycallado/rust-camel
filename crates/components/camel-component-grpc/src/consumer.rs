use std::collections::HashMap as StdHashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use base64::Engine;
use bytes::BytesMut;
use camel_api::{Body, CamelError, Exchange, Message, Value};
use camel_component_api::{ConcurrencyModel, Consumer, ConsumerContext, ExchangeEnvelope};
use camel_proto_compiler::ProtoCache;
use prost::Message as _;
use prost_reflect::{DynamicMessage, MessageDescriptor};
use tokio::sync::mpsc;
use tonic::Status;

use crate::mode::GrpcMode;
use crate::server::GrpcDispatchTable;
use crate::server::GrpcServerRegistry;

static PROTO_CACHE: OnceLock<ProtoCache> = OnceLock::new();

fn proto_cache() -> &'static ProtoCache {
    PROTO_CACHE.get_or_init(ProtoCache::new)
}

/// Resolve the gRPC mode (unary/streaming) for a given method without creating a consumer.
pub fn resolve_grpc_mode(
    proto_path: &PathBuf,
    service_name: &str,
    method_name: &str,
) -> Result<GrpcMode, CamelError> {
    let cache = proto_cache();
    let pool = cache
        .get_or_compile(proto_path, std::iter::empty::<&std::path::Path>())
        .map_err(|e| CamelError::EndpointCreationFailed(format!("failed to compile proto: {e}")))?;

    let svc = pool.get_service_by_name(service_name).ok_or_else(|| {
        CamelError::EndpointCreationFailed(format!(
            "service descriptor not found: {}",
            service_name
        ))
    })?;

    let method = svc
        .methods()
        .find(|m| m.name() == method_name)
        .ok_or_else(|| {
            CamelError::EndpointCreationFailed(format!(
                "method descriptor not found: {}/{}",
                service_name, method_name
            ))
        })?;

    Ok(GrpcMode::from_method(&method))
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
                    headers.push((
                        key_str.to_string(),
                        serde_json::Value::String(v.to_string()),
                    ));
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

pub(crate) enum GrpcStreamItem {
    Message(Vec<u8>),
    Error(tonic::Status),
    Done,
}

pub(crate) enum GrpcReply {
    Ok(Vec<u8>),
    Err(tonic::Status),
}

pub(crate) enum GrpcRequestEnvelope {
    Unary {
        metadata: tonic::metadata::MetadataMap,
        body: Vec<u8>,
        reply_tx: tokio::sync::oneshot::Sender<GrpcReply>,
    },
    ServerStreaming {
        metadata: tonic::metadata::MetadataMap,
        body: Vec<u8>,
        reply_tx: mpsc::Sender<GrpcStreamItem>,
    },
    ClientStreaming {
        metadata: tonic::metadata::MetadataMap,
        body_rx: mpsc::Receiver<Vec<u8>>,
        reply_tx: tokio::sync::oneshot::Sender<GrpcReply>,
    },
    Bidi {
        metadata: tonic::metadata::MetadataMap,
        body_rx: mpsc::Receiver<Vec<u8>>,
        reply_tx: mpsc::Sender<GrpcStreamItem>,
    },
}

// ── Observer registry ──────────────────────────────────────────────────────

static OBSERVER_REGISTRY: OnceLock<std::sync::Mutex<StdHashMap<String, GrpcStreamObserver>>> =
    OnceLock::new();

static OBSERVER_COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_observer_id() -> String {
    let n = OBSERVER_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("obs-{n}")
}

fn observer_registry() -> &'static std::sync::Mutex<StdHashMap<String, GrpcStreamObserver>> {
    OBSERVER_REGISTRY.get_or_init(|| std::sync::Mutex::new(StdHashMap::new()))
}

fn register_observer(id: String, observer: GrpcStreamObserver) {
    let registry = observer_registry();
    let mut registry = match registry.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    registry.insert(id, observer);
}

fn remove_observer(id: &str) -> Option<GrpcStreamObserver> {
    let registry = observer_registry();
    let mut registry = match registry.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    registry.remove(id)
}

pub fn take_stream_observer(exchange: &Exchange) -> Option<GrpcStreamObserver> {
    let id = exchange
        .properties
        .get("CamelGrpcStreamObserverId")?
        .as_str()?;
    remove_observer(id)
}

// ── Observer guard (auto-cleanup on Drop) ──────────────────────────────────

struct ObserverGuard {
    id: String,
}

impl ObserverGuard {
    fn new(id: String) -> Self {
        Self { id }
    }
}

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        remove_observer(&self.id);
    }
}

// ── GrpcStreamObserver ─────────────────────────────────────────────────────

#[derive(Clone)]
pub struct GrpcStreamObserver {
    tx: mpsc::Sender<GrpcStreamItem>,
    resp_desc: MessageDescriptor,
}

impl GrpcStreamObserver {
    pub(crate) fn new(tx: mpsc::Sender<GrpcStreamItem>, resp_desc: MessageDescriptor) -> Self {
        Self { tx, resp_desc }
    }

    pub async fn on_next(&self, json: serde_json::Value) -> Result<(), CamelError> {
        let encoded = json_to_protobuf_bytes(json, self.resp_desc.clone())
            .map_err(|e| CamelError::ProcessorError(format!("failed to encode protobuf: {e}")))?;
        self.tx
            .send(GrpcStreamItem::Message(encoded))
            .await
            .map_err(|_| CamelError::ProcessorError("stream observer channel closed".into()))
    }

    pub async fn on_error(&self, status: Status) {
        if self.tx.send(GrpcStreamItem::Error(status)).await.is_err() {
            tracing::debug!("grpc stream observer: failed to send error, channel closed");
        }
    }

    pub async fn on_completed(&self) {
        if self.tx.send(GrpcStreamItem::Done).await.is_err() {
            tracing::debug!("grpc stream observer: failed to send done, channel closed");
        }
    }
}

// ── Helper ─────────────────────────────────────────────────────────────────

fn json_to_protobuf_bytes(
    json: serde_json::Value,
    desc: MessageDescriptor,
) -> Result<Vec<u8>, Status> {
    let json_str = serde_json::to_string(&json)
        .map_err(|e| Status::internal(format!("failed to serialize JSON: {e}")))?;
    let mut de = serde_json::Deserializer::from_str(&json_str);
    let resp_dyn = DynamicMessage::deserialize(desc, &mut de)
        .map_err(|e| Status::internal(format!("failed to parse JSON into protobuf: {e}")))?;
    let mut buf = BytesMut::new();
    prost::Message::encode(&resp_dyn, &mut buf)
        .map_err(|e| Status::internal(format!("failed to encode protobuf: {e}")))?;
    Ok(buf.to_vec())
}

// ── GrpcConsumer ───────────────────────────────────────────────────────────

pub struct GrpcConsumer {
    host: String,
    port: u16,
    path: String,
    proto_path: PathBuf,
    service_name: String,
    method_name: String,
    mode: GrpcMode,
}

impl GrpcConsumer {
    pub fn new(
        host: String,
        port: u16,
        path: String,
        proto_path: PathBuf,
        service_name: String,
        method_name: String,
        mode: GrpcMode,
    ) -> Self {
        Self {
            host,
            port,
            path,
            proto_path,
            service_name,
            method_name,
            mode,
        }
    }

    fn resolve_descriptors(&self) -> Result<(MessageDescriptor, MessageDescriptor), CamelError> {
        let cache = proto_cache();
        let pool = cache
            .get_or_compile(&self.proto_path, std::iter::empty::<&std::path::Path>())
            .map_err(|e| {
                CamelError::EndpointCreationFailed(format!("failed to compile proto: {e}"))
            })?;

        let svc = pool
            .get_service_by_name(&self.service_name)
            .ok_or_else(|| {
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
        let (req_desc, resp_desc) = self.resolve_descriptors()?;
        let mode = self.mode;

        let (env_tx, mut env_rx) = mpsc::channel::<GrpcRequestEnvelope>(64);
        {
            let mut table = dispatch.write().await;
            if table.contains_key(&self.path) {
                return Err(CamelError::EndpointCreationFailed(format!(
                    "duplicate gRPC consumer path: {}",
                    self.path
                )));
            }
            table.insert(self.path.clone(), (env_tx, mode));
        }

        let path = self.path.clone();
        let host = self.host.clone();
        let port = self.port;
        let sender = ctx.sender();

        // NOTE: Long-running bidi streams hold a semaphore permit for their duration.
        // If this becomes an issue, consider separate concurrency limits for streaming vs unary.
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
                        let _permit = permit;
                        match envelope {
                            GrpcRequestEnvelope::Unary { metadata, body, reply_tx } => {
                                let result = process_unary_request(
                                    body, metadata, req_desc, resp_desc, sender,
                                ).await;
                                let reply = match result {
                                    Ok(bytes) => GrpcReply::Ok(bytes),
                                    Err(status) => GrpcReply::Err(status),
                                };
                                let _ = reply_tx.send(reply);
                            }
                            GrpcRequestEnvelope::ServerStreaming { metadata, body, reply_tx } => {
                                process_server_streaming_request(
                                    body, metadata, req_desc, resp_desc, sender, reply_tx,
                                ).await;
                            }
                            GrpcRequestEnvelope::ClientStreaming { metadata, body_rx, reply_tx } => {
                                process_client_streaming_request(
                                    body_rx, metadata, req_desc, resp_desc, sender, reply_tx,
                                ).await;
                            }
                            GrpcRequestEnvelope::Bidi { metadata, body_rx, reply_tx } => {
                                process_bidi_request(
                                    body_rx, metadata, req_desc, resp_desc, sender, reply_tx,
                                ).await;
                            }
                        }
                    });
                }
            }
        }

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

// ── Unary processor (unchanged) ────────────────────────────────────────────

async fn process_unary_request(
    body: Vec<u8>,
    metadata: tonic::metadata::MetadataMap,
    req_desc: MessageDescriptor,
    resp_desc: MessageDescriptor,
    sender: mpsc::Sender<ExchangeEnvelope>,
) -> Result<Vec<u8>, Status> {
    let req_dyn = DynamicMessage::decode(req_desc, body.as_slice())
        .map_err(|e| Status::invalid_argument(format!("failed to decode protobuf: {e}")))?;

    let json = serde_json::to_value(&req_dyn).map_err(|e| {
        Status::invalid_argument(format!("failed to convert protobuf to JSON: {e}"))
    })?;

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
        other => {
            return Err(Status::internal(format!(
                "expected JSON response body from pipeline, got {other:?}"
            )));
        }
    };

    let json_str = serde_json::to_string(&resp_json)
        .map_err(|e| Status::internal(format!("failed to serialize response JSON: {e}")))?;
    let mut de = serde_json::Deserializer::from_str(&json_str);
    let resp_dyn = DynamicMessage::deserialize(resp_desc, &mut de)
        .map_err(|e| Status::internal(format!("failed to parse JSON into protobuf: {e}")))?;

    let mut buf = BytesMut::new();
    resp_dyn
        .encode(&mut buf)
        .map_err(|e| Status::internal(format!("failed to encode protobuf response: {e}")))?;

    Ok(buf.to_vec())
}

// ── Server-streaming processor ─────────────────────────────────────────────

async fn process_server_streaming_request(
    body: Vec<u8>,
    metadata: tonic::metadata::MetadataMap,
    req_desc: MessageDescriptor,
    resp_desc: MessageDescriptor,
    sender: mpsc::Sender<ExchangeEnvelope>,
    reply_tx: mpsc::Sender<GrpcStreamItem>,
) {
    let req_dyn = match DynamicMessage::decode(req_desc, body.as_slice()) {
        Ok(m) => m,
        Err(e) => {
            let _ = reply_tx
                .send(GrpcStreamItem::Error(Status::invalid_argument(format!(
                    "failed to decode protobuf: {e}"
                ))))
                .await;
            return;
        }
    };

    let json = match serde_json::to_value(&req_dyn) {
        Ok(v) => v,
        Err(e) => {
            let _ = reply_tx
                .send(GrpcStreamItem::Error(Status::invalid_argument(format!(
                    "failed to convert protobuf to JSON: {e}"
                ))))
                .await;
            return;
        }
    };

    let mut msg = Message::new(Body::Json(json));
    for (k, v) in extract_metadata(&metadata) {
        msg.set_header(k, v);
    }

    let observer = GrpcStreamObserver::new(reply_tx.clone(), resp_desc);
    let observer_id = next_observer_id();
    register_observer(observer_id.clone(), observer);
    let _guard = ObserverGuard::new(observer_id.clone());

    let mut exchange = Exchange::new(msg);
    exchange.set_property("CamelGrpcStreamObserverId", Value::String(observer_id));

    let envelope = ExchangeEnvelope {
        exchange,
        reply_tx: None,
    };

    if sender.send(envelope).await.is_err() {
        let _ = reply_tx
            .send(GrpcStreamItem::Error(Status::internal(
                "pipeline channel closed",
            )))
            .await;
    }

    // Wait for the stream receiver to be dropped (stream complete).
    // This keeps the guard alive so the observer stays registered until
    // the route is done. If take_stream_observer was called, the guard's
    // Drop is a no-op. If not, the guard cleans up the leaked observer.
    reply_tx.closed().await;
}

// ── Client-streaming processor ─────────────────────────────────────────────

async fn process_client_streaming_request(
    mut body_rx: mpsc::Receiver<Vec<u8>>,
    metadata: tonic::metadata::MetadataMap,
    req_desc: MessageDescriptor,
    resp_desc: MessageDescriptor,
    sender: mpsc::Sender<ExchangeEnvelope>,
    reply_tx: tokio::sync::oneshot::Sender<GrpcReply>,
) {
    while let Some(body) = body_rx.recv().await {
        let req_dyn = match DynamicMessage::decode(req_desc.clone(), body.as_slice()) {
            Ok(d) => d,
            Err(e) => {
                let _ = reply_tx.send(GrpcReply::Err(Status::invalid_argument(format!(
                    "failed to decode protobuf: {e}"
                ))));
                return;
            }
        };

        let json = match serde_json::to_value(&req_dyn) {
            Ok(j) => j,
            Err(e) => {
                let _ = reply_tx.send(GrpcReply::Err(Status::internal(format!(
                    "failed to convert protobuf to JSON: {e}"
                ))));
                return;
            }
        };

        let mut msg = Message::new(Body::Json(json));
        for (k, v) in extract_metadata(&metadata) {
            msg.set_header(k, v);
        }
        msg.set_header(
            "CamelGrpcClientStreaming".to_string(),
            serde_json::Value::Bool(true),
        );

        let exchange = Exchange::new(msg);
        let (reply_tx_pipe, reply_rx_pipe) = tokio::sync::oneshot::channel();
        let envelope = ExchangeEnvelope {
            exchange,
            reply_tx: Some(reply_tx_pipe),
        };

        if sender.send(envelope).await.is_err() {
            let _ = reply_tx.send(GrpcReply::Err(Status::internal("pipeline channel closed")));
            return;
        }

        // Intentionally discard intermediate replies — only the completion exchange's reply matters.
        let _ = reply_rx_pipe.await;
    }

    // Stream complete — send final Exchange with completion marker
    let mut completion_msg = Message::new(Body::Json(serde_json::Value::Null));
    for (k, v) in extract_metadata(&metadata) {
        completion_msg.set_header(k, v);
    }
    completion_msg.set_header(
        "CamelGrpcClientStreaming".to_string(),
        serde_json::Value::Bool(true),
    );
    completion_msg.set_header(
        "CamelGrpcClientStreamComplete".to_string(),
        serde_json::Value::Bool(true),
    );

    let completion_exchange = Exchange::new(completion_msg);
    let (reply_tx_pipe, reply_rx_pipe) = tokio::sync::oneshot::channel();
    let envelope = ExchangeEnvelope {
        exchange: completion_exchange,
        reply_tx: Some(reply_tx_pipe),
    };

    if sender.send(envelope).await.is_err() {
        let _ = reply_tx.send(GrpcReply::Err(Status::internal("pipeline channel closed")));
        return;
    }

    // The route's response to the completion Exchange becomes the gRPC response
    let result = match reply_rx_pipe.await {
        Ok(Ok(exchange)) => exchange,
        Ok(Err(e)) => {
            let _ = reply_tx.send(GrpcReply::Err(Status::internal(format!(
                "pipeline error: {e}"
            ))));
            return;
        }
        Err(_) => {
            let _ = reply_tx.send(GrpcReply::Err(Status::internal("pipeline reply dropped")));
            return;
        }
    };

    let resp_json = match result.input.body {
        Body::Json(v) => v,
        other => {
            let _ = reply_tx.send(GrpcReply::Err(Status::internal(format!(
                "expected JSON response body from pipeline, got {other:?}"
            ))));
            return;
        }
    };

    let encoded = match json_to_protobuf_bytes(resp_json, resp_desc) {
        Ok(b) => b,
        Err(e) => {
            let _ = reply_tx.send(GrpcReply::Err(Status::internal(format!(
                "failed to encode response: {e}",
            ))));
            return;
        }
    };

    let _ = reply_tx.send(GrpcReply::Ok(encoded));
}

// ── Bidi-streaming processor ───────────────────────────────────────────────

async fn process_bidi_request(
    mut body_rx: mpsc::Receiver<Vec<u8>>,
    metadata: tonic::metadata::MetadataMap,
    req_desc: MessageDescriptor,
    resp_desc: MessageDescriptor,
    sender: mpsc::Sender<ExchangeEnvelope>,
    reply_tx: mpsc::Sender<GrpcStreamItem>,
) {
    let observer = GrpcStreamObserver::new(reply_tx.clone(), resp_desc);
    let observer_id = next_observer_id();
    register_observer(observer_id.clone(), observer.clone());
    let _guard = ObserverGuard::new(observer_id.clone());

    // Spawn a task to forward messages from the client stream to the pipeline
    let sender_clone = sender.clone();
    let metadata_clone = metadata.clone();
    let req_desc_clone = req_desc.clone();

    let forward_task = tokio::spawn(async move {
        let mut sequence: u64 = 0;
        while let Some(body) = body_rx.recv().await {
            let req_dyn = match DynamicMessage::decode(req_desc_clone.clone(), body.as_slice()) {
                Ok(m) => m,
                Err(e) => {
                    let _ = observer
                        .on_error(Status::invalid_argument(format!(
                            "failed to decode protobuf: {e}"
                        )))
                        .await;
                    continue;
                }
            };

            let json = match serde_json::to_value(&req_dyn) {
                Ok(v) => v,
                Err(e) => {
                    let _ = observer
                        .on_error(Status::invalid_argument(format!(
                            "failed to convert protobuf to JSON: {e}"
                        )))
                        .await;
                    continue;
                }
            };

            let mut msg = Message::new(Body::Json(json));
            for (k, v) in extract_metadata(&metadata_clone) {
                msg.set_header(k, v);
            }

            msg.set_header(
                "CamelGrpcBidiSequence",
                serde_json::Value::Number(sequence.into()),
            );
            sequence += 1;

            let mut exchange = Exchange::new(msg);
            exchange.set_property(
                "CamelGrpcStreamObserverId",
                Value::String(observer_id.clone()),
            );

            let envelope = ExchangeEnvelope {
                exchange,
                reply_tx: None,
            };

            if sender_clone.send(envelope).await.is_err() {
                let _ = observer
                    .on_error(Status::internal("pipeline channel closed"))
                    .await;
                break;
            }
        }

        // Signal completion when client stream ends
        observer.on_completed().await;
    });

    // Wait for the forward task to complete
    let _ = forward_task.await;
}
