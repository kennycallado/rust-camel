# Tasks: redis-state-tier

<!--
  Single-phase change. `design.md` records no `## Phases` section: the
  whole change is one cohesive deliverable (the Redis state-family
  adapter). Five ordered tasks; the `## <Module/Crate>` groups are the
  whole grouping.

  Compile-green ordering: adding the `ScenarioTarget::Redis` variant
  breaks the exhaustive matches at `document.rs::ScenarioTarget::bindings`
  and `runner.rs::validate_action` (both the tuple dispatch and the inner
  message-expectation match). Those three sites, the executor, and the
  pure law land together in Task 1, so no intermediate commit has an
  unreachable/partial match or dead code.

  Worker protocol: write the listed tests FIRST, run them against the
  not-yet-implemented code and record the expected RED, then implement the
  steps until the same command is GREEN. Do not invent a test the list
  does not name; do not weaken an assertion to make GREEN.
-->

## camel-integration-test — Redis grammar, value law, and executor

### Task 1: `redis` target grammar, fail-closed projection, atomic executor, dispatch, and steering label

**Files:**
- `crates/camel-integration-test/src/document/redis_target.rs` (new)
- `crates/camel-integration-test/src/document/validate.rs` (modified)
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/lib.rs` (modified)
- `crates/camel-integration-test/Cargo.toml` (modified)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)
- `crates/camel-integration-test/src/runner/redis_validate.rs` (new)
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-integration-test/src/steering.rs` (modified)

**Steps:**
1. In `document/redis_target.rs` add the target types:
   `#[non_exhaustive] #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub
   enum RedisType { String, Hash, List, Set, Zset }` (the ADR-0049
   `lint-non-exhaustive` posture for a public enum) and
   `#[derive(Debug, Clone, PartialEq)] pub struct RedisTarget { pub
   datasource: String, pub key: String, pub r#type: RedisType, pub ttl:
   Option<CountBound> }`; the raw twin `struct RawRedisTarget {
   datasource: String, key: String, r#type: String, ttl:
   Option<RawRedisTtl> }` and `struct
   RawRedisTtl { at_least: Option<String>, at_most: Option<String> }`,
   both `#[serde(deny_unknown_fields, rename_all = "camelCase")]`.
2. Add `impl RedisType { pub fn schema(self) -> &'static [&'static str];
   pub fn as_str(self) -> &'static str; pub fn from_name(name: &str) ->
   Option<RedisType> }` with schemas `["value"]`, `["field", "value"]`,
   `["index", "value"]`, `["member"]`, `["member", "score"]` and names
   `string`/`hash`/`list`/`set`/`zset`.
3. Add `pub(crate) fn redis_target_from_value(content: &serde_yaml::Value,
   index: usize) -> Result<RedisTarget, DocError>`: deserialize
   `RawRedisTarget` (unknown field names the action index); reject an
   empty `datasource` or `key` naming the field; map `RedisType::from_name`
   and reject an unknown type naming the action index and the type token;
   parse `ttl` through `parse_redis_ttl`.
4. Add `fn parse_redis_ttl(raw: &RawRedisTtl, index: usize) ->
   Result<CountBound, DocError>`: an empty map (neither `atLeast` nor
   `atMost`) is an error naming the action index; parse each humantime
   string with `humantime::parse_duration`, require a positive duration
   whose nanosecond value is an exact multiple of `1_000_000` (zero and
   sub-millisecond are load errors, never truncated), convert
   `duration.as_millis()` (u128) through `u64::try_from` (overflow is a
   load error, never wrapping), reject `atLeast > atMost`, then return
   `CountBound::AtLeast`, `CountBound::AtMost`, or `CountBound::Range`.
5. Add `pub(crate) fn redis_expectation_from_value(value: &Value, index:
   usize, schema: &[&str]) -> Result<RowsExpectation, DocError>`: call
   `super::validate::sql_expectation_from_value(value, index)?`, then (a)
   if `columns` is declared, every name SHALL be in `schema` (an unknown
   name is a load error naming the action index and the name); (b) the
   effective projection is the declared `columns` or the full `schema`;
   (c) every `rows` row SHALL have exactly as many cells as the effective
   projection, a mismatch naming the row index (the sql loader defers this
   check to execution; the redis schema is inherent, so redis enforces it
   at load whether or not `columns` is declared).
6. In `document/validate.rs` add the variant `Redis(RedisTarget)` to
   `ScenarioTarget` (next to `Surreal`), reusing `RowsExpectation` through
   the existing `ValidateExpectation::Rows` — no new expectation type.
7. In `document.rs`: declare `pub mod redis_target;`, re-export
   `pub use redis_target::{RedisTarget, RedisType};`, add both names to the
   crate-root re-export list in `lib.rs`, and add the exhaustive-match arm
   `ScenarioTarget::Redis(_) => Vec::new()` to `ScenarioAction::bindings`
   (line ~271) so the variant introduction does not break the build.
8. In `document.rs::build_target` add the `"redis"` arm calling
   `redis_target_from_value` and returning `ScenarioTarget::Redis`; add
   `"redis"` to the unknown-target error string. In the `validate` action
   arm add `ScenarioTarget::Redis(target) => ValidateExpectation::Rows(
   redis_expectation_from_value(&raw.expectation, index,
   target.r#type.schema())?)`. Add `ScenarioTarget::Redis(_)` to the
   `deadline` validity `matches!` list and to the error string (partner,
   sql, surreal, redis). The redis advisory arm is intentionally absent (a
   redis target carries no query text).
9. In `crates/camel-integration-test/Cargo.toml` add the optional
   dependency `redis = { workspace = true, optional = true, features =
   ["tokio-comp", "aio"] }` and the feature `redis = ["dep:redis",
   "camel-bundles/redis"]` (lint-gate-forwarding Rule 1: the `redis`
   feature shadows the bundle gate and forwards it). The grammar and
   validation stay ungated and run in every build.
10. Create `runner/redis_validate.rs` holding ONE snapshot representation
    and the pure law, all `#[cfg(feature = "redis")]` except the twin:
    - `struct Snapshot { observed: Option<RedisType>, tuples:
      Vec<Vec<camel_api::Value>>, columns: Vec<String>, ttl:
      RedisTtlStatus }` — the `columns` are the effective projection in
      projection order, computed once in `decode_snapshot` and reused by
      `decide`/`mismatch`; `ttl` is the validated status, never a raw
      `i64`.
    - `pub(crate) fn redis_observed_type(v: &redis::Value) ->
      Result<Option<RedisType>, String>`: `none` → `None`,
      `string`/`hash`/`list`/`set`/`zset` → `Some(..)`, any other observed
      kind (for example `stream`) → `Err` (apparatus).
    - `pub(crate) fn redis_value_to_cell(v: &redis::Value, column: &str,
      ordinal: usize) -> Result<camel_api::Value, String>`: `Nil` → null;
      `Int` → number; `SimpleString`/`BulkString` → a string when the bytes
      are valid UTF-8, else `Err` naming `column` and `ordinal` (never a
      lossy replacement); any other reply kind → `Err` naming `column` and
      `ordinal` (never a silent null).
    - `pub(crate) fn normalize_score(raw: &str, column: &str, ordinal:
      usize) -> Result<camel_api::Value, String>`: parse to a finite `f64`
      (parse failure or non-finite → `Err`); integral (`fract() == 0.0`)
      emits a JSON integer only inside `[-2^63, 2^63)` — inclusive lower,
      exclusive upper — checked BEFORE conversion, no saturating cast; an
      integral value at `+2^63`/`1e20` → `Err`; a non-integral value → JSON
      float.
    - `pub(crate) fn redis_rows_to_tuples(value_type: RedisType, payload:
      &redis::Value, columns: &[&str]) -> Result<Vec<Vec<camel_api::Value>>,
      String>`: `string` `GET` one row `[value]` (Nil → null cell); `hash`
      `HGETALL` paired into `[field, value]` rows ordered by field (odd
      element count → `Err` naming `value` and the row ordinal); `list`
      `LRANGE` into `[index, value]` (index a number); `set` `SMEMBERS`
      into `[member]` ordered lexicographically; `zset` `ZRANGE key 0 -1
      WITHSCORES` into `[member, score]` in rank order with the score
      through `normalize_score`. Project only `columns` in declaration
      order; an empty container yields zero rows.
    - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] enum RedisTtlStatus
      { Missing, Persistent, Remaining(usize) }` and `fn
      redis_ttl_status(raw: i64) -> Result<RedisTtlStatus, String>`: `-2`
      → `Missing`, `-1` → `Persistent`, `>= 0` → `Remaining(ms)` through a
      checked `usize::try_from` (overflow → `Err`); any other negative
      (`-3`, `i64::MIN`) → `Err`. Decoding runs on EVERY snapshot,
      whether or not a `ttl` bound is declared, so the executor never
      stores a status `decide` cannot use.
    - `fn ttl_holds(status: &RedisTtlStatus, bound: &CountBound) -> bool`
      (infallible): `Missing`/`Persistent` → `false`; `Remaining(ms)` →
      `camel_matchers::bound_holds(bound, ms)`.
    - `fn snapshot_script(value_type: RedisType) -> &'static str`: five
      `const` Lua bodies, one per declared type, each issuing `TYPE`, the
      declared type's read guarded so a mistyped key cannot raise
      `WRONGTYPE`, and `PTTL` in one read-only execution; selected in Rust
      by the declared type and invoked via `redis::cmd("EVAL")` (not
      `redis::Script`, which needs the non-default `script` feature).
