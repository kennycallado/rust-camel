# Delta spec: peek-stale-on-miss

## MODIFIED Requirements

### Requirement: Cache EIP face — cache, cache_invalidate, cache_peek_stale steps

The system SHALL provide three new DSL step kinds: `cache`, `cache_invalidate`, and
`cache_peek_stale`. The `cache` step SHALL accept a `repository:` name (defaulting to
`"memory"`), a `key:` expression, an optional `ttl:` duration, an optional
`max_entry_bytes:` size (default 10 MiB = `DEFAULT_MATERIALIZE_LIMIT`), and an `on_miss:`
sub-pipeline. On hit (entry present and not expired) the step SHALL replace the exchange
body with the cached entry's reconstructed `Body` (reconstructed via `content_type`:
`Bytes → Body::Bytes`, `Text → Body::Text`, `Json → Body::Json`, `Xml → Body::Xml`) and
SHALL NOT run `on_miss`. On miss the step SHALL run the `on_miss` sub-pipeline, then apply
the **write-back materialization policy** to the resulting body:

- **Already-materialized body** (`Body::Bytes`/`Text`/`Json`/`Xml`): the step checks
  `bytes.len() <= max_entry_bytes`. If it fits, the step constructs a `CacheEntry` and
  `set`s it under the key with the supplied `ttl`, then continues with the fresh body. If
  it exceeds the limit, the step passes the original body through **unchanged** (the body
  is still intact), SHALL NOT call `set`, and SHALL log at `debug` level (per ADR-0012 —
  not an error; the oversized entry degrades to uncached).
- **Streaming body** (`Body::Stream`): the step calls `Body::into_bytes(max_entry_bytes)`.
  If materialization succeeds, the step constructs a `CacheEntry` with `content_type =
  ContentType::Bytes` (the materialization yields raw bytes with no higher type info) and
  `set`s it, then continues with the materialized `Body::Bytes` (replacing the consumed
  stream). If materialization fails with `StreamLimitExceeded`, the stream has been
  partially consumed and cannot be re-served — the step SHALL propagate
  `Err(CamelError)` (an oversized stream is a hard error, not a silent passthrough, because
  the consumed stream cannot be recovered).

`cache_invalidate` SHALL accept a `repository:` and `key:` and SHALL remove the key.

`cache_peek_stale` SHALL accept a `repository:`, a `key:`, and an optional `on_miss:`
policy (`"stop"` — the default — or `"continue"`; any other value SHALL be rejected at
route compile time). The step SHALL evaluate the key expression first:

- Key expression resolves to `None`: the step SHALL set `PipelineOutcome::Stopped` for
  the current branch (an anomalous key resolution is fail-closed, not a miss) and SHALL
  emit one `debug`-level log record naming the step and repository.
- `peek_stale` returns `Err`: the step SHALL propagate `Err`.
- Entry present (ignoring expiry): the step SHALL replace the body with the
  reconstructed `Body`, SHALL set the exchange properties `CamelCachePeekHit=true` and
  `CamelCachePeekStale` (true when the entry's `expires_at` has elapsed at evaluation
  time; false when absent or not elapsed), and SHALL continue the pipeline.
- Entry absent (MISS):
  - `on_miss="stop"`: the step SHALL set `CamelCachePeekHit=false` and
    `CamelCachePeekStale=false` on the exchange, SHALL emit one `debug`-level log
    record naming the step and repository (raw keys SHALL NOT be logged — key
    expressions may resolve credential-bearing exchange data), and SHALL set
    `PipelineOutcome::Stopped` for the current branch (the step is used in
    `CircuitBreaker.fallback` where absence means "no stale available" — silently passing
    through would mask the missing fallback).
  - `on_miss="continue"`: the step SHALL set `CamelCachePeekHit=false` and
    `CamelCachePeekStale=false` on the exchange, SHALL leave the body unchanged, and
    SHALL continue the pipeline.

All three steps SHALL use `OutcomeSegment`
(Segment-not-Process per ADR-0023) so that `PipelineOutcome` propagates correctly through
sub-pipelines. If `on_miss` returns `PipelineOutcome::Stopped`, the cache step SHALL
propagate `Stopped` WITHOUT writing back to the repository (no point caching a stopped
branch). If `on_miss` returns `Err`, the cache step SHALL propagate `Err` WITHOUT writing
back. If the repository `get` or `set` returns `Err`, the step SHALL propagate `Err`.

#### Scenario: cache hit short-circuits on_miss

- **GIVEN** a route with `cache: { repository: memory, key: "k", ttl: 1h, on_miss: [ <expensive fetch> ] }`
  and the memory repository already holds a fresh entry under `"k"`
- **WHEN** the route executes one exchange
- **THEN** the body is the cached entry and the `on_miss` sub-pipeline does not run

#### Scenario: cache miss runs on_miss, sets, and continues

- **GIVEN** the same route and an empty repository
- **WHEN** the route executes one exchange
- **THEN** the `on_miss` sub-pipeline runs, the resulting body is `set` under `"k"` with
  the ttl, and the exchange continues downstream with the fresh body

