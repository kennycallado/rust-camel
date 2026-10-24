# Classification Rubric — lint-unbounded-wait burn (mission 394, bd rc-yfsw6)

Baseline: 299 findings on main 3dfada1a7 (see `classify/` TSVs; raw list `classify/sites-299.txt` (committed)).
Lint: `scripts/xtask/src/lint_unbounded_wait.rs`. Marker syntax (from lint source, verify on your sites):
`// allow-test-wait: <non-whitespace reason>` on any source line spanned by the finding expression.
Pass rule: findings <= `scripts/xtask/ratchet-unbounded-wait.max` (currently 296).

## Buckets

**C — production.** Flagged site is NOT inside a `#[test]`/`#[tokio::test]` function, `#[cfg(test)]`
module, or a `tests/` integration file. These are bugs: do NOT edit, do NOT mask. Record
`C` + one line on why it is production. Expected count near zero (lint scans test scope only).

**B — bounded-by-construction (marker).** The wait cannot hang the test because of how it is built:
- channel/broadcast/watch created and driven by the same test (value deterministically available);
- await of an owned task whose EVERY progress dependency is bounded: each inner I/O / recv /
  connect / read already carries its own enforced deadline, or the awaited work is pure
  in-process computation with no I/O dependency (in-memory tower service, deterministic mock
  answering from memory);
- join where completion was already observed before the join (drain/arrival already awaited);
- wait already bounded by an in-repo bounded helper (`wait_until` in crates/camel-test/tests/support/wait.rs,
  `acquire_deadline` in camel-component-api test_support, interval poll loop with its own deadline);
- blocking call already wrapped in an enforced bound (`recv_timeout(30s)`).

TERMINATION PROOF REQUIRED: "the test controls it" and "completion is the assertion" are NOT
proofs on their own — an assertion after an await cannot bound the await. A marker reason must
cover every progress dependency of the awaited work (each blocking op bounded, or purely
in-memory). If any inner operation can stall (network read, handshake, lock you do not hold,
recv on a channel someone else must feed), classify A. Paused tokio time (`start_paused`)
does NOT make a pending receive bounded — a frozen clock never advances a deadline — so
paused-time tests with real recv/join dependencies classify on the rules above, not by the
pause itself.
Marker reason must cite the construction. "Usually fast" / "CI is fast" = NOT valid — that is bucket A.
EXCEPTION (spec unbounded-wait-bounding): the six named global serialization-lock families use
`acquire_deadline` — markers are forbidden there; if you find one flagged, record bucket A with action
`acquire-deadline`.

**A — genuinely unbounded (fix).** The wait can hang if code under test misbehaves: recv on a
component-owned channel, connect, lock acquire outside the deadline families, join of tasks that may
never finish. Fix = wrap in `tokio::time::timeout(..)` (the lint recognizes it) or the shared helper
once decided. Preserve semantics:
- distinguish timeout from channel closure (spec openspec/specs/unbounded-wait-bounding/spec.md);
- keep failures loud: panic/expect with context, never silently skip;
- do not swallow background-task failures (dropping a JoinHandle detaches — abort/join explicitly when needed);
- spec openspec/specs/test-determinism/spec.md: readiness/completion waits fail loudly; silent timeout
  exits only for genuine quiescence drains.
Duration guidance: unit-tier <= 30s, integration-tier <= 60s. One timeout per wait site; no loops.

## Shared-helper rule

Do NOT invent a second helper. Reuse existing `wait_until` (camel-test/tests/support/wait.rs) where the
cluster already uses it. Cross-crate helper placement + lint-recognition decision is made by the design
worker (group A) — if your cluster needs a wrapper the repo does not have, classify the site bucket A
with action `helper-pending` and note the pattern; do not invent your own.

## TSV format (one row per site, tab-separated)

`file:line<TAB>bucket<A|B|C><TAB>action<timeout|marker|helper-pending|acquire-deadline|file-bd|none><TAB>reason <= 12 words>`

Rows sorted by file then line. Counts per bucket go in your report (one line: `A=n B=n C=n`).
