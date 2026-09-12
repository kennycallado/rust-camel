# rc-audm Investigation — STAGE 0 Measurement Strategy

Oracle-authored measurement plan for the rc-audm investigation epic. Workers
execute; this document is the contract for HOW they measure. Read
`CONTEXT-MAP.md` for domain language before touching fixtures.

**Prime directive.** The official era-2 record (`records/20260903T084658Z`) is
the project's public face. It is NOT re-run and NOT republished (human-only).
Every measurement here exists to adjudicate one hypothesis: *fixture/harness
defect* vs *real cost*. Numbers produced under this plan are investigation
evidence, never a record.

---

## 0. Universal measurement protocol (applies to EVERY bench run below)

These rules are non-negotiable. A measurement that skips any of them is
invalid and its finding is void.

### 0.1 Noise gate (mandatory, every run)
The host has ~12 cores, loadavg ~1, but a co-agent fleet adds noise
unpredictably. Before AND after every measurement:
1. Record `/proc/loadavg` (1-min field) and `nproc`.
2. Record `cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq` if
   present (frequency drift is a silent p50 mover).
3. **Reject rule:** if 1-min loadavg drifts by more than ±0.5 absolute OR
   more than ±25% relative between pre/post samples, DISCARD the run and
   retry. Record both samples in the finding artifact regardless.
4. Pin the loadgen + contender to a fixed CPU set when the harness supports
   it (`SERVER_AFFINITY`/`taskset`), leaving ≥2 cores free for the OS. Never
   let a measurement float across all 12 cores while co-agents churn.

### 0.2 Bounds / timeouts (no leftover processes)
- Every subprocess launched under a hard `timeout` (wall-clock). Standalone
  fixture probes: 120 s cap. Harness single-cell re-runs: inherit the
  harness 600 s runaway guard, add an outer `timeout 900`.
- After every run: assert no orphaned contender/bridge/loadgen PIDs
  (`pgrep -f` the fixture path; kill + fail the finding if any survive).
- Pre-clean tmpfs (`/tmp/v3-protocol-b-*.log`, `/tmp/v3-bridge-pid-*.txt`)
  before each probe — the harness already does this; standalone probes must
  replicate it.

### 0.3 Sample-size floor
- **Latency (p50/p99):** ≥ 5 rounds (separate process launches) × ≥ 2,000
  post-warmup samples/round. Report median-of-per-round-p50 AND the p99
  spread across rounds. A single-round number is never an adjudication.
- **Standalone micro-probes** (node fixture isolation): ≥ 3 process launches,
  ≥ 1,000 samples each, report full quartiles + max. Re-instantiation costs
  hide in the tail, not the median — always inspect the max.
- **Throughput (m3):** ≥ 3 rounds × 10 s each, 2 s warmup discarded.

### 0.4 Host-vs-container comparability (CRITICAL for node)
The era-2 record ran **in a container on a pinned node build**. The host is
node v22.23.2. **Host node numbers are NOT directly comparable to the record.**
- Any node probe reports the delta *shape* (ratio between fixture variants on
  the SAME host), never an absolute compared to the record.
- When a finding needs an absolute-vs-record claim, the run MUST use the
  container runner (docker 29.7.2) with the record's node pin, OR the finding
  is downgraded to "shape-only, host node, not record-comparable".

### 0.5 Fixture rebuild reproducibility
- Record the exact toolchain (`rustc -V`, `node -v`, cargo lock hash, node
  `package-lock.json` hash) in every finding.
- Rust fixtures: build with the workspace's debuginfo-free profile (per
  AGENTS.md). If a finding needs frame-level attribution, use
  `RUSTFLAGS="-Cdebuginfo=1"` for that single invocation only, documented in
  the finding — never persisted (splits sccache key).
- **Build in the worktree `./target` only.** Never re-heat the main checkout.

---

## 1. TRIAGE — disposition of all 13 suspects

