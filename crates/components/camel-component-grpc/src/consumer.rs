use std::collections::HashMap as StdHashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use base64::Engine;
use bytes::BytesMut;
use camel_api::redact::redact_host;
use camel_api::security_policy::AuthPrincipal;
use camel_api::store_principal_properties;
use camel_api::{Body, CamelError, Exchange, Message, Value};
use camel_auth::{AuthenticatedPrincipal, CredentialSource, enforce_dispatch, install_carrier};
use camel_component_api::{
    ConcurrencyModel, Consumer, ConsumerContext, ConsumerStartupMode, ExchangeEnvelope,
    InFlightClaim, SecurityContext,
};
use camel_proto_compiler::ProtoCache;
use prost::Message as _;
use prost_reflect::{DynamicMessage, MessageDescriptor};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::Status;
use tracing::{debug, info};

use crate::config::GrpcServerConfig;
use crate::mode::GrpcMode;
use crate::server::GrpcDispatchEntry;
use crate::server::GrpcDispatchTable;
use crate::server::GrpcKernelAuth;
use crate::server::GrpcServerRegistry;

static PROTO_CACHE: OnceLock<ProtoCache> = OnceLock::new();

/// Per-request kernel authentication bundle: the sealed principal minted at
/// the transport boundary plus the plan it must stay bound to
/// (`unify-transport-auth`, Task 2.1).
pub(crate) struct KernelRequestAuth {
    plan: camel_api::security_policy::RouteSecurityPlan,
    principal: AuthenticatedPrincipal,
}

impl KernelRequestAuth {
    /// Install the typed carrier on a fresh request exchange, then enforce
    /// the route binding. The carrier is installed BEFORE the pipeline runs
    /// — a fresh exchange is created per request, so every dispatched
    /// exchange must carry its own principal (the Task 2.9 dispatch check
    /// relies on this). The principal is also mirrored to exchange
    /// properties so route processors can observe the subject.
    /// `enforce_dispatch` fails closed (`Public`
    /// short-circuits to Ok) and denials map to the transport idiom.
    fn apply_to(&self, exchange: &mut Exchange) -> Result<(), Status> {
        store_principal_properties(exchange, self.principal.principal());
        install_carrier(exchange, &self.principal);
        enforce_dispatch(&self.plan, exchange).map_err(|e| match e {
            CamelError::Unauthenticated(msg) => Status::unauthenticated(msg),
            other => Status::internal(other.to_string()),
        })
    }
}

/// Bind a minted kernel principal to its route plan for one request.
///
/// `None` when the route has no kernel state (plan-less: Public
/// pass-through) or when the request carries no minted principal
/// (Public plans pass through without extraction).
fn kernel_request_auth(
    kernel: Option<&GrpcKernelAuth>,
    principal: Option<AuthenticatedPrincipal>,
) -> Option<KernelRequestAuth> {
    let kernel = kernel?;
    let principal = principal?;
    Some(KernelRequestAuth {
        plan: kernel.plan.clone(),
        principal,
    })
}

fn proto_cache() -> &'static ProtoCache {
    PROTO_CACHE.get_or_init(ProtoCache::new)
}

/// rc-ey6v: envelope channel capacity == dispatcher semaphore permits.
///
/// One configured value (`consumerConcurrency`, default 64) derives BOTH
/// the envelope channel capacity and the dispatcher semaphore in
/// `start_inner`, so the semaphore stays the single backpressure point
/// and the channel can never become a second, hidden inflight cap.
/// Mirrors `envelope_channel_capacity` in camel-http (rc-3y6j pattern).
///
/// `.max(1)`: `consumerConcurrency=0` is representable, but
/// `tokio::sync::mpsc::channel(0)` panics and a 0-permit semaphore
/// would stall the consumer forever.
///
/// Values above `tokio::sync::Semaphore::MAX_PERMITS` are rejected
/// fail-closed (bd rc-9kgtm, ADR-0033): the dispatcher semaphore panics
/// above that bound. The channel==semaphore invariant derives both
/// primitives from this one validated value.
pub(crate) fn consumer_concurrency_limit(configured: usize) -> Result<usize, CamelError> {
    if configured > tokio::sync::Semaphore::MAX_PERMITS {
        return Err(CamelError::Config(format!(
            "consumerConcurrency {configured} exceeds the supported upper bound {} (tokio::sync::Semaphore::MAX_PERMITS)",
            tokio::sync::Semaphore::MAX_PERMITS
        )));
    }
    Ok(configured.max(1))
}

/// Map a pipeline error onto the transport denial idiom.
///
/// A pipeline policy denial (`CamelError::Unauthorized`, what
/// `SecurityPolicyService` returns) is `PERMISSION_DENIED` — the status
/// the deleted transport-side scratch evaluation used to emit, so denial
/// semantics survive with enforcement living wholly in the pipeline
/// layer. Every other pipeline error is system-broken, not a denial:
/// INTERNAL, unchanged.
fn pipeline_error_to_status(e: CamelError) -> Status {
    match e {
        CamelError::Unauthorized(msg) => Status::permission_denied(msg),
        other => Status::internal(format!("pipeline error: {other}")),
    }
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

/// Request envelope crossing the server→consumer boundary.
///
/// `kernel_principal` is the sealed principal minted by
/// `kernel_authenticate` at the transport boundary
/// (`unify-transport-auth`, Task 2.1) and is installed as the exchange's
/// typed carrier before the pipeline runs. Policy evaluation is NOT done
/// at the transport (the legacy scratch arm was deleted in
/// `finish-auth-flip`): enforcement lives in the pipeline layer plus the
/// strict dispatch check.
pub(crate) enum GrpcRequestEnvelope {
    Unary {
        metadata: tonic::metadata::MetadataMap,
        body: Vec<u8>,
        reply_tx: tokio::sync::oneshot::Sender<GrpcReply>,
        kernel_principal: Option<AuthenticatedPrincipal>,
    },
    ServerStreaming {
        metadata: tonic::metadata::MetadataMap,
        body: Vec<u8>,
        reply_tx: mpsc::Sender<GrpcStreamItem>,
        kernel_principal: Option<AuthenticatedPrincipal>,
    },
    ClientStreaming {
        metadata: tonic::metadata::MetadataMap,
        body_rx: mpsc::Receiver<Vec<u8>>,
        reply_tx: tokio::sync::oneshot::Sender<GrpcReply>,
        kernel_principal: Option<AuthenticatedPrincipal>,
    },
    Bidi {
        metadata: tonic::metadata::MetadataMap,
        body_rx: mpsc::Receiver<Vec<u8>>,
        reply_tx: mpsc::Sender<GrpcStreamItem>,
        kernel_principal: Option<AuthenticatedPrincipal>,
    },
}

/// Outcome of a cancellation-safe dispatcher-permit wait (bd rc-orr73).
///
/// A saturated permit wait must not pin the consumer loop past shutdown:
/// either the permit arrives, or the cancellation token fires first.
enum PermitWait {
    Granted(tokio::sync::OwnedSemaphorePermit),
    Cancelled,
}

/// Wait for a dispatcher permit, waking immediately on cancellation.
///
/// `biased` polls the cancellation branch first so a token already
/// cancelled before entry resolves `Cancelled` even when a permit is
/// simultaneously available — shutdown wins the race (bd rc-orr73).
async fn acquire_permit_cancel_safe(
    sem: std::sync::Arc<tokio::sync::Semaphore>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<PermitWait, CamelError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Ok(PermitWait::Cancelled),
        permit = sem.acquire_owned() => permit
            .map(PermitWait::Granted)
            .map_err(|_| CamelError::ChannelClosed),
    }
}

