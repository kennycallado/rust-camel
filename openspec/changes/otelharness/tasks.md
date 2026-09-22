# Tasks: otelharness

## camel-otel

### Task 1.1: Move inline `mod tests` to sibling `service_tests.rs`

**Files:**
- `crates/services/camel-otel/src/service_tests.rs` (new)
- `crates/services/camel-otel/src/service.rs` (modified)

**Steps:**
1. Create `service_tests.rs` containing, verbatim, the interior of the inline
   test module in `service.rs` (the block from the `#[cfg(test)]` attribute at
   line 560 through the module's closing brace at line 1280 — copy everything
   between `mod tests {` and its matching final `}`), starting with the
   existing `use super::*;` and `use std::sync::{Arc, Mutex};` lines. Do not
   reformat, rename, or drop any test, doc comment, or attribute.
2. Delete that entire block (attribute + module) from `service.rs`.
3. Immediately below the existing sampler wiring (service.rs lines 42–44:
   `#[cfg(test)]` / `#[path = "sampler_tests.rs"]` / `mod sampler_tests;`),
   add the same three-line shape for the moved module:
   `#[cfg(test)]` / `#[path = "service_tests.rs"]` / `mod tests;`
   so the module path stays `service::tests`.
4. Run `cargo fmt` on both files (formatting of the moved body must not
   change: it is already fmt-clean).

**Tests:** (executable oracle — baseline captured at 6b110708)
- `sibling-move-preserves-test-list`: baseline
  `/tmp/nix-shell.4Pl7wW/opencode/otel-baseline-testlist.txt` (91 tests,
  raw cargo output with interleaved per-target summaries — MUST be filtered):
  `grep ': test' /tmp/nix-shell.4Pl7wW/opencode/otel-baseline-testlist.txt > /tmp/base.list`
  then `cargo test -p camel-otel -- --list | grep ': test' > /tmp/after.list`
  then `diff /tmp/base.list /tmp/after.list` → diff is empty (both sides
  identically filtered; no interleaved summary or timing lines remain).
- `suite-green-after-move`: `cargo test -p camel-otel` → all 91 pass, zero
  failures, including `service::tests::test_stop_bounded_when_metric_export_stalls`
  and `service::tests::test_stop_bounded_when_span_export_stalls`.

**Acceptance:**
- The list diff is empty (same names, same count, same `camel_otel::service::tests::*` paths).
- `cargo test -p camel-otel` exits 0.
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-otel --all-targets -- -D warnings` exits 0
  (`--all-targets` is required: plain clippy does not lint `#[cfg(test)]`
  code, and this task moves test code).
- `grep -c 'mod tests {' crates/services/camel-otel/src/service.rs` = 0;
  `wc -l crates/services/camel-otel/src/service.rs` ≈ 565 (within ±10).

- [x] 1.1

### Task 1.2: Extract shared `bounded_repro` harness, dedupe both stall repros

**Files:**
- `crates/services/camel-otel/src/service_tests.rs` (modified)

**Steps:**
1. Add a private helper to the `tests` module in `service_tests.rs`:

   ```rust
   fn bounded_repro<F, Fut>(tag: &str, regression: &str, body: F)
   where
       F: FnOnce() -> Fut + Send + 'static,
       Fut: std::future::Future<Output = ()>,
   ```

   owning the scaffold exactly as it exists today in both stall tests:
   `mpsc::channel::<Result<(), String>>()`; a spawned
   `std::thread::Builder` named `format!("{tag}-repro")`; inside the thread,
   `catch_unwind(AssertUnwindSafe(|| { let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("repro runtime"); rt.block_on(body()) }))`
   with the panic payload downgraded to a message via the existing
   `&str`/`String` downcast chain falling back to `"unknown panic"`;
   the outer `match rx.recv_timeout(Duration::from_secs(60))` with the exact
   panic texts `repro thread failed (not a hang): {panic_msg}` and
   `repro thread did not finish: stop() hung ({regression} regression)`;
   then `let _ = handle.join();`.
2. Rewrite `test_stop_bounded_when_metric_export_stalls` (rc-q74u) to call
   `bounded_repro("q74u", "rc-q74u", || async move { BODY })` where `BODY`
   is the verbatim interior of the test's current
   `rt.block_on(async { BODY })` block: the `OtelConfig::new` repro config,
   `service.start()`, the collector arming via
   `as_metrics_collector().expect("collector")` +
   `record_exchange_duration("q74u-route", Duration::from_millis(5))`, and
   the 15s-deadline assertion with its exact message string
   "stop() must be bounded when metric export stalls (rc-q74u)".
