# Tasks: redisstreams

Single delivery phase (design.md ## Phases). Ordered: header rejection,
classification, producer commands, consumer group creation, live round
trip, docs, gates.

## camel-redis producer

### Task 1.1: Remove CamelRedis.Command header override (breaking)

**Files:**
- `crates/components/camel-redis/src/producer.rs` (modified)

**Steps:**
1. Change `RedisProducer::resolve_command` to return
   `Result<RedisCommand, CamelError>`: return
   `config.command.clone()`; when `exchange.input.header("CamelRedis.Command")`
   is `Some`, return
   `Err(CamelError::ProcessorError("untrusted header 'CamelRedis.Command' cannot select a Redis command; the endpoint URI 'command=' is the only command source (ADR-0032)".into()))`
   — regardless of the header's value or parsability. No connection is
   attempted before this check: the check runs first in `call()` before
   `apply_default_key`/`execute_with_retry`.
2. Update the `call()` call site to propagate the error with `?`.
3. Delete/rewrite the override-pinning tests
   (`test_resolve_command_from_header`,
   `test_resolve_command_header_overrides_config`,
   `test_resolve_command_invalid_header_falls_back_to_config`,
   `test_resolve_command_non_string_header_falls_back_to_config`).
4. Leave `is_idempotent_command` and the executor's
   `Xreadgroup => consumer-only` gate untouched.

**Tests:**
- `resolve_command_uses_uri_command`: URI
  `redis://localhost:6379?command=GET`, no header → `Ok(RedisCommand::Get)`.
- `resolve_command_rejects_valid_header`: URI `command=GET`, header
  `DEL` → `Err`, message contains `CamelRedis.Command` (regression: an
  injected DEL on a GET endpoint is rejected, not executed).
- `resolve_command_rejects_invalid_header`: header `NOT_A_COMMAND` →
  `Err` naming `CamelRedis.Command`.
- `resolve_command_rejects_non_string_header`: header `json!(123)` →
  `Err` naming `CamelRedis.Command`.
- `producer_rejects_command_header_before_connect`: producer on URI
  `redis://127.0.0.1:1?command=GET` (nothing listens there), exchange with
  header `CamelRedis.Command=DEL` → `call()` returns the header-naming
  error, NOT a connect error — proving rejection happened before dialing.
- `geo_uri_and_header_resolution_matches_spec`: URI
  `command=GEOSEARCH` parses to `RedisCommand::Geosearch` at the config
  level (URI alone, no header — assert the parse result directly); then
  a producer on that URI with header `GEOADD` returns the header-naming
  error from `call()` before any connection (covers the redis-geo
  MODIFIED scenario: URI resolves, header is rejected).

Command: `cargo test -p camel-component-redis --lib`. Expected before impl:
`resolve_command` tests fail to compile/change (signature differs) — they
must pass after.

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0.
- No test asserts header-override or fallback behavior.

- [x] 1.1

### Task 1.2: Classify NOGROUP/BUSYGROUP/NOPERM by server code

**Files:**
- `crates/components/camel-redis/src/transport_error.rs` (modified)

**Steps:**
1. In the classification walk (`is_transient_redis_error` rules), between
   rule 4 (the `RedisError::kind()` structural shapes) and the legacy
   substring fallback, insert: unwrap the source chain (reuse
   `unwrap_arc_dyn_error`), downcast to `redis::RedisError`, and when
   `RedisError::code()` (redis-rs 1.x, verified present) is
   `Some("NOGROUP")`, `Some("BUSYGROUP")`, or `Some("NOPERM")` → verdict
   not transient, decided by the code alone, before any text matching.
2. Document at the match site that these three codes deliberately diverge
   from the legacy table when a classifier word appears inside their
   message (spec: redis-failover MODIFIED, exemption sentence).

**Tests:**
- `stream_server_codes_classify_not_transient`: build real redis-rs
  `RedisError` values that carry a server code — parse the RESP error
  reply bytes with `redis::parse_redis_value`
  (`b"-NOGROUP no such group\r\n"` etc.; redis-rs 1.8.x — no
  `Value::Error` constructor exists) and convert with
  `Value::extract_error()`; for each of `NOGROUP`, `BUSYGROUP`,
  `NOPERM`: FIRST assert `code()` yields the expected code (the
  construction contract), THEN classify → not transient via the new rule.
- `noperm_message_with_classifier_word_stays_fatal`: same construction
  with `b"-NOPERM connection is not allowed for this user\r\n"` →
  `code() == Some("NOPERM")`, verdict not transient (the word
  `connection` in the message must NOT flip the verdict).
- `wrongtype_stays_not_transient`: existing behavior pinned
  (`-WRONGTYPE ...` → not transient, and not via the new code rule —
  `code()` is `Some("WRONGTYPE")`, which is not in the settled set).

Command: `cargo test -p camel-component-redis --lib`. Expected before impl: new
tests fail (rule absent — verdicts come from the legacy fallback, and
`noperm_message_with_classifier_word_stays_fatal` transiently returns
transient).

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: XADD and XLEN — full command surface and implementation

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)
- `crates/components/camel-redis/src/commands/mod.rs` (modified)
- `crates/components/camel-redis/src/commands/stream.rs` (new)
- `crates/components/camel-redis/src/stream_fake_tests.rs` (modified)