11. In `runner/redis_validate.rs` add the executor (feature `redis`):
    - `const REDIS_VALIDATE_POLL_INTERVAL: Duration = Duration::from_millis(100);`
    - `fn apparatus(index: usize, name: &str, text: String) ->
      ScenarioFailure` with the `redis validation: datasource '{name}':
      {text}` shape; `fn projection_error(index: usize, name: &str, detail:
      String) -> ScenarioFailure` delegating to `apparatus` (the ONE
      mapping for every projection/value-law failure).
    - `async fn eval_raw(conn: &redis::aio::MultiplexedConnection, key:
      &str, script: &str, index: usize, name: &str, db_url: &str) ->
      Result<redis::Value, ScenarioFailure>`: one `redis::cmd("EVAL")
      .arg(script).arg(1).arg(key).query_async::<redis::Value>(&mut
      conn.clone())` executed as ONE server-side call; a driver/connection
      error is apparatus with the text sanitized against `db_url` via
      `crate::steering::sanitize_db_error`.
    - `fn decode_snapshot(index: usize, target: &RedisTarget, expected:
      &RowsExpectation, raw: redis::Value) -> Result<Snapshot,
      ScenarioFailure>` (pure, the SINGLE decode site): a reply that is not
      the expected three-element array → apparatus; `redis_observed_type`
      `Err` → apparatus; `redis_ttl_status(raw_pttl)` `Err` → apparatus
      ALWAYS (with or without a declared `ttl` bound — the raw `PTTL` is
      decoded and validated on every snapshot, so `decide` only ever sees a
      legitimate `Missing`/`Persistent`/`Remaining`); when the observed
      type equals the declared type, project `redis_rows_to_tuples` (a
      projection/value-law `Err` maps through `projection_error`) and store
      the effective `columns`; a missing key or a differing observed type
      is verdict DATA (never an `Err`) with empty `tuples`. `Snapshot {
      observed, tuples, columns, ttl: RedisTtlStatus }` — one
      representation; no raw `i64` PTTL leaks past decode.
    - `async fn poll_with_source<F, Fut>(index: usize, target:
      &RedisTarget, expected: &RowsExpectation, deadline:
      Option<Duration>, mut source: F) -> Result<(), ScenarioFailure>`
      where `F: FnMut() -> Fut`, `Fut: Future<Output = Result<redis::Value,
      ScenarioFailure>>`: the ONE poll driver — builds a snapshot closure
      that awaits `source` then `decode_snapshot`s the raw reply, and calls
      `super::poll::poll_until(deadline, REDIS_VALIDATE_POLL_INTERVAL,
      that_closure, early, decide)` with `early` = the count-bound
      `above_ceiling` breach only (redis contents are not monotone) and
      `decide`. It is private, not public API; production calls it once
      with the real `eval_raw` source, and the colocated tests call it with
      an injected source (the deterministic executor seam). A scripted
      injected source returns its sequence and repeats the last reply
      thereafter, so a deadline poll is deterministic. No global state, no
      release-visible hook.
    - `fn decide(index: usize, target: &RedisTarget, expected:
      &RowsExpectation, snapshot: &Snapshot) -> Result<(), ScenarioFailure>`:
      (a) observed `None` or observed != declared → `ValidationMismatch`
      naming the key, the declared type, and the observed type (`none` for
      missing); (b) match through `camel_matchers::rows_match` /
      `bound_holds`; (c) a declared `ttl` through the infallible
      `ttl_holds(&snapshot.ttl, bound)`, a `false` → cell-free mismatch
      naming the rendered bound and the observed status (`Remaining` ms,
      persistent, or missing). `decide` is the ONLY verdict-class producer;
      it never sees an `Err` status and never classifies a projection
      failure.
    - `fn mismatch` / `fn redis_mismatch_detail` rendering only the
      datasource name, the document-authored key, the declared and observed
      type names, the rendered bound or the expected and actual row counts,
      and the `snapshot.columns` in projection order — never a value,
      member, or field identifier. A fail-closed cell's schema column and
      row ordinal live in the APPARATUS detail (`projection_error`), never
      in the verdict detail.
    - `pub(crate) async fn redis_validate_action(index: usize, target:
      &RedisTarget, expected: &RowsExpectation, deadline:
      Option<Duration>, catalog: Option<&Arc<dyn
      camel_api::datasource::DatasourceCatalog>>) -> Result<(),
      ScenarioFailure>`: a `None` catalog is an `ActionTransport` naming
      `"redis validation: no datasource catalog is available; the
      boot-owning caller must pass the cascade's catalog"`; resolve through
      `crate::steering::resolve_datasource::<redis::aio::
      MultiplexedConnection>(catalog, &target.datasource, "redis
      validation")` and map the resolver's complete message straight into
      `ActionTransport`; then call `poll_with_source(..)` with the
      `eval_raw` source bound to the resolved connection and `db_url`.
    - The `#[cfg(not(feature = "redis"))]` twin returns
      `ValidationMismatch { action: index, detail: "redis validation
      requires the `redis` feature" }` with `let _ = (target, expected,
      deadline, catalog);`.
    - Colocate the tests in a `#[cfg(test)] mod tests` INSIDE this file
      (no separate test file, no test-only re-exports): the pure-law tests
      under `#[cfg(feature = "redis")] mod law`, the decode/decision and
      injected-source executor tests under `#[cfg(feature = "redis")] mod
      decide`, the loopback production-acquisition protocol test under
      `#[cfg(feature = "redis")] mod protocol` (tokio only, no Docker),
      and the feature-off twin under `#[cfg(all(test, not(feature =
      "redis")))] mod twin`. Colocation gives exact private access to
      `Snapshot`, `decode_snapshot`, `poll_with_source`, `eval_raw`,
      `decide`, and `projection_error`; `RowsExpectation`/`CountBound`/
      `Expectation` fields are public. No Docker in this module (the live
      module lands in Task 4 behind its own `redis-live` gate).
12. In `runner.rs`: `mod redis_validate;`, `pub(crate) use
    redis_validate::redis_validate_action;`, the dispatch arm
    `(ScenarioTarget::Redis(target), ValidateExpectation::Rows(expected))
    => redis_validate_action(index, target, expected, *deadline,
    datasource_catalog)`, and `ScenarioTarget::Redis(_) => return
    Err(unpaired_validate(index))` in the inner message-expectation match
    (line ~1153). Both exhaustive matches land in this task.
13. In `steering.rs`: widen the resolver gate from `any(feature = "sql",
    feature = "surreal")` to `any(feature = "sql", feature = "surreal",
    feature = "redis")` (both the `use std::sync::Arc;` and
    `resolve_datasource` attributes), and add the
    `#[cfg(all(test, feature = "redis"))]` resolver test module pinning the
    `redis validation` label.

**Tests.** Write first; the RED is the missing grammar/law/executor/twin.
Grammar tests run in BOTH feature configurations; every other test is
feature `redis` unless marked (twin). Commands: `cargo test -p
camel-integration-test --lib <test_fn_name>` (grammar and twin),
`cargo test -p camel-integration-test --lib --features redis
<test_fn_name>` (everything else).
- Grammar (in `document/redis_target.rs` `mod tests` and
  `doc_parse_test.rs`):
  - `redis_missing_datasource_key_or_type_is_load_error`: arrange — three
    documents, each a redis target missing one of `datasource`/`key`/`type`;
    act — `parse_scenario_document` on each; assert — `DocError::Validation
    { index: 0 }` naming the missing field. RED/GREEN.
  - `redis_unknown_type_is_load_error`: arrange — `type: stream`; act —
    `parse_scenario_document`; assert — `DocError::Validation { index: 0 }`
    naming `stream`. RED/GREEN.
  - `redis_unknown_projection_column_is_load_error`: arrange — `type: hash`,
    expectation `columns: [field, missing]`; act — parse; assert —
    `DocError::Validation` naming the action index and `missing`. RED/GREEN.
  - `redis_row_length_mismatch_without_columns_is_load_error`: arrange —
    `type: list`, no `columns`, a three-cell row; act — parse; assert —
    `DocError::Validation` naming the row index (the list schema is two
    cells). RED/GREEN.
  - `redis_row_length_mismatch_with_columns_is_load_error`: arrange — `type:
    zset`, `columns: [member]`, a two-cell row; act — parse; assert —
    `DocError::Validation` naming the row index. RED/GREEN.
  - `redis_empty_ttl_bound_is_load_error`: arrange — `ttl: {}`; act —
    parse; assert — `DocError::Validation` naming the action index.
    RED/GREEN.
  - `redis_inverted_ttl_bound_is_load_error`: arrange — `ttl: {atLeast:
    60s, atMost: 30s}`; act — parse; assert — `DocError::Validation` naming
    the action index. RED/GREEN.
  - `redis_sub_millisecond_ttl_bound_is_load_error`: arrange — `ttl:
    {atLeast: 500us}`; act — parse; assert — `DocError::Validation` naming
    the action index (never truncated). RED/GREEN.
  - `redis_zero_ttl_bound_is_load_error`: arrange — `ttl: {atMost: 0s}`;
    act — parse; assert — `DocError::Validation` naming the action index.
    RED/GREEN.
  - `redis_overflowing_ttl_bound_is_load_error`: arrange — `ttl: {atLeast:
    60000000000000000s}` (millis exceed `u64::MAX`); act — parse; assert —
    `DocError::Validation` naming the action index; the checked conversion
    never wraps. RED/GREEN.
  - `redis_ttl_bounds_parse_to_count_bound`: arrange — three documents
    `{atLeast: 30s}`, `{atMost: 60s}`, `{atLeast: 1ms, atMost: 60s}`; act —
    parse each; assert — the target's `ttl` equals `AtLeast(30_000)`,
    `AtMost(60_000)`, `Range(1, 60_000)` respectively. RED/GREEN.
  - `redis_deadline_accepted`: arrange — a redis target with `deadline:
    2s`; act — parse; assert — `Ok`. RED/GREEN.
  - `redis_target_parses` (in `doc_parse_test.rs`): arrange — a document
    whose `target` is `{redis: {datasource: statedb, key: "user:1", type:
    hash}}`; act — `parse_scenario_document`; assert — the action is
    `ScenarioTarget::Redis(RedisTarget { datasource: "statedb", key:
    "user:1", r#type: RedisType::Hash, ttl: None })`. `cmd: cargo test -p
    camel-integration-test --lib redis_target_parses`. RED/GREEN.
  - `redis_target_requires_the_reused_rows_grammar`: arrange — a redis
    target whose expectation declares neither `rows` nor a count bound; act
    — parse; assert — the reused `sql_expectation_from_value`
    `DocError::Validation` naming the action index. RED/GREEN.
  - `deadline_on_last_received_still_load_error` (regression): arrange — a
    `lastReceived` target with `deadline`; act — parse; assert —
    `DocError::Validation`. GREEN before and after.
