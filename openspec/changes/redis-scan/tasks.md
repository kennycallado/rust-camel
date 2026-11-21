# Tasks: redis-scan

Single-phase change (no phase grouping; design.md has no `## Phases`).

## camel-component-redis

### Task 1.1: Shared scan-page plumbing (encode, parse, resolve)

**Files:**
- `crates/components/camel-redis/src/commands/scan.rs` (new)
- `crates/components/camel-redis/src/commands/mod.rs` (modified — add `pub mod scan;`)

**Steps:**
1. Create `commands/scan.rs` with `pub fn encode_scan_page(verb: ScanVerb, key: Option<&str>, cursor: u64, pattern: &str, count: u64, scan_type: Option<&str>) -> redis::Cmd` where `ScanVerb` is a new `pub enum ScanVerb { Scan, Sscan, Hscan, Zscan }` rendering `"SCAN"|"SSCAN"|"HSCAN"|"ZSCAN"`; the frame is always `<VERB> [<key> ]<cursor> MATCH <pattern> COUNT <count>[ TYPE <type>]` (key present only for the collection variants).
2. Add `pub fn parse_scan_reply<T: redis::FromRedisValue>(reply: redis::Value, verb_name: &str) -> Result<(u64, Vec<T>), CamelError>` that parses the two-element `[cursor, elements]` reply (map parse errors to `CamelError::Io` naming the verb, mirroring `camel-redis-repo/src/executor.rs:158`'s `from_redis_value` usage).
3. Add `pub fn resolve_scan_cursor(exchange: &Exchange) -> Result<u64, CamelError>`: absent header, JSON `null`, or an all-whitespace string → `Ok(0)`; otherwise delegate to the existing `get_u64_header` (commands/mod.rs) so surrounding whitespace trims (`" 17 "` → 17) and malformed values yield the named `invalid_numeric_header` error naming `CamelRedis.Cursor`.
4. Add `pub fn resolve_scan_options(exchange: &Exchange, allow_type: bool) -> Result<(String, u64, Option<String>), CamelError>` returning (pattern, count, type): pattern = `resolve_keys_pattern` precedent (`CamelRedis.Pattern`, default `"*"`); count = `get_u64_header(exchange, "CamelRedis.Count")` defaulting to `const SCAN_PAGE_COUNT: u64 = 100`, and `0` → named `ProcessorError` naming `CamelRedis.Count`; type: check PRESENCE with `exchange.input.header("CamelRedis.Type").is_some()` (never `get_str_header`, which conflates presence with stringness — a present non-string value must not silently vanish) — absent → `Ok(None)`; present with `allow_type == false` → named `ProcessorError` naming `CamelRedis.Type` before any stringness check; present with `allow_type == true` → strict string resolution (`as_str()` → `Ok(Some(s.to_string()))`, any non-string value → named `ProcessorError` naming `CamelRedis.Type` and echoing a bounded preview via `invalid_header_value_preview`).
5. Add `mod tests` covering the pure functions (Tests below); wire `pub mod scan;` into `commands/mod.rs`.

**Tests:**
- `encode_scan_page_scan_start_frame`: no setup → `encode_scan_page(ScanVerb::Scan, None, 0, "*", 100, None)` → `cmd_args`-style assertion the frame is exactly `["SCAN","0","MATCH","*","COUNT","100"]` (reuse the flattened-args pattern from `camel-redis-repo/src/executor.rs::test_support::cmd_args`, in-crate copy or equivalent `args_iter` walk).
- `encode_scan_page_sscan_with_all_options`: → `encode_scan_page(ScanVerb::Sscan, Some("s"), 0, "*", 100, None)` → frame exactly `["SSCAN","s","0","MATCH","*","COUNT","100"]` (pins the blessed scenario "explicit cursor 0 is a fresh iteration" verbatim; a non-zero cursor round-trip is covered by `resolve_scan_cursor_trims_and_parses` and the live lanes).
- `encode_scan_page_scan_explicit_match_count`: → `encode_scan_page(ScanVerb::Scan, None, 17, "a*", 500, None)` → frame exactly `["SCAN","17","MATCH","a*","COUNT","500"]` (pins the opaque-cursor round-trip frame and explicit MATCH/COUNT on the wire).
- `encode_scan_page_scan_type_trailing`: → `encode_scan_page(ScanVerb::Scan, None, 0, "*", 100, Some("set"))` → frame ends `["TYPE","set"]`.
- `parse_scan_reply_keys_page`: `redis::Value::Array([BulkString("17"), Array([BulkString(a), BulkString(b)])])` → `parse_scan_reply::<String>` → `(17, vec!["a","b"])`.
- `parse_scan_reply_hscan_flat_pairs`: two-element reply with flat `[f1,v1,f2,v2]` BulkStrings → `parse_scan_reply::<(String,String)>` → `[(f1,v1),(f2,v2)]`.
- `parse_scan_reply_zscan_pairs`: flat `[m1,"1.5",m2,"2"]` → `parse_scan_reply::<(String,f64)>` → `[(m1,1.5),(m2,2.0)]`.
- `parse_scan_reply_malformed_is_named_error`: `redis::Value::Int(1)` → `parse_scan_reply::<String>` → `Err(CamelError::Io(msg))` with msg containing the verb name passed in.
- `resolve_scan_cursor_absent_null_empty_and_zero`: four exchanges (no header; `json!(null)`; `json!("")`; `json!("  ")`) → each resolves `Ok(0)`; plus `json!(0)` → `Ok(0)`.
- `resolve_scan_cursor_trims_and_parses`: `json!(" 17 ")` → `Ok(17)`; `json!(918)` → `Ok(918)`.
- `resolve_scan_cursor_malformed_names_header`: `json!("abc")` and `json!(-1)` → `Err` whose message contains `CamelRedis.Cursor`.
- `resolve_scan_options_defaults`: no headers → `("*", 100, None)`.
- `resolve_scan_options_count_zero_and_malformed_named`: `CamelRedis.Count=0` and `CamelRedis.Count="abc"` → both `Err` naming `CamelRedis.Count`.
- `resolve_scan_options_type_rejected_on_collection_variants`: `CamelRedis.Type="hash"` with `allow_type=false` → `Err` naming `CamelRedis.Type`; `CamelRedis.Type=json!(true)` (present, non-string) with `allow_type=false` → `Err` naming `CamelRedis.Type` (presence check fires regardless of stringness); `CamelRedis.Type=json!(true)` with `allow_type=true` → `Err` naming `CamelRedis.Type` (strict string resolution); with `allow_type=true` and `"set"` → `Ok(…, Some("set"))`.
- `parse_scan_reply_empty_page`: two-element reply with cursor `"918"` and an empty element array → `parse_scan_reply::<String>` → `(918, vec![])` (pins the empty-page continuation input).

**Acceptance:**
- `cargo test -p camel-component-redis commands::scan` passes (all listed tests).
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exit 0.
- No `unwrap()` outside tests (`cargo xtask lint-unwrap` exit 0).

- [x] 1.1

### Task 1.2: SCAN family commands wired into the component

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified — 4 `RedisCommand` variants, `FromStr`, `as_str`, idempotency classifier)
- `crates/components/camel-redis/src/executor.rs` (modified — routing arm)
- `crates/components/camel-redis/src/commands/scan.rs` (modified — `dispatch`)
- `crates/components/camel-redis/src/commands/batch.rs` (modified — no change expected; verify the typed allowlist still rejects the family and its test names a `SCAN` entry)

