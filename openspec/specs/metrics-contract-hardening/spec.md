# metrics-contract-hardening Specification

## Purpose
TBD - created by archiving change audit-fix-metrics-contract. Update Purpose after archive.
## Requirements
### Requirement: Prometheus diagnostic endpoint must warn on non-loopback bind

The Prometheus service MUST emit a `warn!` log at startup when the bind
address is not loopback, per ADR-0052 rule 3. The warning MUST name the
address and state that the endpoint is reachable from all interfaces.

#### Scenario: Non-loopback bind emits warning

- **Given** a `PrometheusService` configured with address `0.0.0.0:9090`
- **When** `start()` is called
- **Then** a `warn!` log is emitted naming the address and the non-loopback
  exposure

#### Scenario: Loopback bind emits no warning

- **Given** a `PrometheusService` configured with address `127.0.0.1:9090`
- **When** `start()` is called
- **Then** no non-loopback warning is emitted

### Requirement: Dynamic metric-name collectors MUST be bounded by a configurable cap

The `PrometheusMetrics` dynamic counter and histogram DashMaps MUST NOT grow
beyond a configurable maximum number of unique metric-name collectors (default
1024) in single-threaded (sequential) access. The cap applies independently to
`dyn_counters` and `dyn_histograms` (total bound is 2 × cap). When the cap is
exceeded, the metric name MUST NOT be inserted into the DashMap — the
observation is dropped and a `warn!` is emitted naming the rejected name.
The cap check MUST run before acquiring the DashMap entry guard (calling
`len()` while holding an `Entry` guard deadlocks). Under concurrent access,
the `len()` check and subsequent insert are not atomic; a small overcount is
acceptable and MUST NOT be prevented by a global lock.

#### Scenario: Dynamic counter within cap accepted

- **Given** a `PrometheusMetrics` with `max_dynamic_collectors=1024`
- **When** fewer than 1024 unique counter names have been registered
  sequentially
- **Then** new counter names are accepted and observed normally

#### Scenario: Dynamic counter exceeding cap rejected (sequential)

- **Given** a `PrometheusMetrics` with `max_dynamic_collectors=2`
- **When** two unique counter names have been registered sequentially and a
  third unique name arrives
- **Then** the third name is NOT inserted into `dyn_counters`
- **And** the DashMap remains at 2 entries
- **And** a `warn!` is emitted naming the rejected name

#### Scenario: Dynamic histogram exceeding cap rejected (sequential)

- **Given** a `PrometheusMetrics` with `max_dynamic_collectors=2`
- **When** two unique histogram names have been registered sequentially and a
  third unique name arrives
- **Then** the third name is NOT inserted into `dyn_histograms`
- **And** the DashMap remains at 2 entries
- **And** a `warn!` is emitted naming the rejected name

#### Scenario: Default cap is 1024

- **Given** a `PrometheusMetrics` constructed via `PrometheusMetrics::new()`
- **When** the `max_dynamic_collectors` field is inspected
- **Then** it equals 1024

### Requirement: Server task failure MUST update service status to Failed

When the spawned Prometheus server task encounters an error and exits, the
service status MUST be updated to `Failed` (atomic value 2) before the
`warn!` log is emitted. `Lifecycle::status()` MUST NOT report `Started`
after the server has exited with an error. On clean shutdown (Ok return),
the status MUST NOT be set to `Failed`.

#### Scenario: Server task error updates status to Failed

- **Given** a started `PrometheusService` with status `Started` (1)
- **When** the server task encounters a fatal error and exits
- **Then** the status is updated to `Failed` (2)
- **And** `Lifecycle::status()` returns `ServiceStatus::Failed`

#### Scenario: Server task clean exit does not set Failed

- **Given** a started `PrometheusService` with status `Started` (1)
- **When** the server task exits cleanly (graceful shutdown returns Ok)
- **Then** the status is NOT set to `Failed` (2)

### Requirement: Prometheus name normalization is idempotent

The Prometheus exporter SHALL apply the `camel_` prefix to a dynamic metric
name at most once. A name already carrying the `camel.` (dot) or `camel_`
(underscore) prefix MUST normalize to a single `camel_` prefix after
sanitization; a foreign name MUST receive exactly one `camel_` prefix.

#### Scenario: dotted camel.* built-in normalizes to a single prefix

- **GIVEN** a recorded built-in metric name in dot form
  (`camel.cache.misses`)
- **WHEN** the name is normalized for Prometheus export
- **THEN** the exported name is `camel_cache_misses` — the `camel_` prefix
  appears exactly once

#### Scenario: underscore camel_ name is unchanged

- **GIVEN** a metric name already carrying the underscore prefix
  (`camel_exchanges_total`)
- **WHEN** the name is normalized for Prometheus export
- **THEN** the exported name is `camel_exchanges_total`, unchanged

#### Scenario: foreign name is prefixed exactly once

- **GIVEN** a foreign metric name with no camel prefix
  (`exec_successes_total`)
- **WHEN** the name is normalized for Prometheus export
- **THEN** the exported name is `camel_exec_successes_total`

