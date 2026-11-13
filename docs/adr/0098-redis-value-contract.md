# ADR-0098: Redis value contract — body binding and binary safety

- Date: 2026-11-13
- Status: Accepted (decided 2026-11-13)
- Amends: none
- Cross-refs: ADR-0032 (exchange-data trust boundary), ADR-0049 (workspace
  `#[non_exhaustive]` policy for v1.0 contract enums), ADR-0090 (pre-1.0
  compatibility policy), bd `rc-01ka4`, bd `rc-tic86` (SET options, GETDEL,
  GETEX, collections, bitmaps, HLL follow-up)

## Context

The Redis Producer treated value payloads as headers only. `SET`, `APPEND`,
and the other value-writing commands required `CamelRedis.Value`. The
`value_to_redis_arg` helper rendered a JSON string as its raw characters and
any other JSON value as compact JSON. Two limits follow. A route cannot send a
binary payload, because headers are JSON values and JSON cannot carry arbitrary
bytes. A route cannot send the Exchange body to Redis without a copy into a
header first, even when the route already holds the payload in the body.

Replies had the mirror problem. `GET` returned `Body::Json(string|null)`, so a
binary value was rejected at conversion. `MGET` decoded each element into a
`String`, so one non-UTF-8 element failed the whole reply.

This ADR records the contract that removes both limits without breaking
existing routes. The implementation lives in the shared helpers
`resolve_value_bytes`, `encode_body_value`, and `reply_to_body`
(`crates/components/camel-redis/src/commands/mod.rs`). The per-command
resolvers live in `commands/string.rs`, `commands/hash.rs`, `commands/geo.rs`,
`commands/pubsub.rs`, and `commands/stream.rs`.

## Decision

### 1. Value source precedence

A value-writing command reads its value payload from the command's value
header when that header is present, and from the Exchange body otherwise. No
third source exists.

| Value header | Commands |
| --- | --- |
| `CamelRedis.Value` | `SET`, `SETEX`, `SETNX`, `APPEND`, `LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`, `LINSERT`, `LSET`, `HSET`, `HSETNX`, `SADD`, the `ZADD` member, `PUBLISH` |
| `CamelRedis.Member` | `GEOADD` |

The header path is byte-identical to the previous `value_to_redis_arg`
behavior. A JSON string contributes its raw characters. Any other JSON value
contributes compact JSON. Existing routes therefore produce the same wire
bytes.

GEOADD uses `CamelRedis.Member` as its value header. The member is the GEOADD
value payload, so the rule that keeps every existing route working requires
the header that GEOADD already uses. A separate rule for GEOADD would create a
second header for one payload and break existing GEOADD routes.

The body path uses the codec in decision 2. An unencodable body fails with
`CamelError::ProcessorError` before any command frame is sent.

### 2. Body-to-value codec

One shared codec converts the Exchange body to value bytes for every
value-writing command:

| Body variant | Value bytes |
| --- | --- |
| `Body::Text` | UTF-8 bytes of the text |
| `Body::Json` | compact JSON serialization |
| `Body::Xml` | UTF-8 bytes of the XML text |
| `Body::Bytes` | the bytes verbatim |
| `Body::Empty` | rejected before send |
| `Body::Stream` | rejected before send |
| future variant | rejected before send (ADR-0049) |

The rejection is a named processor error. The message names the command and
the rejected variant, for example `SET value: the exchange body is Body::Empty`
followed by the accepted variants. The body path appends a hint to set the
command's value header (`CamelRedis.Value`, or `CamelRedis.Member` for GEOADD).
XADD keeps its one-`body`-field rule and uses the same codec.

### 3. Reply rule

A reply that replaces the body with a value payload uses one rule.
`Body::Text` when the reply bytes are valid UTF-8, `Body::Bytes` otherwise. A
nil reply keeps `Body::Json(null)`. The helper `reply_to_body` implements the
rule.

Only GET and MGET are covered:

- GET applies the rule directly. Its reply shape changes from
  `Body::Json(string|null)` to `Body::Text`, `Body::Bytes`, or
  `Body::Json(null)`.
- MGET queries byte vectors. When every element is nil or valid UTF-8, the
  reply stays the current `Body::Json` array of strings and nulls. When any
  element is binary, the reply is `Body::Bytes` that holds a RESP encoding of
  the reply array. The envelope uses length-prefixed bulk strings (`*N`,
  `$<len>` plus bytes, `$-1` for nil) and preserves element order, nils, and
  byte boundaries. A caller decodes it with `redis::parse_redis_value`.
- Every other value-returning getter keeps its current `Body::Json` reply.
  Replies that are not value payloads (counts, booleans, ack nulls) stay
  `Body::Json`.

### 4. Map commands

HMSET resolves its field-to-value map from `CamelRedis.Values` when that
header is present, byte-identical to the previous header encoding. When the
header is absent, the body must be a JSON object whose entries are the
field-to-value map. Each entry value is encoded like a header value: a JSON
string contributes raw characters, any other JSON value contributes compact
JSON. A `Body::Text` or UTF-8 `Body::Bytes` body is parsed as a JSON object. A
body that does not parse as a JSON object fails with a named pre-send error
that names HMSET.

MSET keeps the `CamelRedis.Values` map header only.

### 5. Out of scope

- No value streaming or chunking. `Body::Stream` is rejected, not buffered.
- No compression. Redis stores the bytes it receives.
- No new command variants. The command enum and the URI surface are
  unchanged.
- bd `rc-tic86` owns the command-family follow-up: SET options (`EX`, `PX`,
  `NX`, `XX`, `KEEPTTL`), GETDEL, GETEX, collection commands, bitmap
  commands, and HLL commands.

## Consequences

- The GET reply shape changes from `Body::Json(string|null)` to `Body::Text`,
  `Body::Bytes`, or `Body::Json(null)`. This is a breaking change for a route
  that reads the GET reply as a JSON string. The change applies even when the
  value header is present. The merge commit carries the `!` breaking marker.
  ADR-0090 applies: no external commitment covers the GET reply shape, so the
  contract changes now instead of carrying a compatibility shim.
- MGET all-text replies are unchanged. A route that reads `["a", null, "c"]`
  keeps that shape.
- A binary MGET reply previously failed at conversion. It now returns the
  lossless RESP envelope, so a route must decode it with
  `redis::parse_redis_value` when it expects binary elements.
- Existing value-header routes produce byte-identical frames. A route that
  sets `CamelRedis.Value` or `CamelRedis.Member` keeps its wire behavior.
- The body fallback makes a missing value header non-fatal. GEOADD no longer
  fails on a missing `CamelRedis.Member` when the body carries the member. A
  `Body::Empty` body still fails before send.
- The three helpers are the single source of the contract. A new
  value-writing command must call them instead of reading the value header
  directly.
