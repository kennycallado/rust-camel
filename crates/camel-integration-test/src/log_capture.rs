//! Process-global log capture for the scenario tier's document-level
//! `logs:` assertions (rc-tdgh5).
//!
//! The composition root installs a global tracing subscriber at boot,
//! unconditionally, first-wins with warn-and-skip on loss
//! (`CamelConfig::configure_context_with_beans`; boot is
//! caller-owned). A scenario driver that wants `logs:` assertions
//! therefore claims the process's subscriber seat BEFORE the boot:
//! [`ensure_capture_subscriber`] installs a registry carrying
//! [`CaptureLayer`] through the same first-wins `try_init`.
//!
//! When the harness owns the seat, every event flows through
//! [`CaptureLayer`], which appends each event to every open capture
//! window whose `[opened_at, now)` interval contains the event
//! timestamp — conservative attribution: a window opened after an
//! event never sees it. Windows are process-global: [`WindowHandle`]
//! registers a buffer in a static registry at open and unregisters at
//! close (RAII on drop); the runner owns open/evaluate/close around
//! the document run. Each buffer is capped at [`LOG_WINDOW_CAP`]
//! events, drop-oldest with a head marker naming the truncation.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use tracing::Level;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;

/// One captured tracing event: when it fired (monotonic, for window
/// attribution), at what level, from which target, with which rendered
/// message.
#[derive(Debug, Clone)]
pub struct LogEvent {
    /// When the event reached the capture layer (monotonic clock).
    pub at: Instant,
    /// The event's level.
    pub level: Level,
    /// The event's target — `module_path!` of the emit site, for the
    /// camel-log component's rendered exchanges:
    /// `camel_component_log`.
    pub target: String,
    /// The rendered `message` field (empty when the event carries
    /// none). camel-log's exchange lines arrive as composites, for
    /// example `[marker] Body: <body>`.
    pub message: String,
}

/// Events kept per window before the oldest drop: drop-oldest with a
/// head marker naming the truncation (the marker sits at TRACE level,
/// so it can never trip a `noLevelAbove` clause).
pub const LOG_WINDOW_CAP: usize = 10_000;

/// Target identifying the in-band truncation marker event.
const MARKER_TARGET: &str = "camel_integration_test::log_capture";

/// One registered window: identity, open timestamp (the attribution
/// boundary), and the shared buffer.
struct WindowEntry {
    id: u64,
    opened_at: Instant,
    buffer: Arc<Mutex<Vec<LogEvent>>>,
}

/// Process-global registry of open windows. Attribution takes this
/// registry lock, then the target buffer's.
static WINDOWS: Mutex<Vec<WindowEntry>> = Mutex::new(Vec::new());
/// Monotonic window identity.
static NEXT_ID: AtomicU64 = AtomicU64::new(0);
/// Whether the harness's capture subscriber owns the process's tracing
/// seat (it won the first-wins `try_init`).
static OWN_INSTALL: AtomicBool = AtomicBool::new(false);

/// An open capture window: identity plus the shared buffer.
pub struct WindowHandle {
    id: u64,
    buffer: Arc<Mutex<Vec<LogEvent>>>,
}

impl WindowHandle {
    /// Closes the window: unregisters it and returns the captured
    /// events in arrival order. Unregistering is idempotent with drop.
    pub fn close(self) -> Vec<LogEvent> {
        unregister(self.id);
        let mut buffer = lock(&self.buffer);
        std::mem::take(&mut *buffer)
    }
}

impl Drop for WindowHandle {
    fn drop(&mut self) {
        unregister(self.id);
    }
}

/// Opens a capture window and registers it in the process-global
/// registry; the window's `[opened_at, now)` interval starts at the
/// registration instant.
pub fn open_window() -> WindowHandle {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let mut windows = lock(&WINDOWS);
    windows.push(WindowEntry {
        id,
        opened_at: Instant::now(),
        buffer: Arc::clone(&buffer),
    });
    WindowHandle { id, buffer }
}

/// Removes a window from the registry (close or drop); a window no
/// longer in the registry captures nothing.
fn unregister(id: u64) {
    let mut windows = lock(&WINDOWS);
    windows.retain(|entry| entry.id != id);
}

/// Poison-tolerant lock: a panic inside one capture path must not take
/// down every later window.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether the harness's capture subscriber owns the process's tracing
/// seat. `false` means a foreign subscriber won the first-wins race
/// (or the harness never installed): events bypass the capture layer
/// and `logs:` assertions cannot run — the runner fails such
/// documents through `ScenarioFailure::LogCaptureUnavailable`.
pub fn capture_installed() -> bool {
    OWN_INSTALL.load(Ordering::Acquire)
}

