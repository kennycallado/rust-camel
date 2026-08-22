# Design: redis-repositories

## Approach

A dedicated repository service crate implements both repository ports, each over its
own multiplexed Redis connection built from its own configuration. The design verdict (consultation
`docs/consultations/redis-repositories-design-verdict.md`, 2026-08-17) and the
pattern audit (`docs/consultations/redis-repositories-pattern-audit.md`,
2026-08-17) fixed the key decisions:

1. **Placement.** Repository service crate `crates/services/camel-redis-repo`,
   depending on `camel-api` (ports) and `camel-component-redis`
   (connection machinery). Repositories are context-scoped named
   infrastructure: the Runtime owns them by name, routes resolve them at
   compile time, and EIP steps share them across routes — the same
   structural role as the named decision sources in `camel-auth`. Their
   reads and writes run during Exchange processing, while configuration
   and named registration happen at context build time. Keep them in a
   service crate, separate from the URI-scheme Component contract.
2. **Reuse, not reimplementation.** The service crate builds its
   connection from the component's `RedisTopology` +
   `topology_from_config` + `MultiplexedExecutor`, and issues
   repository-level commands (`SCAN`, `UNLINK`, `SET NX`, `SET … EXAT`,
   `EXISTS`, `GET`, `DEL`) as `redis::Cmd` values executed through the
   crate's `RepoCommandExecutor` seam; the production executor pipes
   each `Cmd` over the `MultiplexedConnection` returned by
   `MultiplexedExecutor::get_conn` (via `Cmd::query_async`). The
   component's `RedisCommand` dispatch path does not model these
   commands and stays untouched. The additive seam widening on
   `camel-redis`: `topology_from_config` `pub(crate)` to `pub`,
   `get_conn` `pub(crate)` to `pub`, a new `pub async fn refresh(&self)`
   (an `&self` analogue of the existing `&mut self` `reconnect`, needed
   because every repository trait method takes `&self`), plus the
   re-export of `topology_from_config` at the crate root. No third
   "low-level client" crate: the component already is the shared client
   layer.
3. **Wiring at the composition root.** `CacheRepoConfig`, the backend
   match, and registration already live in `camel-config` (not
   `camel-core`). The Redis backend follows the same seam as redb.
   `camel-core` is untouched.
4. **Sentinel.** The service crate enables sentinel support
   unconditionally: its manifest sets both
   `camel-component-redis = { features = ["sentinel"] }` and
   `redis = { features = [..., "sentinel"] }`. Feature unification
   consequence: any application graph that links `camel-redis-repo`
   compiles the component WITH its sentinel support. The component's
   default-off `sentinel` feature and its fail-closed rejection stay
   unchanged for builds that do not link the service crate. Selection is
   explicit: `sentinel_nodes` + `master_name` fields on the repository
   config. No URI scheme, no auto-detection. Auto-detection is rejected:
   it contradicts ADR-0033 (fail-closed, validate at startup) and
   ADR-0032 (operator-owned names never inferred), and a sentinel probe
   cannot produce a positive signal without `master_name` already known.
   The controlling branch for the reused code is
   `cfg(feature = "sentinel")` in `camel-component-redis`
   (topology.rs:293-329), so the component feature is the one that must
   be enabled — the driver feature alone compiles the wrong branch.
5. **Cluster.** Rejected at config validation for repositories. Cluster
   `SCAN` runs per node and would under-report `clear()`; the component's
   cluster support is not production-ready either.

### Command executor seam (test injection)

The repositories do not hold the `MultiplexedConnection` directly; they
issue commands through one internal trait:

```rust
#[async_trait]
pub(crate) trait RepoCommandExecutor: Send + Sync {
    async fn execute(&self, cmd: redis::Cmd) -> Result<redis::Value, CamelError>;
    async fn refresh(&self) -> Result<(), CamelError>;
}
```

- Production: `MultiplexedRepoExecutor` wraps the component's
  `MultiplexedExecutor` (`execute` clones the connection via `get_conn`
  and pipes the `redis::Cmd`; `refresh` delegates to the widened
  `refresh(&self)`).
