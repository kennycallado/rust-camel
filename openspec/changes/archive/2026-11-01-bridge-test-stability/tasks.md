# Tasks: bridge-test-stability

## crates/services/camel-bridge

### Task 1.1: Capture RED evidence for the TMPDIR race and the timing flake (no code change)

**Files:**
- `openspec/changes/bridge-test-stability/evidence/red-camel-bridge-tmpdir-race.log` (new)
- `openspec/changes/bridge-test-stability/evidence/red-mtls-env-unset.log` (new)

**Steps:**
1. From the worktree root, run the concurrent mix that triggers the TMPDIR poisoning
   (tls rung tests hold `TMPDIR` while process tests create tempdirs), 5 iterations,
   saving output:
   `for i in 1 2 3 4 5; do TMPDIR=/home/shared/tmp cargo test -p camel-bridge --lib -- tls tmpdir system_temp cache_dir process_dual partial_ready malformed; done`
   On any iteration failure, save the failing iteration's full output (trim to the
   panic + test-name lines plus a one-line header per iteration) to
   `evidence/red-camel-bridge-tmpdir-race.log`. The expected failure signature is a
   `NotFound` panic from `fake bridge tempdir` / `marker tempdir` /
   `observed env tempdir` while `TMPDIR` points under `/home/shared/tmp/.tmp*/`.
2. If all 5 iterations pass, append to the log:
   `NOT-REPRODUCED in 5 attempts (order-dependent; root cause verified by code inspection: tls.rs rung tests set process TMPDIR incl. a nonexistent path under EnvGuard while parallel tests call tempfile::tempdir)`.
3. Run the mtls baseline:
   `env -u CAMEL_XML_BRIDGE_BINARY_PATH cargo test -p camel-test --features integration-tests --test mtls_bridge_test 2>&1`
   Save the summary + the two panic lines (`xml-bridge binary not found` /
   `CAMEL_XML_BRIDGE_BINARY_PATH`) to `evidence/red-mtls-env-unset.log`.
4. Commit the two evidence files.

**Tests:** (verification-only task — the commands above ARE the test)
- `red-race`: baseline code → concurrent mix under `TMPDIR=/home/shared/tmp` → failure
  signature captured (or NOT-REPRODUCED note per step 2).
- `red-mtls`: baseline code, env var unset → mtls_bridge_test → 2 failures named
  `start_and_connect_mtls_handshake` and `wrong_ca_handshake_rejected`.

**Acceptance:**
- Both evidence files exist and contain either the failure signature or the explicit
  NOT-REPRODUCED note; `red-mtls-env-unset.log` shows exactly 2 failed tests.
- `git status` clean after the commit.

- [x] 1.1

### Task 1.2: Isolate tls.rs TMPDIR rung tests into child processes (rc-2t4j8, rc-zbdfh race half)

**Files:**
- `crates/services/camel-bridge/src/tls.rs` (modified, tests only)

**Steps:**
1. In the `tests` module of `src/tls.rs`, add consts mirroring the `process.rs`
   precedent (`TLS_SPAWN_ISOLATION_CHILD_ENV`): `TLS_RUNG_ISOLATION_CHILD_ENV` with
   value `"CAMEL_BRIDGE_TLS_RUNG_ISOLATION_CHILD"`, `TLS_RUNG_ISOLATION_SCENARIO_ENV`
   with value `"CAMEL_BRIDGE_TLS_RUNG_SCENARIO"`, and `TLS_RUNG_ISOLATION_EXPECT_ENV`
   with value `"CAMEL_BRIDGE_TLS_RUNG_EXPECT"`, plus a const
   `TLS_RUNG_ISOLATION_TEST: &str = "tls::tests::tls_rung_isolation_inner"`.
