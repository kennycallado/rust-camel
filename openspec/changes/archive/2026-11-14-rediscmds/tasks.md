# Tasks: rediscmds

<!--
  Three delivery phases (design.md ## Phases). The whole file is blessed once.
  Phase 1: string family. Phase 2: 6.2/7.0 collections. Phase 3: generics + close-out.
  All tests run from the worktree root. Gate command for every code task:
  cargo fmt --check --all && cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings && cargo test -p camel-component-redis --lib
-->

## Phase 1: string family

### Task 1.1: config.rs + executor.rs — string variants, parsing, classification, routing

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)

**Steps:**
1. Add four enum variants to `RedisCommand`: `Getdel`, `Getex`, `Setrange`, `Getrange` (place next to the existing string variants `Set`/`Get`/`Getset`).
2. Add `"GETDEL" => Ok(RedisCommand::Getdel)`, `"GETEX" => Ok(RedisCommand::Getex)`, `"SETRANGE" => Ok(RedisCommand::Setrange)`, `"GETRANGE" => Ok(RedisCommand::Getrange)` to `FromStr`, mirroring the existing `"SETEX"` arm placement and comment style.
3. Introduce `pub fn as_str(&self) -> &'static str` on `RedisCommand` in `config.rs` (it does not exist yet): one arm per variant returning the uppercase wire token, grouped by family with the same comment style as `FromStr`; include all existing variants plus the four new ones. Later tasks only add arms to this method.
4. In `is_idempotent_command`: add `RedisCommand::Setrange` and `RedisCommand::Getrange` to the true arms (`Setrange` under "Idempotent writes" with the comment `// SETRANGE re-writes the same bytes at a fixed offset; a retry converges`, `Getrange` under reads). Do NOT add `Getdel`/`Getex` — they stay non-idempotent. Add a comment naming them: `// GETDEL/GETEX are absent: the reply carries a payload the first attempt consumed; a retry reports nil.`
5. Reclassify `RedisCommand::Set`: remove it from the true arms; replace its old comment line with: `// SET is absent (reclassified): with CamelRedis.ReturnOldValue a retry reports the post-write value as the old value, and with NX it reports failure after an unconfirmed success. The classifier cannot see options, so the worst-case form governs.` (design.md D3; this is the one deliberate behavior change of this change.) Also update the stale `is_idempotent_command` doc comment above the classifier (it still names SET among the idempotent writes) and flip the existing `test_is_idempotent_command_idempotent_writes` test, which asserts `Set` is true.
6. In `executor.rs`, extend the string routing arm with `| RedisCommand::Getdel | RedisCommand::Getex | RedisCommand::Setrange | RedisCommand::Getrange` — the dispatch match is exhaustive with no wildcard, so the variants must route in the same task that adds them. The `is_string_command` gate rejects them with "Not a string command" until Tasks 1.3/1.4 implement the arms (the proven Task 2.1 template).
7. Extend the existing command-classification unit test in `config.rs`'s test module to cover the four new variants and the `Set` flip.

**Tests:**
- name: `new_string_commands_parse_roundtrip`
  - setup: none (pure functions)
  - action: for each of `("GETDEL", Getdel)`, `("GETEX", Getex)`, `("SETRANGE", Setrange)`, `("GETRANGE", Getrange)` parse with `RedisCommand::from_str` and render with `as_str`
  - assert: parse yields the named variant and `as_str` returns the original uppercase wire token
  - command: `cargo test -p camel-component-redis --lib new_string_commands_parse_roundtrip`
  - expected: fails before step 1-3 (variants missing), passes after
- name: `string_family_retry_classification`
  - setup: none
  - action: call `is_idempotent_command` on `Getdel`, `Getex`, `Setrange`, `Getrange`, `Set`
  - assert: `Getdel`/`Getex`/`Set` are false; `Setrange`/`Getrange` are true
  - command: `cargo test -p camel-component-redis --lib string_family_retry_classification`
  - expected: fails before steps 4-5, passes after

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0 (the crate compiles: variants route in the same task; `is_string_command` rejects them until Tasks 1.3/1.4)
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0
- `grep -c "RedisCommand::Set$" crates/components/camel-redis/src/config.rs` shows `Set` no longer listed inside the `is_idempotent_command` `matches!` block
- `as_str` exists on `RedisCommand` and covers every current variant (a non-exhaustive match fails the clippy gate)

- [x] 1.1

### Task 1.2: SET option surface — headers, conflicts, executor

**Files:**
- `crates/components/camel-redis/src/commands/mod.rs` (modified)
- `crates/components/camel-redis/src/commands/string.rs` (modified)

**Steps:**
1. In `commands/mod.rs`, add `pub(crate) fn require_bool_header(exchange: &Exchange, key: &str) -> Result<Option<bool>, CamelError>`: returns `Ok(None)` when the header is absent, `Ok(Some(b))` when it parses as a boolean, and a named error (`Invalid value for header {key}: expected a boolean, got {preview}` via the existing `invalid_header_value_preview` helper) when present-but-malformed. Do not rely on `get_bool_header` for these options — verify whether it silently accepts garbage; either way the new strict helper is the required path.
2. In `string.rs`, add `enum ExpireUnit { Seconds, Milliseconds }` and `fn resolve_expire_unit(exchange: &Exchange) -> Result<ExpireUnit, CamelError>`: reads `CamelRedis.ExpireUnit` case-insensitively (`"SECONDS"`/`"MILLISECONDS"`), defaults to `Seconds` when absent, named error naming the header and both valid values otherwise. Keep the enum and function file-private (no `pub(crate)`): every caller lives in `string.rs`, and a `pub(crate)` fn returning a private type trips the `private_interfaces` deny lint.
3. In `string.rs`, add `enum SetCondition { Nx, Xx }`, `enum SetExpiry { Ex(u64), Px(u64), Exat(i64), Pxat(i64), KeepTtl }`, `struct SetOptions { condition: Option<SetCondition>, expiry: Option<SetExpiry>, return_old_value: bool }`, and `fn resolve_set_options(exchange: &Exchange) -> Result<SetOptions, CamelError>` — all file-private, same visibility rule as step 2 — implementing the conflict rules:
   - `CamelRedis.SetCondition` invalid value → named error (valid `NX`/`XX`); single-valued header makes NX+XX structurally unrepresentable (design.md D1).
   - `CamelRedis.KeepTtl=true` (via `require_bool_header`) together with `CamelRedis.Timeout` or `CamelRedis.Timestamp` → named error naming `CamelRedis.KeepTtl` and the expiry header.
   - `CamelRedis.Timeout` and `CamelRedis.Timestamp` both present → named error naming both headers.
   - `CamelRedis.ExpireUnit` present with neither `Timeout` nor `Timestamp` → named error naming the unit header and the missing expiry.
   - `Timeout` must parse via `get_u64_header` and be positive (`require_positive`, message: a positive expiry in the selected unit is required for SET; Redis rejects EX 0/PX 0). `Timestamp` parses via `get_i64_header` (existing checked conversion).
   - `CamelRedis.ReturnOldValue` via `require_bool_header`; malformed → named error.
