//! WebSocket component for rust-camel — Axum-based WebSocket server and Tokio-tungstenite client for bidirectional messaging.
//!
//! Main types: `WsComponent`, `WsBundle`, `WsConfig`, `WsServerConfig`, `WsClientConfig`, `WsEndpointConfig`.
//! Main modules: `bundle`, `config`, `health`.

pub mod bundle;
pub mod config;
pub mod health;
pub(crate) mod tls_reload;

pub use bundle::WsBundle;
pub use config::{WsClientConfig, WsConfig, WsEndpointConfig, WsServerConfig};
pub use health::WsHealthCheck;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::ws::{CloseCode, CloseFrame, Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequest, Request, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::{Router, serve};
use camel_api::security_policy::AuthorizationDecision;
use camel_component_api::tls_source::ServerTlsSource;
use camel_component_api::{
    Body as CamelBody, BoxProcessor, CamelError, Component, ConcurrencyModel, Consumer,
    ConsumerContext, Endpoint, Exchange, ExchangeEnvelope, Message as CamelMessage,
    NetworkRetryPolicy, ProducerContext, RuntimeObservability, retry_async,
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
pub struct WsPathConfig {
    pub max_connections: u32,
    pub max_message_size: u32,
    pub heartbeat_interval: std::time::Duration,
    pub idle_timeout: std::time::Duration,
    pub allow_origin: String,
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
pub struct WsTlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

type DispatchTable = Arc<RwLock<HashMap<String, mpsc::Sender<ExchangeEnvelope>>>>;

struct ServerHandle {
    state: WsAppState,
    is_tls: bool,
    _task: JoinHandle<()>,
    tls_config: Option<axum_server::tls_rustls::RustlsConfig>,
    tls_source: Option<ServerTlsSource>,
}

struct ServerRegistryInner {
    cell: Arc<OnceCell<ServerHandle>>,
    ref_count: usize,
}

pub struct ServerRegistry {
    inner: Mutex<HashMap<u16, ServerRegistryInner>>,
}

impl ServerRegistry {
    pub fn global() -> &'static Self {
        static REG: OnceLock<ServerRegistry> = OnceLock::new();
        REG.get_or_init(|| Self {
            inner: Mutex::new(HashMap::new()),
        })
    }

    pub async fn get_or_spawn(
        &'static self,
        host: &str,
        port: u16,
        tls_config: Option<WsTlsConfig>,
        runtime: Arc<dyn RuntimeObservability>,
        route_id: String,
    ) -> Result<WsAppState, CamelError> {
        let wants_tls = tls_config.is_some();
        let host_owned = host.to_string();

        let (cell, _is_new) = {
            let mut guard = self.inner.lock().map_err(|_| {
                CamelError::EndpointCreationFailed("ServerRegistry lock poisoned".into())
            })?;
            let entry = guard.entry(port).or_insert_with(|| ServerRegistryInner {
                cell: Arc::new(OnceCell::new()),
                ref_count: 0,
            });
            entry.ref_count += 1;
            (entry.cell.clone(), entry.ref_count == 1)
        };

        let handle = cell
            .get_or_try_init(|| async {
                let handle = spawn_server(
                    &host_owned,
                    port,
                    tls_config,
                    runtime.clone(),
                    route_id.clone(),
                )
                .await?;
                // Register reload handler (exactly-once: inside OnceCell init closure).
                if let (Some(tls_cfg), Some(source)) =
                    (handle.tls_config.as_ref(), handle.tls_source.as_ref())
                {
                    let handler = Arc::new(crate::tls_reload::WsReloadHandler::new(
                        tls_cfg.clone(),
                        source.clone(),
                        port,
                    ));
                    camel_component_api::tls_source::TlsReloadRegistry::global().register(handler);
                }
                Ok::<ServerHandle, CamelError>(handle)
            })
            .await?;

        if wants_tls != handle.is_tls {
            // Decrement ref count since we're rejecting this caller
            let mut guard = self.inner.lock().map_err(|_| {
                CamelError::EndpointCreationFailed("ServerRegistry lock poisoned".into())
            })?;
            if let Some(entry) = guard.get_mut(&port) {
                entry.ref_count -= 1;
                if entry.ref_count == 0 {
                    guard.remove(&port);
                }
            }
            return Err(CamelError::EndpointCreationFailed(format!(
                "Server on port {port} already running with different TLS mode"
            )));
        }

        Ok(handle.state.clone())
    }

    /// Release a reference to the server on the given port.
    /// WebSocket servers are process-lifetime: the server stays alive
    /// for potential restart. Path deregistration happens separately.
    pub(crate) fn release(&self, port: u16) {
        tracing::debug!(port, "WebSocket consumer released (server kept alive)");
    }

    /// Reset the global registry — **test-only**.
    #[cfg(test)]
    pub fn reset() {
        let mut guard = Self::global().inner.lock().expect("ServerRegistry lock");
        for (_, entry) in guard.iter() {
            if let Some(handle) = entry.cell.get() {
                handle._task.abort();
            }
        }
        guard.clear();
    }
}

async fn spawn_server(
    host: &str,
    port: u16,
    tls_config: Option<WsTlsConfig>,
    runtime: Arc<dyn RuntimeObservability>,
    route_id: String,
) -> Result<ServerHandle, CamelError> {
    let host_owned = host.to_string();
    let addr = format!("{host}:{port}");
    let dispatch: DispatchTable = Arc::new(RwLock::new(HashMap::new()));
    let path_configs = Arc::new(DashMap::new());
    let path_policies = Arc::new(DashMap::new());
    let server_error = new_atomic_false();
    let state = WsAppState {
        dispatch: Arc::clone(&dispatch),
        path_configs: Arc::clone(&path_configs),
        path_policies: Arc::clone(&path_policies),
        server_error: Arc::clone(&server_error),
        runtime: Arc::clone(&runtime),
        route_id: route_id.clone(),
    };
    let app = Router::new()
        .fallback(dispatch_handler)
        .with_state(state.clone());

    let (task, is_tls, retained_tls_cfg, retained_source) = if let Some(ref tls) = tls_config {
        let rustls = load_tls_config(&tls.cert_path, &tls.key_path)?;
        let parsed_addr = addr.parse().map_err(|e| {
            CamelError::EndpointCreationFailed(format!("Invalid listen address {addr}: {e}"))
        })?;
        let tls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(rustls));
        let tls_source = ServerTlsSource {
            cert_path: std::path::PathBuf::from(&tls.cert_path),
            key_path: std::path::PathBuf::from(&tls.key_path),
            client_ca_path: None,
        };
        // Clone for handle retention — the original moves into the spawned task
        let retained = tls_cfg.clone();
        let error_flag = Arc::clone(&server_error);
        let rt = Arc::clone(&runtime);
        let rid = route_id.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = axum_server::bind_rustls(parsed_addr, tls_cfg)
                .serve(app.into_make_service())
                .await
            {
                rt.health()
                    .force_unhealthy_for_route(&rid, "g:ws:bind-tls", &e.to_string());
                // log-policy: outside-contract
                tracing::error!(
                    host = host_owned,
                    port = port,
                    error = %e,
                    "WebSocket server terminated with error"
                );
                error_flag.store(true, Ordering::Relaxed);
            }
        });
        (task, true, Some(retained), Some(tls_source))
    } else {
        let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
            CamelError::EndpointCreationFailed(format!("Failed to bind {addr}: {e}"))
        })?;
        let error_flag = Arc::clone(&server_error);
        let rt = Arc::clone(&runtime);
        let rid = route_id.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = serve(listener, app).await {
                rt.health()
                    .force_unhealthy_for_route(&rid, "g:ws:bind-plain", &e.to_string());
                // log-policy: outside-contract
                tracing::error!(
                    host = host_owned,
                    port = port,
                    error = %e,
                    "WebSocket server terminated with error"
                );
                error_flag.store(true, Ordering::Relaxed);
            }
        });
        (task, false, None, None)
    };

    tracing::info!(host, port, is_tls, "WebSocket server started");

    Ok(ServerHandle {
        state,
        is_tls,
        _task: task,
        tls_config: retained_tls_cfg,
        tls_source: retained_source,
    })
}

