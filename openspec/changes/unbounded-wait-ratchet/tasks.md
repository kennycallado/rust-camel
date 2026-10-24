# Tasks: unbounded-wait-ratchet

Shared worker conventions (apply to every task below):

- Inventory of record: `openspec/changes/unbounded-wait-ratchet/classify/group-{a,b,c}.tsv`
  (columns: `file:line`, bucket, action, reason). Get your task's exact rows with
  `grep -E '<your files>' <tsv>`. Do not adjudicate sites outside your rows.
- Bucket A fix shape: `timeout_wait("<label>", Duration::from_secs(N), <future-expr>).await`
  with `use camel_component_api::test_support::timeout_wait;`. LOOP-CLASS FINDINGS (the lint
  flags the `loop` line): the frozen lint does NOT recognize `timeout_wait` inside a loop as
  per-iteration bounding — use direct `tokio::time::timeout(N, <expr>)` per iteration there
  (recognized deadline region), keep the loop's existing exit conditions, and make `Elapsed`
  fail loudly (panic naming the wait). Preserve existing
  `.expect(..)`/match chains on the inner output (channel-closed stays distinct; the helper
  panics on Elapsed naming label + deadline + call site). Deadline N: 30 for unit-tier
  files, 60 for integration-tier files (`tests/` dirs of integration crates). Label:
  short snake phrase naming the wait (e.g. `"bridge health call"`).
- Bucket B marker shape: `// allow-test-wait: <reason>` on the line spanned by the finding
  expression (same line as the awaited call when it fits within 100 cols; otherwise the
  line above the closing line of the expression). Reason = the TSV reason, verbatim where
  it fits, never a convenience phrase.
- Every task: `cargo fmt` on touched files, `cargo clippy -p <crate> -- -D warnings`
  (or workspace scoping per AGENTS.md clippy gate for multi-crate tasks) must exit 0
  before reporting. Run touched test targets that need no external infra; if a target needs
  live infra (DB/container), run `cargo test -p <crate> --test <target> --no-run` (compile)
  and note `integration-verification-deferred-to-CI` in the report.
- Commit per task. Never leave the tree dirty between tasks.

## Phase 1: Shared helper + marker documentation

### Task 1.1: timeout_wait helper in camel-component-api test_support

**Files:**
- `crates/components/camel-component-api/src/test_support.rs` or `test_support/mod.rs` (modified — same module that holds `acquire_deadline`)
- `crates/components/camel-component-api/src/test_support_tests.rs` or existing test module location for `test_support` (modified — add tests beside existing `acquire_deadline` tests)

**Steps:**
1. Locate `acquire_deadline` in the `test_support` module (feature `test-support`) and add,
   in the same module with the same docs style:
   `#[track_caller] pub fn timeout_wait<T>(what: &str, deadline: Duration, fut: impl Future<Output = T>) -> impl Future<Output = T>`.
2. Implementation: `tokio::time::timeout(deadline, fut)` — on `Ok(out)` return `out`; on
   `Err(_elapsed)` panic with a message containing `what`, the deadline in seconds, and
   `std::panic::Location::caller()` (capture before the await via a `#[track_caller]`-sourced
   `&'static Location`, same pattern `acquire_deadline` uses for its panic).
3. Doc comment: one line stating purpose (bound a one-shot test wait, panic loudly on hang),
   one line stating it is THE shared helper — no per-crate reimplementation (spec
   unbounded-wait-bounding "Shared bounded test-wait helper timeout_wait").
4. Unit tests (below) in the same test style as the existing `acquire_deadline` tests
   (they capture panic payloads; no `tokio::time::sleep`).