4. Rebuild the `RedisCommand::Set` arm in `dispatch`: resolve the value via `body_value_bytes(exchange, "SET", &[HEADER_VALUE])`, then build `redis::cmd("SET")` with args in canonical order: `key`, `value`, `NX`/`XX` if present, then the expiry args (`EX n`/`PX n`/`EXAT n`/`PXAT n`/`KEEPTTL`), then `GET` when `return_old_value`. When `return_old_value` is set, take the reply through the binary-safe `reply_to_body` early-return path (mirror the `Get` arm, including its comment); otherwise reply `serde_json::Value::Null` through the dispatch tail.
5. Add the resolver unit tests listed below in `string.rs`'s test module, reusing `ex_with`.

**Tests:**
- name: `resolve_set_options_defaults_and_expiry`
  - setup: exchange with only `CamelRedis.Timeout=15` → `Ex(15)`; `Timeout=500` + `ExpireUnit=MILLISECONDS` → `Px(500)`; `Timestamp=1731000000` → `Exat(1731000000)`; `Timestamp=1731000000000` + `ExpireUnit=MILLISECONDS` → `Pxat(..)`; `KeepTtl=true` alone → `KeepTtl`
  - action: call `resolve_set_options`
  - assert: the mapped `SetExpiry` variant matches each case; absent headers yield `condition: None`, `return_old_value: false`
  - command: `cargo test -p camel-component-redis --lib resolve_set_options_defaults_and_expiry`
  - expected: fails before steps 2-3, passes after
- name: `resolve_set_options_conflicts_fail_closed`
  - setup: five exchanges: (`KeepTtl=true` + `Timeout=15`), (`Timeout=15` + `Timestamp=1731000000`), (`SetCondition=MAYBE`), (`Timeout=5` + `ExpireUnit=HOURS`), (`ExpireUnit=MILLISECONDS` alone)
  - action: call `resolve_set_options` on each
  - assert: every call errors before send; messages name the headers from the spec scenarios (`CamelRedis.KeepTtl`, both expiry headers, `CamelRedis.SetCondition`, `CamelRedis.ExpireUnit` twice)
  - command: `cargo test -p camel-component-redis --lib resolve_set_options_conflicts_fail_closed`
  - expected: fails before step 3, passes after
- name: `resolve_set_options_malformed_booleans_and_zero_expiry`
  - setup: exchanges with `KeepTtl="yes-please"`, `ReturnOldValue="maybe"`, `Timeout=0`
  - action: call `resolve_set_options`
  - assert: named errors naming `CamelRedis.KeepTtl` / `CamelRedis.ReturnOldValue` / `CamelRedis.Timeout` (with the positive requirement) and echoing the received value
  - command: `cargo test -p camel-component-redis --lib resolve_set_options_malformed_booleans_and_zero_expiry`
  - expected: fails before steps 1/3, passes after

**Acceptance:**
- all three tests pass; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0
- no `CamelRedis.Value` or other value header appears in the SET arm (payload stays `body_value_bytes`)

- [x] 1.2

### Task 1.3: GETDEL and GETEX executors

**Files:**
- `crates/components/camel-redis/src/commands/string.rs` (modified)

