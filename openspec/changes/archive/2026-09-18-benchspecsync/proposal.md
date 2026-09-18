# Proposal: benchspecsync

## Why

The `benchmark-suite` spec was last canonically synced before two later
landings shifted the suite: (1) the rc-h42s6 fixture alignment
(2026-09-16, commit 372c7018) edited two clauses inline — the axum-bare
T3 keep-alive no-trace clause and the smoke WARN-only `id=1` clause —
without a full pass, and (2) the owner untracked `docs/benchmarks/`
period artifacts (ef337c0f, bd rc-mq1sh, 2026-09-17). A full
delta-style review of every requirement/scenario against the current
suite (benchmarks/ + crates/camel-bench) found 8 requirements where
spec text no longer matches what the suite does today. bd rc-ctdq3
(P3, docs/spec zone).

## What Changes

- **MODIFIED requirements** in `openspec/specs/benchmark-suite/spec.md`
  (delta under this change):
  1. Zone contract — codify the tracked `audits/` zone and the
     test-pinned `docs-investigation-strategy.md` level-1 doc.
  2. Era-1 freeze — reports untracked per owner ruling ef337c0f
     (bd rc-mq1sh); evidence now tag-reachable (`bench/era-1-final`)
     with durable numbers in ADR-0066 / RUNBOOK / COVERAGE.
  3. Canonical full-matrix run — record the node-family m3/m4
     measured-set exclusion (rc-h42s6 phase-2, e_opus ruling D5;
     mirrored run.sh ↔ summarize.py, drift-guarded).
  4. Warm tick mode — marker latches inside the FIRST completed tick
     (not "loop starts after marker"); rust-camel-cli plumbing is
     direct argv env, not a wrapper.
  5. Payload-size axis — `BENCH_PAYLOAD_BYTES` scoped to the
     payload-carrying Protocol-B family (t2-json); split-aggregate and
     t2-realistic-eip are outside the payload axis.
  6. Node contender family — dry-run wording to the family-level
     single `package.json` / one `<would-build:npm-ci>` marker.
  7. axum-bare reference contender — smoke scenario retitled: committed
     evidence was deleted (585858c6); regeneration is owner-run-only.
  8. Ratio confidence intervals — CI method is percentile bootstrap on
     the shared SplitMix64 PRNG; BCa is deliberately not applied to a
     ratio of medians.
- **COVERAGE.md fix**: references to untracked `docs/benchmarks/`
  reports name the tag retrieval (no link presented as live in a
  fresh clone).
- **Adjudicated KEEP (no change)**: both inline edits from the fixture
  alignment verify accurate against code (zero `BENCH_HTTP_REQUEST`
  in the axum-bare crate, shape-test-pinned; WARN-only `id=1` smoke
  checks citing e_opus D2) — only the smoke scenario TITLE needed
  rewording. README lowercase `m1`–`m4` are record-schema metric
  identifiers, not confined vocabulary.
- **Explicitly excluded**: any bench/canonical-run execution
  (owner-exclusive), harness code changes, and record mutation. The
  meta.json `scenarios` active-only contract was re-verified as
  matching code (launch-time discovery list is rewritten to the
  active roster by the run.sh meta-hygiene step before measurement).

## Acceptance criteria

- Every requirement/scenario of `benchmark-suite` carries a
  per-clause verdict in `design.md` (MATCH / MODIFIED / HISTORICAL)
  with file:line evidence from the current suite.
- The two known inline edits are explicitly adjudicated with code
  evidence (both KEEP).
- Post-archive spec text matches the suite as verified against code
  and committed records; no scenario asserts an unmet state.

## Risk budget

Docs/spec-zone only: no runtime code, no measurement, no record
mutation. Worst case is a wording regression in spec canon — caught by
e_gpt blessing, r_glm review, and delta validation. Out of bounds:
running any bench suite member, touching harness/runner code,
re-publishing records.

## Bd

- rc-ctdq3 (this change). Discovered-from chain: rc-h42s6.
