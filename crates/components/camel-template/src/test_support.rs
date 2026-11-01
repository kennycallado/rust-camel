//! Test-only helpers for capturing `tracing` warnings.

use std::sync::Arc;

/// Install a bare registry as the process-global tracing default, once per
/// test binary.
///
/// Guards capture tests against callsite-interest poisoning: `tracing`
/// caches each callsite's `Interest` process-wide from its FIRST macro
/// execution, evaluated against the executing thread's dispatcher. A
/// subscriber-less sibling test that hits a shared `warn!` callsite first
/// caches `Interest::never`, so a later thread-local `set_default` capture
/// silently drops events. The global registry heals prior poison and floors
/// future rebuilds at `sometimes` (convention:
/// `docs/testing/tracing-capture-guards.md`).
fn ensure_global_tracing_default() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INIT.set(()).is_ok() {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    }
}

/// Flattens an event's structured fields plus its message into a single
/// `key=value key=value ...` line for substring assertions.
///
/// `%value` (Display) and message fields arrive through `record_debug`;
/// string shorthand fields arrive through `record_str` and are kept
/// unquoted so assertions can match `class=undefined` directly.
struct FieldCollector {
    line: String,
}

impl FieldCollector {
    fn push(&mut self, field: &tracing::field::Field, rendered: &str) {
        if !self.line.is_empty() {
            self.line.push(' ');
        }
        self.line.push_str(field.name());
        self.line.push('=');
        self.line.push_str(rendered);
    }
}

impl tracing::field::Visit for FieldCollector {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.push(field, value);
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.push(field, &format!("{value:?}"));
    }
}

/// `tracing_subscriber` layer recording WARN events as flattened lines.
struct CaptureLayer {
    events: Arc<std::sync::Mutex<Vec<String>>>,
}

impl<C> tracing_subscriber::Layer<C> for CaptureLayer
where
    C: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, C>,
    ) {
        if *event.metadata().level() != tracing::Level::WARN {
            return;
        }
        let mut collector = FieldCollector {
            line: String::new(),
        };
        event.record(&mut collector);
        self.events
            .lock()
            .expect("capture buffer lock")
            .push(collector.line);
    }
}

/// Install a thread-local subscriber recording WARN lines; returns the
/// shared buffer and the default-subscriber guard.
///
/// `set_default` (guard-scoped, thread-local) is used instead of
/// `with_default` because the guard must span the `.await` points of an
/// async test body; `#[tokio::test]` polls the future on the test thread,
/// so the thread-local subscriber observes the producer's warn event.
pub(crate) fn capture_warns() -> (
    Arc<std::sync::Mutex<Vec<String>>>,
    tracing::subscriber::DefaultGuard,
) {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    ensure_global_tracing_default();
    let events: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let layer = CaptureLayer {
        events: Arc::clone(&events),
    };
    let guard = tracing_subscriber::registry().with(layer).set_default();
    (events, guard)
}
