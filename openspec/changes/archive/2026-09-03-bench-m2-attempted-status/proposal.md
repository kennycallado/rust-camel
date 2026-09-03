# Proposal: bench-m2-attempted-status

## Why

The m2 publish gate is fail-closed on warm-applicable cells: a cell counts as
present only when `m2-summary.json` exists. But two real failure modes leave
EXPLICIT attempt evidence on disk while producing no summary:

1. **Unconverged warmup** (`http-server/{camel-standalone-dsl,
   camel-standalone-yaml,node-native}`): protocol-A warmup writes
   `protocol-a-summary.txt` with `status=failed reason=measure-a-error` and
   `MessageBoundUnconverged` — the engine was exercised 0/5 rounds converged;
   this is steady-state physics (JIT/node), not a harness defect.
2. **Probe timeout** (bridge cells at the time of the source run):
   `exit-codes.txt` records `# probe reason: no BENCH_LATENCY within 30s
   timeout` plus `bridge-stderr.log`.

The canonical run `out/20260903T084658Z` (era-2, m1+m2+m3+m4) is otherwise
complete: 106 cells summarized, 38/44 m2 measured, the 6 gaps split exactly
across these two evidence-backed modes. The gate cannot distinguish "cell
never ran" (true gap — must block) from "cell ran and failed measurably"
(attempted — recordable with status). `insufficient-samples` already proves
the precedent: status-carrying cells are publishable members of a record.

## What Changes

- `summarize.py`: when a warm-applicable m2 cell lacks `m2-summary.json` but
  has attempt evidence, emit the cell with status `unconverged` (sentinel
  shows `MessageBoundUnconverged`) or `attempted-timeout` (probe-timeout
  reason), no latency numbers, attempt metadata.
- Publish gate: present = measured OR status-attempted. A cell with NO
  evidence still blocks (fail-closed preserved).
- `schema_version` bumped; record cells may carry `status`.
- Tests: fixtures replicating both evidence shapes from the source run.

**Out of scope**: m1/m3/m4 semantics, warmup budget changes (time-based
protocol A is bd p3 backlog), run.sh emitting status natively at measurement
time (follow-up; derive-from-artifacts is the durable contract), any engine
or fixture fix (already fixed in `af15657e`/`177cea9b`).

## Acceptance criteria

- Re-summarize of `out/20260903T084658Z` yields 44/44 m2 cells present
  (38 measured + 3 `unconverged` + 3 `attempted-timeout`).
- `publish` accepts the record; `records/` index updated; schema_version new.
- A cell dir with neither summary nor evidence still fails the gate (test).
- Old records (no status) still parse and validate (back-compat test).

## Risk budget

- **Acceptable**: additive schema change (bump version); gate semantics
  widened ONLY for exact evidence-backed cells; unrecognized, malformed,
  or conflicting evidence remains MISSING with a loud warning (no
  publishable unknown status).
- **Out of bounds**: weakening fail-closed for evidence-less cells; changing
  measurement semantics of successful cells; touching era-1 records.

Affected: `benchmarks/harness/` (summarize.py + tests) — no Rust crates.
Bd: rc-9mst.