Verified against code on branch `bench` (see indexed source
`bench-investigation-facts`). Two suspects had their fix **already landed**;
this is the single biggest correction to the assumed plan.

| Suspect | Class | Disposition |
|---|---|---|
| **rc-dh7t** (P2, discover skip) | **TOOLING — already in code** | Verify run.sh:850 emits skip notice (it does), add regression test, close. NO measurement. |
| **rc-audm.7** (P3, native m2 status) | **TOOLING** | run.sh already emits m2-summary.json/.txt statuses. Add a single native `status` field so summarize.py stops deriving. NO measurement. |
| **rc-audm.8** (P3, time-based warmup) | **VERIFY-LANDED then MEASURE** | `WarmupConfig{30s,1000,10%}` + rc-tpig adaptive extension ARE in code. The 3 unconverged era-2 cells predate this. Re-run those 3 cells on current code to confirm convergence; if still failing, THEN it is a live measurement. |
| **rc-u047** (P3, dedup presence) | **TOOLING (cosmetic)** | Extract one `identity→measured\|attempted\|None` helper. NO measurement. Do NOT split the 1600-line file in this epic (scope creep). |
| **rc-2k33** (P3, roster triple-author) | **TOOLING** | Either derive from one source OR document the three-file contract + keep the drift test. NO measurement. |
| **rc-am22** (P2, smoke id emission) | **TOOLING + smoke regen** | Restore `BENCH_HTTP_REQUEST id=1` in rust-camel-lib fixture (all other contenders emit it) OR relax assertion; regenerate committed smoke log. Smoke, not a benchmark. |
| **rc-audm.1** (P2, xsd 41ms) | **LOCAL MEASUREMENT** | node `xmllint-wasm` fixture probeable standalone on host node v22. Highest-value node finding. |
| **rc-audm.3** (P2, eip 28µs) | **LOCAL MEASUREMENT** | rust-lib vs node t2-realistic-eip; harness-defect-first hypothesis. |
| **rc-audm.5** (P3, cli pipe tax) | **LOCAL MEASUREMENT** | rust-camel-cli t2-json 2.7ms vs lib 135µs; cli-runtime pipe instrumentation. |
| **rc-audm.6** (P3, xslt protocol) | **LOCAL MEASUREMENT (asymmetry only)** | Prove the 1.7ms-vs-3.0ms gap is measurement-point (in-route marker vs harness probe), not real cost. Then park. |
| **rc-mr6u** (P3, allocator axis) | **LOCAL MEASUREMENT (build-gated)** | mimalloc/jemalloc m3 experiment; needs fixture rebuild. Gated behind rc-audm.3 alloc-share confirmation. |
| **rc-u034** (P3, axum-bare + devnull threads) | **LOCAL MEASUREMENT (new fixture)** | New axum-bare reference contender; devnull 2-thread cap CONFIRMED (cli_runtime.rs:48). |
| **rc-audm.2** (P3, negative m4) | **PARK / ADJUDICATION** | Quarkus native RSS needs java → container-only. No java on host. Park with methodology note unless a container matrix run is separately authorised. |

**Protected — do NOT touch:** rc-audm.9, rc-p2vm.

---

## 2. Per-measurable-suspect designs

### rc-audm.1 — xsd-validation-bridge node = 41 ms/tick (123× quarkus-native)

**Hypothesis tree**
- H1 (LIKELY): per-tick WASM re-instantiation. `xmllint-wasm` module and/or
  the compiled XSD schema is (re)loaded inside the per-message route body
  instead of once at startup. 41 ms is the signature of a module
  compile+instantiate, not a validate.
  - H1a: module instantiated per tick.
  - H1b: schema parsed/compiled per tick (module cached, schema not).
- H2 (LESS LIKELY): genuine wasm validate cost for this payload (would make
  it a real-cost finding, not a fixture defect).
