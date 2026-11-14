# Design: redisbodyonly

## Approach

One resolver, one codec, tombstones at the resolver boundary, four payload
classes as the written contract.

**Resolver.** `commands/mod.rs` replaces `resolve_value_bytes(exchange, verb,
value_header)` with `body_value_bytes(exchange, verb, tombstones)`, where
`tombstones` names the removed headers that are illegal for this command, with
a fixed scope matrix: `CamelRedis.Value` on the scalar payload commands (the
list above, excluding GEOADD), `CamelRedis.Values` on `HMSET`/`MSET`,
`CamelRedis.Member` on `GEOADD` only. The resolver first checks each tombstone
header; if present it fails with the named tombstone error before encoding
anything: `CamelRedis.Value was removed in 0.69; put the value in the body`.
No tombstone fires where the header remains an addressing operand (`Member`/
`Member2`/`Members` on GEOPOS/GEODIST/GEOSEARCH/GEOHASH) or never existed
(`XADD`). Then the resolver encodes the body. `require_value`,
`value_to_redis_arg`, and the header-precedence branch die; `encode_body_value`
keeps its signature and gains two rejections: `Body::Json(Value::Null)` (named
error, pre-send) and the JSON-string raw-character rule replaces
`serde_json::to_string` (which quotes `Json(String("abc"))` into `"abc"`).

**Codec (one rule, all payload classes).** `Body::Text`/`Xml` → UTF-8 bytes;
`Body::Json(Value::String)` → the raw characters; any other JSON → compact
JSON; `Body::Bytes` → verbatim; reject before send: `Body::Empty`,
`Body::Stream`, `Body::Json(Value::Null)`, future variants (ADR-0049). Three
existing behaviors keep their assertions and gain genuinely new coverage so
their named tests stay baseline-red: map entries now reject `null` under the
scalar rule, XADD now encodes a JSON string body as raw characters, and a JSON
empty string now contributes zero bytes.

**Four payload classes** (recorded in the spec):
1. Scalar payload (`SET`, `SETNX`, `SETEX`, `GETSET`, `APPEND`, list writes,
   `SADD`, `SREM`, `SISMEMBER`, `SMOVE`, `ZADD`, `ZREM`, `ZRANK`, `ZREVRANK`,
   `ZSCORE`, `ZINCRBY`, `LREM`, `HSET`, `HSETNX`, `PUBLISH`, `GEOADD` member) —
   body via the codec.
2. Map payload (`HMSET`, `MSET`) — body must be a JSON object, or Text/UTF-8
   Bytes that parse as one; each entry value follows the scalar rule, including
   its null rejection (a `null` map entry fails before send); binary map
   values are a documented limit (headers were JSON too; no regression).
3. Stream entry (`XADD`) — the single `body` field; a JSON object is NOT split
   into fields.
4. Addressing operands (key, destination, pivot, `Member`/`Member2`/`Members`,
   score, increment, timeout) — headers or URI only, never the body.

**Dead code.** The `#[allow(dead_code)] build_redis_cmd` builders exist so
tests can assemble command frames without a connection. Five of them —
string, list, set, zset, hash — are the ones that reach `require_value` /
`value_to_redis_arg` and go away with this change, together with their
value-header tests. The same builder also exists in pubsub.rs, other.rs, and
key.rs, but those never touch the value contract and are out of scope.
Dispatch-only code paths remain.

**Consumers.** camel-test suites and support helpers, the CLI e2e test (sets
header and body to the same value — drop the header), the `redis_state`
fixture YAML, both examples: every `CamelRedis.Value`/`Values`/GEOADD
`Member`-as-payload use moves to the body.

**Docs.** ADR-0098 rewritten in place (body-only; the one-day-old header-first
text is superseded, not amended-on-amend). Specs, README, redis.md,
scenario-redis.md, CONTEXT-MAP.md updated; migration note included.

## Affected crates

- `camel-component-redis`: resolver, codec, tombstones, ten command paths,
  GEOADD member, map commands, dead builders, unit tests.
- `camel-test`: `redis_test.rs`, `redis_sentinel_test.rs`, `support/mod.rs`.
- `camel-cli`: `test_scenario_cli_e2e.rs`.
- `camel-integration-test`: `fixtures/redis_state/order.test.yaml`.
- `examples/redis-example`, `examples/redis-sentinel`: body instead of header.

## Architecture boundaries

Components only: the contract lives inside `camel-component-redis`; no
`camel-core`, DSL, or API surface changes. `HEADER_VALUE` in `lib.rs` is a
public const referenced by tombstone text only — removal decisions stay in the
component. The Exchange body remains the payload channel per ADR-0032.

## Alternatives considered

- **Keep header-first (ADR-0098 as written):** rejected by the e_opus ruling —
  ADR-0090 bans deprecated surfaces, and the header is a route-maintained copy
  of data the body already carries.
- **`valueSource` option / `auto`:** rejected — no option, no fallback; two
  sources for one payload is the ambiguity ADR-0032 avoids.
- **Deprecation warning for one release:** rejected — ADR-0090 requires
  tombstone errors for removed surfaces, not warnings.

Bd: rc-lv8jk
