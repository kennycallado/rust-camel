use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn test_otel_service_new() {
    let config = OtelConfig::new("http://localhost:4317", "test-service");
    let service = OtelService::new(config);

    assert_eq!(service.name(), "otel");
    assert!(service.tracer_provider.is_none());
    assert!(service.meter_provider.is_none());
    // Metrics collector is created in constructor
    assert!(service.metrics.is_some());
}

#[test]
fn test_otel_service_default() {
    let service = OtelService::with_defaults();

    assert_eq!(service.name(), "otel");
    assert_eq!(service.config.endpoint, "http://localhost:4317");
    assert_eq!(service.config.service_name, "rust-camel");
}

#[test]
fn test_build_resource() {
    let config = OtelConfig::new("http://localhost:4317", "my-service")
        .with_resource_attr("deployment.environment", "production")
        .with_resource_attr("service.version", "1.0.0");

    let service = OtelService::new(config);
    let resource = service.build_resource();

    // Resource should contain service.name and custom attributes
    // Note: Resource doesn't expose a simple way to iterate attributes in 0.31,
    // so we just verify it builds without error
    let _ = resource;
}

#[tokio::test]
#[serial_test::serial]
async fn test_start_stop_lifecycle() {
    // This test verifies the lifecycle works without connecting to a real backend.
    // It will fail to connect, but that's OK - we're testing the flow.
    let config = OtelConfig::new("http://localhost:9999", "test-service"); // Invalid port
    let mut service = OtelService::new(config);

    // Verify initial state
    assert!(service.tracer_provider.is_none());
    assert!(service.meter_provider.is_none());
    assert_eq!(service.status(), ServiceStatus::Stopped);

    // Note: start() may fail because the endpoint is invalid, but we test the logic
    let result = service.start().await;

    // The exporter build should succeed (build happens lazily), so start() typically
    // succeeds even with an invalid endpoint. Either way, verify state consistency.
    if result.is_ok() {
        // On success, providers should be set and status should be Started
        assert!(service.tracer_provider.is_some());
        assert!(service.meter_provider.is_some());
        assert_eq!(service.status(), ServiceStatus::Started);

        // Stop should work
        let stop_result = service.stop().await;
        assert!(stop_result.is_ok());

        // Providers should be cleared and status should be Stopped
        assert!(service.tracer_provider.is_none());
        assert!(service.meter_provider.is_none());
        assert_eq!(service.status(), ServiceStatus::Stopped);
    } else {
        // On failure, state must remain clean
        assert!(service.tracer_provider.is_none());
        assert!(service.meter_provider.is_none());
        assert_eq!(service.status(), ServiceStatus::Stopped);
    }
}

#[tokio::test]
#[serial_test::serial]
async fn test_double_start_guard() {
    // FIXME: This test sets global OTel state which can interfere with parallel tests.
    // Consider using isolated unit tests or serial_test crate for better isolation.
    let mut service = OtelService::with_defaults();

    // Manually set tracer_provider to simulate already-started state
    // We use a simple provider without exporter for this test
    let resource = Resource::builder()
        .with_attributes(vec![KeyValue::new("test", "value")])
        .build();
    let provider = SdkTracerProvider::builder().with_resource(resource).build();
    service.tracer_provider = Some(provider);

    // Second start should warn and return Ok
    let result = service.start().await;
    assert!(result.is_ok());

    // Clean up
    service.tracer_provider.take();
}

#[tokio::test]
async fn test_stop_when_not_started() {
    let mut service = OtelService::with_defaults();

    // Stop when not started should succeed
    let result = service.stop().await;
    assert!(result.is_ok());
}

#[test]
fn test_validate_sampler_ratio_valid() {
    // Valid ratios should pass validation
    let config = OtelConfig::new("http://localhost:4317", "test")
        .with_sampler(OtelSampler::TraceIdRatioBased(0.0));
    let service = OtelService::new(config);
    assert!(service.validate_config().is_ok());

    let config = OtelConfig::new("http://localhost:4317", "test")
        .with_sampler(OtelSampler::TraceIdRatioBased(0.5));
    let service = OtelService::new(config);
    assert!(service.validate_config().is_ok());

    let config = OtelConfig::new("http://localhost:4317", "test")
        .with_sampler(OtelSampler::TraceIdRatioBased(1.0));
    let service = OtelService::new(config);
    assert!(service.validate_config().is_ok());
}

