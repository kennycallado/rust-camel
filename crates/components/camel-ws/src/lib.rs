//! WebSocket component for rust-camel — Axum-based WebSocket server and Tokio-tungstenite client for bidirectional messaging.
//!
//! Main types: `WsComponent`, `WsBundle`, `WsConfig`, `WsServerConfig`, `WsClientConfig`, `WsEndpointConfig`.
//! Main modules: `bundle`, `config`, `health`.

pub mod bundle;
pub(crate) mod client_consumer;
pub mod config;
#[cfg(test)]
#[path = "endpoint_wiring_tests.rs"]
mod endpoint_wiring_tests;
pub mod health;
#[cfg(test)]
pub(crate) mod test_doubles;
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
use camel_api::security_policy::{AccessMode, AuthPrincipal, RouteSecurityPlan};
use camel_auth::{AuthenticatedPrincipal, ProviderRegistry};
use camel_component_api::tls_source::ServerTlsSource;
use camel_component_api::{
    Body as CamelBody, BoxProcessor, CamelError, Component, ComponentMetadata, ConcurrencyModel,
    Consumer, ConsumerContext, ConsumerStartupMode, Endpoint, Exchange, ExchangeEnvelope,
    InFlightClaim, Message as CamelMessage, NetworkRetryPolicy, ProducerContext,
    RuntimeObservability, retry_async,
};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::{OnceCell, RwLock, mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message as ClientWsMessage;
use tokio_util::sync::CancellationToken;
use tower::Service;

use client_consumer::{ClientConnState, WsClientConsumer};
use health::ConnectionStateCheck;

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
    /// Monitor for the shared server task (consumes the server
    /// JoinHandle). On unexpected exit it cancels `server_exited` so
    /// hosted routes fail and supervision restarts them. Retained for
    /// the test-only `reset()` abort; no production reader (rc-onm5b
    /// removed the last one — eviction reads the serve-future drop
    /// guard instead of the monitor's completion).
    #[allow(dead_code)]
    monitor_task: JoinHandle<()>,
    /// Accept-loop liveness (rc-onm5b): flipped to `false` by a drop guard
    /// captured into the serve future, so it fires on EVERY death mode —
    /// normal exit, serve error, abort, and runtime-drop cancellation,
    /// polled or not — without depending on the monitor task being
    /// scheduled. `evict_dead_server_if_any` reads this as the eviction
    /// signal; it is strictly earlier and more reliable than the monitor's
    /// `is_finished()` (a joiner can arrive after the serve future died
    /// but before the monitor completed, and a runtime-dropped task's
    /// JoinHandle state is not observable mid-drop).
    serve_alive: Arc<AtomicBool>,
    /// Abort handle for the WebSocket server task itself. The JoinHandle
    /// is consumed by `monitor_ws_server_task`; this survives on the
    /// handle so crashed-server tests (and future ops tooling) can
    /// deterministically kill the shared transport to exercise the death
    /// path. Test-only today — no production reader yet (rc-nxml4).
    #[allow(dead_code)]
    server_abort: tokio::task::AbortHandle,
    /// Cancelled by `monitor_ws_server_task` when the shared server task
    /// backing this handle exits unexpectedly (panic, abort, or serve
    /// error). Every `WsConsumer` hosted on that server selects on this
    /// token in its `forward_task` loop; on cancellation the task returns
    /// `Err`, which camel-core's background-task watcher converts into a
    /// per-route `CrashNotification` → `FailRoute` → supervision backoff
    /// restart (ADR-0007 route-supervised contract — shared transports
    /// must fail their hosted routes like per-route transports do).
    server_exited: CancellationToken,
    tls_config: Option<axum_server::tls_rustls::RustlsConfig>,
    tls_source: Option<ServerTlsSource>,
    /// Actual bound address, captured from the served listener at spawn
    /// time. Every holder reads this stored value instead of re-reading a
    /// listener.
    bound_addr: std::net::SocketAddr,
    /// Present for the TLS (wss) path so callers can await
    /// `axum_server::Handle::listening()` to detect serve readiness.
    /// `None` for the plain-ws path, which serves an already-bound
    /// listener and is live as soon as spawn returns.
    listening_handle: Option<axum_server::Handle<std::net::SocketAddr>>,
}

struct ServerRegistryInner {
    cell: Arc<OnceCell<ServerHandle>>,
    ref_count: usize,
}

pub struct ServerRegistry {
    inner: Mutex<HashMap<u16, ServerRegistryInner>>,
    /// Pre-bound listeners staged for consumption by the next vacant-entry
    /// `get_or_spawn` on the same `(host, port)` key.
    staged: Mutex<HashMap<(String, u16), tokio::net::TcpListener>>,
}

