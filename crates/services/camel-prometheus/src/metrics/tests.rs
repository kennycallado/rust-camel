use super::*;
use camel_api::ComponentMetrics;
use std::sync::Arc;

#[test]
fn test_create_prometheus_metrics() {
    let metrics = PrometheusMetrics::new();
    // Verify registry is accessible
    let _ = metrics.registry();
}

#[test]
fn test_default_implementation() {
    let metrics = PrometheusMetrics::default();
    // Verify registry is accessible
    let _ = metrics.registry();
}

#[test]
fn test_increment_exchanges() {
    let metrics = PrometheusMetrics::new();

    // Should not panic
    metrics.increment_exchanges("test-route");
    metrics.increment_exchanges("test-route");
    metrics.increment_exchanges("other-route");

    // Verify the metric is registered
    let output = metrics.gather();
    assert!(output.contains("camel_exchanges_total"));
    assert!(output.contains("test-route"));
    assert!(output.contains("other-route"));
}

#[test]
fn test_increment_errors() {
    let metrics = PrometheusMetrics::new();

    // Should not panic
    metrics.increment_errors("test-route", "timeout");
    metrics.increment_errors("test-route", "connection_failed");
    metrics.increment_errors("other-route", "timeout");

    // Verify the metric is registered
    let output = metrics.gather();
    assert!(output.contains("camel_errors_total"));
    assert!(output.contains("timeout"));
    assert!(output.contains("connection_failed"));
}

#[test]
fn test_record_exchange_duration() {
    let metrics = PrometheusMetrics::new();

    // Should not panic
    metrics.record_exchange_duration("test-route", Duration::from_millis(50));
    metrics.record_exchange_duration("test-route", Duration::from_millis(150));
    metrics.record_exchange_duration("other-route", Duration::from_secs(1));

    // Verify the metric is registered
    let output = metrics.gather();
    assert!(output.contains("camel_exchange_duration_seconds"));
    assert!(output.contains("test-route"));
}

#[test]
fn test_set_queue_depth() {
    let metrics = PrometheusMetrics::new();

    // Should not panic
    metrics.set_queue_depth("seda:work", 5);
    metrics.set_queue_depth("seda:work", 10);
    metrics.set_queue_depth("aggregator:agg-route", 3);

    // Verify the metric is registered with the `queue` label (spec:
    // `camel_queue_depth{queue}`)
    let output = metrics.gather();
    assert!(output.contains("camel_queue_depth{queue=\"seda:work\"}"));
}

#[test]
fn test_record_circuit_breaker_change() {
    let metrics = PrometheusMetrics::new();

    // Should not panic
    metrics.record_circuit_breaker_change("test-route", "closed", "open");
    metrics.record_circuit_breaker_change("test-route", "open", "half_open");
    metrics.record_circuit_breaker_change("test-route", "half_open", "closed");

    // Verify the metric is registered
    let output = metrics.gather();
    assert!(output.contains("camel_circuit_breaker_state"));
}

#[test]
fn prometheus_registers_new_series() {
    let metrics = PrometheusMetrics::new();
    metrics.increment_retry_attempt("kafka", "connect");
    metrics.increment_circuit_breaker_rejection("r1");
    let body = metrics.gather();
    assert!(
        body.contains("camel_retry_attempts_total{operation=\"connect\",scheme=\"kafka\"} 1"),
        "missing retry series: {body}"
    );
    assert!(
        body.contains("camel_circuit_breaker_rejections_total{route=\"r1\"} 1"),
        "missing rejection series: {body}"
    );
}

#[test]
fn route_state_gauge_transitions() {
    let metrics = PrometheusMetrics::new();
    metrics.set_route_state("r1", "Starting");
    metrics.set_route_state("r1", "Running");
    let body = metrics.gather();
    assert!(
        body.contains("camel_route_state{route=\"r1\",state=\"Running\"} 1"),
        "missing Running series: {body}"
    );
    assert!(
        body.contains("camel_route_state{route=\"r1\",state=\"Starting\"} 0"),
        "Starting series must read 0 after the transition: {body}"
    );
}

