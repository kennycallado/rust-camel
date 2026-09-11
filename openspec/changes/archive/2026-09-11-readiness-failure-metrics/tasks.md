# Tasks: readiness-failure-metrics

## Task 1: Record families on the tracer readiness Err arm

**Files:**
- `crates/camel-core/src/shared/observability/adapters/tracer.rs` (modified — `poll_ready` Err arm records via `record_step_metrics`)
- `crates/camel-core/src/shared/observability/adapters/tracer_tests.rs` (modified — `readiness_err_records_families`, stale bypass comment refreshed)

**Steps:**
1. RED: add `ReadinessErrProcessor` double and
   `readiness_err_records_families` (readiness Err must record one
   exchange, one `increment_errors:r:processor`, one duration); run
   `cargo test -p camel-core --lib readiness_err_records_families` — it
   must fail with `got []`.
2. GREEN: record `record_step_metrics(self.metrics.as_ref(),
   &self.route_id, &self.metric_levers, start.elapsed(), &Err(e.clone()))`
   in the `poll_ready` `Poll::Ready(Err(e))` arm; propagate the error
   unchanged.
3. `cargo test -p camel-core --lib` green.

**Acceptance:**
- New test passes; full `cargo test -p camel-core --lib` exit 0.
- No new label values; CIRCUIT_OPEN skip preserved.

- [x] 1

## Task 2: Integration leg on the readiness path + spec delta

**Files:**
- `crates/camel-test/tests/metrics_wiring_test.rs` (modified — `add_failing_route` drops the `failIfNoConsumers=false` opt-out; sample-level scrape assertions; stale header comments updated; call-time legs keep explicit opt-outs)
- `openspec/changes/readiness-failure-metrics/specs/metrics-collection-wiring/spec.md` (new delta)

**Steps:**
1. RED: switch the shared error leg to `.to("direct:missing")` and
   strengthen the scrape poll/assertions to `camel_exchanges_total{` /
   `camel_errors_total{`; run
   `prometheus_only_emits_pipeline_and_component_metrics` — it must fail
   with "never exposed a camel_exchanges_total sample".
2. GREEN: with Task 1's fix the leg passes; keep the call-time legs
   (`late_registration_after_routes_observed`, `direct_lookup_failure_
   emits_b_prime_once`, `direct_wired_route_no_double_count`) on their own
   explicit `failIfNoConsumers=false` routes.
3. Add the MODIFIED-requirements delta under
   `specs/metrics-collection-wiring/` quoting the full updated
   requirement plus the new readiness scenario.
4. `openspec validate readiness-failure-metrics --type change --json` —
   no delta-structure errors.

**Acceptance:**
- `cargo test -p camel-test --test metrics_wiring_test` green (9 legs).
- Delta validates clean; scenario "readiness-phase producer failure is
  observable" present under the existing disposition-counter requirement.

- [x] 2
