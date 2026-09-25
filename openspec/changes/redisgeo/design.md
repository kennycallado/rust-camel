# Design: redisgeo — GEO command support

## Context

Command flow today: `RedisEndpointConfig::from_uri` parses `command=` into a
`RedisCommand` variant (`config.rs`, case-insensitive `FromStr`);
`RedisProducer::resolve_command` lets header `CamelRedis.Command` override
config; `executor.rs::dispatch_command` routes the variant to a family
module (`commands/{string,key,list,set,hash,zset,pubsub,other}.rs`); each
family reads headers, executes, and writes `exchange.input.body`.
Idempotency classification lives in `config.rs::is_idempotent_command`
(reads and retry-safe writes listed; everything else retries never after
an ambiguous failure). The repo crate (`camel-redis-repo`, ADR-0063) wraps
the same transport behind `RepoCommandExecutor` + `execute_retry_safe`,
with namespaced keys `{prefix}:{name}:{key}`.

Relevant rulings: splitbody (commits 938bea3d, 0af13415) — string
elements stay plain strings; only structured rows become JSON objects.
ADR-0063 — repository services live in `camel-redis-repo`, reusing the
component's connection/topology management.

## Decisions

**D1 — Family module.** New `commands/geo.rs` with `is_geo_command`,
`dispatch`, and per-op handlers, mirroring `zset.rs` (closest analog:
typed member + numeric payloads). One new routing arm in
`dispatch_command`. Enum gains `Geoadd, Geopos, Geodist, Geosearch,
Geohash` with `FromStr` arms ("GEOADD" → …) in `config.rs`.

**D2 — Raw commands.** All five ops build `redis::cmd("GEOADD" | …)` —
the zset precedent for redis-rs method gaps. No redis-rs geo helpers, no
dependency change.

**D3 — Header contract.** One member per exchange. Unlike ZADD/SADD,
which deliver the member as the `CamelRedis.Value` blob header, geo ops
use dedicated string headers `CamelRedis.Member` / `Member2` /
`Members` — coordinates are separate typed headers and the member is
never a serialized blob:

| Op | Required headers | Optional |
|---|---|---|
| GEOADD | `CamelRedis.Key`, `CamelRedis.Longitude`, `CamelRedis.Latitude`, `CamelRedis.Member` | — |
| GEOPOS | `CamelRedis.Key`, `CamelRedis.Members` (array) | — |
| GEODIST | `CamelRedis.Key`, `CamelRedis.Member`, `CamelRedis.Member2` | `CamelRedis.Unit` (m\|km\|mi\|ft, default m) |
| GEOSEARCH | `CamelRedis.Key`, `CamelRedis.Longitude`, `CamelRedis.Latitude` | `CamelRedis.Radius` (with BYRADIUS), `CamelRedis.Width`+`CamelRedis.Height` (with BYBOX), `CamelRedis.Unit`, `CamelRedis.WithDist`, `CamelRedis.WithCoord`, `CamelRedis.Count` |
| GEOHASH | `CamelRedis.Key`, `CamelRedis.Members` (array) | — |

Exactly one of radius or width/height selects BYRADIUS vs BYBOX; both or
neither is a parse error.

**D4 — Fail-early validation** (house rule; no server round trip):
latitude ∈ [-90, 90], longitude ∈ [-180, 180], radius/width/height > 0
and finite, unit ∈ {m, km, mi, ft}, numeric headers must be numbers.
Violation → `CamelError::ProcessorError` naming the header. GEOSEARCH
FROMMEMBER centers are out of scope (deferral ledger).

**D5 — Result shapes** (splitbody rule). One `serde_json::Value` per
reply, set as `Body::Json` like every family module:

- GEOADD → integer added-count.
- GEOPOS → array parallel to input members; each entry `[lon, lat]` or
  `null` when the member is absent.
- GEODIST → number in the requested unit, or `null` when either member
  is missing.
- GEOSEARCH no extras → array of plain member strings. With
  WithDist/WithCoord → array of objects `{member, distance?,
  longitude?, latitude?}` (distance in the requested unit; coordinates
  rounded to 6 decimals, GPS precision). Extras replies parse as raw
  `Vec<Vec<redis::Value>>` rows (2- or 3-element rows depending on the
  extra combination) and map through one pure helper — a fixed redis-rs
  tuple type would fail on arity mismatch between extra combinations.
- GEOHASH → array of geohash strings (null for absent members).

**D6 — Idempotency.** Geopos, Geodist, Geosearch, Geohash join the
read-only list. Geoadd stays unlisted (non-idempotent): the added-count
reply makes retry-after-timeout ambiguous, matching the conservative
default for unlisted writes.

**D7 — Repo surface.** Inherent public methods on
`RedisCacheRepository` (re-exported type), NOT on the `CacheRepository`
trait — geo is a Redis-only capability and the trait is a cross-backend
contract (redb cannot implement it):

```rust
pub enum GeoUnit { Meters, Kilometers, Miles, Feet } // as_str(): "m"|"km"|"mi"|"ft"
pub struct GeoSearchRow { pub member: String, pub distance: f64 }
geo_add(key, longitude, latitude, member) -> i64
geo_search_radius(key, longitude, latitude, radius, unit) -> Vec<GeoSearchRow>
geo_search_box(key, longitude, latitude, width, height, unit) -> Vec<GeoSearchRow>
```

Keys go through `namespaced()`; commands run through the executor seam
with `execute_retry_safe`; validation mirrors D4 before any command is
built. Rows use WITHDIST only (repo contract is members + distances).

**D8 — Tests.** Unit tests inline: `config.rs` (FromStr + idempotency),
`geo.rs` (validation, header resolution, result shaping via helper fns),
`cache_repo.rs` (validation). Integration (TestContainers, Docker,
`integration-tests` feature): component GEO cases in
`camel-test/tests/redis_test.rs`, repo geo cases in
`redis_repositories_test.rs` — both suites already exist with harness
support (`tests/support/redis.rs`).

**D9 — Docs.** `docs/src/components/redis.md ## Commands` gains the five
GEO rows + header table. `examples/redis-example` gains one GEO route
(GEOADD then GEOSEARCH WITHDIST) in the existing style. No
`metadata.rs` change: `command=` already accepts any `RedisCommand`
string, so the DSL schema stays untouched.

## Affected Crates / Boundaries

- **Components**: `camel-component-redis` (config, executor, commands/geo).
- **Services**: `camel-redis-repo` (geo methods, new public types).
- **Tests**: `camel-test` (two integration suites extended).
- **Not touched**: Runtime, DSL, schema, `camel-api` trait, examples
  index beyond `redis-example`.

Single delivery phase — additive, one zone, no ordering hazards.
