# Design: redis-state-tier

## Approach

Redis joins the state-adapter class (ADR-0069 section 14) as a thin
adapter over the waist. It reuses the steering resolver, the `poll_until`
driver, and the shared matcher algebra, and adds only its target grammar,
its type-aware projection, and its type labels. No pre-waist copy, no
generic `state:` verb, no shared state-family trait.

**1. Assertion-only family.** There is no `redis:` prepare action. A
scenario route seeds Redis through the landed `redis:` component
producer; the `validate` redis target asserts. bd rc-w8s70 scopes Redis
to assertion surfaces, a raw command-string grammar would need a
tokenizer the family does not otherwise need, and the producer path
already exercises the component writer. The read/write vocabulary split
the sql and surreal prepare action enforces is unnecessary because the
family adds no write grammar.

**2. Target grammar.** A redis target is
`{redis: {datasource, key, type, ttl?}}`. `datasource` and `key` are
document-authored literals, never env-interpolated (the identifier law).
`type` is required and one of `string`, `hash`, `list`, `set`, `zset`.
`deadline` is added to the loader's valid-target list. The `expectation`
reuses `RowsExpectation` verbatim.

The type schema is inherent and drives load-time validation even when
`columns` is absent: `string` `[value]`, `hash` `[field, value]`,
`list` `[index, value]`, `set` `[member]`, `zset` `[member, score]`.
`columns` selects and reorders a non-empty subset of the schema; an
unknown column and a row whose length differs from the effective
projection are load errors naming the action or row index. `ttl` bounds
must be positive whole milliseconds: a sub-millisecond or zero duration
is a load error (never truncated), and the `u128` to `u64` millisecond
conversion is checked, failing the load on overflow.

**3. Atomic coherent snapshot, verdict-typed.** Each snapshot is one
atomic read-only `EVAL` of a fixed Lua script over the single key,
issuing `TYPE`, the declared type's read, and `PTTL` in one server-side
execution; the observed type, payload, and remaining TTL therefore come
from one coherent view — no `TYPE`/read/`PTTL` race is observable. The
script issues only read commands; `redis::Script` needs the non-default
`script` feature, so the family invokes the script through the always
available `redis::cmd("EVAL")`. The executor dispatches in Rust on the
declared type to pick the script read (`GET`, `HGETALL`, `LRANGE`,
`SMEMBERS`, `ZRANGE ... WITHSCORES`). A snapshot returns success data —
observed type, projected rows, remaining TTL — and NEVER `Err`s because
the key is missing or mistyped; `poll_until` stops on `Err`, so a
type mismatch must not be an error or it would defeat the deadline. Type
agreement and the `ttl` bound are judged only by the final `decide`, so
an initially missing key and a transiently wrong-typed key both recover
by the deadline; still missing or still mistyped at expiry fails as a
verdict-class mismatch. Only the count-bound ceiling breach settles
early. A malformed script reply, an unsupported observed type, a driver
error, and a connection failure are apparatus `Err`s that stop the poll.
When the types agree:

- `string`: `GET key` to one row `[value]`; a Nil reply is a null cell.
- `hash`: `HGETALL key` to `[field, value]`, ordered by field.
- `list`: `LRANGE key 0 -1` to `[index, value]`, index a number.
- `set`: `SMEMBERS key` to `[member]`, ordered lexicographically.
- `zset`: `ZRANGE key 0 -1 WITHSCORES` to `[member, score]`, rank order.

Hash and set are ordered at projection so assertions are deterministic;
list and zset keep their server order. `columns` defaults to the schema.

**4. Value law (fail-closed).** RESP values map as: Nil to null, an
integer to a number, a simple or bulk string to a string when valid
UTF-8. A sorted-set score parses to a finite `f64` and normalizes. A
non-integral value is a JSON float. An integral value is emitted as a
JSON integer only when it lies in `[-2^63, 2^63)` — inclusive lower,
exclusive upper — checked BEFORE conversion, with no saturating cast; an
integral value outside that range (`+2^63`, `1e20`) is an apparatus
error. The shared matcher uses exact `serde_json::Number` equality, which
distinguishes `2` from `2.0`, so the document spells integral scores as
integers and the family's normalization makes the Redis lexical form `2`
or `2.0` match the integer `2`; `1.5` stays a float. Any other reply
kind, a wrong-arity reply for the declared type, and a non-finite score
fail closed naming the schema column and the row ordinal — never the
actual member or field identifier, never the key's data. Empty containers
yield zero rows. No silent null, no lossy coercion, no wildcard-matchable
sentinel.

**5. TTL handling.** The optional `ttl` node parses `atLeast` and
`atMost` whole-millisecond durations into a `camel_matchers::CountBound`
(`atLeast` to `AtLeast`, `atMost` to `AtMost`, both to `Range`),
reusing the shared bound algebra. The coherent snapshot reports `PTTL`:
`-2` missing, `-1` persistent, any other negative fails closed, and
`>= 0` is the remaining whole milliseconds. A nonnegative value is
compared with `bound_holds` after a checked `i64`/`u64` to `usize`
conversion that fails closed on overflow. Every legal bound is positive,
so a remaining `0` satisfies every legal `atMost` and fails every legal
`atLeast` and every `Range` (positive minimum). A persistent or missing
key fails a declared bound with a cell-free mismatch. TTL is evaluated on
every snapshot, so a deadline decides it with the final snapshot.