**Steps:**
1. Add `RedisCommand::Scan`, `RedisCommand::Sscan`, `RedisCommand::Hscan`, `RedisCommand::Zscan` variants; `FromStr`: `"SCAN"|"SSCAN"|"HSCAN"|"ZSCAN"`; `as_str` mapping for each (follow the existing `"KEYS" => Ok(RedisCommand::Keys)` / `RedisCommand::Keys => "KEYS"` pattern). Extend the canonical drift-guard table in `as_str_roundtrip_all_variants` (config.rs ~4349) with the four rows — that test documents itself as requiring a row per new variant.
2. Add the four to `is_idempotent_command` as retry-safe reads (grouped with the read side, with a comment citing the repository pager precedent).
3. Add a routing arm in `dispatch_command` (executor.rs): `RedisCommand::Scan | Sscan | Hscan | Zscan => commands::scan::dispatch(cmd, conn, exchange).await`.
4. Implement `pub async fn dispatch(cmd: &RedisCommand, conn: &mut MultiplexedConnection, exchange: &mut Exchange) -> Result<(), CamelError>` in `commands/scan.rs`: resolve cursor + options (`allow_type = matches!(cmd, RedisCommand::Scan)`); require `CamelRedis.Key` for Sscan/Hscan/Zscan (use `require_key`); build via `encode_scan_page`; execute; parse via `parse_scan_reply` with the element type per verb (`SCAN` → `String`, `SSCAN` → `String`, `HSCAN` → `(String,String)`, `ZSCAN` → `(String,f64)`); set body via `serde_json::json!(elements)` for SCAN/SSCAN (identical output to `set.rs::json_from_members`), `serde_json::json!(HashMap::<String,String>::from_iter(pairs))` for HSCAN (mirroring the `Hgetall` arm), `commands::zset::json_from_scored_members(pairs)` for ZSCAN; set `exchange.input.set_header("CamelRedis.Cursor", serde_json::json!(next_cursor))`; when `next_cursor == 0` also `set_header("CamelRedis.ScanComplete", serde_json::json!(true))` (absent otherwise). Map transport errors via `crate::transport_error::redis_error_to_camel(verb, e)`.
5. Verify the `command=multi` typed allowlist rejects a `SCAN` entry: after step 1 the variants parse, so rejection comes from the not-batchable path (`batchable()` in batch.rs, "command \"SCAN\" is not batchable: …") — NOT the unknown-command path; extend the existing batch-entry rejection test to name a `SCAN` entry explicitly and assert the message names `CamelRedis.Batch`, the entry index, and `SCAN`.

