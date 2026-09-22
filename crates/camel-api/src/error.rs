use std::fmt;
use std::sync::Arc;
use thiserror::Error;

/// Typed security-validation error for fail-closed config/startup checks (ADR-0033).
///
/// Each variant corresponds to a specific Batch 1+ security validation that refuses
/// to start with a misconfigured or dangerous default. Operators can `match` on
/// these variants for programmatic error handling.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ConfigValidationError {
    #[error(
        "aggregator config requires at least one completion bound (size, timeout, predicate, or interval)"
    )]
    AggregatorMissingCompletionBound,

    /// Raised when an Aggregator has none of: max_buckets, a Timeout completion
    /// condition, or a bucket_ttl. At least one memory-release bound is mandatory
    /// (R3-M2) so a unique-correlation-key flood cannot grow the bucket map
    /// without limit.
    #[error("aggregator requires at least one of max_buckets, completionTimeout, or bucket_ttl")]
    AggregatorMissingMemoryBound,

    /// Raised when an Aggregator has a Timeout completion condition but no
    /// `bucket_ttl`. The R3-M3 timeout-task cap may skip spawning a dedicated
    /// timeout task under flood; without `bucket_ttl` there is no fallback
    /// eviction path and the bucket leaks until shutdown. Requiring `bucket_ttl`
    /// whenever Timeout is present makes the cap-skip degradation safe by
    /// construction.
    #[error(
        "aggregator Timeout completion requires bucket_ttl (memory-release bound for the timeout-task cap fallback)"
    )]
    AggregatorTimeoutRequiresTtl,

    #[error("throttler max_requests must be > 0")]
    ThrottlerMaxRequestsZero,

    #[error("loop step must specify either 'count' or 'while', not both")]
    LoopConflictingCountAndWhile,

    #[error("loop step must specify either 'count' or 'while'")]
    LoopMissingCountOrWhile,

    #[error("SQL use_message_body_for_sql requires allow_dynamic_query=true")]
    SqlDynamicQueryWithoutAllowDynamic,
}

/// Typed error for constructing an [`EndpointUri`](crate::EndpointUri) from a base URI
/// plus a `parameters:` map.
///
/// Every variant names the offending key or input in its `Display` text so failures
/// are diagnosable without losing the context of what was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum EndpointUriError {
    /// A `parameters:` key collides with a key already present in the base URI query.
    #[error(
        "endpoint URI parameter `{key}` duplicates a key already present in the base URI query"
    )]
    DuplicateKey { key: String },

    /// The base URI has no non-empty scheme (no `:` before the path).
    #[error("endpoint URI is missing a scheme (expected `scheme:path`)")]
    MissingScheme,

    /// The base URI query contains a pair with an empty key (e.g. `?=value`).
    #[error("endpoint URI query contains a pair with an empty key")]
    EmptyQueryKey,

    /// A `parameters:` key is empty or contains a reserved/unsafe character.
    #[error("endpoint URI parameter key `{key}` is empty or contains a reserved character")]
    InvalidParamKey { key: String },
}

/// Opaque handle to an underlying error, preserving its source chain without
/// exposing the concrete type.
///
/// The opacity contract: the pointee is reachable only through
/// [`std::error::Error::source()`] (returned directly — no `Arc` wrapper hop),
/// the inner handle is private, there is no public `Clone`, and provenance
/// cannot be extracted outside camel-api (short of `unsafe`). Crate internals
/// duplicate the handle via `OpaqueErrorSource::clone_handle` when cloning a
/// [`CamelError`].
///
/// # Examples
///
/// The inner handle cannot be destructured out of the wrapper (private field):
///
/// ```compile_fail
/// use camel_api::OpaqueErrorSource;
///
/// #[derive(Debug)]
/// struct MyError;
///
/// impl std::fmt::Display for MyError {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         f.write_str("my error")
///     }
/// }
///
/// impl std::error::Error for MyError {}
///
/// let OpaqueErrorSource(inner) = OpaqueErrorSource::new(std::sync::Arc::new(MyError));
/// ```
///
/// The wrapper is deliberately not `Clone`, so callers cannot copy the handle
/// out of camel-api:
///
/// ```compile_fail
/// use camel_api::OpaqueErrorSource;
///
/// #[derive(Debug)]
/// struct MyError;
///
/// impl std::fmt::Display for MyError {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         f.write_str("my error")
///     }
/// }
///
/// impl std::error::Error for MyError {}
///
/// let s = OpaqueErrorSource::new(std::sync::Arc::new(MyError));
/// let _ = s.clone();
/// ```
#[derive(Debug)]
pub struct OpaqueErrorSource(Arc<dyn std::error::Error + Send + Sync>);

