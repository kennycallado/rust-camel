# Tasks: redisbodyonly

Single-phase change: the resolver swap, the ten-command port, the consumer
ports, the wire tests, and the docs land as one coherent slice inside
`camel-component-redis` plus its dependents. Tasks are ordered so the tree
compiles and `cargo test -p camel-component-redis --lib` stays green after
every task.

## camel-component-redis

### Task 1.1: Body-only codec, tombstone check, and `body_value_bytes` resolver in commands/mod.rs

**Files:**
- `crates/components/camel-redis/src/commands/mod.rs` (modified)
- `crates/components/camel-redis/src/commands/string.rs` (modified — SET arm only)
- `crates/components/camel-redis/src/value_fake_tests.rs` (modified — delete the SET header-precedence test)

**Steps:**
1. In `encode_body_value`, replace the `Body::Json(v) => serde_json::to_string(v)` arm with a match on the JSON value: `Value::String(s)` contributes `s.as_bytes().to_vec()` (raw characters, no quoting); `Value::Null` returns `Err` from a new named constructor `json_null_body(verb)` producing `"{verb} value: the exchange body is Body::Json(null); a JSON null body is not a valid payload"`; every other `Value` keeps compact JSON serialization. This automatically applies the new codec to XADD, which calls `encode_body_value` directly.
2. Add `pub(crate) fn body_value_bytes(exchange: &Exchange, verb: &str, tombstones: &[&str]) -> Result<Vec<u8>, CamelError>`: first, for each name in `tombstones`, if `exchange.input.header(name).is_some()`, return `Err(CamelError::ProcessorError(format!("{name} was removed in 0.69; put the value in the body")))`; then return `encode_body_value(&exchange.input.body, verb)`. Do NOT append the old `; or set {value_header}` hint — the body is the only source.
3. Keep `resolve_value_bytes` untouched for now (tasks 2.1–4.1 migrate its remaining callers; task 4.1 deletes it).
4. Port the SET live dispatch in `crates/components/camel-redis/src/commands/string.rs` to the new resolver now (switch the `SET` arm from `resolve_value_bytes(exchange, "SET", HEADER_VALUE)` to `body_value_bytes(exchange, "SET", &[crate::HEADER_VALUE])`) so `body_value_bytes` has a production caller and no dead_code denial trips the clippy gate. Every other command keeps its current resolver until its own task.
5. In `crates/components/camel-redis/src/value_fake_tests.rs` delete `value_header_still_wins_over_the_body` (the SET tombstone from step 4 would leave it red) and update any pre-existing test that asserts the removed `; or set CamelRedis.Value` hint suffix in encode errors — the body path no longer appends a header hint.
6. In the `#[cfg(test)] mod tests` of mod.rs, add the codec unit tests listed below using the existing `ex_with` / exchange helpers in that module.

**Tests:**
- `codec_json_string_yields_raw_characters`: exchange with `Body::Json(serde_json::Value::String("abc".into()))` → `body_value_bytes(&ex, "SET", &[])` → `Ok(b"abc".to_vec())` (three bytes, no quote bytes). (Unit-level; the mission test of the same behavior is the task 5.1 wire test.)
- `codec_rejects_json_null_body`: exchange with `Body::Json(serde_json::Value::Null)` → `body_value_bytes(&ex, "SET", &[])` → `Err`, message contains `Body::Json(null)` and `SET`.
- `codec_accepts_empty_string_rejects_empty_body`: `Body::Text("")` → `Ok(Vec::new())`; `Body::Json(serde_json::Value::String(String::new()))` → `Ok(Vec::new())`; `Body::Empty` → `Err` naming `Body::Empty`.
- `tombstone_header_fails_before_encoding`: exchange with `CamelRedis.Value` header `"v"` and `Body::Text("body")` → `body_value_bytes(&ex, "SET", &[crate::HEADER_VALUE])` → `Err` containing `CamelRedis.Value was removed in 0.69`.
- Pre-existing `encode_body_value` tests must keep passing (Text/Bytes/Stream arms unchanged).

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.1

### Task 2.1: Port string.rs and list.rs to the body-only resolver

