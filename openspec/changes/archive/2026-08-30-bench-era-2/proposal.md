# Proposal: bench-era-2

## Why

The project is pre-1.0: the last cheap moment to reset benchmark
conventions. Today `benchmarks/` causes the owner "gray hairs":
five loose reports of unequal weight at `docs/benchmarks/`, visible
spike directories, committed curated `results/` trees, and no
run-level aggregation — the owner's mental model ("run everything
at a date, get dated tables") was never delivered. Meanwhile the
suite's only comparative evidence (v4 ratios with CIs, gauge A/B
0.9890 [0.9785, 1.0126] unresolved-from-zero) must survive any
restructure, and the canonical container protocol is still
unreproducible in one point: the runner image is `:latest`.

Papal orientation (e_opus, 2026-08-30,
`docs/benchmarks/orientation-benchmark-restructure-2026-08-30.md`)
decomposed the owner's request into five pillars and set the
verdicts this change implements: P1 restructure with conditions,
P2 tag-and-freeze (NOT deletion), P3 great initial run v1 baseline,
P4 run-level JSON records, P5 terminology diet.

## What Changes

- **P1 — 3-zone restructure**: `benchmarks/` becomes
  `harness/` (live code, moves only) + `scenarios/` (fixtures) +
  `records/` (generated, published data) + `runner/` (Dockerfile
  pinned by digest) + `attic/` (spikes, dead prose, historical
  reports moved out of level-1 view). Single `bench` facade over
  the existing `run.sh` (subcommands: run / summarize / publish).
- **P4 — run-level JSON**: define `run.json` schema
  (`schema_version`, run_id, date, era, git_commit,
  container_digest, provenance, cells) and `records/index.json`
  (array of run records = the owner's dated tables). Served
  web-fetchable via the existing mdbook/gh-pages infrastructure.
  Per-run summary generated from JSON — golden rule: **no human
  ever types a number into a report**.
- **P3 — the great initial run**: one canonical container-hosted
  run over the validated subset (startup-minimal, http-server,
  t2-json, split-aggregate + payload axis on two reference
  contenders), image pinned by digest, commit tagged — becomes
  `records/` v1 baseline ("v1", not "final": the matrix grows).
- **P2 — tag-and-freeze era-1**: era-1 reports (v2/v3/v4/addendum,
  consultations) move to `docs/benchmarks/history/` and
  historical results trees to `benchmarks/attic/results-era-1/`;
  `git tag bench/era-1-final` freezes the last commit where the
  reports lived at `docs/benchmarks/`. Before the addendum moves,
  ADR-0066 gains the citation of the gauge A/B verdict (0.9890,
  CI includes 1.0) that justifies gauges-ON.
- **P5 — terminology diet**: public vocabulary of five words
  (corrida, escenario, contendiente, fecha, registro); technical
  vocabulary (M1-M4, T-families, pairing) confined to
  `harness/CONTEXT.md`.

**Excluded**: rewriting harness logic (moved, not modified —
byte-reproducible goldens must keep passing); changing the
measurement protocol (SF-3, randomized order, canonical digests
intact); serving records over MCP (open-if, post-v1); building a
website project (static JSON + generated summary only); scenario
fixture content changes.

## Acceptance criteria

- Opening `benchmarks/` shows README + facade + zones; no loose
  spikes or historical reports at level 1.
- `bench run|summarize|publish` exist as thin wrappers; `run.sh`
  semantics unchanged.
- `run.json` schema documented with `schema_version: 1`;
  `records/index.json` present and fetchable as static file.
- A v1 baseline run record exists, produced from a digest-pinned
  container, with gauges ON and zero hand-typed numbers.
- Tag `bench/era-1-final` exists; old reports reachable under it
  and at `docs/benchmarks/history/`; historical results under
  `benchmarks/attic/results-era-1/`; ADR-0066 cites the gauge
  A/B verdict.
- All existing golden-digest and harness tests keep passing
  unmodified.

## Risk budget

- Acceptable: file moves breaking contributor muscle memory;
  README rewrites; new facade bugs (thin wrapper, low blast
  radius).
- Out of bounds: any change to measurement semantics, goldens, or
  published v4 numbers; deleting history before the tag exists;
  harness logic rewrites.

Affected crates: none — no `crates/` changes and no workspace
member path changes (`bench-loadgen` already lives at
`benchmarks/harness/loadgen`). Bd: rc-4mzj (epic, this change),
children rc-f4po (P3 run), soft-dep rc-90ez if split-aggregate
enters the v1 subset.