- Unit-test fake: `FakeRepoExecutor` records every issued `redis::Cmd`
  and returns scripted `redis::Value` results or errors, with call and
  refresh counters (asserts "issued exactly once", retry-once behavior,
  EXAT arguments).
- Construction fake: `FakeStaticTopology` is a service-crate-local
  implementation of the component's pub `RedisTopology` trait returning
  scripted clients, for constructor tests (single, sentinel, cluster
  rejection). The component's `FakeTopology` is `#[cfg(test)]`-gated
  (its `lib.rs` re-export is a dead path for downstream non-test builds)
  and not importable.

Constructor surface (NEW symbols) — `connect` is `async fn`; construction
performs an EAGER connection (fail fast on an unreachable topology, and
resolve-once semantics at construction). The blocking sentinel resolve is
offloaded internally by the component (`topology.rs:270-275`); the
service crate does not add its own `spawn_blocking`:

- `RedisCacheRepository::connect(name: &str, endpoint: &RedisEndpointConfig,
  key_prefix: &str, stale_retention: Duration) -> impl Future<Output = Result<Self, CamelError>>`
  (production, async; the configured `key_prefix` MUST reach the
  repository through this parameter)
- `RedisCacheRepository::with_executor(name: &str, key_prefix: &str,
  stale_retention: Duration, clock: ClockFn, executor: Arc<dyn RepoCommandExecutor>) -> Result<Self, CamelError>`
  (test injection, sync, validates name/prefix)
- `RedisIdempotentRepository::connect(name, endpoint, key_prefix)` (async) /
  `RedisIdempotentRepository::with_executor(name, key_prefix, executor)` (sync)
- `keyspace::namespaced(prefix, repo_name, key) -> String`

Both structs carry manual `impl Debug` printing only the name/prefix (and
retention for the cache repo): the trait requires `Debug`, and
`ClockFn`/`Arc<dyn RepoCommandExecutor>` are not `Debug`.

Both constructors validate the repository `name` with the same rule as
`key_prefix`: non-empty, charset `[A-Za-z0-9:_-]`, rejecting glob
metacharacters — the name is part of every SCAN pattern, so an unsafe
name would break `clear()` scoping exactly like an unsafe prefix.
Invalid name or prefix returns `Err(CamelError::Config)` naming the
argument.

The endpoint input reuses the component's pub `RedisEndpointConfig` and
`SentinelConfig`; no new endpoint type.

### Connection lifecycle

Each repository owns one `MultiplexedExecutor`, constructed from that
repository's own configuration (`url` or `sentinel_nodes` + `master_name`).
Two repositories that happen to target the same server hold two multiplexed
connections; this is intentional. Multiplexed connections are cheap (one
socket each, concurrent awaits pipelined), and per-repository ownership
keeps independently configured repositories fully isolated. There is no
cross-repository connection registry in this change. Construction
performs an EAGER connection (fail fast on an unreachable topology);
the sentinel resolve happens once at construction (the component offloads
the blocking sentinel resolve internally, topology.rs:270-275), not per
operation. On a connection error the
repository calls `refresh()` (executor-level reconnect: drop the cached
connection, re-resolve through the topology) so later operations use a
healthy connection. The sentinel master address is never cached across
reconnects (redis-failover spec, resolution seam).

### Keyspace and clear() safety

Each repository builds keys as `camel:cache:{repo-name}:{key}` or
`camel:idem:{repo-name}:{key}`. Prefixes are disjoint by construction.
`clear()` runs `SCAN MATCH {prefix}* COUNT <batch>` in a loop, then
`UNLINK` the matched keys in batches. `FLUSHDB` and `FLUSHALL` are
forbidden. `key_prefix` is validated to the charset
`[A-Za-z0-9:_-]` — non-empty and free of glob metacharacters
(`*`, `?`, `[`, `]`) and whitespace — so `SCAN MATCH` can never exceed
the promised namespace. When both `cache_repo` and `idempotent_repo`
are `backend = "redis"` and resolve to the same effective endpoint and
database, validation requires distinct effective prefixes (the defaults
`camel:cache` and `camel:idem` already are distinct).

### Cache semantics (ADR-0056)