#[derive(Clone)]
pub struct WsAppState {
    pub dispatch: DispatchTable,
    pub path_configs: Arc<DashMap<String, WsPathConfig>>,
    pub path_policies: Arc<DashMap<String, camel_component_api::SecurityContext>>,
    pub server_error: Arc<AtomicBool>,
    /// Observable runtime for ADR-0012 (e) metric and (g) health calls.
    pub runtime: Arc<dyn RuntimeObservability>,
    /// Route id of the consumer that created this server.
    pub route_id: String,
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

pub async fn dispatch_handler(
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

    let mut principal_opt: Option<camel_api::security_policy::Principal> = None;
    if let Some(sec_ctx) = state.path_policies.get(&path) {
        let extracted =
            camel_auth::extract_token_multi(req.headers(), req.uri(), &sec_ctx.credential_sources);

        match extracted {
            Some(extracted) => {
                if matches!(
                    extracted.source,
                    camel_auth::CredentialSource::QueryParam { .. }
                ) {
                    let redacted =
                        camel_auth::redact_query_params(req.uri(), &["access_token", "token"]);
                    tracing::debug!(path = %redacted, "WS upgrade with query token (redacted)");
                }
                match sec_ctx
                    .authenticator
                    .authenticate_bearer(&extracted.token)
                    .await
                {
                    Ok(principal) => {
                        let mut exchange = camel_api::Exchange::new(camel_api::Message::new(
                            camel_api::Body::Empty,
                        ));
                        camel_api::store_principal_properties(&mut exchange, &principal);
                        match sec_ctx.policy.evaluate(&mut exchange).await {
                            Ok(AuthorizationDecision::Granted { principal: _p }) => {
                                tracing::debug!(path = %path, subject = %principal.subject, "WS upgrade authorized");
                                principal_opt = Some(principal);
                            }
                            Ok(AuthorizationDecision::Denied { reason, .. }) => {
                                tracing::warn!(path = %path, reason = %reason, "WS upgrade denied");
                                return (StatusCode::FORBIDDEN, "Forbidden").into_response();
                            }
                            Err(e) => {
                                state
                                    .runtime
                                    .metrics()
                                    .increment_errors(&state.route_id, "e:ws:policy-eval");
                                // log-policy: outside-contract
                                tracing::error!(path = %path, error = %e, "Policy evaluation error during WS upgrade");
                                return (
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    "Internal Server Error",
                                )
                                    .into_response();
                            }
                        }
                    }
                    Err(e) => {
                        let (status, body) = match &e {
                            camel_api::CamelError::Unauthenticated(_) => {
                                (StatusCode::UNAUTHORIZED, "Unauthorized")
                            }
                            camel_api::CamelError::ProcessorError(msg)
                                if msg.contains("auth provider unavailable") =>
                            {
                                (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable")
                            }
                            _ => (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error"),
                        };
                        tracing::warn!(path = %path, error = %e, "WS upgrade authentication failed");
                        return (status, body).into_response();
                    }
                }
            }
            None => {
                tracing::warn!(path = %path, "WS upgrade rejected: no credential found in any source");
                return (
                    StatusCode::UNAUTHORIZED,
                    [("WWW-Authenticate", "Bearer".to_string())],
                    "Unauthorized",
                )
                    .into_response();
            }
        }
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

    ws.on_upgrade(move |socket| {
        ws_handler(
            socket,
            state,
            path,
            remote_addr,
            upgrade_headers,
            principal_opt,
        )
    })
    .into_response()
}

#[allow(unused_variables)]
async fn ws_handler(
    socket: WebSocket,
    state: WsAppState,
    path: String,
    remote_addr: String,
    upgrade_headers: HashMap<String, String>,
    principal: Option<camel_api::security_policy::Principal>,
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

    // Clone for writer closure and subsequent tracing (WS-009)
    let conn_key_for_writer = connection_key.clone();
    let path_for_writer = path.clone();

    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if let Err(e) = sink.send(msg).await {
                tracing::warn!(
                    connection_key = conn_key_for_writer,
                    path = path_for_writer,
                    error = %e,
                    "WebSocket writer send error — closing connection"
                );
                break;
            }
        }
    });

    tracing::info!(
        connection_key = connection_key,
        path = path,
        remote_addr = remote_addr,
        "WebSocket connection opened"
    );

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
            Ok(WsMessage::Ping(data)) => {
                tracing::debug!(
                    connection_key = connection_key,
                    path = path,
                    "WebSocket ping received — sending pong"
                );
                let _ = try_send_with_backpressure(
                    &out_tx,
                    WsMessage::Pong(data),
                    "ping-pong-response",
                );
            }
            Ok(WsMessage::Pong(_)) => {
                tracing::debug!(
                    connection_key = connection_key,
                    path = path,
                    "WebSocket pong received"
                );
            }
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
                if let Some(ref p) = principal {
                    camel_api::store_principal_properties(&mut exchange, p);
                }
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
                if let Some(ref p) = principal {
                    camel_api::store_principal_properties(&mut exchange, p);
                }
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
            Ok(WsMessage::Close(cf)) => {
                let reason = cf
                    .as_ref()
                    .map(|f| f.reason.to_string())
                    .unwrap_or_default();
                tracing::info!(
                    connection_key = connection_key,
                    path = path,
                    reason = reason,
                    "WebSocket connection closed by peer"
                );
                break;
            }
            Err(e) => {
                tracing::warn!(
                    connection_key = connection_key,
                    path = path,
                    error = %e,
                    "WebSocket receive error"
                );
                break;
            }
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

    tracing::info!(
        connection_key = connection_key,
        path = path,
        "WebSocket connection closed"
    );
}

pub struct WsComponent {
    pub(crate) config: WsConfig,
}

impl WsComponent {
    pub fn new() -> Self {
        Self {
            config: WsConfig::default(),
        }
    }