**Steps:**
1. Add `enum GetexExpiry { Ex(u64), Px(u64), Exat(i64), Pxat(i64) }` and `struct GetexOptions { expiry: Option<GetexExpiry>, persist: bool }` with `fn resolve_getex_options(exchange: &Exchange) -> Result<GetexOptions, CamelError>` (file-private, same visibility rule as Task 1.2): reuse `resolve_expire_unit`; `CamelRedis.Persist` via `require_bool_header`; conflicts — `Persist=true` with any expiry header → named error naming `CamelRedis.Persist` and the expiry header; `Timeout` + `Timestamp` → named error naming both; `Timeout` positive via `require_positive`; `CamelRedis.ExpireUnit` present with neither `Timeout` nor `Timestamp` → named error naming the unit header and the missing expiry (the same rule as `SET`, including when `Persist=true`).
2. Add the `RedisCommand::Getdel` arm: `require_key`, wire `redis::cmd("GETDEL").arg(&key)`, reply through the `reply_to_body` early-return path (binary-safe, nil as the `GET` miss body — copy the `Get` arm's comment style).
3. Add the `RedisCommand::Getex` arm: `require_key`, build `redis::cmd("GETEX")` with `key`, then `EX n`/`PX n`/`EXAT n`/`PXAT n` per the resolved expiry, then `PERSIST` when `persist` and no expiry; reply through `reply_to_body`.
4. Add both variants to `is_string_command`.
5. Add the unit tests below.

**Tests:**
- name: `resolve_getex_options_mappings_and_conflicts`
  - setup: exchanges for: plain (no headers), `Timeout=100`, `Timeout=500`+`ExpireUnit=MILLISECONDS`, `Timestamp=1731000000`, `Timestamp=1731000000000`+`ExpireUnit=MILLISECONDS`, `Persist=true`, `ExpireUnit=MILLISECONDS` alone, (`Persist=true`+`ExpireUnit=MILLISECONDS` alone), (`Persist=true`+`Timeout=10`), (`Timeout=10`+`Timestamp=100`), `Persist="always"`
  - action: call `resolve_getex_options` on each
  - assert: plain → no expiry, no persist; the four expiry headers map to `Ex`/`Px`/`Exat`/`Pxat`; `Persist=true` maps alone; the unit-without-expiry cases (bare and with Persist) and the last three fail before send with messages naming `CamelRedis.ExpireUnit` / `CamelRedis.Persist` / both expiry headers / `CamelRedis.Persist` with the received value
  - command: `cargo test -p camel-component-redis --lib resolve_getex_options_mappings_and_conflicts`
  - expected: fails before steps 1, passes after
- name: `string_command_includes_getdel_getex`
  - setup: none
  - action: call `is_string_command` on `Getdel` and `Getex`
  - assert: both true; `Zadd` still false
  - command: `cargo test -p camel-component-redis --lib string_command_includes_getdel_getex`
  - expected: fails before step 4, passes after

**Acceptance:**
- tests pass; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 1.3

### Task 1.4: SETRANGE and GETRANGE executors

**Files:**
- `crates/components/camel-redis/src/commands/string.rs` (modified)

**Steps:**
1. Add `pub(crate) fn resolve_range_offset(exchange: &Exchange) -> Result<u64, CamelError>`: reads `CamelRedis.Offset` via `get_u64_header`; missing → named error naming `CamelRedis.Offset`; negative (the numeric-string `-1` form) → named error naming `CamelRedis.Offset` and the non-negative requirement (offset 0 is legal).
2. Add the `RedisCommand::Setrange` arm: `require_key`, `resolve_range_offset`, value via `body_value_bytes(exchange, "SETRANGE", &[HEADER_VALUE])`, wire `redis::cmd("SETRANGE")` with `key`, offset, value; reply is the integer new length as `serde_json::json!(n)` through the dispatch tail.
3. Add the `RedisCommand::Getrange` arm: `require_key`, `Start`/`End` via the existing `resolve_range_bounds`-style helpers used by `Lrange` (reuse or mirror them locally), wire `redis::cmd("GETRANGE")` with `key`, start, end; reply through `reply_to_body` (binary-safe slice).
4. Add both variants to `is_string_command`.
5. Add the unit tests below.

**Tests:**
- name: `resolve_range_offset_domains`
  - setup: exchanges with no `CamelRedis.Offset`, `Offset=0`, `Offset=6`, `Offset="-1"`
  - action: call `resolve_range_offset`
  - assert: missing errors naming `CamelRedis.Offset`; 0 and 6 resolve; `-1` errors naming `CamelRedis.Offset` and the non-negative requirement
  - command: `cargo test -p camel-component-redis --lib resolve_range_offset_domains`
  - expected: fails before step 1, passes after
- name: `string_command_includes_ranges`
  - setup: none
  - action: call `is_string_command` on `Setrange` and `Getrange`
  - assert: both true
  - command: `cargo test -p camel-component-redis --lib string_command_includes_ranges`
  - expected: fails before step 4, passes after

**Acceptance:**
- tests pass; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 1.4

### Task 1.5: Phase 1 wire tests — value_fake_tests.rs

**Files:**
- `crates/components/camel-redis/src/value_fake_tests.rs` (modified)

**Steps:**
1. Add wire tests using the existing `StreamFakeServer` + `call_producer` + `args_of` + `resp_*` helpers, one per command, each asserting the exact frame `args_of` captured:
   - `set_with_nx_and_xx_conditions_on_wire`: `SetCondition=NX` → frame `["k", "<body>", "NX"]`; repeat for XX.
   - `set_expiry_units_on_wire`: `Timeout=15` → `["k","<body>","EX","15"]`; `Timeout=500`+`ExpireUnit=MILLISECONDS` → `PX 500`; `Timestamp=1731000000` → `EXAT 1731000000`; `Timestamp=1731000000000`+`ExpireUnit=MILLISECONDS` → `PXAT 1731000000000`; `KeepTtl=true` → `KEEPTTL`.
   - `set_with_get_returns_old_value_binary_safe`: `ReturnOldValue=true`, scripted reply `resp_bulk("old")`, assert the frame ends with `GET` and the exchange body carries `old` bytes.
   - `set_conflicts_fail_before_any_wire_frame`: `KeepTtl=true`+`Timeout=15` exchange against a server that would answer SET; assert the call errors and `args_of("SET")` is empty.
   - `getdel_sends_key_and_returns_binary_value`: scripted `resp_bulk_bytes(b"\xff\x00")` — assert frame `["k"]` and body keeps the bytes.
   - `getex_expiry_and_persist_frames`: one call per mapping (`EX 100`, `PX 500`, `EXAT`, `PXAT`, `PERSIST`, plain `GETEX k`).
   - `setrange_frame_and_length_reply` / `getrange_frame_and_binary_slice`: exact frames `["k","6","world"]` and `["k","0","3"]`; GETRANGE reply bytes carried binary-safe; add an offset-zero SETRANGE case asserting the exact frame `["k","0","<body>"]` (the blessed "SETRANGE accepts offset zero" scenario).
2. Set headers on the exchange exactly as the existing tests do (mirror `keyed_exchange` + header sets used by neighboring tests in this file).

**Tests:**
- the eight test fns named in step 1
  - setup: `StreamFakeServer::start` with the scripted replies listed above
  - action: `call_producer(&server, "<CMD>", "", exchange)` per case
  - assert: `server.args_of("<CMD>")` frames equal the exact byte-string vectors from the spec scenarios; error cases leave `args_of` empty
  - command: `cargo test -p camel-component-redis --lib set_with_nx_and_xx_conditions_on_wire` (and each sibling name)
  - expected: fails before tasks 1.2-1.4 (unresolved variants route to "Not a string command"), passes after

**Acceptance:**
- all eight wire tests pass; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 1.5

### Task 1.6: Phase 1 docs — redis.md

**Files:**
- `docs/src/components/redis.md` (modified)

**Steps:**
1. Add one row per new command to the existing command table (`GETDEL`, `GETEX`, `SETRANGE`, `GETRANGE`), matching the table's column format exactly (read the neighboring rows first).
2. Add a `### SET and GETEX options` subsection directly after `### Expiration`, in the same table style: one row per option header (`CamelRedis.SetCondition` NX|XX, `CamelRedis.KeepTtl`, `CamelRedis.ReturnOldValue`, `CamelRedis.ExpireUnit` SECONDS|MILLISECONDS default SECONDS, `CamelRedis.Persist`) with `Used by`, `Default`, and `Rule` columns; document the conflict rules (KEEPTTL with any expiry; Timeout with Timestamp; ExpireUnit without an expiry; unit default SECONDS matching SETEX/EXPIREAT intuition).
3. Document the `SET` retry reclassification in that subsection: the component no longer auto-retries `SET` after a transport error because `ReturnOldValue`/`NX` lose replay data; there is no per-route classifier override; routes needing plain-SET retry semantics must use route-level redelivery, which re-executes the whole exchange and can duplicate other side effects.
4. Run `cd docs && mdbook test`.

**Tests:**
- name: (mdbook doctest suite)
  - setup: docs tree as edited
  - action: `cd docs && mdbook test`
  - assert: exit 0
  - command: `cd docs && mdbook test`
  - expected: passes after steps 1-3

**Acceptance:**
- `cd docs && mdbook test` exits 0
- the command table contains rows for all four Phase 1 commands (`grep -c "GETDEL\|GETEX\|SETRANGE\|GETRANGE" docs/src/components/redis.md` ≥ 4)
- the option mapping table and the SET retry note exist

- [x] 1.6

## Phase 2: 6.2/7.0 collections

### Task 2.1: config.rs + executor.rs — collection variants, dispatch, blocking list

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)

**Steps:**
1. Add sixteen variants to `RedisCommand`: `Lmpop`, `Blmpop`, `Lmove`, `Blmove`, `Lpos`, `Smismember`, `Sintercard`, `Zmpop`, `Bzmpop`, `Zrandmember`, `Zdiff`, `Zinter`, `Zunion`, `Zrangebylex`, `Hrandfield`, `Hincrbyfloat`, each with the `"LMPOP" => ..` style `FromStr` arm and an `as_str` arm (the method introduced in Task 1.1).
2. In `is_idempotent_command`: add to the true (read) arms `Smismember`, `Sintercard`, `Zdiff`, `Zinter`, `Zunion`, `Zrangebylex`, `Zrandmember`, `Hrandfield`, `Lpos` (one comment line: `// ZRANDMEMBER/HRANDFIELD follow SRANDMEMBER: state untouched, a fresh draw on retry is acceptable`). Do NOT add `Lmpop`, `Blmpop`, `Zmpop`, `Bzmpop`, `Lmove`, `Blmove`, `Hincrbyfloat`; add the comment: `// The pop/move family (LMPOP/BLMPOP/ZMPOP/BZMPOP/LMOVE/BLMOVE) is absent: a retry reports an empty pop or a different item and silently loses the popped one. HINCRBYFLOAT mirrors INCRBY: the reply encodes cumulative state. COPY/UNLINK/TOUCH are classified in Phase 3.`
3. In `executor.rs`, extend the dispatch match: `Lmpop | Blmpop | Lmove | Blmove | Lpos` → `commands::list::dispatch`; `Smismember | Sintercard` → `commands::set::dispatch`; `Zmpop | Bzmpop | Zrandmember | Zdiff | Zinter | Zunion | Zrangebylex` → `commands::zset::dispatch`; `Hrandfield | Hincrbyfloat` → `commands::hash::dispatch`.
4. In `executor.rs`, extend `is_blocking_endpoint` to `RedisCommand::Blpop | RedisCommand::Brpop | RedisCommand::Blmove | RedisCommand::Blmpop | RedisCommand::Bzmpop` and update its doc comment to name the five blocking commands (design.md D2).
5. `grep -rn "RedisCommand::" crates/components/camel-redis/src/` for any other exhaustive `matches!` over the enum (e.g. `metadata.rs`, `health.rs`, `consumer.rs`); extend the ones the compiler flags. Leave `queue.rs` `QueuePopCommand` (`BLPOP`/`BRPOP` only) untouched — the new blocking commands are producer endpoints, not queue-adapter pops.
6. Extend the classification unit test to cover all sixteen variants.

