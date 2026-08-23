# Design: sentinel-data-auth

## Approach

Mirror the existing topology-credential mapping with a data-node
counterpart. `CacheRepoConfig` / `IdempotentRepoConfig` gain three redis
fields: `password: Option<String>`, `username: Option<String>`,
`db: Option<u16>` (validation rejects > 16383 — the Redis limit — and
serde rejects negative integers naturally because `u16` has no negatives).
One consumer: `redis_endpoint_from_fields`
(`crates/camel-config/src/context_ext.rs`), whose sentinel branch sets
`endpoint.password`, `endpoint.username`, and `endpoint.db` after the URI
parse. The `url` branch is untouched — in `url` mode the password and database
selection ride the URI (userinfo password `redis://:pass@host`,
`?db=N` query; commit `60ccdb1b` made that dialect round-trip). Username
in the URI is OUT OF SCOPE: the parser is not modified, and the field is
populated only by the config threading. The new fields are rejected when
`url` is set (single source of truth: password via URI userinfo, username
via sentinel-mode field).

Component widenings (verified against source —
`crates/components/camel-redis/src/config.rs:480-519`): `RedisEndpointConfig`
carries `password: Option<String>` and `db: u8` but NO `username`, and
`u8` cannot hold db values above 255. Two changes (the `db` u8→`u16`
widening is source-breaking for out-of-workspace consumers on a pub
field; acceptable pre-1.0 — the workspace is 0.x and the change rides a
minor release, never a patch): add
`username: Option<String>` (constructors, defaults, redacting Debug) and
widen `db` to `u16` (URI parse/render of `?db=N` already round-trips
integers; 0..=255 behavior unchanged). `sentinel_node_conn_info`
(`topology.rs:332-348`) gains username propagation via
`RedisConnectionInfo::set_username`. No other component surface moves;
`RedisEndpointConfig` is crate-internal config plumbing shared by the
endpoint constructors, so every existing value (0..=255) round-trips unchanged.

Validation follows the established fail-closed matrix: the shared
`validate_redis_topology_fields` helper gains the three field names so
cache and idempotent arms stay symmetric by construction (symmetry has
had two past bugs — it is a hard requirement); `memory`/`redb` arms
reject the fields via the same `does not apply` convention. The
cross-repo prefix-collision rule (`redis_database_key`,
`config.rs:845-868`) hardcodes sentinel db 0; it must take the effective
sentinel db (`db.unwrap_or(0)`) into the identity so two repos on
different logical databases of the same sentinel topology do not
falsely collide (and same-db collision still fires). The hand-written
redacting `Debug` impls gain `password`/`username` redaction (`***`)
beside the existing sentinel credential rules (ADR-0051; camel-config
CONTEXT declares the crate security-sensitive), tested with distinct
secret literals for each field.

## Affected crates

- `camel-config`: config structs, validation matrix, collision identity,
  redacting Debug, endpoint threading, tests.
- `camel-component-redis`: `RedisEndpointConfig.username` field + `db`
  u8→u16 widening + `sentinel_node_conn_info` username propagation +
  round-trip tests.
- `docs/src/configuration/schema.md`: field tables.

## Architecture boundaries

Config layer plus component-config changes that stop at the
existing `RedisTopology` seam. No Runtime, DSL, Services, or Languages
changes; the repository service crate (`camel-redis-repo`) is untouched —
it receives a fully-populated `RedisEndpointConfig` exactly as today.
ADR-0063's explicit-config, fail-closed posture is preserved: no defaults
are invented — unset fields reproduce today's behavior byte-for-byte
(db 0, no data auth).

## Phases

Single phase — one coherent slice, ~5 tasks: (1) component widenings,
(2) config fields + validation + collision, (3) endpoint threading +
redaction, (4) docs, (5) optional live authenticated-sentinel test.
