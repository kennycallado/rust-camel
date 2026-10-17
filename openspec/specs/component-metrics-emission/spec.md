# component-metrics-emission Specification

## Purpose
Metrics emitted by the runtime and its components. It covers route state,
build and uptime info, queue depth, the uniform component-operations family,
endpoint creation, pinned-client cache reuse, and jemalloc memory under the
`jemalloc` feature.

## Requirements
### Requirement: Route state gauge

The system SHALL export `camel_route_state{route,state}` maintained from
`RouteStatusProjection` transitions, such that a scrape reflects the
current state of every route.

#### Scenario: route starts

- **WHEN** a route reaches its startup-complete state (`Started` in the
  `RouteStatusProjection` closed set)
- **THEN** `camel_route_state{route,state="Started"}` equals 1 for that
  route and its previous state gauge equals 0

### Requirement: Build and uptime info

The system SHALL export `camel_build_info{version,git_sha}` (value 1)
and `camel_uptime_seconds` on every scrape under prometheus.

#### Scenario: fresh scrape after restart

- **WHEN** the application restarts and is scraped
- **THEN** `camel_build_info` identifies the new build and
  `camel_uptime_seconds` is near zero, distinguishing restart-reset
  counters from idle health

### Requirement: Queue depth visible for buffered stages

The system SHALL call `MetricsCollector::set_queue_depth` from SEDA
consumers, the aggregator, and the resequencer, exporting
`camel_queue_depth{queue}` that is non-zero under load and drains toward
zero when idle.

#### Scenario: SEDA backlog under load

- **WHEN** a SEDA endpoint receives faster than its consumer drains
- **THEN** `camel_queue_depth` for that queue is positive during the
  backlog and reaches zero after draining

### Requirement: Uniform component operations family

Components SHALL emit
`camel_component_operations_total{component,operation,outcome}` with
outcome ∈ {success, failure} for their principal operations, gated by
the `[observability.metrics].components` lever; components audited as
holding-but-not-emitting (wasm, opensearch, seda, surrealdb remainder,
cxf remainder) SHALL emit this family for errors at minimum.

#### Scenario: healthy route observable

- **WHEN** a route using a swept component processes exchanges
  successfully with the components lever on
- **THEN** `camel_component_operations_total{...,outcome="success"}`
  increases for the component's principal operation

#### Scenario: dead-observability component now emits errors

- **WHEN** a swept component's operation fails with the components lever
  off
- **THEN** the failure is observable on the component's error-family
  emission despite the lever being off

#### Scenario: registry context honors the components lever

- **GIVEN** a `RegistryComponentContext` constructed with the lever
  snapshot on and a wired collector
- **WHEN** `component_metrics_enabled()` is queried and the facade is
  built through `component_metrics()`
- **THEN** the lever reports on and the facade emits the
  component-operations family through the wired collector
- **AND** constructed with the lever off, the family is suppressed
  while the error family still flows

### Requirement: Label values are closed sets

Metric label values emitted through `record_counter` and
`record_histogram` SHALL be string literals, values derived from
`OptionKind::Enum` metadata (ADR-0041), or explicitly allowlisted with a
bd reference; an xtask lint SHALL enforce this workspace-wide.

#### Scenario: lint rejects raw label value

- **WHEN** a label value is a raw runtime string (path, status,
  hostname) without an allow annotation
- **THEN** `cargo xtask lint-metric-labels` fails citing the site

#### Scenario: literal passes

- **WHEN** a label value is a string literal
- **THEN** the lint passes

#### Scenario: enforcement scope

- **WHEN** the lint walks the workspace
- **THEN** it enforces `record_counter`, `record_histogram`,
  `record_component_operation`, and `increment_retry_attempt` label
  positions; the `increment_errors` route-label position stays outside
  enforcement (route ids are user-defined by design — follow-up work
  audits its bounded sets separately)

