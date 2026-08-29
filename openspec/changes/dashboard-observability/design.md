# Design: dashboard-observability

## Context

Epic rc-hrm1 audit (2026-08-26) + re-verified drift pass (2026-08-27).
Collector plumbing is now correct: one late-bound `MetricsHandle` per
context, registration composes, prometheus implies the tracer pipeline
(metrics-handle-late-binding, 8c876d54). What reaches the dashboard is
still structurally wrong: inflated error counts (retry × attempt, breaker
rejections), zero success-path/inventory/backpressure signal, and no
metrics-side configuration lever. Expert rulings N3–N8 live in the bd
descriptions (rc-hrm1.*) and are normative for this design.

Key code anchors (verified this session):

- Retry: `crates/components/camel-component-api/src/network_retry.rs:85`
  (`NetworkRetryPolicy`), helpers `retry_async` (:230) and
  `retry_async_cancelable` (:324); kafka consumer reconnect loop
  `crates/components/camel-kafka/src/consumer.rs:537-576` (package
  `camel-component-kafka`); container double-count pattern
  `crates/components/camel-container/src/lib.rs:1545-1562`.
- Breaker: `crates/camel-processor/src/circuit_breaker.rs` — fast-fail
  sites :121, :134 (poll_ready), :170 (call); `CamelError::CircuitOpen`
  classified `"circuit_open"` at `crates/camel-api/src/error.rs:183`;
  pipeline tracer counts it via
  `crates/camel-core/src/shared/observability/adapters/tracer.rs:235`.
- Queue depth: `MetricsCollector::set_queue_depth`
  (`crates/camel-api/src/metrics.rs`) — zero production callers; the
  `camel_queue_depth` GaugeVec is already registered in
  `crates/services/camel-prometheus/src/metrics.rs:72,145`.
- Route state: `RouteStatusProjection` maintained synchronously
  (ADR-0002/0045) — no new bookkeeping needed for the gauge.
- Config: `[observability.tracer]` / `[observability.prometheus]` resolved
  in `crates/camel-config/src/context_ext.rs` (`effective_tracer_config`,
  now 4-param after this session).
- Audit drift (2026-08-27, authoritative component list): camel-ws emits
  `e:ws:authn` (not fully dead); camel-master emits leadership counters
  (not dead); camel-component-surrealdb is PARTIAL (notification counter at
  consumer.rs:192-194); camel-cxf has exactly one emission site
  (b-prime:cxf:response-marshalling, consumer.rs:310). Fully dead:
  camel-component-wasm, camel-opensearch, camel-component-seda,
  surrealdb remainder, cxf remainder.

## Goals / Non-Goals

**Goals**

1. `camel_errors_total` means "exchange failed" — one increment per
   exhausted retry sequence; breaker rejections excluded.
2. Per-attempt and per-rejection telemetry exist as separate series.
3. Healthy routes are observable: success-path component counters,
   route-state gauge, build/uptime info, queue depth.
4. Metrics families configurable independently of tracing; ADR-0012
   error family always on.
5. Label values provably closed sets, enforced by lint.
6. The collector binding/lifetime contract written down (ADR, amends
   ADR-0012).

**Non-Goals**

- New metric backends; changing the MetricsHandle plumbing landed in
  metrics-handle-late-binding.
- Histogram granularity changes to existing pipeline families.
- Span/tracing semantics (trace-model chain, already merged).
- redis-repo observability (rc-2or1, P3, separate).
- Readiness-phase failure blindness (rc-mn8n, separate P2 follow-up).
- HTTP header/bridge semantics (http-emission-correctness capability
  holds header/bridge requirements only; we align retry accounting on
  HTTP paths, not metric definitions there).
- Test-posture trap (binding for Phases 1-4): default
  `failIfNoConsumers=true` aborts in `poll_ready` BEFORE the traced
  wrapper records anything — failure-path tests MUST route failures to
  call-time (`to:direct:missing?failIfNoConsumers=false`) so the metric
  legs observe them (pattern proven in metrics_wiring_test.rs:163).
- Determinism pins for tasks.md (binding): uptime scenario asserts a
  numeric bound (< 120s), not "near zero"; queue-drain scenarios await
  drain with one sampling tick of lag tolerance; SEDA backlog is built
  with a blocked consumer, not a producer speed race.

## Decisions

### D1 — Retry accounting (ruling N3)

