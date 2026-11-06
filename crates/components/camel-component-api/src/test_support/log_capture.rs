//! Shared tracing log-capture harness for component and integration tests.
//!
//! Promoted from five inline copies (bd rc-421es): the `fmt`-writer pairs in
//! camel-core `lifecycle/adapters/route_controller_trait_tests.rs` and
//! camel-component-wasm `tests/source_bind_gate.rs`, plus the `capture_warns`,
//! `capture_debugs_with_span_records`, and `#[traced_test]` copies.
//!
//! # Families
//!
//! - [`capture_logs`] / [`capture_logs_async`] — a `tracing_subscriber::fmt`
//!   `Subscriber` writing through [`CaptureWriter`] with ANSI off. The sync
//!   form runs the closure on the caller's thread; the async form captures
//!   only what the polling thread emits.
//! - [`capture_warns`] — a hand-rolled `Subscriber` that records WARN and
//!   ERROR events (warn-or-more-severe; the historical camel-config
//!   `>= WARN` predicate with the reversed-`Level`-ordering comparison
//!   corrected — see the fn doc for the `Level`-ordering truth).
//! - [`capture_debugs_with_span_records`] + [`captured_field`] /
//!   [`record_field`] — a `tracing_subscriber` registry with `Layer`s,
//!   capturing DEBUG-and-more-severe events plus span records.
//! - [`traced`] — the `tracing-test` global buffer, behind the
//!   `traced-test-capture` feature.
//!
//! # Guard contract
//!
//! Every function in this module installs a THREAD-LOCAL subscriber only. The
//! module never calls `set_global_default` and contains no process-global
//! tracing guard: guards stay per consumer test binary.
//!
//! `tracing` caches each callsite's `Interest` process-wide from its first
//! macro execution, evaluated against the executing thread's dispatcher. A
//! subscriber-less sibling test that reaches the same callsite first caches
//! `Interest::never`, after which a later thread-local capture silently drops
//! the event. The cure is a process-global registry default installed once
//! per test binary BEFORE these functions run. Every consumer test binary
//! therefore keeps its own colocated guard (a process-global registry default
//! installer) and calls it first. See rc-img5 / rc-6jarb
//! and `docs/testing/tracing-capture-guards.md`; promotion tracked by
//! bd rc-421es.
//!
//! The `traced` family is a DIFFERENT mechanism: `#[traced_test]` installs a
//! global recording subscriber for the test and reads the events back from a
//! process-global buffer. It is required when events originate in spawned
//! tasks, which a thread-local subscriber cannot observe. Its global
//! subscriber is owned by the `tracing-test` macro, not by this module.

#[cfg(test)]
#[path = "log_capture_tests.rs"]
mod log_capture_tests;

use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

use tracing::Event;
use tracing::Id;
use tracing::Level;
use tracing::Metadata;
use tracing::Subscriber;
use tracing::field::Field;
use tracing::field::Visit;
use tracing::span::Attributes;
use tracing::span::Record;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;

/// In-memory writer shared by the `fmt`-based capture families.
///
/// Promoted from camel-core `route_controller_trait_tests.rs` and
/// camel-component-wasm `tests/source_bind_gate.rs`. The sources unwrapped
/// the buffer lock; this copy panics with
/// `"capture buffer lock poisoned"` instead. That expectation is deliberate:
/// it replaces the sources' lock unwrap with identical semantics — a poisoned
/// capture buffer is a test-infrastructure bug — while staying lint-clean.
pub struct CaptureWriter {
    buf: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl std::io::Write for CaptureWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf
            .lock()
            .expect("capture buffer lock poisoned") // allow-unwrap
            .extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriter {
            buf: Arc::clone(&self.buf),
        }
    }
}

