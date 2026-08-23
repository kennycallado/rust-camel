# Proposal: sentinel-data-auth

## Why

The redis repository backends (`cache_repo` / `idempotent_repo`,
`backend = "redis"`, ADR-0063) cannot authenticate the DATA nodes when
running under Sentinel. `redis_endpoint_from_fields`
(`crates/camel-config/src/context_ext.rs`) maps `sentinel_username` /
`sentinel_password` to the client→sentinel connection only; the data
connection (master/replicas) takes its credentials from
`RedisEndpointConfig`, which is never populated in sentinel mode because
`url` is mutually exclusive with `sentinel_nodes`. A production Sentinel
deployment whose master enforces `requirepass` therefore fails with
`NOAUTH` on the first command. Database selection in sentinel mode is
likewise impossible (always db 0), and the cross-repo prefix-collision
rule hardcodes sentinel db 0 so two repos on different logical databases
falsely collide.

The component URI dialect already supports master credentials and `?db=N`
(commit `60ccdb1b` fixed the render/parse round-trip); the gap is the
repo config surface plus one added component field and one source-breaking component field widening. Found by
post-merge operator review of redis-repositories (bd rc-vkpl).

## What Changes

- Add `password`, `username` (both `Option<String>`), and `db`
  (`Option<u16>`, validated 0..=16383) fields to the redis arm of
  `CacheRepoConfig` and `IdempotentRepoConfig`. They authenticate and
  select the database of the DATA nodes in sentinel mode; in `url` mode
  they are rejected (password and db ride the URI — userinfo password and
  `?db=N`; username in the URI is out of scope for this change).
- Thread the fields through `redis_endpoint_from_fields` into
  `RedisEndpointConfig`.
- Component field changes (verified necessary — the endpoint does
  NOT carry a data-node username today, and its `db` is `u8` which cannot
  hold the full Redis db range; the widening is source-breaking on a pub
  field, acceptable pre-1.0, rides a minor release): add `username: Option<String>` to
  `RedisEndpointConfig` (constructors + redacting Debug) and widen `db`
  from `u8` to `u16`; propagate username into
  `sentinel_node_conn_info`'s `RedisConnectionInfo`.
- Extend the fail-closed cross-backend matrix symmetrically for both
  repos (memory/redb reject the three fields; redis rejects them with
  `url` set).
- Include the sentinel-mode `db` in the effective-endpoint identity of
  the cross-repo prefix-collision rule (different dbs on the same
  sentinel topology and prefix do not collide).
- Hand-written redacting `Debug`: data-node `password` and `username`
  redacted with `***` (ADR-0051 posture), tested for both configs.
- Update `docs/src/configuration/schema.md` field tables.

Explicitly excluded: per-sentinel-node credentials (nodes may differ),
ACL work, TLS work, and the registration-name question (bd rc-vl1l,
separate decision).

## Acceptance criteria

- A `Camel.toml` with `[cache_repo] backend = "redis"`, `sentinel_nodes`,
  `master_name`, `sentinel_password`, AND `password` builds an endpoint
  whose data connection carries the master password (unit-asserted on the
  constructed endpoint).
- `db = 2` in sentinel mode selects database 2 on the data connection;
  `db = 20000` fails validation naming the field.
- Validation rejects `password`/`username`/`db` on memory and redb
  backends, and alongside `url` on redis, with errors naming
  `<repo>.<field>` (e.g. `cache_repo.password`, `idempotent_repo.db`), for
  BOTH repos.
- The collision rule treats sentinel db 2 vs db 3 as non-colliding and
  db 2 vs db 2 as colliding.
- `format!("{:?}")` of either config never prints the data-node
  `password` or `username` values.
- All existing camel-config and camel-component-redis tests stay green.

## Risk budget

Security-sensitive (new credential fields + redaction) — redaction must
be tested, never eyeballed. The `u8`→`u16` db widening is a source-breaking
public-field type change, acceptable pre-1.0 in a minor release; it must not alter URI parsing behavior for
existing values (0..=255 round-trips unchanged). No runtime behavior
change for existing configs: unset fields behave exactly as today. Two
crates (camel-config, camel-component-redis) plus docs. Live
authenticated-sentinel test is optional scope and may be deferred to CI
if flaky locally.
