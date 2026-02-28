use bytes::Bytes;

/// The body of a message, supporting common payload types.
#[derive(Debug, Clone, Default)]
pub enum Body {
    /// No body content.
    #[default]
    Empty,
    /// Raw bytes payload.
    Bytes(Bytes),
    /// UTF-8 string payload.
    Text(String),
    /// JSON payload.
    Json(serde_json::Value),
}

impl Body {
    /// Returns `true` if the body is empty.
    pub fn is_empty(&self) -> bool {
        matches!(self, Body::Empty)
    }

    /// Try to get the body as a string, converting from bytes if needed.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Body::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

// Conversion impls
impl From<String> for Body {
    fn from(s: String) -> Self {
        Body::Text(s)
    }
}

impl From<&str> for Body {
    fn from(s: &str) -> Self {
        Body::Text(s.to_string())
    }
}

impl From<Bytes> for Body {
    fn from(b: Bytes) -> Self {
        Body::Bytes(b)
    }
}

impl From<Vec<u8>> for Body {
    fn from(v: Vec<u8>) -> Self {
        Body::Bytes(Bytes::from(v))
    }
}

impl From<serde_json::Value> for Body {
    fn from(v: serde_json::Value) -> Self {
        Body::Json(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_body_default_is_empty() {
        let body = Body::default();
        assert!(body.is_empty());
    }

    #[test]
    fn test_body_from_string() {
        let body = Body::from("hello".to_string());
        assert_eq!(body.as_text(), Some("hello"));
    }

    #[test]
    fn test_body_from_str() {
        let body = Body::from("world");
        assert_eq!(body.as_text(), Some("world"));
    }

    #[test]
    fn test_body_from_bytes() {
        let body = Body::from(Bytes::from_static(b"data"));
        assert!(!body.is_empty());
        assert!(matches!(body, Body::Bytes(_)));
    }

    #[test]
    fn test_body_from_json() {
        let val = serde_json::json!({"key": "value"});
        let body = Body::from(val.clone());
        assert!(matches!(body, Body::Json(_)));
    }
}
