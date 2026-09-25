//! Test-only log capture shared across camel-processor test modules.
//!
//! `#[cfg(test)]`-gated and `pub(crate)`: installs capturing layers over
//! `tracing_subscriber::registry()` via `tracing::subscriber::with_default`
//! for the duration of one closure, recording DEBUG-or-more-severe event
//! fields AND per-span `record` calls. No global state — safe under
//! parallel test threads. (Mirror of camel-config's `log_capture`.)
//!
//! The registry base is required: a minimal hand-rolled `Subscriber`
//! cannot implement `current_span`, so `Span::current().record(..)`
//! (used by `send_to_handler` and the do_try arms) would silently no-op.

use std::fmt;
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing::span::Record;
use tracing::{Event, Id, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

type Sink = Arc<Mutex<Vec<String>>>;

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

/// OnceLock-gated global registry install: heals/prevents callsite-
/// interest poisoning of the shared error-handler `debug!`/`error!`
/// callsites (`error_handler.rs:280/370` and the system-broken
/// `error!` sites in `send_to_handler`), which subscriber-less
/// sibling error-handler tests in this binary can hit first (fix
/// pattern: c3853198; bd rc-img5). Every test that triggers those
/// callsites must call this BEFORE the first evaluation, otherwise
/// the callsite interest is cached as `never` process-wide.
pub(crate) fn ensure_global_registry() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INIT.set(()).is_ok() {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    }
}

/// Runs `f` with capturing layers installed and returns `(f's result,
/// captured event field strings, captured span record strings)`.
/// Events and span records are kept in separate vectors, each in
/// emission order. Rendered as `field="value"` pairs joined by
/// spaces, with the human-readable text under the standard
/// `message` field.
pub(crate) fn capture_debugs_with_span_records<T>(
    f: impl FnOnce() -> T,
) -> (T, Vec<String>, Vec<String>) {
    ensure_global_registry();
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
/// this module as `field=value field=value ... message=...`): returns
/// the value of `name`, ending at the next ` <ident>=` field boundary
/// or end of line.
pub(crate) fn captured_field<'a>(record: &'a str, name: &str) -> Option<&'a str> {
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
pub(crate) fn record_field<'a>(
    captured: &'a [String],
    message_part: &str,
    field: &str,
) -> Option<&'a str> {
    let record = captured.iter().find(|line| line.contains(message_part))?;
    captured_field(record, field)
}
