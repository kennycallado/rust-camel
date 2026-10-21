# Tasks: otel-route-exclusions

Single-phase change. Execute the five tasks in order. Every test, compile,
clippy, format, and lint battery below must run under containment:

```
systemd-run --user --scope --collect --unit=<unit> \
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill \
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test ...
systemctl --user stop <unit>
```

Confirm `pgrep -c compiled_artifa` is `0` before and after each contained
battery. Never use `/tmp` as `TMPDIR`.

## Task 1: Sampler and OtelConfig.exclude_routes (T1-T4)

**Files:**

- `crates/services/camel-otel/Cargo.toml`
- `crates/services/camel-otel/src/config.rs`
- `crates/services/camel-otel/src/service.rs`
- `crates/services/camel-otel/src/sampler_tests.rs`

**Steps:**

1. Add `glob = { workspace = true }` to `[dependencies]` in
   `crates/services/camel-otel/Cargo.toml`.
2. In `config.rs`, add `pub exclude_routes: Vec<String>` to `OtelConfig`,
   initialise it to `Vec::new()` in `new()`, add
   `pub fn with_exclude_routes(mut self, patterns: Vec<String>) -> Self`, and
   add `.field("exclude_routes", &self.exclude_routes)` to the manual
   `Debug` impl.
3. In `OtelConfig::validate()`, after the URL check, compile every pattern
   with `glob::Pattern::new(pattern)`. On error return
   `camel_api::CamelError::Config(format!("invalid [observability.otel]
   exclude_routes pattern '{}': {}", pattern, e))`.
4. In `service.rs`, add:
   `#[derive(Clone, Debug)] struct RouteExcludeSampler { matchers:
   Vec<glob::Pattern>, inner: Sampler }`.
   Implement `ShouldSample::should_sample` with the same six arguments as
   `LinkAwareSampler`. Scan `attributes` for a `KeyValue` whose key is
   `"route_id"` (`kv.key.as_str() == "route_id"`); if one is present and any matcher
   `matches(kv.value.as_str().as_ref())`, return
   `SamplingResult { decision: SamplingDecision::Drop,
   attributes: Vec::new(), trace_state }`, where `trace_state` is built the
   same way as in `LinkAwareSampler`:
   `parent_context.map(|cx| cx.span().span_context().trace_state().clone()).unwrap_or_default()`.
   Otherwise forward all six arguments to `self.inner.should_sample(...)`.
5. Change `LinkAwareSampler` to `struct LinkAwareSampler { inner:
   RouteExcludeSampler }`. Its no-link arm forwards the original six
   arguments to `self.inner.should_sample(...)`.
6. Replace `fn to_sdk_sampler(sampler: &OtelSampler) -> Sampler` with
   `fn to_sdk_sampler(config: &OtelConfig) -> Result<Sampler, CamelError>`.
   Add a private free function in `service.rs`:
   `fn compile_exclude_patterns(patterns: &[String]) ->
   Result<Vec<glob::Pattern>, CamelError>` using the same error text as step
   3. Build the root inner sampler with
   `let inner = match config.sampler { OtelSampler::AlwaysOn =>
   Sampler::AlwaysOn, OtelSampler::AlwaysOff => Sampler::AlwaysOff,
   OtelSampler::TraceIdRatioBased(ratio) =>
   Sampler::TraceIdRatioBased(ratio) };`, then
   `RouteExcludeSampler { matchers, inner }`, wrap it in
   `LinkAwareSampler`, then
   `Sampler::ParentBased(Box::new(...))`. Call it as
   `Self::to_sdk_sampler(&self.config)?` in `init_tracer_provider` (around
   line 361) and `start` (around line 470). No `unwrap`/`expect` in
   production paths.