#[test]
/// Task 3.1 follow-up (review finding): removing a route drops its
/// series — no phantom 1-valued state for undeployed routes.
fn route_state_gauge_removal_drops_series() {
    let metrics = PrometheusMetrics::new();
    metrics.set_route_state("gone", "Started");
    let body = metrics.gather();
    assert!(
        body.contains("camel_route_state{route=\"gone\",state=\"Started\"} 1"),
        "series present before removal: {body}"
    );
    metrics.clear_route_state("gone");
    let body = metrics.gather();
    assert!(
        !body.contains("route=\"gone\""),
        "removed route must leave no series behind: {body}"
    );
}

#[test]
fn build_info_and_uptime_rendered() {
    let metrics = PrometheusMetrics::new();
    metrics.record_build_info("1.2.3", "abc1234");
    metrics.record_uptime(0.5);
    let body = metrics.gather();
    assert!(
        body.contains("camel_build_info{git_sha=\"abc1234\",version=\"1.2.3\"} 1"),
        "missing build-info series: {body}"
    );
    assert!(
        body.contains("camel_uptime_seconds 0.5"),
        "missing uptime series: {body}"
    );
}

#[test]
fn test_gather_returns_prometheus_format() {
    let metrics = PrometheusMetrics::new();

    // Record some metrics
    metrics.increment_exchanges("route-1");
    metrics.increment_errors("route-1", "timeout");
    metrics.set_queue_depth("route-1", 5);

    // Gather metrics
    let output = metrics.gather();

    // Verify output is valid Prometheus text format
    assert!(output.starts_with("# HELP") || output.starts_with("# TYPE"));
    assert!(output.contains("camel_exchanges_total"));
    assert!(output.contains("camel_errors_total"));
    assert!(output.contains("camel_queue_depth"));

    // Verify labels use 'route' not 'route_id'
    assert!(output.contains("route=\"route-1\""));
    assert!(!output.contains("route_id=\"route-1\""));
}

#[test]
fn test_metrics_collector_trait_object() {
    // Verify PrometheusMetrics can be used as a trait object
    let metrics: Arc<dyn MetricsCollector> = Arc::new(PrometheusMetrics::new());

    // All methods should work without panicking
    metrics.increment_exchanges("test-route");
    metrics.increment_errors("test-route", "test-error");
    metrics.record_exchange_duration("test-route", Duration::from_millis(100));
    metrics.set_queue_depth("test-route", 5);
    metrics.record_circuit_breaker_change("test-route", "closed", "open");
}

#[test]
fn test_record_counter_dynamic_basic() {
    let metrics = PrometheusMetrics::new();
    metrics.record_counter("exec_spawns_total", 1.0, &[("route", "r1")]);
    metrics.record_counter("exec_spawns_total", 1.0, &[("route", "r1")]);
    let out = metrics.gather();
    assert!(
        out.contains("camel_exec_spawns_total"),
        "normalized name missing: {out}"
    );
    assert!(out.contains("route=\"r1\""));
}

#[test]
fn test_record_counter_multi_label_ordering_invariant() {
    let metrics = PrometheusMetrics::new();
    // Same metric, labels in different orders — must land on the same series.
    metrics.record_counter(
        "exec_policy_denials_total",
        1.0,
        &[("reason", "denied"), ("route", "r1")],
    );
    metrics.record_counter(
        "exec_policy_denials_total",
        1.0,
        &[("route", "r1"), ("reason", "denied")],
    );
    let out = metrics.gather();
    assert!(out.contains("camel_exec_policy_denials_total"));
    // Both observations recorded on one series (count == 2). Extract the
    // numeric value robustly from the Prometheus text line.
    let count: f64 = out
        .lines()
        .filter(|l| {
            l.contains("camel_exec_policy_denials_total") && l.contains("reason=\"denied\"")
        })
        .filter_map(|l| l.rsplit(' ').next().and_then(|v| v.parse::<f64>().ok()))
        .sum();
    assert_eq!(count, 2.0, "expected count 2, got {count}");
}

#[test]
fn test_record_counter_arity_drift_dropped() {
    let metrics = PrometheusMetrics::new();
    // First observation freezes key-set {route}.
    metrics.record_counter("drift_total", 1.0, &[("route", "r1")]);
    // Second observation adds a key — must be dropped (arity mismatch).
    metrics.record_counter("drift_total", 1.0, &[("route", "r1"), ("extra", "x")]);
    let out = metrics.gather();
    // Only the first observation recorded (count == 1).
    let count: f64 = out
        .lines()
        .filter(|l| l.contains("camel_drift_total"))
        .filter_map(|l| l.rsplit(' ').next().and_then(|v| v.parse::<f64>().ok()))
        .sum();
    assert_eq!(count, 1.0, "expected count 1 (drift dropped), got {count}");
}

