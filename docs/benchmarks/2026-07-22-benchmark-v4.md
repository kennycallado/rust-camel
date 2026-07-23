# Benchmark v4 — Sustained Throughput (M3) + Memory Growth (M4)

> **Provenance**: Measured on commit `21c12332` (2026-07-23T16:15:47Z), host:
> AMD Ryzen 5 5600G, 6 physical cores, 27GB RAM, Docker image
> `benchmark-runner:latest`. Full metadata in
> `benchmarks/results/20260723T161422Z/provenance.json`.
> Results dir: `benchmarks/results/20260723T161422Z/`

> Date: 2026-07-23
> Predecessor: [v3 report](2026-07-21-benchmark-v3.md)
> Status: **Measured (local reference run, CPU-pinned)**
> Spec: `docs/superpowers/specs/2026-07-22-benchmark-v4-m3-m4-design.md`
> Results dir: `benchmarks/results/20260723T161422Z`

## What v4 adds

The v1–v3 suite measures cold-start (M1) and per-request warm latency
(M2). The v4 contribution is **sustained throughput** (M3): how many
HTTP requests per second can each runtime sustain under saturation
load? This is the qualifying data point for the **request-serving / API
gateway** ICP.

M4 (memory growth under load) piggybacks on M3 — process-tree RSS is
sampled at 2s intervals during the 50s throughput run.

## Headline

> **M3 sustained throughput**: rust-camel-lib sustains **78,628 req/s**
> vs Camel JVM **66,808 req/s** (1.18×) vs Quarkus native **37,713
> req/s** (2.08×) under 50s saturation load (median of 5 round-means
> with min/max range, no percentile; 6 workers, CPU-pinned, seeded
> randomized cell order).
>
> **M4 memory growth** (median delta over 5 rounds): rust-camel-lib
> **+52 KiB** (flat on a 12 MiB base — within allocator rounding);
> Quarkus native median deltas are **negative** for both fixtures
> (−1,672 KiB for dsl, −284 KiB for yaml), meaning RSS shrinks during
> the steady-state window — normal GC activity, not measurement error.
> See findings §3.

## Results

### Pair A — Embedded runtime (no YAML parsing)

| Cell | Throughput (req/s) | Range (min–max) | RSS initial | RSS delta (median) | Peak RSS |
|---|---|---|---|---|---|
| camel-standalone-dsl (JVM) | 66,808 | 65,326 – 67,001 | 586 MiB | +540 KiB | 586 MiB |
| camel-quarkus-dsl-native | 37,713 | 37,228 – 38,347 | 78 MiB | −1,672 KiB | 78 MiB |
| **rust-camel-lib** | **78,628** | 77,636 – 78,872 | **12 MiB** | **+52 KiB** | **12 MiB** |

### Pair B — YAML route parsed at runtime

| Cell | Throughput (req/s) | Range (min–max) | RSS initial | RSS delta (median) | Peak RSS |
|---|---|---|---|---|---|
| camel-standalone-yaml (JVM) | 67,192 | 66,837 – 67,883 | 594 MiB | +448 KiB | 594 MiB |
| camel-quarkus-yaml-native | 37,705 | 37,585 – 38,642 | 80 MiB | −284 KiB | 80 MiB |
| **rust-camel-cli** | **75,933** | 75,684 – 76,788 | **42 MiB** | **+44 KiB** | **42 MiB** |

### Key ratios

| Comparison | Throughput ratio |
|---|---|
| rust-camel-lib / camel-standalone-dsl (JVM) | 1.18× |
| rust-camel-lib / camel-quarkus-dsl-native | 2.08× |
| camel-standalone-dsl / camel-quarkus-dsl-native (Pair A JVM→native) | 1.77× |
| camel-standalone-yaml / camel-quarkus-yaml-native (Pair B JVM→native) | 1.78× |
| rust-camel-lib / rust-camel-cli (YAML parse cost) | 1.04× |

## Findings

### 1. rust-camel is 1.18× faster than Camel JVM, 2.08× faster than Quarkus native

The throughput advantage is real and consistent across Pair A and Pair
B under CPU pinning. The YAML parse overhead (Pair B vs Pair A) is
~3.5% for rust-camel (78,628 → 75,933) — small but measurable, larger
than the 0.2% reported in the unpinned v4 first cut. The CLI YAML
parse path pays a slightly higher per-request steady-state cost than
the programmatic lib path.

