# Design: bench-era-2

## Approach

Three delivery phases implementing the e_opus orientation
(`docs/benchmarks/orientation-benchmark-restructure-2026-08-30.md`,
committed with this change). The harness is MOVED, never rewritten:
every golden digest and byte-reproducibility guarantee carries over
unchanged.

**Zone contract (P1).** `benchmarks/` becomes exactly: `README.md`
(owner-facing, generated/curated, five public words),
`bench` (single facade executable), `harness/` (run.sh + loadgen +
summarize generator + technical CONTEXT.md glossary),
`scenarios/` (8 fixture families, content untouched),
`runner/` (Dockerfile + pinning), `records/` (generated published
data — `index.json` + one dir per run; seeded in Phase 1 with an
empty index so the zone contract holds from day one), `attic/`
(spikes, diagnosis prose, era-1 historical results). Nothing else
at level 1. `harness/` already exists at its target shape
(`run.sh`, `loadgen/`); `builder/` folds into `harness/builder/`
(orientation Cond 4: build scripts are harness, not level-1
surface). Era-1 reports live in `docs/`, not `benchmarks/`, so
their disposition is P2's, not the zone contract's.

**Facade (P1).** `bench` is a thin bash dispatcher: `bench run`
→ passthrough to `harness/run.sh` (semantics byte-identical, same
env vars); `bench summarize` → `harness/summarize.py`; `bench
publish` → copies the run record into `records/` and refreshes
`records/index.json`. No orchestration logic lives in the facade.

**Run-level schema (P4).** `run.json` v1: `{ schema_version: 1,
run_id, date, era, git_commit, container_digest, host_provenance,
protocol {rounds, duration_secs, warmup_secs, order_seed},
cells: [{scenario, contender, variant, payload_class, metric,
round_values[], median, unit, input_sha256}],
ratios: [{numerator, denominator, metric, point, ci_lo, ci_hi,
method}] }`. The generator `harness/summarize.py` (python3 stdlib
only, deterministic: sorted keys, fixed float repr) reads per-cell
JSON emitted by run.sh, computes medians, emits `run.json` +
`summary.md` (tables + ratios; CI columns via the existing
`aggregate-ratios` logic invoked as a subprocess — one
implementation of bootstrap, no duplicate math). Golden rule
enforced structurally: summaries are generated artifacts; a
hand-edited summary fails a checksum task in CI (regenerate and
diff must be empty).

**Canonical v1 run (P3).** `runner/Dockerfile` builds the runner
image from the pinned nix/temurin toolchain; the image is recorded
BY DIGEST (resolve after build; `:latest` is forbidden in
records — today's `provenance.json` says `benchmark-runner:latest`,
which this corrects). The v1 subset = the validated families:
startup-minimal, http-server, t2-json, split-aggregate (full cell
sets) + payload axis on two reference contenders (rust-camel
lib, quarkus native) × 4 payload classes. Gauges ON (A/B verdict:
unresolved from zero). Quiet-host eligibility criteria (devnull
baseline stability bound, host-load ceiling) are written into the
`harness/CONTEXT.md` measurement-protocol section BEFORE the run —
they do not exist today and the run record must show conformance.
The run executes in-container, order randomized per
measurement_order.json; output → `records/v1-<date>/run.json` +
`summary.md`. rc-90ez (splitter silent no-op on `Body::Text`)
does NOT block: split-aggregate fixtures unmarshal to JSON arrays
before split, so the bug's code path is untouched (soft-dep
documented).

**Era-1 freeze (P2).** Per the orientation's disposition table:
era-1 reports (v2, v3, v4, addendum, consultations, e_opus
analysis) move `docs/benchmarks/*.md` → `docs/benchmarks/history/`
in Phase 3, immediately before the tag — the docs tree is NOT in
mdbook (`docs/src/SUMMARY.md` references no benchmark page), so
the move has no site-build impact. Before the addendum moves,
ADR-0066 gains the citation of the gauge A/B verdict (point
0.9890, CI [0.9785, 1.0126], interval includes 1.0) that
justifies gauges-ON — today ADR-0066 carries no such citation and
the addendum is its only traceable source (orientation R4).
`git tag bench/era-1-final` freezes the last commit where the
reports lived at `docs/benchmarks/`. Historical `results/` trees
(`benchmarks/results-published/`) move to `attic/results-era-1/`
in Phase 1 — frozen under the same tag, never mixed with the new
era. `COVERAGE.md` (itself moving under `scenarios/`) has its
report links rewritten to `history/` (orientation R5;
`lint-context-citations` stays green). Nothing is deleted; git
history + tag are the bleach-proof record.

**Terminology (P5).** README uses only: corrida, escenario,
contendiente, fecha, registro. M1-M4, T-families, cells, pairing,
seeds live ONLY in `harness/CONTEXT.md` (canonical technical
glossary, moved from `benchmarks/CONTEXT.md`). COVERAGE.md moves
under `scenarios/` as the coverage index (renamed columns keep
technical terms — it is a technical artifact).

## Affected crates

- None in `crates/`. No workspace member path changes: `loadgen`
  already lives at `benchmarks/harness/loadgen` (root `Cargo.toml`
  already points there); scenario fixture crates stay at
  `benchmarks/scenarios/<name>/`. `bench-loadgen` code is
  untouched; tests keep passing byte-identical.
- CI: `bench-smoke` keeps its criterion quick-mode subset
  (`camel-bench` pipeline + body_coercion) and adds a
  `bench run --dry-run` facade smoke (seconds, no JDK); the
  job's comment pointing at `run-all.sh` updates to the facade.
  Phase 2 adds the summary-checksum job.

## Architecture boundaries

Benchmarks workspace only; no Runtime/DSL/Component surface is
touched. The data plane boundary that matters: `records/` is
output, never input to measurement; `harness/` reads nothing from
`records/`. Generated docs (`summary.md`) derive solely from
`run.json`.

## Phases

- **Phase 1 — Zones (P1+P5)**: moves inside `benchmarks/` only
  (spikes → `attic/spikes/`, root prose + `results-published/` →
  `attic/`, `builder/` → `harness/builder/`, COVERAGE.md →
  `scenarios/`), README rewrite, glossary confinement, facade
  with `run` passthrough, `records/` seeded with empty
  `index.json`, CI bench-smoke facade-dry-run + comment updates,
  goldens green after move. Exit: the `benchmarks/` level-1
  listing matches the zone contract; all existing tests +
  bench-smoke pass.
- **Phase 2 — Records layer (P4 schema)**: summarize.py, run.json
  schema + JSON-schema doc, summary.md generator, checksum guard
  CI job, synthetic-fixture unit tests (golden record from
  crafted cells, including one ratio with known CI bounds; two
  invocations byte-identical). Exit: synthetic golden-record and
  checksum-guard tests green; regenerating a sample record's
  summary yields an empty diff.
- **Phase 3 — Canonical run + freeze (P3+P2)**: quiet-host
  criteria written into `harness/CONTEXT.md`; runner/Dockerfile
  digest-pinned; v1 subset execution; `records/v1` published;
  docs-site wiring (static JSON fetch); era-1 reports →
  `docs/benchmarks/history/`; ADR-0066 gauge citation; COVERAGE
  link rewrite; tag `bench/era-1-final`. Exit: v1 `run.json`
  validates against the schema with `container_digest` a
  sha256 digest; `records/index.json` fetchable as static JSON;
  ADR-0066 cites the 0.9890 verdict; no COVERAGE link dangles;
  the tag exists on the last `docs/benchmarks/`-era commit.