/// Runs `f` synchronously under a thread-local `fmt` subscriber and returns
/// the captured text.
///
/// Rendering is `tracing_subscriber`'s default fmt format with ANSI disabled,
/// so each line carries the level, the message, and `key=value` fields.
/// Nothing global is installed; the consumer test binary's own guard must
/// already have run (see the module-level guard contract).
pub fn capture_logs(f: impl FnOnce()) -> String {
    let buf = Arc::new(std::sync::Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_writer(CaptureWriter {
            buf: Arc::clone(&buf),
        })
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let captured = buf.lock().expect("capture buffer lock poisoned").clone(); // allow-unwrap
    String::from_utf8(captured).expect("captured output must be UTF-8") // allow-unwrap
}

/// Runs `fut` to completion under a thread-local `fmt` subscriber and returns
/// its output plus the captured text.
///
/// CURRENT-THREAD EXECUTION ONLY: the thread-local default lives on the
/// thread that polls the future. Events emitted by spawned tasks are not
/// captured, and there is no `Send` or task-migration safety guarantee — on a
/// multi-threaded runtime the future may move to a thread without the
/// subscriber. Use the `traced` family when events come from spawned tasks.
/// Nothing global is installed; the consumer test binary's own guard must
/// already have run (see the module-level guard contract).
pub async fn capture_logs_async<F: std::future::Future>(fut: F) -> (F::Output, String) {
    let buf = Arc::new(std::sync::Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_writer(CaptureWriter {
            buf: Arc::clone(&buf),
        })
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let output = fut.await;
    drop(_guard);
    let captured = buf.lock().expect("capture buffer lock poisoned").clone(); // allow-unwrap
    let captured = String::from_utf8(captured).expect("captured output must be UTF-8"); // allow-unwrap
    (output, captured)
}

/// In-memory event sink shared by the `capture_warns` recorder and its caller.
///
/// Module-level because the `capture_debugs_with_span_records` family reuses
/// it; define it exactly once.
type Sink = Arc<Mutex<Vec<String>>>;

/// Minimal `Subscriber` recording events selected by the
/// warn-or-more-severe predicate.
struct Recorder {
    warnings: Sink,
    next_span_id: std::sync::atomic::AtomicU64,
}

struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        let _ = fmt::write(&mut self.0, format_args!("{}={:?}", field.name(), value));
    }
}

impl Subscriber for Recorder {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _attrs: &Attributes<'_>) -> Id {
        let id = self
            .next_span_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        Id::from_u64(id)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}
    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        // tracing-core orders Levels so that more-severe levels compare
        // SMALLER (ERROR is the minimum, TRACE the maximum): "WARN and more
        // severe" is `<= Level::WARN`. A `>=` comparison here would capture
        // everything EXCEPT `error!` — the opposite of this function's
        // contract (bd rc-icpbt; the historical camel-config predicate).
        if *event.metadata().level() <= Level::WARN {
            let mut visitor = MessageVisitor(String::new());
            event.record(&mut visitor);
            if let Ok(mut slot) = self.warnings.lock() {
                slot.push(visitor.0);
            }
        }
    }

    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}
}

/// Runs `f` with a capturing subscriber installed and returns
/// `(f's result, captured messages)` in emission order.
///
/// Rendering: the unquoted `message` field comes first (`message=<text>`),
/// followed by `field="value"` pairs (str fields quoted via Debug) — the
/// `event!` macro prepends the message.
///
/// Captured set: WARN and ERROR — warn-or-more-severe, as the name says.
/// `tracing-core`'s `Level` `Ord` is reversed (ERROR compares smallest;
/// tracing-core 0.1.36 `src/metadata.rs:945-950`), so the correct
/// comparison for "WARN or more severe" is
/// `*event.metadata().level() <= Level::WARN`. The historical camel-config
/// predicate used `>=`, which selects every level EXCEPT ERROR; the
/// promotion (bd rc-421es) kept it verbatim and deferred the fix. Fixed in
/// bd rc-icpbt together with an audit of every consumer test: no consumer
/// relied on the over-capture (all assertions are needle-filtered on
/// `warn!` messages), so the fix is assertion-neutral for them.
///
/// Unlike the camel-config original this function does NOT install a
/// process-global registry default: that belongs to each consumer test
/// binary's own guard, which must already have run (see the module-level
/// guard contract).
pub fn capture_warns<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    let sink: Sink = Default::default();
    let recorder = Recorder {
        warnings: Arc::clone(&sink),
        next_span_id: Default::default(),
    };
    let out = tracing::subscriber::with_default(recorder, f);
    let collected = sink
        .lock()
        .ok()
        .map(|slot| slot.clone())
        .unwrap_or_default();
    (out, collected)
}

/// Renders `field="value"` pairs joined by spaces.
struct FieldVisitor(String);

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        let _ = fmt::write(&mut self.0, format_args!("{}={:?}", field.name(), value));
    }
}

/// Layer capturing events (`error!`/`warn!`/`debug!`/...) into a sink.
struct EventCaptureLayer {
    events: Sink,
}

impl<S> Layer<S> for EventCaptureLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // tracing-core orders Levels so that more-severe levels
        // compare SMALLER (Error=4 .. Trace=0 with a reversed Ord):
        // "DEBUG and more severe" is `<= Level::DEBUG`. A `>=`
        // comparison here silently drops ERROR/WARN/INFO events.
        if *event.metadata().level() <= Level::DEBUG {
            let mut visitor = FieldVisitor(String::new());
            event.record(&mut visitor);
            if let Ok(mut slot) = self.events.lock() {
                slot.push(visitor.0);
            }
        }
    }
}

