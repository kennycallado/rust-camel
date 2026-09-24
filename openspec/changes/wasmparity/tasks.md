# Tasks: wasmparity

## camel-integration-test

### Task 1.1: Normalize the scenario boot root so the wasm base dir is never empty

**Files:**
- `crates/camel-integration-test/src/boot_scenario.rs` (modified)
- `crates/camel-integration-test/Cargo.toml` (modified)
- `crates/camel-integration-test/tests/wasm_boot_test.rs` (new)
- `crates/camel-integration-test/tests/fixtures/wasm/echo.wasm` (new — copy of `examples/wasm-example/fixtures/echo.wasm`)

**Steps:**
1. Copy `examples/wasm-example/fixtures/echo.wasm` to `crates/camel-integration-test/tests/fixtures/wasm/echo.wasm` (committed binary fixture, precedent: `crates/components/camel-component-wasm/tests/fixtures/init-check.wasm`).
2. In `crates/camel-integration-test/Cargo.toml` `[features]`, add the wasm gate forward (exact precedent — the `sql` forward at line 106-109 with its lint-gate-forwarding Rule 1 comment):
   ```toml
   # Demand-gated WASM activation: forwards camel-bundles' wasm gate so
   # the scenario boot registers the wasm bundle (base-dir fix rc-l3zrr).
   # The harness is a bundles consumer, so the shadow feature must
   # forward it (lint-gate-forwarding Rule 1).
   wasm = ["camel-bundles/wasm"]
   ```
   Without this forward, the `#[cfg(feature = "wasm")]` registration in camel-bundles (src/lib.rs:412-425) never compiles into the test build and the wasm route fails `Component not found: wasm` even after the root fix.
3. In `crates/camel-integration-test/src/boot_scenario.rs`, at the top of `pub async fn boot_scenario` (before the `doc_dir`/`config_path` derivation), add root normalization with a doc comment citing `camel run`'s `try_canonical_project_root` empty-parent rule (crates/camel-cli/src/commands/run.rs:90-101):
   ```rust
   // An empty root (a document named as a bare relative filename)
   // resolves its joins against the process CWD — fine for config load
   // and route files, fatal for the wasm base dir, whose empty
   // canonicalize() fails. Normalize to "." — the same empty-parent rule
   // `camel run` applies (try_canonical_project_root).
   let root: &Path = if root.as_os_str().is_empty() {
       Path::new(".")
   } else {
       root
   };
   ```
4. Write `crates/camel-integration-test/tests/wasm_boot_test.rs` modeled on the harness of `crates/camel-integration-test/tests/common/mod.rs` (`run_logs_document`: `ensure_capture_subscriber` → `parse_scenario_document` → `boot_scenario` → `run_scenario_document`), but booting with a caller-chosen root: a tempdir project containing `Camel.toml` (empty file), `routes/wasm.yaml` declaring `from: "direct:start"` with steps `to: "wasm:echo.wasm"` then `to: "log:wasmout"`, the copied `echo.wasm` guest at the project root, and a scenario document `wasm.test.yaml` declaring `routeFiles: [routes/wasm.yaml]`, one `send` action to `direct:start` with body `"hello-wasm"`, and a `logs:` `contains:` assertion on the body marker (the log line exists only if the exchange traversed the wasm step; fixture text mirrors `log_assertion_test.rs` shapes). The empty-root test MUST hold the defect's CWD premise: acquire `common::RUN_LOCK`, save the current dir, `std::env::set_current_dir(project_tempdir)`, run the boot+document sequence, and restore the previous dir in a drop guard — with CWD elsewhere the empty root's `"Camel.toml"` join and the `"."` base dir both resolve against the wrong directory and the test fails for the wrong reason.

**Tests:** (executable spec — name, arrange, act, assert)
- `empty_boot_root_boots_wasm_route_and_executes_step`: under `RUN_LOCK` with CWD set to the tempdir project root (step 4), scenario document parsed with `parse_scenario_document` (source_path inside the tempdir), capture subscriber installed → call `boot_scenario(&doc, Path::new(""), &env)` with the deliberately EMPTY root (the defect shape: bare relative filename invocation from the project directory) → assert `Ok(ScenarioRun)` (before fix steps 2-3 this fails with `Endpoint creation failed: failed to resolve base directory: ` on the empty path); then `run_scenario_document` executes the `send` action and the `logs` assertion → assert the `DocumentOutcome` has every `per_action` entry `Ok` and no `final_failure` (the wasm echo step executed and the body reached the log endpoint); then `run.boot.shutdown(&mut run.ctx)` completes without error; drop guard restores the previous CWD. Command: `cargo test -p camel-integration-test --features wasm --test wasm_boot_test`. Expected: fails before steps 2-3 (empty-base-dir boot failure), passes after.
- `absolute_boot_root_wasm_boot_unchanged`: same tempdir project and harness, NO CWD change (absolute paths throughout) → `boot_scenario(&doc, root_abs, &env)` with the absolute tempdir root → assert `Ok(ScenarioRun)` and the same all-`Ok` `DocumentOutcome` (regression: non-empty roots keep identical behavior). Command: same. Expected: passes both before and after the fix.

