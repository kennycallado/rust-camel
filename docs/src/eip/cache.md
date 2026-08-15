# Cache

The Cache pattern stores the result of an expensive computation and serves it on subsequent requests with the same key. When the key matches, the route returns the cached body and skips the computation. When the key does not match, the route runs the `on_miss` sub-pipeline, stores the result, and returns it.

```yaml
{{#include ../../../examples/cache-example/routes.yaml:cache-route}}
```

The `cache` step evaluates a `key` expression against each exchange. If the repository holds a live entry for that key, the step replaces the body with the cached bytes and returns `Completed`. If the key is absent or expired, the step runs the `on_miss` sub-pipeline, writes the resulting body to the repository with the given `ttl`, and returns the body to the pipeline. A key expression that evaluates to `None` bypasses the cache. The route runs the `on_miss` sub-pipeline without a lookup or a write-back.

The `cache_invalidate` step removes a single key from the repository. Use it when an upstream event makes a cached entry stale. The `cache_peek_stale` step reads a cached entry and ignores its in-band expiry. This serves a post-expiry entry as a fallback when the source is unavailable.

## cache_peek_stale `on_miss` policy

The `cache_peek_stale` step accepts an `on_miss` policy. The value `"stop"` is the default. On a miss the step sets `PipelineOutcome::Stopped` and the branch stops. The value `"continue"` passes the exchange through unchanged. Both values set the `CamelCachePeekHit=false` and `CamelCachePeekStale=false` exchange properties on a miss.

On a hit the step sets `CamelCachePeekHit=true`. It sets `CamelCachePeekStale=true` when the served entry is past its `expires_at`. A fresh entry sets `CamelCachePeekStale=false`. The properties enable a stale-while-revalidate (SWR) route: peek with `on_miss: continue`, branch on `CamelCachePeekHit`, fetch and cache on a miss, and serve the peeked body otherwise.

```yaml
- cache_peek_stale:
    key: "${header.cacheKey}"
    on_miss: continue
- choice:
    when:
      - simple: "${exchangeProperty.CamelCachePeekHit} == false"
        steps:
          - set_body: "fresh-data"
          - cache:
              key: "${header.cacheKey}"
              ttl: "5s"
              on_miss:
                - log: "SWR miss — fresh body stored"
    otherwise:
      - log: "Serving cached body: ${body}"
```

## Stale-on-error with a circuit breaker

Compose `cache_peek_stale` with a route-level `circuit_breaker` to serve a stale entry when the downstream service fails. The `fallback` list holds a sub-pipeline. The breaker runs the fallback only while the circuit is open.

```yaml
{{#include ../../../examples/cache-example/routes.yaml:cb-fallback-stale-route}}
```

The route body wraps the upstream fetch in a `cache` step that stores the result under a static key. When the fetch fails `failure_threshold` times in a row, the circuit opens. While open, the fallback runs `cache_peek_stale` against the same static key and serves the last cached entry, even when that entry is past its TTL. On a miss (no entry), the default `on_miss: stop` policy stops the fallback cleanly. The exchange completes without a `CircuitOpen` error.

The fallback runs on routes with and without an `error_handler`. A failing fallback step follows the route's error handling. A route with an `error_handler` routes the failure through the handler. A route without one surfaces the raw error. See [Circuit Breaker](circuit-breaker.md) for the breaker states and [Route structure](../yaml-dsl/route-structure.md) for the `fallback` field.

Use the Cache pattern when a route computes the same result more than once. API responses, database lookups, and transform-heavy pipelines benefit from caching.

The default repository is `"memory"` (moka-backed, size-eviction only). A persistent `"persistent"` repository (redb-backed) is available when `[default.cache_repo] backend = "redb"` is set. The redb backend survives process restarts. Its sweep task reclaims entries whose `expires_at + stale_retention` has passed. The memory backend does not run a sweep. Expired entries stay in memory until size pressure evicts them.

The Cache differs from the [Claim Check](claim-check.md) and the [Idempotent Consumer](idempotent-consumer.md). All three use a repository trait. The Cache stores the full computed body with a TTL. The Claim Check stores the original payload without a TTL. The Idempotent Consumer stores only the deduplication key. A route that needs all three can chain them.

Per [ADR-0056](../adr/0056-cache-repository-port.md), the `CacheRepository` trait lives in `camel-api`, with memory and redb backends in `camel-core`. The trait stores `CacheEntry { bytes, content_type, expires_at }` with in-band expiry. Both backends do size-eviction only. The `expires_at` field drives `get()` misses and `peek_stale()` reads. Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), each cache step compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/cache-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/cache-example).
