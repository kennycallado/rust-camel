# Tasks: dashboard-observability

All paths repo-relative to the worktree root. Every task's Tests block is
the contract: write EXACTLY those tests first (RED), then implement until
green. Production `.expect(`/`.unwrap(` need `// allow-unwrap`.

## Phase 1: Error semantics (D1+D2, rc-hrm1.4 + rc-hrm1.5)

### Task 1.1: Retry/breaker trait methods with composite fan-out

- **Files**: `crates/camel-api/src/metrics.rs` (modified),
  `crates/services/camel-prometheus/src/metrics.rs` (modified)
- **Steps**:
  1. Add default (no-op) methods to `MetricsCollector`:
     `fn increment_retry_attempt(&self, scheme: &str, operation: &str)`,
     `fn increment_circuit_breaker_rejection(&self, route: &str)`.
  2. Delegate both in `CompositeMetricsCollector` (extend the existing
     delegation list and ADD a delegation-parity test — none exists today).
  3. In `crates/services/camel-prometheus/src/metrics.rs` register
     `camel_retry_attempts_total` (IntCounterVec, labels `operation`,
     `scheme`) and `camel_circuit_breaker_rejections_total`
     (IntCounterVec, label `route`) next to the existing
     `camel_queue_depth` GaugeVec (:72, :145); implement both methods;
     export them in the `/metrics` render.
- **Tests** (in `crates/camel-api/src/metrics.rs` `#[cfg(test)]` and
  `crates/services/camel-prometheus` unit tests):
  - `composite_delegates_retry_and_rejection` — setup: composite of two
    recording collectors (existing test-double pattern in metrics.rs);
    action: `increment_retry_attempt("kafka","connect")` and
    `increment_circuit_breaker_rejection("r1")`; assert: both members
    observed both calls.
  - `noop_defaults_compile_and_do_nothing` — setup: `NoOpMetrics`; action:
    call both new methods; assert: no panic (compile-level proof).
    command: `cargo test -p camel-api --lib`; expected RED before step 1-2.
  - `prometheus_registers_new_series` — setup: PrometheusMetrics built
    as existing tests do; action: increment both new counters; assert:
    rendered body contains `camel_retry_attempts_total{operation="connect",scheme="kafka"} 1`
    and `camel_circuit_breaker_rejections_total{route="r1"} 1`
    (prometheus text format renders labels alphabetically — assert the
    sorted-order substrings).
    command: `cargo test -p camel-prometheus --lib`; expected RED before
    step 3.
- **Acceptance**: `cargo test -p camel-api --lib` green; `cargo test -p
  camel-prometheus --lib` green; `cargo clippy -p camel-api -p
  camel-prometheus -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: Retry helpers count attempts, error once on exhaustion

- **Files**: `crates/components/camel-component-api/src/network_retry.rs`
  (modified), `crates/components/camel-component-api/src/network_retry_tests.rs`
  (modified — APPEND; file already exists, wired via `#[cfg(test)]
  #[path]` at network_retry.rs:360-362 — do not overwrite existing
  tests), every helper-adopter crate with
  an Err-arm `increment_errors` (enumerate via
  `rg -l 'retry_async(_cancelable)?\(' crates | grep -v test`; known:
  `crates/components/camel-container/src/lib.rs` (:1545 call, :1558-1562
  and :1632/:1649 double-count Err arms), jms/sql/cxf/grpc/master/
  xslt/xj/surrealdb-pool_factory adopters as found) (modified)
