# Design: closures

## Approach

Execution semantics decide scope: analyze a closure body as part of
the test fn when the body executes in the test fn. Three extensions
to the existing pass structure in `scripts/xtask/src/
lint_unbounded_wait.rs` (per-site machinery from 82d9a0c2):

1. **Pre-pass `InlineClosureCollector`** (new pass 0 in
   `scan_test_fn`, plain `Visit` walk, no pruning). It collects the
   start/end `LineColumn` span keys of closures that execute inline:
   - *Awaited IIFE:* every `Expr::Await` whose base (after
     `strip_parens`) is an `Expr::Call` whose func (after
     `strip_parens`) is an `Expr::Closure` — mark that closure.
   - *Sync direct call:* every `Expr::Call` whose func strips to a
     closure with `asyncness: None` and a body that is not (after
     stripping) an `Expr::Async` / `Expr::TryBlock` — mark that
     closure regardless of await. An async-block body only builds a
     future at call time; without an await nothing inside runs.
   Marks are collected over the whole body including closure
   interiors, but a mark only lifts pruning at the closure node
   itself; reachability still flows from the test body, so an IIFE
   inside a closure passed to `tokio::spawn` stays invisible.

2. **Prune-lift in the finders.** `WaitFinder::visit_expr_closure`
   and `LoopAwaitCollector::visit_expr_closure` keep a reference to
   the mark set: marked → `visit::visit_expr_closure` (unroll);
   unmarked → prune (today's behavior). All bounding machinery is
   span-based, so region enclosure, marker suppression, and loop
   subsumption apply to unrolled bodies with no extra code. The
   `TimeoutCollector` already descends closures; a closure-internal
   region can never contain an outer span, so it still bounds
   nothing outside its closure.

3. **Awaited-IIFE site classification.** A shared helper classifies
   an awaited IIFE call by its closure tail (the stripped body, or
   the final expression statement of a block body):
   - async/try-block body → `AsyncBody`: the await itself is not a
     wait site; inner awaits are reported through the unroll.
   - tail is a wait-class call (method in `WAIT_METHODS` or
     `spawn`; free-fn path resolving to the wait/spawn targets) →
     `WaitTail`: the outer await drives that wait inline → report
     the await span (bounded/suppressed rules apply —
     `timeout(d, (|| rx.recv())()).await` stays bounded).
   - otherwise → `Other`: not a wait site (finite or opaque-tail
     closures like `(|| tokio::time::sleep(d))().await`).
   `WaitFinder::visit_expr_await` extends its `Expr::Call` arm with
   this helper; `LoopAwaitCollector::visit_expr_await` maps
   `WaitTail` → `SiteBase::Other` (candidate site) and
   `AsyncBody`/`Other` → no site, so the loop rule and the
   standalone rule agree on every IIFE shape.

**Boundary (documented, follow-up bd):** binding-indirection forms —
`let f = (|| async {..})(); f.await`, `let c = || {..}; c()`,
curried `((outer())())()` calls, and futures driven by macro bodies.
These need ident-to-call data flow; they stay pruned (conservative
false negatives, no false positives).

## Affected crates

- xtask (scripts/xtask): `lint_unbounded_wait.rs` — pre-pass,
  prune-lift, site classification, module docs, tests.
  `ratchet-unbounded-wait.max` — verdict line only if the corpus
  count moves (expected: it does not; no IIFE shapes exist).

## Architecture boundaries

Quality-gate tooling only; no Runtime/DSL/Component code. The lint
stays structural (`syn`, no type info), per ADR-0069 §13.2 R1
("a narrow AST lint … does not claim complete proof"); R6 job-level
timeouts remain the backstop for the residual boundary.

## Phases

Single-phase: one detector extension plus tests plus a ratchet
verdict. No milestone grouping needed.

## Alternatives considered

- **Unroll every closure body:** rejected — closures passed to
  `tokio::spawn` or route builders run in other tasks; reporting
  their waits would flood the corpus with false positives.
- **Ident data-flow for let-bound futures (spawned-name pattern):**
  deferred to the follow-up bd — it doubles the change surface for
  a corpus-zero shape; the direct-call rule covers the reported
  repro exactly.
- **Report every opaque awaited IIFE as a wait:** rejected — finite
  closures (`sleep` tails) would be false positives; the tail
  classification keeps precision.

### Self-grill record

**Questions generated:**
1. [glossary] Do the new terms — "awaited IIFE", "sync direct call",
   "wait-class tail" — collide with existing detector vocabulary
   (`WAIT_METHODS`, `SiteBase`, `is_wait_path`, `is_finite_call_path`)
   or introduce a synonym for a concept already named?
2. [sharpen] The mark predicate says "awaited IIFE = `Expr::Await`
   whose base strips to `Expr::Call` whose func strips to
   `Expr::Closure`". Is "directly awaited" precise enough to exclude
   the argument-position IIFE (`timeout(d, (||..)()).await`)?
3. [scenario] Construct the timeout-wrapping and spawn-nesting inputs
   and confirm each spec scenario's stated outcome AND stated reason
   match what the design's predicate actually produces.
4. [cross-ref] Does `WaitFinder::visit_expr_await` today have an
   `Expr::Call` arm that can host the new tail helper, and does
   `LoopAwaitCollector` expose a `SiteBase::Other` candidate path the
   design can map `WaitTail` onto — i.e. is this an extension of
   82d9a0c2, not a redesign?

**Answers (with citations):**
1. [glossary] No collision. `WAIT_METHODS`/`BLOCKING_METHODS`/
   `FINITE_CALL_TARGETS` and `SiteBase` are reused verbatim by the
   design; the new terms name a genuinely new shape (a closure node
   lifted from the prune set), not a rename
   (`lint_unbounded_wait.rs:279,284,287,1440`). The design's
   `WaitTail`/`AsyncBody`/`Other` classification is a new local helper
   enum, disjoint from `SiteBase`, and it explicitly maps onto the
   existing `SiteBase::Other` for the loop path (`design.md:49-52`).
2. [sharpen] "Directly awaited" is under-specified in prose but the
   predicate is exact in code terms: the mark fires only when
   `Await.base` (after `strip_parens`) *is* the `Call(closure)`. An
   argument-position IIFE inside `timeout(d, (||..)())` is never the
   await base (the await base is the `timeout` call), so it is never
   marked. The spec's GIVEN for "Timeout wrapping an awaited inline
   closure bounds it" uses exactly that argument-position shape, so
   the closure stays pruned — the outcome (no finding) is right but
   the stated reason (unrolled site inside the region) is false
   (`design.md:13-15`, delta `spec.md:61-68`).
3. [scenario] Traced all 12 scenarios against the predicate. Eleven
   match outcome AND reason. One mismatch: "Timeout wrapping an
   awaited inline closure bounds it" — outcome correct, reason wrong
   (no unroll happens; the IIFE is pruned as an un-awaited argument).
   The spawn-nesting scenario is sound because the mark set is
   collected body-wide but reachability is still gated by the
   pruning walk (`design.md:21-24`; confirmed against
   `WaitFinder::visit_expr_closure` prune at
   `lint_unbounded_wait.rs:1654`).
4. [cross-ref] Confirmed extension, not redesign.
   `WaitFinder::visit_expr_await` has an `Expr::Call` arm that today
   only matches `c.func == Expr::Path` and falls through on a closure
   func (`lint_unbounded_wait.rs:1717-1745`); adding a closure-func
   sub-case is a local extension. `LoopAwaitCollector::visit_expr_await`
   already produces `SiteBase::Other` for opaque call bases
   (`lint_unbounded_wait.rs:1523-1552`), so mapping `WaitTail →
   SiteBase::Other` reuses the existing candidate-site path. The
   pre-pass is a plain `Visit` walk added as pass 0 before the
   existing three passes in `scan_test_fn` (`lint_unbounded_wait.rs:1275`).

**Outcome:** refine — direction is sound and faithfully extends the
82d9a0c2 machinery; two artifact defects must be corrected before
planning (delta scenario reason mismatch; non-verbatim MODIFIED
scenarios). See BLESS-WITH-FIXES fix list.
**Self-grill mode:** self-grill-proposals skill

## Plan-review addendum (r_glm, 2026-09-23)

Intentional deviation from verbatim canonical text, ratified for
plan-bless: the MODIFIED "Ratchet verdict recorded on count
movement" scenario says "current ceiling (393)" where the canonical
spec still says (395) — stale from before the subsume lowering
(395→393, commit 82d9a0c2). A MODIFIED requirement is the
legitimate place to correct it; leaving 395 would archive the wrong
ceiling into the normative spec. Both r_glm findings on RED-set
inventories (tasks 1.1 and 1.2) are applied to tasks.md.