**Tests:**
- `scan_command_from_str_roundtrip`: no setup → `"SCAN".parse::<RedisCommand>()` etc. for all four → each variant; `as_str()` inverts each.
- `scan_family_classifies_retry_safe`: no setup → `is_idempotent_command(&RedisCommand::Scan)` (and the other three) → all `true`.
- `scan_collection_variants_require_key_header`: exchange with `CamelRedis.Cursor=5` but no `CamelRedis.Key` → `require_key` (resolver-level, mirroring `test_hset_requires_key` in hash.rs — dispatch needs a live connection and is covered by the camel-test lane) → `Err` naming `CamelRedis.Key`.
- `scan_type_rejected_on_hscan_before_send`: exchange with `CamelRedis.Key=h`, `CamelRedis.Type="hash"`, command `HSCAN` → `resolve_scan_options(…, allow_type=false)` → `Err` naming `CamelRedis.Type` (resolver-level; no connection).
- `batch_entry_scan_is_named_rejection`: batch plan containing a `SCAN` entry → the not-batchable typed error names `CamelRedis.Batch`, the entry index, and `SCAN`.

**Acceptance:**
- `cargo test -p camel-component-redis` passes (full crate unit suite, including the extended `as_str_roundtrip_all_variants` drift-guard table).
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exit 0.
- `cargo xtask lint-unwrap` and `cargo fmt --check --all` exit 0.
- No SCAN wire frame is built outside `commands/scan.rs`: `grep -rn 'arg("SCAN")\|cmd("SCAN")' crates/` has zero hits outside `commands/scan.rs` (the encoder is the single builder; note `encode_scan_page` may render the verb via `ScanVerb::as_str()`, in which case both greps are zero everywhere — the invariant, not the grep hit count, is the criterion).

- [x] 1.2

## camel-redis-repo

### Task 1.3: Repository pager consumes the shared primitives

**Files:**
- `crates/services/camel-redis-repo/src/executor.rs` (modified — `scan_unlink_pattern_where` builds/parses via `camel_component_redis::commands::scan`)