- **Steps**:
  1. Change `retry_async` and `retry_async_cancelable` signatures: drop
     `label: Option<&'static str>`, add params in order `scheme:
     &'static str, operation: &'static str, metrics: Option<&dyn
     MetricsCollector>` (camel-component-api already re-exports
     camel-api items — re-export the trait; reword the inner retry-log
     lines from the old label to scheme/operation). Mechanically update
     ALL call sites: the old label literal becomes `operation`, the
     component name becomes `scheme`.
  2. In `retry_async_inner`: on EVERY attempt (first included) call
     `metrics.increment_retry_attempt(scheme, operation)`.
  3. In the final-failure arm (attempts exhausted, or non-retryable
     first-fail — NOT cancellation: cancelled sequences are clean
     shutdowns, container :1555/:1642 treats `Err if is_cancelled()` as
     clean today, and manufacturing errors on graceful shutdown would
     false-fire alerts): call exactly ONCE
     `metrics.increment_errors(operation, &format!("e:{scheme}:{operation}"))`
     — first arg is the operation label (helper has no route scope;
     ADR-0012 component labels already use this shape), error_type
     follows the container's established `e:{scheme}:{operation}`
     pattern — then return the error.
  4. ADOPTER SWEEP (the actual bug class): for EVERY `retry_async*` call
     site, inspect its Err arm; if the arm still calls `increment_errors`
     for the same failure (camel-container :1558-1562, :1632-:1649
     pattern), REMOVE that call — the helper now owns exhaustion
     accounting. Keep any DISTINCT error classifications that name a
     different, more specific failure.
  5. KAFKA (manual loop, NOT an adopter): keep all six terminal
     increment sites (:304, :318, :334, :378, :507, :524 — one-shot
     failures); ADD one `increment_errors(&route_id,
     "e:kafka:recv-exhaustion")` in the recv-exhaustion arm (:574-577,
     currently log-only) — `route_id` IS in scope there (run_consumer_loop
     param :406, used at :591), so pass it, not a literal.
- **Tests** (new `network_retry_tests.rs`, recording-collector double):
  - `attempts_counted_per_try` — setup: policy max_attempts=3, op fails
    twice then succeeds; action: `retry_async(...)` with
    scheme="t", operation="op"; assert: 3 `increment_retry_attempt("t","op")`
    calls, 0 errors.
  - `exhaustion_errors_once` — setup: policy max_attempts=5, op always
    fails; assert: 5 attempts recorded, exactly 1
    `increment_errors("op", "e:t:op")`.
  - `non_retryable_errors_once_first_attempt` — setup:
    is_retryable=false; assert: 1 attempt, 1 error.
  - `cancelled_mid_sequence_no_error` — setup: cancelable variant,
    cancel token fired after first attempt, op pending; assert: 0
    `increment_errors` (cancellation is clean shutdown, not failure),
    attempts recorded up to cancellation.
  - `none_metrics_is_fine` — setup: metrics=None; action: same failing
    op; assert: error returned unchanged, no panic.
    command: `cargo test -p camel-component-api`; expected RED before
    steps 2-3.
- **Acceptance**: `cargo test -p camel-component-api` green; `cargo
  clippy -p camel-component-api --all-targets -- -D warnings` exits 0;
  workspace sweep proof: `rg -n 'increment_errors' $(rg -l
  'retry_async(_cancelable)?\(' crates --type rust | xargs)` shows no
  Err-arm increment on a failure the helper retries (container sites
  gone).

- [x] 1.2

### Task 1.3: Breaker rejections recorded, excluded from errors_total

- **Files**: `crates/camel-processor/src/circuit_breaker.rs` (modified),
  `crates/camel-core/src/lifecycle/adapters/route_compiler_ext.rs`
  (modified — BOTH construction sites: :236 Gate/new path, :260
  Layer/old path), `crates/camel-processor/benches/circuit_breaker.rs` (modified —
  :50/:61/:88/:117 call sites of the changing constructor),
  `crates/camel-core/src/lifecycle/adapters/route_compiler_tests.rs`
  (modified — :530/:560/:590/:620 Gate constructions),
  `crates/camel-core/src/lifecycle/adapters/continued_e2e.rs` (modified
  — :202 Gate construction; verify path at execution),
  `crates/camel-core/src/shared/observability/adapters/tracer.rs`
  (modified), `crates/camel-core/src/shared/observability/adapters/tracer_tests.rs`
  (new or nearest existing sibling-test file per repo convention)
