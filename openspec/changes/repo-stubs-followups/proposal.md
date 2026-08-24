# repo-stubs-followups

Four post-merge follow-ups from the declarative-repository-stubs holistic
review (bd rc-5fgq, rc-c9ci, rc-k81v, rc-lq1v) — one cleanup pass over the
same camel-cli test modules.

## Why

Trivial change — no spec breakdown needed. Cosmetic/ergonomic debt with zero
correctness risk, filed by the holistic reviewer and blessed by e_opus as
deferrable.

## What changes

- rc-5fgq: deduplicate registry labels ("cache"/"idempotent"/"claimCheck")
  across document.rs / runner.rs / test.rs — one source of truth
  (stub-pairs iterator or pub(crate) constant), warning collapses to one loop.
- rc-c9ci: extract `assert_green(result, n)` helper in
  tests/test_repository_stubs.rs (green-tail assert block copied 4x).
- rc-k81v: negative test — `repositories: { cache: {} }` emits no
  R-REPOSITORY-STUB warning.
- rc-lq1v: pluralize "unknown registry kind" error when multiple unknown
  kinds are present.

Bd: rc-5fgq rc-c9ci rc-k81v rc-lq1v
