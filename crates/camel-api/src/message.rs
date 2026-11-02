use std::collections::HashMap;

use crate::body::Body;
use crate::value::Value;

/// A message flowing through the Camel framework.
#[derive(Debug, Clone)]
pub struct Message {
    /// Raw message headers (metadata) — a raw escape hatch.
    ///
    /// Direct writes to this map bypass header-key normalization, so the map
    /// may contain several keys that differ only by ASCII case. Readers that
    /// need case folding MUST use [`Message::header_ic`] instead of indexing
    /// this map directly.
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

    /// Get a header value by key, matching the stored key exactly.
    pub fn header(&self, key: &str) -> Option<&Value> {
        self.headers.get(key)
    }

    /// Get a header value, matching case-insensitively.
    ///
    /// An exact-case entry wins. Without one, the value whose stored key sorts
    /// first by byte order among the ASCII case-insensitive matches is
    /// returned, so the result does not depend on `HashMap` iteration order.
    pub fn header_ic(&self, key: &str) -> Option<&Value> {
        if let Some(value) = self.headers.get(key) {
            return Some(value);
        }

        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(key))
            .min_by(|a, b| a.0.cmp(b.0))
            .map(|(_, v)| v)
    }

    /// Set a header value, replacing every ASCII case-insensitive equivalent.
    ///
    /// The caller's spelling is inserted verbatim, so iteration and
    /// serialization expose that spelling. This is O(n) in the number of
    /// headers.
    pub fn set_header(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        let key = key.into();
        let value = value.into();
        self.headers.retain(|k, _| !k.eq_ignore_ascii_case(&key));
        self.headers.insert(key, value);
    }

    /// Set a header value with a normalized lowercase key.
    ///
    /// Every ASCII case-insensitive duplicate is removed before the lowercase
    /// key is inserted.
    pub fn set_header_normalized(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        let key = key.into().to_ascii_lowercase();
        let value = value.into();
        self.headers.retain(|k, _| !k.eq_ignore_ascii_case(&key));
        self.headers.insert(key, value);
    }

    /// Remove a header, matching case-insensitively.
    ///
    /// Every entry whose key equals `key` ASCII case-insensitively is removed.
    /// Returns the exact-case entry's value when that entry existed, and
    /// `None` otherwise — including when only case variants were removed.
    pub fn remove_header(&mut self, key: &str) -> Option<Value> {
        let removed = self.headers.remove(key);
        self.headers.retain(|k, _| !k.eq_ignore_ascii_case(key));
        removed
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

    #[test]
    fn header_exact_lookup_stays_case_sensitive() {
        let mut msg = Message::default();
        msg.set_header("Content-Type", "application/xml");

        assert_eq!(msg.header("content-type"), None);
        assert_eq!(
            msg.header("Content-Type"),
            Some(&Value::String("application/xml".into()))
        );
    }

    #[test]
    fn set_header_case_variants_replace_each_other() {
        let mut msg = Message::default();
        msg.set_header("Content-Type", "a/xml");
        msg.set_header("content-type", "application/json");

        assert_eq!(msg.headers.len(), 1);
        assert!(msg.headers.contains_key("content-type"));
        assert_eq!(
            msg.header_ic("CONTENT-TYPE"),
            Some(&Value::String("application/json".into()))
        );
    }

    #[test]
    fn set_header_mixed_case_replaces_existing() {
        let mut msg = Message::default();
        msg.set_header("content-type", "text/plain");
        msg.set_header("CoNtEnT-TyPe", "text/csv");

        assert_eq!(msg.headers.len(), 1);
        assert!(msg.headers.contains_key("CoNtEnT-TyPe"));
    }

    #[test]
    fn remove_header_is_case_insensitive() {
        let mut msg = Message::default();
        msg.set_header("X-Request-Id", "abc");

        // Spec: only an exact-case entry's value is returned, so a
        // case-insensitive-only removal yields `None`; the entry is still gone.
        assert_eq!(msg.remove_header("x-request-id"), None);
        assert_eq!(msg.header("X-Request-Id"), None);
        assert_eq!(msg.header_ic("x-request-id"), None);
    }

    #[test]
    fn remove_header_returns_none_without_exact_case_match() {
        let mut msg = Message::default();
        msg.set_header("Other", "keep");
        msg.headers.insert("X-Dup".into(), 1.into());

        assert_eq!(msg.remove_header("x-dup"), None);
        assert!(!msg.headers.contains_key("X-Dup"));
        assert_eq!(msg.header("Other"), Some(&Value::String("keep".into())));
    }

    #[test]
    fn remove_header_absent_returns_none_leaves_others() {
        let mut msg = Message::default();
        msg.set_header("A", "one");
        msg.set_header("B", 2);

        assert_eq!(msg.remove_header("x-missing"), None);
        assert_eq!(msg.headers.len(), 2);
        assert_eq!(msg.header("A"), Some(&Value::String("one".into())));
        assert_eq!(msg.header("B"), Some(&Value::from(2)));
    }

    #[test]
    fn header_ic_duplicate_spellings_resolve_deterministically() {
        for i in 0..100 {
            let mut msg = Message::default();
            if i % 2 == 0 {
                msg.headers.insert("content-type".into(), "lower".into());
                msg.headers.insert("Content-Type".into(), "title".into());
                msg.headers.insert("x-dup".into(), "lower".into());
                msg.headers.insert("X-Dup".into(), "upper".into());
            } else {
                msg.headers.insert("Content-Type".into(), "title".into());
                msg.headers.insert("content-type".into(), "lower".into());
                msg.headers.insert("X-Dup".into(), "upper".into());
                msg.headers.insert("x-dup".into(), "lower".into());
            }

            assert_eq!(
                msg.header_ic("Content-Type"),
                Some(&Value::String("title".into()))
            );
            assert_eq!(
                msg.header_ic("CONTENT-TYPE"),
                Some(&Value::String("title".into()))
            );
            assert_eq!(msg.header_ic("X-DUP"), Some(&Value::String("upper".into())));
            assert_eq!(
                msg.header_ic("content-type"),
                Some(&Value::String("lower".into()))
            );
            assert_eq!(msg.header_ic("x-dup"), Some(&Value::String("lower".into())));
        }
    }

    #[test]
    fn set_header_normalized_deduplicates() {
        let mut msg = Message::default();
        msg.set_header("X-Request-Id", "old");
        msg.set_header_normalized("X-Request-Id", "abc");

        assert_eq!(msg.headers.len(), 1);
        assert!(msg.headers.contains_key("x-request-id"));
        assert_eq!(
            msg.header("x-request-id"),
            Some(&Value::String("abc".into()))
        );
    }

    #[test]
    fn header_iteration_and_json_preserve_spelling() {
        let mut msg = Message::default();
        msg.set_header("X-Request-Id", "abc");

        assert!(msg.headers.keys().any(|k| k == "X-Request-Id"));
        let json = serde_json::to_value(&msg.headers).expect("headers serialize to object");
        let obj = json.as_object().expect("headers serialize to JSON object");
        assert_eq!(obj.get("X-Request-Id"), Some(&Value::String("abc".into())));
        assert!(!obj.contains_key("x-request-id"));
    }

    #[test]
    fn raw_map_write_visible_to_header_ic_only() {
        let mut msg = Message::default();
        msg.headers.insert("X-Raw".into(), 1.into());

        assert_eq!(msg.header_ic("x-raw"), Some(&Value::from(1)));
        assert_eq!(msg.header("x-raw"), None);
    }
}