7. In `sampler_tests.rs`, update every `to_sdk_sampler` call to the new
   signature by building
   `OtelConfig::new("http://localhost:4317", "svc")`, applying
   `.with_sampler(...)` and `.with_exclude_routes(...)`, and calling
   `OtelService::to_sdk_sampler(&config).expect("valid sampler config")`.
   Change the import to
   `use crate::config::{OtelConfig, OtelSampler};`, change
   `use opentelemetry::Context;` to `use opentelemetry::{Context, KeyValue};`,
   and remove
   `LinkAwareSampler` from the `super` import if it becomes unused.
   Update all existing tests that construct `LinkAwareSampler` directly
   (`link_aware_root_follows_sampled_link`,
   `link_aware_root_follows_unsampled_link`,
   `link_aware_root_without_links_delegates_inner`,
   `provider_mints_linked_root_with_parent_flag`) to use that seam.

**Tests:**

- `exclude_routes_drops_matching_root` (T1). Arrange:
  `OtelConfig::new("http://localhost:4317", "svc")
  .with_sampler(OtelSampler::AlwaysOn)
  .with_exclude_routes(vec!["skip-request*".into()])`, then
  `OtelService::to_sdk_sampler(&config).expect(...)`. Act:
  `should_sample(None, TraceId::from_hex("abcdefabcdefabcdefabcdefabcdefab").expect(...),
  "skip-request", &SpanKind::Internal,
  &[KeyValue::new("route_id", "skip-request")], &[])`. Assert:
  `SamplingDecision::Drop`.
- `exclude_routes_keeps_other_roots` (T2). Same arrange, act with
  `route_id = "orders-in"`. Assert:
  `SamplingDecision::RecordAndSample`.
- `sampled_parent_wins_over_exclusion` (T3). Arrange the same exclusion
  config with pattern `skip-*`; act with a remote `SpanContext` carrying
  `TraceFlags::SAMPLED` and creation attribute `route_id = "skip-me"` on a
  child span. Assert: `SamplingDecision::RecordAndSample`.
- `linked_split_item_follows_origin` (T4, required unsampled-link case).
  Arrange the same exclusion config with pattern `skip-*`; act with no
  parent, name `"item"`, creation attributes
  `&[KeyValue::new("route_id", "skip-me")]` (matching the exclusion), and
  `&[Link::new(link_target(false), Vec::new(), 0)]` using the existing
  `link_target` helper. Assert: `SamplingDecision::Drop`; the unsampled
  link must win before exclusion is consulted.
- `sampled_link_wins_over_exclusion` (T4 counterpart). Arrange the same
  exclusion config; act with no parent, name `"item"`, creation attributes
  `&[KeyValue::new("route_id", "skip-me")]` (matching the exclusion), and
  `&[Link::new(link_target(true), Vec::new(), 0)]`. Assert:
  `SamplingDecision::RecordAndSample`. An exclusion-first implementation
  fails this test because it would return `Drop`.
- `exclude_routes_empty_list_delegates_to_inner`. Arrange an `OtelConfig`
  with `AlwaysOff` and no patterns; act on a root with any `route_id`.
  Assert: `SamplingDecision::Drop`.
- `otel_config_rejects_invalid_exclude_pattern` (in `config.rs` tests).
  Arrange `with_exclude_routes(vec!["[unclosed".into()])`; act `validate()`;
  assert `Err(CamelError::Config(msg))` with `msg` containing `"[unclosed"`.
- `otel_config_accepts_valid_exclude_patterns` (in `config.rs` tests).
  Arrange `with_exclude_routes(vec!["orders-*".into(), "exact-route".into()])`;
  act `validate()`; assert `Ok(())`.

**Acceptance:**

- `systemd-run --user --scope --collect --unit=otel-task1
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-otel
  --lib` exits 0; T1-T4 and the empty-list test are green.
- `systemd-run --user --scope --collect --unit=otel-task1-fmt
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo fmt --check --all`
  exits 0.
- `systemd-run --user --scope --collect --unit=otel-task1-clippy
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo clippy -p
  camel-otel --all-targets -- -D warnings` exits 0.
- `systemd-run --user --scope --collect --unit=otel-golden
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-cli
  --test feature_profiles` exits 0. Regenerate the golden ONLY if the
  dependency closure genuinely changed, and record why.

- [x] 1

## Task 2: Camel config load validation and plumbing (T5)

**Files:**

- `crates/camel-config/src/config.rs`
- `crates/camel-config/src/config_tests/config_validation_tests.rs`
- `crates/camel-config/src/context_ext.rs`

