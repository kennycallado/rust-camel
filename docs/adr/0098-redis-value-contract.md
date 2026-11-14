# ADR-0098: Redis value contract — body binding and binary safety

- Date: 2026-11-14
- Status: Accepted (supersedes the 2026-11-13 header-first decision text of
  this same ADR, rewritten in place per the 2026-11-14 ruling)
- Amends: none
- Cross-refs: ADR-0032 (exchange-data trust boundary), ADR-0049 (workspace
  `#[non_exhaustive]` policy for v1.0 contract enums), ADR-0090 (pre-1.0
  compatibility policy), bd `rc-01ka4` (original header-first decision), bd
  `rc-lv8jk`, bd `rc-tic86` (SET options, GETDEL, GETEX, collections, bitmaps,
  HLL follow-up; blocked)

## Context

The 2026-11-13 revision of this ADR made the value header win when present.
It kept the Exchange body as a fallback. That contract kept existing routes
byte-identical.

The 2026-11-14 ruling cuts the value header entirely. Three reasons support
the cut.

First, ADR-0090 bans a deprecated surface. A surface may stay only for an
external commitment, an off-by-default security opt-in, a wire or format
contract with a component not shipped in lockstep, or a specification
requirement. The value header has no such claim. A warning period is also
excluded, because ADR-0090 requires a tombstone error for a removed surface.

Second, the header never preserved the body. Every producer overwrites the
Exchange body with the reply. A route that must read the body after a Redis
step copies the payload into a header before that step. The header is
therefore a route-maintained copy with no reader benefit.

Third, binary operands need the body. Headers are JSON values, so a header
cannot carry arbitrary bytes. Only the body can carry them.

Ten dispatch paths read `CamelRedis.Value` through `require_value` and had no
body path: `GETSET`, `SREM`, `SISMEMBER`, `SMOVE`, `ZREM`, `ZRANK`,
`ZREVRANK`, `ZSCORE`, `ZINCRBY`, and `LREM`. The body-only rule covers these
ten paths and the writes that already accepted the body.

Replies had a separate defect. `GET` returned `Body::Json(string|null)`, so a
binary value failed at conversion. `MGET` decoded each element into a
`String`, so one non-UTF-8 element failed the whole reply. Decision 3 fixes
both defects. The reply rule is unchanged from the 2026-11-13 revision.

The implementation lives in the shared helper `body_value_bytes`, the codec
`encode_body_value`, and the reply helper `reply_to_body`
(`crates/components/camel-redis/src/commands/mod.rs`). The per-command
resolvers live in `commands/string.rs`, `commands/list.rs`, `commands/set.rs`,
`commands/zset.rs`, `commands/hash.rs`, `commands/geo.rs`, `commands/pubsub.rs`,
and `commands/stream.rs`.

## Decision

### 1. Payload binding

A payload command takes its single scalar payload argument from the Exchange
body only, through the codec in decision 2. No value header exists.

The scalar payload commands are `SET`, `SETEX`, `SETNX`, `GETSET`, `APPEND`,
`LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`, `LINSERT`, `LSET`, `LREM`, `SADD`,
`SREM`, `SISMEMBER`, `SMOVE`, the `ZADD` member, `ZREM`, `ZRANK`, `ZREVRANK`,
`ZSCORE`, `ZINCRBY`, `HSET`, `HSETNX`, the `GEOADD` member, and `PUBLISH`
(message).

An addressing operand never comes from the body. The operands are the key,
the destination, the pivot, the score, the increment, the timeout, the index,
and the count. They come from a header or the URI.

### 2. Body-to-value codec

One shared codec converts the Exchange body to payload bytes for every
payload command:

| Body variant | Payload bytes |
| --- | --- |
| `Body::Text` | UTF-8 bytes of the text |
| `Body::Xml` | UTF-8 bytes of the XML text |
| `Body::Json` holding a string | the raw characters of the string, with no JSON quoting |
| `Body::Json` holding any other value | compact JSON serialization |
| `Body::Bytes` | the bytes verbatim |
| `Body::Empty` | rejected before send |
| `Body::Stream` | rejected before send |
| `Body::Json(null)` | rejected before send |
| future variant | rejected before send (ADR-0049) |

The JSON-string rule fixes a defect. The previous `serde_json::to_string`
call quoted a `Body::Json(String("abc"))` body into `"abc"`. The new rule
sends the three bytes `abc`. The same rule serves XADD and each map entry.

The null rejection is a separate guard. A `GET` miss produces
`Body::Json(null)`. Without the guard, a route that feeds a miss back to a
write command stores the four literal bytes `null`.

An empty value is valid. `Body::Text("")` and `Body::Json(String(""))` each
contribute zero payload bytes. `Body::Empty` is not an empty value: it is a
missing body, and it fails.

The rejection is a named processor error. The message names the command and
the rejected body form, for example `SET value: the exchange body is
Body::Empty`. XADD uses the same codec and keeps its one-`body`-field rule
(decision 5).

### 3. Reply rule

A reply that replaces the body with a value payload uses one rule.
`Body::Text` when the reply bytes are valid UTF-8, `Body::Bytes` otherwise. A
nil reply keeps `Body::Json(null)`. The helper `reply_to_body` implements the
rule.

Only `GET` and `MGET` are covered:

