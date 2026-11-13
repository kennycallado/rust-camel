# Tasks: redisvalue

Single-phase change (design.md has no `## Phases` section). All paths relative to the repo root.
Shared context for every task: blessed spec at `openspec/changes/redisvalue/specs/`; the codec
table and precedence are pinned in `design.md`; errors MUST name the command and the rejected
body variant; the header path must stay byte-identical to `value_to_redis_arg` (JSON string →
raw characters; other JSON → compact JSON).

Expected red/green at start: tests `set_takes_the_exchange_body_when_value_header_is_absent`,
`bytes_body_round_trips_through_set_and_get`, `get_reply_is_text_when_utf8_and_bytes_otherwise`,
`xadd_accepts_a_bytes_body`, and `empty_and_stream_bodies_fail_before_sending` must FAIL before
their implementing task lands; `value_header_still_wins_over_the_body` is the declared
back-compat guard and passes before and after.

## camel-component-redis: value helpers

### Task 1.1: Add the value codec, resolution, and reply helpers to commands/mod.rs

**Files:**
- `crates/components/camel-redis/src/commands/mod.rs` (modified)

**Steps:**
1. Add `pub(crate) fn encode_body_value(body: &Body, verb: &str) -> Result<Vec<u8>, CamelError>`
   implementing the codec table: `Body::Text(s)` → `s.as_bytes().to_vec()`; `Body::Json(v)` →
   `serde_json::to_string(v)` bytes (compact); `Body::Xml(s)` → `s.as_bytes().to_vec()`;
   `Body::Bytes(b)` → `b.to_vec()` verbatim; `Body::Empty` / `Body::Stream` →
   `CamelError::ProcessorError` whose message names `verb` and the variant, wording style of
   `resolve_xadd_body`, e.g. for verb `"SET"` and `Body::Empty`:
   `"SET value: the exchange body is Body::Empty; provide a Text, JSON, or Bytes body"`.
   Any other variant (Body is `#[non_exhaustive]`) → same error class with the variant name.
2. Add `pub(crate) fn resolve_value_bytes(exchange: &Exchange, verb: &str, value_header: &str)
   -> Result<Vec<u8>, CamelError>`: read header `value_header` (same lookup as
   `get_value_header`, mod.rs:168-170); when present encode byte-identically to today via
   `value_to_redis_arg(&value).into_bytes()`; when absent call `encode_body_value` and, on its
   error, return a `ProcessorError` with the same message plus `"; or set {value_header}"`
   appended (commands with a value header get the header hint; direct `encode_body_value`
   callers like XADD do not).
3. Add `pub(crate) fn reply_to_body(reply: Option<&[u8]>) -> Body`: `None` →
   `Body::Json(serde_json::Value::Null)`; `Some(bytes)` →
   `Body::Text(String::from_utf8(bytes.to_vec()))` when valid UTF-8, else
   `Body::Bytes(bytes.to_vec())`.
4. Add the `#[cfg(test)] mod value_helper_tests` unit tests below in the same file.

**Tests:**
- `encode_body_value_text_yields_utf8_bytes`: `encode_body_value(&Body::Text("café".into()), "SET")` → `Ok(b"caf\xc3\xa9".to_vec())`.
- `encode_body_value_json_is_compact`: `encode_body_value(&Body::Json(serde_json::json!({"a":1})), "SET")` → `Ok(br#"{"a":1}"#.to_vec())` (no spaces).
- `encode_body_value_bytes_is_verbatim`: input `[0xFF, 0x00, 0xC3, 0x28]` → identical `Vec<u8>`.
- `encode_body_value_xml_yields_the_xml_text`: `Body::Xml("<a/>".into())` → `b"<a/>"`.
- `encode_body_value_empty_and_stream_name_command_and_variant`: for `Body::Empty` and `Body::Stream` with verb `"SET"` → `Err(CamelError::ProcessorError(msg))` where `msg` contains `"SET"`, `"Body::Empty"` (resp. `"Body::Stream"`).
- `resolve_value_bytes_header_string_is_raw_chars`: exchange with header `CamelRedis.Value` = `"hello"` and `Body::Text("body".into())` → `Ok(b"hello")` (header wins, unquoted).
- `resolve_value_bytes_header_number_is_compact_json`: header value `42` → `Ok(b"42")`.
- `resolve_value_bytes_body_when_header_absent`: no header, `Body::Text("hello".into())` → `Ok(b"hello")`.
- `resolve_value_bytes_error_hints_the_header`: no header, `Body::Empty`, verb `"SET"`, header `"CamelRedis.Value"` → `Err` whose message contains `"SET"`, `"Body::Empty"`, and `"CamelRedis.Value"`.
- `reply_to_body_rule`: `None` → `Body::Json(Value::Null)`; `Some(b"café")` → `Body::Text("café")`; `Some(&[0xC3, 0x28])` → `Body::Bytes` equal to `[0xC3, 0x28]`.