/// Claims the process's tracing seat for capture, before any boot can
/// install its own subscriber. Idempotent once the harness owns the
/// seat; losing the first-wins `try_init` leaves the foreign
/// subscriber in place and [`capture_installed`] at `false`.
///
/// The subscriber composes the capture layer WITH an `fmt` layer, so
/// events keep reaching stdout after capture takes the seat. Honest
/// tradeoff: this passthrough is UNFILTERED — every level prints —
/// unlike the composition root's config-driven `EnvFilter`, because
/// the scenario tier must not re-read ambient config (ADR-0069
/// hermeticity); v1 accepts the verbosity delta.
pub fn ensure_capture_subscriber() {
    if OWN_INSTALL.load(Ordering::Acquire) {
        return;
    }
    let capture = tracing_subscriber::registry()
        .with(CaptureLayer)
        .with(tracing_subscriber::fmt::layer());
    if capture.try_init().is_ok() {
        OWN_INSTALL.store(true, Ordering::Release);
    }
}

/// A scoped dispatch carrying the capture layer: exercises `on_event`
/// and window attribution without touching the process-global
/// subscriber seat, so tests stay deterministic whatever the binary's
/// test ordering installed globally (the same escape hatch these unit
/// tests use).
#[cfg(test)]
pub(crate) fn scoped_capture_dispatch() -> tracing::Dispatch {
    tracing::Dispatch::new(tracing_subscriber::registry().with(CaptureLayer))
}

/// The capture layer: appends every event to every open window whose
/// `[opened_at, now)` interval contains the event timestamp.
struct CaptureLayer;

impl<S> Layer<S> for CaptureLayer
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let at = Instant::now();
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let log_event = LogEvent {
            at,
            level: *event.metadata().level(),
            target: event.metadata().target().to_string(),
            message: visitor.message.unwrap_or_default(),
        };
        let windows = lock(&WINDOWS);
        for window in windows.iter() {
            // Conservative attribution: only a window already open at
            // the event timestamp sees the event.
            if window.opened_at > at {
                continue;
            }
            let mut buffer = lock(&window.buffer);
            // Cap: drop-oldest until the marker and the event both
            // fit. The head drain removes any earlier marker, so one
            // marker stays at the head.
            if buffer.len() + 2 > LOG_WINDOW_CAP {
                let drop_count = buffer.len() + 2 - LOG_WINDOW_CAP;
                buffer.drain(..drop_count);
                buffer.insert(
                    0,
                    LogEvent {
                        at,
                        level: Level::TRACE,
                        target: MARKER_TARGET.to_string(),
                        message: format!(
                            "window cap {LOG_WINDOW_CAP} reached: earlier events dropped"
                        ),
                    },
                );
            }
            buffer.push(log_event.clone());
        }
    }
}

/// Extracts the rendered `message` field. The `info!("{msg}")`
/// convention records through `record_debug`, and `format_args!`'s
/// `Debug` renders the formatted text verbatim — no quoting.
#[derive(Default)]
struct MessageVisitor {
    message: Option<String>,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scoped dispatch carrying the capture layer: exercises
    /// `on_event` without touching the process-global subscriber, so
    /// these tests stay deterministic whatever the binary's test
    /// ordering installed globally.
    fn scoped_capture() -> tracing::Dispatch {
        tracing::Dispatch::new(tracing_subscriber::registry().with(CaptureLayer))
    }

    #[test]
    fn concurrent_windows_attribute_conservatively() {
        let dispatch = scoped_capture();
        let first = open_window();
        let second = open_window();
        // One event through the layer: both already-open windows
        // contain it.
        tracing::dispatcher::with_default(&dispatch, || tracing::info!("shared marker"));
        let late = open_window();
        tracing::dispatcher::with_default(&dispatch, || tracing::info!("later marker"));
        let first_events = first.close();
        let late_events = late.close();
        let second_events = second.close();
        assert!(
            first_events
                .iter()
                .any(|e| e.message.contains("shared marker")),
            "first window (open at event time) must capture: {first_events:?}"
        );
        assert!(
            second_events
                .iter()
                .any(|e| e.message.contains("shared marker")),
            "second window (open at event time) must capture: {second_events:?}"
        );
        assert!(
            first_events
                .iter()
                .any(|e| e.message.contains("later marker"))
        );
        assert!(
            second_events
                .iter()
                .any(|e| e.message.contains("later marker"))
        );
        assert!(
            !late_events
                .iter()
                .any(|e| e.message.contains("shared marker")),
            "conservative attribution: a window opened after the event never sees it: {late_events:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawned_task_events_counted() {
        let dispatch = scoped_capture();
        let window = open_window();
        // The spawned task carries the scoped dispatch onto its own
        // worker thread: capture follows the layer, not the spawning
        // thread.
        let task = tokio::spawn(async move {
            tracing::dispatcher::with_default(&dispatch, || tracing::warn!("spawned task marker"));
        });
        task.await.expect("spawned task completes");
        let events = window.close();
        assert!(
            events
                .iter()
                .any(|e| e.level == Level::WARN && e.message.contains("spawned task marker")),
            "the spawned task's warn must land in the open window: {events:?}"
        );
    }
}