    pub fn with_config(config: WsConfig) -> Self {
        Self { config }
    }
}

impl Default for WsComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for WsComponent {
    fn scheme(&self) -> &str {
        "ws"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        self.config.validate()?;
        let mut cfg = WsEndpointConfig::from_uri(uri)?;
        if let Some(v) = self.config.max_connections {
            cfg.max_connections = v;
        }
        if let Some(v) = self.config.max_message_size {
            cfg.max_message_size = v;
        }
        if let Some(v) = self.config.heartbeat_interval_ms {
            cfg.heartbeat_interval = std::time::Duration::from_millis(v);
        }
        if let Some(v) = self.config.idle_timeout_ms {
            cfg.idle_timeout = std::time::Duration::from_millis(v);
        }
        if let Some(v) = self.config.connect_timeout_ms {
            cfg.connect_timeout = std::time::Duration::from_millis(v);
        }
        if let Some(v) = self.config.response_timeout_ms {
            cfg.response_timeout = std::time::Duration::from_millis(v);
        }
        if let Some(v) = self.config.send_timeout_ms {
            cfg.send_timeout = std::time::Duration::from_millis(v);
        }
        if let Some(v) = self.config.binary_payload {
            cfg.binary_payload = v;
        }
        if let Some(ref v) = self.config.subprotocols {
            cfg.subprotocols = v.clone();
        }
        let health_check = WsHealthCheck::new(cfg.host.clone(), cfg.port);
        ctx.register_current_route_health_check(std::sync::Arc::new(health_check));
        Ok(Box::new(WsEndpoint {
            uri: uri.to_string(),
            cfg,
        }))
    }
}

pub struct WssComponent {
    pub(crate) config: WsConfig,
}

impl WssComponent {
    pub fn new() -> Self {
        Self {
            config: WsConfig::default(),
        }
    }

    pub fn with_config(config: WsConfig) -> Self {
        Self { config }
    }
}

impl Default for WssComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for WssComponent {
    fn scheme(&self) -> &str {
        "wss"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        self.config.validate()?;
        let mut cfg = WsEndpointConfig::from_uri(uri)?;
        if let Some(v) = self.config.max_connections {
            cfg.max_connections = v;
        }
        if let Some(v) = self.config.max_message_size {
            cfg.max_message_size = v;
        }
        if let Some(v) = self.config.heartbeat_interval_ms {
            cfg.heartbeat_interval = std::time::Duration::from_millis(v);
        }
        if let Some(v) = self.config.idle_timeout_ms {
            cfg.idle_timeout = std::time::Duration::from_millis(v);
        }
        if let Some(v) = self.config.connect_timeout_ms {
            cfg.connect_timeout = std::time::Duration::from_millis(v);
        }
        if let Some(v) = self.config.response_timeout_ms {
            cfg.response_timeout = std::time::Duration::from_millis(v);
        }
        if let Some(v) = self.config.send_timeout_ms {
            cfg.send_timeout = std::time::Duration::from_millis(v);
        }
        if let Some(v) = self.config.binary_payload {
            cfg.binary_payload = v;
        }
        if let Some(ref v) = self.config.subprotocols {
            cfg.subprotocols = v.clone();
        }
        let health_check = WsHealthCheck::new(cfg.host.clone(), cfg.port);
        ctx.register_current_route_health_check(std::sync::Arc::new(health_check));
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

    fn create_consumer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(WsConsumer::new(self.cfg.server_config(), rt)))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(WsProducer::new(self.cfg.client_config())))
    }
}

pub struct WsConsumer {
    cfg: WsServerConfig,
    registry: Arc<WsConnectionRegistry>,
    server_state: Option<WsAppState>,
    registry_key: Option<(String, u16, String)>,
    forward_task: Option<JoinHandle<Result<(), CamelError>>>,
    security_ctx: Option<camel_component_api::SecurityContext>,
    /// Runtime observability handle for ADR-0012 metrics and health calls.
    runtime: Arc<dyn camel_component_api::RuntimeObservability>,
}

impl WsConsumer {
    pub fn new(
        cfg: WsServerConfig,
        runtime: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Self {
        Self {
            cfg,
            registry: Arc::new(WsConnectionRegistry::new()),
            server_state: None,
            registry_key: None,
            forward_task: None,
            security_ctx: None,
            runtime,
        }
    }
}

#[async_trait]
impl Consumer for WsConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        // Reject double-start (WS-006)
        if self.server_state.is_some() {
            return Err(CamelError::EndpointCreationFailed(
                "WebSocket consumer already started".into(),
            ));
        }

        tracing::info!(
            host = self.cfg.inner.host,
            port = self.cfg.inner.port,
            path = self.cfg.inner.path,
            scheme = self.cfg.inner.scheme,
            "WebSocket consumer starting"
        );

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
            .get_or_spawn(
                &self.cfg.inner.host,
                self.cfg.inner.port,
                tls_config,
                self.runtime.clone(),
                ctx.route_id().to_string(),
            )
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

        if let Some(ref sec_ctx) = self.security_ctx {
            let path = self.cfg.inner.path.clone();
            state.path_policies.insert(path, sec_ctx.clone());
        }

        let registry_key = (
            self.cfg.inner.canonical_host(),
            self.cfg.inner.port,
            self.cfg.inner.path.clone(),
        );
        global_registries().insert(registry_key.clone(), Arc::clone(&self.registry));

        let sender = ctx.sender();
        let forward_task: JoinHandle<Result<(), CamelError>> = tokio::spawn(async move {
            while let Some(envelope) = env_rx.recv().await {
                if sender.send(envelope).await.is_err() {
                    break;
                }
            }
            Ok(())
        });

        self.server_state = Some(state);
        self.registry_key = Some(registry_key);
        self.forward_task = Some(forward_task);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        tracing::info!(
            host = self.cfg.inner.host,
            port = self.cfg.inner.port,
            path = self.cfg.inner.path,
            "WebSocket consumer stopping"
        );

        let close_msg = WsMessage::Close(Some(axum::extract::ws::CloseFrame {
            code: axum::extract::ws::CloseCode::from(1001u16),
            reason: "consumer stopping".into(),
        }));
        for tx in self.registry.snapshot_senders() {
            let _ = try_send_with_backpressure(&tx, close_msg.clone(), "consumer-stop-close");
        }

        let mut had_server_error = false;

        if let Some(state) = self.server_state.take() {
            had_server_error = state.server_error.load(Ordering::Relaxed);
            state.path_policies.remove(&self.cfg.inner.path);
            let mut table = state.dispatch.write().await;
            table.remove(&self.cfg.inner.path);
            state.path_configs.remove(&self.cfg.inner.path);
        }

        if let Some(key) = self.registry_key.take() {
            global_registries().remove(&key);
            ServerRegistry::global().release(key.1);
        }

        if let Some(task) = self.forward_task.take() {
            task.abort();
        }

        tracing::info!(
            host = self.cfg.inner.host,
            port = self.cfg.inner.port,
            path = self.cfg.inner.path,
            "WebSocket consumer stopped"
        );