/// Best-effort UNAVAILABLE status message at shutdown, shared with the
/// transport's bidi cancel arm (rc-qq8zz).
pub(crate) const SHUTDOWN_STATUS: &str = "consumer shutting down";

/// Best-effort UNAVAILABLE reply for an envelope dropped at shutdown.
///
/// Unary-style envelopes answer on the oneshot; streaming envelopes get a
/// non-blocking `try_send`. Both are best-effort: a gone or full receiver
/// is ignored, never awaited — the shutdown path must not block.
fn reply_unavailable(envelope: GrpcRequestEnvelope) {
    debug!(path = "grpc consumer", "reply unavailable on shutdown");
    match envelope {
        GrpcRequestEnvelope::Unary { reply_tx, .. }
        | GrpcRequestEnvelope::ClientStreaming { reply_tx, .. } => {
            let _ = reply_tx.send(GrpcReply::Err(Status::unavailable(SHUTDOWN_STATUS)));
        }
        GrpcRequestEnvelope::ServerStreaming { reply_tx, .. }
        | GrpcRequestEnvelope::Bidi { reply_tx, .. } => {
            let _ = reply_tx.try_send(GrpcStreamItem::Error(Status::unavailable(SHUTDOWN_STATUS)));
        }
    }
}

// ── Identity-owned dispatch registration (bd rc-orr73, Task 1.2) ──────────

/// Remove a dispatch entry only when it is still owned by `env_tx`.
///
/// Identity check: the guard is the *registration owner*, not the path
/// owner. When Task 1.3's insert overwrites the path with a newer
/// registration on a fresh channel, the stale owner must leave the
/// replacement untouched.
fn remove_owned_entry(
    table: &mut tokio::sync::RwLockWriteGuard<
        '_,
        std::collections::HashMap<String, GrpcDispatchEntry>,
    >,
    path: &str,
    env_tx: &mpsc::Sender<GrpcRequestEnvelope>,
) {
    if table
        .get(path)
        .is_some_and(|entry| entry.0.same_channel(env_tx))
    {
        table.remove(path);
    }
}

/// Identity-owned registration in the server's dispatch table.
///
/// The consumer registers its envelope sender under its route path in
/// `start_inner`; this guard owns that registration and removes it on
/// `cleanup` or drop. Removal is identity-checked (`remove_owned_entry`),
/// so a replacement registration on the same path is never destroyed by
/// a stale owner.
struct DispatchRegistrationGuard {
    dispatch: GrpcDispatchTable,
    path: String,
    env_tx: mpsc::Sender<GrpcRequestEnvelope>,
    cancel: CancellationToken,
    armed: bool,
}

impl DispatchRegistrationGuard {
    fn arm(
        dispatch: GrpcDispatchTable,
        path: String,
        env_tx: mpsc::Sender<GrpcRequestEnvelope>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            dispatch,
            path,
            env_tx,
            cancel,
            armed: true,
        }
    }

    /// Remove this guard's registration, then disarm the guard.
    async fn cleanup(&mut self) {
        let mut table = self.dispatch.write().await;
        remove_owned_entry(&mut table, &self.path, &self.env_tx);
        // Disarm while still holding the write guard: no `.await` between
        // removal and disarm, so a racing Drop cannot re-run removal.
        self.armed = false;
        // rc-qq8zz: every teardown path cancels the entry token so the
        // transport's open streaming calls terminate.
        self.cancel.cancel();
    }
}

impl Drop for DispatchRegistrationGuard {
    fn drop(&mut self) {
        // rc-qq8zz: cancel FIRST — synchronous and runtime-free, before
        // the armed check and the runtime-conditional removal below — so
        // the no-runtime teardown branch still terminates open streaming
        // calls. Double-cancel is idempotent.
        self.cancel.cancel();
        if !self.armed {
            return;
        }
        match self.dispatch.try_write() {
            // Uncontended: remove synchronously, no runtime needed.
            Ok(mut table) => remove_owned_entry(&mut table, &self.path, &self.env_tx),
            Err(_) => {
                // Contended on a live runtime: defer to a spawned task.
                // No runtime (teardown): the entry's sender closes once
                // the consumer's receiver drops, and a new registration
                // replaces the path wholesale — doing nothing is safe.
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let dispatch = Arc::clone(&self.dispatch);
                    let path = self.path.clone();
                    let env_tx = self.env_tx.clone();
                    handle.spawn(async move {
                        let mut table = dispatch.write().await;
                        remove_owned_entry(&mut table, &path, &env_tx);
                    });
                }
            }
        }
    }
}

