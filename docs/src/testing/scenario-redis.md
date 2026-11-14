# Redis state assertions

A Redis validate target reads a key through a named datasource.
Enable `integration-redis` in `camel-cli`, or `redis` in `camel-integration-test`.
The default CLI (`flavor-regular`) and `flavor-full` activate this assertion executor; `flavor-slim` does not.
Without the feature, a valid Redis target returns a named verdict failure.

This family owns the read side plus two delete-only `redis:` prepare forms: the literal-key `del` action and the finally-only `unlinkPrefix` teardown (see [The `redis:` del action](#the-redis-del-action)).
Writing keys stays the route producer's job, or a Redis client's.
There is no `set` action, no generic command runner, and no pattern delete of authored keys.
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
`SETEX` writes the exchange body and reads expiry seconds from `CamelRedis.Timeout`.
The value never travels in a header ([ADR-0098](../adr/0098-redis-value-contract.md)).
Omit `ttl` when expiry is not part of the assertion.

## Type projections

| Declared type | Inherent columns | Row order |
|---|---|---|
| `string` | `value` | One value row |
| `hash` | `field`, `value` | Sorted by field name, not insertion order |
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
Redis cells are strings or numbers, never JSON objects. A `jsonSubset` cell therefore loads without error but never matches.
Use `equals`, `regex`, `contains`, `startsWith`, or `endsWith` on redis cells.
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

## The `redis:` del action

A `redis:` prepare action deletes keys through a named datasource:

```yaml
scenario:
- redis:
    datasource: statedb
    del: [order:42, order:42:lock]
```

`datasource` names a key under `[datasources]` in `Camel.toml`.
`del` must declare at least one key, and it takes literal keys only.
A key containing `*`, `?`, `[`, `]`, `{`, or `}` fails the load, because `del` is not a pattern match.
One `DEL` removes every listed key through the datasource's catalog connection.
A missing key is success, so the action is idempotent and a delete before seeding is safe.
`unlinkPrefix` is the only pattern-shaped delete, and only under a harness-minted namespace prefix (see [unlinkPrefix (namespace-scoped teardown)](#unlinkprefix-namespace-scoped-teardown)).
There is no `FLUSHDB` and no `FLUSHALL`.
Glob-style bulk delete of authored keys is not supported.
Diagnostics name the datasource and sanitize the driver text, never the URL.

The `del` action is accepted in both lists.
In `scenario:` it is the clean-first idiom: delete the test's unique key before seeding it.
In a document-level `finally:` list it is the teardown: delete the keys after the verdict, on both verdict paths.
A teardown failure reports as `teardown-failure` and never masks the verdict.
The `validate` redis target keeps its read vocabulary (`datasource/key/type/ttl`), and the two never mix.

### unlinkPrefix (namespace-scoped teardown)

A `redis:` teardown item may declare `unlinkPrefix` instead of `del`:

```yaml
finally:
- redis:
    datasource: statedb
    unlinkPrefix: "${env:ns}order:"
```

`unlinkPrefix` and `del` are one-of: a document declares exactly one of the two fields. An action with both fields, or with neither field, fails at load.

The action is allowed ONLY in `finally:`. An entry in `scenario:` fails at load with a message that names `unlinkPrefix` and the action position.

The template is exactly `${env:NAME}` plus an optional literal suffix. `NAME` must be a namespace `bindVar` that the same document declares. The load rejects these templates:

- A default construct, for example `${env:ns:-fallback}`.
- A second `${`.
- A suffix with a Redis glob metacharacter: `*`, `?`, `[`, `]`, `\`, or `^`.

The run-time safety contract has these steps:

1. The harness resolves `NAME` from the minted namespace layer only. The document `env:` map and the ambient environment never supply the value.
2. The harness re-checks the resolved value against the mint grammar. It also checks that the value is byte-equal to the value minted for this document. The action issues no `SCAN` when either check fails.
3. The `MATCH` pattern is the escaped namespace-plus-suffix text plus one unescaped `*`.
4. The harness sends `SCAN` with `COUNT 500`. `COUNT` is a hint, so one reply may carry more keys.
5. The harness byte-checks every returned key against the namespace-plus-suffix text before it unlinks anything. A violating key fails the whole reply, and nothing from that reply is unlinked.
6. The harness sends `UNLINK` in batches of at most 500 keys. The action has a 10-second deadline and a 100 000-key cap.

A deadline breach, a cap breach, or a violating key reports as the existing `teardown-failure` apparatus class. The report names the reason and the number of keys deleted so far. There is no new failure class.

A cluster datasource is refused at datasource configuration (see [Datasource configuration](#datasource-configuration)). `KEYS`, `FLUSHDB`, `FLUSHALL`, and generic command runners stay forbidden. The `del` form keeps its literal single-`DEL` contract.

Only a harness-minted prefix-free namespace makes the delete scope provable, so this action is the only pattern-shaped delete in the family.

See [ADR-0069](../adr/0069-integration-tier-testing-contract.md) §8.4 for the ruling.

The out-of-scope boundary is the same as for document namespaces: no cross-run reaper and no cluster support ([Document namespaces](scenario-documents.md#document-namespaces)).

## Durable state and verification

Clean keys with the `del` action above: before seeding in `scenario:`, and after the test in `finally:`.
Do not share keys between parallel scenarios.
A document that shares `statedb` with concurrent jobs uses a namespace and the `unlinkPrefix` teardown, so the scan cannot reach another job's keys.
Shutdown invokes the factory's no-op close hook. It does not delete keys or release every catalog reference.
Connections are drop-scoped: release requires dropping all boot, context, catalog, and cloned connection owners.

The Docker-backed `integration-redis` CI job runs Redis-only CLI tests, live snapshot tests, and full-boot tests.
The default library suite starts no Redis container. Run the live battery explicitly:

```sh
cargo test -p camel-integration-test --features redis,redis-live --lib
cargo test -p camel-integration-test --features redis --test redis_state_test
cargo test -p camel-integration-test --features redis --test redis_state_test finally_redis_del_removes_exactly_the_listed_keys
cargo test -p camel-cli --no-default-features --features integration-redis,itest-e2e --test test_scenario_cli_e2e
```