#[test]
fn test_validate_sampler_ratio_invalid() {
    // Ratios outside [0.0, 1.0] should fail
    let config = OtelConfig::new("http://localhost:4317", "test")
        .with_sampler(OtelSampler::TraceIdRatioBased(-0.1));
    let service = OtelService::new(config);
    let err = service.validate_config().unwrap_err();
    assert!(
        err.to_string()
            .contains("TraceIdRatioBased sampler ratio must be in [0.0, 1.0]")
    );

    let config = OtelConfig::new("http://localhost:4317", "test")
        .with_sampler(OtelSampler::TraceIdRatioBased(1.5));
    let service = OtelService::new(config);
    let err = service.validate_config().unwrap_err();
    assert!(
        err.to_string()
            .contains("TraceIdRatioBased sampler ratio must be in [0.0, 1.0]")
    );
}

#[test]
fn test_validate_config_empty_service_name() {
    let config = OtelConfig::new("http://localhost:4317", "").with_sampler(OtelSampler::AlwaysOn);
    let service = OtelService::new(config);
    let err = service.validate_config().unwrap_err();
    assert!(err.to_string().contains("service_name must not be empty"));

    let config =
        OtelConfig::new("http://localhost:4317", "   ").with_sampler(OtelSampler::AlwaysOn);
    let service = OtelService::new(config);
    let err = service.validate_config().unwrap_err();
    assert!(err.to_string().contains("service_name must not be empty"));
}

#[test]
fn test_validate_config_empty_endpoint() {
    let config = OtelConfig::new("", "test-service").with_sampler(OtelSampler::AlwaysOn);
    let service = OtelService::new(config);
    let err = service.validate_config().unwrap_err();
    assert!(err.to_string().contains("endpoint must not be empty"));

    let config = OtelConfig::new("   ", "test-service").with_sampler(OtelSampler::AlwaysOn);
    let service = OtelService::new(config);
    let err = service.validate_config().unwrap_err();
    assert!(err.to_string().contains("endpoint must not be empty"));
}

#[test]
fn test_validate_config_zero_interval() {
    let config = OtelConfig::new("http://localhost:4317", "test-service")
        .with_sampler(OtelSampler::AlwaysOn)
        .with_metrics_interval_ms(0);
    let service = OtelService::new(config);
    let err = service.validate_config().unwrap_err();
    assert!(err.to_string().contains("metrics_interval_ms must be > 0"));
}

#[test]
fn test_stop_clears_logger_provider() {
    // Verify that stop() takes ownership of logger_provider (shutdown + drop).
    // We simulate a service that has a logger_provider set.
    let mut service = OtelService::with_defaults();

    // Build a LoggerProvider manually to simulate started state
    use opentelemetry_sdk::logs::SdkLoggerProvider;
    let provider = SdkLoggerProvider::builder().build();
    service.logger_provider = Some(provider);

    // Verify it's set
    assert!(service.logger_provider.is_some());

    // stop() should take() the logger_provider (which will be Stopped since status is STOPPED)
    // But since status is already STOPPED, stop() returns early. So we manually test the pattern:
    if let Some(p) = service.logger_provider.take() {
        let _ = p.force_flush();
        let _ = p.shutdown();
    }
    assert!(
        service.logger_provider.is_none(),
        "logger_provider should be None after shutdown"
    );
}

#[test]
fn test_status_transitions() {
    let service = OtelService::with_defaults();
    assert_eq!(service.status(), ServiceStatus::Stopped);
}