- **Steps**:
  1. `CircuitBreakerLayer::new(config)` (circuit_breaker.rs:38-46) gains
     `route_id: Arc<str>` and `metrics: Option<Arc<dyn
     MetricsCollector>>` params (camel-processor already depends on
     camel-api; note the in-scope type at the construction site is the
     trait object, NOT MetricsHandle). `CircuitBreakerGate::new`
     (:257) gains the SAME two params — it is the second breaker path
     (new RouteChannelService route, taken whenever an error handler is
     configured) with its own fast-fail Reject arms (:286, :296).
     Store both in each type.
  2. Production threading at BOTH sites in route_compiler_ext.rs: at
     :260 (Layer, old path) `tracer_metrics` is MOVED BY VALUE into
     `compose_traced_pipeline_with_contracts` at :254 BEFORE the Layer
     construction — clone the Arc first (pattern at :195) and pass the
     clone; `route_id` (:179-:182 fn params, type &str) needs
     `Arc::from(route_id)` (pattern :156). At :236 (Gate, new path —
     `circuit_breaker.map(CircuitBreakerGate::new)`) thread the same
     cloned values. Update benches and test constructions mechanically.
  3. At ALL fast-fail sites — Layer/Service :121, :134 (poll_ready),
     :170 (call); Gate :286, :296 (before_call Reject arms): before
     returning `CamelError::CircuitOpen`, call
     `metrics.increment_circuit_breaker_rejection(&route_id)` when
     metrics is Some. BOTH paths must count — the tracer skip (step 4)
     is global, so a rejection counted nowhere is a spec violation.
  4. In `tracer.rs` (~:235) where `error_type` drives `increment_errors`:
     skip the increment when the classification equals the circuit-open
     marker — add a shared named const `CIRCUIT_OPEN` in camel-api next
     to `CamelError::classify` (error.rs:183 uses the literal once;
     tracer.rs imports the const) — do not inline the literal twice.
  5. Keep `CamelError::CircuitOpen` classification and caller-visible
     behavior unchanged.
- **Tests**:
  - `rejection_counted_not_errored` (tracer_tests.rs): setup: recording
    collector + tracer adapter; action: record an exchange failure whose
    error classifies `circuit_open`; assert: 1
    `increment_circuit_breaker_rejection`, 0 `increment_errors`.
  - `classification_unchanged_for_callers` (camel-api unit, error.rs):
    `CamelError::CircuitOpen.classify() == "circuit_open"` — pins the
    contract the tracer skip relies on.
  - `open_breaker_end_to_end` (new `crates/camel-test/tests/
    error_semantics_test.rs`): setup: route with circuit-breaker EIP
    (threshold small) + failing downstream
    `to:direct:missing?failIfNoConsumers=false` (call-time failure —
    poll_ready aborts record nothing); drive failures until breaker
    opens; send more exchanges; action: scrape `/metrics`; assert:
    `camel_circuit_breaker_rejections_total{route=...}` > 0 and
    `camel_errors_total` did NOT grow during the open-phase sends.
    command: `cargo test -p camel-test --test error_semantics_test`;
    expected RED before steps 1-4.
- **Acceptance**: both test commands green; `cargo clippy -p
  camel-processor -p camel-core -- -D warnings` exits 0.

- [x] 1.3

## Phase 2: Metrics levers (D3, rc-hrm1.1)

### Task 2.1: [observability.metrics] levers, non-disableable errors, tracer decoupling

- **Files**: `crates/camel-core/src/shared/observability/domain/config.rs`
  (modified), `crates/camel-config/src/context_ext.rs` (modified),
  `crates/camel-core/src/shared/observability/adapters/tracer.rs`
  (modified), `docs/src/configuration/schema.md` (modified),
  camel-config inline tests (modified)