- Value format: `serde_json(CacheEntry)`, identical to the redb backend.
- `set`: `set(key, value, ttl)` computes `entry.expires_at =
  ttl.map(|d| now + d)` (identical to the memory/redb backends), then
  serializes the entry and issues ONE command. When `expires_at` is
  `Some(t)`: `SET {namespaced_key} {blob}` with
  `SetExpiry::EXAT((t + stale_retention) as unix-seconds)` — a single
  atomic write; there is no separate `EXPIREAT` command, so an
  immortal-key window is structurally impossible. When `expires_at` is
  `None`: plain `SET` with no expiry option. The Redis TTL is reclamation
  only; expiry semantics stay in-band. On timestamp overflow
  (`SystemTime::checked_add` fails), the write falls back to plain `SET`
  with no TTL rather than failing (mirrors the redb `checked_add` guard).
- `get`: fetch, deserialize, return `None` when in-band `expires_at` has
  passed, `Err` on transport failure (Contract C1).
- `peek_stale`: fetch and return regardless of in-band expiry. Satisfiable
  because reclamation happens at `expires_at + retention`.
- `invalidate`: `UNLINK` the namespaced key.
- `invalidate_prefix(prefix)`: overrides the trait default (which fails
  closed — main added the method wired to the `cache_invalidate
  { key_prefix }` DSL step). Implementation reuses the `clear()` SCAN +
  UNLINK machinery against `{key_prefix}:{repo-name}:{prefix}*` and
  returns the removed count. The resolved step prefix is validated with
  the same glob-metacharacter charset guard before being embedded in the
  SCAN pattern (untrusted-data trust boundary, ADR-0032: the step prefix
  is a simple-language expression resolved from exchange data).
- `async fn stats` (the trait method is async on main): hits and misses
  are tracked in process; `entries` and
  `evictions` are always zero (Redis-side eviction and entry counts are
  not observable through the repository path; non-tracking semantics per
  the base eip-cache spec).

### Idempotent semantics (ADR-0023)

- `add`: `SET key 1 NX`. `Ok(true)` when set, `Ok(false)` when the key
  existed. The repository issues exactly one `SET NX` per trait call and
  never re-issues it after a transport error: a lost response leaves the
  outcome unknown (the insert may have succeeded on the server), so a
  retry could observe its own key and wrongly return `Ok(false)`, while
  assuming success could return `Ok(true)` for a write that never landed.
  On any transport error, `add` returns `Err(CamelError::Io)` immediately
  and may call `refresh()` so the NEXT call uses a healthy connection —
  Contract C1: an unknown outcome stays unknown.
- `contains`: `EXISTS`. `remove`: `UNLINK`. `clear`: prefix `SCAN` +
  `UNLINK` as above. No TTL on keys (out of scope, parity with redb).

### Retry and reconnect policy

The repository layer does not use the component's `execute_with_retry`
(it drives the `RedisCommand`/`Exchange` dispatch path and requires
`&mut self`). Each operation is classified as an outcome-bearing write or
a retry-safe command:

| Operation | Command | Class | On transient transport error |
|---|---|---|---|
| idempotent `add` | `SET NX` | outcome-bearing | no re-issue; `Err(Io)` now; `refresh()` for future calls |
| cache `set` | `SET` / `SET EXAT` | retry-safe (last-writer-wins) | `refresh()` + re-issue once |
| `get` / `peek_stale` | `GET` | retry-safe | `refresh()` + re-issue once |
| `contains` | `EXISTS` | retry-safe | `refresh()` + re-issue once |
| `invalidate` / `remove` | `UNLINK` | retry-safe | `refresh()` + re-issue once |
| `clear` | `SCAN` + `UNLINK` | retry-safe | `refresh()` + re-issue the failed batch |

`SET NX` is the only non-idempotent command in the repository surface;
every other command is idempotent under re-issue. Retry-safe operations
retry at most once after a single `refresh()`, with no backoff loop.
Transience is judged with the component's `is_transient_redis_error`
classifier so behavior matches the component.

### Error mapping

Transport and command failures map to `CamelError::Io` (transient,
C1-critical path, keeps `classify() == "io"` meaningful). Config,
validation, and construction failures map to `CamelError::Config`. No new
error variant. A malformed `stale_retention` value is a `CamelError::Config`
naming the offending field — the existing silent seven-day fallback in the
redb `context_ext` path is a defect and MUST NOT be copied.

