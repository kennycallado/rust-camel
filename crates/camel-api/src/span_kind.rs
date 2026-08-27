/// Compile-time hint for the OpenTelemetry span kind of a step span.
///
/// Contract: this enum is a *compile-time* hint supplied by the DSL compiler
/// when a `TracingProcessor` step span is built; the consumer converts it to
/// `opentelemetry::trace::SpanKind` once at construction. The enum is
/// `#[non_exhaustive]`: variants added in the future must degrade to
/// [`SpanKindHint::Internal`] at the consumption site, never cause a compile
/// failure downstream.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpanKindHint {
    /// Internal span inside a route; the default for plain process steps.
    #[default]
    Internal,
    /// Producer span for a step that sends a message to a destination.
    Producer,
    /// Consumer span for a step that consumes a message from a source.
    Consumer,
    /// Client span for a synchronous outbound call.
    Client,
    /// Server span for a synchronous inbound handler.
    Server,
}