**Steps:**
1. BEFORE refactoring, extend one existing SCAN test to pin the full frame: assert the recorded SCAN command's flattened args equal `["SCAN","0","MATCH","<pattern>","COUNT","100"]` (existing pins cover verb + MATCH only — cursor and COUNT are unpinned today; this extension is the safety net, run it red-agnostic before touching the pager).
2. Replace the inline `redis::Cmd` construction (lines ~150-156) with `encode_scan_page(ScanVerb::Scan, None, cursor, pattern, PAGE as u64, None)` (import from `camel_component_redis::commands::scan`; `PAGE` stays `usize` for the `chunks(PAGE)` batching — only the encoder call converts; `pub` per blessed design).
3. Replace the inline `from_redis_value(reply)` tuple parse (~158-159) with `parse_scan_reply::<String>(reply, "SCAN")`.
4. Keep everything else identical: PAGE constant, `execute_retry_safe` wrapping, UNLINK batching, skip guard, loop exit at cursor 0.
5. Run the repository crate's existing SCAN tests plus the step-1 frame pin.

**Tests:**
- Existing repo tests (`scan_unlink_*` suites in `cache_repo.rs`/`idempotent_repo.rs` and `executor.rs` tests) stay green unchanged.
- `repo_pager_uses_shared_encoder`: extended existing test (step 1) asserts the full frame `["SCAN","0","MATCH","<pattern>","COUNT","100"]` — this is unconditional, not a fallback.

**Acceptance:**
- `cargo test -p camel-redis-repo` passes unchanged.
- `cargo clippy -p camel-redis-repo --all-targets -- -D warnings` exit 0.
- No second SCAN wire-builder exists: `grep -rn 'arg("SCAN")' crates/` shows only `commands/scan.rs` (plus `Cmd arg("SCAN")`-free call sites).

- [x] 1.3

## camel-test (docker lane)

### Task 1.4: Live SCAN-family integration tests

**Files:**
- `crates/camel-test/tests/redis_test.rs` (modified)

**Steps:**
1. Add a `live_redis_reply_full` helper beside `live_redis_reply` (redis_test.rs ~1653) returning the whole reply `Exchange` (body + headers) instead of only the JSON body — SCAN tests need `CamelRedis.Cursor`/`CamelRedis.ScanComplete` headers.
2. Add `redis_live_scan_multipage_match_count`: seed ~250 keys `scan:it:<i>` via direct `MultiplexedConnection` admin connection; route `direct:live-scan` with `command=SCAN`, `CamelRedis.Pattern=scan:it:*`, `CamelRedis.Count=10`; loop from the test: send → assert one exchange (body = array, all elements match the pattern), read `CamelRedis.Cursor` out, feed it back as the next request's header; assert the final reply has `CamelRedis.Cursor=0` and `CamelRedis.ScanComplete=true`; assert every seeded key was seen (full coverage); assert the number of pages > 1 (COUNT 10 vs 250 keys makes single-page impossible on a real server).
3. Add `redis_live_hscan_map_shape`: seed a hash `scan:it:hash` with 200 fields f0..f199 (listpack encoding serves small collections in ONE page regardless of COUNT — 200 fields exceeds `hash-max-listpack-entries` 128 so the hashtable walk is multi-page); `command=HSCAN` with `CamelRedis.Count=1`; assert each page body is a JSON object whose entries are a subset of the seeded pairs, the union of pages equals all 200 pairs, and the last reply carries `ScanComplete=true`; assert page count > 1; make NO ordering claim (hashtable HSCAN returns bucket order, not insertion order).
4. Add `redis_live_zscan_pair_shape`: seed a zset `scan:it:zset` with 200 members scored 1.0..200.0 (same listpack/intset caveat; 200 members exceeds the small-collection encodings); `command=ZSCAN` with `CamelRedis.Count=1`; assert each page body is an array of `{"member","score"}` objects, the accumulated pages cover all 200 members with exact scores, and the last reply carries `ScanComplete=true`; no ordering claim.
5. Add `redis_live_sscan_member_array_body`: seed a set `scan:it:set` with 200 members; `command=SSCAN` with `CamelRedis.Count=1`; assert every page body is a JSON array of member strings, the union covers all 200 members, and the last reply carries `ScanComplete=true`.
6. Add `redis_live_scan_repeats_across_pages_full_coverage`: seed a handful of keys iterated with `COUNT 1` (multi-page walk guaranteed); the loop condition is the ABSENCE of `CamelRedis.ScanComplete` (never body emptiness — this is the empty-page pin: an empty page structurally continues the iteration); assert the union of page bodies covers every seeded key and that page bodies are forwarded verbatim (no component-side dedup or filtering).
7. Add `redis_live_keys_unchanged_frame`: seed 2 keys `keys:it:a`/`keys:it:b`; `command=KEYS` with `CamelRedis.Pattern=keys:it:*`; assert the wire behaviour is unchanged: body is the full JSON key array (both keys) in one exchange, no `CamelRedis.Cursor`/`CamelRedis.ScanComplete` headers set.

