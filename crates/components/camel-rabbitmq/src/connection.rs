//! RabbitMQ connection manager: owns one lapin `Connection` per named broker,
//! re-establishes it with the shared [`NetworkRetryPolicy`], and stamps a
//! monotonically increasing generation counter for stale delivery-tag
//! suppression.
//!
//! lapin's built-in recovery stays disabled: reconnection is driven by
//! [`retry_async_cancelable`] under an outer cancellation race. When a
//! registration-time lifecycle context is bound, each reconnect derives a
//! manager-local child of the CURRENT runtime shutdown token, so a runtime
//! stop cancels a pending attempt; otherwise no token is minted and Drop aborts
//! the tracked tasks. The broker URL is never rendered in the clear — every
//! diagnostic goes through [`camel_api::redact::redact_url`].

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use camel_api::redact::redact_url;
use camel_component_api::{
    CamelError, ComponentContext, NetworkRetryPolicy, retry_async, retry_async_cancelable,
};
use futures::StreamExt;
use tokio::sync::{RwLock, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::RabbitBrokerConfig;
use crate::error::RabbitError;

/// Producer bound for [`RabbitConnectionManager::connection_within`] while the
/// broker is disconnected: fail the publish bounded, never hang (task 1.4).
pub const PUBLISH_DISCONNECTED_BOUND: Duration = Duration::from_secs(2);

/// Bound on `channel.open` for [`RabbitConnectionManager::consumer_channel`].
///
/// A live connection answers `channel.open` promptly; this only fires on a
/// half-dead connection and keeps consumer start from hanging. It follows the
/// manager's bounded-wait convention (the consumer's own connect wait is
/// bounded separately by `CONSUMER_START_BOUND`).
const CHANNEL_OPEN_BOUND: Duration = Duration::from_secs(10);

/// Boxed connect future produced by a [`ConnectFn`].
pub type ConnectFuture =
    Pin<Box<dyn Future<Output = Result<lapin::Connection, lapin::Error>> + Send>>;

/// Connect seam: production installs the real [`lapin::Connection::connect`]
/// adapter, tests install a deterministic fake.
pub type ConnectFn = Arc<dyn Fn(&str) -> ConnectFuture + Send + Sync>;

/// Watched connection lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnStatus {
    /// No live connection (initial state, and after an exhausted reconnect).
    Disconnected,
    /// A single connect flight owns the transition.
    Connecting,
    /// A live connection is stored and handed to producers/consumers.
    Connected,
}

/// Classify a lapin error as connection-scoped (full reconnect) or
/// channel-scoped (recreate the channel only).
///
/// A channel-scoped error closes a single channel but leaves the connection
/// healthy: AMQP *soft* protocol errors such as 404 NOT_FOUND (missing
/// exchange) and 406 PRECONDITION_FAILED. Every other error — IO/runtime
/// shutdown, AMQP *hard* (connection) protocol errors, a missed heartbeat, or
/// any error kind that is not provably channel-local — is treated as
/// connection-scoped (fail-safe: a spurious reconnect is recoverable, whereas
/// ignoring a dead connection is not).
///
/// lapin 4.12 delivers channel-close errors to `Connection::events_listener()`
/// as `Event::Error`, so both the producer and the listener MUST apply this
/// classification or a channel 404 would tear down a healthy connection.
pub(crate) fn is_connection_level_error(error: &lapin::Error) -> bool {
    !error.is_amqp_soft_error()
}

#[derive(Default)]
struct ManagerState {
    conn: Option<Arc<lapin::Connection>>,
    generation: u64,
}

impl ManagerState {
    /// Clear the stored connection only when it still belongs to `generation`.
    ///
    /// A successful reconnect increments `generation`; a failure operation that
    /// captured an earlier generation is stale and MUST NOT erase the newer
    /// connection it would otherwise clobber (master-amended task 1.3 finding
    /// (b); workspace aws-lc scope tracked separately in bd rc-2bofp).
    /// Returns `true` when the slot was cleared.
    fn clear_if_generation(&mut self, generation: u64) -> bool {
        if self.generation == generation {
            self.conn = None;
            true
        } else {
            false
        }
    }
}

