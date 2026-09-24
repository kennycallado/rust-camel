# Tasks: lintwiden

Single-phase change. Tasks run in order; Tasks 3–5 depend on Task 2's
Appendix B inventory, Tasks 4–5 may interleave. Spec delta ships in
this change dir (`specs/unbounded-wait-bounding/spec.md`, authored at
proposal time) — enforcement and spec land together at park.

## Task 1 — Widen scanner scope (tests/ dirs + cfg(test) modules)

- **Files**
  - `scripts/xtask/src/lint_unbounded_wait.rs` (modified)
- **Steps**
  1. Add `fn under_tests_dir(file: &Path) -> bool` — true when any
     `std::path::Component::Normal` of `file` is exactly `tests`.
  2. Add `fn is_cfg_test_mod(m: &syn::ItemMod) -> bool` — true when an
     attribute's path is `cfg` and its meta list tokens are exactly
     `test`.
  3. Thread a scan context through item recursion: extend
     `scan_items(items, chain, lines, findings)` with a
     `ctx: ScanCtx { under_tests_dir: bool, in_cfg_test: bool }`
     parameter (root call from `scan_source` passes
     `under_tests_dir` from the file path, `in_cfg_test: false`;
     `Item::Mod` recursion passes `in_cfg_test: ctx.in_cfg_test ||
     is_cfg_test_mod(m)` and preserves `under_tests_dir`).
  4. Widen the dispatch: `Item::Fn(f) if is_test_fn(f) ||
     ctx.under_tests_dir || ctx.in_cfg_test` calls the existing
     fn-body scan (`scan_test_fn` — rename to `scan_fn_body` with the
     call sites updated, mechanics unchanged).
  5. Update the module doc comment scope statement (top of file,
     `# Why structural, not lexical` section and the `# Enforcement`
     section): scan scope is test-attributed fns everywhere, plus
     non-test fns under `tests/` path components and inside
     `#[cfg(test)]` module subtrees; associated fns stay out of
     scope; spawned-closure pruning unchanged.
  6. Add the unit tests listed under Tests to the existing
     `#[cfg(test)] mod tests` (fixtures select scope by path:
     `Path::new("fixture.rs")` = src scope, `Path::new("tests/
     fixture.rs")` = widened scope).
