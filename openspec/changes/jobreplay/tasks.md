# Tasks: jobreplay

Execution order is SERIAL: Task 1.1 → Task 1.2 → Task 1.3 (1.2 and 1.3
assert behavior that only exists after 1.1 lands; their "fails before
Task 1.1" notes are historical TDD evidence, not instructions to
re-observe red once 1.1 is in).

## camel-cli / commands/job

### Task 1.1: Exclude the SEDA gate from job send retry classification and invert the pinned characterization

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/commands/job/startup_retry_classification_tests.rs` (modified)

**Steps:**
1. In `crates/camel-cli/src/commands/job/mod.rs`, replace the body of
   `is_retryable_startup_failure` with
   `camel_component_seda::is_direct_startup_race(e)` (the seda crate's owned
   discriminator: `EndpointCreationFailed` AND not the gate wordings —
   re-exported already; the current explicit `is_no_active_consumers_gate(e)`
   disjunct and `matches!` are replaced by the single delegation).
2. Rewrite the doc comment on `is_retryable_startup_failure`: drop the
   "seda disjunct is subsumed… kept as ownership marker" note and the
   retryable-gate claim; state the new contract — the SEDA gate wordings
   are NON-retryable because the rejection fires pre-enqueue but inside the
   caller's pipeline (rc-ucemm), every non-gate `EndpointCreationFailed`
   stays retryable (direct registration race plus the documented residual:
   SEDA queue-full and bounded enqueue/fanout timeout errors share the
   variant), referencing the rc-tgaxf fail-fast ruling.
3. Rewrite the doc comment on `send_with_startup_retry`: remove the false
   premise "safe to retry here because a gate rejection is pre-enqueue and
   the job send is the first and only send, so no side effects can have
   run"; state that gate errors return `SendError::Pipeline` on the first
   attempt without sleeping, and that non-gate `EndpointCreationFailed`
   (the direct registration race family and the queue-full residual) remain
   retried every `SEND_RETRY_SLEEP` up to `SEND_RETRY_WINDOW`.
4. In `startup_retry_classification_tests.rs`, invert the two pinned gate
   tests: rename `seda_single_mode_gate_is_retryable` →
   `seda_single_mode_gate_is_not_retryable` asserting
   `!is_retryable_startup_failure(&gate_single())` with a message naming
   rc-ucemm (side-effect duplication); rename `seda_fanout_gate_is_retryable`
   → `seda_fanout_gate_is_not_retryable` likewise for `gate_fanout()`.
5. Update the module doc header of `startup_retry_classification_tests.rs`
   and the helper comments on `gate_single`/`gate_fanout` where they state
   the gate is retryable / pre-enqueue-safe.
6. Add one new characterization test `seda_queue_full_is_retryable`:
   `CamelError::EndpointCreationFailed("SEDA queue 'jobs' is full (size=10)".into())`
   asserts `is_retryable_startup_failure(&e)` is true, with a comment
   marking it the documented residual (bd rc-ucemm scope excludes only the
   no-active-consumer/subscriber gates).

**Tests:** (executable spec)
- `seda_single_mode_gate_is_not_retryable`: `gate_single()` builds the single-mode wording → call `is_retryable_startup_failure` → assert `false`. Command: `cargo test -p camel-cli --lib commands::job::startup_retry_classification_tests`. Expected: fails before step 1 lands (old classification returns true), passes after.
- `seda_fanout_gate_is_not_retryable`: `gate_fanout()` (fanout wording) → same call → assert `false`. Same command and expected.
- `seda_queue_full_is_retryable`: queue-full wording under the same variant → assert `true`. Same command; fails only if the exclusion is over-broad.
- `direct_not_registered_race_is_retryable` (existing, unchanged): direct race wording → assert `true`. Same command.
- `generic_endpoint_creation_failure_is_retryable` (existing, unchanged): `EndpointCreationFailed("unsupported option \`nope\`")` → assert `true`. Same command.
- The five existing non-retryable tests (`function_not_registered_stays_non_retryable`, `generic_pipeline_failure_stays_non_retryable`, `io_failure_stays_non_retryable`, `component_not_found_stays_non_retryable`, `wrapped_not_registered_text_is_not_sniffed`) unchanged and still passing. Same command.

**Acceptance:**
- `cargo test -p camel-cli --lib commands::job::startup_retry_classification_tests` exits 0 with all tests above green.
- `grep -n "is_no_active_consumers_gate" crates/camel-cli/src/commands/job/mod.rs` returns nothing (after delegation the classifier no longer references the gate predicate at all — the old disjunct was split across lines, so a line-based grep on the full call is the reliable removal check).
- `cargo fmt --check --all` exits 0; `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: In-process adversarial test — pre-SEDA side effect executes exactly once through `send_with_startup_retry`