**Tests:**
- `timeout_wait_panics_naming_what_deadline_and_site`: setup — a pending future
  (`std::future::pending::<()>()`); act — `timeout_wait("orders receive",
  Duration::from_millis(50), pending()).await` inside `std::panic::catch_unwind` (or the
  crate's established panic-capture helper); assert — payload contains `orders receive`,
  `50ms` or the deadline text, and this test's file location.
- `timeout_wait_passes_output_through`: setup — an `mpsc` channel, sender dropped after
  sending one value; act — `timeout_wait("recv", Duration::from_secs(5), rx.recv()).await`;
  assert — returns `Some(value)`, no panic.
- `timeout_wait_channel_closed_distinct_from_hang`: setup — dropped sender, no value;
  act — the same bounded recv; assert — returns `None` (channel-closed observed as value,
  not conflated with the deadline path).
- command: `cargo test -p camel-component-api --features test-support --lib timeout_wait`
- expected: all three pass after implementation; the panic test fails before it (helper
  absent → compile error, which is the acceptable pre-state).

**Acceptance:**
- `cargo test -p camel-component-api --features test-support --lib` exits 0.
- `cargo clippy -p camel-component-api --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-test-sleep` exits 0 (no sleeps introduced).
- Exactly one new public fn added; no second bounded-wait helper anywhere.

- [x] 1.1

### Task 1.2: Verify lint mechanics + add ratchet header doc (disposable sample, tracked evidence)

**Files:**
- `crates/camel-test/tests/unbounded_wait_mechanics_scratch.rs` (new, NEVER committed — `rm` before commit)
- `openspec/changes/unbounded-wait-ratchet/phase1-verification.md` (new, committed — the evidence record)
- `scripts/xtask/ratchet-unbounded-wait.max` (modified — header comment line ONLY, value stays 296)

**Steps:**
1. Write a scratch integration-test file with two `#[tokio::test]` fns:
   `helper_shape_kills_finding` — an `mpsc` recv written as
   `timeout_wait("scratch", Duration::from_secs(30), rx.recv()).await.expect("closed")`
   (import from `camel_component_api::test_support`; camel-test already deps it with
   `test-support`); `marker_suppresses_finding` — a bare `rx2.recv().await` line carrying
   `// allow-test-wait: scratch: test-fed channel, value sent before recv` on that line,
   with the send deterministically before the recv in the test body; and
   `loop_helper_shape` — a `loop` whose body awaits
   `timeout_wait("scratch loop", Duration::from_secs(30), rx3.recv()).await` and exits on
   channel-close, to observe whether the lint flags the loop line (expected per lint
   source: loop-class findings need recognized per-iteration bounding; record actual
   behavior). If `loop_helper_shape` IS flagged, Phase 2 loop sites use direct
   `tokio::time::timeout` (as the conventions already mandate) — record the observation.
2. Run `cargo xtask lint-unbounded-wait 2>&1 | tee /home/shared/tmp/394-lint-p1.txt; echo EXIT:${PIPESTATUS[0]}` —
   expected `EXIT:1` (count above ceiling 296 at this point); the count is 299 OR 300 —
   300 exactly when `loop_helper_shape` is flagged (that fn is a live unbounded loop by
   design); verify: `helper_shape_kills_finding` and `marker_suppresses_finding` do NOT
   appear in the findings list (helper-call await is non-wait-class; marker suppresses),
   and record whether `loop_helper_shape` was flagged.
3. Write `phase1-verification.md` recording: (a) helper-shape observation (absent from
   findings), (b) marker-suppression observation (absent from findings), (c) loop
   observation (flagged or not, with the observed count — 299 or 300), (d) the exact lint
   command + its output count line.
4. Add ONE header comment line to `scripts/xtask/ratchet-unbounded-wait.max` ABOVE the
   value line (parser ignores comment lines; value stays 296):
   `# allow-test-wait: <reason> adjudicates a site ONLY when the wait is bounded by construction (test-owned uncontended lock, test-fed channel, join whose every progress dependency is bounded or purely in-memory) — never to mask a wait that could hang; ceiling counts unadjudicated findings and may only decrease.`
5. `rm` the scratch file (it was never `git add`ed). Re-run the lint: count exactly 299
   (scratch gone). Commit ONLY `phase1-verification.md` + the ratchet header line.

**Tests:**
- `helper_shape_kills_finding`: lint observation only (must not appear in findings).
- `marker_suppresses_finding`: lint observation only (must not appear in findings).
- `loop_helper_shape`: lint observation only — record flagged/not-flagged in
  `phase1-verification.md`; this determines nothing new (conventions already mandate
  direct `tokio::time::timeout` in loops) but documents the frozen-lint boundary.
- command: `cargo xtask lint-unbounded-wait 2>&1 | tee /home/shared/tmp/394-lint-p1.txt; echo PIPE0:${PIPESTATUS[0]}`
  — expected: helper/marker fns absent; count 299 or 300 (300 iff loop fn flagged);
  after scratch removal the count is exactly 299.
- expected: evidence file records all three observations with the count line quoted.

**Acceptance:**
- Scratch file absent from the commit and from the worktree (`git status` clean apart from
  the two intended files).
- `phase1-verification.md` committed with all three observations + count lines.
- Ratchet file: header line added, value still exactly `296`; final post-removal lint count exactly 299.
- No other file touched.

- [x] 1.2

## Phase 2: Bucket A — bound the 54 unbounded sites

### Task 2.1: Bound bridge-client family (jms, validator, xslt, cxf)

**Files:**
- `crates/components/camel-jms/src/bridge_client_test.rs` (modified — sites 242, 283, 320, 430, 497)
- `crates/components/camel-jms/src/consumer.rs` (modified — site 2527)
- `crates/components/camel-validator/src/bridge_client_test.rs` (modified — sites 118, 129)
- `crates/components/camel-xslt/src/bridge_client_test.rs` (modified — sites 101, 114)
- `crates/components/camel-cxf/src/bridge_client_test.rs` (modified — site 112)
- `crates/components/camel-cxf/tests/concurrency_test.rs` (modified — sites 90, 112, 150)
- `crates/components/camel-cxf/tests/consumer_unit_test.rs` (modified — site 515)
- `crates/components/camel-cxf/tests/ws_security_test.rs` (modified — sites 52, 109)

**Steps:**
1. Pull rows: `grep -E 'camel-jms|camel-validator|camel-xslt|camel-cxf'
   openspec/changes/unbounded-wait-ratchet/classify/group-{a,c}.tsv | grep '	A	'`.
2. Wrap each listed site per shared conventions (these are tonic `connect()` calls, TLS
   handshakes, and one contended producer mutex at concurrency_test.rs:90 — use 30s for
   `src/` inline tests, 60s for `tests/` files). Keep every existing
   `.expect(..)`/match on the inner result exactly where it is — the helper wraps the
   future expression only.
3. Add the `timeout_wait` import once per file where missing.

**Tests:**
- `cargo test -p camel-jms --lib` — RUN (inline unit tests, infra-free); record exit code.
- `cargo test -p camel-validator --lib` — RUN; record exit code.
- `cargo test -p camel-xslt --lib` — RUN; record exit code.
- `cargo test -p camel-cxf --lib` — RUN (cxf inline `src/bridge_client_test.rs` site 112
  lives in the lib target); record exit code.
- `cargo test -p camel-cxf --test concurrency_test --test consumer_unit_test --test ws_security_test`
  — RUN (in-memory mock bridge, no external process); record exit code; if a target fails
  on a connection error unrelated to this change, re-run that target `--no-run` and record
  `integration-verification-deferred-to-CI` with the failure's first line as evidence.

**Acceptance:**
- `cargo xtask lint-unbounded-wait 2>&1 | tee /home/shared/tmp/394-lint-t21.txt; echo EXIT:${PIPESTATUS[0]}` —
  grep the saved file for the count line: expected 299 − 17 = 282; each wrapped site no
  longer flagged; expected `EXIT:0` (282 below ceiling 296).
- `cargo fmt --check` exit 0; `cargo clippy -p camel-jms -p camel-validator -p camel-xslt -p camel-cxf -- -D warnings`
  exit 0.
- No `.expect` chain removed or renamed; diffs show wrapper additions only.

- [x] 2.1

### Task 2.2: Bound camel-test, camel-cli, camel-core sites

**Files:**
- `crates/camel-test/tests/hot_reload_integration.rs` (modified — sites 132, 223, 320, 405)
- `crates/camel-test/tests/integration_test.rs` (modified — sites 3523, 3567)
- `crates/camel-test/tests/mcp_server_auth_test.rs` (modified — site 302)
- `crates/camel-test/tests/mtls_bridge_test.rs` (modified — site 77)
- `crates/camel-test/tests/sql_test.rs` (modified — site 841)
- `crates/camel-cli/src/commands/test/scenario_tests.rs` (modified — site 577)
- `crates/camel-core/src/lifecycle/adapters/redb_journal.rs` (modified — site 1210)

**Steps:**
1. Pull rows: `grep -E 'camel-test|camel-cli|camel-core'
   openspec/changes/unbounded-wait-ratchet/classify/group-{a,b}.tsv | grep '	A	'`.
2. Wrap each site per shared conventions. hot_reload sites are task joins (task may never
   observe cancellation) — wrap the `.await` of each join with 60s. sql_test.rs:841 is a
   pool connect — 60s. redb_journal.rs:1210 is an open-retry join inside a `#[cfg(test)]`
   module — 30s. Preserve assertion chains.
3. camel-test and camel-core and camel-cli already dep `camel-component-api` with
   `test-support` — add the import per file.

**Tests:**
- `cargo test -p camel-test --test hot_reload_integration` — RUN (tempdir-based, no
  external infra); record exit code.
- `cargo test -p camel-test --test mcp_server_auth_test` — RUN (local staged listener);
  record exit code.
- `cargo test -p camel-test --test mtls_bridge_test` — RUN (local staged listener); record
  exit code.
- `cargo test -p camel-test --test integration_test --no-run` — COMPILE-ONLY (3500+ line
  shared suite; execution owned by CI); record exit code.
- `cargo test -p camel-test --test sql_test --no-run` — COMPILE-ONLY (live DB);
  `integration-verification-deferred-to-CI`.
- `cargo test -p camel-cli --lib` — RUN; record exit code.
- `cargo test -p camel-core --lib redb` — RUN; record exit code.

**Acceptance:**
- Lint findings count 282 − 11 = 271 after this task (saved lint output shows it).
- `cargo clippy -p camel-test -p camel-cli -p camel-core -- -D warnings` exit 0; fmt clean.
- Every touched target appears in the report with its disposition (run exit code, or
  compile-only + defer note) — no target unlisted.

- [x] 2.2

### Task 2.3: Bound camel-integration-test and camel-dsl sites

**Files:**
- `crates/camel-integration-test/tests/partner_verification_test.rs` (modified — sites 754, 997, 1074, 1147, 1272, 1323)
- `crates/camel-integration-test/tests/http_inbound_test.rs` (modified — sites 170, 304)
- `crates/camel-integration-test/src/boot_scenario_test.rs` (modified — site 870)
- `crates/camel-integration-test/src/runner/partner_validate_test.rs` (modified — site 453)
- `crates/camel-dsl/tests/rest_stream_contract_e2e.rs` (modified — site 583)

**Steps:**
1. Pull rows: `grep -E 'camel-integration-test|camel-dsl'
   openspec/changes/unbounded-wait-ratchet/classify/group-{a,c}.tsv | grep '	A	'`.
2. partner_verification joins: wrap the spawned-task join `.await`s (NOT the inner
   `raw_post` body — bounding the join is sufficient and keeps the helper shape
   `timeout_wait("<task> join", Duration::from_secs(60), task).await`); 60s tier.
   partner_validate_test.rs:453 is the same shape (`settling.await` joins a task running
   deadline-free `raw_request`) — wrap the join `.await`, 60s tier.
   boot_scenario_test.rs:870 is a pool acquire — 60s. http_inbound/dsl sites per their
   TSV action (connect/recv wraps), 60s tier.

**Tests:**
- `cargo test -p camel-integration-test --no-run` — COMPILE-ONLY (partner harness targets
  need a live local partner server; execution owned by CI); record exit code and
  `integration-verification-deferred-to-CI`.
- `cargo test -p camel-dsl --test rest_stream_contract_e2e` — RUN (in-process contract
  test); record exit code; if it fails on a missing external service, re-run `--no-run`
  and record the defer note with the failure's first line.

**Acceptance:**
- Lint findings count 271 − 11 = 260 after this task (saved lint output shows it).
- `cargo clippy -p camel-integration-test -p camel-dsl -- -D warnings` exit 0; fmt clean.
- Inner `raw_post` body unchanged (diff shows join-wrap additions only).

- [x] 2.3

### Task 2.4: Bound remaining component sites (rabbitmq, sql, http, llm, mcp, wasm)

**Files:**
- `crates/components/camel-rabbitmq/src/connection/tests.rs` (modified — site 463)
- `crates/components/camel-rabbitmq/src/consumer/tests.rs` (modified — site 287)
- `crates/components/camel-rabbitmq/tests/consumer_readiness.rs` (modified — site 94)
- `crates/components/camel-sql/src/health.rs` (modified — site 265)
- `crates/components/camel-sql/src/pool_factory.rs` (modified — sites 264, 265, 318)
- `crates/components/camel-http/src/lib_tests.rs` (modified — sites 5122, 6973: both are
  loop-class findings; 5122 awaits a TCP connect inside the loop, 6973 awaits
  `setup_consumer_on_free_port(..)` inside the loop)
- `crates/components/camel-component-llm/tests/producer_cache.rs` (modified — site 271)
- `crates/components/camel-component-mcp/tests/server_consumer_test.rs` (modified — sites 455, 460)
- `crates/components/camel-component-mcp/tests/server_registry_test.rs` (modified — site 41)
- `crates/components/camel-component-wasm/tests/source_stream_integration.rs` (modified — site 294)

**Steps:**
1. Pull rows: `grep -E 'camel-rabbitmq|camel-sql|camel-http|camel-component-llm|camel-component-mcp|camel-component-wasm'
   openspec/changes/unbounded-wait-ratchet/classify/group-{a,c}.tsv | grep '	A	'`.
2. Wrap each site per shared conventions, 30s for `src/` inline tests, 60s for `tests/`
   files. sql pool_factory sites are pool connects/acquires — one wrap each (264 and 265
   are two distinct flagged sites; read them separately). http lib_tests 5122/6973 are
   LOOP-class findings: per the conventions, bound the awaited expression per iteration
   with direct `tokio::time::timeout(60, <exact future expression>)` — for 5122 the
   `TcpStream::connect(..)` future, for 6973 the `setup_consumer_on_free_port(..)` future
   — make `Elapsed` fail loudly (panic naming the wait), and keep each loop's existing
   termination conditions byte-identical. All other sites in this task are single-await
   wraps via `timeout_wait`.
3. Preserve all assertion chains; imports per file.

**Tests:**
- `cargo test -p camel-rabbitmq --lib` — RUN; record exit code.
- `cargo test -p camel-rabbitmq --test consumer_readiness` — RUN (local listener);
  record exit code; if it fails on a broker connection error unrelated to this change,
  re-run `--no-run` and record `integration-verification-deferred-to-CI` with the
  failure's first line.
- `cargo test -p camel-sql --lib` — RUN; record exit code.
- `cargo test -p camel-http --lib` — RUN; record exit code.
- `cargo test -p camel-component-llm --test producer_cache` — RUN; record exit code.
- `cargo test -p camel-component-mcp --test server_consumer_test --test server_registry_test`
  — RUN; record exit code.
- `cargo test -p camel-component-wasm --test source_stream_integration --no-run` —
  COMPILE-ONLY (wasm runtime); `integration-verification-deferred-to-CI`.

**Acceptance:**
- Lint findings count 260 − 14 = 246 after this task (bucket A fully burned; saved lint
  output shows it).
- `cargo clippy -p camel-rabbitmq -p camel-sql -p camel-http -p camel-component-llm -p camel-component-mcp -p camel-component-wasm -- -D warnings`
  exit 0; fmt clean.

- [x] 2.4

## Phase 3: Bucket B — adjudicate the 245 bounded-by-construction sites (START ONLY AFTER ALL PHASE-2 TASKS ARE CHECKED OFF)

### Task 3.1: Mark camel-test sites

**Files:**
- every file under `crates/camel-test/` with a B row in `classify/group-{a,b}.tsv` (93 rows: opensearch_test 13, surrealdb_test 13, sql_test 12, integration_test 11, redis_test 9, script_test 7, jsonpath_test 3, otel_trace_tree_test 3, xpath_test 3, seda_test 4, do_try_test 2, throttle_test 2, plus single-row files and `src/harness.rs` 3, `src/time.rs` 1) (all modified, comments only)

**Steps:**
1. Pull rows: `grep 'crates/camel-test' openspec/changes/unbounded-wait-ratchet/classify/*.tsv | grep '	B	'`.
2. Add `// allow-test-wait: <TSV reason>` on the spanned line per shared conventions.
   The dominant pattern here is `h.ctx().lock().await` on the harness context mutex
   (test-owned, uncontended) — the reason states exactly that, matching the existing
   same-file precedents (jsonpath_test.rs:218, otel_trace_tree_test.rs:637,
   xpath_test.rs:217). Comments only — zero code changes.
3. Do NOT mark any site whose TSV bucket is A (those are done in Phase 2).

**Tests:**
- `cargo xtask lint-unbounded-wait 2>&1 | tee /home/shared/tmp/394-lint-t31.txt; echo EXIT:${PIPESTATUS[0]}` —
  grep the saved file for `crates/camel-test`: expected 0 hits (9 bounded in Phase 2 + 93
  marked here); expected `EXIT:0` (count already below ceiling 296 after Phase 2).
- `cargo test -p camel-test --lib` — RUN (compile sanity for `src/harness.rs`/`src/time.rs`
  comment insertions); record exit code. Suite targets needing live DBs are CI's — defer
  note only.

**Acceptance:**
- 93 markers added, one per B row; no code lines changed (`git diff --stat` shows only the
  listed files; each hunk is a comment insertion).
- Saved lint output: camel-test contributes 0 findings.
- `cargo fmt --check` exit 0.

- [x] 3.1

### Task 3.2: Mark grpc, cxf, stream, jms sites

**Files:**
- `crates/components/camel-component-grpc/src/server.rs` (modified — 14 B rows)
- `crates/components/camel-component-grpc/tests/server_auth_test.rs` (7), `crates/components/camel-component-grpc/tests/integration.rs` (2) (modified)
- `crates/components/camel-cxf/tests/producer_unit_test.rs` (14) (modified)
- `crates/components/camel-stream/src/consumer_tests.rs` (10), `crates/components/camel-stream/src/lib.rs` (2) (modified)
- `crates/components/camel-jms/src/component.rs` (7) (modified)
- all other files under those four crates with B rows in the TSVs (modified)

**Steps:**
1. Pull rows: `grep -E 'camel-component-grpc|camel-cxf|camel-stream|camel-jms'
   openspec/changes/unbounded-wait-ratchet/classify/*.tsv | grep '	B	'` (61 rows).
2. Add markers per shared conventions, reasons from the TSV verbatim (dominant patterns:
   stand-in pipeline joins bounded by inner 2s recv timeouts in grpc tests; deterministic
   mock-bridge channels in cxf producer_unit_test; test-controlled streams in camel-stream;
   harness-driven channels in jms component.rs). Comments only.
3. producer_unit_test.rs markers: reason cites the deterministic in-memory mock bridge
   answering from memory (no network dependency) — construction, not convenience.

**Tests:**
- `cargo xtask lint-unbounded-wait 2>&1 | tee /home/shared/tmp/394-lint-t32.txt; echo EXIT:${PIPESTATUS[0]}` —
  grep the saved file for the four crate paths: expected 0 hits (61 marked + 13 bounded in
  Phase 2); expected `EXIT:0`.
- `cargo test -p camel-component-grpc --lib` — RUN; record exit code.
- `cargo test -p camel-stream --lib` — RUN; record exit code.
- `cargo test -p camel-jms --lib` — RUN; record exit code.
- `cargo test -p camel-cxf --no-run` — COMPILE-ONLY for remaining untouched cxf targets;
  record exit code (the touched cxf unit targets ran in 2.1).

**Acceptance:**
- 61 markers added; comments only (`git diff` shows no non-comment line changes).
- Saved lint output: the four crates contribute 0 findings.
- fmt clean; `cargo clippy -p camel-component-grpc -p camel-stream -p camel-jms -p camel-cxf -- -D warnings` exit 0.

- [x] 3.2

### Task 3.3: Mark integration-test, processor, mcp, llm, wasm, direct, http, seda, component-api sites

**Files:**
- all files under `crates/camel-integration-test/`, `crates/camel-processor/`,
  `crates/components/camel-component-mcp/`, `crates/components/camel-component-llm/`,
  `crates/components/camel-component-wasm/`, `crates/components/camel-direct/`,
  `crates/components/camel-http/`, `crates/components/camel-component-seda/`,
  `crates/components/camel-component-api/` with B rows in the TSVs (63 rows) (all modified)

**Steps:**
1. Pull rows: `grep -E 'camel-integration-test|camel-processor|camel-component-mcp|camel-component-llm|camel-component-wasm|camel-direct|camel-http/|camel-component-seda|camel-component-api'
   openspec/changes/unbounded-wait-ratchet/classify/*.tsv | grep '	B	'` (63 rows).
2. Add markers per shared conventions, TSV reasons verbatim where they fit. Dominant
   patterns: mock-state/stream-body mutexes owned solely by the test (mcp, direct, http);
   joins with inner enforced timeouts (mcp e2e, integration-test); harness-owned channels
   (processor cache_eip/wire_tap). Comments only.

**Tests:**
- `cargo xtask lint-unbounded-wait 2>&1 | tee /home/shared/tmp/394-lint-t33.txt; echo EXIT:${PIPESTATUS[0]}` —
  grep the saved file for these crate paths: expected 0 B-row hits (64 marked + 16 A burned
  in Phase 2); expected `EXIT:0`.
- `cargo test -p camel-processor --lib` — RUN; record exit code.
- `cargo test -p camel-component-mcp --no-run` — COMPILE-ONLY (e2e targets need local
  server fixtures); record exit code.
- `cargo test -p camel-direct --lib` — RUN; record exit code.
- `cargo test -p camel-http --lib` — RUN; record exit code.
- `cargo test -p camel-component-seda --lib` — RUN; record exit code.

**Acceptance:**
- 63 markers added; comments only; fmt clean.
- `cargo clippy -p camel-processor -p camel-component-mcp -p camel-direct -p camel-http -p camel-component-seda -p camel-component-api -- -D warnings`
  exit 0.
- Saved lint output: these crates contribute 0 findings for their rows.

- [x] 3.3

### Task 3.4: Mark rabbitmq, sql, redis, validator, xslt, core, lsp, bridge, cron, controlbus, file, ws, opensearch sites

**Files:**
- all files under `crates/components/camel-rabbitmq/`, `crates/components/camel-sql/`,
  `crates/components/camel-redis/`, `crates/components/camel-validator/`,
  `crates/components/camel-xslt/`, `crates/camel-core/`, `crates/camel-lsp/`,
  `crates/services/camel-bridge/`, `crates/components/camel-cron/`,
  `crates/components/camel-controlbus/`, `crates/components/camel-file/`,
  `crates/components/camel-ws/`, `crates/components/camel-opensearch/` with B rows in the
  TSVs (29 rows: 9 + 20) (all modified)

**Steps:**
1. Pull rows: `grep -E 'camel-rabbitmq|camel-sql|camel-redis|camel-validator|camel-xslt|camel-core|camel-lsp|camel-bridge|camel-cron|camel-controlbus|camel-file|camel-ws|camel-opensearch'
   openspec/changes/unbounded-wait-ratchet/classify/*.tsv | grep '	B	'` (29 rows).
2. Add markers per shared conventions, TSV reasons verbatim (dominant patterns:
   connection/engine-owned channels in rabbitmq, pool/consumer harness channels in sql,
   lifecycle adapter test channels in camel-core, engine-driven task joins in camel-bridge).
   Comments only.

**Tests:**
- `cargo xtask lint-unbounded-wait 2>&1 | tee /home/shared/tmp/394-lint-t34.txt; echo EXIT:${PIPESTATUS[0]}` — the saved
  file's count line reads 0 findings, `EXIT:0` (all 299 rows adjudicated: 54 bounded + 245 marked).
  Count 0 is below the untouched ceiling 296, so the gate passes here; record the count
  line + exit status in the commit body. The LOWERING to 0 happens in 4.1.
- `cargo test -p camel-core --lib` — RUN; record exit code.
- `cargo test -p camel-sql --lib` — RUN; record exit code.
- `cargo test -p camel-rabbitmq --lib` — RUN; record exit code.
- `cargo test -p camel-redis --lib` — RUN; record exit code.

**Acceptance:**
- 29 markers added; comments only; fmt clean.
- `cargo clippy -p camel-core -p camel-lsp -p camel-bridge -p camel-cron -p camel-controlbus -p camel-file -p camel-ws -p camel-opensearch -p camel-rabbitmq -p camel-sql -p camel-redis -- -D warnings`
  exit 0.
- `cargo xtask lint-unbounded-wait` reports 0 findings and exits 0 (ceiling untouched at
  296).

- [x] 3.4

## Phase 4: Ratchet lock-in + verification

### Task 4.1: Lower ceiling to 0 + header doc line

**Files:**
- `scripts/xtask/ratchet-unbounded-wait.max` (modified)

**Steps:**
1. Rewrite the file so the value line reads `0` (header comment line from 1.2 stays).
2. Run `cargo xtask lint-unbounded-wait` — must report 0 findings and exit 0.
3. Negative verification (blessed scenario "A new unbounded test wait fails the gate"):
   create an UNTRACKED scratch test file `crates/camel-test/tests/unbounded_wait_negative_scratch.rs`
   with one `#[tokio::test]` fn containing a bare `rx.recv().await` (no bound, no marker);
   run the lint — it MUST report the scratch site and exit non-zero; save the output line
   to the commit body evidence. Then `rm` the scratch file and re-run the lint — exit 0.
   (Scratch never committed; evidence lives in the 4.1 commit body since the ceiling change
   is itself a real committed modification.)

**Tests:**
- command: `cargo xtask lint-unbounded-wait 2>&1 | tee /home/shared/tmp/394-lint-t41a.txt; echo EXIT:${PIPESTATUS[0]}`
  — expected count 0, `EXIT:0`.
- command: with scratch present: `cargo xtask lint-unbounded-wait > /home/shared/tmp/394-lint-t41b.txt 2>&1; echo EXIT:$?`
  — expected: scratch site listed, `EXIT:1` (or non-zero).
- command: after `rm` scratch: same command — expected count 0, `EXIT:0`.
- command: `cat scripts/xtask/ratchet-unbounded-wait.max` — expected: header comment + `0`.
- expected: all three lint runs behave as stated.

**Acceptance:**
- Ceiling value 0; header doc line present (from 1.2); lint exits 0 on the clean tree.
- Negative verification recorded in the 4.1 commit body (scratch site flagged at ceiling 0,
  non-zero exit; clean tree exits 0 after removal).
- Ceiling never raised anywhere in the branch history (`git log -p -- scripts/xtask/ratchet-unbounded-wait.max`
  shows 296 → 0 monotone).

- [x] 4.1

### Task 4.2: Final verification + evidence table

**Files:**
- `openspec/changes/unbounded-wait-ratchet/evidence-table.md` (new)

**Steps:**
1. Generate the mission's evidence table: file → sites → bucket → action, aggregated from
   the three TSVs — one row per DISTINCT file (generate the count from the TSVs with
   `cut -d: -f1 group-*.tsv | sort -u | wc -l`; plan review measured 98 distinct files —
   the table must match whatever the TSVs actually contain), with the A/B/C totals header
   (54/245/0).
2. Run the aggregate suite: `cargo test --workspace --lib` (contained run; record exit).
3. Run the touched-crate test targets recorded as deferred-by-infra in Phase 2 reports
   where their infra IS available locally; list any still-deferred as
   `integration-verification-deferred-to-CI` in the evidence table.
4. Commit the table.

**Tests:**
- `cargo test --workspace --lib` — RUN; record exact exit code (no pipes).
- `cargo xtask lint-unbounded-wait > /home/shared/tmp/394-lint-t42.txt 2>&1; echo EXIT:$?`
  — expected count 0, `EXIT:0` (unchanged from 4.1).
- expected: evidence table complete (98 distinct-file rows or the exact generated count,
  totals 54/245/0).

**Acceptance:**
- `cargo test --workspace --lib` exits 0.
- Evidence table committed with the generated distinct-file count and totals matching the
  TSVs exactly (54/245/0).
- Deferred-infra list explicit (no silent skips).

- [x] 4.2