**Steps:**
1. Add `RedisCommand::Xadd` and `RedisCommand::Xlen` variants; parse
   `"XADD" => Ok(RedisCommand::Xadd)` and `"XLEN" => Ok(RedisCommand::Xlen)`
   in the URI parse match. Satisfy every exhaustive match on
   `RedisCommand` the variants introduce (executor producer dispatch,
   `is_string_command`, and any other match the compiler flags).
2. `is_idempotent_command` (config.rs:1399): add `Xlen` to the read list;
   do NOT add `Xadd`.
3. Executor producer match: `Xadd | Xlen =>
   commands::stream::dispatch(cmd, conn, exchange).await`.
4. New `commands/stream.rs` with `pub(crate) async fn dispatch` matching
   the other command modules' signature (see `commands/geo.rs`), with BOTH
   commands implemented — no stubs:
   - `Xlen`: key = `require_key` (the `CamelRedis.Key` header —
     `apply_default_key` already falls back to the URI `key=`), send
     `XLEN <key>`, write the returned count into the exchange body (u64),
     replacing it.
   - `Xadd`: key = `require_key`; body =
     `exchange.body.clone().try_into_text()` (`camel-api`
     `Body::try_into_text`, body.rs:325) — on error return the named
     processor error it produces; send `XADD <key> * body <text>` (entry
     ID always `*`, exactly one field named `body`); on success KEEP the
     exchange body unchanged, set header `CamelRedis.StreamId` to the
     returned entry ID and header `CamelRedis.Stream` to the key —
     `set_header` overwrites upstream values.

**Tests:**
- `xadd_xlen_parse_roundtrip`: `from_uri("redis://localhost?command=XADD")`
  → `RedisCommand::Xadd`; same for `XLEN` → `Xlen`.
- `xlen_is_idempotent_xadd_is_not`: `is_idempotent_command(&Xlen)` is
  true; `is_idempotent_command(&Xadd)` is false (spec: producer ADDED
  "XADD is non-idempotent" scenario).
- `xlen_returns_count_as_body`: fake RESP server answers `XLEN s1` with
  `7` → producer with URI `command=XLEN` and header `CamelRedis.Key=s1`
  (no URI key — exercises the header path the spec scenario pins) yields
  exchange body `7` and the recorded frame is `XLEN s1`.
- `xadd_wire_frame_and_postconditions`: fake RESP server answers XADD
  with `5-0`; exchange body text `hello` → recorded frame is
  `XADD s1 * body hello`, body still `hello`, headers
  `CamelRedis.StreamId=5-0`, `CamelRedis.Stream=s1`.
- `xadd_header_key_overrides_uri_key`: URI `command=XADD&key=s1`, header
  `CamelRedis.Key=s2` → frame `XADD s2 * body hello`, header
  `CamelRedis.Stream=s2`.
- `xadd_prohibited_body_forms_fail_before_send`: three exchanges —
  `Body::Empty`, `Body::Stream` with valid UTF-8 content, `Body::Bytes`
  with invalid UTF-8 — each fails with a processor error and records zero
  frames.