**Files:**
- `crates/camel-cli/src/commands/job/startup_retry_pipeline_tests.rs` (new)
- `crates/camel-cli/src/commands/job/mod.rs` (modified — register the module)

**Steps:**
1. Create `startup_retry_pipeline_tests.rs` with a module doc: adversarial
   pipeline tests for the rc-ucemm contract — the SEDA gate must fail fast
   with exactly one execution of pre-SEDA side effects.
2. Register in `mod.rs` test-module block (beside
   `startup_retry_classification_tests`): `#[cfg(test)] mod startup_retry_pipeline_tests;`.
3. Test `gate_fails_fast_with_exactly_one_side_effect` — MUST use plain
   `#[tokio::test]` (the crate also uses `#[tokio::test(start_paused =
   true)]` in batch_drain_tests.rs; `start_paused` is FORBIDDEN here
   because auto-advancing sleeps would compress the old code's 3 s spin
   under the elapsed bound and void the fail-fast discriminator):
   - Build a `CamelContext` via
     `CamelContext::builder().build().await.expect("context build")`.
   - Register `camel_component_direct`'s direct component,
     `camel_component_mock::MockComponent::new()`, and
     `camel_component_seda::SedaComponent::new()` (keep a clone of the
     mock handle for assertions).
   - Build the route via `camel_dsl::parse_routes_with_env` on inline YAML
     with an ambient env closure returning `None`:
     route `from direct:jobs` with steps `[to mock:counted, to seda:worker]`
     — NO route consumes `seda:worker`.
   - `ctx.add_route_definition(def).await.expect("add route")` (it returns
     `Result<_, CamelError>`), then
     `ctx.start().await.expect("context start")`.
   - Call `send_with_startup_retry(&ctx, &send, "direct:jobs", &[])` with
     `send` a `document::JobSendAction { to: "direct:jobs", body:
     Some(JobBody::Text("tick")), headers: None }`, measuring wall time
     around this call only.
   - Assert: result is `Err(SendError::Pipeline(e))` with
     `camel_component_seda::is_no_active_consumers_gate(&e)` true; elapsed
     wall time of the call is under 1 s (fail-fast — the old behavior
     spins 3 s); and the mock endpoint `counted` recorded the exchange
     EXACTLY once: `mock.get_endpoint("counted")` returns
     `Option<Arc<MockEndpointInner>>` — `.expect("counted endpoint")`,
     then `inner.expect_count(1); inner.assert_satisfied().await;`
     (`expect_count` is `CountBound::Exact` per inner.rs:211;
     `assert_satisfied` is async).
