# Tasks: httpflake

## camel-component-http (crates/components/camel-http)

### Task 1.1: Loaded-soak repro BEFORE the fix (evidence, pristine tree)

**Files:**
- `openspec/changes/httpflake/evidence/before.md` (new)

**Steps:**
1. Verify the worktree state: the conductor has committed the plan
   artifacts (baseline: WIP commit `97bfa80e` plus the plan-bless fixes —
   `git status --short` MUST be clean before the soak starts); confirm
   code is pristine: `git diff 2606c1dc..HEAD -- crates/` is empty
   (lib.rs byte-identical to the spec-bless commit). No code edits in
   this task.
2. Build the test binary:
   `cargo test -p camel-component-http --lib --no-run` (worktree only —
   NEVER the main checkout; shared `./target` must stay cold).
3. Start 12 CPU spin burners as background shell loops
   (`for i in $(seq 12); do (while :; do :; done) & echo $! >> /tmp/httpflake-burners.pid; done`),
   with a `trap`/script wrapper that kills them by PID file on EXIT/ERR —
   burners MUST NOT survive the task.
4. Under load, run the full suite in a loop, up to 8 iterations, stopping
   early once a run fails:
   `cargo test -p camel-component-http --lib -- --test-threads=12 2>&1 | tee -a <log>`;
   count lines matching `did not become ready`.
5. If step 4 produced 0 matches, run the targeted high-collision variant up
   to 5 iterations:
   `cargo test -p camel-component-http --lib content_type_inferred registry -- --test-threads=12 2>&1 | tee -a <log>`.
6. Record in `evidence/before.md`: date, `nproc`, load average, burner
   count, per-run result (pass/fail + which tests panicked with the
   readiness message), the exact commands, and the raw log path (copy logs
   under `openspec/changes/httpflake/evidence/logs-before/`).
7. Kill the burners; verify with `ps` that none remain; run `df -h
   /home/shared` and record free space.

**Tests:** (executable spec — command, expected)
- `soak-before-full`: full `cargo test -p camel-component-http --lib --
  --test-threads=12` under 12 burners, up to 8 runs → expected: at least 1
  run reports a panic containing `consumer server did not become ready`
  (early-stop once observed). If 0 after 8 runs, the targeted variant must
  produce one within 5 runs; if both stay green, record that honestly
  (see Acceptance) — do not fabricate.
- `burner-hygiene`: after the task, `pgrep -f "while :"` finds no burners
  started by this task → expected: none.

**Acceptance:**
- `evidence/before.md` exists, states burner count and machine load, and
  either (a) documents ≥1 `did not become ready` panic with test name +
  run number, or (b) documents ≥13 loaded green runs AND flags
  `repro-not-observed` so the conductor escalates before Task 1.3.
- No file outside `openspec/changes/httpflake/evidence/` was modified
  (`git status --short` shows only evidence artifacts).

- [x] 1.1

### Task 1.2: Regression test `readiness_survives_concurrent_registry_reset` (RED pre-fix)

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified — test module only)

**Steps:**
1. In the existing `#[cfg(test)]` test module, add
   `#[tokio::test] async fn readiness_survives_concurrent_registry_reset()`
   adjacent to `setup_consumer_on_free_port` (~lib.rs:9200) and the
   `content_type_inferred` tests it exercises — with the code it tests,
   not in the registry-test cluster at ~6194.
2. Test body per the blessed spec scenario:
   - `let contended = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));`
     `let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));`
   - Spawn a hammer OS thread (plain `std::thread::spawn`) looping:
     if `REGISTRY_TEST_MUTEX.try_lock()` fails, increment `contended` then
     take the lock (guard dropped at iteration end); then call
     `ServerRegistry::reset()`; loop while `!stop.load(Ordering::Relaxed)`.
   - Define a Drop guard struct (local to the test fn) holding the
     `JoinHandle` + `stop` Arc whose `Drop` sets `stop = true` and
     `let _ = handle.join();` — so the hammer is stopped and joined even
     if the test panics. Instantiate it before the setup loop.
   - Setup loop: `let mut setups = 0; loop { setups += 1; let (_port, rx,
     token) = setup_consumer_on_free_port("/reset-hammer").await;
     drop(rx); token.cancel(); if (setups >= 25 && contended >= 1) ||
     setups >= 50 { break; } }` — ALWAYS at least 25 setups, continue past
     25 only until one contended reset is observed, hard cap 50.
   - Assert `contended.load(std::sync::atomic::Ordering::SeqCst) >= 1`
     and (implicitly, by not panicking) every setup became ready.