**Acceptance:**
- `cargo test -p camel-component-redis --lib value_helper_tests` passes.
- `cargo fmt --check --all` exits 0.
- NOTE: the `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` gate for this task runs at Task 1.3 — until production callers exist (Task 1.2/1.3), `reply_to_body` is dead code on the lib target and `-D warnings` would fail on the helpers themselves.

- [x] 1.1

## camel-component-redis: string commands + fake-server harness

### Task 1.2: Wire SET/SETEX/SETNX/APPEND to resolve_value_bytes; add the value_fake_tests harness with the SET binding tests

**Files:**
- `crates/components/camel-redis/src/commands/string.rs` (modified)
- `crates/components/camel-redis/src/stream_fake_tests.rs` (modified: harness visibility only)
- `crates/components/camel-redis/src/value_fake_tests.rs` (new)
- `crates/components/camel-redis/src/lib.rs` (modified: `#[cfg(test)] mod value_fake_tests;`)

**Steps:**
1. In `stream_fake_tests.rs`, change the visibility of the harness items used by the new module
   to `pub(crate)`: `resp_bulk`, `resp_array`, `resp_int`, `resp_nil`, `resp_error`,
   `ScriptedAction`, `StreamFakeServer` (+ its `start`, `recorded`, `args_of`,
   `args_of_on_conn`, `wait_for_command`), `call_producer`, `producer_config`,
   `args_as_strings`, `body_exchange`, `keyed_exchange` (lines ~64-93, 170-250, 254-306,
   319-430, 705-740, 1940-1949). Do not change their bodies.
2. In `string.rs`, replace the value source of exactly these dispatch arms: SET (197-204),
   SETNX (222-230), SETEX (231-239), APPEND (289-297) — GET (205-212) and GETSET (213-221)
   are excluded: `let value = resolve_value_bytes(exchange, "SET", HEADER_VALUE)?;` (verb per
   command), then issue the command with the bytes (e.g. `redis::cmd("SET").arg(&key).
   arg(&value)` — `.arg(&value)` with `Vec<u8>` is RESP-bulk-safe). Keep ack replies
   (`serde_json::Value::Null` for SET, `Bool` for SETNX, counts for APPEND) exactly as today.
   Leave `Getset`, `Mset`, and the INCR family untouched.
3. Create `value_fake_tests.rs` importing only what its current tests use:
   `use crate::stream_fake_tests::{call_producer, keyed_exchange, ScriptedAction, StreamFakeServer};`
   (Task 1.3 extends the import list with the RESP helpers it needs).
4. Write the three tests below in `value_fake_tests.rs`, driving the producer with
   `call_producer(&server, "SET", "", exchange).await` (the harness builds the config; no
   direct `producer_config` import needed).

**Tests:**
- `set_takes_the_exchange_body_when_value_header_is_absent`: setup — fake server scripted `+OK` for SET; `keyed_exchange(Body::Text("hello"), "k")` with NO `CamelRedis.Value` header → act — `call_producer` with `RedisCommand::Set` → assert — no error and `server.args_of("SET")` equals `["k", "hello"]`.
- `value_header_still_wins_over_the_body`: setup — same script; `keyed_exchange(Body::Text("body-value"), "k")` with header `CamelRedis.Value` = `"header-value"` → act — `call_producer` → assert — frame args `["k", "header-value"]` (guard test: passes before this change too).
- `empty_and_stream_bodies_fail_before_sending`: setup — fake server with an empty script; four exchanges: `keyed_exchange(Body::Empty, "k")` and a second `keyed_exchange` carrying the same stream-body construction the existing `xadd_prohibited_body_forms_fail_before_send` test uses (stream_fake_tests.rs:2261-2305), each dispatched as SET → act — `call_producer` for each → assert — each returns `Err(CamelError::ProcessorError(msg))` where `msg` contains `"SET"` and `"Body::Empty"` (resp. `"Body::Stream"`), and `server.recorded()` stays empty after every attempt.