**Steps:**

1. Add `#[serde(default)] pub exclude_routes: Vec<String>,` to
   `OtelCamelConfig` after `resource_attrs`, and add
   `exclude_routes: Vec::new()` to `impl Default for OtelCamelConfig`.
2. In `CamelConfig::validate()`, next to the existing
   `observability.otel.metrics_interval_ms` check, add: if
   `self.observability.otel` is `Some(otel)`, compile every
   `otel.exclude_routes` entry with `glob::Pattern::new`. On error return
   `CamelError::Config(format!("invalid [observability.otel] exclude_routes
   pattern '{}': {}", pattern, e))`. This block must run regardless of
   `otel.enabled`.
3. In `context_ext.rs`, chain
   `.with_exclude_routes(otel_cfg.exclude_routes.clone())` onto the
   `OtelConfig::new(...)` builder inside the `#[cfg(feature = "otel")]`
   block.
4. Do not modify any other config struct, route schema or DSL type.
5. Add `use std::io::Write;` inside the T5 test body only.

**Tests:**

- `invalid_glob_fails_config_load` (T5). Arrange: write a
  `tempfile::NamedTempFile` with exactly
  `[observability.otel]\nenabled = false\nexclude_routes = ["[unclosed"]\n`.
  Act: `CamelConfig::from_file(file.path().to_str().expect("utf-8 path"))`.
  Assert: the result is `Err(ConfigError::Message(msg))` and `msg` contains
  `invalid [observability.otel] exclude_routes pattern '[unclosed'`.
  The loader wraps the `CamelError::Config` returned by `validate()`; the
  next test pins that variant directly.
- `invalid_glob_fails_config_validation`. Arrange
  `CamelConfig { observability: ObservabilityConfig { otel:
  Some(OtelCamelConfig { exclude_routes: vec!["[unclosed".into()],
  ..Default::default() }), ..Default::default() }, ..CamelConfig::default() }`.
  Act: `config.validate()`. Assert: `Err(CamelError::Config(msg))` with
  `msg` containing `"[unclosed"`.
- `exclude_routes_default_empty`. Arrange `OtelCamelConfig::default()`.
  Assert: `exclude_routes.is_empty()`.

**Acceptance:**

- `systemd-run --user --scope --collect --unit=otel-task2
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-config`
  exits 0.
- The same command with `--features otel` exits 0.
- `systemd-run --user --scope --collect --unit=otel-task2-clippy
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo clippy -p
  camel-config --all-targets -- -D warnings` exits 0.

- [x] 2

## Task 3: Route exclusion integration contract (T6, R3)

**Files:**

- `crates/services/camel-otel/Cargo.toml`
- `crates/services/camel-otel/src/route_exclusion_tests.rs` (new)
- `crates/services/camel-otel/src/service.rs`

**Steps:**

1. Add dev-dependencies `camel-component-timer = { workspace = true }` and
   `camel-processor = { workspace = true }`.
2. In `service.rs`, next to the existing test module declarations, add
   `#[cfg(test)] #[path = "route_exclusion_tests.rs"] mod
   route_exclusion_tests;`.
3. In `route_exclusion_tests.rs`, define a five-method
   `RecordingCollector` implementing `camel_api::MetricsCollector`
   (`increment_exchanges`, `record_exchange_duration`, `increment_errors`,
   `set_queue_depth`, `record_circuit_breaker_change`) with named field
   `events: Mutex<Vec<String>>`. Add
   `impl RecordingCollector { fn snapshot(&self) -> Vec<String> {
   self.events.lock().expect("collector lock").clone() } }`. Record
   `"{route_id}:exchanges"`,
   `"{route_id}:duration"` and `"{route_id}:errors"`; do not implement
   `record_counter` (it is default-only).
   Imports: `use super::OtelService;`,
   `use crate::config::{OtelConfig, OtelSampler};`,
   `use camel_api::MetricsCollector;`,
   `use camel_builder::{RouteBuilder, StepAccumulator as _};`,
   `use camel_component_timer::TimerComponent;`,
   `use camel_core::CamelContext;`,
   `use opentelemetry::global;`,
   `use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider,
   SimpleSpanProcessor};`,
   `use std::sync::atomic::{AtomicUsize, Ordering};`,
   `use std::sync::{Arc, Mutex};`, `use std::time::Duration;`, and
   `use tokio::sync::Notify;`.