3. Rewrite `test_stop_bounded_when_span_export_stalls` (rc-6ju71) to call
   `bounded_repro("q6ju71", "rc-6ju71", || async move { BODY })` with its
   verbatim body interior: `BatchConfigBuilder` with
   `with_scheduled_delay(Duration::from_secs(3600))`, the locally-defined
   `StalledSpanExporter` (keep the struct and its
   `use std::future::pending;` local to the test fn), `BatchSpanProcessor` +
   `SdkTracerProvider` construction, `OtelService::with_defaults()` with
   `tracer_provider`/`status` set exactly as today, the arming span
   `provider.tracer("stall-tracer").start("stalled-op").end()`, and the 15s
   deadline assert with its exact message string
   "stop() must be bounded when span export stalls (rc-6ju71)". Keep both
   `#[test] #[serial_test::serial]` attribute pairs untouched.
4. Delete the now-duplicated scaffold code and any fn-local imports it owned
   (`use std::panic::{AssertUnwindSafe, catch_unwind};`,
   `use std::sync::mpsc;`) from both test fns — they live in the helper now.
   No other import changes.

**Tests:**
- `harness-single-source`: BEFORE implementation run
  `grep -rc 'mpsc::channel::<Result<(), String>>' crates/services/camel-otel/src/` →
  2 matches today; AFTER → exactly 1 (in `bounded_repro`). Same 2→1 for
  `recv_timeout(Duration::from_secs(60))`.
- `test_stop_bounded_when_metric_export_stalls`: spawned via
  `bounded_repro("q74u", "rc-q74u", …)` → passes; thread name resolves to
  `q74u-repro`; hang diagnostics text unchanged (code-inspection assert:
  the two panic! literals in the helper match the pre-refactor strings
  byte-for-byte).
- `test_stop_bounded_when_span_export_stalls`: spawned via
  `bounded_repro("q6ju71", "rc-6ju71", …)` → passes; thread name
  `q6ju71-repro`; diagnostics unchanged.
- `test-identity-unchanged`: repeat the Task 1.1 list-diff oracle → empty.
- Command: `cargo test -p camel-otel` → all pass.

**Acceptance:**
- Both greps report exactly 1 match in the crate.
- `cargo test -p camel-otel` exits 0 (both stall repros green).
- The Task 1.1 list-diff oracle is still empty.
- `cargo fmt --check --all` exits 0;
  `cargo clippy -p camel-otel --all-targets -- -D warnings` exits 0
  (no unused-import warnings in the `#[cfg(test)]` module — plain clippy
  without `--all-targets` would not catch them).
- Zero edits to the two 15s-deadline assertion strings and both deadline
  constants (`Duration::from_secs(15)`, `Duration::from_secs(60)`).

- [x] 1.2

### Task 1.3: CONTEXT.md layout note + ADR-0012 anchor refresh

**Files:**
- `crates/services/camel-otel/CONTEXT.md` (modified)

**Steps:**
1. Locate the two ADR-0012-annotated sites in the post-refactor
   `service.rs` by content, not by number (the table's current numbers are
   stale): the `error!` whose message contains "OTel config validation
   failed" (start path), and the `warn!` that begins the body of
   `fn drop` (the Drop-impl "dropped without stop" site; at 6b110708 they
   sit at 377 and 534 respectively, both shifting +3 from the Task 1.1
   wiring lines — measure with
   `grep -n 'error!\|warn!' crates/services/camel-otel/src/service.rs`,
   do not trust arithmetic). Update the `## ADR-0012 log-policy
   annotations` table rows to the measured new line numbers.
2. Append a short `## Test layout` section after `## Operational invariants`:
   unit tests for `OtelService` live in `src/service_tests.rs` (wired as
   `#[cfg(test)] #[path] mod tests` beside `sampler_tests.rs`); the
   bounded-stop stall repros (`rc-q74u`, `rc-6ju71`) share the
   `bounded_repro` helper defined there; new provider-path stall repros
   (e.g. logs) must reuse it instead of copying the thread/channel scaffold.
3. Run `cargo xtask lint-context-citations` from the worktree root.

**Tests:**
- `anchors-match`: for each row in the ADR-0012 table, the cited
  `src/service.rs` line contains the annotated log macro
  (`grep -n` at the exact line shows `error!` / `warn!`).
- `context-citations-green`: `cargo xtask lint-context-citations` → exit 0.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- Both table rows point at lines that actually contain the annotated macro.
- The note names `service_tests.rs`, `bounded_repro`, and the logs-path rule
  for future repros.

- [x] 1.3