**Acceptance:**
- `cargo test -p camel-component-redis --lib value_fake_tests` passes with the three tests green.
- Pre-existing `stream_fake_tests` XADD tests still pass (`cargo test -p camel-component-redis --lib xadd`).
- `cargo fmt --check --all` exits 0.
- NOTE: the clippy `-D warnings` gate for Tasks 1.1-1.2 runs at Task 1.3 (dead_code until production callers exist).

- [x] 1.2

### Task 1.3: Binary-safe GET and MGET replies with the shared reply rule

**Files:**
- `crates/components/camel-redis/src/commands/string.rs` (modified)
- `crates/components/camel-redis/src/value_fake_tests.rs` (modified)

**Steps:**
1. In `string.rs`, switch the `Get` arm to a byte query: `redis::cmd("GET").arg(&key).
   query_async::<Option<Vec<u8>>>` (same error mapping via `redis_error_to_camel("GET", e)`),
   then `exchange.input.body = reply_to_body(val.as_deref());` replacing
   `json_from_optional_string` for GET only. These two arms (GET, MGET) set
   `exchange.input.body` directly and `return Ok(())`, bypassing the uniform
   `Body::Json(result)` dispatch tail at string.rs:307-308 (restructure the match
   accordingly); all other arms keep the tail unchanged.
2. Switch the `Mget` arm to `query_async::<Vec<Option<Vec<u8>>>>`; add
   `pub(crate) fn encode_mget_reply_bytes(values: &[Option<Vec<u8>>]) -> Vec<u8>` in
   `string.rs` producing a RESP array of bulk strings (`*N\r\n`, per element
   `$<len>\r\n<bytes>\r\n`, nil as `$-1\r\n`); MGET reply rule: when every element is `None`
   or valid UTF-8 → build `Vec<Option<String>>` from the byte vectors (every element is
   UTF-8-valid in this branch) and set the body with the existing
   `json_array_from_optional_strings` exactly as today; when any element is binary →
   `Body::Bytes(encode_mget_reply_bytes(&values).into())`.
3. Extend the `value_fake_tests.rs` import list with `body_exchange`, `resp_array`,
   `resp_bulk`, `resp_nil` (all promoted in Task 1.2 step 1); add the local
   `fn resp_bulk_bytes(value: &[u8]) -> Vec<u8>` helper producing a RESP bulk string of
   arbitrary bytes (`$<len>\r\n<bytes>\r\n`) and write the four tests below.

**Tests:**
- `bytes_body_round_trips_through_set_and_get`: setup — server scripted `+OK` for SET and `resp_bulk_bytes(&[0xFF, 0xFE, 0x00, 0x01])` for GET → act — SET with `Body::Bytes([0xFF, 0xFE, 0x00, 0x01])` then GET the same key → assert — SET frame carries those exact bytes (compare raw `Vec<Vec<u8>>` args, not `args_as_strings`), and the GET reply body equals `Body::Bytes` with the identical payload.
- `get_reply_is_text_when_utf8_and_bytes_otherwise`: setup — server scripted two GET replies: `resp_bulk("café")` then `resp_bulk_bytes(&[0xC3, 0x28])` → act — two GET exchanges → assert — first body `Body::Text("café")`, second body `Body::Bytes([0xC3, 0x28])`.
- `mget_keeps_the_json_array_for_text_replies`: setup — server scripted MGET replying `resp_array(&[resp_bulk("a"), resp_nil(), resp_bulk("c")])` → act — MGET exchange with `CamelRedis.Keys` header per the existing MGET header contract (string.rs:240-247) → assert — body equals the JSON array `["a", null, "c"]`.
- `mget_encodes_binary_elements_losslessly`: setup — server scripted MGET replying, in order, `resp_bulk("a")`, `resp_nil()`, `resp_bulk_bytes(&[0x00, 0xFF])` → act — MGET exchange → assert — body is `Body::Bytes(buf)` where a local RESP-array decoder recovers exactly three elements in server order: `Some(b"a")`, `None`, `Some(&[0x00, 0xFF])`.

