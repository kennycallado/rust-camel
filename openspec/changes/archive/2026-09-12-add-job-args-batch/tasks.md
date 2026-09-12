# Tasks: add-job-args-batch

Implementation order: this change is implemented AFTER
fix-job-multiroute-startup — the SEDA worker tests require all document
routes to start (the old startup suppression would keep worker routes
stopped).

## camel-cli job command

### Task 2.1: `--arg` flag header injection

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)

**Steps:**
1. In `JobArgs` (mod.rs), add the field:
   `/// Repeatable NAME=VALUE pair injected as a message header at send time (applied after document headers; last occurrence wins).`
   with attribute
   `#[arg(long = "arg", value_name = "NAME=VALUE", value_parser = parse_arg_pair)]`
   and type `pub args: Vec<(String, String)>`.
2. Add `fn parse_arg_pair(raw: &str) -> Result<(String, String), String>`
   in mod.rs: split at the first `=` with `split_once('=')`; on `None`
   return `Err(format!("invalid --arg value `{raw}`: expected NAME=VALUE"))`;
   on an empty name return
   `Err(format!("invalid --arg value `{raw}`: name is empty"))`; otherwise
   return the `(name, value)` pair. An empty value is allowed.
3. Change `send_with_startup_retry` to take a fourth parameter
   `cli_args: &[(String, String)]`. After the loop that applies document
   `send.headers`, add a loop applying `cli_args` in order:
   `message.set_header(k.clone(), serde_json::Value::String(v.clone()));`.
   CLI values are applied last, so they override colliding document
   headers and a repeated name resolves to the last occurrence.
4. In `run_job`, pass `&args.args` at the `send_with_startup_retry` call
   site.

**Tests:** (write first, verify red, then implement)
- `arg_single_and_repeated_reach_route_as_headers`: setup — tempdir,
  `write_config`, route `from: "direct:transform"` with the single step
  `- transform: {simple: "${header.name}-${header.tier}"}`; job doc with
  mode one-shot, timeout 60s, capture-reply true, send to
  `direct:transform` body "x". action —
  `run_job_args(dir, &["job.job.yaml", "--arg", "name=John", "--arg", "tier=gold"])`.
  assert — exit 0; report outcome `Completed`;
  `report["reply"]["body"]` equals `"John-gold"`. Before implementation
  the flag is unknown and clap exits 2, so the test is red.
- `arg_overrides_document_header`: setup — same route shape with simple
  expression `"${header.name}"`; the job doc `send.headers` declares
  `name: Doc`. action —
  `run_job_args(dir, &["job.job.yaml", "--arg", "name=Cli"])`.
  assert — exit 0 and `report["reply"]["body"]` equals `"Cli"`.
- `malformed_arg_is_usage_error`: setup — any valid one-shot fixture.
  action — `run_job_args(dir, &["job.job.yaml", "--arg", "noequals"])`.
  assert — exit 2 and stderr contains `expected NAME=VALUE`. Repeat with
  `--arg "=value"` and assert exit 2 with stderr containing
  `name is empty`.

**Acceptance:**
- `cargo test -p camel-cli --test job_one_shot_test arg_` passes (3 tests).
- `cargo test -p camel-cli --test job_one_shot_test` full suite green.
- `cargo fmt --check` clean; `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: batch grammar acceptance (`JobMode` enum)

**Files:**
- `crates/camel-cli/src/commands/job/document.rs` (modified)
- `crates/camel-cli/src/commands/job/document_tests.rs` (modified)
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/CONTEXT.md` (modified, failure-modes rows)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)

**Steps:**
1. document.rs: add
   `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub(crate) enum JobMode { OneShot, Batch }`
   with `pub(crate) fn as_str(&self) -> &'static str` returning
   `"one-shot"` for `OneShot` and `"batch"` for `Batch`. Change
   `ExecuteSection.mode` from `String` to `JobMode`. Update the field's
   doc comment (currently "v1 accepts only `one-shot` (`batch` is
   reserved)") to name both accepted values.
2. In `parse_job_document`, change the mode match to
   `"one-shot" => JobMode::OneShot`, `"batch" => JobMode::Batch`,
   `other => return Err(JobDocError::UnsupportedMode(other.to_string()))`.
   Delete the `JobDocError::BatchReserved` variant and its `Display` arm.
