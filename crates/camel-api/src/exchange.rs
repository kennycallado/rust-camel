use std::collections::HashMap;

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
#[derive(Debug, Clone)]
pub struct Exchange {
    /// The input (incoming) message.
    pub input: Message,
    /// The output (response) message, populated for InOut patterns.
    pub output: Option<Message>,
    /// Exchange-scoped properties for passing data between processors.
    pub properties: HashMap<String, Value>,
    /// Error state, if processing failed.
    pub error: Option<CamelError>,
    /// The exchange pattern.
    pub pattern: ExchangePattern,
}

impl Exchange {
    /// Create a new exchange with the given input message.
    pub fn new(input: Message) -> Self {
        Self {
            input,
            output: None,
            properties: HashMap::new(),
            error: None,
            pattern: ExchangePattern::default(),
        }
    }

    /// Create a new exchange with the InOut pattern.
    pub fn new_in_out(input: Message) -> Self {
        Self {
            input,
            output: None,
            properties: HashMap::new(),
            error: None,
            pattern: ExchangePattern::InOut,
        }
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
}
