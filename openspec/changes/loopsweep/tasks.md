# Tasks: loopsweep

## Conversion recipes (referenced by every task; definitions, not deferral)

- **R-P1 (enclosing timeout)** — wrap the whole loop:
  `tokio::time::timeout(BUDGET, async { ORIGINAL_LOOP }).await.expect(MSG);`
  where BUDGET is the budget constant chosen per the budget rule below,
  ORIGINAL_LOOP is the original loop verbatim (internal break/condition
  logic unchanged; if the loop produced a value via `break v`, the async
  block evaluates to that same type so the binding keeps its type), and
  MSG is a message naming WHAT was awaited and the concrete BUDGET
  value, e.g. `"route context must reach steady state within 3s"` for a
  2s-deadline site (deadline + 50%). The `.expect`
  fires only when awaited progress never arrives. Existing internal
  `Instant` deadline asserts stay (fast-fail context).
- **R-P3 (retry-wrap)** — for `let _r: Result<(), ()> = loop { .. break
  Ok/Err .. }` retry loops: apply R-P1 to the loop so `_r` keeps its
  type via `timeout(BUDGET, async { loop }).await.expect("retry loop
  must finish within 30s")`. Retry logic, counters, and assertions stay
  untouched.
- **R-M (marker)** — append to the `loop {` line exactly:
  `// allow-test-wait: spawned test-server accept loop; teardown-bounded by test runtime (ADR-0069 §13.2 R1)`.
  No code change.
- **Budget rule**: the site's existing deadline constant + 50% margin;
  else 30× the loop's poll interval; floor 10s. Prefer `Duration::from_secs`
  literals matching neighboring test constants; name the concrete value
  in MSG.
- **Per-task count check**: from the worktree root run
  `/home/shared/rust-camel-fleet-bin/xtask lint-unbounded-wait`; the
  printed findings number must equal the task's stated post-task count.
  The ceiling stays 548 until Phase 2 (findings below ceiling = OK).
- **Infra deferral**: if a test target requires external infra (Redis
  Sentinel, Kafka, JMS broker, SQL server) and is not runnable locally,
  verify with `cargo test -p CRATE --test TARGET --no-run` and record
  "integration-verification-deferred-to-CI" in the task result.

## Phase 1: Convert all 37 loop sites per-shape

### camel-core

#### Task 1.1: Bound 3 await-carrying loops in camel-core tests (R-P1)

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified)
- `crates/camel-core/tests/runtime_consistency_test.rs` (modified)

**Steps:**
1. `route_controller_tests.rs:4021` in `context_start_does_not_fail_fast_on_immediate_error`:
   wrap the readiness loop (R-P1) with budget from the test's existing
   deadline/interval constants (deadline + 50%); MSG names
   `"route context must reach steady state"` plus the concrete budget.
2. `runtime_consistency_test.rs:300` in
   `supervision_restart_reflects_crash_recovery_progress_in_runtime_projection`:
   R-P1 with the test's existing deadline constant + 50%; MSG names
   crash-recovery progress.
3. `runtime_consistency_test.rs:357` in
   `supervision_respects_runtime_stopped_state_and_skips_restart`:
   R-P1 same budget rule; MSG names the stopped-state observation.
4. `cargo fmt` the two files.

**Tests:**
- `context_start_does_not_fail_fast_on_immediate_error`: lib tests build with the wrapped loop → `cargo test -p camel-core --lib context_start_does_not_fail_fast_on_immediate_error` → passes (readiness arrives normally; success path unchanged).
- `supervision_restart_reflects_crash_recovery_progress_in_runtime_projection`: `cargo test -p camel-core --test runtime_consistency_test supervision_restart_reflects_crash_recovery_progress_in_runtime_projection` → passes.
- `supervision_respects_runtime_stopped_state_and_skips_restart`: `cargo test -p camel-core --test runtime_consistency_test supervision_respects_runtime_stopped_state_and_skips_restart` → passes.
- Lint: `/home/shared/rust-camel-fleet-bin/xtask lint-unbounded-wait` reports findings = 545 (548 − 3).

