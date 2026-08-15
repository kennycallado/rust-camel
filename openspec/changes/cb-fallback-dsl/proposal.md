# Proposal: cb-fallback-dsl

Bd: rc-bzf6 (blocks rc-q16d)

## Why

The canonical eip-cache spec scenario ("Stale-on-error composition with CircuitBreaker")
promises a YAML shape where a circuit breaker declares a `fallback:` sub-pipeline serving
stale cache on upstream failure. That shape does not parse today: `RouteDslCircuitBreaker`
exposes only `failure_threshold` + `open_duration_ms`, so the EFFIS anchor
(CB → stale cache) is reachable only programmatically via `CircuitBreakerConfig::fallback`.
The stale-on-error resilience pattern is therefore unavailable to every YAML/DSL user.

Two structural facts shape the design:

1. **Layering** — processor construction is camel-core's monopoly
   (`StepCompilerRegistry`, step_resolution.rs). The DSL/declarative/canonical planes can
   only carry UNRESOLVED steps, exactly like `cache_peek_stale.on_miss` already does.
2. **Spec/architecture drift** — the canon scenario models CB as a *step* with
   `steps:`/`fallback:` sub-pipelines, but the runtime models CB as a *route-level layer*
   (consumed by the `CircuitBreakerGate` on error-handler routes and the Tower
   `CircuitBreakerService` on handler-less routes). No CB step verb exists.

## What Changes

- Add `fallback: Vec<RouteDslStep>` to `RouteDslCircuitBreaker` (route-level; CB-as-step
  redesign explicitly rejected as YAGNI).
- Thread fallback through the declarative and canonical planes
  (`CanonicalCircuitBreakerSpec.fallback: Vec<CanonicalStepSpec>`, serde-default +
  skip-when-empty — same pattern as `CanonicalStepSpec::Cache.on_miss`). DSL ↔ canonical
  round-trips losslessly.
- Add a `circuit_breaker_fallback: Vec<BuilderStep>` sidecar to `RouteDefinition`
  (camel-core); at route compile, resolve it via `resolve_steps` → pipeline compose →
  `BoxProcessor` and attach through the existing `CircuitBreakerConfig::fallback`
  (camel-api). Mirrors the `on_miss` precedent end-to-end.
- Guard the canonical contract: `validate_contract()` recurses into fallback steps;
  the camel-builder reverse path (opaque `BoxProcessor` → canonical) fails closed with a
  hard error (ADR-0016 "No silent loss").
- Pin the clean-outcome guarantee with regression tests: a stopped fallback (peek MISS,
  default `on_miss: stop`) already surfaces `Ok(exchange)` at the pipeline boundary
  (ADR-0024 single-translation-site) — assert it, change nothing.
- Amend the eip-cache canon scenario to the route-level shape this change implements.
- Regenerate `schemas/dsl/route-schema.json` (+ camel-lint copy) and TS bindings, document
  the surface in `docs/src/yaml-dsl/`, add an examples route.

Excluded: CB-as-step verb, changes to the legacy Tower `CircuitBreakerService` (its
existing fallback support consumes the same compiled config unchanged), new cache
repository behavior (stale-on-error stays a composition, per the existing requirement).

## Acceptance criteria

- YAML `circuit_breaker.fallback: [ cache_peek_stale: {...} ]` parses on a route
  (`deny_unknown_fields` intact; unknown keys still rejected).
- Absent/empty `fallback` leaves route behavior identical to today.
- Non-empty `fallback` compiles in camel-core to a `BoxProcessor`; when the circuit is
  open the exchange body is produced by the fallback sub-pipeline (both runtime
  consumers — the `CircuitBreakerGate` on error-handler routes and the Tower
  `CircuitBreakerService` on handler-less routes — read the same config).
- A fallback that stops (peek MISS, `on_miss: stop`) surfaces as `Ok(exchange)` with
  Exchange state intact — never `CircuitOpen`, never a boundary error.
- Canonical roundtrip (spec → serialize → rebuild) preserves fallback steps;
  `validate_contract` rejects invalid nested fallback steps; the builder reverse path
  errors on opaque fallback instead of silently dropping it.
- `cargo xtask schema --check` passes after regen; fmt/clippy green.
- eip-cache delta scenario matches the implemented route-level shape; `openspec validate`
  passes.

## Risk budget

Acceptable: additive AST/canonical/RouteDefinition fields (serde-defaulted / sidecar),
one fail-closed error branch in camel-builder (programmatic-only path, documented).
Out of bounds: touching the `CacheRepository` trait, changing default `on_miss`
semantics, CB state-machine redesign, adding Stop-translation at any caller (violates
ADR-0024 single-translation-site), breaking the canonical spec contract for existing
serialized routes.
