//! Test-only span harness backed by the SDK in-memory exporter.
//!
//! ONE global `SdkTracerProvider` is installed per test binary: the first
//! `test_spans()` call builds the exporter/provider pair and invokes
//! `global::set_tracer_provider` exactly once per process. Every call then
//! acquires a process-wide async mutex and resets the exporter, so test
//! bodies are serialized and spans cannot leak between tests.
//!
//! As defense-in-depth (the provider is process-global and other tests may
//! run concurrently), tests still filter the finished spans returned by
//! [`finish`] by the trace id of the parent span they created themselves.

use std::sync::{Arc, OnceLock};

use opentelemetry::global;
use opentelemetry_sdk::trace::{
    InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor, SpanData,
};

/// Handle returned by [`test_spans`].
///
/// Holding `TestSpans` keeps the serialization guard alive; pass it to
/// [`finish`] to flush and collect the spans exported during the test.
pub(crate) struct TestSpans {
    provider: SdkTracerProvider,
    exporter: Arc<InMemorySpanExporter>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

/// Install (once per process) the global in-memory tracer provider, reset the
/// exporter, and acquire the lock that serializes test bodies.
pub(crate) async fn test_spans() -> TestSpans {
    static HARNESS: OnceLock<(SdkTracerProvider, Arc<InMemorySpanExporter>)> = OnceLock::new();
    static LOCK: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();

    let (provider, exporter) = HARNESS.get_or_init(|| {
        let exporter = Arc::new(InMemorySpanExporter::default());
        let provider = SdkTracerProvider::builder()
            .with_span_processor(SimpleSpanProcessor::new(exporter.as_ref().clone()))
            .build();
        global::set_tracer_provider(provider.clone());
        (provider, exporter)
    });

    let guard = LOCK
        .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
        .lock_owned()
        .await;

    exporter.reset();

    TestSpans {
        provider: provider.clone(),
        exporter: Arc::clone(exporter),
        _guard: guard,
    }
}

/// Flush the provider and collect the spans exported while the guard was held.
pub(crate) fn finish(spans: TestSpans) -> Vec<SpanData> {
    // Test-only failure path: the in-memory provider flush cannot be handled.
    spans.provider.force_flush().expect("flush exported spans"); // allow-unwrap
    // Test-only failure path: the in-memory read cannot be handled.
    spans
        .exporter
        .get_finished_spans()
        .expect("read exported spans") // allow-unwrap
}
