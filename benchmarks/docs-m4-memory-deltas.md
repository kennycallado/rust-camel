# M4 memory deltas: negative quarkus-native values and node-native +26 MiB — mechanism analysis

Date: 2026-09-30 · bd rc-audm.2 (P3), input to rc-ycmef · mission 336
Worktree: `.worktrees/../rust-camel-worktrees/336-negdeltas` @ `feature/336-negdeltas`
Method: record forensics + harness-code reading + fixture-local experiment. No
record was re-run or republished; `benchmarks/records/**` untouched.

## 1. What m4 actually measures (harness semantics)

From `benchmarks/harness/run.sh` (m3/m4 arm) and `loadgen/src/rss.rs`:

1. Per round, a **fresh contender process** is started (killed at round end).
2. The load driver (`measure-throughput`) runs 10 s warmup + 50 s sustained
   load (POST, keep-alive pool, connections = available parallelism).
3. At t = 10 s — after warmup, **before the measured window** — an
   `rss-sample` background process starts sampling the contender's
   process-tree ΣVmRSS from `/proc/<pid>/status` every 2000 ms for 50 s.
4. `delta = sample[last] − sample[first]` — i.e. **RSS at t=60 s minus RSS at
   t=10 s**, both single 2-s-cadence samples of one fresh process.
5. 5 rounds; record stores per-round deltas, median is published.

`M3_WARMUP_SECS=10` is a fixed constant (only `M3_DURATION_SECS` is
env-overridable). The baseline is the **first sample at t=10 s of a process
whose memory state has not settled** — this is the root of both anomalies
below.

## 2. Record data (both era-2 records, http-server m4, KiB)

| contender | run 20260903T084658Z rounds | median | run 20260915T093128Z rounds | median |
|---|---|---|---|---|
| camel-quarkus-dsl-native | −4416, −1512, **+1484**, −1016, −112 | −1016 | −792, **+532**, −496, −888, −64 | −496 |
| camel-quarkus-yaml-native | −504, −5108, −5100, −4684, **+1548** | −4684 | −504, **+216**, −2548, **+2572**, −1936 | −504 |
| camel-standalone-dsl (JVM) | 316…560 | +348 | 344…484 | +464 |
| camel-standalone-yaml (JVM) | 236…392 | +304 | 188…404 | +276 |
| node-native | 26748…27432 | +26900 | 25640…26976 | +26740 |
| node-fastify | 116…740 | +356 | 0, 8, 0, 4, 332 | +4 |
| rust-camel-cli | 56…104 | +80 | 16…80 | +32 |
| rust-camel-lib | 44…144 | +76 | 88…228 | +128 |
| axum-bare (run 2 only) | — | — | 0, 0, 0, 0, 4 | 0 |

## 3. Quarkus natives: negative deltas are REAL reclamation, but a
measurement-window artifact in interpretation

**Verdict (rc-audm.2 acceptance): real behavior, not a sampling artifact —
the RSS decrease is genuine page release; but the negative *delta* is an
artifact of the metric's window semantics (unsettled baseline at t=10 s), not
"the contender releases memory under load".**

Reasoning:

1. **Page-cache artifact: refuted by construction.** VmRSS
   (`/proc/<pid>/status`) counts only pages mapped into the process. File
   page-cache pages are never part of VmRSS.
2. **NUMA artifact: refuted.** NUMA migration moves pages between nodes; the
   resident total is unchanged. THP collapse (`khugepaged`) adds padding and
   pushes RSS **up**, not down.
3. **The decrease is therefore genuine unmap** — `madvise(MADV_DONTNEED)` /
   `munmap` — from the runtime: GraalVM native-image GC uncommitting heap
   grown during the warmup burst, and/or glibc `malloc` trimming per-thread
   arena tops (default `M_TRIM_THRESHOLD` 128 KiB) after warmup-burst
   allocations. Both mechanisms return warmup-transient over-allocation.
4. **Mixed-sign rounds prove the unsettled-baseline interpretation.** Within
   a single cell, rounds flip sign (dsl-native run 1: −4416…+1484; yaml-native
   run 2: −2548…+2572). A deterministic "releases memory under load" mechanism
   would give consistently negative, tight rounds. Instead the sign depends on
   whether the t=10 s baseline caught the warmup transient inflated (→
   negative delta as it drains) or already settled (→ small positive drift).
   Contrast node-native: a deterministic mechanism (§4) produces tight,
   same-sign rounds — the two signatures are visibly different classes.
5. **Run-1 → run-2 narrowing** (yaml-native median −4684 → −504) is
   environment sensitivity of the same transient, consistent with a
   stochastic baseline, not a stable property of the contender.
6. **Alternative sampler artifact, examined and dismissed (with a check for
   the container run):** the sampler sums the process *tree*; a child exiting
   mid-window would fake a negative delta. Quarkus natives are single-process
   (vert.x in-process; no fork), so the tree is one PID throughout. The raw
   per-round m4 files (with per-sample `process_count`) are not retained in
   the record; an authorized container run can close this conclusively by
   checking `process_count` is constant per trace.

