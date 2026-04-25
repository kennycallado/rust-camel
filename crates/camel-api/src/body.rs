use crate::error::CamelError;
/// General-purpose default limit for [`Body::materialize()`] (10 MB).
///
/// This is separate from [`stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD`] (128 KB),
/// which is the OOM-protection limit used by `StreamCacheService`.
pub const DEFAULT_MATERIALIZE_LIMIT: usize = 10 * 1024 * 1024;

use bytes::{Bytes, BytesMut};
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::Mutex;
use tokio_util::io::StreamReader;

/// A boxed [`AsyncRead`] for reading body content without materializing it into memory.
///
/// Returned by [`Body::into_async_read()`].
pub type BoxAsyncRead = Pin<Box<dyn AsyncRead + Send + Unpin>>;

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
///
/// # Clone Semantics
///
/// The stream is **single-consumption**. When cloning a `Body::Stream`,
/// all clones share the same underlying stream handle. Only the first
/// clone to consume the stream will succeed; subsequent attempts will
/// return `CamelError::AlreadyConsumed`.
///
/// # Example
///
/// ```rust
/// use camel_api::{Body, StreamBody, error::CamelError};
/// use futures::stream;
/// use bytes::Bytes;
/// use std::sync::Arc;
/// use tokio::sync::Mutex;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), CamelError> {
/// let chunks = vec![Ok(Bytes::from("data"))];
/// let stream = stream::iter(chunks);
/// let body = Body::Stream(StreamBody {
///     stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
///     metadata: Default::default(),
/// });
///
/// let clone = body.clone();
///
/// // First consumption succeeds
/// let _ = body.into_bytes(1024).await?;
///
/// // Second consumption fails
/// let result = clone.into_bytes(1024).await;
/// assert!(matches!(result, Err(CamelError::AlreadyConsumed)));
/// # Ok(())
/// # }
/// ```
pub struct StreamBody {
    /// The actual byte stream, wrapped in an Arc and Mutex to allow Clone for Body.
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

// ---------------------------------------------------------------------------
// StreamAsyncRead — adapts Body::Stream into AsyncRead
// ---------------------------------------------------------------------------

/// Private adapter that implements [`AsyncRead`] for [`Body::Stream`].
///
/// On the first `poll_read`, attempts a non-blocking `try_lock()` on the inner
/// `Arc<Mutex<Option<BoxStream>>>`:
/// - If the lock succeeds and the stream is `Some`, extracts it and creates
///   an active [`StreamReader`].
/// - If the stream is `None` (already consumed), returns an [`io::Error`].
/// - If the lock is contended (extremely rare), wakes the task and returns
///   `Poll::Pending` to retry.
#[allow(clippy::type_complexity)]
struct StreamAsyncRead {
    arc: Arc<Mutex<Option<BoxStream<'static, Result<Bytes, CamelError>>>>>,
    /// Holds the active reader after the stream is extracted on first poll.
    reader: Option<Box<dyn AsyncRead + Send + Unpin>>,
    /// Set to true when the stream was already consumed (prevents further reads).
    consumed: bool,
}

impl AsyncRead for StreamAsyncRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.consumed {
            return Poll::Ready(Err(io::Error::other("stream already consumed")));
        }
        // Lazy init: extract the stream on first poll
        if self.reader.is_none() {
            // Extract stream in a separate scope to avoid holding the lock while modifying self
            let extracted = {
                match self.arc.try_lock() {
                    Ok(mut guard) => guard.take(),
                    Err(_) => {
                        // Lock contended — schedule a retry
                        cx.waker().wake_by_ref();
                        return Poll::Pending;
                    }
                }
            };
            // Now safe to modify self since lock is dropped
            match extracted {
                Some(stream) => {
                    let mapped = stream.map_err(|e: CamelError| io::Error::other(e.to_string()));
                    self.reader = Some(Box::new(StreamReader::new(mapped)));
                }
                None => {
                    self.consumed = true;
                    return Poll::Ready(Err(io::Error::other("stream already consumed")));
                }
            }
        }
        // Delegate to the active reader
        Pin::new(self.reader.as_mut().unwrap()).poll_read(cx, buf)
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
    /// XML payload (well-formed XML string; use `try_into_xml()` for validation).
    Xml(String),
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
            Body::Xml(s) => Body::Xml(s.clone()),
            Body::Stream(s) => Body::Stream(s.clone()),
        }
    }
}