- H3: harness Protocol-B measurement point captures process-level jitter
  (ruled out early — the fixture emits absolute per-tick ns from inside the
  route, so the harness parse cannot inflate the number).

**Measurement design** — standalone fixture probe (NO harness, NO container).
1. Read `contenders/node/node-native/xsd-validation-bridge.mjs` and
   `.../node-fastify/xsd-validation-bridge.mjs`. Identify where the wasm
   module + schema are created relative to the per-tick handler.
2. Drive the fixture handler in a tight loop under host node v22
   (`node --version` recorded), 3 launches × 1,000 iterations, timestamping
   each validate with `process.hrtime.bigint()`. Report p50/p99/max +
   first-iteration cost separately (instantiation shows as a giant iter-1).
3. **Instrumented A/B:** add a one-line probe counting module-instantiate and
   schema-compile calls (temporary, not committed). If count == iterations →
   H1 proven. If count == 1 → H1 refuted, escalate to H2.

**Bounds:** 120 s per launch. **Noise gate:** §0.1. **Samples:** §0.3 micro.

**Evidence threshold (fixture-defect vs real-cost):**
- FIXTURE DEFECT if instantiate/compile count scales with iterations, OR if
  hoisting the module+schema out of the handler drops p50 by ≥ 10× on the
  same host. Then: file the fixture fix as a finding, note the record is
  measuring a fixture bug (methodology note, record NOT re-run).
- REAL COST if count == 1 and hoisting changes nothing and per-validate p50
  stays multi-ms. Then park with the real-cost hypothesis.

**Comparability:** shape-only vs quarkus (java, not on host). The 123× ratio
cannot be re-derived on host; only the node self-improvement from hoisting is
host-valid. State this explicitly in the finding.

### rc-audm.3 — t2-realistic-eip rust-lib 28 µs vs node 9–11 µs (warm)

**Hypothesis tree**
- H1 (harness-defect-FIRST, per brief): fixture unfairness — the rust-lib
  route does strictly more work per tick than the node fixture (extra
  serialization, an extra EIP hop, a clone the node path avoids).
- H2: real EIP overhead — tower/camel per-message dispatch + exchange
  allocation. Prior art rc-audm.4 already attributes rust-lib HTTP to
  alloc+memmove 28% / HeaderMap 19% / hyper+tokio 18% / camel 5%; the EIP
  tick path shares the alloc component but NOT the http-crate component.
- H3: node numbers are JIT-warmed to an unfair steady state the rust path
  can't match by construction (real, but a "different runtime physics" note,
  not a defect).

**Measurement design**
1. **Fixture-parity audit FIRST (no run):** diff the per-tick body of
   `contenders/rust-camel-lib` t2-realistic-eip against
   `node-native/t2-realistic-eip.mjs`. Enumerate every operation per tick on
   each side. This is the cheapest possible adjudication and per the brief is
   the FIRST step. If node skips an EIP step rust does → H1 confirmed by
   inspection, no run needed.
2. If bodies are parity: re-run the single cell `t2-realistic-eip/rust-camel-lib`
   under the current harness (Protocol B, §0.3 sample floor) to reproduce
   ~28 µs on host. Compare to node-native same-host re-run (shape-only).
3. If confirming real cost: allocation attribution. No `perf` on host; use
   the loadgen self-instrumentation path OR a `RUSTFLAGS="-Cdebuginfo=1"`
   single build + `/proc`-based sampling. Confirm whether the 28% alloc share
   from rc-audm.4 holds on the tick path (this feeds rc-mr6u).

**Evidence threshold:**
- FIXTURE DEFECT if the per-tick work differs (H1). Adjudicate by inspection +
  a parity-corrected re-run showing the gap shrinks toward node.
- REAL COST if bodies are byte-parity and the 28 µs reproduces with alloc as
  the dominant share. Park with "real EIP alloc cost, candidate for rc-mr6u".