- `GET` applies the rule directly. Its reply shape is `Body::Text`,
  `Body::Bytes`, or `Body::Json(null)`.
- `MGET` queries byte vectors. When every element is nil or valid UTF-8, the
  reply stays a `Body::Json` array of strings and nulls. When any element is
  binary, the reply is a `Body::Bytes` that holds a RESP encoding of the
  reply array. The envelope uses length-prefixed bulk strings (`*N`,
  `$<len>` plus bytes, `$-1` for nil). It preserves element order, nils, and
  byte boundaries. A caller decodes it with `redis::parse_redis_value`.
- Every other value-returning getter keeps its `Body::Json` reply. Replies
  that are not value payloads (counts, booleans, ack nulls) stay
  `Body::Json`.

### 4. Map commands

`HMSET` and `MSET` take a JSON-object body. A `Body::Text` body or a UTF-8
`Body::Bytes` body that parses as a JSON object is also accepted. Each entry
is a field-to-value pair. Each entry value is encoded by the scalar rule in
decision 2, including the null rejection. A body that does not parse as a
JSON object fails with a named pre-send error that names the command.

`CamelRedis.Values` is removed. Decision 6 defines its tombstone.

Binary map values are a documented limit. The map body is JSON or UTF-8 text,
so a binary value cannot travel in a map entry. This is not a regression: the
previous header path was also JSON.

### 5. Stream entry

`XADD` appends one entry with exactly one field named `body`. A JSON-object
body is not split into per-key fields. The object contributes one field value
that holds the compact JSON of the object.

### 6. Tombstones

A removed payload header fails with a named processor error before any
command frame is sent. The tombstone scope is fixed by this matrix:

| Removed header | Tombstoned on |
| --- | --- |
| `CamelRedis.Value` | `SET`, `SETEX`, `SETNX`, `GETSET`, `APPEND`, `LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`, `LINSERT`, `LSET`, `LREM`, `SADD`, `SREM`, `SISMEMBER`, `SMOVE`, `ZADD`, `ZREM`, `ZRANK`, `ZREVRANK`, `ZSCORE`, `ZINCRBY`, `HSET`, `HSETNX`, `PUBLISH` |
| `CamelRedis.Values` | `HMSET`, `MSET` |
| `CamelRedis.Member` | `GEOADD` |

The error text is `"{name} was removed in 0.69; put the value in the body"`.
The check runs first, before the codec, so no frame reaches the socket.

The tombstone fires only on the command set in the matrix. `CamelRedis.Member`,
`CamelRedis.Member2`, and `CamelRedis.Members` keep their addressing-operand
roles on `GEOPOS`, `GEODIST`, `GEOSEARCH`, and `GEOHASH`. `XADD` never carried
these headers, so no tombstone applies to it.

### 7. Out of scope

- No `valueSource` option and no automatic source selection. Two sources for
  one payload is the ambiguity ADR-0032 excludes.
- No deprecation shim and no warning period. ADR-0090 requires a tombstone.
- No value streaming or chunking. `Body::Stream` is rejected, not buffered.
- No compression. Redis stores the bytes it receives.
- No new command variants. The command enum and the URI surface are
  unchanged.
- bd `rc-tic86` owns the command-family follow-up: `SET` options (`EX`, `PX`,
  `NX`, `XX`, `KEEPTTL`), `GETDEL`, `GETEX`, collection commands, bitmap
  commands, and HLL commands. The follow-up stays blocked.

## Consequences

- This is a breaking change for a route that sets `CamelRedis.Value`,
  `CamelRedis.Values`, or `CamelRedis.Member` as a GEOADD payload. The route
  now fails before send with the tombstone error. The merge commit carries the
  `!` breaking marker.
- The reply shape of `GET` is `Body::Text`, `Body::Bytes`, or
  `Body::Json(null)`. A route that read the reply as a JSON string must
  change. ADR-0090 applies: no external commitment covers the reply shape, so
  the contract changes now instead of carrying a compatibility shim.
- `MGET` all-text replies are unchanged. A route that reads `["a", null,
  "c"]` keeps that shape. A binary `MGET` reply now returns the lossless RESP
  envelope.
- The body is the only payload source. A route cannot drift from the value it
  sends, because Redis receives the body bytes directly.
- The three helpers and the tombstone matrix are the single source of the
  contract. A new payload command must call them instead of a value header.

## Migration

Move the header value into the body before the Redis step.

```yaml
# Before: the value lived in a header.
- set_header:
    key: CamelRedis.Value
    value: "${body}"
- to: "redis://localhost:6379?command=SET"

# After: the value is the body.
- to: "redis://localhost:6379?command=SET"
```

A route that needs a fixed value and a different body for logging keeps the
body in a property. It sets the fixed value as the body before the Redis step.

```yaml
- set_property:
    name: "originalBody"
    simple: "${body}"
- set_body: "fixed-value"
- to: "redis://localhost:6379?command=SET"
```

`enrich` is the alternative when the payload must come from a second source.

A route that used `CamelRedis.Values` passes a JSON-object body instead. A
route that used `CamelRedis.Member` on `GEOADD` passes the member in the body.
A route that used `CamelRedis.Member` or `CamelRedis.Member2` on `GEODIST`,
`GEOPOS`, `GEOSEARCH`, or `GEOHASH` needs no change, because those headers
remain addressing operands.