**Acceptance:**
- `cargo test -p camel-component-redis --lib value_fake_tests` passes.
- `cargo test -p camel-component-redis --lib` passes in full (no regression in string/header tests).
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0 (this also gates the Task 1.1/1.2 helpers, which have production callers from here on).
- `cargo fmt --check --all` exits 0.

- [x] 1.3

## camel-component-redis: list, hash, set, zset wiring

### Task 1.4: Route LPUSH/RPUSH/LPUSHX/RPUSHX/LINSERT/LSET/HSET/HMSET/HSETNX/SADD/ZADD through resolve_value_bytes

**Files:**
- `crates/components/camel-redis/src/commands/list.rs` (modified)
- `crates/components/camel-redis/src/commands/hash.rs` (modified)
- `crates/components/camel-redis/src/commands/set.rs` (modified)
- `crates/components/camel-redis/src/commands/zset.rs` (modified)
- `crates/components/camel-redis/src/value_fake_tests.rs` (modified)

**Steps:**
1. `list.rs`: in the dispatch arms for `Lpush` (234-242), `Rpush` (243-251), `Lpushx`
   (252-260), `Rpushx` (261-269), `Linsert` (330-344), `Lset` (345-353) replace
   `require_value` + `value_to_redis_arg` with
   `resolve_value_bytes(exchange, "<VERB>", HEADER_VALUE)?` and push the bytes as the value
   argument (`.arg(&value)`); keep count/ack replies unchanged. LPOP/RPOP/LLEN/LRANGE/LINDEX
   arms (270-329) stay untouched.
2. `hash.rs`: `Hset` (192-201) and `Hsetnx` (210-219) same replacement. For `Hmset`
   (220-227): keep the `CamelRedis.Values` header path (via the existing
   `resolve_hash_values_map`, byte-identical encoding) when present; when absent, resolve the
   map from the body: `Body::Json(Value::Object(map))` directly, `Body::Text`/`Body::Bytes`
   parsed with `serde_json::from_str::<serde_json::Map<String, serde_json::Value>>`, anything
   else (including `Empty`/`Stream`) → `CamelError::ProcessorError` naming `"HMSET"` and the
   rejected variant; encode each map value with `value_to_redis_arg(&value).into_bytes()`
   (JSON string values stay unquoted) and issue `hset_multiple` with `(String, Vec<u8>)` pairs.
3. `set.rs`: `Sadd` (208-215) same replacement as step 1. Do not touch SREM/SISMEMBER/SMOVE.
4. `zset.rs`: `Zadd` (282-291): the member is the value payload — replace
   `require_value` with `resolve_value_bytes(exchange, "ZADD", HEADER_VALUE)?`; score handling
   (`resolve_zadd_score`) unchanged.
5. Write the two tests below in `value_fake_tests.rs`.

**Tests:**
- `lpush_takes_the_exchange_body_when_value_header_is_absent`: setup — server scripted `:1\r\n` for LPUSH; `keyed_exchange(Body::Text("job"), "q")` with no value header → act — `call_producer` with `RedisCommand::Lpush` → assert — frame args `["q", "job"]`, reply body JSON integer `1`.
- `hmset_takes_a_json_object_body_when_values_header_is_absent`: setup — server scripted `+OK` for HMSET; exchange with `CamelRedis.Key` = `"h"`, no `CamelRedis.Values`, `Body::Json(json!({"field": "value"}))` → act — `call_producer` with `RedisCommand::Hmset` → assert — frame args `["h", "field", "value"]` (string value unquoted), no error.

**Acceptance:**
- `cargo test -p camel-component-redis --lib` passes in full.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.4

## camel-component-redis: geo + pubsub wiring

### Task 1.5: GEOADD member body fallback; PUBLISH message through the codec

**Files:**
- `crates/components/camel-redis/src/commands/geo.rs` (modified)
- `crates/components/camel-redis/src/commands/pubsub.rs` (modified)
- `crates/components/camel-redis/src/value_fake_tests.rs` (modified)