### Config validation matrix (fail-closed, ADR-0033)

For each of `cache_repo` / `idempotent_repo` with `backend = "redis"`,
validation rejects with an error that names the dotted field
(`cache_repo.<field>` / `idempotent_repo.<field>`) and states the violated
rule:

- neither `url` nor `sentinel_nodes` set (no topology)
- both `url` and `sentinel_nodes` set (mutual exclusion)
- empty `sentinel_nodes` list, or any empty node entry
- empty `master_name` when `sentinel_nodes` is set
- `master_name`, `sentinel_username`, or `sentinel_password` set without
  `sentinel_nodes` (orphan fields)
- `url` scheme other than `redis://` or `rediss://`
- any cluster topology field (unsupported for repositories)
- `key_prefix` that is empty or contains characters outside
  `[A-Za-z0-9:_-]` (glob metacharacters would break `clear()` scoping)
- `stale_retention` that fails humantime parsing (cache only)
- cache and idempotent both `redis` on the same effective endpoint and
  database with identical effective prefixes

### Credential redaction (ADR-0051)

`CacheRepoConfig` and `IdempotentRepoConfig` gain credential-bearing
fields (`url` userinfo, `sentinel_username`, `sentinel_password`). Both
structs carry manual `Debug` implementations that redact: URL userinfo
rendered as `redis://***@host`, and sentinel credentials rendered as
`***`. Tests assert formatted output omits every credential literal.
`cargo xtask lint-secrets` is part of the phase gates.

## Affected crates

- `crates/services/camel-redis-repo` (new): publishable crate. Manifest
  carries workspace-inherited fields (`version.workspace`, `edition.workspace`,
  `license.workspace`, `repository.workspace`, `homepage.workspace` — the
  workspace is edition 2024, MSRV 1.89; do not hard-code an edition),
  plus `description`, `documentation` (docs.rs URL), `readme`,
  `keywords`, `categories`, and `[lints] workspace = true`. The root
  `Cargo.toml` gains the `[workspace.dependencies]` entry
  `camel-redis-repo` following the exact-version path pattern of the
  other service crates. Content: `RepoCommandExecutor` seam,
  `MultiplexedRepoExecutor`, `FakeRepoExecutor`, `FakeStaticTopology`,
  keyspace helpers, error mapping, `RedisCacheRepository`,
  `RedisIdempotentRepository`, unit tests. The cache repository
  constructor takes `stale_retention: Duration` (like the redb backend) and
  uses it to compute the `EXAT` timestamp; `context_ext` passes the config
  field through. Includes `README.md`.
- `crates/components/camel-redis`: additive connection-seam widening —
  `topology_from_config` `pub(crate)` to `pub`, `MultiplexedExecutor::get_conn`
  `pub(crate)` to `pub`, new `pub async fn MultiplexedExecutor::refresh(&self)`,
  plus the crate-root re-export of `topology_from_config`. No behavior change.
- `crates/camel-config`: `CacheRepoConfig` gains `backend = "redis"` and
  Redis fields (`url`, `sentinel_nodes`, `master_name`,
  `sentinel_username`, `sentinel_password`, `key_prefix`, `stale_retention`
  reuse). `idempotent_repo` generalizes from
  `Option<RedbIdempotentConfig>` to `Option<IdempotentRepoConfig>` — a
  `#[serde(deny_unknown_fields)]` STRUCT (not a serde-tagged enum) with
  `backend: String` and `#[serde(default = "default_idempotent_backend")]`
  (default `"redb"`), so existing TOML that sets only `path` and
  `durability` keeps parsing. Fields: `backend`; redb: `path`,
  `durability`; redis: `url`, `sentinel_nodes`, `master_name`,
  `sentinel_username`, `sentinel_password`, `key_prefix` (default
  `camel:idem`). The redb-only fields on `CacheRepoConfig` (`path`,
  `cache_size`, `sweep_interval`, `max_entries`) are NOT read by the
  redis validation branch and are not required when `backend = "redis"`
  (the struct is shared; the branch simply ignores them). Validation
  implements the full matrix above; `context_ext`
  registers both repositories under `"redis"`. Manual redacting `Debug`
  for both structs.