One `increment_errors` per exhausted `NetworkRetryPolicy` sequence,
executed by the policy helpers themselves (`retry_async` /
`retry_async_cancelable` final-failure arm), NOT by call sites.
Cancellation is NOT failure: a cancelled sequence emits no error
(container :1555/:1642 already treats cancelled as clean). Two
verified site classes: (a) ADOPTERS that call the helpers AND increment
on their own Err arm double-count — camel-container lib.rs:1545-1562
(`e:container:events-connect`) is the pattern; Phase 1 sweeps every
helper Err arm and removes those increments. (b) The kafka consumer is a
MANUAL event loop (comments :542-544 reject retry_async) — its six
increment sites (:304, :318, :334, :378, :507, :524) are terminal
one-shot failures that KEEP incrementing, and its recv-exhaustion arm
(:574-577) currently counts NOTHING (log only) — Phase 1 ADDS one
increment there, matching helper final-failure semantics. Error-label
contract for helper-owned exhaustion: `increment_errors(operation,
"e:{scheme}:{operation}")` — first arg is the operation label (the
helper has no route scope; existing ADR-0012 component labels already
use this shape), error_type follows the container's established
`e:{scheme}:{operation}` pattern. Per-attempt
telemetry: new trait method
`increment_retry_attempt(scheme: &str, operation: &str)` →
`camel_retry_attempts_total{scheme,operation}` counter. `scheme` values
are closed sets per component (e.g. kafka: `connect`, `dispatch`,
`fetch`; redis: `command`; http: `request`) — the literal set is declared
in each component and covered by the Phase 5 lint. Call-site cleanup is
part of Phase 1: remove per-attempt increment_errors from retry-wrapped
error paths in kafka (first), then sweep other NetworkRetryPolicy users
(list built during Phase 1 audit step; http/redis covered in Phase 4
where their success paths land anyway).

### D2 — Breaker rejections (ruling N4)

New trait method
`increment_circuit_breaker_rejection(route: &str)` →
`camel_circuit_breaker_rejections_total{route}`. Called at the three
fast-fail sites in circuit_breaker.rs. Exclusion from
`camel_errors_total` happens in the pipeline tracer
(tracer.rs:235 area): `error_type == "circuit_open"` skips
increment_errors (the breaker already recorded the rejection). This is
the classifier boundary — components keep seeing `CamelError::CircuitOpen`
unchanged.

### D3 — Metrics levers (rc-hrm1.1)

New `[observability.metrics]` table: `enabled` (default true when
prometheus or otel metrics export is on), `exchange = true|false`,
`duration = true|false`, `components = true|false` (default false —
opt-in, per audit cost note success-path counters are new cardinality).
ADR-0012 error family is NOT configurable — always emitted; the configuration schema (docs/src/configuration/schema.md, guarded by `cargo xtask schema --check`)
updated; `MetricsLeversConfig` mirrors TracerConfig's serde pattern
(Option<bool> fields + explicit-set detection where defaults need it,
reusing the Raw-intermediate technique from metrics-handle-late-binding
T1.3). Gating point: a levers struct consulted by the pipeline tracer
before non-error families and by component emission helpers — never for
increment_errors.

### D4 — Inventory + backpressure emissions (rulings N5/N6)

- `set_route_state(route, state)` — gauge set from the same place
  RouteStatusProjection transitions are published (route state service);
  states are the closed set of the projection enum.
- `record_build_info(version, git_sha)` — set once at context build;
  `camel_build_info{version,git_sha}` value 1 (standard info-gauge
  pattern). git_sha from `option_env!("VERGEN_GIT_SHA").unwrap_or("unknown")`
  — camel-core has no vergen build script today; accepting "unknown"
  until a build.rs lands (optional follow-up, not this change).
- `record_uptime()` — `camel_uptime_seconds` set on scrape is not
  possible with push semantics; instead set on registration + periodic
  refresh tied to existing service tick, or a Gauge backed by Instant
  start (preferred if camel-prometheus supports closures; else set at
  registration and every service tick).
- `set_queue_depth(queue, depth)` — RENAME the trait parameter and the
  registered GaugeVec label from `route` to `queue` (zero production
  callers today, rename is free; spec says `{queue}`). Wire SEDA consumer
  queue, aggregator
  buffer, resequencer buffer. Report on mutation is too hot; sample on
  the existing component tick (SEDA poll loop, aggregator/RE-sequencer
  maintenance passes). Label `queue` is component-declared closed set.

### D5 — Component success-path family (rc-6s6h)

