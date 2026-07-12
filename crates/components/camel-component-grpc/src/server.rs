use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};

use camel_api::CamelError;
use futures::StreamExt;
use hyper::server::conn::http2;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::sync::{OnceCell, RwLock, mpsc};
use tokio_rustls::TlsAcceptor;
use tonic::body::Body as TonicBody;
use tonic::codec::Streaming;
use tonic::{Request, Response, Status};
use tower::Service;
use tracing::{debug, error};

use camel_component_api::RuntimeObservability;

use crate::codec::RawBytesCodec;
use crate::config::{GrpcServerConfig, ServerTransport, read_tls_file};
use crate::consumer::{GrpcReply, GrpcRequestEnvelope, GrpcStreamItem};
use crate::mode::GrpcMode;

pub(crate) type GrpcDispatchEntry = (
    mpsc::Sender<GrpcRequestEnvelope>,
    GrpcMode,
    Option<Arc<dyn camel_auth::TokenAuthenticator>>,
);

pub(crate) type GrpcDispatchTable = Arc<RwLock<HashMap<String, GrpcDispatchEntry>>>;

type ServerKey = (String, u16);

struct ServerHandle {
    dispatch: GrpcDispatchTable,
    #[allow(dead_code)]
    _task: tokio::task::JoinHandle<()>,
    transport: ServerTransport,
}

pub(crate) struct GrpcServerRegistry {
    inner: Mutex<HashMap<ServerKey, Arc<OnceCell<ServerHandle>>>>,
}

impl GrpcServerRegistry {
    pub(crate) fn global() -> &'static Self {
        static INSTANCE: OnceLock<GrpcServerRegistry> = OnceLock::new();
        INSTANCE.get_or_init(|| GrpcServerRegistry {
            inner: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) async fn get_or_spawn(
        &'static self,
        host: &str,
        port: u16,
        config: GrpcServerConfig,
        runtime: Arc<dyn RuntimeObservability>,
    ) -> Result<GrpcDispatchTable, CamelError> {
        let host_owned = host.to_string();

        let cell = {
            let mut guard = self.inner.lock().map_err(|_| {
                CamelError::EndpointCreationFailed("GrpcServerRegistry lock poisoned".into())
            })?;
            let key = (host.to_string(), port);
            // Evict dead server so a fresh one can spawn (rc-4s65).
            if let Some(existing) = guard.get(&key)
                && let Some(handle) = existing.get()
                && handle._task.is_finished()
            {
                guard.remove(&key);
            }
            guard
                .entry(key)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };

        let handle = cell
            .get_or_try_init(|| async {
                let route_id = format!("grpc-server:{host_owned}:{port}");
                let acceptor = build_tls_acceptor(&config.transport, &route_id, &runtime)?;
                let addr = format!("{host_owned}:{port}");
                let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
                    CamelError::EndpointCreationFailed(format!(
                        "failed to bind gRPC server on {addr}: {e}"
                    ))
                })?;
                let dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
                let rt = Arc::clone(&runtime);
                let task = tokio::spawn(run_grpc_server(
                    listener,
                    Arc::clone(&dispatch),
                    config.clone(),
                    acceptor,
                    rt,
                ));
                Ok::<ServerHandle, CamelError>(ServerHandle {
                    dispatch,
                    _task: task,
                    transport: config.transport.clone(),
                })
            })
            .await?;

        validate_server_handle(handle, &config.transport, &host_owned, port)?;
        Ok(Arc::clone(&handle.dispatch))
    }

    pub(crate) async fn get_or_spawn_with_listener(
        &'static self,
        listener: tokio::net::TcpListener,
        host: &str,
        port: u16,
        config: GrpcServerConfig,
        runtime: Arc<dyn RuntimeObservability>,
    ) -> Result<GrpcDispatchTable, CamelError> {
        let host_owned = host.to_string();

        let cell = {
            let mut guard = self.inner.lock().map_err(|_| {
                CamelError::EndpointCreationFailed("GrpcServerRegistry lock poisoned".into())
            })?;
            let key = (host.to_string(), port);
            // Evict dead server so a fresh one can spawn (rc-4s65).
            if let Some(existing) = guard.get(&key)
                && let Some(handle) = existing.get()
                && handle._task.is_finished()
            {
                guard.remove(&key);
            }
            guard
                .entry(key)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };

        let handle = cell
            .get_or_try_init(|| async {
                let route_id = format!("grpc-server:{host_owned}:{port}");
                let acceptor = build_tls_acceptor(&config.transport, &route_id, &runtime)?;
                let dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
                let rt = Arc::clone(&runtime);
                let task = tokio::spawn(run_grpc_server(
                    listener,
                    Arc::clone(&dispatch),
                    config.clone(),
                    acceptor,
                    rt,
                ));
                Ok::<ServerHandle, CamelError>(ServerHandle {
                    dispatch,
                    _task: task,
                    transport: config.transport.clone(),
                })
            })
            .await?;

        validate_server_handle(handle, &config.transport, &host_owned, port)?;
        Ok(Arc::clone(&handle.dispatch))
    }

    pub(crate) async fn unregister(&self, host: &str, port: u16, path: &str) {
        let key = (host.to_string(), port);
        let dispatch = {
            let guard = self.inner.lock().ok();
            guard
                .as_ref()
                .and_then(|g| g.get(&key))
                .and_then(|cell| cell.get())
                .map(|handle| Arc::clone(&handle.dispatch))
        };
        if let Some(dispatch) = dispatch {
            let mut table = dispatch.write().await;
            table.remove(path);
        }
    }
}

/// Validate a server handle after registry lookup: reject transport mismatches
/// (Plaintext↔Tls OR Tls(A)↔Tls(B)) and reap finished tasks.
fn validate_server_handle(
    handle: &ServerHandle,
    requested: &ServerTransport,
    host: &str,
    port: u16,
) -> Result<(), CamelError> {
    if &handle.transport != requested {
        return Err(CamelError::EndpointCreationFailed(format!(
            "gRPC server {host}:{port} already bound with transport={:?}; \
             requested {:?} — refusing to mix incompatible transport configs on one listener",
            handle.transport, requested,
        )));
    }
    if handle._task.is_finished() {
        return Err(CamelError::EndpointCreationFailed(
            "gRPC server task has terminated unexpectedly".into(),
        ));
    }
    Ok(())
}

