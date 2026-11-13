# Tasks: rediskeyowner

## camel-redis :: classifier (rc-bvfzk)

### Task 1.1: Remove Rename/Renamenx/Move from the idempotent classifier + regression tests

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified — test only)

**Steps:**
1. Write BOTH failing tests FIRST, before touching the classifier:
   a. In `config.rs`, add `rename_renamenx_move_are_not_idempotent` to the existing `mod tests` (near `test_is_idempotent_command_non_idempotent`, line ~2918): three `assert!(!is_idempotent_command(...))` calls for `RedisCommand::Rename`, `RedisCommand::Renamenx`, `RedisCommand::Move`.
   b. In `executor.rs`, add the test `non_idempotent_command_is_attempted_once` in the existing `mod tests` (next to `test_non_idempotent_command_not_retried`, line ~655). Import `is_idempotent_command` INSIDE `mod tests` (`use crate::config::is_idempotent_command;` at the top of the test module) — do NOT extend the file-level import at line 14, which would be an unused-import warning in non-test targets.
2. Run both tests; BOTH must FAIL (config test: all three classify idempotent; executor test: with `Rename` classified idempotent the loop retries — 2 calls, 1 reconnect, and ends `Ok` because FakeExecutor returns `Ok` after exhausting the sequence). Record the failures.
3. Implement the fix: in `is_idempotent_command` (config.rs:1550-1560), delete the three lines `| RedisCommand::Rename`, `| RedisCommand::Renamenx`, `| RedisCommand::Move` from the "Idempotent writes" arm. Directly below the `matches!` arm add one comment line explaining the absence: `// Rename/Renamenx/Move are deliberately absent: a retry after an unconfirmed success reports visible failure (RENAME: ERR no such key; RENAMENX/MOVE: 0).`
4. Re-run both tests; both must now PASS.

**Tests:**
- `rename_renamenx_move_are_not_idempotent`: no setup → call `is_idempotent_command` on `Rename`, `Renamenx`, `Move` → assert each returns `false`. Command: `cargo test -p camel-component-redis --lib rename_renamenx_move_are_not_idempotent`. Expected before fix: FAIL.
- `non_idempotent_command_is_attempted_once`: `FakeExecutor::new(vec![transient_err("connection reset")])`, `cmd = RedisCommand::Rename`, policy `max_attempts: 10`, delays 1ms/10ms (copy the existing test style) → call `execute_with_retry(&mut { executor }, &cmd, &mut exchange, is_idempotent_command(&cmd), &policy)` (the retry flag MUST come from the classifier, not a hardcoded `false`) → assert `result.is_err()`, `call_count == 1`, `reconnect_count == 0`. Command: `cargo test -p camel-component-redis --lib non_idempotent_command_is_attempted_once`. Expected before fix: FAIL (2 calls, 1 reconnect, returns `Ok`).

**Acceptance:**
- Both new tests pass; `cargo test -p camel-component-redis --lib` exits 0 (no existing test asserts the old classification of the three — verified: the idempotent-writes test at config.rs:2911 asserts only Set, Del, and Expire variants).
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.1

## camel-redis :: key resolution (rc-kcpuw)

### Task 2.1: URI `key=` wins over `CamelRedis.Key`; publish the effective key (unit + wire witnesses)

**Files:**
- `crates/components/camel-redis/src/producer.rs` (modified)
- `crates/components/camel-redis/src/stream_fake_tests.rs` (modified)

**Steps:**
1. Write the three WIRE tests FIRST and run them — they must run (and go red) before any unit test references the not-yet-existing `resolve_key`, which would break compilation of the whole test binary:
   a. In `stream_fake_tests.rs` (producer XADD/XLEN wire section, line ~1937): REPLACE `xadd_header_key_overrides_uri_key` (line ~2153) with `xadd_uri_key_overrides_exchange_header` (inverted expectation — the old assertion must NOT survive), ADD `xadd_header_key_used_when_uri_declares_none`, ADD `blpop_then_xadd_does_not_reuse_the_popped_key` (see Tests).
   b. Run the three wire tests; observe red: the two precedence tests FAIL at runtime against current code (`s2`/`in` win) while the fallback witness `xadd_header_key_used_when_uri_declares_none` stays green (expected — it witnesses unchanged behavior). Record the failures.
