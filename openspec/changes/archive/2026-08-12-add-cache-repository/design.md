# Design: add-cache-repository

## Approach

Introduce `CacheRepository` as a **third repository port** alongside `IdempotentRepository`
and `ClaimCheckRepository`, following the same wiring pattern (ADR-0028 verbatim:
`NamedRegistry<dyn T>` + `register_X_repository`/`X_repository` on `CamelContext`, default
`"memory"` backend registered in `context_builder`). It is NOT a component (ADR-0001 +
ADR-0046) and does NOT extend `ClaimCheckRepository` (would break its payload-ownership
contract per ADR-0028).

**Three architectural rulings drive the design** (full reasoning in
`docs/rulings/cache-repository-ruling-2026-08-11.md` + addendum):

1. **Payload type is `CacheEntry { bytes, content_type, expires_at }`, NOT `Body`.** `Body`
   derives only `Debug/Default/Clone` (`crates/camel-api/src/body.rs:161`) — it is not
   `Serialize`, so no redb/redis backend can store it. `Body::Stream` is single-consumption
   (`body.rs:244-257`, `AlreadyConsumed`) and cannot be re-served. The EIP face materializes
   `Body → CacheEntry` on write via `Body::into_bytes(max_size)` and reconstructs
   `CacheEntry → Body` on read.
2. **In-band `expires_at` everywhere; NO native backend TTL eviction.** `peek_stale` is
   contradictory with native eviction — moka's `Builder::expire_after` or redis `SETEX`
   would physically remove the entry, making `peek_stale` return `Ok(None)` exactly when the
   resilience hook needs it. All backends store `expires_at: Option<SystemTime>` (NOT
   `Instant` — must survive serialization and process restart) inside the entry. `get`
   honors it; `peek_stale` ignores it. Each backend reclaims entries past
   `expires_at + stale_retention` via its own sweep mechanism (memory: lazy on access +
   moka size-eviction; redb: background tokio task; redis: deferred to v1.1).
3. **moka for size-eviction ONLY.** Built with no `Expiry`, no `time_to_live` (verified moka
   0.12.16: default `Expiry = None` = never time-expires). moka provides TinyLFU size
   eviction; the cache layer provides in-band time expiry on top. The two do not interact.

**Stale-on-error is COMPOSED, not baked in.** `CircuitBreakerConfig.fallback:
Option<BoxProcessor>` already exists (`crates/camel-processor/src/circuit_breaker.rs:118`).
Users write `circuitBreaker{ fetch_upstream }.fallback{ cache_peek_stale }`. This is the
advantage over Apache Camel's caffeine-cache (a dumb k/v that forces you to compose
resilience separately). The cache contributes only `peek_stale` and the resilience wiring
is owned by `CircuitBreaker`.

**EIP face is a new `CacheSegment`, NOT `EnrichmentStrategy.on_no_poll` reuse.** That hook
is passthrough-when-empty (`enrichment_strategy.rs:19-21`, returns the original unchanged)
with no write-back. Cache-miss semantics are *run producing sub-pipeline → `set` →
continue* — structurally a `Filter`/`doTry`-shaped `OutcomeSegment`, per ADR-0023's
Segment-not-Process decision for the analogous Idempotent Consumer.

## Affected crates

- **`camel-api`**: new module `cache.rs` — `CacheRepository` trait, `CacheEntry`,
  `ContentType` (`#[non_exhaustive]`), `CacheStats` (`#[non_exhaustive]`). Serde derive
  behind `serde` feature (consistent with `idempotent.rs`/`claim_check.rs`).
- **`camel-core`**: new module `cache/` — `MemoryCacheRepository` (moka), `RedbCacheRepository`.
  New `CamelContext::register_cache_repository`/`cache_repository` methods. Default
  `"memory"` registered in `context_builder.rs` (mirrors lines 212-229 for idempotent/
  claim_check). New `[default.cache_repo]` opt-in wired in `camel-config/src/context_ext.rs`
  (mirrors lines 219-229 for `idempotent_repo`).
- **`camel-config`**: new `cache_repo: Option<CacheRepoConfig>` field on `CamelConfig`,
  mirroring `idempotent_repo`. `CacheRepoConfig` carries a `backend: "memory" | "redb"`
  discriminator (default `"memory"`) plus backend-specific sub-fields (`max_capacity` for
  memory; `path`, `stale_retention`, `max_entries` for redb). Schema, env-var flattening,
  profile section, validation.
- **`camel-dsl`**: new `cache`, `cache_invalidate`, `cache_peek_stale` step kinds in the YAML
  route AST and canonical route spec; step compiler in `step_compilers/`.