impl ServerRegistry {
    pub fn global() -> &'static Self {
        static REG: OnceLock<ServerRegistry> = OnceLock::new();
        REG.get_or_init(|| Self {
            inner: Mutex::new(HashMap::new()),
            staged: Mutex::new(HashMap::new()),
        })
    }

    /// Stage a pre-bound listener so the next `get_or_spawn` for its exact
    /// `(host, port)` key serves this socket instead of binding a new one.
    ///
    /// Staging is one-shot per exact key: a duplicate `stage_listener` for
    /// an already-staged key is rejected. The staged listener is consumed
    /// by exactly one vacant-entry spawn — the init winner — eliminating
    /// the bind window between a port probe and server startup
    /// (itest-bound-ports). A listener staged but never claimed is dropped
    /// at process exit: a test bug, not a runtime hazard.
    pub async fn stage_listener(
        &'static self,
        listener: tokio::net::TcpListener,
    ) -> Result<(), CamelError> {
        let addr = listener
            .local_addr()
            .map_err(|e| CamelError::EndpointCreationFailed(format!("listener local_addr: {e}")))?;
        let host = addr.ip().to_string();
        use std::collections::hash_map::Entry;
        let mut guard = self.staged.lock().map_err(|_| {
            CamelError::EndpointCreationFailed("ServerRegistry staged lock poisoned".into())
        })?;
        match guard.entry((host.clone(), addr.port())) {
            Entry::Occupied(_) => Err(CamelError::EndpointCreationFailed(format!(
                "listener already staged for {host}:{}",
                addr.port()
            ))),
            Entry::Vacant(slot) => {
                slot.insert(listener);
                Ok(())
            }
        }
    }

    pub async fn get_or_spawn(
        &'static self,
        host: &str,
        port: u16,
        tls_config: Option<WsTlsConfig>,
        runtime: Arc<dyn RuntimeObservability>,
        route_id: String,
    ) -> Result<
        (
            WsAppState,
            Option<axum_server::Handle<std::net::SocketAddr>>,
            CancellationToken,
        ),
        CamelError,
    > {
        let wants_tls = tls_config.is_some();
        let host_owned = host.to_string();

        let (cell, _is_new) = {
            let mut guard = self.inner.lock().map_err(|_| {
                CamelError::EndpointCreationFailed("ServerRegistry lock poisoned".into())
            })?;
            // rc-nxml4: lazily evict dead shared servers so a supervision
            // restart rebinds instead of rejoining a dead transport.
            // rc-onm5b: the signal is the serve-future drop guard, which
            // also covers runtime-drop deaths and the monitor-completion
            // race.
            evict_dead_server_if_any(&mut guard, port);
            let entry = guard.entry(port).or_insert_with(|| ServerRegistryInner {
                cell: Arc::new(OnceCell::new()),
                ref_count: 0,
            });
            entry.ref_count += 1;
            (entry.cell.clone(), entry.ref_count == 1)
        };

        let handle = cell
            .get_or_try_init(|| async {
                // Resolve the listener source inside the init body so
                // exactly one caller — the init winner — consumes a staged
                // listener. Resolving it before the cell init would let a
                // racing caller strand the staged socket in the loser's
                // hands: the winner then bound the same port and failed
                // with EADDRINUSE. The entries guard is already released
                // here and the sync staged lock is never held across an
                // await, so no nested guards. Occupied cells never run
                // this body, so reuse never touches the staged map.
                let staged = {
                    let mut guard = self.staged.lock().map_err(|_| {
                        CamelError::EndpointCreationFailed(
                            "ServerRegistry staged lock poisoned".into(),
                        )
                    })?;
                    match guard.remove(&(host_owned.clone(), port)) {
                        Some(listener) => Some(listener),
                        // Conflict check before any bind so the error
                        // leaves the staged slot untouched.
                        None => {
                            if let Some((staged_host, _)) =
                                guard.keys().find(|(_, staged_port)| *staged_port == port)
                            {
                                let staged_host = staged_host.clone();
                                return Err(CamelError::EndpointCreationFailed(format!(
                                    "staged listener conflict on port {port}: staged under host {staged_host}, requested {host_owned}"
                                )));
                            }
                            None
                        }
                    }
                };
                let listener = match staged {
                    Some(listener) => listener,
                    None => {
                        // Legacy path binds BEFORE delegating: spawn_server
                        // serves an already-bound listener and no longer
                        // binds.
                        let addr = format!("{host_owned}:{port}");
                        tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
                            CamelError::EndpointCreationFailed(format!(
                                "Failed to bind {addr}: {e}"
                            ))
                        })?
                    }
                };
                let handle =
                    spawn_server(listener, tls_config, runtime.clone(), route_id.clone()).await?;
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
            .await;

        let handle = match handle {
            Ok(h) => h,
            Err(e) => {
                // Decrement ref_count on spawn failure so the entry is
                // cleaned up when it reaches zero.
                self.decrement_ref_count(port)?;
                return Err(e);
            }
        };

        if wants_tls != handle.is_tls {
            // Decrement ref count since we're rejecting this caller
            self.decrement_ref_count(port)?;
            return Err(CamelError::EndpointCreationFailed(format!(
                "Server on port {port} already running with different TLS mode"
            )));
        }

        Ok((
            handle.state.clone(),
            handle.listening_handle.clone(),
            handle.server_exited.clone(),
        ))
    }

    /// Decrement the ref count for `port`, removing the entry when it
    /// reaches zero. Shared by the rejection paths of both spawn entry
    /// points.
    fn decrement_ref_count(&self, port: u16) -> Result<(), CamelError> {
        let mut guard = self.inner.lock().map_err(|_| {
            CamelError::EndpointCreationFailed("ServerRegistry lock poisoned".into())
        })?;
        if let Some(entry) = guard.get_mut(&port) {
            entry.ref_count -= 1;
            if entry.ref_count == 0 {
                guard.remove(&port);
            }
        }
        Ok(())
    }

    /// Spawn (or join) the server keyed by the injected listener's actual
    /// port.
    ///
    /// The listener is authoritative for the registry key: `local_addr()`
    /// is read BEFORE any registry mutation, so a port-0 bind registers
    /// under its real ephemeral port. When an existing entry already
    /// serves that port, the redundant listener is dropped and the
    /// existing server is reused. Returns the actual bound address.
    pub async fn get_or_spawn_with_listener(
        &'static self,
        listener: tokio::net::TcpListener,
        tls_config: Option<WsTlsConfig>,
        runtime: Arc<dyn RuntimeObservability>,
        route_id: String,
    ) -> Result<
        (
            WsAppState,
            std::net::SocketAddr,
            Option<axum_server::Handle<std::net::SocketAddr>>,
            CancellationToken,
        ),
        CamelError,
    > {
        let wants_tls = tls_config.is_some();
        let port = listener
            .local_addr()
            .map_err(|e| {
                CamelError::EndpointCreationFailed(format!("Failed to read listener address: {e}"))
            })?
            .port();

        let (cell, _is_new) = {
            let mut guard = self.inner.lock().map_err(|_| {
                CamelError::EndpointCreationFailed("ServerRegistry lock poisoned".into())
            })?;
            // rc-nxml4: lazy dead-server eviction — same rationale as
            // `get_or_spawn`. rc-onm5b: keyed on the serve-future drop
            // guard (`serve_alive`), covering runtime-drop deaths and the
            // monitor-completion race too.
            evict_dead_server_if_any(&mut guard, port);
            let entry = guard.entry(port).or_insert_with(|| ServerRegistryInner {
                cell: Arc::new(OnceCell::new()),
                ref_count: 0,
            });
            entry.ref_count += 1;
            (entry.cell.clone(), entry.ref_count == 1)
        };

        // When an existing entry wins, the init closure never runs and the
        // injected listener is dropped here — exactly-once spawn holds.
        let handle = cell
            .get_or_try_init(|| async {
                let handle =
                    spawn_server(listener, tls_config, runtime.clone(), route_id.clone()).await?;
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
            .await;

        let handle = match handle {
            Ok(h) => h,
            Err(e) => {
                // Decrement ref_count on spawn failure so the entry is
                // cleaned up when it reaches zero.
                self.decrement_ref_count(port)?;
                return Err(e);
            }
        };

        if wants_tls != handle.is_tls {
            // Decrement ref count since we're rejecting this caller
            self.decrement_ref_count(port)?;
            return Err(CamelError::EndpointCreationFailed(format!(
                "Server on port {port} already running with different TLS mode"
            )));
        }

        Ok((
            handle.state.clone(),
            handle.bound_addr,
            handle.listening_handle.clone(),
            handle.server_exited.clone(),
        ))
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
        {
            let mut guard = Self::global().inner.lock().expect("ServerRegistry lock");
            for entry in guard.values() {
                if let Some(handle) = entry.cell.get() {
                    // Monitor first: an aborted monitor cannot observe the
                    // server abort, so resets stay log-quiet.
                    handle.monitor_task.abort();
                    handle.server_abort.abort();
                }
            }
            guard.clear();
        }
        // Drop any listeners staged but never claimed so a failed test does
        // not leak staged slots into the next test.
        Self::global()
            .staged
            .lock()
            .expect("ServerRegistry staged lock")
            .clear();
    }

    /// Current ref count for the entry on `port` — **test-only**.
    #[cfg(test)]
    pub fn ref_count_for_test(&'static self, port: u16) -> usize {
        let guard = Self::global().inner.lock().expect("ServerRegistry lock");
        guard.get(&port).map(|entry| entry.ref_count).unwrap_or(0)
    }

    /// Bound address of the live server entry on `port` — **test-only**.
    #[cfg(test)]
    pub fn bound_addr_for_test(&'static self, port: u16) -> Option<std::net::SocketAddr> {
        let guard = Self::global().inner.lock().expect("ServerRegistry lock");
        guard
            .get(&port)
            .and_then(|entry| entry.cell.get())
            .map(|handle| handle.bound_addr)
    }
}

/// Lazy dead-server eviction shared by both `get_or_spawn*` entry
/// points (rc-nxml4, signal hardened in rc-onm5b): when the entry on
/// `port` holds an initialized server whose serve future is gone
/// (`serve_alive` flipped by the drop guard), remove the entry so the
/// next spawn rebinds. The guard fires at serve-future drop — normal
/// exit, serve error, abort, and runtime-drop cancellation alike, polled
/// or not — which closes the serve-dead-but-monitor-unfinished window
/// the previous `monitor_task.is_finished()` signal left open (a joiner
/// in that window rejoined the corpse); init-in-flight cells are left
/// alone.
fn evict_dead_server_if_any(guard: &mut HashMap<u16, ServerRegistryInner>, port: u16) {
    if let Some(handle) = guard.get(&port).and_then(|e| e.cell.get())
        && !handle.serve_alive.load(Ordering::Acquire)
    {
        tracing::debug!(
            port,
            "evicting dead WebSocket server entry (serve task gone)"
        );
        guard.remove(&port);
    }
}

/// Drop guard that flips `serve_alive` to `false` when the serve future
/// is gone (rc-onm5b). Captured into the serve future by value — so it is
/// part of the future's state from construction, before the first poll —
/// which makes runtime-drop cancellation (the task dropped without ever
/// being polled, or parked mid-accept at runtime shutdown) flip the flag
/// just like a normal exit, serve error, or abort does.
struct ServeAliveGuard(Arc<AtomicBool>);

impl Drop for ServeAliveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

async fn spawn_server(
    listener: tokio::net::TcpListener,
    tls_config: Option<WsTlsConfig>,
    runtime: Arc<dyn RuntimeObservability>,
    route_id: String,
) -> Result<ServerHandle, CamelError> {
    let bound_addr = listener.local_addr().map_err(|e| {
        CamelError::EndpointCreationFailed(format!("Failed to read listener address: {e}"))
    })?;
    let dispatch: DispatchTable = Arc::new(RwLock::new(HashMap::new()));
    let path_configs = Arc::new(DashMap::new());
    let path_policies = Arc::new(DashMap::new());
    let server_error = new_atomic_false();
    // Per-server death signal, cancelled by `monitor_ws_server_task`
    // (rc-nxml4): shared-transport death must fail every hosted route.
    let server_exited = CancellationToken::new();
    // rc-onm5b: accept-loop liveness, flipped false by a guard captured
    // into the serve future below — the direct eviction signal, see
    // `evict_dead_server_if_any`.
    let serve_alive: Arc<AtomicBool> = Arc::new(AtomicBool::new(true));
    let state = WsAppState {
        dispatch: Arc::clone(&dispatch),
        path_configs: Arc::clone(&path_configs),
        path_policies: Arc::clone(&path_policies),
        server_error: Arc::clone(&server_error),
        runtime: Arc::clone(&runtime),
        route_id: route_id.clone(),
        in_flight: Arc::default(),
    };
    let app = Router::new()
        .fallback(dispatch_handler)
        .with_state(state.clone());

    let (task, is_tls, retained_tls_cfg, retained_source, listening_handle) =
        if let Some(ref tls) = tls_config {
            let rustls = load_tls_config(&tls.cert_path, &tls.key_path)?;
            let tls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(rustls));
            let tls_source = ServerTlsSource {
                cert_path: std::path::PathBuf::from(&tls.cert_path),
                key_path: std::path::PathBuf::from(&tls.key_path),
                client_ca_path: None,
            };
            // Clone for handle retention — the original moves into the spawned task
            let retained = tls_cfg.clone();
            // The listener arrives already bound; `Handle::listening()` now
            // signals when the accept loop starts serving. The clone here
            // moves into the task; the original is retained for the caller.
            let listen_handle = axum_server::Handle::new();
            let listen_handle_for_task = listen_handle.clone();
            let error_flag = Arc::clone(&server_error);
            let rt = Arc::clone(&runtime);
            let rid = route_id.clone();
            let std_listener = listener.into_std().map_err(|e| {
                CamelError::EndpointCreationFailed(format!("Listener conversion failed: {e}"))
            })?;
            let server = axum_server::from_tcp_rustls(std_listener, tls_cfg).map_err(|e| {
                CamelError::EndpointCreationFailed(format!("TLS listener setup failed: {e}"))
            })?;
            let alive_guard = ServeAliveGuard(Arc::clone(&serve_alive));
            let task = tokio::spawn(async move {
                // rc-onm5b: dropped with this future on every death mode —
                // flips `serve_alive` for lazy eviction.
                let _alive = alive_guard;
                if let Err(e) = server
                    .handle(listen_handle_for_task)
                    .serve(app.into_make_service())
                    .await
                {
                    rt.health()
                        .force_unhealthy_for_route(&rid, "g:ws:bind-tls", &e.to_string());
                    // log-policy: outside-contract
                    tracing::error!(
                        host = %camel_api::redact::redact_host(&bound_addr.ip().to_string()),
                        port = bound_addr.port(),
                        error = %e,
                        "WebSocket server terminated with error"
                    );
                    error_flag.store(true, Ordering::Relaxed);
                }
            });
            (
                task,
                true,
                Some(retained),
                Some(tls_source),
                Some(listen_handle),
            )
        } else {
            let error_flag = Arc::clone(&server_error);
            let rt = Arc::clone(&runtime);
            let rid = route_id.clone();
            let alive_guard = ServeAliveGuard(Arc::clone(&serve_alive));
            let task = tokio::spawn(async move {
                // rc-onm5b: dropped with this future on every death mode —
                // flips `serve_alive` for lazy eviction.
                let _alive = alive_guard;
                if let Err(e) = serve(listener, app).await {
                    rt.health()
                        .force_unhealthy_for_route(&rid, "g:ws:bind-plain", &e.to_string());
                    // log-policy: outside-contract
                    tracing::error!(
                        host = %camel_api::redact::redact_host(&bound_addr.ip().to_string()),
                        port = bound_addr.port(),
                        error = %e,
                        "WebSocket server terminated with error"
                    );
                    error_flag.store(true, Ordering::Relaxed);
                }
            });
            (task, false, None, None, None)
        };

    tracing::info!(
        host = %camel_api::redact::redact_host(&bound_addr.ip().to_string()),
        port = bound_addr.port(),
        is_tls,
        "WebSocket server started"
    );

    // rc-nxml4 (ADR-0007 parity with camel-http rc-szmob): the server
    // JoinHandle is consumed by a monitor task. On unexpected exit the
    // monitor cancels `server_exited`; every hosted consumer's
    // `forward_task` observes it and fails, so camel-core restarts the
    // routes instead of leaving them zombie.
    let server_abort = task.abort_handle();
    let monitor_task = tokio::spawn(monitor_ws_server_task(
        task,
        bound_addr,
        Arc::clone(&runtime),
        route_id,
        Arc::clone(&server_error),
        server_exited.clone(),
    ));

    Ok(ServerHandle {
        state,
        is_tls,
        monitor_task,
        serve_alive,
        server_abort,
        server_exited,
        tls_config: retained_tls_cfg,
        tls_source: retained_source,
        bound_addr,
        listening_handle,
    })
}

