# Tasks: peek-stale-on-miss

## Task 1.1 — PeekStaleMissPolicy + peek outcome properties in CachePeekStaleService

Files:
- crates/camel-processor/src/cache_eip.rs (modified)
- crates/camel-processor/src/lib.rs (modified)
- crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs (modified — call-site update only, step 7)

Steps:
1. Define `pub const CAMEL_CACHE_PEEK_HIT: &str = "CamelCachePeekHit";` and `pub const CAMEL_CACHE_PEEK_STALE: &str = "CamelCachePeekStale";` in cache_eip.rs, next to the CachePeekStaleService section. Re-export both from lib.rs by extending the existing `pub use cache_eip` re-export list (lib.rs line ~51).
2. Define `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum PeekStaleMissPolicy { Stop, Continue }` in cache_eip.rs with a doc-comment: `Stop` (default) preserves the CircuitBreaker.fallback absence-Stops contract; `Continue` leaves the body untouched on MISS so `choice` can branch on `CamelCachePeekHit`. Re-export from lib.rs.
3. Change `CachePeekStaleService` to carry `miss_policy: PeekStaleMissPolicy`; change `CachePeekStaleService::new` to take it as a third parameter. Update the doc-comment (lines ~418-426) to the blessed semantics: key-None arm Stops + `debug!` log; `Err` → `Failed`; HIT sets body + properties + `Completed`; MISS per policy.
4. Add a private helper `fn set_peek_properties(exchange: &mut Exchange, hit: bool, stale: bool)` that writes `serde_json::Value::Bool` values under the two constants via `exchange.set_property` (same pattern as `store_principal_properties` usage in the codebase).
5. Rework `run` (lines ~455-477):
   - key-None arm: `tracing::debug!(step = "cache_peek_stale", repository = %self.repository_name(), "key expression resolved to None; stopping branch")` then `Stopped`. Add a `repository_name()` helper returning `&str` for logging (check `CacheRepository` in crates/camel-api/src/cache.rs first for an existing name accessor; if none exists, log without the repository name and mark that deviation in the task report — do NOT add trait surface in this change).
   - HIT arm: compute `stale = entry.expires_at.map(|t| t <= SystemTime::now()).unwrap_or(false)` BEFORE `reconstruct_body` consumes the entry; set body; call `set_peek_properties(&mut exchange, true, stale)`; return `Completed`.
   - MISS arm `Stop`: `set_peek_properties(&mut exchange, false, false)`; `tracing::debug!` with fields `step = "cache_peek_stale"`, `repository` (via the name accessor), message `"peek miss; stopping branch per on_miss=stop"`; return `Stopped`. Raw cache keys MUST NOT appear in the record.
   - MISS arm `Continue`: `set_peek_properties(&mut exchange, false, false)`; return `Completed` with body untouched.
   - Raw cache keys MUST NOT appear in any log record.
