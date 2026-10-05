# Redis state assertions

A Redis validate target reads a key through a named datasource.
Enable `integration-redis` in `camel-cli`, or `redis` in `camel-integration-test`.
The default CLI does not activate this assertion executor.
Without the feature, a valid Redis target returns a named verdict failure.

This family provides assertions only. There is no `redis:` scenario prepare action.
Use the Redis producer in a route, or seed keys with a Redis client.
There is no generic `state:` action.

## Datasource configuration

```toml
[datasources.statedb]
provider = "redis"
db_url = "${env:REDIS_URL}"
```

The driver accepts `redis://[user:password@]host[:port][/db]` URLs.
For example, `redis://localhost:6379/1` selects database 1.
ACL usernames and passwords retain their driver semantics.
`rediss://` requires the Redis component's `tls` feature (`redis-tls` in the CLI).
Sentinel and cluster datasource URLs are not supported.
Route endpoint URLs use a different parser. They select databases with `?db=1`, not `/1`.

The boot registers `RedisPoolFactory` with the shared datasource catalog.
Each alias caches one multiplexed connection. The health check issues `PING`.
Factory errors omit URLs and driver text. Resolver errors pass through the shared URL sanitizer.
The canonical diagnostic boundary is `camel_api::redact`, not a family-specific redactor.

## Validate grammar

```yaml
routeFiles: [routes/order.yaml]
scenario:
- send:
    to: direct:order
    body: order-value
    headers:
      CamelRedis.Value: order-value
      CamelRedis.Timeout: 30
- validate:
    target:
      redis:
        datasource: statedb
        key: order:42
        type: string
        ttl: {atLeast: 1s, atMost: 60s}
    expectation:
      rows: [[order-value]]
    deadline: 3s
```

The route's producer URL must name the same key:

```yaml
routes:
- id: redis-order
  from: direct:order
  steps:
  - to: redis://localhost:6379?command=SETEX&key=order:42
```

`datasource` and `key` are document-authored literals, not environment placeholders.
`type` is required. It must be `string`, `hash`, `list`, `set`, or `zset`.
`SETEX` reads expiry seconds from `CamelRedis.Timeout`.
Omit `ttl` when expiry is not part of the assertion.

## Type projections

| Declared type | Inherent columns | Row order |
|---|---|---|
| `string` | `value` | One value row |
| `hash` | `field`, `value` | Field order |
| `list` | `index`, `value` | Numeric index order |
| `set` | `member` | Lexicographic member order |
| `zset` | `member`, `score` | Ascending rank order |

`columns` selects a subset and determines column order. It does not change row order.
For a hash with `name=alice` and `age=42`, this assertion reverses the columns:

```yaml
expectation:
  columns: [value, field]
  rows: [['42', age], [alice, name]]
```

Unknown columns and row-width mismatches are load errors, even without explicit `columns`.
`rows` uses the shared cell matchers, including `{ignore: null}` to match any cell.
`unordered: true` ignores row order but retains the declared column order.
Use exactly one row shape: `rows`, `count`, `atLeast`, `atMost`, or a paired `atLeast`/`atMost` range.
Count bounds apply to projected rows, not columns.

Valid UTF-8 strings become string cells. Integers become numeric cells. Scalar Nil becomes null.
Malformed replies, invalid UTF-8, unsupported types, and odd pair counts fail closed as apparatus errors.
Sorted-set scores must be finite. Integral scores become JSON integers only within `[-2^63, 2^63)`.
An integral score outside that range is an apparatus error. Non-integral scores become JSON floats.
Write score `2` as integer `2` in expected rows. Write fractional score `1.5` as `1.5`.

## Atomic snapshots and deadlines

Each acquisition sends one fixed, read-only `EVAL` script.
The script reads `TYPE`, the declared type's payload, and `PTTL` in one atomic execution.
Keys are command arguments, not script text.
Missing keys and supported wrong-type keys remain snapshot data.
They become verdict mismatches only when the final snapshot cannot satisfy the target.
An initially missing or wrong-type key can therefore recover during a deadline.

Without `deadline`, one snapshot decides.
With `deadline`, the shared poll driver reads every 100 ms and judges the final snapshot.
A matching early snapshot never settles the assertion: a later deletion, retype, or expiry can invalidate it.
Only a count ceiling breach ends the window early.
Malformed replies and driver failures stop immediately as apparatus failures.
The driver waits for an in-flight acquisition; the assertion deadline does not cancel that acquisition.

## TTL bounds

`ttl` accepts `atLeast`, `atMost`, or both, as positive whole-millisecond duration strings.
Zero bounds, sub-millisecond precision, inverted ranges, and numeric overflow are load errors.
Redis `PTTL` has these meanings:

- `-2`: missing key. Every declared TTL bound fails.
- `-1`: persistent key. Every declared TTL bound fails.
- `0` or greater: remaining milliseconds, checked before conversion to matcher counts.
- Other negatives: apparatus failure, even without a TTL assertion.

Remaining `0` passes every legal `atMost`. It fails every legal `atLeast` and range.
The final snapshot decides TTL together with type and rows.
Diagnostics include permitted type/count metadata and observed TTL, but never actual cell values, fields, or members.

## Durable state and verification

Delete each test's unique key before seeding it. Delete the key after the test.
Do not share keys between parallel scenarios.
Shutdown invokes the factory's no-op close hook. It does not delete keys or release every catalog reference.
Connections are drop-scoped: release requires dropping all boot, context, catalog, and cloned connection owners.

The Docker-backed `integration-redis` CI job runs Redis-only CLI tests, live snapshot tests, and full-boot tests.
The default library suite starts no Redis container. Run the live battery explicitly:

```sh
cargo test -p camel-integration-test --features redis,redis-live --lib
cargo test -p camel-integration-test --features redis --test redis_state_test
cargo test -p camel-cli --no-default-features --features integration-redis,itest-e2e --test test_scenario_cli_e2e
```
