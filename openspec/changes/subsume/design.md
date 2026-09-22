# Design: subsume

## Approach

Single-file change to `scripts/xtask/src/lint_unbounded_wait.rs`. The
detector already has correct enclosure machinery for direct waits:
`TimeoutCollector` records the span of each bounding timeout call's
future argument (arg index 1) into `future_regions`, and
`maybe_report` suppresses only when the wait's span lies inside a
region. The defect is localized to `WaitFinder::visit_expr_loop`: when
a loop contains awaits and is not itself region-bounded, it runs
`TimeoutSeeker` over the loop body and suppresses the loop finding if
ANY bounding call exists anywhere in the subtree — no enclosure test.

Replace that carve-out with per-await-site boundedness:

1. Collect await sites in the loop body with the same scope pruning
   `AwaitSeeker` uses today (closures, nested fn items, impl/trait fns
   pruned; async blocks kept). The seeker evolves from `found: bool`
   to recording each `ExprAwait` (or its span plus base expression).
2. A site is bounded when ANY of:
   - **Region enclosure**: site span inside a `future_regions` span
     (covers `timeout(d, async { .. })` wrapping and whole-loop
     wrapping — `loop_inside_timeout_not_reported` stays green).
   - **Await-on-timeout**: the base expression, after `strip_parens`,
     is a `Call` whose func path satisfies `is_bounding_path` (the
     per-iteration `match timeout(d, rx.recv()).await` shape).
   - **Binding provenance**: the base is a single-segment path whose
     ident is bound in the loop subtree by exactly one
     `let x = <bounding call>` and no other binding of that ident
     exists in the subtree (`let f = timeout(d, w); f.await`). A
     binding that is never awaited bounds nothing else (the dropped
     `let _ = timeout(..)` case falls out naturally: `_` can never be
     an await base, and no other site is enclosed).
3. If the loop has at least one await site and ANY site is unbounded,
  report the loop exactly as today (push `loop_spans`, marker check).
  If all sites are bounded or there are no awaits, do not report.
  Individual wait findings inside a reported loop stay subsumed;
  bounded single waits outside loops keep the existing
  `maybe_report` path untouched.

`TimeoutSeeker` is removed (its only caller is the loop carve-out).
Rule (c) adds a NEW single-binding provenance pass: a let-collector
(mirroring `SpawnCollector`'s shape, scope-pruned like the await
collector) records idents bound exactly once in the loop subtree by a
bounding `timeout(...)` call and not bound by anything else; an await
base path resolving to such an ident is bounded. Idents bound more
than once (rebinding, shadowing) bound nothing — the await reports.
`is_bounding_path`, the candidate/provenance resolver, markers, and
the ratchet read/compare logic are untouched. Aliased/glob-imported
timeouts resolve through `is_bounding_path` in both rule (2) and the
`TimeoutCollector` regions, so alias coverage is preserved.

Ratchet protocol: after the fix, run the lint over member trees. If
the count exceeds 395, inventory each new finding; false positives are
detector defects (fix before landing), true positives are either
bounded in-tree (small, mechanical) or the ratchet is raised WITH the
inventory recorded in the park report and a bd note (aliaswait
precedent). Exact 395 green is the expected outcome.

## Affected crates

- xtask (scripts/xtask): `lint_unbounded_wait.rs` detector + in-module
  tests. No other crate.

## Architecture boundaries

Test-tooling boundary only (assurance plane, per CONTEXT-MAP
assurance-tooling capability). No Runtime, DSL, Component, Service,
Language, or Function code is touched; no published API changes.

## Phases

Single-phase: one detector change plus tests plus a ratchet verdict.
No milestone grouping needed.