4. Test `fanout_gate_fails_fast_with_exactly_one_side_effect`: identical
   fixture and assertions, except the seda step is
   `to seda:worker?multipleConsumers=true` (fanout mode — URI param per
   seda's parameter table, `multipleConsumers`). With no subscribers the
   gate fires the fanout wording; assert the gate error is the
   no-active-SUBSCRIBERS form (`is_no_active_consumers_gate` still true),
   elapsed < 1 s, mock count exactly one. This gives the fanout spec
   scenario send-loop coverage, not just classifier coverage.
5. Test `discard_if_no_consumers_proceeds_without_gate_error`:
   same fixture but the seda step is `to seda:worker?discardIfNoConsumers=true`
   (verify the exact URI parameter name in camel-component-seda's
   parameter parsing before writing the fixture). Assert the send returns
   `Ok` (exchange discarded at the seda producer — no
   `EndpointCreationFailed` ever reaches the classifier) and the mock
   endpoint still recorded exactly one exchange.

**Tests:** (executable spec)
- `gate_fails_fast_with_exactly_one_side_effect`: booted ctx, route `direct:jobs → mock:counted → seda:worker`, no seda consumer → `send_with_startup_retry` → `Err(Pipeline(gate))`, elapsed < 1 s, mock count exactly 1. Command: `cargo test -p camel-cli --lib commands::job::startup_retry_pipeline_tests`. Expected (historical, pre-1.1): fails with count > 1 and elapsed ≈ 3 s.
- `fanout_gate_fails_fast_with_exactly_one_side_effect`: seda step `seda:worker?multipleConsumers=true`, no subscribers → `Err(Pipeline(gate))` (fanout wording), elapsed < 1 s, mock count exactly 1. Same command. Expected (historical, pre-1.1): fails likewise.
- `discard_if_no_consumers_proceeds_without_gate_error`: same route with `discardIfNoConsumers=true` on the seda step → send returns `Ok`, mock count exactly 1. Same command; independent of Task 1.1 (discard path never classifies).

**Acceptance:**
- `cargo test -p camel-cli --lib commands::job::startup_retry_pipeline_tests` exits 0.
- No `tokio::time::sleep` or polling loops added in test code beyond reading mock state once after the send settles (mock receipt of the single exchange may need a short bounded wait for the consumer-side processing — if so, use one bounded readiness helper with a deadline, not an unbounded loop; `cargo xtask lint-test-sleep` and `lint-unbounded-wait` stay green).
- `cargo fmt --check --all` exits 0; `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: Subprocess end-to-end test — exactly one side-effect write before a never-activating seda consumer

**Files:**
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified — add one test; do NOT create a new integration-test binary, the binary count is gate-tracked)

**Steps:**
1. Add test `seda_gate_side_effect_executes_exactly_once` to
   `job_one_shot_test.rs`, following the file's established fixture
   pattern (`write_config`, tempdir, `routes/job-route.yaml`, job doc,
   `run_job`).
2. Fixture route YAML — `from direct:jobs` with steps
   `[to file:{dir}?fileName=count.txt&fileExist=append, to seda:worker]`
   (substitute `{dir}` with the tempdir path; the file component's
   parameter is `fileExist` with value `append` — see
   `FileExistStrategy::from_str`). NO route consumes `seda:worker`.
3. Job doc: one-shot, `timeout: 60s`, send to `direct:jobs`, body `tick`,
   no capture-reply.
4. Assertions on `(code, stdout, stderr)`:
   - `code == 1` (pipeline failure class).
   - Parsed report: `outcome == "Failed"`, `error` contains
     `has no active consumers`.
   - Read `count.txt` after process exit (the pipeline's file write is
     synchronous with the send — a plain read suffices): exact content
     `== "tick"` (two executions would read `"ticktick"`, three
     `"tickticktick"` — exact-bytes is the only discriminator a line
     count cannot make). The timing proof (fail-fast vs the old 3 s spin)
     lives in Task 1.2 where elapsed wraps `send_with_startup_retry`
     directly; report `duration_ms` is boot-dominated and NOT asserted
     here (flaky on loaded CI).
5. Keep the test self-contained: no env vars, no network, deterministic.

**Tests:** (executable spec)
- `seda_gate_side_effect_executes_exactly_once`: fixture above → `camel job` subprocess → exit 1, `Failed` report naming the gate, `count.txt` bytes exactly `"tick"`. Command: `cargo build -p camel-cli && cargo test -p camel-cli --test job_one_shot_test seda_gate_side_effect_executes_exactly_once`. Expected (historical, pre-1.1): fails with `"tick"` repeated ~150×.

**Acceptance:**
- `cargo test -p camel-cli --test job_one_shot_test` exits 0 (all tests in the binary, old and new).
- `git diff --name-status 34222b47 -- crates/camel-cli/tests` shows exactly one line: `M	crates/camel-cli/tests/job_one_shot_test.rs` (no new integration-test binary).
- `cargo fmt --check --all` exits 0; `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.3