**Files:**
- `crates/components/camel-redis/src/commands/string.rs` (modified)
- `crates/components/camel-redis/src/commands/list.rs` (modified)
- `crates/components/camel-redis/src/commands/mod.rs` (modified — shared map helper moves here)
- `crates/components/camel-redis/src/commands/hash.rs` (modified — `resolve_hmset_values` calls the moved helper)

**Steps:**
1. Move `hmset_body_map`, `parse_hmset_body`, and `unencodable_hmset_body` from hash.rs into mod.rs as one shared helper `pub(crate) fn map_body_map(body: &Body, verb: &str) -> Result<serde_json::Map<String, serde_json::Value>, CamelError>` (same arms: Json object as-is, Text parsed, UTF-8 Bytes parsed, everything else a named pre-send error naming `verb`), and have `resolve_hmset_values` call it (hash.rs keeps compiling).
2. In string.rs live `dispatch`: switch `SETNX`, `SETEX`, `APPEND` from `resolve_value_bytes(exchange, verb, HEADER_VALUE)` to `body_value_bytes(exchange, verb, &[crate::HEADER_VALUE])` (SET already moved in task 1.1); switch `GETSET` from `require_value` + `value_to_redis_arg` to `body_value_bytes(exchange, "GETSET", &[crate::HEADER_VALUE])` and pass `&value` (bytes) to `redis::cmd("GETSET")` via `query_async`.
3. In string.rs: rewrite `resolve_mset_values` to take the map from the body only — first a tombstone check (`exchange.input.header("CamelRedis.Values").is_some()` returns the named tombstone error `CamelRedis.Values was removed in 0.69; put the value in the body`), then `map_body_map(&exchange.input.body, "MSET")` with each entry encoded by the scalar rule (`Value::String(s)` → `s`, other → compact JSON, `Value::Null` → named `MSET` pre-send error); the old `CamelRedis.Values` header branch is gone. Replace the two Values-header unit tests `test_resolve_mset_values_extracts_pairs` (string.rs:454) and `test_resolve_mset_values_requires_header` (string.rs:490, asserts the old header error) with body-map tests: a `Body::Json` object yields the pairs, and a `Body::Json({"bad":null})` body errors before send.
4. In list.rs live `dispatch`: switch `LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`, `LINSERT`, `LSET`, `LREM` to `body_value_bytes(exchange, verb, &[crate::HEADER_VALUE])`.
5. Delete `#[allow(dead_code)] build_redis_cmd` from string.rs and list.rs together with every `#[cfg(test)]` test that only exercises it, and with the now-unused `require_value`-based helper `resolve_set_args`-style closures in list.rs (grep `require_value` in both files; after the port there must be zero hits).
6. Update the files' unit tests: keep header-resolution error tests (missing key, missing field, malformed increment), delete dead-builder arg-assembly tests (wire assertions arrive in task 5.1's fake-server tests).

**Tests:**
- Existing unit tests in string.rs/list.rs that do not touch `build_redis_cmd` or `require_value` keep passing (e.g. `resolve_increment` defaults, `resolve_lrem_count` validation).
- `cargo test -p camel-component-redis --lib` exits 0 after the dead-builder tests are removed.

**Acceptance:**
- `grep -n "require_value\|value_to_redis_arg" crates/components/camel-redis/src/commands/string.rs crates/components/camel-redis/src/commands/list.rs` returns zero hits.
- `cargo test -p camel-component-redis --lib` exits 0.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0 and reports no dead_code warnings for string.rs/list.rs builders (they are gone).
- `cargo fmt --check --all` exits 0.

- [x] 2.1

### Task 3.1: Port set.rs and zset.rs to the body-only resolver

**Files:**
- `crates/components/camel-redis/src/commands/set.rs` (modified)
- `crates/components/camel-redis/src/commands/zset.rs` (modified)