### rc-audm.5 — cli pipe tax: t2-json cli 2.7 ms vs lib 135 µs (20×)

**Hypothesis tree**
- H1 (LIKELY): stdout flush granularity — the cli runtime buffers/flushes
  per-tick with a syscall or line-buffered writer, and the stdin pump wakes on
  coarse granularity. 2.7 ms ≫ 5–50 µs pipe RTT ⇒ the cost is scheduling/flush
  policy, not the pipe.
- H2: stdin read wakeup — cli blocks on a read that only wakes on a timer or
  large buffer fill.
- H3: real per-message cli dispatch cost (route parse/exchange build per
  message) independent of the pipe.

**Measurement design**
1. **Isolate the pipe from the route:** write a minimal loopback harness that
   feeds the cli its stdin protocol and times round-trips, with the route
   replaced by an identity/no-op (or the smallest registered route). If the
   2.7 ms persists with a no-op route → H1/H2 (pipe/flush). If it vanishes →
   H3 (route work).
2. **Flush-policy A/B:** in `cli_runtime.rs`, the only stdout flush in the
   hot region is at :65 (devnull marker). Locate the cli message loop's write
   path; instrument write-count and flush-count per message. Compare
   BufWriter vs immediate-flush vs `write_all`+explicit-flush timings.
3. Cross-check against Protocol A: the record shows HTTP is IDENTICAL (166 µs)
   for cli and lib — so the tax is pipe-transport-specific, which already
   rules out H3 partially. Use that as a built-in control.

**Evidence threshold:**
- FIXTURE/RUNTIME DEFECT if the no-op-route probe still shows multi-ms and
  flush-count scales per message (H1). Fix = batch/flush policy; land as a cli
  runtime finding.
- REAL if HTTP-identical + no-op-route is fast + only the real route is slow
  (H3) — but the HTTP-identical control makes this unlikely. Park if so.

**Shared instrumentation:** the stdin/stdout pump instrumentation here is the
SAME probe rc-audm.6 needs for the cli xslt protocol asymmetry. Build it once.

### rc-audm.6 — xslt-bridge cli 1.7 ms manual vs lib 3.0 ms record

**Hypothesis (narrow — this is park-oriented):** the gap is a
**measurement-point asymmetry**, not a real cli-vs-lib cost. The record's lib
number is a harness Protocol-B probe (in-route `BENCH_LATENCY` emission, absolute
ns per tick); the manual cli 1.7 ms was measured at a different point
(wall-clock around the cli invocation vs in-route marker).

**Measurement design (asymmetry only — do NOT chase the absolute):**
1. Instrument BOTH the cli and lib xslt routes to emit `BENCH_LATENCY` at the
   IDENTICAL in-route point (immediately around the xslt transform call).
2. Re-measure both under Protocol B, same host, §0.3 floor.
3. If the two now agree within noise → asymmetry PROVEN, the 1.7-vs-3.0 gap
   was a probe-placement artifact.

**Evidence threshold / park note:** once the same-point measurement collapses
the gap (or bounds it), PARK. This suspect does not need root-causing beyond
proving the asymmetry — the brief says the next canonical run measures both
natively. See §4 for the exact park note.

### rc-mr6u — allocator axis (mimalloc/jemalloc)