3. Run the test pre-fix (the fix lands in Task 1.3):
   `cargo test -p camel-component-http --lib
   readiness_survives_concurrent_registry_reset -- --test-threads=1`.
   Expect RED (failure mode: `contended >= 1` assertion fails after 50
   setups — pre-fix nothing but the hammer takes the mutex, so its
   try_lock rarely blocks — and/or a readiness panic when a hammer reset
   lands inside an unprotected stage→ready window). Capture the failing
   output to `evidence/regression-red.txt`.
4. Do NOT gate-fix anything in this task; the test stays failing until
   Task 1.3.

**Tests:**
- `readiness_survives_concurrent_registry_reset`: setup = hammer thread
  looping legal resets (try_lock contention counted) + fixed-cap setup
  loop → action = run setups on fresh ephemeral ports, always ≥25, cap 50,
  stop past 25 only at `contended >= 1` → assert = `contended >= 1` holds
  and every setup returned ready (no readiness panic); hammer joined via
  Drop guard. Command: `cargo test -p camel-component-http --lib
  readiness_survives_concurrent_registry_reset -- --test-threads=1`.
  Expected pre-fix: FAIL (this task). Expected post-fix (Task 1.3): PASS.

**Acceptance:**
- `cargo build -p camel-component-http --tests` exits 0 (test compiles).
- Pre-fix run captured in `evidence/regression-red.txt` shows the test
  FAILING (either assertion or readiness panic) — RED is the deliverable.
- `cargo fmt --check` clean; no `unwrap()` added (use `expect` with
  context; `lint-unwrap` runs at STAGE 4).

- [x] 1.2

### Task 1.3: Fix — mutex through setup→ready + 10 s / doubling backoff readiness budget

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified — readiness helper
  only)

**Steps:**
1. In `setup_consumer_on_free_port`, acquire
   `let _registry_guard = REGISTRY_TEST_MUTEX.lock().expect("REGISTRY_TEST_MUTEX");`
   BEFORE `ServerRegistry::global().stage_listener(listener).await` so
   the guard covers: stage_listener, consumer spawn, the readiness poll
   loop, and the 8-iteration tail-yield loop. The guard drops when the
   function returns — do not return early between acquisition and
   readiness completion. (`expect` over the siblings' `unwrap()` is
   deliberate: new code names the lock in the failure message;
   lint-unwrap excludes `#[cfg(test)]` scope either way.)
2. If clippy flags holding the sync guard across `.await`
   (`await_holding_lock`), add a targeted
   `#[allow(clippy::await_holding_lock)]` on the helper only — first
   verify how the 29 existing guard sites pass today and match their
   pattern exactly.
3. Extract the readiness poll loop (deadline + backoff + panic) into a
   test-module fn `async fn wait_for_registry_ready(host: &str, port: u16)`
   that: polls `ServerRegistry::global().bound_addr(host, port)` in a
   loop; on each miss asserts
   `tokio::time::Instant::now() < deadline` with the panic message using
   the spec's hint string verbatim — `consumer server did not become
   ready on port {port} — registry entry absent (concurrent reset or
   starvation)`; sleeps with a backoff that starts at 1 ms, doubles each
   iteration, and caps at 64 ms; deadline = 10 s from loop entry. The
   helper calls this fn instead of the inline loop.
4. Add the deadline-hint test:
   `#[tokio::test] #[should_panic(expected = "registry entry absent
   (concurrent reset or starvation)")] async fn
   readiness_deadline_fires_loud_with_hint()` that binds a
   `tokio::net::TcpListener` on `127.0.0.2:0` (NOT 127.0.0.1 — 0 uses of
   127.0.0.2 crate-wide, so no concurrent test can initialize a registry
   entry under the same key and turn the poll Some), reads the port,
   DROPS the listener without staging or spawning a consumer, and calls
   `wait_for_registry_ready("127.0.0.2", port).await` — no entry ever
   appears, so the 10 s deadline fires (~10 s wall time, acceptable for
   one deterministic test). The expected substring covers the hint, so
   hint removal is regression-covered.
5. Run the regression test → must be GREEN now:
   `cargo test -p camel-component-http --lib
   readiness_survives_concurrent_registry_reset -- --test-threads=1`.
6. Run the full suite unloaded: `cargo test -p camel-component-http
   --lib` (367 tests exist today; with the 2 new tests expect zero
   failures — record the observed pass count in the task result).
7. `cargo fmt --check` and
   `cargo clippy -p camel-component-http --all-targets -- -D warnings`
   both clean.

**Tests:**
- `readiness_survives_concurrent_registry_reset`: same spec as Task 1.2 →
  expected now PASS (mutex excludes every hammer reset from the
  stage→ready window; contention observed on the first setup's hold).
  Exercises blessed scenario `staged-consumer-becomes-ready` (every setup
  becomes ready via the registry poll) alongside the 8
  `content_type_inferred_*` tests.
- `readiness_deadline_fires_loud_with_hint`: no registry entry for the
  polled 127.0.0.2 port → 10 s deadline fires → panic message carries the
  spec's hint string. Command: `cargo test -p camel-component-http --lib
  readiness_deadline_fires_loud_with_hint`. Expected: PASS (panics with
  the expected message; takes ~10 s).
- `content_type_inferred_*` family (8 tests): unloaded full-suite run →
  all green, no readiness panics. Command: `cargo test -p
  camel-component-http --lib`.

**Acceptance:**
- Regression test PASS (`--test-threads=1` and inside the default
  parallel full suite).
- Deadline test PASSES via `#[should_panic]` matching the spec's hint
  substring `registry entry absent (concurrent reset or starvation)`.
