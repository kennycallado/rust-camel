//! Error types for the SurrealDB component.
//!
//! Preserves source errors for diagnosis. Adds retryability classification
//! for NetworkRetryPolicy.

/// Errors produced by the SurrealDB component.
///
/// Note: `thiserror` can only derive one `From<surrealdb::Error>` per type.
/// `Connection` gets `#[from]`; `Query` uses `#[source]` with a manual constructor.
#[derive(Debug, thiserror::Error)]
pub enum SurrealDbError {
    /// Connection or auth failure to SurrealDB server.
    #[error("connection failed: {source}")]
    Connection {
        #[from]
        source: surrealdb::Error,
    },

    /// Query execution failure (non-transient).
    #[error("query failed: {source}")]
    Query {
        #[source]
        source: surrealdb::Error,
    },

    /// Vector input doesn't match expected format.
    #[error("invalid vector: expected array<f32>, got {0}")]
    InvalidVector(String),

    /// Identifier (table/field name) failed validation.
    #[error("invalid identifier: {0} (allowed: ASCII alphanumeric + underscore)")]
    InvalidIdentifier(String),

    /// Named datasource not found in catalog.
    #[error("datasource '{0}' not found in catalog")]
    DatasourceNotFound(String),

    /// Required URI parameter is missing.
    #[error("missing required parameter: {0}")]
    MissingParam(String),

    /// DatasourceHandle downcast to `Surreal<Any>` failed.
    #[error("downcast failed: expected Surreal<Any> for datasource '{0}'")]
    DowncastFailed(String),

    /// LIVE SELECT used with non-WebSocket protocol.
    #[error("live query requires WebSocket protocol (ws/wss), got: {0}")]
    LiveRequiresWebSocket(String),

    /// Invalid CamelSurrealDbParams header value.
    #[error("invalid params header: {0}")]
    InvalidParam(String),

    /// Invalid or unsupported body content.
    #[error("invalid body: {0}")]
    InvalidBody(String),

    /// Operation not supported in this context (e.g. Live on producer).
    #[error("operation not supported: {0}")]
    NotSupported(String),
}

impl SurrealDbError {
    /// Constructor for Query errors (since `#[from]` is only on Connection).
    pub fn query(source: surrealdb::Error) -> Self {
        Self::Query { source }
    }

    /// Classifies whether this error is retryable by `NetworkRetryPolicy`.
    ///
    /// Only `Connection { .. }` is retryable: transient transport/setup
    /// failures (connection refused, DNS hiccup, TLS negotiation drop) that
    /// can resolve on the next attempt. `Query { .. }` errors are never
    /// retried — a query that reached the server and was rejected (syntax,
    /// permission, constraint) will not succeed on resend, and resending a
    /// non-idempotent write after an ambiguous transport failure risks
    /// duplicates (ADR-0013).
    ///
    /// Currently consumed only by `SurrealDbPoolFactory::create` for retrying
    /// transport establishment. Producer paths map SDK errors to
    /// non-retryable `Query` variants (see ADR-0013); when read-side
    /// operations distinguish transport-drop from query-rejected, this
    /// classifier will need a matching variant and the producer can start
    /// consuming this policy.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Connection { .. })
    }
}