- **`camel-processor`**: new `CacheSegment` outcome-composition primitive (if not already
  present) and the cache step service (`CacheService`).
- **`camel-tests`** (integration): `circuitBreaker.fallback{ cache_peek_stale }` end-to-end
  test using the EFFIS-shape workload.
- **`Cargo.toml` (workspace)**: pin `moka = "0.12"` under `[workspace.dependencies]`.
- **`docs/adr/0056-*.md`**: new ADR recording the 6 decisions.
- **`CONTEXT-MAP.md`**: new Key Terms (`CacheRepository`, `CacheEntry`, `Cache EIP`) +
  disambiguation note vs `stream_cache` (which is unrelated OOM protection for streaming
  bodies, ADR separate).

## Architecture boundaries

- **Data plane**: cache step is an `OutcomeSegment` over `BoxProcessor`, like Filter/doTry.
  No `Service::call` for cache ops — the trait methods are called from within the step's
  segment, not from a Tower service wrapper.
- **Control plane**: `CacheRepository` is a port in `camel-api`, mirroring
  `IdempotentRepository`/`ClaimCheckRepository`. No lifecycle hooks (`start`/`stop`) on the
  trait — backends self-manage (moka is sync; redb's sweep task binds to the context's
  lifetime via `CancellationToken`).
- **DSL boundary**: `camel-dsl` defines the step kinds; `camel-core` provides the step
  compiler. `camel-core` does NOT depend on `camel-dsl` (ADR-0008 invariant preserved).
- **Component boundary**: NO `cache://` component. The cache is reached only via the EIP
  step. `camel-component-redis` stays generic-purpose; future `camel-cache-redis` (v1.1)
  will be a separate crate in `services/` impl'ing the trait.
- **Non-exhaustive policy** (ADR-0049): `ContentType`, `CacheStats` are `#[non_exhaustive]`.
  `CacheEntry` is NOT (backends construct it via struct literal — ADR-0049 §Rule 3
  struct-literal exception).
- **Publish topology** (ADR-0055): `moka` becomes a real dep of `camel-core`; no dev-dep
  cycle introduced (camel-core already re-export-free for the api layer).

## Phases

### Phase 1: Foundation — port + memory backend + wiring + ADR

- **Goal:** ship the `CacheRepository` port, types, moka memory backend, `CamelContext`
  wiring, and ADR-0056 + CONTEXT-MAP. No DSL face yet (the port is exercised by unit tests
  in `camel-core`).
- **Dependencies:** `moka` workspace dep pin; ADR-0033 safe-defaults (mandatory
  `max_capacity`); ADR-0028 wiring pattern; ADR-0049 non_exhaustive policy.
- **Externally-visible types/interfaces:** `camel_api::CacheRepository` trait,
  `CacheEntry`, `ContentType`, `CacheStats`, `CamelContext::register_cache_repository`,
  `CamelContext::cache_repository`.
- **Deliverable:** trait + memory backend wired as `"memory"` default; ADR-0056 accepted;
  CONTEXT-MAP Key Terms updated.
- **Exit-criteria:** unit tests prove `get`/`set`/`peek_stale`/`invalidate`/`clear`/`stats`
  on `MemoryCacheRepository`; `peek_stale` returns entry after in-band expiry; mandatory
  `max_capacity` enforces size bound; `cargo test -p camel-core --lib` green; clippy/fmt
  clean; `lint-non-exhaustive` passes on the new enums.

### Phase 2: EIP face — DSL steps + step compiler + CacheSegment

- **Goal:** expose the cache to YAML/JSON routes via `cache`, `cache_invalidate`, and
  `cache_peek_stale` steps. The user can now write `cache:` in a route.
- **Dependencies:** Phase 1; ADR-0023 Segment-not-Process precedent; `OutcomeSegment`
  machinery in `camel-core/src/lifecycle/adapters/outcome_composition.rs`.
- **Externally-visible types/interfaces:** `RouteDslStep::Cache`, `RouteDslStep::CacheInvalidate`,
  `RouteDslStep::CachePeekStale`, the canonical spec step kinds, the step compiler
  registration.
- **Deliverable:** three step kinds parse, compile, and execute end-to-end against the
  default `"memory"` repository.
- **Exit-criteria:** YAML routes using each step compile and run in `camel-tests`;
  `cache:` with `on_miss` sub-pipeline fetches upstream → `set`s → continues on miss, and
  short-circuits with cached body on hit; `cache_peek_stale` serves post-expiry entries;
  `cache_invalidate` removes keys; canonical route spec schema accepts the new steps.