**Acceptance:**
- `cargo test -p camel-integration-test --features wasm --test wasm_boot_test` passes (both tests).
- `cargo fmt --check`, `cargo clippy -p camel-integration-test --features wasm --all-targets -- -D warnings`, and `cargo xtask lint-gate-forwarding` exit 0.
- `crates/camel-integration-test/src/boot_scenario.rs` change is the root normalization only — no signature change, no other behavioral edit.

- [x] 1.1

## camel-cli

### Task 2.1: Honest `full*` annotation, advisory, and actionable lean-registry failure for FULL-derived unit documents

**Files:**
- `crates/camel-cli/src/commands/test.rs` (modified)
- `crates/camel-cli/src/commands/test/driver_tests.rs` (modified)
- `crates/camel-cli/src/commands/test/junit.rs` (modified — doc comment only)

**Steps:**
1. In `crates/camel-cli/src/commands/test.rs`, replace `fn tier_label(tier: Tier) -> &'static str` (lines 307-313) with a context-aware labeler: `fn tier_label(unit: bool, tier: Tier) -> &'static str` returning `"lean"` for `(unit=true, Tier::Lean)`, `"full*"` for `(unit=true, Tier::Full)`, and `"full"` for `(unit=false, _)` — scenario documents are always FULL. Update the call site (line 480) to pass `matches!(doc_variant, Unit(..))`; the doc-variant is available at that point via the `loaded` value before the `match loaded` at line 536 (derive `unit` from the `document::ParsedDocument` arm or the `LoadedDoc` value — one boolean, no struct change).
2. In `run_tests_full`, immediately after the tier annotation line written at line 535 (`writeln!(out, "{} [{label}]", path.display())`), add: when the document is a unit document whose derived tier is FULL, write one stderr advisory `R-UNIT-FULL: derived full, executed on the lean registry (direct, log, mock, seda, timer — ADR-0064); components outside the lean set need a scenario document` (pattern precedent: the `R-REPOSITORY-STUB` stderr line, lines 527-532).
3. In the `LoadedDoc::Unit` result arm (lines 577-589), when `result.doc_error` is `Some` AND the derived tier is FULL, append to the doc-error string (before it is written to `err` and stored in `junit::DocReport`): ` — unit documents execute on the lean registry (direct, log, mock, seda, timer); use a scenario document for wasm and other full-boot components`. The condition is tier-derived only — no error-message parsing, no `CamelError` variant matching.
4. Update the `junit.rs` doc comment (line 24: "(`lean` / `full`)") to "(`lean` / `full` / `full*` for FULL-derived unit documents)". No code change in junit.rs — the `tier` property already mirrors the label string. Also update the `test.rs` module doc (line 16: "`[lean]`/`[full]` tier annotation line") to name the `full*` form for FULL-derived unit documents.
5. Add the three tests below to `crates/camel-cli/src/commands/test/driver_tests.rs`, following that file's existing tempdir + `run_tests_full` harness patterns.

**Tests:** (executable spec — name, arrange, act, assert)
- `unit_wasm_doc_annotates_full_star_with_advisory`: tempdir with `route.yaml` (`from: "direct:start"`, steps `to: "wasm:echo.wasm"`, `to: "mock:result"` — no wasm guest file needed, the lean registry misses before any file resolution) and `doc.test.yaml` (unit vocabulary: `routeFiles: [route.yaml]`, one input to `direct:start`, `expects: mock:result: count: 1`) → run `run_tests_full` with that file, capturing `out` and `err` → assert `out` contains the line `doc.test.yaml [full*]`; assert `err` contains `R-UNIT-FULL:` and the list `direct, log, mock, seda, timer`; assert summary exit code 2. Command: `cargo test -p camel-cli --lib unit_wasm_doc`. Expected: fails before (label is `[full]`, no advisory), passes after.
- `unit_wasm_failure_names_lean_registry_and_alternative`: same fixture → run `run_tests_full` → assert the reported doc-error line contains `Component not found: wasm` AND `lean registry` AND `scenario document` in one string (the appended hint), i.e. the failure is never the bare `Component not found: wasm`. Command: same. Expected: fails before (bare message), passes after.
- `lean_doc_annotation_stays_lean`: tempdir with a lean route (`direct:start` → `mock:result`) and unit doc → run `run_tests_full` → assert `out` contains `doc.test.yaml [lean]` exactly (regression: the labeler refactor does not disturb lean documents) and no `R-UNIT-FULL` line appears. Command: same. Expected: passes before and after.

**Acceptance:**
- `cargo test -p camel-cli --lib` passes (all existing driver/scenario/junit tests plus the three new ones).
- `cargo fmt --check` and `cargo clippy -p camel-cli -- -D warnings` exit 0.
- The lean registry registration in `runner.rs` is untouched (ADR-0064 set unchanged); `camel run` files (`run.rs`, `run_tests.rs`) untouched.

- [x] 2.1
