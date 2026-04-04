pub mod config;

pub use config::{WsClientConfig, WsConfig, WsEndpointConfig, WsServerConfig};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::ws::{CloseCode, CloseFrame, Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequest, Request, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::{Router, serve};
use camel_api::{Body as CamelBody, BoxProcessor, CamelError, Exchange, Message as CamelMessage};
use camel_component::{
    Component, ConcurrencyModel, Consumer, ConsumerContext, Endpoint, ExchangeEnvelope,
    ProducerContext,
};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::{OnceCell, RwLock, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message as ClientWsMessage;
use tower::Service;

#[derive(Clone)]
struct WsPathConfig {
    max_connections: u32,
    max_message_size: u32,
    heartbeat_interval: std::time::Duration,
    idle_timeout: std::time::Duration,
    allow_origin: String,
}

impl Default for WsPathConfig {
    fn default() -> Self {
        let cfg = WsEndpointConfig::default();
        Self {
            max_connections: cfg.max_connections,
            max_message_size: cfg.max_message_size,
            heartbeat_interval: cfg.heartbeat_interval,
            idle_timeout: cfg.idle_timeout,
            allow_origin: cfg.allow_origin,
        }
    }
}

#[derive(Clone)]
struct WsTlsConfig {
    cert_path: String,
    key_path: String,
}

type DispatchTable = Arc<RwLock<HashMap<String, mpsc::Sender<ExchangeEnvelope>>>>;

struct ServerHandle {
    state: WsAppState,
    is_tls: bool,
    _task: JoinHandle<()>,
}

pub struct ServerRegistry {
    inner: Mutex<HashMap<u16, Arc<OnceCell<ServerHandle>>>>,
}

impl ServerRegistry {
    pub fn global() -> &'static Self {
        static REG: OnceLock<ServerRegistry> = OnceLock::new();
        REG.get_or_init(|| Self {
            inner: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) async fn get_or_spawn(
        &'static self,
        host: &str,
        port: u16,
        tls_config: Option<WsTlsConfig>,
    ) -> Result<WsAppState, CamelError> {
        let wants_tls = tls_config.is_some();
        let host_owned = host.to_string();

        let cell = {
            let mut guard = self.inner.lock().map_err(|_| {
                CamelError::EndpointCreationFailed("ServerRegistry lock poisoned".into())
            })?;
            guard
                .entry(port)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };

        let handle = cell
            .get_or_try_init(|| async { spawn_server(&host_owned, port, tls_config).await })
            .await?;

        if wants_tls != handle.is_tls {
            return Err(CamelError::EndpointCreationFailed(format!(
                "Server on port {port} already running with different TLS mode"
            )));
        }

        Ok(handle.state.clone())
    }
}

async fn spawn_server(
    host: &str,
    port: u16,
    tls_config: Option<WsTlsConfig>,
) -> Result<ServerHandle, CamelError> {
    let addr = format!("{host}:{port}");
    let dispatch: DispatchTable = Arc::new(RwLock::new(HashMap::new()));
    let path_configs = Arc::new(DashMap::new());
    let state = WsAppState {
        dispatch: Arc::clone(&dispatch),
        path_configs: Arc::clone(&path_configs),
    };
    let app = Router::new()
        .fallback(dispatch_handler)
        .with_state(state.clone());

    let (task, is_tls) = if let Some(ref tls) = tls_config {
        let rustls = load_tls_config(&tls.cert_path, &tls.key_path)?;
        let parsed_addr = addr.parse().map_err(|e| {
            CamelError::EndpointCreationFailed(format!("Invalid listen address {addr}: {e}"))
        })?;
        let tls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(rustls));
        let task = tokio::spawn(async move {
            let _ = axum_server::bind_rustls(parsed_addr, tls_cfg)
                .serve(app.into_make_service())
                .await;
        });
        (task, true)
    } else {
        let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
            CamelError::EndpointCreationFailed(format!("Failed to bind {addr}: {e}"))
        })?;
        let task = tokio::spawn(async move {
            let _ = serve(listener, app).await;
        });
        (task, false)
    };

    Ok(ServerHandle {
        state,
        is_tls,
        _task: task,
    })
}

#[derive(Clone)]
struct WsAppState {
    dispatch: DispatchTable,
    path_configs: Arc<DashMap<String, WsPathConfig>>,
}

pub struct WsConnectionRegistry {
    connections: DashMap<String, mpsc::Sender<WsMessage>>,
}

static GLOBAL_CONNECTION_REGISTRIES: OnceLock<
    DashMap<(String, u16, String), Arc<WsConnectionRegistry>>,
> = OnceLock::new();

fn global_registries() -> &'static DashMap<(String, u16, String), Arc<WsConnectionRegistry>> {
    GLOBAL_CONNECTION_REGISTRIES.get_or_init(DashMap::new)
}

impl Default for WsConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl WsConnectionRegistry {
    pub fn new() -> Self {
        Self {
            connections: DashMap::new(),
        }
    }

    pub fn insert(&self, key: String, tx: mpsc::Sender<WsMessage>) {
        self.connections.insert(key, tx);
    }

    pub fn remove(&self, key: &str) {
        self.connections.remove(key);
    }