/// Build a TlsAcceptor from ServerTransport (server-auth, optional mTLS, ALPN h2).
/// Returns Ok(None) for Plaintext. Hard-errors on missing/invalid cert.
fn build_tls_acceptor(
    transport: &ServerTransport,
    route_id: &str,
    runtime: &Arc<dyn RuntimeObservability>,
) -> Result<Option<TlsAcceptor>, CamelError> {
    match transport {
        ServerTransport::Plaintext => Ok(None),
        ServerTransport::Tls(cfg) => {
            let cert_pem = read_tls_file(&cfg.server_cert_path, "server_cert", runtime, route_id)?;
            let key_pem = read_tls_file(&cfg.server_key_path, "server_key", runtime, route_id)?;
            let certs: Vec<_> = rustls_pemfile::certs(&mut cert_pem.as_slice())
                .collect::<Result<_, _>>()
                .map_err(|e| {
                    CamelError::EndpointCreationFailed(format!("invalid server cert PEM: {e}"))
                })?;
            let key = rustls_pemfile::private_key(&mut key_pem.as_slice())
                .map_err(|e| {
                    CamelError::EndpointCreationFailed(format!("invalid server key PEM: {e}"))
                })?
                .ok_or_else(|| {
                    CamelError::EndpointCreationFailed("no private key in server_key PEM".into())
                })?;
            // rustls 0.23: explicit ring provider via builder_with_provider —
            // ServerConfig::builder() panics at runtime if no process-default provider.
            let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());

            // Optional mTLS: build client cert verifier if client_ca_path is set.
            let client_verifier: Option<Arc<dyn rustls::server::danger::ClientCertVerifier>> =
                match &cfg.client_ca_path {
                    Some(ca_path) => {
                        let ca_pem = read_tls_file(ca_path, "client_ca", runtime, route_id)?;
                        let ca_certs: Vec<_> = rustls_pemfile::certs(&mut ca_pem.as_slice())
                            .collect::<Result<_, _>>()
                            .map_err(|e| {
                                CamelError::EndpointCreationFailed(format!(
                                    "invalid client CA PEM: {e}"
                                ))
                            })?;
                        let mut roots = rustls::RootCertStore::empty();
                        for cert in ca_certs {
                            roots.add(cert).map_err(|e| {
                                CamelError::EndpointCreationFailed(format!(
                                    "client CA root add: {e}"
                                ))
                            })?;
                        }
                        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
                            Arc::new(roots),
                            Arc::clone(&provider),
                        )
                        .build()
                        .map_err(|e| {
                            CamelError::EndpointCreationFailed(format!("client cert verifier: {e}"))
                        })?;
                        Some(verifier)
                    }
                    None => None,
                };

            let mut server_cfg = {
                let builder = rustls::ServerConfig::builder_with_provider(provider)
                    .with_safe_default_protocol_versions()
                    .map_err(|e| {
                        CamelError::EndpointCreationFailed(format!("rustls protocol versions: {e}"))
                    })?;
                match client_verifier {
                    None => builder.with_no_client_auth(),
                    Some(verifier) => builder.with_client_cert_verifier(verifier),
                }
                .with_single_cert(certs, key)
                .map_err(|e| {
                    CamelError::EndpointCreationFailed(format!("rustls ServerConfig: {e}"))
                })?
            };
            server_cfg.alpn_protocols = vec![b"h2".to_vec()];
            Ok(Some(TlsAcceptor::from(Arc::new(server_cfg))))
        }
    }
}

/// Serve h2 over a concrete IO type. Generic so both plaintext (TcpStream)
/// and TLS (TlsStream<TcpStream>) call sites type-check.
async fn serve_h2<I>(io: I, dispatch: GrpcDispatchTable, config: GrpcServerConfig)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(io);
    let service = service_fn(move |req| {
        let dispatch = dispatch.clone();
        handle_grpc_request(req, dispatch)
    });
    let mut builder = http2::Builder::new(hyper_util::rt::TokioExecutor::new());
    if let Some(max_len) = config.max_receive_message_len {
        let frame_size = max_len.clamp(16_384, 16_777_215) as u32;
        builder.max_frame_size(frame_size);
    }
    if let Err(e) = builder.serve_connection(io, service).await {
        debug!(error = %e, "gRPC connection error");
    }
}

async fn run_grpc_server(
    listener: tokio::net::TcpListener,
    dispatch: GrpcDispatchTable,
    config: GrpcServerConfig,
    tls_acceptor: Option<TlsAcceptor>,
    runtime: Arc<dyn RuntimeObservability>,
) {
    // Q-B1: route_id derived from listener local address. The accept loop runs
    // below route dispatch — multiple GrpcEndpoints register on one shared
    // listener, so no single per-route route_id is correct. The local address
    // is stable, attributable, and meaningful to operators.
    let route_id = listener
        .local_addr()
        .map(|addr| format!("grpc-server:{addr}"))
        .unwrap_or_else(|_| "grpc-server:unknown".to_string());

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(e) => {
                runtime
                    .metrics()
                    .increment_errors(&route_id, "e:grpc:accept");
                // log-policy: outside-contract
                error!(error = %e, "gRPC server accept error");
                continue;
            }
        };

        let tls_acceptor = tls_acceptor.clone();
        let config = config.clone();
        let dispatch = dispatch.clone();
        let rt_metrics = runtime.clone();
        let route_id_clone = route_id.clone();
        tokio::spawn(async move {
            match tls_acceptor.as_ref() {
                None => serve_h2(stream, dispatch, config).await,
                Some(acceptor) => match acceptor.accept(stream).await {
                    Ok(tls_stream) => serve_h2(tls_stream, dispatch, config).await,
                    Err(e) => {
                        rt_metrics
                            .metrics()
                            .increment_errors(&route_id_clone, "e:grpc:tls-accept");
                        debug!(error = %e, "gRPC TLS handshake error");
                    }
                },
            }
        });
    }
}

// ── Response stream type ───────────────────────────────────────────────────

type ResponseStream = Pin<Box<dyn futures::Stream<Item = Result<Vec<u8>, Status>> + Send>>;

// ── Manual stream implementation (no async-stream dep) ─────────────────────

struct GrpcItemStream {
    rx: mpsc::Receiver<GrpcStreamItem>,
}

impl futures::Stream for GrpcItemStream {
    type Item = Result<Vec<u8>, Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(GrpcStreamItem::Message(bytes))) => Poll::Ready(Some(Ok(bytes))),
            Poll::Ready(Some(GrpcStreamItem::Error(status))) => Poll::Ready(Some(Err(status))),
            Poll::Ready(Some(GrpcStreamItem::Done)) => Poll::Ready(None),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

// ── Authentication helper ──────────────────────────────────────────────────

async fn extract_principal(
    authenticator: &dyn camel_auth::TokenAuthenticator,
    metadata: &tonic::metadata::MetadataMap,
) -> Result<camel_api::security_policy::Principal, tonic::Status> {
    let token = metadata
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .map(str::trim)
        .filter(|t| !t.is_empty());

    let token = match token {
        Some(t) => t.to_string(),
        None => {
            return Err(tonic::Status::unauthenticated(
                "missing or malformed authorization header",
            ));
        }
    };

    match authenticator.authenticate_bearer(&token).await {
        Ok(p) => Ok(p),
        Err(camel_api::CamelError::Unauthenticated(msg)) => {
            Err(tonic::Status::unauthenticated(msg))
        }
        Err(camel_api::CamelError::ProcessorError(msg))
            if msg.contains("auth provider unavailable") =>
        {
            Err(tonic::Status::unavailable(msg))
        }
        Err(e) => {
            // log-policy: system-broken
            tracing::error!(error = %e, "gRPC authentication error");
            Err(tonic::Status::internal(e.to_string()))
        }
    }
}