- `xadd_text_roundtrip_byte_for_byte` (fake harness): XADD text
  `María & café ☕` scripted, then the XREADGROUP consumer path reads the
  entry → `Body::Text("María & café ☕")` byte for byte.

Command: `cargo test -p camel-component-redis --lib`. Expected before impl: parse
test fails (unknown command error); the rest fail against the absent
module.

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0.
- No "not yet implemented" or stubbed command arm remains.

- [x] 1.3

### Task 1.4: XADD never retried; ambiguous failure = unknown outcome

**Files:**
- `crates/components/camel-redis/src/commands/stream.rs` (modified)
- `crates/components/camel-redis/src/stream_fake_tests.rs` (modified)

**Steps:**
1. Verify `execute_with_retry` consults `is_idempotent_command(&cmd)`
   (executor.rs:527-531 disables retries for non-idempotent commands) —
   `Xadd` is false, so the loop must not re-send on transient failure.
   Fix only if the loop retries non-idempotent commands.
2. In the XADD handler, classify transport failures conservatively:
   a failure raised while setting up the exchange (body conversion, key
   resolution, command build — before any I/O) keeps its plain error; a
   failure during the command I/O attempt (`query_async` exposes no
   write-completion status, so "was it written?" is unknowable) becomes
   a processor error naming the unknown outcome — "XADD outcome unknown:
   the command attempt failed during transport; the entry may or may not
   exist" — WITHOUT claiming the request was sent. Do not retry either
   way.

**Tests:**
- `xadd_transient_reply_sends_exactly_once`: fake RESP server answers
  the first XADD with a transient transport error (then would answer
  success) → producer returns `Err`, exactly ONE `XADD` frame recorded
  (no retry).
- `xadd_disconnected_after_write_names_unknown_outcome`: fake RESP
  server records the XADD frame then closes the connection without
  replying → producer returns `Err` whose message names the unknown
  outcome (without claiming the request was sent), and exactly one XADD
  frame was recorded by the harness.

Command: `cargo test -p camel-component-redis --lib`. Expected before impl:
`xadd_transient_reply_sends_exactly_once` passes after Task 1.3 (this
task proves it); `xadd_disconnected_after_write_names_unknown_outcome`
fails (message does not name unknown outcome yet).

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0.

- [x] 1.4

## camel-redis consumer

