# Proposal: redisgeo — GEO command support for the Redis stack

## Why

The camel-cache team consumes the Redis stack for location-aware caching.
Their workloads need geospatial indexes (member plus longitude/latitude in
one Redis key) and proximity queries (radius and bounding-box searches with
distances). Today the stack has zero GEO coverage:

- `camel-component-redis` producer supports string/key/list/set/hash/zset,
  pubsub, queue, sentinel, TLS, health — no GEO commands.
- `camel-redis-repo` exposes get/set/peek/invalidate/stats — no geo surface.

Consumers currently fall back to raw Redis clients, which bypasses the
component's retry, metrics, and error-transport conventions.

bd: rc-z459a (P2, owner-requested).

## What Changes

1. **Component producer ops** (`crates/components/camel-redis`): GEOADD,
   GEOPOS, GEODIST, GEOSEARCH, and GEOHASH as typed `RedisCommand`
   variants, dispatched through the existing family-module pattern
   (`commands/geo.rs`, routing in `executor.rs`, parsing in `config.rs`).
2. **Repo service surface** (`crates/services/camel-redis-repo`): public
   geo methods on `RedisCacheRepository` — store `(key, lon, lat, member)`
   and radius/box queries returning members with distances. Redis-only
   capability; the cross-backend `CacheRepository` trait in `camel-api`
   is NOT extended.
3. **Value shape rule** (splitbody ruling, commits 938bea3d / 0af13415):
   GEOSEARCH members stay plain strings; structured rows (member +
   distance + coordinates) become proper JSON objects. No quoted-JSON
   strings.
4. **Fail-early validation**: malformed coordinates (latitude outside
   `[-90, 90]`, longitude outside `[-180, 180]`, non-positive radius or
   box dimensions, unknown unit) are rejected at parse time with
   `CamelError::ProcessorError` — never via a server round trip.
5. **Spec**: new `openspec/specs/redis-geo` — one requirement per command
   plus error paths (bad coordinates, missing key producing empty-result
   semantics).
6. **Docs**: `docs/src/components/redis.md` Commands section entry plus a
   GEO route in `examples/redis-example` (existing style).

No dependency bumps: the pinned `redis` crate executes GEO commands
through `redis::cmd`, which the house pattern already uses for command
gaps.

## Acceptance Criteria

- All five GEO commands round-trip against a TestContainers Redis
  (`camel-test/tests/redis_test.rs`), including distance assertions in
   every unit.
- Repo geo methods round-trip in `camel-test/tests/redis_repositories_test.rs`.
- Malformed coordinates fail at parse time (unit tests, no server).
- Missing key: GEOPOS/GEOSEARCH/GEOHASH return empty results; GEODIST
  returns null — matching Redis semantics.
- `cargo test -p camel-component-redis` and `-p camel-redis-repo` stay
  green (561+ baseline preserved, count grows).
- No changes to `CacheRepository`, no schema/metadata registry changes.

## Risk Budget

Low. Additive only: new enum variants, one new family module, inherent
repo methods, new spec, docs. No existing command, trait, or wire format
changes. Main risks are result-shape drift (guarded by the splitbody rule
scenarios) and retry-classification mistakes (GEOADD stays
non-idempotent; reads are idempotent) — both pinned by scenarios.
