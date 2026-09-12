# Tasks: steplatency

## camel-core compiled-step metadata

### Task 1.1: Retain declared To URIs during compilation

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/endpoints.rs` (modified)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/routing.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/transforms.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_ext.rs` (modified)

**Steps:**
1. Add `to_uri: Option<Arc<str>>` to `CompiledStep::Process` and initialize it to `None` in every existing process-step constructor.
2. Add `BuilderStep::to_uri_metadata() -> Option<Arc<str>>`, returning the raw declared URI only for `BuilderStep::To`, and stamp it in the compiler registry before endpoint compilation.
3. Preserve the field through compiler and lifecycle transformations, including route interception and body-contract assembly.
4. Add a focused compiler test that compiles `To("direct:orders")` and asserts `Some("direct:orders")`, plus a non-To process test asserting `None`.

**Tests:**
- `compiled_to_step_retains_declared_uri`: arrange a route-definition compiler with one `BuilderStep::To("direct:orders")`; act by compiling the step; assert the resulting `CompiledStep::Process.to_uri` is `Some(Arc::from("direct:orders"))`; command `cargo test -p camel-core compiled_to_step_retains_declared_uri --lib`; expected: fails before the metadata implementation and passes after it.
- `compiled_processor_step_has_no_declared_uri`: arrange a `BuilderStep::Processor` with a no-op processor; act by compiling it; assert `CompiledStep::Process.to_uri` is `None`; command `cargo test -p camel-core compiled_processor_step_has_no_declared_uri --lib`; expected: fails before the field is wired and passes after it.

**Acceptance:**
- Every `CompiledStep::Process` constructor initializes or preserves `to_uri`.
- The URI is the authored URI before endpoint resolution/interception and is stored as shared immutable text.
- `cargo test -p camel-core --lib step_compilers` exits 0.
- `cargo fmt --check` exits 0.

- [x] 1.1

## camel-core tracing and metrics

### Task 2.1: Emit per-To step duration histograms

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified)
- `crates/camel-core/src/shared/observability/adapters/tracer.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_span_tests.rs` (modified)
- `crates/camel-core/src/shared/observability/adapters/tracer_tests.rs` (modified)

**Steps:**
1. Extend traced pipeline destructuring and reconstruction to preserve `to_uri`, and pass it into `TracingProcessor` using the existing `Arc<str>` ownership pattern.
2. Extend `record_step_metrics` with `to_uri`; when `include_duration` and `levers.durations_enabled()` are both true, call `record_histogram("step_duration_secs", duration.as_secs_f64(), &[("route", route_id), ("to_uri", uri)])`.
3. Keep exchange/error metrics and readiness behavior unchanged, and add the metric-label lint justification for the intentional declared-URI label.
4. Add recording-metrics tests for successful and failed call-time To steps, the `include_duration` gate, the `durations_enabled()` gate, and readiness failures.

**Tests:**
- `records_step_duration_with_route_and_uri_labels`: arrange recording metrics with durations enabled and a traced `To` processor labeled route `orders`; act by calling the processor; assert exactly one `step_duration_secs` histogram has labels `route=orders` and `to_uri=direct:orders`; command `cargo test -p camel-core records_step_duration_with_route_and_uri_labels --lib`; expected: fails before emission and passes after it.
- `records_step_duration_for_failed_call`: arrange recording metrics with durations enabled and a processor returning `CamelError`; act by calling it; assert one `step_duration_secs` histogram is recorded with the same labels; command `cargo test -p camel-core records_step_duration_for_failed_call --lib`; expected: fails before failed-call semantics are implemented and passes after it.
- `does_not_record_step_duration_when_include_duration_is_false`: arrange duration-enabled recording metrics and a readiness failure; act through `poll_ready`; assert zero `step_duration_secs` observations and unchanged exchange/error records; command `cargo test -p camel-core does_not_record_step_duration_when_include_duration_is_false --lib`; expected: fails before the call-time gate is wired and passes after it.
- `does_not_record_step_duration_when_duration_lever_is_disabled`: arrange a call-time To processor with the duration lever disabled; act by calling it; assert zero `step_duration_secs` observations and unchanged exchange/error records; command `cargo test -p camel-core does_not_record_step_duration_when_duration_lever_is_disabled --lib`; expected: fails before the duration lever gate is wired and passes after it.
- `readiness_failure_does_not_record_step_duration`: arrange a producer that returns an error from `poll_ready`; act by polling readiness; assert zero `step_duration_secs` observations and the existing readiness exchange/error records; command `cargo test -p camel-core readiness_failure_does_not_record_step_duration --lib`; expected: fails before readiness exclusion is tested and passes after it.

**Acceptance:**
- Call-time successful and failed To attempts emit the required histogram only when both gates are enabled.
- Readiness attempts never emit `step_duration_secs`.
- `cargo test -p camel-core --lib tracer` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- No `BENCH_LATENCY_FILE` tooling file is modified.

- [x] 2.1

## Documentation and integration verification

### Task 3.1: Verify OTEL metric wiring and architectural documentation

**Files:**
- `crates/camel-test/tests/metrics_wiring_test.rs` (modified)
- `docs/adr/0074-step-latency-otel-attribution.md` (modified)

**Steps:**
1. Extend the existing metrics wiring probe to assert the `step_duration_secs` family is exported by the configured collector path for a To route and has no observations for a processor-only route.
2. Update ADR-0074 from Proposed to Accepted and record the implemented test evidence and the call-time failed-attempt semantics.
3. Run the focused integration test and inspect the complete diff to confirm benchmark tooling remains untouched.

**Tests:**
- `otel_metrics_expose_step_duration_family`: arrange the existing `metrics_wiring_test` collector and a route containing `To("direct:orders")`; act by executing one exchange through the OTEL-enabled route; assert the exported metric family includes `step_duration_secs` with `route` and `to_uri` labels; command `cargo test -p camel-test --test metrics_wiring_test otel_metrics_expose_step_duration_family`; expected: fails before the family is emitted and passes after it is wired.
- `otel_metrics_omit_step_duration_for_processor_route`: arrange the same collector and a route containing only a processor; act by executing one exchange; assert no `step_duration_secs` data points are exported; command `cargo test -p camel-test --test metrics_wiring_test otel_metrics_omit_step_duration_for_processor_route`; expected: fails if non-To steps accidentally acquire URI labels and passes when the To-only contract is enforced.

**Acceptance:**
- The focused metrics wiring test passes and checks the public exporter-facing family.
- ADR-0074 is Accepted and matches the implemented declared-URI behavior.
- `git diff --name-only HEAD~1 -- crates/camel-cli/src/commands/bench_instrument.rs crates/camel-cli/src/commands/run.rs benchmarks` produces no changed benchmark-tool path.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core --no-deps` exits 0.

- [x] 3.1