### Task 1.5: createGroup/groupStart URI params with parse-time validation

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/streams.rs` (modified)
- `crates/components/camel-redis/src/consumer.rs` (modified)

**Steps:**
1. Add to the Streams consumer config: `create_group: bool` (default
   `false`) and `group_start: Option<GroupStart>` where
   `enum GroupStart { Latest, Earliest }` is a NEW public enum in
   config.rs. Parse URI params `createGroup` (bool) and `groupStart`
   (exactly `latest` or `earliest`).
2. Validation at URI parse time (inside the same parse path as
   `parse_stream_recover_params`): `groupStart` present without
   `createGroup=true` → `Err` naming `groupStart`;
   `createGroup=true` without `groupStart` → `Err` naming `groupStart`;
   any other `groupStart` value → `Err` naming `groupStart`.
3. Thread the parsed values into `StreamConsumerParams` (streams.rs:46 —
   add the two fields) and its construction site in consumer.rs:144, so
   the streams session (Task 1.6) can read them.

**Tests:**
- `group_start_without_create_group_fails_at_parse`: URI
  `...command=XREADGROUP&key=s1&group=g&consumer=c&groupStart=latest` →
  `from_uri` errs naming `groupStart`.
- `create_group_without_group_start_fails_at_parse`: URI
  `...&createGroup=true` → errs naming `groupStart`.
- `invalid_group_start_value_fails_at_parse`: `groupStart=now` → errs
  naming `groupStart`.
- `valid_pair_parses`: `...&createGroup=true&groupStart=earliest` →
  `create_group == true`, `group_start == Some(GroupStart::Earliest)`.
- `defaults_parse`: plain XREADGROUP URI → `create_group == false`,
  `group_start == None`.

Command: `cargo test -p camel-component-redis --lib`. Expected before impl: parse
tests fail (params ignored or unknown).

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0.

- [x] 1.5

### Task 1.6: XGROUP CREATE once per session, readiness before the first read

**Files:**
- `crates/components/camel-redis/src/streams.rs` (modified)
- `crates/components/camel-redis/src/stream_fake_tests.rs` (modified)
- `crates/components/camel-redis/src/stream_session_tests.rs` (modified,
  if session-level fixtures live there)
- `crates/components/camel-redis/src/consumer.rs` (modified, only if the
  params handoff from Task 1.5 needs it)

**Steps:**
1. Add an `XGROUP CREATE <key> <group> <start> MKSTREAM` I/O operation to
   `StreamReadIo` (it has none today) and send it in `read_task_loop` ONCE
   per session, after the connection is established and BEFORE entering
   the `Connected` state that issues the first XREADGROUP. Preserve the
   loop's existing cancellation shape (the create send must be
   cancellable like the read I/O around it — no new unbounded wait).
   `GroupStart::Latest` → `$`, `GroupStart::Earliest` → `0`.
2. Match the reply error by downcasting to `RedisError` and comparing
   `code()` (reuse the transport_error chain-unwrapping helper):
   `Some("BUSYGROUP")` → success, log `info!` naming group/key, continue.
   Never send `XGROUP SETID`.
3. Ordering: create reply handled (success or BUSYGROUP) → fire readiness
   (`mark_ready` via the existing `on_ready` closure) → first XREADGROUP
   is sent. Readiness precedes the first read; it no longer waits for the
   first successful read when `create_group == true`. When
   `create_group == false` the existing readiness point is unchanged.
4. Add readiness-mark recording to the fake harness
   (stream_fake_tests.rs records frames/connections today): record a
   `mark_ready` event with its position in the frame log so
   create→ready→read ordering is assertable.
5. `create_group == false`: unchanged behavior — no XGROUP CREATE frame;
   the first read's NOGROUP stays fail-closed (non-transient via Task
   1.2) and the session returns `Err` for supervision.
6. NOPERM/WRONGTYPE on create: non-transient → session returns `Err` for
   supervision (no create retry).

**Tests:**
- `create_group_sends_mkstream_before_first_read_and_readies_first`: fake
  RESP server, URI `...&createGroup=true&groupStart=earliest` → recorded
  order: connect, `XGROUP CREATE s1 g 0 MKSTREAM`, readiness mark,
  XREADGROUP. Exactly one CREATE frame for the session.
- `busygroup_is_success_with_info_log_no_setid`: fake server answers
  XGROUP CREATE with `-BUSYGROUP Consumer Group name already exists` →
  session proceeds (readiness fires, XREADGROUP sent), an `info!` event
  was recorded by the tracing test subscriber, and NO `XGROUP SETID`
  frame exists in the log.
- `noperm_on_create_returns_err_non_transient`: fake server answers
  XGROUP CREATE with `-NOPERM ...` → session `Err`, classified
  non-transient (supervision path), no re-send of XGROUP CREATE.
- `default_path_nogroup_supervision_no_create_frame`: plain URI (no
  createGroup) against a server answering the first read with `-NOGROUP
  no such group` → zero `XGROUP CREATE` frames, session `Err` classified
  non-transient for supervision.

Command: `cargo test -p camel-component-redis --lib`. Expected before impl: frame
order tests fail (no XGROUP CREATE sent).

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0.

- [x] 1.6

## camel-test live round trip

### Task 1.7: Live testcontainers round trip on redis:7.0-alpine

**Files:**
- `crates/camel-test/tests/redis_streams_roundtrip_test.rs` (new)

**Steps:**
1. Copy the testcontainers setup pattern from
   `crates/camel-test/tests/redis_test.rs` (image pinned
   `redis:7.0-alpine`; the file carries
   `#![cfg(feature = "integration-tests")]` — replicate the same
   feature-gated header).