**Steps:**
1. `geo.rs`: `Geoadd` (150-165): update `resolve_geoadd_args` (geo.rs:113-121) so the member
   is resolved through `resolve_value_bytes(exchange, "GEOADD", MEMBER_HEADER)?` where
   `MEMBER_HEADER` is either the literal `"CamelRedis.Member"` used by `require_member`
   (geo.rs:96) or a new constant promoted next to `HEADER_VALUE` in lib.rs — leaving the
   member resolution where it stands today would reject a missing member header BEFORE the
   body fallback can run; coordinate parsing/validation stays unchanged and pre-send. Push
   the member bytes as the member argument (arg order per geo.rs:155-159).
   GEOPOS/GEODIST/GEOHASH/GEOSEARCH untouched (their `require_member` callers stay).
2. `pubsub.rs`: `Publish` (92-102): replace the `extract_publish_message` call with
   `resolve_value_bytes(exchange, "PUBLISH", HEADER_VALUE)?` (header now wins, body fallback),
   push the bytes as the message argument; keep the channel header and receivers-count reply
   unchanged. Then DELETE the now-orphaned `pub(crate) fn extract_publish_message`
   (pubsub.rs:33-50) and its two unit tests `test_extract_publish_message_from_text_body` /
   `test_extract_publish_message_serializes_json_body` (pubsub.rs:168-183) — an uncalled
   `pub(crate)` fn fails clippy `-D warnings`. `resolve_publish_message` (pubsub.rs:22-31)
   STAYS (it feeds the `#[allow(dead_code)]` `build_redis_cmd` twin).
3. In `value_fake_tests.rs` add a small local exchange builder
   `fn geo_exchange(body: Body, key: &str, lon: f64, lat: f64) -> Exchange` (sets the
   `CamelRedis.Key`, `CamelRedis.Longitude`, `CamelRedis.Latitude` headers; the promoted
   harness builders only set body/Key) and write the three tests below.

**Tests:**
- `geoadd_takes_the_body_member_when_member_header_is_absent`: setup — server scripted `:1\r\n` for GEOADD; `geo_exchange(Body::Text("Palermo"), "sicily", 13.361389, 38.115556)` with no `CamelRedis.Member` → act — `call_producer` with `RedisCommand::Geoadd` → assert — the GEOADD frame's last argument is `Palermo` (member position per the existing arg order in geo.rs:155-159).
- `geoadd_missing_member_with_empty_body_fails_before_sending`: setup — server with empty script; `geo_exchange(Body::Empty, "sicily", 13.361389, 38.115556)` → act — `call_producer` → assert — `Err(CamelError::ProcessorError(msg))` with `msg` containing `"GEOADD"` and `"Body::Empty"`, and `server.recorded()` empty.
- `publish_prefers_the_value_header_and_takes_the_body_as_fallback`: setup — server scripted `:0\r\n` for PUBLISH twice; two exchanges with `CamelRedis.Channel` = `"news"`: one with header `CamelRedis.Value` = `"from-header"` + `Body::Text("from-body")`, one with no header + `Body::Text("from-body")` → act — two `call_producer` calls with `RedisCommand::Publish` → assert — first frame message argument is `from-header`, second is `from-body`.

**Acceptance:**
- `cargo test -p camel-component-redis --lib` passes in full.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.5

## camel-component-redis: streams codec

### Task 1.6: XADD goes through the shared codec; bytes bodies accepted

**Files:**
- `crates/components/camel-redis/src/commands/stream.rs` (modified)
- `crates/components/camel-redis/src/value_fake_tests.rs` (modified)
- `crates/components/camel-redis/src/stream_fake_tests.rs` (modified: one test case removed)

**Steps:**
1. In `stream.rs`, reimplement `resolve_xadd_body` (stream.rs:35-46) as
   `encode_body_value(&exchange.input.body, "XADD")` returning `Vec<u8>` (rename to
   `resolve_xadd_body_bytes` to keep NEW-symbol naming honest; update the single caller at
   stream.rs:94). Push the bytes as the `body` field argument (stream.rs:95-99). Keep the
   one-`body`-field rule, the `*` auto-id, and the `CamelRedis.StreamId` postcondition
   (stream.rs:117-120) unchanged.
2. In `stream_fake_tests.rs:2279-2283`, REMOVE the `("non-UTF-8 bytes",
   Body::from(vec![0xff, 0xfe]))` case from the `xadd_prohibited_body_forms_fail_before_send`
   cases array: bytes bodies are now accepted by design and are positively covered by
   `xadd_accepts_a_bytes_body`. The remaining Empty/Stream cases and the wording assertion
   (`contains("XADD") && contains("body")`, lines 2294-2297) survive the codec message
   unchanged.
