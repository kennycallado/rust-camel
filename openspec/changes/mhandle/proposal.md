# Proposal: mhandle — MetricsHandle through build_redis_*_repo for C1 transient counts

bd: rc-0dcfp (P3, re-filed from rc-pleop; superseding scope of closed rc-2or1).

## Why

rc-2or1 landed tracing + atomic counters for the redis repositories
(`transient_refreshes` on `RedisIdempotentRepository`, hits/misses atomics on
`RedisCacheRepository`) but never wired a `MetricsHandle` through the
`build_redis_*_repo` construction path in `camel-config`. Result: C1-class
events — a transport failure classified transient by
`is_transient_redis_error`, either lost-outcome (`add` returning `Err`) or
retry-recovered (`execute_retry_safe`) — are invisible in every metrics
family. Operators scraping `camel_component_operations_total` see nothing
from redis-backed repositories during sentinel failovers or connection
flakiness.

## What Changes

- `camel-redis-repo`: `RedisCacheRepository` and
  `RedisIdempotentRepository` gain a `ComponentMetrics` facade threaded
  through `connect`/`with_executor`. Every transient classification
  (`is_transient_redis_error == true`) emits exactly one
  `observe("redis", <bounded operation literal>, failed=true)` — landing in
  `camel_component_operations_total{component=redis,operation,outcome}`
  (lever-gated) plus the never-gated error family as `e:redis:<operation>`.
- `camel-redis-repo`: `execute_retry_safe`/`scan_unlink_pattern` take the
  facade plus a caller-bounded operation label so the classification site
  (not the caller) emits.
- `camel-config`: `build_redis_cache_repo`/`build_redis_idempotent_repo`
  construct the facade from the context's shared late-bound handle
  (`ctx.metrics()`) and the `[observability.metrics].components` lever
  snapshot from the `CamelConfig` being applied.
- `camel-test`: the three TLS live-test `connect` call sites pass a lever-off
  facade (compile-only; tests stay `#[ignore]`d live suites).
- rc-2or1's atomic counters, accessors, and tracing remain intact and
  unchanged.

Excluded: the redis COMPONENT (camel-component-redis) — it already receives
`RuntimeObservability::component_metrics()` at endpoint creation; cache
hit/miss family wiring (camel_cache_* — deferred under rc-kah44); any change
to `MetricsCollector`, `ComponentMetrics`, or prometheus families.

## Acceptance criteria

- A transient classification on any redis-repo path is observable as
  `record_component_operation("redis", <op>, "failure")` through a recording
  collector, with the matching `e:redis:<op>` error-family forward.
- Non-transient errors emit nothing (C1 is transient-only).
- Lever off suppresses the component-operations series; the error family
  still flows (facade contract).
- `build_redis_*_repo` threads the shared handle + lever snapshot; rc-2or1
  counters and tests remain green.
- Gates: fmt, clippy affected `-D warnings`, affected test suites,
  lint-metric-labels.

## Risk budget

Construction-path signature changes in three crates (redis-repo, config,
test) — mechanical, compiler-verified. Behavioral risk is bounded to NEW
emissions on transient paths; retry/refresh semantics, counter semantics,
and error surfacing are untouched. No new metric families, no open labels
(closed literal set through the annotated facade).
