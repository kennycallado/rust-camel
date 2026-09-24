# Proposal: lintwiden

## Why

`lint-unbounded-wait` walks only `#[test]` / `#[tokio::test]` function
bodies. Plain helper fns in files under `tests/` directories — and
non-test fns inside `#[cfg(test)]` modules — are invisible to the
ratchet, so waits that park a test binary forever escape enforcement
if they sit one fn boundary away from the test (drainscope holistic
finding, commit 378dbe6d; bd rc-h2qwr, discovered-from rc-j27pc).
The lexical seed (drainscope design.md Appendix A) counted 30
wait-class candidates in non-test fn bodies under `tests/`, 27 with
no timeout enclosure, over 18 files.

## What Changes

- Widen `scripts/xtask` `lint-unbounded-wait` AST walk: scan non-test
  fn bodies when the file is under a `tests/` directory (any path
  component named `tests`) OR the fn is inside a `#[cfg(test)]`
  module. Test-fn findings unchanged (shared scan machinery).
- Spawned-closure bodies inside helper fns stay pruned, mirroring the
  test-body rule (binding-indirection kin tracked in bd rc-eow0s).
- AST-derived full inventory of newly-visible helper-fn sites
  (widened scanner is the source of truth; Appendix A is only the
  lexical seed). Per-site adjudication: bounded in-tree
  (`acquire_deadline` for global test locks, per-iteration deadline
  D-recipe for drains, `tokio::time::timeout` wrap for connects),
  `allow-test-wait` marker with site-specific justification, or a
  ratchet-ceiling entry — no silent sites.
- Ratchet ceiling stays monotone: target is zero new unadjudicated
  findings at the current ceiling 296. Any ceiling increase requires
  review justification recorded once in the park notes (mission
  allowance), not taken silently.
- Spec delta: `unbounded-wait-bounding` gains a requirement covering
  helper-fn scan scope, so spec scope and ratchet scope move together.
- Lint scope unit tests: helper fn visible under `tests/`, invisible
  in plain src scope, visible in `#[cfg(test)]` modules, test-fn
  results unchanged, spawn-closure pruning preserved.

## Impact

- `scripts/xtask/src/lint_unbounded_wait.rs` (scanner + unit tests).
- Test files under `tests/` dirs in camel-test, camel-integration-test,
  camel-dsl, camel-cxf (conversions/markers per inventory), plus
  whatever the AST inventory adds beyond the seed (residual sweep).
- `openspec/specs/unbounded-wait-bounding/spec.md` (delta in this
  change; enforcement and spec land together).
- Affected specs: `unbounded-wait-bounding`.