- **Steps**:
  1. OPEN WITH DECOMPOSITION (inter-phase review finding): camel-prometheus
     metrics.rs is at 971 lines — extract the metric-family
     registration/render into a submodule BEFORE appending levers work,
     keeping the public surface unchanged (pure move commit).
  2. New `MetricsLeversConfig { enabled: bool, exchange: bool, duration:
     bool, components: bool }` in the observability domain config, serde
     via the Raw-intermediate + `Option<bool>` explicit-set technique
     (pattern: TracerConfig in the same file). Defaults: enabled=true,
     exchange=true, duration=true, components=false. No field exists for
     errors — structurally non-disableable.
  3. Parse `[observability.metrics]` in the effective-config assembly
     (context_ext.rs, next to `[observability.tracer]` handling at
     :612-636); store levers on the built context alongside other
     observability state.
  4. DECOUPLE pipeline enablement from spans (spec Req 1): today
     explicit `tracer.enabled=false` disables the whole pipeline
     (context_ext.rs:613-627) and kills metrics export. Change so
     explicit tracing-off gates SPAN creation only; the pipeline
     (adapter) still runs and emits metric families when prometheus is
     on. Pipeline-off happens ONLY via `[observability.metrics]
     enabled=false` — and even then the error family still flows (step
     4's never-gate rule + spec Req 2).
  5. Gate the pipeline families: before `increment_exchanges` consult
     `levers.exchange`; before `record_exchange_duration` consult
     `levers.duration`; NEVER gate `increment_errors` by any lever.
  6. Document the table in `docs/src/configuration/schema.md` (keys,
     defaults, error-family non-disableable note, tracer-decoupling
     semantics) following the existing `[observability.tracer]` entry
     format.
- **Tests**:
  - `levers_parse_and_default` — setup: TOML with only
    `[observability.metrics]`; assert: enabled=true, components=false
    defaults; explicit `duration = false` parses false.
  - `absent_table_means_defaults` — no table: levers = all-default.
  - `unknown_key_rejected` — `[observability.metrics] bogus = true`
    fails validation (deny-unknown-keys consistent with sibling tables).
  - `metrics_on_tracer_off` (spec Req 1 scenario): tracer.enabled=false,
    metrics enabled, prometheus on; run one exchange; assert: metric
    families exported, no spans created (assert via the tracer adapter
    test double: 0 span records, 1+ metric records).
  - `metrics_off_tracer_on` (spec Req 1 scenario): metrics.enabled=false,
    tracer on; assert: spans created; `camel_errors_total` still exported
    (inject one failure), `camel_exchanges_total` not.
  - `duration_family_disabled_but_errors_survive` (tracer adapter unit):
    levers duration=false; record a success and a failure; assert: 0
    `record_exchange_duration`, 1 `increment_errors`, exchanges per
    lever.
  - `schema_check_passes` — command: `cargo xtask schema --check`; assert
    exit 0.
    commands: `cargo test -p camel-config --lib`, `cargo test -p
    camel-core --lib`, `cargo xtask schema --check`; expected RED before
    steps 1-4 (decoupling + family tests).
- **Acceptance**: all listed commands exit 0; `rg -n 'observability.metrics'
  docs/src/configuration/schema.md` documents all four keys.

- [x] 2.1

## Phase 3: Inventory + backpressure emissions (D4, rc-hrm1.6 + rc-hrm1.7)

### Task 3.1: camel_route_state gauge from RouteStatusProjection

- **Files**: `crates/camel-api/src/metrics.rs` (modified),
  `crates/services/camel-prometheus/src/metrics.rs` (modified),
  route-state publication site in camel-core (locate via `rg -l
  RouteStatusProjection crates/camel-core/src` — expected
  lifecycle/application area) (modified)
- **Steps**:
  1. Trait method `fn set_route_state(&self, route: &str, state: &str)`
     (default no-op; Composite delegates).
  2. Prometheus: IntGaugeVec `camel_route_state{route,state}`;
     implementation keeps a per-route last-state map so each call sets
     the new state to 1 and the previous to 0 (callers stay one-arg).
  3. At the point `RouteStatusProjection` transitions are published, call
     `set_route_state(route_id, new_state)`. States are the projection
     enum's Debug names — closed set by construction.
- **Tests**:
  - `route_state_gauge_transitions` (camel-prometheus unit): set
    Starting then Running for route r1; assert body has
    `camel_route_state{route="r1",state="Running"} 1` and the Starting
    series 0.
  - `route_starts_end_to_end` (`crates/camel-test/tests/
    inventory_metrics_test.rs`, new): start an app with one route +
    prometheus; scrape; assert `camel_route_state{route=...,state="Running"} 1`
    (or the startup-complete state the projection uses — match the
    enum).
    command: `cargo test -p camel-test --test inventory_metrics_test`;
    expected RED before steps 1-3.
- **Acceptance**: commands green; `cargo clippy -p camel-api -p
  camel-prometheus -p camel-core -- -D warnings` exits 0.

- [x] 3.1

### Task 3.2: camel_build_info and camel_uptime_seconds

- **Files**: `crates/camel-api/src/metrics.rs` (modified),
  `crates/services/camel-prometheus/src/metrics.rs` (modified),
  `crates/camel-core/src/context_builder.rs` (modified)
- **Steps**:
  1. Trait methods `fn record_build_info(&self, version: &str, git_sha:
     &str)` and `fn record_uptime(&self, seconds: f64)` (defaults no-op;
     Composite delegates).
  2. Prometheus: `camel_build_info{git_sha,version}` IntGaugeVec set to
     1 (text format renders labels alphabetically); `camel_uptime_seconds`
     Gauge set to the given value.
  3. In `context_builder.rs` `build()` (after MetricsHandle creation at
     :213): record build info — `version` from
     `env!("CARGO_PKG_VERSION")` of camel-core, `git_sha` from
     `option_env!("VERGEN_GIT_SHA").unwrap_or("unknown")` (accepted
     trade-off: camel-core has no vergen build script; optional build.rs
     is a follow-up, NOT this change).
  4. Uptime refresh: on the existing service tick that reaches the
     collector (locate the periodic path camel-prometheus already uses);
     if none reaches it, `tokio::spawn` a 60s-interval refresh task at
     build, aborted with the context's shutdown token (follow existing
     spawned-service patterns in context_builder.rs).
