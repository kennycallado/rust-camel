## ADDED Requirements

### Requirement: Repository transient classifications are observable in the component-operations family

The redis repository service crate SHALL emit exactly one
component-operations observation (`component="redis"`, bounded operation
literal, `outcome="failure"`) for every error classified transient by
`is_transient_redis_error` on a repository path, through the
`ComponentMetrics` facade threaded from the construction site. The
repositories SHALL NOT emit for non-transient errors or successful
operations, and the repositories SHALL preserve rc-2or1's atomic counters
and tracing unchanged.

#### Scenario: retry-recovered transient on a cache path is counted once

- **GIVEN** a `RedisCacheRepository` built with a facade over a recording
  collector (components lever on) and a fake executor whose first reply to
  `get` is a transient error and second is `Nil`
- **WHEN** `get` is called and returns `Ok(None)`
- **THEN** the collector recorded exactly one
  `record_component_operation("redis", "get", "failure")` and one
  `increment_errors("redis", "e:redis:get")`, and no success observation

#### Scenario: lost-outcome transient add surfaces Err and is counted

- **GIVEN** a `RedisIdempotentRepository` built with a facade over a
  recording collector (components lever on) and a fake executor whose `add`
  reply is a transient error
- **WHEN** `add` is called
- **THEN** `add` returns `Err`, `transient_refresh_count()` is 1, and the
  collector recorded `record_component_operation("redis", "add", "failure")`
  plus the `e:redis:add` error-family forward

#### Scenario: non-transient error emits nothing

- **GIVEN** a repository built with a facade over a recording collector and
  a fake executor whose reply is a non-transient error
- **WHEN** the operation is called and returns `Err`
- **THEN** the collector recorded no component-operation and no
  error-family forward from the repository

#### Scenario: transient during a scoped clear is counted per failing batch

- **GIVEN** a `RedisCacheRepository` built with a facade over a recording
  collector (components lever on) and a fake executor whose SCAN replies
  succeed and whose first UNLINK batch reply is a transient error followed
  by success
- **WHEN** `clear` is called and completes
- **THEN** the collector recorded exactly one
  `record_component_operation("redis", "clear", "failure")` and one
  `increment_errors("redis", "e:redis:clear")` — one observation per
  transient classification, not per logical call

#### Scenario: components lever off suppresses the family, not the errors

- **GIVEN** a repository built with a facade snapshotted with the
  `[observability.metrics].components` lever off
- **WHEN** a transient classification occurs
- **THEN** no component-operation is recorded and the `e:redis:<operation>`
  error-family forward still reaches the collector

#### Scenario: builders thread the shared handle and lever snapshot

- **GIVEN** a `CamelConfig` with a redis cache or idempotent repo backend
  and `[observability.metrics].components` set
- **WHEN** `configure_context` builds the repository
- **THEN** the repository's facade wraps the context's shared late-bound
  metrics handle with the lever value from that config, so a collector
  registered after construction is still observed