3. Change the `UnsupportedMode` `Display` text to
   `"unsupported execute.mode `{mode}`: expected `one-shot` or `batch`"`.
4. mod.rs: every `JobReport` construction site renders the mode as
   `doc.execute.mode.as_str().to_string()`. Update the `JobReport.mode`
   field doc comment (currently "(`one-shot`)") to name both modes.
5. Update `crates/camel-cli/CONTEXT.md` failure-modes prose: the row
   describing `mode: batch` as a reserved load-error trigger now states
   batch is an accepted mode (one-shot and batch; other values rejected
   at load).
6. document_tests.rs: replace `batch_mode_is_reserved_and_rejected` with
   `batch_mode_parses` — the VALID_ONE_SHOT fixture with `one-shot`
   replaced by `batch` parses and `doc.execute.mode` equals
   `JobMode::Batch`. Keep `garbage_mode_is_rejected` asserting the
   `UnsupportedMode("sometimes")` variant and add an assertion that the
   rendered error message contains `one-shot` and `batch`.
7. job_one_shot_test.rs: delete `batch_mode_is_rejected_at_load` and add
   `batch_mode_loads_and_runs_direct` — a batch-mode doc over the direct
   transform fixture (`from: "direct:transform"`,
   `- set_body: {value: "job-done"}`, capture-reply true). At this task
   the runtime still runs the one-shot path for batch; assert exit 0,
   report outcome `Completed`, and `report["mode"]` equals `"batch"`.

**Tests:** as specified in steps 5 and 6 (write first, verify red, then
implement).

**Acceptance:**
- `cargo test -p camel-cli --lib` (document_tests) green.
- `cargo test -p camel-cli --test job_one_shot_test batch_` green.
- `cargo test -p camel-cli --test job_one_shot_test` full suite green.
- `cargo fmt --check` clean; `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 2.2

### Task 2.3: `BatchDepthProbe` and drain-until-empty runtime

**Files:**
- `crates/camel-cli/src/commands/job/batch.rs` (new)
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)

**Steps:**
1. Create `crates/camel-cli/src/commands/job/batch.rs` (declared from
   mod.rs with `mod batch;`) holding the drain observer: the file starts
   with a module doc comment stating the drain contract (two consecutive
   post-send zero samples per expected queue; pre-send zeros never
   satisfy; empty expected set completes immediately).
2. batch.rs: add
   `#[derive(Default)] struct BatchDepthProbeState { zero_run: u32 }`
   and `struct BatchDepthProbe { expected: HashSet<String>, queues: Mutex<HashMap<String, BatchDepthProbeState>> }`
   with `BatchDepthProbe::new(expected: HashSet<String>) -> Self`. All
   items `pub(crate)` where mod.rs needs them. Use the repo lock-poison
   pattern for every `Mutex::lock` (match the `lint-unwrap` convention
   used elsewhere in the crate, for example
   `unwrap_or_else(|e| e.into_inner())`). No write-only state fields:
   `zero_run` is the only per-queue state (a zero streak of 2 already
   implies the last sample was 0).
3. Implement `camel_api::MetricsCollector` for `BatchDepthProbe`:
   `set_queue_depth(&self, queue: &str, depth: usize)` updates state only
   for labels in `expected`: if `depth == 0` increment `zero_run`, else
   reset `zero_run` to 0. Every other trait method is
   a no-op body. This consumes the existing `seda:<name>` label set; it
   declares no new labels.
4. Add `struct BatchProbeLifecycle(Arc<BatchDepthProbe>)` implementing
   `camel_api::Lifecycle` with `as_metrics_collector` returning
   `Some(Arc::clone(&self.0) as Arc<dyn camel_api::MetricsCollector>)`;
   implement any other required `Lifecycle` methods as no-ops (check the
   trait definition for required methods).