    pub fn len(&self) -> usize {
        self.connections.len()
    }

    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    pub fn snapshot_senders(&self) -> Vec<mpsc::Sender<WsMessage>> {
        self.connections.iter().map(|e| e.value().clone()).collect()
    }

    pub fn get_senders_for_keys(&self, keys: &[String]) -> Vec<mpsc::Sender<WsMessage>> {
        keys.iter()
            .filter_map(|k| self.connections.get(k).map(|e| e.value().clone()))
            .collect()
    }
}

async fn dispatch_handler(
    State(state): State<WsAppState>,
    req: Request<Body>,
) -> impl IntoResponse {
    let path = req.uri().path().to_string();
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let remote_addr = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.to_string())
        .unwrap_or_default();
    let table = state.dispatch.read().await;
    if !table.contains_key(&path) {
        return (
            StatusCode::NOT_FOUND,
            "no ws endpoint registered for this path",
        )
            .into_response();
    }
    drop(table);

    let path_config = state
        .path_configs
        .get(&path)
        .map(|entry| entry.value().clone())
        .unwrap_or_default();
    if !is_origin_allowed(&path_config.allow_origin, origin.as_deref()) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }

    let upgrade_headers: HashMap<String, String> = req
        .headers()
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().to_lowercase(), v.to_str().ok()?.to_string())))
        .collect();

    let ws: WebSocketUpgrade = match WebSocketUpgrade::from_request(req, &()).await {
        Ok(ws) => ws,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "not a websocket request").into_response();
        }
    };

    ws.on_upgrade(move |socket| ws_handler(socket, state, path, remote_addr, upgrade_headers))
        .into_response()
}

#[allow(unused_variables)]
async fn ws_handler(
    socket: WebSocket,
    state: WsAppState,
    path: String,
    remote_addr: String,
    upgrade_headers: HashMap<String, String>,
) {
    let connection_key = uuid::Uuid::new_v4().to_string();
    let path_config = state
        .path_configs
        .get(&path)
        .map(|entry| entry.value().clone())
        .unwrap_or_default();

    let env_tx = {
        let table = state.dispatch.read().await;
        table.get(&path).cloned()
    };
    let Some(env_tx) = env_tx else {
        return;
    };

    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<WsMessage>(32);

    let registry = global_registries();
    let mut registry_key = None;
    for entry in registry.iter() {
        if entry.key().2 == path {
            entry.value().insert(connection_key.clone(), out_tx.clone());
            registry_key = Some(entry.key().clone());
            break;
        }
    }

    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let _ = sink.send(msg).await;
        }
    });

    let mut over_limit = false;
    if let Some(key) = &registry_key
        && let Some(entry) = registry.get(key)
        && entry.len() > path_config.max_connections as usize
    {
        over_limit = true;
    }
    if over_limit {
        try_send_with_backpressure(
            &out_tx,
            WsMessage::Close(Some(CloseFrame {
                code: CloseCode::from(1013u16),
                reason: "max connections exceeded".into(),
            })),
            "max-connections-close",
        );
        if let Some(key) = registry_key.clone()
            && let Some(entry) = registry.get(&key)
        {
            entry.remove(&connection_key);
        }
        drop(out_tx);
        let _ = writer.await;
        return;
    }

    let heartbeat_task = if path_config.heartbeat_interval > std::time::Duration::ZERO {
        let ping_tx = out_tx.clone();
        let interval = path_config.heartbeat_interval;
        Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let _ = try_send_with_backpressure(
                    &ping_tx,
                    WsMessage::Ping(Vec::new().into()),
                    "heartbeat-ping",
                );
            }
        }))
    } else {
        None
    };

    loop {
        let next_msg = if path_config.idle_timeout > std::time::Duration::ZERO {
            match tokio::time::timeout(path_config.idle_timeout, stream.next()).await {
                Ok(msg) => msg,
                Err(_) => {
                    try_send_with_backpressure(
                        &out_tx,
                        WsMessage::Close(Some(CloseFrame {
                            code: CloseCode::from(1000u16),
                            reason: "idle timeout".into(),
                        })),
                        "idle-timeout-close",
                    );
                    break;
                }
            }
        } else {
            stream.next().await
        };

        let Some(msg) = next_msg else {
            break;
        };

        match msg {
            Ok(WsMessage::Text(text)) => {
                if text.len() > path_config.max_message_size as usize {
                    try_send_with_backpressure(
                        &out_tx,
                        WsMessage::Close(Some(CloseFrame {
                            code: CloseCode::from(1009u16),
                            reason: "message too large".into(),
                        })),
                        "max-message-size-close-text",
                    );
                    break;
                }

                let mut message = CamelMessage::new(CamelBody::Text(text.to_string()));
                message.set_header(
                    "CamelWsConnectionKey",
                    serde_json::Value::String(connection_key.clone()),
                );
                message.set_header("CamelWsPath", serde_json::Value::String(path.clone()));
                message.set_header(
                    "CamelWsRemoteAddress",
                    serde_json::Value::String(remote_addr.clone()),
                );

                #[allow(unused_mut)]
                let mut exchange = Exchange::new(message);
                #[cfg(feature = "otel")]
                {
                    camel_otel::extract_into_exchange(&mut exchange, &upgrade_headers);
                }
                if env_tx
                    .send(ExchangeEnvelope {
                        exchange,
                        reply_tx: None,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(WsMessage::Binary(data)) => {
                if data.len() > path_config.max_message_size as usize {
                    try_send_with_backpressure(
                        &out_tx,
                        WsMessage::Close(Some(CloseFrame {
                            code: CloseCode::from(1009u16),
                            reason: "message too large".into(),
                        })),
                        "max-message-size-close-binary",
                    );
                    break;
                }

                let mut message = CamelMessage::new(CamelBody::Bytes(data));
                message.set_header(
                    "CamelWsConnectionKey",
                    serde_json::Value::String(connection_key.clone()),
                );
                message.set_header("CamelWsPath", serde_json::Value::String(path.clone()));
                message.set_header(
                    "CamelWsRemoteAddress",
                    serde_json::Value::String(remote_addr.clone()),
                );

                #[allow(unused_mut)]
                let mut exchange = Exchange::new(message);
                #[cfg(feature = "otel")]
                {
                    camel_otel::extract_into_exchange(&mut exchange, &upgrade_headers);
                }
                if env_tx
                    .send(ExchangeEnvelope {
                        exchange,
                        reply_tx: None,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(WsMessage::Close(_)) | Err(_) => break,
            _ => {}
        }
    }

    if let Some(task) = heartbeat_task {
        task.abort();
    }

    if let Some(key) = registry_key
        && let Some(entry) = registry.get(&key)
    {
        entry.remove(&connection_key);
    }
    drop(out_tx);
    let _ = writer.await;
}

pub struct WsComponent;

impl Component for WsComponent {
    fn scheme(&self) -> &str {
        "ws"
    }

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let cfg = WsEndpointConfig::from_uri(uri)?;
        Ok(Box::new(WsEndpoint {
            uri: uri.to_string(),
            cfg,
        }))
    }
}

pub struct WssComponent;

impl Component for WssComponent {
    fn scheme(&self) -> &str {
        "wss"
    }

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let cfg = WsEndpointConfig::from_uri(uri)?;
        Ok(Box::new(WsEndpoint {
            uri: uri.to_string(),
            cfg,
        }))
    }
}

struct WsEndpoint {
    uri: String,
    cfg: WsEndpointConfig,
}

impl Endpoint for WsEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(WsConsumer::new(self.cfg.server_config())))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(WsProducer::new(self.cfg.client_config())))
    }
}

