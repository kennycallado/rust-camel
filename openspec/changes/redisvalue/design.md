# Design: redisvalue

## Approach

Three shared helpers in `crates/components/camel-redis/src/commands/mod.rs`:

1. `resolve_value_bytes(exchange, verb) -> Result<Vec<u8>, CamelError>` — precedence: the command's value header first, encoded **byte-identically to today** (`value_to_redis_arg`: JSON string → its raw characters, any other JSON → compact `to_string()` — never the quoted `Body::Json` codec branch), then `into_bytes()`; header absent → codec over `exchange.input.body`. For every command except GEOADD the value header is `CamelRedis.Value`; GEOADD's value header is `CamelRedis.Member` (the member is GEOADD's value payload; honoring it is what decision 1's "keeps every existing route working" requires, and no third source is introduced — documented as the ruling rationale in ADR-0098).
2. `encode_body_value(&Body) -> Result<Vec<u8>, CamelError>` — the codec table: `Text` → UTF-8 bytes; `Json` → compact JSON bytes; `Xml` → the XML string as UTF-8 bytes (gap-fill: the mission table lists five variants but `Body` also has `Xml`, which today's XADD/PUBLISH path already accepts via `try_into_text` — the codec keeps that parity); `Bytes` → raw bytes verbatim; `Empty`/`Stream` → named `ProcessorError` **naming the command and the rejected body variant** before any frame is sent (wording style of `resolve_xadd_body`); `Body` is `#[non_exhaustive]`, so unknown future variants fail closed with the same error class (ADR-0049).
3. `reply_to_body(Option<&[u8]>) -> Body` — `None` → `Body::Json(null)` (absence keeps today's shape); `Some(bytes)` → `Body::Text` if valid UTF-8, else `Body::Bytes`.

Command wiring: SET, SETEX, SETNX, APPEND, LPUSH, RPUSH, LPUSHX, RPUSHX, LINSERT, LSET, HSET, HSETNX, SADD, ZADD (member), GEOADD (member), PUBLISH (message) call `resolve_value_bytes` and push the result as a byte argument (redis 1.8 implements `ToRedisArgs for &[u8]` → RESP bulk string); call sites drop the `String` conversion. HMSET resolves its field→value map from `CamelRedis.Values` when present (byte-identical encoding as above); when absent, the body must be a JSON object (`Body::Json(Object)`, or `Text`/`Bytes` that deserialize as a JSON object) whose entries are the field→value map — a map payload binds to a JSON-object body, values encoded byte-identically; anything else fails with the same named pre-send error. MSET is not in the mission's command list and keeps the header-only path. XADD replaces `resolve_xadd_body`'s text coercion with the codec, keeping its one-`body`-field rule and its pre-send Empty/Stream errors; `Body::Bytes` now works.

Replies: ack/count/flag replies (SET `Null`, SETNX bool, LPUSH/HSET/SADD/ZADD/GEOADD/PUBLISH/XLEN/STRLEN counts) stay `Body::Json` — untouched. **Getter scope: covered getters are GET and MGET only.** All other value-returning getters (GETSET, LPOP/RPOP, HGET/HGETALL, SMEMBERS, ZRANGE, …) keep `Body::Json` and are out of scope. GET switches its query to `Option<Vec<u8>>` and maps via `reply_to_body` — its reply shape changes from `Body::Json(string|null)` to `Text`/`Bytes`/`Json(null)` even when headers are present; that is the breaking change and the merge commit carries `!`. MGET switches to `Vec<Option<Vec<u8>>>`: when every element is nil-or-UTF-8 the reply stays today's `Body::Json` array (unchanged shape — the migration note must not claim otherwise); when any element is binary the reply is `Body::Bytes` holding a RESP encoding of the reply array (bulk strings, length-prefixed — lossless and boundary-safe), decoded in tests with a small RESP-array parser to prove order, boundaries, and completeness. Alternatives rejected at spec-bless: erroring on binary MGET (binary-hostile), base64-in-JSON (invented encoding).

Spec placement: new capability `redis-values`. The six existing redis specs are per-topic (failover, geo, pubsub-readiness, streams-consumer/producer, tls); the value contract cuts across strings/lists/sets/zset/hash/geo/pubsub/streams, so extending one topic spec would scatter a single contract over six directories. GEOADD's member fallback MODIFIES the existing `redis-geo` requirement `GEOADD stores a member with coordinates` (its `Missing member header fails` scenario pins header-only absence); the delta restates that requirement with the precedence rule.

Trust boundary (ADR-0032): the body is untrusted exchange data flowing to a data-plane sink; the codec's fail-closed Empty/Stream rejection is the validation step. No control-plane crossing. Value size follows the existing header-path behavior — no new cap (caps belong to `rc-tic86`'s scope).

Tests: new `value_fake_tests.rs` reuses the scripted `StreamFakeServer` harness (harness functions become `pub(crate)`; additive `resp_bulk_bytes` helper for non-UTF-8 payloads). The six mission-named tests mirror the delta scenarios one-to-one. Four fail before the change; `value_header_still_wins_over_the_body` is the declared back-compat guard; `empty_and_stream_bodies_fail_before_sending` asserts the codec's named error wording, which does not exist before (today's message is "Missing required header").

## Affected crates

- `camel-component-redis`: `commands/mod.rs` (helpers), `string.rs`, `list.rs`, `hash.rs`, `set.rs`, `zset.rs`, `geo.rs`, `pubsub.rs`, `stream.rs` (wiring), `value_fake_tests.rs` (new), `lib.rs` (module decl).
- docs: `docs/src/components/redis.md`, `crates/components/camel-redis/README.md`, `docs/adr/0098-redis-value-contract.md`.
- No `camel-api` change: `Body` is already bytes-capable.

## Architecture boundaries

Component-local contract change (Components ring): no Runtime, DSL, Services, Languages, or Functions surface moves; `RedisMetadataDescriptor` (URI params only) is untouched. ADR-0090 governs the legality of the pre-1.0 contract change; ADR-0098 records it.

## Alternatives considered

- Extend an existing redis spec (redis-geo / redis-streams-producer) — rejected: scatters one cross-cutting contract across per-topic specs.
- HMSET stays header-only — rejected at spec-bless: the mission lists HMSET among helper users and a JSON object body is a valid map payload; the body path is specified instead.
- Base64-encode binary MGET elements or map values inside JSON — rejected: an invented encoding the mission did not pin.
- Encode map-header values through the codec's `Json` branch — rejected: would quote string values (`"hello"`) where today writes `hello`; the header path stays `value_to_redis_arg` byte-identical.
- Per-command resolution functions — rejected: duplicates the codec; the mission demands one table in one place.
- Body-only binding (drop the header) — rejected: breaks every existing route; decision 1 pins header precedence.