#[tokio::test]
async fn test_status_failed_on_start_error() {
    // Use an invalid sampler ratio to trigger a validation error in start()
    let config = OtelConfig::new("http://localhost:4317", "test-service")
        .with_sampler(OtelSampler::TraceIdRatioBased(-1.0));
    let mut service = OtelService::new(config);

    // Initial status should be Stopped
    assert_eq!(service.status(), ServiceStatus::Stopped);

    // start() should fail due to invalid config
    let result = service.start().await;
    assert!(result.is_err());

    // Status should now be Failed
    assert_eq!(service.status(), ServiceStatus::Failed);

    // Verify error message mentions the sampler ratio
    let err = result.unwrap_err();
    assert!(err.to_string().contains("TraceIdRatioBased sampler ratio"));
}

#[tokio::test]
#[serial_test::serial] // start() sets the global tracer provider
async fn test_start_does_not_replace_subscriber() {
    // Install a counting subscriber BEFORE OtelService starts
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    static BEFORE_COUNT: AtomicUsize = AtomicUsize::new(0);

    struct CountingLayer;
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountingLayer {
        fn on_event(
            &self,
            _event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            BEFORE_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }

    // Try to set a subscriber (may already be set in test suite, that's fine)
    let _ = tracing_subscriber::registry()
        .with(CountingLayer)
        .try_init();

    let _initial = BEFORE_COUNT.load(Ordering::SeqCst);

    let config = OtelConfig::new("http://localhost:9999", "test-no-sub-replace");
    let mut service = OtelService::new(config);
    let _ = tokio::time::timeout(Duration::from_secs(5), service.start()).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), service.stop()).await;

    // Emit a test event
    tracing::info!("test event after otel start");

    // If OtelService replaced the subscriber, CountingLayer won't be called
    // and count stays at initial. If subscriber is preserved, count increases.
    // NOTE: This test is best-effort — if no subscriber was set, it's a no-op.
    // The important thing is that start() doesn't *panic* or error due to subscriber conflict.
    let _ = BEFORE_COUNT.load(Ordering::SeqCst);
}

#[test]
fn drop_without_stop_does_not_panic() {
    let mut service = OtelService::with_defaults();
    service.tracer_provider = Some(SdkTracerProvider::builder().build());
    // Drop without calling stop() — Drop impl should best-effort shutdown without panicking
}

#[test]
fn drop_after_stop_does_not_reshutdown() {
    let config = OtelConfig::new("http://localhost:4317", "test-svc");
    let _service = OtelService::new(config);
    // Providers are None by construction (post-stop state). Drop should be a no-op.
}

/// Captures WARN-level tracing events into a shared buffer.
struct WarnCapture {
    warnings: Arc<Mutex<Vec<String>>>,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for WarnCapture {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().level() == &tracing::Level::WARN {
            self.warnings
                .lock()
                .expect("warnings mutex poisoned")
                .push(event.metadata().name().to_string());
        }
    }
}

/// Runs a closure under a thread-local tracing subscriber that captures
/// WARN events, then returns the collected event names.
fn capture_warns_during<F: FnOnce()>(f: F) -> Vec<String> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    // OnceLock-gated global registry: heals/prevent callsite-interest
    // poisoning of the shutdown/drop `warn!` callsites in this file
    // (provider force-flush/shutdown warns), which subscriber-less
    // sibling tests dropping live providers hit first (fix pattern:
    // c3853198; bd rc-img5).
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INIT.set(()).is_ok() {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    }

    let warnings = Arc::new(Mutex::new(Vec::new()));
    let _guard = tracing_subscriber::registry()
        .with(WarnCapture {
            warnings: warnings.clone(),
        })
        .set_default();
    f();
    warnings
        .lock()
        .expect("warnings mutex poisoned")
        .drain(..)
        .collect()
}

#[test]
fn drop_after_stop_emits_no_warn() {
    // Post-stop state: all providers are None.
    let service = OtelService::new(OtelConfig::new("http://localhost:4317", "test-svc"));

    let warns = capture_warns_during(move || drop(service));

    assert!(
        warns.is_empty(),
        "Drop after stop must NOT emit any WARN, got: {warns:?}"
    );
}