**Acceptance:**
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- The three targeted tests pass (or target is recorded infra-deferred with `--no-run` proof).
- Fleet lint findings number = 545.

- [x] 1.1

### camel-test

#### Task 1.2: Bound 5 loops + mark 2 accept-loop servers in camel-test tests

**Files:**
- `crates/camel-test/tests/redis_repositories_test.rs` (modified)
- `crates/camel-test/tests/redis_sentinel_test.rs` (modified)
- `crates/camel-test/tests/seda_test.rs` (modified)
- `crates/camel-test/tests/component_emission_test.rs` (modified)
- `crates/camel-test/tests/http_test.rs` (modified)

**Steps:**
1. `redis_repositories_test.rs:1332` in
   `cache_sentinel_failover_reauth_live`: R-P1, budget =
   `FAILOVER_DEADLINE` (or the test's deadline constant) + 50%; MSG names
   `"cache must re-auth against the new master"` plus the concrete budget.
2. `redis_sentinel_test.rs:508` in `producer_recovers_after_sentinel_failover`:
   R-P1, budget = `FAILOVER_DEADLINE` + 50%.
3. `redis_sentinel_test.rs:577` in `queue_consumer_recovers_after_sentinel_failover`:
   R-P1, same budget rule.
4. `redis_sentinel_test.rs:650` in `pubsub_consumer_resubscribes_after_sentinel_failover`:
   R-P1, same budget rule.
5. `seda_test.rs:305` in `seda_single_consumer_survives_suspend_resume`:
   R-P1 from the test's existing poll interval ×30 (floor 10s).
6. `component_emission_test.rs:663` in `http_roundtrip_observed`: R-M
   (spawned accept-loop test server). Before appending the marker,
   confirm the R1 bounding properties: the owning test's client-side
   readiness assertions are bounded (they assert on responses from its
   own requests, which fail or return), and teardown is the test
   runtime drop aborting the spawned accept task.
7. `http_test.rs:587` in `http_custom_ok_status_code_range`: R-M
   (spawned accept-loop test server); same R1 bounding confirmation —
   bounded client assertions, teardown via runtime drop.
8. `cargo fmt` the five files.

**Tests:**
- `seda_single_consumer_survives_suspend_resume`: `cargo test -p camel-test --test seda_test seda_single_consumer_survives_suspend_resume` → passes (suspends/resumes arrive normally).
- `http_custom_ok_status_code_range`: `cargo test -p camel-test --test http_test http_custom_ok_status_code_range` → passes.
- `http_roundtrip_observed`: `cargo test -p camel-test --test component_emission_test http_roundtrip_observed` → passes.
- Redis-sentinel/repositories targets: infra-gated — verify `cargo test -p camel-test --test redis_sentinel_test --no-run` and `--test redis_repositories_test --no-run` compile; mark integration-verification-deferred-to-CI if not runnable locally.
- Lint: findings = 538 (545 − 7).

**Acceptance:**
- `cargo clippy -p camel-test --all-targets -- -D warnings` exits 0.
- Runnable targeted tests pass; redis targets at least compile.
- Fleet lint findings number = 538.

- [x] 1.2

### camel-integration-test + camel-direct

#### Task 1.3: Bound 4 loops (itest partner/log + direct ×2) (R-P1)

**Files:**
- `crates/camel-integration-test/src/http_partner_test.rs` (modified)
- `crates/camel-integration-test/tests/log_assertion_test.rs` (modified)
- `crates/components/camel-direct/src/direct_tests.rs` (modified)

**Steps:**
1. `http_partner_test.rs:504` in `plain_string_send_dials_literal_without_partner`:
   wrap the request-head read loop INSIDE the spawned `served` task
   (R-P1); budget 30s; MSG names `"partner must receive the complete
   request head"`. A timeout panic inside the spawn propagates when the
   test awaits the join handle — verify the test awaits `served` and
   keep that await.
2. `log_assertion_test.rs:232` in `multi_thread_document_fails_on_warn`:
   R-P1; budget 30× the test's poll sleep (floor 10s); MSG names the
   warn-log arrival.
3. `direct_tests.rs:555` in `test_direct_crashed_consumer_entry_is_overwritable`:
   R-P1 from the test's existing deadline/interval constants.
4. `direct_tests.rs:1158` in `test_direct_cycle_never_succeeds_or_hangs`:
   R-P1 from the test's existing constants; MSG preserves the test's
   "must terminate, not hang" intent.
5. `cargo fmt` the three files.

**Tests:**
- `plain_string_send_dials_literal_without_partner`: `cargo test -p camel-integration-test --lib plain_string_send_dials_literal_without_partner` → passes.
- `multi_thread_document_fails_on_warn`: `cargo test -p camel-integration-test --test log_assertion_test multi_thread_document_fails_on_warn` → passes.
- `test_direct_crashed_consumer_entry_is_overwritable` and `test_direct_cycle_never_succeeds_or_hangs`: `cargo test -p camel-direct --lib test_direct_crashed_consumer_entry_is_overwritable test_direct_cycle_never_succeeds_or_hangs` (two filters → two runs if needed) → both pass.
- Lint: findings = 534 (538 − 4).

**Acceptance:**
- `cargo clippy -p camel-integration-test -p camel-direct --all-targets -- -D warnings` exits 0.
- The four targeted tests pass (or infra-deferred with `--no-run` proof).
- Fleet lint findings number = 534.

- [x] 1.3

### camel-master

#### Task 1.4: Bound 5 loops in camel-master tests (R-P1)

**Files:**
- `crates/components/camel-master/src/tests/acquisition_budget.rs` (modified)
- `crates/components/camel-master/src/tests/delegate_errors.rs` (modified)
- `crates/components/camel-master/src/tests/leadership_state_gauge.rs` (modified)
- `crates/components/camel-master/src/tests/stale_stamp.rs` (modified)

**Steps:**
1. `acquisition_budget.rs:65` in `term_bump_at_exhausted_budget_reacquires_fresh`:
   R-P1; budget 30× the loop's sleep interval (floor 10s); MSG names
   budget reacquisition.
2. `delegate_errors.rs:39` in `delegate_permanent_error_terminates_master_without_retry`:
   R-P1; MSG preserves "terminates without retry" intent.
3. `leadership_state_gauge.rs:336` in `term_bump_while_active_reconciles_once`:
   R-P1; MSG names the reconcile observation.
4. `stale_stamp.rs:74` in `tick_renews_epoch_advance_restamps_without_delivery`:
   R-P1.
5. `stale_stamp.rs:213` in `dead_delegate_stale_stamp_resets_budget`: R-P1.
6. `cargo fmt` the four files.

**Tests:**
- `term_bump_at_exhausted_budget_reacquires_fresh`: `cargo test -p camel-master --lib term_bump_at_exhausted_budget_reacquires_fresh` → passes.
- `delegate_permanent_error_terminates_master_without_retry`: `cargo test -p camel-master --lib delegate_permanent_error_terminates_master_without_retry` → passes.
- `term_bump_while_active_reconciles_once`: `cargo test -p camel-master --lib term_bump_while_active_reconciles_once` → passes.
- `tick_renews_epoch_advance_restamps_without_delivery` and `dead_delegate_stale_stamp_resets_budget`: `cargo test -p camel-master --lib stale_stamp` → passes.
- Lint: findings = 529 (534 − 5).

**Acceptance:**
- `cargo clippy -p camel-master --all-targets -- -D warnings` exits 0.
- The five targeted tests pass.
- Fleet lint findings number = 529.

- [x] 1.4

### camel-jms + camel-kafka

#### Task 1.5: Bound 2 sleep-polls (jms) + wrap 2 retry loops (jms, kafka)

**Files:**
- `crates/components/camel-jms/src/component.rs` (modified)
- `crates/components/camel-jms/src/consumer.rs` (modified)
- `crates/components/camel-kafka/src/consumer.rs` (modified)

**Steps:**
1. `jms component.rs:1781` in `pool_shutdown_awaits_health_monitor`:
   R-P1; budget 30× the loop's sleep (floor 10s); MSG names
   health-monitor shutdown.
2. `jms component.rs:2173` in `test_jms_bridge_pool_drop_cleans_up_slots`:
   R-P1; MSG names slot cleanup.
3. `jms consumer.rs:898` in
   `retry_loop_invokes_operation_exactly_max_attempts_times`: R-P3;
   overall budget 30s; the `calls` assertion stays byte-identical.
4. `kafka consumer.rs:1437` in
   `retry_loop_invokes_operation_exactly_max_attempts_times`: R-P3;
   overall budget 30s; assertions untouched.
5. `cargo fmt` the three files.

**Tests:**
- `pool_shutdown_awaits_health_monitor`: `cargo test -p camel-jms --lib pool_shutdown_awaits_health_monitor` → passes (broker-independent unit).
- `test_jms_bridge_pool_drop_cleans_up_slots`: `cargo test -p camel-jms --lib test_jms_bridge_pool_drop_cleans_up_slots` → passes; if broker-gated, `--no-run` + record deferral.
- `retry_loop_invokes_operation_exactly_max_attempts_times` (jms): `cargo test -p camel-jms --lib retry_loop_invokes_operation_exactly_max_attempts_times` → passes with `calls == max_attempts` unchanged.
- `retry_loop_invokes_operation_exactly_max_attempts_times` (kafka): `cargo test -p camel-component-kafka --lib retry_loop_invokes_operation_exactly_max_attempts_times` → passes (if kafka lib tests are infra-gated, `--no-run` + deferral note).
- Lint: findings = 525 (529 − 4).

**Acceptance:**
- `cargo clippy -p camel-jms --all-targets -- -D warnings` exits 0; `cargo clippy -p camel-component-kafka --all-targets -- -D warnings` exits 0.
- Runnable targeted tests pass; others compile with deferral recorded.
- Fleet lint findings number = 525.

- [x] 1.5

### camel-sql + camel-container

#### Task 1.6: Wrap 3 attempt-bounded retry loops (R-P3)

**Files:**
- `crates/components/camel-sql/src/consumer.rs` (modified)
- `crates/components/camel-sql/src/producer.rs` (modified)
- `crates/components/camel-container/src/lib.rs` (modified)

**Steps:**
1. `sql consumer.rs:1678` in
   `retry_loop_invokes_operation_exactly_max_attempts_times`: R-P3;
   budget 30s; assertions untouched.
2. `sql producer.rs:1298` in
   `retry_loop_invokes_operation_exactly_max_attempts_times`: R-P3;
   budget 30s.
3. `container lib.rs:2668` in
   `retry_loop_invokes_operation_exactly_max_attempts_times`: R-P3;
   budget 30s.
4. `cargo fmt` the three files.

**Tests:**
- `retry_loop_invokes_operation_exactly_max_attempts_times` (sql consumer): `cargo test -p camel-sql --lib retry_loop_invokes_operation_exactly_max_attempts_times` → passes, `calls` count assertion unchanged.
- Same name (sql producer): same command (same lib) → passes.
- Same name (container): `cargo test -p camel-container --lib retry_loop_invokes_operation_exactly_max_attempts_times` → passes.
- Lint: findings = 522 (525 − 3).

**Acceptance:**
- `cargo clippy -p camel-sql -p camel-container --all-targets -- -D warnings` exits 0.
- The three targeted tests pass.
- Fleet lint findings number = 522.

- [x] 1.6

### camel-ws + camel-xj + camel-xslt

#### Task 1.7: Wrap 4 attempt-bounded retry loops (R-P3)

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified)
- `crates/components/camel-xj/src/component.rs` (modified)
- `crates/components/camel-xslt/src/component.rs` (modified)