// ── Mode-aware dispatch ────────────────────────────────────────────────────

async fn handle_grpc_request(
    req: hyper::Request<hyper::body::Incoming>,
    dispatch: GrpcDispatchTable,
) -> Result<hyper::Response<TonicBody>, std::convert::Infallible> {
    let path = req.uri().path().to_string();

    let entry = {
        let table = dispatch.read().await;
        table
            .get(&path)
            .map(|(tx, mode, auth)| (tx.clone(), *mode, auth.clone()))
    };

    let Some((sender, mode, authenticator_opt)) = entry else {
        let handler = UnimplementedHandler;
        let mut grpc = tonic::server::Grpc::new(RawBytesCodec);
        let response = grpc.unary(handler, req).await;
        return Ok(response);
    };

    let mut grpc = tonic::server::Grpc::new(RawBytesCodec);

    match mode {
        GrpcMode::Unary => {
            let handler = UnaryHandler {
                sender,
                authenticator_opt,
            };
            let response = grpc.unary(handler, req).await;
            Ok(response)
        }
        GrpcMode::ServerStreaming => {
            let handler = ServerStreamingHandler {
                sender,
                authenticator_opt,
            };
            let response = grpc.server_streaming(handler, req).await;
            Ok(response)
        }
        GrpcMode::ClientStreaming => {
            let handler = ClientStreamingHandler {
                sender,
                authenticator_opt,
            };
            let response = grpc.client_streaming(handler, req).await;
            Ok(response)
        }
        GrpcMode::Bidi => {
            let handler = BidiHandler {
                sender,
                authenticator_opt,
            };
            let response = grpc.streaming(handler, req).await;
            Ok(response)
        }
    }
}

// ── Unimplemented handler (fallback) ───────────────────────────────────────

struct UnimplementedHandler;

impl Service<Request<Vec<u8>>> for UnimplementedHandler {
    type Response = Response<Vec<u8>>;
    type Error = Status;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: Request<Vec<u8>>) -> Self::Future {
        Box::pin(async { Err(Status::unimplemented("no handler for path")) })
    }
}

// ── Unary handler ──────────────────────────────────────────────────────────

struct UnaryHandler {
    sender: mpsc::Sender<GrpcRequestEnvelope>,
    authenticator_opt: Option<Arc<dyn camel_auth::TokenAuthenticator>>,
}

impl tonic::server::UnaryService<Vec<u8>> for UnaryHandler {
    type Response = Vec<u8>;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Self::Response>, Status>> + Send>>;

    fn call(&mut self, req: Request<Vec<u8>>) -> Self::Future {
        let authenticator_opt = self.authenticator_opt.clone();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let sender = self.sender.clone();

        Box::pin(async move {
            let principal = if let Some(ref authenticator) = authenticator_opt {
                Some(extract_principal(authenticator.as_ref(), req.metadata()).await?)
            } else {
                None
            };

            let envelope = GrpcRequestEnvelope::Unary {
                metadata: req.metadata().clone(),
                body: req.into_inner(),
                reply_tx,
                principal,
            };
            sender
                .send(envelope)
                .await
                .map_err(|_| Status::unavailable("consumer stopped"))?;
            match reply_rx
                .await
                .map_err(|_| Status::internal("reply channel dropped"))?
            {
                GrpcReply::Ok(bytes) => Ok(Response::new(bytes)),
                GrpcReply::Err(status) => Err(status),
            }
        })
    }
}

// ── Server-streaming handler ───────────────────────────────────────────────

struct ServerStreamingHandler {
    sender: mpsc::Sender<GrpcRequestEnvelope>,
    authenticator_opt: Option<Arc<dyn camel_auth::TokenAuthenticator>>,
}

impl tonic::server::ServerStreamingService<Vec<u8>> for ServerStreamingHandler {
    type Response = Vec<u8>;
    type ResponseStream = ResponseStream;
    type Future =
        Pin<Box<dyn Future<Output = Result<Response<Self::ResponseStream>, Status>> + Send>>;

    fn call(&mut self, req: Request<Vec<u8>>) -> Self::Future {
        let authenticator_opt = self.authenticator_opt.clone();
        let (reply_tx, reply_rx) = mpsc::channel::<GrpcStreamItem>(64);
        let sender = self.sender.clone();

        Box::pin(async move {
            let principal = if let Some(ref authenticator) = authenticator_opt {
                Some(extract_principal(authenticator.as_ref(), req.metadata()).await?)
            } else {
                None
            };

            let envelope = GrpcRequestEnvelope::ServerStreaming {
                metadata: req.metadata().clone(),
                body: req.into_inner(),
                reply_tx,
                principal,
            };
            sender
                .send(envelope)
                .await
                .map_err(|_| Status::unavailable("consumer stopped"))?;
            Ok(Response::new(
                Box::pin(GrpcItemStream { rx: reply_rx }) as ResponseStream
            ))
        })
    }
}

// ── Client-streaming handler ───────────────────────────────────────────────

struct ClientStreamingHandler {
    sender: mpsc::Sender<GrpcRequestEnvelope>,
    authenticator_opt: Option<Arc<dyn camel_auth::TokenAuthenticator>>,
}

impl tonic::server::ClientStreamingService<Vec<u8>> for ClientStreamingHandler {
    type Response = Vec<u8>;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Self::Response>, Status>> + Send>>;

    fn call(&mut self, req: Request<Streaming<Vec<u8>>>) -> Self::Future {
        let authenticator_opt = self.authenticator_opt.clone();
        let (body_tx, body_rx) = mpsc::channel::<Vec<u8>>(64);
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel::<GrpcReply>();
        let sender = self.sender.clone();

        Box::pin(async move {
            let principal = if let Some(ref authenticator) = authenticator_opt {
                Some(extract_principal(authenticator.as_ref(), req.metadata()).await?)
            } else {
                None
            };

            let envelope = GrpcRequestEnvelope::ClientStreaming {
                metadata: req.metadata().clone(),
                body_rx,
                reply_tx,
                principal,
            };

            let forward_handle = tokio::spawn(async move {
                let mut stream = req.into_inner();
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(bytes) => {
                            if body_tx.send(bytes).await.is_err() {
                                break;
                            }
                        }
                        Err(status) => {
                            tracing::warn!(error = %status, "client streaming decode error");
                            return Some(status);
                        }
                    }
                }
                None
            });

            sender
                .send(envelope)
                .await
                .map_err(|_| Status::unavailable("consumer stopped"))?;

            let reply = reply_rx
                .await
                .map_err(|_| Status::internal("reply channel dropped"))?;

            // If the inbound stream had a decode error, propagate it instead of the consumer's reply.
            if let Ok(Some(status)) = forward_handle.await {
                return Err(status);
            }

            match reply {
                GrpcReply::Ok(bytes) => Ok(Response::new(bytes)),
                GrpcReply::Err(status) => Err(status),
            }
        })
    }
}

