## ADDED Requirements

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