**Tests:**
- name: `collection_commands_parse_roundtrip`
  - setup: none
  - action: parse + `as_str` each of the sixteen new tokens (`LMPOP`, `BLMPOP`, `LMOVE`, `BLMOVE`, `LPOS`, `SMISMEMBER`, `SINTERCARD`, `ZMPOP`, `BZMPOP`, `ZRANDMEMBER`, `ZDIFF`, `ZINTER`, `ZUNION`, `ZRANGEBYLEX`, `HRANDFIELD`, `HINCRBYFLOAT`)
  - assert: variant and wire token round-trip
  - command: `cargo test -p camel-component-redis --lib collection_commands_parse_roundtrip`
  - expected: fails before step 1, passes after
- name: `collection_retry_classification`
  - setup: none
  - action: `is_idempotent_command` on all sixteen
  - assert: the nine reads true; `Lmpop`/`Blmpop`/`Zmpop`/`Bzmpop`/`Lmove`/`Blmove`/`Hincrbyfloat` false
  - command: `cargo test -p camel-component-redis --lib collection_retry_classification`
  - expected: fails before step 2, passes after
- name: `blocking_endpoint_includes_new_blocking_commands`
  - setup: construct the endpoint/config the existing `is_blocking_endpoint` test uses (find it in `executor.rs` tests)
  - action: evaluate `is_blocking_endpoint` for `Blmove`, `Blmpop`, `Bzmpop`, and for `Lmove` (non-blocking control)
  - assert: the three blocking commands true, `Lmove` false; `deadline()` returns `Deadline::Free` for them when no response timeout is configured
  - command: `cargo test -p camel-component-redis --lib blocking_endpoint_includes_new_blocking_commands`
  - expected: fails before step 4, passes after

**Acceptance:**
- tests pass; `cargo test -p camel-component-redis --lib` exits 0 (family executors may still reject the new variants at this point — dispatch arms route them, `is_*_command` gates reject until Tasks 2.3-2.6 land)
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 2.1

### Task 2.2: mod.rs — strict multi-key resolver

**Files:**
- `crates/components/camel-redis/src/commands/mod.rs` (modified)

**Steps:**
1. Add `pub(crate) fn require_key_list(exchange: &Exchange, verb: &str) -> Result<Vec<String>, CamelError>`: reads `CamelRedis.Keys` and fails closed naming `CamelRedis.Keys` (plus `verb`) when: the header is absent; it is not a JSON array (e.g. a bare string); the array is empty; or any entry is not a JSON string — the entry error names the offending index and echoes a bounded preview of the value. Nothing is silently dropped (this replaces `get_str_vec_header` at every new multi-key consumer; the existing `MGET` resolver is grandfathered and stays as-is).
2. Add the unit tests below in `mod.rs`'s test module.

**Tests:**
- name: `require_key_list_strict_domains`
  - setup: exchanges with `Keys=["a","b"]`, `Keys=[]`, `Keys="a"`, `Keys=["a", 42]`, `Keys=["a", null]`, no header
  - action: call `require_key_list(&ex, "LMPOP")`
  - assert: `["a","b"]` resolves; every other case errors with a message containing `CamelRedis.Keys` and `LMPOP`; the index error names index `1` and the offending value for both malformed cases
  - command: `cargo test -p camel-component-redis --lib require_key_list_strict_domains`
  - expected: fails before step 1, passes after

**Acceptance:**
- test passes; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 2.2

### Task 2.3: list.rs — LMPOP, BLMPOP, LMOVE, BLMOVE, LPOS

**Files:**
- `crates/components/camel-redis/src/commands/list.rs` (modified)

**Steps:**
1. Add `pub(crate) fn resolve_pop_direction(exchange: &Exchange) -> Result<&'static str, CamelError>`: reads `CamelRedis.Where` case-insensitively; `LEFT`/`RIGHT` map to the static tokens; missing or invalid → named error naming `CamelRedis.Where` and the valid values.
2. Add `pub(crate) fn resolve_lmove_directions(exchange: &Exchange) -> Result<(&'static str, &'static str), CamelError>`: reads `CamelRedis.WhereFrom` and `CamelRedis.WhereTo` (both `LEFT`/`RIGHT`, both required); missing → named error naming the missing header; invalid → named error naming the header and valid values.
3. Add `pub(crate) fn resolve_pop_count_positive(exchange: &Exchange) -> Result<Option<u64>, CamelError>`: optional `CamelRedis.Count` via `get_u64_header`; present-but-zero or present-but-garbage → named error naming `CamelRedis.Count` and the positive requirement.
4. Add `pub(crate) fn resolve_lpos_args(exchange: &Exchange) -> Result<(Option<i64>, Option<i64>), CamelError>`: `CamelRedis.Rank` via `get_i64_header`, zero → named error naming `CamelRedis.Rank` and the non-zero requirement (any positive or negative rank valid); `CamelRedis.Count` via `get_i64_header`, negative → named error naming `CamelRedis.Count` and the non-negative requirement (0 = all matches).
5. Extend `is_list_command` with `Lmpop`, `Blmpop`, `Lmove`, `Blmove`, `Lpos`.
6. Executors in `dispatch`:
   - `Lmpop`: `require_key_list` + direction + count; wire `LMPOP <n> <keys...> LEFT|RIGHT [COUNT n]`; reply nil-or-`[key, [elements]]`: all-UTF-8 → JSON null/array body; any binary element → RESP-encoded `Bytes` body via a new `encode_lmpop_reply_bytes` mirroring `encode_mget_reply_bytes` for the nested shape.
   - `Blmpop`: same + leading `CamelRedis.Timeout` seconds via the existing `resolve_blocking_timeout` (missing/0 = block forever): `BLMPOP <timeout> <n> <keys...> LEFT|RIGHT [COUNT n]`.
   - `Lmove`: `require_key` + `require_str_header(exchange, "CamelRedis.Destination")` + directions; `LMOVE src dst LEFT|RIGHT LEFT|RIGHT`; reply follows the `GET` reply rule via `reply_to_body` (single bulk string).
   - `Blmove`: same + `resolve_blocking_timeout` appended last: `BLMOVE src dst LEFT|RIGHT LEFT|RIGHT <timeout>`.
   - `Lpos`: `require_key`, element from the body via `body_value_bytes(exchange, "LPOS", &[HEADER_VALUE])` (no element header exists), rank/count args when present: `LPOS k <element> [RANK n] [COUNT n]`; reply integer or array of integers as JSON.