2. Test flow: (a) producer `command=XADD&key=rt` writes text `hello`;
   (b) consumer `command=XREADGROUP&key=rt&group=g&consumer=c&createGroup=true&groupStart=earliest`
   delivers one Exchange with body `hello` and headers `CamelRedis.Stream=rt`,
   `CamelRedis.StreamId=<id>`; the route completes (XACK after pipeline
   success is the consumer's own behavior); (c) producer
   `command=XLEN&key=rt` reads `1`; (d) raw redis connection (redis crate,
   same test) asserts `XPENDING rt g` summary count `0` — pending list
   empty.
3. Bounded acknowledgement barrier before asserting pending-empty:
   `XPENDING rt g` is polled with a tokio interval loop (50 ms, cap 2 s,
   no `thread::sleep` — lint-test-sleep) until the summary count is `0`;
   the final assertion requires count `0`. `send_and_wait` completing
   does NOT prove the asynchronous XACK landed.
4. Follow the existing local-run pattern for Docker availability
   (inspect `redis_test.rs` first and match it).

**Tests:**
- `live_streams_roundtrip_xadd_consumer_ack_xlen_pending_empty`: full
  flow above (with the bounded ack barrier), asserting body
  byte-equality (`hello`), XLEN `1`, pending `0`.

Command: `cargo test -p camel-test --features integration-tests --test
redis_streams_roundtrip_test -- --nocapture`. Expected before impl: file
absent; after impl passes against Docker. (Without the feature flag the
cfg-gated test compiles away and "passes" vacuously — never run it
without `--features integration-tests`.)

**Acceptance:**
- Test exits 0 locally with Docker running.
- `cargo clippy -p camel-test --all-targets -- -D warnings` exits 0.
- Round-trip text is byte-identical (spec: producer ADDED round-trip
  scenario).

- [x] 1.7

## docs

### Task 1.8: Component docs for streams producer + header migration

**Files:**
- `docs/src/components/redis.md` (modified)

**Steps:**
1. `## Commands` table: add Stream group row:
   `XREADGROUP`, `XADD`, `XLEN`; note XADD/XLEN are producer commands and
   XREADGROUP is consumer-only.
2. `## Producer`: XADD section — one `body` field from
   `try_into_text`, ID `*`, key via `CamelRedis.Key`/URI, body kept,
   `CamelRedis.StreamId` + `CamelRedis.Stream` set (producer header
   table), at-least-once paragraph (non-idempotent, never retried,
   ambiguous failure = unknown outcome, duplicates on redelivery,
   Idempotent Consumer + `RedisIdempotentRepository` mitigation,
   `CamelRedis.StreamId` is not a dedup key); XLEN section (count as
   body).
3. Version floor sentence: "Redis >= 7.0 (Valkey >= 7.2 compatible)".
4. Migration note for the header removal (breaking): `CamelRedis.Command`
   is rejected; use `choice`/`when` with one endpoint per command —
   include a short YAML route example.
5. `### Streams consumer groups` parameter table: add `createGroup`
   (default `false`) and `groupStart` (`latest`/`earliest`, required with
   `createGroup=true`, `$`/`0` mapping, MKSTREAM, BUSYGROUP = existing
   group wins) rows.

**Tests:**
- `cd docs && mdbook test` exits 0 (no broken links/code fences).

**Acceptance:**
- `mdbook test` exits 0.
- No doc text claims the header override works.
- Migration example uses `choice`/`when` with one endpoint per command.

- [x] 1.8

## gates

### Task 1.9: Full local gate sweep

**Files:** none (verification only; fixes land in the touched files)

**Steps:**
1. `cargo fmt --check --all`
2. `cargo clippy --workspace --all-features --exclude camel-cli
   --exclude camel-component-kafka --exclude security-keycloak
   --exclude security-wasm-policy --all-targets -- -D warnings` and the
   three extra clippy invocations from AGENTS.md QUALITY GATES.
3. `cargo test -p camel-component-redis --lib`,
   `cargo test -p camel-test --features integration-tests --test
   redis_streams_roundtrip_test`
4. `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-redis -p camel-test
   --no-deps`
5. `bash scripts/gates-local.sh`

**Tests:** n/a (this task IS the verification).

**Acceptance:**
- Every command exits 0; failures loop back to the owning task.

- [x] 1.9