- Value law / projection (in `mod tests::law`, feature `redis`). Every
  bullet states the exact RESP fixture (`redis::Value`), the call, and the
  exact assertion:
  - `string_projection_reads_one_value_row`: arrange — `payload =
    Value::BulkString(b"alice".to_vec())`, `RedisType::String`, `columns =
    &["value"]`; act — `redis_rows_to_tuples(String, &payload, columns)`;
    assert — `== vec![vec![Value::String("alice")]]`.
  - `string_nil_reply_is_null_cell`: arrange — `payload = Value::Nil`,
    `RedisType::String`, `columns = &["value"]`; act — the same call;
    assert — `== vec![vec![Value::Null]]`.
  - `hash_projection_orders_fields_by_field`: arrange — `payload =
    Value::Array([BulkString("b"), BulkString("2"), BulkString("a"),
    BulkString("1")])`, `RedisType::Hash`, `columns = &["field","value"]`;
    act — the same call; assert — `== [[String("a"),String("1")],
    [String("b"),String("2")]]` (field order, not reply order).
  - `hash_wrong_arity_reply_fails_closed_by_column_and_ordinal`: arrange —
    `payload = Value::Array([BulkString("a")])` (odd count),
    `RedisType::Hash`, `columns = &["field","value"]`; act — the same call;
    assert — `Err` whose text contains `value` and the row ordinal `0`, and
    no field identifier.
  - `list_projection_reads_index_value_rows`: arrange — `payload =
    Value::Array([BulkString("a"), BulkString("b"), BulkString("c")])`,
    `RedisType::List`, `columns = &["index","value"]`; act — the same call;
    assert — `== [[Number(0),"a"],[Number(1),"b"],[Number(2),"c"]]`.
  - `set_projection_orders_members_lexicographically`: arrange — `payload =
    Value::Array([BulkString("c"), BulkString("a"), BulkString("b")])`,
    `RedisType::Set`, `columns = &["member"]`; act — the same call; assert —
    `== [["a"],["b"],["c"]]`.
  - `zset_projection_reads_member_score_rows_in_rank_order`: arrange —
    `payload = Value::Array([BulkString("bob"), BulkString("1.5"),
    BulkString("alice"), BulkString("2")])`, `RedisType::Zset`, `columns =
    &["member","score"]`; act — the same call; assert — `== [["bob", 1.5],
    ["alice", Number(2)]]` (ascending rank: bob 1.5 before alice 2).
  - `columns_select_subset_projection`: arrange — the paired hash
    `payload = Array([BulkString("name"),BulkString("alice"),
    BulkString("age"),BulkString("42")])`, `columns = &["value"]`; act —
    `redis_rows_to_tuples(Hash, &payload, columns)`; assert — `==
    [["42"],["alice"]]` (field-sorted: age before name, only the value
    column).
  - `columns_reorder_projection`: arrange — the same `payload`, `columns =
    &["value","field"]`; act — the same call; assert — `==
    [["42","age"],["alice","name"]]` (field-sorted, declaration order).
  - `empty_container_yields_zero_rows`: arrange — `payload =
    Value::Array(vec![])` (an empty CONTAINER; a `String` payload is a
    scalar, so the container case uses a set), `RedisType::Set`, `columns =
    &["member"]`; act — `redis_rows_to_tuples(Set, &payload, columns)`;
    assert — `== Vec::<Vec<Value>>::new()`.
  - `unsupported_reply_kind_fails_closed_by_column_and_ordinal`: arrange —
    `Value::Okay` in the scalar slot; act —
    `redis_value_to_cell(&Value::Okay, "value", 0)`; assert — `Err` whose
    text contains the column `value` and ordinal `0`, never a null.
  - `non_utf8_cell_fails_closed_by_column_and_ordinal`: arrange —
    `Value::BulkString(vec![0xff, 0xfe])`; act — `redis_value_to_cell(..,
    "value", 0)`; assert — `Err` whose text contains the column and
    ordinal, never a lossy replacement.
  - `integer_reply_maps_to_number`: arrange — `Value::Int(42)`; act —
    `redis_value_to_cell(&Value::Int(42), "value", 0)`; assert — `==
    Value::from(42i64)`.
  - `valid_utf8_bulk_string_maps_to_string`: arrange —
    `Value::BulkString(b"hello".to_vec())`; act — `redis_value_to_cell(..,
    "value", 0)`; assert — `== Value::String("hello".into())`.
  - `integral_score_normalizes_to_integer`: arrange — call the function
    twice, `normalize_score("2", "score", 0)` and
    `normalize_score("2.0", "score", 0)`; act — each call; assert — each
    result is `Ok(Value::Number(n))` with `n.is_i64()` true and
    `Value::Number(n) == json!(2)`.
  - `non_integral_score_matches_float`: arrange —
    `normalize_score("1.5", "score", 0)`; act — the call; assert —
    `Ok(Value::Number(n))` with `n.is_f64()` true and equal to `json!(1.5)`.
  - `i64_lower_bound_score_is_accepted_as_integer`: arrange —
    `normalize_score("-9223372036854775808", "score", 0)`; act — the call;
    assert — `Ok(Value::Number(n))` with `n.is_i64()` true and equal to
    `json!(-9223372036854775808i64)`.
  - `plus_2_pow_63_score_fails_closed_apparatus`: arrange —
    `normalize_score("9223372036854775808", "score", 0)`; act — the call;
    assert — `Err` (never a saturated integer).
  - `large_integral_score_fails_closed_apparatus`: arrange —
    `normalize_score("1e20", "score", 0)`; act — the call; assert — `Err`.
  - `non_finite_score_fails_closed`: arrange — `normalize_score("inf",
    "score", 0)` and `normalize_score("nan", "score", 0)`; act — each call;
    assert — both are `Err` naming `score` and ordinal `0`.
  - `redis_observed_type_maps_none_and_known_types`: arrange —
    `Value::SimpleString("none")` and each of `"string"`, `"hash"`,
    `"list"`, `"set"`, `"zset"`, `"stream"`; act — `redis_observed_type`
    each; assert — `Ok(None)` for `none`, `Ok(Some(..))` for the five known
    names, `Err` for `stream`.
  - `redis_ttl_status_decodes_missing_persistent_and_remaining`: arrange —
    the raw values `-2`, `-1`, `0`, `60_000`; act — `redis_ttl_status`
    each; assert — `Ok(Missing)`, `Ok(Persistent)`, `Ok(Remaining(0))`,
    `Ok(Remaining(60_000))`.
  - `redis_ttl_status_unknown_negative_fails_closed`: arrange — `-3` and
    `i64::MIN`; act — `redis_ttl_status` each; assert — both are `Err`.
  - `ttl_holds_missing_and_persistent_fail`: arrange — statuses `Missing`
    and `Persistent`, bound `CountBound::AtLeast(1)`; act — `ttl_holds`
    each; assert — both `false`.
  - `ttl_holds_at_most_fails_above_bound`: arrange — `Remaining(60_000)`,
    `CountBound::AtMost(30_000)`; act — `ttl_holds`; assert — `false`.
  - `ttl_holds_zero_remaining_satisfies_at_most_not_at_least_or_range`:
    arrange — `Remaining(0)` against `AtMost(1)`, `AtLeast(1)`,
    `Range(1, 60_000)`; act — `ttl_holds` each; assert — `true`, `false`,
    `false`.
  - `snapshot_script_selects_declared_read`: arrange — each `RedisType`;
    act — `snapshot_script(t)`; assert — the returned body contains `TYPE`,
    the declared read (`GET`/`HGETALL`/
    `LRANGE`/`SMEMBERS`/`ZRANGE`), and `PTTL`.
