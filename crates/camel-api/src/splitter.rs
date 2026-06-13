use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;

use crate::body::Body;
use crate::error::CamelError;
use crate::exchange::Exchange;
use crate::message::Message;

/// A function that splits a single exchange into multiple fragment exchanges.
pub type SplitExpression = Arc<dyn Fn(&Exchange) -> Vec<Exchange> + Send + Sync>;

/// A function that lazily produces a stream of exchange fragments.
///
/// Used by [`StreamingSplitterService`] for v1 sequential streaming split
/// (e.g., ZIP entry extraction, CSV/JSON streaming in future work).
///
/// Each call returns a `Stream` that yields fragments one at a time.
pub type StreamingSplitExpression = Arc<
    dyn Fn(Exchange) -> Pin<Box<dyn Stream<Item = Result<Exchange, CamelError>> + Send>>
        + Send
        + Sync,
>;

/// Strategy for aggregating fragment results back into a single exchange.
#[derive(Clone, Default)]
pub enum AggregationStrategy {
    /// Result is the last fragment's exchange (default).
    #[default]
    LastWins,
    /// Collects all fragment bodies into a JSON array.
    CollectAll,
    /// Returns the original exchange unchanged.
    Original,
    /// Custom aggregation function: `(accumulated, next) -> merged`.
    Custom(Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync>),
}

/// The streaming format to use when splitting a stream body.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum StreamSplitFormat {
    /// Auto-detect the format from the body content.
    #[default]
    Auto,
    /// Newline-delimited JSON — each line is a complete JSON value.
    Ndjson,
    /// Split by newlines, each line becomes a text fragment.
    Lines,
    /// Split into fixed-size byte chunks.
    Chunks,
    /// ZIP archive — materialized format, each entry becomes a fragment exchange.
    Zip,
}

/// Configuration for splitting a streaming body into fragments.
///
/// Controls how the stream splitter processes the body, including format
/// detection, sizing limits, and metadata propagation.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct StreamSplitConfig {
    /// The streaming format to use.
    pub format: StreamSplitFormat,
    /// Maximum size (in bytes) of a single record or chunk.
    pub max_record_bytes: usize,
    /// Number of records/chunks to collect into a single exchange batch.
    pub batch_size: usize,
    /// Explicit chunk size in bytes (required when format is [`Chunks`](StreamSplitFormat::Chunks)).
    pub chunk_size: Option<usize>,
    /// Whether to include origin metadata in each fragment.
    pub include_origin: bool,
}

impl Default for StreamSplitConfig {
    fn default() -> Self {
        Self {
            format: StreamSplitFormat::Auto,
            max_record_bytes: 1024 * 1024,
            batch_size: 1,
            chunk_size: None,
            include_origin: true,
        }
    }
}

impl StreamSplitConfig {
    /// Validates the configuration.
    ///
    /// # Errors
    ///
    /// Returns [`CamelError::Config`] if:
    /// - `batch_size` is `0`
    /// - `max_record_bytes` is `0`
    /// - `format` is [`Chunks`](StreamSplitFormat::Chunks) but `chunk_size` is `None`
    /// - `format` is [`Zip`](StreamSplitFormat::Zip) but `chunk_size` is `Some(...)`
    /// - `chunk_size` is `Some(0)`
    pub fn validate(&self) -> Result<(), CamelError> {
        if self.batch_size == 0 {
            return Err(CamelError::Config(
                "stream split batch_size must be > 0".into(),
            ));
        }
        if self.max_record_bytes == 0 {
            return Err(CamelError::Config(
                "stream split max_record_bytes must be > 0".into(),
            ));
        }
        if self.format == StreamSplitFormat::Chunks && self.chunk_size.is_none() {
            return Err(CamelError::Config(
                "stream split format=Chunks requires chunk_size".into(),
            ));
        }
        // Zip+chunk_size check must come before the generic chunk_size zero/exceeds
        // checks so that `Zip + Some(0)` yields the more specific error.
        if self.format == StreamSplitFormat::Zip && self.chunk_size.is_some() {
            return Err(CamelError::Config(
                "stream split format=Zip does not support chunk_size".into(),
            ));
        }
        if let Some(cs) = self.chunk_size
            && cs == 0
        {
            return Err(CamelError::Config(
                "stream split chunk_size must be > 0".into(),
            ));
        }
        if self.format == StreamSplitFormat::Chunks
            && let Some(cs) = self.chunk_size
            && cs > self.max_record_bytes
        {
            return Err(CamelError::Config(
                "stream split chunk_size must be <= max_record_bytes".into(),
            ));
        }
        Ok(())
    }
}