2. Add the inert inner test `#[test] fn tls_rung_isolation_inner()`: first line returns
   early unless `TLS_RUNG_ISOLATION_CHILD_ENV` is set (same guard style as
   `tls_spawn_isolation_inner` in `process.rs`). It then dispatches on
   `TLS_RUNG_ISOLATION_SCENARIO_ENV` and runs the ORIGINAL body of the corresponding
   rung test with `EnvGuard` usage removed (the child's env is already correct):
   - `"tmpdir-writable"`: `resolve_tls_base()` equals the path in
     `TLS_RUNG_ISOLATION_EXPECT_ENV`;
   - `"tmpdir-unset"`: `resolve_tls_base()` equals `std::env::temp_dir()`;
   - `"cache-dir-rung"`: NO `tempfile::tempdir()` in the child (its TMPDIR is
     deliberately unusable — fixture creation lives in the outer test). The inner
     keeps only the original cache-rung logic that does not create fixtures:
     `prepare_cache_root(&expected_cache_root())` (which derives
     `<cache_root>/<version>/tls` from the child's own env: XDG on Linux, HOME on
     macOS), then asserts `resolve_tls_base()` equals that expected path, preserving
     the original permission/material assertions;
   - `"all-rungs-fail"`: `resolve_tls_base()` is an error whose message contains
     `XDG_CACHE_HOME` and `$TMPDIR` (fixture `fresh`/blocker file created by the
     outer; the inner creates nothing).
3. Convert each outer test (`tmpdir_rung_wins_when_writable`,
   `system_temp_rung_when_tmpdir_unset`, `cache_dir_rung_when_no_tmp_usable`,
   `total_failure_names_xdg_override`) to spawn the test binary as a child, copying
   the spawn helper mechanics from `process.rs` (`std::env::current_exe()`, args
   `["--exact", TLS_RUNG_ISOLATION_TEST, "--test-threads=1", "--nocapture"]`, panic
   message including child stdout/stderr on failure). Env per scenario:
   - `tmpdir-writable`: create `fresh = tempfile::tempdir()`; child env sets marker,
     scenario, `TMPDIR=$fresh`, `EXPECT=$fresh`; drop `fresh` only after the child
     exits;
   - `tmpdir-unset`: child env sets marker + scenario, and REMOVES `TMPDIR`;
   - `cache-dir-rung`: `fresh` tempdir created by the OUTER; child env sets marker,
     scenario, `TMPDIR=$fresh/no-such-dir`, `XDG_CACHE_HOME=$fresh/xdg-cache`,
     `HOME=$fresh/home` (the inner derives the expected `<root>/<version>/tls` path
     from its own env via `expected_cache_root()`; no fixture creation inside the
     child);
   - `all-rungs-fail`: `fresh` tempdir + blocker FILE created by the OUTER; child env
     sets marker, scenario, `TMPDIR=$fresh/no-such-dir`, and `XDG_CACHE_HOME`/`HOME`
     at `<blocker>/child` (same construction as today's
     `total_failure_names_xdg_override`).
4. Delete nothing else; non-TMPDIR `EnvGuard` tests in the file stay untouched.
5. Run `TMPDIR=/home/shared/tmp cargo test -p camel-bridge --lib -- tls --test-threads=8`
   then the Task 1.1 concurrent mix 5 times — all green.
6. `cargo fmt --check --all` and `cargo clippy -p camel-bridge --all-targets -- -D warnings`;
   fix any issues. Commit.

**Tests:**
- `tmpdir_rung_wins_when_writable`: outer spawns isolated child with
   `TMPDIR=$fresh` → child asserts `resolve_tls_base()==$fresh` → outer asserts exit
   success; parent process env never mutated.
- `system_temp_rung_when_tmpdir_unset`: child spawned with `TMPDIR` removed → child
   asserts `resolve_tls_base()==std::env::temp_dir()` → exit success.
- `cache_dir_rung_when_no_tmp_usable`: child with nonexistent `TMPDIR` + synthetic
   XDG/HOME → child asserts resolved base equals the `expected_cache_root()`-derived
   path (original assertions preserved verbatim) → exit success.
- `total_failure_names_xdg_override` (outer): child with TMPDIR nonexistent and
   XDG/HOME under a file-blocked path → child asserts `resolve_tls_base()` error
   message contains `XDG_CACHE_HOME` and `$TMPDIR` → exit success.
- Race regression: Task 1.1 mix ×5 under `TMPDIR=/home/shared/tmp` → 0 failures.
- Command: `TMPDIR=/home/shared/tmp cargo test -p camel-bridge --lib` → all pass.

**Acceptance:**
- `grep -rn "and_set(\"TMPDIR\"" crates/services/camel-bridge/src/` returns 0 hits
  (no process-env TMPDIR mutation remains outside child processes).
- `TMPDIR=/home/shared/tmp cargo test -p camel-bridge --lib` passes.
- fmt + clippy clean on camel-bridge.

- [x] 1.2

### Task 1.3: Deterministic enclosing-budget readiness test via paused clock (rc-zbdfh timing half)

**Files:**
- `crates/services/camel-bridge/Cargo.toml` (modified: dev-dependencies)
- `crates/services/camel-bridge/src/process.rs` (modified)

**Steps:**
1. Add to `[dev-dependencies]` in `crates/services/camel-bridge/Cargo.toml`:
   `tokio = { workspace = true, features = ["test-util"] }`.
2. Write the new test body FIRST (RED — fails to compile until step 3): replace
   `partial_ready_is_bounded_and_completes_in_one_timeout` with a
   `#[tokio::test(start_paused = true)]` test that:
   - builds `tokio::io::duplex(64)`; wraps the reader half in
     `tokio::io::BufReader`;
   - spawns a feeder task that writes JSON readiness lines to the writer half with
     `tokio::time::sleep` gaps (virtual time) per row;
   - calls `read_ready_within(&mut reader, budget_ms, expect_soap_ready, name)`;
   - asserts with `tokio::time::Instant` (virtual elapsed), never wall-clock.
3. Extract the helper: in `src/process.rs`, add private
   `async fn read_ready_within<R: tokio::io::AsyncRead + Unpin>(reader: &mut tokio::io::BufReader<R>, start_timeout_ms: u64, expect_soap_ready: bool, name: &str) -> Result<(u16, Option<u16>), BridgeError>`
   containing exactly today's `timeout` expression from `BridgeProcess::start` (the
   one wrapping `read_ready_ports(&mut reader, expect_soap_ready)` in
   `Duration::from_millis(start_timeout_ms)`), with the mapping:
   `Ok(Ok(ports)) -> Ok(ports)`; elapsed -> today's `tracing::error!` + today's
   `BridgeError::Timeout` message (keep the exact message text, which uses the
   spec `name`); `Ok(Err(StdoutClosed))` -> passed through as
   `Err(BridgeError::StdoutClosed)`. Replace the inline expression in `start` with a
   call to the helper, leaving the `StdoutClosed` child classification
   (`eof_exit_error`) in `start` unchanged.