- Executor / decision (in `mod tests::decide`, feature `redis`):
  - `projection_error_maps_to_action_transport`: arrange — call
    `projection_error(0, "statedb", "column `value` row 0: unsupported
    reply kind")`; act — the call; assert — the result is
    `ScenarioFailure::ActionTransport { action: 0, source:
    TransportError::Other { message } }` with `message == "redis
    validation: datasource 'statedb': column `value` row 0: unsupported
    reply kind"` (the typed classification the value law requires).
  - `decode_snapshot_unknown_negative_pttl_without_ttl_bound_is_apparatus`:
    arrange — `target = RedisTarget { datasource: "statedb", key: "k",
    r#type: String, ttl: None }`, `raw = Value::Array([SimpleString(
    "string"), BulkString(b"x"), Int(-3)])`; act —
    `decode_snapshot(0, &target, &expected, raw)`; assert —
    `Err(ActionTransport)` naming `statedb` and the PTTL detail; the raw is
    validated regardless of a bound.
  - `decode_snapshot_unknown_negative_pttl_with_ttl_bound_is_apparatus`:
    arrange — the same `raw`, `target.ttl = Some(CountBound::AtLeast(1000))`;
    act — `decode_snapshot`; assert — the same `Err(ActionTransport)`.
  - `decode_snapshot_missing_key_is_verdict_data`: arrange — `raw =
    Value::Array([SimpleString("none"), Array([]), Int(-2)])`, `target =
    RedisTarget { datasource: "statedb", key: "k", r#type: String, ttl:
    None }`; act — `decode_snapshot(0, &target, &expected, raw)`; assert —
    `Ok(Snapshot)` with `observed: None` and `ttl: Missing`, never an
    `Err`.
  - `poll_with_source_unknown_negative_pttl_stops_immediately_without_bound`
    and `poll_with_source_unknown_negative_pttl_stops_immediately_with_bound`:
    arrange an injected source whose first raw is
    `Array[SimpleString("string"), BulkString(b"x"), Int(-3)]` (the
    no-bound case target has `ttl: None`, the with-bound case
    `ttl: Some(AtLeast(1))`), deadline `2s`; act `poll_with_source`; assert
    `ActionTransport` and `elapsed < 1s` AND the source was called exactly
    once (the apparatus path stops the poll on the first snapshot, it does
    not wait the window).
  - `poll_with_source_first_missing_then_present_passes`: arrange an
    injected source whose call 1 is
    `Array[SimpleString("none"), Array([]), Int(-2)]` and every later call
    is `Array[SimpleString("string"), BulkString(b"v"), Int(-1)]`, target
    `string` with `rows: [["v"]]`, deadline `2s`; act; assert `Ok(())`, the
    source was called at least twice, and call 1 was the missing reply (the
    missing first snapshot is verdict data, not a snapshot error).
  - `poll_with_source_first_present_then_deleted_final_snapshot_fails`:
    arrange call 1 present (`string`/`v`), later calls missing; target
    `string` with `rows: [["v"]]`, deadline `2s`; act; assert
    `ValidationMismatch` on the final snapshot and at least two
    acquisitions (no early settle).
  - `poll_with_source_transient_wrong_type_then_correct_passes`: arrange
    call 1 `Array[SimpleString("string"), BulkString(b"v"), Int(-1)]` for a
    `hash` target, later calls `Array[SimpleString("hash"),
    Array([BulkString("name"), BulkString("alice")]), Int(-1)]` with
    `rows: [["alice"]]` (columns absent → schema `[field, value]`, so use
    `columns: [value]` and rows `[["alice"]]`); deadline `2s`; act; assert
    `Ok(())` and at least two acquisitions.
  - `poll_with_source_ttl_passes_early_fails_at_deadline`: arrange call 1
    `Array[SimpleString("string"), BulkString(b"v"), Int(60_000)]`, later
    calls the same payload with `Int(0)`, target `string` with
    `ttl: Some(AtLeast(30_000))` and no rows (count bound), deadline `2s`;
    act; assert `ValidationMismatch` on the final snapshot, at least two
    acquisitions, and the detail names `at least 30000` and the observed
    `0` ms (TTL never settles early).
  - `poll_with_source_ceiling_breach_stops_immediately`: arrange call 1
    `Array[SimpleString("set"), Array([BulkString("a"), BulkString("b")]),
    Int(-1)]`, target `set` with `bound: Some(AtMost(1))`, deadline `2s`;
    act; assert `ValidationMismatch` with `elapsed < 1s` and exactly one
    acquisition (the early judgment fires on the first snapshot).
  - `redis_decide_missing_key_is_mismatch`: arrange `target =
    RedisTarget { datasource: "statedb", key: "rc-decide", r#type: String,
    ttl: None }`; `expected = RowsExpectation { columns:
    Some(vec!["value"]), unordered: false, rows:
    Some(vec![vec![Expectation::Equals(json!("v"))]]), bound: None }`;
    `snapshot = Snapshot { observed: None, tuples: vec![], columns:
    vec!["value"], ttl: RedisTtlStatus::Missing }`. act —
    `decide(0, &target, &expected, &snapshot)`. assert —
    `Err(ValidationMismatch { action: 0, .. })` whose detail contains
    `rc-decide`, declared `string`, and observed `none`.
  - `redis_decide_wrong_type_is_mismatch`: arrange `target.r#type = Hash`,
    `expected.columns = Some(["field","value"])`, `expected.rows =
    Some([[Equals("name"), Equals("alice")]])`; `snapshot.observed =
    Some(String)`, `tuples = vec![]`, `columns =
    vec!["field","value"]`, `ttl = Remaining(60_000)`. act — `decide`.
    assert — `Err(ValidationMismatch)` whose detail contains declared
    `hash` and observed `string`, and does NOT contain `alice` (the value
    is never projected on a type mismatch).
  - `redis_decide_type_agreement_rows_pass`: arrange `target.r#type =
    String`, `expected.rows = Some([[Equals("alice")]])`; `snapshot =
    Snapshot { observed: Some(String), tuples:
    vec![vec![Value::String("alice")]], columns: vec!["value"], ttl:
    Persistent }`. act/assert — `decide(0, ..) == Ok(())`.
  - `redis_decide_count_bound_passes_on_projected_row_count`: arrange
    `target.r#type = Set`, `expected = RowsExpectation { columns: None,
    unordered: false, rows: None, bound: Some(CountBound::AtLeast(2)) }`;
    `snapshot.observed = Some(Set)`, `tuples =
    vec![vec!["a"],vec!["b"],vec!["c"]]`, `columns = vec!["member"]`,
    `ttl = Persistent`. act/assert — `decide == Ok(())`.
  - `redis_decide_ceiling_breach_is_mismatch`: arrange `target.r#type =
    Set`, `expected.bound = Some(CountBound::AtMost(1))`; `snapshot =
    { observed: Some(Set), tuples: vec![["a"],["b"],["c"]], columns:
    vec!["member"], ttl: Persistent }`. act/assert — `decide` returns
    `Err(ValidationMismatch)` whose detail contains `at most 1` and
    `actual 3 rows`.
  - `redis_decide_persistent_key_fails_ttl_bound`: arrange
    `target.r#type = String`, `target.ttl = Some(CountBound::AtLeast(1000))`;
    `expected = { columns: None, unordered: false, rows: None, bound:
    Some(CountBound::AtLeast(0)) }` (rows pass so TTL is isolated);
    `snapshot = { observed: Some(String), tuples: vec![["v"]], columns:
    vec!["value"], ttl: Persistent }`. act/assert — `decide` returns
    `Err(ValidationMismatch)` whose detail contains `at least 1000` and
    `persistent`.
  - `redis_decide_zero_remaining_ttl_bounds`: arrange three cases, all
    `target.r#type = String`, `snapshot = { observed: Some(String),
    tuples: vec![["v"]], columns: vec!["value"], ttl: Remaining(0) }`,
    `expected.bound = Some(CountBound::AtLeast(0))` (rows pass). act/assert
    — with `target.ttl = Some(AtMost(1))` → `Ok(())`; with
    `target.ttl = Some(AtLeast(1))` → `Err(ValidationMismatch)`; with
    `target.ttl = Some(Range(1, 60_000))` → `Err(ValidationMismatch)`.
  - `redis_mismatch_detail_elides_values_and_db_url`: arrange
    `target.r#type = String`, `key = "rc-detail"`;
    `expected.rows = Some([[Equals("expected-secret")]])`; `snapshot =
    { observed: Some(String), tuples:
    vec![vec![Value::String("actual-secret")]], columns: vec!["value"],
    ttl: Persistent }`. act — `decide`. assert — the detail contains
    `rc-detail`, `expected 1 rows`, `actual 1 rows`, and the column name
    `value`, and contains NEITHER `actual-secret` NOR `expected-secret` NOR
    any `db_url` sentinel (the detail function never receives the URL).
  - `redis_secret_hash_fields_and_set_members_never_reach_diagnostics`:
    arrange two DELIBERATELY MISMATCHING cases (the expected cells differ
    from the actual sentinel cells, so `decide` must return a
    `ValidationMismatch`, never `Ok`). (1) `target.r#type = Hash`, `key =
    "rc-secret"`, `expected.columns = Some(["field","value"])`,
    `expected.rows = Some([[Equals("EXPECTED_FIELD"),
    Equals("EXPECTED_VALUE")]])` (non-secret expected cells); `snapshot =
    { observed: Some(Hash), tuples:
    vec![vec![Value::String("SENTINEL_FIELD"),
    Value::String("SENTINEL_VALUE")]], columns: vec!["field","value"], ttl:
    Persistent }`. (2) `target.r#type = Set`, `expected.columns =
    Some(["member"])`, `expected.rows =
    Some([[Equals("EXPECTED_MEMBER")]])`; `snapshot = { observed:
    Some(Set), tuples: vec![vec![Value::String("SENTINEL_MEMBER")]],
    columns: vec!["member"], ttl: Persistent }`. act — `decide` for each
    case. assert — each returns `Err(ValidationMismatch)` whose detail
    contains the datasource, the key `rc-secret`, the declared and observed
    type names, the expected and actual row counts, and the schema column
    names in projection order; each detail contains NEITHER `SENTINEL_FIELD`
    NOR `SENTINEL_VALUE` NOR `SENTINEL_MEMBER` (and no `EXPECTED_*` cell
    value) — no field or member identifier ever reaches the diagnostic.
  - `redis_ttl_mismatch_detail_includes_observed_status`: arrange
    `target.r#type = String`, `target.ttl = Some(CountBound::AtLeast(6000))`
    (chosen so `Remaining(5_000)` FAILS), `expected.bound =
    Some(CountBound::AtLeast(0))` (rows pass); snapshot A `ttl = Persistent`,
    snapshot B `ttl = Remaining(5_000)`. act — `decide` on each. assert —
    both return `Err(ValidationMismatch)`; A's detail contains `persistent`;
    B's detail contains the rendered bound `at least 6000` and the observed
    `5000` (remaining milliseconds).
  - `redis_validate_no_catalog_fails_closed`: arrange a `ScenarioDocument`
    with one `Validate { target: ScenarioTarget::Redis(target),
    expectation: ValidateExpectation::Rows(expected), deadline: None,
    elapsed_at_least: None }`, `PartnerRouter::new(BTreeMap::new())`,
    `ScenarioVars::new()`. act — `run_scenario(&doc, &router, &mut vars)`.
    assert — `Err(ScenarioFailure::ActionTransport { action: 0, source:
    TransportError::Other { message } })` with `message` containing
    `no datasource catalog`.
  - `redis_unknown_datasource_names_label`: arrange a
    `RuntimeDatasourceCatalog` holding config `other` only, and a `redis`
    target naming `missing`. act — `redis_validate_action(0, &target,
    &expected, None, Some(&catalog))`. assert —
    `Err(ActionTransport)` whose `Other { message }` equals exactly
    `redis validation: unknown datasource 'missing'`.
  - `redis_resolution_pool_failure_redacts_url`: arrange a test-local
    `StubRedisFactory` implementing `PoolFactory` (name `redis`) whose
    `create` returns `Err(CamelError::ProcessorError("cannot open
    <SENTINEL_URL>"))`, registered into a catalog with a `statedb` config
    whose `db_url = <SENTINEL_URL>`; target names `statedb`. act —
    `redis_validate_action(0, &target, &expected, None, Some(&catalog))`.
    assert — `Err(ActionTransport)` whose message contains
    `redis validation`, `statedb`, and `[REDACTED]`, and NOT the sentinel.
  - `redis_downcast_failure_keeps_driver_detail`: arrange a test-local
    `StubWrongTypeFactory` (name `redis`) whose `create` returns
    `Ok(Arc::new(()) as Arc<dyn Any + Send + Sync>)`, registered with a
    `statedb` config; target names `statedb`. act —
    `redis_validate_action(0, &target, &expected, None, Some(&catalog))`.
    assert — `Err(ActionTransport)` whose message contains
    `redis validation`, `statedb`, and `failed to downcast handle`, and no
    `db_url`.
  - `resolver_redis_validation_label_pinned` (in `steering.rs`, feature
    `redis`): arrange a catalog lacking `missing`. act —
    `resolve_datasource::<redis::aio::MultiplexedConnection>(&catalog,
    "missing", "redis validation")`. assert — `Err` equals exactly
    `redis validation: unknown datasource 'missing'`.