6. Update the existing test at line ~1143 (none_key case) for the new `new` signature. Update any other `CachePeekStaleService::new` call sites in the crate tests.
7. Keep the workspace green: update the ONE external call site, `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (~line 204), to pass `camel_processor::PeekStaleMissPolicy::Stop` explicitly (today's default) — this preserves behavior until Task 1.2 threads the DSL value.

Tests (all in cache_eip.rs `mod tests`, using the existing `test_utils::MockCacheRepository` and the existing `fixed_key()`/`none_key()` helpers; follow the async-run pattern of the tests at lines 1102-1150):
- name: `peek_stale_miss_stop_sets_properties_and_stops`
  setup: MockCacheRepository with no entry under the fixed key
  action: run service with `PeekStaleMissPolicy::Stop`
  assert: outcome is `PipelineOutcome::Stopped(_)`; exchange properties `CamelCachePeekHit == Value::Bool(false)` and `CamelCachePeekStale == Value::Bool(false)`
  command: `cargo test -p camel-processor --lib peek_stale_miss_stop`
  expected: fails before step 5 (no properties set), passes after
- name: `peek_stale_miss_continue_completes_with_body_untouched`
  setup: MockCacheRepository empty; exchange body `Body::Text("orig")`
  action: run service with `PeekStaleMissPolicy::Continue`
  assert: outcome is `Completed(ex)`; body still `Body::Text("orig")`; both properties false
  command: `cargo test -p camel-processor --lib peek_stale_miss_continue`
  expected: fails before (Stopped), passes after
- name: `peek_stale_hit_sets_hit_and_stale_properties`
  setup: MockCacheRepository seeded with an entry whose `expires_at` is 1ms ago (post-expiry), via the mock's seed helper
  action: run service (either policy)
  assert: `Completed`; body is the entry body; `CamelCachePeekHit == true`, `CamelCachePeekStale == true`
  command: `cargo test -p camel-processor --lib peek_stale_hit_sets`
  expected: fails before (no properties), passes after
- name: `peek_stale_hit_fresh_sets_stale_false`
  setup: seeded entry with `expires_at` 1h in the future
  action/assert: `Completed`; `CamelCachePeekHit == true`, `CamelCachePeekStale == false`
  command: `cargo test -p camel-processor --lib peek_stale_hit_fresh`
  expected: fails before, passes after
- name: `peek_stale_miss_stop_emits_debug_log`
  setup: empty mock repo; the `CapturingWriter` MakeWriter tracing-capture pattern already used in this crate (see wire_tap.rs tests ~927-939 — copy that helper shape locally into the cache_eip test module)
  action: run service with Stop policy
  assert: exactly one captured record at DEBUG level whose message contains `"peek miss"` and whose fields include the repository name
  command: `cargo test -p camel-processor --lib peek_stale_miss_stop_emits`
  expected: fails before, passes after
- name: `peek_stale_key_none_stops_with_debug_log`
  setup: service built with `none_key()` expression; same CapturingWriter pattern
  action: run service
  assert: `Stopped(_)`; exactly one DEBUG record whose message contains `"resolved to None"`
  command: `cargo test -p camel-processor --lib peek_stale_key_none`
  expected: outcome already passes before (behavior pinned); log assertion passes after

Acceptance:
- `cargo test -p camel-processor --lib` passes including the six new tests
- `cargo clippy -p camel-processor -- -D warnings` exits 0
- `cargo build --workspace` exits 0 (core.rs call site updated with Stop default)
- `cargo fmt --check` clean for cache_eip.rs
- No raw cache key string appears in any `tracing` call added by this task

- [x] 1.1

## Task 1.2 — DSL surface: on_miss field on cache_peek_stale

Files:
- crates/camel-api/src/runtime.rs (modified — `CanonicalStepSpec::CachePeekStale` at ~164 gains `on_miss: Option<String>` with `#[serde(default, skip_serializing_if = "Option::is_none")]` so existing canonical JSON stays byte-compatible when the knob is absent)
- crates/camel-dsl/src/model.rs (modified)
- crates/camel-dsl/src/route_ast.rs (modified — `CachePeekStaleBody` at ~1038 gains `pub on_miss: Option<String>` with the same serde/schemars derives as its siblings)
- crates/camel-dsl/src/yaml.rs (modified — the `CachePeekStale` conversion arm at ~1302-1312 maps the new field into the StepDef)
- crates/camel-dsl/src/compile.rs (modified)
- crates/camel-core/src/lifecycle/application/route_definition.rs (modified — `BuilderStep::CachePeekStale` variant ~296 gains `on_miss: camel_processor::PeekStaleMissPolicy`)
- crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs (modified — bind `on_miss` and pass it to the 3-arg constructor)
- schemas/dsl/route-schema.json (modified — regenerated)
- schemas/canonical-route-spec.json (modified — regenerated)
- schemas/ts/CanonicalStepSpec.ts (modified — regenerated)
- crates/camel-lint/schema/route-schema.json (modified — copy of the regenerated DSL schema; `cargo xtask schema --check` verifies both)

