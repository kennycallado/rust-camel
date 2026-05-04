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
use tonic::body::Body as TonicBody;
use tonic::codec::Streaming;
use tonic::{Request, Response, Status};
use tower::Service;
use tracing::{debug, error};

use crate::codec::RawBytesCodec;
use crate::consumer::{GrpcReply, GrpcRequestEnvelope, GrpcStreamItem};
use crate::mode::GrpcMode;

pub(crate) type GrpcDispatchTable =
    Arc<RwLock<HashMap<String, (mpsc::Sender<GrpcRequestEnvelope>, GrpcMode)>>>;

type ServerKey = (String, u16);

struct ServerHandle {
    dispatch: GrpcDispatchTable,
    #[allow(dead_code)]
    _task: tokio::task::JoinHandle<()>,
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
    ) -> Result<GrpcDispatchTable, CamelError> {
        let host_owned = host.to_string();

        let cell = {
            let mut guard = self.inner.lock().map_err(|_| {
                CamelError::EndpointCreationFailed("GrpcServerRegistry lock poisoned".into())
            })?;
            let key = (host.to_string(), port);
            guard
                .entry(key)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };

        let handle = cell
            .get_or_try_init(|| async {
                let addr = format!("{host_owned}:{port}");
                let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
                    CamelError::EndpointCreationFailed(format!(
                        "failed to bind gRPC server on {addr}: {e}"
                    ))
                })?;
                let dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
                let task = tokio::spawn(run_grpc_server(listener, Arc::clone(&dispatch)));
                Ok::<ServerHandle, CamelError>(ServerHandle {
                    dispatch,
                    _task: task,
                })
            })
            .await?;

        if handle._task.is_finished() {
            return Err(CamelError::EndpointCreationFailed(
                "gRPC server task has terminated unexpectedly".into(),
            ));
        }

        Ok(Arc::clone(&handle.dispatch))
    }

    pub(crate) async fn get_or_spawn_with_listener(
        &'static self,
        listener: tokio::net::TcpListener,
        host: &str,
        port: u16,
    ) -> Result<GrpcDispatchTable, CamelError> {
        let cell = {
            let mut guard = self.inner.lock().map_err(|_| {
                CamelError::EndpointCreationFailed("GrpcServerRegistry lock poisoned".into())
            })?;
            let key = (host.to_string(), port);
            guard
                .entry(key)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };

        let handle = cell
            .get_or_try_init(|| async {
                let dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
                let task = tokio::spawn(run_grpc_server(listener, Arc::clone(&dispatch)));
                Ok::<ServerHandle, CamelError>(ServerHandle {
                    dispatch,
                    _task: task,
                })
            })
            .await?;

        if handle._task.is_finished() {
            return Err(CamelError::EndpointCreationFailed(
                "gRPC server task has terminated unexpectedly".into(),
            ));
        }

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

async fn run_grpc_server(listener: tokio::net::TcpListener, dispatch: GrpcDispatchTable) {
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(e) => {
                error!(error = %e, "gRPC server accept error");
                continue;
            }
        };

        let io = TokioIo::new(stream);
        let dispatch = dispatch.clone();

        tokio::spawn(async move {
            let service = service_fn(move |req| {
                let dispatch = dispatch.clone();
                handle_grpc_request(req, dispatch)
            });

            if let Err(e) = http2::Builder::new(hyper_util::rt::TokioExecutor::new())
                .serve_connection(io, service)
                .await
            {
                debug!(error = %e, "gRPC connection error");
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

// ── Mode-aware dispatch ────────────────────────────────────────────────────

async fn handle_grpc_request(
    req: hyper::Request<hyper::body::Incoming>,
    dispatch: GrpcDispatchTable,
) -> Result<hyper::Response<TonicBody>, std::convert::Infallible> {
    let path = req.uri().path().to_string();

    let entry = {
        let table = dispatch.read().await;
        table.get(&path).map(|(tx, mode)| (tx.clone(), *mode))
    };

    let Some((sender, mode)) = entry else {
        let handler = UnimplementedHandler;
        let mut grpc = tonic::server::Grpc::new(RawBytesCodec);
        let response = grpc.unary(handler, req).await;
        return Ok(response);
    };

    let mut grpc = tonic::server::Grpc::new(RawBytesCodec);

    match mode {
        GrpcMode::Unary => {
            let handler = UnaryHandler { sender };
            let response = grpc.unary(handler, req).await;
            Ok(response)
        }
        GrpcMode::ServerStreaming => {
            let handler = ServerStreamingHandler { sender };
            let response = grpc.server_streaming(handler, req).await;
            Ok(response)
        }
        GrpcMode::ClientStreaming => {
            let handler = ClientStreamingHandler { sender };
            let response = grpc.client_streaming(handler, req).await;
            Ok(response)
        }
        GrpcMode::Bidi => {
            let handler = BidiHandler { sender };
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
}

impl tonic::server::UnaryService<Vec<u8>> for UnaryHandler {
    type Response = Vec<u8>;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Self::Response>, Status>> + Send>>;

    fn call(&mut self, req: Request<Vec<u8>>) -> Self::Future {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let envelope = GrpcRequestEnvelope::Unary {
            metadata: req.metadata().clone(),
            body: req.into_inner(),
            reply_tx,
        };
        let sender = self.sender.clone();
        Box::pin(async move {
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
}

impl tonic::server::ServerStreamingService<Vec<u8>> for ServerStreamingHandler {
    type Response = Vec<u8>;
    type ResponseStream = ResponseStream;
    type Future =
        Pin<Box<dyn Future<Output = Result<Response<Self::ResponseStream>, Status>> + Send>>;

    fn call(&mut self, req: Request<Vec<u8>>) -> Self::Future {
        let (reply_tx, reply_rx) = mpsc::channel::<GrpcStreamItem>(64);
        let envelope = GrpcRequestEnvelope::ServerStreaming {
            metadata: req.metadata().clone(),
            body: req.into_inner(),
            reply_tx,
        };
        let sender = self.sender.clone();
        Box::pin(async move {
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
}

impl tonic::server::ClientStreamingService<Vec<u8>> for ClientStreamingHandler {
    type Response = Vec<u8>;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Self::Response>, Status>> + Send>>;

    fn call(&mut self, req: Request<Streaming<Vec<u8>>>) -> Self::Future {
        let (body_tx, body_rx) = mpsc::channel::<Vec<u8>>(64);
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel::<GrpcReply>();
        let envelope = GrpcRequestEnvelope::ClientStreaming {
            metadata: req.metadata().clone(),
            body_rx,
            reply_tx,
        };
        let sender = self.sender.clone();

        Box::pin(async move {
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
}

impl tonic::server::StreamingService<Vec<u8>> for BidiHandler {
    type Response = Vec<u8>;
    type ResponseStream = ResponseStream;
    type Future =
        Pin<Box<dyn Future<Output = Result<Response<Self::ResponseStream>, Status>> + Send>>;

    fn call(&mut self, req: Request<Streaming<Vec<u8>>>) -> Self::Future {
        let (body_tx, body_rx) = mpsc::channel::<Vec<u8>>(64);
        let (reply_tx, reply_rx) = mpsc::channel::<GrpcStreamItem>(64);
        let reply_tx_forward = reply_tx.clone();
        let envelope = GrpcRequestEnvelope::Bidi {
            metadata: req.metadata().clone(),
            body_rx,
            reply_tx,
        };
        let sender = self.sender.clone();

        Box::pin(async move {
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
    use std::task::Poll;

    use futures::{Stream, StreamExt};
    use tokio::sync::mpsc;
    use tonic::Status;
    use tower::Service;

    use super::*;
    use crate::consumer::{GrpcReply, GrpcStreamItem};

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
        let mut stream = GrpcItemStream { rx };
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(
            Pin::new(&mut stream).poll_next(&mut cx),
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
        let result = handler.call(req).await;
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
            table.insert("/test.Service/Method".to_string(), (tx, GrpcMode::Unary));
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
            table.insert(path.clone(), (tx, GrpcMode::ServerStreaming));
        }
        let table = dispatch.read().await;
        let (_, mode) = table.get(&path).unwrap();
        assert_eq!(*mode, GrpcMode::ServerStreaming);
    }

    #[tokio::test]
    async fn test_dispatch_table_remove_returns_entry() {
        let dispatch: GrpcDispatchTable = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(4);
        let path = "/pkg.Service/Method".to_string();
        {
            let mut table = dispatch.write().await;
            table.insert(path.clone(), (tx, GrpcMode::Bidi));
        }
        {
            let mut table = dispatch.write().await;
            let removed = table.remove(&path);
            assert!(removed.is_some());
            let (_, mode) = removed.unwrap();
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
                table.insert(format!("/svc/M{i}"), (tx, *mode));
            }
        }
        let table = dispatch.read().await;
        assert_eq!(table.len(), 4);
        for (i, expected_mode) in modes.iter().enumerate() {
            let (_, mode) = table.get(&format!("/svc/M{i}")).unwrap();
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
        };
        match envelope {
            GrpcRequestEnvelope::Unary {
                metadata: m,
                body: b,
                reply_tx: tx,
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
}