/// Handles of detached reconnect owners and error listeners.
///
/// When a lifecycle context is bound, reconnects derive a manager-local child
/// token from the current runtime shutdown token and stop on it; the registry
/// still lets Drop abort any straggler. With no bound context the manager
/// mints no token at all (a fresh root would be a banned lifecycle root) and
/// relies solely on aborting these handles to drop pending attempts.
#[derive(Default)]
struct TaskRegistry {
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl TaskRegistry {
    /// Record a spawned task, pruning already-finished handles so the registry
    /// does not grow across reconnects.
    fn track(&self, handle: JoinHandle<()>) {
        let mut handles = self.handles.lock().unwrap_or_else(PoisonError::into_inner);
        handles.retain(|handle| !handle.is_finished());
        handles.push(handle);
    }

    /// Abort every tracked task, draining the registry. Aborting drops the
    /// task future, so a pending connect attempt or error listener is dropped.
    fn abort_all(&self) {
        let mut handles = self.handles.lock().unwrap_or_else(PoisonError::into_inner);
        for handle in handles.drain(..) {
            handle.abort();
        }
    }
}

/// Owns the lapin connection for one resolved broker.
pub struct RabbitConnectionManager {
    url: String,
    inner: RwLock<ManagerState>,
    /// Canonical published connection generation — the authoritative
    /// lock-free snapshot read by [`Self::current_generation`]. `0` is the
    /// reserved "no connection published yet" value, never a lock-contention
    /// fallback. Updated only when a real connection is published, under the
    /// state write lock and the failure-flight mutex.
    generation: AtomicU64,
    /// Serializes the failure fence (generation check + `Connected ->
    /// Disconnected` transition) against connection publication (`Connecting ->
    /// Connected` + generation store), so a late failure for a replaced
    /// connection can never demote its successor (reviewer Race 1).
    failure_flight: Mutex<()>,
    retry: NetworkRetryPolicy,
    connect_fn: ConnectFn,
    status_tx: watch::Sender<ConnStatus>,
    /// Registration-time lifecycle supplier. When present, each reconnect
    /// derives a manager-local child of the CURRENT runtime shutdown token so
    /// a runtime stop cancels pending attempts; when absent the manager mints
    /// no token and relies on [`TaskRegistry`] abort-on-Drop instead.
    context: Option<Arc<dyn ComponentContext>>,
    /// Detached tasks to abort on Drop (see [`TaskRegistry`]).
    tasks: Arc<TaskRegistry>,
    /// The manager-local child token of the most recent flight. Cancelled on
    /// Drop; cancelling it never cancels the parent runtime token.
    local_cancel: Mutex<Option<CancellationToken>>,
}

impl RabbitConnectionManager {
    /// Build a manager from an explicit URL, retry policy and connect seam.
    pub fn new(url: impl Into<String>, retry: NetworkRetryPolicy, connect_fn: ConnectFn) -> Self {
        // The initial receiver is dropped: `send_if_modified`/`send_replace`
        // still update the watched value with zero receivers, and callers get a
        // fresh view via `subscribe()`.
        let (status_tx, _status_rx) = watch::channel(ConnStatus::Disconnected);
        Self {
            url: url.into(),
            inner: RwLock::new(ManagerState::default()),
            generation: AtomicU64::new(0),
            failure_flight: Mutex::new(()),
            retry,
            connect_fn,
            status_tx,
            context: None,
            tasks: Arc::new(TaskRegistry::default()),
            local_cancel: Mutex::new(None),
        }
    }