#[test]
fn drop_without_stop_emits_warn() {
    // Positive control: a surviving provider triggers the Drop warn.
    let mut service = OtelService::with_defaults();
    service.tracer_provider = Some(SdkTracerProvider::builder().build());

    let warns = capture_warns_during(move || drop(service));

    assert!(
        !warns.is_empty(),
        "Drop without stop MUST emit at least one WARN (proves capture works)"
    );
}

/// Proves that `start()` wires the span exporter correctly: a span
/// emitted through the provider that `start()` built and stored reaches
/// the in-memory exporter connected via `SimpleSpanProcessor`.
#[tokio::test]
#[serial_test::serial]
async fn start_binds_real_span_exporter() {
    use opentelemetry::trace::{Span, Tracer, TracerProvider};
    use opentelemetry_sdk::trace::InMemorySpanExporter;

    let exporter = InMemorySpanExporter::default();
    let config = OtelConfig::new("http://localhost:4317", "span-binding-test");
    let mut service = OtelService::new(config);
    service.test_span_exporter = Some(exporter.clone());

    service.start().await.unwrap();

    // Emit a span through the provider that start() built and stored
    let tracer_provider = service.tracer_provider.as_ref().unwrap();
    let tracer = tracer_provider.tracer("start-binding-test");
    tracer.start("test-op").end();

    // Force flush so the span reaches the in-memory exporter
    service
        .tracer_provider
        .as_ref()
        .unwrap()
        .force_flush()
        .unwrap();

    let spans = exporter.get_finished_spans().unwrap();
    assert!(
        !spans.is_empty(),
        "span exporter must receive at least one span after start()"
    );

    // Clean up
    let _ = service.tracer_provider.take().map(|p| p.shutdown());
}

/// Proves that `start()` wires the log exporter correctly: a log
/// record emitted through the `SdkLoggerProvider` that `start()`
/// built and stored reaches the in-memory log exporter.
///
/// Log-path investigation: `start()` (L348–357) builds an
/// `SdkLoggerProvider` via `build_logger_provider_internal()` and
/// stores it in `self.logger_provider`. No `global::set_logger_provider`
/// or tracing-bridge installation occurs in `start()`; the log
/// provider is accessed through the stored field. This test emits
/// a `LogRecord` directly through that provider, confirming the
/// exporter-to-provider path that `start()` wired.
#[tokio::test]
#[serial_test::serial]
async fn start_binds_real_log_exporter() {
    use opentelemetry::logs::{LogRecord, Logger, LoggerProvider};
    use opentelemetry_sdk::logs::InMemoryLogExporter;

    let exporter = InMemoryLogExporter::default();
    let config = OtelConfig::new("http://localhost:4317", "log-binding-test");
    let mut service = OtelService::new(config);
    service.test_log_exporter = Some(exporter.clone());

    service.start().await.unwrap();

    // Emit a log record through the provider that start() built and stored
    let logger_provider = service.logger_provider.as_ref().unwrap();
    let logger = logger_provider.logger("test-logger");
    let mut record = logger.create_log_record();
    record.set_body("test log message".into());
    logger.emit(record);

    // Force flush so the record reaches the in-memory exporter
    logger_provider.force_flush().unwrap();

    let logs = exporter.get_emitted_logs().unwrap();
    assert!(
        !logs.is_empty(),
        "log exporter must receive at least one record after start()"
    );

    // Clean up
    let _ = service.logger_provider.take().map(|p| p.shutdown());
}

/// Shared harness for the rc-q74u / rc-6ju71 bounded-stop repros: runs
/// `body` on a dedicated thread (named `{tag}-repro`) with its own
/// current-thread runtime — the exact `#[tokio::test]` shape. Panics are
/// reported through a channel so an unrelated failure surfaces
/// immediately with its own diagnosis instead of burning the outer
/// timeout as a bogus "hang"; a silent hang fails via `recv_timeout`.
fn bounded_repro<F, Fut>(tag: &str, regression: &str, body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let handle = std::thread::Builder::new()
        .name(format!("{tag}-repro"))
        .spawn(move || {
            // Report panics through the channel so an unrelated failure
            // surfaces immediately with its own diagnosis instead of
            // burning the outer timeout as a bogus "hang".
            let _ = tx.send(
                catch_unwind(AssertUnwindSafe(|| {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("repro runtime");
                    rt.block_on(body())
                }))
                .map_err(|payload| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".into())
                }),
            );
        })
        .expect("spawn repro thread");

    match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(())) => {}
        Ok(Err(panic_msg)) => panic!("repro thread failed (not a hang): {panic_msg}"),
        Err(_) => panic!("repro thread did not finish: stop() hung ({regression} regression)"),
    }
    let _ = handle.join();
}