**Steps:**
1. `ws lib.rs:4999` in
   `retry_loop_invokes_operation_exactly_max_attempts_times`: R-P3;
   budget 30s.
2. `ws lib.rs:5041` in
   `retry_loop_with_max_attempts_1_invokes_operation_once`: R-P3;
   budget 30s; the "invokes once" assertion stays byte-identical.
3. `xj component.rs:604` in
   `retry_loop_invokes_operation_exactly_max_attempts_times`: R-P3.
4. `xslt component.rs:356` in
   `retry_loop_invokes_operation_exactly_max_attempts_times`: R-P3.
5. `cargo fmt` the three files.

**Tests:**
- `retry_loop_invokes_operation_exactly_max_attempts_times` (ws): `cargo test -p camel-ws --lib retry_loop_invokes_operation_exactly_max_attempts_times` → passes.
- `retry_loop_with_max_attempts_1_invokes_operation_once` (ws): same command, second filter → passes.
- `retry_loop_invokes_operation_exactly_max_attempts_times` (xj): `cargo test -p camel-xj --lib retry_loop_invokes_operation_exactly_max_attempts_times` → passes.
- `retry_loop_invokes_operation_exactly_max_attempts_times` (xslt): `cargo test -p camel-xslt --lib retry_loop_invokes_operation_exactly_max_attempts_times` → passes.
- Lint: findings = 518 (522 − 4).