    /// Bind a registration-time lifecycle context.
    ///
    /// The bundle hands in a slot-bound context
    /// (`RegistryComponentContext::with_shutdown_slot`) whose
    /// `shutdown_token()` resolves the CURRENT runtime token per call: after a
    /// stop/start cycle the derived child is fresh, never a stale cancelled
    /// one. Reconnects derive a child at activation, so a runtime stop cancels
    /// a pending attempt while the manager stays alive for the next start.
    pub fn with_lifecycle_context(mut self, context: Arc<dyn ComponentContext>) -> Self {
        self.context = Some(context);
        self
    }

    /// Build a manager for a configured broker, resolving credentials/vhost
    /// into the URL and installing the real lapin connect adapter.
    pub fn from_broker_config(broker: &RabbitBrokerConfig, retry: NetworkRetryPolicy) -> Self {
        Self::new(
            resolve_broker_url(broker),
            retry,
            Arc::new(connect_with_lapin),
        )
    }

    /// Establish the connection, delegating to the detached single-flight owner.
    ///
    /// The detached task always owns the transition, even when this is the
    /// first caller to request it. Dropping this future (an abandoned caller)
    /// therefore only abandons the waiter, never the owner, so `Connecting`
    /// cannot wedge. Returns `Ok` once a connection is stored and `Err` if the
    /// in-flight attempt is exhausted.
    pub async fn connect(self: &Arc<Self>) -> Result<(), CamelError> {
        self.ensure_connecting();
        self.wait_until_settled().await
    }

    /// Kick off a detached reconnect when the watched status is `Disconnected`.
    ///
    /// The task is raced against a manager-local child of the CURRENT runtime
    /// shutdown token when a lifecycle context is bound (a runtime stop
    /// cancels the pending attempt), and is tracked for abort-on-Drop when no
    /// token is bound. The detached task captures only a [`Weak`] plus cloned
    /// inputs, so no strong manager reference survives the retry.
    pub fn ensure_connecting(self: &Arc<Self>) {
        if !self.claim_flight() {
            return;
        }
        let cancel = self.shutdown_child();
        if let Some(child) = &cancel {
            *self
                .local_cancel
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(child.clone());
        }
        let handle = tokio::spawn({
            let weak = Arc::downgrade(self);
            let url = self.url.clone();
            let retry = self.retry.clone();
            let connect_fn = Arc::clone(&self.connect_fn);
            let status_tx = self.status_tx.clone();
            let tasks = Arc::clone(&self.tasks);
            async move {
                let _ = run_reconnect(weak, url, retry, connect_fn, cancel, status_tx, tasks).await;
            }
        });
        self.tasks.track(handle);
    }

    /// Derive the manager-local child of the current runtime shutdown token.
    ///
    /// Resolves the parent per activation (never caches it) so a runtime
    /// stop/start cycle picks up the fresh slot token. Returns `None` when no
    /// lifecycle context is bound or the context is unbound
    /// (`ControllerComponentContext` keeps the `None` default) — the caller
    /// then relies on abort-on-Drop and MUST NOT mint a root token.
    pub(crate) fn shutdown_child(&self) -> Option<CancellationToken> {
        let parent = self.context.as_ref()?.shutdown_token()?;
        Some(parent.child_token())
    }

    /// Bounded wait for a live connection plus its generation counter.
    pub async fn connection_within(
        self: &Arc<Self>,
        bound: Duration,
    ) -> Result<(Arc<lapin::Connection>, u64), CamelError> {
        self.ensure_connecting();

        let mut rx = self.status_tx.subscribe();
        let wait = async {
            loop {
                let status = *rx.borrow_and_update();
                match status {
                    ConnStatus::Connected => return Ok(()),
                    ConnStatus::Disconnected => return Err(()),
                    ConnStatus::Connecting => {
                        if rx.changed().await.is_err() {
                            return Err(());
                        }
                    }
                }
            }
        };
        match tokio::time::timeout(bound, wait).await {
            Ok(Ok(())) => {}
            Ok(Err(())) | Err(_) => return Err(self.disconnected_error()),
        }

        let state = self.inner.read().await;
        match state.conn.as_ref() {
            Some(conn) => Ok((Arc::clone(conn), state.generation)),
            None => Err(self.disconnected_error()),
        }
    }