**6. Waist reuse.** The executor has the sql and surreal validate seam
shape: `resolve_datasource::<redis::aio::MultiplexedConnection>` through
the boot catalog with label `redis validation`, then `poll_until` at a
100 ms interval. Redis key contents are not monotone, so there is no
early settle; the only mid-window exit is a count bound's ceiling breach
(`above_ceiling`), identical to sql and surreal.

**7. Datasource registration.** `camel-component-redis` gains a
`RedisPoolFactory` (kind `redis`, supported schemes `redis` and
`rediss`). `create` SHALL NOT reuse `RedisEndpointConfig::from_uri`: that
component parser reads the db from a `db=` query parameter and rejects a
URL path `/1`, and it always discards an ACL username. Instead the
factory uses the driver's own URL parser, `redis::Client::open(db_url)`,
whose grammar is
`redis://[<username>][:<password>@]<host>[:<port>][/<db>]` — ACL username
and database index included. `close` keeps the default no-op; `check`
issues `PING` on a `MultiplexedConnection` handle. A scheme outside
`redis`/`rediss`, a `rediss://` URL when the component `tls` feature is
off, and any URL the driver rejects are config-class errors carrying the
datasource and no raw URL (the resolver sanitizes too). Sentinel and
cluster topologies are rejected: they need component config, not a
datasource URL. `RedisBundle::with_catalog` registers the factory exactly
as `SqlBundle` and `SurrealDbBundle` do. The `camel-bundles` boot
switches the redis bundle to
`bundle_from_config(..).with_catalog(..)` under the `redis` feature; the
lint catalog keeps registering the bundle handle-free.

**8. Demand gate.** `redis = ["dep:redis", "camel-bundles/redis"]` in
`camel-integration-test` (lint-gate-forwarding Rule 1). The grammar
loads in every build; a well-formed redis target fails at action time
naming the `redis` feature when it is off (the sql and surreal split).
`camel-cli` gains `integration-redis`; a dedicated CI job runs the
Docker (testcontainers) battery with deterministic mutations (a route
that writes, deletes, or retypes the key at known points) to cover the
missing-to-present and wrong-type-to-correct recovery paths and the
expiry-failure path. The default suite stays Docker-free.

**9. Redaction and lifecycle (ADR-0051, ADR-0069 section 9).** Mismatch
and fail-closed diagnostics carry only the datasource name, the
document-authored key, the declared and observed type names, the
rendered bound or expected and actual row counts, for a `ttl` bound the
observed remaining milliseconds or the missing/persistent status, and
the schema column names plus row ordinals — never the resolved `db_url`
(driver errors pass the ADR-0051 sanitizer) nor any actual value, member,
or field identifier. The handle lifecycle is drop-scoped, not
close-scoped: the boot's catalog and the boot context's health registry
hold the only references, the redis `close` is the default no-op, and
`close_all` removes neither. The handle is released only when the
boot-owned catalog and context are dropped at boot scope end; no shutdown
step is claimed to release it. Redis state is durable, so cross-boot
isolation is the document author's clean-first responsibility.

## Affected crates

- `camel-integration-test`: `RedisTarget` grammar and loader,
  `redis_validate` executor with the projection and value law, the
  `redis` feature, the deadline target list.
- `camel-component-redis`: `pool_factory.rs`, `RedisBundle::with_catalog`,
  the `MultiplexedConnection` handle export.
- `camel-bundles`: boot wiring of the redis bundle's datasource catalog.
- `camel-cli`: the `integration-redis` feature.
- `.github/workflows`: `integration-redis.yml`.
- `docs`: `scenario-redis.md` plus the SUMMARY entry, the ADR-0069
  section 8 ladder, and the `camel-integration-test` CONTEXT terms.

## Architecture boundaries

Runtime: the executor resolves handles only through the boot's
`DatasourceCatalog`; the boot owns the handle lifecycle. DSL: no change;
scenario documents parse in `camel-integration-test`. Components:
`camel-component-redis` gains only the pool factory and the bundle
catalog hook; component behavior and URI grammar are unchanged. Core
purity fences hold: no tier concept enters core. Hermeticity: the
default suite is untouched; Docker appears only in the dedicated job.

## Alternatives considered

- **A `redis:` prepare action with command strings.** Rejected: it needs
  a command tokenizer, duplicates the landed producer path, and bd
  rc-w8s70 scopes Redis to assertions.
- **Infer the type from `TYPE` instead of declaring it.** Rejected: an
  explicit `type` makes the assertion intentional and gives wrong-type
  behavior a concrete, testable surface.
- **TTL as a projected column.** Rejected: TTL is key-level, the shared
  value matchers cannot express a range, and the count-bound algebra can.
- **Reuse `RedisEndpointConfig::from_uri` for the datasource URL.**
  Rejected: that component parser reads the db from `db=`, rejects a
  `/1` path, and discards the ACL username; `redis::Client::open` uses
  the driver's full URL grammar.
- **Separate `TYPE`, read, and `PTTL` calls.** Rejected: a concurrent
  mutation can split them into an incoherent snapshot; one atomic `EVAL`
  makes the three a single view.
- **Return the type mismatch as a snapshot `Err`.** Rejected: `poll_until`
  stops on `Err`, so an initially missing or transiently wrong-typed key
  could never recover before the deadline. The mismatch is verdict data
  decided at expiry.
- **An embedded in-process Redis.** Rejected: no supported embedded
  server exists; the Docker job is the honest tier-3 carrier.