### Requirement: Pinned client cache reuse is visible

Every HTTP component's pinned client cache exposes its reuse behavior through
the wired Prometheus collector: client builds (misses), cache hits, and the
approximate current entry count, labeled with the owning component. The
series MUST be sufficient to detect a client-proliferation regression
(entry count climbing) and a reuse regression (misses tracking hits 1:1)
without test-only accessors. Misses MUST count client constructions: under
concurrent single-flight resolution of one cold key, exactly one miss and
N−1 hits are recorded. Emission MUST be a no-op when no collector is wired
(ADR-0066 late binding), never an error on the request path. The component
label value MUST come from a closed two-value set (`camel-http`,
`camel-https`) enforced by a unit-test invariant.

#### Scenario: steady pinned traffic registers hits and a bounded size

- **WHEN** an HTTPS producer serves repeated requests whose DNS-pinned
  address set is unchanged within the cache TTL
- **THEN** the wired Prometheus collector receives pinned-client-cache hit
  counter increments for component `camel-https`
- **AND** it receives the cache size gauge with the approximate moka entry
  count, which stays bounded by the cache capacity (64)

#### Scenario: a client build registers exactly one miss

- **WHEN** a request resolves a pinned key that is absent or expired and the
  cache builds a new client
- **THEN** the wired Prometheus collector receives one pinned-client-cache
  miss counter increment for the owning component
- **AND** N−1 concurrent waiters on the same cold key each register a hit,
  because misses count client constructions, not waiter count

#### Scenario: components are distinguished by label

- **WHEN** both `camel-http` and `camel-https` components are active in one
  runtime
- **THEN** their cache series carry distinct component label values drawn
  from the closed two-value set and never aggregate into one another's series

### Requirement: Allocator memory is visible under jemalloc

When the binary is built with the `jemalloc` allocator feature, the `camel
run` command periodically reports jemalloc's allocated, resident, active, and
mapped byte totals through the wired Prometheus collector, identified by a
closed set of stat labels. Each sample MUST advance the jemalloc epoch before
reading stats. Read failures MUST degrade to a warning and retry on the next
interval; an initialization failure MUST disable the sampler with a
warning — either way, never aborting the run. Emission MUST be
unconditional while the feature is enabled (no extra configuration lever) and
MUST NOT compile into binaries built without the feature. The stat label set
MUST be closed (no free-form strings). Diagnosis supported: growing
`allocated` indicates live allocation growth; flat `allocated` with growing
`resident` indicates allocator retention.

#### Scenario: one sample maps a snapshot to the four gauges

- **WHEN** the sampler performs one successful read returning byte totals
  for allocated, resident, active, and mapped
- **THEN** the wired Prometheus collector receives exactly four
  allocator-memory gauge emissions whose values equal the read totals,
  unchanged
- **AND** the read advanced the jemalloc epoch before reading

#### Scenario: a read failure warns and retries, never aborts

- **WHEN** a sample's epoch advance or stat read fails
- **THEN** no allocator-memory emission is made for that tick and a warning
  is logged
- **AND** the sampler continues and the next tick attempts the read again
  without terminating `camel run`

#### Scenario: no allocator series without the feature

- **WHEN** the binary is built without the `jemalloc` feature
- **THEN** no allocator-memory series is emitted and no jemalloc-ctl code is
  compiled into the binary

### Requirement: Endpoint creation is visible

The runtime SHALL count every endpoint creation in the resolver path as
`camel_core_endpoints_created_total{component=<scheme>}` (recorded in dot
form as `camel.core.endpoints_created_total`). The `component` label value
is the URI scheme of the created endpoint; its value set is bounded by the
registered component schemes and is an open label by construction (rc-haik).

#### Scenario: creating endpoints for two components increments each label once

- **GIVEN** a runtime with components registered under two schemes
  (`direct` and `mock`)