- Production acquisition protocol (in `mod tests::protocol`, feature
  `redis`, `#[tokio::test]`; loopback fake RESP server, tokio `full`
  already present, NO new dependency, NO Docker). This asserts the REAL
  `eval_raw` performs exactly ONE network command — a command-construction
  unit test alone is not sufficient evidence:
  - `eval_raw_sends_exactly_one_eval_command`: arrange — bind a
    `tokio::net::TcpListener` on `127.0.0.1:0` and spawn a server task that
    (a) parses RESP arrays (`*<n>\r\n$<len>\r\n<bytes>\r\n…`) and (b)
    answers every `CLIENT SETINFO …` handshake command with `+OK\r\n`
    (the redis-rs RESP2/no-password/db-0 setup pipeline: two SETINFO
    commands, no HELLO/AUTH/SELECT), then forwards every POST-handshake
    command's arg vector on an `mpsc` and replies to the first with the
    canned snapshot `*3\r\n+string\r\n$1\r\nv\r\n:-1\r\n`; build
    `redis::Client::open(format!("redis://{addr}"))` +
    `get_multiplexed_async_connection()`. Bound listener bind to one second.
    Bound the complete connection setup and assertion sequence to ten
    seconds, including the driver handshake and the first recorded-command
    receive. Capture assertion panics with the existing `futures::FutureExt`
    `catch_unwind` so every failure reaches cleanup. act — call the REAL
    `eval_raw(&conn, "rc-proto", snapshot_script(RedisType::String), 0,
    "statedb", "redis://127.0.0.1:1")` and await it under
    `tokio::time::timeout(Duration::from_secs(5), ..)`. assert — the raw is
    `Array([SimpleString("string"), BulkString(b"v"), Int(-1)])`; the
    first recorded post-handshake command is EXACTLY
    `["EVAL", snapshot_script(RedisType::String), "1", "rc-proto"]`
    (fixed script, `numkeys` 1, the declared key) and the script contains
    `TYPE`, the declared-type read `GET`, and `PTTL`; NO second command
    arrives within a bounded `tokio::time::timeout(Duration::from_millis(
    200), rx.recv())`; and no recorded command has a first arg in
    `{TYPE, GET, HGETALL, LRANGE, SMEMBERS, ZRANGE, PTTL, SELECT, AUTH,
    HELLO}` (any separate command would be a defect). cleanup — after the
    bounded assertion future ends or is cancelled, `abort()` the server
    and await it under `tokio::time::timeout(1s, ..)` on success, timeout,
    and captured panic. Drop the connection before server cleanup. Report
    the original failure only after cleanup. These three phases impose a
    twelve-second test ceiling. The test runs in the redis-only build
    (`cargo test -p camel-integration-test --lib --features redis
    eval_raw_sends_exactly_one_eval_command`) with no server and no
    Docker.
- Feature-off twin (in `mod tests::twin`):
  - `redis_validate_feature_off_names_gate`: arrange — a well-formed
    `RedisTarget`/`RowsExpectation` with `catalog: None`; act —
    `redis_validate_action(0, &target, &expected, None, None)`; assert —
    exactly `ValidationMismatch { action: 0, detail: "redis validation
    requires the `redis` feature" }`.

**Acceptance:**
- `cargo test -p camel-integration-test --lib` passes (feature off) and
  `--lib --features redis` passes; the latter starts no container (the live
  module lands in Task 4 behind the `redis-live` gate). No dead code (every
  law function has the executor and the colocated tests as consumers).
- `cargo check -p camel-integration-test --no-default-features --features
  redis` exits 0 (compiles without `http`/`sql`).
- `cargo clippy -p camel-integration-test -- -D warnings` and
  `cargo clippy -p camel-integration-test --features redis -- -D warnings`
  exit 0; `cargo fmt --check --all` exits 0.

- [x] 1

## camel-component-redis — pool factory and bundle catalog hook

### Task 2: `RedisPoolFactory` (driver URL grammar, TLS/scheme fail-closed, canonical redaction, no-op close) and `RedisBundle::with_catalog`

**Files:**
- `crates/components/camel-redis/src/pool_factory.rs` (new)
- `crates/components/camel-redis/src/lib.rs` (modified)
- `crates/components/camel-redis/src/bundle.rs` (modified)
- `crates/components/camel-redis/Cargo.toml` (modified — only if a test needs a new dev-dep)

**Steps:**
1. Create `pool_factory.rs` with `pub struct RedisPoolFactory;` and `impl
   camel_api::datasource::PoolFactory`: `name()` → `"redis"`;
   `supported_schemes()` → `&["redis", "rediss"]`; `create()` rejects any
   `db_url` whose scheme is not exactly `redis`/`rediss` (including
   sentinel `redis+sentinel://` and cluster forms) as
   `CamelError::Config`, then parses with the driver's OWN grammar
   `redis::Client::open(db_url)` (NOT `RedisEndpointConfig::from_uri`,
   which reads `db=` and rejects `/1` and discards an ACL username) and
   opens a `redis::aio::MultiplexedConnection` through
   `get_multiplexed_async_connection()`; the handle is `Arc::new(
   connection) as Arc<dyn Any + Send + Sync>`. `check()` downcasts to
   `redis::aio::MultiplexedConnection` and issues `PING` (`Healthy` on Ok,
   `Unhealthy` otherwise). `close()` is left as the trait default no-op
   (drop-scoped lifecycle, not close-scoped).
2. Redaction boundary: factory error text SHALL NEVER contain the raw
   `db_url`. When a URL hint is useful, render it through the canonical
   `camel_api::redact::redact_url` (ADR-0051) — do NOT define a local
   `redact_db_url`. The executor's resolver additionally runs
   `sanitize_db_error` against the resolved `db_url`, so no raw URL can
   reach a failure.
3. Expose `pub(crate) fn open_client(db_url: &str) ->
   Result<redis::Client, CamelError>` used by `create` and by the offline
   URL tests (driver-parser semantics without a server).
4. In `lib.rs` add `mod pool_factory;` and `pub use
   pool_factory::RedisPoolFactory;`.
5. In `bundle.rs` add a `catalog: Option<Arc<dyn
   camel_api::datasource::DatasourceCatalog>>` field to `RedisBundle`
   (initialize `None` in `from_toml`) and `pub fn with_catalog(mut self,
   catalog: Arc<dyn DatasourceCatalog>) -> Self` registering
   `Arc::new(crate::pool_factory::RedisPoolFactory)` under the `"redis"`
   kind exactly as `SurrealDbBundle::with_catalog` does (a registration
   failure logs at warn; never fails the boot). `register_all` keeps
   registering the three existing components unchanged.

**Tests** (in `pool_factory.rs` and `bundle.rs` `mod tests`, offline; RED
is the missing factory). Command: `cargo test -p camel-component-redis
--lib <test_fn_name>`, with `--features tls` for the `rediss_with_tls`
test.
- `factory_name_is_redis`: arrange `let factory = RedisPoolFactory;`;
  act `factory.name()`; assert `== "redis"`.
- `factory_supports_redis_and_rediss`: arrange `let factory =
  RedisPoolFactory;`; act `factory.supported_schemes()`; assert `==
  ["redis", "rediss"]`.
- `factory_matches_redis_urls_and_rejects_others`: arrange — configs
  `redis://localhost:6379`, `rediss://localhost:6379`,
  `postgresql://localhost:5432/db`; act — `factory.matches(&config)` each;
  assert — true, true, false.
- `unsupported_scheme_is_rejected_redacted`: arrange — config
  `db_url = "redis+sentinel://host:26379"` (and a `redis+cluster://` form);
  act — `factory.create(&config)`; assert — `Err` whose text contains no
  raw URL.
- `malformed_url_is_rejected_redacted`: arrange — `db_url =
  "redis://[::bad"`; act — `create`; assert — `Err` with no raw URL.
- `malformed_url_credential_is_redacted`: arrange — `db_url =
  "redis://user:s3cr3t-sentinel@[::bad"`; act — `create`; assert — `Err`
  whose text contains neither `s3cr3t-sentinel` nor the raw `db_url` (the
  credential negative test for the canonical redaction boundary).
- `rediss_without_tls_is_rejected` (build WITHOUT the component `tls`
  feature): arrange — `db_url = "rediss://host:6379"`; act — `create`;
  assert — `Err` with no raw URL and no connection constructed (the driver
  rejects `rediss` when TLS support is not compiled).
- `rediss_with_tls_opens_client` (`#[cfg(feature = "tls")]`): arrange —
  call `open_client("rediss://host:6379")`; act — the call; assert — `Ok`
  (parse only, no network).
- `open_client_honors_acl_username_and_database`: arrange —
  `open_client("redis://alice:secret@host:6379/1")`; act — the call, then
  `client.get_connection_info().redis_settings()`; assert — `Ok` and
  `username() == Some("alice")`, `password() == Some("secret")`,
  `db() == 1`; additionally `open_client("redis://host:6379/1")` returns
  `Ok` (the component endpoint parser would reject `/1`).
- `close_is_default_noop`: arrange — `handle =
  DatasourceHandle::new("statedb", "redis", Arc::new(()))`; act —
  `RedisPoolFactory.close(&handle)`; assert — resolves `Ok(())` without
  touching the handle.
- `bundle_with_catalog_registers_redis_factory` (in `bundle.rs`): arrange
  — `catalog` and `RedisBundle::from_toml(toml::Value::Table(
  toml::map::Map::new())).expect("empty config").with_catalog(
  catalog.clone())`; act — `catalog.register_factory("redis",
  Arc::new(RedisPoolFactory))`; assert — returns the already-registered
  error (the slot is filled).
- Existing `redis_bundle_registers_expected_schemes` and
  `redis_bundle_from_toml_*` stay green (regression).