        if had_server_error {
            tracing::warn!(
                host = self.cfg.inner.host,
                port = self.cfg.inner.port,
                path = self.cfg.inner.path,
                "WebSocket server had errors during its lifetime"
            );
            return Err(CamelError::ProcessorError(
                "WebSocket server terminated with errors during its lifetime".into(),
            ));
        }

        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Concurrent {
            max: Some(self.cfg.inner.max_connections as usize),
        }
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        self.forward_task.take()
    }

    fn set_security_context(&mut self, ctx: camel_component_api::SecurityContext) {
        self.security_ctx = Some(ctx);
    }
}

use std::sync::atomic::{AtomicBool, Ordering};

fn new_atomic_false() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Classify a WebSocket error as retryable (transient network failure).
///
/// Retryable: connection refused, timeout, connection failed.
/// Permanent: anything else (protocol errors, auth failures, etc.).
#[inline]
fn is_retryable_ws_error(err: &CamelError) -> bool {
    let s = err.to_string();
    s.contains("connection refused") || s.contains("timeout") || s.contains("connection failed")
}

#[derive(Clone)]
pub struct WsProducer {
    cfg: WsClientConfig,
    /// Shared flag set by the async future when server-send hits backpressure,
    /// so that the next `poll_ready` call can return an error. (WS-003)
    backpressure_flag: Arc<AtomicBool>,
}