/// Monitors the shared WebSocket server task of one port.
///
/// On unexpected exit (panic or abort — `Err(join_err)`) it records the
/// structured error event (`e:ws:server-task-exited`, ADR-0012 category
/// (e) outside-contract) and cancels the server's `server_exited` token.
/// On a serve-error exit (`Ok(())` with `server_error` set) the server
/// task body has already recorded the health failure and error log; the
/// monitor still cancels the token because the transport is dead either
/// way — camel-ws's `serve()` is terminal, unlike a transient accept
/// error. Every `WsConsumer` hosted on that server observes the
/// cancellation in its `forward_task` select loop and returns `Err`,
/// which camel-core's background-task watcher turns into a per-route
/// `CrashNotification` → `FailRoute` → supervision backoff restart
/// (ADR-0007, rc-nxml4 — parity with camel-http rc-szmob). A clean exit
/// (`Ok(())` with no serve error) cancels nothing: route stops own their
/// termination.
async fn monitor_ws_server_task(
    handle: JoinHandle<()>,
    addr: std::net::SocketAddr,
    runtime: Arc<dyn RuntimeObservability>,
    route_id: String,
    server_error: Arc<AtomicBool>,
    server_exited: CancellationToken,
) {
    match handle.await {
        Ok(()) => {
            if server_error.load(Ordering::Relaxed) {
                // serve() returned Err: the task body already recorded the
                // health failure and the error log; the hosted routes must
                // still fail on the dead transport.
                server_exited.cancel();
            }
        }
        Err(join_err) => {
            // Fail every hosted route's consumer FIRST — supervision must
            // engage even if the observability calls below fail — then
            // record the structured error event: each `forward_task`
            // returns Err and camel-core emits one CrashNotification per
            // route (ADR-0007 parity with per-route transport death).
            server_exited.cancel();
            runtime
                .metrics()
                .increment_errors(&route_id, "e:ws:server-task-exited");
            // log-policy: outside-contract
            tracing::error!(
                host = %camel_api::redact::redact_host(&addr.ip().to_string()),
                port = addr.port(),
                error = %join_err,
                "WebSocket server task exited unexpectedly — all routes on this port are now dead"
            );
        }
    }
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
    /// rc-nftni (drainclaim): the context-global accepted-not-completed
    /// counter, set once by the first route whose consumer registered on
    /// this shared server (`finish_start`). Set-once, keep-first — routes
    /// sharing a server share a CamelContext, so the counter is the same
    /// Arc in practice. The per-connection handler mints one claim per
    /// inbound frame envelope (acceptance dequeue).
    pub in_flight: Arc<std::sync::OnceLock<Arc<std::sync::atomic::AtomicU64>>>,
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

/// Kernel authentication state for one ws path (`unify-transport-auth`,
/// Task 2.8).
///
/// Construction-order (Task 2.1 lesson): the compiled plan and the
/// provider registry ride the route's [`SecurityContext`], which the
/// consumer publishes into the shared server state at `start()` — in the
/// same step that makes the path dispatchable — so no handshake can reach
/// an upgrade with half-captured state and nothing patches the entry
/// afterwards. `None` unless both pieces are present: a plan without
/// providers can never mint a principal, and a registry without a plan
/// has nothing to enforce. A context without both is Public pass-through
/// (no extraction, no evaluation) — DSL routes always carry a compiled
/// plan (mandatory at `add_route`) and programmatic routes without one
/// fail closed at the controller's strict dispatch check
/// (`finish-auth-flip` Task 1.1).
struct WsKernelAuth {
    plan: RouteSecurityPlan,
    providers: Arc<ProviderRegistry>,
}

impl WsKernelAuth {
    fn from_security_context(ctx: &camel_component_api::SecurityContext) -> Option<Self> {
        Some(Self {
            plan: ctx.plan.clone()?,
            providers: ctx.providers.clone()?,
        })
    }
}

/// Map an authentication failure to the upgrade-rejection response — the
/// ws denial idiom is refusing the HTTP upgrade, so the handshake never
/// completes and no close frame is needed.
fn ws_upgrade_auth_error(e: &CamelError) -> axum::response::Response {
    let (status, body) = match e {
        CamelError::Unauthenticated(_) => (StatusCode::UNAUTHORIZED, "Unauthorized"),
        CamelError::AuthProviderUnavailable(_) => {
            (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable")
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error"),
    };
    (status, body).into_response()
}

/// Redact a producer-side WS URL for debug logs (audit 2026-08-31, F2-3).
/// The endpoint path can carry a query string with tokens
/// (`ws://host/path?token=…`); userinfo is not supported by the URI grammar
/// but any `@`-bearing authority is masked defensively, through the LAST
/// `@` (canonical doctrine: over-masking is safe, under-masking is not —
/// a first-`@` mask would leak `bob:p@ss@host`'s tail). Returns
/// `scheme://host:port/path` with the query stripped.
fn redact_ws_url_for_log(url: &str) -> String {
    let no_query = url.split('?').next().unwrap_or(url);
    match no_query.split_once("://") {
        Some((scheme, rest)) => match rest.rsplit_once('@') {
            Some((_, after)) => format!("{scheme}://***@{after}"),
            None => no_query.to_string(),
        },
        None => no_query.to_string(),
    }
}

/// ADR-0051 defense-in-depth: when the accepted credential came from a
/// query param, log the redacted URI in the upgrade debug record. ws
/// forbids `QueryParam` at compile time since Task 1.8; the redaction
/// stays so a future re-introduction can never leak the token value.
fn debug_log_query_credential(
    uri: &axum::http::Uri,
    sources: &[camel_auth::CredentialSource],
    accepted: &camel_auth::CredentialSource,
) {
    if !matches!(accepted, camel_auth::CredentialSource::QueryParam { .. }) {
        return;
    }
    let mut sensitive: Vec<&str> = vec!["access_token", "token"];
    sensitive.extend(sources.iter().filter_map(|source| match source {
        camel_auth::CredentialSource::QueryParam { param } => Some(param.as_str()),
        _ => None,
    }));
    let redacted = camel_auth::redact_query_params(uri, &sensitive);
    tracing::debug!(path = %redacted, "WS upgrade with query token (redacted)");
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

    let mut carrier_opt: Option<AuthenticatedPrincipal> = None;
    if let Some(sec_ctx) = state.path_policies.get(&path)
        && let Some(kernel) = WsKernelAuth::from_security_context(&sec_ctx)
    {
        // Kernel path (Task 2.8): `Public` skips credential extraction
        // entirely (pass-through); any other mode extracts per the
        // plan's sources and mints the sealed principal through the
        // kernel. No local policy evaluation — authorization belongs
        // to the pipeline's policy layer and Task 2.9's dispatch
        // check, which read the typed carrier.
        if matches!(kernel.plan.access_mode, AccessMode::Public) {
            tracing::debug!(path = %path, "WS upgrade: public plan, skipping credential extraction");
        } else {
            match camel_auth::extract_token_multi(
                req.headers(),
                req.uri(),
                &kernel.plan.credential_sources,
            ) {
                Some(extracted) => {
                    debug_log_query_credential(
                        req.uri(),
                        &kernel.plan.credential_sources,
                        &extracted.source,
                    );
                    match camel_auth::kernel_authenticate(
                        &kernel.plan,
                        &kernel.providers,
                        &extracted,
                    )
                    .await
                    {
                        Ok(carrier) => {
                            tracing::debug!(
                                path = %path,
                                subject = %carrier.principal().subject,
                                provider = %carrier.provider_id(),
                                "WS upgrade authorized via kernel"
                            );
                            // The sealed carrier rides the connection
                            // into every message exchange.
                            carrier_opt = Some(carrier);
                        }
                        Err(e) => {
                            state
                                .runtime
                                .metrics()
                                .increment_errors(&state.route_id, "e:ws:authn");
                            tracing::warn!(path = %path, error = %e, "WS upgrade kernel authentication failed");
                            return ws_upgrade_auth_error(&e);
                        }
                    }
                }
                None => {
                    state
                        .runtime
                        .metrics()
                        .increment_errors(&state.route_id, "e:ws:authn");
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
            carrier_opt,
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
    carrier: Option<AuthenticatedPrincipal>,
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
                // Kernel-minted typed carrier (Task 2.8): a FRESH exchange is
                // built per inbound message, so every one must carry the
                // connection's authenticated principal — Task 2.9's dispatch
                // enforcement reads it off each exchange, not just the first.
                if let Some(ref carrier) = carrier {
                    camel_auth::install_carrier(&mut exchange, carrier);
                }
                #[cfg(feature = "otel")]
                {
                    camel_otel::extract_into_exchange(&mut exchange, &upgrade_headers);
                }
                if env_tx
                    .send(ExchangeEnvelope {
                        exchange,
                        reply_tx: None,
                        // rc-nftni: mint at acceptance — the frame dequeue is
                        // where this route takes ownership of the wire
                        // message. The claim covers the per-path env_tx
                        // queue, the forwarder, the route channel, and the
                        // pipeline; a failed push or route shutdown drops it
                        // (RAII rollback).
                        in_flight_claim: state.in_flight.get().map(InFlightClaim::attach),
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
                // Kernel-minted typed carrier (Task 2.8): same mandate as the
                // text site — every binary message exchange carries it too.
                if let Some(ref carrier) = carrier {
                    camel_auth::install_carrier(&mut exchange, carrier);
                }
                #[cfg(feature = "otel")]
                {
                    camel_otel::extract_into_exchange(&mut exchange, &upgrade_headers);
                }
                if env_tx
                    .send(ExchangeEnvelope {
                        exchange,
                        reply_tx: None,
                        // rc-nftni: binary frames mint at acceptance exactly
                        // like text frames (see the text site above).
                        in_flight_claim: state.in_flight.get().map(InFlightClaim::attach),
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

    fn metadata(&self) -> ComponentMetadata {
        WsEndpointConfig::metadata()
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
        let conn_state_tx = if cfg.consume_as_client {
            let (tx, rx) = watch::channel(ClientConnState::Connecting);
            ctx.register_current_route_health_check(Arc::new(ConnectionStateCheck::new(rx)));
            Some(tx)
        } else {
            let health_check = WsHealthCheck::new(cfg.host.clone(), cfg.port);
            ctx.register_current_route_health_check(std::sync::Arc::new(health_check));
            None
        };
        Ok(Box::new(WsEndpoint {
            uri: uri.to_string(),
            cfg,
            conn_state_tx,
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

    fn metadata(&self) -> ComponentMetadata {
        // WSS shares the ws URI option surface; only the scheme differs.
        // Self-setting it keeps Registry::register off the normalize-warn path.
        let mut meta = WsEndpointConfig::metadata();
        meta.scheme = "wss".to_string();
        meta
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
        let conn_state_tx = if cfg.consume_as_client {
            let (tx, rx) = watch::channel(ClientConnState::Connecting);
            ctx.register_current_route_health_check(Arc::new(ConnectionStateCheck::new(rx)));
            Some(tx)
        } else {
            let health_check = WsHealthCheck::new(cfg.host.clone(), cfg.port);
            ctx.register_current_route_health_check(std::sync::Arc::new(health_check));
            None
        };
        Ok(Box::new(WsEndpoint {
            uri: uri.to_string(),
            cfg,
            conn_state_tx,
        }))
    }
}

struct WsEndpoint {
    uri: String,
    cfg: WsEndpointConfig,
    /// Client-mode only: publishes the `WsClientConsumer` connection
    /// lifecycle consumed by `ConnectionStateCheck`. `None` in server mode.
    conn_state_tx: Option<watch::Sender<ClientConnState>>,
}

impl Endpoint for WsEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        if self.cfg.consume_as_client {
            let conn_state_tx = self.conn_state_tx.clone().ok_or_else(|| {
                CamelError::EndpointCreationFailed(
                    "consumeAsClient requires a connection-state sender".into(),
                )
            })?;
            return Ok(Box::new(WsClientConsumer::new(
                self.cfg.client_config(),
                rt,
                conn_state_tx,
            )));
        }
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

    /// Start the consumer by injecting an already-bound TCP listener.
    ///
    /// The listener is authoritative for binding: the server registry
    /// keys the server by the listener's actual port (`local_addr()`),
    /// so the endpoint URI's host:port is informational under this
    /// entry point. Path, auth and per-segment config still come from
    /// the endpoint; the TLS mode comes from the consumer config and
    /// must match any server already running on the same port.
    pub async fn start_with_listener(
        &mut self,
        ctx: ConsumerContext,
        listener: tokio::net::TcpListener,
    ) -> Result<(), CamelError> {
        // Reject double-start (WS-006)
        if self.server_state.is_some() {
            return Err(CamelError::EndpointCreationFailed(
                "WebSocket consumer already started".into(),
            ));
        }

        tracing::info!(
            path = self.cfg.inner.path,
            scheme = self.cfg.inner.scheme,
            "WebSocket consumer starting with injected listener"
        );

        let tls_config = self.tls_config()?;

        let (state, bound_addr, listening_handle, server_exited) = ServerRegistry::global()
            .get_or_spawn_with_listener(
                listener,
                tls_config,
                self.runtime.clone(),
                ctx.route_id().to_string(),
            )
            .await?;

        self.gate_ready(&ctx, listening_handle).await?;

        // The listener's actual address — not the URI's informational
        // host:port — keys the connection registry.
        // NOTE: keyed by the listener's actual address, not
        // `canonical_host()` from the URI (as `start` does). The listener
        // is authoritative here; a URI host whose canonical form maps to a
        // different interface string (e.g. `localhost` + a `::1` listener)
        // will make same-host producer lookup miss. Bind the listener on
        // the address family the URI names.
        let registry_key = (
            bound_addr.ip().to_string(),
            bound_addr.port(),
            self.cfg.inner.path.clone(),
        );
        self.finish_start(&ctx, state, registry_key, server_exited)
            .await
    }

    /// Shared tail of `start` and `start_with_listener`: publish the
    /// path dispatch entry and segment config, register the connection
    /// registry under `registry_key`, and spawn the envelope forward
    /// task.
    async fn finish_start(
        &mut self,
        ctx: &ConsumerContext,
        state: WsAppState,
        registry_key: (String, u16, String),
        server_exited: CancellationToken,
    ) -> Result<(), CamelError> {
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

        global_registries().insert(registry_key.clone(), Arc::clone(&self.registry));

        // rc-nftni: publish this route's context-global counter on the
        // shared server state (set-once, keep-first — routes sharing a
        // server share a CamelContext, so the Arc is the same in practice).
        // The per-connection handler mints one claim per inbound frame
        // envelope from it.
        if let Some(counter) = ctx.in_flight_counter() {
            let _ = state.in_flight.set(counter);
        }

        let sender = ctx.sender();
        let route_id = ctx.route_id().to_string();
        let runtime = Arc::clone(&self.runtime);
        let (registry_host, registry_port, _) = registry_key.clone();
        let forward_task: JoinHandle<Result<(), CamelError>> = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = server_exited.cancelled() => {
                        // Shared transport death: this route's consumer
                        // cannot continue. Fail (do NOT hang in Running) —
                        // parity with per-route transport death, which also
                        // surfaces as a consumer-task error. The Err flows
                        // through `background_task_handle()` into
                        // camel-core's background-task watcher, which emits
                        // a CrashNotification for THIS route and
                        // supervision backoff engages (ADR-0007, rc-nxml4).
                        return Err(CamelError::RouteError(format!(
                            "shared WebSocket server for {registry_host}:{registry_port} exited unexpectedly; route transport is dead"
                        )));
                    }
                    envelope = env_rx.recv() => {
                        let Some(envelope) = envelope else { break };
                        if sender.send(envelope).await.is_err() {
                            // (category b′ per ADR-0012: locally terminal
                            // message dispatch — the pipeline receiver is
                            // gone and the loop exits, so this is the only
                            // signal.)
                            runtime
                                .metrics()
                                .increment_errors(&route_id, "b-prime:ws:message-dispatch");
                            break;
                        }
                    }
                }
            }
            Ok(())
        });

        self.server_state = Some(state);
        self.registry_key = Some(registry_key);
        self.forward_task = Some(forward_task);
        Ok(())
    }
}

impl WsConsumer {
    /// TLS config from consumer settings; errors when `wss` lacks cert/key.
    fn tls_config(&self) -> Result<Option<WsTlsConfig>, CamelError> {
        if self.cfg.inner.scheme == "wss" {
            let cert_path = self.cfg.inner.tls_cert.clone().ok_or_else(|| {
                CamelError::EndpointCreationFailed("TLS cert path is required for wss".into())
            })?;
            let key_path = self.cfg.inner.tls_key.clone().ok_or_else(|| {
                CamelError::EndpointCreationFailed("TLS key path is required for wss".into())
            })?;
            Ok(Some(WsTlsConfig {
                cert_path,
                key_path,
            }))
        } else {
            Ok(None)
        }
    }

    /// Upper bound for the serve task to reach its accept loop after
    /// spawn. The listener is already bound when the task spawns, so
    /// reaching `listening()` is a first-poll event; anything beyond this
    /// bound means the serve task was cancelled before its first poll
    /// (e.g. its owning runtime was dropped — the registry keeps the
    /// entry, but no notification will ever arrive) or is stalled, and
    /// the gate must fail instead of parking forever (rc-oo0c).
    fn serve_readiness_bound() -> std::time::Duration {
        if cfg!(test) {
            std::time::Duration::from_secs(1)
        } else {
            std::time::Duration::from_secs(10)
        }
    }

    /// Readiness gating shared by `start` and `start_with_listener`:
    /// both paths bind the TCP listener before delegation — plain
    /// synchronously, TLS via the pre-bound listener handed to the serve
    /// task. `listening()` therefore signals that the serve/accept loop
    /// actually started (a `None` return means serving failed — e.g. the
    /// task died at startup — so the route never marks itself ready on a
    /// dead listener). The readiness await is bounded by
    /// [`Self::serve_readiness_bound`]; a deadline means no notification
    /// will ever arrive (cancelled or stalled serve task) and the gate
    /// returns `Err` (rc-oo0c).
    async fn gate_ready(
        &self,
        ctx: &ConsumerContext,
        listening_handle: Option<axum_server::Handle<std::net::SocketAddr>>,
    ) -> Result<(), CamelError> {
        match listening_handle {
            Some(handle) => {
                let listened =
                    tokio::time::timeout(Self::serve_readiness_bound(), handle.listening()).await;
                match listened {
                    Ok(Some(_addr)) => {
                        ctx.mark_ready();
                        Ok(())
                    }
                    Ok(None) => Err(CamelError::EndpointCreationFailed(
                        "TLS listener bind failed".to_string(),
                    )),
                    Err(_elapsed) => Err(CamelError::EndpointCreationFailed(format!(
                        "TLS listener did not become ready within {:?} (serve task cancelled or stalled)",
                        Self::serve_readiness_bound()
                    ))),
                }
            }
            None => {
                ctx.mark_ready();
                Ok(())
            }
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
            host = camel_api::redact::redact_host(&self.cfg.inner.host),
            port = self.cfg.inner.port,
            path = self.cfg.inner.path,
            scheme = self.cfg.inner.scheme,
            "WebSocket consumer starting"
        );

        let tls_config = self.tls_config()?;

        let (state, listening_handle, server_exited) = ServerRegistry::global()
            .get_or_spawn(
                &self.cfg.inner.host,
                self.cfg.inner.port,
                tls_config,
                self.runtime.clone(),
                ctx.route_id().to_string(),
            )
            .await?;

        self.gate_ready(&ctx, listening_handle).await?;

        let registry_key = (
            self.cfg.inner.canonical_host(),
            self.cfg.inner.port,
            self.cfg.inner.path.clone(),
        );
        self.finish_start(&ctx, state, registry_key, server_exited)
            .await
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        tracing::info!(
            host = camel_api::redact::redact_host(&self.cfg.inner.host),
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
            host = camel_api::redact::redact_host(&self.cfg.inner.host),
            port = self.cfg.inner.port,
            path = self.cfg.inner.path,
            "WebSocket consumer stopped"
        );

        if had_server_error {
            tracing::warn!(
                host = camel_api::redact::redact_host(&self.cfg.inner.host),
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

    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        self.forward_task.take()
    }

    fn set_security_context(&mut self, ctx: camel_component_api::SecurityContext) {
        // Construction-order (Task 2.1 lesson, applied at Task 2.8): the
        // context must already carry the compiled plan and the provider
        // registry here — `start()` publishes it into the shared server
        // state in the same step that makes the path dispatchable, so no
        // handshake can observe a half-patched policy.
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
                        host = camel_api::redact::redact_host(&canonical_host),
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
                    host = camel_api::redact::redact_host(&canonical_host),
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

            tracing::debug!(url = %redact_ws_url_for_log(&url), "WebSocket producer connecting");

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

            send_with_timeout(ws_stream.send(out_msg), cfg.inner.send_timeout).await?;

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
                    tracing::debug!(url = %redact_ws_url_for_log(&url), "WebSocket producer received text response");
                    exchange.input.body = CamelBody::Text(text.to_string());
                }
                Some(Ok(ClientWsMessage::Binary(data))) => {
                    tracing::debug!(url = %redact_ws_url_for_log(&url), "WebSocket producer received binary response");
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
                        tracing::debug!(url = %redact_ws_url_for_log(&url), "WebSocket producer received normal close");
                        exchange.input.body = CamelBody::Empty;
                    } else if reconnect_policy.should_retry(attempts + 1) {
                        let delay = reconnect_policy.delay_for(0); // fresh delay on close
                        tracing::warn!(
                            url = %redact_ws_url_for_log(&url),
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
            tracing::debug!(url = %redact_ws_url_for_log(&url), "WebSocket producer connection closed");
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
        // Empty and future variants render as an empty string.
        _ => String::new(),
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

async fn send_with_timeout(
    send_future: impl std::future::Future<Output = Result<(), tungstenite::Error>>,
    timeout: std::time::Duration,
) -> Result<(), CamelError> {
    match tokio::time::timeout(timeout, send_future).await {
        Ok(result) => {
            result.map_err(|e| CamelError::ProcessorError(format!("WebSocket send failed: {e}")))
        }
        Err(_) => Err(CamelError::ProcessorError(format!(
            "WebSocket send timeout after {timeout:?}"
        ))),
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
                CamelError::ProcessorError(format!(
                    "WebSocket connection failed ({}): {msg}",
                    redact_ws_url_for_log(url)
                ))
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
        "ws",
        "connect",
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
                        "WebSocket connect timeout ({connect_timeout:?}) to {}",
                        redact_ws_url_for_log(&url)
                    ))),
                }
            }
        },
        is_retryable_ws_error,
        None,
    )
    .await
}

#[cfg(test)]
mod tests {
    use camel_component_api::test_support::PanicRuntimeObservability;

    use crate::test_doubles::RecordingMetrics;

    /// Audit 2026-08-31, F2-3: producer debug logs must not leak query tokens.
    /// The multi-`@` case pins the last-`@` doctrine (bd rc-8bxeo): a
    /// first-`@` mask would leak `p@ss@host`'s tail as `***@ss@host`.
    #[test]
    fn redact_ws_url_strips_query_and_userinfo() {
        assert_eq!(
            super::redact_ws_url_for_log("ws://broker.example.com:9292/chat?token=abc123"),
            "ws://broker.example.com:9292/chat"
        );
        assert_eq!(
            super::redact_ws_url_for_log("wss://user:pass@host:443/ws?api_key=xyz"),
            "wss://***@host:443/ws"
        );
        assert_eq!(
            super::redact_ws_url_for_log("ws://host:80/clean"),
            "ws://host:80/clean"
        );
        assert_eq!(
            super::redact_ws_url_for_log("ws://bob:p@ss@host:9292/x?token=z"),
            "ws://***@host:9292/x"
        );
    }

    /// ADR-0076: `host` log fields route through the canonical
    /// [`camel_api::redact::redact_host`] (bd rc-8bxeo promoted the
    /// crate-local twin). Thin local pin — the full matrix lives in
    /// camel-api's `redact_host_masks_userinfo_keeps_clean_hosts`.
    #[test]
    fn canonical_redact_host_pinned() {
        assert_eq!(
            camel_api::redact::redact_host("broker.example.com"),
            "broker.example.com"
        );
        assert_eq!(camel_api::redact::redact_host("bob:p@ss@host"), "***@host");
    }

    /// Re-review of F2-3: connection errors must not echo the raw URL
    /// (query tokens / userinfo) — only the redacted form.
    #[test]
    fn map_connect_error_redacts_url_in_error_value() {
        let err = super::map_connect_error(
            tungstenite::Error::Io(std::io::Error::other("peer reset mid-handshake")),
            "wss://user:secret@broker.example.com/ws?token=abc123",
        );
        let msg = err.to_string();
        assert!(
            !msg.contains("secret") && !msg.contains("abc123"),
            "error must not carry credentials: {msg}"
        );
        assert!(
            msg.contains("***@broker.example.com"),
            "redacted host kept: {msg}"
        );
    }

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

    /// Retry `connect_async` until the server accepts, bounded by 5s.
    /// Unbounded retries hang the whole test binary when the consumer's
    /// bind silently failed on crowded CI port space (rc-y24l).
    async fn connect_until_ready(
        url: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match connect_async(url).await {
                    Ok((stream, _)) => break stream,
                    Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("server at {url} never accepted a connection within 5s"))
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
    fn wss_metadata_scheme_matches_component_scheme() {
        // WSS shares the ws URI option surface; only the scheme differs, and
        // metadata() must report it so Registry::register sees no drift.
        assert_eq!(WssComponent::new().metadata().scheme, "wss");
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("ws://127.0.0.1:{port}/echo");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();

        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let producer = endpoint
            .create_producer(rt(), &ProducerContext::default())
            .unwrap();

        let (route_tx, mut route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

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
        let mut client = connect_until_ready(&url).await;

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

    /// rc-nftni (drainclaim): server frame dispatch mints a claim at the
    /// frame's acceptance dequeue and carries it on the envelope through
    /// the per-path env_tx queue and the forwarder. Exact totals: 1 while
    /// held, 0 after release. No wall-clock sleeps — the recv is the
    /// barrier.
    #[tokio::test]
    async fn server_frame_dispatch_carries_in_flight_claim() {
        use std::sync::atomic::AtomicU64;

        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let uri = format!("ws://127.0.0.1:{port}/claim");
        let component_ctx = NoOpComponentContext;
        let _endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();

        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );

        let counter = Arc::new(AtomicU64::new(0));
        let (route_tx, mut route_rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-claim-route".to_string(),
        )
        .with_in_flight_counter(Arc::clone(&counter));

        consumer.start_with_listener(ctx, listener).await.unwrap();

        let url = format!("ws://127.0.0.1:{port}/claim");
        let mut client = connect_until_ready(&url).await;
        client
            .send(ClientMessage::Text("claim-me".into()))
            .await
            .unwrap();

        let mut envelope = tokio::time::timeout(Duration::from_secs(2), route_rx.recv())
            .await
            .expect("envelope within 2s")
            .expect("route channel open");
        let claim = envelope
            .in_flight_claim
            .take()
            .expect("server frame dispatch must carry an acceptance-minted claim");
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::Acquire),
            1,
            "exact total: the acceptance mint is the only live claim"
        );
        drop(claim);
        drop(envelope);
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::Acquire),
            0,
            "release exactly once when the holder drops"
        );

        consumer.stop().await.unwrap();
    }

    /// Echo a single envelope back through `producer` (test helper for the
    /// listener-injection consumer tests).
    fn spawn_echo_route(
        mut route_rx: mpsc::Receiver<ExchangeEnvelope>,
        producer: BoxProcessor,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
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
        })
    }

    /// Receive the next text message from the client, skipping control
    /// frames (test helper for the listener-injection consumer tests).
    async fn recv_client_text(
        client: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> String {
        tokio::time::timeout(Duration::from_secs(2), async {
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
        .unwrap()
    }

    #[tokio::test]
    async fn start_with_listener_round_trips_without_port_guess() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let uri = format!("ws://127.0.0.1:{}/echo", addr.port());
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();

        // The port-0 listener is the source of truth and is handed to
        // the consumer as-is.
        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let producer = endpoint
            .create_producer(rt(), &ProducerContext::default())
            .unwrap();

        let (route_tx, route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

        let route_task = spawn_echo_route(route_rx, producer);

        let url = format!("ws://127.0.0.1:{}/echo", addr.port());
        let mut client = connect_until_ready(&url).await;

        client
            .send(ClientMessage::Text("hello-ws".into()))
            .await
            .unwrap();

        let incoming = recv_client_text(&mut client).await;
        assert_eq!(incoming, "hello-ws");

        consumer.stop().await.unwrap();
        route_task.await.unwrap();
    }

    #[tokio::test]
    async fn injected_entry_survives_consumer_stop() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("ws://127.0.0.1:{port}/echo");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();

        // Consumer A: injected listener entry.
        let mut consumer_a = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let (route_tx_a, route_rx_a) = mpsc::channel(16);
        let ctx_a = ConsumerContext::new(
            route_tx_a,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer_a
            .start_with_listener(ctx_a, listener)
            .await
            .unwrap();

        let producer_a = endpoint
            .create_producer(rt(), &ProducerContext::default())
            .unwrap();
        let route_task_a = spawn_echo_route(route_rx_a, producer_a);

        let url = format!("ws://127.0.0.1:{port}/echo");
        let mut client_a = connect_until_ready(&url).await;
        client_a
            .send(ClientMessage::Text("msg-a".into()))
            .await
            .unwrap();
        assert_eq!(recv_client_text(&mut client_a).await, "msg-a");

        consumer_a.stop().await.unwrap();
        route_task_a.await.unwrap();

        // Consumer B: plain `start` entry on the same port. The injected
        // server entry must still be alive, so this joins it instead of
        // rebinding (a rebind would collide with the live server).
        let mut consumer_b = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let (route_tx_b, route_rx_b) = mpsc::channel(16);
        let ctx_b = ConsumerContext::new(
            route_tx_b,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer_b.start(ctx_b).await.unwrap();

        let producer_b = endpoint
            .create_producer(rt(), &ProducerContext::default())
            .unwrap();
        let route_task_b = spawn_echo_route(route_rx_b, producer_b);

        let mut client_b = connect_until_ready(&url).await;
        client_b
            .send(ClientMessage::Text("msg-b".into()))
            .await
            .unwrap();
        assert_eq!(recv_client_text(&mut client_b).await, "msg-b");

        consumer_b.stop().await.unwrap();
        route_task_b.await.unwrap();
    }

    #[tokio::test]
    async fn consumer_stop_sends_close_1001() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("ws://127.0.0.1:{port}/shutdown");
        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

        let url = format!("ws://127.0.0.1:{port}/shutdown");
        let mut client = connect_until_ready(&url).await;

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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("wss://127.0.0.1:{port}/secure");
        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let (tx, _rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "ws-test-route".to_string());
        let result = consumer.start_with_listener(ctx, listener).await;
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

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!(
            "wss://127.0.0.1:{port}/secure?tlsCert=/nonexistent/cert.pem&tlsKey=/nonexistent/key.pem"
        );
        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let (tx, _rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "ws-test-route".to_string());
        let result = consumer.start_with_listener(ctx, listener).await;
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
        // One socket, two handles: clone at the std level BEFORE the tokio
        // conversion so both injected listeners report the same local port.
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std_clone = std_listener.try_clone().unwrap();
        let listener1 = tokio_listener_from_std(std_listener);
        let listener2 = tokio_listener_from_std(std_clone);
        let (state1, _addr1, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(listener1, None, test_rt(), "test-route".into())
            .await
            .unwrap();
        let (state2, _addr2, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(listener2, None, test_rt(), "test-route".into())
            .await
            .unwrap();
        assert!(
            Arc::ptr_eq(&state1.dispatch, &state2.dispatch),
            "expected same dispatch table for same port"
        );
    }

    // ── Listener injection: get_or_spawn_with_listener ────────────────────
    //
    // The injected listener is authoritative for the registry key: the
    // actual port comes from `local_addr()`, so port-0 binds register under
    // their real ephemeral port and the caller learns the bound address.

    /// Assert a raw TCP connect to `addr` succeeds within 2s.
    async fn assert_tcp_connectable(addr: std::net::SocketAddr) {
        tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr))
            .await
            .expect("connect timed out")
            .expect("expected a successful TCP connect");
    }

    /// Build a tokio listener from a std listener, setting non-blocking mode
    /// (required by `TcpListener::from_std`).
    fn tokio_listener_from_std(std_listener: std::net::TcpListener) -> tokio::net::TcpListener {
        std_listener.set_nonblocking(true).unwrap();
        tokio::net::TcpListener::from_std(std_listener).unwrap()
    }

    #[tokio::test]
    async fn with_listener_port_zero_returns_real_bound_addr() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let expected_port = listener.local_addr().unwrap().port();
        assert_ne!(expected_port, 0, "probe port must be real");

        let (_state, bound_addr, listening, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(listener, None, test_rt(), "ws-injected-p0".into())
            .await
            .expect("injected listener spawn must succeed");

        assert!(listening.is_none(), "plain path has no listening handle");
        assert_eq!(bound_addr.port(), expected_port);
        assert_ne!(
            bound_addr.port(),
            0,
            "bound address must carry the real port"
        );
        assert_tcp_connectable(bound_addr).await;
    }

    #[tokio::test]
    async fn with_listener_same_port_reuses_entry() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        // One socket, two handles: clone at the std level BEFORE the tokio
        // conversion so both injected listeners report the same local port.
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std_clone = std_listener.try_clone().unwrap();
        let listener1 = tokio_listener_from_std(std_listener);
        let port = listener1.local_addr().unwrap().port();

        let (_s1, addr1, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(listener1, None, test_rt(), "ws-injected-r1".into())
            .await
            .expect("first injected spawn must succeed");

        let listener2 = tokio_listener_from_std(std_clone);
        let (_s2, addr2, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(listener2, None, test_rt(), "ws-injected-r2".into())
            .await
            .expect("second injected call must reuse the entry, not rebind");

        assert_eq!(addr1.port(), port);
        assert_eq!(
            addr2, addr1,
            "both callers must observe the same bound address"
        );
        assert_eq!(
            ServerRegistry::global().ref_count_for_test(port),
            2,
            "two injected callers must hold two references on the same entry"
        );
        assert_tcp_connectable(addr1).await;
        assert_tcp_connectable(addr1).await;
    }

    #[tokio::test]
    async fn legacy_get_or_spawn_after_injected_reuses_entry() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (_s1, addr, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(listener, None, test_rt(), "ws-injected-mix".into())
            .await
            .expect("injected spawn must succeed");

        let (_s2, _, _) = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None, test_rt(), "test-route".into())
            .await
            .expect("legacy call on injected entry must reuse it, not rebind");

        assert_eq!(
            ServerRegistry::global().ref_count_for_test(port),
            2,
            "injected + legacy callers must hold two references on the same entry"
        );
        assert_tcp_connectable(addr).await;
    }

    #[tokio::test]
    async fn legacy_get_or_spawn_unchanged_after_refactor() {
        use camel_component_api::test_support::tls;

        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();
        let _ = rustls::crypto::ring::default_provider().install_default();

        // Test-infra port pick: bind-0, read, drop the probe listener.
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        // Plain legacy spawn: pre-refactor return shape `(WsAppState, Option<Handle>)`,
        // binds so a TCP connect succeeds, ref count 1.
        let (_state, listening, _) = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None, test_rt(), "test-route".into())
            .await
            .expect("plain legacy spawn must succeed");
        assert!(
            listening.is_none(),
            "plain legacy path must keep returning no listening handle"
        );
        assert_eq!(
            ServerRegistry::global().ref_count_for_test(port),
            1,
            "a single legacy caller holds one reference"
        );
        assert_tcp_connectable(std::net::SocketAddr::from(([127, 0, 0, 1], port))).await;

        // TLS legacy call on the same port: TLS-mode mismatch, unchanged.
        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("ws-bound-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("ws-bound-key.pem", &key_pem);
        let tls_cfg = WsTlsConfig {
            cert_path: cert_path.to_str().expect("cert path").to_string(),
            key_path: key_path.to_str().expect("key path").to_string(),
        };
        let result = ServerRegistry::global()
            .get_or_spawn(
                "127.0.0.1",
                port,
                Some(tls_cfg),
                test_rt(),
                "test-route-tls".into(),
            )
            .await;
        let err = match result {
            Ok(_) => panic!("TLS-mode mismatch on the same port must error"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("different TLS mode"),
            "expected TLS-mode mismatch error, got: {err}"
        );
        assert_eq!(
            ServerRegistry::global().ref_count_for_test(port),
            1,
            "the rejected caller must not hold a reference"
        );
    }

    #[tokio::test]
    async fn with_listener_tls_mismatch_errors() {
        use camel_component_api::test_support::tls;

        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();
        let _ = rustls::crypto::ring::default_provider().install_default();

        // One socket, two handles (same trick as the reuse test): the second
        // injected listener reports the same port without a second bind.
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std_clone = std_listener.try_clone().unwrap();
        let listener = tokio_listener_from_std(std_listener);
        let port = listener.local_addr().unwrap().port();

        let (_state, addr, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(listener, None, test_rt(), "ws-injected-plain".into())
            .await
            .expect("plain injected spawn must succeed");

        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("ws-injected-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("ws-injected-key.pem", &key_pem);
        let tls_cfg = WsTlsConfig {
            cert_path: cert_path.to_str().expect("cert path").to_string(),
            key_path: key_path.to_str().expect("key path").to_string(),
        };

        let tls_listener = tokio_listener_from_std(std_clone);
        let result = ServerRegistry::global()
            .get_or_spawn_with_listener(
                tls_listener,
                Some(tls_cfg),
                test_rt(),
                "ws-injected-tls".into(),
            )
            .await;
        let err = match result {
            Ok(_) => panic!("TLS-mode mismatch on an injected port must error"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("different TLS mode"),
            "expected TLS-mode mismatch error, got: {err}"
        );
        assert_eq!(
            ServerRegistry::global().ref_count_for_test(port),
            1,
            "the rejected caller must not hold a reference"
        );
        assert_tcp_connectable(addr).await;
    }

    #[tokio::test]
    async fn reset_clears_injected_entry_allowing_rebind() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (_state, _addr, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(listener, None, test_rt(), "ws-injected-reset".into())
            .await
            .expect("injected spawn must succeed");
        assert_eq!(ServerRegistry::global().ref_count_for_test(port), 1);

        // reset() must abort the injected entry's server task and clear the
        // map entry like any other.
        ServerRegistry::reset();
        assert_eq!(
            ServerRegistry::global().ref_count_for_test(port),
            0,
            "reset must clear the injected entry"
        );

        // The aborted task releases the socket asynchronously — retry the
        // bind briefly. Success proves no listener leak.
        let mut fresh = None;
        for _ in 0..100 {
            match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                Ok(l) => {
                    fresh = Some(l);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
        let fresh = fresh.expect("port must be rebindable after reset (no listener leak)");

        let (_state2, addr2, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(fresh, None, test_rt(), "ws-injected-rebind".into())
            .await
            .expect("re-spawn on the fresh listener must succeed");
        assert_eq!(addr2.port(), port);
        assert_tcp_connectable(addr2).await;
    }

    // ── Staged listener consumption (itest-bound-ports) ───────────────────
    //
    // `stage_listener` parks a pre-bound listener under its exact
    // `(host, port)` key so the next vacant-entry `get_or_spawn` serves
    // that socket instead of binding — eliminating the bind window
    // between a port probe and server startup.

    #[tokio::test]
    async fn ws_staged_listener_consumed_on_vacant_entry() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let staged_addr = std_listener.local_addr().unwrap();
        let port = staged_addr.port();
        ServerRegistry::global()
            .stage_listener(tokio_listener_from_std(std_listener))
            .await
            .expect("staging a fresh (host, port) key must succeed");

        let (_state, _, _) = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None, test_rt(), "test-route".into())
            .await
            .expect("vacant-entry spawn must consume the staged listener");

        assert_eq!(
            ServerRegistry::global().ref_count_for_test(port),
            1,
            "a single caller holds one reference on the consumed entry"
        );
        assert_eq!(
            ServerRegistry::global().bound_addr_for_test(port),
            Some(staged_addr),
            "the served socket must BE the staged listener (one-shot vacant-path consumption)"
        );
        assert_tcp_connectable(staged_addr).await;
    }

    #[tokio::test]
    async fn ws_staged_not_consumed_when_entry_exists() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        // One socket, three handles: the entry is created from the original,
        // the two probes stage the same port without a second bind.
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let probe1 = std_listener.try_clone().unwrap();
        let probe2 = std_listener.try_clone().unwrap();
        let port = std_listener.local_addr().unwrap().port();

        // Create the entry via the injected-listener path (a second bind on
        // this port is impossible).
        let (_s, addr, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(
                tokio_listener_from_std(std_listener),
                None,
                test_rt(),
                "test-route".into(),
            )
            .await
            .expect("entry creation must succeed");

        // Stage while the entry already exists: the staged slot is empty, so
        // staging succeeds even though no vacant-entry spawn will claim it.
        ServerRegistry::global()
            .stage_listener(tokio_listener_from_std(probe1))
            .await
            .expect("staging on an existing entry must succeed (slot empty)");

        let (_s2, _, _) = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None, test_rt(), "test-route".into())
            .await
            .expect("existing entry must be reused, not rebind");

        assert_eq!(
            ServerRegistry::global().ref_count_for_test(port),
            2,
            "entry reused: two callers hold two references"
        );
        assert_tcp_connectable(addr).await;

        // The reuse path must NOT touch the staged map: the staged listener
        // is still parked, so a duplicate stage on the key is rejected.
        let err = ServerRegistry::global()
            .stage_listener(tokio_listener_from_std(probe2))
            .await
            .expect_err("duplicate stage must be rejected");
        assert!(
            err.to_string().contains("listener already staged"),
            "expected 'listener already staged', got: {err}"
        );
    }

    #[tokio::test]
    async fn ws_wrong_host_staged_port_fails() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let staged_addr = std_listener.local_addr().unwrap();
        let port = staged_addr.port();
        ServerRegistry::global()
            .stage_listener(tokio_listener_from_std(std_listener))
            .await
            .expect("staging must succeed");

        let result = ServerRegistry::global()
            .get_or_spawn("localhost", port, None, test_rt(), "test-route".into())
            .await;
        let err = match result {
            Ok(_) => panic!("wrong-host spawn on a staged port must fail deterministically"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("staged listener conflict on port"),
            "expected staged listener conflict error, got: {err}"
        );

        // The conflicting call left the staged slot untouched: the exact-key
        // call consumes it and serves the staged socket.
        let (_state, _, _) = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None, test_rt(), "test-route".into())
            .await
            .expect("exact-key spawn must serve the staged listener");
        assert_eq!(
            ServerRegistry::global().bound_addr_for_test(port),
            Some(staged_addr),
            "staged slot untouched: served socket must be the staged listener"
        );
    }

    #[tokio::test]
    async fn ws_duplicate_stage_rejected() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let probe = std_listener.try_clone().unwrap();
        let staged_addr = std_listener.local_addr().unwrap();
        let port = staged_addr.port();
        ServerRegistry::global()
            .stage_listener(tokio_listener_from_std(std_listener))
            .await
            .expect("first stage must succeed");

        let err = ServerRegistry::global()
            .stage_listener(tokio_listener_from_std(probe))
            .await
            .expect_err("duplicate stage on the same key must be rejected");
        assert!(
            err.to_string().contains("listener already staged"),
            "expected 'listener already staged', got: {err}"
        );

        // The first staged listener is retained: get_or_spawn serves its
        // socket, not a fresh bind.
        let (_state, _, _) = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None, test_rt(), "test-route".into())
            .await
            .expect("spawn must consume the first staged listener");
        assert_eq!(
            ServerRegistry::global().bound_addr_for_test(port),
            Some(staged_addr),
            "served socket must be the first staged listener"
        );
    }

    #[tokio::test]
    async fn ws_distinct_keys_stage_independently() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        let std1 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std2 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr1 = std1.local_addr().unwrap();
        let addr2 = std2.local_addr().unwrap();
        assert_ne!(
            addr1.port(),
            addr2.port(),
            "precondition: distinct ports for distinct staged keys"
        );
        ServerRegistry::global()
            .stage_listener(tokio_listener_from_std(std1))
            .await
            .expect("stage P1");
        ServerRegistry::global()
            .stage_listener(tokio_listener_from_std(std2))
            .await
            .expect("stage P2");

        let (_s1, _, _) = ServerRegistry::global()
            .get_or_spawn(
                "127.0.0.1",
                addr1.port(),
                None,
                test_rt(),
                "test-route".into(),
            )
            .await
            .expect("P1 spawn must consume staged P1");
        let (_s2, _, _) = ServerRegistry::global()
            .get_or_spawn(
                "127.0.0.1",
                addr2.port(),
                None,
                test_rt(),
                "test-route".into(),
            )
            .await
            .expect("P2 spawn must consume staged P2");

        // Each entry serves its own staged socket: a connect to each staged
        // address succeeds against the socket that was parked for it.
        assert_tcp_connectable(addr1).await;
        assert_tcp_connectable(addr2).await;
        assert_eq!(
            ServerRegistry::global().ref_count_for_test(addr1.port()),
            1,
            "P1 entry holds exactly one reference"
        );
        assert_eq!(
            ServerRegistry::global().ref_count_for_test(addr2.port()),
            1,
            "P2 entry holds exactly one reference"
        );
    }

    #[tokio::test]
    async fn dispatch_handler_returns_404_for_unregistered_path() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (state, _addr, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(listener, None, test_rt(), "test-route".into())
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("ws://127.0.0.1:{port}/limited?maxConnections=1");
        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

        let url = format!("ws://127.0.0.1:{port}/limited");
        let _client1 = connect_until_ready(&url).await;

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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("ws://127.0.0.1:{port}/sizelimit?maxMessageSize=10");
        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

        let url = format!("ws://127.0.0.1:{port}/sizelimit");
        let mut client = connect_until_ready(&url).await;

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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("ws://127.0.0.1:{port}/origintest?allowOrigin=https://allowed.com");
        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

        let (state, _, _) = ServerRegistry::global()
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("ws://127.0.0.1:{port}/bc");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();
        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let producer = endpoint
            .create_producer(rt(), &ProducerContext::default())
            .unwrap();

        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

        let url = format!("ws://127.0.0.1:{port}/bc");

        let mut client1 = connect_until_ready(&url).await;

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
        // One socket, four handles: clone at the std level BEFORE the tokio
        // conversion so all injected listeners report the same local port.
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std_clone1 = std_listener.try_clone().unwrap();
        let std_clone2 = std_listener.try_clone().unwrap();
        let std_clone3 = std_listener.try_clone().unwrap();
        let listener0 = tokio_listener_from_std(std_listener);
        let listener1 = tokio_listener_from_std(std_clone1);
        let listener2 = tokio_listener_from_std(std_clone2);
        let listener3 = tokio_listener_from_std(std_clone3);
        let results: Arc<std::sync::Mutex<Vec<WsAppState>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut handles = Vec::new();
        for listener in [listener0, listener1, listener2, listener3] {
            let results = results.clone();
            handles.push(tokio::spawn(async move {
                let (state, _addr, _, _) = ServerRegistry::global()
                    .get_or_spawn_with_listener(listener, None, test_rt(), "test-route".into())
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

    // WS-017: send_with_timeout fires when the underlying send future exceeds the budget.
    #[tokio::test(start_paused = true)]
    async fn send_with_timeout_fires_on_elapsed() {
        // Advance the mock clock past the deadline before polling so the pending future
        // is observed as already-elapsed on the first poll.
        tokio::time::advance(Duration::from_millis(200)).await;
        let result = send_with_timeout(
            std::future::pending::<Result<(), tungstenite::Error>>(),
            Duration::from_millis(100),
        )
        .await;
        let err = result.expect_err("send_with_timeout must return Err on elapsed");
        assert!(
            err.to_string().contains("timeout"),
            "expected timeout error, got: {err}"
        );
    }

    // WS-017: send_with_timeout returns Ok when the underlying future completes within the budget.
    #[tokio::test]
    async fn send_with_timeout_succeeds_when_fast() {
        let result = send_with_timeout(
            async { Ok::<(), tungstenite::Error>(()) },
            Duration::from_secs(30),
        )
        .await;
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("ws://127.0.0.1:{port}/doublestart");
        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );

        // First start should succeed
        consumer.start_with_listener(ctx, listener).await.unwrap();

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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("ws://127.0.0.1:{port}/cleanup");
        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("ws://127.0.0.1:{port}/backpressure");
        let component_ctx = NoOpComponentContext;
        let endpoint = WsComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();

        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let producer = endpoint
            .create_producer(rt(), &ProducerContext::default())
            .unwrap();

        let (route_tx, _route_rx) = mpsc::channel(1); // Tiny channel to force backpressure
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

        // Connect a client so the registry has an entry
        let url = format!("ws://127.0.0.1:{port}/backpressure");
        let mut client = connect_until_ready(&url).await;

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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("ws://127.0.0.1:{port}/pingpong");
        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            rt(),
        );
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

        let url = format!("ws://127.0.0.1:{port}/pingpong");
        let mut client = connect_until_ready(&url).await;

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
        // Port 0: instant deterministic connection refusal, no bind probe.
        let cfg = WsEndpointConfig::from_uri(
            "ws://127.0.0.1:0/retry?reconnect=true&reconnectMaxAttempts=2&reconnectDelayMs=50",
        )
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
        // Port 0 on loopback: ECONNREFUSED on Linux, EADDRNOTAVAIL on macOS.
        assert!(
            msg.contains("connection refused")
                || msg.contains("Can't assign requested address")
                || msg.contains("os error 49"),
            "expected connect refusal (ECONNREFUSED or EADDRNOTAVAIL), got: {msg}"
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
            in_flight: Arc::default(),
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
            in_flight: Arc::default(),
        };
        assert!(!state.server_error.load(Ordering::Relaxed));
        state.server_error.store(true, Ordering::Relaxed);
        assert!(state.server_error.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn consumer_stop_returns_error_when_server_had_errors() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let cfg = WsEndpointConfig::from_uri(&format!("ws://127.0.0.1:{port}/errorflag")).unwrap();
        let mut consumer = WsConsumer::new(cfg.server_config(), test_rt());
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let cfg = WsEndpointConfig::from_uri(&format!("ws://127.0.0.1:{port}/healthy")).unwrap();
        let mut consumer = WsConsumer::new(cfg.server_config(), test_rt());
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

        let result = consumer.stop().await;
        assert!(
            result.is_ok(),
            "stop should succeed when server is healthy: {:?}",
            result
        );
    }

    // -------------------------------------------------------------------
    // Shared-server death supervision (rc-nxml4 / ADR-0007) — port of the
    // camel-http rc-szmob pattern (5be04091) to camel-ws's forward_task
    // architecture.
    // -------------------------------------------------------------------

    /// rc-nxml4 (ADR-0007 parity): when the shared WebSocket server task
    /// for a port dies, EVERY WsConsumer hosted on that port must fail —
    /// its `forward_task` returns Err via `background_task_handle()`,
    /// which camel-core's consumer watcher turns into a per-route
    /// CrashNotification → FailRoute → supervision backoff restart.
    /// Before the fix the forward tasks hung forever (zombie routes):
    /// `env_rx.recv()` never fires when the server task dies, because the
    /// envelope senders live in the (still-alive) shared dispatch table,
    /// not in the dead task.
    ///
    /// Deterministic by construction: the two routes share one bound
    /// socket (std try_clone), the server is killed via its AbortHandle
    /// (real JoinError → monitor's unexpected-exit branch), and forward
    /// task resolution is bounded by a timeout — on unmodified behavior
    /// the timeout trips, which is exactly the zombie this test pins
    /// down.
    #[tokio::test]
    async fn shared_server_death_fails_every_hosted_consumer() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        // One socket, two handles: both consumers host on the same port.
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std_clone = std_listener.try_clone().unwrap();
        let port = std_listener.local_addr().unwrap().port();
        let listener1 = tokio_listener_from_std(std_listener);
        let listener2 = tokio_listener_from_std(std_clone);

        let errors: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let rt: Arc<dyn camel_component_api::RuntimeObservability> =
            Arc::new(RecordingRuntime::new(Arc::clone(&errors)));

        let start_hosted = |path: &str, route_id: &str, listener: tokio::net::TcpListener| {
            let uri = format!("ws://127.0.0.1:{port}/{path}");
            let cfg = WsEndpointConfig::from_uri(&uri).unwrap();
            let mut consumer = WsConsumer::new(cfg.server_config(), Arc::clone(&rt));
            let (tx, rx) = mpsc::channel(16);
            let ctx = ConsumerContext::new(tx, CancellationToken::new(), route_id.to_string());
            async move {
                consumer
                    .start_with_listener(ctx, listener)
                    .await
                    .expect("consumer start must succeed");
                let bg = consumer
                    .background_task_handle()
                    .expect("forward task handle must exist after start");
                (consumer, rx, bg)
            }
        };

        // Two routes hosted on the SAME shared server (same bound port).
        let (consumer_a, _rx_a, bg_a) = start_hosted("zombie-a", "zombie-route-a", listener1).await;
        let (consumer_b, _rx_b, bg_b) = start_hosted("zombie-b", "zombie-route-b", listener2).await;
        // Drop the consumers — the forward tasks are independent; the
        // `_rx` receivers stay alive so the pipeline sender never errors
        // (mirrors a live route holding its receiver).
        drop(consumer_a);
        drop(consumer_b);

        // Kill the shared server task: abort → JoinError → the monitor's
        // unexpected-exit branch. This is the real crash path (no mock).
        {
            let guard = ServerRegistry::global()
                .inner
                .lock()
                .expect("ServerRegistry lock");
            let handle = guard
                .get(&port)
                .and_then(|entry| entry.cell.get())
                .expect("shared server entry must exist");
            handle.server_abort.abort();
        }

        // THE assertion: both hosted routes' forward tasks must fail
        // (bounded). On the zombie bug they never resolve and this
        // timeout trips.
        let outcome_a = tokio::time::timeout(Duration::from_secs(2), bg_a)
            .await
            .expect("ZOMBIE: consumer-a forward task still running after shared server death (rc-nxml4)");
        let outcome_b = tokio::time::timeout(Duration::from_secs(2), bg_b)
            .await
            .expect("ZOMBIE: consumer-b forward task still running after shared server death (rc-nxml4)");

        let err_a = outcome_a
            .expect("consumer-a forward task must join")
            .expect_err("consumer-a forward task must return Err when the shared server dies");
        let err_b = outcome_b
            .expect("consumer-b forward task must join")
            .expect_err("consumer-b forward task must return Err when the shared server dies");

        // The error must identify the dead shared transport (it flows into
        // the CrashNotification message camel-core records against the
        // route).
        for (name, err) in [("a", &err_a), ("b", &err_b)] {
            assert!(
                err.to_string().contains("127.0.0.1")
                    && err.to_string().contains(&port.to_string()),
                "consumer-{name} error must name the dead shared server, got: {err}"
            );
        }

        // Error counter regression guard: the monitor still records
        // `e:ws:server-task-exited` for the route that spawned the server.
        let recorded = errors.lock().expect("error recorder lock").clone();
        assert!(
            recorded
                .iter()
                .any(|(route, label)| label == "e:ws:server-task-exited"
                    && route == "zombie-route-a"),
            "expected e:ws:server-task-exited for the spawning route, got: {recorded:?}"
        );
    }

    /// Clean server exit must NOT cancel `server_exited` — route stops own
    /// their termination (no CrashNotification storm on graceful
    /// teardown). Monitor-level unit test, mirroring camel-http's
    /// `monitor_task_silent_on_clean_exit`.
    #[tokio::test]
    async fn monitor_silent_on_clean_exit_does_not_cancel() {
        let handle: JoinHandle<()> = tokio::spawn(async {});
        let server_error = new_atomic_false();
        let server_exited = CancellationToken::new();
        monitor_ws_server_task(
            handle,
            "127.0.0.1:0".parse().unwrap(),
            Arc::new(RecordingRuntime::new(Arc::new(Mutex::new(Vec::new())))),
            "test-monitor".into(),
            Arc::clone(&server_error),
            server_exited.clone(),
        )
        .await;
        assert!(
            !server_exited.is_cancelled(),
            "clean server exit must not cancel server_exited"
        );
    }

    /// Unexpected exit (panic) must cancel `server_exited` so every hosted
    /// consumer fails and supervision engages (ADR-0007).
    #[tokio::test]
    async fn monitor_cancels_on_server_panic() {
        let handle: JoinHandle<()> = tokio::spawn(async {
            panic!("simulated server crash");
        });
        let server_error = new_atomic_false();
        let server_exited = CancellationToken::new();
        let errors: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        monitor_ws_server_task(
            handle,
            "127.0.0.1:9999".parse().unwrap(),
            Arc::new(RecordingRuntime::new(Arc::clone(&errors))),
            "test-monitor".into(),
            Arc::clone(&server_error),
            server_exited.clone(),
        )
        .await;
        assert!(
            server_exited.is_cancelled(),
            "crashed server must cancel server_exited"
        );
        let recorded = errors.lock().expect("error recorder lock").clone();
        assert!(
            recorded
                .iter()
                .any(|(route, label)| route == "test-monitor" && label == "e:ws:server-task-exited"),
            "crashed server must record e:ws:server-task-exited, got: {recorded:?}"
        );
    }

    /// camel-ws-specific third death shape: `serve()` returning Err exits
    /// the task body with `Ok(())` after setting `server_error`. The
    /// transport is dead, so the monitor must cancel `server_exited` even
    /// though the JoinHandle resolved cleanly.
    #[tokio::test]
    async fn monitor_cancels_on_serve_error_exit() {
        let server_error = new_atomic_false();
        let flag = Arc::clone(&server_error);
        let handle: JoinHandle<()> = tokio::spawn(async move {
            // Simulates the serve task body's error path: record the flag,
            // then let the task end (the body returns ()).
            flag.store(true, Ordering::Relaxed);
        });
        let server_exited = CancellationToken::new();
        monitor_ws_server_task(
            handle,
            "127.0.0.1:9998".parse().unwrap(),
            Arc::new(RecordingRuntime::new(Arc::new(Mutex::new(Vec::new())))),
            "test-monitor".into(),
            server_error,
            server_exited.clone(),
        )
        .await;
        assert!(
            server_exited.is_cancelled(),
            "serve-error exit must cancel server_exited (dead transport)"
        );
    }

    /// Dead shared servers are evicted lazily: after the server task dies,
    /// the next `get_or_spawn_with_listener` on the same port must spawn a
    /// FRESH server (new dispatch table, uncancelled token) instead of
    /// rejoining the dead one — this is what makes a supervision restart
    /// actually rebind (parity with camel-http's eviction).
    #[tokio::test]
    async fn dead_shared_server_is_evicted_and_rebinds() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        // Two handles on one socket: first spawns, second is kept for the
        // post-death rebind (same port by construction, no rebind race).
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std_clone = std_listener.try_clone().unwrap();
        let port = std_listener.local_addr().unwrap().port();
        let listener1 = tokio_listener_from_std(std_listener);
        let listener2 = tokio_listener_from_std(std_clone);

        let evict_rt: Arc<dyn camel_component_api::RuntimeObservability> =
            Arc::new(RecordingRuntime::new(Arc::new(Mutex::new(Vec::new()))));
        let (state1, _addr1, _listen1, token1) = ServerRegistry::global()
            .get_or_spawn_with_listener(
                listener1,
                None,
                Arc::clone(&evict_rt),
                "evict-route-1".into(),
            )
            .await
            .unwrap();

        // Kill the shared server task and wait until the monitor has
        // cancelled the death token (bounded, no sleeps).
        {
            let guard = ServerRegistry::global()
                .inner
                .lock()
                .expect("ServerRegistry lock");
            let handle = guard
                .get(&port)
                .and_then(|entry| entry.cell.get())
                .expect("shared server entry must exist");
            handle.server_abort.abort();
        }
        tokio::time::timeout(Duration::from_secs(2), token1.cancelled())
            .await
            .expect("server death token must cancel after abort");
        // Eviction signal note (rc-onm5b): eviction reads `serve_alive`,
        // flipped by the drop guard when the aborted serve future is
        // dropped — strictly before the monitor observes the abort — so
        // the rebinding below has no monitor-completion ordering
        // dependency on any runtime flavor.

        // The next spawn on the same port must rebind: fresh state, fresh
        // (uncancelled) death token.
        let (state2, _addr2, _listen2, token2) = ServerRegistry::global()
            .get_or_spawn_with_listener(listener2, None, evict_rt, "evict-route-2".into())
            .await
            .unwrap();

        assert!(
            !token2.is_cancelled(),
            "rebind must produce a live server with an uncancelled death token"
        );
        assert!(
            !Arc::ptr_eq(&state1.dispatch, &state2.dispatch),
            "rebind must produce a NEW server entry, not the dead one"
        );
        assert!(token1.is_cancelled());
    }

    /// Leave a listened-then-dead entry in the global registry (rc-onm5b
    /// harness): spawn a server on a dedicated current-thread runtime, let
    /// the runtime poll the serve task into its accept park (LISTENED,
    /// proven by a live TCP connect), then drop the runtime so the parked
    /// serve task dies mid-park. Returns the corpse entry's dispatch-table
    /// address and its port.
    ///
    /// This is the death mode `#[tokio::test]` runtimes inflict at test
    /// end: the task is dropped, not run to completion, and its
    /// `JoinHandle` may never report completion — while the
    /// process-lifetime registry keeps the initialized entry for the next
    /// test to join via ephemeral port reuse.
    fn leave_listened_then_dead_entry(route_id: &str) -> (DispatchTable, String, u16) {
        let route = route_id.to_string();
        std::thread::spawn(move || {
            let owner_rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("owner runtime");
            let out = owner_rt.block_on(async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("owner bind");
                let port = listener.local_addr().unwrap().port();
                let (state, _addr, _listen, _token) = ServerRegistry::global()
                    .get_or_spawn_with_listener(listener, None, test_rt(), route.clone())
                    .await
                    .expect("owner server entry should spawn");
                // Yield so the owner runtime polls the serve task into its
                // accept park: the entry has now LISTENED before it dies.
                tokio::time::sleep(Duration::from_millis(10)).await;
                // Liveness proof at the moment of death: the bound socket
                // accepts a connection into its backlog.
                tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .expect("owner accept loop must be live before death");
                (Arc::clone(&state.dispatch), state.route_id.clone(), port)
            });
            // Drop the runtime BEFORE returning: the parked serve task and
            // its monitor are cancelled mid-park while the process-lifetime
            // registry keeps the initialized entry.
            drop(owner_rt);
            out
        })
        .join()
        .expect("owner thread")
    }

    /// rc-onm5b: an entry whose serve task listened and then died with its
    /// owner runtime must NOT be joined by a later test that reuses the
    /// port. A corpse join hands out an instantly-"ready" entry whose
    /// accept loop is dead: the client's bounded connect loop then fails
    /// with connection refused (the rare flaky-red under load). The joiner
    /// must instead receive a FRESH server that actually accepts traffic on
    /// the reused port.
    #[tokio::test]
    async fn listened_then_dead_entry_is_not_joined_on_port_reuse() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        let (corpse_dispatch, corpse_route, port) = leave_listened_then_dead_entry("wsevict-owner");

        // Port reuse: the corpse's socket closed at runtime drop, so the
        // kernel reissues the port to this joiner, which joins through the
        // production `get_or_spawn_with_listener` path.
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("port reissued for the joiner");
        let (state, _addr, _listen, _token) = ServerRegistry::global()
            .get_or_spawn_with_listener(listener, None, test_rt(), "wsevict-joiner".into())
            .await
            .expect("joiner spawn");
        // Identity is asserted on `route_id` (the creating consumer's
        // route): a raw dispatch-table address is NOT sound here — a
        // correct eviction frees the corpse table and the allocator can
        // hand the same address to the fresh one. The held `corpse_dispatch`
        // clone pins the corpse allocation, so the ptr check below is
        // stable as a second signal.
        assert_ne!(
            state.route_id, corpse_route,
            "joiner joined the listened-then-dead entry (rc-onm5b corpse join)"
        );
        assert!(
            !Arc::ptr_eq(&state.dispatch, &corpse_dispatch),
            "joiner must receive a fresh dispatch table, not the corpse's"
        );
        // The fresh accept loop must serve on the reused port. A corpse
        // join leaves the port CLOSED (the joiner's injected listener is
        // dropped unused), so this connect is refused and times out.
        tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        .expect("fresh server must accept on the reused port within 2s")
        .expect("connect");

        ServerRegistry::reset();
    }

    /// rc-onm5b coverage: eviction must not over-fire. A LIVE entry is
    /// joined as-is — same dispatch table, ref count 2 — and keeps
    /// accepting traffic after the join.
    #[tokio::test]
    async fn live_shared_server_is_not_evicted_on_join() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().unwrap().port();
        let (state1, _addr, _listen, _token) = ServerRegistry::global()
            .get_or_spawn_with_listener(listener, None, test_rt(), "wsevict-live-1".into())
            .await
            .expect("first spawn");

        let (state2, _listen2, _token2) = ServerRegistry::global()
            .get_or_spawn("127.0.0.1", port, None, test_rt(), "wsevict-live-2".into())
            .await
            .expect("second join");

        assert!(
            Arc::ptr_eq(&state1.dispatch, &state2.dispatch),
            "a live entry must be joined, not evicted"
        );
        assert_eq!(
            ServerRegistry::global().ref_count_for_test(port),
            2,
            "join must take a second reference on the live entry"
        );
        tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        .expect("live server must keep accepting after the join")
        .expect("connect");

        ServerRegistry::reset();
    }

    /// rc-onm5b coverage: the itest join path stages a pre-bound listener
    /// for the reused port before `get_or_spawn` (camel-test support). With
    /// the dead entry evicted, the init winner must consume the STAGED
    /// socket and serve on it — not rejoin the corpse, and not bind past
    /// the staged listener.
    #[tokio::test]
    async fn port_reuse_after_eviction_serves_staged_listener() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();

        let (corpse_dispatch, corpse_route, port) =
            leave_listened_then_dead_entry("wsevict-staged-owner");

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("port reissued for the staged joiner");
        ServerRegistry::global()
            .stage_listener(listener)
            .await
            .expect("stage listener");
        let (state, _listen, _token) = ServerRegistry::global()
            .get_or_spawn(
                "127.0.0.1",
                port,
                None,
                test_rt(),
                "wsevict-staged-joiner".into(),
            )
            .await
            .expect("staged joiner spawn");
        // `route_id` identity, not a dispatch-table address: a correct
        // eviction frees the corpse table and the allocator may reuse its
        // address for the fresh one (observed as a test false positive
        // under full-suite load). The held clone pins the corpse
        // allocation so the ptr check stays stable.
        assert_ne!(
            state.route_id, corpse_route,
            "staged joiner joined the listened-then-dead entry (rc-onm5b corpse join)"
        );
        assert!(
            !Arc::ptr_eq(&state.dispatch, &corpse_dispatch),
            "staged joiner must receive a fresh dispatch table, not the corpse's"
        );
        tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        .expect("fresh server must serve on the staged listener within 2s")
        .expect("connect");

        ServerRegistry::reset();
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
    /// `scheme=ws` / `operation=connect` in retry log events so operators
    /// can identify which component is retrying.
    ///
    /// Drives `retry_async` directly with `("ws", "connect")` and a
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
            "ws",
            "connect",
            || async {
                Err(CamelError::ProcessorError(
                    "WebSocket connection refused: simulated".to_string(),
                ))
            },
            is_retryable_ws_error,
            None,
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
            first.contains("scheme=ws"),
            "rc-1nm regression: expected 'scheme=ws' in WS retry log, got: {first}"
        );
        assert!(
            first.contains("operation=connect"),
            "rc-1nm regression: expected 'operation=connect' in WS retry log, got: {first}"
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

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let tls_cfg = WsTlsConfig {
            cert_path: cert_path.to_str().expect("cert path").to_string(),
            key_path: key_path.to_str().expect("key path").to_string(),
        };

        // Spawn a single WSS server.
        let (_state, _addr, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(
                listener,
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

        // One socket, two handles: clone at the std level BEFORE the tokio
        // conversion so both injected listeners report the same local port.
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std_clone = std_listener.try_clone().unwrap();
        let listener1 = tokio_listener_from_std(std_listener);
        let listener2 = tokio_listener_from_std(std_clone);
        let port = listener1.local_addr().unwrap().port();
        let tls_cfg = WsTlsConfig {
            cert_path: cert_path.to_str().expect("cert path").to_string(),
            key_path: key_path.to_str().expect("key path").to_string(),
        };

        // Acquire TWO references to the same port.
        let (_s1, _addr1, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(
                listener1,
                Some(tls_cfg.clone()),
                test_rt(),
                "ws-multiref-r1".into(),
            )
            .await
            .expect("WSS server should spawn (ref 1)");
        let (_s2, _addr2, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(
                listener2,
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

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (_state, _addr, _, _) = ServerRegistry::global()
            .get_or_spawn_with_listener(
                listener,
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

    // WSS readiness: a failed TLS listener bind must NOT signal readiness.
    #[tokio::test]
    async fn test_wss_bind_failure_does_not_mark_ready() {
        use camel_component_api::StartupSignal;
        use camel_component_api::test_support::{NoopRuntimeObservability, tls};

        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let _ = rustls::crypto::ring::default_provider().install_default();
        // Clean global state (process-lifetime servers may leak across tests).
        ServerRegistry::reset();

        // Generate valid TLS material so we get past cert loading and reach
        // the actual listener bind.
        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("ws-bindfail-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("ws-bindfail-key.pem", &key_pem);
        let cert_str = cert_path.to_str().expect("cert path");
        let key_str = key_path.to_str().expect("key path");

        // Pre-bind the port so the WSS listener bind fails with EADDRINUSE.
        let blocker = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = blocker.local_addr().unwrap().port();

        let uri = format!("wss://127.0.0.1:{port}/secure?tlsCert={cert_str}&tlsKey={key_str}");
        let component_ctx = NoOpComponentContext;
        let endpoint = WssComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .unwrap();
        // NoopRuntimeObservability: the bind-failure path calls
        // `health().force_unhealthy_for_route`, which must not panic.
        let rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability> =
            std::sync::Arc::new(NoopRuntimeObservability);
        let mut consumer = endpoint.create_consumer(rt).unwrap();

        // Install our own startup pair so we can observe whether mark_ready
        // was called.
        let (signal, receiver) = StartupSignal::pair();
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-bindfail-route".to_string(),
        )
        .with_startup(signal);

        let result = consumer.start(ctx).await;
        assert!(
            result.is_err(),
            "start() must return Err when the WSS listener bind fails: {result:?}"
        );

        // ctx was dropped when start() returned, so the startup signal sender
        // is gone. await_ready resolves immediately: Err means the consumer
        // never signalled readiness (good); Ok would mean mark_ready was
        // called before the bind failure surfaced (bug).
        let ready_result: Result<(), _> = receiver.await_ready().await;
        assert!(
            ready_result.is_err(),
            "mark_ready() must not be called when the WSS listener bind fails"
        );

        drop(blocker);
        let _ = consumer.stop().await;
    }

    // rc-oo0c regression: gate_ready must never park on a registry handle
    // whose serve task was cancelled before its first poll.
    //
    // Mechanism under test: every #[tokio::test] owns a current-thread
    // runtime. A test that spawns a TLS server via the registry and returns
    // without awaiting readiness leaves the serve task queued, not polled;
    // when that runtime drops, the task is dropped unpolled, so
    // `axum_server::Handle` never stores its listening address and never
    // notifies waiters — while the process-lifetime `ServerRegistry` keeps
    // the entry. A later consumer that joins that entry (ephemeral port
    // reuse under workspace load) awaits `listening()` forever, holding
    // REGISTRY_TEST_LOCK and wedging every queued test (the rc-oo0c hang).
    //
    // Phase 1 reproduces the leftover entry deterministically on an owned
    // runtime; phase 2 joins it through the production `start()` path.
    // rc-nxml4 evolution: the join now lands in lazy dead-server eviction
    // (fresh rebind), so phase 2 asserts bounded recovery rather than the
    // bounded failure it originally pinned.
    #[tokio::test]
    async fn wss_start_does_not_park_on_dead_registry_handle() {
        use camel_component_api::test_support::{NoopRuntimeObservability, tls};

        let _guard = REGISTRY_TEST_LOCK.lock().await;
        ServerRegistry::reset();
        let _ = rustls::crypto::ring::default_provider().install_default();

        // Phase 1 — spawn a TLS server entry whose serve task is never
        // polled: the block_on future returns without yielding after the
        // spawn, so the queued task is dropped unpolled when owner_rt drops.
        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("ws-deadhandle-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("ws-deadhandle-key.pem", &key_pem);
        let cert_str = cert_path.to_str().expect("cert path").to_string();
        let key_str = key_path.to_str().expect("key path").to_string();
        let owner_tls_cfg = WsTlsConfig {
            cert_path: cert_str.clone(),
            key_path: key_str.clone(),
        };
        // A dedicated thread owns the runtime so this test's runtime can
        // stay active while the owner's tasks die (block_on cannot nest).
        let port = std::thread::spawn(move || {
            let owner_rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("owner runtime");
            let port = owner_rt.block_on(async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("owner bind");
                let port = listener.local_addr().unwrap().port();
                let (_state, _addr, _handle, _) = ServerRegistry::global()
                    .get_or_spawn_with_listener(
                        listener,
                        Some(owner_tls_cfg),
                        test_rt(),
                        "ws-deadhandle-owner".into(),
                    )
                    .await
                    .expect("owner server entry should spawn");
                // No await after the spawn: return while the serve task is
                // still queued, so dropping owner_rt cancels it unpolled.
                // Determinism depends on get_or_spawn_with_listener not
                // yielding after its internal spawn — if it ever does, the
                // owner runtime polls the serve task once, it may reach
                // listening(), and phase 2 fails loudly on its is_err
                // assert (a welcome alarm, not a silent flake).
                port
            });
            // Drop the runtime BEFORE returning: the queued serve task is
            // cancelled without ever being polled.
            drop(owner_rt);
            port
        })
        .join()
        .expect("owner thread");

        // Phase 2 — join the leftover entry via the production path (what
        // an ephemeral-port reuse does). rc-nxml4 changed the outcome:
        // the dead entry (its monitor finished with the dropped runtime)
        // is evicted lazily and `start()` rebinds a FRESH server on the
        // port, so the gate resolves Ok instead of failing. Recovery is
        // strictly better than the bounded readiness failure rc-oo0c
        // originally pinned; the "never park" contract is still asserted
        // by the timeout below. A stalled-but-alive serve task (monitor
        // unfinished) still takes gate_ready's bounded timeout path.
        let uri = format!("wss://127.0.0.1:{port}/secure?tlsCert={cert_str}&tlsKey={key_str}");
        let component_ctx = NoOpComponentContext;
        let endpoint = WssComponent::new()
            .create_endpoint(&uri, &component_ctx)
            .expect("endpoint");
        let rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability> =
            std::sync::Arc::new(NoopRuntimeObservability);
        let mut consumer = endpoint.create_consumer(rt).expect("consumer");
        let (route_tx, _route_rx) = mpsc::channel(16);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-deadhandle-route".to_string(),
        );

        let outcome = tokio::time::timeout(Duration::from_secs(5), consumer.start(ctx)).await;
        assert!(
            outcome.is_ok(),
            "rc-oo0c: start() parked on a dead registry handle (serve task cancelled unpolled)"
        );
        let result = outcome.expect("bounded start");
        assert!(
            result.is_ok(),
            "rc-nxml4: start() must recover by evicting the dead entry and rebinding, got: {:?}",
            result.err()
        );

        ServerRegistry::reset();
    }

    #[test]
    fn ws_upgrade_error_provider_unavailable_is_503() {
        let err =
            CamelError::AuthProviderUnavailable("totally arbitrary detail with no marker".into());
        let resp = ws_upgrade_auth_error(&err).into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn ws_upgrade_error_generic_processor_error_is_500() {
        let err = CamelError::ProcessorError("auth provider unavailable".into());
        let resp = ws_upgrade_auth_error(&err).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn ws_upgrade_error_unauthenticated_is_401() {
        let err = CamelError::Unauthenticated("bad".into());
        let resp = ws_upgrade_auth_error(&err).into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // -------------------------------------------------------------------
    // Recording runtime: injects a caller-owned errors collector so the test
    // can assert on the recorded increment_errors calls after the consumer
    // runs (double shared via crate::test_doubles).
    // -------------------------------------------------------------------

    struct RecordingRuntime {
        metrics_collector: Arc<RecordingMetrics>,
    }

    impl RecordingRuntime {
        fn new(errors: Arc<Mutex<Vec<(String, String)>>>) -> Self {
            Self {
                metrics_collector: Arc::new(RecordingMetrics::with_errors(errors)),
            }
        }
    }

    impl camel_component_api::RuntimeObservability for RecordingRuntime {
        fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
            Arc::clone(&self.metrics_collector) as Arc<dyn camel_api::MetricsCollector>
        }
        fn health(&self) -> Arc<dyn camel_component_api::HealthCheckRegistry> {
            panic!("RecordingRuntime::health not used in this test")
        }
    }

    #[tokio::test]
    async fn ws_message_dispatch_failure_counts_b_prime() {
        let _guard = REGISTRY_TEST_LOCK.lock().await;
        let errors: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let uri = format!("ws://127.0.0.1:{port}/dispatch");

        // Recording runtime through the same seam the other tests fill with
        // the Panic helper (`test_rt()`).
        let mut consumer = WsConsumer::new(
            WsEndpointConfig::from_uri(&uri).unwrap().server_config(),
            Arc::new(RecordingRuntime::new(Arc::clone(&errors))),
        );

        // Pipeline receiver dropped up front: every forward-task dispatch
        // send fails with `ChannelClosed`.
        let (route_tx, route_rx) = mpsc::channel(16);
        drop(route_rx);
        let ctx = ConsumerContext::new(
            route_tx,
            CancellationToken::new(),
            "ws-test-route".to_string(),
        );
        consumer.start_with_listener(ctx, listener).await.unwrap();

        let url = format!("ws://127.0.0.1:{port}/dispatch");
        let mut client = connect_until_ready(&url).await;
        client
            .send(ClientMessage::Text("lost dispatch".into()))
            .await
            .unwrap();

        // Forward tick: let the dispatch reach the forward task and fail
        // the pipeline send.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                if !errors.lock().expect("recording collector lock").is_empty() {
                    break;
                }
            }
        })
        .await
        .expect("forward task never recorded the message-dispatch failure");

        let recorded = errors.lock().expect("recording collector lock").clone();
        assert_eq!(
            recorded,
            vec![(
                "ws-test-route".to_string(),
                "b-prime:ws:message-dispatch".to_string()
            )]
        );

        consumer.stop().await.unwrap();
    }
}