### Phase 3: Persistence + observability — redb backend + Camel.toml + OTel + integration test

- **Goal:** ship the opt-in persistent backend, stats plumbing, and the resilience
  composition demonstration.
- **Dependencies:** Phase 2; redb = "4" (already in workspace); Camel.toml config pattern
  (`[default.idempotent_repo]` reference); OTel metrics path (camel-processor CONTEXT.md).
- **Externally-visible types/interfaces:** `RedbCacheRepository`, `CamelConfig.cache_repo:
  Option<CacheRepoConfig>` (with `backend: "memory" | "redb"` discriminator), `[default.cache_repo]`
  profile section, `CacheStats` populated by both backends, OTel metric instruments
  `camel.cache.{hits,misses}`.
- **Deliverable:** redb backend opt-in via Camel.toml (`backend = "redb"`); sweep task bound
  to context `CancellationToken`; integration test demonstrating
  `circuitBreaker{ fetch }.fallback{ cache_peek_stale }` resilience on the EFFIS shape.
- **Exit-criteria:** `[default.cache_repo] backend = "redb"` registers `"persistent"` redb
  alongside the default `"memory"` (single registration — configured capacity applied before
  the default-memory registration, no duplicate); `max_entries` is optional with a backend
  default; entries survive handle drop+reopen; `peek_stale` works post-expiry on redb
  (regression test); sweep task stops cleanly on context shutdown; OTel metrics observable
  in test; integration test passes; `lint-context-citations` clean.

## Alternatives considered

1. **`cache://` component** — REJECTED (ADR-0001 + ADR-0046). Reproduces the
   `value_to_redis_arg.to_string()` type-destruction antipattern verified at
   `crates/components/camel-component-redis/commands/mod.rs:63-67`.
2. **Extend `ClaimCheckRepository` with TTL** — REJECTED. Breaks its payload-ownership
   contract (ADR-0028); same logic that kept Idempotent and ClaimCheck as separate traits.
3. **`Body` as stored type** — REJECTED. `Body` is not `Serialize`; `Body::Stream` is
   single-consumption (ruling §1).
4. **moka with custom `Expiry`** — REJECTED. Conflicts with `peek_stale`; moka would
   physically evict entries the resilience hook needs. moka is built for size-eviction only.
5. **Native backend TTL eviction** — REJECTED. Makes `peek_stale` semantics non-uniform
   across backends; in-band expiry is the prerequisite for stale-on-error composition.
6. **`on_no_poll` reuse for cache miss** — REJECTED. That hook is passthrough; no
   fetch-and-write-back semantics (ruling §2).
7. **redb as default backend** — REJECTED by kenny (addendum Change 2). Memory default is
   consistent with `IdempotentRepository`/`ClaimCheckRepository`; persistence is opt-in.

## ADR-0056 will record these 6 decisions

(per e_opus addendum §4, verbatim)

1. **Memory-default despite the anchor** — consistency with Idempotent/ClaimCheck
   (`context_builder.rs:212-229`); persistence is a documented one-line config, with the
   EFFIS `[default.cache_repo] backend = "redb"` worked example:
   ```toml
   [default.cache_repo]
   backend = "redb"          # "memory" (default) | "redb"
   path = "data/cache.redb"  # required when backend = "redb"
   stale_retention = "168h"  # keep stale entries 7 days past expires_at
   max_entries = 100000      # optional cap
   ```
2. **In-band `expires_at` everywhere (no native TTL)** — uniform `peek_stale`; **correctness
   prerequisite** for the stale-on-error composition (not merely "uniform semantics"). Record
   the `SystemTime`-not-`Instant` decision and the clock-skew caveat.
3. **moka = size-eviction only, TTL is in-band** — explicit note that no custom `Expiry` and
   no `time_to_live` are configured, so moka never time-expires (this is the fact that makes
   `peek_stale` work on the memory tier).
4. **Mandatory `max_capacity` on the memory default** — ADR-0033 safe-defaults +
   `AggregatorConfig::validate()` D-A5 precedent; an unbounded default memory cache is
   rejected. Default 10_000 entries.
5. **Retention window ≠ TTL** — the persistent tiers reclaim at `expires_at + retention`,
   not at `expires_at`; this is what gives `peek_stale` post-expiry reach on redb/redis.
   Without this recorded, an implementer would "helpfully" reclaim at `expires_at` and
   silently break resilience.
6. **No `sweep()` on the trait; reclamation is per-backend** — records why (contract honesty;
   idempotent/claimcheck precedent), so a future contributor does not add it.