impl PartialEq for Body {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Body::Empty, Body::Empty) => true,
            (Body::Text(a), Body::Text(b)) => a == b,
            (Body::Json(a), Body::Json(b)) => a == b,
            (Body::Bytes(a), Body::Bytes(b)) => a == b,
            (Body::Xml(a), Body::Xml(b)) => a == b,
            // Stream: two streams are never equal (single-consumption)
            _ => false,
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
            Body::Xml(s) => {
                if s.len() > max_size {
                    return Err(CamelError::StreamLimitExceeded(max_size));
                }
                Ok(Bytes::from(s))
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

    /// Materialize stream with sensible default limit (10 MB).
    ///
    /// Convenience method for common cases where you need the stream content
    /// but don't want to specify a custom limit. For the tighter stream-cache
    /// threshold (128 KB), use [`stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD`]
    /// with [`Body::into_bytes()`] instead.
    ///
    /// # Example
    /// ```ignore
    /// let body = Body::Stream(stream);
    /// let bytes = body.materialize().await?;
    /// ```
    pub async fn materialize(self) -> Result<Bytes, CamelError> {
        self.into_bytes(DEFAULT_MATERIALIZE_LIMIT).await
    }

    /// Convert the body into an [`AsyncRead`] without materializing it into memory.
    ///
    /// - [`Body::Empty`] → empty reader (0 bytes)
    /// - [`Body::Bytes`] → in-memory cursor
    /// - [`Body::Text`] → UTF-8 bytes cursor
    /// - [`Body::Json`] → serialized JSON bytes cursor
    /// - [`Body::Xml`] → UTF-8 bytes cursor
    /// - [`Body::Stream`] → streams chunk-by-chunk via [`StreamReader`];
    ///   if the stream was already consumed, the reader returns an [`io::Error`]
    ///   on the first read
    pub fn into_async_read(self) -> BoxAsyncRead {
        match self {
            Body::Empty => Box::pin(tokio::io::empty()),
            Body::Bytes(b) => Box::pin(std::io::Cursor::new(b)),
            Body::Text(s) => Box::pin(std::io::Cursor::new(s.into_bytes())),
            Body::Json(v) => match serde_json::to_vec(&v) {
                Ok(bytes) => Box::pin(std::io::Cursor::new(bytes)) as BoxAsyncRead,
                Err(e) => {
                    let err = io::Error::new(io::ErrorKind::InvalidData, e.to_string());
                    let stream = futures::stream::iter(vec![Err::<Bytes, io::Error>(err)]);
                    Box::pin(StreamReader::new(stream)) as BoxAsyncRead
                }
            },
            Body::Xml(s) => Box::pin(std::io::Cursor::new(s.into_bytes())),
            Body::Stream(s) => Box::pin(StreamAsyncRead {
                arc: s.stream,
                reader: None,
                consumed: false,
            }),
        }
    }

    /// Try to get the body as a string, converting from bytes if needed.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Body::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Try to get the body as an XML string.
    pub fn as_xml(&self) -> Option<&str> {
        match self {
            Body::Xml(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Convert this body to `Body::Text`, consuming it.
    /// Returns `Err(TypeConversionFailed)` if the conversion is not possible.
    /// `Body::Stream` always fails — materialize with `into_bytes()` first.
    pub fn try_into_text(self) -> Result<Body, CamelError> {
        crate::body_converter::convert(self, crate::body_converter::BodyType::Text)
    }

    /// Convert this body to `Body::Json`, consuming it.
    /// Returns `Err(TypeConversionFailed)` if the conversion is not possible.
    /// `Body::Stream` always fails — materialize with `into_bytes()` first.
    pub fn try_into_json(self) -> Result<Body, CamelError> {
        crate::body_converter::convert(self, crate::body_converter::BodyType::Json)
    }

    /// Convert this body to `Body::Bytes`, consuming it.
    /// Returns `Err(TypeConversionFailed)` if the conversion is not possible.
    /// `Body::Stream` always fails — materialize with `into_bytes()` first.
    pub fn try_into_bytes_body(self) -> Result<Body, CamelError> {
        crate::body_converter::convert(self, crate::body_converter::BodyType::Bytes)
    }

    /// Convert this body to `Body::Xml`, consuming it.
    /// Returns `Err(TypeConversionFailed)` if the conversion is not possible.
    /// `Body::Stream` always fails — materialize with `into_bytes()` first.
    pub fn try_into_xml(self) -> Result<Body, CamelError> {
        crate::body_converter::convert(self, crate::body_converter::BodyType::Xml)
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

        // Body::Xml
        let xml = "<root><child>value</child></root>";
        let body = Body::Xml(xml.to_string());
        let result = body.materialize().await.unwrap();
        assert_eq!(result, Bytes::from(xml));
    }

    #[tokio::test]
    async fn test_materialize_exceeds_default_limit() {
        use futures::stream;

        // 11MB stream - should fail with default 10MB limit
        let large_data = vec![0u8; 11 * 1024 * 1024];
        let chunks = vec![Ok(Bytes::from(large_data))];
        let stream = stream::iter(chunks);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        });

        let result = body.materialize().await;
        assert!(matches!(
            result,
            Err(CamelError::StreamLimitExceeded(10_485_760))
        ));
    }

    #[test]
    fn stream_variants_are_never_equal() {
        use futures::stream;

        let make_stream = || {
            let s = stream::iter(vec![Ok(Bytes::from_static(b"data"))]);
            Body::Stream(StreamBody {
                stream: Arc::new(Mutex::new(Some(Box::pin(s)))),
                metadata: StreamMetadata::default(),
            })
        };
        assert_ne!(make_stream(), make_stream());
    }

    // XML body tests

    #[test]
    fn test_body_xml_as_xml() {
        let xml = "<root><child>value</child></root>";
        let body = Body::Xml(xml.to_string());
        assert_eq!(body.as_xml(), Some(xml));
    }

    #[test]
    fn test_body_non_xml_as_xml_returns_none() {
        // Body::Text should return None for as_xml()
        let body = Body::Text("<root/>".to_string());
        assert_eq!(body.as_xml(), None);

        // Body::Empty should return None
        let body = Body::Empty;
        assert_eq!(body.as_xml(), None);

        // Body::Bytes should return None
        let body = Body::Bytes(Bytes::from("<root/>"));
        assert_eq!(body.as_xml(), None);

        // Body::Json should return None
        let body = Body::Json(serde_json::json!({"key": "value"}));
        assert_eq!(body.as_xml(), None);
    }

    #[test]
    fn test_body_xml_partial_eq() {
        // Same XML content should be equal
        let body1 = Body::Xml("a".to_string());
        let body2 = Body::Xml("a".to_string());
        assert_eq!(body1, body2);

        // Different XML content should not be equal
        let body1 = Body::Xml("a".to_string());
        let body2 = Body::Xml("b".to_string());
        assert_ne!(body1, body2);
    }

    #[test]
    fn test_body_xml_not_equal_to_other_variants() {
        // Body::Xml should not equal Body::Text even with same content
        let xml_body = Body::Xml("x".to_string());
        let text_body = Body::Text("x".to_string());
        assert_ne!(xml_body, text_body);
    }

    #[test]
    fn test_try_into_xml_from_text() {
        let body = Body::Text("<root/>".to_string());
        let result = body.try_into_xml();
        assert!(matches!(result, Ok(Body::Xml(ref s)) if s == "<root/>"));
    }

    #[test]
    fn test_try_into_xml_invalid_text() {
        let body = Body::Text("not xml".to_string());
        let result = body.try_into_xml();
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_body_xml_clone() {
        let original = Body::Xml("hello".to_string());
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    // ---------- into_async_read tests ----------

    #[tokio::test]
    async fn test_into_async_read_empty() {
        use tokio::io::AsyncReadExt;
        let body = Body::Empty;
        let mut reader = body.into_async_read();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn test_into_async_read_bytes() {
        use tokio::io::AsyncReadExt;
        let body = Body::Bytes(Bytes::from("hello"));
        let mut reader = body.into_async_read();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"hello");
    }

    #[tokio::test]
    async fn test_into_async_read_text() {
        use tokio::io::AsyncReadExt;
        let body = Body::Text("world".to_string());
        let mut reader = body.into_async_read();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"world");
    }

    #[tokio::test]
    async fn test_into_async_read_json() {
        use tokio::io::AsyncReadExt;
        let body = Body::Json(serde_json::json!({"key": "val"}));
        let mut reader = body.into_async_read();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed["key"], "val");
    }

    #[tokio::test]
    async fn test_into_async_read_xml() {
        use tokio::io::AsyncReadExt;
        let body = Body::Xml("<root/>".to_string());
        let mut reader = body.into_async_read();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"<root/>");
    }

    #[tokio::test]
    async fn test_into_async_read_stream_multichunk() {
        use tokio::io::AsyncReadExt;
        let chunks: Vec<Result<Bytes, CamelError>> = vec![
            Ok(Bytes::from("foo")),
            Ok(Bytes::from("bar")),
            Ok(Bytes::from("baz")),
        ];
        let stream = futures::stream::iter(chunks);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata {
                size_hint: None,
                content_type: None,
                origin: None,
            },
        });
        let mut reader = body.into_async_read();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"foobarbaz");
    }

    #[tokio::test]
    async fn test_into_async_read_already_consumed() {
        use tokio::io::AsyncReadExt;
        // Mutex holds None → stream already consumed
        type MaybeStream = Arc<Mutex<Option<BoxStream<'static, Result<Bytes, CamelError>>>>>;
        let arc: MaybeStream = Arc::new(Mutex::new(None));
        let body = Body::Stream(StreamBody {
            stream: arc,
            metadata: StreamMetadata {
                size_hint: None,
                content_type: None,
                origin: None,
            },
        });
        let mut reader = body.into_async_read();
        let mut buf = Vec::new();
        let result = reader.read_to_end(&mut buf).await;
        assert!(result.is_err());
    }
}