/// Configuration for the Splitter EIP.
pub struct SplitterConfig {
    /// Expression that splits an exchange into fragments.
    pub expression: SplitExpression,
    /// How to aggregate fragment results.
    pub aggregation: AggregationStrategy,
    /// Whether to process fragments in parallel.
    pub parallel: bool,
    /// Maximum number of parallel fragments (None = unlimited).
    pub parallel_limit: Option<usize>,
    /// Whether to stop processing on the first exception.
    ///
    /// In parallel mode this only affects aggregation (the first error is
    /// propagated), **not** in-flight futures — `join_all` cannot cancel
    /// already-spawned work.
    pub stop_on_exception: bool,
}

impl SplitterConfig {
    /// Create a new splitter config with the given split expression.
    pub fn new(expression: SplitExpression) -> Self {
        Self {
            expression,
            aggregation: AggregationStrategy::default(),
            parallel: false,
            parallel_limit: None,
            stop_on_exception: true,
        }
    }

    /// Set the aggregation strategy for combining fragment results.
    pub fn aggregation(mut self, strategy: AggregationStrategy) -> Self {
        self.aggregation = strategy;
        self
    }

    /// Enable or disable parallel fragment processing.
    pub fn parallel(mut self, parallel: bool) -> Self {
        self.parallel = parallel;
        self
    }

    /// Set the maximum number of concurrent fragments in parallel mode.
    pub fn parallel_limit(mut self, limit: usize) -> Self {
        self.parallel_limit = Some(limit);
        self
    }

    /// Control whether processing stops on the first fragment error.
    ///
    /// In parallel mode this only affects aggregation — see the field-level
    /// doc comment for details.
    pub fn stop_on_exception(mut self, stop: bool) -> Self {
        self.stop_on_exception = stop;
        self
    }