pub struct WsConsumer {
    cfg: WsServerConfig,
    registry: Arc<WsConnectionRegistry>,
    server_state: Option<WsAppState>,
    registry_key: Option<(String, u16, String)>,
    forward_task: Option<JoinHandle<()>>,
}

impl WsConsumer {
    pub fn new(cfg: WsServerConfig) -> Self {
        Self {
            cfg,
            registry: Arc::new(WsConnectionRegistry::new()),
            server_state: None,
            registry_key: None,
            forward_task: None,
        }
    }
}

#[async_trait]
impl Consumer for WsConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        let tls_config = if self.cfg.inner.scheme == "wss" {
            let cert_path = self.cfg.inner.tls_cert.clone().ok_or_else(|| {
                CamelError::EndpointCreationFailed("TLS cert path is required for wss".into())
            })?;
            let key_path = self.cfg.inner.tls_key.clone().ok_or_else(|| {
                CamelError::EndpointCreationFailed("TLS key path is required for wss".into())
            })?;
            Some(WsTlsConfig {
                cert_path,
                key_path,
            })
        } else {
            None
        };

        let state = ServerRegistry::global()
            .get_or_spawn(&self.cfg.inner.host, self.cfg.inner.port, tls_config)
            .await?;

        let (env_tx, mut env_rx) = mpsc::channel::<ExchangeEnvelope>(64);
        {
            let mut table = state.dispatch.write().await;
            table.insert(self.cfg.inner.path.clone(), env_tx);
        }

        state.path_configs.insert(
            self.cfg.inner.path.clone(),
            WsPathConfig {
                max_connections: self.cfg.inner.max_connections,
                max_message_size: self.cfg.inner.max_message_size,
                heartbeat_interval: self.cfg.inner.heartbeat_interval,
                idle_timeout: self.cfg.inner.idle_timeout,
                allow_origin: self.cfg.inner.allow_origin.clone(),
            },
        );

        let registry_key = (
            self.cfg.inner.canonical_host(),
            self.cfg.inner.port,
            self.cfg.inner.path.clone(),
        );
        global_registries().insert(registry_key.clone(), Arc::clone(&self.registry));

        let sender = ctx.sender();
        let forward_task = tokio::spawn(async move {
            while let Some(envelope) = env_rx.recv().await {
                if sender.send(envelope).await.is_err() {
                    break;
                }
            }
        });

        self.server_state = Some(state);
        self.registry_key = Some(registry_key);
        self.forward_task = Some(forward_task);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        let close_msg = WsMessage::Close(Some(axum::extract::ws::CloseFrame {
            code: axum::extract::ws::CloseCode::from(1001u16),
            reason: "consumer stopping".into(),
        }));
        for tx in self.registry.snapshot_senders() {
            let _ = try_send_with_backpressure(&tx, close_msg.clone(), "consumer-stop-close");
        }

        if let Some(state) = self.server_state.take() {
            let mut table = state.dispatch.write().await;
            table.remove(&self.cfg.inner.path);
            state.path_configs.remove(&self.cfg.inner.path);
        }

        if let Some(key) = self.registry_key.take() {
            global_registries().remove(&key);
        }

        if let Some(task) = self.forward_task.take() {
            task.abort();
        }

        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Concurrent {
            max: Some(self.cfg.inner.max_connections as usize),
        }
    }
}

