use super::OtelService;
use crate::config::{OtelConfig, OtelSampler};
use camel_api::MetricsCollector;
use camel_builder::{RouteBuilder, StepAccumulator as _};
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use camel_processor::LogLevel;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::Sampler;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Layer, SubscriberExt};

struct RecordingCollector {
    events: Mutex<Vec<String>>,
}

impl RecordingCollector {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    fn snapshot(&self) -> Vec<String> {
        self.events.lock().expect("collector lock").clone()
    }
}

impl MetricsCollector for RecordingCollector {
    fn increment_exchanges(&self, route_id: &str) {
        self.events
            .lock()
            .expect("collector lock")
            .push(format!("{route_id}:exchanges"));
    }

    fn record_exchange_duration(&self, route_id: &str, _duration: Duration) {
        self.events
            .lock()
            .expect("collector lock")
            .push(format!("{route_id}:duration"));
    }

    fn increment_errors(&self, route_id: &str, _error_type: &str) {
        self.events
            .lock()
            .expect("collector lock")
            .push(format!("{route_id}:errors"));
    }

    fn set_queue_depth(&self, _queue: &str, _depth: usize) {}

    fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}
}

async fn wait_for_ticks(notify: &Notify, ticks: &AtomicUsize, target: usize) {
    while ticks.load(Ordering::SeqCst) < target {
        tokio::time::timeout(Duration::from_secs(5), notify.notified())
            .await
            .expect("timer route did not reach target ticks");
    }
}

#[tokio::test]
#[serial_test::serial]
async fn excluded_route_exports_no_spans_but_counts_metrics() {
    let exporter = InMemorySpanExporter::default();
    let sampler = OtelService::to_sdk_sampler(
        &OtelConfig::new("http://localhost:4317", "t6")
            .with_sampler(OtelSampler::AlwaysOn)
            .with_exclude_routes(vec!["noisy-*".into()]),
    )
    .expect("valid sampler config");
    let provider = SdkTracerProvider::builder()
        .with_sampler(sampler)
        .with_span_processor(SimpleSpanProcessor::new(exporter.clone()))
        .build();
    global::set_tracer_provider(provider.clone());

    let collector = Arc::new(RecordingCollector::new());
    let ctx = CamelContext::builder()
        .metrics(Arc::clone(&collector) as Arc<dyn MetricsCollector>)
        .build()
        .await
        .expect("context builds");
    let mut ctx = ctx
        .with_tracer_config(camel_core::TracerConfig {
            enabled: true,
            ..Default::default()
        })
        .await;
    ctx.register_component(TimerComponent::new());

    let noisy_notify = Arc::new(Notify::new());
    let noisy_ticks = Arc::new(AtomicUsize::new(0));
    let real_notify = Arc::new(Notify::new());
    let real_ticks = Arc::new(AtomicUsize::new(0));

    let noisy_definition = {
        let notify_step = Arc::clone(&noisy_notify);
        let ticks_step = Arc::clone(&noisy_ticks);
        RouteBuilder::from("timer:noisy?period=50")
            .route_id("noisy-tick")
            .process(move |ex| {
                let ticks = Arc::clone(&ticks_step);
                let notify = Arc::clone(&notify_step);
                async move {
                    ticks.fetch_add(1, Ordering::SeqCst);
                    notify.notify_one();
                    Ok(ex)
                }
            })
            .build()
            .expect("noisy route builds")
    };
    let real_definition = {
        let notify_step = Arc::clone(&real_notify);
        let ticks_step = Arc::clone(&real_ticks);
        RouteBuilder::from("timer:real?period=50")
            .route_id("real-flow")
            .process(move |ex| {
                let ticks = Arc::clone(&ticks_step);
                let notify = Arc::clone(&notify_step);
                async move {
                    ticks.fetch_add(1, Ordering::SeqCst);
                    notify.notify_one();
                    Ok(ex)
                }
            })
            .build()
            .expect("real route builds")
    };

    ctx.add_route_definition(noisy_definition)
        .await
        .expect("noisy route registers");
    ctx.add_route_definition(real_definition)
        .await
        .expect("real route registers");

    ctx.start().await.expect("context starts");
    wait_for_ticks(&noisy_notify, &noisy_ticks, 3).await;
    wait_for_ticks(&real_notify, &real_ticks, 3).await;
    ctx.stop().await.expect("context stops");
    provider.force_flush().expect("flush spans");

    let spans = exporter.get_finished_spans().expect("exported spans");
    let noisy_spans: Vec<_> = spans
        .iter()
        .filter(|span| {
            span.name.starts_with("noisy-tick")
                || span.attributes.iter().any(|kv| {
                    kv.key.as_str() == "route_id" && kv.value.as_str().starts_with("noisy-")
                })
        })
        .map(|span| span.name.to_string())
        .collect();
    assert!(
        noisy_spans.is_empty(),
        "excluded route must export no spans, got: {noisy_spans:?}"
    );
    assert!(
        spans.iter().any(|span| span.name == "real-flow"),
        "non-excluded route must export a span named real-flow"
    );

    let events = collector.snapshot();
    for (route_id, family) in [
        ("noisy-tick", "exchanges"),
        ("noisy-tick", "duration"),
        ("real-flow", "exchanges"),
        ("real-flow", "duration"),
    ] {
        let event = format!("{route_id}:{family}");
        let count = events.iter().filter(|recorded| *recorded == &event).count();
        assert!(
            count >= 3,
            "collector must record at least 3 {event} events after >=3 ticks, got {count}: {events:?}"
        );
    }
}