**Acceptance:**
- `cargo clippy -p camel-ws -p camel-xj -p camel-xslt --all-targets -- -D warnings` exits 0.
- The four targeted tests pass.
- Fleet lint findings number = 518.

- [x] 1.7

### camel-component-grpc + camel-component-seda + camel-redis + camel-template + loadgen

#### Task 1.8: Bound 4 loops + mark 3 accept-loop servers

**Files:**
- `crates/components/camel-component-grpc/tests/server_auth_test.rs` (modified)
- `crates/components/camel-component-seda/src/lib.rs` (modified)
- `crates/components/camel-redis/src/topology_tests.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)
- `crates/components/camel-template/tests/common/mod.rs` (modified)
- `benchmarks/harness/loadgen/src/cli_runtime.rs` (modified)

**Steps:**
1. `grpc server_auth_test.rs:569` in
   `grpc_client_streaming_pipeline_denial_regression`: wrap the loop
   INSIDE the spawned pipeline task (R-P1); budget 30s; MSG names
   `"pipeline must observe the completion marker"` — expiry is failure
   (silent break would pass without the completion exchange).
2. `camel-component-seda lib.rs:3008` in
   `fanout_gauge_stays_positive_while_blocked_subscriber_has_backlog`:
   R-P1 from the test's existing deadline constant + 50%.
3. `camel-redis topology_tests.rs:709` in
   `sentinel_resolve_carries_username_to_master_connection`: R-P1 from
   the test's existing deadline/interval constants.
4. `camel-redis executor.rs:880` in `cached_fast_path_skips_gate`: R-M
   (spawned stub-server accept loop); confirm R1 bounding properties
   first — the test's cache assertions are bounded, teardown via
   runtime drop.
5. `camel-template tests/common/mod.rs:166` in
   `retry_loop_eventually_succeeds` (a `#[tokio::test]` inside the
   shared `tests/common` module, compiled into the
   `template_reload_integration` target): R-P1 from the helper's
   existing deadline constant + 50%; MSG names eventual success.