#[test]
fn test_record_counter_value_guards() {
    let metrics = PrometheusMetrics::new();
    metrics.record_counter("bad_total", f64::NAN, &[("route", "r1")]);
    metrics.record_counter("bad_total", -1.0, &[("route", "r1")]);
    metrics.record_counter("bad_total", 1.5, &[("route", "r1")]);
    // All values rejected — metric must NOT appear in gather output.
    let out = metrics.gather();
    assert!(
        !out.contains("camel_bad_total"),
        "value guards should prevent cache population; found metric in output"
    );
}

#[test]
fn test_record_counter_tombstone_on_collision() {
    let metrics = PrometheusMetrics::new();
    // "camel_exchanges_total" already registered as a fixed metric.
    // A dynamic call with the same normalized name must tombstone (AlreadyReg),
    // and NOT panic.
    metrics.record_counter("exchanges_total", 1.0, &[("route", "r1")]);
    // Call again — tombstone should make this a silent no-op (no re-attempt).
    metrics.record_counter("exchanges_total", 1.0, &[("route", "r2")]);
    // Verify the tombstone was inserted (None = registration failed).
    let tombstoned = metrics
        .dyn_counters
        .get("camel_exchanges_total")
        .map(|e| e.is_none())
        .unwrap_or(false);
    assert!(tombstoned, "expected tombstone for colliding metric name");
}

#[test]
fn test_record_counter_warn_dedup() {
    let metrics = PrometheusMetrics::new();
    // Three bad-value calls for the same name.
    metrics.record_counter("dedup_total", f64::NAN, &[("route", "r1")]);
    metrics.record_counter("dedup_total", -1.0, &[("route", "r1")]);
    metrics.record_counter("dedup_total", 1.5, &[("route", "r1")]);
    // The warned set should contain the name exactly once (dedup).
    assert!(
        metrics.warned.contains("dedup_total"),
        "warned set should contain the offending name"
    );
}

#[test]
fn test_record_counter_fixed_and_dynamic_in_one_gather() {
    let metrics = PrometheusMetrics::new();
    metrics.increment_exchanges("r1"); // fixed metric
    metrics.record_counter("dyn_total", 1.0, &[("route", "r1")]); // dynamic metric
    let out = metrics.gather();
    assert!(
        out.contains("camel_exchanges_total"),
        "fixed metric missing"
    );
    assert!(out.contains("camel_dyn_total"), "dynamic metric missing");
}

#[test]
fn test_record_counter_trait_object_dispatch() {
    // Retain a concrete handle to verify post-call state; Arc::downcast
    // requires `Any` which MetricsCollector does not have.
    let concrete = Arc::new(PrometheusMetrics::new());
    let dynref: Arc<dyn MetricsCollector> = concrete.clone();
    dynref.record_counter("trait_total", 1.0, &[("route", "r1")]);
    // Verify it dispatched to the real impl (not the no-op default).
    let out = concrete.gather();
    assert!(out.contains("camel_trait_total"));
}

#[test]
fn test_record_counter_concurrent_no_panic() {
    use std::thread;
    let metrics = Arc::new(PrometheusMetrics::new());
    let mut handles = Vec::new();
    for i in 0..4 {
        let m = Arc::clone(&metrics);
        handles.push(thread::spawn(move || {
            let route = format!("route-{i}");
            for _ in 0..100 {
                m.record_counter("concurrent_total", 1.0, &[("route", &route)]);
            }
        }));
    }
    for h in handles {
        h.join().expect("thread panicked under contention");
    }
    // Verify the metric was recorded (total == 400).
    let out = metrics.gather();
    let total: f64 = out
        .lines()
        .filter(|l| l.contains("camel_concurrent_total"))
        .filter_map(|l| l.rsplit(' ').next().and_then(|v| v.parse::<f64>().ok()))
        .sum();
    assert_eq!(total, 400.0, "expected 400 total observations, got {total}");
}