**Acceptance:**
- `cargo test -p camel-component-redis --lib` and `--lib --features tls`
  pass.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings`
  exits 0.

- [x] 2

## camel-bundles and camel-cli — boot wiring, feature gates, profiles, all-targets

### Task 3: register the redis catalog hook at boot, forward the CLI gate, update `feature_profiles`, prove the all-targets build

**Files:**
- `crates/camel-bundles/src/lib.rs` (modified)
- `crates/camel-cli/Cargo.toml` (modified)
- `crates/camel-cli/src/commands/test/scenario.rs` (modified)
- `crates/camel-cli/tests/test_scenario_cli_e2e.rs` (modified)
- `crates/camel-cli/tests/feature_profiles.rs` (modified)

**Steps:**
1. In `crates/camel-bundles/src/lib.rs` replace the handle-free redis
   registration under `#[cfg(feature = "redis")]` with the datasource
   shape used by sql/surrealdb: `bundle_from_config::<
   camel_component_redis::RedisBundle>(config)` then
   `.with_catalog(Arc::clone(&datasource_catalog))` then `register_all`,
   logging at `error!` on a config failure. The handle-free lint catalog in
   `camel-cli/src/lib.rs` keeps `register_bundle_empty!(ctx,
   camel_component_redis::RedisBundle)` unchanged.
2. In `crates/camel-cli/Cargo.toml` add `integration-redis =
   ["camel-integration-test/redis"]` (mirroring `integration-sql`), add
   `"integration-redis"` to the `full` and `flavor-full` bodies (NOT to
   `flavor-regular`/`flavor-slim`, so the default suite keeps the executor
   uncompiled), and add to `[dev-dependencies]`:
   `testcontainers.workspace = true`,
   `testcontainers-modules.workspace = true`, and
   `redis = { workspace = true, features = ["tokio-comp", "aio"] }` — the
   Redis CLI e2e seeds keys out-of-band through an async `redis::Client`
   (`get_multiplexed_async_connection`), which needs the async features.
3. In `crates/camel-cli/src/commands/test/scenario.rs` extend the
   full-boot selection for redis, mirroring the surreal arms exactly:
   - Add the redis-only `BOOT_SCHEMES` arm
     `#[cfg(all(not(feature = "integration-http"), not(feature =
     "integration-sql"), not(feature = "integration-surreal"), feature =
     "integration-redis"))] const BOOT_SCHEMES: [&str; 2] = [FAKE_SCHEME,
     "direct"];` (redis references a named datasource, never a wire
     endpoint, so no `http` scheme). The http/sql/surreal arms already
     cover every combination that includes one of them; the redis-only arm
     is the missing one.
   - Replace the 8-arm `PROVIDED_ADAPTERS` const with a
     `fn provided_adapters() -> String` that composes cfg'd fragments
     (`fake` always; `http`/`sql`/`surreal`/`redis` appended under their
     features) and update the single `format!` site (line ~459) to call it.
     This keeps the message accurate for every 4-feature combination
     without 16 near-duplicate arms.
   - Widen EVERY full-boot gate predicate to include
     `feature = "integration-redis"`: (a) the `root` parameter's
     `#[cfg_attr(not(any(feature = "integration-http", feature =
     "integration-sql", feature = "integration-surreal")),
     allow(unused_variables))]` (lines ~337-344); (b) the `#[cfg(any(feature
     = "integration-http", feature = "integration-sql", feature =
     "integration-surreal"))]` block that computes
     `has_sql`/`has_surreal`/`has_redis` and selects
     `run_scenario_full_boot` (lines ~358-362); and (c) the FUNCTION
     `run_scenario_full_boot`'s own `#[cfg(any(feature =
     "integration-http", feature = "integration-sql", feature =
     "integration-surreal"))]` attribute (lines ~533-537). Missing (c)
     leaves the redis-only build with an undefined function at the
     selection call site.
   - Add the target-side term exactly as `has_surreal`:
     `#[cfg(feature = "integration-redis")] let has_redis =
     doc.scenario.iter().any(|action| matches!(action,
     camel_integration_test::ScenarioAction::Validate { target:
     camel_integration_test::ScenarioTarget::Redis(_), .. }));` plus the
     `#[cfg(not(feature = "integration-redis"))] let has_redis = false;`
     twin, and add `|| has_redis` to the full-boot condition. There is no
     `redis:` prepare action, so the validate target is the only redis
     action form. The target grammar is UNGATED, so without the feature
     gate a redis-target document in a featureless build would route to
     the full boot and fail generically instead of the named action-time
     demand-gate error — the feature-off behavior is preserved.
4. In `crates/camel-cli/tests/test_scenario_cli_e2e.rs` add the
   `#[cfg(feature = "integration-redis")]` Redis CLI e2e cases (step 5),
   mirroring the surreal fixtures: a shared testcontainers Redis container
   (`OnceCell<ContainerAsync<Redis>>`, tag `7-alpine`), a unique key per
   test (`rc-redis-cli-<test>`), and `DEL` clean-first.
5. In `crates/camel-cli/tests/feature_profiles.rs` update
   `flavor_marker_table`'s expected `flavor-full` line to include
   `"integration-redis"`; leave `NON_FLAVOR_AXES` untouched — placement in
   `flavor-full` is what keeps `full_covers_universe` green.
6. Run the compile gates below and record exit codes.

**Tests** (write the `flavor_marker_table` edit FIRST: RED while the new
feature is unplaced, GREEN after step 2).
- `flavor_marker_table`: arrange — read `crates/camel-cli/Cargo.toml`; act
  — collect the `[features]` lines starting with `flavor-`/`default`; assert
  — the exact `flavor-full` line reads `flavor-full = ["flavor-regular",
  "exec", "kafka", "surrealdb", "integration-surreal", "integration-redis",
  "containers"]`. `cmd: cargo test -p camel-cli --test feature_profiles
  flavor_marker_table`. RED/GREEN.
- `full_covers_universe`: arrange — parse the `[features]` table; act —
  compute `flavor-full`'s transitive body; assert — `integration-redis` is
  in the body and no non-axis feature is unplaced. `cmd: cargo test -p
  camel-cli --test feature_profiles full_covers_universe`. RED while
  unplaced, GREEN after step 2.
- `default_closure_matches_golden`: arrange — `cargo tree -p camel-cli -e
  features,no-dev`; act — compare against the golden fixture; assert — the
  default closure is unchanged (`integration-redis` is not in
  `flavor-regular`, so it never appears). `cmd` as above. GREEN before and
  after.
- `redis_tls_implies_redis`: arrange — read `crates/camel-cli/Cargo.toml`;
  act — collect the `[features]` lines starting with `redis`; assert — the
  set is exactly `redis = ["dep:camel-component-redis",
  "camel-bundles/redis"]` and `redis-tls = ["redis",
  "camel-component-redis/tls"]` (the new
  `integration-redis` line starts with `integration`, so it is not
  collected). `cmd` as above. GREEN before and after.
- Standalone compiles: `cargo check -p camel-integration-test
  --no-default-features --features redis` and `cargo check -p camel-cli
  --no-default-features --features integration-redis` exit 0.
- Redis CLI e2e (in `test_scenario_cli_e2e.rs`, `#[cfg(feature =
  "integration-redis")]`; command `cargo test -p camel-cli
  --no-default-features --features integration-redis,itest-e2e --test
  test_scenario_cli_e2e <test_fn_name>`; the shared container + unique key
  `rc-redis-cli-<test>` per step 4). These are the ONLY tests that exercise
  the redis-only CLI path; without them the redis feature compiles but runs
  zero Redis scenarios.
  - `redis_only_doc_boots_full`: arrange a temp project with `Camel.toml`
    `[datasources.statedb] provider = "redis" db_url =
    "redis://127.0.0.1:<port>/0"`, `routes.yaml` `direct:start` →
    `redis://127.0.0.1:<port>?command=SET&key=rc-redis-cli-direct`, and a
    document that `send`s to `direct:start` then `validate`s a redis
    `string` target on that key with a `deadline`; act spawn `camel test
    <doc>`; assert exit 0, stdout contains `[full]`, the
    `#scenario[0] send` and `#scenario[1] validate` PASS rows, and `2
    passed, 0 failed`.
  - `redis_validate_only_doc_boots_full`: arrange the same project but seed
     `rc-redis-cli-validate-only-pass` out-of-band via a raw `redis::Client`
    `SET`, and a document with ONLY a redis `validate` `string` target on
    that key (no `send`/`receive`; the route has a `direct:` consumer the
    document never stimulates); act spawn `camel test`; assert exit 0,
    `[full]`, `#scenario[0] validate`, and `1 passed, 0 failed` — this
    exercises the `has_redis` target-side full-boot selection (the `wired`
    predicate alone would keep it on the smoke path and fail closed).
  - `redis_validate_only_mismatch_exits_1`: arrange — seed
     `rc-redis-cli-validate-only-mismatch` = `CLI_ACTUAL_SECRET` and a
     document whose redis `validate` `string` target expects
     `rows: [["CLI_EXPECTED_SECRET"]]`; use that unique mismatch key for
     setup, target, and cleanup; act — spawn
    `camel test <doc>`; assert — exit code 1 (verdict class), the
    path-aware row assertion (the row format is `FAIL {path}#{endpoint} —
    {detail}`, so the document path sits between `FAIL ` and `#`):
    `stdout.lines().any(|l| l.starts_with("FAIL ")
    && l.contains("#scenario[0] validate"))`, plus
    `stdout.contains("0 passed, 1 failed")`; the `validation-mismatch`
    report line names the key, the declared and observed types, and the
     counts, and contains neither `CLI_ACTUAL_SECRET` nor
     `CLI_EXPECTED_SECRET`; count labels `actual` and `expected` remain
     allowed. Do NOT assert the
    literal `FAIL #scenario[0] validate` (the path breaks that substring).
- Feature-off preservation: in a build without `integration-redis`, a
  document whose only action is a redis validate target SHALL stay on the
  smoke path and fail at action time with the named
  `redis validation requires the redis feature` message. The twin returns
  `ValidationMismatch`, which is VERDICT-class: the CLI exits **1** and the
  detail renders on **stdout** as a `FAIL {path}#scenario[0] validate`
  row — NOT exit 2 and NOT stderr. Command `cargo test -p camel-cli
  --no-default-features --features integration-sql,itest-e2e --test
  test_scenario_cli_e2e redis_target_feature_off_demand_gate` (the
  `redis_target_feature_off_demand_gate` test is `#[cfg(all(feature =
  "itest-e2e", not(feature = "integration-redis")))]`). Arrange — a
  redis-target-only document (a `direct:` route it never stimulates, so
  `wire_endpoint_refs` is empty and `has_redis` is the feature-off twin
  `false`, keeping the run on the smoke path); act — spawn `camel test`;
  assert — `output.status.code() == Some(1)`, `stdout.lines().any(
  |l| l.starts_with("FAIL ") && l.contains("#scenario[0] validate") &&
  l.contains("requires the `redis` feature"))`, and
  `stdout.contains("0 passed, 1 failed")`; assert the run did NOT take the
  generic full-boot path (no `full-boot-failure` in the output).
