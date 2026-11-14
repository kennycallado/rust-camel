# Proposal: redisbodyonly

## Why

ADR-0098 (one day old) landed a header-first value contract: `CamelRedis.Value`
when present, else the body. The e_opus ruling (2026-11-14) cuts the header:
ADR-0090 bans deprecated surfaces and requires tombstone errors, and the header
never preserved the body — every producer overwrites it with the reply, so the
header is a copy the route must maintain for no reason. Body-only is the
contract 0.69.0 ships.

The scope is wider than "values". Ten dispatch paths still read
`CamelRedis.Value` through `require_value` with no body fallback (`GETSET`,
`SREM`, `SISMEMBER`, `SMOVE`, `ZREM`, `ZRANK`, `ZREVRANK`, `ZSCORE`, `ZINCRBY`,
`LREM`), so the rule is: *the command's single payload argument comes from the
body*. `CamelRedis.Values` disappears with the same tombstone (`HMSET`/`MSET`
take a JSON-object body), and `CamelRedis.Member` on `GEOADD` becomes a
tombstone (the member comes from the body).

## What Changes

Affected crates: `camel-component-redis`, `camel-test`, `camel-cli`,
`camel-integration-test` fixtures, `examples/redis-example`,
`examples/redis-sentinel`.

- Replace `resolve_value_bytes` with a body-only resolver
  (`body_value_bytes`); delete `require_value` / `value_to_redis_arg` from
  payload paths and the `#[allow(dead_code)]` `build_redis_cmd` builders.
- Tombstones: `CamelRedis.Value` anywhere on an affected command,
  `CamelRedis.Values` on `HMSET`/`MSET`, and `CamelRedis.Member` on `GEOADD`
  fail before sending with a named error. `Member`/`Member2`/`Members` on the
  other GEO commands stay addressing operands.
- Codec fix: a JSON string body contributes its raw characters (fixes the
  current `Body::Json(String("abc"))` → `"abc"` quoting bug); `Body::Json(null)`
  is rejected before send (a `GET` miss must not store the literal `null`).
- Four payload classes written into `redis-values`: scalar, map (`HMSET`/`MSET`),
  stream entry (`XADD`), addressing operands (never the body).
- Port consumers: `camel-test` (redis, sentinel, support), CLI e2e test,
  `redis_state/order.test.yaml` fixture, both examples.
- Rewrite ADR-0098 in place; update `redis-values` + `redis-geo` specs,
  component README, `docs/src/components/redis.md`,
  `docs/src/testing/scenario-redis.md`, `CONTEXT-MAP.md`.

Excluded: no `valueSource` option, no deprecation shim, no new command variants
(`rc-tic86` stays blocked).

## Acceptance criteria

- The nine named tests from the mission exist and each fails before the change
  (tombstones, JSON-string raw characters, JSON-null rejection, binary operand
  symmetry for ZADD/ZSCORE and SADD/SISMEMBER, map bodies, XADD single field,
  empty-string acceptance).
- No production path uses `CamelRedis.Value` or `CamelRedis.Values` as a
  payload source (presence checks for the tombstones are the exception).
- `openspec validate redisbodyonly --type change --json` and
  `cargo xtask archive redisbodyonly --check` pass.

## Risk budget

Breaking change, accepted by the ruling; the merge commit carries `!`. The
tombstone must fire before any frame reaches the socket (binary operand
symmetry makes the body the only byte-faithful source). Out of bounds: new
commands, new options, compat shims, changes to reply shapes beyond what
ADR-0098 already landed.

Bd: rc-lv8jk