- **Tests**:
  - `build_info_and_uptime_rendered` (camel-prometheus unit): record
    ("1.2.3","abc1234") and uptime 0.5; assert body contains
    `camel_build_info{git_sha="abc1234",version="1.2.3"} 1` and
    `camel_uptime_seconds 0.5`.
  - `fresh_scrape_shows_restart` (inventory_metrics_test.rs): build
    context, scrape; assert `camel_uptime_seconds` parses < 120
    (determinism pin) and `camel_build_info` present.
    command: `cargo test -p camel-test --test inventory_metrics_test`;
    expected RED before steps 1-3.
- **Acceptance**: commands green; no `unwrap()` without `// allow-unwrap`.

- [x] 3.2

### Task 3.3: set_queue_depth wired in SEDA, aggregator, resequencer

- **Files**: `crates/camel-api/src/metrics.rs` (modified — param
  rename), `crates/services/camel-prometheus/src/metrics.rs` (modified —
  label rename), `crates/components/camel-component-seda/src/lib.rs`
  (modified), `crates/camel-processor/src/aggregator.rs` (modified),
  `crates/camel-processor/src/resequencer/` buffer-holding modules
  (mod/batch/stream — locate the buffer owners) (modified)
- **Steps**:
  1. RENAME (zero production callers today): trait param
     `set_queue_depth(route_id, depth)` → `set_queue_depth(queue, depth)`
     and the registered GaugeVec label `route` → `queue` (spec says
     `{queue}`). Update all trait implementors (NoOp, Composite,
     prometheus, otel, test doubles) and the existing registration sites
     (:72, :145).
  2. SEDA consumer: on its existing poll/tick loop, report
     `set_queue_depth("seda:<endpoint-name>", queue.len())`.
  3. Aggregator: on its maintenance pass, report buffered group count as
     `set_queue_depth("aggregator:<correlation-or-route>", n)`.
  4. Resequencer: report buffered awaiting-sequence size as
     `set_queue_depth("resequencer:<route>", n)` on its maintenance pass.
  5. Queue label values are component-declared closed sets (endpoint
     names / route ids — bounded identifiers, acceptable per D6).
- **Tests** (`crates/camel-test/tests/inventory_metrics_test.rs`,
  appended):
  - `seda_backlog_visible` — setup: SEDA route whose consumer is blocked
    (a processor that parks until a oneshot fires — blocked-consumer
    backlog, NOT a speed race); enqueue N > 0; action: allow one poll
    tick, scrape; assert `camel_queue_depth` series for the seda queue
    > 0; then release the consumer, await drain with one extra sampling
    tick of tolerance, scrape; assert the series is 0.
  - `aggregator_buffer_visible` — partial group in flight (missing
    closing message); assert queue_depth > 0 for the aggregator label;
    complete the group; await drain (one tick tolerance); assert 0.
  - `resequencer_buffer_visible` — out-of-order batches with the gap
    sequence missing; assert queue_depth > 0 for the resequencer label;
    deliver the gap; await drain (one tick tolerance); assert 0.
    command: `cargo test -p camel-test --test inventory_metrics_test`;
    expected RED before steps 2-4.
