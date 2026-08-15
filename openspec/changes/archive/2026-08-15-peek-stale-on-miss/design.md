# Design: peek-stale-on-miss

## Approach

Extend `CachePeekStaleService` (crates/camel-processor/src/cache_eip.rs) with an
`on_miss` policy and peek-result properties. The service is an `OutcomePipeline`
segment (ADR-0025, Segment-not-Process per ADR-0023); all changes stay inside
the segment — no repository-trait, backend, or pipeline-composition changes.

### Service semantics (CachePeekStaleService::run)

- `key_expr` resolves to `None`: `Stopped(exchange)` + `debug!` log (anomalous
  key resolution is fail-closed, not a miss). Unchanged outcome, added log.
- `peek_stale(key)` returns `Err(e)`: `Failed(e)`. Unchanged.
- `Ok(Some(entry))` (HIT): reconstruct body, set on exchange, set properties
  `CamelCachePeekHit=true` and `CamelCachePeekStale=(entry.expires_at` elapsed
  at evaluation time`)`, return `Completed`. Body behavior unchanged.
- `Ok(None)` (MISS):
  - `on_miss = Stop` (default): set `CamelCachePeekHit=false` and
    `CamelCachePeekStale=false` on the exchange, `debug!` log (step and repository
    only — raw keys are NOT logged: key expressions may resolve credential-bearing
    exchange data), return `Stopped(exchange)`. Outcome- and body-compatible with
    today plus observability.
  - `on_miss = Continue`: set `CamelCachePeekHit=false` (and
    `CamelCachePeekStale=false`), leave body untouched, return
    `Completed(exchange)`.

Property values are `serde_json::Value::Bool` in the exchange property map,
consistent with existing principal/fragment properties.

### Constants

`pub const CAMEL_CACHE_PEEK_HIT: &str = "CamelCachePeekHit";` and
`CAMEL_CACHE_PEEK_STALE: &str = "CamelCachePeekStale";` exported from
`camel-processor` (precedent: `CAMEL_LOOP_INDEX` in loop_eip, documented in the
crate CONTEXT.md Language section).

### DSL surface

`cache_peek_stale:` mapping gains optional `on_miss:` string field
(`"stop" | "continue"`, default `"stop"`; unknown value = route compile error,
fail closed per ADR-0033 spirit). Plumb: camel-dsl step model →
`compile_declarative_steps` → camel-core `route_definition` →
`step_compilers/core.rs` constructor call → `CachePeekStaleService::new`
signature gains the policy.

### Observability (absorbs rc-uow1)

One `debug!` record on each Stopped arm (MISS-stop, key-None), naming the step
and repository only. Keys are not credentials, but key expressions may resolve
credential-bearing exchange data, so raw keys are not logged. `warn!` would be
wrong (stop is successful control flow, not a fault); ADR-0012 categories target
error paths, this is informational.

### User-facing documentation

CONTEXT-MAP.md requires user docs for a new DSL key: update
`docs/src/eip/cache.md` (knob + properties + SWR recipe),
`docs/src/yaml-dsl/step-verbs.md` (`on_miss:` field), and add an anchored
stale-while-revalidate route to `examples/cache-example/routes.yaml`.

## Affected crates and boundaries

| Crate | Change |
|---|---|
| `camel-processor` | `CachePeekStaleService` (policy field, properties, logs), consts, tests |
| `camel-dsl` | step model field + compile plumbing + parse tests |
| `camel-core` | `route_definition` + `step_compilers/core.rs` wiring, integration tests |
| docs | `crates/camel-processor/CONTEXT.md` Language entries + EIP row; `docs/src/eip/cache.md`; `docs/src/yaml-dsl/step-verbs.md`; `examples/cache-example/routes.yaml` |

No changes: `camel-api` (no new outcome variant — reuse Completed/Stopped),
`CacheRepository` trait, memory/redb backends, consumers.

## Risks / invariants

- Default path must remain outcome- and body-compatible: pinned by keeping the
  existing absence-Stops scenario green (new properties and the debug record are
  observable additions, not behavior changes).
- `CamelCachePeekStale` computation: entry may carry `expires_at: None`
  (never-expires) → stale=false. Use evaluation-time `SystemTime::now()`.
- Backwards compat: new YAML field is optional; old routes compile identically.

## Testing strategy

Unit (camel-processor, MockCacheRepository): MISS+stop (Stopped + property +
log assertion via captured subscriber), MISS+continue (Completed, body
unchanged, properties), HIT fresh/stale flag, key-None arm. DSL parse tests
(default, explicit stop/continue, invalid value rejected). camel-core compiler
test: YAML route compiles with `on_miss: continue` and service receives policy.