3. Run the pre-existing XADD tests (`cargo test -p camel-component-redis --lib xadd`): text
   round-trip and wire-frame tests must pass unchanged.
4. Extend `empty_and_stream_bodies_fail_before_sending` (from Task 1.2) with two more cases:
   XADD with `Body::Empty` and XADD with a stream body → assert `ProcessorError` and empty
   `server.recorded()` (no strict wording match — XADD's codec message names the body field).
5. Write the test below in `value_fake_tests.rs`.

**Tests:**
- `xadd_accepts_a_bytes_body`: setup — server scripted `resp_bulk("1-1")` for XADD; exchange with `CamelRedis.Key` = `"s1"` and `Body::Bytes([0x00, 0xFF, 0x10])` → act — `call_producer` with `RedisCommand::Xadd` → assert — XADD frame args equal `["s1", "*", "body", <raw bytes>]` compared as raw byte vectors, header `CamelRedis.StreamId` = `"1-1"`, and the exchange body is unchanged.

**Acceptance:**
- `cargo test -p camel-component-redis --lib` passes in full.
- Pre-existing XADD text round-trip and wire-frame tests pass unmodified.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.6

## docs + ADR

### Task 1.7: Write ADR-0098 and update the component docs

**Files:**
- `docs/adr/0098-redis-value-contract.md` (new)
- `docs/src/components/redis.md` (modified)
- `crates/components/camel-redis/README.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Write `docs/adr/0098-redis-value-contract.md` with 0097-style front matter (`# ADR-0098:
   Redis value contract — body binding and binary safety`, `- Date: 2026-11-13`, `- Status:
   Accepted (decided 2026-11-13)`, `- Cross-refs: ADR-0090, ADR-0032, bd rc-01ka4`).
   Record: (a) the precedence rule — the command's value header wins (`CamelRedis.Value`;
   `CamelRedis.Member` for GEOADD, whose member is its value payload — the ruling rationale:
   decision 1's "keeps every existing route working" requires honoring the header GEOADD
   actually uses; no third source), otherwise the exchange body; (b) the codec table
   (Text→UTF-8, Json→compact, Xml→XML text, Bytes→verbatim, Empty/Stream→named error naming
   command+variant before send, future variants fail closed per ADR-0049); (c) the reply rule
   (Text if UTF-8 else Bytes; nil → Json(null); covered getters GET and MGET; MGET all-text
   stays the JSON array, binary elements → the lossless RESP-array `Body::Bytes` envelope);
   (d) map commands (HMSET header-or-JSON-object body; MSET header-only); (e) what stays out —
   no value streaming/chunking/compression, no new command variants, `rc-tic86` owns SET
   options/GETDEL/GETEX/collections; (f) consequences — GET reply shape changes from
   `Body::Json(string|null)` to Text/Bytes/Json(null) even when headers are present (breaking,
   commit carries `!`), MGET all-text replies unchanged, binary MGET previously errored.
2. In `docs/src/components/redis.md`, add a "Value source and reply rule" subsection after the
   producer intro: a header table row for `CamelRedis.Value` (and `CamelRedis.Member` for
   GEOADD) stating the body fallback and codec forms, and the reply rule sentence
   (GET/MGET Text/Bytes/Json(null), MGET text array unchanged). Keep it consistent with the
   existing prose at line ~210.
3. In `crates/components/camel-redis/README.md`, add a `CamelRedis.Value` row to the input
   header table (~lines 264-273) and replace the statements at lines 276-278 with the
   precedence + codec + reply-rule contract.
4. Add the ADR-0098 index line to `CONTEXT-MAP.md` `## Architecture Decisions` following the
   existing entry format (one line, crate list `(camel-component-redis)`).

**Tests:** (docs-only task; verification is the doc toolchain + spec validation)
- `cd docs && mdbook test` passes (the new redis.md prose must not break doc tests).
- `openspec validate redisvalue --type change --json` exits with `"valid": true`.

**Acceptance:**
- ADR-0098 exists with all four pinned decisions and the GEOADD ruling rationale.
- `cd docs && mdbook test` exits 0.
- `openspec validate redisvalue --type change --json` reports valid.
- `bash scripts/gates-local.sh` exits 0.

- [x] 1.7
