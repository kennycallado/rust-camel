## MODIFIED Requirements

### Requirement: Stale-on-error composition with CircuitBreaker

The system SHALL allow users to compose stale-on-error resilience by combining the
route-level `circuit_breaker` configuration (with its `fallback:` sub-pipeline) and the
`cache_peek_stale` step. No feature of the cache SHALL bake stale-on-error into the
`CacheRepository` trait or its backends. The composition SHALL be demonstrable
end-to-end from YAML. A fallback that stops (peek MISS with the default `on_miss: stop`
policy) SHALL surface as a clean outcome, not an error.

#### Scenario: circuitBreaker fallback serves cached stale entry on upstream failure

- **GIVEN** a route of the shape `from: ...` with route-level
  `circuit_breaker: { failure_threshold: 1, open_duration_ms: 60000, fallback: [
  cache_peek_stale: { repository: persistent, key: "tile-xyz" } ] }`, where the route
  body performs the upstream fetch, and a `"persistent"` repository holding a stale
  (past-expiry) entry under `"tile-xyz"`
- **WHEN** the upstream fetch fails enough times that the circuit opens and a further
  exchange arrives
- **THEN** the fallback runs, `cache_peek_stale` returns the post-expiry entry, and the
  exchange body is the stale cached value (instead of an error propagating)

#### Scenario: fallback miss yields a clean outcome

- **GIVEN** the same route shape with an open circuit, but no entry (fresh or stale)
  under `"tile-xyz"`
- **WHEN** the fallback `cache_peek_stale` misses and stops per the default
  `on_miss: stop` policy
- **THEN** the route surfaces `Ok(exchange)` with the Exchange state intact — no
  `CircuitOpen` and no error escapes the circuit breaker fallback path, because the
  composed fallback pipeline translates Stop to `Ok` at its own pipeline boundary
  (ADR-0024/0025)
