# Proposal: bench-axum-bare

## Why

The era-2 record claims rust-lib http runs at 65% of the devnull ceiling, and the
rc-audm.4 profile attributes that gap to HTTP-stack machinery (alloc 28%,
http-crate 19%, hyper 9%, tokio 9%, axum/tower 5%) with camel itself at ~5%.
Today that decomposition exists only as a one-off perf profile — no published
record cell substantiates it, because nothing sits between the devnull raw-TCP
ceiling and rust-lib in the roster. bd rc-u034 (P3) asks for an intermediate
reference contender so stack-tax vs camel-tax becomes a first-class, recordable
measurement at the next canonical run.

This change also lands a small documentation correction owed to bd rc-audm.6:
`benchmarks/docs-investigation-strategy.md` §4 still carries the pre-burst
hypothesized park note (same-point re-measurement collapses the cli/lib gap),
which §8 refuted (bracket containment is anti-directional). The §4 note gets an
additive supersession pointer — the historical note is never rewritten.

## What Changes

- New fixture crate `benchmarks/contenders/axum-bare/` (package
  `axum-bare-fixture`): minimal axum 0.8 + tokio HTTP server, zero camel deps,
  same T3 route shape as rust-lib (`POST /bench` → drain body → `200 "pong"`,
  `BENCH_HTTP_REQUEST received` / `id=<n>` emissions), bare `BENCH_ROUTE_READY`
  marker after listener bind, `BENCH_AXUM_BARE_PORT` override for tests.
- Roster: `http-server/axum-bare` registered in run.sh (scenario-conditional
  `add_cell`, NOT in Pair A/B), summarize.py `expected_roster` gains the
  reference cell (53-cell roster: 5×8 + 2×6 + 1 reference), drift test
  extended, warm-24.py untouched (24/24), build-all.sh builds the fixture,
  http-server smoke gains the artifact + committed log.
- Specs: benchmark-suite (full-matrix 52→53, consolidated builds, new
  axum-bare requirement), benchmark-records (expected-roster prose).
- Docs: `harness/CONTEXT.md` roster-contract row; `records/SCHEMA.md`
  era-qualified cell-count prose; strategy doc §4 rc-audm.6 addendum.

Named out-of-zone exception (e_glm pre-flight), scoped exactly: in the root
`Cargo.toml` only the workspace `members` entry (and, if required, the
`default-members` exclusion) for the new fixture may change — no workspace
dependencies, profiles, package metadata, or other root files (zone lease
is otherwise benchmarks/** only). The `Cargo.lock` delta is limited to the
new `axum-bare-fixture` package entry; every third-party version stays
pinned (axum 0.8.9 already in the lock).

Excluded: any measurement of the new cell (next canonical run measures it);
the devnull worker-thread sweep (deferred, investigation-only); the
rc-audm.8 warmup trailing-window redesign (separate blessed-protocol change);
any record republication (canonical record 20260903T084658Z never re-run).

## Acceptance criteria

- `expected_roster(all 7 scenarios)` = 53 identities including
  `http-server/axum-bare`; drift test guards the reference tuple against
  run.sh source in both directions; all harness python tests green.
- `python3 benchmarks/harness/summarize.py --check benchmarks/records`
  byte-identical before/after (hash captured before the first roster edit).
- Fixture integration test passes (marker flushed, 200/pong, id=1, body
  drain); http-server smoke case passes with a committed `axum-bare.log`
  containing no timing numbers.
- fmt, clippy (-D warnings) on the new crate, xtask lints, rustdoc gate green.
- No product-crate changes; `Cargo.lock` gains ONLY the
  `axum-bare-fixture` package entry — no third-party version changes, no
  new third-party packages.

## Risk budget

Acceptable: roster-count churn in tests/docs. Out of bounds: touching
published records, changing measurement protocol or era, devnull ceiling
semantics, Pair A/B membership, and any warmup/warm-gate behavior.

Bd: rc-u034 (rides: rc-audm.6 docs addendum). Parent rc-audm stays open.
