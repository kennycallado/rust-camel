# Design: bench-m2-attempted-status

## Approach

`summarize.py` already walks per-cell directories per m2 round
(`m2-round-<r>/<scenario>/<contender>/`, plus the flat
`<scenario>_<contender>` variant) looking for `m2-summary.json`.
Today: no summary → cell absent from run.json → publish gate counts it
MISSING. The evidence artifacts the harness itself writes in those same
directories are ignored.

Change the cell-collection walk: when a cell produced no valid
`m2-summary.json` in ANY round, the summarizer classifies its on-disk
evidence (all round dirs of that cell):

- `unconverged` — a `protocol-a-summary.txt` contains BOTH
  `warmup failed-stability: MessageBoundUnconverged` AND
  `status=failed reason=measure-a-error` (both lines, exact substrings).
- `attempted-timeout` — an `exit-codes.txt` line matching
  `# probe reason: no BENCH_LATENCY within 30s timeout` (exact).
- Anything else — sentinel present but malformed, unrecognized content,
  or the two statuses conflicting across rounds — is UNCLASSIFIED:
  the cell remains MISSING and a loud warning names the cell and the
  offending artifact. Status is never invented for ambiguous evidence
  (fail-closed). There is no `attempted-unknown` publishable status.

Precedence: one valid `m2-summary.json` in any round ⇒ MEASURED; attempt
evidence is then ignored.

Cell shapes in `run.json` (schema_version 2):

- measured: today's shape, unchanged (latency fields, NO status field).
- attempted: `{scenario, contender, metric: "m2", status:
  "unconverged"|"attempted-timeout", reason: <nonempty exact sentinel
  line>, rounds: <round dirs scanned>}` — no latency fields.

The publisher VALIDATES shapes and counts PRESENT only:
measured-with-latency, or allowed status + nonempty reason + no latency
fields. Status outside the two, missing/empty reason, status mixed with
latency, or a bare `{"metric":"m2"}` ⇒ MISSING (blocks publish) — a
hand-authored cell cannot bypass completeness.

Publish output gains the split (`present 44/44: 38 measured, 6
attempted`). A cell with no directory, empty evidence, or unclassified
evidence remains MISSING → fail-closed preserved.

`SCHEMA_VERSION` 1 → 2 (additive optional `status`/`reason` on m2
cells; measured shape unchanged). Compatibility is ONE-WAY: new tooling
reads v1 and v2; old tooling need not read v2. `INDEX_SCHEMA_VERSION`
stays 1 — the index entry shape is unaffected by cell-level status.

Classification lives in one helper (`classify_m2_attempt(cell_dirs) ->
Optional[dict]`) in the summarizer; the publisher re-validates shapes.
No changes to run.sh, builder, or fixtures. Test fixtures replicate the
two real shapes verbatim from `out/20260903T084658Z` — the artifact
format is the contract.

## Affected crates

None (Python/bash harness only):

- `benchmarks/harness/summarize.py`: evidence classifier, cell walk,
  shape validation at publish, SCHEMA_VERSION, publish output.
- `benchmarks/harness/test_summarize.py`, `test_publish.py`: new cases
  (both statuses, malformed, conflicting, precedence, shapes, v1/v2).

## Architecture boundaries

Benchmark harness side-channel — no Runtime/DSL/Components/Languages
code. The record schema (benchmark-records spec) is the contract being
extended, additively, versioned by `schema_version` (era comparability
rule: old records immutable, new fields additive). Fail-closed
principle preserved end to end: status is granted only for exact,
harness-written evidence; the publisher independently re-validates.

## Alternatives considered

- **Re-run m2 after the fixture fixes** (~10 h): rejected — the record's
  value is single-run provenance (one build, one host, one day); mixing
  runs breaks comparability and costs a day.
- **run.sh emits m2-summary.json with status natively**: right
  long-term (follow-up bd), but cannot help the already-frozen canonical
  run; artifact formats are stable documented shapes, so deriving in the
  summarizer is a durable contract, not log archaeology.
- **Loosen the gate to scenario-level**: rejected — hides real per-cell
  gaps; per-cell fail-closed stays.
- **Publishable `attempted-unknown`**: rejected (blessing review) —
  unrecognized evidence must block, not warn-and-pass.

Single-phase change (small coherent slice, ~3 tasks).