#[test]
fn test_record_histogram_dynamic_basic() {
    let metrics = PrometheusMetrics::new();
    metrics.record_histogram("exec_duration_secs", 0.15, &[("route", "r1")]);
    metrics.record_histogram("exec_duration_secs", 0.5, &[("route", "r1")]);
    let out = metrics.gather();
    assert!(
        out.contains("camel_exec_duration_secs"),
        "normalized name missing: {out}"
    );
    assert!(out.contains("route=\"r1\""));
    // Prometheus text format emits histogram count as a _count suffixed series.
    assert!(
        out.contains("camel_exec_duration_secs_count"),
        "expected histogram count series, got: {out}"
    );
}

#[test]
fn test_record_histogram_nan_rejected() {
    let metrics = PrometheusMetrics::new();
    metrics.record_histogram("nan_hist", f64::NAN, &[("route", "r1")]);
    let out = metrics.gather();
    assert!(
        !out.contains("camel_nan_hist"),
        "NaN value should not create a histogram; found it in output: {out}"
    );
}

#[test]
fn test_record_histogram_accepts_fractional() {
    let metrics = PrometheusMetrics::new();
    // Histograms legitimately accept fractional values (durations, cost).
    metrics.record_histogram("frac_hist", 1.5, &[("route", "r1")]);
    let out = metrics.gather();
    assert!(out.contains("camel_frac_hist"));
}

#[test]
fn test_record_histogram_trait_object_dispatch() {
    let concrete = Arc::new(PrometheusMetrics::new());
    let dynref: Arc<dyn MetricsCollector> = concrete.clone();
    dynref.record_histogram("trait_hist", 0.25, &[("route", "r1")]);
    let out = concrete.gather();
    assert!(out.contains("camel_trait_hist"));
}

#[test]
fn default_max_dynamic_collectors_is_1024() {
    let metrics = PrometheusMetrics::new();
    assert_eq!(metrics.max_dynamic_collectors(), 1024);
}

/// Task 4.1: two observes through the lever-on facade render the uniform
/// component-operations family with the exact label set.
#[test]
fn prometheus_renders_family() {
    let concrete = Arc::new(PrometheusMetrics::new());
    let facade = ComponentMetrics::new(concrete.clone() as Arc<dyn MetricsCollector>, true);
    facade.observe("redis", "command", false);
    facade.observe("redis", "command", true);
    let body = concrete.gather();
    assert!(
        body.contains(
            "camel_component_operations_total{component=\"redis\",operation=\"command\",outcome=\"success\"} 1"
        ),
        "missing component-ops success series: {body}"
    );
}

/// Task 4.1 render-level lever pin (inter-phase deferred finding,
/// metrics-configuration Req 3 "default excludes component family"):
/// with the components lever off the family is absent from gather()
/// output; with it on the family renders. Seam pinned: the
/// facade+collector seam — lever gating lives BEFORE prometheus (the
/// off-facade never calls `record_component_operation`), so no child
/// series is created and the registry renders nothing for the family.
#[test]
fn components_lever_suppresses_family_render() {
    let concrete = Arc::new(PrometheusMetrics::new());

    let off = ComponentMetrics::new(concrete.clone() as Arc<dyn MetricsCollector>, false);
    off.observe("redis", "command", true);
    off.observe("redis", "command", false);
    let body = concrete.gather();
    assert!(
        !body.contains("camel_component_operations_total"),
        "lever off must keep the family out of the render: {body}"
    );

    let on = ComponentMetrics::new(concrete.clone() as Arc<dyn MetricsCollector>, true);
    on.observe("redis", "command", false);
    let body = concrete.gather();
    assert!(
        body.contains("camel_component_operations_total"),
        "lever on must render the family: {body}"
    );
}

#[test]
fn dynamic_counter_within_cap_accepted() {
    let metrics = PrometheusMetrics::new().with_max_dynamic_collectors(3);
    metrics.record_counter("within_a", 1.0, &[]);
    metrics.record_counter("within_b", 1.0, &[]);
    metrics.record_counter("within_c", 1.0, &[]);
    let out = metrics.gather();
    assert!(out.contains("camel_within_a"), "missing a: {out}");
    assert!(out.contains("camel_within_b"), "missing b: {out}");
    assert!(out.contains("camel_within_c"), "missing c: {out}");
}

