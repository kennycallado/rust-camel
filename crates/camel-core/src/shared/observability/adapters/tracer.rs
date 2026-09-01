use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use opentelemetry::trace::{SpanKind, SpanRef, Status, TraceContextExt, Tracer};
use opentelemetry::{Context as OtelContext, InstrumentationScope, KeyValue, global};
use tower::Service;
use tracing::Instrument;

use crate::shared::observability::domain::{DetailLevel, MetricsLeversConfig};
use camel_api::metrics::MetricsCollector;
use camel_api::{BoxProcessor, CIRCUIT_OPEN, CamelError, Exchange, SpanKindHint, body_type_name};

/// RAII guard that ensures an OTel span is ended when dropped.
///
/// This prevents span leaks if the inner processor panics or returns early.
/// `pub(crate)` so the route compiler can reuse it for the route root span.
pub(crate) struct SpanEndGuard(pub(crate) OtelContext);

impl Drop for SpanEndGuard {
    fn drop(&mut self) {
        self.0.span().end();
    }
}

/// A processor wrapper that emits tracing spans for each step.
///
/// This processor wraps another processor and adds distributed tracing by:
/// 1. Starting a native OpenTelemetry span for each exchange
/// 2. Propagating the OTel context through `exchange.otel_context`
/// 3. Recording errors and status on the span
///
/// When no OTel provider is configured (noop provider), spans are no-ops with minimal overhead.
pub struct TracingProcessor {
    inner: BoxProcessor,
    route_id: String,
    step_id: String,
    span_name: String,
    step_index: usize,
    detail_level: DetailLevel,
    metrics: Option<Arc<dyn MetricsCollector>>,
    /// OTel span kind precomputed from `SpanKindHint` at construction.
    span_kind: SpanKind,
    /// Whether step spans are created. Gates SPANS only — metric families
    /// (errors unconditionally) keep flowing when false
    /// (metrics-configuration Req 1).
    spans_enabled: bool,
    /// Per-family metric levers; `increment_errors` is never gated.
    metric_levers: MetricsLeversConfig,
}

/// Positional fallback span-name fragment for unlabeled steps. Shared by
/// `TracingProcessor` (process steps) and `segment_span` (segment steps).
pub(crate) fn step_id_for(index: usize) -> String {
    format!("step-{index}")
}

impl TracingProcessor {
    /// Wrap a processor with tracing.
    ///
    /// `label` names the span after the DSL step it wraps (e.g. `log`,
    /// `to:direct`); when `None` the span falls back to the positional
    /// `step-{index}` id. `kind_hint` selects the OTel span kind for the
    /// step span and is converted once here.
    pub fn new(
        inner: BoxProcessor,
        route_id: String,
        step_index: usize,
        detail_level: DetailLevel,
        metrics: Option<Arc<dyn MetricsCollector>>,
        label: Option<Arc<str>>,
        kind_hint: SpanKindHint,
    ) -> Self {
        let step_id = step_id_for(step_index);
        let span_name = format!("{route_id}:{}", label.as_deref().unwrap_or(&step_id));
        let span_kind = match kind_hint {
            SpanKindHint::Internal => SpanKind::Internal,
            SpanKindHint::Producer => SpanKind::Producer,
            SpanKindHint::Consumer => SpanKind::Consumer,
            SpanKindHint::Client => SpanKind::Client,
            SpanKindHint::Server => SpanKind::Server,
            // `SpanKindHint` is `#[non_exhaustive]`: unknown future variants
            // degrade to `Internal` (the promised forward-compat behavior).
            _ => SpanKind::Internal,
        };
        Self {
            inner,
            route_id,
            step_id,
            span_name,
            step_index,
            detail_level,
            metrics,
            span_kind,
            // Defaults preserve the fully-traced behavior for direct
            // constructors; the pipeline composer overrides per the
            // effective tracer config.
            spans_enabled: true,
            metric_levers: MetricsLeversConfig::default(),
        }
    }

    /// Sets whether step spans are created (metrics still flow when off).
    pub fn with_spans_enabled(mut self, enabled: bool) -> Self {
        self.spans_enabled = enabled;
        self
    }

    /// Sets the per-family metric levers. The error family ignores them.
    pub fn with_metric_levers(mut self, levers: MetricsLeversConfig) -> Self {
        self.metric_levers = levers;
        self
    }

