use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use opentelemetry::Context;
use uuid::Uuid;

use crate::error::CamelError;
use crate::message::Message;
use crate::value::Value;

/// The exchange pattern (fire-and-forget or request-reply).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExchangePattern {
    /// Fire-and-forget: message sent, no reply expected.
    #[default]
    InOnly,
    /// Request-reply: message sent, reply expected.
    InOut,
}

/// An Exchange represents a message being routed through the framework.
///
/// It contains the input message, an optional output message,
/// properties for passing data between processors, and error state.
#[derive(Debug)]
pub struct Exchange {
    /// The input (incoming) message.
    pub input: Message,
    /// The output (response) message, populated for InOut patterns.
    pub output: Option<Message>,
    /// Exchange-scoped properties for passing data between processors.
    pub properties: HashMap<String, Value>,
    /// Non-serializable extension values (e.g., channel senders).
    /// Stored as `Arc<dyn Any + Send + Sync>` so cloning is cheap (ref-count bump).
    pub extensions: HashMap<String, Arc<dyn Any + Send + Sync>>,
    /// Error state, if processing failed.
    pub error: Option<CamelError>,
    /// The exchange pattern.
    pub pattern: ExchangePattern,
    /// Unique correlation ID for distributed tracing.
    pub correlation_id: String,
    /// OpenTelemetry context for distributed tracing propagation.
    /// Carries the active span context between processing steps.
    /// Defaults to an empty context (noop span) if OTel is not active.
    pub otel_context: Context,
}

impl Exchange {
    /// Create a new exchange with the given input message.
    pub fn new(input: Message) -> Self {
        Self {
            input,
            output: None,
            properties: HashMap::new(),
            extensions: HashMap::new(),
            error: None,
            pattern: ExchangePattern::default(),
            correlation_id: Uuid::new_v4().to_string(),
            otel_context: Context::new(),
        }
    }

    /// Create a new exchange with the InOut pattern.
    pub fn new_in_out(input: Message) -> Self {
        Self {
            input,
            output: None,
            properties: HashMap::new(),
            extensions: HashMap::new(),
            error: None,
            pattern: ExchangePattern::InOut,
            correlation_id: Uuid::new_v4().to_string(),
            otel_context: Context::new(),
        }
    }

    /// Get the correlation ID for this exchange.
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }

    /// Get a property value.
    pub fn property(&self, key: &str) -> Option<&Value> {
        self.properties.get(key)
    }

    /// Set a property value.
    pub fn set_property(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.properties.insert(key.into(), value.into());
    }

    /// Check if the exchange has an error.
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }

    /// Set an error on this exchange.
    pub fn set_error(&mut self, error: CamelError) {
        self.error = Some(error);
    }

    /// Store a non-serializable extension value (e.g. a channel sender).
    pub fn set_extension(&mut self, key: impl Into<String>, value: Arc<dyn Any + Send + Sync>) {
        self.extensions.insert(key.into(), value);
    }

    /// Retrieve a typed extension value. Returns `None` if the key is absent
    /// or the stored value is not of type `T`.
    pub fn get_extension<T: Any>(&self, key: &str) -> Option<&T> {
        self.extensions.get(key)?.downcast_ref::<T>()
    }
}

impl Clone for Exchange {
    fn clone(&self) -> Self {
        Self {
            input: self.input.clone(),
            output: self.output.clone(),
            properties: self.properties.clone(),
            extensions: self.extensions.clone(), // Arc ref-count bump, cheap
            error: self.error.clone(),
            pattern: self.pattern,
            correlation_id: self.correlation_id.clone(),
            otel_context: self.otel_context.clone(),
        }
    }
}

impl Default for Exchange {
    fn default() -> Self {
        Self::new(Message::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_new() {
        let msg = Message::new("test");
        let ex = Exchange::new(msg);
        assert_eq!(ex.input.body.as_text(), Some("test"));
        assert!(ex.output.is_none());
        assert!(!ex.has_error());
        assert_eq!(ex.pattern, ExchangePattern::InOnly);
    }

    #[test]
    fn test_exchange_in_out() {
        let ex = Exchange::new_in_out(Message::default());
        assert_eq!(ex.pattern, ExchangePattern::InOut);
    }

    #[test]
    fn test_exchange_properties() {
        let mut ex = Exchange::default();
        ex.set_property("key", Value::Bool(true));
        assert_eq!(ex.property("key"), Some(&Value::Bool(true)));
        assert_eq!(ex.property("missing"), None);
    }

    #[test]
    fn test_exchange_error() {
        let mut ex = Exchange::default();
        assert!(!ex.has_error());
        ex.set_error(CamelError::ProcessorError("test".into()));
        assert!(ex.has_error());
    }

    #[test]
    fn test_exchange_lifecycle() {
        let mut ex = Exchange::new(Message::new("input data"));
        assert_eq!(ex.input.body.as_text(), Some("input data"));

        // Set some properties
        ex.set_property("processed", Value::Bool(true));

        // Set output
        ex.output = Some(Message::new("output data"));
        assert!(ex.output.is_some());

        // Verify no error
        assert!(!ex.has_error());
    }

    #[test]
    fn test_exchange_otel_context_default() {
        let ex = Exchange::default();
        // Field must exist and be accessible — compilation is the test
        // Also verify it's a fresh context (noop span)
        use opentelemetry::trace::TraceContextExt;
        assert!(!ex.otel_context.span().span_context().is_valid());
    }

    #[test]
    fn test_exchange_otel_context_propagates_in_clone() {
        let ex = Exchange::default();
        let cloned = ex.clone();
        // Both should have the same (empty) context
        use opentelemetry::trace::TraceContextExt;
        assert!(!cloned.otel_context.span().span_context().is_valid());
    }

    #[test]
    fn test_set_and_get_extension() {
        use std::sync::Arc;
        let mut ex = Exchange::default();
        ex.set_extension("my.key", Arc::new(42u32));
        let val: Option<&u32> = ex.get_extension("my.key");
        assert_eq!(val, Some(&42u32));
    }

    #[test]
    fn test_get_extension_wrong_type_returns_none() {
        use std::sync::Arc;
        let mut ex = Exchange::default();
        ex.set_extension("my.key", Arc::new(42u32));
        let val: Option<&String> = ex.get_extension("my.key");
        assert!(val.is_none());
    }

    #[test]
    fn test_get_extension_missing_key_returns_none() {
        let ex = Exchange::default();
        let val: Option<&u32> = ex.get_extension("nope");
        assert!(val.is_none());
    }

    #[test]
    fn test_clone_shares_extension_arc() {
        use std::sync::Arc;
        let mut ex = Exchange::default();
        ex.set_extension("shared", Arc::new(99u64));
        let cloned = ex.clone();
        // Both see the same value
        assert_eq!(ex.get_extension::<u64>("shared"), Some(&99u64));
        assert_eq!(cloned.get_extension::<u64>("shared"), Some(&99u64));
    }
}
