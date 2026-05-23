use std::collections::HashMap;

use crate::body::Body;
use crate::value::Value;

/// A message flowing through the Camel framework.
#[derive(Debug, Clone)]
pub struct Message {
    /// Message headers (metadata).
    // TODO(API-003): Replace HashMap with a proper CaseInsensitiveMap for full HTTP compliance.
    pub headers: HashMap<String, Value>,
    /// Message body (payload).
    pub body: Body,
}

impl Default for Message {
    fn default() -> Self {
        Self {
            headers: HashMap::new(),
            body: Body::Empty,
        }
    }
}

impl Message {
    /// Create a new message with the given body.
    pub fn new(body: impl Into<Body>) -> Self {
        Self {
            headers: HashMap::new(),
            body: body.into(),
        }
    }

    /// Get a header value by key.
    pub fn header(&self, key: &str) -> Option<&Value> {
        self.headers.get(key)
    }

    /// Get a header value, matching case-insensitively.
    pub fn header_ic(&self, key: &str) -> Option<&Value> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    /// Set a header value.
    pub fn set_header(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.headers.insert(key.into(), value.into());
    }

    /// Set a header value with a normalized lowercase key.
    pub fn set_header_normalized(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.headers
            .insert(key.into().to_ascii_lowercase(), value.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_default() {
        let msg = Message::default();
        assert!(msg.body.is_empty());
        assert!(msg.headers.is_empty());
    }

    #[test]
    fn test_message_new_with_body() {
        let msg = Message::new("hello");
        assert_eq!(msg.body.as_text(), Some("hello"));
    }

    #[test]
    fn test_message_headers() {
        let mut msg = Message::default();
        msg.set_header("key", Value::String("value".into()));
        assert_eq!(msg.header("key"), Some(&Value::String("value".into())));
        assert_eq!(msg.header("missing"), None);
    }

    #[test]
    fn test_header_ic_case_insensitive() {
        let mut msg = Message::default();
        msg.set_header("Content-Type", Value::String("application/json".into()));
        assert_eq!(
            msg.header_ic("content-type"),
            Some(&Value::String("application/json".into()))
        );
        assert_eq!(
            msg.header_ic("CONTENT-TYPE"),
            Some(&Value::String("application/json".into()))
        );
    }

    #[test]
    fn test_set_header_normalized_lowercases_key() {
        let mut msg = Message::default();
        msg.set_header_normalized("X-Request-Id", Value::String("abc-123".into()));

        assert!(msg.headers.contains_key("x-request-id"));
        assert_eq!(
            msg.header("x-request-id"),
            Some(&Value::String("abc-123".into()))
        );
    }
}