    /// Metrics-only fast path (`spans_enabled = false`): no OTel span, no
    /// local tracing span, context passes through untouched; metric
    /// families are recorded per the levers.
    fn call_metrics_only(
        &mut self,
        exchange: Exchange,
        start: Instant,
    ) -> Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>> {
        let fresh = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, fresh);
        let metrics = self.metrics.clone();
        let route_id = self.route_id.clone();
        let levers = self.metric_levers.clone();
        Box::pin(async move {
            let result = inner.call(exchange).await;
            record_step_metrics(
                metrics.as_ref(),
                &route_id,
                &levers,
                start.elapsed(),
                &result,
            );
            result
        })
    }
}

/// Emits the step metric families per the levers: `record_exchange_duration`
/// only when the duration family is enabled, `increment_exchanges` only when
/// the exchange family is enabled, and `increment_errors` NEVER gated
/// (metrics-configuration Req 2). Circuit-open rejections are excluded here
/// as well (dashboard-observability D2): the breaker counts them.
fn record_step_metrics(
    metrics: Option<&Arc<dyn MetricsCollector>>,
    route_id: &str,
    levers: &MetricsLeversConfig,
    duration: std::time::Duration,
    result: &Result<Exchange, CamelError>,
) {
    let Some(metrics) = metrics else { return };
    if levers.durations_enabled() {
        metrics.record_exchange_duration(route_id, duration);
    }
    if levers.exchanges_enabled() {
        metrics.increment_exchanges(route_id);
    }
    if let Err(e) = result {
        let error_class = e.classify();
        if error_class != CIRCUIT_OPEN {
            metrics.increment_errors(route_id, error_class);
        }
    }
}

impl Service<Exchange> for TracingProcessor {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let start = Instant::now();

        // Metrics-only mode (dashboard-observability D3): spans are gated by
        // `tracer.enabled`, but the pipeline adapter still runs so metric
        // families — errors unconditionally — keep flowing.
        if !self.spans_enabled {
            return self.call_metrics_only(exchange, start);
        }

        let span_name = self.span_name.clone();
        let span_kind = self.span_kind.clone();

        // Get the global tracer (noop if no provider is configured)
        let tracer = global::tracer_with_scope(
            InstrumentationScope::builder("camel-core")
                .with_version(env!("CARGO_PKG_VERSION"))
                .build(),
        );

        // Extract parent context from exchange.otel_context
        let parent_cx = exchange.otel_context.clone();

        // Build span attributes (Minimal set; Medium/Full extras appended below)
        let mut attributes =
            step_span_attributes(&self.route_id, self.step_index, exchange.correlation_id());

        if self.detail_level >= DetailLevel::Medium {
            attributes.push(KeyValue::new(
                "headers_count",
                exchange.input.headers.len() as i64,
            ));
            attributes.push(KeyValue::new(
                "body_type",
                body_type_name(&exchange.input.body),
            ));
            attributes.push(KeyValue::new("has_error", exchange.has_error()));
        }

        // Start a new span as a child of the parent context
        let span = tracer
            .span_builder(span_name)
            .with_kind(span_kind)
            .with_attributes(attributes.iter().cloned())
            .start_with_context(&tracer, &parent_cx);

        // Derive the step context from the parent (not from the ambient
        // current context): parent entries such as baggage stay attached, and
        // the parent context is restored on the result exchange after the step.
        let cx = parent_cx.with_span(span);

        // Store back into exchange so downstream processors inherit this context
        exchange.otel_context = cx.clone();

        // Also create a tracing span for local dev logging
        let tracing_span = tracing::info_span!(
            target: "camel_tracer",
            "step",
            correlation_id = %exchange.correlation_id(),
            route_id = %self.route_id,
            step_id = %self.step_id,
            step_index = self.step_index,
            duration_ms = tracing::field::Empty,
            status = tracing::field::Empty,
            headers_count = tracing::field::Empty,
            body_type = tracing::field::Empty,
            has_error = tracing::field::Empty,
            output_body_type = tracing::field::Empty,
            header_0 = tracing::field::Empty,
            header_1 = tracing::field::Empty,
            header_2 = tracing::field::Empty,
            error = tracing::field::Empty,
            error_type = tracing::field::Empty,
        );

        if self.detail_level >= DetailLevel::Medium {
            tracing_span.record("headers_count", exchange.input.headers.len() as u64);
            tracing_span.record("body_type", body_type_name(&exchange.input.body));
            tracing_span.record("has_error", exchange.has_error());
        }