- **Tests** (all in `scripts/xtask/src/lint_unbounded_wait.rs`)
  - name: `helper_fn_under_tests_dir_reported`
    setup: source `async fn helper() { let v = rx.recv().await; }` at
    known line, no test attribute
    action: `scan_source(src, Path::new("tests/fixture.rs"))`
    assert: findings == vec![that line]
    command: `cargo test -p xtask helper_fn_under_tests_dir_reported`
    expected: fails before the widening (no finding), passes after
  - name: `helper_fn_in_src_not_reported`
    setup: same source as above
    action: `scan_source(src, Path::new("fixture.rs"))`
    assert: findings empty
    command: `cargo test -p xtask helper_fn_in_src_not_reported`
    expected: passes before and after (production scope unchanged)
  - name: `helper_fn_in_cfg_test_module_reported`
    setup: source `#[cfg(test)] mod t { async fn helper() { let v =
    rx.recv().await; } }` at known line
    action: `scan_source(src, Path::new("fixture.rs"))`
    assert: findings == vec![helper's wait line]
    command: `cargo test -p xtask helper_fn_in_cfg_test_module_reported`
    expected: fails before, passes after
  - name: `cfg_test_submodule_helper_reported`
    setup: `#[cfg(test)] mod outer { mod inner { async fn helper() {
    let v = rx.recv().await; } } }`
    action: `scan_source(src, Path::new("fixture.rs"))`
    assert: findings == vec![wait line]
    command: `cargo test -p xtask cfg_test_submodule_helper_reported`
    expected: fails before, passes after
  - name: `helper_fn_in_plain_module_not_reported`
    setup: `mod t { async fn helper() { let v = rx.recv().await; } }`
    (no cfg)
    action: `scan_source(src, Path::new("fixture.rs"))`
    assert: findings empty
    command: `cargo test -p xtask helper_fn_in_plain_module_not_reported`
    expected: passes before and after
  - name: `test_fn_findings_identical_under_both_scopes`
    setup: `#[tokio::test] async fn t() { let v = rx.recv().await; }`
    action: scan the same source under both paths
    assert: both return the same single finding at the same line
    command: `cargo test -p xtask test_fn_findings_identical_under_both_scopes`
    expected: passes before and after
  - name: `spawned_closure_in_helper_fn_not_reported`
    setup: `async fn helper() { tokio::spawn(async move { let v =
    rx.recv().await; }); }` under `tests/fixture.rs` (spawn call not
    awaited)
    action: `scan_source(src, Path::new("tests/fixture.rs"))`
    assert: findings empty
    command: `cargo test -p xtask spawned_closure_in_helper_fn_not_reported`
    expected: passes after (pruning preserved; would fail if widening
    unrolled spawned closures)
  - name: `main_fn_under_tests_dir_reported`
    setup: `async fn main() { let v = rx.recv().await; }` under
    `tests/fixture.rs`
    action: `scan_source(src, Path::new("tests/fixture.rs"))`
    assert: findings == vec![wait line]
    command: `cargo test -p xtask main_fn_under_tests_dir_reported`
    expected: fails before, passes after
  - name: `helper_fn_in_tests_rs_file_not_reported`
    setup: non-test `async fn helper` with an unenclosed wait,
    scanned under the path `src/tests.rs` (a FILE named tests.rs —
    no `tests` directory component)
    action: `scan_source(src, Path::new("src/tests.rs"))`
    assert: findings empty
    command: `cargo test -p xtask helper_fn_in_tests_rs_file_not_reported`
    expected: passes before and after
  - name: `helper_fn_in_cfg_all_test_module_not_reported`
    setup: `#[cfg(all(test, feature = "x"))] mod t { async fn
    helper() { let v = rx.recv().await; } }` — cfg list beyond a
    bare `test` is deliberately NOT widened
    action: `scan_source(src, Path::new("fixture.rs"))`
    assert: findings empty
    command: `cargo test -p xtask helper_fn_in_cfg_all_test_module_not_reported`
    expected: passes before and after
- **Acceptance**
  - `cargo test -p xtask` exits 0 (existing tests + the ten new
    tests; existing fixture results unchanged — no existing test
    edited except mechanical rename fallout).
  - `cargo run -q -p xtask -- lint-unbounded-wait` reports a count
    above 296 and EXITS 1 at this point — expected mid-task state
    (widening visible; adjudication is Tasks 3–5, ceiling
    deliberately untouched in this task; the nonzero exit is the
    ratchet doing its job, not a task failure).
  - `cargo clippy -p xtask -- -D warnings` exits 0.
  - `cargo fmt --check` clean for the file.

## Task 2 — AST inventory + Appendix B adjudication table

- **Files**
  - `openspec/changes/lintwiden/design.md` (modified — Appendix B
    appended)
- **Steps**
  1. Scratch capture: set `scripts/xtask/ratchet-unbounded-wait.max`
     to `0`, run `cargo run -q -p xtask -- lint-unbounded-wait >
     /tmp/lintwiden-widened.txt 2>&1`, restore the file to `296`
     immediately (verify with `git diff --stat` — the ratchet file
     must show no diff afterwards).
  2. Extract the `file:line` finding list (lines matching
     `^  /.+:[0-9]+$`, worktree prefix stripped), subtract
     `openspec/changes/lintwiden/baseline-sites.txt` (296 entries)
     with `comm`/`diff` — the remainder is the helper-fn inventory.
  3. For each remainder site: open the file, identify the enclosing
     helper fn name and the wait class (lock / recv / connect /
     spawned-handle await / loop / blocking_recv).
  4. Append `## Appendix B — AST inventory of newly-visible
     helper-fn sites` to design.md: one table row per site —
     `file:line | helper fn | wait class | adjudication class
     (D4.1–D4.5) | owner task (3/4/5)`. Rows for sites already
     bounded in-tree (e.g. the grpc drain converted by drainscope
     378dbe6d — it must NOT appear in the remainder) are not needed;
     the table lists only unadjudicated sites.
  5. Cross-check against the lexical seed (drainscope design.md
     Appendix A): every seed file appears in the table or is
     accounted for (converted by drainscope, or lexically-seeded but
     AST-bounded). Record the cross-check result in one sentence
     under the table.
- **Tests**
  - name: `appendix-b-completeness`
    setup: widened scratch capture from Step 1
    action: compare remainder count vs Appendix B row count
    assert: equal (every newly-visible site has a row with a
    non-empty adjudication class and owner task)
    command: manual diff of `/tmp/lintwiden-widened.txt` remainder vs
    `grep -c '^| ' openspec/changes/lintwiden/design.md` appendix
    section
    expected: passes when the table is complete
  - name: `ratchet-untouched`
    action: `git diff --stat scripts/xtask/ratchet-unbounded-wait.max`
    assert: empty output
    command: as stated
    expected: passes
- **Acceptance**
  - design.md contains Appendix B with ≥ 1 row per remainder site,
    every row carrying an adjudication class and owner task.
  - `git diff` shows only design.md modified for this task.
  - The ratchet file still reads 296.

## Task 3 — Adjudicate camel-test helper sites (class D4.1 locks)

- **Files**
  - `crates/camel-test/tests/controlbus_test.rs` (modified)
  - `crates/camel-test/tests/direct_top_level_test.rs` (modified)
  - `crates/camel-test/tests/do_try_test.rs` (modified)
  - `crates/camel-test/tests/integration_test.rs` (modified)
  - `crates/camel-test/tests/jsonpath_test.rs` (modified)
  - `crates/camel-test/tests/loop_test.rs` (modified)
  - `crates/camel-test/tests/marshal_test.rs` (modified)
  - `crates/camel-test/tests/otel_direct_hop_regression.rs` (modified)
  - `crates/camel-test/tests/otel_trace_tree_test.rs` (modified)
  - `crates/camel-test/tests/script_test.rs` (modified)
  - `crates/camel-test/tests/xpath_test.rs` (modified)
  - `crates/camel-test/tests/cache_resilience.rs` (modified)
  - (any additional `crates/camel-test/**` file listed in Appendix B
    with owner task 3 — same treatment)
- **Steps**
  1. Read Appendix B rows owned by task 3 (all
     `crates/camel-test/tests/*` sites).
  2. For each lock-class site: replace `let g =
     <LOCK>.lock().await;` with `let g = acquire_deadline(&<LOCK>,
     "<LOCK_NAME> (<short context>)", TEST_LOCK_DEADLINE).await;`
     matching the c2a48f20 conversion shape; add `use
     camel_component_api::test_support::{TEST_LOCK_DEADLINE,
     acquire_deadline};` where missing (dev-dependency already
     declares `features = ["test-support"]`).
  3. For any non-lock site in these files (per Appendix B class):
     apply its D4 class — per-iteration deadline D-recipe for drains
     (D4.2), timeout wrap for connects (D4.3), marker with
     site-specific reason only where a deadline is semantically
     wrong (D4.4).
  4. Update Appendix B status column for every task-3 row
     (`done (converted)` / `done (marked: <reason>)`).
- **Tests**
  - name: `lint-clean-camel-test`
    setup: conversions applied
    action: scratch max-0 lint run (same procedure as Task 2 Step 1,
    restore afterwards)
    assert: remainder list contains zero sites under
    `crates/camel-test/`
    command: `cargo run -q -p xtask -- lint-unbounded-wait` with
    scratch max 0, remainder recomputed against baseline-sites.txt
    expected: passes after conversions
  - name: `camel-test-compiles`
    action: `cargo check --tests -p camel-test`
    assert: exit 0
    command: as stated
    expected: passes
- **Acceptance**
  - Zero unadjudicated helper findings under `crates/camel-test/**`.
  - `cargo check --tests -p camel-test` exits 0.
  - Appendix B task-3 rows all read done with the disposition noted.
  - `git diff --stat scripts/xtask/ratchet-unbounded-wait.max` empty.

## Task 4 — Adjudicate integration-test, dsl, cxf sites (+ grpc verify)

- **Files**
  - `crates/camel-integration-test/Cargo.toml` (modified — add a
    SEPARATE `[dev-dependencies]` entry `camel-component-api =
    { workspace = true, features = ["test-support"] }`; never touch
    the existing production `[dependencies]` line — the feature
    doc forbids production enablement)
  - `crates/camel-integration-test/tests/circuit_fallback_test.rs`
    (modified)
  - `crates/camel-integration-test/tests/direct_reply_test.rs`
    (modified)
  - `crates/camel-integration-test/tests/http_partner_scripting_test.rs`
    (modified)
  - `crates/camel-dsl/tests/rest_negotiation_e2e.rs` (modified)
  - `crates/components/camel-cxf/tests/consumer_unit_test.rs`
    (modified)
  - (any additional file listed in Appendix B with owner task 4)
- **Steps**
  1. Read Appendix B rows owned by task 4.
  2. Lock-class sites (camel-integration-test, cxf
     `wait_for_consumer_request_sender` /
     `wait_for_recorded_responses`): D4.1-mechanism conversion as in
     Task 3 — NOTE these are per-test STATE mutexes inside bounded
     retry loops, not process-global serialization locks: use
     `acquire_deadline` with a file-appropriate deadline (not the
     900 s `TEST_LOCK_DEADLINE` rationale) and record the Appendix B
     row as "acquire_deadline mechanism, state mutex". For
     camel-integration-test first add the separate dev-dependency
     entry per the Files note.
  3. `serve_gate` recv site in camel-dsl rest_negotiation_e2e.rs:
     D4.2 per-iteration deadline D-recipe with the file's existing
     deadline convention (drainscope shape).
  4. cxf `open_consumer_stream` connect site: D4.3 timeout wrap with
     a named deadline consistent with the file's conventions.
  5. Verify grpc: Appendix B must contain NO row for
     crates/components/camel-component-grpc/tests/integration.rs
     (drainscope 378dbe6d converted the drain); if a row exists,
     adjudicate it per its class and record why the seed cross-check
     missed it.
  6. Update Appendix B status for every task-4 row.
- **Tests**
  - name: `lint-clean-task4-files`
    action: scratch max-0 lint run, remainder vs baseline
    assert: zero sites in the task-4 file set
    command: `cargo run -q -p xtask -- lint-unbounded-wait` with
    scratch max 0
    expected: passes after conversions
  - name: `task4-crates-compile`
    action: `cargo check --tests -p camel-integration-test -p
      camel-dsl -p camel-component-cxf`
    assert: exit 0
    command: as stated
    expected: passes
- **Acceptance**
  - Zero unadjudicated helper findings in the task-4 file set.
  - `cargo check --tests -p camel-integration-test -p camel-dsl -p
    camel-component-cxf` exits 0.
  - grpc row absent from Appendix B (or adjudicated with recorded
    reason).
  - Ratchet file untouched.

## Task 5 — Residual sweep (all remaining Appendix B sites)

- **Files**
  - every file with an Appendix B row owned by task 5 (cfg(test)
    src-module helpers, spawned-handle awaits, loops,
    blocking_recv sites, benchmarks/fuzz/examples tests trees —
    the exact set is Appendix-B-defined; each listed there with
    file:line)
- **Steps**
  1. Read Appendix B rows owned by task 5 (all rows not owned by
     tasks 3–4).
  2. Apply each row's adjudication class: D4.1 lock conversions
     (adding the `test-support` dev-dependency feature where the
     crate lacks it, one Cargo.toml edit per crate, minimal), D4.2
     drain recipes, D4.3 connect wraps, D4.4 markers with
     site-specific justification text naming why a deadline is
     wrong for that wait.
  3. A site is D4.5 (ceiling entry) ONLY if genuinely unreachable by
     D4.1–D4.4 — collect any such site with its justification for
     the park notes; do not edit the ratchet in this task.
  4. Update Appendix B status for every task-5 row.
- **Tests**
  - name: `lint-total-at-baseline`
    action: scratch max-0 lint run, remainder vs baseline
    assert: remainder set == the D4.5 set (empty if no ceiling
    entries), i.e. every non-D4.5 site is converted or marked
    command: `cargo run -q -p xtask -- lint-unbounded-wait` with
    scratch max 0
    expected: passes when the sweep is complete
  - name: `sweep-crates-compile`
    action: `cargo check --tests` for each crate touched in this
    task
    assert: exit 0 per crate
    command: as stated
    expected: passes
- **Acceptance**
  - Scratch remainder contains only named D4.5 sites (target:
    none).
  - Every touched crate compiles with `--tests`.
  - Ratchet file untouched by this task.

## Task 6 — Final ceiling + gate run (conductor-executed)

- **Files**
  - `scripts/xtask/ratchet-unbounded-wait.max` (modified only if a
    D4.5 set exists or headroom opened)
  - `openspec/changes/lintwiden/design.md` (modified — Appendix B
    status column finalized)
- **Steps**
  1. Run `cargo run -q -p xtask -- lint-unbounded-wait` at ceiling
     296: expected `OK (296 findings = max 296)` when the D4.5 set
     is empty. If the observed count is BELOW 296, do NOT lower the
     ceiling in this step — stop and diff the scratch max-0 site
     list against `baseline-sites.txt` to find which test-fn finding
     vanished (test fns are untouched by design; a vanished finding
     means an import-resolution side effect or an unintended scope
     regression that must be understood before any ceiling change).
  2. If a non-empty D4.5 set exists: raise the ceiling ONCE to
     `296 + |D4.5|` with the justification recorded in the park
     notes and bd (mission allowance); otherwise leave 296.
  3. Full gate run (worktree): `cargo fmt --check --all`; `cargo
     clippy -p xtask -- -D warnings`; `cargo test -p xtask`; the 16
     xtask lints (`lint-unwrap`, `lint-secrets`,
     `lint-single-source`, `lint-non-exhaustive`,
     `lint-log-levels`, `lint-log-redaction`, `lint-cancel-tokens`,
     `lint-test-sleep`, `lint-unbounded-wait`, `lint-ignore`,
     `lint-publish-cycles`, `lint-publish-registration`,
     `lint-component-deps`, `lint-gate-forwarding`,
     `lint-context-citations`, `lint-metric-labels`); `cargo test -p
     camel-cli --lib`; `openspec validate lintwiden --type change`.
  4. Commit all work; write park notes; trigger the fleet buzzer;
     update bd rc-h2qwr. NO merge, NO archive — parked.
- **Tests**
  - name: `all-gates-green`
    action: run the gate list above
    assert: every command exits 0 (or N/A recorded with reason)
    command: as listed
    expected: passes
- **Acceptance**
  - All gates green, ceiling final value recorded in park notes
    (target: 296 unchanged).
  - Work committed on `feature/lintwiden`; buzzer fired (or park
    file + bd note as fallback signal per rc-2rzb4).