5. Add methods on `BatchDepthProbe`: `reset(&self)` sets `zero_run` to 0
   for every queue (called after the trigger send completes, so pre-send
   zero samples never satisfy the drain); `all_drained(&self) -> bool`
   returns true when every expected label has an entry with
   `zero_run >= 2` (two consecutive zero samples; an empty expected set
   returns true — a document with no seda consumers completes
   immediately). Add `pub(crate) async fn drain_until_empty(probe: &BatchDepthProbe, deadline: tokio::time::Instant) -> bool`
   in batch.rs: loop `if probe.all_drained() { return true; }` else sleep
   at most 100 ms AND never past the deadline
   (`let now = tokio::time::Instant::now(); let nap = deadline.saturating_duration_from(now).min(Duration::from_millis(100)); if nap.is_zero() { return false; } tokio::time::sleep(nap).await;`);
   return false once the deadline passes.
6. In `run_job` (mod.rs), after the consumer gate passes, compute the
   expected queue set: for every route def where
   `document::scheme_of_uri(def.from_uri()) == Some("seda")`, insert
   `document::uri_base(def.from_uri()).to_string()` (the gauge label is
   exactly the seda URI base). When `doc.execute.mode` is `Batch`,
   construct the probe and call
   `ctx.add_lifecycle(BatchProbeLifecycle(Arc::clone(&probe)))`
   BEFORE `ctx.start()`.
7. Batch send path: keep the existing timeout-wrapped send unchanged. On
   the `Ok(Ok(reply))` arm, when mode is `Batch`: call `probe.reset()`,
   then
   `batch::drain_until_empty(&probe, tokio_deadline).await`; false builds
   the `Timeout` report (same shape as the send-timeout arm); true falls
   through to the `Completed` report (the existing reply/capture handling
   applies).
8. Batch teardown budget: replace the unconditional
   `let budget = remaining.max(MIN_SHUTDOWN_BUDGET);` with a mode
   branch: batch uses `deadline.saturating_duration_since(Instant::now())`
   only (no floor, per the blessed design — teardown cannot run past the
   overall deadline; one-shot keeps the existing
   `remaining.max(MIN_SHUTDOWN_BUDGET)`). Known accepted artifact: on the
   batch Timeout path the remaining budget is ~0, so the shutdown call
   fails with a "drain timeout: job teardown exceeded 0s" detail that
   lands in `shutdown_error`/stderr — the `Timeout` verdict already
   governs exit 2; add a code comment stating this is accepted v1
   behavior.

**Tests:** (write first, verify red, then implement)
- `batch_drains_fanout_until_empty_exits_0`: setup — tempdir,
  `write_config`, routes: target `id: "fan"`, `from: "direct:fan"`,
  steps `- to: "seda:w1"`, `- to: "seda:w2"`, `- to: "seda:w3"`; workers
  `id: "w1"`, `from: "seda:w1"`, step `- to: "file:<tempdir-abs-path>?fileName=w1.txt"`
  (same for w2, w3). Job doc: mode batch, timeout 60s, send to
  `direct:fan` body "m". action — `run_job(dir, "job.job.yaml")`.
  assert — exit 0; `report["mode"]` equals `"batch"` and
  `report["outcome"]` equals `"Completed"`; after exit, `w1.txt`,
  `w2.txt`, and `w3.txt` all exist in the tempdir and each contains `m`
  (retry-read up to 2 s). Before the drain implementation the job exits
  before workers finish, so at least one file is missing and the test is
  red.
- `batch_no_seda_routes_completes_immediately`: the
  `batch_mode_loads_and_runs_direct` fixture from task 2.2 now exercises
  the real drain path with an empty expected set; extend its assertions
  to confirm exit 0, `Completed`, mode `batch` still hold (rename the
  test to `batch_no_seda_routes_completes_immediately` and keep the
  direct-transform fixture).