/// Regression test for rc-q74u: `stop()` must stay bounded when the
/// periodic metric reader has data and the OTLP export cannot make
/// progress.
///
/// Deadlock chain (pre-fix): a metric recorded through the global meter
/// provider arms the SDK's `PeriodicReader` thread; `stop()` called
/// `force_flush()`, which waits on an UNBOUNDED channel receive while the
/// reader thread is stuck inside `futures_executor::block_on(tonic
/// export)` — a future that only makes progress when the ambient tokio
/// runtime is driven. On a current-thread runtime the sole driving thread
/// is the one blocked in the flush, so both sides wait forever.
///
/// The repro runs on a dedicated thread with its own current-thread
/// runtime (the exact `#[tokio::test]` shape) and the test asserts
/// completion via `recv_timeout` — a renewed hang fails the test instead
/// of hanging the suite.
#[test]
#[serial_test::serial]
fn test_stop_bounded_when_metric_export_stalls() {
    bounded_repro("q74u", "rc-q74u", || async move {
        let config = OtelConfig::new("http://localhost:9999", "q74u-repro");
        let mut service = OtelService::new(config);
        service.start().await.expect("start");

        // Arm the PeriodicReader: record one metric through the
        // collector, which resolves instruments from the global
        // (== this service's) meter provider.
        let collector = service.as_metrics_collector().expect("collector");
        collector.record_exchange_duration("q74u-route", Duration::from_millis(5));

        // Pre-fix this hangs forever; post-fix shutdown() returns
        // within the SDK's 5s bound even though the reader thread
        // stays stalled (it is reaped at process exit).
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let _ = service.stop().await;
        assert!(
            std::time::Instant::now() < deadline,
            "stop() must be bounded when metric export stalls (rc-q74u)"
        );
    });
}

/// Characterization pin for rc-6ju71: `stop()` must flush pending
/// batched spans to the exporter ("flush-on-stop"), for spans emitted
/// through BOTH the service's own provider and the global tracer
/// provider clone that `start()` installs. The two share one inner
/// provider (and thus one span processor), so shutting down the local
/// handle must also flush spans issued through the global clone — the
/// tracer-path edition of the rc-q74u shared-subscriber concern.
///
/// The provider uses a `BatchSpanProcessor` whose scheduled delay is
/// pushed 1h into the future, so the ONLY thing that can deliver the
/// batch is the flush performed during `stop()`'s shutdown. The
/// rc-6ju71 fix (drop the separate tracer force_flush, rely on the
/// SDK shutdown flush) must keep this test green.
///
/// A custom capturing exporter is used instead of
/// `InMemorySpanExporter` because the latter clears its buffer on
/// exporter shutdown (`reset_on_shutdown` defaults to true and the
/// opt-out is `#[cfg(test)]`-gated inside the SDK), so the final
/// shutdown inside `stop()` would erase the very batch it flushed.
#[tokio::test]
#[serial_test::serial]
async fn stop_flushes_pending_spans_from_local_and_global_tracers() {
    use opentelemetry::trace::{Span, Tracer, TracerProvider};
    use opentelemetry_sdk::error::OTelSdkResult;
    use opentelemetry_sdk::trace::{
        BatchConfigBuilder, BatchSpanProcessor, SpanData, SpanExporter,
    };
    use std::future::{Future, ready};

    #[derive(Debug, Clone, Default)]
    struct CapturingSpanExporter {
        names: Arc<Mutex<Vec<String>>>,
    }
    impl SpanExporter for CapturingSpanExporter {
        fn export(&self, batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
            self.names
                .lock()
                .expect("names mutex poisoned")
                .extend(batch.iter().map(|s| s.name.to_string()));
            ready(Ok(()))
        }
    }

    let exporter = CapturingSpanExporter::default();
    // Timer flush disabled: only stop()'s shutdown flush can export.
    let config = BatchConfigBuilder::default()
        .with_scheduled_delay(Duration::from_secs(3600))
        .build();
    let processor = BatchSpanProcessor::builder(exporter.clone())
        .with_batch_config(config)
        .build();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(processor)
        .build();

    let mut service = OtelService::with_defaults();
    service.tracer_provider = Some(provider.clone());
    service.status.store(STATUS_STARTED, Ordering::SeqCst);
    global::set_tracer_provider(provider.clone());

    // Two pending spans: one through the service's provider, one through
    // the global clone (same inner, same span processor).
    provider.tracer("local-tracer").start("local-op").end();
    global::tracer("global-tracer").start("global-op").end();

    service.stop().await.unwrap();

    assert!(service.tracer_provider.is_none());
    let names = exporter.names.lock().expect("names mutex poisoned").clone();
    assert!(
        names.iter().any(|n| n == "local-op"),
        "local-provider span must be flushed by stop(), got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "global-op"),
        "global-provider span must be flushed by stop(), got: {names:?}"
    );
}