/// Register a consumer's envelope sender under its route path.
///
/// A live entry (sender not closed) fails as a duplicate, unchanged from
/// the previous inline check. An entry whose sender is closed is a stale
/// registration left behind by a forced abort or runtime teardown (bd
/// rc-orr73): its consumer is gone and nothing will ever remove it, so it
/// is replaceable — the fresh sender overwrites it instead of the path
/// being poisoned forever.
fn insert_dispatch_entry(
    table: &mut tokio::sync::RwLockWriteGuard<
        '_,
        std::collections::HashMap<String, GrpcDispatchEntry>,
    >,
    path: &str,
    env_tx: mpsc::Sender<GrpcRequestEnvelope>,
    mode: GrpcMode,
    kernel: Option<Arc<GrpcKernelAuth>>,
    cancel: CancellationToken,
) -> Result<(), CamelError> {
    if table.get(path).is_some_and(|entry| !entry.0.is_closed()) {
        return Err(CamelError::EndpointCreationFailed(format!(
            "duplicate gRPC consumer path: {path}"
        )));
    }
    table.insert(path.to_string(), (env_tx, mode, kernel, cancel));
    Ok(())
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

/// Reject credential sources the gRPC transport cannot carry.
///
/// gRPC metadata maps to HTTP headers only: `authorization_header` maps to the
/// `authorization` metadata key and `{header: {name}}` to the same-named
/// metadata key. Query parameters and cookies have no gRPC metadata
/// representation, so a route declaring them must fail at load (ADR-0033
/// fail-closed), not silently authenticate nothing at request time.
pub(crate) fn validate_credential_sources(sources: &[CredentialSource]) -> Result<(), CamelError> {
    for source in sources {
        let source_kind = match source {
            CredentialSource::QueryParam { .. } => "query_param",
            CredentialSource::Cookie { .. } => "cookie",
            _ => continue,
        };
        return Err(CamelError::Config(format!(
            "grpc routes cannot carry {source_kind} credential sources; supported: authorization_header, header" // allow-secret: field names in error text, not values
        )));
    }
    Ok(())
}

pub struct GrpcConsumer {
    host: String,
    port: u16,
    path: String,
    proto_path: PathBuf,
    service_name: String,
    method_name: String,
    mode: GrpcMode,
    security_ctx: Option<SecurityContext>,
    runtime: Arc<dyn camel_component_api::RuntimeObservability>,
    server_config: GrpcServerConfig,
    /// rc-ey6v: configured concurrency limit (`consumerConcurrency` URI
    /// param, default 64). Derives both the envelope channel capacity and
    /// the dispatcher semaphore in `start_inner` — never a second,
    /// independent cap.
    consumer_concurrency: usize,
}

impl GrpcConsumer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        host: String,
        port: u16,
        path: String,
        proto_path: PathBuf,
        service_name: String,
        method_name: String,
        mode: GrpcMode,
        runtime: Arc<dyn camel_component_api::RuntimeObservability>,
        server_config: GrpcServerConfig,
        consumer_concurrency: usize,
    ) -> Self {
        Self {
            host,
            port,
            path,
            proto_path,
            service_name,
            method_name,
            mode,
            security_ctx: None,
            runtime,
            server_config,
            consumer_concurrency,
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

    /// Validate every credential-source list this route can extract from:
    /// the configured sources and, when a compiled plan is present, the
    /// plan's own sources (fail-closed at load, ADR-0033).
    fn validate_route_credential_sources(&self) -> Result<(), CamelError> {
        let Some(sec_ctx) = &self.security_ctx else {
            return Ok(());
        };
        validate_credential_sources(&sec_ctx.credential_sources)?;
        if let Some(plan) = &sec_ctx.plan {
            validate_credential_sources(&plan.credential_sources)?;
        }
        Ok(())
    }

    pub async fn start_with_listener(
        &mut self,
        ctx: ConsumerContext,
        listener: tokio::net::TcpListener,
    ) -> Result<(), CamelError> {
        self.validate_route_credential_sources()?;
        // rc-9kgtm: fail-closed before registry mutation — an oversized
        // value must never reach Semaphore::new (panic) or the shared
        // server registry.
        let _concurrency = consumer_concurrency_limit(self.consumer_concurrency)?;
        let dispatch = GrpcServerRegistry::global()
            .get_or_spawn_with_listener(
                listener,
                &self.host,
                self.port,
                self.server_config.clone(),
                Arc::clone(&self.runtime),
            )
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

        // rc-ey6v: channel==semaphore invariant — the envelope channel
        // capacity and the dispatcher semaphore (below) derive from the
        // SAME `concurrency` value, so the channel can never become a
        // second, hidden backpressure point (camel-http
        // `envelope_channel_capacity`, rc-3y6j pattern).
        let concurrency = consumer_concurrency_limit(self.consumer_concurrency)?;
        let (env_tx, mut env_rx) = mpsc::channel::<GrpcRequestEnvelope>(concurrency);
        // Kernel interceptor state is captured HERE, at dispatch-entry
        // construction, from the security context wired before start
        // (Task 2.1 construction-order lifecycle fix). The per-request
        // handlers are built from this entry, so the plan is present
        // before any request arrives — never patched on afterwards.
        let kernel = self
            .security_ctx
            .as_ref()
            .and_then(GrpcKernelAuth::from_security_context)
            .map(Arc::new);
        let path = self.path.clone();
        // Clone BEFORE the insert: the registration guard needs a
        // `same_channel`-comparable clone of the sender that actually
        // lands in the table (bd rc-orr73).
        let env_tx_for_guard = env_tx.clone();
        // rc-qq8zz: per-registration child token of the consumer's
        // cancellation token. It rides the dispatch entry so the
        // transport's open streaming calls observe shutdown and abort.
        let handler_cancel = ctx.cancel_token().child_token();
        {
            let mut table = dispatch.write().await;
            insert_dispatch_entry(
                &mut table,
                &path,
                env_tx,
                mode,
                kernel.clone(),
                handler_cancel.clone(),
            )?;
        }
        let mut registration = DispatchRegistrationGuard::arm(
            Arc::clone(&dispatch),
            path.clone(),
            env_tx_for_guard,
            handler_cancel,
        );

        let host = self.host.clone();
        let port = self.port;
        let sender = ctx.sender();
        // rc-nftni (drainclaim): capture the context-global counter once;
        // every envelope this raw-sender consumer constructs carries a
        // claim minted at the acceptance dequeue below.
        let in_flight = ctx.in_flight_counter();

        info!(
            path = %redact_host(&path),
            host = %redact_host(&host),
            port = port,
            mode = ?mode,
            "grpc consumer started, waiting for requests"
        );

        // NOTE: Long-running bidi streams hold a semaphore permit for their duration.
        // If this becomes an issue, consider separate concurrency limits for streaming vs unary.
        // rc-ey6v: channel==semaphore invariant — permit count derives from
        // the same `concurrency` value as the envelope channel above; the
        // semaphore is the single backpressure point (camel-http
        // `envelope_channel_capacity`, rc-3y6j pattern).
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut join_set = tokio::task::JoinSet::new();

        loop {
            tokio::select! {
                biased;
                _ = ctx.cancelled() => {
                    info!(
                        path = %path,
                        "grpc consumer cancelled, shutting down"
                    );
                    break;
                }
                envelope = env_rx.recv() => {
                    let Some(envelope) = envelope else { break };

                    // rc-nftni: mint at acceptance — the dequeue of the
                    // transport's GrpcRequestEnvelope is where this consumer
                    // takes ownership of the wire request. The claim covers
                    // the semaphore wait and (unary/server-streaming) decode,
                    // route channel, and pipeline; every decode/rejection
                    // exit below drops it (RAII). It moves into the spawned
                    // processor future: for the streaming modes it is never
                    // consumed there, so it is held CALL-SCOPED — for the
                    // whole call, including idle gaps between chunk
                    // exchanges — while each chunk/completion envelope mints
                    // its own claim in the processor (transient +1 overlap,
                    // sound, never a false zero; pinned by
                    // grpc_consumer_client_streaming_holds_claim_across_idle_gap).
                    let claim = in_flight.as_ref().map(InFlightClaim::attach);

                    // rc-orr73: a saturated permit wait must not pin the
                    // loop past shutdown — cancellation wins the race, the
                    // dropped envelope gets a best-effort UNAVAILABLE, and
                    // the registration guard drops (removing the entry).
                    let permit = match acquire_permit_cancel_safe(
                        semaphore.clone(),
                        ctx.cancel_token(),
                    )
                    .await
                    {
                        Ok(PermitWait::Granted(permit)) => permit,
                        Ok(PermitWait::Cancelled) => {
                            reply_unavailable(envelope);
                            break;
                        }
                        Err(e) => {
                            reply_unavailable(envelope);
                            return Err(e);
                        }
                    };
                    let req_desc = req_desc.clone();
                    let resp_desc = resp_desc.clone();
                    let sender = sender.clone();
                    let correlation_id = next_observer_id();
                    let path_for_log = path.clone();
                    // Kernel state captured at dispatch-entry construction,
                    // cloned per request; the principal minted by the
                    // interceptor binds to this plan for the carrier install.
                    // Per-request policy evaluation is NOT done here —
                    // enforcement lives in the pipeline layer plus the
                    // strict dispatch check.
                    let kernel = kernel.clone();
                    let in_flight = in_flight.clone();

                    debug!(
                        path = %path_for_log,
                        correlation_id = %correlation_id,
                        "grpc consumer received request"
                    );

                    join_set.spawn(async move {
                        let _permit = permit;
                        match envelope {
                            GrpcRequestEnvelope::Unary { metadata, body, reply_tx, kernel_principal } => {
                                debug!(
                                    path = %path_for_log,
                                    correlation_id = %correlation_id,
                                    size = body.len(),
                                    "grpc consumer processing unary request"
                                );

                                let kernel_auth = kernel_request_auth(kernel.as_deref(), kernel_principal);
                                let result = process_unary_request(
                                    body, metadata, req_desc, resp_desc, sender, kernel_auth, claim,
                                ).await;
                                let reply = match result {
                                    Ok(bytes) => GrpcReply::Ok(bytes),
                                    Err(status) => GrpcReply::Err(status),
                                };
                                let _ = reply_tx.send(reply);
                            }
                            GrpcRequestEnvelope::ServerStreaming { metadata, body, reply_tx, kernel_principal } => {
                                debug!(
                                    path = %path_for_log,
                                    correlation_id = %correlation_id,
                                    size = body.len(),
                                    "grpc consumer processing server streaming request"
                                );

                                let kernel_auth = kernel_request_auth(kernel.as_deref(), kernel_principal);
                                process_server_streaming_request(
                                    body, metadata, req_desc, resp_desc, sender, reply_tx, kernel_auth, claim,
                                ).await;
                            }
                            GrpcRequestEnvelope::ClientStreaming { metadata, body_rx, reply_tx, kernel_principal } => {
                                debug!(
                                    path = %path_for_log,
                                    correlation_id = %correlation_id,
                                    "grpc consumer processing client streaming request"
                                );

                                let kernel_auth = kernel_request_auth(kernel.as_deref(), kernel_principal);
                                process_client_streaming_request(
                                    body_rx, metadata, req_desc, resp_desc, sender, reply_tx, kernel_auth, &in_flight,
                                ).await;
                            }
                            GrpcRequestEnvelope::Bidi { metadata, body_rx, reply_tx, kernel_principal } => {
                                debug!(
                                    path = %path_for_log,
                                    correlation_id = %correlation_id,
                                    "grpc consumer processing bidi streaming request"
                                );

                                let kernel_auth = kernel_request_auth(kernel.as_deref(), kernel_principal);
                                process_bidi_request(
                                    body_rx, metadata, req_desc, resp_desc, sender, reply_tx, kernel_auth, &in_flight,
                                ).await;
                            }
                        }
                    });
                }
            }
        }

        join_set.shutdown().await;

        // rc-orr73: identity-checked, awaited removal of THIS consumer's
        // dispatch registration (disarm happens under the write lock).
        registration.cleanup().await;

        info!(
            path = %path,
            "grpc consumer stopped"
        );

        Ok(())
    }
}

#[async_trait]
impl Consumer for GrpcConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        self.validate_route_credential_sources()?;
        // rc-9kgtm: fail-closed before the info! log, registry
        // get_or_spawn (which binds the listener), and readiness
        // signalling — an oversized value must never reach
        // Semaphore::new (panic).
        let _concurrency = consumer_concurrency_limit(self.consumer_concurrency)?;
        info!(
            host = %redact_host(&self.host),
            port = self.port,
            service = %self.service_name,
            method = %self.method_name,
            mode = ?self.mode,
            "grpc consumer starting"
        );
        let dispatch = GrpcServerRegistry::global()
            .get_or_spawn(
                &self.host,
                self.port,
                self.server_config.clone(),
                Arc::clone(&self.runtime),
            )
            .await?;
        // gRPC listener is bound inside get_or_spawn (TcpListener::bind
        // before tokio::spawn). Signal readiness now that the bind succeeded.
        ctx.mark_ready();
        self.start_inner(ctx, dispatch).await
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        info!(
            host = %redact_host(&self.host),
            port = self.port,
            service = %self.service_name,
            method = %self.method_name,
            "grpc consumer stopping"
        );
        GrpcServerRegistry::global()
            .unregister(&self.host, self.port, &self.path)
            .await;
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Concurrent { max: None }
    }

    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    fn set_security_context(&mut self, ctx: SecurityContext) {
        self.security_ctx = Some(ctx);
    }
}