    /// Open a channel on the live connection and return its origin identity.
    ///
    /// The connection `Arc`, its generation, and the channel are captured
    /// together under one read lock so a caller can never pair a channel with a
    /// generation/connection from a different publication: the task 2.5
    /// stale-delivery-tag guard compares this generation against
    /// [`Self::current_generation`], and the consumer engine inspects the
    /// returned connection's status to tell a channel-local termination on a
    /// healthy connection from actual connection death — a torn read would
    /// silently defeat both. A `channel.open` failure is classified internally
    /// against the captured origin (see [`Self::handle_channel_open_failure`]),
    /// so the manager is demoted only when that origin is actually dead. The
    /// `channel.open` is bounded so a half-dead connection cannot hang consumer
    /// start.
    pub async fn consumer_channel(
        self: &Arc<Self>,
    ) -> Result<(lapin::Channel, u64, Arc<lapin::Connection>), CamelError> {
        let (conn, generation) = {
            let state = self.inner.read().await;
            match state.conn.as_ref() {
                Some(conn) => (Arc::clone(conn), state.generation),
                None => return Err(self.disconnected_error()),
            }
        };
        let channel = match tokio::time::timeout(CHANNEL_OPEN_BOUND, conn.create_channel()).await {
            Ok(Ok(channel)) => channel,
            Ok(Err(error)) => {
                // Classify against the CAPTURED origin connection/generation —
                // never the current one — and demote only a dead origin.
                self.handle_channel_open_failure(generation, conn.status().connected());
                return Err(RabbitError::PublishFailed(format!(
                    "rabbitmq create_channel on {} failed: {error}",
                    redact_url(&self.url)
                ))
                .into());
            }
            Err(_) => {
                self.handle_channel_open_failure(generation, conn.status().connected());
                return Err(RabbitError::PublishFailed(format!(
                    "rabbitmq create_channel on {} timed out after {CHANNEL_OPEN_BOUND:?}",
                    redact_url(&self.url)
                ))
                .into());
            }
        };
        Ok((channel, generation, conn))
    }

    /// The current connection generation (stale-tag guard).
    ///
    /// This is an authoritative lock-free snapshot of the canonical published
    /// generation: it never masks a valid generation behind lock contention.
    /// The old `try_read` accessor returned `0` while a reconnect writer held
    /// the state lock, which judged a valid disposition stale and dropped its
    /// ack forever. `0` is reserved for "no connection published yet".
    pub fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Report a failure of the connection published as `generation`.
    ///
    /// The generation fence and the `Connected -> Disconnected` transition run
    /// under the failure-flight mutex, mutually exclusive with publication, so
    /// a late failure for a connection that has already been replaced is
    /// ignored without touching the successor's status, slot, or reconnect
    /// state (reviewer Race 1). The reconnect start is serialized with the
    /// demotion under the same mutex.
    pub fn note_failure_for_generation(self: &Arc<Self>, generation: u64) {
        let _flight = self
            .failure_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.demote_locked(generation) {
            self.ensure_connecting();
        }
    }

    /// Report a failed channel-open attempt against its ORIGIN connection.
    ///
    /// The caller reads `origin_generation` and `origin_connected` from the
    /// exact connection the `channel.open` was attempted on BEFORE the await,
    /// so a failure can never be mis-tagged with a generation that a concurrent
    /// reconnect published in the meantime. Only an actually dead origin is
    /// demoted: a soft/transient failure on a still-healthy connection must not
    /// invalidate the shared connection. A late report for an origin that has
    /// since been replaced no-ops inside the generation fence.
    ///
    /// Never substitutes the current generation: an old attempt must not be
    /// re-labelled with a newer publication.
    fn handle_channel_open_failure(
        self: &Arc<Self>,
        origin_generation: u64,
        origin_connected: bool,
    ) {
        if !origin_connected {
            self.note_failure_for_generation(origin_generation);
        }
    }