- **Acceptance**: command green; `rg -n 'set_queue_depth'
  crates/components/camel-component-seda crates/camel-processor` shows
  the three production call sites.

- [x] 3.3

## Phase 4: Component emission sweep (D5, rc-6s6h + rc-q25t + rc-bfnw)

### Task 4.1: Uniform component operations family + lever facade

- **Files**: `crates/camel-api/src/metrics.rs` (modified),
  `crates/camel-api/src/component_metrics.rs` (new),
  `crates/services/camel-prometheus/src/metrics.rs` (modified),
  camel-core context surface exposing the facade to components
  (`crates/camel-core/src/context.rs` or the RuntimeObservability
  acquisition path components already use — locate, then modify)
- **Steps**:
  1. Trait method `fn record_component_operation(&self, component: &str,
     operation: &str, outcome: &str)` (default no-op; Composite
     delegates).
  2. Prometheus: `camel_component_operations_total{component,operation,outcome}`
     IntCounterVec; outcome values "success"/"failure" only (document in
     the collector).
  3. New `ComponentMetrics` facade in `crates/camel-api/src/component_metrics.rs`:
     wraps `Arc<MetricsHandle>` + a `components_enabled: bool` PRIMITIVE
     (snapshot taken at construction in camel-core, where
     `MetricsLeversConfig` lives — camel-api cannot depend on
     camel-core, so the levers type stays out of the facade). Method
     `observe(&self, component: &str, operation: &str, failed: bool)`:
     emits `record_component_operation(..., "failure"|"success")` only
     when `components_enabled`; ALWAYS forwards failures to
     `increment_errors(component, &format!("e:{component}:{operation}"))`
     (error family never lever-gated).
  4. Expose the facade where components acquire observability today
     (RuntimeObservability path); construction site snapshots the
     current levers.
- **Tests**:
  - `facade_gates_components_not_errors` (camel-api unit): enabled=false;
    observe(failed=true); assert: 1 error, 0 component-op. enabled=true;
    observe failed=false then true; assert: ops recorded with both
    outcomes AND the failure still increments errors.
  - `prometheus_renders_family` (camel-prometheus unit): two observes;
    assert body contains
    `camel_component_operations_total{component="redis",operation="command",outcome="success"} 1`.
    commands: `cargo test -p camel-api --lib`, `cargo test -p
    camel-prometheus --lib`; expected RED before steps 1-3.
- **Acceptance**: commands green; clippy on camel-api + camel-prometheus
  clean.

- [x] 4.1

### Task 4.2: Dead-observability sweep — wasm, opensearch, seda, surrealdb, cxf

- **Files**: `crates/components/camel-component-wasm/src/producer.rs`
  (modified), `crates/components/camel-opensearch/src/producer/mod.rs`
  (modified), `crates/components/camel-component-seda/src/lib.rs`
  (modified), `crates/components/camel-component-surrealdb/src/consumer.rs`
  (modified), `crates/components/camel-cxf/src/consumer.rs` (modified)
- **Steps** (per component, via the Task 4.1 facade; replace stale
  "Phase B" doc comments with the real call):
  1. wasm producer (:85 observability field dead): observe
     `("wasm","invoke", failed)` around the component invocation.
  2. opensearch producer (:52 runtime field dead): observe
     `("opensearch", <the request-dispatch entrypoint operation>, failed)`
     at the operation boundary.
  3. seda consumer/producer (~:505-586, ~:768-771): observe
     `("seda","consume"|"produce", failed)`; drop the stale Phase B
     comments.
  4. surrealdb remainder (PARTIAL — keep the existing
     `b-prime:surrealdb:notification` emission at :192-194): observe
     `("surrealdb","query", failed)` on the remaining principal op.
  5. cxf remainder (keep `b-prime:cxf:response-marshalling` at :310):
     observe `("cxf","consume", failed)` at the consumer entrypoint.