- All-targets (benches/examples included): `cargo check -p
  camel-integration-test --all-targets --features redis`, `cargo check -p
  camel-component-redis --all-targets`, and `cargo check -p camel-bundles
  --all-targets --features redis` exit 0.
- Gate lints: `cargo xtask lint-gate-forwarding` and `cargo xtask
  lint-component-deps` exit 0.

**Acceptance:**
- `cargo test -p camel-cli --test feature_profiles` passes (18 tests,
  baseline count preserved).
- `cargo check -p camel-cli --no-default-features --features
  integration-redis,itest-e2e --tests` exits 0 (the Redis CLI e2e cases
  compile under the redis-only gate); the cases themselves run in Task 4's
  `integration-redis` workflow.
- All other commands in **Tests** exit 0.
- `cargo xtask lint-publish-cycles` and `cargo xtask
  lint-publish-registration` exit 0.

- [x] 3

## camel-integration-test and CI — Docker integration battery

### Task 4: testcontainers Redis battery (unique keys, bounded coordination, recovery, TTL, apparatus timing, teardown) and the `integration-redis` workflow

**Files:**
- `crates/camel-integration-test/src/runner/redis_validate.rs` (modified — the colocated `redis-live` test module)
- `crates/camel-integration-test/tests/redis_state_test.rs` (new)
- `crates/camel-integration-test/tests/fixtures/redis_state/Camel.toml` (new)
- `crates/camel-integration-test/tests/fixtures/redis_state/routes/order.yaml` (new)
- `crates/camel-integration-test/tests/fixtures/redis_state/order.test.yaml` (new)
- `crates/camel-integration-test/Cargo.toml` (modified — `redis-live` feature + testcontainers dev-deps)
- `.github/workflows/integration-redis.yml` (new)

**Steps:**
1. In `crates/camel-integration-test/Cargo.toml` add the test-only feature
   `redis-live = []` to `[features]` and the `[dev-dependencies]`
   `testcontainers.workspace = true` and
   `testcontainers-modules.workspace = true` (the workspace entry already
   enables `redis`). The live module is gated
   `#[cfg(all(test, feature = "redis", feature = "redis-live"))]`, so the
   default `cargo test -p camel-integration-test --lib` and Task 1's
   `--lib --features redis` executed sets are unchanged and start no
   container.
2. In `runner/redis_validate.rs` add the colocated live module
   `#[cfg(all(test, feature = "redis", feature = "redis-live"))] mod live`.
   It starts one shared Redis container (`OnceCell<ContainerAsync<Redis>>`,
   tag `7-alpine`, the `camel-test/tests/support/redis.rs` idiom) and
   opens a `redis::Client` against it. Every test uses a UNIQUE literal key
   (`rc-redis-state:<test_fn_name>`) and `DEL`s it before and after
   (clean-first), so tests never share state on the durable container.
3. The first-snapshot ack uses the `poll_with_source` seam (no global
   state, no release hook): the test's source closure wraps the private
   `eval_raw`, and on its FIRST call it sends a `tokio::sync::oneshot`
   `first_taken` signal and awaits an `ack` oneshot BEFORE returning the
    fetched raw reply. Bound the entire controller sequence (waiting for
    `first_taken`, performing the mutation, and sending `ack`) with
    `tokio::time::timeout(Duration::from_secs(1), controller)`. The source
    bounds waiting for `ack` with the same one-second timeout. Recovery
    tests use a three-second validation deadline and count acquisitions:
    assert the captured first observation and at least two acquisitions.
    After acknowledgment, join using
    `tokio::time::timeout(Duration::from_secs(10), &mut validate_handle)`.
    On controller error or either timeout, drop the acknowledgment sender,
    abort the validation task, then await its handle under a one-second
    cleanup timeout before reporting failure; cleanup the unique key.
    No spawned task may remain detached after a failed handshake. This
   guarantees the SAME deadline execution observed the pre-mutation first
   snapshot before the mutation landed; no wall-clock sleep, no spawn
   race. The executor's second/final snapshot then reads the mutated state.