    /// Mark the stored connection dead and trigger a reconnect.
    ///
    /// Unqualified external-force entry: it targets whatever connection is
    /// currently published. Callers that know the originating connection
    /// generation MUST use [`Self::note_failure_for_generation`] so a late
    /// failure cannot demote a replacement.
    pub fn note_failure(self: &Arc<Self>) {
        let generation = self.current_generation();
        let _flight = self
            .failure_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.demote_locked(generation);
        self.ensure_connecting();
    }

    /// Demote the connection published as `generation`, without re-arming.
    ///
    /// Used by the error listener's parent-cancel path: against an already
    /// cancelled parent a reconnect would only spawn a doomed task, so
    /// re-arming is left to the next activation. Returns `true` when this call
    /// won the demotion.
    fn demote_connection_for_generation(self: &Arc<Self>, generation: u64) -> bool {
        let _flight = self
            .failure_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.demote_locked(generation)
    }

    /// Core demotion, assuming the failure-flight mutex is held.
    ///
    /// Fences on the canonical generation first, then claims the
    /// `Connected -> Disconnected` transition. The state lock is an async guard
    /// and is never awaited while the std failure-flight mutex is held: a
    /// `try_write` miss defers the clear to a spawned task that re-checks the
    /// generation under the write lock, so an old generation's deferred clear
    /// can never flip the status or start a replacement retry after a new
    /// connection is published.
    fn demote_locked(self: &Arc<Self>, generation: u64) -> bool {
        if self.generation.load(Ordering::Acquire) != generation {
            return false;
        }
        let claimed = self.status_tx.send_if_modified(|status| {
            if *status == ConnStatus::Connected {
                *status = ConnStatus::Disconnected;
                true
            } else {
                false
            }
        });
        if !claimed {
            return false;
        }
        if let Ok(mut state) = self.inner.try_write() {
            let _ = state.clear_if_generation(generation);
        } else {
            // The state lock is held by a reader/writer; clear asynchronously,
            // guarded by `generation` so a newer connection cannot be erased.
            let weak = Arc::downgrade(self);
            tokio::spawn(async move {
                if let Some(manager) = weak.upgrade() {
                    let _ = manager.apply_failure_clear(generation).await;
                }
            });
        }
        true
    }

    /// Test seam: publish a generation/status without a real
    /// `lapin::Connection` (which cannot be constructed off-broker). Keeps the
    /// canonical atomic and the state mirror consistent, exactly as a real
    /// publication does.
    #[cfg(test)]
    async fn seed_published_state(&self, generation: u64, status: ConnStatus) {
        {
            let mut state = self.inner.write().await;
            state.generation = generation;
        }
        self.generation.store(generation, Ordering::Release);
        let _ = self.status_tx.send_replace(status);
    }

    /// Atomically claim the reconnect flight.
    ///
    /// `send_if_modified` is atomic and works with zero receivers, so the sync
    /// `ensure_connecting`/`note_failure` entry points can claim the flight
    /// without awaiting the async state lock.
    fn claim_flight(&self) -> bool {
        self.status_tx.send_if_modified(|status| {
            if *status == ConnStatus::Disconnected {
                *status = ConnStatus::Connecting;
                true
            } else {
                false
            }
        })
    }

    /// Apply a failure clear captured at `observed` generation.
    ///
    /// Returns `true` when the dead connection was cleared. A mismatch means a
    /// successful reconnect published a newer connection that the stale failure
    /// must not erase.
    async fn apply_failure_clear(&self, observed: u64) -> bool {
        let mut state = self.inner.write().await;
        state.clear_if_generation(observed)
    }

    async fn wait_until_settled(&self) -> Result<(), CamelError> {
        let mut rx = self.status_tx.subscribe();
        loop {
            let status = *rx.borrow_and_update();
            match status {
                ConnStatus::Connected => return Ok(()),
                ConnStatus::Disconnected => return Err(self.disconnected_error()),
                ConnStatus::Connecting => {
                    if rx.changed().await.is_err() {
                        return Err(self.disconnected_error());
                    }
                }
            }
        }
    }

