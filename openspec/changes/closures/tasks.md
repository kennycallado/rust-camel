# Tasks: closures

Single-phase change (per `design.md ## Phases`): one detector
extension in `scripts/xtask/src/lint_unbounded_wait.rs`, in-module
tests, and a ratchet verdict. All tasks work in the worktree
`/home/shared/rust-camel-worktrees/closures`; never build or test
in the main checkout.

Shared context for every task (do not repeat in steps):
- The lint walks each `#[test]` / `#[tokio::test]` fn body in three
  passes orchestrated by `scan_test_fn` (~line 1275): SpawnCollector
  (names bound to spawn expressions), TimeoutCollector (timeout
  future-arg region spans), WaitFinder (reports unbounded waits).
- `type Span = (LineColumn, LineColumn)` (line 356); `span_of`,
  `span_contains`, `strip_parens` helpers sit right below it.
- `WaitFinder::visit_expr_closure` (~1654) and
  `LoopAwaitCollector::visit_expr_closure` (~1488) prune every
  closure body — this is the bug site.
- In-module tests live in `mod tests` (~1754) using the
  `findings(src) -> Vec<usize>` helper (1-based line numbers of
  expected findings).
- Gates for every task: `cargo fmt --check`,
  `cargo clippy -p xtask --all-targets -- -D warnings`,
  `cargo test -p xtask lint_unbounded` — all from the worktree root.
  No `unwrap()`/`expect()`/`panic!` additions in non-test code
  (`cargo xtask lint-unwrap` must stay green).

## xtask lint-unbounded-wait

### Task 1.1: Inline-closure mark pre-pass and prune-lift

**Files:**
- `scripts/xtask/src/lint_unbounded_wait.rs` (modified)

**Steps:**
1. Write the nine tests listed below FIRST, run
   `cargo test -p xtask lint_unbounded`, and confirm the RED set is
   exactly three failing tests —
   `awaited_iife_closure_body_reported`,
   `nested_awaited_iife_reports_once`,
   `sync_direct_call_blocking_reported` — while the other six pass
   vacuously (they expect empty findings; the current prune already
   produces empty, so they pin the behavior against regressions).
2. Add a `InlineClosureCollector<'a>` visitor struct with one
   field `marks: &'a mut HashSet<Span>` (plus no other state). Its
   `Visit` impl uses DEFAULT traversal (descends into closure
   bodies — marks are collected body-wide) and overrides
   `visit_expr_await` and `visit_expr_call`:
   - `visit_expr_await`: if `strip_parens(&e.base)` is
     `syn::Expr::Call(c)` and `strip_parens(c.func)` is
     `syn::Expr::Closure(cl)`, insert `span_of(cl)` into `marks`
     (the await drives the closure body inline — mark regardless
     of closure kind).
   - `visit_expr_call`: if `strip_parens(call.func)` is
     `syn::Expr::Closure(cl)` and the closure executes
     synchronously at call time — `cl.asyncness.is_none()` and
     `strip_parens(&cl.body)` is neither `syn::Expr::Async` nor
     `syn::Expr::TryBlock` — insert `span_of(cl)` into `marks`
     (the body's statements run at the call even without an
     await; an async-block body only builds a future).
3. Wire it into `scan_test_fn` as pass 0 (before SpawnCollector):
   `let mut inline_marks: HashSet<Span> = HashSet::new();
   InlineClosureCollector { marks: &mut inline_marks }
   .visit_block(f.block.as_ref());`