**Steps:**
1. In set.rs live `dispatch`: switch `SADD` to `body_value_bytes(exchange, "SADD", &[crate::HEADER_VALUE])`; switch `SREM`, `SISMEMBER`, `SMOVE` from `require_value` + `value_to_redis_arg` to `body_value_bytes(exchange, verb, &[crate::HEADER_VALUE])` and pass the bytes to `redis::cmd(...)` / the typed conn methods as byte arguments.
2. In zset.rs live `dispatch`: switch `ZADD` to `body_value_bytes(exchange, "ZADD", &[crate::HEADER_VALUE])`; switch `ZREM`, `ZRANK`, `ZREVRANK`, `ZSCORE`, `ZINCRBY` from `require_value` + `value_to_redis_arg` to `body_value_bytes(exchange, verb, &[crate::HEADER_VALUE])` with bytes arguments. Score/increment/destination stay header-resolved (addressing operands).
3. Delete the `#[allow(dead_code)] build_redis_cmd` builders from both files together with their tests and the `require_value`-based helpers they use (set.rs lines with the `(require_key, require_value)` tuple helper, zset.rs equivalents).
4. Update remaining unit tests the same way as task 2.1 (keep header-resolution error tests, drop dead-builder tests).

**Tests:**
- Existing non-builder unit tests keep passing (`is_idempotent_command` classification, missing-key errors).
- `cargo test -p camel-component-redis --lib` exits 0.

**Acceptance:**
- `grep -n "require_value\|value_to_redis_arg" crates/components/camel-redis/src/commands/set.rs crates/components/camel-redis/src/commands/zset.rs` returns zero hits.
- `cargo test -p camel-component-redis --lib` exits 0.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 3.1

### Task 4.1: Port hash.rs, geo.rs, pubsub.rs; delete the old resolver trio

**Files:**
- `crates/components/camel-redis/src/commands/hash.rs` (modified)
- `crates/components/camel-redis/src/commands/geo.rs` (modified)
- `crates/components/camel-redis/src/commands/pubsub.rs` (modified)
- `crates/components/camel-redis/src/commands/mod.rs` (modified — cleanup)
- `crates/components/camel-redis/src/lib.rs` (modified — `HEADER_VALUE` doc comment)
- `crates/components/camel-redis/src/value_fake_tests.rs` (modified — port the PUBLISH header-precedence test)

**Steps:**
1. In hash.rs live `dispatch`: switch `HSET`, `HSETNX` to `body_value_bytes(exchange, verb, &[crate::HEADER_VALUE])`; rewrite `resolve_hmset_values` to `body_value_bytes`-free body-only map resolution: tombstone check `CamelRedis.Values` (return the named tombstone error if present), then `map_body_map(&exchange.input.body, "HMSET")` with scalar-rule entry encoding including the null-entry rejection; update the `unencodable_hmset_body` message to `"...provide a JSON object body"` (drop the `or set CamelRedis.Values` suffix).
2. In geo.rs: switch the `GEOADD` member resolution from `resolve_value_bytes(exchange, "GEOADD", MEMBER_HEADER)` to `body_value_bytes(exchange, "GEOADD", &[MEMBER_HEADER])`; update `MEMBER_HEADER`'s doc comment (geo.rs:95-97) — it currently says GEOADD falls back to the exchange body when the header is absent, which is false after this change: GEOADD takes the member from the body only and tombstones the header; leave `Member2`/`Members` and every other GEO command untouched (addressing operands — the existing geo.rs unit tests resolving `Member`/`Member2`/`Members` must stay green, they are the ownership evidence for the GEODIST addressing-operand scenario).
3. In pubsub.rs: switch `PUBLISH` to `body_value_bytes(exchange, "PUBLISH", &[crate::HEADER_VALUE])`.
4. In value_fake_tests.rs port `publish_prefers_the_value_header_and_takes_the_body_as_fallback` (line ~458): it sets `CamelRedis.Value` and asserts header precedence — rewrite it as a PUBLISH body-only wire test (message from the body, `publish_takes_the_message_from_the_body`) so the PUBLISH tombstone does not leave it red.
5. Delete `#[allow(dead_code)] build_redis_cmd` from hash.rs with its tests; delete `resolve_hash_values_map` (the `CamelRedis.Values` reader) from hash.rs together with its direct caller test `test_hmset_values_extraction` (hash.rs:433-446); replace the HMSET header-precedence test `hmset_header_wins_over_body` with a Values-tombstone test — HMSET with `CamelRedis.Values` set and a `Body::Empty` body asserts the tombstone error fires before body encoding, proving tombstone precedence over rejection order.
6. In mod.rs delete `resolve_value_bytes`, `require_value`, and `value_to_redis_arg`; grep the whole crate (`grep -rn "resolve_value_bytes\|require_value\|value_to_redis_arg\|resolve_hash_values_map" crates/components/camel-redis/src/`) and port or delete every remaining reference, including mod.rs's own unit tests (the header-resolves tests become tombstone/body tests already written in task 1.1).
7. Check `HEADER_VALUE` in lib.rs: keep the `pub const` (it names the tombstone in messages and docs), update its doc comment to state it is a removed header rejected with a tombstone error.