4. Define a test-only bounded observer instead of an inline sleep:
   `async fn wait_for_ticks(notify: &tokio::sync::Notify, ticks:
   &std::sync::atomic::AtomicUsize, target: usize)` that loops on
   `notify.notified().await` inside
   `tokio::time::timeout(Duration::from_secs(5), ...)`, ends with
   `.await.expect("timer route did not reach target ticks")`, and returns
   when `ticks.load(Ordering::SeqCst) >= target`. Timer steps call
   `notify_one()` after incrementing the counter.
5. Build the provider with
   `OtelService::to_sdk_sampler(&OtelConfig::new("http://localhost:4317",
   "t6").with_sampler(OtelSampler::AlwaysOn).with_exclude_routes(vec!["noisy-*".into()]))`,
   `.expect("valid sampler config")`,
   `SimpleSpanProcessor::new(exporter.clone())`, then
   `global::set_tracer_provider(provider.clone())`.
6. Build the context with
   `let mut ctx = CamelContext::builder().metrics(Arc::clone(&collector) as
   Arc<dyn camel_api::MetricsCollector>).build().await.expect("context
   builds")`, then
   `.with_tracer_config(camel_core::TracerConfig { enabled: true,
   ..Default::default() }).await`, then
   `ctx.register_component(camel_component_timer::TimerComponent::new())`.
7. Add two timer routes with explicit ids, using
   `RouteBuilder::from(...).route_id(...)`: `noisy-tick` from
   `timer:noisy?period=50` and `real-flow` from `timer:real?period=50`.
   Build each route into a `RouteDefinition` and register it with
   `ctx.add_route_definition(noisy_definition).await.expect("noisy route
   registers")` and
   `ctx.add_route_definition(real_definition).await.expect("real route
   registers")`.
   Create two local pairs, `noisy_notify`/`noisy_ticks` and
   `real_notify`/`real_ticks`, each `Arc<Notify>` and `Arc<AtomicUsize>`.
   Each route has one `.process(move |ex| { let ticks =
   Arc::clone(&ticks_step); let notify = Arc::clone(&notify_step); async
   move { ticks.fetch_add(1, Ordering::SeqCst); notify.notify_one();
   Ok(ex) } })` step; `.log` is not used in T6.
   Import `camel_builder::StepAccumulator as _;` for `.process`.

**Test:**

- `excluded_route_exports_no_spans_but_counts_metrics` (T6). Arrange the
  provider and two-route context above plus `RecordingCollector`. Act:
  `ctx.start().await`, `wait_for_ticks(noisy_notify, noisy_ticks, 3)`,
  `wait_for_ticks(real_notify, real_ticks, 3)`, `ctx.stop().await`,
  `provider.force_flush().expect("flush spans")`. Assert: (a) no exported span has a name starting
  with `noisy-tick` or a `route_id` attribute starting with `noisy-`; (b) at
  least one exported span is named `real-flow`; (c) `collector.snapshot()`
  contains `noisy-tick:exchanges`, `noisy-tick:duration`,
  `real-flow:exchanges` and `real-flow:duration`.
  Mark the test `#[tokio::test]` and `#[serial_test::serial]`.

**Acceptance:**

- `systemd-run --user --scope --collect --unit=otel-task3
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-otel
  --lib` exits 0.
- `systemd-run --user --scope --collect --unit=otel-task3-lint-sleep
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo xtask
  lint-test-sleep` exits 0 (no inline sleep was added).
- `systemd-run --user --scope --collect --unit=otel-task3-clippy
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo clippy -p
  camel-otel --all-targets -- -D warnings` exits 0.

- [x] 3

## Task 4: Bridge reality, comment correction and T7 negative control

**Files:**

- `crates/services/camel-otel/Cargo.toml`
- `crates/services/camel-otel/src/service.rs`
- `crates/services/camel-otel/src/route_exclusion_tests.rs`