7. Add the unit tests below.

**Tests:**
- name: `resolve_pop_direction_domains`
  - setup: exchanges with `Where=LEFT`, `Where=right`, `Where=UP`, missing
  - action: call `resolve_pop_direction`
  - assert: `LEFT` and lowercase `right` map; `UP` and missing error naming `CamelRedis.Where` and both valid values
  - command: `cargo test -p camel-component-redis --lib resolve_pop_direction_domains`
  - expected: fails before step 1, passes after
- name: `resolve_lmove_directions_domains`
  - setup: exchanges with both valid, `WhereFrom=CENTER`, missing `WhereTo`
  - action: call `resolve_lmove_directions`
  - assert: valid pair maps to `("LEFT","RIGHT")`; each bad case errors naming the offending header
  - command: `cargo test -p camel-component-redis --lib resolve_lmove_directions_domains`
  - expected: fails before step 2, passes after
- name: `resolve_lpos_args_domains`
  - setup: exchanges with `Rank=0`, `Count=-1`, `Rank=-2`+`Count=0`, no headers
  - action: call `resolve_lpos_args`
  - assert: `Rank=0` errors (non-zero requirement); `Count=-1` errors (non-negative requirement); `-2`/`0` resolve; absent headers resolve to `(None, None)`
  - command: `cargo test -p camel-component-redis --lib resolve_lpos_args_domains`
  - expected: fails before step 4, passes after
- name: `resolve_pop_count_positive_domain`
  - setup: exchanges with `Count=2`, `Count=0`, `Count=-1`, no header
  - action: call `resolve_pop_count_positive`
  - assert: 2 resolves to `Some(2)`; 0 and -1 error naming `CamelRedis.Count` and the positive requirement; absent resolves `None`
  - command: `cargo test -p camel-component-redis --lib resolve_pop_count_positive_domain`
  - expected: fails before step 3, passes after
- name: `lpos_body_forms_fail_closed_or_compact`
  - setup: exchanges with body `Body::Empty`, `Body::Stream(..)`, `Body::Json(null)`, `Body::Text("")`, `Body::Json({"a":1})`, all with `CamelRedis.Key=k`
  - action: resolve the LPOS element through `body_value_bytes(exchange, "LPOS", &[HEADER_VALUE])`
  - assert: the first three error naming `LPOS` and the payload body rule; `Text("")` resolves to an empty bulk argument; the JSON object resolves via compact serialization
  - command: `cargo test -p camel-component-redis --lib lpos_body_forms_fail_closed_or_compact`
  - expected: fails before step 6, passes after
- name: `lmpop_wire_args_assembly`
  - setup: exchange with `Keys=["a","b"]`, `Where=LEFT`, `Count=2`
  - action: build the LMPOP argument list the way the executor does (extract the arg-assembly into a testable helper, e.g. `fn lmpop_args(keys: &[String], direction: &str, count: Option<u64>) -> Vec<String>` returning the arguments in wire order as string renderings)
  - assert: args equal `["2","a","b","LEFT","COUNT","2"]`; without count `["2","a","b","LEFT"]`
  - command: `cargo test -p camel-component-redis --lib lmpop_wire_args_assembly`
  - expected: fails before step 6, passes after

**Acceptance:**
- tests pass; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 2.3

### Task 2.4: set.rs — SMISMEMBER, SINTERCARD

**Files:**
- `crates/components/camel-redis/src/commands/set.rs` (modified)

**Steps:**
1. Add `pub(crate) fn resolve_members_list(exchange: &Exchange) -> Result<Vec<String>, CamelError>`: strict read of `CamelRedis.Members` (same validation shape as `require_key_list`: non-empty array of strings, malformed entry names header + index + preview); missing/invalid → named error naming `CamelRedis.Members`.
2. Add `pub(crate) fn resolve_sintercard_limit(exchange: &Exchange) -> Result<Option<i64>, CamelError>`: optional `CamelRedis.Limit` via `get_i64_header`; negative → named error naming `CamelRedis.Limit` and the non-negative requirement.
3. Extend `is_set_command` with `Smismember`, `Sintercard`.
4. Executors: `Smismember` — `require_key` + members; wire `SMISMEMBER k <members...>`; reply array of 1/0 → JSON boolean array. `Sintercard` — `require_key_list` + limit; wire `SINTERCARD <n> <keys...> [LIMIT n]`; reply integer → JSON.
5. Add the unit tests below.

**Tests:**
- name: `resolve_members_list_strict`
  - setup: exchanges with `Members=["a","b"]`, `Members=[]`, `Members="a"`, `Members=["a", 7]`
  - action: call `resolve_members_list`
  - assert: valid resolves; every bad case errors naming `CamelRedis.Members`; the entry error names index `1`
  - command: `cargo test -p camel-component-redis --lib resolve_members_list_strict`
  - expected: fails before step 1, passes after
- name: `resolve_sintercard_limit_domain`
  - setup: exchanges with `Limit=10`, `Limit=-1`, no header
  - action: call `resolve_sintercard_limit`
  - assert: 10 resolves; -1 errors naming `CamelRedis.Limit` and the non-negative requirement; absent resolves `None`
  - command: `cargo test -p camel-component-redis --lib resolve_sintercard_limit_domain`
  - expected: fails before step 2, passes after

**Acceptance:**
- tests pass; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 2.4

### Task 2.5: zset.rs — ZMPOP, BZMPOP, ZRANDMEMBER, ZDIFF, ZINTER, ZUNION, ZRANGEBYLEX

**Files:**
- `crates/components/camel-redis/src/commands/zset.rs` (modified)