4. Test rows (budget 4_000, `expect_soap_ready = true`, `name` from
   `fake_bridge_config`'s spec). The `soap-ready` line arrives at virtual t=1_000 ms
   in every row so a per-line-reset implementation is actually discriminated from the
   enclosing budget (reset deadline would be 5_000 ms, enclosing deadline 4_000 ms):
   - Positive: feeder sleeps 1_000 ms, writes
     `{"status":"soap-ready","port":23456}`, sleeps 2_500 ms, writes
     `{"status":"ready","port":12345}` (ready at 3_500 < 4_000) → assert
     `Ok((12345, Some(23456)))` and virtual elapsed < 4_000 ms.
   - Counterexample (enclosing budget, no per-line reset): feeder sleeps 1_000 ms,
     writes soap-ready, sleeps 3_500 ms, writes ready (ready at 4_500 > 4_000; a
     per-line-reset implementation would succeed at 4_500) → assert
     `Err(BridgeError::Timeout(_))` with virtual elapsed == 4_000 ms.
   - Injected negative: feeder writes nothing and holds the writer open
     (`std::future::pending().await`) → assert `Err(BridgeError::Timeout(_))` with
     virtual elapsed == 4_000 ms.
   Ports appear only as parsed JSON fields — nothing binds them (no free ports).
5. Delete the old marker-tempdir/`touch '{marker}'` machinery of the replaced test;
   other tests using `fake_bridge_script` stay unchanged.
6. Run `cargo test -p camel-bridge --lib partial_ready` 5 times — deterministic pass
   (same result every run, no wall-clock dependency). Run
   `cargo xtask lint-test-sleep`; if the virtual-clock sleeps are flagged, apply the
   repo's established test-wait allowance annotation used elsewhere in the suite (do
   not weaken the lint). Run `cargo xtask lint-unbounded-wait` — must stay green.