#[derive(Clone)]
pub struct WsProducer {
    cfg: WsClientConfig,
}

impl WsProducer {
    pub fn new(cfg: WsClientConfig) -> Self {
        Self { cfg }
    }
}

impl Service<Exchange> for WsProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let cfg = self.cfg.clone();

        Box::pin(async move {
            let canonical_host = cfg.inner.canonical_host();
            let key = (
                canonical_host.clone(),
                cfg.inner.port,
                cfg.inner.path.clone(),
            );

            let send_to_all = exchange
                .input
                .header("CamelWsSendToAll")
                .and_then(|v| v.as_bool())
                .or_else(|| exchange.input.header("sendToAll").and_then(|v| v.as_bool()))
                .unwrap_or(false);

            let conn_keys_header = exchange
                .input
                .header("CamelWsConnectionKey")
                .and_then(|v| v.as_str())
                .map(str::to_string);

            let local_exists = global_registries().contains_key(&key);
            let server_send_mode = send_to_all || conn_keys_header.is_some() || local_exists;

            let message_type = exchange
                .input
                .header("CamelWsMessageType")
                .and_then(|v| v.as_str())
                .unwrap_or("text")
                .to_ascii_lowercase();

            if server_send_mode {
                let registry = global_registries().get(&key).map(|e| Arc::clone(e.value()));
                let Some(registry) = registry else {
                    return Err(CamelError::ProcessorError(format!(
                        "WebSocket local consumer not found for {}:{}{}",
                        canonical_host, cfg.inner.port, cfg.inner.path
                    )));
                };

                let out_msg = body_to_axum_ws_message(
                    std::mem::take(&mut exchange.input.body),
                    &message_type,
                )
                .await?;

                let targets = if send_to_all {
                    registry.snapshot_senders()
                } else if let Some(keys) = conn_keys_header {
                    let parsed: Vec<String> = keys
                        .split(',')
                        .map(str::trim)
                        .filter(|k| !k.is_empty())
                        .map(str::to_string)
                        .collect();
                    registry.get_senders_for_keys(&parsed)
                } else {
                    registry.snapshot_senders()
                };

                for tx in targets {
                    let _ = try_send_with_backpressure(&tx, out_msg.clone(), "producer-send");
                }

                return Ok(exchange);
            }

            let url = format!(
                "{}://{}:{}{}",
                cfg.inner.scheme, cfg.inner.host, cfg.inner.port, cfg.inner.path
            );

            #[allow(unused_mut)]
            let mut request = url
                .clone()
                .into_client_request()
                .map_err(|e| CamelError::ProcessorError(format!("WebSocket request error: {e}")))?;

            #[cfg(feature = "otel")]
            {
                let mut otel_headers = HashMap::new();
                camel_otel::inject_from_exchange(&exchange, &mut otel_headers);
                for (k, v) in otel_headers {
                    if let (Ok(name), Ok(val)) = (
                        http::header::HeaderName::from_bytes(k.as_bytes()),
                        http::header::HeaderValue::from_str(&v),
                    ) {
                        request.headers_mut().insert(name, val);
                    }
                }
            }

            let connect_future = tokio_tungstenite::connect_async(request);
            let (mut ws_stream, _) =
                tokio::time::timeout(cfg.inner.connect_timeout, connect_future)
                    .await
                    .map_err(|_| {
                        CamelError::ProcessorError(format!(
                            "WebSocket connect timeout ({:?}) to {url}",
                            cfg.inner.connect_timeout
                        ))
                    })?
                    .map_err(|e| map_connect_error(e, &url))?;

            let out_msg =
                body_to_client_ws_message(std::mem::take(&mut exchange.input.body), &message_type)
                    .await?;

            ws_stream
                .send(out_msg)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("WebSocket send failed: {e}")))?;

            let incoming = tokio::time::timeout(cfg.inner.response_timeout, async {
                loop {
                    match ws_stream.next().await {
                        Some(Ok(ClientWsMessage::Ping(_))) | Some(Ok(ClientWsMessage::Pong(_))) => {
                            continue;
                        }
                        other => break other,
                    }
                }
            })
            .await
            .map_err(|_| CamelError::ProcessorError("WebSocket response timeout".into()))?;

            match incoming {
                Some(Ok(ClientWsMessage::Text(text))) => {
                    exchange.input.body = CamelBody::Text(text.to_string());
                }
                Some(Ok(ClientWsMessage::Binary(data))) => {
                    exchange.input.body = CamelBody::Bytes(data);
                }
                Some(Ok(ClientWsMessage::Close(frame))) => {
                    let normal = frame
                        .as_ref()
                        .map(|f| {
                            f.code == tungstenite::protocol::frame::coding::CloseCode::Normal
                                || f.code == tungstenite::protocol::frame::coding::CloseCode::Away
                        })
                        .unwrap_or(true);

                    if normal {
                        exchange.input.body = CamelBody::Empty;
                    } else {
                        let code = frame.map(|f| u16::from(f.code)).unwrap_or_default();
                        return Err(CamelError::ProcessorError(format!(
                            "WebSocket peer closed: code {code}"
                        )));
                    }
                }
                Some(Ok(_)) | None => {
                    exchange.input.body = CamelBody::Empty;
                }
                Some(Err(e)) => {
                    return Err(CamelError::ProcessorError(format!(
                        "WebSocket receive failed: {e}"
                    )));
                }
            }

            let _ = ws_stream.close(None).await;
            Ok(exchange)
        })
    }
}