**What an authorized container run would still add (optional, not required
for the disposition):** the split between GC uncommit vs malloc-trim (via
`smaps_rollup` anon-vs-file, `MALLOC_ARENA_MAX` A/B, or strace madvise
counts), and the in-window trace shape (records retain only per-round
deltas). It cannot change the artifact-vs-real verdict: that follows from
VmRSS semantics plus the round-value distribution above.

## 4. node-native +26 MiB: V8 committed-heap growth steps, baseline sampled
before plateau (wasm attribution formally retracted)

Fixture-local experiment, host node v22.23.2 (container used its own node;
mechanism is V8-generic). Protocol mirrors the harness: fresh process,
keep-alive POST load at ~18 k req/s (container: ~25.7 k), external 2 s
VmRSS sampler, plus passive 1 s in-process instrumentation
(`process.memoryUsage()` + `v8.getHeapStatistics()`). Artifacts:
`/tmp/bench-probe-negdeltas/{A,B}/` (vmrss.csv, heap.jsonl, driver.json).

**Round A (harness protocol, 10 s warmup + 50 s window):** window VmRSS flat
at ~72.2 MiB for 34 s, then a single **step of +6.7 MiB** (72.2 → 79.0 MiB),
flat to window end. Attribution: `heapTotal` stepped **+8 MiB exactly**
(17.4 → 25.8 MiB) while `heapUsed` went **down** 1.0 MiB; `external` /
`arrayBuffers` flat (+15 KiB); `malloced_memory` flat. The RSS step **is**
the V8 committed-heap step becoming resident.

**Round B (identical, but 70 s warmup):** window delta **+24 KiB**. The
heap-step log over process life shows growth 11→12→16→24 MiB completing by
t≈48 s of load; with the baseline taken at t=70 s the plateau is already
reached and the window shows nothing.

**Verdict:** the +26 MiB is **deterministic V8 committed-heap growth in
discrete steps that has not plateaued at the t=10 s baseline**. It is not a
leak (live set flat, `heapUsed` declining across the window), not Buffer/
agent churn (external flat), and not wasm — the 2026-09-11 burst attribution
"explained by rc-audm.1 wasm heap churn" is hereby **formally retracted**
(per rc-ycmef; it was already impossible: m4 is http-server-only, no wasm in
the cell). node-fastify's +4 KiB is the same runtime with a *higher*
per-request allocation rate: it reaches the heap plateau during the 10 s
warmup, so its window is flat. The published cell therefore measures
**distance-to-plateau, not steady-state growth**, and the "4 orders of
magnitude vs fastify" reading is invalid as a memory-behavior comparison.

## 5. Unified root cause and remediation options

Both anomalies are the same metric defect: **m4's baseline is a single
sample of an unsettled process at t=10 s.** Contenders differ only in settle
behavior — rust/axum settle instantly (≈0), JVM drifts slightly positive
(no uncommit in window), node grows stepped to a late plateau (+MiB),
native-image drains warmup transient below baseline (−MiB). Cross-contender
m4 comparison as published is invalid.

Options (any one restores comparability; (a) is the smallest change and
matches the rc-audm.8 precedent):

- (a) **Baseline settle-gate**: require N consecutive 2 s samples within ε
  (e.g. 3 samples, 256 KiB) before the measured window starts — the RSS
  analogue of the blessed trailing-window warmup protocol
  (`benchmarks/harness/CONTEXT.md` §2, rc-audm.8).
- (b) Publish `max − initial` alongside `final − initial`, plus absolute
  `rss_initial`/`rss_final`, so post-hoc re-interpretation is possible.
- (c) Adaptive warmup extension until the settle-gate passes (bounded).

Independent of the above, the summarizer should carry per-sample
`process_count` into `m4-summary.json` (constant-tree check, §3.6).

## 6. Record corrections to note (no republish)

- `records/20260903T084658Z/CAVEATS.md` §"XSD memory results" line — "the
  `node-native` m4 increase includes heap churn from the per-tick XSD
  workers" — is **false for m4** (http-server cell, no wasm) and is
  retracted by rc-ycmef + this note. Records are never republished; the
  retraction lives here and in bd rc-ycmef/rc-audm.2.
- `docs-investigation-strategy.md` §8 row rc-audm.2 repeats the retracted
  attribution ("node +26.3MiB explained by audm.1"); the epic notes already
  carry the 2026-09-17 correction pointer. Do not quote the stale line.

## 7. Dispositions

| question (bd acceptance) | answer |
|---|---|
| negative m4 deltas artifact or real? | Real RSS reclamation (genuine unmap); negative *delta* is a measurement-window artifact — unsettled t=10 s baseline drains during the window. Not page-cache, not NUMA. |
| conclusion with method recorded? | This note; method = record forensics + harness-semantics analysis + fixture-local experiment. |
| node-native +26 MiB dispositioned? | V8 committed-heap growth steps pre-plateau (experiment A/B); retracts wasm attribution; informs rc-ycmef and rc-wkqt0. |