**Acceptance:**
- `cargo test -p camel-cli --test job_one_shot_test batch_` green (3 tests).
- `cargo test -p camel-cli --test job_one_shot_test` full suite green.
- `cargo fmt --check` clean; `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 2.3

### Task 2.4: batch edge tests (in-flight worker, timeout, arg combo)

**Files:**
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)
- `crates/camel-cli/src/commands/job/mod.rs` (modified only if a test exposes a defect)

**Steps:**
1. Add `batch_waits_for_in_flight_worker`: setup — routes: target
   `id: "fan"`, `from: "direct:fan"`, step `- to: "seda:slow"`; worker
   `id: "slow"`, `from: "seda:slow"`, steps
   `- delay: 1500`,
   `- to: "file:<tempdir-abs-path>?fileName=slow.txt"`.
   Job doc: mode batch, timeout 60s, send to `direct:fan` body "m".
   action — `run_job`. assert — exit 0 with outcome `Completed` AND
   `slow.txt` exists containing `m` immediately after process exit (no
   retry): the process must not exit while the worker holds the exchange
   in flight. Include a test comment stating the coupling: this guard
   works because seda `stop()` aborts forwarders without draining
   in-flight work — if seda stop ever gains in-flight drain, this test
   silently stops guarding the drain loop and must be re-pointed.
2. Add `batch_timeout_expires_with_timeout_outcome`: setup — a
   self-feeding route `id: "loop"`, `from: "seda:loop"`, step
   `- to: "seda:loop"` (the queue never drains). Job doc: mode batch,
   timeout 2s, send to `seda:loop`. action — `run_job`, measuring
   elapsed time. assert — exit 2; report outcome `Timeout`; elapsed
   under 15 s.
3. Add `batch_works_with_arg_injection`: setup — the fanout fixture from
   task 2.3 with one queue; the worker runs
   `- transform: {simple: "id-${header.batch-id}"}` before the file step
   (`fileName=tagged.txt`). action —
   `run_job_args(dir, &["job.job.yaml", "--arg", "batch-id=42"])`.
   assert — exit 0, outcome `Completed`, and `tagged.txt` contains
   `id-42` (retry-read up to 2 s).
4. If any of the three exposes a drain defect, fix it inside mod.rs in
   this task and re-run all batch tests.

**Tests:** the three tests above are the deliverable.

**Acceptance:**
- `cargo test -p camel-cli --test job_one_shot_test batch_` green (5 batch tests total).
- `cargo test -p camel-cli --test job_one_shot_test` full suite green.
- `cargo fmt --check` clean; `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 2.4

### Task 2.5: `shutdown_error` report field

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/CONTEXT.md` (modified, failure-modes rows)

**Steps:**
1. `JobReport`: add the field after `error`:
   `/// Teardown failure detail when a shutdown failure follows a recorded verdict; `error` keeps the pipeline/timeout verdict.`
   with `#[serde(skip_serializing_if = "Option::is_none")] shutdown_error: Option<String>,`.
   Update every `JobReport` construction site (the Timeout, Transport,
   Failed, and Completed arms) with `shutdown_error: None`. Also update
   the `error` field's doc comment (currently "Error detail for
   `Failed`/`Timeout` outcomes and shutdown failures after a recorded
   verdict") to "Error detail for `Failed`/`Timeout` outcomes" — the
   shutdown detail moves to `shutdown_error`.
2. In the shutdown-failure path, replace
   `if report.error.is_none() { report.error = Some(detail); }` with
   `report.shutdown_error = Some(detail);` — the shutdown detail always
   lands in `shutdown_error`; `error` keeps the pipeline or timeout
   verdict; exit stays 2.
3. Update `crates/camel-cli/CONTEXT.md` failure-modes prose: the row
   stating the report `error` carries the shutdown detail now states the
   shutdown detail lands in the `shutdown_error` field while `error`
   keeps the verdict, and shutdown failures still force exit 2.
4. Add `#[cfg(test)] mod report_tests` at the end of mod.rs with
   `shutdown_error_serializes_alongside_error`: construct a `JobReport`
   with `error: Some("pipeline failed".to_string())` and
   `shutdown_error: Some("shutdown failure: x".to_string())`, serialize
   with `serde_json::to_string`, assert the JSON contains both strings;
   and `shutdown_error_omitted_when_absent`: construct without
   `shutdown_error`, serialize, assert the JSON lacks the
   `shutdown_error` key. Fill the remaining `JobReport` fields with
   placeholder-free values (document: "doc", mode: "one-shot", outcome:
   "Failed", terminated_early: false, duration_ms: 1, reply: None).

**Tests:** the two unit tests in step 3.

**Acceptance:**
- `cargo test -p camel-cli --lib report_tests` green.
- `cargo test -p camel-cli --test job_one_shot_test` full suite green.
- `cargo fmt --check` clean; `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 2.5