/// Shared byte buffer for the `fmt` layer, mirroring the test pattern in
/// `crates/camel-processor/src/log.rs`.
#[derive(Clone)]
struct FmtBuf(Arc<Mutex<Vec<u8>>>);

impl FmtBuf {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    fn contents(&self) -> String {
        let bytes = self.0.lock().expect("fmt buffer lock").clone();
        String::from_utf8(bytes).expect("fmt buffer is valid utf-8")
    }
}

impl std::io::Write for FmtBuf {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("fmt buffer lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for FmtBuf {
    type Writer = FmtBuf;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// T7: engine spans are not orphan roots.
///
/// One real timer exchange exports exactly the route root and its two step
/// spans. The engine's route-compiler and camel-tracer spans are `tracing`
/// spans whose OTel bridge is left on its default no-op tracer, so they never
/// become orphan roots. The `fmt` layer still sees the route log marker.
#[test]
#[serial_test::serial]
fn engine_spans_are_not_orphan_roots() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .with_span_processor(SimpleSpanProcessor::new(exporter.clone()))
        .build();
    global::set_tracer_provider(provider.clone());

    let fmt_buf = FmtBuf::new();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer())
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(fmt_buf.clone())
                .with_ansi(false)
                .with_filter(LevelFilter::INFO),
        );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime builds");

    tracing::subscriber::with_default(subscriber, || {
        runtime.block_on(async {
            let ctx = CamelContext::builder()
                .build()
                .await
                .expect("context builds");
            let mut ctx = ctx
                .with_tracer_config(camel_core::TracerConfig {
                    enabled: true,
                    ..Default::default()
                })
                .await;
            ctx.register_component(TimerComponent::new());

            let notify = Arc::new(Notify::new());
            let notify_step = Arc::clone(&notify);
            let definition = RouteBuilder::from("timer:t7?period=50&repeatCount=1")
                .route_id("t7-normal")
                .log("t7-fmt-marker", LogLevel::Info)
                .process(move |ex| {
                    let notify = Arc::clone(&notify_step);
                    async move {
                        notify.notify_one();
                        Ok(ex)
                    }
                })
                .build()
                .expect("route builds");
            ctx.add_route_definition(definition)
                .await
                .expect("route registers");

            ctx.start().await.expect("context starts");
            tokio::time::timeout(Duration::from_secs(5), notify.notified())
                .await
                .expect("t7 route did not fire");
            ctx.stop().await.expect("context stops");

            drop(tracing::debug_span!(
                target: "camel_core::lifecycle::adapters::route_compiler",
                "pipeline_step",
                index = 0
            ));
            drop(tracing::debug_span!(
                target: "camel_core::lifecycle::adapters::route_compiler",
                "error_handler",
                step_index = 0
            ));
            drop(tracing::info_span!(target: "camel_function", "function"));
            drop(tracing::info_span!(target: "camel_tracer", "step"));
        });
    });

    provider.force_flush().expect("flush spans");

    let spans = exporter.get_finished_spans().expect("exported spans");
    let route_root = spans
        .iter()
        .find(|span| span.name == "t7-normal")
        .expect("route root span exported");
    let trace_id = route_root.span_context.trace_id();
    let mut names: Vec<String> = spans
        .iter()
        .filter(|span| span.span_context.trace_id() == trace_id)
        .map(|span| span.name.to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "t7-normal".to_string(),
            "t7-normal:log".to_string(),
            "t7-normal:step-1".to_string(),
        ],
        "route trace must contain exactly the route root and its two step spans"
    );
    for forbidden in ["pipeline_step", "error_handler", "function", "step"] {
        assert!(
            spans.iter().all(|span| span.name != forbidden),
            "engine span {forbidden} must not be exported through the no-op bridge"
        );
    }

    let fmt_output = fmt_buf.contents();
    assert!(
        fmt_output.contains("t7-fmt-marker"),
        "fmt layer must capture the route log marker, got: {fmt_output}"
    );
}

/// Control: proves the bridge would export engine twins when wired to a real
/// tracer. Production leaves the bridge on its no-op tracer; this local
/// provider and explicit `.with_tracer` are test-only.
#[test]
fn real_tracer_bridge_exports_engine_twins() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .with_span_processor(SimpleSpanProcessor::new(exporter.clone()))
        .build();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("control")));

    tracing::subscriber::with_default(subscriber, || {
        drop(tracing::debug_span!(
            target: "camel_core::lifecycle::adapters::route_compiler",
            "pipeline_step",
            index = 0
        ));
        drop(tracing::debug_span!(
            target: "camel_core::lifecycle::adapters::route_compiler",
            "error_handler",
            step_index = 0
        ));
        drop(tracing::info_span!(target: "camel_tracer", "step"));
        drop(tracing::info_span!(target: "camel_function", "function"));
    });

    provider.force_flush().expect("flush spans");

    let spans = exporter.get_finished_spans().expect("exported spans");
    let mut names: Vec<String> = spans.iter().map(|span| span.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "error_handler".to_string(),
            "function".to_string(),
            "pipeline_step".to_string(),
            "step".to_string(),
        ],
        "a real tracer bridge must export all four engine-target spans"
    );
}
