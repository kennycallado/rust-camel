# Tasks: subsume

## scripts/xtask (lint-unbounded-wait)

### Task 1.1: Per-await-site loop boundedness in lint_unbounded_wait

**Files:**
- `scripts/xtask/src/lint_unbounded_wait.rs` (modified)

**Steps:**
1. Add a `LoopAwaitCollector<'a>` visitor next to `AwaitSeeker`
   (around line 1407). It carries `ResolvesPaths` fields
   (`chain`, `body_top`, `body_nested`, `non_terminal_locals` —
   same wiring as `TimeoutSeeker` uses today) plus three pieces of
   state: `sites: Vec<(Span, SiteBase)>` where `SiteBase` is a new
   enum `{ TimeoutCall, Ident(String), Other }`, `bound_idents:
   HashSet<String>`, and `shadowed_idents: HashSet<String>`.
   Scope pruning identical to `AwaitSeeker`: empty-body
   `visit_expr_closure`, `visit_item_fn`, `visit_impl_item_fn`,
   `visit_trait_item_fn`; async blocks and nested loops traversed.
2. In `LoopAwaitCollector::visit_expr_await`: classify the base via
   `strip_parens` — a `Call` whose func is an `Expr::Path` satisfying
   `is_bounding_path` pushes `(span_of(e), SiteBase::TimeoutCall)`;
   a single-segment `Expr::Path` pushes
   `(span_of(e), SiteBase::Ident(name))`; anything else pushes
   `(span_of(e), SiteBase::Other)`. Then recurse with
   `visit::visit_expr_await`.
3. In `LoopAwaitCollector::visit_local`: extract pattern idents
   (reuse the `PatIdents` walker at line 1071). If the local has
   exactly one pattern ident and its initializer (after
   `strip_parens`) is a `Call` whose func path satisfies
   `is_bounding_path`, insert that ident into `bound_idents`;
   otherwise insert every pattern ident of this local into
   `shadowed_idents`. Then recurse.
4. Rewrite `WaitFinder::visit_expr_loop` (line 1541): keep the
   `bounded(loop_span)` / `subsumed(loop_span)` early-out. Replace the
   `AwaitSeeker` + `TimeoutSeeker` pair with one `LoopAwaitCollector`
   run over `e.body`. If `sites` is empty, do not report (matches
   `loop_without_await_not_reported`). Otherwise the loop is bounded
   iff EVERY site satisfies at least one of: (a)
   `self.bounded(site_span)` — the existing `future_regions`
   enclosure check; (b) `SiteBase::TimeoutCall`; (c)
   `SiteBase::Ident(n)` where `bound_idents.contains(n)` and NOT
   `shadowed_idents.contains(n)`. If any site fails, push the loop
   span onto `loop_spans` and report exactly as today (marker check
   via `self.suppressed(sp.0.line, sp.1.line)` before pushing the
   `Finding { line: sp.0.line }`).
5. Delete the `TimeoutSeeker` struct, its `ResolvesPaths` impl, its
   `Visit` impl, and its doc comment (lines ~1422-1457) — its only
   caller was the old carve-out. Do not delete `AwaitSeeker` (verify
   no remaining callers; if none remain outside tests, delete it too
   and note it in the result).
6. Add the tests listed below to `mod tests` next to the existing
   boundedness tests (after `loop_with_per_iteration_timeout_not_reported`,
   line ~1758). Use the existing `findings(src)` helper and
   `#[tokio::test]`-shaped source strings like the neighboring tests.

**Tests:** (all in `mod tests`, command
`cargo test -p xtask lint_unbounded_wait` from the worktree.
Every src below uses the same layout as the existing tests —
`#[tokio::test]` on line 1, `async fn t() {` on line 2, `loop {` on
line 3 — so the expected finding list is always `vec![3]`.)
- `dropped_sibling_timeout_does_not_bound_loop_waits`:
  src = tokio::test fn whose body is
  `loop { let _ = tokio::time::timeout(d, async { tick(); }); rx.recv().await; }`
  → assert `findings(src) == vec![3]`.
  Expected: FAIL before implementation (currently suppressed by
  TimeoutSeeker), PASS after.
- `timeout_in_disjoint_branch_does_not_bound_loop_waits`:
  body = `loop { if go() { tokio::time::timeout(d, a()).await; } else { rx.recv().await; } }`
  → assert findings == vec![3]. FAIL before, PASS after.
- `inner_timeout_not_enclosing_loop_waits_reported`:
  body = `loop { let _ = tokio::time::timeout(d, other()).await; rx.recv().await; }`
  → assert findings == vec![3]. FAIL before, PASS after.
- `rebound_local_timeout_reported`:
  body = `loop { let f = tokio::time::timeout(d, rx.recv()); let f = other(); f.await; }`
  → assert findings == vec![3]. FAIL before, PASS after.
