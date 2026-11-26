use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use opentelemetry::Context;
use tokio_util::sync;
use uuid::Uuid;

use crate::error::CamelError;
use crate::from_body::FromBody;
use crate::message::Message;
use crate::value::Value;

/// Extension key used by use_original_message: stashes the pre-route Message
/// so the error handler can restore body+headers before DLC dispatch.
pub const ORIGINAL_MESSAGE_EXTENSION: &str = "CamelOriginalMessage";

/// Extension key carrying the per-dispatch cancellation marker.
///
/// `ConsumerContext::send_and_wait` stamps an `Arc<CancellationToken>` under
/// this key when it enqueues an exchange. Each pipeline entry takes (removes)
/// the marker; cancelling the token stops the pipeline before its next step
/// (ADR-0043). The dispatch token is independent of the consumer-lifetime and
/// route-stop tokens.
///
/// This is enqueue-only, non-serializable exchange metadata. A reply returned
/// from `send_and_wait` carries no marker because the pipeline took it at
/// entry.
pub const DISPATCH_CANCEL_EXTENSION: &str = "CamelDispatchCancel";

/// Extension key for the Content-Type provenance snapshot taken with
/// `ORIGINAL_MESSAGE_EXTENSION` at route entry when `use_original_message` is
/// enabled. Capture replaces the snapshot unconditionally; restore reads it to
/// rebuild the inherited marker.
pub const ORIGINAL_CONTENT_TYPE_INHERITED_EXTENSION: &str = "CamelOriginalContentTypeInherited";

/// Marker-presence snapshot stored under `ORIGINAL_CONTENT_TYPE_INHERITED_EXTENSION`.
/// Stored as `Arc<OriginalContentTypeInherited>` via `Exchange::set_extension`.
pub struct OriginalContentTypeInherited(pub bool);

/// Exchange property marking that the current `Content-Type` was inherited from
/// the incoming request rather than written by a route step.
///
/// Set by the HTTP consumer at ingress and cleared by any route step that
/// writes or removes `Content-Type`, so downstream components can suppress
/// value-equality checks in favour of provenance.
pub const PROPERTY_HTTP_CONTENT_TYPE_INHERITED: &str = "CamelHttpContentTypeInherited";

/// Pipeline-executor-recognized stop signal. Set by processors (ThrottleStrategy::Drop,
/// SamplingService) that cannot return PipelineOutcome::Stopped directly because they
/// are Tower `Service<Exchange>` (Process mode), not OutcomePipeline (Segment).
/// The executor checks this after each step completion. See ADR-0024 amendment.
pub const CAMEL_STOP: &str = "CamelStop";