- `crates/camel-test`: new integration test
  `tests/redis_repositories_test.rs` under the existing
  `integration-tests` feature (testcontainers, self-provisioned Redis and
  Sentinel topology — the `redis_sentinel_test.rs` pattern, never
  `#[ignore]`, per ADR-0054). `camel-redis-repo` is an optional dependency
  of `camel-test` under `integration-tests` (leaf direction only; the
  service crate never depends on `camel-test`).
- `.github/workflows/ci.yml`: add
  `cargo test -p camel-test --features integration-tests --test redis_repositories_test`
  alongside the existing `redis_test` target.

## Architecture boundaries

Repository service vs Component: the crate holds context-scoped named
infrastructure consumed by EIP steps. It implements no `Component`
contract, owns no URI scheme, and creates no endpoints. Dependency chain stays acyclic:
`camel-api <- camel-redis <- camel-redis-repo <- camel-config` and
`camel-redis-repo <- camel-test (optional, integration-tests)`.
`camel-core` imports no component crate; the hexagonal boundary test is
unaffected. DSL surface is untouched: the cache and idempotent steps
resolve repositories by name from the context registry, so `"redis"`
becomes usable from YAML with no DSL change. Lifecycle note (ADR-0028
StepLifecycle rule): repositories need no explicit lifecycle — the
multiplexed connection is established lazily on first use and dropped
with the executor handle; there is no consumer task and no background
task. The new ADR records this justification.

## Phases

### Phase 1: seam and service-crate foundation

- **Goal:** the repository service crate exists as a publishable,
  workspace-hygenic crate and can open a connection (single, sentinel,
  TLS) through the component's widened seams; the command executor seam,
  keyspace, and error mapping are in place and unit-tested.
- **Dependencies:** none (first phase).
- **Externally-visible types/interfaces:** on `camel-component-redis`:
  `pub topology_from_config`, `pub MultiplexedExecutor::get_conn`, `pub
  async fn MultiplexedExecutor::refresh(&self)`, crate-root re-export of
  `topology_from_config`. In `camel-redis-repo`: `RepoCommandExecutor`
  (crate-internal trait), `MultiplexedRepoExecutor`, `FakeRepoExecutor`,
  `FakeStaticTopology`, `keyspace::namespaced`, error mapping helper.
- **Deliverable:** root `Cargo.toml` workspace dependency entry;
  publishable manifest + `README.md`; the seam widening commits on
  `camel-redis`; compiling crate with unit tests.
- **Exit-criteria (commands):** `cargo build -p camel-redis-repo`;
  `cargo test -p camel-redis-repo` green (FakeStaticTopology
  construction for single and sentinel topologies; cluster rejection;
  FakeRepoExecutor recording and counters); `cargo clippy -p
  camel-redis-repo -p camel-component-redis -- -D warnings` clean;
  `cargo xtask lint-publish-cycles` clean.

### Phase 2: RedisCacheRepository

- **Goal:** the cache backend works end to end: trait impl, camel-config
  acceptance of `backend = "redis"`, registration under `"redis"`, real
  Redis integration coverage.
- **Dependencies:** Phase 1.
- **Externally-visible types/interfaces:**
  `camel_redis_repo::RedisCacheRepository` (`connect` + test
  `with_executor` constructor); `CacheRepoConfig` Redis fields.
- **Deliverable:** trait impl with unit tests over `FakeRepoExecutor`
  (EXAT-vs-plain SET, in-band expiry, `peek_stale`, C1 `Err`,
  `invalidate`, `invalidate_prefix` (one-namespace purge + step-prefix
  guard), retry-once on `set`, clear scoped to prefix with a
  foreign-prefix survivor, stats one-hit-one-miss); camel-config wiring
  (fields, full validation matrix, redacting `Debug` + redaction tests,
  registration under `"redis"`); camel-test
  `tests/redis_repositories_test.rs` cache section (non-ignored,
  testcontainers); CI target.