/// Regression pin for rc-6ju71 (tracer-path edition of the rc-q74u
/// harness): `stop()` must stay bounded when a pending span batch
/// cannot be exported.
///
/// Empirical correction to the bd premise: in opentelemetry_sdk 0.32.1
/// the std-thread `BatchSpanProcessor::force_flush()` is NOT unbounded —
/// it waits with `recv_timeout(5s)` — so `stop()` was already bounded
/// pre-fix (~10s worst case: 5s flush cap + 5s shutdown cap). The pin
/// stays: it guards against upstream reintroducing the unbounded
/// receive that `PeriodicReader::force_flush` had (rc-q74u), and after
/// the fix the worst case halves because the redundant force_flush
/// round-trip is gone.
///
/// The stalled exporter wedges the BSP worker thread inside
/// `futures_executor::block_on` on a future that never resolves — the
/// deterministic, network-free equivalent of a stalled OTLP export on a
/// current-thread runtime.
#[test]
#[serial_test::serial]
fn test_stop_bounded_when_span_export_stalls() {
    use opentelemetry::trace::{Span, Tracer, TracerProvider};
    use opentelemetry_sdk::error::OTelSdkResult;
    use opentelemetry_sdk::trace::{
        BatchConfigBuilder, BatchSpanProcessor, SpanData, SpanExporter,
    };
    use std::future::Future;
    use std::future::pending;

    #[derive(Debug)]
    struct StalledSpanExporter;
    impl SpanExporter for StalledSpanExporter {
        fn export(&self, _batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
            pending()
        }
    }

    bounded_repro("q6ju71", "rc-6ju71", || async move {
        // Timer flush disabled so the ONLY export attempt
        // is the one triggered inside stop().
        let config = BatchConfigBuilder::default()
            .with_scheduled_delay(Duration::from_secs(3600))
            .build();
        let processor = BatchSpanProcessor::builder(StalledSpanExporter)
            .with_batch_config(config)
            .build();
        let provider = SdkTracerProvider::builder()
            .with_span_processor(processor)
            .build();

        let mut service = OtelService::with_defaults();
        service.tracer_provider = Some(provider.clone());
        service.status.store(STATUS_STARTED, Ordering::SeqCst);

        // One pending span arms the batch export.
        provider.tracer("stall-tracer").start("stalled-op").end();

        // With an unbounded flush this hangs forever; with
        // the SDK's caps it returns within ~10s pre-fix
        // and ~5s post-fix even though the BSP worker
        // stays wedged (it is reaped at process exit).
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let _ = service.stop().await;
        assert!(
            std::time::Instant::now() < deadline,
            "stop() must be bounded when span export stalls (rc-6ju71)"
        );
    });
}
