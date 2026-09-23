# Proposal: closures

## Why

`lint-unbounded-wait` prunes every closure body unconditionally. A
closure that is directly invoked and awaited — `(|| async { rx.recv()
.await })().await` — executes inline in the test body, so its waits
park the test like any other unbounded site. Today the prune hides
them from the quality gate (bd rc-q2l8u repro: add that line to a
test, run `cargo xtask lint-unbounded-wait`, observe no finding).
This is a false-negative hole in the ADR-0069 §13.2 R1 detector, the
last retro item on the v0.52.0 backlog.

## What Changes

Included (extend the per-site machinery from 82d9a0c2; no redesign):

- A pre-pass marks closures whose bodies execute in the test fn:
  (a) the closure is the callee of a directly-awaited call
  (IIFE awaited, any closure kind), and (b) the closure is
  synchronous (not `async ||`, body not an async/try block) and
  directly invoked — its statements run at call time even without
  an await.
- `WaitFinder` and `LoopAwaitCollector` prune unmarked closures only;
  marked closure bodies join the enclosing-body analysis at their
  source spans. Existing region bounding, suppression, and loop
  subsumption apply unchanged because all of them are span-based.
- Awaited IIFE calls whose closure tail (through a final expression
  statement) is itself a wait-class call (`(|| rx.recv())().await`)
  are reported at the await site — the body has no inner await to
  unroll, the outer await drives the future inline.
- Closures passed as arguments, stored in bindings, or executed in
  spawned work stay pruned. Binding-indirection forms (`let f =
  iife(); f.await`, `let c = || {..}; c()`, curried calls) are a
  documented boundary with a follow-up bd.
- Ratchet 393: corpus scan finds zero IIFE-await shapes; movement
  must be inventoried before any change to the ceiling.
- Tests for every closure shape in the in-module lint suite.

Excluded: new wait classes, macro-interior waits (still opaque),
data-flow tracking of let-bound futures/closures.

## Acceptance criteria

- The bd repro reports a finding at the inner wait line.
- Direct call awaited → reported; direct call not awaited (async
  body) → not reported; closure passed as arg → not reported;
  nested direct call → reported once; sync direct call with a
  blocking wait → reported.
- Awaited IIFE inside a timeout future region stays bounded; a
  timeout inside an awaited IIFE bounds the IIFE's inner waits.
- Existing lint tests pass unchanged; ratchet verdict recorded.
- Gates: fmt, clippy `-p xtask --all-targets -D warnings`,
  `cargo test -p xtask`, build --workspace, all 16 xtask lints on
  the touched file.

## Risk budget

Detector precision only; no runtime code changes. Acceptable: zero
corpus movement (expected); any movement requires a full
false-positive/true-positive/reclassification inventory before the
ratchet moves. Out of bounds: new detector classes, await-data-flow
analysis, ratchet increases.

Bd: rc-q2l8u