**Steps:**

1. Add `tracing-opentelemetry = { workspace = true }` as a dev-dependency.
2. Extend the existing Task 3 imports with only the new Task 4/T7 types:
   `use camel_processor::LogLevel;`,
   `use opentelemetry::trace::TracerProvider;`,
   `use opentelemetry_sdk::trace::Sampler;`,
   `use tracing_subscriber::filter::LevelFilter;`, and
   `use tracing_subscriber::layer::{Layer, SubscriberExt};`.
   Do not repeat `CamelContext`, `RouteBuilder`, `StepAccumulator`,
   `TimerComponent`, `global`, the SDK trace types, `Arc`, `Mutex`,
   `Notify`, or `Duration`; Task 3 already imports them.
3. Correct ONLY the comment at
   `crates/services/camel-otel/src/service.rs:351` (the
   `init_tracer_provider` doc). Replace the false "picks up the global
   provider" claim with: the global tracer provider must exist before route
   spans are created because the engine mints spans through the global OTel
   tracer. Do not imply that this method initializes the logger. Also state that
   `tracing_opentelemetry::layer()` is constructed with a no-op tracer by
   default and does NOT read the global provider. Do not change any call,
   do not touch other comments, and do not add `.with_tracer` to production
   code.
4. In `route_exclusion_tests.rs`, define a local `FmtBuf` implementing
   `std::io::Write` and `tracing_subscriber::fmt::MakeWriter` over
   `Arc<Mutex<Vec<u8>>>`, mirroring the existing pattern in
   `crates/camel-processor/src/log.rs` tests.
5. T7 subscriber: `tracing_subscriber::registry()`
   `.with(tracing_opentelemetry::layer())`
   `.with(tracing_subscriber::fmt::layer().with_writer(fmt_buf.clone())
   .with_ansi(false).with_filter(LevelFilter::INFO))`. Do not call
   `.with_tracer` in T7.
6. T7 provider: real global `SdkTracerProvider` over
   `InMemorySpanExporter` + `SimpleSpanProcessor` with `AlwaysOn`, installed
   via `global::set_tracer_provider(provider.clone())`.
7. T7 execution: create
   `tokio::runtime::Builder::new_current_thread().enable_all().build()`, then
   call `tracing::subscriber::with_default(subscriber, || runtime.block_on(async { ... }))`
   so the subscriber is retained across route execution. Inside: build
   `let mut ctx = CamelContext::builder().build().await.expect("context
   builds")`, apply
   `camel_core::TracerConfig { enabled: true, ..Default::default() }` with
   `with_tracer_config(camel_core::TracerConfig { enabled: true,
   ..Default::default() }).await`, register `TimerComponent`, and build one
   route definition `let definition =
   RouteBuilder::from("timer:t7?period=50&repeatCount=1")
   .route_id("t7-normal").log("t7-fmt-marker", LogLevel::Info)
   .process(move |ex| { let notify = Arc::clone(&notify_step); async move {
   notify.notify_one(); Ok(ex) } }).build().expect("route builds")`.
   Define `let notify_step = Arc::clone(&notify);` before the builder.
   Register it with
   `ctx.add_route_definition(definition).await.expect("route registers")`.
   Start, await
   `tokio::time::timeout(Duration::from_secs(5),
   notify.notified()).await.expect("t7 route did not fire")`, stop.
   Still inside the same subscriber scope after `ctx.stop().await`, manually
   emit and drop exactly four spans:
   `drop(tracing::debug_span!(target:
   "camel_core::lifecycle::adapters::route_compiler", "pipeline_step",
   index = 0))`,
   `drop(tracing::debug_span!(target:
   "camel_core::lifecycle::adapters::route_compiler", "error_handler",
   step_index = 0))`,
   `drop(tracing::info_span!(target: "camel_function", "function"))`, and
   `drop(tracing::info_span!(target: "camel_tracer", "step"))`. After the
   scope closes, call `provider.force_flush().expect("flush spans")`.