6. `loadgen cli_runtime.rs:810` in
   `warmup_drive_request_deadline_stops_new_requests`: R-M (spawned
   accept-loop stub server); confirm R1 bounding first — the warmup
   client itself asserts on deadline-stopped request counts.
7. `loadgen cli_runtime.rs:874` in
   `warmup_drive_body_deadline_stops_inflight_request`: R-M; same R1
   bounding confirmation.
8. `cargo fmt` the six files.

**Tests:**
- `grpc_client_streaming_pipeline_denial_regression`: `cargo test -p camel-component-grpc --test server_auth_test grpc_client_streaming_pipeline_denial_regression` → passes.
- `fanout_gauge_stays_positive_while_blocked_subscriber_has_backlog`: `cargo test -p camel-component-seda --lib fanout_gauge_stays_positive_while_blocked_subscriber_has_backlog` → passes.
- `cached_fast_path_skips_gate`: `cargo test -p camel-redis --lib cached_fast_path_skips_gate` → passes.
- `retry_loop_eventually_succeeds` (template common): `cargo test -p camel-template --test template_reload_integration retry_loop_eventually_succeeds` → passes.
- `sentinel_resolve_carries_username_to_master_connection`: `cargo test -p camel-redis --lib sentinel_resolve_carries_username_to_master_connection` → passes; if sentinel-env-gated, `--no-run` + deferral.
- Loadgen tests `warmup_drive_request_deadline_stops_new_requests` and `warmup_drive_body_deadline_stops_inflight_request`: `cargo test -p bench-loadgen --lib warmup_drive` (package name is `bench-loadgen`; `cli_runtime.rs` is src) → both pass.
- Lint: findings = 511 (518 − 7).