Steps:
1. Add `pub on_miss: Option<String>` to `CachePeekStaleStepDef` (model.rs ~line 500) with doc-comment: `"stop"` (default) or `"continue"`; validated at compile time. Add the same field to `CachePeekStaleBody` in route_ast.rs (~1038) and to `CanonicalStepSpec::CachePeekStale` in camel-api/src/runtime.rs (~164, with the serde omit attributes named in Files).
2. Add the field to every construction site of `CachePeekStaleStepDef`: the yaml.rs conversion arm (~1312), both compile.rs arms' input side, and the existing tests (compile.rs ~4781 gets `on_miss: None`). NOTE: `camel-builder/src/lib.rs:1157` matches `BuilderStep::CachePeekStale { .. }` with a rest pattern — no change needed there.
3. Add `fn parse_peek_stale_on_miss(raw: Option<&str>) -> Result<camel_processor::PeekStaleMissPolicy, CamelError>` in compile.rs: `None | Some("stop") => Stop`, `Some("continue") => Continue`, anything else → `Err(CamelError::RouteError(msg))` where `msg` names the invalid value and the allowed set (fail closed; do NOT include unrelated exchange data).
4. Thread the parsed policy through BOTH compile paths: `DeclarativeStep::CachePeekStale` arm at compile.rs ~1201 and `CanonicalStepSpec::CachePeekStale` arms at ~468 and ~1439, producing `BuilderStep::CachePeekStale { repository, key, on_miss }`. The canonical arm reads the new `Option<String>` through `parse_peek_stale_on_miss` (canonical JSON with absent/`null` on_miss compiles to `Stop`). Update the single pattern match in step_compilers/core.rs (~187) to bind `on_miss` and pass it to `CachePeekStaleService::new(repo, key_expr, on_miss)` — replacing the `Stop` placeholder from Task 1.1 step 7.
5. Update `step_kind_name`-style match arms (compile.rs ~1574, ~1960) — no change needed beyond what compilation requires.
6. Regenerate schema artifacts: run `cargo xtask schema`, then copy the regenerated `schemas/dsl/route-schema.json` over `crates/camel-lint/schema/route-schema.json` (mirror what the xtask's schema-check compares, scripts/xtask/src/main.rs ~1106-1128), and commit all four artifacts.

Tests (crates/camel-dsl — place near the existing CachePeekStale test at compile.rs ~4781):
- name: `cache_peek_stale_on_miss_default_is_stop`
  setup: declarative step `CachePeekStaleStepDef { repository: None, key: fixed, on_miss: None }` compiled through the declarative path
  action: compile to BuilderStep
  assert: `BuilderStep::CachePeekStale { on_miss: PeekStaleMissPolicy::Stop, .. }`
  command: `cargo test -p camel-dsl --lib cache_peek_stale_on_miss_default`
  expected: fails before (field missing), passes after
- name: `cache_peek_stale_on_miss_continue_compiles`
  action: same with `on_miss: Some("continue")`
  assert: policy variant `Continue`
  command: `cargo test -p camel-dsl --lib cache_peek_stale_on_miss_continue`
  expected: fails before, passes after
- name: `cache_peek_stale_on_miss_invalid_rejected`
  action: `on_miss: Some("skip")`
  assert: compile returns `Err` whose Display contains `must be "stop" or "continue"`
  command: `cargo test -p camel-dsl --lib cache_peek_stale_on_miss_invalid`
  expected: fails before, passes after
- name: YAML parse test `cache_peek_stale_on_miss_yaml_parses` in the existing `yaml.rs` test module (mod tests at ~1879; a cache_peek_stale fixture already exists at ~4779 — extend it or add a sibling fixture with `on_miss: continue`)
  action: parse the YAML route through the module's established fixture-parsing helper
  assert: parsed `DeclarativeStep::CachePeekStale` carries `on_miss == Some("continue")`
  command: `cargo test -p camel-dsl --lib cache_peek_stale_on_miss_yaml_parses`
  expected: fails before, passes after
- name: canonical round-trip test `canonical_cache_peek_stale_on_miss_round_trip` in the `crates/camel-api/src/runtime.rs` test module
  action: serialize `CanonicalStepSpec::CachePeekStale { on_miss: Some("continue") }` to JSON and deserialize back; also serialize with `on_miss: None` and assert the JSON key is absent
  assert: round trip preserves `Some("continue")`; the None case emits no `on_miss` key (backward-compatible canonical JSON)
  command: `cargo test -p camel-api --lib canonical_cache_peek_stale_on_miss_round_trip`
  expected: fails before, passes after

Acceptance:
- `cargo test -p camel-dsl --lib` passes
- `cargo clippy -p camel-dsl -- -D warnings` exits 0
- `cargo build --workspace` exits 0 (real policy wired in core.rs)
- `cargo xtask schema --check` exits 0

- [x] 1.2

## Task 1.3 — camel-core integration tests (SWR end-to-end)

Files:
- crates/camel-core/tests/peek_stale_on_miss_test.rs (new file)

Steps:
1. Create the integration test file. Reuse the harness pattern from a neighboring camel-core integration test that compiles a declarative route and executes exchanges (grep `tests/` for an existing route-execution test; follow its fixture helpers for building the step-compiler context — do not invent a new harness).

Tests:
- name: `swr_route_compiles_and_continues_on_miss` (integration)
  setup: declarative route `from: "direct://swr"` with steps: `set_body` (text "orig"), `cache_peek_stale { repository: "memory", key: fixed, on_miss: continue }`, `set_body` (simple "PIPELINE-COMPLETE"). Compile the route, execute one exchange through the compiled pipeline (follow the harness pattern of neighboring camel-core route tests for invoking a compiled route on an exchange).
  action: run one exchange with an empty memory repository
  assert: final body is "PIPELINE-COMPLETE" (the second set_body ran — the MISS did not stop the pipeline); property `CamelCachePeekHit` is false on the exchange
  command: `cargo test -p camel-core --test peek_stale_on_miss_test`
  expected: fails with Stop-truncation (body stays "orig") before Task 1.2 wiring, passes after
- name: `swr_route_default_stop_stops_pipeline`
  setup: same route without `on_miss` (default stop)
  action/assert: pipeline outcome is Stopped / final set_body does not run; body remains "orig"
  command: same invocation, test name `swr_route_default_stop`
  expected: passes both before and after (pins the default)
- name: `peek_stale_service_receives_policy_from_dsl`
  setup/action: compile only (no execution) the `on_miss: continue` route; inspect the compiled `BuilderStep::CachePeekStale` variant
  assert: carries `PeekStaleMissPolicy::Continue`
  command: `cargo test -p camel-core peek_stale_service_receives_policy`
  expected: passes (wiring landed in Task 1.2; this test pins it against regression)

Acceptance:
- `cargo test -p camel-core --lib` passes
- `cargo test -p camel-core --test peek_stale_on_miss_test` passes
- `cargo clippy -p camel-core -- -D warnings` exits 0

- [x] 1.3

## Task 1.4 — Docs and context alignment

Files:
- crates/camel-processor/CONTEXT.md (modified)
- docs/src/eip/cache.md (modified)
- docs/src/yaml-dsl/step-verbs.md (modified)
- examples/cache-example/routes.yaml (modified)

Steps:
1. crates/camel-processor/CONTEXT.md: in the EIP catalog `cache_eip` row, append `on_miss stop|continue knob + CamelCachePeekHit/CamelCachePeekStale properties` to the CachePeekStaleService description. In the Language section, add entries for `CamelCachePeekHit` and `CamelCachePeekStale` following the existing entry format (name, one-paragraph definition, source anchor `crates/camel-processor/src/cache_eip.rs`).
2. docs/src/eip/cache.md: add an `on_miss` subsection under cache_peek_stale documenting both values, the default, the two properties, and a stale-while-revalidate recipe YAML snippet (peek with on_miss=continue → choice on `exchangeProperty.CamelCachePeekHit` → serve vs fetch+cache).
3. docs/src/yaml-dsl/step-verbs.md: add the `on_miss:` field to the cache_peek_stale row/section following the file's existing field-listing format.
4. examples/cache-example/routes.yaml: add a second route with id `swr-cache-route` demonstrating peek + on_miss: continue + choice; keep the existing `cache-route` anchor untouched (docs include it by anchor).
5. STE-writing: keep prose in Simplified Technical English per the repo skill; English only.

Tests:
- name: docs-anchors-intact
  action: `grep -c "cache-route" examples/cache-example/routes.yaml` and `grep -n "{{#include" docs/src/eip/cache.md`
  assert: the original `cache-route` id still exists exactly once and the include path is unchanged
  command: as above
  expected: passes
- name: swr-example-fields-valid
  action: `grep -n "on_miss: continue" examples/cache-example/routes.yaml` and `grep -n "CamelCachePeekHit" examples/cache-example/routes.yaml docs/src/eip/cache.md`
  assert: the example route uses the exact field name landed in Task 1.2 and the property name landed in Task 1.1
  command: as above
  expected: passes
- name: context-citations
  action: `cargo xtask lint-context-citations`
  assert: exit 0 (new Language entries carry valid source anchors)
  command: `cargo xtask lint-context-citations`
  expected: passes

Acceptance:
- `cargo xtask lint-context-citations` exits 0
- No new Spanish prose in touched files
- `grep -c "on_miss: continue" examples/cache-example/routes.yaml` returns at least 1

- [x] 1.4

## Coverage matrix — delta scenarios to tests

| Delta scenario | Owning test (crates/camel-processor/src/cache_eip.rs unless noted) |
|---|---|
| cache hit short-circuits on_miss (preserved) | `cache_hit_short_circuits_on_miss` |
| cache miss runs on_miss, sets, continues (preserved) | `cache_miss_runs_on_miss_sets_continues` |
| cache miss oversized materialized body skips write-back (preserved) | `cache_miss_oversized_materialized_body_skips_writeback` |
| cache miss oversized stream propagates Err (preserved) | `cache_miss_oversized_stream_propagates_err` |
| cache on_miss Stopped propagates without write-back (preserved) | `cache_on_miss_stopped_propagates_without_writeback` |
| cache on_miss Err propagates without write-back (preserved) | `cache_on_miss_err_propagates_without_writeback` |
| cache repository get Err propagates (preserved) | `cache_repository_get_err_propagates` |
| cache repository set Err propagates (preserved) | `cache_repository_set_err_propagates` |
| cache_peek_stale serves post-expiry entry (preserved) | `cache_peek_stale_serves_post_expiry_entry` + NEW `peek_stale_hit_sets_hit_and_stale_properties` seeds a genuinely ELAPSED `expires_at` (existing test seeds `expires_at: None`, so the elapsed case is only proven by the new test) |
| cache_peek_stale HIT sets peek properties | 1.1 `peek_stale_hit_sets_hit_and_stale_properties`, `peek_stale_hit_fresh_sets_stale_false` |
| cache_peek_stale on absence Stops the branch (+props+log) | 1.1 `peek_stale_miss_stop_sets_properties_and_stops`, `peek_stale_miss_stop_emits_debug_log` |
| cache_peek_stale on_miss continue passes through on absence | 1.1 `peek_stale_miss_continue_completes_with_body_untouched` + 1.3 `swr_route_compiles_and_continues_on_miss` |
| cache_peek_stale on_miss invalid value fails compile | 1.2 `cache_peek_stale_on_miss_invalid_rejected` |
| cache_peek_stale key expression None Stops with debug log | 1.1 `peek_stale_key_none_stops_with_debug_log` |
| cache_invalidate removes the key (preserved) | `cache_invalidate_calls_repository_invalidate` |