// ── Unary processor (unchanged) ────────────────────────────────────────────

async fn process_unary_request(
    body: Vec<u8>,
    metadata: tonic::metadata::MetadataMap,
    req_desc: MessageDescriptor,
    resp_desc: MessageDescriptor,
    sender: mpsc::Sender<ExchangeEnvelope>,
    kernel_auth: Option<KernelRequestAuth>,
    claim: Option<InFlightClaim>,
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

    let mut exchange = Exchange::new(msg);
    if let Some(auth) = kernel_auth.as_ref() {
        // Carrier install + route-binding enforcement before the pipeline.
        auth.apply_to(&mut exchange)?;
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let envelope = ExchangeEnvelope {
        exchange,
        reply_tx: Some(reply_tx),
        // rc-nftni: acceptance-minted claim (drainclaim); decode failure
        // above dropped it via RAII, a failed push below rolls it back.
        in_flight_claim: claim,
    };

    sender
        .send(envelope)
        .await
        .map_err(|_| Status::internal("pipeline channel closed"))?;

    let result = reply_rx
        .await
        .map_err(|_| Status::internal("pipeline reply dropped"))?
        .map_err(pipeline_error_to_status)?;

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

#[allow(clippy::too_many_arguments)]
async fn process_server_streaming_request(
    body: Vec<u8>,
    metadata: tonic::metadata::MetadataMap,
    req_desc: MessageDescriptor,
    resp_desc: MessageDescriptor,
    sender: mpsc::Sender<ExchangeEnvelope>,
    reply_tx: mpsc::Sender<GrpcStreamItem>,
    kernel_auth: Option<KernelRequestAuth>,
    claim: Option<InFlightClaim>,
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
    register_observer(observer_id.clone(), observer.clone());
    let _guard = ObserverGuard::new(observer_id.clone());

    let mut exchange = Exchange::new(msg);
    if let Some(auth) = kernel_auth.as_ref()
        && let Err(status) = auth.apply_to(&mut exchange)
    {
        let _ = reply_tx.send(GrpcStreamItem::Error(status)).await;
        return;
    }
    exchange.set_property("CamelGrpcStreamObserverId", Value::String(observer_id));

    // The envelope carries a pipeline reply channel so a pipeline error
    // (policy denial included) reaches this processor instead of dying
    // with `reply_tx: None` — the regression where a denial ended the
    // stream as a silent, empty success.
    let (pipeline_reply_tx, pipeline_reply_rx) = tokio::sync::oneshot::channel();
    let envelope = ExchangeEnvelope {
        exchange,
        reply_tx: Some(pipeline_reply_tx),
        // rc-nftni: acceptance-minted claim (drainclaim); decode/auth
        // failures above dropped it via RAII, a failed push below rolls it
        // back.
        in_flight_claim: claim,
    };

    if sender.send(envelope).await.is_err() {
        let _ = reply_tx
            .send(GrpcStreamItem::Error(Status::internal(
                "pipeline channel closed",
            )))
            .await;
        return;
    }

    // The pipeline verdict decides the stream's terminal frame: a
    // pipeline error is surfaced client-visibly via the observer (the
    // same denial idiom the deleted transport-side scratch evaluation
    // emitted). A successful result streamed through the observer adds
    // nothing; so does a dropped reply sender (route stand-ins that
    // never reply) — the observer stream stays the truth either way.
    if let Ok(Err(e)) = pipeline_reply_rx.await {
        observer.on_error(pipeline_error_to_status(e)).await;
    }

    // Wait for the stream receiver to be dropped (stream complete).
    // This keeps the guard alive so the observer stays registered until
    // the route is done. If take_stream_observer was called, the guard's
    // Drop is a no-op. If not, the guard cleans up the leaked observer.
    reply_tx.closed().await;
}

// ── Client-streaming processor ─────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn process_client_streaming_request(
    mut body_rx: mpsc::Receiver<Vec<u8>>,
    metadata: tonic::metadata::MetadataMap,
    req_desc: MessageDescriptor,
    resp_desc: MessageDescriptor,
    sender: mpsc::Sender<ExchangeEnvelope>,
    reply_tx: tokio::sync::oneshot::Sender<GrpcReply>,
    kernel_auth: Option<KernelRequestAuth>,
    in_flight: &Option<Arc<AtomicU64>>,
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

        let mut exchange = Exchange::new(msg);
        if let Some(auth) = kernel_auth.as_ref()
            && let Err(status) = auth.apply_to(&mut exchange)
        {
            let _ = reply_tx.send(GrpcReply::Err(status));
            return;
        }
        let (reply_tx_pipe, reply_rx_pipe) = tokio::sync::oneshot::channel();
        let envelope = ExchangeEnvelope {
            exchange,
            reply_tx: Some(reply_tx_pipe),
            // rc-nftni: each chunk envelope mints its own claim at its
            // acceptance (the body_rx dequeue inside this loop).
            in_flight_claim: in_flight.as_ref().map(InFlightClaim::attach),
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

    let mut completion_exchange = Exchange::new(completion_msg);
    if let Some(auth) = kernel_auth.as_ref()
        && let Err(status) = auth.apply_to(&mut completion_exchange)
    {
        let _ = reply_tx.send(GrpcReply::Err(status));
        return;
    }
    let (reply_tx_pipe, reply_rx_pipe) = tokio::sync::oneshot::channel();
    let envelope = ExchangeEnvelope {
        exchange: completion_exchange,
        reply_tx: Some(reply_tx_pipe),
        // rc-nftni: the completion exchange mints its claim like every
        // chunk envelope.
        in_flight_claim: in_flight.as_ref().map(InFlightClaim::attach),
    };

    if sender.send(envelope).await.is_err() {
        let _ = reply_tx.send(GrpcReply::Err(Status::internal("pipeline channel closed")));
        return;
    }

    // The route's response to the completion Exchange becomes the gRPC response
    let result = match reply_rx_pipe.await {
        Ok(Ok(exchange)) => exchange,
        Ok(Err(e)) => {
            let _ = reply_tx.send(GrpcReply::Err(pipeline_error_to_status(e)));
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

#[allow(clippy::too_many_arguments)]
async fn process_bidi_request(
    mut body_rx: mpsc::Receiver<Vec<u8>>,
    metadata: tonic::metadata::MetadataMap,
    req_desc: MessageDescriptor,
    resp_desc: MessageDescriptor,
    sender: mpsc::Sender<ExchangeEnvelope>,
    reply_tx: mpsc::Sender<GrpcStreamItem>,
    kernel_auth: Option<KernelRequestAuth>,
    in_flight: &Option<Arc<AtomicU64>>,
) {
    let observer = GrpcStreamObserver::new(reply_tx.clone(), resp_desc);
    let observer_id = next_observer_id();
    register_observer(observer_id.clone(), observer.clone());
    let _guard = ObserverGuard::new(observer_id.clone());

    // Spawn a task to forward messages from the client stream to the pipeline
    let sender_clone = sender.clone();
    let metadata_clone = metadata.clone();
    let req_desc_clone = req_desc.clone();
    let in_flight_clone = in_flight.clone();

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
            if let Some(auth) = kernel_auth.as_ref()
                && let Err(status) = auth.apply_to(&mut exchange)
            {
                let _ = observer.on_error(status).await;
                break;
            }
            exchange.set_property(
                "CamelGrpcStreamObserverId",
                Value::String(observer_id.clone()),
            );

            // Each message envelope carries a pipeline reply channel so
            // a pipeline error (policy denial included) becomes a
            // client-visible stream error instead of dying with
            // `reply_tx: None`. The forwarding loop stays non-blocking:
            // a per-message watcher renders the verdict via the
            // observer.
            let (pipeline_reply_tx, pipeline_reply_rx) = tokio::sync::oneshot::channel();
            let envelope = ExchangeEnvelope {
                exchange,
                reply_tx: Some(pipeline_reply_tx),
                // rc-nftni: each message envelope mints its own claim at
                // its acceptance (the body_rx dequeue inside this loop).
                in_flight_claim: in_flight_clone.as_ref().map(InFlightClaim::attach),
            };

            if sender_clone.send(envelope).await.is_err() {
                let _ = observer
                    .on_error(Status::internal("pipeline channel closed"))
                    .await;
                break;
            }

            let verdict_observer = observer.clone();
            tokio::spawn(async move {
                if let Ok(Err(e)) = pipeline_reply_rx.await {
                    verdict_observer.on_error(pipeline_error_to_status(e)).await;
                }
            });
        }

        // Signal completion when client stream ends
        observer.on_completed().await;
    });

    // Wait for the forward task to complete
    let _ = forward_task.await;
}

#[cfg(test)]
mod tests {
    use super::*;

    use camel_component_api::StartupSignal;
    use tokio_util::sync::CancellationToken;

    /// Thin local pin — the full matrix lives in camel-api's
    /// `redact_host_masks_userinfo_keeps_clean_hosts`.
    #[test]
    fn canonical_redact_host_pinned() {
        assert_eq!(redact_host("host.example:8080"), "host.example:8080");
        assert_eq!(redact_host("a@b@c"), "***@c");
    }

    #[test]
    fn grpc_credential_sources_uncarryable_rejected_at_load() {
        let query = CredentialSource::QueryParam {
            param: "ticket".to_string(),
        };
        let err = validate_credential_sources(&[query]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("query_param"), "message was: {msg}");
        assert!(msg.contains("grpc"), "message was: {msg}");

        let cookie = CredentialSource::Cookie {
            name: "session".to_string(),
        };
        let err = validate_credential_sources(&[cookie]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cookie"), "message was: {msg}");
        assert!(msg.contains("grpc"), "message was: {msg}");

        // Carryable sources pass validation.
        let carryable = vec![
            CredentialSource::AuthorizationHeader,
            CredentialSource::Header {
                name: "x-api-key".to_string(),
            },
        ];
        assert!(validate_credential_sources(&carryable).is_ok());
        assert!(validate_credential_sources(&[]).is_ok());
    }

    /// rc-ey6v + rc-9kgtm: one configured value derives BOTH the envelope
    /// channel capacity and the dispatcher semaphore (channel==semaphore
    /// invariant). Normalizes 0 to 1 — `mpsc::channel(0)` panics and a
    /// 0-permit semaphore would stall the consumer. Values above
    /// `tokio::sync::Semaphore::MAX_PERMITS` are rejected fail-closed
    /// because the dispatcher semaphore panics above that bound. Mirrors
    /// camel-http's `envelope_channel_capacity` tests (rc-3y6j).
    #[test]
    fn consumer_concurrency_limit_normalizes_zero_and_rejects_beyond_limit() {
        let l = tokio::sync::Semaphore::MAX_PERMITS;
        assert!(matches!(consumer_concurrency_limit(0), Ok(1)));
        assert!(matches!(consumer_concurrency_limit(1), Ok(1)));
        assert!(matches!(consumer_concurrency_limit(7), Ok(7)));
        assert!(matches!(consumer_concurrency_limit(l - 1), Ok(v) if v == l - 1));
        assert!(matches!(consumer_concurrency_limit(l), Ok(v) if v == l));
        match consumer_concurrency_limit(l + 1) {
            Err(CamelError::Config(msg)) => {
                assert!(msg.contains("consumerConcurrency"), "message was: {msg}");
                assert!(msg.contains(&format!("{}", l + 1)), "message was: {msg}");
                assert!(msg.contains(&format!("{l}")), "message was: {msg}");
            }
            other => panic!("expected CamelError::Config, got: {other:?}"),
        }
    }

    /// rc-9kgtm: the accepted upper bound is exactly the primitives' real
    /// bound — a semaphore of L permits and a bounded channel of capacity L
    /// must both construct without panicking.
    #[test]
    fn tokio_primitives_accept_exactly_the_limit() {
        let l = tokio::sync::Semaphore::MAX_PERMITS;
        let _semaphore = tokio::sync::Semaphore::new(l);
        let (_tx, _rx) = tokio::sync::mpsc::channel::<()>(l);
    }

    /// rc-9kgtm: `start()` rejects an oversized `consumer_concurrency`
    /// before registry insertion, listener binding, or readiness
    /// signalling — the port must remain free afterwards.
    #[tokio::test]
    async fn start_rejects_oversized_concurrency_before_registry_bind_or_readiness()
    -> Result<(), CamelError> {
        let l = tokio::sync::Semaphore::MAX_PERMITS;
        let unused_port = {
            let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| {
                CamelError::EndpointCreationFailed(format!("port probe bind failed: {e}"))
            })?;
            let port = probe
                .local_addr()
                .map_err(|e| {
                    CamelError::EndpointCreationFailed(format!("port probe local_addr failed: {e}"))
                })?
                .port();
            drop(probe);
            port
        };
        let (signal, receiver) = StartupSignal::pair();
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "concpanic-test".into())
            .with_startup(signal);
        let mut consumer = GrpcConsumer::new(
            "127.0.0.1".into(),
            unused_port,
            "/helloworld.Greeter/SayHello".into(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto"),
            "helloworld.Greeter".into(),
            "SayHello".into(),
            GrpcMode::Unary,
            Arc::new(camel_component_api::NoOpComponentContext),
            GrpcServerConfig::default(),
            l + 1,
        );

        let result = consumer.start(ctx).await;

        match result {
            Err(CamelError::Config(msg)) => {
                assert!(msg.contains("consumerConcurrency"), "message was: {msg}");
                assert!(msg.contains(&format!("{}", l + 1)), "message was: {msg}");
                assert!(msg.contains(&format!("{l}")), "message was: {msg}");
            }
            other => panic!("expected CamelError::Config, got: {other:?}"),
        }
        assert!(
            !GrpcServerRegistry::global().contains_server("127.0.0.1", unused_port)?,
            "registry must not contain a server entry for the rejected start"
        );
        match receiver.await_ready().await {
            Err(_) => {}
            Ok(()) => panic!("readiness must not be signalled for a rejected start"),
        }
        assert!(
            std::net::TcpListener::bind(("127.0.0.1", unused_port)).is_ok(),
            "port {unused_port} must remain free after the rejected start"
        );
        Ok(())
    }

    /// rc-9kgtm: `start_with_listener()` rejects an oversized
    /// `consumer_concurrency` before shared-server registry mutation or
    /// readiness signalling.
    #[tokio::test]
    async fn start_with_listener_rejects_oversized_concurrency_before_registry_mutation()
    -> Result<(), CamelError> {
        let l = tokio::sync::Semaphore::MAX_PERMITS;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| {
                CamelError::EndpointCreationFailed(format!("listener bind failed: {e}"))
            })?;
        let port = listener
            .local_addr()
            .map_err(|e| {
                CamelError::EndpointCreationFailed(format!("listener local_addr failed: {e}"))
            })?
            .port();
        let (signal, receiver) = StartupSignal::pair();
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ConsumerContext::new(tx, CancellationToken::new(), "concpanic-test".into())
            .with_startup(signal);
        let mut consumer = GrpcConsumer::new(
            "127.0.0.1".into(),
            port,
            "/helloworld.Greeter/SayHello".into(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto"),
            "helloworld.Greeter".into(),
            "SayHello".into(),
            GrpcMode::Unary,
            Arc::new(camel_component_api::NoOpComponentContext),
            GrpcServerConfig::default(),
            l + 1,
        );

        let result = consumer.start_with_listener(ctx, listener).await;

        match result {
            Err(CamelError::Config(msg)) => {
                assert!(msg.contains("consumerConcurrency"), "message was: {msg}");
                assert!(msg.contains(&format!("{}", l + 1)), "message was: {msg}");
                assert!(msg.contains(&format!("{l}")), "message was: {msg}");
            }
            other => panic!("expected CamelError::Config, got: {other:?}"),
        }
        assert!(
            !GrpcServerRegistry::global().contains_server("127.0.0.1", port)?,
            "registry must not contain a server entry for the rejected start"
        );
        match receiver.await_ready().await {
            Err(_) => {}
            Ok(()) => panic!("readiness must not be signalled for a rejected start"),
        }
        Ok(())
    }

    // ── rc-orr73: cancel-safe permit wait + UNAVAILABLE shutdown reply ────

    /// A free semaphore grants the permit immediately, well inside the
    /// timeout.
    #[tokio::test]
    async fn permit_wait_cancel_safe_grants_permit_when_free() {
        let sem = Arc::new(tokio::sync::Semaphore::new(1));
        let token = CancellationToken::new();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            acquire_permit_cancel_safe(sem, token),
        )
        .await
        .expect("free permit must be granted promptly");

        assert!(matches!(outcome, Ok(PermitWait::Granted(_))));
    }

    /// A saturated semaphore wait resolves `Cancelled` promptly once the
    /// token fires — the core rc-orr73 guarantee.
    #[tokio::test]
    async fn permit_wait_cancel_safe_returns_cancelled_on_token_cancel() {
        let sem = Arc::new(tokio::sync::Semaphore::new(1));
        let _held = sem.clone().acquire_owned().await.unwrap();
        let token = CancellationToken::new();

        let handle = tokio::spawn(acquire_permit_cancel_safe(sem, token.clone()));
        token.cancel();

        let outcome = tokio::time::timeout(std::time::Duration::from_millis(500), handle)
            .await
            .expect("cancelled wait must resolve promptly")
            .expect("task must not panic");

        assert!(matches!(outcome, Ok(PermitWait::Cancelled)));
    }

    /// Unary and ClientStreaming envelopes answer on the oneshot with an
    /// UNAVAILABLE `GrpcReply::Err`.
    #[tokio::test]
    async fn reply_unavailable_unary_and_client_streaming_send_reply_err() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        reply_unavailable(GrpcRequestEnvelope::Unary {
            metadata: tonic::metadata::MetadataMap::new(),
            body: Vec::new(),
            reply_tx: tx,
            kernel_principal: None,
        });
        assert!(matches!(
            rx.await,
            Ok(GrpcReply::Err(ref status)) if status.code() == tonic::Code::Unavailable
        ));

        let (_body_tx, body_rx) = mpsc::channel(1);
        let (tx, rx) = tokio::sync::oneshot::channel();
        reply_unavailable(GrpcRequestEnvelope::ClientStreaming {
            metadata: tonic::metadata::MetadataMap::new(),
            body_rx,
            reply_tx: tx,
            kernel_principal: None,
        });
        assert!(matches!(
            rx.await,
            Ok(GrpcReply::Err(ref status)) if status.code() == tonic::Code::Unavailable
        ));
    }

    /// Streaming envelopes get a best-effort `try_send` of a
    /// `GrpcStreamItem::Error`; a full channel is dropped silently instead
    /// of blocking the shutdown path.
    #[tokio::test]
    async fn reply_unavailable_streaming_try_sends_error_item() {
        let (tx, mut rx) = mpsc::channel::<GrpcStreamItem>(1);
        reply_unavailable(GrpcRequestEnvelope::ServerStreaming {
            metadata: tonic::metadata::MetadataMap::new(),
            body: Vec::new(),
            reply_tx: tx,
            kernel_principal: None,
        });
        assert!(matches!(
            rx.recv().await,
            Some(GrpcStreamItem::Error(ref status)) if status.code() == tonic::Code::Unavailable
        ));

        let (_body_tx, body_rx) = mpsc::channel(1);
        let (tx, mut rx) = mpsc::channel::<GrpcStreamItem>(1);
        reply_unavailable(GrpcRequestEnvelope::Bidi {
            metadata: tonic::metadata::MetadataMap::new(),
            body_rx,
            reply_tx: tx,
            kernel_principal: None,
        });
        assert!(matches!(
            rx.recv().await,
            Some(GrpcStreamItem::Error(ref status)) if status.code() == tonic::Code::Unavailable
        ));

        // Full channel: `reply_unavailable` must return synchronously
        // without panicking, dropping the reply best-effort.
        let (tx, mut rx) = mpsc::channel::<GrpcStreamItem>(1);
        tx.send(GrpcStreamItem::Done).await.unwrap();
        reply_unavailable(GrpcRequestEnvelope::ServerStreaming {
            metadata: tonic::metadata::MetadataMap::new(),
            body: Vec::new(),
            reply_tx: tx,
            kernel_principal: None,
        });
        assert!(matches!(rx.recv().await, Some(GrpcStreamItem::Done)));
        assert!(rx.recv().await.is_none());
    }

    // ── Task 1.2 (rc-orr73): identity-owned dispatch registration guard ──

    /// Dropping an armed guard removes its own registration from the
    /// dispatch table. Removal is observed by polling under a deadline
    /// because the contended-lock fallback defers removal to a spawned
    /// task.
    #[tokio::test]
    async fn dispatch_guard_drop_removes_owned_entry() {
        let table: GrpcDispatchTable =
            Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
        let path = "/t/S/M".to_string();
        let (tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        table.write().await.insert(
            path.clone(),
            (tx.clone(), GrpcMode::Unary, None, CancellationToken::new()),
        );

        let guard = DispatchRegistrationGuard::arm(
            table.clone(),
            path.clone(),
            tx,
            CancellationToken::new(),
        );
        drop(guard);

        let removal = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while table.read().await.contains_key(&path) {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await; // allow-test-sleep: poll for spawned drop-cleanup task
            }
        })
        .await;
        assert!(
            removal.is_ok() && !table.read().await.contains_key(&path),
            "dispatch entry for {path} must be removed within 1s"
        );
    }

    /// `cleanup` removes only the entry the guard itself owns: a
    /// replacement registration on the same path (different channel,
    /// Task 1.3's insert) survives, and a disarmed second cleanup is a
    /// no-op.
    #[tokio::test]
    async fn dispatch_guard_cleanup_spares_replacement_entry() {
        let table: GrpcDispatchTable =
            Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
        let path = "/t/S/M".to_string();
        let (tx1, _rx1) = mpsc::channel::<GrpcRequestEnvelope>(1);
        let (tx2, _rx2) = mpsc::channel::<GrpcRequestEnvelope>(1);

        let mut guard = DispatchRegistrationGuard::arm(
            table.clone(),
            path.clone(),
            tx1,
            CancellationToken::new(),
        );
        table.write().await.insert(
            path.clone(),
            (tx2.clone(), GrpcMode::Unary, None, CancellationToken::new()),
        );

        guard.cleanup().await;

        {
            let table = table.read().await;
            let entry = table
                .get(&path)
                .expect("replacement entry must survive cleanup of the stale owner");
            assert!(
                entry.0.same_channel(&tx2),
                "surviving entry must be the replacement's channel"
            );
        }

        // Disarmed: the second cleanup changes nothing.
        guard.cleanup().await;
        let table = table.read().await;
        assert!(
            table
                .get(&path)
                .is_some_and(|entry| entry.0.same_channel(&tx2)),
            "disarmed cleanup must not touch the replacement entry"
        );
    }

    /// Task 1.3 (rc-orr73): `insert_dispatch_entry` replaces a stale
    /// entry whose sender is closed (receiver dropped after a forced
    /// abort / runtime teardown) but still rejects a live duplicate.
    #[tokio::test]
    async fn dispatch_insert_replaces_closed_sender_entry() {
        let table: GrpcDispatchTable =
            Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
        let path = "/t/S/M".to_string();

        // Stale entry: its receiver was dropped, so `tx.is_closed()` is true.
        let (stale_tx, stale_rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        drop(stale_rx);
        assert!(stale_tx.is_closed());
        table.write().await.insert(
            path.clone(),
            (stale_tx, GrpcMode::Unary, None, CancellationToken::new()),
        );

        // A fresh live sender replaces the closed entry.
        let (fresh_tx, _fresh_rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        {
            let mut guard = table.write().await;
            insert_dispatch_entry(
                &mut guard,
                &path,
                fresh_tx.clone(),
                GrpcMode::Unary,
                None,
                CancellationToken::new(),
            )
            .expect("closed stale entry must be replaceable");
        }
        {
            let table = table.read().await;
            let entry = table
                .get(&path)
                .expect("entry must exist after replacement");
            assert!(
                entry.0.same_channel(&fresh_tx),
                "stored entry must be the fresh sender, not the stale one"
            );
        }

        // A live entry still fails as a duplicate, entry unchanged.
        let (other_tx, _other_rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        let mut guard = table.write().await;
        let err = insert_dispatch_entry(
            &mut guard,
            &path,
            other_tx,
            GrpcMode::Unary,
            None,
            CancellationToken::new(),
        )
        .expect_err("live duplicate registration must be rejected");
        assert!(err.to_string().contains("duplicate"), "message was: {err}");
        assert!(
            guard
                .get(&path)
                .is_some_and(|entry| entry.0.same_channel(&fresh_tx)),
            "rejected duplicate must leave the live entry unchanged"
        );
    }

    /// rc-qq8zz: `cleanup` removes the entry AND cancels its token, so the
    /// transport's open streaming calls observe a graceful-stop teardown.
    #[tokio::test]
    async fn dispatch_guard_cancels_token_on_cleanup() {
        let table: GrpcDispatchTable =
            Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
        let path = "/t/S/M".to_string();
        let (tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        let token = CancellationToken::new();
        table.write().await.insert(
            path.clone(),
            (tx.clone(), GrpcMode::Unary, None, token.clone()),
        );

        let mut guard =
            DispatchRegistrationGuard::arm(table.clone(), path.clone(), tx, token.clone());
        guard.cleanup().await;

        assert!(token.is_cancelled(), "cleanup must cancel the entry token");
        let removal = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while table.read().await.contains_key(&path) {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await; // allow-test-sleep: poll for removal
            }
        })
        .await;
        assert!(
            removal.is_ok() && !table.read().await.contains_key(&path),
            "dispatch entry for {path} must be removed within 1s of cleanup"
        );
    }

    /// rc-qq8zz: Drop cancels the token as its FIRST statement — before the
    /// armed check and the runtime-conditional removal — so the entry token
    /// is cancelled synchronously even on a no-runtime teardown.
    #[tokio::test]
    async fn dispatch_guard_cancels_token_on_drop() {
        let table: GrpcDispatchTable =
            Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
        let path = "/t/S/M".to_string();
        let (tx, _rx) = mpsc::channel::<GrpcRequestEnvelope>(1);
        let token = CancellationToken::new();
        table.write().await.insert(
            path.clone(),
            (tx.clone(), GrpcMode::Unary, None, token.clone()),
        );

        let guard = DispatchRegistrationGuard::arm(table.clone(), path.clone(), tx, token.clone());

        drop(guard);

        // No await needed: cancel is the first statement of `drop`.
        assert!(
            token.is_cancelled(),
            "drop must cancel the entry token synchronously"
        );

        let removal = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while table.read().await.contains_key(&path) {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await; // allow-test-sleep: poll for spawned drop-cleanup task
            }
        })
        .await;
        assert!(
            removal.is_ok() && !table.read().await.contains_key(&path),
            "dispatch entry for {path} must be removed within 1s of drop"
        );
    }
}