async fn body_to_axum_ws_message(
    body: CamelBody,
    message_type: &str,
) -> Result<WsMessage, CamelError> {
    match message_type {
        "binary" => Ok(WsMessage::Binary(body.into_bytes(10 * 1024 * 1024).await?)),
        _ => Ok(WsMessage::Text(body_to_text(body).await?.into())),
    }
}

async fn body_to_client_ws_message(
    body: CamelBody,
    message_type: &str,
) -> Result<ClientWsMessage, CamelError> {
    match message_type {
        "binary" => Ok(ClientWsMessage::Binary(
            body.into_bytes(10 * 1024 * 1024).await?,
        )),
        _ => Ok(ClientWsMessage::Text(body_to_text(body).await?.into())),
    }
}

async fn body_to_text(body: CamelBody) -> Result<String, CamelError> {
    Ok(match body {
        CamelBody::Empty => String::new(),
        CamelBody::Text(s) => s,
        CamelBody::Xml(s) => s,
        CamelBody::Json(v) => v.to_string(),
        CamelBody::Bytes(b) => String::from_utf8_lossy(&b).to_string(),
        CamelBody::Stream(stream) => {
            let bytes = CamelBody::Stream(stream)
                .into_bytes(10 * 1024 * 1024)
                .await?;
            String::from_utf8_lossy(&bytes).to_string()
        }
    })
}

fn is_origin_allowed(allowed_origin: &str, request_origin: Option<&str>) -> bool {
    if allowed_origin == "*" {
        return true;
    }
    request_origin.is_some_and(|origin| origin == allowed_origin)
}

fn try_send_with_backpressure(tx: &mpsc::Sender<WsMessage>, msg: WsMessage, context: &str) -> bool {
    match tx.try_send(msg) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%context, %error, "dropping websocket outbound message due to backpressure");
            false
        }
    }
}

fn load_tls_config(
    cert_path: &str,
    key_path: &str,
) -> Result<tokio_rustls::rustls::ServerConfig, CamelError> {
    use std::fs::File;
    use std::io::BufReader;

    let cert_file = File::open(cert_path)
        .map_err(|e| CamelError::EndpointCreationFailed(format!("TLS cert file error: {e}")))?;
    let key_file = File::open(key_path)
        .map_err(|e| CamelError::EndpointCreationFailed(format!("TLS key file error: {e}")))?;

    let certs = rustls_pemfile::certs(&mut BufReader::new(cert_file))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CamelError::EndpointCreationFailed(format!("TLS cert parse error: {e}")))?;

    let key = rustls_pemfile::private_key(&mut BufReader::new(key_file))
        .map_err(|e| CamelError::EndpointCreationFailed(format!("TLS key parse error: {e}")))?
        .ok_or_else(|| CamelError::EndpointCreationFailed("TLS: no private key found".into()))?;

    tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| CamelError::EndpointCreationFailed(format!("TLS config error: {e}")))
}