/// Layer capturing per-span `record` calls (e.g. a field recorded on
/// an entered span via `Span::current().record(..)`).
struct SpanRecordLayer {
    records: Sink,
}

impl<S> Layer<S> for SpanRecordLayer
where
    S: Subscriber,
{
    fn on_record(&self, _id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor(String::new());
        values.record(&mut visitor);
        if let Ok(mut slot) = self.records.lock() {
            slot.push(visitor.0);
        }
    }
}

/// Runs `f` with capturing layers installed over a `registry` base and returns
/// `(f's result, captured event field strings, captured span record strings)`.
///
/// The registry base is required: a minimal hand-rolled `Subscriber` cannot
/// implement `current_span`, so `Span::current().record(..)` would silently
/// no-op.
///
/// Captured set: DEBUG and more severe (DEBUG, INFO, WARN, ERROR). Events and
/// span records are kept in separate vectors, each in emission order.
/// Rendering: the unquoted `message` field comes first (`message=<text>`),
/// followed by `field="value"` pairs (str fields quoted via Debug) — the
/// `event!` macro prepends the message.
///
/// Unlike the camel-processor original this function does NOT install a
/// process-global registry default (`ensure_global_registry`): that belongs to
/// each consumer test binary's own guard, which must already have run (see the
/// module-level guard contract).
pub fn capture_debugs_with_span_records<T>(f: impl FnOnce() -> T) -> (T, Vec<String>, Vec<String>) {
    let events: Sink = Default::default();
    let span_records: Sink = Default::default();
    let subscriber = tracing_subscriber::registry()
        .with(EventCaptureLayer {
            events: Arc::clone(&events),
        })
        .with(SpanRecordLayer {
            records: Arc::clone(&span_records),
        });
    let out = tracing::subscriber::with_default(subscriber, f);
    let collected = events
        .lock()
        .ok()
        .map(|slot| slot.clone())
        .unwrap_or_default();
    let spans = span_records
        .lock()
        .ok()
        .map(|slot| slot.clone())
        .unwrap_or_default();
    (out, collected, spans)
}

/// Structured field lookup over one captured record line (rendered by
/// this module as `message=... field="value" field="value" ...`): returns
/// the value of `name`, ending at the next ` <ident>=` field boundary
/// or end of line.
pub fn captured_field<'a>(record: &'a str, name: &str) -> Option<&'a str> {
    let start = record
        .match_indices(&format!("{name}="))
        .find(|(i, _)| *i == 0 || record[..*i].ends_with(' '))?
        .0
        + name.len()
        + 1;
    let rest = &record[start..];
    let end = rest
        .match_indices(' ')
        .find_map(|(i, _)| {
            let after = &rest[i + 1..];
            let eq = after.find('=')?;
            let key = &after[..eq];
            (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .then_some(i)
        })
        .unwrap_or(rest.len());
    Some(rest[..end].trim_end())
}

/// Find the captured record carrying `message_part` and look up a
/// structured field on it.
pub fn record_field<'a>(
    captured: &'a [String],
    message_part: &str,
    field: &str,
) -> Option<&'a str> {
    let record = captured.iter().find(|line| line.contains(message_part))?;
    captured_field(record, field)
}

/// #[traced_test] companion family over the process-global `tracing-test` buffer.
#[cfg(feature = "traced-test-capture")]
pub mod traced {
    use std::sync::Mutex;

    use tracing_test::internal::global_buf;

    /// Guards the process-wide `tracing-test` buffer; see [`lock_log_capture`].
    static LOG_CAPTURE_LOCK: Mutex<()> = Mutex::new(());

    /// Serialize access to the shared `#[traced_test]` global buffer: the
    /// buffer is process-wide and not cleared between tests, so concurrent
    /// emission would race capture assertions.
    pub fn lock_log_capture() -> std::sync::MutexGuard<'static, ()> {
        // Poison-resilient: a failed assertion in one log test must not
        // cascade a PoisonError into its sibling.
        LOG_CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Clear the `#[traced_test]` global buffer so a test's assertions start
    /// from a known-empty state. Call under [`lock_log_capture`].
    pub fn clear_captured_logs() {
        let mut buf = global_buf()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        buf.clear();
    }

    /// Return the captured log record lines whose message contains every
    /// needle. `#[traced_test]` writes each record as one line into the
    /// shared global buffer, so line matching approximates per-record
    /// message matching. Each returned line carries the level (e.g. ` WARN `)
    /// and the structured `key=value` fields, so callers can pin both.
    pub fn captured_records_containing(needles: &[&str]) -> Vec<String> {
        let buf = global_buf()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        String::from_utf8_lossy(&buf)
            .lines()
            .filter(|line| needles.iter().all(|needle| line.contains(needle)))
            .map(str::to_owned)
            .collect()
    }
}