2. Add the two UNIT tests to `producer.rs` `mod tests`: `uri_key_wins_over_exchange_header` and `header_key_used_when_uri_declares_none` (see Tests). This step deliberately comes after the wire red-run: these reference `RedisProducer::resolve_key`, which does not exist yet, so the test binary fails to compile — that is the recorded red state for the unit pair.
3. In `producer.rs`, replace `fn apply_default_key(exchange: &mut Exchange, config: &RedisEndpointConfig)` (line ~65) with:
   `fn resolve_key(exchange: &Exchange, config: &RedisEndpointConfig) -> Option<String>` returning `config.key.clone()` when `config.key` is `Some`, otherwise `exchange.input.header("CamelRedis.Key").and_then(|v| v.as_str()).map(str::to_string)` (a non-string header value yields `None` — header left untouched, so `require_key` surfaces its existing `Missing required header: CamelRedis.Key` error).
4. In `Service::call` (line ~134), replace `Self::apply_default_key(&mut exchange, &config);` with the resolve-then-publish step:
   `if let Some(key) = Self::resolve_key(&exchange, &config) { exchange.input.set_header("CamelRedis.Key", serde_json::Value::String(key)); }` — publish happens BEFORE `execute_with_retry`, so command handlers keep reading `CamelRedis.Key` via `require_key` unchanged.
5. Adapt the remaining `apply_default_key` unit tests to the resolver API (the old fn is gone; compile breaks otherwise): DELETE `test_apply_default_key_preserves_existing_header` (line ~302) — it is replaced by `uri_key_wins_over_exchange_header`, which carries the inverted expectation; `test_apply_default_key_sets_when_missing` (line ~290) becomes a `resolve_key` call asserting `Some("cfg-key")` (rename to `resolve_key_returns_uri_key`); `test_apply_default_key_does_nothing_when_config_key_is_none` (line ~348) becomes a `resolve_key` call asserting `None` when the URI has no `key=` and no header is set (rename to `resolve_key_returns_none_without_sources`).
6. Run the full batch; every test from steps 1-2 must now be GREEN.

**Tests:**
- `uri_key_wins_over_exchange_header` (unit; REPLACES the deleted `test_apply_default_key_preserves_existing_header` and carries its inverted expectation): config `RedisEndpointConfig::from_uri("redis://localhost:6379?key=out")`, exchange with header `CamelRedis.Key = json!("in")` → `RedisProducer::resolve_key(&exchange, &config)` → assert `Some("out".to_string())`. Command: `cargo test -p camel-component-redis --lib uri_key_wins_over_exchange_header`. Red state before fix: compile error (`resolve_key` undefined).
- `header_key_used_when_uri_declares_none` (unit): config `from_uri("redis://localhost:6379")` (no `key=`), exchange with header `CamelRedis.Key = json!("s2")` → `resolve_key` → assert `Some("s2".to_string())`. Command: `cargo test -p camel-component-redis --lib header_key_used_when_uri_declares_none`.
- `xadd_uri_key_overrides_exchange_header` (wire, REPLACES `xadd_header_key_overrides_uri_key`): fake server answers XADD `5-0`; `call_producer(&server, "XADD", "&key=s1", keyed_exchange(Body::Text("hello".to_string()), "s2"))` → assert frame `["s1", "*", "body", "hello"]`, `CamelRedis.Stream == "s1"`, `CamelRedis.Key == "s1"` (effective key published). Command: `cargo test -p camel-component-redis --lib xadd_uri_key_overrides_exchange_header`. Red before fix: frame is `["s2", ...]`.
- `xadd_header_key_used_when_uri_declares_none` (wire): fake server answers XADD `5-0`; `call_producer(&server, "XADD", "", keyed_exchange(Body::Text("hello".to_string()), "s2"))` (no `key=` in URI) → assert frame `["s2", "*", "body", "hello"]`, `CamelRedis.Stream == "s2"`. Command: `cargo test -p camel-component-redis --lib xadd_header_key_used_when_uri_declares_none`. Green before AND after fix (this is the unchanged-fallback witness); include it so the delta scenario has a wire owner.
- `blpop_then_xadd_does_not_reuse_the_popped_key` (wire): fake server answers XADD `7-0`; exchange built with `body_exchange(Body::Text("job".to_string()))` then `exchange.input.set_header("CamelRedis.Key", serde_json::json!("in"))` (simulates an upstream BLPOP consumer, consumer.rs:735) → `call_producer(&server, "XADD", "&key=out", exchange)` → assert frame `["out", "*", "body", "job"]`, `CamelRedis.Key == "out"`, `CamelRedis.Stream == "out"`. Command: `cargo test -p camel-component-redis --lib blpop_then_xadd_does_not_reuse_the_popped_key`. Red before fix: frame is `["in", ...]` (WRONGTYPE on real Redis).