#### Scenario: cache miss with oversized materialized body skips write-back

- **GIVEN** a route with `cache: { ..., max_entry_bytes: 1024, on_miss: [ <produces Body::Bytes of 2 KiB> ] }`
- **WHEN** the route executes on an empty repository
- **THEN** the `on_miss` sub-pipeline runs, the original `Body::Bytes` passes through
  unchanged, no `set` is called, a `debug`-level log record is emitted, and the exchange
  continues with the fresh body

#### Scenario: cache miss with oversized stream propagates Err

- **GIVEN** a route with `cache: { ..., max_entry_bytes: 1024, on_miss: [ <produces Body::Stream exceeding 1 KiB> ] }`
- **WHEN** the route executes on an empty repository and `Body::into_bytes(1024)` returns
  `StreamLimitExceeded`
- **THEN** the cache step propagates `Err(CamelError)` (the consumed stream cannot be
  re-served; oversized streams are a hard error, not a silent passthrough)

#### Scenario: cache on_miss Stopped propagates without write-back

- **GIVEN** a route with `cache: { ..., on_miss: [ <filter that returns Stopped> ] }`
- **WHEN** the route executes on an empty repository and `on_miss` returns `Stopped`
- **THEN** the cache step propagates `PipelineOutcome::Stopped` downstream and `set` is
  NEVER called on the repository

#### Scenario: cache on_miss Err propagates without write-back

- **GIVEN** a route with `cache: { ..., on_miss: [ <step that returns Err> ] }`
- **WHEN** the route executes on an empty repository and `on_miss` returns `Err`
- **THEN** the cache step propagates `Err` downstream and `set` is NEVER called

#### Scenario: cache repository get Err propagates as Err

- **GIVEN** a route with `cache: { repository: custom, ... }` where the `"custom"`
  repository's `get` returns `Err`
- **WHEN** the route executes
- **THEN** the cache step propagates `Err` (Contract C1 — backend failure never silently
  becomes a miss)

#### Scenario: cache repository set Err propagates as Err

- **GIVEN** a route with `cache: { ... }` on an empty repository whose `set` returns `Err`
- **WHEN** the route executes, `on_miss` runs successfully, and the write-back `set` fails
- **THEN** the cache step propagates `Err` (the write-back failure is not silently swallowed)

#### Scenario: cache_peek_stale serves post-expiry entry

- **GIVEN** a repository with `set("k", entry, Some(1ms))`, 10ms elapsed, and a route step
  `cache_peek_stale: { repository: memory, key: "k" }`
- **WHEN** the route executes
- **THEN** the exchange body is the post-expiry cached entry

#### Scenario: cache_peek_stale HIT sets peek properties

- **GIVEN** a repository holding a post-expiry entry under `"k"` and a route step
  `cache_peek_stale: { repository: memory, key: "k" }`
- **WHEN** the route executes
- **THEN** the exchange properties `CamelCachePeekHit=true` and `CamelCachePeekStale=true`
  are set and the body is the stale entry

#### Scenario: cache_peek_stale on absence Stops the branch

- **GIVEN** an empty repository and a route step
  `cache_peek_stale: { repository: memory, key: "absent" }`
- **WHEN** the route executes
- **THEN** the step sets `PipelineOutcome::Stopped` for the current branch (does NOT
  pass through with an unchanged body), `CamelCachePeekHit=false` and
  `CamelCachePeekStale=false` are set on the exchange, and one `debug`-level log record
  naming the step and repository is emitted

#### Scenario: cache_peek_stale on_miss continue passes through on absence

- **GIVEN** an empty repository and a route step
  `cache_peek_stale: { repository: memory, key: "absent", on_miss: continue }` followed
  by a `log` step
- **WHEN** the route executes
- **THEN** the pipeline reaches the `log` step with the body unchanged,
  `CamelCachePeekHit=false` and `CamelCachePeekStale=false` are set, and no
  `PipelineOutcome::Stopped` is returned

#### Scenario: cache_peek_stale on_miss invalid value fails compile

- **GIVEN** a route step `cache_peek_stale: { repository: memory, key: "k", on_miss: skip }`
- **WHEN** the route compiles
- **THEN** compilation fails with an error naming the invalid `on_miss` value

#### Scenario: cache_peek_stale key expression None Stops with debug log

- **GIVEN** a route step `cache_peek_stale: { repository: memory, key: <expression that
  resolves to None> }`
- **WHEN** the route executes
- **THEN** the step sets `PipelineOutcome::Stopped` for the current branch and one
  `debug`-level log record naming the step and repository is emitted

#### Scenario: cache_invalidate removes the key

- **GIVEN** a repository holding `"k"` and a route step
  `cache_invalidate: { repository: memory, key: "k" }` followed by `cache: { ..., key: "k" }`
- **WHEN** the route executes
- **THEN** the second step misses (`on_miss` runs) because the first step removed the entry