7. `cargo fmt --check --all`; `cargo clippy -p camel-bridge --all-targets -- -D warnings`.
   Commit.

**Tests:**
- `partial_ready_is_bounded_and_completes_in_one_timeout` (rewritten): duplex reader +
   virtual clock, rows as in step 4 → exact assertions above; command
   `cargo test -p camel-bridge --lib partial_ready` ×5 → pass every run.
- `cargo test -p camel-bridge --lib` → full crate green (extraction is
   behavior-identical; dual-ready/malformed/no-soap-wait tests unchanged and passing).

**Acceptance:**
- No `std::time::Instant` wall-clock assertion remains in
  `partial_ready_is_bounded_and_completes_in_one_timeout`; no real `sleep` in the
  test; the old shell-sleep fixtures for this test are gone.
- `cargo test -p camel-bridge --lib partial_ready` passes 5/5 consecutive runs.
- `cargo xtask lint-test-sleep` and `cargo xtask lint-unbounded-wait` exit 0.
- fmt + clippy clean on camel-bridge.

- [x] 1.3

## crates/camel-test

### Task 1.4: Skip-with-diagnostic for absent bridge binary in mtls tests (rc-dya5z)

**Files:**
- `crates/camel-test/tests/support/xml_bridge.rs` (modified)
- `crates/camel-test/tests/mtls_bridge_test.rs` (modified)
- `crates/camel-test/README.md` (modified)

**Steps:**
1. In `tests/support/xml_bridge.rs`, add next to `require_xml_bridge_binary`:
   `pub fn xml_bridge_binary_or_skip() -> Option<std::path::PathBuf>` — when
   `CAMEL_XML_BRIDGE_BINARY_PATH` is absent from the environment, print to stderr one
   line naming the env var, the provisioning command
   (`cargo xtask build-xml-bridge`), the export instruction, and the word `SKIP`;
   return `None`. When the var is set but does not name an existing file (including
   when empty), panic with the same provisioning hint (fail loudly on
   misconfiguration). Do not change `require_xml_bridge_binary` or its other callers.
2. Update the `xml_bridge.rs` module doc to describe the skip path alongside the
   existing fail-closed description.
3. In `mtls_bridge_test.rs`, change `start_and_connect_mtls_handshake` and
   `wrong_ca_handshake_rejected` to
   `let Some(binary) = support::xml_bridge::xml_bridge_binary_or_skip() else { return; };`
   — everything after that line stays byte-identical.
4. Add a README section `## xml-bridge integration tests` to
   `crates/camel-test/README.md`: the env var name, `cargo xtask build-xml-bridge`
   provisioning, export/unset instructions, and the skip/fail semantics (absent →
   SKIP diagnostic, set-but-invalid → hard failure).
5. Verify (all three runs are required — no substitutions, no deferrals):
   - `env -u CAMEL_XML_BRIDGE_BINARY_PATH cargo test -p camel-test --features integration-tests --test mtls_bridge_test -- --nocapture`
     → 2/2 pass, stderr shows both SKIP lines.
   - `CAMEL_XML_BRIDGE_BINARY_PATH=/nonexistent/x-bridge cargo test -p camel-test --features integration-tests --test mtls_bridge_test`
     → exactly the same 2 tests FAIL with the provisioning hint (loud misconfiguration).
   - `CAMEL_XML_BRIDGE_BINARY_PATH= cargo test -p camel-test --features integration-tests --test mtls_bridge_test`
     → the same 2 tests FAIL with the provisioning hint (empty value is
     set-but-invalid, never a skip).
   - With a valid binary path: run
     `cargo xtask build-xml-bridge` first if
     `bridges/xml/build/native/xml-bridge` is not already present, then
     `CAMEL_XML_BRIDGE_BINARY_PATH=<that path> cargo test -p camel-test --features integration-tests --test mtls_bridge_test`
     → 2/2 pass with FULL assertions exercised (no early return taken).
6. `cargo fmt --check --all`; `cargo clippy -p camel-test --features
   integration-tests --all-targets -- -D warnings`. Commit (include updated evidence
   file `evidence/green-mtls-unset.log` with the skip-run output summary).