impl From<SurrealDbError> for camel_api::CamelError {
    fn from(err: SurrealDbError) -> Self {
        match err {
            SurrealDbError::Connection { source } => {
                let msg = format!("connection failed: {source}");
                camel_api::CamelError::ProcessorErrorWithSource(msg, std::sync::Arc::new(source))
            }
            SurrealDbError::Query { source } => {
                let msg = format!("query failed: {source}");
                camel_api::CamelError::ProcessorErrorWithSource(msg, std::sync::Arc::new(source))
            }
            _ => camel_api::CamelError::ProcessorError(err.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: construct a surrealdb::Error for testing.
    /// surrealdb::Error is a struct with private fields but has pub constructors
    /// like `Error::internal(message)`, `Error::connection(message, details)`, etc.
    fn make_surreal_error(message: &str) -> surrealdb::Error {
        surrealdb::Error::internal(message.to_string())
    }

    #[test]
    fn connection_error_is_retryable() {
        let err = SurrealDbError::Connection {
            source: make_surreal_error("connection refused"),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn query_error_is_not_retryable() {
        let err = SurrealDbError::query(make_surreal_error("syntax error"));
        assert!(!err.is_retryable());
    }

    #[test]
    fn invalid_identifier_not_retryable() {
        let err = SurrealDbError::InvalidIdentifier("droptable;".into());
        assert!(!err.is_retryable());
    }

    #[test]
    fn connection_error_preserves_source() {
        let source = make_surreal_error("connection refused");
        let err = SurrealDbError::Connection { source };
        let src = std::error::Error::source(&err);
        assert!(src.is_some());
    }

    #[test]
    fn query_constructor_creates_query_variant() {
        let err = SurrealDbError::query(make_surreal_error("syntax error"));
        assert!(matches!(err, SurrealDbError::Query { .. }));
        assert!(!err.is_retryable());
    }

    #[test]
    fn datasource_not_found_display() {
        let err = SurrealDbError::DatasourceNotFound("my-ds".into());
        assert!(err.to_string().contains("my-ds"));
    }

    #[test]
    fn missing_param_display() {
        let err = SurrealDbError::MissingParam("table".into());
        assert!(err.to_string().contains("table"));
    }

    #[test]
    fn downcast_failed_display() {
        let err = SurrealDbError::DowncastFailed("my-ds".into());
        assert!(err.to_string().contains("my-ds"));
    }

    #[test]
    fn live_requires_websocket_display() {
        let err = SurrealDbError::LiveRequiresWebSocket("http://localhost".into());
        assert!(err.to_string().contains("http://localhost"));
        assert!(err.to_string().contains("WebSocket"));
    }

    #[test]
    fn invalid_vector_display() {
        let err = SurrealDbError::InvalidVector("string instead of array".into());
        assert!(err.to_string().contains("array<f32>"));
    }

    #[test]
    fn from_connection_preserves_source_in_camel_error() {
        use camel_api::CamelError;
        let err = SurrealDbError::Connection {
            source: surrealdb::Error::internal("connection refused".into()),
        };
        let camel_err: CamelError = err.into();
        match camel_err {
            CamelError::ProcessorErrorWithSource(msg, arc) => {
                assert!(
                    msg.contains("connection failed"),
                    "msg must contain 'connection failed', got: {msg}"
                );
                let _ = arc; // source preserved via Arc<dyn Error>
            }
            other => panic!("expected ProcessorErrorWithSource, got {other:?}"),
        }
    }

    #[test]
    fn from_query_preserves_source_in_camel_error() {
        use camel_api::CamelError;
        let err = SurrealDbError::query(surrealdb::Error::internal("syntax error".into()));
        let camel_err: CamelError = err.into();
        match camel_err {
            CamelError::ProcessorErrorWithSource(msg, arc) => {
                assert!(
                    msg.contains("query failed"),
                    "msg must contain 'query failed', got: {msg}"
                );
                let _ = arc; // source preserved via Arc
            }
            other => panic!("expected ProcessorErrorWithSource, got {other:?}"),
        }
    }

    #[test]
    fn invalid_param_display() {
        let err = SurrealDbError::InvalidParam("expected string, got integer".into());
        assert!(err.to_string().contains("invalid params header"));
    }

    #[test]
    fn invalid_param_not_retryable() {
        let err = SurrealDbError::InvalidParam("bad value".into());
        assert!(!err.is_retryable());
    }

    #[test]
    fn invalid_body_display() {
        let err = SurrealDbError::InvalidBody("expected JSON, got binary".into());
        assert!(err.to_string().contains("invalid body"));
    }

    #[test]
    fn invalid_body_not_retryable() {
        let err = SurrealDbError::InvalidBody("bad body".into());
        assert!(!err.is_retryable());
    }

    #[test]
    fn not_supported_display() {
        let err = SurrealDbError::NotSupported("live on producer".into());
        assert!(err.to_string().contains("operation not supported"));
    }

    #[test]
    fn not_supported_not_retryable() {
        let err = SurrealDbError::NotSupported("not allowed".into());
        assert!(!err.is_retryable());
    }

    #[test]
    fn from_datasource_not_found_uses_plain_processor_error() {
        use camel_api::CamelError;
        let err = SurrealDbError::DatasourceNotFound("my-ds".into());
        let camel_err: CamelError = err.into();
        match camel_err {
            CamelError::ProcessorError(msg) => {
                assert!(msg.contains("my-ds"));
                assert!(msg.contains("datasource"));
            }
            other => panic!("expected ProcessorError for no-source variant, got {other:?}"),
        }
    }
}
