# Era-3 Re-Run Manifest (frozen 2026-09-16)

- **Authority:** e_opus ruling item 2 (mission 98 Phase B checkpoint,
  `e_opus-fixture-shape-ruling.md`): "frozen, enumerated list of re-run
  vs carried-forward arms (D3 invalidates ALL cli Protocol-B arms)."
- **Alignment landing:** worktree `feature/benchintegrity` at
  `58d414ab`+ (D1/D2/D4/D5 fixtures, D3 sentinels, F1/F2 sweep) and
  `81bc9b96` (degenerate-pair rule). Era-2 record
  `20260915T093128Z` stays sealed.
- **Scheduling:** master-owned (daytime, host 8080 free, quiet-host
  gates, `BENCH_SCRATCH_DIR` on /home/shared, ~9G; the metric default
  is now the FULL set — bare `run-all` produces the complete record).

## Disposition rule

A cell is RE-RUN when (a) its own fixture/instrument/window changed in
the alignment, or (b) any cell it is compared with in the same
(scenario × metric) comparison changed — cross-contender cells must be
same-era. A cell is CARRIED only when nothing in its scenario changed
at all.

## Frozen list

| Scenario | Contenders | Metrics | Disposition | Reason |
|---|---|---|---|---|
| http-server | all 8 + axum-bare | m1+m2+m3+m4 — EXCEPT node-fastify/node-native: **m1+m2 only** (D5 exclusion, see gate 6) | **RE-RUN** | D1 changed lib/node×2/axum fixtures; same-era rule pulls the unchanged Java/cli cells in (m2 protocol-A + m3/m4 cross-contender). Node family is out of m3/m4 this era (declared confound acceptable for m2 protocol-A only) |
| t2-json | all 8 | m1+m2 | **RE-RUN** | D4 moved the output assert out of the window on EVERY cell (window content changed for all); D3 cli sentinels |
| t2-realistic-eip | all 8 | m1+m2 | **RE-RUN** | D3 re-anchor: stamps moved after set_body, marker after close on every cell; lib double-marker fix |
| split-aggregate | all 8 | m1+m2 | **RE-RUN** | lib clock+delay=0, node t0 position, cli window + F2 assert; same-era rule |
| xsd-validation-bridge | all 6 | m1+m2 | **RE-RUN** | cli wrapper switched to pair mode (D3) — window semantics changed for the cli arm; same-era rule for the pair-A ratios it shares |
| xslt-bridge | all 6 | m1+m2 | **RE-RUN** | same as xsd |
| startup-minimal | all 8 | m1 | **RE-RUN** | the shared rust-camel-lib fixture BINARY changed (http-server + warm-tick scenario modules + trace gate); even though the startup-minimal code path is untouched, the measured artifact is new — same-era rule applies to the scenario's m1 cross-cell comparison (trivial cost: m1-only, minutes) |

**Branch (startup-minimal, retired):** an earlier draft carried era-2
startup-minimal numbers on a zero-change assumption; stage-4 review
(e_glm) corrected it — the shared lib fixture binary changed, so the
scenario re-runs (m1-only, trivial). If the WRAPPER-ASYM ruling (item
4) additionally lands node fixture changes before era-3, nothing
changes here — the scenario is already re-run.

**Every cli Protocol-B arm (t2-json, split-aggregate,
t2-realistic-eip, xsd, xslt — m2) is invalidated BY CONSTRUCTION**
(ruling parenthetical): era-2 cli m2 values must never be cited as
like-for-like against era-3 numbers.

## Era-3 gates (in order)

1. **Fresh smoke** per D2: `scenarios/http-server/smoke/run.sh` runs
   against live cells (the committed stale logs were deleted at
   `585858c6`); hard assert = marker + 200/pong; `id=1` is WARN-only.
   New transcripts land as the fresh evidence.
2. **Trace-absent proof** per D1: `benchmarks/harness/checks/trace-absent.sh`
   on the built lib fixture binary MUST PASS (verified in-worktree at
   alignment time: PASS default / FAIL `--features bench-trace`).
3. **Bridge pair-mode record gate** (explicit, from the W3 review): a
   2026-09-03 diagnosis once claimed pair mode "emits no BENCH_LATENCY
   records for bridge routes"; the current code contradicts it
   (`inject_timing` wraps the top-level `to(validator/xslt)`). The
   FIRST bridge arm of era-3 must confirm the cli bridge cells emit
   one record per tick under pair mode — if the old diagnosis was
   right, this fails the m2 probe loudly (fail-visible, not silent).
4. Quiet-host gates + full-metric default (`run-all` bare = complete
   record).
5. One-time native rebuild per quarkus artifact is EXPECTED (the
   rc-wdy13 fix makes the fingerprint cache honest for the first
   time — `6a375fbb`); budget ~65-80s per artifact, not a failure.
6. **Node family stays OUT of m3/m4** (D5): parser retained (no
   content-type in bench clients ⇒ 415 without it), confound declared
   for m2 protocol-A only. The m3/m4 roster for http-server excludes
   node-fastify/node-native this era. The exclusion is MECHANICAL
   (run.sh `M3_EXCLUDED_CONTENDERS` filter inside `m3_measure`,
   mirrored in summarize.py as a drift guard) and UNCONDITIONAL —
   **era-4 inherits it silently until manually removed**; put "lift
   D5 node m3/m4 exclusion?" on the era-4 checklist (requires a
   body-handling strategy for no-content-type clients first).
7. Degenerate-pair rule applies at summarize time automatically
   (bridge cli renders "Unpaired (context-only)"; no cross-pair rows).

## Accepted residual divergences (documented, not fixed)

- **Assert placement asymmetry between scenarios (D3+D4 rationale,
  stage-4 e_glm):** t2-json's output assert is pure output verification
  (no pipeline-state dependency) — ruled OUTSIDE the window on every
  cell; split-aggregate's completion assert reads `CamelAggregatedSize`,
  which exists only on the aggregation completion path (the pipeline's
  own semantics) — ruled INSIDE the window, uniformly on all 8 cells.
  The distinction is verification-of-output vs completion-of-work; both
  placements are scenario-uniform, so neither introduces cross-cell
  bias.
- **Marker-gate idioms** (proposal §6): JVM AtomicBoolean CAS vs cli
  idempotent-repo probe vs lib AtomicBool swap — all O(1) once-only
  branches after first fire; accepted as implementation idiom, inside
  windows uniformly.
- **quarkus jetty vs platform-http** (http-server Pair A): transport
  axis confounded with JVM-vs-native inside the pair; pre-existing,
  documented in the fixtures (NativeBenchRoute.java:16-19).
- **Node engine caveats** (xmllint-wasm ≠ Xerces-J; Saxon-JS ≠
  Saxon-HE): declared family properties, documented in-file.
- **lib tracing-subscriber init retained** in the http-server fixture:
  era-2 parity (present at `9e8f36f` with the bare route).