fn map_connect_error(err: tungstenite::Error, url: &str) -> CamelError {
    match err {
        tungstenite::Error::Io(ioe) if ioe.kind() == std::io::ErrorKind::ConnectionRefused => {
            CamelError::ProcessorError(format!("WebSocket connection refused: {ioe}"))
        }
        tungstenite::Error::Tls(_) => {
            CamelError::ProcessorError("WebSocket TLS handshake failed: handshake error".into())
        }
        other => {
            let msg = other.to_string();
            if msg.to_lowercase().contains("connection refused") {
                CamelError::ProcessorError(format!("WebSocket connection refused: {msg}"))
            } else if msg.to_lowercase().contains("tls") {
                CamelError::ProcessorError(format!("WebSocket TLS handshake failed: {msg}"))
            } else {
                CamelError::ProcessorError(format!("WebSocket connection failed ({url}): {msg}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use tokio::sync::mpsc;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as ClientMessage;
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[test]
    fn ws_component_scheme_is_ws() {
        assert_eq!(WsComponent.scheme(), "ws");
    }

    #[test]
    fn wss_component_scheme_is_wss() {
        assert_eq!(WssComponent.scheme(), "wss");
    }

    #[test]
    fn endpoint_config_defaults_match_spec() {
        let cfg = WsEndpointConfig::default();
        assert_eq!(cfg.scheme, "ws");
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.path, "/");
        assert_eq!(cfg.max_connections, 100);
        assert_eq!(cfg.max_message_size, 65536);
        assert!(!cfg.send_to_all);
        assert_eq!(cfg.heartbeat_interval, Duration::ZERO);
        assert_eq!(cfg.idle_timeout, Duration::ZERO);
        assert_eq!(cfg.connect_timeout, Duration::from_secs(10));
        assert_eq!(cfg.response_timeout, Duration::from_secs(30));
        assert_eq!(cfg.allow_origin, "*");
        assert_eq!(cfg.tls_cert, None);
        assert_eq!(cfg.tls_key, None);
    }

    #[test]
    fn endpoint_config_parses_uri_params() {
        let uri = "ws://localhost:9001/chat?maxConnections=42&maxMessageSize=1024&sendToAll=true&heartbeatIntervalMs=1500&idleTimeoutMs=2500&connectTimeoutMs=3500&responseTimeoutMs=4500&allowOrigin=https://example.com&tlsCert=/tmp/cert.pem&tlsKey=/tmp/key.pem";
        let cfg = WsEndpointConfig::from_uri(uri).unwrap();

        assert_eq!(cfg.scheme, "ws");
        assert_eq!(cfg.host, "localhost");
        assert_eq!(cfg.port, 9001);
        assert_eq!(cfg.path, "/chat");
        assert_eq!(cfg.max_connections, 42);
        assert_eq!(cfg.max_message_size, 1024);
        assert!(cfg.send_to_all);
        assert_eq!(cfg.heartbeat_interval, Duration::from_millis(1500));
        assert_eq!(cfg.idle_timeout, Duration::from_millis(2500));
        assert_eq!(cfg.connect_timeout, Duration::from_millis(3500));
        assert_eq!(cfg.response_timeout, Duration::from_millis(4500));
        assert_eq!(cfg.allow_origin, "https://example.com");
        assert_eq!(cfg.tls_cert.as_deref(), Some("/tmp/cert.pem"));
        assert_eq!(cfg.tls_key.as_deref(), Some("/tmp/key.pem"));
    }

    #[test]
    fn endpoint_config_override_chain_uri_overrides_defaults() {
        let cfg = WsEndpointConfig::from_uri("ws://127.0.0.1:8089/echo?maxConnections=7").unwrap();
        assert_eq!(cfg.max_connections, 7);
        assert_eq!(cfg.max_message_size, 65536);
        assert!(!cfg.send_to_all);
        assert_eq!(cfg.response_timeout, Duration::from_secs(30));
    }

    #[test]
    fn endpoint_trait_creates_consumer_and_producer() {
        let endpoint = WsComponent
            .create_endpoint("ws://127.0.0.1:9010/trait")
            .unwrap();

        endpoint.create_consumer().unwrap();
        endpoint
            .create_producer(&ProducerContext::default())
            .unwrap();
    }

    #[test]
    fn ws_consumer_concurrency_model_uses_max_connections() {
        let cfg = WsEndpointConfig::from_uri("ws://127.0.0.1:9011/cm?maxConnections=321").unwrap();
        let consumer = WsConsumer::new(cfg.server_config());
        assert_eq!(
            consumer.concurrency_model(),
            ConcurrencyModel::Concurrent { max: Some(321) }
        );
    }

    #[tokio::test]
    async fn connection_registry_add_remove_broadcast_and_targeted_send() {
        let registry = WsConnectionRegistry::new();
        let (tx1, mut rx1) = mpsc::channel(8);
        let (tx2, mut rx2) = mpsc::channel(8);

        registry.insert("k1".into(), tx1);
        registry.insert("k2".into(), tx2);
        assert_eq!(registry.len(), 2);

        for tx in registry.snapshot_senders() {
            tx.send(WsMessage::Text("broadcast".into())).await.unwrap();
        }

        assert_eq!(rx1.recv().await, Some(WsMessage::Text("broadcast".into())));
        assert_eq!(rx2.recv().await, Some(WsMessage::Text("broadcast".into())));

        let target = registry.get_senders_for_keys(&["k1".to_string()]);
        assert_eq!(target.len(), 1);
        target[0]
            .send(WsMessage::Text("targeted".into()))
            .await
            .unwrap();

        assert_eq!(rx1.recv().await, Some(WsMessage::Text("targeted".into())));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), rx2.recv())
                .await
                .is_err()
        );

        registry.remove("k1");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn host_canonicalization_maps_local_hosts_to_loopback() {
        let c1 = WsEndpointConfig::from_uri("ws://0.0.0.0:9100/a")
            .unwrap()
            .canonical_host();
        let c2 = WsEndpointConfig::from_uri("ws://localhost:9101/b")
            .unwrap()
            .canonical_host();
        let c3 = WsEndpointConfig::from_uri("ws://127.0.0.1:9102/c")
            .unwrap()
            .canonical_host();

        assert_eq!(c1, "127.0.0.1");
        assert_eq!(c2, "127.0.0.1");
        assert_eq!(c3, "127.0.0.1");
    }

    #[tokio::test]
    async fn echo_flow_round_trips_message_through_consumer_and_producer() {
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/echo");
        let endpoint = WsComponent.create_endpoint(&uri).unwrap();

        let mut consumer = endpoint.create_consumer().unwrap();
        let producer = endpoint
            .create_producer(&ProducerContext::default())
            .unwrap();

        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let route_task = tokio::spawn(async move {
            if let Some(envelope) = route_rx.recv().await {
                let payload = envelope
                    .exchange
                    .input
                    .body
                    .as_text()
                    .unwrap_or_default()
                    .to_string();
                let key = envelope
                    .exchange
                    .input
                    .header("CamelWsConnectionKey")
                    .and_then(|v| v.as_str())
                    .unwrap()
                    .to_string();

                let mut response = Exchange::new(CamelMessage::new(CamelBody::Text(payload)));
                response
                    .input
                    .set_header("CamelWsConnectionKey", serde_json::Value::String(key));
                producer.oneshot(response).await.unwrap();
            }
        });

        let url = format!("ws://127.0.0.1:{port}/echo");
        let (mut client, _) = loop {
            match connect_async(&url).await {
                Ok(ok) => break ok,
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        };

        client
            .send(ClientMessage::Text("hello-ws".into()))
            .await
            .unwrap();

        let incoming = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match client.next().await {
                    Some(Ok(ClientMessage::Text(txt))) => break txt.to_string(),
                    Some(Ok(ClientMessage::Ping(_))) | Some(Ok(ClientMessage::Pong(_))) => continue,
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => panic!("ws receive failed: {e}"),
                    None => panic!("websocket closed before echo"),
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(incoming, "hello-ws");

        consumer.stop().await.unwrap();
        route_task.await.unwrap();
    }

    #[tokio::test]
    async fn consumer_stop_sends_close_1001() {
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/shutdown");
        let endpoint = WsComponent.create_endpoint(&uri).unwrap();

        let mut consumer = endpoint.create_consumer().unwrap();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let url = format!("ws://127.0.0.1:{port}/shutdown");
        let (mut client, _) = loop {
            match connect_async(&url).await {
                Ok(ok) => break ok,
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        };

        client
            .send(ClientMessage::Text("keepalive".into()))
            .await
            .unwrap();

        consumer.stop().await.unwrap();

        let close_code = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match client.next().await {
                    Some(Ok(ClientMessage::Close(frame))) => break frame.map(|f| f.code),
                    Some(Ok(ClientMessage::Ping(_))) | Some(Ok(ClientMessage::Pong(_))) => continue,
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => panic!("ws receive failed: {e}"),
                    None => panic!("websocket closed without close frame"),
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(close_code, Some(CloseCode::Away));
    }

    #[test]
    fn wildcard_origin_allows_anything() {
        assert!(is_origin_allowed("*", None));
        assert!(is_origin_allowed("*", Some("https://example.com")));
    }

    #[test]
    fn exact_origin_requires_match() {
        assert!(is_origin_allowed(
            "https://example.com",
            Some("https://example.com")
        ));
        assert!(!is_origin_allowed(
            "https://example.com",
            Some("https://other.com")
        ));
        assert!(!is_origin_allowed("https://example.com", None));
    }

    #[test]
    fn endpoint_config_rejects_invalid_scheme() {
        let result = WsEndpointConfig::from_uri("http://localhost:9000/path");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Invalid WebSocket scheme"),
            "expected scheme error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn wss_consumer_start_fails_without_tls_cert() {
        let port = free_port();
        let endpoint = WssComponent
            .create_endpoint(&format!("wss://127.0.0.1:{port}/secure"))
            .unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();
        let (tx, _rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new());
        let result = consumer.start(ctx).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("TLS cert path is required"),
            "expected TLS cert error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn wss_consumer_start_fails_with_nonexistent_cert() {
        let port = free_port();
        let endpoint = WssComponent
            .create_endpoint(&format!(
                "wss://127.0.0.1:{port}/secure?tlsCert=/nonexistent/cert.pem&tlsKey=/nonexistent/key.pem"
            ))
            .unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();
        let (tx, _rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new());
        let result = consumer.start(ctx).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("TLS cert file error"),
            "expected cert file error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn server_registry_returns_same_state_for_same_port() {
        let port = free_port();
        let state1 = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None)
            .await
            .unwrap();
        let state2 = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None)
            .await
            .unwrap();
        assert!(
            Arc::ptr_eq(&state1.dispatch, &state2.dispatch),
            "expected same dispatch table for same port"
        );
    }

    #[tokio::test]
    async fn dispatch_handler_returns_404_for_unregistered_path() {
        let port = free_port();
        let state = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None)
            .await
            .unwrap();
        let app = Router::new().fallback(dispatch_handler).with_state(state);
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            tower::ServiceExt::oneshot(
                app,
                axum::http::Request::builder()
                    .uri("/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn client_mode_producer_connects_and_echoes() {
        let port = free_port();

        let app = Router::new().route(
            "/echo",
            axum::routing::get(|ws: WebSocketUpgrade| async move {
                ws.on_upgrade(|mut socket: WebSocket| async move {
                    while let Some(Ok(msg)) = socket.recv().await {
                        match msg {
                            WsMessage::Text(text) => {
                                let _ = socket.send(WsMessage::Text(text)).await;
                            }
                            WsMessage::Binary(data) => {
                                let _ = socket.send(WsMessage::Binary(data)).await;
                            }
                            WsMessage::Close(_) => break,
                            _ => {}
                        }
                    }
                })
            }),
        );
        let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        let server_task = tokio::spawn(async move {
            let _ = serve(listener, app).await;
        });

        let cfg = WsEndpointConfig::from_uri(&format!("ws://127.0.0.1:{port}/echo")).unwrap();
        let producer = WsProducer::new(cfg.client_config());

        let exchange = Exchange::new(CamelMessage::new(CamelBody::Text("hello-client".into())));
        let result = loop {
            tokio::time::sleep(Duration::from_millis(25)).await;
            match tokio::time::timeout(Duration::from_secs(3), producer.oneshot(exchange)).await {
                Ok(Ok(r)) => break r,
                Ok(Err(_)) => panic!("producer call failed"),
                Err(_) => panic!("producer call timed out"),
            }
        };

        assert_eq!(result.input.body.as_text().unwrap(), "hello-client");

        server_task.abort();
    }

    #[tokio::test]
    async fn max_connections_rejects_with_close_1013() {
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/limited?maxConnections=1");
        let endpoint = WsComponent.create_endpoint(&uri).unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let url = format!("ws://127.0.0.1:{port}/limited");
        let (_client1, _) = loop {
            match connect_async(&url).await {
                Ok(ok) => break ok,
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        };

        tokio::time::sleep(Duration::from_millis(100)).await;

        let (mut client2, _) = connect_async(&url).await.unwrap();

        let close_code = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match client2.next().await {
                    Some(Ok(ClientMessage::Close(frame))) => break frame.map(|f| f.code),
                    Some(Ok(ClientMessage::Ping(_))) | Some(Ok(ClientMessage::Pong(_))) => continue,
                    Some(Ok(ClientMessage::Text(_))) => continue,
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => panic!("client2 ws receive failed: {e}"),
                    None => panic!("client2 closed without close frame"),
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(
            close_code,
            Some(CloseCode::from(1013u16)),
            "expected 1013 (Try Again Later) for max connections"
        );

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn max_message_size_rejects_with_close_1009() {
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/sizelimit?maxMessageSize=10");
        let endpoint = WsComponent.create_endpoint(&uri).unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let url = format!("ws://127.0.0.1:{port}/sizelimit");
        let (mut client, _) = loop {
            match connect_async(&url).await {
                Ok(ok) => break ok,
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        };

        let oversized = "x".repeat(100);
        client
            .send(ClientMessage::Text(oversized.into()))
            .await
            .unwrap();

        let close_code = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match client.next().await {
                    Some(Ok(ClientMessage::Close(frame))) => break frame.map(|f| f.code),
                    Some(Ok(ClientMessage::Ping(_))) | Some(Ok(ClientMessage::Pong(_))) => continue,
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => panic!("ws receive failed: {e}"),
                    None => panic!("websocket closed without close frame"),
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(
            close_code,
            Some(CloseCode::from(1009u16)),
            "expected 1009 (Message Too Big) for oversized message"
        );

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn origin_rejection_returns_403() {
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/origintest?allowOrigin=https://allowed.com");
        let endpoint = WsComponent.create_endpoint(&uri).unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let state = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None)
            .await
            .unwrap();
        let app = Router::new().fallback(dispatch_handler).with_state(state);

        let response = tokio::time::timeout(
            Duration::from_secs(2),
            tower::ServiceExt::oneshot(
                app,
                axum::http::Request::builder()
                    .uri("/origintest")
                    .header("origin", "https://evil.com")
                    .header("upgrade", "websocket")
                    .header("connection", "Upgrade")
                    .header("sec-websocket-version", "13")
                    .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                    .body(Body::empty())
                    .unwrap(),
            ),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "expected 403 for disallowed origin"
        );

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn broadcast_sends_to_all_connected_clients() {
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/bc");
        let endpoint = WsComponent.create_endpoint(&uri).unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();
        let producer = endpoint
            .create_producer(&ProducerContext::default())
            .unwrap();

        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(route_tx, CancellationToken::new());
        consumer.start(ctx).await.unwrap();

        let url = format!("ws://127.0.0.1:{port}/bc");

        let (mut client1, _) = loop {
            match connect_async(&url).await {
                Ok(ok) => break ok,
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        };

        let (mut client2, _) = connect_async(&url).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut response =
            Exchange::new(CamelMessage::new(CamelBody::Text("broadcast-msg".into())));
        response
            .input
            .set_header("CamelWsSendToAll", serde_json::Value::Bool(true));
        producer.oneshot(response).await.unwrap();

        let recv1 = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match client1.next().await {
                    Some(Ok(ClientMessage::Text(txt))) => break txt.to_string(),
                    Some(Ok(ClientMessage::Ping(_))) | Some(Ok(ClientMessage::Pong(_))) => continue,
                    _ => panic!("client1 unexpected message or close"),
                }
            }
        })
        .await
        .unwrap();

        let recv2 = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match client2.next().await {
                    Some(Ok(ClientMessage::Text(txt))) => break txt.to_string(),
                    Some(Ok(ClientMessage::Ping(_))) | Some(Ok(ClientMessage::Pong(_))) => continue,
                    _ => panic!("client2 unexpected message or close"),
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(recv1, "broadcast-msg");
        assert_eq!(recv2, "broadcast-msg");

        consumer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_get_or_spawn_returns_same_state() {
        let port = free_port();
        let results: Arc<std::sync::Mutex<Vec<WsAppState>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let results = results.clone();
            handles.push(tokio::spawn(async move {
                let state = ServerRegistry::global()
                    .get_or_spawn("127.0.0.1", port, None)
                    .await
                    .unwrap();
                results.lock().unwrap().push(state);
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let states = results.lock().unwrap();
        assert_eq!(states.len(), 4);
        for i in 1..states.len() {
            assert!(
                Arc::ptr_eq(&states[0].dispatch, &states[i].dispatch),
                "all concurrent callers should get the same dispatch table"
            );
        }
    }
}
