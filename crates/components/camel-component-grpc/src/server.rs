use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};

use camel_api::CamelError;
use hyper::server::conn::http2;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::sync::{OnceCell, RwLock, mpsc};
use tonic::{Request, Response, Status, body::Body as TonicBody};
use tower::Service;
use tracing::{debug, error};

use crate::codec::RawBytesCodec;
use crate::consumer::{GrpcReply, GrpcRequestEnvelope};

pub(crate) type GrpcDispatchTable =
    Arc<RwLock<HashMap<String, mpsc::Sender<GrpcRequestEnvelope>>>>;

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

async fn handle_grpc_request(
    req: hyper::Request<hyper::body::Incoming>,
    dispatch: GrpcDispatchTable,
) -> Result<hyper::Response<TonicBody>, std::convert::Infallible> {
    let path = req.uri().path().to_string();

    let sender = {
        let table = dispatch.read().await;
        table.get(&path).cloned()
    };

    let Some(sender) = sender else {
        let handler = UnimplementedHandler;
        let mut grpc = tonic::server::Grpc::new(RawBytesCodec);
        let response = grpc.unary(handler, req).await;
        return Ok(response);
    };

    let handler = GrpcHandler {
        sender,
        path,
    };
    let mut grpc = tonic::server::Grpc::new(RawBytesCodec);
    let response = grpc.unary(handler, req).await;
    Ok(response)
}

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

struct GrpcHandler {
    sender: mpsc::Sender<GrpcRequestEnvelope>,
    #[allow(dead_code)]
    path: String,
}

impl Service<Request<Vec<u8>>> for GrpcHandler {
    type Response = Response<Vec<u8>>;
    type Error = Status;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Vec<u8>>) -> Self::Future {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let envelope = GrpcRequestEnvelope {
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