    fn disconnected_error(&self) -> CamelError {
        RabbitError::Disconnected.into()
    }
}

impl fmt::Debug for RabbitConnectionManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RabbitConnectionManager")
            .field("url", &redact_url(&self.url))
            .field("retry", &self.retry)
            .field("status", &*self.status_tx.borrow())
            .finish_non_exhaustive()
    }
}

impl Drop for RabbitConnectionManager {
    fn drop(&mut self) {
        // Cancel only the manager-local child token — never the parent runtime
        // token. With no bound context there is no token; aborting the tracked
        // tasks drops any pending attempt / listener instead.
        if let Some(child) = self
            .local_cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            child.cancel();
        }
        self.tasks.abort_all();
    }
}

/// Run the connect retry core with no strong manager reference.
///
/// When `cancel` is `Some` (a bound lifecycle context derived a child token),
/// the whole retry future is raced against cancellation so a pending connect
/// attempt is dropped immediately. With `cancel` `None` the non-cancelable
/// core runs and Drop aborts the task instead. Either way the weak manager
/// reference is upgraded only *after* the result, briefly, to publish
/// connection/generation/status. A cancelled or failed flight always resets
/// the watched status to `Disconnected`, so a future runtime restart is never
/// stranded in `Connecting`.
async fn run_reconnect(
    weak: Weak<RabbitConnectionManager>,
    url: String,
    retry: NetworkRetryPolicy,
    connect_fn: ConnectFn,
    cancel: Option<CancellationToken>,
    status_tx: watch::Sender<ConnStatus>,
    tasks: Arc<TaskRegistry>,
) -> Result<(), CamelError> {
    let redacted = redact_url(&url);
    let result: Result<lapin::Connection, CamelError> = match cancel.as_ref() {
        Some(token) => tokio::select! {
            biased;
            _ = token.cancelled() => {
                let msg = format!("rabbitmq connect to {redacted} cancelled");
                Err(RabbitError::PublishFailed(msg).into())
            }
            result = retry_async_cancelable(
                &retry,
                "rabbitmq",
                "connect",
                || connect_fn(&url),
                |_: &lapin::Error| true,
                token,
                None,
            ) => result.map_err(|err| RabbitError::PublishFailed(format!(
                "rabbitmq connect to {redacted} failed: {err}"
            )).into()),
        },
        None => retry_async(
            &retry,
            "rabbitmq",
            "connect",
            || connect_fn(&url),
            |_: &lapin::Error| true,
            None,
        )
        .await
        .map_err(|err| {
            RabbitError::PublishFailed(format!("rabbitmq connect to {redacted} failed: {err}"))
                .into()
        }),
    };

    let Some(manager) = weak.upgrade() else {
        return result.map(|_| ());
    };

    match result {
        Ok(conn) => {
            let conn = Arc::new(conn);
            let published_generation = {
                // The state write lock is acquired BEFORE the std failure-flight
                // mutex (never held across an await): publication and failure
                // demotion are mutually exclusive, so a late failure for the
                // replaced connection cannot interleave between the generation
                // store and the `Connected` broadcast.
                let mut state = manager.inner.write().await;
                let _flight = manager
                    .failure_flight
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                state.conn = Some(Arc::clone(&conn));
                state.generation = state.generation.wrapping_add(1);
                let generation = state.generation;
                manager.generation.store(generation, Ordering::Release);
                let _ = status_tx.send_replace(ConnStatus::Connected);
                generation
            };
            let listener =
                spawn_error_listener(Arc::downgrade(&manager), conn, published_generation, cancel);
            tasks.track(listener);
            Ok(())
        }
        Err(err) => {
            {
                let mut state = manager.inner.write().await;
                state.conn = None;
            }
            let _ = status_tx.send_replace(ConnStatus::Disconnected);
            Err(err)
        }
    }
}