One uniform family, not per-component inventions:
`camel_component_operations_total{component,operation,outcome}` where
outcome ∈ {success, failure}. Failure legs MAY additionally increment
ADR-0012-style specific counters where they exist today (cxf
response-marshalling, ws authn) — those stay. The uniform family is the
default emission for the Phase 4 sweep (wasm, opensearch, seda,
surrealdb remainder, cxf remainder + success paths on kafka, redis,
http where retry accounting already lands). Gated by
`[observability.metrics].components` (opt-in, D3). The `ComponentMetrics`
facade lives in camel-api and stores a `components_enabled: bool`
PRIMITIVE snapshot taken at construction in camel-core (where the levers
type lives) — camel-api cannot depend on camel-core, so the levers
struct itself stays out of the facade.

### D6 — Label closed-set lint (ruling N8)

xtask `lint-metric-labels`: every `record_counter`/`record_histogram`
label VALUE argument must be (a) a string literal, or (b) BEST-EFFORT
recognized `OptionKind::Enum`-derived expressions (ADR-0041 uri_options
paths; source-text heuristic — default-deny when undecidable), or (c)
annotated `// allow-open-label` with a bd reference. Core rule is
default-deny: literal or annotate. Runs in CI
gate set alongside lint-context-citations.

### D7 — ADR (ruling N7, amends ADR-0012)

New ADR: one late-bound collector slot per context (MetricsHandle);
registration order irrelevant; multi-backend fan-out composes;
`tracer.enabled` gates spans ONLY; metrics gating independent
(D3 levers); error family non-disableable. Task-only (documentation
capability already governs ADR format).

## Risks / Trade-offs

- **`camel_errors_total` semantics change** — headline user-visible
  change; alert thresholds calibrated to inflated counts will fire less.
  Merge commit must state it. Ruled correct by N3/N4.
- **Trait growth** — five new default methods on MetricsCollector;
  out-of-tree implementors keep compiling (NoOp defaults). Composite must
  delegate all of them (test enforces parity of the delegation list).
- **Cardinality** — `camel_component_operations_total` bounded by
  closed label sets; components opt-in default-off contains the blast
  radius. Route-state gauge bounded by route count.
- **Uptime gauge freshness** — depends on chosen refresh path; documented
  in the ADR; acceptable staleness = one tick.

## Phases

Delivery order; each phase-group implements and reviews before the next.
Full task blocks live in tasks.md; single plan-bless covers ALL phases.

1. **Error semantics** (D1+D2): trait methods + prometheus registration +
   policy-helper exhaustion accounting + tracer circuit_open exclusion +
   kafka call-site cleanup + tests. Exits: one error per exhausted
   sequence; attempts series exists; rejections excluded (rc-hrm1.4,
   rc-hrm1.5 done).
2. **Metrics levers** (D3): levers config + schema.md + gating + tests.
   Exits: exchange/duration/components disableable independently; error
   family not disableable (rc-hrm1.1 done).
3. **Inventory + backpressure** (D4): route-state, build-info, uptime,
   queue-depth wiring + tests. Exits: gauges on scrape under
   prometheus-only; queue depth non-zero under SEDA load (rc-hrm1.6,
   rc-hrm1.7 done).
4. **Component sweep** (D5): uniform family + dead-component emissions +
   kafka/redis/http success paths + tests. Exits: every audited component
   emits; audit table in tests (rc-6s6h, rc-q25t, rc-bfnw done).
5. **Closure** (D6+D7): label lint + ADR. Two tasks — below the
   inter-phase-review threshold; relies on per-task r_glm + the archive
   holistic gate. Exits: lint green workspace-wide (allowlist only with
   bd refs); ADR merged amending ADR-0012 (rc-hrm1.8, rc-hrm1.9 done).

## Migration Plan

- API: MetricsCollector trait additions are additive (default methods).
  `retry_async*` signatures and `CircuitBreakerLayer/Gate::new` params ARE
  breaking for published crates (camel-component-api, camel-processor) —
  every caller is workspace-internal and swept by the tasks; the merge
  commit must state the helper/constructor signatures changed.
- Behavior: `camel_errors_total` counts drop by the retry-factor and by
  breaker rejection rate — intentional, documented in ADR + merge commit.
- Labels: helper-owned exhaustion errors change shape — adopters' Err-arm
  labels (e.g. container's `e:container:events-connect` emitted at the
  call site) become helper-emitted `e:{scheme}:{operation}` with the
  operation label as first arg; first-arg route attribution for
  helper-emitted errors is the operation, not the route (helpers have no
  route scope). Recorded in ADR-0066.
- Config: new `[observability.metrics]` table; absence = current
  behavior plus new opt-out-able families on; `components` defaults
  off, so default deployments see only the two semantic fixes + gauges.

## Open Questions

- None blocking; D4 uptime refresh mechanism left to implementation
  (closure-backed gauge preferred, tick-set fallback sanctioned).