**Gated behind rc-audm.3** confirming alloc is the dominant share on a hot
path. Only run if rc-audm.3 (or rc-audm.4's 28% HTTP share) is confirmed live.

**Design:** build the http rust fixture with `mimalloc` and `jemalloc`
global allocators (feature-gated, workspace-checked — do NOT add a dep the
workspace lacks without checking `Cargo.toml`). Re-run m3 (throughput) for
http, 3×10 s, §0.1 gate. Expected 10–20% m3 gain if the 28% alloc share holds.

**Evidence threshold:** report the m3 delta with CI. This is an EXPERIMENT
(bench-axis), not a defect adjudication — its output is a recommendation, not
a fixture fix. Never touches the record.

### rc-u034 — axum-bare reference + devnull worker threads

**Design (two independent pieces):**
1. **axum-bare contender:** new fixture between devnull (ceiling) and rust-lib,
   isolating stack-tax (hyper/axum/tower) from camel-tax in the m3 table.
   Build + smoke + one m3 cell. This is additive tooling + one measurement.
2. **devnull thread raise:** cli_runtime.rs:48 `worker_threads(2)` CONFIRMED.
   Prior art (rc-audm.4) says devnull starves under load. Measure m3 ceiling
   at worker_threads ∈ {2, 4, 6} to confirm the cap is the ceiling limiter.
   **CAUTION:** raising devnull threads changes the CEILING the record cites.
   This is investigation-only; the record's ceiling is NOT republished.

**Evidence threshold:** axum-bare m3 that sits between devnull and rust-lib
validates the stack-vs-camel decomposition. devnull thread sweep showing m3
rising with threads confirms the starvation hypothesis.

### rc-audm.2 — negative m4 RSS deltas (PARK / adjudication)

Quarkus native RSS requires java → **not measurable on host** (no java). Two
paths: (a) authorise a container run (heavy, separate budget), or (b) adjudicate
as a methodology note. **Default: PARK** with the malloc-arena/trim hypothesis
(§4). node-native +26.3 MiB is a wasm-heap artifact and is separately explained
by rc-audm.1's fixture finding if H1 holds (the wasm heap grows under per-tick
instantiation).

---

## 3. Ordering (what unblocks what)

Execute in this order. Rationale = shared instrumentation + dependency gates.

**Phase A — tooling fixes (no measurement, land first, orthogonal to all runs):**
1. rc-dh7t (verify skip notice + test + close) — pure, isolated.
2. rc-am22 (restore/relax smoke id + regen log) — pure, isolated.
3. rc-audm.7 (native m2 status field) — pure, isolated.
4. rc-u047 (dedup presence helper) — pure, isolated.
5. rc-2k33 (roster single-source or documented contract + drift test) — pure.
   These are independent commits (one per concern per constraint). They do NOT
   invalidate any measurement because the measurements below read fixture
   timings, not roster/tooling shape. See §6 for the sequencing proof.

**Phase B — cheap adjudications (inspection-first, may need no run):**
6. rc-audm.3 fixture-parity audit (pure inspection; may close by inspection).
7. rc-audm.8 verify-landed: re-run the 3 previously-unconverged cells on
   current code. If they converge (expected — the fix is in), close/park with
   evidence. If not, escalate to a live measurement.

**Phase C — local measurements (build shared instrumentation once):**
8. rc-audm.5 cli pipe tax — builds the stdin/stdout pump instrumentation.
9. rc-audm.6 xslt asymmetry — REUSES the pump instrumentation from step 8.
   (This is the primary ordering dependency: 8 unblocks 9.)
10. rc-audm.1 xsd node probe — independent (node fixture, own instrumentation).
    Its H1 outcome INFORMS rc-audm.2's node-native +26 MiB explanation.