- **Exit-criteria (commands):** `cargo test -p camel-redis-repo -p
  camel-config` green (including new validation and redaction tests);
  `cargo test -p camel-test --features integration-tests --test
  redis_repositories_test` green; `cargo xtask lint-secrets` clean;
  existing memory/redb config tests still pass.

### Phase 3: RedisIdempotentRepository and documentation

- **Goal:** the idempotent backend works end to end, the idempotent
  config is generalized, and the change is fully documented per the
  documentation-governance pattern.
- **Dependencies:** Phase 2.
- **Externally-visible types/interfaces:**
  `camel_redis_repo::RedisIdempotentRepository` (`connect` + test
  `with_executor` constructor); `IdempotentRepoConfig` on `CamelConfig`.
- **Deliverable:** trait impl with unit tests (SET NX atomicity, no
  re-issue after lost response with counters, clear scoping); camel-config
  generalization with backward-compat tests (existing redb TOML parses
  unchanged), validation matrix, redaction; camel-test integration
  idempotent + sentinel-failover sections; ADR-0063 "Redis repository
  service" with `Amends: ADR-0023, ADR-0056` metadata, reciprocal
  `Amended by: ADR-0063` notes on both older ADRs' future-backend
  placement paragraphs, cross-reference (no amendment) in ADR-0028, and
  the no-lifecycle justification; `crates/services/camel-redis-repo/CONTEXT.md`
  with sections for scope boundary, language, connection/retry ownership,
  sentinel feature posture, keyspace and `clear()` safety, credential
  redaction, test seams (`FakeRepoExecutor`, `FakeStaticTopology`,
  camel-test integration), dependency boundary, and lifecycle/crash
  ownership, with symbol/path citations that pass
  `cargo xtask lint-context-citations`; `crates/services/CONTEXT.md`
  gains the "Repository service" definition (context-scoped, named
  infrastructure used during Exchange processing, not necessarily a
  `Lifecycle` implementation) with `_Avoid_: service (unqualified),
  Component, repository adapter crate`; `CONTEXT-MAP.md` gains a nested
  `Redis Repository Service` Contexts entry, a Services-to-Runtime
  repository-registration relationship, Key Terms ("Redis repository
  backend", "repository service crate") citing ADR-0063, the ADR-0063
  index entry, and backend-list updates in the touched context docs
  (`crates/camel-api/CONTEXT.md`, `crates/camel-core/CONTEXT.md`) —
  APPENDING the Redis backend to the existing CacheRepository Key Term
  (which already documents `invalidate_prefix` and async `stats` on
  main), never rewriting main's existing sentences; mdBook updates
  (`docs/src/configuration/schema.md` redis backend
  reference, `docs/src/eip/cache.md` and
  `docs/src/eip/idempotent-consumer.md` backend lists); new
  `examples/redis-repositories/` with an anchored, checked-in
  `Camel.toml` + example source.
- **Exit-criteria (commands):** `cargo test -p camel-redis-repo -p
  camel-config -p camel-test --features integration-tests --test
  redis_repositories_test` green; `cargo xtask lint-context-citations`
  clean; `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0;
  existing redb TOML fixture parses with `backend = "redb"`.

## Alternatives considered

- **Repositories inside `camel-redis`** (option A). Rejected: mixes
  repository infrastructure into a URI-scheme Component with a different
  lifecycle, consumer, and trait surface.
- **A shared low-level client crate extracted from the component.**
  Rejected: premature third publish node; the component already exposes
  the needed seams publicly (after two visibility widenings, one new
  `refresh` method, and a crate-root re-export).
- **Generic factory-hook registry in `camel-config`.** Rejected:
  over-engineering for a third backend; the existing direct match at the
  composition root is simpler and already established by redb.
- **Sentinel auto-detection (probe the endpoint).** Rejected: no positive
  signal without `master_name`; contradicts fail-closed validation culture
  (ADR-0033, ADR-0032); adds startup latency and non-determinism.
- **Cluster support in v1.** Rejected: `SCAN` per node under-reports
  `clear()`; a half-working cluster clear is worse than a clear rejection.
- **`#[ignore]`-gated live tests inside the service crate.** Rejected:
  ADR-0054 forbids external-service `#[ignore]` tests; live coverage
  belongs in `camel-test` with testcontainers, never ignored.