- **Tests** (`crates/camel-test/tests/component_emission_test.rs`, new):
  - `dead_components_now_emit` — parameterized over the sweepable
    components with an existing camel-test harness (mock-driven where
    real infra is absent; `#[ignore]` + reason for ones needing
    wasm/opensearch/surrealdb backends — enumerate deferrable ones in
    the ignore list, marked integration-verification-deferred-to-CI);
    for each drivable component: drive one success (lever on) and one
    failure (lever off); assert component-op success series present
    with lever on; error family present with lever off.
  - `audit_table_complete` — a `const AUDIT: &[(&str,&str)]` listing
    component/op for all five — cross-checked by the test driving the
    mock-backed subset.
    command: `cargo test -p camel-test --test component_emission_test`;
    expected RED before steps 1-5.
- **Acceptance**: command green (or explicitly `#[ignore]`d entries
  documented); `rg -n 'Phase B'
  crates/components/camel-component-seda` returns 0 hits.

- [x] 4.2

### Task 4.3: kafka/redis/http success paths + scheme alignment

- **Files**: `crates/components/camel-kafka/src/consumer.rs` + producer
  path (modified), `crates/components/camel-redis/src/` consumer +
  producer (modified), `crates/components/camel-http/src/` (modified)
- **Steps**:
  1. Each component: adopt the facade at its principal operation
     boundary — kafka `("kafka","consume"|"produce")`, redis
     `("redis","command")`, http `("http","request")`.
  2. Align retry scheme labels (Task 1.2) with these operation names:
     `camel_retry_attempts_total{scheme,operation}` uses scheme =
     component name, operation = principal op (kafka's Task-1.2
     recv-exhaustion error label stays `"e:kafka:recv-exhaustion"` —
     recv is a consume-leg operation; note it in the const table).
  3. Do not remove existing failure-specific emissions (redis consumer
     :297-444 etc.) — they layer on top.