11. rc-audm.3 live measurement (only if step 6 didn't close it) — its alloc
    attribution GATES rc-mr6u.

**Phase D — gated experiments:**
12. rc-mr6u (only if step 11 confirms alloc share) — allocator axis.
13. rc-u034 (axum-bare + devnull threads) — independent, can run anytime in C/D.

**Phase E — parks:**
14. rc-audm.2 park note (unless container run authorised).

Cross-suspect information flow:
- rc-audm.1 H1 (wasm re-instantiation) → explains rc-audm.2 node-native
  +26 MiB heap growth. Run .1 before finalising .2's park note.
- rc-audm.5 pump instrumentation → reused by rc-audm.6. Build once.
- rc-audm.3 alloc attribution → gates rc-mr6u. Do not build the allocator axis
  before confirming alloc dominates.

---

## 4. Parks — narrowed hypotheses + exact park notes

**rc-audm.6 (xslt protocol asymmetry) — PARK after proving asymmetry:**
> Narrowed hypothesis: the 1.7 ms (manual cli) vs 3.0 ms (lib record) gap is a
> measurement-point artifact, not a cli-vs-lib cost difference. The record's
> lib number is an in-route Protocol-B `BENCH_LATENCY` emission; the manual cli
> number was taken at a different probe point. Same-point re-measurement
> (both routes emitting `BENCH_LATENCY` around the transform) collapses the
> gap to within noise. Parked: no root-cause action needed; the next canonical
> run measures both natively at the same point. Not a fixture defect, not a
> real-cost finding.

> **SUPERSEDED (2026-09-11, outcome per §8 burst):** inspection proved the
> bracket map — the manual cli ~1.7 ms is an in-route route-mode marker
> covering the whole tick (`bench_instrument.rs:193-259`); the record's lib
> 3.0 ms bracket covers only the `.to(xslt)` dispatch
> (`xslt-bridge.rs:150-173`). The cli bracket strictly CONTAINS the lib
> bracket, so bracket asymmetry cannot yield cli < lib — the hypothesized
> same-point-collapse note above is REFUTED (anti-directional). Residual
> ~1.3 ms is run-condition-associated. Adjudicator: the next canonical run
> (measures both natively, same point). Probe-cost correction NOT needed —
> emit I/O sits outside the brackets on both sides.

**rc-audm.2 (negative m4 RSS) — PARK (no java on host):**
> Narrowed hypothesis: quarkus-native negative RSS deltas are a
> malloc-arena/trim artifact — the native image trims arenas after startup so
> the post-warmup RSS sample falls below the baseline sample (negative delta
> is measurement-window, not a real memory saving). node-native +26.3 MiB is a
> wasm-heap artifact, correlated with the rc-audm.1 per-tick-instantiation
> finding (if H1 holds, the wasm heap grows under repeated instantiation).
> Parked: quarkus RSS is not measurable on this host (no java); adjudication
> requires an authorised container matrix run. Methodology note added to the
> record's summary as a known-artifact caveat WITHOUT re-running the record.

**rc-audm.8 (time-based warmup) — PARK-as-landed if verification passes:**
> Verified: the time-based warmup budget (30 s OR 1,000 msgs, 10%
> within-warmup stability) and the rc-tpig adaptive window extension are
> already in `warmup.rs` + `run.sh`. The 3 unconverged http cells in the era-2
> record predate this code. Re-running those 3 cells on current code
> **does not converge** (verified 2026-09-11, host node v22.23.2, worktree
> `bench` @ 7029fd39, bench-loadgen rebuilt from current source; warmup unit
> tests 10/10 pass).
> - node-native (protocol A, the only host-verifiable cell): warmup
>   `MessageBoundUnconverged` in 4/5 rounds at the cell's own calibrated rate
>   1280/s (`max_sustainable_rate_per_sec=1280`); rate sweep 300–1280/s gives
>   5/14 stable overall (500/s: 3/3, 800/s: 1/3, 300/s: 0/3) — the first-500
>   vs second-500 p50 drift (~9–15%, V8 tier-up) straddles the 10% tolerance.
> - The 30 s wall is structurally inert here: at any calibrated rate ≥34/s
>   the 1,000-msg bound binds first (0.78–3.3 s), so the "time-based" budget
>   component never engages. The adaptive 6×/600 s extension is protocol-B
>   only — protocol A has no extension path (extension-assisted: NO).
> - JVM cells (standalone-dsl, standalone-yaml) deferred: no JDK on host;
>   measurement-mode artifact resolution hard-fails (deferral is dry-run-only).
> PARK-as-landed FAILS → per the rc-audm.8 triage row this escalates to a
> live measurement / warmup-criterion design question (tolerance vs bound
> interaction), NOT a park.

---

## 5. Risks (adversarial — the record is the public face)

1. **Host noise false positives.** A co-agent spike during a p50 probe inflates
   the tail and can flip a fixture-defect verdict. MITIGATION: §0.1 gate with
   reject-and-retry; report pre/post loadavg in every finding; require the
   effect to survive ≥ 3 launches. A single-run finding is inadmissible.
2. **Node version drift (record=container/pinned, host=v22.23.2).** Any host
   node absolute compared to the record is INVALID. MITIGATION: §0.4 —
   host node findings are shape-only (ratio between fixture variants on the
   same host). Absolute-vs-record claims require the container runner or are
   downgraded. This is the single most likely way to publish a wrong number.
3. **Fixture rebuild irreproducibility.** A rust fixture rebuilt with a
   different profile/allocator/lock changes the timing. MITIGATION: §0.5 —
   record toolchain + lock hashes; build in worktree target only; debuginfo
   only per-invocation.
4. **Instrumentation changing the measured value.** Adding a per-tick counter
   or flush probe can itself move p50 (observer effect), especially for the
   µs-scale eip/cli suspects. MITIGATION: instrument OUT of the hot path
   (counters incremented, printed once at exit — never per-tick I/O); A/B the
   instrumented vs clean build to bound the probe's own cost.
5. **devnull thread raise (rc-u034) mutating the cited ceiling.** Raising
   worker_threads changes the m3 ceiling the record references. MITIGATION:
   investigation-only; never republish; document the sweep as hypothesis
   evidence, not a new ceiling.
6. **Adaptive-window extension masking a real drift (rc-audm.8).** The 6×
   extension can turn a genuinely non-converging (drifting) runtime into a
   "passed" cell by brute-force sampling. MITIGATION: when verifying .8, also
   inspect WHETHER the cell converged or merely collected enough samples via
   extension — report the distinction; a cell that only passes via max
   extension is still a drift signal, not a clean pass.

---

## 6. Tooling-fix sequencing vs measurement validity

**Do the tooling fixes (Phase A) FIRST, and they do NOT invalidate any
measurement.** Proof of orthogonality:
- rc-dh7t, rc-audm.7, rc-u047, rc-2k33 touch discovery/roster/summary SHAPE —
  they change which cells are *reported* and how *statuses* are *derived*, not
  the per-tick fixture *timings* the measurements read. A measurement reads
  `/tmp/v3-protocol-b-*.log` values; these fixes never touch those values.
- rc-am22 touches the http-server SMOKE assertion + committed smoke log — smoke
  is a liveness check, not a timing. It gates whether a cell is *allowed* to
  measure, not what it measures.

Therefore: **fix-first is safe.** There is no "measure on record-era code then
fix" requirement for these five — the fixes are shape/tooling, the measurements
are timing. The ONE exception is rc-audm.8: it is *already* landed, so its
verification measurement inherently runs on post-fix code (that IS the point —
we are checking the landed fix converges the old-failing cells).

Each tooling fix lands as its own commit (per constraint), caveman-commit style
(`fix(bench): …`, `refactor(bench): …`, `test(bench): …`), `Bd: rc-xxx` footer.
No pushing (human-only).

---

## 7. Deliverable checklist for workers (per finding)

Every finding artifact MUST contain:
- [ ] Hypothesis under test + the tree branch being adjudicated.
- [ ] Pre/post loadavg + freq samples (noise gate §0.1).
- [ ] Toolchain + lock hashes (repro §0.5).
- [ ] Sample counts (rounds × samples) meeting §0.3.
- [ ] Host-vs-container comparability statement (§0.4) if node is involved.
- [ ] Verdict: fixture-defect | real-cost | park, with the threshold that
      decided it.
- [ ] Explicit statement that the official record was NOT re-run/republished.

---

## 8. Burst outcome (2026-09-11, worktree bench @ 3d8e3a03)

| Suspect | Disposition | Evidence / commit |
|---|---|---|
| rc-audm.1 | ROOT-CAUSED fixture defect | xmllint-wasm spawns a fresh worker (wasm compile+instantiate + schema parse) per validate; counters: 1001 spawns / 1000 iters. Hoisting = 39x p50 drop (41.8ms -> ~1.1ms host, shape-only). Fix `7029fd39`. Artifact /tmp/bench-probe-audm1/FINDING-audm1.md |
| rc-audm.2 | PARKED (no java on host) | Quarkus native RSS needs container. node +26.3MiB explained by audm.1 (wasm heap churn from per-tick workers). Methodology caveat note filed; record NOT re-run |
| rc-audm.3 | REAL-COST + record-ratio artifact | Fixtures PARITY by inspection (audit table on file). Host live: rust 18.4us (real EIP cost; ~1.5x vs record = container overhead), node ~450ns => record's 3x ratio is a node-container artifact. Attribution: 76 allocs + 20.6KiB per tick (counting allocator, observer-effect nil). 2nd optimization target confirmed |
| rc-audm.5 | ROOT-CAUSED, premise corrected | NO pipe in the cell (timer-driven route, in-process markers; read syscalls 0.18/tick). 20x = language-step cost: cli yaml uses js(boa)+rhai ~1.1ms + 580us json machinery vs lib native closure. Relabel record reading: language-step cost, not pipe tax |
| rc-audm.6 | PARKED (asymmetry proven anti-directional) | cli 1.7ms bracket strictly CONTAINS lib 3.0ms bracket => bracket asymmetry cannot explain cli < lib; wall-clock hypothesis refuted. Residual ~1.3ms = run-condition. Adjudicator: next canonical run |
| rc-audm.7 | LANDED | Native m2 status field in m2-summary.json; summarize prefers it, derives as fallback. `80f77d0b`. Era-2 record re-summarizes identically |
| rc-audm.8 | LIVE FINDING (parked with root cause) | node-native http m2 STILL unconverged on current code (4/5 rounds MessageBoundUnconverged; 5/14 stable in sweep). Root cause: stability window hard-capped at first max_messages (warmup.rs:145 second_end=min(n,max_messages)); 1000-msg bound fires ~40x before 30s wall at http rates; protocol A has no adaptive-extension path. Narrowed design: trailing-window time-based comparison (needs blessed-protocol change) |
| rc-dh7t | VERIFIED-LANDED + test | Skip notice in code; test_discover.py (2 tests); `0d3850c5`. Reopened for master landing review |
| rc-am22 | ROOT-CAUSED + fixed | Route strip (pre-6b3a31b5) carried verbatim by consolidation 8596f6c1. Emission restored, smoke PASS, log regenerated; `d70f289c` |
| rc-mr6u | AXIS LANDED, A/B PARKED | opt-in alloc-mimalloc feature committed `3d8e3a03` (OFF by default; clippy green both feature states). A/B unrun: scratch driver taskset cpu-list bug + host loadavg 12-17 (fleet) — inadmissible for m3. Binaries+script at /tmp/bench-probe-mr6u/ |
| rc-u034 | PARKED (scope) | axum-bare contender = roster-affecting addition (three-file contract + completeness rules) => own change. devnull 2-thread cap confirmed (rc-audm.4 starvation prior art) |
| rc-u047 | LANDED | identity->measured/attempted/None helper dedups presence logic. `08939861` |
| rc-2k33 | LANDED (documented-contract option) | CONTEXT.md contract + strengthened drift test enumerating three sources + 52-arithmetic pin. `2546fd37` |

Commits on branch bench (order): 0d3850c5, d70f289c, 7029fd39, 80f77d0b, 08939861, 2546fd37, 3d8e3a03 (+ this doc).
Harness python tests: 77/77 green after all changes. Official record 20260903T084658Z never re-run, never republished.
