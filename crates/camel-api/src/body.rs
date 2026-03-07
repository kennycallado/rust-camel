use crate::error::CamelError;
use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use futures::stream::BoxStream;
use std::sync::Arc;
use tokio::sync::Mutex;

const DEFAULT_MATERIALIZE_LIMIT: usize = 10 * 1024 * 1024;

/// Metadata associated with a stream body.
#[derive(Debug, Clone, Default)]
pub struct StreamMetadata {
    /// Expected size of the stream if known.
    pub size_hint: Option<u64>,
    /// Content type of the stream content.
    pub content_type: Option<String>,
    /// Origin of the stream (e.g. "file:///path/to/file").
    pub origin: Option<String>,
}

/// A body that wraps a lazy-evaluated stream of bytes.
pub struct StreamBody {
    /// The actual byte stream, wrapped in an Arc and Mutex to allow Clone for Body.
    ///
    /// ### Clone Semantics
    /// The stream is **single-consumption**. When cloning a `Body::Stream`,
    /// all clones share the same underlying stream handle. Only the first
    /// clone to consume the stream will succeed; subsequent attempts will
    /// return `CamelError::AlreadyConsumed`.
    #[allow(clippy::type_complexity)]
    pub stream: Arc<Mutex<Option<BoxStream<'static, Result<Bytes, CamelError>>>>>,
    /// Metadata associated with the stream.
    pub metadata: StreamMetadata,
}

impl std::fmt::Debug for StreamBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamBody")
            .field("metadata", &self.metadata)
            .field("stream", &"<BoxStream>")
            .finish()
    }
}

impl Clone for StreamBody {
    fn clone(&self) -> Self {
        Self {
            stream: Arc::clone(&self.stream),
            metadata: self.metadata.clone(),
        }
    }
}

/// The body of a message, supporting common payload types.
#[derive(Debug, Default)]
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
    /// Streaming payload.
    Stream(StreamBody),
}

impl Clone for Body {
    fn clone(&self) -> Self {
        match self {
            Body::Empty => Body::Empty,
            Body::Bytes(b) => Body::Bytes(b.clone()),
            Body::Text(s) => Body::Text(s.clone()),
            Body::Json(v) => Body::Json(v.clone()),
            Body::Stream(s) => Body::Stream(s.clone()),
        }
    }
}

impl Body {
    /// Returns `true` if the body is empty.
    pub fn is_empty(&self) -> bool {
        matches!(self, Body::Empty)
    }

    /// Convert the body into `Bytes`, consuming it if it is a stream.
    /// This is an async operation because it may need to read from an underlying stream.
    /// A `max_size` limit is enforced to prevent OOM errors.
    pub async fn into_bytes(self, max_size: usize) -> Result<Bytes, CamelError> {
        match self {
            Body::Empty => Ok(Bytes::new()),
            Body::Bytes(b) => {
                if b.len() > max_size {
                    return Err(CamelError::StreamLimitExceeded(max_size));
                }
                Ok(b)
            }
            Body::Text(s) => {
                if s.len() > max_size {
                    return Err(CamelError::StreamLimitExceeded(max_size));
                }
                Ok(Bytes::from(s))
            }
            Body::Json(v) => {
                let b = serde_json::to_vec(&v)
                    .map_err(|e| CamelError::TypeConversionFailed(e.to_string()))?;
                if b.len() > max_size {
                    return Err(CamelError::StreamLimitExceeded(max_size));
                }
                Ok(Bytes::from(b))
            }
            Body::Stream(s) => {
                let mut stream_lock = s.stream.lock().await;
                let mut stream = stream_lock.take().ok_or(CamelError::AlreadyConsumed)?;

                let mut buffer = BytesMut::new();
                while let Some(chunk_res) = stream.next().await {
                    let chunk = chunk_res?;
                    if buffer.len() + chunk.len() > max_size {
                        return Err(CamelError::StreamLimitExceeded(max_size));
                    }
                    buffer.extend_from_slice(&chunk);
                }
                Ok(buffer.freeze())
            }
        }
    }

    /// Materialize stream with sensible default limit (10MB).
    /// 
    /// Convenience method for common cases where you need the stream content
    /// but don't want to specify a custom limit.
    /// 
    /// # Example
    /// ```ignore
    /// let body = Body::Stream(stream);
    /// let bytes = body.materialize().await?;
    /// ```
    pub async fn materialize(self) -> Result<Bytes, CamelError> {
        self.into_bytes(DEFAULT_MATERIALIZE_LIMIT).await
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

    #[tokio::test]
    async fn test_into_bytes_from_stream() {
        use futures::stream;
        let chunks = vec![Ok(Bytes::from("hello ")), Ok(Bytes::from("world"))];
        let stream = stream::iter(chunks);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        });

        let result = body.into_bytes(100).await.unwrap();
        assert_eq!(result, Bytes::from("hello world"));
    }

    #[tokio::test]
    async fn test_into_bytes_limit_exceeded() {
        use futures::stream;
        let chunks = vec![Ok(Bytes::from("this is too long"))];
        let stream = stream::iter(chunks);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        });

        let result = body.into_bytes(5).await;
        assert!(matches!(result, Err(CamelError::StreamLimitExceeded(5))));
    }

    #[tokio::test]
    async fn test_into_bytes_already_consumed() {
        use futures::stream;
        let chunks = vec![Ok(Bytes::from("data"))];
        let stream = stream::iter(chunks);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        });

        let cloned = body.clone();
        let _ = body.into_bytes(100).await.unwrap();

        let result = cloned.into_bytes(100).await;
        assert!(matches!(result, Err(CamelError::AlreadyConsumed)));
    }

    #[tokio::test]
    async fn test_materialize_with_default_limit() {
        use futures::stream;
        
        // Small stream under limit - should succeed with default 10MB limit
        let chunks = vec![Ok(Bytes::from("test data"))];
        let stream = stream::iter(chunks);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        });

        let result = body.materialize().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Bytes::from("test data"));
    }

    #[tokio::test]
    async fn test_materialize_non_stream_body_types() {
        // Verify materialize() works with all body types, not just streams
        
        // Body::Empty
        let body = Body::Empty;
        let result = body.materialize().await.unwrap();
        assert!(result.is_empty());

        // Body::Bytes
        let body = Body::Bytes(Bytes::from("bytes data"));
        let result = body.materialize().await.unwrap();
        assert_eq!(result, Bytes::from("bytes data"));

        // Body::Text
        let body = Body::Text("text data".to_string());
        let result = body.materialize().await.unwrap();
        assert_eq!(result, Bytes::from("text data"));

        // Body::Json
        let body = Body::Json(serde_json::json!({"key": "value"}));
        let result = body.materialize().await.unwrap();
        assert_eq!(result, Bytes::from_static(br#"{"key":"value"}"#));
    }

    #[tokio::test]
    async fn test_materialize_exceeds_default_limit() {
        use futures::stream;
        
        // 15MB stream - should fail with default 10MB limit
        let large_data = vec![0u8; 15 * 1024 * 1024];
        let chunks = vec![Ok(Bytes::from(large_data))];
        let stream = stream::iter(chunks);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        });

        let result = body.materialize().await;
        assert!(matches!(result, Err(CamelError::StreamLimitExceeded(10_485_760))));
    }
}