4. In `tests/redis_state_test.rs` add the full-boot e2e and lifecycle
   tests (`#![cfg(feature = "redis")]`): `LayeredEnv` is built with the
   doc env and a harness-provisioned map carrying `REDIS_URL =
   "redis://127.0.0.1:<mapped>/0"` (for `Camel.toml`'s
   `${env:REDIS_URL}`) and `REDIS_PRODUCER_URI =
   "redis://127.0.0.1:<mapped>?command=SET&key=rc-redis-state:<name>"` (for
   `routes/order.yaml`'s `to: ${env:REDIS_PRODUCER_URI}`); `boot_scenario`
   resolves `${env:}` through `LayeredEnv` for both config and route
   discovery, so no process env is touched and no fixed port is guessed.
5. Add `.github/workflows/integration-redis.yml` mirroring
   `integration-sql.yml`/`integration-surreal.yml` path filters (Cargo.toml,
   Cargo.lock, `crates/components/camel-redis/**`,
   `crates/camel-integration-test/**`, `crates/camel-bundles/**`,
   `crates/camel-cli/src/**`,
   `crates/camel-cli/tests/test_scenario_cli_e2e.rs`,
   `crates/camel-cli/Cargo.toml`, the workflow itself) and steps: disk
   reclaim, toolchain, rust-cache, then (a) the independence proof
   `cargo test -p camel-cli --no-default-features --features
   integration-redis,itest-e2e --test test_scenario_cli_e2e`, (b) the
   colocated live battery `cargo test -p camel-integration-test --features
   redis,redis-live --lib`, and (c) the full-boot e2e `cargo test -p
   camel-integration-test --features redis --test redis_state_test`. No
   `#[ignore]` on any loopback test.
   Set the job-level `timeout-minutes: 30` ceiling (ADR-0069 §13.2 R6).
   Bound each Redis live/boot test's complete async execution, including
   setup and cleanup, to 120 seconds. Redis CLI tests use separate
   setup/cleanup async budgets of 60/10 seconds and the existing bounded
   `common::run_binary` helper's 90-second child deadline with concurrent
   pipe draining and kill/reap. Do not claim a Tokio total ceiling across
   that synchronous helper.
   Refusal tests use the ADR-0070 reserved `127.0.0.1:1` address, not a
   bind/read/drop ephemeral-port probe.

**Tests.** Colocated live tests run with `cargo test -p
camel-integration-test --features redis,redis-live --lib <test_fn_name>`;
the full-boot e2e runs with `cargo test -p camel-integration-test
--features redis --test redis_state_test <test_fn_name>`. Write first; the
RED is the missing module/wiring.

- Colocated live (in `redis_validate.rs` `mod live`; real `eval_raw`
  through `poll_with_source`; first-snapshot ack per step 3; unique key
  `rc-redis-state:<test_fn_name>` `DEL`ed before and after; every spawned
  task aborted and joined under timeout on every path):
  - `live_initially_missing_key_appears_within_deadline_passes`:
    arrange — `DEL` the unique key; source wraps `eval_raw` and records
    each decoded `observed` plus an acquisition counter; target `string`
    with `rows: [["v"]]`, deadline `3s`. act — await `first_taken` (the
    same execution's first acquisition), `SET key v`, send `ack`, join the
    validate under `10s`. assert — `Ok(())`; the recorded first observation
    is `None` (the missing first snapshot was verdict data); acquisitions
    ≥ 2.
  - `live_transient_wrong_type_corrected_within_deadline_passes`:
    arrange — `SET key v`; target `hash` with `columns: [value]` and
    `rows: [["alice"]]`, deadline `3s`. act — await `first_taken`, then
    `DEL` + `HSET key name alice` (retype), `ack`, join. assert — `Ok(())`;
    first observation `Some(String)`; acquisitions ≥ 2.
  - `live_no_early_settle_first_present_then_deleted_final_snapshot_fails`:
    arrange — `SET key v`; target `string` with `rows: [["v"]]`, deadline
    `3s`. act — await `first_taken` (present), `DEL`, `ack`, join. assert —
    `ValidationMismatch` on the final snapshot; first observation
    `Some(String)`; acquisitions ≥ 2 (a matching first snapshot is not
    proof).
  - `live_ttl_decided_at_deadline_final_snapshot`: arrange — `SET key v`
    then `PEXPIRE key 60000`; target `string` with
    `ttl: Some(AtLeast(30_000))`, deadline `3s`. act — await `first_taken`
    (assert the captured first `ttl` is `Remaining(ms)` with `ms >= 30000`),
    `PERSIST key`, `ack`, join. assert — `ValidationMismatch` on the final
    snapshot naming the bound and the persistent status; acquisitions ≥ 2;
    no early successful settlement.
  - `live_projection_failure_is_apparatus_and_stops_immediately`: arrange —
    a raw `redis::Client` `SET key <0xff 0xfe>` (non-UTF8); target `string`
    with a count bound, deadline `2s`; source counts acquisitions. act —
    join the validate. assert — `ActionTransport` (never a verdict),
    `elapsed < 1s`, and exactly one acquisition (the apparatus path stops
    the poll immediately).
  - `live_mutated_key_is_observed_as_one_coherent_snapshot`: a bounded
    acquisition-ACK phased mutation proving a mutation landing between two
    acquisitions of one deadline execution is observed as two coherent
    snapshots. arrange — `SET key pre` (a string) before starting; the
    source uses a call-index protocol: on call k it `eval_raw`s, decodes
    via `decode_snapshot` to record `observed` into a
    `Mutex<Vec<Option<RedisType>>>`, and for k ∈ {1, 2} sends `sampled(k)`
    on an `mpsc` and awaits `ack(k)` (bounded by
    `tokio::time::timeout(Duration::from_secs(1), ack_rx)`) before
    returning the raw (k ≥ 3 returns immediately); target `string` with
    count bound `atLeast: 0` (atomicity, not values), deadline `3s`. act —
    run the controller inline (no spawned retyper): await `sampled(1)`
    (assert observed `Some(String)`), mutate `DEL key; HSET key f v`
    (string→hash), send `ack(1)`; await `sampled(2)` (assert observed
    `Some(Hash)`), send `ack(2)`; join the validate under
    `tokio::time::timeout(Duration::from_secs(10), validate)`. assert — the
    recorded observations begin `[Some(String), Some(Hash)]` (the SAME
    deadline execution sampled string then hash, each mutation acked after
    the preceding acquisition) and the result is `Ok(())` or
    `ValidationMismatch`, never `ActionTransport` (no snapshot errors on a
    mid-window retype). cleanup — `DEL key`; on any controller error or
    timeout, drop the ack senders, abort the validate task, and await its
    handle under a 1s cleanup timeout, so no
    task is left detached.
  - `live_projections_all_types`: arrange — five unique keys: `SET`
    `alice`; `HSET` `name alice`; `RPUSH` `a b`; `SADD` `a b c`; `ZADD
    1.5 bob 2 alice` (score-then-member order). act — validate each with
    its exact rows (`string` `[["alice"]]`; `hash` columns `[field,value]`
    `[["name","alice"]]`; `list` `[[0,"a"],[1,"b"]]`; `set`
    `[["a"],["b"],["c"]]`; `zset` `[["bob",1.5],["alice",2]]` — ascending
    rank). assert — each validate is `Ok(())`.
  - `live_columns_subset_projection`: arrange — `HSET key name alice age
    42`; act — validate `hash` `columns: [value]`, `unordered: true`, rows
    `[["42"],["alice"]]`; assert — `Ok(())` (field-sorted: age before name,
    only the value column).
  - `live_columns_reorder_projection`: arrange — the same hash; act —
    validate `hash` `columns: [value, field]`, ordered rows
    `[["42","age"],["alice","name"]]`; assert — `Ok(())` (field-sorted,
    declaration order).
  - `live_wildcard_ignore_cell_matches_any_value`: arrange — `HSET key name
    alice`; act — validate `hash` `columns: [field,value]`, rows
    `[[{ignore: null}, {equals: "alice"}]]`; assert — `Ok(())`.
  - `live_integral_and_non_integral_scores_match`: arrange — `ZADD key
    1.5 bob 2 alice` (score-then-member order); act — validate `zset`
    ordered rows `[["bob",1.5],["alice",2]]`; assert — `Ok(())` (ascending
    rank; the integral `2` matches the JSON integer, `1.5` the float).
  - `live_ttl_at_least_passes_and_at_most_fails`: arrange — `SET key v EX
    60`; act — validate `ttl: Some(AtLeast(30_000))`; assert `Ok(())`; act
    again with `ttl: Some(AtMost(30_000))`; assert `ValidationMismatch`
    naming the bound and the observed milliseconds and containing no value.
  - `live_persistent_and_missing_keys_fail_ttl_bound`: arrange — `SET key
    v` with no expiry; act — validate `ttl: Some(AtLeast(1000))`; assert
    `ValidationMismatch` naming the persistent status; arrange — `DEL key`;
    act — validate the same bound; assert `ValidationMismatch` naming the
    key and observed `none`.
  - `live_ceiling_breach_fails_immediately`: arrange — `SADD key a b`;
    target `set` with `bound: Some(AtMost(1))`, deadline `2s`; act — join;
    assert — `ValidationMismatch` with `elapsed < 1s` (the early judgment
    fires on the first snapshot).
- Full-boot e2e (in `tests/redis_state_test.rs`, `#![cfg(feature =
  "redis")]`; unique key `rc-redis-state:<test_fn_name>`; every test
  `DEL`s the key before and after):
  - `redis_state_e2e_route_write_validate`: arrange — the fixture project
    (`Camel.toml` with `provider = "redis"` and `${env:REDIS_URL}`, a
    `direct:order` route whose `to:` is `${env:REDIS_PRODUCER_URI}`, the
    `order.test.yaml` send-then-validate document) booted through
    `boot_scenario` with the harness env map. act — run the document via
    `run_scenario_document` and `shutdown`. assert — every action `Ok`,
    verdict `Pass`, `final_failure` `None`; `DEL` the key.
  - `redis_datasource_create_succeeds_and_handles_ping`: arrange — the
    booted run. act — `run.boot.datasource_catalog().get_pool("statedb")`
    then downcast. assert — the downcast is a
    `redis::aio::MultiplexedConnection` and `PING` returns `Ok`.
  - `missing_key_is_validation_mismatch_at_expiry`: arrange — `DEL` the
    unique key; target `string`, no deadline. act — validate. assert —
    `ValidationMismatch` naming the key, declared `string`, observed
    `none`.
  - `wrong_type_at_expiry_is_validation_mismatch`: arrange — `SET key v`;
    target `hash`. act — validate. assert — `ValidationMismatch` naming
    declared `hash`, observed `string`, value not projected.
  - `driver_error_is_sanitized_live`: arrange — a catalog whose redis
    datasource points at a closed port. act — validate. assert —
    apparatus-class `ActionTransport` carrying the datasource name and no
    `db_url`.
  - `redis_single_catalog_invariant`: arrange — the booted catalog. act —
    `get_pool("statedb")` twice, downcast both. assert — the two
    `Arc<MultiplexedConnection>` handles are `Arc::ptr_eq`.
  - `redis_handle_is_drop_scoped_not_close_scoped`: arrange — first boot
    reads the alias and clones the downcast connection. act — teardown and
    drop the boot-owned catalog/context, then boot the same alias again.
    assert — the second handle is NOT `Arc::ptr_eq` with the cloned first,
    and the unique key persists (the no-op close released nothing; the
    author's clean-first responsibility).
  - `shutdown_closes_pools_without_releasing_redis`: arrange — a booted
    scenario whose redis validate resolved a handle and seeded a unique
    key. act — `run.boot.shutdown(..)`. assert — shutdown returns without
    close errors and a post-shutdown read still sees the key.

**Acceptance:**
- `cargo test -p camel-integration-test --features redis,redis-live --lib`
  passes (colocated live battery, Docker runner).
- `cargo test -p camel-integration-test --features redis --test
  redis_state_test` passes (full-boot e2e).
- `cargo test -p camel-integration-test --lib` still passes and starts no
  container (the live module is gated by `redis-live`).
- `.github/workflows/integration-redis.yml` exists, matching the
   `integration-surreal.yml` shape, and its three commands exit 0 in CI.
- The workflow declares `timeout-minutes: 30`. Redis live/boot executions
  have a 120-second ceiling, including cleanup. Redis CLI setup/cleanup
  have separate 60/10-second async ceilings. Child executions use the
  existing bounded `common::run_binary` helper's 90-second deadline.

- [x] 4

## docs — book page, ADR ladder, CONTEXT terms

### Task 5: `scenario-redis.md`, SUMMARY/index entries, ADR-0069 §8 row, CONTEXT family terms, final gates

**Files:**
- `docs/src/testing/scenario-redis.md` (new)
- `docs/src/testing/index.md` (modified)
- `docs/src/SUMMARY.md` (modified)
- `docs/adr/0069-integration-tier-testing-contract.md` (modified)
- `crates/camel-integration-test/CONTEXT.md` (modified)
- `crates/components/camel-redis/CONTEXT.md` (modified)

**Steps:**
1. Write `docs/src/testing/scenario-redis.md` mirroring
   `scenario-sql.md`/`scenario-surreal.md`: the assertion-only family (no
   `redis:` prepare action; routes seed through the landed `redis:`
   producer); the `target: {redis: {datasource, key, type, ttl?}}` grammar;
   the inherent per-type schema and the `columns` subset/reorder rule; the
   `rows`/count-bound expectation reuse; the fail-closed value law and the
   integral-score range `[-2^63, 2^63)` (out-of-range and non-finite scores
   are apparatus); the atomic single-`EVAL` snapshot (type/payload/TTL from
   one read) and the no-early-settle final-snapshot poll; the TTL semantics
   (`-2` missing, `-1` persistent, `0` remaining, no legal zero bound,
   other negatives apparatus); the driver URL grammar
   (`redis://[user][:pass@]host[:port][/db]`), the `provider = "redis"`
   convention, and the canonical `camel_api::redact` boundary; the
   clean-first responsibility for durable Redis state; the
   `integration-redis` Docker job.
2. Add the page to `docs/src/testing/index.md` (beside the SQL/SurrealDB
   chapters) and to `docs/src/SUMMARY.md` under Testing.
3. In ADR-0069 §8 add the redis rung: the demand signal (`redis` feature),
   the `integration-redis` CI job, the Docker (testcontainers) carrier, and
   the assertion-only boundary.
4. Update `crates/camel-integration-test/CONTEXT.md` (the redis validate
   target, the projection/TTL law, the `redis validation` family label, the
   drop-scoped handle) and `crates/components/camel-redis/CONTEXT.md` (the
   `RedisPoolFactory` and `RedisBundle::with_catalog` datasource hook,
   distinct from the endpoint parser). Cite `openspec/specs/integration-tier`
   per lint-context-citations.

**Tests:**
- `cargo xtask lint-context-citations` exits 0 (every new CONTEXT claim
  cites the spec).
- `cargo xtask schema --check` exits 0.
- `mdbook build docs` and `mdbook test docs` exit 0.
- No Rust test owns prose; these executable checks are the owning gate.

**Acceptance:**
- `cargo xtask lint-context-citations`, `cargo xtask schema --check`,
  `mdbook build docs`, and `mdbook test docs` all exit 0.
- `cargo xtask lint-log-redaction` and `cargo xtask lint-unwrap` exit 0.
- Final all-targets gate (benches/examples included): `cargo check -p
  camel-integration-test --all-targets --features redis`, `cargo check -p
  camel-component-redis --all-targets`, and `cargo check -p camel-cli
  --no-default-features --features integration-redis --all-targets` exit 0.
- Baseline batteries re-run green: `cargo test -p camel-integration-test
  --lib`, `cargo test -p camel-cli --test feature_profiles`, and the
  existing SQL and Surreal suites unchanged.

- [x] 5
