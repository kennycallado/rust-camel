use camel_api::{CamelError, Exchange, IdentityProcessor, Value, ValueSource};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::Service;
use tracing::{debug, error, info, trace, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone)]
pub struct LogProcessor {
    inner: IdentityProcessor,
    level: LogLevel,
    message: String,
}

// TODO(PROC-004): Add metrics instrumentation — processed count, error count, latency histograms
// are not yet instrumented on processors. Consider wiring MetricsCollector into LogProcessor and
// incrementing a counter on each call, recording elapsed time, and tracking errors.

impl LogProcessor {
    pub fn new(level: LogLevel, message: String) -> Self {
        Self {
            inner: IdentityProcessor,
            level,
            message,
        }
    }
}

impl Service<Exchange> for LogProcessor {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let msg = self.message.clone();
        let exchange_id = exchange.correlation_id.clone();
        let body_preview = sanitize_preview(
            &exchange
                .input
                .body
                .as_text()
                .unwrap_or("")
                .chars()
                .take(64)
                .collect::<String>(),
        );
        debug!(exchange_id = %exchange_id, body_preview = %body_preview, "LogProcessor processing exchange");
        match self.level {
            LogLevel::Trace => trace!(exchange_id = %exchange_id, "{}", msg),
            LogLevel::Debug => debug!(exchange_id = %exchange_id, "{}", msg),
            LogLevel::Info => info!(exchange_id = %exchange_id, "{}", msg),
            LogLevel::Warn => warn!(exchange_id = %exchange_id, "{}", msg),
            // Deliberately unannotated: the `log` step is a user-output
            // mechanism — the route selects this level — so the call sits
            // outside ADR-0012's operational convention. The lint-log-levels
            // exclusion is symbol-bound (structural, in scripts/xtask).
            LogLevel::Error => error!(exchange_id = %exchange_id, "{}", msg),
        }
        self.inner.call(exchange)
    }
}

/// A log processor that evaluates a message expression against the Exchange at call-time.
/// Analogous to [`DynamicSetHeader`](crate::dynamic_set_header::DynamicSetHeader).
///
/// A failed evaluation fails the step BEFORE logging — no log record is
/// emitted with a null message.
#[derive(Clone)]
pub struct DynamicLog {
    inner: IdentityProcessor,
    level: LogLevel,
    source: ValueSource,
}

impl DynamicLog {
    pub fn new(level: LogLevel, source: impl Into<ValueSource>) -> Self {
        Self {
            inner: IdentityProcessor,
            level,
            source: source.into(),
        }
    }
}