- **WHEN** one endpoint is created per scheme through the resolver
- **THEN** the counter carries one increment under `component="direct"` and
  one increment under `component="mock"`

#### Scenario: repeated dynamic URIs for the same component accumulate on that label

- **GIVEN** a runtime with a component registered under one scheme
- **WHEN** several endpoints with distinct dynamic URIs of that scheme are
  created through the resolver
- **THEN** the counter accumulates under the single
  `component=<scheme>` label

### Requirement: Executable emission proof for artifact-gated components

The wasm `invoke` and cxf `consume` facade wiring SHALL be backed by
executable in-crate emission proof that requires no external artifacts
(no native bridge binary; only committed test fixtures), driven through
the real production paths (endpoint creation and producer call for wasm;
consumer task and response handling for cxf) against a recording
`RuntimeObservability` double published in camel-component-api test
support. The `AUDIT` const in `crates/camel-test/tests/component_emission_test.rs`
SHALL remain the authoritative swept-operations table with every entry
backed by executable proof.

#### Scenario: wasm invoke success leg

- **GIVEN** a recording RuntimeObservability double with the components
  lever on, and the committed echo guest copied into a temp base dir
- **WHEN** a wasm endpoint created through `WasmComponent::create_endpoint`
  produces an exchange whose producer call completes successfully
- **THEN** the double records the component operation
  `("wasm", "invoke", "success")` and no error-family increment

#### Scenario: wasm invoke failure leg

- **GIVEN** the same wiring pointed at a file that is not a valid WASM
  module, with the components lever on
- **WHEN** the producer call fails
- **THEN** the double records `increment_errors("wasm", "e:wasm:invoke")`
  on the never-gated error family, and the component operation
  `("wasm", "invoke", "failure")`

#### Scenario: wasm invoke failure with the lever off

- **GIVEN** the same failing wiring with the components lever off
- **WHEN** the producer call fails
- **THEN** the double still records
  `increment_errors("wasm", "e:wasm:invoke")` and records no component
  operation at all

#### Scenario: cxf consume success leg via mock bridge

- **GIVEN** a recording double with the lever on and a real
  `CxfConsumer` started against the in-test mock bridge with a ready
  bridge slot
- **WHEN** the route answers a consumed request without fault
- **THEN** the double records `("cxf", "consume", "success")` and no
  error-family increment

#### Scenario: cxf consume route-error leg

- **GIVEN** the same wiring with a route handler that rejects the
  exchange, with the components lever on
- **WHEN** the consumer emits the fault response
- **THEN** the double records `increment_errors("cxf", "e:cxf:consume")`
  and the component operation `("cxf", "consume", "failure")`

#### Scenario: cxf consume failure with the lever off

- **GIVEN** the same route-error wiring with the components lever off
- **WHEN** the consumer emits the fault response
- **THEN** the double still records
  `increment_errors("cxf", "e:cxf:consume")` and records no component
  operation at all

#### Scenario: cxf marshalling failure retains the b-prime label

- **GIVEN** the same wiring (lever on) with a consumer route id distinct
  from the component name (for example `emission-marshalling-leg`) and a
  route handler that returns a stream output body (unmarshallable by the
  consumer)
- **WHEN** the response marshalling fails
- **THEN** the double records exactly one route-scoped error
  `increment_errors("<route-id>", "b-prime:cxf:response-marshalling")`
  under the consumer's route id, plus the component-scoped
  `increment_errors("cxf", "e:cxf:consume")` and one component operation
  `("cxf", "consume", "failure")` — the intended design-D5 double-count
  where the two error labels differ and land under different first
  labels (route id vs component)

#### Scenario: AUDIT table stays authoritative

- **WHEN** the camel-test component emission suite runs
- **THEN** the `AUDIT` const still lists exactly the swept
  component-operation pairs including wasm `invoke` and cxf `consume`,
  and the module documentation points to the crate-internal proofs for
  both entries instead of claiming no honest harness exists