8. Optional test-only control: add
   `real_tracer_bridge_exports_engine_twins` using a LOCAL provider (never
   global) and
   `tracing_opentelemetry::layer().with_tracer(local_provider.tracer("control"))`
   inside `tracing::subscriber::with_default`. Emit
   `debug_span!(target: "camel_core::lifecycle::adapters::route_compiler",
   "pipeline_step", index = 0)`,
   `debug_span!(target:
   "camel_core::lifecycle::adapters::route_compiler", "error_handler",
   step_index = 0)`,
   `info_span!(target: "camel_tracer", "step")` and
   `info_span!(target: "camel_function", "function")`; drop each span, call
   `provider.force_flush().expect("flush spans")`, and assert all four are
   exported. This test-only wiring must not be used in production.
   Import `opentelemetry::trace::TracerProvider` for `.tracer(...)`.
9. No `bridge_span_filter`, `is_engine_span_target` or `ENGINE_SPAN_TARGETS`
   symbol is added anywhere.

**Tests:**

- `engine_spans_are_not_orphan_roots` (T7). Arrange the global provider,
  scoped subscriber and `FmtBuf` above. Act one timer exchange with
  `repeatCount=1`, then emit/drop the four engine-target spans under the
  same subscriber scope, then
  `provider.force_flush().expect("flush spans")`. Assert: (a) the exported
  spans in the `t7-normal` trace have exactly the names `t7-normal`,
  `t7-normal:log` and `t7-normal:step-1`; (b) no exported span is named
  `pipeline_step`,
  `error_handler`, `function` or `step`; (c) the `FmtBuf` string contains
  `t7-fmt-marker`.
  Mark the test `#[test]` and `#[serial_test::serial]`; it uses a manual
  current-thread runtime, not `#[tokio::test]`. T6 and T7 both use the
  unkeyed `serial_test` attribute (the shared default serial key), so their
  global tracer-provider writes cannot interleave.
- `real_tracer_bridge_exports_engine_twins` (control, optional but included
  here). Arrange a local provider and explicit `.with_tracer`. Act emit and
  drop all four engine-target spans (`pipeline_step`, `error_handler`,
  `function`, `step`), then
  `provider.force_flush().expect("flush spans")`. Assert all four are
  exported, proving why production leaves the bridge on its no-op tracer.

**Acceptance:**

- `systemd-run --user --scope --collect --unit=otel-task4
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-otel
  --lib` exits 0.
- `systemd-run --user --scope --collect --unit=otel-task4-clippy
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo clippy -p
  camel-otel --all-targets -- -D warnings` exits 0.

- [x] 4

## Task 5: Tracing and configuration docs (R6)

**Files:**

- `docs/src/services/tracing.md`
- `docs/src/configuration/schema.md`

**Steps:**

1. Add a section "Excluding routes from tracing" to
   `docs/src/services/tracing.md`, after the operational invariants. Include
   this TOML example:

   ```toml
   [observability.otel]
   enabled = true
   endpoint = "http://localhost:4317"
   exclude_routes = ["*-inbox-worker", "health-*"]
   ```

   State that patterns are globs, matched case-sensitively against the full
   route id. When a matching route starts a new trace, its root and child
   spans are not exported. Metrics remain unchanged. Invalid patterns fail
   config load with an error that names the pattern, even when `enabled = false`.
2. Document parent-based sampling explicitly: a sampled inbound parent or
   sampled link still wins over exclusion. Child routes inherit the parent
   sampling decision. Do not describe exclusion as an unconditional export filter.
3. In `docs/src/configuration/schema.md`, add this row to the
   `[observability.otel]` table:
   | `exclude_routes` | `string[]` | `[]` | Glob patterns (case-sensitive, full match on route id). A matching route that starts a new trace keeps metrics but exports no spans; a sampled inbound parent or sampled link still wins. Invalid patterns fail config load. |
4. Do not create `CHANGELOG.md`. R6's changelog entry is carried by generated
   release notes (design D1).

**Test:**

- `systemd-run --user --scope --collect --unit=otel-docs
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 mdbook test` run from
  `docs/`. Act: build the guide tests. Assert: exit code 0.

**Acceptance:**

- The contained `mdbook test` above exits 0.
- No `CHANGELOG.md` exists in the repository root.

- [x] 5