**Steps:**
1. Add `pub(crate) fn resolve_order_by(exchange: &Exchange) -> Result<&'static str, CamelError>`: reads `CamelRedis.OrderBy` case-insensitively; `MIN`/`MAX` map to static tokens; missing/invalid → named error naming `CamelRedis.OrderBy` and both valid values (sorted-set pops order by score, not list ends — design.md D-header note).
2. Add `pub(crate) fn resolve_optional_count(exchange: &Exchange) -> Result<Option<i64>, CamelError>`: optional `CamelRedis.Count` via `get_i64_header`, any i64 accepted (ZRANDMEMBER semantics; negative = repeats); garbage → named error naming `CamelRedis.Count`.
3. Extend `is_zset_command` with `Zmpop`, `Bzmpop`, `Zrandmember`, `Zdiff`, `Zinter`, `Zunion`, `Zrangebylex`.
4. Executors:
   - `Zmpop`: `require_key_list` + `resolve_order_by` + positive `Count` (reuse Task 2.3's `resolve_pop_count_positive`); wire `ZMPOP <n> <keys...> MIN|MAX [COUNT n]`; reply nil-or-`[key, [[member, score], ...]]` → JSON null/array, binary members via a new `encode_zmpop_reply_bytes` (RESP-encoded, mirroring `encode_mget_reply_bytes`).
   - `Bzmpop`: same + leading `resolve_blocking_timeout`: `BZMPOP <timeout> <n> <keys...> MIN|MAX [COUNT n]` (missing/0 = block forever; absent Count sends `COUNT 1`).
   - `Zrandmember`: `require_key` + `resolve_optional_count`; wire `ZRANDMEMBER k [count]`; without count the single-member reply follows the `GET` reply rule (`reply_to_body`); with count the array follows the `MGET` binary rule (reuse/extend `encode_mget_reply_bytes`).
   - `Zdiff`/`Zinter`/`Zunion`: `require_key_list`; wire `<CMD> <n> <keys...>`; reply member array → JSON per the `MGET` binary rule (no `WEIGHTS`/`AGGREGATE` arguments exist in this change).
   - `Zrangebylex`: `require_key` + `CamelRedis.Min`/`CamelRedis.Max` via `require_str_header` (missing bound → named error naming the missing header); wire `ZRANGEBYLEX k <min> <max>`; reply member array per the binary rule.
5. Add the unit tests below.

**Tests:**
- name: `resolve_order_by_domains`
  - setup: exchanges with `OrderBy=MIN`, `OrderBy=max`, `OrderBy=LEFT`, missing
  - action: call `resolve_order_by`
  - assert: `MIN`/lowercase `max` map; `LEFT` and missing error naming `CamelRedis.OrderBy` and `MIN`/`MAX`
  - command: `cargo test -p camel-component-redis --lib resolve_order_by_domains`
  - expected: fails before step 1, passes after
- name: `zmpop_wire_args_assembly`
  - setup: keys `["a","b"]`, `OrderBy=MAX`, `Count=2` (and a no-count variant)
  - action: build the args list the executor assembles (extract a testable helper if needed)
  - assert: `["2","a","b","MAX","COUNT","2"]`; without count `["2","a","b","MAX"]`; BZMPOP variant with timeout 0 → `["0","2","a","b","MIN","COUNT","1"]`
  - command: `cargo test -p camel-component-redis --lib zmpop_wire_args_assembly`
  - expected: fails before step 4, passes after
- name: `zrangebylex_requires_both_bounds`
  - setup: exchanges with `Min=[a` only, `Max=+` only, both set
  - action: resolve the bounds via `require_str_header` for `CamelRedis.Min`/`CamelRedis.Max`
  - assert: each missing bound errors naming that header; both set resolve
  - command: `cargo test -p camel-component-redis --lib zrangebylex_requires_both_bounds`
  - expected: fails before step 4, passes after
- name: `zset_multi_key_requires_key_list`
  - setup: exchange with `Keys=[]`
  - action: call `require_key_list(&ex, "ZMPOP")` (shared resolver from Task 2.2)
  - assert: errors naming `CamelRedis.Keys` and `ZMPOP`
  - command: `cargo test -p camel-component-redis --lib zset_multi_key_requires_key_list`
  - expected: passes only after Task 2.2 + step 4 wiring

**Acceptance:**
- tests pass; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 2.5

### Task 2.6: hash.rs — HRANDFIELD, HINCRBYFLOAT

**Files:**
- `crates/components/camel-redis/src/commands/hash.rs` (modified)

**Steps:**
1. Add `pub(crate) fn resolve_float_increment(exchange: &Exchange) -> Result<f64, CamelError>`: reads `CamelRedis.Increment` via `get_f64_header`; missing → named error naming `CamelRedis.Increment`; garbage → named error naming `CamelRedis.Increment` and echoing the received value (reuse the `invalid_numeric_header` shape). HINCRBYFLOAT is the only consumer; the integer `INCRBY` resolver is untouched.
2. Extend `is_hash_command` with `Hrandfield`, `Hincrbyfloat`.
3. Executors: `Hrandfield` — `require_key` + `resolve_optional_count` (Task 2.5's helper); wire `HRANDFIELD k [count]`; without count the single-member reply follows the `GET` reply rule; with count the array follows the `MGET` binary rule. `Hincrbyfloat` — `require_key` + `require_str_header(exchange, "CamelRedis.Field")` + `resolve_float_increment`; wire `HINCRBYFLOAT k <field> <increment>` (render the float losslessly, e.g. via the shortest round-trip form the existing float formatting uses); reply bulk-string float → JSON string body.
4. Add the unit tests below.

**Tests:**
- name: `resolve_float_increment_domains`
  - setup: exchanges with `Increment=1.5`, `Increment=2`, `Increment=abc`, no header
  - action: call `resolve_float_increment`
  - assert: `1.5` and integer-string `2` resolve; `abc` and missing error naming `CamelRedis.Increment` (`abc` case echoes the value)
  - command: `cargo test -p camel-component-redis --lib resolve_float_increment_domains`
  - expected: fails before step 1, passes after

**Acceptance:**
- test passes; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 2.6

### Task 2.7: Phase 2 wire tests + blocking-bypass tests

**Files:**
- `crates/components/camel-redis/src/value_fake_tests.rs` (modified)
- `crates/components/camel-redis/src/blocking_pop_tests.rs` (modified)
- `crates/components/camel-redis/src/stream_fake_tests.rs` (modified — `connection_count` to `pub(crate)`)

**Steps:**
1. In `value_fake_tests.rs`, add wire tests using the existing `StreamFakeServer` + `call_producer` + `args_of` + `resp_*` helpers (use `resp_bulk_bytes` for binary bulk replies — `resp_bulk` takes `&str`), one test per command covering all sixteen Phase 2 commands (fifteen fns; `zdiff_zinter_zunion_frames` covers three), each asserting the exact frame `args_of` captured:
   - `lmpop_frame_and_reply_shape`: `Keys=["a","b"]`, `Where=LEFT`, `Count=2` → `["2","a","b","LEFT","COUNT","2"]`; scripted reply `resp_array(&[resp_bulk("a"), resp_array(&[resp_bulk("x"), resp_bulk("y")])])`; assert the JSON body `["a",["x","y"]]`.
   - `blmpop_frame_with_timeout`: `Timeout=5`, `Count=2` → `["5","2","a","b","LEFT","COUNT","2"]`; `Timeout=0` variant → leading `"0"`.
   - `lmove_frame_and_element_reply` / `blmove_frame`: `["src","dst","LEFT","RIGHT"]` (+ trailing `"1"` for BLMOVE); reply `resp_bulk("x")` → body carries `x`.
   - `lpos_frame_from_body`: body `x`, `Rank=1`, `Count=3` → `["k","x","RANK","1","COUNT","3"]`; no-rank/no-count variant → `["k","x"]`; a `Count=0` variant asserting the exact frame `["k","x","COUNT","0"]` (the blessed "COUNT 0 means all matches" scenario).
   - `smismember_frame_and_bool_reply`: `Members=["a","b"]` → `["k","a","b"]`; reply `resp_array(&[resp_int(1), resp_int(0)])` → body `[true,false]`.
   - `sintercard_frame_with_and_without_limit`: `["2","a","b","LIMIT","10"]` / `["2","a","b"]`.
   - `zmpop_frame_and_reply_shape`: `OrderBy=MAX`, `Count=2` → `["2","a","b","MAX","COUNT","2"]`; reply `resp_array(&[resp_bulk("a"), resp_array(&[resp_array(&[resp_bulk("m"), resp_bulk("1.5")])])])` → JSON `["a",[["m",1.5]]]`.
   - `bzmpop_frame`: `Timeout=0`, `OrderBy=MIN` → `["0","2","a","b","MIN","COUNT","1"]`; a `Timeout=0` + `Count=2` variant asserting `["0","2","a","b","MIN","COUNT","2"]` (the blessed explicit-count scenario).
   - `zrandmember_frames_and_replies`: no count → `["k"]` with `resp_bulk("m")` → body carries `m`; `Count=-5` → `["k","-5"]`.
   - `zdiff_zinter_zunion_frames`: `["2","a","b"]` / `["3","a","b","c"]` / `["2","a","b"]`; member-array reply → JSON array.
   - `zrangebylex_frame`: `Min=[a`, `Max=+` → `["k","[a","+"]`.
   - `hrandfield_frame`: no count `["k"]`; `Count=3` → `["k","3"]`.
   - `hincrbyfloat_frame_and_float_reply`: `Field=balance`, `Increment=1.5` → `["k","balance","1.5"]`; reply `resp_bulk("2.5")` → body `"2.5"`.
   - `lmpop_empty_keys_fails_before_send`: `Keys=[]` → call errors and `args_of("LMPOP")` is empty.
2. In `blocking_pop_tests.rs`, add a producer-side section (new `mod` or clearly-commented block) reusing the `StreamFakeServer` delayed-release scripted action (`DeferredOn`, the read-record-release variant) to hold each reply past the redis-rs default 500 ms response timeout:
   - `blmpop_bypasses_driver_response_deadline`: hold the reply ~700 ms, then release; assert the producer call completes with the programmed reply and the server saw exactly one connection.
   - `bzmpop_bypasses_driver_response_deadline`: same shape for `BZMPOP`.
   - `blmove_bypasses_driver_response_deadline`: same shape for `BLMOVE`.
   - Assert single-connection directly: widen `StreamFakeServer::connection_count` from module-private to `pub(crate)` in `stream_fake_tests.rs` (one-line change; list `stream_fake_tests.rs` as modified in Files) so the test can call it — do not fall back to frame-only assertions.
   - Keep the delay inside the server helper/closure (the existing pattern in this file): a `tokio::time::sleep` directly in a test body adds a counted site and trips the `lint-test-sleep` ratchet ceiling.
   - Do NOT put the producer-side tests through `queue_session`/`QueuePopCommand` — these are producer endpoints.

**Tests:**
- the wire fns from step 1 and the three bypass fns from step 2
  - setup: scripted servers as listed
  - action: `call_producer` per wire case; delayed-release producer calls for the bypass cases
  - assert: exact frames per the spec scenarios; bypass tests complete with the delayed reply on one connection
  - command: `cargo test -p camel-component-redis --lib lmpop_frame_and_reply_shape` (and each sibling), `cargo test -p camel-component-redis --lib blmpop_bypasses_driver_response_deadline` (and siblings)
  - expected: wire tests fail before Tasks 2.3-2.6, pass after; bypass tests fail before Task 2.1 step 4 (500 ms driver timeout cuts the reply / reconnect appears), pass after

**Acceptance:**
- all added tests pass; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 2.7

### Task 2.8: Phase 2 docs — redis.md

**Files:**
- `docs/src/components/redis.md` (modified)

**Steps:**
1. Add one row per command to the command table: `LMPOP`, `BLMPOP`, `LMOVE`, `BLMOVE`, `LPOS`, `SMISMEMBER`, `SINTERCARD`, `ZMPOP`, `BZMPOP`, `ZRANDMEMBER`, `ZDIFF`, `ZINTER`, `ZUNION`, `ZRANGEBYLEX`, `HRANDFIELD`, `HINCRBYFLOAT` — matching the existing column format.
2. Extend the `### Blocking pops` section: `BLMPOP`, `BZMPOP`, and `BLMOVE` join `BLPOP`/`BRPOP` as blocking endpoints with no driver response deadline; the blocking `CamelRedis.Timeout` follows the pop convention (missing or 0 blocks forever).
3. Note in the sorted-set row/section that `ZDIFF`/`ZINTER`/`ZUNION` ship without `WEIGHTS`/`AGGREGATE` (deferred per design.md D4; the bd follow-up is filed in Task 3.5).
4. Run `cd docs && mdbook test`.

**Tests:**
- name: (mdbook doctest suite)
  - setup: docs as edited
  - action: `cd docs && mdbook test`
  - assert: exit 0
  - command: `cd docs && mdbook test`
  - expected: passes after steps 1-3

**Acceptance:**
- `cd docs && mdbook test` exits 0
- all sixteen rows present (`grep -c "LMPOP\|BLMPOP\|LMOVE\|BLMOVE\|LPOS\|SMISMEMBER\|SINTERCARD\|ZMPOP\|BZMPOP\|ZRANDMEMBER\|ZDIFF\|ZINTER\|ZUNION\|ZRANGEBYLEX\|HRANDFIELD\|HINCRBYFLOAT" docs/src/components/redis.md` ≥ 16)
- the blocking-pops section names all five blocking commands

- [x] 2.8

## Phase 3: generics and close-out

### Task 3.1: config.rs + executor.rs — UNLINK, COPY, TOUCH

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)

**Steps:**
1. Add variants `Unlink`, `Copy`, `Touch` with `FromStr` arms (`"UNLINK"`, `"COPY"`, `"TOUCH"`) and `as_str` arms (the method introduced in Task 1.1).
2. In `is_idempotent_command`: add `Unlink` and `Touch` to the true arms (`Unlink` under "Idempotent writes" with `// mirrors DEL: a retry converges, the count reply 1→0 is the accepted deviation`; `Touch` under reads with `// TOUCH only refreshes idle time`). Do NOT add `Copy`; comment: `// COPY is absent: without REPLACE a retry reports 0 after an unconfirmed success; with REPLACE the retry re-reads a possibly-changed source.`
3. In `executor.rs`, route `Unlink | Copy | Touch` → `commands::key::dispatch`.
4. Extend the classification unit test.

**Tests:**
- name: `generic_key_commands_parse_and_classify`
  - setup: none
  - action: parse + classify `UNLINK`/`COPY`/`TOUCH`
  - assert: round-trip holds; `Unlink`/`Touch` idempotent true; `Copy` false
  - command: `cargo test -p camel-component-redis --lib generic_key_commands_parse_and_classify`
  - expected: fails before steps 1-2, passes after

**Acceptance:**
- test passes; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 3.1

### Task 3.2: key.rs — UNLINK, COPY, TOUCH executors

**Files:**
- `crates/components/camel-redis/src/commands/key.rs` (modified)

**Steps:**
1. Extend `is_key_command` (the file's family matcher) with `Unlink`, `Copy`, `Touch`.
2. Executors:
   - `Unlink`: `require_key`; wire `UNLINK k` (mirror the existing `Del` arm); reply integer → JSON.
   - `Copy`: `require_key` + `require_str_header(exchange, "CamelRedis.Destination")`; `CamelRedis.Replace` via `require_bool_header` (malformed → named error); `CamelRedis.Db` optional via `get_u64_header` + `u64_to_i64_checked` — absent resolves to `None` and NO `DB` argument is sent (the current database; do NOT reuse `resolve_move_db`, which defaults absent Db to 0 and would violate the blessed frame `COPY src dst`); wire `COPY src dst [DB n] [REPLACE]` in that order; reply 0/1 → JSON boolean.
   - `Touch`: `require_key_list` (Task 2.2's strict resolver); wire `TOUCH <keys...>`; reply integer → JSON.
3. Add the unit tests below.

**Tests:**
- name: `copy_arg_resolution_domains`
  - setup: exchanges with full options (`Replace=true`, `Db=3`), minimal (no optionals), missing `Destination`, `Replace="sometimes"`
  - action: resolve the COPY arguments (extract a testable `resolve_copy_args` helper if needed)
  - assert: full → `(src, dst, Some(3), true)`; minimal → no DB, no REPLACE; missing destination errors naming `CamelRedis.Destination`; malformed Replace errors naming `CamelRedis.Replace`
  - command: `cargo test -p camel-component-redis --lib copy_arg_resolution_domains`
  - expected: fails before step 2, passes after
- name: `touch_requires_key_list`
  - setup: exchange with `Keys=[]`
  - action: call `require_key_list(&ex, "TOUCH")`
  - assert: errors naming `CamelRedis.Keys` and `TOUCH`
  - command: `cargo test -p camel-component-redis --lib touch_requires_key_list`
  - expected: fails before step 2 wiring, passes after

**Acceptance:**
- tests pass; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 3.2

### Task 3.3: Phase 3 wire tests

**Files:**
- `crates/components/camel-redis/src/value_fake_tests.rs` (modified)

**Steps:**
1. Add three wire tests, scripted replies and exact `args_of` assertions:
   - `unlink_frame_and_count_reply`: `["k"]`, reply `resp_int(1)` → body `1`.
   - `copy_frames_with_and_without_options`: `["src","dst","DB","3","REPLACE"]` and bare `["src","dst"]`; reply `resp_int(1)` → body `true`.
   - `touch_frame_and_count_reply`: `Keys=["a","b"]` → `["a","b"]`, reply `resp_int(2)` → body `2`.
   - `copy_missing_destination_fails_before_send`: no `CamelRedis.Destination` → call errors, `args_of("COPY")` empty.

**Tests:**
- the four fns from step 1
  - setup: scripted servers
  - action: `call_producer` per case
  - assert: exact frames and reply bodies
  - command: `cargo test -p camel-component-redis --lib unlink_frame_and_count_reply` (and siblings)
  - expected: fails before Tasks 3.1-3.2, passes after

**Acceptance:**
- tests pass; `cargo test -p camel-component-redis --lib` exits 0
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exit 0

- [x] 3.3

### Task 3.4: Phase 3 docs + full-table completeness

**Files:**
- `docs/src/components/redis.md` (modified)

**Steps:**
1. Add rows for `UNLINK`, `COPY`, `TOUCH` to the command table.
2. Completeness sweep: verify every command shipped by this change has a docs row (23 commands: the Phase 1 four + Phase 2 sixteen + these three) and that the `SET` retry-reclassification note and blocking-pops section are accurate against the final code (`is_idempotent_command`, `is_blocking_endpoint`).
3. Run `cd docs && mdbook test`.

**Tests:**
- name: (mdbook doctest suite)
  - setup: docs as edited
  - action: `cd docs && mdbook test`
  - assert: exit 0
  - command: `cd docs && mdbook test`
  - expected: passes after steps 1-2

**Acceptance:**
- `cd docs && mdbook test` exits 0
- all 23 new commands have rows; grep over `docs/src/components/redis.md` finds each token
- the docs `SET` note matches the final `is_idempotent_command` (Set false) and the blocking list matches `is_blocking_endpoint` (five commands)

- [x] 3.4

### Task 3.5: close-out — spec validation and archive check

**Files:**
- none (verification only)

**Steps:**
1. Run `openspec validate rediscmds --type change` from the worktree root; fix any delta-structure error it reports (loop back through the owning task if a fix is needed).
2. Run `cargo xtask archive rediscmds --check` from the worktree root; it must exit 0.
3. Compile the downstream consumers with the new endpoint surface (mission gates): `cargo check -p camel-test --features integration-tests --tests` and `cargo check -p camel-cli --tests`; both must exit 0.
4. Write `.closeout.md` inside `openspec/changes/rediscmds/` recording: the exit code and output tail of every mission gate you ran (`cargo fmt --check --all`, `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings`, `cargo test -p camel-component-redis --lib`, the two `cargo check` gates, `cd docs && mdbook test`, `bash scripts/gates-local.sh`), `cargo xtask schema --check` marked `N/A — no schema files moved` (this change touches no `schema*` file), the `git diff --stat` vs the merge-base with `main`, the option-surface decision table from design.md D1, and the final per-command `is_idempotent_command` classification table (all 23 new commands plus the `Set` reclassification). The merge gate itself (`scripts/gates-local.sh`) must exit 0 here; if a pre-existing failure unrelated to this change blocks it, say so explicitly in `.closeout.md` instead of claiming green.
5. File the deferred-feature follow-up from the repo root (NOT the worktree): `(cd /home/kenny/dev/rust-camel && bd create "redis: ZDIFF/ZINTER/ZUNION WEIGHTS and AGGREGATE options" --description="Deferred from rc-tic86 (design.md D4): ZMPOP-family multi-key set ops shipped without WEIGHTS/AGGREGATE, keeping parity with ZUNIONSTORE/ZINTERSTORE which also lack them. Add both options when a consumer needs weighted/aggregate set combinations." -t feature -p 3 --deps discovered-from:rc-tic86 --json)`; record the returned id in the docs row of Task 2.8 step 3 if it lands after it.
6. Final audit vs design.md: `is_idempotent_command` covers exactly the design D3 table (reads + `Setrange`/`Unlink`/`Touch` true; `Set`, `Getdel`, `Getex`, `Copy`, `Hincrbyfloat`, pop/move family false); `is_blocking_endpoint` names exactly `Blpop`, `Brpop`, `Blmove`, `Blmpop`, `Bzmpop`; report any drift by fixing the owning code, not the design.
7. Confirm `grep -rn "RedisCommand::" crates/components/camel-redis/src/` shows no match site missing the new variants (all exhaustive matches compile under clippy `-D warnings`, which is the mechanical proof).

**Tests:**
- name: (verification commands)
  - setup: full implementation landed
  - action: run the commands in steps 1-5
  - assert: validate + archive check + both cargo checks + `scripts/gates-local.sh` exit 0; the bd follow-up exists with a `discovered-from:rc-tic86` link; `.closeout.md` exists with the gate table, diffstat, option-surface table, and classification table
  - command: `openspec validate rediscmds --type change && cargo xtask archive rediscmds --check && cargo check -p camel-test --features integration-tests --tests && cargo check -p camel-cli --tests && bash scripts/gates-local.sh`
  - expected: passes when all prior tasks are complete

**Acceptance:**
- all five verification commands exit 0
- `.closeout.md` present with gate table, diffstat, option-surface decision table, per-command idempotency classification, and the schema N/A note
- the bd WEIGHTS/AGGREGATE follow-up exists, linked `discovered-from:rc-tic86`
- no drift between design.md D3/D2 tables and the code

- [x] 3.5
