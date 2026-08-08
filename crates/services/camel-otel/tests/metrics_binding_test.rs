//! Integration test for rc-z0y3: proves the post-start meter binding in
//! `OtelMetrics` resolves to the REAL global `MeterProvider`, not a cached
//! no-op.
//!
//! Background (audit-fix-otel-lifecycle, task 1.3):
//! Before the fix, `OtelMetrics` resolved and cached its `Meter` on the very
//! first record call. If that call happened before `OtelService::start()`
//! installed the real global `MeterProvider`, the cached meter was the no-op
//! default — and every later record silently vanished. The fix gates meter
//! and instrument resolution on a `started: AtomicBool` set by
//! `mark_started()`, which is called immediately after the global provider is
//! installed. Pre-start records are silent no-ops that populate nothing.
//!
//! This test exercises the full path end-to-end with a real
//! `SdkMeterProvider` plus an `InMemoryMetricExporter` so we can read what
//! actually got exported.

use camel_api::MetricsCollector;
use camel_otel::OtelMetrics;
use opentelemetry::global;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

/// Metric name asserted on. Kept in sync with `metric_names::EXCHANGES_TOTAL`
/// in `camel-otel/src/metrics.rs`.
const EXCHANGES_TOTAL: &str = "camel.exchanges.total";

/// Walk a `Vec<ResourceMetrics>` and return the name of every metric it
/// contains. Used for assertion messages so failures are diagnosable.
fn metric_names(resource_metrics: &[ResourceMetrics]) -> Vec<String> {
    resource_metrics
        .iter()
        .flat_map(|rm| rm.scope_metrics())
        .flat_map(|sm| {
            sm.metrics()
                .map(|m| m.name().to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Does a `Vec<ResourceMetrics>` contain a metric with the given name?
fn has_metric(resource_metrics: &[ResourceMetrics], name: &str) -> bool {
    resource_metrics
        .iter()
        .flat_map(|rm| rm.scope_metrics())
        .any(|sm| sm.metrics().any(|m| m.name() == name))
}

#[test]
#[serial_test::serial]
fn post_start_binds_real_provider() {
    // 1. Build a real OTel pipeline that funnels into an in-memory exporter
    //    so we can read what was actually recorded. PeriodicReader +
    //    force_flush is the only fully synchronous collect path available
    //    without enabling the `experimental_metrics_custom_reader` feature
    //    (which is NOT enabled in the workspace, so `ManualReader` and the
    //    `MetricReader` trait are crate-private).
    let exporter = InMemoryMetricExporter::default();
    // Short interval as defense in depth: force_flush is the primary
    // mechanism, but a small interval means the first scheduled export
    // happens promptly if any code path bypasses flush.
    let reader = PeriodicReader::builder(exporter.clone())
        .with_interval(std::time::Duration::from_millis(50))
        .build();
    let provider = SdkMeterProvider::builder().with_reader(reader).build();

    // 2. Create OtelMetrics — NOT yet started — WHILE the global is still the
    //    no-op DEFAULT. This is the canonical rc-z0y3 bug shape: a record now
    //    (without the fix) would resolve `global::meter_with_scope` against the
    //    no-op default and cache that no-op meter permanently. We deliberately
    //    do NOT call `set_meter_provider` until after the pre-start record, so
    //    this test catches a regression that re-broke the unset-global caching
    //    path.
    let metrics = OtelMetrics::new("binding-test");

    // 3. Record before mark_started AND before the real global is installed.
    //    Per the contract, this is a silent no-op: instruments() returns None,
    //    meter() returns None, no DashMap entries are created, no OTel call is
    //    made, and crucially NO no-op meter is cached in the OnceLock.
    metrics.increment_exchanges("route-1");

    // 4. NOW install the real global provider — simulating OtelService::start()
    //    having set it up.
    global::set_meter_provider(provider.clone());

    // 5. Force a flush so any data that somehow reached the exporter would be
    //    visible. With the gate, the pre-start record populated nothing, so the
    //    real exporter is empty.
    provider.force_flush().expect("force_flush should succeed");
    let pre_start_metrics = exporter
        .get_finished_metrics()
        .expect("get_finished_metrics should succeed");
    assert!(
        !has_metric(&pre_start_metrics, EXCHANGES_TOTAL),
        "pre-start recording must not produce `{}`; got {:?}",
        EXCHANGES_TOTAL,
        metric_names(&pre_start_metrics),
    );

    // Clear between phases so post-start assertions only see what was
    // recorded after the gate flipped.
    exporter.reset();

    // 6. Flip the start gate. From this point, the OnceLocks for `meter`
    //    and `instruments` are allowed to resolve — and the global provider
    //    is the real one installed in step 4. (Under a regression that cached
    //    a no-op meter at step 3, this post-start record would vanish into the
    //    cached no-op and the next assertion would fail.)
    metrics.mark_started();

    // 7. Record again. The gate is open, so the call walks meter_inner() →
    //    global::meter_with_scope(...) → resolves to our SdkMeterProvider →
    //    increments the real counter.
    metrics.increment_exchanges("route-1");

    // 8. Force flush and assert the metric is now in the exporter.
    provider.force_flush().expect("force_flush should succeed");
    let post_start_metrics = exporter
        .get_finished_metrics()
        .expect("get_finished_metrics should succeed");
    assert!(
        has_metric(&post_start_metrics, EXCHANGES_TOTAL),
        "post-start recording must produce `{}`; got {:?}",
        EXCHANGES_TOTAL,
        metric_names(&post_start_metrics),
    );

    // Best-effort shutdown. We don't assert on the result — the test
    // contract is the metric export, not provider teardown.
    let _ = provider.shutdown();
}
