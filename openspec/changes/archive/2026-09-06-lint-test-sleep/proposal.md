# Proposal: lint-test-sleep

## Why

The flaky-tests epic (bd rc-99d5) adjudicated `sleep-as-sync` as an eliminable
flake class: a fixed sleep in a test body assumes state after N ms, and CI load
breaks that assumption (observed: 2/11 flake on the aggregator timeout test,
fixed as rc-95ic). The known inventory (~280 `sleep` matches across test code)
is misleading: most sleeps are legitimate simulate-work inside processor/route
closures. Nobody knows the true count of sleeps that act as synchronization in
test function bodies.

Per the e_opus escalation ruling (2026-09-06), the lint runs advisory-first to
measure the real debt BEFORE any bulk conversion to `wait_until` barriers.
Bd: rc-99d5.2 (child of rc-99d5).

## What Changes

- New advisory xtask lint `lint-test-sleep`: scans `#[test]` / `#[tokio::test]`
  function bodies and reports direct `tokio::time::sleep` / `std::thread::sleep`
  calls (fully-qualified, `use`-imported short forms, and aliased imports),
  using a syn AST visitor.
- Sleeps inside closures (`syn::Expr::Closure`) nested in the test function are
  excluded (simulate-work is legitimate there); only lexical scanning cannot
  make this distinction, hence syn.
- Escape hatch: `// allow-test-sleep: <reason>` (non-empty reason required) on
  the sleep line, mirroring lint-unwrap.
- Advisory rollout: findings are printed with a summary count; the command
  exits 0 when scanning completes. It is NOT added to the CI quality gates yet.

Explicitly excluded: hard-fail mode, diff-gating (lint-commits-style), bulk
sleep-to-barrier conversion, `sleep_until`/other timer APIs, non-Tokio test
frameworks (rstest etc.), and the R1 timeout-dominance check itself (bd
rc-3lx2 — the visitor infra built here is structured so rc-3lx2 can extend it,
per the adjudication "fold into R1, do NOT create a separate R8 rule").

## Acceptance criteria

- `cargo xtask lint-test-sleep` prints every direct-in-body test sleep as
  `file:line` with a summary count and exits 0 when scanning completes.
- Sleeps in processor/route-builder closures in the same test fns are NOT
  reported.
- `// allow-test-sleep: <reason>` (non-empty reason) suppresses a finding.
- Unit tests cover: `std::thread::sleep` in `#[test]`, `tokio::time::sleep` in
  `#[tokio::test(flavor = "multi_thread")]`, closure exclusion, non-test
  functions ignored, short-form resolution via `use` (including aliased
  imports), escape hatch (empty marker does not suppress), `sleep_until` not
  flagged.
- Existing gates stay green (fmt, clippy on xtask, other xtask lints).

## Risk budget

Tooling-only change in `scripts/xtask`; no runtime crate is touched, so runtime
risk is zero. Main risk is false positives (noisy report eroding trust) —
mitigated by the conservative closure-exclusion rule and the escape hatch.
False negatives (including synchronization sleeps hidden inside closures) are
accepted at this stage: this is an advisory measurement, and the later
hard-fail phase tightens precision.