- **Tests** (append to component_emission_test.rs; kafka legs `#[ignore]`
  if no broker — the repo's kafka test posture governs):
  - `redis_success_and_failure_observed` — per the repo's existing redis
    test harness; drive both outcomes; assert outcome="success"/
    "failure" series.
  - `http_roundtrip_observed` — in-repo http test server (existing
    pattern); drive 200 and 5xx; assert series for both outcomes.
  - `retry_scheme_matches_operation_set` — unit: the scheme/operation
    literal sets used in retry calls are members of the component op
    sets (compile-time const comparison in the test).
    command: `cargo test -p camel-test --test component_emission_test`;
    expected RED before steps 1-2.
- **Acceptance**: command green (ignores documented); clippy on the
  three component crates clean.

- [x] 4.3

## Phase 5: Closure (D6+D7, rc-hrm1.8 + rc-hrm1.9)

### Task 5.1: xtask lint-metric-labels — closed label sets

- **Files**: `scripts/xtask/src/main.rs` (modified — new lint subcommand
  following lint-context-citations structure), `AGENTS.md` (modified —
  add gate entry), `.github/workflows/ci.yml` if the gate list is
  mirrored there (modified)
- **Steps**:
  1. New subcommand `lint-metric-labels`: walk crates/ for calls to
     `record_counter`, `record_histogram`, `record_component_operation`,
     and `increment_retry_attempt`; the LABEL VALUE arguments must be
     (a) string literals, or (b) BEST-EFFORT recognized
     `OptionKind::Enum`-derived expressions (ADR-0041 uri_options path
     heuristic — when undecidable, default-DENY), or (c) annotated
     `// allow-open-label <bd-ref>`.
  2. Emit violations as `file:line: label value not provably closed`
     and exit 1 when any exist.
  3. Run workspace-wide; fix or annotate ALL hits (annotations must cite
     a bd id; create follow-up bds if a hit is out-of-scope to fix).
  4. Add the gate to AGENTS.md `## QUALITY GATES` after
     lint-context-citations.
- **Tests**:
  - `lint_passes_clean` — command: `cargo xtask lint-metric-labels`;
    assert exit 0 after step 3.
  - `lint_catches_raw_label` — xtask unit test with a synthetic snippet
    string (not a repo file): feed the detector a call with a
    `format!`-built label; assert violation reported.
    command: `cargo test -p xtask && cargo xtask
    lint-metric-labels`; expected RED before step 1.
- **Acceptance**: both commands exit 0; `rg -n 'allow-open-label'
  crates` hits all carry a bd reference.

- [x] 5.1

### Task 5.2: ADR-0066 collector binding and lifetime contract

- **Files**: `docs/adr/0066-metrics-collector-binding-and-lifetime.md`
  (new), `docs/adr/0012-*.md` (modified — amendment note pointing at
  0066; locate exact filename at execution)
- **Steps**:
  1. Write ADR-0066 per ruling N7: one late-bound collector slot per
     context (MetricsHandle, arc-swap); registration order irrelevant;
     multi-backend fan-out composes; `tracer.enabled` gates spans ONLY;
     metrics gating independent via `[observability.metrics]` levers;
     ADR-0012 error family the only non-disableable family; retry
     accounting (one error per exhausted sequence) and breaker-rejection
     exclusion recorded as amendments to ADR-0012 semantics. MUST also
     record (inter-phase review, Phase 1): (a) rejection-counter unit is
     readiness probes, not logical sends (parked callers retry poll_ready
     on backoff — ~1 rejection/backoff-interval per parked send; pin the
     unit choice and its alerting implication); (b) helper-owned
     exhaustion errors place the OPERATION in the first-arg (route)
     label position of camel_errors_total by design — dashboards keying
     on route must expect component-operation pseudo-routes there;
     (c) REGISTRATION-ORDER CONTRACT (inter-phase review, Phase 3):
     identity gauges (build info, uptime) are re-published on
     registration because they fire pre-registration at build time;
     route-state is transition-driven and does NOT replay to collectors
     registered after transitions fired (canonical path registers
     pre-start — `configure_context` — and is e2e-proven; post-start
     metrics registration shows uptime without route inventory until
     the next transition; queue-depth self-heals via 250ms samplers);
     pin this asymmetry as contract; (d) SAMPLING CADENCE TABLE:
     uptime 60s refresh task, SEDA/aggregator 250ms samplers,
     aggregator TTL sweep ttl/2 (min 50ms), resequencer post-accept
     only (timeout-release staleness = known ceiling); (e) DOUBLE-COUNT
     CONTRACT (4.1 review): facade failures land on
     `e:{component}:{operation}` — retained component-specific error
     labels MUST never equal that string (true double-count of one
     series); dashboards summing the whole camel_errors_total family
     count each component failure twice (uniform series + specific
     series) — intended per D5, state it in the ADR and the 4.2 audit
     table; (f) HELPER/FACADE LABEL COLLISION (4.2 review): retry_async
     with metrics=Some emits e:{scheme}:{operation} — byte-identical to
     the facade label when scheme==component (the opensearch shape).
     Today all helper call sites pass None so each failure counts once;
     ADR-0066 MUST mandate: a component that adopts the facade at an
     operation boundary where retry_async runs with metrics=Some would
     same-series double-count — the ADR forbids wiring BOTH at one
     boundary (choose one error owner).; (g) VOCABULARY ASYMMETRY (F4 review): kafka emits both consume+produce; redis emits command (producer) only — consumer stays outside the family vocabulary (D5 names producer op only); state it in the ADR vocabulary table; (h) RECORDING-DOUBLE DEBT: ~10 doubles across crates, growing per trait method — consolidation bd-tracked (rc-4dvi), ADR notes intent.
  2. Add an amendment section to ADR-0012 referencing 0066 (follow the
     repo's existing amendment-note pattern — check ADR-0046/0050/0052
     for the format).
  3. English prose; STE-writing discipline for durable ADR text.
- **Tests**:
  - `adr_rendered` — the docs build command the README documents
    includes ADR 0066 in the listing.
  - `adr_mentions_all_clauses` — `rg -c 'late-bound|composes|gates spans
    ONLY|non-disableable' docs/adr/0066-*.md` returns >= 4 matches.
    expected: N/A (docs task — no RED phase).
- **Acceptance**: `rg` check >= 4; docs listing includes 0066; ADR-0012
  carries the amendment note.

- [x] 5.2