**Tests:**
- `cargo test -p camel-component-redis --lib` exits 0.
- `grep -rn "resolve_value_bytes\|require_value\|value_to_redis_arg\|resolve_hash_values_map" crates/components/camel-redis/src/` returns zero hits.

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- Zero hits for the four deleted symbols across the crate.

- [x] 4.1

### Task 5.1: Wire-level fake-server tests for the mission test names

**Files:**
- `crates/components/camel-redis/src/value_fake_tests.rs` (modified)
- `crates/components/camel-redis/src/stream_fake_tests.rs` (modified)

**Steps:**
1. In value_fake_tests.rs, rename `set_takes_the_exchange_body_when_value_header_is_absent` to `set_carries_the_body_as_the_value`, dropping the back-compat wording from the module doc comment (`value_header_still_wins_over_the_body` was already deleted in task 1.1).
2. Add the tombstone test: iterate the matrix — SET with `CamelRedis.Value`, HMSET with `CamelRedis.Values`, MSET with `CamelRedis.Values`, GEOADD (with `CamelRedis.Longitude`/`Latitude` and body) with `CamelRedis.Member` — each asserting the call fails with an error containing `was removed in 0.69` and `server.args_of(<CMD>)` is empty (no frame reached the fake server).
3. Add the codec, binary-symmetry, map, and empty-string tests listed below, using the `StreamFakeServer` + `call_producer` + `args_of` harness and `keyed_exchange`/`body_exchange` helpers; GEOADD tests need `CamelRedis.Longitude`, `CamelRedis.Latitude` headers.
4. In stream_fake_tests.rs add the XADD tests listed below, asserting the exact field list of the XADD frame.
5. Run the whole fake suite: `cargo test -p camel-component-redis --lib value_fake_tests` and `cargo test -p camel-component-redis --lib stream_fake_tests`.

**Tests:**
- `value_header_tombstone_fails_before_send`: for each matrix row (SET/Value, HMSET/Values, MSET/Values, GEOADD/Member) → call_producer → Err containing `was removed in 0.69`; `server.args_of(cmd)` empty in all four rows.
- `json_string_body_contributes_raw_characters`: SET with `Body::Json(Value::String("abc"))` → frame `[b"k", b"abc"]` (no quote bytes).
- `json_null_body_is_rejected`: SET with `Body::Json(Value::Null)` → Err; no SET frame.
- `bytes_body_round_trips_zadd_then_zscore`: ZADD with `CamelRedis.Score` = `1.5` and `Body::Bytes(b"\xFF\xFEbin")` → ZADD frame member bytes verbatim; scripted ZSCORE reply `bulk 1.5`; ZSCORE with the same `Body::Bytes` → `Ok`, ZSCORE frame member bytes verbatim, body is JSON number 1.5.
- `bytes_body_round_trips_sadd_then_sismember`: SADD with `Body::Bytes(b"\xFF\xFEbin")` → frame verbatim, scripted reply `:1`; SISMEMBER same body, scripted reply `:1` → `Ok`, frame verbatim, body JSON true.
- `mset_takes_a_json_object_body`: MSET with `Body::Json({"k1":"v1","k2":"v2"})` → frame `[k1, v1, k2, v2]`, values unquoted.
- `hmset_map_values_follow_the_scalar_rule`: HMSET with `Body::Json({"n":42,"s":"txt","o":{"k":1}})` → frame `[field..., "42", "txt", "{\"k\":1}"]`; then HMSET with `Body::Json({"bad":null})` → Err naming HMSET and no HMSET frame; then MSET with `Body::Json({"bad":null})` → Err naming MSET and no MSET frame.
- `smove_destination_stays_a_header_member_comes_from_the_body`: SMOVE with `CamelRedis.Key`, `CamelRedis.Destination`, `Body::Text("member1")` → frame `[src, dest, "member1"]`, member last, no extra operand.
- `empty_string_body_is_accepted_and_empty_body_is_not`: SET `Body::Text("")` → frame `[b"k", b""]`; SET `Body::Json(Value::String(""))` → frame `[b"k", b""]`; SET `Body::Empty` → Err, no frame.
- `xadd_does_not_split_a_json_object_into_fields` (stream_fake_tests.rs): XADD with `Body::Json({"sensor":"s1","temp":21})` → exactly one field pair `["body", "{\"sensor\":\"s1\",\"temp\":21}"]`; XADD with `Body::Json(Value::String("abc"))` → one pair `["body", "abc"]`.
- Existing value_fake_tests/stream_fake_tests tests that set `CamelRedis.Value` or `CamelRedis.Values` as payload must be ported to bodies in this task (grep both files).

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0 (all fake suites green).
- Every test name in this block exists exactly once across the two files.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0; `cargo fmt --check --all` exits 0.