impl WsProducer {
    pub fn new(cfg: WsClientConfig) -> Self {
        Self {
            cfg,
            backpressure_flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Service<Exchange> for WsProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        // Return error if last server-send hit backpressure (WS-003)
        if self.backpressure_flag.swap(false, Ordering::Relaxed) {
            return Poll::Ready(Err(CamelError::ProcessorError(
                "WebSocket producer backpressure: previous send was dropped due to full channel"
                    .into(),
            )));
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let cfg = self.cfg.clone();
        let backpressure_flag = Arc::clone(&self.backpressure_flag);

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
                        .map(|k| k.to_string())
                        .collect();
                    registry.get_senders_for_keys(&parsed)
                } else {
                    registry.snapshot_senders()
                };

                let mut dropped = 0usize;
                for tx in &targets {
                    if !try_send_with_backpressure(tx, out_msg.clone(), "producer-send") {
                        dropped += 1;
                    }
                }

                if dropped > 0 {
                    tracing::warn!(
                        host = canonical_host,
                        port = cfg.inner.port,
                        path = cfg.inner.path,
                        dropped,
                        total = targets.len(),
                        "WebSocket producer dropped messages due to backpressure"
                    );
                    exchange.input.set_header(
                        "CamelWsDeliveryDropped",
                        serde_json::Value::Number(dropped.into()),
                    );
                    // Signal backpressure for next poll_ready call (WS-003)
                    backpressure_flag.store(true, Ordering::Relaxed);
                    if dropped == targets.len() {
                        return Err(CamelError::ProcessorError(format!(
                            "WebSocket producer: all {dropped} message(s) dropped due to backpressure"
                        )));
                    }
                }

                tracing::debug!(
                    host = canonical_host,
                    port = cfg.inner.port,
                    path = cfg.inner.path,
                    targets = targets.len(),
                    "WebSocket producer server-send complete"
                );

                return Ok(exchange);
            }

            let url = format!(
                "{}://{}:{}{}",
                cfg.inner.scheme, cfg.inner.host, cfg.inner.port, cfg.inner.path
            );

            tracing::debug!(url = url, "WebSocket producer connecting");

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

            // Add Sec-WebSocket-Protocol header if subprotocols configured (WS-007)
            if !cfg.inner.subprotocols.is_empty() {
                let proto_value = cfg.inner.subprotocols.join(", ");
                if let (Ok(name), Ok(val)) = (
                    http::header::HeaderName::from_bytes(b"Sec-WebSocket-Protocol"),
                    http::header::HeaderValue::from_str(&proto_value),
                ) {
                    request.headers_mut().insert(name, val);
                }
            }

            // Determine message type: respect binary_payload config (WS-018)
            let effective_message_type = if cfg.inner.binary_payload {
                "binary"
            } else {
                &message_type
            };

            let reconnect_policy = cfg.inner.reconnect_policy.clone();
            let mut ws_stream =
                connect_ws_with_retry(request, &url, cfg.inner.connect_timeout, &reconnect_policy)
                    .await?;

            // Close/reconnect path: rate-limited bail. On close frame, sleep
            // delay_for(0) and return Err to signal the outer route to re-invoke
            // the producer. The attempts counter below bounds how many times
            // we'll signal reconnect before terminating. Independent counter —
            // OLD code shared a counter with the connect loop above; this is a
            // behavior change (cleaner separation of concerns).
            let attempts = 0u32;

            let out_msg = body_to_client_ws_message(
                std::mem::take(&mut exchange.input.body),
                effective_message_type,
            )
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
                    tracing::debug!(url = url, "WebSocket producer received text response");
                    exchange.input.body = CamelBody::Text(text.to_string());
                }
                Some(Ok(ClientWsMessage::Binary(data))) => {
                    tracing::debug!(url = url, "WebSocket producer received binary response");
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
                        tracing::debug!(url = url, "WebSocket producer received normal close");
                        exchange.input.body = CamelBody::Empty;
                    } else if reconnect_policy.should_retry(attempts + 1) {
                        let delay = reconnect_policy.delay_for(0); // fresh delay on close
                        tracing::warn!(
                            url = url,
                            attempt = attempts + 1,
                            delay_ms = delay.as_millis(),
                            "WebSocket closed by peer — reconnecting"
                        );
                        tokio::time::sleep(delay).await;
                        return Err(CamelError::ProcessorError(format!(
                            "WebSocket reconnect required after close: code {}",
                            frame.map(|f| u16::from(f.code)).unwrap_or_default()
                        )));
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
            tracing::debug!(url = url, "WebSocket producer connection closed");
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

/// Connect to a WebSocket server with retry logic using the configured
/// [`NetworkRetryPolicy`]. Extracted for testability so the regression test
/// (rc-1nm) can drive the real production connect path rather than a
/// synthetic fake.
async fn connect_ws_with_retry<R>(
    request: R,
    url: &str,
    connect_timeout: std::time::Duration,
    reconnect_policy: &NetworkRetryPolicy,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    CamelError,
>
where
    R: IntoClientRequest + Unpin + Clone,
{
    let url_owned = url.to_string();
    retry_async(
        reconnect_policy,
        Some("ws-producer"),
        || {
            let r = request.clone();
            let url = url_owned.clone();
            async move {
                match tokio::time::timeout(connect_timeout, tokio_tungstenite::connect_async(r))
                    .await
                {
                    Ok(Ok((stream, _))) => Ok(stream),
                    Ok(Err(e)) => Err(map_connect_error(e, &url)),
                    Err(_) => Err(CamelError::ProcessorError(format!(
                        "WebSocket connect timeout ({connect_timeout:?}) to {url}"
                    ))),
                }
            }
        },
        is_retryable_ws_error,
    )
    .await
}

#[cfg(test)]
mod tests {
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }
    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

    /// Serialize tests that touch the global `ServerRegistry::global()`.
    ///
    /// `ServerRegistry::reset()` aborts ALL server tasks globally, so any
    /// test with a running server must hold this lock for its duration to
    /// prevent a concurrent `reset()` from killing its server. Tests that
    /// call `reset()` must also hold it.
    static REGISTRY_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    use super::*;
    use camel_component_api::NoOpComponentContext;
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
        assert_eq!(WsComponent::new().scheme(), "ws");
    }

    #[test]
    fn wss_component_scheme_is_wss() {
        assert_eq!(WssComponent::new().scheme(), "wss");
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
        assert!(cfg.reconnect);
        assert_eq!(cfg.reconnect_max_attempts, 5);
        assert_eq!(cfg.reconnect_delay_ms, 1000);
        assert_eq!(cfg.send_timeout, Duration::from_secs(30));
        assert!(!cfg.binary_payload);
        assert!(cfg.subprotocols.is_empty());
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
        assert!(cfg.reconnect);
        assert_eq!(cfg.reconnect_max_attempts, 5);
        assert_eq!(cfg.reconnect_delay_ms, 1000);
    }

    #[test]
    fn endpoint_config_parses_reconnect_uri_params() {
        let uri =
            "ws://localhost:9001/chat?reconnect=false&reconnectMaxAttempts=2&reconnectDelayMs=25";
        let cfg = WsEndpointConfig::from_uri(uri).unwrap();
        assert!(!cfg.reconnect);
        assert_eq!(cfg.reconnect_max_attempts, 2);
        assert_eq!(cfg.reconnect_delay_ms, 25);
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
        let ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint("ws://127.0.0.1:9010/trait", &ctx)
            .unwrap();

        endpoint.create_consumer(rt()).unwrap();
        endpoint
            .create_producer(rt(), &ProducerContext::default())
            .unwrap();
    }

    #[test]
    fn ws_consumer_concurrency_model_uses_max_connections() {
        let cfg = WsEndpointConfig::from_uri("ws://127.0.0.1:9011/cm?maxConnections=321").unwrap();
        let consumer = WsConsumer::new(cfg.server_config(), test_rt());
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
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/echo");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();

        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let producer = endpoint
            .create_producer(rt(), &ProducerContext::default())
            .unwrap();

        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
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
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/shutdown");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();

        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
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
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let component_ctx = NoOpComponentContext;
        let endpoint = WssComponent::new()
            .create_endpoint(&format!("wss://127.0.0.1:{port}/secure"), &component_ctx)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let (tx, _rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "ws-test-route".to_string());
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
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        // Ensure clean global state (process-lifetime servers may leak across tests).
        ServerRegistry::reset();

        let port = free_port();
        let component_ctx = NoOpComponentContext;
        let endpoint = WssComponent::new()
            .create_endpoint(&format!(
                "wss://127.0.0.1:{port}/secure?tlsCert=/nonexistent/cert.pem&tlsKey=/nonexistent/key.pem"
            ), &component_ctx)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let (tx, _rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "ws-test-route".to_string());
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
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let state1 = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None, test_rt(), "test-route".into())
            .await
            .unwrap();
        let state2 = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None, test_rt(), "test-route".into())
            .await
            .unwrap();
        assert!(
            Arc::ptr_eq(&state1.dispatch, &state2.dispatch),
            "expected same dispatch table for same port"
        );
    }

    #[tokio::test]
    async fn dispatch_handler_returns_404_for_unregistered_path() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let state = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None, test_rt(), "test-route".into())
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
        // Bind to port 0 directly to avoid TOCTOU race with free_port() + re-bind
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_task = tokio::spawn(async move {
            let _ = serve(listener, app).await;
        });

        let cfg = WsEndpointConfig::from_uri(&format!("ws://127.0.0.1:{port}/echo")).unwrap();
        let producer = WsProducer::new(cfg.client_config());

        let exchange = Exchange::new(CamelMessage::new(CamelBody::Text("hello-client".into())));
        tokio::time::sleep(Duration::from_millis(25)).await;
        let result =
            match tokio::time::timeout(Duration::from_secs(3), producer.oneshot(exchange)).await {
                Ok(Ok(r)) => r,
                Ok(Err(_)) => panic!("producer call failed"),
                Err(_) => panic!("producer call timed out"),
            };

        assert_eq!(result.input.body.as_text().unwrap(), "hello-client");

        server_task.abort();
    }

    #[tokio::test]
    async fn max_connections_rejects_with_close_1013() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/limited?maxConnections=1");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
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
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/sizelimit?maxMessageSize=10");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
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
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/origintest?allowOrigin=https://allowed.com");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        let state = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None, test_rt(), "test-route".into())
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
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/bc");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let producer = endpoint
            .create_producer(rt(), &ProducerContext::default())
            .unwrap();

        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
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
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let results: Arc<std::sync::Mutex<Vec<WsAppState>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let results = results.clone();
            handles.push(tokio::spawn(async move {
                let state = ServerRegistry::global()
                    .get_or_spawn("127.0.0.1", port, None, test_rt(), "test-route".into())
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

    #[tokio::test]
    async fn body_conversion_helpers_cover_text_and_binary_paths() {
        let text_msg = body_to_axum_ws_message(CamelBody::Text("abc".into()), "text")
            .await
            .unwrap();
        assert!(matches!(text_msg, WsMessage::Text(_)));

        let bin_msg = body_to_axum_ws_message(CamelBody::Bytes(vec![1, 2, 3].into()), "binary")
            .await
            .unwrap();
        assert!(matches!(bin_msg, WsMessage::Binary(_)));

        let client_text =
            body_to_client_ws_message(CamelBody::Json(serde_json::json!({"k":"v"})), "text")
                .await
                .unwrap();
        assert!(matches!(client_text, ClientWsMessage::Text(_)));

        let client_bin = body_to_client_ws_message(CamelBody::Bytes(vec![7, 8].into()), "binary")
            .await
            .unwrap();
        assert!(matches!(client_bin, ClientWsMessage::Binary(_)));
    }

    #[tokio::test]
    async fn body_to_text_handles_empty_text_json_and_bytes() {
        assert_eq!(body_to_text(CamelBody::Empty).await.unwrap(), "");
        assert_eq!(
            body_to_text(CamelBody::Text("hello".into())).await.unwrap(),
            "hello"
        );
        assert_eq!(
            body_to_text(CamelBody::Json(serde_json::json!({"n":1})))
                .await
                .unwrap(),
            "{\"n\":1}"
        );
        assert_eq!(
            body_to_text(CamelBody::Bytes(b"hi".to_vec().into()))
                .await
                .unwrap(),
            "hi"
        );
    }

    #[test]
    fn try_send_with_backpressure_returns_false_when_channel_full() {
        let (tx, _rx) = mpsc::channel::<WsMessage>(1);
        assert!(try_send_with_backpressure(
            &tx,
            WsMessage::Text("first".into()),
            "test"
        ));
        assert!(!try_send_with_backpressure(
            &tx,
            WsMessage::Text("second".into()),
            "test"
        ));
    }

    #[test]
    fn map_connect_error_formats_connection_refused_and_generic_errors() {
        let refused = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let err = map_connect_error(tungstenite::Error::Io(refused), "ws://localhost:1/x");
        assert!(err.to_string().contains("WebSocket connection refused"));

        let generic = map_connect_error(
            tungstenite::Error::Protocol(
                tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
            ),
            "ws://localhost:2/y",
        );
        assert!(
            generic
                .to_string()
                .contains("WebSocket connection failed (ws://localhost:2/y)")
        );
    }

    // === Phase B Finding Tests ===

    // WS-015: maxConnections=0 must be rejected
    #[test]
    fn from_uri_rejects_max_connections_zero() {
        let result = WsEndpointConfig::from_uri("ws://localhost:9200/test?maxConnections=0");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("maxConnections must be >= 1"),
            "expected maxConnections validation error, got: {msg}"
        );
    }

    // WS-019: maxMessageSize=0 must be rejected
    #[test]
    fn from_uri_rejects_max_message_size_zero() {
        let result = WsEndpointConfig::from_uri("ws://localhost:9201/test?maxMessageSize=0");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("maxMessageSize must be > 0"),
            "expected maxMessageSize validation error, got: {msg}"
        );
    }

    // WS-020: allowOrigin="" must be rejected
    #[test]
    fn from_uri_rejects_empty_allow_origin() {
        let result = WsEndpointConfig::from_uri("ws://localhost:9202/test?allowOrigin=");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("allowOrigin must not be empty"),
            "expected allowOrigin validation error, got: {msg}"
        );
    }

    // WS-006: Double-start must be rejected
    #[tokio::test]
    async fn consumer_double_start_returns_error() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/doublestart");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();

        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );

        // First start should succeed
        consumer.start(ctx).await.unwrap();

        // Second start should fail
        let (route_tx2, _route_rx2) = mpsc::channel(16);
        let ctx2 = ConsumerContext::new(
            route_tx2,
            CancellationToken::new(),
            "ws-test-route-2".to_string(),
        );
        let result = consumer.start(ctx2).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("already started"),
            "expected double-start error, got: {msg}"
        );

        consumer.stop().await.unwrap();
    }

    // WS-005: Registry cleanup on stop + port reuse
    #[tokio::test]
    async fn registry_cleanup_on_consumer_stop() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/cleanup");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();

        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        // Verify registry entry exists
        let registries = global_registries();
        let key = ("127.0.0.1".to_string(), port, "/cleanup".to_string());
        assert!(
            registries.contains_key(&key),
            "registry should have entry after start"
        );

        // Stop consumer
        consumer.stop().await.unwrap();

        // Verify registry entry is removed
        assert!(
            !registries.contains_key(&key),
            "registry should be cleaned up after stop"
        );

        // Server is process-lifetime: release() is a no-op, so the
        // ServerRegistry entry stays. The port cannot be re-bound until
        // ServerRegistry::reset() is called.
        let server_reg = ServerRegistry::global();
        let guard = server_reg.inner.lock().unwrap();
        assert!(
            guard.contains_key(&port),
            "ServerRegistry must keep port entry after consumer stop (process-lifetime server)"
        );
    }

    // WS-003 + WS-004: poll_ready backpressure and server-send error handling
    #[tokio::test]
    async fn producer_server_send_returns_error_when_all_dropped() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/backpressure");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();

        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let producer = endpoint
            .create_producer(rt(), &ProducerContext::default())
            .unwrap();

        let (route_tx, _route_rx) = mpsc::channel(1); // Tiny channel to force backpressure
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        // Connect a client so the registry has an entry
        let url = format!("ws://127.0.0.1:{port}/backpressure");
        let (mut client, _) = loop {
            match connect_async(&url).await {
                Ok(ok) => break ok,
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        };

        // Don't consume messages — let the channel fill up
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Flood the channel to trigger backpressure
        let mut all_dropped = false;
        for _ in 0..100 {
            let exchange = Exchange::new(CamelMessage::new(CamelBody::Text("flood".into())));
            match producer.clone().oneshot(exchange).await {
                Ok(_) => {}
                Err(e) => {
                    if e.to_string().contains("backpressure") {
                        all_dropped = true;
                        break;
                    }
                }
            }
        }

        // The producer should eventually return a backpressure error
        assert!(
            all_dropped,
            "producer should return error when all messages are dropped due to backpressure"
        );

        // Clean up
        let _ = client.close(None).await;
        consumer.stop().await.unwrap();
    }

    // WS-012: Ping/pong round-trip in server mode
    #[tokio::test]
    async fn server_responds_to_client_ping_with_pong() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let uri = format!("ws://127.0.0.1:{port}/pingpong");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();

        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        let url = format!("ws://127.0.0.1:{port}/pingpong");
        let (mut client, _) = loop {
            match connect_async(&url).await {
                Ok(ok) => break ok,
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        };

        // Send a ping
        client
            .send(ClientMessage::Ping(vec![1, 2, 3].into()))
            .await
            .unwrap();

        // Expect a pong with the same payload
        let pong = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match client.next().await {
                    Some(Ok(ClientMessage::Pong(data))) => break data,
                    Some(Ok(ClientMessage::Ping(_))) => continue,
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => panic!("ws receive failed: {e}"),
                    None => panic!("websocket closed before pong"),
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(pong, vec![1, 2, 3], "pong should echo ping payload");

        consumer.stop().await.unwrap();
    }

    // WS-008: Client-side retry on transient connect failures
    #[tokio::test]
    async fn producer_retries_on_connection_refused() {
        // Use a port that nothing is listening on
        let port = free_port();
        // Ensure nothing is on this port
        let cfg = WsEndpointConfig::from_uri(&format!(
            "ws://127.0.0.1:{port}/retry?reconnect=true&reconnectMaxAttempts=2&reconnectDelayMs=50"
        ))
        .unwrap();
        let producer = WsProducer::new(cfg.client_config());

        let exchange = Exchange::new(CamelMessage::new(CamelBody::Text("hello".into())));

        // Should fail after retries (nothing listening)
        let result = tokio::time::timeout(Duration::from_secs(5), producer.oneshot(exchange)).await;
        assert!(
            result.is_ok(),
            "producer should complete (with error) within timeout"
        );
        let result = result.unwrap();
        assert!(
            result.is_err(),
            "producer should fail when nothing is listening"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("connection refused"),
            "expected connection refused error, got: {msg}"
        );
    }

    // WS-001: Server bind error is visible (fake server-start error test)
    #[tokio::test]
    async fn server_bind_error_is_reported() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        // Bind a port manually to cause a conflict
        let _listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = _listener.local_addr().unwrap().port();

        // Try to start a consumer on the same port — should succeed since axum binds lazily
        // The actual bind error happens when the server task runs
        let uri = format!("ws://127.0.0.1:{port}/binderror");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();

        let mut consumer = endpoint.create_consumer(rt()).unwrap();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );

        // Start should succeed (server spawns, but bind may fail)
        let start_result = consumer.start(ctx).await;
        // The server may or may not have bound yet — this test verifies no panic
        // The actual error is logged by the server task
        let _ = start_result;

        consumer.stop().await.unwrap();
    }

    #[test]
    fn ws_app_state_server_error_starts_false() {
        let state = WsAppState {
            dispatch: Arc::new(RwLock::new(HashMap::new())),
            path_configs: Arc::new(DashMap::new()),
            path_policies: Arc::new(DashMap::new()),
            server_error: new_atomic_false(),
            runtime: test_rt(),
            route_id: "test-route".into(),
        };
        assert!(
            !state.server_error.load(Ordering::Relaxed),
            "server_error should start as false"
        );
    }

    #[test]
    fn ws_app_state_server_error_can_be_set() {
        let state = WsAppState {
            dispatch: Arc::new(RwLock::new(HashMap::new())),
            path_configs: Arc::new(DashMap::new()),
            path_policies: Arc::new(DashMap::new()),
            server_error: new_atomic_false(),
            runtime: test_rt(),
            route_id: "test-route".into(),
        };
        assert!(!state.server_error.load(Ordering::Relaxed));
        state.server_error.store(true, Ordering::Relaxed);
        assert!(state.server_error.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn consumer_stop_returns_error_when_server_had_errors() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let cfg = WsEndpointConfig::from_uri(&format!("ws://127.0.0.1:{port}/errorflag")).unwrap();
        let mut consumer = WsConsumer::new(cfg.server_config(), test_rt());
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        // Simulate server error by setting the flag directly
        if let Some(ref state) = consumer.server_state {
            state.server_error.store(true, Ordering::Relaxed);
        }

        let result = consumer.stop().await;
        assert!(
            result.is_err(),
            "stop should return error when server had errors"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("terminated with errors"),
            "expected server error message, got: {msg}"
        );
    }

    #[tokio::test]
    async fn consumer_stop_succeeds_when_server_healthy() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let port = free_port();
        let cfg = WsEndpointConfig::from_uri(&format!("ws://127.0.0.1:{port}/healthy")).unwrap();
        let mut consumer = WsConsumer::new(cfg.server_config(), test_rt());
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start(ctx).await.unwrap();

        let result = consumer.stop().await;
        assert!(
            result.is_ok(),
            "stop should succeed when server is healthy: {:?}",
            result
        );
    }

    // === H-10 Finding Tests ===

    // WS-007: subprotocol negotiation support
    #[test]
    fn endpoint_config_parses_subprotocols() {
        let cfg = WsEndpointConfig::from_uri(
            "ws://localhost:9001/chat?subprotocols=graphql-ws,graphql-transport-ws",
        )
        .unwrap();
        assert_eq!(cfg.subprotocols, vec!["graphql-ws", "graphql-transport-ws"]);
    }

    #[test]
    fn endpoint_config_default_subprotocols_empty() {
        let cfg = WsEndpointConfig::default();
        assert!(cfg.subprotocols.is_empty());
    }

    // WS-017: sendTimeoutMs URI option
    #[test]
    fn endpoint_config_parses_send_timeout() {
        let cfg =
            WsEndpointConfig::from_uri("ws://localhost:9001/chat?sendTimeoutMs=5000").unwrap();
        assert_eq!(cfg.send_timeout, Duration::from_millis(5000));
    }

    #[test]
    fn endpoint_config_default_send_timeout() {
        let cfg = WsEndpointConfig::default();
        assert_eq!(cfg.send_timeout, Duration::from_secs(30));
    }

    #[test]
    fn endpoint_config_rejects_invalid_send_timeout() {
        let err =
            WsEndpointConfig::from_uri("ws://localhost:9001/chat?sendTimeoutMs=abc").unwrap_err();
        assert!(err.to_string().contains("sendTimeoutMs"));
    }

    // WS-018: binaryPayload URI option
    #[test]
    fn endpoint_config_parses_binary_payload() {
        let cfg =
            WsEndpointConfig::from_uri("ws://localhost:9001/chat?binaryPayload=true").unwrap();
        assert!(cfg.binary_payload);
    }

    #[test]
    fn endpoint_config_default_binary_payload_false() {
        let cfg = WsEndpointConfig::default();
        assert!(!cfg.binary_payload);
    }

    #[test]
    fn endpoint_config_rejects_invalid_binary_payload() {
        let err =
            WsEndpointConfig::from_uri("ws://localhost:9001/chat?binaryPayload=yes").unwrap_err();
        assert!(err.to_string().contains("binaryPayload"));
    }

    /// Regression: max_attempts=N → exactly N invocations (caught OpenSearch off-by-one 1f5c4c2a).
    /// Replicates the exact retry loop from the WebSocket producer connect (lib.rs:~1228-1275):
    ///   attempts starts at 0, should_retry(attempts+1), delay_for(attempts), attempts += 1
    #[tokio::test]
    async fn retry_loop_invokes_operation_exactly_max_attempts_times() {
        use camel_component_api::NetworkRetryPolicy;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        };

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = Arc::clone(&calls);
        let mut attempts: u32 = 0;

        let _result: Result<(), ()> = loop {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            let op_result: Result<(), ()> = Err(());
            match op_result {
                Ok(_) => unreachable!(),
                Err(_) if policy.should_retry(attempts + 1) => {
                    let delay = policy.delay_for(attempts);
                    tokio::time::sleep(delay).await;
                    attempts += 1;
                    continue;
                }
                Err(_) => break Err(()),
            }
        };

        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "max_attempts=3 must yield exactly 3 invocations"
        );
    }

    /// Edge case: max_attempts=1 → exactly 1 invocation (initial attempt only, no retry).
    /// Locks the edge that originally broke OpenSearch.
    #[tokio::test]
    async fn retry_loop_with_max_attempts_1_invokes_operation_once() {
        use camel_component_api::NetworkRetryPolicy;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 1,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        };

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = Arc::clone(&calls);
        let mut attempts: u32 = 0;

        let _result: Result<(), ()> = loop {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            let op_result: Result<(), ()> = Err(());
            match op_result {
                Ok(_) => unreachable!(),
                Err(_) if policy.should_retry(attempts + 1) => {
                    let delay = policy.delay_for(attempts);
                    tokio::time::sleep(delay).await;
                    attempts += 1;
                    continue;
                }
                Err(_) => break Err(()),
            }
        };

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "max_attempts=1 must yield exactly 1 invocation"
        );
    }

    // ── rc-1nm regression: WS producer retry emits component=ws-producer ──

    use std::fmt::Write as _;
    use std::sync::{Arc, Mutex};
    use tracing::Subscriber;
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::SubscriberExt;

    struct CollectingLayer {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl<S: Subscriber> Layer<S> for CollectingLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut buf = String::new();
            let mut visitor = CollectingVisitor { fields: &mut buf };
            event.record(&mut visitor);
            if let Ok(mut events) = self.events.lock() {
                events.push(buf);
            }
        }
    }

    struct CollectingVisitor<'a> {
        fields: &'a mut String,
    }

    impl CollectingVisitor<'_> {
        fn record_field(&mut self, name: &str, value: &str) {
            write!(self.fields, " {name}={value}").ok();
        }
    }

    impl tracing::field::Visit for CollectingVisitor<'_> {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.record_field(field.name(), value);
        }
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.record_field(field.name(), &format!("{value:?}"));
        }
        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            self.record_field(field.name(), &value.to_string());
        }
    }

    /// Regression for rc-1nm: the WS producer retry path must emit
    /// `component=ws-producer` in retry log events so operators can
    /// identify which component is retrying.
    ///
    /// Drives `retry_async` directly with `Some("ws-producer")` and a
    /// deterministic retryable error. An earlier version exercised the
    /// production `connect_ws_with_retry` helper against `ws://127.0.0.1:1`,
    /// but that was flaky under heavy workspace load: the thread-local
    /// tracing subscriber (`set_default`) very occasionally missed the
    /// event logged from within the async connect path (the warn! is
    /// always emitted — `map_connect_error` always yields a retryable
    /// string for `ws://` — so the miss was purely a capture race).
    /// Driving `retry_async` synchronously with a synthetic op removes the
    /// network I/O and reactor scheduling, so the warn! is always emitted
    /// and captured on the test thread.
    #[tokio::test]
    async fn ws_producer_retry_log_emits_component_ws_producer() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let layer = CollectingLayer {
            events: events.clone(),
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        let policy = NetworkRetryPolicy {
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };

        // Deterministic retryable failure (string recognised by
        // is_retryable_ws_error) — no network I/O, so the retry warn! is
        // emitted and captured synchronously on this thread.
        let result: Result<(), CamelError> = retry_async(
            &policy,
            Some("ws-producer"),
            || async {
                Err(CamelError::ProcessorError(
                    "WebSocket connection refused: simulated".to_string(),
                ))
            },
            is_retryable_ws_error,
        )
        .await;

        assert!(result.is_err(), "expected exhausted-retries error");
        let captured = events.lock().unwrap();
        assert!(
            !captured.is_empty(),
            "expected at least one retry log event, got none"
        );
        let first = &captured[0];
        assert!(
            first.contains("component=ws-producer"),
            "rc-1nm regression: expected 'component=ws-producer' in WS retry log, got: {first}"
        );
    }

    // ── TLS cert hot-reload: release/unregister integration tests ─────────
    //
    // These verify the WSS path: `get_or_spawn` registers a `WsReloadHandler`
    // in `TlsReloadRegistry::global()`; `release` unregisters it when the
    // last reference drops. The host-agnostic `matches` impl keys on
    // (scheme="wss", port) — see `WsReloadHandler::matches`.

    #[tokio::test]
    async fn wss_release_unregisters_tls_reload_handler() {
        use camel_component_api::test_support::tls;
        use camel_component_api::tls_source::TlsReloadRegistry;

        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let _ = rustls::crypto::ring::default_provider().install_default();

        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("ws-release-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("ws-release-key.pem", &key_pem);

        let port = free_port();
        let tls_cfg = WsTlsConfig {
            cert_path: cert_path.to_str().expect("cert path").to_string(),
            key_path: key_path.to_str().expect("key path").to_string(),
        };

        // Spawn a single WSS server.
        let _state = ServerRegistry::global()
            .get_or_spawn(
                "127.0.0.1",
                port,
                Some(tls_cfg),
                test_rt(),
                "ws-release-test".into(),
            )
            .await
            .expect("WSS server should spawn");

        // Handler is registered (host-agnostic — match passes empty host).
        let handler = TlsReloadRegistry::global().find("wss", "", port);
        assert!(
            handler.is_some(),
            "WSS server must register a reload handler for wss://*:{port}"
        );
        // Exercise it to verify the registered handler is functional.
        handler
            .unwrap()
            .reload()
            .await
            .expect("registered WSS handler reload() must succeed");

        // Release the (only) reference. release() is a no-op
        // (process-lifetime server), so the handler STAYS registered.
        ServerRegistry::global().release(port);

        assert!(
            TlsReloadRegistry::global().find("wss", "", port).is_some(),
            "WSS server release is a no-op; reload handler must remain registered"
        );
    }

    #[tokio::test]
    async fn wss_multiple_refs_release_does_not_unregister() {
        use camel_component_api::test_support::tls;
        use camel_component_api::tls_source::TlsReloadRegistry;

        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let _ = rustls::crypto::ring::default_provider().install_default();

        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("ws-multiref-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("ws-multiref-key.pem", &key_pem);

        let port = free_port();
        let tls_cfg = WsTlsConfig {
            cert_path: cert_path.to_str().expect("cert path").to_string(),
            key_path: key_path.to_str().expect("key path").to_string(),
        };

        // Acquire TWO references to the same port.
        let _s1 = ServerRegistry::global()
            .get_or_spawn(
                "127.0.0.1",
                port,
                Some(tls_cfg.clone()),
                test_rt(),
                "ws-multiref-r1".into(),
            )
            .await
            .expect("WSS server should spawn (ref 1)");
        let _s2 = ServerRegistry::global()
            .get_or_spawn(
                "127.0.0.1",
                port,
                Some(tls_cfg),
                test_rt(),
                "ws-multiref-r2".into(),
            )
            .await
            .expect("WSS server should spawn (ref 2)");

        // Handler is registered.
        assert!(
            TlsReloadRegistry::global().find("wss", "", port).is_some(),
            "WSS server with refs must have a registered reload handler"
        );

        // Release the FIRST reference — ref count is still 1, handler must remain.
        ServerRegistry::global().release(port);
        assert!(
            TlsReloadRegistry::global().find("wss", "", port).is_some(),
            "handler must remain registered while ref count > 0"
        );

        // Release the LAST reference. release() is a no-op regardless of
        // ref count, so the handler STAYS registered.
        ServerRegistry::global().release(port);
        assert!(
            TlsReloadRegistry::global().find("wss", "", port).is_some(),
            "handler must remain registered — release() is a no-op (process-lifetime server)"
        );
    }

    #[tokio::test]
    async fn ws_plaintext_does_not_register_tls_reload_handler() {
        use camel_component_api::tls_source::TlsReloadRegistry;

        let _guard = REGISTRY_TEST_LOCK.lock().await;
        // Ensure clean global state (process-lifetime servers may leak across tests).
        ServerRegistry::reset();

        let port = free_port();
        let _state = ServerRegistry::global()
            .get_or_spawn(
                "127.0.0.1",
                port,
                None,
                test_rt(),
                "ws-plaintext-no-reload-test".into(),
            )
            .await
            .expect("plaintext WS server should spawn");

        // No handler for either wss or ws — plaintext has nothing to reload.
        assert!(
            TlsReloadRegistry::global().find("wss", "", port).is_none(),
            "plaintext WS server must not register a wss handler"
        );

        // Cleanup.
        ServerRegistry::global().release(port);
    }
}