**Tests:**
- The seven `#[tokio::test(flavor = "multi_thread")]` functions above ARE the tests (setup: shared_redis container via the existing `support::redis::shared_redis()`; action: direct-exchange sends; assert: as listed per step).

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test redis_test -- redis_live_scan redis_live_sscan redis_live_hscan redis_live_zscan redis_live_keys` passes against local Docker (libtest accepts multiple filters after `--`; the filters cover all six new tests: `redis_live_scan_multipage_match_count`, `redis_live_scan_repeats_across_pages_full_coverage`, `redis_live_sscan_member_array_body`, `redis_live_hscan_map_shape`, `redis_live_zscan_pair_shape`, `redis_live_keys_unchanged_frame`).
- No new test sleeps (`cargo xtask lint-test-sleep` exit 0); waits use `wait_until` or direct awaits.

- [x] 1.4

## docs

### Task 1.5: SCAN family docs section with cursor-loop example

**Files:**
- `docs/src/components/redis.md` (modified — or the component page's actual path; locate with `grep -rn "command=KEYS" docs/src` first and use that file)

**Steps:**
1. Locate the camel-redis component page (`grep -rn "KEYS" docs/src --include="*.md"`).
2. Add a "SCAN family (cursor iteration)" section after the KEYS content: the four commands, the header table (`CamelRedis.Cursor` in/out, `CamelRedis.ScanComplete`, `CamelRedis.Pattern`/`Count`/`Type`), the one-exchange-per-page rule, and the body shapes per command.
3. Add ONE runnable example: a route snippet iterating `SCAN` pages until `CamelRedis.ScanComplete` is set, showing the cursor header being fed back (follow the page's existing route-snippet style).
4. Add the iteration-guarantee note: full coverage, possible repeats, COUNT is a hint; and a one-line prefer-SCAN-over-KEYS note (docs only — no KEYS behaviour claim).
5. Note the `TYPE` caveat (SCAN-only, Redis ≥ 6.0).

**Tests:**
- `cd docs && mdbook test` passes (the example is either a fenced non-tested snippet per the page's existing convention, or a tested one if the page uses `{{#playground}}`).

**Acceptance:**
- `cd docs && mdbook test` exit 0.
- The section names all four commands and both output headers (grep-verifiable).

- [x] 1.5

## change-dir bookkeeping

### Task 1.6: Review verdict artifact and gate sweep

**Files:**
- `openspec/changes/redis-scan/.review.json` (new — written at STAGE 4 by the conductor, not by a worker)

**Steps:**
1. Conductor-only: after the holistic review passes, write `.review.json` (verdict + reviewer + impl hash) inside the change dir and commit it with the archive-prep commit.
2. Conductor-only: run the full gate sweep (gates-local.sh + docker lanes) and record the table in the park report.

**Tests:**
- None (bookkeeping).

**Acceptance:**
- `.review.json` exists inside `openspec/changes/redis-scan/` and is committed.

- [x] 1.6