4. Add an `inline: &'a HashSet<Span>` field to `WaitFinder` and to
   `LoopAwaitCollector` (constructed in
   `WaitFinder::visit_expr_loop`, pass `self.inline`). In both,
   change `visit_expr_closure` from unconditional prune to:
   if `self.inline.contains(&span_of(c))` then
   `visit::visit_expr_closure(self, c)` (unroll) else prune
   (empty body, today's behavior). Update the prune comments to
   say "closures that do not execute in the test body run
   elsewhere (route builders, spawned work); inline closures
   (directly invoked and awaited, or sync and directly invoked)
   unroll".
5. Keep `LoopAwaitCollector` await-site classification UNCHANGED
   in this task (an IIFE-call await base still falls to
   `SiteBase::Other`); tail classification is task 1.2.
6. Update the module-doc scope sentence (line ~12 "closures,
   nested `fn` items, and associated fns are out of scope") to
   state: closure bodies whose closure executes in the test body —
   directly invoked and awaited, or synchronous and directly
   invoked — are analyzed as part of the enclosing body; all other
   closures stay out of scope. Also extend the line ~108
   "closure-internal call" mention if needed for accuracy.
7. Run the task gates; all tests green.

**Tests:** (executable spec — name, arrange, act, assert)
- `awaited_iife_closure_body_reported`: src
  `#[tokio::test]\nasync fn t() {\n    let v = (|| async { rx.recv().await })().await;\n}\n`
  → `findings(src)` equals `vec![3]` (bd rc-q2l8u repro: the inner
  receive await reports once unrolled).
- `iife_call_not_awaited_not_reported`: src
  `#[tokio::test]\nasync fn t() {\n    let f = (|| async { rx.recv().await })();\n    drop(f);\n}\n`
  → `findings(src)` empty (future never polled; boundary case for
  let-bound futures is out of scope this change).
- `closure_passed_as_argument_stays_pruned`: src
  `#[tokio::test]\nasync fn t() {\n    helper(|r: Rx| async move { r.recv().await });\n}\n`
  → `findings(src)` empty.
- `nested_awaited_iife_reports_once`: src
  `#[tokio::test]\nasync fn t() {\n    (|| async { (|| async { rx.recv().await })().await })().await;\n}\n`
  → `findings(src)` equals `vec![3]` exactly once (both bodies
  unroll; wrapper awaits are not sites).
- `sync_direct_call_blocking_reported`: src
  `#[test]\nfn t() {\n    let v = (|| { rx.blocking_recv() })();\n}\n`
  → `findings(src)` equals `vec![3]` (sync body executes at call
  time).
- `iife_in_timeout_future_argument_stays_pruned`: src
  `#[tokio::test]\nasync fn t() {\n    let _ = tokio::time::timeout(d, (|| async { rx.recv().await })()).await;\n}\n`
  → `findings(src)` empty (the closure call is an argument, never
  directly awaited).
- `unrolled_iife_inside_timeout_region_bounded`: src
  `#[tokio::test]\nasync fn t() {\n    let _ = tokio::time::timeout(d, async { (|| async { rx.recv().await })().await }).await;\n}\n`
  → `findings(src)` empty (directly-awaited IIFE unrolls inside
  the timeout future region; site is region-bounded).
- `timeout_inside_awaited_iife_bounds_body`: src
  `#[tokio::test]\nasync fn t() {\n    (|| async { let _ = tokio::time::timeout(d, rx.recv()).await; })().await;\n}\n`
  → `findings(src)` empty (unrolled body's only await is on the
  bounding timeout call).
- `iife_inside_spawned_closure_stays_pruned`: src
  `#[tokio::test]\nasync fn t() {\n    tokio::spawn(|| { (|| async { rx.recv().await })().await; });\n}\n`
  → `findings(src)` empty (mark exists body-wide but the spawn
  closure prunes before the IIFE is reachable).

Command: `cargo test -p xtask lint_unbounded` — the RED set before
step 2 is exactly the three tests above; all nine pass after
step 4.

**Acceptance:**
- All nine new tests pass; every pre-existing test in
  `cargo test -p xtask lint_unbounded` still passes unchanged
  (especially `waits_in_closure_not_reported`,
  `timeout_inside_spawned_closure_does_not_bound_loop`).
- `cargo fmt --check` and
  `cargo clippy -p xtask --all-targets -- -D warnings` exit 0.
- Module doc names the new closure scope rule.

- [x] 1.1

### Task 1.2: Awaited-IIFE tail classification

**Files:**
- `scripts/xtask/src/lint_unbounded_wait.rs` (modified)

**Steps:**
1. Write the six tests listed below FIRST and confirm the RED set
   is exactly four failing tests —
   `bare_future_tail_iife_awaited_reported`,
   `async_closure_tail_reported`,
   `block_tail_final_expr_stmt_reported` (nothing reports the outer
   await yet), and `finite_tail_iife_in_loop_not_reported` (the
   closure-func await base hits LoopAwaitCollector's opaque-call
   `_ => Some(SiteBase::Other)` fallback, so the loop is reported
   today) — while two pass:
   `finite_tail_iife_awaited_not_reported` (vacuous — nothing
   reports it today either) and
   `awaited_iife_in_loop_reported_once` (already green via the
   same `SiteBase::Other` fallback; it must STAY green through the
   classification change).
2. Add an enum `IifeTail { AsyncBody, WaitTail, Other }` near
   `SiteBase` (~line 1440) with a doc comment: classification of an
   awaited directly-invoked closure by what the await drives.
3. Add a free function
   `fn classify_iife_tail<R: ResolvesPaths>(r: &R, call: &syn::ExprCall) -> IifeTail`
   that: extracts the callee closure via `strip_parens` on
   `call.func` (return `IifeTail::Other` if it is not a closure);
   takes the stripped closure body; if the body strips to
   `syn::Expr::Async` or `syn::Expr::TryBlock` returns
   `AsyncBody`; else extracts the tail expression — the stripped
   body itself, or, when the body is `syn::Expr::Block`, the
   expression of its FINAL statement when that statement is
   `syn::Stmt::Expr` (otherwise return `IifeTail::Other`); then
   returns    `WaitTail` when that tail strips to a wait-class call —
   `syn::Expr::MethodCall` whose method is in `WAIT_METHODS` or
   named `spawn`, or `syn::Expr::Call` whose func is a
   `syn::Expr::Path` with `r.is_wait_path(&pe.path)` — and
   `IifeTail::Other` otherwise.
4. Extend `WaitFinder::visit_expr_await`'s `syn::Expr::Call` arm
   (currently only matches `Expr::Path` funcs): when the func
   strips to a closure, call `classify_iife_tail(self, c)` and on
   `WaitTail` run `self.maybe_report(sp)` (bounded / subsumed /
   suppressed rules apply unchanged); `AsyncBody` and `Other`
   report nothing at the outer await (inner sites report via the
   task-1.1 unroll).
5. Extend `LoopAwaitCollector::visit_expr_await`'s
   `syn::Expr::Call` base match: when the func strips to a
   closure, map `classify_iife_tail(self, c)` to sites —
   `WaitTail` → `Some(SiteBase::Other)` (candidate site),
   `AsyncBody` and `Other` → `None` (no site) — so the loop rule
   and the standalone rule agree on every IIFE shape. Update the
   arm's comment to say the IIFE classification mirrors
   WaitFinder.
6. Update the module-doc detector-class list (line ~20) with one
   sentence: an awaited directly-invoked closure whose tail is a
   wait-class call is reported at the await site.
7. Run the task gates; all tests green.

**Tests:** (executable spec)
- `bare_future_tail_iife_awaited_reported`: src
  `#[tokio::test]\nasync fn t() {\n    let v = (|| rx.recv())().await;\n}\n`
  → `findings(src)` equals `vec![3]`.
- `async_closure_tail_reported`: src
  `#[tokio::test]\nasync fn t() {\n    let v = (async || rx.recv())().await;\n}\n`
  → `findings(src)` equals `vec![3]` (async-closure syntax, bare
  tail, no inner await).
- `block_tail_final_expr_stmt_reported`: src
  `#[tokio::test]\nasync fn t() {\n    let v = (|| { prep(); rx.recv() })().await;\n}\n`
  → `findings(src)` equals `vec![3]` (tail read through the final
  expression statement).
- `finite_tail_iife_awaited_not_reported`: src
  `#[tokio::test]\nasync fn t() {\n    (|| tokio::time::sleep(d))().await;\n}\n`
  → `findings(src)` empty.
- `awaited_iife_in_loop_reported_once`: src
  `#[tokio::test]\nasync fn t() {\n    loop {\n        (|| async { rx.recv().await })().await;\n    }\n}\n`
  → `findings(src)` equals `vec![3]` (loop line once; inner site
  subsumed).
- `finite_tail_iife_in_loop_not_reported`: src
  `#[tokio::test]\nasync fn t() {\n    loop {\n        (|| tokio::time::sleep(d))().await;\n    }\n}\n`
  → `findings(src)` empty.

Command: `cargo test -p xtask lint_unbounded`.

**Acceptance:**
- All six new tests pass; full `cargo test -p xtask lint_unbounded`
  green including task-1.1 tests.
- `cargo fmt --check` and
  `cargo clippy -p xtask --all-targets -- -D warnings` exit 0.

- [x] 1.2

### Task 1.3: Ratchet verdict and boundary documentation

**Files:**
- `scripts/xtask/ratchet-unbounded-wait.max` (modified — verdict
  comment; integer unchanged unless the run finds movement, in
  which case the inventory below governs)
- `scripts/xtask/src/lint_unbounded_wait.rs` (modified — module-doc
  boundary paragraph only, no logic)

**Steps:**
1. Run `cargo xtask lint-unbounded-wait` from the worktree root
   and record the total count against the ceiling 393
   (`scripts/xtask/ratchet-unbounded-wait.max` line 596).
2. If the count is 393: append one line to the ratchet header
   comment block (above the inventory section):
   `# Closure-unroll verdict (2026-09-23, bd rc-q2l8u): 393 -> 393,
   # zero movement — corpus contains no directly-invoked closure
   # shapes; unroll and tail classification are net-new coverage.`
   and change nothing else in the file.
3. If the count differs from 393: STOP and report the exact added
   or removed findings (file:line) to the conductor — do not edit
   the ceiling yourself; the conductor owns the inventory and the
   movement decision (mission order requirement).
4. Extend the module-doc boundary paragraph (the one listing
   stream I/O and macro-body blind spots, ~lines 41-51) with:
   binding-indirection forms stay pruned — a future built by a
   directly-invoked closure but let-bound before awaiting
   (`let f = (|| async { .. })(); f.await`), a closure let-bound
   before its direct call (`let c = || { .. }; c()`), and curried
   multi-level calls (`((f())())()`); their waits are conservative
   false negatives tracked in bd rc-q2l8u's follow-up. Do not
   change any code in this task.
5. Run the task gates.

**Tests:** (executable spec)
- `ratchet_verdict_zero_movement`: full-workspace run → command
  `cargo xtask lint-unbounded-wait` → expected exit 0 with count
  393 and the verdict line present in the ratchet file (verify by
  `grep -c "Closure-unroll verdict" scripts/xtask/ratchet-unbounded-wait.max`
  → `1`). If the run fails on count, expected outcome is a report
  to the conductor, not a file edit.

**Acceptance:**
- `cargo xtask lint-unbounded-wait` exits 0.
- Ratchet file contains the verdict line; the integer is 393
  (unless the conductor rules otherwise on inventory).
- Module doc documents the binding-indirection boundary.
- `cargo fmt --check`,
  `cargo clippy -p xtask --all-targets -- -D warnings`, and
  `cargo test -p xtask lint_unbounded` all exit 0.

- [x] 1.3