/// Check whether the exchange carries the CamelStop signal.
///
/// Returns `true` if the `CamelStop` property is set to a boolean `true` value.
/// Used by the pipeline executor (`run_steps`) and segment adapters to detect
/// Process-mode processors that cannot return `PipelineOutcome::Stopped` directly.
pub fn is_camel_stop(exchange: &Exchange) -> bool {
    exchange
        .property(CAMEL_STOP)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Handle owning the per-dispatch cancellation signal stamped by
/// `ConsumerContext::send_and_wait`.
///
/// The handle holds a [`sync::DropGuard`] on the dispatch token: dropping it
/// cancels the token, which stops the pipeline before its next step (ADR-0043).
/// [`DispatchCancellation::disarm`] defuses the guard for a completed dispatch,
/// and [`DispatchCancellation::drain_on`] makes a shutdown-driven drop (the
/// consumer-lifetime token already cancelled) leave the dispatch running so the
/// ADR-0043 drain grace governs the exchange.
///
/// The stamped marker is non-serializable exchange metadata; a reply from
/// `send_and_wait` carries none because the pipeline takes it at entry.
#[must_use = "dropping the handle cancels the stamped dispatch token; disarm it on completion"]
pub struct DispatchCancellation {
    token: sync::CancellationToken,
    guard: Option<sync::DropGuard>,
    drain_owner: Option<sync::CancellationToken>,
}

impl DispatchCancellation {
    /// Borrow the dispatch token this handle owns.
    pub fn token(&self) -> &sync::CancellationToken {
        &self.token
    }

    /// Mark the consumer-lifetime token that owns this dispatch.
    ///
    /// When the owner is already cancelled at drop time, the drop is
    /// shutdown-driven and must NOT cancel the dispatch (ADR-0043 drain grace);
    /// the route stop owns the exchange instead.
    pub fn drain_on(mut self, owner: sync::CancellationToken) -> Self {
        self.drain_owner = Some(owner);
        self
    }

    /// Defuse the guard without cancelling the token.
    ///
    /// Used when the dispatch completed: `DropGuard::disarm` returns the live
    /// token, and dropping that token does not cancel.
    pub fn disarm(mut self) {
        if let Some(guard) = self.guard.take() {
            drop(guard.disarm());
        }
    }
}

impl Drop for DispatchCancellation {
    fn drop(&mut self) {
        let Some(guard) = self.guard.take() else {
            return;
        };
        if self
            .drain_owner
            .as_ref()
            .is_some_and(sync::CancellationToken::is_cancelled)
        {
            // Shutdown owns the exchange: defuse the guard.
            drop(guard.disarm());
        }
        // Otherwise dropping the guard cancels the dispatch token.
    }
}

/// Property key for the exception message (error Display string).
pub const PROPERTY_EXCEPTION_MESSAGE: &str = "CamelExceptionMessage";
/// Property key for the exception kind (error variant name via classify()).
pub const PROPERTY_EXCEPTION_KIND: &str = "CamelExceptionKind";
/// Property key for the caught exception (Java Camel parity alias).
pub const PROPERTY_EXCEPTION_CAUGHT: &str = "CamelExceptionCaught";
/// Property key set when an exception has been handled (Java Camel parity).
pub const PROPERTY_EXCEPTION_HANDLED: &str = "CamelExceptionHandled";

/// The exchange pattern (fire-and-forget or request-reply).
/// exhaustive-by-contract: the InOnly|InOut MEP dichotomy is a fixed, spec-level closed set.
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
    /// RAII claim on the context-global in-flight gauge — the
    /// accepted-not-completed counter (drainclaim, claimfamily). Pipeline
    /// drain sites split a sibling
    /// of the envelope's claim onto the exchange so residency inside
    /// pipeline-embedded stash sites (resequencer buffers, aggregator
    /// buckets — raw-`Exchange` territory with no envelope) stays
    /// counted. `None` on every constructor, on clones, and on
    /// exchanges accepted through uncounted paths.
    pub in_flight_claim: Option<crate::in_flight::InFlightClaim>,
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
            in_flight_claim: None,
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
            in_flight_claim: None,
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

    /// Remove an exchange property. Idempotent: removing an absent key is a no-op.
    /// Used by provenance writers to clear `PROPERTY_HTTP_CONTENT_TYPE_INHERITED`.
    pub fn remove_property(&mut self, key: &str) {
        self.properties.remove(key);
    }

    /// Check if the exchange has an error.
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }

    /// Set an error on this exchange.
    ///
    /// Automatically populates exchange properties with error context so that
    /// all languages can access error information via their property mechanisms:
    /// - `CamelExceptionMessage` — the error's Display string
    /// - `CamelExceptionKind` — the error variant name (via `CamelError::classify()`)
    /// - `CamelExceptionCaught` — alias for Java Camel parity
    ///
    /// For `HttpOperationFailed` it also sets the `CamelHttpResponseCode` and
    /// `CamelHttpResponseText` headers, because a failing producer loses its
    /// exchange with the `Err` and error-handling expressions would otherwise
    /// have no way to read the status.
    pub fn set_error(&mut self, error: CamelError) {
        if let CamelError::HttpOperationFailed {
            status_code,
            status_text,
            ..
        } = &error
        {
            self.input.set_header(
                "CamelHttpResponseCode",
                Value::Number((*status_code).into()),
            );
            self.input
                .set_header("CamelHttpResponseText", Value::String(status_text.clone()));
        }
        let msg = error.to_string();
        let kind = error.classify().to_string();
        self.properties.insert(
            PROPERTY_EXCEPTION_MESSAGE.to_string(),
            Value::String(msg.clone()),
        );
        self.properties
            .insert(PROPERTY_EXCEPTION_KIND.to_string(), Value::String(kind));
        self.properties
            .insert(PROPERTY_EXCEPTION_CAUGHT.to_string(), Value::String(msg));
        self.error = Some(error);
    }

    /// Clear the error and remove all exception properties.
    ///
    /// Called after `handled:true` in on_exception steps to prevent stale
    /// error state from leaking into subsequent processing.
    pub fn clear_error(&mut self) {
        self.error = None;
        self.properties.remove(PROPERTY_EXCEPTION_MESSAGE);
        self.properties.remove(PROPERTY_EXCEPTION_KIND);
        self.properties.remove(PROPERTY_EXCEPTION_CAUGHT);
    }

    /// Mark the exception as handled and clear the error.
    ///
    /// Sets `CamelExceptionHandled = true` then calls `clear_error()`.
    /// This matches Java Camel's `Exchange.EXCEPTION_HANDLED` semantics.
    pub fn handle_error(&mut self) {
        self.properties
            .insert(PROPERTY_EXCEPTION_HANDLED.to_string(), Value::Bool(true));
        self.clear_error();
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

    /// Deserialize the body into type `T`.
    ///
    /// Uses built-in conversions for `String`, `Vec<u8>`, [`bytes::Bytes`], and
    /// `serde_json::Value`. For custom types, implement [`FromBody`] or use
    /// the `impl_from_body_via_serde!` macro.
    ///
    /// # Example
    /// ```rust,ignore
    /// let text: String = exchange.body_as::<String>()?;
    /// let raw: Vec<u8> = exchange.body_as::<Vec<u8>>()?;
    /// ```
    pub fn body_as<T: FromBody>(&self) -> Result<T, CamelError> {
        T::from_body(&self.input.body)
    }

    /// Clone for a fanout branch copy. Unlike [`Exchange::clone`], a
    /// carried in-flight claim is split onto the copy (claimfamily):
    /// the sibling counts the copy independently and releases when the
    /// copy completes; the original claim stays live.
    pub fn branch_clone(&self) -> Self {
        let mut copy = self.clone();
        copy.in_flight_claim = self.in_flight_claim.as_ref().map(|claim| claim.split());
        copy
    }

    /// Begin a per-dispatch cancellation scope and stamp its marker.
    ///
    /// Mints a fresh [`sync::CancellationToken`], stores a clone under
    /// [`DISPATCH_CANCEL_EXTENSION`], and returns a [`DispatchCancellation`]
    /// whose drop cancels the token unless disarmed or defused by a
    /// shutdown-driven drop. Cancelling the token stops the pipeline before its
    /// next step (ADR-0043); the token is independent of the consumer-lifetime
    /// and route-stop tokens.
    #[must_use = "dropping the handle cancels the stamped dispatch token; disarm it on completion"]
    pub fn begin_dispatch_cancel(&mut self) -> DispatchCancellation {
        let token = sync::CancellationToken::new();
        self.set_extension(DISPATCH_CANCEL_EXTENSION, Arc::new(token.clone()));
        DispatchCancellation {
            guard: Some(token.clone().drop_guard()),
            token,
            drain_owner: None,
        }
    }

    /// Take the dispatch-cancellation marker, removing it from the exchange.
    ///
    /// Called at pipeline entry: an exchange that entered the pipeline no
    /// longer carries the marker, so a reply from `send_and_wait` returns none.
    pub fn take_dispatch_cancel(&mut self) -> Option<sync::CancellationToken> {
        self.extensions
            .remove(DISPATCH_CANCEL_EXTENSION)
            .and_then(|value| value.downcast_ref::<sync::CancellationToken>().cloned())
    }

    /// Peek at the dispatch-cancellation marker without removing it.
    ///
    /// Read-only accessor intended for tests and diagnostics.
    pub fn dispatch_cancel(&self) -> Option<sync::CancellationToken> {
        self.get_extension::<sync::CancellationToken>(DISPATCH_CANCEL_EXTENSION)
            .cloned()
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
            // claimfamily: clones carry no claim — duplicating one would
            // double-release on drop. Fanout sites that need each copy
            // counted split a sibling explicitly.
            in_flight_claim: None,
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
    use crate::Body;
    use crate::in_flight::{InFlightClaim, InFlightGauge};
    use serde_json::json;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn remove_property_removes_and_is_idempotent() {
        let mut ex = Exchange::new(Message::default());
        ex.set_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED, Value::Bool(true));
        assert_eq!(
            ex.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED),
            Some(&Value::Bool(true))
        );

        ex.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED);
        assert_eq!(ex.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED), None);

        ex.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED);
        assert_eq!(ex.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED), None);
    }

    #[test]
    fn remove_property_absent_is_noop() {
        let mut ex = Exchange::new(Message::default());
        assert_eq!(ex.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED), None);

        ex.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED);

        assert_eq!(ex.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED), None);
    }

    #[test]
    fn original_content_type_snapshot_roundtrips() {
        let mut ex = Exchange::new(Message::default());
        ex.set_extension(
            ORIGINAL_CONTENT_TYPE_INHERITED_EXTENSION,
            Arc::new(OriginalContentTypeInherited(true)),
        );

        let snapshot = ex
            .get_extension::<OriginalContentTypeInherited>(
                ORIGINAL_CONTENT_TYPE_INHERITED_EXTENSION,
            )
            .expect("snapshot stored under its extension key");
        assert!(snapshot.0);

        let fresh = Exchange::new(Message::default());
        assert!(
            fresh
                .get_extension::<OriginalContentTypeInherited>(
                    ORIGINAL_CONTENT_TYPE_INHERITED_EXTENSION
                )
                .is_none()
        );
    }

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
    fn test_set_error_populates_properties() {
        let mut ex = Exchange::default();
        ex.set_error(CamelError::ProcessorError("boom".into()));

        assert!(ex.has_error());
        assert_eq!(
            ex.properties.get(PROPERTY_EXCEPTION_MESSAGE),
            Some(&Value::String("Processor error: boom".to_string()))
        );
        assert_eq!(
            ex.properties.get(PROPERTY_EXCEPTION_KIND),
            Some(&Value::String("processor".to_string()))
        );
        assert_eq!(
            ex.properties.get(PROPERTY_EXCEPTION_CAUGHT),
            Some(&Value::String("Processor error: boom".to_string()))
        );
    }

    #[test]
    fn test_set_error_http_failure_exposes_status_headers() {
        let mut ex = Exchange::default();
        ex.set_error(CamelError::HttpOperationFailed {
            method: "GET".into(),
            url: "http://example.test/x".into(),
            status_code: 404,
            status_text: "Not Found".into(),
            response_body: None,
        });

        assert_eq!(
            ex.input.header("CamelHttpResponseCode"),
            Some(&Value::Number(404.into()))
        );
        assert_eq!(
            ex.input.header("CamelHttpResponseText"),
            Some(&Value::String("Not Found".to_string()))
        );
    }

    #[test]
    fn test_set_error_non_http_leaves_http_headers_untouched() {
        let mut ex = Exchange::default();
        ex.set_error(CamelError::ProcessorError("boom".into()));

        assert!(ex.input.header("CamelHttpResponseCode").is_none());
        assert!(ex.input.header("CamelHttpResponseText").is_none());
    }

    #[test]
    fn test_clear_error_removes_properties() {
        let mut ex = Exchange::default();
        ex.set_error(CamelError::RouteError("fail".into()));
        assert!(ex.has_error());
        assert!(ex.properties.contains_key(PROPERTY_EXCEPTION_MESSAGE));

        ex.clear_error();

        assert!(!ex.has_error());
        assert!(!ex.properties.contains_key(PROPERTY_EXCEPTION_MESSAGE));
        assert!(!ex.properties.contains_key(PROPERTY_EXCEPTION_KIND));
        assert!(!ex.properties.contains_key(PROPERTY_EXCEPTION_CAUGHT));
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

    #[test]
    fn test_body_as_string_from_text() {
        let ex = Exchange::new(Message::new(Body::Text("hello".to_string())));

        let result = ex.body_as::<String>();

        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn test_body_as_string_from_json_string() {
        let ex = Exchange::new(Message::new(Body::Json(json!("hello"))));

        let result = ex.body_as::<String>();

        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn test_body_as_json_value_from_json_number() {
        let ex = Exchange::new(Message::new(Body::Json(json!(42))));

        let result = ex.body_as::<serde_json::Value>();

        assert_eq!(result.unwrap(), json!(42));
    }

    #[test]
    fn test_body_as_vec_u8_from_bytes() {
        let ex = Exchange::new(Message::new(Body::from(vec![1u8, 2, 3, 4])));

        let result = ex.body_as::<Vec<u8>>();

        assert_eq!(result.unwrap(), vec![1u8, 2, 3, 4]);
    }

    #[test]
    fn test_body_as_string_from_empty_returns_err() {
        let ex = Exchange::new(Message::new(Body::Empty));

        let result = ex.body_as::<String>();

        assert!(result.is_err());
    }

    #[test]
    fn branch_clone_splits_claim_original_unaffected() {
        let gauge = Arc::new(InFlightGauge::new());
        let mut src = Exchange::new(Message::default());
        src.in_flight_claim = Some(InFlightClaim::attach(&gauge));

        let copy = src.branch_clone();

        assert_eq!(gauge.total(), 2);
        assert!(copy.in_flight_claim.is_some());
        assert!(src.in_flight_claim.is_some());
    }

    #[test]
    fn branch_clone_sibling_releases_independently() {
        let gauge = Arc::new(InFlightGauge::new());
        let mut src = Exchange::new(Message::default());
        src.in_flight_claim = Some(InFlightClaim::attach(&gauge));

        let copy = src.branch_clone();
        drop(copy);
        assert_eq!(gauge.total(), 1);

        drop(src);
        assert_eq!(gauge.total(), 0);
    }

    #[test]
    fn plain_clone_carries_no_claim() {
        let gauge = Arc::new(InFlightGauge::new());
        let mut src = Exchange::new(Message::default());
        src.in_flight_claim = Some(InFlightClaim::attach(&gauge));
        assert_eq!(gauge.total(), 1);

        let copy = src.clone();

        assert_eq!(gauge.total(), 1);
        assert!(copy.in_flight_claim.is_none());
        assert!(src.in_flight_claim.is_some());
    }

    #[test]
    fn dispatch_cancel_begin_stamps_a_live_token() {
        let mut ex = Exchange::new(Message::new("x"));

        let cancel = ex.begin_dispatch_cancel();

        assert!(ex.dispatch_cancel().is_some());
        assert!(!ex.dispatch_cancel().expect("marker stamped").is_cancelled());
        assert!(!cancel.token().is_cancelled());
    }

    #[test]
    fn dispatch_cancel_guard_cancels_on_drop() {
        let mut ex = Exchange::new(Message::new("x"));
        let cancel = ex.begin_dispatch_cancel();

        let token = cancel.token().clone();
        drop(cancel);

        assert!(token.is_cancelled());
    }

    #[test]
    fn dispatch_cancel_disarm_leaves_token_live() {
        let mut ex = Exchange::new(Message::new("x"));
        let cancel = ex.begin_dispatch_cancel();

        let token = cancel.token().clone();
        cancel.disarm();

        assert!(!token.is_cancelled());
    }

    #[test]
    fn dispatch_cancel_take_removes_the_marker() {
        let mut ex = Exchange::new(Message::new("x"));
        ex.begin_dispatch_cancel().disarm();

        let taken = ex.take_dispatch_cancel();

        assert!(taken.is_some());
        assert!(ex.take_dispatch_cancel().is_none());
        assert!(ex.dispatch_cancel().is_none());
    }

    #[test]
    fn dispatch_cancel_drop_during_shutdown_leaves_token_live() {
        let mut ex = Exchange::new(Message::new("x"));
        let owner = CancellationToken::new();
        let cancel = ex.begin_dispatch_cancel().drain_on(owner.clone());

        let token = cancel.token().clone();
        owner.cancel();
        drop(cancel);

        assert!(!token.is_cancelled());
    }

    #[test]
    fn dispatch_cancel_drop_cancels_when_owner_live() {
        let mut ex = Exchange::new(Message::new("x"));
        let owner = CancellationToken::new();
        let cancel = ex.begin_dispatch_cancel().drain_on(owner.clone());

        let token = cancel.token().clone();
        assert!(!owner.is_cancelled());
        drop(cancel);

        assert!(token.is_cancelled());
    }

    #[test]
    fn dispatch_cancel_clone_shares_the_same_token() {
        let mut ex = Exchange::new(Message::new("x"));
        ex.begin_dispatch_cancel().disarm();

        let copy = ex.clone();
        let token = copy.dispatch_cancel().expect("marker on clone");
        token.cancel();

        assert!(
            ex.dispatch_cancel()
                .expect("marker on original")
                .is_cancelled()
        );
    }
}