    /// Validates the configuration.
    ///
    /// Returns `Err(CamelError::Config)` if `parallel_limit` is set to 0,
    /// which would cause a `Semaphore::new(0)` panic at runtime.
    pub fn validate(&self) -> Result<(), CamelError> {
        if self.parallel && self.parallel_limit == Some(0) {
            return Err(CamelError::Config(
                "splitter parallel_limit must be > 0".to_string(),
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a fragment exchange that inherits headers, properties, and OTel context
/// from the parent, but with a new body.
///
/// # OpenTelemetry Trace Propagation
///
/// Each fragment inherits the parent's `otel_context`, which carries the active span
/// context. When TracingProcessor processes a fragment, it will create a child span
/// linked to the parent span. This creates a natural fan-out relationship in the
/// distributed trace:
///
/// ```text
/// ParentExchange (span A)
///   ├─ Fragment 1 (span B, child of A)
///   ├─ Fragment 2 (span C, child of A)
///   └─ Fragment N (span N, child of A)
/// ```
///
/// This parent-child relationship is the correct semantic for message splitting,
/// as fragments are logical subdivisions of the parent message, not independent
/// operations that merely reference the parent (which would warrant span links).
pub fn fragment_exchange(parent: &Exchange, body: Body) -> Exchange {
    let mut msg = Message::new(body);
    msg.headers = parent.input.headers.clone();
    let mut ex = Exchange::new(msg);
    ex.properties = parent.properties.clone();
    ex.pattern = parent.pattern;
    // Inherit OTel context so fragment spans are children of the parent span
    ex.otel_context = parent.otel_context.clone();
    ex
}

/// Split the exchange body by newlines. Returns one fragment per line.
/// Non-text bodies produce an empty vec.
pub fn split_body_lines() -> SplitExpression {
    Arc::new(|exchange: &Exchange| {
        let text = match &exchange.input.body {
            Body::Text(s) => s.as_str(),
            _ => return Vec::new(),
        };
        text.lines()
            .map(|line| fragment_exchange(exchange, Body::Text(line.to_string())))
            .collect()
    })
}

/// Split a JSON array body into one fragment per element.
/// Non-array bodies produce an empty vec.
pub fn split_body_json_array() -> SplitExpression {
    Arc::new(|exchange: &Exchange| {
        let arr = match &exchange.input.body {
            Body::Json(serde_json::Value::Array(arr)) => arr,
            _ => return Vec::new(),
        };
        arr.iter()
            .map(|val| fragment_exchange(exchange, Body::Json(val.clone())))
            .collect()
    })
}

/// Split the exchange body using a custom function that operates on the body.
pub fn split_body<F>(f: F) -> SplitExpression
where
    F: Fn(&Body) -> Vec<Body> + Send + Sync + 'static,
{
    Arc::new(move |exchange: &Exchange| {
        f(&exchange.input.body)
            .into_iter()
            .map(|body| fragment_exchange(exchange, body))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

    #[test]
    fn test_split_body_lines() {
        let mut ex = Exchange::new(Message::new("a\nb\nc"));
        ex.input.set_header("source", Value::String("test".into()));
        ex.set_property("trace", Value::Bool(true));

        let fragments = split_body_lines()(&ex);
        assert_eq!(fragments.len(), 3);
        assert_eq!(fragments[0].input.body.as_text(), Some("a"));
        assert_eq!(fragments[1].input.body.as_text(), Some("b"));
        assert_eq!(fragments[2].input.body.as_text(), Some("c"));

        // Verify headers and properties inherited
        for frag in &fragments {
            assert_eq!(
                frag.input.header("source"),
                Some(&Value::String("test".into()))
            );
            assert_eq!(frag.property("trace"), Some(&Value::Bool(true)));
        }
    }

    #[test]
    fn test_split_body_lines_empty() {
        let ex = Exchange::new(Message::default()); // Body::Empty
        let fragments = split_body_lines()(&ex);
        assert!(fragments.is_empty());
    }

    #[test]
    fn test_split_body_json_array() {
        let arr = serde_json::json!([1, 2, 3]);
        let ex = Exchange::new(Message::new(arr));

        let fragments = split_body_json_array()(&ex);
        assert_eq!(fragments.len(), 3);
        assert!(matches!(&fragments[0].input.body, Body::Json(v) if *v == serde_json::json!(1)));
        assert!(matches!(&fragments[1].input.body, Body::Json(v) if *v == serde_json::json!(2)));
        assert!(matches!(&fragments[2].input.body, Body::Json(v) if *v == serde_json::json!(3)));
    }

    #[test]
    fn test_split_body_json_array_not_array() {
        let obj = serde_json::json!({"not": "array"});
        let ex = Exchange::new(Message::new(obj));

        let fragments = split_body_json_array()(&ex);
        assert!(fragments.is_empty());
    }

    #[test]
    fn test_split_body_custom() {
        let splitter = split_body(|body: &Body| match body {
            Body::Text(s) => s
                .split(',')
                .map(|part| Body::Text(part.trim().to_string()))
                .collect(),
            _ => Vec::new(),
        });

        let mut ex = Exchange::new(Message::new("x, y, z"));
        ex.set_property("id", Value::from(42));

        let fragments = splitter(&ex);
        assert_eq!(fragments.len(), 3);
        assert_eq!(fragments[0].input.body.as_text(), Some("x"));
        assert_eq!(fragments[1].input.body.as_text(), Some("y"));
        assert_eq!(fragments[2].input.body.as_text(), Some("z"));

        // Properties inherited
        for frag in &fragments {
            assert_eq!(frag.property("id"), Some(&Value::from(42)));
        }
    }

    #[test]
    fn test_splitter_config_defaults() {
        let config = SplitterConfig::new(split_body_lines());
        assert!(matches!(config.aggregation, AggregationStrategy::LastWins));
        assert!(!config.parallel);
        assert!(config.parallel_limit.is_none());
        assert!(config.stop_on_exception);
    }

    #[test]
    fn test_splitter_config_builder() {
        let config = SplitterConfig::new(split_body_lines())
            .aggregation(AggregationStrategy::CollectAll)
            .parallel(true)
            .parallel_limit(4)
            .stop_on_exception(false);

        assert!(matches!(
            config.aggregation,
            AggregationStrategy::CollectAll
        ));
        assert!(config.parallel);
        assert_eq!(config.parallel_limit, Some(4));
        assert!(!config.stop_on_exception);
    }

    #[test]
    fn test_fragment_exchange_inherits_otel_context() {
        use opentelemetry::Context;
        use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId};

        // Create parent exchange with a valid span context
        let mut parent = Exchange::new(Message::new("test"));
        let trace_id = TraceId::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 123]);
        let span_id = SpanId::from_bytes([0, 0, 0, 0, 0, 0, 1, 200]);
        let span_context = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            true,
            Default::default(),
        );
        let expected_trace_id = span_context.trace_id();
        parent.otel_context = Context::current().with_remote_span_context(span_context);

        // Create fragment via split_body_lines
        let fragments = split_body_lines()(&parent);
        assert!(!fragments.is_empty(), "Should have at least one fragment");

        // Verify each fragment has the same span context as parent
        for fragment in &fragments {
            let span = fragment.otel_context.span();
            let frag_span_ctx = span.span_context();
            assert!(
                frag_span_ctx.is_valid(),
                "Fragment should have valid span context"
            );
            assert_eq!(
                frag_span_ctx.trace_id(),
                expected_trace_id,
                "Fragment should have same trace ID as parent"
            );
        }
    }

    #[test]
    fn test_stream_split_config_defaults_valid() {
        let config = StreamSplitConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_stream_split_config_batch_size_zero_rejected() {
        let config = StreamSplitConfig {
            batch_size: 0,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("batch_size"));
    }

    #[test]
    fn test_stream_split_config_max_record_bytes_zero_rejected() {
        let config = StreamSplitConfig {
            max_record_bytes: 0,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("max_record_bytes"));
    }

    #[test]
    fn test_stream_split_config_chunks_requires_chunk_size() {
        let config = StreamSplitConfig {
            format: StreamSplitFormat::Chunks,
            chunk_size: None,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("Chunks requires chunk_size"));
    }

    #[test]
    fn test_stream_split_config_chunk_size_zero_rejected() {
        let config = StreamSplitConfig {
            format: StreamSplitFormat::Chunks,
            chunk_size: Some(0),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("chunk_size must be > 0"));
    }

    #[test]
    fn test_stream_split_config_chunk_size_exceeds_max_record_bytes() {
        let config = StreamSplitConfig {
            format: StreamSplitFormat::Chunks,
            chunk_size: Some(2000),
            max_record_bytes: 1000,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("chunk_size must be <= max_record_bytes")
        );
    }

    #[test]
    fn test_stream_split_config_zip_rejects_chunk_size() {
        let config = StreamSplitConfig {
            format: StreamSplitFormat::Zip,
            chunk_size: Some(1024),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("Zip does not support chunk_size"));
    }

    #[test]
    fn test_all_fragments_share_same_trace_context() {
        use opentelemetry::Context;
        use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId};

        // Create parent with a specific trace ID
        let mut parent = Exchange::new(Message::new("line1\nline2\nline3"));
        let trace_id =
            TraceId::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x3B, 0x9A, 0xCA, 0x09]);
        let span_id = SpanId::from_bytes([0, 0, 0, 0, 0, 0, 0, 111]);
        let span_context = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            true,
            Default::default(),
        );
        parent.otel_context = Context::current().with_remote_span_context(span_context);

        let fragments = split_body_lines()(&parent);
        assert_eq!(fragments.len(), 3);

        // All fragments should share the same trace ID
        let trace_ids: Vec<_> = fragments
            .iter()
            .map(|f| {
                let span = f.otel_context.span();
                span.span_context().trace_id()
            })
            .collect();

        assert!(
            trace_ids.iter().all(|&id| id == trace_id),
            "All fragments should have the same trace ID"
        );
    }
}