- `loop_awaits_timeout_local_binding_not_reported`:
  body = `loop { let f = tokio::time::timeout(d, rx.recv()); match f.await { Ok(v) => { drop(v); } Err(_) => break, } }`
  → assert findings is empty. PASS before AND after (regression pin
  for rule c).
- `timeout_inside_spawned_closure_does_not_bound_loop`:
  body = `loop { tokio::spawn(async { let _ = tokio::time::timeout(d, tick()).await; }); rx.recv().await; }`
  → assert findings == vec![3]. FAIL before, PASS after.
- `await_in_unenclosed_async_block_in_loop_reported`:
  body = `loop { let b = async { rx.recv().await; }; b; tokio::time::timeout(d, other()).await; }`
  → assert findings == vec![3]. FAIL before, PASS after.
- Keep-green verification (no edits): `loop_with_per_iteration_timeout_not_reported`,
  `loop_inside_timeout_not_reported`, `nested_loop_subsumed_by_outer_loop_finding`,
  `loop_with_await_reported_and_subsumes_waits`,
  `timeout_region_does_not_leak_to_later_waits`,
  `glob_imported_timeout_bounds_await_loop`,
  `recv_inside_aliased_timeout_not_reported`,
  `glob_in_scope_qualified_timeout_still_bounds`,
  `marker_on_loop_line_suppresses_loop`,
  `named_import_bounds_despite_same_scope_glob` all still pass.

**Acceptance:**
- `cargo test -p xtask lint_unbounded_wait` exits 0 with the 7 new
  tests plus all 78 pre-existing in-module tests passing.
- `cargo fmt --check` and
  `cargo clippy -p xtask --all-targets -- -D warnings` exit 0.
- `rg -n "TimeoutSeeker" scripts/xtask/src/` returns no hits.
- Spec scenarios covered: dropped-sibling, disjoint-branch,
  await-on-timeout (keep-green), awaited-local-binding, rebound-local,
  spawned-closure, unenclosed-async-block.

### Task 1.2: Ratchet verdict and full gate run

**Files:**
- `scripts/xtask/ratchet-unbounded-wait.max` (modified only if
  inventory-justified raise occurs)
- possibly member-tree test files listed by the lint output (modified
  only for bounding newly-surfaced true positives; each edit wraps the
  site in `tokio::time::timeout` or adds an adjudicated marker)

**Steps:**
1. Capture the pre-change baseline finding list: create a scratch
   worktree at the pre-implementation commit
   (`git -C <worktree> worktree add --detach <tmpdir> <sha-of-spec-commit>`),
   set its `scripts/xtask/ratchet-unbounded-wait.max` to `0`, run
   `cargo run -p xtask -- lint-unbounded-wait` there, save the printed
   finding list to a file, restore the ratchet value, remove the
   scratch worktree. (The CLI prints the full list only when the
   count exceeds the max, which is why the max is temporarily 0.)
2. From the main worktree run
   `cargo run -p xtask -- lint-unbounded-wait` and capture the exit
   code and the reported count.
3. If the count equals 395 and exit code is 0: record "ratchet
   exact-green 395" — done, no file changes in this task.
4. If the count exceeds 395: run the lint again with
   `ratchet-unbounded-wait.max` temporarily set to `0` in the main
   worktree (restore it before committing), diff the printed list
   against the baseline file from step 1, and for EACH new finding
   open the site and classify: true positive (an await in a loop that
   no timeout wraps) or false positive (a legitimately bounded site
   the new rules miss). Fix false positives in the detector and
   re-run. For true positives either bound the site in-tree (wrap the
   wait in `tokio::time::timeout(d, ..)` matching neighboring test
   style) or, if the site list is large, raise
   `ratchet-unbounded-wait.max` to the new count. Record the
   per-finding inventory (file, line, classification, action) in the
   task result message — it goes into the park report and a bd
   comment on rc-ohddw.
5. Run the full local gate set from the worktree:
   `cargo fmt --check --all`,
   `cargo clippy -p xtask --all-targets -- -D warnings`,
   `cargo test -p xtask`,
   `cargo build --workspace`.

**Tests:** (verification commands, expected outcomes)
- `cargo run -p xtask -- lint-unbounded-wait`: exits 0 with count
  395, OR exits non-zero with an inventory whose every entry is
  classified in the result message.
- `cargo test -p xtask`: exits 0 (the full xtask suite — module,
  ratchet, and CLI tests).
- `cargo build --workspace`: exits 0.
- `cargo fmt --check --all`: exits 0.
- `cargo clippy -p xtask --all-targets -- -D warnings`: exits 0.

**Acceptance:**
- Ratchet verdict recorded: exact-green 395 with
  `ratchet-unbounded-wait.max` unchanged, or a raise committed
  together with the inventory list in the result message.
- All four gate commands exit 0.
- Spec scenario covered: ratchet-verdict-recorded-on-count-movement.

- [x] 1.1
- [x] 1.2