impl OpaqueErrorSource {
    /// Wrap an existing error as an opaque source.
    pub fn new(source: Arc<dyn std::error::Error + Send + Sync>) -> Self {
        Self(source)
    }

    /// Duplicate the inner handle for the manual `Clone` impl on
    /// [`CamelError`]. Crate-private by design: the wrapper itself is not
    /// `Clone`, so external code cannot duplicate provenance out of camel-api.
    fn clone_handle(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl fmt::Display for OpaqueErrorSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for OpaqueErrorSource {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // Pointee directly — NO Arc wrapper hop, so `downcast_ref` on the
        // returned trait object reaches the wrapped error itself.
        Some(self.0.as_ref())
    }
}

/// Core error type for the Camel framework.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CamelError {
    #[error("Component not found: {0}")]
    ComponentNotFound(String),

    #[error("Endpoint creation failed: {0}")]
    EndpointCreationFailed(String),

    /// Like `EndpointCreationFailed` but preserves the source error chain
    /// for downstream inspection (e.g. typed gate-rejection classification).
    #[error("Endpoint creation failed: {0}")]
    EndpointCreationFailedWithSource(String, #[source] OpaqueErrorSource),

    #[error("Processor error: {0}")]
    ProcessorError(String),

    /// Like `ProcessorError` but preserves the source error chain
    /// for downstream inspection (e.g. via `std::error::Error::source()`).
    #[error("Processor error: {0}")]
    ProcessorErrorWithSource(String, #[source] Arc<dyn std::error::Error + Send + Sync>),

    #[error("Type conversion failed: {0}")]
    TypeConversionFailed(String),

    #[error("Invalid URI: {0}")]
    InvalidUri(String),

    #[error("Channel closed")]
    ChannelClosed,

    #[error("Route error: {0}")]
    RouteError(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Dead letter channel failed: {0}")]
    DeadLetterChannelFailed(String),

    #[error("Circuit breaker open: {0}")]
    CircuitOpen(String),

    #[error("HTTP {method} {url} failed: {status_code} {status_text}")]
    HttpOperationFailed {
        method: String,
        url: String,
        status_code: u16,
        status_text: String,
        response_body: Option<String>,
    },

    /// Producer `call()` failed on a shutdown signal — the consumer/semaphore
    /// is closing and the producer cannot acquire a permit. Distinct from Stop EIP
    /// (which is successful control flow). Used by JMS/OpenSearch producers. See ADR-0024.
    #[error("Consumer stopping: semaphore closed during call")]
    ConsumerStopping,

    #[error("Configuration error: {0}")]
    Config(String),

    /// Typed security-validation error (ADR-0033). Promotes Batch 1+ config/startup
    /// failure modes from stringly-typed `Config(_)` to a matchable enum so
    /// operators can discriminate programmatically.
    #[error("Configuration validation error: {0}")]
    ConfigValidation(ConfigValidationError),

    #[error("Body stream has already been consumed")]
    AlreadyConsumed,

    #[error("Stream size exceeded limit: {0}")]
    StreamLimitExceeded(usize),

    #[error("Unauthenticated: {0}")]
    Unauthenticated(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    /// Auth provider (JWKS/introspection/token endpoint) is unreachable or failing.
    /// Promotes the auth-provider-down signal from a stringly-typed ProcessorError to
    /// a matchable variant; WebSocket and gRPC transports map it to 503 / UNAVAILABLE.
    #[error("Auth provider unavailable: {0}")]
    AuthProviderUnavailable(String),

    #[error("Validation failed: {0}")]
    ValidationError(String),

    #[error("Template reload failed: {0}")]
    TemplateReload(String),

    /// Typed endpoint-URI construction error (see [`EndpointUriError`]). Promotes the
    /// fail-closed `EndpointUri` merge failures from stringly-typed errors to a matchable
    /// variant so operators can discriminate programmatically.
    #[error("Endpoint URI error: {0}")]
    EndpointUri(EndpointUriError),

    /// The request body media type does not match the declared/consumed type
    /// (REST DSL default-strict content negotiation, HTTP 415).
    #[error("Unsupported media type: consumed {consumed}, declared {declared}")]
    UnsupportedMediaType { consumed: String, declared: String },

    /// The response representation cannot satisfy the client's Accept header
    /// (REST DSL default-strict content negotiation, HTTP 406).
    #[error("Not acceptable: accept {accept}, produced {produced}")]
    NotAcceptable { accept: String, produced: String },
}

/// Manual `Clone` impl: every arm clones its fields normally, except
/// `EndpointCreationFailedWithSource`, which duplicates the opaque source
/// handle via the crate-private `OpaqueErrorSource::clone_handle` (the wrapper
/// itself is deliberately not `Clone`). Exhaustive like `variant_name()` — a
/// new variant without an arm fails compilation.
impl Clone for CamelError {
    fn clone(&self) -> Self {
        match self {
            Self::ComponentNotFound(msg) => Self::ComponentNotFound(msg.clone()),
            Self::EndpointCreationFailed(msg) => Self::EndpointCreationFailed(msg.clone()),
            Self::EndpointCreationFailedWithSource(msg, source) => {
                Self::EndpointCreationFailedWithSource(msg.clone(), source.clone_handle())
            }
            Self::ProcessorError(msg) => Self::ProcessorError(msg.clone()),
            Self::ProcessorErrorWithSource(msg, source) => {
                Self::ProcessorErrorWithSource(msg.clone(), Arc::clone(source))
            }
            Self::TypeConversionFailed(msg) => Self::TypeConversionFailed(msg.clone()),
            Self::InvalidUri(msg) => Self::InvalidUri(msg.clone()),
            Self::ChannelClosed => Self::ChannelClosed,
            Self::RouteError(msg) => Self::RouteError(msg.clone()),
            Self::Io(msg) => Self::Io(msg.clone()),
            Self::DeadLetterChannelFailed(msg) => Self::DeadLetterChannelFailed(msg.clone()),
            Self::CircuitOpen(msg) => Self::CircuitOpen(msg.clone()),
            Self::HttpOperationFailed {
                method,
                url,
                status_code,
                status_text,
                response_body,
            } => Self::HttpOperationFailed {
                method: method.clone(),
                url: url.clone(),
                status_code: *status_code,
                status_text: status_text.clone(),
                response_body: response_body.clone(),
            },
            Self::ConsumerStopping => Self::ConsumerStopping,
            Self::Config(msg) => Self::Config(msg.clone()),
            Self::ConfigValidation(e) => Self::ConfigValidation(e.clone()),
            Self::AlreadyConsumed => Self::AlreadyConsumed,
            Self::StreamLimitExceeded(limit) => Self::StreamLimitExceeded(*limit),
            Self::Unauthenticated(msg) => Self::Unauthenticated(msg.clone()),
            Self::Unauthorized(msg) => Self::Unauthorized(msg.clone()),
            Self::AuthProviderUnavailable(msg) => Self::AuthProviderUnavailable(msg.clone()),
            Self::ValidationError(msg) => Self::ValidationError(msg.clone()),
            Self::TemplateReload(msg) => Self::TemplateReload(msg.clone()),
            Self::EndpointUri(e) => Self::EndpointUri(e.clone()),
            Self::UnsupportedMediaType { consumed, declared } => Self::UnsupportedMediaType {
                consumed: consumed.clone(),
                declared: declared.clone(),
            },
            Self::NotAcceptable { accept, produced } => Self::NotAcceptable {
                accept: accept.clone(),
                produced: produced.clone(),
            },
        }
    }
}

/// Classification marker for `CamelError::CircuitOpen`.
///
/// Shared named constant so the pipeline tracer's circuit-open exclusion
/// (skip `increment_errors` — the breaker already recorded the rejection)
/// and `classify` itself cannot drift apart (dashboard-observability D2).
pub const CIRCUIT_OPEN: &str = "circuit_open";

impl CamelError {
    pub fn classify(&self) -> &'static str {
        #[allow(unreachable_patterns)]
        match self {
            Self::ComponentNotFound(_) => "component",
            Self::EndpointCreationFailed(_)
            | Self::EndpointCreationFailedWithSource(_, _)
            | Self::InvalidUri(_)
            | Self::EndpointUri(_) => "endpoint",
            Self::ProcessorError(_)
            | Self::ProcessorErrorWithSource(_, _)
            | Self::AuthProviderUnavailable(_) => "processor",
            Self::TypeConversionFailed(_) | Self::AlreadyConsumed => "type_conversion",
            Self::Io(_) => "io",
            Self::RouteError(_) => "route",
            Self::CircuitOpen(_) => CIRCUIT_OPEN,
            Self::HttpOperationFailed { .. } => "http",
            Self::Config(_) | Self::ConfigValidation(_) => "config",
            Self::DeadLetterChannelFailed(_) => "dead_letter",
            Self::ConsumerStopping => "consumer_stop",
            Self::StreamLimitExceeded(_) => "stream",
            Self::ChannelClosed => "channel",
            Self::Unauthenticated(_) => "unauthenticated",
            Self::Unauthorized(_) => "unauthorized",
            Self::ValidationError(_) => "validation",
            Self::TemplateReload(_) => "template",
            Self::UnsupportedMediaType { .. } => "unsupported_media_type",
            Self::NotAcceptable { .. } => "not_acceptable",
            _ => "unknown",
        }
    }

    /// Stable variant name used by `doTry` catch-by-variant matchers.
    ///
    /// `ProcessorErrorWithSource` and `AuthProviderUnavailable` alias to
    /// `"ProcessorError"`, and `EndpointCreationFailedWithSource` aliases to
    /// `"EndpointCreationFailed"` — the aliased variants are not distinguishable
    /// by name in MVP (see spec §5.4), so existing `doTry` catch handlers keep
    /// matching.
    ///
    /// The enum is `#[non_exhaustive]`; this match lives in the defining crate (camel-api),
    /// so internal exhaustive matching is allowed. Adding a new variant without updating
    /// this method will fail to compile, surfaced by `variant_name_tests`.
    pub fn variant_name(&self) -> &'static str {
        match self {
            Self::ComponentNotFound(_) => "ComponentNotFound",
            Self::EndpointCreationFailed(_) => "EndpointCreationFailed",
            Self::EndpointCreationFailedWithSource(_, _) => "EndpointCreationFailed",
            Self::ProcessorError(_) => "ProcessorError",
            Self::ProcessorErrorWithSource(_, _) => "ProcessorError",
            Self::AuthProviderUnavailable(_) => "ProcessorError",
            Self::TypeConversionFailed(_) => "TypeConversionFailed",
            Self::InvalidUri(_) => "InvalidUri",
            Self::ChannelClosed => "ChannelClosed",
            Self::RouteError(_) => "RouteError",
            Self::Io(_) => "Io",
            Self::DeadLetterChannelFailed(_) => "DeadLetterChannelFailed",
            Self::CircuitOpen(_) => "CircuitOpen",
            Self::HttpOperationFailed { .. } => "HttpOperationFailed",
            Self::ConsumerStopping => "ConsumerStopping",
            Self::Config(_) => "Config",
            Self::ConfigValidation(_) => "ConfigValidation",
            Self::AlreadyConsumed => "AlreadyConsumed",
            Self::StreamLimitExceeded(_) => "StreamLimitExceeded",
            Self::Unauthenticated(_) => "Unauthenticated",
            Self::Unauthorized(_) => "Unauthorized",
            Self::ValidationError(_) => "ValidationError",
            Self::TemplateReload(_) => "TemplateReload",
            Self::EndpointUri(_) => "EndpointUri",
            Self::UnsupportedMediaType { .. } => "UnsupportedMediaType",
            Self::NotAcceptable { .. } => "NotAcceptable",
        }
    }
}

impl From<std::io::Error> for CamelError {
    fn from(err: std::io::Error) -> Self {
        CamelError::Io(err.to_string())
    }
}

impl From<crate::template::TemplateError> for CamelError {
    fn from(err: crate::template::TemplateError) -> Self {
        CamelError::Config(err.to_string())
    }
}

impl From<ConfigValidationError> for CamelError {
    fn from(e: ConfigValidationError) -> Self {
        CamelError::ConfigValidation(e)
    }
}

impl From<EndpointUriError> for CamelError {
    fn from(e: EndpointUriError) -> Self {
        CamelError::EndpointUri(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `super::*` brings in thiserror's `Error` derive macro; import the trait
    // anonymously so `source()` is callable in tests.
    use std::error::Error as _;

    /// Minimal source error for opaque-provenance tests.
    #[derive(Debug)]
    struct SampleSource;

    impl fmt::Display for SampleSource {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("sample source")
        }
    }

    impl std::error::Error for SampleSource {}

    fn all_error_samples() -> Vec<CamelError> {
        vec![
            CamelError::ComponentNotFound("x".to_string()),
            CamelError::EndpointCreationFailed("x".to_string()),
            CamelError::EndpointCreationFailedWithSource(
                "x".to_string(),
                OpaqueErrorSource::new(Arc::new(SampleSource)),
            ),
            CamelError::ProcessorError("x".to_string()),
            CamelError::ProcessorErrorWithSource(
                "x".to_string(),
                Arc::new(std::io::Error::other("inner")),
            ),
            CamelError::TypeConversionFailed("x".to_string()),
            CamelError::InvalidUri("x".to_string()),
            CamelError::ChannelClosed,
            CamelError::RouteError("x".to_string()),
            CamelError::Io("x".to_string()),
            CamelError::DeadLetterChannelFailed("x".to_string()),
            CamelError::CircuitOpen("x".to_string()),
            CamelError::HttpOperationFailed {
                method: "GET".to_string(),
                url: "https://example.com".to_string(),
                status_code: 500,
                status_text: "Internal Server Error".to_string(),
                response_body: Some("error".to_string()),
            },
            CamelError::ConsumerStopping,
            CamelError::Config("x".to_string()),
            CamelError::ConfigValidation(ConfigValidationError::ThrottlerMaxRequestsZero),
            CamelError::AlreadyConsumed,
            CamelError::StreamLimitExceeded(42),
            CamelError::Unauthenticated("token expired".to_string()),
            CamelError::Unauthorized("missing admin role".to_string()),
            CamelError::AuthProviderUnavailable("jwks down".to_string()),
            CamelError::ValidationError("body does not match schema".to_string()),
            CamelError::TemplateReload("reload failed".to_string()),
            CamelError::EndpointUri(EndpointUriError::MissingScheme),
            CamelError::UnsupportedMediaType {
                consumed: "text/plain".to_string(),
                declared: "application/json".to_string(),
            },
            CamelError::NotAcceptable {
                accept: "application/xml".to_string(),
                produced: "application/json".to_string(),
            },
        ]
    }

    #[test]
    fn test_http_operation_failed_display() {
        let err = CamelError::HttpOperationFailed {
            method: "GET".to_string(),
            url: "https://example.com/test".to_string(),
            status_code: 404,
            status_text: "Not Found".to_string(),
            response_body: Some("page not found".to_string()),
        };
        let msg = format!("{err}");
        assert!(msg.contains("404"));
        assert!(msg.contains("Not Found"));
    }

    #[test]
    fn test_http_operation_failed_clone() {
        let err = CamelError::HttpOperationFailed {
            method: "POST".to_string(),
            url: "https://api.example.com/users".to_string(),
            status_code: 500,
            status_text: "Internal Server Error".to_string(),
            response_body: None,
        };
        let cloned = err.clone();
        assert!(matches!(
            cloned,
            CamelError::HttpOperationFailed {
                status_code: 500,
                ..
            }
        ));
    }

    #[test]
    fn test_classify_maps_all_variants() {
        assert_eq!(
            CamelError::ComponentNotFound("x".to_string()).classify(),
            "component"
        );
        assert_eq!(
            CamelError::EndpointCreationFailed("x".to_string()).classify(),
            "endpoint"
        );
        assert_eq!(
            CamelError::ProcessorError("x".to_string()).classify(),
            "processor"
        );
        assert_eq!(
            CamelError::TypeConversionFailed("x".to_string()).classify(),
            "type_conversion"
        );
        assert_eq!(
            CamelError::InvalidUri("x".to_string()).classify(),
            "endpoint"
        );
        assert_eq!(CamelError::ChannelClosed.classify(), "channel");
        assert_eq!(CamelError::RouteError("x".to_string()).classify(), "route");
        assert_eq!(CamelError::Io("x".to_string()).classify(), "io");
        assert_eq!(
            CamelError::DeadLetterChannelFailed("x".to_string()).classify(),
            "dead_letter"
        );
        assert_eq!(
            CamelError::CircuitOpen("x".to_string()).classify(),
            "circuit_open"
        );
        assert_eq!(
            CamelError::HttpOperationFailed {
                method: "GET".to_string(),
                url: "https://example.com".to_string(),
                status_code: 500,
                status_text: "Internal Server Error".to_string(),
                response_body: None,
            }
            .classify(),
            "http"
        );
        assert_eq!(CamelError::Config("x".to_string()).classify(), "config");
        assert_eq!(
            CamelError::ConfigValidation(ConfigValidationError::ThrottlerMaxRequestsZero)
                .classify(),
            "config"
        );
        assert_eq!(CamelError::AlreadyConsumed.classify(), "type_conversion");
        assert_eq!(CamelError::StreamLimitExceeded(42).classify(), "stream");
        assert_eq!(
            CamelError::ValidationError("bad".to_string()).classify(),
            "validation"
        );
    }

    #[test]
    fn test_classify_output_is_ascii_and_short() {
        for error in all_error_samples() {
            let class = error.classify();
            assert!(class.is_ascii());
            // "unsupported_media_type" (REST negotiation, L2) sets the floor at 22
            assert!(class.len() <= 22, "class too long: {class}");
        }
    }

    #[test]
    fn test_auth_variants_classify() {
        assert_eq!(
            CamelError::Unauthenticated("x".to_string()).classify(),
            "unauthenticated"
        );
        assert_eq!(
            CamelError::Unauthorized("x".to_string()).classify(),
            "unauthorized"
        );
    }

    #[test]
    fn test_validation_error_classify() {
        assert_eq!(
            CamelError::ValidationError("bad".to_string()).classify(),
            "validation"
        );
    }

    #[test]
    fn template_reload_classifies_as_template() {
        let err = CamelError::TemplateReload("boom".into());
        assert_eq!(err.classify(), "template");
    }

    #[test]
    fn template_reload_variant_name() {
        let err = CamelError::TemplateReload("boom".into());
        assert_eq!(err.variant_name(), "TemplateReload");
    }

    #[test]
    fn test_auth_variants_are_clone() {
        let err = CamelError::Unauthenticated("test".to_string());
        let cloned = err.clone();
        assert!(matches!(cloned, CamelError::Unauthenticated(_)));

        let err2 = CamelError::Unauthorized("test".to_string());
        let cloned2 = err2.clone();
        assert!(matches!(cloned2, CamelError::Unauthorized(_)));
    }

    #[test]
    fn classification_unchanged_for_callers() {
        // Pins the contract the pipeline-tracer circuit_open exclusion
        // relies on (dashboard-observability D2): CircuitOpen must keep
        // classifying as "circuit_open" — callers and the tracer skip
        // branch match on exactly this literal.
        assert_eq!(
            CamelError::CircuitOpen("breaker open".into()).classify(),
            "circuit_open"
        );
    }

    #[test]
    fn auth_provider_unavailable_display_carries_detail() {
        let err = CamelError::AuthProviderUnavailable("conn refused".into());
        let msg = err.to_string();
        assert!(msg.contains("conn refused"));
        assert!(
            msg.starts_with("Auth provider unavailable"),
            "display should start with 'Auth provider unavailable', got: {msg}"
        );
    }

    #[test]
    fn classify_negotiation_errors() {
        let unsupported = CamelError::UnsupportedMediaType {
            consumed: "text/plain".into(),
            declared: "application/json".into(),
        };
        let not_acceptable = CamelError::NotAcceptable {
            accept: "application/xml".into(),
            produced: "application/json".into(),
        };
        assert_eq!(unsupported.classify(), "unsupported_media_type");
        assert_eq!(not_acceptable.classify(), "not_acceptable");
    }

    #[test]
    fn variant_names_negotiation_errors() {
        let unsupported = CamelError::UnsupportedMediaType {
            consumed: "text/plain".into(),
            declared: "application/json".into(),
        };
        let not_acceptable = CamelError::NotAcceptable {
            accept: "application/xml".into(),
            produced: "application/json".into(),
        };
        assert_eq!(unsupported.variant_name(), "UnsupportedMediaType");
        assert_eq!(not_acceptable.variant_name(), "NotAcceptable");
    }

    #[test]
    fn display_negotiation_errors() {
        let unsupported = CamelError::UnsupportedMediaType {
            consumed: "text/plain".into(),
            declared: "application/json".into(),
        };
        let not_acceptable = CamelError::NotAcceptable {
            accept: "application/xml".into(),
            produced: "application/json".into(),
        };
        let unsupported_msg = unsupported.to_string();
        assert!(unsupported_msg.contains("text/plain"));
        assert!(unsupported_msg.contains("application/json"));
        let not_acceptable_msg = not_acceptable.to_string();
        assert!(not_acceptable_msg.contains("application/xml"));
        assert!(not_acceptable_msg.contains("application/json"));
    }

    #[test]
    fn opaque_error_source_exposes_only_pointee() {
        let src = OpaqueErrorSource::new(Arc::new(SampleSource));
        let pointee = src.source().unwrap();
        assert!(pointee.downcast_ref::<SampleSource>().is_some());
    }

    #[test]
    fn endpoint_creation_failed_with_source_aliases_to_plain() {
        let e = CamelError::EndpointCreationFailedWithSource(
            "d".to_string(),
            OpaqueErrorSource::new(Arc::new(SampleSource)),
        );
        assert_eq!(e.variant_name(), "EndpointCreationFailed");
        assert_eq!(e.classify(), "endpoint");
        assert_eq!(e.to_string(), "Endpoint creation failed: d");
    }

    #[test]
    fn clone_preserves_variant_identity_for_all_error_samples() {
        for e in all_error_samples() {
            let c = e.clone();
            assert_eq!(c.variant_name(), e.variant_name());
            assert_eq!(c.classify(), e.classify());
            assert_eq!(c.to_string(), e.to_string());
        }
    }

    #[test]
    fn camel_error_clone_preserves_source_provenance() {
        let e = CamelError::EndpointCreationFailedWithSource(
            "d".to_string(),
            OpaqueErrorSource::new(Arc::new(SampleSource)),
        );
        let c = e.clone();
        // `CamelError::source()` (thiserror #[source]) yields the wrapper
        // itself; the pointee is one more `source()` hop away — that hop is
        // the pointee-only mechanism under test (no Arc wrapper in between).
        let wrapper = c.source().unwrap();
        let pointee = wrapper.source().unwrap();
        assert!(pointee.downcast_ref::<SampleSource>().is_some());
    }
}

#[cfg(test)]
mod variant_name_tests {
    use super::{CamelError, ConfigValidationError, EndpointUriError, OpaqueErrorSource};
    use std::sync::Arc;

    /// Representative value for each enum variant. This test fails to compile
    /// when a new variant is added to CamelError without updating variant_name().
    /// The enum is `#[non_exhaustive]` but this match lives in the same crate, so internal
    /// exhaustive matching is allowed.
    ///
    /// The table must list every variant exactly once (`cases.len()` is asserted
    /// below). When adding a CamelError variant, also update
    /// `test_exception_kind_vocabulary_classification_guard` in
    /// crates/camel-dsl/src/compile.rs and make the register-or-document
    /// decision (bd rc-5u8co).
    #[test]
    fn variant_name_covers_all_variants() {
        let cases: Vec<(CamelError, &str)> = vec![
            (
                CamelError::ComponentNotFound("x".into()),
                "ComponentNotFound",
            ),
            (
                CamelError::EndpointCreationFailed("x".into()),
                "EndpointCreationFailed",
            ),
            (
                CamelError::EndpointCreationFailedWithSource(
                    "x".into(),
                    OpaqueErrorSource::new(Arc::new(std::io::Error::other("y"))),
                ),
                "EndpointCreationFailed", // aliased
            ),
            (CamelError::ProcessorError("x".into()), "ProcessorError"),
            (
                CamelError::ProcessorErrorWithSource(
                    "x".into(),
                    Arc::new(std::io::Error::other("y")),
                ),
                "ProcessorError", // aliased
            ),
            (
                CamelError::TypeConversionFailed("x".into()),
                "TypeConversionFailed",
            ),
            (CamelError::InvalidUri("x".into()), "InvalidUri"),
            (CamelError::ChannelClosed, "ChannelClosed"),
            (CamelError::RouteError("x".into()), "RouteError"),
            (CamelError::Io("x".into()), "Io"),
            (
                CamelError::DeadLetterChannelFailed("x".into()),
                "DeadLetterChannelFailed",
            ),
            (CamelError::CircuitOpen("x".into()), "CircuitOpen"),
            (
                CamelError::HttpOperationFailed {
                    method: "GET".into(),
                    url: "https://example.com".into(),
                    status_code: 500,
                    status_text: "Internal Server Error".into(),
                    response_body: None,
                },
                "HttpOperationFailed",
            ),
            (CamelError::ConsumerStopping, "ConsumerStopping"),
            (CamelError::Config("x".into()), "Config"),
            (
                CamelError::ConfigValidation(ConfigValidationError::ThrottlerMaxRequestsZero),
                "ConfigValidation",
            ),
            (CamelError::AlreadyConsumed, "AlreadyConsumed"),
            (CamelError::StreamLimitExceeded(42), "StreamLimitExceeded"),
            (CamelError::Unauthenticated("x".into()), "Unauthenticated"),
            (CamelError::Unauthorized("x".into()), "Unauthorized"),
            (CamelError::ValidationError("bad".into()), "ValidationError"),
            (CamelError::TemplateReload("x".into()), "TemplateReload"),
            (
                CamelError::EndpointUri(EndpointUriError::MissingScheme),
                "EndpointUri",
            ),
            (
                CamelError::UnsupportedMediaType {
                    consumed: "text/plain".into(),
                    declared: "application/json".into(),
                },
                "UnsupportedMediaType",
            ),
            (
                CamelError::NotAcceptable {
                    accept: "application/xml".into(),
                    produced: "application/json".into(),
                },
                "NotAcceptable",
            ),
            (
                CamelError::AuthProviderUnavailable("x".into()),
                "ProcessorError",
            ),
        ];

        assert_eq!(
            cases.len(),
            26,
            "variant_name_covers_all_variants must cover every CamelError variant; \
             extend this table and the camel-dsl classification guard"
        );

        for (err, expected) in cases {
            assert_eq!(
                err.variant_name(),
                expected,
                "variant_name mismatch for {:?}",
                err
            );
        }
    }

    #[test]
    fn auth_provider_unavailable_classifies_as_processor() {
        let err = CamelError::AuthProviderUnavailable("jwks down".into());
        assert_eq!(err.classify(), "processor");
    }

    #[test]
    fn auth_provider_unavailable_variant_name_aliases_processor_error() {
        let err = CamelError::AuthProviderUnavailable("jwks down".into());
        assert_eq!(err.variant_name(), "ProcessorError");
    }
}