- [x] 5.1

## Consumer ports

### Task 6.1: Port camel-test suites, CLI e2e, redis_state fixture, and both examples

**Files:**
- `crates/camel-test/tests/redis_test.rs` (modified)
- `crates/camel-test/tests/redis_sentinel_test.rs` (modified)
- `crates/camel-test/tests/support/mod.rs` (modified)
- `crates/camel-cli/tests/test_scenario_cli_e2e.rs` (modified)
- `crates/camel-integration-test/tests/fixtures/redis_state/order.test.yaml` (modified)
- `examples/redis-example/src/main.rs` (modified)
- `examples/redis-sentinel/src/main.rs` (modified)

**Steps:**
1. Grep all seven files for `CamelRedis.Value`, `CamelRedis.Values`, and `CamelRedis.Member` (GEOADD context only). For each hit that feeds a value-payload command, remove the header and put the same value in the exchange body (`set_body` / the YAML `body:` field); the CLI e2e test currently sets header AND body to the same value — delete the header line, keep the body.
2. In `support/mod.rs`, update any helper that constructs payload headers (e.g. a `value_exchange`-style helper) to construct bodies instead, keeping the helper signature callers already use.
3. In the redis_state fixture YAML, move the payload from the header mapping to the body mapping of the same step.
4. In examples: `redis-example` has `CamelRedis.Value` (two SET sites) and `CamelRedis.Member` for GEOADD (line ~109) — set the body before the producer step instead; `redis-sentinel` has one `CamelRedis.Value` SET site. Update surrounding comments that mention the value header.
5. Compile gates: `cargo check -p camel-test --tests`, `cargo check -p camel-cli --tests`, `cargo check -p redis-example`, `cargo check -p redis-sentinel`. Live Redis/Docker suites stay in CI (compile-only here).

**Tests:**
- Compile-only: the four `cargo check` commands above exit 0.
- No assertions are weakened: grep the ported suites after the edit — every former value-header payload appears as a body.

**Acceptance:**
- Payload-header uses are gone: `grep -rn "CamelRedis.Value\b\|CamelRedis.Values\b" crates/camel-test/tests crates/camel-cli/tests crates/camel-integration-test/tests/fixtures examples/redis-example examples/redis-sentinel` returns zero hits. GEO addressing-operand uses (`CamelRedis.Member`/`Member2`/`Members` on GEOPOS/GEODIST/GEOSEARCH/GEOHASH, e.g. redis_test.rs:369-451) MUST remain — they are mandated coverage, do not delete them.
- The four `cargo check` commands exit 0.

- [x] 6.1

## Docs and canon prose

### Task 7.1: Rewrite ADR-0098 in place and update docs, README, canon spec prose, CONTEXT-MAP