- `cargo test -p camel-component-http --lib` exits 0 with zero failures
  (observed count recorded; 367 pre-existing + 2 new).
- `cargo fmt --check --all` clean;
  `cargo clippy -p camel-component-http --all-targets -- -D warnings`
  exits 0 (STAGE 4's workspace `--all-features` clippy covers the otel
  feature; no code touched here is otel-gated).
- Diff touches ONLY `setup_consumer_on_free_port`, the new
  `wait_for_registry_ready` fn, the new deadline test, and the test added
  in Task 1.2 — no production code.

<!-- Amendment (post-holistic, 2026-09-14): the final diff exceeds the
"ONLY setup_consumer_on_free_port/wait_for_registry_ready/deadline
test/1.2-test" scope above in three review-driven ways, all documented in
evidence/diagnosis.md + evidence/after.md: (a) registry_rejects_tls_on_plain_port
now guards its reset (spec R2 completion — traced unguarded mid-window
reset); (b) lock_registry_test_mutex poison-recovering helper + conversion
of all 36 acquisition sites (r_glm finding: one panic must not cascade
~30 PoisonError failures); (c) the deadline test polls a held 127.0.0.1
listener under the unreachable "localhost" key (127.0.0.2 breaks macOS
weekly CI, rc-dwmd precedent). -->

- [x] 1.3

### Task 1.4: Loaded-soak AFTER the fix (evidence) + hygiene

**Files:**
- `openspec/changes/httpflake/evidence/after.md` (new)
- `openspec/changes/httpflake/evidence/logs-after/` (new, raw logs)

**Steps:**
1. With the fix from Task 1.3 in the tree, start the same 12-burner load
   (same PID-file + trap hygiene as Task 1.1).
2. Run EXACTLY 15 full-suite iterations under load:
   `cargo test -p camel-component-http --lib -- --test-threads=12`; count
   `did not become ready` matches across all 15 (must be 0).
3. Run 3 targeted high-collision iterations:
   `cargo test -p camel-component-http --lib content_type_inferred
   registry readiness_survives -- --test-threads=12` (must be green).
4. Record `evidence/after.md`: same fields as before.md (date, nproc,
   load, burners, per-run results, commands, log copies under
   `logs-after/`), plus before/after delta summary.
5. Kill burners, verify none remain; `df -h /home/shared` recorded.

**Tests:** (executable spec)
- `soak-after-full`: 15/15 full-suite loaded runs with 0 panics matching
  `did not become ready` → expected: 0.
- `soak-after-targeted`: 3/3 targeted loaded runs green → expected: 3.

**Acceptance:**
- `evidence/after.md` documents 15 loaded full-suite runs, 0 readiness
  panics, 3 targeted green runs.
- Before/after story present: if before.md documented ≥1 panic under the
  same load, after.md shows 0 under that load; if before.md recorded
  `repro-not-observed`, the story becomes "no repro either way — the
  deterministic regression test (Task 1.2/1.3) is the pre/post evidence"
  and states that explicitly.
- No burners survive (`pgrep`); disk free space recorded.

- [x] 1.4