/// Render an evaluated log-message value: strings pass through unquoted,
/// everything else uses its JSON representation.
fn log_value_to_string(value: Value) -> String {
    match value {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

impl Service<Exchange> for DynamicLog {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let source = self.source.clone();
        let level = self.level;
        Box::pin(async move {
            let value = source.evaluate(&exchange).await?;
            let exchange_id = exchange.correlation_id.clone();
            let msg = sanitize_preview(&log_value_to_string(value));
            match level {
                LogLevel::Trace => trace!(exchange_id = %exchange_id, "{}", msg),
                LogLevel::Debug => debug!(exchange_id = %exchange_id, "{}", msg),
                LogLevel::Info => info!(exchange_id = %exchange_id, "{}", msg),
                LogLevel::Warn => warn!(exchange_id = %exchange_id, "{}", msg),
                // Deliberately unannotated: user-output mechanism, see
                // `LogProcessor::call` (symbol-bound lint exclusion).
                LogLevel::Error => error!(exchange_id = %exchange_id, "{}", msg),
            }
            Ok(exchange)
        })
    }
}

/// Strip control characters that could enable log injection.
/// Replaces control chars with U+FFFD (replacement char) to preserve
/// alignment while making injection visible.
fn sanitize_preview(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::body::Body;
    use camel_api::{Message, Value};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_log_processor_passes_exchange_through() {
        let mut processor = LogProcessor::new(LogLevel::Info, "test message".into());
        let exchange = Exchange::default();
        let result = processor.call(exchange).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_log_processor_preserves_exchange_body() {
        let mut processor = LogProcessor::new(LogLevel::Debug, "debug message".into());
        let mut exchange = Exchange::default();
        exchange.input.body = Body::Text("test body".into());
        let result = processor.call(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("test body"));
    }

    fn sync_source<F>(f: F) -> camel_api::ValueSource
    where
        F: Fn(&Exchange) -> camel_api::Value + Send + Sync + 'static,
    {
        camel_api::ValueSource::Sync(std::sync::Arc::new(f))
    }

    #[tokio::test]
    async fn test_dynamic_log_evaluates_body() {
        let svc = DynamicLog::new(
            LogLevel::Info,
            sync_source(|ex: &Exchange| {
                camel_api::Value::String(format!("body={}", ex.input.body.as_text().unwrap_or("")))
            }),
        );
        let exchange = Exchange::new(Message::new("hello"));
        let result = svc.oneshot(exchange).await.unwrap();
        // Exchange passes through unchanged
        assert_eq!(result.input.body.as_text(), Some("hello"));
    }

    #[tokio::test]
    async fn test_dynamic_log_evaluates_header() {
        let svc = DynamicLog::new(
            LogLevel::Info,
            sync_source(|ex: &Exchange| {
                let counter = ex
                    .input
                    .header("CamelTimerCounter")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                camel_api::Value::String(format!("{} World", counter))
            }),
        );
        let mut msg = Message::new("");
        msg.set_header("CamelTimerCounter", Value::Number(42.into()));
        let exchange = Exchange::new(msg);
        let result = svc.oneshot(exchange).await.unwrap();
        // Exchange passes through unchanged
        assert_eq!(
            result.input.header("CamelTimerCounter"),
            Some(&Value::Number(42.into()))
        );
    }

    #[tokio::test]
    async fn dynamic_log_error_fails_step() {
        use std::sync::Arc;

        use camel_api::{BoxValueFuture, ValueSource};

        let source = ValueSource::Async(Arc::new(|_: &Exchange| {
            Box::pin(async { Err(CamelError::ProcessorError("log boom".into())) }) as BoxValueFuture
        }));
        let svc = DynamicLog::new(LogLevel::Info, source);

        let result = svc.oneshot(Exchange::new(Message::new("x"))).await;
        assert!(
            result.is_err(),
            "a failed log-message expression must fail the step, not log a null message"
        );
    }

    #[test]
    fn body_preview_strips_control_chars() {
        let body_with_injection = "no\ttab\nnl\rcr\0null\u{1B}esc";
        let preview: String = body_with_injection.chars().take(64).collect();
        let sanitized = sanitize_preview(&preview);
        assert!(!sanitized.contains('\n'), "newlines must be replaced");
        assert!(
            !sanitized.contains('\r'),
            "carriage returns must be replaced"
        );
        assert!(!sanitized.contains('\t'), "tabs must be replaced");
        assert!(!sanitized.contains('\0'), "null must be replaced");
        assert!(
            !sanitized.contains('\u{1B}'),
            "ESC (ANSI injection) must be replaced"
        );
        // U+FFFD replacement char must appear (replacement, not deletion)
        assert!(
            sanitized.contains('\u{FFFD}'),
            "control chars must be replaced with U+FFFD, not deleted"
        );
    }

    #[tokio::test]
    async fn dynamic_log_sanitizes_control_chars() {
        let svc = DynamicLog::new(
            LogLevel::Info,
            sync_source(|_ex: &Exchange| {
                camel_api::Value::String("fake\nline\rinjection".to_string())
            }),
        );
        // The exchange passes through; sanitization happens inside call().
        // We verify the exchange is unchanged and the call succeeds — the
        // sanitization itself is validated by body_preview_strips_control_chars.
        let exchange = Exchange::default();
        let result = svc.oneshot(exchange).await;
        assert!(result.is_ok());
    }

    /// Run `f` with a thread-local subscriber and return the captured output.
    fn capture_logs<F: FnOnce()>(f: F) -> String {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b); // allow-unwrap
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buf {
            type Writer = Buf;
            fn make_writer(&'a self) -> Buf {
                self.clone()
            }
        }

        let buf = Buf(Arc::new(Mutex::new(Vec::new())));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = buf.0.lock().unwrap().clone(); // allow-unwrap
        String::from_utf8(bytes).unwrap() // allow-unwrap
    }

    #[test]
    fn log_processor_error_level_emits_error_not_warn() {
        let out = capture_logs(|| {
            let mut p = LogProcessor::new(LogLevel::Error, "boom-static".into());
            drop(p.call(Exchange::default()));
        });
        assert!(
            out.contains("ERROR") && out.contains("boom-static"),
            "level error must emit ERROR, got: {out}"
        );
        assert!(!out.contains("WARN"), "must not downgrade to WARN: {out}");
    }

    #[test]
    fn dynamic_log_error_level_emits_error_not_warn() {
        let out = capture_logs(|| {
            let mut svc = DynamicLog::new(
                LogLevel::Error,
                sync_source(|_ex: &Exchange| camel_api::Value::String("boom-dynamic".into())),
            );
            let fut = svc.call(Exchange::default());
            futures::executor::block_on(fut).unwrap(); // allow-unwrap
        });
        assert!(
            out.contains("ERROR") && out.contains("boom-dynamic"),
            "level error must emit ERROR, got: {out}"
        );
        assert!(!out.contains("WARN"), "must not downgrade to WARN: {out}");
    }

    #[test]
    fn log_processor_warn_level_still_emits_warn() {
        let out = capture_logs(|| {
            let mut p = LogProcessor::new(LogLevel::Warn, "w-static".into());
            drop(p.call(Exchange::default()));
        });
        assert!(out.contains("WARN") && !out.contains("ERROR"), "got: {out}");
    }
}
