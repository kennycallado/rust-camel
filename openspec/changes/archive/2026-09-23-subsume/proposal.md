# Proposal: subsume

## Why

bd rc-ohddw (P2, retro520 finding): `lint-unbounded-wait` suppresses a
loop finding when ANY `timeout`/`timeout_at` call site exists anywhere
in the loop subtree — including timeouts that never enclose the loop's
own awaits. A dropped sibling timeout (`let _ = timeout(d, fut);`), a
timeout wrapping a different branch, or a timeout inside a spawned
closure all hide a genuinely unbounded `recv().await` in the same loop.
The retro520 blind-spot checklist calls this out: "one timeout
anywhere != all waits bounded (loop subsumption gaps)".

## What Changes

- `scripts/xtask/src/lint_unbounded_wait.rs` only: replace the
  loop-level "any timeout in subtree" carve-out (`TimeoutSeeker` in
  `visit_expr_loop`) with per-await-site structural boundedness.
- An await site inside a loop is bounded only by a timeout that WRAPS
  that site: (a) the site lies in a timeout future region (lexical
  enclosure, existing `future_regions` spans), (b) the await's base
  expression is itself a bounding timeout call (per-iteration
  `timeout(d, w).await`), or (c) the await's base is a local bound in
  the loop subtree by `let x = <bounding timeout call>` and that local
  is awaited itself (provenance enclosure).
- Unrelated timeouts (siblings, disjoint branches, closure-internal)
  no longer suppress the loop finding. Loop analysis counts only
  wait-class await sites: finite-call targets (`sleep`,
  `yield_now`) and out-of-class method awaits (`send`, `notified`,
  `on_next`, `accept`, stream I/O) are not sites, consistent with the
  V1 wait classification. Subsumption, marker, and
  candidate/provenance machinery is unchanged.
- Red/green in-module tests pin the three hiding shapes and the two
  legitimate per-iteration shapes (direct await-on-timeout, local
  binding awaited).

Affected crates: none (xtask lint only). No runtime code changes.

## Acceptance criteria

- The three hiding patterns report the loop: dropped sibling timeout,
  timeout in a disjoint branch, inner timeout not enclosing the loop
  body waits.
- `loop { match timeout(d, rx.recv()).await .. }` and the awaited
  local-binding shape stay unreported.
- All 78 existing in-module tests pass unmodified.
- Ratchet stays exact-green at 395, or is raised only with an
  inventoried list of new TRUE positives (aliaswait precedent).
- Gates: fmt, clippy -p xtask --all-targets -D warnings,
  cargo test -p xtask, workspace build.

## Risk budget

Tightening detection may surface new findings in member trees. False
positives are defects to fix before landing; true positives are either
fixed in-tree (small) or ratchet-raised with inventory. No changes to
runtime crates; blast radius is the lint and its tests. Out of bounds:
redesigning the candidate/provenance resolver, widening WAIT_METHODS,
any runtime behavior change.