**Tests:**
- `start_and_connect_mtls_handshake`: env var unset → SKIP diagnostic + pass;
   valid var → full mTLS handshake assertions unchanged.
- `wrong_ca_handshake_rejected`: env var unset → SKIP diagnostic + pass; valid var →
   wrong-CA rejection assertions unchanged.
- Command (unset): `env -u CAMEL_XML_BRIDGE_BINARY_PATH cargo test -p camel-test --features integration-tests --test mtls_bridge_test`
   → 2 passed, 0 failed.
- Command (invalid): `CAMEL_XML_BRIDGE_BINARY_PATH=/nonexistent/x-bridge cargo test -p camel-test --features integration-tests --test mtls_bridge_test`
   → 2 failed with provisioning hint (expected failure — proves no weakening).
- Command (empty): `CAMEL_XML_BRIDGE_BINARY_PATH= cargo test -p camel-test --features integration-tests --test mtls_bridge_test`
   → 2 failed with provisioning hint (empty is set-but-invalid, never a skip).
- Command (valid): `CAMEL_XML_BRIDGE_BINARY_PATH=<worktree native bridge> cargo test -p camel-test --features integration-tests --test mtls_bridge_test`
   → 2 passed with full assertions.

**Acceptance:**
- Unset run: 2/2 pass with SKIP lines on stderr; invalid and empty runs: the same 2
  tests fail loudly; valid run (binary built if needed via `cargo xtask
  build-xml-bridge`): 2/2 pass with full assertions — no deferral.
- fmt + clippy (with `--features integration-tests`) clean on camel-test; README
  section present.

- [x] 1.4

## Verification

### Task 1.5: GREEN battery, gate battery, and per-ticket evidence

**Files:**
- `openspec/changes/bridge-test-stability/evidence/green-camel-bridge-lib-x3.log` (new)
- `openspec/changes/bridge-test-stability/evidence/green-timing-determinism.log` (new)
- `openspec/changes/bridge-test-stability/evidence/green-mtls-with-binary.log` (new)
- `openspec/changes/bridge-test-stability/evidence/green-camel-test-suite.log` (new)
- `openspec/changes/bridge-test-stability/evidence/gates.log` (new)

**Steps:**
1. Run `TMPDIR=/home/shared/tmp cargo test -p camel-bridge --lib` 3 times; save the
   three summaries to `green-camel-bridge-lib-x3.log`.
2. Run `cargo test -p camel-bridge --lib partial_ready` 5 times (default threads, no
   TMPDIR requirement); save summaries to `green-timing-determinism.log`.
3. Re-run the Task 1.1 concurrent mix 5 times; append summaries to
   `green-camel-bridge-lib-x3.log`.
4. Run `cargo test -p camel-test --features integration-tests --test mtls_bridge_test`
   with a valid binary path and with the var unset; save both summaries to
   `green-mtls-with-binary.log`.
5. Run the WIDER suite both ways (proposal acceptance): 
   `env -u CAMEL_XML_BRIDGE_BINARY_PATH cargo test -p camel-test` and
   `cargo test -p camel-test` with a valid `CAMEL_XML_BRIDGE_BINARY_PATH`; save both
   summaries to `green-camel-test-suite.log` (env-preconditioned tests skip cleanly
   in the first run).
6. Gate battery — run each, record command + exit code in `gates.log`:
   - `cargo fmt --check --all`
   - `cargo clippy -p camel-bridge --all-targets -- -D warnings`
   - `cargo clippy -p camel-test --features integration-tests --all-targets -- -D warnings`
   - every `cargo xtask lint-*` gate listed in the AGENTS.md QUALITY GATES block
   - `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-builder
     -p camel-dsl -p camel-endpoint -p camel-bridge -p camel-test --no-deps`
   - `openspec validate --all --strict`
7. Commit the evidence files.

**Tests:** (the commands above are the tests)
- `green-lib-x3`: 3/3 full `--lib` runs pass under `TMPDIR=/home/shared/tmp`.
- `green-timing`: 5/5 deterministic passes of the timing test.
- `green-mtls` + `green-suite`: pass both with and without the env var, at file and
  crate scope.
- `gates`: every gate command exits 0.

**Acceptance:**
- All evidence logs present; every recorded run and gate green; `git status` clean.

- [x] 1.5
