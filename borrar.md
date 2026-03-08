Issues
Critical (Must Fix)

1. Semantic incorrectness in set_queue_depth and record_circuit_breaker_change (metrics.rs:155-181)
   The MetricsCollector trait defines set_queue_depth to SET an absolute value (as evidenced by the Prometheus implementation using Gauge.set()). However, the OTel implementation uses UpDownCounter.add(depth) which treats depth as a delta, not an absolute value.
   Impact: If you call:
   metrics.set_queue_depth("route", 5); // Counter becomes 5
   metrics.set_queue_depth("route", 10); // Counter becomes 15, NOT 10!
   Similarly for record_circuit_breaker_change:
   metrics.record_circuit_breaker_change("route", "closed", "open"); // +1 → total: 1
   metrics.record_circuit_breaker_change("route", "open", "closed"); // +0 → total: 1 (should be 0)
   metrics.record_circuit_breaker_change("route", "closed", "half_open"); // +2 → total: 3 (should be 2)
   Fix: Use ObservableGauge with callbacks, or track previous values per route to compute deltas:
   // Option A: ObservableGauge (preferred for true gauge semantics)
   use opentelemetry::metrics::ObservableGauge;
   // Option B: Track previous values (requires interior mutability)
   use std::sync::Mutex;
   struct OtelMetrics {
   // ...
   queue_depths: Mutex<HashMap<String, i64>>,
   }

---

Important (Should Fix) 2. Weak test in test_start_stop_lifecycle (service.rs:268-293)
if result.is_ok() {
// assertions...
}
// If start failed due to connection issues, that's expected with invalid endpoint
The test passes even when start() fails, which means it doesn't actually verify the lifecycle. Either:

- Use a mock/no-op exporter for testing
- Or assert that the behavior is correct when it succeeds

3. Missing validation of TraceIdRatioBased ratio bounds (config.rs:18)
   The TraceIdRatioBased(f64) should be validated to be in range [0.0, 1.0]. While OTel SDK may handle invalid values, explicit validation provides better error messages:
   OtelSampler::TraceIdRatioBased(ratio) if !(0.0..=1.0).contains(&ratio) => {
   return Err(CamelError::Config("Sampler ratio must be between 0.0 and 1.0"));
   }
4. Global state pollution between tests (service.rs tests)
   Tests that call global::set_tracer_provider() and global::set_meter_provider() pollute global state. Running tests in parallel could cause flaky behavior. Consider:

- Using #[serial] test attribute
- Or creating isolated providers without setting globals in tests

5. Inconsistent attribute key naming (metrics.rs:29-31)
   pub const ROUTE*ID: &str = "route.id";
   pub const CIRCUIT_BREAKER_TO_STATE: &str = "circuit_breaker.to_state"; // inconsistent!
   One uses . separator, the other uses *. Follow OpenTelemetry semantic conventions consistently (typically dots).

---

Minor (Nice to Have) 6. Box::leak documentation could be clearer (metrics.rs:76-77)
The comment says "small, one-time memory leak per unique service name" but doesn't quantify the impact. Consider adding:
// Typically ~20-50 bytes per unique service name.
// Acceptable for long-running services with static service names. 7. Missing #[non_exhaustive] on enums (config.rs:3, 12)
OtelProtocol and OtelSampler should be #[non_exhaustive] to allow adding variants without breaking changes. 8. Endpoint URL construction could use url crate (service.rs:69, 85)
.with_endpoint(format!("{}/v1/traces", self.config.endpoint))
If endpoint has a trailing slash, this produces http://host:4317//v1/traces. Consider using proper URL joining.