**Acceptance:**
- All five new/renamed tests pass; `cargo test -p camel-component-redis --lib` exits 0 — including the existing `xlen_returns_count_as_body` (stream_fake_tests.rs:1952, owns the MODIFIED XLEN requirement: same resolver, its URI/header agree so behavior is unchanged) and every other existing wire test.
- `grep -n "apply_default_key" crates/components/camel-redis/src/producer.rs` returns zero hits; `grep -rn "xadd_header_key_overrides_uri_key" crates/` returns zero hits.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 2.1

## docs

### Task 3.1: Invert the documented key precedence + migration note

**Files:**
- `docs/src/components/redis.md` (modified)
- `crates/components/camel-redis/README.md` (checked; modified only if it states the old precedence)

**Steps:**
1. `docs/src/components/redis.md` line ~241 (XADD/XLEN paragraph): replace "Both take the stream key from the `CamelRedis.Key` header, with the URI `key=` as the fallback." with "Both take the stream key from the URI `key=`, falling back to the `CamelRedis.Key` header when the URI declares no key; the effective key is published on `CamelRedis.Key`." Append one migration sentence: "Migration from pre-0.x routes that relied on header-over-URI: omit `key=` from the producer URI to keep per-message key routing."
2. `docs/src/components/redis.md` line ~251 (header table row): change the description cell from "Stream key; falls back to the URI `key=`" to "Stream key; the URI `key=` wins when present (omit `key=` for per-message keys)". Keep the exact 3-column table shape (`| \`CamelRedis.Key\` | input | ... |`).
3. `crates/components/camel-redis/README.md`: grep for the old-precedence phrasing (`falls back to the URI`, `header ... fallback`, `CamelRedis.Key` precedence claims). Verified occurrences (line ~185 dead-letter example, line ~266 header table "Redis key") do NOT state precedence — if confirmed by your own grep, change nothing and report "README: no old-precedence statement found"; if any statement is found, invert it and add the same one-sentence migration note.
4. Verify no stale claim remains: `grep -rn "falls back to the URI" docs/src/components/redis.md crates/components/camel-redis/README.md` returns zero hits.

**Tests:**
- Documentation task — no Rust tests. Verification command: `cd docs && mdbook test` must exit 0 (the guide's doctests/link checks still compile).

**Acceptance:**
- `cd docs && mdbook test` exits 0.
- `grep -rn "falls back to the URI" docs/src/components/redis.md crates/components/camel-redis/README.md` returns zero hits.
- Both touched spots carry migration guidance (full migration sentence in the paragraph; the compressed `omit \`key=\`` parenthetical in the table cell).

- [x] 3.1