/// Watch a live connection for broker-side failures and mark it failed.
///
/// lapin 4 has no `Connection::on_error`; connection-level errors are delivered
/// through [`lapin::Connection::events_listener`] as [`lapin::Event::Error`].
/// A closed event stream is treated the same as an error: in both cases the
/// live connection is gone and recovery belongs to the retry policy. The
/// listener owns only a [`Weak`] manager so it never keeps the manager alive,
/// and stops on the manager-local child token when one is bound (otherwise the
/// caller aborts its handle on Drop). On parent cancel it demotes the live
/// connection, settling `Disconnected` so a later restart re-arms, without
/// spawning a reconnect against the cancelled parent.
///
/// `generation` is the published generation of `conn`: every failure report is
/// qualified with it so a listener that fires after the connection has been
/// replaced cannot demote its successor (reviewer Race 1).
fn spawn_error_listener(
    weak: Weak<RabbitConnectionManager>,
    conn: Arc<lapin::Connection>,
    generation: u64,
    cancel: Option<CancellationToken>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut events = Box::pin(conn.events_listener());
        loop {
            tokio::select! {
                biased;
                _ = wait_cancelled(&cancel) => {
                    // Runtime parent cancel stops the runtime: demote the
                    // now-dead connection and settle `Disconnected`, but do NOT
                    // re-arm. The parent is cancelled, so a reconnect would
                    // only spawn a doomed task; the next activation (fresh
                    // parent after a restart) re-arms through
                    // `ensure_connecting`.
                    if let Some(manager) = weak.upgrade() {
                        manager.demote_connection_for_generation(generation);
                    }
                    break;
                }
                event = events.next() => match event {
                    // A connection-level error means the live connection is
                    // gone: mark it failed so the retry policy owns recovery.
                    Some(lapin::Event::Error(error)) => {
                        if is_connection_level_error(&error) {
                            if let Some(manager) = weak.upgrade() {
                                manager.note_failure_for_generation(generation);
                            }
                            break;
                        }
                        // A channel-scoped (soft AMQP) error closed only a
                        // channel: the connection stays live, so keep watching
                        // (the producer recreates its channel). Breaking here
                        // would reintroduce a full reconnect for a 404/406.
                        tracing::debug!(
                            "RabbitMQ channel-level error; retaining broker connection"
                        );
                    }
                    // A closed event stream means the live connection is gone.
                    None => {
                        if let Some(manager) = weak.upgrade() {
                            manager.note_failure_for_generation(generation);
                        }
                        break;
                    }
                    Some(_) => {}
                },
            }
        }
    })
}

/// Await cancellation, or never complete when no token is bound.
async fn wait_cancelled(cancel: &Option<CancellationToken>) {
    match cancel {
        Some(token) => token.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

/// The real connect adapter installed by [`RabbitConnectionManager::from_broker_config`].
fn connect_with_lapin(url: &str) -> ConnectFuture {
    let url = url.to_string();
    Box::pin(async move {
        lapin::Connection::connect(&url, lapin::ConnectionProperties::default()).await
    })
}

/// Resolve a broker's connection URL: explicit credentials and vhost override
/// (and complete) whatever the base URL carries.
fn resolve_broker_url(broker: &RabbitBrokerConfig) -> String {
    let Ok(mut parsed) = url::Url::parse(&broker.url) else {
        return broker.url.clone();
    };
    if let Some(username) = &broker.username {
        let _ = parsed.set_username(username);
    }
    if let Some(password) = &broker.password {
        let _ = parsed.set_password(Some(password.expose()));
    }
    if let Some(vhost) = &broker.vhost {
        parsed.set_path(vhost);
    }
    parsed.to_string()
}

/// Reuse the integration fixture (`tests/common`) from the in-crate regression
/// so it can read the manager's private connection/generation/status seam. The
/// module is test-only and never compiled into the shipped crate. `pub(crate)`
/// lets the producer's mandatory-return regression lease the same fixture
/// without compiling it twice.
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
pub(crate) mod docker_fixture;

#[cfg(test)]
mod tests;