**Files:**
- `docs/adr/0098-redis-value-contract.md` (modified — rewritten in place)
- `openspec/specs/redis-values/spec.md` (modified — Purpose section and trailing prose only; requirement blocks sync from the delta at archive)
- `openspec/specs/redis-geo/spec.md` (modified — Purpose section only)
- `crates/components/camel-redis/README.md` (modified)
- `docs/src/components/redis.md` (modified)
- `docs/src/testing/scenario-redis.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Load the `ste-writing` skill before editing any `.md` file.
2. Rewrite `docs/adr/0098-redis-value-contract.md` in place: same file, new status line `Amends: none` stays, decision sections become — (1) payload binding: body-only for the scalar list incl. GETSET/SREM/SISMEMBER/SMOVE/ZREM/ZRANK/ZREVRANK/ZSCORE/ZINCRBY/LREM and the GEOADD member; (2) codec: JSON string raw characters, other JSON compact, Text/Xml UTF-8, Bytes verbatim, reject Empty/Stream/Json(null) before send; (3) reply rule unchanged (Text/Bytes/null); (4) map commands: HMSET/MSET JSON-object body, `CamelRedis.Values` gone, null entries rejected, binary map values a documented limit; (5) stream entry: XADD one-`body`-field, JSON object not split; (6) tombstones: the scope matrix (Value on scalar commands, Values on HMSET/MSET, Member on GEOADD) with the named error text and the ADR-0090 rationale; (7) out of scope: no valueSource option, no shim, rc-tic86 unchanged. Context section: why the one-day-old header-first cut is superseded (header never preserved the body — every producer overwrites it with the reply; ADR-0090 bans deprecated surfaces). Keep the `Bd:` cross-refs and add bd `rc-lv8jk`.
3. In `openspec/specs/redis-values/spec.md`: replace the `TBD` Purpose line with a two-sentence purpose (body-only payload contract for the Redis producer, codec + tombstones) and delete the stale trailing sentence about GEOADD's member fallback (line 72 region). Do NOT touch requirement blocks — `cargo xtask archive redisbodyonly` syncs those from the delta.
4. In `openspec/specs/redis-geo/spec.md`: update the Purpose paragraph — GEOADD's member comes from the body; `Member`/`Member2`/`Members` remain addressing operands on the read commands.
5. Update `crates/components/camel-redis/README.md`: replace every value-header example with body-based routes; add the migration note: move the header value into the body; for "value fixed, body for logging" stash the body in a property (or use `enrich`) and `setBody` before the Redis step; tombstone behavior table for the three removed headers.
6. Update `docs/src/components/redis.md` and `docs/src/testing/scenario-redis.md` the same way (they document the header contract today); keep mdbook include-anchors intact.
7. Update `CONTEXT-MAP.md` wherever it references the Redis value contract or `CamelRedis.Value`.
8. Verify docs: `cd docs && mdbook build` and `cd docs && mdbook test` (from the repo root; both must exit 0). If mdbook is not on PATH use `nix shell nixpkgs#mdbook -c mdbook build docs`.

**Tests:**
- `cd docs && mdbook build` exits 0; `cd docs && mdbook test` exits 0.
- `grep -rn "CamelRedis.Value" docs/src crates/components/camel-redis/README.md CONTEXT-MAP.md` returns hits only inside the tombstone/migration context (naming the removed header), never as a supported input.

**Acceptance:**
- ADR-0098 contains the tombstone matrix and the migration note; no "value header wins" precedence text remains anywhere.
- mdbook build and test exit 0.
- `cargo xtask lint-single-source` and `cargo xtask lint-context-citations` exit 0 (docs governance gates touched by these edits).
- `openspec validate redisbodyonly --type change --json` exits 0 (items valid).
- `cargo xtask archive redisbodyonly --check` exits 0 (delta/canon compatibility after the canon Purpose edits).
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0 (mission gate).
- `bash scripts/gates-local.sh` exits 0 (final pre-park sweep; the mission's remaining named gates — fmt, lib tests, `cargo check` of the ported suites and examples — were already run per-task; record all exit codes for the park).

- [x] 7.1
