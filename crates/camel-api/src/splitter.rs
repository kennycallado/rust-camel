use std::sync::Arc;

use crate::body::Body;
use crate::exchange::Exchange;
use crate::message::Message;

/// A function that splits a single exchange into multiple fragment exchanges.
pub type SplitExpression = Arc<dyn Fn(&Exchange) -> Vec<Exchange> + Send + Sync>;

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
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a fragment exchange that inherits headers and properties from the
/// parent, but with a new body.
fn fragment_exchange(parent: &Exchange, body: Body) -> Exchange {
    let mut msg = Message::new(body);
    msg.headers = parent.input.headers.clone();
    let mut ex = Exchange::new(msg);
    ex.properties = parent.properties.clone();
    ex.pattern = parent.pattern;
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
}