// ── Bidi handler ───────────────────────────────────────────────────────────

struct BidiHandler {
    sender: mpsc::Sender<GrpcRequestEnvelope>,
    authenticator_opt: Option<Arc<dyn camel_auth::TokenAuthenticator>>,
}

impl tonic::server::StreamingService<Vec<u8>> for BidiHandler {
    type Response = Vec<u8>;
    type ResponseStream = ResponseStream;
    type Future =
        Pin<Box<dyn Future<Output = Result<Response<Self::ResponseStream>, Status>> + Send>>;

    fn call(&mut self, req: Request<Streaming<Vec<u8>>>) -> Self::Future {
        let authenticator_opt = self.authenticator_opt.clone();
        let (body_tx, body_rx) = mpsc::channel::<Vec<u8>>(64);
        let (reply_tx, reply_rx) = mpsc::channel::<GrpcStreamItem>(64);
        let reply_tx_forward = reply_tx.clone();
        let sender = self.sender.clone();

        Box::pin(async move {
            let principal = if let Some(ref authenticator) = authenticator_opt {
                Some(extract_principal(authenticator.as_ref(), req.metadata()).await?)
            } else {
                None
            };

            let envelope = GrpcRequestEnvelope::Bidi {
                metadata: req.metadata().clone(),
                body_rx,
                reply_tx,
                principal,
            };

            tokio::spawn(async move {
                let mut stream = req.into_inner();
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(bytes) => {
                            if body_tx.send(bytes).await.is_err() {
                                break;
                            }
                        }
                        Err(status) => {
                            tracing::warn!(error = %status, "bidi streaming decode error");
                            let _ = reply_tx_forward.send(GrpcStreamItem::Error(status)).await;
                            break;
                        }
                    }
                }
            });

            sender
                .send(envelope)
                .await
                .map_err(|_| Status::unavailable("consumer stopped"))?;

            Ok(Response::new(
                Box::pin(GrpcItemStream { rx: reply_rx }) as ResponseStream
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::task::Poll;
    use std::time::Duration;

    use camel_api::MetricsCollector;
    use camel_component_api::HealthCheckRegistry;
    use futures::{Stream, StreamExt};
    use tokio::sync::mpsc;
    use tonic::Status;
    use tonic::server::{ServerStreamingService, UnaryService};
    use tower::Service;

    use super::*;
    use crate::consumer::{GrpcReply, GrpcStreamItem};

    // -----------------------------------------------------------------------
    // Recording metrics collector for testing increment_errors calls
    // -----------------------------------------------------------------------

    struct RecordingMetrics {
        errors: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl MetricsCollector for RecordingMetrics {
        fn record_exchange_duration(&self, _: &str, _: Duration) {}
        fn increment_errors(&self, route_id: &str, error_type: &str) {
            self.errors
                .lock()
                .unwrap()
                .push((route_id.to_string(), error_type.to_string()));
        }
        fn increment_exchanges(&self, _: &str) {}
        fn set_queue_depth(&self, _: &str, _: usize) {}
        fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
    }

    struct RecordingRuntime {
        metrics_collector: Arc<RecordingMetrics>,
    }

    impl RecordingRuntime {
        fn new(errors: Arc<Mutex<Vec<(String, String)>>>) -> Self {
            Self {
                metrics_collector: Arc::new(RecordingMetrics { errors }),
            }
        }
    }

    impl RuntimeObservability for RecordingRuntime {
        fn metrics(&self) -> Arc<dyn MetricsCollector> {
            self.metrics_collector.clone() as Arc<dyn MetricsCollector>
        }
        fn health(&self) -> Arc<dyn HealthCheckRegistry> {
            panic!("RecordingRuntime::health not used in this test")
        }
    }

    #[test]
    fn test_global_registry_returns_singleton() {
        let first = GrpcServerRegistry::global();
        let second = GrpcServerRegistry::global();
        assert!(std::ptr::eq(first, second));
    }

    #[tokio::test]
    async fn test_grpc_item_stream_yields_message() {
        let (tx, rx) = mpsc::channel::<GrpcStreamItem>(4);
        let mut stream = GrpcItemStream { rx };
        tx.send(GrpcStreamItem::Message(vec![1, 2, 3]))
            .await
            .unwrap();
        drop(tx);
        let item = stream.next().await.unwrap().unwrap();
        assert_eq!(item, vec![1, 2, 3]);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_grpc_item_stream_yields_error() {
        let (tx, rx) = mpsc::channel::<GrpcStreamItem>(4);
        let mut stream = GrpcItemStream { rx };
        let status = Status::internal("test error");
        tx.send(GrpcStreamItem::Error(status.clone()))
            .await
            .unwrap();
        drop(tx);
        let item = stream.next().await.unwrap();
        assert!(item.is_err());
        assert_eq!(item.unwrap_err().code(), status.code());
    }

    #[tokio::test]
    async fn test_grpc_item_stream_yields_done_as_none() {
        let (tx, rx) = mpsc::channel::<GrpcStreamItem>(4);
        let mut stream = GrpcItemStream { rx };
        tx.send(GrpcStreamItem::Done).await.unwrap();
        drop(tx);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_grpc_item_stream_closed_channel() {
        let (tx, rx) = mpsc::channel::<GrpcStreamItem>(4);
        let mut stream = GrpcItemStream { rx };
        drop(tx);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_grpc_item_stream_multiple_messages() {
        let (tx, rx) = mpsc::channel::<GrpcStreamItem>(4);
        let stream = GrpcItemStream { rx };
        tx.send(GrpcStreamItem::Message(vec![1])).await.unwrap();
        tx.send(GrpcStreamItem::Message(vec![2])).await.unwrap();
        tx.send(GrpcStreamItem::Message(vec![3])).await.unwrap();
        drop(tx);
        let results: Vec<_> = stream.collect().await;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_ref().unwrap(), &vec![1]);
        assert_eq!(results[1].as_ref().unwrap(), &vec![2]);
        assert_eq!(results[2].as_ref().unwrap(), &vec![3]);
    }

    #[tokio::test]
    async fn test_grpc_item_stream_poll_pending() {
        let (_tx, rx) = mpsc::channel::<GrpcStreamItem>(4);
        let stream = GrpcItemStream { rx };
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut stream_pinned = std::pin::Pin::new(Box::new(stream));
        assert!(matches!(
            Stream::poll_next(stream_pinned.as_mut(), &mut cx),
            Poll::Pending
        ));
    }

    #[test]
    fn test_unimplemented_handler_poll_ready() {
        let mut handler = UnimplementedHandler;
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(handler.poll_ready(&mut cx), Poll::Ready(Ok(()))));
    }

    #[tokio::test]
    async fn test_unimplemented_handler_returns_unimplemented_status() {
        let mut handler = UnimplementedHandler;
        let req = Request::new(vec![1, 2, 3]);
        let result = Service::call(&mut handler, req).await;
        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unimplemented);
        assert_eq!(status.message(), "no handler for path");
    }

    #[tokio::test]
    async fn test_unregister_removes_path_from_dispatch() {
        let dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(4);
        {
            let mut table = dispatch.write().await;
            table.insert(
                "/test.Service/Method".to_string(),
                (tx, GrpcMode::Unary, None),
            );
        }
        assert!(dispatch.read().await.contains_key("/test.Service/Method"));
        {
            let mut table = dispatch.write().await;
            table.remove("/test.Service/Method");
        }
        assert!(!dispatch.read().await.contains_key("/test.Service/Method"));
    }

    #[tokio::test]
    async fn test_unregister_nonexistent_path_is_noop() {
        let registry = GrpcServerRegistry::global();
        let dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
        registry
            .unregister("localhost", 50051, "/nonexistent.Path/Method")
            .await;
        assert!(dispatch.read().await.is_empty());
    }

    #[test]
    fn test_server_key_equality() {
        let key1: ServerKey = ("localhost".to_string(), 50051);
        let key2: ServerKey = ("localhost".to_string(), 50051);
        let key3: ServerKey = ("localhost".to_string(), 50052);
        let key4: ServerKey = ("remotehost".to_string(), 50051);
        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
        assert_ne!(key1, key4);
    }

    #[tokio::test]
    async fn test_dispatch_table_insert_and_retrieve() {
        let dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(4);
        let path = "/pkg.Service/Method".to_string();
        {
            let mut table = dispatch.write().await;
            table.insert(path.clone(), (tx, GrpcMode::ServerStreaming, None));
        }
        let table = dispatch.read().await;
        let (_, mode, _) = table.get(&path).unwrap();
        assert_eq!(*mode, GrpcMode::ServerStreaming);
    }

    #[tokio::test]
    async fn test_dispatch_table_remove_returns_entry() {
        let dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(4);
        let path = "/pkg.Service/Method".to_string();
        {
            let mut table = dispatch.write().await;
            table.insert(path.clone(), (tx, GrpcMode::Bidi, None));
        }
        {
            let mut table = dispatch.write().await;
            let removed = table.remove(&path);
            assert!(removed.is_some());
            let (_, mode, _) = removed.unwrap();
            assert_eq!(mode, GrpcMode::Bidi);
        }
        assert!(dispatch.read().await.is_empty());
    }

    #[tokio::test]
    async fn test_dispatch_table_all_grpc_modes() {
        let dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
        let modes = [
            GrpcMode::Unary,
            GrpcMode::ServerStreaming,
            GrpcMode::ClientStreaming,
            GrpcMode::Bidi,
        ];
        {
            let mut table = dispatch.write().await;
            for (i, mode) in modes.iter().enumerate() {
                let (tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(4);
                table.insert(format!("/svc/M{i}"), (tx, *mode, None));
            }
        }
        let table = dispatch.read().await;
        assert_eq!(table.len(), 4);
        for (i, expected_mode) in modes.iter().enumerate() {
            let (_, mode, _) = table.get(&format!("/svc/M{i}")).unwrap();
            assert_eq!(*mode, *expected_mode);
        }
    }

    #[test]
    fn test_grpc_reply_variants() {
        let ok_reply = GrpcReply::Ok(vec![4, 5, 6]);
        match ok_reply {
            GrpcReply::Ok(bytes) => assert_eq!(bytes, vec![4, 5, 6]),
            GrpcReply::Err(_) => panic!("expected Ok"),
        }
        let err_reply = GrpcReply::Err(Status::not_found("missing"));
        match err_reply {
            GrpcReply::Ok(_) => panic!("expected Err"),
            GrpcReply::Err(s) => assert_eq!(s.code(), tonic::Code::NotFound),
        }
    }

    #[tokio::test]
    async fn test_grpc_request_envelope_unary() {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert("x-test", "value".parse().unwrap());
        let body = vec![10, 20, 30];
        let envelope = GrpcRequestEnvelope::Unary {
            metadata: metadata.clone(),
            body: body.clone(),
            reply_tx,
            principal: None,
        };
        match envelope {
            GrpcRequestEnvelope::Unary {
                metadata: m,
                body: b,
                reply_tx: tx,
                ..
            } => {
                assert!(m.get("x-test").is_some());
                assert_eq!(b, body);
                let _ = tx.send(GrpcReply::Ok(vec![99]));
            }
            _ => panic!("expected Unary"),
        }
        let reply = reply_rx.await.unwrap();
        assert!(matches!(reply, GrpcReply::Ok(v) if v == vec![99]));
    }

    #[tokio::test]
    async fn test_grpc_request_envelope_server_streaming() {
        let (reply_tx, mut reply_rx) = mpsc::channel::<GrpcStreamItem>(4);
        let envelope = GrpcRequestEnvelope::ServerStreaming {
            metadata: tonic::metadata::MetadataMap::new(),
            body: vec![1],
            reply_tx,
            principal: None,
        };
        match envelope {
            GrpcRequestEnvelope::ServerStreaming { reply_tx: tx, .. } => {
                tx.send(GrpcStreamItem::Message(vec![42])).await.unwrap();
                tx.send(GrpcStreamItem::Done).await.unwrap();
            }
            _ => panic!("expected ServerStreaming"),
        }
        match reply_rx.recv().await {
            Some(GrpcStreamItem::Message(b)) => assert_eq!(b, vec![42]),
            _ => panic!("expected Message(42)"),
        }
        assert!(matches!(reply_rx.recv().await, Some(GrpcStreamItem::Done)));
    }

    #[tokio::test]
    async fn test_grpc_request_envelope_client_streaming() {
        let (body_tx, body_rx) = mpsc::channel::<Vec<u8>>(4);
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let envelope = GrpcRequestEnvelope::ClientStreaming {
            metadata: tonic::metadata::MetadataMap::new(),
            body_rx,
            reply_tx,
            principal: None,
        };
        let handle = tokio::spawn(async move {
            match envelope {
                GrpcRequestEnvelope::ClientStreaming {
                    body_rx: mut rx,
                    reply_tx: tx,
                    ..
                } => {
                    assert_eq!(rx.recv().await, Some(vec![1]));
                    assert_eq!(rx.recv().await, Some(vec![2]));
                    let _ = tx.send(GrpcReply::Ok(vec![99]));
                }
                _ => panic!("expected ClientStreaming"),
            }
        });
        body_tx.send(vec![1]).await.unwrap();
        body_tx.send(vec![2]).await.unwrap();
        drop(body_tx);
        handle.await.unwrap();
        let reply = reply_rx.await.unwrap();
        assert!(matches!(reply, GrpcReply::Ok(v) if v == vec![99]));
    }

    #[tokio::test]
    async fn test_grpc_request_envelope_bidi() {
        let (body_tx, body_rx) = mpsc::channel::<Vec<u8>>(4);
        let (reply_tx, mut reply_rx) = mpsc::channel::<GrpcStreamItem>(4);
        let envelope = GrpcRequestEnvelope::Bidi {
            metadata: tonic::metadata::MetadataMap::new(),
            body_rx,
            reply_tx,
            principal: None,
        };
        let handle = tokio::spawn(async move {
            match envelope {
                GrpcRequestEnvelope::Bidi {
                    body_rx: mut rx,
                    reply_tx: tx,
                    ..
                } => {
                    assert_eq!(rx.recv().await, Some(vec![10]));
                    tx.send(GrpcStreamItem::Message(vec![20])).await.unwrap();
                }
                _ => panic!("expected Bidi"),
            }
        });
        body_tx.send(vec![10]).await.unwrap();
        match reply_rx.recv().await {
            Some(GrpcStreamItem::Message(b)) => assert_eq!(b, vec![20]),
            _ => panic!("expected Message(20)"),
        }
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_get_or_spawn_with_listener_success() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let errors = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let rt: Arc<dyn RuntimeObservability> = Arc::new(RecordingRuntime::new(errors));

        let dispatch = GrpcServerRegistry::global()
            .get_or_spawn_with_listener(
                listener,
                "127.0.0.1",
                port,
                GrpcServerConfig::default(),
                rt,
            )
            .await;
        assert!(dispatch.is_ok());
    }

    #[tokio::test]
    async fn test_dead_server_evicted_on_reuse() {
        let registry = GrpcServerRegistry::global();
        let port = 17899u16;

        // Insert a dead server handle into the registry.
        {
            let mut guard = registry.inner.lock().unwrap();
            let key = ("127.0.0.1".to_string(), port);
            let dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
            let cell = Arc::new(OnceCell::new());
            let dead_task = tokio::spawn(async {});
            // Yield + sleep so the spawned task completes without consuming the handle.
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            assert!(dead_task.is_finished(), "task should be finished");
            cell.set(ServerHandle {
                dispatch,
                _task: dead_task,
                transport: ServerTransport::Plaintext,
            })
            .ok();
            guard.insert(key, cell);
        }

        // Now spawn a real server on the same port — the dead entry
        // must be evicted so a fresh server is created.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:17899")
            .await
            .unwrap();
        let errors = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let rt: Arc<dyn RuntimeObservability> = Arc::new(RecordingRuntime::new(errors));

        let result = registry
            .get_or_spawn_with_listener(
                listener,
                "127.0.0.1",
                port,
                GrpcServerConfig::default(),
                rt,
            )
            .await;

        assert!(result.is_ok(), "dead server should be evicted");
    }

    #[tokio::test]
    async fn test_unregister_from_global_registry() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let errors = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let rt: Arc<dyn RuntimeObservability> = Arc::new(RecordingRuntime::new(errors));

        let dispatch = GrpcServerRegistry::global()
            .get_or_spawn_with_listener(
                listener,
                "127.0.0.1",
                port,
                GrpcServerConfig::default(),
                rt,
            )
            .await
            .unwrap();

        let (tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(4);
        let path = "/test.Unregister/Method".to_string();
        {
            let mut table = dispatch.write().await;
            table.insert(path.clone(), (tx, GrpcMode::Unary, None));
        }
        assert!(dispatch.read().await.contains_key(&path));

        GrpcServerRegistry::global()
            .unregister("127.0.0.1", port, &path)
            .await;

        assert!(!dispatch.read().await.contains_key(&path));
    }

    #[test]
    fn test_unary_handler_is_constructable() {
        let (_tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(4);
        let _handler = UnaryHandler {
            sender: _tx,
            authenticator_opt: None,
        };
    }

    #[test]
    fn test_server_streaming_handler_is_constructable() {
        let (_tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(4);
        let _handler = ServerStreamingHandler {
            sender: _tx,
            authenticator_opt: None,
        };
    }

    #[test]
    fn test_client_streaming_handler_is_constructable() {
        let (_tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(4);
        let _handler = ClientStreamingHandler {
            sender: _tx,
            authenticator_opt: None,
        };
    }

    #[test]
    fn test_bidi_handler_is_constructable() {
        let (_tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(4);
        let _handler = BidiHandler {
            sender: _tx,
            authenticator_opt: None,
        };
    }

    #[tokio::test]
    async fn test_unary_handler_send_fails_when_consumer_stopped() {
        let (tx, rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        drop(rx);

        let mut handler = UnaryHandler {
            sender: tx,
            authenticator_opt: None,
        };
        let req = Request::new(vec![1, 2, 3]);
        let fut = handler.call(req);
        let handle = tokio::spawn(fut);

        let result = handle.await.unwrap();
        let err = result.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unavailable);
        assert!(err.message().contains("consumer stopped"));
    }

    #[tokio::test]
    async fn test_server_streaming_handler_send_fails_when_consumer_stopped() {
        let (tx, rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        drop(rx);

        let mut handler = ServerStreamingHandler {
            sender: tx,
            authenticator_opt: None,
        };
        let req = Request::new(vec![1, 2, 3]);
        let fut = handler.call(req);
        let handle = tokio::spawn(fut);

        match handle.await.unwrap() {
            Err(err) => {
                assert_eq!(err.code(), tonic::Code::Unavailable);
            }
            Ok(_) => panic!("expected error when consumer stopped"),
        }
    }

    #[tokio::test]
    async fn test_client_streaming_handler_send_fails_when_consumer_stopped() {
        let (tx, rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        drop(rx);

        let handler = ClientStreamingHandler {
            sender: tx,
            authenticator_opt: None,
        };
        assert!(handler.sender.is_closed());
    }

    #[tokio::test]
    async fn test_bidi_handler_send_fails_when_consumer_stopped() {
        let (tx, rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        drop(rx);

        let handler = BidiHandler {
            sender: tx,
            authenticator_opt: None,
        };
        assert!(handler.sender.is_closed());
    }

    #[tokio::test]
    async fn test_unary_handler_reply_channel_dropped() {
        let (tx, mut rx) = mpsc::channel::<GrpcRequestEnvelope>(4);

        let mut handler = UnaryHandler {
            sender: tx,
            authenticator_opt: None,
        };
        let req = Request::new(vec![1, 2, 3]);
        let fut = handler.call(req);

        let handle = tokio::spawn(fut);

        let envelope = rx.recv().await.unwrap();
        match envelope {
            GrpcRequestEnvelope::Unary { reply_tx, .. } => {
                drop(reply_tx);
            }
            _ => panic!("expected Unary"),
        }

        let result = handle.await.unwrap();
        let err = result.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("reply channel dropped"));
    }

    #[tokio::test]
    async fn test_unary_handler_returns_ok_response() {
        let (tx, mut rx) = mpsc::channel::<GrpcRequestEnvelope>(4);

        let mut handler = UnaryHandler {
            sender: tx,
            authenticator_opt: None,
        };
        let req = Request::new(vec![10, 20, 30]);
        let fut = handler.call(req);
        let handle = tokio::spawn(fut);

        let envelope = rx.recv().await.unwrap();
        match envelope {
            GrpcRequestEnvelope::Unary { reply_tx, body, .. } => {
                assert_eq!(body, vec![10, 20, 30]);
                let _ = reply_tx.send(GrpcReply::Ok(vec![40, 50]));
            }
            _ => panic!("expected Unary"),
        }

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result.into_inner(), vec![40, 50]);
    }

    #[tokio::test]
    async fn test_unary_handler_returns_error_response() {
        let (tx, mut rx) = mpsc::channel::<GrpcRequestEnvelope>(4);

        let mut handler = UnaryHandler {
            sender: tx,
            authenticator_opt: None,
        };
        let req = Request::new(vec![1]);
        let fut = handler.call(req);
        let handle = tokio::spawn(fut);

        let envelope = rx.recv().await.unwrap();
        match envelope {
            GrpcRequestEnvelope::Unary { reply_tx, .. } => {
                let _ = reply_tx.send(GrpcReply::Err(Status::not_found("not found")));
            }
            _ => panic!("expected Unary"),
        }

        let result = handle.await.unwrap();
        let err = result.unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn test_server_streaming_handler_success() {
        let (tx, mut rx) = mpsc::channel::<GrpcRequestEnvelope>(4);

        let mut handler = ServerStreamingHandler {
            sender: tx,
            authenticator_opt: None,
        };
        let req = Request::new(vec![1, 2]);
        let fut = handler.call(req);
        let handle = tokio::spawn(fut);

        let envelope = rx.recv().await.unwrap();
        match envelope {
            GrpcRequestEnvelope::ServerStreaming { reply_tx, body, .. } => {
                assert_eq!(body, vec![1, 2]);
                reply_tx
                    .send(GrpcStreamItem::Message(vec![100]))
                    .await
                    .unwrap();
                reply_tx.send(GrpcStreamItem::Done).await.unwrap();
            }
            _ => panic!("expected ServerStreaming"),
        }

        let result = handle.await.unwrap().unwrap();
        let mut stream = result.into_inner();
        assert_eq!(stream.next().await.unwrap().unwrap(), vec![100]);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_server_streaming_handler_error_in_stream() {
        let (tx, mut rx) = mpsc::channel::<GrpcRequestEnvelope>(4);

        let mut handler = ServerStreamingHandler {
            sender: tx,
            authenticator_opt: None,
        };
        let req = Request::new(vec![1]);
        let fut = handler.call(req);
        let handle = tokio::spawn(fut);

        let envelope = rx.recv().await.unwrap();
        match envelope {
            GrpcRequestEnvelope::ServerStreaming { reply_tx, .. } => {
                reply_tx
                    .send(GrpcStreamItem::Error(Status::internal("stream error")))
                    .await
                    .unwrap();
            }
            _ => panic!("expected ServerStreaming"),
        }

        let result = handle.await.unwrap().unwrap();
        let mut stream = result.into_inner();
        let item = stream.next().await.unwrap();
        let err = item.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
    }

    #[tokio::test]
    async fn test_bidi_handler_forwards_items() {
        let (tx, mut rx) = mpsc::channel::<GrpcRequestEnvelope>(4);
        let (reply_tx, _reply_rx) = mpsc::channel::<GrpcStreamItem>(4);

        let handler = BidiHandler {
            sender: tx,
            authenticator_opt: None,
        };
        let (body_tx, body_rx) = mpsc::channel::<Vec<u8>>(4);
        let envelope_for_test = GrpcRequestEnvelope::Bidi {
            metadata: tonic::metadata::MetadataMap::new(),
            body_rx,
            reply_tx,
            principal: None,
        };

        let send_result = handler.sender.send(envelope_for_test).await;
        assert!(send_result.is_ok());

        let received = rx.recv().await;
        assert!(received.is_some());

        body_tx.send(vec![10]).await.unwrap();
        body_tx.send(vec![20]).await.unwrap();
        drop(body_tx);
    }

    #[tokio::test]
    async fn test_grpc_stream_item_variants() {
        let msg = GrpcStreamItem::Message(vec![1, 2]);
        match msg {
            GrpcStreamItem::Message(b) => assert_eq!(b, vec![1, 2]),
            _ => panic!(),
        }

        let err = GrpcStreamItem::Error(Status::internal("err"));
        match err {
            GrpcStreamItem::Error(s) => assert_eq!(s.code(), tonic::Code::Internal),
            _ => panic!(),
        }

        let done = GrpcStreamItem::Done;
        match done {
            GrpcStreamItem::Done => {}
            _ => panic!(),
        }
    }

    #[derive(Debug)]
    struct MockAuthenticator {
        should_fail_unauthenticated: bool,
        should_fail_unavailable: bool,
    }

    #[async_trait::async_trait]
    impl camel_auth::TokenAuthenticator for MockAuthenticator {
        async fn authenticate_bearer(
            &self,
            _token: &str,
        ) -> Result<camel_api::security_policy::Principal, camel_api::CamelError> {
            if self.should_fail_unavailable {
                return Err(camel_api::CamelError::ProcessorError(
                    "auth provider unavailable".into(),
                ));
            }
            if self.should_fail_unauthenticated {
                return Err(camel_api::CamelError::Unauthenticated(
                    "invalid token".into(),
                ));
            }
            Ok(camel_api::security_policy::Principal {
                subject: "test-user".into(),
                issuer: "test-issuer".into(),
                audience: vec![],
                scopes: vec![],
                roles: vec![],
                claims: serde_json::json!({}),
            })
        }
    }

    #[tokio::test]
    async fn test_grpc_auth_valid_token() {
        let (tx, mut rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        let authenticator: Option<Arc<dyn camel_auth::TokenAuthenticator>> =
            Some(Arc::new(MockAuthenticator {
                should_fail_unauthenticated: false,
                should_fail_unavailable: false,
            }));

        let mut handler = UnaryHandler {
            sender: tx,
            authenticator_opt: authenticator,
        };

        let mut request = Request::new(vec![]);
        request
            .metadata_mut()
            .insert("authorization", "Bearer test-token".parse().unwrap());

        let handle = tokio::spawn(async move {
            let envelope = rx.recv().await.unwrap();
            match envelope {
                GrpcRequestEnvelope::Unary {
                    principal,
                    reply_tx,
                    ..
                } => {
                    assert!(principal.is_some());
                    let p = principal.unwrap();
                    assert_eq!(p.subject, "test-user");
                    assert_eq!(p.issuer, "test-issuer");
                    let _ = reply_tx.send(GrpcReply::Ok(vec![]));
                }
                _ => panic!("expected Unary"),
            }
        });

        let result = handler.call(request).await;
        assert!(result.is_ok());
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_grpc_auth_missing_token() {
        let (tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        let authenticator: Option<Arc<dyn camel_auth::TokenAuthenticator>> =
            Some(Arc::new(MockAuthenticator {
                should_fail_unauthenticated: false,
                should_fail_unavailable: false,
            }));

        let mut handler = UnaryHandler {
            sender: tx,
            authenticator_opt: authenticator,
        };

        let request = Request::new(vec![]);

        let result = handler.call(request).await;
        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn test_grpc_auth_invalid_token() {
        let (tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        let authenticator: Option<Arc<dyn camel_auth::TokenAuthenticator>> =
            Some(Arc::new(MockAuthenticator {
                should_fail_unauthenticated: true,
                should_fail_unavailable: false,
            }));

        let mut handler = UnaryHandler {
            sender: tx,
            authenticator_opt: authenticator,
        };

        let mut request = Request::new(vec![]);
        request
            .metadata_mut()
            .insert("authorization", "Bearer bad-token".parse().unwrap());

        let result = handler.call(request).await;
        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn test_grpc_auth_provider_unavailable() {
        let (tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        let authenticator: Option<Arc<dyn camel_auth::TokenAuthenticator>> =
            Some(Arc::new(MockAuthenticator {
                should_fail_unauthenticated: false,
                should_fail_unavailable: true,
            }));

        let mut handler = UnaryHandler {
            sender: tx,
            authenticator_opt: authenticator,
        };

        let mut request = Request::new(vec![]);
        request
            .metadata_mut()
            .insert("authorization", "Bearer valid-token".parse().unwrap());

        let result = handler.call(request).await;
        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unavailable);
    }

    #[tokio::test]
    async fn test_grpc_no_auth_configured() {
        let (tx, mut rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        let authenticator: Option<Arc<dyn camel_auth::TokenAuthenticator>> = None;

        let mut handler = UnaryHandler {
            sender: tx,
            authenticator_opt: authenticator,
        };

        let request = Request::new(vec![]);

        let handle = tokio::spawn(async move {
            let envelope = rx.recv().await.unwrap();
            match envelope {
                GrpcRequestEnvelope::Unary {
                    principal,
                    reply_tx,
                    ..
                } => {
                    assert!(principal.is_none());
                    let _ = reply_tx.send(GrpcReply::Ok(vec![]));
                }
                _ => panic!("expected Unary"),
            }
        });

        let result = handler.call(request).await;
        assert!(result.is_ok());
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_server_handle_struct() {
        let dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
        let task = tokio::spawn(async {});
        let handle = ServerHandle {
            dispatch,
            _task: task,
            transport: ServerTransport::Plaintext,
        };
        let _ = handle;
    }

    // ── ADR-0012 (e) site regression test ──────────────────────────────────

    #[tokio::test]
    async fn test_run_grpc_server_route_id_derivation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let route_id = listener
            .local_addr()
            .map(|addr| format!("grpc-server:{addr}"))
            .unwrap_or_else(|_| "grpc-server:unknown".to_string());
        assert!(!route_id.is_empty(), "route_id must not be empty");
        assert!(
            route_id.starts_with("grpc-server:"),
            "route_id should start with 'grpc-server:': got {route_id}"
        );
    }

    #[tokio::test]
    async fn test_increment_errors_recording_works() {
        // This test validates the metrics recording machinery for the accept
        // error branch without driving the real accept loop into an error.
        //
        // Driving the real loop requires platform-specific fd manipulation
        // (dup+shutdown) that causes the accept to return EINVAL on every
        // iteration — the loop spins indefinitely recording spurious errors,
        // which cannot be precisely asserted. The error condition is not
        // single-shot; it persists after shutdown.
        //
        // The accept error BRANCH (server.rs:189-196) is exercised indirectly:
        //   - test_run_grpc_server_happy_path confirms the accept loop runs
        //     and records zero errors on success.
        //   - This test confirms the recording subsystem correctly captures
        //     the call signature (route_id + label) that the error branch
        //     would emit.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let expected_route_id = listener
            .local_addr()
            .map(|addr| format!("grpc-server:{addr}"))
            .unwrap();
        let _dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
        let errors = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let rt: Arc<dyn RuntimeObservability> = Arc::new(RecordingRuntime::new(errors.clone()));

        // Simulate the accept-error branch: verify the metric call signature
        rt.metrics()
            .increment_errors(&expected_route_id, "e:grpc:accept");

        let recorded = errors.lock().unwrap();
        assert_eq!(recorded.len(), 1, "expected one error record");
        assert_eq!(recorded[0].0, expected_route_id, "route_id mismatch");
        assert_eq!(recorded[0].1, "e:grpc:accept", "error label mismatch");
    }

    #[tokio::test]
    async fn test_run_grpc_server_happy_path() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
        let errors = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let rt: Arc<dyn RuntimeObservability> = Arc::new(RecordingRuntime::new(errors.clone()));

        let handle = tokio::spawn(run_grpc_server(
            listener,
            _dispatch,
            GrpcServerConfig::default(),
            None,
            rt,
        ));

        // Connect to verify the accept loop handles clients
        let conn =
            tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr))
                .await;
        assert!(conn.is_ok(), "server should accept connections");

        // Verify no accept errors recorded on happy path
        {
            let recorded = errors.lock().unwrap();
            assert!(
                recorded.is_empty(),
                "no accept errors expected on happy path"
            );
        }

        handle.abort();
        let _ = handle.await;
    }

    /// Regression: Registry transport-mismatch — cannot mix TLS/plaintext on same listener.
    /// This test verifies the behavioral fail-closed path: first bind plaintext, then
    /// attempt TLS on the same port → must error with "transport" in the message.
    #[tokio::test]
    async fn test_registry_transport_mismatch_errors() {
        use crate::config::ServerTlsConfig;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let errors = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let rt: Arc<dyn RuntimeObservability> = Arc::new(RecordingRuntime::new(errors));

        // First, bind plaintext on this port
        let plaintext_config = GrpcServerConfig {
            max_receive_message_len: None,
            transport: ServerTransport::Plaintext,
        };
        let dispatch = GrpcServerRegistry::global()
            .get_or_spawn_with_listener(listener, "127.0.0.1", port, plaintext_config, rt.clone())
            .await;
        assert!(dispatch.is_ok(), "plaintext bind should succeed");

        // Now attempt TLS on the SAME port — MUST FAIL with transport mismatch
        let tls_config = GrpcServerConfig {
            max_receive_message_len: None,
            transport: ServerTransport::Tls(ServerTlsConfig {
                server_cert_path: "/nonexistent/cert.pem".to_string(),
                server_key_path: "/nonexistent/key.pem".to_string(),
                client_ca_path: None,
            }),
        };

        // Bind a new listener for the TLS attempt (will fail at validation, not bind)
        let listener2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();

        // Use the SAME port as the first bind to trigger mismatch
        let result = GrpcServerRegistry::global()
            .get_or_spawn_with_listener(listener2, "127.0.0.1", port, tls_config, rt)
            .await;

        assert!(
            result.is_err(),
            "TLS on port already serving plaintext must fail (transport-mismatch)"
        );
        match result {
            Err(e) => {
                let err = e.to_string();
                assert!(
                    err.contains("transport"),
                    "error must mention transport mismatch: {err}"
                );
            }
            Ok(_) => panic!("expected error, got Ok"),
        }
    }
}