**Acceptance:**
- `cargo clippy -p camel-component-grpc -p camel-component-seda -p camel-redis -p camel-template --all-targets -- -D warnings` exits 0; `cargo clippy -p bench-loadgen --all-targets -- -D warnings` exits 0.
- Runnable targeted tests pass; others compile with deferral recorded.
- Fleet lint findings number = 511.

- [x] 1.8

## Phase 2: Lower the ratchet and verify

### scripts/xtask

#### Task 2.1: Lower ratchet ceiling to 511 and prune inventory

**Files:**
- `scripts/xtask/ratchet-unbounded-wait.max` (modified)

**Steps:**
1. Run `/home/shared/rust-camel-fleet-bin/xtask lint-unbounded-wait`;
   confirm the findings line reports 511 unadjudicated findings.
2. Change the integer ceiling line from `548` to `511`.
3. Delete the 37 inventory comment lines for the converted/marked
   sites (each entry has the form `# - PATH:LINE`; line numbers refer
   to the pre-change tree — match by path and proximity, not exact
   line, since Phase 1 edits shifted lines).
4. Append a dated comment line above the inventory noting the
   loopsweep burn-down (2026-09-21, bd rc-w32w7): 32 converted (23
   R-P1 + 9 R-P3), 5 R-M markers.
5. Re-run the lint; it must print `OK (511 findings = max 511)`.

**Tests:**
- Lint self-consistency: `/home/shared/rust-camel-fleet-bin/xtask lint-unbounded-wait` → exit 0, `511 findings = max 511`.
- Monotonicity guard: ceiling 511 < 548 (the file's own header rule holds).
- Inventory pruning check: `grep -c '^# - ' scripts/xtask/ratchet-unbounded-wait.max` equals 511.

**Acceptance:**
- Lint exits 0 with `511 findings = max 511`.
- Inventory entry count = 511 (integer line equals entry count).

- [x] 2.1

### worktree-wide verification

#### Task 2.2: Phase-exit verification sweep

**Files:** (no source changes; verification only)

**Steps:**
1. `cargo fmt --check --all` from the worktree root.
2. `cargo clippy --workspace --all-features --exclude camel-cli
   --exclude camel-component-kafka --exclude security-keycloak
   --exclude security-wasm-policy -- -D warnings` plus the per-crate
   arms from AGENTS.md `## QUALITY GATES` (kafka `--all-targets`, cli).
3. `cargo build --workspace` (local build gate).
4. `cargo test --workspace --lib` and
   `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`.
5. Fleet lints relevant to a test-wait diff:
   `lint-unbounded-wait`, `lint-test-sleep`, `lint-ignore`,
   `lint-unwrap`, `lint-cancel-tokens`, `lint-log-levels`.
6. Fail-loud structural check (delta-spec scenario 1, satisfied by
   construction and verified structurally): for each of the 23 R-P1 and
   9 R-P3 sites, `git diff main...HEAD` must show the conversion as
   `timeout(` wrapping the loop with a trailing `.expect(` carrying a
   non-empty message — verify with a diff grep counting 32 matching
   expect-carrying timeout conversions; the negative path (progress
   never arrives → panic with message) is stdlib `timeout`+`expect`
   behavior, not separately executed.
7. Record every gate's exit code; for any failure, capture the first
   error lines (do not fix beyond reverting to the task loop).

**Tests:**
- Gate list: each command in Steps 1-5 exits 0 (or is recorded with a
  pre-existing-failure exemption verified against main).
- No `unwrap()` introduced by Phase 1 (lint-unwrap exit 0).

**Acceptance:**
- All five step groups green or explicitly exempted with evidence.
- Gate exit codes recorded in the task result.

- [x] 2.2