#[tracing_test::traced_test]
#[test]
fn dynamic_counter_exceeding_cap_rejected() {
    let metrics = PrometheusMetrics::new().with_max_dynamic_collectors(2);
    metrics.record_counter("capd_a", 1.0, &[]);
    metrics.record_counter("capd_b", 1.0, &[]);
    metrics.record_counter("capd_c", 1.0, &[]); // over cap — must be dropped
    let out = metrics.gather();
    assert!(out.contains("camel_capd_a"), "a missing: {out}");
    assert!(out.contains("camel_capd_b"), "b missing: {out}");
    assert!(
        !out.contains("camel_capd_c"),
        "c should have been rejected, but appears in output: {out}"
    );
    assert!(logs_contain("cap exceeded"));
}

#[tracing_test::traced_test]
#[test]
fn dynamic_histogram_exceeding_cap_rejected() {
    let metrics = PrometheusMetrics::new().with_max_dynamic_collectors(2);
    metrics.record_histogram("caph_a", 0.1, &[]);
    metrics.record_histogram("caph_b", 0.2, &[]);
    metrics.record_histogram("caph_c", 0.3, &[]); // over cap — must be dropped
    let out = metrics.gather();
    assert!(out.contains("camel_caph_a"), "a missing: {out}");
    assert!(out.contains("camel_caph_b"), "b missing: {out}");
    assert!(
        !out.contains("camel_caph_c"),
        "c should have been rejected, but appears in output: {out}"
    );
    assert!(logs_contain("cap exceeded"));
}

#[test]
fn existing_counter_still_works_after_cap_hit() {
    let metrics = PrometheusMetrics::new().with_max_dynamic_collectors(2);
    metrics.record_counter("repeat_a", 1.0, &[]);
    metrics.record_counter("repeat_b", 1.0, &[]); // fills cap
    metrics.record_counter("repeat_c", 1.0, &[]); // rejected
    metrics.record_counter("repeat_a", 5.0, &[]); // already tracked — must still update
    let out = metrics.gather();
    // Total value for `a` series should be 1.0 + 5.0 = 6.0.
    let total: f64 = out
        .lines()
        .filter(|l| l.contains("camel_repeat_a"))
        .filter_map(|l| l.rsplit(' ').next().and_then(|v| v.parse::<f64>().ok()))
        .sum();
    assert_eq!(
        total, 6.0,
        "expected 6.0 for `a` after cap hit, got {total}"
    );
    assert!(
        !out.contains("camel_repeat_c"),
        "c should have been rejected: {out}"
    );
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn normalize_prom_name_prepends_camel_prefix() {
        assert_eq!(
            normalize_prom_name("exec_spawns_total"),
            "camel_exec_spawns_total"
        );
    }

    #[test]
    fn normalize_prom_name_keeps_existing_camel_prefix() {
        assert_eq!(normalize_prom_name("camel_foo_total"), "camel_foo_total");
    }

    #[test]
    fn normalize_prom_name_replaces_invalid_chars() {
        // dots and spaces are invalid in Prometheus metric names
        assert_eq!(
            normalize_prom_name("my.metric name"),
            "camel_my_metric_name"
        );
    }

    #[test]
    fn normalize_prom_name_numeric_name_gets_camel_prefix() {
        assert_eq!(normalize_prom_name("123foo"), "camel_123foo");
    }

    #[test]
    fn sort_label_pairs_orders_by_key() {
        let labels = [("route", "r1"), ("reason", "denied")];
        let sorted = sort_label_pairs(&labels);
        assert_eq!(sorted, vec![("reason", "denied"), ("route", "r1")]);
    }

    #[test]
    fn sort_label_pairs_already_sorted_is_noop() {
        let labels = [("code", "0"), ("route", "r1")];
        let sorted = sort_label_pairs(&labels);
        assert_eq!(sorted, vec![("code", "0"), ("route", "r1")]);
    }

    #[test]
    fn counter_value_ok_accepts_positive_integers() {
        assert!(counter_value_ok(1.0));
        assert!(counter_value_ok(5.0));
        assert!(counter_value_ok(0.0));
    }

    #[test]
    fn counter_value_ok_rejects_nan_negative_and_fractional() {
        assert!(!counter_value_ok(f64::NAN));
        assert!(!counter_value_ok(-1.0));
        assert!(!counter_value_ok(1.5));
    }
}