        if self.detail_level >= DetailLevel::Full {
            let headers: Vec<_> = exchange.input.headers.iter().take(3).collect();
            if let Some((k, v)) = headers.first() {
                tracing_span.record("header_0", format!("{k}={v:?}"));
            }
            if let Some((k, v)) = headers.get(1) {
                tracing_span.record("header_1", format!("{k}={v:?}"));
            }
            if let Some((k, v)) = headers.get(2) {
                tracing_span.record("header_2", format!("{k}={v:?}"));
            }
        }

        // Consume the ORIGINAL inner that `poll_ready` readied: its
        // reservations (e.g. DirectProducer's pending semaphore permit)
        // belong to that instance, so re-readying a clone would drop them.
        // The fresh clone stays in `self.inner` as the unreadied placeholder
        // for the next ready/call cycle.
        let fresh = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, fresh);
        let detail_level = self.detail_level.clone();
        let metrics = self.metrics.clone();
        let route_id = self.route_id.clone();
        let levers = self.metric_levers.clone();

        Box::pin(
            async move {
                // Note: ContextGuard is not Send (it uses thread-local storage), so we cannot
                // hold it across await points in an async fn. Instead, we propagate the OTel
                // context through exchange.otel_context, which is Send + Sync.

                // Create guard to ensure span is ended even on panic
                let _guard = SpanEndGuard(cx.clone());

                let result = inner.call(exchange).await;

                let duration = start.elapsed();
                let duration_ms = duration.as_millis() as u64;
                tracing::Span::current().record("duration_ms", duration_ms);

                // Record metric families per the levers (errors never gated).
                record_step_metrics(metrics.as_ref(), &route_id, &levers, duration, &result);

                match result {
                    Ok(mut ex) => {
                        tracing::Span::current().record("status", "success");
                        cx.span().set_status(Status::Ok);

                        if detail_level >= DetailLevel::Medium {
                            tracing::Span::current()
                                .record("output_body_type", body_type_name(&ex.input.body));
                            cx.span().set_attribute(KeyValue::new(
                                "output_body_type",
                                body_type_name(&ex.input.body),
                            ));
                        }

                        // Restore the caller's context: the step span ends with
                        // this future, so downstream steps must not chain onto it.
                        ex.otel_context = parent_cx.clone();
                        Ok(ex)
                    }
                    Err(e) => {
                        record_exception(&cx.span(), &e);
                        let error_class = e.classify();
                        tracing::Span::current().record("status", "error");
                        tracing::Span::current().record("error", e.to_string());
                        tracing::Span::current().record("error_type", error_class);
                        Err(e)
                    }
                }
            }
            .instrument(tracing_span),
        )
    }
}

impl Clone for TracingProcessor {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            route_id: self.route_id.clone(),
            step_id: self.step_id.clone(),
            span_name: self.span_name.clone(),
            step_index: self.step_index,
            detail_level: self.detail_level.clone(),
            metrics: self.metrics.clone(),
            span_kind: self.span_kind.clone(),
            spans_enabled: self.spans_enabled,
            metric_levers: self.metric_levers.clone(),
        }
    }
}

/// R4-L8: cap only the span-attr representation. Exchange.correlation_id is untouched.
pub(crate) fn capped_correlation_id(id: &str) -> &str {
    const CAP: usize = 128;
    if id.len() > CAP {
        "<oversized:correlation_id>"
    } else {
        id
    }
}

/// Minimal-level span attributes for a pipeline step span.
///
/// `correlation_id` is the raw exchange correlation id; this is the single
/// capping site for its span-attribute representation (R4-L8). Medium/Full
/// extras (`headers_count`, `body_type`, `has_error`) are appended by the
/// caller, not here.
pub(crate) fn step_span_attributes(
    route_id: &str,
    step_index: usize,
    correlation_id: &str,
) -> Vec<KeyValue> {
    vec![
        KeyValue::new("messaging.system", "camel"),
        KeyValue::new(
            "correlation_id",
            capped_correlation_id(correlation_id).to_string(),
        ),
        KeyValue::new("route_id", route_id.to_string()),
        KeyValue::new("step_index", step_index as i64),
    ]
}

pub(crate) fn record_exception(span: &SpanRef<'_>, e: &CamelError) {
    let error_class = e.classify();
    span.set_status(Status::error(e.to_string()));
    span.add_event(
        "exception",
        vec![
            KeyValue::new("exception.type", error_class.to_string()),
            KeyValue::new("exception.message", e.to_string()),
        ],
    );
}

#[cfg(test)]
#[path = "tracer_tests.rs"]
mod tests;