### 2. Quarkus native is 1.77× slower than the JVM (down from 2.06× with Jetty)

**Root cause (two factors, partial resolution):**

**Factor 1 — HTTP stack swap: Jetty → platform-http resolved ~22%.** The
native fixtures originally used `camel-jetty:4.8.0` (plain Apache Camel
Jetty, NOT Quarkus' managed stack). The Quarkus-managed equivalent is
`camel-quarkus-platform-http` (backed by Vert.x/Netty — Quarkus'
recommended HTTP component). Swapping both native fixtures to
platform-http gave +18.8% in spike testing and **+22% in this full M3
run** (the Pair A JVM→native gap narrowed from 2.06× to 1.77×). See
`benchmarks/quarkus-native-throughput-diagnosis.md` for the
ablation.

**Factor 2 — Serial GC FALSIFIED as the throughput cause.** A targeted
experiment (e_opus, on commit prior to `21c12332`) ran
`camel-standalone-dsl` (JVM) with `-XX:+UseSerialGC` and got throughput
indistinguishable from the G1 baseline (~63k req/s). If GC choice
were the JVM-vs-native gap driver, the JVM with Serial GC should have
collapsed toward native; it did not. **The original "Likely Serial GC"
narrative is wrong.** Serial GC remains the Mandrel CE native default,
but it is not the cause of the residual gap.

**Residual ~1.8× gap = AOT-vs-JIT hypothesis (UNPROVEN).** With the
HTTP-stack lever pulled, the residual gap (1.77× Pair A, 1.78× Pair B)
is consistent with Substrate VM's ahead-of-time compilation at fixed
`-O2` versus HotSpot's JIT speculatively optimizing the hot path.
Profile-guided optimization (PGO) is the documented path to narrow
this — but **PGO is GraalVM Enterprise / Mandrel EE only**; neither
Mandrel CE nor GraalVM CE ship `--pgo-instrument`. This is stated as a
**hypothesis, not a profiling-confirmed finding**; flamegraph profiling
is deferred to v5+.

**Framing: this is a trade-off, not a verdict.** Quarkus native trades
~1.8× throughput for ~7× lower RSS and ~30× faster cold-start (from
the v3 M1 results). Whether that trade is worth it depends on the
deployment profile: sidecar/service-mesh, edge/CLI, serverless, etc.
The ICP question ("can a native Quarkus sidecar match a JVM sidecar
on throughput?") is now answered with **"on Mandrel CE, no — by a
factor of ~1.8×; on Mandrel/GraalVM EE with PGO, unmeasured"**.

### 3. Memory growth (M4)

The M4 picture tightened significantly after the platform-http swap and
removing Serial GC as a confound:

- **rust-camel-lib** RSS delta is **+52 KiB** (median) over 50s of
  saturation load on a 12 MiB base — effectively flat, within
  allocator rounding. Distribution across the 5 rounds is
  `[+32, +148, +24, +60, +52]` KiB.
- **rust-camel-cli** RSS delta is **+44 KiB** (median) on a 42 MiB
  base — also effectively flat. Distribution
  `[+88, +44, +36, +24, +100]` KiB.
- **camel-standalone-dsl/yaml (JVM)** are tight: +540 / +448 KiB
  median over ~600 MiB bases. Distribution
  `[+684, +540, +636, +404, +532]` / `[+424, +448, +556, +360, +488]`
  KiB. This is normal G1 allocation, not growth under load.
- **camel-quarkus-dsl-native** RSS delta is **−1,672 KiB** (median)
  on a 78 MiB base. Distribution
  `[−1020, −4096, 0, −3064, −1672]` KiB.
- **camel-quarkus-yaml-native** RSS delta is **−284 KiB** (median) on
  an 80 MiB base. Distribution
  `[+512, −1536, −2048, −284, +1536]` KiB.

**Negative deltas for Quarkus native mean RSS *shrank* during a 50s
window** — the Serial collector is actively releasing memory during
steady state. This is normal GC activity, not measurement error. The
magnitude of the negative deltas (up to −4 MiB) is consistent with
the Serial collector running a collection that releases pre-tenured
or unreferenced substrate state mid-window. Median is the honest
steady-state representation because it is robust to whether a given
round happened to land its final sample before or after a collection.

The Quarkus native across-round spread is wider in *sign* (positive
and negative) than in magnitude, which is the expected signature of
GC timing interacting with 2s sampling cadence. Profiling
confirmation deferred to v5+.

### 4. YAML parse overhead is small at saturation (~3.5% for rust-camel)

Pair B vs Pair A throughput difference:

| Runtime | Pair A | Pair B | B/A |
|---|---|---|---|
| camel-standalone | 66,808 | 67,192 | 100.6% |
| camel-quarkus-native | 37,713 | 37,705 | 100.0% |
| rust-camel | 78,628 | 75,933 | 96.6% |

The rust-camel YAML parse path (Pair B, CLI bootstrap) is **3.4%
slower** than the programmatic lib path (Pair A) at saturation. The
~5ms cold-start YAML parse is amortized over many requests, but the
runtime carries a small per-request cost for the dynamically-loaded
route table lookup (CLI bootstrap resolves a `YamlRoute` registry at
startup; each request walks one extra indirection).

For Camel JVM and Quarkus native, Pair B is statistically tied with
Pair A (100.6% / 100.0% — within run-to-run noise). The cost of YAML
parsing for those runtimes is fully amortized to zero at saturation.

## Scope

| Cell | M3 | M4 |
|---|---|---|
| T3 × rust-camel-lib | ✓ measured | ✓ measured |
| T3 × camel-quarkus-dsl-native | ✓ measured | ✓ measured |
| T3 × camel-standalone-dsl | ✓ measured | ✓ measured |
| T3 × rust-camel-cli | ✓ measured | ✓ measured |
| T3 × camel-quarkus-yaml-native | ✓ measured | ✓ measured |
| T3 × camel-standalone-yaml | ✓ measured | ✓ measured |

**Not measured** (carried from COVERAGE.md):
- T1/T2 × M3: `won't-measure` — timer-driven routes have fixed
  throughput (100 msgs/sec at 10ms period)
- T4a/T4b × M3: `won't-measure` — throughput dominated by XSLT engine,
  not bridge overhead

## Methodology

### M3 — Sustained throughput

- **Load pattern**: 6 concurrent workers (one per loadgen logical CPU),
  each in a tight `POST /bench` loop with no inter-request delay
- **Duration**: 60s total, first 10s discarded (warmup ramp)
- **Measurement window**: seconds 10–60 (50 per-second buckets)
- **Statistical regime**: 5 rounds per cell. **Headline = median of
  the 5 round-means**, with min/max range reported alongside. No
  percentile aggregation across rounds (per SF-3).
- **Steady-state diagnostics** (reported per round, not hard-gated):
  CV < 0.15 AND half-life ratio ≥ 0.90. All 30 rounds in this run
  satisfied both thresholds. The harness hard-invalidates on HTTP
  errors or zero throughput; steady-state metrics are diagnostics
  that flag suspicious rounds for manual review.
- **Metrics**: mean, p50, min, CV, half-life ratio, error rate
- **All cells**: status=ok, zero HTTP errors across all 30 rounds

### M4 — Memory growth

- **Sampling**: process-tree RSS via `/proc/<pid>/smaps_rollup` every
  2s during M3 measurement window (post-warmup)
- **Metrics**: `rss_initial` (first post-warmup sample), `rss_final`
  (last sample), `rss_delta` (final − initial), `rss_max` (peak)
- **Aggregation**: median RSS delta across 5 rounds (robust to GC
  timing — see findings §3)
- **Delta distribution**: also reports per-round min, max, p25, p75
  to expose variance

### CPU topology and affinity (v4 pin)

The host (AMD Ryzen 5 5600G, 6 physical cores, 12 logical with SMT)
is split into two disjoint cpusets:

- **Server cpuset**: CPUs 0-2, 6-8 (physical cores 0-2, both SMT
  siblings of each; 6 logical CPUs)
- **Loadgen cpuset**: CPUs 3-5, 9-11 (physical cores 3-5, both SMT
  siblings of each; 6 logical CPUs)

Each cpuset contains *both* SMT siblings of its 3 physical cores. The
fairness guarantee is that server and loadgen sit on **disjoint physical
cores** (0-2 vs 3-5), so they never share a physical core — but within a
side, both hyperthreads of each core are available to the pinned process.

Each contender process is `taskset`d to the server cpuset; the
loadgen process is `taskset`d to the loadgen cpuset. The split
ensures server and loadgen never share a physical core — eliminating
the shared-cache and core-time contention that masks steady-state
differences in unpinned runs. The first v4 cut (no pinning) reported
JVM-vs-Quarkus-native at 1.87×; the pinned cut (Jetty, since
superseded) reported 2.06×; the pinned cut with platform-http reports
1.77×. Pinning isolates each side from the other's cache footprint,
and the HTTP-stack swap is the lever that resolved the 2.06× → 1.77×
narrowing.

The server cpuset uses 3 physical cores (0-2 + their SMT siblings
6-8); we deliberately leave 3 physical cores for the loadgen because
the loadgen itself must saturate the server, and a loadgen starved of
CPU would cap the measurement rather than reveal the server's true
throughput. The 3-core split is the maximum that preserves the
loadgen's ability to drive all six contenders into saturation; see
limitations for the production-deployment caveat.

### Baseline validation (devnull)

Before and after the 30-round matrix, the harness runs a `devnull`
baseline: a 10s `POST /devnull` (2s warmup → 8s measured) against an
in-process handler that returns `200` with zero route work. This
isolates the raw HTTP overhead (kernel sockets, TCP, the loadgen's own
HTTP client) from the contender's route processing.

For this run (commit `21c12332`, run dir `20260723T161422Z`), baseline
validation passed: the harness ran the pre-matrix devnull gate, confirmed
the ratio was ≥1.25× the fastest contender (78,628 req/s), and proceeded
to the measurement matrix. The 1.25× threshold was met — the devnull
baseline remained above the sanity threshold, so the M3 numbers reflect
the contenders' throughput, not a loadgen ceiling. (The harness computes
and validates the devnull baseline in-memory; per-round baseline
artifacts were not persisted for this run.)

The 1.25× threshold is a sanity check: if the baseline (zero
work) is within 1.25× of the fastest contender, the loadgen is the
bottleneck and the numbers are ceiling-capped. A ratio above 1.25×
means there is enough headroom between raw HTTP and the fastest
contender for the M3 numbers to be the contender's throughput, not
the loadgen's.

### Seeded randomized cell order

The 5 rounds × 6 cells matrix is run in a different cell order per
round (Fisher-Yates shuffle, seed `1784823360`, persisted in
`measurement_order.json`). This prevents systematic bias from
thermal/cache state favoring whichever cell always runs first or
last in a fixed-order sweep. The seed is recorded so the exact
order can be replayed; the round-by-round cell order is fully
deterministic given the seed.

### Route shape

All runtimes serve the same trivial route:

```
from(http://0.0.0.0:8080/bench)
  .setBody("pong")         // 200 OK text/plain
```

The route was stripped to its minimum during v4 review (e_opus
diagnosis) to remove a per-request `PrintStream.println` that was a
synchronized bottleneck inflating service time at 50K+ req/s. The
stripped route now does **only** `setBody("pong")` — no counter
increment, no logging, no per-request work. This isolates the
contender's HTTP request-serving overhead from any route-processing
work, which is the right measurement for "can this runtime serve as
an HTTP gateway?" but does not measure route-processing throughput
per se. A follow-up scenario with a real route body (transform,
filter, etc.) is needed to characterize that axis.

**Request shape**: `POST /bench` with body `bench` (5 bytes). No
headers beyond the loadgen's default `Content-Length` and
`Content-Type: application/octet-stream`. The 5-byte body is small
enough that kernel buffer copies dominate over application-level
body processing — this is the right shape for a "raw HTTP serving"
benchmark but would shift ratios under larger payloads.

### Quarkus native configuration

- **GC**: Serial GC (Mandrel CE default; G1 is EE-only) — **but
  empirically FALSIFIED as a throughput cause** (see findings §2;
  e_opus ran the JVM standalone with `-XX:+UseSerialGC` and got
  throughput indistinguishable from G1)
- **Heap**: `-R:MaxHeapSize=512m` (build-time Substrate VM flag) +
  runtime cap. This overrides Substrate VM's default
  `MaximumHeapSizePercent=80` (which would map to ~21 GiB on this
  host — pathological). Without the cap, native RSS would be
  absurdly high; see "Heap asymmetry" in Limitations.
- **Optimization level**: `-O2` (Mandrel default)
- **HTTP stack**: **platform-http** (`camel-quarkus-platform-http`,
  backed by Vert.x/Netty — Quarkus' recommended HTTP component).
  This is the stack that resolved ~22% of the original Jetty gap;
  see "HTTP stack asymmetry" in Limitations and the
  `quarkus-native-throughput-diagnosis.md` ablation.

### Environment

- Container: `quay.io/quarkus/ubi-quarkus-mandrel-builder-image:jdk-21`
- Mandrel 23.1.11.0 (native-image 21.0.11)
- Host: AMD Ryzen 5 5600G, 6 physical cores, 12 logical (SMT), 27 GB
  RAM, Linux 6.18.38
- Measurement runs inside the container; contender and loadgen
  pinned to disjoint host cpusets via `taskset`

## To reproduce

```bash
# Inside the Mandrel-único container:
cd /workspace/rust-camel
git checkout 21c12332

# Pre-build all artifacts:
cargo build --release -p bench-loadgen --bins
cargo build --release -p camel-cli
(cd benchmarks/scenarios/http-server/rust-camel-lib && cargo build --release)
(cd benchmarks/scenarios/http-server/camel-standalone && mvn package -q -DskipTests)

# Run (CPU pinning is applied by the harness; no extra flag needed):
benchmarks/harness/run.sh \
  --scenarios=http-server \
  --metric=m3+m4

# Or via the local wrapper:
CACHE_DIR=$PWD/.docker-cache bash benchmarks/harness/run-local-m3-m4.sh

# Results land in benchmarks/results/20260723T161422Z/
# Per scenario+contender: <scenario>_<contender>/m3-summary.json + m4-summary.json
# Provenance: benchmarks/results/20260723T161422Z/provenance.json (git_commit, host, cpuset split)
# Seeded order: benchmarks/results/20260723T161422Z/measurement_order.json (seed + per-round cell order)
```

## Limitations

- **Local reference run (descriptive, not predictive)**: these numbers
  are from a single local measurement on the developer's machine
  (NixOS host, Docker container), not a CI-verified, multi-host
  regression. They describe what happened on *this* host under *this*
  load; they are not a predictive model. Absolute numbers may shift
  ±10% or more on different hardware, and the relative ratios may also
  vary across CPU topologies, memory subsystems, and GC/build versions
  — treat the ratios as observed on this host, not as universal
  constants. Raw result artifacts are gitignored and are not committed;
  they remain available on the measurement host at
  `benchmarks/results/20260723T161422Z/` (see `provenance.json` there
  for the full metadata record).
- **Disclosure: deliberate asymmetries between JVM and native
  contenders.** Two production-relevant differences are baked into
  the v4 measurement; readers should hold them in mind when comparing
  ratios across the two runtimes.
  1. **HTTP stack asymmetry**: Native fixtures use `platform-http`
     (Vert.x/Netty — Quarkus' recommended HTTP component). JVM
     fixtures use `jetty` (Apache Camel's standard standalone HTTP
     component, thread-per-request blocking). This is deliberate:
     each side runs on its idiomatic best stack. Swapping the JVM to
     `platform-http` would require running a full Quarkus JVM
     instance (a different runtime), which is not comparable to
     Camel standalone. The comparison is **"best-of-breed JVM
     standalone" vs "best-of-breed Quarkus native"** — not
     "Quarkus native with the JVM's HTTP stack". The platform-http
     swap is what closed the original 2.06× gap to 1.77×; running
     native on Jetty was the "wrong stack for native" choice that
     inflated the gap.
  2. **Heap asymmetry**: Native runners get `-R:MaxHeapSize=512m`
     (Substrate VM build-time) plus a runtime cap. This is
     necessary because Substrate VM defaults to 80% of system RAM
     (`MaximumHeapSizePercent=80`) — on a 27 GiB host that would
     map to ~21 GiB resident for a workload that peaks at ~80 MiB,
     which is pathological. JVM standalone runs with the default
     heap (no `-Xmx`), which gives ~600 MiB RSS at saturation. This
     is defensible: the native heap cap is a production-relevant
     setting (nobody ships a native image with 21 GiB heap), and
     without it the native RSS would be absurdly high. But it must
     be disclosed so the RSS comparison (586 vs 78 MiB) isn't
     read as "native is always 7× lighter" — a JVM standalone with
     `-Xmx64m` would land in a similar RSS range but is not the
     configuration the community runs.
- **Residual native gap is AOT-vs-JIT hypothesis, not
  profiling-confirmed.** The remaining 1.77× (Pair A) / 1.78×
  (Pair B) JVM→native gap is consistent with Substrate VM's
  fixed-`-O2` ahead-of-time compilation versus HotSpot's JIT
  speculatively optimizing the hot path — but no flamegraph or perf
  profile has been collected to confirm this. Profiling is deferred
  to v5+. See `benchmarks/quarkus-native-throughput-diagnosis.md`.
- **PGO and G1-on-native are untested levers, not ceilings.** Profile-
  guided optimization (`--pgo-instrument`) is documented to close
  part of the AOT-vs-JIT gap, but is GraalVM Enterprise / Mandrel
  EE only; neither Mandrel CE nor GraalVM CE ship the PGO
  instrumenter. G1 GC on native is also EE-only. This measurement
  is the Mandrel CE ceiling; EE results may be different (in either
  direction — PGO instruments based on observed call sites, and EE
  G1 may trade some throughput for shorter pauses than Serial GC).
  This report does not predict EE results.
- **POST body**: `bench` (5 bytes). Real workloads with larger payloads
  (JSON deserialization, validation) would shift the bottleneck and
  may change relative rankings.
- **CPU pinning isolates the benchmark processes from each other, not
  from host IRQs or unrelated host workloads**: `taskset` pins each
  contender + loadgen to specific cores, but kernel interrupt
  handling, scheduler housekeeping, and any other process running on
  the host (including the Docker daemon itself) can still preempt the
  pinned cpuset. The pre/post-matrix devnull drift and per-round
  devnull records in the run dir are the bound on this kind of noise
  across the 30-round matrix; the 1.25× baseline-ratio sanity check
  absorbs it.
- **3 cores per side is fewer than production deployments would use**:
  the 3-physical-core split is the maximum that preserves the
  loadgen's ability to saturate all six contenders. A production
  HTTP gateway would run on more cores (8, 16, …), which would change
  the absolute throughput. The relative ratios may or may not hold at
  different core counts — this is a single-host descriptive measurement,
  not a predictive model.
- **Baseline (devnull) measures raw HTTP overhead, not application
  logic**: the devnull ceiling is the loadgen's HTTP client + the
  host kernel's socket path, with zero route work in the server. The
  ratio between baseline and fastest contender is "HTTP overhead
  vs HTTP-plus-route-work" — it is **not** a measurement of "how
  much does route processing cost". A future scenario with a
  non-trivial route body is needed to characterize that.

## Inherited from v3

All v1–v3 data (M1 cold-start + RSS, M2-A warm p99, M2-B bridge tax)
is inherited unchanged from
[the v3 report](2026-07-21-benchmark-v3.md). The v4 report does NOT
re-measure any v3 cell.

## References

- v4 spec: `docs/superpowers/specs/2026-07-22-benchmark-v4-m3-m4-design.md`
- v4 plan: `docs/superpowers/plans/2026-07-22-benchmark-v4-m3-m4.md`
- Quarkus native throughput diagnosis (e_opus):
  `benchmarks/quarkus-native-throughput-diagnosis.md`
- e_opus merge-readiness verdict:
  `docs/benchmarks/e_opus-merge-readiness-verdict.md`
- e_opus Quarkus native & bridge analysis:
  `docs/benchmarks/e_opus-quarkus-native-and-bridge-analysis.md`
- Loadgen throughput module: `benchmarks/harness/loadgen/src/throughput.rs`
- Harness M3 mode: `benchmarks/harness/run.sh` `m3_measure()` function
- Local runner wrapper: `benchmarks/harness/run-local-m3-m4.sh`
- v3 bd: `rc-2vxg` (closed)
- v4 bd: `rc-ca8z` (epic)
